//! A client-side park tracker: what the bot believes is in space around it.
//!
//! **Not a second Destiny.** The real client runs `_destiny.dll`, a fixed
//! one-second-step simulation; this is a belief kept from the same pushes,
//! and its motion law is the **server's own** -- `space_state::Motion::after`
//! to apply a command and `Motion::advanced_at` plus `tethered_in_plane` to
//! carry a ball forward, called exactly as `Registry::advance_character`
//! calls them (`service.rs`). So when the bot decides "in range", it agrees
//! with the server's model by construction. That is the same trade as using
//! `Park::decode` for the blob (`lib.rs::read_set_state`): it cannot catch
//! the server's model being wrong, and it never disagrees with it. Where the
//! two can still part -- the cadence of a mutual orbit, below -- the
//! cure is the client's own: `UpdateStateRequest` and a fresh `SetState`
//! (`Conn::request_state`).
//!
//! What it applies, from `DoDestinyUpdate` (`docs/lanes/bots.md` §4.1.6):
//!
//! | action | effect here | evidence |
//! |---|---|---|
//! | `SetState` | replaces everything; slims from the bag's `slims` | `destiny.rs` `Bootstrap::to_push` |
//! | `AddBalls2` | adds balls and their slims | `BallsAdded::to_push`: one positional, `(blob, extra)` |
//! | `RemoveBalls` / `RemoveBall` | removes | `BallsRemoved::to_push`: one positional, a list |
//! | `SetBallRadius` | radius | `mining::RockRadius`: `(ballID, radius)` |
//! | `SetBallPosition`, `SetBallVelocity` | position, velocity | `destiny::Action::args`: `(ball, x, y, z)` |
//! | `Stop`, `GotoDirection`, `GotoPoint`, `FollowBall`, `Orbit`, `SetSpeedFraction`, `WarpTo` | mode, via `Motion::after` | `destiny::Action::args` |
//!
//! | `SetMaxSpeed` | the ball's top speed (a module changed it) | `modules.rs`: `(ball, max)` |
//! | `SetBallMass` / `SetBallAgility` | the ball's mass (kg) / agility (a module changed it) | `modules.rs`: `(ball, value)` |
//!
//! Everything else (`OnSpecialFX`, `OnDamageStateChange`, ...) is not about
//! where things are and is ignored here; it is still in the frame for any
//! caller who wants it.
//!
//! **Each ball moves with its own blob numbers** (FX6): mass, radius, top
//! speed and agility as the ball record carried them, then `SetMaxSpeed`,
//! which is what the client's Destiny integrates with and what the server
//! wrote from the hull it integrates with. Before FX6 the hull came from
//! `destiny::hull::find` by slim type, which in a bot process (no
//! `--typedata` installed) knows the hand table only: every NPC type from
//! the bundle flew as `hull::FALLBACK`, at 1 m/s.
//!
//! **No hull table at all** (FX10). CP58b turned `hull::find` in a bot
//! process into the invented `hull::FIXTURE`, so the last table read here --
//! a ball with no physics block, which fell back to its type's row -- went
//! too: a ball that is not `isFree` carries mass and radius in its record
//! and no motion numbers, and the client's Destiny does not integrate it
//! (`destiny/encode.rs` `FLAG_IS_FREE` **[V]**), so here it has a top speed
//! of zero. Every number a ball moves with is now the server's own, from
//! the blob; the bot's lock range is `crate::hulls`'.
//!
//! **Why the ego drifts after an orbit fight, and the cure** (FX6, the drift
//! CP55 saw). An escort orbits its NPC while the NPC orbits it
//! (`npc::brain`'s `Phase::Orbit`): a **mutual tether**. The shared law
//! (`Motion::advanced_at` then `tethered_in_plane`) keeps the pair's
//! separation exact, but the pair as a whole translates, and by how much
//! depends on the **cadence** of the advances: the same 120 s mutual orbit
//! at 2 km, advanced every 0.05 s, 1 s and 3 s, moved the pair 79 km, 83 km
//! and 6 km (the lane note has the run). The server advances on its ticks
//! and reads, the bot on its observations, so their pairs part by tens to
//! hundreds of km in a fight, while every distance the fight reads (ego to
//! NPC) stays right. The wreck then arrives at the server's place and the
//! ego is far from it. The park cannot reproduce the server's cadence, so it
//! asks for the truth instead: when the ball the ego is tethered to leaves
//! the park, [`World::adrift`] is set until the next `SetState`, and the
//! driver answers with one `UpdateStateRequest` (`Conn::request_state`).
//! The law itself is the server's (`space_state.rs`), not this file's.

use eve_marshal::PyValue;
use eve_server::destiny::{self, Action, Mode, hull};
use eve_server::space_state::{self, Motion};
use std::collections::BTreeMap;

