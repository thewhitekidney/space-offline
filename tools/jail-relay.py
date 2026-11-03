#!/usr/bin/env python3
"""The friend's relay (IP22-b, ruling 6; RR1 H2). Stdlib only.

Runs INSIDE the rootless jail's lo-only namespace (tools/jail-rootless.sh
--relay) and bridges the client's TCP to the unix sockets that
tools/friend-tunnel.sh's `ssh -L` makes on the friend's own machine:

    10.137.0.1:26000  ->  $XDG_RUNTIME_DIR/eve-game.sock  ->  owner 127.0.0.1:26001
    10.137.0.1:5222   ->  $XDG_RUNTIME_DIR/eve-chat.sock  ->  owner 127.0.0.1:5223

A pathname unix socket is reached through the filesystem, not the network
namespace, so this needs no veth, no nft and no sudo; the namespace still has
no route anywhere. The relay only ever connects to the two socket paths it
was given; the client cannot ask it for anything else.

Optionally (`--dns`) it also answers DNS the way eve-dns's jail policy does,
for a friend who has no server build: a name under `--dns-suffix` gets an A
record with `--dns-target`; any other type on such a name gets NOERROR with
no answer (never NXDOMAIN, which a dual-stack resolver would cache for A
too); every other name is NXDOMAIN.

    jail-relay.py run --map 10.137.0.1:26000=SOCK --map 10.137.0.1:5222=SOCK \\
                      [--dns 10.137.0.1:53 --dns-target 10.137.0.1 --dns-log FILE]
    jail-relay.py selftest      # socketpairs, a temp unix socket, a DNS query
"""

from __future__ import annotations

import argparse
import os
import socket
import struct
import sys
import tempfile
import threading
import time

BUF = 65536
# [A] MAX_CONNS: one client holds a game and a chat connection, plus brief
# extras (the boot probe, a reconnect). Falsified by a friend's client being
# refused while it is the only one running.
MAX_CONNS = 16
# The suffix eve-dns's jail policy resolves (eve-dns EVE_SERVERS_SUFFIX).
DNS_SUFFIX = "servers.eveonline.com"
DNS_TTL = 60


def log(msg: str) -> None:
    print(f"jail-relay: {msg}", file=sys.stderr, flush=True)


# --- the bridge ---------------------------------------------------------------

def _pump(src: socket.socket, dst: socket.socket) -> None:
    """Copy src to dst until EOF, then half-close dst so the EOF propagates."""
    try:
        while True:
            data = src.recv(BUF)
            if not data:
                break
            dst.sendall(data)
    except OSError:
        pass
    finally:
        try:
            dst.shutdown(socket.SHUT_WR)
        except OSError:
            pass


def bridge(a: socket.socket, b: socket.socket) -> None:
    """Both directions; returns when both have seen EOF; closes both."""
    t = threading.Thread(target=_pump, args=(b, a), daemon=True)
    t.start()
    _pump(a, b)
    t.join()
    a.close()
    b.close()


def serve_map(listen: tuple[str, int], path: str, slots: threading.BoundedSemaphore,
              ready: threading.Event | None = None) -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(listen)
    srv.listen(8)
    log(f"{listen[0]}:{listen[1]} -> {path}")
    if ready is not None:
        ready.set()
    while True:
        conn, _ = srv.accept()
        if not slots.acquire(blocking=False):
            log(f"refused a connection on :{listen[1]}: {MAX_CONNS} already open")
            conn.close()
            continue
        up = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            up.connect(path)
        except OSError as e:
            log(f"{path}: {e.strerror or e} (is tools/friend-tunnel.sh running?)")
            up.close()
            conn.close()
            slots.release()
            continue

        def run(c=conn, u=up) -> None:
            try:
                bridge(c, u)
            finally:
                slots.release()

        threading.Thread(target=run, daemon=True).start()


# --- DNS (eve-dns's jail policy, minimal) ----------------------------------------

def _qname(msg: bytes, off: int) -> tuple[str, int]:
    labels = []
    while True:
        if off >= len(msg):
            raise ValueError("truncated name")
        n = msg[off]
        if n == 0:
            return ".".join(labels), off + 1
        if n & 0xC0:
            raise ValueError("compressed question name")
        off += 1
        labels.append(msg[off:off + n].decode("ascii", "replace").lower())
        off += n


