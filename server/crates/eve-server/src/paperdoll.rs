//! **The character creator's doll, kept and served back** (#205).
//!
//! A character's face is the paperdoll the character creator built. The
//! character sheet's avatar (`avatarHeaderPreview`) asks
//! `paperDollServer.GetPaperDollData(charID)` for it, and the client's
//! **scene portrait** path asks `GetPaperDollDataBatch` for the DNA and
//! `GetPaperDollPortraitDataFor` for the camera and light **[V decompiled
//! `evephotosvc/scenePortraitQueue.pyc`, `ccSvc.GetPortraitData`]**. That
//! path runs only for a session holding `ROLE_PROGRAMMER` (a dev listener's,
//! `docs/reference/session-roles.md`); a player listener's sessions take the
//! 2D portrait from the image server instead (`crate::images`).
//!
//! This module keeps what the creator sends: `CreateCharacterWithDoll`'s
//! `charInfo` and `portraitInfo`, each stored as one marshal stream in delta
//! kind [`KIND`].
//!
//! # The shape check
//!
//! Both values come from a client and are served to every other client that
//! looks at the character, so neither is relayed as it came. Each is read
//! into the doll's known shape and **rebuilt** from what was read; anything
//! else is refused and nothing is kept (the character is still made, with a
//! blank avatar, as before #205). The shape, as the 3707965 character
//! creator sends it **[V: two creator dolls read back out of a dev world,
//! 24NOV2026]**:
//!
//! - `charInfo`: a `utillib.KeyVal` of exactly `colors`, `modifiers` and
//!   `sculpts` (each a list of at most [`MAX_ROWS`] rows) and `appearance`
//!   (one row, or `None`). A row is a `__newobj__` instance of one
//!   `paperDollDefinitions` row class ([`ROWS`]), its arguments numbers or
//!   `None` and nothing else: a colour row has 5, a modifier row 3, a
//!   sculpting row 4 and the appearance row 1.
//! - `portraitInfo`: a `utillib.KeyVal` of exactly `lightID`,
//!   `lightColorID` and `backgroundID` (ints), `lightIntensity` and
//!   `cameraFieldOfView` (numbers), `cameraPosition` and `cameraPoi`
//!   (three numbers each) and `poseData`: a dict of at most [`MAX_POSE`]
//!   names (letters, digits and `_`, at most [`MAX_NAME`] bytes) to a number
//!   or three numbers (`HeadLookTarget`). Or `None`: no portrait setup.
//!
//! A number is an int or a finite float. No strings, no other classes, no
//! nesting past the shape above. A row kept before the check existed is
//! checked again when read, and served only if it passes.
//!
//! **[A]** the row arities and the number-or-`None` arguments hold for every
//! doll the creator makes, not only the two read back. Falsified by a
//! `[paperdoll] ... refused` line in the server log for a doll the creator
//! made (that character's avatar stays blank).

use crate::pyutil::{KEYVAL_CLASS, pydict};
use crate::service::Registry;
use eve_marshal::{ObjectForm, PyValue};

/// The delta kind (`delta_kinds::KINDS`, owner `paperdoll`):
/// `char -> charInfo portraitInfo`, each a hex marshal stream.
pub const KIND: &str = "pdoll";

/// The most rows one `charInfo` list may hold. The creator sends a dozen or
/// so (2 colours, 10 to 11 modifiers, 0 to 13 sculpts in the dolls read).
pub const MAX_ROWS: usize = 128;
/// The most `poseData` entries (22 in the dolls read).
pub const MAX_POSE: usize = 64;
/// The longest `poseData` name, in bytes.
pub const MAX_NAME: usize = 64;
/// The longest kept stream, in bytes (the dolls read are 0.6 to 2.2 KiB). A
/// stored row longer than this is not decoded.
pub const MAX_BYTES: usize = 32 * 1024;

/// The module every doll row class lives in.
const ROW_MODULE: &str = "eve.common.script.paperDoll.paperDollDefinitions";

/// One kind of doll row: the `charInfo` key it sits under, its class (in
/// [`ROW_MODULE`]) and how many arguments its constructor takes.
#[derive(Debug, Clone, Copy)]
pub struct RowSpec {
    pub key: &'static str,
    pub class: &'static str,
    pub arity: usize,
}