/// What a ball's slim item said about it: the client's only source for
/// "what is this and whose".
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SlimInfo {
    pub type_id: Option<i64>,
    pub owner_id: Option<i64>,
    /// `launcherID` on a wreck's slim: the ship that died (GAP-LOOT: an NPC
    /// wreck's owner may now be a player, so the launcher says what it was).
    pub launcher_id: Option<i64>,
    /// `lootRights` on a wreck's slim: `(owner, corp, fleet, abandoned)`
    /// (`michelle.Park.HaveLootRight`) **[V]**.
    pub loot_rights: Option<(i64, i64, Option<i64>, bool)>,
    /// The pilot, for a ship. `None` for everything without one.
    pub char_id: Option<i64>,
    pub corp_id: Option<i64>,
    /// The pilot's alliance, when the slim carries one (`allianceID`, C1's
    /// overlay; SOC-C5 hunts by it).
    pub alliance_id: Option<i64>,
    /// `hackingSecurityState` [V key]: 0 secure, 1 being hacked, 2 hacked,
    /// on a hackable can's slim (HK2); `None` on everything else (HK4).
    pub hacking_state: Option<i64>,
    /// `gateActivationRange` [V key, `menusvc.ActivateAccelerationGate`] on
    /// an acceleration gate's slim (Y-COMBAT): how near to approach before
    /// `keeper.ActivateAccelerationGate`. `None` on everything else.
    pub gate_range: Option<f64>,
    /// `securityStatus` [V key] on a pilot's ship slim (SEC-1a): what the
    /// overview's Outlaw and Dangerous flags read.
    pub security_status: Option<f64>,
    /// `otherSolarSystemClass` [V key, `wormhole.rs` `slim_fields`] on a
    /// wormhole's slim (Y-WH): the class the hole leads to (1-6 J-space, 7
    /// high, 8 low, 9 null), which a player reads off the info window
    /// before jumping. `None` on everything else.
    pub wormhole_class: Option<i64>,
}

/// One ball, as last told and carried forward.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tracked {
    pub ball_id: i64,
    /// Position, velocity, mode and throttle **as of [`Self::since`]**.
    pub motion: Motion,
    /// Metres. A rock's shrinks as it is mined (`SetBallRadius`).
    pub radius: f64,
    /// [`space_state::now_seconds`] when `motion` was last written -- the
    /// same clock, and the same field name, as the server's `Presence`.
    pub since: f64,
    pub slim: Option<SlimInfo>,
    /// The ball's own numbers from its record, then `SetMaxSpeed`: mass
    /// and radius always, top speed and agility from the physics block (a
    /// ball without one does not move: top speed zero). `None` only for a
    /// ball built by hand.
    pub own_hull: Option<hull::Hull>,
}

impl Tracked {
    /// Whether a warp is commanded for this ball: aligning for it or in it.
    /// What the bots mean by "busy travelling": nothing else to order until
    /// it lands (or the server stops it). Use [`Self::warp_active`] for "in
    /// the tunnel" (EVE: aligning is ordinary flight until activation).
    pub fn warp_commanded(&self) -> bool {
        self.motion.warp_commanded()
    }

    /// Whether the warp has activated: the ball is in the tunnel.
    pub fn warp_active(&self) -> bool {
        self.motion.warp_active()
    }

    pub fn type_id(&self) -> Option<i64> {
        self.slim.and_then(|s| s.type_id)
    }

    /// The hull this ball is integrated with: its own record's numbers
    /// ([`Self::own_hull`], module header), else `FALLBACK`. Never a type
    /// table's row (FX10).
    fn hull(&self) -> hull::Hull {
        self.own_hull.unwrap_or(hull::FALLBACK)
    }
}

/// The park, as one bot believes it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct World {
    /// The ship the camera would attach to: `SetState`'s `ego`.
    pub ego: Option<i64>,
    /// The latest destiny stamp seen. The park's clock.
    pub stamp: Option<i64>,
    /// How many `SetState`s have been applied since the last reset. Zero
    /// means "never had a state": nothing here can be trusted yet.
    pub states: u64,
    /// The ball the ego was tethered to (orbit or follow) has left the park
    /// since the last `SetState`: the ego's believed position may be far
    /// from the server's (module header). Cleared by the next `SetState`.
    pub adrift: bool,
    /// Whose ball that was (its slim's owner), while [`Self::adrift`]: a
    /// wing follower whose FC warped off grid is adrift with the FC's
    /// character here, and the driver does not resync for it (the bot
    /// re-binds on the FC's landing anyway). `None` when the slim named no
    /// owner. Cleared with `adrift`.
    pub adrift_owner: Option<i64>,
    /// The ego's own cloak mode from the last `CloakBall` (CLOAK-1): `0`
    /// while visible. No bot reads it: a gate-cloaked bot breaks it by its
    /// next order (CLOAK-2) and the brain never looks.
    pub ego_cloak: u8,
    balls: BTreeMap<i64, Tracked>,
}

impl World {
    /// Forget everything: the client's `Park.Release`. Called on dock and on
    /// jump ([`crate::SessionChange::releases_park`]).
    pub fn reset(&mut self) {
        *self = World::default();
    }

    /// Whether a `SetState` has arrived since the last reset.
    pub fn has_state(&self) -> bool {
        self.states > 0
    }

    pub fn get(&self, ball_id: i64) -> Option<&Tracked> {
        self.balls.get(&ball_id)
    }

    pub fn ego_ball(&self) -> Option<&Tracked> {
        self.get(self.ego?)
    }

    pub fn balls(&self) -> impl Iterator<Item = &Tracked> {
        self.balls.values()
    }

    /// **The rocks.** `[A]`: the real client knows a rock by its type's
    /// category (25), from static data this crate does not ship. The server
    /// allocates every rock id in `asteroid::MIN_ROCK_ID..END_ROCK_ID`, so
    /// that range is the classifier -- the same "agree with the server by
    /// construction" trade as the rest of this module, and wrong the day a
    /// rock comes from anywhere else.
    pub fn rocks(&self) -> impl Iterator<Item = &Tracked> {
        self.balls.values().filter(|b| is_rock(b.ball_id))
    }

