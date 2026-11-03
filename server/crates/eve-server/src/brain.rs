//! **The character brain** (GAP-BRAIN): the pilot's skills and implants, as
//! the effect lists the client's dogma applies to its own ship and character.
//!
//! The client holds neither the skills nor the implants as dogma items, so
//! without this its fitting window and Show Info work every stat out without
//! a single skill bonus. `clientDogmaLocation` takes the brain in two places
//! [V] (`cdl.py:1274-1289`, `bdl.py:286-352,1755-1808`):
//!
//! - at init, as `charInfo[1]` of `GetAllInfo`: `(version, charEffects,
//!   shipEffects, structureEffects)`. The client replaces the version with -1.
//! - later, as `OnServerBrainUpdated((version, grayMatter))`. An update at or
//!   below the current version is ignored; `grayMatter` is a whole marshal
//!   stream of `(charEffects, shipEffects, structureEffects)`.
//!
//! Each effect becomes a `BrainLiteralAttribute` with `forceUnnurfed`, so
//! **a brain effect is never stacking-penalised** [V]; skills and implants are
//! unnerfed categories in the engine too (`dogma::UNNERFED_CATEGORIES`).
//!
//! **The values are the engine's.** [`crate::dogma::brain_mods`] is a view of
//! the evaluator's own modifier collection, so what the client adds up is
//! what `dogma::sheet` adds up. This module only gives them the wire shape.
//!
//! ## Rollout
//!
//! A malformed brain in `GetAllInfo` could break the client's dogma init at
//! login, whereas one in a push is only logged. The init brain is behind
//! [`BRAIN_IN_GET_ALL_INFO`], now **on**: without it the client boards a
//! docked ship with an empty brain, fits unskilled, and a module the skilled
//! server keeps online (a fifth launcher over the unskilled CPU) goes offline
//! in the client's own dogma, and only a brain at login can prevent that (a
//! later `OnServerBrainUpdated` does not bring it back). Falsifier for the
//! switch: a traceback from `GetLiteralKey` or `ApplyBrainEffects`, or a login
//! that stalls in dogma init; set the const to `false` to roll back.
//!
//! **[A]** the `BrainEffect` is sent as a new-style instance (`((cls,),
//! state)`, no `__init__`) carrying the whole `__dict__` the constructor
//! would have set; the spec's alternative is the call form with
//! `KeyVal(invItem=KeyVal(typeID=...))` as `fromAttrib`. Falsifier: the
//! client logs a traceback from `GetLiteralKey` or `ApplyBrainEffects`.
//! **[A]** `toItemID` is the ship for ship effects and the character for
//! character effects; the ship path ignores it [V].

use crate::dogma::{BrainMod, BrainTarget};
use crate::pyutil::new_style;
use crate::service::Registry;
use eve_marshal::PyValue;
use std::sync::atomic::{AtomicI64, Ordering};

/// Whether `GetAllInfo` carries the real brain. On (see the module docs); the
/// rollback is `false`.
pub const BRAIN_IN_GET_ALL_INFO: bool = true;

/// The client's class for one effect (`effect.py:51`); whitelisted.
const CLASS: &str = "eve.common.script.dogma.effect.BrainEffect";

/// The last version handed out. Process-local and monotonic across all
/// characters, which is monotonic for each of them, and the init brain's -1
/// is below every value: the first update is 1.
static VERSION: AtomicI64 = AtomicI64::new(0);

fn next_version() -> i64 {
    VERSION.fetch_add(1, Ordering::Relaxed) + 1
}

/// The client's `modifierType` and `extras` for a target
/// (`pyc:eve/common/script/dogma/effect.py:5-9`).
fn modifier_type(t: BrainTarget) -> (&'static [u8], Vec<PyValue>) {
    match t {
        BrainTarget::ShipItem | BrainTarget::CharItem => (b"M", vec![]),
        BrainTarget::ShipLocation => (b"L", vec![]),
        BrainTarget::ShipGroup(g) => (b"LG", vec![PyValue::Int(g)]),
        BrainTarget::ShipSkill(s) => (b"LRS", vec![PyValue::Int(s)]),
        BrainTarget::CharOwnerSkill(s) => (b"ORS", vec![PyValue::Int(s)]),
    }
}

/// Whether the effect goes in the character list (else the ship list).
fn on_character(t: BrainTarget) -> bool {
    matches!(t, BrainTarget::CharItem | BrainTarget::CharOwnerSkill(_))
}

