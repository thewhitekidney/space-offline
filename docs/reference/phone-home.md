# How the client phones home, and what calls home when

**Build described:** 3662168 (the stand-in for the 2026-11-09 freeze). 3619879
has the same paths except where a row says otherwise (P9's host differs, for
one; the P-rows were not re-read in 3619879 beyond that). Source: lane
`r-phone-home` (issue #40), `findings/outbound-endpoints.txt`, and the patcher
(`docs/guides/patcher.md`). Hostnames are listed here on purpose; no client
bytes or extracted data are.

**Marks.** [V] read in this build's code or bytes. [I] inferred. [A]
assumption, with its falsifier in the row or in `docs/lanes/phone-home.md`.

**"Closed by"** names the patcher tier (`A` launch env and args, `P` prefs,
`B` byte edits, `C` constants in `code.ccp`) or "open". **Open** means only the
jail (`docs/guides/jail.md`) stops it today. **Status:** tier C and B3 are
written, reviewed once and fixed (#40). Tier C is in the default tiers since the
re-pin to 3662168 (`docs/guides/patcher.md`, "Tier C").

**Re-derive it for a new build:**
`tools/analysis.sh python3 -I tools/eve_patch_author.py hosts /client` lists
every host-like string in `code.ccp` and `bin64` and exits 1 on one that is
not in the reviewed `out/census/known-hosts.txt`. Then read the new host's
caller, add a row here, and add a patch or a census scenario for it.
`tools/census.sh` shows which paths actually fire.

## Native (`bin64`)

| id | path | host or mechanism | trigger | data | default | closed by | mark |
|---|---|---|---|---|---|---|---|
| N1 | Crashpad upload (`eve_crashmon.exe`, WinHTTP) | `sentry.io` minidump URL in `exefile.exe` | a crash, or a hang (`frameTimeTimeout` 30 s) | minidump, crash keys (tags, user, geo, IP), session, start args minus tokens, CPU/RAM/OS, PDM data | yes | B1, `/noCrashReportUpload`, `breakpadUpload=0` | site [V], effect [I] until a forced crash |
| N3 | LaunchDarkly native SDK | six `*.launchdarkly.com` hosts | first flag read | hostname, user id, tenant, build, later character id and language | yes | B2 | [V] |
| N4 | Gateway gRPC (Go) | `{tier}-public-gateway.evetech.net`, or `EVE_PUBLIC_GATEWAY_ADDRESS` | service start | see "To our server only" | yes | env (A) to our server; C8 turns the discovery name into `gateway.invalid` | [V] |
| N5 | `BlueRemoteStream` resource CDN (libcurl in `blue.dll`) | `clientresources.eveonline.com` (primary, and the backup, which is the same host); `res.eveprobe.ccpgames.com` is the `prepare()` default | a `res:/` file missing from `ResFiles` | the requested paths | no (0 rows missing) | primary: `/resfileserver` (A); the constants: C2 (`autoexec_client_core`) and C3 (`remotefilecache`: the backup and the probe default) | [V] |
| N6 | WPAD from N5 | `GetAutoProxyUrl`: IE config, then `WinHttpGetProxyForUrl`, result becomes `CURLOPT_PROXY` | the first curl transfer, if the IE config says autodetect | `wpad.*` lookups | no (needs a miss) | open | site [V], Wine default [I] |
| N7 | Tracy | `TracyClient.dll`, UDP broadcast and listen | profiler API only | profiler data | no | `/tools=`, `/enableTelemetry` are forbidden args | gate [V] |
| N8 | RAD Telemetry | `rad_tm_win64.dll` | `/enableTelemetry` | profile | no | forbidden arg | [V] |
| N9 | Wwise comms and WAAPI | `_audio2.dll` sockets, an embedded HTTP server | no Python caller found | audio debug | no | none needed | [V] no caller |
| N10 | GELF log sink | `blue.EnableNetworkLogging`, prefs `networkLogging` | the prefs key | the whole client log | no | `verify` forbids the key | [V] |
| N11 | Browser hand-off | `blue.os.ShellExecute` and `webbrowser` (about 40 callers: chat links, store, survey, help, petition), then `winebrowser`, then `xdg-open` | a user click | the URL, including the `GetUserToken` value on store and survey URLs | no | open (jail: `WINEDLLOVERRIDES=winebrowser.exe=d`) | callers [V], chain [I] |
| N12 | NVIDIA Streamline OTA | `sl.interposer.dll` runs `nvngx_update.exe` and loads downloaded plugins | `slInit` in `_trinity_dx12.dll`, NVIDIA GPU, DX12 | hardware and driver ids to NVIDIA [I] | flags on (0xCD) | B3 | site [V], flag names [I] |
| N13 | Authenticode revocation | `crl3/crl4/ocsp.digicert.com`, `crl.comodoca.com`, `ocsp.usertrust.com`, an entrust AIA | load of a signed DLL | a CRL GET | [A] no; falsifier: such a name in the census DNS log | open | [I] |

Not paths [V]: no CEF, Chromium, WebView, Steamworks or overlay DLL in
`bin64`; `sl.*`, `nvngx_*`, `libxess`, FidelityFX and Aftermath import no
socket API; `_videoplayer.dll` has none either.

## Python (`code.ccp`)

| id | path | host | module | trigger | data | default | closed by | mark |
|---|---|---|---|---|---|---|---|---|
| P1 | Sentry events | `sentry.io` | `monolithsentry` (`requests`) | every logged error (1 per minute), `local_error_spike_report` | stack with locals, session, user, IP and geo, language | yes | env DSN (A), prefs DSN (P), proxy (A), C1 | [V] |
| P2 | Public-IP lookup | `api.ipify.org` | `monolithipify`, and `store_payment_svc` on a purchase | every start | IP, user agent | yes | proxy (A), C4 (the purchase caller reads the same constant) | [V] |
| P3 | Google Analytics | `www.google-analytics.com/collect` | `characterCreationMetrics` | character creation | step metrics | yes, in creation | proxy (A), C5 | [V] |
| P4 | Honeycomb, beeline | `api.honeycomb.io` | `monolithhoneycomb*`, `libhoney` | needs a write key from the server's globalConfig | session ids, machine, country | no (we send no key) | our server sends none | [V] |
| P5 | Datadog, statsd, Prometheus | `app.datadoghq.com` | `monolithmetrics` | never on a client (`on_client()` returns first) | none | no | code | [V] |
| P6 | VictoriaMetrics | `vm-single.dev.evetech.net` | `victoriametrics`, `statscapture`, a new node-graph atom (728) | a node graph with that atom [I: QA; shipped graphs not scanned] | metric floats, machine name | no | proxy (A), C7 | sink [V] |
| P7 | Image server | `imageserver.eveonline.com` | `evePhotosvc` | the server sends such a URL | ids | no | we send none | [V] |
| P8 | URL textures | any | `photo.GetTextureFromURL` (`requests`): omega offer, VGS, `urlSprite`, subcellular images | a server-provided URL | a fetch | only if our data has a URL | proxy (A); otherwise open | [V] |
| P8b | Rich-text `<img>`, CSS, page | any, including `ftp://` | `browserImage`, `browserutil.GetStringFromURL` (`urllib2`, UA `CCP-minibrowser/3.0`), `edit.py`, `parserbase.py` | rendered HTML with a remote URL | a fetch, which shows the viewer to the URL's host | [I] reach from player text unchecked | proxy (A) for http(s); `ftp://` goes past it and is open | sink [V] |
| P8c | `ImageDownloader` | any https | `imagecaching` (store catalogue) | a catalogue URL from the server | ETag fetch | no | proxy (A) | [V] |
| P9 | Bug reports | `ebr-service.ccpeveweb.com/v1` in 3662168 (3619879 has `bugsservice.eveonline.com/api`; override in both: globalConfig `bugReporting_BugReportServer`) | `bugReporting`, `bugreporter` | the Help window, Report Bug, Send | `Authorization: Bearer` access token, description, session, computer info, attachments (screenshots, logs, settings, `prefs.ini`, crash list, dxdiag, PDM data) | on a user action | proxy (A), C6 | [V] |
| P10 | SSO refresh | `login.eveonline.com`, or the token's `iss` | `clientRefreshToken` | `/refreshToken=` | the token | no | forbidden arg | [V] |
| P11, P12 | Vault, FSD ids | `vault.evetech.net`, `fsdid.evetech.net` | `monolithconfig.secrets`, `fsd.idGenerator` | dev tools | n/a | no | proxy (A) | [I] |
| P13 | Video index | any | `videoplayer.index` (`urllib2`) | an index URL | a fetch | no shipped default found | proxy (A) | sink [V] |
| P13b | Subtitles | the URL given | `shared/subtitles.py` | a non-`res:` path | a fetch | no | proxy (A) | [V] |
| P16 | NetEase | `*.evepc.163.com`, `ma79.gdl.netease.com`, kiring | region code | `boot.region == 'optic'` | | no | region | [V] |
| P17 | Patch URLs | `client.eveonline.com` | constants only | never | | no | n/a | [V] |
| P18 | Inbound HTTP | listens | `Startup` | prefs `http` | | no | `verify` forbids the key | [V] |
| P19 | Exoplanets, subcellular | MMOS URLs, a CloudFront name | `projectdiscovery` | Project Discovery opened | task fetch | on an action | proxy (A) | sink [V] |
| P20 | QA uploads | `trilambda-ws:8088` | `statscapture` | `eveclientqatools` | machine name, stats | no | proxy (A) | [V] |

Dead in this build [V]: `evebannerads` (`request_ad` has no caller, the URL is
`https://www.invalid.url`), `vgs/client/banners`, the ESI getter.

URLs that arrive as **data** (P7, P8, P8b, P8c, P13, P13b, P19) have no
constant to change: only the proxy (and, today, the jail) stops them.

## Wine and Proton

| id | path | trigger | closed by | mark |
|---|---|---|---|---|
| W1 | gecko and mono downloads on a new prefix | a prefix is created | open; our prefix script makes the prefix once | [I] |
| W2 | the resolver and reverse lookups | any name lookup | open (jail: `eve-dns`) | [V] |
| W3 | Proton's NGX updater | `PROTON_ENABLE_NGX_UPDATER`, off by default | open | [I] from memory; falsifier: Proton's `proton` script |

## To our server only, by design

None of this leaves the machine that runs the server, which is a friend's.
It is not a leak, but friends should know it exists.

| channel | what the client sends | our handling |
|---|---|---|
| macho `monolithTaskTrackerMgr.UpdateClientAchievmentsAndCounters` | achievement counters (`client_openMap` and so on) | answered `None`, dropped; the client service's `IsEnabled` is False, so it may never send [I] |
| macho `eventLog.LogClientStats`, `LogPlayerRequestedDisconnect`, `LogClientEvent` | frame times, memory and resfile counters, disconnect stage, a crash-info tuple (GPU vendor and device, driver, OS), fast-checkout funnel | answered and dropped |
| macho `alert.SendClientStackTraceAlert`, `BeanDelivery` | client stack traces with arguments, session ids | answered; `evecap` reads them |
| macho `operationsManager`, `seasonManager.process_client_event` | tutorial and season events | game data |
| macho handshake | login token, build, language | login |
| gateway `publish_event_payload` (about 38 modules) | `UserInformationGathered` (PDM hardware data, device id, all graphics settings), `ConnectionLost` (user, reason, country, ASN, IP if known), UI window, tab, button and video analytics, map, agency, job board, omega upsell | goes to `EVE_PUBLIC_GATEWAY_ADDRESS` only, which is us |
| gateway requests | store, PLEX vault, local chat | game |
| XMPP 5222 | chat, plain with `chatserver_disable_tls=1` | ours |
| `GetUserToken` value | put into store and survey URLs | issued by us, worthless elsewhere; N11 would carry it |

`ConnectionLost.ip_address` stays empty only if the public-IP lookup (P2) is
dead, so closing P2 also keeps a friend's address out of these events.

**Most sensitive if left open:** N1 (a minidump), P1 (stack locals), P9
(`prefs.ini`, logs, the bearer token), N3 (hostname, ids), P2 (the public
IP).

## What is still open

N6, N11, N13, W1-W3 and everything delivered as data (P7, P8, P8b incl.
`ftp://`, P8c, P13, P13b, P19). Until the jail is dropped that is the jail's
job; after it, each needs its own recipe or a decision (the `r-no-jail`
research). `docs/lanes/phone-home.md` lists the open [A] and [I] marks.
