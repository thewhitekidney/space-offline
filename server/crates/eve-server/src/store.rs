//! Durability: the half of the world that has to outlive the process.
//!
//! The owner's test for the skill work is *"buy a skill, see it trained,
//! relaunch, still trained"*, and the last clause was the one still false --
//! `tools/up.sh` restarts the server, so every grant died with it. This is the
//! seam that fixes it, and it is deliberately the **smallest** thing that can:
//! one trait, one file format, one snapshot.
//!
//! # Why a text file and not SQLite
//!
//! `docs/reference/world-state.md` §3 recommends SQLite eventually, and the reasoning
//! there still holds: the wallet journal and market history are
//! append-forever and read-by-range, and a snapshot rewrites the whole world
//! on every save. But SQLite needs a **dependency decision** --
//! `server/deps.allow` is empty and stays empty until the owner says
//! otherwise -- plus an unverified claim that the distro ships a usable
//! `librust-rusqlite-dev`. Neither is mine to decide unattended.
//!
//! So: a snapshot behind a trait, which is exactly the stepping stone that
//! document plans for. The one thing kept from the SQLite argument is
//! **inspectability** -- its best point was `sqlite3 world.db 'select * from
//! items'` answering "where did my item go" in one command. A line-oriented
//! text format answers it with `grep`, and needs no reader binary of its own.
//! `.evecap` already taught us what a bespoke binary format costs: it needed a
//! whole tool before anyone could read it.
//!
//! # The write is tmp + fsync + rename
//!
//! Modelled on `capture.rs`, with **one policy deliberately inverted**:
//! `capture.rs` uses `File::create_new` *because a capture is evidence* and
//! must never be overwritten. A world file is the opposite -- the newest state
//! is the only one that matters -- so this truncates and replaces. The
//! contrast is worth stating, because the two look like the same operation.
//!
//! The rename is what makes a crash mid-save safe: either the old file or the
//! new one is there, never half of either.
//!
//! # The write is off the world lock (CP52, owner decision O1)
//!
//! [`Saver`] wraps the [`FileStore`]: its [`Store::save`] renders the
//! snapshot to text on the caller's thread, which holds the world lock (CPU
//! only), and hands the text to a `world-saver` thread that writes, fsyncs
//! and renames with **no** lock. Payload turns no longer save at all: the
//! clock saves a dirty world every `push::SAVE_EVERY`, so **RAM may lead disk
//! by up to `SAVE_EVERY`** and a crash loses at most that much. This is the
//! deliberate revision of `docs/reference/world-state.md` §4 rule 3, and the allocator
//! leap on load ([`settle`], C7 §3.4) is what keeps it safe: ids handed out
//! in the lost window are never reissued.
//!
//! - **The render is off the lock too (PERF-SAVE).** Under the lock the
//!   caller only clones the world into a [`Snapshot`] and hands it over
//!   ([`Store::save_owned`]); the saver thread renders and writes it. At the
//!   soak's size (50 characters, 3 000 killmails, a 9 MB file) the clone is
//!   a few ms and the render about 27 ms, so the lock no longer pays for
//!   the text. `docs/lanes/perf-save.md` has the numbers.
//! - The saver holds the world's lock file (`<file>.lock`, ACC2) until its
//!   last write is done, so `--grant` can never write under it.
//! - [`Store::close`] writes what is queued, joins the thread and releases
//!   the lock; a save after it is dropped, not written lock-free.
//! - Saves queued faster than the disk takes them are coalesced: only the
//!   newest text is written.
//!
//! # `world` and `delta`: generated content is never stored
//!
//! `world = generate(seed) + deltas` (`docs/content/plan.md` §4.2, C7 §3).
//! Two record types, not a schema bump, so a schema-2 file from before them
//! still loads:
//!
//! ```text
//! world  <seed_hex> <birth_filetime> <day> <last_rollover_filetime> <gen_version>
//! delta  <kind> <key_count> <key fields...> <value fields...>
//! ```
//!
//! The `world` line is written first after the magic line. A file without
//! one is a pre-C7 world: [`settle`] mints one and sets
//! [`Snapshot::world_minted`], which the registry turns into a dirty world so
//! the next save writes it. A `delta` row is kept **raw**: this file knows no
//! kind, so a kind no module reads survives load and save byte for byte.
//! `key_count` is a field of its own because the split between key and
//! value is the only structure a row has, and the file cannot infer it.
//!
//! # Upgrade safety (IP11; V6 F10, review M5 and M9, ruling 10)
//!
//! The owner's world file is real play data, so every path here fails safe:
//! the good file is replaced only by a save this build is sure of.
//!
//! - **The header names the format and the build:**
//!   `eve-offline-world 2 format=<STORE_FORMAT>.<kinds hash> build=<sha>`.
//!   An older build reads the first two tokens only, so the extra ones cost
//!   it nothing. The format is [`format_id`]; the sha is `EVE_GIT_SHA`, read
//!   at run time ([`build_sha`]), and is for names and logs only -- shas are
//!   not ordered, and a dev day of commits must not churn the backups.
//! - **A format change is backed up before the first save**, to
//!   `<file>.f<old format>-<old sha>.bak`, keeping [`BACKUPS_KEPT`] and the
//!   oldest of each STORE_FORMAT ([`backups_to_delete`]). A failed
//!   backup fails the save, and the good file stays.
//! - **A downgrade is refused**: a file holding records this build skipped,
//!   or a higher `STORE_FORMAT`, is never saved over unless the server runs
//!   with `--accept-downgrade` (and then it is backed up first). The session
//!   is written to `<file>.downgrade` instead, so nothing played is lost.
//! - **A tainted world never writes the file** (review M5): after a recovered
//!   panic, saves go to `<file>.tainted`, for forensics, and the log says so.
//! - **Every write is tmp + fsync + rename + directory fsync**, so a SIGTERM
//!   or a crash at any point leaves the old file or the new one, and at worst
//!   a stale `<file>.tmp` that the next save truncates and no load reads.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::delta::{Deltas, World};
use crate::industry::Job;
use crate::item::Item;
use crate::ledger::Entry;
use crate::live_status::{SaveCost, SaveWrites};
use crate::skill::SkillEntry;

/// The format's version, written as the first token of the file.
///
/// Read and refused rather than guessed at: a world we cannot parse is not a
/// world we may silently replace with an empty one. `PRAGMA user_version` is
/// the idea; this is the one-line form of it.
/// **Bumped to 2 by Round 0.3**, when the bio stopped being one string for
/// the whole server and became one per character. The `desc` record gained a
/// character id, and a `desc` line of the old shape is ambiguous -- a bio of
/// `"42"` is indistinguishable from a character id -- so there is no safe
/// heuristic and the version is the honest mechanism. A schema-1 world is
/// refused by name rather than silently read wrong; see the match in `parse`.
pub const SCHEMA: u32 = 2;

const MAGIC: &str = "eve-offline-world";

/// The store's own format number, below [`SCHEMA`]: bump it when a record
/// type is added or changes meaning in a way an older build can read past
/// (an unknown record is skipped, so the schema stays). A file with a higher
/// number is a downgrade. Together with [`kinds_hash`] it is [`format_id`],
/// the axis backups and the downgrade check are keyed on (review M9).
///
/// **2 since IP27-b**, which added the `character` record (ruling 8). An
/// unknown record is skipped, so a new record type alone would not need a
/// bump; this one does because a build that skips it would save the world
/// back without it and drop every character's owner silently. With the bump
/// an older build sees a higher format and a skipped record, and diverts
/// its saves to `<file>.downgrade` (IP11). A format-1 file reads as "no
/// character records": the roster comes from the boot config, every
/// character unowned, and the first save backs the old file up.
///
/// **3 since FX3**, which appended the character's station to the
/// `character` record (`docs/port-tracker.md` §72.1). A format-2 build would
/// read the record, ignore the extra fields and save it back without them,
/// putting every pilot back at their seed station; the bump makes that build
/// see a newer format and divert its saves (IP11). A format-2 record (five
/// fields) reads as "no saved station": the seed's station stands, exactly
/// as before, and the first save backs the old file up.
///
/// **4 since P-START-3**, which appended the character's identity (race,
/// bloodline, gender, school, corporation; [`IdentityRecord`]) to the
/// `character` record. A format-3 build would read past the extra fields and
/// save the record back without them, resetting every pilot's race and
/// school to the default; the bump makes it divert its saves (IP11). A
/// format-3 record (5 or 12 fields) reads as "no saved identity": the seed's
/// (`Character::default()`'s) stands, as that build had it.
///
/// **5 since REVIEW-FIX** (the review's item 15), for CORP-BUY's two trailing
/// `mktorder` fields (`corp_id`, `key_id`). A format-4 build reads such a line
/// without them, takes a corporation's order for its issuer's own and, on a
/// cancel or expiry, refunds the corp-paid escrow to the issuer's wallet: a
/// one-off faucet on rollback. The bump makes that build divert its saves
/// (IP11) instead. A format-4 `mktorder` line (no corp fields) reads as a
/// pilot's own order, which it was.
pub const STORE_FORMAT: u32 = 5;

/// How many `<file>.f*.bak` backups a format change keeps [A]: the last five
/// format changes, which is a few weeks of landings. Falsified by an owner
/// who needs a world from further back; `serve.sh`'s per-start copies (IP10)
/// are the deeper history.
pub const BACKUPS_KEPT: usize = 5;

/// The build name used when `EVE_GIT_SHA` is unset or unusable.
pub const UNKNOWN_BUILD: &str = "unknown";

/// FNV-1a over the kinds, sorted, one per line. Std only and stable across
/// Rust releases, which `DefaultHasher` does not promise. Order-free: the
/// list's order is presentation, the set is the format.
pub fn kinds_hash_of(kinds: &[&str]) -> u32 {
    let mut sorted: Vec<&str> = kinds.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut h: u32 = 0x811c_9dc5;
    for k in sorted {
        for b in k.bytes().chain(std::iter::once(b'\n')) {
            h ^= u32::from(b);
            h = h.wrapping_mul(0x0100_0193);
        }
    }
    h
}

/// [`kinds_hash_of`] over `delta_kinds::KINDS`: adding, renaming or removing
/// a kind is a format change (`delta_kinds.rs`).
pub fn kinds_hash() -> u32 {
    let kinds: Vec<&str> = crate::delta_kinds::KINDS.iter().map(|(k, _)| *k).collect();
    kinds_hash_of(&kinds)
}

/// This build's world format, `<STORE_FORMAT>.<kinds hash>`, then
/// `.<name>v<version>` for each installed module, in list order
/// ([`module_suffix`]). With no module installed it is the core's alone.
pub fn format_id() -> String {
    format!(
        "{STORE_FORMAT}.{:08x}{}",
        kinds_hash(),
        module_suffix(&crate::module::plan().modules)
    )
}

/// PR-SEAM-2: each module's own world-format number is part of the file's
/// format, so a world saved under another module version is backed up before
/// the first save, like a core format change. Letters and digits only
/// (`clean` drops everything else from a header); a module with no letters or
/// digits in its name is left out.
pub fn module_suffix(modules: &[(&str, u32)]) -> String {
    modules
        .iter()
        .filter_map(|(name, version)| {
            let n: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            (!n.is_empty()).then(|| format!(".{n}v{version}"))
        })
        .collect()
}

/// This build's git sha, from `EVE_GIT_SHA` at run time (no recompile per
/// commit, review M9), or [`UNKNOWN_BUILD`].
pub fn build_sha() -> String {
    clean(
        &std::env::var("EVE_GIT_SHA").unwrap_or_default(),
        UNKNOWN_BUILD,
    )
}

/// Keep `[0-9A-Za-z.]`, at most 40 characters, or `fallback`. A header
/// field becomes part of a file name, so nothing in it may name a path.
fn clean(raw: &str, fallback: &str) -> String {
    let v: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
        .take(40)
        .collect();
    if v.is_empty() || v.chars().all(|c| c == '.') {
        fallback.to_owned()
    } else {
        v
    }
}

/// What a file's header said and what its parse stepped over.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Provenance {
    /// `format=`, cleaned; `None` for a file from before IP11.
    pub format: Option<String>,
    /// `build=`, cleaned; `None` for a file from before IP11.
    pub build: Option<String>,
    /// Lines of a record type this build does not know. Each is data an
    /// older build would drop on its next save.
    pub skipped: usize,
}

/// One `character` record: who a character is beyond what the boot config
/// says (packet IP27-b, `docs/lanes/account-spec.md` §3.3).
///
/// `character<TAB>id<TAB>ship_id<TAB>owner_user_id or -<TAB>name[<TAB>station x7][<TAB>identity x5]`,
/// the name escaped like every other text field. Written for every
/// character, so a world in accounts mode knows its roster without any
/// `--character` flag. The station (FX3, format 3) is absent in a format-2
/// file and for a character with no station; the identity (P-START-3,
/// format 4) is absent before format 4. So a record is 5, 10, 12 or 17
/// fields, and any other length is fatal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterRecord {
    pub character_id: i64,
    /// The capsule the character was seeded with. A character the boot
    /// config does not know is seeded from it; the active ship is then
    /// derived from the items, as for every character.
    pub ship_id: i64,
    /// `None` is unowned, written `-`.
    pub owner_user_id: Option<i64>,
    pub name: String,
    /// The pilot's station when the world was saved (FX3): since FX1 it is
    /// whichever station they last docked at, not the seed's. `None` in a
    /// format-2 file; the seed's station then stands.
    pub station: Option<StationRecord>,
    /// Who the pilot is (P-START-3). `None` in a file before format 4; the
    /// seed's identity then stands.
    pub identity: Option<IdentityRecord>,
}

/// The five `Character` fields that say who a pilot is, as the `character`
/// record carries them (P-START-3), in this order. Without them a restart
/// reset every pilot's race, bloodline, gender, school and corporation to
/// `Character::default()`'s.
///
/// The corporation is the character's own field; a player corporation's
/// membership is its own record and `corp::sync_characters` reconciles the
/// two after a load, as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityRecord {
    pub race_id: i64,
    pub bloodline_id: i64,
    pub gender_id: i64,
    pub school_id: i64,
    pub corporation_id: i64,
}

impl IdentityRecord {
    pub fn of(c: &crate::model::Character) -> Self {
        IdentityRecord {
            race_id: c.race_id,
            bloodline_id: c.bloodline_id,
            gender_id: c.gender_id,
            school_id: c.school_id,
            corporation_id: c.corporation_id,
        }
    }

