//! The world model: the things the server is about, as data.
//!
//! **Why this module exists.** These three types were declared in
//! `service.rs`, beside the dispatcher, and fifteen modules reached *up* into
//! the service layer to get at them -- `destiny.rs` 21 times, `service/space.rs`
//! 24, and 78 references in all. That is an inverted dependency, and it was
//! not caused by `service.rs` being large: it was caused by the world model
//! living in the dispatcher's file. `docs/archive/plan-subsystems.md` P0.5 has the
//! measurement.
//!
//! The rule that keeps it fixed: **nothing in here knows what a `PyValue`
//! is.** A `Character` is who somebody is; turning one into the row the
//! select screen reads is presentation, and `impl Character::to_row` stays in
//! `service.rs` where the wire format lives. Rust allows an inherent impl in
//! any module of the defining crate, so the split costs nothing at the call
//! site and buys the direction of the arrow.

/// The corporation every character starts in.
///
/// **An NPC corp, and that is load-bearing rather than flavour.** Several
/// client paths branch on `idCheckers.IsPlayerCorporation(corpID)`, and an
/// NPC corp takes the cheap side of every one of them -- most usefully the
/// session-change recalculation, where a falsy `corprole` is what keeps
/// `officeManager.GetCorpOfficeAtLocation` out of the blocking wave
/// (`docs/archive/entering-the-game.md` §1). It is also the answer
/// `charMgr.GetPublicInfo` gives for a character it has never heard of, for
/// the same reason: nothing downstream should take a player-corp branch
/// about somebody who does not exist.
pub const NPC_CORPORATION_ID: i64 = 1_000_001;
/// Settings the stub services answer from.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// The world rules a module may change; `Default` is today's behaviour.
    pub rules: crate::rules::Rules,
    pub user_id: i64,
    /// The node we answer as. Must match the handshake's `proxy_nodeid`:
    /// `MachoResolveObject` hands this to the client as the owner of a bound
    /// object, and the client then addresses calls on that object to it.
    pub node_id: i64,
    pub user_name: String,
    /// How many character slots the select screen draws.
    pub character_slots: i64,
    /// The characters to offer. Empty is legal and gives an empty screen,
    /// which is still a successful login.
    pub characters: Vec<Character>,
    /// Name for the corporation every character belongs to. The select
    /// screen reads it from `cfg.eveowners`, not from the character row.
    pub corporation_name: String,
    /// Names for the station and system a character is sitting in, read from
    /// `cfg.evelocations`.
    pub station_name: String,
    pub solar_system_name: String,
    /// The system's star, planets, moons, belts, stations and gates.
    ///
    /// Empty by default and populated from `--celestials FILE`, which
    /// `tools/celestials.py` generates out of the player's own client
    /// install. `crate::celestial`'s header has why it is a file rather than
    /// a table in the source.
    pub celestials: crate::celestial::Catalogue,
    /// The rocks in the system's asteroid belts.
    ///
    /// Empty by default. From `--belt-rocks FILE` it is a lazy field: the
    /// offset index over the file (`asteroid::index`), laid out per system
    /// against `celestials` while the system is warm or hot (CP14').
    /// `crate::asteroid` has why the counts are the client's and the
    /// positions are ours.
    pub rocks: crate::asteroid::Field,
    /// The client-CI scenario to run, if any.
    ///
    /// **`None` is the only normal value**, and that is the whole safety
    /// argument: with no scenario selected the `GetSettingsInfo` blob is the
    /// constant it has always been, so a player's session cannot be affected
    /// by a mechanism nobody asked for. Set from `--scenario NAME`, validated
    /// **at startup** rather than here -- a mistyped name that quietly ran a
    /// normal session would be a test that passes by doing nothing, so
    /// `cli.rs` refuses to start instead.
    ///
    /// What it does: the blob gains an `execfile` of
    /// `c:\eve-logs\client-ci\<name>.py`, which the client runs after
    /// character selection, when `sm`, `session` and the service layer are
    /// up. `crate::pycode::settings_info_scenario_func` has the evidence and
    /// `docs/lanes/ci-trigger.md` the reasoning, including why the scenario
    /// runs inline rather than in its own tasklet.
    pub scenario: Option<String>,
    /// The catch-all account's `user_id` without `--accounts`, `None` with
    /// it (`docs/lanes/account-spec.md` §4.1, §4.6; packet IP27-b).
    ///
    /// **`Some` is today's server**: one account, every session is it, and
    /// it may play every character, owned or not. `None` is accounts mode:
    /// a session plays only the characters whose
    /// [`Character::owner_user_id`] is its own. Set by `Server::bind` from
    /// the handshake's accounts, so it cannot disagree with them.
    pub catch_all: Option<i64>,
    /// `(user_id, start_character)` for each account that names one, in
    /// file order (§3.4). Empty without `--accounts`. Provisioned once at
    /// startup by `Registry::provision_start_characters`.
    pub start_characters: Vec<(i64, String)>,
    /// The `--character` roster as `(character_id, ,school=N)` (P-START-2,
    /// R-START §5.2). Each one that the world file does not know yet is
    /// placed at its school's first station and given
    /// `crate::start::STARTING_BALANCE` once, by `Server::bind`
    /// (`Registry::start_roster`); one the world knows keeps what it saved.
    /// Empty without `--character`: the one default character stays where
    /// [`Character::default`] puts it.
    pub fresh_roster: Vec<(i64, Option<i64>)>,
    /// `eve-server --dev` (DEV-MODE): a debugging world where the market
    /// has a dev seller, new characters start rich and the bots may be
    /// granted skills. `false` in every ordinary run.
    pub dev: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        SandboxConfig {
            rules: crate::rules::Rules::default(),
            user_id: 1,
            node_id: 1,
            user_name: "eve-offline".to_owned(),
            character_slots: 3,
            characters: vec![Character::default()],
            corporation_name: "Capsuleer Collective".to_owned(),
            station_name: "Home Station".to_owned(),
            solar_system_name: "Home".to_owned(),
            // **Empty is the ordinary case.** A server started without
            // `--celestials` behaves exactly as it did before the catalogue
            // existed: every lookup misses and the park holds the ships and
            // the station.
            celestials: crate::celestial::Catalogue::default(),
            rocks: crate::asteroid::Field::default(),
            scenario: None,
            catch_all: Some(1),
            start_characters: Vec::new(),
            fresh_roster: Vec::new(),
            dev: false,
        }
    }
}

