//! The rookie ship: `dogmaIM.CreateNewbieShip(shipID, locationID)`.
//!
//! Packet E7 (research R-ECON §2.4). The docked UI's "Board my Corvette"
//! button (`corvetteButton.pyc`, label `UI/Station/CreateNewbieShipButton`)
//! runs `station/base.pyc::CreateNewbieShip` (`:701`-`:727`) **[V]**:
//!
//! - not docked: it raises `NewbieShipError('MustBeDocked')` itself (`:705`);
//! - already in a ship of `groupCorvette`: `AlreadyInNewbieShip` (`:715`);
//! - in any other ship but a capsule: it asks
//!   `UI/Station/ConfirmMoveToNewbieShip`, "Are you sure you wish to leave your
//!   active ship and board a Corvette?" (`:718`), so **the server serves a
//!   pilot in a ship as well as one in a capsule** and the old ship stays in
//!   the hangar;
//! - then `sm.RemoteSvc('dogmaIM').CreateNewbieShip(shipID, locationID)`,
//!   reply discarded (`:723`, `POP_TOP`); a `UserError` is re-raised to the
//!   player, anything else becomes `ErrorCreatingNewbieShip` (`:724`-`:727`).
//!
//! The three keys are dialogs with no arguments (`findings/user-errors.tsv`:
//! `MustBeDocked` 4655, `AlreadyInNewbieShip` 7258, `ErrorCreatingNewbieShip`
//! 7255) **[V]**, so this server raises them as bare `UserError`s: the client
//! checks only for a pilot it can see, and a second client, a stale window or
//! a bot reaches the server unchecked.
//!
//! **What is minted.** The race's corvette, `races[r].shipTypeID` **[V]**
//! (the bundle's `agents/races.tsv`, P-START-X), in the station hangar, fitted
//! from `content/econ/starter-fit.tsv` (authored: the client ships no starter
//! fit, so every row is [A]), then boarded exactly as `ship.Board` boards: the
//! capsule moves into it at `flagCapsule` and the `{'shipid': (old, new)}`
//! session change re-primes godma.
//!
//! **Market groups, checked over the bundle (R-ECON §2.4 had "no market
//! group" [V]):** the check falsifies that claim for most of the fit. The four
//! corvettes and the civilian miner and afterburner have a market group in
//! build 3619879's `types/market.tsv`; only the four civilian guns have none.
//! The load prints every starter type that has one ([`market_listed`]), so a
//! closed economy that wants them unsellable has the list.

use crate::content::{Boxed, Content, Origin, Sources, TableSpec};
use crate::item::{Change, Edit, Slot};
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "starter_ship",
    load,
    fixture: || Box::new(Table::default()) as Boxed,
};

/// The bundle's race table and the header `schema.md` freezes (P-START-X).
pub const RACES: (&str, &str) = (
    "agents/races.tsv",
    "#race_id\tship_type_id\tskills\tname_id",
);

/// The authored fit under `--authored`, and its header (`schema.md` §4).
pub const FIT_FILE: &str = "econ/starter-fit.tsv";
pub const FIT_HEADER: &str = "ship_type_id\ttype_id\twhy";
const SHIPPED_FIT: &str = include_str!("../../../../content/econ/starter-fit.tsv");

/// `groupCorvette` (`inventorycommon/const`), what the client's
/// `idCheckers.IsNewbieShip(groupID)` compares against **[V]**.
pub const GROUP_CORVETTE: i64 = 237;

/// The three refusal keys (module doc) **[V]**.
pub const MUST_BE_DOCKED: &str = "MustBeDocked";
pub const ALREADY_IN_NEWBIE_SHIP: &str = "AlreadyInNewbieShip";
pub const ERROR_CREATING_NEWBIE_SHIP: &str = "ErrorCreatingNewbieShip";

/// Race -> corvette, and corvette -> its fit in row order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    ships: BTreeMap<i64, i64>,
    fits: BTreeMap<i64, Vec<i64>>,
}

impl Table {
    /// The corvette `race_id` is given, if the race has one.
    pub fn ship_of_race(&self, race_id: i64) -> Option<i64> {
        self.ships.get(&race_id).copied()
    }

