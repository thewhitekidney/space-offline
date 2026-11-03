# The phone-home patcher (`tools/eve_patch.py`)

Owner decision O11 (`docs/port-tracker.md` §81): friends run Linux. Each
person runs a patcher, through our setup scripts, that removes the phone-home
paths from **their own copy** of the client. We never hand out client bytes.
O17: the patcher knows which build it is patching. Each build has its own
manifest, `tools/patches/<build>.json`, because the target build will move
to the newest one later. The jail (`docs/guides/jail.md`) stays as the second layer.
No client patch limits what a hostile server can do: a server can run Python
inside the client.

The research behind every recipe, with its [V]/[I] marks, is
`out/research/R-PATCH.md` (lane R-PATCH). The committed list of every
outbound path, when it fires and what closes it, is
`docs/reference/phone-home.md`. This page is
the operator's view.

## Commands

All commands use only the Python standard library. Run them with the
isolated interpreter, so the client directory cannot plant a module:

    python3 -I tools/eve_patch.py status     --client DIR
    python3 -I tools/eve_patch.py plan       --client DIR      # dry run
    python3 -I tools/eve_patch.py apply      --client DIR [--tier A,P,B,C] [--only B0,B1]
    python3 -I tools/eve_patch.py verify     --client DIR [--no-resfiles]
    python3 -I tools/eve_patch.py revert     --client DIR
    python3 -I tools/eve_patch.py launch-env --client DIR [--server ADDR] [--json] [--check-args "ARGS"]

`DIR` is the install, the directory that holds `start.ini` and `bin64/`
(for example `.../drive_c/CCP/EVE/tq-offline`). `--build N` names the build
when `start.ini` cannot be read. `--prefs PATH` points at a prefs.ini
directly; otherwise the patcher finds it the way `run-client.sh` does.
`--resfiles DIR` points at the `ResFiles` directory; the default is the
`ResFiles` directory beside the install.

Exit codes: 0 means ok. 1 means not (fully) applied, or a check failed.
2 means refused, and nothing was written. 3 means `--check-args` found a
forbidden argument.

## What it changes, and why

| id | tier | file | change | closes |
|---|---|---|---|---|
| B0 | B | `bin64/blue.dll` | 4 bytes: the `root:/manifest.dat` self-check always passes | lets the client accept every other edit (same as `client-patch-manifest.py`) |
| P0 | B | `start.ini` | `cryptoPack = Placebo`, LF line endings (the exact bytes `client-prepare.py` always wrote) | not phone-home; needed to talk to our server |
| B1 | B | `bin64/exefile.exe` | the Crashpad upload URL is built with length 0 | crash-dump upload to sentry.io ([I] until a forced crash proves it) |
| B2 | B | `bin64/launchdarkly_stackless_client_sdk.pyd` | six `launchdarkly.com` hosts become `lndarkly.invalid` (same length, a reserved TLD) | LaunchDarkly flags and events |
| prefs | P | `prefs.ini` | `breakpadUpload=0`, `sentry_io_dsn=http://0@127.0.0.1:9/0`, `chatserver_disable_tls=1` | crash reporting and Sentry, even on a launch without our env; plain-TCP chat |
| launch | A | none (env/args) | `HTTP(S)_PROXY=http://127.0.0.1:9`, `NO_PROXY=<server>,localhost,127.0.0.1`, `EVE_PUBLIC_GATEWAY_ADDRESS=<server>:8443`, `EVE_CLIENT_SENTRY_DSN=...`, `/noCrashReportUpload`, `/resfileserver=http://127.0.0.1:9/` | every Python HTTP path (Sentry, ipify, Google Analytics, ...), the gRPC gateway, the primary resource CDN |
| never | A | none | the launcher never passes `/refreshToken`, `/enableTelemetry`, `/py`, `/tools=`, `/thinclient` | SSO refresh, RAD telemetry, dev hooks |

| B3 | B | `bin64/_trinity_dx12.dll` | one byte: the default Streamline `Preferences.flags` 0x49 becomes 0x01 before the code ORs in 0x84, so the over-the-air updater and downloaded plugins are off | NVIDIA Streamline OTA (`nvngx_update.exe`, plugin downloads); DLSS itself stays. [I] flag names, from the SDK header |
| C1 | C | `code.ccp` | eight members, twelve marshal strings (below): the Sentry DSN, the resource CDN (three constants), the public-IP lookup, Google Analytics, the bug-report service, VictoriaMetrics and the gateway discovery suffix become dead local addresses or a reserved name | the same paths the launch layer closes, but **in the file**, so a launch without the env is closed too |

## Tier C: constants inside `code.ccp`

`code.ccp` is a zip whose members are all method 0 and flags 0, yet every
body is a zlib stream (level 6) and the zip's CRC32 is the CRC of that
stream. So a member whose marshal string is replaced recompresses into its
own slot, at level 9 (then 8, 7, 6, with memLevel 8 and 9) until it fits,
and the rest of the slot is zeroed. Only that member's CRC and its two
size fields change, in the local header and the central directory. Nothing
moves and nothing is appended. A member that does not fit refuses, with
nothing written.

