//! Where a new character starts: its school's first starting station.
//!
//! Packet P-START-1 (research R-START §2, §5.2). The client's own rule is
//! `characterdata/schools.py` `get_school_starting_station`: the school's
//! `startingStations[Newbie-System-Count - 1 random]`, and that count
//! defaults to 1, so **index 0** **[V]** for the default; whether TQ ever set
//! it higher is R-START Q3. The table is `agents/schools.tsv`
//! (`docs/content/schema.md`), written by `tools/content_agents.py`.
//!
//! The schools a race may pick are its rows that are not starter space: the
//! client's `get_school_ids_by_race_id` filters `is_starter_space_school`
//! **[V]**, and those rows are TQ's instanced starter-space path, whose
//! stations cross factions. [`place_new_character`] refuses them.
//!
//! With no bundle the table is empty: every school is unknown, and a caller
//! keeps the character exactly as `Character::default()` made it.
//!
//! P-START-2 wires it: every creation path (a world account's sign-up and
//! `start_character`, through `Registry::create_character`, and the
//! `--character` roster, through `Registry::start_roster`) places the new
//! character with [`place_or_keep`] and credits `rules.start_isk` ([`STARTING_BALANCE`] by default) with one
//! journal row ([`fund_new_character`]). A character loaded from the world
//! file is never placed or credited again.
//!
//! Lane start-skills (R-START §2.3, Q5): the same table carries each race's
//! starter skills (`agents/races.tsv` `skills`, `races[r].skills` **[V]**),
//! which [`give_starter_skills`] puts on a new character's sheet, and each
//! bloodline's race (`agents/bloodlines.tsv`, `bloodlines[b].raceID`
//! **[V]**), which [`create_with_doll`] checks.

use crate::content::{Boxed, Content, Origin, Sources, TableSpec};
use crate::model::Character;
use crate::service::Registry;
use std::collections::BTreeMap;

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "schools",
    load,
    fixture: || Box::new(Table::default()) as Boxed,
};

/// The bundle file, and the header `schema.md` freezes (P-START-X added
/// `starter_space`).
pub const FILE: (&str, &str) = (
    "agents/schools.tsv",
    "#school_id\tcorp_id\trace_id\tcareer_id\tsystem_id\tstarting_stations\tcareer_agents\tname_id\tstarter_space",
);

/// The bundle's bloodline table, and the header `schema.md` freezes (lane
/// start-skills). The race file is `crate::starter_ship::RACES`, read here
/// for its `skills` column.
pub const BLOODLINES: (&str, &str) = (
    "agents/bloodlines.tsv",
    "#bloodline_id\trace_id\tcorp_id\tname_id",
);

/// The ISK a new character starts with: **1,000,000, the owner's choice**
/// (tracker §83, "1 mil for now"). No client fact exists: the amount is
/// server-side on TQ (R-START §3; `gatekeeper` even A/B-tested it), and
/// EvEJS grants 100,000. The journal row it goes with is
/// [`REF_NEW_CHARACTER_STARTING_FUNDS`].
pub(crate) const STARTING_BALANCE: f64 = 1_000_000.0;

/// `accountingentrytypes` 10004, "New Character Starting Funds" **[V]**
/// (R-START §3): the journal row that explains [`STARTING_BALANCE`].
/// `account.GetEntryTypes` lists it, or the client drops the row.
pub const REF_NEW_CHARACTER_STARTING_FUNDS: i64 = 10_004;

/// **[A]** The starting funds' `ownerID1`: the system, as a GM grant's
/// (`slash.rs`); there is no counterparty. Falsified by a TQ journal row
/// naming the school corporation or CONCORD.
const STARTING_FUNDS_FROM: i64 = 1;

/// One `agents/schools.tsv` row, the columns a start reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct School {
    pub corp_id: i64,
    pub race_id: i64,
    /// `schoolmap`'s system, which agrees with the first station's.
    pub system_id: Option<i64>,
    /// In the client's order: the first is the start.
    pub starting_stations: Vec<i64>,
    pub starter_space: bool,
}

/// School id -> its row; race id -> its starter skills; bloodline id -> its
/// race.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    rows: BTreeMap<i64, School>,
    /// `(skill type id, level)`, sorted by type id; a race with none is
    /// absent.
    skills: BTreeMap<i64, Vec<(i64, i64)>>,
    bloodlines: BTreeMap<i64, i64>,
}

impl Table {
    pub fn get(&self, school_id: i64) -> Option<&School> {
        self.rows.get(&school_id)
    }

    /// The school's start: its **first** listed station (index 0, the
    /// client's default; see the module doc). `None` for an unknown school
    /// or one with no stations.
    pub fn start_station(&self, school_id: i64) -> Option<i64> {
        self.get(school_id)?.starting_stations.first().copied()
    }

    /// The schools a character of `race_id` may start in, sorted: that
    /// race's rows, never a starter-space one.
    pub fn schools_of_race(&self, race_id: i64) -> Vec<i64> {
        self.rows
            .iter()
            .filter(|(_, s)| s.race_id == race_id && !s.starter_space)
            .map(|(id, _)| *id)
            .collect()
    }

    /// The starter skills of `race_id` (`races[r].skills` **[V]**), as
    /// `(type id, level)` sorted by type id; empty for a race with none or
    /// without a bundle.
    pub fn starter_skills(&self, race_id: i64) -> &[(i64, i64)] {
        self.skills.get(&race_id).map_or(&[], Vec::as_slice)
    }

    /// The race of `bloodline_id`, if the bundle lists it.
    pub fn race_of_bloodline(&self, bloodline_id: i64) -> Option<i64> {
        self.bloodlines.get(&bloodline_id).copied()
    }

    /// Whether the bundle listed any bloodline: without `agents/bloodlines.tsv`
    /// the bloodline is not checked (a bundle extracted before lane
    /// start-skills).
    pub fn knows_bloodlines(&self) -> bool {
        !self.bloodlines.is_empty()
    }

    /// From the file's data lines. A malformed line is an error naming it.
    pub fn parse<S: AsRef<str>>(lines: &[S]) -> Result<Table, String> {
        let mut rows = BTreeMap::new();
        for (i, l) in lines.iter().enumerate() {
            let f: Vec<&str> = l.as_ref().split('\t').collect();
            let (id, row) = parse_row(&f)
                .ok_or_else(|| format!("{}: data line {}: {:?}", FILE.0, i + 1, l.as_ref()))?;
            rows.insert(id, row);
        }
        Ok(Table {
            rows,
            ..Table::default()
        })
    }

    /// `agents/races.tsv` data lines (`crate::starter_ship::RACES`): the
    /// `skills` column, `type=level;...`. Only the race id and the skills
    /// are read; the ship is `starter_ship`'s. A malformed line is an error
    /// naming it.
    pub fn parse_races<S: AsRef<str>>(&mut self, lines: &[S]) -> Result<(), String> {
        let file = crate::starter_ship::RACES.0;
        for (i, l) in lines.iter().enumerate() {
            let bad = || format!("{file}: data line {}: {:?}", i + 1, l.as_ref());
            let f: Vec<&str> = l.as_ref().split('\t').collect();
            let [race, _ship, skills, _name] = f[..] else {
                return Err(bad());
            };
            let race: i64 = race.parse().map_err(|_| bad())?;
            let mut map: Vec<(i64, i64)> = skills_map(skills).ok_or_else(bad)?;
            map.sort_unstable();
            if !map.is_empty() {
                self.skills.insert(race, map);
            }
        }
        Ok(())
    }

    /// `agents/bloodlines.tsv` data lines ([`BLOODLINES`]). A malformed line
    /// is an error naming it.
    pub fn parse_bloodlines<S: AsRef<str>>(&mut self, lines: &[S]) -> Result<(), String> {
        for (i, l) in lines.iter().enumerate() {
            let bad = || format!("{}: data line {}: {:?}", BLOODLINES.0, i + 1, l.as_ref());
            let f: Vec<&str> = l.as_ref().split('\t').collect();
            let [bloodline, race, corp, name] = f[..] else {
                return Err(bad());
            };
            corp.parse::<i64>().map_err(|_| bad())?;
            name.parse::<i64>().map_err(|_| bad())?;
            self.bloodlines.insert(
                bloodline.parse().map_err(|_| bad())?,
                race.parse().map_err(|_| bad())?,
            );
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn insert(&mut self, school_id: i64, row: School) {
        self.rows.insert(school_id, row);
    }
}

fn opt(s: &str) -> Option<Option<i64>> {
    if s.is_empty() {
        Some(None)
    } else {
        s.parse().ok().map(Some)
    }
}

fn list(s: &str) -> Option<Vec<i64>> {
    if s.is_empty() {
        return Some(Vec::new());
    }
    s.split(',').map(|x| x.parse().ok()).collect()
}

/// `type=level;type=level` (`schema.md` `map<i64,i64>`); empty is empty.
fn skills_map(s: &str) -> Option<Vec<(i64, i64)>> {
    if s.is_empty() {
        return Some(Vec::new());
    }
    s.split(';')
        .map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((k.parse().ok()?, v.parse().ok()?))
        })
        .collect()
}

