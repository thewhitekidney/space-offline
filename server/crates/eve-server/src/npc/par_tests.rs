//! PA-8a: the planned NPC loop equals the legacy loop, bit for bit, at any
//! thread count, on a frozen clock -- through kills, a remote repairer, the
//! law, drones and an undock-protected pilot.

use super::*;
use crate::damage::{Damage, Profile, ShipDamage};
use crate::space_state::Motion;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SYSTEM: i64 = 30_000_001;
const RIFTER: i64 = 587;
const PILOTS: usize = 40;
const TICKS: usize = 120;
/// The invented remote-repairer row of `npc_data::Table::fixture`.
const RATAI: &str = "\
attribute\t1454\t10000.0\tnpcRemoteArmorRepairDuration
type\t990300001\t99111\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t10\t3852\t1453=0.6;1455=33.0;1456=0.4;1464=22000.0;1501=2.0\tRatai A
";

fn pilot(i: usize) -> i64 {
    90_080_001 + i as i64
}

fn ship(i: usize) -> i64 {
    100_080_001 + i as i64
}

/// The fixture rat with a remote armour repairer (typedata rows `RATAI`).
fn healer_kind() -> &'static NpcType {
    static KIND: std::sync::OnceLock<NpcType> = std::sync::OnceLock::new();
    KIND.get_or_init(|| NpcType {
        type_id: 990_300_001,
        hull: Hull {
            type_id: 990_300_001,
            ..FIXTURE_RAT.hull
        },
        ..FIXTURE_RAT
    })
}

fn frail(at: BlueTime, hp: f64) -> ShipDamage {
    let one = Damage {
        em: 1.0,
        explosive: 1.0,
        kinetic: 1.0,
        thermal: 1.0,
    };
    ShipDamage::full(
        Profile {
            shield_capacity: hp,
            shield_recharge_rate: 1.5e6,
            armor_hp: hp,
            hp,
            shield_resonance: one,
            armor_resonance: one,
            hull_resonance: one,
        },
        at,
    )
}

/// What an arena holds, so the tests can look at it.
struct Arena {
    reg: Registry,
    /// Fixture rats, then the healers, then the law.
    rats: Vec<i64>,
    healers: Vec<i64>,
    wounded: Vec<i64>,
    now: BlueTime,
    /// This world's own wall clock (the frozen one is the thread's, shared
    /// by every arena a test steps in turn).
    at: Duration,
}

