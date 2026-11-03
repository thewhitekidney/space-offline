//! Service dispatch: turning a `CallReq` into a `CallRsp`.
//!
//! The client calls a named service and method
//! (`ServiceCallGPCS.RemoteServiceCallWithoutTheStars`), the payload being
//! `(1, method, args, kwargs)` with the service name in the packet's
//! destination address. We answer with a `CallRsp` whose payload is the
//! return value. `docs/reference/login-path.md` §8 lists the calls the client makes, in
//! order, and what shape it consumes from each — this module implements that
//! table and nothing more.
//!
//! **The design point is that almost every answer is allowed to be empty.**
//! `GetServiceInfo` may return `{}`; the blacklist and the price map may be
//! empty containers; `Ping`'s return is ignored. CCP's client is tolerant here
//! because the real cluster is heterogeneous. So a server that answers a
//! dozen calls with near-nothing gets a long way, and the calls where the
//! shape genuinely matters are few and are marked as such below.

use crate::model::{Character, SandboxConfig, Target};
use crate::pyutil::{bstr, instance, keyval, opt_id, pydict};
use eve_marshal::{Column, DbRow, DbRowDescriptor, DbType, DbValue, PyValue};
use eve_rowset::cfg::{self, Location, Owner};
use std::time::{Duration, Instant};

/// `inventorycommon/const.pyc`: `groupCharacter`. The **only** reason a
/// character's item row survives `UpdateItem`'s category filter.
const GROUP_CHARACTER: i64 = 1;
/// `categoryOwner`. Not one of the eleven categories the filter accepts,
/// which is why the group check above matters.
const CATEGORY_OWNER: i64 = 1;
/// The character type. Same value `multi_owners` already reports for an
/// owner's `typeID`; it must be a type FSD knows, because a missing dogma
/// attribute falls back through `evetypes.GetCategoryID(typeID)`. **[I]**
const CHARACTER_TYPE_ID: i64 = 1373;

/// Character attribute ids, from `dogma/const.pyc`. `godma.Prime` scatters
/// these five by **name** (`memory`, `perception`, `willpower`, `charisma`,
/// `intelligence`).
pub(crate) const ATTR_CHARISMA: i64 = 164;
pub(crate) const ATTR_INTELLIGENCE: i64 = 165;
pub(crate) const ATTR_MEMORY: i64 = 166;
pub(crate) const ATTR_PERCEPTION: i64 = 167;
pub(crate) const ATTR_WILLPOWER: i64 = 168;

/// Which container a bound inventory object is, from its bind params.
///
/// An inventory is minted with the arguments of the call that produced it --
/// `GetInventory(10004, None)` or `GetInventoryFromId(<itemID>, 0)` -- so the
/// first element is the container key either way.
fn container_of(bind_params: Option<&PyValue>) -> Option<i64> {
    match bind_params? {
        PyValue::Tuple(parts) | PyValue::List(parts) => match parts.first()? {
            PyValue::Int(id) => Some(*id),
            _ => None,
        },
        PyValue::Int(id) => Some(*id),
        _ => None,
    }
}

/// The capsule, verified from `inventorycommon/const.pyc`: `typeCapsule`,
/// `groupCapsule`, `categoryShip`, `flagHangar`.
///
/// A capsule is the cheapest honest ship, and it is what the client itself
/// falls back to on `ShipNotInHangar`.
pub(crate) const TYPE_CAPSULE: i64 = 670;
/// PLEX. It gets its own history call because the ticker's default list
/// contains it and `PopulatePriceHistoryCacheForManyTypeIDs` splits it out of
/// the set before asking.
const TYPE_PLEX: i64 = 44_992;
/// 2100-01-01 as a win64 FILETIME: "far enough in the future to mean never".
///
/// **The magnitude is the contract, not the date.** Anything the client
/// formats as a time has to arrive as a Python `long`, and our encoder picks
/// the integer tag by magnitude -- so a value under 2^31 arrives as an `int`
/// and hits one of two walls, neither of which is loud.
/// `dateTimeFormatters.FormatDateTime` opens with `isinstance(value, long)`
/// and its else branch logs a traceback and returns `None`: a **blank field,
/// no exception, nothing in our log**.
/// `timeIntervalFormatters._FormatTimeIntervalGetParts` is blunter and raises
/// `ValueError('TimeInterval should be a blue time (long), recieved int.')`.
///
/// It is also deliberately *not* the i64 sentinel used for
/// `GetCharacterSelectionData`'s `subscriptionEndTime`, which is a year in
/// the millions. `FormatDateTime` hands the value to native
/// `blue.os.GetTimeParts`, and a real date keeps that in a range it has some
/// reason to tolerate.
const FILETIME_2100: i64 = 157_469_184_000_000_000;
/// `appConst.refGMCashTransfer` -- the journal entry type a GM ISK grant uses.
const REF_GM_CASH_TRANSFER: i64 = 3;
/// `appConst.refPlayerDonation`.
const REF_PLAYER_DONATION: i64 = 10;
/// `containerHangar`: one of nine magic container ids that make
/// `invCacheContainer` ask the server what it is, rather than taking its key
/// as the item id.
const CONTAINER_HANGAR: i64 = 10_004;

/// What we tell the client a station hangar holds, in cubic metres.
///
/// **[I] policy, not data.** EVE station hangars are unlimited and the
/// client has no sentinel for that, so this is the least-wrong finite
/// stand-in: large enough that `capacity - used` never goes negative in
/// practice, small enough to stay exact in an `f64`.
const STATION_HANGAR_CAPACITY: f64 = 1_000_000_000.0;
pub(crate) const GROUP_CAPSULE: i64 = 29;

/// `inventorycommon/const.pyc`: `groupStation`. The same 15 the client itself
/// puts in its `invbroker` bind params, `(stationID, groupStation)`.
const GROUP_STATION: i64 = 15;
/// `categoryStation`.
const CATEGORY_STATION: i64 = 3;

/// `CLONE_STATE_OMEGA` from `clonegrade/const.py`: a full subscription.
/// `0` is `CLONE_STATE_ALPHA`.
const CLONE_STATE_OMEGA: i64 = 1;

/// One call off the wire.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    pub service: &'a str,
    pub method: &'a str,
    pub args: &'a [PyValue],
    pub kwargs: &'a [(PyValue, PyValue)],
    /// For a call on a **bound object**, the parameters it was minted with.
    ///
    /// Most arms do not need it: we are one node with one object per
    /// service, so the service name is usually identity enough. Inventories
    /// are the exception -- the client binds several, and which one you are
    /// holding is the whole question. `GetSelfInvItem` has to answer "the
    /// station" for the hangar container and "the capsule" for the ship's,
    /// and only this tells them apart.
    pub bind_params: Option<&'a PyValue>,
    /// The acting character, or `None` before one was selected.
    ///
    /// **`None` is not a cue to pick one.** An arm with no actor answers its
    /// no-character branch -- the one it already has for an empty roster.
    /// This field replaced 29 reads of `config.characters.first()`, every one
    /// of which meant "the acting character" and answered "whoever is listed
    /// first" instead.
    ///
    /// `Option<i64>` rather than `i64` is the whole design. A sentinel -- `0`,
    /// or "the first character" -- is a value every arm can accidentally use;
    /// `None` is a value no arm can read a character out of without saying
    /// what it does instead. EvEJS reached the same conclusion the hard way
    /// and could only write it down as a convention
    /// (`services/_shared/sessionIdentity.js:5-18`): its ad-hoc
    /// `session.characterID || session.charid || session.userid || 140000001`
    /// chains "silently fall back to a hardcoded character, so a session with
    /// no identity acts as a specific real player". 29 files import their
    /// resolver; 277 of those chains remain. A rule you have to remember
    /// fixed a tenth of the problem, so ours is a type.
    pub actor: Option<i64>,
}

/// Why a call could not be answered -- or, for `Raise`, how it answers.
///
/// Not `Eq`: `Raise` carries a `PyValue`, which can hold an `f64`.
#[derive(Debug, Clone, PartialEq)]
pub enum ServiceError {
    /// No service of that name is registered.
    NoSuchService(String),
    /// The service exists but does not implement that method.
    NoSuchMethod { service: String, method: String },
    /// The arguments were not what the method needs.
    BadArguments {
        service: String,
        method: String,
        why: String,
    },
    /// **Not a failure: the method is specified to raise.**
    ///
    /// A handful of calls are defined by what they throw rather than what
    /// they return, and for those the project's "always answer with a value"
    /// default is actively wrong. `slash.SlashCmd("/")` is the one that
    /// forced this: the client reads the server's command list out of the
    /// *exception*, and binds the local only inside the `except` block, so
    /// returning normally is an `UnboundLocalError` that breaks every slash
    /// command. See `eve_proto::user_error`.
    ///
    /// The payload is the exception as a `PyValue`; `dispatch` sends it as a
    /// `MACHONETERR_WRAPPEDEXCEPTION` `ErrorResponse` rather than a `CallRsp`.
    Raise(PyValue),
}

impl core::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ServiceError::Raise(_) => write!(f, "raises a UserError by design"),
            ServiceError::NoSuchService(s) => write!(f, "no such service {s}"),
            ServiceError::NoSuchMethod { service, method } => {
                write!(f, "{service} has no method {method}")
            }
            ServiceError::BadArguments {
                service,
                method,
                why,
            } => write!(f, "{service}.{method}: {why}"),
        }
    }
}

impl std::error::Error for ServiceError {}

impl Character {
    /// The row the select screen reads, as a `KeyVal`. `delete_at` is the
    /// FILETIME the character may be deleted from, when it is waiting to be
    /// (`start::prepare_delete`).
    #[cfg(test)]
    fn to_row(&self) -> PyValue {
        self.to_row_deleting(None)
    }

    fn to_row_deleting(&self, delete_at: Option<i64>) -> PyValue {
        let opt = |v: Option<i64>| v.map(PyValue::Int).unwrap_or(PyValue::None);
        keyval(vec![
            ("characterID", PyValue::Int(self.character_id)),
            ("characterName", PyValue::Unicode(self.name.clone())),
            ("logoffDate", PyValue::Int(self.logoff_date)),
            ("corporationID", PyValue::Int(self.corporation_id)),
            ("allianceID", opt(self.alliance_id)),
            ("stationID", opt(self.station_id)),
            ("solarSystemID", opt(self.solar_system_id)),
            ("factionID", opt(self.faction_id)),
            ("balance", PyValue::Real(self.balance)),
            ("balanceChange", PyValue::Real(0.0)),
            ("skillPoints", PyValue::Int(self.skill_points)),
            ("unreadMailCount", PyValue::Int(0)),
            ("shipTypeID", PyValue::Int(self.ship_type_id)),
            ("paperdollState", PyValue::Int(self.paperdoll_state)),
            ("finishedSkills", PyValue::List(vec![])),
            // Not in training. `GetSkillInTrainingInfo` branches on exactly
            // this and returns an all-None dict when it is None, so the six
            // fields below are never reached -- the slot code then checks
            // `currentSkill is not None` and draws no training bar.
            //
            // They are carried anyway because the names are confirmed (they
            // are read in the other branch) and a row that names its whole
            // column set is a truer stand-in for the real server's. Filling
            // them in is how a future sandbox puts a skill in training.
            ("skillTypeID", PyValue::None),
            ("toLevel", PyValue::None),
            ("trainingStartTime", PyValue::None),
            ("trainingEndTime", PyValue::None),
            ("queueEndTime", PyValue::None),
            ("finishSP", PyValue::None),
            ("trainedSP", PyValue::None),
            // Read as `bool(...)` by `IsPreparingForDeletion`, and returned
            // raw by `GetDeletePrepareTime` -- which the **mouse-over**
            // handler calls, so a missing one throws on every hover.
            ("deletePrepareDateTime", opt(delete_at)),
            // `bool(...)` again, via `IsUnavailable`: a locked character
            // (transferring, on sale) cannot be selected.
            ("lockTypeID", PyValue::None),
        ])
    }
}

/// Balls in a system from which `advance_system` builds its movers in parallel
/// (~0.8 us a mover; PA-8b). [A] untuned: tune on `brawl_bench`.
const MOVERS_PAR_MIN: usize = 512;

/// Every service call the client makes before the character-select screen.
mod agents;
mod alliance;
mod base;
mod bounty;
mod character;
pub(crate) mod corp;
mod ess;
// CORP-W: `pub(crate)` for the payout hooks (`npc::bounty`, `mission_rewards`).
pub(crate) mod corp_office;
pub(crate) mod corp_wallet;
mod facwar;
// W16 CP53a: `facWarMgr` and enlistment (`fwCharacterEnlistmentMgr`, out of
// `facwar`); `pub(crate)` for its `world_ext::SLOTS` row.
pub(crate) mod facwar_mgr;
mod fleet;
mod gaps2;
pub(crate) mod gm;
pub(crate) mod gm_econ;
pub(crate) mod gm_live;
mod hacking;
mod industry;
// L2 SKINS: `shipCosmeticsMgr`, out of `inventory`.
mod cosmetics;
// S1 (#158): `corpRecProxy`, the recruitment adverts, empty.
mod corp_rec;
// W16 CP44: ship insurance, out of `inventory`; `pub(crate)` for its
// `kill_hooks::PLAYER` row.
pub(crate) mod insurance;
mod inventory;
// SOC-C6: the kill report wire (`warStatisticMgr`, the combat log).
mod keeper;
pub(crate) mod killmail;
mod live;
mod live_misc;
mod mail;
mod market;
mod mining_ui;
// NOTIF-1: `notificationMgr` and the corp-side notification producers.
pub(crate) mod crate_svc;
pub(crate) mod notifications;
pub(crate) mod planet;
pub(crate) mod relocate;
pub(crate) mod repair;
mod reprocess;
mod share_links;
mod sites;
mod social;
// U2-UPWELL: `structureDirectory`, `structureProfiles`, `structureSettings`,
// `structureGuests` and `/upwell`.
pub(crate) mod structures;
// SOC-D1: `sovMgr`, `devIndexManager` and the `/sov` command.
pub(crate) mod sov;
// SOC-D2: `ship.Drop`, the fuel-access arms, `/sovfuel` and `/boostsov`.
mod owner_groups;
pub(crate) mod sov_hub;
pub(crate) mod space;
mod stance;
mod store;
mod super_weapon;
mod tales;
// GAP-CALENDAR: `calendarProxy` and `calendarMgr`.
mod calendar;
// SOC-MEDALS: the `corporationSvc` medal calls.
mod corp_medals;
// S7: corp shares and dividends, the `corpRegistry` share calls.
mod corp_shares;
// GAP-TRADE: `trademgr`, the direct trade window.
mod trade;
mod wallet;
// C8: player wars over the wire.
pub(crate) mod war;
mod world;

// W10.5 seams S8 and S12: the items `crate::warp_subject` and
// `crate::movement_guard` name from inside this module's private children.
#[cfg(test)]
pub(crate) use space::warp_guarded;
pub(crate) use wallet::break_package;
pub(crate) use world::{WARP_SUBJECT_BOOKMARK, bookmark_target};

/// For `crate::slash`'s `/setstanding` (seam S1).
pub(crate) use crate::standings::set_standing_slash;

pub use crate::clones::Clones;
/// For `combat::kill`: a podded pilot wakes up at home (`docs/lanes/clones.md` §3).
pub(crate) use crate::clones::revive_in_clone;
pub use crate::contacts::Contacts;
/// For `crate::session_hooks`' contact-presence row (seam S14).
pub(crate) use crate::contacts::contact_presence;
/// For `push::spawn_world_ticker`: skill levels whose time has come.
pub(crate) use crate::skill_queue::skill_training_tick;
/// The agent rowset, now in `service/agents.rs` (seam S5).
pub use agents::agent_rowset;
pub use corp::Corporations;
pub use market::Book as MarketBook;
pub use world::Bookmarks;

/// For `crate::session_hooks`' log-off row (seam S14): `space` keeps the
/// warp `pub(super)`, so it leaves this module through here.
pub(crate) fn emergency_warp(reg: &mut Registry, actor: i64, now: crate::damage::BlueTime) {
    space::emergency_warp(reg, actor, now);
}

