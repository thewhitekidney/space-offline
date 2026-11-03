//! Stargate jumps: from the click to arrival in the next system, on the clock.
//!
//! Spec: `docs/lanes/stargates.md` (P3.3). Protocol truth:
//! `docs/re/jump-gates.md`, superseded where the spec says so (§2.2, §2.3).
//!
//! ## The sequence
//!
//! `beyonce.CmdStargateJump(stargateID, beaconID, shipID)` arrives from
//! `gateJumpSvc.JumpThroughGate:131`, wrapped in
//! `sessionMgr.PerformSessionChange`, which **discards the answer** and
//! waits for nothing after it **[V]**. What moves the client is three things
//! we send, and their order is the whole design:
//!
//! | t | what | to | why then |
//! |---|---|---|---|
//! | 0 | [`JumpFx`] -- `OnSpecialFX(..., 'effects.JumpOut', ...)` | jumper + origin observers | the tunnel: `subwaySvc.OnSpecialFX:481` starts the jump for `shipID == session.shipid` and plays the departure for everyone else **[V]** |
//! | [`REMOVE_AFTER`] | `RemoveBalls([ship])` | the same | the jumper's own ball must be **gone** before the session change |
//! | [`JUMP_DELAY`] | session change `solarsystemid(2): (origin, dest)` | jumper | `gameui` finds no ego, rebuilds the park, binds `beyonce` again |
//! | bind | `SetState` at the destination gate | jumper; `AddBalls2` to the destination's pilots | [`take_arrival`] in `destiny::on_ballpark_bind` |
//!
//! **Why not one dispatcher turn**, as `re/jump-gates.md` §3.5 asked: the
//! dispatcher sends pushes *after* the session change by construction, and
//! the client applies a `RemoveBalls` only on its next Destiny tick, while
//! `gameui._HandleSessionChangeInSpace:773` tests `session.shipid in bp.balls`
//! immediately -- so the removal has to have been *applied* a tick before the
//! change arrives (`docs/lanes/stargates.md` §2.2).
//!
//! **Why remove the jumper's own ball at all** -- the spec's top risk 1, and
//! `[I]`: if the ego is still in the park when the change lands, `gameui`
//! takes the "valid ego" branch, never calls `ChangePrimaryView`, and the
//! park is the origin's for ever with nothing logged as an error. One client
//! log line settles it on the first relaunch: `without a ego ball properly
//! inserted` is the branch we want; `Heading for inflight with a valid ego`
//! is this premise failing. Fallback if it fails: remove *every* ball in the
//! jumper's park, as `Park.Release` does (`michelle.py:1196`).
//!
//! ## What a jump moves, and what it does not
//!
//! **The roster presence moves; the character does not.**
//! `Character::solar_system_id` is the system the character's *station* is
//! in, at every site that reads it outside space (login, undock, dock,
//! locations); the roster is already where in-space code looks
//! (`docs/lanes/stargates.md` §2.3). So the jump takes the presence out of
//! the roster at t=0 and the destination's bind puts it back in the other
//! system. A relog after a jump is docked at home, which is true.
//!
//! ## What this does not do
//!
//! No jump cloak and no `effects.JumpIn` (an arrival cloak never cleared
//! disables the autopilot for good, `autopilot.py:507` **[V]**); no
//! server-side range check (the client enforces 2,500 m before it sends,
//! `menusvc.py:3541`, as for docking -- a bot can jump from anywhere, P3.3b);
//! no region change (the pair is one constellation).

use crate::service::{Registry, Request, ServiceError};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// From the click to the session change.
///
/// `const.invulnerabilityJumping = 4000`, the client's own number
/// (`ballparkCommon/jumpdelay.py:8-11`) **[V]** as a value; that it is what
/// the real server waits is `[I]`. It is also the destination's loading time:
/// `subwaySvc.StartJump:555-557` loads the scene while it waits, and nothing
/// in the subway times the wait out **[V]**.
pub const JUMP_DELAY: Duration = Duration::from_millis(4000);

/// From the click to the jumper's ball leaving every park.
///
/// `[A]`. At least one client Destiny tick (one second) after the FX, so the
/// subway has locked the ship's model before its ball goes
/// (`subwaySvc.StartJump:579-584`), and at least one tick before the session
/// change, so the removal is *applied* when `gameui` looks. Falsifiers: the
/// jumper's own tunnel shows no ship (too early); `Heading for inflight with
/// a valid ego` in the client log (too late).
pub const REMOVE_AFTER: Duration = Duration::from_millis(2000);

/// How far outside the destination gate's surface the ship appears, metres.
///
/// `[A]`, and bounded by a `[V]` constraint the tests hold it to: the
/// arrival must be **inside** `const.maxStargateJumpingDistance` (2,500 m,
/// surface to surface) so that an immediate jump back is legal. EvEJS uses
/// 12 km `[V2]`, which our own constraint rules out.
pub const ARRIVAL_CLEARANCE: f64 = 1000.0;

/// `const.maxStargateJumpingDistance`: the client's range for a jump,
/// surface to surface **[V]**.
pub const MAX_JUMP_DISTANCE: f64 = 2500.0;

/// The departure effect's guid **[V]** (`re/jump-gates.md` §3.1).
pub const FX_JUMP_OUT: &str = "effects.JumpOut";

/// One character's jump, between the click and the destination's bind.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Leg {
    /// Out of the roster, waiting for [`JUMP_DELAY`].
    Departing {
        at: Instant,
        origin: i64,
        dest_system: i64,
        dest_gate: i64,
    },
    /// The session change has gone; waiting for the destination's bind.
    Arriving { system: i64, gate: i64 },
}

/// Every jump in flight, by character.
///
/// **No entry outlives its session**: [`tick`] drops the jump of anybody
/// with no live connection, so a client that dies in the tunnel leaves
/// nothing behind -- and, because their presence already left the roster,
/// nothing in space either. Their next login is docked at home.
///
/// The second map is where a jump that lands at a point rather than by a
/// gate lands ([`arm_at`], W17 CP54b's wormhole).
#[derive(Debug, Clone, Default)]
pub struct Jumps(BTreeMap<i64, Leg>, BTreeMap<i64, [f64; 3]>);

impl Jumps {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether this character is between a gate and the next system's bind.
    pub fn contains(&self, character_id: i64) -> bool {
        self.0.contains_key(&character_id)
    }

    /// Every character in a jump, any leg: whose ship is out of the roster
    /// but not out of the world (`combat::tick` keeps its health record).
    pub fn characters(&self) -> impl Iterator<Item = i64> + '_ {
        self.0.keys().copied()
    }

    /// Every jump in flight as `(character, destination system)`: where a
    /// departing leg is going and where an arriving one lands.
    /// `crate::liveness` warms these before the arrival (C7 §2.1).
    pub fn destinations(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        self.0.iter().map(|(&c, leg)| match *leg {
            Leg::Departing { dest_system, .. } => (c, dest_system),
            Leg::Arriving { system, .. } => (c, system),
        })
    }

    /// The system a jumper's session names: the origin until the session
    /// change ([`JUMP_DELAY`]), the destination after it. `None` when not
    /// in a jump.
    pub fn session_system(&self, character_id: i64) -> Option<i64> {
        self.0.get(&character_id).map(|leg| match *leg {
            Leg::Departing { origin, .. } => origin,
            Leg::Arriving { system, .. } => system,
        })
    }
}