    /// Make this the character's identity: all five fields.
    pub fn apply(&self, c: &mut crate::model::Character) {
        c.race_id = self.race_id;
        c.bloodline_id = self.bloodline_id;
        c.gender_id = self.gender_id;
        c.school_id = self.school_id;
        c.corporation_id = self.corporation_id;
    }
}

/// The seven `Character` fields that put a pilot in a station, as the
/// `character` record carries them (FX3), in this order. The same seven as
/// `service::character::StationRef`, kept here because `store.rs` cannot
/// name that type and must not depend on its shape.
///
/// **Why the whole station, not just its id.** This server has no station
/// table (`Registry::dock_target`): a station is known only because some
/// character was seeded at it, which a restart with fewer `--character`
/// flags, or accounts mode, does not promise.
///
/// **`docked_station` is not stored** and needs no field: it is
/// `station_id` unless the roster or a jump says otherwise, and neither is
/// persisted, so a pilot saved in space comes back docked at this station,
/// where their ship item already is (`Registry::dock_at` moved it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StationRecord {
    pub station_id: i64,
    pub owner_id: i64,
    pub type_id: i64,
    pub operation_id: i64,
    pub solar_system_id: i64,
    pub constellation_id: i64,
    pub region_id: i64,
}

impl StationRecord {
    /// The character's station; `None` for a character with none.
    pub fn of(c: &crate::model::Character) -> Option<Self> {
        Some(StationRecord {
            station_id: c.station_id?,
            owner_id: c.station_owner_id,
            type_id: c.station_type_id,
            operation_id: c.station_operation_id,
            solar_system_id: c.solar_system_id?,
            constellation_id: c.constellation_id,
            region_id: c.region_id,
        })
    }

    /// Make this the character's station: all seven fields, never some.
    pub fn seat(&self, c: &mut crate::model::Character) {
        c.station_id = Some(self.station_id);
        c.station_owner_id = self.owner_id;
        c.station_type_id = self.type_id;
        c.station_operation_id = self.operation_id;
        c.solar_system_id = Some(self.solar_system_id);
        c.constellation_id = self.constellation_id;
        c.region_id = self.region_id;
    }
}

/// Everything that must survive a restart.
///
/// Deliberately *not* everything the world holds. Excluded, each for a
/// reason already recorded in `docs/reference/world-state.md` §2: the session and its
/// derived attributes (the client derives `locationid` and `corprole` and must
/// not be told them), bound objects and `next_object_id` (persisting those
/// would hand a reconnecting client a handle to an object that no longer
/// exists), captures, and anything regenerable from the client's own static
/// data.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// Every character and its owner (IP27-b). Empty in a format-1 file.
    pub characters: Vec<CharacterRecord>,
    /// `(character_id, balance)`.
    pub balances: Vec<(i64, f64)>,
    pub items: Vec<Item>,
    /// The item id allocator's next value.
    ///
    /// **Persisted because id reuse across a restart is a real hazard, not a
    /// theoretical one.** The client caches aggressively, so handing a
    /// reconnecting client an id it already has cached against a different
    /// object is a stale object, not a missing one. `docs/reference/world-state.md` §2
    /// item 4 lists id sequences as world state in their own right.
    pub next_item_id: i64,
    pub journal: Vec<Entry>,
    pub next_transaction_id: i64,
    /// `(character_id, skills)`.
    pub skills: Vec<(i64, Vec<SkillEntry>)>,
    /// `(character_id, logoff_date)`, a blue FILETIME each.
    ///
    /// **A new record type rather than a schema bump**, which is what the
    /// parser's leniency about unknown records is for: a schema-2 world
    /// written before this simply has no `logoff` lines, and the characters
    /// sort by roster order exactly as they did.
    pub logoff_dates: Vec<(i64, i64)>,
    /// The Bio tab's text, **per character**.
    ///
    /// Was one `String` for the whole server, which meant two players shared
    /// one bio: whoever saved last owned it, and the other player's
    /// character sheet showed it as their own.
    pub character_description: Vec<(i64, String)>,
    /// `(character_id, from_id, standing)`: NPC standings toward each
    /// character. Zeros are never written -- the client deletes a zero row,
    /// so a stored zero is a row that should not exist (`service/
    /// character.rs`, `Standings`). A new record type, so an older world
    /// file simply has none.
    pub standings: Vec<(i64, i64, f64)>,
    /// Every industry job, finished or running.
    ///
    /// **Including the finished ones**, and that is not completeness for its
    /// own sake: `GetJobsByOwner(ownerID, includeCompleted=True)` is what
    /// fills the Jobs tab's history, and `blueprintSvc` frees a blueprint
    /// only on seeing a status `>= 100`. A world that dropped delivered jobs
    /// would reopen with a blank history and, worse, with blueprints the
    /// client still believes are busy.
    pub jobs: Vec<Job>,
    /// The job id allocator's next value.
    ///
    /// Same hazard as [`Snapshot::next_item_id`], one layer up: a reused job
    /// id lands on `industrySvc._jobs[jobID]`, which is a dict the client
    /// keeps for the session, so a new job would inherit a dead one's row.
    pub next_job_id: i64,
    /// Every blueprint's ME, TE and runs that are not an unresearched
    /// original's: researched originals and every copy. Opaque here, like the
    /// bookmarks: the type renders and parses its own `bpstate` records
    /// (`crate::industry::Blueprints`). A new record type, so an older world
    /// file simply has none -- and every blueprint in it was an original.
    pub blueprints: crate::industry::Blueprints,
    /// Bookmark folders, subfolders and bookmarks. Opaque here: the type
    /// renders and parses its own records (`service/world.rs`).
    pub bookmarks: crate::service::Bookmarks,
    /// Address books and block lists. Opaque like the bookmarks: the type
    /// renders and parses its own records (`service/character.rs`).
    pub contacts: crate::service::Contacts,
    /// Home stations, jump clones and revived capsules. Opaque like the
    /// contacts: the type renders and parses its own records
    /// (`service/character.rs`).
    pub clones: crate::service::Clones,
    /// Skill training queues. Opaque like the clones: the type renders and
    /// parses its own records (`skill.rs`).
    pub skill_queues: crate::skill::Queues,
    /// Player corporations, memberships and applications. Opaque like the
    /// contacts: the type renders and parses its own records
    /// (`service/corp.rs`).
    pub corporations: crate::service::Corporations,
    /// Every market order and trade. Opaque here, like the bookmarks: the
    /// type renders and parses its own records (`service/market.rs`).
    pub market: crate::service::MarketBook,
    /// Every planetary colony and the PI id allocators. Opaque like the
    /// market: the type renders and parses its own records (`planet.rs`).
    pub planets: crate::planet::Saved,
    /// Every contract, and the escrow it accounts for. Opaque like the
    /// market: the type renders and parses its own records (`contracts.rs`).
    pub contracts: crate::contracts::Contracts,
    /// Every `delta` row and the `world` record (`crate::delta`). Opaque
    /// here: rows are saved and loaded raw, whatever their kind.
    pub deltas: Deltas,
    /// Set by [`settle`] when the file had no `world` record and one was
    /// minted. Never written. The registry marks itself dirty on it, so the
    /// new seed reaches the disk with the next save rather than being drawn
    /// again at the next load.
    pub world_minted: bool,
    /// Set by `Registry::to_snapshot` from `Registry::tainted` (review M5).
    /// Never written. A tainted snapshot is never saved over the world file:
    /// [`FileStore`] writes `<file>.tainted` instead.
    pub tainted: bool,
}

/// How far each allocator jumps on load (C7 §3.4) [A]: the most ids a
/// session can hand out after its last save. Falsified by nothing
/// realistic; it can only waste ids. The cost is that item ids pass 2^31
/// after about two thousand restarts, which plan §3.4 already assumes is
/// safe.
pub const LEAP: i64 = 1_000_000;

/// What a load does to a parsed file before the registry sees it.
///
/// 1. **A pre-C7 file gets a world.** No generated content existed before
///    the `world` record did, so drawing a seed now loses nothing.
/// 2. **The allocator leap** (C7 §3.4): each allocator becomes
///    `max(stored, highest id on disk + 1) + LEAP`. A save can trail RAM
///    (a crash, or a coalesced saver), and a reissued id is a stale object
///    in a client that cached the old one. The market's and the contracts'
///    allocators are private to their types, whose parsers already take
///    them above every row on disk; they are not leapt.
pub fn settle(s: &mut Snapshot, now: i64) {
    if s.deltas.world().is_none() {
        s.deltas.set_world(World::mint(now));
        s.world_minted = true;
    }
    let leap = |stored: i64, ids: &mut dyn Iterator<Item = i64>| -> i64 {
        let above = ids
            .max()
            .map_or(stored, |m| stored.max(m.saturating_add(1)));
        above.saturating_add(LEAP)
    };
    s.next_item_id = leap(s.next_item_id, &mut s.items.iter().map(|i| i.item_id));
    s.next_transaction_id = leap(
        s.next_transaction_id,
        &mut s.journal.iter().map(|e| e.transaction_id),
    );
    s.next_job_id = leap(s.next_job_id, &mut s.jobs.iter().map(|j| j.job_id));
    s.planets.next_pin_id = leap(
        s.planets.next_pin_id,
        &mut s
            .planets
            .colonies
            .iter()
            .flat_map(|c| c.pins.iter().map(|p| p.pin_id)),
    );
    s.planets.next_route_id = leap(
        s.planets.next_route_id,
        &mut s
            .planets
            .colonies
            .iter()
            .flat_map(|c| c.routes.iter().map(|r| r.route_id)),
    );
}

/// Somewhere a [`Snapshot`] can be kept.
///
/// A trait rather than a concrete type so that the SQLite step in
/// `docs/reference/world-state.md` §5 can replace the implementation without touching
/// anything above it -- which is the whole reason the seam goes in before the
/// database does.
pub trait Store: Send + Sync {
    /// The stored world, or `None` if there is not one yet.
    fn load(&self) -> io::Result<Option<Snapshot>>;
    fn save(&self, snapshot: &Snapshot) -> io::Result<()>;
    /// [`Self::save`], timed for the `[save]` line (E1): the render and any
    /// write on the caller's thread apart, and the rendered size. The caller
    /// fills in `snapshot`. This default can only time the whole call, so it
    /// reports it as the write.
    fn save_timed(&self, snapshot: &Snapshot) -> io::Result<SaveCost> {
        let start = Instant::now();
        self.save(snapshot)?;
        Ok(SaveCost {
            write: start.elapsed(),
            ..SaveCost::default()
        })
    }
    /// [`Self::save_timed`] for a snapshot the caller is done with (PERF-SAVE):
    /// a store that renders and writes off the caller's thread takes it
    /// without a copy, so the caller's world lock is held for the clone that
    /// made the snapshot and nothing more. The default is [`Self::save_timed`].
    fn save_owned(&self, snapshot: Snapshot) -> io::Result<SaveCost> {
        self.save_timed(&snapshot)
    }
    /// The writes finished off the caller's thread since the last call
    /// (E1b). A store that writes inside [`Self::save`] has none.
    fn take_writes(&self) -> SaveWrites {
        SaveWrites::default()
    }
    /// Shutdown: write everything already handed to [`Store::save`], then
    /// refuse later saves. A store that writes as it is called has nothing
    /// to do; [`Saver`] joins its thread and releases the world's lock.
    fn close(&self) {}
}

/// A store that keeps nothing. The default, and what the tests use.
#[derive(Debug, Default)]
pub struct NoStore;

impl Store for NoStore {
    fn load(&self) -> io::Result<Option<Snapshot>> {
        Ok(None)
    }
    fn save(&self, _snapshot: &Snapshot) -> io::Result<()> {
        Ok(())
    }
}

/// One file on disk.
#[derive(Debug)]
pub struct FileStore {
    path: PathBuf,
    /// This build's sha, for the header and the backup names.
    build: String,
    /// `--accept-downgrade`: save over records this build skipped.
    accept_downgrade: bool,
    guard: Mutex<Guard>,
}

/// What the last load decided the first save must do first.
#[derive(Debug, Default)]
struct Guard {
    /// Copy the file here before the first save over it.
    backup: Option<PathBuf>,
    /// Why the file may not be saved over, for a downgrade.
    refused: Option<String>,
    said_tainted: bool,
    said_refused: bool,
}

impl FileStore {
    /// A store at `path`, named by `EVE_GIT_SHA` ([`build_sha`]).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        FileStore {
            path: path.into(),
            build: build_sha(),
            accept_downgrade: false,
            guard: Mutex::default(),
        }
    }

    /// Name this build `sha` instead of `EVE_GIT_SHA` (tests).
    #[cfg(test)]
    pub fn with_build(mut self, sha: &str) -> Self {
        self.build = clean(sha, UNKNOWN_BUILD);
        self
    }

    /// `--accept-downgrade`: a file holding records this build skips is
    /// saved over all the same, after a backup.
    pub fn accept_downgrade(mut self, yes: bool) -> Self {
        self.accept_downgrade = yes;
        self
    }

    /// `<file><suffix>`, beside the file.
    fn beside(&self, suffix: &str) -> PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(suffix);
        self.path.with_file_name(name)
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, Guard> {
        // A panic while holding this cannot leave it half-written in a way
        // that matters: every field is a whole value.
        self.guard.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Decide, from a loaded file's header, what the first save must do.
    fn judge(&self, p: &Provenance) {
        let ours = format_id();
        let theirs = p.format.clone().unwrap_or_else(|| "legacy".to_owned());
        let newer = theirs
            .split('.')
            .next()
            .and_then(|n| n.parse::<u32>().ok())
            .is_some_and(|n| n > STORE_FORMAT);
        let mut g = self.guard();
        *g = Guard::default();
        if theirs != ours || p.skipped > 0 {
            let build = p.build.as_deref().unwrap_or(UNKNOWN_BUILD);
            g.backup = Some(self.beside(&format!(".f{theirs}-{build}.bak")));
            crate::outln!(
                "[world] file format {theirs} (build {build}), this build {ours} (build {}): \
                 backed up before the first save",
                self.build
            );
        }
        if (newer || p.skipped > 0) && !self.accept_downgrade {
            let why = format!(
                "{} holds {} record(s) this build skips (format {theirs}, this build {ours}); \
                 it will not be saved over -- this session goes to {} -- unless the server \
                 runs with --accept-downgrade",
                self.path.display(),
                p.skipped,
                self.beside(".downgrade").display()
            );
            crate::errln!("[world] DOWNGRADE REFUSED: {why}");
            g.refused = Some(why);
        }
    }

    /// Copy the file to `to`, atomically, then prune to [`BACKUPS_KEPT`].
    /// A file that is not there has nothing to lose.
    fn back_up(&self, to: &Path) -> io::Result<()> {
        let bytes = match fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        write_atomic(to, &bytes)?;
        crate::errln!(
            "[world] backed up {} to {}",
            self.path.display(),
            to.display()
        );
        self.prune(to);
        Ok(())
    }

    /// Remove all but the newest [`BACKUPS_KEPT`] and the oldest of each
    /// format of `<file>.f*.bak`, never
    /// `keep`. Best effort: a backup that will not go is only disk.
    fn prune(&self, keep: &Path) {
        let Some(dir) = self.path.parent().map(|d| {
            if d.as_os_str().is_empty() {
                Path::new(".")
            } else {
                d
            }
        }) else {
            return;
        };
        let stem = self.path.file_name().unwrap_or_default().to_string_lossy();
        let prefix = format!("{stem}.f");
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let found: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.starts_with(&prefix) && n.ends_with(".bak")
            })
            .map(|e| {
                let t = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                (t, e.path())
            })
            .filter(|(_, p)| p.file_name() != keep.file_name())
            .collect();
        let named: Vec<(std::time::SystemTime, String)> = found
            .iter()
            .map(|(t, p)| {
                (
                    *t,
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                )
            })
            .collect();
        let doomed = backups_to_delete(&named, &stem, BACKUPS_KEPT.saturating_sub(1));
        for (_, old) in found.into_iter().filter(|(_, p)| {
            doomed.contains(
                &p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            )
        }) {
            if let Err(e) = fs::remove_file(&old) {
                crate::errln!("[world] could not prune {}: {e}", old.display());
            }
        }
    }
}

