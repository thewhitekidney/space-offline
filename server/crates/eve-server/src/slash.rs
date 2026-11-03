//! The slash console's commands: one row per command in [`COMMANDS`].
//!
//! W10.5 seam S1 (`docs/content/plan.md` §6.1). The arms were one `match` in
//! `Registry::slash` in `service.rs`, so every lane adding a GM command
//! edited the hottest file in the crate. Each arm is now a function here,
//! moved verbatim -- `self` became the `reg` argument and nothing else -- and
//! [`Registry::slash`] is a lookup in [`COMMANDS`]. A new command is a
//! function in its own module plus one `COMMANDS` row, which the coordinator
//! adds at landing (the table is coordinator-only, like `DOMAINS`).
//!
//! Three arms touched private `Registry` fields, which a sibling module
//! cannot: `dirty` now goes through `mark_dirty`, the journal through
//! `journal_mut` and the store through `spawn_items`, each the same write.
//!
//! [`Registry::SLASH_COMMANDS`] stays where it is: it is the autocompletion
//! list the client is told, in its own order, and listing a name authorises
//! nothing.

use crate::pyutil::bstr;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use std::time::Duration;

/// A command's body: the world, the caller, and the words after the name.
pub type Run = fn(&mut Registry, Option<i64>, &[&str]) -> Result<PyValue, ServiceError>;

/// One slash command: what the player types, and what runs.
#[derive(Clone, Copy)]
pub struct Command {
    /// The word after the `/`, lowercase, as the client sends it.
    pub name: &'static str,
    /// The grammar, for a help line. Nothing reads it yet.
    pub usage: &'static str,
    /// The command itself.
    pub run: Run,
}

