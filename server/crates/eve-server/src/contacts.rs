//! Contacts (P5.11) -- the address book, and the standings players set on
//! each other.
//!
//! Moved verbatim out of `service/character.rs` by IP37-b; the `charMgr`
//! arms that serve it stay there. `docs/lanes/contacts.md` has the research
//! (every call, every push, and how a contact standing becomes an overview
//! colour), the coordinator edits, and what is deliberately not modelled.
//!
//! **Why this matters more than its size.** These are the standings that
//! colour one player's ship in another player's overview:
//! `StateSvc._GetRelationship` (`state.py:916-921`) asks
//! `addressbook.GetRelationship(slimItem.ownerID, slimItem.corpID,
//! slimItem.allianceID)`, and that reads only the address book this module
//! serves (`addressbookService.py:321-343`). NPC standings never enter it.

use crate::pyutil::keyval;
use crate::service::{Registry, Request, ServiceError};
use crate::standings::{MAX_STANDING, MIN_STANDING};
use eve_marshal::PyValue;

/// One entry in a character's address book.
///
/// The four fields are the four attributes `addressbookService.GetContacts`
/// copies off each row (`:297-302`) **[V]**: `contactID`, `inWatchlist`,
/// `relationshipID`, `labelMask`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Contact {
    /// `relationshipID`: the standing this character has set, -10 ..= 10.
    /// The window offers exactly `contactHighStanding` (10),
    /// `contactGoodStanding` (5), `contactNeutralStanding` (0),
    /// `contactBadStanding` (-5) and `contactHorribleStanding` (-10)
    /// (`appConst.py:2260-2264`, `contactManagementWindow.py:26-30`) **[V]**,
    /// but the overview's tests are ranges (`state.py:68-105`), so any value
    /// in between is stored as given.
    pub relationship: f64,
    /// `inWatchlist`: whether this character is told when the contact logs
    /// on and off ([`ContactPresence`]).
    pub watchlist: bool,
    /// `labelMask`: a bit per contact label. Kept so that an edit does not
    /// clear it; nothing here assigns labels yet (see the note).
    pub label_mask: i64,
}

/// Every character's address book, and every character's block list.
///
/// Keyed `owner -> contact`. **One-directional, like the client's:** Alice
/// adding Bob at -10 changes how Bob looks to Alice and nothing about how
/// Alice looks to Bob, because Bob's overview reads Bob's own book.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Contacts {
    books: std::collections::BTreeMap<i64, std::collections::BTreeMap<i64, Contact>>,
    blocked: std::collections::BTreeMap<i64, std::collections::BTreeSet<i64>>,
}

