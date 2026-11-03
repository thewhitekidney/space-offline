//! **Personal notes** (GAPS-2, X-GAPS item 3; lane note
//! `docs/lanes/gaps-2.md`): the note on a character or corporation in the
//! People & Places info window, and the notepad's own notes and settings.
//!
//! Two stores, one delta kind ([`KIND`]), both per character and both
//! private to it:
//!
//! - **a note on an owner** -- `charMgr.GetNote(ownerID)` (the info window's
//!   Notes tab, `panelNotes.Load:40`) answers its text and
//!   `SetNote(ownerID, text)` (`panelNotes._SaveNodeThread:67`,
//!   `notepad.SaveNote:624`) replaces it [V]. Rows `char "i" owner -> text`.
//! - **the notepad's notes** -- `charMgr.GetOwnerNoteLabels()` lists every
//!   note as a row with `noteID` and `label` (`notepad.LoadNotesData:106-128`),
//!   `GetOwnerNote(noteID)` is read as `rows[0].note` and `rows[0].label`
//!   (`notepad.ShowNote:598-600`, `GetNoteText:281-286`), `AddOwnerNote(label,
//!   text)` answers the new id (`:205`, `:238`, `:709`), `EditOwnerNote(
//!   noteID, label[, text])` changes either (`:203`, `:245`, `:628`, `:734`)
//!   and `RemoveOwnerNote(noteID)` deletes (`:132`, `:210`, `:793`) [V].
//!   Rows `char "n" noteID -> label text`.
//!
//! The notepad keeps its own structure in the **label**: `"N:<name>"` is a
//! note, `"S:<setting>"` is one part of a setting string (the folder tree
//! is the setting `Folders`, split into 6,000-character parts), and `"I:..."`
//! is read back as an owner's note (`LoadNotesData:110-128`); any label that
//! does not start with `I`, `N` or `S` and a colon is deleted by the client
//! as a bad note (`:110`, `:131-132`) [V]. The server therefore stores a
//! label verbatim and interprets nothing.
//!
//! Limits are ours **[A]**, tuning that bounds what one character can make
//! the world file hold: [`MAX_TEXT_CHARS`], [`MAX_LABEL_CHARS`] and
//! [`MAX_NOTES`]. The client splits a setting into 6,000-character parts and
//! refuses a note over 3,900 (`SetSetting:199`, `SaveNote:621`) [V], so
//! neither limit is reachable from the notepad. Falsified by a retail
//! refusal at another size.

use crate::pyutil::keyval;
use crate::service::Registry;
use eve_marshal::PyValue;

/// The delta kind this module owns (`delta_kinds::KINDS`): `char "i" owner
/// -> text`, and `char "n" noteID -> label text`.
pub const KIND: &str = "note";

/// The longest note text kept, in characters. **[A]** tuning.
pub const MAX_TEXT_CHARS: usize = 8_000;
/// The longest label kept. **[A]** tuning (the notepad's names are at most
/// 80 characters and a prefix, `NewNote:703`).
pub const MAX_LABEL_CHARS: usize = 160;
/// The most notepad notes one character keeps, settings parts included.
/// **[A]** tuning.
pub const MAX_NOTES: usize = 400;

const OWNER: &str = "i";
const NOTE: &str = "n";

fn clamp(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn owner_key(ch: i64, owner: i64) -> Vec<String> {
    vec![ch.to_string(), OWNER.to_owned(), owner.to_string()]
}

fn note_key(ch: i64, id: i64) -> Vec<String> {
    vec![ch.to_string(), NOTE.to_owned(), id.to_string()]
}

// ---------------------------------------------------------------------------
// The note on an owner
// ---------------------------------------------------------------------------

/// `charMgr.GetNote(ownerID)`: the text, `""` when there is none (the edit
/// box is set to it unguarded, `panelNotes.Load:40-41`) [V].
pub fn owner_note(reg: &Registry, ch: i64, owner: i64) -> String {
    reg.deltas()
        .of(KIND)
        .get(&owner_key(ch, owner))
        .and_then(|v| v.first().cloned())
        .unwrap_or_default()
}

/// `charMgr.SetNote(ownerID, text)`. An empty text deletes the row.
pub fn set_owner_note(reg: &mut Registry, ch: i64, owner: i64, text: &str) {
    let key = owner_key(ch, owner);
    if text.is_empty() {
        reg.deltas_mut().del(KIND, &key);
    } else {
        reg.deltas_mut()
            .put(KIND, key, vec![clamp(text, MAX_TEXT_CHARS)]);
    }
}

// ---------------------------------------------------------------------------
// The notepad
// ---------------------------------------------------------------------------

/// One notepad note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub note_id: i64,
    pub label: String,
    pub text: String,
}