/// The four row kinds **[V: the creator's dolls, see the module doc; every
/// class is in the client's `globalsWhitelist`]**. The last one is
/// `appearance`, a single row rather than a list.
pub const ROWS: [RowSpec; 4] = [
    RowSpec {
        key: "colors",
        class: "ColorSelectionRow",
        arity: 5,
    },
    RowSpec {
        key: "modifiers",
        class: "ModifierRow",
        arity: 3,
    },
    RowSpec {
        key: "sculpts",
        class: "SculptingRow",
        arity: 4,
    },
    RowSpec {
        key: "appearance",
        class: "AppearanceRow",
        arity: 1,
    },
];

/// The `portraitInfo` keys and the kind of value each holds.
const PORTRAIT_KEYS: [(&str, Kind); 8] = [
    ("lightID", Kind::Int),
    ("lightColorID", Kind::Int),
    ("lightIntensity", Kind::Number),
    ("backgroundID", Kind::Int),
    ("cameraFieldOfView", Kind::Number),
    ("cameraPosition", Kind::Vec3),
    ("cameraPoi", Kind::Vec3),
    ("poseData", Kind::Pose),
];

#[derive(Debug, Clone, Copy)]
enum Kind {
    Int,
    Number,
    Vec3,
    Pose,
}

fn class_name(spec: &RowSpec) -> String {
    format!("{ROW_MODULE}.{}", spec.class)
}

fn key_text(k: &PyValue) -> Option<&str> {
    match k {
        PyValue::Bytes(b) => std::str::from_utf8(b).ok(),
        PyValue::Unicode(u) => Some(u),
        _ => None,
    }
}

/// The attributes of a `utillib.KeyVal`, each name at most once.
fn keyval_attrs(v: &PyValue) -> Result<Vec<(&str, &PyValue)>, String> {
    let PyValue::Instance { class, state } = v else {
        return Err("not a KeyVal".to_owned());
    };
    if class.as_slice() != KEYVAL_CLASS.as_bytes() {
        return Err(format!(
            "class {:?}, not a KeyVal",
            String::from_utf8_lossy(class)
        ));
    }
    dict_entries(state)
}

/// The `(name, value)` entries of a dict with text keys, each name at most
/// once.
fn dict_entries(v: &PyValue) -> Result<Vec<(&str, &PyValue)>, String> {
    let PyValue::Dict(pairs) = v else {
        return Err("not a dict".to_owned());
    };
    let mut out: Vec<(&str, &PyValue)> = Vec::with_capacity(pairs.len());
    for (val, key) in pairs {
        let name = key_text(key).ok_or("a key that is not text")?;
        if out.iter().any(|(n, _)| *n == name) {
            return Err(format!("{name:?} twice"));
        }
        out.push((name, val));
    }
    Ok(out)
}

fn number(v: &PyValue) -> Result<PyValue, String> {
    match v {
        PyValue::Int(n) => Ok(PyValue::Int(*n)),
        PyValue::Real(r) if r.is_finite() => Ok(PyValue::Real(*r)),
        _ => Err("a value that is not a number".to_owned()),
    }
}

fn int(v: &PyValue) -> Result<PyValue, String> {
    match v {
        PyValue::Int(n) => Ok(PyValue::Int(*n)),
        _ => Err("a value that is not an int".to_owned()),
    }
}

fn vec3(v: &PyValue) -> Result<PyValue, String> {
    match v {
        PyValue::Tuple(xs) if xs.len() == 3 => Ok(PyValue::Tuple(
            xs.iter().map(number).collect::<Result<_, _>>()?,
        )),
        _ => Err("not three numbers".to_owned()),
    }
}

/// The arguments of one doll row of kind `spec`, each a number or `None`
/// (served as `0.0` when `zero` is set, see [`dna`]).
fn row_args(v: &PyValue, spec: &RowSpec, zero: bool) -> Result<Vec<PyValue>, String> {
    let bad = || format!("a {} entry that is not a {} row", spec.key, spec.class);
    let PyValue::ObjectEx {
        form: ObjectForm::New,
        header,
        list,
        dict,
    } = v
    else {
        return Err(bad());
    };
    if !list.is_empty() || !dict.is_empty() {
        return Err(bad());
    }
    let PyValue::Tuple(outer) = header.as_ref() else {
        return Err(bad());
    };
    let [PyValue::Tuple(items)] = outer.as_slice() else {
        return Err(bad());
    };
    let Some((PyValue::Global(class), args)) = items.split_first() else {
        return Err(bad());
    };
    if class.as_slice() != class_name(spec).as_bytes() || args.len() != spec.arity {
        return Err(bad());
    }
    args.iter()
        .map(|a| match a {
            PyValue::None if zero => Ok(PyValue::Real(0.0)),
            PyValue::None => Ok(PyValue::None),
            other => number(other),
        })
        .collect()
}

