//! **Contact labels** (GAPS-2, X-GAPS item 3; lane note
//! `docs/lanes/gaps-2.md`): the coloured tags a pilot puts on the contacts
//! in their own address book.
//!
//! The client (`addressbookService.py`; for `contactType == 'contact'` its
//! service is `charMgr`, `_GetSvcForContactType:1238`) [V]:
//!
//! - `GetLabels()` -> a dict `{labelID: KeyVal(labelID, name, color)}`
//!   (`GetContactLabels:1213`; `CreateContactLabel:1197` assigns a `KeyVal`
//!   into the answer, so the dict must be mutable and its values `KeyVal`s,
//!   as `charMgr.GetLabels` has always documented);
//! - `CreateLabel(name, color)` -> the new `labelID` (`:1195`, returned to
//!   the caller at `:1199`);
//! - `EditLabel(labelID, name, color)` (`:1181`; either of the last two may
//!   be `None`, and the client raises `MailLabelMustProvideName` itself when
//!   both are, `:1178-1179`), answer dropped;
//! - `DeleteLabel(labelID)` (`:1260`), answer dropped, after which the
//!   client clears the bit from its own copy of every contact
//!   (`:1276-1279`: `labelMask &= maxLong - labelID`);
//! - `AssignLabels(contactIDs, labelID)` (`:1289`) and
//!   `RemoveLabels(contactIDs, labelID)` (`:1328`), answers dropped.
//!
//! **A label id is a bit**: the client sets it with `labelMask |= labelID`
//! (`:1302`) and reads a mask back with `GetLabelMaskAsList`, which tests
//! bit `n` and names it `2**n` (`mailSvc.py:358-369`) [V]. So an id is the
//! lowest power of two no label of the character uses, and the contact's
//! `labelMask` (already in `Contact`, and sent by `GetContacts`,
//! `contacts.rs`) carries them.
//!
//! The name is at most `const.contactMaxLabelSize` = 40 (`appConst.py`;
//! `RenameContactLabelFromUI:1167` uses `mailMaxLabelSize`, also 40) [V].
//! The number of labels is **[A]** `const.mailMaxNumLabels` = 25, the mail
//! window's cap [V as a number] -- the client names no separate one for
//! contacts; falsified by a retail refusal of the 26th contact label.
//!
//! Stored as delta kind [`KIND`]: `char label_id -> name color`.

use crate::pyutil::keyval;
use crate::service::Registry;
use eve_marshal::PyValue;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "clabel";

/// `const.contactMaxLabelSize` [V].
pub const MAX_NAME_CHARS: usize = 40;
/// **[A]** `const.mailMaxNumLabels` [V as a number; A that it applies].
pub const MAX_LABELS: usize = 25;

/// One label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub label_id: i64,
    pub name: String,
    /// The client's colour value, `None` when it sent none.
    pub color: Option<i64>,
}

fn key(ch: i64, label_id: i64) -> Vec<String> {
    vec![ch.to_string(), label_id.to_string()]
}

fn row(name: &str, color: Option<i64>) -> Vec<String> {
    vec![
        name.chars().take(MAX_NAME_CHARS).collect(),
        color.map_or(String::new(), |c| c.to_string()),
    ]
}

/// Every label of `ch`, by id.
pub fn labels(reg: &Registry, ch: i64) -> Vec<Label> {
    let mut out: Vec<Label> = reg
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            let [c, id] = k.as_slice() else {
                return None;
            };
            if *c != ch.to_string() {
                return None;
            }
            Some(Label {
                label_id: id.parse().ok()?,
                name: v.first().cloned().unwrap_or_default(),
                color: v.get(1).and_then(|c| c.parse().ok()),
            })
        })
        .collect();
    out.sort_by_key(|l| l.label_id);
    out
}

/// `GetLabels()`: `{labelID: KeyVal}`.
pub fn labels_value(reg: &Registry, ch: i64) -> PyValue {
    PyValue::Dict(
        labels(reg, ch)
            .into_iter()
            .map(|l| {
                (
                    keyval(vec![
                        ("labelID", PyValue::Int(l.label_id)),
                        ("name", PyValue::Unicode(l.name)),
                        ("color", l.color.map_or(PyValue::None, PyValue::Int)),
                    ]),
                    PyValue::Int(l.label_id),
                )
            })
            .collect(),
    )
}

/// `CreateLabel(name, color)`: the new id, `None` at [`MAX_LABELS`] or for a
/// name that is empty after trimming.
pub fn create(reg: &mut Registry, ch: i64, name: &str, color: Option<i64>) -> Option<i64> {
    let name = name.trim();
    let have = labels(reg, ch);
    if name.is_empty() || have.len() >= MAX_LABELS {
        return None;
    }
    let id = (0..MAX_LABELS as u32)
        .map(|bit| 1i64 << bit)
        .find(|id| have.iter().all(|l| l.label_id != *id))?;
    reg.deltas_mut().put(KIND, key(ch, id), row(name, color));
    Some(id)
}

