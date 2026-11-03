//! **Ship SKINs** (L2 SKINS, `handback/r-nes-econ/report.md` §2.2, F9-F11):
//! licences, activating a licence item, applying a SKIN to a ship, and the
//! gateway state that shows it to everyone else.
//!
//! ## What the client does **[V]**
//!
//! - Ownership is machoNet (`shipCosmeticsMgr`, [`crate::service::cosmetics`]):
//!   the licences a pilot holds (`.skinID, .expires, .isSingleUse`),
//!   `ActivateSkinLicense(itemIDs)` and `ApplySkinToShip(shipID, skinID|None)`.
//! - The **applied** SKIN is the gateway (`cosmetics/client/
//!   shipSkinStateController.py`): `eve_public.cosmetic.ship.api.GetRequest`
//!   (a ship id) answers a `State{character, ship, skin}`, `GetAllInBubble`
//!   the states in the asker's park, and `SetNotice{state}` re-skins a live
//!   ship on any client that holds it. A viewer who joins later asks after 3 s.
//!   So **a friend sees your SKIN because the gateway says so**, with no slim
//!   item field.
//!
//! ## Data
//!
//! The bundle's `skins/licenses.tsv` and `skins/skins.tsv`
//! (`tools/content_skins.py`, the `skins` section): the client build's own
//! tables, as one content table ([`TABLE`]). With no bundle the table is empty
//! and no SKIN exists.
//!
//! ## What is persisted, and why by licence type
//!
//! Between client builds two SKIN ids swapped meaning (port review
//! r-port-diff §4), so **nothing here is keyed or valued by SKIN id**. Delta
//! kind [`KIND`]:
//!
//! - `lic char lic_type -> expires single_use bound`: a licence a character
//!   holds. `expires` is a FILETIME, `0` is permanent. `bound` is the ship a
//!   single-use licence was applied to (`0` none). At most one row per SKIN
//!   per character: activating another licence of the same SKIN replaces it
//!   (a temporary one extends, a permanent one wins).
//! - `app ship -> lic_type`: the SKIN on a ship.
//!
//! The SKIN id is looked up in the table at read time. An applied SKIN counts
//! only while the ship's **current owner** holds an unexpired licence for it
//! that is not bound to another ship, as the client's own `AppliedSkinStorage`
//! does. A single-use licence **stays held** (the client still lists it and
//! `GetFirstPartySkinData` still answers) until it expires; it is bound to
//! the first ship it is applied to and refused on any other.
//!
//! ## Authorization
//!
//! The acting character is the session's (`Request::actor`). A character id
//! the client sends is never believed: `GetLicencedSkinsForShipType(charID,
//! ...)` answers only for the actor. `GetFirstPartySkinData(licenseeID, ...)`
//! answers for **any** licensee: the client asks with a ship owner's id on
//! purpose (`preview.py`, `controller.py`), and `expires` and `isSingleUse`
//! are not secret; nothing it answers lets the asker change anything.
//! Activating needs the pilot docked ([`Registry::docked_station`]) and
//! the item in that station's hangar; applying needs the ship to be the
//! pilot's, in that hangar or flown.
//!
//! ## [A]
//!
//! - A single-use licence (the real table has 132, mostly 7 and 30 day) is
//!   bound to the first ship it is applied to, stays held until it expires,
//!   and serves no other ship. Falsifier: the client offering it for a second
//!   ship, or TQ removing it from the list on apply.
//! - Activating a temporary licence of a SKIN already held temporarily extends
//!   it from its current expiry. Falsifier: the licence window's countdown.
//! - A temporary licence's expiry is noticed lazily (on the next read), not
//!   pushed. Falsifier: a ship keeping its SKIN on screen past the expiry
//!   until something re-reads it.
//! - `SkinAlreadyApplied` is the user error for applying the SKIN a ship
//!   already wears (the client's apply service swallows exactly that key).
//! - `SetNotice` goes to every pilot (a `Target::Character` each), because
//!   the client caches the `no_skin` answer and never re-asks. Falsifier: a
//!   viewer's cache keeping the old state after an apply.

use crate::content::{Boxed, Origin, Sources, TableSpec};
use crate::delta::Deltas;
use crate::gateway::{Ground, Reply, Target, get_bytes, get_uint, put_bytes, put_varint};
use crate::item::ItemType;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Duration;

/// The delta kind (`delta_kinds::KINDS`): `lic char lic_type -> expires
/// single_use` and `app ship -> lic_type kept`.
pub const KIND: &str = "skin";
const LIC: &str = "lic";
const APP: &str = "app";

/// `categoryShipSkin` **[V]** (`inventorycommon/const.py`).
pub const CATEGORY_SHIP_SKIN: i64 = 91;
/// `categoryShip` **[V]**.
const CATEGORY_SHIP: i64 = 6;
/// FILETIME ticks (100 ns) in a day.
const DAY_TICKS: i64 = 864_000_000_000;
/// The event goes out after the answer, like the vault's (the client builds
/// its listener when the call returns).
pub const EVENT_DELAY: Duration = Duration::from_millis(400);

const LICENSES: (&str, &str) = (
    "skins/licenses.tsv",
    "#type_id\tgroup_id\tskin_id\tduration\tsingle_use\tvolume\tname",
);
const SKINS: (&str, &str) = ("skins/skins.tsv", "#skin_id\tvisible\tstructure\thulls");

const NOTICE_SET: &str = "eve_public.cosmetic.ship.api.SetNotice";
const PKG: &str = "eve_public.cosmetic.ship.api.";

// ---- the table -------------------------------------------------------------------

/// One licence type: the item a pilot holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Licence {
    pub skin_id: i64,
    /// Days; -1 is permanent.
    pub duration: i64,
    pub single_use: bool,
}

/// One SKIN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinRow {
    pub visible: bool,
    pub structure: bool,
    /// The hull type ids it fits.
    pub hulls: Vec<i64>,
}