    /// The ship a character is flying, by its slim's `charID`.
    pub fn ship_of(&self, character_id: i64) -> Option<&Tracked> {
        self.balls
            .values()
            .find(|b| b.slim.and_then(|s| s.char_id) == Some(character_id))
    }

    /// Centre distance minus both radii, in metres; negative inside. The
    /// number the client's own gates read (`surfaceDist`, `autopilot.py:624`)
    /// **[V]**. Read from the last [`Self::advance_to`].
    pub fn surface_distance(&self, a: i64, b: i64) -> Option<f64> {
        let (a, b) = (self.get(a)?, self.get(b)?);
        Some(space_state::distance(a.motion.position, b.motion.position) - a.radius - b.radius)
    }

    /// Carry every ball forward to `now` ([`space_state::now_seconds`]'s
    /// clock), with the server's law. Idempotent at a fixed `now`.
    pub fn advance_to(&mut self, now: f64) {
        let ids: Vec<i64> = self.balls.keys().copied().collect();
        for id in ids {
            self.advance_one(id, now);
        }
    }

    /// One ball, as `Registry::advance_character` does it: integrate, then
    /// put a follower or orbiter back on its tether, with the range as a
    /// centre separation (`orbit::centre_distance`) and the follower's own
    /// radius taken **from its hull**, which is the server's choice.
    fn advance_one(&mut self, id: i64, now: f64) {
        let Some(ball) = self.balls.get(&id).copied() else {
            return;
        };
        let dt = now - ball.since;
        if dt <= 0.0 {
            return;
        }
        let hull = ball.hull();
        let tether = ball
            .motion
            .tether()
            .filter(|(target, _)| *target != id)
            .and_then(|(target, range)| {
                let t = self.balls.get(&target)?;
                Some((
                    t.motion.position,
                    eve_server::orbit::centre_distance(range, f64::from(hull.radius), t.radius),
                ))
            });
        let mut motion = ball.motion.advanced_at(&hull, dt, now);
        if let Some((at, range)) = tether {
            motion.tethered_in_plane(at, range, id, now as u32);
        }
        if let Some(b) = self.balls.get_mut(&id) {
            b.motion = motion;
            b.since = now;
        }
    }

    /// Apply one `DoDestinyUpdate`'s arguments, at `now`.
    pub fn apply_update(&mut self, args: &[PyValue], now: f64) {
        for (stamp, name, operands) in crate::read_action_list(args) {
            self.stamp = Some(self.stamp.map_or(stamp, |s| s.max(stamp)));
            self.apply_action(stamp, &name, &operands, now);
        }
    }

    fn apply_action(&mut self, stamp: i64, name: &str, operands: &[PyValue], now: f64) {
        match name {
            "SetState" => self.set_state(operands, now),
            "AddBalls2" => self.add_balls(operands, now),
            // SEC-1a: `Park.OnSlimItemChange` stores the new slim as it is.
            "OnSlimItemChange" => {
                if let Some(PyValue::Int(id)) = operands.first()
                    && let Some(slim) = operands.get(1).and_then(slim_of)
                    && let Some(b) = self.balls.get_mut(id)
                {
                    b.slim = Some(slim.1);
                }
            }
            "RemoveBalls" => {
                if let Some(PyValue::List(ids) | PyValue::Tuple(ids)) = operands.first() {
                    for id in ids {
                        if let PyValue::Int(id) = id {
                            self.remove(*id);
                        }
                    }
                }
            }
            "RemoveBall" => {
                if let Some(PyValue::Int(id)) = operands.first() {
                    self.remove(*id);
                }
            }
            // `michelle.RealFlushState` removes a cloaked ball locally for
            // every ball but the ego's (CLOAK-1).
            "CloakBall" => {
                if let Some(PyValue::Int(id)) = operands.first() {
                    if Some(*id) == self.ego {
                        if let Some(PyValue::Int(mode)) = operands.get(1) {
                            self.ego_cloak = u8::try_from(*mode).unwrap_or(0);
                        }
                    } else {
                        self.remove(*id);
                    }
                }
            }
            "UncloakBall" => {
                if let Some(PyValue::Int(id)) = operands.first()
                    && Some(*id) == self.ego
                {
                    self.ego_cloak = 0;
                }
            }
            "SetMaxSpeed" => {
                if let (Some(PyValue::Int(id)), Some(max)) =
                    (operands.first(), crate::real_at(operands, 1))
                    && let Some(b) = self.balls.get_mut(id)
                {
                    let mut h = b.hull();
                    h.max_velocity = max as f32;
                    b.own_hull = Some(h);
                }
            }
            "SetBallMass" => {
                if let (Some(PyValue::Int(id)), Some(mass)) =
                    (operands.first(), crate::real_at(operands, 1))
                    && let Some(b) = self.balls.get_mut(id)
                {
                    let mut h = b.hull();
                    h.mass = mass;
                    b.own_hull = Some(h);
                }
            }
            "SetBallAgility" => {
                if let (Some(PyValue::Int(id)), Some(agility)) =
                    (operands.first(), crate::real_at(operands, 1))
                    && let Some(b) = self.balls.get_mut(id)
                {
                    let mut h = b.hull();
                    h.agility = agility as f32;
                    b.own_hull = Some(h);
                }
            }
            "SetBallRadius" => {
                if let (Some(PyValue::Int(id)), Some(r)) =
                    (operands.first(), crate::real_at(operands, 1))
                    && let Some(b) = self.balls.get_mut(id)
                {
                    b.radius = r;
                }
            }
            _ => {
                let Some(action) = action_of(name, operands) else {
                    return;
                };
                let id = action.ball();
                // **Advance before applying**, as the server does: `after`
                // turns a heading into a point relative to the position it
                // finds, so a stale position compounds.
                self.advance_one(id, now);
                let hull = match self.balls.get(&id) {
                    Some(b) => b.hull(),
                    None => return,
                };
                if let Some(b) = self.balls.get_mut(&id) {
                    let from = b.motion.position;
                    b.motion.after(action);
                    if let Action::WarpTo {
                        target, min_range, ..
                    } = action
                    {
                        refine_warp(&mut b.motion, from, target, min_range, stamp, &hull);
                    }
                    b.since = now;
                }
            }
        }
    }