impl Contacts {
    /// The record tags this type reads and writes in the world file. The
    /// same opaque shape as `Bookmarks`: `store.rs` only routes these.
    pub(crate) const RECORDS: &'static [&'static str] = &["contact", "contactblock"];

    /// One contact of one owner, if there is one.
    #[cfg(test)]
    pub(crate) fn get(&self, owner: i64, contact_id: i64) -> Option<Contact> {
        self.books.get(&owner)?.get(&contact_id).copied()
    }

    /// Add or replace a contact. An existing entry keeps its labels.
    pub(crate) fn put(&mut self, owner: i64, contact_id: i64, relationship: f64, watchlist: bool) {
        let book = self.books.entry(owner).or_default();
        let label_mask = book.get(&contact_id).map_or(0, |c| c.label_mask);
        book.insert(
            contact_id,
            Contact {
                relationship,
                watchlist,
                label_mask,
            },
        );
    }

    /// Change the standing of contacts that exist. Ids that are not in the
    /// book are skipped: the client only offers this on entries it lists.
    pub(crate) fn set_relationship(&mut self, owner: i64, ids: &[i64], relationship: f64) -> usize {
        let Some(book) = self.books.get_mut(&owner) else {
            return 0;
        };
        let mut changed = 0;
        for id in ids {
            if let Some(c) = book.get_mut(id) {
                c.relationship = relationship;
                changed += 1;
            }
        }
        changed
    }

    /// GAPS-2: set (`on`) or clear a label bit on those of `ids` that are
    /// in the book; returns how many changed. The bit is a contact label's
    /// id (`crate::contact_labels`).
    pub(crate) fn set_label(&mut self, owner: i64, ids: &[i64], bit: i64, on: bool) -> usize {
        let Some(book) = self.books.get_mut(&owner) else {
            return 0;
        };
        let mut changed = 0;
        for id in ids {
            if let Some(c) = book.get_mut(id) {
                let next = if on {
                    c.label_mask | bit
                } else {
                    c.label_mask & !bit
                };
                if next != c.label_mask {
                    c.label_mask = next;
                    changed += 1;
                }
            }
        }
        changed
    }

    /// GAPS-2: clear a label bit from every contact of `owner` (a label was
    /// deleted); whether any contact had it.
    pub(crate) fn clear_label(&mut self, owner: i64, bit: i64) -> bool {
        let Some(book) = self.books.get_mut(&owner) else {
            return false;
        };
        let mut any = false;
        for c in book.values_mut() {
            if c.label_mask & bit != 0 {
                c.label_mask &= !bit;
                any = true;
            }
        }
        any
    }

    /// Remove contacts; returns how many were there.
    pub(crate) fn remove(&mut self, owner: i64, ids: &[i64]) -> usize {
        let Some(book) = self.books.get_mut(&owner) else {
            return 0;
        };
        let removed = ids.iter().filter(|id| book.remove(id).is_some()).count();
        if book.is_empty() {
            self.books.remove(&owner);
        }
        removed
    }

    /// Block or unblock owners; returns whether anything changed.
    pub(crate) fn set_blocked(&mut self, owner: i64, ids: &[i64], blocked: bool) -> bool {
        let set = self.blocked.entry(owner).or_default();
        let changed = ids
            .iter()
            .filter(|id| {
                if blocked {
                    set.insert(**id)
                } else {
                    set.remove(*id)
                }
            })
            .count();
        if set.is_empty() {
            self.blocked.remove(&owner);
        }
        changed > 0
    }

    /// One owner's book, in contact-id order.
    pub(crate) fn book(&self, owner: i64) -> Vec<(i64, Contact)> {
        self.books
            .get(&owner)
            .map(|b| b.iter().map(|(id, c)| (*id, *c)).collect())
            .unwrap_or_default()
    }

    /// One owner's block list, in id order.
    pub(crate) fn blocked_by(&self, owner: i64) -> Vec<i64> {
        self.blocked
            .get(&owner)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Every owner who has `character_id` in their book **with the watchlist
    /// box ticked** -- the audience for that character's log-on and log-off.
    pub(crate) fn watchers_of(&self, character_id: i64) -> Vec<i64> {
        self.books
            .iter()
            .filter(|(owner, book)| {
                **owner != character_id && book.get(&character_id).is_some_and(|c| c.watchlist)
            })
            .map(|(owner, _)| *owner)
            .collect()
    }

    /// Append this type's records to a world file being written.
    ///
    /// `contact\t<owner>\t<contactID>\t<relationship>\t<watch 0|1>\t<labelMask>`
    /// and `contactblock\t<owner>\t<blockedID>`. Nothing at all for a world
    /// with no contacts, so an older file reads back identical.
    pub(crate) fn render(&self, out: &mut String) {
        use std::fmt::Write as _;
        for (owner, book) in &self.books {
            for (id, c) in book {
                let _ = writeln!(
                    out,
                    "contact\t{owner}\t{id}\t{}\t{}\t{}",
                    c.relationship,
                    u8::from(c.watchlist),
                    c.label_mask
                );
            }
        }
        for (owner, ids) in &self.blocked {
            for id in ids {
                let _ = writeln!(out, "contactblock\t{owner}\t{id}");
            }
        }
    }

    /// Read one record whose tag is in [`Contacts::RECORDS`]. Strict, like
    /// the bookmarks: a line we cannot read is a contact the player would
    /// silently lose, so the world is refused and the line named.
    pub(crate) fn parse_record(&mut self, f: &[&str]) -> Result<(), String> {
        let num = |i: usize| -> Result<i64, String> {
            f.get(i)
                .and_then(|v| v.parse::<i64>().ok())
                .ok_or_else(|| format!("field {i} is not an integer"))
        };
        match f.first().copied() {
            Some("contact") => {
                let relationship = f
                    .get(3)
                    .and_then(|v| v.parse::<f64>().ok())
                    .filter(|r| r.is_finite())
                    .ok_or_else(|| "field 3 is not a number".to_owned())?;
                let (owner, id) = (num(1)?, num(2)?);
                self.books.entry(owner).or_default().insert(
                    id,
                    Contact {
                        relationship,
                        watchlist: num(4)? != 0,
                        label_mask: num(5)?,
                    },
                );
            }
            Some("contactblock") => {
                self.blocked.entry(num(1)?).or_default().insert(num(2)?);
            }
            other => return Err(format!("{other:?} is not a contacts record")),
        }
        Ok(())
    }
}

