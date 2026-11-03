#!/usr/bin/env python3
"""The census report (packet CEN-1, `docs/guides/census.md`; R-CENSUS §3).

Merges the four recorders of one run, classifies every event, checks the
canaries and the harness, and gives a verdict for the copy.

  census_report.py RUN --role control [--paths out/census/paths.json]
  census_report.py RUN --role subject --control CONTROL_RUN/report.json

RUN holds (written by the harness, CEN-2/CEN-3):

  tun.jsonl     R1, `census_sink.py`
  nft.json      R2, `nft -j list table inet eve_census`
  dns.log       R3, this run's slice of the eve-dns query log
  proxy.jsonl   R4b, `census_proxy.py`
  exec.log      R4a, `tools/census/browser-shim` (absent = no exec)
  canary.jsonl  the canary windows, `census_canary.py`
  crash.txt     new crash-DB files since the run's mark (absent = none)

Writes RUN/report.json (every event and its class) and RUN/summary.txt (at
most 30 lines), and prints the summary. Exit status: 0 for V1, V2 or a
complete control, 1 FAIL, 2 INVALID, 3 INCONCLUSIVE.

Classes (R-CENSUS §3.2): ALLOWED, ABSORBED, NOISE, CANARY, LOCAL, LEAK.
LOCAL is ours: loopback traffic other than the dead proxy. It cannot leave
the netns, so it never fails V1; unjailed, loopback is the host's, so V2
requires none.

Verdicts (§3.3): INVALID (a canary missing, an R1 flow missing from R2, a
recorder missing, or the sink died); FAIL (any LEAK); INCONCLUSIVE (a path
the control never exercised: "not passed"); V1, patch verified under the
jail; V2, V1 plus no DNS query but the server name and no LOCAL traffic.

The path table (`--paths`) names each reachable phone-home path and how to
recognise it. Its names come from the client, so it lives in `out/`
(gitignored), never in the repository:

  {"paths": {"<id>": {"what": "...", "names": ["host.example", "*.glob.example"],
                      "endpoints": ["10.137.0.1:8443"], "exec": false, "crash": false}}}
"""

import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.append(_HERE)

import argparse  # noqa: E402
import fnmatch  # noqa: E402
import ipaddress  # noqa: E402
import json  # noqa: E402

import census_canary  # noqa: E402
import census_wire as wire  # noqa: E402

REPO = os.path.dirname(_HERE)
DEFAULT_PATHS = os.path.join(REPO, "out", "census", "paths.json")

#: The one name the client may look up (R-PATCH P15).
SERVER_NAME = "tranquility.servers.eveonline.com"
JAIL_HOST = census_canary.JAIL_HOST
#: The game and chat ports on the jail host (`tools/jail-setup.sh`).
ALLOWED_TCP = (26000, 5222)
#: Where the patcher's Tier A sends HTTP: the dead proxy.
PROXY_ENDPOINT = ("127.0.0.1", 9)
#: The gateway env's target; the jail drops it (R-PATCH N4).
GATEWAY_ENDPOINT = (JAIL_HOST, census_canary.DROP_PORT)
#: B2 renames LaunchDarkly's hosts into this reserved name (R-PATCH B2).
ABSORBED_SUFFIX = ".lndarkly.invalid"
#: Every renamed-away name: B2's, and tier C8's gateway discovery suffix
#: (`{tier}-public-gateway.gateway.invalid`, docs/guides/patcher.md).
ABSORBED_SUFFIXES = (ABSORBED_SUFFIX, ".gateway.invalid")
#: Kernel ICMPv6 a netns emits by itself: RS, NS, MLDv2 report.
NOISE_ICMPV6 = (133, 135, 143)
NOISE_NET = ipaddress.ip_network("ff02::/16")

#: Known-open names: a path nothing closes yet, recorded as such instead of a
#: silent pass or a failure nobody can fix today. `ftp://` goes past the
#: HTTP(S) proxy (issue #40; the launcher-env fix, G2, was not built).
KNOWN_OPEN = {"census-mini-ftp.census.invalid": "ftp:// bypasses the launcher's proxy; no patch closes it yet"}

CLASSES = ("ALLOWED", "ABSORBED", "NOISE", "CANARY", "LOCAL", "OPEN", "LEAK")
SUMMARY_MAX = 30

EXIT = {"V1": 0, "V2": 0, "CONTROL": 0, "FAIL": 1, "INVALID": 2, "INCONCLUSIVE": 3}


