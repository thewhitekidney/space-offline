//! **The dynamic bounty system** (W15 packet CP38, `docs/content/C5-economy.md`
//! §5.3 and E5; lane note `docs/lanes/cp38.md`).
//!
//! A per-system multiplier `d` on every NPC bounty paid there. Ratting a
//! system drains it, and it recovers toward [`DEFAULT`] with time. The client
//! asks for it with `dynamicBountyMgr.GetOutputForClientSolarSystem()`, which
//! it unpacks as `output, dbsIsActive` and memoizes for 5,400 s **[V]**
//! (`dynamicResourceSvc.py:92`, `:104-106`), and it is told of a change by
//! `OnSystemDynamicBountyStateChanged(systemID, rate)`, which primes that
//! cache with `(rate, True)` **[V]** (`:201-208`).
//!
//! **What persists.** A `dbs` delta ([`KIND`]), `system_id → d touched_at`,
//! only while `d` is not [`DEFAULT`]. The value read is always the lazy one
//! ([`multiplier`]): the stored `d` moved toward [`DEFAULT`] by
//! [`RECOVERY_PER_HOUR`] for every hour since `touched_at`. Nothing ticks.
//! The rollover ([`on_rollover`]) drops the rows that have fully recovered
//! and tells each system whose value has climbed by a push step since it was
//! last told.
//!
//! **What does not persist.** What each system was last told ([`Told`], a
//! world slot): after a restart every client asks again anyway.

use crate::damage::BlueTime;
use crate::notify::{Addressed, Push};
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Duration;

/// The delta kind this module owns (`delta_kinds::KINDS`, plan §4.2):
/// `system_id → d touched_at` (`touched_at` blue FILETIME).
pub const KIND: &str = "dbs";

/// The multiplier of an untouched system: bounties paid as listed. CCP's own
/// "DBS offline" answer is this value too, `(1.0, False)` **[V]**
/// (`dynamicResourceSvc.py:104-106`).
pub const DEFAULT: f64 = 1.0;

/// `[A]` (C5 §5.3) the floor of `d`: a farmed-out system still pays a
/// quarter. Falsified by a system that is not worth ratting at the floor.
pub const D_MIN: f64 = 0.25;

/// `[A]` (C5 §5.3) the ceiling of `d`. Nothing in CP38 raises `d` above
/// [`DEFAULT`]; the bound is there for a later controller.
pub const D_MAX: f64 = 1.25;

/// `[A]` (C5 §7.3) the global multiplier. Phase 1 ships no controller, so it
/// is the identity. Falsified by a week of `[econ]` lines with a drift.
pub const DBS_GLOBAL: f64 = 1.0;

/// `[A]` (C5 §5.3) ISK paid out of a security-0.5 system that costs it 1.0
/// of `d`, divided by `(1.5 − 0.5)`: `POOL_s = POOL_BASE × (1.5 − sec)`.
/// Falsified by "my system's bounties collapsed after an hour": raise it.
pub const POOL_BASE: f64 = 200_000_000.0;

/// `[A]` (C5 §5.3) how far `d` moves back toward [`DEFAULT`] in an hour,
/// in multiplier units (linear: a floored system is whole again in under a
/// day, so by the next rollover). Falsified by a system that stays farmed
/// for days.
pub const RECOVERY_PER_HOUR: f64 = 0.04;

/// `[A]` (C5 §5.3) the change in `d` since a system was last told that
/// earns a push. Falsified by a pilot's info panel visibly out of step.
pub const DBS_PUSH_STEP: f64 = 0.05;

/// Blue time is 10,000 ticks to the millisecond.
const BLUE_PER_HOUR: f64 = 36_000_000_000.0;

/// The bounds hold together. Checked when the crate compiles.
const _: () = assert!(D_MIN > 0.0 && D_MIN < DEFAULT && DEFAULT < D_MAX);

