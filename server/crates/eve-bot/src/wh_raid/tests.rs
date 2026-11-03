use super::*;
use crate::brain::{Ball, GunInfo, Hostile, ModuleInfo, ParkView, Place, Policy, ShipInfo};
use crate::hull_choice::{Class, HULLS};

fn hull(class: Class) -> i64 {
    HULLS
        .iter()
        .find(|h| h.class == class)
        .expect("the table has every class")
        .type_id
}

fn server() -> SocketAddr {
    "127.0.0.1:1".parse().expect("literal")
}

fn ball(id: i64, x: f64) -> Ball {
    Ball {
        id,
        position: [x, 0.0, 0.0],
        radius: 50.0,
    }
}

fn gun() -> GunInfo {
    GunInfo {
        item_id: 2,
        type_id: crate::escort::GUN,
        flag_id: 27,
        online: true,
        active: false,
        target: None,
        rounds: Some(100),
        capacity: 100,
        range: 1_000.0,
        reload: Duration::ZERO,
        kind: crate::brain::GunKind::Weapon,
        reach: 0.0,
    }
}

fn park() -> ParkView {
    ParkView {
        ego: ball(1, 0.0),
        ego_warping: false,
        rocks: vec![],
        belts: vec![],
        station: None,
        boss: None,
        taken: BTreeMap::new(),
        hostiles: vec![],
        wrecks: vec![],
        ego_tether: None,
    }
}

/// A bot in space in a hull of `class`, armed, with a salvager fitted.
fn scene(class: Class, j_space: bool, now: Instant) -> Observation {
    let mut o = Observation::empty(now);
    o.place = Place::InSpace {
        system: 30_000_001,
        bound: true,
    };
    o.ship = Some(ShipInfo {
        ship_id: 1,
        type_id: Some(hull(class)),
    });
    o.guns = vec![gun()];
    o.lock_range = 20_000.0;
    o.salvager = Some(ModuleInfo {
        item_id: 9,
        flag_id: 19,
        online: true,
        active: false,
        target: None,
        range: 5_000.0,
    });
    o.j_space = j_space;
    o.park = Some(park());
    o
}

fn memory() -> Memory {
    Memory::new("Raider1", Policy::fast())
}

/// The quiet grid has settled: the site runner's next step is the Agency.
fn settled(m: &mut Memory, o: &mut Observation, now: Instant) -> Vec<Act> {
    let _ = in_space(m, o);
    o.now = now + site_runner::SITE_SETTLE * 2;
    in_space(m, o)
}

// ---- the class rule ------------------------------------------------------------

#[test]
fn a_class_runs_the_difficulty_its_table_says_and_no_class_is_outside_c1_to_c6() {
    assert_eq!(class_difficulty(0), None);
    assert_eq!(class_difficulty(7), None);
    assert_eq!(class_difficulty(-1), None);
    for class in 1..=6 {
        assert_eq!(
            class_difficulty(class),
            Some(CLASS_DIFFICULTY[(class - 1) as usize])
        );
    }
    for pair in CLASS_DIFFICULTY.windows(2) {
        assert!(pair[0] <= pair[1], "a higher class is never easier");
    }
    assert!(CLASS_DIFFICULTY.iter().all(|d| *d >= MIN_DIFFICULTY));
}

#[test]
fn a_hull_is_sent_only_where_its_cap_runs_every_site() {
    // Below the lowest anomaly: nowhere.
    assert_eq!(max_class(MIN_DIFFICULTY - 1), None);
    assert!(!capable(Some(MIN_DIFFICULTY - 1)));
    assert!(!capable(None));
    // Monotone, and always a class whose difficulty fits.
    let mut last = 0;
    for cap in MIN_DIFFICULTY..=8 {
        let c = max_class(cap).expect("at the minimum, class 1");
        assert!(c >= last);
        assert!(class_difficulty(c).expect("class") <= cap);
        if c < 6 {
            assert!(class_difficulty(c + 1).expect("class") > cap, "the highest");
        }
        last = c;
    }
    assert_eq!(max_class(99), Some(6));
}