/// `EditLabel(labelID, name, color)`: `false` for a label that does not
/// exist. A `None` field is kept.
pub fn edit(reg: &mut Registry, ch: i64, id: i64, name: Option<&str>, color: Option<i64>) -> bool {
    let Some(l) = labels(reg, ch).into_iter().find(|l| l.label_id == id) else {
        return false;
    };
    let new_name = match name.map(str::trim) {
        Some(n) if !n.is_empty() => n.to_owned(),
        _ => l.name,
    };
    reg.deltas_mut()
        .put(KIND, key(ch, id), row(&new_name, color.or(l.color)));
    true
}

/// `DeleteLabel(labelID)`: the label and its bit on every contact.
pub fn delete(reg: &mut Registry, ch: i64, id: i64) -> bool {
    let gone = reg.deltas_mut().del(KIND, &key(ch, id)).is_some();
    if gone && reg.contacts.clear_label(ch, id) {
        reg.mark_dirty();
    }
    gone
}

/// `AssignLabels(contactIDs, labelID)` and `RemoveLabels`: the bit set or
/// cleared on those of the contacts that are in the book. Only a label the
/// character has can be assigned. Returns how many contacts changed.
pub fn assign(reg: &mut Registry, ch: i64, ids: &[i64], label_id: i64, on: bool) -> usize {
    if on && !labels(reg, ch).iter().any(|l| l.label_id == label_id) {
        return 0;
    }
    let n = reg.contacts.set_label(ch, ids, label_id, on);
    if n > 0 {
        reg.mark_dirty();
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;
    const CAROL: i64 = 90_000_003;

    fn world() -> Registry {
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ..Character::default()
            }],
            ..SandboxConfig::default()
        });
        reg.contacts.put(ALICE, BOB, 5.0, false);
        reg.contacts.put(ALICE, CAROL, -5.0, false);
        reg
    }

    fn mask(reg: &Registry, who: i64) -> i64 {
        reg.contacts
            .book(ALICE)
            .into_iter()
            .find(|(id, _)| *id == who)
            .map_or(-1, |(_, c)| c.label_mask)
    }

    /// Ids are bits, the lowest free one; a freed bit is reused.
    #[test]
    fn label_ids_are_the_lowest_free_bits() {
        let mut reg = world();
        let a = create(&mut reg, ALICE, "Friends", Some(3)).unwrap();
        let b = create(&mut reg, ALICE, "Traders", None).unwrap();
        assert_eq!((a, b), (1, 2));
        assert!(delete(&mut reg, ALICE, a));
        assert_eq!(create(&mut reg, ALICE, "Again", None), Some(1));
        assert_eq!(create(&mut reg, ALICE, "  ", None), None, "no empty names");
        assert_eq!(labels(&reg, BOB).len(), 0, "labels are per character");
    }

    #[test]
    fn edit_keeps_what_it_is_not_given() {
        let mut reg = world();
        let a = create(&mut reg, ALICE, "Friends", Some(3)).unwrap();
        assert!(edit(&mut reg, ALICE, a, Some("Pals"), None));
        let l = &labels(&reg, ALICE)[0];
        assert_eq!((l.name.as_str(), l.color), ("Pals", Some(3)));
        assert!(edit(&mut reg, ALICE, a, None, Some(7)));
        let l = &labels(&reg, ALICE)[0];
        assert_eq!((l.name.as_str(), l.color), ("Pals", Some(7)));
        assert!(!edit(&mut reg, ALICE, 1 << 5, Some("x"), None));
    }

    /// Assign sets the bit on contacts in the book and only those; remove
    /// clears it; delete clears it everywhere.
    #[test]
    fn assignment_lives_on_the_contacts_mask() {
        let mut reg = world();
        let a = create(&mut reg, ALICE, "Friends", None).unwrap();
        let b = create(&mut reg, ALICE, "Traders", None).unwrap();
        assert_eq!(assign(&mut reg, ALICE, &[BOB, CAROL, 777], a, true), 2);
        assert_eq!(assign(&mut reg, ALICE, &[BOB], b, true), 1);
        assert_eq!((mask(&reg, BOB), mask(&reg, CAROL)), (a | b, a));
        assert_eq!(assign(&mut reg, ALICE, &[BOB], a, false), 1);
        assert_eq!(mask(&reg, BOB), b);
        assert_eq!(
            assign(&mut reg, ALICE, &[BOB], 1 << 9, true),
            0,
            "no such label"
        );
        assert!(delete(&mut reg, ALICE, b));
        assert_eq!((mask(&reg, BOB), mask(&reg, CAROL)), (0, a));
        // An edit of the standing keeps the labels (contacts.rs's rule).
        reg.contacts.put(ALICE, CAROL, 10.0, false);
        assert_eq!(mask(&reg, CAROL), a);
    }

    #[test]
    fn the_answer_is_a_dict_of_keyvals() {
        let mut reg = world();
        let a = create(&mut reg, ALICE, "Friends", Some(2)).unwrap();
        let PyValue::Dict(d) = labels_value(&reg, ALICE) else {
            panic!("a dict")
        };
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].1, PyValue::Int(a));
        assert!(format!("{:?}", d[0].0).contains("Friends"));
    }

    #[test]
    fn the_cap_holds() {
        let mut reg = world();
        for i in 0..MAX_LABELS {
            assert!(create(&mut reg, ALICE, &format!("L{i}"), None).is_some());
        }
        assert_eq!(create(&mut reg, ALICE, "one more", None), None);
    }
}