/// The STORE_FORMAT number in a backup name `<stem>.f<N>.<hash>-<sha>.bak`.
fn backup_format(stem: &str, name: &str) -> Option<u32> {
    let rest = name.strip_prefix(stem)?.strip_prefix(".f")?;
    rest.split('.').next()?.parse().ok()
}

/// Which backups to delete (pure). Keep the `newest` by mtime (name breaks
/// ties), and ALSO the oldest backup of each STORE_FORMAT: the pre-upgrade
/// original of that format, which a night of landings (each changes the
/// kinds hash and so makes a backup) must never push out. Names that do not
/// parse as a backup of `stem` are never listed for deletion. Returns names.
fn backups_to_delete(
    found: &[(std::time::SystemTime, String)],
    stem: &str,
    newest: usize,
) -> Vec<String> {
    let mut sorted: Vec<&(std::time::SystemTime, String)> = found
        .iter()
        .filter(|(_, n)| backup_format(stem, n).is_some())
        .collect();
    sorted.sort_by(|a, b| b.cmp(a)); // newest first
    let mut oldest: std::collections::HashMap<u32, &String> = std::collections::HashMap::new();
    for (_, n) in sorted.iter().rev() {
        if let Some(f) = backup_format(stem, n) {
            oldest.entry(f).or_insert(n);
        }
    }
    sorted
        .iter()
        .skip(newest)
        .filter(|(_, n)| !oldest.values().any(|o| *o == n))
        .map(|(_, n)| n.clone())
        .collect()
}

impl Store for FileStore {
    fn load(&self) -> io::Result<Option<Snapshot>> {
        match fs::read_to_string(&self.path) {
            Ok(text) => {
                let (mut s, p) = parse_with(&text)?;
                self.judge(&p);
                settle(&mut s, crate::pyutil::filetime_now());
                Ok(Some(s))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn save(&self, snapshot: &Snapshot) -> io::Result<()> {
        self.write_rendered(&self.render(snapshot), snapshot.tainted)
    }

    fn save_timed(&self, snapshot: &Snapshot) -> io::Result<SaveCost> {
        let start = Instant::now();
        let text = self.render(snapshot);
        let render = start.elapsed();
        let start = Instant::now();
        self.write_rendered(&text, snapshot.tainted)?;
        Ok(SaveCost {
            snapshot: Duration::ZERO,
            render,
            write: start.elapsed(),
            bytes: text.len() as u64,
        })
    }
}

impl FileStore {
    /// The file's text for `snapshot`. CPU only: this is the half of a save
    /// that runs under the world lock ([`Saver`]).
    pub fn render(&self, snapshot: &Snapshot) -> String {
        render_as(snapshot, &self.build)
    }

    /// The disk half of a save: IP11's diverts, the backup, then the atomic
    /// write. `tainted` is the snapshot's own flag, carried beside its text.
    fn write_rendered(&self, text: &str, tainted: bool) -> io::Result<()> {
        let mut g = self.guard();
        // **Tainted first**: a half-written world is not saved anywhere it
        // could be mistaken for the real one, downgrade or not.
        if tainted {
            let to = self.beside(".tainted");
            if !g.said_tainted {
                g.said_tainted = true;
                crate::errln!(
                    "[world] TAINTED: not saving over {}; this and every later save goes to {} \
                     -- the good file is the one from before the panic",
                    self.path.display(),
                    to.display()
                );
            }
            return write_atomic(&to, text.as_bytes());
        }
        if let Some(why) = g.refused.clone() {
            if !g.said_refused {
                g.said_refused = true;
                crate::errln!("[world] DOWNGRADE REFUSED: {why}");
            }
            let to = self.beside(".downgrade");
            return write_atomic(&to, text.as_bytes());
        }
        // **Backed up before the first save, or not saved.** A copy we
        // could not make is a reason to keep the old file, not to lose it.
        if let Some(to) = g.backup.clone() {
            self.back_up(&to)?;
            g.backup = None;
        }
        write_atomic(&self.path, text.as_bytes())
    }
}

/// A snapshot cloned under the world lock, on its way to the disk. It is
/// rendered to text on the saver thread (PERF-SAVE), after coalescing, so a
/// save the disk never reaches is never rendered at all.
struct Rendered {
    snapshot: Snapshot,
    /// The save's number in this process, for the `[save]` line.
    generation: u64,
}

enum SaveJob {
    Write(Box<Rendered>),
    /// Answered once every write sent before it is on disk (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    Flush(mpsc::Sender<()>),
}

/// The off-lock saver (CP52): a [`FileStore`] whose writes happen on a
/// thread of their own. See the module comment for the rule it revises.
///
/// **Coalesced, not queued:** when the disk falls behind, the thread writes
/// only the newest text it has been handed. Every text is a whole world,
/// so an older one has nothing the newer one lacks.
pub struct Saver {
    store: Arc<FileStore>,
    /// `None` once [`Store::close`] has run.
    tx: Mutex<Option<mpsc::Sender<SaveJob>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// The writes the thread finished since the clock last took them (E1b).
    /// A leaf lock: held only to add one write or to take the total, never
    /// while taking another lock, so the clock may take it under the world
    /// lock.
    written: Arc<Mutex<SaveWrites>>,
    generation: AtomicU64,
    said_closed: AtomicBool,
}

impl Saver {
    /// Start the `world-saver` thread over `store`. `lock` is the world's
    /// lock file (`account_book::lock_world`): the thread owns it and drops
    /// it only after its last write, so no write ever lands unlocked.
    pub fn spawn(store: FileStore, lock: Option<fs::File>) -> io::Result<Saver> {
        let store = Arc::new(store);
        let (tx, rx) = mpsc::channel();
        let disk = Arc::clone(&store);
        let written = Arc::new(Mutex::new(SaveWrites::default()));
        let tally = Arc::clone(&written);
        let thread = std::thread::Builder::new()
            .name("world-saver".to_owned())
            .spawn(move || {
                let _lock = lock;
                saver_loop(&disk, &rx, &tally);
            })?;
        Ok(Saver {
            store,
            tx: Mutex::new(Some(tx)),
            thread: Mutex::new(Some(thread)),
            written,
            generation: AtomicU64::new(0),
            said_closed: AtomicBool::new(false),
        })
    }

    /// Block until every save handed over so far is on disk. Not needed
    /// for correctness ([`Store::close`] does it at shutdown); for tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn flush(&self) {
        let (ack, done) = mpsc::channel();
        if self
            .sender()
            .is_some_and(|tx| tx.send(SaveJob::Flush(ack)).is_ok())
        {
            let _ = done.recv();
        }
    }

    fn sender(&self) -> Option<mpsc::Sender<SaveJob>> {
        // Every field is a whole value: a poisoned lock is still consistent.
        self.tx.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Store for Saver {
    fn load(&self) -> io::Result<Option<Snapshot>> {
        self.store.load()
    }

    fn save(&self, snapshot: &Snapshot) -> io::Result<()> {
        self.save_timed(snapshot).map(|_| ())
    }

    /// Copy the snapshot (a caller that can give it up uses
    /// [`Store::save_owned`]); render and write on the saver thread.
    fn save_timed(&self, snapshot: &Snapshot) -> io::Result<SaveCost> {
        self.save_owned(snapshot.clone())
    }

    /// Hand the snapshot to the saver thread, which renders and writes it
    /// with no lock (PERF-SAVE: the render used to run here, under the
    /// caller's world lock). `Ok` means *handed over*: a failed write is
    /// logged there, as a failed save always was, and never fails the
    /// player's call. The render and write times arrive through
    /// [`Store::take_writes`].
    fn save_owned(&self, snapshot: Snapshot) -> io::Result<SaveCost> {
        let Some(tx) = self.sender() else {
            if !self.said_closed.swap(true, Ordering::Relaxed) {
                crate::errln!(
                    "[world] a save after shutdown was dropped: the saver is closed and \
                     the world's lock released"
                );
            }
            return Ok(SaveCost::default());
        };
        let rendered = Rendered {
            snapshot,
            generation: self.generation.fetch_add(1, Ordering::Relaxed) + 1,
        };
        tx.send(SaveJob::Write(Box::new(rendered)))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the world saver is gone"))?;
        Ok(SaveCost::default())
    }

    fn take_writes(&self) -> SaveWrites {
        std::mem::take(&mut *self.written.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn close(&self) {
        drop(self.tx.lock().unwrap_or_else(|e| e.into_inner()).take());
        let thread = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(t) = thread
            && t.join().is_err()
        {
            crate::errln!("[world] the saver thread panicked; the last save may not be on disk");
        }
    }
}

impl Drop for Saver {
    /// A saver dropped without [`Store::close`] (a failed `bind`) still
    /// writes what it holds and releases the lock.
    fn drop(&mut self) {
        self.close();
    }
}

/// The `world-saver` thread: write the newest text, answer flushes, and end
/// when every sender is gone, after writing what was queued. Each write is
/// added to `written` for the clock's `[save]` line (E1b); the thread
/// prints nothing of its own but a failure.
fn saver_loop(store: &FileStore, rx: &mpsc::Receiver<SaveJob>, written: &Mutex<SaveWrites>) {
    while let Ok(first) = rx.recv() {
        let mut newest: Option<Box<Rendered>> = None;
        let mut acks = Vec::new();
        let mut coalesced = 0u64;
        for job in std::iter::once(first).chain(std::iter::from_fn(|| rx.try_recv().ok())) {
            match job {
                SaveJob::Write(r) => coalesced += u64::from(newest.replace(r).is_some()),
                SaveJob::Flush(ack) => acks.push(ack),
            }
        }
        if let Some(r) = newest {
            let start = Instant::now();
            let text = store.render(&r.snapshot);
            let tainted = r.snapshot.tainted;
            drop(r.snapshot);
            match store.write_rendered(&text, tainted) {
                Ok(()) => written.lock().unwrap_or_else(|e| e.into_inner()).observe(
                    text.len() as u64,
                    start.elapsed(),
                    coalesced,
                    r.generation,
                ),
                Err(e) => crate::errln!("[world] save failed: {e}"),
            }
        }
        for ack in acks {
            let _ = ack.send(());
        }
    }
}

/// tmp + fsync + rename + directory fsync.
///
/// The tmp is `<path>.tmp`, in the same directory, so the rename stays in
/// one filesystem and is atomic. A kill before the rename leaves the old
/// file whole and a stale tmp that the next write truncates; a kill after
/// it leaves the new file whole.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, bytes, || Ok(()))
}

/// [`write_atomic`] with `before_rename` run between the synced tmp and the
/// rename: the instant a crash is worst placed. A test fails it to prove the
/// old file survives; the real write passes a no-op.
fn write_atomic_with(
    path: &Path,
    bytes: &[u8],
    before_rename: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    if let Some(dir) = dir {
        fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        // Before the rename, not after: a rename that lands while the data
        // is still in the page cache can leave a correctly-named file with
        // nothing in it.
        f.sync_all()?;
    }
    before_rename()?;
    fs::rename(&tmp, path)?;
    // The rename itself lives in the directory; without this a power cut
    // can bring back the old name. Best effort: not every platform opens a
    // directory, and the data is already safe either way.
    if let Ok(d) = fs::File::open(dir.unwrap_or(Path::new("."))) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Escape a string so it survives a tab-separated, line-oriented file.
///
/// Percent-escaping rather than backslashes, so that the escape character
/// cannot appear unescaped and the transform is its own inverse's input.
pub(crate) fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '%' => out.push_str("%25"),
            '\t' => out.push_str("%09"),
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            _ => out.push(c),
        }
    }
    out
}

pub(crate) fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let hex: String = it.by_ref().take(2).collect();
        match u8::from_str_radix(&hex, 16) {
            Ok(b) => out.push(b as char),
            // An unterminated escape is kept verbatim rather than dropped:
            // losing a character silently is worse than keeping a stray '%'.
            Err(_) => {
                out.push('%');
                out.push_str(&hex);
            }
        }
    }
    out
}

#[cfg(test)]
fn render(s: &Snapshot) -> String {
    render_as(s, UNKNOWN_BUILD)
}

fn render_as(s: &Snapshot, build: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{MAGIC} {SCHEMA} format={} build={build}\n",
        format_id()
    ));
    if let Some(w) = s.deltas.world() {
        out.push_str(&format!(
            "world\t{:016x}\t{}\t{}\t{}\t{}\n",
            w.seed, w.birth, w.day, w.last_rollover, w.gen_version
        ));
    }
    for c in &s.characters {
        let owner = c.owner_user_id.map_or("-".to_owned(), |u| u.to_string());
        out.push_str(&format!(
            "character\t{}\t{}\t{owner}\t{}",
            c.character_id,
            c.ship_id,
            esc(&c.name)
        ));
        if let Some(st) = &c.station {
            out.push_str(&format!(
                "\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                st.station_id,
                st.owner_id,
                st.type_id,
                st.operation_id,
                st.solar_system_id,
                st.constellation_id,
                st.region_id
            ));
        }
        if let Some(id) = &c.identity {
            out.push_str(&format!(
                "\t{}\t{}\t{}\t{}\t{}",
                id.race_id, id.bloodline_id, id.gender_id, id.school_id, id.corporation_id
            ));
        }
        out.push('\n');
    }
    for (id, balance) in &s.balances {
        out.push_str(&format!("balance\t{id}\t{balance}\n"));
    }
    for i in &s.items {
        out.push_str(&format!(
            "item\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            i.item_id,
            i.type_id,
            i.owner_id,
            i.location_id,
            i.flag_id,
            i.quantity,
            i.group_id,
            i.category_id,
            i.singleton as u8
        ));
    }
    out.push_str(&format!("nextitem\t{}\n", s.next_item_id));
    for e in &s.journal {
        out.push_str(&format!(
            "journal\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            e.transaction_id,
            e.date,
            e.entry_type_id,
            e.owner_id_1,
            e.owner_id_2,
            e.reference_id,
            e.amount,
            e.balance,
            esc(&e.description)
        ));
    }
    out.push_str(&format!("nexttx\t{}\n", s.next_transaction_id));
    for (char_id, skills) in &s.skills {
        for k in skills {
            out.push_str(&format!(
                "skill\t{}\t{}\t{}\t{}\t{}\n",
                char_id, k.type_id, k.level, k.points, k.rank
            ));
        }
    }
    for (char_id, when) in &s.logoff_dates {
        if *when > 0 {
            out.push_str(&format!("logoff\t{}\t{}\n", char_id, when));
        }
    }
    for (char_id, text) in &s.character_description {
        if !text.is_empty() {
            out.push_str(&format!("desc\t{}\t{}\n", char_id, esc(text)));
        }
    }
    for (char_id, from_id, standing) in &s.standings {
        if *standing != 0.0 {
            out.push_str(&format!("standing\t{char_id}\t{from_id}\t{standing}\n"));
        }
    }
    for j in &s.jobs {
        // Twenty-eight columns in `industry::Job`'s own field order, so that
        // a diff of two world files is readable and so that adding a field
        // means appending a column rather than renumbering.
        //
        // `-1` is the `None` of the three `Option<i64>`s. A sentinel rather
        // than an empty field because an empty field is indistinguishable
        // from a truncated line, and `-1` is not a valid character id, date
        // or type id -- `opt_num` is the only reader of it.
        out.push_str(&format!(
            "job\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            j.job_id,
            j.activity_id,
            j.blueprint_id,
            j.blueprint_type_id,
            j.blueprint_location_id,
            j.blueprint_location_flag_id,
            j.blueprint_copy as u8,
            j.facility_id,
            j.owner_id,
            j.status.code(),
            j.installer_id,
            j.completed_character_id.unwrap_or(-1),
            j.solar_system_id,
            j.station_id,
            j.start_date,
            j.end_date,
            j.pause_date.unwrap_or(-1),
            j.runs,
            j.licensed_runs,
            j.successful_runs,
            j.cost,
            j.time_in_seconds,
            j.probability,
            j.product_type_id,
            j.optional_type_id.unwrap_or(-1),
            j.optional_type_id2.unwrap_or(-1),
            j.output_location_id,
            j.output_flag_id,
        ));
    }
    out.push_str(&format!("nextjob\t{}\n", s.next_job_id));
    s.blueprints.render(&mut out);
    s.bookmarks.render(&mut out);
    s.contacts.render(&mut out);
    s.clones.render(&mut out);
    s.skill_queues.render(&mut out);
    s.corporations.render(&mut out);
    s.market.render(&mut out);
    s.planets.render(&mut out);
    s.contracts.render(&mut out);
    for (kind, rows) in s.deltas.kinds() {
        for (key, value) in rows {
            let mut f = vec!["delta".to_owned(), esc(kind), key.len().to_string()];
            f.extend(key.iter().chain(value).map(|v| esc(v)));
            out.push_str(&f.join("\t"));
            out.push('\n');
        }
    }
    out
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