/// Every slash command the server answers, in the order the old `match`
/// tried them. **Coordinator-only**: a packet exports its function and the
/// coordinator adds the row.
pub const COMMANDS: &[Command] = &[
    // BOT-CTL: runtime bot control over the eve-bots link (dev servers).
    Command {
        name: "bots",
        usage: "/bots <count> [allied|hostile] <kind> [in <system>] | list | despawn <group|all> | bring [group] | pause|resume <group|bot> | inspect <bot> | stop-all | help",
        run: crate::bot_control::slash,
    },
    Command {
        name: "giveisk",
        usage: "/giveisk [character] <amount>",
        run: giveisk,
    },
    Command {
        name: "pushtest",
        usage: "/pushtest [seconds]",
        run: pushtest,
    },
    Command {
        name: "tp",
        usage: "/tp [km]",
        run: tp,
    },
    Command {
        name: "whoami",
        usage: "/whoami",
        run: whoami,
    },
    Command {
        name: "giveskill",
        usage: "/giveskill <skill|typeID> [level]",
        run: giveskill,
    },
    Command {
        name: "removeskill",
        usage: "/removeskill <skill|typeID> [level]",
        run: removeskill,
    },
    Command {
        name: "givestarterskills",
        usage: "/givestarterskills",
        run: givestarterskills,
    },
    Command {
        name: "createitem",
        usage: "/createitem <type|id> [quantity]",
        run: createitem,
    },
    Command {
        name: "setstanding",
        usage: "/setstanding <fromID> <me|characterID> <value>",
        run: setstanding,
    },
    Command {
        name: "entity",
        usage: "/entity deploy <qty> <typeID>",
        run: entity,
    },
    // E1: the offline agents (lazy settlement of logged-off careers).
    Command {
        name: "settle",
        usage: "/settle [park <career> | <game hours> [name] | sweep]",
        run: crate::econ_agents::wire::settle_cmd,
    },
    Command {
        name: "whjump",
        usage: "/whjump <class 1-6 | systemID | fx | fxmod>",
        run: crate::wormhole_sites::slash,
    },
    Command {
        name: "whmass",
        usage: "/whmass <percent 1-100>",
        run: crate::wormhole_jump::slash,
    },
    Command {
        name: "sigresolve",
        usage: "/sigresolve",
        run: crate::signal_tracker::slash,
    },
    Command {
        name: "abyss",
        usage: "/abyss enter <tier 0-6> | leave | status",
        run: crate::abyss::slash,
    },
    Command {
        name: "escalate",
        usage: "/escalate [dungeonID]",
        run: crate::escalation::slash,
    },
    Command {
        name: "live",
        usage: "/live [systemID]",
        run: crate::service::gm_live::live,
    },
    Command {
        name: "rollover",
        usage: "/rollover [now]",
        run: crate::service::gm_live::rollover,
    },
    Command {
        name: "essdeposit",
        usage: "/essdeposit <isk>",
        run: crate::service::gm_live::essdeposit,
    },
    Command {
        name: "econ",
        usage: "/econ [regionID | report [regionID] [hours] | tsv [hours] | price-tape [regionID] [days] | audit [days] | price <typeID> | history [days] | closed [days] | burn on|off]",
        run: crate::service::gm_econ::econ,
    },
    Command {
        name: "grant",
        usage: "/grant <username> <player|gm>",
        run: crate::account_book::grant,
    },
    Command {
        name: "sov",
        usage: "/sov set <system|here> <alliance|none> | /sov clear <system|here> | /sov capture <system|here> <alliance> | /sov info [system|here]",
        run: crate::service::sov::slash,
    },
    Command {
        name: "sovfuel",
        usage: "/sovfuel [status [system|here]] | /sovfuel <blocks|all> | /sovfuel burn <days>",
        run: crate::service::sov_hub::sovfuel,
    },
    Command {
        name: "givelp",
        usage: "/givelp <corpID> <amount>",
        run: crate::loyalty::slash,
    },
    Command {
        name: "secstatus",
        usage: "/secstatus [characterID] <value>",
        run: crate::sec_status::slash,
    },
    Command {
        name: "flag",
        usage: "/flag suspect|criminal [characterID]",
        run: crate::sentry::slash,
    },
    Command {
        name: "dock-capitals",
        usage: "/dock-capitals [on|off]",
        run: crate::dock_policy::slash,
    },
    Command {
        name: "jumpfatigue",
        usage: "/jumpfatigue 0 [characterID]",
        run: crate::jump_fatigue::slash,
    },
    // DEV-MODE: refused unless the server runs --dev.
    Command {
        name: "devskills",
        usage: "/devskills all5 | ids <id,id,..> | hull <typeID>",
        run: crate::dev_mode::skills,
    },
    Command {
        name: "devcorp",
        usage: "/devcorp <ticker>",
        run: crate::dev_mode::corp,
    },
    Command {
        name: "tr",
        usage: "/tr me <stationID|systemID|celestialID|shipID|characterID> | /tr me pos=x,y,z",
        run: crate::service::relocate::tr,
    },
    Command {
        name: "devminecrit",
        usage: "/devminecrit crit | waste | both | off",
        run: crate::mining::slash,
    },
    Command {
        name: "devhome",
        usage: "/devhome <system name|id>",
        run: crate::dev_mode::home,
    },
    // U2-UPWELL: a finished Upwell structure at the caller's ship.
    Command {
        name: "upwell",
        usage: "/upwell spawn <astrahus|raitaru|athanor|typeID> <name> | /upwell list",
        run: crate::service::structures::slash,
    },
    Command {
        name: "shutdown",
        usage: "/shutdown [seconds] [message] | /shutdown cancel | /shutdown status",
        run: crate::shutdown::slash,
    },
];

impl Registry {
    /// Run one slash command, and say something useful back.
    ///
    /// The client does not parse the result: `_SlashCmd` stringifies whatever
    /// we return (`'slash result: %s' % res`) and prints it in the console.
    /// So the return value is the **only** channel we have to the player
    /// here, and it is worth writing for them. There is no role check to
    /// perform either -- the client's dispatcher has none, and gating in the
    /// real server happens at the call sites.
    ///
    /// The command list in [`Self::SLASH_COMMANDS`] drives autocompletion
    /// only; it authorises nothing, and a command missing from it still runs.
    pub(crate) fn slash(
        &mut self,
        actor: Option<i64>,
        line: &str,
    ) -> Result<PyValue, ServiceError> {
        let mut words = line.trim_start_matches('/').split_whitespace();
        let Some(cmd) = words.next() else {
            return Ok(bstr("empty command"));
        };
        let args: Vec<&str> = words.collect();
        match crate::module::plan()
            .commands
            .iter()
            .find(|c| c.name == cmd)
        {
            Some(c) => (c.run)(self, actor, &args),
            None => Ok(bstr(&format!("not implemented yet: {line}"))),
        }
    }
}

