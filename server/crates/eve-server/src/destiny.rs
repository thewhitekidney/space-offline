//! Destiny: the ball state blob, and the state a ship undocks into.
//!
//! Round 2's entry point. The full spec is `docs/lanes/round2-spec.md`; this
//! module is §3 of it, and the parts worth repeating are the ones that decide
//! bytes.
//!
//! # What this is
//!
//! The client's space simulation is **client-side**: `destiny.Ballpark` is a
//! native object in `_destiny.dll` that evolves balls on the client's own
//! tick, and the server's job is to hand it a starting state and then send
//! only the *changes*. A parked ship therefore needs **no per-tick traffic at
//! all** -- which is the whole point of Destiny and the reason the minimum
//! viable undock is one notification and nothing else.
//!
//! That starting state is a byte blob that the native
//! `ReadFullStateFromStream` parses. We did not guess its layout: on
//! 2026-10-21 we drove the client's own `WriteFullStateToStream` over 64
//! parks, varying one argument at a time, and the result is
//! `findings/destiny-ref/` plus `docs/reference/destiny-reference-blob.md` §9. Every
//! offset below was measured twice -- once by varying an `AddBall` argument
//! and once by assigning the `destiny.Ball` attribute -- and the client's own
//! parser read our re-encoded bytes back unchanged. **[V]** throughout.
//!
//! # The four client rules this module exists to satisfy
//!
//! All four are reads of `eve/client/script/remote/michelle.pyc` in build
//! 3619879, and two of them contradict documents in this repo. The spec has
//! the disassembly; the summary is:
//!
//! 1. **`waitForBubble` must be false.** `Park.DoPreTick:1124` returns the
//!    moment it sees a waiting entry at the head of `history`, and the only
//!    thing that clears the flag is a later batch at the *exact same* stamp.
//!    EvEJS's `AddBalls2(waitForBubble=true)` then `SetState(stamp+1)` cannot
//!    work by that route; what rescues it is that `FlushState:1439` *deletes*
//!    every entry older than the newest `SetState`, so their first update is
//!    thrown away. Do not copy it.
//! 2. **The `SetState` must be the first entry of its batch.**
//!    `RealFlushState:1518` honours `SetState` only as `state[0]`; anything in
//!    front of it leaves `validState` at 0 and the batch logs `Events
//!    ignored`, which is a silent nothing and the hardest failure here to
//!    diagnose. So the bootstrap batch is exactly one entry.
//! 3. **One distinct stamp per batch**, or `michelle:666` calls
//!    `clientStatsSvc.OnFatalDesync()`.
//! 4. **`DoDestinyUpdate` is a `Do*` event**, which `dispatch::notify`'s doc
//!    comment otherwise forbids. It is safe here and the reason is checkable:
//!    the handler only logs, expands entries and appends to `Park.history`.
//!    Everything that could block -- the native parse, the prime wave, the
//!    scene work -- happens later on the client's own tick, out of
//!    `DoPreTick`.
//!
//! # Where the push goes out, and why not at the undock
//!
//! `michelle.DoDestinyUpdate` returns immediately while `self.__bp is None`,
//! and at `ship.Undock` time it **is** `None`: the ballpark is built about
//! three seconds later, from `OnPrimaryViewChanged`. A state pushed with the
//! undock reply is silently discarded.
//!
//! So the hook is the `beyonce.MachoBindObject` that follows, by which point
//! the `Park` exists with an empty history. See [`on_ballpark_bind`], which
//! `dispatch::pending_for_bound_call` calls.
//!
//! # What this module does not have
//!
//! No entity model. `Character` has a station and a system and no position,
//! velocity or undocked flag, so the ball below is built from `Character`
//! plus constants. A second ship in the same system, and therefore a roster
//! to hold it, is increment 2 -- and `docs/evejs/inventory/space-destiny.md` already
//! says that the entity model, not the encoder, is what our side actually
//! lacks.

use crate::pyutil::keyval;
use crate::service::Registry;
use eve_marshal::PyValue;

/// The encoders and the slim types (IP36-b): every item is re-exported, so
/// `crate::destiny::Park`, `crate::destiny::Slim` and the rest keep their
/// paths.
mod encode;
pub use encode::*;

/// Warp and the Destiny actions (IP36-c): `Action`, `warp_factor_in`,
/// `warp_trajectory`, `warp_flight_seconds` and `warp_arrival` are
/// re-exported, so `crate::destiny::Action` and the rest keep their paths.
mod warp;
pub use warp::*;

/// The push types (IP36-d): `StateUpdate`, `ActionUpdate`, `BallsAdded`,
/// `BallsRemoved`, `SpecialFx`, `DamageStateChange`, `Kill`,
/// `SlimItemChange`, `WreckSlim` and the rest are re-exported, so
/// `crate::destiny::Kill` and every other push keeps its path. Not the
/// crate-root `crate::push`.
mod push;
pub use push::*;

// ---------------------------------------------------------------------------
// The bootstrap state: what a ship undocks into.
// ---------------------------------------------------------------------------

/// The four hull numbers the ball record carries.
///
/// **From the client's own tables, not from constants.** `mass` and `radius`
/// are columns of `types.fsdbinary`; `maxVelocity` and `agility` are dogma
/// attributes **37** and **70** of `typedogma.fsdbinary`. That split is not a
/// guess -- `dogma/data.get_type_attributes` *injects* four synthesised
/// attributes into every type's list, `attributeMass` (4) from
/// `evetypes.GetMass` and `attributeRadius` (162) from `evetypes.GetRadius`,
/// and those two read the types-table columns through `typesLoader.pyd`. So
/// mass and radius are deliberately absent from `typedogma` because the
/// client puts them there itself. **[V]**
///
/// `GetRadius` is `float(radius or 1.0)`, which is where the native
/// initialiser below comes from on the data side.
///
/// # Why these nineteen
///
/// **A hull is here exactly when it is a
/// category-6 type in [`crate::item::SPAWNABLE`]** -- i.e. when a character
/// can actually come to be flying it. Shuttles are deliberately absent
/// because no shuttle is in `SPAWNABLE`; a row for a type
/// `item::find_type_in` does not know is a row nothing can reach.
/// `docs/lanes/hull-numbers.md` §2.1 lists the eleven published shuttles for
/// whoever adds them to `SPAWNABLE` first.
///
/// # The check that makes the table trustworthy
///
/// `mass` and `agility` come from two different files, and the client's own
/// `alignTime = -ln(0.25) * agility * mass / 1e6` is published for every hull
/// -- so evaluating it is a test that the two columns agree *with each other*
/// and with the outside world, not merely that each is individually
/// plausible. Rifter 4.73 s, Megathron 15.55 s, Tempest 16.00 s.
/// `tests::the_published_align_times_come_back_out_of_the_table` runs it.
///
/// Regeneration: `docs/lanes/hull-numbers.md` §3 has the command, which
/// prints `ABSENT` rather than any default and prints the attribute ids it
/// used.
pub mod hull;

/// The stamp a state is pushed at, and the clock written into the blob.
///
/// **Unix seconds.** Monotonic at 1 Hz, fits `u32` until 2106, and the same
/// choice EvEJS made (`delivery/stamps.js`, `floor(nowMs/1000) & 0x7fffffff`)
/// (**[V2]**).
///
/// **The first `SetState` cannot be got wrong by picking the wrong clock**,
/// which is why this is allowed to be a wall clock rather than something
/// negotiated with the client. `Park.DoPreTick:1130` defers a batch only when
/// its stamp is 1 or 2 ticks *ahead* of the client's ballpark clock, and a
/// deferral is harmless because the next tick retries; for every other value
/// it flushes at once, and `ReadFullStateFromStream` then overwrites the
/// client's `currentTime` from the blob's own header. **[V]**
///
/// That is **not** true of anything after the first state. A non-`SetState`
/// action whose stamp is far ahead makes `SynchroniseToSimulationTime:1415`
/// run `stamp - lastStamp` native `Evolve`s in a loop, inside a handler with
/// `block_trap` set -- which is the one way a push can wedge the client's
/// simulation. Movement has to clamp rather than trust; see
/// `docs/lanes/round2-spec.md` §4 increment 3.
///
/// A clock that goes backwards across a restart would be a problem for the
/// *second* state, not this one, and `SystemTime` before 1970 is the only way
/// to get a zero here.
pub fn tick() -> u32 {
    #[cfg(test)]
    let t = crate::test_clock::system_now();
    #[cfg(not(test))]
    let t = std::time::SystemTime::now();
    t.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as u32)
}

/// A whole `SetState` payload: the blob, the ego, and the slim items.
///
/// Named for what the client calls it. Every field below is a `LOAD_ATTR`
/// operand in `Park.SetState` (`michelle.py:1224-1282`) **[V]**, and the
/// fields that are *not* here are as deliberate as the ones that are -- see
/// [`Bootstrap::to_push`].
#[derive(Debug, Clone, PartialEq)]
pub struct Bootstrap {
    /// The action's stamp. **Must equal `park.current_time`**: the batch's
    /// stamp decides when the client flushes it, and the blob's header decides
    /// what the client's clock becomes when it does. Two different numbers
    /// means the park starts out of step with the actions that follow it.
    /// [`Bootstrap::new`] is the only constructor so that this cannot drift.
    pub stamp: u32,
    pub park: Park,
    /// `self.ego = long(bag.ego)`. The ship's **item** id, which is what
    /// `gameui` stored as `wannaBeEgo` from `session.shipid` **[V]** -- so a
    /// mismatch here is a camera that never attaches to anything.
    pub ego: i64,
    pub slims: Vec<Slim>,
    /// `(ballID, state)` per ball that has health, as [`DamageState`] builds
    /// it. A ball absent from here has **no gauges**, not full ones.
    pub damage: Vec<(i64, PyValue)>,
    /// The wrecks in the system, whose balls are in `park` like any other's;
    /// their slims carry three keys [`Slim`] has not ([`WreckSlim`]). Set
    /// with [`Bootstrap::with_wrecks`].
    pub wrecks: Vec<WreckSlim>,
    /// The system's stargates, whose balls are in `park`; their slims carry
    /// the `jumps` row [`Slim`] has not ([`GateSlim`]). Set with
    /// [`Bootstrap::with_gates`].
    pub gates: Vec<GateSlim>,
    /// The fitted modules of the ships among `slims` ([`SlimModule`]). Set
    /// with [`Bootstrap::with_modules`].
    pub modules: ShipModules,
    /// The drones in the system, as `droneState` rows
    /// ([`crate::drones::DroneRow`]). Set with [`Bootstrap::with_drones`].
    pub drones: Vec<crate::drones::DroneRow>,
    /// The system's customs offices, `(ballID, slim)`, whose balls are in
    /// `park`; the slim is built by [`crate::planet::CustomsOffice::slim`]
    /// (`docs/lanes/planet.md` §11). Set with [`Bootstrap::with_orbitals`].
    pub orbitals: Vec<(i64, PyValue)>,
    /// Extra attributes for slims among `slims`, by item id
    /// ([`crate::slim_extra`]). Set with [`Bootstrap::with_extras`].
    pub extras: SlimExtras,
    /// The effects running in the scene (`OnSpecialFX` starts, `to` empty):
    /// a late arrival's `effectStates` ([`crate::modules::active_fx`]). Set
    /// by [`scene`]; [`Self::retain_balls`] keeps those with both ends kept.
    pub fx: Vec<crate::modules::SpecialFx>,
    /// The ego pilot's `dbuffState` ([`crate::dbuff::state_value_of`]);
    /// `None` is an empty one.
    pub dbuff: Option<PyValue>,
}

/// Positions of a list's rows by key id, sorted by `(id, position)`: every
/// row of an id is kept, in list order, so a lookup answers "all" and "first".
#[derive(Debug, Default)]
pub struct Keyed(Vec<(i64, u32)>);

impl Keyed {
    fn new(ids: impl Iterator<Item = i64>) -> Self {
        let mut v: Vec<(i64, u32)> = ids.zip(0u32..).collect();
        v.sort_unstable();
        Keyed(v)
    }

    /// Every position holding `id`, ascending.
    fn all(&self, id: i64) -> impl Iterator<Item = u32> + '_ {
        let lo = self.0.partition_point(|(k, _)| *k < id);
        self.0[lo..]
            .iter()
            .take_while(move |(k, _)| *k == id)
            .map(|(_, p)| *p)
    }

    /// The first position holding `id` (what `Iterator::find` returns).
    fn first(&self, id: i64) -> Option<u32> {
        self.all(id).next()
    }
}

/// [`Bootstrap::add_index`]: the keyed positions of a scene's lists.
#[derive(Debug, Default)]
pub struct AddIndex {
    ball: Keyed,
    slim: Keyed,
    damage: Keyed,
    wreck: Keyed,
    module: Keyed,
    extra: Keyed,
}

impl Bootstrap {
    /// A state whose action stamp and blob clock are the same number.
    pub fn new(
        stamp: u32,
        mut park: Park,
        ego: i64,
        slims: Vec<Slim>,
        damage: Vec<(i64, PyValue)>,
    ) -> Self {
        park.current_time = stamp;
        Bootstrap {
            stamp,
            park,
            ego,
            slims,
            damage,
            wrecks: Vec::new(),
            gates: Vec::new(),
            modules: Vec::new(),
            drones: Vec::new(),
            orbitals: Vec::new(),
            extras: Vec::new(),
            fx: Vec::new(),
            dbuff: None,
        }
    }

    /// The same state with extra attributes on some of its [`Slim`]s
    /// ([`crate::slim_extra`]).
    pub fn with_extras(mut self, extras: SlimExtras) -> Self {
        self.extras = extras;
        self
    }

    /// The same state with the system's customs offices' slims; their balls
    /// must already be in the park ([`Self::every_slim_has_a_ball`]).
    pub fn with_orbitals(mut self, orbitals: Vec<(i64, PyValue)>) -> Self {
        self.orbitals = orbitals;
        self
    }

    /// The same state with the system's drones in `droneState`; their balls
    /// are in the park and their slims among `slims`.
    pub fn with_drones(mut self, drones: Vec<crate::drones::DroneRow>) -> Self {
        self.drones = drones;
        self
    }

    /// The same state with its ships' fitted modules, which go out as each
    /// ship slim's `modules` ([`SlimModule`]).
    pub fn with_modules(mut self, modules: ShipModules) -> Self {
        self.modules = modules;
        self
    }

    /// The same state with the system's gates' slims; their balls must
    /// already be in the park ([`Self::every_slim_has_a_ball`]).
    pub fn with_gates(mut self, gates: Vec<GateSlim>) -> Self {
        self.gates = gates;
        self
    }

    /// The same state with the system's wrecks' slims; their balls must
    /// already be in the park ([`Self::every_slim_has_a_ball`]).
    pub fn with_wrecks(mut self, wrecks: Vec<WreckSlim>) -> Self {
        self.wrecks = wrecks;
        self
    }

    /// Whether every slim item names a ball that exists.
    ///
    /// `Park.SetState` raises `RuntimeError('BallNotInPark', …)` for a
    /// `slimItem.itemID` the blob did not create **[V]**, which kills the
    /// whole state. Checked here rather than asserted, so the caller can
    /// refuse to send a state the client would reject.
    pub fn every_slim_has_a_ball(&self) -> bool {
        let has = |id: i64| self.park.balls.iter().any(|b| b.ball_id == id);
        self.slims.iter().all(|s| has(s.item_id))
            && self.wrecks.iter().all(|w| has(w.item_id))
            && self.gates.iter().all(|g| has(g.item_id))
            && self.orbitals.iter().all(|(id, _)| has(*id))
    }

    /// The same state with only the balls `keep` admits, and everything that
    /// rides on a ball -- slim, gauges, fitted modules, extras, drone row --
    /// going with it, so [`Self::every_slim_has_a_ball`] still holds.
    ///
    /// The ego is the caller's to keep: a state with no ego ball sets
    /// `validState` and then fails in the first `GetBall(ego)` read (see
    /// [`bootstrap_for`]).
    pub fn retain_balls(&mut self, keep: impl Fn(&i64) -> bool) {
        self.park.balls.retain(|b| keep(&b.ball_id));
        self.slims.retain(|s| keep(&s.item_id));
        self.damage.retain(|(id, _)| keep(id));
        self.wrecks.retain(|w| keep(&w.item_id));
        self.gates.retain(|g| keep(&g.item_id));
        self.modules.retain(|(id, _)| keep(id));
        // `droneState` is the drone window's "in local space" list
        // (`dronesUtil.GetDronesInLocalSpace:26-36`) [V]: a drone off this
        // grid is not in local space. **[A]** that CCP drops the row too.
        self.drones.retain(|d| keep(&d.drone_id));
        self.orbitals.retain(|(id, _)| keep(id));
        self.extras.retain(|(id, _)| keep(id));
        self.fx
            .retain(|f| keep(&f.ship_id) && f.target_id.is_none_or(|t| keep(&t)));
    }

    /// **`ids` out of this scene as one `AddBalls2`**, for `to`: what the
    /// grid's reconcile sends a pilot whose grid gained balls
    /// (`crate::grid`). Each ball carries what it carries in a `SetState` --
    /// a ship its gauges and modules, a rock or a site object its bare slim,
    /// a wreck its three wreck keys -- so a ball reads the same whichever
    /// way it reached the park.
    ///
    /// Only slim-bearing balls and wrecks: the global furniture (gates with
    /// their `jumps` row, customs offices) is in every park from the state
    /// on and is never added. An id this scene has no ball for is skipped.
    /// `None` when nothing is left.
    pub fn added(&self, to: Vec<i64>, stamp: u32, ids: &[i64]) -> Option<BallsAdded> {
        self.added_reference(to, stamp, ids)
    }

    /// The scan [`Bootstrap::added`] was before PA-7: O(slims x ids). Kept as
    /// the oracle [`Bootstrap::added_indexed`] is held to.
    pub fn added_reference(&self, to: Vec<i64>, stamp: u32, ids: &[i64]) -> Option<BallsAdded> {
        let ball = |id: i64| self.park.balls.iter().find(|b| b.ball_id == id).copied();
        let mut balls = Vec::new();
        let mut extra = Vec::new();
        let mut wrecks = Vec::new();
        // Slim-bearing balls first, then wrecks: the blob's balls in the same
        // order as the `extraBallData` entries that name them.
        for s in self.slims.iter().filter(|s| ids.contains(&s.item_id)) {
            let Some(b) = ball(s.item_id) else { continue };
            let damage = self
                .damage
                .iter()
                .find(|(id, _)| *id == s.item_id)
                .map(|(_, d)| d.clone());
            balls.push(b);
            extra.push((*s, damage));
        }
        for wr in self.wrecks.iter().filter(|w| ids.contains(&w.item_id)) {
            let Some(b) = ball(wr.item_id) else { continue };
            balls.push(b);
            wrecks.push(*wr);
        }
        if balls.is_empty() {
            return None;
        }
        let has = |id: &i64| balls.iter().any(|b| b.ball_id == *id);
        let modules = self
            .modules
            .iter()
            .filter(|(id, _)| has(id))
            .cloned()
            .collect();
        let extras = self
            .extras
            .iter()
            .filter(|(id, _)| has(id))
            .cloned()
            .collect();
        Some(BallsAdded {
            to,
            stamp,
            park: Park {
                current_time: stamp,
                balls,
            },
            extra,
            wrecks,
            modules,
            extras,
            fx: Vec::new(),
        })
    }

    /// [`Bootstrap::added`]'s answer from an [`AddIndex`] of this scene: the
    /// same `BallsAdded`, field for field, in O(k log k) for `k` ids instead
    /// of O(slims x k). One index serves every viewer of a reconcile.
    pub fn added_indexed(
        &self,
        ix: &AddIndex,
        to: Vec<i64>,
        stamp: u32,
        ids: &[i64],
    ) -> Option<BallsAdded> {
        // `contains` makes duplicates irrelevant.
        let mut want: Vec<i64> = ids.to_vec();
        want.sort_unstable();
        want.dedup();
        let ball = |id: i64| ix.ball.first(id).map(|p| self.park.balls[p as usize]);
        let mut balls = Vec::new();
        let mut extra = Vec::new();
        let mut wrecks = Vec::new();
        // The scan's order: slims in `slims` order, then wrecks in theirs.
        let mut at: Vec<u32> = want.iter().flat_map(|id| ix.slim.all(*id)).collect();
        at.sort_unstable();
        for p in at {
            let s = &self.slims[p as usize];
            let Some(b) = ball(s.item_id) else { continue };
            let damage = ix
                .damage
                .first(s.item_id)
                .map(|d| self.damage[d as usize].1.clone());
            balls.push(b);
            extra.push((*s, damage));
        }
        let mut at: Vec<u32> = want.iter().flat_map(|id| ix.wreck.all(*id)).collect();
        at.sort_unstable();
        for p in at {
            let wr = &self.wrecks[p as usize];
            let Some(b) = ball(wr.item_id) else { continue };
            balls.push(b);
            wrecks.push(*wr);
        }
        if balls.is_empty() {
            return None;
        }
        let mut has: Vec<i64> = balls.iter().map(|b| b.ball_id).collect();
        has.sort_unstable();
        has.dedup();
        let mut at: Vec<u32> = has.iter().flat_map(|id| ix.module.all(*id)).collect();
        at.sort_unstable();
        let modules = at
            .iter()
            .map(|p| self.modules[*p as usize].clone())
            .collect();
        let mut at: Vec<u32> = has.iter().flat_map(|id| ix.extra.all(*id)).collect();
        at.sort_unstable();
        let extras = at
            .iter()
            .map(|p| self.extras[*p as usize].clone())
            .collect();
        Some(BallsAdded {
            to,
            stamp,
            park: Park {
                current_time: stamp,
                balls,
            },
            extra,
            wrecks,
            modules,
            extras,
            fx: Vec::new(),
        })
    }

    /// The lookups [`Bootstrap::added_indexed`] needs, built once per scene.
    pub fn add_index(&self) -> AddIndex {
        AddIndex {
            ball: Keyed::new(self.park.balls.iter().map(|b| b.ball_id)),
            slim: Keyed::new(self.slims.iter().map(|s| s.item_id)),
            damage: Keyed::new(self.damage.iter().map(|(id, _)| *id)),
            wreck: Keyed::new(self.wrecks.iter().map(|w| w.item_id)),
            module: Keyed::new(self.modules.iter().map(|(id, _)| *id)),
            extra: Keyed::new(self.extras.iter().map(|(id, _)| *id)),
        }
    }

