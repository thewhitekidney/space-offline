//! Pure tests for the capital pair and its gates. **Every id is invented**
//! (9907xxxxx items and types, 3099xxxx systems); the client's group ids appear
//! as the constants the module names. No `[A]` value is an expected value: the
//! assertions name the constants.

use super::*;
use crate::brain::{ActKind, Place, Policy};
use crate::career::{CAREERS, Signals, block_capital, find, utility};
use crate::mind::{TRAIT_COUNT, Traits};

const DREAD: i64 = 485;
const FREIGHTER: i64 = 513;
const HOME: i64 = 30_990_001;
const FAR: i64 = 30_990_002;
const NEAR: i64 = 30_990_003;
const ALT: i64 = 90_700_002;
const PILOT: i64 = 90_700_001;

fn ship(id: i64, group: i64, singleton: bool) -> ItemRow {
    ItemRow {
        item_id: id,
        type_id: 990_700_000 + group,
        owner_id: PILOT,
        location_id: 60_990_001,
        flag_id: 4,
        quantity: 1,
        group_id: group,
        category_id: CATEGORY_SHIP,
        singleton,
    }
}

fn blocked() -> Signals {
    let mut s = Signals::default();
    block_capital(&mut s);
    s
}

fn means<'a>(hangar: &'a [ItemRow]) -> Means<'a> {
    Means {
        hangar,
        active_group: None,
        wallet: Some(1_000_000.0),
        jumper_price: None,
        freighter_price: None,
        partner: true,
    }
}

fn push(char_id: i64, system: i64, ball: i64, on: bool) -> Vec<PyValue> {
    vec![
        PyValue::Int(char_id),
        PyValue::Int(system),
        PyValue::Int(ball),
        PyValue::Int(21_094),
        PyValue::Bool(on),
    ]
}

// ---- the beacon book -----------------------------------------------------------

#[test]
fn a_lit_field_is_held_and_a_doused_one_forgotten() {
    let mut b = Beacons::default();
    b.on_push(BEACON_EVENT, &push(ALT, FAR, 9_800_000_000_001, true));
    let got = b.of(ALT).expect("held");
    assert_eq!((got.system, got.ball), (FAR, 9_800_000_000_001));
    // Lit again elsewhere: replaced, not doubled.
    b.on_push(BEACON_EVENT, &push(ALT, NEAR, 9_800_000_000_002, true));
    assert_eq!(b.len(), 1);
    assert_eq!(b.of(ALT).map(|x| x.system), Some(NEAR));
    b.on_push(BEACON_EVENT, &push(ALT, NEAR, 9_800_000_000_002, false));
    assert!(b.is_empty());
}

#[test]
fn another_event_or_a_short_push_changes_nothing() {
    let mut b = Beacons::default();
    b.on_push("OnFleetJoin", &push(ALT, FAR, 1, true));
    b.on_push(BEACON_EVENT, &[PyValue::Int(ALT)]);
    b.on_push(BEACON_EVENT, &[PyValue::Int(ALT), PyValue::Int(FAR)]);
    assert!(b.is_empty());
    // An integer `active` reads too.
    let mut args = push(ALT, FAR, 5, true);
    args[4] = PyValue::Int(1);
    b.on_push(BEACON_EVENT, &args);
    assert_eq!(b.len(), 1);
    b.clear();
    assert!(b.is_empty());
}

// ---- owning ---------------------------------------------------------------------

#[test]
fn only_an_assembled_hull_of_the_group_is_owned() {
    let hangar = vec![
        ship(30, DREAD, false),
        ship(20, FREIGHTER, true),
        ship(10, DREAD, true),
    ];
    assert_eq!(
        owned_hull(&hangar, &JUMPER_GROUPS).map(|r| r.item_id),
        Some(10)
    );
    assert_eq!(
        owned_hull(&hangar, &FREIGHTER_GROUPS).map(|r| r.item_id),
        Some(20)
    );
    assert!(owned_hull(&[ship(30, DREAD, false)], &JUMPER_GROUPS).is_none());
}