#[cfg(test)]
fn parse(text: &str) -> io::Result<Snapshot> {
    parse_with(text).map(|(s, _)| s)
}

fn parse_with(text: &str) -> io::Result<(Snapshot, Provenance)> {
    let mut lines = text.lines();
    let header = lines.next().unwrap_or_default();
    let mut parts = header.split_whitespace();
    if parts.next() != Some(MAGIC) {
        return Err(bad("not an eve-offline world file"));
    }
    match parts.next().map(|v| v.parse::<u32>()) {
        Some(Ok(v)) if v == SCHEMA => {}
        // **Refused, not migrated and not ignored.** A world we cannot read
        // is not a world we may replace with an empty one: the player would
        // lose everything and the only symptom would be a fresh character.
        Some(Ok(v)) => return Err(bad(&format!("world schema {v}, this build reads {SCHEMA}"))),
        _ => return Err(bad("world file has no schema version")),
    }
    let mut prov = Provenance::default();
    for token in parts {
        if let Some(v) = token.strip_prefix("format=") {
            prov.format = Some(clean(v, "unknown"));
        } else if let Some(v) = token.strip_prefix("build=") {
            prov.build = Some(clean(v, UNKNOWN_BUILD));
        }
    }

    let mut s = Snapshot::default();
    let mut skills: Vec<(i64, Vec<SkillEntry>)> = Vec::new();
    for (n, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let num = |i: usize| -> io::Result<i64> {
            f.get(i)
                .and_then(|v| v.parse::<i64>().ok())
                .ok_or_else(|| bad(&format!("line {}: field {i} is not an integer", n + 2)))
        };
        let real = |i: usize| -> io::Result<f64> {
            f.get(i)
                .and_then(|v| v.parse::<f64>().ok())
                .ok_or_else(|| bad(&format!("line {}: field {i} is not a number", n + 2)))
        };
        match f[0] {
            // **Malformed is fatal**, like a job: a character we cannot read
            // is an owner we would silently drop on the next save.
            "character" => {
                let owner = match f.get(3).copied() {
                    Some("-") => None,
                    _ => Some(num(3)?),
                };
                let name = f
                    .get(4)
                    .map(|v| unesc(v))
                    .filter(|n| !n.is_empty())
                    .ok_or_else(|| bad(&format!("line {}: a character with no name", n + 2)))?;
                // Five fields is a format-2 record: no saved station. A
                // station is all seven or fatal -- half a station would seat
                // a pilot in a system that does not hold it -- and the
                // identity (format 4) all five: 5, 10, 12 or 17 fields.
                let (has_station, has_identity) = match f.len() {
                    5 => (false, false),
                    10 => (false, true),
                    12 => (true, false),
                    17 => (true, true),
                    _ => {
                        return Err(bad(&format!(
                            "line {}: a character's station is not seven fields \
                             or its identity not five",
                            n + 2
                        )));
                    }
                };
                let station = if has_station {
                    Some(StationRecord {
                        station_id: num(5)?,
                        owner_id: num(6)?,
                        type_id: num(7)?,
                        operation_id: num(8)?,
                        solar_system_id: num(9)?,
                        constellation_id: num(10)?,
                        region_id: num(11)?,
                    })
                } else {
                    None
                };
                let at = if has_station { 12 } else { 5 };
                let identity = if has_identity {
                    Some(IdentityRecord {
                        race_id: num(at)?,
                        bloodline_id: num(at + 1)?,
                        gender_id: num(at + 2)?,
                        school_id: num(at + 3)?,
                        corporation_id: num(at + 4)?,
                    })
                } else {
                    None
                };
                s.characters.push(CharacterRecord {
                    character_id: num(1)?,
                    ship_id: num(2)?,
                    owner_user_id: owner,
                    name,
                    station,
                    identity,
                });
            }
            "balance" => s.balances.push((num(1)?, real(2)?)),
            "item" => s.items.push(Item {
                item_id: num(1)?,
                type_id: num(2)?,
                owner_id: num(3)?,
                location_id: num(4)?,
                flag_id: num(5)?,
                quantity: num(6)?,
                group_id: num(7)?,
                category_id: num(8)?,
                singleton: num(9)? != 0,
            }),
            "nextitem" => s.next_item_id = num(1)?,
            "journal" => s.journal.push(Entry {
                transaction_id: num(1)?,
                date: num(2)?,
                entry_type_id: num(3)?,
                owner_id_1: num(4)?,
                owner_id_2: num(5)?,
                reference_id: num(6)?,
                amount: real(7)?,
                balance: real(8)?,
                description: unesc(f.get(9).copied().unwrap_or("")),
            }),
            "nexttx" => s.next_transaction_id = num(1)?,
            "skill" => {
                let char_id = num(1)?;
                let entry = SkillEntry {
                    type_id: num(2)?,
                    level: num(3)?,
                    points: num(4)?,
                    rank: num(5)?,
                };
                match skills.iter_mut().find(|(id, _)| *id == char_id) {
                    Some((_, v)) => v.push(entry),
                    None => skills.push((char_id, vec![entry])),
                }
            }
            "logoff" => s.logoff_dates.push((num(1)?, num(2)?)),
            "standing" => s.standings.push((num(1)?, num(2)?, real(3)?)),
            "job" => {
                let opt_num = |i: usize| -> io::Result<Option<i64>> {
                    Ok(match num(i)? {
                        -1 => None,
                        v => Some(v),
                    })
                };
                let code = num(10)?;
                // **A status we do not know is fatal, not skipped.** The
                // whole point of the field is whether the job is finished;
                // guessing it either frees a blueprint that is still busy or
                // strands one for ever, and both are invisible until a player
                // notices. Unknown *record types* are the lenient case, and
                // this is not one.
                let status = crate::industry::Status::from_code(code).ok_or_else(|| {
                    bad(&format!(
                        "line {}: {code} is not an industry job status",
                        n + 2
                    ))
                })?;
                s.jobs.push(Job {
                    job_id: num(1)?,
                    activity_id: num(2)?,
                    blueprint_id: num(3)?,
                    blueprint_type_id: num(4)?,
                    blueprint_location_id: num(5)?,
                    blueprint_location_flag_id: num(6)?,
                    blueprint_copy: num(7)? != 0,
                    facility_id: num(8)?,
                    owner_id: num(9)?,
                    status,
                    installer_id: num(11)?,
                    completed_character_id: opt_num(12)?,
                    solar_system_id: num(13)?,
                    station_id: num(14)?,
                    start_date: num(15)?,
                    end_date: num(16)?,
                    pause_date: opt_num(17)?,
                    runs: num(18)?,
                    licensed_runs: num(19)?,
                    successful_runs: num(20)?,
                    cost: num(21)?,
                    time_in_seconds: num(22)?,
                    probability: real(23)?,
                    product_type_id: num(24)?,
                    optional_type_id: opt_num(25)?,
                    optional_type_id2: opt_num(26)?,
                    output_location_id: num(27)?,
                    output_flag_id: num(28)?,
                })
            }
            "nextjob" => s.next_job_id = num(1)?,
            "world" => {
                let seed = f
                    .get(1)
                    .and_then(|v| u64::from_str_radix(v, 16).ok())
                    .ok_or_else(|| bad(&format!("line {}: the seed is not hex", n + 2)))?;
                let small = |i: usize| -> io::Result<u32> {
                    u32::try_from(num(i)?)
                        .map_err(|_| bad(&format!("line {}: field {i} is out of range", n + 2)))
                };
                s.deltas.set_world(World {
                    seed,
                    birth: num(2)?,
                    day: small(3)?,
                    last_rollover: num(4)?,
                    gen_version: small(5)?,
                });
            }
            // **A malformed row is fatal**, like a job's: a row we cannot
            // split is one whose difference we would silently lose.
            "delta" => {
                let kind = f
                    .get(1)
                    .ok_or_else(|| bad(&format!("line {}: a delta with no kind", n + 2)))?;
                let keys = usize::try_from(num(2)?)
                    .ok()
                    .filter(|k| 3 + k <= f.len())
                    .ok_or_else(|| bad(&format!("line {}: the key count is wrong", n + 2)))?;
                let field = |v: &&str| unesc(v);
                // Unchecked: a kind this build does not list is kept (CP5).
                s.deltas.put_verbatim(
                    &unesc(kind),
                    f[3..3 + keys].iter().map(field).collect(),
                    f[3 + keys..].iter().map(field).collect(),
                );
            }
            "desc" => s
                .character_description
                .push((num(1)?, unesc(f.get(2).copied().unwrap_or("")))),
            tag if crate::planet::Saved::RECORDS.contains(&tag) => s
                .planets
                .parse_record(&f)
                .map_err(|why| bad(&format!("line {}: {why}", n + 2)))?,
            tag if crate::industry::Blueprints::RECORDS.contains(&tag) => s
                .blueprints
                .parse_record(&f)
                .map_err(|why| bad(&format!("line {}: {why}", n + 2)))?,
            tag if crate::service::MarketBook::RECORDS.contains(&tag) => s
                .market
                .parse_record(&f)
                .map_err(|why| bad(&format!("line {}: {why}", n + 2)))?,
            tag if crate::contracts::Contracts::RECORDS.contains(&tag) => s
                .contracts
                .parse_record(&f)
                .map_err(|why| bad(&format!("line {}: {why}", n + 2)))?,
            tag if crate::service::Bookmarks::RECORDS.contains(&tag) => s
                .bookmarks
                .parse_record(&f)
                .map_err(|why| bad(&format!("line {}: {why}", n + 2)))?,
            tag if crate::service::Contacts::RECORDS.contains(&tag) => s
                .contacts
                .parse_record(&f)
                .map_err(|why| bad(&format!("line {}: {why}", n + 2)))?,
            tag if crate::service::Clones::RECORDS.contains(&tag) => s
                .clones
                .parse_record(&f)
                .map_err(|why| bad(&format!("line {}: {why}", n + 2)))?,
            tag if crate::skill::Queues::RECORDS.contains(&tag) => s
                .skill_queues
                .parse_record(&f)
                .map_err(|why| bad(&format!("line {}: {why}", n + 2)))?,
            tag if crate::service::Corporations::RECORDS.contains(&tag) => s
                .corporations
                .parse_record(&f)
                .map_err(|why| bad(&format!("line {}: {why}", n + 2)))?,
            // An unknown record is **skipped, not fatal**, which is the one
            // place this is lenient: it lets a newer build add a record type
            // that an older one can still mostly read. The schema version
            // covers the changes that are not safe that way. **Counted**:
            // a save would drop it, so the store refuses one (IP11).
            _ => {
                prov.skipped += 1;
                continue;
            }
        }
    }
    s.skills = skills;
    Ok((s, prov))
}

