//! Group contacts (SOC-C2): the address books a **corporation** and an
//! **alliance** keep, how they combine with a character's own book into the
//! relationship the overview colours by, and [`allied`], the one question
//! the bots and the killmail lane ask of all three.
//!
//! `docs/lanes/soc-c2.md` has the research. The short form, every line
//! **[V]** in build 3619879 (`addressbookService.py`, `base_corporation.py`,
//! `all_cso.py`, `parklife/state.py`):
//!
//! - the client loads three books at once (`GetContacts`, `:276-287`):
//!   `charMgr.GetContactList()` (served by `crate::contacts`), the client
//!   `corp` service's `GetContactList` -> `corpRegistry.GetCorporateContacts()`
//!   (never called while `session.corpid` is an NPC corp,
//!   `base_corporation.py:1260-1262`), and, only when `session.allianceid`,
//!   `alliance.GetContactList` -> `allianceRegistry.GetAllianceContacts()`
//!   (`all_cso.py:300-302`);
//! - `GetRelationship(charID, corporationID, allianceID)` (`:321-365`) is
//!   pure client code over those three books. It fills **nine** numbers, one
//!   per (book, target) pair, each `contactNeutralStanding` unless the target
//!   is in that book, and `hasRelationship` if any target was found;
//! - the overview then asks "is **any** of the nine High / Good / Horrible /
//!   Bad" (`IfAnyRelations`, `state.py:1069-1074`) in the default flag order
//!   High, Good, ..., Horrible, Bad, ..., Neutral, NoStanding
//!   (`defaultFlagOrder`, `:197-219`). So there is no "personal beats corp"
//!   precedence: **the bluest of the nine wins**, and a single blue among
//!   reds colours the ship blue. [`standing_flag`] is that rule.
//!
//! Persistence is one delta kind, [`KIND`]: `owner contact -> relationship
//! labelMask`, where `owner` is a corporation or an alliance id.

use crate::contacts::{contacts_bad_args, id_list, relationship_arg, relationship_value};
use crate::pyutil::keyval;
use crate::service::{Registry, Request, ServiceError};
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// The delta kind: `owner contact -> relationship labelMask`.
pub const KIND: &str = "orgcontact";

/// `appConst.corpRoleDirector` **[V]** (`appConst.py`, also
/// `service/corp.rs`).
pub const CORP_ROLE_DIRECTOR: i64 = 1;
/// `appConst.corpRoleDiplomat` **[V]** (`appConst.py:454`).
pub const CORP_ROLE_DIPLOMAT: i64 = 17_179_869_184;

/// `appConst.contactGoodStanding` and `contactBadStanding` **[V]**
/// (`appConst.py:2260-2264`): the two edges every overview test is built on.
const GOOD: f64 = 5.0;
const BAD: f64 = -5.0;

// ---- the pure model ----------------------------------------------------------

/// `appConst.maxContacts = 1024` **[V]**: the most contacts a book holds.
/// Used for a corporation's book **[A]** (the constant is the personal
/// book's; falsified by Tranquility letting a corp keep more).
pub const MAX_CORP_CONTACTS: usize = 1024;
/// `appConst.maxAllianceContacts = 2600` **[V]**.
pub const MAX_ALLIANCE_CONTACTS: usize = 2600;

/// `inventorycommon.const` id ranges **[V]**: factions, NPC corporations and
/// agents, which are not in any table of ours and are valid contacts.
const FACTION_IDS: std::ops::RangeInclusive<i64> = 500_000..=599_999;
const NPC_CORPORATION_IDS: std::ops::RangeInclusive<i64> = 1_000_000..=1_999_999;
const AGENT_IDS: std::ops::RangeInclusive<i64> = 3_000_000..=3_999_999;

/// Whether `id` names something a book may hold: a character, a player
/// corporation, an alliance, or an id in the client's faction, NPC
/// corporation or agent ranges (SEC-FIX 8; it used to be any id above 1).
fn is_owner(reg: &Registry, id: i64) -> bool {
    reg.character(id).is_some()
        || reg.corporations().get(id).is_some()
        || crate::alliance::name_of(reg.deltas(), id).is_some()
        || FACTION_IDS.contains(&id)
        || NPC_CORPORATION_IDS.contains(&id)
        || AGENT_IDS.contains(&id)
}

/// The nine numbers `addressbookService.GetRelationship` returns, under the
/// client's own names. **[V]** `:321-365`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Relationships {
    pub pers_to_pers: f64,
    pub pers_to_corp: f64,
    pub pers_to_alliance: f64,
    pub corp_to_pers: f64,
    pub corp_to_corp: f64,
    pub corp_to_alliance: f64,
    pub alliance_to_pers: f64,
    pub alliance_to_corp: f64,
    pub alliance_to_alliance: f64,
    /// Whether any of the nine came from a book rather than the default.
    pub has_relationship: bool,
}

impl Relationships {
    /// All nine, in `GetAllRelationshipGroups`' order (alliance, corp,
    /// personal; `state.py:1025-1033`). The order cannot change an `any` or
    /// an `all`; it is kept so a reader can line it up with the client.
    pub fn all(&self) -> [f64; 9] {
        [
            self.alliance_to_pers,
            self.alliance_to_corp,
            self.alliance_to_alliance,
            self.corp_to_pers,
            self.corp_to_corp,
            self.corp_to_alliance,
            self.pers_to_pers,
            self.pers_to_corp,
            self.pers_to_alliance,
        ]
    }
}

