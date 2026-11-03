//! **Character settings** (SMALL-SOCIAL; lane note `docs/lanes/small-social.md`):
//! the server-side `settingKey -> text` map of one character, which the
//! client keeps for things that must follow the pilot to another machine:
//! the fleet auto-reject, saved fleet setups, mail settings, inventory
//! filters.
//!
//! The client (`characterSettingsSvc.py`) [V]:
//!
//! - `charMgr.GetCharacterSettings()` once per session (`_GetSettings:30-33`)
//!   -> a mutable dict `{settingKey: value}`; it subscripts it, tests `in`,
//!   assigns into it and `del`s from it;
//! - `Save(key, value)` (`:44-51`): an empty or `None` value first calls
//!   `Delete(key)`, then **still** calls `charMgr.SaveCharacterSetting(key,
//!   value)` (so an empty value reaching us means delete), refuses a value
//!   over 102400 itself, and updates its own copy; the answer is dropped;
//! - `Delete(key)` (`:53-57`): `charMgr.DeleteCharacterSetting(key)` only for
//!   a key it already holds; answer dropped.
//!
//! Stored as delta kind [`KIND`]: `char key -> value`. The limits below are
//! ours **[A]**, bounds on what one character can make the world file hold;
//! the client's own 102400 cap (a [V] number) is the per-value one.

use crate::service::Registry;
use eve_marshal::PyValue;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "csetting";

/// The client's own refusal size, `characterSettingsSvc.Save:47` [V].
pub const MAX_VALUE_BYTES: usize = 102_400;
/// The most settings one character keeps. **[A]** tuning; falsified by a
/// retail character holding more.
pub const MAX_SETTINGS: usize = 300;
/// The longest key kept, in characters. **[A]** tuning.
pub const MAX_KEY_CHARS: usize = 120;

fn key(ch: i64, setting: &str) -> Vec<String> {
    vec![ch.to_string(), setting.to_owned()]
}

/// Every setting of `ch`, by key.
pub fn all(reg: &Registry, ch: i64) -> Vec<(String, String)> {
    let who = ch.to_string();
    reg.deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| match k.as_slice() {
            [c, name] if *c == who => Some((name.clone(), v.first().cloned().unwrap_or_default())),
            _ => None,
        })
        .collect()
}

/// One setting.
pub fn get(reg: &Registry, ch: i64, setting: &str) -> Option<String> {
    reg.deltas()
        .of(KIND)
        .get(&key(ch, setting))
        .and_then(|v| v.first().cloned())
}

/// `GetCharacterSettings()`: a real dict, `{key: value}`.
pub fn settings_value(reg: &Registry, ch: i64) -> PyValue {
    PyValue::Dict(
        all(reg, ch)
            .into_iter()
            .map(|(k, v)| (PyValue::Unicode(v), PyValue::Unicode(k)))
            .collect(),
    )
}

/// `SaveCharacterSetting(key, value)`: an empty value deletes. `false` when
/// the setting was refused (a key that is empty or too long, a value over
/// [`MAX_VALUE_BYTES`], or the [`MAX_SETTINGS`]th new key).
pub fn save(reg: &mut Registry, ch: i64, setting: &str, value: &str) -> bool {
    if value.is_empty() {
        delete(reg, ch, setting);
        return true;
    }
    if setting.is_empty()
        || setting.chars().count() > MAX_KEY_CHARS
        || value.len() > MAX_VALUE_BYTES
    {
        return false;
    }
    let k = key(ch, setting);
    if !reg.deltas().of(KIND).contains_key(&k) && all(reg, ch).len() >= MAX_SETTINGS {
        return false;
    }
    reg.deltas_mut().put(KIND, k, vec![value.to_owned()]);
    true
}

