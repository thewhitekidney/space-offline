"""Tests for the census harness (packet CEN-1, `docs/guides/census.md`).

Stdlib unittest on synthetic bytes and invented rows only: no TUN device is
created, nothing runs as root, no client is launched, and no packet leaves
this host (the proxy test listens on 127.0.0.1 with an ephemeral port and
talks to itself). Addresses are from the documentation ranges and names
end in `.invalid`.
"""

import ast
import contextlib
import io
import importlib.util
import json
import os
import pathlib
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import unittest

TOOLS = pathlib.Path(__file__).resolve().parent
if str(TOOLS) not in sys.path:
    sys.path.append(str(TOOLS))


def _load(name):
    spec = importlib.util.spec_from_file_location(name, TOOLS / f"{name}.py")
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


wire = _load("census_wire")
sink = _load("census_sink")
canary = _load("census_canary")
report = _load("census_report")
proxy = _load("census_proxy")

ME4, PEER4 = "198.18.0.2", "198.51.100.7"
ME6, PEER6 = "fd00:ce:5::2", "2001:db8:77::1"
BANNED = {"socket", "ssl", "http", "urllib", "subprocess", "ctypes"}


def client_hello(name: str) -> bytes:
    """A minimal TLS 1.2 ClientHello carrying one server_name."""
    host = name.encode()
    sni = struct.pack("!HBH", len(host) + 3, 0, len(host)) + host
    exts = struct.pack("!HH", 0x000A, 4) + b"\x00\x02\x00\x17"  # an extension before SNI
    exts += struct.pack("!HH", 0, len(sni)) + sni
    body = b"\x03\x03" + bytes(32) + b"\x00" + b"\x00\x02\x13\x01" + b"\x01\x00"
    body += struct.pack("!H", len(exts)) + exts
    hs = b"\x01" + len(body).to_bytes(3, "big") + body
    return b"\x16\x03\x01" + struct.pack("!H", len(hs)) + hs


class Wire(unittest.TestCase):
    def test_v4_tcp_syn_parses(self):
        pkt = wire.build_tcp(ME4, PEER4, 40000, 443, 1000, 0, wire.TCP_SYN)
        p = wire.parse_packet(pkt)
        self.assertEqual((p["v"], p["proto"], p["src"], p["dst"], p["sport"], p["dport"], p["flags"], p["seq"]),
                         (4, "tcp", ME4, PEER4, 40000, 443, "S", 1000))
        self.assertTrue(wire.verify_checksums(pkt))

    def test_v6_tcp_syn_parses_through_an_extension_header(self):
        seg = wire.build_tcp(ME6, PEER6, 40001, 8443, 5, 0, wire.TCP_SYN)[40:]
        hop = bytes([wire.PROTO_TCP, 0]) + bytes(6)  # hop-by-hop, 8 bytes, next = TCP
        pkt = wire.build_ipv6(ME6, PEER6, 0, hop + seg)
        p = wire.parse_packet(pkt)
        self.assertEqual((p["v"], p["proto"], p["dst"], p["dport"], p["flags"]), (6, "tcp", PEER6, 8443, "S"))

    def test_udp_and_icmp_parse(self):
        u = wire.parse_packet(wire.build_udp(ME4, PEER4, 5353, 53, wire.dns_query("x.census.invalid")))
        self.assertEqual((u["proto"], u["dport"], wire.dns_qname(u["payload"])), ("udp", 53, "x.census.invalid"))
        i4 = wire.parse_packet(wire.build_icmp(ME4, PEER4, 8, 0, b"\x00\x01\x00\x02ping"))
        self.assertEqual((i4["proto"], i4["type"], i4["code"]), ("icmp", 8, 0))
        i6pkt = wire.build_icmp(ME6, PEER6, 128, 0, b"\x00\x01\x00\x02")
        i6 = wire.parse_packet(i6pkt)
        self.assertEqual((i6["proto"], i6["type"]), ("icmpv6", 128))
        self.assertTrue(wire.verify_checksums(i6pkt))

    def test_garbage_raises_parse_error(self):
        for bad in (b"", b"\x45" + bytes(5), b"\x70" + bytes(40)):
            with self.assertRaises(wire.ParseError):
                wire.parse_packet(bad)

    def test_checksum_matches_a_known_good_pair(self):
        # The textbook IPv4 header (RFC 1071 style worked example): its
        # checksum field holds 0xb861, and the header sums to zero.
        hdr = bytes.fromhex("450000730000400040110000c0a80001c0a800c7")
        self.assertEqual(wire.checksum(hdr), 0xB861)
        self.assertEqual(wire.checksum(hdr[:10] + b"\xb8\x61" + hdr[12:]), 0)

    def test_every_built_packet_verifies(self):
        for src, dst in ((ME4, PEER4), (ME6, PEER6)):
            for pkt in (wire.build_tcp(src, dst, 1, 2, 3, 4, wire.TCP_ACK, b"odd"),
                        wire.build_udp(src, dst, 1, 2, b"abcde")):
                self.assertTrue(wire.verify_checksums(pkt), (src, pkt.hex()))

    def test_sni_host_and_qname(self):
        self.assertEqual(wire.tls_sni(client_hello("Hello.Census.Invalid")), "hello.census.invalid")
        self.assertIsNone(wire.tls_sni(b"\x16\x03\x01\x00\x01\x02"))
        req = b"GET /a HTTP/1.1\r\nUser-Agent: x\r\nHost: Web.Census.Invalid:8080\r\n\r\n"
        self.assertEqual(wire.http_request(req), ("GET", "/a", "web.census.invalid:8080"))
        self.assertEqual(wire.http_request(b"CONNECT t.census.invalid:443 HTTP/1.1\r\n\r\n")[2],
                         "t.census.invalid:443")
        self.assertEqual(wire.http_request(b"GET http://a.census.invalid/x HTTP/1.0\r\n\r\n")[2],
                         "a.census.invalid")
        self.assertIsNone(wire.http_request(b"\x16\x03\x01junk"))
        self.assertEqual(wire.payload_name(client_hello("s.census.invalid"), 443), "s.census.invalid")
        self.assertEqual(wire.payload_name(wire.dns_query("q.census.invalid"), 53), "q.census.invalid")