A site is `{offset, old_len, after_ascii}`: the offset of a marshal string
(`s` or `t`) in the inflated `.pyc`, the length it must have, and the whole
new value (the type byte is kept). The manifest holds each member's inflated
stock and patched sha256, so a changed member is refused before anything is
written.

**The hash of a patched `code.ccp` is canonical.** Compressed bytes depend
on the host's zlib (zlib or zlib-ng), so `sha256_patched` of a `ccp` entry
is `ccp_canon`: the sha256 of the file with each target's slot, CRC and
size fields zeroed, followed by each member's inflated-patched hash, after
inflating each from the file and checking that its headers agree. The
whole-file hash is only used for `stock`. `pin-client.sh` accepts a patched
`code.ccp` the same way.

**Tier C is in the default tiers** (`DEFAULT_TIERS` is `A,P,B,C` in
`eve_patch.py`) since the re-pin to build 3662168 (2026-10-31, issue #19). The
coordinator booted a tier C copy of 3662168 (`tq-3662168-c`) and the
regression run found nothing new, which was the condition this page used to
set ([I] then: stock `zipimport` reads sizes from the central directory; CCP's
own reader was not disassembled). Falsifier, still: the client does not boot,
or an import error names one of the eight members. Then `revert` restores the
backup (31 MB beside `code.ccp`), and `--tier A,P,B` leaves C out. `C1`
requires `B0`. Because C is now selected by default, an unknown `code.ccp` blocks
every run, as an unknown `blue.dll` always did. Builds whose manifest has no
tier C entry (3619879, 3628513) are unaffected: there is nothing to apply.

What C does not touch: `{}.servers.eveonline.com`, `serverInfo`,
`XmppChatMgr`, the `EVE_PUBLIC_GATEWAY_ADDRESS` branch of `get_address`,
`cryptoPack`. The gateway edit (C8) changes only `get_discovery_address` and
`discover_address`, which run when the env is absent; the discovery name
becomes `<tier>-public-gateway.gateway.invalid`, which never resolves.

How a file is patched. Nothing is located at apply time. A target file is
`stock`, `patched` or `unknown`, decided by its whole-file sha256. Every
write command refuses when any target is `unknown`, so a different build, a
half-patched file or a file Steam replaced is never touched. A file's
recipes are written into `<file>.eveoffline-tmp`, the result is checked
against `sha256_patched`, and then `os.replace` swaps it in. So every file is
either stock or patched, never half done. The first write leaves
`<file>.eveoffline-orig` beside the file. This is a byte-for-byte backup,
trusted only while it hashes to stock. `apply` on a patched install is a
no-op. `revert` restores from the backups and keeps them. For each
prefs.ini, `apply` records the values it replaced in
`prefs.ini.eveoffline-prior.json`. `revert` puts those values back, unless
you changed a key after `apply`; your value then wins.

`apply` computes every patched file in memory first, so a patch that cannot
be made (for example a tier C member that does not fit its slot on this
host's zlib) refuses with exit 2 before a single file, backup or pref is
written. Only then are files written, each one whole. A write error midway
(a locked file on Windows, a full disk) stops with exit 1 and a message;
`apply` again finishes, `revert` undoes. `verify` prints `WARN` for any
unknown target whose tier is not selected, since it was not checked (only a
run with `--tier A,P,B` skips `code.ccp`). `revert` exits 1 and names an unknown
`code.ccp` it left alone, rather than claim every target is stock.

**Where prefs.ini is found.** Under Wine: `drive_c/users/*/AppData/Local/CCP/EVE/c_<install
path>_*/settings*/prefs.ini`. On Windows (an install on a drive such as
`C:\CCP\EVE\tq`): `%LOCALAPPDATA%\CCP\EVE\c_ccp_eve_tq_*\settings*\prefs.ini`,
the drive letter in front of the same lowercased path. `--prefs PATH` always
wins. The Windows rule is a unit test and has not met a real Windows install.

The patcher refuses in these cases:

- an install named `tq` (the live one), unless you pass `--allow-real`;
- an install inside this repository;
- a build with no manifest;
- a backup that does not hash to stock;
- B1, B2, B3, C1 or P0 when B0 is neither patched nor selected (`requires`);
- a tier C member that is not the manifest's stock, or whose patched form
  does not fit its slot.

`verify` also fails when prefs.ini holds `networkLogging`, `http` or
`GOPYCHARM`, or when a `resfileindex*.txt` row is missing from `ResFiles` or
has the wrong size. A missing row means the client downloads it from the
CDN.

## The launch layer

Both launch paths end in `tools/jail-client.sh`. Once the PATCH-1
coordinator diff lands, that script runs `eve_patch.py launch-env` as you.
It takes only the `env` lines whose key is in its fixed allow-list, adds any
missing `arg` lines, and refuses the launch (exit 3) when the build has no
manifest or the arguments hold a forbidden one. The output format is one
item per line:

    env HTTP_PROXY=http://127.0.0.1:9
    arg /noCrashReportUpload
    forbid-arg /refreshToken

`--json` prints the same content as JSON. `WINEDLLOVERRIDES=winebrowser.exe=d`
stays a constant in `jail-client.sh` and is not in the manifest.

## Checking it worked: the DNS census

The patch is accepted only after a live run, and the complete one is
`tools/census.sh` (`docs/guides/census.md`): a control copy, the patched
subject, and a no-env subject that proves tier C by itself. The older, plain
version: run a `tools/client-ci.sh` scenario in the jail, then count the
names in `logs/eve-dns.log`. After
patching, none of these may appear:

    sentry.io  api.ipify.org  *.launchdarkly.com  *-public-gateway.evetech.net
    clientresources.eveonline.com  www.google-analytics.com

The only names that should remain are `tranquility.servers.eveonline.com`
(the stub we answer) and `*.lndarkly.invalid` (B2 working: the renamed hosts
are still looked up and never resolve). For B1, force one crash in the jail.
The log must show no `sentry.io` lookup from `eve_crashmon.exe`, and the
Crashpad database must hold a pending report.

## A new build

1. Create the manifest by hand from the research. Copy
   `tools/patches/<old>.json` to `tools/patches/<new>.json`, then set
   `build`, `identify.start_ini_build`, and for every file `size`,
   `sha256_stock`, `sha256_patched`, the site offsets and the window
   hashes. A window is the bytes `[offset - window, offset + len + window)`.
2. Check it against the install without writing anything:

       tools/analysis.sh python3 -I tools/eve_patch_author.py check /client

   For every file this confirms three things: the stock bytes (or their
   backup) hash to `sha256_stock`; the patcher's own transform gives
   `sha256_patched`; and every window moves from stock to patched.
3. `python3 -I tools/test_eve_patch.py` lints every manifest. The lint
   rejects a `before` key and any replacement longer than 64 bytes. It also
   checks hash formats, offsets inside the file, and that `requires` is
   acyclic. When the pin is for the same build, the stock hashes must equal
   the ones in `tools/client-pin.sha256`.
4. Run the live DNS census above.

`tools/new-build.sh` prints this `locate --merge` line at its patch step and
runs `hosts` right after the manifest exists, stopping on a new host.

Finding the recipes in a new build's files:

       tools/analysis.sh python3 -I tools/eve_patch_author.py locate /client \\
           --merge tools/patches/<old>.json > tools/patches/<new>.json

`locate` reads each target's stock bytes (its `.eveoffline-orig` backup only
when that hashes to this build's manifest stock; a stale backup from another
build is ignored with a warning), finds C1, B1, B2
and B3 structurally and prints a manifest with every hash taken by applying
the patcher's own transform. A rule whose hit count is not exactly the
expected one refuses, and prints nothing: C1 by the anchor table
`CCP_RULES` (a string constant per member), B1 by the Crashpad URL and its
`mov r8d, len`, B2 by the six LaunchDarkly hosts, B3 by the function that
names `slInit` (the one `mov [rbp-d], 0x49` followed by `or [rbp-d], 0x84`;
it needs capstone, which the analysis image has). B0 and P0 stay with
`tools/client-patch-manifest.py`. Read the diff against the old manifest
before you trust it, and stop for review if `monolithsentry`,
`autoexec_client_core`, `connectionConfig` or `remotefilecache` changed
shape.

The alarm for a new outbound path:

       tools/analysis.sh python3 -I tools/eve_patch_author.py hosts /client

lists every host-like string in the client (all of `code.ccp`'s members and
every file under `bin64`, ASCII and UTF-16; URLs of any scheme and TLD,
including a Sentry-style `https://key@host/id`, and bare names under a list of
TLDs; a member that cannot be read shows up as `unparsed-member:<name>`) and exits 1 when one is not in
`out/census/known-hosts.txt`. That list lives outside the repository; write
the first one with `--write-known`, read it, and keep it. A new build that
brings a new host fails here before any client runs.

## What stays the jail's job

The patch does not cover the following, so the jail still has to:

- server code push;
- a launch without our env: ipify, analytics, gateway discovery, the CDN
  constants and the Sentry DSN are closed by tier C once it is on, and the
  no-env census run (`docs/guides/census.md`) is what shows it;
- URLs the server hands the client as data (photo, minibrowser, image and
  subtitle fetches, exoplanet data): there is no constant to change;
- Authenticode revocation lookups, WPAD from blue's resource stream, and
  Streamline's updater on a GPU other than NVIDIA's;
- Crashpad, if B1's [I] turns out wrong;
- links the user clicks;
- Tracy, RAD, Wwise and WPAD;
- IP literals we never found;
- a new build's new endpoints;
- Wine itself.

R-PATCH §6 has the table.

## Legal

Each person patches their own lawfully obtained copy so it can work with a
private server. The repository holds only sha256 hashes, offsets, lengths,
our own replacement bytes and hostnames. Backups stay on that person's
machine. CCP's EULA forbids modifying the client; under O11 that risk is the
owner's to accept.
