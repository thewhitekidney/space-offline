//! **Share links** (lane `s1-share-links`, issue #81): the stored overview
//! profiles, inventory filters and broadcast settings a pilot pastes into
//! chat as a link, which another pilot opens.
//!
//! The client [V, `docs/lanes/` handback r-boosters §8]:
//!
//! - `overviewPresetMgr.StoreLinkAndGetID(data)` and
//!   `sharedSettingsMgr.StoreSettingLinkAndGetID(settingTypeID, data)` answer
//!   `None` (a `UserError` on the client) or a `KeyVal` with `hashvalue`,
//!   `sqID` (and `settingTypeID`);
//! - `GetStoredPreset((hash, sqID))` and
//!   `GetStoredSharedSetting((hash, sqID, settingTypeID))` answer the YAML
//!   text, or `None`; the client runs `yaml.safe_load` and then
//!   `GetDictFromList`, so the top level must be a list of `[key, value]`
//!   pairs even though the client sent a dict.
//!
//! The YAML is written in flow style, which is JSON-compatible, so no YAML
//! dependency is needed. Stored as delta kind [`KIND`]: `hash sq -> type owner
//! yaml`. Type 0 is an overview profile; 2 and 3 are the client's
//! `SHARED_SETTING_ITEMS` and `SHARED_SETTING_BROADCAST`. A fetch needs no
//! owner check, because the link is the capability.

use crate::service::Registry;
use eve_marshal::PyValue;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "sharelink";

/// The stored type of an overview profile.
pub const TYPE_OVERVIEW: i64 = 0;
/// `sharedSettings.SHARED_SETTING_ITEMS` [V].
pub const TYPE_ITEMS: i64 = 2;
/// `sharedSettings.SHARED_SETTING_BROADCAST` [V].
pub const TYPE_BROADCAST: i64 = 3;

/// The longest YAML kept, the same as `char_settings`. **[A]** tuning.
pub const MAX_YAML_BYTES: usize = 102_400;
/// The most links one character keeps. **[A]** tuning; falsified by a retail
/// pilot sharing more.
pub const MAX_PER_OWNER: usize = 200;
/// The deepest nesting accepted, so a hostile value cannot recurse far.
const MAX_DEPTH: usize = 32;

/// Double-quoted YAML scalar. Non-ASCII goes out as `\uXXXX` (astral planes
/// as a surrogate pair, because the client's Python is a narrow build **[A]**;
/// falsified by a mangled emoji in a loaded profile name).
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ' '..='~' => out.push(c),
            _ => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04X}"));
                }
            }
        }
    }
    out.push('"');
    out
}

fn text_of(v: &PyValue) -> Option<String> {
    match v {
        PyValue::Unicode(s) => Some(s.clone()),
        PyValue::Bytes(b) => String::from_utf8(b.clone()).ok(),
        _ => None,
    }
}