class Handshake(unittest.TestCase):
    def _synack(self, me, peer):
        syn = wire.build_tcp(me, peer, 40000, 443, 7000, 0, wire.TCP_SYN)
        (reply,) = wire.respond(wire.parse_packet(syn))
        r = wire.parse_packet(reply)
        self.assertTrue(wire.verify_checksums(reply))
        self.assertEqual((r["src"], r["dst"], r["sport"], r["dport"]), (peer, me, 443, 40000))
        self.assertEqual((r["flags"], r["ack"]), ("SA", 7001))
        opts = r["_l4"][20:24]
        self.assertEqual(opts, struct.pack("!BBH", 2, 4, wire.HANDSHAKE_MSS))
        return r

    def test_syn_ack_round_trips_v4_and_v6(self):
        self._synack(ME4, PEER4)
        self._synack(ME6, PEER6)

    def test_first_data_segment_is_acked_then_reset(self):
        sa = self._synack(ME4, PEER4)
        data = wire.build_tcp(ME4, PEER4, 40000, 443, 7001, sa["seq"] + 1, wire.TCP_ACK | wire.TCP_PSH,
                              client_hello("d.census.invalid"))
        ack, rst = (wire.parse_packet(x) for x in wire.respond(wire.parse_packet(data)))
        n = len(client_hello("d.census.invalid"))
        self.assertEqual((ack["flags"], ack["ack"], ack["seq"]), ("A", 7001 + n, sa["seq"] + 1))
        self.assertEqual((rst["flags"], rst["seq"]), ("R", sa["seq"] + 1))

    def test_echo_requests_are_answered_and_udp_is_not(self):
        (r4,) = wire.respond(wire.parse_packet(wire.build_icmp(ME4, PEER4, 8, 0, b"\x00\x09\x00\x01hi")))
        p4 = wire.parse_packet(r4)
        self.assertEqual((p4["type"], p4["dst"], p4["payload"]), (0, ME4, b"\x00\x09\x00\x01hi"))
        self.assertTrue(wire.verify_checksums(r4))
        (r6,) = wire.respond(wire.parse_packet(wire.build_icmp(ME6, PEER6, 128, 0, b"\x00\x09\x00\x01")))
        self.assertEqual(wire.parse_packet(r6)["type"], 129)
        self.assertTrue(wire.verify_checksums(r6))
        self.assertEqual(wire.respond(wire.parse_packet(wire.build_udp(ME4, PEER4, 1, 53, b"q"))), [])
        rst = wire.build_tcp(ME4, PEER4, 1, 2, 3, 0, wire.TCP_RST)
        self.assertEqual(wire.respond(wire.parse_packet(rst)), [])

    def test_observe_mode_never_replies(self):
        syn = wire.build_tcp(ME4, PEER4, 40000, 443, 1, 0, wire.TCP_SYN)
        line, replies = sink.handle(syn, "observe", 5)
        self.assertEqual((replies, line["replied"], line["flags"]), ([], 0, "S"))
        line, replies = sink.handle(syn, "handshake", 5)
        self.assertEqual(len(replies), 1)

    def test_payload_is_capped_and_bad_packets_logged(self):
        big = wire.build_udp(ME4, PEER4, 1, 9999, b"x" * 5000)
        line = sink.record(big, 1)
        self.assertEqual((line["plen"], len(line["payload"])), (5000, 2 * sink.PAYLOAD_CAP))
        self.assertIn("bad", sink.record(b"\x60\x00", 1))

    def test_replay_writes_the_log_and_the_replies(self):
        pkts = [(1000, wire.build_tcp(ME4, PEER4, 40000, 443, 1, 0, wire.TCP_SYN)),
                (1001, wire.build_udp(ME6, PEER6, 5, 53, wire.dns_query("r.census.invalid")))]
        with tempfile.TemporaryDirectory() as d:
            pcap, out, rep = (os.path.join(d, n) for n in ("in.pcap", "tun.jsonl", "out.pcap"))
            pathlib.Path(pcap).write_bytes(sink.write_pcap(pkts))
            self.assertEqual(sink.main(["--replay", pcap, "--out", out, "--mode", "handshake",
                                        "--replies", rep]), 0)
            lines = [json.loads(x) for x in pathlib.Path(out).read_text().splitlines()]
            self.assertEqual([x.get("event") for x in lines], ["start", None, None, "stop"])
            self.assertEqual((lines[1]["t"], lines[1]["dst"], lines[2]["proto"]), (1000, PEER4, "udp"))
            (ts, reply), = list(sink.read_pcap(pathlib.Path(rep).read_bytes()))
            self.assertEqual(wire.parse_packet(reply)["flags"], "SA")

    def test_attach_refuses_any_other_device(self):
        with self.assertRaises(SystemExit):
            sink.attach("eth0")