    fn set_state(&mut self, operands: &[PyValue], now: f64) {
        let Some(PyValue::Instance { state: bag, .. }) = operands.first() else {
            return;
        };
        let Some(PyValue::Bytes(blob)) = crate::dict_get(bag, "state") else {
            return;
        };
        let Some(park) = destiny::Park::decode(blob) else {
            return;
        };
        let slims = match crate::dict_get(bag, "slims") {
            Some(PyValue::List(s)) => s.iter().filter_map(slim_of).collect(),
            _ => Vec::new(),
        };
        self.balls.clear();
        self.ego = match crate::dict_get(bag, "ego") {
            Some(PyValue::Int(ego)) => Some(*ego),
            _ => None,
        };
        self.insert(&park, slims, now);
        self.states += 1;
        self.adrift = false;
        self.adrift_owner = None;
    }

    /// A ball leaves the park. If the ego was tethered to it, the ego is
    /// [`Self::adrift`].
    fn remove(&mut self, id: i64) {
        let Some(gone) = self.balls.remove(&id) else {
            return;
        };
        let tethered = self
            .ego_ball()
            .and_then(|e| e.motion.tether())
            .is_some_and(|(target, _)| target == id);
        if tethered {
            self.adrift = true;
            self.adrift_owner = gone.slim.and_then(|s| s.owner_id);
        }
    }

    fn add_balls(&mut self, operands: &[PyValue], now: f64) {
        // One positional, a 2-tuple `(blob, extraBallData)`.
        let Some(PyValue::Tuple(pair)) = operands.first() else {
            return;
        };
        let Some(PyValue::Bytes(blob)) = pair.first() else {
            return;
        };
        let Some(park) = destiny::Park::decode(blob) else {
            return;
        };
        let slims = match pair.get(1) {
            // Each entry is a bare slim dict, or `(dict, damageState)`.
            Some(PyValue::List(extra)) => extra
                .iter()
                .filter_map(|e| match e {
                    PyValue::Tuple(p) => p.first().and_then(slim_of),
                    other => slim_of(other),
                })
                .collect(),
            _ => Vec::new(),
        };
        self.insert(&park, slims, now);
    }

    fn insert(&mut self, park: &destiny::Park, slims: Vec<(i64, SlimInfo)>, now: f64) {
        let slims: BTreeMap<i64, SlimInfo> = slims.into_iter().collect();
        for b in &park.balls {
            let (velocity, speed_fraction) = match b.physics {
                Some(p) => (p.velocity, p.speed_fraction),
                None => ([0.0; 3], 0.0),
            };
            let own_hull = Some(hull::Hull {
                type_id: slims.get(&b.ball_id).and_then(|s| s.type_id).unwrap_or(0),
                mass: b.mass,
                radius: b.radius,
                max_velocity: b.physics.map_or(0.0, |p| p.max_velocity),
                agility: b.physics.map_or(hull::FALLBACK.agility, |p| p.agility),
                ..hull::FALLBACK
            });
            self.balls.insert(
                b.ball_id,
                Tracked {
                    ball_id: b.ball_id,
                    motion: Motion {
                        position: b.position,
                        velocity,
                        mode: b.mode,
                        speed_fraction,
                    },
                    radius: f64::from(b.radius),
                    since: now,
                    slim: slims.get(&b.ball_id).copied(),
                    own_hull,
                },
            );
        }
    }
}

/// `[A]` -- see [`World::rocks`].
pub fn is_rock(ball_id: i64) -> bool {
    (eve_server::asteroid::MIN_ROCK_ID..eve_server::asteroid::END_ROCK_ID).contains(&ball_id)
        // A moon belt's rock (MN-C) is a rock too, in its own id band.
        || eve_server::moon_belt::is_moon_id(ball_id)
}

/// **The warp, refined the way `service::space::warp` refines it**, so the
/// bot's warping ball has the same future as the server's.
///
/// `Motion::after(WarpTo)` alone leaves an aligning warp with no length, which
/// the integrator can only carry as an align for ever. The server then sets
/// the activation stamp to `stamp + ceil(align_seconds)`, the `goto` to
/// `warp_arrival(position, target, min_range)` and the length to the distance
/// there. The action's `target` and `min_range` are exactly what that
/// function used (`aim`, `standoff`), and its stamp is the batch's -- so this
/// is the same computation from the same inputs. **[I]** that the batch's
/// stamp is the one `warp` allocated: they are the same `next_destiny_stamp`
/// call today.
fn refine_warp(
    motion: &mut Motion,
    from: [f64; 3],
    target: [f64; 3],
    min_range: f64,
    stamp: i64,
    hull: &hull::Hull,
) {
    let arrival = destiny::warp_arrival(from, target, min_range);
    let align_s = hull::align_seconds(hull);
    if let Mode::Warp {
        goto,
        effect_stamp,
        total_distance,
        ..
    } = &mut motion.mode
    {
        let stamp = u32::try_from(stamp).unwrap_or(0);
        *effect_stamp = stamp.saturating_add(align_s.ceil().max(0.0) as u32) as i32;
        *goto = arrival;
        *total_distance = space_state::distance(from, arrival);
    }
}

