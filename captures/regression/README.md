# Kept captures

A capture is the **only artefact in this project that cannot be
regenerated.** It is a recording of a real client, taken on a machine with a
real installation; no amount of reading bytecode reproduces one. Everything
else here — the server, the docs, the findings — could be rebuilt from the
client and enough time.

The whole `captures/` directory was ignored, and so the archive was
untracked, unbacked-up, and present on exactly one disk. These four are
tracked so that is no longer true, and so a replay test has stable input.

Read them with:

```sh
tools/dev.sh cargo run -q -p eve-server --bin evecap -- /repo/captures/regression/FILE --triage
tools/dev.sh cargo run -q -p eve-server --bin evecap -- /repo/captures/regression/FILE --calls
tools/dev.sh cargo run -q -p eve-server --bin evecap -- /repo/captures/regression/FILE --alerts
```

| file | why it is kept |
| --- | --- |
| `1792428778-0002` | The richest docked session: the player opened many windows, so it holds the widest spread of station-side calls — 21 client tracebacks, which was the high-water mark. |
| `1792430889-0002` | The window-opening run that produced the character-sheet, contracts and map worklist. Also the run whose `--calls` output showed `slash.SlashCmd("/")` as a hang, which was a bug in our *tool* and not in the server. |
| `1792431326-0002` | **Holds a bug we have since fixed**, and that is the point of keeping it: `michelle.py(453) AddBallpark` → `TypeError: must be tuple, not list`, from `beyonce.GetFormations` answering an empty list where native `blue.dll` demanded an empty tuple. The player saw a black screen. A recording of the failure is worth more than the memory of it. |
| `1792437742-0002` | A session that reached **space**: the undock chain complete, `GetFormations() -> ()`, the ballpark bound, and the whole in-space call sequence. The reference for what the client asks for once undocked. |

**Do not prune these to save space.** 584 KB buys the only regression evidence
that comes from outside our own assumptions: `tests/loopback.rs` proves the
pieces agree with each other, and a capture proves they agree with CCP's
client.

New captures still land in `captures/` and stay ignored. Promote one here
when it records something the set above does not.