/// The SKIN data of one client build.
#[derive(Debug, Clone, Default)]
pub struct Skins {
    licences: BTreeMap<i64, Licence>,
    skins: BTreeMap<i64, SkinRow>,
    types: Vec<ItemType>,
    /// `type_id -> position in types`, so a lookup is not a scan of ~12k rows.
    index: BTreeMap<i64, usize>,
}

fn leak(s: &str) -> &'static str {
    // Once per licence type at startup: the table is loaded once per process.
    Box::leak(s.to_owned().into_boxed_str())
}

fn int(s: &str, what: &str) -> Result<i64, String> {
    s.parse()
        .map_err(|_| format!("{what} {s:?} is not an integer"))
}

impl Skins {
    /// From the data lines of `licenses.tsv` and `skins.tsv`.
    pub fn parse<S: AsRef<str>>(licence_lines: &[S], skin_lines: &[S]) -> Result<Skins, String> {
        let mut t = Skins::default();
        for (i, line) in skin_lines.iter().enumerate() {
            let bad = |e: String| format!("{}: data line {}: {e}", SKINS.0, i + 1);
            let f: Vec<&str> = line.as_ref().split('\t').collect();
            let [id, visible, structure, hulls] = f[..] else {
                return Err(bad(format!("{} fields, want 4", f.len())));
            };
            let hulls = hulls
                .split(',')
                .filter(|h| !h.is_empty())
                .map(|h| int(h, "hull"))
                .collect::<Result<Vec<_>, _>>()
                .map_err(bad)?;
            t.skins.insert(
                int(id, "skin_id").map_err(|e| format!("{}: {e}", SKINS.0))?,
                SkinRow {
                    visible: visible == "1",
                    structure: structure == "1",
                    hulls,
                },
            );
        }
        for (i, line) in licence_lines.iter().enumerate() {
            let bad = |e: String| format!("{}: data line {}: {e}", LICENSES.0, i + 1);
            let f: Vec<&str> = line.as_ref().split('\t').collect();
            let [id, group, skin, duration, single, volume, name] = f[..] else {
                return Err(bad(format!("{} fields, want 7", f.len())));
            };
            let type_id = int(id, "type_id").map_err(bad)?;
            let skin_id = int(skin, "skin_id").map_err(bad)?;
            if !t.skins.contains_key(&skin_id) {
                return Err(bad(format!("SKIN {skin_id} is not in {}", SKINS.0)));
            }
            t.licences.insert(
                type_id,
                Licence {
                    skin_id,
                    duration: int(duration, "duration").map_err(bad)?,
                    single_use: single == "1",
                },
            );
            t.index.insert(type_id, t.types.len());
            t.types.push(ItemType {
                type_id,
                name: leak(name),
                group_id: int(group, "group_id").map_err(bad)?,
                category_id: CATEGORY_SHIP_SKIN,
                singleton: false,
                volume: volume
                    .parse()
                    .map_err(|_| bad(format!("volume {volume:?}")))?,
                capacity: 0.0,
            });
        }
        Ok(t)
    }

    pub fn is_empty(&self) -> bool {
        self.licences.is_empty()
    }

    /// The licence of an item type.
    pub fn licence(&self, type_id: i64) -> Option<&Licence> {
        self.licences.get(&type_id)
    }

    pub fn skin(&self, skin_id: i64) -> Option<&SkinRow> {
        self.skins.get(&skin_id)
    }

    /// The SKIN a licence type unlocks.
    pub fn skin_of(&self, type_id: i64) -> Option<i64> {
        self.licences.get(&type_id).map(|l| l.skin_id)
    }

    /// Whether `skin_id` fits hull type `hull`.
    pub fn fits(&self, skin_id: i64, hull: i64) -> bool {
        self.skin(skin_id).is_some_and(|s| s.hulls.contains(&hull))
    }

    /// The licence item types.
    pub fn item_types(&self) -> &[ItemType] {
        &self.types
    }

    /// One licence item type, by id.
    pub fn item(&self, type_id: i64) -> Option<&ItemType> {
        self.index.get(&type_id).map(|&i| &self.types[i])
    }
}

/// One licence item type of `content`, by id (`item::find_in`'s last fall-through).
pub fn item_in(content: &crate::content::Content, type_id: i64) -> Option<&ItemType> {
    content.try_get::<Skins>().and_then(|t| t.item(type_id))
}

/// The licence item types of `content`.
pub fn item_types_in(content: &crate::content::Content) -> &[ItemType] {
    content
        .try_get::<Skins>()
        .map_or(&[][..], Skins::item_types)
}

/// This table's registration row: one line in `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "skins",
    load: boxed_load,
    fixture: boxed_fixture,
};

fn boxed_load(src: &Sources) -> Result<Boxed, String> {
    let Some(b) = src.bundle else {
        // No bundle, no client data: no SKIN exists.
        return Ok(Box::new(Skins::default()));
    };
    let lic = b.data_lines(LICENSES.0, LICENSES.1)?;
    let skins = b.data_lines(SKINS.0, SKINS.1)?;
    if lic.is_empty() {
        src.note(Origin::Fixture);
    }
    Ok(Box::new(Skins::parse(&lic, &skins)?))
}

/// Invented rows for tests: ids are not the client's.
pub mod fixture {
    /// A permanent licence of [`SKIN_A`].
    pub const LIC_PERM: i64 = 990_801;
    /// A 30 day licence of the same SKIN.
    pub const LIC_TEMP: i64 = 990_802;
    /// A permanent licence of [`SKIN_B`], which fits another hull.
    pub const LIC_OTHER: i64 = 990_803;
    /// A single-use licence of [`SKIN_C`].
    pub const LIC_SINGLE: i64 = 990_804;
    pub const SKIN_A: i64 = 100;
    pub const SKIN_B: i64 = 101;
    pub const SKIN_C: i64 = 102;
    /// A hull no fixture SKIN fits.
    pub const HULL_NONE: i64 = 990_899;

