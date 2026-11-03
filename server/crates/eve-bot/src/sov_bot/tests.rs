//! Pure tests of the planner, the schedule and the hall. Ids are invented;
//! no [A] constant appears as an expected value, only by name.

use super::*;
use eve_server::pyutil::keyval;

const ME: i64 = 99_000_001;
const FOE: i64 = 99_000_002;
const FRIEND: i64 = 99_000_003;
const NOW: i64 = 1_000_000_000_000;
const HOME: i64 = 30_900_001;

fn hub(system: i64, holder: Option<i64>, phase: Phase) -> HubView {
    HubView {
        system,
        hub_id: 5_000_000 + system,
        holder,
        adm: 1.0,
        phase,
        campaign: None,
        contesters: Vec::new(),
    }
}

fn open() -> Phase {
    Phase::Open { closes: NOW + 100 }
}

fn shut_in(secs: i64) -> Phase {
    Phase::Shut {
        opens: NOW + secs * TICKS,
    }
}

fn near(_: i64) -> Option<u32> {
    Some(1)
}

fn far(_: i64) -> Option<u32> {
    Some(3)
}

fn neutral(_: i64) -> f64 {
    0.0
}

fn foe_red(a: i64) -> f64 {
    if a == FOE { -0.9 } else { 0.0 }
}

fn never(_: i64) -> bool {
    false
}

fn facts<'a>(
    hubs: &'a [HubView],
    jumps: &'a dyn Fn(i64) -> Option<u32>,
    relation: &'a dyn Fn(i64) -> f64,
    linkers: usize,
    previous: Option<&'a Target>,
) -> Facts<'a> {
    Facts {
        me: ME,
        hubs,
        jumps,
        relation,
        bloc_enemy: &never,
        bloc_mate: &never,
        now: NOW,
        linkers,
        previous,
    }
}

#[test]
fn a_phase_is_read_from_the_rows_fields_at_now() {
    assert_eq!(phase_of(None, None, None, NOW), Phase::Neutral);
    assert_eq!(
        phase_of(Some(ME), None, Some((NOW - 10, NOW + 10)), NOW),
        Phase::Open { closes: NOW + 10 }
    );
    assert_eq!(
        phase_of(Some(ME), None, Some((NOW + 10, NOW + 20)), NOW),
        Phase::Shut { opens: NOW + 10 }
    );
    // Overtime: the window has ended and the row still shows it.
    assert!(matches!(
        phase_of(Some(ME), None, Some((NOW - 20, NOW - 10)), NOW),
        Phase::Open { .. }
    ));
    assert_eq!(
        phase_of(Some(ME), Some(NOW + 5), Some((NOW - 20, NOW + 10)), NOW),
        Phase::Reinforced { fight_at: NOW + 5 }
    );
    assert_eq!(
        phase_of(Some(ME), Some(NOW - 5), None, NOW),
        Phase::NodeFight
    );
    assert_eq!(phase_of(Some(ME), None, None, NOW), Phase::Unknown);
}