/// Every notepad note of `ch`, in id order.
pub fn notes(reg: &Registry, ch: i64) -> Vec<Note> {
    let mut out: Vec<Note> = reg
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            let [c, kind, id] = k.as_slice() else {
                return None;
            };
            if *c != ch.to_string() || kind != NOTE {
                return None;
            }
            Some(Note {
                note_id: id.parse().ok()?,
                label: v.first().cloned().unwrap_or_default(),
                text: v.get(1).cloned().unwrap_or_default(),
            })
        })
        .collect();
    out.sort_by_key(|n| n.note_id);
    out
}

/// One note by id.
pub fn note(reg: &Registry, ch: i64, id: i64) -> Option<Note> {
    reg.deltas().of(KIND).get(&note_key(ch, id)).map(|v| Note {
        note_id: id,
        label: v.first().cloned().unwrap_or_default(),
        text: v.get(1).cloned().unwrap_or_default(),
    })
}

fn put_note(reg: &mut Registry, ch: i64, n: &Note) {
    reg.deltas_mut().put(
        KIND,
        note_key(ch, n.note_id),
        vec![
            clamp(&n.label, MAX_LABEL_CHARS),
            clamp(&n.text, MAX_TEXT_CHARS),
        ],
    );
}

/// `AddOwnerNote(label, text)`: a new note, answering its id; `None` when
/// the character is at [`MAX_NOTES`]. Ids count up from 1 and are not reused
/// while a higher one exists.
pub fn add(reg: &mut Registry, ch: i64, label: &str, text: &str) -> Option<i64> {
    let existing = notes(reg, ch);
    if existing.len() >= MAX_NOTES {
        return None;
    }
    let id = existing.last().map_or(1, |n| n.note_id + 1);
    put_note(
        reg,
        ch,
        &Note {
            note_id: id,
            label: label.to_owned(),
            text: text.to_owned(),
        },
    );
    Some(id)
}

/// `EditOwnerNote(noteID, label[, text])`: `false` for a note that does not
/// exist (nothing is created). A `None` text keeps the text: the rename
/// sends the label alone (`notepad.RenameNote:734`) [V].
pub fn edit(reg: &mut Registry, ch: i64, id: i64, label: &str, text: Option<&str>) -> bool {
    let Some(mut n) = note(reg, ch, id) else {
        return false;
    };
    n.label = label.to_owned();
    if let Some(t) = text {
        n.text = t.to_owned();
    }
    put_note(reg, ch, &n);
    true
}

/// `RemoveOwnerNote(noteID)`.
pub fn remove(reg: &mut Registry, ch: i64, id: i64) -> bool {
    reg.deltas_mut().del(KIND, &note_key(ch, id)).is_some()
}

/// `GetOwnerNoteLabels()`: a list of rows with `noteID` and `label`.
/// **[A]** a plain list of `KeyVal`s: both readers only iterate and read
/// attributes (`LoadNotesData:106-128`); falsified by a `.Index` or
/// `len()`-of-rowset consumer, of which none was found in the client.
pub fn labels_value(reg: &Registry, ch: i64) -> PyValue {
    PyValue::List(
        notes(reg, ch)
            .into_iter()
            .map(|n| {
                keyval(vec![
                    ("noteID", PyValue::Int(n.note_id)),
                    ("label", PyValue::Unicode(n.label)),
                ])
            })
            .collect(),
    )
}

