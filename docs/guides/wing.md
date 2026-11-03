# The wing: bots that fly with you (owner cheat-sheet)

Status: FC-0 to FC-3 are in. Commands marked **(not yet)** parse and are answered, but do
nothing until their lane lands (FC-4 sov, FC-5 doctrine).

## Before you start

- Run the dev world with `eve-bots --control` (the `/bots` link) and be a GM.
- Form a fleet in the client. For sov work your corp must be in an alliance
  (Corporation > Alliances > Create); `/bots ... wing` warns when it is not.

## Raising the wing

| You type | What you get |
|---|---|
| `/bots 6 wing` | DPS that join your fleet (Stabbers up to 5, Hurricanes beyond) and follow you. |
| `/bots join` | The bots look for your fleet now (and your fleet is listed with no approval). You can also invite them yourself. |
| `/bots 10 sovfleet` | The DPS share of a sov fleet (60 %: 6 of 10) as a wing. Add `/bots 2 logi` yourself; the linkers and hub hauler of the mix are **(not yet)**; the reply says so. |
| `/bots 2 logi` | Repairers (`winglogi`, FC-2): they join your fleet and rep you and the wing. |
| `/bots 2 linkers` | **(not yet)**: Entosis pilots. |
| `/bots hub hauler` | **(not yet)**: a freighter with the hub, fuel and Strontium. |

## Moving (client buttons)

- Fly: they follow you anyway, through gates, into docks and out again.
- Warp Fleet To / Warp To: they warp with you. Jump To on a gate: they go ahead and jump.
  Travel To: they fly there. Align To: they align. Hold Position: they stop.

## Fighting (client)

- Broadcast **Target**: the primary. Fleet **target tags**: the order of fire.
- **Heal Armor/Shield**: the logi reps you (`winglogi`).

## Words

Say them in **fleet chat** (only your own lines count, and only when the whole line is the command:
"hold the door" is talk) or type `/bots <words>`. `/bots order <group> <words>` targets one group.
The wing's **lead** (the lowest character id still posting) answers once with `copy: ...`.

| Words | What happens |
|---|---|
| `follow` / `follow me` / `on me` / `form up` / `come` / `regroup` | Anchor on you. Cancels hold, dock, go home, the stance and a primary. |
| `hold` / `stay` / `stop` / `hold position` | Hold where they are (10 minutes, or until `follow`). Guns and repairs go on. |
| `dock` / `dock up` | Dock: in your station if you are docked, else at home. They stay docked until `follow`. |
| `go home` / `rtb` | Back to staging and stay there until `follow`. |
| `rearm` / `reship` / `resupply` | Dock, resupply, come back. |
| `weapons free` | Stance Free: shoot hostile pilots near you without waiting (the fight reads it: FC-2). |
| `weapons hold` / `hold fire` | Stance Hold: do not open fire. |
| `defend` | Stance Defend (the default). |
| `primary <name>` / `focus <name>` | Shoot that target when a ship with that name is on grid (60 s). NPC names only for now; ship types and pilots come with FC-2. |
| `status` / `x` / `report` | One report line from the lead: how many on grid, docked, elsewhere; the stance; where you are. |
| `entosis` / `entosis hub` / `entosis node` / `link` | **(not yet)** linkers link it. |
| `stop entosis` / `unlink` | **(not yet)** linkers stop. |
| `drop hub` / `launch hub` | **(not yet)** the hauler launches the hub at the sun. |
| `fuel hub` / `fuel` | **(not yet)** the hauler fuels it. |

Sov words are answered `not yet: ...` so you know they were heard.

## Admin (unchanged)

`/bots list`, `inspect <bot>`, `pause|resume <group>`, `hold <group>` (pause, with a name),
`despawn <group|all>`, `bring [group]` (docked), `come [group]` (they come to you, even in space).
A bare `/bots hold` is the wing's hold.

## How to read a failure

- `no wing follows you: /bots 6 wing first`: no bot has a wing seat for you (spawn one).
- A bot that does not obey: `/bots inspect <bot>` shows its last acts; it obeys only while in your
  fleet.
- `copy:` never appears: the lead posts only from inside a fleet; with chat off nobody speaks.
