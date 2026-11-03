#!/usr/bin/env python3
"""The census canaries (packet CEN-1, `docs/guides/census.md`; R-CENSUS §1.5).

A canary is a deliberate attempt at a target nobody can own (TEST-NET-1
`192.0.2.0/24`, the IPv6 documentation prefix `2001:db8::/32`, or a name
under `.invalid`). Each one must show up in the recorders named in
`CANARIES`, or `census_report.py` calls the run INVALID: a harness that
cannot see a canary cannot vouch for a silence.

The table is the single source. Three parties fire the rows (`by`):

  host      this script, `run`, in the client's view of the jail;
  wine      the orchestrator (`tools/census.sh`, CEN-3) through Proton;
  client    the client scenario `census_canary` (CEN-4).

  census_canary.py plan                       # the table, as JSON
  census_canary.py mark start|end WINDOW --out RUN/canary.jsonl
  census_canary.py run --out RUN/canary.jsonl # the host rows, bracketed

Each party brackets its rows with `mark start`/`mark end` (`run` does its
own). A canary target seen **inside** a window is CANARY; the same target
outside every window is a LEAK.

`run` refuses (exit 2) unless the v4 and v6 default routes both go to
`census0` and nowhere else (`/proc/net/route`, `/proc/net/ipv6_route`), so
on an ordinary host it sends nothing.
"""

import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.append(_HERE)

import argparse  # noqa: E402
import json  # noqa: E402
import time  # noqa: E402

import census_wire as wire  # noqa: E402

DEVICE = "census0"
CONNECT_TIMEOUT = 2.0
#: The jail's host side: the only address the netns routes without census0.
JAIL_HOST = "10.137.0.1"
#: The port the jail drops (the gateway env points there): R2 sees it, R1 never does.
DROP_PORT = 8443

ROLES = ("subject", "control")


def _r1(proto, dst, port=None):
    e = {"rec": "R1", "proto": proto, "dst": dst}
    if port is not None:
        e["dport"] = port
    return e


def _r2(set_name, dst, key):
    return {"rec": "R2", "set": set_name, "dst": dst, "key": key}


def _r3(name):
    return {"rec": "R3", "name": name}


def _proxy(host):
    return {"rec": "R4p", "host": host}


def _exec(needle):
    return {"rec": "R4e", "arg": needle}


def _both(*ev):
    return {"subject": list(ev), "control": list(ev)}


#: id -> {by, how, targets (what makes an event this canary's), expect
#: (per role: evidence that must be present), absent (per role: evidence
#: that must NOT be present)}. Targets are matched against R1/R2 addresses,
#: R3 names, proxy hosts and exec args.
CANARIES = {
    "v4-tcp": {"by": "host", "how": "connect 192.0.2.10:443",
               "targets": ["192.0.2.10"],
               "expect": _both(_r1("tcp", "192.0.2.10", 443), _r2("tcp4", "192.0.2.10", 443))},
    "v6-tcp": {"by": "host", "how": "connect [2001:db8::10]:443",
               "targets": ["2001:db8::10"],
               "expect": _both(_r1("tcp", "2001:db8::10", 443), _r2("tcp6", "2001:db8::10", 443))},
    "dns-literal": {"by": "host", "how": "UDP DNS query for canary-dns.census.invalid to 192.0.2.53:53",
                    "targets": ["192.0.2.53", "canary-dns.census.invalid"],
                    "expect": _both(_r1("udp", "192.0.2.53", 53), _r2("udp4", "192.0.2.53", 53))},
    "drop-path": {"by": "host", "how": f"connect {JAIL_HOST}:{DROP_PORT}",
                  "targets": [f"{JAIL_HOST}:{DROP_PORT}"],
                  "expect": _both(_r2("tcp4", JAIL_HOST, DROP_PORT)),
                  "absent": _both(_r1("tcp", JAIL_HOST, DROP_PORT))},
    "wine-icmp": {"by": "wine", "how": "proton run ping.exe -n 1 192.0.2.7",
                  "targets": ["192.0.2.7"],
                  "expect": _both(_r1("icmp", "192.0.2.7"), _r2("oth4", "192.0.2.7", "icmp"))},
    "wine-dns": {"by": "wine", "how": "proton run ping.exe -n 1 canary-wine.census.invalid (the lookup)",
                 "targets": ["canary-wine.census.invalid"],
                 "expect": _both(_r3("canary-wine.census.invalid"))},
    "py-v4": {"by": "client", "how": "socket.create_connection(('192.0.2.11', 443), 2)",
              "targets": ["192.0.2.11"],
              "expect": _both(_r1("tcp", "192.0.2.11", 443), _r2("tcp4", "192.0.2.11", 443))},
    "py-v6": {"by": "client", "how": "socket.create_connection(('2001:db8::11', 443), 2)",
              "targets": ["2001:db8::11"],
              "expect": _both(_r1("tcp", "2001:db8::11", 443), _r2("tcp6", "2001:db8::11", 443))},
    "py-dns": {"by": "client", "how": "socket.gethostbyname('canary-py.census.invalid')",
               "targets": ["canary-py.census.invalid"],
               "expect": _both(_r3("canary-py.census.invalid"))},
    "http-urllib2": {"by": "client", "how": "urllib2.urlopen('http://192.0.2.12/')",
                     "targets": ["192.0.2.12"],
                     "expect": {"subject": [_proxy("192.0.2.12")],
                                "control": [_r1("tcp", "192.0.2.12", 80)]}},
    "http-requests": {"by": "client", "how": "requests.get('https://canary-rq.census.invalid/')",
                      "targets": ["canary-rq.census.invalid"],
                      "expect": {"subject": [_proxy("canary-rq.census.invalid")],
                                 "control": [_r3("canary-rq.census.invalid")]}},
    "winhttp": {"by": "client", "how": "ctypes WinHttpOpen/WinHttpConnect('192.0.2.13', 443)/WinHttpSendRequest",
                "targets": ["192.0.2.13"],
                "expect": _both(_r1("tcp", "192.0.2.13", 443))},
    "browser": {"by": "client", "how": "blue.os.ShellExecute('https://canary-br.census.invalid/')",
                "targets": ["canary-br.census.invalid"],
                "expect": {"subject": [], "control": [_exec("canary-br.census.invalid")]},
                "absent": {"subject": [_exec("canary-br.census.invalid")], "control": []}},
}