/// One `BrainEffect` object. Pure.
pub fn effect_value(m: &BrainMod, to_item: i64) -> PyValue {
    let (kind, extras) = modifier_type(m.target);
    new_style(
        CLASS,
        vec![
            ("fromAttrib", PyValue::None),
            ("value", PyValue::Real(m.value)),
            ("toItemID", PyValue::Int(to_item)),
            ("modifierType", PyValue::Bytes(kind.to_vec())),
            ("toAttribID", PyValue::Int(m.attr)),
            ("operation", PyValue::Int(m.op)),
            ("extras", PyValue::Tuple(extras)),
            ("skills", PyValue::List(vec![PyValue::Int(m.source_type)])),
        ],
    )
}

/// `(charEffects, shipEffects, structureEffects)` of a modifier list. Pure.
pub fn lists(mods: &[BrainMod], char_id: i64, ship_id: i64) -> (PyValue, PyValue, PyValue) {
    let (mut chars, mut ships) = (Vec::new(), Vec::new());
    for m in mods {
        if on_character(m.target) {
            chars.push(effect_value(m, char_id));
        } else {
            ships.push(effect_value(m, ship_id));
        }
    }
    (
        PyValue::List(chars),
        PyValue::List(ships),
        PyValue::List(vec![]),
    )
}

/// Every modifier the character's skills and implants make.
pub fn mods_of(reg: &Registry, character_id: i64) -> Vec<BrainMod> {
    let skills: Vec<(i64, i64)> = reg
        .skills_for(Some(character_id))
        .map(|s| s.entries().map(|e| (e.type_id, e.level)).collect())
        .unwrap_or_default();
    let mut implants: Vec<i64> = reg
        .store()
        .contents_of(character_id)
        .filter(|i| i.flag_id == crate::dogma::FLAG_IMPLANT)
        .map(|i| i.type_id)
        .collect();
    implants.sort_unstable();
    let table = crate::typedata::installed_in(reg.content()).table();
    let boosters = crate::boosters::inputs_of(reg, character_id);
    crate::dogma::brain_mods(table, &skills, &implants, &boosters)
}

fn ship_of(reg: &Registry, character_id: i64) -> i64 {
    reg.character(character_id).map_or(0, |c| c.ship_id)
}

/// The 4-tuple `GetAllInfo` carries: `(version, charEffects, shipEffects,
/// structureEffects)`. The version is 0; the client replaces it with -1.
/// With [`BRAIN_IN_GET_ALL_INFO`] off the lists are empty.
pub fn initial(reg: &Registry, character_id: i64) -> PyValue {
    let (c, s, st) = if BRAIN_IN_GET_ALL_INFO {
        lists(
            &mods_of(reg, character_id),
            character_id,
            ship_of(reg, character_id),
        )
    } else {
        (
            PyValue::List(vec![]),
            PyValue::List(vec![]),
            PyValue::List(vec![]),
        )
    };
    PyValue::Tuple(vec![PyValue::Int(0), c, s, st])
}

/// The argument of `OnServerBrainUpdated`: `(version, grayMatter)`, the
/// version one higher than any handed out before.
pub fn update(reg: &Registry, character_id: i64) -> PyValue {
    let (c, s, st) = lists(
        &mods_of(reg, character_id),
        character_id,
        ship_of(reg, character_id),
    );
    let blob = crate::wire::dumps(&PyValue::Tuple(vec![c, s, st]));
    PyValue::Tuple(vec![PyValue::Int(next_version()), PyValue::Bytes(blob)])
}

/// `OnServerBrainUpdated((version, grayMatter))`, to one character.
#[derive(Debug, Clone, PartialEq)]
pub struct BrainUpdated {
    pub character_id: i64,
    pub brain: PyValue,
}

impl crate::notify::Push for BrainUpdated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnServerBrainUpdated", vec![self.brain.clone()])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character_id])
    }
}

/// Tell the client its skills or implants changed what they do.
pub(crate) fn announce(reg: &mut Registry, character_id: i64) {
    let brain = update(reg, character_id);
    reg.push(BrainUpdated {
        character_id,
        brain,
    });
}