/// One movement action's operands as the server's own `Action`, or `None`
/// for anything that is not one -- the inverse of `destiny::Action::args`.
fn action_of(name: &str, ops: &[PyValue]) -> Option<Action> {
    let ball = match ops.first()? {
        PyValue::Int(id) => *id,
        _ => return None,
    };
    let triple = || -> Option<[f64; 3]> {
        Some([
            crate::real_at(ops, 1)?,
            crate::real_at(ops, 2)?,
            crate::real_at(ops, 3)?,
        ])
    };
    let target = || match ops.get(1) {
        Some(PyValue::Int(t)) => Some(*t),
        _ => None,
    };
    Some(match name {
        "SetSpeedFraction" => Action::SetSpeedFraction {
            ball,
            fraction: crate::real_at(ops, 1)?,
        },
        "GotoDirection" => Action::GotoDirection {
            ball,
            direction: triple()?,
        },
        "GotoPoint" => Action::GotoPoint {
            ball,
            point: triple()?,
        },
        "Stop" => Action::Stop { ball },
        "FollowBall" => Action::FollowBall {
            ball,
            target: target()?,
            range: crate::real_at(ops, 2)?,
        },
        "Orbit" => Action::Orbit {
            ball,
            target: target()?,
            range: crate::real_at(ops, 2)?,
        },
        "SetBallPosition" => Action::SetBallPosition {
            ball,
            point: triple()?,
        },
        "SetBallVelocity" => Action::SetBallVelocity {
            ball,
            velocity: triple()?,
        },
        "WarpTo" => Action::WarpTo {
            ball,
            target: triple()?,
            min_range: crate::real_at(ops, 4).unwrap_or(0.0),
            warp_factor: match ops.get(5) {
                Some(PyValue::Int(f)) => *f,
                _ => 1_000,
            },
        },
        _ => return None,
    })
}

/// A slim item -- a `foo.SlimItem` instance in a `SetState`, a plain dict in
/// an `AddBalls2` -- as `(itemID, what it says)`.
fn slim_of(v: &PyValue) -> Option<(i64, SlimInfo)> {
    let dict = match v {
        PyValue::Instance { state, .. } => state.as_ref(),
        other => other,
    };
    let int = |key: &str| match crate::dict_get(dict, key) {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    };
    Some((
        int("itemID")?,
        SlimInfo {
            type_id: int("typeID"),
            owner_id: int("ownerID"),
            launcher_id: int("launcherID"),
            loot_rights: match crate::dict_get(dict, "lootRights") {
                Some(PyValue::Tuple(t)) => match t.as_slice() {
                    [PyValue::Int(o), PyValue::Int(c), f, PyValue::Bool(a)] => Some((
                        *o,
                        *c,
                        match f {
                            PyValue::Int(f) => Some(*f),
                            _ => None,
                        },
                        *a,
                    )),
                    _ => None,
                },
                _ => None,
            },
            char_id: int("charID"),
            corp_id: int("corpID"),
            alliance_id: int("allianceID"),
            hacking_state: int(eve_server::hacking::KEY_SECURITY_STATE),
            gate_range: match crate::dict_get(dict, eve_server::combat_site::KEY_GATE_RANGE) {
                Some(PyValue::Real(r)) => Some(*r),
                Some(PyValue::Int(n)) => Some(*n as f64),
                _ => None,
            },
            security_status: match crate::dict_get(dict, "securityStatus") {
                Some(PyValue::Real(r)) => Some(*r),
                _ => None,
            },
            wormhole_class: int("otherSolarSystemClass"),
        },
    ))
}

#[cfg(test)]
mod tests {
    //! Frames built with the server's own encoders (`docs/lanes/bots.md` §7,
    //! T2), so these test the reading, not a copy of the writing.

    use super::*;
    use eve_server::destiny::{Ball, BallsAdded, BallsRemoved, Bootstrap, Park, Physics, Slim};
    use eve_server::notify::Push;

    const SHIP: i64 = 100_000_001;
    const OTHER: i64 = 100_000_002;
    const ROCK: i64 = eve_server::asteroid::MIN_ROCK_ID;
    /// A ship type from the invented `hull::FIXTURE`; nothing here reads
    /// its row (FX10).
    const MERLIN: i64 = 603;

    fn ball(id: i64, at: [f64; 3], radius: f32, moving: bool) -> Ball {
        Ball {
            ball_id: id,
            mode: Mode::Stop,
            radius,
            position: at,
            flags: 0,
            mass: 1.0e6,
            cloak: 0,
            harmonic: -1,
            corporation_id: 0,
            alliance_id: 0,
            physics: moving.then_some(Physics {
                max_velocity: 300.0,
                velocity: [0.0; 3],
                agility: 1.0,
                speed_fraction: 0.0,
            }),
            formation_id: destiny::NO_FORMATION,
        }
    }

    fn slim(id: i64, type_id: i64, char_id: Option<i64>) -> Slim {
        Slim {
            item_id: id,
            type_id,
            owner_id: char_id.unwrap_or(1),
            char_id,
            corp_id: None,
            response_thresholds: None,
        }
    }