// **The one GM grant that needs no persistence layer.**
//
// `account.GetCashBalance` already reads the actor's
// `Character::balance`, which is live server state, so moving
// that number is the whole implementation. The `/giveisk
// <character> <amount>` form still ignores the name and credits
// the actor -- named here because with two characters that is a
// visible limitation rather than a harmless simplification. It is
// worth doing ahead of the rest of the GM commands precisely
// because it is the one that does not need an item store, a type
// table or a write-ahead log -- the others really are blocked on
// `docs/reference/world-state.md`, and this one never was.
//
// Two honesty constraints, both stated to the player rather than
// hidden:
//
// - **It does not survive a restart.** Nothing does yet. Saying
//   so in the console is better than letting someone grant
//   themselves a trillion ISK and discover it tomorrow.
// - **The wallet does not refresh by itself.** A real server
//   sends `OnAccountChange`, and we have no server-initiated
//   push path at all -- `conn::serve` only ever writes in
//   response to a read. So the new balance is live on the wire
//   the instant this returns, and invisible until something asks
//   again. Reopening the wallet asks again.
//
// The amount is parsed from the **last** word, which accepts both
// `/giveisk 5000000` and the `/giveisk <character> 5000000` form
// the real command takes. There is only one character to give to,
// so a name is accepted and ignored rather than rejected: this is
// a console in a sandbox, and refusing a command over an argument
// that could not have meant anything else is just friction.
fn giveisk(reg: &mut Registry, actor: Option<i64>, args: &[&str]) -> Result<PyValue, ServiceError> {
    let Some(last) = args.last() else {
        return Ok(bstr("usage: /giveisk [character] <amount>"));
    };
    // Tolerate thousands separators and a trailing "isk", both of
    // which a player typing a large number will produce.
    let cleaned: String = last
        .trim_end_matches("isk")
        .trim_end_matches("ISK")
        .chars()
        .filter(|c| *c != ',' && *c != '_')
        .collect();
    let Ok(amount) = cleaned.parse::<f64>() else {
        return Ok(bstr(&format!("not a number: {last}")));
    };
    if !amount.is_finite() {
        return Ok(bstr("amount must be finite"));
    }
    let Some(actor) = actor else {
        return Ok(bstr("no character to credit"));
    };
    let Some(c) = reg.character_mut(actor) else {
        return Ok(bstr("no character to credit"));
    };
    c.balance += amount;
    let (now, owner_id) = (c.balance, c.character_id);
    reg.mark_dirty();
    // **And the journal, which is what tracker #1 actually asked
    // for.** `entryTypeID` 3 is `refGMCashTransfer`, which the
    // client's own `accountingentrytypes` table and two
    // independent readings of its localisation corpus agree
    // resolves to "GM Cash Transfer" -- so the Type column reads
    // as something a player can understand rather than blank.
    //
    // `ownerID1` is the system rather than a character: a GM
    // grant has no counterparty. `balance` is the balance *after*
    // the grant, so the journal reads as a running total, which
    // is how the window presents it.
    //
    // `crate::pyutil::filetime_now()` and not zero. A year-1601 date takes down
    // the whole Journal tab -- see `ledger`'s module docs -- and
    // there is no honest reason to write an entry with no date.
    reg.journal_mut().record(
        crate::pyutil::filetime_now(),
        crate::ledger::REF_GM_CASH_TRANSFER,
        1,
        owner_id,
        0,
        amount,
        now,
        "",
    );
    // **The grant is visible without reopening anything now**, and
    // this is the first thing the server ever tells the client
    // unprompted. `WalletSvc.OnAccountChange` updates its own
    // `_wealth` from the pushed balance, blinks the neocom and
    // scatters `OnPersonalAccountChangedClient` with the
    // difference, so the wallet does not even re-ask us.
    //
    // The apology this replaced said "reopen the wallet to see
    // it", which was wrong twice over: `GetCashBalance` is called
    // once in `WalletSvc.Run` and cached, so reopening does not
    // re-ask -- and a relog builds a fresh `Registry`, so the
    // grant was unobservable by *any* route.
    reg.push(crate::notify::AccountChange {
        owner_id,
        balance: now,
    });
    Ok(bstr(&format!("balance {now:.2} ISK ({amount:+.2}).")))
}