    /// The rows, with the hull of A and C set to `hull`.
    pub fn lines(hull: i64) -> (Vec<String>, Vec<String>) {
        let lic = vec![
            format!("{LIC_PERM}\t990821\t{SKIN_A}\t-1\t0\t0.1\tFixture SKIN A"),
            format!("{LIC_TEMP}\t990821\t{SKIN_A}\t30\t0\t0.1\tFixture SKIN A 30d"),
            format!("{LIC_OTHER}\t990821\t{SKIN_B}\t-1\t0\t0.1\tFixture SKIN B"),
            format!("{LIC_SINGLE}\t990821\t{SKIN_C}\t-1\t1\t0.1\tFixture SKIN C"),
        ];
        let skins = vec![
            format!("{SKIN_A}\t1\t0\t{hull}"),
            format!("{SKIN_B}\t1\t0\t{HULL_NONE}"),
            format!("{SKIN_C}\t1\t0\t{hull}"),
        ];
        (lic, skins)
    }
}

fn boxed_fixture() -> Boxed {
    let (l, s) = fixture::lines(990_898);
    Box::new(Skins::parse(&l, &s).expect("the fixture rows are well formed"))
}

fn table(reg: &Registry) -> &Skins {
    static EMPTY: std::sync::OnceLock<Skins> = std::sync::OnceLock::new();
    reg.content()
        .try_get::<Skins>()
        .unwrap_or_else(|| EMPTY.get_or_init(Skins::default))
}

/// **Refuse to start** on a world holding SKIN rows when the content has no
/// SKIN table (a bundle extracted before the `skins` section, or another
/// build's): every licence would be unknown and every ship unskinned, silently.
pub fn check_table(reg: &Registry) -> Result<(), String> {
    if !table(reg).is_empty() || reg.deltas().of(KIND).is_empty() {
        return Ok(());
    }
    Err(
        "the world file holds SKIN licence rows but the content bundle has no skins/licenses.tsv; \
re-extract with tools/up.sh (the `skins` section of tools/content.py)"
            .to_owned(),
    )
}

// ---- licences --------------------------------------------------------------------

/// A licence a character holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub lic_type: i64,
    pub skin_id: i64,
    /// FILETIME; `None` is permanent.
    pub expires: Option<i64>,
    pub single_use: bool,
    /// A single-use licence is bound to the first ship it is applied to.
    pub bound: Option<i64>,
}

fn num(v: &[String], i: usize) -> Option<i64> {
    v.get(i)?.parse().ok()
}

fn lic_key(ch: i64, lic_type: i64) -> Vec<String> {
    vec![LIC.to_owned(), ch.to_string(), lic_type.to_string()]
}

/// Every licence row of `ch`, expired or not, whose type the table knows.
fn rows_of(d: &Deltas, t: &Skins, ch: i64) -> Vec<Held> {
    let c = ch.to_string();
    let from = vec![LIC.to_owned(), c.clone(), String::new()];
    d.of(KIND)
        .range(from..)
        .take_while(|(k, _)| k.len() == 3 && k[0] == LIC && k[1] == c)
        .filter_map(|(k, v)| {
            let lic_type: i64 = k[2].parse().ok()?;
            let expires = num(v, 0)?;
            Some(Held {
                lic_type,
                skin_id: t.skin_of(lic_type)?,
                expires: (expires != 0).then_some(expires),
                single_use: v.get(1).is_some_and(|s| s == "1"),
                bound: num(v, 2).filter(|b| *b != 0),
            })
        })
        .collect()
}

/// The licences `ch` holds at `now`.
pub fn held(reg: &Registry, ch: i64, now: i64) -> Vec<Held> {
    rows_of(reg.deltas(), table(reg), ch)
        .into_iter()
        .filter(|h| h.expires.is_none_or(|e| e >= now))
        .collect()
}

/// Why a SKIN call was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkinError {
    NoActor,
    NoSuchItem,
    NotYours,
    NotDocked,
    NotALicence,
    NotAShip,
    NotLicensed,
    WrongHull,
    AlreadyLicensed,
    AlreadyApplied,
    NotHere,
    BoundElsewhere,
}

impl SkinError {
    fn text(self) -> &'static str {
        match self {
            SkinError::NoActor => "no character",
            SkinError::NoSuchItem => "no such item",
            SkinError::NotYours => "that is not yours",
            SkinError::NotDocked => "SKIN licences are activated in a station",
            SkinError::NotALicence => "that item is not a SKIN licence",
            SkinError::NotAShip => "that is not a ship",
            SkinError::NotLicensed => "you do not hold a licence for that SKIN",
            SkinError::WrongHull => "that SKIN does not fit this hull",
            SkinError::AlreadyLicensed => "you already hold a permanent licence for that SKIN",
            SkinError::AlreadyApplied => "SkinAlreadyApplied",
            SkinError::BoundElsewhere => "that single-use licence is bound to another ship",
            SkinError::NotHere => "the ship is not in this station or flown by you",
        }
    }

    /// The marshal answer.
    pub fn raise(self) -> ServiceError {
        if self == SkinError::AlreadyApplied {
            return ServiceError::Raise(eve_proto::user_error("SkinAlreadyApplied", vec![]));
        }
        ServiceError::Raise(eve_proto::user_error(
            "CustomNotify",
            crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(self.text()))]),
        ))
    }
}