# ------------------------------------------------------------------ loading


def _lines(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            return f.read().splitlines()
    except FileNotFoundError:
        return None


def _jsonl(lines):
    out = []
    for ln in lines or ():
        ln = ln.strip()
        if ln:
            try:
                out.append(json.loads(ln))
            except ValueError:
                out.append({"bad": "not json", "raw": ln[:80]})
    return out


def parse_dns_log(lines) -> list:
    """eve-dns query lines: `query NAME. TYPE <verdict> [peer=IP:PORT] [t=MS]`."""
    out = []
    for ln in lines or ():
        f = ln.split()
        if len(f) < 3 or f[0] != "query":
            continue
        ev = {"rec": "R3", "name": f[1].rstrip(".").lower(), "qtype": f[2],
              "answer": " ".join(x for x in f[3:] if "=" not in x)}
        for x in f[3:]:
            k, _, v = x.partition("=")
            if k == "peer" and v:
                ev["peer"] = v
            elif k == "t" and v.isdigit():
                ev["t"] = int(v)
        out.append(ev)
    return out


def parse_exec_log(lines) -> list:
    """browser-shim lines: TAB-separated `t= name= pid= argc= arg=...`."""
    def unesc(s):
        out, i = [], 0
        while i < len(s):
            if s[i] == "\\" and i + 1 < len(s):
                out.append({"\\": "\\", "t": "\t", "n": "\n"}.get(s[i + 1], s[i + 1]))
                i += 2
            else:
                out.append(s[i])
                i += 1
        return "".join(out)

    out = []
    for ln in lines or ():
        if not ln.strip():
            continue
        ev = {"rec": "R4e", "args": []}
        for field in ln.split("\t"):
            k, _, v = field.partition("=")
            if k == "arg":
                ev["args"].append(unesc(v))
            elif k == "t" and v.isdigit():
                ev["t"] = int(v) or None
            elif k == "name":
                ev["name"] = unesc(v)
        out.append(ev)
    return out


def _proto_key(v):
    if v in (1, "icmp"):
        return "icmp"
    if v in (58, "ipv6-icmp", "icmpv6"):
        return "icmpv6"
    return str(v)


def parse_nft(text: str) -> list:
    """The elements of table `eve_census` from `nft -j list table`.

    [A] The JSON shape of a dynamic set's elements with per-element
    counters: `{"elem": {"val": {"concat": [addr, key]}, "counter": {...}}}`,
    or a bare `{"concat": [...]}`. Falsifier: the first real `census-dump`
    parses to no elements while R1 has flows (the run is then INVALID, so a
    wrong guess cannot pass).
    """
    doc = json.loads(text)
    out = []
    for item in doc.get("nftables", []):
        s = item.get("set")
        if not s or s.get("table", "eve_census") != "eve_census":
            continue
        name = s.get("name", "")
        for el in s.get("elem", []) or []:
            packets = None
            if isinstance(el, dict) and "elem" in el:
                packets = (el["elem"].get("counter") or {}).get("packets")
                el = el["elem"].get("val")
            if not isinstance(el, dict) or "concat" not in el or len(el["concat"]) != 2:
                continue
            dst, key = el["concat"]
            ev = {"rec": "R2", "set": name, "dst": str(ipaddress.ip_address(dst)), "count": packets}
            if name.startswith(("tcp", "udp")):
                ev["proto"], ev["dport"] = name[:3], int(key)
            else:
                ev["proto"] = _proto_key(key)
            out.append(ev)
    return out


def r1_flows(records: list) -> list:
    """Packets grouped into flows: one event per (proto, dst, port/type)."""
    flows = {}
    for r in records:
        if "event" in r:
            continue
        if "bad" in r:
            key = ("bad", r.get("raw", "")[:40])
            flows.setdefault(key, {"rec": "R1", "proto": "unparsed", "dst": None, "t": r.get("t"),
                                   "count": 0, "detail": r["bad"]})["count"] += 1
            continue
        proto = r.get("proto")
        port = r.get("dport") if proto in ("tcp", "udp") else r.get("type")
        key = (proto, r.get("dst"), port)
        f = flows.get(key)
        if f is None:
            f = flows[key] = {"rec": "R1", "proto": proto, "dst": r.get("dst"), "v": r.get("v"),
                              "t": r.get("t"), "count": 0, "syn": False}
            if proto in ("tcp", "udp"):
                f["dport"] = port
            else:
                f["type"] = port
        f["count"] += 1
        if proto == "tcp" and "S" in r.get("flags", "") and "A" not in r.get("flags", ""):
            f["syn"] = True
        if "name" not in f and r.get("payload"):
            name = wire.payload_name(bytes.fromhex(r["payload"]), r.get("dport"))
            if name:
                f["name"] = name.rstrip(".").lower()
    return list(flows.values())


def load_run(run: str) -> dict:
    """Every recorder of one run. A missing required file is a fault."""
    faults = []

    def need(name):
        lines = _lines(os.path.join(run, name))
        if lines is None:
            faults.append(f"recorder file {name} is missing")
        return lines

    tun = _jsonl(need("tun.jsonl"))
    nft_text = need("nft.json")
    r2 = []
    if nft_text is not None:
        try:
            r2 = parse_nft("\n".join(nft_text))
        except (ValueError, AttributeError) as err:
            faults.append(f"nft.json does not parse: {err}")
    proxy = _jsonl(need("proxy.jsonl"))
    return {
        "faults": faults,
        "tun": tun,
        "r1": r1_flows(tun),
        "r2": r2,
        "r3": parse_dns_log(need("dns.log")),
        "r4e": parse_exec_log(_lines(os.path.join(run, "exec.log"))),
        "r4p": [dict(p, rec="R4p") for p in proxy if "event" not in p],
        "proxy_events": [p for p in proxy if "event" in p],
        "canary": _jsonl(_lines(os.path.join(run, "canary.jsonl"))),
        "crash": [x for x in (_lines(os.path.join(run, "crash.txt")) or []) if x.strip()],
    }


# ---------------------------------------------------------- classification


def _host_only(host):
    if not host:
        return None
    host = host.lower()
    if host.startswith("["):
        return host[1:].split("]", 1)[0]
    if host.count(":") == 1:
        return host.split(":", 1)[0]
    return host


def windows_of(canary_log: list) -> list:
    """`(start, end)` pairs from the window marks; an unclosed window is
    open-ended (the harness died mid-window, and a canary there still
    counts)."""
    open_, out = {}, []
    for e in canary_log:
        if e.get("event") == "window-start":
            open_[e.get("window")] = e.get("t", 0)
        elif e.get("event") == "window-end" and e.get("window") in open_:
            out.append((open_.pop(e["window"]), e.get("t", 0)))
    out.extend((s, float("inf")) for s in open_.values())
    return out


def event_targets(ev: dict) -> list:
    """The strings a canary target may equal: address, address:port, name,
    proxy host, exec args (substring, see `canary_of`)."""
    out = []
    if ev.get("dst"):
        out.append(ev["dst"])
        if ev.get("dport") is not None:
            out.append(f"{ev['dst']}:{ev['dport']}")
    for k in ("name", "host"):
        if ev.get(k):
            out.append(_host_only(ev[k]))
    return out


def canary_of(ev: dict, canaries: dict):
    """The id of the canary whose target this event hits, or None."""
    targets = event_targets(ev)
    for cid, row in canaries.items():
        for tgt in row["targets"]:
            if ":" in tgt and "." in tgt:  # addr:port targets need the port
                if tgt in targets:
                    return cid
            elif tgt in targets or any(tgt in a for a in ev.get("args", ())):
                return cid
    return None


def classify(ev: dict, ctx: dict):
    """`(class, note)` for one event."""
    cid = canary_of(ev, ctx["canaries"])
    if cid:
        t = ev.get("t")
        if t is None or any(s <= t <= e for s, e in ctx["windows"]):
            return "CANARY", cid
        return "LEAK", f"canary target {cid} outside every canary window"
    rec = ev["rec"]
    if rec == "R4e":
        return "LEAK", "a browser launcher ran"
    if rec == "R4p":
        return "ABSORBED", f"dead proxy: {ev.get('host') or ev.get('kind')}"
    if rec == "R3":
        if ev["name"] == ctx["server_name"]:
            return "ALLOWED", "the server name"
        if ev["name"] in KNOWN_OPEN:
            return "OPEN", KNOWN_OPEN[ev["name"]]
        if ev["name"].endswith(ABSORBED_SUFFIXES):
            return "ABSORBED", "a patch renamed it (B2 or C8)"
        return "LEAK", "a name lookup"
    dst, proto, port = ev.get("dst"), ev.get("proto"), ev.get("dport")
    if dst is None:
        return "LEAK", ev.get("detail", "unparsed packet")
    if dst == JAIL_HOST:
        if proto == "tcp" and port in ALLOWED_TCP:
            return "ALLOWED", "our server"
        if proto in ("tcp", "udp") and port == 53:
            return "ALLOWED", "our resolver"
        # The rootless jail's client and resolver share this address, so the
        # output hook also counts each reply, to the port the query came from.
        if proto == "udp" and port in ctx.get("resolver_peers", ()):
            return "NOISE", "our resolver's reply"
        if proto == "tcp" and port == GATEWAY_ENDPOINT[1]:
            return "ABSORBED", "the gateway env's target: ours, and the jail allows it"
    if (dst, port) == PROXY_ENDPOINT and proto == "tcp":
        return "ABSORBED", "a connect to the dead proxy"
    addr = ipaddress.ip_address(dst)
    if proto == "icmpv6" and addr in NOISE_NET and ev.get("type", NOISE_ICMPV6[0]) in NOISE_ICMPV6:
        return "NOISE", "kernel ICMPv6"
    if addr.is_loopback:
        return "LOCAL", "loopback"
    return "LEAK", "an outbound attempt"


# --------------------------------------------------------------- the checks


def _matches(ev: dict, spec: dict) -> bool:
    rec = spec["rec"]
    if ev["rec"] != rec:
        return False
    if rec == "R1":
        return (ev.get("proto") == spec["proto"] and ev.get("dst") == spec["dst"]
                and ("dport" not in spec or ev.get("dport") == spec["dport"]))
    if rec == "R2":
        if ev.get("set") != spec["set"] or ev.get("dst") != spec["dst"]:
            return False
        key = spec["key"]
        return ev.get("dport") == key if isinstance(key, int) else ev.get("proto") == _proto_key(key)
    if rec == "R3":
        return ev.get("name") == spec["name"]
    if rec == "R4p":
        return _host_only(ev.get("host")) == spec["host"]
    if rec == "R4e":
        return any(spec["arg"] in a for a in ev.get("args", ()))
    return False


def check_canaries(events: list, role: str, canaries: dict):
    """`(missing, forbidden)`: expected evidence absent; forbidden evidence
    present, as `(canary id, spec)` pairs."""
    missing, forbidden = [], []
    for cid, row in canaries.items():
        for spec in row["expect"].get(role, []):
            if not any(_matches(e, spec) for e in events):
                missing.append((cid, spec))
        for spec in row.get("absent", {}).get(role, []):
            if any(_matches(e, spec) for e in events):
                forbidden.append((cid, spec))
    return missing, forbidden


def r2_key(flow: dict):
    """The R2 element an R1 flow must have, or None when R2 does not count
    it (a TCP flow seen without its SYN, an unparsed packet)."""
    proto, dst = flow.get("proto"), flow.get("dst")
    if dst is None:
        return None
    fam = "6" if ":" in dst else "4"
    if proto == "tcp":
        return ("tcp" + fam, dst, flow["dport"]) if flow.get("syn") else None
    if proto == "udp":
        return ("udp" + fam, dst, flow["dport"])
    return ("oth" + fam, dst, _proto_key(proto))


def r1_missing_from_r2(r1: list, r2: list) -> list:
    have = {(e["set"], e["dst"], e.get("dport", e.get("proto"))) for e in r2}
    return [f for f in r1 if (k := r2_key(f)) is not None and k not in have]


def name_matches(name, patterns) -> bool:
    if not name:
        return False
    name = name.rstrip(".").lower()
    for p in patterns:
        p = p.lower()
        if "*" in p or "?" in p:
            if fnmatch.fnmatchcase(name, p):
                return True
        elif name == p or name.endswith("." + p):
            return True
    return False


def path_hits(ev: dict, spec: dict) -> bool:
    if ev.get("class") == "CANARY":
        return False
    if spec.get("exec") and ev["rec"] == "R4e":
        return True
    if ev.get("dst") and f"{ev['dst']}:{ev.get('dport')}" in spec.get("endpoints", ()):
        return True
    return any(name_matches(_host_only(ev.get(k)), spec.get("names", ())) for k in ("name", "host"))


def sink_faults(tun: list) -> list:
    starts = [r for r in tun if r.get("event") == "start"]
    stops = [r for r in tun if r.get("event") == "stop"]
    if not starts:
        return ["the sink never started (no start line)"]
    if not stops:
        return ["the sink died (no stop line)"]
    if stops[-1].get("reason") == "error":
        return ["the sink stopped on an error"]
    return []


# ------------------------------------------------------------------- report


def build_report(run: dict, role: str, paths: dict, server_name: str = SERVER_NAME,
                 control=None, canaries=None, noenv: bool = False) -> dict:
    canaries = census_canary.CANARIES if canaries is None else canaries
    ctx = {"canaries": canaries, "windows": windows_of(run["canary"]), "server_name": server_name,
           "resolver_peers": {int(e["peer"].rpartition(":")[2]) for e in run["r3"]
                              if e.get("peer", "").startswith(JAIL_HOST + ":")}}
    events = run["r1"] + run["r2"] + run["r3"] + run["r4e"] + run["r4p"]
    for ev in events:
        ev["class"], ev["note"] = classify(ev, ctx)

    invalid = list(run["faults"]) + sink_faults(run["tun"])
    missing, forbidden = check_canaries(events, role, canaries)
    for cid, spec in missing:
        invalid.append(f"canary {cid} missing from {spec['rec']}")
    for cid, spec in forbidden:
        if spec["rec"] == "R4e":  # the patch let it through: a leak, not a harness fault
            for ev in events:
                if _matches(ev, spec):
                    ev["class"], ev["note"] = "LEAK", f"canary {cid} must not reach {spec['rec']} here"
        else:
            invalid.append(f"canary {cid} reached {spec['rec']}, which it must not (harness fault)")
    for f in r1_missing_from_r2(run["r1"], run["r2"]):
        invalid.append(f"R1 flow {f['proto']} {f['dst']}:{f.get('dport', f.get('type'))} is missing from R2")

    per_path = {}
    if noenv:
        begins = [e for e in run["canary"] if e.get("event") == "stage" and e.get("stage") == "begin"]
        if not begins:
            invalid.append("a no-env run, but no scenario recorded its begin stage: nothing proves the env was off")
        for e in begins:
            if e.get("noenv") is not True:
                invalid.append(f"a no-env run, but {e.get('scenario')} began with noenv={e.get('noenv')!r}: the patcher's env was applied")
    for pid, spec in sorted(paths.items()):
        hits = [e for e in events if path_hits(e, spec)]
        exercised = bool(hits) or (bool(spec.get("crash")) and bool(run["crash"]))
        per_path[pid] = {"what": spec.get("what", ""), "exercised": exercised,
                         "leaks": sum(1 for e in hits if e["class"] == "LEAK"),
                         "open": sum(1 for e in hits if e["class"] == "OPEN")}

    counts = {c: sum(1 for e in events if e["class"] == c) for c in CLASSES}
    rep = {"role": role, "noenv": noenv, "invalid": invalid, "counts": counts, "paths": per_path,
           "crash": run["crash"], "events": events, "server_name": server_name}

    if invalid:
        rep["verdict"] = "INVALID"
    elif role == "control":
        rep["verdict"] = "CONTROL"
        rep["inconclusive"] = sorted(p for p, v in per_path.items() if not v["exercised"])
    else:
        rep.update(verdict_subject(rep, control, events))
    return rep


def verdict_subject(rep: dict, control, events: list) -> dict:
    ctrl_paths = (control or {}).get("paths", {})
    ctrl_ok = bool(control) and control.get("verdict") == "CONTROL"
    status = {}
    for pid, v in rep["paths"].items():
        if v["leaks"]:
            status[pid] = "LEAK"
        elif v.get("open"):
            status[pid] = "OPEN"
        elif not ctrl_ok or not ctrl_paths.get(pid, {}).get("exercised"):
            status[pid] = "INCONCLUSIVE"
        else:
            status[pid] = "PASS"
    out = {"path_status": status,
           "inconclusive": sorted(p for p, s in status.items() if s == "INCONCLUSIVE")}
    if not ctrl_ok:
        out["note"] = "no valid control report" if not control else f"the control is {control.get('verdict')}"
    out["open"] = sorted(p for p, s in status.items() if s == "OPEN")
    if rep["counts"]["LEAK"]:
        out["verdict"] = "FAIL"
    elif out["open"] or rep["counts"]["OPEN"]:
        # Known-open is "not passed", never a pass (see KNOWN_OPEN).
        out["verdict"] = "INCONCLUSIVE"
        out["note"] = "known-open path(s), not closed by any patch: " + ", ".join(out["open"] or ["(unnamed)"])
    elif out["inconclusive"] or not rep["paths"]:
        out["verdict"] = "INCONCLUSIVE"
        if not rep["paths"]:
            out["note"] = "no path table (--paths)"
    else:
        names = sorted({e["name"] for e in events if e["rec"] == "R3" and e["class"] != "CANARY"
                        and e["name"] != rep["server_name"]})
        strict = not names and not rep["counts"]["LOCAL"]
        out["verdict"] = "V2" if strict else "V1"
        out["v2_blockers"] = names + (["LOCAL traffic"] if rep["counts"]["LOCAL"] else [])
    return out


def _evline(e: dict) -> str:
    if e["rec"] == "R4e":
        return f"  R4e        {e.get('name', '?')} {' '.join(e.get('args', []))[:120]} ({e['note']})"
    where = e.get("name") or e.get("host") or e.get("dst") or "?"
    if e.get("dport") is not None and not e.get("name"):
        where = f"{where}:{e['dport']}"
    return f"  {e['rec']:<3} {e.get('proto', ''):<6} {where} ({e['note']})"


def summary(rep: dict, label: str = "") -> list:
    """At most `SUMMARY_MAX` lines; the verdict first."""
    v = rep["verdict"]
    lines = [f"VERDICT {v}: {rep['role']}{' (no-env)' if rep.get('noenv') else ''}{' ' + label if label else ''}"]
    if rep.get("open"):
        lines.append("OPEN (a known path nothing closes yet; reads as not passed): " + ", ".join(rep["open"]))
    if rep.get("inconclusive"):
        lines.append("INCONCLUSIVE (never exercised in the control; their patches stay [I]): "
                     + ", ".join(rep["inconclusive"]))
    if rep.get("note"):
        lines.append(f"note: {rep['note']}")
    for why in rep["invalid"][:5]:
        lines.append(f"invalid: {why}")
    if len(rep["invalid"]) > 5:
        lines.append(f"invalid: ... and {len(rep['invalid']) - 5} more")
    lines.append("classes: " + ", ".join(f"{c} {rep['counts'][c]}" for c in CLASSES))
    for pid, p in rep["paths"].items():
        st = rep.get("path_status", {}).get(pid, "-")
        lines.append(f"path {pid:<5} exercised={'yes' if p['exercised'] else 'no '} "
                     f"leaks={p['leaks']} {st}")
    lines.append("No known client path uses an IP literal: the canaries are the only proof for R1 and R2.")
    leaks = [e for e in rep["events"] if e["class"] == "LEAK"]
    absorbed = [e for e in rep["events"] if e["class"] == "ABSORBED"]
    opens = [e for e in rep["events"] if e["class"] == "OPEN"]
    for title, evs in (("LEAK", leaks), ("OPEN", opens), ("ABSORBED", absorbed)):
        room = SUMMARY_MAX - len(lines) - 2
        if evs and room > 0:
            lines.append(f"{title} ({len(evs)}):")
            lines.extend(_evline(e) for e in evs[:room])
            if len(evs) > room:
                lines[-1] = f"  ... and {len(evs) - room + 1} more in report.json"
    return lines[:SUMMARY_MAX]


def load_paths(path: str) -> dict:
    try:
        with open(path) as f:
            return json.load(f).get("paths", {})
    except FileNotFoundError:
        return {}


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("run")
    ap.add_argument("--role", choices=("control", "subject"), required=True)
    ap.add_argument("--control", help="the control run's report.json (subject only)")
    ap.add_argument("--paths", default=DEFAULT_PATHS)
    ap.add_argument("--server-name", default=SERVER_NAME)
    ap.add_argument("--label", default="")
    ap.add_argument("--noenv", action="store_true", help="the run is the no-env subject: every scenario's begin stage must record noenv")
    a = ap.parse_args(argv)
    control = None
    if a.control:
        with open(a.control) as f:
            control = json.load(f)
    rep = build_report(load_run(a.run), a.role, load_paths(a.paths), a.server_name, control, noenv=a.noenv)
    lines = summary(rep, a.label or os.path.basename(os.path.normpath(a.run)))
    with open(os.path.join(a.run, "report.json"), "w") as f:
        json.dump(rep, f, indent=1, sort_keys=True, default=str)
    with open(os.path.join(a.run, "summary.txt"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))
    return EXIT[rep["verdict"]]


if __name__ == "__main__":
    sys.exit(main())
