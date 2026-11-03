# Bench ramp: how many bots in one system before the server bends

`tools/bench-ramp.sh` starts its **own** release `eve-server` (a `--dev` world
copied from `var/bench/base`, port 26500, chat 26501, no client, no `--bubble`,
so nothing sheds) and one `eve-bots`, then ramps bots into one system: N0 at
the start, K more every H seconds, up to NMAX. It never touches `var/dev` or
port 26000, so it is safe beside a live session, but at full scale it loads the
machine: run the big ramps on a quiet one.

```sh
tools/bench-ramp.sh --prep --max 300                 # once: warm the accounts and kits
tools/bench-ramp.sh                                  # 50 -> 300, +50 every 75 s, one grid
tools/bench-ramp.sh --start 10 --add 10 --max 30 --hold 60     # the small proof (3 min)
tools/bench-ramp.sh --grid spread --mix miners=100             # miners in fives, a belt each
tools/bench-ramp.sh --stagger-ms 0 --start 100 --add 100       # a login burst per step
```

Options are in `--help`: `--start --add --max --hold --stagger-ms --grid --mix
--system --port --run --no-build --prep`.

## What it does

- **Accounts**: made once, on `var/bench/base/world.dat` (the existing
  `tools/bot-accounts.sh`; passwords in `~/.config/eve-offline/bench-bots.tsv`,
  mode 600), then reused by every run. Names are `bm<G>x<N>` (miners),
  `br..` (ratters), `bf..` (fighters). A new run copies `base` to
  `var/bench/<run>/`, so every run starts alike.
- **`--prep`**: all NMAX bots log in at once on `base` and buy and fit their
  kits from the dev market, then it stops (at most 10 minutes; ~2 min for 30).
  Without it a ramp still runs, but its first minutes measure kit shopping
  (a roster bot is docked and buying for about its first minute).
- **Logins** use `eve-bots --ramp N0,K,H,GAP_MS`, not the human 3-13 s stagger:
  batch 0 is N0 bots, batch b is K bots, each `GAP_MS` apart (default 50 ms,
  20 a second; 0 = the whole batch in one burst, which is itself a test).
  The roster is interleaved in proportion to the group sizes, so every step
  adds the same mix.
- **Scenario**: `--grid same` puts the miners in one fleet (a group follows its
  boss to his belt; a fleet holds at most 256, so groups split at 250) and the
  ratters (`sites`) and fighters (`hunter`, Rifters) are one group each.
  `--grid spread` makes the miners fleets of five, each boss picking a belt of
  his own among the five nearest. `--mix` weights the three kinds (default
  `miners=100`; see the warning below). `--system` is the home (default Amamake).
  **[A]** that "same grid" really is one grid: fleets follow their boss, but
  nothing here reads the grid sets; the proof run's `in_space` shows the bots are
  out, Falsifier: the server's
  `[live]` line (pilots per system) fine but the `destiny_frames_per_tick`
  well under pilots x (pilots - 1) / pilots-per-grid.