/// Give `ch` one licence of type `lic_type`, as activating it would: the row
/// of the same SKIN is replaced, a temporary one extends from its expiry.
/// Used by [`activate`]; later sources (drops, bots) call it too. Returns the
/// SKIN id. Writes nothing on `Err`.
pub fn grant(reg: &mut Registry, ch: i64, lic_type: i64, now: i64) -> Result<i64, SkinError> {
    let t = table(reg);
    let lic = t.licence(lic_type).ok_or(SkinError::NotALicence)?.clone();
    let same: Vec<Held> = rows_of(reg.deltas(), t, ch)
        .into_iter()
        .filter(|h| h.skin_id == lic.skin_id)
        .collect();
    let live = |h: &&Held| h.expires.is_none_or(|e| e >= now);
    if same.iter().filter(live).any(|h| h.expires.is_none()) {
        return Err(SkinError::AlreadyLicensed);
    }
    let expires = if lic.duration < 0 {
        0
    } else {
        let base = same
            .iter()
            .filter(live)
            .filter_map(|h| h.expires)
            .max()
            .unwrap_or(now)
            .max(now);
        base.saturating_add(lic.duration.saturating_mul(DAY_TICKS))
    };
    // Extending a licence that is already bound keeps it bound.
    let bound = same.iter().filter(live).find_map(|h| h.bound).unwrap_or(0);
    for h in &same {
        reg.deltas_mut().del(KIND, &lic_key(ch, h.lic_type));
    }
    reg.deltas_mut().put(
        KIND,
        lic_key(ch, lic_type),
        vec![
            expires.to_string(),
            if lic.single_use { "1" } else { "0" }.to_owned(),
            bound.to_string(),
        ],
    );
    reg.mark_dirty();
    Ok(lic.skin_id)
}

/// `ActivateSkinLicense(itemIDs)` for `actor`: every item must be the actor's
/// SKIN licence in the hangar of the station the actor is docked in, named
/// once, and not a permanent licence the actor already holds; then one unit of
/// each is consumed and its licence granted. All or nothing.
/// Returns the SKIN ids activated.
pub fn activate(reg: &mut Registry, actor: i64, item_ids: &[i64]) -> Result<Vec<i64>, SkinError> {
    let station = reg.docked_station(actor).ok_or(SkinError::NotDocked)?;
    let now = crate::pyutil::filetime_now();
    let mut seen = std::collections::BTreeSet::new();
    let mut plan: Vec<(i64, i64)> = Vec::new();
    for &id in item_ids {
        if !seen.insert(id) {
            continue;
        }
        let it = reg.store().get(id).ok_or(SkinError::NoSuchItem)?;
        if it.owner_id != actor {
            return Err(SkinError::NotYours);
        }
        if it.location_id != station || it.flag_id != crate::model::FLAG_HANGAR {
            return Err(SkinError::NotDocked);
        }
        if table(reg).licence(it.type_id).is_none() {
            return Err(SkinError::NotALicence);
        }
        plan.push((id, it.type_id));
    }
    // Dry run on the rows, so a refusal part-way leaves nothing changed.
    let before = reg.deltas().clone();
    let mut skins = Vec::new();
    for &(_, ty) in &plan {
        match grant(reg, actor, ty, now) {
            Ok(s) => skins.push(s),
            Err(e) => {
                *reg.deltas_mut() = before;
                return Err(e);
            }
        }
    }
    let edits: Vec<crate::item::Edit> = plan
        .iter()
        .map(|&(item_id, _)| {
            let have = reg.store().get(item_id).map_or(1, |i| i.quantity);
            if have > 1 {
                crate::item::Edit::Consume {
                    item_id,
                    quantity: 1,
                }
            } else {
                crate::item::Edit::Destroy { item_id }
            }
        })
        .collect();
    if reg.apply_items(&edits).is_err() {
        *reg.deltas_mut() = before;
        return Err(SkinError::NoSuchItem);
    }
    for &s in &skins {
        event(
            reg,
            actor,
            "OnSkinLicenseActivated",
            vec![PyValue::Int(s), PyValue::Int(actor)],
        );
    }
    Ok(skins)
}

// ---- applied ---------------------------------------------------------------------

fn app_key(ship: i64) -> Vec<String> {
    vec![APP.to_owned(), ship.to_string()]
}

/// The SKIN on `ship`, if any and still valid: its current owner holds an
/// unexpired licence for it that is not bound to another ship.
pub fn applied_skin(reg: &Registry, ship: i64) -> Option<i64> {
    let row = reg.deltas().of(KIND).get(&app_key(ship))?;
    let skin = table(reg).skin_of(num(row, 0)?)?;
    let owner = reg.store().get(ship)?.owner_id;
    let now = crate::pyutil::filetime_now();
    held(reg, owner, now)
        .iter()
        .any(|h| h.skin_id == skin && h.bound.is_none_or(|b| b == ship))
        .then_some(skin)
}

/// Whether `skin_id` fits hull type `hull` in `reg`'s table.
pub fn skin_fits(reg: &Registry, skin_id: i64, hull: i64) -> bool {
    table(reg).fits(skin_id, hull)
}