/// One character on the select screen.
///
/// The fields are exactly the attributes `charselData.py`,
/// `characterSlots.py` and `characterSelection.py` read off a row
/// (`docs/reference/login-path.md` §8), enumerated by decompiling those modules and
/// enforced by the tests on `ROW_ATTRIBUTES`. **Being present matters as much
/// as being right**: a missing attribute is an `AttributeError` inside the
/// client with nothing to show for it on our side, and that is what froze the
/// first select screen we reached. Three are load-bearing beyond presence:
///
/// - `logoff_date` is the **sort key** for `GetChars()`, so it must be
///   comparable across every character.
/// - `paperdoll_state` is compared against
///   `paperdoll.State.force_recustomize` and `no_existing_customization`;
///   either value diverts the client into **character creation** instead of
///   entering the game. `0` is the quiet value.
/// - `skillTypeID` (emitted, not a field here) is the branch point in
///   `GetSkillInTrainingInfo`: `None` means nothing is training and the six
///   training fields behind it are never read.
#[derive(Debug, Clone)]
pub struct Character {
    pub character_id: i64,
    pub name: String,
    pub logoff_date: i64,
    pub corporation_id: i64,
    pub alliance_id: Option<i64>,
    pub station_id: Option<i64>,
    /// "My station's system" -- **not** where the
    /// pilot is once undocked: read [`crate::service::Registry::session_system`] for that.
    pub solar_system_id: Option<i64>,
    pub faction_id: Option<i64>,
    pub balance: f64,
    pub skill_points: i64,
    pub ship_type_id: i64,
    pub paperdoll_state: i64,
    /// The station's own identity, as the client's static data has it.
    ///
    /// These four are not free choices: the client ships TQ's `npcStations`
    /// table (`bin64/staticdata/mapObjects.db`) and resolves station types
    /// and operations through FSD, so an invented value raises rather than
    /// rendering oddly. `stationSvc.GetStationItemBits` hands exactly these
    /// to the client. Taken verbatim from that table:
    ///
    /// ```text
    /// stationID 60012526 | ownerID 1000126 | typeID 2502 | operationID 33
    /// ```
    ///
    /// and that station really is in solar system 30000001, which is the one
    /// the session claims. Our first pick was station 60000004 in system
    /// 30000001 -- a combination that does not exist, because 60000004 lives
    /// in 30002780. Nothing had complained yet, but the client holds its own
    /// copy of this table and would eventually disagree with us about where
    /// we are.
    pub station_owner_id: i64,
    pub station_type_id: i64,
    pub station_operation_id: i64,
    /// Where the station sits. The session carries all three, and the client
    /// derives nothing here.
    ///
    /// **[V]** as of 2026-10-21, and the file we could not find is
    /// `res:/staticdata/starmapcache.pickle`. It is a plain Python 2 pickle,
    /// not cFSD, which is why nothing that reads `mapObjects.db` found the
    /// chain. `solarSystems[30000001]` reads
    ///
    /// ```text
    /// constellationID 20000001 | regionID 10000001 | factionID 500007
    /// ```
    ///
    /// matching both values below exactly. The same row also carries
    /// `planetCountByType {2016: 1, 11: 2, 13: 1, 2014: 2}`, whose keys are
    /// planet type ids in `globaltypes` -- so the home system's PI resources
    /// are fully specifiable from the client too.
    pub constellation_id: i64,
    pub region_id: i64,
    pub gender_id: i64,
    pub bloodline_id: i64,
    pub race_id: i64,
    /// When the character was created, as a **blue FILETIME** -- 100ns ticks
    /// since 1601, not a Unix time and not a small integer.
    ///
    /// The type is load-bearing in a way that fails quietly.
    /// `localization.formatters.FormatDateTime` begins
    ///
    /// ```python
    /// if isinstance(value, long):
    ///     ...
    ///     blue.os.GetTimeParts(value)
    /// else:
    ///     logger.LogTraceback('datetime only accepts blue time ...')
    ///     return
    /// ```
    ///
    /// so anything that unmarshals as a Python 2 `int` rather than a `long`
    /// takes the `else` and returns `None` -- a `STACKTRACE` record and a
    /// blank birth date, with no exception. Our encoder picks the integer tag
    /// by magnitude, so a real FILETIME (~1.3e17) is emitted as INT64 and
    /// arrives as a `long` by construction; `0`, `1` or any value below 2^31
    /// would not.
    ///
    /// The default is 2003-05-06, the day EVE launched. It is a fixed date
    /// rather than `filetime_now()` on purpose: a birth date that changes
    /// every time the server restarts is a thing you would eventually chase.
    pub create_date_time: i64,
    /// The capsule the character is sitting in.
    ///
    /// **In EVE you are always in a ship or a capsule**, and the client knows
    /// it: the fitting window refuses to open with
    /// `CannotPerformActionWithoutShip`, which is reached purely from
    /// `session.shipid` being `None`.
    ///
    /// The id must be **>= `minPlayerItem` (100000000)**, and that is not
    /// cosmetic. `idCheckers.IsSystemOrNPC(id)` is `id < 90000000`, and
    /// `cfg.Recordset._Prime` routes such keys through the client's *local*
    /// sqlite fetcher before it ever asks us; `IsPlayerItem` is
    /// `100000000 <= id < minFakeItem` and gates the SKIN lookup. Do not
    /// reason by analogy from our `character_id` of 90000001 -- that is a
    /// valid *character* id and specifically not a player item.
    ///
    /// The pod lives in the station hangar like any other ship. TQ's own
    /// session builder says so, walking *up* the location chain:
    ///
    /// ```python
    /// if item.categoryID == const.categoryShip:
    ///     updateDict['shipid'] = itemID
    /// elif item.groupID == const.groupStation:
    ///     updateDict['stationid'] = itemID
    /// ...
    /// itemID = item.locationID
    /// ```
    ///
    /// so "the character is in a pod, the pod is in the hangar" is literally
    /// how `shipid` and `stationid` are derived. There is no separate
    /// "active ship" list anywhere in the client -- every consumer reads the
    /// ordinary `flagHangar` listing and picks the row whose `itemID`
    /// matches `session.shipid`.
    pub ship_id: i64,
    /// The account that plays this character (ruling 8; account-spec §3.3).
    ///
    /// `None` is **unowned**: without `--accounts` the catch-all plays it,
    /// as every character was played before accounts; with `--accounts`
    /// nobody does until a `start_character` adopts it (§3.4) [A]. World
    /// state, written in the world file's `character` record, so a restart
    /// keeps each character on its account.
    pub owner_user_id: Option<i64>,
    /// **The bot marker** (W14 CP40, C7 §5.3): set by the `--character
    /// NAME,bot` spec, never derived from a name (ruling 2). Server-side
    /// only: no row, slim item or overview carries it (ruling 5). Read by
    /// `crate::liveness`, which ticks a system with no human less often
    /// under load.
    pub is_bot: bool,
    /// The school the character was made in (`agents/schools.tsv`, P-START-2):
    /// `GetPublicInfo.schoolID`, which the client's career agents and "set
    /// home station" read. Set by `crate::start::place_new_character`;
    /// [`DEFAULT_SCHOOL_ID`] for a character no school placed.
    pub school_id: i64,
}