    fn state(now: f64) -> World {
        let park = Park {
            current_time: 0,
            balls: vec![
                ball(SHIP, [0.0; 3], 40.0, true),
                ball(ROCK, [50_000.0, 0.0, 0.0], 2_000.0, false),
            ],
        };
        let boot = Bootstrap::new(
            100,
            park,
            SHIP,
            vec![slim(SHIP, MERLIN, Some(90_000_001)), slim(ROCK, 1230, None)],
            vec![],
        );
        let mut w = World::default();
        w.apply_update(&boot.to_push().1, now);
        w
    }

    #[test]
    fn a_set_state_gives_balls_slims_and_the_ego() {
        let w = state(1_000.0);
        assert!(w.has_state());
        assert_eq!(w.ego, Some(SHIP));
        assert_eq!(w.stamp, Some(100));
        assert_eq!(w.ego_ball().and_then(Tracked::type_id), Some(MERLIN));
        assert_eq!(w.ship_of(90_000_001).map(|b| b.ball_id), Some(SHIP));
        assert_eq!(w.rocks().map(|b| b.ball_id).collect::<Vec<_>>(), vec![ROCK]);
        assert_eq!(
            w.surface_distance(SHIP, ROCK),
            Some(50_000.0 - 40.0 - 2_000.0)
        );
    }

    #[test]
    fn add_remove_and_radius_are_incremental() {
        let mut w = state(1_000.0);
        let added = BallsAdded::new(
            vec![1],
            101,
            Park {
                current_time: 0,
                balls: vec![ball(OTHER, [0.0, 9_000.0, 0.0], 40.0, true)],
            },
            vec![(slim(OTHER, MERLIN, Some(90_000_002)), Some(PyValue::None))],
        )
        .expect("a batch");
        w.apply_update(&added.to_push().1, 1_000.0);
        assert_eq!(w.ship_of(90_000_002).map(|b| b.ball_id), Some(OTHER));
        assert_eq!(w.balls().count(), 3, "added, not replaced");

        let shrink =
            eve_server::mining::RockRadius::new(vec![1], 102, ROCK, 1_500.0).expect("a batch");
        w.apply_update(&shrink.to_push().1, 1_000.0);
        assert_eq!(w.get(ROCK).map(|b| b.radius), Some(1_500.0));

        let gone = BallsRemoved::new(vec![1], 103, vec![ROCK]).expect("a batch");
        w.apply_update(&gone.to_push().1, 1_000.0);
        assert!(w.get(ROCK).is_none());
        assert_eq!(w.stamp, Some(103));
    }

    #[test]
    fn a_follow_is_carried_to_its_tether_as_the_server_carries_it() {
        // The server snaps a follower onto `range + both radii` from its
        // target on every advance (`advance_character`); so must we, or the
        // bot's "in range" and the server's disagree.
        let mut w = state(1_000.0);
        let follow = eve_server::destiny::ActionUpdate::new(
            vec![1],
            101,
            vec![Action::FollowBall {
                ball: SHIP,
                target: ROCK,
                range: 500.0,
            }],
        )
        .expect("a batch");
        w.apply_update(&follow.to_push().1, 1_000.0);
        w.advance_to(1_010.0);
        // The follower's own radius is its record's (the server writes the
        // record from the hull it integrates with).
        let own = w.ego_ball().expect("ego").radius;
        let want = 500.0 + own + 2_000.0;
        let got = space_state::distance(
            w.ego_ball().expect("ego").motion.position,
            w.get(ROCK).expect("rock").motion.position,
        );
        assert!((got - want).abs() < 1e-6, "{got} vs {want}");
    }

    #[test]
    fn a_warp_lands_where_the_server_says_it_lands() {
        let mut w = state(1_000.0);
        let target = [1.0e9, 0.0, 0.0];
        let warp = eve_server::destiny::ActionUpdate::new(
            vec![1],
            1_000,
            vec![Action::WarpTo {
                ball: SHIP,
                target,
                min_range: 10_000.0,
                warp_factor: 3_000,
            }],
        )
        .expect("a batch");
        w.apply_update(&warp.to_push().1, 1_000.0);
        assert!(w.ego_ball().expect("ego").warp_commanded());
        // An hour later the warp is long over: the ball is stopped at the
        // server's arrival point.
        w.advance_to(4_600.0);
        let ego = w.ego_ball().expect("ego");
        assert!(!ego.warp_commanded(), "{:?}", ego.motion.mode);
        let arrival = destiny::warp_arrival([0.0; 3], target, 10_000.0);
        assert!(space_state::distance(ego.motion.position, arrival) < 1.0);
    }

    /// A type the bot's hull table does not know (an invented id).
    const UNKNOWN_TYPE: i64 = 990_001;

    fn goto(w: &mut World, id: i64, now: f64) {
        let go = eve_server::destiny::ActionUpdate::new(
            vec![1],
            101,
            vec![
                Action::SetSpeedFraction {
                    ball: id,
                    fraction: 1.0,
                },
                Action::GotoPoint {
                    ball: id,
                    point: [1.0e9, 0.0, 0.0],
                },
            ],
        )
        .expect("a batch");
        w.apply_update(&go.to_push().1, now);
    }