// **The one-line answer to "is the push path alive".**
//
// Queues the balance the player already has, `n` seconds from now,
// and answers the console immediately. The only difference from
// `/giveisk` is *which thread writes the frame and when*: this one
// leaves on the push ticker, with the client having sent us nothing
// at all in between -- which is the whole point, because the client
// is entitled to be silent for four minutes at a stretch and the
// keepalive is 240 s.
//
// The balance is deliberately unchanged, so a wallet that reacts
// proves the path and not the arithmetic: the neocom blinks and
// `OnPersonalAccountChangedClient` scatters a difference of zero.
//
// It is also the only command here that is not a real EVE one, and
// it stays. `tools/check-resources.sh` exists for the same reason.
fn pushtest(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let secs: u64 = match args.last() {
        Some(w) => match w.parse() {
            Ok(n) => n,
            Err(_) => return Ok(bstr(&format!("not a number: {w}"))),
        },
        None => 3,
    };
    // A delay longer than the client's own keepalive would prove
    // something different and take four minutes to do it.
    if secs > 120 {
        return Ok(bstr("pick 120 seconds or less"));
    }
    let Some(c) = actor.and_then(|id| reg.character(id)) else {
        return Ok(bstr("no character to notify"));
    };
    let (owner_id, balance) = (c.character_id, c.balance);
    reg.push_later(
        Duration::from_secs(secs),
        crate::notify::AccountChange { owner_id, balance },
    );
    Ok(bstr(&format!(
        "queued an OnAccountChange for {secs}s from now; \
         the wallet should blink with the same balance, \
         without the client asking for anything"
    )))
}

// **`/tp <km>`: put my ship that far out from the station.**
//
// Ours, not CCP's, and it exists because of a practical wall the
// morning after warp shipped. The client only offers "Warp To"
// for a target **more than 150 km away**
// (`appConst.minWarpDistance`) **[V]**, and the only objects in
// this sandbox are the station and the other ships. A capsule
// undocks 16 km out and cruises at a few hundred metres a
// second, so reaching warp range is an eight-to-twenty-minute
// flight in a straight line, every time anyone wants to test
// warp. That is not a test, it is a commute.
//
// **`SetBallPosition` and `SetBallVelocity` are the right
// instruments and the reason is unusual.** Both sit *outside*
// all three of the client's dispatch sets, so `RealFlushState`
// calls `_parent_SetBallPosition` and nothing else happens: no
// state reset, no scatter, no bracket rebuild. Which is exactly
// what a teleport wants. `docs/lanes/movement.md` §8 found this
// while establishing that they cannot be used to *correct* a
// drifted observer -- the same property, read the other way
// round.
//
// `Stop` comes along so the ship is parked where it was put
// rather than carrying on toward whatever it was last sent at.
//
// The audience is the system, not the pilot: a ship that
// teleports on one screen and not the others is a desync we
// caused on purpose.
fn tp(reg: &mut Registry, actor: Option<i64>, args: &[&str]) -> Result<PyValue, ServiceError> {
    let km: f64 = match args.first() {
        Some(w) => match w.parse() {
            Ok(n) => n,
            Err(_) => return Ok(bstr(&format!("not a number: {w}"))),
        },
        // Comfortably past `minWarpDistance`, so the default is
        // the thing you wanted.
        None => 200.0,
    };
    // **The floor is the station's own radius, not zero.**
    // `/tp 0` put the ship at the origin, which is the middle of
    // a 15.7 km station model -- the exact state
    // `destiny::berth` goes to some trouble to keep ships out
    // of. 20 km is `destiny::UNDOCK_OFFSET`, so the smallest
    // teleport this allows is where a ship would have been
    // standing anyway.
    let floor = crate::destiny::UNDOCK_OFFSET / 1_000.0;
    if !(floor..=1.0e6).contains(&km) {
        return Ok(bstr(&format!(
            "pick between {floor} and 1000000 km; below that you \
             are inside the station model"
        )));
    }
    let Some(actor) = actor else {
        return Ok(bstr("no character"));
    };
    reg.advance_character(actor);
    let Some(here) = reg.roster().get(actor).copied() else {
        return Ok(bstr("you are not in space"));
    };
    // Outward from the pilot's station, where it is
    // (`Registry::station_position`, FX4). A ship sitting exactly on it
    // has no outward to go, and gets one.
    let centre = reg
        .character(actor)
        .and_then(|c| c.station_id)
        .map_or([0.0; 3], |id| reg.station_position(id));
    let from = [
        here.motion.position[0] - centre[0],
        here.motion.position[1] - centre[1],
        here.motion.position[2] - centre[2],
    ];
    let out = crate::space_state::unit(from).unwrap_or([1.0, 0.0, 0.0]);
    let metres = km * 1_000.0;
    let point = [
        centre[0] + out[0] * metres,
        centre[1] + out[1] * metres,
        centre[2] + out[2] * metres,
    ];
    let actions = vec![
        crate::destiny::Action::SetBallPosition {
            ball: here.ship_id,
            point,
        },
        crate::destiny::Action::SetBallVelocity {
            ball: here.ship_id,
            velocity: [0.0; 3],
        },
        crate::destiny::Action::Stop { ball: here.ship_id },
    ];
    if let Some(p) = reg.roster_mut().get_mut(actor) {
        for action in &actions {
            p.motion.after(*action);
        }
    }
    let stamp = reg.next_destiny_stamp();
    let mut to = vec![actor];
    to.extend(reg.roster().others_in_system(here.solar_system_id, actor));
    if let Some(update) = crate::destiny::ActionUpdate::new(to, stamp, actions) {
        reg.push(update);
    }
    // **A teleport crosses grids**, so the parks are brought up to it now
    // rather than at the next reconcile (`docs/lanes/grid-research.md` §3):
    // the move above goes to whoever held the ship where it was, and this
    // takes it out of their parks and puts it into the new grid's.
    crate::grid::reconcile_in(reg, here.solar_system_id);
    Ok(bstr(&format!(
        "moved you {km} km out from the station; the station is \
         now a legal warp target (the client needs 150 km)"
    )))
}