    /// `("DoDestinyUpdate", [state, waitForBubble])`.
    ///
    /// `state` is `[(stamp, ('SetState', (bag,)))]` -- **one entry**, because
    /// `RealFlushState` honours `SetState` only as `state[0]` and more than
    /// one distinct stamp in a batch calls `OnFatalDesync`.
    ///
    /// `waitForBubble` is `0`. See the module header, rule 1: a waiting entry
    /// at the head of `history` is never flushed by `DoPreTick`. The value
    /// reaches `['no','yes'][waitForBubble]`, so an int is right and anything
    /// that is not an index raises inside `DumpHistory`.
    ///
    /// **No third argument.** A truthy `dogmaMessages` makes
    /// `michelle.DoDestinyUpdate` scatter `OnMultiEvent`, which is a different
    /// mechanism with its own payload rules.
    ///
    /// The `bag` is a `utillib.KeyVal`: whitelisted, old-style, no
    /// `__setstate__`, so the state is an attribute dict **[V]**.
    ///
    /// Three attributes are **empty containers and must not be `None`** --
    /// `droneState` is iterated in a dict comprehension, `damageState` has
    /// `.iteritems()` called on it, `effectStates` is iterated -- and `None`
    /// raises in `SetState` before anything is drawn **[V]**.
    ///
    /// **`damageState` must not be *empty* either, and that cost a player a
    /// ship with no health bars.** The round-2 spec called an empty one "safe
    /// for drawing" because every reader guards it -- which was true and was
    /// the wrong question. `SetState` stores `self.damageState[k] = (v, now)`
    /// for each entry, `michelle.GetDamageState` returns **`None` for a ball
    /// with no entry** (`:1304`), and `baseTacticalEntry.SetDamageState:154`
    /// then does `if state[i] is None: gauge.state = UI_HIDDEN`. So no entry
    /// is not "full health", it is "no gauge" -- and the first real client to
    /// undock showed zero across shield, armour and hull. See [`DamageState`]
    /// for the shape, which has a second trap inside it. **[V]**
    ///
    /// Two are `None` on evidence rather than by omission. `solItem` is stored
    /// at `michelle:1275` and **never read**: a whole-tree search finds it in
    /// three modules, and the two that take it as an argument ignore it
    /// (`EvEJS sends an 11-column packed row here, which on this build is dead
    /// weight` -- **[V2]**). `dbuffState` falsy makes `OnDbuffUpdated` reset
    /// the incoming-dbuff tracker, which is the correct state for a ship
    /// nothing has shot.
    ///
    /// `industryLevel` and `researchLevel` feed one `in dict.keys()`
    /// membership test in `audioService`, so any integer is safe **[V]**.
    ///
    /// `aggressors` and `allianceBridges` are **omitted**: EvEJS sends `{}`
    /// for both (**[V2]**) and our build's `Park.SetState` never reads either
    /// **[V]**. Sending them would be harmless; omitting them is honest.
    pub fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let bag = keyval(vec![
            ("state", PyValue::Bytes(self.park.encode())),
            ("ego", PyValue::Int(self.ego)),
            (
                "slims",
                PyValue::List(
                    self.slims
                        .iter()
                        .map(|s| s.to_pyvalue_with(&self.modules, &self.extras))
                        .chain(self.wrecks.iter().map(|w| w.to_pyvalue()))
                        .chain(self.gates.iter().map(|g| g.to_pyvalue()))
                        .chain(self.orbitals.iter().map(|(_, s)| s.clone()))
                        .collect(),
                ),
            ),
            // **The drone window's "in space" list** comes from here and
            // from `OnDroneStateChange`, nowhere else
            // (`dronesUtil.GetDronesInLocalSpace:26-36`) [V]: a pilot who
            // undocks to find their drones still out must be told.
            (
                "droneState",
                PyValue::List(self.drones.iter().map(|d| d.to_pyvalue()).collect()),
            ),
            (
                "damageState",
                PyValue::Dict(
                    self.damage
                        .iter()
                        .map(|&(ball, ref state)| (state.clone(), PyValue::Int(ball)))
                        .collect(),
                ),
            ),
            // **Each tuple is `OnSpecialFX`'s positionals, at least nine**
            // (`michelle.py:973-975`, `OnSpecialFX(*t)` then
            // `ScatterEwars(*t)`): a shorter one raises inside `SetState`
            // and leaves the park half built **[V]**. `SpecialFx::to_push`
            // sends fourteen.
            (
                "effectStates",
                PyValue::List(
                    self.fx
                        .iter()
                        .map(|f| PyValue::Tuple(crate::notify::Push::to_push(f).1))
                        .collect(),
                ),
            ),
            ("solItem", PyValue::None),
            ("industryLevel", PyValue::Int(0)),
            ("researchLevel", PyValue::Int(0)),
            ("dbuffState", self.dbuff.clone().unwrap_or(PyValue::None)),
        ]);
        let action = PyValue::Tuple(vec![
            PyValue::Int(i64::from(self.stamp)),
            PyValue::Tuple(vec![
                crate::pyutil::bstr("SetState"),
                PyValue::Tuple(vec![bag]),
            ]),
        ]);
        (
            "DoDestinyUpdate",
            vec![PyValue::List(vec![action]), PyValue::Int(0)],
        )
    }
}

/// One ball's health, as `damageState[ballID]`.
///
/// **A 3-sequence `(shield, armour, hull)`, and the shield slot is a trap
/// that bit us once already.**
/// `eveInflight.damageStateValue.CalculateCurrentDamageStateValues(state, time)`
/// is the only decoder **[V]**:
///
/// ```text
///  10  shieldValues = damageState[0]
///  11  if isinstance(shieldValues, (list, tuple)):
///  12      now = blue.os.GetSimTime()
///  13      shieldHealth, tau = shieldValues[:2]        # SLICE+2, UNPACK_SEQUENCE 2
///  14      if tau > 0:
///  16          eToX = exp((time - now) / dogmaConst.dgmTauConstant / tau)
///  17          shieldHealth = (1.0 + (sqrt(shieldHealth) - 1.0) * eToX) ** 2
///  18      ret.append(shieldHealth)
///  20  else: ret.append(None)
///  21  ret = ret + list(damageState[-2:])
/// ```
///
/// So:
///
/// 1. **`(health, tau)` are the shield slot's first two elements**, and
///    anything after them is unread. This is `[:2]`, not `[2:]`: CPython
///    2.7's `SLICE+2` pops the *stop* bound. An earlier reading of this had
///    the slice inverted and so put the health in slot 2, where nothing reads
///    it, while the client read our zero in slot 0 -- **a shield bar that was
///    empty and red on an undamaged ship**, which a player found before any
///    test did.
/// 2. **The shield slot must be a list or tuple**, because anything else
///    appends `None` and `SetDamageState:154` hides that gauge. A bare `1.0`
///    shield is a *hidden* shield bar -- the failure that looks like data and
///    is a shape. Two elements suffice; three is what CCP's own producer
///    sends (below) and what we send.
/// 3. `tau = 0.0` skips the regeneration curve entirely: the `/tau` sits
///    *inside* the `tau > 0` guard, so zero is safe rather than a division by
///    zero, and the health passes through verbatim. That is right for a ship
///    nothing has shot ([`DamageState::FULL`]) and **wrong for a damaged
///    one**, whose bar would then never creep back up: a ship with a damage
///    record sends CCP's own `tau = shieldRechargeRate / 5.0`
///    ([`crate::damage::ShieldBar::tau_ms`], via `From<Bars>`), and the client
///    runs the curve from the moment it stores the entry.
///    (An earlier note justified the zero by saying our stamp is not the
///    client's sim clock. True, but moot: `michelle.Park.SetState:1264-1268`
///    stores `self.damageState[k] = (v, blue.os.GetSimTime())`, so **our
///    stamp never reaches this decoder at all** -- the client timestamps the
///    entry with its own clock as it reads it. **[V]**)
/// 4. Each value is **the fraction of the bar to fill**, clamped to
///    `[0.0, 1.0]` by `mathext.clamp` after `round(x, 3)` -- so `1.0` is full
///    and `0.0` is an empty bar rather than a hidden one. **[V]**, three
///    ways: CCP's producer computes `1.0 - damage/max`
///    (`baseDogmaLocation.GetDamageStateEx:1776`), `SetDamageState:161`
///    raises its `gotDmg` flag when a bar gets **narrower**, and
///    `progression.healthwidgets` feeds the same number into a container
///    named `health` as `healthPercent`.
///
/// **CCP's own producer**, for when we want real numbers rather than full
/// bars -- `baseDogmaLocation.GetDamageStateEx:1760-1784` **[V]**:
///
/// ```text
/// ((shieldCharge / shieldCapacity, shieldRechargeRate / 5.0, blue.os.GetSimTime()),
///  1.0 - armorDamage / armorHP,
///  1.0 - damage / hp)
/// ```
///
/// -- so the third shield element is the server's own timestamp, unread by
/// this decoder, and `tau` is the recharge rate in milliseconds over five.
/// Note `GetFullChargedInfo`'s zero branch at `:1766`: a hull whose
/// `shieldCapacity` is **0** gets a bare float in the shield slot, and so a
/// legitimately *hidden* shield bar.
///
/// One more thing consumers see: `michelle.GetDamageState:1314` appends
/// `state[-2:]` a **second** time, so what reaches a consumer is a **5-list**
/// `[shield, armour, hull, armour, hull]`. `healthwidgets` does
/// `UNPACK_SEQUENCE 5` on it, so the five is load-bearing rather than slop.
/// **[V]**
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageState {
    /// The shield fraction, or `None` for a hull with no shield at all --
    /// which CCP's producer sends as a **bare float**, hiding the bar
    /// (`GetDamageStateEx:1766`, point 2 above) **[V]**.
    pub shield: Option<f64>,
    /// `tau` in milliseconds: `shieldRechargeRate / 5.0`, or `0.0` to freeze
    /// the bar (point 3).
    pub shield_tau_ms: f64,
    pub armour: f64,
    pub hull: f64,
}

impl DamageState {
    /// Undamaged: every bar full, and frozen -- a full shield has nowhere to
    /// regenerate to, so `tau` has nothing to do. Every ship without a
    /// damage record is sent this (`crate::combat::damage_state`).
    pub const FULL: DamageState = DamageState {
        shield: Some(1.0),
        shield_tau_ms: 0.0,
        armour: 1.0,
        hull: 1.0,
    };

    /// The marshal form. See the type's own documentation for why the shield
    /// is a sequence and the other two are bare floats.
    pub fn to_pyvalue(self) -> PyValue {
        let shield = match self.shield {
            Some(fraction) => PyValue::Tuple(vec![
                // `[0]`: the shield health. **First**, because the decoder
                // takes `shieldValues[:2]`.
                PyValue::Real(fraction),
                // `[1]` `tau`, ms. Zero freezes the bar; CCP's own
                // `shieldRechargeRate / 5.0` makes the client regenerate it.
                PyValue::Real(self.shield_tau_ms),
                // `[2]`: unread here. CCP puts `blue.os.GetSimTime()` in it;
                // we send zero and match their arity rather than their value,
                // because a clock of ours has no meaning on their side -- the
                // client stamps the entry with its own clock on arrival
                // (`michelle.py:2076`) **[V]**.
                PyValue::Real(0.0),
            ]),
            // A hull with no shield: a bare float, so the gauge is hidden
            // rather than drawn empty. The value is unread.
            None => PyValue::Real(0.0),
        };
        PyValue::Tuple(vec![
            shield,
            PyValue::Real(self.armour),
            PyValue::Real(self.hull),
        ])
    }

    /// What the client will draw, given what we send: `[shield, armour,
    /// hull]` as `CalculateCurrentDamageStateValues` computes it.
    ///
    /// The decoder in ten lines of Rust, used only by the tests -- so that a
    /// test can assert **what the player sees** rather than the shape of our
    /// own tuple. The shape is what we got wrong; a test that pinned it would
    /// have passed throughout.
    #[cfg(test)]
    fn as_the_client_reads_it(value: &PyValue) -> Vec<Option<f64>> {
        Self::as_the_client_reads_it_after(value, 0.0)
    }

    /// The same decoder, `elapsed_ms` after the client stored the entry --
    /// lines 13-17 transcribed, the regeneration curve included:
    ///
    /// ```text
    /// 13  shieldHealth, tau = shieldValues[:2]
    /// 14  if tau > 0:
    /// 15      sq = sqrt(shieldHealth)
    /// 16      eToX = exp((time - now) / dogmaConst.dgmTauConstant / tau)
    /// 17      shieldHealth = (1.0 + (sq - 1.0) * eToX) ** 2
    /// ```
    ///
    /// `(time - now) / dgmTauConstant` is `-elapsed_ms` **[V]**.
    #[cfg(test)]
    fn as_the_client_reads_it_after(value: &PyValue, elapsed_ms: f64) -> Vec<Option<f64>> {
        let PyValue::Tuple(state) = value else {
            panic!("a damage state is a sequence: {value:?}")
        };
        let shield = match &state[0] {
            PyValue::Tuple(v) | PyValue::List(v) if v.len() >= 2 => match (&v[0], &v[1]) {
                (PyValue::Real(health), PyValue::Real(tau)) if *tau > 0.0 => {
                    let e = (-elapsed_ms / tau).exp();
                    Some((1.0 + (health.sqrt() - 1.0) * e).powi(2))
                }
                (PyValue::Real(health), PyValue::Real(_)) => Some(*health),
                other => panic!("(health, tau) are floats: {other:?}"),
            },

            // Anything that is not a sequence appends `None`, which hides
            // the gauge.
            _ => None,
        };
        let plain = |v: &PyValue| match v {
            PyValue::Real(f) => Some(*f),
            _ => None,
        };
        vec![shield, plain(&state[1]), plain(&state[2])]
    }
}

/// The bars a [`crate::damage::ShipDamage`] reports, as the wire carries them.
///
/// The one conversion between the damage model and this encoder, so that the
/// `/5` lives in exactly one place -- [`crate::damage::ShieldBar::tau_ms`] --
/// and `OnDamageStateChange`, `SetState` and `AddBalls2` cannot disagree
/// about it.
impl From<crate::damage::Bars> for DamageState {
    fn from(b: crate::damage::Bars) -> DamageState {
        DamageState {
            shield: b.shield.map(|s| s.fraction),
            shield_tau_ms: b.shield.map_or(0.0, |s| s.tau_ms),
            armour: b.armour,
            hull: b.hull,
        }
    }
}

/// The station a character undocks from, as a ball.
///
/// **Without this the player is nowhere.** The ballpark is the whole scene --
/// the client renders only balls it has been given -- so a park containing
/// one ship is a ship in an empty universe, which is exactly what the first
/// real undock looked like. The real server fills the initial ballpark from
/// the solar system's contents; this is the first one of those we have.
///
/// `radius` is **15,733 m**, read out of `types.fsdbinary`'s `radius` column
/// for type 2502 **[V]** -- type 2502 is the Minmatar Trade Post (hull `ms3`) and it
/// really is sixteen kilometres across, and the number matters here because it is what the ship's offset
/// has to clear.
///
/// **Not free** (`isFree` clear, so a 64-byte record): a station has no
/// velocity for the client's simulation to integrate, and the flag byte is
/// `0x1e`, which is exactly what the client's own non-free dump carried
/// (`findings/destiny-ref/10-arg-a5_i.bin`).
///
/// `mass` is `0.0`, which is also what the types table says for a station.
pub fn station_ball(station_id: i64, radius: f32, owner_id: i32) -> Ball {
    station_ball_at(station_id, radius, owner_id, [0.0, 0.0, 0.0])
}

/// [`station_ball`] where the station actually is: a catalogued station at
/// CCP's own coordinates (`Registry::station_position`, FX4).
pub fn station_ball_at(station_id: i64, radius: f32, owner_id: i32, position: [f64; 3]) -> Ball {
    Ball {
        ball_id: station_id,
        mode: Mode::Stop,
        radius,
        position,
        flags: FLAGS_STATIC_BALL,
        mass: 0.0,
        cloak: 0,
        harmonic: -1,
        corporation_id: owner_id,
        alliance_id: -1,
        physics: None,
        formation_id: NO_FORMATION,
    }
}

/// One celestial as a ball: a body in space that nothing flies.
///
/// [`station_ball`] with the radius and type coming from the catalogue
/// instead of a constant. Not free -- the physics block is what the client's
/// own simulation evolves, and a planet does not need evolving -- and owned
/// by nobody, which for a bracket means the client names it from its own
/// static data.
pub fn celestial_ball(c: &crate::celestial::Celestial) -> Ball {
    Ball {
        ball_id: c.id,
        mode: Mode::Stop,
        // `f32` on the wire, which for a planet's millions of metres costs
        // about a metre of precision and matters to nothing: the radius
        // decides a bracket's size and the warp standoff, not a position.
        radius: c.radius as f32,
        position: c.position,
        flags: FLAGS_STATIC_BALL,
        mass: 0.0,
        cloak: 0,
        harmonic: -1,
        corporation_id: -1,
        alliance_id: -1,
        physics: None,
        formation_id: NO_FORMATION,
    }
}

/// One rock as a ball: [`celestial_ball`]'s shape with the rock's radius.
///
/// **The radius is the only size there is.** The client scales the model by
/// the ball radius (`scalableSpaceObject.py:37`) and picks the Small/Medium/
/// Large model by it too (group graphics' `categoryAttribute` is `'radius'`)
/// **[V]**; it never recomputes a radius from ore outside the dungeon editor.
/// So this number is what the pilot sees, and `crate::asteroid` makes it the
/// client's own law of the rock's quantity.
///
/// `Mode::Stop`, not free, flags `0x1e`, mass `0.0`: **[A]**, the planets'
/// precedent. CCP sends `RIGID` (11) **[V2]**, which we have no encoder for
/// and a ball that never moves does not need. A rock's type says mass `1e35`
/// **[V-data]**, which matters only to bumping (human check).
pub fn rock_ball(r: &crate::asteroid::Rock) -> Ball {
    Ball {
        ball_id: r.id,
        mode: Mode::Stop,
        radius: r.radius as f32,
        position: r.position,
        flags: FLAGS_STATIC_BALL,
        mass: 0.0,
        cloak: 0,
        harmonic: -1,
        corporation_id: -1,
        alliance_id: -1,
        physics: None,
        formation_id: NO_FORMATION,
    }
}

/// A rock's slim item: the existing five fields and nothing new.
///
/// `typeID` is the **ore type**; `groupID`, `categoryID` and `radius` come
/// from it client-side, and every asteroid path reads only those and the
/// item id (`docs/lanes/asteroids.md` §1) **[V]**. Owner `1` as for a
/// planet; no pilot, so `ProcessBallAdd` primes no owner, and category 25 is
/// explicitly excluded from its `cfg.evelocations.Hint` **[V]**.
pub fn rock_slim(r: &crate::asteroid::Rock) -> Slim {
    Slim {
        item_id: r.id,
        type_id: r.type_id,
        owner_id: 1,
        char_id: None,
        corp_id: None,
        response_thresholds: None,
    }
}

/// Station type 2502's radius, in metres. **[V]**, `types.fsdbinary`.
///
/// Read for the one station this sandbox has rather than tabulated: there is
/// no second station to look up, and a `hull::FIXTURE` row for a type that is
/// not a hull would be in the wrong table.
pub const STATION_RADIUS: f32 = 15_733.0;

/// A station type's radius, metres (FX7): the content typedata's own
/// `radius` for the type (`crate::typedata::installed_in`), else
/// [`STATION_RADIUS`] (no bundle, or an extract older than FX7's category 3).
pub fn station_radius(content: &crate::content::Content, station_type_id: i64) -> f32 {
    station_radius_in(
        crate::typedata::installed_in(content).table(),
        station_type_id,
    )
}

/// [`station_radius`] against a given table. A row that is not category 3
/// (`categoryStation` [V]), or whose radius is 1 m or less, is no answer:
/// `tools/typedata.py` writes `radius or 1.0`, the client's `GetRadius`
/// default, so 1.0 means "no radius" and not a one-metre station.
pub fn station_radius_in(table: &crate::typedata::Table, station_type_id: i64) -> f32 {
    table
        .get(station_type_id)
        .filter(|r| r.category_id == 3 && r.radius > 1.0)
        .map_or(STATION_RADIUS, |r| r.radius as f32)
}

/// How far off the station the ship appears, in metres.
///
/// **Clear of the hull, and that is the whole requirement.** The real server
/// computes an undock point from the station's docking-locator geometry, which
/// we do not have; a ship at the same coordinates as a sixteen-kilometre
/// station model is inside it, which is a worse answer than being plainly
/// outside and a little way off. `[I]` on the distance: anything past
/// [`STATION_RADIUS`] looks the same from the camera.
pub const UNDOCK_OFFSET: f64 = 20_000.0;

/// The ego ball for a character's ship, parked beside the station.
///
/// This was at the origin, on the grounds that "there is nothing else in the
/// ballpark to be near". **There is now**: the station is a ball, so the
/// origin is the middle of a sixteen-kilometre station model and the ship has
/// to be outside it. [`UNDOCK_OFFSET`] is that distance and
/// `docs/lanes/round2-spec.md` §3.4's note about the real server's
/// docking-locator geometry still stands -- this is "plainly outside", not
/// "where the docking bay is".
///
/// The hull's numbers come from `content` ([`hull::of_in`]).
pub fn ego_ball_in(
    content: &crate::content::Content,
    ship_id: i64,
    ship_type_id: i64,
    corporation_id: i32,
    alliance_id: i32,
    motion: &crate::space_state::Motion,
) -> Ball {
    let h = hull::of_in(content, ship_type_id);
    ego_ball_of(h, ship_id, corporation_id, alliance_id, motion)
}

/// [`ego_ball_in`] for a piloted ship: its **fitted** mass, top speed and
/// agility when `crate::dogma` has a sheet for it (an overdrive, a running
/// afterburner, Navigation, Evasive Maneuvering), so the client's own
/// simulation of the ball flies the ship the server integrates.
pub fn pilot_ball(
    w: &Registry,
    ship_id: i64,
    ship_type_id: i64,
    corporation_id: i32,
    alliance_id: i32,
    motion: &crate::space_state::Motion,
) -> Ball {
    let mut ball = match crate::dogma::hull(w, ship_id) {
        Some(h) => ego_ball_of(&h, ship_id, corporation_id, alliance_id, motion),
        None => ego_ball_in(
            w.content(),
            ship_id,
            ship_type_id,
            corporation_id,
            alliance_id,
            motion,
        ),
    };
    // CLOAK-1: a cloaked ego is rebuilt cloaked (a relog, a state request);
    // observers never get the ball while it is cloaked.
    ball.cloak = crate::cloak::mode_of(w, ship_id);
    ball
}

/// [`ego_ball_in`] from a hull in hand.
pub fn ego_ball_of(
    h: &hull::Hull,
    ship_id: i64,
    corporation_id: i32,
    alliance_id: i32,
    motion: &crate::space_state::Motion,
) -> Ball {
    Ball {
        ball_id: ship_id,
        // **The mode the server last commanded**, not a constant. A fresh
        // arrival is `DSTBALL_STOP`, which is "come to a halt" rather than
        // "is halted": a ball with velocity in STOP decelerates to rest under
        // the client's own simulation, which is what leaving a station looks
        // like. A ship that is flying when this blob is built -- a desync
        // recovery, or another pilot's bootstrap -- carries the mode it was
        // given, because a recovery that parks a moving ship is a teleport.
        mode: motion.mode,
        radius: h.radius,
        position: motion.position,
        flags: FLAGS_SHIP_BALL,
        mass: h.mass,
        cloak: 0,
        // `-1`, as every dump has it.
        harmonic: -1,
        corporation_id,
        alliance_id,
        // An ego ball must be free: the physics block is what the client's own
        // simulation evolves, and a ball without it cannot be flown.
        physics: Some(Physics {
            max_velocity: h.max_velocity,
            velocity: motion.velocity,
            agility: h.agility,
            // `speedFraction` is what the player has *asked* for, which is not
            // how fast the ship is going: a ship shoved out of a station mouth
            // has a velocity and a fraction of zero. The client's own
            // `activeShipController.SetSpeed` debounces on
            // `int(ball.speedFraction * 1000)`, so a blob that disagrees with
            // the throttle the player set makes their next drag to the same
            // notch vanish. **[V]**
            speed_fraction: motion.speed_fraction,
        }),
        formation_id: NO_FORMATION,
    }
}