    /// **FX6: a ball flies with its own record's numbers**, as the client's
    /// Destiny does, and not as `hull::FALLBACK` when its type is not in the
    /// bot's table.
    #[test]
    fn a_ball_the_hull_table_does_not_know_flies_with_its_own_record() {
        let mut w = World::default();
        let park = Park {
            current_time: 0,
            balls: vec![ball(OTHER, [0.0; 3], 40.0, true)],
        };
        let boot = Bootstrap::new(
            100,
            park.clone(),
            OTHER,
            vec![slim(OTHER, UNKNOWN_TYPE, None)],
            vec![],
        );
        w.apply_update(&boot.to_push().1, 1_000.0);
        goto(&mut w, OTHER, 1_000.0);
        w.advance_to(1_010.0);

        let b = park.balls[0];
        let p = b.physics.expect("a free ball");
        let own = hull::Hull {
            mass: b.mass,
            radius: b.radius,
            max_velocity: p.max_velocity,
            agility: p.agility,
            ..hull::FALLBACK
        };
        let mut want = Motion {
            position: b.position,
            velocity: p.velocity,
            mode: Mode::Stop,
            speed_fraction: 1.0,
        };
        want.after(Action::GotoPoint {
            ball: OTHER,
            point: [1.0e9, 0.0, 0.0],
        });
        let want = want.advanced_at(&own, 10.0, 1_010.0);
        let got = w.get(OTHER).expect("the ball").motion.position;
        assert!(
            space_state::distance(got, want.position) < 1e-6,
            "{got:?} vs {:?}",
            want.position
        );
        assert!(
            got[0] > f64::from(hull::FALLBACK.max_velocity) * 10.0,
            "faster than the fallback hull could: {got:?}"
        );
    }

    /// **FX10: a ball with no physics block does not move**, whatever its
    /// type, as the client's Destiny leaves a ball that is not `isFree`; and
    /// it keeps its record's mass and radius.
    #[test]
    fn a_ball_without_physics_keeps_its_record_and_stays_put() {
        let mut w = state(1_000.0);
        let rock = w.get(ROCK).and_then(|b| b.own_hull).expect("its record");
        assert_eq!((rock.radius, rock.max_velocity), (2_000.0, 0.0));
        goto(&mut w, ROCK, 1_000.0);
        w.advance_to(1_060.0);
        assert_eq!(
            w.get(ROCK).map(|b| b.motion.position),
            Some([50_000.0, 0.0, 0.0])
        );
    }

    /// `SetMaxSpeed` (a module came on or off) changes how fast the ball
    /// flies from then on.
    #[test]
    fn set_max_speed_changes_the_balls_top_speed() {
        let run = |max: Option<f64>| {
            let mut w = state(1_000.0);
            if let Some(max) = max {
                let up = eve_server::destiny::ActionUpdate::new(
                    vec![1],
                    101,
                    vec![Action::SetMaxSpeed { ball: SHIP, max }],
                )
                .expect("a batch");
                w.apply_update(&up.to_push().1, 1_000.0);
            }
            goto(&mut w, SHIP, 1_000.0);
            w.advance_to(1_030.0);
            w.ego_ball().expect("ego").motion.position[0]
        };
        let base = run(None);
        let own = state(1_000.0)
            .ego_ball()
            .and_then(|b| b.own_hull)
            .expect("a free ball");
        let faster = run(Some(f64::from(own.max_velocity) * 2.0));
        assert!(faster > base * 1.5, "{faster} vs {base}");
    }

    /// `SetBallMass` and `SetBallAgility` (a propulsion module, a plate)
    /// change the ball's own numbers, and leave its top speed alone.
    #[test]
    fn set_ball_mass_and_agility_change_the_balls_own_numbers() {
        let mut w = state(1_000.0);
        let before = w.ego_ball().and_then(|b| b.own_hull).expect("a free ball");
        let up = eve_server::destiny::ActionUpdate::new(
            vec![1],
            101,
            vec![
                Action::SetBallAgility {
                    ball: SHIP,
                    agility: 0.625,
                },
                Action::SetBallMass {
                    ball: SHIP,
                    mass: 7_654_321.0,
                },
            ],
        )
        .expect("a batch");
        w.apply_update(&up.to_push().1, 1_000.0);
        let after = w.ego_ball().and_then(|b| b.own_hull).expect("its record");
        assert_eq!(after.mass, 7_654_321.0);
        assert_eq!(after.agility, 0.625);
        assert_eq!(after.max_velocity, before.max_velocity);
    }

    /// **FX6, the cure for CP55's drift**: when the ball the ego orbits
    /// leaves the park, the park says it is adrift until a fresh state.
    #[test]
    fn losing_the_egos_tether_target_sets_adrift_until_the_next_state() {
        let mut w = state(1_000.0);
        let added = BallsAdded::new(
            vec![1],
            101,
            Park {
                current_time: 0,
                balls: vec![ball(OTHER, [0.0, 9_000.0, 0.0], 40.0, true)],
            },
            vec![(slim(OTHER, MERLIN, None), Some(PyValue::None))],
        )
        .expect("a batch");
        w.apply_update(&added.to_push().1, 1_000.0);

        // Another ball leaving says nothing about the ego.
        let gone = BallsRemoved::new(vec![1], 102, vec![ROCK]).expect("a batch");
        w.apply_update(&gone.to_push().1, 1_000.0);
        assert!(!w.adrift);

        let orbit = eve_server::destiny::ActionUpdate::new(
            vec![1],
            103,
            vec![Action::Orbit {
                ball: SHIP,
                target: OTHER,
                range: 2_000.0,
            }],
        )
        .expect("a batch");
        w.apply_update(&orbit.to_push().1, 1_000.0);
        let gone = BallsRemoved::new(vec![1], 104, vec![OTHER]).expect("a batch");
        w.apply_update(&gone.to_push().1, 1_001.0);
        assert!(w.adrift, "the ego's orbit target left");

        let boot = Bootstrap::new(
            105,
            Park {
                current_time: 0,
                balls: vec![ball(SHIP, [0.0; 3], 40.0, true)],
            },
            SHIP,
            vec![slim(SHIP, MERLIN, Some(90_000_001))],
            vec![],
        );
        w.apply_update(&boot.to_push().1, 1_002.0);
        assert!(!w.adrift, "a SetState is the truth again");
    }