// `whoami` is answered entirely client-side and should never
// reach us; if it does, something has changed in the dispatcher
// and that is worth knowing rather than guessing about.
fn whoami(
    _reg: &mut Registry,
    _actor: Option<i64>,
    _args: &[&str],
) -> Result<PyValue, ServiceError> {
    Ok(bstr(
        "whoami is handled in the client; seeing this means the \
     client's slash dispatcher changed",
    ))
}

// **The GM skill commands, which are the cheap half of issue
// #5.** Buying a skill needs the market; granting one needs only
// the map, and it is what makes the skill system testable at all
// without ISK, a price table or a market window.
//
// Both push `OnSkillForcedRefresh` for the same reason
// `purchase_skills` does: the client's skill caches are only
// reset by that event, so a grant the client never re-reads is a
// grant nobody can see.
fn skill_command(
    reg: &mut Registry,
    cmd: &str,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let Some(actor) = actor else {
        return Ok(bstr("no character to change"));
    };
    if cmd == "givestarterskills" {
        let starter = crate::skill::Skills::starter_in(reg.content());
        let Some(s) = reg.skills_mut(actor) else {
            return Ok(bstr("no skills for this character"));
        };
        *s = starter;
        let n = s.len();
        reg.mark_dirty();
        reg.push(crate::skill::ForcedRefresh);
        return Ok(bstr(&format!("reset to the {n} starter skills")));
    }
    let Some(first) = args.first() else {
        return Ok(bstr(&format!("usage: /{cmd} <skill|typeID> [level]")));
    };
    // A level is only a level if the last word parses as one, so
    // a skill name with spaces still works.
    let (name, level) = match args.last().and_then(|w| w.parse::<i64>().ok()) {
        Some(n) if args.len() > 1 => (args[..args.len() - 1].join(" "), n),
        _ => (args.join(" "), 1),
    };
    // Cloned out of `reg.content()`, so the skill sheet can be borrowed
    // mutably below (CP59b).
    let Some(t) = crate::skill::find_any_in(reg.content(), &name).cloned() else {
        let names: Vec<&str> = crate::skill::all_in(reg.content())
            .map(|s| s.name)
            .collect();
        return Ok(bstr(&format!(
            "no such skill '{first}' -- known: {}",
            names.join(", ")
        )));
    };
    let (type_id, skill_name) = (t.type_id, t.name);
    let Some(s) = reg.skills_mut(actor) else {
        return Ok(bstr("no skills for this character"));
    };
    let line = if cmd == "removeskill" {
        if s.remove(type_id) {
            format!("removed {skill_name}")
        } else {
            return Ok(bstr(&format!("{skill_name} was not trained")));
        }
    } else {
        let e = s.grant_type(&t, level);
        format!("{skill_name} is now level {} ({} SP)", e.level, e.points)
    };
    reg.mark_dirty();
    reg.push(crate::skill::ForcedRefresh);
    Ok(bstr(&format!("{line}.")))
}