/// The pool of `system`: ISK paid there that drains `d` by 1.0. A system the
/// universe table lacks is treated as security 1.0 `[A]`, as
/// `npc_gen::belt_squad_in` does: the smallest pool, so the sandbox errs on
/// the side of draining.
pub fn pool(reg: &Registry, system: i64) -> f64 {
    let sec = reg
        .content()
        .get::<crate::celestial::Universe>()
        .system(system)
        .map_or(1.0, |s| s.security)
        .clamp(-1.0, 1.0);
    POOL_BASE * (1.5 - sec)
}

/// A stored row, `(d, touched_at)`; `None` when unreadable.
fn read_row(value: &[String]) -> Option<(f64, BlueTime)> {
    let [d, at] = value else { return None };
    let d: f64 = d.parse().ok()?;
    d.is_finite().then_some(())?;
    Some((d.clamp(D_MIN, D_MAX), at.parse().ok()?))
}

/// `d` stored at `at`, recovered to `now`: moved toward [`DEFAULT`] by
/// [`RECOVERY_PER_HOUR`] an hour, never past it. A clock that runs backwards
/// recovers nothing.
pub fn recovered(d: f64, at: BlueTime, now: BlueTime) -> f64 {
    let hours = (now.saturating_sub(at)).max(0) as f64 / BLUE_PER_HOUR;
    let step = RECOVERY_PER_HOUR * hours;
    if d < DEFAULT {
        (d + step).min(DEFAULT)
    } else {
        (d - step).max(DEFAULT)
    }
}

/// **The multiplier of `system` at `now`**: its row recovered, or
/// [`DEFAULT`] with no row.
pub fn multiplier(reg: &Registry, system: i64, now: BlueTime) -> f64 {
    reg.deltas()
        .of(KIND)
        .get(&vec![system.to_string()])
        .and_then(|v| read_row(v))
        // Lane clock: the game hours since `at`, so a compressed day
        // recovers a day's worth. Derived from the stored `at`, so a restart
        // at another scale recovers the time since at the new pace.
        .map_or(DEFAULT, |(d, at)| {
            let game_now = at.saturating_add(crate::game_clock::of(reg).elapsed(now - at));
            recovered(d, at, game_now)
        })
}

/// The ISK a bounty of `base` pays in `system` at `now`:
/// `base × d × DBS_GLOBAL` (C5 §5.3), to the cent `[A]` (the wallet keeps
/// cents; falsified by a client that shows sub-cent amounts).
pub fn payout(reg: &Registry, system: i64, base: f64, now: BlueTime) -> f64 {
    cents(base * multiplier(reg, system, now) * DBS_GLOBAL)
}

fn cents(isk: f64) -> f64 {
    (isk * 100.0).round() / 100.0
}

/// **Drain `system` by `paid` ISK at `now`**: `d` falls by `paid / pool`,
/// to no lower than [`D_MIN`], and the row is written (or dropped, at
/// [`DEFAULT`]). When `d` has moved by [`DBS_PUSH_STEP`] or more since the
/// system was last told, the pilots in it are told, on the clock. Answers
/// the new `d`.
pub fn drain(reg: &mut Registry, system: i64, paid: f64, now: BlueTime) -> f64 {
    let before = multiplier(reg, system, now);
    let paid = if paid.is_finite() { paid.max(0.0) } else { 0.0 };
    let after = (before - paid / pool(reg, system)).clamp(D_MIN, D_MAX);
    write(reg, system, after, now);
    tell_if_moved(reg, system, after);
    after
}

/// Store `d` for `system` as of `now`; [`DEFAULT`] is no row.
fn write(reg: &mut Registry, system: i64, d: f64, now: BlueTime) {
    let key = vec![system.to_string()];
    if d == DEFAULT {
        reg.deltas_mut().del(KIND, &key);
    } else {
        // Rust's `{}` for f64 is the shortest string that parses back exactly.
        reg.deltas_mut()
            .put(KIND, key, vec![d.to_string(), now.to_string()]);
    }
}