#[test]
fn the_combat_ladder_decides_who_may_dive() {
    let cap = |c: Class| site_runner::difficulty_cap(hull(c));
    assert!(!capable(cap(Class::Frigate)), "frigates stay in k-space");
    for c in [
        Class::Destroyer,
        Class::Cruiser,
        Class::Battlecruiser,
        Class::Battleship,
    ] {
        assert!(capable(cap(c)), "{c:?}");
    }
    // Bigger hulls may go at least as deep.
    let depth = |c: Class| cap(c).and_then(max_class).unwrap_or(0);
    assert!(depth(Class::Destroyer) <= depth(Class::Cruiser));
    assert!(depth(Class::Cruiser) <= depth(Class::Battleship));
}

#[test]
fn a_dive_may_be_capped_to_a_class() {
    for class in 1..=6 {
        assert!(Leads::JSpaceTo(3).matches(class) == (class <= 3));
        assert!(!Leads::JSpaceTo(6).matches(class + 6));
    }
    assert!(!Leads::JSpaceTo(6).matches(0));
    assert!(!Leads::JSpaceTo(6).matches(8), "k-space is not J-space");
}

// ---- the board ---------------------------------------------------------------------

#[test]
fn a_raider_posts_cleared_sites_and_a_hacker_from_the_same_system_finds_them() {
    let board = Board::new();
    let now = Instant::now();
    assert!(!board.due_from(server(), 100, now));
    board.post(server(), 900, 100, "ABC-123", now);
    assert!(board.due_from(server(), 100, now));
    assert!(!board.due_from(server(), 101, now), "another home");
    assert!(!board.due_from("127.0.0.1:2".parse().expect("literal"), 100, now));
    assert!(board.is_cleared(server(), 900, "ABC-123", now));
    assert!(!board.is_cleared(server(), 900, "XYZ-999", now));
    assert!(!board.is_cleared(server(), 901, "ABC-123", now));
    assert_eq!(board.systems_from(server(), 100, now), vec![900]);
    // A second post of the same site is one site.
    board.post(server(), 900, 100, "ABC-123", now);
    board.retire(server(), 900, "ABC-123");
    assert!(!board.is_cleared(server(), 900, "ABC-123", now));
    assert!(!board.due_from(server(), 100, now), "nothing left");
}

#[test]
fn guards_come_back_after_the_ttl() {
    let board = Board::new();
    let now = Instant::now();
    board.post(server(), 900, 100, "ABC-123", now);
    let almost = now + CLEARED_TTL - Duration::from_secs(1);
    assert!(board.is_cleared(server(), 900, "ABC-123", almost));
    assert!(board.due_from(server(), 100, almost));
    let late = now + CLEARED_TTL;
    assert!(!board.is_cleared(server(), 900, "ABC-123", late));
    assert!(!board.due_from(server(), 100, late));
    assert!(board.systems_from(server(), 100, late).is_empty());
}

// ---- the rules -------------------------------------------------------------------

#[test]
fn a_rested_armed_cruiser_with_a_salvager_dives_and_nothing_else_does() {
    let now = Instant::now();
    let m = memory();
    let o = scene(Class::Cruiser, false, now);
    assert!(want_dive(&m, &o));
    // A frigate, no salvager, no guns, a full hold, already inside, a trip
    // home under way, a failed try: none.
    assert!(!want_dive(&m, &scene(Class::Frigate, false, now)));
    let mut x = scene(Class::Cruiser, false, now);
    x.salvager = None;
    assert!(!want_dive(&m, &x));
    let mut x = scene(Class::Cruiser, false, now);
    x.guns.clear();
    assert!(!want_dive(&m, &x));
    let mut x = scene(Class::Cruiser, false, now);
    x.hold_full = true;
    assert!(!want_dive(&m, &x));
    assert!(!want_dive(&m, &scene(Class::Cruiser, true, now)));
    let mut homing = memory();
    homing.homing = Some(Homing::Repair);
    assert!(!want_dive(&homing, &o));
    let mut x = scene(Class::Cruiser, false, now);
    x.failures.insert(ActKind::Raid, GIVE_UP_AFTER);
    assert!(!want_dive(&m, &x));
    // Rested only after the rest.
    let mut rested = memory();
    rested.raid.retry_after = Some(now + DIVE_REST);
    assert!(!want_dive(&rested, &o));
    let mut later = o.clone();
    later.now = now + DIVE_REST;
    assert!(want_dive(&rested, &later));
}

