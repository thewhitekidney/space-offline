//! `hackingMgr`: the hacking window's inputs (R-HACK packet HK2, lane
//! `hk2`; the spec is gitignored because it holds client ids).
//!
//! The client reaches it as `sm.RemoteSvc('hackingMgr')`, a plain global
//! service, and **discards every return value** [V `hackingUISvc`]: each arm
//! answers `None`, and the turn goes out as `OnHackingTurnComplete`
//! (`crate::hacking::TurnComplete`), with the answer.
//!
//! | Method | Args | When |
//! |---|---|---|
//! | `ClickedOnTile` | `(coord)` | a tile click with no utility selected, blocked tiles included [V] |
//! | `UsedUtilityElement` | `(index, coord)` | a tile click with Kernel Rot or Secondary Vector selected [V] |
//! | `UsedUtilityElementOnVirus` | `(index)` | a click on a Self Repair or Shield slot, or keys 1..3 [V] |
//! | `QuitHackingAttempt` | `()` | the player closes the window [V] |
//! | `StartNewGameInstance` | `(targetID, gameType, moduleTypeID, difficulty, coherence, strength, slots)` | **debug only**: the client's `/openhacking` [V] |
//!
//! Real play never calls `StartNewGameInstance`: the server opens the game
//! from the analyzer (`crate::hacking::on_completed`). It is honoured only
//! with `targetID` `None` and a GM caller, as a practice board on no can;
//! anything else is ignored.

use super::*;
use crate::hacking::{self, Coord, Input, Style, VirusStats};

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &["hackingMgr"];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Ok(PyValue::None);
    };
    match req.method {
        "ClickedOnTile" => match coord(req.args.first()) {
            Some(c) => hacking::input(reg, actor, Input::Click(c)),
            None => hacking::ack(reg, actor),
        },
        "UsedUtilityElement" => match (index(req.args.first()), coord(req.args.get(1))) {
            (Some(i), Some(c)) => hacking::input(reg, actor, Input::UseOnTile(i, c)),
            _ => hacking::ack(reg, actor),
        },
        "UsedUtilityElementOnVirus" => match index(req.args.first()) {
            Some(i) => hacking::input(reg, actor, Input::UseOnVirus(i)),
            None => hacking::ack(reg, actor),
        },
        "QuitHackingAttempt" => hacking::input(reg, actor, Input::Quit),
        "StartNewGameInstance" => start_practice(reg, actor, req.args),
        _ => return Err(reg.no_method(req)),
    }
    Ok(PyValue::None)
}

/// An `(x, y)` tuple (or list) of two integers [V: a `tileDataByCoord` key].
fn coord(v: Option<&PyValue>) -> Option<Coord> {
    let (PyValue::Tuple(xs) | PyValue::List(xs)) = v? else {
        return None;
    };
    let [PyValue::Int(x), PyValue::Int(y)] = xs.as_slice() else {
        return None;
    };
    Some((i32::try_from(*x).ok()?, i32::try_from(*y).ok()?))
}

/// A slot index, 0-based [V `hackingWindow`].
fn index(v: Option<&PyValue>) -> Option<usize> {
    match v? {
        PyValue::Int(i) => usize::try_from(*i).ok(),
        _ => None,
    }
}

fn number(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Int(i) => Some(*i as f64),
        PyValue::Real(r) => Some(*r),
        _ => None,
    }
}

/// Whether the caller may open a practice board: a server-made call, or a
/// session with `ROLE_GML` (the slash console's own gate, `service/gm.rs`).
fn is_gm(caller: Caller) -> bool {
    match caller {
        Caller::Internal => true,
        Caller::Session { role, .. } => role & crate::roles::ROLE_GML != 0,
    }
}

/// `StartNewGameInstance(targetID, gameType, moduleTypeID, difficulty,
/// virusCoherence, virusStrength, virusSlots)`: a practice board for a GM
/// when `targetID` is `None`; otherwise nothing (a real game starts from the
/// analyzer, never from the client).
fn start_practice(reg: &mut Registry, actor: i64, args: &[PyValue]) {
    if !matches!(args.first(), Some(PyValue::None)) || !is_gm(reg.caller()) {
        crate::outln!("[hack] {actor}: StartNewGameInstance ignored (a target, or not a GM)");
        return;
    }
    let style = match args.get(1) {
        Some(PyValue::Int(1)) => Style::Archeology,
        _ => Style::Hacking,
    };
    let tier = number(args.get(3)).map_or(1, |d| d.round().clamp(1.0, 4.0) as u8);
    let virus = VirusStats::from_attributes(
        number(args.get(4)).unwrap_or(1.0),
        number(args.get(5)).unwrap_or(1.0),
        number(args.get(6)).unwrap_or(0.0),
    );
    hacking::practice(reg, actor, style, tier, virus);
}

