//! T3-E: tactical destroyer stances. **Invented ids, names and numbers**
//! throughout; only the attribute, group and flag ids are the client's (the
//! ids the code under test reads). Nothing from the real mode table is here.

use super::*;
use crate::content::Content;
use crate::service::Registry;

pub(crate) const ACTOR: i64 = 90_000_001;
pub(crate) const SHIP_TYPE: i64 = 990_970_001;
/// A hull of the right group whose name no mode carries.
const BARE_TYPE: i64 = 990_970_002;
/// The three modes of [`SHIP_TYPE`], in [`SUFFIXES`] order.
const MODES: [i64; 3] = [990_970_011, 990_970_012, 990_970_013];
const DEFENCE_EFFECT: i64 = 990_970_901;
const SPEED_EFFECT: i64 = 990_970_902;
/// `modeEmResistancePostDiv` is the mode's own attribute; an invented
/// divisor rides on it.
const DIVISOR_ATTR: i64 = 1997;
const SPEED_ATTR: i64 = 2002;
const ARMOR_EM: i64 = 267;
pub(crate) const AGILITY: i64 = 70;
/// The invented divisor every mode's attribute carries, and the hull's
/// invented resonance and agility.
const RESONANCE_DIVISOR: f64 = 2.0;
const HULL_RESONANCE: f64 = 0.5;
const HULL_AGILITY: f64 = 3.0;
/// The invented `stanceSwitchTime`, ms.
const SWITCH_MS: i64 = 10_000;
const NOW: BlueTime = 130_000_000_000_000_000;

fn mode_row(id: i64, suffix: &str, effect: Option<(i64, i64)>) -> String {
    let (effects, attrs) = match effect {
        Some((e, a)) => (e.to_string(), format!("{a}={}", RESONANCE_DIVISOR)),
        None => (String::new(), String::new()),
    };
    format!(
        "type\t{id}\t{GROUP_STANCE_MODE}\t7\t0\t1\t5.0\t0.0\t100.0\t50.0\t0\t{effects}\t{attrs}\t\
         Fixture Wasp {suffix}\n"
    )
}