fn giveskill(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    skill_command(reg, "giveskill", actor, args)
}

fn removeskill(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    skill_command(reg, "removeskill", actor, args)
}

fn givestarterskills(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    skill_command(reg, "givestarterskills", actor, args)
}

// **`createitem` is the first mutator that puts something in the
// world the player can pick up.**
//
// The real EVE command's name, not one of ours, because it is the
// one a GM would already type. With no arguments it lists what it
// can grant, which is the only discoverable thing to do when the
// table is two dozen types rather than all of EVE.
//
// The push is not optional and it is not decoration: see
// `notify::InvalidateLocations`. Without it the item exists,
// every call reports it, and the window shows nothing until the
// player docks somewhere else.
fn createitem(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let Some(c) = actor.and_then(|id| reg.character(id)) else {
        return Ok(bstr("no character to give anything to"));
    };
    let (owner, ship) = (c.character_id, c.ship_id);
    // `docked_station`, not the row's `station_id`: that stays set in space.
    let docked = reg.docked_station(owner);
    let station = docked.unwrap_or(0);
    if args.is_empty() {
        let names: Vec<&str> = crate::item::SPAWNABLE.iter().map(|t| t.name).collect();
        return Ok(bstr(&format!(
            "usage: /createitem <type|id> [quantity]\nknown types: {}",
            names.join(", ")
        )));
    }
    // `/createitem rifter 2` and `/createitem "Iteron Mark V"`
    // both have to work, and a name can contain spaces -- so the
    // quantity is only a quantity if the *last* word parses as
    // one, and everything before it is the type.
    let (name, quantity) = match args.last().and_then(|w| w.parse::<i64>().ok()) {
        Some(n) if args.len() > 1 => (args[..args.len() - 1].join(" "), n),
        // A single numeric argument is a type id, not a quantity.
        _ => (args.join(" "), 1),
    };
    // The client's console passes the line on as typed, quotes included
    // (`/createitem "Salvager I" 2`): drop them, the words are already joined.
    let name = name.trim_matches('"').trim().to_owned();
    if docked.is_none() {
        return Ok(bstr(&create_in_hold(reg, owner, ship, &name, quantity)));
    }
    let (ids, t) = match reg.spawn_items(&name, quantity, owner, station) {
        Ok(v) => v,
        Err(e) => return Ok(bstr(&format!("{e}"))),
    };
    reg.mark_dirty();
    let (type_name, count) = (t.name, ids.len());
    // **Both pushes, because they cover different windows.**
    // `OnItemChange` adds the item to an inventory that is open
    // right now; `OnInvalidateCacheForLocations` drops the
    // container so a window opened *later* re-asks, which matters
    // because `invCacheContainer.listed` has no TTL and would
    // otherwise never refetch. Neither subsumes the other.
    for id in &ids {
        if let Some(item) = reg.item(*id) {
            let item = item.clone();
            let row = reg.item_row_value(&item);
            reg.push(crate::notify::ItemChange::spawned(row));
        }
    }
    reg.push(crate::notify::InvalidateLocations {
        location_ids: vec![station],
    });
    Ok(bstr(&format!(
        "created {count} x {type_name} ({}). \
         Reopen the inventory to see {}.",
        ids.iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        if count == 1 { "it" } else { "them" }
    )))
}