/// Where a ship appears when it leaves this character's station, and how fast.
///
/// The seed for [`crate::space_state::Motion`], and the whole of what the
/// undock geometry decides.
pub fn undock_motion(w: &Registry, actor: i64) -> Option<(i64, crate::space_state::Motion)> {
    let c = w.character(actor)?;
    let ship_id = c.ship_id;
    // **The station's own mouth, from CCP's own code.** `crate::docking` is
    // `ballparkCommon.docking` ported: the ship's group picks an undock-point
    // *category*, the station's graphic supplies the locators in that
    // category, and the answer is a position and a heading in model space --
    // which is world space, because the only rotation input the client has is
    // a `dunRotation` we do not send, and nothing in `mapObjects.db` carries
    // one to send. See `docs/lanes/undock-points.md` §3 for that null result.
    //
    // `None` means we have no locator data for this station type, and then
    // the flat [`UNDOCK_OFFSET`] is the visible fallback. CCP's own
    // `DEFAULT_DIRECTION` is a **zero** offset -- the station's centre, which
    // is the bug a player reported -- so this deliberately does not follow
    // them there.
    //
    // **From the station's own position (FX4)**, which for a catalogued
    // station is CCP's: undocking at the origin put every pilot inside the
    // sun of their system.
    let station_position = c.station_id.map_or([0.0; 3], |id| w.station_position(id));
    // **An Upwell structure** (U4): its locators are rotated by the slim's
    // `dunRotation` (`docking::undock_vector` has the branch), and a type
    // with no locator rows leaves the ship a little off its surface.
    let structure = c
        .station_id
        .and_then(|id| crate::structure_dock::geometry(w, id));
    let mouth = match structure {
        Some(g) => crate::docking::undock_vector(
            w.content().get::<crate::docking::Table>(),
            w.content(),
            c.station_type_id,
            station_position,
            c.ship_type_id,
            Some(g.rotation),
            0,
        ),
        None => crate::docking::undock_point(
            w.content().get::<crate::docking::Table>(),
            w.content(),
            c.station_type_id,
            station_position,
            c.ship_type_id,
        ),
    };
    // **A berth each.** One locator serves every ship that leaves, so two
    // pilots undocking into the same system arrived at the same coordinates
    // -- and then coasted on the same shove, so they *stayed* there,
    // occupying one another. The real server has no such problem because it
    // has the station's whole docking geometry; we have one point, so the
    // second ship steps sideways off it. See [`berth`].
    // **Pilots only**: an NPC is in the roster too (`crate::npc`), and is
    // not a ship that left this station's mouth.
    let pilots: Vec<[f64; 3]> = c.solar_system_id.map_or_else(Vec::new, |sys| {
        w.roster()
            .others_in_system(sys, actor)
            .into_iter()
            .filter(|id| w.character(*id).is_some())
            .filter_map(|id| w.roster().get(id).map(|p| p.motion.position))
            .collect()
    });
    let beside = [
        station_position[0]
            + structure.map_or(UNDOCK_OFFSET, |g| {
                g.radius + crate::structure_dock::FALLBACK_MARGIN
            }),
        station_position[1],
        station_position[2],
    ];
    let position = mouth.map_or(beside, |u| {
        free_berth(u.position, station_position, &pilots)
    });
    // `docking.ShipUndockVelocity`: **1.75x the hull's own top speed, capped
    // at 700 m/s** -- the shove out of the mouth, which the STOP mode above
    // then decelerates to rest. A ship that undocks does coast.
    let velocity = mouth.map_or([0.0; 3], |u| {
        crate::docking::ship_undock_velocity(
            u.direction,
            f64::from(hull::of_in(w.content(), c.ship_type_id).max_velocity),
        )
    });
    Some((
        ship_id,
        crate::space_state::Motion::arrived(position, velocity),
    ))
}