#[test]
fn in_k_space_the_raider_dives_when_it_may_and_runs_sites_when_it_may_not() {
    let now = Instant::now();
    let mut m = memory();
    let o = scene(Class::Cruiser, false, now);
    assert_eq!(in_space(&mut m, &o), vec![Act::Raid(Op::In)]);
    // A frigate is the site runner exactly.
    let f = scene(Class::Frigate, false, now);
    let mut a = memory();
    let mut b = memory();
    assert_eq!(
        in_space(&mut a, &f),
        (site_runner::ROLE.in_space)(&mut b, &f)
    );
}

#[test]
fn a_failed_dive_waits_before_the_next_and_the_bot_runs_k_space_sites_meanwhile() {
    let now = Instant::now();
    let mut m = memory();
    let mut o = scene(Class::Cruiser, false, now);
    o.failures.insert(ActKind::Raid, 1);
    let acts = in_space(&mut m, &o);
    assert!(!acts.contains(&Act::Raid(Op::In)), "{acts:?}");
    assert_eq!(m.raid.retry_after, Some(now + DIVE_RETRY));
    // Past the wait, with the failure gone, it tries again.
    o.failures.clear();
    o.now = now + DIVE_RETRY;
    assert_eq!(in_space(&mut m, &o), vec![Act::Raid(Op::In)]);
}

#[test]
fn inside_a_quiet_grid_runs_the_next_anomaly_then_sweeps_when_the_agency_has_none() {
    let now = Instant::now();
    let mut m = memory();
    let mut o = scene(Class::Cruiser, true, now);
    assert_eq!(settled(&mut m, &mut o, now).last(), Some(&Act::Sites));
    assert_eq!(m.raid.phase, Phase::Combat);
    // The Agency had no anomaly: sweep.
    o.failures.insert(ActKind::Sites, 1);
    // The site runner settles again before it asks the Agency again.
    o.now = now + site_runner::SITE_SETTLE * 3;
    let _ = in_space(&mut m, &o);
    o.now = now + site_runner::SITE_SETTLE * 6;
    let acts = in_space(&mut m, &o);
    assert_eq!(acts.last(), Some(&Act::Raid(Op::Sweep)), "{acts:?}");
    assert_eq!(m.raid.phase, Phase::Sweep);
}

#[test]
fn a_hull_too_light_to_sweep_goes_out_when_the_anomalies_are_done() {
    let now = Instant::now();
    let mut m = memory();
    let mut o = scene(Class::Destroyer, true, now);
    assert!(
        !may_sweep(&m, &o),
        "the premise: a destroyer's cap is under SWEEP_MIN_CAP"
    );
    let _ = in_space(&mut m, &o);
    o.failures.insert(ActKind::Sites, 1);
    o.now = now + site_runner::SITE_SETTLE * 2;
    let acts = in_space(&mut m, &o);
    assert_eq!(acts.last(), Some(&Act::Raid(Op::Out)), "{acts:?}");
    assert!(m.raid.done);
}