class NoSocketInTheSink(unittest.TestCase):
    def _imports(self, name):
        tree = ast.parse((TOOLS / f"{name}.py").read_text())
        mods = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                mods.update(a.name.split(".")[0] for a in node.names)
            elif isinstance(node, ast.ImportFrom) and node.module:
                mods.add(node.module.split(".")[0])
            elif isinstance(node, ast.Call) and getattr(node.func, "id", "") == "__import__":
                mods.add("__import__")
        return mods

    def test_the_sink_and_its_wire_module_import_nothing_that_talks(self):
        for name in ("census_sink", "census_wire"):
            mods = self._imports(name)
            self.assertFalse(mods & (BANNED | {"__import__", "importlib"}), (name, mods & BANNED))
        self.assertIn("census_wire", self._imports("census_sink"))

    def test_loading_the_sink_loads_no_banned_module(self):
        code = ("import sys; sys.path.append(sys.argv[1]); import census_sink; "
                "print(sorted(m for m in sys.modules if m.split('.')[0] in %r))" % (sorted(BANNED),))
        r = subprocess.run([sys.executable, "-I", "-c", code, str(TOOLS)],
                           capture_output=True, text=True, timeout=60)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(r.stdout.strip(), "[]")


class Proxy(unittest.TestCase):
    def test_every_bind_uses_the_loopback_constant(self):
        self.assertEqual(proxy.BIND_HOST, "127.0.0.1")
        tree = ast.parse((TOOLS / "census_proxy.py").read_text())
        binds = [n for n in ast.walk(tree) if isinstance(n, ast.Call)
                 and isinstance(n.func, ast.Attribute) and n.func.attr == "bind"]
        self.assertTrue(binds)
        for b in binds:
            arg = b.args[0]
            self.assertIsInstance(arg, ast.Tuple)
            self.assertEqual(getattr(arg.elts[0], "id", None), "BIND_HOST")
        self.assertFalse([n for n in ast.walk(tree) if isinstance(n, ast.Call)
                          and isinstance(n.func, ast.Attribute) and n.func.attr in ("connect", "create_connection")])

    def test_classify_requests(self):
        c = proxy.classify(b"CONNECT a.census.invalid:443 HTTP/1.1\r\nHost: a.census.invalid:443\r\n\r\n")
        self.assertEqual((c["kind"], c["host"]), ("connect", "a.census.invalid:443"))
        self.assertEqual(proxy.classify(b"GET http://192.0.2.99/x HTTP/1.1\r\n\r\n")["kind"], "absolute")
        o = proxy.classify(b"POST /api/0/store/ HTTP/1.1\r\nHost: 127.0.0.1:9\r\n\r\n")
        self.assertEqual((o["kind"], o["host"]), ("origin", "127.0.0.1:9"))
        self.assertEqual(proxy.classify(client_hello("t.census.invalid"))["host"], "t.census.invalid")
        self.assertEqual(proxy.classify(b"\x00\x01")["kind"], "not-http")
        self.assertEqual(proxy.classify(b"")["kind"], "empty")

    def test_it_logs_and_answers_403_on_loopback(self):
        with tempfile.TemporaryDirectory() as d:
            out = os.path.join(d, "proxy.jsonl")
            p = proxy.Proxy(out, 0)
            self.assertEqual(p.sock.getsockname()[0], "127.0.0.1")
            th = threading.Thread(target=p.serve, kwargs={"max_conns": 1}, daemon=True)
            th.start()
            with socket.create_connection(("127.0.0.1", p.port), timeout=5) as c:
                c.sendall(b"CONNECT b.census.invalid:443 HTTP/1.1\r\n\r\n")
                got = c.recv(200)
            th.join(5)
            for _ in range(50):
                if pathlib.Path(out).read_text():
                    break
                threading.Event().wait(0.05)
            p.close()
            self.assertTrue(got.startswith(b"HTTP/1.1 403"))
            rec = json.loads(pathlib.Path(out).read_text().splitlines()[0])
            self.assertEqual((rec["kind"], rec["host"]), ("connect", "b.census.invalid:443"))