#: The rows this script fires, in order.
HOST_ROWS = ("v4-tcp", "v6-tcp", "dns-literal", "drop-path")


def now_ms() -> int:
    return int(time.time() * 1000)


def append(path: str, obj: dict) -> None:
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        os.write(fd, (json.dumps(obj, sort_keys=True) + "\n").encode())
    finally:
        os.close(fd)


# ------------------------------------------------------------------ the guard


RTF_REJECT = 0x0200


def v4_defaults(text: str) -> list:
    """Interfaces of the v4 default routes in `/proc/net/route` text."""
    out = []
    for line in text.splitlines()[1:]:
        f = line.split()
        if len(f) >= 8 and f[1] == "00000000" and f[7] == "00000000":
            if not int(f[3], 16) & RTF_REJECT:
                out.append(f[0])
    return out


def v6_defaults(text: str) -> list:
    """Interfaces of the v6 default routes in `/proc/net/ipv6_route` text
    (reject routes, which the kernel parks on `lo`, are skipped)."""
    out = []
    for line in text.splitlines():
        f = line.split()
        if len(f) >= 10 and f[0] == "0" * 32 and f[1] == "00":
            if not int(f[8], 16) & RTF_REJECT:
                out.append(f[9])
    return out


def guard(route4: str, route6: str):
    """None when census mode is in place, else why not."""
    d4, d6 = v4_defaults(route4), v6_defaults(route6)
    if d4 != [DEVICE]:
        return f"the v4 default routes are {d4 or 'none'}, not exactly [{DEVICE}]"
    if d6 != [DEVICE]:
        return f"the v6 default routes are {d6 or 'none'}, not exactly [{DEVICE}]"
    return None


def _read(path: str) -> str:
    try:
        with open(path) as f:
            return f.read()
    except OSError:
        return ""


# ------------------------------------------------------------------- the rows


def fire(row: str) -> str:
    """Fire one host row; the result as text. Imports `socket` lazily so
    `plan`, `mark` and the tests never touch it."""
    import socket

    def tcp(host, port):
        fam = socket.AF_INET6 if ":" in host else socket.AF_INET
        s = socket.socket(fam, socket.SOCK_STREAM)
        s.settimeout(CONNECT_TIMEOUT)
        try:
            s.connect((host, port))
            return "connected"
        except OSError as err:
            return type(err).__name__ + (f" errno {err.errno}" if err.errno else "")
        finally:
            s.close()

    if row == "v4-tcp":
        return tcp("192.0.2.10", 443)
    if row == "v6-tcp":
        return tcp("2001:db8::10", 443)
    if row == "drop-path":
        return tcp(JAIL_HOST, DROP_PORT)
    if row == "dns-literal":
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            s.sendto(wire.dns_query("canary-dns.census.invalid"), ("192.0.2.53", 53))
            return "sent"
        except OSError as err:
            return type(err).__name__
        finally:
            s.close()
    raise ValueError(f"{row} is not a host row")


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("plan")
    m = sub.add_parser("mark")
    m.add_argument("edge", choices=("start", "end"))
    m.add_argument("window")
    m.add_argument("--out", required=True)
    r = sub.add_parser("run")
    r.add_argument("--out", required=True)
    a = ap.parse_args(argv)

    if a.cmd == "plan":
        print(json.dumps(CANARIES, indent=1, sort_keys=True))
        return 0
    if a.cmd == "mark":
        append(a.out, {"event": f"window-{a.edge}", "window": a.window, "t": now_ms()})
        return 0
    why = guard(_read("/proc/net/route"), _read("/proc/net/ipv6_route"))
    if why:
        print(f"census_canary: refusing: {why}", file=sys.stderr)
        return 2
    append(a.out, {"event": "window-start", "window": "host", "t": now_ms()})
    for row in HOST_ROWS:
        t = now_ms()
        append(a.out, {"event": "canary", "id": row, "t": t, "result": fire(row)})
    append(a.out, {"event": "window-end", "window": "host", "t": now_ms()})
    return 0


if __name__ == "__main__":
    sys.exit(main())