/// **The first berth nobody is sitting in** (UNDOCK-REG). The berth index used
/// to be the number of pilots anywhere in the system, so with fifty bots out
/// mining the owner was put `25 * BERTH` sideways of the mouth -- outside tether
/// range. Only a ship still near a berth occupies it.
pub(crate) fn free_berth(mouth: [f64; 3], station: [f64; 3], pilots: &[[f64; 3]]) -> [f64; 3] {
    let taken = |at: [f64; 3]| {
        pilots.iter().any(|p| {
            let d = [p[0] - at[0], p[1] - at[1], p[2] - at[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() < BERTH * 0.5
        })
    };
    // The cap is the old behaviour's worst case, so it terminates.
    (0..=pilots.len())
        .map(|i| berth(mouth, station, i))
        .find(|at| !taken(*at))
        .unwrap_or(mouth)
}

/// How far apart two ships leaving the same station mouth are placed.
///
/// `[I]`. Far enough to be two brackets on the overview and two models in the
/// scene rather than one, and near enough that both are plainly "at the
/// station": 750 m is under a twentieth of [`STATION_RADIUS`] and larger than
/// the radius of anything this sandbox can fly.
pub const BERTH: f64 = 750.0;

/// The `index`-th ship's spot at a mouth, stepped around the station from the
/// first.
///
/// **Tangentially, in a patch.**
///
/// Tangentially -- perpendicular to the line from the station's centre, not
/// to the exit heading -- because of what the first version of this did. It
/// stepped sideways from the heading, which is the intuitive answer, and put
/// the second ship **inside the station model**: station 2502's mouth is
/// 16,152 m from the centre and its radius is 15,733, so there are 419 m of
/// clearance, and a 750 m step in a direction that is not tangential spends
/// more than that. A tangential step cannot: it moves the ship to
/// `sqrt(r^2 + step^2)` from the centre, which is always *further* out than
/// the mouth it started from. The clearance is now a guarantee of the
/// geometry rather than a property of one station's numbers.
///
/// A patch (UNDOCK-2) because a line puts the n-th ship `n / 2` berths away,
/// and with a crowd at the mouth that was kilometres; see [`berth_cell`].
///
/// The tangent is built from whichever world axis is least parallel to the
/// radial, so there is no mouth for which the cross product collapses. A
/// mouth at the station's own centre returns the position untouched -- one
/// ship at the right place beats several at a NaN, which the native discards
/// in silence.
pub(crate) fn berth(position: [f64; 3], station: [f64; 3], index: usize) -> [f64; 3] {
    if index == 0 {
        return position;
    }
    let Some(radial) = crate::space_state::unit([
        position[0] - station[0],
        position[1] - station[1],
        position[2] - station[2],
    ]) else {
        return position;
    };
    let axis = if radial[0].abs() <= radial[1].abs() && radial[0].abs() <= radial[2].abs() {
        [1.0, 0.0, 0.0]
    } else if radial[1].abs() <= radial[2].abs() {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let cross = [
        radial[1] * axis[2] - radial[2] * axis[1],
        radial[2] * axis[0] - radial[0] * axis[2],
        radial[0] * axis[1] - radial[1] * axis[0],
    ];
    let Some(tangent) = crate::space_state::unit(cross) else {
        return position;
    };
    // **A square patch of the tangent plane, nearest cell first** (UNDOCK-2).
    // The first version was one line (`+1, -1, +2, -2`), so the n-th ship
    // stood `n / 2 * BERTH` off the mouth: in the real client the owner's
    // undock was 6 km sideways (8 berths) with the bots crowding the mouth
    // and 8.1 km from the station's centre. A patch puts the n-th at about
    // `sqrt(n) / 2 * BERTH`, 25 ships inside 2.2 km. Every cell is in the
    // tangent plane, so each is still *further* out than the mouth.
    let across = [
        radial[1] * tangent[2] - radial[2] * tangent[1],
        radial[2] * tangent[0] - radial[0] * tangent[2],
        radial[0] * tangent[1] - radial[1] * tangent[0],
    ];
    let (i, j) = berth_cell(index);
    let (a, b) = (i as f64 * BERTH, j as f64 * BERTH);
    [
        position[0] + tangent[0] * a + across[0] * b,
        position[1] + tangent[1] * a + across[1] * b,
        position[2] + tangent[2] * a + across[2] * b,
    ]
}

/// The `index`-th cell of the berth patch: `(0, 0)` first, then by distance
/// from it, ties by `(i, j)` so the order is fixed. [`berth`]'s grid.
pub(crate) fn berth_cell(index: usize) -> (i64, i64) {
    // Every cell within `reach` of the origin is more than `index` cells.
    let reach = (index as f64).sqrt().ceil() as i64 + 1;
    let mut cells: Vec<(i64, i64)> = (-reach..=reach)
        .flat_map(|i| (-reach..=reach).map(move |j| (i, j)))
        .collect();
    cells.sort_by_key(|&(i, j)| (i * i + j * j, i, j));
    cells[index]
}

/// The state this character should be looking at, or `None` if there is none.
///
/// Reads [`crate::space_state::Motion`] for where the ship is and what it is
/// doing, seeding it from [`undock_motion`] the first time. That is what makes
/// a desync recovery a recovery: a fresh `SetState` is the **only** thing that
/// clears the client's `validState` after a `RequestReset` (`michelle.py:1076`,
/// `:1260`, `:1527`) **[V]**, and one built from the undock constants would
/// clear it by teleporting the ship back to the station mouth and stopping it.
///
/// `None` for a connection that is nobody, and for a character the roster does
/// not know. There is deliberately **no docked/undocked test**: this server has
/// no such state -- `Character` carries a station and a system and nothing else
/// -- and it does not need one here, because the client binds `beyonce` only on
/// entering space. The bind *is* the signal; see [`on_ballpark_bind`].
pub fn bootstrap_for(w: &mut Registry, actor: i64) -> Option<Bootstrap> {
    // **Nothing re-seeds a pilot between two systems.** From the jump until
    // the destination's bind, the roster has no presence for them, and the
    // seed below would put them back at the home station's mouth -- so an
    // `UpdateStateRequest` in that window would teleport a jumper home
    // (`docs/lanes/stargates.md` §3.1). Their next state is the arrival's.
    if crate::jump::in_transit(w, actor) {
        return None;
    }
    // **Nor between a login's session change and its bind, when the ship
    // is to be restored at its log-off spot**: the seed below is the
    // station's mouth, and the bind is what lands the ship at the spot.
    if !w.roster().is_in_space(actor) && crate::logoff_spot::get(w, actor).is_some() {
        return None;
    }
    let (ship_id, seed) = undock_motion(w, actor)?;
    let c = w.character(actor)?;
    // **Home is where the station is; space is where the roster says.**
    // `Character::solar_system_id` is the station's system and never moves
    // (`docs/lanes/stargates.md` §2.3); a pilot who jumped is in the system
    // their presence names. The home system is only the seed's.
    let home = c.solar_system_id?;
    // A structure is a ball of the scene already (U3), not a station to add.
    let station = c
        .station_id
        .filter(|id| !crate::structure_dock::is_structure(w, *id))
        .map(|id| (id, c.station_type_id, c.station_owner_id));
    let solar_system_id = w.roster().get(actor).map_or(home, |p| p.solar_system_id);
    // **The caller is in the park whether or not the roster has caught up.**
    // `Park.SetState` assigns `self.ego` without checking that the ball
    // exists, and `GetMyPosition` then reads `GetBall(ego).x` off a `None`
    // -- so an ego-less state *succeeds*, sets `validState`, and fails later
    // in a read, somewhere that names neither us nor the state. CCP's own
    // bootstrap force-appends the ego for the same reason. Seeding here makes
    // "is in the roster" and "has a ball" the same fact.
    if !w.roster().is_in_space(actor) {
        // A warp's late arrival batch outlives a dock or a jump; it must not
        // snap the ship it left behind onto the new ball (GRID-VIS).
        w.cancel_scheduled(|p| crate::service::space::is_warp_settle(p, ship_id));
        w.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                actor, ship_id, home, seed,
            ));
    }
    // **Every ball in this park, integrated to one instant.** The positions
    // below are read straight out of the roster, and a roster entry is only
    // true as of the moment its ship was last commanded -- so without this a
    // pilot binding their ballpark is handed every other ship where it was
    // when it last got a click, which for a ship crossing a grid is
    // kilometres out and never corrects. There is no correction to send:
    // `SetBallPosition` and `SetBallVelocity` are outside every client
    // dispatch set and reset nothing (`docs/lanes/movement.md` §8). The state
    // is the only chance to be right.
    w.advance_system(solar_system_id);
    // **The pilot's own station, only at home and only when the catalogue
    // has no row for it** -- a catalogued station is a ball among the
    // system's furniture, at its own position (FX4). An uncatalogued one
    // sits at the origin of *its* system; in any other one the origin is the
    // sun, and a trade post inside it is a ball the client would draw where
    // no station is (`docs/lanes/stargates.md` §3.1).
    let station = station.filter(|s| solar_system_id == home && !w.celestials().is_served(s.0));
    // **From the world's allocator, not from the clock directly.** Every
    // stamp this server sends has to be non-decreasing for ever: a batch
    // below the last `SetState`'s stamp is dropped with
    // `Michelle discarded a state that was too old` (`michelle.py:1443-1448`),
    // and one below the park's current time needs a snapshot to rewind to --
    // which the first action after it wipes (`FlushSimulationHistory:1375`).
    // See `Registry::next_destiny_stamp` and `docs/lanes/movement.md` §4.
    let stamp = w.next_destiny_stamp();
    let mut state = scene(w, solar_system_id, ship_id, station, stamp);
    // **The grid, not the system** (GRID-B, `docs/lanes/grid-research.md`
    // §4). The overview and the brackets list every ball in the park with no
    // distance test at all (`overviewWindow.pyc:1270-1272`,
    // `bracketMgr.pyc:339-392`) **[V]**, so a rock 1 AU away that is in the
    // park is a row on the player's screen -- which is what a pilot
    // undocking in Tanoo saw (`docs/port-tracker.md` §74.2). The global
    // furniture stays; a local ball stays only inside
    // [`crate::grid::GRID_ENTER`] of the ship. The model of what this
    // client now holds is written when the state is drained
    // (`crate::grid::filter`, `Resets`), not here.
    let ego_at = w
        .roster()
        .get(actor)
        .map_or([0.0; 3], |p| p.motion.position);
    let keep = crate::grid::grid_of(w, solar_system_id, ship_id, ego_at);
    state.retain_balls(|id| keep.contains(id));
    Some(state)
}

/// A `SetState` for a client whose park is already running: restamped past
/// everything sent so far ([`Registry::next_setstate_stamp`]) so that the
/// client's `FlushState` gives it an entry of its own, at index 0, where
/// `RealFlushState` honours it. Both clocks move together, as
/// [`StateUpdate`]'s `restamp` does.
///
/// **[A]** the ball positions stay as of the bootstrap's own second, so the
/// scene is up to a second behind its stamp; falsified by a moving ship that
/// visibly steps back after a `RequestReset`.
pub fn mid_session_state(w: &mut Registry, actor: i64, state: Bootstrap) -> Option<StateUpdate> {
    let stamp = w.next_setstate_stamp(actor)?;
    let mut update = StateUpdate {
        to: vec![actor],
        state,
    };
    crate::notify::Push::restamp(&mut update, stamp);
    Some(update)
}

/// **Every ball in one system, at one instant**: the whole scene, before any
/// grid is applied. `ego` is the ship the state is for (`0` for nobody's, as
/// `crate::grid`'s reconcile builds it to take balls out of), and `station`
/// the pilot's uncatalogued home station when it belongs in this system
/// ([`bootstrap_for`] decides).
///
/// Split out of [`bootstrap_for`] so that the reconcile's `AddBalls2` is
/// built from the same slims, gauges and fitted modules a `SetState` would
/// carry ([`Bootstrap::added`]): two builders of one ball are two answers to
/// what it looks like.
pub fn scene(
    w: &Registry,
    solar_system_id: i64,
    ego: i64,
    station: Option<(i64, i64, i64)>,
    stamp: u32,
) -> Bootstrap {
    // One instant for every ship's health, the same one the positions were
    // just brought up to. Blue time, because that is `crate::damage`'s clock.
    let now = crate::pyutil::filetime_now();
    // **Every moving ball as of the stamp's second boundary**, not of now
    // (`Motion::at_boundary`).
    let boundary = crate::space_state::now_seconds().floor();
    let mut balls = Vec::new();
    let mut slims = Vec::new();
    let mut damage = Vec::new();
    // **What every pilot has fitted**, so the arriver's client draws other
    // ships' guns ([`SlimModule`]); its own ship's are read from its own
    // dogma and this entry is ignored for it.
    let mut modules: ShipModules = Vec::new();
    // **Everyone in the system, not just the caller.** This is what makes a
    // second player visible: the bootstrap is "the scene as it is", and a
    // pilot who undocks into an occupied system has to see who is already
    // there -- the other half, telling *them*, is `on_ballpark_bind`'s
    // `BallsAdded`.
    for p in w.roster().in_system(solar_system_id) {
        let at = p.motion.at_boundary(p.since, boundary);
        // **An NPC is in the roster under its own id** and has no character
        // row (`docs/lanes/npc.md` §5): without this branch a pilot who
        // undocks after a spawn is never shown it -- the roster-leak `continue`
        // below would skip it in silence.
        // **A drone is in the roster under its item id** too, and is owned
        // by a character it is not (`crate::drones`).
        if let Some(d) = w.modules().drones().get(p.character_id).copied() {
            balls.push(crate::drones::ball(w, &d, &at));
            slims.push(crate::drones::slim(w, &d));
            damage.push((
                p.ship_id,
                crate::combat::damage_state(w, p.ship_id, now).to_pyvalue(),
            ));
            continue;
        }
        // **A fighter squadron** likewise (`crate::fighters`).
        if let Some(s) = w
            .ext::<crate::fighters::State>()
            .squadron(p.character_id)
            .copied()
        {
            if let Some(b) = crate::fighters::ball(w, &s, &at) {
                balls.push(b);
                slims.push(crate::fighters::slim(w, &s));
                damage.push((
                    p.ship_id,
                    crate::combat::damage_state(w, p.ship_id, now).to_pyvalue(),
                ));
            }
            continue;
        }
        // **An abandoned ship** likewise (`crate::pilotless`): a ship ball
        // without the interactive bit and a slim with no `charID`, its
        // fitted modules drawn like any ship's.
        if let Some(a) = crate::pilotless::get(w, p.character_id) {
            balls.push(crate::pilotless::ball(w, &a, &at));
            slims.push(crate::pilotless::slim(&a));
            modules.push((
                p.ship_id,
                crate::modules::slim_modules(w.store(), p.ship_id),
            ));
            damage.push((
                p.ship_id,
                crate::combat::damage_state(w, p.ship_id, now).to_pyvalue(),
            ));
            continue;
        }
        if let Some(n) = w.npcs().get(p.character_id) {
            balls.push(crate::npc::ball(w.content(), n, &at));
            slims.push(crate::npc::slim(n));
            damage.push((
                p.ship_id,
                crate::combat::damage_state(w, p.ship_id, now).to_pyvalue(),
            ));
            continue;
        }
        let Some(pc) = w.character(p.character_id) else {
            // A presence with no character is a roster leak, not a ball we
            // can build. Skipping it loses one ship; inventing one would put
            // a nameless bracket on everybody's screen for ever.
            continue;
        };
        let corporation_id = i32::try_from(pc.corporation_id).unwrap_or(-1);
        let alliance_id = pc
            .alliance_id
            .and_then(|a| i32::try_from(a).ok())
            .unwrap_or(-1);
        balls.push(pilot_ball(
            w,
            p.ship_id,
            pc.ship_type_id,
            corporation_id,
            alliance_id,
            &at,
        ));
        slims.push(Slim {
            item_id: p.ship_id,
            type_id: pc.ship_type_id,
            owner_id: p.character_id,
            // **`charID`, always.** Without it `uix.GetSlimItemName` returns
            // `evetypes.GetName(typeID)` -- the *hull* name -- and asks
            // nobody, so a system of players reads as a fleet of identically
            // named Ibises with no log line on either side.
            char_id: Some(p.character_id),
            corp_id: Some(pc.corporation_id),
            response_thresholds: None,
        });
        modules.push((
            p.ship_id,
            crate::modules::slim_modules(w.store(), p.ship_id),
        ));
        // **The ship's real bars**, regenerating at its own `tau`, for a ship
        // somebody has shot; three full ones for a ship nobody has. A pilot
        // who undocks into a fight is told the truth about it.
        damage.push((
            p.ship_id,
            crate::combat::damage_state(w, p.ship_id, now).to_pyvalue(),
        ));
    }
    // **The system's own furniture.** Planets, moons and belts, from the
    // extract `--celestials` loaded. Without these the client renders a
    // complete-looking solar system from its own static data and has no
    // *ball* for any of it -- so no bracket, no right-click menu, and
    // therefore no `CmdWarpToStuff` for us to answer. A player found this by
    // being able to warp to the station and to nothing else; the station was
    // the only celestial we had ever put in the park. `crate::celestial`.
    //
    // **This system's furniture only** (`Catalogue::served_in`): with two
    // systems loaded, the other one's planets would otherwise sit in this
    // scene at their own coordinates.
    //
    // **And its stargates**, each with the `jumps` row that makes a
    // double-click a jump rather than an `IndexError` ([`GateSlim`]).
    let mut gates = Vec::new();
    for c in w.celestials().served_in(solar_system_id) {
        // **Every station, where it is (FX4).** Before this only the
        // pilot's own station was a ball, at the origin and only at home:
        // in any other system there was nothing to right-click, so no
        // "Warp to" and no "Dock". The radius is the station type's own
        // (FX7, [`station_radius`]), [`STATION_RADIUS`] without typedata.
        if c.kind == crate::celestial::Kind::Station {
            let owner = w
                .content()
                .get::<crate::stations::Table>()
                .get(c.id)
                .map(|s| s.owner_id)
                .or_else(|| station.filter(|s| s.0 == c.id).map(|s| s.2))
                .unwrap_or(1);
            balls.push(station_ball_at(
                c.id,
                station_radius(w.content(), c.type_id),
                i32::try_from(owner).unwrap_or(-1),
                c.position,
            ));
            slims.push(Slim {
                item_id: c.id,
                type_id: c.type_id,
                owner_id: owner,
                char_id: None,
                corp_id: Some(owner),
                response_thresholds: None,
            });
            continue;
        }
        balls.push(celestial_ball(c));
        if c.kind == crate::celestial::Kind::Gate {
            // `served_in` admits a gate only with somewhere to go, so this
            // answers; the `else` is unreachable and harmless.
            if let Some((to_gate, to_system)) = w.celestials().jump_of(c.id) {
                gates.push(GateSlim {
                    item_id: c.id,
                    type_id: c.type_id,
                    to_gate,
                    to_system,
                });
            }
            continue;
        }
        slims.push(Slim {
            item_id: c.id,
            type_id: c.type_id,
            // **`cfg.eveowners` already knows who owns a planet**, and the
            // client's only test on an owner is whether the id is in the
            // player range (90,000,000 and up) -- `1` is the EVE system
            // owner and is comfortably outside it, so the bracket takes the
            // celestial's own name from the client's static data.
            owner_id: 1,
            // No pilot and no corporation: `ProcessBallAdd` primes an owner
            // only for a `charID` that is not None, which is right here for
            // the same reason it is right for the station.
            char_id: None,
            corp_id: None,
            response_thresholds: None,
        });
    }
    // **The rocks in this system's belts**, from `--belt-rocks`. Without
    // them the client compares its own belt counts with an empty park and
    // calls every belt "fully depleted" -- and a pilot has nothing to lock.
    //
    // **No `damage` entry**: a missing one is "no gauges", which is right
    // for a rock, and a full one would draw three HP rings on it.
    //
    // **Every belt's rocks here; [`bootstrap_for`] keeps the grid's.** The
    // scene is the whole system so the reconcile can take any rock out of
    // it (`crate::grid`); what a pilot is *sent* is the rocks within
    // `GRID_ENTER` of their ship, and a warp brings the next belt's in by
    // `AddBalls2` (`docs/lanes/grid-research.md` §5).
    for r in w.rocks().in_system(solar_system_id) {
        balls.push(rock_ball(r));
        slims.push(rock_slim(r));
    }
    // **The site objects** a pilot instantiated (W14 CP28a): beacons,
    // structures and clouds. Their rocks are in the field, above. No
    // `damage` entry, like a rock.
    for (ball, slim) in crate::site::balls_in(w, solar_system_id) {
        balls.push(ball);
        slims.push(slim);
    }
    // **The lit cynosural fields** (CAP-D, `crate::cyno`): local balls, so a
    // pilot who warps onto a field's grid is sent it.
    for (ball, slim) in crate::cyno::balls_in(w, solar_system_id) {
        balls.push(ball);
        slims.push(slim);
    }
    // **The warp disrupt probes out** (BUBBLES-SB, `crate::bubble`): local
    // balls, like a cynosural field.
    for (ball, slim) in crate::bubble::balls_in(w, solar_system_id) {
        balls.push(ball);
        slims.push(slim);
    }
    // **The system's Sovereignty Hub** (SOC-D2, `crate::sov_hub`): always
    // global, so it is in no `grid::local_balls` and every park keeps it.
    for (ball, slim) in crate::sov_hub::balls_in(w, solar_system_id) {
        balls.push(ball);
        slims.push(slim);
    }
    // **The ESS and its warp beacon** (ESS-2, `crate::ess`): the ESS is a
    // local ball, the beacon a global one.
    for (ball, slim) in crate::ess::balls_in(w, solar_system_id)
        .into_iter()
        .chain(crate::ess::beacon_balls_in(w, solar_system_id))
    {
        balls.push(ball);
        slims.push(slim);
    }
    // **The Upwell structures** (U3, `crate::structure_space`): global in
    // their system, like the hub.
    for (ball, slim) in crate::structure_space::balls_in(w, solar_system_id) {
        balls.push(ball);
        slims.push(slim);
    }
    // **The campaign's command nodes** (SOC-D3b, `crate::command_nodes`):
    // global in their system, like the hub.
    for (ball, slim) in crate::command_nodes::balls_in(w, solar_system_id) {
        balls.push(ball);
        slims.push(slim);
    }
    // **The wrecks**, so a pilot who undocks after a fight sees what it
    // left (`docs/lanes/damage-death.md`, P2.2c). No `damage` entry, like a
    // rock: a wreck has no gauges. Its `launcherID` names a ball long gone,
    // which `Wreck.Prepare:74` survives by falling back to a random rotation
    // (`docs/re/damage-death.md` §6) **[V]**.
    let mut wrecks = Vec::new();
    for (ball, slim) in crate::combat::wrecks_in(w, solar_system_id) {
        balls.push(ball);
        wrecks.push(slim);
    }
    // **The customs offices**, one over each planet (`docs/lanes/planet.md`
    // §11): what PI imports and exports through. No `damage` entry, like a
    // rock.
    let mut orbitals = Vec::new();
    for office in crate::planet::customs_offices_in(
        w.celestials(),
        crate::planet::statics_in(w.content()),
        solar_system_id,
    ) {
        balls.push(office.ball());
        orbitals.push((office.item_id, office.slim()));
    }
    // **The station the character just left.** Without it the scene is one
    // ship in an empty universe, which is what the first real undock looked
    // like: the player reported being "nowhere, at least not outside the
    // station". The ballpark *is* the scene.
    //
    // Still keyed on the caller's own `station_id` rather than on the
    // system's contents, which is the same thing while this sandbox has one
    // station and is the line to change when it does not. [`bootstrap_for`]
    // passes it only at home and only when the catalogue has no row for it.
    if let Some((station_id, station_type_id, station_owner_id)) = station {
        balls.push(station_ball(
            station_id,
            station_radius(w.content(), station_type_id),
            i32::try_from(station_owner_id).unwrap_or(-1),
        ));
        slims.push(Slim {
            item_id: station_id,
            type_id: station_type_id,
            owner_id: station_owner_id,
            // A station has no pilot. `ProcessBallAdd` primes an owner only
            // for a `charID` that is not None, which is correct here: the
            // bracket wants the station's name, which the client already has
            // from `config.GetMultiLocationsEx`.
            char_id: None,
            corp_id: Some(station_owner_id),
            response_thresholds: None,
        });
    }
    // **Whatever else a registered source says about these slims**
    // (`crate::slim_extra::SOURCES`, empty today).
    let slim_ids: Vec<i64> = slims.iter().map(|s: &Slim| s.item_id).collect();
    let extras = crate::slim_extra::for_slims(w, &slim_ids);
    // **What is already running in this system**, for a late arrival's
    // `effectStates` and the reconcile's replay (FX-REPLAY); and the ego's
    // incoming boosts, which a `SetState` would otherwise reset.
    let in_scene: std::collections::BTreeSet<i64> = slim_ids.iter().copied().collect();
    let fx = crate::modules::active_fx(w, &in_scene);
    let dbuff = (ego != 0).then(|| crate::dbuff::state_value_of(w, ego));
    let mut state = Bootstrap::new(
        stamp,
        Park {
            current_time: stamp,
            balls,
        },
        ego,
        slims,
        // **Every ship's health, and no station's.** A station has no damage
        // gauges; a ball with no entry here simply has none drawn, which is
        // right for the station and was the bug for the ship.
        damage,
    )
    .with_wrecks(wrecks)
    .with_orbitals(orbitals)
    .with_gates(gates)
    .with_modules(modules)
    .with_drones(crate::drones::rows_in(w, solar_system_id))
    .with_extras(extras);
    state.fx = fx;
    state.dbuff = dbuff;
    state
}

/// Queue the bootstrap state, on the bind that proves the ballpark exists.
///
/// Called from `dispatch::pending_for_bound_call` for
/// `beyonce.MachoBindObject(solarSystemID, None)`, whose fused call is
/// `PyValue::None` and so whose fused method is the empty string.
///
/// **Why this bind and not the undock.** `michelle.DoDestinyUpdate` returns
/// immediately while `self.__bp is None`, and the ballpark is built about
/// three seconds after the undock session change, from
/// `OnPrimaryViewChanged`. A state pushed any earlier is discarded in
/// silence -- which is the failure mode with no symptom at all, and the
/// reason this hook exists rather than a line in the undock arm. By the time
/// this bind arrives, `AddBallpark` has run and the `Park` has an empty
/// history. The capture puts the bind 1.4 ms after `GetFormations`'s answer.
/// **[V]**
///
/// `push`, not `push_later`: the queue drains **after** the response in the
/// same flush, which is what we want, and `FlushState` is pure bookkeeping
/// that tolerates arriving before the park's first tick because `DoPreTick`
/// is what flushes. If a relaunch ever shows the state landing inside the
/// sub-millisecond window before `Park.__init__` has run, a
/// `push_later(250ms)` is the fix and costs one tick.
///
/// A state whose slim items do not all name balls is **not sent**: it would
/// raise `BallNotInPark` on the client and lose the whole state, so refusing
/// locally is strictly better than being refused remotely.
pub fn on_ballpark_bind(w: &mut Registry, actor: Option<i64>) {
    let Some(actor) = actor else { return };
    w.forget_setstate(actor);
    // **A pilot docked in an Upwell structure binds too** (U4): the
    // `structure` view keeps a park, and a bind after a seating is the
    // client's new park. `crate::structure_dock::on_bind` answers those.
    if crate::structure_dock::on_bind(w, actor) {
        return;
    }
    // **A bind means a ship has just arrived in space**, and the mouth is
    // where it starts -- not wherever the last flight ended.
    // `michelle.Park.Release` destroys the park and unbinds the object
    // (`:1196`, `:1213`), so the next undock builds a new one and binds
    // again: this function runs once per undock, which is exactly when the
    // arrival should be rewritten. Without this, docking and undocking would
    // put the ship back where it was when it docked.
    //
    // **The bind, not the undock, is also where the roster gains a member.**
    // A client that dies in the three seconds between the undock session
    // change and this bind is then never in the roster at all, which is the
    // safe direction: nobody is told about a ship that never appeared.
    //
    // **Unless the ship came through a gate.** A jump leaves a pending
    // arrival (`crate::jump`), and this bind -- the destination's, sent after
    // the jump's session change -- is where it lands: by the gate, in the
    // other system, at rest.
    //
    // **Unless the ship never left.** A pilot who logs back in while their
    // ship is held in space (`crate::crimewatch::logged_off`) binds with a
    // presence already in the roster: the ship is where the hold left it,
    // perhaps a million kilometres from the station after its emergency
    // warp, and everyone else in the system has had its ball all along. So
    // nothing is re-seeded and nobody is told again -- the pilot gets the
    // scene, and that is all. Only that login's first bind
    // (`crimewatch::take_rejoin`): every other bind keeps the contract above.
    if crate::crimewatch::take_rejoin(w, actor) {
        if let Some(state) = bootstrap_for(w, actor)
            && state.every_slim_has_a_ball()
            && let Some(update) = mid_session_state(w, actor, state)
        {
            w.push(update);
        }
        return;
    }
    let spot = crate::logoff_spot::take(w, actor);
    let mut arrived_by_jump = false;
    let (ship_id, motion, solar_system_id) = match spot {
        Some(s) => (
            s.ship_id,
            crate::space_state::Motion::arrived(s.position, [0.0; 3]),
            s.system,
        ),
        None => match crate::jump::take_arrival(w, actor) {
            Some(arrival) => {
                arrived_by_jump = true;
                let Some(ship_id) = w.character(actor).map(|c| c.ship_id) else {
                    return;
                };
                (ship_id, arrival.motion, arrival.system)
            }
            None => {
                let Some((ship_id, motion)) = undock_motion(w, actor) else {
                    return;
                };
                let Some(home) = w.character(actor).and_then(|c| c.solar_system_id) else {
                    return;
                };
                (ship_id, motion, home)
            }
        },
    };
    w.roster_mut()
        .arrive(crate::space_state::Presence::arriving(
            actor,
            ship_id,
            solar_system_id,
            motion,
        ));
    // UNDOCK-INVULN: a ship that has just come out of a station or through a
    // gate is protected until it acts or the timer ends.
    // A ship restored at its log-off spot gets the restoring protection.
    // CLOAK-2: a gate (or wormhole, or jump drive) arrival is cloaked
    // instead; an undock keeps the invulnerability.
    let now = crate::pyutil::filetime_now();
    if spot.is_some() {
        crate::invuln::grant_restoring(w, actor, ship_id, now);
    } else if arrived_by_jump {
        // A jump inside the undock window keeps the ship id, so the invuln
        // tick would never see it: end the protection here.
        crate::invuln::break_for(w, actor);
        crate::cloak::grant_jump(w, actor, ship_id, now);
    } else {
        // A stale gate cloak from before a dock is gone.
        crate::cloak::forget(w, ship_id);
        crate::invuln::grant(w, actor, ship_id, now);
    }
    // A jump's arrival takes its scene as a SetState that opens its own entry
    // (`live_park`): a push decided before the turn that armed the leg can
    // still post after the session change, and it must not share the entry
    // the first SetState lands in. **[A]** falsified by `Events ignored` in
    // `fci_combat_site` under repeated `/tr`.
    announce_arrival(w, actor, ship_id, motion, solar_system_id, arrived_by_jump);
}

/// **A ship undocking from an Upwell structure into the park its client
/// already holds** (U4, the `structure` view, which does not re-bind): the
/// seating `on_ballpark_bind` does at a bind, now, with the client's scene
/// replaced by a mid-session `SetState` whose ego is the ship.
pub(crate) fn seat_into_live_park(w: &mut Registry, actor: i64) {
    let Some((ship_id, motion)) = undock_motion(w, actor) else {
        return;
    };
    // Docked in the structure's system, which is where the ship appears.
    let Some(home) = w.session_system(actor) else {
        return;
    };
    w.roster_mut()
        .arrive(crate::space_state::Presence::arriving(
            actor, ship_id, home, motion,
        ));
    // A stale gate cloak from before the dock is gone, as at an ordinary undock.
    crate::cloak::forget(w, ship_id);
    crate::invuln::grant(w, actor, ship_id, crate::pyutil::filetime_now());
    announce_arrival(w, actor, ship_id, motion, home, true);
}

/// The second half of an arrival: the arriver's scene, and the others'
/// `AddBalls2`. `live_park` is a client whose park is up already, which takes
/// the scene as a mid-session `SetState` ([`mid_session_state`]).
fn announce_arrival(
    w: &mut Registry,
    actor: i64,
    ship_id: i64,
    motion: crate::space_state::Motion,
    solar_system_id: i64,
    live_park: bool,
) {
    let Some(state) = bootstrap_for(w, actor) else {
        return;
    };
    if !state.every_slim_has_a_ball() {
        return;
    }
    // Everyone already here, captured before the push so the audience cannot
    // include the arriver.
    // Characters only: an NPC in the roster has no park to add a ball to.
    let observers: Vec<i64> = w
        .roster()
        .others_in_system(solar_system_id, actor)
        .into_iter()
        .filter(|id| w.character(*id).is_some())
        .collect();
    let stamp = state.stamp;
    if live_park {
        if let Some(update) = mid_session_state(w, actor, state) {
            w.push(update);
        }
    } else {
        w.push(StateUpdate {
            to: vec![actor],
            state,
        });
    }

    // **And the other half: tell the people already in space.** An
    // `AddBalls2` rather than a fresh state for each of them -- a `SetState`
    // would reset their ballpark, rebuild every model and bracket, and
    // teleport their own ship to the position we last stored. Once per other
    // player entering the system.
    // A cloaked arrival (CLOAK-2) is not shown to them; the break adds it.
    if observers.is_empty() || crate::cloak::is_cloaked(w, ship_id) {
        return;
    }
    let Some(c) = w.character(actor) else { return };
    let ball = pilot_ball(
        w,
        ship_id,
        c.ship_type_id,
        i32::try_from(c.corporation_id).unwrap_or(-1),
        c.alliance_id
            .and_then(|a| i32::try_from(a).ok())
            .unwrap_or(-1),
        &motion,
    );
    let slim = Slim {
        item_id: ship_id,
        type_id: c.ship_type_id,
        owner_id: actor,
        char_id: Some(actor),
        corp_id: Some(c.corporation_id),
        response_thresholds: None,
    };
    // The **pair** form, not a bare dict: a bare dict makes the client store
    // `damageState[itemID] = (None, now)`, and a `None` state hides the
    // gauges rather than filling them. That is the empty-health-bar defect
    // arriving by a third door -- see `BallsAdded::to_push`.
    if let Some(added) = BallsAdded::new(
        observers,
        stamp,
        Park {
            current_time: stamp,
            balls: vec![ball],
        },
        vec![(
            slim,
            Some(
                crate::combat::damage_state(w, ship_id, crate::pyutil::filetime_now()).to_pyvalue(),
            ),
        )],
    ) {
        // **And what it has fitted.** This is the one moment an observer
        // learns it, and the only one that can draw it: a ship's guns are
        // fitted once, when its model assembles (`ship.AssembleModel:381` ->
        // `FitHardpoints`, a no-op while `self.fitted`, `:530`), and
        // `OnSlimItemChange` refits only a modular hull's subsystems
        // (`michelle.py:2021`, `IsSubSystemChange`) -- `ship.OnSlimItemUpdated`
        // never touches them (`ship.py:304-323`) **[V]**. Hence fitting is
        // refused in space (`crate::fitting::refuse_in_space`).
        let fitted = crate::modules::slim_modules(w.store(), ship_id);
        let extras = crate::slim_extra::for_slims(w, &[ship_id]);
        w.push(
            added
                .with_modules(vec![(ship_id, fitted)])
                .with_extras(extras),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IP36-b moved the encoders and slim types to `destiny/encode.rs`.
    /// Every moved public item must still answer at its old
    /// `crate::destiny::` path, so no caller changes; a missing re-export
    /// fails this test at compile time.
    #[test]
    fn reexports_cover_every_moved_item() {
        let _: [u8; 4] = [
            crate::destiny::PACKET_FULL_STATE,
            crate::destiny::FLAG_IS_FREE,
            crate::destiny::FLAGS_FREE_BALL,
            crate::destiny::NO_FORMATION,
        ];
        let _: f64 = crate::destiny::WARP_LENGTH_ALIGNING;
        let _ = std::any::type_name::<(
            crate::destiny::Mode,
            crate::destiny::Physics,
            crate::destiny::Ball,
            crate::destiny::Park,
            crate::destiny::Slim,
            crate::destiny::SlimModule,
            crate::destiny::ShipModules,
            crate::destiny::SlimExtra,
            crate::destiny::SlimExtras,
            crate::destiny::GateSlim,
        )>();
        let row: fn(i64, i64) -> PyValue = crate::destiny::jumps_row;
        assert!(matches!(row(1, 2), PyValue::DbRow(_)));
    }

    /// **A slim without response thresholds is the five keys it always
    /// was**, in both forms, and one with them adds exactly the two, as
    /// floats -- `docs/lanes/npc.md` §5 S3. The first half is what keeps
    /// every existing ball's wire bytes unchanged.
    #[test]
    fn response_thresholds_are_two_float_keys_or_nothing() {
        let plain = Slim {
            item_id: 1,
            type_id: 587,
            owner_id: 90_000_001,
            char_id: Some(90_000_001),
            corp_id: Some(1_000_009),
            response_thresholds: None,
        };
        let keys = |v: PyValue| -> Vec<(PyValue, PyValue)> {
            match v {
                PyValue::Dict(pairs) => pairs,
                PyValue::Instance { state, .. } => match *state {
                    PyValue::Dict(pairs) => pairs,
                    other => panic!("an attribute dict, got {other:?}"),
                },
                other => panic!("a dict, got {other:?}"),
            }
        };
        let names = |pairs: &[(PyValue, PyValue)]| -> Vec<PyValue> {
            pairs.iter().map(|(_, k)| k.clone()).collect()
        };
        let five: Vec<PyValue> = ["itemID", "typeID", "ownerID", "charID", "corpID"]
            .iter()
            .map(|k| crate::pyutil::bstr(k))
            .collect();
        assert_eq!(names(&keys(plain.to_dict(&[]))), five);
        assert_eq!(names(&keys(plain.to_pyvalue(&[]))), five);

        let npc = Slim {
            response_thresholds: Some((11.0, 2.5)),
            ..plain
        };
        for pairs in [keys(npc.to_dict(&[])), keys(npc.to_pyvalue(&[]))] {
            assert_eq!(pairs.len(), 7);
            let get = |name: &str| {
                pairs
                    .iter()
                    .find(|(_, k)| *k == crate::pyutil::bstr(name))
                    .map(|(v, _)| v.clone())
            };
            assert_eq!(get("hostile_response_threshold"), Some(PyValue::Real(11.0)));
            assert_eq!(get("friendly_response_threshold"), Some(PyValue::Real(2.5)));
        }
    }

    /// The stamp allocator never goes backwards, and the bootstrap takes its
    /// stamp from it.
    ///
    /// Two client rules at once (`docs/lanes/movement.md` §4.4): a batch below
    /// the last `SetState`'s stamp is discarded as too old, and one below the
    /// park's clock needs a snapshot the first action wipes. Pins
    /// *non-decreasing*, not any particular number -- asserting
    /// `stamp == tick()` would fail the moment the clamp did its job, which is
    /// precisely when it matters.
    #[test]
    fn stamps_never_go_backwards_even_if_the_clock_does() {
        let mut reg = Registry::new(crate::model::SandboxConfig::default());
        let actor = reg.config().characters[0].character_id;
        let bootstrap = bootstrap_for(&mut reg, actor).expect("a state");
        let mut last = bootstrap.stamp;
        assert_eq!(
            bootstrap.stamp, bootstrap.park.current_time,
            "the batch's stamp and the blob's clock are one number"
        );
        for _ in 0..5 {
            let stamp = reg.next_destiny_stamp();
            assert!(
                stamp >= last,
                "a stamp below the last SetState's is discarded as too old: \
                 {stamp} < {last}"
            );
            last = stamp;
        }
    }

    /// A full-health ship draws three full bars, as the client computes them.
    ///
    /// Pins **the three numbers the client ends up with**, by running its own
    /// decoder (`DamageState::as_the_client_reads_it`), rather than the shape
    /// of the tuple we pack. That distinction is the whole point: the shape
    /// assertion this replaces was true of a payload whose shield health sat
    /// in a slot nothing reads, and a player saw an empty red shield bar on
    /// an undamaged ship while the test stayed green.
    ///
    /// `1.0` is a full bar and not an empty one, three ways: CCP's producer
    /// computes `1.0 - damage/max`, `SetDamageState:161` flags damage when a
    /// bar **narrows**, and `healthwidgets` calls the same number
    /// `healthPercent`.
    #[test]
    fn a_full_health_ship_reads_as_three_full_bars() {
        assert_eq!(
            DamageState::as_the_client_reads_it(&DamageState::FULL.to_pyvalue()),
            vec![Some(1.0), Some(1.0), Some(1.0)],
        );

        // And a damaged one reads back as itself, so the shield is carried by
        // the slot the client actually looks at.
        let hurt = DamageState {
            shield: Some(0.25),
            shield_tau_ms: 0.0,
            armour: 0.5,
            hull: 0.75,
        };

        assert_eq!(
            DamageState::as_the_client_reads_it(&hurt.to_pyvalue()),
            vec![Some(0.25), Some(0.5), Some(0.75)],
            "each bar must carry its own number: a shield in the wrong slot \
             reads as the zero that happens to be in slot 0"
        );
    }

    /// A shield slot that is not a sequence hides the gauge rather than
    /// emptying it.
    ///
    /// The distinction that cost a round: `michelle.GetDamageState` returns
    /// `None` for a ball with no entry, and `SetDamageState:154` sets
    /// `UI_HIDDEN` for a `None` -- so "no gauge" and "empty gauge" are
    /// different failures with different causes, and only one of them looks
    /// like damage.
    #[test]
    fn a_bare_float_shield_is_a_hidden_bar_not_an_empty_one() {
        let bare = PyValue::Tuple(vec![
            PyValue::Real(1.0),
            PyValue::Real(1.0),
            PyValue::Real(1.0),
        ]);
        assert_eq!(
            DamageState::as_the_client_reads_it(&bare),
            vec![None, Some(1.0), Some(1.0)],
            "a non-sequence shield slot appends None, which hides the gauge"
        );
    }

    /// **A damaged shield regenerates on the client exactly as the server
    /// computes it**, because the bar now carries CCP's own `tau`.
    ///
    /// Two transcriptions checked against each other, both `[V]`: the
    /// client's bar decoder (`CalculateCurrentDamageStateValues:13-17`, run
    /// here by [`DamageState::as_the_client_reads_it_after`]) and godma's
    /// `GetChargeValue` (`crate::damage::charge_at`), which the server uses to
    /// decide when a ship dies. If the `/5` were done twice, or not at all, or
    /// the frozen `tau = 0` were still on the wire, the bar would sit still
    /// or run at a different speed from the ring and the server, and this
    /// fails.
    #[test]
    fn a_damaged_shield_bar_regenerates_with_the_servers_own_curve() {
        use crate::damage::{Damage, Profile, ShipDamage};
        let c = crate::content::Content::fixture();
        let h = hull::find_in(&c, 587).expect("Rifter");
        let t0: i64 = 133_000_000_000_000_000;
        let mut ship = ShipDamage::full(Profile::from_hull_in(&c, h), t0);
        ship.apply(
            Damage {
                kinetic: 300.0,
                ..Damage::ZERO
            },
            t0,
        );
        let sent = DamageState::from(ship.bars(t0)).to_pyvalue();
        let at_arrival = DamageState::as_the_client_reads_it_after(&sent, 0.0);
        assert_eq!(
            at_arrival[0],
            Some(ship.shield_at(t0) / h.shield_capacity),
            "on arrival the bar is what we sent"
        );
        for minutes in [1.0, 5.0, 20.0] {
            let ms: f64 = minutes * 60_000.0;
            let later = t0 + (ms as i64) * 10_000;
            let client = DamageState::as_the_client_reads_it_after(&sent, ms)[0]
                .expect("the shield bar is shown");
            let server = ship.shield_at(later) / h.shield_capacity;
            assert!(
                (client - server).abs() < 1e-9,
                "after {minutes} min the client's bar reads {client}, the server {server}"
            );
            assert!(client > at_arrival[0].unwrap(), "and it has moved");
        }
    }

    /// A ship with no damage record is three full bars, frozen -- the shape
    /// every existing undock already sends -- and a hull with no shield is a
    /// **hidden** bar, not an empty one (`GetDamageStateEx:1766`) **[V]**.
    #[test]
    fn bars_without_a_shield_hide_the_gauge() {
        let none = DamageState::from(crate::damage::Bars {
            shield: None,
            armour: 1.0,
            hull: 0.5,
        });
        assert_eq!(
            DamageState::as_the_client_reads_it(&none.to_pyvalue()),
            vec![None, Some(1.0), Some(0.5)],
        );
    }

    /// **A wreck in a `SetState` is a slim like any other ball's, with its
    /// three keys** (`docs/re/damage-death.md` §6) **[V]**, and a state whose
    /// wreck has no ball is refused before the client raises
    /// `BallNotInPark` (`michelle.py` `SetState`).
    #[test]
    fn a_state_carries_its_wrecks_and_refuses_one_without_a_ball() {
        let wreck = WreckSlim {
            item_id: 9001,
            type_id: 26550,
            owner_id: 77,
            corp_id: 88,
            launcher_id: 4242,
            is_empty: true,
            fleet_id: None,
            abandoned: false,
        };
        let park = Park {
            current_time: 0,
            balls: vec![wreck_ball(9001, [0.0; 3])],
        };
        let state = Bootstrap::new(10, park.clone(), 1, vec![], vec![]).with_wrecks(vec![wreck]);
        assert!(state.every_slim_has_a_ball());
        let (_, args) = state.to_push();
        let PyValue::List(batch) = &args[0] else {
            panic!("a batch")
        };
        let PyValue::Tuple(entry) = &batch[0] else {
            panic!("(stamp, call)")
        };
        let PyValue::Tuple(call) = &entry[1] else {
            panic!("(name, args)")
        };
        let PyValue::Tuple(bag) = &call[1] else {
            panic!("(bag,)")
        };
        // The wreck's own instance, whole, somewhere in the bag's `slims`.
        assert!(
            format!("{:?}", bag[0]).contains(&format!("{:?}", wreck.to_pyvalue())),
            "the wreck's slim, with launcherID, isEmpty and lootRights"
        );

        let orphan = Bootstrap::new(
            10,
            Park {
                balls: vec![],
                ..park
            },
            1,
            vec![],
            vec![],
        )
        .with_wrecks(vec![wreck]);
        assert!(!orphan.every_slim_has_a_ball());
    }

    /// A ship that has arrived somewhere and been told nothing, at the
    /// origin: the geometry is not what most of these tests are about.
    pub(super) fn parked() -> crate::space_state::Motion {
        crate::space_state::Motion::arrived([0.0; 3], [0.0; 3])
    }

    // The client's own output. `destiny.Ballpark.WriteFullStateToStream`
    // produced every one of these inside the real client on 2026-10-21, so a
    // byte comparison against them is a property and not a tautology -- which
    // is the whole reason the dump was worth a human's evening.
    //
    // The alternative, asserting that the encoder matches a constant we wrote
    // down ourselves, passes for ever and proves nothing.
    const EMPTY: &[u8] = include_bytes!("../../../../findings/destiny-ref/01-empty-park.bin");
    const EMPTY_ALT: &[u8] =
        include_bytes!("../../../../findings/destiny-ref/02-empty-park-alt-tick.bin");
    const ONE_BALL: &[u8] = include_bytes!("../../../../findings/destiny-ref/03-base-one-ball.bin");
    const TWO_BALLS: &[u8] = include_bytes!("../../../../findings/destiny-ref/04-two-balls.bin");
    const NOT_FREE: &[u8] = include_bytes!("../../../../findings/destiny-ref/10-arg-a5_i.bin");
    const STOP: &[u8] = include_bytes!("../../../../findings/destiny-ref/66-call-Stop.bin");
    const FOLLOW: &[u8] = include_bytes!("../../../../findings/destiny-ref/67-call-FollowBall.bin");
    const ORBIT: &[u8] = include_bytes!("../../../../findings/destiny-ref/68-call-Orbit.bin");
    const GOTO: &[u8] = include_bytes!("../../../../findings/destiny-ref/69-call-GotoPoint.bin");
    const TROLL: &[u8] =
        include_bytes!("../../../../findings/destiny-ref/71-call-SetBallTroll.bin");

    /// The dump's two ball ids, which several vectors reference.
    const BALL_A: i64 = 0x1112_1314_1516_1718;
    const BALL_B: i64 = 0x0102_0304_0506_0708;
    const DUMP_TICK: u32 = 0x1122_3344;

    fn decoded(bytes: &[u8]) -> Park {
        Park::decode(bytes).expect("the client's own writer produced this")
    }

    /// **The property that matters most, over every vector we have.**
    ///
    /// Read the client's bytes, write them back, and the bytes are identical.
    /// That is the same test the client's own parser applied to us on
    /// 2026-10-21, in the other direction, and it covers every offset, width
    /// and endianness at once -- including the fields we have no name for,
    /// which is exactly where a hand-written expectation would be silently
    /// wrong.
    #[test]
    fn every_reference_blob_round_trips_byte_for_byte() {
        for (what, bytes) in [
            ("01-empty-park", EMPTY),
            ("02-empty-park-alt-tick", EMPTY_ALT),
            ("03-base-one-ball", ONE_BALL),
            ("04-two-balls", TWO_BALLS),
            ("10-arg-a5_i", NOT_FREE),
            ("66-call-Stop", STOP),
            ("67-call-FollowBall", FOLLOW),
            ("68-call-Orbit", ORBIT),
            ("69-call-GotoPoint", GOTO),
            ("71-call-SetBallTroll", TROLL),
        ] {
            let park = Park::decode(bytes)
                .unwrap_or_else(|| panic!("{what}: the client wrote it, we must read it"));
            assert_eq!(
                park.encode(),
                bytes,
                "{what}: re-encoding the client's own state must reproduce it"
            );
        }
    }

    /// The header is a type byte and a little-endian u32 clock, and nothing
    /// else -- the two empty parks differ in the clock alone.
    #[test]
    fn an_empty_park_is_a_five_byte_header() {
        assert_eq!(EMPTY.len(), 5);
        assert_eq!(EMPTY[0], PACKET_FULL_STATE);
        let a = decoded(EMPTY);
        let b = decoded(EMPTY_ALT);
        assert!(a.balls.is_empty() && b.balls.is_empty());
        assert_eq!(a.current_time, DUMP_TICK);
        assert_eq!(b.current_time, 0x5566_7788);
        assert_eq!(
            Park {
                current_time: b.current_time,
                ..a
            },
            b,
            "the only difference between the two dumps is the tick"
        );
    }

    /// Every field of the 100-byte record, against the values the dump was
    /// driven with.
    ///
    /// These numbers are not ours: they are the arguments the dump script
    /// passed to `AddBall`, chosen to be distinguishable, and reading them
    /// back out of the client's own serialisation is what pins each offset to
    /// each field. Nine of them were confirmed twice -- once by varying the
    /// `AddBall` argument and once by assigning the `destiny.Ball` attribute.
    #[test]
    fn one_stop_ball_matches_the_clients_own_writer() {
        assert_eq!(
            ONE_BALL.len(),
            105,
            "5-byte header plus one 100-byte record"
        );
        let park = decoded(ONE_BALL);
        assert_eq!(park.current_time, DUMP_TICK);
        let [ball]: [Ball; 1] = park.balls.try_into().expect("exactly one ball");
        assert_eq!(ball.ball_id, BALL_B);
        assert_eq!(ball.mode, Mode::Stop);
        assert_eq!(ball.radius, 56.25);
        assert_eq!(ball.position, [101.0, 202.0, 303.0]);
        assert_eq!(ball.flags, FLAGS_FREE_BALL);
        assert_eq!(ball.mass, 1234.5);
        assert_eq!(ball.cloak, 0);
        assert_eq!(ball.harmonic, -1);
        assert_eq!(ball.corporation_id, -1);
        assert_eq!(ball.alliance_id, -1);
        let p = ball
            .physics
            .expect("flags bit 0 is set, so the block is there");
        assert_eq!(p.max_velocity, 78.5);
        assert_eq!(p.velocity, [11.0, 22.0, 33.0]);
        assert_eq!(p.agility, 44.5);
        assert_eq!(p.speed_fraction, 55.25);
        assert_eq!(ball.formation_id, NO_FORMATION);
    }

    /// The trailing `0xff` is a **field**, not a terminator.
    ///
    /// Two balls, 205 bytes: `5 + 100 + 100`, with the second record starting
    /// at the byte after the first one's `0xff`. A decoder that treated it as
    /// a terminator would read one ball here and be wrong in a way that only
    /// shows up with company in space.
    #[test]
    fn the_record_stride_is_one_hundred_bytes_with_nothing_between() {
        assert_eq!(TWO_BALLS.len(), 205);
        let park = decoded(TWO_BALLS);
        assert_eq!(park.balls.len(), 2);
        assert_eq!(park.balls[0].ball_id, BALL_A);
        assert_eq!(park.balls[1].ball_id, BALL_B);
        assert_eq!(TWO_BALLS[0x68], NO_FORMATION);
        assert_eq!(
            &TWO_BALLS[0x69..0x71],
            &BALL_B.to_le_bytes(),
            "the next record begins immediately after the 0xff"
        );
    }

    /// `isFree` gates 36 bytes, and the `0xff` is written either way.
    ///
    /// 69 bytes, not 105: the physics block is absent and the record is 64.
    /// The `0xff` is still the last byte, immediately after `allianceID` --
    /// which is what puts it *outside* the gated block rather than inside it,
    /// and that is the one thing about this byte we do know.
    /// **CAPTURE-UNDOCK: no ship and no static is massive.** The owner's
    /// capture held the ego and every Hulk as `0x1f` and the station and
    /// statics as `0x1e` with mass 0.0 -- massive balls the client's native
    /// collides. TQ's ships are non-massive (EvEJS: `0x01`/`0x09`).
    #[test]
    fn no_ship_and_no_static_ball_is_massive() {
        let motion = crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]);
        let ship = ego_ball_of(&hull::FALLBACK, 1, 0, 0, &motion);
        assert_eq!(ship.flags, 0x1b, "free, global, interactive, junk");
        assert_eq!(ship.flags & FLAG_IS_MASSIVE, 0);
        assert!(ship.flags & FLAG_IS_FREE != 0, "a ship still has physics");
        for (what, flags) in [
            ("station", station_ball(2, 100.0, 0).flags),
            ("static", FLAGS_STATIC_BALL),
        ] {
            assert_eq!(flags, 0x1a, "{what}: not free, not massive");
        }
    }

    /// `SetBallMassive` goes out as `["SetBallMassive", [id, 0|1]]`, ints.
    #[test]
    fn set_ball_massive_is_two_ints() {
        let a = Action::SetBallMassive {
            ball: 77,
            massive: false,
        };
        assert_eq!(a.name(), "SetBallMassive");
        assert_eq!(a.args(), vec![PyValue::Int(77), PyValue::Int(0)]);
        let b = Action::SetBallMassive {
            ball: 77,
            massive: true,
        };
        assert_eq!(b.args()[1], PyValue::Int(1));
    }

    /// `SetBallInteractive` goes out as `["SetBallInteractive", [id, 0|1]]`, ints.
    #[test]
    fn set_ball_interactive_is_two_ints() {
        let a = Action::SetBallInteractive {
            ball: 77,
            interactive: false,
        };
        assert_eq!(a.name(), "SetBallInteractive");
        assert_eq!(a.ball(), 77);
        assert_eq!(a.args(), vec![PyValue::Int(77), PyValue::Int(0)]);
        let b = Action::SetBallInteractive {
            ball: 77,
            interactive: true,
        };
        assert_eq!(b.args(), vec![PyValue::Int(77), PyValue::Int(1)]);
    }

    #[test]
    fn a_ball_that_is_not_free_omits_the_physics_block() {
        assert_eq!(NOT_FREE.len(), 69, "5-byte header plus a 64-byte record");
        assert_eq!(NOT_FREE[0x2a] & FLAG_IS_FREE, 0, "bit 0 clear");
        assert_eq!(*NOT_FREE.last().expect("non-empty"), NO_FORMATION);
        let park = decoded(NOT_FREE);
        let ball = park.balls[0];
        assert_eq!(ball.flags, 0x1e, "the client's own dump");
        assert!(
            ball.physics.is_none(),
            "a ball with bit 0 clear has no velocity to read"
        );
        assert_eq!(ball.formation_id, NO_FORMATION);
    }

    /// Each mode writes its own tail, after the `0xff`, with the values the
    /// dump called for.
    ///
    /// The file sizes are the measurement: all five dumps are the same
    /// two-ball park plus one call, so the size difference from
    /// `66-call-Stop.bin` **is** the tail length.
    #[test]
    fn each_mode_tail_matches_the_dump() {
        for (what, bytes, want, size) in [
            ("STOP", STOP, Mode::Stop, 205),
            (
                "FOLLOW",
                FOLLOW,
                Mode::Follow {
                    ball_id: BALL_A,
                    range: 500.0,
                },
                217,
            ),
            (
                "ORBIT",
                ORBIT,
                Mode::Orbit {
                    ball_id: BALL_A,
                    range: 1000.0,
                },
                217,
            ),
            ("GOTO", GOTO, Mode::Goto([4004.0, 5005.0, 6006.0]), 229),
            (
                "TROLL",
                TROLL,
                Mode::Troll {
                    effect_stamp: DUMP_TICK + 1,
                },
                209,
            ),
        ] {
            assert_eq!(
                bytes.len(),
                size,
                "{what}: the tail length is the size delta"
            );
            let park = decoded(bytes);
            assert_eq!(park.balls.len(), 2, "{what}: both dumps are two-ball parks");
            assert_eq!(
                park.balls[0].mode,
                Mode::Stop,
                "{what}: the first ball is the untouched baseline"
            );
            assert_eq!(park.balls[1].mode, want, "{what}: the tail we decoded");
        }
    }

    /// A ball in WARP goes out non-massive whatever its stored flags say:
    /// a massive warper is collided by the client's native pass, which
    /// overwrites the tail's length slot and makes every observer log
    /// `Ship stuck in extended warp` each tick (2026-10-28 client logs, all
    /// 0x1f balls). Other modes keep their flags.
    #[test]
    fn a_warping_ball_is_encoded_non_massive() {
        let mut ball = Ball {
            ball_id: BALL_A,
            mode: Mode::Warp {
                goto: [1e9, 0.0, 0.0],
                effect_stamp: 1_000,
                total_distance: 5.0e8,
                stop_distance: 5_000.0,
                warp_factor: 3_500,
            },
            radius: 2.0,
            position: [1.0, 2.0, 3.0],
            flags: FLAGS_FREE_BALL,
            mass: 32_000.0,
            cloak: 0,
            harmonic: 0,
            corporation_id: -1,
            alliance_id: -1,
            physics: Some(Physics {
                max_velocity: 200.0,
                velocity: [0.0; 3],
                agility: 1.0,
                speed_fraction: 1.0,
            }),
            formation_id: NO_FORMATION,
        };
        let flags_of = |b: &Ball| {
            Park {
                current_time: 1_010,
                balls: vec![*b],
            }
            .encode()[5 + 8 + 1 + 4 + 24]
        };
        assert_eq!(flags_of(&ball) & FLAG_IS_MASSIVE, 0, "warp: not massive");
        ball.mode = Mode::Stop;
        assert_ne!(flags_of(&ball) & FLAG_IS_MASSIVE, 0, "other modes keep it");
    }

    /// **The WARP tail is the 52 bytes the client's own writer produced**,
    /// field by field at the offsets it put them at.
    ///
    /// There is no reference dump for this one and there cannot be: the only
    /// warp blob we have was written by a live client about a park holding
    /// CCP's own celestial geometry, which does not go in this repository.
    /// `tools/client-ci/scenarios/warp_blob.py` is the measurement and it
    /// re-runs against the client; this is the layout it measured, pinned
    /// where the encoder can be held to it. `docs/port-tracker.md` §38 has
    /// the numbers the client gave, including the two it gave twice.
    ///
    /// The length is asserted the same way every other tail was measured --
    /// as the size difference against the identical ball in `STOP`, which
    /// writes no tail at all.
    #[test]
    fn the_warp_tail_is_fifty_two_bytes_in_the_order_the_client_wrote_them() {
        let base = Ball {
            ball_id: BALL_A,
            mode: Mode::Stop,
            radius: 2.0,
            position: [1.0, 2.0, 3.0],
            flags: 0,
            mass: 32_000.0,
            cloak: 0,
            harmonic: 0,
            corporation_id: -1,
            alliance_id: -1,
            physics: None,
            formation_id: NO_FORMATION,
        };
        let warp = Mode::Warp {
            goto: [1e12, -2e11, 3.5e11],
            effect_stamp: -2,
            // Negative stamp, so this ball is aligning and has no length
            // yet. That is not a detail of the fixture: it is why +28 read
            // `-1.0` in every blob we ever captured.
            total_distance: Mode::warp_total_distance(-2, 9_999.0),
            stop_distance: 4_071_000.0,
            warp_factor: 4_000,
        };
        let parked = Park {
            current_time: 1_792_801_503,
            balls: vec![base],
        }
        .encode();
        let warping = Park {
            current_time: 1_792_801_504,
            balls: vec![Ball { mode: warp, ..base }],
        }
        .encode();
        assert_eq!(
            warping.len() - parked.len(),
            52,
            "the tail length is the size delta against STOP, which is how the \
             client was asked: parked {} bytes, warping {} bytes",
            parked.len(),
            warping.len()
        );

        // The tail starts immediately after the `formationID` byte, which is
        // the last byte of the parked record -- so the parked encoding's
        // length is exactly the offset of the tail. Reading them the other
        // way round is the mistake the first hand decode made: the tail then
        // appears to start with `0xff` and to be one byte longer.
        let tail = &warping[parked.len()..];
        assert_eq!(tail.len(), 52);
        let f64_at =
            |off: usize| f64::from_le_bytes(tail[off..off + 8].try_into().expect("8 bytes"));
        assert_eq!(f64_at(0), 1e12, "+0 goto X");
        assert_eq!(f64_at(8), -2e11, "+8 goto Y");
        assert_eq!(f64_at(16), 3.5e11, "+16 goto Z");
        assert_eq!(
            i32::from_le_bytes(tail[24..28].try_into().expect("4 bytes")),
            -2,
            "+24 effectStamp, signed: negative is 'still aligning' and the \
             client tests exactly that"
        );
        assert_eq!(
            f64_at(28),
            WARP_LENGTH_ALIGNING,
            "+28 is the warp's total length, and an aligning ball has none"
        );
        assert_eq!(f64_at(36), 4_071_000.0, "+36 stop distance from the centre");
        assert_eq!(
            i64::from_le_bytes(tail[44..52].try_into().expect("8 bytes")),
            4_000,
            "+44 warp factor, AU/s x 1000"
        );

        // And the reader agrees with the writer, which is what keeps the two
        // halves of this from drifting apart.
        let back = Park::decode(&warping).expect("a park we just wrote");
        assert_eq!(back.balls.len(), 1);
        assert_eq!(back.balls[0].mode, warp, "the tail round-trips");
    }

    /// **A warping ball's `goto` is where it will stop, once it is actually
    /// warping** -- and the commanded centre until then.
    ///
    /// Not a preference. Our `WarpTo` action sends the raw centre with the
    /// standoff in a separate field, and the client's writer echoed that
    /// centre to the metre while the ball was aligning, then wrote the centre
    /// pulled back by *exactly* the stop distance once the warp had activated
    /// (§38.1, measured at 5 061 000.0 m against 5 061 000.0 m). The
    /// discriminator is the stamp's sign, not which command came first.
    /// **An activated warp writes its real length at +28, and only an
    /// aligning one writes the sentinel.**
    ///
    /// The case this project never captured. Every blob we dumped had a
    /// negative stamp, so `-1.0` at +28 looked like a constant for three
    /// sections; §46 settled it against CCP's own serialiser, whose comment
    /// for that slot is "Total length of warp". The two states are asserted
    /// together here because the pair is the finding -- a test that only
    /// pinned the sentinel would have passed all along and taught nothing.
    #[test]
    fn a_warps_total_length_is_written_once_it_has_one() {
        // Aligning: no length yet, whatever the caller knows.
        assert_eq!(
            Mode::warp_total_distance(-2, 4.2e11),
            WARP_LENGTH_ALIGNING,
            "a ball that has not activated has no length to report"
        );
        assert_eq!(Mode::warp_total_distance(-1, 0.0), WARP_LENGTH_ALIGNING);

        // Activated: the length itself. Zero is a valid stamp here -- the
        // sign test is `< 0`, matching `michelle.IsPreparingWarp`.
        assert_eq!(Mode::warp_total_distance(0, 4.2e11), 4.2e11);
        assert_eq!(Mode::warp_total_distance(1_792_801_958, 4.2e11), 4.2e11);

        // And it reaches the wire at +28, not just the struct.
        let length = 4.2e11;
        let hull = &hull::FALLBACK;
        let ball = Ball {
            ball_id: 100_000_001,
            mode: Mode::Warp {
                goto: [1e12, -2e11, 3.5e11],
                effect_stamp: 1_792_801_958,
                total_distance: Mode::warp_total_distance(1_792_801_958, length),
                stop_distance: 4_071_000.0,
                warp_factor: 4_000,
            },
            position: [0.0; 3],
            radius: hull.radius,
            flags: 0,
            mass: hull.mass,
            cloak: 0,
            harmonic: 0,
            corporation_id: -1,
            alliance_id: -1,
            physics: None,
            formation_id: NO_FORMATION,
        };
        // **The park clock has to be at or past the ball's activation, and
        // this fixture used to have it 455 seconds *before*.** The stamp
        // `1792801958` is measured -- §38.1 records it with the words "after
        // activation" -- but the `current_time` beside it was a number
        // somebody chose, and the pair was incoherent: a ball that had
        // activated, in a park whose clock said the activation was still
        // minutes away. Nothing could notice until §59 made the encoder read
        // that clock, and then this test failed with `-1.0` against
        // `420000000000.0`.
        //
        // Worth recording rather than quietly editing, because the lesson is
        // about fixtures and not about warps: **a fixture assembled from one
        // measured field and one invented one is not a measurement**, and it
        // will pass every test written against either field alone.
        let blob = Park {
            current_time: 1_792_802_013,
            balls: vec![ball],
        }
        .encode();
        let back = Park::decode(&blob).expect("our own blob reads back");
        match back.balls[0].mode {
            Mode::Warp { total_distance, .. } => assert_eq!(
                total_distance, length,
                "the length did not survive the round trip"
            ),
            ref other => panic!("expected a warp: {other:?}"),
        }
    }

    /// **A blob never claims a length for a warp that has not started.**
    ///
    /// The discriminator is the blob's own `current_time`, which is the field
    /// the client adopts as `mCurrentTime` (§56.1), so the question "has this
    /// warp activated" is one the blob answers about itself. §59 made this
    /// reachable by storing a *predicted* activation stamp; before that no
    /// record ever held an unactivated warp and the encoder could write the
    /// stored value unconditionally.
    ///
    /// Asserted at the byte level rather than through the decoder, because
    /// what matters is what goes on the wire -- a round trip through our own
    /// reader would agree with our own writer whatever both did.
    #[test]
    fn an_unactivated_warp_is_encoded_with_no_length() {
        let ball = |stamp: i32| Ball {
            ball_id: 100_000_001,
            mode: Mode::Warp {
                goto: [1.0, 2.0, 3.0],
                effect_stamp: stamp,
                total_distance: 987_654.0,
                stop_distance: 1_000.0,
                warp_factor: 4_000,
            },
            radius: 27.0,
            position: [0.0; 3],
            flags: 0,
            mass: 1.0e6,
            cloak: 0,
            harmonic: 0,
            corporation_id: 0,
            alliance_id: 0,
            physics: None,
            formation_id: 0xff,
        };
        // **Anchored on the goto triple, not on the stamp.** Searching for
        // the stamp's bytes found the *park header's* `current_time` instead
        // when the two happened to be equal, and read the length from twelve
        // bytes into the wrong field -- `4.94e-316`, which is what a float
        // made of someone else's bytes looks like. The goto is three distinct
        // doubles and unmistakable, and §46's offsets are measured from it:
        // stamp at +24, length at +28.
        let length_of = |stamp: i32| -> f64 {
            let blob = Park {
                current_time: 1_000,
                balls: vec![ball(stamp)],
            }
            .encode();
            let mut needle = Vec::new();
            for v in [1.0_f64, 2.0, 3.0] {
                needle.extend_from_slice(&v.to_le_bytes());
            }
            let at = blob
                .windows(needle.len())
                .position(|w| w == needle.as_slice())
                .expect("the goto triple is in the blob");
            let tail = at + needle.len();
            assert_eq!(
                i32::from_le_bytes(blob[tail..tail + 4].try_into().expect("4 bytes")),
                stamp,
                "the stamp sits immediately after the goto (§46 +24)"
            );
            f64::from_le_bytes(blob[tail + 4..tail + 12].try_into().expect("8 bytes"))
        };

        assert_eq!(
            length_of(900),
            987_654.0,
            "a warp activated 100 ticks ago carries its real length"
        );
        assert_eq!(
            length_of(1_000),
            987_654.0,
            "activating exactly now counts as activated"
        );
        // A warp that activates 100 ticks from now is *aligning* to an
        // observer: stamp -1, the commanded centre, no length (see
        // `a_warp_still_aligning_is_written_as_the_client_writes_it`).
        let future = Park::decode(
            &Park {
                current_time: 1_000,
                balls: vec![ball(1_100)],
            }
            .encode(),
        )
        .expect("decodes");
        match future.balls[0].mode {
            Mode::Warp {
                effect_stamp,
                total_distance,
                ..
            } => {
                assert_eq!(effect_stamp, -1);
                assert_eq!(total_distance, WARP_LENGTH_ALIGNING);
            }
            ref other => panic!("expected a warp: {other:?}"),
        }
        assert_eq!(
            length_of(-1),
            WARP_LENGTH_ALIGNING,
            "and neither does one the client itself marks as preparing"
        );
    }

    /// **CAPTURE-UNDOCK: a warp still aligning reaches an observer as the
    /// client writes one** -- negative stamp, the commanded centre, no
    /// length -- and never as an "activated" warp whose activation tick has
    /// not come. The owner's capture held `WARP(stamp +880, len -1)` in a
    /// `SetState` stamped 879: the native evaluates `WarpDistance` for such a
    /// ball with no length, a NaN position it discards in silence.
    #[test]
    fn a_warp_still_aligning_is_written_as_the_client_writes_it() {
        let from = [0.0, 0.0, 0.0];
        let centre = [10_000.0, 0.0, 0.0];
        let stop = 1_000.0;
        // What the server stores once it has predicted the activation.
        let arrival = warp_arrival(from, centre, stop);
        let ball = Ball {
            ball_id: 100_000_002,
            mode: Mode::Warp {
                goto: arrival,
                effect_stamp: 1_005,
                total_distance: 9_000.0,
                stop_distance: stop,
                warp_factor: 3_500,
            },
            position: from,
            radius: 20.0,
            flags: 0,
            mass: 1.0e6,
            cloak: 0,
            harmonic: 0,
            corporation_id: 0,
            alliance_id: 0,
            physics: None,
            formation_id: NO_FORMATION,
        };
        let read = |clock: u32| {
            let back = Park::decode(
                &Park {
                    current_time: clock,
                    balls: vec![ball],
                }
                .encode(),
            )
            .expect("decodes");
            back.balls[0].mode
        };
        let Mode::Warp {
            goto,
            effect_stamp,
            total_distance,
            ..
        } = read(1_000)
        else {
            panic!("a warp")
        };
        assert!(effect_stamp < 0, "preparing is a negative stamp");
        assert_eq!(total_distance, WARP_LENGTH_ALIGNING);
        assert!(
            crate::space_state::distance(goto, centre) < 1e-6,
            "the commanded centre, not the arrival point: {goto:?}"
        );
        // Once the clock reaches the activation it is the stored warp.
        let Mode::Warp {
            goto,
            effect_stamp,
            total_distance,
            ..
        } = read(1_005)
        else {
            panic!("a warp")
        };
        assert_eq!((effect_stamp, total_distance), (1_005, 9_000.0));
        assert_eq!(goto, arrival);
    }

    #[test]
    fn a_warps_goto_follows_the_stamps_sign() {
        let from = [0.0, 0.0, 0.0];
        let centre = [1000.0, 0.0, 0.0];
        let stop = 400.0;

        assert_eq!(
            Mode::warp_goto(from, centre, stop, -2),
            centre,
            "aligning: the client echoes the centre we commanded"
        );
        let arrived = Mode::warp_goto(from, centre, stop, 1_792_801_958);
        assert_eq!(
            arrived,
            [600.0, 0.0, 0.0],
            "activated: the standoff is folded in, on the side the ship \
             approaches from"
        );
        let gap = crate::space_state::distance(arrived, centre);
        assert!(
            (gap - stop).abs() < 1e-6,
            "the arrival point is exactly the stop distance from the centre, \
             which is the identity the client's own numbers satisfied: {gap} \
             against {stop}"
        );
        // Zero is "activated", not "aligning": the client's test is `< 0`.
        assert_ne!(
            Mode::warp_goto(from, centre, stop, 0),
            centre,
            "a zero stamp is not negative, and IsPreparingWarp tests `< 0`"
        );
    }

    /// A blob we cannot read is refused, and the three ways that happens are
    /// each distinct from "the park ended".
    ///
    /// The native reader stops when its first 8-byte read comes up short, so
    /// a trailing fragment ends the list -- but a record that *starts* and
    /// does not finish is corruption, and a mode we have no tail length for
    /// cannot be skipped because the tail length is not in the bytes.
    #[test]
    fn an_unreadable_blob_is_refused_rather_than_truncated() {
        assert!(
            Park::decode(&[1, 0, 0, 0, 0]).is_none(),
            "packet type 1 is WriteBallsToStream, which has no header"
        );
        assert!(Park::decode(&[]).is_none(), "no header at all");
        assert!(
            Park::decode(&ONE_BALL[..0x40]).is_none(),
            "a record that starts and does not finish is not an empty park"
        );
        let mut unknown_mode = ONE_BALL.to_vec();
        unknown_mode[0x0d] = 3;
        assert!(
            Park::decode(&unknown_mode).is_none(),
            "mode 3 is one of the four whose tail we have never measured, so \
             its length is unknown and every later record would be mis-framed"
        );
        let mut fragment = ONE_BALL.to_vec();
        fragment.extend_from_slice(&[0xff; 7]);
        assert!(
            Park::decode(&fragment).is_some(),
            "fewer than eight trailing bytes is where the client stops reading"
        );
    }

    /// The action's stamp and the blob's clock are one number.
    ///
    /// `Bootstrap::new` is the only constructor for exactly this: the stamp
    /// decides *when* the client flushes the batch and the header decides what
    /// its clock becomes when it does, so two different values start the park
    /// out of step with every action that follows.
    #[test]
    fn a_bootstraps_stamp_and_blob_clock_are_the_same_number() {
        let park = Park {
            current_time: 7,
            balls: vec![ego_ball_in(
                &crate::content::Content::fixture(),
                100_000_001,
                670,
                1_000_126,
                -1,
                &parked(),
            )],
        };
        let state = Bootstrap::new(999, park, 100_000_001, vec![], vec![]);
        assert_eq!(state.park.current_time, 999, "the constructor overrides it");
        let (_, args) = state.to_push();
        let PyValue::List(actions) = &args[0] else {
            panic!("the batch is a sequence")
        };
        let PyValue::Tuple(action) = &actions[0] else {
            panic!("an action is (stamp, (name, args))")
        };
        assert_eq!(action[0], PyValue::Int(999));
        let decoded_blob = {
            let PyValue::Tuple(call) = &action[1] else {
                panic!("(name, args)")
            };
            let PyValue::Tuple(call_args) = &call[1] else {
                panic!("args is a tuple")
            };
            let state_bytes = match crate::pyutil::dict_get(
                match &call_args[0] {
                    PyValue::Instance { state, .. } => state,
                    other => panic!("the bag is a KeyVal instance: {other:?}"),
                },
                b"state",
            ) {
                Some(PyValue::Bytes(b)) => b.clone(),
                other => panic!("bag.state is a byte string: {other:?}"),
            };
            Park::decode(&state_bytes).expect("our own blob")
        };
        assert_eq!(
            decoded_blob.current_time, 999,
            "the blob's header clock is the action's stamp"
        );
    }

    /// The `bag` the client unpacks: one `SetState`, `waitForBubble` false,
    /// and the three attributes that must not be `None`.
    ///
    /// Each assertion is a client rule rather than a fact about our code --
    /// the batch length is `RealFlushState`'s, the falsy flag is
    /// `DoPreTick`'s, and the three containers are the ones `SetState`
    /// iterates before testing anything.
    #[test]
    fn the_bootstrap_payload_obeys_the_four_client_rules() {
        let ship = 100_000_001;
        let state = Bootstrap::new(
            42,
            Park {
                current_time: 42,
                balls: vec![ego_ball_in(
                    &crate::content::Content::fixture(),
                    ship,
                    670,
                    1_000_126,
                    -1,
                    &parked(),
                )],
            },
            ship,
            vec![Slim {
                item_id: ship,
                type_id: 670,
                owner_id: 90_000_001,
                char_id: Some(90_000_001),
                corp_id: Some(1_000_126),
                response_thresholds: None,
            }],
            vec![(ship, DamageState::FULL.to_pyvalue())],
        );
        let (event, args) = state.to_push();
        assert_eq!(event, "DoDestinyUpdate");
        assert_eq!(
            args.len(),
            2,
            "no third argument: dogmaMessages fires OnMultiEvent"
        );
        assert_eq!(args[1], PyValue::Int(0), "waitForBubble must be falsy");
        let PyValue::List(actions) = &args[0] else {
            panic!("the batch is a sequence")
        };
        assert_eq!(
            actions.len(),
            1,
            "SetState is honoured only as state[0], and >1 distinct stamp is \
             a fatal desync"
        );
        let PyValue::Tuple(action) = &actions[0] else {
            panic!("(stamp, call)")
        };
        let PyValue::Tuple(call) = &action[1] else {
            panic!("(name, args)")
        };
        assert_eq!(call[0], crate::pyutil::bstr("SetState"));
        let PyValue::Tuple(call_args) = &call[1] else {
            panic!("args is a tuple")
        };
        let bag = match &call_args[0] {
            PyValue::Instance { class, state } => {
                assert_eq!(class, crate::pyutil::KEYVAL_CLASS.as_bytes());
                state.as_ref()
            }
            other => panic!("the bag is a KeyVal: {other:?}"),
        };
        let attr = |name: &[u8]| {
            crate::pyutil::dict_get(bag, name)
                .unwrap_or_else(|| panic!("bag.{} is missing", String::from_utf8_lossy(name)))
        };
        assert_eq!(attr(b"ego"), &PyValue::Int(ship));
        assert!(matches!(attr(b"droneState"), PyValue::List(_)));
        assert!(matches!(attr(b"damageState"), PyValue::Dict(_)));
        assert!(matches!(attr(b"effectStates"), PyValue::List(_)));
        // `slims` names the ego ball, or `SetState` raises `BallNotInPark`
        // and the whole state is lost.
        let PyValue::List(slims) = attr(b"slims") else {
            panic!("slims is iterated")
        };
        assert_eq!(slims.len(), 1);
        let PyValue::Instance {
            class,
            state: slim_attrs,
        } = &slims[0]
        else {
            panic!("a slim item is an instance")
        };
        assert_eq!(class, b"foo.SlimItem");
        assert_eq!(
            crate::pyutil::dict_get(slim_attrs, b"itemID"),
            Some(&PyValue::Int(ship)),
            "the slim item must name a ball the blob created"
        );
        assert!(
            crate::pyutil::dict_get(slim_attrs, b"charID").is_some_and(|v| *v != PyValue::None),
            "ProcessBallAdd primes the pilot's name only when charID is not None"
        );
        assert!(state.every_slim_has_a_ball());
    }

    /// A state whose slim item names no ball is not sendable.
    ///
    /// `BallNotInPark` is raised by `SetState` before anything is drawn and it
    /// loses the entire state, so the check exists to refuse locally rather
    /// than be refused remotely. The property is the *detection*; the caller's
    /// use of it is in `on_ballpark_bind`.
    #[test]
    fn a_slim_item_naming_no_ball_is_detected() {
        let state = Bootstrap::new(
            1,
            Park {
                current_time: 1,
                balls: vec![ego_ball_in(
                    &crate::content::Content::fixture(),
                    100_000_001,
                    670,
                    -1,
                    -1,
                    &parked(),
                )],
            },
            100_000_001,
            vec![Slim {
                item_id: 999_999,
                type_id: 670,
                owner_id: 90_000_001,
                char_id: Some(90_000_001),
                corp_id: None,
                response_thresholds: None,
            }],
            vec![],
        );
        assert!(!state.every_slim_has_a_ball());
    }

    /// The ego ball is free, parked, and carries the ship's own item id.
    ///
    /// Free because the physics block is what the client's simulation
    /// evolves -- a ball without it cannot be flown -- and parked because a
    /// ship that has just undocked is not going anywhere until the player
    /// asks.
    #[test]
    fn the_ego_ball_is_a_free_parked_ball_with_the_ships_id() {
        let ball = ego_ball_in(
            &crate::content::Content::fixture(),
            100_000_001,
            670,
            1_000_126,
            -1,
            &crate::space_state::Motion::arrived([UNDOCK_OFFSET, 0.0, 0.0], [0.0; 3]),
        );
        assert_eq!(ball.ball_id, 100_000_001);
        assert_eq!(ball.mode, Mode::Stop);
        // Off the station, not at the origin: the origin is the middle of a
        // 15.7 km station model now that the station is a ball.
        assert_eq!(ball.position, [UNDOCK_OFFSET, 0.0, 0.0]);
        assert!(
            f64::from(STATION_RADIUS) < UNDOCK_OFFSET,
            "clear of the hull"
        );
        assert_eq!(ball.corporation_id, 1_000_126);
        let p = ball.physics.expect("an ego ball must be free");
        assert_eq!(p.velocity, [0.0, 0.0, 0.0]);
        assert_eq!(p.speed_fraction, 0.0);
        // And the encoder agrees with the flag byte, which is the invariant
        // `physics: Option` exists to hold.
        let blob = Park {
            current_time: 0,
            balls: vec![ball],
        }
        .encode();
        assert_eq!(blob.len(), 105);
        assert_eq!(blob[0x2a] & FLAG_IS_FREE, FLAG_IS_FREE);
    }

    /// `physics` decides bit 0, not the stored flag byte.
    ///
    /// The two could disagree -- the struct has both -- and on the wire they
    /// must not, because bit 0 is what tells the reader how many bytes the
    /// record is. The property is that the *data* wins over the flag.
    #[test]
    fn the_physics_block_decides_the_free_bit() {
        let mut ball = ego_ball_in(
            &crate::content::Content::fixture(),
            1,
            670,
            -1,
            -1,
            &parked(),
        );
        ball.physics = None;
        // Flags still say free; the data says otherwise.
        ball.flags = FLAGS_FREE_BALL;
        let blob = Park {
            current_time: 0,
            balls: vec![ball],
        }
        .encode();
        assert_eq!(blob.len(), 69, "no physics block means a 64-byte record");
        assert_eq!(blob[0x2a] & FLAG_IS_FREE, 0, "and bit 0 cleared to match");
        assert_eq!(
            Park::decode(&blob).expect("round trip").balls[0].physics,
            None
        );
    }

    /// A tick is a plausible wall clock, which is all the first state needs.
    #[test]
    fn the_tick_is_unix_seconds() {
        let t = tick();
        // 2026-10-21 is 1 792 e9 ms; any build of this running before 2020 or
        // after 2106 has bigger problems than this assertion.
        assert!(t > 1_577_836_800, "after 2020-01-01: {t}");
    }

    /// The hull numbers are **looked up**, not constant.
    ///
    /// The wrong version of this test asserts the specific numbers for one
    /// hull, which passes just as well against a hardcoded table -- so this
    /// asserts that two different hulls differ in all four, which a constant
    /// cannot do. The capsule and the Rifter are the two the sandbox can
    /// actually produce.
    #[test]
    fn the_ego_balls_hull_numbers_come_from_the_type_not_a_constant() {
        let pod = ego_ball_in(
            &crate::content::Content::fixture(),
            100_000_001,
            670,
            -1,
            -1,
            &parked(),
        );
        let rifter = ego_ball_in(
            &crate::content::Content::fixture(),
            100_000_002,
            587,
            -1,
            -1,
            &parked(),
        );
        assert_ne!(pod.mass, rifter.mass);
        assert_ne!(pod.radius, rifter.radius);
        let (a, b) = (pod.physics.expect("free"), rifter.physics.expect("free"));
        assert_ne!(a.max_velocity, b.max_velocity);
        assert_ne!(a.agility, b.agility);
    }

    /// The capsule's four numbers, because it is the hull the first undocked
    /// ship flies and because the MVP's placeholders were **wrong for exactly
    /// it**: 1e6 kg against 32,000, radius 1.0 against 2.0.
    #[test]
    fn the_capsule_carries_its_own_numbers() {
        let pod = ego_ball_in(
            &crate::content::Content::fixture(),
            100_000_001,
            670,
            -1,
            -1,
            &parked(),
        );
        let want = hull::CAPSULE;
        assert_eq!(pod.mass, want.mass);
        assert_eq!(pod.radius, want.radius);
        let p = pod.physics.expect("an ego ball is free");
        assert_eq!(p.max_velocity, want.max_velocity);
        assert_eq!(p.agility, want.agility);
    }

    /// A hull the table does not know gets the native initialisers, which are
    /// visibly a default rather than a plausible wrong ship.
    #[test]
    fn an_unknown_hull_falls_back_to_the_natives_own_initialisers() {
        assert!(hull::find_in(&crate::content::Content::fixture(), 1).is_none());
        let ball = ego_ball_in(
            &crate::content::Content::fixture(),
            100_000_001,
            1,
            -1,
            -1,
            &parked(),
        );
        assert_eq!(ball.mass, hull::FALLBACK.mass);
        assert_eq!(ball.radius, hull::FALLBACK.radius);
    }

    /// The per-ball state is `(shieldSequence, armour, hull)`.
    ///
    /// The arity only. **What the numbers mean is pinned by
    /// `a_full_health_ship_reads_as_three_full_bars`**, which runs the
    /// client's own decoder -- because this test, in its previous form,
    /// asserted that the shield slot had four elements with the health at
    /// index 2, and that belief was wrong in a way no shape assertion could
    /// catch: the client reads `shieldValues[:2]`, so it was reading our
    /// padding zero as the shield health and drawing an empty red bar on an
    /// undamaged ship.
    #[test]
    fn the_per_ball_state_is_a_shield_sequence_and_two_floats() {
        let PyValue::Tuple(state) = DamageState::FULL.to_pyvalue() else {
            panic!("the per-ball state is a sequence")
        };
        assert_eq!(state.len(), 3, "shield, armour, hull");
        let PyValue::Tuple(shield) = &state[0] else {
            panic!(
                "a non-sequence shield decodes to None and HIDES the gauge: {:?}",
                state[0]
            )
        };
        assert_eq!(
            shield.len(),
            3,
            "CCP's own producer sends three: (health, tau, simTime)"
        );
        assert!(matches!(state[1], PyValue::Real(_)));
        assert!(matches!(state[2], PyValue::Real(_)));
    }

    /// The bootstrap carries a damage entry **for the ship**, because a ball
    /// with no entry has no gauges rather than full ones.
    ///
    /// `michelle.GetDamageState` returns `None` for a ball it has no entry
    /// for, which is indistinguishable from a ball at zero health as far as
    /// the HUD is concerned -- except that it hides the bars instead of
    /// emptying them. An empty `damageState` dict was therefore not "safe",
    /// which is what the spec called it.
    #[test]
    fn the_bootstrap_gives_the_ship_health_and_the_station_none() {
        let mut reg = Registry::new(crate::model::SandboxConfig::default());
        let actor = reg.config().characters[0].character_id;
        let ship = reg.character(actor).expect("docked").ship_id;
        let station = reg
            .character(actor)
            .expect("docked")
            .station_id
            .expect("docked");
        let state = bootstrap_for(&mut reg, actor).expect("a docked character has a state");
        let (_, args) = state.to_push();
        let PyValue::List(actions) = &args[0] else {
            panic!("the batch is a sequence")
        };
        let PyValue::Tuple(action) = &actions[0] else {
            panic!("(stamp, call)")
        };
        let PyValue::Tuple(call) = &action[1] else {
            panic!("(name, args)")
        };
        let PyValue::Tuple(call_args) = &call[1] else {
            panic!("args")
        };
        let PyValue::Instance { state: bag, .. } = &call_args[0] else {
            panic!("the bag")
        };
        let PyValue::Dict(entries) =
            crate::pyutil::dict_get(bag, b"damageState").expect("damageState is read by name")
        else {
            panic!("`.iteritems()` needs a dict")
        };
        assert_eq!(
            entries.len(),
            1,
            "the ship has health; the station has none"
        );
        assert_eq!(
            entries[0].1,
            PyValue::Int(ship),
            "keyed by ball id, and the ball is the ship"
        );
        assert_ne!(
            entries[0].1,
            PyValue::Int(station),
            "a station has no damage gauges"
        );
        let _ = reg.take_pending();
    }

    /// **An orbiting ship stays at the thing it is orbiting.**
    ///
    /// `Motion::advanced` alone cannot do this: an orbit's position depends
    /// on where the *other* ball is, and a `Motion` does not know. So it
    /// cruises in a straight line, and a pilot who joined a minute into
    /// somebody's orbit was shown that ship nine kilometres downrange.
    ///
    /// The property is **bounded, not exact**: after any amount of orbiting,
    /// the ship is about `range` from its target. Where around it is still
    /// `[A]` -- the orbit plane and swept angle are the client's business and
    /// we are never told either -- so this deliberately asserts a shell and
    /// not a point. The tempting wrong version asserts a position, which
    /// would pin a limitation in place and fail the moment the approximation
    /// improved.
    #[test]
    fn an_orbiting_ship_stays_at_what_it_orbits_rather_than_cruising_away() {
        const ALICE: i64 = 1_000_001;
        let alice = crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: 100_000_001,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![alice],
            ..crate::model::SandboxConfig::default()
        });
        on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();

        let station_id = reg
            .character(ALICE)
            .expect("Alice exists")
            .station_id
            .expect("docked somewhere");
        const RANGE: f64 = 2_500.0;
        {
            let p = reg.roster_mut().get_mut(ALICE).expect("in space");
            p.motion.after(Action::Orbit {
                ball: p.ship_id,
                target: station_id,
                range: RANGE,
            });
            p.motion.after(Action::SetSpeedFraction {
                ball: p.ship_id,
                fraction: 1.0,
            });
            p.since -= 60.0;
        }

        reg.advance_system(
            reg.character(ALICE)
                .expect("Alice exists")
                .solar_system_id
                .expect("in a system"),
        );
        let position = reg
            .roster()
            .get(ALICE)
            .expect("still in space")
            .motion
            .position;
        let from_station = crate::space_state::distance([0.0; 3], position);
        // **The range is the gap between the hulls, not the distance between
        // the centres.** This asserted `from_station == RANGE` until the
        // orbit law was read out of Carbon (`docs/port-tracker.md` §54): a
        // ship 2,500 m from the station's *origin* is 13 km inside it. The
        // law is stated here as subtraction rather than by calling
        // `orbit::centre_distance`, so that the test says what the client
        // does instead of agreeing with our transcription of it.
        let ship_radius = f64::from(
            hull::find_in(
                reg.content(),
                reg.character(ALICE).expect("Alice exists").ship_type_id,
            )
            .expect("Alice's hull is in the table")
            .radius,
        );
        let gap = from_station - f64::from(STATION_RADIUS) - ship_radius;
        assert!(
            (gap - RANGE).abs() < 1.0,
            "a minute into a {RANGE} m orbit the ship has {gap} m of space \
             between its hull and the station it is circling, at \
             {from_station} m from the station's centre"
        );
        let _ = reg.take_pending();
    }

    /// **A pilot undocks at the mouth however many ships are out in the
    /// system** (UNDOCK-REG). The berth index was the head count of the whole
    /// system, so with a crowd mining far away the owner left the station
    /// `n * BERTH` sideways of it, outside tether range; the bots' balls were
    /// strewn the same way. Only a ship sitting near the mouth takes a berth.
    #[test]
    fn a_crowd_far_from_the_station_does_not_push_an_undock_off_the_mouth() {
        const ALICE: i64 = 1_000_001;
        let chars: Vec<crate::model::Character> = std::iter::once(ALICE)
            .chain(1_000_002..1_000_032)
            .map(|id| crate::model::Character {
                character_id: id,
                name: format!("P{id}"),
                ship_id: id + 100_000_000,
                ..crate::model::Character::default()
            })
            .collect();
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: chars,
            ..crate::model::SandboxConfig::default()
        });
        let (_, alone) = undock_motion(&reg, ALICE).expect("a mouth");
        let home = reg
            .character(ALICE)
            .and_then(|c| c.solar_system_id)
            .expect("home");
        for id in 1_000_002..1_000_032_i64 {
            let far = crate::space_state::Motion::arrived([9.0e9, 0.0, 0.0], [0.0; 3]);
            reg.roster_mut()
                .arrive(crate::space_state::Presence::arriving(
                    id,
                    id + 100_000_000,
                    home,
                    far,
                ));
        }
        let (_, with_crowd) = undock_motion(&reg, ALICE).expect("a mouth");
        assert_eq!(
            with_crowd.position, alone.position,
            "30 ships 9e9 m away must not move the undock point"
        );
        // And a ship sitting on the mouth does take it.
        let squatter = crate::space_state::Motion::arrived(alone.position, [0.0; 3]);
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                1_000_002,
                100_000_000 + 1_000_002,
                home,
                squatter,
            ));
        let (_, shifted) = undock_motion(&reg, ALICE).expect("a mouth");
        assert!(crate::space_state::distance(shifted.position, alone.position) > BERTH * 0.99);
    }

    /// **A crowd at the mouth moves an undock a couple of berths, not
    /// kilometres** (UNDOCK-2). In the real client the owner's ego was
    /// 6 km sideways of the mouth (8 berths of a line) with the bots
    /// undocking around it.
    #[test]
    fn a_crowd_on_the_berths_leaves_the_next_ship_within_a_few_berths() {
        let station = [1.0e9, 2.0e9, -3.0e9];
        let mouth = [
            station[0] + 2_000.0,
            station[1] - 3_000.0,
            station[2] + 1_000.0,
        ];
        let r0 = crate::space_state::distance(mouth, station);
        for crowd in [8_usize, 24, 48] {
            let pilots: Vec<[f64; 3]> = (0..crowd).map(|i| berth(mouth, station, i)).collect();
            let at = free_berth(mouth, station, &pilots);
            let off = crate::space_state::distance(at, mouth);
            // The patch holds `crowd + 1` cells inside a radius of
            // `sqrt((crowd + 1) / pi) + 1` berths.
            let bound = ((crowd as f64 + 1.0) / std::f64::consts::PI).sqrt() + 1.0;
            assert!(
                off <= bound * BERTH,
                "{crowd} ships in the berths put the next {off} m off the mouth"
            );
            assert!(
                crate::space_state::distance(at, station) >= r0,
                "a berth must stay outside the mouth's own radius"
            );
        }
    }

    /// Berths never share a cell and are a berth apart, the first 60.
    #[test]
    fn berths_are_distinct_and_a_berth_apart() {
        let station = [0.0, 0.0, 0.0];
        let mouth = [20_000.0, 0.0, 0.0];
        let all: Vec<[f64; 3]> = (0..60).map(|i| berth(mouth, station, i)).collect();
        for (a, p) in all.iter().enumerate() {
            for q in &all[a + 1..] {
                assert!(crate::space_state::distance(*p, *q) > BERTH * 0.999);
            }
        }
    }

    /// **Two pilots do not undock into each other.**
    ///
    /// One locator serves every ship, so the second pilot used to arrive at
    /// the first one's coordinates -- and both coast on the same shove at the
    /// same speed, so they stayed there: one bracket, two ships, and a
    /// collision model the client has to resolve by itself.
    ///
    /// The property is **separation**, and the other half is that both ships
    /// are still at the station: a fix that scattered them across the grid
    /// would pass a separation test and be a worse bug. So this asserts a
    /// band -- at least a berth apart, and both still within a station radius
    /// of the mouth. The offset's direction and sign are deliberately not
    /// asserted; they are a choice, not a requirement.
    #[test]
    fn two_pilots_leaving_one_station_get_a_berth_each() {
        const ALICE: i64 = 1_000_001;
        const BOB: i64 = 1_000_002;
        let alice = crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: 100_000_001,
            ..crate::model::Character::default()
        };
        let bob = crate::model::Character {
            character_id: BOB,
            name: "Bob".to_owned(),
            ship_id: 100_000_002,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![alice, bob],
            ..crate::model::SandboxConfig::default()
        });

        let (_, first) = undock_motion(&reg, ALICE).expect("Alice has a mouth");
        on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        let (_, second) = undock_motion(&reg, BOB).expect("Bob has a mouth");

        let apart = crate::space_state::distance(first.position, second.position);
        // A band, not the constant. The first version of this read
        // `apart >= BERTH` and failed at 749.9999999999995 m -- which is the
        // feature working and the assertion pinning an `f64` division. The
        // property is "a berth apart", and a thousandth of one is noise.
        assert!(
            apart > BERTH * 0.999,
            "two ships {apart} m apart share a berth: {:?} and {:?}",
            first.position,
            second.position
        );
        // Both still at the station, which is the half a separation test
        // alone would let through.
        for p in [first.position, second.position] {
            let from_station = crate::space_state::distance([0.0; 3], p);
            assert!(
                from_station > f64::from(STATION_RADIUS),
                "outside the station model: {from_station} m"
            );
            assert!(
                from_station < f64::from(STATION_RADIUS) * 2.0,
                "and still at it, not scattered: {from_station} m"
            );
        }
        let _ = reg.take_pending();
    }

    /// **A pilot who binds their ballpark mid-flight is told where the other
    /// ship actually is, not where it was last clicked.**
    ///
    /// The defect this round closed, and it was unfixable by any later
    /// correction: `SetBallPosition` and `SetBallVelocity` sit outside every
    /// client dispatch set and reset nothing (`docs/lanes/movement.md` §8),
    /// so the state we build at bind time is the only chance to be right. A
    /// ship that had been cruising for a minute appeared in the newcomer's
    /// park at the station mouth, and then flew on from there for ever --
    /// kilometres out, with nothing wrong on either side.
    ///
    /// **Asserted as physics, not as our arithmetic.** The bounds are
    /// `0 < travelled < max_velocity * elapsed`, which are true of *any*
    /// correct integrator and false of two specific bugs: no integration at
    /// all (nothing moves) and an integrator that outruns the hull. What
    /// shape the curve has between those bounds is
    /// `space_state::tests::align_time_is_when_three_quarters_of_the_speed_is_there`'s
    /// business, and pinning it again here would be pinning the
    /// implementation twice.
    #[test]
    fn a_pilot_joining_mid_flight_is_told_where_the_other_ship_actually_is() {
        const ALICE: i64 = 1_000_001;
        const BOB: i64 = 1_000_002;
        let alice = crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: 100_000_001,
            ..crate::model::Character::default()
        };
        let bob = crate::model::Character {
            character_id: BOB,
            name: "Bob".to_owned(),
            ship_id: 100_000_002,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![alice, bob],
            ..crate::model::SandboxConfig::default()
        });

        // Alice undocks, and is told to fly somewhere a long way off at full
        // throttle -- the two commands a click and a throttle drag send.
        on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        let mouth = reg
            .roster()
            .get(ALICE)
            .expect("Alice is in space")
            .motion
            .position;
        const ELAPSED: f64 = 60.0;
        let ship_type_id = reg.character(ALICE).expect("Alice exists").ship_type_id;
        let top_speed = f64::from(hull::of_in(reg.content(), ship_type_id).max_velocity);
        {
            let p = reg.roster_mut().get_mut(ALICE).expect("in space");
            p.motion.after(Action::GotoPoint {
                ball: p.ship_id,
                point: [1.0e12, 0.0, 0.0],
            });
            p.motion.after(Action::SetSpeedFraction {
                ball: p.ship_id,
                fraction: 1.0,
            });
            // A minute of flying, without a minute of waiting.
            p.since -= ELAPSED;
        }

        // Bob binds his ballpark, which is where he is handed the scene.
        let state = bootstrap_for(&mut reg, BOB).expect("Bob gets a state");
        let alice_ball = state
            .park
            .balls
            .iter()
            .find(|b| b.ball_id == 100_000_001)
            .expect("Alice's ship is in Bob's park");
        let travelled = crate::space_state::distance(mouth, alice_ball.position);
        assert!(
            travelled > 0.0,
            "Alice has been cruising for {ELAPSED}s and Bob was told she is \
             still at the station mouth: {:?}",
            alice_ball.position
        );
        assert!(
            travelled < top_speed * ELAPSED,
            "she cannot have covered {travelled} m in {ELAPSED}s at {top_speed} m/s"
        );

        // And Bob himself is at his own mouth, because he has only just
        // arrived. Compared against `undock_motion`'s answer for *Bob*
        // rather than against Alice's position: they share a station and so
        // share a locator today, and the very next change offsets the second
        // ship so two pods do not undock into each other.
        //
        // **Within a metre, not exactly.** A ship leaves the mouth with a
        // shove -- 1.75x its top speed, capped at 700 m/s -- so a correct
        // integrator moves it a little even over the microseconds this test
        // takes, and an earlier version of this assertion failed by 3.5 mm.
        // That failure was the integrator working. The property is "has not
        // been flying since 1970", and a metre says it.
        let bob_ball = state
            .park
            .balls
            .iter()
            .find(|b| b.ball_id == 100_000_002)
            .expect("Bob's own ship is in his park");
        // Bob's seat is recorded in the roster; undock_motion now answers for
        // a world where Alice has flown on and the berth is free again.
        let bob_seed = reg.roster().get(BOB).expect("Bob is seated").motion;
        assert!(
            crate::space_state::distance(bob_seed.position, bob_ball.position) < 1.0,
            "a ship that just undocked has not been flying since 1970: {:?} \
             against a mouth at {:?}",
            bob_ball.position,
            bob_seed.position
        );
        let _ = reg.take_pending();
    }

    /// **A catalogued planet is a ball in the park, and therefore warpable.**
    ///
    /// The two halves are one fact, which is the finding this test records: a
    /// celestial the client has no *ball* for has no bracket and no
    /// right-click menu, so no `CmdWarpToStuff` is ever sent and the server
    /// never hears about it. A player could warp to the station and to
    /// nothing else, because the station was the only celestial we put in the
    /// park.
    ///
    /// So this asserts the ball, the slim item beside it, and that the same
    /// id resolves as a warp destination -- and that the kind we decline
    /// to serve (a gate with no partner) resolves to neither.
    #[test]
    fn a_catalogued_planet_is_both_a_ball_and_a_warp_destination() {
        const PLANET: i64 = 40_000_002;
        const BELT: i64 = 40_000_003;
        const STAR: i64 = 40_000_001;
        const GATE: i64 = 50_000_056;
        let catalogue = crate::celestial::Catalogue::parse(
            "star\t40000001\t45041\t63350000.0\t0.0\t0.0\t0.0\n\
             planet\t40000002\t11\t5060000.0\t161891117336.0\t21288951986.0\t-73529712226.0\n\
             belt\t40000003\t15\t0.0\t161967513600.0\t21288837120.0\t-73505464320.0\n\
             gate\t50000056\t29624\t0.0\t331516354560.0\t43597455360.0\t-586353991680.0\n",
        );
        let mut reg = Registry::new(crate::model::SandboxConfig {
            celestials: catalogue,
            ..crate::model::SandboxConfig::default()
        });
        let actor = reg.config().characters[0].character_id;
        let state = bootstrap_for(&mut reg, actor).expect("a state");

        let planet = state
            .park
            .balls
            .iter()
            .find(|b| b.ball_id == PLANET)
            .expect("the planet is a ball, or nobody can right-click it");
        assert_eq!(
            planet.position,
            [161_891_117_336.0, 21_288_951_986.0, -73_529_712_226.0],
            "the client draws it from its own data, so our ball has to agree"
        );
        assert!(
            planet.physics.is_none() && planet.flags & FLAG_IS_FREE == 0,
            "a planet is not flown, so it has no physics block to evolve"
        );
        assert!(state.park.balls.iter().any(|b| b.ball_id == BELT));
        let star = state
            .park
            .balls
            .iter()
            .find(|b| b.ball_id == STAR)
            .expect("the sun is a ball too: the client warps to it");
        assert_eq!(star.position, [0.0; 3]);
        assert!(star.physics.is_none() && star.flags & FLAG_IS_FREE == 0);
        let sun_slim = state
            .slims
            .iter()
            .find(|s| s.item_id == STAR)
            .expect("the sun has a slim item");
        assert_eq!(
            sun_slim.type_id, 45041,
            "the slim's type is what gives the client the sun's group and category"
        );
        assert_eq!(sun_slim.char_id, None);
        assert!(
            !state.park.balls.iter().any(|b| b.ball_id == GATE),
            "a gate this extract names no partner for could have no jumps row, \
             and that raises IndexError on a double-click"
        );
        // Every slim names a ball, which is the invariant that keeps
        // `SetState` from raising `BallNotInPark` and losing the lot.
        assert!(state.every_slim_has_a_ball());
        let slim = state
            .slims
            .iter()
            .find(|s| s.item_id == PLANET)
            .expect("the planet has a slim item");
        assert_eq!(slim.type_id, 11);
        assert_eq!(slim.char_id, None, "a planet has no pilot");

        // And the same id is a warp destination, with the radius that keeps a
        // warp out of the body.
        let target = reg.ball_target(PLANET).expect("warpable");
        assert_eq!(target.position, planet.position);
        assert_eq!(target.radius, 5_060_000.0);
        let sun = reg.ball_target(STAR).expect("the sun is warpable");
        assert_eq!(
            sun.warp_in,
            Some(crate::warp_in::sun_warp_in([0.0; 3], 63_350_000.0)),
            "a warp to the sun lands at the client's own sun warp-in point"
        );
        assert_eq!(
            reg.ball_target(GATE),
            None,
            "nor this gate: a warp to a ball the client has no bracket for"
        );
        let _ = reg.take_pending();
    }

    /// **FX4: every station is a ball where it is, in its own system.**
    ///
    /// A player jumped to a second system and could not warp to any of its
    /// stations: the only station the server had ever put in a park was the
    /// pilot's own, at the origin, at home. Ids past the default pilot's own
    /// station are invented (`schema.md` §1.1); positions are invented.
    #[test]
    fn every_station_is_a_ball_at_its_own_position_in_its_own_system() {
        const HOME: i64 = 60_012_526; // the default pilot's station
        const AWAY: i64 = 69_000_002;
        const AWAY_SYSTEM: i64 = 39_000_002;
        const HOME_AT: [f64; 3] = [4.0e11, -2.0e10, 7.0e10];
        const AWAY_AT: [f64; 3] = [-3.0e11, 1.0e10, 2.0e11];
        let catalogue = crate::celestial::Catalogue::parse(&format!(
            "#kind\tid\ttype_id\tradius\tx\ty\tz\tsystem\tdest\tdest_system\n\
             #system\t30000001\t20000001\t10000001\n\
             #system\t{AWAY_SYSTEM}\t29000001\t19000001\n\
             station\t{HOME}\t2502\t0.0\t{}\t{}\t{}\t30000001\t0\t0\n\
             station\t{AWAY}\t1529\t0.0\t{}\t{}\t{}\t{AWAY_SYSTEM}\t0\t0\n",
            HOME_AT[0], HOME_AT[1], HOME_AT[2], AWAY_AT[0], AWAY_AT[1], AWAY_AT[2],
        ));
        let mut stations = crate::stations::Table::default();
        stations.insert(
            AWAY,
            crate::stations::Station {
                system_id: AWAY_SYSTEM,
                region_id: 19_000_001,
                owner_id: 1_999_002,
                operation_id: 99,
            },
        );
        let mut content = crate::content::Content::fixture();
        content.insert("stations", stations);
        let mut reg = Registry::new(crate::model::SandboxConfig {
            celestials: catalogue,
            ..crate::model::SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content));
        let actor = reg.config().characters[0].character_id;
        let ship = reg.config().characters[0].ship_id;

        // At home: the home station at its own position, the ship at its
        // mouth, and no station of another system.
        let state = bootstrap_for(&mut reg, actor).expect("a state");
        let home = state
            .park
            .balls
            .iter()
            .find(|b| b.ball_id == HOME)
            .expect("home is a ball");
        assert_eq!(home.position, HOME_AT, "not the origin, which is the sun");
        assert!(!state.park.balls.iter().any(|b| b.ball_id == AWAY));
        let ego = state
            .park
            .balls
            .iter()
            .find(|b| b.ball_id == ship)
            .expect("ego");
        let off = crate::space_state::distance(ego.position, HOME_AT);
        assert!(
            off < 100_000.0,
            "the ship undocks beside its station, {off} m from it"
        );
        assert!(state.every_slim_has_a_ball());

        // In the other system: its station, owned by its corporation.
        reg.roster_mut().leave(actor);
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                actor,
                ship,
                AWAY_SYSTEM,
                crate::space_state::Motion::arrived([0.0, 5.0e11, 0.0], [0.0; 3]),
            ));
        let state = bootstrap_for(&mut reg, actor).expect("a state");
        let away = state
            .park
            .balls
            .iter()
            .find(|b| b.ball_id == AWAY)
            .expect("a ball");
        assert_eq!(away.position, AWAY_AT);
        assert!(!state.park.balls.iter().any(|b| b.ball_id == HOME));
        let slim = state
            .slims
            .iter()
            .find(|s| s.item_id == AWAY)
            .expect("a slim");
        assert_eq!((slim.type_id, slim.owner_id), (1529, 1_999_002));
        assert!(state.every_slim_has_a_ball());

        // A warp destination, where the client draws it.
        assert_eq!(reg.ball_target(AWAY).map(|t| t.position), Some(AWAY_AT));
        assert_eq!(reg.ball_target(HOME).map(|t| t.position), Some(HOME_AT));

        // Dockable here, with the row's owner and the catalogue's system;
        // the home station is not, from another system.
        let to = reg.dock_target(actor, AWAY).expect("dockable");
        assert_eq!(
            (
                to.solar_system_id,
                to.constellation_id,
                to.region_id,
                to.owner_id,
                to.operation_id,
                to.type_id
            ),
            (AWAY_SYSTEM, 29_000_001, 19_000_001, 1_999_002, 99, 1529)
        );
        assert_eq!(reg.dock_target(actor, HOME), None);
        let _ = reg.take_pending();
    }

    /// The park has the station in it, and the ship is outside it.    /// The park has the station in it, and the ship is outside it.
    ///
    /// **The ballpark is the whole scene**: the client renders only balls it
    /// was given, so a park with one ship is a ship in an empty universe --
    /// which is what the first real undock looked like. The property is
    /// "there is something to be outside of, and we are outside it", not the
    /// specific offset.
    #[test]
    fn the_station_is_in_the_park_and_the_ship_is_clear_of_it() {
        let mut reg = Registry::new(crate::model::SandboxConfig::default());
        let actor = reg.config().characters[0].character_id;
        let c = reg.character(actor).expect("docked");
        let (ship_id, station_id) = (c.ship_id, c.station_id);
        let state = bootstrap_for(&mut reg, actor).expect("a state");
        assert_eq!(state.park.balls.len(), 2, "the ship and the station");
        let ship = state
            .park
            .balls
            .iter()
            .find(|b| b.ball_id == ship_id)
            .expect("the ego ball");
        let station = state
            .park
            .balls
            .iter()
            .find(|b| Some(b.ball_id) == station_id)
            .expect("the station ball");
        assert!(
            station.physics.is_none(),
            "a station has no velocity to integrate, so isFree is clear"
        );
        assert_eq!(station.flags & FLAG_IS_FREE, 0);
        assert_eq!(
            station.radius, STATION_RADIUS,
            "type 2502's own radius, from types.fsdbinary"
        );
        // **Euclidean, not along one axis.** This measured `position[0]`
        // alone, which was true of the flat `+X` placeholder and wrong the
        // moment a real mouth arrived: station 2502's is at
        // `(17, 10096, -12607)`, which is 16 km out and 17 m along X. The
        // property was always radial distance; the test had quietly pinned
        // the placeholder's geometry instead.
        let distance = ship
            .position
            .iter()
            .zip(station.position.iter())
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f64>()
            .sqrt();
        assert!(
            distance > f64::from(station.radius),
            "the ship must be outside the station model: {distance} m from a \
             {} m radius",
            station.radius
        );
        // Every slim item still names a ball, which is the invariant that
        // keeps `SetState` from raising `BallNotInPark` and losing the lot.
        assert!(state.every_slim_has_a_ball());
        assert_eq!(state.slims.len(), 2);
    }

    /// A world with rocks: one belt here, one in the next system. The law
    /// and counts are a fixture; nothing below asserts one of their numbers
    /// as the client's.
    fn rocky() -> Registry {
        let celestials = crate::celestial::Catalogue::parse(
            "belt\t40000003\t15\t0.0\t100000.0\t0.0\t0.0\n\
             belt\t40000020\t15\t0.0\t-100000.0\t0.0\t0.0\n",
        );
        let counts = crate::asteroid::Counts::parse(
            "40000003\t0\t1228\t2\t30000001\t0.5\t90.0\t16255.0\n\
             40000003\t0\t1230\t3\t30000001\t0.5\t90.0\t16255.0\n\
             40000020\t5\t1230\t4\t30000002\t0.5\t90.0\t16255.0\n",
        );
        let rocks = crate::asteroid::Field::generate(
            &counts,
            &celestials,
            &crate::asteroid::Quantities::default(),
        );
        Registry::new(crate::model::SandboxConfig {
            celestials,
            rocks,
            ..crate::model::SandboxConfig::default()
        })
    }

    /// **Every rock in the system is a ball with a slim item, and has no
    /// gauges.** The slim is the existing five fields with the ore as its
    /// type; the ball is a body nothing flies. A rock in another system is
    /// in neither.
    #[test]
    fn the_systems_rocks_are_balls_with_slims_and_no_gauges() {
        let mut reg = rocky();
        let actor = reg.config().characters[0].character_id;
        let here = reg.character(actor).unwrap().solar_system_id.unwrap();
        let rocks: Vec<crate::asteroid::Rock> = reg.rocks().iter().copied().collect();
        let state = bootstrap_for(&mut reg, actor).expect("a state");
        assert!(state.every_slim_has_a_ball(), "BallNotInPark loses the lot");
        let mut served = 0;
        for r in &rocks {
            let ball = state.park.balls.iter().find(|b| b.ball_id == r.id);
            if r.solar_system_id != here {
                assert!(ball.is_none(), "a rock from another system: {r:?}");
                continue;
            }
            served += 1;
            let ball = ball.expect("a rock in this system is a ball");
            assert_eq!(ball.mode, Mode::Stop);
            assert!(ball.physics.is_none());
            assert_eq!(ball.flags, FLAGS_STATIC_BALL);
            assert_eq!(ball.radius, r.radius as f32, "the radius is the drawn size");
            assert_eq!(ball.position, r.position);
            let slim = state.slims.iter().find(|s| s.item_id == r.id).unwrap();
            assert_eq!(slim.type_id, r.type_id, "the ore type is the slim's type");
            assert_eq!(slim.owner_id, 1);
            assert_eq!((slim.char_id, slim.corp_id), (None, None));
            assert!(
                !state.damage.iter().any(|(id, _)| *id == r.id),
                "a rock has no HP gauges"
            );
        }
        assert_eq!(served, 5);
        let _ = reg.take_pending();
    }

    /// No `--belt-rocks`, no rock: the park holds what it held before.
    #[test]
    fn without_rocks_the_park_has_none() {
        let mut reg = Registry::new(crate::model::SandboxConfig::default());
        let actor = reg.config().characters[0].character_id;
        let state = bootstrap_for(&mut reg, actor).expect("a state");
        assert!(!state.park.balls.iter().any(|b| {
            (crate::asteroid::MIN_ROCK_ID..crate::asteroid::END_ROCK_ID).contains(&b.ball_id)
        }));
        let _ = reg.take_pending();
    }

    /// Two bootstraps of the same world carry the same rocks, byte for byte.
    #[test]
    fn rocks_are_the_same_in_every_bootstrap() {
        let mut reg = rocky();
        let actor = reg.config().characters[0].character_id;
        let rock_part = |s: &Bootstrap| {
            let balls: Vec<Ball> = s
                .park
                .balls
                .iter()
                .filter(|b| {
                    b.ball_id >= crate::asteroid::MIN_ROCK_ID
                        && b.ball_id < crate::asteroid::END_ROCK_ID
                })
                .copied()
                .collect();
            let slims: Vec<Slim> = s
                .slims
                .iter()
                .filter(|x| balls.iter().any(|b| b.ball_id == x.item_id))
                .copied()
                .collect();
            (balls, slims)
        };
        let a = rock_part(&bootstrap_for(&mut reg, actor).unwrap());
        let b = rock_part(&bootstrap_for(&mut reg, actor).unwrap());
        assert!(!a.0.is_empty());
        assert_eq!(a, b);
        let _ = reg.take_pending();
    }

    /// **A rock is a destination**: `ball_target` resolves it to its own
    /// position and radius with no warp-in point, and an id in the asteroid
    /// range that is not a rock resolves to nothing.
    #[test]
    fn a_rock_is_a_warp_and_approach_destination() {
        let reg = rocky();
        let r = *reg.rocks().iter().next().unwrap();
        let t = reg.ball_target(r.id).expect("a rock resolves");
        assert_eq!(t.position, r.position);
        assert_eq!(t.radius, r.radius);
        assert_eq!(t.warp_in, None);
        assert_eq!(reg.ball_target(crate::asteroid::MIN_ROCK_ID + 199), None);
    }

    /// **Approach a rock and you stop at its surface, not its centre**: the
    /// tether reconciles to `centre_distance(range, hull, rock)`, the station
    /// test's shape with a rock in the station's place.
    #[test]
    fn a_ship_following_a_rock_keeps_the_hull_gap() {
        let mut reg = rocky();
        let actor = reg.config().characters[0].character_id;
        on_ballpark_bind(&mut reg, Some(actor));
        let _ = reg.take_pending();
        let rock = *reg.rocks().in_system(30_000_001).next().unwrap();
        let ship_type = reg.character(actor).unwrap().ship_type_id;
        let hull = *hull::of_in(reg.content(), ship_type);
        {
            let p = reg.roster_mut().get_mut(actor).expect("in space");
            p.motion.after(Action::FollowBall {
                ball: p.ship_id,
                target: rock.id,
                range: 500.0,
            });
            p.since -= 10.0;
        }
        reg.advance_character(actor);
        let at = reg.roster().get(actor).unwrap().motion.position;
        let want = crate::orbit::centre_distance(500.0, f64::from(hull.radius), rock.radius);
        let got = crate::space_state::distance(at, rock.position);
        assert!(
            (got - want).abs() < 1e-3,
            "{got} m from the rock's centre, want {want}"
        );
        let _ = reg.take_pending();
    }

    // ---- a ship's fitted modules, for the pilots who see it (P2.5) ----

    const P25_ALICE: i64 = 1_000_001;
    const P25_BOB: i64 = 1_000_002;
    const P25_ALICE_SHIP: i64 = 100_000_001;
    const P25_BOB_SHIP: i64 = 100_000_002;
    const P25_GUN: i64 = 100_000_050;
    const P25_MED: i64 = 100_000_051;
    const P25_IN_CARGO: i64 = 100_000_052;
    /// `flagHiSlot0`, `flagMedSlot0`, `flagCargo` [V].
    const P25_HI_SLOT_0: i64 = 27;
    const P25_MED_SLOT_0: i64 = 19;

    /// A second module type, any but the gun's.
    fn p25_other_module() -> &'static crate::item::ItemType {
        crate::item::SPAWNABLE
            .iter()
            .find(|t| t.category_id == crate::fitting::CATEGORY_MODULE && t.type_id != 486)
            .expect("a second spawnable module")
    }

    /// Alice and Bob, both docked. Alice's ship has a 200mm AutoCannon I in
    /// high slot 0 with rounds loaded, another module in medium slot 0, and
    /// a third module lying in her cargo hold.
    fn p25_alice_fitted() -> Registry {
        let pilot = |id, name: &str, ship| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![
                pilot(P25_ALICE, "Alice", P25_ALICE_SHIP),
                pilot(P25_BOB, "Bob", P25_BOB_SHIP),
            ],
            ..crate::model::SandboxConfig::default()
        });
        let other = p25_other_module();
        let module = |item_id, type_id, group_id, flag_id| crate::item::Item {
            item_id,
            type_id,
            owner_id: P25_ALICE,
            location_id: P25_ALICE_SHIP,
            flag_id,
            quantity: 1,
            group_id,
            category_id: crate::fitting::CATEGORY_MODULE,
            singleton: true,
        };
        let mut items = reg.store().items().to_vec();
        items.push(module(P25_GUN, 486, 55, P25_HI_SLOT_0));
        items.push(module(
            P25_MED,
            other.type_id,
            other.group_id,
            P25_MED_SLOT_0,
        ));
        items.push(module(
            P25_IN_CARGO,
            other.type_id,
            other.group_id,
            crate::fitting::FLAG_CARGO,
        ));
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: P25_IN_CARGO + 1,
            ..crate::store::Snapshot::default()
        });
        // EMP S, loaded: a charge at the gun's own flag.
        reg.apply_items(&[crate::item::Edit::Create {
            type_id: 185,
            quantity: 50,
            to: crate::item::Slot::new(P25_ALICE, P25_ALICE_SHIP, P25_HI_SLOT_0),
        }])
        .expect("loaded");
        let _ = reg.take_pending();
        reg
    }

    /// The `modules` attribute of a slim in either form, if it has one.
    fn p25_modules_of(slim: &PyValue) -> Option<PyValue> {
        let attrs = match slim {
            PyValue::Instance { state, .. } => state.as_ref(),
            other => other,
        };
        crate::pyutil::dict_get(attrs, b"modules").cloned()
    }

    /// `(itemID, typeID, flagID)` as the client unpacks it (`turretSet.py:453`).
    fn p25_entry(item_id: i64, type_id: i64, flag_id: i64) -> PyValue {
        PyValue::Tuple(vec![
            PyValue::Int(item_id),
            PyValue::Int(type_id),
            PyValue::Int(flag_id),
        ])
    }

    /// Alice's two fitted modules, in flag order: medium slot 0 (19) before
    /// high slot 0 (27).
    fn p25_alice_list() -> PyValue {
        PyValue::List(vec![
            p25_entry(P25_MED, p25_other_module().type_id, P25_MED_SLOT_0),
            p25_entry(P25_GUN, 486, P25_HI_SLOT_0),
        ])
    }

    /// Every slim in a `SetState` push, by `itemID`.
    fn p25_state_slims(state: &Bootstrap) -> Vec<(i64, PyValue)> {
        let (_, args) = state.to_push();
        let PyValue::List(actions) = &args[0] else {
            panic!("a batch")
        };
        let PyValue::Tuple(action) = &actions[0] else {
            panic!("(stamp, call)")
        };
        let PyValue::Tuple(call) = &action[1] else {
            panic!("(name, args)")
        };
        let PyValue::Tuple(call_args) = &call[1] else {
            panic!("args")
        };
        let PyValue::Instance { state: bag, .. } = &call_args[0] else {
            panic!("a KeyVal")
        };
        let Some(PyValue::List(slims)) = crate::pyutil::dict_get(bag, b"slims") else {
            panic!("bag.slims")
        };
        slims
            .iter()
            .filter_map(|s| {
                let PyValue::Instance { state, .. } = s else {
                    return None;
                };
                match crate::pyutil::dict_get(state, b"itemID") {
                    Some(PyValue::Int(id)) => Some((*id, s.clone())),
                    _ => None,
                }
            })
            .collect()
    }

    /// **A ship's slim lists exactly its fitted modules** -- not the rounds
    /// loaded in the gun, not a module in the hold -- as `(itemID, typeID,
    /// flagID)` triples, which is what an observer's `FitTurrets` unpacks
    /// (`turretSet.py:453`) **[V]**. A ship with nothing fitted sends no
    /// `modules`, which the client reads as `[]` (`slimItem.py:36-38`) **[V]**.
    #[test]
    fn a_ship_slim_lists_exactly_its_fitted_modules() {
        let mut reg = p25_alice_fitted();
        on_ballpark_bind(&mut reg, Some(P25_ALICE));
        let _ = reg.take_pending();
        on_ballpark_bind(&mut reg, Some(P25_BOB));
        let _ = reg.take_pending();
        let state = bootstrap_for(&mut reg, P25_BOB).expect("Bob gets a state");
        let slims = p25_state_slims(&state);
        let alice = slims
            .iter()
            .find(|(id, _)| *id == P25_ALICE_SHIP)
            .map(|(_, s)| s)
            .expect("Alice's ship is in Bob's state");
        assert_eq!(p25_modules_of(alice), Some(p25_alice_list()));
        let bob = slims
            .iter()
            .find(|(id, _)| *id == P25_BOB_SHIP)
            .map(|(_, s)| s)
            .expect("Bob's own ship");
        assert_eq!(p25_modules_of(bob), None, "nothing fitted, nothing sent");
    }

    /// **The modules go to the pilots already in the system when the ship
    /// arrives**: Alice undocking into Bob's system reaches him as an
    /// `AddBalls2` whose slim carries her modules -- the moment his client
    /// fits her guns (`ship.AssembleModel` -> `FitHardpoints`) **[V]**. She
    /// herself is not in that audience.
    #[test]
    fn an_arriving_ships_modules_reach_the_systems_other_pilots() {
        let mut reg = p25_alice_fitted();
        on_ballpark_bind(&mut reg, Some(P25_BOB));
        let _ = reg.take_pending();
        on_ballpark_bind(&mut reg, Some(P25_ALICE));
        let sent: Vec<_> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| (p.audience(), p.to_push()))
            .collect();
        let to_bob: Vec<_> = sent
            .iter()
            .filter(|(to, _)| to.as_ref().is_some_and(|to| to.contains(&P25_BOB)))
            .collect();
        assert_eq!(to_bob.len(), 1, "{sent:?}");
        let (to, (_, args)) = to_bob[0];
        assert_eq!(to, &Some(vec![P25_BOB]), "Bob, and not Alice");
        // `[(stamp, ('AddBalls2', ((blob, [(slimDict, damage)]),)))]`
        let PyValue::List(entries) = &args[0] else {
            panic!("a batch")
        };
        let PyValue::Tuple(entry) = &entries[0] else {
            panic!("(stamp, call)")
        };
        let PyValue::Tuple(call) = &entry[1] else {
            panic!("(name, args)")
        };
        assert_eq!(call[0], crate::pyutil::bstr("AddBalls2"));
        let PyValue::Tuple(call_args) = &call[1] else {
            panic!("args")
        };
        let PyValue::Tuple(state_and_extra) = &call_args[0] else {
            panic!("(state, extra)")
        };
        let PyValue::List(extra) = &state_and_extra[1] else {
            panic!("extraBallData")
        };
        let PyValue::Tuple(pair) = &extra[0] else {
            panic!("(slim, damage)")
        };
        assert!(
            matches!(pair[0], PyValue::Dict(_)),
            "AddBalls2 wants a dict"
        );
        assert_eq!(p25_modules_of(&pair[0]), Some(p25_alice_list()));
    }

    /// **An unfit removes the module from the slim**: what the next observer
    /// is told is the ship as it is now.
    #[test]
    fn an_unfitted_module_leaves_the_slim() {
        let mut reg = p25_alice_fitted();
        let station = reg
            .character(P25_ALICE)
            .and_then(|c| c.station_id)
            .expect("docked");
        reg.apply_items(&[crate::item::Edit::Move {
            item_id: P25_MED,
            to: crate::item::Slot::new(P25_ALICE, station, crate::model::FLAG_HANGAR),
        }])
        .expect("unfitted");
        on_ballpark_bind(&mut reg, Some(P25_ALICE));
        on_ballpark_bind(&mut reg, Some(P25_BOB));
        let _ = reg.take_pending();
        let state = bootstrap_for(&mut reg, P25_BOB).expect("Bob gets a state");
        let alice = p25_state_slims(&state)
            .into_iter()
            .find(|(id, _)| *id == P25_ALICE_SHIP)
            .map(|(_, s)| s)
            .expect("Alice's ship");
        assert_eq!(
            p25_modules_of(&alice),
            Some(PyValue::List(vec![p25_entry(P25_GUN, 486, P25_HI_SLOT_0)]))
        );
    }

    /// **W10.5 seam S9: an extra lands on its own slim only**, in both
    /// builders. Two slims, one extra for the first and one for an id that
    /// is not in the batch: the first slim gains the key after its own
    /// attributes, the second is untouched, and the stray entry is nowhere.
    #[test]
    fn an_extra_lands_on_its_own_slim_only() {
        const MINE: i64 = 40_000_001;
        const OTHER: i64 = 40_000_002;
        const STRAY: i64 = 40_000_003;
        let slim = |item_id| Slim {
            item_id,
            type_id: 1,
            owner_id: 2,
            char_id: None,
            corp_id: None,
            response_thresholds: None,
        };
        let park = Park {
            current_time: 0,
            balls: vec![station_ball(MINE, 1.0, 2), station_ball(OTHER, 1.0, 2)],
        };
        let extras = || {
            vec![
                (MINE, vec![("seamExtra", PyValue::Int(7))]),
                (STRAY, vec![("seamExtra", PyValue::Int(8))]),
            ]
        };
        // `(itemID, keys)` of every attribute dict in a push, in order.
        fn slims(v: &PyValue, out: &mut Vec<(i64, Vec<PyValue>)>) {
            match v {
                PyValue::Tuple(xs) | PyValue::List(xs) => xs.iter().for_each(|x| slims(x, out)),
                PyValue::Instance { state, .. } => slims(state, out),
                PyValue::Dict(pairs) => {
                    let id = pairs.iter().find_map(|(v, k)| {
                        (*k == crate::pyutil::bstr("itemID")).then_some(v.clone())
                    });
                    if let Some(PyValue::Int(id)) = id {
                        out.push((id, pairs.iter().map(|(_, k)| k.clone()).collect()));
                    } else {
                        pairs.iter().for_each(|(v, _)| slims(v, out));
                    }
                }
                _ => {}
            }
        }
        let key = crate::pyutil::bstr("seamExtra");
        let check = |push: (&'static str, Vec<PyValue>)| {
            let mut found = Vec::new();
            slims(&PyValue::List(push.1), &mut found);
            let ids: Vec<i64> = found.iter().map(|(id, _)| *id).collect();
            assert_eq!(ids, vec![MINE, OTHER]);
            assert_eq!(found[0].1.last(), Some(&key), "after its own attributes");
            assert!(!found[1].1.contains(&key), "not on another slim");
        };

        let state = Bootstrap::new(
            10,
            park.clone(),
            MINE,
            vec![slim(MINE), slim(OTHER)],
            vec![],
        )
        .with_extras(extras());
        check(state.to_push());

        let added = BallsAdded::new(
            vec![2],
            10,
            park,
            vec![(slim(MINE), None), (slim(OTHER), None)],
        )
        .expect("a batch")
        .with_extras(extras());
        check(crate::notify::Push::to_push(&added));
    }

    /// FX7: a station ball takes its type's typedata radius, and falls back
    /// to [`STATION_RADIUS`] without typedata, for a type the extract lacks,
    /// for a type that is not a station, and for the tool's `or 1.0`.
    /// Invented rows.
    #[test]
    fn a_station_takes_its_types_radius_and_falls_back_without_one() {
        let t = crate::typedata::Table::parse(
            "type\t990000101\t99015\t3\t0\t1\t0.0\t0.0\t0.0\t42000.0\t0\t\t\tFixture Station
type\t990000102\t99015\t3\t0\t1\t0.0\t0.0\t0.0\t1.0\t0\t\t\tFixture Unsized Station
type\t990000103\t99025\t6\t1\t1\t0.0\t0.0\t1000.0\t50000.0\t0\t\t\tFixture Hull
",
        )
        .expect("the fixture parses");
        assert_eq!(station_radius_in(&t, 990_000_101), 42_000.0);
        for (id, why) in [
            (990_000_102, "the tool's GetRadius default, not a size"),
            (990_000_103, "not category 3"),
            (990_000_199, "not in the extract"),
        ] {
            assert_eq!(station_radius_in(&t, id), STATION_RADIUS, "{why}");
        }
        assert_eq!(
            station_radius_in(&crate::typedata::Table::default(), 990_000_101),
            STATION_RADIUS,
            "an empty extract"
        );
        assert_eq!(
            station_radius(&crate::content::Content::fixture(), 2502),
            STATION_RADIUS,
            "the fixture content carries no station type"
        );
    }

    /// The client's history merge, ported from PA-10 report 1.2-1.4
    /// (`Park.FlushState`, `RealFlushState`): a state whose first entry is a
    /// `SetState` sets the floor and purges entries strictly older; any other
    /// state older than the floor is dropped; same-stamp entries are
    /// *extended*, never doubled. `flush` returns whether it was kept.
    #[derive(Default)]
    struct ClientHistory {
        latest_set_state: i64,
        history: Vec<(i64, Vec<String>)>,
    }

    impl ClientHistory {
        fn flush(&mut self, push: &dyn crate::notify::Push) -> bool {
            let (_, args) = push.to_push();
            let PyValue::List(entries) = &args[0] else {
                panic!("a batch is a list")
            };
            let mut state = Vec::new();
            for e in entries {
                let PyValue::Tuple(e) = e else {
                    panic!("tuple")
                };
                let (PyValue::Int(stamp), PyValue::Tuple(named)) = (&e[0], &e[1]) else {
                    panic!("(stamp, (name, args))")
                };
                let PyValue::Bytes(name) = &named[0] else {
                    panic!("name")
                };
                state.push((*stamp, String::from_utf8_lossy(name).into_owned()));
            }
            if state[0].1 == "SetState" {
                self.latest_set_state = state[0].0;
                self.history.retain(|h| h.0 >= self.latest_set_state);
            } else if state[0].0 < self.latest_set_state {
                return false;
            }
            for (stamp, name) in state {
                match self.history.iter().position(|h| h.0 >= stamp) {
                    Some(i) if self.history[i].0 == stamp => self.history[i].1.push(name),
                    Some(i) => self.history.insert(i, (stamp, vec![name])),
                    None => self.history.push((stamp, vec![name])),
                }
            }
            true
        }

        /// What `RealFlushState` does with the head entry: a `SetState` counts
        /// only at index 0.
        fn reset_honoured(&self, entry: usize) -> bool {
            self.history[entry]
                .1
                .first()
                .is_some_and(|n| n == "SetState")
        }
    }

    /// A desync recovery sent while the client still holds an unflushed
    /// entry at the world's newest stamp must open its own entry, at index
    /// 0 -- where it used to be appended behind the old actions and skipped
    /// -- and the actions after it must not be "too old".
    #[test]
    fn a_mid_session_set_state_opens_its_own_entry_and_later_actions_land() {
        const ALICE: i64 = 1_000_001;
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: 100_000_001,
                ..crate::model::Character::default()
            }],
            ..crate::model::SandboxConfig::default()
        });
        on_ballpark_bind(&mut reg, Some(ALICE));
        let ship = reg.roster().get(ALICE).expect("in space").ship_id;
        let batch = |reg: &mut Registry| {
            let stamp = reg.next_destiny_stamp();
            ActionUpdate::new(
                vec![ALICE],
                stamp,
                vec![Action::SetSpeedFraction {
                    ball: ship,
                    fraction: 0.5,
                }],
            )
            .expect("a batch")
        };

        // The client holds one action, not yet flushed (the bind's own state
        // went out a tick earlier and has drained).
        let mut client = ClientHistory::default();
        let _ = reg.take_pending();
        let before = batch(&mut reg);
        assert!(client.flush(&before));
        let held = reg.last_destiny_stamp_for_test();

        // Before the fix: the state at the same stamp is appended and skipped.
        let old_way = StateUpdate {
            to: vec![ALICE],
            state: bootstrap_for(&mut reg, ALICE).expect("a state"),
        };
        let mut naive = ClientHistory {
            latest_set_state: client.latest_set_state,
            history: client.history.clone(),
        };
        assert!(naive.flush(&old_way));
        let at = naive.history.iter().position(|h| i64::from(held) == h.0);
        assert!(
            !naive.reset_honoured(at.expect("an entry at the held stamp")),
            "the hazard: a SetState extending an old entry is skipped"
        );

        // After: the recovery the server really sends.
        crate::service::space::resend_state(&mut reg, Some(ALICE));
        let mut sent = reg.take_pending();
        assert_eq!(sent.len(), 1);
        let state = sent.remove(0).1;
        assert!(client.flush(state.as_ref()));
        let set_stamp = client.latest_set_state;
        assert!(set_stamp > i64::from(held), "past everything handed out");
        let entry = client
            .history
            .iter()
            .position(|h| h.0 == set_stamp)
            .expect("its own entry");
        assert!(client.reset_honoured(entry), "SetState lands at index 0");
        assert_eq!(client.history[entry].1.len(), 1);

        // And the actions that follow are not "too old", and share the entry.
        let after = batch(&mut reg);
        assert!(client.flush(&after), "not discarded as too old");
        assert!(client.reset_honoured(entry), "still first in its entry");
        assert_eq!(client.history[entry].1.len(), 2);
    }

    /// A bot storm: 100 pilots each ask for a resync inside one second, and
    /// one of them asks 100 times. The stamps stay within `tick + 2`, the
    /// repeat asks are answered once, and every state that is sent while the
    /// floor has headroom opens its own entry under the client's merge law.
    #[test]
    fn a_resync_storm_keeps_stamps_within_two_of_the_clock() {
        let mut reg = Registry::new(crate::model::SandboxConfig::default());
        let tick0 = tick();
        let mut clients: Vec<ClientHistory> = Vec::new();
        let mut stamps = Vec::new();
        for who in 0..100_i64 {
            // The client holds an action at the world's newest stamp.
            let held = reg.next_destiny_stamp();
            let mut client = ClientHistory::default();
            client
                .history
                .push((i64::from(held), vec!["GotoDirection".to_owned()]));
            let state = Bootstrap::new(
                held,
                Park {
                    current_time: held,
                    balls: Vec::new(),
                },
                1,
                Vec::new(),
                Vec::new(),
            );
            let id = 2_000_000 + who;
            let sent = mid_session_state(&mut reg, id, state.clone());
            let update = sent.expect("the first ask is answered");
            let stamp = update.state.stamp;
            // The same pilot asking again at once: the same stamp, and the
            // floor stays put.
            for _ in 0..99 {
                let again = mid_session_state(&mut reg, id, state.clone()).expect("answered");
                assert_eq!(again.state.stamp, stamp);
            }
            assert!(
                stamp <= tick() + 2 && stamp >= tick0,
                "stamp {stamp} outside tick+2"
            );
            assert_eq!(update.state.park.current_time, stamp);
            if stamp > held {
                assert!(client.flush(&update));
                let at = client.history.iter().position(|h| h.0 == i64::from(stamp));
                assert!(client.reset_honoured(at.expect("its own entry")));
            }
            stamps.push(stamp);
            clients.push(client);
        }
        assert!(stamps.windows(2).all(|w| w[0] <= w[1]), "never backwards");
        assert!(stamps.iter().all(|&s| s <= tick() + 2));
        assert!(
            reg.next_destiny_stamp() <= tick() + 2,
            "later actions stay in the window too"
        );
    }
}