/// **What each system was last told**, by system id; a system missing here
/// was told [`DEFAULT`] (or nothing, which the client reads as the same).
/// A world slot: not persisted.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Told(BTreeMap<i64, f64>);

impl Told {
    /// What `system` was last told.
    pub fn of(&self, system: i64) -> f64 {
        self.0.get(&system).copied().unwrap_or(DEFAULT)
    }
}

/// The [`Told`] world slot (`world_ext::SLOTS`). Its `make` reads no deltas.
pub const SLOT: SlotSpec = SlotSpec {
    name: "econ_dbs",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(Told::default())
}

/// Record that `system` now knows `d`: a client was answered it.
pub fn told(reg: &mut Registry, system: i64, d: f64) {
    if d == DEFAULT {
        reg.ext_mut::<Told>().0.remove(&system);
    } else {
        reg.ext_mut::<Told>().0.insert(system, d);
    }
}

/// Push `d` to the pilots in `system` when it is [`DBS_PUSH_STEP`] or more
/// from what the system was last told. Answers whether it pushed.
fn tell_if_moved(reg: &mut Registry, system: i64, d: f64) -> bool {
    if (d - reg.ext::<Told>().of(system)).abs() < DBS_PUSH_STEP {
        return false;
    }
    told(reg, system, d);
    let to = crate::dungeon::audience(reg, system);
    if !to.is_empty() {
        reg.schedule(
            Duration::ZERO,
            Addressed {
                to: Some(to),
                inner: Box::new(StateChanged { system, rate: d }),
            },
        );
    }
    true
}

/// `OnSystemDynamicBountyStateChanged(systemID, rate)` **[V]**
/// (`dynamicResourceSvc.py:201-208`).
#[derive(Debug, Clone, PartialEq)]
pub struct StateChanged {
    pub system: i64,
    pub rate: f64,
}

impl Push for StateChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnSystemDynamicBountyStateChanged",
            vec![PyValue::Int(self.system), PyValue::Real(self.rate)],
        )
    }
}

/// **The `GetOutputForClientSolarSystem` answer** for `system` at `now`:
/// `(d, True)` (C5 §5.3), and the system is recorded as told it.
pub fn answer(reg: &mut Registry, system: i64, now: BlueTime) -> PyValue {
    let d = multiplier(reg, system, now);
    told(reg, system, d);
    PyValue::Tuple(vec![PyValue::Real(d), PyValue::Bool(true)])
}