/// Where a jump ends: [`take_arrival`]'s answer to `on_ballpark_bind`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arrival {
    pub system: i64,
    pub motion: crate::space_state::Motion,
}

/// Whether this character is in a jump. `destiny::bootstrap_for` refuses to
/// build a state while it is, or an `UpdateStateRequest` in the window would
/// seed the jumper at the home station's mouth.
pub fn in_transit(w: &Registry, character_id: i64) -> bool {
    w.jumps().contains(character_id)
}

/// The session change has gone and the destination's bind has not come: the
/// client's next park takes its SetState first or never (`michelle.py`
/// RealFlushState), so nothing else may reach it in this window.
pub fn awaiting_bind(w: &Registry, character_id: i64) -> bool {
    matches!(w.jumps().0.get(&character_id), Some(Leg::Arriving { .. }))
}

/// `beyonce.CmdStargateJump(stargateID, beaconID, shipID)`.
///
/// `beaconID` is the **destination** gate (`gateJumpSvc.JumpThroughGate`
/// reads it from the gate's `jumps` row) **[V]**. Answered `None`, which
/// `PerformSessionChange` discards; the effect is the sequence in the module
/// header. Refused, with nothing changed, when:
///
/// - the caller is not in space, or `shipID` is not the ship they fly;
/// - a jump of theirs is already in flight;
/// - `stargateID` is not a gate in the caller's current system;
/// - `beaconID` is not that gate's partner;
/// - the partner is not loaded on this server.
pub fn cmd_stargate_jump(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(notify("No character on this session."));
    };
    let (Some(gate_id), Some(beacon_id), Some(ship_id)) = (int(req, 0), int(req, 1), int(req, 2))
    else {
        return Err(notify(
            "CmdStargateJump needs (stargateID, beaconID, shipID).",
        ));
    };
    let Some(here) = reg.roster().get(actor).copied() else {
        return Err(notify("You are not in space."));
    };
    if here.ship_id != ship_id {
        return Err(notify("That is not the ship you are flying."));
    }
    if reg.jumps().contains(actor) {
        return Err(notify("You are already jumping."));
    }
    // Under a weapons timer the gate says no, in the client's own words
    // (`CantJumpAfterAggression`, `crate::crimewatch`).
    // A criminal's refusal comes first (CONCORD-1b): its words, not the timer's.
    crate::concord::refuse_jump(reg, Some(actor), crate::pyutil::filetime_now())?;
    crate::crimewatch::refuse_jump(reg, Some(actor), crate::pyutil::filetime_now())?;
    // The registered movement guards (W10.5 seam S12), refused the same way.
    if let Some(refusal) =
        crate::movement_guard::check(&crate::module::plan().jump_guards, reg, actor)
    {
        return Err(refusal.into_error());
    }
    let origin = here.solar_system_id;
    let catalogue = reg.celestials();
    let is_gate_here = catalogue
        .get(gate_id)
        .is_some_and(|c| c.kind == crate::celestial::Kind::Gate)
        && catalogue.system_of(gate_id) == Some(origin);
    if !is_gate_here {
        return Err(notify("That stargate is not in this system."));
    }
    if catalogue.jump_of(gate_id).map(|(to, _)| to) != Some(beacon_id) {
        return Err(notify("That is not where this stargate goes."));
    }
    let Some((_, partner)) = catalogue.gate(gate_id) else {
        return Err(notify(
            "This stargate's destination is not loaded on this server.",
        ));
    };
    let dest_gate = partner.id;
    let Some(dest_system) = catalogue.system_of(dest_gate).filter(|s| *s != 0) else {
        return Err(notify(
            "This stargate's destination is not loaded on this server.",
        ));
    };

    // **Everyone whose park holds the ship**, the jumper first: the FX is
    // the jumper's tunnel and everyone else's departure flash
    // (`subwaySvc.OnSpecialFX:481` / `:495`), and the removal is the same
    // audience's. Characters only -- an NPC has no park.
    let mut audience = vec![actor];
    audience.extend(
        reg.roster()
            .others_in_system(origin, actor)
            .into_iter()
            .filter(|id| reg.character(*id).is_some()),
    );
    let stamp = reg.next_destiny_stamp();
    reg.push(JumpFx {
        to: audience.clone(),
        stamp,
        ship: ship_id,
        gate: gate_id,
        dest_system,
    });
    // **Locks before the roster**, for the reason `Registry::dock_change`
    // gives: `left_space` needs the presence to find the ball, and a victim
    // nobody tells keeps a stuck bracket. `WithAnswer`: this is a call, and
    // its answer's flush carries them.
    crate::targeting::left_space(
        reg,
        actor,
        crate::targeting::Reason::Unlocked,
        crate::targeting::Delivery::WithAnswer,
    );
    let held = crate::jump_drive::park_held(reg, actor, ship_id);
    reg.roster_mut().leave(actor);
    // **The jumper's park is the origin's, and it is over** (`crate::grid`):
    // the destination's `SetState` starts the next one. Until then the
    // jumper is unmodelled, so the tunnel and the removal above still reach
    // them; everyone else's copy of the removal goes to the parks that hold
    // the ship when it leaves. The jumper's own copy drops its whole park,
    // ego last (S8; `jump_drive::schedule_departure_removal`).
    crate::grid::forget(reg, actor);
    crate::jump_drive::schedule_departure_removal(reg, stamp, ship_id, held, audience);
    reg.jumps_mut().0.insert(
        actor,
        Leg::Departing {
            at: Instant::now(),
            origin,
            dest_system,
            dest_gate,
        },
    );
    crate::outln!(
        "[jump] character {actor} ship {ship_id}: gate {gate_id} in {origin} -> \
         gate {dest_gate} in {dest_system}"
    );
    Ok(PyValue::None)
}

/// The jump's share of the world clock: send each session change that is
/// due, and forget the jumps of anybody who is gone.
///
/// Called from `push::spawn_world_ticker` under the world lock, **before**
/// `session_changes_due`, so a change queued here leaves in the same tick.
pub fn tick(w: &mut Registry, now: Instant) {
    let gone: Vec<i64> = w
        .jumps()
        .0
        .keys()
        .copied()
        .filter(|c| w.sessions().of_character(*c).is_empty())
        .collect();
    for c in gone {
        w.jumps_mut().0.remove(&c);
        w.jumps_mut().1.remove(&c);
    }
    let due: Vec<(i64, i64, i64, i64)> = w
        .jumps()
        .0
        .iter()
        .filter_map(|(&c, leg)| match *leg {
            Leg::Departing {
                at,
                origin,
                dest_system,
                dest_gate,
            } if now.saturating_duration_since(at) >= JUMP_DELAY => {
                Some((c, origin, dest_system, dest_gate))
            }
            _ => None,
        })
        .collect();
    for (c, origin, dest_system, dest_gate) in due {
        let change = session_change(system_row(w, c, origin), system_row(w, c, dest_system));
        w.queue_session_change(c, change);
        w.jumps_mut().0.insert(
            c,
            Leg::Arriving {
                system: dest_system,
                gate: dest_gate,
            },
        );
        crate::map_stats::on_jump(w, dest_system);
        crate::outln!("[jump] character {c}: session change {origin} -> {dest_system}");
    }
}

