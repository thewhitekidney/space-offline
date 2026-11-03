# GM commands

GM slash commands are typed in local chat by a GM character. Make a character's
account a GM with `tools/account.sh grant <NAME> gm` while the server is stopped
(in game, `/grant <username> gm` does the same for another account).

The list below is generated from the `COMMANDS` table in
`server/crates/eve-server/src/slash.rs`, which is the one source: a command that
is not in that table answers "not implemented yet". The reply is printed in the
client's console, so each command says what it did. To refresh this table, read
the `usage` strings in that file; a new command adds one `COMMANDS` row there.

| command | usage |
|---|---|
| `/bots` | `/bots <count> [allied\|hostile] <kind> [in <system>] \| list \| despawn <group\|all> \| bring [group] \| pause\|resume <group\|bot> \| inspect <bot> \| stop-all \| help` |
| `/giveisk` | `/giveisk [character] <amount>` |
| `/pushtest` | `/pushtest [seconds]` |
| `/tp` | `/tp [km]` |
| `/whoami` | `/whoami` |
| `/giveskill` | `/giveskill <skill\|typeID> [level]` |
| `/removeskill` | `/removeskill <skill\|typeID> [level]` |
| `/givestarterskills` | `/givestarterskills` |
| `/createitem` | `/createitem <type\|id> [quantity]` |
| `/setstanding` | `/setstanding <fromID> <me\|characterID> <value>` |
| `/entity` | `/entity deploy <qty> <typeID>` |
| `/settle` | `/settle [park <career> \| <game hours> [name] \| sweep]` |
| `/whjump` | `/whjump <class 1-6 \| systemID \| fx \| fxmod>` |
| `/whmass` | `/whmass <percent 1-100>` |
| `/sigresolve` | `/sigresolve` |
| `/abyss` | `/abyss enter <tier 0-6> \| leave \| status` |
| `/escalate` | `/escalate [dungeonID]` |
| `/live` | `/live [systemID]` |
| `/rollover` | `/rollover [now]` |
| `/essdeposit` | `/essdeposit <isk>` |
| `/econ` | `/econ [regionID \| report [regionID] [hours] \| tsv [hours] \| price-tape [regionID] [days] \| audit [days] \| price <typeID> \| history [days] \| closed [days] \| burn on\|off]` |
| `/grant` | `/grant <username> <player\|gm>` |
| `/sov` | `/sov set <system\|here> <alliance\|none> \| /sov clear <system\|here> \| /sov capture <system\|here> <alliance> \| /sov info [system\|here]` |
| `/sovfuel` | `/sovfuel [status [system\|here]] \| /sovfuel <blocks\|all> \| /sovfuel burn <days>` |
| `/givelp` | `/givelp <corpID> <amount>` |
| `/secstatus` | `/secstatus [characterID] <value>` |
| `/flag` | `/flag suspect\|criminal [characterID]` |
| `/dock-capitals` | `/dock-capitals [on\|off]` |
| `/jumpfatigue` | `/jumpfatigue 0 [characterID]` |
| `/devskills` | `/devskills all5 \| ids <id,id,..> \| hull <typeID>` |
| `/devcorp` | `/devcorp <ticker>` |
| `/tr` | `/tr me <stationID\|systemID\|celestialID\|shipID\|characterID> \| /tr me pos=x,y,z` |
| `/devminecrit` | `/devminecrit crit \| waste \| both \| off` |
| `/devhome` | `/devhome <system name\|id>` |
| `/upwell` | `/upwell spawn <astrahus\|raitaru\|athanor\|typeID> <name> \| /upwell list` |
| `/shutdown` | `/shutdown [seconds] [message] \| /shutdown cancel \| /shutdown status` |

Server flags (`--concord`, `--with-bots` and the rest) are parsed in
`server/crates/eve-server/src/bin/eve-server.rs`; the setup and run commands are
in `docs/guides/dev-mode.md` and `docs/guides/contributing.md`.