#[test]
fn a_structure_row_is_read_as_the_client_gets_it() {
    let row = keyval(vec![
        ("itemID", PyValue::Int(7_001)),
        ("typeID", PyValue::Int(TYPE_INFRASTRUCTURE_HUB)),
        ("allianceID", PyValue::Int(FOE)),
        ("defenseMultiplier", PyValue::Real(2.5)),
        ("campaignState", PyValue::None),
        (
            "vulnerabilityState",
            PyValue::Tuple(vec![PyValue::Int(NOW - 1), PyValue::Int(NOW + 1)]),
        ),
    ]);
    let h = HubView::from_row(HOME, &row, NOW).expect("a hub");
    assert_eq!((h.hub_id, h.holder, h.adm), (7_001, Some(FOE), 2.5));
    assert_eq!(h.phase, Phase::Open { closes: NOW + 1 });
    let neutral_row = keyval(vec![
        ("itemID", PyValue::Int(7_002)),
        ("typeID", PyValue::Int(TYPE_INFRASTRUCTURE_HUB)),
        ("allianceID", PyValue::None),
        ("campaignState", PyValue::None),
        ("vulnerabilityState", PyValue::None),
    ]);
    let n = HubView::from_row(HOME, &neutral_row, NOW).expect("a hub");
    assert_eq!((n.holder, n.phase, n.adm), (None, Phase::Neutral, 1.0));
    let fight = keyval(vec![
        ("itemID", PyValue::Int(7_003)),
        ("allianceID", PyValue::Int(ME)),
        (
            "campaignState",
            PyValue::Tuple(vec![
                PyValue::Int(2),
                PyValue::Int(ME),
                PyValue::Int(NOW - 5),
                PyValue::Dict(vec![]),
            ]),
        ),
    ]);
    assert_eq!(
        HubView::from_row(HOME, &fight, NOW).map(|h| h.phase),
        Some(Phase::NodeFight)
    );
    // Not a hub: another type of structure row.
    let tcu = keyval(vec![
        ("itemID", PyValue::Int(7_004)),
        ("typeID", PyValue::Int(TYPE_INFRASTRUCTURE_HUB + 1)),
    ]);
    assert_eq!(HubView::from_row(HOME, &tcu, NOW), None);
}

#[test]
fn the_planner_claims_the_nearer_neutral_hub() {
    let hubs = [
        hub(1, None, Phase::Neutral),
        hub(2, None, Phase::Neutral),
        hub(3, None, Phase::Neutral),
    ];
    let jumps = |s: i64| Some(if s == 2 { 1 } else { 4 });
    let t = plan(&facts(&hubs, &jumps, &neutral, 1, None)).expect("a claim");
    assert_eq!((t.system, t.kind), (2, Kind::Claim));
}

#[test]
fn a_claim_needs_a_link_in_the_alliance() {
    let hubs = [hub(1, None, Phase::Neutral)];
    assert_eq!(plan(&facts(&hubs, &near, &neutral, 0, None)), None);
    assert!(plan(&facts(&hubs, &near, &neutral, 1, None)).is_some());
}

#[test]
fn a_system_with_no_hub_is_never_a_target() {
    // Nothing in the view is a hub-less system: the planner is shown hubs
    // only, and a held hub of an ally or a neutral holder is left alone.
    let hubs = [
        hub(1, Some(FRIEND), open()),
        hub(2, Some(99_000_009), open()),
    ];
    assert_eq!(plan(&facts(&hubs, &near, &neutral, 5, None)), None);
    assert_eq!(plan(&facts(&[], &near, &neutral, 5, None)), None);
}

#[test]
fn an_enemy_hub_is_attacked_only_when_its_window_is_due() {
    let open_now = [hub(1, Some(FOE), open())];
    let t = plan(&facts(&open_now, &near, &foe_red, 3, None)).expect("an attack");
    assert_eq!((t.kind, t.holder), (Kind::Attack, Some(FOE)));
    // Shut for an hour: wait. Shut for under the lead time: go.
    let later = [hub(1, Some(FOE), shut_in(3_600))];
    assert_eq!(plan(&facts(&later, &near, &foe_red, 3, None)), None);
    let soon = [hub(1, Some(FOE), shut_in(LEAD_SECS - 1))];
    assert!(plan(&facts(&soon, &near, &foe_red, 3, None)).is_some());
    // A neutral holder is not an enemy.
    assert_eq!(plan(&facts(&open_now, &near, &neutral, 3, None)), None);
}

#[test]
fn an_attack_needs_the_force_its_adm_asks_for() {
    let mut h = hub(1, Some(FOE), open());
    h.adm = 4.0;
    let hubs = [h];
    assert_eq!(linkers_wanted(4.0), 4);
    assert_eq!(plan(&facts(&hubs, &near, &foe_red, 3, None)), None);
    assert!(plan(&facts(&hubs, &near, &foe_red, 4, None)).is_some());
    assert_eq!(linkers_wanted(0.0), 1, "never fewer than one");
    assert_eq!(
        linkers_wanted(1_000.0),
        LINKERS_MAX,
        "never more than the cap"
    );
}