/// **The rollover step** (`rollover_steps::STEPS`): drop every row that has
/// recovered to [`DEFAULT`] by `now`, and push to each system that has
/// climbed [`DBS_PUSH_STEP`] or more since it was last told. Its count is
/// the rows dropped.
pub fn on_rollover(reg: &mut Registry, now: BlueTime) -> usize {
    let rows: Vec<(i64, f64)> = reg
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            let system: i64 = k.first()?.parse().ok()?;
            let (d, at) = read_row(v)?;
            Some((system, recovered(d, at, now)))
        })
        .collect();
    let mut dropped = 0;
    for &(system, d) in &rows {
        if d == DEFAULT {
            reg.deltas_mut().del(KIND, &[system.to_string()]);
            dropped += 1;
        }
    }
    let told: Vec<i64> = reg.ext::<Told>().0.keys().copied().collect();
    for system in told {
        let d = multiplier(reg, system, now);
        tell_if_moved(reg, system, d);
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYSTEM: i64 = 30_000_001;
    const OTHER: i64 = 30_000_002;
    const T0: BlueTime = 130_000_000_000_000_000;
    const HOUR: BlueTime = 36_000_000_000;

    const ALICE: i64 = 90_000_101;

    fn reg() -> Registry {
        crate::testkit::registry()
    }

    /// Alice undocked; answers the registry and her system.
    fn world() -> (Registry, i64) {
        let mut reg = crate::testkit::registry_with(|c| {
            c.characters = vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: 100_000_101,
                ship_type_id: 587,
                ..crate::model::Character::default()
            }];
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        let _ = pushes(&mut reg);
        let system = reg.roster().get(ALICE).expect("in space").solar_system_id;
        (reg, system)
    }

    fn pushes(reg: &mut Registry) -> Vec<(Vec<i64>, &'static str, Vec<PyValue>)> {
        reg.take_due(std::time::Instant::now() + Duration::from_secs(600))
            .into_iter()
            .map(|p| {
                let (n, a) = p.to_push();
                (p.audience().unwrap_or_default(), n, a)
            })
            .collect()
    }

    /// An untouched system pays the listed bounty and stores nothing.
    #[test]
    fn an_untouched_system_is_the_default_and_has_no_row() {
        let reg = reg();
        assert_eq!(multiplier(&reg, SYSTEM, T0), DEFAULT);
        assert_eq!(payout(&reg, SYSTEM, 1_000.0, T0), 1_000.0 * DBS_GLOBAL);
        assert!(reg.deltas().of(KIND).is_empty());
    }

    /// **The pool drains**: a payout lowers `d` by `paid / pool`, only in
    /// its own system, and never below `D_MIN`; the next payout is smaller.
    #[test]
    fn the_pool_drains_by_paid_over_pool_and_floors() {
        let mut reg = reg();
        let p = pool(&reg, SYSTEM);
        let d = drain(&mut reg, SYSTEM, p / 10.0, T0);
        assert!((d - (DEFAULT - 0.1)).abs() < 1e-12);
        assert_eq!(multiplier(&reg, SYSTEM, T0), d);
        assert_eq!(multiplier(&reg, OTHER, T0), DEFAULT);
        assert!(payout(&reg, SYSTEM, 1_000.0, T0) < payout(&reg, OTHER, 1_000.0, T0));
        assert_eq!(drain(&mut reg, SYSTEM, p * 100.0, T0), D_MIN);
        assert_eq!(drain(&mut reg, SYSTEM, p, T0), D_MIN, "the floor holds");
    }

    /// A lower security is a bigger pool: null drains slower than high.
    #[test]
    fn a_lower_security_system_has_the_bigger_pool() {
        use crate::celestial::Universe;
        let reg = reg();
        let u = reg.content().get::<Universe>();
        let mut secs: Vec<(i64, f64)> = u.systems().map(|s| (s.id, s.security)).collect();
        secs.sort_by(|a, b| a.1.total_cmp(&b.1));
        let (low, high) = (
            secs.first().expect("a system"),
            secs.last().expect("a system"),
        );
        assert!(low.1 < high.1, "the fixture spans securities");
        assert!(pool(&reg, low.0) > pool(&reg, high.0));
    }

    /// **The pool recovers lazily**: a read later sees `d` moved back by
    /// `RECOVERY_PER_HOUR` an hour, capped at the default, without any
    /// write; a drain after that starts from the recovered value.
    #[test]
    fn the_pool_recovers_lazily_on_read() {
        let mut reg = reg();
        let p = pool(&reg, SYSTEM);
        let d0 = drain(&mut reg, SYSTEM, p / 2.0, T0);
        let row = reg.deltas().of(KIND).clone();
        let d1 = multiplier(&reg, SYSTEM, T0 + HOUR);
        assert!((d1 - (d0 + RECOVERY_PER_HOUR)).abs() < 1e-12);
        assert_eq!(reg.deltas().of(KIND), &row, "a read writes nothing");
        assert_eq!(multiplier(&reg, SYSTEM, T0 + 1_000 * HOUR), DEFAULT);
        assert_eq!(
            multiplier(&reg, SYSTEM, T0 - HOUR),
            d0,
            "no recovery backwards"
        );
        let d2 = drain(&mut reg, SYSTEM, 0.0, T0 + HOUR);
        assert!((d2 - d1).abs() < 1e-12);
    }

    /// Lane clock: at scale N the pool recovers an hour's worth in an hour
    /// over N.
    #[test]
    fn at_scale_n_the_pool_recovers_n_times_faster() {
        let mut reg = reg();
        crate::game_clock::set(&mut reg, crate::game_clock::GameClock::new(5).unwrap());
        let p = pool(&reg, SYSTEM);
        let d0 = drain(&mut reg, SYSTEM, p / 2.0, T0);
        let d1 = multiplier(&reg, SYSTEM, T0 + HOUR / 5);
        assert!(
            (d1 - (d0 + RECOVERY_PER_HOUR)).abs() < 1e-12,
            "{d0} -> {d1}"
        );
        assert_eq!(
            multiplier(&reg, SYSTEM, T0 - HOUR),
            d0,
            "no recovery backwards"
        );
    }

    /// The rollover drops recovered rows and keeps the rest; a store
    /// round-trip of the row gives the same multiplier.
    #[test]
    fn the_rollover_drops_recovered_rows_only() {
        let mut reg = reg();
        let p = pool(&reg, SYSTEM);
        drain(&mut reg, SYSTEM, p / 2.0, T0);
        drain(&mut reg, OTHER, p / 2.0, T0 + 1_000 * HOUR);
        let later = T0 + 1_000 * HOUR;
        let before = multiplier(&reg, OTHER, later);
        assert_eq!(on_rollover(&mut reg, later), 1);
        assert_eq!(reg.deltas().of(KIND).len(), 1);
        assert_eq!(multiplier(&reg, SYSTEM, later), DEFAULT);
        let mut again = crate::testkit::registry();
        for (k, v) in reg.deltas().of(KIND).clone() {
            again.deltas_mut().put(KIND, k, v);
        }
        assert_eq!(multiplier(&again, OTHER, later), before);
    }

    /// **The push fires on a step of at least `DBS_PUSH_STEP`**, by name,
    /// `(systemID, rate)` to the pilots in the system; a smaller move is
    /// silent, and the step is measured from what was last told.
    #[test]
    fn the_push_fires_on_a_step_of_at_least_dbs_push_step() {
        let (mut reg, system) = world();
        let p = pool(&reg, system);
        let small = p * DBS_PUSH_STEP * 0.6;
        drain(&mut reg, system, small, T0);
        assert!(pushes(&mut reg).is_empty(), "under a step is silent");
        let d = drain(&mut reg, system, small, T0);
        let out = pushes(&mut reg);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].0, vec![ALICE]);
        assert_eq!(out[0].1, "OnSystemDynamicBountyStateChanged");
        assert_eq!(out[0].2, vec![PyValue::Int(system), PyValue::Real(d)]);
        assert_eq!(reg.ext::<Told>().of(system), d);
        drain(&mut reg, system, small, T0);
        assert!(pushes(&mut reg).is_empty(), "measured from the last push");
    }

    /// The rollover tells a system that has climbed back a push step.
    #[test]
    fn the_rollover_tells_a_recovered_system() {
        let (mut reg, system) = world();
        let p = pool(&reg, system);
        drain(&mut reg, system, p / 2.0, T0);
        let _ = pushes(&mut reg);
        on_rollover(&mut reg, T0 + 1_000 * HOUR);
        let out = pushes(&mut reg);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].0, vec![ALICE]);
        assert_eq!(out[0].2, vec![PyValue::Int(system), PyValue::Real(DEFAULT)]);
        assert_eq!(reg.ext::<Told>().of(system), DEFAULT);
    }

    /// **The answer is a `(float, bool)` tuple** [V], active, carrying the
    /// system's multiplier.
    #[test]
    fn the_answer_is_a_float_bool_tuple() {
        let mut reg = reg();
        let p = pool(&reg, SYSTEM);
        let d = drain(&mut reg, SYSTEM, p / 4.0, T0);
        assert_eq!(
            answer(&mut reg, SYSTEM, T0),
            PyValue::Tuple(vec![PyValue::Real(d), PyValue::Bool(true)])
        );
        assert_eq!(reg.ext::<Told>().of(SYSTEM), d);
    }
}