/// `ApplySkinToShip(shipID, skinID|None)` for `actor`. The ship must be the
/// actor's, a ship, and either in the hangar of the station the actor is
/// docked in or the one they fly; a SKIN needs a held licence and a hull it
/// fits. `None` clears. Queues the gateway notice. Writes nothing on `Err`.
pub fn apply(
    reg: &mut Registry,
    actor: i64,
    ship: i64,
    skin: Option<i64>,
) -> Result<(), SkinError> {
    let it = reg.store().get(ship).ok_or(SkinError::NoSuchItem)?;
    if it.owner_id != actor {
        return Err(SkinError::NotYours);
    }
    if it.category_id != CATEGORY_SHIP {
        return Err(SkinError::NotAShip);
    }
    let in_hangar = reg
        .docked_station(actor)
        .is_some_and(|s| it.location_id == s && it.flag_id == crate::model::FLAG_HANGAR);
    let flown = reg.roster().get(actor).is_some_and(|p| p.ship_id == ship);
    if !in_hangar && !flown {
        return Err(SkinError::NotHere);
    }
    let hull = it.type_id;
    let now = crate::pyutil::filetime_now();
    match skin {
        None => {
            if reg.deltas_mut().del(KIND, &app_key(ship)).is_none() {
                return Ok(());
            }
        }
        Some(sid) => {
            let of_skin: Vec<Held> = held(reg, actor, now)
                .into_iter()
                .filter(|h| h.skin_id == sid)
                .collect();
            // A single-use licence works on the one ship it is bound to.
            let h = of_skin
                .iter()
                .filter(|h| h.bound.is_none_or(|b| b == ship))
                .max_by_key(|h| h.expires.unwrap_or(i64::MAX))
                .ok_or(if of_skin.is_empty() {
                    SkinError::NotLicensed
                } else {
                    SkinError::BoundElsewhere
                })?
                .clone();
            let fits = table(reg)
                .skin(sid)
                .is_some_and(|s| s.hulls.contains(&hull));
            if !fits {
                return Err(SkinError::WrongHull);
            }
            if applied_skin(reg, ship) == Some(sid) {
                return Err(SkinError::AlreadyApplied);
            }
            prune(reg);
            if h.single_use && h.bound.is_none() {
                // Bind it: the row stays (held, with its expiry) but serves only this ship.
                reg.deltas_mut().put(
                    KIND,
                    lic_key(actor, h.lic_type),
                    vec![
                        h.expires.unwrap_or(0).to_string(),
                        "1".to_owned(),
                        ship.to_string(),
                    ],
                );
            }
            reg.deltas_mut()
                .put(KIND, app_key(ship), vec![h.lic_type.to_string()]);
        }
    }
    reg.mark_dirty();
    announce(reg, ship, actor);
    Ok(())
}

/// Drop the applied rows of ships that no longer exist.
fn prune(reg: &mut Registry) {
    let gone: Vec<Vec<String>> = reg
        .deltas()
        .of(KIND)
        .keys()
        .filter(|k| k.len() == 2 && k[0] == APP)
        .filter(|k| {
            k[1].parse::<i64>()
                .ok()
                .is_none_or(|s| reg.store().get(s).is_none())
        })
        .cloned()
        .collect();
    for k in gone {
        reg.deltas_mut().del(KIND, &k);
    }
}

// ---- machoNet events -------------------------------------------------------------

#[derive(Debug, Clone)]
struct Event {
    to: i64,
    name: &'static str,
    args: Vec<PyValue>,
}

impl crate::notify::Push for Event {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (self.name, self.args.clone())
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

fn event(reg: &mut Registry, to: i64, name: &'static str, args: Vec<PyValue>) {
    reg.push_later(EVENT_DELAY, Event { to, name, args });
}

// ---- the gateway -----------------------------------------------------------------

fn put_field_varint(out: &mut Vec<u8>, field: u64, v: u64) {
    put_varint(out, field << 3);
    put_varint(out, v);
}

fn sequential(id: i64) -> Vec<u8> {
    let mut m = Vec::new();
    put_field_varint(&mut m, 1, id as u64);
    m
}

/// `cosmetic.ship.State{character, ship, skin}` for `ship`: the owner is the
/// item's, or `asker` for a ship the world does not hold.
fn state_bytes(reg: &Registry, ship: i64, asker: i64) -> Vec<u8> {
    let owner = reg.store().get(ship).map_or(asker, |i| i.owner_id);
    let mut skin = Vec::new();
    match applied_skin(reg, ship) {
        Some(sid) => {
            let mut first = Vec::new();
            put_bytes(&mut first, 1, &sequential(sid));
            put_bytes(&mut skin, 1, &first);
        }
        None => put_field_varint(&mut skin, 3, 1),
    }
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &sequential(owner));
    put_bytes(&mut out, 2, &sequential(ship));
    put_bytes(&mut out, 3, &skin);
    out
}

/// Whether `viewer`'s park holds `ship`: modelled parks say so, an
/// unmodelled one is unfiltered (`grid`).
fn holds(reg: &Registry, viewer: i64, ship: i64) -> bool {
    reg.ext::<crate::grid::Parks>()
        .of(viewer)
        .is_none_or(|p| p.contains(&ship))
}

/// Queue `SetNotice{state}` for `ship` to **every pilot**, one
/// `Target::Character` each, under the world lock (nothing is sent: `gateway_out`).
///
/// Not just the owner and the park: a viewer who cached `no_skin` (the 200 the
/// gateway gives an unskinned ship) never asks again, so a viewer elsewhere, or
/// one who saw the ship while its owner was docked, would keep the old state
/// for the session. A notice for a ship the client does not have in its scene
/// is dropped by the client (`ship.py` filters by id), and a pilot with no open
/// gateway stream loses it, so the cost is one small message per pilot.
fn announce(reg: &mut Registry, ship: i64, owner: i64) {
    let mut payload = Vec::new();
    put_bytes(&mut payload, 1, &state_bytes(reg, ship, owner));
    let pilots: Vec<i64> = reg
        .config()
        .characters
        .iter()
        .map(|c| c.character_id)
        .collect();
    for c in pilots {
        crate::gateway_out::queue(reg, NOTICE_SET, payload.clone(), Target::Character(c));
    }
}

/// The player ships in `asker`'s park, one state each.
fn bubble(reg: &Registry, asker: i64) -> Vec<Vec<u8>> {
    let Some(me) = reg.roster().get(asker) else {
        return Vec::new();
    };
    reg.roster()
        .in_system(me.solar_system_id)
        .into_iter()
        .filter(|p| {
            p.character_id == asker
                || (reg.character(p.character_id).is_some() && holds(reg, asker, p.ship_id))
        })
        .map(|p| state_bytes(reg, p.ship_id, p.character_id))
        .collect()
}

fn reply(status: u32, payload: Option<(String, Vec<u8>)>) -> Reply {
    Reply { status, payload }
}