/// `GetOwnerNote(noteID)`: a one-row list with `note` and `label`
/// (`ShowNote:598-600`, `GetNoteText:281-286`). A missing note is an empty
/// list, which the client's `[0]` raises on -- as it would for retail.
pub fn note_value(reg: &Registry, ch: i64, id: i64) -> PyValue {
    PyValue::List(
        note(reg, ch, id)
            .into_iter()
            .map(|n| {
                keyval(vec![
                    ("noteID", PyValue::Int(n.note_id)),
                    ("label", PyValue::Unicode(n.label)),
                    ("note", PyValue::Unicode(n.text)),
                ])
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;

    fn world() -> Registry {
        Registry::new(SandboxConfig {
            characters: vec![Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ..Character::default()
            }],
            ..SandboxConfig::default()
        })
    }

    #[test]
    fn a_note_on_an_owner_is_private_and_clearable() {
        let mut reg = world();
        assert_eq!(owner_note(&reg, ALICE, BOB), "");
        set_owner_note(&mut reg, ALICE, BOB, "owes me a Rifter\nline two\ttab");
        assert_eq!(
            owner_note(&reg, ALICE, BOB),
            "owes me a Rifter\nline two\ttab"
        );
        assert_eq!(owner_note(&reg, BOB, ALICE), "", "Bob's notes are his own");
        set_owner_note(&mut reg, ALICE, BOB, "");
        assert_eq!(owner_note(&reg, ALICE, BOB), "");
        assert!(
            reg.deltas().of(KIND).is_empty(),
            "a cleared note leaves no row"
        );
    }

    /// The notepad's life: add a note, a setting part, edit the label alone,
    /// edit the text, remove.
    #[test]
    fn the_notepad_adds_edits_and_removes() {
        let mut reg = world();
        let a = add(&mut reg, ALICE, "N:Plans", "<br>").expect("added");
        let s = add(&mut reg, ALICE, "S:Folders", "1::F::None::Main").expect("added");
        assert_eq!((a, s), (1, 2));
        assert!(edit(&mut reg, ALICE, a, "N:Plans II", None));
        let n = note(&reg, ALICE, a).expect("there");
        assert_eq!((n.label.as_str(), n.text.as_str()), ("N:Plans II", "<br>"));
        assert!(edit(&mut reg, ALICE, a, "N:Plans II", Some("body")));
        assert_eq!(note(&reg, ALICE, a).unwrap().text, "body");
        assert!(
            !edit(&mut reg, ALICE, 99, "N:x", None),
            "edit creates nothing"
        );
        assert_eq!(notes(&reg, BOB).len(), 0);
        assert!(remove(&mut reg, ALICE, a));
        assert!(!remove(&mut reg, ALICE, a));
        assert_eq!(notes(&reg, ALICE).len(), 1);
        // A new note takes an id above the highest, not the freed one.
        assert_eq!(add(&mut reg, ALICE, "N:Again", "").unwrap(), 3);
    }

    fn attr<'a>(v: &'a PyValue, name: &str) -> &'a PyValue {
        let PyValue::Instance { state, .. } = v else {
            panic!("expected a KeyVal, got {v:?}")
        };
        crate::pyutil::dict_get(state, name.as_bytes())
            .unwrap_or_else(|| panic!("no .{name} on {v:?}"))
    }

    #[test]
    fn the_wire_shapes_are_what_the_notepad_reads() {
        let mut reg = world();
        let id = add(&mut reg, ALICE, "N:Plans", "<br>body").unwrap();
        let PyValue::List(rows) = labels_value(&reg, ALICE) else {
            panic!("a list")
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(
            attr(&rows[0], "label"),
            &PyValue::Unicode("N:Plans".to_owned())
        );
        assert_eq!(attr(&rows[0], "noteID"), &PyValue::Int(id));
        let PyValue::List(one) = note_value(&reg, ALICE, id) else {
            panic!("a list")
        };
        assert_eq!(one.len(), 1);
        assert_eq!(
            attr(&one[0], "note"),
            &PyValue::Unicode("<br>body".to_owned())
        );
        assert_eq!(
            attr(&one[0], "label"),
            &PyValue::Unicode("N:Plans".to_owned())
        );
        let PyValue::List(none) = note_value(&reg, ALICE, 99) else {
            panic!("a list")
        };
        assert!(none.is_empty());
    }

    #[test]
    fn the_caps_hold() {
        let mut reg = world();
        for i in 0..MAX_NOTES {
            assert!(add(&mut reg, ALICE, &format!("N:{i}"), "").is_some());
        }
        assert_eq!(add(&mut reg, ALICE, "N:one too many", ""), None);
        let long = "x".repeat(MAX_TEXT_CHARS + 10);
        set_owner_note(&mut reg, ALICE, BOB, &long);
        assert_eq!(owner_note(&reg, ALICE, BOB).chars().count(), MAX_TEXT_CHARS);
    }
}