class Shim(unittest.TestCase):
    NAMES = {"xdg-open", "x-www-browser", "sensible-browser", "firefox", "chromium", "google-chrome",
             "konqueror", "gio", "kde-open", "mozilla", "netscape", "galeon", "opera", "dillo"}

    def test_every_launcher_name_is_the_shim(self):
        bindir = TOOLS / "census" / "bin"
        have = {p.name for p in bindir.iterdir()}
        self.assertLessEqual(self.NAMES, have)
        for p in bindir.iterdir():
            self.assertTrue(p.is_symlink(), p)
            self.assertEqual(os.readlink(p), "../browser-shim")

    def test_the_shim_logs_argv_and_fails(self):
        args = ["https://e.census.invalid/a\tb", "back\\slash", "two\nlines"]
        with tempfile.TemporaryDirectory() as d:
            log = os.path.join(d, "exec.log")
            r = subprocess.run([str(TOOLS / "census" / "bin" / "xdg-open"), *args],
                               env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "CENSUS_EXEC_LOG": log},
                               capture_output=True, timeout=30)
            self.assertEqual(r.returncode, 1)
            (ev,) = report.parse_exec_log(pathlib.Path(log).read_text().splitlines())
        self.assertEqual((ev["name"], ev["args"]), ("xdg-open", args))
        self.assertGreater(ev["t"], 10 ** 12)


class Canary(unittest.TestCase):
    ROUTE4 = ("Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\n"
              "{dev}\t00000000\t00000000\t0001\t0\t0\t0\t00000000\t0\t0\t0\n"
              "eve-ns\t0089000A\t00000000\t0001\t0\t0\t0\tFCFFFFFF\t0\t0\t0\n")
    ROUTE6 = ("{z} 00 {z} 00 {z} 00000400 00000001 00000000 00000001 {dev}\n"
              "{z} 00 {z} 00 {z} ffffffff 00000001 00000000 00200200 lo\n")

    def _r(self, dev4, dev6):
        z = "0" * 32
        return self.ROUTE4.format(dev=dev4), self.ROUTE6.format(z=z, dev=dev6)

    def test_the_guard_wants_both_defaults_on_census0(self):
        self.assertIsNone(canary.guard(*self._r("census0", "census0")))
        self.assertIn("v4", canary.guard(*self._r("eth0", "census0")))
        self.assertIn("v6", canary.guard(*self._r("census0", "eth0")))
        self.assertIn("v4", canary.guard("Iface\n", ""))

    def test_run_refuses_outside_census_mode_and_sends_nothing(self):
        saved = canary._read, canary.fire
        canary._read = lambda path: ""
        canary.fire = lambda row: self.fail("fired outside census mode")
        try:
            with tempfile.TemporaryDirectory() as d:
                out = os.path.join(d, "c.jsonl")
                self.assertEqual(canary.main(["run", "--out", out]), 2)
                self.assertFalse(os.path.exists(out))
        finally:
            canary._read, canary.fire = saved

    def test_every_target_is_unownable(self):
        import ipaddress
        nets = [ipaddress.ip_network(n) for n in ("192.0.2.0/24", "2001:db8::/32")]
        for cid, row in canary.CANARIES.items():
            for t in row["targets"]:
                if t.endswith(".census.invalid") or t.startswith(canary.JAIL_HOST + ":"):
                    continue
                self.assertTrue(any(ipaddress.ip_address(t) in n for n in nets), (cid, t))
            for role in canary.ROLES:
                self.assertIn(role, row["expect"])

    def test_marks_make_windows(self):
        with tempfile.TemporaryDirectory() as d:
            out = os.path.join(d, "c.jsonl")
            canary.main(["mark", "start", "wine", "--out", out])
            canary.main(["mark", "end", "wine", "--out", out])
            canary.main(["mark", "start", "client", "--out", out])
            log = [json.loads(x) for x in pathlib.Path(out).read_text().splitlines()]
        (s, e), (s2, e2) = report.windows_of(log)
        self.assertLessEqual(s, e)
        self.assertEqual(e2, float("inf"))


