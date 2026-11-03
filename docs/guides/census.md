# The census: proving the patched client phones nobody

Packet CEN-1 built the recorders and the report; CEN-2 (helper verbs),
CEN-3 (shim, rootless, orchestrator, eve-dns log fields) and CEN-4 (client
scenarios) wire them into a run. The design is the research lane
R-CENSUS (`out/research/R-CENSUS.md`, gitignored because it names client
endpoints). The patcher it checks is `docs/guides/patcher.md`; the jail it extends
is `docs/guides/jail.md`.

## Why the jail alone cannot answer the question

The jail's netns has no default route. A connect to anything outside
10.137.0.0/30, and every IPv6 connect, fails with ENETUNREACH **before**
any netfilter hook, so nothing records it. `nf_log` is off in non-init
netns (`nf_log_all_netns=0`), so the jail's `log` rules never fire either.
Only DNS lookups are visible today. "Nothing in eve-dns.log" therefore does
not mean "nothing attempted": an IP literal is invisible.

The census gives the netns a default route (v4 and v6) into a TUN device,
`census0`, that a process we own reads. Every attempt becomes a packet in
a file, and nothing can leave: a TUN device has no wire.

## The recorders

| id | file in RUN/ | written by | sees |
|---|---|---|---|
| R1 | `tun.jsonl` | `tools/census_sink.py` | every packet routed off the jail, v4 and v6: addresses, ports, TCP flags, ICMP type, up to 2048 payload bytes |
| R2 | `nft.json` | the `eve_census` nft table, dumped by the helper (CEN-2) | per-flow counters for every output packet, before the jail's drop: catches what goes to 10.137.0.1 or loopback, which R1 never sees |
| R3 | `dns.log` | eve-dns, this run's slice (CEN-3 adds `peer=` and `t=`) | every name looked up |
| R4a | `exec.log` | `tools/census/browser-shim` | every browser hand-off (`xdg-open`, Wine's `winebrowser` list), argv logged, exit 1 |
| R4b | `proxy.jsonl` | `tools/census_proxy.py` on 127.0.0.1:9 | every request the patcher's Tier A sends to the dead proxy, named; answered 403 |
| — | `canary.jsonl` | `tools/census_canary.py` | the canary windows and the host canaries' results |
| — | `crash.txt` | the orchestrator (CEN-3) | new crash-DB files since the run's mark |

**The sink opens no socket.** `census_sink.py` and `census_wire.py`, the
only code it loads of ours, import nothing from `socket`, `ssl`, `http`,
`urllib`, `subprocess` or `ctypes`. `test_census.py` pins that twice: by
reading the source, and by loading the sink in a fresh interpreter and
listing `sys.modules`. Its only writes are its log and, in handshake mode,
replies into the same TUN fd. It attaches only to a device named
`census0`, and it never creates one: the helper or the rootless holder
does, owned by the caller.

**The proxy binds only 127.0.0.1** (a constant with no option; the test
reads every `bind` call), and it never connects anywhere.

### Sink modes

- `observe` (the default, every verdict run): record, never answer. The
  client sees a timeout and retries; each retry is recorded.
- `handshake` (control and diagnosis runs): SYN gets a SYN-ACK (MSS 1400);
  the first data segment gets an ACK and then an RST; an echo request gets
  an echo reply; UDP gets nothing. The responder keeps no state (its
  sequence number is a hash of the flow). The first data bytes are then
  in R1, and the report names an IP-literal flow by its TLS SNI, HTTP
  `Host:` or DNS question.

`census_sink.py --replay in.pcap --out x.jsonl [--mode handshake --replies
out.pcap]` feeds a pcap (raw IP or Ethernet) through the same code, for
testing without a device.

## The canaries

A canary is a deliberate attempt at a target nobody can own: TEST-NET-1,
the IPv6 documentation prefix or a `.invalid` name. Each must reach the
recorders `census_canary.py plan` names, or the run is **INVALID**. Three
parties fire them, each bracketing its rows with
`census_canary.py mark start|end WINDOW`:

- `host`: `census_canary.py run` in the client's view: TCP to a v4 and a
  v6 literal, a DNS query to a hard-coded resolver address, and a connect
  to 10.137.0.1:8443, which must reach R2 but never R1 (the jail drops
  it). `run` refuses unless both default routes go to `census0` and
  nowhere else, so on an ordinary host it sends nothing.
- `wine`: the orchestrator, through Proton: `ping.exe` (ICMP) and
  `ping.exe` with a name (Wine's resolver; GE-Proton10-34 has no `nslookup.exe`).
- `client`: the `census_canary` scenario: the client's Python sockets and
  resolver, `urllib2` and `requests` (through the proxy when patched,
  direct in the control), WinHTTP, and `ShellExecute` (no launcher when
  patched, `xdg-open` in the control).

A canary target seen outside every window is a LEAK. No known client path
uses an IP literal, so the canaries are the only proof that R1 and R2 see;
the summary says so on every run.

## Classes

| class | events |
|---|---|
| ALLOWED | TCP to 10.137.0.1 on 26000 or 5222; DNS to 10.137.0.1; a lookup of the server name |
| ABSORBED | any dead-proxy request (host listed); a connect to 127.0.0.1:9; TCP to 10.137.0.1:8443; a lookup under `.lndarkly.invalid` (B2 at work) or `.gateway.invalid` (tier C8 at work) |
| NOISE | kernel ICMPv6 (RS, NS, MLDv2: types 133, 135, 143) to `ff02::/16` |
| CANARY | a canary target inside a canary window (R2 and old-format R3 lines carry no time, so a canary target there always counts) |
| LOCAL | other loopback traffic. It cannot leave the netns, so it never fails V1; unjailed, loopback is the host's, so V2 requires none. This class is ours, not R-CENSUS's |
| LEAK | everything else in R1, R2 or R3; every browser launcher exec |

## Verdicts

Per copy, by `census_report.py RUN --role control|subject`:

- **INVALID**: a recorder file is missing, the sink has no clean stop line
  (it died), a canary is missing, the drop canary reached R1, or an R1 flow
  is missing from R2. The run is not evidence of anything.
- **FAIL**: any LEAK, in the subject.
- **INCONCLUSIVE**: a path the control never exercised. It is reported as
  "not passed", never as passed. The control's report lists these
  (`--role control` gives `VERDICT CONTROL` and the list); the subject's
  report reads the control's (`--control CONTROL/report.json`) and marks
  each such path INCONCLUSIVE. With no valid control, every path is
  INCONCLUSIVE. If the crash path never fires under Wine, B1 stays [I],
  and the summary's second line says so.
- **V1, patch verified (jailed)**: no LEAK at all, and every path exercised
  in the control.
- **V2, eligible to run unjailed (strict)**: V1, plus no lookup at all but
  the server name (even an absorbed `.lndarkly.invalid` lookup would reach
  an ISP resolver unjailed) and no LOCAL traffic. Absorbed proxy hits are
  listed; they stay absorbed unjailed only while nothing on the host
  listens on port 9.

The owner's criterion ("zero outbound attempts except to our server, for
both patched builds, and the unpatched copy showing the attempts") is V1 on
both subjects with a complete control. Whether the jail can go is V2 plus
R-CENSUS §5, which a census cannot answer: any server the client talks to
can push code into it.

Exit status: 0 for V1, V2 or a complete control; 1 FAIL; 2 INVALID;
3 INCONCLUSIVE. The report writes `RUN/report.json` (every event with its
class and note) and `RUN/summary.txt` (at most 30 lines, the verdict
first).

## The path table

Which events belong to which phone-home path comes from
`out/census/paths.json` (gitignored: the names are the client's). Each path
lists `names` (exact, with subdomains, or a glob), `endpoints`
(`addr:port`), `exec` (the browser hand-off) and `crash` (a new crash-DB
file counts as exercised). An event matches a path by its looked-up name,
its SNI or Host, its proxy host or its endpoint; canary events never match.
A path is exercised in the control when any event matches it; it leaks in
the subject when a LEAK event matches it. With no path table the subject
is INCONCLUSIVE.

## What a run needs from the other packets

CEN-1 runs nothing by itself. The hooks it expects are listed in
`docs/lanes/cen1.md` ("Hooks for CEN-2, CEN-3 and CEN-4").

## Running it (CEN-2, CEN-3)

### Once: the helper with the census verbs (owner, sudo, about 5 minutes)

The helper is a snapshot, so the new verbs exist only after a reinstall,
and sudo grants them only after four more lines:

```sh
tools/install-jail-helper.sh --diff | less     # read it: the census section, and the
                                               # jail chains lose their dead `log` rules
sudo tools/install-jail-helper.sh
sudo visudo -f /etc/sudoers.d/eve-offline      # add the four lines below
```

```sudoers
youruser ALL=(root) NOPASSWD: /usr/local/sbin/eve-offline-jail census-up, \
                              /usr/local/sbin/eve-offline-jail census-down, \
                              /usr/local/sbin/eve-offline-jail census-dump, \
                              /usr/local/sbin/eve-offline-jail census-verify
```

(Append them to the existing entry, or keep them as a second `youruser
ALL=(root) NOPASSWD:` line in the same file; `visudo -c` checks it.) Like
every verb, they take no arguments. The rootless path (`--rootless`) needs
neither step.

### The run

```sh
tools/census.sh --check     # the preflight for every (copy, scenario); starts nothing
tools/census.sh --all       # unattended, hours; ends with out/census/<utc>/SUMMARY.md
```

`census.sh` runs, per copy (the control first) and per scenario,
`EVE_CLIENT_DIR=COPY tools/client-ci.sh run SCEN --census RUN [--census-control]
--census-mode M --timeout T`, where `T` is the scenario's own
`EOCI_CENSUS_TIMEOUT_S` (CEN-4) and `M` is `handshake` for the control,
`observe` for a subject (`--mode` overrides both). One client-ci run is one
**segment**:

1. the stack and the jail as ever, and `verify` (no route but the /30);
2. `census-up` (helper) or the holder's census (`jail-rootless.sh --census
   up`, started before the server), then `census-verify` (or the rootless
   gate, which now expects census0 and skips its own probes and lookups,
   since the recorders would count them as the client's);
3. the client, with `EVE_CENSUS=1` (and `EVE_CENSUS_CONTROL=1` for the
   control) in its launch-env. The shim refuses either key unless census0 is
   a TUN device here and both default routes go to it alone. In census mode
   it puts `tools/census/bin` first on `PATH`, sets `CENSUS_EXEC_LOG`, passes
   `EVE_CENSUS=1` to the client, fires the host canaries
   (`census_canary.py run`) and the Wine ones (`ping.exe` twice: an echo, and a name lookup
   between `mark start|end wine`), then starts the client. The control also
   loses the patcher's env and args, the winebrowser override and
   `/noCrashReportUpload`;
4. after the client: `census-dump` into `RUN/nft-SCEN.json`, `census-down`
   (SIGTERM, so each recorder writes its stop line), and `verify` again,
   which must pass;
5. the segment is appended to `RUN`: `tun.jsonl` and `proxy.jsonl` (each
   must hold exactly one start and one clean stop, else a fault), `exec.log`,
   `canary.jsonl` plus the scenario's own `c:\eve-logs\client-ci\SCEN.census.jsonl`
   (the client window), the resolver log's bytes written since census-up
   (`dns.log`), new crash-DB files under the prefix (`crash.txt`), and a
   `segments.tsv` row.

Then `census.sh` merges the `nft-*.json` dumps into `nft.json` and runs
`census_report.py RUN --role control`, then each subject with `--control
CONTROL/report.json`. A segment fault (`RUN/faults.txt`: a recorder that
died or restarted, a refused segment, `verify` failing after census-down)
makes that copy INVALID whatever the report says. `SUMMARY.md` has one
section per copy; the exit status is the worst verdict.

### eve-dns: `peer=` and `t=`

Each query line now ends ` peer=IP:PORT t=UNIXMS` (`peer=` is absent only
when no transport is involved, as in the unit tests). The fields are
appended, so `jail-drops.sh` reads the line as before; `census_report.py`
reads both formats.

### The path table

`out/census/paths.json` is `{"paths": {ID: {...}}}`, one entry per
phone-home path (gitignored: it names the client's endpoints). An entry has
`what` (text), and any of `names` (lookups, SNI, Host or proxy hosts that
are this path's evidence; exact, with subdomains, or a glob), `endpoints`
(`addr:port`), `exec: true` (a browser hand-off counts) and `crash: true`
(a new crash-DB file counts). CEN-4's scenarios use three invented names,
which must be in it for N11 and P13 to count as exercised:
`census-shell.census.invalid` and `census-urlutil.census.invalid` (N11) and
`census-video.census.invalid` (P13).

### The phone-home scenarios (issue #40, `docs/lanes/phone-home.md`)

Three scenarios beyond CEN-4's six, for the paths those never reach. All
refuse outside a census, fire only invented `.invalid` names, and are in
`census.sh`'s default list.

| scenario | fires | control sees | subject (tier C on) |
|---|---|---|---|
| `census_report_bug` | `BugReporter(<the client's own server constant>)`: `IsBugsServiceOnline`, then `SendBugReport` with invented text and no files | a lookup of the bug-report host | only a connect to `127.0.0.1:9` |
| `census_urls` | `photo` and `browserImage` `GetTextureFromURL`; the minibrowser's `GetStringFromURL` for http, https and **ftp**; `ImageDownloader.fetch`; `Subtitles.LoadSubtitleFile`; `MMOSGateway.get_data_from_url`; `send_metrics_to_victoria_metrics` with its default URL | a lookup per invented name, and one for the metrics host | the same lookups (URLs are data: nothing to patch), except the metrics URL, which is a connect to `127.0.0.1:9` |
| `census_cdn` | an invented index row for a `res:/` path no ResFiles holds, read through `blue.paths.GetFileContentsWithYield`, with the cache folder on an empty temp directory; it also logs `blue.remoteFileCache.server`, `.backupServer`, `.prefix` | lookups of the CDN names, perhaps `wpad` | only a connect to `127.0.0.1:9` |

[A] for `census_cdn`: blue reads the cache folder, the servers and the index
at call time, and a row added at run time is honoured. Falsifier: no event
at all in the control. The three attributes are in the census log, so a
changed constant shows up there.

The ftp case is the Tier A gap: `ftp://` goes past `HTTP(S)_PROXY`, and no
patch closes it yet (the launcher-env fix, G2, was not built; the no-jail
research decides). Its name is a **known-open** row (class `OPEN`,
`KNOWN_OPEN` in `census_report.py`): not a LEAK, and never a pass. A subject
that shows it reads INCONCLUSIVE with the note "known-open path(s)", and its
path (P8b) has status OPEN.

### A segment that did not run is a fault

`census.sh` turns any non-exempt segment that exited non-zero, has no
segment row, or is listed in `noclient.txt` into a line of `faults.txt`, so
the copy is INVALID and none of its paths can read as passed. Exempt from
the exit-code rule (not from the no-client rule): `census_crash` and
`census_hang`, which may end with the client dead. The scenarios' own checks
are real too: a call that raised `ImportError`, `NameError` or `TypeError`
never reached the network and fails the check; `census_cdn` also checks that
the cache folder really moved, that the Remote file system is registered and
that the invented index row was accepted.

### The no-env run

`tools/census.sh --copies tq-control:control,tq-3662168:noenv` (or
`tools/client-ci.sh run SCEN --census RUN --census-noenv`) launches the
patched copy with the patcher's **files** and **args** but none of its env
(`EVE_CENSUS_NOENV=1` in the launch-env, refused outside a census exactly
like `EVE_CENSUS_CONTROL`, and with it). Boot, character select and
`census_cc` must show no `sentry`, public-IP, `*-public-gateway.*`, CDN or
analytics name, and the gateway discovery name should appear under
`.gateway.invalid`. The role has its own scenario list, `census_canary,
census_idle,census_cc` (`--noenv-scenarios` changes it): every other
scenario fires names the launcher's proxy absorbs, so without it they would
be LEAKs by construction. Each scenario's `begin` stage records
`noenv=true` (the launcher puts `EVE_CENSUS_NOENV=1` in the client's
environment), `census_report.py --noenv` turns a missing or false one into
INVALID, and the run directory holds `role.txt`: a report proves it was no-env. It is the only proof that tier C works without the
launcher. The `noenv` copy is judged as a subject against the control, in
its own `RUN-noenv` directory.

A complete run is the control copy (`apply --tier B --only B0,P0`: `--only` does
not limit the prefs tier, so `--tier B` is what keeps the control's prefs stock), the
subject, the no-env subject, and every scenario above plus the six of CEN-4.
`N12` (Streamline) cannot be exercised on a software GPU: `verify` proves the
byte, and the owner reads `WINEDEBUG=+process` once on an NVIDIA machine for
`nvngx_update`; report it INCONCLUSIVE, never passed.

### What is still an assumption

Besides CEN-1's: the nft census table, the TUN device and the two sysctls
work in a user-namespace-owned netns (rootless only; falsifier:
`tools/jail-rootless.sh --census selftest RUNDIR` fails at that step); and
Proton's Wine ships `ping.exe` (R-CENSUS; falsifier: the wine canaries are
missing from a run whose host canaries are present). It does not ship
`nslookup.exe`: that was the wine-dns canary's whole failure in the 2026-10-30
rootless runs (checked in GE-Proton10-34), so the lookup is now `ping.exe` with a
name. A clean user+net namespace with the same `ping_group_range` formula sends the
echo (so wine-icmp's cause is not the sysctl; see `docs/lanes/phone-home.md`), and
each run now leaves `wine-canary.log` and `wine-canary.out` in its run directory.
