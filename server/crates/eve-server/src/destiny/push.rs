//! The Destiny push types: every `crate::notify::Push` this module sends,
//! and the wreck a kill leaves.
//!
//! Moved verbatim out of `destiny.rs` (IP36-d); `destiny` re-exports every
//! item here with `pub use push::*`, so every caller's path is unchanged.
//! Not to be confused with the crate-root `crate::push`, the push path.
//! The module doc of [`super`] is the spec this code satisfies.

use super::{
    Action, Ball, Bootstrap, DamageState, FLAG_IS_FREE, FLAG_IS_MASSIVE, FLAGS_STATIC_BALL, Mode,
    NO_FORMATION, Park, ShipModules, Slim, SlimExtras, SlimModule,
};
use crate::pyutil::instance;
use eve_marshal::PyValue;

#[cfg(doc)]
use super::rock_ball;

/// The Destiny state a ship undocks into, with the character it is for.
///
/// **The only `Do*` push in the project**, and the one exception to the
/// `On`-prefix rule `dispatch::notify` documents. The reason is checkable:
/// `michelle.DoDestinyUpdate` only logs, expands entries and appends to
/// `Park.history`, so nothing that could block runs inside the `block_trap`
/// window.
///
/// Carries the whole [`Bootstrap`] rather than a finished payload for the
/// same reason `crate::fleet::Push` carries a `FleetEvent`: the client
/// evidence for the shape belongs beside the encoder that has the bytes, not
/// in the dispatcher.
#[derive(Debug, Clone, PartialEq)]
pub struct StateUpdate {
    pub to: Vec<i64>,
    pub state: Bootstrap,
}

impl crate::notify::Push for StateUpdate {
    /// Both clocks, for the reason [`Bootstrap::stamp`] gives: the two
    /// numbers must be equal or the park starts out of step with every
    /// action that follows it.
    fn restamp(&mut self, stamp: u32) {
        self.state.stamp = stamp;
        self.state.park.current_time = stamp;
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        self.state.to_push()
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    /// A `SetState` replaces the park: `Park.SetState` calls `ClearAll`
    /// first **[V]** (see [`BallsAdded`]).
    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Resets(self.state.park.balls.iter().map(|b| b.ball_id).collect())
    }
}

/// A batch of actions at one stamp, for one or more characters.
///
/// **The stamp is a field of the batch, not of an action**, so
/// `michelle.py:659-666`'s rule cannot be broken by construction: a batch with
/// more than one distinct stamp logs `Found update batch with N items and M
/// timestamps`, calls `clientStatsSvc.OnFatalDesync()` -- and then **flushes
/// the batch anyway** **[V]**, which is worse than a clean refusal.
/// [`Bootstrap`] already does the same thing for the bootstrap's one entry.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionUpdate {
    to: Vec<i64>,
    stamp: u32,
    actions: Vec<Action>,
}

impl ActionUpdate {
    /// A batch, or `None` if there is nothing safe to send.
    ///
    /// Refuses two things locally rather than being refused remotely, which is
    /// the same bargain [`Bootstrap::every_slim_has_a_ball`] makes:
    ///
    /// - **an empty batch**, which the client logs as `Empty state received
    ///   from remote ballpark` and drops (`michelle.py:1428`, `:1511`) **[V]**;
    /// - **a non-finite float**, which a native discards with no trace at all
    ///   ([`Action::is_finite`]).
    pub fn new(to: Vec<i64>, stamp: u32, actions: Vec<Action>) -> Option<Self> {
        if actions.is_empty() || !actions.iter().all(|a| a.is_finite()) {
            return None;
        }
        Some(ActionUpdate { to, stamp, actions })
    }

    pub fn stamp(&self) -> u32 {
        self.stamp
    }

    pub fn actions(&self) -> &[Action] {
        &self.actions
    }
}