/// The three books one viewer sees through: contact id -> relationship.
#[derive(Debug, Clone, Copy)]
pub struct Books<'a> {
    pub personal: &'a BTreeMap<i64, f64>,
    pub corp: &'a BTreeMap<i64, f64>,
    pub alliance: &'a BTreeMap<i64, f64>,
}

/// `GetRelationship(charID, corporationID, allianceID)`, **[V]** line for
/// line: each book is asked for the character, the corporation and the
/// alliance, an absent `allianceID` is skipped, and anything not found
/// stays neutral (0).
pub fn relationship(
    books: Books<'_>,
    char_id: i64,
    corp_id: i64,
    alliance_id: Option<i64>,
) -> Relationships {
    let find =
        |book: &BTreeMap<i64, f64>, id: Option<i64>| id.and_then(|id| book.get(&id).copied());
    let (c, k) = (Some(char_id), Some(corp_id));
    let found = [
        find(books.personal, c),
        find(books.personal, k),
        find(books.personal, alliance_id),
        find(books.corp, c),
        find(books.corp, k),
        find(books.corp, alliance_id),
        find(books.alliance, c),
        find(books.alliance, k),
        find(books.alliance, alliance_id),
    ];
    let v = found.map(|f| f.unwrap_or(0.0));
    Relationships {
        pers_to_pers: v[0],
        pers_to_corp: v[1],
        pers_to_alliance: v[2],
        corp_to_pers: v[3],
        corp_to_corp: v[4],
        corp_to_alliance: v[5],
        alliance_to_pers: v[6],
        alliance_to_corp: v[7],
        alliance_to_alliance: v[8],
        has_relationship: found.iter().any(Option::is_some),
    }
}

/// The overview's standing flags, in the client's default priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandingFlag {
    /// Any of the nine `> 5` (`IsHighStanding`, `state.py:68-73`).
    High,
    /// Any in `(0, 5]` (`IsGoodStanding`, `:76-81`).
    Good,
    /// Any `< -5` (`IsHorribleStanding`, `:100-105`).
    Horrible,
    /// Any in `[-5, 0)` (`IsBadStanding`, `:92-97`).
    Bad,
    /// Found in some book, and all nine are 0 (`CheckStandingNeutral`,
    /// `:969-986`; the client also requires not the same corp or alliance).
    Neutral,
    /// In no book (`CheckNoStanding`, `:1236-1244`).
    NoStanding,
}

/// Which standing flag wins, assuming every flag is switched on (the
/// player's overview settings decide that, client side).
///
/// **[V]** order: `defaultFlagOrder` lists High (`:203`) and Good (`:204`)
/// before Horrible (`:210`) and Bad (`:211`), and each test is an `any` over
/// all nine. Flags that are not standings (same fleet, same player corp,
/// same alliance, war, crimewatch) come earlier still and are not modelled
/// here; [`allied`] covers the corp and alliance ones.
pub fn standing_flag(r: &Relationships) -> StandingFlag {
    let all = r.all();
    let any = |f: &dyn Fn(f64) -> bool| all.iter().any(|v| f(*v));
    if any(&|v| v > GOOD) {
        StandingFlag::High
    } else if any(&|v| v > 0.0 && v <= GOOD) {
        StandingFlag::Good
    } else if any(&|v| v < BAD) {
        StandingFlag::Horrible
    } else if any(&|v| (BAD..0.0).contains(&v)) {
        StandingFlag::Bad
    } else if r.has_relationship {
        StandingFlag::Neutral
    } else {
        StandingFlag::NoStanding
    }
}

/// Who a character is, for [`allied`]: the three ids a slim item carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Affiliation {
    pub character_id: i64,
    pub corporation_id: i64,
    pub alliance_id: Option<i64>,
}

/// **Whether `viewer` counts `other` as a friend** -- the coalition test the
/// bots hunt by and C5 reads. Pure, for the bot process as much as the
/// server.
///
/// True when any of these holds:
///
/// - they are the same character;
/// - they are in the same **player** corporation. Two characters in one NPC
///   corp are not allied **[A]** (falsified by the owner wanting NPC-corp
///   mates to leave each other alone): every new character starts in one,
///   so "same corp" there would make every rookie a friend of every other;
/// - they are in the same alliance;
/// - the viewer's corporation or the viewer's alliance has the other's
///   corporation or the other's alliance in its book **above 0**.
///   `group_standing(owner, target)` answers that book entry. This is what
///   makes a coalition: alliance A setting alliance B (or corp C) blue.
///
/// - **REL-FIX (D2)**: the viewer's corporation or alliance book has the
///   other's **character id** above 0 (a lone pilot in an NPC corporation
///   has no group of their own to list), or the viewer's own **personal**
///   book does (`personal_standing(other.character_id)`, see
///   [`allied_with`]).
///
/// One-sided on purpose, like the client's own books: a coalition is mutual
/// when both sides set the blue. This form consults no personal book
/// (`allied_with` does); the group books answer on the character id too.
pub fn allied(
    viewer: Affiliation,
    other: Affiliation,
    group_standing: impl Fn(i64, i64) -> Option<f64>,
) -> bool {
    allied_with(viewer, other, group_standing, |_| None)
}