/// One doll row of kind `spec`, built from its arguments.
fn row_value(spec: &RowSpec, args: Vec<PyValue>) -> PyValue {
    let mut items = Vec::with_capacity(args.len() + 1);
    items.push(PyValue::Global(class_name(spec).into_bytes()));
    items.extend(args);
    PyValue::ObjectEx {
        form: ObjectForm::New,
        header: Box::new(PyValue::Tuple(vec![PyValue::Tuple(items)])),
        list: Vec::new(),
        dict: Vec::new(),
    }
}

fn rebuild_row(v: &PyValue, spec: &RowSpec, zero: bool) -> Result<PyValue, String> {
    Ok(row_value(spec, row_args(v, spec, zero)?))
}

/// `charInfo`, checked to the doll's shape and rebuilt from what was read
/// (module doc). With `zero`, every missing sculpt weight is `0.0` ([`dna`]).
fn rebuild_char_info(v: &PyValue, zero: bool) -> Result<PyValue, String> {
    let attrs = keyval_attrs(v)?;
    if let Some((name, _)) = attrs
        .iter()
        .find(|(n, _)| !ROWS.iter().any(|s| s.key == *n))
    {
        return Err(format!("charInfo key {name:?}"));
    }
    let get = |key: &str| {
        attrs
            .iter()
            .find(|(n, _)| *n == key)
            .map(|(_, v)| *v)
            .ok_or(format!("charInfo without {key:?}"))
    };
    let mut out = Vec::with_capacity(ROWS.len());
    for spec in &ROWS[..3] {
        let PyValue::List(rows) = get(spec.key)? else {
            return Err(format!("charInfo {:?} is not a list", spec.key));
        };
        if rows.len() > MAX_ROWS {
            return Err(format!("charInfo {:?}: {} rows", spec.key, rows.len()));
        }
        let rows = rows
            .iter()
            .map(|r| rebuild_row(r, spec, zero && spec.key == "sculpts"))
            .collect::<Result<_, _>>()?;
        out.push((spec.key, PyValue::List(rows)));
    }
    let spec = &ROWS[3];
    let appearance = match get(spec.key)? {
        PyValue::None => PyValue::None,
        r => rebuild_row(r, spec, false)?,
    };
    out.push((spec.key, appearance));
    Ok(crate::pyutil::keyval(out))
}

/// `charInfo` as it may be kept: the doll's shape, rebuilt (module doc).
pub fn check_char_info(v: &PyValue) -> Result<PyValue, String> {
    rebuild_char_info(v, false)
}

fn pose_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn pose(v: &PyValue) -> Result<PyValue, String> {
    let entries = dict_entries(v)?;
    if entries.len() > MAX_POSE {
        return Err(format!("poseData of {} entries", entries.len()));
    }
    let mut out = Vec::with_capacity(entries.len());
    for (name, val) in entries {
        if !pose_name(name) {
            return Err("a poseData name that is not a plain name".to_owned());
        }
        let val = match val {
            PyValue::Tuple(_) => vec3(val)?,
            other => number(other)?,
        };
        out.push((name, val));
    }
    Ok(pydict(out))
}