fn parse_row(f: &[&str]) -> Option<(i64, School)> {
    let [
        id,
        corp,
        race,
        career,
        system,
        stations,
        agents,
        name,
        starter,
    ] = f[..]
    else {
        return None;
    };
    opt(career)?;
    list(agents)?;
    name.parse::<i64>().ok()?;
    Some((
        id.parse().ok()?,
        School {
            corp_id: corp.parse().ok()?,
            race_id: race.parse().ok()?,
            system_id: opt(system)?,
            starting_stations: list(stations)?,
            starter_space: match starter {
                "0" => false,
                "1" => true,
                _ => return None,
            },
        },
    ))
}

fn load(src: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => {
            src.note(Origin::Bundle);
            let mut t = Table::parse(&b.data_lines(FILE.0, FILE.1)?)?;
            let races = crate::starter_ship::RACES;
            t.parse_races(&b.data_lines(races.0, races.1)?)?;
            t.parse_bloodlines(&b.data_lines(BLOODLINES.0, BLOODLINES.1)?)?;
            t
        }
        None => Table::default(),
    }))
}

/// Who a new character is, as its creator chose: the race, bloodline and
/// gender, and the school (`None`: the race's [`default_school_for_race`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewCharacter {
    pub race_id: i64,
    pub bloodline_id: i64,
    pub gender_id: i64,
    pub school_id: Option<i64>,
}

impl NewCharacter {
    /// **[A]** A character whose creator chose nothing: a world account's
    /// sign-up (the login token carries a name and a password only) and a
    /// `start_character`. `Character::default`'s race, bloodline and gender,
    /// and that race's default school. Falsified by the owner wanting
    /// sign-up characters spread over the races; in-game creation
    /// (`CreateCharacterWithDoll`) carries the player's own choice.
    pub fn unchosen() -> Self {
        let c = Character::default();
        NewCharacter {
            race_id: c.race_id,
            bloodline_id: c.bloodline_id,
            gender_id: c.gender_id,
            school_id: None,
        }
    }
}

/// Why a character could not be placed. The character is then untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    /// No such school (or no bundle: the table is empty).
    UnknownSchool(i64),
    /// A starter-space school, which the client never offers a race and
    /// whose stations cross factions (R-START §2.1).
    StarterSpace(i64),
    /// The school lists no station.
    NoStation(i64),
    /// The first station is in neither the seeded stations nor the
    /// catalogue and `stations/stations.tsv`.
    UnknownStation { school_id: i64, station_id: i64 },
    /// The school is another race's (`CreateCharacterWithDoll` only: the
    /// client offers a race its own schools **[V]**).
    WrongRace { school_id: i64, race_id: i64 },
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::UnknownSchool(s) => write!(f, "school {s} is not in {}", FILE.0),
            StartError::StarterSpace(s) => write!(f, "school {s} is a starter-space school"),
            StartError::NoStation(s) => write!(f, "school {s} lists no starting station"),
            StartError::UnknownStation {
                school_id,
                station_id,
            } => write!(
                f,
                "school {school_id}'s first station {station_id} is not a known station"
            ),
            StartError::WrongRace { school_id, race_id } => {
                write!(f, "school {school_id} is not a school of race {race_id}")
            }
        }
    }
}

/// Put a new character at `school_id`'s start: its first starting station
/// (all seven station fields, through `Registry::station_ref` and the
/// store's `StationRecord::seat`), and its corporation the school's
/// corporation **[I]** (the client calls it "the character's school corp",
/// `ccSvc.GetSchoolAndSchoolCorpInfoForChar`; EvEJS's starter corps are the
/// school corps, R-START §5.2).
///
/// Called on a character **before** `Registry::seed_character`, so its
/// capsule is created at the new station (`Registry::start_roster` moves
/// an already-seeded one). Nothing else is changed but `school_id`, the
/// wallet included. On an error the character is untouched.
pub(crate) fn place_new_character(
    reg: &Registry,
    character: &mut Character,
    school_id: i64,
) -> Result<(), StartError> {
    let table = reg.content().get::<Table>();
    let school = table
        .get(school_id)
        .ok_or(StartError::UnknownSchool(school_id))?;
    if school.starter_space {
        return Err(StartError::StarterSpace(school_id));
    }
    let station_id = table
        .start_station(school_id)
        .ok_or(StartError::NoStation(school_id))?;
    let s = reg
        .station_ref(station_id)
        .ok_or(StartError::UnknownStation {
            school_id,
            station_id,
        })?;
    crate::store::StationRecord {
        station_id: s.station_id,
        owner_id: s.owner_id,
        type_id: s.type_id,
        operation_id: s.operation_id,
        solar_system_id: s.solar_system_id,
        constellation_id: s.constellation_id,
        region_id: s.region_id,
    }
    .seat(character);
    character.corporation_id = school.corp_id;
    character.school_id = school_id;
    Ok(())
}

/// [`place_new_character`] at `school`, or, with none given, at
/// [`default_school_for_race`] of the character's race. A failure (no
/// bundle, an unknown or starter-space school) logs one line and leaves the
/// character exactly as it was: a sandbox without a bundle starts where
/// `Character::default` puts it, as before P-START-2. True when placed.
pub(crate) fn place_or_keep(
    reg: &Registry,
    character: &mut Character,
    school: Option<i64>,
) -> bool {
    let Some(school_id) =
        school.or_else(|| default_school_for_race(reg.content(), character.race_id))
    else {
        crate::errln!(
            "[start] {:?}: race {} has no school in {} (no bundle?); kept at station {:?}",
            character.name,
            character.race_id,
            FILE.0,
            character.station_id
        );
        return false;
    };
    match place_new_character(reg, character, school_id) {
        Ok(()) => true,
        Err(e) => {
            crate::errln!(
                "[start] {:?}: {e}; kept at station {:?}",
                character.name,
                character.station_id
            );
            false
        }
    }
}

/// Credit a new character `rules.start_isk` ([`STARTING_BALANCE`] by default), with one
/// [`REF_NEW_CHARACTER_STARTING_FUNDS`] journal row dated `now`, and mark
/// the world dirty. Called once per character, at creation; nothing calls
/// it for a character loaded from the world file.
pub(crate) fn fund_new_character(reg: &mut Registry, character_id: i64, now: i64) {
    // Born now: `Character::default`'s date is 2003, which every age-limited
    // accelerator refuses. Saved as a delta so a reload keeps it.
    reg.deltas_mut().put(
        BORN_KIND,
        vec![character_id.to_string()],
        vec![now.to_string()],
    );
    let amount = if reg.config().dev {
        DEV_BALANCE
    } else {
        reg.config().rules.start_isk
    };
    let start_sp = reg.config().rules.start_sp;
    let Some(c) = reg.character_mut(character_id) else {
        return;
    };
    c.create_date_time = now;
    c.balance += amount;
    let balance = c.balance;
    reg.journal_mut().record(
        now,
        REF_NEW_CHARACTER_STARTING_FUNDS,
        STARTING_FUNDS_FROM,
        character_id,
        0,
        amount,
        balance,
        "",
    );
    if reg.config().dev {
        // Funded at creation: no top-up row is owed later.
        reg.deltas_mut().put(
            DEV_TOPUP_KIND,
            vec![character_id.to_string()],
            vec!["1".to_owned()],
        );
    }
    // The world rule `start_sp`, in dev too. Not every path reaches here: a default
    // `--dev` pilot is topped up by `dev_top_up`, which grants it as well.
    crate::free_sp::grant(reg, character_id, start_sp);
    reg.mark_dirty();
}

/// The delta kind that records when a character was made: `char -> FILETIME`.
pub const BORN_KIND: &str = "charborn";

/// Give each character its saved birth date (a character with no row keeps
/// the default). Called by `apply_snapshot` after the roster is seated.
pub(crate) fn apply_births(reg: &mut Registry) {
    let born: Vec<(i64, i64)> = reg
        .deltas()
        .of(BORN_KIND)
        .iter()
        .filter_map(|(k, v)| Some((k.first()?.parse().ok()?, v.first()?.parse().ok()?)))
        .collect();
    for (id, at) in born {
        if let Some(c) = reg.character_mut(id) {
            c.create_date_time = at;
        }
    }
}

/// The delta kind of a character put up for deletion or deleted (SOC-CHARDEL):
/// `char -> due done`. `due` is the FILETIME from which the delete is allowed;
/// `done` is `1` once deleted. A tombstone: the character row, its name and
/// everything that resolves it (mail, killmails, corp history, assets and
/// orders left as they are) stay; only the select list and
/// `SelectCharacterID` drop it. Whether the name is freed is the owner's
/// call; it is not (default, reversible).
pub const DELETE_KIND: &str = "chardel";

/// Why a delete step was refused. All of them reach the client as a
/// `UserError` raised by the service (`DeleteCharacter`'s return value is
/// shown after the "end" dialog, so a refusal is raised instead).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteRefusal {
    /// Not a character this caller may play, or already deleted.
    NotYours,
    /// A pilot who is logged in cannot be deleted.
    Online,
    /// The wait has not run, or the character was never put up for deletion.
    TooEarly,
    /// The CEO of a player corporation hands the corp on first.
    Ceo,
}