    /// The modules fitted into a fresh `ship_type_id`, in fit order.
    pub fn fit_of(&self, ship_type_id: i64) -> &[i64] {
        self.fits.get(&ship_type_id).map_or(&[], Vec::as_slice)
    }

    /// Every type this table mints: the corvettes and their modules, sorted.
    pub fn types(&self) -> Vec<i64> {
        let mut t: Vec<i64> = self
            .ships
            .values()
            .chain(self.fits.values().flatten())
            .copied()
            .collect();
        t.sort_unstable();
        t.dedup();
        t
    }

    /// `agents/races.tsv` data lines: a race with an empty `ship_type_id` has
    /// no corvette. A malformed line is an error naming it.
    pub fn parse_races<S: AsRef<str>>(&mut self, lines: &[S]) -> Result<(), String> {
        for (i, l) in lines.iter().enumerate() {
            let bad = || format!("{}: data line {}: {:?}", RACES.0, i + 1, l.as_ref());
            let f: Vec<&str> = l.as_ref().split('\t').collect();
            let [race, ship, _skills, _name] = f[..] else {
                return Err(bad());
            };
            let race: i64 = race.parse().map_err(|_| bad())?;
            if !ship.is_empty() {
                self.ships.insert(race, ship.parse().map_err(|_| bad())?);
            }
        }
        Ok(())
    }

    /// The authored fit rows, `(ship_type_id, type_id)`, in file order.
    pub fn add_fit(&mut self, rows: &[(i64, i64)]) {
        for &(ship, module) in rows {
            self.fits.entry(ship).or_default().push(module);
        }
    }
}

/// The authored fit's rows; a malformed row is skipped and logged by the
/// reader.
fn fit_rows(file: &crate::authored::File) -> Vec<(i64, i64)> {
    file.parse_rows(|r| Ok((r.i64(0)?, r.i64(1)?)))
}