# ----------------------------------------------------------------- the report

SERVER = "game.example.invalid"
PATHS = {
    "X1": {"what": "an invented telemetry host", "names": ["tele.census.invalid"]},
    "X2": {"what": "an invented glob", "names": ["*-gw.census.invalid"]},
    "X3": {"what": "the browser hand-off", "exec": True},
    "X4": {"what": "a crash upload", "crash": True, "names": ["crash.census.invalid"]},
}


def nft_doc(elems_by_set, wrapped=True):
    items = [{"metainfo": {"json_schema_version": 1}}, {"table": {"family": "inet", "name": "eve_census"}}]
    for name, elems in elems_by_set.items():
        out = []
        for dst, key in elems:
            val = {"concat": [dst, key]}
            out.append({"elem": {"val": val, "counter": {"packets": 1, "bytes": 60}}} if wrapped else val)
        items.append({"set": {"family": "inet", "name": name, "table": "eve_census", "elem": out}})
    return json.dumps({"nftables": items})


class Run:
    """A synthetic run directory: canaries fired as `role` expects, plus
    whatever a test adds."""

    def __init__(self, role="subject", t0=1_000_000):
        self.role, self.t0 = role, t0
        self.tun = [{"event": "start", "t": t0, "mode": "observe"}]
        self.sets = {"tcp4": [], "tcp6": [], "udp4": [], "udp6": [], "oth4": [], "oth6": []}
        self.dns, self.execs, self.proxy, self.crash = [], [], [], []
        self.canary = [{"event": "window-start", "window": "all", "t": t0}]
        self.stopped = True
        self._fire_canaries()
        self.canary.append({"event": "window-end", "window": "all", "t": t0 + 500})

    def pkt(self, pkt, t, r2=True):
        rec = sink.record(pkt, t)
        self.tun.append(rec)
        if r2:
            fam = str(rec["v"])
            if rec["proto"] == "tcp" and rec["flags"] == "S":
                self.sets["tcp" + fam].append((rec["dst"], rec["dport"]))
            elif rec["proto"] == "udp":
                self.sets["udp" + fam].append((rec["dst"], rec["dport"]))
            elif rec["proto"] in ("icmp", "icmpv6"):
                self.sets["oth" + fam].append((rec["dst"], rec["proto"] if fam == "4" else "ipv6-icmp"))

    def syn(self, dst, port, t, r2=True):
        src = ME6 if ":" in dst else ME4
        self.pkt(wire.build_tcp(src, dst, 40000, port, 1, 0, wire.TCP_SYN), t, r2)

    def _fire_canaries(self):
        t = self.t0 + 10
        self.syn("192.0.2.10", 443, t)
        self.syn("2001:db8::10", 443, t)
        self.pkt(wire.build_udp(ME4, "192.0.2.53", 5000, 53, wire.dns_query("canary-dns.census.invalid")), t)
        self.sets["tcp4"].append((canary.JAIL_HOST, canary.DROP_PORT))
        self.pkt(wire.build_icmp(ME4, "192.0.2.7", 8, 0, b"\x00\x01\x00\x01"), t)
        self.dns.append(f"query canary-wine.census.invalid. A NXDOMAIN peer=10.137.0.2:5000 t={t}")
        self.syn("192.0.2.11", 443, t)
        self.syn("2001:db8::11", 443, t)
        self.dns.append("query canary-py.census.invalid. A NXDOMAIN")
        self.syn("192.0.2.13", 443, t)
        if self.role == "subject":
            self.proxy.append({"t": t, "kind": "absolute", "host": "192.0.2.12", "method": "GET"})
            self.proxy.append({"t": t, "kind": "connect", "host": "canary-rq.census.invalid:443"})
        else:
            self.syn("192.0.2.12", 80, t)
            self.dns.append(f"query canary-rq.census.invalid. A NXDOMAIN t={t}")
            self.execs.append(f"t={t}\tname=xdg-open\tpid=1\targc=1\targ=https://canary-br.census.invalid/")

    def write(self, d):
        tun = list(self.tun)
        if self.stopped:
            tun.append({"event": "stop", "t": self.t0 + 9999, "reason": "signal SIGTERM"})
        files = {
            "tun.jsonl": "\n".join(json.dumps(x) for x in tun),
            "nft.json": nft_doc(self.sets),
            "dns.log": "\n".join(self.dns),
            "proxy.jsonl": "\n".join(json.dumps(x) for x in [{"event": "start", "t": 0}] + self.proxy),
            "canary.jsonl": "\n".join(json.dumps(x) for x in self.canary),
        }
        if self.execs:
            files["exec.log"] = "\n".join(self.execs)
        if self.crash:
            files["crash.txt"] = "\n".join(self.crash)
        for k, v in files.items():
            pathlib.Path(d, k).write_text(v + "\n")
        return report.load_run(d)

    def report(self, control=None, paths=PATHS):
        with tempfile.TemporaryDirectory() as d:
            return report.build_report(self.write(d), self.role, paths, SERVER, control)