/// [`allied`] plus the viewer's own **personal** book: `personal_standing(id)`
/// is the viewer's contact standing toward the character `id`. A personal
/// contact above 0 on the other's character id makes them allied -- how a
/// lone human friend in an NPC corporation counts (REL-FIX, D2). Only the
/// character id is looked up: a personal contact on a corporation or
/// alliance is the client's colour, not the bots' policy.
pub fn allied_with(
    viewer: Affiliation,
    other: Affiliation,
    group_standing: impl Fn(i64, i64) -> Option<f64>,
    personal_standing: impl Fn(i64) -> Option<f64>,
) -> bool {
    if viewer.character_id == other.character_id {
        return true;
    }
    if viewer.corporation_id == other.corporation_id
        && !crate::standings::is_npc_owner(viewer.corporation_id)
    {
        return true;
    }
    if viewer.alliance_id.is_some() && viewer.alliance_id == other.alliance_id {
        return true;
    }
    if personal_standing(other.character_id).is_some_and(|r| r > 0.0) {
        return true;
    }
    let owners = [Some(viewer.corporation_id), viewer.alliance_id];
    let targets = [
        Some(other.character_id),
        Some(other.corporation_id),
        other.alliance_id,
    ];
    owners.iter().flatten().any(|owner| {
        targets
            .iter()
            .flatten()
            .any(|target| group_standing(*owner, *target).is_some_and(|r| r > 0.0))
    })
}

// ---- the world's books -------------------------------------------------------

/// One entry of a group's book: what `OnOrganizationContactsUpdated`'s
/// triples carry (`:240-258`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroupContact {
    pub relationship: f64,
    pub label_mask: i64,
}

fn key(owner: i64, contact: i64) -> Vec<String> {
    vec![owner.to_string(), contact.to_string()]
}

/// One group's book, in contact-id order. Rows that do not parse are
/// skipped, not fatal: a delta row is ours, and only a hand edit breaks one.
pub fn book(reg: &Registry, owner: i64) -> Vec<(i64, GroupContact)> {
    let owner_s = owner.to_string();
    let mut out: Vec<(i64, GroupContact)> = reg
        .deltas()
        .of(KIND)
        .iter()
        .filter(|(k, _)| k.first() == Some(&owner_s))
        .filter_map(|(k, v)| {
            let id = k.get(1)?.parse::<i64>().ok()?;
            let relationship = v.first()?.parse::<f64>().ok().filter(|r| r.is_finite())?;
            let label_mask = v.get(1).and_then(|m| m.parse().ok()).unwrap_or(0);
            Some((
                id,
                GroupContact {
                    relationship,
                    label_mask,
                },
            ))
        })
        .collect();
    out.sort_by_key(|(id, _)| *id);
    out
}

/// One entry of one group's book.
pub fn get(reg: &Registry, owner: i64, contact: i64) -> Option<GroupContact> {
    let v = reg.deltas().of(KIND).get(&key(owner, contact))?;
    Some(GroupContact {
        relationship: v.first()?.parse().ok()?,
        label_mask: v.get(1).and_then(|m| m.parse().ok()).unwrap_or(0),
    })
}

fn book_map(reg: &Registry, owner: Option<i64>) -> BTreeMap<i64, f64> {
    owner
        .map(|o| {
            book(reg, o)
                .into_iter()
                .map(|(id, c)| (id, c.relationship))
                .collect()
        })
        .unwrap_or_default()
}

fn affiliation(reg: &Registry, character_id: i64) -> Option<Affiliation> {
    reg.character(character_id).map(|c| Affiliation {
        character_id,
        corporation_id: c.corporation_id,
        alliance_id: c.alliance_id,
    })
}

/// What `viewer`'s client would compute for `owner`'s ship: the server's
/// mirror of `GetRelationship`, over the books this server keeps.
/// `None` for a character the world does not have.
pub fn relationship_for(reg: &Registry, viewer: i64, owner: i64) -> Option<Relationships> {
    let v = affiliation(reg, viewer)?;
    let o = affiliation(reg, owner)?;
    let personal: BTreeMap<i64, f64> = reg
        .contacts
        .book(viewer)
        .into_iter()
        .map(|(id, c)| (id, c.relationship))
        .collect();
    let corp = book_map(reg, Some(v.corporation_id));
    let alliance = book_map(reg, v.alliance_id);
    Some(relationship(
        Books {
            personal: &personal,
            corp: &corp,
            alliance: &alliance,
        },
        o.character_id,
        o.corporation_id,
        o.alliance_id,
    ))
}

/// [`allied`] over this world's characters and group books. `false` for a
/// character the world does not have.
pub fn allied_in(reg: &Registry, viewer: i64, other: i64) -> bool {
    match (affiliation(reg, viewer), affiliation(reg, other)) {
        (Some(v), Some(o)) => allied_with(
            v,
            o,
            |owner, target| get(reg, owner, target).map(|c| c.relationship),
            |id| {
                reg.contacts
                    .book(viewer)
                    .into_iter()
                    .find_map(|(c, k)| (c == id).then_some(k.relationship))
            },
        ),
        _ => false,
    }
}

fn put(reg: &mut Registry, owner: i64, contact: i64, relationship: f64) {
    let label_mask = get(reg, owner, contact).map_or(0, |c| c.label_mask);
    reg.deltas_mut().put(
        KIND,
        key(owner, contact),
        vec![relationship.to_string(), label_mask.to_string()],
    );
}

// ---- the wire ----------------------------------------------------------------