**Warning, found by the proof**: in the dev world a roster Rifter (fighters) or
Vexor (ratters) cannot buy its ammo or guns (`MISSING: 1000 of type ..: asks
at station .. (cap 15.84 ..)`: the dev market's 1000-lot does not fit the hold),
so those bots sit docked for ever and add connections, not grid load. That is
why the default mix is all miners; `--mix miners=60,ratters=25,fighters=15`
is there for when the kit is fixed. Check `bots.log` for `MISSING`.

## What it records

Per step (one hold), `var/bench/<run>/steps.tsv` (one row per step) and
`summary.txt`; the raw `server.log` and `bots.log` stay beside them.

| column | source |
| --- | --- |
| `pilots`, `in_space` | the last `[live]` line of the step |
| `work_p95_ms`, `work_max_ms`, `held_*`, `wait_max_ms`, `late_max_ms`, `over_budget`, `skipped` | the server's `[world] tick health` lines (one per 10 s window of 40 ticks). `work_p95` is the p95 over **ticks** (the server prints it now); `held_p95` is the p95 over windows of each window's worst hold |
| `srv_*`, `bots_*` cpu% (of one core), RSS MB, threads | /proc, every 2 s; RSS and threads are the step's peak |
| `out_kBps` | everything posted to connection outboxes (`Writers::post`) |
| `destiny_kBps`, `destiny_frames_per_tick`, `destiny_bytes_per_bot_s` | the `DoDestinyUpdate` pushes, per recipient |
| `act_failed`, `act_refused`, `usererrors` | the bots' act lines (`FAILED`, `refused`) and the server's UserErrors |

The server's tick-health line has three new trailing fields for this
(`work p95`, `out N frames B bytes`, `destiny N frames B bytes`; counters are
relaxed atomics, reset at each report). `tools/client-ci.sh` and
`tools/soak_summary.py` read the line's front and are unaffected.

**PA-10 coalescing** (`--coalesce-destiny`, default off) adds ` parts P merged M`
right after the destiny bytes: `P` is the mergeable Destiny pushes posted to
outboxes and `M` how many of them were appended to a part already queued, so
`M / P` is the reduction. With the flag on, `destiny N frames` counts the frames
the **writers** encode (a merged entry is one), not one per push per client; with
it off the clock counts them as before. Compare the two with
`tools/bench-ramp.sh --grid same` at 50, 262 and 500, flag off and on; in
process, `cargo test --release -p eve-server coalesce_bench -- --ignored --nocapture`
prints both side by side (ticker encode, writer encode, frames/s).

**The PA-0 profile lines** (lane pa0-prof, `live_status::prof`) follow the
tick-health line every window, in the same `[world] tick ...` style (measurement
only; `Instant` pairs and relaxed atomics):

| line | what |
| --- | --- |
| `tick sections` | per clock tick, the mean and max (ms) of `take_due` (scan, stamp, restamp), `filter` (`grid::filter`), `audience` (audience to client ids), `session_changes`, and `save` (the clock's `to_snapshot`; `n` is the saves) |
| `tick lock` | the world lock's occupancy over the window: the clock's hold and the payload turns' hold, each in ms and percent, payload wait, and the total. **From PA-12a the payload numbers are the EXCLUSIVE occupancy** (write-lock turns); turns served under `world.read()` overlap each other, so they are reported apart at the end of the line (`read turns N held X ms (shared, not in occupancy) ...`) and an older series is not comparable without `EVE_READ_TURNS=0` |
| `tick methods` | payload turns per `service.method`: top 10 by total hold and by count as `name:count:hold:max:ro|ro?|rw`: `ro` = served under the read lock at least once (PA-12a), `ro?` = a **guess** from the name (`Get`/`List`/`Is`/`Has`/`Find`/...), with the share of turns and of hold the guess covers; the line ends `ro-served turns X% hold Y%, fallbacks N` (read-eligible calls the write path served because output was queued). `EVE_READ_TURNS=0` turns read turns off for an A/B run |
| `tick writers` | the push-ticker: frames and bytes written, pass time, deflate (calls that compressed), and the longest single `write_all` |

`steps.tsv` gains the matching columns (`take_due_ms` ... `longest_write_ms`),
and `tools/soak_summary.py` prints a four-line digest.

**Not measured**: the bots' own lag (server stamp to the bot applying it): the
bots keep no such clock. Proxy: `act_failed` and `act_refused`, and bots
disconnecting (pilots falling). Balls on the grid are not counted either:
read `pilots`, and for the rest `npcs`/`rocks` in the `[live]` line.

## The knee and the stop

- **Knee** (`summary.txt`): the first step where `held_p95` or `work_p95`
  passes 50% of the 250 ms tick (125 ms), or any tick went over budget or
  was skipped.
- **Stop early**: 3 consecutive windows with ticks over budget, 20+ skipped
  ticks in a step, or pilots falling by more than 10% from the step before.
  The server then stops through its stop file (world saved) and the summary is
  written.

## How long

Steps cost `--hold` each plus the server start (about 5 s) and the stop (10-20 s).
50 -> 300 in steps of 50 is six steps: **6 x 75 s = 7.5 min, about 8 min** with
the defaults, plus `--prep` once (about 5-10 min for 300 bots, the accounts
once, minutes more). Hold of 60 s is the minimum worth having (six windows).
The proof (10 -> 30, hold 60) took 3 min.

## Where it should break (read from the code, [A] until a big run)

1. **The world write lock, per tick.** `clock_turn` runs every stage and the
   drain under one write lock. `grid::filter` runs per push, per audience
   member, so a system-wide push (a ship's Destiny update goes to every pilot
   on its grid) costs O(n) lookups, and n ships each moving means O(n^2) per
   tick on one grid. `grid::reconcile_in` runs every 4th tick (1 s) and is
   O(viewers x local balls) with a `BTreeSet` lookup each, plus a scene
   rebuild when anybody gains a ball: expect `held_max` and the
   `tick stages` line's `grid` stage to grow roughly with n^2 and to spike
   once a second. Look at `[world] tick stages` first.
2. **Frame bytes**: `destiny_frames_per_tick` x pilots; `out_kBps` per bot
   grows linearly with grid size, so total output is O(n^2). Every connection
   has a writer thread and an outbox (4096 frames / 16 MiB cap); a bot that
   reads slowly overflows it and is disconnected (a `[push] ... outbox over
   cap` line, and pilots falling).
3. **The bots' side**: one OS thread per bot (see `threads`) plus the park
   integration per bot: each bot applies every Destiny frame to its own park
   copy, so bots CPU is O(n) per bot, O(n^2) overall. The 13 MB per bot RSS is
   mostly its park and the content tables (shared).
4. **Not a limit here**: saves (every 5 s, render under the lock, but linear in
   world size), chat, the 166-thread count (one reader + writer per client).

If the columns are not enough, instrument next: per-stage times are already in
`[world] tick stages`; add a `grid` sub-timer for `filter` vs `reconcile_in`;
count balls per park (`Parks`) and the largest audience per push; time the bots'
apply of one `DoDestinyUpdate` (driver) for the lag.

## Server flags and the connection cap

`EVE_BENCH_SERVER_ARGS="--tick-threads 1" tools/bench-ramp.sh ...` passes extra
flags to `eve-server` (and the summary line names them). The bench listener
takes `cap=max(2048, 2 x --max)` live connections: the loopback default (128)
refused every bot past it. The one-shot sign-up server in
`tools/bot-accounts.sh` takes 4096 for the same reason. The PA-3 `slice_max`
field of the tick-health line becomes the `slice_max_ms` and `slice_p95_ms`
columns (0 for a server without it).

## Profiling the bots process (BOT-OPT)

`EVE_BENCH_BOT_ARGS="--stats 20 --quiet" tools/bench-ramp.sh ...` passes extra
flags to `eve-bots`. `--stats SECS` prints a `[stats]` line (process CPU, RSS,
threads, bytes in, and the CPU all bots spent decoding, applying destiny,
refreshing, deciding, absorbing, logging); `--quiet` prints no act lines. `eve-bots`
restarts itself once with `MALLOC_ARENA_MAX=2` (glibc's per-thread arenas
doubled the RSS of 40 bots); set the variable yourself to choose another value.