#[test]
fn a_reinforced_enemy_is_not_attacked_until_its_node_fight() {
    let reinforced = [hub(1, Some(FOE), Phase::Reinforced { fight_at: NOW + 1 })];
    assert_eq!(plan(&facts(&reinforced, &near, &foe_red, 3, None)), None);
    let fight = [hub(1, Some(FOE), Phase::NodeFight)];
    let t = plan(&facts(&fight, &near, &foe_red, 3, None)).expect("the fight");
    assert_eq!(t.kind, Kind::Attack);
}

#[test]
fn defence_comes_before_any_claim_or_attack() {
    let hubs = [
        hub(1, None, Phase::Neutral),
        hub(2, Some(FOE), open()),
        hub(3, Some(ME), Phase::NodeFight),
    ];
    let t = plan(&facts(&hubs, &near, &foe_red, 4, None)).expect("a defence");
    assert_eq!((t.system, t.kind), (3, Kind::Defend));
    // Reinforced waits for its lead time, then is defended too.
    let early = [hub(
        3,
        Some(ME),
        Phase::Reinforced {
            fight_at: NOW + 3_600 * TICKS,
        },
    )];
    assert_eq!(plan(&facts(&early, &near, &neutral, 4, None)), None);
    let due_soon = [hub(
        3,
        Some(ME),
        Phase::Reinforced {
            fight_at: NOW + TICKS,
        },
    )];
    assert_eq!(
        plan(&facts(&due_soon, &near, &neutral, 4, None)).map(|t| t.kind),
        Some(Kind::Defend)
    );
    // An own hub in its ordinary window is nothing to plan for.
    let calm = [hub(3, Some(ME), open())];
    assert_eq!(plan(&facts(&calm, &near, &neutral, 4, None)), None);
}

#[test]
fn a_worse_relation_and_a_richer_adm_make_a_better_target_and_distance_a_worse_one() {
    let mut rich = hub(1, Some(FOE), open());
    rich.adm = 3.0;
    let poor = hub(2, Some(FOE), open());
    let hubs = [poor.clone(), rich.clone()];
    let t = plan(&facts(&hubs, &near, &foe_red, 6, None)).expect("a target");
    assert_eq!(t.system, 1, "the richer hub");
    let jumps = |s: i64| Some(if s == 1 { 4 } else { 1 });
    let t = plan(&facts(&hubs, &jumps, &foe_red, 6, None)).expect("a target");
    assert!(t.score > 0.0);
    let by_far = plan(&facts(&[rich], &far, &foe_red, 6, None)).expect("a target");
    let by_near = plan(&facts(&[poor], &near, &foe_red, 6, None)).expect("a target");
    assert!(by_far.score != by_near.score);
}

#[test]
fn the_running_campaign_is_kept_against_a_slightly_better_one() {
    let hubs = [hub(1, None, Phase::Neutral), hub(2, None, Phase::Neutral)];
    let jumps = |s: i64| Some(if s == 2 { 1 } else { 2 });
    let first = plan(&facts(&hubs, &jumps, &neutral, 1, None)).expect("a claim");
    assert_eq!(first.system, 2);
    // The other hub is now nearer by a hair; the campaign stays.
    let jumps2 = |s: i64| Some(if s == 1 { 1 } else { 2 });
    let kept = plan(&facts(&hubs, &jumps2, &neutral, 1, Some(&first))).expect("a claim");
    assert_eq!(kept.system, 2);
    let moved = plan(&facts(&hubs, &jumps2, &neutral, 1, None)).expect("a claim");
    assert_eq!(moved.system, 1, "without the stick it would move");
}

#[test]
fn a_hub_beyond_the_view_is_costly_but_known() {
    let hubs = [hub(9, None, Phase::Neutral)];
    let unknown = |_: i64| None;
    let t = plan(&facts(&hubs, &unknown, &neutral, 1, None));
    assert_eq!(t.map(|t| t.system), Some(9));
}

#[test]
fn due_schedules_by_the_lead_time() {
    assert!(due(Phase::Neutral, NOW));
    assert!(due(open(), NOW));
    assert!(due(Phase::NodeFight, NOW));
    assert!(due(shut_in(LEAD_SECS), NOW));
    assert!(!due(shut_in(LEAD_SECS + 1), NOW));
    assert!(!due(Phase::Unknown, NOW));
    assert!(!due(
        Phase::Reinforced {
            fight_at: NOW + (LEAD_SECS + 1) * TICKS
        },
        NOW
    ));
}