/// The arrival this character's bind should land, if they are mid-jump and
/// the session change has gone. Consumes it: one bind, one arrival.
pub fn take_arrival(w: &mut Registry, character_id: i64) -> Option<Arrival> {
    let Leg::Arriving { system, gate } = *w.jumps().0.get(&character_id)? else {
        return None;
    };
    w.jumps_mut().0.remove(&character_id);
    // A jump armed by [`arm_at`] lands where it was told to (W17 CP54b).
    if let Some(spot) = w.jumps_mut().1.remove(&character_id) {
        return Some(Arrival {
            system,
            motion: crate::space_state::Motion::arrived(spot, [0.0; 3]),
        });
    }
    let gate = *w.celestials().get(gate)?;
    // A berth each, as at a station mouth: pilots already here, characters
    // only, are the index.
    let index = w
        .roster()
        .others_in_system(system, character_id)
        .into_iter()
        .filter(|id| w.character(*id).is_some())
        .count();
    Some(Arrival {
        system,
        motion: crate::space_state::Motion::arrived(arrival_position(&gate, index), [0.0; 3]),
    })
}

/// **A jump that lands at a point rather than by a gate** (W17 CP54b,
/// `crate::wormhole_jump`): the same legs, session change and bind as a
/// stargate jump, and [`take_arrival`] lands it at `spot`, at rest. The
/// caller has already sent its own FX and removal and taken the ship out of
/// the roster, as [`cmd_stargate_jump`] does.
pub fn arm_at(w: &mut Registry, character_id: i64, origin: i64, dest_system: i64, spot: [f64; 3]) {
    w.jumps_mut().1.insert(character_id, spot);
    w.jumps_mut().0.insert(
        character_id,
        Leg::Departing {
            at: Instant::now(),
            origin,
            dest_system,
            dest_gate: 0,
        },
    );
}

/// **Land a character in `dest_system` at `spot`, now**: the session change
/// has been (or is being) sent by the caller, and the `beyonce` bind that
/// follows lands the ship there at rest, as a gate arrival does. SESSION-GM's
/// `/tr` and `/devhome` use it; the caller has already taken the ship out of
/// the roster.
pub fn arm_arrival(w: &mut Registry, character_id: i64, dest_system: i64, spot: [f64; 3]) {
    w.jumps_mut().1.insert(character_id, spot);
    w.jumps_mut().0.insert(
        character_id,
        Leg::Arriving {
            system: dest_system,
            gate: 0,
        },
    );
}

/// Where the `index`-th ship to arrive at this gate appears.
///
/// `[A]`: [`ARRIVAL_CLEARANCE`] outside the gate's surface, on the line from
/// the gate toward the system's origin (the star -- the side a ship arriving
/// from elsewhere in the system would come from), then stepped tangentially
/// as `destiny::berth` steps ships at a station mouth. At rest. A gate at the
/// origin itself, which no real one is, gets an arbitrary axis.
pub fn arrival_position(gate: &crate::celestial::Celestial, index: usize) -> [f64; 3] {
    let p = gate.position;
    let inward = crate::space_state::unit([-p[0], -p[1], -p[2]]).unwrap_or([1.0, 0.0, 0.0]);
    let out = gate.radius + ARRIVAL_CLEARANCE;
    let spot = [
        p[0] + inward[0] * out,
        p[1] + inward[1] * out,
        p[2] + inward[2] * out,
    ];
    crate::destiny::berth(spot, p, index)
}

/// A system's constellation and region, from the extract's `#system` line,
/// or -- for the character's home system, which an old extract may not have
/// a line for -- from the character row.
///
/// Also the login's: a pilot who logs back in to a ship held in space
/// (`crate::crimewatch::logged_off`) is told the system the ship is in,
/// which after a jump is not the character row's (`Registry::character_change`).
pub(crate) fn system_row(
    w: &Registry,
    character_id: i64,
    system: i64,
) -> Option<crate::celestial::SystemRow> {
    if let Some(row) = w.celestials().system(system) {
        return Some(*row);
    }
    // A system the catalogue has no `#system` line for but the universe table
    // holds (a jump-drive destination need not have a gate): the same two ids,
    // from `universe/systems.tsv`.
    if let Some(s) = w
        .content()
        .get::<crate::celestial::Universe>()
        .system(system)
    {
        return Some(crate::celestial::SystemRow {
            id: system,
            constellation_id: s.constellation_id,
            region_id: s.region_id,
        });
    }
    let c = w.character(character_id)?;
    (c.solar_system_id == Some(system)).then_some(crate::celestial::SystemRow {
        id: system,
        constellation_id: c.constellation_id,
        region_id: c.region_id,
    })
}

/// The jump's session change: `solarsystemid` and `solarsystemid2` as
/// `(origin, dest)`, and `constellationid` / `regionid` only when they move.
///
/// **Nothing else.** No `locationid` (derived client-side from
/// `solarsystemid`, `basesession.py:2183`), no `shipid` (the same ship, and
/// the `invCache` trap `Registry::dock_change` documents), no `stationid`
/// (`re/jump-gates.md` §3.3) **[V]**. A key whose new value equals the
/// client's is dropped by the client anyway; sending only what moves keeps
/// the change honest.
///
/// `[A]` when either system has no `#system` row: the two system keys only,
/// i.e. the constellation is taken not to move. True for the one pair this
/// sandbox ships; falsified by a jump between constellations with an old
/// extract, which the client would show as the wrong constellation name.
pub fn session_change(
    origin: Option<crate::celestial::SystemRow>,
    dest: Option<crate::celestial::SystemRow>,
) -> Vec<(PyValue, PyValue)> {
    let pair = |a: i64, b: i64| PyValue::Tuple(vec![PyValue::Int(a), PyValue::Int(b)]);
    let (o, d) = (origin.map_or(0, |r| r.id), dest.map_or(0, |r| r.id));
    let mut kv = vec![
        ("solarsystemid", pair(o, d)),
        ("solarsystemid2", pair(o, d)),
    ];
    if let (Some(o), Some(d)) = (origin, dest) {
        if o.constellation_id != d.constellation_id {
            kv.push((
                "constellationid",
                pair(o.constellation_id, d.constellation_id),
            ));
        }
        if o.region_id != d.region_id {
            kv.push(("regionid", pair(o.region_id, d.region_id)));
        }
    }
    let PyValue::Dict(entries) = crate::pyutil::pydict(kv) else {
        unreachable!("pydict builds a dict")
    };
    entries
}

/// The departure: one `OnSpecialFX` with all **fourteen** positionals.
///
/// `(shipID, moduleID, moduleTypeID, targetID, otherTypeID, guid,
/// isOffensive, start, active, duration, repeat, startTime, timeFromStart,
/// graphicInfo)` -- the argcount of both `Park.OnSpecialFX`
/// (`michelle.py:2087`) and `subwaySvc.OnSpecialFX` (`subwaySvc.py:472`)
/// **[V]**. `destiny::SpecialFx` sends nine and stops before `graphicInfo`,
/// which a jump cannot: `subwaySvc.py:477` reads the destination system out
/// of `graphicInfo[0]`, and `targetID` is the **origin gate**, stored as
/// `self.gateID` (`:483`) **[V]**.
///
/// In `nonDestinyCriticalFunctions`, so it costs the park no `Evolve`
/// (`michelle.py:1048-1057`) **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct JumpFx {
    pub to: Vec<i64>,
    pub stamp: u32,
    pub ship: i64,
    pub gate: i64,
    pub dest_system: i64,
}