/// Forty pilots, a ring of rats round them, three remote repairers with
/// hurt wards, two ships of the law on one pilot, ten drones engaging rats,
/// one pilot under undock protection, and half the pilots one volley from
/// death. Frozen clock; invented ids.
fn arena() -> Arena {
    let at = Duration::from_secs(1_800_000_000);
    crate::test_clock::freeze(at);
    let chars = (0..PILOTS)
        .map(|i| crate::model::Character {
            character_id: pilot(i),
            name: format!("Pilot {i}"),
            ship_id: ship(i),
            ship_type_id: RIFTER,
            ..crate::model::Character::default()
        })
        .collect();
    let table = crate::typedata::Table::parse(RATAI).expect("the invented rows parse");
    let mut content = crate::content::Content::fixture();
    content.insert("npc_data", crate::npc_data::Table::fixture());
    content.insert("typedata", crate::typedata::Installed::build(table));
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: chars,
        ..crate::model::SandboxConfig::default()
    })
    .with_content(Arc::new(content));
    for i in 0..PILOTS {
        crate::destiny::on_ballpark_bind(&mut reg, Some(pilot(i)));
        let at = [(i % 8) as f64 * 1_500.0, 0.0, (i / 8) as f64 * 1_500.0];
        let p = reg.roster_mut().get_mut(pilot(i)).expect("in space");
        p.motion = Motion::arrived(at, [0.0; 3]);
    }
    let now = crate::pyutil::filetime_now();
    for i in (0..PILOTS).step_by(2) {
        reg.combat_mut().set_ship(ship(i), frail(now, 1.0));
    }
    let mut rats = Vec::new();
    for k in 0..24 {
        let a = k as f64 * 0.26;
        let at = [
            6_000.0 + 14_000.0 * a.cos(),
            500.0,
            3_000.0 + 14_000.0 * a.sin(),
        ];
        rats.push(spawn(&mut reg, &FIXTURE_RAT, SYSTEM, at).expect("an id"));
    }
    let mut healers = Vec::new();
    for k in 0..3 {
        let at = [2_000.0 * k as f64, 800.0, 16_000.0];
        healers.push(spawn(&mut reg, healer_kind(), SYSTEM, at).expect("an id"));
    }
    for k in 0..2 {
        let at = [-4_000.0, 100.0 * k as f64, 2_000.0];
        let law = Law { offender: pilot(3) };
        rats.push(spawn_bound(&mut reg, &FIXTURE_RAT, SYSTEM, at, law).expect("an id"));
    }
    // Hurt wards for the repairers: past the 0.4 threshold of the row.
    let wounded: Vec<i64> = rats[..6].to_vec();
    for w in &wounded {
        let mut hurt = frail(now, 100.0);
        hurt.apply(
            Damage {
                em: 180.0,
                ..Damage::default()
            },
            now,
        );
        reg.combat_mut().set_ship(*w, hurt);
    }
    // Ten drones of two pilots, engaging rats.
    for (k, rat) in rats.iter().copied().enumerate().take(10) {
        let owner = 6 + k % 2;
        let id = 195_080_001 + k as i64;
        let at = reg
            .roster()
            .get(pilot(owner))
            .expect("in space")
            .motion
            .position;
        reg.roster_mut().arrive(Presence::arriving(
            id,
            id,
            SYSTEM,
            Motion::arrived([at[0] + 50.0 * k as f64, at[1], at[2] + 80.0], [0.0; 3]),
        ));
        let mut d = crate::drones::Drone::launched(
            id,
            crate::drones::HOBGOBLIN_I,
            pilot(owner),
            ship(owner),
            SYSTEM,
        );
        d.order = crate::drones::Order::Engage(rat);
        reg.modules_mut().drones_mut().insert(d);
    }
    // NPC-THREAT: a ledger the views read -- pilots far and near have hurt
    // some rats, so candidates carry threat and the 10 s window runs out
    // inside the test's 30 s.
    for (k, rat) in rats.iter().copied().enumerate().take(12) {
        for i in 0..3 {
            let who = pilot((k + 7 * i) % PILOTS);
            crate::npc_threat::on_act(&mut reg, who, rat, 20.0 * (i as f64 + 1.0), now);
        }
    }
    crate::invuln::set_enabled(&mut reg, true);
    crate::invuln::grant(&mut reg, pilot(5), ship(5), now);
    let _ = reg.take_pending();
    let _ = reg.take_due(Instant::now() + Duration::from_secs(3_600));
    Arena {
        reg,
        rats,
        healers,
        wounded,
        now,
        at,
    }
}

#[derive(Clone, Copy)]
enum How {
    Legacy,
    Planned(Tuning),
}

fn tuning(threads: usize, gate: usize, fault: Option<i64>) -> How {
    How::Planned(Tuning {
        threads,
        gate,
        fault,
    })
}

/// Every piece of state the loop writes, as text: equal strings are
/// bit-equal states (`{:?}` of an `f64` round-trips).
fn fingerprint(reg: &mut Registry, actions: &[destiny::Action]) -> String {
    let npcs: Vec<(i64, Phase, Option<i64>, bool, Mind)> = reg
        .npcs()
        .iter()
        .map(|n| (n.id, n.phase, n.target, n.firing, n.mind))
        .collect();
    let balls = reg.roster().in_system(SYSTEM);
    let ships: Vec<_> = balls
        .iter()
        .map(|p| (p.ship_id, reg.combat().ship(p.ship_id).copied()))
        .collect();
    let pending: Vec<_> = reg
        .take_pending()
        .into_iter()
        .map(|(after, p)| (after, p.audience(), p.to_push()))
        .collect();
    let due: Vec<_> = reg
        .take_due(Instant::now() + Duration::from_secs(3_600))
        .into_iter()
        .map(|p| (p.audience(), p.to_push()))
        .collect();
    format!(
        "{npcs:?}\n{balls:?}\n{:?}\n{ships:?}\n{actions:?}\n{pending:?}\n{due:?}",
        reg.locks()
    )
}