#[test]
fn an_alliances_prime_hour_is_its_own_and_in_range() {
    let hours: BTreeSet<i64> = (0..200).map(|k| prime_hour_for(99_000_000 + k)).collect();
    assert!(hours.iter().all(|h| (0..24).contains(h)));
    assert!(
        hours.len() >= 12,
        "ids spread over the day: {}",
        hours.len()
    );
    assert_eq!(
        prime_hour_for(ME),
        prime_hour_for(ME),
        "a function of the id"
    );
}

#[test]
fn fuel_runs_list_only_own_hubs() {
    let hubs = [
        hub(1, Some(ME), open()),
        hub(2, Some(FOE), open()),
        hub(3, None, Phase::Neutral),
        hub(4, Some(ME), Phase::NodeFight),
    ];
    assert_eq!(
        fuel_runs(ME, &hubs),
        vec![(1, hubs[0].hub_id), (4, hubs[3].hub_id)]
    );
}

fn plan_with(kind: Kind, fuel: &[(i64, i64)]) -> Plan {
    Plan {
        target: Some(Target {
            system: 1,
            hub_id: 11,
            kind,
            holder: None,
            phase: Phase::Neutral,
            score: 1.0,
        }),
        fuel: fuel.to_vec(),
    }
}

fn kit(link: bool, guns: bool, hauler: bool) -> Kit {
    Kit { link, guns, hauler }
}

#[test]
fn a_claim_sends_one_link_and_guards_and_haulers_take_the_rest() {
    let roster: BTreeMap<i64, Kit> = [
        (1, kit(true, true, false)),
        (2, kit(true, true, false)),
        (3, kit(false, true, true)),
        (4, kit(false, false, true)),
    ]
    .into_iter()
    .collect();
    let d = assign(&plan_with(Kind::Claim, &[(8, 80)]), &roster);
    assert_eq!(d[&1], Duty::Entosis);
    assert_eq!(d[&2], Duty::Idle, "a claim needs one link");
    assert_eq!(d[&3], Duty::Guard);
    assert_eq!(d[&4], Duty::Haul(80));
}

#[test]
fn an_attack_sends_every_link_up_to_the_cap() {
    let roster: BTreeMap<i64, Kit> = (1..=10).map(|k| (k, kit(true, false, false))).collect();
    let d = assign(&plan_with(Kind::Attack, &[]), &roster);
    assert_eq!(
        d.values().filter(|x| **x == Duty::Entosis).count(),
        LINKERS_MAX
    );
}

#[test]
fn guards_are_capped_and_haulers_share_the_hubs_round_robin() {
    let roster: BTreeMap<i64, Kit> = (1..=8).map(|k| (k, kit(false, true, true))).collect();
    let d = assign(&plan_with(Kind::Defend, &[(8, 80), (9, 90)]), &roster);
    assert_eq!(
        d.values().filter(|x| **x == Duty::Guard).count(),
        GUARDS_MAX
    );
    let hauls: Vec<Duty> = d
        .values()
        .copied()
        .filter(|x| matches!(x, Duty::Haul(_)))
        .collect();
    assert_eq!(hauls.len(), 8 - GUARDS_MAX);
    assert!(hauls.contains(&Duty::Haul(80)) && hauls.contains(&Duty::Haul(90)));
}

#[test]
fn no_campaign_means_haulers_only_and_no_hubs_means_idle() {
    let roster: BTreeMap<i64, Kit> = [(1, kit(true, true, false)), (2, kit(false, false, true))]
        .into_iter()
        .collect();
    let none = Plan {
        target: None,
        fuel: vec![(8, 80)],
    };
    let d = assign(&none, &roster);
    assert_eq!((d[&1], d[&2]), (Duty::Idle, Duty::Haul(80)));
    let nothing = Plan::default();
    let d = assign(&nothing, &roster);
    assert_eq!((d[&1], d[&2]), (Duty::Idle, Duty::Idle));
}