impl DeleteRefusal {
    /// The client's own message keys, both with no arguments **[V]**
    /// (`findings/user-errors.tsv`: `CanNotDeleteCEO`,
    /// `CanNotDeleteCharacterForUnknownReason`).
    pub fn key(self) -> &'static str {
        match self {
            DeleteRefusal::Ceo => "CanNotDeleteCEO",
            _ => "CanNotDeleteCharacterForUnknownReason",
        }
    }
}

fn delete_row(d: &crate::delta::Deltas, character_id: i64) -> Option<(i64, bool)> {
    let row = d.of(DELETE_KIND).get(&[character_id.to_string()][..])?;
    let due = row.first()?.parse().ok()?;
    Some((due, row.get(1).is_some_and(|v| v == "1")))
}

/// Whether `character_id` has been deleted (a tombstone). Cheap when nobody has.
pub fn is_deleted(d: &crate::delta::Deltas, character_id: i64) -> bool {
    !d.of(DELETE_KIND).is_empty() && delete_row(d, character_id).is_some_and(|(_, done)| done)
}

/// The FILETIME a waiting character may be deleted from, or `None`: the
/// select row's `deletePrepareDateTime`.
pub fn delete_due(d: &crate::delta::Deltas, character_id: i64) -> Option<i64> {
    match delete_row(d, character_id)? {
        (due, false) => Some(due),
        _ => None,
    }
}

/// Whose character it is: the part of the checks every one of the three calls
/// makes, so a waiting pilot made CEO can still cancel.
fn may_touch(
    reg: &Registry,
    caller: crate::service::Caller,
    character_id: i64,
) -> Result<(), DeleteRefusal> {
    let c = reg.character(character_id).ok_or(DeleteRefusal::NotYours)?;
    // A bot is never removed through the select screen.
    if c.is_bot || !reg.may_play(caller, c) {
        return Err(DeleteRefusal::NotYours);
    }
    Ok(())
}

/// What a caller may start or finish: [`may_touch`], and not a CEO.
fn may_remove(
    reg: &Registry,
    caller: crate::service::Caller,
    character_id: i64,
) -> Result<(), DeleteRefusal> {
    may_touch(reg, caller, character_id)?;
    if crate::service::corp::is_ceo(reg, character_id) {
        return Err(DeleteRefusal::Ceo);
    }
    Ok(())
}

/// `PrepareCharacterForDelete`: put the character up for deletion and answer
/// the FILETIME from which `DeleteCharacter` is allowed (`now` plus
/// `rules.char_delete_secs`). Asked again, the first time stands.
pub fn prepare_delete(
    reg: &mut Registry,
    caller: crate::service::Caller,
    character_id: i64,
    now: i64,
) -> Result<i64, DeleteRefusal> {
    may_remove(reg, caller, character_id)?;
    if let Some(due) = delete_due(reg.deltas(), character_id) {
        return Ok(due);
    }
    let due = now.saturating_add(
        reg.config()
            .rules
            .char_delete_secs
            .saturating_mul(10_000_000),
    );
    reg.deltas_mut().put(
        DELETE_KIND,
        vec![character_id.to_string()],
        vec![due.to_string(), "0".to_owned()],
    );
    reg.mark_dirty();
    Ok(due)
}

/// `CancelCharacterDeletePrepare`: take the character off the wait. A
/// character that is not waiting is left alone.
pub fn cancel_delete(
    reg: &mut Registry,
    caller: crate::service::Caller,
    character_id: i64,
) -> Result<(), DeleteRefusal> {
    may_touch(reg, caller, character_id)?;
    if delete_due(reg.deltas(), character_id).is_some() {
        reg.deltas_mut()
            .del(DELETE_KIND, &[character_id.to_string()]);
        reg.mark_dirty();
    }
    Ok(())
}

/// `DeleteCharacter`: tombstone a character whose wait has run. It leaves its
/// player corporation like an expelled member; its items, orders and ships
/// stay where they are (abandoned), and its name stays taken.
pub fn delete_character(
    reg: &mut Registry,
    caller: crate::service::Caller,
    character_id: i64,
    now: i64,
) -> Result<(), DeleteRefusal> {
    may_remove(reg, caller, character_id)?;
    if reg.sessions().is_online(character_id) {
        return Err(DeleteRefusal::Online);
    }
    match delete_due(reg.deltas(), character_id) {
        Some(due) if due <= now => {}
        _ => return Err(DeleteRefusal::TooEarly),
    }
    crate::service::corp::leave_for_delete(reg, character_id);
    let due = delete_due(reg.deltas(), character_id).unwrap_or(now);
    reg.deltas_mut().put(
        DELETE_KIND,
        vec![character_id.to_string()],
        vec![due.to_string(), "1".to_owned()],
    );
    reg.mark_dirty();
    Ok(())
}

/// What a character starts with under `--dev` (DEV-MODE).
pub const DEV_BALANCE: f64 = 1_000_000_000.0;
/// The delta kind that records a dev top-up: one row per character.
pub const DEV_TOPUP_KIND: &str = "devtopup";

/// `--dev`: bring every character of a dev world that was not funded here
/// up to [`DEV_BALANCE`], once (a row in [`DEV_TOPUP_KIND`] says it was done),
/// with a journal row. Called by `Server::bind` after the world loads.
pub(crate) fn dev_top_up(reg: &mut Registry, now: i64) -> usize {
    let start_sp = reg.config().rules.start_sp;
    let ids: Vec<i64> = reg
        .config()
        .characters
        .iter()
        .map(|c| c.character_id)
        .collect();
    let mut n = 0;
    for id in ids {
        let key = vec![id.to_string()];
        if reg.deltas().of(DEV_TOPUP_KIND).get(&key[..]).is_some() {
            continue;
        }
        let Some(c) = reg.character_mut(id) else {
            continue;
        };
        let add = (DEV_BALANCE - c.balance).max(0.0);
        c.balance += add;
        let balance = c.balance;
        reg.journal_mut().record(
            now,
            REF_NEW_CHARACTER_STARTING_FUNDS,
            STARTING_FUNDS_FROM,
            id,
            0,
            add,
            balance,
            "dev top-up",
        );
        reg.deltas_mut()
            .put(DEV_TOPUP_KIND, key, vec!["1".to_owned()]);
        // A default `--dev` pilot (no `--character`) never went through
        // `fund_new_character`, so the world rule `start_sp` reaches it here,
        // once, under the same row (fci_free_sp).
        crate::free_sp::grant(reg, id, start_sp);
        n += 1;
    }
    n
}

/// Give a new character its race's starter skills (`races[r].skills`
/// **[V]**, R-START Q5): the sheet is replaced by exactly that map, each
/// skill at its level with the client's points for it
/// (`skill::Skills::grant_type`); a level-0 entry is injected untrained.
/// A race with no starter skills (or no bundle) keeps the sheet
/// `Registry::seed_character` gave it (`skill::Skills::starter_in`). A skill
/// the skill table does not know is left out and logged. Called once, at
/// creation, by `Registry::create_character`; the sheet persists with the
/// world as every skill sheet does. True when the race's map was applied.
pub(crate) fn give_starter_skills(reg: &mut Registry, character_id: i64, race_id: i64) -> bool {
    let map = reg
        .content()
        .get::<Table>()
        .starter_skills(race_id)
        .to_vec();
    if map.is_empty() {
        return false;
    }
    let mut sheet = crate::skill::Skills::new();
    let mut unknown = Vec::new();
    for (type_id, level) in map {
        match crate::skill::find_in(reg.content(), type_id) {
            Some(t) => {
                sheet.grant_type(t, level);
            }
            None => unknown.push(type_id),
        }
    }
    if !unknown.is_empty() {
        crate::errln!(
            "[start] race {race_id}: {} starter skills not in the skill table, left out: {unknown:?}",
            unknown.len()
        );
    }
    let Some(s) = reg.skills_mut(character_id) else {
        return false;
    };
    *s = sheet;
    reg.mark_dirty();
    true
}

/// **[A]** The school of a character the server makes for a race: the
/// lowest school id that race may start in (R-START Q2: the owner picks
/// between a fixed default, a `,school=N` option and a seeded draw within
/// the race). The client itself draws at random among the same schools
/// (`characterNaming.SetSchool`) **[V]**. Falsified by the owner choosing
/// another rule. `None` for a race with no school (or no bundle).
pub(crate) fn default_school_for_race(content: &Content, race_id: i64) -> Option<i64> {
    content
        .get::<Table>()
        .schools_of_race(race_id)
        .first()
        .copied()
}