pub(crate) fn content() -> Content {
    let mut text = String::from(crate::typedata::FIXTURE);
    text += &format!("attribute\t{AGILITY}\t1.0\tagility\n");
    text += &format!("attribute\t{DIVISOR_ATTR}\t1.0\tmodeEmResistancePostDiv\n");
    text += &format!("attribute\t{SPEED_ATTR}\t1.0\tmodeAgilityPostDiv\n");
    text += &format!("attribute\t{ATTR_STANCE_SWITCH_TIME}\t0.0\tstanceSwitchTime\n");
    text += &format!("effect\t{DEFENCE_EFFECT}\t0\t0\t0\t0\t0\t0\t0\tinventedDefence\t\n");
    text += &format!("effect\t{SPEED_EFFECT}\t0\t0\t0\t0\t0\t0\t0\tinventedSpeed\t\n");
    // PostDiv is operation 5 [V-data: R-T3 §4 item 4].
    text += &format!(
        "modifier\t{DEFENCE_EFFECT}\tshipID\tItemModifier\t{ARMOR_EM}\t{DIVISOR_ATTR}\t5\t\t\n"
    );
    text += &format!(
        "modifier\t{SPEED_EFFECT}\tshipID\tItemModifier\t{AGILITY}\t{SPEED_ATTR}\t5\t\t\n"
    );
    for (id, name) in [(SHIP_TYPE, "Fixture Wasp"), (BARE_TYPE, "Fixture Bare")] {
        text += &format!(
            "type\t{id}\t{GROUP_TACTICAL_DESTROYER}\t6\t1\t1\t100000.0\t0.0\t1200000.0\t50.0\t0\t\t\
             9=500.0;11=45.0;12=3.0;13=3.0;14=3.0;48=125.0;{AGILITY}={HULL_AGILITY};\
             {ARMOR_EM}={HULL_RESONANCE};{ATTR_STANCE_SWITCH_TIME}={SWITCH_MS}.0\t{name}\n"
        );
    }
    text += &mode_row(
        MODES[0],
        SUFFIXES[0].1,
        Some((DEFENCE_EFFECT, DIVISOR_ATTR)),
    );
    text += &mode_row(MODES[1], SUFFIXES[1].1, Some((SPEED_EFFECT, SPEED_ATTR)));
    text += &mode_row(MODES[2], SUFFIXES[2].1, None);
    let table = crate::typedata::Table::parse(&text).expect("the invented rows parse");
    let mut c = Content::fixture();
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

fn fresh() -> Registry {
    Registry::new(crate::model::SandboxConfig::default())
        .with_content(std::sync::Arc::new(content()))
}

/// Alice, docked, flying a new [`SHIP_TYPE`]. The registry and the ship id.
pub(crate) fn world() -> (Registry, i64) {
    let mut reg = fresh();
    let station = reg
        .character(ACTOR)
        .and_then(|c| c.station_id)
        .expect("the fixture character is docked");
    let at = Slot::new(ACTOR, station, crate::model::FLAG_HANGAR);
    reg.apply_items(&[Edit::Create {
        type_id: SHIP_TYPE,
        quantity: 1,
        to: at,
    }])
    .expect("the ship");
    let ship = reg
        .store()
        .items()
        .iter()
        .find(|i| i.type_id == SHIP_TYPE)
        .map(|i| i.item_id)
        .expect("made");
    let c = reg.character_mut(ACTOR).expect("Alice");
    c.ship_id = ship;
    c.ship_type_id = SHIP_TYPE;
    (reg, ship)
}

fn at_156(reg: &Registry, ship: i64) -> Vec<(i64, i64, bool)> {
    reg.store()
        .contents_of(ship)
        .filter(|i| i.flag_id == FLAG_HIDDEN_MODIFIERS)
        .map(|i| (i.item_id, i.type_id, i.singleton))
        .collect()
}

#[test]
fn set_stance_replaces_the_hidden_item() {
    let (mut reg, ship) = world();
    assert!(at_156(&reg, ship).is_empty(), "a new ship has no mode yet");
    let first = set_stance(&mut reg, ACTOR, ship, STANCE_DEFENCE, NOW)
        .expect("allowed")
        .expect("switched");
    assert_eq!((first.old, first.new), (None, STANCE_DEFENCE));
    assert_eq!(at_156(&reg, ship), vec![(first.item_id, MODES[0], true)]);
    assert_eq!(current(&reg, ship), Some(STANCE_DEFENCE));
    // The lock over, the next switch destroys the old item: exactly one.
    let later = NOW + 2 * SWITCH_MS * MSEC;
    let second = set_stance(&mut reg, ACTOR, ship, STANCE_SPEED, later)
        .expect("allowed")
        .expect("switched");
    assert_eq!(
        (second.old, second.new),
        (Some(STANCE_DEFENCE), STANCE_SPEED)
    );
    assert_eq!(at_156(&reg, ship), vec![(second.item_id, MODES[1], true)]);
    assert!(
        reg.store().get(first.item_id).is_none(),
        "the old one is gone"
    );
    // The same stance again is the client's early return: nothing changes.
    let same = set_stance(&mut reg, ACTOR, ship, STANCE_SPEED, later);
    assert_eq!(same, Ok(None));
    assert_eq!(at_156(&reg, ship).len(), 1);
}

#[test]
fn stance_push_names_ship_and_stance() {
    let (mut reg, ship) = world();
    let _ = reg.take_pending();
    set_stance(&mut reg, ACTOR, ship, STANCE_SNIPER, NOW).expect("switched");
    let pushes: Vec<_> = reg
        .take_pending()
        .into_iter()
        .map(|(_, p)| (p.to_push(), p.audience()))
        .collect();
    let want = (
        (
            "OnStanceActive",
            vec![PyValue::Int(ship), PyValue::Int(STANCE_SNIPER)],
        ),
        Some(vec![ACTOR]),
    );
    assert!(pushes.contains(&want), "{pushes:?}");
    // The slim field the client reads (`GetStanceIDFromSlimItem`: the third).
    let field = slim_field(&reg, ship).expect("a stance");
    assert_eq!(
        field,
        PyValue::Tuple(vec![
            PyValue::None,
            PyValue::Int(NOW),
            PyValue::Int(STANCE_SNIPER)
        ])
    );
}

#[test]
fn switch_within_the_lock_is_refused() {
    let (mut reg, ship) = world();
    set_stance(&mut reg, ACTOR, ship, STANCE_DEFENCE, NOW).expect("switched");
    let inside = NOW + SWITCH_MS * MSEC - 1;
    assert_eq!(
        set_stance(&mut reg, ACTOR, ship, STANCE_SPEED, inside),
        Err(Refusal::Changing)
    );
    assert_eq!(current(&reg, ship), Some(STANCE_DEFENCE), "nothing moved");
    // The instant the lock ends the switch goes through.
    let free = NOW + SWITCH_MS * MSEC;
    assert!(set_stance(&mut reg, ACTOR, ship, STANCE_SPEED, free).is_ok());
}

#[test]
fn defence_mode_divides_resonances_by_the_table() {
    let (mut reg, ship) = world();
    let bare = crate::dogma::sheet(&reg, ship).expect("a sheet");
    assert_eq!(bare.ship(ARMOR_EM), HULL_RESONANCE);
    set_stance(&mut reg, ACTOR, ship, STANCE_DEFENCE, NOW).expect("switched");
    let sheet = crate::dogma::sheet(&reg, ship).expect("a sheet");
    // PostDiv: the resonance divided by the mode's number.
    assert_eq!(sheet.ship(ARMOR_EM), HULL_RESONANCE / RESONANCE_DIVISOR);
    assert_eq!(
        sheet.ship(AGILITY),
        HULL_AGILITY,
        "the other mode's field untouched"
    );
    // Another mode takes the first one's effect away with its item.
    set_stance(
        &mut reg,
        ACTOR,
        ship,
        STANCE_SPEED,
        NOW + 2 * SWITCH_MS * MSEC,
    )
    .expect("switched");
    let sheet = crate::dogma::sheet(&reg, ship).expect("a sheet");
    assert_eq!(sheet.ship(ARMOR_EM), HULL_RESONANCE);
    assert_eq!(sheet.ship(AGILITY), HULL_AGILITY / RESONANCE_DIVISOR);
}

#[test]
fn an_unknown_hull_has_no_stances() {
    let (mut reg, ship) = world();
    let c = reg.content();
    // A tactical destroyer no mode is named after, and a hull of another group.
    assert!(stances_of(c, BARE_TYPE).is_empty());
    assert!(stances_of(c, 990_000_001).is_empty(), "a frigate");
    assert!(
        stances_of(c, 1).is_empty(),
        "a type that is not in the table"
    );
    assert_eq!(mode_type(c, SHIP_TYPE, 4), None, "no fourth stance");
    assert_eq!(stances_of(c, SHIP_TYPE).len(), SUFFIXES.len());
    // The call on a hull without stances changes nothing.
    let station = reg
        .character(ACTOR)
        .and_then(|c| c.station_id)
        .expect("docked");
    reg.apply_items(&[Edit::Create {
        type_id: BARE_TYPE,
        quantity: 1,
        to: Slot::new(ACTOR, station, crate::model::FLAG_HANGAR),
    }])
    .expect("a bare hull");
    let bare = reg
        .store()
        .items()
        .iter()
        .find(|i| i.type_id == BARE_TYPE)
        .map(|i| i.item_id)
        .expect("made");
    reg.character_mut(ACTOR).expect("Alice").ship_id = bare;
    assert_eq!(
        set_stance(&mut reg, ACTOR, bare, STANCE_DEFENCE, NOW),
        Err(Refusal::NoSuchStance)
    );
    assert!(at_156(&reg, bare).is_empty());
    // And the caller's ship is the only one: another id is not hers.
    assert_eq!(
        set_stance(&mut reg, ACTOR, ship, STANCE_DEFENCE, NOW),
        Err(Refusal::NotYours)
    );
}

#[test]
fn stance_survives_a_restart() {
    let (mut reg, ship) = world();
    set_stance(&mut reg, ACTOR, ship, STANCE_SNIPER, NOW).expect("switched");
    let dir = std::env::temp_dir().join(format!(
        "eve-t3e-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).expect("a directory to write in");
    let store = crate::store::FileStore::new(dir.join("world.txt"));
    crate::store::Store::save(&store, &reg.to_snapshot()).expect("write the world");
    let snap = crate::store::Store::load(&store)
        .expect("read the world")
        .expect("a world was written");
    let _ = std::fs::remove_dir_all(&dir);
    let mut back = fresh();
    back.apply_snapshot(snap);
    assert_eq!(current(&back, ship), Some(STANCE_SNIPER));
    assert_eq!(at_156(&back, ship).len(), 1);
    assert!(at_156(&back, ship)[0].2, "still a singleton");
    // The slim says where it stands; the lock did not survive, which is fine.
    assert_eq!(
        slim_field(&back, ship),
        Some(PyValue::Tuple(vec![
            PyValue::Int(STANCE_SNIPER),
            PyValue::Int(0),
            PyValue::Int(STANCE_SNIPER)
        ]))
    );
    // The world file holds the ship; the pilot's seat is the fixture's.
    back.character_mut(ACTOR).expect("Alice").ship_id = ship;
    assert_eq!(
        set_stance(&mut back, ACTOR, ship, STANCE_DEFENCE, NOW).map(|d| d.map(|d| d.old)),
        Ok(Some(Some(STANCE_SNIPER)))
    );
}

#[test]
fn a_bot_picks_by_situation() {
    assert_eq!(pick(HURT_AT, Some(1.0), 10.0), STANCE_DEFENCE);
    assert_eq!(pick(0.0, Some(5.0), 10.0), STANCE_SNIPER);
    assert_eq!(pick(0.0, Some(50.0), 10.0), STANCE_SPEED);
    assert_eq!(pick(0.0, None, 10.0), STANCE_SPEED);
}

/// The client's dogma drops the old mode's modifiers only when an
/// `OnItemChange` takes the item off the ship (flag 156 is a fitting flag
/// to it): the replaced mode is told as moved to the junkyard.
#[test]
fn a_replaced_mode_is_told_as_leaving_the_ship() {
    let (mut reg, ship) = world();
    set_stance(&mut reg, ACTOR, ship, STANCE_DEFENCE, NOW)
        .expect("allowed")
        .expect("switched");
    let _ = reg.take_pending();
    let later = NOW + 2 * SWITCH_MS * MSEC;
    set_stance(&mut reg, ACTOR, ship, STANCE_SPEED, later).expect("switched");
    let sent: Vec<_> = reg
        .take_pending()
        .into_iter()
        .map(|(_, p)| p.to_push())
        .filter(|(n, _)| *n == "OnItemChange")
        .collect();
    // The two item changes are the old mode leaving and the new one arriving;
    // only the first has a change dict naming the ship.
    let leaving = |a: &Vec<PyValue>| match &a[1] {
        PyValue::Dict(change) => change.contains(&(
            PyValue::Int(ship),
            PyValue::Int(crate::item::IX_LOCATION_ID),
        )),
        _ => false,
    };
    let (_, args) = sent
        .iter()
        .find(|(_, a)| leaving(a))
        .unwrap_or_else(|| panic!("no item change for the old mode: {sent:?}"));
    let PyValue::Dict(change) = &args[1] else {
        panic!("the change is a dict: {args:?}");
    };
    // A marshal dict holds (value, key).
    assert!(change.contains(&(
        PyValue::Int(ship),
        PyValue::Int(crate::item::IX_LOCATION_ID)
    )));
    assert!(change.contains(&(
        PyValue::Int(FLAG_HIDDEN_MODIFIERS),
        PyValue::Int(crate::item::IX_FLAG)
    )));
}

/// One switch from an existing mode sends one `OnInvalidateCacheForLocations`
/// and two `OnItemChange` (old leaving, new arriving): a second invalidation
/// landed on a hold tracker that had just refetched its container.
#[test]
fn a_switch_invalidates_the_locations_once() {
    let (mut reg, ship) = world();
    set_stance(&mut reg, ACTOR, ship, STANCE_DEFENCE, NOW)
        .expect("allowed")
        .expect("switched");
    let _ = reg.take_pending();
    let later = NOW + 2 * SWITCH_MS * MSEC;
    set_stance(&mut reg, ACTOR, ship, STANCE_SPEED, later).expect("switched");
    let names: Vec<&'static str> = reg
        .take_pending()
        .into_iter()
        .map(|(_, p)| p.to_push().0)
        .collect();
    let n = |want: &str| names.iter().filter(|x| **x == want).count();
    assert_eq!(n("OnInvalidateCacheForLocations"), 1, "{names:?}");
    assert_eq!(n("OnItemChange"), 2, "{names:?}");
}