#[test]
fn the_capital_careers_are_blocked_for_everyone_until_the_bot_owns_the_hull() {
    let mut s = blocked();
    let poor = vec![ship(1, 25, true)];
    lift_blocks(&mut s, &means(&poor));
    // A frigate-class hull and 1 M ISK: no capital, no freighter. (The alt
    // needs a ship and a partner, which it has.)
    assert!(s.blocked.contains("capital_pilot"));
    assert!(s.blocked.contains("freighter_hauler"));
    assert!(!s.blocked.contains("cyno_alt"));

    let mut s = blocked();
    let owns = vec![ship(10, DREAD, true)];
    lift_blocks(&mut s, &means(&owns));
    assert!(!s.blocked.contains("capital_pilot"));
    assert!(s.blocked.contains("freighter_hauler"));

    let mut s = blocked();
    let hauls = vec![ship(20, FREIGHTER, true)];
    lift_blocks(&mut s, &means(&hauls));
    assert!(!s.blocked.contains("freighter_hauler"));
    assert!(s.blocked.contains("capital_pilot"));
}

#[test]
fn a_capital_needs_a_partner_but_a_freighter_does_not() {
    let hangar = vec![ship(10, DREAD, true), ship(20, FREIGHTER, true)];
    let mut s = blocked();
    lift_blocks(
        &mut s,
        &Means {
            partner: false,
            ..means(&hangar)
        },
    );
    assert!(s.blocked.contains("capital_pilot"));
    assert!(s.blocked.contains("cyno_alt"));
    assert!(!s.blocked.contains("freighter_hauler"));
}

#[test]
fn a_bot_with_the_isk_may_buy_a_hull_and_a_poor_one_may_not() {
    let none: Vec<ItemRow> = Vec::new();
    let price = 700_000_000.0;
    let rich = Means {
        wallet: Some(price / HULL_WALLET_SHARE),
        jumper_price: Some(price),
        ..means(&none)
    };
    assert!(rich.can_fly_capital());
    let short = Means {
        wallet: Some(price),
        ..rich
    };
    assert!(!short.can_fly_capital(), "the cushion for fuel and fees");
    let unread = Means {
        jumper_price: None,
        ..rich
    };
    assert!(!unread.can_fly_capital());
    assert!(!rich.can_haul(), "its price is the freighter's, unread");
}

#[test]
fn the_active_hull_counts_as_owned() {
    let none: Vec<ItemRow> = Vec::new();
    let m = Means {
        active_group: Some(DREAD),
        ..means(&none)
    };
    assert!(m.can_fly_capital());
}

#[test]
fn the_careers_are_blocked_in_the_signals_eve_bots_runs_with_and_open_after_the_gate() {
    let traits = Traits([0.5; TRAIT_COUNT]);
    for name in crate::career::CAPITAL_CAREERS {
        let c = find(CAREERS, name).expect("a row");
        assert!(c.served(), "{name}");
        assert_eq!(utility(c, &traits, 1e9, &blocked()), 0.0, "{name}");
    }
    let hangar = vec![ship(10, DREAD, true), ship(20, FREIGHTER, true)];
    let mut s = blocked();
    lift_blocks(&mut s, &means(&hangar));
    for name in crate::career::CAPITAL_CAREERS {
        let c = find(CAREERS, name).expect("a row");
        let mut t = Traits([0.9; TRAIT_COUNT]);
        t.0[crate::mind::Trait::Risk as usize] = 0.1;
        assert!(utility(c, &t, 1e9, &s) > 0.0, "{name}");
    }
}

#[test]
fn a_poor_bot_is_never_seeded_into_a_capital_career() {
    // 50 bots over many seeds at the signals `eve-bots` runs with: none.
    let s = blocked();
    for seed in 0..20 {
        for i in 0..50 {
            let m = crate::mind::Mind::seeded(&format!("Invented Pilot {i}"), seed, CAREERS, &s, 0);
            assert!(
                !crate::career::CAPITAL_CAREERS.contains(&m.career.as_str()),
                "{} seeded {}",
                i,
                m.career
            );
        }
    }
}

#[test]
fn a_starting_bot_needs_longer_than_a_server_lifetime_to_afford_a_capital() {
    // The lane note's arithmetic, with the careers' own priors: even the
    // cheapest hull on the order of a thousand times the starting wallet.
    let wallet = 1_000_000.0;
    let hull = 700_000_000.0;
    let best_prior = CAREERS
        .iter()
        .map(|c| c.prior_isk_h)
        .fold(0.0_f64, f64::max);
    let hours = hours_to_afford(wallet, hull, best_prior).expect("it earns");
    assert!(hours > 24.0 * 365.0, "{hours} h");
    assert_eq!(hours_to_afford(hull, hull, 0.0), Some(0.0));
    assert_eq!(hours_to_afford(wallet, hull, 0.0), None);
}

