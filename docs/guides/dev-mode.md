# Dev mode

A debugging world where nothing waits for training, building or income. For
the owner watching bots and testing the client; **not** a real world, and it
never mixes with one.

## Start it

```sh
tools/dev-world.sh                                   # 50 bots (content/bots/dev/world50.tsv)
tools/dev-world.sh world50 --home "Some System" --client
tools/dev-world.sh pvp --home "A LowSec System"      # two 10-ship fleets, opposite sides
tools/dev-world.sh mining | pve | path/to/roster.tsv
```

`--home` takes a system **name** (or id); every roster row without its own
system is seated there. `--client` brings up the jail and your client (as
`tools/serve.sh --client` does); log in with the account from
`tools/account.sh login` (the listener signs up new names). Needs
`out/content` (extracted once by `tools/serve.sh` or `tools/up.sh`). Ctrl-C
stops the bots, then the server writes the world.

## What `eve-server --dev` changes

- **A loud banner**, and a separate world: `out/dev/world.dat` by default.
  The world is marked by `world.dat.dev`. `--dev` refuses an existing world
  without the mark (somebody's normal save); a normal run refuses a marked
  world. `--grant` and `--mark-bot` are offline tools and ignore the mark.
- **A dev seller in every station**: for every published market type
  (`types/market.tsv`) the station shows an ask of **10,000,000 units at 100 ISK**.
  It is synthesised when a book is read (`GetOrders`, the three ask lookups,
  the bots' reads); nothing is stored. A buy goes through the ordinary buy
  path (wallet, journal, the goods created in the buyer's hangar), and the
  stock is 10,000,000 again. Real player orders show beside it. It leaves **no
  trade**: the price history, the price index, the econ tape and the bots'
  price memory never see the 100 ISK.
- **ISK**: a new character starts with 1,000,000,000; an existing character in
  a dev world is topped up to that once (a journal row, and a `devtopup` delta
  row records it).
- **Dev-only slash commands** (each answers "dev only" on a normal server):
  `/devskills all5 | ids <id,..> | hull <typeID>` (skills and their
  prerequisites at V), `/devcorp <ticker>` (join or found a corporation, no
  fee), `/devhome <system name|id>` (dock the caller at a station of that system at
  once, from a station or from space, with no reconnect: the same teleport as
  `/tr`).

## Rosters

`eve-bots --roster FILE [--roster-home SYSTEM]` (the launcher passes both).
TSV, `#` comments, columns `group side role count skills ship [system]`:

| column | meaning |
| --- | --- |
| group | letters and digits; its bots are `<group>1..N`; the first is the boss |
| side | groups on one side share one corporation (friendly); other sides are other corporations, so `society::hostile_to` counts them hostile and the `hunter` role fights them |
| role | a `--loop` role: `mine foreman escort hunter sites missions trader builder hack ice pi huff ...` |
| count | bots in the group |
| skills | `all5` (every skill at V), `role` (the role's plans, the hull's skills and their prerequisites, at V), or a plan name (`core miner foreman ice gas builder reactor hauler explorer ratter hunter pi`) |
| ship | a type **name** resolved from the bundle at run time, or `-` for the role's own hull |
| system | a system name or id, else `--roster-home` |

Each roster bot, once a connection and docked: `/devhome` (one reconnect),
`/devskills`, `/devcorp`, then the closed economy's own order
(`acquire::run_order`): the hull, its modules and rounds, bought from the dev
market. No GM item is created. The group's boss then forms a fleet and invites
the group as it logs in (a `foreman` forms its own fleet at the belt and
`mine` bots follow it, as those roles already do).

Shipped: `content/bots/dev/mining.tsv` (two Orca foremen + five Hulks each),
`pvp.tsv` (ten Rifters against ten Merlins), `pve.tsv` (sites and missions),
`world50.tsv` (50 bots: miners, hunters on two sides, sites, missions,
traders, builders, hackers, ice, PI, gas).

## Limits

The hunter and hostile roles fight only where the law does not reach (low
sec, null sec, wormholes), so a PvP roster wants a system there. A role that
chooses its own hull (the miner's rank ladder) may board a better one than
the roster's if it holds one. Tests: `tests/bots/dev_mode.rs`, the
`a_dev_*` and `a_normal_server_*` tests in `service/market.rs`, `start.rs`,
`dev_mode.rs`; client scenario `fci_dev_mode`.