def exercised_control():
    c = Run("control")
    t = c.t0 + 1000
    c.dns.append(f"query tele.census.invalid. A NXDOMAIN t={t}")
    c.dns.append(f"query dev-gw.census.invalid. A NXDOMAIN t={t}")
    c.execs.append(f"t={t}\tname=xdg-open\tpid=2\targc=1\targ=https://shop.census.invalid/")
    c.crash.append("prefix/crashpad/reports/0001.dmp")
    return c


class Classes(unittest.TestCase):
    def _cls(self, ev, windows=((0, 10),)):
        ctx = {"canaries": canary.CANARIES, "windows": list(windows), "server_name": SERVER,
               "resolver_peers": {40001}}
        return report.classify(dict(ev), ctx)[0]

    def test_a_resolver_reply_is_noise_and_any_other_udp_to_the_host_leaks(self):
        ev = {"rec": "R2", "set": "udp4", "dst": report.JAIL_HOST, "proto": "udp", "count": 2}
        self.assertEqual(self._cls({**ev, "dport": 40001}), "NOISE")
        self.assertEqual(self._cls({**ev, "dport": 40002}), "LEAK")

    def test_every_class_row(self):
        J = canary.JAIL_HOST
        for port in report.ALLOWED_TCP:
            self.assertEqual(self._cls({"rec": "R2", "proto": "tcp", "dst": J, "dport": port}), "ALLOWED")
        self.assertEqual(self._cls({"rec": "R2", "proto": "udp", "dst": J, "dport": 53}), "ALLOWED")
        self.assertEqual(self._cls({"rec": "R3", "name": SERVER}), "ALLOWED")
        self.assertEqual(self._cls({"rec": "R4p", "host": "x.census.invalid:443"}), "ABSORBED")
        self.assertEqual(self._cls({"rec": "R3", "name": "sdk" + report.ABSORBED_SUFFIX}), "ABSORBED")
        self.assertEqual(self._cls({"rec": "R3", "name": "x-public-gateway.gateway.invalid"}), "ABSORBED")
        self.assertEqual(self._cls({"rec": "R3", "name": "x-public-gateway.gateway.invalid.example"}), "LEAK")
        self.assertEqual(self._cls({"rec": "R2", "proto": "tcp", "dst": "127.0.0.1", "dport": 9}), "ABSORBED")
        for typ in report.NOISE_ICMPV6:
            self.assertEqual(self._cls({"rec": "R1", "proto": "icmpv6", "dst": "ff02::2", "type": typ}), "NOISE")
        self.assertEqual(self._cls({"rec": "R1", "proto": "icmpv6", "dst": "ff02::2", "type": 128}), "LEAK")
        self.assertEqual(self._cls({"rec": "R1", "proto": "tcp", "dst": "192.0.2.10", "dport": 443, "t": 5}),
                         "CANARY")
        self.assertEqual(self._cls({"rec": "R1", "proto": "tcp", "dst": "192.0.2.10", "dport": 443, "t": 50}),
                         "LEAK")
        self.assertEqual(self._cls({"rec": "R2", "proto": "tcp", "dst": "127.0.0.1", "dport": 4000}), "LOCAL")
        self.assertEqual(self._cls({"rec": "R1", "proto": "tcp", "dst": "198.51.100.9", "dport": 443}), "LEAK")
        self.assertEqual(self._cls({"rec": "R3", "name": "tele.census.invalid"}), "LEAK")
        self.assertEqual(self._cls({"rec": "R4e", "args": ["https://shop.census.invalid/"]}), "LEAK")
        self.assertEqual(self._cls({"rec": "R1", "proto": "unparsed", "dst": None}), "LEAK")

    def test_nft_parse_both_element_shapes(self):
        sets = {"tcp4": [("192.0.2.1", 443)], "oth6": [("2001:db8::1", "ipv6-icmp")], "oth4": [("192.0.2.2", 1)]}
        for wrapped in (True, False):
            evs = report.parse_nft(nft_doc(sets, wrapped))
            got = sorted((e["set"], e["dst"], e.get("dport", e.get("proto"))) for e in evs)
            self.assertEqual(got, [("oth4", "192.0.2.2", "icmp"), ("oth6", "2001:db8::1", "icmpv6"),
                                   ("tcp4", "192.0.2.1", 443)])

    def test_dns_log_old_and_new_formats(self):
        evs = report.parse_dns_log(["query A.Census.Invalid. A -> 10.137.0.1",
                                    "query b.census.invalid. AAAA NXDOMAIN peer=10.137.0.2:41 t=123",
                                    "eve-dns: listening"])
        self.assertEqual([(e["name"], e.get("t"), e.get("peer")) for e in evs],
                         [("a.census.invalid", None, None), ("b.census.invalid", 123, "10.137.0.2:41")])

    def test_handshake_payload_names_an_ip_literal_flow(self):
        recs = [sink.record(wire.build_tcp(ME4, "198.51.100.3", 1, 443, 1, 0, wire.TCP_SYN), 1),
                sink.record(wire.build_tcp(ME4, "198.51.100.3", 1, 443, 2, 9, wire.TCP_ACK,
                                           client_hello("lit.census.invalid")), 2)]
        (flow,) = report.r1_flows(recs)
        self.assertEqual((flow["name"], flow["count"], flow["syn"]), ("lit.census.invalid", 2, True))

    def test_name_patterns(self):
        self.assertTrue(report.name_matches("a.tele.census.invalid.", ["tele.census.invalid"]))
        self.assertFalse(report.name_matches("xtele.census.invalid", ["tele.census.invalid"]))
        self.assertTrue(report.name_matches("dev-gw.census.invalid", ["*-gw.census.invalid"]))


