#!/usr/bin/env python3
"""Check a run's SERVER and BOT logs against a scenario's declared expectations.

A client-ci scenario sees only the client.  Half of every lane's human check is
a line in a log: `UNANSWERED <service>.<method>` (a missing arm), a `[hack]`
line that must be there, a tick-health maximum.  Those are declared where the
check lives, as comment directives in the scenario's own header, or in a
`log-checks/<name>.expect` file for a check that has no client part (a bot
soak):

    # SERVER-EXPECT: present <regex>          eve-server.log has a matching line
    # SERVER-EXPECT: absent  <regex>          ... has none
    # BOTS-EXPECT:   present|absent <regex>   the same against eve-bots.log
    # SERVER-MAX[skip=N]: <regex, one group> <= <number>
    # BOTS-MAX[skip=N]:   <regex, one group> <= <number>
                                              every match's captured float is
                                              <= number; the first N matches
                                              are skipped (warm-up)

Usage:

    python3 -I tools/client-ci/server-expect.py NAME [--log PATH] [--bots-log PATH]
    python3 -I tools/client-ci/server-expect.py --list

Run it right after `tools/client-ci.sh run NAME` (`up.sh` truncates
`logs/eve-server.log` at each start, so the log is that run's).  Exit 0: every
directive holds.  1: one does not.  2: usage.  3: a log it needs is missing,
which is "could not look", never a pass.  Stdlib only; reads files only.
"""
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
SCEN = os.path.join(HERE, "scenarios")
CHECKS = os.path.join(HERE, "log-checks")

DIRECTIVE = re.compile(
    r"^#\s*(SERVER|BOTS)-(EXPECT|MAX)(?:\[skip=(\d+)\])?:\s*(.*?)\s*$")
MAX_BODY = re.compile(r"^(.*\S)\s+<=\s+([0-9]+(?:\.[0-9]+)?)$")


def parse(text):
    """The directives in `text`, in order: dicts with log, kind, ..."""
    out = []
    for n, line in enumerate(text.splitlines(), 1):
        m = DIRECTIVE.match(line)
        if not m:
            continue
        log, kind, skip, body = m.group(1), m.group(2), m.group(3), m.group(4)
        if kind == "EXPECT":
            parts = body.split(None, 1)
            if len(parts) != 2 or parts[0] not in ("present", "absent"):
                raise ValueError(f"line {n}: EXPECT needs 'present|absent <regex>'")
            out.append({"log": log, "kind": "expect", "want": parts[0],
                        "rx": re.compile(parts[1]), "line": n, "src": parts[1]})
        else:
            mm = MAX_BODY.match(body)
            if not mm:
                raise ValueError(f"line {n}: MAX needs '<regex> <= <number>'")
            rx = re.compile(mm.group(1))
            if rx.groups != 1:
                raise ValueError(f"line {n}: MAX regex needs exactly one group")
            out.append({"log": log, "kind": "max", "rx": rx, "skip": int(skip or 0),
                        "limit": float(mm.group(2)), "line": n, "src": mm.group(1)})
    return out


def evaluate(directives, logs):
    """`[(ok, message)]`.  `logs` maps 'SERVER'/'BOTS' to a list of lines or None."""
    results = []
    for d in directives:
        lines = logs.get(d["log"])
        tag = f"{d['log']}-{d['kind'].upper()}"
        if lines is None:
            results.append((None, f"{tag} line {d['line']}: the {d['log'].lower()} log is missing"))
            continue
        if d["kind"] == "expect":
            hits = [ln for ln in lines if d["rx"].search(ln)]
            ok = bool(hits) if d["want"] == "present" else not hits
            shown = (hits[0].strip()[:160] if hits else "no matching line")
            results.append((ok, f"{tag} {d['want']} /{d['src']}/ -> "
                                f"{len(hits)} line(s); {shown}"))
        else:
            vals = []
            for ln in lines:
                m = d["rx"].search(ln)
                if m:
                    try:
                        vals.append(float(m.group(1)))
                    except ValueError:
                        pass
            vals = vals[d["skip"]:]
            worst = max(vals) if vals else None
            ok = worst is not None and worst <= d["limit"]
            results.append((ok, f"{tag} /{d['src']}/ <= {d['limit']:g} "
                                f"(skip {d['skip']}) -> {len(vals)} value(s), worst "
                                f"{'none' if worst is None else f'{worst:g}'}"))
    return results


def read_lines(path):
    try:
        with open(path, errors="replace") as fh:
            return fh.read().splitlines()
    except OSError:
        return None


def find_source(name):
    for path in (os.path.join(SCEN, name + ".py"), os.path.join(CHECKS, name + ".expect")):
        if os.path.isfile(path):
            return path
    return None


def names():
    out = []
    for d, ext in ((SCEN, ".py"), (CHECKS, ".expect")):
        if os.path.isdir(d):
            out += [n[:-len(ext)] for n in sorted(os.listdir(d))
                    if n.endswith(ext) and not n.startswith("_")]
    return out


def main(argv):
    args = list(argv)
    server_log = os.path.join(REPO, "logs", "eve-server.log")
    bots_log = os.path.join(REPO, "logs", "eve-bots.log")
    if "--list" in args:
        for n in names():
            path = find_source(n)
            with open(path) as fh:
                try:
                    count = len(parse(fh.read()))
                except ValueError:
                    count = -1
            print(f"  {n:24} {count} directive(s)")
        return 0
    for flag in ("--log", "--bots-log"):
        if flag in args:
            i = args.index(flag)
            try:
                value = args[i + 1]
            except IndexError:
                print(f"{flag} needs a path", file=sys.stderr)
                return 2
            del args[i:i + 2]
            if flag == "--log":
                server_log = value
            else:
                bots_log = value
    if len(args) != 1:
        print(__doc__.split("Usage:")[1].split("Run it")[0].strip(), file=sys.stderr)
        return 2
    path = find_source(args[0])
    if path is None:
        print(f"no scenario or log-check named {args[0]!r}; try --list", file=sys.stderr)
        return 2
    with open(path) as fh:
        try:
            directives = parse(fh.read())
        except ValueError as e:
            print(f"{path}: {e}", file=sys.stderr)
            return 2
    if not directives:
        print(f"{args[0]}: declares no log expectations")
        return 0
    results = evaluate(directives, {"SERVER": read_lines(server_log),
                                    "BOTS": read_lines(bots_log)})
    bad = missing = 0
    for ok, message in results:
        mark = "ok  " if ok else ("MISS" if ok is None else "FAIL")
        print(f"{mark} {message}")
        bad += ok is False
        missing += ok is None
    if bad:
        return 1
    return 3 if missing else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