/// `portraitInfo` as it may be kept: `None`, or the portrait setup's shape,
/// rebuilt (module doc).
pub fn check_portrait_info(v: &PyValue) -> Result<PyValue, String> {
    if *v == PyValue::None {
        return Ok(PyValue::None);
    }
    let attrs = keyval_attrs(v)?;
    if let Some((name, _)) = attrs
        .iter()
        .find(|(n, _)| !PORTRAIT_KEYS.iter().any(|(k, _)| k == n))
    {
        return Err(format!("portraitInfo key {name:?}"));
    }
    let mut out = Vec::with_capacity(PORTRAIT_KEYS.len());
    for (key, kind) in PORTRAIT_KEYS {
        let val = attrs
            .iter()
            .find(|(n, _)| *n == key)
            .map(|(_, v)| *v)
            .ok_or(format!("portraitInfo without {key:?}"))?;
        let val = match kind {
            Kind::Int => int(val),
            Kind::Number => number(val),
            Kind::Vec3 => vec3(val),
            Kind::Pose => pose(val),
        }
        .map_err(|e| format!("portraitInfo {key:?}: {e}"))?;
        out.push((key, val));
    }
    Ok(crate::pyutil::keyval(out))
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Keep the creator's doll of `ch`, once both values pass the shape check
/// (module doc); the rebuilt values are what is kept. `Err` says why nothing
/// was kept.
pub fn store(
    reg: &mut Registry,
    ch: i64,
    char_info: &PyValue,
    portrait_info: &PyValue,
) -> Result<(), String> {
    let ci = crate::wire::dumps(&check_char_info(char_info)?);
    let pi = crate::wire::dumps(&check_portrait_info(portrait_info)?);
    if ci.len() > MAX_BYTES || pi.len() > MAX_BYTES {
        return Err(format!("{} and {} bytes", ci.len(), pi.len()));
    }
    reg.deltas_mut()
        .put(KIND, vec![ch.to_string()], vec![to_hex(&ci), to_hex(&pi)]);
    Ok(())
}

fn field(reg: &Registry, ch: i64, i: usize) -> Option<PyValue> {
    let row = reg.deltas().of(KIND).get(&[ch.to_string()][..])?;
    let hex = row.get(i)?;
    if hex.len() > 2 * MAX_BYTES {
        return None;
    }
    crate::wire::loads(&from_hex(hex)?).ok()
}

/// `ch`'s DNA (`charInfo`), if a doll was kept and still passes the shape
/// check, with every missing sculpt weight served as `0.0`.
///
/// **Why the zeros:** the creator sends `None` for a sculpt axis it does not
/// use, but the doll loader the character sheet runs
/// (`character.ApplyDBRowToDoll` -> `ConvertWeightKeyValue`) takes `abs()` of
/// every weight and raised `TypeError: bad operand type for abs():
/// 'NoneType'` on a dev character's doll **[V client traceback,
/// 19NOV2026]**. **[A]** a live server stores 0 there: a zero weight is no
/// morph on that axis, the same as none. Falsified by a doll whose avatar
/// differs from the creator's own snapshot on a sculpted axis. Only the
/// `sculpts` rows are touched.
pub fn dna(reg: &Registry, ch: i64) -> Option<PyValue> {
    rebuild_char_info(&field(reg, ch, 0)?, true).ok()
}

/// `ch`'s portrait setup (`portraitInfo`), if one was kept and still passes
/// the shape check.
pub fn portrait_info(reg: &Registry, ch: i64) -> Option<PyValue> {
    match check_portrait_info(&field(reg, ch, 1)?).ok()? {
        PyValue::None => None,
        v => Some(v),
    }
}

/// `GetPaperDollDataBatch(charIDs)`: `{charID: dna}` for every id with a
/// kept doll. A real dict always (`_FetchBatchData` calls `iteritems`).
/// `PyValue::Dict` stores `(value, key)` (`pyutil::pydict`).
pub fn batch_value(reg: &Registry, ids: &[i64]) -> PyValue {
    PyValue::Dict(
        ids.iter()
            .filter_map(|&id| dna(reg, id).map(|d| (d, PyValue::Int(id))))
            .collect(),
    )
}

/// `GetPaperDollPortraitDataFor(charID)`: `(data, _)` where `data[0]` is the
/// portrait setup (`ccSvc.GetPortraitData` returns `data[0]` when
/// `len(data)`, else `None`).
pub fn portrait_value(reg: &Registry, ch: i64) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::List(portrait_info(reg, ch).into_iter().collect()),
        PyValue::None,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pyutil::{bstr, keyval};

    fn row(i: usize, args: Vec<PyValue>) -> PyValue {
        row_value(&ROWS[i], args)
    }

    fn color(a: i64) -> PyValue {
        let n = PyValue::Int;
        row(
            0,
            vec![n(a), n(2), n(3), PyValue::Real(0.0), PyValue::Real(0.0)],
        )
    }

    fn modifier(a: i64) -> PyValue {
        row(1, vec![PyValue::Int(a), PyValue::Int(9), PyValue::None])
    }

    fn sculpt(weights: [Option<f64>; 3]) -> PyValue {
        let mut args = vec![PyValue::Int(4)];
        args.extend(weights.map(|w| w.map_or(PyValue::None, PyValue::Real)));
        row(2, args)
    }

    /// An invented doll of the creator's shape (module doc).
    fn char_info_with(sculpts: Vec<PyValue>) -> PyValue {
        keyval(vec![
            ("colors", PyValue::List(vec![color(1), color(2)])),
            ("modifiers", PyValue::List(vec![modifier(5), modifier(6)])),
            ("sculpts", PyValue::List(sculpts)),
            ("appearance", row(3, vec![PyValue::Real(0.5)])),
        ])
    }

    fn char_info() -> PyValue {
        char_info_with(vec![sculpt([Some(0.25), None, Some(-0.5)])])
    }

    fn v3(a: f64, b: f64, c: f64) -> PyValue {
        PyValue::Tuple(vec![PyValue::Real(a), PyValue::Real(b), PyValue::Real(c)])
    }

    fn pose_with(extra: Vec<(&str, PyValue)>) -> PyValue {
        let mut p = vec![
            ("SmileLeft", PyValue::Real(0.25)),
            ("HeadLookTarget", v3(0.0, 1.0, 0.5)),
        ];
        p.extend(extra);
        pydict(p)
    }

    fn portrait_with(pose: PyValue) -> PyValue {
        keyval(vec![
            ("lightID", PyValue::Int(11)),
            ("lightColorID", PyValue::Int(12)),
            ("lightIntensity", PyValue::Real(0.5)),
            ("backgroundID", PyValue::Int(13)),
            ("cameraFieldOfView", PyValue::Real(0.3)),
            ("cameraPosition", v3(0.0, 1.5, 1.25)),
            ("cameraPoi", v3(0.0, 1.5, 0.125)),
            ("poseData", pose),
        ])
    }

    fn portrait() -> PyValue {
        portrait_with(pose_with(vec![]))
    }

    fn evil() -> PyValue {
        PyValue::Instance {
            class: b"evil.Payload".to_vec(),
            state: Box::new(pydict(vec![])),
        }
    }

    /// The KeyVal `v` with attribute `name` set to `bad`.
    fn with_attr(v: &PyValue, name: &str, bad: PyValue) -> PyValue {
        let PyValue::Instance { class, state } = v else {
            panic!("not a KeyVal")
        };
        let PyValue::Dict(mut pairs) = (**state).clone() else {
            panic!("state not a dict")
        };
        pairs.retain(|(_, k)| key_text(k) != Some(name));
        pairs.push((bad, bstr(name)));
        PyValue::Instance {
            class: class.clone(),
            state: Box::new(PyValue::Dict(pairs)),
        }
    }

    /// The row classes the shape check builds are ones the client will read:
    /// an unlisted one aborts the whole read (`pyutil::instance`).
    #[test]
    fn every_row_class_is_whitelisted() {
        for spec in &ROWS {
            assert!(eve_rowset::is_whitelisted(class_name(spec).as_bytes()));
        }
        assert!(eve_rowset::is_whitelisted(KEYVAL_CLASS.as_bytes()));
    }

    /// A well-formed doll is kept and comes back through all three answers;
    /// a character without one gets the empty shapes.
    #[test]
    fn a_well_formed_doll_is_kept_and_served() {
        let mut reg = crate::testkit::registry_with(|_| {});
        let (ci, pi) = (char_info(), portrait());
        assert_eq!(check_char_info(&ci), Ok(ci.clone()));
        assert_eq!(check_portrait_info(&pi), Ok(pi.clone()));
        store(&mut reg, 91_000_001, &ci, &pi).expect("kept");
        let served = char_info_with(vec![sculpt([Some(0.25), Some(0.0), Some(-0.5)])]);
        assert_eq!(dna(&reg, 91_000_001), Some(served.clone()));
        assert_eq!(portrait_info(&reg, 91_000_001), Some(pi.clone()));
        assert_eq!(
            batch_value(&reg, &[91_000_001, 91_000_002]),
            PyValue::Dict(vec![(served, PyValue::Int(91_000_001))])
        );
        assert_eq!(
            portrait_value(&reg, 91_000_001),
            PyValue::Tuple(vec![PyValue::List(vec![pi]), PyValue::None])
        );
        assert_eq!(
            portrait_value(&reg, 91_000_002),
            PyValue::Tuple(vec![PyValue::List(vec![]), PyValue::None])
        );
        assert_eq!(dna(&reg, 91_000_002), None);
    }

    /// Only the sculpts' missing weights are served as `0.0`: a modifier's
    /// `None` stays, and the kept stream keeps the creator's `None`.
    #[test]
    fn missing_sculpt_weights_are_served_as_zero() {
        let mut reg = crate::testkit::registry_with(|_| {});
        let ci = char_info_with(vec![sculpt([None, Some(0.5), None])]);
        store(&mut reg, 91_000_001, &ci, &PyValue::None).expect("kept");
        let want = char_info_with(vec![sculpt([Some(0.0), Some(0.5), Some(0.0)])]);
        assert_eq!(dna(&reg, 91_000_001), Some(want));
        assert_eq!(field(&reg, 91_000_001, 0), Some(ci));
        assert_eq!(portrait_info(&reg, 91_000_001), None);
    }

    /// Text keys may come as `unicode`; the rebuilt doll is the same.
    #[test]
    fn unicode_keys_are_read_as_names() {
        let PyValue::Instance { class, state } = char_info() else {
            unreachable!()
        };
        let PyValue::Dict(pairs) = *state else {
            unreachable!()
        };
        let pairs = pairs
            .into_iter()
            .map(|(v, k)| (v, PyValue::Unicode(key_text(&k).unwrap().to_owned())))
            .collect();
        let uni = PyValue::Instance {
            class,
            state: Box::new(PyValue::Dict(pairs)),
        };
        assert_eq!(check_char_info(&uni), Ok(char_info()));
    }

    /// Every `charInfo` that is not the doll's shape is refused, and a
    /// refused doll keeps nothing.
    #[test]
    fn a_char_info_of_another_shape_is_refused() {
        let other_row = PyValue::ObjectEx {
            form: ObjectForm::New,
            header: Box::new(PyValue::Tuple(vec![PyValue::Tuple(vec![
                PyValue::Global(b"evil.Row".to_vec()),
                PyValue::Int(1),
                PyValue::Int(2),
                PyValue::None,
            ])])),
            list: vec![],
            dict: vec![],
        };
        let PyValue::ObjectEx { header, .. } = modifier(1) else {
            unreachable!()
        };
        let with_items = PyValue::ObjectEx {
            form: ObjectForm::New,
            header: header.clone(),
            list: vec![evil()],
            dict: vec![],
        };
        let call_form = PyValue::ObjectEx {
            form: ObjectForm::Call,
            header,
            list: vec![],
            dict: vec![],
        };
        let list = |r: PyValue| PyValue::List(vec![r]);
        let ci = char_info();
        let n = PyValue::Int;
        let bad = [
            (
                "a plain dict",
                pydict(vec![("colors", PyValue::List(vec![]))]),
            ),
            ("another class", evil()),
            ("an unknown key", with_attr(&ci, "extra", n(1))),
            (
                "a missing key",
                keyval(vec![
                    ("colors", PyValue::List(vec![])),
                    ("modifiers", PyValue::List(vec![])),
                    ("appearance", PyValue::None),
                ]),
            ),
            ("rows not a list", with_attr(&ci, "colors", n(1))),
            ("an object as a row", with_attr(&ci, "colors", list(evil()))),
            (
                "another row class",
                with_attr(&ci, "modifiers", list(other_row)),
            ),
            (
                "a colour row as a modifier",
                with_attr(&ci, "modifiers", list(color(1))),
            ),
            (
                "a row with items",
                with_attr(&ci, "modifiers", list(with_items)),
            ),
            (
                "a row of the call form",
                with_attr(&ci, "modifiers", list(call_form)),
            ),
            (
                "a row of the wrong arity",
                with_attr(&ci, "modifiers", list(row(1, vec![n(1)]))),
            ),
            (
                "a string in a row",
                with_attr(
                    &ci,
                    "modifiers",
                    list(row(1, vec![bstr("x"), n(1), PyValue::None])),
                ),
            ),
            (
                "an object nested in a row",
                with_attr(
                    &ci,
                    "modifiers",
                    list(row(1, vec![evil(), n(1), PyValue::None])),
                ),
            ),
            (
                "a list nested in a row",
                with_attr(
                    &ci,
                    "sculpts",
                    list(row(
                        2,
                        vec![
                            n(1),
                            PyValue::List(vec![n(1)]),
                            PyValue::None,
                            PyValue::None,
                        ],
                    )),
                ),
            ),
            (
                "a NaN weight",
                with_attr(&ci, "sculpts", list(sculpt([Some(f64::NAN), None, None]))),
            ),
            (
                "too many rows",
                with_attr(
                    &ci,
                    "modifiers",
                    PyValue::List((0..=MAX_ROWS as i64).map(modifier).collect()),
                ),
            ),
            (
                "appearance not a row",
                with_attr(&ci, "appearance", bstr("x")),
            ),
        ];
        let mut reg = crate::testkit::registry_with(|_| {});
        for (why, v) in &bad {
            assert!(check_char_info(v).is_err(), "charInfo with {why} passed");
            assert!(
                store(&mut reg, 91_000_001, v, &portrait()).is_err(),
                "{why}"
            );
        }
        assert_eq!(dna(&reg, 91_000_001), None);
        assert_eq!(portrait_info(&reg, 91_000_001), None);
    }

    /// Every `portraitInfo` that is not the portrait setup's shape is
    /// refused, and the doll with it keeps nothing.
    #[test]
    fn a_portrait_info_of_another_shape_is_refused() {
        let pi = portrait();
        let many: Vec<String> = (0..=MAX_POSE).map(|i| format!("P{i}")).collect();
        let bad = [
            ("another class", evil()),
            ("an unknown key", with_attr(&pi, "script", bstr("x"))),
            ("a float id", with_attr(&pi, "lightID", PyValue::Real(1.0))),
            (
                "a string number",
                with_attr(&pi, "lightIntensity", bstr("1")),
            ),
            (
                "two numbers for a position",
                with_attr(
                    &pi,
                    "cameraPoi",
                    PyValue::Tuple(vec![PyValue::Int(1), PyValue::Int(2)]),
                ),
            ),
            (
                "pose not a dict",
                with_attr(&pi, "poseData", PyValue::List(vec![])),
            ),
            (
                "a dict nested in the pose",
                portrait_with(pose_with(vec![("Nested", pose_with(vec![]))])),
            ),
            (
                "an object in the pose",
                portrait_with(pose_with(vec![("Evil", evil())])),
            ),
            (
                "a pose name that is not a name",
                portrait_with(pose_with(vec![("../x", PyValue::Int(1))])),
            ),
            (
                "too many pose entries",
                portrait_with(pydict(
                    many.iter().map(|k| (k.as_str(), PyValue::Int(0))).collect(),
                )),
            ),
        ];
        let mut reg = crate::testkit::registry_with(|_| {});
        for (why, v) in &bad {
            assert!(
                check_portrait_info(v).is_err(),
                "portraitInfo with {why} passed"
            );
            assert!(
                store(&mut reg, 91_000_001, &char_info(), v).is_err(),
                "{why}"
            );
        }
        assert_eq!(dna(&reg, 91_000_001), None);
    }

    /// A row kept before the shape check is checked when read: one that
    /// fails is not served, nor is one past the size cap.
    #[test]
    fn a_kept_row_of_another_shape_is_not_served() {
        let mut reg = crate::testkit::registry_with(|_| {});
        let raw = to_hex(&crate::wire::dumps(&evil()));
        reg.deltas_mut()
            .put(KIND, vec!["91000001".to_owned()], vec![raw.clone(), raw]);
        assert_eq!(dna(&reg, 91_000_001), None);
        assert_eq!(portrait_info(&reg, 91_000_001), None);
        let big = "00".repeat(MAX_BYTES + 1);
        reg.deltas_mut()
            .put(KIND, vec!["91000002".to_owned()], vec![big.clone(), big]);
        assert_eq!(field(&reg, 91_000_002, 0), None);
    }

    #[test]
    fn hex_round_trips() {
        let b = vec![0u8, 1, 0x7e, 0xff];
        assert_eq!(from_hex(&to_hex(&b)), Some(b));
        assert_eq!(from_hex("abc"), None);
        assert_eq!(from_hex("zz"), None);
    }
}