#[test]
fn a_swept_site_that_goes_quiet_is_cleared_and_the_cap_of_sweeps_ends_the_dive() {
    let now = Instant::now();
    let mut m = memory();
    m.raid.phase = Phase::Sweep;
    m.raid.swept = vec!["AAA-111".to_owned()];
    m.raid.sweeping = Some("AAA-111".to_owned());
    let mut o = scene(Class::Battleship, true, now);
    let acts = settled(&mut m, &mut o, now);
    assert_eq!(m.raid.cleared, vec!["AAA-111".to_owned()]);
    assert!(m.raid.sweeping.is_none());
    assert_eq!(acts.last(), Some(&Act::Raid(Op::Sweep)), "one more");
    // The cap reached: out.
    m.raid.swept = (0..MAX_SWEEPS).map(|i| format!("S-{i}")).collect();
    m.raid.sweeping = Some("S-0".to_owned());
    m.raid.landed = None;
    o.now += site_runner::SITE_SETTLE * 4;
    m.dwell_until = None;
    let _ = in_space(&mut m, &o);
    o.now += site_runner::SITE_SETTLE * 2;
    let acts = in_space(&mut m, &o);
    assert_eq!(acts.last(), Some(&Act::Raid(Op::Out)), "{acts:?}");
    assert!(m.raid.done);
}

#[test]
fn a_failed_sweep_means_no_signature_left_and_the_dive_ends() {
    let now = Instant::now();
    let mut m = memory();
    m.raid.phase = Phase::Sweep;
    let mut o = scene(Class::Battleship, true, now);
    o.failures.insert(ActKind::Raid, 1);
    let acts = settled(&mut m, &mut o, now);
    assert_eq!(acts.last(), Some(&Act::Raid(Op::Out)), "{acts:?}");
}

#[test]
fn a_fight_is_fought_to_the_end_even_with_a_reason_to_go_home() {
    let now = Instant::now();
    let mut m = memory();
    m.homing = Some(Homing::Repair);
    let mut o = scene(Class::Cruiser, true, now);
    let mut p = park();
    p.hostiles = vec![Hostile {
        ball: ball(50, 5_000.0),
        name: "Sleeper".into(),
        type_id: 1,
        shooting: None,
    }];
    o.park = Some(p);
    let acts = in_space(&mut m, &o);
    assert!(!acts.contains(&Act::Raid(Op::Out)), "{acts:?}");
    assert!(
        acts.iter().any(|a| matches!(a, Act::Orbit { id: 50, .. })),
        "{acts:?}"
    );
    assert_eq!(m.homing, None, "no retreat mid-fight");
}

#[test]
fn a_quiet_grid_and_a_reason_to_go_home_leave_by_the_hole() {
    let now = Instant::now();
    for why in [Homing::HoldFull, Homing::Repair, Homing::Rearm] {
        let mut m = memory();
        m.homing = Some(why);
        let o = scene(Class::Cruiser, true, now);
        let acts = in_space(&mut m, &o);
        assert_eq!(acts, vec![Act::Raid(Op::Out)], "{why:?}");
    }
}

#[test]
fn the_ceiling_ends_a_dive() {
    let now = Instant::now();
    let mut m = memory();
    let mut o = scene(Class::Cruiser, true, now);
    let _ = in_space(&mut m, &o);
    assert_eq!(m.raid.inside_since, Some(now));
    o.now = now + RAID_CEILING;
    assert_eq!(in_space(&mut m, &o), vec![Act::Raid(Op::Out)]);
    assert!(m.homing.is_some(), "the way home follows the way out");
}

#[test]
fn with_no_way_out_found_the_bot_says_so_instead_of_scanning_forever() {
    let now = Instant::now();
    let mut m = memory();
    m.homing = Some(Homing::HoldFull);
    let mut o = scene(Class::Cruiser, true, now);
    o.failures.insert(ActKind::Raid, GIVE_UP_AFTER);
    let acts = in_space(&mut m, &o);
    assert!(
        acts.iter().any(|a| matches!(a, Act::Missing(_))),
        "{acts:?}"
    );
    assert!(!acts.contains(&Act::Raid(Op::Out)));
}