#[test]
fn the_cheapest_hull_is_read_off_the_price_list() {
    let mut c = eve_server::content::Content::fixture();
    let rows = format!(
        "{}type\t990700485\t485\t6\t1\t0\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Dread\n\
         type\t990700486\t485\t6\t1\t0\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Dread Two\n\
         type\t990700513\t513\t6\t1\t0\t1.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Freighter\n",
        eve_server::typedata::FIXTURE
    );
    let table = eve_server::typedata::Table::parse(&rows).expect("rows parse");
    c.insert("typedata", eve_server::typedata::Installed::build(table));
    let prices = [
        (990_700_485, 3.0e9),
        (990_700_486, 2.0e9),
        (990_700_513, 1.0e9),
        (12, 5.0),
    ];
    assert_eq!(cheapest_hull(&c, &JUMPER_GROUPS, &prices), Some(2.0e9));
    assert_eq!(cheapest_hull(&c, &FREIGHTER_GROUPS, &prices), Some(1.0e9));
    assert_eq!(cheapest_hull(&c, &JUMPER_GROUPS, &[]), None);
    let mut types = hull_types(&c, &JUMPER_GROUPS);
    types.sort_unstable();
    assert_eq!(types, vec![990_700_485, 990_700_486]);
}

// ---- fuel -----------------------------------------------------------------------

#[test]
fn a_jump_costs_the_servers_floor_of_the_product_and_at_least_one() {
    assert_eq!(fuel_for(3.2504, 1_000.0), 3_250);
    assert_eq!(fuel_for(0.0001, 10.0), 1);
}

#[test]
fn the_pilot_buys_the_legs_plus_the_margin_less_what_it_holds() {
    let need = fuel_for(3.0, 1_000.0) as f64 * FUEL_LEGS as f64 * (1.0 + FUEL_MARGIN);
    let want = need.ceil() as i64;
    assert_eq!(fuel_to_buy(0, 3.0, 1_000.0, FUEL_LEGS), want);
    assert_eq!(fuel_to_buy(1_000, 3.0, 1_000.0, FUEL_LEGS), want - 1_000);
    assert_eq!(fuel_to_buy(want, 3.0, 1_000.0, FUEL_LEGS), 0);
    assert_eq!(fuel_to_buy(want * 5, 3.0, 1_000.0, FUEL_LEGS), 0);
}

#[test]
fn the_fuel_budget_is_a_share_of_the_wallet_and_never_negative() {
    assert_eq!(fuel_budget(1_000.0), 1_000.0 * FUEL_WALLET_SHARE);
    assert_eq!(fuel_budget(-5.0), 0.0);
}

// ---- the destination -------------------------------------------------------------

fn cand(system: i64, ly: f64, security: f64, gates: Option<u32>) -> Candidate {
    Candidate {
        system,
        ly,
        security,
        gates,
    }
}

#[test]
fn the_target_is_low_or_null_inside_the_range_and_the_longest_walk() {
    let range = 5.0;
    let cands = [
        cand(30_990_010, 2.0, 0.9, Some(5)),                  // high sec
        cand(30_990_011, 2.0, 0.45, Some(5)),                 // rounds up to high sec
        cand(30_990_012, 2.0, -0.3, Some(2)),                 // fits, short walk
        cand(30_990_013, 4.0, -0.5, Some(4)),                 // fits, longer walk
        cand(30_990_014, range, -0.5, Some(6)),               // beyond RANGE_USE of the range
        cand(30_990_015, 3.0, -0.5, None),                    // no route
        cand(30_990_016, 3.0, -0.5, Some(ALT_MAX_JUMPS + 1)), // too far to fly
        cand(HOME, 1.0, -0.5, Some(1)),                       // home itself
    ];
    assert_eq!(choose_dest(HOME, &cands, range), Some(30_990_013));
    assert_eq!(choose_dest(HOME, &[], range), None);
    assert_eq!(choose_dest(HOME, &cands[..2], range), None);
}

#[test]
fn a_tie_in_the_walk_goes_to_the_nearer_system_then_the_lower_id() {
    let cands = [
        cand(30_990_021, 3.0, -0.5, Some(3)),
        cand(30_990_022, 2.0, -0.5, Some(3)),
        cand(30_990_020, 2.0, -0.5, Some(3)),
    ];
    assert_eq!(choose_dest(HOME, &cands, 5.0), Some(30_990_020));
}