/// **[A]** The school of a character no school placed: [`Character::default`]'s,
/// a test's, or one whose school the bundle does not know. A key of build
/// 3619879's `schools.fsdbinary` **[V]** (so `schools.get_school` cannot
/// `KeyError`); that it is the character's school is the assumption. Was
/// `service/character.rs`'s `SCHOOL_ID`. Falsified by nothing a player sees
/// but the "school HQ" marker; a placed character carries its real school.
pub const DEFAULT_SCHOOL_ID: i64 = 11;

impl Default for Character {
    fn default() -> Self {
        Character {
            character_id: 90_000_001,
            name: "Capsuleer".to_owned(),
            logoff_date: 0,
            corporation_id: NPC_CORPORATION_ID,
            alliance_id: None,
            station_id: Some(60_012_526),
            station_owner_id: 1_000_126,
            station_type_id: 2_502,
            station_operation_id: 33,
            solar_system_id: Some(30_000_001),
            ship_id: 100_000_001,
            faction_id: None,
            balance: 0.0,
            skill_points: 0,
            ship_type_id: 670,
            paperdoll_state: 0,
            constellation_id: 20_000_001,
            region_id: 10_000_001,
            gender_id: 1,
            bloodline_id: 1,
            race_id: 1,
            // 2003-05-06 00:00:00 UTC in blue FILETIME ticks.
            create_date_time: 126_966_528_000_000_000,
            owner_user_id: None,
            is_bot: false,
            school_id: DEFAULT_SCHOOL_ID,
        }
    }
}

/// Where something is and how big it is.
///
/// A position alone is not enough to aim at an object: see
/// [`Registry::ball_target`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    pub position: [f64; 3],
    pub radius: f64,
    /// The client's own **warp-in point** for this target, when it has one.
    ///
    /// `None` is the ordinary case and not a gap: `HasWarpInPoint` is true
    /// only for a celestial of radius 90 km or more, so ships, stations,
    /// belts and stargates all have none and a warp to them aims at the
    /// position itself. See [`crate::warp_in`] for the rule and why it
    /// matters: a warp-in point is **one shared point per body**, so every
    /// pilot lands on the same spot, and `minRange` is measured from there.
    pub warp_in: Option<[f64; 3]>,
}

pub const FLAG_HANGAR: i64 = 4;

pub const CATEGORY_SHIP: i64 = 6;

/// The corporation that owns our station and employs the agent above.
///
/// **The Ammatar Consulate**, and that is read rather than assumed: the
/// client's own `npccorporations.fsdbinary` row for 1000126 has ticker `ATC`,
/// `nameID` resolving to "Ammatar Consulate", `solarSystemID` 30000001 and
/// `stationID` 60012526 -- the same system and station our character sits in.
pub const STATION_AGENT_CORP: i64 = 1_000_126;