#[test]
fn coming_out_ends_the_dive_and_starts_the_rest() {
    let now = Instant::now();
    let mut s = State {
        entered: Some(wh_dive::Entered {
            from: 1,
            class: 2,
            arrival: [0.0; 3],
        }),
        system: Some(9),
        inside_since: Some(now),
        phase: Phase::Sweep,
        swept: vec!["A".to_owned()],
        cleared: vec!["A".to_owned()],
        done: true,
        ..State::default()
    };
    s.finish(now);
    assert_eq!(
        s,
        State {
            retry_after: Some(now + DIVE_REST),
            ..State::default()
        }
    );
    // And the k-space bot that finds a stale dive in its memory ends it.
    let mut m = memory();
    m.raid.inside_since = Some(now);
    let o = scene(Class::Frigate, false, now);
    let _ = in_space(&mut m, &o);
    assert_eq!(m.raid.inside_since, None);
    assert!(m.raid.retry_after.is_some());
}

#[test]
fn the_role_and_the_career_are_wired() {
    assert_eq!(ROLE.name(), "raid");
    assert!(!ROLE.civilian);
    assert!(crate::roles::ROLES.iter().any(|r| r.name() == "raid"));
    let c = crate::career::CAREERS
        .iter()
        .find(|c| c.name == "wormhole_raider")
        .expect("a raider row");
    assert!(c.served());
    assert_eq!(c.role.map(|r| r.name()), Some("raid"));
    assert_eq!(Op::In.describe(), Op::In.describe());
    assert_eq!(Act::Raid(Op::In).kind(), ActKind::Raid);
    assert_eq!(ActKind::Raid.name(), "raid");
}

// ---- the model -------------------------------------------------------------------

#[test]
fn a_wreck_costs_cycles_and_is_salvaged_with_the_chance_of_the_tries() {
    let (cycles, ok) = model::wreck_cycles(0.5, 1);
    assert!((cycles - 1.0).abs() < 1e-9 && (ok - 0.5).abs() < 1e-9);
    let (c1, o1) = model::wreck_cycles(0.58, 1);
    let (c5, o5) = model::wreck_cycles(0.58, 5);
    assert!(o5 > o1 && c5 > c1);
    assert!(o5 <= 1.0 && c5 <= 5.0);
    let (c0, o0) = model::wreck_cycles(0.0, 5);
    assert_eq!((c0, o0), (5.0, 0.0));
}

#[test]
fn the_scarcest_type_adds_to_the_mean() {
    let f = model::binding_factor(19, 124, 40);
    assert!(
        f > 1.0,
        "{f}: the mean is reached long before every type is"
    );
    assert!(f < 2.0, "{f}");
    assert_eq!(f, model::binding_factor(19, 124, 40), "reproducible");
    // One type has no spread to wait for.
    let one = model::binding_factor(1, 100, 10);
    assert!((1.0..1.05).contains(&one), "{one}");
}

#[test]
fn more_raiders_never_slow_a_kit_and_the_anomalies_cap_the_gain() {
    let p = model::Params::default();
    let days: Vec<f64> = [1, 2, 4, 8, 16, 64]
        .iter()
        .map(|n| model::days_to_kit(&p, *n))
        .collect();
    for w in days.windows(2) {
        assert!(w[1] <= w[0], "{days:?}");
    }
    assert_eq!(days[4], days[5], "the supply of anomalies binds: {days:?}");
    // Wrecks that yield nothing: never.
    let hollow = model::Params {
        wrecks_per_anomaly: 0.0,
        ..p
    };
    assert_eq!(model::per_bot_day(&hollow), 0.0);
    assert!(model::days_to_kit(&hollow, 10).is_infinite());
    // More systems, more supply.
    let more = model::Params {
        systems: p.systems * 4.0,
        ..p
    };
    assert!(model::supply_anomalies_per_day(&more) > model::supply_anomalies_per_day(&p));
}