/// `OnOrganizationContactsUpdated(ownerID, updates)`, `updates` a list of
/// `(contactID, level, labelMask)` triples. **[V]** handler shape
/// (`addressbookService.py:227-260`): `level is None and labelMask is None`
/// removes; a new id is created with `labelMask or 0`; for an id the cache
/// has, **`labelMask is None` sets the relationship** and anything else sets
/// only the mask. So an add or an edit travels as `(id, level, None)`.
///
/// That the real server sends it is **[I]** (a handler with this name and
/// shape exists for nothing else). Audience **[A]**: every online member of
/// the corp (or of the alliance) except the one who made the change, whose
/// client already updated its own cache (`:950`, `:958`, `:1020-1025`).
#[derive(Debug, Clone, PartialEq)]
pub struct OrgContactsUpdated {
    pub to: Vec<i64>,
    pub owner_id: i64,
    /// `(contactID, level)`; `None` for a removal.
    pub updates: Vec<(i64, Option<f64>)>,
}

impl crate::notify::Push for OrgContactsUpdated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let updates = self
            .updates
            .iter()
            .map(|(id, level)| {
                PyValue::Tuple(vec![
                    PyValue::Int(*id),
                    level.map_or(PyValue::None, relationship_value),
                    PyValue::None,
                ])
            })
            .collect();
        (
            "OnOrganizationContactsUpdated",
            vec![PyValue::Int(self.owner_id), PyValue::List(updates)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// Which book a call acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Corp,
    Alliance,
}

/// The owner whose book this caller reads: their corp or their alliance.
fn reader(reg: &Registry, req: &Request<'_>, group: Group) -> Option<i64> {
    let c = reg.character(req.actor?)?;
    match group {
        Group::Corp => Some(c.corporation_id),
        Group::Alliance => c.alliance_id,
    }
}

/// The owner whose book this caller may **change**, or why not.
///
/// **[V]** client gates: `OfferAddCorpContact` needs `session.IsDiplomat()`
/// (`characterCheckers.py:53-58`), which is `corprole & (corpRoleDirector |
/// corpRoleDiplomat)` (`sessionChecker.py:84-85`); `OfferAddAllianceContact`
/// and `OfferRemoveAllianceContact` add `IsAllianceMember()` and
/// `IsExecutorCorp()` (`:42-51`, `:298-307`). **[I]** that the server checks
/// the same: a client offers these only to such a caller, so anything else
/// reaching here is refused.
fn writer(
    reg: &Registry,
    req: &Request<'_>,
    group: Group,
    executor_corp: Option<i64>,
) -> Result<i64, ServiceError> {
    let me = req
        .actor
        .ok_or_else(|| contacts_bad_args(req, "no character"))?;
    let Some((corp, roles)) = reg.corp_membership(me) else {
        return Err(contacts_bad_args(req, "not in a player corporation"));
    };
    if roles & (CORP_ROLE_DIRECTOR | CORP_ROLE_DIPLOMAT) == 0 {
        return Err(contacts_bad_args(req, "not a director or diplomat"));
    }
    match group {
        Group::Corp => Ok(corp),
        Group::Alliance => {
            let alliance = reg
                .character(me)
                .and_then(|c| c.alliance_id)
                .ok_or_else(|| contacts_bad_args(req, "not in an alliance"))?;
            // SEC-FIX 1: an unknown executor refuses too (it used to skip).
            if executor_corp != Some(corp) {
                return Err(contacts_bad_args(req, "not the executor corporation"));
            }
            Ok(alliance)
        }
    }
}

/// The online characters to tell, for a change by `actor` to `owner`'s book.
fn audience(reg: &Registry, group: Group, owner: i64, actor: i64) -> Vec<i64> {
    reg.sessions()
        .online_characters()
        .into_iter()
        .filter(|c| *c != actor)
        .filter(|c| {
            reg.character(*c).is_some_and(|ch| match group {
                Group::Corp => ch.corporation_id == owner,
                Group::Alliance => ch.alliance_id == Some(owner),
            })
        })
        .collect()
}

fn announce(
    reg: &mut Registry,
    group: Group,
    owner: i64,
    actor: i64,
    updates: Vec<(i64, Option<f64>)>,
) {
    if updates.is_empty() {
        return;
    }
    let to = audience(reg, group, owner, actor);
    if !to.is_empty() {
        reg.push(OrgContactsUpdated {
            to,
            owner_id: owner,
            updates,
        });
    }
}

/// A group book as the client caches it: a **dict** `contactID -> KeyVal`.
///
/// **[V]** every reader is a mapping operation: `in` and `[id]
/// .relationshipID` (`GetRelationship`), `.iteritems()`
/// (`GetContactsByMinRelationship`, `:374`, `:378`), `.pop` and item
/// assignment (`:244`, `:254`, `:950`, `:1022`), `cfg.eveowners.Prime(...)`
/// over its keys (`:312-313`). The value's four attributes are the ones the
/// client itself builds for a pushed entry (`:249-253`): `contactID`,
/// `relationshipID`, `inWatchlist` (0) and `labelMask`.
fn book_value(reg: &Registry, owner: Option<i64>) -> PyValue {
    let entries = owner
        .map(|o| book(reg, o))
        .unwrap_or_default()
        .into_iter()
        .map(|(id, c)| {
            (
                keyval(vec![
                    ("contactID", PyValue::Int(id)),
                    ("relationshipID", relationship_value(c.relationship)),
                    ("inWatchlist", PyValue::Bool(false)),
                    ("labelMask", PyValue::Int(c.label_mask)),
                ]),
                PyValue::Int(id),
            )
        })
        .collect();
    PyValue::Dict(entries)
}

/// The contact arms shared by both registries.
fn contact_arms(
    reg: &mut Registry,
    req: &Request<'_>,
    group: Group,
    names: [&str; 4],
    executor_corp: Option<i64>,
) -> Option<Result<PyValue, ServiceError>> {
    let [get_name, add_name, edit_name, remove_name] = names;
    let m = req.method;
    if m == get_name {
        return Some(Ok(book_value(reg, reader(reg, req, group))));
    }
    // `GetLabels` for the group's labels: the same `{}` latch as
    // `charMgr.GetLabels` (`service/character.rs`), reached through
    // `addressbookService.GetContactLabels` (`:1213-1221`) and
    // `all_cso.GetLabels` (`:321-322`). No group labels exist yet.
    if m == "GetLabels" {
        return Some(Ok(PyValue::Dict(vec![])));
    }
    let known = [
        add_name,
        edit_name,
        remove_name,
        "EditContactsRelationshipID",
    ];
    if !known.contains(&m) {
        return None;
    }
    Some((|| {
        let owner = writer(reg, req, group, executor_corp)?;
        let actor = req.actor.unwrap_or_default();
        let mut updates = Vec::new();
        if m == add_name || m == edit_name {
            // `(contactID, relationshipID)`, positional on the wire
            // (`base_corporation.py:1266/1270`, `all_cso.py:306/310`). An
            // edit of an id not in the book creates it, like
            // `charMgr.EditContact` [A].
            let Some(PyValue::Int(id)) = req.args.first() else {
                return Err(contacts_bad_args(
                    req,
                    "the first argument is the contact id",
                ));
            };
            if *id <= 1 || *id == owner || !is_owner(reg, *id) {
                return Err(contacts_bad_args(req, "not an owner that can be a contact"));
            }
            let r = relationship_arg(req.args.get(1)).map_err(|w| contacts_bad_args(req, &w))?;
            let cap = match group {
                Group::Corp => MAX_CORP_CONTACTS,
                Group::Alliance => MAX_ALLIANCE_CONTACTS,
            };
            if get(reg, owner, *id).is_none() && book(reg, owner).len() >= cap {
                return Err(contacts_bad_args(req, "the contact book is full"));
            }
            put(reg, owner, *id, r);
            updates.push((*id, Some(r)));
        } else if m == remove_name {
            let ids = id_list(req.args.first())
                .ok_or_else(|| contacts_bad_args(req, "contactIDs is a list of ids"))?;
            for id in ids {
                if reg.deltas_mut().del(KIND, &key(owner, id)).is_some() {
                    updates.push((id, None));
                }
            }
        } else {
            // `EditContactsRelationshipID(contactIDs, relationshipID)`
            // (`base_corporation.py:1278`, `all_cso.py:318`): existing
            // entries only, like the personal one.
            let ids = id_list(req.args.first())
                .ok_or_else(|| contacts_bad_args(req, "contactIDs is a list of ids"))?;
            let r = relationship_arg(req.args.get(1)).map_err(|w| contacts_bad_args(req, &w))?;
            for id in ids {
                if get(reg, owner, id).is_some() {
                    put(reg, owner, id, r);
                    updates.push((id, Some(r)));
                }
            }
        }
        announce(reg, group, owner, actor, updates);
        Ok(PyValue::None)
    })())
}

/// The contact arms of `corpRegistry`, or `None` for any other method.
///
/// **The hook** (coordinator diff, `service/corp.rs`): consulted before the
/// player-corp arms, `if let Some(a) = crate::group_contacts::corp_registry(reg,
/// req) { return a; }`.
pub fn corp_registry(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    contact_arms(
        reg,
        req,
        Group::Corp,
        [
            "GetCorporateContacts",
            "AddCorporateContact",
            "EditCorporateContact",
            "RemoveCorporateContacts",
        ],
        None,
    )
}

/// The contact arms of `allianceRegistry`, or `None` for any other method.
/// `executor_corp` is the executor corporation of the CALLER's own alliance;
/// `None` (no such alliance, or none open) refuses every write.
///
/// **The hook** for SOC-C1's `allianceRegistry` dispatcher:
/// `if let Some(a) = crate::group_contacts::alliance_registry(reg, req,
/// executor) { return a; }`.
///
/// Not served, on purpose: `GetRelationships`, `SetRelationship` and
/// `DeleteRelationship` (the old alliance relationships table,
/// `all_cso_relationships.py`). **[V]** no window in this build calls
/// `alliance.GetRelationships`, `SetRelationship` or `DeleteRelationship`;
/// only the service defines them. Alliance diplomacy is the contact book
/// above.
pub fn alliance_registry(
    reg: &mut Registry,
    req: &Request<'_>,
    executor_corp: Option<i64>,
) -> Option<Result<PyValue, ServiceError>> {
    contact_arms(
        reg,
        req,
        Group::Alliance,
        [
            "GetAllianceContacts",
            "AddAllianceContact",
            "EditAllianceContact",
            "RemoveAllianceContacts",
        ],
        executor_corp,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};

    // Invented ids throughout.
    const ALICE: i64 = 90_000_101;
    const BOB: i64 = 90_000_102;
    const CAROL: i64 = 90_000_103;
    const CORP_A: i64 = 98_000_001;
    const CORP_B: i64 = 98_000_002;
    const ALLY_X: i64 = 99_000_001;
    const ALLY_Y: i64 = 99_000_002;
    const NPC_CORP: i64 = 1_000_001;

    fn aff(c: i64, corp: i64, ally: Option<i64>) -> Affiliation {
        Affiliation {
            character_id: c,
            corporation_id: corp,
            alliance_id: ally,
        }
    }

    fn map(rows: &[(i64, f64)]) -> BTreeMap<i64, f64> {
        rows.iter().copied().collect()
    }

    fn rel(
        p: &[(i64, f64)],
        c: &[(i64, f64)],
        a: &[(i64, f64)],
        ally: Option<i64>,
    ) -> Relationships {
        let (p, c, a) = (map(p), map(c), map(a));
        relationship(
            Books {
                personal: &p,
                corp: &c,
                alliance: &a,
            },
            BOB,
            CORP_B,
            ally,
        )
    }

    #[test]
    fn each_book_fills_its_own_three_slots() {
        let r = rel(
            &[(BOB, 1.0)],
            &[(CORP_B, -10.0)],
            &[(ALLY_Y, 10.0)],
            Some(ALLY_Y),
        );
        assert_eq!(r.pers_to_pers, 1.0);
        assert_eq!(r.corp_to_corp, -10.0);
        assert_eq!(r.alliance_to_alliance, 10.0);
        assert_eq!(r.pers_to_corp, 0.0);
        assert!(r.has_relationship);
    }

    #[test]
    fn an_absent_alliance_is_never_looked_up() {
        let r = rel(&[], &[], &[], None);
        assert_eq!(r, Relationships::default());
        assert_eq!(standing_flag(&r), StandingFlag::NoStanding);
    }

    #[test]
    fn a_corp_contact_at_minus_ten_is_horrible() {
        let r = rel(&[], &[(BOB, -10.0)], &[], None);
        assert_eq!(r.corp_to_pers, -10.0);
        assert_eq!(standing_flag(&r), StandingFlag::Horrible);
    }

    /// The client's order: any blue beats any red, whoever set it.
    #[test]
    fn the_bluest_of_the_nine_wins() {
        let r = rel(&[(BOB, -10.0)], &[], &[(CORP_B, 5.0)], None);
        assert_eq!(standing_flag(&r), StandingFlag::Good);
        let r = rel(&[(BOB, 3.0)], &[(CORP_B, 10.0)], &[], None);
        assert_eq!(standing_flag(&r), StandingFlag::High);
        let r = rel(&[(BOB, -5.0)], &[(CORP_B, -10.0)], &[], None);
        assert_eq!(standing_flag(&r), StandingFlag::Horrible);
    }

    #[test]
    fn the_band_edges_are_the_clients() {
        let one = |v: f64| standing_flag(&rel(&[(BOB, v)], &[], &[], None));
        assert_eq!(one(10.0), StandingFlag::High);
        assert_eq!(one(5.0), StandingFlag::Good);
        assert_eq!(one(0.5), StandingFlag::Good);
        assert_eq!(one(0.0), StandingFlag::Neutral);
        assert_eq!(one(-0.5), StandingFlag::Bad);
        assert_eq!(one(-5.0), StandingFlag::Bad);
        assert_eq!(one(-5.5), StandingFlag::Horrible);
    }

    #[test]
    fn allied_by_corp_alliance_and_coalition() {
        let none = |_: i64, _: i64| None;
        assert!(allied(
            aff(ALICE, CORP_A, None),
            aff(BOB, CORP_A, None),
            none
        ));
        assert!(!allied(
            aff(ALICE, NPC_CORP, None),
            aff(BOB, NPC_CORP, None),
            none
        ));
        assert!(allied(
            aff(ALICE, CORP_A, Some(ALLY_X)),
            aff(BOB, CORP_B, Some(ALLY_X)),
            none
        ));
        assert!(!allied(
            aff(ALICE, CORP_A, None),
            aff(BOB, CORP_B, None),
            none
        ));
        // Alliance X sets alliance Y blue: a coalition, seen from X.
        let x_likes_y = |o: i64, t: i64| (o == ALLY_X && t == ALLY_Y).then_some(5.0);
        let a = aff(ALICE, CORP_A, Some(ALLY_X));
        let b = aff(BOB, CORP_B, Some(ALLY_Y));
        assert!(allied(a, b, x_likes_y));
        assert!(!allied(b, a, x_likes_y), "one-sided until Y returns it");
        // Corp A sets alliance Y blue; a red or neutral entry is not a friend.
        let a_to_y = |v: f64| move |o: i64, t: i64| (o == CORP_A && t == ALLY_Y).then_some(v);
        assert!(allied(a, b, a_to_y(10.0)));
        assert!(!allied(a, b, a_to_y(0.0)));
        assert!(!allied(a, b, a_to_y(-10.0)));
    }

    /// **REL-FIX D2**: a lone pilot in an NPC corp is allied through a
    /// group book entry on their character id, or the viewer's personal one;
    /// an unlisted pilot in the same NPC corp is not.
    #[test]
    fn a_lone_pilot_is_allied_by_a_char_id_entry_or_a_personal_contact() {
        let none = |_: i64, _: i64| None;
        let a = aff(ALICE, CORP_A, None);
        let b = aff(BOB, NPC_CORP, None);
        assert!(!allied(a, b, none));
        let corp_to_bob = |v: f64| move |o: i64, t: i64| (o == CORP_A && t == BOB).then_some(v);
        assert!(allied(a, b, corp_to_bob(5.0)));
        assert!(!allied(a, b, corp_to_bob(-5.0)));
        assert!(!allied(a, b, corp_to_bob(0.0)));
        let mine = |v: f64| move |id: i64| (id == BOB).then_some(v);
        assert!(allied_with(a, b, none, mine(3.0)));
        assert!(!allied_with(a, b, none, mine(-3.0)));
        assert!(!allied_with(a, aff(CAROL, NPC_CORP, None), none, mine(3.0)));
        // One-sided: Bob's book says nothing about Alice.
        assert!(!allied_with(b, a, none, mine(3.0)));
    }

    // ---- over the registry ----

    fn world() -> Registry {
        let pilot = |id, name: &str, corp| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: id + 10_000_000,
            corporation_id: corp,
            balance: 1.0e9,
            ..Character::default()
        };
        Registry::new(SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice", NPC_CORP),
                pilot(BOB, "Bob", NPC_CORP),
                pilot(CAROL, "Carol", NPC_CORP),
            ],
            ..SandboxConfig::default()
        })
    }

    fn call(
        reg: &mut Registry,
        actor: i64,
        method: &str,
        args: &[PyValue],
    ) -> Option<Result<PyValue, ServiceError>> {
        corp_registry(
            reg,
            &Request {
                service: "corpRegistry",
                method,
                args,
                kwargs: &[],
                bind_params: None,
                actor: Some(actor),
            },
        )
    }

    /// A player corp founded the way the create form does it
    /// (`service/corp.rs`'s own test helper sends the same fifteen).
    fn found_corp(reg: &mut Registry, ceo: i64) -> i64 {
        let uni = |s: &str| PyValue::Unicode(s.to_owned());
        let args = [
            uni("Invented Corp"),
            uni("INVC"),
            uni("A test corp."),
            uni("http://"),
            PyValue::Real(0.1),
            PyValue::Int(419),
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::Bool(true),
            PyValue::Bool(false),
            PyValue::Real(0.0),
        ];
        reg.dispatch(&Request {
            service: "corpRegistry",
            method: "AddCorporation",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ceo),
        })
        .expect("founded");
        let corp = reg.character(ceo).expect("a pilot").corporation_id;
        assert!(reg.corp_membership(ceo).is_some(), "a player corp");
        corp
    }

    #[test]
    fn a_member_without_the_role_cannot_edit_and_others_fall_through() {
        let mut reg = world();
        let r = call(
            &mut reg,
            ALICE,
            "AddCorporateContact",
            &[PyValue::Int(BOB), PyValue::Int(-10)],
        );
        assert!(matches!(r, Some(Err(_))), "no player corp: refused");
        assert!(call(&mut reg, ALICE, "GetMember", &[]).is_none());
        let r = call(&mut reg, ALICE, "GetCorporateContacts", &[]).expect("ours");
        assert_eq!(r.expect("answered"), PyValue::Dict(vec![]));
    }

    /// **REL-FIX D2 over the registry**: Alice's personal contact on Bob (an
    /// NPC-corp pilot) makes Bob allied to her, and only hers; a red one
    /// does not.
    #[test]
    fn a_personal_contact_makes_a_lone_pilot_allied() {
        let mut reg = world();
        assert!(!allied_in(&reg, ALICE, BOB));
        let add = |reg: &mut Registry, who: i64, rel: i64| {
            reg.dispatch(&Request {
                service: "charMgr",
                method: "AddContact",
                args: &[PyValue::Int(who), PyValue::Int(rel)],
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            })
            .expect("answered");
        };
        add(&mut reg, BOB, 3);
        assert!(allied_in(&reg, ALICE, BOB));
        assert!(!allied_in(&reg, BOB, ALICE), "Bob's book is empty");
        assert!(!allied_in(&reg, ALICE, CAROL), "an unlisted pilot");
        add(&mut reg, BOB, -5);
        assert!(!allied_in(&reg, ALICE, BOB));
    }

    #[test]
    fn a_corp_contact_colours_the_ceos_view_and_persists_as_a_delta() {
        let mut reg = world();
        let corp = found_corp(&mut reg, ALICE);
        reg.take_dirty();
        call(
            &mut reg,
            ALICE,
            "AddCorporateContact",
            &[PyValue::Int(BOB), PyValue::Int(-10)],
        )
        .expect("ours")
        .expect("added");
        assert!(reg.take_dirty(), "a change must ask for the save");
        assert_eq!(
            get(&reg, corp, BOB),
            Some(GroupContact {
                relationship: -10.0,
                label_mask: 0
            })
        );
        let r = relationship_for(&reg, ALICE, BOB).expect("both exist");
        assert_eq!(r.corp_to_pers, -10.0);
        assert_eq!(standing_flag(&r), StandingFlag::Horrible);
        // The answer the client caches.
        let v = call(&mut reg, ALICE, "GetCorporateContacts", &[])
            .unwrap()
            .unwrap();
        let PyValue::Dict(entries) = &v else {
            panic!("a dict: {v:?}")
        };
        assert_eq!(entries.len(), 1);
        // A dict entry is (value, key): the key is the contact id.
        assert_eq!(entries[0].1, PyValue::Int(BOB));
        // The delta row survives a snapshot into a fresh world.
        let snap = reg.to_snapshot();
        let mut again = world();
        again.apply_snapshot(snap);
        assert_eq!(book(&again, corp), book(&reg, corp));
        // Edit, multi-edit, remove.
        call(
            &mut reg,
            ALICE,
            "EditCorporateContact",
            &[PyValue::Int(BOB), PyValue::Int(5)],
        )
        .unwrap()
        .unwrap();
        assert!(
            allied_in(&reg, ALICE, BOB),
            "REL-FIX D2: a corp blue on a character id counts, the lone friend"
        );
        call(
            &mut reg,
            ALICE,
            "EditContactsRelationshipID",
            &[
                PyValue::List(vec![PyValue::Int(BOB), PyValue::Int(CAROL)]),
                PyValue::Int(10),
            ],
        )
        .unwrap()
        .unwrap();
        assert_eq!(get(&reg, corp, BOB).map(|c| c.relationship), Some(10.0));
        assert_eq!(get(&reg, corp, CAROL), None, "only existing entries");
        call(
            &mut reg,
            ALICE,
            "RemoveCorporateContacts",
            &[PyValue::List(vec![PyValue::Int(BOB)])],
        )
        .unwrap()
        .unwrap();
        assert!(book(&reg, corp).is_empty());
    }

    #[test]
    fn the_update_push_is_the_clients_triple_shape() {
        use crate::notify::Push as _;
        let p = OrgContactsUpdated {
            to: vec![BOB],
            owner_id: CORP_A,
            updates: vec![(CAROL, Some(-10.0)), (ALICE, None)],
        };
        let (name, args) = p.to_push();
        assert_eq!(name, "OnOrganizationContactsUpdated");
        assert_eq!(args[0], PyValue::Int(CORP_A));
        let PyValue::List(u) = &args[1] else { panic!() };
        let PyValue::Tuple(t) = &u[1] else { panic!() };
        assert_eq!(t, &vec![PyValue::Int(ALICE), PyValue::None, PyValue::None]);
        let PyValue::Tuple(t) = &u[0] else { panic!() };
        assert_eq!(t[2], PyValue::None, "labelMask None sets the relationship");
        assert_eq!(p.audience(), Some(vec![BOB]));
    }

    #[test]
    fn the_alliance_book_needs_an_alliance_and_the_executor() {
        let mut reg = world();
        let corp = found_corp(&mut reg, ALICE);
        // Real alliances: an invented contact id is refused (SEC-FIX 8), and
        // an alliance cannot befriend itself.
        let (ally_x, ally_y) = {
            let mut st = crate::alliance::Alliances::load(reg.deltas());
            let x = st
                .create(CORP_B + 10, BOB, "Own Pact", "OWN", "", "", 1)
                .expect("founds");
            let y = st
                .create(CORP_B, BOB, "Other Pact", "OTHR", "", "", 1)
                .expect("founds");
            st.save(reg.deltas_mut());
            (x, y)
        };
        let args = [PyValue::Int(ally_y), PyValue::Int(10)];
        let r = Request {
            service: "allianceRegistry",
            method: "AddAllianceContact",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        assert!(matches!(
            alliance_registry(&mut reg, &r, None),
            Some(Err(_))
        ));
        reg.character_mut(ALICE).unwrap().alliance_id = Some(ally_x);
        assert!(matches!(
            alliance_registry(&mut reg, &r, Some(corp + 1)),
            Some(Err(_))
        ));
        // SEC-FIX 1: an unknown executor is a refusal, not a skipped check.
        assert!(matches!(
            alliance_registry(&mut reg, &r, None),
            Some(Err(_))
        ));
        alliance_registry(&mut reg, &r, Some(corp))
            .unwrap()
            .unwrap();
        assert_eq!(
            get(&reg, ally_x, ally_y).map(|c| c.relationship),
            Some(10.0)
        );
        // Bob in alliance Y is now Alice's coalition friend.
        let bob = reg.character_mut(BOB).unwrap();
        bob.alliance_id = Some(ally_y);
        bob.corporation_id = CORP_B;
        assert!(allied_in(&reg, ALICE, BOB));
        assert!(!allied_in(&reg, BOB, ALICE));
        let r = relationship_for(&reg, ALICE, BOB).unwrap();
        assert_eq!(r.alliance_to_alliance, 10.0);
        assert_eq!(standing_flag(&r), StandingFlag::High);
    }

    /// SEC-FIX 8: a contact is a real owner, and a book is bounded.
    #[test]
    fn a_contact_must_exist_and_a_book_is_capped() {
        let mut reg = world();
        let corp = found_corp(&mut reg, ALICE);
        let add = |reg: &mut Registry, id: i64| {
            call(
                reg,
                ALICE,
                "AddCorporateContact",
                &[PyValue::Int(id), PyValue::Int(5)],
            )
            .expect("ours")
        };
        assert!(add(&mut reg, 7_777_777_777).is_err(), "no such owner");
        assert!(add(&mut reg, 123_456).is_err(), "no such owner");
        assert!(add(&mut reg, BOB).is_ok());
        assert!(add(&mut reg, *NPC_CORPORATION_IDS.start()).is_ok());
        // Fill the book to the cap with agent ids, then one more is refused
        // while an edit of a kept one still works.
        for n in 0..MAX_CORP_CONTACTS {
            reg.deltas_mut().put(
                KIND,
                key(corp, *AGENT_IDS.start() + n as i64),
                vec!["5".into(), "0".into()],
            );
        }
        assert!(book(&reg, corp).len() >= MAX_CORP_CONTACTS);
        assert!(
            add(&mut reg, *FACTION_IDS.start()).is_err(),
            "the book is full"
        );
        assert!(add(&mut reg, BOB).is_ok(), "an edit of a kept contact");
    }
}