/// The same, from where there is no payload turn to ride (the training clock).
pub(crate) fn announce_later(reg: &mut Registry, character_id: i64) {
    let brain = update(reg, character_id);
    reg.schedule(
        std::time::Duration::ZERO,
        BrainUpdated {
            character_id,
            brain,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::Push;

    fn m(target: BrainTarget, attr: i64, value: f64) -> BrainMod {
        BrainMod {
            source_type: 980_000_011,
            target,
            op: 6,
            attr,
            value,
        }
    }

    fn state_of(v: &PyValue) -> &PyValue {
        let PyValue::ObjectEx { header, .. } = v else {
            panic!("an ObjectEx")
        };
        let PyValue::Tuple(h) = header.as_ref() else {
            panic!("a header")
        };
        &h[1]
    }

    #[test]
    fn an_effect_is_a_new_style_brain_effect_with_the_clients_fields() {
        let v = effect_value(&m(BrainTarget::ShipSkill(980_000_010), 64, 25.0), 7);
        let PyValue::ObjectEx { header, .. } = &v else {
            panic!()
        };
        let PyValue::Tuple(h) = header.as_ref() else {
            panic!()
        };
        assert_eq!(
            h[0],
            PyValue::Tuple(vec![PyValue::Global(CLASS.as_bytes().to_vec())])
        );
        let st = state_of(&v);
        let get = |k: &[u8]| crate::pyutil::dict_get(st, k).cloned().expect("field");
        assert_eq!(get(b"value"), PyValue::Real(25.0));
        assert_eq!(get(b"modifierType"), PyValue::Bytes(b"LRS".to_vec()));
        assert_eq!(get(b"toAttribID"), PyValue::Int(64));
        assert_eq!(get(b"operation"), PyValue::Int(6));
        assert_eq!(get(b"toItemID"), PyValue::Int(7));
        assert_eq!(get(b"fromAttrib"), PyValue::None);
        assert_eq!(
            get(b"extras"),
            PyValue::Tuple(vec![PyValue::Int(980_000_010)])
        );
        assert_eq!(
            get(b"skills"),
            PyValue::List(vec![PyValue::Int(980_000_011)])
        );
    }

    #[test]
    fn every_target_has_the_clients_modifier_type_and_list() {
        let cases = [
            (BrainTarget::ShipItem, &b"M"[..], 0usize, false),
            (BrainTarget::CharItem, b"M", 0, true),
            (BrainTarget::ShipLocation, b"L", 0, false),
            (BrainTarget::ShipGroup(55), b"LG", 1, false),
            (BrainTarget::ShipSkill(9), b"LRS", 1, false),
            (BrainTarget::CharOwnerSkill(9), b"ORS", 1, true),
        ];
        for (t, kind, extras, on_char) in cases {
            let (k, e) = modifier_type(t);
            assert_eq!((k, e.len(), on_character(t)), (kind, extras, on_char));
        }
        let mods: Vec<BrainMod> = cases.iter().map(|c| m(c.0, 1, 1.0)).collect();
        let (c, s, st) = lists(&mods, 10, 20);
        let PyValue::List(c) = c else { panic!() };
        let PyValue::List(s) = s else { panic!() };
        assert_eq!((c.len(), s.len()), (2, 4));
        assert_eq!(st, PyValue::List(vec![]));
    }

    #[test]
    fn the_initial_brain_carries_the_same_lists_as_an_update() {
        let reg = Registry::new(crate::model::SandboxConfig::default());
        let who = reg.config().characters[0].character_id;
        let PyValue::Tuple(t) = initial(&reg, who) else {
            panic!()
        };
        assert_eq!(t.len(), 4);
        assert_eq!(t[0], PyValue::Int(0));
        let PyValue::Tuple(u) = update(&reg, who) else {
            panic!()
        };
        let PyValue::Bytes(blob) = &u[1] else {
            panic!("grayMatter is bytes")
        };
        let PyValue::Tuple(three) = crate::wire::loads(blob).expect("a marshal stream") else {
            panic!("a 3-tuple")
        };
        assert_eq!(&t[1..], &three[..]);
    }

    #[test]
    fn a_skilled_characters_initial_brain_is_not_empty() {
        // An invented table: one skill whose level scales the ship's CPU.
        const TABLE: &str = "\
attribute\t48\t0.0\tcpuOutput
attribute\t280\t0.0\tskillLevel
attribute\t424\t0.0\tcpuOutputBonus2
attrmeta\t48\t1\t1\t0\t0\t4
effect\t9100\t0\t0\t0\t0\t0\t0\t0\tskillEffect\t
effect\t9102\t0\t0\t0\t0\t0\t0\t0\tcpuSkill\t
modifier\t9100\titemID\tItemModifier\t280\t276\t2\t\t
modifier\t9102\titemID\tItemModifier\t424\t280\t0\t\t
modifier\t9102\tshipID\tItemModifier\t48\t424\t6\t\t
type\t980000011\t255\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9100,9102\t424=5.0\tTest CPU Skill
";
        let mut content = crate::content::Content::fixture();
        content.insert(
            crate::typedata::TABLE.name,
            crate::typedata::Installed::build(
                crate::typedata::Table::parse(TABLE).expect("parses"),
            ),
        );
        let mut reg = Registry::new(crate::model::SandboxConfig::default())
            .with_content(std::sync::Arc::new(content));
        let who = reg.config().characters[0].character_id;
        *reg.skills_mut(who).expect("has skills") =
            crate::skill::Skills::restore(vec![crate::skill::SkillEntry {
                type_id: 980_000_011,
                level: 4,
                points: 1,
                rank: 1,
            }]);
        let PyValue::Tuple(t) = initial(&reg, who) else {
            panic!()
        };
        let PyValue::List(ships) = &t[2] else {
            panic!("a list")
        };
        assert_eq!(ships.len(), 1, "the CPU skill's one ship effect");
        let get = |k: &[u8]| crate::pyutil::dict_get(state_of(&ships[0]), k).cloned();
        assert_eq!(get(b"value"), Some(PyValue::Real(20.0)));
        assert_eq!(
            get(b"skills"),
            Some(PyValue::List(vec![PyValue::Int(980_000_011)]))
        );
    }

    #[test]
    fn the_update_is_a_version_and_a_marshal_blob_of_three_lists() {
        let reg = Registry::new(crate::model::SandboxConfig::default());
        let who = reg.config().characters[0].character_id;
        let PyValue::Tuple(t) = update(&reg, who) else {
            panic!()
        };
        let PyValue::Int(v) = t[0] else { panic!() };
        assert!(v >= 1);
        let PyValue::Bytes(blob) = &t[1] else {
            panic!("grayMatter is bytes")
        };
        let PyValue::Tuple(three) = crate::wire::loads(blob).expect("a marshal stream") else {
            panic!("a 3-tuple")
        };
        assert_eq!(three.len(), 3);
        assert!(three.iter().all(|l| matches!(l, PyValue::List(_))));
    }

    #[test]
    fn a_blob_with_effects_round_trips() {
        let mods = vec![
            m(BrainTarget::ShipItem, 48, 20.0),
            m(BrainTarget::CharOwnerSkill(9), 64, 6.0),
        ];
        let (c, s, st) = lists(&mods, 10, 20);
        let want = PyValue::Tuple(vec![c, s, st]);
        let got = crate::wire::loads(&crate::wire::dumps(&want)).expect("loads");
        assert_eq!(got, want);
    }

    #[test]
    fn versions_increase() {
        let reg = Registry::new(crate::model::SandboxConfig::default());
        let who = reg.config().characters[0].character_id;
        let ver = |v: PyValue| match v {
            PyValue::Tuple(t) => match t[0] {
                PyValue::Int(n) => n,
                _ => panic!(),
            },
            _ => panic!(),
        };
        let a = ver(update(&reg, who));
        let b = ver(update(&reg, who));
        assert!(b > a);
    }

    #[test]
    fn the_push_is_one_tuple_to_one_character() {
        let p = BrainUpdated {
            character_id: 5,
            brain: PyValue::Tuple(vec![PyValue::Int(1), PyValue::Bytes(vec![])]),
        };
        let (name, args) = p.to_push();
        assert_eq!(name, "OnServerBrainUpdated");
        assert_eq!(args.len(), 1);
        assert_eq!(p.audience(), Some(vec![5]));
    }

    /// **Size and build time on the real table** (not part of the gate: it
    /// needs extracted client data, which the repository never holds).
    ///
    /// `EVE_BRAIN_TYPEDATA=/path/typedata.tsv cargo test -p eve-server --lib
    /// brain::tests::measure -- --ignored --nocapture`: every skill of the
    /// table at level 5, no implants.
    #[test]
    #[ignore = "needs the extracted typedata.tsv (EVE_BRAIN_TYPEDATA)"]
    fn measure_the_brain_of_a_pilot_with_every_skill() {
        let path = std::env::var("EVE_BRAIN_TYPEDATA").expect("EVE_BRAIN_TYPEDATA");
        let table = crate::typedata::Table::load(std::path::Path::new(&path)).expect("loads");
        let text = std::fs::read_to_string(&path).expect("reads");
        let mut skills: Vec<(i64, i64)> = text
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                (f.first() == Some(&"type") && f.get(3) == Some(&"16"))
                    .then(|| f[1].parse::<i64>().ok())
                    .flatten()
            })
            .map(|t| (t, 5))
            .collect();
        skills.sort_unstable();
        let t0 = std::time::Instant::now();
        let mods = crate::dogma::brain_mods(&table, &skills, &[], &[]);
        let built = t0.elapsed();
        let t1 = std::time::Instant::now();
        let (c, s, st) = lists(&mods, 90_000_001, 100_000_001);
        let blob = crate::wire::dumps(&PyValue::Tuple(vec![c, s, st]));
        let wired = t1.elapsed();
        let t2 = std::time::Instant::now();
        let again = crate::dogma::brain_mods(&table, &skills, &[], &[]);
        let second = t2.elapsed();
        eprintln!(
            "MEASURE skills={} mods={} marshal_bytes={} brain_mods={:?} (second {:?}) marshal={:?}",
            skills.len(),
            mods.len(),
            blob.len(),
            built,
            second,
            wired
        );
        assert_eq!(mods, again);
    }
}
