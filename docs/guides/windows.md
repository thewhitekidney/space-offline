# The Windows client

[getting-started.md](getting-started.md) runs the client on Linux, in the jail,
through Proton. This page is another shape: **one Windows PC**. The server runs
in WSL2, and the **native Windows client** talks to it over `127.0.0.1`. It is
what the README's "a similar setup on Windows is wanted" asks for.

What it gives you: sign-up and login, undock, in-game chat, and the AIR
new-player experience from character creation through scene 2.4. Those were
verified on build 3707965 (V24.02). A fresh-PC run took sign-up, the station,
undock and chat on a later build of the same line (see the last section).
What it does **not** give you is a network jail. The Linux path has one; a
native Windows client has no namespace to put it in. Read
[No jail on Windows](#no-jail-on-windows) before you point the client at a
server you do not own.

Paths on the WSL side are relative to the repository root in your WSL home.
Paths on the Windows side are yours. The EVE launcher installs to
`C:\Fenris Creations\EVE\tq` by default, so this guide writes the offline copy
as `C:\Fenris Creations\EVE\tq-offline`. That path has a space. Quote it
everywhere, on Windows and in WSL (`"/mnt/c/Fenris Creations/EVE/tq-offline"`).
Every flag and `EVE_*` variable is in
[launch-and-env.md](../reference/launch-and-env.md).

## 1. Prerequisites

On a stock Windows 11 PC, in this order:

- **WSL2 with Ubuntu.** A stock Windows 11 install has no WSL. `wsl --status`
  says "not installed". In an administrator PowerShell run `wsl --install`,
  then **reboot**, then create the Ubuntu user when it asks. Use Ubuntu 24.04
  or newer ([getting-started.md](getting-started.md#supported-platforms)).
  A fresh-PC run hit three things:
  - Before the reboot, `wsl --status` says virtualization is not enabled.
    That is the pending reboot, not a firmware setting.
  - A distro installed before the reboot may not be registered after it.
    Check `wsl -l -v`. If it lists none, run `wsl --install -d Ubuntu` again.
  - `wsl --install -d Ubuntu --no-launch` never asks for a user, so the
    distro's default user stays `root`. Add one (`useradd -m -s /bin/bash <you>`),
    set it in `/etc/wsl.conf` (`[user]`, then `default=<you>`), and run
    `wsl --shutdown`.
- **The Linux toolchain, inside WSL.** The server, the build and the content
  extraction run in WSL as [getting-started.md](getting-started.md)
  describes. Nothing in the server changes for Windows. Inside Ubuntu you
  need `git`, `python3` and Docker. **Docker Engine in the distro** is what
  this guide was run with (`/usr/bin/docker`, the `docker.io` or Docker CE
  packages). Docker Desktop's WSL integration is not verified here. On Ubuntu
  26.04, `apt-get install docker.io docker-buildx git python3` was enough, and
  systemd started the engine. Clone the repository into your WSL home.
- **Python 3.11 or newer on Windows.** The patcher and the launcher run on
  Windows with `py -3 -I`, standard library only. They run from the WSL clone
  through its Windows path, for example in PowerShell:

  ```powershell
  Set-Location \\wsl.localhost\Ubuntu\home\<you>\space-offline
  ```

  A clone on the Windows side works as well; the fresh-PC run used one.
- **A Windows EVE install, at an accepted build.** Get the launcher from
  `https://www.eveonline.com/download`. Its installer is signed by **Fenris
  Creations hf.**, not "CCP ehf". That is expected. Install EVE with it and let
  it download every resource file. The build must be the `pinned` row of
  `tools/builds.tsv`, or one the registry marks `equivalent` or `provisional`
  ([build-compat.md](build-compat.md)). It must also have a patcher manifest,
  `tools/patches/<build>.json`. Check the install's build on Windows:

  ```
  type "C:\Fenris Creations\EVE\tq\start.ini" | findstr /b build
  ```

  and the pinned row, from the repository root in WSL:

  ```sh
  awk -F'\t' '$3=="pinned"{print "pinned: " $1}' tools/builds.tsv
  ```

  The launcher always installs the current Tranquility build, and Tranquility
  patches often. On 20 Nov 2026 a fresh install was already one build past the
  pinned row. If your build is not accepted, stop here. The content step, the
  patcher and the server refuse it until the registry catches up
  ([tracking-tq.md](tracking-tq.md)).

**How the two sides reach each other.** WSL2 forwards loopback one way.
Windows reaches a listener on WSL's `127.0.0.1` through `localhostForwarding`,
which is on by default. All three ports this guide uses arrive that way: a
fresh-PC run reached 26000, 5222 and 8443 from Windows with
`Test-NetConnection`. The other direction is different. WSL reaches Windows'
`127.0.0.1` only with `networkingMode=mirrored` in `.wslconfig`. This guide
does not need that direction, and nothing here was run in mirrored mode.

## 2. Build the server in WSL

```sh
# in WSL, at the repository root
tools/dev.sh --image
EVE_MAX_JOBS=4 tools/dev.sh cargo build --locked --release --workspace
```

WSL2 gets about half the PC's memory, and `tools/dev.sh` waits for about 12 GB
free. On a 16 GB PC, lower the parallelism with `EVE_MAX_JOBS`. The fresh-PC
run built the release in 3.5 minutes with 4 jobs in a 7 GB WSL VM. Raising
`.wslconfig` `memory=` instead starves Windows once the client runs.

## 3. Make the offline client copy

As on Linux ([jail.md](jail.md#we-run-a-copy-never-the-install-you-play-on)),
run a **copy**, never the install you play on. The copy gets patched, and the
launcher can revalidate and revert a live install under you.

With EVE closed, copy the install directory *beside its ResFiles*, so the
client's `..\ResFiles` still resolves:

```
robocopy "C:\Fenris Creations\EVE\tq" "C:\Fenris Creations\EVE\tq-offline" /E
```

`ResFiles` stays where it is, shared by both installs. It was 10 GB right
after a fresh install, and it is not worth duplicating. Keep `tq-offline` and
`ResFiles` under one parent so `tq-offline\..\ResFiles` points at it.

## 4. Prepare the copy with the patcher

The copy has to be patched before it can play on this server. Without that,
the server drops the connection right after the handshake. The patcher is
`tools/eve_patch.py`, the same one the Linux path uses
([patcher.md](patcher.md) says what it changes and why). From the clone, in
PowerShell:

```powershell
py -3 -I tools\eve_patch.py plan   --client "C:\Fenris Creations\EVE\tq-offline"   # dry run
py -3 -I tools\eve_patch.py apply  --client "C:\Fenris Creations\EVE\tq-offline"
py -3 -I tools\eve_patch.py verify --client "C:\Fenris Creations\EVE\tq-offline"
```

`apply` does three things:

- It sets `cryptoPack = Placebo` in `start.ini`, with the `blue.dll` fix that
  lets the client accept an edited file.
- It closes the phone-home paths in the copy's files: the Crashpad upload,
  LaunchDarkly, NVIDIA Streamline's updater, and the constants in `code.ccp`.
- It sets three keys in the client's `prefs.ini`: `chatserver_disable_tls=1`
  (chat over plain TCP), `breakpadUpload=0` and `sentry_io_dsn`. A key that is
  already there is replaced, never added twice.

It refuses an install named `tq` (the live one) and a build it has no
manifest for. `revert` undoes it.

**Where prefs.ini is.** The client keeps it under `%LOCALAPPDATA%\CCP\EVE`, in
a folder named after the install path and the server. The client lower-cases
the path, turns `\`, `/` and spaces into `_`, drops the `:`, and appends `_`
and the server address [V utillib.GetClientUniqueFolderName, build 3724492;
the folders on two PCs match it]. For the default path and this server:

```
%LOCALAPPDATA%\CCP\EVE\c_fenris_creations_eve_tq-offline_127.0.0.1\settings\prefs.ini
```

The patcher finds the install's folders as `<folder>_*\settings*\prefs.ini`.
The client writes its folder on its first launch, so before that there is
nothing to find. `apply` then creates `<folder>_tranquility`, the name the
Linux path uses, and a client launched with `/server:127.0.0.1` never reads
it. The launcher in step 6 hands the patcher the right folder before every
launch, so you need do nothing more. If you launch by hand instead, pass that
path to `apply` with `--prefs`:

```powershell
py -3 -I tools\eve_patch.py apply --client "C:\Fenris Creations\EVE\tq-offline" `
  --prefs "$env:LOCALAPPDATA\CCP\EVE\c_fenris_creations_eve_tq-offline_127.0.0.1\settings\prefs.ini"
```

**The hand route, `common.ini`** (optional; [V] on builds 3707965 and
3724492, see the last section). Before the patcher was public, this page put
`cryptoPack = Placebo` in a new `common.ini` at the install root:

```ini
[main]
cryptoPack = Placebo
```

The client reads `common.ini` over `start.ini`, and `manifest.dat` does not
cover it, so it needed no `blue.dll` change. A stock `start.ini` with that
file logged in, undocked and chatted on both builds. It closes no phone-home
path, though, and the launcher in step 6 refuses a copy that `verify` does not
pass. The launcher also refuses a `common.ini` whose `cryptoPack` is not
`Placebo`, because it would win over the patcher's `start.ini`.

## 5. Build the content bundle and run the server

The content bundle reads the client copy, so it comes after step 3. Point
the content tools at the copy from WSL with `EVE_CLIENT`. The quotes matter;
`tools/analysis.sh` handles the space.

```sh
# in WSL, at the repository root
export EVE_CLIENT="/mnt/c/Fenris Creations/EVE/tq-offline"
tools/analysis.sh --build
tools/analysis.sh python3 -I /work/tools/content.py --client /client \
    --resfiles /client-root/ResFiles --out /work/out/content
```

Then run the server **headless**: no jail and no Linux client. Leave it
running. A dev world is the quickest:

```sh
tools/dev-world.sh none        # dev server on 127.0.0.1:26000, no bots
```

`tools/dev-world.sh` needs the content bundle. Without it, it stops with "run
tools/serve.sh or tools/up.sh once to extract it". Those are the Linux jail
scripts, and they use sudo. On Windows the content step above is what it
wants.

Three things matter for the Windows client:

- **The gateway (port 8443) must be on.** Without it the client's
  achievements, goals and licences fail silently. `tools/dev-world.sh` turns
  it on for the main world (it adds `--gateway` on port 26000). If you start
  `eve-offline-server` another way, pass `--gateway` yourself
  ([launch-and-env.md](../reference/launch-and-env.md)).
- **It binds `127.0.0.1`.** The listeners are `26000` (game), `5222` (chat)
  and `8443` (gateway), which WSL2 forwards to Windows. A listener a new
  player signs up against needs `signup=on` ([dev-mode.md](dev-mode.md),
  [hosting.md](hosting.md)). `tools/dev-world.sh` sets it.
- **A build that is not `pinned`.** A `provisional` build is served on the bundle
  of a `pinned` or `equivalent` build of its line
  ([build-compat.md](build-compat.md#provisional-builds)).
  An `unverified` build needs its own bundle and `--client-build <N>` on the
  server ([content-bundle.md](content-bundle.md)). `--client-build` takes
  registered builds only: an unregistered one exits with "not in the build
  registry". `tools/dev-world.sh` does not pass it.

## 6. Launch the client

`tools/windows_launch.py` launches the native client. It uses only the
standard library. From the clone, in PowerShell:

```powershell
py -3 -I tools\windows_launch.py --root "C:\Fenris Creations\EVE\tq-offline" --token me:mypass --dry-run
py -3 -I tools\windows_launch.py --root "C:\Fenris Creations\EVE\tq-offline" --token me:mypass
```

It writes no client file itself. Everything goes through the patcher:

1. `eve_patch.py launch-env --client ROOT --server S --json` gives the launch
   layer. That is the environment (`HTTP_PROXY` and `HTTPS_PROXY` at a dead
   loopback port, `NO_PROXY` for the server, `EVE_PUBLIC_GATEWAY_ADDRESS`,
   `EVE_CLIENT_SENTRY_DSN`), the arguments the client must get
   (`/noCrashReportUpload`, `/resfileserver=http://127.0.0.1:9/`) and the ones
   it must never get (`/tools`, `/py`, `/refreshToken`, `/enableTelemetry`,
   `/thinclient`). The launcher checks its own arguments against that last
   list. The Linux launch reads the same layer.
2. `eve_patch.py apply --tier P` sets the `prefs.ini` keys, in every folder of
   this install and in the one the client uses for this server.
3. `eve_patch.py verify --no-resfiles` must pass. An unpatched copy is not
   launched.
4. It runs `bin64\exefile.exe /server:S /port:N /ssoToken=<user>:<password>`
   with the layer's arguments, its environment, and the working directory at
   the install root, so `..\ResFiles` resolves.

A step that fails stops the launch with the patcher's exit code. `--dry-run`
asks for the launch layer and prints what it would run, with the password
masked. It writes nothing and launches nothing. It still fails when `--root`
holds no `bin64\exefile.exe`, or when the build has no manifest, so a wrong
setup shows up before launch day.

| Flag | Meaning |
|---|---|
| `--root DIR` | the offline install root (holds `bin64\exefile.exe`) |
| `--token user:pass` | the sso token; a `signup=on` listener makes the account |
| `--server HOST` | server address (default `127.0.0.1`; also names the prefs folder) |
| `--port N` | server port (default `26000`) |
| `--gateway host:port` | `EVE_PUBLIC_GATEWAY_ADDRESS` (default `<server>:8443`, from the launch layer) |
| `--prefs PATH` | a `prefs.ini` for the patcher, instead of the ones found (repeatable) |
| `--tier T` | the tiers `verify` checks (default the patcher's, `A,P,B,C`) |
| `--allow-real` | allow an install named `tq`, the one you play on |
| `--dry-run` | write nothing, launch nothing, print what would run |

**The sign-up token.** Against a listener with `signup=on`, a token for an
unknown user **creates** the account and a first character. The fresh-PC run
went from there to character selection. Against a listener without it, the
user must already exist ([hosting.md](hosting.md), [dev-mode.md](dev-mode.md)).

**By hand.** To launch without the launcher, print the launch layer and set
it yourself:

```powershell
py -3 -I tools\eve_patch.py launch-env --client "C:\Fenris Creations\EVE\tq-offline" --server 127.0.0.1
```

Every `env` line must be set, every `arg` line passed, and no `forbid-arg`
ever. Without `EVE_PUBLIC_GATEWAY_ADDRESS` the client dials CCP's gateway, and
the goals, achievements and licence calls go nowhere
([phone-home.md](../reference/phone-home.md), N4).

| Symptom | Fix |
|---|---|
| "The connection to the server was closed" right after login | the copy is not patched; run step 4 |
| "Could not connect to the chat server at 127.0.0.1:5222" | the client's `prefs.ini` lacks `chatserver_disable_tls=1`; the server log says so. Launch with the launcher, or `apply --prefs` the folder from step 4 |
| `no manifest for build N` | the install's build has no patcher manifest; see step 1 |
| `looks like the install you play on` | `--root` names `tq`; launch the copy |

## No jail on Windows

The Linux client runs in a network namespace with **no route to anything but
this server** ([jail.md](jail.md)). There is no equivalent here. A native
Windows client runs with your full network access. Two facts from the Linux
docs carry over, and they matter more without the jail:

- **The server can run code in the client.** Under `Placebo` a server, or
  anyone who can rewrite its traffic, can hand the client Python that it runs
  as you ([jail.md](jail.md#the-server-can-run-code-in-the-client)). This is
  true on every platform. On Linux the jail bounds what that code can reach on
  the network. On Windows nothing does. **Only connect to a server whose
  owner you trust, and never over a raw internet path.** The traffic is
  plaintext.
- **The client phones home.** The patcher and the launch layer close the
  paths [patcher.md](patcher.md) lists. The rest of
  [phone-home.md](../reference/phone-home.md) is still open: URLs the server
  hands the client, links you click, and new endpoints in a new build. On
  Linux the jail catches those. Here nothing does.

### The optional firewall rule

A host firewall rule gives back part of the jail's egress control. An
**outbound block** on the client binaries stops them reaching the internet.
In an elevated PowerShell:

```powershell
New-NetFirewallRule -DisplayName "eve-offline: client no egress" -Direction Outbound `
  -Program "C:\Fenris Creations\EVE\tq-offline\bin64\exefile.exe" -Action Block -Profile Any
New-NetFirewallRule -DisplayName "eve-offline: crashmon no egress" -Direction Outbound `
  -Program "C:\Fenris Creations\EVE\tq-offline\bin64\eve_crashmon.exe" -Action Block -Profile Any
```

Remove them with `Remove-NetFirewallRule -DisplayName "eve-offline: *"`.

[A] Windows Firewall does not filter loopback, so with both rules the client
still reaches the server on `127.0.0.1`. Falsifier: with the rules in place,
the client cannot log in, or chat cannot connect. Then loopback is filtered,
and the rules have to go. Neither half of this rule has been run end to end;
the fresh-PC run did not reach it. In mirrored mode WSL also sits behind the
Hyper-V firewall, which this guide has not met.

What this rule is, and what it is not:

- It blocks the client's **own** outbound sockets. A local server still
  works, and a phone-home does not.
- `eve_crashmon.exe` is the Crashpad handler, a separate process, so it needs
  its own rule ([phone-home.md](../reference/phone-home.md), N1).
- It is **weaker than the namespace jail**. It is a host firewall rule, not
  isolation. Anything that runs as you, including code the server runs in the
  client, can add, remove or route around it, and it does not touch other
  processes. It is a backstop against the client's routine phone-homes, not a
  boundary a hostile server cannot cross.

The launcher does not create this rule. It needs admin, and it is a policy
choice, so you apply it by hand.

## The display mode is saved, and it can save the wrong one

Run the client fullscreen at your monitor's native resolution. The client
keeps its fullscreen mode in
`%LOCALAPPDATA%\CCP\EVE\<settings folder>\settings\core_public__.yaml`, as
`device: FullScreenSettings: [<stamp>, {adapter, height, presentInterval, width}]`.
It saves whatever size its window last had. Two things have made it save a
smaller one: a tool resizing the game window, and the displays sleeping and
waking while the client ran (it came back minimized at 1280x720). Either way
the monitor switches to the smaller mode at the next launch, and the UI is
squeezed to it.

The fix, with the client closed: back the file up, set `width` and `height`
in that entry back to the native resolution, and relaunch. Window positions
laid out at the small size stay wrong until **Esc > Reset Settings > Reset to
Default Window Positions**. A longer display-sleep timeout
(Settings > System > Power) keeps the second cause away during long sessions.

## What is verified, and what is not

**Build 3707965 (V24.02), on the PC this guide was written on:** sign-up and
login, undock, in-game chat, and the AIR new-player experience through scene
2.4, with the server in WSL2 and the native client over loopback. That run
used the `common.ini` route. The rest of act 2 needs the AIR NPE work still in
progress (#205).

**A fresh-PC run, 19 to 23 Nov 2026:** a Windows 11 Home laptop (16 GB) with
no WSL, git, Docker or EVE install, following this guide's earlier version.
Steps 1 to 3 and the server build ran as written; the gaps it found are folded
in above. The fresh EVE install was one build past the pinned row (3724492),
so the content step stopped as it should. With a local-only registry entry for
that build, never committed, it went on to sign-up, character selection, the
station, undock, and chat. It used the `common.ini` route and the earlier
launcher, which then wrote the chat line into the right folder.

**The patcher on Windows, 24 Nov 2026:** on a Windows 11 PC with build
3707965, Windows Python ran `tools/eve_patch.py` from the WSL clone through
`\\wsl.localhost`. `status` found the client's own folder
(`<folder>_127.0.0.1\settings\prefs.ini`). On a scratch copy of the patched
files, `apply`, `verify` and `revert` all passed, and `apply` took 3.4 s. On a
copy the client had never run, `plan` would create `<folder>_tranquility`,
which is why the launcher passes the folder itself. On the same scratch copy,
with only the final launch stubbed out, the launcher ran `launch-env`, created
`<folder>_127.0.0.1\settings\prefs.ini` through `apply --tier P`, passed
`verify`, and built the command and environment step 6 lists. Its tests pass
under Windows Python as well as in the Linux gate.

**Not verified yet:**

- [A] A copy patched by `eve_patch.py apply` starts and plays on native
  Windows as the `common.ini` copy did. The patched bytes are the ones the
  Linux path boots under Proton. Falsifier: the patched copy does not start,
  or the server drops it after the handshake. Then `revert`, and use the
  `common.ini` route until it is fixed.
- [A] A `prefs.ini` the patcher creates before the client's first launch is
  read and kept by the client. Falsifier: after the first launch the file has
  lost `chatserver_disable_tls=1`, or chat fails.
- The firewall rule (above), and a real client started by this version of the
  launcher.