/// A relationship as the client should see it: an `int` when it is a whole
/// number, which is every value the window offers and the type the client
/// stores in its own cache after an add (`appConst.contactHighStanding` and
/// friends are ints). **[A]** that the real server sent the same; every
/// reader compares numerically (`state.py:68-105`), so either type works.
pub(crate) fn relationship_value(r: f64) -> PyValue {
    if r.fract() == 0.0 && r.abs() <= MAX_STANDING {
        PyValue::Int(r as i64)
    } else {
        PyValue::Real(r)
    }
}

/// `charMgr.GetContactList`: `KeyVal(addresses=[...], blocked=[...])`.
///
/// **[V]** `addressbookService.GetContacts` (`:290-308`):
///
/// - each `addresses` row is read for `.contactID`, `.inWatchlist`,
///   `.relationshipID` and `.labelMask` and copied into a fresh `KeyVal`
///   (`:297-302`) -- except when `IsNPC(contactID) and
///   IsCharacter(contactID)`, an agent, which only has its `.contactID`
///   appended to `self.agents` (`:293-295`);
/// - each `blocked` row is read for **`.senderID`**, not `.contactID`, and
///   skipped when it is `0` (`:305-308`).
///
/// Nothing else is done with either list -- no `.Index`, no `len`, no
/// header -- so a list of `KeyVal`s is the shape; the real server's row
/// family cannot matter to these readers.
pub(crate) fn contact_list_value(reg: &Registry, actor: Option<i64>) -> PyValue {
    let (addresses, blocked) = match actor {
        Some(me) => (
            reg.contacts
                .book(me)
                .into_iter()
                .map(|(id, c)| {
                    keyval(vec![
                        ("contactID", PyValue::Int(id)),
                        ("inWatchlist", PyValue::Bool(c.watchlist)),
                        ("relationshipID", relationship_value(c.relationship)),
                        ("labelMask", PyValue::Int(c.label_mask)),
                    ])
                })
                .collect(),
            reg.contacts
                .blocked_by(me)
                .into_iter()
                .map(|id| keyval(vec![("senderID", PyValue::Int(id))]))
                .collect(),
        ),
        None => (vec![], vec![]),
    };
    keyval(vec![
        ("addresses", PyValue::List(addresses)),
        ("blocked", PyValue::List(blocked)),
    ])
}