/// `charUnboundMgr.ValidateNameEx`'s answer for a good name: the client's
/// `VALID` **[V]** (`charcreation/steps/sections/chooseNameSection.py`).
pub const NAME_VALID: i64 = 1;
/// Its failure codes **[V]** (the same file's `invalidNameReasonNamesByID`:
/// -1 too short, -2 too long, -5 illegal character, -7 consecutive spaces,
/// -101 unavailable). -6 (too many spaces) is never sent: no rule here
/// counts spaces.
pub const NAME_TOO_SHORT: i64 = -1;
pub const NAME_TOO_LONG: i64 = -2;
pub const NAME_ILLEGAL_CHARACTER: i64 = -5;
pub const NAME_CONSECUTIVE_SPACES: i64 = -7;
pub const NAME_UNAVAILABLE: i64 = -101;

/// A name's shape, by the one rule this server has for a visible name
/// (`account_book`'s sign-up rule, which calls this): ASCII letters, digits,
/// space, `-`, `_` and `'`, no leading or trailing space, no doubled space,
/// `account_book::NAME_LEN` bytes long. [`NAME_VALID`] or the client's code
/// for the first rule broken.
pub fn name_shape(name: &str) -> i64 {
    let len = crate::account_book::NAME_LEN;
    let ok_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | '\'');
    if name.len() < *len.start() {
        NAME_TOO_SHORT
    } else if name.len() > *len.end() {
        NAME_TOO_LONG
    } else if !name.chars().all(ok_char) || name.starts_with(' ') || name.ends_with(' ') {
        NAME_ILLEGAL_CHARACTER
    } else if name.contains("  ") {
        NAME_CONSECUTIVE_SPACES
    } else {
        NAME_VALID
    }
}

/// `ValidateNameEx`: [`name_shape`], then [`NAME_UNAVAILABLE`] for a name
/// that is already a character's or an account's (ignoring ASCII case, as
/// sign-up does) or one of our tools' tokens (ruling 2).
pub(crate) fn validate_name(reg: &Registry, name: &str) -> i64 {
    let shape = name_shape(name);
    if shape != NAME_VALID {
        return shape;
    }
    let taken = reg
        .config()
        .characters
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case(name))
        || crate::account_book::find(reg.deltas(), name).is_some()
        || crate::accounts::TOOL_TOKENS
            .iter()
            .any(|t| t.eq_ignore_ascii_case(name));
    if taken { NAME_UNAVAILABLE } else { NAME_VALID }
}

/// The bot-name flavour of a client race id (`appConst` **[V]**: Caldari 1,
/// Minmatar 2, Amarr 4, Gallente 8), or `None` for any other id.
fn race_flavour(race_id: i64) -> Option<&'static str> {
    match race_id {
        1 => Some("caldari"),
        2 => Some("minmatar"),
        4 => Some("amarr"),
        8 => Some("gallente"),
        _ => None,
    }
}

/// `charUnboundMgr.GetValidRandomName(raceID)`: a name the creator's
/// Randomize button may fill in, valid and free **[V]** (the client marks
/// the answer valid without asking `ValidateNameEx`). It starts at `seed`
/// in the race's shipped pool (`bot_names::pool_of`; an id that is no race
/// draws from all four) and walks to the first name [`validate_name`]
/// passes. A used-up pool falls back to a pool name with digits, so the
/// answer is never a name the server would refuse.
pub(crate) fn random_name(reg: &Registry, race_id: i64, seed: u64) -> String {
    const ALL: [&str; 4] = ["caldari", "minmatar", "amarr", "gallente"];
    let flavour = race_flavour(race_id).unwrap_or(ALL[(seed % 4) as usize]);
    let pool = crate::bot_names::pool_of(flavour);
    let n = pool.len();
    let first = if n == 0 {
        0
    } else {
        (seed / 4 % n as u64) as usize
    };
    for k in 0..n {
        let name = &pool[(first + k) % n];
        if validate_name(reg, name) == NAME_VALID {
            return name.clone();
        }
    }
    let base = pool.get(first).map_or("Pilot", String::as_str);
    let len = *crate::account_book::NAME_LEN.end();
    (1u64..)
        .map(|d| {
            let digits = d.to_string();
            let keep = len.saturating_sub(digits.len() + 1).min(base.len());
            format!("{}{digits}", base[..keep].trim_end())
        })
        .find(|c| validate_name(reg, c) == NAME_VALID)
        .unwrap_or_else(|| base.to_owned())
}

/// The `UserError` key `CreateCharacterWithDoll` raises for a name
/// [`validate_name`] refuses **[V: the keys are the client's,
/// `findings/user-errors.tsv`]**. The legacy creator catches a
/// `CharNameInvalid*` itself and shows "failed for some reason"
/// (`characterCreationContainer`, the `CreateCharacterWithDoll` caller)
/// **[V]**; the name step's `ValidateNameEx` has already told the player.
pub fn name_error_key(code: i64) -> &'static str {
    match code {
        NAME_TOO_SHORT => "CharNameInvalidMinLength",
        NAME_TOO_LONG => "CharNameInvalidMaxLength",
        NAME_ILLEGAL_CHARACTER => "CharNameInvalidSomeChar",
        NAME_UNAVAILABLE => "CharNameInvalidTaken",
        _ => "CharNameInvalid",
    }
}

/// **[A]** The `UserError` key for a school the server refuses (unknown,
/// starter space, another race's, or not placeable): R-START Q6 found no
/// client key for exactly this. `CharCreationInvalidFaction` is the
/// client's own character-creation error dialog (an `error`, `modal`
/// entry of build 3619879's dialogs), and anything but `CharNameInvalid*`
/// reaches the player as a dialog (`prompt_player_fsd_dialog`) **[V]**.
/// Falsified by a TQ capture of the refusal, or by the dialog's text
/// reading wrong for a school.
pub const SCHOOL_ERROR_KEY: &str = "CharCreationInvalidFaction";

/// The key a `CreateCharacterWithDoll` whose bloodline is not of its race
/// raises. The key is the client's own **[V]** (`findings/user-errors.tsv`:
/// `CCMustSelectRaceAndBloodline`, which `characterCreationContainer.py`
/// raises itself when the pair is incomplete); that TQ raised it for a
/// mismatched pair is **[A]**, as for [`SCHOOL_ERROR_KEY`]. Falsified by a
/// TQ capture of the refusal.
pub const BLOODLINE_ERROR_KEY: &str = "CCMustSelectRaceAndBloodline";

/// Why `CreateCharacterWithDoll` refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateError {
    /// A [`validate_name`] code other than [`NAME_VALID`].
    Name(i64),
    /// The school is unknown, starter space, unplaceable, or another
    /// race's.
    School(StartError),
    /// The bloodline is unknown, or of another race (`bloodlines[b].raceID`
    /// **[V]**).
    Bloodline { bloodline_id: i64, race_id: i64 },
}

impl CreateError {
    /// The `UserError` key the client is sent.
    pub fn error_key(&self) -> &'static str {
        match self {
            CreateError::Name(code) => name_error_key(*code),
            CreateError::School(_) => SCHOOL_ERROR_KEY,
            CreateError::Bloodline { .. } => BLOODLINE_ERROR_KEY,
        }
    }
}