// **In space there is no hangar**: the stack goes into the pilot's ship hold
// (flag 5) instead of an orphan row at location 0 that nobody could reach.
// **[A]** retail GM behaviour in space is unconfirmed; this is GM-only. Falsifier:
// a client that `/createitem`s in space and finds the stack elsewhere.
fn create_in_hold(reg: &mut Registry, owner: i64, ship: i64, name: &str, quantity: i64) -> String {
    if !(1..=1000).contains(&quantity) {
        return crate::item::SpawnError::BadQuantity(quantity).to_string();
    }
    let Some(t) = crate::item::find_type_in(reg.content(), name).copied() else {
        return crate::item::SpawnError::UnknownType(name.to_owned()).to_string();
    };
    let plan = [crate::item::Edit::Create {
        type_id: t.type_id,
        quantity,
        to: crate::item::Slot::new(owner, ship, crate::fitting::FLAG_CARGO),
    }];
    let changes = match reg.apply_items(&plan) {
        Ok(c) => c,
        Err(e) => return format!("{e}"),
    };
    let ids: Vec<String> = changes
        .iter()
        .filter_map(|c| match c {
            crate::item::Change::Created { item_id, .. } => Some(item_id.to_string()),
            _ => None,
        })
        .collect();
    format!(
        "created {} x {} ({}) in your ship's cargo hold. Reopen the cargo to see {}.",
        ids.len(),
        t.name,
        ids.join(", "),
        if ids.len() == 1 { "it" } else { "them" }
    )
}

// `/setstanding <fromID> <me|characterID> <value>`, in the
// grammar the client's own QA tools send. The parsing, the
// store and both pushes are `service/character.rs`'s standings
// section, which has the evidence.
fn setstanding(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    Ok(crate::service::set_standing_slash(reg, actor, args))
}

// `/entity deploy <qty> <typeID>`: a hostile NPC in space beside
// the caller. `crate::npc` has the evidence and the refusals.
fn entity(reg: &mut Registry, actor: Option<i64>, args: &[&str]) -> Result<PyValue, ServiceError> {
    Ok(crate::npc::entity_slash(reg, actor, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name is matched exactly against what the client sends, and the
    /// client lowercases the command word (`"/" + lowercased-command`, see
    /// `service/gm.rs`). So an uppercase name could never be reached, and a
    /// duplicate would be answered by the first row and silently not by the
    /// second -- the same hazard `DOMAINS` guards against.
    #[test]
    fn command_names_are_unique_and_lowercase() {
        let mut seen: Vec<&str> = Vec::new();
        for c in COMMANDS {
            assert!(!c.name.is_empty(), "a command with no name");
            assert_eq!(
                c.name,
                c.name.to_lowercase(),
                "{} can never match: the client lowercases the command",
                c.name
            );
            assert!(
                !c.name.contains(char::is_whitespace) && !c.name.starts_with('/'),
                "{:?} is not one word",
                c.name
            );
            assert!(
                !seen.contains(&c.name),
                "{} is in COMMANDS twice; the second row would never run",
                c.name
            );
            seen.push(c.name);
        }
    }

    /// In space `/createitem` puts the stack in the ship's cargo hold, not at
    /// location 0.
    #[test]
    fn createitem_in_space_goes_to_the_ship_hold() {
        let mut reg = Registry::new(crate::model::SandboxConfig::default());
        let c = reg.config().characters[0].clone();
        crate::destiny::on_ballpark_bind(&mut reg, Some(c.character_id));
        assert_eq!(reg.docked_station(c.character_id), None);
        let name = crate::item::SPAWNABLE
            .iter()
            .find(|t| !t.singleton)
            .expect("a stackable spawnable type")
            .name;
        let before = reg.store().items().len();
        createitem(&mut reg, Some(c.character_id), &[name, "3"]).unwrap();
        let new: Vec<_> = reg.store().items().iter().skip(before).collect();
        assert_eq!(new.len(), 1);
        assert_eq!(
            (new[0].location_id, new[0].flag_id, new[0].quantity),
            (c.ship_id, crate::fitting::FLAG_CARGO, 3)
        );
        assert!(reg.store().items().iter().all(|i| i.location_id != 0));
    }
}