/// Of `types`, those `types/market.tsv` data lines give a market group, as
/// `(type_id, market_group_id)`, sorted. The column is the fourth
/// (`econ_exchange::MARKET`).
pub fn market_listed<S: AsRef<str>>(types: &[i64], market_lines: &[S]) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = market_lines
        .iter()
        .filter_map(|l| {
            let mut f = l.as_ref().split('\t');
            let type_id: i64 = f.next()?.parse().ok()?;
            let group: i64 = f.nth(2)?.parse().ok()?;
            types.contains(&type_id).then_some((type_id, group))
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn load(src: &Sources) -> Result<Boxed, String> {
    let mut t = Table::default();
    let file = src.authored(FIT_FILE, SHIPPED_FIT, FIT_HEADER)?;
    t.add_fit(&fit_rows(&file));
    if let Some(b) = src.bundle {
        src.note(Origin::Bundle);
        t.parse_races(&b.data_lines(RACES.0, RACES.1)?)?;
        let market = b.data_lines(
            crate::econ_exchange::MARKET.0,
            crate::econ_exchange::MARKET.1,
        )?;
        let listed = market_listed(&t.types(), &market);
        if !listed.is_empty() {
            crate::errln!(
                "[starter] {} of {} starter types have a market group (sellable): {}",
                listed.len(),
                t.types().len(),
                listed
                    .iter()
                    .map(|(ty, g)| format!("{ty}@{g}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
    }
    Ok(Box::new(t))
}

fn refuse(key: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, vec![]))
}

/// The modules of `fit` that go into a fresh `ship_type_id`, each with its
/// slot flag: the lowest free slot of its rack, within the hull's slot and
/// hardpoint counts. A module that does not fit, or that this server cannot
/// place, is skipped and logged.
fn place_fit(content: &Content, ship_type_id: i64, fit: &[i64]) -> Vec<(i64, i64)> {
    let Some(hull) = crate::fitting::hull_in(content, ship_type_id) else {
        crate::errln!("[starter] hull {ship_type_id} has no fitting numbers; fitted nothing");
        return Vec::new();
    };
    let mut used: BTreeMap<i64, ()> = BTreeMap::new();
    let mut hardpoints: Vec<crate::fitting::Hardpoint> = Vec::new();
    let mut out = Vec::new();
    for &type_id in fit {
        let Some(m) = crate::modules::find_in(content, type_id) else {
            crate::errln!("[starter] {type_id} is not a module this server fits; skipped");
            continue;
        };
        if let Some(hp) = m.hardpoint {
            let n = hardpoints.iter().filter(|h| **h == hp).count() as i64;
            if n >= hull.hardpoints(hp) {
                crate::errln!(
                    "[starter] {type_id}: hull {ship_type_id} has no hardpoint left; skipped"
                );
                continue;
            }
        }
        let first = m.rack.first_flag();
        let Some(flag) = (first..first + hull.slots(m.rack)).find(|f| !used.contains_key(f)) else {
            crate::errln!("[starter] {type_id}: hull {ship_type_id} has no slot left; skipped");
            continue;
        };
        used.insert(flag, ());
        if let Some(hp) = m.hardpoint {
            hardpoints.push(hp);
        }
        out.push((type_id, flag));
    }
    out
}

/// `dogmaIM.CreateNewbieShip(shipID, locationID)`: mint the pilot's race's
/// corvette in the station hangar, fit it from the starter table, and board
/// it. Both arguments are ignored: the docked station and the active ship are
/// ours to know. Answers `None` (the client discards it).
///
/// Refused with [`MUST_BE_DOCKED`] in space, with [`ALREADY_IN_NEWBIE_SHIP`]
/// when the active ship is a corvette **[V: the client's own check]** or when
/// one of the pilot's corvettes is already in this station's hangar **[A]**
/// (R-ECON §2.4: the rookie ship is reissued only while none is there;
/// falsified by TQ handing a second corvette to a pilot with one in the
/// hangar), and with [`ERROR_CREATING_NEWBIE_SHIP`] when the race has no
/// corvette (no bundle) or the store refuses the plan.
pub(crate) fn create_newbie_ship(
    reg: &mut Registry,
    actor: Option<i64>,
) -> Result<PyValue, ServiceError> {
    let Some(c) = actor.and_then(|id| reg.character(id)).cloned() else {
        return Err(refuse(ERROR_CREATING_NEWBIE_SHIP));
    };
    let Some(station) = reg.docked_station(c.character_id) else {
        return Err(refuse(MUST_BE_DOCKED));
    };
    let is_corvette = |i: &crate::item::Item| {
        i.owner_id == c.character_id
            && i.category_id == crate::model::CATEGORY_SHIP
            && i.group_id == GROUP_CORVETTE
    };
    if reg.item(c.ship_id).is_some_and(is_corvette)
        || reg.store().items().iter().any(|i| {
            is_corvette(i) && i.location_id == station && i.flag_id == crate::model::FLAG_HANGAR
        })
    {
        return Err(refuse(ALREADY_IN_NEWBIE_SHIP));
    }
    let content = reg.content();
    let Some(table) = content.try_get::<Table>() else {
        return Err(refuse(ERROR_CREATING_NEWBIE_SHIP));
    };
    let Some(ship_type_id) = table.ship_of_race(c.race_id) else {
        crate::errln!(
            "[starter] race {} has no corvette in {} (no bundle?)",
            c.race_id,
            RACES.0
        );
        return Err(refuse(ERROR_CREATING_NEWBIE_SHIP));
    };
    let fit = place_fit(content, ship_type_id, table.fit_of(ship_type_id));
    let failed = |e: crate::item::EditError| {
        crate::errln!("[starter] item plan refused: {e}");
        refuse(ERROR_CREATING_NEWBIE_SHIP)
    };

    // 1. The hull, in the hangar.
    let hangar = Slot::new(c.character_id, station, crate::model::FLAG_HANGAR);
    let made = reg
        .apply_items(&[Edit::Create {
            type_id: ship_type_id,
            quantity: 1,
            to: hangar,
        }])
        .map_err(failed)?;
    let Some(ship_id) = made.iter().find_map(|ch| match ch {
        Change::Created { item_id, .. } => Some(*item_id),
        _ => None,
    }) else {
        return Err(refuse(ERROR_CREATING_NEWBIE_SHIP));
    };

    // 2. The modules, one each in its slot, and the capsule climbing in.
    let mut plan: Vec<Edit> = fit
        .iter()
        .map(|&(type_id, flag)| Edit::Create {
            type_id,
            quantity: 1,
            to: Slot::new(c.character_id, ship_id, flag),
        })
        .collect();
    let pod = crate::fitting::capsule_of(reg.store(), &c).map(|p| p.item_id);
    if let Some(pod) = pod {
        plan.push(Edit::Move {
            item_id: pod,
            to: Slot::new(c.character_id, ship_id, crate::fitting::FLAG_CAPSULE),
        });
    }
    let made = reg.apply_items(&plan).map_err(failed)?;
    let modules: Vec<i64> = made
        .iter()
        .filter_map(|ch| match ch {
            Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .collect();

    // 3. Assembled in place: godma drops a non-singleton module [V]
    // (`Edit::Assemble`'s doc).
    if !modules.is_empty() {
        let plan: Vec<Edit> = modules
            .iter()
            .filter_map(|&id| reg.item(id).map(|i| (id, i.slot())))
            .map(|(item_id, to)| Edit::Assemble { item_id, to })
            .collect();
        reg.apply_items(&plan).map_err(failed)?;
    }
    for &m in &modules {
        crate::modules::on_fitted(reg, ship_id, ship_type_id, m);
    }

    // 4. Aboard, as `ship.Board` leaves it.
    let Some(cm) = reg.character_mut(c.character_id) else {
        return Err(refuse(ERROR_CREATING_NEWBIE_SHIP));
    };
    let old = cm.ship_id;
    cm.ship_id = ship_id;
    cm.ship_type_id = ship_type_id;
    reg.queue_session_change(
        c.character_id,
        crate::pyutil::pypairs(vec![(
            "shipid",
            PyValue::Tuple(vec![PyValue::Int(old), PyValue::Int(ship_id)]),
        )]),
    );
    Ok(PyValue::None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FLAG_HANGAR, SandboxConfig};

    // Invented ids only (`schema.md` §1.1), never a client value.
    const RACE: i64 = 91;
    const OTHER_RACE: i64 = 92;
    const HULL: i64 = 990_870_001;
    /// `typedata::FIXTURE`'s autocannon (hi rack, turret) and shield
    /// extender (medium rack).
    const GUN: i64 = 990_000_002;
    const MED: i64 = 990_000_003;

    /// `typedata::FIXTURE` plus one invented corvette: the fixture frigate's
    /// numbers (3 hi, 3 medium, 4 low, 2 turrets) in [`GROUP_CORVETTE`].
    fn content(table: Table) -> Content {
        let hull = format!(
            "type\t{HULL}\t{GROUP_CORVETTE}\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
             9=310.0;11=45.0;12=4.0;13=3.0;14=3.0;37=355.0;48=125.0;55=150000.0;70=3.1;\
             101=0.0;102=2.0;552=40.0;600=5.0\tFixture Corvette\n"
        );
        let text = format!("{}{hull}", crate::typedata::FIXTURE);
        let td = crate::typedata::Table::parse(&text).expect("the test rows parse");
        let mut c = Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(td));
        c.insert(TABLE.name, table);
        c
    }

    /// Race [`RACE`] flies [`HULL`], fitted with three guns (one more than
    /// the hull's two turrets) and a medium-rack module.
    fn table() -> Table {
        let mut t = Table::default();
        t.parse_races(&[
            format!("{RACE}\t{HULL}\t\t1"),
            format!("{OTHER_RACE}\t\t\t2"),
        ])
        .expect("parses");
        t.add_fit(&[(HULL, GUN), (HULL, MED), (HULL, GUN), (HULL, GUN)]);
        t
    }

    /// The default sandbox's pilot, of [`RACE`], docked in a capsule.
    fn world() -> (Registry, i64) {
        let mut reg = Registry::new(SandboxConfig::default())
            .with_content(std::sync::Arc::new(content(table())));
        let me = reg.config().characters[0].character_id;
        reg.character_mut(me).expect("pilot").race_id = RACE;
        let _ = reg.take_session_changes();
        let _ = reg.take_pending();
        (reg, me)
    }

    /// Whether `v`'s Debug form holds `text` as marshal bytes (a key).
    fn shows(v: &impl std::fmt::Debug, text: &str) -> bool {
        format!("{v:?}").contains(&format!("{:?}", text.as_bytes()))
    }

    /// Whether the call was refused with the UserError `want`.
    fn refused_with(r: Result<PyValue, ServiceError>, want: &str) -> bool {
        match r {
            Err(ServiceError::Raise(v)) => shows(&v, want),
            other => panic!("expected a UserError, got {other:?}"),
        }
    }

    fn corvettes(reg: &Registry, me: i64) -> Vec<crate::item::Item> {
        reg.store()
            .items()
            .iter()
            .filter(|i| i.owner_id == me && i.type_id == HULL)
            .cloned()
            .collect()
    }

    #[test]
    fn a_docked_pilot_in_a_capsule_gets_the_race_s_corvette_fitted_and_boarded() {
        let (mut reg, me) = world();
        let pod = reg.character(me).expect("pilot").ship_id;
        let station = reg.docked_station(me).expect("docked");
        assert_eq!(create_newbie_ship(&mut reg, Some(me)), Ok(PyValue::None));

        let ships = corvettes(&reg, me);
        assert_eq!(ships.len(), 1, "{ships:?}");
        let ship = &ships[0];
        assert!(ship.singleton, "boardable");
        assert_eq!(
            (ship.location_id, ship.flag_id),
            (station, FLAG_HANGAR),
            "in the station hangar"
        );
        let c = reg.character(me).expect("pilot");
        assert_eq!((c.ship_id, c.ship_type_id), (ship.item_id, HULL), "aboard");
        let capsule = reg.item(pod).expect("the capsule");
        assert_eq!(
            (capsule.location_id, capsule.flag_id),
            (ship.item_id, crate::fitting::FLAG_CAPSULE)
        );

        // Two guns (the hull's two turrets; the third is skipped) in the
        // first two high slots, the medium module in the first medium slot,
        // every one assembled.
        let mut fitted: Vec<(i64, i64, bool)> = reg
            .store()
            .contents_of(ship.item_id)
            .filter(|i| crate::item::is_ship_fitting_flag(i.flag_id))
            .map(|i| (i.flag_id, i.type_id, i.singleton))
            .collect();
        fitted.sort_unstable();
        let hi = crate::fitting::Rack::High.first_flag();
        let med = crate::fitting::Rack::Medium.first_flag();
        assert_eq!(
            fitted,
            vec![(med, MED, true), (hi, GUN, true), (hi + 1, GUN, true)]
        );

        let changes = reg.take_session_changes();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(changes[0].0, me);
        assert!(shows(&changes[0].1, "shipid"), "{:?}", changes[0].1);
    }

    #[test]
    fn a_second_call_aboard_or_with_the_corvette_in_the_hangar_is_already_in_newbie_ship() {
        let (mut reg, me) = world();
        create_newbie_ship(&mut reg, Some(me)).expect("the first");
        assert!(refused_with(
            create_newbie_ship(&mut reg, Some(me)),
            ALREADY_IN_NEWBIE_SHIP
        ));

        // Back in the capsule, the corvette parked in the hangar.
        let station = reg.docked_station(me).expect("docked");
        let c = reg.character(me).cloned().expect("pilot");
        let pod = crate::fitting::capsule_of(reg.store(), &c)
            .expect("capsule")
            .clone();
        reg.apply_items(&[Edit::Move {
            item_id: pod.item_id,
            to: Slot::new(me, station, FLAG_HANGAR),
        }])
        .expect("out");
        let cm = reg.character_mut(me).expect("pilot");
        cm.ship_id = pod.item_id;
        cm.ship_type_id = pod.type_id;
        assert!(refused_with(
            create_newbie_ship(&mut reg, Some(me)),
            ALREADY_IN_NEWBIE_SHIP
        ));
        assert_eq!(corvettes(&reg, me).len(), 1, "no second corvette");
    }

    #[test]
    fn undocked_the_call_is_must_be_docked_and_mints_nothing() {
        let (mut reg, me) = world();
        let ship = reg.character(me).expect("pilot").ship_id;
        let system = reg
            .character(me)
            .and_then(|c| c.solar_system_id)
            .expect("system");
        use crate::space_state::{Motion, Presence};
        reg.roster_mut().arrive(Presence::arriving(
            me,
            ship,
            system,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        assert!(refused_with(
            create_newbie_ship(&mut reg, Some(me)),
            MUST_BE_DOCKED
        ));
        assert!(corvettes(&reg, me).is_empty());
    }

    #[test]
    fn a_race_without_a_corvette_is_error_creating_newbie_ship() {
        let (mut reg, me) = world();
        reg.character_mut(me).expect("pilot").race_id = OTHER_RACE;
        assert!(refused_with(
            create_newbie_ship(&mut reg, Some(me)),
            ERROR_CREATING_NEWBIE_SHIP
        ));
        assert!(corvettes(&reg, me).is_empty());
    }

    #[test]
    fn a_pilot_in_another_ship_leaves_it_in_the_hangar_and_boards_the_corvette() {
        let (mut reg, me) = world();
        let station = reg.docked_station(me).expect("docked");
        // The fixture frigate, not a corvette, made active as Board does.
        let made = reg
            .apply_items(&[Edit::Create {
                type_id: 990_000_001,
                quantity: 1,
                to: Slot::new(me, station, FLAG_HANGAR),
            }])
            .expect("a frigate");
        let Change::Created {
            item_id: frigate, ..
        } = made[0]
        else {
            panic!("{made:?}");
        };
        let c = reg.character(me).cloned().expect("pilot");
        let pod = crate::fitting::capsule_of(reg.store(), &c)
            .expect("pod")
            .item_id;
        reg.apply_items(&[Edit::Move {
            item_id: pod,
            to: Slot::new(me, frigate, crate::fitting::FLAG_CAPSULE),
        }])
        .expect("aboard");
        reg.character_mut(me).expect("pilot").ship_id = frigate;

        create_newbie_ship(&mut reg, Some(me)).expect("served");
        let ship = corvettes(&reg, me)[0].item_id;
        assert_eq!(reg.character(me).map(|c| c.ship_id), Some(ship));
        let old = reg.item(frigate).expect("kept");
        assert_eq!((old.location_id, old.flag_id), (station, FLAG_HANGAR));
        assert_eq!(reg.item(pod).map(|p| p.location_id), Some(ship));
    }

    #[test]
    fn market_listed_names_only_starter_types_with_a_market_group() {
        let lines = [
            format!("{HULL}\t{GROUP_CORVETTE}\t6\t1999\t0.0"),
            format!("{GUN}\t55\t7\t\t0.0"),
            format!("{MED}\t38\t7\t1998\t0.0"),
            "990870999\t55\t7\t1997\t0.0".to_owned(),
        ];
        assert_eq!(
            market_listed(&table().types(), &lines),
            vec![(MED, 1998), (HULL, 1999)]
        );
    }

    #[test]
    fn a_malformed_races_line_is_an_error_naming_the_file() {
        let mut t = Table::default();
        let e = t
            .parse_races(&["91\tnot-a-ship\t\t1"])
            .expect_err("refused");
        assert!(e.contains(RACES.0), "{e}");
        let e = t.parse_races(&["91\t990870001"]).expect_err("refused");
        assert!(e.contains("data line 1"), "{e}");
    }

    #[test]
    fn the_shipped_fit_parses_with_no_skipped_row() {
        let file = crate::authored::Dir::compiled_only()
            .load(FIT_FILE, SHIPPED_FIT, FIT_HEADER)
            .expect("the shipped file loads");
        let (rows, skipped) = file.parse_rows_logged(|r| Ok((r.i64(0)?, r.i64(1)?)));
        assert!(skipped.is_empty(), "{skipped:?}");
        assert!(!rows.is_empty());
    }
}