#[test]
fn the_hall_plans_once_a_round_and_hands_out_the_duties() {
    let hall = SovHall::new();
    let t0 = Instant::now();
    hall.report(ME, 1, kit(true, true, false), t0);
    hall.report(ME, 2, kit(false, true, true), t0);
    hall.report(FOE, 9, kit(true, false, false), t0);
    assert_eq!(hall.linkers(ME), 1);
    assert!(hall.take_planning(ME, t0));
    assert!(!hall.take_planning(ME, t0), "one planner a round");
    assert!(hall.take_planning(ME, t0 + PLAN_EVERY));
    hall.set_plan(ME, plan_with(Kind::Claim, &[(8, 80)]));
    assert_eq!(hall.duty(ME, 1), Duty::Entosis);
    assert_eq!(hall.duty(ME, 2), Duty::Guard);
    assert_eq!(hall.duty(ME, 777), Duty::Idle, "a stranger has no duty");
    assert_eq!(
        hall.duty(FOE, 9),
        Duty::Idle,
        "another alliance has its own plan"
    );
    assert_eq!(hall.log(ME).len(), 1);
    hall.set_plan(ME, plan_with(Kind::Claim, &[(8, 80)]));
    assert_eq!(
        hall.log(ME).len(),
        1,
        "the same campaign is not logged twice"
    );
}

#[test]
fn a_silent_pilot_leaves_the_roster() {
    let hall = SovHall::new();
    let t0 = Instant::now();
    hall.report(ME, 1, kit(true, false, false), t0);
    hall.report(
        ME,
        2,
        kit(false, false, true),
        t0 + ROSTER_TTL + Duration::from_secs(1),
    );
    assert_eq!(hall.linkers(ME), 0);
}

#[test]
fn the_prime_hour_is_tried_by_a_pilot_once_in_a_while() {
    let hall = SovHall::new();
    let t0 = Instant::now();
    assert!(hall.take_prime(ME, 1, t0));
    assert!(!hall.take_prime(ME, 1, t0 + Duration::from_secs(1)));
    assert!(
        hall.take_prime(ME, 2, t0),
        "another pilot may be the director"
    );
    assert!(hall.take_prime(ME, 1, t0 + REFUEL_EVERY));
}

fn line_map() -> Map {
    // 1 - 2 - 3 - 4 - 5, all null; 6 off 1 is high sec.
    let mut m = Map::new();
    for (a, b) in [(1, 2), (2, 3), (3, 4), (4, 5), (1, 6)] {
        m = m
            .with_gate(a, 100 * a + b, b, 100 * b + a)
            .with_gate(b, 100 * b + a, a, 100 * a + b);
    }
    for s in 1..=5 {
        m = m.with_security(s, -0.4);
    }
    m.with_security(6, 0.9)
}

#[test]
fn candidates_are_the_null_systems_in_range_nearest_first() {
    let map = line_map();
    let held: BTreeMap<i64, i64> = [(4, FOE)].into_iter().collect();
    let c = candidates(&map, 1, &held);
    let ids: Vec<i64> = c.iter().map(|(s, _)| *s).collect();
    assert_eq!(
        ids,
        vec![1, 2, 3, 4, 5],
        "the high-sec neighbour is left out: {c:?}"
    );
    assert!(c.iter().all(|(_, d)| *d <= VIEW_JUMPS));
}

#[test]
fn a_node_tour_starts_at_the_hub_and_reaches_the_gates_neighbours() {
    let map = line_map();
    let t = tour(&map, 3);
    assert_eq!(t.first(), Some(&3));
    assert!(t.contains(&2) && t.contains(&4) && t.contains(&1) && t.contains(&5));
    assert!(!t.contains(&6) || NODE_TOUR_JUMPS >= 3);
}