// ---------------------------------------------------------------------------
// Tests: invented ids and numbers throughout; no [A] value appears in one.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dungeon_data::{Dungeon, Entry, Object, Room};
    use crate::hacking::{
        Board, CanState, CoreLevel, EFFECT_DO_HACKING, Hacks, KEY_SECURITY_STATE, Node, NodeStats,
        Rules, SKILL_ARCHAEOLOGY,
    };
    use crate::population::{self, Site};
    use crate::site::Sites;

    const ALICE: i64 = 1_000_001;
    const MERLIN: i64 = 100_000_070;
    const ANALYZER: i64 = 100_000_071;
    /// `flagMedSlot0` [V].
    const MED_SLOT_0: i64 = 19;
    const T0: i64 = 133_000_000_000_000_000;

    /// Invented types: a data can, a relic can, a beacon and an analyzer.
    const CAN_TYPE: i64 = 990_120_701;
    const RELIC_TYPE: i64 = 990_120_702;
    const BEACON_TYPE: i64 = 990_120_703;
    const ANALYZER_TYPE: i64 = 990_538_001;
    /// An invented "Hacking" skill: the data can's and the analyzer's.
    const DATA_SKILL: i64 = 990_217_001;
    const EFFECT_NAME: &str = "fixtureHack";
    /// The analyzer's invented numbers.
    const RANGE: f64 = 4_000.0;
    const DURATION_MS: i64 = 7_000;
    const VIRUS_COH: i64 = 31;
    const VIRUS_STR: i64 = 9;

    const DUNGEON: i64 = 999_401;
    const SITE_ID: i64 = population::SITE_BASE + 7_777;
    /// Room-local positions of the objects (metres from the anchor).
    const CAN_POS: [f64; 3] = [3_000.0, 0.0, 0.0];
    const CAN_RADIUS: f64 = 50.0;

    /// `typedata::FIXTURE` plus the four invented types and the analyzer's
    /// effect row.
    fn content() -> crate::content::Content {
        let rows = format!(
            "type\t{CAN_TYPE}\t990001\t2\t1\t1\t1.0\t0.0\t0.0\t{CAN_RADIUS}\t0\t\t182={DATA_SKILL}.0;1919=1.0;1927=1.0\tFixture Data Can\n\
             type\t{RELIC_TYPE}\t990001\t2\t1\t1\t1.0\t0.0\t0.0\t{CAN_RADIUS}\t0\t\t182={SKILL_ARCHAEOLOGY}.0;1919=1.0;1927=1.0\tFixture Relic Can\n\
             type\t{BEACON_TYPE}\t990002\t2\t1\t1\t1.0\t0.0\t0.0\t100.0\t0\t\t\tFixture Beacon\n\
             type\t{ANALYZER_TYPE}\t990538\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{EFFECT_DO_HACKING}\t13,16,{EFFECT_DO_HACKING}\t6=1.0;30=1.0;50=5.0;54={RANGE};73={DURATION_MS}.0;182={DATA_SKILL}.0;1909={VIRUS_COH}.0;1910={VIRUS_STR}.0;1911=2.0\tFixture Analyzer\n\
             effect\t{EFFECT_DO_HACKING}\t2\t0\t73\t6\t54\t0\t0\t{EFFECT_NAME}\teffects.FixtureHack\n"
        );
        let text = format!("{}{rows}", crate::typedata::FIXTURE);
        let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
        let mut c = crate::content::Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        let mut dungeons = crate::dungeon_data::Table::fixture();
        dungeons.dungeons.insert(DUNGEON, dungeon());
        c.insert("dungeon_data", dungeons);
        c
    }

    /// An invented data dungeon: one room with a beacon, two data cans and
    /// a relic can.
    fn dungeon() -> Dungeon {
        let obj = |id, type_id, pos, radius| Object {
            room: 7,
            id,
            type_id,
            pos,
            radius,
            ..Object::default()
        };
        Dungeon {
            id: DUNGEON,
            archetype: 45,
            entry_object: Some(9001),
            rooms: vec![Room {
                id: 7,
                ..Room::default()
            }],
            objects: vec![
                obj(9001, BEACON_TYPE, [0.0, 0.0, 2_000.0], Some(100.0)),
                obj(9002, CAN_TYPE, CAN_POS, Some(CAN_RADIUS)),
                obj(9003, CAN_TYPE, [-60_000.0, 0.0, 0.0], Some(CAN_RADIUS)),
                obj(9004, RELIC_TYPE, [3_000.0, 300.0, 0.0], Some(CAN_RADIUS)),
            ],
            ..Dungeon::default()
        }
    }

    /// Invented rules, the second failure destroying the can.
    fn rules() -> Rules {
        Rules {
            dims: [(5, 4), (6, 5), (7, 6), (9, 8)],
            hole_percent: 20,
            defenses: [1, 2, 3, 4],
            caches: [1, 1, 2, 2],
            utilities: [1, 1, 1, 2],
            defense_weights: [3, 3, 2, 2, 2],
            basic_defense_max_tier: 1,
            defense_stats: [
                NodeStats::flat(17, 3),
                NodeStats::flat(9, 6),
                NodeStats::flat(13, 2),
                NodeStats::flat(11, 2),
                NodeStats::flat(12, 2),
            ],
            core_level: [
                CoreLevel::Low,
                CoreLevel::Low,
                CoreLevel::Medium,
                CoreLevel::High,
            ],
            core_stats: [(7, 5), (19, 7), (29, 9)],
            cache_weights: [1, 1, 1],
            utility_weights: [1, 1, 1, 1],
            min_start_distance: 2,
            restoration_heal: 4,
            suppressor_amount: 2,
            secondary_vector: (6, 2),
            self_repair: (5, 2),
            shield_hits: 2,
            distance_cap: 4,
            failures_to_destroy: 2,
        }
    }

    /// A board won in two clicks by the test virus: the core beside the
    /// start, weaker than one hit.
    fn easy_board() -> Board {
        let core = Node::Core {
            level: CoreLevel::Low,
            coherence: VIRUS_STR - 1,
            strength: 1,
        };
        Board::from_layout(3, 2, (0, 0), vec![((0, 0), Node::Segment), ((1, 0), core)])
            .expect("layout")
    }

    struct World {
        reg: Registry,
        /// The placed ids, by dungeon object id.
        can: i64,
        far_can: i64,
        relic: i64,
        beacon: i64,
    }

    /// Alice in a Merlin with the analyzer in med slot 0, at the data site,
    /// 1 km off the near can's surface; the site instantiated, every push
    /// drained.
    fn world() -> World {
        let alice = crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![alice],
            ..crate::model::SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content()));
        let station = reg
            .character(ALICE)
            .and_then(|c| c.station_id)
            .expect("docked");
        let mut items = reg.store().items().to_vec();
        for (item_id, type_id, location_id, flag_id) in [
            (MERLIN, 603, station, crate::model::FLAG_HANGAR),
            (ANALYZER, ANALYZER_TYPE, MERLIN, MED_SLOT_0),
        ] {
            let t = crate::item::find_in(reg.content(), type_id).expect("a known type");
            items.push(crate::item::Item {
                item_id,
                type_id,
                owner_id: ALICE,
                location_id,
                flag_id,
                quantity: 1,
                group_id: t.group_id,
                category_id: t.category_id,
                singleton: true,
            });
        }
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: MERLIN + 100,
            ..crate::store::Snapshot::default()
        });
        let c = reg.character_mut(ALICE).expect("Alice");
        c.ship_id = MERLIN;
        c.ship_type_id = 603;
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let system = reg.roster().get(ALICE).expect("in space").solar_system_id;
        let anchor = [5.0e10, 1.0e9, -2.0e10];
        let site = Site {
            system,
            row: 0,
            k: 0,
            slot: 0,
            generation: 0,
            site_id: SITE_ID,
            dungeon_id: DUNGEON,
            archetype: 45,
            entry: Entry::Signature,
            anchor,
            target_id: "HAK-001".into(),
            state: population::State::Live,
            at: 0,
        };
        assert!(crate::site::instantiate(&mut reg, &site));
        let id_of = |reg: &Registry, dun: i64| {
            reg.ext::<Sites>()
                .get(SITE_ID)
                .expect("instantiated")
                .objects
                .iter()
                .find(|p| p.dun_object_id == dun)
                .expect("placed")
                .item_id
        };
        let (beacon, can, far_can, relic) = (
            id_of(&reg, 9001),
            id_of(&reg, 9002),
            id_of(&reg, 9003),
            id_of(&reg, 9004),
        );
        let hull = f64::from(crate::destiny::hull::of_in(reg.content(), 603).radius);
        let at = [
            anchor[0] + CAN_POS[0] - CAN_RADIUS - hull - 1_000.0,
            anchor[1],
            anchor[2],
        ];
        park(&mut reg, at);
        reg.ext_mut::<Hacks>().set_rules(rules());
        drain(&mut reg);
        World {
            reg,
            can,
            far_can,
            relic,
            beacon,
        }
    }

    fn park(reg: &mut Registry, at: [f64; 3]) {
        let p = reg.roster_mut().get_mut(ALICE).expect("in space");
        p.motion = crate::space_state::Motion::arrived(at, [0.0; 3]);
        p.since = crate::space_state::now_seconds();
    }

    /// One push as sent: `(event, args, audience)`.
    type Sent = (&'static str, Vec<PyValue>, Option<Vec<i64>>);

    fn drain(reg: &mut Registry) -> Vec<Sent> {
        let far = Instant::now() + Duration::from_secs(3_600);
        let mut out: Vec<Box<dyn crate::notify::Push>> =
            reg.take_pending().into_iter().map(|(_, p)| p).collect();
        out.extend(reg.take_due(far));
        out.into_iter()
            .map(|p| {
                let (name, args) = p.to_push();
                (name, args, p.audience())
            })
            .collect()
    }

    /// The event ids of every `OnHackingTurnComplete` sent to Alice.
    fn turns(sent: &[Sent]) -> Vec<Vec<i64>> {
        sent.iter()
            .filter(|(n, _, to)| {
                *n == "OnHackingTurnComplete" && to.as_deref() == Some(&[ALICE][..])
            })
            .map(|(_, args, _)| {
                let PyValue::List(events) = &args[0] else {
                    panic!("events are a list")
                };
                events
                    .iter()
                    .map(|e| match crate::pyutil::dict_get(e, b"eventID") {
                        Some(PyValue::Int(i)) => *i,
                        other => panic!("eventID {other:?}"),
                    })
                    .collect()
            })
            .collect()
    }

    /// Every destiny call in the `DoDestinyUpdate`s sent: `(name, args)`.
    fn destiny_calls(sent: &[Sent]) -> Vec<(Vec<u8>, Vec<PyValue>)> {
        let mut out = Vec::new();
        for (name, args, _) in sent {
            if *name != "DoDestinyUpdate" {
                continue;
            }
            let PyValue::List(entries) = &args[0] else {
                continue;
            };
            for e in entries {
                let PyValue::Tuple(t) = e else { continue };
                let Some(PyValue::Tuple(call)) = t.get(1) else {
                    continue;
                };
                let (Some(PyValue::Bytes(n)), Some(PyValue::Tuple(a))) =
                    (call.first(), call.get(1))
                else {
                    continue;
                };
                out.push((n.clone(), a.clone()));
            }
        }
        out
    }

    /// The `(hackingSecurityState, ownerID)` of every `OnSlimItemChange` for
    /// `id`.
    fn slim_changes(sent: &[Sent], id: i64) -> Vec<(i64, i64)> {
        destiny_calls(sent)
            .into_iter()
            .filter(|(n, a)| n == b"OnSlimItemChange" && a.first() == Some(&PyValue::Int(id)))
            .map(|(_, a)| {
                let PyValue::Instance { state, .. } = &a[1] else {
                    panic!("an instance slim")
                };
                let int = |k: &str| match crate::pyutil::dict_get(state, k.as_bytes()) {
                    Some(PyValue::Int(v)) => *v,
                    other => panic!("{k}: {other:?}"),
                };
                (int(KEY_SECURITY_STATE), int("ownerID"))
            })
            .collect()
    }

    fn activate(reg: &mut Registry, target: i64) -> Result<PyValue, ServiceError> {
        crate::modules::activate(reg, Some(ALICE), ANALYZER, EFFECT_NAME, Some(target), 0, T0)
    }

    fn period() -> i64 {
        DURATION_MS * crate::modules::MSEC
    }

    /// The world clock's stages that matter here, at `at`.
    fn clock(reg: &mut Registry, at: i64) {
        crate::modules::tick(reg, at);
        crate::combat::tick(reg, at);
        crate::mining::tick(reg, at);
        crate::hacking::tick(reg, Instant::now(), at);
    }

    fn call(reg: &mut Registry, method: &str, args: Vec<PyValue>) -> PyValue {
        let req = Request {
            service: "hackingMgr",
            method,
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        reg.dispatch(&req).expect("hackingMgr answers")
    }

    fn coord_arg(x: i64, y: i64) -> PyValue {
        PyValue::Tuple(vec![PyValue::Int(x), PyValue::Int(y)])
    }

    /// Lock, activate and run the cycle out: the game is open.
    fn open_game(w: &mut World, at: i64) {
        w.reg.locks_mut().add(MERLIN, w.can);
        crate::modules::activate(
            &mut w.reg,
            Some(ALICE),
            ANALYZER,
            EFFECT_NAME,
            Some(w.can),
            0,
            at,
        )
        .expect("the analyzer starts");
        clock(&mut w.reg, at + period());
    }

    /// **Activation is refused** for an unlocked can, a can out of range, a
    /// skill mismatch, a can someone is hacking, and a non-hackable object;
    /// nothing runs and nothing is armed.
    #[test]
    fn activation_is_refused_where_it_must_be() {
        let mut w = world();
        // Unlocked.
        assert!(activate(&mut w.reg, w.can).is_err());
        // Out of range.
        w.reg.locks_mut().add(MERLIN, w.far_can);
        assert!(activate(&mut w.reg, w.far_can).is_err());
        // A relic can and a data analyzer.
        w.reg.locks_mut().add(MERLIN, w.relic);
        assert!(activate(&mut w.reg, w.relic).is_err());
        // Not hackable.
        w.reg.locks_mut().add(MERLIN, w.beacon);
        assert!(activate(&mut w.reg, w.beacon).is_err());
        // Being hacked by somebody else.
        w.reg.locks_mut().add(MERLIN, w.can);
        let other = crate::hacking::Game::new(
            easy_board(),
            &rules(),
            VirusStats::from_attributes(10.0, 1.0, 0.0),
            Style::Hacking,
            None,
        );
        w.reg.ext_mut::<Hacks>().start(1_000_002, w.can, other);
        assert!(activate(&mut w.reg, w.can).is_err());
        assert!(w.reg.modules().running(ANALYZER).is_none());
        assert_eq!(w.reg.ext::<Hacks>().armed(ALICE), None);
    }

    /// **The first cycle end opens exactly one game**: nothing before it;
    /// at it, one `OnHackingTurnComplete` to the hacker, the start batch in
    /// order, and the can's slim goes 0 -> 1 for everyone in the system.
    #[test]
    fn the_cycle_end_pushes_one_start_and_the_slim_goes_to_one() {
        let mut w = world();
        assert_eq!(crate::hacking::security_state(&w.reg, w.can), Some(0));
        w.reg.locks_mut().add(MERLIN, w.can);
        activate(&mut w.reg, w.can).expect("the analyzer starts");
        clock(&mut w.reg, T0 + period() - 1);
        assert!(
            turns(&drain(&mut w.reg)).is_empty(),
            "not before the cycle ends"
        );
        clock(&mut w.reg, T0 + period());
        let sent = drain(&mut w.reg);
        let t = turns(&sent);
        assert_eq!(t.len(), 1, "one push");
        assert_eq!(t[0].first(), Some(&crate::hacking::EVENT_GAME_STYLE));
        assert_eq!(t[0].last(), Some(&crate::hacking::EVENT_GAME_START));
        assert_eq!(slim_changes(&sent, w.can), vec![(1, crate::site::OWNER)]);
        assert_eq!(w.reg.ext::<Hacks>().can_state(w.can), CanState::BeingHacked);
        // The bootstrap's slim says so too (the site's S9 source).
        let extras = crate::slim_extra::for_slims(&w.reg, &[w.can]);
        assert!(extras[0].1.contains(&(KEY_SECURITY_STATE, PyValue::Int(1))));
        // A later cycle starts nothing more.
        clock(&mut w.reg, T0 + 3 * period());
        assert!(turns(&drain(&mut w.reg)).is_empty());
    }

    /// **Warping mid-game loses it**: `GAME_LOST` on the clock, one failure
    /// counted, the can secure again.
    #[test]
    fn warping_loses_the_game_and_counts_one_failure() {
        let mut w = world();
        open_game(&mut w, T0);
        drain(&mut w.reg);
        let p = w.reg.roster_mut().get_mut(ALICE).expect("in space");
        p.motion.mode = crate::destiny::Mode::Warp {
            goto: [0.0; 3],
            effect_stamp: 0,
            total_distance: 0.0,
            stop_distance: 0.0,
            warp_factor: 3000,
        };
        crate::hacking::tick(&mut w.reg, Instant::now(), T0 + period());
        let sent = drain(&mut w.reg);
        assert_eq!(turns(&sent), vec![vec![crate::hacking::EVENT_GAME_LOST]]);
        assert_eq!(w.reg.ext::<Hacks>().failures(w.can), 1);
        assert_eq!(slim_changes(&sent, w.can), vec![(0, crate::site::OWNER)]);
        assert!(w.reg.ext::<Hacks>().game(ALICE).is_none());
    }

    /// **The second failure removes the can**: `RemoveBalls`, the lock
    /// closed as `Exploding`, gone from the site, and counted done.
    #[test]
    fn the_second_failure_removes_the_ball() {
        let mut w = world();
        open_game(&mut w, T0);
        drain(&mut w.reg);
        assert_eq!(
            call(&mut w.reg, "QuitHackingAttempt", vec![]),
            PyValue::None
        );
        let sent = drain(&mut w.reg);
        assert_eq!(turns(&sent), vec![vec![crate::hacking::EVENT_GAME_LOST]]);
        assert!(
            crate::site::object(&w.reg, w.can).is_some(),
            "one failure: still there"
        );
        assert_eq!(crate::site::hackables_left(&w.reg, SITE_ID), (3, 3));

        open_game(&mut w, T0 + 10 * period());
        drain(&mut w.reg);
        call(&mut w.reg, "QuitHackingAttempt", vec![]);
        let sent = drain(&mut w.reg);
        let removed = destiny_calls(&sent).into_iter().any(|(n, a)| {
            n == b"RemoveBalls" && a == vec![PyValue::List(vec![PyValue::Int(w.can)])]
        });
        assert!(removed, "RemoveBalls for the can: {sent:?}");
        assert!(
            sent.iter().any(|(n, _, _)| *n == "OnTarget"),
            "the lock closes"
        );
        assert!(crate::site::object(&w.reg, w.can).is_none());
        assert!(!w.reg.locks().targets_of(MERLIN).contains(&w.can));
        assert!(
            crate::site::balls_in(&w.reg, w.reg.roster().get(ALICE).unwrap().solar_system_id)
                .iter()
                .all(|(b, _)| b.ball_id != w.can)
        );
        assert_eq!(crate::site::hackables_left(&w.reg, SITE_ID), (3, 2));
    }

    /// **Success**: two clicks on a scripted board win; the slim goes to 2
    /// with the hacker as owner, the can is counted done, and a second
    /// activation is refused.
    #[test]
    fn success_sets_state_two_and_the_hacker_as_owner() {
        let mut w = world();
        w.reg.ext_mut::<Hacks>().script_next(easy_board());
        open_game(&mut w, T0);
        drain(&mut w.reg);
        call(&mut w.reg, "ClickedOnTile", vec![coord_arg(1, 0)]);
        call(&mut w.reg, "ClickedOnTile", vec![coord_arg(1, 0)]);
        let sent = drain(&mut w.reg);
        assert_eq!(
            turns(&sent).last().and_then(|t| t.last()),
            Some(&crate::hacking::EVENT_GAME_WON)
        );
        assert_eq!(slim_changes(&sent, w.can), vec![(2, ALICE)]);
        assert_eq!(w.reg.ext::<Hacks>().owner(w.can), Some(ALICE));
        assert_eq!(
            crate::site::object(&w.reg, w.can).map(|(p, _)| p.owner_id),
            Some(ALICE)
        );
        assert_eq!(crate::site::hackables_left(&w.reg, SITE_ID), (3, 2));
        let later = T0 + 10 * period();
        assert!(
            crate::modules::activate(
                &mut w.reg,
                Some(ALICE),
                ANALYZER,
                EFFECT_NAME,
                Some(w.can),
                0,
                later
            )
            .is_err(),
            "a hacked can is refused"
        );
    }

    /// The arms with no game answer `None` and push one `ACK` (quit: none);
    /// a malformed coordinate is an `ACK` too.
    #[test]
    fn the_arms_without_a_game_answer_none_and_ack() {
        let mut w = world();
        let ack = vec![vec![crate::hacking::EVENT_ACK]];
        assert_eq!(
            call(&mut w.reg, "ClickedOnTile", vec![coord_arg(2, 3)]),
            PyValue::None
        );
        assert_eq!(turns(&drain(&mut w.reg)), ack);
        let args = vec![PyValue::Int(0), coord_arg(2, 3)];
        assert_eq!(call(&mut w.reg, "UsedUtilityElement", args), PyValue::None);
        assert_eq!(turns(&drain(&mut w.reg)), ack);
        assert_eq!(
            call(
                &mut w.reg,
                "UsedUtilityElementOnVirus",
                vec![PyValue::Int(1)]
            ),
            PyValue::None
        );
        assert_eq!(turns(&drain(&mut w.reg)), ack);
        assert_eq!(
            call(&mut w.reg, "ClickedOnTile", vec![PyValue::None]),
            PyValue::None
        );
        assert_eq!(turns(&drain(&mut w.reg)), ack);
        assert_eq!(
            call(&mut w.reg, "QuitHackingAttempt", vec![]),
            PyValue::None
        );
        assert!(turns(&drain(&mut w.reg)).is_empty());
    }

    /// **A lock on a hackable can succeeds**; a lock on a site object that
    /// is not hackable still gets the silent cancel.
    #[test]
    fn a_hackable_can_locks_and_a_beacon_does_not() {
        let mut w = world();
        assert!(crate::targeting::lock(&mut w.reg, ALICE, w.can).is_ok());
        assert!(w.reg.locks().targets_of(MERLIN).contains(&w.can));
        assert!(crate::targeting::lock(&mut w.reg, ALICE, w.beacon).is_err());
        assert!(!w.reg.locks().targets_of(MERLIN).contains(&w.beacon));
    }

    /// `StartNewGameInstance(None, ...)` opens a practice board for a GM
    /// and does nothing for anyone else or with a target.
    #[test]
    fn the_practice_board_is_for_gms_only() {
        let mut w = world();
        let args = vec![
            PyValue::None,
            PyValue::Int(0),
            PyValue::Int(ANALYZER_TYPE),
            PyValue::Int(1),
            PyValue::Int(30),
            PyValue::Int(5),
            PyValue::Int(2),
        ];
        let player = Caller::Session {
            user_id: 1,
            role: 0,
        };
        let r = Request {
            service: "hackingMgr",
            method: "StartNewGameInstance",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        assert_eq!(w.reg.dispatch_as(player, &r), Ok(PyValue::None));
        assert!(turns(&drain(&mut w.reg)).is_empty(), "not a GM");
        let mut aimed = args.clone();
        aimed[0] = PyValue::Int(w.can);
        call(&mut w.reg, "StartNewGameInstance", aimed);
        assert!(turns(&drain(&mut w.reg)).is_empty(), "a target");
        call(&mut w.reg, "StartNewGameInstance", args);
        let t = turns(&drain(&mut w.reg));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].last(), Some(&crate::hacking::EVENT_GAME_START));
        // Quitting it counts against no can.
        call(&mut w.reg, "QuitHackingAttempt", vec![]);
        assert_eq!(
            w.reg.ext::<Hacks>().failures(crate::hacking::PRACTICE_CAN),
            0
        );
    }

    /// The completion rule as a table.
    #[test]
    fn a_site_with_cans_completes_when_every_can_is_done() {
        use crate::site::complete_with_cans as done;
        assert!(done(true, true, 0, 0), "no can: the base rule");
        assert!(!done(false, false, 0, 0));
        assert!(done(false, false, 3, 0), "cans only");
        assert!(!done(false, false, 3, 1));
        assert!(!done(false, true, 3, 0), "squads still up");
        assert!(done(true, true, 3, 0));
    }
}