class Verdicts(unittest.TestCase):
    def test_a_clean_control_and_subject_reach_v2(self):
        ctrl = exercised_control().report()
        self.assertEqual((ctrl["verdict"], ctrl["inconclusive"], ctrl["invalid"]), ("CONTROL", [], []))
        sub = Run("subject")
        sub.dns.append(f"query {SERVER}. A -> 10.137.0.1")
        sub.sets["tcp4"].append((canary.JAIL_HOST, report.ALLOWED_TCP[0]))
        rep = sub.report(ctrl)
        self.assertEqual(rep["verdict"], "V2", rep["invalid"])
        self.assertEqual(set(rep["path_status"].values()), {"PASS"})

    def test_an_absorbed_lookup_keeps_v1_but_not_v2(self):
        ctrl = exercised_control().report()
        sub = Run("subject")
        sub.dns.append("query sdk" + report.ABSORBED_SUFFIX + ". A NXDOMAIN")
        rep = sub.report(ctrl)
        self.assertEqual(rep["verdict"], "V1")
        self.assertEqual(rep["v2_blockers"], ["sdk" + report.ABSORBED_SUFFIX])

    def test_an_unexercised_control_path_is_inconclusive_not_passed(self):
        c = exercised_control()
        c.crash = []
        c.execs = c.execs[:1]  # keep the canary's exec, drop the path's
        ctrl = c.report()
        self.assertEqual(ctrl["inconclusive"], ["X3", "X4"])
        rep = Run("subject").report(ctrl)
        self.assertEqual(rep["verdict"], "INCONCLUSIVE")
        self.assertEqual((rep["path_status"]["X3"], rep["path_status"]["X1"]), ("INCONCLUSIVE", "PASS"))
        self.assertIn("X3", report.summary(rep)[1])

    def test_a_known_open_name_is_open_never_a_pass_and_never_a_failure(self):
        ctrl = exercised_control().report()
        sub = Run("subject")
        sub.dns.append("query census-mini-ftp.census.invalid. A NXDOMAIN")
        rep = sub.report(ctrl)
        self.assertEqual(rep["counts"]["OPEN"], 1)
        self.assertEqual(rep["counts"]["LEAK"], 0)
        self.assertEqual(rep["verdict"], "INCONCLUSIVE", rep.get("note"))
        self.assertIn("known-open", rep["note"])
        self.assertTrue(any(l.startswith("OPEN") for l in report.summary(rep)))

    def test_a_noenv_run_must_prove_the_env_was_off(self):
        ctrl = exercised_control().report()
        for stages, want in (([], "no scenario recorded its begin stage"),
                             ([{"event": "stage", "stage": "begin", "scenario": "s", "noenv": False}], "the patcher's env was applied")):
            sub = Run("subject")
            sub.canary += stages
            with tempfile.TemporaryDirectory() as d:
                rep = report.build_report(sub.write(d), "subject", PATHS, SERVER, ctrl, noenv=True)
            self.assertEqual(rep["verdict"], "INVALID")
            self.assertTrue(any(want in x for x in rep["invalid"]), rep["invalid"])
        sub = Run("subject")
        sub.canary.append({"event": "stage", "stage": "begin", "scenario": "s", "noenv": True})
        with tempfile.TemporaryDirectory() as d:
            rep = report.build_report(sub.write(d), "subject", PATHS, SERVER, ctrl, noenv=True)
        self.assertEqual(rep["invalid"], [])

    def test_no_control_means_inconclusive(self):
        self.assertEqual(Run("subject").report(None)["verdict"], "INCONCLUSIVE")

    def test_a_missing_canary_is_invalid(self):
        sub = Run("subject")
        sub.dns = [x for x in sub.dns if "canary-py" not in x]
        rep = sub.report(exercised_control().report())
        self.assertEqual(rep["verdict"], "INVALID")
        self.assertIn("canary py-dns missing from R3", rep["invalid"])

    def test_an_r1_flow_missing_from_r2_is_invalid(self):
        sub = Run("subject")
        sub.syn("198.51.100.4", 443, sub.t0 + 2000, r2=False)
        rep = sub.report(exercised_control().report())
        self.assertEqual(rep["verdict"], "INVALID")
        self.assertTrue(any("missing from R2" in x for x in rep["invalid"]))

    def test_a_dead_sink_or_missing_recorder_is_invalid(self):
        sub = Run("subject")
        sub.stopped = False
        self.assertIn("the sink died (no stop line)", sub.report()["invalid"])
        with tempfile.TemporaryDirectory() as d:
            Run("subject").write(d)
        with tempfile.TemporaryDirectory() as d:
            Run("subject").write(d)
            os.remove(os.path.join(d, "nft.json"))
            self.assertIn("recorder file nft.json is missing", report.load_run(d)["faults"])

    def test_the_drop_canary_in_r1_is_a_harness_fault(self):
        sub = Run("subject")
        sub.syn(canary.JAIL_HOST, canary.DROP_PORT, sub.t0 + 20)
        rep = sub.report(exercised_control().report())
        self.assertEqual(rep["verdict"], "INVALID")

    def test_a_leak_fails_and_is_attributed(self):
        sub = Run("subject")
        sub.dns.append(f"query tele.census.invalid. A NXDOMAIN t={sub.t0 + 3000}")
        sub.syn("198.51.100.5", 443, sub.t0 + 3000)
        rep = sub.report(exercised_control().report())
        self.assertEqual(rep["verdict"], "FAIL")
        self.assertEqual((rep["path_status"]["X1"], rep["counts"]["LEAK"]), ("LEAK", 3))

    def test_the_browser_canary_reaching_a_launcher_in_the_subject_is_a_leak(self):
        sub = Run("subject")
        sub.execs.append(f"t={sub.t0 + 20}\tname=xdg-open\tpid=1\targc=1\targ=https://canary-br.census.invalid/")
        rep = sub.report(exercised_control().report())
        self.assertEqual(rep["verdict"], "FAIL")
        self.assertEqual(rep["invalid"], [])

    def test_the_summary_stays_short(self):
        sub = Run("subject")
        for i in range(200):
            sub.syn(f"198.51.100.{i % 250}", 1000 + i, sub.t0 + 5000)
        rep = sub.report(exercised_control().report())
        lines = report.summary(rep)
        self.assertLessEqual(len(lines), report.SUMMARY_MAX)
        self.assertTrue(lines[0].startswith("VERDICT FAIL"))
        self.assertIn("more in report.json", lines[-1])

    def test_main_writes_the_report_and_exits_by_verdict(self):
        with tempfile.TemporaryDirectory() as d:
            exercised_control().write(d)
            paths = os.path.join(d, "paths.json")
            pathlib.Path(paths).write_text(json.dumps({"paths": PATHS}))
            with contextlib.redirect_stdout(io.StringIO()):
                rc = report.main([d, "--role", "control", "--paths", paths, "--server-name", SERVER])
            self.assertEqual(rc, 0)
            self.assertEqual(json.loads(pathlib.Path(d, "report.json").read_text())["verdict"], "CONTROL")
            self.assertTrue(pathlib.Path(d, "summary.txt").read_text().startswith("VERDICT CONTROL"))


if __name__ == "__main__":
    unittest.main()