#[test]
fn the_role_gives_nothing_and_needs_a_hall() {
    assert!(
        setup(Lacks {
            ship: true,
            module: true,
            ammo: true,
            isk: true
        })
        .is_empty()
    );
    assert_eq!(crate::roles::parse("sov").map(|r| r.name), Some("sov"));
    let mut m = Memory::new("Invented Sov", brain::Policy::human());
    m.policy.role = &ROLE;
    let mut o = Observation::empty(Instant::now());
    o.place = brain::Place::Docked { station: 9 };
    let acts = docked(&mut m, &o);
    assert!(matches!(acts.first(), Some(Act::Missing(_))), "{acts:?}");
    m.policy.sov = Some(std::sync::Arc::new(SovHall::new()));
    let acts = docked(&mut m, &o);
    assert_eq!(acts.first(), Some(&Act::Sov), "{acts:?}");
}

#[test]
fn the_role_never_sends_a_slash_or_a_given_item() {
    // Nothing from nowhere: every act the role's docked rule can return is
    // a sov turn, a wait, a reconnect, a missing note or the escort's.
    let mut m = Memory::new("Invented Sov", brain::Policy::human());
    m.policy.role = &ROLE;
    m.policy.sov = Some(std::sync::Arc::new(SovHall::new()));
    let mut o = Observation::empty(Instant::now());
    o.place = brain::Place::Docked { station: 9 };
    let acts = docked(&mut m, &o);
    assert!(!acts.iter().any(|a| matches!(a, Act::GmSetup { .. })));
}

// ---- R-REL-2: the defender tells the society -------------------------------------------

fn under_siege(by: &[i64]) -> HubView {
    HubView {
        campaign: Some(NOW + 5),
        contesters: by.to_vec(),
        ..hub(HOME, Some(ME), Phase::NodeFight)
    }
}

#[test]
fn the_siege_is_seen_on_my_own_hub_in_a_campaign_only() {
    assert_eq!(
        sieges(&[under_siege(&[FOE])], ME),
        vec![(5_000_000 + HOME, NOW + 5, FOE)]
    );
    // Someone else's hub, or mine with the window merely open: no siege.
    assert!(sieges(&[under_siege(&[FOE])], FRIEND).is_empty());
    assert!(sieges(&[hub(HOME, Some(ME), open())], ME).is_empty());
}

#[test]
fn a_campaign_row_names_the_scoring_teams_but_not_the_holder() {
    let row = keyval(vec![
        ("itemID", PyValue::Int(5_000_001)),
        ("allianceID", PyValue::Int(ME)),
        (
            "campaignState",
            PyValue::Tuple(vec![
                PyValue::Int(1),
                PyValue::Int(ME),
                PyValue::Int(NOW),
                PyValue::Dict(vec![
                    (PyValue::Real(2.0), PyValue::Int(FOE)),
                    (PyValue::Real(9.0), PyValue::Int(ME)),
                    (PyValue::Real(0.0), PyValue::Int(FRIEND)),
                    (PyValue::Real(5.0), PyValue::Int(-ME)),
                ]),
            ]),
        ),
    ]);
    let v = HubView::from_row(HOME, &row, NOW - 1).expect("a hub");
    assert_eq!(v.contesters, vec![FOE]);
    assert_eq!(v.campaign, Some(NOW));
}

#[test]
fn the_siege_event_is_emitted_by_the_defender_once_per_campaign() {
    let hall = crate::corpmind::Hall::new(60);
    {
        let mut st = hall.lock();
        st.corps.insert(
            ME,
            crate::corpmind::CorpMind {
                id: ME,
                ceo: Some(1),
                profile: crate::society::Profile::of("pirate", crate::career::Band::Low),
                founded: 0,
                alliance: Some(ME),
                name: String::new(),
                applied_alliance: None,
                salt: 0,
            },
        );
        st.members.insert(
            1,
            crate::corpmind::Member {
                name: "Invented".to_owned(),
                traits: crate::mind::Traits([0.5; crate::mind::TRAIT_COUNT]),
                career: "pirate".to_owned(),
                band: crate::career::Band::Low,
                corp: ME,
                alliance: None,
                last_turn: None,
                satisfaction: crate::society::Satisfaction::default(),
                applied: None,
                salt: 0,
            },
        );
    }
    let hubs = [under_siege(&[FOE])];
    tell_society(&hall, ME, &hubs, None);
    let after = hall.lock().relations.get(ME, FOE);
    assert!(after < 0.0, "the defender holds a grudge: {after}");
    tell_society(&hall, ME, &hubs, None);
    assert_eq!(
        hall.lock().relations.get(ME, FOE),
        after,
        "once per campaign"
    );
    // A planning pass that attacks a holder adds to the contest.
    let target = Target {
        system: HOME,
        hub_id: 1,
        kind: Kind::Attack,
        holder: Some(FRIEND),
        phase: open(),
        score: 1.0,
    };
    tell_society(&hall, ME, &[], Some(&target));
    assert!(hall.lock().relations.get(ME, FRIEND) < 0.0);
}