fn scalar(v: &PyValue) -> Option<String> {
    Some(match v {
        PyValue::None => "null".to_owned(),
        PyValue::Bool(b) => b.to_string(),
        PyValue::Int(n) => n.to_string(),
        PyValue::Real(f) if f.is_finite() => {
            let s = f.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        PyValue::Unicode(_) | PyValue::Bytes(_) => quote(&text_of(v)?),
        _ => return None,
    })
}

fn node(v: &PyValue, depth: usize) -> Option<String> {
    if depth > MAX_DEPTH {
        return None;
    }
    match v {
        PyValue::List(items) | PyValue::Tuple(items) => {
            let parts: Option<Vec<String>> = items.iter().map(|i| node(i, depth + 1)).collect();
            Some(format!("[{}]", parts?.join(", ")))
        }
        PyValue::Dict(entries) => {
            let mut parts = sorted_pairs(entries, depth)?;
            parts.sort();
            let body: Vec<String> = parts
                .into_iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect();
            Some(format!("{{{}}}", body.join(", ")))
        }
        other => scalar(other),
    }
}

/// `(key, value)` YAML of each entry; a marshal dict entry is `(value, key)`.
/// A key must be a scalar, because `safe_load` cannot build a list key.
fn sorted_pairs(entries: &[(PyValue, PyValue)], depth: usize) -> Option<Vec<(String, String)>> {
    entries
        .iter()
        .map(|(value, key)| Some((scalar(key)?, node(value, depth + 1)?)))
        .collect()
}

/// The YAML text of `v`, or `None` for a value that has no YAML form here.
/// A top-level dict becomes a **list of `[key, value]` pairs** (sorted by
/// key, so the same data always gives the same text), the shape the client's
/// `GetDictFromList` reads back.
pub fn to_yaml(v: &PyValue) -> Option<String> {
    match v {
        PyValue::Dict(entries) => {
            let mut parts = sorted_pairs(entries, 0)?;
            parts.sort();
            let body: Vec<String> = parts
                .into_iter()
                .map(|(k, v)| format!("[{k}, {v}]"))
                .collect();
            Some(format!("[{}]", body.join(", ")))
        }
        PyValue::List(_) | PyValue::Tuple(_) => node(v, 0),
        _ => None,
    }
}

/// 64-bit FNV-1a over the YAML, as 16 hex digits.
fn hash_of(yaml: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in yaml.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn key(hash: &str, sq: i64) -> Vec<String> {
    vec![hash.to_owned(), sq.to_string()]
}

/// A stored link: `(hashvalue, sqID)`.
pub type Link = (String, i64);

/// Store `data` as a link of `ty` for `owner`. A repeat of the same YAML and
/// type, by anyone, answers the existing link. `None` for a value with no
/// YAML form, a YAML over [`MAX_YAML_BYTES`], or an owner already at
/// [`MAX_PER_OWNER`].
pub fn store(reg: &mut Registry, owner: i64, ty: i64, data: &PyValue) -> Option<Link> {
    let yaml = to_yaml(data)?;
    if yaml.len() > MAX_YAML_BYTES {
        return None;
    }
    let hash = hash_of(&yaml);
    let (ty_s, who) = (ty.to_string(), owner.to_string());
    let mut sq = 1;
    loop {
        match reg.deltas().of(KIND).get(&key(&hash, sq)) {
            None => break,
            Some(row) if row.first() == Some(&ty_s) && row.get(2) == Some(&yaml) => {
                return Some((hash, sq));
            }
            Some(_) => sq += 1,
        }
    }
    let mine = reg
        .deltas()
        .of(KIND)
        .values()
        .filter(|r| r.get(1) == Some(&who))
        .count();
    if mine >= MAX_PER_OWNER {
        return None;
    }
    reg.deltas_mut()
        .put(KIND, key(&hash, sq), vec![ty_s, who, yaml]);
    Some((hash, sq))
}

/// The YAML behind a link of type `ty`, or `None`.
pub fn fetch(reg: &Registry, hash: &str, sq: i64, ty: i64) -> Option<String> {
    let row = reg.deltas().of(KIND).get(&key(hash, sq))?;
    if row.first()? != &ty.to_string() {
        return None;
    }
    row.get(2).cloned()
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

    fn u(s: &str) -> PyValue {
        PyValue::Unicode(s.to_owned())
    }

    /// A marshal dict entry is `(value, key)`.
    fn dict(entries: &[(&str, PyValue)]) -> PyValue {
        PyValue::Dict(entries.iter().map(|(k, v)| (v.clone(), u(k))).collect())
    }

    #[test]
    fn the_top_level_is_a_list_of_pairs_and_a_tuple_is_a_list() {
        let d = dict(&[
            ("filter", PyValue::Tuple(vec![u("mine"), PyValue::Int(1)])),
            ("alpha", PyValue::Bool(true)),
        ]);
        assert_eq!(
            to_yaml(&d).as_deref(),
            Some(r#"[["alpha", true], ["filter", ["mine", 1]]]"#)
        );
    }

    #[test]
    fn nested_dicts_scalars_and_the_unrepresentable() {
        let d = dict(&[(
            "tab",
            dict(&[("n", PyValue::Real(2.0)), ("z", PyValue::None)]),
        )]);
        assert_eq!(
            to_yaml(&d).as_deref(),
            Some(r#"[["tab", {"n": 2.0, "z": null}]]"#)
        );
        // A bare scalar is not a profile; a list key and NaN have no form.
        assert_eq!(to_yaml(&PyValue::Int(3)), None);
        assert_eq!(to_yaml(&dict(&[("x", PyValue::Real(f64::NAN))])), None);
        let list_key = PyValue::Dict(vec![(PyValue::Int(1), PyValue::List(vec![]))]);
        assert_eq!(to_yaml(&list_key), None);
        let mut deep = PyValue::Int(1);
        for _ in 0..40 {
            deep = PyValue::List(vec![deep]);
        }
        assert_eq!(to_yaml(&dict(&[("d", deep)])), None);
    }

    #[test]
    fn a_non_ascii_name_is_escaped() {
        let d = dict(&[("name", u("Ünï \"q\" \u{1F600}"))]);
        assert_eq!(
            to_yaml(&d).as_deref(),
            Some(r#"[["name", "\u00DCn\u00EF \"q\" \uD83D\uDE00"]]"#)
        );
    }

    #[test]
    fn each_type_round_trips_and_a_type_mismatch_is_unknown() {
        let mut reg = world();
        let data = dict(&[("filter", PyValue::Tuple(vec![u("f"), PyValue::Int(0)]))]);
        for ty in [TYPE_OVERVIEW, TYPE_ITEMS, TYPE_BROADCAST] {
            let (h, sq) = store(&mut reg, ALICE, ty, &data).expect("stored");
            assert!(!h.contains("//"));
            assert_eq!(fetch(&reg, &h, sq, ty), to_yaml(&data));
            let other = if ty == TYPE_ITEMS {
                TYPE_BROADCAST
            } else {
                TYPE_ITEMS
            };
            assert_eq!(fetch(&reg, &h, sq, other), None);
        }
        // The same YAML under three types takes three sqIDs on one hash.
        assert_eq!(reg.deltas().of(KIND).len(), 3);
    }

    #[test]
    fn an_unknown_key_gives_none() {
        let reg = world();
        assert_eq!(fetch(&reg, "0000000000000000", 1, TYPE_OVERVIEW), None);
    }

    #[test]
    fn the_same_data_by_anyone_answers_the_same_link() {
        let mut reg = world();
        let d = dict(&[("a", PyValue::Int(1))]);
        let first = store(&mut reg, ALICE, TYPE_OVERVIEW, &d).expect("stored");
        let again = store(&mut reg, BOB, TYPE_OVERVIEW, &d).expect("stored");
        assert_eq!(first, again);
        assert_eq!(reg.deltas().of(KIND).len(), 1);
        // Entries in another wire order are the same data.
        let swapped = PyValue::Dict(vec![(PyValue::Int(2), u("b")), (PyValue::Int(1), u("a"))]);
        let ordered = PyValue::Dict(vec![(PyValue::Int(1), u("a")), (PyValue::Int(2), u("b"))]);
        assert_eq!(
            store(&mut reg, ALICE, TYPE_ITEMS, &swapped),
            store(&mut reg, BOB, TYPE_ITEMS, &ordered)
        );
    }

    #[test]
    fn a_hash_collision_takes_the_next_sq() {
        let mut reg = world();
        let d = dict(&[("a", PyValue::Int(1))]);
        let yaml = to_yaml(&d).expect("yaml");
        let hash = hash_of(&yaml);
        // Plant a different row on this hash's first slot.
        reg.deltas_mut().put(
            KIND,
            key(&hash, 1),
            vec!["0".to_owned(), "1".to_owned(), "[]".to_owned()],
        );
        let (h, sq) = store(&mut reg, ALICE, TYPE_OVERVIEW, &d).expect("stored");
        assert_eq!((h.as_str(), sq), (hash.as_str(), 2));
        assert_eq!(fetch(&reg, &h, 2, TYPE_OVERVIEW), Some(yaml));
    }

    #[test]
    fn limits_hold() {
        let mut reg = world();
        let big = dict(&[("n", u(&"x".repeat(MAX_YAML_BYTES)))]);
        assert_eq!(store(&mut reg, ALICE, TYPE_OVERVIEW, &big), None);
        for i in 0..MAX_PER_OWNER as i64 {
            assert!(
                store(
                    &mut reg,
                    ALICE,
                    TYPE_OVERVIEW,
                    &dict(&[("i", PyValue::Int(i))])
                )
                .is_some()
            );
        }
        let more = dict(&[("i", PyValue::Int(-1))]);
        assert_eq!(store(&mut reg, ALICE, TYPE_OVERVIEW, &more), None);
        // Another pilot has their own allowance, and a repeat is not new.
        assert!(store(&mut reg, BOB, TYPE_OVERVIEW, &more).is_some());
        assert!(
            store(
                &mut reg,
                ALICE,
                TYPE_OVERVIEW,
                &dict(&[("i", PyValue::Int(0))])
            )
            .is_some()
        );
    }

    #[test]
    fn a_link_survives_a_reload() {
        let mut reg = world();
        let d = dict(&[("n", u("Ünï"))]);
        let (h, sq) = store(&mut reg, ALICE, TYPE_BROADCAST, &d).expect("stored");
        let dir = std::env::temp_dir().join(format!(
            "eve-sharelinks-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("a directory");
        let file = crate::store::FileStore::new(dir.join("world.txt"));
        crate::store::Store::save(&file, &reg.to_snapshot()).expect("write");
        let snap = crate::store::Store::load(&file)
            .expect("read")
            .expect("a world");
        let _ = std::fs::remove_dir_all(&dir);
        let mut fresh = world();
        assert_eq!(fetch(&fresh, &h, sq, TYPE_BROADCAST), None);
        fresh.apply_snapshot(snap);
        assert_eq!(fetch(&fresh, &h, sq, TYPE_BROADCAST), to_yaml(&d));
    }
}