/// `charUnboundMgr.CreateCharacterWithDoll` (P-START-4): a new character
/// owned by `user_id`, with the creator's race, bloodline, gender and
/// school, placed and funded as every new character is
/// (`Registry::create_character`). The school must be the race's and
/// placeable: it is checked by placing a probe first, so a refusal creates
/// nothing. The bloodline must be of the race (`agents/bloodlines.tsv`); a
/// bundle without that file checks no bloodline.
pub(crate) fn create_with_doll(
    reg: &mut Registry,
    user_id: i64,
    name: &str,
    new: NewCharacter,
) -> Result<i64, CreateError> {
    let code = validate_name(reg, name);
    if code != NAME_VALID {
        return Err(CreateError::Name(code));
    }
    let table = reg.content().get::<Table>();
    if table.knows_bloodlines() && table.race_of_bloodline(new.bloodline_id) != Some(new.race_id) {
        return Err(CreateError::Bloodline {
            bloodline_id: new.bloodline_id,
            race_id: new.race_id,
        });
    }
    let school_id = new
        .school_id
        .ok_or(CreateError::School(StartError::UnknownSchool(0)))?;
    let race = reg
        .content()
        .get::<Table>()
        .get(school_id)
        .map(|s| s.race_id);
    match race {
        Some(r) if r != new.race_id => {
            return Err(CreateError::School(StartError::WrongRace {
                school_id,
                race_id: new.race_id,
            }));
        }
        _ => {}
    }
    let mut probe = Character::default();
    place_new_character(reg, &mut probe, school_id).map_err(CreateError::School)?;
    Ok(reg.create_character(user_id, name.to_owned(), new))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids only (`schema.md` §1.1), never a client value.
    const SCHOOL: i64 = 99;
    const CORP: i64 = 1_999_001;
    const RACE: i64 = 91;
    const FIRST: i64 = 69_000_001;
    const SECOND: i64 = 69_000_002;
    const SYSTEM: i64 = 39_000_001;

    fn school(race_id: i64, stations: &[i64], starter_space: bool) -> School {
        School {
            corp_id: CORP,
            race_id,
            system_id: Some(SYSTEM),
            starting_stations: stations.to_vec(),
            starter_space,
        }
    }

    fn table() -> Table {
        let mut t = Table::default();
        t.insert(SCHOOL, school(RACE, &[FIRST, SECOND], false));
        t.insert(97, school(RACE, &[FIRST], false));
        t.insert(98, school(RACE, &[SECOND], true));
        t.insert(96, school(92, &[FIRST], false));
        t.insert(95, school(RACE, &[], false));
        t
    }

    /// A registry whose catalogue and `stations/stations.tsv` know the two
    /// invented stations, and whose schools are [`table`].
    fn registry() -> Registry {
        registry_from(table(), crate::model::SandboxConfig::default())
    }

    /// The same on `schools` and `config` (its characters seeded first).
    fn registry_from(schools: Table, config: crate::model::SandboxConfig) -> Registry {
        let catalogue = crate::celestial::Catalogue::parse(&format!(
            "#kind\tid\ttype_id\tradius\tx\ty\tz\tsystem\tdest\tdest_system\n\
             #system\t{SYSTEM}\t29000001\t19000001\n\
             station\t{FIRST}\t1529\t0.0\t1.0\t2.0\t3.0\t{SYSTEM}\t0\t0\n\
             station\t{SECOND}\t1530\t0.0\t4.0\t5.0\t6.0\t{SYSTEM}\t0\t0\n"
        ));
        let mut stations = crate::stations::Table::default();
        for (id, owner) in [(FIRST, 1_999_101), (SECOND, 1_999_102)] {
            stations.insert(
                id,
                crate::stations::Station {
                    system_id: SYSTEM,
                    region_id: 19_000_001,
                    owner_id: owner,
                    operation_id: 99,
                },
            );
        }
        let mut content = Content::fixture();
        content.insert("stations", stations);
        content.insert("schools", schools);
        Registry::new(crate::model::SandboxConfig {
            celestials: catalogue,
            ..config
        })
        .with_content(std::sync::Arc::new(content))
    }

    /// A school for `Character::default()`'s race (whatever the [A] value
    /// is), so the unchosen path has somewhere to go in [`registry`].
    const UNCHOSEN_SCHOOL: i64 = 93;

    fn table_with_unchosen() -> Table {
        let mut t = table();
        t.insert(
            UNCHOSEN_SCHOOL,
            school(Character::default().race_id, &[SECOND], false),
        );
        t
    }

    fn registry_with_unchosen() -> Registry {
        registry_from(
            table_with_unchosen(),
            crate::model::SandboxConfig::default(),
        )
    }

    fn journal_of(reg: &Registry, id: i64) -> Vec<crate::ledger::Entry> {
        reg.journal()
            .entries()
            .iter()
            .filter(|e| e.owner_id_2 == id || e.owner_id_1 == id)
            .cloned()
            .collect()
    }

    /// One starting-funds row, for `STARTING_BALANCE`, and the balance it
    /// leaves.
    fn assert_funded_once(reg: &Registry, id: i64) {
        let c = reg.character(id).expect("a character");
        assert_eq!(c.balance, STARTING_BALANCE, "balance");
        let rows = journal_of(reg, id);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].entry_type_id, REF_NEW_CHARACTER_STARTING_FUNDS);
        assert_eq!(rows[0].amount, STARTING_BALANCE);
        assert_eq!(rows[0].balance, STARTING_BALANCE);
    }

    /// `rules.start_isk` is what a new character is credited, in both the
    /// balance and the journal row; the default is `STARTING_BALANCE`.
    #[test]
    fn the_start_isk_rule_sets_the_starting_funds() {
        let config = crate::model::SandboxConfig {
            rules: crate::rules::Rules {
                start_isk: 12_345.0,
                ..Default::default()
            },
            ..crate::model::SandboxConfig::default()
        };
        let mut reg = registry_from(table(), config);
        let id = reg.create_character(
            1001,
            "Invented Pilot".to_owned(),
            NewCharacter {
                race_id: RACE,
                bloodline_id: 901,
                gender_id: 0,
                school_id: Some(SCHOOL),
            },
        );
        assert_eq!(reg.character(id).unwrap().balance, 12_345.0);
        let rows = journal_of(&reg, id);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].amount, 12_345.0);
    }

    /// `rules.start_sp` becomes the new character's free skill points; the
    /// default leaves the pool empty.
    #[test]
    fn the_start_sp_rule_grants_free_skill_points() {
        let new = || NewCharacter {
            race_id: RACE,
            bloodline_id: 901,
            gender_id: 0,
            school_id: Some(SCHOOL),
        };
        let config = crate::model::SandboxConfig {
            rules: crate::rules::Rules {
                start_sp: 4_321,
                ..Default::default()
            },
            ..crate::model::SandboxConfig::default()
        };
        let mut reg = registry_from(table(), config);
        let id = reg.create_character(1001, "Invented Pilot".to_owned(), new());
        assert_eq!(crate::free_sp::of(&reg, id), 4_321);
        let mut plain = registry();
        let id = plain.create_character(1001, "Invented Pilot".to_owned(), new());
        assert_eq!(crate::free_sp::of(&plain, id), 0);
    }

    /// A default `--dev` pilot is only topped up, never created here, so the
    /// top-up carries `start_sp` too, once.
    #[test]
    fn the_dev_top_up_grants_start_sp_once() {
        let config = crate::model::SandboxConfig {
            dev: true,
            rules: crate::rules::Rules {
                start_sp: 4_321,
                ..Default::default()
            },
            ..crate::model::SandboxConfig::default()
        };
        let mut reg = registry_from(table(), config);
        let first = reg.config().characters[0].character_id;
        assert_eq!(crate::free_sp::of(&reg, first), 0);
        assert_eq!(dev_top_up(&mut reg, 7), 1);
        assert_eq!(crate::free_sp::of(&reg, first), 4_321);
        assert_eq!(dev_top_up(&mut reg, 8), 0);
        assert_eq!(crate::free_sp::of(&reg, first), 4_321, "once");
    }

    #[test]
    fn a_created_character_starts_at_its_school_with_the_starting_funds_once() {
        let mut reg = registry();
        let id = reg.create_character(
            1001,
            "Invented Pilot".to_owned(),
            NewCharacter {
                race_id: RACE,
                bloodline_id: 901,
                gender_id: 0,
                school_id: Some(SCHOOL),
            },
        );
        let c = reg.character(id).expect("created");
        assert_eq!(
            (
                c.station_id,
                c.solar_system_id,
                c.corporation_id,
                c.school_id
            ),
            (Some(FIRST), Some(SYSTEM), CORP, SCHOOL)
        );
        assert_eq!((c.race_id, c.bloodline_id, c.gender_id), (RACE, 901, 0));
        assert_eq!(c.owner_user_id, Some(1001));
        let capsule = reg.store().get(c.ship_id).expect("its capsule");
        assert_eq!(capsule.location_id, FIRST, "the capsule is at the school");
        assert_funded_once(&reg, id);
    }

    /// **DEV-MODE**: under `--dev` a new character starts with a billion and
    /// is not topped up again; an existing one is topped up once, with a
    /// journal row; a normal world never is.
    #[test]
    fn a_dev_world_funds_a_new_character_to_a_billion_and_tops_up_once() {
        let config = crate::model::SandboxConfig {
            dev: true,
            ..crate::model::SandboxConfig::default()
        };
        let mut reg = registry_from(table(), config);
        let id = reg.create_character(
            1001,
            "Invented Pilot".to_owned(),
            NewCharacter {
                race_id: RACE,
                bloodline_id: 901,
                gender_id: 0,
                school_id: Some(SCHOOL),
            },
        );
        assert_eq!(reg.character(id).expect("created").balance, DEV_BALANCE);
        assert_eq!(
            dev_top_up(&mut reg, 7),
            1,
            "the seeded default character only"
        );
        assert_eq!(reg.character(id).expect("created").balance, DEV_BALANCE);
        assert_eq!(dev_top_up(&mut reg, 8), 0, "once");
        let first = reg.config().characters[0].character_id;
        assert_eq!(reg.character(first).expect("seeded").balance, DEV_BALANCE);
        assert_eq!(journal_of(&reg, first).len(), 1, "the top-up is journalled");
    }

    #[test]
    fn a_normal_world_is_funded_as_before() {
        let mut reg = registry();
        let id = reg.create_owned_character(1001, "Invented Pilot".to_owned());
        assert_eq!(
            reg.character(id).expect("created").balance,
            STARTING_BALANCE
        );
    }

    #[test]
    fn sign_up_and_start_character_take_the_unchosen_race_s_default_school() {
        let mut reg = registry_with_unchosen();
        let id = reg.create_owned_character(1001, "Invented Pilot".to_owned());
        let c = reg.character(id).expect("created");
        let want = NewCharacter::unchosen();
        assert_eq!(
            (c.race_id, c.bloodline_id, c.gender_id),
            (want.race_id, want.bloodline_id, want.gender_id)
        );
        assert_eq!((c.school_id, c.station_id), (UNCHOSEN_SCHOOL, Some(SECOND)));
        assert_funded_once(&reg, id);
    }

    /// `provision_start_characters`: an absent one is created, placed and
    /// funded; an adopted (case 2) or found (case 1) one is neither moved
    /// nor credited.
    #[test]
    fn only_a_provisioned_character_that_is_created_is_placed_and_funded() {
        let mut reg = registry_from(
            table_with_unchosen(),
            crate::model::SandboxConfig {
                catch_all: None,
                start_characters: vec![
                    (1001, Character::default().name),
                    (1002, "Invented Pilot".to_owned()),
                ],
                ..crate::model::SandboxConfig::default()
            },
        );
        let adopted = Character::default().character_id;
        reg.provision_start_characters().expect("provisions");
        reg.provision_start_characters()
            .expect("found the second time");
        let c = reg.character(adopted).expect("the seed character");
        assert_eq!(c.owner_user_id, Some(1001));
        assert_eq!(c.balance, Character::default().balance, "not credited");
        assert_eq!(c.station_id, Character::default().station_id, "not moved");
        assert!(journal_of(&reg, adopted).is_empty());
        let created = reg
            .config()
            .characters
            .iter()
            .find(|c| c.name == "Invented Pilot")
            .map(|c| c.character_id)
            .expect("created");
        assert_eq!(
            reg.character(created).and_then(|c| c.station_id),
            Some(SECOND)
        );
        assert_funded_once(&reg, created);
    }

    /// A restart grants nothing twice: the world saved and applied to a
    /// fresh server keeps the one row and the balance, and `start_roster`
    /// skips a saved character.
    #[test]
    fn a_restart_does_not_grant_twice() {
        let mut reg = registry();
        let id = reg.create_character(
            1001,
            "Invented Pilot".to_owned(),
            NewCharacter {
                race_id: RACE,
                bloodline_id: 901,
                gender_id: 1,
                school_id: Some(SCHOOL),
            },
        );
        let snap = reg.to_snapshot();
        let mut again = registry();
        let saved = snap.characters.iter().map(|r| r.character_id).collect();
        assert!(again.start_roster(&saved).is_empty());
        again.apply_snapshot(snap);
        assert_funded_once(&again, id);
        assert_eq!(again.character(id).and_then(|c| c.station_id), Some(FIRST));
    }

    /// A created character is born now, not in 2003 (age-limited boosters),
    /// and a save and load keeps the date.
    #[test]
    fn a_created_character_is_born_now_and_stays_so_after_a_reload() {
        let mut reg = registry();
        let id = reg.create_character(
            1001,
            "Invented Pilot".to_owned(),
            NewCharacter {
                race_id: RACE,
                bloodline_id: 901,
                gender_id: 1,
                school_id: Some(SCHOOL),
            },
        );
        let born = reg.character(id).expect("created").create_date_time;
        let now = crate::pyutil::filetime_now();
        assert!(
            (now - born).abs() < 60 * 10_000_000,
            "born within a minute of now"
        );
        let snap = reg.to_snapshot();
        let mut again = registry();
        let saved = snap.characters.iter().map(|r| r.character_id).collect();
        again.start_roster(&saved);
        again.apply_snapshot(snap);
        assert_eq!(again.character(id).map(|c| c.create_date_time), Some(born));
    }

    /// Without a bundle a created character starts where
    /// `Character::default()` puts it, as before, and is still funded.
    #[test]
    fn without_a_bundle_a_created_character_starts_at_the_default_station() {
        let mut reg = crate::testkit::registry();
        let id = reg.create_owned_character(1001, "Invented Pilot".to_owned());
        let c = reg.character(id).expect("created");
        let d = Character::default();
        assert_eq!(
            (c.station_id, c.corporation_id, c.school_id),
            (d.station_id, d.corporation_id, d.school_id)
        );
        assert_funded_once(&reg, id);
    }

    /// The `--character` roster: a fresh one is placed at its `,school=N`
    /// (its capsule moved with it) and returned for funding; one the world
    /// file knows is left alone.
    #[test]
    fn start_roster_places_only_characters_the_world_does_not_know() {
        let alice = Character::default();
        let bob = Character {
            character_id: alice.character_id + 1,
            ship_id: alice.ship_id + 1,
            name: "Bob".to_owned(),
            ..Character::default()
        };
        let mut reg = registry_from(
            table(),
            crate::model::SandboxConfig {
                characters: vec![alice.clone(), bob.clone()],
                fresh_roster: vec![
                    (alice.character_id, Some(SCHOOL)),
                    (bob.character_id, Some(SCHOOL)),
                ],
                ..crate::model::SandboxConfig::default()
            },
        );
        let saved = std::iter::once(bob.character_id).collect();
        assert_eq!(reg.start_roster(&saved), vec![alice.character_id]);
        let a = reg.character(alice.character_id).expect("alice");
        assert_eq!((a.station_id, a.school_id), (Some(FIRST), SCHOOL));
        let pod = reg.store().get(alice.ship_id).expect("her capsule");
        assert_eq!(pod.location_id, FIRST, "the capsule moved with her");
        assert_eq!(
            reg.character(bob.character_id).and_then(|c| c.station_id),
            bob.station_id,
            "a saved character is not re-placed"
        );
    }

    /// `account.GetEntryTypes` lists the starting-funds type, or the client
    /// drops the row from the Journal tab.
    #[test]
    fn the_entry_types_list_the_starting_funds() {
        let mut reg = crate::testkit::registry();
        let got = reg
            .dispatch(&crate::service::Request {
                service: "account",
                method: "GetEntryTypes",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: None,
            })
            .expect("answers");
        let eve_marshal::PyValue::List(rows) = got else {
            panic!("a list: {got:?}");
        };
        let want = format!("Int({REF_NEW_CHARACTER_STARTING_FUNDS})");
        assert!(rows.iter().any(|r| format!("{r:?}").contains(&want)));
    }

    use crate::service::{Caller, Request, ServiceError};
    use eve_marshal::PyValue;

    const PLAYER: i64 = 1001;
    const OTHER: i64 = 1002;

    fn as_user(user_id: i64) -> Caller {
        Caller::Session { user_id, role: 0 }
    }

    fn unbound(
        reg: &mut Registry,
        user_id: i64,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch_as(
            as_user(user_id),
            &Request {
                service: "charUnboundMgr",
                method,
                args,
                kwargs: &[],
                bind_params: None,
                actor: None,
            },
        )
    }

    /// The client's ten arguments, in its order.
    fn doll(name: &str, race_id: i64, school_id: i64) -> Vec<PyValue> {
        vec![
            PyValue::Unicode(name.to_owned()),
            PyValue::Int(race_id),
            PyValue::Int(901),
            PyValue::Int(0),
            PyValue::Int(0),
            PyValue::None,
            PyValue::None,
            PyValue::Int(school_id),
            PyValue::None,
            PyValue::None,
        ]
    }

    /// Accounts mode: each session plays only its own characters.
    fn accounts_registry() -> Registry {
        registry_from(
            table(),
            crate::model::SandboxConfig {
                catch_all: None,
                ..crate::model::SandboxConfig::default()
            },
        )
    }

    /// The `UserError`'s message key.
    fn raised_key(got: Result<PyValue, ServiceError>) -> String {
        let Err(ServiceError::Raise(PyValue::ObjectEx { header, .. })) = &got else {
            panic!("expected a UserError, got {got:?}");
        };
        match &**header {
            PyValue::Tuple(h) => match h.get(1) {
                Some(PyValue::Tuple(args)) => match args.first() {
                    Some(PyValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
                    other => panic!("a key: {other:?}"),
                },
                other => panic!("arguments: {other:?}"),
            },
            other => panic!("a header: {other:?}"),
        }
    }

    #[test]
    fn create_character_with_doll_seats_funds_and_owns_the_new_character() {
        let mut reg = accounts_registry();
        let got = unbound(
            &mut reg,
            PLAYER,
            "CreateCharacterWithDoll",
            &doll("Invented Pilot", RACE, SCHOOL),
        );
        let Ok(PyValue::Int(id)) = got else {
            panic!("a charID: {got:?}");
        };
        let c = reg.character(id).expect("created");
        assert_eq!(c.owner_user_id, Some(PLAYER));
        assert_eq!(
            (c.race_id, c.bloodline_id, c.gender_id, c.school_id),
            (RACE, 901, 0, SCHOOL)
        );
        assert_eq!((c.station_id, c.corporation_id), (Some(FIRST), CORP));
        assert_funded_once(&reg, id);

        let select = |reg: &mut Registry, user| {
            let got = unbound(reg, user, "GetCharacterSelectionData", &[]).expect("answers");
            format!("{got:?}").contains("Invented Pilot")
        };
        assert!(select(&mut reg, PLAYER), "the creator sees it");
        assert!(!select(&mut reg, OTHER), "another account does not");
        let count = |reg: &mut Registry, user| unbound(reg, user, "GetNumCharacters", &[]);
        assert!(matches!(count(&mut reg, PLAYER), Ok(PyValue::Int(1))));
        assert!(matches!(count(&mut reg, OTHER), Ok(PyValue::Int(0))));
    }

    /// Another race's, a starter-space, an unknown and an unplaceable
    /// school are each refused with the school key, and nothing is made.
    #[test]
    fn create_character_with_doll_refuses_a_school_it_cannot_start_in() {
        let mut reg = accounts_registry();
        let before = reg.config().characters.len();
        for (race, school) in [(92, SCHOOL), (RACE, 98), (RACE, 1), (RACE, 95)] {
            let key = raised_key(unbound(
                &mut reg,
                PLAYER,
                "CreateCharacterWithDoll",
                &doll("Invented Pilot", race, school),
            ));
            assert_eq!(key, SCHOOL_ERROR_KEY, "race {race} school {school}");
        }
        assert_eq!(reg.config().characters.len(), before, "nothing created");
        assert!(reg.journal().entries().is_empty(), "nothing funded");
    }

    /// A taken or malformed name is refused with its `CharNameInvalid*` key.
    #[test]
    fn create_character_with_doll_refuses_a_taken_or_bad_name() {
        let mut reg = accounts_registry();
        let taken = Character::default().name.to_ascii_uppercase();
        for (name, want) in [
            (taken.as_str(), name_error_key(NAME_UNAVAILABLE)),
            ("ab", name_error_key(NAME_TOO_SHORT)),
            ("Bad*Name", name_error_key(NAME_ILLEGAL_CHARACTER)),
        ] {
            let key = raised_key(unbound(
                &mut reg,
                PLAYER,
                "CreateCharacterWithDoll",
                &doll(name, RACE, SCHOOL),
            ));
            assert_eq!(key, want, "{name}");
        }
        assert_eq!(reg.config().characters.len(), 1, "nothing created");
    }

    #[test]
    fn validate_name_ex_answers_the_client_s_codes() {
        let mut reg = accounts_registry();
        let long = "x".repeat(*crate::account_book::NAME_LEN.end() + 1);
        let short = "x".repeat(*crate::account_book::NAME_LEN.start() - 1);
        let taken = Character::default().name.to_ascii_lowercase();
        for (name, want) in [
            ("Invented Pilot", NAME_VALID),
            (short.as_str(), NAME_TOO_SHORT),
            (long.as_str(), NAME_TOO_LONG),
            ("Bad*Name", NAME_ILLEGAL_CHARACTER),
            (" Leading", NAME_ILLEGAL_CHARACTER),
            ("Two  Spaces", NAME_CONSECUTIVE_SPACES),
            (taken.as_str(), NAME_UNAVAILABLE),
        ] {
            let got = unbound(
                &mut reg,
                PLAYER,
                "ValidateNameEx",
                &[PyValue::Unicode(name.to_owned()), PyValue::Int(0)],
            );
            assert!(
                matches!(got, Ok(PyValue::Int(v)) if v == want),
                "{name:?}: {got:?}"
            );
        }
    }

    #[test]
    fn a_random_name_is_valid_and_free_for_each_race() {
        let reg = accounts_registry();
        for race in [1, 2, 4, 8, 0, 99] {
            for seed in [0u64, 1, 7, 12_345, u64::MAX] {
                let name = random_name(&reg, race, seed);
                assert_eq!(
                    validate_name(&reg, &name),
                    NAME_VALID,
                    "{race} {seed} {name:?}"
                );
            }
        }
        // Each race draws from its own flavour.
        assert!(crate::bot_names::pool_of("amarr").contains(&random_name(&reg, 4, 3)));
        assert!(crate::bot_names::pool_of("gallente").contains(&random_name(&reg, 8, 3)));
    }

    #[test]
    fn the_walk_passes_a_taken_name() {
        let mut reg = accounts_registry();
        let first = random_name(&reg, 1, 40);
        assert_eq!(random_name(&reg, 1, 40), first, "same seed, same name");
        let new = NewCharacter {
            race_id: RACE,
            bloodline_id: 901,
            gender_id: 0,
            school_id: Some(SCHOOL),
        };
        reg.create_character(1001, first.to_ascii_uppercase(), new);
        let next = random_name(&reg, 1, 40);
        assert_ne!(next.to_ascii_lowercase(), first.to_ascii_lowercase());
        assert_eq!(validate_name(&reg, &next), NAME_VALID);
    }

    #[test]
    fn the_start_is_the_first_listed_station_never_a_later_one() {
        let t = table();
        assert_eq!(t.start_station(SCHOOL), Some(FIRST));
        assert_eq!(t.start_station(98), Some(SECOND));
        assert_eq!(t.start_station(95), None, "no stations");
        assert_eq!(t.start_station(1), None, "unknown");
    }

    #[test]
    fn a_race_s_schools_are_sorted_and_never_starter_space_or_another_race() {
        assert_eq!(table().schools_of_race(RACE), vec![95, 97, SCHOOL]);
        assert_eq!(table().schools_of_race(92), vec![96]);
        assert!(table().schools_of_race(93).is_empty());
    }

    #[test]
    fn the_default_school_is_the_race_s_lowest() {
        let mut content = Content::fixture();
        content.insert("schools", table());
        assert_eq!(default_school_for_race(&content, RACE), Some(95));
        assert_eq!(default_school_for_race(&content, 93), None);
        assert_eq!(
            default_school_for_race(&Content::fixture(), RACE),
            None,
            "no bundle, no school"
        );
    }

    #[test]
    fn placing_seats_all_seven_fields_and_the_school_corp_and_nothing_else() {
        let reg = registry();
        let mut c = Character::default();
        place_new_character(&reg, &mut c, SCHOOL).expect("placed");
        let want = Character {
            station_id: Some(FIRST),
            station_owner_id: 1_999_101,
            station_type_id: 1529,
            station_operation_id: 99,
            solar_system_id: Some(SYSTEM),
            constellation_id: 29_000_001,
            region_id: 19_000_001,
            corporation_id: CORP,
            school_id: SCHOOL,
            ..Character::default()
        };
        assert_eq!(format!("{c:?}"), format!("{want:?}"));
        assert_eq!(
            c.balance,
            Character::default().balance,
            "the wallet is P-START-2's"
        );
    }

    #[test]
    fn a_refused_school_leaves_the_character_untouched() {
        let reg = registry();
        let untouched = format!("{:?}", Character::default());
        for (school_id, want) in [
            (1, StartError::UnknownSchool(1)),
            (98, StartError::StarterSpace(98)),
            (95, StartError::NoStation(95)),
        ] {
            let mut c = Character::default();
            assert_eq!(place_new_character(&reg, &mut c, school_id), Err(want));
            assert_eq!(format!("{c:?}"), untouched, "school {school_id}");
        }
    }

    #[test]
    fn a_station_nobody_knows_is_an_error_not_a_half_seat() {
        let mut t = table();
        t.insert(94, school(RACE, &[69_000_009], false));
        let mut content = Content::fixture();
        content.insert("schools", t);
        let reg = Registry::new(crate::model::SandboxConfig::default())
            .with_content(std::sync::Arc::new(content));
        let mut c = Character::default();
        assert_eq!(
            place_new_character(&reg, &mut c, 94),
            Err(StartError::UnknownStation {
                school_id: 94,
                station_id: 69_000_009
            })
        );
        assert_eq!(format!("{c:?}"), format!("{:?}", Character::default()));
    }

    #[test]
    fn without_a_bundle_every_school_is_unknown() {
        let reg = crate::testkit::registry();
        let mut c = Character::default();
        assert_eq!(
            place_new_character(&reg, &mut c, SCHOOL),
            Err(StartError::UnknownSchool(SCHOOL))
        );
    }

    #[test]
    fn the_schema_example_row_parses() {
        // `schema.md` `agents/schools.tsv`'s invented example row.
        let t = Table::parse(&["99\t1999001\t1\t\t39000001\t69000001\t3999001\t999001\t0"])
            .expect("parses");
        assert_eq!(
            t.get(99),
            Some(&School {
                corp_id: 1_999_001,
                race_id: 1,
                system_id: Some(39_000_001),
                starting_stations: vec![69_000_001],
                starter_space: false,
            })
        );
        let t = Table::parse(&["98\t1999002\t2\t\t\t\t\t999002\t1"]).expect("empty lists");
        let s = t.get(98).expect("row");
        assert!(s.starter_space && s.starting_stations.is_empty() && s.system_id.is_none());
    }

    #[test]
    fn a_malformed_line_is_an_error_naming_the_file_and_line() {
        for bad in [
            "99\t1999001",
            "99\t1999001\t1\t\t\t69000001\t\t999001\t2",
            "99\t1999001\t1\t\t\tx\t\t999001\t0",
        ] {
            let e = Table::parse(&[bad]).expect_err(bad);
            assert!(e.contains(FILE.0) && e.contains("data line 1"), "{e}");
        }
    }

    // --- lane start-skills ---------------------------------------------------

    /// Invented skills: two of the skill fixture's rows (`skill::FIXTURE`),
    /// and one id no skill table holds.
    const SKILL_ONE: i64 = 990_020_001;
    const SKILL_THREE: i64 = 990_020_003;
    const NO_SKILL: i64 = 990_029_999;
    /// Invented bloodlines: one of [`RACE`], one of race 92.
    const OUR_BLOODLINE: i64 = 901;
    const THEIR_BLOODLINE: i64 = 902;

    /// [`table`] with [`RACE`]'s starter skills and the two bloodlines.
    fn table_with_skills() -> Table {
        let mut t = table();
        t.parse_races(&[format!(
            "{RACE}\t\t{SKILL_THREE}=0;{NO_SKILL}=1;{SKILL_ONE}=2\t999001"
        )])
        .expect("races");
        t.parse_bloodlines(&[
            format!("{OUR_BLOODLINE}\t{RACE}\t{CORP}\t999011"),
            format!("{THEIR_BLOODLINE}\t92\t{CORP}\t999012"),
        ])
        .expect("bloodlines");
        t
    }

    fn create(reg: &mut Registry, race_id: i64, school_id: i64) -> i64 {
        reg.create_character(
            1001,
            "Invented Pilot".to_owned(),
            NewCharacter {
                race_id,
                bloodline_id: OUR_BLOODLINE,
                gender_id: 0,
                school_id: Some(school_id),
            },
        )
    }

    fn sheet(reg: &Registry, id: i64) -> Vec<(i64, i64, i64)> {
        reg.skills_for(Some(id))
            .expect("a skill sheet")
            .entries()
            .map(|e| (e.type_id, e.level, e.points))
            .collect()
    }

    /// The sheet is exactly the race's map, each skill at its level with the
    /// client's points for its rank; a level-0 skill is injected with none;
    /// a skill the table does not know is left out. It survives a restart.
    #[test]
    fn a_created_character_gets_exactly_its_race_s_starter_skills() {
        let mut reg = registry_from(table_with_skills(), crate::model::SandboxConfig::default());
        let id = create(&mut reg, RACE, SCHOOL);
        let rank = |t| {
            crate::skill::find_in(reg.content(), t)
                .expect("a fixture skill")
                .rank
        };
        let want = vec![
            (SKILL_ONE, 2, crate::skill::sp_for_level(rank(SKILL_ONE), 2)),
            (SKILL_THREE, 0, 0),
        ];
        assert_eq!(sheet(&reg, id), want);
        assert!(want[0].2 > 0, "level 2 has points");

        let snap = reg.to_snapshot();
        let mut again = registry_from(table_with_skills(), crate::model::SandboxConfig::default());
        again.apply_snapshot(snap);
        assert_eq!(sheet(&again, id), want, "the sheet persists with the world");
    }

    /// A race with no starter skills keeps the seeded sheet.
    #[test]
    fn a_race_without_starter_skills_keeps_the_seeded_sheet() {
        let mut reg = registry_from(table_with_skills(), crate::model::SandboxConfig::default());
        let id = create(&mut reg, 92, 96);
        let seeded: Vec<(i64, i64, i64)> = crate::skill::Skills::starter_in(reg.content())
            .entries()
            .map(|e| (e.type_id, e.level, e.points))
            .collect();
        assert_eq!(sheet(&reg, id), seeded);
    }

    /// A bloodline of another race, or one the bundle does not list, is
    /// refused with the bloodline key and nothing is made; the race's own
    /// bloodline is created.
    #[test]
    fn create_character_with_doll_refuses_a_bloodline_of_another_race() {
        let mut reg = registry_from(
            table_with_skills(),
            crate::model::SandboxConfig {
                catch_all: None,
                ..crate::model::SandboxConfig::default()
            },
        );
        let before = reg.config().characters.len();
        for bloodline in [THEIR_BLOODLINE, 903] {
            let mut args = doll("Invented Pilot", RACE, SCHOOL);
            args[2] = PyValue::Int(bloodline);
            let key = raised_key(unbound(&mut reg, PLAYER, "CreateCharacterWithDoll", &args));
            assert_eq!(key, BLOODLINE_ERROR_KEY, "bloodline {bloodline}");
        }
        assert_eq!(reg.config().characters.len(), before, "nothing created");
        assert!(reg.journal().entries().is_empty(), "nothing funded");

        let mut args = doll("Invented Pilot", RACE, SCHOOL);
        args[2] = PyValue::Int(OUR_BLOODLINE);
        let got = unbound(&mut reg, PLAYER, "CreateCharacterWithDoll", &args);
        let Ok(PyValue::Int(id)) = got else {
            panic!("a charID: {got:?}");
        };
        assert_eq!(sheet(&reg, id)[0].0, SKILL_ONE, "and the race's skills");
    }

    #[test]
    fn the_error_keys_map_one_to_one() {
        assert_eq!(
            CreateError::Bloodline {
                bloodline_id: THEIR_BLOODLINE,
                race_id: RACE
            }
            .error_key(),
            BLOODLINE_ERROR_KEY
        );
        assert_eq!(
            CreateError::School(StartError::UnknownSchool(SCHOOL)).error_key(),
            SCHOOL_ERROR_KEY
        );
        assert_eq!(
            CreateError::Name(NAME_TOO_SHORT).error_key(),
            name_error_key(NAME_TOO_SHORT)
        );
    }

    #[test]
    fn the_races_and_bloodlines_schema_example_rows_parse() {
        // `schema.md`'s invented example rows.
        let mut t = Table::default();
        t.parse_races(&["99\t990001\t990101=3;990102=1\t999001", "98\t\t\t999002"])
            .expect("races");
        assert_eq!(t.starter_skills(99), &[(990_101, 3), (990_102, 1)]);
        assert!(t.starter_skills(98).is_empty());
        assert!(!t.knows_bloodlines());
        t.parse_bloodlines(&["901\t99\t1999001\t999011"])
            .expect("bloodlines");
        assert_eq!(t.race_of_bloodline(901), Some(99));
        assert_eq!(t.race_of_bloodline(902), None);
        for bad in ["99\t\t990101\t999001", "99\t\t990101=x\t999001", "99\t\t"] {
            let e = t.parse_races(&[bad]).expect_err(bad);
            assert!(e.contains(crate::starter_ship::RACES.0), "{e}");
        }
        for bad in ["901\t99\t1999001", "901\tx\t1999001\t999011"] {
            let e = t.parse_bloodlines(&[bad]).expect_err(bad);
            assert!(e.contains(BLOODLINES.0) && e.contains("data line 1"), "{e}");
        }
    }

    /// Through `TABLE.load`: `races.tsv` and `bloodlines.tsv` load beside
    /// `schools.tsv`, and an absent file is empty.
    #[test]
    fn the_race_and_bloodline_files_load_through_their_headers() {
        let dir = crate::testkit::scratch_dir("start-skills");
        let b = crate::content::Bundle {
            dir: dir.path().to_path_buf(),
        };
        let authored = crate::authored::Dir::compiled_only();
        std::fs::create_dir_all(dir.path().join("agents")).expect("dir");
        let races = crate::starter_ship::RACES;
        std::fs::write(
            dir.path().join(races.0),
            format!("{}\n99\t990001\t990101=3\t999001\n", races.1),
        )
        .expect("write");
        let load = || (TABLE.load)(&Sources::new(Some(&b), &authored));
        let t = load().expect("loads without bloodlines");
        let t = t.downcast_ref::<Table>().expect("a schools table");
        assert_eq!(t.starter_skills(99), &[(990_101, 3)]);
        assert!(!t.knows_bloodlines());

        std::fs::write(
            dir.path().join(BLOODLINES.0),
            format!("{}\n901\t99\t1999001\t999011\n", BLOODLINES.1),
        )
        .expect("write");
        let t = load().expect("loads");
        let t = t.downcast_ref::<Table>().expect("a schools table");
        assert_eq!(t.race_of_bloodline(901), Some(99));
    }

    /// Through `TABLE.load` on a bundle directory: the rows load under the
    /// frozen header, an absent file is an empty table, and a wrong header
    /// is an error naming the file.
    #[test]
    fn the_bundle_file_loads_through_its_header() {
        let dir = crate::testkit::scratch_dir("pstart-schools");
        let b = crate::content::Bundle {
            dir: dir.path().to_path_buf(),
        };
        let authored = crate::authored::Dir::compiled_only();
        let load = || (TABLE.load)(&Sources::new(Some(&b), &authored));
        let empty = load().expect("an absent file loads");
        assert_eq!(empty.downcast_ref::<Table>(), Some(&Table::default()));

        std::fs::create_dir_all(dir.path().join("agents")).expect("dir");
        let row = "99\t1999001\t1\t\t39000001\t69000001\t3999001\t999001\t0";
        std::fs::write(dir.path().join(FILE.0), format!("{}\n{row}\n", FILE.1)).expect("write");
        let t = load().expect("loads");
        let t = t.downcast_ref::<Table>().expect("a schools table");
        assert_eq!(t.start_station(99), Some(69_000_001));

        std::fs::write(
            dir.path().join(FILE.0),
            format!("#school_id\tcorp_id\n{row}\n"),
        )
        .expect("write");
        let Err(e) = load() else {
            panic!("a wrong header refuses");
        };
        assert!(e.contains(FILE.0) && e.contains("header"), "{e}");
    }
}