def dns_answer(query: bytes, target: str, suffix: str = DNS_SUFFIX) -> tuple[bytes, str] | None:
    """The reply to one query and a log line, or None to drop it."""
    if len(query) < 12:
        return None
    qid, flags, qd = struct.unpack(">HHH", query[:6])
    if flags & 0x8000 or qd != 1:
        return None
    try:
        name, end = _qname(query, 12)
    except ValueError:
        return None
    if end + 4 > len(query):
        return None
    qtype, qclass = struct.unpack(">HH", query[end:end + 4])
    question = query[12:end + 4]
    matched = name == suffix or name.endswith("." + suffix)
    rd = flags & 0x0100
    answers = b""
    if not matched:
        rcode, what = 3, "NXDOMAIN"
    elif qtype in (1, 255) and qclass in (1, 255):
        rcode, what = 0, f"-> {target}"
        answers = (b"\xc0\x0c" + struct.pack(">HHIH", 1, 1, DNS_TTL, 4)
                   + socket.inet_aton(target))
    else:
        rcode, what = 0, "NODATA"
    head = struct.pack(">HHHHHH", qid, 0x8000 | 0x0400 | rd | rcode, 1,
                       1 if answers else 0, 0, 0)
    return head + question + answers, f"query {name}. {qtype} {what}"


def serve_dns(listen: tuple[str, int], target: str, logfile: str | None,
              ready: threading.Event | None = None) -> None:
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind(listen)
    out = open(logfile, "a", buffering=1) if logfile else None
    if logfile:
        os.chmod(logfile, 0o600)
    log(f"dns on {listen[0]}:{listen[1]}: *.{DNS_SUFFIX} -> {target}, else NXDOMAIN")
    if ready is not None:
        ready.set()
    while True:
        data, peer = s.recvfrom(512)
        r = dns_answer(data, target)
        if r is None:
            continue
        s.sendto(r[0], peer)
        if out:
            out.write(r[1] + "\n")


# --- the command line -------------------------------------------------------------

def hostport(v: str) -> tuple[str, int]:
    host, _, port = v.rpartition(":")
    return host, int(port)


def cmd_run(ns: argparse.Namespace) -> int:
    slots = threading.BoundedSemaphore(MAX_CONNS)
    threads = []
    for m in ns.map:
        left, sep, path = m.partition("=")
        if not sep or not path.startswith("/"):
            log(f"--map wants ADDR:PORT=/abs/socket, not {m!r}")
            return 2
        threads.append(threading.Thread(target=serve_map, args=(hostport(left), path, slots),
                                         daemon=True))
    if ns.dns:
        threads.append(threading.Thread(target=serve_dns,
                                        args=(hostport(ns.dns), ns.dns_target, ns.dns_log),
                                        daemon=True))
    if not threads:
        log("nothing to do (no --map, no --dns)")
        return 2
    for t in threads:
        t.start()
    while all(t.is_alive() for t in threads):
        time.sleep(1)
    log("a listener stopped; exiting")
    return 1