/// `assert_eq!` for two fingerprints that shows where they part, not both.
fn same(got: &str, want: &str, what: &str) {
    if got == want {
        return;
    }
    let at = got
        .bytes()
        .zip(want.bytes())
        .position(|(g, w)| g != w)
        .unwrap_or(got.len().min(want.len()));
    let around = |s: &str| s[at.saturating_sub(300)..(at + 300).min(s.len())].to_string();
    panic!(
        "{what}: parted at byte {at}\n got: ...{}\nwant: ...{}",
        around(got),
        around(want)
    );
}

/// One clock turn of the NPC stage on a system, as `tick_system` does it.
fn turn(a: &mut Arena, how: How) -> (String, Option<RunStats>) {
    a.at += Duration::from_millis(250);
    crate::test_clock::freeze(a.at);
    a.now += 2_500_000;
    let reg = &mut a.reg;
    let alone = nobody_here(reg, SYSTEM);
    let ids: Vec<i64> = reg
        .npcs()
        .iter()
        .filter(|n| n.solar_system_id == SYSTEM)
        .map(|n| n.id)
        .collect();
    reg.advance_system(SYSTEM);
    let mut actions = Vec::new();
    let stats = match how {
        How::Legacy => {
            run_system_legacy(reg, a.now, alone, ids, &mut actions);
            None
        }
        How::Planned(t) => Some(run_planned(reg, SYSTEM, a.now, alone, ids, t, &mut actions)),
    };
    (fingerprint(reg, &actions), stats)
}

fn dead(a: &Arena) -> usize {
    (0..PILOTS)
        .filter(|i| a.reg.roster().get(pilot(*i)).map(|p| p.ship_id) != Some(ship(*i)))
        .count()
}

#[test]
fn scene_plan_equals_the_legacy_loop() {
    let mut worlds = [arena(), arena(), arena()];
    let hows = [
        How::Legacy,
        tuning(1, NPC_PAR_WORK, None),
        tuning(4, 0, None),
    ];
    let (mut planned, mut fresh, mut kill_ticks, mut stale_without_cause) = (0, 0, 0, 0);
    let before: Vec<f64> = worlds[0]
        .wounded
        .iter()
        .map(|w| worlds[0].reg.combat().ship(*w).expect("hurt").armour_left())
        .collect();
    let spawned0 = crate::par::spawned_here();
    for t in 0..TICKS {
        let dead0 = dead(&worlds[0]);
        let gold = turn(&mut worlds[0], hows[0]).0;
        let killed = dead(&worlds[0]) > dead0;
        let revision0 = worlds[1].reg.roster().revision();
        for k in 1..3 {
            let (got, stats) = turn(&mut worlds[k], hows[k]);
            same(
                &got,
                &gold,
                &format!("world {k} against the legacy loop at tick {t}"),
            );
            let stats = stats.expect("planned");
            assert!(!stats.fell_back);
            if k == 2 {
                planned += stats.planned;
                fresh += stats.fresh;
                if killed {
                    kill_ticks += 1;
                    assert!(
                        stats.fresh > 0,
                        "a kill leaves later plans stale (tick {t})"
                    );
                }
            }
            if k == 1 && !killed && worlds[1].reg.roster().revision() == revision0 {
                stale_without_cause += stats.fresh;
            }
        }
    }
    assert!(planned > 0, "plans were used");
    assert!(fresh > 0, "stale plans were worked out again");
    assert!(kill_ticks > 0, "pilots died inside the loop");
    assert!(dead(&worlds[0]) >= 2, "{} dead", dead(&worlds[0]));
    assert_eq!(
        stale_without_cause, 0,
        "nothing changed, yet a plan was thrown away"
    );
    // A remote repairer mended a ward (an `Intent::Assist` was applied).
    let after: Vec<f64> = worlds[0]
        .wounded
        .iter()
        .map(|w| worlds[0].reg.combat().ship(*w).expect("hurt").armour_left())
        .collect();
    assert!(
        before.iter().zip(&after).any(|(b, a)| a > b),
        "{before:?} -> {after:?} with healers {:?}",
        worlds[0].healers
    );
    assert!(
        crate::par::spawned_here() > spawned0,
        "world 2 planned on workers"
    );
    crate::test_clock::thaw();
}