#[test]
fn candidates_are_read_from_the_centres_and_the_map() {
    let ly = eve_server::celestial::LIGHTYEAR;
    let centers = Centers::parse(&[
        format!("s\t{HOME}\t0.0\t0.0\t0.0"),
        format!("s\t{FAR}\t{}\t0.0\t0.0", 2.0 * ly),
        format!("s\t{NEAR}\t{}\t0.0\t0.0", 40.0 * ly),
    ])
    .expect("centres");
    let map = Map::new()
        .with_gate(HOME, 1, FAR, 2)
        .with_gate(FAR, 2, HOME, 1)
        .with_security(FAR, -0.4)
        .with_security(HOME, -0.2)
        .with_security(NEAR, -0.2);
    let got = candidates(HOME, &centers, &map, 5.0);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].system, FAR);
    assert!((got[0].ly - 2.0).abs() < 1e-9);
    assert_eq!(got[0].gates, Some(1));
    assert_eq!(choose_dest(HOME, &got, 5.0), Some(FAR));
}

#[test]
fn the_leg_goes_away_from_home_and_then_back() {
    assert_eq!(leg_target(HOME, HOME, FAR), FAR);
    assert_eq!(leg_target(FAR, HOME, FAR), HOME);
    assert_eq!(leg_target(NEAR, HOME, FAR), HOME, "lost: go home");
}

// ---- fatigue ----------------------------------------------------------------------

#[test]
fn a_clean_pair_waits_for_nothing_and_a_hop_sets_both_timers() {
    let now = Instant::now();
    let p = Pace::default();
    assert_eq!(p.wait_for(now, 3.0), Duration::ZERO);
    let after = p.after_jump(now, 3.0);
    assert_eq!(
        after.fatigue_s,
        fatigue::fatigue_after(0.0, 3.0, ASSUMED_MULTIPLIER)
    );
    assert_eq!(
        after.activation_s,
        fatigue::activation_after(0.0, 3.0, ASSUMED_MULTIPLIER)
    );
    assert!(after.fatigue_s > 0.0 && after.activation_s > 0.0);
    // Straight after, the wait is the larger of the activation timer and the
    // time for the next hop's fatigue to fall to the ceiling.
    let next = fatigue::fatigue_after(after.fatigue_s, 3.0, ASSUMED_MULTIPLIER);
    let want = after.activation_s.max(next - FATIGUE_CEILING_S);
    let wait = after.wait_for(now, 3.0);
    assert!((wait.as_secs_f64() - want).abs() < 1e-6, "{wait:?} {want}");
    // A short hop leaves the activation timer the binding one.
    let short = Pace::default().after_jump(now, 0.5);
    assert!(
        (short.wait_for(now, 0.5).as_secs_f64()
            - short.activation_s.max(
                fatigue::fatigue_after(short.fatigue_s, 0.5, ASSUMED_MULTIPLIER)
                    - FATIGUE_CEILING_S
            ))
        .abs()
            < 1e-6
    );
}

#[test]
fn the_timers_run_down_in_real_time() {
    let t0 = Instant::now();
    let p = Pace {
        fatigue_s: 1_000.0,
        activation_s: 300.0,
        set_at: Some(t0),
    };
    let (f, a) = p.remaining(t0 + Duration::from_secs(100));
    assert_eq!((f, a), (900.0, 200.0));
    let (f, a) = p.remaining(t0 + Duration::from_secs(10_000));
    assert_eq!((f, a), (0.0, 0.0));
    assert_eq!(
        p.wait_for(t0 + Duration::from_secs(10_000), 1.0),
        Duration::ZERO
    );
}

#[test]
fn a_pair_over_the_fatigue_ceiling_rests_until_the_next_hop_would_fit() {
    let t0 = Instant::now();
    let high = Pace {
        fatigue_s: FATIGUE_CEILING_S * 3.0,
        activation_s: 0.0,
        set_at: Some(t0),
    };
    let wait = high.wait_for(t0, 1.0).as_secs_f64();
    let left_after = fatigue::fatigue_after(high.fatigue_s, 1.0, ASSUMED_MULTIPLIER);
    assert!(
        (wait - (left_after - FATIGUE_CEILING_S)).abs() < 1e-6,
        "{wait}"
    );
    // Waiting that long leaves a hop that lands on the ceiling.
    let later = t0 + Duration::from_secs_f64(wait);
    assert!(high.wait_for(later, 1.0) <= Duration::from_secs(1) + Duration::from_secs_f64(wait));
}