/// `DeleteCharacterSetting(key)`: whether there was one.
pub fn delete(reg: &mut Registry, ch: i64, setting: &str) -> bool {
    if get(reg, ch, setting).is_none() {
        return false;
    }
    reg.deltas_mut().del(KIND, &key(ch, setting)).is_some()
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
    fn save_get_delete_and_the_empty_value() {
        let mut reg = world();
        assert!(save(&mut reg, ALICE, "autoReject", "1"));
        assert!(save(&mut reg, ALICE, "fleetSetups", "a: 1\n\tb: %2"));
        assert_eq!(get(&reg, ALICE, "autoReject").as_deref(), Some("1"));
        assert_eq!(get(&reg, BOB, "autoReject"), None, "per character");
        assert_eq!(all(&reg, ALICE).len(), 2);
        // The client saves '' after deleting; that is a delete here too.
        assert!(save(&mut reg, ALICE, "autoReject", ""));
        assert_eq!(get(&reg, ALICE, "autoReject"), None);
        assert!(delete(&mut reg, ALICE, "fleetSetups"));
        assert!(!delete(&mut reg, ALICE, "fleetSetups"));
        assert!(all(&reg, ALICE).is_empty());
    }

    fn call(reg: &mut Registry, who: i64, method: &str, args: &[PyValue]) -> PyValue {
        reg.dispatch(&crate::service::Request {
            service: "charMgr",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        })
        .expect("answered")
    }

    fn u(s: &str) -> PyValue {
        PyValue::Unicode(s.to_owned())
    }

    /// The wire: a save is served back by `GetCharacterSettings`, survives
    /// the world file (tabs, newlines and percent signs in a YAML value
    /// included) and a delete takes it out.
    #[test]
    fn a_setting_round_trips_through_a_save_and_load() {
        let mut reg = world();
        let yaml = "- name: a\n\tb: 100%\n";
        assert_eq!(
            call(
                &mut reg,
                ALICE,
                "SaveCharacterSetting",
                &[u("fleetSetups"), u(yaml)]
            ),
            PyValue::None
        );
        call(
            &mut reg,
            ALICE,
            "SaveCharacterSetting",
            &[u("autoRejectDuel"), u("1")],
        );
        call(
            &mut reg,
            ALICE,
            "SaveCharacterSetting",
            &[u("gone"), u("x")],
        );
        call(&mut reg, ALICE, "DeleteCharacterSetting", &[u("gone")]);

        let dir = std::env::temp_dir().join(format!(
            "eve-csettings-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("a directory");
        let store = crate::store::FileStore::new(dir.join("world.txt"));
        crate::store::Store::save(&store, &reg.to_snapshot()).expect("write");
        let snap = crate::store::Store::load(&store)
            .expect("read")
            .expect("a world");
        let _ = std::fs::remove_dir_all(&dir);

        let mut fresh = world();
        assert_eq!(
            call(&mut fresh, ALICE, "GetCharacterSettings", &[]),
            PyValue::Dict(vec![])
        );
        fresh.apply_snapshot(snap);
        let PyValue::Dict(d) = call(&mut fresh, ALICE, "GetCharacterSettings", &[]) else {
            panic!("a dict")
        };
        assert_eq!(d.len(), 2);
        assert!(d.contains(&(u(yaml), u("fleetSetups"))));
        assert!(d.contains(&(u("1"), u("autoRejectDuel"))));
        // Another pilot sees none of it.
        assert_eq!(
            call(&mut fresh, BOB, "GetCharacterSettings", &[]),
            PyValue::Dict(vec![])
        );
    }

    #[test]
    fn limits_hold() {
        let mut reg = world();
        assert!(!save(&mut reg, ALICE, "", "x"));
        assert!(!save(
            &mut reg,
            ALICE,
            "k",
            &"x".repeat(MAX_VALUE_BYTES + 1)
        ));
        assert!(save(&mut reg, ALICE, "k", &"x".repeat(MAX_VALUE_BYTES)));
        for i in 0..MAX_SETTINGS - 1 {
            assert!(save(&mut reg, ALICE, &format!("s{i}"), "v"));
        }
        assert!(!save(&mut reg, ALICE, "one more", "v"));
        assert!(save(&mut reg, ALICE, "k", "replaced"), "a replace is fine");
    }
}