#[test]
fn a_dive_clears_whole_anomalies_inside_its_ceiling() {
    let p = model::Params::default();
    let k = model::anomalies_per_dive(&p);
    assert_eq!(k, k.floor());
    let used = p.overhead_s + p.scan_s + k * model::anomaly_s(&p);
    assert!(used <= p.ceiling_s);
    assert!(
        used + model::anomaly_s(&p) > p.ceiling_s,
        "one more would not fit"
    );
}

// ---- the guards get time to come ------------------------------------------------

#[test]
fn a_grid_just_set_out_for_is_not_clear_until_a_guard_has_come_or_the_patience_is_out() {
    let now = Instant::now();
    let mut m = memory();
    let mut o = scene(Class::Cruiser, true, now);
    // The first look: the arrival grid. The Agency is asked, and the bot
    // remembers that it set out.
    assert_eq!(settled(&mut m, &mut o, now).last(), Some(&Act::Sites));
    assert_eq!(m.raid.landed, Some(o.now));
    assert!(!m.raid.fought);
    // The site runner settles on the new grid and asks again; nobody has
    // come yet, so the raider waits instead.
    let set_out = o.now;
    let _ = in_space(&mut m, &o);
    o.now = set_out + site_runner::SITE_SETTLE * 2;
    assert_eq!(
        in_space(&mut m, &o).last(),
        Some(&Act::Wait(brain::SHORT_WAIT))
    );
    // The patience out, the grid is an empty site: on to the next.
    o.now = set_out + GRID_PATIENCE;
    let _ = in_space(&mut m, &o);
    o.now += site_runner::SITE_SETTLE * 2;
    assert_eq!(in_space(&mut m, &o).last(), Some(&Act::Sites));
}

#[test]
fn a_fight_on_the_grid_makes_the_quiet_after_it_a_clear_site() {
    let now = Instant::now();
    let mut m = memory();
    let mut o = scene(Class::Cruiser, true, now);
    let _ = settled(&mut m, &mut o, now);
    let set_out = o.now;
    // A Sleeper comes and dies.
    let mut p = park();
    p.hostiles = vec![Hostile {
        ball: ball(50, 5_000.0),
        name: "Sleeper".into(),
        type_id: 1,
        shooting: None,
    }];
    o.park = Some(p);
    let _ = in_space(&mut m, &o);
    assert!(m.raid.fought);
    o.park = Some(park());
    m.dwell_until = None;
    let _ = in_space(&mut m, &o);
    o.now = set_out + site_runner::SITE_SETTLE * 3;
    assert_eq!(
        in_space(&mut m, &o).last(),
        Some(&Act::Sites),
        "well inside the patience"
    );
    assert!(o.now < set_out + GRID_PATIENCE);
}

#[test]
fn a_refused_agency_call_is_no_grid_to_wait_on() {
    let now = Instant::now();
    let mut m = memory();
    let mut o = scene(Class::Cruiser, true, now);
    let _ = settled(&mut m, &mut o, now);
    // The Agency had nothing: the bot did not move, so nothing is awaited.
    o.failures.insert(ActKind::Sites, 1);
    let _ = in_space(&mut m, &o);
    o.now += site_runner::SITE_SETTLE * 3;
    let acts = in_space(&mut m, &o);
    assert_eq!(acts.last(), Some(&Act::Raid(Op::Sweep)), "{acts:?}");
}

#[test]
fn a_swept_grid_waits_for_its_guards_before_it_is_called_cleared() {
    let now = Instant::now();
    let mut m = memory();
    m.raid.phase = Phase::Sweep;
    m.raid.sweeping = Some("AAA-111".to_owned());
    m.raid.swept = vec!["AAA-111".to_owned()];
    m.raid.landed = Some(now);
    let mut o = scene(Class::Battleship, true, now);
    let _ = in_space(&mut m, &o);
    o.now = now + site_runner::SITE_SETTLE * 2;
    let acts = in_space(&mut m, &o);
    assert_eq!(acts.last(), Some(&Act::Wait(brain::SHORT_WAIT)), "{acts:?}");
    assert!(m.raid.cleared.is_empty(), "not cleared yet");
    o.now = now + GRID_PATIENCE;
    let _ = in_space(&mut m, &o);
    o.now += site_runner::SITE_SETTLE * 2;
    let _ = in_space(&mut m, &o);
    assert_eq!(m.raid.cleared, vec!["AAA-111".to_owned()]);
}