/// R-REL-5: two holders no further off than each other, neither enemy by
/// `R` enough to matter in the score; the one the whole bloc named is the
/// campaign, and a bloc enemy is attacked even where `R` alone would leave
/// it.
#[test]
fn sov_attack_prefers_the_blocs_enemy() {
    const OTHER_FOE: i64 = 99_000_004;
    let hubs = [
        hub(HOME + 1, Some(OTHER_FOE), open()),
        hub(HOME + 2, Some(FOE), open()),
    ];
    let both = |a: i64| {
        if a == FOE || a == OTHER_FOE {
            ATTACK_R
        } else {
            0.0
        }
    };
    let foe_named = |a: i64| a == FOE;
    let mut f = facts(&hubs, &near, &both, 3, None);
    f.bloc_enemy = &foe_named;
    let t = plan(&f).expect("a campaign");
    assert_eq!(t.holder, Some(FOE), "the bloc's enemy first");
    // A bloc enemy is a target even where the own view stays above the bar.
    let calm = |_: i64| 0.0;
    let mut f = facts(&hubs, &near, &calm, 3, None);
    f.bloc_enemy = &foe_named;
    assert_eq!(plan(&f).and_then(|t| t.holder), Some(FOE));
    // Without the bloc and with a calm view, nobody is attacked.
    assert_eq!(plan(&facts(&hubs, &near, &calm, 3, None)), None);
}

/// R-REL-5: a coalition mate is never attacked, and its hub in a node fight
/// is aided by guards only -- no link goes on a mate's hub.
#[test]
fn a_mates_hub_is_aided_in_its_node_fight_and_never_attacked() {
    let hubs = [hub(HOME + 1, Some(FRIEND), Phase::NodeFight)];
    let mate = |a: i64| a == FRIEND;
    let red = |_: i64| ATTACK_R;
    let mut f = facts(&hubs, &near, &red, 6, None);
    f.bloc_mate = &mate;
    let t = plan(&f).expect("an aid");
    assert_eq!(t.kind, Kind::Aid);
    // Out of its node fight, a mate's hub is left alone even at a bad `R`.
    let shut = [hub(HOME + 1, Some(FRIEND), open())];
    let mut f = facts(&shut, &near, &red, 6, None);
    f.bloc_mate = &mate;
    assert_eq!(plan(&f), None);
    // Guards go; links do not.
    let plan_aid = Plan {
        target: Some(t),
        fuel: vec![],
    };
    let roster = BTreeMap::from([
        (
            1,
            Kit {
                link: true,
                guns: true,
                hauler: false,
            },
        ),
        (
            2,
            Kit {
                link: false,
                guns: true,
                hauler: false,
            },
        ),
    ]);
    let duties = assign(&plan_aid, &roster);
    assert_ne!(duties[&1], Duty::Entosis);
    assert_eq!(duties[&2], Duty::Guard);
}

/// REVIEW-FIX2: a haul's fuel is the demand layer's days of burn, less what
/// the ship already carries (the sortie reads `haul_units`).
#[test]
fn review_fix2_a_haul_is_sized_by_the_demand_layers_fuel_days() {
    let days = crate::demand::FE_FUEL_DAYS;
    assert_eq!(haul_units(0), (days * FUEL_PER_DAY as f64).ceil() as i64);
    assert_eq!(haul_units(10), haul_units(0) - 10);
    assert_eq!(haul_units(i64::MAX / 2), 0);
}