#[test]
fn the_pilots_timers_read_off_the_tuple() {
    let now = 133_000_000_000_000_000_i64;
    let v = PyValue::Tuple(vec![
        PyValue::Int(now + 30 * 10_000_000),
        PyValue::Int(now + 600 * 10_000_000),
        PyValue::Int(now),
    ]);
    let t = Timers::from_value(&v).expect("a 3-tuple");
    assert_eq!(t.activation_wait(now), Duration::from_secs(30));
    assert_eq!(t.activation_wait(now + 60 * 10_000_000), Duration::ZERO);
    assert!(Timers::from_value(&PyValue::None).is_none());
    assert!(Timers::from_value(&PyValue::Tuple(vec![PyValue::None])).is_none());
}

// ---- the pilot's turn ---------------------------------------------------------------

fn fuel() -> Fuel {
    Fuel {
        isotope: 990_700_900,
        range_ly: 5.0,
        per_ly: 1_000.0,
    }
}

fn lit(system: i64) -> Beacon {
    Beacon {
        char_id: ALT,
        system,
        ball: 9_800_000_000_007,
        type_id: 21_094,
    }
}

fn pilot_at_home() -> CapFacts {
    CapFacts {
        docked: false,
        system: HOME,
        beacon: Some(lit(FAR)),
        ly: Some(3.0),
        fuel: Some(fuel()),
        held: fuel_for(3.0, 1_000.0),
        timer: Duration::ZERO,
        home: Some(HOME),
    }
}

#[test]
fn a_pilot_with_a_field_in_range_fuel_and_no_timer_jumps() {
    assert_eq!(cap_turn(&pilot_at_home()), CapStep::Jump(lit(FAR)));
}

#[test]
fn the_pilot_refuses_in_the_servers_order() {
    let base = pilot_at_home();
    // Unreadable drive: nothing can be done.
    assert!(matches!(
        cap_turn(&CapFacts { fuel: None, ..base }),
        CapStep::Missing(_)
    ));
    // A field in its own system is no destination.
    assert!(matches!(
        cap_turn(&CapFacts {
            beacon: Some(lit(HOME)),
            ..base
        }),
        CapStep::Wait(_)
    ));
    // No distance: no centres.
    assert!(matches!(
        cap_turn(&CapFacts { ly: None, ..base }),
        CapStep::Missing(_)
    ));
    // Out of range: its drive's, read off the hull.
    assert!(matches!(
        cap_turn(&CapFacts {
            ly: Some(fuel().range_ly + 0.1),
            ..base
        }),
        CapStep::Missing(_)
    ));
    // Short of fuel for this jump (even when range and timer are fine).
    assert!(matches!(
        cap_turn(&CapFacts {
            held: fuel_for(3.0, 1_000.0) - 1,
            ..base
        }),
        CapStep::Missing(_)
    ));
    // The timer runs: wait, do not burn the alt's ozone for nothing.
    assert!(matches!(
        cap_turn(&CapFacts {
            timer: Duration::from_secs(5),
            ..base
        }),
        CapStep::Wait(_)
    ));
    // The edge of the range is inclusive, as the server's.
    assert_eq!(
        cap_turn(&CapFacts {
            ly: Some(fuel().range_ly),
            held: fuel_for(fuel().range_ly, 1_000.0),
            ..base
        }),
        CapStep::Jump(lit(FAR))
    );
}

#[test]
fn a_docked_pilot_with_no_field_stocks_its_bay_first() {
    let f = CapFacts {
        docked: true,
        beacon: None,
        ly: None,
        held: 10,
        ..pilot_at_home()
    };
    let CapStep::Stock(n) = cap_turn(&f) else {
        panic!("a short bay stocks")
    };
    assert_eq!(n, fuel_to_buy(10, fuel().range_ly, 1_000.0, FUEL_LEGS));
    // A full bay settles at home.
    let full = CapFacts { held: n + 10, ..f };
    assert_eq!(cap_turn(&full), CapStep::Settle);
}

#[test]
fn the_pilot_with_no_field_settles_at_home_and_waits_elsewhere() {
    let at_home = CapFacts {
        beacon: None,
        ly: None,
        ..pilot_at_home()
    };
    assert_eq!(cap_turn(&at_home), CapStep::Settle);
    let away = CapFacts {
        system: FAR,
        ..at_home
    };
    assert!(matches!(cap_turn(&away), CapStep::Wait(_)));
}

// ---- the alt's turn --------------------------------------------------------------------

fn alt_ready() -> AltFacts {
    AltFacts {
        docked: true,
        system: HOME,
        partnered: true,
        has_cyno: true,
        lit: false,
        ozone_held: 2_000,
        ozone_needed: 1_000,
        target: Some(FAR),
        pace_wait: Duration::ZERO,
    }
}