pub(crate) fn contacts_bad_args(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

/// An id list argument. The client always sends a list
/// (`DeleteContacts([agentID])` at `:975`, `BlockOwners([ownerID])` at
/// `:792`, a selection everywhere else); a bare id is accepted as a list
/// of one rather than refused.
pub(crate) fn id_list(v: Option<&PyValue>) -> Option<Vec<i64>> {
    match v? {
        PyValue::Int(id) => Some(vec![*id]),
        PyValue::List(items) | PyValue::Tuple(items) => items
            .iter()
            .map(|i| match i {
                PyValue::Int(id) => Some(*id),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

/// A relationship argument: an int or a real in -10 ..= 10. Absent or `None`
/// is 0, the neutral standing -- the one-argument agent form
/// (`AddContact(agentID)`, `:874`) has no relationship at all.
pub(crate) fn relationship_arg(v: Option<&PyValue>) -> Result<f64, String> {
    let r = match v {
        None | Some(PyValue::None) => 0.0,
        Some(PyValue::Int(n)) => *n as f64,
        Some(PyValue::Real(r)) => *r,
        Some(other) => return Err(format!("relationship {other:?} is not a number")),
    };
    if r.is_finite() && (MIN_STANDING..=MAX_STANDING).contains(&r) {
        Ok(r)
    } else {
        Err(format!("relationship {r} is outside -10 ..= 10"))
    }
}

/// `AddContact` and `EditContact`, one function: both are an upsert of one
/// entry with the same five positional arguments.
///
/// **[V]** three arities, all real:
///
/// - `AddContact(agentID)` -- the agent path, `addressbookService.py:874`;
/// - `AddContact(characterID, relationship)` -- `form_tournament.py:428/430`,
///   with `-10` and `10`;
/// - `getattr(charMgr, 'AddContact' | 'EditContact')(contactID,
///   relationshipID, inWatchlist, sendNotification, message)` through
///   `eveMisc.CSPAChargedAction` (`:941`, `eveMisc.py:145`). That indirect
///   call is why neither name appears in the call census.
///
/// The return is discarded by every caller. `CSPAChargedAction` catches a
/// `UserError('ContactCostNotApproved')` to ask the player to pay a CSPA
/// charge (`eveMisc.py:147-155`); we charge nothing, so we never raise it.
///
/// Not done, **[A]**: `sendNotification` and `message` would have the real
/// server send the added character an "added you as a contact"
/// notification. Accepted and ignored.
pub(crate) fn add_contact(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(me) = req.actor else {
        return Err(contacts_bad_args(req, "no character"));
    };
    let Some(PyValue::Int(contact_id)) = req.args.first() else {
        return Err(contacts_bad_args(
            req,
            "the first argument is the contact id",
        ));
    };
    let contact_id = *contact_id;
    // The client refuses these itself (`UI/PeopleAndPlaces/CannotAddSelf`,
    // `:847-849`; `CantbookmarkEveSystem`, `:758-760`), so reaching here
    // means a client we do not understand. Refused rather than stored.
    if contact_id == me || contact_id <= 1 {
        return Err(contacts_bad_args(req, "not an owner that can be a contact"));
    }
    let relationship =
        relationship_arg(req.args.get(1)).map_err(|why| contacts_bad_args(req, &why))?;
    let watchlist = match req.args.get(2) {
        Some(PyValue::Bool(b)) => *b,
        Some(PyValue::Int(n)) => *n != 0,
        _ => false,
    };
    reg.contacts.put(me, contact_id, relationship, watchlist);
    reg.mark_dirty();
    Ok(PyValue::None)
}

/// `EditContactsRelationshipID(contactIDs, relationshipID)` -- the
/// multi-select edit, `addressbookService.py:831`. **[V]**
pub(crate) fn edit_contacts_relationship(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(me) = req.actor else {
        return Err(contacts_bad_args(req, "no character"));
    };
    let ids = id_list(req.args.first())
        .ok_or_else(|| contacts_bad_args(req, "contactIDs is a list of ids"))?;
    let relationship =
        relationship_arg(req.args.get(1)).map_err(|why| contacts_bad_args(req, &why))?;
    if reg.contacts.set_relationship(me, &ids, relationship) > 0 {
        reg.mark_dirty();
    }
    Ok(PyValue::None)
}

/// `DeleteContacts(contactIDs)` -- `:975` (an agent) and `:1000`. **[V]**
pub(crate) fn delete_contacts(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(me) = req.actor else {
        return Err(contacts_bad_args(req, "no character"));
    };
    let ids = id_list(req.args.first())
        .ok_or_else(|| contacts_bad_args(req, "contactIDs is a list of ids"))?;
    if reg.contacts.remove(me, &ids) > 0 {
        reg.mark_dirty();
    }
    Ok(PyValue::None)
}

/// `BlockOwners([ownerID])` (`:792`) and `UnblockOwners(ownerIDs)` (`:809`).
/// **[V]** signatures; the client keeps its own `self.blocked` and scatters
/// its own events. What blocking *does* on a real server -- dropping that
/// owner's mail, chat invites and convos -- is not modelled: nothing here
/// delivers any of those from one player to another yet.
pub(crate) fn block_owners(
    reg: &mut Registry,
    req: &Request<'_>,
    block: bool,
) -> Result<PyValue, ServiceError> {
    let Some(me) = req.actor else {
        return Err(contacts_bad_args(req, "no character"));
    };
    let ids = id_list(req.args.first())
        .ok_or_else(|| contacts_bad_args(req, "ownerIDs is a list of ids"))?;
    if reg.contacts.set_blocked(me, &ids, block) {
        reg.mark_dirty();
    }
    Ok(PyValue::None)
}

/// `OnContactLoggedOn(charID)` / `OnContactLoggedOff(charID)`.
///
/// **[V]** one argument, the character who came or went:
/// `onlineStatus.OnContactLoggedOn(self, charID)` (`onlineStatus.py:34-41`)
/// writes `blue.DBRow(self.onlineStatus.header, [charID, True|False])`
/// into the cache the Contacts window's online dot reads, and
/// `ContactNotificationAdapter` (`contactNotificationAdapter.py:14-36`)
/// turns the same event into the "<name> is online / offline" notification.
///
/// Audience **[A]**: every online character who has this one in their book
/// with the watchlist ticked. The client's own wording ("watch list") and
/// the fact that it scatters `OnContactAddedToWatchlist` only for
/// `inWatchlist` contacts (`addressbookService.py:965-968`) are why.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ContactPresence {
    pub to: Vec<i64>,
    pub character_id: i64,
    pub online: bool,
}

impl crate::notify::Push for ContactPresence {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            if self.online {
                "OnContactLoggedOn"
            } else {
                "OnContactLoggedOff"
            },
            vec![PyValue::Int(self.character_id)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// The presence frame for a character who just came online or went offline,
/// or `None` when no one online is watching. Built, not queued, like
/// `Registry::station_guest_frame`: the log-on path queues it on the turn
/// (`push`), the log-off path on the clock (`schedule`), because a
/// disconnect is not a payload and nothing would drain the turn queue.
pub(crate) fn contact_presence(
    reg: &Registry,
    character_id: i64,
    online: bool,
) -> Option<ContactPresence> {
    let to: Vec<i64> = reg
        .contacts
        .watchers_of(character_id)
        .into_iter()
        .filter(|w| reg.sessions().is_online(*w))
        .collect();
    if to.is_empty() {
        return None;
    }
    Some(ContactPresence {
        to,
        character_id,
        online,
    })
}

#[cfg(test)]
mod contacts_tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};

    /// The tests were written inside `service/character.rs` and called its
    /// `dispatch`; `Registry::dispatch` routes `charMgr` to that one
    /// function (no service is claimed by two modules), so this is the same
    /// call made from outside the private `service::character` module.
    fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
        reg.dispatch(req)
    }

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;

    fn two() -> Registry {
        let alice = Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: 100_000_001,
            ..Character::default()
        };
        let bob = Character {
            character_id: BOB,
            name: "Bob".to_owned(),
            ship_id: 100_000_002,
            ..Character::default()
        };
        Registry::new(SandboxConfig {
            characters: vec![alice, bob],
            ..SandboxConfig::default()
        })
    }

    fn call(
        reg: &mut Registry,
        actor: i64,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        dispatch(
            reg,
            &Request {
                service: "charMgr",
                method,
                args,
                kwargs: &[],
                bind_params: None,
                actor: Some(actor),
            },
        )
    }

    fn attr<'a>(v: &'a PyValue, name: &str) -> &'a PyValue {
        let PyValue::Instance { state, .. } = v else {
            panic!("expected a KeyVal, got {v:?}")
        };
        crate::pyutil::dict_get(state, name.as_bytes())
            .unwrap_or_else(|| panic!("no .{name} on {v:?}"))
    }

    fn list<'a>(v: &'a PyValue, name: &str) -> &'a [PyValue] {
        let PyValue::List(items) = attr(v, name) else {
            panic!(".{name} must be a list")
        };
        items
    }

    fn contact_list(reg: &mut Registry, actor: i64) -> PyValue {
        call(reg, actor, "GetContactList", &[]).expect("answered")
    }

    /// The five-argument form `CSPAChargedAction` makes, `:941`.
    fn add5(reg: &mut Registry, me: i64, who: i64, rel: i64, watch: bool) {
        call(
            reg,
            me,
            "AddContact",
            &[
                PyValue::Int(who),
                PyValue::Int(rel),
                PyValue::Bool(watch),
                PyValue::Bool(false),
                PyValue::None,
            ],
        )
        .expect("added");
    }

    /// A new character's book is empty, and both attributes the client
    /// iterates are present.
    #[test]
    fn an_empty_book_still_carries_both_lists() {
        let mut reg = two();
        let v = contact_list(&mut reg, ALICE);
        assert!(list(&v, "addresses").is_empty());
        assert!(list(&v, "blocked").is_empty());
    }

    /// The goal of the packet, and every attribute `GetContacts` reads
    /// (`:297-302`).
    #[test]
    fn an_added_contact_comes_back_with_the_four_attributes_the_client_reads() {
        let mut reg = two();
        add5(&mut reg, ALICE, BOB, -10, true);
        let v = contact_list(&mut reg, ALICE);
        let [row] = list(&v, "addresses") else {
            panic!("one contact")
        };
        assert_eq!(attr(row, "contactID"), &PyValue::Int(BOB));
        // Compared as a number: whether it travels as an int or a real is
        // ours (`relationship_value`), and every reader compares it.
        let rel = match attr(row, "relationshipID") {
            PyValue::Int(n) => *n as f64,
            PyValue::Real(r) => *r,
            other => panic!("relationshipID must be a number, got {other:?}"),
        };
        assert_eq!(rel, -10.0);
        assert_eq!(attr(row, "inWatchlist"), &PyValue::Bool(true));
        assert_eq!(attr(row, "labelMask"), &PyValue::Int(0));
        // One-directional: Bob's book is his own.
        assert!(list(&contact_list(&mut reg, BOB), "addresses").is_empty());
    }

    /// The other two arities the client really uses.
    #[test]
    fn the_one_and_two_argument_forms_are_accepted() {
        let mut reg = two();
        call(
            &mut reg,
            ALICE,
            "AddContact",
            &[PyValue::Int(BOB), PyValue::Int(10)],
        )
        .expect("tournament form");
        assert_eq!(
            reg.contacts.get(ALICE, BOB).map(|c| c.relationship),
            Some(10.0)
        );
        call(&mut reg, ALICE, "AddContact", &[PyValue::Int(3_008_416)]).expect("agent form");
        assert!(reg.contacts.get(ALICE, 3_008_416).is_some());
    }

    #[test]
    fn edit_multi_edit_and_delete_change_the_book() {
        let mut reg = two();
        add5(&mut reg, ALICE, BOB, -10, false);
        call(
            &mut reg,
            ALICE,
            "EditContact",
            &[
                PyValue::Int(BOB),
                PyValue::Int(5),
                PyValue::Bool(true),
                PyValue::Bool(false),
                PyValue::None,
            ],
        )
        .expect("edited");
        assert_eq!(
            reg.contacts.get(ALICE, BOB),
            Some(Contact {
                relationship: 5.0,
                watchlist: true,
                label_mask: 0
            })
        );
        call(
            &mut reg,
            ALICE,
            "EditContactsRelationshipID",
            &[PyValue::List(vec![PyValue::Int(BOB)]), PyValue::Int(-5)],
        )
        .expect("multi-edited");
        assert_eq!(
            reg.contacts.get(ALICE, BOB).map(|c| c.relationship),
            Some(-5.0)
        );
        call(
            &mut reg,
            ALICE,
            "DeleteContacts",
            &[PyValue::List(vec![PyValue::Int(BOB)])],
        )
        .expect("deleted");
        assert!(list(&contact_list(&mut reg, ALICE), "addresses").is_empty());
    }

    /// Blocked rows are read for `.senderID` (`:305-308`), not `.contactID`.
    #[test]
    fn a_blocked_owner_comes_back_as_a_sender_id() {
        let mut reg = two();
        call(
            &mut reg,
            ALICE,
            "BlockOwners",
            &[PyValue::List(vec![PyValue::Int(BOB)])],
        )
        .expect("blocked");
        let v = contact_list(&mut reg, ALICE);
        let [row] = list(&v, "blocked") else {
            panic!("one blocked")
        };
        assert_eq!(attr(row, "senderID"), &PyValue::Int(BOB));
        call(
            &mut reg,
            ALICE,
            "UnblockOwners",
            &[PyValue::List(vec![PyValue::Int(BOB)])],
        )
        .expect("unblocked");
        assert!(list(&contact_list(&mut reg, ALICE), "blocked").is_empty());
    }

    /// Through a real world file and into a fresh world, the way a restart
    /// does it; and the change asks for the save at all.
    #[test]
    fn contacts_survive_the_world_file() {
        let mut reg = two();
        reg.take_dirty();
        add5(&mut reg, ALICE, BOB, -10, true);
        call(
            &mut reg,
            BOB,
            "BlockOwners",
            &[PyValue::List(vec![PyValue::Int(ALICE)])],
        )
        .expect("blocked");
        assert!(reg.take_dirty(), "a change must mark the world dirty");
        let dir = std::env::temp_dir().join(format!(
            "eve-contacts-{}-{:?}",
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
        let mut fresh = two();
        assert!(
            fresh.contacts.get(ALICE, BOB).is_none(),
            "or this proves nothing"
        );
        fresh.apply_snapshot(snap);
        assert_eq!(fresh.contacts, reg.contacts);
    }

    #[test]
    fn contacts_round_trip_through_their_records() {
        let mut c = Contacts::default();
        c.put(ALICE, BOB, -7.5, true);
        c.put(ALICE, 1_000_125, 10.0, false);
        c.set_blocked(BOB, &[ALICE], true);
        let mut text = String::new();
        c.render(&mut text);
        let mut back = Contacts::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            assert!(Contacts::RECORDS.contains(&f[0]));
            back.parse_record(&f).expect("a record we wrote");
        }
        assert_eq!(back, c);
    }

    /// The push's name and its one argument, `onlineStatus.py:34-41`.
    #[test]
    fn the_presence_push_is_one_character_id() {
        let on = ContactPresence {
            to: vec![ALICE],
            character_id: BOB,
            online: true,
        };
        use crate::notify::Push as _;
        assert_eq!(on.to_push(), ("OnContactLoggedOn", vec![PyValue::Int(BOB)]));
        let off = ContactPresence {
            online: false,
            ..on
        };
        assert_eq!(
            off.to_push(),
            ("OnContactLoggedOff", vec![PyValue::Int(BOB)])
        );
        assert_eq!(off.audience(), Some(vec![ALICE]));
    }
}