#[test]
fn a_hull_that_may_dive_outfits_its_launcher_once_a_dock_before_it_undocks() {
    let now = Instant::now();
    let mut o = scene(Class::Cruiser, false, now);
    o.place = Place::Docked {
        station: 60_000_001,
    };
    o.park = None;
    let mut m = memory();
    let first = docked(&mut m, &o);
    // Whatever the site runner would do, the launcher comes before an undock.
    if let Some(at) = first.iter().position(|a| matches!(a, Act::Undock)) {
        assert_eq!(first[at - 1], Act::Raid(Op::Outfit), "{first:?}");
        assert!(m.raid.outfitted);
        let again = docked(&mut m, &o);
        assert!(!again.contains(&Act::Raid(Op::Outfit)), "{again:?}");
    }
    // A frigate is never outfitted.
    let f = scene(Class::Frigate, false, now);
    let mut m = memory();
    assert!(!docked(&mut m, &f).contains(&Act::Raid(Op::Outfit)));
}

#[test]
fn the_raid_kit_is_the_ratters_and_the_scanners() {
    let kit = crate::acquire::kit_of(&ROLE);
    assert_eq!(kit, RAID_KIT);
    for want in [
        crate::salvage::SALVAGER_I,
        probe::CORE_LAUNCHER,
        probe::CORE_PROBE,
    ] {
        assert!(kit.more.iter().any(|(t, _)| *t == want), "{want}");
    }
    assert_eq!(kit.hull, crate::salvage::RATTER_KIT.hull);
}

/// **The honest measurement** (not asserted; run with `-- --ignored
/// --nocapture`): game days to the Ancient Salvage of a first Tengu kit with
/// N raiders, by the pure model and its [A] defaults, and what moves it.
#[test]
#[ignore = "prints the model's table for docs/lanes/t3-salvage.md"]
fn model_days_to_a_first_tengu_kit() {
    let p = model::Params::default();
    println!(
        "per anomaly: {:.0} s, {:.1} Ancient Salvage; {} anomalies a dive; {:.0} Ancient Salvage a bot-day; anomaly supply {:.1} a day ({:.0} units a day)",
        model::anomaly_s(&p),
        model::anomaly_units(&p),
        model::anomalies_per_dive(&p),
        model::per_bot_day(&p),
        model::supply_anomalies_per_day(&p),
        model::supply_anomalies_per_day(&p) * model::anomaly_units(&p),
    );
    println!(
        "kit need {} units over {} types; binding factor {:.2}",
        p.types as i64 * p.need_each,
        p.types,
        model::binding_factor(p.types, p.need_each, 200)
    );
    for systems in [1.0, 6.0, 30.0, 150.0] {
        let q = model::Params { systems, ..p };
        let row: Vec<String> = [1u32, 3, 5, 10, 20, 50]
            .iter()
            .map(|n| format!("N={n}: {:.1} d", model::days_to_kit(&q, *n)))
            .collect();
        println!("J systems reached {systems:>5}: {}", row.join("  "));
    }
    for kill_s in [60.0, 120.0, 300.0, 600.0] {
        let q = model::Params {
            kill_s,
            systems: 30.0,
            ..p
        };
        let row: Vec<String> = [1u32, 5, 20]
            .iter()
            .map(|n| format!("N={n}: {:.1} d", model::days_to_kit(&q, *n)))
            .collect();
        println!(
            "kill {kill_s:>4.0} s a ship (30 systems): {}",
            row.join("  ")
        );
    }
}