#[test]
fn the_alt_goes_to_the_target_when_everything_is_ready() {
    assert_eq!(alt_turn(&alt_ready()), AltStep::Leg(FAR));
}

#[test]
fn the_alt_forms_the_fleet_stocks_ozone_and_rests_in_that_order() {
    let a = alt_ready();
    assert!(matches!(
        alt_turn(&AltFacts {
            has_cyno: false,
            ..a
        }),
        AltStep::Missing(_)
    ));
    assert_eq!(
        alt_turn(&AltFacts {
            partnered: false,
            ozone_held: 0,
            ..a
        }),
        AltStep::Form
    );
    assert_eq!(
        alt_turn(&AltFacts {
            ozone_held: 300,
            ..a
        }),
        AltStep::Stock(700)
    );
    // Not docked: nothing to buy with; it goes with what it has.
    assert_eq!(
        alt_turn(&AltFacts {
            docked: false,
            ozone_held: 300,
            ..a
        }),
        AltStep::Leg(FAR)
    );
    assert_eq!(
        alt_turn(&AltFacts {
            pace_wait: Duration::from_secs(90),
            ..a
        }),
        AltStep::Rest(Duration::from_secs(90))
    );
    assert!(matches!(
        alt_turn(&AltFacts { target: None, ..a }),
        AltStep::Missing(_)
    ));
    assert_eq!(alt_turn(&AltFacts { lit: true, ..a }), AltStep::Hold);
}

// ---- the roles ----------------------------------------------------------------------------

fn memory(role: &'static RoleSpec, partner: Option<i64>) -> Memory {
    let mut p = Policy::human();
    p.role = role;
    p.partner = partner;
    Memory::new("Invented Capital", p)
}

fn obs(place: Place) -> Observation {
    let mut o = Observation::empty(Instant::now());
    o.place = place;
    o
}

#[test]
fn a_pair_member_without_a_partner_says_so_and_is_never_given_kit() {
    for role in [&ROLE, &ALT_ROLE] {
        let mut m = memory(role, None);
        let acts = (role.docked)(
            &mut m,
            &obs(Place::Docked {
                station: 60_990_001,
            }),
        );
        assert!(matches!(acts.first(), Some(Act::Missing(w)) if *w == MISSING_PARTNER));
        assert!(
            !acts
                .iter()
                .any(|a| matches!(a, Act::GmSetup { .. } | Act::Acquire { .. }))
        );
        assert!(
            (role.setup)(Lacks {
                ship: true,
                module: true,
                ammo: true,
                isk: true
            })
            .is_empty()
        );
    }
}

#[test]
fn a_paired_member_takes_a_turn_when_due_then_waits() {
    let mut m = memory(&ROLE, Some(ALT));
    let o = obs(Place::Docked {
        station: 60_990_001,
    });
    let acts = (ROLE.docked)(&mut m, &o);
    assert_eq!(acts.first(), Some(&Act::Capital));
    assert!(matches!(acts.get(1), Some(Act::Wait(_))));
    let again = (ROLE.docked)(&mut m, &o);
    assert!(matches!(again.as_slice(), [Act::Wait(_)]));
}

#[test]
fn the_roles_are_in_the_table_the_alt_is_told_apart_and_the_kit_is_empty() {
    assert!(crate::roles::parse("capital").is_some());
    assert!(crate::roles::parse("cyno-alt").is_some());
    assert!(crate::roles::parse("freighter").is_some());
    assert!(is_alt(&ALT_ROLE) && !is_alt(&ROLE));
    for role in [&ROLE, &ALT_ROLE, &crate::hauler::ROLE] {
        let kit = crate::acquire::kit_of(role);
        assert_eq!(kit.hull, None);
        assert!(kit.modules.is_none() && kit.ammo.is_none());
    }
}

#[test]
fn a_failing_pair_member_reconnects_and_in_space_it_needs_a_park() {
    let mut m = memory(&ROLE, Some(ALT));
    let mut o = obs(Place::Docked { station: 1 });
    o.failures.insert(ActKind::Capital, brain::GIVE_UP_AFTER);
    assert_eq!((ROLE.docked)(&mut m, &o), vec![Act::Reconnect]);
    let o = obs(Place::InSpace {
        bound: true,
        system: HOME,
    });
    assert!(matches!(
        (ROLE.in_space)(&mut m, &o).as_slice(),
        [Act::Wait(_)]
    ));
}