def selftest() -> int:
    fails = []

    def check(cond: bool, what: str) -> None:
        print(("ok    " if cond else "FAIL  ") + what)
        if not cond:
            fails.append(what)

    def recv_exact(s: socket.socket, n: int) -> bytes:
        got = b""
        while len(got) < n:
            chunk = s.recv(n - len(got))
            if not chunk:
                break
            got += chunk
        return got

    def eof(s: socket.socket) -> bool:
        try:
            return s.recv(1) == b""
        except OSError:  # a timeout: the EOF never came
            return False

    # 1. two socketpairs: client <-> [relay a | relay b] <-> server
    client, ra = socket.socketpair()
    rb, server = socket.socketpair()
    for s in (client, server):
        s.settimeout(5)
    t = threading.Thread(target=bridge, args=(ra, rb), daemon=True)
    t.start()
    client.sendall(b"hello server")
    check(recv_exact(server, 12) == b"hello server", "bytes client -> server")
    server.sendall(b"hello client")
    check(recv_exact(client, 12) == b"hello client", "bytes server -> client")
    big = os.urandom(3 * BUF + 17)
    w = threading.Thread(target=client.sendall, args=(big,), daemon=True)
    w.start()
    check(recv_exact(server, len(big)) == big, f"{len(big)} bytes in order, client -> server")
    w.join()
    client.shutdown(socket.SHUT_WR)
    check(eof(server), "client EOF reaches the server")
    server.sendall(b"after half-close")
    check(recv_exact(client, 16) == b"after half-close", "server still sends after the client's EOF")
    server.shutdown(socket.SHUT_WR)
    check(eof(client), "server EOF reaches the client")
    t.join(5)
    check(not t.is_alive(), "the bridge returns once both sides are done")
    check(ra.fileno() == -1 and rb.fileno() == -1, "the bridge closed its two ends")
    client.close()
    server.close()

    # 2. the listener: TCP on loopback -> a unix socket, as in the jail
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "eve-game.sock")
        us = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        us.bind(path)
        us.listen(1)
        probe = socket.socket()
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
        probe.close()
        ready = threading.Event()
        threading.Thread(target=serve_map, args=(("127.0.0.1", port), path,
                                                  threading.BoundedSemaphore(2), ready),
                         daemon=True).start()
        ready.wait(5)
        c = socket.create_connection(("127.0.0.1", port), timeout=5)
        us.settimeout(5)
        u, _ = us.accept()
        u.settimeout(5)
        c.sendall(b"ping")
        check(recv_exact(u, 4) == b"ping", "TCP -> unix socket")
        u.sendall(b"pong")
        check(recv_exact(c, 4) == b"pong", "unix socket -> TCP")
        u.close()
        check(eof(c), "the unix side closing closes the TCP side")
        c.close()
        # The socket gone (tunnel down): a connect is closed, not hung.
        us.close()
        os.unlink(path)
        c = socket.create_connection(("127.0.0.1", port), timeout=5)
        c.settimeout(5)
        try:
            check(c.recv(1) == b"", "no tunnel: the client's connection is closed")
        except OSError:
            check(True, "no tunnel: the client's connection is reset")
        c.close()

    # 3. DNS, as eve-dns's jail policy
    def q(name: str, qtype: int) -> bytes:
        body = b"".join(bytes([len(p)]) + p.encode() for p in name.split(".")) + b"\0"
        return struct.pack(">HHHHHH", 0x1234, 0x0100, 1, 0, 0, 0) + body + struct.pack(">HH", qtype, 1)

    target = "10.0.0.7"  # invented
    r, _ = dns_answer(q("tranquility." + DNS_SUFFIX, 1), target)
    check(r[:2] == b"\x12\x34" and r[3] & 0x0F == 0 and r[6:8] == b"\x00\x01"
          and r.endswith(socket.inet_aton(target)), "A under the suffix -> the target")
    r, _ = dns_answer(q("tranquility." + DNS_SUFFIX, 28), target)
    check(r[3] & 0x0F == 0 and r[6:8] == b"\x00\x00", "AAAA under the suffix -> NODATA, not NXDOMAIN")
    r, _ = dns_answer(q("sentry.io", 1), target)
    check(r[3] & 0x0F == 3 and r[6:8] == b"\x00\x00", "sentry.io -> NXDOMAIN")
    r, _ = dns_answer(q("evil" + DNS_SUFFIX, 1), target)
    check(r[3] & 0x0F == 3, "a name that only ends in the suffix's text -> NXDOMAIN")
    check(dns_answer(b"\x00" * 5, target) is None, "a runt packet is dropped")

    print("SELFTEST " + ("OK" if not fails else f"FAILED ({len(fails)})"))
    return 0 if not fails else 1


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(prog="jail-relay.py", description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="verb", required=True)
    r = sub.add_parser("run")
    r.add_argument("--map", action="append", default=[], metavar="ADDR:PORT=SOCKET")
    r.add_argument("--dns", metavar="ADDR:PORT")
    r.add_argument("--dns-target", default="10.137.0.1")
    r.add_argument("--dns-log")
    sub.add_parser("selftest")
    ns = ap.parse_args(argv)
    if ns.verb == "selftest":
        return selftest()
    return cmd_run(ns)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