    /// `michelle` removes a cloaked ball locally for every ball but the
    /// ego's (CLOAK-1): the park drops another ship's, and records the
    /// ego's own mode.
    #[test]
    fn a_cloak_removes_another_ball_and_records_the_egos_own() {
        let mut w = state(1_000.0);
        assert!(w.get(ROCK).is_some());
        let other = eve_server::destiny::ActionUpdate::new(
            vec![1],
            101,
            vec![Action::CloakBall {
                ball: ROCK,
                mode: 1,
            }],
        )
        .expect("a batch");
        w.apply_update(&other.to_push().1, 1_001.0);
        assert!(w.get(ROCK).is_none(), "the other ball is dropped");
        assert_eq!(w.ego_cloak, 0);
        let own = eve_server::destiny::ActionUpdate::new(
            vec![1],
            102,
            vec![Action::CloakBall {
                ball: SHIP,
                mode: 1,
            }],
        )
        .expect("a batch");
        w.apply_update(&own.to_push().1, 1_002.0);
        assert!(w.get(SHIP).is_some(), "the ego stays");
        assert_eq!(w.ego_cloak, 1);
        let off = eve_server::destiny::ActionUpdate::new(
            vec![1],
            103,
            vec![Action::UncloakBall { ball: SHIP }],
        )
        .expect("a batch");
        w.apply_update(&off.to_push().1, 1_003.0);
        assert_eq!(w.ego_cloak, 0);
    }

    /// The ball the ego was tethered to names its owner when it leaves, so
    /// the driver can tell a wing follower whose FC warped off from a bot
    /// that lost a rock; the next state clears it.
    #[test]
    fn the_lost_tether_target_names_its_owner() {
        const FC_CHAR: i64 = 90_000_007;
        let mut w = state(1_000.0);
        let added = BallsAdded::new(
            vec![1],
            101,
            Park {
                current_time: 0,
                balls: vec![ball(OTHER, [0.0, 9_000.0, 0.0], 40.0, true)],
            },
            vec![(slim(OTHER, MERLIN, Some(FC_CHAR)), Some(PyValue::None))],
        )
        .expect("a batch");
        w.apply_update(&added.to_push().1, 1_000.0);
        let follow = eve_server::destiny::ActionUpdate::new(
            vec![1],
            103,
            vec![Action::FollowBall {
                ball: SHIP,
                target: OTHER,
                range: 500.0,
            }],
        )
        .expect("a batch");
        w.apply_update(&follow.to_push().1, 1_000.0);
        let gone = BallsRemoved::new(vec![1], 104, vec![OTHER]).expect("a batch");
        w.apply_update(&gone.to_push().1, 1_001.0);
        assert!(w.adrift);
        assert_eq!(w.adrift_owner, Some(FC_CHAR));

        let boot = Bootstrap::new(
            105,
            Park {
                current_time: 0,
                balls: vec![ball(SHIP, [0.0; 3], 40.0, true)],
            },
            SHIP,
            vec![slim(SHIP, MERLIN, Some(90_000_001))],
            vec![],
        );
        w.apply_update(&boot.to_push().1, 1_002.0);
        assert!(!w.adrift);
        assert_eq!(w.adrift_owner, None);
    }

    #[test]
    fn a_reset_forgets_everything() {
        let mut w = state(1_000.0);
        w.reset();
        assert!(!w.has_state());
        assert_eq!(w.balls().count(), 0);
        assert_eq!(w.ego, None);
    }

    /// PA-10 T10: one merged frame (same-stamp batches concatenated in
    /// order, which is what `--coalesce-destiny` sends) gives the same park
    /// as the separate frames applied in order.
    #[test]
    fn a_merged_frame_leaves_the_park_where_the_separate_frames_do() {
        let follow = eve_server::destiny::ActionUpdate::new(
            vec![1],
            101,
            vec![Action::FollowBall {
                ball: SHIP,
                target: ROCK,
                range: 500.0,
            }],
        )
        .expect("a batch");
        let shrink =
            eve_server::mining::RockRadius::new(vec![1], 101, ROCK, 1_500.0).expect("a batch");
        let gone = BallsRemoved::new(vec![1], 101, vec![ROCK]).expect("a batch");
        let frames: Vec<Vec<PyValue>> =
            vec![follow.to_push().1, shrink.to_push().1, gone.to_push().1];
        let mut separate = state(1_000.0);
        for f in &frames {
            separate.apply_update(f, 1_000.0);
        }
        let mut entries = Vec::new();
        for f in &frames {
            let PyValue::List(e) = &f[0] else {
                panic!("a batch is a list")
            };
            entries.extend(e.iter().cloned());
        }
        assert!(entries.len() >= 3);
        let mut merged = state(1_000.0);
        merged.apply_update(&[PyValue::List(entries), PyValue::Int(0)], 1_000.0);
        assert!(merged.get(ROCK).is_none(), "the removal applied last");
        assert_eq!(merged.stamp, separate.stamp);
        assert_eq!(merged, separate);
    }
}