/// Every subsystem module, paired with the services it answers for.
///
/// `service.rs` was 7374 lines in one file, and that -- not anything about
/// the work -- was what held parallel implementation to a single agent. The
/// arms now live one subsystem per file, so a lane adding a contracts method
/// and a lane adding a market method never open the same file. Each module
/// owns its services, its helpers as they migrate, and **its own `mod
/// tests`**, which is the other half of the problem: two lanes each adding a
/// test would otherwise contend for one test module.
///
/// A service appearing in two modules' `SERVICES` is a bug the first match
/// wins silently, so there is a test that the lists are disjoint.
pub type DomainDispatch = fn(&mut Registry, &Request<'_>) -> Result<PyValue, ServiceError>;
pub(crate) const DOMAINS: &[(&[&str], DomainDispatch)] = &[
    (base::SERVICES, base::dispatch),
    (character::SERVICES, character::dispatch),
    // The agents, out of `character` (W10.5 seam S5).
    (agents::SERVICES, agents::dispatch),
    (inventory::SERVICES, inventory::dispatch),
    // L2 SKINS: the ship SKINs (`shipCosmeticsMgr`), out of `inventory`.
    (cosmetics::SERVICES, cosmetics::dispatch),
    // Ship insurance (W16 CP44), out of `inventory`.
    (insurance::SERVICES, insurance::dispatch),
    // `warStatisticMgr.GetKillMail`: the kill report window (SOC-C6).
    (killmail::SERVICES, killmail::dispatch),
    // GAP-CALENDAR: the calendar, out of `live`.
    (calendar::SERVICES, calendar::dispatch),
    (wallet::SERVICES, wallet::dispatch),
    (corp::SERVICES, corp::dispatch),
    // S1 (#158): the recruitment adverts, empty lists.
    (corp_rec::SERVICES, corp_rec::dispatch),
    // C8: `warRegistry`, `warsInfoMgr`, the mutual-war invites, treaties and
    // bills.
    (war::SERVICES, war::dispatch),
    // SOC-C1: alliances, beside the corps they are made of.
    (alliance::SERVICES, alliance::dispatch),
    (space::SERVICES, space::dispatch),
    // W10.5 seam S2: split out of `space`, one module per subsystem.
    // S1: `overviewPresetMgr` and `sharedSettingsMgr`, the chat share links.
    (share_links::SERVICES, share_links::dispatch),
    (sites::SERVICES, sites::dispatch),
    // `keeper.ActivateAccelerationGate`: the gates between a site's rooms
    // (Y-COMBAT).
    (keeper::SERVICES, keeper::dispatch),
    // `hackingMgr`: the hacking window's inputs (R-HACK HK2).
    (hacking::SERVICES, hacking::dispatch),
    (facwar::SERVICES, facwar::dispatch),
    // `facWarMgr` and enlistment (W16 CP53a).
    (facwar_mgr::SERVICES, facwar_mgr::dispatch),
    (bounty::SERVICES, bounty::dispatch),
    // #89 ESS-1: `essMgr`, the ESS main bank's panel data.
    (ess::SERVICES, ess::dispatch),
    // MINE-UI: the survey scanner and the Mining Ledger window.
    (mining_ui::SERVICES, mining_ui::dispatch),
    (tales::SERVICES, tales::dispatch),
    (world::SERVICES, world::dispatch),
    // U2-UPWELL: the Upwell structure directory, profiles, settings, guests.
    (structures::SERVICES, structures::dispatch),
    // SOC-D1: sovereignty (`sovMgr`, `devIndexManager`).
    (sov::SERVICES, sov::dispatch),
    // `shipStanceMgr.SetShipStance`: a tactical destroyer's mode (T3-E).
    (stance::SERVICES, stance::dispatch),
    // `superWeaponMgr`: a Lance's aim (NW-9a).
    (super_weapon::SERVICES, super_weapon::dispatch),
    // `ownerGroupManager`: access groups (GAP-ACCESS).
    (owner_groups::SERVICES, owner_groups::dispatch),
    (social::SERVICES, social::dispatch),
    // `mailMgr` and `mailingListsMgr`: the mail window (MAIL-1).
    (mail::SERVICES, mail::dispatch),
    // `notificationMgr`: the notification centre and window (NOTIF-1).
    (notifications::SERVICES, notifications::dispatch),
    (store::SERVICES, store::dispatch),
    (market::SERVICES, market::dispatch),
    (industry::SERVICES, industry::dispatch),
    (reprocess::SERVICES, reprocess::dispatch),
    // `crateService`: opening a crate, docked (L6A-GEODES).
    (crate_svc::SERVICES, crate_svc::dispatch),
    // GAP-REPAIR: the Repair Shop window.
    (repair::SERVICES, repair::dispatch),
    (planet::SERVICES, planet::dispatch),
    (live::SERVICES, live::dispatch),
    // Lane B1's breaker sweep: typed empties for unrouted services whose
    // consumer throws on `None` (`docs/lanes/breaker-sweep.md`).
    (live_misc::SERVICES, live_misc::dispatch),
    (gm::SERVICES, gm::dispatch),
    (fleet::SERVICES, fleet::dispatch),
    // GAP-TRADE: `trademgr` and its world-minted session object.
    (trade::SERVICES, trade::dispatch),
    // PROG-MINE: `inSpaceCompressionMgr`, the Compress menu entry.
    (crate::compression::SERVICES, crate::compression::dispatch),
    // Drone commands: the `entity` bound object (`docs/lanes/drones.md`).
    (crate::drones::SERVICES, crate::drones::dispatch),
    // `wormholeMgr`: the wormhole jump (W17 CP54b).
    (
        crate::wormhole_jump::SERVICES,
        crate::wormhole_jump::dispatch,
    ),
    // `randomJumpMgr`: the Needlejack filaments (#88).
    (crate::random_jump::SERVICES, crate::random_jump::dispatch),
    // `abyssalMgr`: the Abyss probe (#91).
    (crate::abyss::SERVICES, crate::abyss::dispatch),
    // `econAgent`: a bot parks its career at log-off (E1).
    (
        crate::econ_agents::wire::SERVICES,
        crate::econ_agents::wire::dispatch,
    ),
];

/// A read arm (PA-12a): `None` is "not a read I serve", and the caller falls
/// back to the write path. `&Registry`, so it can neither mutate nor queue a
/// push; the interior caches it reaches (`Mutex` slots) are pure memoisation.
/// `Caller` is explicit because [`Registry::caller`] is only set inside
/// [`Registry::dispatch_as`]; an arm that needs it must not be listed here.
pub(crate) type DomainRead =
    fn(&Registry, Caller, &Request<'_>) -> Option<Result<RoValue, ServiceError>>;

/// A read arm's answer (PERF-BURST): built here, or the kept `Arc` itself.
/// The read turn drops the world guard **before** it encodes, and a `Shared`
/// answer is encoded from the `Arc` with no copy of the value at all (the
/// `config.GetAverageMarketPrices` dict is ~12k entries and 300 clients ask
/// at login).
#[derive(Debug, Clone)]
pub enum RoValue {
    Owned(PyValue),
    Shared(std::sync::Arc<PyValue>),
}

impl RoValue {
    /// The value to encode, with no copy.
    pub fn as_value(&self) -> &PyValue {
        match self {
            RoValue::Owned(v) => v,
            RoValue::Shared(a) => a,
        }
    }

    /// An owned value (a deep copy for `Shared` unless it is the last handle).
    pub fn into_owned(self) -> PyValue {
        match self {
            RoValue::Owned(v) => v,
            RoValue::Shared(a) => std::sync::Arc::try_unwrap(a).unwrap_or_else(|a| (*a).clone()),
        }
    }
}

/// The `(service, method)` pairs a domain's read arm answers, and the arm.
/// Eligibility is decided from this table alone, with no lock
/// ([`Registry::read_eligible`]). The write arms stay the reference
/// implementation; a test compares the two for every row.
pub(crate) const READS: &[(&[(&str, &str)], DomainRead)] = &[
    (base::READ_METHODS, base::dispatch_ro),
    (sites::READ_METHODS, sites::dispatch_ro),
    (character::READ_METHODS, character::dispatch_ro),
    (inventory::READ_METHODS, inventory::dispatch_ro),
];

/// Who is making the call being dispatched (`docs/lanes/account-spec.md`
/// §3.5).
///
/// **`Internal` is a server-made call**: the ticker, a test, an arm
/// re-dispatching. `Session` is a client payload, set by `dispatch.rs`
/// through [`Registry::dispatch_as`] for exactly one dispatch. A field here
/// rather than on [`Request`] because `Request` has about 160 literal sites;
/// a lookup by connection was the other option, and arms do not know their
/// `client_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    Internal,
    Session { user_id: i64, role: i64 },
}

pub struct Registry {
    config: SandboxConfig,
    /// Character id -> position in `config.characters` (PERF-STAGES), kept by
    /// [`Self::add_character`]; `char_indexed` is how many characters it
    /// covers, and a list of another length (a test pushing to it) is
    /// scanned instead.
    char_index: crate::item::IdMap<usize>,
    char_indexed: usize,
    /// Who the dispatch in progress is for; [`Caller::Internal`] outside
    /// [`Registry::dispatch_as`]. Never persisted.
    caller: Caller,
    /// The Bio tab's text, for as long as this process lives.
    ///
    /// `bioPanel.ApplyAttributes` builds a fresh panel on every character
    /// sheet open and `LoadBioFromServer` re-reads us each time, so without
    /// somewhere to keep this the bio the player typed is replaced by the
    /// placeholder the next time they look -- which is what the capture
    /// shows: `SetCharacterDescription(u"hello")`, and nothing afterwards
    /// holding "hello".
    ///
    /// **Keyed by character**, because the protocol already is: the client
    /// calls `GetCharacterDescription(session.charid)` and we used to
    /// discard the argument and hold one `String` for the whole server. Two
    /// players then shared one bio -- whoever saved last owned it, and the
    /// other player's character sheet showed it as their own.
    ///
    /// Absent means the character has never saved one, which is a different
    /// thing from the empty string and renders the same.
    character_description: std::collections::BTreeMap<i64, String>,
    /// ESS push throttle (#89): system -> (last `OnSystemESSDataChanged` sent,
    /// a deposit since). Never persisted; see `crate::ess::tick`.
    ess_told: std::collections::BTreeMap<i64, (i64, bool)>,
    /// Notifications the world owes the client, drained by `dispatch.rs` after
    /// every payload.
    ///
    /// **An arm can only ask.** There is no socket, no session and no node id
    /// in scope here, which is what makes "never hold a lock across a socket
    /// write" (`docs/reference/world-state.md` §4 rule 4) a property of the types rather
    /// than a rule to remember once this sits behind an `RwLock`.
    pending: Vec<(Duration, Box<dyn crate::notify::Push>)>,
    /// Notifications the world owes the client at a **time of its own
    /// choosing**, drained by `push::spawn_world_ticker` and by nothing else.
    ///
    /// Deliberately a second queue rather than a longer delay on `pending`;
    /// [`Registry::schedule`] has the reason, which is an ordering guarantee
    /// `destiny::on_ballpark_bind` relies on.
    scheduled: Vec<(Instant, Box<dyn crate::notify::Push>)>,
    /// The instant of the clock tick now running, or `None` outside one.
    /// `schedule` counts from it (lane tick-schedule, #136).
    tick_now: Option<Instant>,
    /// The last Destiny stamp this world handed out.
    ///
    /// **Every stamp this server sends must be non-decreasing, for ever**, and
    /// the reason is three client behaviours stacked on each other
    /// (`docs/lanes/movement.md` §4, all **[V]**):
    ///
    /// - a batch whose stamp is below the last `SetState`'s is dropped before
    ///   it reaches the dispatcher, with `Michelle discarded a state that was
    ///   too old` (`michelle.py:1443-1448`);
    /// - a batch below the park's clock needs a snapshot to rewind to, and
    ///   applying any action sets `shouldRebase`, which makes the next
    ///   `DoPostTick` wipe the snapshot ring (`FlushSimulationHistory:1375`) --
    ///   so the rewind window collapses to "now" after the first command;
    /// - failing to rewind is `OnRecoverableDesync` + `RequestReset`, after
    ///   which `validState` is false and **every** later batch is silently
    ///   ignored until a fresh `SetState`.
    ///
    /// So the allocator clamps: `max(tick(), last)`. The one way our side can
    /// break the rule is a wall clock that steps backwards -- NTP, a container
    /// restart, a suspended laptop -- and the clamp costs one comparison and
    /// removes the whole class.
    last_destiny_stamp: u32,
    /// The stamp of the last mid-session `SetState` sent to each character
    /// ([`Self::next_setstate_stamp`]).
    setstate_at: std::collections::HashMap<i64, u32>,
    /// Everyone in space: where they are, in what, and doing what.
    ///
    /// **The server's only representation of "undocked".**
    /// `Character::station_id` is never cleared -- nothing in the tree assigns
    /// it after `Registry::new` -- so before this existed the world could not
    /// answer the question at all, and `characters_at_station` listed pilots
    /// who were in space as station guests.
    ///
    /// **Not in the snapshot**, for the same reason fleets are not: a world
    /// reloaded from disk has nobody in space, because a session is what puts
    /// a ship in a ballpark and sessions do not survive the process. A
    /// position for a ship nobody is flying is a row that cannot be read back
    /// into anything.
    roster: crate::space_state::Roster,
    /// Every station a character has been attached to since start: the
    /// seven fields that put a pilot in one (`character::StationRef`), by
    /// station id. **Where [`Self::dock_target`] looks a station up** (FX1).
    /// This server has no station table, so without it a seed station that
    /// its last pilot docked away from would stop being a station at all.
    /// Rebuilt by [`Self::seed_character`]; not persisted, because it is
    /// derived from the characters.
    stations: std::collections::BTreeMap<i64, character::StationRef>,
    /// Who has a lock on whom, by ship id.
    ///
    /// **Not persisted, and not in `store::Snapshot`.** A lock is a fact
    /// about two ships that are both in space right now; a restart takes
    /// every ship out of space, so a lock that survived one would name balls
    /// in nobody's park. `crate::targeting`'s header has the rest.
    locks: crate::targeting::Locks,
    /// Online state, running effects and drained capacitors.
    /// `crate::modules`'s header has the rest; not persisted, like `locks`.
    modules: crate::modules::Modules,
    /// Every ship's health and every wreck. `crate::combat`'s header has the
    /// rest; not persisted, like `modules`.
    combat: crate::combat::Combat,
    /// Every live NPC. `crate::npc`'s header has the rest; not persisted,
    /// like `roster`: an NPC is a ball in space, and a restart empties space.
    npcs: crate::npc::Npcs,
    /// Stargate jumps in flight (`crate::jump`). Not persisted, like
    /// `roster`: a restart empties space, and a jump is a way through it.
    jumps: crate::jump::Jumps,
    /// Probes in space and each scanner's last results (`crate::scan`).
    /// Not persisted, like `roster`: a restart empties space.
    scan: crate::scan::Probes,
    /// Session changes a mutator asked for, `(character id, change)`,
    /// drained by `dispatch.rs` after the call -- the same shape as
    /// `Fleets::pending_sessions`, for everything that is not a fleet.
    ///
    /// **Queued by the mutator, on success only.** `with_session_change` runs
    /// on the error path too (`dispatch.rs`), so a change keyed on the method
    /// name -- `session_change_for` -- would also fire for a call that was
    /// refused. A refused `ship.Board` must not move anybody into a ship
    /// (`docs/lanes/fitting.md`).
    pending_session_changes: Vec<(i64, Vec<(PyValue, PyValue)>)>,
    /// Everything the world owns.
    ///
    /// Seeded in `new` with the capsule the character is sitting in, which
    /// used to be synthesised from `Character::ship_id` at every read. The
    /// difference matters the moment anything else is in the hangar: a
    /// synthesised row cannot be moved, repackaged or destroyed, and every
    /// consumer would have had to know which rows were real.
    store: crate::item::Store,
    /// Where the ISK went. tracker #1.
    ///
    /// Append-forever and read by range, which is the shape that argues for
    /// SQLite over a snapshot file once this stops being one character --
    /// see `docs/reference/world-state.md` §3.
    journal: crate::ledger::Journal,
    /// Skills, per character.
    ///
    /// Keyed by character id rather than held on `Character` because skills
    /// are not a field of the select screen's row -- and because the client's
    /// whole model of them is a cache filled from one bound object, not the
    /// item store. See `crate::skill` for why that distinction is forced
    /// rather than chosen.
    pub(crate) skills: std::collections::BTreeMap<i64, crate::skill::Skills>,
    /// Who is connected right now, and which character each of them is.
    ///
    /// Shared like the rest of the world and **never persisted**
    /// (`docs/reference/world-state.md` §2): a stored "who is online" is a list of
    /// people who are not. `crate::session_table` has the shape and the
    /// identity/presence distinction it keeps.
    sessions: crate::session_table::SessionTable,
    /// Fleets, in RAM, never persisted. `crate::fleet` says why.
    fleets: crate::fleet::Fleets,
    /// Every colony, and the static table they simulate against.
    ///
    /// `pub(crate)` because the arm reads it directly: unlike fleets, the
    /// colony simulation is a pure function of a colony and a `now`, so there
    /// is nothing for a `Registry` method to coordinate.
    pub(crate) planet: crate::planet::Colonies,
    /// Every industry job the world holds.
    ///
    /// `crate::industry::Jobs` owns the logic and the refusals; this is only
    /// where they live.
    ///
    /// **Persisted, and unlike fleets that is not optional.** A fleet is a
    /// conversation and a stored one is wrong (`crate::fleet`'s header says
    /// why). A job is the opposite: a promise with a clock on it, whose
    /// materials have *already* been consumed by `install_job`. A restart
    /// that dropped the promise and kept the consumption is the one failure
    /// mode here that costs a player something, which is why this field
    /// carried a warning not to restart a world with jobs in it until the
    /// `store.rs` record existed. It exists: `job` and `nextjob` lines, the
    /// allocator included, and
    /// `service::tests::an_industry_job_survives_the_world_file` installs a
    /// job through the arm and reads it back out of a real file.
    jobs: crate::industry::Jobs,
    /// The Industry window's material monitors (`industry::Monitors`):
    /// transient, never saved; a reconnect makes a new one.
    industry_monitors: industry::Monitors,
    /// Every market order and every trade: `service::market::Book`, whose
    /// doc has the rest. Persisted as `store::Snapshot::market` lines.
    market: market::Book,
    /// Every contract and its escrow bookkeeping: `crate::contracts`, whose
    /// doc has the rest. Persisted as `store::Snapshot::contracts` lines; the
    /// `contractProxy` arms in `service/wallet.rs` are its only writers.
    contracts: crate::contracts::Contracts,
    /// NPC standings toward each character. The type, its rules and the
    /// pushes that keep the client in step live in `crate::standings`; this
    /// is only where they are kept.
    pub(crate) standings: crate::standings::Standings,
    /// Bookmark folders, subfolders and bookmarks. Persisted; `world.rs`'s
    /// bookmark section has the shape and why.
    bookmarks: world::Bookmarks,
    /// Every character's address book and block list. Persisted;
    /// `crate::contacts` has the shape and why.
    pub(crate) contacts: crate::contacts::Contacts,
    /// Home stations, jump clones, and revived capsules. Persisted;
    /// `crate::clones` has the shape and why.
    pub(crate) clones: crate::clones::Clones,
    /// Every character's skill training queue. Persisted; `crate::skill`'s
    /// training section has the shape, `crate::skill_queue` the glue.
    pub(crate) skill_queues: crate::skill::Queues,
    /// Player corporations, their members and the applications between
    /// them. Persisted; the player-corporation section of `service/corp.rs`
    /// has the shape and why.
    corporations: corp::Corporations,
    /// The chat hub, when chat is on (`--chat`). `None` is today's
    /// behaviour: `XmppChatMgr` answers `None` and the client never dials.
    ///
    /// **Lock order: the world, then the hub.** A service arm holds the
    /// world write lock and may then take the hub; `crate::chat` never holds
    /// the hub while waiting for the world. Not persisted: chat is presence.
    chat: Option<std::sync::Arc<crate::chat::ChatHub>>,
    /// The loaded data, shared and read-only (`docs/content/plan.md` §1.5).
    /// [`Self::new`] uses every registered table's fixture; the server with
    /// a bundle builds one with [`Self::with_content`].
    pub(crate) content: std::sync::Arc<crate::content::Content>,
    /// Module state that has no field here, by type: `crate::world_ext`,
    /// built from `world_ext::SLOTS`. Read with [`Self::ext`].
    ext: crate::world_ext::Slots,
    /// The direct trades in progress (GAP-TRADE). Not persisted: the staged
    /// items are (`crate::trade::KIND`) and a load returns them.
    pub(crate) trades: crate::trade::Trades,
    /// Bound objects the **world** minted, by id: `(bound service name, bind
    /// params)`. A per-session `bound_objects` entry is only good on the
    /// connection that was handed it; a reference pushed to another pilot
    /// (`OnTradeInitiate`) has to resolve on theirs, so `handle_bound_call`
    /// falls back to this table. Not persisted.
    world_objects: std::collections::BTreeMap<String, (String, PyValue)>,
    /// The next `N=<node>:w<seq>` serial.
    world_object_seq: u64,
    /// The world's changes against what the generators make, and the world
    /// record (`crate::delta`). Persisted whole by `store.rs`'s `world` and
    /// `delta` records.
    deltas: crate::delta::Deltas,
    /// Set by anything that changes durable state; cleared by the save.
    ///
    /// **A flag rather than saving on every payload**, because
    /// `docs/reference/world-state.md` §4 rule 4 forbids store I/O on the synchronous
    /// draw path -- `GetSelfInvItem` and `dogmaIM.GetAllInfo` are called while
    /// the client is drawing, and an fsync there is a hang rather than a
    /// slowdown. With this flag the only payloads that ever touch the disk are
    /// the ones that changed something.
    dirty: bool,
    /// Set when a payload panicked while holding the world lock and the lock
    /// was recovered anyway (review M5, ruling 10). A half-written change may
    /// be in memory, so the store must not save it over the good file
    /// (IP11). Sticky: nothing clears it but a restart. Set by
    /// [`Self::taint`], which nothing calls yet (IP9 will, at the recovery
    /// in `dispatch.rs`).
    tainted: bool,
}

impl Registry {
    pub fn new(mut config: SandboxConfig) -> Self {
        // Read before `config` is moved into the struct literal below.
        let node_id_for_fleets = config.node_id;
        // Seeded one by one below, through the one function that seeds a
        // character whatever it came from (flags, the world, an account).
        let characters = std::mem::take(&mut config.characters);
        let skills = std::collections::BTreeMap::new();
        let store = crate::item::Store::new();
        let content = std::sync::Arc::new(crate::content::Content::fixture());
        let ext = crate::world_ext::Slots::build(&crate::module::plan().slots, &content);
        let mut reg = Registry {
            config,
            char_index: Default::default(),
            char_indexed: 0,
            caller: Caller::Internal,
            character_description: std::collections::BTreeMap::new(),
            ess_told: std::collections::BTreeMap::new(),
            pending: Vec::new(),
            scheduled: Vec::new(),
            tick_now: None,
            store,
            journal: crate::ledger::Journal::new(),
            skills,
            sessions: crate::session_table::SessionTable::default(),
            fleets: crate::fleet::Fleets::new(node_id_for_fleets),
            planet: crate::planet::Colonies::new(),
            jobs: crate::industry::Jobs::new(),
            industry_monitors: industry::Monitors::default(),
            market: market::Book::default(),
            contracts: crate::contracts::Contracts::default(),
            standings: crate::standings::Standings::default(),
            last_destiny_stamp: 0,
            setstate_at: std::collections::HashMap::new(),
            roster: crate::space_state::Roster::default(),
            stations: std::collections::BTreeMap::new(),
            locks: crate::targeting::Locks::default(),
            modules: crate::modules::Modules::default(),
            combat: crate::combat::Combat::default(),
            npcs: crate::npc::Npcs::default(),
            jumps: crate::jump::Jumps::default(),
            scan: crate::scan::Probes::default(),
            pending_session_changes: Vec::new(),
            bookmarks: world::Bookmarks::default(),
            contacts: crate::contacts::Contacts::default(),
            clones: crate::clones::Clones::default(),
            skill_queues: crate::skill::Queues::default(),
            corporations: corp::Corporations::default(),
            chat: None,
            content,
            ext,
            trades: crate::trade::Trades::default(),
            world_objects: std::collections::BTreeMap::new(),
            world_object_seq: 0,
            deltas: {
                // Never without a world record (review CA6): `server.rs`
                // replaces the fixture on load or mint.
                let mut d = crate::delta::Deltas::default();
                d.set_world(crate::delta::World::FIXTURE);
                d
            },
            dirty: false,
            tainted: false,
        };
        for c in characters {
            reg.seed_character(c);
        }
        reg
    }

    /// Add one character to the world: its starter skills, its capsule, and
    /// its place on the roster (`docs/lanes/account-spec.md` §3.3). The
    /// per-character half of [`Self::new`], so a character from `--character`,
    /// from the world file or from an account's `start_character` is seeded
    /// the same way.
    ///
    /// # Panics
    ///
    /// **Two characters sharing a `ship_id` would silently corrupt the
    /// store.** `Store::insert` is a `push` with no uniqueness check, so the
    /// capsule below would be a second item with one id and `GetSelfInvItem`
    /// would match whichever came first. Caught here, where a fixture or a
    /// config is wrong, rather than in an inventory window that shows the
    /// wrong hull. Likewise two characters with one `character_id`.
    pub fn seed_character(&mut self, c: Character) {
        for other in &self.config.characters {
            assert_ne!(
                c.ship_id, other.ship_id,
                "characters {} and {} share ship_id {}; the item store \
                 would hold two items with one id",
                other.character_id, c.character_id, c.ship_id
            );
            assert_ne!(
                c.character_id, other.character_id,
                "two characters with character_id {}; `Registry::character` \
                 answers one of them and which is undefined",
                c.character_id
            );
        }
        self.skills.insert(
            c.character_id,
            crate::skill::Skills::starter_in(&self.content),
        );
        // The capsule is an ordinary item, and its id is the character's
        // `ship_id` rather than one the store allocates -- the session
        // already told the client that id, and `GetSelfInvItem` matches on
        // it.
        self.store.insert(crate::item::Item {
            item_id: c.ship_id,
            type_id: TYPE_CAPSULE,
            owner_id: c.character_id,
            location_id: c.station_id.unwrap_or(0),
            flag_id: crate::model::FLAG_HANGAR,
            quantity: 1,
            group_id: GROUP_CAPSULE,
            category_id: crate::model::CATEGORY_SHIP,
            singleton: true,
        });
        if let Some(station) = character::StationRef::of(&c) {
            self.stations.entry(station.station_id).or_insert(station);
        }
        // Only a list the index covers stays covered: one changed behind it
        // (a test pushing to it) is scanned from then on.
        let covered = self.char_indexed == self.config.characters.len();
        self.char_index
            .entry(c.character_id)
            .or_insert(self.config.characters.len());
        self.config.characters.push(c);
        if covered {
            self.char_indexed = self.config.characters.len();
        }
    }

    /// Whether `caller` may play `c` (account-spec §4.6): the one predicate
    /// behind the select screen and `SelectCharacterID`.
    ///
    /// Without `--accounts` (`catch_all` set) everyone may play everything,
    /// as before accounts. With it, a session plays only its own characters;
    /// an unowned one is nobody's until a `start_character` adopts it [A].
    /// A server-made call ([`Caller::Internal`]) is refused only for a deleted
    /// character.
    pub fn may_play(&self, caller: Caller, c: &Character) -> bool {
        // A deleted character is nobody's (SOC-CHARDEL): off the select
        // screen, the count and `SelectCharacterID`, its row kept for names.
        if crate::start::is_deleted(&self.deltas, c.character_id) {
            return false;
        }
        if self.config.catch_all.is_some() {
            return true;
        }
        match caller {
            Caller::Internal => true,
            Caller::Session { user_id, .. } => c.owner_user_id == Some(user_id),
        }
    }

    /// [`Self::character_change`] for `caller`, or `None` when `caller` may
    /// not play that character (§4.6). Without this a friend who sends
    /// another account's `characterID` plays it, and the select screen's
    /// filter is cosmetic.
    pub fn character_change_as(
        &self,
        caller: Caller,
        character_id: i64,
    ) -> Option<Vec<(PyValue, PyValue)>> {
        let c = self.character(character_id)?;
        if !self.may_play(caller, c) {
            return None;
        }
        self.character_change(character_id)
    }

    /// Give each account its `start_character`, once (account-spec §3.4).
    ///
    /// Called by `Server::bind` after the world is applied. For each
    /// `(user_id, name)` in file order, the character named `name`:
    ///
    /// 1. owned by this account: nothing to do;
    /// 2. unowned: adopted. This is how a world saved before accounts gets
    ///    its owners [A];
    /// 3. owned by another account: `Err`, naming both, and startup stops;
    /// 4. absent: created by [`Self::create_owned_character`]: placed at
    ///    its school and given the starting funds (P-START-2), with
    ///    `character_id` one above the largest in the world and `ship_id`
    ///    from the item allocator, so it cannot collide with an item.
    ///
    /// Adopting or creating marks the world dirty, so the next save writes
    /// it and the next start finds it by name (case 1). That is "once".
    /// Names match exactly, case included [A].
    pub fn provision_start_characters(&mut self) -> Result<(), String> {
        for (user_id, name) in self.config.start_characters.clone() {
            let found = self.config.characters.iter_mut().find(|c| c.name == name);
            match found {
                Some(c) => match c.owner_user_id {
                    Some(owner) if owner == user_id => {}
                    None => {
                        c.owner_user_id = Some(user_id);
                        self.dirty = true;
                    }
                    Some(owner) => {
                        return Err(format!(
                            "start_character {name:?} of account {user_id} is already \
                             owned by account {owner}"
                        ));
                    }
                },
                None => {
                    self.create_owned_character(user_id, name);
                }
            }
        }
        Ok(())
    }

    /// **The account mark feeds the one predicate** (BOTMARK): every
    /// character owned by a bot-marked account (`crate::account_book`,
    /// `eve-server --mark-bot`) is `Character::is_bot`, the flag
    /// `crate::liveness` and `crate::econ_ledger` read. Only ever sets it,
    /// so a `--character NAME,bot` roster bot stays one. Called by
    /// [`Self::apply_snapshot`] and [`Self::create_character`].
    pub fn mark_bot_characters(&mut self) {
        let bots = crate::account_book::bot_accounts(&self.deltas);
        if bots.is_empty() {
            return;
        }
        for c in &mut self.config.characters {
            if c.owner_user_id.is_some_and(|u| bots.contains(&u)) {
                c.is_bot = true;
            }
        }
    }

    /// A new character owned by `user_id`, returning its id: case 4 above,
    /// and a world account's sign-up (`crate::account_book`). Neither knows
    /// a race, so it is [`crate::start::NewCharacter::unchosen`]'s **[A]**,
    /// and the school is that race's default; see [`Self::create_character`].
    pub fn create_owned_character(&mut self, user_id: i64, name: String) -> i64 {
        self.create_character(user_id, name, crate::start::NewCharacter::unchosen())
    }

    /// A new character owned by `user_id`, returning its id: every creation
    /// path's one body (P-START-2). `character_id` is one above the largest
    /// in the world and `ship_id` comes from the item allocator, so it
    /// cannot collide with an item. The character is placed at its school
    /// **before** it is seeded, so its capsule is created there
    /// (`crate::start::place_or_keep`: without a bundle it keeps
    /// [`Character::default`]'s station and corp), given its race's starter
    /// skills (`crate::start::give_starter_skills`: without a bundle it keeps
    /// the seeded sheet), then credited the starting funds once. Marks the
    /// world dirty.
    pub(crate) fn create_character(
        &mut self,
        user_id: i64,
        name: String,
        new: crate::start::NewCharacter,
    ) -> i64 {
        let base = Character::default();
        let character_id = self
            .config
            .characters
            .iter()
            .map(|c| c.character_id + 1)
            .max()
            .unwrap_or(base.character_id);
        let ship_id = self.store.next_id();
        let mut c = Character {
            character_id,
            name,
            ship_id,
            owner_user_id: Some(user_id),
            race_id: new.race_id,
            bloodline_id: new.bloodline_id,
            gender_id: new.gender_id,
            ..base
        };
        c.is_bot = crate::account_book::is_bot_account(&self.deltas, user_id);
        crate::start::place_or_keep(self, &mut c, new.school_id);
        let race_id = c.race_id;
        self.seed_character(c);
        crate::start::give_starter_skills(self, character_id, race_id);
        crate::start::fund_new_character(self, character_id, crate::pyutil::filetime_now());
        self.dirty = true;
        character_id
    }

    /// Place each `--character` the world file does not know yet
    /// (`SandboxConfig::fresh_roster` minus `saved`) at its school, moving
    /// the capsule [`Self::new`] already seeded, and return their ids. Called
    /// by `Server::bind` **before** `apply_snapshot`, so a saved character is
    /// never re-placed; the caller credits the returned ids
    /// (`crate::start::fund_new_character`) **after** it, so the journal row
    /// is not replaced by the file's journal.
    pub fn start_roster(&mut self, saved: &std::collections::BTreeSet<i64>) -> Vec<i64> {
        let mut placed = Vec::new();
        for (id, school) in self.config.fresh_roster.clone() {
            if saved.contains(&id) {
                continue;
            }
            let Some(mut c) = self.character(id).cloned() else {
                continue;
            };
            crate::start::place_or_keep(self, &mut c, school);
            if let Some(station) = c.station_id {
                let content = std::sync::Arc::clone(&self.content);
                let moved = self.store.apply_in(
                    &content,
                    &[crate::item::Edit::Move {
                        item_id: c.ship_id,
                        to: crate::item::Slot::new(
                            c.character_id,
                            station,
                            crate::model::FLAG_HANGAR,
                        ),
                    }],
                );
                if let Err(e) = moved {
                    crate::errln!("[start] {:?}: capsule not moved: {e:?}", c.name);
                }
            }
            if let Some(station) = character::StationRef::of(&c) {
                self.stations.entry(station.station_id).or_insert(station);
            }
            if let Some(slot) = self.character_mut(id) {
                *slot = c;
            }
            placed.push(id);
        }
        placed
    }

    /// The same world on other loaded data: a bundle's, or a test's own rows.
    ///
    /// The slots are rebuilt from the new content, so this is for a world
    /// just made by [`Self::new`], before anything has written to them.
    pub fn with_content(mut self, content: std::sync::Arc<crate::content::Content>) -> Self {
        self.ext = crate::world_ext::Slots::build(&crate::module::plan().slots, &content);
        self.content = content;
        self
    }

    /// The loaded data (`docs/content/plan.md` §1.5).
    pub fn content(&self) -> &crate::content::Content {
        &self.content
    }

    /// The slot of type `T` (`crate::world_ext`).
    ///
    /// # Panics
    ///
    /// When no `SlotSpec` in `world_ext::SLOTS` makes a `T`: a missing
    /// registration, which the module's first test hits.
    pub fn ext<T: std::any::Any>(&self) -> &T {
        self.ext
            .get()
            .unwrap_or_else(|| crate::world_ext::unregistered::<T>())
    }

    /// The same, mutably.
    pub fn ext_mut<T: std::any::Any>(&mut self) -> &mut T {
        self.ext
            .get_mut()
            .unwrap_or_else(|| crate::world_ext::unregistered::<T>())
    }

    /// The world's delta rows (`crate::delta`).
    pub fn deltas(&self) -> &crate::delta::Deltas {
        &self.deltas
    }

    /// The same, mutably, and marks the world dirty: deltas are durable,
    /// and the save runs under the same lock as the change
    /// (`docs/reference/world-state.md` §4 rule 3).
    pub fn deltas_mut(&mut self) -> &mut crate::delta::Deltas {
        self.dirty = true;
        &mut self.deltas
    }

    /// The world record: the seed, birth and day every generator is a
    /// function of. Never `None` (review CA6): [`Self::new`] starts with
    /// [`crate::delta::World::FIXTURE`] and [`Self::apply_snapshot`] keeps the
    /// current record when a snapshot carries none.
    pub fn world(&self) -> &crate::delta::World {
        self.deltas.world().unwrap_or(&crate::delta::World::FIXTURE)
    }

    /// Mark the in-memory world as possibly half-written (see the field).
    pub fn taint(&mut self) {
        self.tainted = true;
    }

    /// Whether [`Self::taint`] has been called since start: the store then
    /// never writes `world.dat` (IP11).
    pub fn tainted(&self) -> bool {
        self.tainted
    }

    /// The next Destiny stamp, clamped so that it never goes backwards.
    ///
    /// See [`Self::last_destiny_stamp`] for why the clamp is not belt and
    /// braces. Equal stamps in consecutive batches are free and safe -- the
    /// client's `SynchroniseToSimulationTime` evolves `range(0)` times -- so
    /// several commands inside one second share a stamp and nothing is lost.
    pub fn next_destiny_stamp(&mut self) -> u32 {
        let stamp = crate::destiny::tick().max(self.last_destiny_stamp);
        self.last_destiny_stamp = stamp;
        stamp
    }

    /// The stamp for a **mid-session** `SetState` to `to` (desync recovery, a
    /// held ship's rejoin), or `None` when one is already on its way.
    ///
    /// The client's `Park.FlushState` purges history entries strictly older
    /// than a `SetState`'s stamp and *extends* one at the same stamp; its
    /// `RealFlushState` then honours a `SetState` only as the first entry of a
    /// batch (`michelle.pyc:1423-1545` **[V]**, PA-10 report 1.2-1.4). Our
    /// stamps lead the client by 1-2 ticks, so an entry at
    /// [`Self::last_destiny_stamp`] may still be queued there, and the state
    /// must open its own: `last + 1`.
    ///
    /// **Bounded: never more than `tick + 2`**, the lead `DoPreTick` is known
    /// to hold (`stamp - currentTime < 3`). The floor is world-wide, and bots
    /// resync often, so an unbounded `+1` per call would walk every client's
    /// stamps away from the clock. Two things keep it bounded:
    ///
    /// - **a client with a `SetState` still ahead of the clock gets the same
    ///   stamp again, and the floor does not move.** Its client time is at
    ///   most `tick`, so an entry stamped past `tick` has not flushed; the
    ///   repeat lands in that same entry behind the first (skipped by the
    ///   client, harmless), and a bot, which reads the frame's own state,
    ///   still gets the answer it asked for. One floor step per client per
    ///   tick, however often it asks.
    /// - **the stamp is capped at `max(tick + 2, last)`.** Past the cap the
    ///   stamp can equal an entry a client holds -- the third distinct client
    ///   to resync in one second, at most -- and that state may then be
    ///   skipped client-side **[A]**: falsified by a human client that, after
    ///   a `RequestReset` during a bot storm, keeps "Events ignored" going.
    ///
    /// Not for a first bind: the park is empty then, and every undock moving
    /// the floor would walk it ahead of the clock. (A stale push could still
    /// share the stamp; `grid::decide` skips pushes to a pilot awaiting its
    /// bind, so the park is in fact empty.)
    pub fn next_setstate_stamp(&mut self, to: i64) -> Option<u32> {
        let tick = crate::destiny::tick();
        if let Some(&at) = self.setstate_at.get(&to)
            && at > tick
        {
            return Some(at);
        }
        let want = tick.max(self.last_destiny_stamp.saturating_add(1));
        let stamp = want.min(tick.saturating_add(2).max(self.last_destiny_stamp));
        self.last_destiny_stamp = self.last_destiny_stamp.max(stamp);
        self.setstate_at.insert(to, stamp);
        Some(stamp)
    }

    /// A new park (a bind) owes the old one nothing: forget its pending
    /// `SetState`, or a recovery asked for within two ticks would be refused.
    pub fn forget_setstate(&mut self, to: i64) {
        self.setstate_at.remove(&to);
    }

    #[cfg(test)]
    pub(crate) fn last_destiny_stamp_for_test(&self) -> u32 {
        self.last_destiny_stamp
    }

    /// Everyone in space.
    pub fn roster(&self) -> &crate::space_state::Roster {
        &self.roster
    }

    /// The same, mutably: `crate::destiny` seeds it on the ballpark bind and
    /// `service/space.rs` writes a motion into it as commands arrive.
    pub fn roster_mut(&mut self) -> &mut crate::space_state::Roster {
        &mut self.roster
    }

    /// Every lock in the world.
    pub fn locks(&self) -> &crate::targeting::Locks {
        &self.locks
    }

    /// The same, mutably: `crate::targeting` is the only caller, and its
    /// free functions are where the pushes that go with a lock change live.
    pub fn locks_mut(&mut self) -> &mut crate::targeting::Locks {
        &mut self.locks
    }

    /// Module state. `crate::modules` is the only writer.
    pub fn modules(&self) -> &crate::modules::Modules {
        &self.modules
    }

    pub fn modules_mut(&mut self) -> &mut crate::modules::Modules {
        &mut self.modules
    }

    /// Ship health and wrecks. `crate::combat` is the only writer.
    pub fn combat(&self) -> &crate::combat::Combat {
        &self.combat
    }

    pub fn combat_mut(&mut self) -> &mut crate::combat::Combat {
        &mut self.combat
    }

    /// Every live NPC. `crate::npc` and `crate::combat::kill` are the
    /// writers.
    pub fn npcs(&self) -> &crate::npc::Npcs {
        &self.npcs
    }

    pub fn npcs_mut(&mut self) -> &mut crate::npc::Npcs {
        &mut self.npcs
    }

    /// Probes and scan results. `crate::scan` is the only writer.
    pub fn scan(&self) -> &crate::scan::Probes {
        &self.scan
    }

    pub fn scan_mut(&mut self) -> &mut crate::scan::Probes {
        &mut self.scan
    }

    /// An NPC's standing toward a character, 0.0 when there is none. For
    /// `crate::mission`'s rewards; `crate::standings` owns the type.
    pub(crate) fn npc_standing(&self, character_id: i64, from_id: i64) -> f64 {
        self.standings.get(character_id, from_id)
    }

    /// Set an NPC's standing toward a character and push both standing
    /// events, through `crate::standings`'s one setter. Returns the
    /// standing it replaced.
    pub(crate) fn set_npc_standing(
        &mut self,
        character_id: i64,
        from_id: i64,
        standing: f64,
    ) -> f64 {
        self.dirty = true;
        crate::standings::set_standing_and_push(self, character_id, from_id, standing)
    }

    /// Stargate jumps in flight. `crate::jump` is the only writer.
    pub fn jumps(&self) -> &crate::jump::Jumps {
        &self.jumps
    }

    pub fn jumps_mut(&mut self) -> &mut crate::jump::Jumps {
        &mut self.jumps
    }

    /// **Who flies a roster entry**: a character, or an NPC.
    ///
    /// The roster is keyed by character id and an NPC sits in it under its
    /// own ball id (`docs/lanes/npc.md` §5), so anything that turns a
    /// presence into a hull, an owner or a corporation asks here and never
    /// `character(p.character_id)` -- which answers `None` for an NPC and
    /// used to fall back to type 0 in silence: a frozen ball, the fallback
    /// hull's HP, no wreck.
    pub fn pilot(&self, roster_key: i64) -> Option<crate::npc::Pilot> {
        // **And a drone** (`crate::drones::pilot_of`): in the roster under
        // its item id, flown by nobody, owned by a character.
        crate::npc::pilot_of(self, roster_key)
            .or_else(|| crate::drones::pilot_of(self, roster_key))
            // A fighter squadron (`crate::fighters::pilot_of`), likewise.
            .or_else(|| crate::fighters::pilot_of(self, roster_key))
            // An abandoned ship (`crate::pilotless::pilot_of`), likewise.
            .or_else(|| crate::pilotless::pilot_of(self, roster_key))
    }

    /// Ask for a session change for `character_id`, sent after the current
    /// call's answer. Call it only once the mutation has succeeded: see
    /// [`Self::pending_session_changes`].
    pub fn queue_session_change(&mut self, character_id: i64, change: Vec<(PyValue, PyValue)>) {
        // MAP-PERSONAL: a move into another system is a visit.
        crate::visits::on_session_change(self, character_id, &change);
        self.pending_session_changes.push((character_id, change));
    }

    /// Everything [`Self::queue_session_change`] queued. `dispatch.rs` only.
    pub fn take_session_changes(&mut self) -> Vec<(i64, Vec<(PyValue, PyValue)>)> {
        std::mem::take(&mut self.pending_session_changes)
    }

    /// Bring one character's stored motion up to the present.
    ///
    /// **Integrated on read, not on a timer**, and that is the whole design:
    /// there is no background task advancing the world, nothing to keep
    /// alive, and no difference between a world somebody is watching and one
    /// nobody is. A position is computed exactly when it is about to be used
    /// -- which, because `Motion::advanced` is closed form, costs the same
    /// whether the ship was commanded a second ago or an hour ago.
    ///
    /// Call this **before** reading a position for any purpose and before
    /// applying a new command, because `Motion::after` turns a heading into a
    /// point relative to the position it finds. Without it, every command
    /// compounded the staleness of the one before.
    ///
    /// Silent on a character with no hull and on one not in space: neither is
    /// an error, and both are ordinary during login.
    pub fn advance_character(&mut self, character_id: i64) {
        let Some(system) = self.roster.get(character_id).map(|p| p.solar_system_id) else {
            return;
        };
        // **A pair is stepped together** (FX9): a ship tethered to another
        // ship moves on the tick grid with it, so advancing either one means
        // advancing the system (`space_state::Roster::advance`).
        if self.roster.has_pair(system) {
            self.advance_system(system);
            return;
        }
        self.advance_unpaired(character_id);
    }

    /// [`Self::advance_character`] for a caller that has already checked the
    /// system has no tethered pair (`Roster::has_pair`), so the check is not
    /// repeated per pilot.
    pub fn advance_unpaired(&mut self, character_id: i64) {
        if let Some(m) = self.mover(character_id) {
            self.roster.advance(&[m], crate::space_state::now_seconds());
        }
    }

    /// The hull a character's ship **flies**: the **fitted** top speed,
    /// agility and mass when `crate::dogma` has a sheet for it (a running
    /// afterburner, Navigation, an overdrive); the hull table's otherwise.
    /// What the integrator (`mover`) and the warp's activation prediction
    /// (`service::space::warp_to_point`) must both read, because the pilot's
    /// own ball (`destiny::pilot_ball`) carries these numbers.
    pub fn flight_hull(&self, character_id: i64) -> Option<crate::destiny::hull::Hull> {
        let ship_type_id = self.pilot(character_id).map(|p| p.ship_type_id)?;
        let p = self.roster.get(character_id)?;
        let fitted = crate::dogma::hull(self, p.ship_id);
        let mut hull = *fitted
            .as_ref()
            .or_else(|| crate::destiny::hull::find_in(self.content(), ship_type_id))
            .unwrap_or(&crate::destiny::hull::FALLBACK);
        if fitted.is_none() {
            // A ship with no sheet (an NPC) flies webbed too (K-EWAR);
            // `dogma::hull` has multiplied it in for one that has.
            hull.max_velocity =
                (f64::from(hull.max_velocity) * crate::ewar::speed_factor(self, p.ship_id)) as f32;
        }
        Some(hull)
    }

    /// What `Roster::advance` needs to carry one ship forward: its hull and
    /// what it is tethered to. `None` for a character with no hull, which is
    /// ordinary during login.
    fn mover(&self, character_id: i64) -> Option<crate::space_state::Mover> {
        self.pilot(character_id)?;
        let p = self.roster.get(character_id)?;
        // **A ship at rest needs no hull** (PERF-STAGES): stopped, not moving
        // and tethered to nothing, every integrator step is the identity
        // (`Motion::advanced_at`: the commanded velocity of STOP is zero, so
        // neither the drag nor the arrival test can move it), so the fitted
        // numbers below -- a dogma sheet per ship, the cost of a system
        // advance -- would change nothing. The bots mining a belt sit here.
        if p.motion.is_at_rest() {
            return Some(crate::space_state::Mover {
                character_id,
                hull: crate::destiny::hull::FALLBACK,
                tether: None,
            });
        }
        let hull = self.flight_hull(character_id)?;
        // **Never a tether to our own ball**: `CmdOrbit` on your own ship is
        // a command the client can send, and snapping to it moved the ship
        // an orbit radius on every read (test in `space_state`).
        //
        // **`ball_target`, not `ball_position`: a range is a gap between
        // hulls.** Carbon computes the centre separation for both follow
        // and orbit as `mFollowRange + mRadius + other->mRadius`
        // (`Ballpark.cpp:1086` and `:1264`) `[V]` (§54.4).
        let tether = p
            .motion
            .tether()
            .filter(|(ball, _)| *ball != p.ship_id)
            .and_then(|(ball, range)| {
                self.ball_target(ball).map(|t| crate::space_state::Tether {
                    ball_id: ball,
                    range: crate::orbit::centre_distance(range, f64::from(hull.radius), t.radius),
                    at: t.position,
                })
            });
        Some(crate::space_state::Mover {
            character_id,
            hull,
            tether,
        })
    }

    /// Where a ball is, for the few things that have to ask.
    pub fn ball_position(&self, ball_id: i64) -> Option<[f64; 3]> {
        self.ball_target(ball_id).map(|t| t.position)
    }

    /// The system's celestials, as loaded from the extract.
    pub fn celestials(&self) -> &crate::celestial::Catalogue {
        &self.config.celestials
    }

    /// The rocks in the belts, as laid out from `--belt-rocks`.
    pub fn rocks(&self) -> &crate::asteroid::Field {
        &self.config.rocks
    }

    /// The rocks, mutably. `crate::mining` is the only writer: a cycle takes
    /// ore out, and a mined-out rock leaves the field.
    pub fn rocks_mut(&mut self) -> &mut crate::asteroid::Field {
        &mut self.config.rocks
    }

    /// Where a ball is **and how big it is**.
    ///
    /// Ships from the roster, every station ([`Self::station_position`]:
    /// the catalogue's, else the origin), and the rest below.
    ///
    /// **The radius is not decoration.** A warp aimed at an object's centre
    /// flies the ship *into the model*: a player warped to the station and
    /// landed 16 km inside it, because the station's own radius is 15,733 m
    /// and we sent a destination of `(0, 0, 0)` with no offset. Anything
    /// choosing a destination needs to know how far "the edge" is, so the
    /// two travel together.
    pub fn ball_target(&self, ball_id: i64) -> Option<Target> {
        if let Some(p) = self.roster.by_ship(ball_id) {
            return Some(self.ship_target(p));
        }
        // **An Upwell structure comes before "a character's station"** (U4):
        // a pilot docked in one has `station_id == structure`, and the branch
        // below would answer with the origin.
        if let Some(t) = crate::structure_space::target(self, ball_id) {
            return Some(t);
        }
        let station = self
            .config
            .celestials
            .get(ball_id)
            .is_some_and(|c| c.kind == crate::celestial::Kind::Station);
        if station
            || self
                .config
                .characters
                .iter()
                .any(|c| c.station_id == Some(ball_id))
        {
            // The type's own radius (FX7); a station known only as a
            // character's home, with no catalogue row, keeps the constant.
            let radius = self
                .config
                .celestials
                .get(ball_id)
                .map_or(crate::destiny::STATION_RADIUS, |c| {
                    crate::destiny::station_radius(self.content(), c.type_id)
                });
            return Some(Target {
                position: self.station_position(ball_id),
                radius: f64::from(radius),
                // A station is category 3, not categoryCelestial, so the
                // client's own gate gives it no warp-in point however large
                // it is. The radius floor at the warp site keeps the ship
                // out of the model instead.
                warp_in: None,
            });
        }
        // **A rock in a belt.** Without this, approach, orbit and warp to a
        // rock resolve nothing and the ship cruises off (`docs/lanes/
        // asteroids.md` §4).
        if let Some(rock) = self.config.rocks.get(ball_id) {
            return Some(rock.target());
        }
        // **A site object** (W14 CP28a): a beacon or structure of an
        // instantiated room, so approach and orbit resolve it.
        if let Some(t) = crate::site::object_target(self, 0, ball_id, None) {
            return Some(t);
        }
        // **A Sovereignty Hub** (SOC-D2): approach, orbit, warp to.
        if let Some(t) = crate::sov_hub::target(self, ball_id) {
            return Some(t);
        }
        // **The ESS or its beacon** (ESS-2): approach, orbit, warp to.
        if let Some(t) = crate::ess::target(self, ball_id) {
            return Some(t);
        }
        // **A cynosural field** (CAP-D): approach, orbit.
        if let Some(t) = crate::cyno::target(self, ball_id) {
            return Some(t);
        }
        // **A warp disrupt probe** (BUBBLES-SB): approach, orbit.
        if let Some(t) = crate::bubble::target(self, ball_id) {
            return Some(t);
        }
        // **A command node** (SOC-D3b): warp to it, approach, orbit.
        if let Some(t) = crate::command_nodes::target(self, ball_id) {
            return Some(t);
        }
        // **A customs office** (`docs/lanes/planet.md` §11): one per planet,
        // derived from the planet, so it is never in the item store.
        if let Some(office) = crate::planet::customs_office(
            &self.config.celestials,
            crate::planet::statics_in(&self.content),
            ball_id,
        ) {
            return Some(office.target());
        }
        // **A wreck** (P2.4). Without this, "Approach" on a wreck -- the
        // client's own way into loot range (`menusvc.OpenCargo:3584`
        // approaches when out of range) -- resolved nothing: the client
        // drew its ship closing in while the server kept it where it was,
        // and the take was then refused `NotCloseEnoughToLoot`, measured
        // from the stale spot. Found by the P8c escort bot
        // (`docs/lanes/bots.md`), whose own park agreed with the client's.
        if let Some(wr) = self.combat.wreck(ball_id) {
            return Some(Target {
                position: wr.position,
                radius: f64::from(crate::destiny::WRECK_RADIUS),
                warp_in: None,
            });
        }
        // **And the system's furniture.** Only the kinds we actually put in
        // the park: resolving one we never sent would answer a warp to a
        // ball the client has no bracket for, which is a warp to a place the
        // player could not have asked about.
        self.config
            .celestials
            .get(ball_id)
            .filter(|c| self.config.celestials.is_served(c.id))
            .map(|c| Target {
                position: c.position,
                radius: c.radius,
                warp_in: crate::warp_in::warp_in_point(c),
            })
    }

    /// The same for a **character** rather than a ball.
    ///
    /// `CmdWarpToStuff`'s `'char'` subject carries a character id, not a ship
    /// id -- a fleet member's "Warp to" sends `('char', 90000001)` `[V]`,
    /// observed on the wire -- and the roster is keyed by character, so this
    /// is the shorter path as well as the correct one.
    pub fn character_target(&self, character_id: i64) -> Option<Target> {
        self.roster.get(character_id).map(|p| self.ship_target(p))
    }

    fn ship_target(&self, p: &crate::space_state::Presence) -> Target {
        let type_id = self
            .pilot(p.character_id)
            .map_or(0, |pilot| pilot.ship_type_id);
        let hull = crate::destiny::hull::of_in(self.content(), type_id);
        Target {
            position: p.motion.position,
            radius: f64::from(hull.radius),
            // A ship is not a celestial; warping to one aims at the ship.
            warp_in: None,
        }
    }

    /// This hull's scan resolution, as the client was told it.
    ///
    /// The only input to lock *speed* that comes from our side; the target's
    /// signature radius the client reads out of its own FSD.
    pub(crate) fn scan_resolution_of(&self, ship_type_id: i64) -> f64 {
        let h = crate::destiny::hull::of_in(self.content(), ship_type_id);
        targeting_of(h).scan_resolution
    }

    /// This hull's targeting range, as the client was told it (attribute 76).
    ///
    /// Beyond it a lock cannot be held, which is what
    /// [`crate::targeting::sweep_out_of_range`] enforces. Read through
    /// `targeting_of` and not off the hull, so that a hull with no targeting
    /// numbers gets the pod's substitute here exactly as it does everywhere
    /// else -- a capsule whose table row is zeroed must not be given an
    /// infinite range by accident.
    pub(crate) fn max_target_range_of(&self, ship_type_id: i64) -> f64 {
        let h = crate::destiny::hull::of_in(self.content(), ship_type_id);
        targeting_of(h).max_target_range
    }

    /// The lock range of this **ship** as dogma serves it (fitted range
    /// modules, the system's effect beacon), the type's own number when the
    /// ship has no sheet: what attribute 76 says on the client, so what the
    /// range sweep and the NPCs' view must enforce.
    pub(crate) fn max_target_range_of_ship(&self, ship_id: i64, ship_type_id: i64) -> f64 {
        match crate::dogma::hull(self, ship_id) {
            Some(h) => targeting_of(&h).max_target_range,
            None => self.max_target_range_of(ship_type_id),
        }
    }

    /// The same for everyone in one solar system, so that a state built from
    /// the roster is true of every ball in it at the same instant.
    ///
    /// One instant matters: a park whose balls were integrated to different
    /// moments is a park where two ships that left together are no longer
    /// abreast, which is visible and has no innocent explanation.
    pub fn advance_system(&mut self, solar_system_id: i64) {
        self.advance_system_with(solar_system_id, crate::par::threads());
    }

    /// [`Self::advance_system`] with an explicit thread count (tests, benches).
    ///
    /// The movers are built in parallel (PA-8b): `mover` only reads the
    /// registry (its one write is the derived dogma sheet cache), `par_map`
    /// keeps the order, and `Roster::advance` stays serial, so the result is
    /// the serial one at any thread count.
    pub(crate) fn advance_system_with(&mut self, solar_system_id: i64, threads: usize) {
        // One call for the whole system, so every ship is carried to the
        // same instant, and a ship tethered to another ship is stepped with
        // it on the client's tick grid rather than in character order (FX9,
        // `space_state::Roster::advance`).
        let here = self.roster.in_system(solar_system_id);
        let serial = |reg: &Self| -> Vec<crate::space_state::Mover> {
            here.iter()
                .filter_map(|p| reg.mover(p.character_id))
                .collect()
        };
        let movers = if here.len() >= MOVERS_PAR_MIN && threads > 1 {
            match crate::par::par_map_with(threads, &here, MOVERS_PAR_MIN, |p| {
                self.mover(p.character_id)
            }) {
                Ok(v) => v.into_iter().flatten().collect(),
                Err(e) => {
                    crate::errln!("[par] advance_system movers: {e}; serial");
                    serial(self)
                }
            }
        } else {
            serial(self)
        };
        self.roster
            .advance(&movers, crate::space_state::now_seconds());
    }

    /// The pre-PA-8b body, kept as the oracle of the golden test.
    #[cfg(test)]
    pub(crate) fn advance_system_legacy(&mut self, solar_system_id: i64) {
        let movers: Vec<crate::space_state::Mover> = self
            .roster
            .in_system(solar_system_id)
            .iter()
            .filter_map(|p| self.mover(p.character_id))
            .collect();
        self.roster
            .advance(&movers, crate::space_state::now_seconds());
    }

    /// Everything that has to outlive the process.
    pub fn to_snapshot(&self) -> crate::store::Snapshot {
        crate::store::Snapshot {
            characters: self
                .config
                .characters
                .iter()
                .map(|c| crate::store::CharacterRecord {
                    character_id: c.character_id,
                    ship_id: c.ship_id,
                    owner_user_id: c.owner_user_id,
                    name: c.name.clone(),
                    station: crate::store::StationRecord::of(c),
                    identity: Some(crate::store::IdentityRecord::of(c)),
                })
                .collect(),
            balances: self
                .config
                .characters
                .iter()
                .map(|c| (c.character_id, c.balance))
                .collect(),
            items: self.store.items().to_vec(),
            next_item_id: self.store.next_id(),
            // E4: none once the ledger log holds them.
            journal: self.journal.snapshot_rows(),
            next_transaction_id: self.journal.next_id(),
            skills: self
                .skills
                .iter()
                .map(|(id, s)| (*id, s.entries().cloned().collect()))
                .collect(),
            logoff_dates: self
                .config
                .characters
                .iter()
                .filter(|c| c.logoff_date > 0)
                .map(|c| (c.character_id, c.logoff_date))
                .collect(),
            character_description: self
                .character_description
                .iter()
                .map(|(id, text)| (*id, text.clone()))
                .collect(),
            standings: self.standings.entries(),
            jobs: self.jobs.all().to_vec(),
            next_job_id: self.jobs.next_id(),
            blueprints: self.jobs.blueprints().clone(),
            bookmarks: self.bookmarks.clone(),
            contacts: self.contacts.clone(),
            clones: self.clones.clone(),
            skill_queues: self.skill_queues.clone(),
            corporations: self.corporations.clone(),
            market: self.market.clone(),
            planets: self.planet.saved(),
            contracts: self.contracts.clone(),
            deltas: self.deltas.clone(),
            world_minted: false,
            tainted: self.tainted,
        }
    }

    /// Replace the durable half of the world with a stored one.
    ///
    /// **Applied over the seed rather than instead of it.** `Registry::new`
    /// has already built the station, the capsule and the starter skills from
    /// `SandboxConfig`, and those are the parts a restart should rebuild
    /// identically. What a snapshot carries is what the *player* changed, so
    /// anything it holds replaces its seeded counterpart and anything it does
    /// not is left as seeded. That is why a snapshot with no skills for a
    /// character leaves the starter set in place instead of emptying it --
    /// loading a world should never be destructive.
    pub fn apply_snapshot(&mut self, snap: crate::store::Snapshot) {
        // A world record drawn at this load is not on disk yet.
        let minted = snap.world_minted;
        // Replaced whole: the seed has no deltas. A file load always carries
        // a world record (`store::settle`); a hand-built snapshot may not,
        // and then the current one stays, so `world()` is never `None`.
        let world = *self.world();
        self.deltas = snap.deltas;
        if self.deltas.world().is_none() {
            self.deltas.set_world(world);
        }
        // **The roster first**, before the items, skills and balances below
        // look characters up (IP27-b, account-spec §3.3). A seeded character
        // takes the record's owner and keeps the seed's name: the flags name
        // it, as before accounts [A]. A record with no seeded counterpart is
        // seeded here -- its capsule and starter skills are then replaced by
        // the snapshot's items and skills like everyone's. A format-1 file
        // has no records: the roster is the seed, every character unowned.
        //
        // **The saved station is seated last** (FX3, `docs/port-tracker.md`
        // §72.1), after `restore_capsules`: it is where the pilot was when
        // the world was saved -- the station they last docked at (FX1) or
        // were revived in -- and the clone record's station is older. A
        // format-2 record has none, and the seed's station stands.
        let mut seats: Vec<(i64, crate::store::StationRecord)> = Vec::new();
        for r in snap.characters {
            if let Some(st) = r.station {
                seats.push((r.character_id, st));
            }
            // **The saved identity wins over the seed's** (P-START-3), for
            // a seeded character as for a new one, before
            // `corp::sync_characters` below reconciles player corps. A record
            // from before format 4 has none, and the seed's stands.
            if let Some(c) = self.character_mut(r.character_id) {
                c.owner_user_id = r.owner_user_id;
                if let Some(id) = r.identity {
                    id.apply(c);
                }
                continue;
            }
            let clash = self
                .config
                .characters
                .iter()
                .find(|c| c.ship_id == r.ship_id)
                .map(|c| c.character_id);
            if let Some(other) = clash {
                // Never a panic on a file: the character is left out, loudly.
                crate::errln!(
                    "[world] character {} not loaded: ship_id {} is character {other}'s",
                    r.character_id,
                    r.ship_id
                );
                continue;
            }
            let mut c = Character {
                character_id: r.character_id,
                ship_id: r.ship_id,
                owner_user_id: r.owner_user_id,
                name: r.name,
                ..Character::default()
            };
            if let Some(st) = r.station {
                st.seat(&mut c);
            }
            if let Some(id) = r.identity {
                id.apply(&mut c);
            }
            self.seed_character(c);
        }
        crate::start::apply_births(self);
        // After the roster: a bot-marked account's characters are bots.
        self.mark_bot_characters();
        for (id, balance) in snap.balances {
            if let Some(c) = self
                .config
                .characters
                .iter_mut()
                .find(|c| c.character_id == id)
            {
                c.balance = balance;
            }
        }
        if !snap.items.is_empty() {
            self.store = crate::item::Store::restore(snap.items, snap.next_item_id);
        }
        if !snap.journal.is_empty() {
            self.journal = crate::ledger::Journal::restore(snap.journal, snap.next_transaction_id);
        } else {
            // E4: the rows are in the ledger log; the allocator (and its
            // C7 §3.4 leap) is still in the snapshot.
            self.journal.raise_next_id(snap.next_transaction_id);
        }
        // **Restored even when the list is empty**, unlike the items and the
        // journal above, because `next_job_id` has to survive a world whose
        // every job was delivered and pruned. The same reasoning as
        // `Store::restore`'s allocator, and `Jobs::restore` takes the `max`
        // itself so a snapshot whose allocator trails its own rows cannot
        // hand out a live id.
        // Replaced whole: the seed has no bookmarks, so there is nothing of
        // the seed's to keep.
        self.bookmarks = snap.bookmarks;
        // Replaced whole, for the same reason: the seed has no contacts.
        self.contacts = snap.contacts;
        // Replaced whole, for the same reason: the seed has no clones.
        self.clones = snap.clones;
        // Replaced whole, for the same reason: the seed trains nothing.
        self.skill_queues = snap.skill_queues;
        // Replaced whole, for the same reason: the seed has no player corp.
        // Then made true of the characters, whose `corporation_id` is the
        // field every other reader uses.
        self.corporations = snap.corporations;
        corp::sync_characters(self);
        // Replaced whole, like the bookmarks: the seed has no orders.
        self.market = snap.market;
        // Replaced whole, for the same reason: the seed has no colonies. The
        // static table is kept -- it is regenerable and not in the file.
        self.planet.restore(snap.planets);
        // Replaced whole, like the market: the seed has no contracts.
        self.contracts = snap.contracts;
        if !snap.jobs.is_empty() || snap.next_job_id > 0 {
            self.jobs = crate::industry::Jobs::restore(snap.jobs, snap.next_job_id);
        }
        // After the jobs, because `Jobs::restore` builds a fresh table.
        // Replaced whole, like the bookmarks: the seed has no researched
        // blueprints and no copies.
        self.jobs.restore_blueprints(snap.blueprints);
        for (id, entries) in snap.skills {
            if !entries.is_empty() {
                self.skills
                    .insert(id, crate::skill::Skills::restore(entries));
            }
        }
        for (id, when) in snap.logoff_dates {
            if when > 0
                && let Some(c) = self
                    .config
                    .characters
                    .iter_mut()
                    .find(|c| c.character_id == id)
            {
                c.logoff_date = when;
            }
        }
        // Per character, and non-destructive like the rest: a stored bio
        // replaces that character's, and a character the snapshot says
        // nothing about keeps whatever it has.
        for (id, text) in snap.character_description {
            if !text.is_empty() {
                self.character_description.insert(id, text);
            }
        }
        // Replaced only when the file has some, like the items: a world
        // file from before standings existed leaves them as seeded (none).
        if !snap.standings.is_empty() {
            self.standings = crate::standings::Standings::restore(snap.standings);
        }
        // `Character.ship_id` is not in the snapshot: the active ship is
        // derived from where the capsule is (`docs/lanes/fitting.md` §7).
        // A revive mints a new capsule; seat it first, so the derivation
        // below starts from an item that exists (`character::Clones`).
        character::restore_capsules(self);
        for (character_id, st) in seats {
            if let Some(c) = self.character_mut(character_id) {
                st.seat(c);
            }
            // Known to `dock_target` again, as a seeded station is: a
            // restart need not seed anyone there.
            if let Some(c) = self.character(character_id)
                && let Some(station) = character::StationRef::of(c)
            {
                self.stations.entry(station.station_id).or_insert(station);
            }
        }
        crate::fitting::restore_active_ships(self);
        crate::weapon_bank::restore(self);
        crate::modules::restore_offline(self);
        crate::logoff_spot::restore(self);
        // Items a trade had staged when the world was saved go home (GAP-TRADE).
        let recovered = crate::trade::recover(self);
        // A load is not a change the disk needs told about, unless it minted
        // the world record (`store::settle`) or returned staged items.
        self.dirty = minted || recovered;
    }

    /// Answer `DoAction` with the station agent, apply whatever it changed,
    /// and queue the push.
    ///
    /// **The push is not optional.** `OnAgentMissionChange` is the only
    /// journal-cache invalidation short of a session reset, so an accepted
    /// mission without it is one the journal never shows -- the difference
    /// between "works" and "works after a relog". The conversation, the
    /// cargo, the pocket and the reward are `crate::mission::converse`'s.
    pub fn agent_do_action(&mut self, actor: Option<i64>, action: Option<i64>) -> PyValue {
        let (answer, change) = crate::mission::converse(self, actor, action);
        // Last: the journal reads back what the conversation made true.
        crate::mission::tell_journal(self, change);
        answer
    }

    /// This character's place in the conversation. `Offered` for a character
    /// who has never spoken to the agent, and for no actor at all. Read by
    /// the tests; the arms go through `crate::mission`.
    #[cfg(test)]
    fn mission_state(&self, actor: Option<i64>) -> crate::mission::State {
        crate::mission::state(self, actor)
    }

    /// Mark durable state as changed, for a subsystem whose mutation does
    /// not go through one of the methods that already do (`apply_items`,
    /// `charge`): a colony edit is the first.
    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Whether anything durable changed, clearing the flag.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub fn store(&self) -> &crate::item::Store {
        &self.store
    }

    /// `Store::spawn`, for `/createitem` in `crate::slash` (seam S1).
    ///
    /// A door for that one grant rather than a `store_mut`: the caller marks
    /// the world dirty and sends both pushes itself, as it did when it was
    /// an arm in this file, and `apply_items` stays the way everything else
    /// mutates the store.
    pub(crate) fn spawn_items(
        &mut self,
        type_id_or_name: &str,
        quantity: i64,
        owner_id: i64,
        location_id: i64,
    ) -> Result<(Vec<i64>, crate::item::ItemType), crate::item::SpawnError> {
        self.store.spawn_in(
            &self.content,
            type_id_or_name,
            quantity,
            owner_id,
            location_id,
        )
    }

    /// Run an item plan, push what changed, and mark the world dirty.
    ///
    /// **The one place a domain module may mutate the store**, which is what
    /// makes the push and the dirty flag impossible to forget. `Store::apply`
    /// had no caller anywhere in the tree -- Round 0.5 landed the two-phase
    /// API and nothing used it -- and `Registry::store` is read-only, so a
    /// domain module could resolve a plan and not run it. Reprocessing is its
    /// first consumer.
    ///
    /// The notifications are built from the returned `Change`s and not from a
    /// second guess at the diff, which is the entire reason `Change` exists;
    /// see `item::Change`.
    ///
    /// On `Err` **nothing was applied** -- that is `apply`'s phase-one
    /// guarantee -- so there is nothing to push and nothing to undo.
    pub fn apply_items(
        &mut self,
        edits: &[crate::item::Edit],
    ) -> Result<Vec<crate::item::Change>, crate::item::EditError> {
        let goods = crate::econ_ledger::goods_of(&self.store, &self.content, edits);
        let changes = self.store.apply_in(&self.content, edits)?;
        crate::econ_ledger::add_goods(self, &goods);
        if changes.is_empty() {
            return Ok(changes);
        }
        self.dirty = true;
        let mut locations: Vec<i64> = Vec::new();
        let mut everyone: Vec<i64> = Vec::new();
        let mut rows = Vec::new();
        let mut gone: Vec<(crate::item::Item, Vec<i64>)> = Vec::new();
        for c in &changes {
            // **Addressed to the owners, before and after.** An unaddressed
            // push means "the caller", which is only right when the caller
            // owns everything that moved: a market fill moves the seller's
            // goods inside the buyer's call, an expiry moves anybody's, and a
            // death on the world tick has no caller at all (found by the
            // P2.2c lane). Only characters count -- escrow and the system own
            // things too, and have no session to tell.
            let mut owners: Vec<i64> = c.slots().iter().map(|s| s.owner_id).collect();
            // A destroyed item is gone from the store, so there is no row to
            // send for it; the snapshot in the change is told below as a move
            // to the junkyard (`tell_destroyed`).
            let item = self.store.get(c.item_id()).cloned();
            if let Some(i) = &item {
                owners.push(i.owner_id);
            }
            owners.retain(|id| self.character(*id).is_some());
            owners.sort_unstable();
            owners.dedup();
            for id in &owners {
                if !everyone.contains(id) {
                    everyone.push(*id);
                }
            }
            // **A loaded charge is never an item row** to the client: it is a
            // `(shipID, flagID, typeID)` sublocation, and `crate::modules`
            // tells godma and the fitting window about it in that shape
            // (`crate::item::is_loaded_charge`, P2.3). An `OnItemChange` with
            // its store id would put a second, int-keyed charge on the HUD
            // button.
            if let crate::item::Change::Destroyed {
                item: gone_item, ..
            } = c
                && !crate::item::is_loaded_charge(gone_item)
                && !owners.is_empty()
            {
                gone.push(((**gone_item).clone(), owners.clone()));
            }
            if let Some(i) = item.filter(|i| !crate::item::is_loaded_charge(i)) {
                rows.push((i, c.previous_columns(), owners));
            }
            for slot in c.slots() {
                if !locations.contains(&slot.location_id) {
                    locations.push(slot.location_id);
                }
            }
        }
        // **A destroyed item leaves the client's lists by its own row.** The
        // location invalidation below does not reach a hangar list: that is
        // keyed `(const.containerHangar, None)`, not the station id, and the
        // client serves its cached list for minutes (fci_reprocess, fci_inject,
        // 2026-10-31). A row moved to the junkyard is what `_ProcessItemChange`
        // removes.
        for (item, owners) in gone {
            for owner in owners {
                self.tell_destroyed(item.clone(), owner);
            }
        }
        // `None` -- the caller -- only when no character owns any of it,
        // which is the behaviour this function always had.
        let to = |owners: Vec<i64>| (!owners.is_empty()).then_some(owners);
        for (item, change, owners) in rows {
            let row = self.item_row_value(&item);
            // For the item's current owner, the receiver; for anyone else on
            // the push `is_new_item` is false and the client only marks seen.
            let location = crate::notify::badging_location(
                item.flag_id,
                item.category_id,
                item.location_id,
                self.docked_station(item.owner_id),
            );
            self.push(crate::notify::Addressed {
                to: to(owners),
                inner: Box::new(crate::notify::ItemChange {
                    row,
                    change,
                    location,
                }),
            });
        }
        industry::refresh_monitors(self, &everyone);
        if !locations.is_empty() {
            self.push(crate::notify::Addressed {
                to: to(everyone),
                inner: Box::new(crate::notify::InvalidateLocations {
                    location_ids: locations,
                }),
            });
        }
        Ok(changes)
    }

    /// Tell `owner`'s client that `item` (a snapshot taken **before** it was
    /// destroyed) left its place: an `OnItemChange` with the row moved to the
    /// junkyard (`const.locationJunkyard`, `appConst.py`), as the real server sends one.
    ///
    /// [`Self::apply_items`] calls this for every destroyed or merged-away item
    /// (a destroyed item has no row, and the location invalidation alone misses
    /// a hangar list and the client's dogma, which unfits a module only on an
    /// item change that takes it off the ship), so callers going through it need
    /// not. Call it directly only after [`Self::apply_items_quiet`].
    pub fn tell_destroyed(&mut self, mut item: crate::item::Item, owner: i64) {
        const LOCATION_JUNKYARD: i64 = 10;
        let change = vec![
            (crate::item::IX_LOCATION_ID, item.location_id),
            (crate::item::IX_FLAG, item.flag_id),
        ];
        item.location_id = LOCATION_JUNKYARD;
        item.flag_id = 0;
        let row = self.item_row_value(&item);
        self.push(crate::notify::Addressed {
            to: Some(vec![owner]),
            inner: Box::new(crate::notify::ItemChange {
                row,
                change,
                location: PyValue::None,
            }),
        });
    }

    /// [`Self::apply_items`] without telling anyone: the plan is applied and
    /// the world marked dirty, and **no** `OnItemChange` or location
    /// invalidation is pushed.
    ///
    /// For a change whose news the caller sends in another shape -- a round
    /// fired out of a loaded gun is a sublocation's attribute 805 moving
    /// (`crate::modules::ChargeQuantity`), not an inventory event, and an
    /// invalidation of the ship's inventory every cycle of every gun would be
    /// a cache flush the client never asked for.
    pub fn apply_items_quiet(
        &mut self,
        edits: &[crate::item::Edit],
    ) -> Result<Vec<crate::item::Change>, crate::item::EditError> {
        let goods = crate::econ_ledger::goods_of(&self.store, &self.content, edits);
        let changes = self.store.apply_in(&self.content, edits)?;
        crate::econ_ledger::add_goods(self, &goods);
        if !changes.is_empty() {
            self.dirty = true;
        }
        Ok(changes)
    }

    /// This character's trained skills, for a domain module that needs a
    /// level rather than the whole `PyValue` dict.
    ///
    /// `skills_of` is private and `skill_dict` returns marshal, so there was
    /// no way to ask "what level is this skill" from outside this file --
    /// which every yield, bonus and requirement check needs.
    pub fn skills_for(&self, actor: Option<i64>) -> Option<&crate::skill::Skills> {
        actor.and_then(|id| self.skills_of(id))
    }

    pub fn journal(&self) -> &crate::ledger::Journal {
        &self.journal
    }

    /// The journal, mutably: for `/giveisk` in `crate::slash`, which records
    /// its own GM cash transfer and lives outside this module (seam S1).
    pub(crate) fn journal_mut(&mut self) -> &mut crate::ledger::Journal {
        &mut self.journal
    }

    /// This character's skills.
    ///
    /// `skills` is already keyed by character id, so the old
    /// `skills_of_current` went through `characters.first()` only to find out
    /// *which* id to look up. With the actor in hand the map answers
    /// directly, and an unknown actor gets `None` rather than someone
    /// else's skills.
    fn skills_of(&self, actor: i64) -> Option<&crate::skill::Skills> {
        self.skills.get(&actor)
    }

    /// `{typeID: CharacterSkillEntry}` for the actor.
    pub fn skill_dict(&self, actor: Option<i64>) -> PyValue {
        match actor.and_then(|id| self.skills_of(id)) {
            Some(s) => s.to_pyvalue(),
            None => PyValue::Dict(vec![]),
        }
    }

    /// The actor's skills, mutably.
    pub(crate) fn skills_mut(&mut self, actor: i64) -> Option<&mut crate::skill::Skills> {
        self.skills.get_mut(&actor)
    }

    /// Buy skills for the selected character, and ask the client to re-read.
    ///
    /// **The list and the push are one deliverable.** The caller is
    /// `while skill_type_ids != seen: BeNice()`, and `seen` is filled from
    /// `GetSkillIncludingLapsed` *without* `renew` -- i.e. from the client's
    /// already-populated cache -- so that loop issues no service call at all
    /// and no answer of ours, at any shape or cadence, can end it. Only a push
    /// that invalidates the cache can. Returning the honest list *without*
    /// the push would therefore be strictly worse than returning the empty
    /// list we used to: a 100%-CPU tasklet instead of a silent no-op.
    pub fn purchase_skills(&mut self, actor: Option<i64>, type_ids: &[i64]) -> Vec<i64> {
        let Some(actor) = actor else {
            return Vec::new();
        };
        let Some(skills) = self.skills.get_mut(&actor) else {
            return Vec::new();
        };
        match skills.purchase_in(&self.content, type_ids) {
            Ok(bought) => {
                self.dirty = true;
                // No debit. The client has already checked the balance
                // client-side and will not notice, and charging without a
                // price table would be inventing numbers. Stated here rather
                // than left as an omission.
                self.push(crate::skill::ForcedRefresh);
                bought
            }
            // Refused: the empty list, which is the only answer that does not
            // spin. The client's own validation should have caught anything
            // we refuse here, so this is the belt to that braces.
            Err(_) => Vec::new(),
        }
    }

    /// The character's own station (`Character::station_id`, "my station"),
    /// as an industry facility.
    ///
    /// **Not a docked gate**, and it used to claim it was: `station_id` is
    /// never cleared on undock, so this has always answered in space too
    /// (`docs/lanes/docked.md`). That is kept on purpose `[A]`: retail
    /// industry is remote -- a facility in the system you are in is
    /// `DISTANCE_PER_LEVEL[0] == 0` jumps away, installable from space with
    /// no skill -- and the industry window browses facilities undocked.
    pub fn industry_facility(&self, actor: Option<i64>) -> Option<crate::industry::Facility> {
        let c = self.character(actor?)?;
        Some(crate::industry::Facility::npc_station(
            c.station_id?,
            c.station_owner_id,
            c.station_type_id,
            c.solar_system_id?,
        ))
    }

    pub fn jobs(&self) -> &crate::industry::Jobs {
        &self.jobs
    }

    /// Record a copy's runs, ME and TE minted outside a job (HK3), saved
    /// with the world like a delivered copy's.
    pub fn set_blueprint_state(&mut self, state: crate::industry::BlueprintState) {
        self.dirty = true;
        self.jobs.set_blueprint(state);
    }

    /// The job table, mutably.
    ///
    /// **For the clock, and later for the store.** A job takes minutes to
    /// finish and a test does not get to wait, so a test rewrites `end_date`
    /// and commits the job back -- which is exactly the operation
    /// `Jobs::restore` will perform when job persistence lands, so this is
    /// the seam that work will use rather than scaffolding for it. `cfg(test)`
    /// until then, because an unused accessor is a signal worth keeping.
    #[cfg(test)]
    pub(crate) fn jobs_mut(&mut self) -> &mut crate::industry::Jobs {
        &mut self.jobs
    }

    /// Run an install plan and record the job.
    ///
    /// **The order is the whole function.** `apply_items` is all-or-nothing
    /// and can fail; the job is committed only after it returns `Ok`, so a
    /// refused plan leaves no job behind and a committed job always has its
    /// materials already consumed. The reverse order would create jobs out of
    /// nothing whenever a plan was refused.
    pub fn install_job(
        &mut self,
        install: crate::industry::Install,
    ) -> Result<i64, crate::item::EditError> {
        self.apply_items(&install.edits)?;
        self.dirty = true;
        let job = install.job;
        // `commit_install`, not `commit`: an install from a copy also spends
        // the copy's runs, and that row is `crate::industry::Blueprints`'.
        let job_id = self.jobs.commit_install(&install);
        // **And the fee is charged**, which `docs/port-tracker.md` §15.6
        // carried as open on the grounds that there was no public debit on
        // `Registry`. There is one now.
        //
        // `plan_install` has already refused the job if the balance did not
        // cover it (`industry::Refusal::AccountFunds`), and it did so in this
        // same dispatcher turn under this same world write lock -- so nothing
        // can have spent the money in between and the subtraction cannot take
        // a wallet negative. Free jobs are not journalled: a zero-ISK row in
        // the wallet is noise, not history.
        //
        // The counterparty is the station's owner, read from the character
        // rather than from the job, because `industry::Job` records the
        // facility id and not who owns it -- and the character is docked in
        // that very station, in this turn, by construction.
        let fee = crate::industry::Cost::ref_type(job.activity_id).filter(|_| job.cost > 0);
        if let Some(entry_type) = fee {
            let station_owner = self
                .character(job.installer_id)
                .map_or(0, |c| c.station_owner_id);
            self.charge(
                job.installer_id,
                job.cost as f64,
                entry_type,
                station_owner,
                job_id,
            );
        }
        self.push(crate::notify::IndustryJob {
            to: vec![job.owner_id],
            job,
            successful_runs: None,
        });
        Ok(job_id)
    }

    /// Take ISK out of a character's wallet, record it, and tell the client.
    ///
    /// **The public debit two subsystems have been asking for.**
    /// `purchase_skills` and `service::reprocess`'s tax each carry a comment
    /// saying they do not charge because this did not exist; industry's
    /// install fee is the first caller. All three halves of a debit are here,
    /// because any one of them alone is a bug: the balance (what the player
    /// can spend), the journal row (what the Wallet window's Journal tab
    /// shows) and the `OnAccountChange` push (which is the only reason the
    /// neocom updates without a relog -- `GetCashBalance` is called once in
    /// `WalletSvc.Run` and cached).
    ///
    /// `amount` is **positive and is taken**. The journal keeps the signed
    /// figure, negative for a debit, which is how the client renders it;
    /// `balance` on the row is the balance *after*, so the Journal tab reads
    /// as a running total.
    ///
    /// **This does not check the balance**, and that is deliberate rather
    /// than an omission: the caller has to have refused the transaction
    /// already, because the refusal needs to name a reason the client has a
    /// message for. Returns the new balance, or `None` when there is no such
    /// character -- never a partial debit.
    ///
    /// `crate::pyutil::filetime_now()` and not zero: a year-1601 date takes down the whole
    /// Journal tab, see `crate::ledger`'s module docs.
    pub fn charge(
        &mut self,
        actor: i64,
        amount: f64,
        entry_type_id: i64,
        counterparty_id: i64,
        reference_id: i64,
    ) -> Option<f64> {
        let c = self.character_mut(actor)?;
        c.balance -= amount;
        let (balance, owner_id) = (c.balance, c.character_id);
        self.dirty = true;
        self.journal.record(
            crate::pyutil::filetime_now(),
            entry_type_id,
            owner_id,
            counterparty_id,
            reference_id,
            -amount,
            balance,
            "",
        );
        self.push(crate::notify::AccountChange { owner_id, balance });
        Some(balance)
    }

    /// Deliver a finished job: create the output, then mark the job done.
    ///
    /// Same order and the same reason as [`Self::install_job`]. The push
    /// carries the real `successfulRuns` here, which is the one case where it
    /// is not `None`.
    pub fn deliver_job(
        &mut self,
        delivery: crate::industry::Delivery,
        done: crate::industry::Job,
    ) -> Result<(), crate::item::EditError> {
        let changes = self.apply_items(&delivery.edits)?;
        self.dirty = true;
        // The changes, because a copy or an invention mints blueprints whose
        // ids only `apply` knows, and each needs its runs, ME and TE recorded
        // against that id (`crate::industry::Jobs::commit_delivery`).
        self.jobs.commit_delivery(&delivery, done, &changes);
        self.push(crate::notify::IndustryJob {
            to: vec![done.owner_id],
            job: done,
            successful_runs: Some(done.successful_runs),
        });
        Ok(())
    }

    /// Cancel a job. No items move: the materials are gone, which is what
    /// cancelling an industry job costs in EVE too.
    pub fn cancel_job(&mut self, done: crate::industry::Job) {
        self.dirty = true;
        self.jobs.commit(done);
        self.push(crate::notify::IndustryJob {
            to: vec![done.owner_id],
            job: done,
            successful_runs: None,
        });
    }

    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }

    /// The world rules, writable. Only `rules::set_boot` and
    /// `rules::set_live` call this: they validate first, and a live change of
    /// the training rate freezes the queues around the write.
    pub(crate) fn rules_mut(&mut self) -> &mut crate::rules::Rules {
        &mut self.config.rules
    }

    /// Is friendly fire legal in this player corporation? What
    /// `corpRegistry.GetAggressionSettings` answers (`corp.rs`), read by
    /// crimewatch's `CanAttackFreely` (CW-1). `false` for an NPC corporation
    /// or an unknown id.
    pub fn corp_friendly_fire(&self, corporation_id: i64) -> bool {
        self.corporations
            .get(corporation_id)
            .is_some_and(|c| c.aggression.legal_at(crate::pyutil::filetime_now()))
    }

    /// The character with this id, or `None`.
    ///
    /// **`Option`, not `Vec`.** A character id names exactly one character;
    /// two `Character`s with one id in `config.characters` would be a data
    /// bug, not a case to handle.
    ///
    /// This is the replacement for `config.characters.first()`, which this
    /// file and its submodules read at 29 sites to mean "the acting
    /// character". It was right for exactly one character and silently wrong
    /// for two: `.first()` answers about whoever `SandboxConfig` happens to
    /// list first, for every caller, forever. The acting character now
    /// arrives as `Request::actor`.
    ///
    /// Linear over a `Vec` on purpose: at N <= 10 it is free, and it keeps
    /// `character_selection_data`'s order and `to_snapshot` untouched -- a
    /// container migration would have been a second mechanical change on top
    /// of the first.
    /// How many characters exist (a cache witness: it moves when one is
    /// created or deleted).
    pub(crate) fn character_count(&self) -> usize {
        self.config.characters.len()
    }

    pub fn character(&self, id: i64) -> Option<&Character> {
        let found = self.char_at(id).map(|at| &self.config.characters[at]);
        debug_assert_eq!(
            found.map(|c| c.ship_id),
            self.config
                .characters
                .iter()
                .find(|c| c.character_id == id)
                .map(|c| c.ship_id),
            "character index out of step with config.characters"
        );
        found
    }

    /// The same, mutably. One caller today: `/giveisk`.
    pub fn character_mut(&mut self, id: i64) -> Option<&mut Character> {
        let i = self.char_at(id)?;
        self.config.characters.get_mut(i)
    }

    /// Position of a character in `config.characters`: the index when it
    /// covers the whole list (hit verified), the scan otherwise.
    fn char_at(&self, id: i64) -> Option<usize> {
        let list = &self.config.characters;
        if self.char_indexed == list.len() {
            return match self.char_index.get(&id) {
                Some(&at) if list.get(at).is_some_and(|c| c.character_id == id) => Some(at),
                Some(_) => list.iter().position(|c| c.character_id == id),
                None => None,
            };
        }
        list.iter().position(|c| c.character_id == id)
    }

    /// The player corp a character is in and their `rolesAtAll` there, or
    /// `None` in an NPC corp (SOC-C2, `crate::group_contacts`).
    pub(crate) fn corp_membership(&self, character_id: i64) -> Option<(i64, i64)> {
        self.corporations
            .membership(character_id)
            .map(|m| (m.corporation_id, m.roles))
    }

    /// Player corporations, their members and applications, for the modules
    /// outside `service` (C8, `crate::war`); the field itself stays private.
    pub(crate) fn corporations(&self) -> &corp::Corporations {
        &self.corporations
    }

    /// Tests only (SMALL-SOCIAL): enrol a member or give one roles.
    #[cfg(test)]
    pub(crate) fn corporations_mut(&mut self) -> &mut corp::Corporations {
        &mut self.corporations
    }

    /// Give a CEO from before CORP-W the wallet roles on login (CORP-T,
    /// `corp::heal_ceo_roles`). `dispatch.rs` calls it before the login
    /// change is built, so the change carries the role keys.
    pub(crate) fn heal_ceo_roles_at_login(&mut self, character_id: i64) {
        corp::heal_ceo_roles(self, character_id);
    }

    pub fn sessions(&self) -> &crate::session_table::SessionTable {
        &self.sessions
    }

    /// The chat hub, if chat is on. See the field.
    pub fn chat(&self) -> Option<&std::sync::Arc<crate::chat::ChatHub>> {
        self.chat.as_ref()
    }

    /// Turn chat on. Called once, from `Server::bind`, before the world is
    /// shared -- and from tests.
    pub fn set_chat(&mut self, hub: std::sync::Arc<crate::chat::ChatHub>) {
        // The player channels the world file kept.
        hub.seed_players(self.deltas().of(crate::chat::KIND));
        self.chat = Some(hub);
        // A world attached to a hub always has a directory (PA-11).
        crate::chat_dir::publish(self);
    }

    /// Register a connection. Called from `server::serve_one` on connect and
    /// nowhere else.
    ///
    /// **Takes the world write lock on a path that previously did not touch
    /// it**, so there is nothing fallible in here: a panic while holding that
    /// lock poisons it and takes the world away from every other player
    /// (`docs/reference/world-state.md` §4 rule 5).
    pub fn session_opened(&mut self, entry: crate::session_table::SessionEntry) {
        self.sessions.insert(entry);
    }

    /// Unregister a connection, from `serve_one`'s exit path -- explicitly,
    /// on every path, the same discipline as `Wire::close`.
    ///
    /// **Also stamps the character's logoff date**, because this is the one
    /// place that knows a session ended and which character it was. Doing it
    /// here rather than in a disconnect handler somewhere is what makes it
    /// impossible to forget on an exit path.
    pub fn session_closed(&mut self, client_id: i64) {
        self.session_closed_with(client_id, &crate::module::plan().on_logoff);
    }

    /// [`Self::session_closed`] with its logoff table passed in, so
    /// `session_hooks`' tests can run their own rows through the real
    /// last-session guard.
    pub(crate) fn session_closed_with(
        &mut self,
        client_id: i64,
        hooks: &[crate::session_hooks::Row],
    ) {
        let who = self
            .sessions
            .entries()
            .iter()
            .find(|e| e.client_id == client_id)
            .and_then(|e| e.character_id);
        self.sessions.remove(client_id);
        if let Some(id) = who {
            // **Both of these are inside the `is_online` check, and the stamp
            // was not.** A character with two sessions who closes one is
            // still playing, so stamping a logoff date for them is simply
            // false -- and it is not inert, because `logoff_date` is what
            // orders the character-select screen (newest first), so the first
            // window to close would push a character who is still in space to
            // the top of the list as "most recently played".
            //
            // The guard runs **after** `sessions.remove` above, which is what
            // makes it mean "no sessions left" rather than "no other
            // sessions": `characters_at_station` and `is_online` both read the
            // session table, and with this entry already gone a remaining one
            // still counts.
            if !self.sessions.is_online(id) {
                self.stamp_logoff(id);
                // The rest of a logoff is `session_hooks::ON_LOGOFF` (seam S14):
                // stop modules, contact presence, guest frame, the log-off hold
                // with its emergency warp, fleet leave -- in that order.
                crate::session_hooks::fire(hooks, self, id);
            }
        }
    }

    /// Record that this character just logged off.
    ///
    /// **Strictly later than every other character's**, which is not
    /// fussiness: `charselData.py:65` sorts the select screen with
    /// `key=lambda x: x.logoffDate, reverse=True`, so this field decides
    /// which character the player sees first. **[V]** Two characters logging
    /// off inside one FILETIME tick would otherwise tie, and `list.sort` is
    /// stable, so the tie silently falls back to roster order -- the screen
    /// would then show a stale "most recent" with nothing wrong anywhere to
    /// find. One tick past the highest is cheaper than the bug report.
    fn stamp_logoff(&mut self, character_id: i64) {
        let highest = self
            .config
            .characters
            .iter()
            .map(|c| c.logoff_date)
            .max()
            .unwrap_or(0);
        let now = crate::pyutil::filetime_now().max(highest + 1);
        if let Some(c) = self.character_mut(character_id) {
            c.logoff_date = now;
            self.dirty = true;
        }
    }

    /// The one writer of character identity onto a session entry. Paired
    /// with the write to `Session::character_id`: both, or neither.
    pub fn session_selected_character(&mut self, client_id: i64, character_id: i64) {
        let was_online = self.sessions.is_online(character_id);
        self.sessions.set_character(client_id, character_id);
        // Back in the ship their log-off left in space, if it is still there
        // (`Self::character_change` put them in it): no longer held, and the
        // ballpark bind that follows finds it where it is.
        crate::crimewatch::reclaim(self, character_id);
        self.announce_station_guest(character_id, true);
        // Watchers hear of the first session only; a second window is not a
        // log-on. `crate::contacts::contact_presence`.
        if !was_online && let Some(n) = crate::contacts::contact_presence(self, character_id, true)
        {
            self.push(n);
        }
        // The first session only, as above (seam S14; empty today).
        if !was_online {
            crate::session_hooks::fire(&crate::module::plan().on_login, self, character_id);
        }
    }

    /// Tell everyone **else** in this character's station that they came or
    /// went.
    ///
    /// The audience excludes the character themselves on purpose: their own
    /// client has just asked `GetGuests` and already has the full list, and
    /// `_add_guest` would put them in their own lobby twice.
    ///
    /// Queued through [`Self::push`] like any other notification, so it is
    /// routed by audience and posted to the other characters' own
    /// connections. Silent when nobody else is there, which is the common
    /// case and not worth a frame.
    pub(crate) fn announce_station_guest(&mut self, character_id: i64, arrived: bool) {
        if let Some(n) = self.station_guest_frame(character_id, arrived) {
            self.push(n);
        }
        // A pilot docked in a structure (U4): its own events, arrivals only
        // (the departure is `session_hooks`'s, on the clock queue).
        if arrived {
            crate::structure_dock::announce_login(self, character_id);
        }
    }

    /// The `OnCharNowInStation` / `OnCharNoLongerInStation` frame, built but
    /// not queued.
    ///
    /// Split out from [`Self::announce_station_guest`] so the **departure**
    /// path can put it on the clock queue instead of the turn queue — see
    /// [`Self::session_closed`], where there is no turn to ride on.
    pub(crate) fn station_guest_frame(
        &self,
        character_id: i64,
        arrived: bool,
    ) -> Option<crate::notify::StationGuest> {
        let me = self.character(character_id)?;
        // Docked-now, not "my station": a pilot whose connection drops in
        // space was never a guest, and telling the lobby they left it would
        // be the first it heard of them.
        let station = self.docked_station(character_id)?;
        // A structure has its own guest events (U4).
        if crate::structure_dock::is_structure(self, station) {
            return None;
        }
        // The row the panel wants, built the same way `station.GetGuests`
        // builds it -- see `notify::StationGuest` for why identical matters.
        let row = vec![
            PyValue::Int(me.character_id),
            PyValue::Int(me.corporation_id),
            me.alliance_id.map_or(PyValue::None, PyValue::Int),
            PyValue::None,
        ];
        let to: Vec<i64> = self
            .characters_at_station(station)
            .into_iter()
            .map(|c| c.character_id)
            .filter(|&id| id != character_id)
            .collect();
        if to.is_empty() {
            return None;
        }
        Some(crate::notify::StationGuest { to, row, arrived })
    }

    /// Distinct characters docked in this station, for `station.GetGuests`.
    ///
    /// The station comes from `Character::station_id` rather than from the
    /// session entry: a docked character's station is held on the character
    /// and nothing else moves it yet, and `docs/reference/world-state.md:109-112` says
    /// explicitly not to copy `stationid`/`locationid` onto a session --
    /// they are derived, and the client derives them too.
    /// Everyone standing in this station: online, docked there, and **not in
    /// space**.
    ///
    /// That last clause was missing and the omission was invisible until two
    /// players could be undocked at once. `Character::station_id` is never
    /// cleared -- nothing in the tree assigns it after `Registry::new` -- so
    /// it means "the station this character undocks from", not "where they
    /// are". Without the roster there was no way to tell the difference, and
    /// the guest list showed pilots who were out flying.
    pub fn characters_at_station(&self, station_id: i64) -> Vec<&Character> {
        self.sessions
            .online_characters()
            .into_iter()
            .filter(|id| self.docked_station(*id) == Some(station_id))
            .filter_map(|id| self.character(id))
            .collect()
    }

    /// **The station this character is docked in right now**, or `None`
    /// when they are not docked. The one answer to "is this pilot docked";
    /// `docs/lanes/docked.md` is the audit of every reader.
    ///
    /// Two different facts were sharing one field, and this splits them:
    ///
    /// - `Character::station_id` is **"my station"**: the station the pilot
    ///   is based at, undocks from and docks back into. Nothing clears it on
    ///   undock (`undock_change` and `dock_change` both *read* it), and the
    ///   item store addresses a pilot's hangar -- and an undocked ship -- by
    ///   it. It is the station half of what `Character::solar_system_id` is
    ///   for systems (`docs/lanes/stargates.md` §2.3).
    /// - **Docked-now** is derived: the pilot is not in space. "In space" is
    ///   the roster (`space_state::Roster::is_in_space`, the server's only
    ///   representation of undocked) **or a stargate jump in flight**
    ///   (`crate::jump::Jumps`): a jumper leaves the roster at the gate and
    ///   rejoins it at the far side's bind, and in between is in neither a
    ///   station nor a ballpark.
    ///
    /// `[A]` **One gap, by design:** between `ship.Undock`'s session change
    /// and the ballpark bind that seeds the roster
    /// (`destiny::on_ballpark_bind`) the pilot still reads as docked. The
    /// client is tearing its station view down in that window and makes
    /// none of the calls this guards; closing it would need a third
    /// "undocking" state with its own writers.
    pub fn docked_station(&self, character_id: i64) -> Option<i64> {
        if self.roster.is_in_space(character_id) || self.jumps.contains(character_id) {
            return None;
        }
        // A pilot logged back in to the spot their ship left space at is in
        // space from the login's session change, before the bind seats them.
        if crate::logoff_spot::get(self, character_id).is_some() {
            return None;
        }
        self.character(character_id)?.station_id
    }

    /// **The system this character is in**: what their client has as
    /// `session.solarsystemid2`. In space, the roster's; in a jump, the
    /// origin until the session change and the destination after
    /// (`jump::Jumps::session_system`); docked (or offline), their
    /// station's. The one answer to "which system is this pilot in";
    /// `Character::solar_system_id` alone is "my station's system", which
    /// only docking, a clone jump and seating move.
    pub fn session_system(&self, character_id: i64) -> Option<i64> {
        if let Some(p) = self.roster.get(character_id) {
            return Some(p.solar_system_id);
        }
        if let Some(s) = self.jumps.session_system(character_id) {
            return Some(s);
        }
        if let Some(spot) = crate::logoff_spot::get(self, character_id) {
            return Some(spot.system);
        }
        self.character(character_id)?.solar_system_id
    }

    /// Mint a bound object **owned by the world**, for a reference that must
    /// resolve on more than one connection (GAP-TRADE's `tradeSession`).
    ///
    /// The id is `N=<node>:w<seq>`: one colon, as `ParseObjectID` wants, and
    /// the `w` keeps it apart from the process-global integer serials of
    /// per-session objects. Returns the id and the reference to put in a
    /// reply or a push. **[A]** the client treats a reference inside a
    /// notification as it does one in a reply; falsified by the receiver's
    /// window not opening, or its `List()` going to an unknown object.
    pub(crate) fn mint_world_object(
        &mut self,
        bound: &str,
        bind_params: PyValue,
    ) -> (String, PyValue) {
        self.world_object_seq += 1;
        let id = format!("N={}:w{}", self.config.node_id, self.world_object_seq);
        self.world_objects
            .insert(id.clone(), (bound.to_owned(), bind_params));
        let reference = eve_proto::object_reference(&id, crate::pyutil::filetime_now());
        (id, reference)
    }

    /// Whether `id` is a world-minted id that was issued and is gone now
    /// (UNMACHO): `N=<our node>:w<seq>` with `seq` at most the last minted.
    pub(crate) fn world_object_gone(&self, id: &str) -> bool {
        id.strip_prefix("N=")
            .and_then(|r| r.split_once(':'))
            .and_then(|(node, rest)| Some((node.parse::<i64>().ok()?, rest.strip_prefix('w')?)))
            .and_then(|(node, seq)| Some((node, seq.parse::<u64>().ok()?)))
            .is_some_and(|(node, seq)| {
                node == self.config.node_id
                    && (1..=self.world_object_seq).contains(&seq)
                    && !self.world_objects.contains_key(id)
            })
    }

    /// The service and bind params of a world-minted object, if `id` is one.
    pub(crate) fn world_object(&self, id: &str) -> Option<(String, PyValue)> {
        self.world_objects.get(id).cloned()
    }

    /// Forget the world-minted objects `keep` does not name, once there are
    /// more than `above` of them (a finished object stays answerable for a
    /// while: a closing window still calls `Abort` on it).
    pub(crate) fn prune_world_objects(&mut self, above: usize, keep: impl Fn(&str) -> bool) {
        if self.world_objects.len() > above {
            self.world_objects.retain(|id, _| keep(id));
        }
    }

    /// This character's Bio tab text, or `None` if they never saved one.
    /// The ESS push throttle row of `system` (see `crate::ess`).
    pub(crate) fn ess_told_mut(&mut self, system: i64) -> &mut (i64, bool) {
        self.ess_told.entry(system).or_insert((0, false))
    }

    /// Systems with a deposit not yet pushed.
    pub(crate) fn ess_dirty(&self) -> Vec<(i64, i64)> {
        self.ess_told
            .iter()
            .filter(|(_, v)| v.1)
            .map(|(k, v)| (*k, v.0))
            .collect()
    }

    pub fn character_description(&self, id: i64) -> Option<String> {
        self.character_description.get(&id).cloned()
    }

    pub fn set_character_description(&mut self, id: i64, text: String) {
        self.character_description.insert(id, text);
        self.dirty = true;
    }

    /// Pushes queued and not yet drained (the turn trace's `left` and `queued`).
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Entries in the clock's queue (`schedule`), due or not.
    pub fn scheduled_len(&self) -> usize {
        self.scheduled.len()
    }

    /// Hand over everything the world wants pushed, and forget it.
    ///
    /// Draining rather than reading is deliberate: a notification describes a
    /// transition, so sending one twice is a lie about how many times it
    /// happened -- `WalletSvc._OnCharAccountChange` would blink the neocom
    /// twice and compute a second difference of zero.
    ///
    /// **And filtered to the grid as it leaves** (`crate::grid::filter`): a
    /// Destiny push reaches only the clients whose park holds the balls it
    /// names, and the model of each park follows the same order as the
    /// queue. This drain and [`Self::take_due`] are the only two ways out.
    pub fn take_pending(&mut self) -> Vec<(Duration, Box<dyn crate::notify::Push>)> {
        let mut out = Vec::new();
        for (after, n) in std::mem::take(&mut self.pending) {
            for p in crate::grid::filter(self, n) {
                out.push((after, p));
            }
        }
        out
    }

    /// Ask for a notification to go out in the same flush as the answer.
    ///
    /// Generic over [`crate::notify::Push`], which is the whole point of that
    /// trait: this function is the only thing the world needs to know about
    /// the wire, and adding a push costs one new type in the module that owns
    /// it rather than an edit here and another in `dispatch.rs`.
    pub(crate) fn push(&mut self, n: impl crate::notify::Push + 'static) {
        self.pending.push((Duration::ZERO, Box::new(n)));
    }

    /// **One cache drop for a batch of inventory moves.** A call that moves
    /// many items (a multi-fit, "Fit to active ship", a saved fitting) goes
    /// through [`Self::apply_items`] once per item, and each of those queued
    /// its own `OnInvalidateCacheForLocations`. The client drops the ship's
    /// inventory container on every one and re-binds and re-lists it for each
    /// consumer that next asks (the client log's "godma is using an old thrown
    /// away invCacheContainer" and one `GetInventoryFromId` per module), in
    /// the middle of the very item changes the open fitting window is
    /// processing: seventeen modules cost seventeen re-binds and a stall.
    ///
    /// Everything queued since `from` (a [`Self::pending_len`] taken before
    /// the batch) that is a plain invalidation is folded into one push per
    /// audience, naming the union of the locations, and sent after the rest.
    /// It is idempotent, so only the count and the order change: the item
    /// rows all arrive first, then the cache is dropped once.
    pub(crate) fn coalesce_invalidations(&mut self, from: usize) {
        const EVENT: &str = "OnInvalidateCacheForLocations";
        if from >= self.pending.len() {
            return;
        }
        let tail = self.pending.split_off(from);
        let mut merged: Vec<(Option<Vec<i64>>, Vec<i64>)> = Vec::new();
        for (after, push) in tail {
            let (name, args) = push.to_push();
            if name != EVENT || !after.is_zero() {
                self.pending.push((after, push));
                continue;
            }
            let audience = push.audience();
            let mut ids: Vec<i64> = Vec::new();
            if let Some(PyValue::List(list)) = args.first() {
                ids.extend(list.iter().filter_map(|v| match v {
                    PyValue::Int(n) => Some(*n),
                    _ => None,
                }));
            }
            match merged.iter_mut().find(|(a, _)| *a == audience) {
                Some((_, all)) => {
                    for id in ids {
                        if !all.contains(&id) {
                            all.push(id);
                        }
                    }
                }
                None => merged.push((audience, ids)),
            }
        }
        for (audience, location_ids) in merged {
            self.push(crate::notify::Addressed {
                to: audience,
                inner: Box::new(crate::notify::InvalidateLocations { location_ids }),
            });
        }
    }

    /// Ask for a notification to go out `after` has passed, on the ticker's
    /// thread rather than this one.
    pub(crate) fn push_later(&mut self, after: Duration, n: impl crate::notify::Push + 'static) {
        self.pending.push((after, Box::new(n)));
    }

    /// Ask for a notification to be sent `after` has passed, **by the clock
    /// and not by anybody's payload turn**.
    ///
    /// **This is the second queue, and the separation is the design.**
    /// [`Self::pending`] is drained by `dispatch.rs` after each payload, and
    /// `destiny::on_ballpark_bind` depends on that: its `SetState` has to land
    /// *after* the `CallRsp` in the same flush, so a clock that could take a
    /// fresh entry out of `pending` could reorder a ballpark bootstrap ahead
    /// of the answer to the bind that asked for it. Two queues with one owner
    /// each cannot interact, which is why this is a field rather than a
    /// `Duration` on the existing one. `docs/port-tracker.md` §30.4 has the
    /// attempt that got this wrong first.
    ///
    /// What this unlocks is everything the server wants to do **on its own
    /// schedule**: a warp's arrival reconciliation, a lock dropped when its
    /// target leaves range, and -- the premise of the end goal -- a bot that
    /// acts while every human is idle. Before it, `Registry::take_pending`
    /// was called from exactly one place and nothing the server decided by
    /// itself could reach a quiet client.
    ///
    /// The stamp is **not** chosen here. See
    /// [`crate::notify::Push::restamp`]: a Destiny batch built now and sent
    /// later is stamped when it leaves, which is what makes a delayed batch
    /// monotonic without any future-stamp arithmetic.
    pub(crate) fn schedule(&mut self, after: Duration, n: impl crate::notify::Push + 'static) {
        // A delay counts from the call: the tick's `now` predates the wait for
        // the clock's lock, which can eat the delay whole.
        let due = match self.tick_now {
            Some(now) if after.is_zero() => now,
            _ => Instant::now() + after,
        };
        self.scheduled.push((due, Box::new(n)));
    }

    /// **The one seam for "leaves in this tick".** The clock sets the `now` it
    /// drains with before its work and clears it after the drain, so `schedule`
    /// with a zero delay inside a stage is due at that very `now`. Read from
    /// `Instant::now()` instead, the stamp is later than the tick's `now` and
    /// the push leaves a tick (`push::TICK`) late. Returns the previous value
    /// so a nested caller can restore it.
    pub(crate) fn set_tick_now(&mut self, now: Option<Instant>) -> Option<Instant> {
        std::mem::replace(&mut self.tick_now, now)
    }

    /// Drop the scheduled pushes `gone` names, before they leave. How many.
    ///
    /// For a push that a later decision makes wrong: a warp's arrival
    /// reconciliation, once the pilot has taken control of the landed ship.
    pub(crate) fn cancel_scheduled(
        &mut self,
        gone: impl Fn(&dyn crate::notify::Push) -> bool,
    ) -> usize {
        let before = self.scheduled.len();
        let started = std::time::Instant::now();
        self.scheduled.retain(|(_, n)| !gone(n.as_ref()));
        crate::clock_probe::note_cancel(before, started.elapsed());
        before - self.scheduled.len()
    }

    /// Is any scheduled push (not yet drained by the clock) one `which` names?
    pub(crate) fn has_scheduled(&self, which: impl Fn(&dyn crate::notify::Push) -> bool) -> bool {
        self.scheduled.iter().any(|(_, n)| which(n.as_ref()))
    }

    /// Everything scheduled whose time has come, stamped as it leaves.
    ///
    /// Called only by `push::spawn_world_ticker`, under the world lock, which
    /// it drops before touching a wire.
    ///
    /// Two things happen here and both have to happen *here*:
    ///
    /// - **the stamp is bound now**, with the same
    ///   [`Self::next_destiny_stamp`] every immediate push uses, so a batch
    ///   that waited sixty seconds carries the simulation time it is actually
    ///   arriving at and the floor advances exactly as it always does;
    /// - **one stamp for the whole drain**, not one per batch. Two batches
    ///   leaving in the same tick are two notifications in the same second,
    ///   and equal stamps in consecutive batches are free -- the client's
    ///   `SynchroniseToSimulationTime` evolves `range(0)` times. Calling
    ///   `next_destiny_stamp` per batch would give the same answer anyway,
    ///   since it is whole Unix seconds; taking it once says so.
    ///
    /// Entries not yet due are kept, in order. `Vec::retain` is enough here
    /// and a priority queue would be premature: the queue holds one entry per
    /// ship in flight, not one per tick.
    pub fn take_due(&mut self, now: Instant) -> Vec<Box<dyn crate::notify::Push>> {
        if self.scheduled.iter().all(|(at, _)| *at > now) {
            // The overwhelmingly common case, and worth not taking a stamp
            // for: `next_destiny_stamp` moves the floor, and a clock that
            // nudged it four times a second forever would make the floor a
            // function of uptime rather than of what we have sent.
            return Vec::new();
        }
        let mut due = Vec::new();
        let mut keep = Vec::new();
        for (at, n) in std::mem::take(&mut self.scheduled) {
            if at <= now {
                due.push(n);
            } else {
                keep.push((at, n));
            }
        }
        self.scheduled = keep;
        // A push whose truth ended while it waited is dropped (RAT-GHOST).
        due.retain(|n| n.still_stands(self));
        if due.is_empty() {
            return Vec::new();
        }
        let started = std::time::Instant::now();
        let stamp = self.next_destiny_stamp();
        for n in &mut due {
            n.restamp(stamp);
        }
        // **Filtered to the grid after the stamp is bound**, so a batch cut
        // for one pilot carries the stamp it leaves with
        // (`crate::grid::filter`, [`Self::take_pending`]).
        let filtering = std::time::Instant::now();
        let out = crate::grid::filter_all(self, due);
        // PA-0 profile: the filter apart from the scan, stamp and restamp.
        let filtered = filtering.elapsed();
        crate::live_status::prof::add(crate::live_status::prof::Sec::DrainFilter, filtered);
        crate::live_status::prof::add(
            crate::live_status::prof::Sec::DrainTake,
            started.elapsed().saturating_sub(filtered),
        );
        out
    }

    /// How many notifications are waiting on the clock.
    ///
    /// For tests and for the startup line; nothing in the server branches on
    /// it.
    pub fn scheduled_count(&self) -> usize {
        self.scheduled.len()
    }

    /// Service names we answer for, derived from the table that actually
    /// routes them.
    ///
    /// This was a hand-maintained list sitting beside `DOMAINS`, and a list
    /// beside a table is the shape of thing that rots: `userSvc` sat in it
    /// for a while with no arm anywhere, so the log line and the tests both
    /// claimed we answered a service every real call to would have been
    /// refused. It was also **the last file every lane had to touch to add a
    /// service** -- one file as the parallelism cap, for the second time.
    /// Deriving it means a module advertises its own services and nothing
    /// else has to agree.
    ///
    /// `OBJECT_RETURNING` services are in here too: their call is answered a
    /// layer up, in [`Self::dispatch`], so they have no arm in any module but
    /// they are still services we serve.
    pub fn services() -> Vec<&'static str> {
        let mut out: Vec<&'static str> = crate::module::plan()
            .services
            .iter()
            .flat_map(|(services, _)| services.iter().copied())
            .chain(Self::OBJECT_RETURNING.iter().map(|(svc, _, _)| *svc))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The command names we tell the client we know.
    ///
    /// Returned from `slash.SlashCmd("/")` inside a `UserError`, and used only
    /// for autocompletion in `MatchCmd` -- listing a name authorises nothing and
    /// omitting one forbids nothing. Deliberately short: these are the ones a
    /// single-player sandbox actually wants, and every one of them is
    /// station-safe (the client hides the Destiny-dependent spawn commands while
    /// docked, in `GetGMTypeMenu`'s `if not session.stationid`).
    pub const SLASH_COMMANDS: [&str; 13] = [
        "createitem",
        // `/entity deploy <qty> <typeID>`, the client's own GM string
        // (`menusvc.py:1058-1065`). Not station-safe like the rest: it
        // answers "you must be in space" while docked.
        "entity",
        "giveisk",
        "giveskill",
        "removeskill",
        "givestarterskills",
        "setstanding",
        "whoami",
        // Not a real EVE command. Ours, and it stays: the one-line answer to
        // "is the push path alive", in the spirit of
        // `tools/check-resources.sh`.
        "pushtest",
        // Also ours, and also not a real EVE command. See the arm.
        "tp",
        // Ours (W13 CP25, C7 §7.3-§7.4): the live views and the forced
        // daily rollover.
        "live",
        "rollover",
        // Ours (W16 CP43, C5 §7.4): the economy ledger's view.
        "econ",
    ];

    /// Plain service calls whose return value is an **object**, not data.
    ///
    /// `(service, method, the synthetic service name its methods live under)`.
    ///
    /// Most bound objects arrive through `MachoBindObject`, where the client
    /// asks for one explicitly. These are the other way in: an ordinary
    /// `CallReq` whose *return* is a live reference, which the client then
    /// calls methods on. `skillsvc.GetSkillHandler` is the model —
    ///
    /// ```python
    /// if not self.skillHandler:
    ///     self.skillHandler = session.ConnectToRemoteService('skillMgr2').GetMySkillHandler()
    /// return self.skillHandler
    /// ```
    ///
    /// — and it then calls some twenty-five methods on the result. **No data
    /// value can stand in**: the client never reads an attribute off it, only
    /// calls methods, so `None` or an empty container both raise
    /// `AttributeError` inside `SkillQueueService.Run`. That failure is worse
    /// than it looks, because `StartServiceRun` stores the exception and
    /// `WaitForServiceObjectState` re-raises it on *every* later
    /// `sm.GetService('skillqueue')` — the service is poisoned for the rest
    /// of the session.
    ///
    /// Leaving it unanswered is merely a hang, and not in the session-change
    /// blocking wave, so it costs the neocom skills button rather than the
    /// transition.
    pub const OBJECT_RETURNING: &'static [(&'static str, &'static str, &'static str)] = &[
        ("skillMgr2", "GetMySkillHandler", "skillMgr2::skillHandler"),
        // Both of these are **bound** calls on `invbroker` whose return
        // is itself an object. `PrimeLocation` ends with
        // `invCache.GetInventoryFromId(itemID, 1)`, which resolves
        // through `eveMoniker.GetInventoryMgr()` to
        // `Moniker('invbroker', (stationID, groupStation))` -- so this
        // fires *inside* the blocking session-change handler and is a
        // second gate behind `dogmaIM.GetAllInfo`.
        //
        // The client calls `SetSessionCheck` on the result locally and
        // wraps it in an `invCacheContainer`, whose constructor makes no
        // remote calls, so nothing else is needed at prime time.
        //
        // The second argument (`passive`) is server-side semantics only;
        // no client code branches on it, so one reference serves both
        // values. The hangar's `GetInventory(containerHangar, None)`
        // binds the same moniker.
        ("invbroker", "GetInventoryFromId", "invbroker::inventory"),
        ("invbroker", "GetInventory", "invbroker::inventory"),
        // **The first object-returning call with a side effect**, which is
        // why `handle_object_returning_call` now dispatches before it mints.
        // `fleetSvc.CreateFleet:395` **[V]** assigns the result and
        // immediately calls `.Init` and `.GetFleetID` on it, so this is a
        // reference or it is an `AttributeError`; and unlike the three above
        // -- all pure handle-getters with no registry arm at all -- this one
        // has to create a fleet on the way through.
        ("fleetObjectHandler", "CreateFleet", "fleetObjectHandler"),
        // The per-solar-system scan manager. `scanSvc.GetScanMan`
        // (`eve/client/script/parklife/scanSvc.py`, the method body) is the
        // `skillMgr2` shape exactly:
        //
        // ```text
        //     def GetScanMan(self):
        //         if self.remoteObject is None:
        //             self.remoteObject = sm.RemoteSvc('scanMgr').GetSystemScanMgr()
        //         return self.remoteObject
        // ```
        //
        // The attribute is called `remoteObject`, and every one of the nine
        // uses in that module and in `sensorSuiteService.py` is an
        // **uppercase method call** on it -- `GetFullState`, `RequestScans`,
        // `DestroyProbe`, `ReconnectToLostProbes`, `RecoverProbes`,
        // `SetActivityState`, `QAOverrideProbeExpiry`, `QAScanSites`. No
        // attribute is ever read off it, so no data value can stand in: this
        // is a reference or it is an `AttributeError`.
        //
        // `None` was not merely useless, it was a **live lock**, and of the
        // poisoned-cache kind rather than the throwing kind -- see the arm
        // for `scanMgr::systemScanMgr` in `service/space.rs`.
        ("scanMgr", "GetSystemScanMgr", "scanMgr::systemScanMgr"),
    ];

    /// The synthetic service for a call in [`Self::OBJECT_RETURNING`].
    pub fn object_returning(service: &str, method: &str) -> Option<&'static str> {
        Self::OBJECT_RETURNING
            .iter()
            .find(|(s, m, _)| *s == service && *m == method)
            .map(|(_, _, bound)| *bound)
    }

    /// [`Self::dispatch`] on behalf of `caller`.
    ///
    /// The caller is set for this one dispatch and reset to
    /// [`Caller::Internal`] on every exit, an unwind included (a drop
    /// guard), so a panicking arm cannot leave the next ticker call looking
    /// like a player's. The world write lock is held across the whole
    /// dispatch already, so no other connection ever sees the field.
    pub fn dispatch_as(
        &mut self,
        caller: Caller,
        req: &Request<'_>,
    ) -> Result<PyValue, ServiceError> {
        struct Reset<'r>(&'r mut Registry);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.caller = Caller::Internal;
            }
        }
        self.caller = caller;
        let guard = Reset(self);
        guard.0.dispatch(req)
    }

    /// Lock-free: may `(service, method)` be tried under `world.read()`?
    /// A yes is not a promise; [`Self::dispatch_ro`] may still say `None`.
    pub fn read_eligible(service: &str, method: &str) -> bool {
        method == "MachoResolveObject"
            || READS
                .iter()
                .any(|(ms, _)| ms.iter().any(|(s, m)| *s == service && *m == method))
    }

    /// The read twin of [`Self::dispatch_as`]: the same answer for the calls
    /// in [`READS`], computed from `&self`. `None` = take the write path.
    /// Never touches `self.caller`.
    pub fn dispatch_ro(
        &self,
        caller: Caller,
        req: &Request<'_>,
    ) -> Option<Result<RoValue, ServiceError>> {
        if req.method == "MachoResolveObject" {
            return Some(Ok(RoValue::Owned(PyValue::Int(self.config.node_id))));
        }
        READS
            .iter()
            .filter(|(ms, _)| {
                ms.iter()
                    .any(|(s, m)| *s == req.service && *m == req.method)
            })
            .find_map(|(_, arm)| arm(self, caller, req))
    }

    /// True when a payload turn would drain something into its reply:
    /// queued pushes, session changes, fleet session changes. A read turn
    /// cannot drain them, so it requires `false` (byte identity with the
    /// write path).
    pub fn turn_output_pending(&self) -> bool {
        !self.pending.is_empty()
            || !self.pending_session_changes.is_empty()
            || self.fleets.has_session_changes()
    }

    /// Who the dispatch in progress is for (see [`Caller`]).
    pub fn caller(&self) -> Caller {
        self.caller
    }

    pub fn dispatch(&mut self, req: &Request<'_>) -> Result<PyValue, ServiceError> {
        // `MachoResolveObject` is answered for **every** service, not only the
        // ones we otherwise know.
        //
        // It is one of `CoreService.__machomethods__` -- along with
        // `MachoResolve`, `MachoBindObject` and
        // `MachoGetObjectBoundToSession` -- which every bindable service
        // exports and which are exempt from the `__exportedcalls__` check and
        // run at `ROLE_ANY` (`service.pyc`,
        // `ServiceCallWrapper.PreCall_IsExportedCall`). It asks one question:
        // which node owns this object. We are one node, so the answer never
        // depends on the service or the bindParams.
        //
        // Answering it generically is also the better *diagnostic*. The
        // login-to-station path binds at least `dogmaIM`, `crimewatch`,
        // `invbroker`, `warRegistry` and `officeManager`, and a resolve we
        // refuse would hang the client with only the service name to show for
        // it. Resolving instead lets the client get as far as
        // `MachoBindObject`, whose fused call names the actual method in the
        // `UNANSWERED` line -- which is the thing we need to know.
        //
        // `None` here would be an error the client reports as
        // 'Resolution failure: ResolveObject returned None'.
        if req.method == "MachoResolveObject" {
            return Ok(PyValue::Int(self.config.node_id));
        }

        for (services, dispatch) in &crate::module::plan().services {
            if services.contains(&req.service) {
                return dispatch(self, req);
            }
        }
        Err(ServiceError::NoSuchService(req.service.to_owned()))
    }

    fn macho_net(&self, req: &Request<'_>) -> Result<PyValue, ServiceError> {
        match req.method {
            // An empty dict is correct, not a stub. The client uses this only
            // as a *hint* for guessing which node owns a service; with no
            // hint, calls go to the ANY address, which is exactly what a
            // single-node server wants. The real proxy wraps it in a
            // CachedObject; a bare dict is what the client stores.
            "GetServiceInfo" => Ok(PyValue::Dict(vec![])),
            "GetTime" => Ok(PyValue::Long(crate::pyutil::filetime_now())),
            // MAP-PILOTS: `(sol, sta, divisor)` for the pilot map filters
            // (`mapSvc.py:407-421`). Any other key echoes the client's
            // default, as the real `machoNet` does (`machoNet.py:1708`).
            "GetClusterGameStatisticsForClient" => Ok(match req.args.first() {
                Some(PyValue::Bytes(b)) if b == b"EVE" => crate::map_stats::pilot_counts(self),
                Some(PyValue::Unicode(s)) if s == "EVE" => crate::map_stats::pilot_counts(self),
                _ => req.args.get(1).cloned().unwrap_or(PyValue::None),
            }),
            _ => Err(self.no_method(req)),
        }
    }

    fn char_unbound_mgr(&self, req: &Request<'_>) -> Result<PyValue, ServiceError> {
        match req.method {
            "GetCharacterSelectionData" => Ok(self.character_selection_data()),
            // Truthy draws the "lapsed game time" warning bar.
            "GetCharOmegaDowngradeStatus" => Ok(PyValue::Bool(false)),
            // None means unlocked. Anything else raises a UserError.
            "GetCharacterLockType" => Ok(PyValue::None),
            // `gatekeeper.user` tests `cohortID in GetCohortsForUser()`
            // (`gatekeeper.py:50`); `None` was `TypeError: argument of type
            // 'NoneType' is not iterable`, "Failed to query the gatekeeper"
            // on the character-creation view. No experiment cohorts here.
            "GetCohortsForUser" => Ok(PyValue::List(vec![])),
            // **Half the price of `ROLE_QA`**, and the half with teeth.
            //
            // `ccSvc._QAPickStarterSystem` opens
            //
            //     system_ids = sorted(sm.RemoteSvc('charUnboundMgr').GetQAStarterSystemIDs())
            //     if not system_ids:
            //         return None
            //
            // with no `try/except` anywhere above it, so an unanswered call
            // hangs character creation outright. `sorted([])` is falsy and the
            // function returns immediately, which is the behaviour we want --
            // we are not offering a choice of starter systems because there is
            // one system.
            //
            // This is the call `roles.rs` named as the reason `ROLE_QA` was
            // kept out of the default mask. It is two lines, and it buys the
            // entire Insider UI and QA menus.
            "GetQAStarterSystemIDs" => Ok(PyValue::List(vec![])),
            _ => Err(self.no_method(req)),
        }
    }

    /// The 4-tuple `(userDetails, trainingDetails, characterDetails, wars)`.
    ///
    /// Shapes from `charselData.CharacterSelectionData.FetchCharacterInfoForUserID`:
    ///
    /// - `userDetails` is indexed `[0]`, so it is a **list of one**.
    /// - `trainingDetails` is opaque and is initialised to `(None, None)`
    ///   before the call, so a 2-tuple of `None` is the safe stub.
    /// - `characterDetails` is iterated and read by attribute — which is why
    ///   `KeyVal` suffices and the rowset layer is not needed here.
    /// - `wars` is grouped by `.characterID`; empty is fine.
    fn character_selection_data(&self) -> PyValue {
        let user = keyval(vec![
            // From the caller, so the select screen, `initialstate` and the
            // session read one value once accounts land (account-spec §5).
            // `Internal` keeps the configured id.
            (
                "userID",
                PyValue::Int(match self.caller {
                    Caller::Session { user_id, .. } => user_id,
                    Caller::Internal => self.config.user_id,
                }),
            ),
            ("userName", PyValue::Unicode(self.config.user_name.clone())),
            ("characterSlots", PyValue::Int(self.config.character_slots)),
            (
                "maxCharacterSlots",
                PyValue::Int(self.config.character_slots),
            ),
            ("creationDate", PyValue::Int(0)),
            // Far future, in FILETIME ticks: the subscription never lapses.
            ("subscriptionEndTime", PyValue::Int(i64::MAX / 2)),
        ]);
        PyValue::Tuple(vec![
            PyValue::List(vec![user]),
            PyValue::Tuple(vec![PyValue::None, PyValue::None]),
            // Only what the caller may play (§4.6): every character without
            // `--accounts`, as before; an account's own with it.
            PyValue::List(
                self.config
                    .characters
                    .iter()
                    .filter(|c| self.may_play(self.caller, c))
                    .map(|c| {
                        c.to_row_deleting(crate::start::delete_due(&self.deltas, c.character_id))
                    })
                    .collect(),
            ),
            PyValue::List(vec![]),
        ])
    }

    /// The session change that takes the character out of the station.
    ///
    /// **Exactly two attributes, and both halves of both pairs matter.**
    ///
    /// `eveSessions.IsUndockingSessionChange` is
    ///
    /// ```python
    /// goingFromStation = 'stationid'     in change and change['stationid'][0]
    /// goingToSpace     = 'solarsystemid' in change and change['solarsystemid'][1]
    /// ```
    ///
    /// so `stationid` must be present with a truthy *old* value and
    /// `solarsystemid` present with a truthy *new* one. `ComputeNotifySets`
    /// rewrites the old value to whatever the client currently holds, so the
    /// station id here is documentation rather than data — but the key has to
    /// be there.
    ///
    /// **`locationid` is derived and must still not be sent.**
    /// `RecalculateDependantAttributesWithChanges` computes
    ///
    /// ```python
    /// new = changes.get('stationid', [None, None])[1] \
    ///    or changes.get('solarsystemid', [None, None])[1]
    /// ```
    ///
    /// so `stationid`'s *new* value has to be `None` for `locationid` to pick
    /// up the system instead of staying on the station. And that derived
    /// `locationid` is the only thing that makes
    /// `crimewatchSvc.ProcessSessionChange` fire, because it gates on
    /// `'locationid' in change or 'charid' in change` and we send neither.
    ///
    /// `corprole` stays out by itself: every `rolesAt*` is 0 and `stationid`'s
    /// new value is falsy, so the recalculation's `old != corprole` test is
    /// `0 != 0`. That is what keeps `officeManager.GetMyCorporationsOffices`
    /// out of the blocking wave, exactly as at login — the same §1 rule from
    /// `docs/archive/entering-the-game.md`, paying off a second time.
    ///
    /// `shipid` is absent because it does not change, and an attribute whose
    /// new value equals the current one is dropped *silently*.
    /// `solarsystemid2` is already correct from login.
    pub fn undock_change(&mut self, actor: Option<i64>) -> Option<Vec<(PyValue, PyValue)>> {
        // GAP-REPAIR: the damage a dock stashed goes back into the health
        // record, so a ship leaves as it came in.
        // A structure's undock keeps the system and drops the structure
        // (U4); `structureDocking.Undock` has done the repair hook itself.
        if let Some(id) = self
            .character(actor?)
            .and_then(|c| c.station_id)
            .filter(|id| crate::structure_dock::is_structure(self, *id))
        {
            return Some(crate::structure_dock::undock_change(id));
        }
        crate::repair::on_undock(self, actor?);
        let c = self.character(actor?)?;
        let station = c.station_id?;
        let system = c.solar_system_id?;
        let PyValue::Dict(entries) = pydict(vec![
            (
                "stationid",
                PyValue::Tuple(vec![PyValue::Int(station), PyValue::None]),
            ),
            (
                "solarsystemid",
                PyValue::Tuple(vec![PyValue::None, PyValue::Int(system)]),
            ),
        ]) else {
            unreachable!("pydict builds a dict")
        };
        Some(entries)
    }

    /// The reverse of [`Self::undock_change`]: `{stationid: (None, station),
    /// solarsystemid: (system, None)}`.
    ///
    /// Two keys and no more. **`station` is `Character::station_id` read
    /// after the `CmdDock` arm ran**, and that arm has already made the
    /// station the pilot named their station ([`Self::dock_at`], FX1). So
    /// this reads "my station" and gets the station they docked at; before
    /// FX1 it read the stored one whatever `CmdDock` named, and every ship
    /// docked at its pilot's first station (`docs/port-tracker.md` §72.1).
    ///
    /// - **`locationid` and `corprole` are derived**, by
    ///   `basesession.RecalculateDependantAttributesWithChanges`
    ///   (`basesession.py:2183`): `locationid` becomes
    ///   `stationid_new or solarsystemid_new` by itself, so sending it is a
    ///   lie the client overwrites. **[V]**
    /// - **`shipid` must not be sent at all.** The player docks in the ship
    ///   they undocked in, and a `shipid` in the change while
    ///   `solarsystemid` is `None` takes `invCache.ProcessSessionChange`'s
    ///   branch at `invCache.py:182`, which deletes an inventory and
    ///   **returns early, skipping the cache invalidation the dock needs**.
    ///   **[V]**
    ///
    /// **`solarsystemid` going absent is what tears the ballpark down.** Not
    /// `OnPrimaryViewChanged`: its `RemoveBallpark` branch fires only for
    /// `newView == 'structure'` (`michelle.py:211`) and a station dock gives
    /// `'hangar'`. The teardown is `michelle.UpdateBallpark:227`, gated at
    /// `:216` on `session.solarsystemid` being falsy **[V]**. Leave the system
    /// set and the client draws the hangar over a live ballpark, scatters
    /// `OnEnterSpace`, and -- the part that costs the next undock -- never
    /// re-binds `beyonce`, because the `Park` is still there.
    ///
    /// The ship's motion is forgotten here: it is not in space any more, and
    /// the next undock seeds a fresh one at the mouth.
    pub fn dock_change(&mut self, actor: Option<i64>) -> Option<Vec<(PyValue, PyValue)>> {
        let c = self.character(actor?)?;
        let station = c.station_id?;
        let system = c.solar_system_id?;
        // Out of the roster: this character is no longer in space, and the
        // observers who still have their ball are told by the arm that called
        // us -- see `service/space.rs`'s `CmdDock`.
        //
        // **Locks before the roster**, for the reason `session_closed` gives
        // at length: `left_space` needs the presence to find the ball, and a
        // victim nobody tells keeps a stuck bracket.
        //
        // `WithAnswer`, unlike the disconnect path: a dock *is* a call, and
        // `dispatch` drains `pending` after its answer, so these ride out in
        // the same flush as the session change that tears the park down.
        crate::targeting::left_space(
            self,
            actor?,
            crate::targeting::Reason::Unlocked,
            crate::targeting::Delivery::WithAnswer,
        );
        // GAP-REPAIR: the ship's damage is stashed, not mended.
        crate::repair::on_dock(self, actor?);
        self.roster.leave(actor?);
        let PyValue::Dict(entries) = pydict(vec![
            (
                "stationid",
                PyValue::Tuple(vec![PyValue::None, PyValue::Int(station)]),
            ),
            (
                "solarsystemid",
                PyValue::Tuple(vec![PyValue::Int(system), PyValue::None]),
            ),
        ]) else {
            unreachable!("pydict builds a dict")
        };
        Some(entries)
    }

    /// The station `CmdDock(station_id)` may put `actor` in, or `None` for
    /// a dock the client could not have sent (raised as `NotDockable`).
    ///
    /// **The client's own gates, and which of them the server repeats:**
    ///
    /// - **The station is a ball in the pilot's ballpark**, so it is in the
    ///   pilot's system. `movementFunctions.DockOrJumpOrActivateGate:399`
    ///   reads the target with `bp.GetInvItem(itemID)` and docks only when
    ///   its `groupID` is `groupStation` (`:400`); `RealDock:558` then sends
    ///   `CmdDock(itemID, session.shipid)` on the system's remote park
    ///   **[V]**. Repeated here: the station's system must be the one the
    ///   pilot's ship is in (the roster's, not `Character::solar_system_id`,
    ///   which is the *stored station's* system).
    /// - **Range: surface distance under 2,500 m.** `menusvc.DockStation`
    ///   (`menusvc.py:3503`) goes through `GetCloseAndTryCommand`, whose
    ///   `interactionRange` defaults to `DEFAULT_INTERACTION_RANGE = 2500`
    ///   (`menusvc.py:158`, `:4170`), and `autopilot.InInteractionRange`
    ///   sends only when `destBall.surfaceDist < interactionRange`
    ///   (`autopilot.py:682`, called at `:624`) **[V]**. **Repeated loosely**, not here
    ///   but in `dock_policy::refuse_out_of_range` (2,500 m plus a lag
    ///   allowance, only for a pilot in space and a catalogued station), so
    ///   a modified client cannot dock from across the system. `[A]` for the
    ///   allowance; falsified by an honest dock refused.
    ///
    /// **Which stations exist** ([`Self::station_ref`]): one some character
    /// was seeded at, and with a bundle every NPC station (FX4). The
    /// pilot's own station is always known.
    ///
    /// **Not in space** -- the window between `ship.Undock` and the ballpark
    /// bind, [`Self::docked_station`]'s one gap -- only the pilot's own
    /// station is accepted, which is what every dock did before FX1.
    pub(crate) fn dock_target(&self, actor: i64, station_id: i64) -> Option<character::StationRef> {
        // A structure is docked at through `structureDocking.Dock`, with its
        // access rules (U4), never through `CmdDock`.
        if crate::structure_dock::is_structure(self, station_id) {
            return None;
        }
        let c = self.character(actor)?;
        let mine = c.station_id == Some(station_id);
        let station = if mine {
            character::StationRef::of(c)?
        } else {
            self.station_ref(station_id)?
        };
        match self.roster.get(actor) {
            Some(here) => (here.solar_system_id == station.solar_system_id).then_some(station),
            None => mine.then_some(station),
        }
    }

    /// Every NPC station the server can name: the content's rows and the
    /// seeded ones, in id order, structures left out. For a last-resort
    /// harbour (`structure_dock::harbour`).
    pub(crate) fn npc_station_refs(&self) -> Vec<character::StationRef> {
        let mut ids: std::collections::BTreeSet<i64> = self
            .content()
            .get::<crate::stations::Table>()
            .iter()
            .map(|(id, _)| id)
            .collect();
        ids.extend(self.stations.keys().copied());
        ids.into_iter()
            .filter_map(|id| self.station_ref(id))
            .collect()
    }

    /// A station as the seven fields that put a pilot in it: a seeded one
    /// (`Registry::stations`), else any NPC station the catalogue places
    /// and `stations/stations.tsv` owns (`crate::stations`, FX4). `None`
    /// for an id that is neither -- with no bundle, every unseeded one.
    pub(crate) fn station_ref(&self, station_id: i64) -> Option<character::StationRef> {
        // An Upwell structure is no station (U4), though a pilot docked in
        // one is in `stations`; `structure_dock::station_ref` has it.
        if crate::structure_dock::is_structure(self, station_id) {
            return None;
        }
        if let Some(s) = self.stations.get(&station_id) {
            return Some(*s);
        }
        let cat = &self.config.celestials;
        let c = cat
            .get(station_id)
            .filter(|c| c.kind == crate::celestial::Kind::Station)?;
        let row = self
            .content()
            .get::<crate::stations::Table>()
            .get(station_id)?;
        let system = cat.system_of(station_id).filter(|s| *s != 0)?;
        Some(character::StationRef {
            station_id,
            owner_id: row.owner_id,
            type_id: c.type_id,
            operation_id: row.operation_id,
            solar_system_id: system,
            constellation_id: cat.system(system).map_or(0, |s| s.constellation_id),
            region_id: row.region_id,
        })
    }

    /// Where a station is: CCP's own coordinates when the catalogue has it,
    /// which is where the client draws it and where `cfg.evelocations` puts
    /// it (`crate::stations`) **[V]**; the origin otherwise, which is where
    /// every station sat before FX4 and still sits in a sandbox with no
    /// catalogue.
    pub fn station_position(&self, station_id: i64) -> [f64; 3] {
        // An Upwell structure is where its row says (U4).
        if let Some(g) = crate::structure_dock::geometry(self, station_id) {
            return g.position;
        }
        self.config
            .celestials
            .get(station_id)
            .filter(|c| c.kind == crate::celestial::Kind::Station)
            .map_or([0.0; 3], |c| c.position)
    }

    /// Make `station` this pilot's station, and put their ship in its
    /// hangar. Called from the `CmdDock` arm after [`Self::dock_target`],
    /// before [`Self::dock_change`] builds the session change from it.
    ///
    /// **"Docked here" makes it "my station"** (FX1): undock leaves from
    /// it, `GetSelfInvItem` and the hangar read it, `docked_station` answers
    /// it. `Character::station_id` keeps its meaning -- the station the
    /// pilot belongs to -- and the meaning just follows the pilot.
    ///
    /// **The active ship moves, nothing else does.** A ship in space is
    /// still an item in the hangar it left (the store has no in-space
    /// location), so docking elsewhere is a [`crate::item::Edit::Move`] of
    /// that one item; its fitting and cargo are located in the ship and go
    /// with it. Everything else stays where it was, as assets do.
    ///
    /// `false`, with nothing changed, when the move is refused.
    pub(crate) fn dock_at(&mut self, actor: i64, station: character::StationRef) -> bool {
        let Some(ship_id) = self.character(actor).map(|c| c.ship_id) else {
            return false;
        };
        let away = self
            .store
            .get(ship_id)
            .is_some_and(|i| i.location_id != station.station_id);
        if away {
            let to = crate::item::Slot::new(actor, station.station_id, crate::model::FLAG_HANGAR);
            if let Err(e) = self.apply_items(&[crate::item::Edit::Move {
                item_id: ship_id,
                to,
            }]) {
                crate::outln!(
                    "[dock] {actor}: ship {ship_id} to {}: {e}",
                    station.station_id
                );
                return false;
            }
        }
        let Some(c) = self.character_mut(actor) else {
            return false;
        };
        if c.station_id != Some(station.station_id) {
            c.station_id = Some(station.station_id);
            c.station_owner_id = station.owner_id;
            c.station_type_id = station.type_id;
            c.station_operation_id = station.operation_id;
            c.solar_system_id = Some(station.solar_system_id);
            c.constellation_id = station.constellation_id;
            c.region_id = station.region_id;
            self.dirty = true;
        }
        true
    }

    /// The session change that puts a character in the game.
    ///
    /// Returns the `{attr: (old, new)}` map for
    /// `SessionChangeNotification`, or `None` if we have no such character.
    ///
    /// The field list is not invented: it is what
    /// `SessionMgr.GetSessionValuesFromRowset` sets, which is the **real
    /// server's own session-building code** and ships in the client tree
    /// (`eve/common/script/sys/eveSessions.pyc`). Four things about it are
    /// easy to get wrong and all four fail quietly:
    ///
    /// - **`locationid` and `corprole` must not be sent.** The client derives
    ///   them in `RecalculateDependantAttributesWithChanges`:
    ///   `locationid = stationid or solarsystemid`, and `corprole` from the
    ///   `rolesAt*` values according to whether `stationid` matches
    ///   `hqID`/`baseID`.
    /// - **The `old` value is discarded**, so `None` is correct there.
    /// - **A pair whose new value equals the client's current one is dropped**,
    ///   and if that empties the map then no handler runs at all, silently.
    /// - **Docked means `solarsystemid` stays absent** while `solarsystemid2`
    ///   carries the system; `gameui.OnSessionChanged` routes on
    ///   `stationid is not None` before it ever looks at a system.
    ///
    /// Two deliberate choices, both to keep this first cut small:
    ///
    /// - **Every `rolesAt*` is 0.** That keeps `corprole` out of the derived
    ///   change, which removes a blocking `officeManager` call from the
    ///   critical path.
    /// - **`shipid` is absent.** A character with no active ship is a state
    ///   the client handles explicitly: `_MakeShipActive` opens with
    ///   `if shipID is None: log.LogTraceback(...); return`, so the
    ///   `shipState` from `GetAllInfo` is never unpacked and we do not have
    ///   to model it yet. Giving the character a ship means supplying both.
    pub fn character_change(&self, character_id: i64) -> Option<Vec<(PyValue, PyValue)>> {
        let c = self
            .config
            .characters
            .iter()
            .find(|c| c.character_id == character_id)?;
        let pair = |v: PyValue| PyValue::Tuple(vec![PyValue::None, v]);
        let opt = |v: Option<i64>| v.map(PyValue::Int).unwrap_or(PyValue::None);
        // **Logging back in to a ship still in space.** A pilot who quit
        // undocked left the ship there for a while
        // (`crate::crimewatch::logged_off`), and while it is in the roster
        // the login lands them in it: `stationid` absent and `solarsystemid`
        // set, which `gameui.OnSessionChanged:592-597` routes to
        // `_HandleSessionChangeInSpace` and the inflight view **[V]** --
        // the same change an undock makes, from a character screen. The
        // ballpark bind that follows finds the ship where it is
        // (`destiny::on_ballpark_bind`). Otherwise the login is docked, as
        // it always was: `station_id` is "my station" and never cleared.
        //
        // **Or to the spot it left space at** (`crate::logoff_spot`): the
        // hold ended while they were away, so the ship is no longer in the
        // roster, but the login is the same in-space change and the bind
        // seats the ship at the spot.
        let in_space = self.roster.get(character_id).copied();
        let spot = if in_space.is_none() {
            crate::logoff_spot::get(self, character_id)
        } else {
            None
        };
        let here = in_space
            .map(|p| p.solar_system_id)
            .or(spot.map(|s| s.system));
        let system = here.or(c.solar_system_id);
        let row = here.and_then(|s| crate::jump::system_row(self, character_id, s));
        let constellation_id = row.map_or(c.constellation_id, |r| r.constellation_id);
        let region_id = row.map_or(c.region_id, |r| r.region_id);
        let ship_id = in_space.map_or(c.ship_id, |p| p.ship_id);
        let station_id = if here.is_some() { None } else { c.station_id };
        // **Docked in an Upwell structure**: `stationid` stays absent, and
        // `structureid` and `solarsystemid` are set, which is what
        // `eveSessions` builds for a pilot in one (U4, spec §2).
        let structure_id = station_id.filter(|id| crate::structure_dock::is_structure(self, *id));
        let station_id = if structure_id.is_some() {
            None
        } else {
            station_id
        };
        let roles = corp::session_role_masks(self, c.character_id);
        let mut kv = vec![
            ("charid", pair(PyValue::Int(c.character_id))),
            ("corpid", pair(PyValue::Int(c.corporation_id))),
            ("allianceid", pair(opt(c.alliance_id))),
            // The militia the character serves, directly or through the
            // corp (W16 CP53a); `None` outside factional warfare.
            (
                "warfactionid",
                pair(opt(facwar_mgr::war_faction(self, c.character_id))),
            ),
            ("hqID", pair(PyValue::None)),
            ("baseID", pair(PyValue::None)),
            // A player corp's member logs in holding their roles; the NPC
            // corp's members, as before, with none. All four masks
            // (SOC-C3: the roles window sets the location ones too).
            ("rolesAtAll", pair(PyValue::Int(roles[0]))),
            ("rolesAtHQ", pair(PyValue::Int(roles[1]))),
            ("rolesAtBase", pair(PyValue::Int(roles[2]))),
            ("rolesAtOther", pair(PyValue::Int(roles[3]))),
            ("fleetid", pair(PyValue::None)),
            ("fleetrole", pair(PyValue::None)),
            ("wingid", pair(PyValue::None)),
            ("squadid", pair(PyValue::None)),
            // The capsule. This single field is the fitting window's whole
            // gate: `GetShipIDForFittingWindow` is `eveCfg.GetActiveShip()`
            // is `return session.shipid`, and `None` there is the
            // `CannotPerformActionWithoutShip` prompt.
            //
            // It also changes what `godma.Prime` asks us for: `primeShip` is
            // `session.shipid and session.shipid not in self.invByID`, so the
            // fused `GetAllInfo` call becomes `(True, True, None)` rather
            // than the `(True, None, None)` we had been seeing.
            ("shipid", pair(PyValue::Int(ship_id))),
            ("stationid", pair(opt(station_id))),
            ("structureid", pair(opt(structure_id))),
            // Docked: `solarsystemid` stays None, `solarsystemid2` carries it.
            ("solarsystemid2", pair(opt(system))),
            ("constellationid", pair(PyValue::Int(constellation_id))),
            ("regionid", pair(PyValue::Int(region_id))),
            ("genderID", pair(PyValue::Int(c.gender_id))),
            ("bloodlineID", pair(PyValue::Int(c.bloodline_id))),
            ("raceID", pair(PyValue::Int(c.race_id))),
            (
                "corpAccountKey",
                pair(PyValue::Int(corp_wallet::login_key(self, c.character_id))),
            ),
        ];
        // In space: `solarsystemid` too, as an undock sends it.
        if here.is_some() || structure_id.is_some() {
            kv.push(("solarsystemid", pair(opt(system))));
        }
        let PyValue::Dict(entries) = pydict(kv) else {
            unreachable!("pydict returns a dict")
        };
        Some(entries)
    }

    /// `map.GetStationInfo`: every station, as the map's service filters and
    /// the faction-war colouring read them (MAP-STATIONS).
    ///
    /// A `CRowset`, because callers do `.Index('stationID')`. The rows are
    /// the whole `stations/stations.tsv` table, then any seeded station the
    /// table lacks, then the caller's own docked station if neither names it,
    /// in ascending id order. With no bundle the table is empty and the answer
    /// is what it was: the seeded stations and the caller's.
    ///
    /// The values match the client's own `npcStations` table, so its static
    /// data and ours agree about where a station is.
    fn station_info(&self, actor: Option<i64>) -> PyValue {
        self.station_rowset(actor).to_pyvalue()
    }

    /// [`Self::station_info`] before it is marshalled.
    pub(crate) fn station_rowset(&self, actor: Option<i64>) -> eve_rowset::CRowset {
        let columns = [
            ("stationID", DbType::I4),
            ("solarSystemID", DbType::I4),
            ("ownerID", DbType::I4),
            ("operationID", DbType::I4),
        ]
        .into_iter()
        .map(|(name, db_type)| Column {
            name: name.as_bytes().to_vec(),
            db_type,
        })
        .collect();
        let descriptor =
            DbRowDescriptor::new(columns).expect("the station columns are a valid layout");
        // id -> (system, owner, operation); the table first, so a seeded
        // station of the same id does not replace the content's row.
        let mut rows: std::collections::BTreeMap<i64, (i64, i64, i64)> =
            std::collections::BTreeMap::new();
        for (id, st) in self.content().get::<crate::stations::Table>().iter() {
            rows.insert(id, (st.system_id, st.owner_id, st.operation_id));
        }
        for s in self.stations.values() {
            rows.entry(s.station_id)
                .or_insert((s.solar_system_id, s.owner_id, s.operation_id));
        }
        if let Some(c) = actor.and_then(|id| self.character(id))
            && let Some(station) = c.station_id
        {
            rows.entry(station).or_insert((
                c.solar_system_id.unwrap_or(0),
                c.station_owner_id,
                c.station_operation_id,
            ));
        }
        // An Upwell structure is no NPC station (U4), though a pilot based
        // there is in `stations`.
        rows.retain(|id, _| !crate::structure_dock::is_structure(self, *id));
        let mut rowset = eve_rowset::CRowset::new(descriptor);
        for (id, (system, owner, operation)) in rows {
            rowset
                .push_values(vec![
                    DbValue::Int(id),
                    DbValue::Int(system),
                    DbValue::Int(owner),
                    DbValue::Int(operation),
                ])
                .expect("the station row matches its descriptor");
        }
        rowset
    }

    /// `map.GetStationCount`: `(solarSystemID, count)` pairs, the same set
    /// [`Self::station_info`] lists (MAP-STATIONS).
    pub(crate) fn station_counts(&self) -> Vec<(i64, i64)> {
        let mut by_system = self
            .content()
            .get::<crate::stations::Table>()
            .count_by_system();
        let in_table: Vec<i64> = self
            .content()
            .get::<crate::stations::Table>()
            .iter()
            .map(|(id, _)| id)
            .collect();
        for s in self.stations.values() {
            if !in_table.contains(&s.station_id)
                && !crate::structure_dock::is_structure(self, s.station_id)
            {
                *by_system.entry(s.solar_system_id).or_insert(0) += 1;
            }
        }
        by_system.into_iter().collect()
    }

    /// The `GetAllInfo` reply: a primed character and nothing else.
    ///
    /// All six attributes must **exist** -- `Prime` reads each of them -- but
    /// five of them may be falsy, which skips the corresponding
    /// `PrimeLocation`. Only `charInfo` carries anything.
    ///
    /// `shipState` is read unconditionally and its *value* is irrelevant here:
    /// `MakeShipActive` is pooled onto another tasklet and `_MakeShipActive`
    /// opens with `if shipID is None: ... return`, so with no `shipid` in the
    /// session it never unpacks it. (The note in `docs/archive/entering-the-game.md` that
    /// a `None` `shipState` triggers a nested `ship.Board` is true only when
    /// there *is* a ship: that fallback sits after the early return.)
    fn all_info(&self, actor: Option<i64>) -> PyValue {
        // **Read once.** The two halves below used to take
        // `characters.first()` separately, which was two lookups for one
        // answer -- and two lookups for one answer is how the halves drift.
        let who = actor.and_then(|id| self.character(id));
        let char_info = match who {
            Some(c) => PyValue::Tuple(vec![
                PyValue::Dict(vec![(
                    self.character_dogma_row(c),
                    PyValue::Int(c.character_id),
                )]),
                // The "brain": `(version, charEffects, shipEffects,
                // structureEffects)`, from `brain::initial`. It must be a
                // **truthy** tuple -- `DogmaLocation.__init__` does
                // `if charBrain: initialBrain = (-1,) + charBrain[1:]`, so the
                // version is replaced anyway. The effect lists carry the skills
                // and implants (`brain::BRAIN_IN_GET_ALL_INFO`, on); later
                // changes reach the client by `OnServerBrainUpdated`
                // (GAP-BRAIN).
                crate::brain::initial(self, c.character_id),
            ]),
            None => PyValue::None,
        };
        // **At a structure's controls** (U6C) the structure is the ship: it
        // is primed as the pilot's `shipInfo` too (`ProcessAllInfo` forces
        // it when `shipid == structureid`), and its dict is the one entry.
        let controlled =
            who.and_then(|c| crate::structure_dock::control::controlling(self, c.character_id));
        let ship_info = match who {
            Some(_) if controlled.is_some() => controlled
                .and_then(|id| self.structure_dogma_row(id).map(|row| (row, id)))
                .map_or(PyValue::None, |(row, id)| {
                    PyValue::Dict(vec![(row, PyValue::Int(id))])
                }),
            // `{itemKey: row}`. `PrimeLocation` takes `.keys()`, reorders the
            // key list, and subscripts back -- so a dict, with one entry for
            // an empty pod.
            //
            // **Not optional once `shipid` is set.** `godma.IsItemReady` is
            // `itemID in self.invitems`, and `invitems` is written only by
            // `UpdateItem`, which only runs for what `shipInfo` primes. Without
            // it: `corvetteButton` spins a 10 Hz tasklet forever waiting for
            // the ship to be ready, and -- worse -- the fitting window's
            // `GetTypeIDForController` gets `None` from `godma.GetItem`, feeds
            // it to `evetypes.GetCategoryID`, and raises
            // `TypeNotFoundException` out of `ApplyAttributes`, which is not
            // wrapped.
            Some(c) => {
                let mut entries = vec![(self.capsule_dogma_row(c), PyValue::Int(c.ship_id))];
                // Every fitted module, or the HUD has no button to click
                // (`docs/re/modules.md` §0.1).
                entries.extend(crate::modules::ship_info_entries(self, c));
                PyValue::Dict(entries)
            }
            None => PyValue::None,
        };
        // **A structure the pilot is docked in is primed first.** The capsule's
        // row names the structure as its location, and godma's `UpdateItem`
        // does `itemsByLocationID[location].add(..)` for any location that is
        // not a system or a station, so an unprimed structure is a `KeyError`
        // out of `PrimeLocation` (every dock, undock and login at an Upwell
        // structure). A station stays `None`: `IsStation` skips that line.
        let structure_info = match actor.and_then(|a| crate::structure_dock::dogma_host(self, a)) {
            Some(id) => match self.structure_dogma_row(id) {
                Some(row) => PyValue::Dict(vec![(row, PyValue::Int(id))]),
                None => PyValue::None,
            },
            None => PyValue::None,
        };
        keyval(vec![
            ("structureInfo", structure_info),
            ("shipInfo", ship_info),
            ("charInfo", char_info),
            ("shipModifiedCharAttribs", PyValue::None),
            (
                "systemWideEffectsOnShip",
                actor.map_or(PyValue::None, |a| crate::system_effect::info(self, a)),
            ),
            // **`UNPACK_SEQUENCE 4`, unconditionally.** `_MakeShipActive`
            // does
            //
            //     self.instanceCache, self.instanceFlagQuantityCache,
            //     self.wbData, heatStates = shipState
            //
            // with no guard, so `None` is a `TypeError` -- and worse, the
            // `shipState is None` branch *above* it fires a nested
            // `PerformSelectiveSessionChange('board', ...)` into
            // `ship.Board`, which mutates the session and sets an undock
            // cooldown nobody asked for. Sending the tuple is the same data
            // by a shorter route: `PerformSessionChange` just returns
            // `func(*args)`, so `Board`'s return value *is* this tuple.
            //
            // The four slots, each matching the client's own reset in
            // `ClearInstanceCache`:
            //   instanceCache              {}   -- `GetInstance` rebuilds any
            //                                   miss locally from godma, and
            //                                   `baseDogmaItem.Load` ignores
            //                                   zero values, so an empty cache
            //                                   and a zero-filled one behave
            //                                   identically.
            //   instanceFlagQuantityCache  the loaded charges,
            //                                   `{shipID: {flagID: row}}`
            //                                   (`crate::modules::
            //                                   instance_flag_quantity_cache`
            //                                   has the evidence; P5.12)
            //   wbData                     None -- `SetWeaponBanks` opens
            //                                   `if data is None: return`.
            //   heatStates                 {}   -- stored and never read back;
            //                                   one STORE_FAST, no LOAD_FAST.
            (
                "shipState",
                crate::weapon_bank::ship_state(
                    self,
                    controlled.or(who.map(|c| c.ship_id)).unwrap_or(0),
                ),
            ),
        ])
    }

    /// The capsule as dogma sees it, and as the hangar lists it.
    ///
    /// Same `ItemGetInfo` shape as the character row, over the same item
    /// descriptor -- that identity is load-bearing. `godma.invitems` is a
    /// rowset built on `invCache.GetItemHeader()`, which is our own
    /// `invbroker.GetItemDescriptor()`, so `row.invItem` must be packed
    /// against exactly that 10-column layout.
    ///
    /// `UpdateItem` has a gauntlet the pod has to pass, and one of its gates
    /// is easy to miss: **`singleton` must be truthy.** The test is
    ///
    /// ```python
    /// if (isSubLocation or not item.singleton) and (itemCategoryID != const.categoryCharge or ...):
    ///     return
    /// ```
    ///
    /// so a `singleton` of 0 drops the capsule at `Godma:UpdateItem.skip3`,
    /// silently, and it never gets attributes. `categoryShip` is in the
    /// accepted category tuple and `flagHangar` is not one of the skipped
    /// flags, so the rest passes.
    ///
    /// **`attributes` is not optional, and believing it was cost a player
    /// three empty health rings.** The note here used to say the pod's real
    /// attributes come from FSD static data and that an empty dict was
    /// therefore fine. They do not. `godma.StateManager.GetAttribute`'s miss
    /// path (`:2224-2234`) falls back to
    /// `self.attributesByName[attrKey].defaultValue` -- the **attribute
    /// definition's** default, from `dogmaattributes` -- and **never looks at
    /// the type's base value in `typedogma`** **[V]**. So every attribute the
    /// HUD reads is `0.0` unless it is on this wire, and
    /// `activeShipController.GetShieldHPPortion` and friends each catch the
    /// resulting `ZeroDivisionError` and return `0.0`, which draws an empty
    /// gauge. Three rings, all empty, on an undamaged ship.
    ///
    /// Note that this is a **different** gauge family from the Destiny blob's
    /// `damageState` (`crate::destiny::DamageState`), which feeds the target
    /// and selected-item bars. Nothing under `shipHud/` reads `damageState`
    /// at all, so the two had to be fixed separately and the first fix did
    /// not touch this.
    ///
    /// Keyed by **integer attributeID**, values **floats**: the client does
    /// the id-to-name translation itself in
    /// `StateManager.RefreshItemAttributes` (`:2026-2032`), which iterates
    /// `info.attributes.iteritems()` and calls
    /// `dogma.data.get_attribute_name(attributeID)` **[V]**.
    ///
    /// **Every value must be a float.** `activeShipController` has no
    /// `CO_FUTURE_DIVISION` -- `co_flags` has bit `0x2000` clear -- so
    /// `GetArmorHPPortion` compiles to `BINARY_DIVIDE` **[V]**. With integers
    /// that is Python 2 floor division: a full ship reads `156/156 == 1` and
    /// looks perfect, while a damaged one reads `106/156 == 0` and draws an
    /// **empty** ring. An `Int` here is a bug that only appears in combat,
    /// and only on armour and structure -- the shield's charged path returns
    /// a float from `math.exp` and is accidentally immune.
    ///
    /// `shieldCharge` (264) and `charge` (18) are **instance state**, not
    /// type data: neither has a base value on any hull, and on the real
    /// server they are two of the seven columns of the dogma instance row
    /// (`baseDogmaLocation.__init__:181`) **[V]**. Seeded to the capacity, so
    /// a ship undocks full. That is also the recharge formula's fixed point
    /// -- at `value == capacity` the exponential term drops out of
    /// `GetChargeValue` entirely **[V]** -- which is what makes `time: None`
    /// safe with a charged attribute in play. **[I]** that the real server
    /// seeds them full; the client cannot tell, and the formula converges to
    /// the cap either way.
    ///
    /// `activeEffects` stays empty, which keeps `info.time` out of
    /// `RefreshItemEffects` (`:1999`) and so keeps `time: None` safe there
    /// too.
    ///
    /// **A pod in space does not name a structure.** Its stored location is
    /// the structure it undocked from, which godma has not primed once the
    /// pilot is out (`UpdateItem` raises `KeyError` on it), so the row sent
    /// says the pilot's system. The stored item is not moved.
    pub(crate) fn capsule_dogma_row(&self, c: &Character) -> PyValue {
        keyval(vec![
            ("itemID", PyValue::Int(c.ship_id)),
            (
                "invItem",
                match self.item(c.ship_id) {
                    Some(i) => {
                        let outside = crate::structure_dock::is_structure(self, i.location_id)
                            && crate::structure_dock::dogma_host(self, c.character_id)
                                != Some(i.location_id);
                        match self.present_system(c.character_id).filter(|_| outside) {
                            Some(system) => {
                                let mut moved = i.clone();
                                moved.location_id = system;
                                PyValue::DbRow(Box::new(self.item_row(&moved)))
                            }
                            None => PyValue::DbRow(Box::new(self.item_row(i))),
                        }
                    }
                    // Unreachable while `Registry::new` seeds the capsule,
                    // and `None` here would be an `AttributeError` inside
                    // `UpdateItem` rather than a missing pod. Kept explicit
                    // so the day the capsule stops being seeded is a visible
                    // change rather than a silent one.
                    None => PyValue::None,
                },
            ),
            // Modified by the ship's dogma when there is a sheet, so the
            // HUD's rings and capacitor are drawn against the fitted maxima
            // the server's combat uses.
            (
                "attributes",
                with_instance_state(
                    self,
                    c,
                    with_fit_state(
                        self,
                        c.ship_id,
                        with_stance_switch_time(
                            self.content(),
                            c.ship_type_id,
                            match crate::dogma::hull(self, c.ship_id) {
                                Some(h) => ship_attributes_of(&h),
                                None => ship_attributes(self.content(), c.ship_type_id),
                            },
                        ),
                    ),
                ),
            ),
            ("activeEffects", PyValue::Dict(vec![])),
            // `UpdateAttribute` falls back to `blue.os.GetSimTime()` when
            // `time` is None, and `_IsAttributeChangeRelevant` returns True
            // for a falsy `wallclockTime` -- so both may be None.
            ("time", PyValue::None),
            ("wallclockTime", PyValue::None),
        ])
    }

    /// Every item this character owns, anywhere, as item rows.
    ///
    /// **This exists so that two lanes cannot disagree about one shape.**
    /// The assets window spans two services -- `charMgr.ListIncludingContainers`
    /// and `invbroker.GetContainerContents` -- and `invCache` merges their
    /// answers into one cache keyed by `realItem.itemID`:
    ///
    /// ```text
    ///     realItems = self.moniker.ListIncludingContainers()
    ///     for realItem in realItems:
    ///         self.cachedItemsAll[realItem.itemID] = realItem
    /// ```
    ///
    /// So the rows must carry `itemID` **as an attribute**, which means
    /// `blue.DBRow` with the `item_columns()` layout, which is already what
    /// `List` answers with. Either service answering a different shape would
    /// fill the window with half of one and half of the other, and the two
    /// halves are written in different files by different people.
    /// `docs/archive/plan-docked-subsystems.md` calls this the round's one
    /// cross-lane contract.
    ///
    /// Today the character owns exactly one thing: the capsule they are
    /// sitting in. The signature is already the one the item store will want,
    /// so the callers do not change when the store lands
    /// (`docs/reference/world-state.md`).
    pub fn owned_item_rows(&self, actor: Option<i64>) -> Vec<PyValue> {
        let Some(c) = actor.and_then(|id| self.character(id)) else {
            return Vec::new();
        };
        self.store
            .owned_by(c.character_id)
            .map(|i| PyValue::DbRow(Box::new(self.item_row(i))))
            .collect()
    }

    /// What is in the character's station hangar.
    ///
    /// Narrower than [`Self::owned_item_rows`] on purpose: that one is the
    /// cross-station query the assets window merges, and this one is a single
    /// container's contents. They share `item_row`, which is the part the
    /// cross-lane contract is actually about -- `invCache.ListIncludingContainers`
    /// keys both answers into one cache by `itemID`, so the *shape* must
    /// match even where the sets do not.
    pub fn hangar_item_rows(&self, actor: Option<i64>) -> Vec<PyValue> {
        let Some(c) = actor.and_then(|id| self.character(id)) else {
            return Vec::new();
        };
        let station = c.station_id.unwrap_or(0);
        // The station's index, not every item in the world (PERF-HOLDS);
        // the same rows in the same (store) order.
        self.store
            .contents_of(station)
            .filter(|i| {
                i.owner_id == c.character_id
                    && i.location_id == station
                    && i.flag_id == crate::model::FLAG_HANGAR
            })
            .map(|i| PyValue::DbRow(Box::new(self.item_row(i))))
            .collect()
    }

    /// `{capacity, used}` for a container, in cubic metres.
    ///
    /// **The shape is the client's own.** `invControllers.py:69` builds its
    /// zero sentinel as `Row(['capacity', 'used'], [0, 0.0])`, and every
    /// consumer reads `.capacity` and `.used` by attribute -- 64 attribute
    /// reads, 36 arithmetic uses and 28 comparisons across the tree, almost
    /// all of them `capacity - used`. A `KeyVal` satisfies attribute access
    /// and is whitelisted; nothing on this path needs a real `Row`.
    ///
    /// **This was unanswered, and the census summary made it look cheap.** It
    /// said `unpack_2`, which would have been a two-tuple. The detail said
    /// otherwise, and the `is None` caller guards at seven sites mean a wrong
    /// answer is a live re-ask on the inventory draw path rather than a
    /// visible error -- which is why it was parked rather than guessed at.
    ///
    /// The capsule's capacity really is `0.0`: a pod has no cargo hold.
    /// **The station hangar's is [I] policy** -- see
    /// `STATION_HANGAR_CAPACITY`.
    ///
    /// The one answer deliberately *not* given is the client's own escape
    /// hatch: `BaseInvContainer.GetCapacity` substitutes its zero sentinel
    /// for a `RuntimeError` whose `args[0]` is `CharacterNotAtStation` or
    /// `FakeItemNotFound`. Raising one of those would be a lie -- the
    /// character *is* at the station -- and worse than the truth, because a
    /// capacity of zero makes `HasEnoughSpaceForItems` refuse everything.
    pub fn container_capacity(&self, actor: Option<i64>, container: Option<i64>) -> PyValue {
        let station = actor
            .and_then(|id| self.character(id))
            .and_then(|c| c.station_id)
            .unwrap_or(0);
        let (capacity, used) = match container {
            // The station hangar: effectively unlimited, holding whatever is
            // in it.
            Some(CONTAINER_HANGAR) => (
                STATION_HANGAR_CAPACITY,
                self.store
                    .used_volume_in(&self.content, station, crate::model::FLAG_HANGAR),
            ),
            // A real item's own inventory -- the capsule, today. Its capacity
            // is the type's, and a pod's is zero.
            Some(id) => {
                let capacity = self
                    .item(id)
                    .and_then(|i| crate::item::find_in(&self.content, i.type_id))
                    .map_or(0.0, |t| t.capacity);
                (
                    capacity,
                    self.store
                        .used_volume_in(&self.content, id, crate::model::FLAG_HANGAR),
                )
            }
            None => (0.0, 0.0),
        };
        keyval(vec![
            ("capacity", PyValue::Real(capacity)),
            ("used", PyValue::Real(used)),
        ])
    }

    /// One item's row, as the push path needs it.
    ///
    /// Public only because `notify::ItemChange` is packed here and read in
    /// `dispatch.rs`, where the session and the node id live -- an arm still
    /// cannot send anything.
    pub fn item_row_value(&self, i: &crate::item::Item) -> PyValue {
        PyValue::DbRow(Box::new(self.item_row(i)))
    }

    /// One item by id, wherever it is.
    pub fn item(&self, item_id: i64) -> Option<&crate::item::Item> {
        self.store.items().iter().find(|i| i.item_id == item_id)
    }

    /// A structure as godma's `UpdateItem` takes it: the same `KeyVal` shape
    /// as [`Self::capsule_dogma_row`], with the structure type's attributes
    /// and an `invItem` located in its system (`categoryStructure` makes
    /// `UpdateItem` create the location's item set). The store item is used
    /// when there is one; a structure in space need not have one, so it is
    /// built from the structure row otherwise. `None` for a gone structure.
    fn structure_dogma_row(&self, id: i64) -> Option<PyValue> {
        let s = crate::structure::get(self.deltas(), id)?;
        let item = match self.item(id) {
            Some(i) => {
                let mut i = i.clone();
                i.location_id = s.system;
                i
            }
            None => crate::item::Item {
                item_id: id,
                type_id: s.type_id,
                owner_id: s.owner_corp,
                location_id: s.system,
                flag_id: 0,
                quantity: 1,
                group_id: crate::structure_profile::group_of(s.type_id),
                category_id: crate::structure_profile::CATEGORY_STRUCTURE,
                singleton: true,
            },
        };
        Some(keyval(vec![
            ("itemID", PyValue::Int(id)),
            ("invItem", PyValue::DbRow(Box::new(self.item_row(&item)))),
            ("attributes", ship_attributes(self.content(), s.type_id)),
            ("activeEffects", PyValue::Dict(vec![])),
            ("time", PyValue::None),
            ("wallclockTime", PyValue::None),
        ]))
    }

    /// The capsule's item row, shared by the hangar listing and dogma.
    ///
    /// Every field is checked somewhere:
    ///
    /// - `categoryID == categoryShip (6)` is what `GetShipItemFromHangar`
    ///   matches on, and what partitions the station's Ship Hangar from its
    ///   Item Hangar (`StationItems.IsItemHere` excludes `categoryShip`;
    ///   `StationShips.IsItemHere` requires it).
    /// - `flagID == flagHangar (4)`, because the active ship is listed in the
    ///   ordinary hangar. `flagPilot (57)` is **not** this row's flag -- it is
    ///   applied client-side by `_OnCharacterEmbarkation` to the *character*
    ///   inside the pod, and `characterDogmaItem.SetLocation` raises if it is
    ///   anything else.
    /// - `locationID == stationID` and `ownerID == charID` are both tested by
    ///   `ProcessInventoryChange` and by every `IsItemHere`.
    /// - `singleton` must be truthy, for `UpdateItem` and for `ShipPicker`.
    pub(crate) fn item_row(&self, i: &crate::item::Item) -> DbRow {
        DbRow::new(
            item_row_descriptor(),
            vec![
                DbValue::Int(i.item_id),
                DbValue::Int(i.type_id),
                DbValue::Int(i.owner_id),
                DbValue::Int(i.location_id),
                DbValue::Int(i.flag_id),
                DbValue::Int(i.quantity),
                DbValue::Int(i.group_id),
                DbValue::Int(i.category_id),
                // `customInfo`, index 8. NULL rather than a value: an
                // ordinary item has no custom info, and every consumer of
                // this column reads it only as a descriptor key.
                DbValue::Null,
                // `stacksize` and `quantity` carry the same number. They are
                // separate columns in CCP's layout and we have never seen
                // them disagree; if something ever reads them apart, this is
                // the line that was guessing.
                DbValue::Int(i.quantity),
                DbValue::Int(i.singleton as i64),
            ],
        )
        .expect("the item row matches the item descriptor")
    }

    /// One character as dogma sees it: an `ItemGetInfo`-shaped `KeyVal`.
    ///
    /// Every attribute here is read by `UpdateItem` or `RefreshItemAttributes`
    /// and a `KeyVal` raises `AttributeError` on a missing one, so none is
    /// optional.
    ///
    /// Two values are load-bearing in a way that is easy to miss:
    ///
    /// - **`groupID` must be `groupCharacter` (1).** `UpdateItem` drops
    ///   anything whose `categoryID` is not one of eleven item categories
    ///   *unless* `groupID == groupCharacter`, and a character's category is
    ///   `categoryOwner`, which is not in that list. The group check is the
    ///   only reason the row survives at all.
    /// - **`singleton` must be truthy.** `skip3` is
    ///   `if (isSubLocation or not item.singleton) and (categoryID !=
    ///   categoryCharge or groupID == groupMine): return`, and for a
    ///   character the right-hand side is true -- so a falsy `singleton`
    ///   returns early and no attributes are ever registered. The row still
    ///   lands in `invitems` (that happens before the guard), so `Prime`
    ///   completes and the only symptom is a `Godma:UpdateItem.skip3` line
    ///   in the client's log and every attribute falling back to its default.
    ///
    /// `flagID` must not be `flagCargo`, `flagDroneBay` or a fighter tube;
    /// `flagNone` is 0. `time` and `wallclockTime` may be None -- the former
    /// becomes `blue.os.GetSimTime()`, the latter makes every change count.
    fn character_dogma_row(&self, c: &Character) -> PyValue {
        let station = c.station_id.unwrap_or(0);
        let row = DbRow::new(
            item_row_descriptor(),
            vec![
                DbValue::Int(c.character_id),
                DbValue::Int(CHARACTER_TYPE_ID),
                DbValue::Int(c.character_id),
                DbValue::Int(station),
                DbValue::Int(0),
                DbValue::Int(1),
                DbValue::Int(GROUP_CHARACTER),
                DbValue::Int(CATEGORY_OWNER),
                // `customInfo`, index 8. NULL rather than a value: an
                // ordinary item has no custom info, and every consumer of
                // this column reads it only as a descriptor key.
                DbValue::Null,
                DbValue::Int(1),
                DbValue::Int(1),
            ],
        )
        .expect("the character row matches the item descriptor");

        keyval(vec![
            ("itemID", PyValue::Int(c.character_id)),
            // Must be a real `blue.DBRow`, not another KeyVal:
            // `ItemWrapper.__getattr__` tests
            // `attribute in d['dbrow'].__columns__`, and `__columns__` is a
            // native DBRow member. A KeyVal there turns every attribute
            // lookup into an `AttributeError` raised from inside
            // `__getattr__`.
            ("invItem", PyValue::DbRow(Box::new(row))),
            // The five attributes `Prime` scatters. Keys must be FSD-known
            // attribute ids. Supplying them is optional -- `GetAttribute`
            // falls back to `attributesByName[name].defaultValue` from FSD --
            // but it is cheap, and it avoids the fallback's
            // `evetypes.GetCategoryID(typeID)` lookup entirely. The values
            // are `character::skill_attribute`'s, the one function
            // `GetAttributes` and the training clock also read.
            (
                "attributes",
                PyValue::Dict(
                    [
                        ATTR_CHARISMA,
                        ATTR_INTELLIGENCE,
                        ATTR_MEMORY,
                        ATTR_PERCEPTION,
                        ATTR_WILLPOWER,
                    ]
                    .into_iter()
                    .map(|a| {
                        (
                            PyValue::Real(crate::skill_queue::client_attribute(
                                self,
                                c.character_id,
                                a,
                            )),
                            PyValue::Int(a),
                        )
                    })
                    .collect(),
                ),
            ),
            ("activeEffects", PyValue::Dict(vec![])),
            ("time", PyValue::None),
            ("wallclockTime", PyValue::None),
        ])
    }

    /// The ids a `GetMulti*Ex` call is asking about.
    ///
    /// The argument is one list of keys. Anything else is a shape we do not
    /// know, and an empty result is the safe reading: it teaches the client
    /// that we have nothing, rather than hanging it.
    fn requested_ids(req: &Request<'_>) -> Vec<i64> {
        match req.args.first() {
            Some(PyValue::List(v)) | Some(PyValue::Tuple(v)) => v
                .iter()
                .filter_map(|x| match x {
                    PyValue::Int(i) => Some(*i),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// `config.GetMultiOwnersEx(ids)`.
    ///
    /// Two kinds of owner exist in this sandbox: the characters themselves
    /// and the one corporation they belong to.
    ///
    /// The `typeID` column is **[I]**. It is the owner's item type -- `2` for
    /// a corporation and a character bloodline type for a character -- and
    /// the client uses it to tell the two apart. The values here are EVE's
    /// long-standing ones but are not confirmed from this build, and nothing
    /// on the select screen appears to read them; the names are what it
    /// draws. Worth pinning down if an owner ever renders as the wrong kind
    /// of thing.
    fn multi_owners(&self, req: &Request<'_>) -> PyValue {
        const TYPE_CORPORATION: i32 = 2;
        const TYPE_CHARACTER: i32 = 1373;

        let wanted = Self::requested_ids(req);
        let mut owners: Vec<Owner> = Vec::new();
        for id in wanted {
            if let Some(c) = self.config.characters.iter().find(|c| c.character_id == id) {
                owners.push(Owner::new(id as i32, c.name.clone(), TYPE_CHARACTER));
            } else if let Some(name) = corp::player_corp_name(self, id) {
                // Before the NPC branch: a player corp's members *are*
                // characters whose `corporation_id` is this id, and that
                // branch would name it after the NPC corp.
                owners.push(Owner::new(id as i32, name, TYPE_CORPORATION));
            } else if let Some(name) = crate::alliance::name_of(self.deltas(), id) {
                // SOC-C1: `typeAlliance`, which is how a link or the info
                // window knows the id is an alliance.
                owners.push(Owner::new(
                    id as i32,
                    name,
                    crate::alliance::TYPE_ALLIANCE as i32,
                ));
            } else if self
                .config
                .characters
                .iter()
                .any(|c| c.corporation_id == id)
            {
                owners.push(Owner::new(
                    id as i32,
                    self.config.corporation_name.clone(),
                    TYPE_CORPORATION,
                ));
            }
        }
        cfg::owners_header_data(&owners).to_pyvalue()
    }

    /// `config.GetMultiCorpTickerNamesEx(ids)`.
    ///
    /// Only `corporationID` and `tickerName` are ever read
    /// (`CrpTickerNames.__getattr__`); the shape and colour columns exist in
    /// the header and go out as `None`.
    fn multi_corp_tickers(&self, req: &Request<'_>) -> PyValue {
        let wanted = Self::requested_ids(req);
        let tickers: Vec<(i32, String)> = wanted
            .into_iter()
            .filter_map(|id| {
                // A player corp's own ticker first; it is also the string
                // `corp::corporation_row` sends for it.
                if let Some(t) = corp::player_corp_ticker(self, id) {
                    return Some((id as i32, t));
                }
                self.config
                    .characters
                    .iter()
                    .any(|c| c.corporation_id == id)
                    // The same string `corp::corporation_row` sends as
                    // `tickerName`. Two literals would be two tickers on
                    // screen for one corporation, depending on which panel
                    // drew it.
                    .then(|| (id as i32, corp::CORP_TICKER.to_owned()))
            })
            .collect();
        cfg::corp_ticker_header_data(&tickers).to_pyvalue()
    }

    /// `config.GetMultiLocationsEx(ids)`.
    ///
    /// Stations and solar systems a character might be sitting in. The
    /// coordinates are zero, which is true of a sandbox with no map rather
    /// than a placeholder.
    fn multi_locations(&self, req: &Request<'_>) -> PyValue {
        let wanted = Self::requested_ids(req);
        let mut locations: Vec<Location> = Vec::new();
        for id in wanted {
            let chars = &self.config.characters;
            if chars.iter().any(|c| c.station_id == Some(id)) {
                locations.push(
                    Location::new(id, self.config.station_name.clone()).at(
                        chars
                            .iter()
                            .find(|c| c.station_id == Some(id))
                            .and_then(|c| c.solar_system_id)
                            .unwrap_or(0),
                        0.0,
                        0.0,
                        0.0,
                    ),
                );
            } else if chars.iter().any(|c| c.solar_system_id == Some(id)) {
                locations.push(
                    Location::new(id, self.config.solar_system_name.clone()).at(id, 0.0, 0.0, 0.0),
                );
            } else if let Some(c) = chars.iter().find(|c| c.ship_id == id) {
                // The capsule needs a location row, and omitting it is not
                // the usual harmless gap. `cfg.evelocations` is a
                // `cfg.Recordset`, and `Recordset.Get` **raises** on a miss:
                //
                //     if key in self.data: return self.rowclass(self, key)
                //     raise KeyError('RecordNotFound', key)
                //
                // The fitting window's `ConstructLayout` ends in
                // `UpdateShipName` -> `GetFitName` -> `cfg.evelocations.Get
                // (itemID)`, and `invCtrl.ShipCargo.GetName` does the same
                // with no guard at all. (`_Prime` does remember a missing key
                // in `knownLuzers` and stop asking -- but the first `Get`
                // still raises, so that caching does not save us.)
                locations.push(Location::new(id, "Capsule".to_owned()).at(
                    c.station_id.unwrap_or(0),
                    0.0,
                    0.0,
                    0.0,
                ));
            }
        }
        // U3: an Upwell structure's name and place.
        crate::structure_space::overlay(self, &Self::requested_ids(req), &mut locations);
        // GAPS-2: the names pilots gave their ships and containers.
        crate::item_names::overlay(self, &Self::requested_ids(req), &mut locations);
        cfg::locations_header_data(&locations).to_pyvalue()
    }

    /// The station, as an item row: what the hangar inventory *is*.
    ///
    /// See the `GetSelfInvItem` arm for why the station is the right answer.
    /// `groupID` is `groupStation` (15) -- the same constant the client puts
    /// in its own `invbroker` bind params, `(stationID, 15)` -- and
    /// `categoryID` is `categoryStation` (3).
    fn station_self_item(&self, actor: Option<i64>) -> PyValue {
        let Some(c) = actor.and_then(|id| self.character(id)) else {
            return PyValue::None;
        };
        // **An Upwell structure is not a station** (U4): its own group and
        // category, or `GetClassForItem` builds the wrong dogma item.
        let (group, category) = if c
            .station_id
            .is_some_and(|id| crate::structure_dock::is_structure(self, id))
        {
            let t = crate::item::find_in(&self.content, c.station_type_id);
            (
                t.map_or(crate::structure_profile::group_of(c.station_type_id), |t| {
                    t.group_id
                }),
                crate::structure_profile::CATEGORY_STRUCTURE,
            )
        } else {
            (GROUP_STATION, CATEGORY_STATION)
        };
        let row = DbRow::new(
            item_row_descriptor(),
            vec![
                DbValue::Int(c.station_id.unwrap_or(0)),
                DbValue::Int(c.station_type_id),
                DbValue::Int(c.station_owner_id),
                // A station's own location is the solar system, not itself.
                DbValue::Int(c.solar_system_id.unwrap_or(0)),
                DbValue::Int(0),
                DbValue::Int(1),
                DbValue::Int(group),
                DbValue::Int(category),
                // `customInfo`, index 8. NULL rather than a value: an
                // ordinary item has no custom info, and every consumer of
                // this column reads it only as a descriptor key.
                DbValue::Null,
                DbValue::Int(1),
                DbValue::Int(1),
            ],
        )
        .expect("the station row matches the item descriptor");
        PyValue::DbRow(Box::new(row))
    }

    fn no_method(&self, req: &Request<'_>) -> ServiceError {
        ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }
    }

    /// A human-readable summary for the log, so an unanswered call is visible
    /// without turning on a debugger.
    /// The diagnostic line an `UNANSWERED` log entry is built from.
    ///
    /// **The actor is in it because without one the line cannot be
    /// attributed.** With two players connected, `charMgr.GetPublicInfo/1`
    /// twice in a log is either one player asking twice or two players asking
    /// once, and those want different fixes.
    pub fn describe(req: &Request<'_>) -> String {
        match req.actor {
            Some(id) => format!("{}.{}/{} as {id}", req.service, req.method, req.args.len()),
            None => format!(
                "{}.{}/{} with no character",
                req.service,
                req.method,
                req.args.len()
            ),
        }
    }
}

/// The item row descriptor, as `invbroker.GetItemDescriptor` returns it.
///
/// **Provenance, because this one is reconstructed rather than read off the
/// wire.** The real descriptor comes from the server's own database schema,
/// so it is not in the client and no capture has carried one. The columns
/// here are the client's *own* idea of an item row, taken from
/// `eveAssets/assetSearching.GetFakeRowset`, which synthesises item rows for
/// asset search:
///
/// ```python
/// blue.DBRowDescriptor((('itemID',     const.DBTYPE_I8),
///                       ('typeID',     const.DBTYPE_I4),
///                       ('ownerID',    const.DBTYPE_I4),
///                       ('groupID',    const.DBTYPE_I4),
///                       ('categoryID', const.DBTYPE_I4),
///                       ('quantity',   const.DBTYPE_I4),
///                       ('singleton',  const.DBTYPE_I4),
///                       ('stacksize',  const.DBTYPE_I4),
///                       ('locationID', const.DBTYPE_I8),
///                       ('flagID',     const.DBTYPE_I2),
///                       ('stationID',  const.DBTYPE_I8)))
/// ```
///
/// That is the best evidence available and it is the client's, not ours — but
/// it is still **[I]**, not **[V]**: it is what the client builds when it
/// fabricates item rows, not necessarily what CCP's `invbroker` returns.
///
/// **That revisit has happened, and the descriptor was wrong.** The sentence
/// here used to read "we ship no items, so nothing reads these columns yet" --
/// which was already false when written: the capsule and the station's own
/// item both cross the wire on the docked path. `customInfo` was missing at
/// index 8, so `stacksize` and `singleton` sat one slot low and the client's
/// hard-coded `ixSingleton` (10) pointed past the end of the row. See the
/// assertion in `tests/loopback.rs` for how the indices come from two
/// different files and why only one of them was found the first time.
///
/// **`GetItemDescriptor` is not the row descriptor.** It answers CCP's shape:
/// the nine physical columns (itemID .. customInfo) plus `stacksize` and
/// `singleton` as *virtual* columns, carried as the third element of the
/// `__reduce__` triple (the state list `[(name, function), ..]`, the two
/// functions being the whitelisted `eveCfg.StackSize` and `eveCfg.Singleton`).
/// Godma builds both charge-row descriptors from `itemrd`: `subloc_internalrd`
/// drops the `dbtype == 0` columns and keeps a physical `stacksize` it never
/// fills (the HUD's `charge.stacksize` reads `None`, the label 0, Ctrl+R
/// raises), while `sublocrd` copies `itemrd.virtual`, so with a virtual
/// `stacksize` the `OnItemChange` row's count resolves to its `quantity`.
/// With the virtual column the `ItemWrapper` falls through to the live
/// attribute 805. Every row we send stays on the eleven-physical-column
/// [`item_row_descriptor`]: a virtual `Singleton` reads a negative quantity
/// and we do not encode singletons that way.
///
/// - **[A1]** the third header element is the descriptor's `__setstate__`
///   state and recreates the two virtual columns after the physical ones.
///   Falsifier: login hangs at `invCache.GetItemHeader`, or the client log
///   shows "Internal error in VirtualGet" or a traceback in `godma.sublocrd`.
/// - **[A2]** `len(itemrd)` counts the virtual columns (11), so
///   `invCache.List`'s `len(item) != len(header)` still accepts our
///   eleven-column rows. Falsifier: empty hangar/cargo and 'Invalid Item'.
///   (Retail rows are indexed at `ixSingleton = 10`, which supports 11.)
/// - **[A3]** a `(name, function)` 2-tuple is the whole entry format.
///   Falsifier: the same symptoms as A1.
pub fn item_descriptor() -> PyValue {
    let physical = DbRowDescriptor::new(item_columns()[..ITEM_PHYSICAL_COLUMNS].to_vec())
        .expect("the physical item columns are a valid layout");
    let PyValue::ObjectEx {
        form,
        header,
        list,
        dict,
    } = physical.to_pyvalue()
    else {
        unreachable!("a descriptor is an ObjectEx");
    };
    let PyValue::Tuple(mut h) = *header else {
        unreachable!("a descriptor header is a tuple");
    };
    // Order matters: stacksize then singleton land at ixStackSize 9 and
    // ixSingleton 10.
    h.push(PyValue::List(
        ITEM_VIRTUAL_COLUMNS
            .iter()
            .map(|(name, global)| {
                PyValue::Tuple(vec![
                    PyValue::Bytes(name.as_bytes().to_vec()),
                    PyValue::Global(global.as_bytes().to_vec()),
                ])
            })
            .collect(),
    ));
    PyValue::ObjectEx {
        form,
        header: Box::new(PyValue::Tuple(h)),
        list,
        dict,
    }
}

/// Columns of the item descriptor that are physical in CCP's answer
/// (itemID .. customInfo).
const ITEM_PHYSICAL_COLUMNS: usize = 9;

/// CCP's two virtual item columns, in index order (9, 10), each computed by
/// a whitelisted client function.
const ITEM_VIRTUAL_COLUMNS: [(&str, &str); 2] = [
    ("stacksize", "eve.common.script.sys.eveCfg.StackSize"),
    ("singleton", "eve.common.script.sys.eveCfg.Singleton"),
];

/// The one descriptor every item row is built against: the capsule and
/// station rows, `OnItemChange`, the dogma prime's `invItem`, hangar and
/// contents lists. `GetItemDescriptor` answers with the same value, so the
/// client's `itemrd` and every row we send cannot disagree.
pub fn item_row_descriptor() -> DbRowDescriptor {
    DbRowDescriptor::new(item_columns()).expect("the item descriptor's columns are a valid layout")
}

/// The item row's columns, in **CCP's order**.
///
/// This order is confirmed, and it is not the one we shipped first. Two
/// places in `godma.pyc` build a row against this very descriptor
/// (`godma.itemrd`, which *is* `invbroker.GetItemDescriptor()`'s return) with
/// a hard-coded value list, and both agree:
///
/// ```text
/// OnGodmaPrimeItem:  itemID, typeID, ownerID, locationID, flag,
///                    quantity, groupID, categoryID, stackSize     (BUILD_LIST 9)
/// GetAttribute:      itemKey, typeID, ownerID, locationID, flagID,
///                    quantity, GetGroupID(typeID), categoryID, None (BUILD_LIST 9)
/// ```
///
/// and `carbon/common/lib/const.pyc` independently pins the first six by
/// index: `ixItemID=0, ixTypeID=1, ixOwnerID=2, ixLocationID=3, ixFlag=4,
/// ixQuantity=5`.
///
/// Our first version of this was reconstructed from
/// `assetSearching.GetFakeRowset` and marked **[I]**; it had eleven columns
/// in a different order. It worked anyway, because the client takes the
/// descriptor *from us* and builds rows against it — which is exactly why a
/// wrong order could sit here unnoticed, and why it is worth fixing now
/// rather than when something finally reads a row positionally.
///
/// **One deliberate deviation, in the rows only.** CCP's descriptor has nine
/// physical columns and two virtual ones (`stacksize`, `singleton`); our rows
/// carry all eleven as physical columns, because `eve-marshal` cannot emit
/// virtual ones in a row (`docs/reference/dbrow.md` Gap 4) and a virtual
/// `Singleton` would read 0 for every module (retail encodes a singleton as a
/// negative quantity). `GetItemDescriptor` itself answers CCP's shape, see
/// [`item_descriptor`]; the client's `itemrd` therefore counts eleven columns
/// either way, which is what the hard-coded indices require.
///
/// The remaining cost: a client-side `blue.DBRow(itemrd, [nine values])` such
/// as `GetAttribute`'s `invItem` fallback fills the virtual slots itself, and
/// `OnGodmaPrimeItem`'s nine-value row (sent by `modules::PrimeCharge` on the
/// sublocation branch, which uses `sublocrd`) is fine against it.
fn item_columns() -> Vec<Column> {
    [
        ("itemID", DbType::I8),
        ("typeID", DbType::I4),
        ("ownerID", DbType::I4),
        ("locationID", DbType::I8),
        ("flagID", DbType::I2),
        ("quantity", DbType::I4),
        ("groupID", DbType::I4),
        ("categoryID", DbType::I4),
        // Index 8, and it is not optional: the client's row indices are
        // hard-coded and `ixSingleton` is **10**, so omitting this shifts
        // `stacksize` and `singleton` down one and index 10 does not exist.
        ("customInfo", DbType::WStr),
        ("stacksize", DbType::I4),
        ("singleton", DbType::I4),
    ]
    .into_iter()
    .map(|(name, db_type)| Column {
        name: name.as_bytes().to_vec(),
        db_type,
    })
    .collect()
}

/// The `serviceInfo` dict, were we ever to answer it non-empty.
///
/// Kept because it documents the shape (`{serviceLogName: machoResolveString}`)
/// and because a multi-node server would need it. Not wired up: `{}` is the
/// right answer for one node.
/// Who is online: an empty `CRowset` keyed on `contactID`.
///
/// See the `onlineStatus` arm for why this must be the `CRowset` family and
/// not `Rowset`: the client later builds `blue.DBRow(self.onlineStatus.header,
/// ...)`, and `blue.DBRow` needs a real `DBRowDescriptor`, which only this
/// family's `.header` is.
///
/// `online` is `DBTYPE_BOOL`, which the packed layout stores as a *bit* rather
/// than a byte -- see `docs/reference/dbrow.md` on size classes. With no rows that costs
/// nothing, but it is the reason the column type is worth getting right now
/// rather than when there is data in it.
/// One `GetEntryTypes` row, as a `utillib.KeyVal`.
///
/// `entryTypeName` is sent even though the client immediately overwrites it
/// with a localised string: `GetStaticData` does
/// `setattr(rec, 'entryTypeName', ...)`, and a KeyVal would accept the
/// attribute either way -- but sending it documents the column, and the one
/// shape that must not be used here is a row class that would *reject* the
/// assignment.
///
/// `entryTypeNameID` may be `None`: `localization.GetByMessageID` opens
/// `if messageID is None: return ''`.
fn entry_type(entry_type_id: i64, name: &str) -> PyValue {
    keyval(vec![
        ("entryTypeID", PyValue::Int(entry_type_id)),
        // **Not `None`, which blanks the Type column.** `GetStaticData`
        // overwrites `entryTypeName` with `GetByMessageID(entryTypeNameID)`,
        // and that returns `''` for `None` -- so the name we send below is
        // discarded and the column renders empty. The real id is
        // `59792 + entryTypeID` for the run `0..=108` only; above that the
        // mapping is a table, because the client's own
        // `accountingentrytypes.fsdbinary` pairs `entryTypeID` with
        // `entryTypeNameID` directly and the two stop agreeing at 109. This
        // comment said `0..=120` until 2026-10-20 and that was wrong: the
        // *message* block 59792..59912 is contiguous, but it spans three
        // disjoint entry-type runs, so extrapolating to 120 printed twelve
        // real names against the wrong entry types. See
        // `ledger::entry_type_name_id`.
        (
            "entryTypeNameID",
            match crate::ledger::entry_type_name_id(entry_type_id) {
                Some(id) => PyValue::Int(id),
                None => PyValue::None,
            },
        ),
        ("entryTypeName", PyValue::Unicode(name.to_owned())),
    ])
}

/// Jump clone state, as `GetCloneState` reads it.
///
/// **Three attributes, no more and no fewer.** `clonejumpsvc.py(90)
/// GetCloneState` is literally
///
/// ```text
/// kv = lm.GetCloneState()
/// self.jumpClones = kv.clones              # line 94 -- the AttributeError
/// self.jumpCloneImplants = kv.implants
/// self.timeLastJump = kv.timeLastJump
/// ```
///
/// and the code object's `co_names` holds exactly `clones`, `implants` and
/// `timeLastJump`, so the list is complete and is not the decompiler's
/// reading of it. A `KeyVal` short of any one of them fails identically one
/// line later. `KeyVal` rather than a `util.Row` because all three reads are
/// attribute reads and nothing subscripts or sizes the container itself.
///
/// **`clones` cannot be `None`, and that is a live lock rather than a
/// traceback.** `GetCloneState` caches on `if self.jumpClones is None:`, so
/// `None` raises before the assignment, the cache stays `None`, and the next
/// read re-issues the bound call -- the compounding pattern in
/// `docs/guides/debugging-the-client.md`. An empty `CRowset` is falsy *without* being
/// `None`, so the cache fills and the call happens once.
///
/// **A `CRowset` and not `[]`, even empty.** `jumpClonesPanel.py(593)`
/// iterates it unguarded, while `clonejumpsvc.py(143) DestroyInstalledClone`
/// calls `.Index('jumpCloneID')` and `clonejumpsvc.py(313)
/// GetImplantsForClone` calls `.Filter('jumpCloneID')`. Today's guards would
/// all be satisfied by any falsy iterable; a rowset is what keeps the arm
/// correct on the day we have clones to send. Columns must be declared even
/// with no rows, because `CFilterRowset.Build` opens with
/// `self.header.Keys().index(self.columnName)` -- it resolves the name before
/// it looks at the row count.
///
/// `timeLastJump` is the one field where `None` is the honest answer rather
/// than a stand-in: `jumpClonesPanel.py(664)` guards with
/// `if last_jump_filetime:` before `filetime_to_datetime`, so a falsy value
/// means "never jumped", `remaining_time_until_next_jump` falls to
/// `datetime.timedelta()`, and that is what makes `is_jump_available` true
/// and stops the controller starting a cooldown thread.
pub(crate) fn clone_state() -> PyValue {
    keyval(vec![
        ("clones", clones_rowset()),
        ("implants", clone_implants_rowset()),
        ("timeLastJump", PyValue::None),
    ])
}

/// Installed jump clones, empty, as a `CRowset`.
///
/// Columns are what the client reads off a row: `jumpCloneID`, `locationID`
/// and `cloneName` in `jumpClonesPanel.clones`, plus `typeID`.
fn clones_rowset() -> PyValue {
    let columns = [
        ("jumpCloneID", DbType::I4),
        ("typeID", DbType::I4),
        ("locationID", DbType::I8),
        ("cloneName", DbType::WStr),
    ]
    .into_iter()
    .map(|(name, db_type)| Column {
        name: name.as_bytes().to_vec(),
        db_type,
    })
    .collect();
    let descriptor = DbRowDescriptor::new(columns).expect("the clone columns are a valid layout");
    eve_rowset::CRowset::new(descriptor).to_pyvalue()
}

/// Implants fitted to jump clones, empty, as a `CRowset`.
///
/// `jumpCloneID` is the column `GetImplantsForClone` filters on and must be
/// declared even with no rows; `typeID` is the only value the client reads.
pub(crate) fn clone_implants_rowset() -> PyValue {
    let columns = [("jumpCloneID", DbType::I4), ("typeID", DbType::I4)]
        .into_iter()
        .map(|(name, db_type)| Column {
            name: name.as_bytes().to_vec(),
            db_type,
        })
        .collect();
    let descriptor = DbRowDescriptor::new(columns).expect("the implant columns are a valid layout");
    eve_rowset::CRowset::new(descriptor).to_pyvalue()
}

/// Clones installed in a ship or a station, empty, as a `CRowset`.
///
/// `shipconfig.ShowCloneFacility` reads `each['ownerID']` and
/// `each['jumpCloneID']` -- **by column name, not as attributes** -- so a
/// `DBRow` inside a `CRowset` is the shape, and `infosvc.GetCurrentShipCloneCount`
/// takes `len()` of the same thing.
fn ship_clones_rowset() -> PyValue {
    let columns = [
        ("jumpCloneID", DbType::I4),
        ("ownerID", DbType::I4),
        ("locationID", DbType::I8),
        ("typeID", DbType::I4),
    ]
    .into_iter()
    .map(|(name, db_type)| Column {
        name: name.as_bytes().to_vec(),
        db_type,
    })
    .collect();
    let descriptor =
        DbRowDescriptor::new(columns).expect("the ship-clone columns are a valid layout");
    eve_rowset::CRowset::new(descriptor).to_pyvalue()
}

/// The wallet journal, empty, as a `CRowset`.
///
/// Columns are the ones `Transaction.__init__` and `transactionsUtil` read.
/// `description` is a YAML string and the empty string is the safe value --
/// both consumers open `if not transaction.description: return`.
fn transactions_rowset(entries: &[&crate::ledger::Entry]) -> PyValue {
    let columns = [
        ("transactionID", DbType::I8),
        ("transactionDate", DbType::FileTime),
        ("entryTypeID", DbType::I4),
        ("ownerID1", DbType::I4),
        ("ownerID2", DbType::I4),
        ("referenceID", DbType::I8),
        ("amount", DbType::Cy),
        ("balance", DbType::Cy),
        ("description", DbType::WStr),
    ]
    .into_iter()
    .map(|(name, db_type)| Column {
        name: name.as_bytes().to_vec(),
        db_type,
    })
    .collect();
    let descriptor =
        DbRowDescriptor::new(columns).expect("the transaction columns are a valid layout");
    let mut rowset = eve_rowset::CRowset::new(descriptor);
    for e in entries {
        rowset
            .push_values(vec![
                DbValue::Int(e.transaction_id),
                // Never zero. A year-1601 date reaches `time.strftime`, which
                // CPython 2 refuses below 1900, and the `ValueError` is not
                // caught by the panel's `except AttributeError` -- so it takes
                // down the whole Journal tab rather than blanking one cell.
                DbValue::Int(e.date),
                DbValue::Int(e.entry_type_id),
                DbValue::Int(e.owner_id_1),
                DbValue::Int(e.owner_id_2),
                DbValue::Int(e.reference_id),
                // **ISK x 10,000.** `DBTYPE_CY`'s getter divides by 10,000.0
                // on read, so sending ISK directly shows every figure as a
                // ten-thousandth of itself. The scaling lives here and
                // nowhere else; `ledger::Entry` holds plain ISK.
                DbValue::Int((e.amount * 10_000.0).round() as i64),
                DbValue::Int((e.balance * 10_000.0).round() as i64),
                // A `WStr` column is an `Object` slot in the variable-length
                // area, so the value is a marshalled `PyValue`, not a bare
                // string. The empty string is the safe default and both
                // consumers open with `if not transaction.description:
                // return`.
                DbValue::Object(PyValue::Unicode(e.description.clone())),
            ])
            .expect("the journal row matches the descriptor declared above");
    }
    rowset.to_pyvalue()
}

/// The ids inside a `GetHistoryForManyTypeIDs` argument.
///
/// The argument is a Python **`set`**, and a set does not marshal as a
/// container -- it arrives as a reduce-style object, which `evecap` renders
///
/// ```text
/// <Call (<global __builtin__.set>, ([16272, 33681, ...]))>
/// ```
///
/// i.e. tag `0x22`, callable `__builtin__.set`, with the real list one level
/// down inside the argument tuple. Reading `args[0]` as a sequence finds
/// nothing. A plain list or tuple is accepted too, because nothing about the
/// caller guarantees the set and a future one may not use it.
fn type_ids_of(arg: Option<&PyValue>) -> Vec<i64> {
    fn ints(v: &PyValue, out: &mut Vec<i64>) {
        match v {
            PyValue::Int(i) => out.push(*i),
            PyValue::List(items) | PyValue::Tuple(items) => {
                for item in items {
                    ints(item, out);
                }
            }
            // The set wrapper: everything we want is in the header's
            // argument tuple, and the callable itself contributes no ints.
            PyValue::ObjectEx { header, .. } => ints(header, out),
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Some(v) = arg {
        ints(v, &mut out);
    }
    out
}

/// The six columns of a market price-history row, in wire order.
///
/// Order is not a free choice: `pricehistorychart.RenderTable` formats by
/// position -- `rec[0]` date, `rec[1]` low, `rec[2]` high, `rec[3]` average,
/// `rec[4]` quantity, `rec[5]` orders -- and `marketsvc` does
/// `priceVolume += entry[3] * entry[4]`.
fn price_history_descriptor() -> DbRowDescriptor {
    let columns = [
        ("historyDate", DbType::FileTime),
        ("lowPrice", DbType::Cy),
        ("highPrice", DbType::Cy),
        ("avgPrice", DbType::Cy),
        ("volume", DbType::I8),
        ("orders", DbType::I4),
    ]
    .into_iter()
    .map(|(name, db_type)| Column {
        name: name.as_bytes().to_vec(),
        db_type,
    })
    .collect();
    DbRowDescriptor::new(columns).expect("the price-history columns are a valid layout")
}

/// `{typeID: (old, new)}` with both halves empty, one entry per id asked for.
///
/// Three constraints, all of them load-bearing.
///
/// **Both halves must be `CRowset`s sharing one descriptor.**
/// `GetHistoryRowList` opens `dbutil.RowList((new.header, []))`, and
/// `RowsInit` raises `expected (DBRowDesciptor, [])` for anything whose
/// header is not a real `blue.DBRowDescriptor` -- so an old-style `Rowset`,
/// whose header is a list of strings, throws here. It then appends every row
/// of `old`, and `RowList.append` checks `type(row) != blue.DBRow` and
/// `row.__columns__ != self.columns`.
///
/// **There must be an entry for every id requested.** A missing key falls
/// through to `GetPriceHistory(typeID)`, which is *two* further remote calls
/// per type. The ticker asks for 31 types at once, so a partial answer costs
/// 62 extra round trips rather than one gap.
///
/// **Empty is safer than fabricated.** With both halves empty,
/// `GetHistoryRowList` appends exactly one synthetic row, so `len(history)`
/// is 1 and `TickerItem.LoadData` stops at its own `len(history) < 7` guard.
/// Anything longer walks into `hist[-1] / hist[-7] - 1.0`, and the padding
/// `GetHistoryRowList` inserts carries `lastPrice = 0.0` -- a
/// `ZeroDivisionError` waiting for whoever supplies real rows later.
fn empty_price_histories(type_ids: &[i64]) -> PyValue {
    let descriptor = price_history_descriptor();
    PyValue::Dict(
        type_ids
            .iter()
            .map(|id| {
                let pair = PyValue::Tuple(vec![
                    eve_rowset::CRowset::new(descriptor.clone()).to_pyvalue(),
                    eve_rowset::CRowset::new(descriptor.clone()).to_pyvalue(),
                ]);
                // Dict entries are (value, key) in wire order.
                (pair, PyValue::Int(*id))
            })
            .collect(),
    )
}

/// The NPC-to-NPC standings rowset: a **`Rowset`** instance, with no rows.
///
/// `Standing.__RefreshStandings` does nothing with this but call `.Filter`
/// on it, twice, before any other statement:
///
/// ```python
/// tmp = standingsMgr.GetNPCNPCStandings()
/// self.npcnpcstandingsto = tmp.Filter('toID')
/// self.npcnpcstandingsfrom = tmp.Filter('fromID')
/// ```
///
/// So this is the fourth row of the "empty is not one answer, it is four"
/// table in `docs/guides/debugging-the-client.md`: it has to be an object with methods.
/// `Filter` then does `header.index(idName)`, which is why both key columns
/// must be named even though there are no rows to key.
///
/// A `CRowset` would satisfy the same calls (`CFilterRowset` is a `dict`
/// subclass, and `DBRow` resolves `.toID` through `__columns__`). `Rowset`
/// is chosen because its header is a plain list of strings rather than a
/// `DBRowDescriptor`, so there is nothing to get wrong about column types.
pub fn npc_npc_standings() -> PyValue {
    instance(
        "eve.common.script.sys.rowset.Rowset",
        vec![
            (
                "header",
                // `fromID` and `toID` are the two `Filter` keys; `standing` is
                // read as `standing.standing` in `Standing.GetStanding`.
                PyValue::List(vec![bstr("fromID"), bstr("toID"), bstr("standing")]),
            ),
            ("lines", PyValue::List(vec![])),
        ],
    )
}

/// The agent rowset: a **`Rowset`** instance, with no rows.
///
/// Named for the class, `eve.common.script.sys.rowset.Rowset`, which is also
/// exactly what goes on the wire. Earlier comments here called it a
/// `util.Rowset`; that name is legacy and resolves to **nothing** in V24.01 --
/// the string `util.Rowset` has zero hits in the whole client tree, the class
/// body declares no `__guid__` at all, and `util.py` imports only `Row`,
/// `Wrapper` and `KeyVal`. Only `FilterRowset` still carries a `__guid__`
/// (`util.FilterRowset`). The wire identity has always been the dotted module
/// path, so this was never a bug -- but the name misled a reader once, so it
/// is gone.
///
/// See the `agentMgr.GetAgents` arm for why it must be this family and not
/// `CRowset`. The columns are the union of every attribute read off an agent
/// row across the agent service, the standings and mission helpers, and the
/// Agency's locator-agent provider. They have to be declared even empty,
/// because `Index` and `Filter` resolve a column by name up front.
/// The thirteen epic-arc starter agents, in `standingUIConst`'s own order.
///
/// Read from that module's constant pool, not from any table of ours.
const EPIC_ARC_STARTER_AGENTS: [i64; 13] = [
    3019415, // AGENT_KARDE_ROMU
    3019396, // AGENT_AURSA_KUNIVURI
    3019412, // AGENT_ROINERON_AVIVIERE
    3019411, // AGENT_ARSTEN_TAKALO
    3019467, // AGENT_ELLAR_STIN
    3019465, // AGENT_ATON_HORDNER
    3019466, // AGENT_ARAJNA_ASHIA
    3019460, // AGENT_ABDIEL_VERAT
    3019463, // AGENT_ARMENT_CAUTE
    3019462, // AGENT_ATMA_AULATO
    3019464, // AGENT_YADA_VINJIVAS
    3019457, // AGENT_KORI_LATMAKI
    3019356, // AGENT_SISTER_ALITURA
];

/// Escalating paths, empty, as an old-style `Rowset` **instance**.
///
/// Any non-`None` value breaks the 3-second loop described on the arm, so
/// the shape is chosen for the consumers rather than for the guard:
///
/// - the Job Board provider and the Agency content provider both iterate it
///   (`GET_ITER`), so `None` is a `TypeError` on those paths too;
/// - `journal.py(273) OnEscalatingPathMessage` does
///   `IndexRowset(exp1.header, exp1.lines, 'instanceID')`, and
///   `IndexRowset.__init__` runs `header.index('instanceID')` **before** it
///   looks at the row count. So `instanceID` must be declared even empty,
///   and a bare `[]` -- which would stop the loop -- has no `.header`. That
///   path needs a notify we never send, which is why `[]` survives today and
///   is still the wrong answer.
///
/// `Rowset` and not `CRowset`: nothing calls `.Index()` on this, so a
/// CRowset would buy exposure to `blue.pyos.XUtil_Index` for nothing.
///
/// **[I]** on every column name except `instanceID`. The others are the
/// attributes the client reads off a row, but no escalation row has ever
/// crossed the wire and the real header may name or order them differently.
/// Only `instanceID` is load-bearing while the rowset is empty.
pub fn escalating_path_rowset() -> PyValue {
    let columns = [
        "instanceID",
        "destDungeon",
        "srcDungeon",
        "dungeon",
        "expiryTime",
        "journalEntryTemplateID",
    ];
    instance(
        "eve.common.script.sys.rowset.Rowset",
        vec![
            (
                "header",
                PyValue::List(columns.iter().map(|c| bstr(c)).collect()),
            ),
            ("lines", PyValue::List(vec![])),
        ],
    )
}

/// Stations holding this character's assets, as a `CRowset`.
///
/// `CRowset` and not `Rowset`, because `assetsWindow.py(561)` and
/// `corp_ui_accounts.py(936)` call `.Index('stationID')` and then subscript
/// the result -- only the CRowset path yields a `CIndexedRowset`, which
/// subclasses `dict`. Columns are declared even with no rows because
/// `XUtil_Index` resolves the column name before it counts rows.
///
/// The four are the union of what the client reads off a station row:
/// `stationID` and `solarSystemID`, `typeID` -- **not** `stationTypeID` --
/// and `itemCount`. **[I]** on the widths only. `stationID` is I8 because a
/// player structure's id does not fit in I4, even though a station's does.
///
/// **It must not be empty, and that is the other half of the assets window.**
/// `assetsWindow.Search_thread` groups the items it was given by station and
/// then throws away every group whose station is not in here:
///
/// ```text
///     ownerStations = ...ListStations().Index('stationID')
///     for stationID, stationItems in stations.iteritems():
///         stationData = ownerStations.get(stationID, None)
///         if stationData is None:
///             continue
///         sortlocations.append((stationData.solarSystemID, stationID, ...))
/// ```
///
/// So an empty rowset here makes an answered `ListIncludingContainers`
/// invisible: the items are sent, every one of them is dropped, and the window
/// is empty with no traceback anywhere. That is a *worse* failure than the
/// `TypeError` it replaces, because nothing names it -- which is why this
/// landed in the same round as the two calls that feed it, not after.
pub fn station_assets_rowset(reg: &Registry, actor: Option<i64>) -> PyValue {
    let columns = [
        ("stationID", DbType::I8),
        ("typeID", DbType::I4),
        ("solarSystemID", DbType::I4),
        ("itemCount", DbType::I4),
    ]
    .into_iter()
    .map(|(name, db_type)| Column {
        name: name.as_bytes().to_vec(),
        db_type,
    })
    .collect();
    let descriptor = DbRowDescriptor::new(columns).expect("the station columns are a valid layout");
    let mut rowset = eve_rowset::CRowset::new(descriptor);

    // One row per station where this character owns something, which today is
    // one station holding one capsule. `itemCount` is the count the window
    // prints in the location label (`GetLocationData` reads
    // `fakeItems or station.itemCount`), so it is derived from
    // `owned_item_rows` rather than written as a literal -- the two cannot
    // then disagree about how many things are there.
    if let Some(c) = actor.and_then(|id| reg.character(id))
        && let (Some(station), Some(system)) = (c.station_id, c.solar_system_id)
    {
        let count = reg.owned_item_rows(actor).len() as i64;
        rowset
            .push_values(vec![
                DbValue::Int(station),
                DbValue::Int(c.station_type_id),
                DbValue::Int(system),
                DbValue::Int(count),
            ])
            .expect("the station row matches the descriptor declared above");
    }
    rowset.to_pyvalue()
}

/// The one agent that is really in our station.
///
/// **Read out of the client's own `npccharacters.fsdbinary`**, not invented:
/// filtered to `stationID == 60012526` -- the station we actually put the
/// character in -- which yields four NPC characters, all in corporation
/// 1000126, which is the same `station_owner_id` the station row already
/// carries. Their names resolve from the shipped localisation corpus.
///
/// **This was the wrong person until the `mission-model` lane caught it.** The
/// first pick was 3004001, "Anfrid Uptemulf", chosen because an early read
/// showed him carrying the record's optional `agent` sub-struct. He does not:
/// his presence word is `0x1a0`, and bit `0x1` is **clear**. He is an employee
/// of the corporation at that station, not an agent.
///
/// Three of the four NPC characters there *are* agents, all in division 24
/// (Security): 3019363 at level 1 type 10, 3018559 at level 3 type 8, and this
/// one. **3010110 is the only one that is both level 1 and `agentType` 2**,
/// which is the combination `eveCfg.CanUseAgent` short-circuits on -- so it is
/// the only one of the three a character with no standings can talk to.
///
/// **No name is sent.** An id below `minPlayerItem` is `IsSystemOrNPC`, so
/// `cfg.Recordset._Prime` resolves it through the client's own local data
/// before it ever asks us -- which means the client supplies the real name and
/// we cannot get it wrong.
const STATION_AGENT_ID: i64 = 3_010_110;

/// The agent's own level, type and division, **read rather than chosen**.
///
/// The `agent` sub-struct is **inline at record +8**, not a pointer -- which
/// is what the three implausible readings recorded here earlier were really
/// about. Two separate mistakes: one read followed record +8 as a pointer
/// (giving `agentType` 959459378 and a read off the end of the payload), and
/// the others were of 3004001, a row with **no `agent` struct at all**, so
/// every field was whatever happened to be in the padding.
///
/// With the struct read inline, the four characters at this station come out
/// coherent -- `(division, level, type)` of `(24, 1, 10)`, `(24, 1, 2)` and
/// `(24, 3, 8)` for the three agents, and nothing for the employee -- and
/// `pyd-schemas.tsv`'s convention is confirmed rather than doubted. Found by
/// the `mission-model` lane; the same inline-struct convention the
/// `dungeon-model` lane found for dict entries.
const STATION_AGENT_LEVEL: i64 = 1;
const STATION_AGENT_TYPE: i64 = 2;
/// Division 24, Security.
const STATION_AGENT_DIVISION: i64 = 24;

/// The faction that corporation belongs to.
///
/// **Read from the client, after I claimed twice that it was absent.** My
/// first two readings of `npccorporations.factionID` applied the record's
/// `delta` a second time -- `findings/pyd-schemas.tsv`'s offsets are
/// record-relative and already include it -- so I was reading `iconID` (1542)
/// and a presence word eight bytes past the end of a 224-byte record, which
/// lands in the next row and reads zero. Corrected by the `loader-pyd` lane,
/// which then closed every row of the column into `factions.fsdbinary`:
/// 273/273, zero exceptions.
///
/// Safe to send here, where it would not be on the epic-arc rows above: the
/// threshold loop that would claim a standings icon iterates
/// `EPIC_ARC_STARTER_AGENTS` only, and this agent is not in it.
const STATION_AGENT_FACTION: i64 = 500_007;

/// The ship's dogma attributes, as `{attributeID: value}`.
///
/// Ten entries, from [`crate::destiny::hull`]'s table -- the same table the
/// Destiny ball reads its mass and radius from, and the same provenance:
/// every number is the hull's own base attribute out of the client's static
/// data, read with `tools/dogma.py type <typeID>`.
///
/// Each pair is a maximum and the state that sits against it, because the HUD
/// draws a ratio of the two:
///
/// | ring | value | maximum |
/// |---|---|---|
/// | shield | `shieldCharge` 264 | `shieldCapacity` 263 |
/// | armour | `armorHP` 265 less `armorDamage` 266 | `armorHP` 265 |
/// | structure | `hp` 9 less `damage` 3 | `hp` 9 |
///
/// `damage` and `armorDamage` are sent as zero rather than omitted. They
/// would read as zero either way, but the fallback that produces that zero
/// costs two `invitems` lookups, an `evetypes.GetCategoryID` and a caught
/// `KeyError` **[V]** -- and `shipUI.UpdateGauges` is a polling loop rather
/// than an event handler, so that path would run for ever. Sending them also
/// means the first real damage packet finds a non-zero `oldValue` in
/// `ApplyAttributeChange`, which is what stops a change *to* zero being
/// dropped as a no-op.
///
/// The recharge rates are not ring inputs. They are here because
/// `godma.UpdateAttribute` builds a charged attribute only when **both** the
/// rate and the capacity are truthy (`:2055`) **[V]**, so a missing rate
/// costs the shield and the capacitor their regeneration and leaves a
/// half-built state that `ApplyAttributeChange` can later subscript into a
/// `KeyError`.
fn ship_attributes(content: &crate::content::Content, ship_type_id: i64) -> PyValue {
    ship_attributes_of(crate::destiny::hull::of_in(content, ship_type_id))
}

/// [`ship_attributes`] from a hull in hand: `crate::dogma::hull`'s fitted one.
fn ship_attributes_of(h: &crate::destiny::hull::Hull) -> PyValue {
    let targeting = targeting_of(h);
    // `(value, key)`: marshal dict entries are inverted on the wire, which
    // `pyutil::pydict` encodes once for string keys. These keys are integers,
    // so the inversion is written out here.
    PyValue::Dict(vec![
        // **Mass, top speed and agility, or the module tooltips divide by
        // zero.** Omitted, `GetAttribute` answers the definition's
        // `defaultValue` (0), so `clientDogmaLocation` saw a ship of mass 0 and
        // an MWD/AB tooltip died in `moduleButtonTooltip._GetVelocityRatio`
        // (`1 + boost * factor * 0.01 / mass`: `ZeroDivisionError`, 6 in the
        // 2026-10-28 client logs, mass = 0.0 on a type 11176 hull). The values
        // are the hull's, which `destiny::hull` takes from the same columns
        // the client's own FSD has (mass: `types`; 37 and 70: `typedogma`).
        // [A] the HUD and tooltips want the hull's unboosted numbers here;
        // falsified by a tooltip whose "with propulsion" figure is wrong.
        (PyValue::Real(h.mass), PyValue::Int(4)),
        (PyValue::Real(f64::from(h.max_velocity)), PyValue::Int(37)),
        (PyValue::Real(f64::from(h.agility)), PyValue::Int(70)),
        (PyValue::Real(h.hp), PyValue::Int(9)),
        (PyValue::Real(0.0), PyValue::Int(3)),
        (PyValue::Real(h.armor_hp), PyValue::Int(265)),
        (PyValue::Real(0.0), PyValue::Int(266)),
        (PyValue::Real(h.shield_capacity), PyValue::Int(263)),
        // State, seeded full. See `Registry::capsule_dogma_row` for why this
        // is not a type attribute and why full is the right seed.
        (PyValue::Real(h.shield_capacity), PyValue::Int(264)),
        (PyValue::Real(h.shield_recharge_rate), PyValue::Int(479)),
        (PyValue::Real(h.capacitor_capacity), PyValue::Int(482)),
        (PyValue::Real(h.capacitor_capacity), PyValue::Int(18)),
        (PyValue::Real(h.capacitor_recharge_rate), PyValue::Int(55)),
        // **The three targeting attributes, and a zero in any of them is
        // worse than a wrong value.** `GetAttribute` falls back to the
        // *attribute definition's* `defaultValue` -- all three default to
        // 0 -- so omitting them never raises and silently disables
        // targeting: a zero 76 hides the right-click "Lock target" entry
        // (`IsWithinTargettingRange` is `dist < 0`), and a zero 192 makes
        // the ctrl-marquee mass-lock select nothing
        // (`balls[:int(0)]`). `docs/re/targeting.md` §6.1-6.2.
        (PyValue::Real(targeting.max_target_range), PyValue::Int(76)),
        (
            PyValue::Real(targeting.max_locked_targets),
            PyValue::Int(192),
        ),
        (PyValue::Real(targeting.scan_resolution), PyValue::Int(564)),
    ])
}

/// **The ship's instance state as it is, not as a new ship has it**: the
/// shield (264), armour damage (266) and hull damage (3) from the ship's
/// health record, and the capacitor (18) from its charge, so a ship that
/// jumped a gate arrives as damaged and as drained as it left. Without a
/// record (nothing has hit it, or a dock forgot it) the full seeds of
/// [`ship_attributes_of`] stand. `[A]` the client's curves restart from
/// these values on arrival (`time: None`); falsified by a HUD ring that
/// jumps on the first server push after a jump.
fn with_instance_state(w: &Registry, c: &Character, attrs: PyValue) -> PyValue {
    let PyValue::Dict(mut e) = attrs else {
        return attrs;
    };
    let now = crate::pyutil::filetime_now();
    let mut set = |id: i64, v: f64| {
        e.retain(|(_, k)| *k != PyValue::Int(id));
        e.push((PyValue::Real(v), PyValue::Int(id)));
    };
    if let Some(s) = w.combat().ship(c.ship_id) {
        let r = s.rings(now);
        set(crate::damage::attr::SHIELD_CHARGE, r.shield_charge);
        set(crate::damage::attr::ARMOR_DAMAGE, r.armor_damage);
        set(crate::damage::attr::DAMAGE, r.damage);
    } else if let Some(h) = crate::repair::hurt(w, c.ship_id) {
        // Docked with damage (GAP-REPAIR): the stash is the record.
        set(crate::damage::attr::ARMOR_DAMAGE, h.armor);
        set(crate::damage::attr::DAMAGE, h.structure);
    }
    if w.modules().charge(c.ship_id).is_some() {
        let (cap, _) = crate::modules::capacitor_at(w, c.ship_id, c.ship_type_id, now);
        set(18, cap);
    }
    PyValue::Dict(e)
}

/// The ship's fitting state beside its attributes (`crate::fitstate`):
/// outputs, loads, calibration, free hardpoints and rig slots.
fn with_fit_state(w: &Registry, ship_id: i64, attrs: PyValue) -> PyValue {
    match attrs {
        PyValue::Dict(mut e) => {
            for (id, v) in crate::fitstate::attributes(w, ship_id) {
                e.retain(|(_, k)| *k != PyValue::Int(id));
                e.push((PyValue::Real(v), PyValue::Int(id)));
            }
            PyValue::Dict(e)
        }
        other => other,
    }
}

/// A stance hull's `stanceSwitchTime` (1985, ms) beside the ten: the button's
/// reactivation ramp reads `godma.GetItem(ship).stanceSwitchTime`
/// (`ship_stance_button.py:58` [V]) and without it the ramp is 0 s (FCI-FIX2).
fn with_stance_switch_time(
    content: &crate::content::Content,
    ship_type_id: i64,
    attrs: PyValue,
) -> PyValue {
    let ticks = crate::stance::switch_ticks(content, ship_type_id);
    match attrs {
        PyValue::Dict(mut e) if ticks > 0 => {
            e.push((
                PyValue::Real((ticks / crate::stance::MSEC) as f64),
                PyValue::Int(crate::stance::ATTR_STANCE_SWITCH_TIME),
            ));
            PyValue::Dict(e)
        }
        other => other,
    }
}

/// What a capsule is served instead of CCP's own zeros.
///
/// **A deliberate divergence from the shipped data, and the one in this file
/// worth arguing about.** `typedogma` for type 670 really is
/// `76 = 0, 192 = 0, 564 absent` `[V]` -- **a pod genuinely cannot lock
/// anything in EVE**, which is correct of the real game and fatal here,
/// because every character in this sandbox is sitting in one and **there is
/// no way to change ship**: nothing answers an `ActivateShip`, so a tester
/// cannot get into a hull that can lock.
///
/// So serving the honest zeros would mean shipping targeting that no human
/// can reach. These are an Impairor's numbers, which is the smallest real
/// hull's, and they make a pod a weak but functional ship.
///
/// **Delete this the day ship changing works**, and the morning test note
/// says so. Until then a pod locking things is us, not CCP.
const POD_TARGETING: Targeting = Targeting {
    max_target_range: 25_000.0,
    max_locked_targets: 3.0,
    scan_resolution: 485.0,
};

/// How many targets this hull may hold, as the **client was told** rather
/// than as the table has it.
///
/// `crate::targeting` has to enforce the limit against the same number the
/// player's HUD is counting against, which for a pod is the floor above and
/// not CCP's zero. A server that enforced the table's value while the client
/// drew three slots would refuse a lock the player could see room for.
pub(crate) fn max_locked_targets_of(content: &crate::content::Content, ship_type_id: i64) -> f64 {
    let h = crate::destiny::hull::of_in(content, ship_type_id);
    targeting_of(h).max_locked_targets
}

/// The three numbers the client needs to let a player lock anything.
struct Targeting {
    max_target_range: f64,
    max_locked_targets: f64,
    scan_resolution: f64,
}

/// This hull's targeting numbers, with [`POD_TARGETING`] standing in for a
/// hull that CCP gave none.
///
/// Keyed on "the hull cannot lock" rather than on the capsule's type id, so
/// anything else in the table with zeros gets the same treatment -- and so
/// that the reason is in the condition rather than in a magic number.
fn targeting_of(h: &crate::destiny::hull::Hull) -> Targeting {
    if h.max_locked_targets <= 0.0 || h.max_target_range <= 0.0 {
        return POD_TARGETING;
    }
    Targeting {
        max_target_range: h.max_target_range,
        max_locked_targets: h.max_locked_targets,
        scan_resolution: h.scan_resolution,
    }
}

pub fn service_info_dict(names: &[&str]) -> PyValue {
    pydict(names.iter().map(|n| (*n, bstr("location"))).collect())
}

/// Take the world's write lock, recovering from poisoning.
///
/// **One policy, one place, because the project had two and the difference
/// was invisible and permanent.** `dispatch.rs` and `push::spawn_world_ticker`
/// recovered with `into_inner()`; `server::serve_one`'s two session-table
/// acquisitions did not, and logged a line instead. A `RwLock`'s poison flag
/// **never clears** -- nothing calls `clear_poison` -- so after the first
/// panic anywhere under this lock, for the rest of the process's life,
/// dispatch kept working and the world ticker kept working while **every new
/// connection failed to enter the session table and every ending one failed
/// to leave it**.
///
/// That is not a cosmetic difference. `session_table::of_character` is how a
/// notification finds somebody who is not the caller, so a connection missing
/// from the table receives no addressed push at all: no fleet invite, no
/// `AddBalls2` for another pilot's ship, nothing from the world ticker. The
/// player's own calls are answered normally. They would be flying in a system
/// where nobody else exists, with two `eprintln` lines on the server's
/// terminal as the only evidence.
///
/// **Recovering is right here, and the reasoning is the same one
/// `push::Writers::guard` gives:** the value behind this lock has no
/// invariant a panic could leave half-broken -- a panicking dispatch arm has
/// changed some of the world and not the rest, which is a wrong world rather
/// than an unreadable one -- and refusing punishes every other player for one
/// connection's bug. The opposite policy belongs where a value *does* have an
/// invariant a panic can break: `Wire`, whose mutex guards a frame boundary,
/// and which both tickers therefore skip permanently.
pub(crate) fn world_write(
    world: &std::sync::RwLock<Registry>,
) -> std::sync::RwLockWriteGuard<'_, Registry> {
    world.write().unwrap_or_else(|e| e.into_inner())
}

/// A world lock guard that notes its wait and hold in
/// [`crate::clock_probe`] when it drops: the lock sites the turn stats do not
/// time (session open and close, login, sign-up, the gateway, shutdown's
/// flush). `site` names it in the `other held max` field of `[world] tick
/// lock`.
pub(crate) struct Timed<G> {
    guard: G,
    site: &'static str,
    wait: std::time::Duration,
    acquired: std::time::Instant,
}

impl<G> Timed<G> {
    fn new(guard: G, site: &'static str, asked: std::time::Instant) -> Self {
        let acquired = std::time::Instant::now();
        Timed {
            guard,
            site,
            wait: acquired.saturating_duration_since(asked),
            acquired,
        }
    }
}

impl<G> Drop for Timed<G> {
    fn drop(&mut self) {
        crate::clock_probe::note_other(self.site, self.wait, self.acquired.elapsed());
    }
}

impl<G: std::ops::Deref<Target = Registry>> std::ops::Deref for Timed<G> {
    type Target = Registry;
    fn deref(&self) -> &Registry {
        &self.guard
    }
}

impl<G: std::ops::DerefMut<Target = Registry>> std::ops::DerefMut for Timed<G> {
    fn deref_mut(&mut self) -> &mut Registry {
        &mut self.guard
    }
}

/// [`world_write`], timed under `site` (I-6) and counted among the threads
/// queued for the lock (I-5).
pub(crate) fn world_write_timed<'a>(
    world: &'a std::sync::RwLock<Registry>,
    site: &'static str,
) -> Timed<std::sync::RwLockWriteGuard<'a, Registry>> {
    let asked = std::time::Instant::now();
    let waiting = crate::clock_probe::Waiter::enter();
    let guard = world_write(world);
    drop(waiting);
    Timed::new(guard, site, asked)
}

/// The read twin of [`world_write_timed`]; a poisoned lock is recovered as
/// every other read of the world does.
pub(crate) fn world_read_timed<'a>(
    world: &'a std::sync::RwLock<Registry>,
    site: &'static str,
) -> Timed<std::sync::RwLockReadGuard<'a, Registry>> {
    let asked = std::time::Instant::now();
    let waiting = crate::clock_probe::Waiter::enter();
    let guard = world.read().unwrap_or_else(|e| e.into_inner());
    drop(waiting);
    Timed::new(guard, site, asked)
}

#[cfg(test)]
mod advance_tests;
#[cfg(test)]
mod tests;