#[cfg(test)]
mod perf;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        Snapshot {
            characters: vec![
                CharacterRecord {
                    character_id: 90_000_001,
                    ship_id: 100_000_000,
                    owner_user_id: Some(1001),
                    name: "Invented\tPilot".to_owned(),
                    station: Some(StationRecord {
                        station_id: 69_000_002,
                        owner_id: 1_999_002,
                        type_id: 990_002,
                        operation_id: 99,
                        solar_system_id: 39_000_001,
                        constellation_id: 29_000_001,
                        region_id: 19_000_001,
                    }),
                    identity: Some(IdentityRecord {
                        race_id: 91,
                        bloodline_id: 901,
                        gender_id: 0,
                        school_id: 99,
                        corporation_id: 1_999_001,
                    }),
                },
                CharacterRecord {
                    character_id: 90_000_002,
                    ship_id: 100_000_005,
                    owner_user_id: None,
                    name: "Unowned".to_owned(),
                    station: None,
                    identity: Some(IdentityRecord {
                        race_id: 92,
                        bloodline_id: 902,
                        gender_id: 1,
                        school_id: 98,
                        corporation_id: 1_999_002,
                    }),
                },
            ],
            balances: vec![(90_000_001, 1_234_567.89)],
            items: vec![Item {
                item_id: 100_000_000,
                type_id: 587,
                owner_id: 90_000_001,
                location_id: 60_012_526,
                flag_id: 4,
                quantity: 1,
                group_id: 25,
                category_id: 6,
                singleton: true,
            }],
            next_item_id: 100_000_001,
            journal: vec![Entry {
                transaction_id: 1,
                date: 126_966_528_000_000_000,
                entry_type_id: 3,
                owner_id_1: 1,
                owner_id_2: 90_000_001,
                reference_id: 0,
                amount: 1_000_000.0,
                balance: 1_234_567.89,
                description: String::new(),
            }],
            next_transaction_id: 2,
            skills: vec![(
                90_000_001,
                vec![SkillEntry {
                    type_id: 3300,
                    level: 2,
                    points: 1415,
                    rank: 1,
                }],
            )],
            logoff_dates: vec![(90_000_001, 133_000_000_000_000_000)],
            character_description: vec![(90_000_001, "hello".to_owned())],
            standings: vec![(90_000_001, 500_001, 2.5), (90_000_001, 1_000_125, -0.1)],
            // A running job, with **every `Option` populated** so that the
            // round trip exercises the `-1` sentinel in both directions
            // rather than only the `None` one.
            jobs: vec![Job {
                job_id: 1,
                activity_id: 1,
                blueprint_id: 100_000_002,
                blueprint_type_id: 21_858,
                blueprint_location_id: 60_012_526,
                blueprint_location_flag_id: 4,
                blueprint_copy: false,
                facility_id: 60_012_526,
                owner_id: 90_000_001,
                status: crate::industry::Status::Installed,
                installer_id: 90_000_001,
                completed_character_id: None,
                solar_system_id: 30_000_001,
                station_id: 60_012_526,
                start_date: 133_000_000_000_000_000,
                end_date: 133_000_003_000_000_000,
                pause_date: None,
                runs: 2,
                licensed_runs: 10,
                successful_runs: 2,
                cost: 4_321,
                time_in_seconds: 600,
                probability: 1.0,
                product_type_id: 21_857,
                optional_type_id: None,
                optional_type_id2: None,
                output_location_id: 60_012_526,
                output_flag_id: 4,
            }],
            next_job_id: 2,
            blueprints: Default::default(),
            bookmarks: Default::default(),
            contacts: Default::default(),
            clones: Default::default(),
            skill_queues: Default::default(),
            corporations: Default::default(),
            market: Default::default(),
            planets: Default::default(),
            contracts: Default::default(),
            deltas: sample_deltas(),
            world_minted: false,
            tainted: false,
        }
    }

    fn v(fields: &[&str]) -> Vec<String> {
        fields.iter().map(|f| (*f).to_owned()).collect()
    }

    /// Invented rows, a world record with every field off its default, and
    /// a kind no module reads.
    fn sample_deltas() -> Deltas {
        let mut d = Deltas::default();
        d.set_world(World {
            seed: 0x0123_4567_89ab_cdef,
            birth: 133_000_000_000_000_000,
            day: 3,
            last_rollover: 133_000_002_000_000_000,
            gen_version: 7,
        });
        d.put(
            "rock",
            v(&["9001", "3"]),
            v(&["17", "1.5", "-2", "3e9", "40"]),
        );
        d.put("beltwait", v(&["9002"]), v(&["133000001000000000"]));
        d.put_verbatim("zzfuture", v(&["a", "b"]), v(&["c"]));
        d
    }

    #[test]
    fn a_snapshot_survives_a_round_trip() {
        let s = sample();
        let back = parse(&render(&s)).expect("parses");
        assert_eq!(back, s);
    }

    /// A job's `Option` fields survive as `Some`, not only as `None`.
    ///
    /// The `-1` sentinel has two directions and `sample()` only exercises
    /// one. A delivered job is the case that carries all three.
    #[test]
    fn a_delivered_jobs_optional_fields_survive() {
        let mut s = sample();
        s.jobs[0].status = crate::industry::Status::Delivered;
        s.jobs[0].completed_character_id = Some(90_000_002);
        s.jobs[0].pause_date = Some(133_000_002_000_000_000);
        s.jobs[0].optional_type_id = Some(34);
        s.jobs[0].optional_type_id2 = Some(35);
        let back = parse(&render(&s)).expect("parses");
        assert_eq!(back, s);
        assert_eq!(back.jobs[0].completed_character_id, Some(90_000_002));
        assert_eq!(back.jobs[0].pause_date, Some(133_000_002_000_000_000));
    }

    /// A status code this build does not know is **fatal**, not skipped.
    ///
    /// The lenient case is an unknown *record type*; a job whose status we
    /// cannot read is a job whose blueprint we would either free while it is
    /// still busy or strand for ever, and neither shows up until a player
    /// notices. The property is "the world refuses to load", not "the error
    /// has that wording".
    #[test]
    fn a_job_with_an_unknown_status_refuses_to_load() {
        let good = render(&sample());
        let line = good
            .lines()
            .find(|l| l.starts_with("job\t"))
            .expect("the sample has a job");
        let mut f: Vec<&str> = line.split('\t').collect();
        f[10] = "4242";
        let broken = good.replace(line, &f.join("\t"));
        let e = parse(&broken).expect_err("4242 is not a status");
        assert!(
            format!("{e}").contains("4242"),
            "the error should name the code it could not read: {e}"
        );
    }

    /// A world written before jobs existed still loads, and loads with none.
    ///
    /// This is what the parser's leniency about unknown records buys, and it
    /// is the reason the job record did not need a schema bump: a schema-2
    /// file from yesterday has no `job` or `nextjob` lines at all.
    #[test]
    fn a_world_written_before_jobs_existed_still_loads() {
        let text =
            format!("{MAGIC} {SCHEMA}\nbalance\t90000001\t5.0\nnextitem\t100000000\nnexttx\t1\n");
        let s = parse(&text).expect("an older schema-2 world");
        assert!(s.jobs.is_empty());
        assert_eq!(s.next_job_id, 0, "and no allocator to restore");
    }

    /// IP27-b: a character's owner, and its absence, survive the text, and
    /// so does a name holding the characters that end a field.
    #[test]
    fn owner_user_id_round_trips() {
        let s = sample();
        let back = parse(&render(&s)).expect("parses");
        assert_eq!(back.characters, s.characters);
        assert!(back.characters.iter().any(|c| c.owner_user_id.is_none()));
        assert!(back.characters.iter().any(|c| c.owner_user_id.is_some()));
        let bad_owner = format!("{MAGIC} {SCHEMA}\ncharacter\t1\t2\tsomeone\tX\n");
        assert!(parse(&bad_owner).is_err(), "an unreadable owner is fatal");
        let no_name = format!("{MAGIC} {SCHEMA}\ncharacter\t1\t2\t-\n");
        assert!(parse(&no_name).is_err(), "a nameless character is fatal");
    }

    /// REVIEW-FIX: the corp fields of `mktorder` are a format bump (5).
    #[test]
    fn the_corp_order_fields_are_a_format_bump() {
        const { assert!(STORE_FORMAT >= 5, "mktorder gained corp_id and key_id") };
        assert!(format_id().starts_with("5."));
    }

    /// The format before IP27-b (no `character` records) still loads, with
    /// no characters, and is backed up before the first save over it.
    #[test]
    fn a_world_from_before_character_records_loads_and_is_backed_up() {
        const { assert!(STORE_FORMAT > 1, "the character record is a format bump") };
        let dir = scratch("prechar");
        let path = dir.join("world.dat");
        let old = Snapshot {
            characters: Vec::new(),
            ..sample()
        };
        let text = with_header(
            &render(&old),
            &format!("{MAGIC} {SCHEMA} format=1.{:08x} build=old", kinds_hash()),
        );
        fs::write(&path, &text).expect("writes");
        let store = FileStore::new(&path).with_build("new");
        let s = store.load().expect("loads").expect("a world");
        assert!(s.characters.is_empty());
        store.save(&sample()).expect("saves");
        assert_eq!(backups(&dir).len(), 1, "{:?}", backups(&dir));
        let now = store.load().expect("loads").expect("a world");
        assert_eq!(now.characters, sample().characters);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The format has to survive the characters that would otherwise end a
    /// line or a field, because a bio is free text the player types.
    #[test]
    fn text_with_tabs_and_newlines_survives() {
        let mut s = Snapshot {
            character_description: vec![(90_000_001, "a\tb\nc%d\re".to_owned())],
            ..Default::default()
        };
        s.journal.push(Entry {
            transaction_id: 1,
            date: 1,
            entry_type_id: 3,
            owner_id_1: 1,
            owner_id_2: 2,
            reference_id: 0,
            amount: 1.0,
            balance: 1.0,
            description: "line\tone\nline two".to_owned(),
        });
        let rendered = render(&s);
        // The property is not a line count -- `render` always emits the two
        // allocator lines -- it is that **no embedded character escapes its
        // field**. Asserting the count instead was wrong and said nothing.
        for (n, line) in rendered.lines().enumerate().skip(1) {
            let fields: Vec<&str> = line.split('\t').collect();
            match fields[0] {
                // `desc <characterID> <text>`: three fields since the bio
                // became per character.
                "desc" => assert_eq!(fields.len(), 3, "line {n}: {line:?}"),
                "logoff" => assert_eq!(fields.len(), 3, "line {n}: {line:?}"),
                // Five in format 2; twelve with the station (FX3).
                "character" => assert!(
                    fields.len() == 5 || fields.len() == 12,
                    "line {n}: {line:?}"
                ),
                "journal" => assert_eq!(fields.len(), 10, "line {n}: {line:?}"),
                _ => {}
            }
        }
        assert!(
            rendered.contains("%09") && rendered.contains("%0A") && rendered.contains("%25"),
            "tab, newline and percent must all be escaped: {rendered:?}"
        );
        let back = parse(&rendered).expect("parses");
        assert_eq!(back, s);
    }

    /// **A world we cannot read must not be silently replaced by an empty
    /// one.** The player would lose everything and the only symptom would be
    /// a fresh character -- the exact failure mode that makes people stop
    /// trusting a save file.
    #[test]
    fn an_unreadable_world_is_refused_rather_than_ignored() {
        assert!(parse("").is_err(), "an empty file is not a world");
        assert!(parse("something else\t1\n").is_err(), "wrong magic");
        assert!(parse(&format!("{MAGIC}\n")).is_err(), "no version");
        let future = format!("{MAGIC} {}\n", SCHEMA + 1);
        let e = parse(&future).expect_err("a newer schema must be refused");
        assert!(
            format!("{e}").contains(&(SCHEMA + 1).to_string()),
            "the error must name the version it found: {e}"
        );
    }

    /// An unknown record type is skipped, so an older build can still read
    /// most of a newer file. The schema version is what guards the changes
    /// that are *not* safe this way.
    #[test]
    fn an_unknown_record_is_skipped_not_fatal() {
        let text = format!("{MAGIC} {SCHEMA}\nbalance\t7\t1.5\nfuturething\tx\ty\n");
        let s = parse(&text).expect("parses");
        assert_eq!(s.balances, vec![(7, 1.5)]);
    }

    /// The `world` record round-trips, `gen_version` included, and is the
    /// first line after the magic line (C7 §3.2).
    #[test]
    fn the_world_record_round_trips_with_gen_version() {
        let s = sample();
        let text = render(&s);
        assert!(
            text.lines()
                .nth(1)
                .is_some_and(|l| l.starts_with("world\t")),
            "the world line comes first: {text:?}"
        );
        let back = parse(&text).expect("parses");
        assert_eq!(back.deltas.world(), s.deltas.world());
        assert_eq!(back, s);
    }

    /// A file from before the `world` record gets one at load, and the
    /// registry that applies it is dirty, so the next save writes the seed.
    /// A file that has one keeps it unchanged, and a load of it is clean.
    #[test]
    fn a_pre_c7_file_gets_a_seed_and_is_marked_dirty() {
        let old = format!("{MAGIC} {SCHEMA}\nbalance\t90000001\t5.0\n");
        let mut s = parse(&old).expect("a pre-C7 world");
        assert_eq!(s.deltas.world(), None);
        settle(&mut s, 133_000_000_000_000_000);
        assert!(s.world_minted);
        let w = *s.deltas.world().expect("a world was minted");
        assert_eq!(w.birth, 133_000_000_000_000_000);
        // The minted record reaches the file and is not drawn again.
        let mut again = parse(&render(&s)).expect("parses");
        settle(&mut again, 133_000_009_000_000_000);
        assert!(!again.world_minted);
        assert_eq!(again.deltas.world(), Some(&w));

        let mut reg = crate::service::Registry::new(crate::model::SandboxConfig::default());
        reg.apply_snapshot(s);
        assert!(reg.take_dirty(), "a minted world must be saved");
        assert_eq!(reg.deltas().world(), Some(&w));
        reg.apply_snapshot(again);
        assert!(!reg.take_dirty(), "loading a C7 world is not a change");
    }

    /// Every delta row comes back as it went in, including the fields the
    /// format must escape, an empty field, and a row with no value fields.
    /// It also survives the registry's `to_snapshot` and `apply_snapshot`.
    #[test]
    fn every_delta_row_round_trips_raw() {
        let mut d = sample_deltas();
        d.put(
            "site",
            v(&["30000001", "a\tb", ""]),
            v(&["x\ny", "%25", ""]),
        );
        d.put("arc", v(&["90000001", "5"]), Vec::new());
        d.put("lp", Vec::new(), v(&["1"]));
        let s = Snapshot {
            deltas: d.clone(),
            ..Default::default()
        };
        let back = parse(&render(&s)).expect("parses");
        assert_eq!(back.deltas, d);

        let mut reg = crate::service::Registry::new(crate::model::SandboxConfig::default());
        reg.apply_snapshot(back);
        assert_eq!(reg.to_snapshot().deltas, d);
    }

    /// A kind no module reads is kept: the line it was loaded from is
    /// written back byte for byte. Unknown *record types* are still skipped.
    #[test]
    fn an_unknown_kind_is_preserved_verbatim() {
        let line = "delta\tnot-a-kind-yet\t2\t30000001\tk%09\t1.25\t\tlast";
        let text = format!("{MAGIC} {SCHEMA}\n{line}\nfuturething\tx\n");
        let s = parse(&text).expect("parses");
        let mut reg = crate::service::Registry::new(crate::model::SandboxConfig::default());
        reg.apply_snapshot(s);
        let out = render(&reg.to_snapshot());
        assert!(out.lines().any(|l| l == line), "{line:?} not in {out:?}");
        assert!(!out.contains("futurething"));
    }

    /// A delta row whose key count runs past its fields is refused, like a
    /// job with an unknown status: losing the row silently is worse.
    #[test]
    fn a_truncated_delta_row_refuses_to_load() {
        let text = format!("{MAGIC} {SCHEMA}\ndelta\trock\t3\t9001\t3\n");
        assert!(parse(&text).is_err());
    }

    /// After a load every allocator is above every id in the file, even
    /// when the stored allocator trails its own rows. The rule, not the
    /// size of the leap.
    #[test]
    fn after_load_every_allocator_is_above_every_id_on_disk() {
        let mut s = sample();
        // Allocators that trail the rows they allocated.
        s.next_item_id = 1;
        s.next_transaction_id = 1;
        s.next_job_id = 1;
        let point = crate::planet::SurfacePoint {
            latitude: 0.5,
            longitude: 0.5,
        };
        s.planets.colonies.push(crate::planet::Colony {
            planet_id: 40_000_001,
            owner_id: 90_000_001,
            planet_type_id: 11,
            planet_radius: 1.0,
            level: 0,
            current_sim_time: 0,
            pins: vec![crate::planet::Pin::new(
                900_000_000_123,
                9_100,
                90_000_001,
                point,
                0,
            )],
            links: Vec::new(),
            routes: vec![crate::planet::Route {
                route_id: 77,
                owner_id: 90_000_001,
                path: vec![900_000_000_123],
                commodity_type_id: 9_200,
                commodity_quantity: 1,
            }],
        });
        s.planets.next_pin_id = 5;
        s.planets.next_route_id = 5;

        let dir = std::env::temp_dir().join(format!("eve-world-leap-{}", std::process::id()));
        let store = FileStore::new(dir.join("world.txt"));
        store.save(&s).expect("saves");
        let back = store.load().expect("loads").expect("a world");
        let _ = fs::remove_dir_all(&dir);

        let max = |ids: &mut dyn Iterator<Item = i64>| ids.max().expect("rows");
        assert!(back.next_item_id > max(&mut s.items.iter().map(|i| i.item_id)));
        assert!(back.next_transaction_id > max(&mut s.journal.iter().map(|e| e.transaction_id)));
        assert!(back.next_job_id > max(&mut s.jobs.iter().map(|j| j.job_id)));
        assert!(back.planets.next_pin_id > 900_000_000_123);
        assert!(back.planets.next_route_id > 77);
        // And never below what was stored, when that is the larger.
        let mut ahead = sample();
        ahead.next_item_id = 500_000_000;
        settle(&mut ahead, 0);
        assert!(ahead.next_item_id > 500_000_000);
    }

    #[test]
    fn a_file_store_round_trips_through_the_filesystem() {
        let dir = std::env::temp_dir().join(format!("eve-world-{}", std::process::id()));
        let store = FileStore::new(dir.join("world.txt"));
        assert_eq!(
            store.load().expect("a missing file is not an error"),
            None,
            "no world yet is None, not an error -- a first run is normal"
        );
        let s = sample();
        store.save(&s).expect("saves");
        // What comes back is the file after `settle`, which is a function of
        // the file alone when the file has a world record.
        let mut want = s.clone();
        settle(&mut want, 0);
        assert_eq!(store.load().expect("loads"), Some(want.clone()));

        // Saving twice replaces rather than appending or refusing -- the
        // opposite of `capture.rs`, whose `create_new` exists because a
        // capture is evidence.
        store.save(&s).expect("saves again");
        assert_eq!(store.load().expect("loads"), Some(want));
        // And no temporary file is left behind.
        assert!(!dir.join("world.txt.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }
    // ---- IP11: upgrade safety -------------------------------------------

    /// A fresh directory per test: tests share one process and run at once.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eve-ip11-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// `text` with its header replaced by `header`.
    fn with_header(text: &str, header: &str) -> String {
        let body = text.split_once('\n').map_or("", |(_, b)| b);
        format!("{header}\n{body}")
    }

    fn backups(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .expect("reads")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".bak"))
            .collect();
        v.sort();
        v
    }

    /// A file of another format is copied aside, byte for byte, before the
    /// first save writes over it, under its own format and sha -- and only
    /// once, not at every save.
    #[test]
    fn a_newer_format_is_backed_up_before_first_save() {
        let dir = scratch("backup");
        let path = dir.join("world.dat");
        let old = with_header(
            &render(&sample()),
            &format!("{MAGIC} {SCHEMA} format=0.deadbeef build=abc123"),
        );
        fs::write(&path, &old).expect("writes");
        let store = FileStore::new(&path).with_build("def456");
        let s = store.load().expect("loads").expect("a world");
        assert!(backups(&dir).is_empty(), "a load alone writes nothing");

        store.save(&s).expect("saves");
        let bak = dir.join("world.dat.f0.deadbeef-abc123.bak");
        assert_eq!(fs::read_to_string(&bak).expect("a backup"), old);
        let now = fs::read_to_string(&path).expect("the new file");
        assert!(
            now.lines()
                .next()
                .is_some_and(|h| h.contains(&format!("format={}", format_id()))
                    && h.contains("build=def456")),
            "{now:?}"
        );
        store.save(&s).expect("saves again");
        assert_eq!(backups(&dir).len(), 1, "one backup per format change");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A file from before the header named a format is a format change too:
    /// the owner's first IP11 start backs it up.
    #[test]
    fn a_legacy_header_is_backed_up_too() {
        let dir = scratch("legacy");
        let path = dir.join("world.dat");
        let old = with_header(&render(&sample()), &format!("{MAGIC} {SCHEMA}"));
        fs::write(&path, &old).expect("writes");
        let store = FileStore::new(&path).with_build("def456");
        let s = store.load().expect("loads").expect("a world");
        store.save(&s).expect("saves");
        assert_eq!(
            backups(&dir),
            vec!["world.dat.flegacy-unknown.bak".to_owned()]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// The same format from another commit is not a change: shas are not
    /// ordered, and a day of landings must not churn the backups (M9).
    #[test]
    fn the_same_format_from_another_build_makes_no_backup() {
        let dir = scratch("samefmt");
        let path = dir.join("world.dat");
        FileStore::new(&path)
            .with_build("aaa111")
            .save(&sample())
            .expect("saves");
        let store = FileStore::new(&path).with_build("bbb222");
        let s = store.load().expect("loads").expect("a world");
        store.save(&s).expect("saves");
        assert!(backups(&dir).is_empty(), "{:?}", backups(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A file holding a record this build does not know is not saved over:
    /// that save would drop the record. The session goes beside it instead,
    /// and `--accept-downgrade` lets the save through after a backup.
    #[test]
    fn skipped_records_block_the_save() {
        let dir = scratch("skipped");
        let path = dir.join("world.dat");
        let text = format!("{}futurething\tx\ty\n", render(&sample()));
        fs::write(&path, &text).expect("writes");

        let store = FileStore::new(&path).with_build("def456");
        let s = store.load().expect("loads").expect("a world");
        store
            .save(&s)
            .expect("a refused save is diverted, not an error");
        assert_eq!(fs::read_to_string(&path).expect("reads"), text, "untouched");
        assert!(
            dir.join("world.dat.downgrade").exists(),
            "the session is kept"
        );
        assert!(backups(&dir).is_empty());

        let store = FileStore::new(&path)
            .with_build("def456")
            .accept_downgrade(true);
        let s = store.load().expect("loads").expect("a world");
        store.save(&s).expect("saves");
        let bak = backups(&dir);
        assert_eq!(bak.len(), 1, "backed up first: {bak:?}");
        assert_eq!(fs::read_to_string(dir.join(&bak[0])).expect("reads"), text);
        assert!(
            !fs::read_to_string(&path)
                .expect("reads")
                .contains("futurething")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A higher `STORE_FORMAT` is a downgrade even with nothing skipped.
    #[test]
    fn a_higher_store_format_blocks_the_save() {
        let dir = scratch("higher");
        let path = dir.join("world.dat");
        let text = with_header(
            &render(&sample()),
            &format!(
                "{MAGIC} {SCHEMA} format={}.00000000 build=x",
                STORE_FORMAT + 1
            ),
        );
        fs::write(&path, &text).expect("writes");
        let store = FileStore::new(&path);
        let s = store.load().expect("loads").expect("a world");
        store.save(&s).expect("diverted");
        assert_eq!(fs::read_to_string(&path).expect("reads"), text);
        let _ = fs::remove_dir_all(&dir);
    }

    /// While the registry is tainted, the good file is never written: the
    /// save lands in `<file>.tainted`, whatever else the store decided.
    #[test]
    fn a_tainted_world_never_overwrites_the_good_file() {
        let dir = scratch("tainted");
        let path = dir.join("world.dat");
        let store = FileStore::new(&path).with_build("def456");
        store.save(&sample()).expect("saves");
        let good = fs::read_to_string(&path).expect("reads");

        let mut bad = sample();
        bad.balances[0].1 = -1.0;
        bad.tainted = true;
        store.save(&bad).expect("diverted");
        store.save(&bad).expect("and again");
        assert_eq!(fs::read_to_string(&path).expect("reads"), good);
        let side = fs::read_to_string(dir.join("world.dat.tainted")).expect("forensic copy");
        assert!(parse(&side).expect("parses").balances[0].1 < 0.0);

        // Through the registry: a tainted registry's snapshot says so.
        let mut reg = crate::service::Registry::new(crate::model::SandboxConfig::default());
        assert!(!reg.to_snapshot().tainted);
        reg.taint();
        assert!(reg.to_snapshot().tainted);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Backups beyond the kept count go, oldest first, and never the new
    /// one. The rule, not the number.
    #[test]
    fn backups_beyond_the_kept_count_are_pruned() {
        let dir = scratch("prune");
        let path = dir.join("world.dat");
        for n in 0..BACKUPS_KEPT + 2 {
            fs::write(dir.join(format!("world.dat.f0.{n:08x}-old.bak")), "x").expect("w");
        }
        fs::write(dir.join("other.dat.f0.0-old.bak"), "x").expect("w");
        let old = with_header(
            &render(&sample()),
            &format!("{MAGIC} {SCHEMA} format=0.feedface build=prev"),
        );
        fs::write(&path, &old).expect("writes");
        let store = FileStore::new(&path).with_build("def456");
        let s = store.load().expect("loads").expect("a world");
        store.save(&s).expect("saves");
        let ours: Vec<String> = backups(&dir)
            .into_iter()
            .filter(|n| n.starts_with("world.dat."))
            .collect();
        // The newest BACKUPS_KEPT, plus the oldest of format 0.
        assert_eq!(ours.len(), BACKUPS_KEPT + 1, "{ours:?}");
        assert!(ours.contains(&"world.dat.f0.00000000-old.bak".to_owned()));
        assert!(ours.contains(&"world.dat.f0.feedface-prev.bak".to_owned()));
        assert!(
            dir.join("other.dat.f0.0-old.bak").exists(),
            "not ours to prune"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    fn at(secs: u64) -> std::time::SystemTime {
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)
    }

    /// The oldest backup of each format survives any number of newer ones;
    /// the rest go beyond the newest N. Invented names.
    #[test]
    fn the_oldest_backup_of_each_format_is_never_pruned() {
        let mut v = vec![
            (at(10), "w.f3.aaaa0001-x.bak".to_owned()),
            (at(20), "w.f3.aaaa0002-x.bak".to_owned()),
            (at(30), "w.f4.bbbb0001-x.bak".to_owned()),
        ];
        for n in 0..8u64 {
            v.push((at(100 + n), format!("w.f4.cccc{n:04}-x.bak")));
        }
        v.push((at(5), "other.f4.0-x.bak".to_owned()));
        v.push((at(6), "w.fz.0-x.bak".to_owned()));
        let gone = backups_to_delete(&v, "w", BACKUPS_KEPT - 1);
        assert!(
            !gone.contains(&"w.f3.aaaa0001-x.bak".to_owned()),
            "f3 original"
        );
        assert!(
            !gone.contains(&"w.f4.bbbb0001-x.bak".to_owned()),
            "f4 original"
        );
        assert!(gone.contains(&"w.f3.aaaa0002-x.bak".to_owned()));
        assert!(
            !gone
                .iter()
                .any(|n| n.starts_with("other") || n.starts_with("w.fz"))
        );
        for n in 4..8u64 {
            assert!(!gone.contains(&format!("w.f4.cccc{n:04}-x.bak")));
        }
        // Few backups: nothing goes.
        assert!(backups_to_delete(&v[..2], "w", BACKUPS_KEPT - 1).is_empty());
    }

    /// A header is file content, and its fields become a file name: nothing
    /// in them can point outside the world's directory.
    #[test]
    fn a_header_cannot_name_a_path() {
        let dir = scratch("path");
        let path = dir.join("world.dat");
        let old = with_header(
            &render(&sample()),
            &format!("{MAGIC} {SCHEMA} format=../../x/y build=../../z"),
        );
        fs::write(&path, &old).expect("writes");
        let store = FileStore::new(&path);
        let s = store.load().expect("loads").expect("a world");
        store.save(&s).expect("saves");
        let bak = backups(&dir);
        assert_eq!(bak.len(), 1, "{bak:?}");
        assert!(!bak[0].contains('/'));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A save killed before its rename (SIGTERM, a crash) leaves a stale
    /// tmp: the load never reads it and the next save replaces it.
    #[test]
    fn a_stale_tmp_from_a_killed_save_is_ignored() {
        let dir = scratch("tmp");
        let path = dir.join("world.dat");
        let store = FileStore::new(&path);
        store.save(&sample()).expect("saves");
        fs::write(dir.join("world.dat.tmp"), "eve-offline-world 2\nbalance\t1").expect("w");
        let mut want = sample();
        settle(&mut want, 0);
        assert_eq!(store.load().expect("loads"), Some(want));
        store.save(&sample()).expect("saves over the stale tmp");
        assert!(!dir.join("world.dat.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// A sample with a different balance, so two saves can be told apart.
    fn sample_with(balance: f64) -> Snapshot {
        let mut s = sample();
        s.balances[0].1 = balance;
        s
    }

    fn loaded(path: &Path) -> Snapshot {
        FileStore::new(path)
            .load()
            .expect("loads")
            .expect("a world")
    }

    /// CP52: the saver writes on its own thread, and when saves arrive
    /// faster than it writes, the newest one is what lands.
    #[test]
    fn the_saver_writes_the_newest_snapshot() {
        let dir = scratch("saver");
        let path = dir.join("world.dat");
        let saver = Saver::spawn(FileStore::new(&path), None).expect("spawns");
        for n in 1..=20 {
            saver.save(&sample_with(f64::from(n))).expect("handed over");
        }
        saver.flush();
        assert_eq!(loaded(&path).balances[0].1, 20.0);
        assert_eq!(saver.load().expect("loads"), Some(loaded(&path)));
        let _ = fs::remove_dir_all(&dir);
    }

    /// E1b: a saver's save reports its render and size to the caller with
    /// no write, and the write arrives through `take_writes`: every
    /// generation accounted for as written or coalesced, the newest last,
    /// and taken once.
    #[test]
    fn the_savers_writes_are_timed_off_the_caller() {
        let dir = scratch("saver-timed");
        let path = dir.join("world.dat");
        let saver = Saver::spawn(FileStore::new(&path), None).expect("spawns");
        for n in 1..=5 {
            let cost = saver
                .save_timed(&sample_with(f64::from(n)))
                .expect("handed over");
            assert_eq!(cost.write, Duration::ZERO, "the write is not the caller's");
            assert_eq!(cost.render, Duration::ZERO, "nor is the render (PERF-SAVE)");
        }
        saver.flush();
        let w = saver.take_writes();
        assert_eq!(w.writes + w.coalesced, 5, "{w:?}");
        assert_eq!(w.generation, 5);
        let direct = FileStore::new(dir.join("direct.dat"))
            .save_timed(&sample_with(1.0))
            .expect("saves");
        let bytes = direct.bytes;
        assert!(w.bytes >= bytes && w.max <= w.total, "{w:?}");
        assert_eq!(saver.take_writes(), SaveWrites::default(), "taken once");
        // A store that writes on the caller times its write there instead.
        assert!(direct.bytes > 0);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Shutdown flushes the last snapshot: `close` with a save still queued
    /// writes it before returning, and a save after `close` is dropped.
    #[test]
    fn close_writes_the_last_snapshot_and_refuses_later_ones() {
        let dir = scratch("close");
        let path = dir.join("world.dat");
        let saver = Saver::spawn(FileStore::new(&path), None).expect("spawns");
        saver.save(&sample_with(7.0)).expect("handed over");
        saver.close();
        assert_eq!(loaded(&path).balances[0].1, 7.0);
        saver.save(&sample_with(8.0)).expect("dropped quietly");
        saver.close();
        assert_eq!(loaded(&path).balances[0].1, 7.0, "nothing after close");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The world's lock file (ACC2) is held by the saver until its last
    /// write is done: `--grant` cannot take it while the saver runs, and
    /// can once it is closed.
    #[test]
    fn the_saver_holds_the_world_lock_until_closed() {
        let dir = scratch("lock");
        let path = dir.join("world.dat");
        let lock = crate::account_book::lock_world(&path).expect("locks");
        let saver = Saver::spawn(FileStore::new(&path), Some(lock)).expect("spawns");
        saver.save(&sample()).expect("handed over");
        saver.flush();
        assert!(
            crate::account_book::lock_world(&path).is_err(),
            "still held"
        );
        drop(saver);
        assert!(crate::account_book::lock_world(&path).is_ok(), "released");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A crash in the middle of an off-lock write leaves the previous
    /// world: the write is still tmp + rename, so a half-written tmp is
    /// never read, and the next saver writes over it.
    #[test]
    fn a_crash_mid_write_keeps_the_previous_world() {
        let dir = scratch("crash");
        let path = dir.join("world.dat");
        {
            let saver = Saver::spawn(FileStore::new(&path), None).expect("spawns");
            saver.save(&sample_with(1.0)).expect("handed over");
            saver.close();
        }
        // The crash: the saver died with half a world in its tmp.
        let half = render(&sample_with(2.0));
        fs::write(dir.join("world.dat.tmp"), &half[..half.len() / 2]).expect("w");
        assert_eq!(loaded(&path).balances[0].1, 1.0, "the old world, whole");
        let saver = Saver::spawn(FileStore::new(&path), None).expect("spawns");
        saver.save(&sample_with(3.0)).expect("handed over");
        saver.close();
        assert_eq!(loaded(&path).balances[0].1, 3.0);
        assert!(!dir.join("world.dat.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// PERF-SAVE: a crash after the tmp is written and synced but before the
    /// rename (the worst instant) leaves the previous world whole, and the
    /// next save goes through.
    #[test]
    fn a_crash_between_the_tmp_write_and_the_rename_keeps_the_old_world() {
        let dir = scratch("crash-before-rename");
        let path = dir.join("world.dat");
        let store = FileStore::new(&path);
        store.save(&sample_with(1.0)).expect("saves");
        let new_text = store.render(&sample_with(2.0));
        let crashed = write_atomic_with(&path, new_text.as_bytes(), || {
            Err(io::Error::other("killed before the rename"))
        });
        assert!(crashed.is_err());
        assert_eq!(loaded(&path).balances[0].1, 1.0, "the old world, whole");
        let tmp = fs::read_to_string(dir.join("world.dat.tmp")).expect("the tmp is there");
        assert_eq!(tmp, new_text, "the new world sat complete in the tmp");
        store
            .save(&sample_with(3.0))
            .expect("the next save goes through");
        assert_eq!(loaded(&path).balances[0].1, 3.0);
        let _ = fs::remove_dir_all(&dir);
    }

    /// PERF-SAVE: the saver renders off the caller's thread, and what it
    /// writes is exactly what a render under the lock would have been.
    #[test]
    fn an_owned_save_writes_the_same_world_as_a_render() {
        let dir = scratch("owned-save");
        let path = dir.join("world.dat");
        let saver = Saver::spawn(FileStore::new(&path), None).expect("spawns");
        let cost = saver.save_owned(sample_with(7.0)).expect("handed over");
        assert_eq!(
            cost.render,
            Duration::ZERO,
            "nothing rendered under the lock"
        );
        saver.close();
        assert_eq!(
            fs::read_to_string(&path).expect("written"),
            FileStore::new(&path).render(&sample_with(7.0)),
            "byte for byte the render under the lock"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// IP11's divert still applies off the lock: a tainted snapshot,
    /// rendered under it, goes to `<file>.tainted` and never over the file.
    #[test]
    fn the_saver_keeps_the_tainted_divert() {
        let dir = scratch("saver-taint");
        let path = dir.join("world.dat");
        let saver = Saver::spawn(FileStore::new(&path), None).expect("spawns");
        saver.save(&sample_with(1.0)).expect("handed over");
        saver.flush();
        let mut bad = sample_with(-1.0);
        bad.tainted = true;
        saver.save(&bad).expect("handed over");
        saver.close();
        assert_eq!(loaded(&path).balances[0].1, 1.0);
        assert!(dir.join("world.dat.tainted").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// The format moves with the set of kinds and not with their order.
    #[test]
    fn the_format_hashes_the_set_of_kinds() {
        assert_eq!(kinds_hash_of(&["a", "b"]), kinds_hash_of(&["b", "a"]));
        assert_ne!(kinds_hash_of(&["a", "b"]), kinds_hash_of(&["a", "b", "c"]));
        assert_ne!(kinds_hash_of(&["ab"]), kinds_hash_of(&["a", "b"]));
        assert!(format_id().starts_with(&format!("{STORE_FORMAT}.")));
    }

    /// PR-SEAM-2: no module adds nothing to the format (so the core server's
    /// files are as before); a module's name and version add one `.nameVn`,
    /// in a spelling a header keeps; another version is another format.
    #[test]
    fn a_modules_version_is_part_of_the_format() {
        assert_eq!(module_suffix(&[]), "");
        assert_eq!(format_id(), format!("{STORE_FORMAT}.{:08x}", kinds_hash()));
        assert_eq!(module_suffix(&[("econ", 1)]), ".econv1");
        assert_eq!(
            module_suffix(&[("econ", 1), ("a-b", 12), ("--", 3)]),
            ".econv1.abv12"
        );
        assert_ne!(module_suffix(&[("econ", 1)]), module_suffix(&[("econ", 2)]));
        let header = format!("x.{:08x}{}", 7, module_suffix(&[("econ", 1)]));
        assert_eq!(clean(&header, "unknown"), header, "survives `clean`");
    }

    // ---- Q-FORMAT: worlds across builds with different delta kinds ------

    /// One invented row per kind in `kinds` (rows of a kind this build does
    /// not know are kept raw, so the shape is free).
    fn q_snapshot(kinds: &[&str]) -> Snapshot {
        let mut s = sample();
        for (i, k) in kinds.iter().enumerate() {
            s.deltas.put_verbatim(
                k,
                vec![format!("{}", 7000 + i), "k\tab".to_owned()],
                vec![format!("{i}.5"), String::new(), "tail".to_owned()],
            );
        }
        s
    }

    fn delta_lines(text: &str) -> Vec<String> {
        let mut v: Vec<String> = text
            .lines()
            .filter(|l| l.starts_with("delta\t"))
            .map(str::to_owned)
            .collect();
        v.sort();
        v
    }

    /// A world saved by an OLDER build (same `STORE_FORMAT`, fewer kinds, so
    /// another kinds hash) loads here with every row, is copied aside before
    /// the first save, is not diverted, and is rewritten under today's format.
    #[test]
    fn a_world_from_a_build_with_fewer_kinds_loads_intact_and_is_backed_up() {
        let dir = scratch("qfewer");
        let path = dir.join("world.dat");
        let all: Vec<&str> = crate::delta_kinds::KINDS.iter().map(|(k, _)| *k).collect();
        let subset: Vec<&str> = all.iter().copied().step_by(2).collect();
        assert!(subset.len() < all.len() && !subset.is_empty());
        let old_hash = kinds_hash_of(&subset);
        assert_ne!(old_hash, kinds_hash());
        let old = with_header(
            &render(&q_snapshot(&subset)),
            &format!("{MAGIC} {SCHEMA} format={STORE_FORMAT}.{old_hash:08x} build=oldsha"),
        );
        fs::write(&path, &old).expect("writes");

        let store = FileStore::new(&path).with_build("newsha");
        let s = store.load().expect("loads").expect("a world");
        assert_eq!(
            s.deltas.kinds().map(|(_, r)| r.len()).sum::<usize>(),
            delta_lines(&old).len(),
            "every row of the old world is loaded"
        );
        store.save(&s).expect("saves");

        let bak = dir.join(format!(
            "world.dat.f{STORE_FORMAT}.{old_hash:08x}-oldsha.bak"
        ));
        assert_eq!(fs::read_to_string(&bak).expect("a backup"), old);
        assert!(!dir.join("world.dat.downgrade").exists(), "not diverted");
        let now = fs::read_to_string(&path).expect("the new file");
        assert_eq!(delta_lines(&now), delta_lines(&old), "every row survives");
        assert!(now.lines().next().is_some_and(
            |h| h.contains(&format!("format={}", format_id())) && h.contains("build=newsha")
        ));
        // And the rows, now added to, still load.
        let again = FileStore::new(&path).load().expect("loads").expect("world");
        assert_eq!(
            again.deltas.kinds().map(|(_, r)| r.len()).sum::<usize>(),
            delta_lines(&old).len()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A world saved by a NEWER build (same `STORE_FORMAT`, kinds this build
    /// does not list) is not refused and loses nothing: the unknown kinds are
    /// kept verbatim, backed up first, and written back on the next save.
    #[test]
    fn a_world_with_kinds_this_build_does_not_list_keeps_them_verbatim() {
        let dir = scratch("qmore");
        let path = dir.join("world.dat");
        let mut kinds: Vec<&str> = crate::delta_kinds::KINDS.iter().map(|(k, _)| *k).collect();
        kinds.extend(["zzfuturea", "zzfutureb"]);
        let theirs = kinds_hash_of(&kinds);
        let text = with_header(
            &render(&q_snapshot(&kinds)),
            &format!("{MAGIC} {SCHEMA} format={STORE_FORMAT}.{theirs:08x} build=futuresha"),
        );
        fs::write(&path, &text).expect("writes");

        let store = FileStore::new(&path).with_build("newsha");
        let s = store.load().expect("loads, not refused").expect("a world");
        store.save(&s).expect("saves");
        assert!(!dir.join("world.dat.downgrade").exists(), "not diverted");
        assert_eq!(backups(&dir).len(), 1, "{:?}", backups(&dir));
        let bak = &backups(&dir)[0];
        assert_eq!(fs::read_to_string(dir.join(bak)).expect("reads"), text);
        let now = fs::read_to_string(&path).expect("reads");
        assert_eq!(delta_lines(&now), delta_lines(&text), "no row dropped");
        assert!(now.contains("zzfuturea") && now.contains("zzfutureb"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// The same, through the live registry (`apply_snapshot` / `to_snapshot`),
    /// which is the path the running server takes: one row of every kind this
    /// build lists plus an unlisted one comes out as it went in.
    #[test]
    fn every_listed_kind_and_an_unlisted_one_survive_the_registry() {
        let mut kinds: Vec<&str> = crate::delta_kinds::KINDS.iter().map(|(k, _)| *k).collect();
        kinds.push("zzfuturea");
        let text = render(&q_snapshot(&kinds));
        let mut reg = crate::service::Registry::new(crate::model::SandboxConfig::default());
        reg.apply_snapshot(parse(&text).expect("parses"));
        let out = render(&reg.to_snapshot());
        assert_eq!(delta_lines(&out), delta_lines(&text));
    }

    /// A world from a build with a HIGHER `STORE_FORMAT` is never saved over
    /// (an older build cannot know what an existing row now means): the
    /// session is diverted, the file untouched.
    #[test]
    fn a_higher_format_with_unknown_kinds_is_diverted_not_overwritten() {
        let dir = scratch("qhigher");
        let path = dir.join("world.dat");
        let text = with_header(
            &render(&q_snapshot(&["zzfuturea"])),
            &format!(
                "{MAGIC} {SCHEMA} format={}.0badcafe build=futuresha",
                STORE_FORMAT + 1
            ),
        );
        fs::write(&path, &text).expect("writes");
        let store = FileStore::new(&path).with_build("newsha");
        let s = store.load().expect("loads").expect("a world");
        store.save(&s).expect("diverted, not an error");
        assert_eq!(fs::read_to_string(&path).expect("reads"), text);
        assert!(dir.join("world.dat.downgrade").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- FX3: the pilot's station survives a restart --------------------

    /// Invented ids (no client data in tests). Alice is seeded at the
    /// `Character` default's station; Bob at a second station in her
    /// system, which is how this server knows it exists.
    const FX3_ALICE: i64 = 1_000_001;
    const FX3_ALICE_SHIP: i64 = 100_000_001;
    const FX3_BOB: i64 = 1_000_002;
    const FX3_SECOND: i64 = 69_000_002;

    fn fx3_config() -> crate::model::SandboxConfig {
        let alice = crate::model::Character {
            character_id: FX3_ALICE,
            name: "Alice".to_owned(),
            ship_id: FX3_ALICE_SHIP,
            ..crate::model::Character::default()
        };
        let bob = crate::model::Character {
            character_id: FX3_BOB,
            name: "Bob".to_owned(),
            ship_id: 100_000_002,
            station_id: Some(FX3_SECOND),
            station_owner_id: 1_999_002,
            station_type_id: 990_002,
            station_operation_id: 99,
            ..alice.clone()
        };
        crate::model::SandboxConfig {
            characters: vec![alice, bob],
            ..crate::model::SandboxConfig::default()
        }
    }

    /// Alice, undocked and then docked at Bob's station, as `CmdDock` does
    /// it (FX1): `dock_target`, `dock_at`, then the session change.
    fn fx3_docked_at_second() -> crate::service::Registry {
        let mut reg = crate::service::Registry::new(fx3_config());
        crate::destiny::on_ballpark_bind(&mut reg, Some(FX3_ALICE));
        let _ = reg.take_pending();
        let target = reg.dock_target(FX3_ALICE, FX3_SECOND).expect("dockable");
        assert!(reg.dock_at(FX3_ALICE, target));
        reg.dock_change(Some(FX3_ALICE)).expect("a dock change");
        assert_eq!(reg.docked_station(FX3_ALICE), Some(FX3_SECOND), "setup");
        reg
    }

    /// `reg`'s world saved, a fresh server from the same flags, the file
    /// applied: a restart.
    fn fx3_restart(name: &str, text: &str) -> crate::service::Registry {
        let dir = scratch(name);
        let path = dir.join("world.dat");
        fs::write(&path, text).expect("writes");
        let snap = FileStore::new(&path)
            .load()
            .expect("loads")
            .expect("a world");
        let _ = fs::remove_dir_all(&dir);
        let mut reg = crate::service::Registry::new(fx3_config());
        reg.apply_snapshot(snap);
        reg
    }

    /// The seven fields of a record, for comparing whole stations.
    fn fx3_station(reg: &crate::service::Registry, id: i64) -> Option<StationRecord> {
        StationRecord::of(reg.character(id).expect("a character"))
    }

    /// **Docked at a second station, saved, restarted: still there**
    /// (`docs/port-tracker.md` §72.1). Every field of her station is Bob's,
    /// her ship is in its hangar, she reads as docked there, and the undock
    /// leaves from it.
    #[test]
    fn a_pilot_docked_at_a_second_station_is_there_after_a_restart() {
        let before = fx3_docked_at_second();
        let want = fx3_station(&before, FX3_BOB);
        let mut reg = fx3_restart("fx3-second", &render(&before.to_snapshot()));

        assert_eq!(fx3_station(&reg, FX3_ALICE), want, "her station, all of it");
        assert_eq!(reg.docked_station(FX3_ALICE), Some(FX3_SECOND));
        let ship = reg.character(FX3_ALICE).map(|c| c.ship_id);
        assert_eq!(ship, Some(FX3_ALICE_SHIP), "the same active ship");
        let item = reg.store().get(FX3_ALICE_SHIP).expect("her ship");
        assert_eq!(
            (item.location_id, item.flag_id),
            (FX3_SECOND, crate::model::FLAG_HANGAR),
            "in the second station's hangar"
        );
        let out = reg.undock_change(Some(FX3_ALICE)).expect("an undock");
        let station = crate::pyutil::pairs_get(&out, b"stationid").cloned();
        assert_eq!(
            station,
            Some(eve_marshal::PyValue::Tuple(vec![
                eve_marshal::PyValue::Int(FX3_SECOND),
                eve_marshal::PyValue::None
            ])),
            "and undocks from it"
        );
        // Bob is untouched: one pilot's dock is not another's.
        assert_eq!(fx3_station(&reg, FX3_BOB), want);
    }

    /// **The clone capsule's station does not win on load** (FX1's second
    /// follow-up). A revive records the capsule and the station it was put
    /// in; when she docks elsewhere afterwards, the character record is the
    /// newer word, so the load seats her where she last docked.
    #[test]
    fn a_clone_capsule_record_does_not_reseat_a_pilot_who_docked_since() {
        let before = fx3_docked_at_second();
        let home = crate::model::Character::default();
        let old = StationRecord::of(&home).expect("the seed station");
        assert_ne!(old.station_id, FX3_SECOND, "setup: two stations");
        // The record a revive at home would have left (`Clones::render`).
        let text = format!(
            "{}clonecapsule\t{FX3_ALICE}\t{FX3_ALICE_SHIP}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            render(&before.to_snapshot()),
            old.station_id,
            old.owner_id,
            old.type_id,
            old.operation_id,
            old.solar_system_id,
            old.constellation_id,
            old.region_id
        );
        let reg = fx3_restart("fx3-capsule", &text);
        assert_eq!(
            reg.character(FX3_ALICE).and_then(|c| c.station_id),
            Some(FX3_SECOND)
        );
        assert_eq!(
            reg.character(FX3_ALICE).map(|c| c.ship_id),
            Some(FX3_ALICE_SHIP),
            "the capsule is still seated"
        );
    }

    /// **A format-2 world loads unchanged**: its `character` records carry
    /// no station, so every pilot is where the seed puts them, exactly as
    /// that build would have loaded it; and the file is backed up before
    /// the first save writes the new format over it.
    #[test]
    fn a_format_2_world_loads_unchanged_and_is_backed_up() {
        const { assert!(STORE_FORMAT > 2, "the station field is a format bump") };
        let reg = crate::service::Registry::new(fx3_config());
        let mut snap = reg.to_snapshot();
        for c in &mut snap.characters {
            c.station = None;
            c.identity = None;
        }
        let text = with_header(
            &render(&snap),
            &format!("{MAGIC} {SCHEMA} format=2.{:08x} build=old", kinds_hash()),
        );
        assert!(
            text.lines()
                .filter(|l| l.starts_with("character\t"))
                .all(|l| l.split('\t').count() == 5),
            "setup: format-2 records"
        );

        let dir = scratch("fx3-format2");
        let path = dir.join("world.dat");
        fs::write(&path, &text).expect("writes");
        let store = FileStore::new(&path).with_build("new");
        let loaded = store.load().expect("loads").expect("a world");
        assert!(loaded.characters.iter().all(|c| c.station.is_none()));
        let mut after = crate::service::Registry::new(fx3_config());
        after.apply_snapshot(loaded);
        for id in [FX3_ALICE, FX3_BOB] {
            let key = |r: &crate::service::Registry| {
                let c = r.character(id).expect("a character");
                (StationRecord::of(c), c.ship_id, c.owner_user_id)
            };
            assert_eq!(key(&after), key(&reg), "character {id} as seeded");
        }
        store.save(&after.to_snapshot()).expect("saves");
        assert_eq!(backups(&dir).len(), 1, "{:?}", backups(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A station is all seven fields or the world is refused: half of one
    /// would seat a pilot in a system that does not hold it.
    #[test]
    fn a_character_with_half_a_station_is_refused() {
        let short = format!("{MAGIC} {SCHEMA}\ncharacter\t1\t2\t-\tX\t69000002\t1\n");
        assert!(parse(&short).is_err());
        let bad = format!("{MAGIC} {SCHEMA}\ncharacter\t1\t2\t-\tX\t1\t2\t3\t4\tfive\t6\t7\n");
        assert!(parse(&bad).is_err());
        let whole = format!("{MAGIC} {SCHEMA}\ncharacter\t1\t2\t-\tX\t1\t2\t3\t4\t5\t6\t7\n");
        let s = parse(&whole).expect("seven fields read");
        assert_eq!(s.characters[0].station.map(|st| st.region_id), Some(7));
    }

    /// **Who a pilot is survives a restart** (P-START-3): race, bloodline,
    /// gender, school and corporation are saved, and a fresh server from
    /// the same flags takes them over the seed's -- with a station (17
    /// fields) and without one (10).
    #[test]
    fn a_pilot_s_identity_survives_a_save_and_reload() {
        const { assert!(STORE_FORMAT > 3, "the identity fields are a format bump") };
        let mut before = crate::service::Registry::new(fx3_config());
        let who = IdentityRecord {
            race_id: 91,
            bloodline_id: 901,
            gender_id: 0,
            school_id: 99,
            corporation_id: 1_999_001,
        };
        who.apply(before.character_mut(FX3_ALICE).expect("alice"));
        before.character_mut(FX3_BOB).expect("bob").station_id = None;
        let text = render(&before.to_snapshot());
        let counts: Vec<usize> = text
            .lines()
            .filter(|l| l.starts_with("character\t"))
            .map(|l| l.split('\t').count())
            .collect();
        assert_eq!(counts, vec![17, 10], "setup: with and without a station");
        let reg = fx3_restart("pstart3-identity", &text);
        let got = |id| IdentityRecord::of(reg.character(id).expect("a character"));
        assert_eq!(got(FX3_ALICE), who);
        assert_eq!(
            got(FX3_BOB),
            IdentityRecord::of(before.character(FX3_BOB).expect("bob"))
        );
        assert_ne!(got(FX3_BOB), who, "one pilot's identity is not another's");
    }

    /// **A format-3 world loads unchanged**: its records (5 and 12 fields)
    /// carry no identity, so the seed's stands; the file is backed up before
    /// the first save writes format 4 over it.
    #[test]
    fn a_format_3_world_loads_with_the_seed_identity_and_is_backed_up() {
        let mut seeded = fx3_config();
        seeded.characters[0].race_id = 91;
        seeded.characters[0].school_id = 99;
        let reg = crate::service::Registry::new(seeded.clone());
        let mut snap = reg.to_snapshot();
        for c in &mut snap.characters {
            c.identity = None;
        }
        snap.characters[1].station = None;
        let text = with_header(
            &render(&snap),
            &format!("{MAGIC} {SCHEMA} format=3.{:08x} build=old", kinds_hash()),
        );
        let counts: Vec<usize> = text
            .lines()
            .filter(|l| l.starts_with("character\t"))
            .map(|l| l.split('\t').count())
            .collect();
        assert_eq!(counts, vec![12, 5], "setup: format-3 records");

        let dir = scratch("pstart3-format3");
        let path = dir.join("world.dat");
        fs::write(&path, &text).expect("writes");
        let store = FileStore::new(&path).with_build("new");
        let loaded = store.load().expect("loads").expect("a world");
        assert!(loaded.characters.iter().all(|c| c.identity.is_none()));
        let mut after = crate::service::Registry::new(seeded);
        after.apply_snapshot(loaded);
        for id in [FX3_ALICE, FX3_BOB] {
            let key = |r: &crate::service::Registry| {
                IdentityRecord::of(r.character(id).expect("a character"))
            };
            assert_eq!(key(&after), key(&reg), "character {id} as seeded");
        }
        store.save(&after.to_snapshot()).expect("saves");
        assert_eq!(backups(&dir).len(), 1, "{:?}", backups(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A record of any length but 5, 10, 12 or 17 is fatal: a half identity
    /// would give a pilot another race's bloodline.
    #[test]
    fn a_character_with_half_an_identity_is_refused() {
        let line = |n: usize| {
            let extra: Vec<String> = (1..=n).map(|i| i.to_string()).collect();
            format!(
                "{MAGIC} {SCHEMA}\ncharacter\t1\t2\t-\tX\t{}\n",
                extra.join("\t")
            )
        };
        for n in [1, 4, 6, 8, 11, 13] {
            assert!(parse(&line(n)).is_err(), "{} fields", n + 5);
        }
        let s = parse(&line(5)).expect("ten fields read");
        assert_eq!(s.characters[0].station, None);
        assert_eq!(s.characters[0].identity.map(|i| i.corporation_id), Some(5));
        let s = parse(&line(12)).expect("seventeen fields read");
        assert_eq!(s.characters[0].station.map(|st| st.region_id), Some(7));
        assert_eq!(s.characters[0].identity.map(|i| i.race_id), Some(8));
    }
}