impl crate::notify::Push for JumpFx {
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let args = vec![
            PyValue::Int(self.ship),
            // moduleID: `None` scatters once (`Park.OnSpecialFX:2088`).
            PyValue::None,
            PyValue::None,
            // targetID: the gate the ship is leaving through.
            PyValue::Int(self.gate),
            PyValue::None,
            crate::pyutil::bstr(FX_JUMP_OUT),
            // isOffensive, start, active.
            PyValue::Int(0),
            PyValue::Int(1),
            PyValue::Int(0),
            // duration, repeat, startTime, timeFromStart: the signature's
            // own defaults (`-1, None, None, 0`) **[V]**.
            PyValue::Int(-1),
            PyValue::None,
            PyValue::None,
            PyValue::Int(0),
            // graphicInfo: `[0]` is where the tunnel goes.
            PyValue::Tuple(vec![PyValue::Int(self.dest_system)]),
        ];
        let entry = PyValue::Tuple(vec![
            PyValue::Int(i64::from(self.stamp)),
            PyValue::Tuple(vec![
                crate::pyutil::bstr("OnSpecialFX"),
                PyValue::Tuple(args),
            ]),
        ]);
        (
            "DoDestinyUpdate",
            vec![PyValue::List(vec![entry]), PyValue::Int(0)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    /// The jumping ship, which is the first argument `Park.OnSpecialFX`
    /// looks up.
    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Names(vec![self.ship])
    }
}

fn int(req: &Request<'_>, i: usize) -> Option<i64> {
    match req.args.get(i) {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    }
}

/// Our own words: `CustomNotify` is the generic escape hatch
/// (`findings/user-errors.tsv`), for refusals about *this server*.
fn notify(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::Push;

    const ALICE: i64 = 1_000_001;
    const BOB: i64 = 1_000_002;
    const ALICE_SHIP: i64 = 100_000_001;
    const BOB_SHIP: i64 = 100_000_002;
    /// The characters' home: `Character::default()`'s system.
    const A: i64 = 30_000_001;
    const B: i64 = 30_000_003;
    const GATE_A: i64 = 51;
    const GATE_B: i64 = 52;
    /// In B, leading to a gate this catalogue does not hold.
    const GATE_OUT: i64 = 53;

    /// Two systems, **synthetic coordinates and ids** (the no-client-data
    /// rule), in the extract's ten-column format. Three gate radii, so the
    /// arrival bound is checked against more than one type.
    fn catalogue(constellation_b: i64) -> crate::celestial::Catalogue {
        crate::celestial::Catalogue::parse(&format!(
            "#system\t{A}\t10\t100\n\
             #system\t{B}\t{constellation_b}\t100\n\
             planet\t12\t11\t500.0\t100000000.0\t0.0\t0.0\t{A}\t0\t0\n\
             gate\t{GATE_A}\t29624\t2473.0\t5000000000.0\t0.0\t0.0\t{A}\t{GATE_B}\t{B}\n\
             planet\t22\t11\t500.0\t0.0\t100000000.0\t0.0\t{B}\t0\t0\n\
             gate\t{GATE_B}\t16\t3532.0\t0.0\t-5000000000.0\t1000.0\t{B}\t{GATE_A}\t{A}\n\
             gate\t{GATE_OUT}\t17\t2323.0\t0.0\t0.0\t7000000000.0\t{B}\t99\t9\n"
        ))
    }

    fn world_with(constellation_b: i64) -> Registry {
        let pilot = |id, name: &str, ship| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice", ALICE_SHIP),
                pilot(BOB, "Bob", BOB_SHIP),
            ],
            celestials: catalogue(constellation_b),
            ..crate::model::SandboxConfig::default()
        });
        for (client, who) in [(1, ALICE), (2, BOB)] {
            reg.session_opened(crate::session_table::SessionEntry {
                client_id: client,
                session_id: client,
                user_id: client,
                character_id: Some(who),
            });
        }
        reg
    }

    fn world() -> Registry {
        world_with(10)
    }

    /// Alice in space in her home system, nothing queued.
    fn undocked(reg: &mut Registry, who: i64) {
        crate::destiny::on_ballpark_bind(reg, Some(who));
        let _ = reg.take_pending();
    }

    fn jump(reg: &mut Registry, args: &[PyValue]) -> Result<PyValue, ServiceError> {
        cmd_stargate_jump(
            reg,
            &Request {
                service: "beyonce",
                method: "CmdStargateJump",
                args,
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        )
    }

    fn alice_jumps(reg: &mut Registry) -> Result<PyValue, ServiceError> {
        jump(
            reg,
            &[
                PyValue::Int(GATE_A),
                PyValue::Int(GATE_B),
                PyValue::Int(ALICE_SHIP),
            ],
        )
    }

    /// The one action in a `DoDestinyUpdate` push: `(name, operands)`.
    fn action(p: &dyn Push) -> (String, Vec<PyValue>) {
        let (event, args) = p.to_push();
        assert_eq!(event, "DoDestinyUpdate");
        let Some(PyValue::List(entries)) = args.first() else {
            panic!("no entry list: {args:?}")
        };
        let PyValue::Tuple(entry) = &entries[0] else {
            panic!("not an entry")
        };
        let PyValue::Tuple(named) = &entry[1] else {
            panic!("not a named action")
        };
        let (Some(PyValue::Bytes(name)), Some(PyValue::Tuple(ops))) = (named.first(), named.get(1))
        else {
            panic!("not (name, args)")
        };
        (String::from_utf8_lossy(name).into_owned(), ops.clone())
    }

    /// A dict's value by `str` key; `PyValue::Dict` is `(value, key)`.
    fn key<'a>(dict: &'a PyValue, name: &str) -> Option<&'a PyValue> {
        let PyValue::Dict(pairs) = dict else {
            return None;
        };
        pairs
            .iter()
            .find(|(_, k)| *k == crate::pyutil::bstr(name))
            .map(|(v, _)| v)
    }

    /// Every `foo.SlimItem`'s attribute dict in a pushed state.
    fn slim_dicts(v: &PyValue, out: &mut Vec<PyValue>) {
        match v {
            PyValue::Instance { class, state } if class.as_slice() == b"foo.SlimItem" => {
                out.push((**state).clone())
            }
            PyValue::Instance { state, .. } => slim_dicts(state, out),
            PyValue::Tuple(xs) | PyValue::List(xs) => xs.iter().for_each(|x| slim_dicts(x, out)),
            PyValue::Dict(kv) => kv.iter().for_each(|(v, _)| slim_dicts(v, out)),
            _ => {}
        }
    }

    fn slim_of(state: &crate::destiny::Bootstrap, item: i64) -> Option<PyValue> {
        let (_, args) = state.to_push();
        let mut all = Vec::new();
        args.iter().for_each(|a| slim_dicts(a, &mut all));
        all.into_iter()
            .find(|d| key(d, "itemID") == Some(&PyValue::Int(item)))
    }

    /// The change as `{key: (old, new)}`, keys as strings.
    fn change_map(change: &[(PyValue, PyValue)]) -> BTreeMap<String, (i64, i64)> {
        change
            .iter()
            .map(|(v, k)| {
                let PyValue::Bytes(k) = k else {
                    panic!("a key is a str: {k:?}")
                };
                let PyValue::Tuple(pair) = v else {
                    panic!("a value is a pair: {v:?}")
                };
                let (PyValue::Int(a), PyValue::Int(b)) = (&pair[0], &pair[1]) else {
                    panic!("a pair of ints: {pair:?}")
                };
                (String::from_utf8_lossy(k).into_owned(), (*a, *b))
            })
            .collect()
    }

    /// Drive a whole jump to the destination's bind; returns nothing queued.
    fn through(reg: &mut Registry) {
        alice_jumps(reg).expect("a legal jump");
        let _ = reg.take_pending();
        let late = Instant::now() + JUMP_DELAY + Duration::from_millis(50);
        let _ = reg.take_due(late);
        tick(reg, late);
        let _ = reg.take_session_changes();
        crate::destiny::on_ballpark_bind(reg, Some(ALICE));
    }

    /// A jump's arrival seats its state past every stamp already handed out,
    /// so a stale push decided before the leg was armed cannot share its entry.
    #[test]
    fn a_jump_arrival_state_is_stamped_past_a_stale_push() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        alice_jumps(&mut reg).expect("a legal jump");
        let _ = reg.take_pending();
        let late = Instant::now() + JUMP_DELAY + Duration::from_millis(50);
        let _ = reg.take_due(late);
        tick(&mut reg, late);
        let _ = reg.take_session_changes();
        // A push decided before the leg was armed, stamped at the newest.
        let stale = reg.next_destiny_stamp();
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        assert!(
            reg.last_destiny_stamp_for_test() > stale,
            "the arrival's SetState opens its own entry"
        );
    }

    /// **The arrival's state is the destination gate's grid** (GRID-B,
    /// `docs/lanes/grid-research.md` §5 "Jump"): the gate and the far
    /// system's planet are in it, a pilot 1 AU from the gate is not, and
    /// once he flies up to the gate the reconcile adds him.
    #[test]
    fn jump_arrival_state_is_the_gate_grid() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                BOB,
                BOB_SHIP,
                B,
                crate::space_state::Motion::arrived([1.5e11, 0.0, 0.0], [0.0; 3]),
            ));
        through(&mut reg);
        let _ = reg.take_pending();
        let park = |reg: &Registry| {
            reg.ext::<crate::grid::Parks>()
                .of(ALICE)
                .cloned()
                .expect("the arrival's state is modelled")
        };
        let held = park(&reg);
        for id in [GATE_B, 22, ALICE_SHIP] {
            assert!(held.contains(&id), "{id} is in the arrival's state");
        }
        assert!(!held.contains(&BOB_SHIP), "Bob is 1 AU from the gate");
        assert!(!held.contains(&GATE_A), "nothing of the origin");

        let gate = reg.celestials().get(GATE_B).expect("the gate").position;
        let p = reg.roster_mut().get_mut(BOB).expect("in B");
        p.motion =
            crate::space_state::Motion::arrived([gate[0] + 20_000.0, gate[1], gate[2]], [0.0; 3]);
        p.since = crate::space_state::now_seconds();
        crate::grid::reconcile_in(&mut reg, B);
        let _ = reg.take_due(Instant::now() + Duration::from_secs(1));
        assert!(park(&reg).contains(&BOB_SHIP), "now on the gate's grid");
    }

    /// **1. A gate's slim carries a `jumps` row that unpacks as
    /// `(toCelestialID, locationID)`** -- two `I8` columns, in that order,
    /// because `subwaySvc.py:82` unpacks it by position and two other
    /// consumers read it by name. A ship's slim has no `jumps` key at all.
    #[test]
    fn a_gate_slim_carries_a_jumps_row_and_a_ship_slim_does_not() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        let state = crate::destiny::bootstrap_for(&mut reg, ALICE).expect("a state");
        assert!(state.every_slim_has_a_ball());
        let gate = slim_of(&state, GATE_A).expect("the gate is in the state");
        let Some(PyValue::List(rows)) = key(&gate, "jumps") else {
            panic!("jumps is a list: {gate:?}")
        };
        assert_eq!(rows.len(), 1, "jumps[0] is read with no guard");
        let PyValue::DbRow(row) = &rows[0] else {
            panic!("a DBRow: {:?}", rows[0])
        };
        assert_eq!(
            row.descriptor().names(),
            vec![b"toCelestialID".to_vec(), b"locationID".to_vec()]
        );
        assert_eq!(
            row.values(),
            &[
                eve_marshal::DbValue::Int(GATE_B),
                eve_marshal::DbValue::Int(B)
            ]
        );
        let ship = slim_of(&state, ALICE_SHIP).expect("the ship's slim");
        assert_eq!(key(&ship, "jumps"), None, "a ship's slim is unchanged");
        // `StargateSetup` indexes these when the slim has the attribute at
        // all (a missing one reads `None`): they are there, as `(stage, max)`
        // pairs (FCI-FIX2, the real client failed to add every gate).
        for name in [
            "destinationCorruptionStageAndMaximum",
            "localCorruptionStageAndMaximum",
            "destinationSuppressionStageAndMaximum",
            "localSuppressionStageAndMaximum",
        ] {
            assert!(
                matches!(key(&gate, name), Some(PyValue::Tuple(t)) if t.len() == 2),
                "{name}: {gate:?}"
            );
        }
    }

    /// **2 and 3. Each system's park is its own**: A's gate and planet in
    /// A, B's in B, and the home station's ball only at home -- in B it
    /// would be a trade post inside the sun.
    #[test]
    fn each_park_holds_its_own_system_and_the_station_only_at_home() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        let station = reg
            .character(ALICE)
            .and_then(|c| c.station_id)
            .expect("a station");
        let ids = |s: &crate::destiny::Bootstrap| -> Vec<i64> {
            s.park.balls.iter().map(|b| b.ball_id).collect()
        };
        let home = crate::destiny::bootstrap_for(&mut reg, ALICE).expect("a state");
        let home = ids(&home);
        assert!(home.contains(&12) && home.contains(&GATE_A) && home.contains(&station));
        assert!(!home.contains(&22) && !home.contains(&GATE_B) && !home.contains(&GATE_OUT));

        through(&mut reg);
        let away = crate::destiny::bootstrap_for(&mut reg, ALICE).expect("a state");
        assert!(away.every_slim_has_a_ball());
        let away = ids(&away);
        assert!(away.contains(&22) && away.contains(&GATE_B) && away.contains(&GATE_OUT));
        assert!(!away.contains(&12) && !away.contains(&GATE_A));
        assert!(!away.contains(&station), "no station outside its system");
        assert!(away.contains(&ALICE_SHIP));
    }

    /// **4. The jump's session change is never in the call's own reply.**
    /// The dispatcher would put it in front of the answer and every push
    /// behind it -- the wrong order (`docs/lanes/stargates.md` §2.2).
    #[test]
    fn the_arm_queues_no_session_change_and_answers_none() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        assert_eq!(alice_jumps(&mut reg).expect("a legal jump"), PyValue::None);
        assert!(reg.take_session_changes().is_empty());
    }

    /// **5. Order on the clock**: the FX with the answer, the removal
    /// strictly later, the session change strictly after the removal is due,
    /// and only once.
    #[test]
    fn fx_then_removal_then_session_change_in_that_order() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        undocked(&mut reg, BOB);
        // What Alice's own park holds before she leaves: all of it goes in her
        // copy of the removal, ego last (S8); Bob's copy is the ship alone.
        let mut dropped: Vec<PyValue> = reg
            .ext::<crate::grid::Parks>()
            .of(ALICE)
            .map(|p| {
                p.iter()
                    .copied()
                    .filter(|b| *b != ALICE_SHIP)
                    .map(PyValue::Int)
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            dropped.contains(&PyValue::Int(GATE_A)),
            "her park is modelled and holds the gate: {dropped:?}"
        );
        dropped.push(PyValue::Int(ALICE_SHIP));
        let before = Instant::now();
        alice_jumps(&mut reg).expect("a legal jump");
        let after = Instant::now();

        let now: Vec<(String, Vec<i64>)> = reg
            .take_pending()
            .iter()
            .map(|(_, p)| (action(p.as_ref()).0, p.audience().unwrap_or_default()))
            .collect();
        assert!(
            now.contains(&("OnSpecialFX".to_owned(), vec![ALICE, BOB])),
            "the FX goes with the answer, to the jumper and the observer: {now:?}"
        );
        assert!(
            !now.iter().any(|(n, _)| n == "RemoveBalls"),
            "nothing removed yet: {now:?}"
        );
        assert!(reg.take_due(before + Duration::from_millis(1)).is_empty());
        tick(&mut reg, before + Duration::from_millis(1));
        assert!(reg.take_session_changes().is_empty());

        // The removal's moment: due now, and still no session change.
        let removal = after + REMOVE_AFTER;
        let due = reg.take_due(removal);
        let removed: Vec<(String, Vec<PyValue>, Vec<i64>)> = due
            .iter()
            .map(|p| {
                let (n, ops) = action(p.as_ref());
                (n, ops, p.audience().unwrap_or_default())
            })
            .collect();
        assert_eq!(
            removed,
            vec![
                (
                    "RemoveBalls".to_owned(),
                    vec![PyValue::List(dropped)],
                    vec![ALICE]
                ),
                (
                    "RemoveBalls".to_owned(),
                    vec![PyValue::List(vec![PyValue::Int(ALICE_SHIP)])],
                    vec![BOB]
                )
            ],
            "the jumper's own park empties with her ship last; the observer loses the ship alone"
        );
        tick(&mut reg, removal);
        assert!(
            reg.take_session_changes().is_empty(),
            "the removal has to be applied before the change arrives"
        );

        let change_at = after + JUMP_DELAY;
        tick(&mut reg, change_at);
        let changes = reg.take_session_changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, ALICE, "the jumper's, not the observer's");
        tick(&mut reg, change_at + Duration::from_secs(5));
        assert!(reg.take_session_changes().is_empty(), "only once");
    }

    /// **6. The change has exactly the keys that move**, in both directions
    /// of the constellation question.
    #[test]
    fn the_change_carries_only_what_moves() {
        for (constellation_b, moves) in [(10, false), (20, true)] {
            let mut reg = world_with(constellation_b);
            undocked(&mut reg, ALICE);
            alice_jumps(&mut reg).expect("a legal jump");
            tick(&mut reg, Instant::now() + JUMP_DELAY);
            let changes = reg.take_session_changes();
            let map = change_map(&changes[0].1);
            assert_eq!(map.get("solarsystemid"), Some(&(A, B)));
            assert_eq!(map.get("solarsystemid2"), Some(&(A, B)));
            for absent in ["locationid", "shipid", "stationid", "regionid"] {
                assert!(!map.contains_key(absent), "{absent} in {map:?}");
            }
            assert_eq!(
                map.get("constellationid").copied(),
                moves.then_some((10, 20)),
                "{map:?}"
            );
            assert_eq!(map.len(), if moves { 3 } else { 2 });
        }
    }

    /// **7. `JumpFx` is fourteen positionals**: `targetID` the origin gate,
    /// the guid `effects.JumpOut`, `graphicInfo[0]` the destination system.
    /// Thirteen would shift `graphicInfo` into `timeFromStart` and the
    /// tunnel would have nowhere to go.
    #[test]
    fn the_fx_has_fourteen_arguments_and_the_destination_last() {
        let fx = JumpFx {
            to: vec![ALICE],
            stamp: 7,
            ship: ALICE_SHIP,
            gate: GATE_A,
            dest_system: B,
        };
        let (name, ops) = action(&fx);
        assert_eq!(name, "OnSpecialFX");
        assert_eq!(ops.len(), 14);
        assert_eq!(ops[0], PyValue::Int(ALICE_SHIP));
        assert_eq!(ops[3], PyValue::Int(GATE_A), "targetID is the origin gate");
        assert_eq!(ops[5], PyValue::Bytes(b"effects.JumpOut".to_vec()));
        assert_eq!(ops[13], PyValue::Tuple(vec![PyValue::Int(B)]));
    }

    /// **8. Arrival is a legal place to jump back from**: surface distance
    /// to the gate under `maxStargateJumpingDistance`, and never inside the
    /// gate, for every gate type in the fixture and the first five arrivals.
    #[test]
    fn every_arrival_is_within_jump_range_and_outside_the_gate() {
        let c = catalogue(10);
        for id in [GATE_A, GATE_B, GATE_OUT] {
            let gate = c.get(id).expect("a gate");
            for index in 0..5 {
                let at = arrival_position(gate, index);
                let surface = crate::space_state::distance(at, gate.position) - gate.radius;
                assert!(
                    surface > 0.0 && surface < MAX_JUMP_DISTANCE,
                    "gate {id} arrival {index}: {surface} m from the surface"
                );
            }
        }
    }

    /// **9. The roster moves; the character does not.** The station is
    /// still in the home system, and a relog says so.
    #[test]
    fn the_presence_moves_and_the_character_stays_home() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        through(&mut reg);
        let p = reg.roster().get(ALICE).copied().expect("in space again");
        assert_eq!(p.solar_system_id, B);
        assert_eq!(p.ship_id, ALICE_SHIP);
        assert_eq!(
            reg.character(ALICE).and_then(|c| c.solar_system_id),
            Some(A)
        );
        assert!(!in_transit(&reg, ALICE), "the bind consumed the arrival");
        let gate = *reg.celestials().get(GATE_B).expect("gate B");
        let surface = crate::space_state::distance(p.motion.position, gate.position) - gate.radius;
        assert!(surface > 0.0 && surface < MAX_JUMP_DISTANCE, "{surface}");
    }

    /// **10. In transit, nothing re-seeds the jumper** -- not at the home
    /// mouth, not anywhere -- from the click until the destination's bind.
    #[test]
    fn no_state_is_built_for_a_pilot_between_systems() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        alice_jumps(&mut reg).expect("a legal jump");
        assert!(in_transit(&reg, ALICE));
        assert!(crate::destiny::bootstrap_for(&mut reg, ALICE).is_none());
        assert!(!reg.roster().is_in_space(ALICE), "and nothing was seeded");
        tick(&mut reg, Instant::now() + JUMP_DELAY);
        assert!(crate::destiny::bootstrap_for(&mut reg, ALICE).is_none());
        assert!(!reg.roster().is_in_space(ALICE));
    }

    /// **A jumper is not docked** at any point of the jump: not in the
    /// tunnel, where they are in neither the roster nor a station, and not
    /// on the far side. `Character::station_id` never moves, so it cannot
    /// say this; `Registry::docked_station` is the reader that can
    /// (`docs/lanes/docked.md`).
    #[test]
    fn a_pilot_is_never_docked_from_undock_through_the_far_gate() {
        let mut reg = world();
        let home = reg.character(ALICE).and_then(|c| c.station_id);
        assert!(home.is_some());
        assert_eq!(reg.docked_station(ALICE), home, "docked before undock");
        undocked(&mut reg, ALICE);
        assert_eq!(reg.docked_station(ALICE), None, "in space");
        alice_jumps(&mut reg).expect("a legal jump");
        assert!(!reg.roster().is_in_space(ALICE));
        assert_eq!(reg.docked_station(ALICE), None, "in the tunnel");
        let _ = reg.take_pending();
        let late = Instant::now() + JUMP_DELAY + Duration::from_millis(50);
        let _ = reg.take_due(late);
        tick(&mut reg, late);
        let _ = reg.take_session_changes();
        assert_eq!(
            reg.docked_station(ALICE),
            None,
            "session changed, not bound"
        );
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        assert_eq!(reg.docked_station(ALICE), None, "on the far side");
        assert_eq!(
            reg.character(ALICE).and_then(|c| c.station_id),
            home,
            "\"my station\" never moved"
        );
    }

    /// A jump inside the undock window keeps the ship id, so the invuln tick
    /// would never end the protection: the jump arrival ends it itself.
    #[test]
    fn a_jump_inside_the_undock_window_ends_the_undock_protection() {
        let mut reg = world();
        crate::invuln::set_enabled(&mut reg, true);
        undocked(&mut reg, ALICE);
        let now = crate::pyutil::filetime_now();
        assert!(crate::invuln::character_protected(&reg, ALICE, now));
        alice_jumps(&mut reg).expect("a legal jump");
        let late = Instant::now() + JUMP_DELAY + Duration::from_millis(50);
        let _ = reg.take_due(late);
        tick(&mut reg, late);
        let _ = reg.take_session_changes();
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let now = crate::pyutil::filetime_now();
        assert!(
            !crate::invuln::character_protected(&reg, ALICE, now),
            "cloaked by the jump, not invulnerable too"
        );
    }

    /// `Registry::session_system` names what the client's
    /// `session.solarsystemid2` has through two jumps: the origin until the
    /// session change, the destination after it, and the roster in space.
    #[test]
    fn session_system_follows_the_session_through_two_jumps() {
        let mut reg = world();
        assert_eq!(reg.session_system(ALICE), Some(A), "docked");
        undocked(&mut reg, ALICE);
        assert_eq!(reg.session_system(ALICE), Some(A), "undocked");
        // A -> B.
        alice_jumps(&mut reg).expect("a legal jump");
        assert_eq!(reg.session_system(ALICE), Some(A), "before the change");
        let late = Instant::now() + JUMP_DELAY + Duration::from_millis(50);
        let _ = reg.take_due(late);
        tick(&mut reg, late);
        let _ = reg.take_session_changes();
        assert_eq!(reg.session_system(ALICE), Some(B), "after the change");
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        assert_eq!(reg.session_system(ALICE), Some(B), "bound: the roster");
        // B -> A: until the change the session is still B (the home is A).
        let ship = PyValue::Int(ALICE_SHIP);
        jump(
            &mut reg,
            &[PyValue::Int(GATE_B), PyValue::Int(GATE_A), ship],
        )
        .expect("back through B's gate");
        assert_eq!(
            reg.session_system(ALICE),
            Some(B),
            "departing B: the origin, not the home"
        );
        let later = late + JUMP_DELAY + Duration::from_millis(50);
        let _ = reg.take_due(later);
        tick(&mut reg, later);
        let _ = reg.take_session_changes();
        assert_eq!(reg.session_system(ALICE), Some(A), "after the change");
        assert_eq!(
            reg.character(ALICE).and_then(|c| c.solar_system_id),
            Some(A),
            "\"my station's system\" never moved"
        );
    }

    /// **11. Every refusal raises and changes nothing.**
    #[test]
    fn refusals_raise_and_leave_the_world_alone() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        let ship = PyValue::Int(ALICE_SHIP);
        let cases: Vec<(&str, Vec<PyValue>)> = vec![
            (
                "a gate in another system",
                vec![PyValue::Int(GATE_B), PyValue::Int(GATE_A), ship.clone()],
            ),
            (
                "a planet",
                vec![PyValue::Int(12), PyValue::Int(GATE_B), ship.clone()],
            ),
            (
                "the wrong beacon",
                vec![PyValue::Int(GATE_A), PyValue::Int(GATE_OUT), ship.clone()],
            ),
            (
                "somebody else's ship",
                vec![
                    PyValue::Int(GATE_A),
                    PyValue::Int(GATE_B),
                    PyValue::Int(BOB_SHIP),
                ],
            ),
            ("no arguments", vec![]),
        ];
        let roster = reg.roster().clone();
        for (what, args) in cases {
            let got = jump(&mut reg, &args);
            assert!(
                matches!(got, Err(ServiceError::Raise(_))),
                "{what}: {got:?}"
            );
            assert_eq!(reg.roster(), &roster, "{what}");
            assert!(reg.jumps().is_empty(), "{what}");
            assert!(reg.take_pending().is_empty(), "{what}");
        }

        // An unloaded destination, from B's far gate.
        through(&mut reg);
        let _ = reg.take_pending();
        let roster = reg.roster().clone();
        let got = jump(
            &mut reg,
            &[PyValue::Int(GATE_OUT), PyValue::Int(99), ship.clone()],
        );
        assert!(matches!(got, Err(ServiceError::Raise(_))), "{got:?}");
        assert_eq!(reg.roster(), &roster);
        assert!(reg.jumps().is_empty());

        // A second jump while one is in flight: the first is untouched.
        let back = [PyValue::Int(GATE_B), PyValue::Int(GATE_A), ship];
        jump(&mut reg, &back).expect("back through B's gate");
        let _ = reg.take_pending();
        let got = jump(&mut reg, &back);
        assert!(matches!(got, Err(ServiceError::Raise(_))), "{got:?}");
        assert_eq!(reg.jumps().len(), 1);
        assert!(reg.take_pending().is_empty());
    }

    /// **12. A disconnect mid-jump leaves no entry** after the next tick, and
    /// the next login is docked at home with nothing in space.
    #[test]
    fn a_pilot_who_disconnects_in_the_tunnel_leaves_nothing() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        alice_jumps(&mut reg).expect("a legal jump");
        reg.session_closed(1);
        tick(&mut reg, Instant::now());
        assert!(reg.jumps().is_empty());
        assert!(!reg.roster().is_in_space(ALICE));
        assert!(reg.take_session_changes().is_empty());
    }

    /// The destination's pilots are told about the arriver, at the gate --
    /// the `AddBalls2` half of `on_ballpark_bind`, reached from a jump.
    #[test]
    fn a_pilot_already_in_the_destination_sees_the_arrival() {
        let mut reg = world();
        undocked(&mut reg, BOB);
        undocked(&mut reg, ALICE);
        // Bob goes first, then Alice follows him through.
        let _ = cmd_stargate_jump(
            &mut reg,
            &Request {
                service: "beyonce",
                method: "CmdStargateJump",
                args: &[
                    PyValue::Int(GATE_A),
                    PyValue::Int(GATE_B),
                    PyValue::Int(BOB_SHIP),
                ],
                kwargs: &[],
                bind_params: None,
                actor: Some(BOB),
            },
        )
        .expect("Bob jumps");
        let late = Instant::now() + JUMP_DELAY;
        tick(&mut reg, late);
        let _ = reg.take_session_changes();
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        let _ = reg.take_pending();

        through(&mut reg);
        let to_bob: Vec<String> = reg
            .take_pending()
            .iter()
            .filter(|(_, p)| p.audience() == Some(vec![BOB]))
            .map(|(_, p)| action(p.as_ref()).0)
            .collect();
        assert_eq!(to_bob, vec!["AddBalls2".to_owned()]);
        let (a, b) = (
            reg.roster().get(ALICE).expect("Alice").motion.position,
            reg.roster().get(BOB).expect("Bob").motion.position,
        );
        assert!(
            crate::space_state::distance(a, b) > 1.0,
            "a berth each, not one spot for two ships"
        );
    }
    // ---- jump-damage lane: health and capacitor survive the tunnel ----

    /// A record for Alice's ship with the armour at or under 40 %.
    fn hurt_alice(reg: &mut Registry) {
        let h = crate::destiny::hull::of_in(reg.content(), 587);
        let profile = crate::damage::Profile::from_hull_in(reg.content(), h);
        let now = crate::pyutil::filetime_now();
        let mut rec = crate::damage::ShipDamage::full(profile, now);
        let volley = crate::damage::Damage {
            em: 20.0,
            explosive: 20.0,
            kinetic: 20.0,
            thermal: 20.0,
        };
        while rec.bars(now).armour > 0.4 && !rec.is_destroyed() {
            rec.apply(volley, now);
        }
        assert!(rec.bars(now).armour < 1.0);
        reg.combat_mut().set_ship(ALICE_SHIP, rec);
    }

    /// One attribute of the primed ship row, by id.
    fn primed(reg: &Registry, attr_id: i64) -> f64 {
        let c = reg.character(ALICE).expect("alice").clone();
        let row = reg.capsule_dogma_row(&c);
        let attrs = key_of_instance(&row, "attributes").expect("attributes");
        let PyValue::Dict(pairs) = attrs else {
            panic!("a dict")
        };
        match pairs.iter().find(|(_, k)| *k == PyValue::Int(attr_id)) {
            Some((PyValue::Real(v), _)) => *v,
            other => panic!("attribute {attr_id}: {other:?}"),
        }
    }

    fn key_of_instance(row: &PyValue, name: &str) -> Option<PyValue> {
        match row {
            PyValue::Instance { state, .. } => key(state, name).cloned(),
            other => key(other, name).cloned(),
        }
    }

    /// **A gate jump is not a repair.** The record is there at every tick of
    /// the tunnel, the bars on arrival are the bars that left, and the
    /// primed rings agree with them.
    #[test]
    fn health_survives_a_gate_jump() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        hurt_alice(&mut reg);
        let before = crate::combat::damage_state(&reg, ALICE_SHIP, crate::pyutil::filetime_now());
        alice_jumps(&mut reg).expect("a legal jump");
        let _ = reg.take_pending();
        for _ in 0..5 {
            crate::combat::tick(&mut reg, crate::pyutil::filetime_now());
            assert!(
                reg.combat().ship(ALICE_SHIP).is_some(),
                "kept in the tunnel"
            );
        }
        let late = Instant::now() + JUMP_DELAY + Duration::from_millis(50);
        let _ = reg.take_due(late);
        tick(&mut reg, late);
        let _ = reg.take_session_changes();
        crate::combat::tick(&mut reg, crate::pyutil::filetime_now());
        assert!(
            reg.combat().ship(ALICE_SHIP).is_some(),
            "kept at the change"
        );
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::combat::tick(&mut reg, crate::pyutil::filetime_now());
        let now = crate::pyutil::filetime_now();
        let after = crate::combat::damage_state(&reg, ALICE_SHIP, now);
        assert_eq!(after.armour, before.armour);
        assert_eq!(after.hull, before.hull);
        assert!(after.armour < 0.5);
        let rings = reg.combat().ship(ALICE_SHIP).expect("kept").rings(now);
        assert_eq!(primed(&reg, 266), rings.armor_damage);
        assert!(primed(&reg, 266) > 0.0, "the prime is not full");
        assert_eq!(primed(&reg, 3), rings.damage);
    }

    /// **The capacitor rides along**: a drained ship primes drained.
    #[test]
    fn capacitor_survives_a_gate_jump() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        let full = primed(&reg, 18);
        assert!(full > 0.0);
        reg.modules_mut()
            .set_capacitor(ALICE_SHIP, 0.0, crate::pyutil::filetime_now());
        through(&mut reg);
        crate::combat::tick(&mut reg, crate::pyutil::filetime_now());
        let cap = primed(&reg, 18);
        assert!(cap < full * 0.5, "{cap} of {full}");
    }

    /// A wormhole or jump-drive jump uses the same legs (`arm_at`), so the
    /// same rule keeps the record.
    #[test]
    fn health_survives_an_armed_point_jump() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        hurt_alice(&mut reg);
        reg.roster_mut().leave(ALICE);
        arm_at(&mut reg, ALICE, A, B, [0.0, 0.0, 0.0]);
        crate::combat::tick(&mut reg, crate::pyutil::filetime_now());
        assert!(reg.combat().ship(ALICE_SHIP).is_some());
    }

    /// **A jumper who disconnects in the tunnel loses the record** on the
    /// next tick after `jump::tick` drops the leg.
    #[test]
    fn a_pilot_who_disconnects_in_the_tunnel_loses_the_record() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        hurt_alice(&mut reg);
        alice_jumps(&mut reg).expect("a legal jump");
        crate::combat::tick(&mut reg, crate::pyutil::filetime_now());
        assert!(reg.combat().ship(ALICE_SHIP).is_some());
        reg.session_closed(1);
        tick(&mut reg, Instant::now());
        assert!(!in_transit(&reg, ALICE));
        crate::combat::tick(&mut reg, crate::pyutil::filetime_now());
        assert!(reg.combat().ship(ALICE_SHIP).is_none());
    }

    /// Someone else's record is not held by Alice's jump, and a ship in
    /// space is not affected.
    #[test]
    fn a_stranger_ship_out_of_space_is_still_forgotten() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        hurt_alice(&mut reg);
        let rec = *reg.combat().ship(ALICE_SHIP).expect("record");
        reg.combat_mut().set_ship(BOB_SHIP, rec);
        alice_jumps(&mut reg).expect("a legal jump");
        crate::combat::tick(&mut reg, crate::pyutil::filetime_now());
        assert!(reg.combat().ship(ALICE_SHIP).is_some());
        assert!(reg.combat().ship(BOB_SHIP).is_none(), "Bob is docked");
    }
}