impl crate::notify::Push for ActionUpdate {
    /// Bind the stamp at send time, for a batch that was scheduled earlier.
    ///
    /// Every entry in a batch shares one stamp by construction -- the stamp
    /// is a field of the batch and not of an action, which is why
    /// `michelle.py:659-666`'s `Found update batch with N items and M
    /// timestamps` cannot be provoked from here -- so restamping is one
    /// assignment and cannot break that invariant. See
    /// `crate::notify::Push::restamp`.
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    /// `("DoDestinyUpdate", [[(stamp, (name, args)), ...], 0])`.
    ///
    /// One entry is `(stamp, (funcName, args))`, unpacked twice by
    /// `Park.RealFlushState` -- `eventStamp, event = state[0]` then
    /// `funcName, args = event` (`michelle.py:1515-1516`) **[V]** -- so each
    /// level has exactly two elements and `args` is the whole positional list
    /// as one tuple, because every dispatch site is `CALL_FUNCTION_VAR 0`.
    ///
    /// Same envelope as [`Bootstrap::to_push`]: same notification, same
    /// `waitForBubble = 0`, no third argument. The differences are that the
    /// name is not `SetState`, the arguments are scalars rather than a
    /// `KeyVal`, and a batch may hold several entries -- provided every stamp
    /// is equal, which [`ActionUpdate::new`] guarantees by holding one.
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let entries = self
            .actions
            .iter()
            .map(|a| {
                PyValue::Tuple(vec![
                    PyValue::Int(i64::from(self.stamp)),
                    PyValue::Tuple(vec![
                        // A Python 2 `str`, because the client does
                        // `'_parent_' + funcName` and because that is what its
                        // own writer emits.
                        crate::pyutil::bstr(a.name()),
                        PyValue::Tuple(a.args()),
                    ]),
                ])
            })
            .collect();
        (
            "DoDestinyUpdate",
            vec![PyValue::List(entries), PyValue::Int(0)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    /// A warp's arrival reconciliation, read from the actions themselves:
    /// `SetBallPosition`, `SetBallVelocity`, `Stop`, `SetBallMassive`, all on
    /// one ball (the observers' batch `warp_to_point` builds), or a lone
    /// `SetBallMassive` (the pilot's). Names the same batches as the old test
    /// on `to_push` (`service::space::is_warp_settle_slow`).
    fn cancel_tag(&self) -> Option<crate::notify::CancelTag> {
        use super::Action::{SetBallMassive, SetBallPosition, SetBallVelocity, Stop};
        let ball = match self.actions.as_slice() {
            [SetBallMassive { ball, .. }] => *ball,
            [
                SetBallPosition { ball, .. },
                SetBallVelocity { ball: b2, .. },
                Stop { ball: b3 },
                SetBallMassive { ball: b4, .. },
            ] if ball == b2 && ball == b3 && ball == b4 => *ball,
            _ => return None,
        };
        Some(crate::notify::CancelTag::WarpSettle { ball })
    }

    /// Every ball an entry acts on: its first argument, which is what the
    /// native looks up (`every_action_names_its_ball_first`). An orbit's
    /// *target* is not counted -- `docs/lanes/grid-research.md` R6.
    fn grid(&self) -> crate::grid::GridRole {
        let mut balls: Vec<i64> = self.actions.iter().map(|a| a.ball()).collect();
        balls.sort_unstable();
        balls.dedup();
        // A batch of cloaks takes the balls out of the holders' parks (the
        // client removes a cloaked ball itself); anything else only names.
        if self
            .actions
            .iter()
            .all(|a| matches!(a, super::Action::CloakBall { .. }))
        {
            crate::grid::GridRole::Hides(balls)
        } else {
            crate::grid::GridRole::Names(balls)
        }
    }

    /// The entries about the balls `keep` admits, in order, at the same
    /// stamp. One batch carries a whole system's NPC moves (`crate::npc`),
    /// and a pilot who holds three of five NPCs is sent those three.
    fn narrowed(
        &self,
        to: Vec<i64>,
        keep: &dyn Fn(i64) -> bool,
    ) -> Option<Box<dyn crate::notify::Push>> {
        let actions = self
            .actions
            .iter()
            .copied()
            .filter(|a| keep(a.ball()))
            .collect();
        ActionUpdate::new(to, self.stamp, actions).map(|u| Box::new(u) as _)
    }
}

/// One ball appearing in a park that is already running.
///
/// **`AddBalls2`, never a fresh `SetState`.** A `SetState` is a reset:
/// `Park.SetState` calls `ClearAll`, re-reads the blob in mode 0, wipes
/// `slimItems`, reassigns `self.ego` and restarts the native park **[V]**. The
/// player sees every model and bracket destroyed and rebuilt, the camera snap
/// back, and -- the one that is not cosmetic -- their own ship **teleport** to
/// whatever position the server last stored, a jump that grows with how long
/// they have been flying. Once per other player entering the system.
/// `AddBalls2` touches none of those: not `ego`, not `validState`, not
/// `ClearAll`, not `_parent_Start` **[V]**.
///
/// **The blob is our ordinary one, byte for byte.** Round 2's spec said an
/// append needs packet type 1; it does not. The type byte is read and
/// bounds-checked by `ReadFullStateFromStream` and then **never referenced
/// again** -- what selects append is the function's *second* argument, which
/// `michelle.AddBalls2` passes as `2` **[V]**, and which we do not control or
/// need to. So [`Park::encode`] is reused unchanged, and the encoder the
/// client's own bytes already test stays the only one.
///
/// **The header tick still matters.** The append path stores the blob's
/// four-byte tick into the observer's ballpark **before any mode test**
/// **[V]**, so an `AddBalls2` whose blob clock disagrees with its stamp moves
/// that observer's clock somewhere the server did not choose, and the next
/// action's forward-evolve loop measures against it. [`BallsAdded::new`] takes
/// one stamp and writes it into both, the way [`Bootstrap::new`] does.
#[derive(Debug, Clone, PartialEq)]
pub struct BallsAdded {
    pub(super) to: Vec<i64>,
    pub(super) stamp: u32,
    pub(super) park: Park,
    /// One entry per ball, in the park's order: the slim item, and the health
    /// of anything that has any.
    pub(super) extra: Vec<(Slim, Option<PyValue>)>,
    /// Wrecks, after `extra` and in the park's order: their slims carry the
    /// three keys [`Slim`] has not ([`WreckSlim`]). Only the grid's
    /// reconcile adds a wreck this way ([`Bootstrap::added`]); a kill's own
    /// wreck rides in [`Kill`].
    pub(super) wrecks: Vec<WreckSlim>,
    /// The fitted modules of the ships among `extra` ([`SlimModule`]).
    pub(super) modules: ShipModules,
    /// Extra attributes for slims among `extra`, by item id
    /// ([`crate::slim_extra`]).
    pub(super) extras: SlimExtras,
    /// The effects already running on the balls added (FX-REPLAY): each goes
    /// out as an `OnSpecialFX` entry after the `AddBalls2`, in the same stamp
    /// and batch, so it cannot overtake the add and be dropped for a ball not
    /// yet in the park (the way `jump::JumpFx` rides the Destiny batch).
    pub(super) fx: Vec<crate::modules::SpecialFx>,
}

impl BallsAdded {
    /// A batch whose blob clock is its stamp, or `None` if there is nothing
    /// to send.
    ///
    /// Refuses an empty batch for the same reason [`ActionUpdate::new`] does:
    /// the client logs `Empty state received from remote ballpark` and drops
    /// it. Refuses a slim that names no ball in the blob because
    /// `ProcessBallAdd:1612` is a **plain subscript** -- a `KeyError`, not the
    /// tidy `BallNotInPark` a bad `SetState` raises -- and one bad entry kills
    /// the whole append.
    pub fn new(
        to: Vec<i64>,
        stamp: u32,
        mut park: Park,
        extra: Vec<(Slim, Option<PyValue>)>,
    ) -> Option<Self> {
        if park.balls.is_empty() || extra.is_empty() {
            return None;
        }
        if !extra
            .iter()
            .all(|(s, _)| park.balls.iter().any(|b| b.ball_id == s.item_id))
        {
            return None;
        }
        park.current_time = stamp;
        Some(BallsAdded {
            to,
            stamp,
            park,
            extra,
            wrecks: Vec::new(),
            modules: Vec::new(),
            extras: Vec::new(),
            fx: Vec::new(),
        })
    }

    /// The same batch with its ships' fitted modules, which go out as each
    /// ship slim's `modules` ([`SlimModule`]).
    pub fn with_modules(mut self, modules: ShipModules) -> Self {
        self.modules = modules;
        self
    }

    /// The same batch with extra attributes on some of its slims
    /// ([`crate::slim_extra`]).
    pub fn with_extras(mut self, extras: SlimExtras) -> Self {
        self.extras = extras;
        self
    }

    /// The same batch with the effects running on its balls, replayed after
    /// the add ([`crate::modules::active_fx`]).
    pub fn with_fx(mut self, fx: Vec<crate::modules::SpecialFx>) -> Self {
        self.fx = fx;
        self
    }
}

impl crate::notify::Push for BallsAdded {
    /// **Both clocks, or neither.** `BallsAdded::new` writes the stamp into
    /// `park.current_time` as well, because the batch's stamp decides when
    /// the client flushes the entry and the blob's header decides what the
    /// client's clock *becomes* when it does. A `restamp` that set only
    /// `self.stamp` would ship a batch whose envelope and blob disagree --
    /// the failure [`Bootstrap::stamp`] warns about, arriving through a
    /// method that looks like a one-liner. Watched to fail:
    /// `every_stamped_push_can_be_restamped` catches exactly that half fix.
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
        self.park.current_time = stamp;
    }

    /// `("DoDestinyUpdate", [[(stamp, ("AddBalls2", ((blob, extra),)))], 0])`.
    ///
    /// **One positional argument, which is a 2-tuple.** The entry's own
    /// argument tuple holds exactly one element and that element is
    /// `(state, extraBallData)` **[V]**.
    ///
    /// Each element of `extraBallData` is a **`(dict, damageState)` tuple**,
    /// and both halves of that are traps:
    ///
    /// - It must be a real tuple. The client's test is `type(data) is tuple`,
    ///   an identity check against the exact type **[V]**, so a two-element
    ///   *list* is taken for the slim dict itself, `__dict__ = [..]` raises,
    ///   and the whole append dies -- including the balls after it.
    /// - A **bare dict** is legal and wrong for a ship:
    ///   `AddBalls2:1700` then stores `damageState[itemID] = (None, now)`,
    ///   `GetDamageState` returns `None` for it, and the gauges are
    ///   **hidden** **[V]**. That is the empty-health-bar defect of
    ///   `docs/port-tracker.md` §19.9 and §23 arriving by a third door, so
    ///   the pair form is used for anything with health and the bare dict
    ///   only for a station.
    ///
    /// The slim is a plain attribute **dict**, not a `foo.SlimItem` instance:
    /// the client does `slimItem.__dict__ = slimItemDict` **[V]**. That is the
    /// one place this differs from the bootstrap, which sends instances.
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let extra: Vec<PyValue> = self
            .extra
            .iter()
            .map(|(slim, damage)| {
                let dict = slim.to_dict_with(&self.modules, &self.extras);
                match damage {
                    Some(d) => PyValue::Tuple(vec![dict, d.clone()]),
                    None => dict,
                }
            })
            // A wreck is a bare dict: no gauges, like a rock (see [`Kill`]).
            .chain(self.wrecks.iter().map(|w| w.to_dict()))
            .collect();
        let entry = PyValue::Tuple(vec![
            PyValue::Int(i64::from(self.stamp)),
            PyValue::Tuple(vec![
                crate::pyutil::bstr("AddBalls2"),
                PyValue::Tuple(vec![PyValue::Tuple(vec![
                    PyValue::Bytes(self.park.encode()),
                    PyValue::List(extra),
                ])]),
            ]),
        ]);
        // The running effects, after the add and in its stamp: `OnSpecialFX`
        // is not Destiny-critical, so it costs the park no `Evolve`.
        let fx = self.fx.iter().map(|f| {
            PyValue::Tuple(vec![
                PyValue::Int(i64::from(self.stamp)),
                PyValue::Tuple(vec![
                    crate::pyutil::bstr("OnSpecialFX"),
                    PyValue::Tuple(crate::notify::Push::to_push(f).1),
                ]),
            ])
        });
        let entries: Vec<PyValue> = std::iter::once(entry).chain(fx).collect();
        (
            "DoDestinyUpdate",
            vec![PyValue::List(entries), PyValue::Int(0)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    /// Each ball with where it is, so the drain can tell whose grid it is on
    /// without looking it up again.
    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Adds(
            self.park
                .balls
                .iter()
                .map(|b| (b.ball_id, b.position))
                .collect(),
        )
    }

    /// The balls `keep` admits, each with its slim, gauges and modules: a
    /// warm system's rocks reach each pilot as that pilot's belt
    /// (`crate::asteroid`).
    fn narrowed(
        &self,
        to: Vec<i64>,
        keep: &dyn Fn(i64) -> bool,
    ) -> Option<Box<dyn crate::notify::Push>> {
        let balls: Vec<Ball> = self
            .park
            .balls
            .iter()
            .copied()
            .filter(|b| keep(b.ball_id))
            .collect();
        if balls.is_empty() {
            return None;
        }
        Some(Box::new(BallsAdded {
            to,
            stamp: self.stamp,
            park: Park {
                current_time: self.park.current_time,
                balls,
            },
            extra: self
                .extra
                .iter()
                .filter(|(s, _)| keep(s.item_id))
                .cloned()
                .collect(),
            wrecks: self
                .wrecks
                .iter()
                .copied()
                .filter(|w| keep(w.item_id))
                .collect(),
            modules: self
                .modules
                .iter()
                .filter(|(id, _)| keep(*id))
                .cloned()
                .collect(),
            extras: self
                .extras
                .iter()
                .filter(|(id, _)| keep(*id))
                .cloned()
                .collect(),
            fx: self.fx.clone(),
        }))
    }
}

/// Balls leaving an observer's park, because their pilot docked or left.
///
/// **One argument, and that is the whole trap.** `Park.RemoveBalls` is
/// `(ballIDs, exploders=None, isRelease=False)` **[V]**, and
/// `RealFlushState:1575-1577` appends its *own* `exploders` dict as the second
/// positional before calling it. A server that helpfully sends two arguments
/// therefore has its second one land in `isRelease`. It survives most of the
/// time, because `exploders` is built only from `TerminalPlayDestructionEffect`
/// entries and is `{}` in any batch without an explosion -- which is exactly
/// what makes it worth pinning the **arity** rather than waiting for the
/// symptom. When it does fire, `bracketMgr.DoBallsRemove` closes every bracket
/// on that player's screen.
///
/// Plural rather than the singular `RemoveBall`, deliberately: `audioService`
/// and `environmentSvc` listen only for the plural `DoBallsRemove` **[V]**. The
/// singular is the right call for a ship that was *destroyed*
/// (`RemoveBall(id, terminal=True)` does the explosion in one entry with no
/// `exploders` slot to get wrong), which this server has no way to cause yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BallsRemoved {
    to: Vec<i64>,
    stamp: u32,
    balls: Vec<i64>,
}

impl BallsRemoved {
    pub fn new(to: Vec<i64>, stamp: u32, balls: Vec<i64>) -> Option<Self> {
        if balls.is_empty() || to.is_empty() {
            return None;
        }
        Some(BallsRemoved { to, stamp, balls })
    }
}

impl crate::notify::Push for BallsRemoved {
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let entry = PyValue::Tuple(vec![
            PyValue::Int(i64::from(self.stamp)),
            PyValue::Tuple(vec![
                crate::pyutil::bstr("RemoveBalls"),
                // **Exactly one element.** See the type's own note.
                PyValue::Tuple(vec![PyValue::List(
                    self.balls.iter().copied().map(PyValue::Int).collect(),
                )]),
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

    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Removes(self.balls.clone())
    }

    /// The balls `keep` admits, still one positional argument.
    fn narrowed(
        &self,
        to: Vec<i64>,
        keep: &dyn Fn(i64) -> bool,
    ) -> Option<Box<dyn crate::notify::Push>> {
        let balls = self.balls.iter().copied().filter(|b| keep(*b)).collect();
        BallsRemoved::new(to, self.stamp, balls).map(|r| Box::new(r) as _)
    }
}

/// A visual effect, which is how a warp gets a tunnel.
///
/// **Not an [`Action`], deliberately.** Every `Action` is a native
/// `destiny.Ballpark` method, reached by the client as
/// `getattr(self, '_parent_' + funcName)(*args)` (`michelle.py:1581`) and
/// carrying nothing but numbers. `OnSpecialFX` is the other route out of the
/// same batch: a **Python** method on `Park`, in
/// `nonDestinyCriticalFunctions` (`michelle.py:1048-1057`), dispatched by
/// `getattr(self, funcName)(*args)` at `:1553` **without**
/// `SynchroniseToSimulationTime` **[V]**. Its arguments include `None`s and a
/// guid string, so putting it in `Action` would have cost that enum its
/// "every argument after the ball is a number" invariant -- and that
/// invariant is a real test (`every_action_names_its_ball_first`) guarding a
/// real client requirement.
///
/// ## The nine arguments are copied from the client's own call
///
/// `Park.OnSpecialFX` is `(self, shipID, moduleID, moduleTypeID, targetID,
/// otherTypeID, guid, isOffensive, start, active, duration=-1, repeat=None,
/// startTime=None, timeFromStart=0, graphicInfo=None)` -- `argcount 15` with
/// **five** defaults, read off the `MAKE_FUNCTION 5` at offset 564 of the
/// `Park` class body with its five `LOAD_CONST`s `-1, None, None, 0, None`
/// **[V]**. So nine positionals is not a guess at a minimum: it is the
/// signature's own boundary.
///
/// And nine is what the client itself passes, at `spaceMgr.py:268` **[V]**:
///
/// ```text
/// self.FxSequencer.OnSpecialFX(ball.id, None, None, None, None,
///                              'effects.Warping', 0, 1, 0)
/// ```
///
/// which it does when a ball arrives **already in** `DSTBALL_WARP` and it is
/// the player's own ship -- the reconnect-into-warp path. That is the same
/// event we need for a warp that starts normally, so this sends exactly those
/// nine values and invents none of them.
///
/// ## Why it reaches the screen
///
/// `Park.OnSpecialFX` scatters once (`moduleID` is `None`, and
/// `isinstance(None, collections.Iterable)` is false, so the per-module loop
/// at `michelle.py:2090` is skipped) **[V]**, and
/// `fxSequencer.OnSpecialFX` (`:441`) has three early returns, none of which
/// fires here **[V]**:
///
/// - `:477` `start == 1 and guid in self.disabledGuids` -- a graphics
///   setting, the player's to set;
/// - `:480` `IsTravelEffect(guid) and sm.GetService('subway').Enabled()` --
///   `res:/staticdata/traveleffects.fsdbinary` holds **only** the jump
///   family (`effects.JumpOut`, `JumpDriveOut`, `JumpBridgeOut`,
///   `JumpOutAbyssal`, …), and `effects.Warping` is **not in it** **[V]**, so
///   the subway service does not swallow this the way it swallows a gate
///   jump;
/// - `:485` a `FX_LONG_ONESHOT_GUIDS` staleness check, which needs a
///   `startTime`, and ours defaults to `None`.
///
/// ## One event for everyone watching
///
/// `Warping.Prepare` (`Warp.py:43-48`) stops `effects.Warping` and starts
/// `effects.WarpOut` instead when the effect's ship is **not** the player's
/// **[V]**. So the same nine arguments give the pilot a tunnel and every
/// observer a departure flash, and the audience is simply everyone whose park
/// holds the ball.
#[derive(Debug, Clone, PartialEq)]
pub struct SpecialFx {
    to: Vec<i64>,
    stamp: u32,
    ball: i64,
    guid: &'static str,
    /// `start` in the signature: 1 begins the effect, 0 ends it.
    start: bool,
}

/// The warp tunnel, and the observers' departure flash.
pub const FX_WARPING: &str = "effects.Warping";

impl SpecialFx {
    pub fn new(
        to: Vec<i64>,
        stamp: u32,
        ball: i64,
        guid: &'static str,
        start: bool,
    ) -> Option<Self> {
        if to.is_empty() {
            return None;
        }
        Some(SpecialFx {
            to,
            stamp,
            ball,
            guid,
            start,
        })
    }
}

impl crate::notify::Push for SpecialFx {
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let entry = PyValue::Tuple(vec![
            PyValue::Int(i64::from(self.stamp)),
            PyValue::Tuple(vec![
                crate::pyutil::bstr("OnSpecialFX"),
                PyValue::Tuple(vec![
                    PyValue::Int(self.ball),
                    // moduleID, moduleTypeID, targetID, otherTypeID: a warp
                    // is not a module firing at anything.
                    PyValue::None,
                    PyValue::None,
                    PyValue::None,
                    PyValue::None,
                    crate::pyutil::bstr(self.guid),
                    // isOffensive. `Warping` never reads it; the client
                    // passes 0 here and so do we.
                    PyValue::Int(0),
                    PyValue::Int(i64::from(self.start)),
                    // active. 0 in the client's own call, which is the only
                    // observation of this argument in this position.
                    PyValue::Int(0),
                ]),
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

    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Names(vec![self.ball])
    }
}

/// One ship's bars changed: `OnDamageStateChange(shipID, damageState)`.
///
/// **A destiny action, not a notification** -- it is a method of
/// `michelle.Park`, which is not a service, so the only door is a
/// `DoDestinyUpdate` batch (`docs/re/damage-death.md` §0.2 point 1) **[V]**.
/// The signature is `(self, shipID, damageState)`, `michelle.py:2070`
/// (`co_varnames`) **[V]**, and the handler stores
/// `damageState[shipID] = (state, blue.os.GetSimTime())` (`:2076`) -- so the
/// regeneration curve restarts from *the client's* clock on arrival, and the
/// state is exactly the per-ball shape [`DamageState`] already encodes for
/// `SetState`.
///
/// It is in `nonDestinyCriticalFunctions` (`michelle.py:1048-1057`), so
/// `RealFlushState:1553` applies it **without** `SynchroniseToSimulationTime`
/// **[V]**: a batch of these cannot desync the park, and may repeat the last
/// stamp. The one-stamp-per-batch rule still applies, and holds by
/// construction -- one entry per batch.
#[derive(Debug, Clone, PartialEq)]
pub struct DamageStateChange {
    to: Vec<i64>,
    stamp: u32,
    ball: i64,
    state: DamageState,
}

impl DamageStateChange {
    /// `None` for nobody to tell.
    pub fn new(to: Vec<i64>, stamp: u32, ball: i64, state: DamageState) -> Option<Self> {
        if to.is_empty() {
            return None;
        }
        Some(DamageStateChange {
            to,
            stamp,
            ball,
            state,
        })
    }
}

impl crate::notify::Push for DamageStateChange {
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    /// `("DoDestinyUpdate", [[(stamp, ("OnDamageStateChange", (shipID,
    /// state)))], 0])`. The same envelope as every other batch here.
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let entry = PyValue::Tuple(vec![
            PyValue::Int(i64::from(self.stamp)),
            PyValue::Tuple(vec![
                crate::pyutil::bstr("OnDamageStateChange"),
                PyValue::Tuple(vec![PyValue::Int(self.ball), self.state.to_pyvalue()]),
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

    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Names(vec![self.ball])
    }
}

/// Wreck type 26550's radius -- every wreck `crate::combat::WRECKS` names is
/// 14.0 m in `types.fsdbinary` **[V]**.
pub const WRECK_RADIUS: f32 = 14.0;

/// A wreck's slim item: the five ordinary attributes plus the three only a
/// wreck has (`docs/re/damage-death.md` §6) **[V]**.
///
/// - `launcherID`: the ball that exploded. `Wreck.Prepare:74` looks it up to
///   inherit its rotation and the explosion's timing, which is why the
///   wreck's `AddBalls2` rides in the same batch as the removal, **ahead** of
///   it ([`Kill`]).
/// - `isEmpty`: picks the bracket's full/empty icon
///   (`invControllers.py:1910`).
/// - `lootRights`: a **4-tuple** `(ownerID, corpID, fleetID, abandoned)`,
///   unpacked by `Park.IsAbandoned:1948` and `Park.HaveLootRight:1959`.
///
/// No `charID`: `ProcessBallAdd` primes an owner only for a `charID` that is
/// not `None`, and a wreck has no pilot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WreckSlim {
    pub item_id: i64,
    pub type_id: i64,
    pub owner_id: i64,
    pub corp_id: i64,
    pub launcher_id: i64,
    pub is_empty: bool,
    /// `lootRights[2]`: the owner's fleet when the wreck was made.
    pub fleet_id: Option<i64>,
    /// `lootRights[3]`: the owner gave the rights up.
    pub abandoned: bool,
}

impl WreckSlim {
    /// The attribute pairs, shared by both forms below.
    ///
    /// `lootRights = (owner, owner's corp, owner's fleet or None, abandoned)`
    /// and `ownerID` the owner (GAP-LOOT). `[A]` that `ownerID` is the
    /// wreck's owner as well; falsified by an overview that colours the wreck
    /// as someone else's.
    fn pairs(self) -> Vec<(&'static str, PyValue)> {
        vec![
            ("itemID", PyValue::Int(self.item_id)),
            ("typeID", PyValue::Int(self.type_id)),
            ("ownerID", PyValue::Int(self.owner_id)),
            ("corpID", PyValue::Int(self.corp_id)),
            ("launcherID", PyValue::Int(self.launcher_id)),
            ("isEmpty", PyValue::Bool(self.is_empty)),
            (
                "lootRights",
                PyValue::Tuple(vec![
                    PyValue::Int(self.owner_id),
                    PyValue::Int(self.corp_id),
                    self.fleet_id.map_or(PyValue::None, PyValue::Int),
                    PyValue::Bool(self.abandoned),
                ]),
            ),
        ]
    }

    /// A plain dict, as `AddBalls2` wants a slim (`slimItem.__dict__ =
    /// slimItemDict`), like [`Slim`]'s.
    pub(super) fn to_dict(self) -> PyValue {
        crate::pyutil::pydict(self.pairs())
    }

    /// A `foo.SlimItem` instance, as `SetState`'s `slims` wants one -- the
    /// same split [`Slim`] has between its two forms **[V]**.
    pub(super) fn to_pyvalue(self) -> PyValue {
        instance("foo.SlimItem", self.pairs())
    }
}

/// A wreck as a ball: [`rock_ball`]'s shape at the dead ship's position.
///
/// Not free, `Mode::Stop`, mass `0.0`: `[A]`, the planets' and rocks'
/// precedent -- a wreck does not move under our simulation. Falsified by a
/// wreck the client draws drifting or snapping.
pub fn wreck_ball(wreck_id: i64, position: [f64; 3]) -> Ball {
    Ball {
        ball_id: wreck_id,
        mode: Mode::Stop,
        radius: WRECK_RADIUS,
        position,
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

/// A ship's death, as every client in its park sees it: **one batch, one
/// stamp**, the wreck's `AddBalls2` first, the ship's `RemoveBall` second and
/// the pilot's capsule's `AddBalls2` third (`docs/re/damage-death.md` §0
/// steps 8-10, `crate::damage::Death::steps`'s `kill_batch` steps) **[V]**.
///
/// - **`RemoveBall(ballID, terminal)`**, the singular: `michelle.py:1762`,
///   `(self, ballID, terminal)` by `co_varnames` **[V]**. With
///   `terminal = True` it sets the explosion itself and plays it; and the
///   singular is not in `:1575`'s `RemoveBalls` branch, so no `exploders`
///   argument is ever appended -- the trap [`BallsRemoved`] documents does
///   not exist on this path.
/// - **The wreck before the removal**, because `Wreck.Prepare:74` resolves
///   `launcherID` to the dead ship's ball, and `RealFlushState` applies a
///   batch in order.
/// - **The capsule after it**, in the `(dict, damageState)` pair form a ship
///   needs ([`BallsAdded::to_push`]): it is a ship with gauges. Its order
///   against the session change is free (`gameui.DoBallAdd:1116`'s
///   `wannaBeEgo`, §4.2) **[V]**; against the removal it is §0's order.
/// - **Every clock from one field**: each blob header is written from
///   `stamp` at encode time, so [`Push::restamp`](crate::notify::Push::restamp)
///   cannot move one without the others -- [`BallsAdded`]'s rule.
#[derive(Debug, Clone, PartialEq)]
pub struct Kill {
    to: Vec<i64>,
    stamp: u32,
    victim: i64,
    wreck: Option<(Ball, WreckSlim)>,
    capsule: Option<(Ball, Slim, DamageState)>,
}

impl Kill {
    /// `wreck` is `None` for a death that leaves none (a capsule's), and
    /// `capsule` for one that ejects none (also a capsule's). `None` for
    /// nobody to tell, or for a slim that names a different ball than the
    /// one it rides with.
    pub fn new(
        to: Vec<i64>,
        stamp: u32,
        victim: i64,
        wreck: Option<(Ball, WreckSlim)>,
        capsule: Option<(Ball, Slim, DamageState)>,
    ) -> Option<Self> {
        if to.is_empty() {
            return None;
        }
        // `ProcessBallAdd:1612` is a plain subscript: a slim naming no ball
        // in the blob is a `KeyError` that kills the whole batch.
        if let Some((ball, slim)) = wreck
            && ball.ball_id != slim.item_id
        {
            return None;
        }
        if let Some((ball, slim, _)) = capsule
            && ball.ball_id != slim.item_id
        {
            return None;
        }
        Some(Kill {
            to,
            stamp,
            victim,
            wreck,
            capsule,
        })
    }
}

impl crate::notify::Push for Kill {
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let stamp = PyValue::Int(i64::from(self.stamp));
        let add = |ball: Ball, extra: PyValue| {
            let park = Park {
                current_time: self.stamp,
                balls: vec![ball],
            };
            PyValue::Tuple(vec![
                stamp.clone(),
                PyValue::Tuple(vec![
                    crate::pyutil::bstr("AddBalls2"),
                    PyValue::Tuple(vec![PyValue::Tuple(vec![
                        PyValue::Bytes(park.encode()),
                        PyValue::List(vec![extra]),
                    ])]),
                ]),
            ])
        };
        let mut entries = Vec::new();
        if let Some((ball, slim)) = self.wreck {
            // A **bare** dict, not a `(dict, damageState)` pair: a wreck has
            // no gauges, and `AddBalls2:1700` stores `(None, now)` for it --
            // the hidden-gauge path that is right for a wreck and a station
            // and wrong for a ship. See `BallsAdded::to_push`.
            entries.push(add(ball, slim.to_dict()));
        }
        entries.push(PyValue::Tuple(vec![
            stamp.clone(),
            PyValue::Tuple(vec![
                crate::pyutil::bstr("RemoveBall"),
                PyValue::Tuple(vec![PyValue::Int(self.victim), PyValue::Bool(true)]),
            ]),
        ]));
        if let Some((ball, slim, damage)) = self.capsule {
            entries.push(add(
                ball,
                PyValue::Tuple(vec![slim.to_dict(&[]), damage.to_pyvalue()]),
            ));
        }
        (
            "DoDestinyUpdate",
            vec![PyValue::List(entries), PyValue::Int(0)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    /// **One batch, per viewer whose park holds the victim** (GRID-B,
    /// `docs/lanes/grid-research.md` §5 "Kill"). The wreck and the capsule
    /// appear where the ship died, so a client that held the ship has them
    /// on its grid; one that did not is told nothing here and is given the
    /// wreck by the next reconcile if its grid reaches it. Never split:
    /// `Wreck.Prepare:74` resolves `launcherID` to the dying ball, so the
    /// wreck without the removal behind it would be a wreck with no launcher.
    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Swaps {
            gone: self.victim,
            adds: self
                .wreck
                .iter()
                .map(|(b, _)| b.ball_id)
                .chain(self.capsule.iter().map(|(b, _, _)| b.ball_id))
                .collect(),
        }
    }
}

/// A wreck's slim item, changed: `OnSlimItemChange(itemID, newSlim)` in a
/// `DoDestinyUpdate`, to every client whose park holds the wreck.
///
/// **What turns a looted-out wreck's bracket empty.** `Park.OnSlimItemChange`
/// (`michelle.py:2006`, `(self, itemID, newSlim)` by `co_varnames`) **[V]**
/// stores `newSlim` into `slimItems` as it is -- no `foo.SlimItem()` is built
/// around it the way `AddBalls2` builds one around a dict -- so the slim goes
/// in its **instance** form, [`WreckSlim::to_pyvalue`], the one `SetState`
/// uses. It then scatters `OnSlimItemChange(old, new)` and tells the ball
/// (`OnSlimItemUpdated`); `invCtrl.ItemWreck.GetIconName`
/// (`invControllers.py:1915`) reads `slimItem.isEmpty` out of `slimItems` to
/// pick the bracket icon **[V]**.
///
/// It is in `nonDestinyCriticalFunctions` (`michelle.py:1052`) **[V]**, like
/// [`DamageStateChange`], so it is applied without resynchronising the park;
/// one entry, one stamp.
#[derive(Debug, Clone, PartialEq)]
pub struct SlimItemChange {
    to: Vec<i64>,
    stamp: u32,
    slim: WreckSlim,
}

impl SlimItemChange {
    /// `None` for nobody to tell.
    pub fn new(to: Vec<i64>, stamp: u32, slim: WreckSlim) -> Option<Self> {
        if to.is_empty() {
            return None;
        }
        Some(SlimItemChange { to, stamp, slim })
    }
}

impl crate::notify::Push for SlimItemChange {
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let entry = PyValue::Tuple(vec![
            PyValue::Int(i64::from(self.stamp)),
            PyValue::Tuple(vec![
                crate::pyutil::bstr("OnSlimItemChange"),
                PyValue::Tuple(vec![
                    PyValue::Int(self.slim.item_id),
                    self.slim.to_pyvalue(),
                ]),
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

    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Names(vec![self.slim.item_id])
    }
}

/// One entry of a [`ShipHandoff`].
#[derive(Debug, Clone, PartialEq)]
pub enum HandoffEntry {
    /// A destiny action on a ball.
    Act(Action),
    /// `OnSlimItemChange(slim.item_id, slim)`: the slim in its **instance**
    /// form, with the ship's fitted modules and its slim extras, because the
    /// client stores it as it is (`michelle.py:2006`) and `IsSubSystemChange`
    /// reads `.modules` of a modular hull.
    Slim {
        slim: Slim,
        modules: Vec<SlimModule>,
        extras: SlimExtras,
    },
}

impl HandoffEntry {
    /// The ball the entry is about, which is what the grid filter reads.
    pub fn subject(&self) -> i64 {
        match self {
            HandoffEntry::Act(a) => a.ball(),
            HandoffEntry::Slim { slim, .. } => slim.item_id,
        }
    }
}

/// **A ship changing hands in space** (BE-2): an eject, or a boarding, as
/// every client in the park sees it. **One batch, one stamp**, entries in
/// the order the caller built them.
///
/// The ball a pilot leaves becomes interactive-off (`SetBallInteractive`,
/// bit 0x08 of the flags byte), stops, and gets a slim with no `charID`; the
/// ball a pilot boards becomes interactive and gets the pilot's slim. Both are
/// `nonDestinyCriticalFunctions` entries or plain setters, so none of them
/// resynchronises the park **[V]**.
///
/// Each entry is about one ball, so a viewer whose park holds only the ship
/// that stayed is sent its entries and not the others' (`narrowed`).
#[derive(Debug, Clone, PartialEq)]
pub struct ShipHandoff {
    to: Vec<i64>,
    stamp: u32,
    entries: Vec<HandoffEntry>,
}

impl ShipHandoff {
    /// `None` for nobody to tell, or nothing to say, or a non-finite number
    /// (`ActionUpdate::new`'s rule).
    pub fn new(to: Vec<i64>, stamp: u32, entries: Vec<HandoffEntry>) -> Option<Self> {
        if to.is_empty() || entries.is_empty() {
            return None;
        }
        let finite = entries.iter().all(|e| match e {
            HandoffEntry::Act(a) => a.is_finite(),
            HandoffEntry::Slim { .. } => true,
        });
        finite.then_some(ShipHandoff { to, stamp, entries })
    }

    pub fn entries(&self) -> &[HandoffEntry] {
        &self.entries
    }

    pub fn stamp(&self) -> u32 {
        self.stamp
    }
}

impl crate::notify::Push for ShipHandoff {
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let stamp = PyValue::Int(i64::from(self.stamp));
        let entries = self
            .entries
            .iter()
            .map(|e| {
                let (name, args) = match e {
                    HandoffEntry::Act(a) => (a.name(), a.args()),
                    HandoffEntry::Slim {
                        slim,
                        modules,
                        extras,
                    } => (
                        "OnSlimItemChange",
                        vec![
                            PyValue::Int(slim.item_id),
                            slim.to_pyvalue_with(&[(slim.item_id, modules.clone())], extras),
                        ],
                    ),
                };
                PyValue::Tuple(vec![
                    stamp.clone(),
                    PyValue::Tuple(vec![crate::pyutil::bstr(name), PyValue::Tuple(args)]),
                ])
            })
            .collect();
        (
            "DoDestinyUpdate",
            vec![PyValue::List(entries), PyValue::Int(0)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    fn grid(&self) -> crate::grid::GridRole {
        let mut balls: Vec<i64> = self.entries.iter().map(HandoffEntry::subject).collect();
        balls.sort_unstable();
        balls.dedup();
        crate::grid::GridRole::Names(balls)
    }

    fn narrowed(
        &self,
        to: Vec<i64>,
        keep: &dyn Fn(i64) -> bool,
    ) -> Option<Box<dyn crate::notify::Push>> {
        let entries = self
            .entries
            .iter()
            .filter(|e| keep(e.subject()))
            .cloned()
            .collect();
        ShipHandoff::new(to, self.stamp, entries).map(|u| Box::new(u) as _)
    }
}

// `FLAG_IS_MASSIVE` is in `encode`: a missile collides with its target.
// `LaunchMissile`'s `massive = 1` calls `SetBallMassive` anyway **[V]**
// (R-MISSILE §1.3), so the blob's bit only has to agree with it.

/// **A missile ball's flag byte**: `isFree | isGlobal | isMassive`.
///
/// `[A]` the global bit stays set, as on every other ball this server sends
/// until grid phase 2 (`crate::grid::FLAG_IS_GLOBAL`): `MissileFollow`
/// cancels a missile whose bubble differs from its target's **[V]**, and a
/// global target with a bubbled missile is the case nobody has tested.
/// R-MISSILE §1.1's `0x05` (retail, FREE | MASSIVE) is the fallback.
/// Falsified by the client logging `in different bubbles` for our missile.
pub const FLAGS_MISSILE: u8 = FLAG_IS_FREE | crate::grid::FLAG_IS_GLOBAL | FLAG_IS_MASSIVE;

/// The `launchModules` entry for an NPC: an NPC's launcher is its
/// `modules[0]` (`entityShip.FitHardpoints`) **[V]**.
pub const NPC_LAUNCH_MODULE: i64 = 0;

/// **A missile, as the ball its `AddBalls2` carries** (R-MISSILE §1.1).
///
/// Holds the facts rather than a finished [`Ball`] so that the one clock
/// the tail carries, `effectStamp`, is written from the batch's stamp at
/// encode time ([`MissileBall::ball`]) and cannot disagree with it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MissileBall {
    /// The missile's own ball id.
    pub id: i64,
    /// The launcher's ball: the player's ship, or the NPC itself.
    /// `mOwnerId` and `LaunchMissile`'s third argument.
    pub owner: i64,
    /// The target's ball: `mFollowId` and `LaunchMissile`'s second argument.
    pub target: i64,
    /// Where the launcher is. `PyLaunchMissile` moves the missile there
    /// anyway **[V]**.
    pub from: [f64; 3],
    /// Where the target is, for the launch direction.
    pub toward: [f64; 3],
    /// The charge type's `radius`.
    pub radius: f64,
    /// The target's radius, for `mFollowRange = -(rm + rt)` **[V]**.
    pub target_radius: f64,
    /// The charge type's `mass`.
    pub mass: f64,
    /// The charge's attribute 37 (times the entity's 645 for an NPC).
    pub max_velocity: f64,
    /// The charge's attribute 70.
    pub agility: f64,
    pub corporation_id: i32,
    pub alliance_id: i32,
}

impl MissileBall {
    /// The ball record, mode 5 with its tail stamped `stamp`, or `None` when
    /// there is no launch direction (launcher on the target, or a non-finite
    /// position): a normalised zero would be a NaN the native drops in
    /// silence ([`Action::is_finite`]).
    pub fn ball(&self, stamp: u32) -> Option<Ball> {
        // `Mode::missile` falls back to `goto = from` with no direction;
        // a launch is refused instead, because `LaunchMissile` would aim it
        // along a zero vector.
        crate::space_state::unit([
            self.toward[0] - self.from[0],
            self.toward[1] - self.from[1],
            self.toward[2] - self.from[2],
        ])?;
        Some(Ball {
            ball_id: self.id,
            // The tail by Carbon's `MissileFollow` rule (lane m1).
            mode: Mode::missile(
                self.target,
                self.owner,
                stamp as i32,
                self.radius,
                self.target_radius,
                self.from,
                self.toward,
            ),
            radius: self.radius as f32,
            position: self.from,
            flags: FLAGS_MISSILE,
            mass: self.mass,
            cloak: 0,
            harmonic: -1,
            corporation_id: self.corporation_id,
            alliance_id: self.alliance_id,
            physics: Some(super::Physics {
                max_velocity: self.max_velocity as f32,
                velocity: [0.0; 3],
                agility: self.agility as f32,
                // `MissileFollow` sets 1.0 **[V]**.
                speed_fraction: 1.0,
            }),
            formation_id: NO_FORMATION,
        })
    }
}

/// **A missile's slim item**: a bare dict, like a wreck's, with no
/// damage-state pair -- a missile has no gauges (R-MISSILE §1.2).
///
/// - `typeID` is the **charge**: `Missile.LoadModel` reads it for both the
///   missile graphic and the impact graphic (`missile.py:90-98`) **[V]**.
/// - `sourceShipID` is the launcher ship's ball (`missile.py:88`) **[V]**.
/// - `launchModules` is a list: the launcher item ids for a player, keyed
///   like the ship slim's `modules`, or `[NPC_LAUNCH_MODULE]` for an NPC
///   (`missile.py:89`, `_GetModelTurret`) **[V]**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissileSlim {
    pub item_id: i64,
    pub type_id: i64,
    /// The launching character, or the NPC's owner corporation.
    pub owner_id: i64,
    pub source_ship_id: i64,
    pub launch_modules: Vec<i64>,
}

impl MissileSlim {
    /// The slim for an NPC's missile: `launchModules == [0]`.
    pub fn npc(item_id: i64, type_id: i64, owner_id: i64, npc_ball: i64) -> Self {
        MissileSlim {
            item_id,
            type_id,
            owner_id,
            source_ship_id: npc_ball,
            launch_modules: vec![NPC_LAUNCH_MODULE],
        }
    }

    /// A plain dict, as `AddBalls2` wants a slim.
    pub fn to_dict(&self) -> PyValue {
        crate::pyutil::pydict(vec![
            ("itemID", PyValue::Int(self.item_id)),
            ("typeID", PyValue::Int(self.type_id)),
            ("ownerID", PyValue::Int(self.owner_id)),
            ("sourceShipID", PyValue::Int(self.source_ship_id)),
            (
                "launchModules",
                PyValue::List(
                    self.launch_modules
                        .iter()
                        .copied()
                        .map(PyValue::Int)
                        .collect(),
                ),
            ),
        ])
    }
}

/// **A missile appearing and launched, in one batch at one stamp**
/// (R-MISSILE §1.3): `AddBalls2((blob, [slim]))` then
/// `LaunchMissile(missile, target, launcher, aimed = 1, massive = 1)`.
///
/// - **The add first**: the action must name a ball the park holds **[V]**.
/// - **One stamp, three places**: the batch entries, the blob header and
///   the tail's `effectStamp`. All three are written from `stamp` when the
///   push is encoded, so [`Push::restamp`](crate::notify::Push::restamp)
///   moves them together. If the tail's stamp and the launch's differed, the
///   800 ms straight-flight window would land on the wrong tick `[I]`.
/// - **The audience is every client holding both the launcher and the
///   target** ([`crate::grid::GridRole::Spawns`]): `MissileFollow` cancels
///   without the target **[V]**, and `LaunchMissile` returns silently
///   without the launcher.
#[derive(Debug, Clone, PartialEq)]
pub struct MissileLaunched {
    to: Vec<i64>,
    stamp: u32,
    missile: MissileBall,
    slim: MissileSlim,
}

impl MissileLaunched {
    /// `None` for nobody to tell, a slim that names another ball (the
    /// `ProcessBallAdd` `KeyError` [`BallsAdded::new`] refuses), ids that
    /// `MissileFollow` would cancel as `Inconsistent ids for missiles`
    /// **[V]**, or no launch direction ([`MissileBall::ball`]).
    pub fn new(to: Vec<i64>, stamp: u32, missile: MissileBall, slim: MissileSlim) -> Option<Self> {
        if to.is_empty() || slim.item_id != missile.id {
            return None;
        }
        let MissileBall {
            id, owner, target, ..
        } = missile;
        if id == target || id == owner || target == owner {
            return None;
        }
        missile.ball(stamp)?;
        Some(MissileLaunched {
            to,
            stamp,
            missile,
            slim,
        })
    }

    /// The missile's ball id.
    pub fn ball_id(&self) -> i64 {
        self.missile.id
    }

    fn launch(&self) -> Action {
        Action::LaunchMissile {
            ball: self.missile.id,
            target: self.missile.target,
            owner: self.missile.owner,
            aimed: true,
            massive: true,
        }
    }
}

impl crate::notify::Push for MissileLaunched {
    /// One field, three clocks: see the type.
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let stamp = PyValue::Int(i64::from(self.stamp));
        let ball = self
            .missile
            .ball(self.stamp)
            .expect("MissileLaunched::new checked the direction");
        let park = Park {
            current_time: self.stamp,
            balls: vec![ball],
        };
        let add = PyValue::Tuple(vec![
            stamp.clone(),
            PyValue::Tuple(vec![
                crate::pyutil::bstr("AddBalls2"),
                PyValue::Tuple(vec![PyValue::Tuple(vec![
                    PyValue::Bytes(park.encode()),
                    // A bare dict: no gauges (see [`MissileSlim`]).
                    PyValue::List(vec![self.slim.to_dict()]),
                ])]),
            ]),
        ]);
        let launch = self.launch();
        let fire = PyValue::Tuple(vec![
            stamp,
            PyValue::Tuple(vec![
                crate::pyutil::bstr(launch.name()),
                PyValue::Tuple(launch.args()),
            ]),
        ]);
        (
            "DoDestinyUpdate",
            vec![PyValue::List(vec![add, fire]), PyValue::Int(0)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    /// Whole, to the clients holding the launcher, the source ship and the
    /// target; and the missile joins their parks, so the removal that ends
    /// it reaches exactly them.
    fn grid(&self) -> crate::grid::GridRole {
        let mut needs = vec![
            self.missile.owner,
            self.slim.source_ship_id,
            self.missile.target,
        ];
        needs.sort_unstable();
        needs.dedup();
        crate::grid::GridRole::Spawns {
            needs,
            adds: vec![self.missile.id],
        }
    }
}

impl Kill {
    /// **A missile that missed: `RemoveBall(id, True)` and nothing else.**
    ///
    /// `terminal = True` sets `EXPLOSION`, and `Missile.Release` then sees
    /// `not collided` and calls `Expire()` and `ReleaseAll()`: the warhead
    /// vanishes in flight (`michelle.RemoveBall:1273-1281`,
    /// `missile.py:245-248`) **[V]**. A kill with no wreck and no capsule is
    /// exactly that one entry, sent to the clients holding the ball.
    ///
    /// A missile that **hit** goes with a plain [`BallsRemoved`] instead, so
    /// its warhead finishes the explosion.
    pub fn fizzle(to: Vec<i64>, stamp: u32, missile: i64) -> Option<Self> {
        Kill::new(to, stamp, missile, None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::super::ego_ball_in;
    use super::super::tests::parked;
    use super::super::warp::tests::EVERY_ACTION;
    use super::*;
    use crate::notify::Push;

    /// One of each stamped push, named exhaustively, with invented ids.
    fn every_stamped_push(built: u32, ball: Ball, slim: Slim) -> Vec<Box<dyn Push>> {
        vec![
            Box::new(
                ActionUpdate::new(vec![1], built, vec![Action::Stop { ball: 4242 }])
                    .expect("a batch"),
            ),
            Box::new(BallsRemoved::new(vec![1], built, vec![4242]).expect("a removal")),
            Box::new(
                ShipHandoff::new(
                    vec![1],
                    built,
                    vec![
                        HandoffEntry::Act(Action::Stop { ball: 4242 }),
                        HandoffEntry::Slim {
                            slim,
                            modules: Vec::new(),
                            extras: Vec::new(),
                        },
                    ],
                )
                .expect("a handoff"),
            ),
            Box::new(SpecialFx::new(vec![1], built, 4242, FX_WARPING, true).expect("an effect")),
            Box::new(
                DamageStateChange::new(vec![1], built, 4242, DamageState::FULL)
                    .expect("a damage update"),
            ),
            Box::new(
                SlimItemChange::new(
                    vec![1],
                    built,
                    WreckSlim {
                        item_id: 4242,
                        type_id: 26550,
                        owner_id: 1,
                        corp_id: 2,
                        launcher_id: 4243,
                        is_empty: true,
                        fleet_id: None,
                        abandoned: false,
                    },
                )
                .expect("a slim change"),
            ),
            Box::new(
                Kill::new(
                    vec![1],
                    built,
                    4243,
                    Some((
                        wreck_ball(4242, [0.0; 3]),
                        WreckSlim {
                            item_id: 4242,
                            type_id: 26550,
                            owner_id: 1,
                            corp_id: 2,
                            launcher_id: 4243,
                            is_empty: true,
                            fleet_id: None,
                            abandoned: false,
                        },
                    )),
                    Some((
                        Ball {
                            ball_id: 4244,
                            ..ball
                        },
                        Slim {
                            item_id: 4244,
                            ..slim
                        },
                        DamageState::FULL,
                    )),
                )
                .expect("a kill"),
            ),
            Box::new(
                BallsAdded::new(
                    vec![1],
                    built,
                    Park {
                        current_time: built,
                        balls: vec![ball],
                    },
                    vec![(slim, None)],
                )
                .expect("an addition"),
            ),
            Box::new(launched(built)),
            Box::new(StateUpdate {
                to: vec![1],
                state: Bootstrap::new(
                    built,
                    Park {
                        current_time: built,
                        balls: vec![ball],
                    },
                    4242,
                    vec![slim],
                    Vec::new(),
                ),
            }),
        ]
    }

    /// **Every push that carries a Destiny stamp can be restamped, and the
    /// ones with two clocks move both.**
    ///
    /// A scheduled batch is stamped when it *leaves*, not when it is built
    /// (`crate::notify::Push::restamp`), which is what lets this server act
    /// on its own clock at all. A stamped push that forgets to override
    /// `restamp` keeps the stamp it was constructed with -- so the first lane
    /// to schedule, say, a delayed `AddBalls2` for a timed NPC spawn ships a
    /// batch dated however long the delay was in the past. The client either
    /// logs `Michelle discarded a state that was too old` or tries to rewind
    /// and raises `OnRecoverableDesync`, after which it ignores every later
    /// batch. Silent on our side, terminal on theirs.
    ///
    /// Two of the five carry **two** clocks: `BallsAdded` and `StateUpdate`
    /// write the stamp into the blob header too, because the entry's stamp
    /// decides when the client flushes it and the blob's header decides what
    /// the client's clock becomes when it does. So the obvious one-line
    /// `restamp` is a *half* fix for those, and this asserts the blob as well
    /// as the envelope -- a half fix passes an envelope-only test.
    ///
    /// Exhaustive by construction: the `match` below must name every variant
    /// of the stamped set, so adding a sixth stamped push without a
    /// `restamp` stops this compiling.
    #[test]
    fn every_stamped_push_can_be_restamped() {
        use crate::notify::Push;
        const BUILT: u32 = 1_000;
        const SENT: u32 = 2_000;

        let ball = Ball {
            ball_id: 4242,
            mode: Mode::Stop,
            radius: 30.0,
            position: [1.0, 2.0, 3.0],
            flags: 0,
            mass: 1000.0,
            cloak: 0,
            harmonic: 0,
            corporation_id: -1,
            alliance_id: -1,
            physics: None,
            formation_id: NO_FORMATION,
        };
        let slim = Slim {
            item_id: 4242,
            type_id: 670,
            owner_id: 1,
            char_id: None,
            corp_id: None,
            response_thresholds: None,
        };

        // One of each stamped push, named exhaustively.
        let mut pushes = every_stamped_push(BUILT, ball, slim);

        for push in &mut pushes {
            let (name, _) = push.to_push();
            push.restamp(SENT);
            let (_, args) = push.to_push();
            let PyValue::List(entries) = &args[0] else {
                panic!("{name}: the batch is a sequence: {args:?}")
            };
            for entry in entries {
                let PyValue::Tuple(e) = entry else {
                    panic!("{name}: an entry is a tuple")
                };
                assert_eq!(
                    e[0],
                    PyValue::Int(i64::from(SENT)),
                    "{name} kept the stamp it was built with; a scheduled one \
                     would go out dated in the past"
                );
            }
        }

        // And the two that also carry a blob clock. Read back out of the
        // encoded bytes, because that is where a half fix would show.
        let mut added = BallsAdded::new(
            vec![1],
            BUILT,
            Park {
                current_time: BUILT,
                balls: vec![ball],
            },
            vec![(slim, None)],
        )
        .expect("an addition");
        added.restamp(SENT);
        assert_eq!(
            added.park.current_time, SENT,
            "AddBalls2's blob header must move with its stamp, or the park \
             starts out of step with every action that follows it"
        );
        let mut state = StateUpdate {
            to: vec![1],
            state: Bootstrap::new(
                BUILT,
                Park {
                    current_time: BUILT,
                    balls: vec![ball],
                },
                4242,
                vec![slim],
                Vec::new(),
            ),
        };
        state.restamp(SENT);
        assert_eq!(state.state.stamp, SENT);
        assert_eq!(
            state.state.park.current_time, SENT,
            "SetState's blob header must move with its stamp"
        );
    }

    /// An entry is a pair of a stamp and a pair.
    ///
    /// `Park.RealFlushState` does `UNPACK_SEQUENCE 2` twice --
    /// `eventStamp, event = state[0]` then `funcName, args = event`
    /// (`michelle.py:1515-1516`) **[V]** -- so both levels must have exactly
    /// two elements. Pins the **arity of each level**; asserting the whole
    /// `PyValue` tree against a literal we wrote ourselves would pass for ever
    /// and say nothing about why the shape is that shape.
    #[test]
    fn an_action_entry_is_a_pair_of_a_stamp_and_a_pair() {
        let update = ActionUpdate::new(vec![7], 1_700_000_000, vec![EVERY_ACTION[0]])
            .expect("one finite action is a batch");
        let (event, args) = crate::notify::Push::to_push(&update);
        assert_eq!(event, "DoDestinyUpdate");
        assert_eq!(args.len(), 2, "state and waitForBubble, and no third");
        assert_eq!(
            args[1],
            PyValue::Int(0),
            "a waiting entry at the head of history is never flushed"
        );
        let PyValue::List(entries) = &args[0] else {
            panic!("the batch is a sequence: {args:?}")
        };
        let PyValue::Tuple(entry) = &entries[0] else {
            panic!("an entry is a tuple: {entries:?}")
        };
        assert_eq!(entry.len(), 2, "(stamp, event)");
        assert_eq!(entry[0], PyValue::Int(1_700_000_000));
        let PyValue::Tuple(call) = &entry[1] else {
            panic!("the event is a tuple: {entry:?}")
        };
        assert_eq!(call.len(), 2, "(funcName, args)");
        assert!(
            matches!(call[0], PyValue::Bytes(_)),
            "the name is a Python 2 str, because the client does \
             `'_parent_' + funcName`: {call:?}"
        );
        assert!(
            matches!(call[1], PyValue::Tuple(_)),
            "args is one tuple, splatted by CALL_FUNCTION_VAR: {call:?}"
        );
    }

    /// A batch carries one distinct stamp, whatever it holds.
    ///
    /// `michelle.py:659-666`: more than one logs `Found update batch with N
    /// items and M timestamps`, calls `OnFatalDesync` -- **and flushes the
    /// batch anyway** **[V]**. The stamp is a field of the batch rather than
    /// of an action so that this cannot be got wrong, and the test asserts the
    /// property rather than a particular number.
    #[test]
    fn a_batch_has_one_distinct_stamp() {
        let update = ActionUpdate::new(vec![7], 4242, EVERY_ACTION.to_vec()).expect("a batch");
        let (_, args) = crate::notify::Push::to_push(&update);
        let PyValue::List(entries) = &args[0] else {
            panic!("the batch is a sequence")
        };
        assert_eq!(entries.len(), EVERY_ACTION.len());
        let stamps: std::collections::BTreeSet<i64> = entries
            .iter()
            .map(|e| {
                let PyValue::Tuple(e) = e else {
                    panic!("an entry is a tuple")
                };
                let PyValue::Int(stamp) = e[0] else {
                    panic!("a stamp is an int")
                };
                stamp
            })
            .collect();
        assert_eq!(stamps.len(), 1, "one stamp, or OnFatalDesync: {stamps:?}");
    }

    /// A batch the client would discard is not sent at all.
    ///
    /// Two of them, and the second is the dangerous one: an empty batch merely
    /// logs `Empty state received from remote ballpark`, while a non-finite
    /// float is discarded by `_destiny.dll` with **no exception, no log and no
    /// trace on either side** (`NaN in GotoDirection. Ignored`) **[V]**.
    ///
    /// Pins "nothing is sent", not "the NaN is replaced": clamping a NaN to
    /// zero would fly the ship somewhere nobody asked for.
    #[test]
    fn a_batch_the_client_would_discard_is_never_built() {
        assert!(
            ActionUpdate::new(vec![7], 1, vec![]).is_none(),
            "an empty batch is dropped by the client with one log line"
        );
        let nan = Action::GotoDirection {
            ball: 1,
            direction: [f64::NAN, 0.0, 0.0],
        };
        assert!(!nan.is_finite());
        assert!(
            ActionUpdate::new(vec![7], 1, vec![EVERY_ACTION[0], nan]).is_none(),
            "one non-finite float refuses the whole batch, because the native \
             discards it in silence"
        );
        let infinite = Action::SetSpeedFraction {
            ball: 1,
            fraction: f64::INFINITY,
        };
        assert!(ActionUpdate::new(vec![7], 1, vec![infinite]).is_none());
        assert!(
            EVERY_ACTION.iter().all(|a| a.is_finite()),
            "and the ordinary ones are all fine"
        );
    }

    /// A batch is addressed to the characters it names.
    ///
    /// The same contract [`StateUpdate`] has: `dispatch::pending_pushes` reads
    /// `audience()` and routes, and a push with no audience goes to the acting
    /// session -- which is the bug `docs/lanes/fleet.md` §4 found.
    #[test]
    fn an_action_batch_is_addressed_to_its_characters() {
        let update = ActionUpdate::new(vec![98, 99], 1, vec![EVERY_ACTION[3]]).expect("a batch");
        assert_eq!(
            crate::notify::Push::audience(&update),
            Some(vec![98, 99]),
            "a ship's motion is news to everyone watching it, not just its pilot"
        );
        assert_eq!(update.stamp(), 1);
        assert_eq!(update.actions(), &[EVERY_ACTION[3]]);
    }

    /// `OnDamageStateChange` is **one destiny entry** `(stamp,
    /// ('OnDamageStateChange', (shipID, state)))` in a `DoDestinyUpdate`,
    /// with the two-argument signature of `michelle.py:2070` **[V]** and the
    /// same per-ball state `SetState` carries.
    #[test]
    fn a_damage_update_is_one_destiny_entry_naming_the_ship_then_its_state() {
        use crate::notify::Push;
        let state = DamageState {
            shield: Some(0.5),
            shield_tau_ms: 125_000.0,
            armour: 0.25,
            hull: 1.0,
        };
        let p = DamageStateChange::new(vec![7, 8], 1234, 4242, state).expect("an update");
        let (name, args) = p.to_push();
        assert_eq!(name, "DoDestinyUpdate");
        assert_eq!(args.len(), 2, "no dogmaMessages third argument");
        assert_eq!(args[1], PyValue::Int(0), "waitForBubble");
        let PyValue::List(entries) = &args[0] else {
            panic!("the batch is a list")
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0],
            PyValue::Tuple(vec![
                PyValue::Int(1234),
                PyValue::Tuple(vec![
                    crate::pyutil::bstr("OnDamageStateChange"),
                    PyValue::Tuple(vec![PyValue::Int(4242), state.to_pyvalue()]),
                ]),
            ])
        );
        assert_eq!(p.audience(), Some(vec![7, 8]));
        assert!(DamageStateChange::new(vec![], 1, 1, state).is_none());
    }

    /// **A kill is one batch at one stamp: the wreck, the terminal removal,
    /// then the capsule** (`docs/re/damage-death.md` §0 steps 8-10, §8.7)
    /// **[V]**.
    ///
    /// Pins the things the client is strict about: one distinct stamp
    /// (`DoDestinyUpdate:660`), the wreck ahead of the removal so its
    /// `launcherID` still resolves (`Wreck.Prepare:74`), `RemoveBall`'s two
    /// arguments `(ballID, terminal)` (`michelle.py:1762`), the wreck's
    /// three slim keys with `lootRights` a 4-tuple (§6), and the capsule's
    /// extra as a real `(dict, damageState)` tuple (`type(data) is tuple`,
    /// `AddBalls2`). Plus every blob clock, which must equal the stamp.
    #[test]
    fn a_kill_is_one_batch_wreck_then_removal_then_capsule() {
        use crate::notify::Push;
        let slim = WreckSlim {
            item_id: 9001,
            type_id: 26550,
            owner_id: 77,
            corp_id: 88,
            launcher_id: 4242,
            is_empty: true,
            fleet_id: None,
            abandoned: false,
        };
        let pod = Slim {
            item_id: 9002,
            type_id: crate::damage::CAPSULE_TYPE_ID,
            owner_id: 77,
            char_id: Some(77),
            corp_id: Some(88),
            response_thresholds: None,
        };
        let pod_ball = ego_ball_in(
            &crate::content::Content::fixture(),
            9002,
            pod.type_id,
            88,
            -1,
            &parked(),
        );
        let mut kill = Kill::new(
            vec![1, 2],
            500,
            4242,
            Some((wreck_ball(9001, [1.0, 2.0, 3.0]), slim)),
            Some((pod_ball, pod, DamageState::FULL)),
        )
        .expect("a kill");
        kill.restamp(600);
        let (_, args) = kill.to_push();
        let PyValue::List(entries) = &args[0] else {
            panic!("a list")
        };
        assert_eq!(
            entries.len(),
            3,
            "the wreck, the removal and the capsule, together"
        );
        let mut stamps = Vec::new();
        let mut names = Vec::new();
        for e in entries {
            let PyValue::Tuple(e) = e else {
                panic!("(stamp, call)")
            };
            stamps.push(e[0].clone());
            let PyValue::Tuple(call) = &e[1] else {
                panic!("(name, args)")
            };
            names.push(call[0].clone());
        }
        assert!(stamps.iter().all(|s| *s == PyValue::Int(600)), "one stamp");
        assert_eq!(
            names,
            vec![
                crate::pyutil::bstr("AddBalls2"),
                crate::pyutil::bstr("RemoveBall"),
                crate::pyutil::bstr("AddBalls2")
            ],
            "the wreck must land while the dead ship's ball still exists"
        );
        let PyValue::Tuple(remove) = &entries[1] else {
            unreachable!()
        };
        let PyValue::Tuple(call) = &remove[1] else {
            unreachable!()
        };
        assert_eq!(
            call[1],
            PyValue::Tuple(vec![PyValue::Int(4242), PyValue::Bool(true)]),
            "RemoveBall(ballID, terminal=True)"
        );
        let PyValue::Tuple(add) = &entries[0] else {
            unreachable!()
        };
        let PyValue::Tuple(call) = &add[1] else {
            unreachable!()
        };
        let PyValue::Tuple(one) = &call[1] else {
            panic!("one positional")
        };
        assert_eq!(one.len(), 1, "AddBalls2 takes one argument, a 2-tuple");
        let PyValue::Tuple(chunk) = &one[0] else {
            panic!("(blob, extra)")
        };
        let PyValue::Bytes(blob) = &chunk[0] else {
            panic!("the blob")
        };
        let park = Park::decode(blob).expect("our own blob decodes");
        assert_eq!(
            park.current_time, 600,
            "the blob clock moved with the stamp"
        );
        assert_eq!(park.balls.len(), 1);
        assert_eq!(park.balls[0].ball_id, 9001);
        let PyValue::List(extra) = &chunk[1] else {
            panic!("extra")
        };
        let get = |k: &str| crate::pyutil::dict_get(&extra[0], k.as_bytes()).cloned();
        assert_eq!(get("itemID"), Some(PyValue::Int(9001)), "names the ball");
        assert_eq!(get("launcherID"), Some(PyValue::Int(4242)));
        assert!(
            matches!(get("isEmpty"), Some(PyValue::Bool(_))),
            "isEmpty is present; its value is ours to choose"
        );

        let Some(PyValue::Tuple(rights)) = get("lootRights") else {
            panic!("lootRights is a tuple")
        };
        assert_eq!(rights.len(), 4, "(ownerID, corpID, fleetID, abandoned)");
        assert_eq!(get("charID"), None, "a wreck has no pilot to prime");

        // The capsule: its own blob at the same clock, and the pair form.
        let PyValue::Tuple(add) = &entries[2] else {
            unreachable!()
        };
        let PyValue::Tuple(call) = &add[1] else {
            unreachable!()
        };
        let PyValue::Tuple(one) = &call[1] else {
            panic!("one positional")
        };
        let PyValue::Tuple(chunk) = &one[0] else {
            panic!("(blob, extra)")
        };
        let PyValue::Bytes(blob) = &chunk[0] else {
            panic!("the blob")
        };
        let park = Park::decode(blob).expect("our own blob decodes");
        assert_eq!(park.current_time, 600, "the capsule's clock moved too");
        assert_eq!(park.balls.len(), 1);
        assert_eq!(park.balls[0].ball_id, 9002);
        let PyValue::List(extra) = &chunk[1] else {
            panic!("extra")
        };
        let PyValue::Tuple(pair) = &extra[0] else {
            panic!("a ship's extra is a real tuple, or its gauges are hidden")
        };
        assert_eq!(pair.len(), 2, "(slim dict, damageState)");
        let get = |k: &str| crate::pyutil::dict_get(&pair[0], k.as_bytes()).cloned();
        assert_eq!(get("itemID"), Some(PyValue::Int(9002)), "names the ball");
        assert_eq!(get("charID"), Some(PyValue::Int(77)), "the pilot");
        assert_eq!(pair[1], DamageState::FULL.to_pyvalue());
    }

    /// **A wreck's changed slim is `OnSlimItemChange(itemID, newSlim)` with
    /// the slim as a `foo.SlimItem` instance** (`michelle.py:2006` stores it
    /// into `slimItems` unwrapped) and `isEmpty` as given **[V]**.
    #[test]
    fn a_slim_item_change_carries_the_instance_form_with_is_empty() {
        use crate::notify::Push;
        let slim = WreckSlim {
            item_id: 4242,
            type_id: 26550,
            owner_id: 1,
            corp_id: 2,
            launcher_id: 4243,
            is_empty: true,
            fleet_id: None,
            abandoned: false,
        };
        assert!(SlimItemChange::new(Vec::new(), 7, slim).is_none());
        let p = SlimItemChange::new(vec![1, 2], 7, slim).expect("a change");
        assert_eq!(p.audience(), Some(vec![1, 2]));
        let (name, args) = p.to_push();
        assert_eq!(name, "DoDestinyUpdate");
        let PyValue::List(entries) = &args[0] else {
            panic!("a batch")
        };
        assert_eq!(entries.len(), 1);
        let PyValue::Tuple(e) = &entries[0] else {
            panic!("(stamp, call)")
        };
        assert_eq!(e[0], PyValue::Int(7));
        let PyValue::Tuple(call) = &e[1] else {
            panic!("(name, args)")
        };
        assert_eq!(call[0], crate::pyutil::bstr("OnSlimItemChange"));
        let PyValue::Tuple(a) = &call[1] else {
            panic!("args")
        };
        assert_eq!(a.len(), 2, "(itemID, newSlim)");
        assert_eq!(a[0], PyValue::Int(4242));
        let PyValue::Instance { class, state } = &a[1] else {
            panic!("a SlimItem instance, not a dict: {:?}", a[1])
        };
        assert_eq!(class, b"foo.SlimItem");
        assert_eq!(
            crate::pyutil::dict_get(state, b"isEmpty"),
            Some(&PyValue::Bool(true))
        );
    }

    // ---- M2: the missile push. Invented ids and numbers throughout. ----

    const MISSILE: i64 = 9_500_000_000_001;
    const LAUNCHER: i64 = 100_045_001;
    const TARGET: i64 = 100_045_002;
    const CHARGE: i64 = 990_460_002;
    const PILOT: i64 = 90_045_001;
    const LAUNCHER_ITEM: i64 = 100_045_101;

    fn missile_from(owner: i64, target: i64) -> MissileBall {
        MissileBall {
            id: MISSILE,
            owner,
            target,
            from: [1_000.0, 0.0, 0.0],
            toward: [1_000.0, 30_000.0, 0.0],
            radius: 25.0,
            target_radius: 40.0,
            mass: 500.0,
            max_velocity: 3_000.0,
            agility: 0.001,
            corporation_id: 1_000_001,
            alliance_id: -1,
        }
    }

    fn player_slim(source: i64) -> MissileSlim {
        MissileSlim {
            item_id: MISSILE,
            type_id: CHARGE,
            owner_id: PILOT,
            source_ship_id: source,
            launch_modules: vec![LAUNCHER_ITEM],
        }
    }

    fn launched(stamp: u32) -> MissileLaunched {
        MissileLaunched::new(
            vec![1],
            stamp,
            missile_from(LAUNCHER, TARGET),
            player_slim(LAUNCHER),
        )
        .expect("a launch")
    }

    /// `(stamp, name, args)` of every entry.
    fn stamped(p: &dyn crate::notify::Push) -> Vec<(PyValue, String, Vec<PyValue>)> {
        let (event, args) = p.to_push();
        assert_eq!(event, "DoDestinyUpdate");
        let PyValue::List(es) = &args[0] else {
            panic!("a batch is a list")
        };
        es.iter()
            .map(|e| {
                let PyValue::Tuple(e) = e else {
                    panic!("an entry")
                };
                let PyValue::Tuple(call) = &e[1] else {
                    panic!("(name, args)")
                };
                let (PyValue::Bytes(n), PyValue::Tuple(a)) = (&call[0], &call[1]) else {
                    panic!("(str, tuple)")
                };
                (
                    e[0].clone(),
                    String::from_utf8_lossy(n).into_owned(),
                    a.clone(),
                )
            })
            .collect()
    }

    /// The park and slim list an `AddBalls2` entry carries.
    fn added(args: &[PyValue]) -> (Park, Vec<PyValue>) {
        let [PyValue::Tuple(pair)] = args else {
            panic!("AddBalls2 takes one positional, a pair: {args:?}")
        };
        let (PyValue::Bytes(blob), PyValue::List(slims)) = (&pair[0], &pair[1]) else {
            panic!("(blob, [slim])")
        };
        (Park::decode(blob).expect("our blob decodes"), slims.clone())
    }

    /// **The add, then the launch, at one stamp**: the launch must name a
    /// ball the park already holds, and a batch with two stamps is a fatal
    /// desync (`michelle.py:659-666`).
    #[test]
    fn a_launch_is_the_add_then_the_launch_at_one_stamp() {
        let e = stamped(&launched(1_234));
        let names: Vec<&str> = e.iter().map(|(_, n, _)| n.as_str()).collect();
        assert_eq!(names, ["AddBalls2", "LaunchMissile"]);
        assert!(e.iter().all(|(s, _, _)| *s == PyValue::Int(1_234)));
        assert_eq!(
            e[1].2,
            vec![
                PyValue::Int(MISSILE),
                PyValue::Int(TARGET),
                PyValue::Int(LAUNCHER),
                PyValue::Bool(true),
                PyValue::Bool(true),
            ],
            "(srcID, dstID, ownerID, aimed, massive), the \"LLLbb\" order"
        );
        let (park, slims) = added(&e[0].2);
        assert_eq!(park.balls.len(), 1);
        assert_eq!(slims.len(), 1);
        let ball = park.balls[0];
        assert_eq!(ball.ball_id, MISSILE);
        assert_eq!(ball.mode.code(), 5, "DSTBALL_MISSILE");
        let Mode::Missile {
            follow_id,
            follow_range,
            owner_id,
            goto,
            ..
        } = ball.mode
        else {
            panic!("a missile tail: {:?}", ball.mode)
        };
        assert_eq!((follow_id, owner_id), (TARGET, LAUNCHER));
        assert_eq!(follow_range, -65.0, "-(missile radius + target radius)");
        assert_eq!(goto[0], 1_000.0, "straight along +y from the launcher");
        assert!(goto[1] > 1.0e15);
        assert_eq!(ball.position, [1_000.0, 0.0, 0.0]);
    }

    /// **`restamp` moves all three clocks**: the entries, the blob header and
    /// the tail's `effectStamp`. A half fix leaves the straight-flight window
    /// on the wrong tick.
    #[test]
    fn a_restamped_launch_agrees_in_all_three_places() {
        use crate::notify::Push;
        let mut p = launched(1_000);
        p.restamp(2_000);
        let e = stamped(&p);
        assert!(e.iter().all(|(s, _, _)| *s == PyValue::Int(2_000)));
        let (park, _) = added(&e[0].2);
        assert_eq!(park.current_time, 2_000, "the blob header");
        let Mode::Missile { effect_stamp, .. } = park.balls[0].mode else {
            panic!("a missile tail")
        };
        assert_eq!(effect_stamp, 2_000, "the tail");
    }

    /// The slim's keys, and that it is a bare dict: a missile has no gauges.
    #[test]
    fn a_missile_slim_carries_the_charge_the_source_ship_and_its_launcher() {
        let e = stamped(&launched(1));
        let (_, slims) = added(&e[0].2);
        let slim = &slims[0];
        assert!(matches!(slim, PyValue::Dict(_)), "a bare dict: {slim:?}");
        let get = |k: &[u8]| crate::pyutil::dict_get(slim, k).cloned();
        assert_eq!(get(b"itemID"), Some(PyValue::Int(MISSILE)));
        assert_eq!(get(b"typeID"), Some(PyValue::Int(CHARGE)), "the charge");
        assert_eq!(get(b"ownerID"), Some(PyValue::Int(PILOT)));
        assert_eq!(get(b"sourceShipID"), Some(PyValue::Int(LAUNCHER)));
        assert_eq!(
            get(b"launchModules"),
            Some(PyValue::List(vec![PyValue::Int(LAUNCHER_ITEM)]))
        );
    }

    /// An NPC's launcher is its `modules[0]` **[V]**.
    #[test]
    fn an_npc_missile_launches_from_module_zero() {
        let slim = MissileSlim::npc(MISSILE, CHARGE, 1_000_125, LAUNCHER);
        assert_eq!(
            crate::pyutil::dict_get(&slim.to_dict(), b"launchModules"),
            Some(&PyValue::List(vec![PyValue::Int(0)]))
        );
        assert_eq!(slim.source_ship_id, LAUNCHER);
    }

    /// The ids `MissileFollow` cancels as inconsistent, a slim for another
    /// ball, and a launch with no direction are refused here.
    #[test]
    fn a_launch_the_client_would_cancel_is_refused() {
        let new = |m: MissileBall, s: MissileSlim| MissileLaunched::new(vec![1], 1, m, s);
        assert!(new(missile_from(LAUNCHER, LAUNCHER), player_slim(LAUNCHER)).is_none());
        assert!(new(missile_from(MISSILE, TARGET), player_slim(MISSILE)).is_none());
        assert!(new(missile_from(LAUNCHER, MISSILE), player_slim(LAUNCHER)).is_none());
        let mut other = player_slim(LAUNCHER);
        other.item_id += 1;
        assert!(new(missile_from(LAUNCHER, TARGET), other).is_none());
        let mut on_top = missile_from(LAUNCHER, TARGET);
        on_top.toward = on_top.from;
        assert!(new(on_top, player_slim(LAUNCHER)).is_none());
        assert!(
            MissileLaunched::new(
                Vec::new(),
                1,
                missile_from(LAUNCHER, TARGET),
                player_slim(LAUNCHER)
            )
            .is_none()
        );
    }

    /// A miss is the singular `RemoveBall(id, True)` and nothing else; a hit
    /// is the plain one-argument `RemoveBalls`.
    #[test]
    fn a_miss_fizzles_and_a_hit_is_a_plain_removal() {
        let miss = Kill::fizzle(vec![1], 7, MISSILE).expect("a fizzle");
        let e = stamped(&miss);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].1, "RemoveBall");
        assert_eq!(e[0].2, vec![PyValue::Int(MISSILE), PyValue::Bool(true)]);
        let hit = BallsRemoved::new(vec![1], 7, vec![MISSILE]).expect("a removal");
        let e = stamped(&hit);
        assert_eq!(e[0].1, "RemoveBalls");
        assert_eq!(e[0].2, vec![PyValue::List(vec![PyValue::Int(MISSILE)])]);
    }

    /// **The audience is the clients holding both ends, and only they hold
    /// the missile afterwards**: Bob holds Alice but not Carol, so a missile
    /// from Alice at Carol reaches nobody; one from Alice at Bob reaches
    /// Alice and Bob, joins their parks, and both of its removals reach
    /// exactly them.
    #[test]
    fn a_missile_reaches_only_the_parks_holding_both_ends() {
        use crate::grid::tests::*;
        use std::time::Duration;
        let mut reg = world();
        for who in [ALICE, BOB, CAROL] {
            undock(&mut reg, who);
        }
        put(&mut reg, CAROL, BELT_A_AT);
        let _ = settle(&mut reg);
        assert!(held(&reg, BOB).contains(&ALICE_SHIP));
        assert!(!held(&reg, BOB).contains(&CAROL_SHIP));
        assert!(!held(&reg, ALICE).contains(&CAROL_SHIP));
        let everyone = vec![ALICE, BOB, CAROL];
        let launch = |owner: i64, target: i64| {
            let mut m = missile_from(owner, target);
            m.toward = [m.from[0], m.from[1] + 1.0, m.from[2]];
            MissileLaunched::new(everyone.clone(), 1, m, player_slim(owner)).expect("a launch")
        };

        reg.schedule(Duration::ZERO, launch(ALICE_SHIP, CAROL_SHIP));
        let out = drain(&mut reg);
        for who in [ALICE, BOB, CAROL] {
            assert!(
                sent_to(&out, who).is_empty(),
                "{who} lacks one end and must get nothing: {out:?}"
            );
            assert!(!held(&reg, who).contains(&MISSILE));
        }

        reg.schedule(Duration::ZERO, launch(ALICE_SHIP, BOB_SHIP));
        let out = drain(&mut reg);
        for who in [ALICE, BOB] {
            let names: Vec<String> = sent_to(&out, who).into_iter().map(|(n, _)| n).collect();
            assert_eq!(names, ["AddBalls2", "LaunchMissile"], "{who}, whole");
            assert!(held(&reg, who).contains(&MISSILE));
        }
        assert!(sent_to(&out, CAROL).is_empty());
        assert!(!held(&reg, CAROL).contains(&MISSILE));

        // The reconcile leaves the missile alone: it is no local ball.
        let out = settle(&mut reg);
        assert!(!removed_from(&out, ALICE).contains(&MISSILE));
        assert!(held(&reg, ALICE).contains(&MISSILE));

        // A miss reaches exactly the holders, and takes it out of their parks.
        reg.schedule(
            Duration::ZERO,
            Kill::fizzle(everyone.clone(), 0, MISSILE).expect("a fizzle"),
        );
        let out = drain(&mut reg);
        let reached: Vec<i64> = [ALICE, BOB, CAROL]
            .into_iter()
            .filter(|w| !sent_to(&out, *w).is_empty())
            .collect();
        assert_eq!(reached, [ALICE, BOB]);
        assert!(!held(&reg, ALICE).contains(&MISSILE));
        assert!(!held(&reg, BOB).contains(&MISSILE));

        // And a hit, on a second launch, the same.
        reg.schedule(Duration::ZERO, launch(ALICE_SHIP, BOB_SHIP));
        let _ = drain(&mut reg);
        reg.schedule(
            Duration::ZERO,
            BallsRemoved::new(everyone.clone(), 0, vec![MISSILE]).expect("a removal"),
        );
        let out = drain(&mut reg);
        assert!(removed_from(&out, ALICE).contains(&MISSILE));
        assert!(removed_from(&out, BOB).contains(&MISSILE));
        assert!(sent_to(&out, CAROL).is_empty());
        assert!(!held(&reg, BOB).contains(&MISSILE));
    }

    /// PA-10 §9 T1: every Destiny push type through the coalescing
    /// classifier. `StateUpdate` (a `SetState`) is a frame, a barrier; every
    /// other type is a part, with its one stamp.
    #[test]
    fn the_classifier_takes_every_destiny_push_but_the_set_state() {
        let ball = Ball {
            ball_id: 4242,
            mode: Mode::Stop,
            radius: 30.0,
            position: [1.0, 2.0, 3.0],
            flags: 0,
            mass: 1000.0,
            cloak: 0,
            harmonic: 0,
            corporation_id: -1,
            alliance_id: -1,
            physics: None,
            formation_id: NO_FORMATION,
        };
        let slim = Slim {
            item_id: 4242,
            type_id: 670,
            owner_id: 1,
            char_id: None,
            corp_id: None,
            response_thresholds: None,
        };
        let pushes = every_stamped_push(1_000, ball, slim);
        assert!(pushes.len() >= 9, "the whole set");
        for push in &pushes {
            let (event, args) = push.to_push();
            let is_set_state = matches!(
                &args[0],
                PyValue::List(e) if matches!(
                    &e[0],
                    PyValue::Tuple(t) if matches!(&t[1], PyValue::Tuple(n) if n[0] == PyValue::Bytes(b"SetState".to_vec()))
                )
            );
            let got = crate::push::destiny_part(event, &args);
            if is_set_state {
                assert!(got.is_none(), "a SetState is a barrier");
            } else {
                let (stamp, n, _) = got.unwrap_or_else(|| panic!("{args:?} is a part"));
                assert_eq!(stamp, 1_000);
                assert!(n >= 1);
            }
        }
        assert_eq!(
            pushes
                .iter()
                .filter(|p| crate::push::destiny_part(p.to_push().0, &p.to_push().1).is_none())
                .count(),
            1,
            "exactly the StateUpdate is a frame"
        );
    }
}