/// With one thread nothing is spawned; the production gate keeps a small
/// system on the caller's thread at any thread count.
#[test]
fn a_small_system_plans_on_the_callers_thread() {
    let mut a = arena();
    let spawned0 = crate::par::spawned_here();
    for _ in 0..5 {
        let _ = turn(&mut a, tuning(1, 0, None));
        let _ = turn(&mut a, tuning(12, NPC_PAR_WORK, None));
    }
    assert_eq!(crate::par::spawned_here(), spawned0);
    crate::test_clock::thaw();
}

/// A panic in a plan (here planted in the last rat's, which a worker plans)
/// sends the system to the legacy loop and changes nothing.
#[test]
fn a_worker_panic_falls_back_to_the_serial_loop() {
    let mut worlds = [arena(), arena()];
    let fault = *worlds[1].rats.last().expect("rats");
    let hows = [How::Legacy, tuning(4, 0, Some(fault))];
    for t in 0..40 {
        let gold = turn(&mut worlds[0], hows[0]).0;
        let (got, stats) = turn(&mut worlds[1], hows[1]);
        same(&got, &gold, &format!("tick {t}"));
        assert!(stats.expect("planned").fell_back);
    }
    crate::test_clock::thaw();
}

/// On a world nobody has touched the scene's view is `view_of`'s.
#[test]
fn view_in_equals_view_of_on_a_still_world() {
    let mut a = arena();
    a.reg.advance_system(SYSTEM);
    let scene = scene_of(&a.reg, SYSTEM, a.now);
    let (mut seen, mut threatened) = (0, 0);
    let all: Vec<Npc> = a.reg.npcs().iter().copied().collect();
    for n in all {
        let got = view_in(&a.reg, &scene, &n, a.now);
        assert_eq!(got, view_of(&a.reg, &n, a.now), "npc {}", n.id);
        seen += got.as_ref().map_or(0, |v| v.candidates.len());
        threatened += got.map_or(0, |v| v.candidates.iter().filter(|c| c.aggressor).count());
    }
    assert!(seen > 0, "the views were not empty");
    assert!(threatened > 0, "the views carry the ledger");
    crate::test_clock::thaw();
}

/// A plan is stale once the protections its view filtered by change: a grant
/// (undock or gate jump) neither moves the roster's revision nor the NPC's
/// record, so the stale check compares [`cover_marks`] by value. A pilot who
/// gets protected after the plan leaves the planned view with a candidate the
/// live one drops.
#[test]
fn a_grant_after_the_plan_changes_the_cover_marks_and_the_view() {
    let mut a = arena();
    a.reg.advance_system(SYSTEM);
    let rat = a.rats[0];
    let npc = *a.reg.npcs().get(rat).expect("a rat");
    let marks = cover_marks(&a.reg);
    let revision = a.reg.roster().revision();
    let planned = view_of(&a.reg, &npc, a.now);
    // Pilot 11 (in the arena's open) jumps a gate: protected at once.
    crate::invuln::grant(&mut a.reg, pilot(11), ship(11), a.now);
    assert_eq!(a.reg.roster().revision(), revision, "the roster is unmoved");
    assert_ne!(cover_marks(&a.reg), marks, "the plan is stale");
    let live = view_of(&a.reg, &npc, a.now);
    let had = |v: &Option<View>| {
        v.as_ref()
            .is_some_and(|v| v.candidates.iter().any(|c| c.ship_id == ship(11)))
    };
    assert!(had(&planned) && !had(&live), "the views differ");
    crate::test_clock::thaw();
}