/// The gateway arm (`gateway_ext::CHAIN`): `cosmetic.ship.api.GetRequest` and
/// `GetAllInBubbleRequest`. `GetRequest` answers **200 with a state for any
/// ship id** (`no_skin` when unskinned; the 404 the stub gave made every
/// viewer wait 3 s and log an error). `None` for everything else. On a ground
/// with no world there is no ship to ask about: 404, which the client reads as
/// "no state".
pub fn gateway(ty: &str, body: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    let rest = ty.strip_prefix(PKG)?;
    if !matches!(rest, "GetRequest" | "GetAllInBubbleRequest") {
        return None;
    }
    let name = format!("{}Response", ty.strip_suffix("Request").unwrap_or(ty));
    let Ground::Server(w) = g else {
        return Some(reply(404, None));
    };
    let reg = crate::service::world_read_timed(w, "skins_gateway");
    let mut out = Vec::new();
    if rest == "GetRequest" {
        let ship = get_bytes(body, 1).and_then(|s| get_uint(&s, 1));
        let Some(ship) = ship.filter(|s| *s > 0) else {
            return Some(reply(404, None));
        };
        put_bytes(&mut out, 1, &state_bytes(&reg, ship as i64, character));
    } else {
        for s in bubble(&reg, character) {
            put_bytes(&mut out, 1, &s);
        }
    }
    Some(reply(200, Some((name, out))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture::*;
    use std::sync::{Arc, RwLock};

    const ALICE: i64 = 1_001;
    const BOB: i64 = 1_002;
    const STATION: i64 = 600_001;
    const SYSTEM: i64 = 3_001;

    /// Two docked pilots; the SKIN table fits Alice's starting hull.
    fn world() -> Registry {
        let mut reg = crate::testkit::registry_with(|cfg| {
            let a = crate::model::Character {
                character_id: ALICE,
                owner_user_id: Some(11),
                station_id: Some(STATION),
                ..Default::default()
            };
            let a_ship = a.ship_id;
            let mut b = a.clone();
            b.character_id = BOB;
            b.ship_id = a_ship + 1;
            b.owner_user_id = Some(12);
            cfg.characters = vec![a, b];
            cfg.catch_all = None;
        });
        let hull = reg
            .store()
            .get(reg.character(ALICE).unwrap().ship_id)
            .map(|i| i.type_id)
            .expect("the pilot starts in a ship");
        let (l, s) = fixture::lines(hull);
        let mut content = crate::content::Content::fixture();
        content.insert("skins", Skins::parse(&l, &s).unwrap());
        reg = reg.with_content(Arc::new(content));
        reg
    }

    fn ship_of(reg: &Registry, who: i64) -> i64 {
        reg.character(who).unwrap().ship_id
    }

    /// A licence item of `ty` in `who`'s hangar.
    fn item(reg: &mut Registry, who: i64, ty: i64) -> i64 {
        let at = crate::item::Slot::new(who, STATION, crate::model::FLAG_HANGAR);
        let ch = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id: ty,
                quantity: 1,
                to: at,
            }])
            .expect("the licence type is known");
        ch.iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .unwrap()
    }

    /// Put `who` in space, in one system, in their ship.
    fn arrive(reg: &mut Registry, who: i64) {
        let ship = ship_of(reg, who);
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                who,
                ship,
                SYSTEM,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
    }

    fn give(reg: &mut Registry, who: i64, ty: i64) {
        grant(reg, who, ty, crate::pyutil::filetime_now()).unwrap();
    }

    #[test]
    fn activating_consumes_the_item_and_adds_the_licence() {
        let mut reg = world();
        let it = item(&mut reg, ALICE, LIC_PERM);
        let _ = reg.take_pending();
        let skins = activate(&mut reg, ALICE, &[it]).unwrap();
        assert_eq!(skins, [SKIN_A]);
        assert!(reg.store().get(it).is_none(), "the licence item is gone");
        // The client's hangar list drops it on an item change to the
        // junkyard, not on the invalidation alone.
        let told: Vec<_> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push())
            .filter(|(n, _)| *n == "OnItemChange")
            .collect();
        assert_eq!(told.len(), 1, "one item change: {told:?}");
        let PyValue::Dict(change) = &told[0].1[1] else {
            panic!("the change is a dict: {:?}", told[0].1);
        };
        assert!(
            change.contains(&(
                PyValue::Int(crate::model::FLAG_HANGAR),
                PyValue::Int(crate::item::IX_FLAG)
            )),
            "{change:?}"
        );
        let h = held(&reg, ALICE, crate::pyutil::filetime_now());
        assert_eq!(
            (h.len(), h[0].lic_type, h[0].skin_id, h[0].expires),
            (1, LIC_PERM, SKIN_A, None)
        );
        assert!(held(&reg, BOB, 0).is_empty(), "nobody else gets it");
    }

    #[test]
    fn a_licence_is_kept_by_type_not_by_skin_id() {
        let mut reg = world();
        give(&mut reg, ALICE, LIC_PERM);
        let key = lic_key(ALICE, LIC_PERM);
        assert!(reg.deltas().of(KIND).contains_key(&key));
        for (k, v) in reg.deltas().of(KIND) {
            assert!(
                !k.iter().chain(v).any(|f| f == &SKIN_A.to_string()),
                "no persisted field is a SKIN id: {k:?} {v:?}"
            );
        }
    }

    #[test]
    fn activation_needs_a_docked_pilot_and_the_pilots_own_item_in_the_hangar() {
        let mut reg = world();
        let mine = item(&mut reg, ALICE, LIC_PERM);
        assert_eq!(activate(&mut reg, BOB, &[mine]), Err(SkinError::NotYours));
        assert_eq!(activate(&mut reg, ALICE, &[7]), Err(SkinError::NoSuchItem));
        let ship = ship_of(&reg, ALICE);
        assert_eq!(
            activate(&mut reg, ALICE, &[ship]),
            Err(SkinError::NotALicence)
        );
        // Undocked: the roster says so, `Character::station_id` does not change.
        arrive(&mut reg, ALICE);
        assert_eq!(
            activate(&mut reg, ALICE, &[mine]),
            Err(SkinError::NotDocked)
        );
        assert!(reg.store().get(mine).is_some() && held(&reg, ALICE, 0).is_empty());
    }

    #[test]
    fn a_permanent_licence_held_refuses_a_second_and_nothing_is_consumed() {
        let mut reg = world();
        give(&mut reg, ALICE, LIC_PERM);
        let a = item(&mut reg, ALICE, LIC_PERM);
        let b = item(&mut reg, ALICE, LIC_OTHER);
        assert_eq!(
            activate(&mut reg, ALICE, &[b, a]),
            Err(SkinError::AlreadyLicensed)
        );
        assert!(reg.store().get(a).is_some() && reg.store().get(b).is_some());
        assert_eq!(held(&reg, ALICE, 0).len(), 1, "the batch is all or nothing");
    }

    #[test]
    fn a_skin_has_one_licence_row_per_character_whatever_is_activated() {
        let mut reg = world();
        let now = crate::pyutil::filetime_now();
        grant(&mut reg, ALICE, LIC_TEMP, now).unwrap();
        grant(&mut reg, ALICE, LIC_TEMP, now).unwrap();
        let h = held(&reg, ALICE, now);
        assert_eq!(h.len(), 1);
        grant(&mut reg, ALICE, LIC_PERM, now).unwrap();
        let h = held(&reg, ALICE, now);
        assert_eq!((h.len(), h[0].lic_type, h[0].expires), (1, LIC_PERM, None));
    }

    #[test]
    fn an_expired_licence_is_not_held() {
        let mut reg = world();
        let now = crate::pyutil::filetime_now();
        grant(&mut reg, ALICE, LIC_TEMP, now).unwrap();
        assert!(held(&reg, ALICE, now + 31 * DAY_TICKS).is_empty());
    }

    #[test]
    fn apply_needs_a_licence_and_a_hull_it_fits() {
        let mut reg = world();
        let ship = ship_of(&reg, ALICE);
        assert_eq!(
            apply(&mut reg, ALICE, ship, Some(SKIN_A)),
            Err(SkinError::NotLicensed)
        );
        give(&mut reg, ALICE, LIC_OTHER);
        assert_eq!(
            apply(&mut reg, ALICE, ship, Some(SKIN_B)),
            Err(SkinError::WrongHull),
            "a SKIN for another hull"
        );
        assert_eq!(applied_skin(&reg, ship), None);
        give(&mut reg, ALICE, LIC_PERM);
        apply(&mut reg, ALICE, ship, Some(SKIN_A)).unwrap();
        assert_eq!(applied_skin(&reg, ship), Some(SKIN_A));
        // Applying what is already on the ship changes nothing.
        let _ = apply(&mut reg, ALICE, ship, Some(SKIN_A));
        assert_eq!(applied_skin(&reg, ship), Some(SKIN_A));
    }

    #[test]
    fn apply_is_the_owners_only() {
        let mut reg = world();
        give(&mut reg, BOB, LIC_PERM);
        let ship = ship_of(&reg, ALICE);
        assert_eq!(
            apply(&mut reg, BOB, ship, Some(SKIN_A)),
            Err(SkinError::NotYours)
        );
        assert_eq!(apply(&mut reg, ALICE, 7, None), Err(SkinError::NoSuchItem));
        // Not a ship.
        let lic = item(&mut reg, ALICE, LIC_PERM);
        assert_eq!(apply(&mut reg, ALICE, lic, None), Err(SkinError::NotAShip));
    }

    #[test]
    fn apply_none_clears_and_a_lapsed_licence_unapplies() {
        let mut reg = world();
        let ship = ship_of(&reg, ALICE);
        give(&mut reg, ALICE, LIC_PERM);
        apply(&mut reg, ALICE, ship, Some(SKIN_A)).unwrap();
        apply(&mut reg, ALICE, ship, None).unwrap();
        assert_eq!(applied_skin(&reg, ship), None);
        apply(&mut reg, ALICE, ship, Some(SKIN_A)).unwrap();
        // The owner loses the licence: the SKIN stops counting.
        reg.deltas_mut().del(KIND, &lic_key(ALICE, LIC_PERM));
        assert_eq!(applied_skin(&reg, ship), None);
    }

    #[test]
    fn a_single_use_licence_puts_its_skin_on_the_ship() {
        let mut reg = world();
        let ship = ship_of(&reg, ALICE);
        give(&mut reg, ALICE, LIC_SINGLE);
        apply(&mut reg, ALICE, ship, Some(SKIN_C)).unwrap();
        assert_eq!(applied_skin(&reg, ship), Some(SKIN_C));
    }

    #[test]
    fn apply_queues_one_set_notice_through_the_outbox_never_a_publish() {
        let mut reg = world();
        let ship = ship_of(&reg, ALICE);
        give(&mut reg, ALICE, LIC_PERM);
        apply(&mut reg, ALICE, ship, Some(SKIN_A)).unwrap();
        let q = crate::gateway_out::take(&mut reg);
        assert!(!q.is_empty());
        assert!(q.iter().all(|n| n.0 == NOTICE_SET));
        let state = get_bytes(&q[0].1, 1).expect("a state");
        assert_eq!(
            get_bytes(&state, 2).and_then(|s| get_uint(&s, 1)),
            Some(ship as u64)
        );
        let skin = get_bytes(&state, 3).unwrap();
        let fp = get_bytes(&skin, 1).expect("firstparty");
        let id = get_bytes(&fp, 1).unwrap();
        assert_eq!(get_uint(&id, 1), Some(SKIN_A as u64));
    }

    /// The viewer who cached `no_skin` never asks again, so the notice must
    /// reach every pilot whatever the owner or the viewer is doing: the owner
    /// docked, the viewer docked or elsewhere, nobody on any grid.
    #[test]
    fn the_notice_reaches_every_pilot_even_when_the_owner_applies_docked() {
        let mut reg = world();
        let ship = ship_of(&reg, ALICE);
        give(&mut reg, ALICE, LIC_PERM);
        assert!(reg.roster().get(ALICE).is_none(), "the owner is docked");
        apply(&mut reg, ALICE, ship, Some(SKIN_A)).unwrap();
        let mut to: Vec<i64> = crate::gateway_out::take(&mut reg)
            .iter()
            .map(|n| match n.2 {
                Target::Character(c) => c,
                _ => panic!("one Character target per pilot"),
            })
            .collect();
        to.sort_unstable();
        assert_eq!(to, [ALICE, BOB]);
    }

    #[test]
    fn the_notice_reaches_a_viewer_in_another_system_too() {
        let mut reg = world();
        let ship = ship_of(&reg, ALICE);
        give(&mut reg, ALICE, LIC_PERM);
        arrive(&mut reg, ALICE);
        let bob_ship = ship_of(&reg, BOB);
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                BOB,
                bob_ship,
                SYSTEM + 1,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        apply(&mut reg, ALICE, ship, Some(SKIN_A)).unwrap();
        let q = crate::gateway_out::take(&mut reg);
        assert!(
            q.iter()
                .any(|n| matches!(n.2, Target::Character(c) if c == BOB))
        );
    }

    fn ground(reg: Registry) -> Ground {
        Ground::Server(Arc::new(RwLock::new(reg)))
    }

    fn get_req(ship: i64) -> Vec<u8> {
        let mut b = Vec::new();
        put_bytes(&mut b, 1, &sequential(ship));
        b
    }

    #[test]
    fn get_request_answers_firstparty_or_no_skin_for_any_ship() {
        let mut reg = world();
        let ship = ship_of(&reg, ALICE);
        give(&mut reg, ALICE, LIC_PERM);
        apply(&mut reg, ALICE, ship, Some(SKIN_A)).unwrap();
        let g = ground(reg);
        let ty = format!("{PKG}GetRequest");
        let r = gateway(&ty, &get_req(ship), BOB, &g).unwrap();
        assert_eq!(r.status, 200);
        let (name, bytes) = r.payload.unwrap();
        assert_eq!(name, format!("{PKG}GetResponse"));
        let state = get_bytes(&bytes, 1).unwrap();
        assert_eq!(
            get_bytes(&state, 1).and_then(|c| get_uint(&c, 1)),
            Some(ALICE as u64),
            "the owner, whoever asks"
        );
        let skin = get_bytes(&state, 3).unwrap();
        assert!(get_bytes(&skin, 1).is_some(), "firstparty");
        // A ship nobody skinned, or does not exist: 200 with no_skin.
        let r = gateway(&ty, &get_req(5), BOB, &g).unwrap();
        assert_eq!(r.status, 200);
        let state = get_bytes(&r.payload.unwrap().1, 1).unwrap();
        let skin = get_bytes(&state, 3).unwrap();
        assert_eq!(get_uint(&skin, 3), Some(1), "no_skin");
        // No ship id at all.
        assert_eq!(gateway(&ty, &[], BOB, &g).unwrap().status, 404);
        // Other types fall through.
        assert!(gateway("eve_public.plex.vault.api.BalanceRequest", &[], 1, &g).is_none());
    }

    #[test]
    fn all_in_bubble_lists_the_ships_in_the_askers_park() {
        let mut reg = world();
        let ship = ship_of(&reg, ALICE);
        give(&mut reg, ALICE, LIC_PERM);
        apply(&mut reg, ALICE, ship, Some(SKIN_A)).unwrap();
        arrive(&mut reg, ALICE);
        arrive(&mut reg, BOB);
        let g = ground(reg);
        let ty = format!("{PKG}GetAllInBubbleRequest");
        let r = gateway(&ty, &[], BOB, &g).unwrap();
        assert_eq!(r.status, 200);
        let (name, bytes) = r.payload.unwrap();
        assert_eq!(name, format!("{PKG}GetAllInBubbleResponse"));
        let states = crate::gateway::field(&bytes, 1);
        assert_eq!(states.len(), 2, "both pilots' ships");
        // Docked askers see none.
        let r = gateway(&ty, &[], 777, &g).unwrap();
        assert!(r.payload.unwrap().1.is_empty());
    }

    #[test]
    fn a_ground_without_a_world_says_no_state() {
        let ty = format!("{PKG}GetRequest");
        let r = gateway(&ty, &get_req(5), 1, &Ground::local()).unwrap();
        assert_eq!(r.status, 404);
    }

    #[test]
    fn the_table_holds_the_licence_item_types_and_refuses_a_bad_row() {
        let reg = world();
        let t = table(&reg);
        assert!(crate::item::find_in(reg.content(), LIC_PERM).is_some());
        assert_eq!(
            item_in(reg.content(), LIC_TEMP).map(|i| i.type_id),
            Some(LIC_TEMP)
        );
        assert!(item_in(reg.content(), LIC_TEMP + 100).is_none());
        assert_eq!(t.skin_of(LIC_TEMP), Some(SKIN_A));
        let (mut l, s) = fixture::lines(1);
        l.push(format!("{LIC_PERM}\t1\t999\t-1\t0\t0.1\tx"));
        assert!(
            Skins::parse(&l, &s).is_err(),
            "a licence of an unknown SKIN"
        );
        assert!(Skins::parse(&["1\t2".to_owned()], &[]).is_err());
    }

    #[test]
    fn a_world_with_skin_rows_and_no_table_refuses_to_start() {
        let mut reg = crate::testkit::registry();
        assert!(check_table(&reg).is_ok());
        reg.deltas_mut()
            .put(KIND, lic_key(1, 2), vec!["0".into(), "0".into()]);
        let mut content = crate::content::Content::fixture();
        content.insert("skins", Skins::default());
        let reg = reg.with_content(Arc::new(content));
        assert!(check_table(&reg).is_err());
        assert!(check_table(&world()).is_ok());
    }
}
