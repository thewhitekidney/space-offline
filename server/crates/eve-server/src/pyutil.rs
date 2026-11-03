//! Small helpers for building the Python values the client expects.
//!
//! These exist because two conventions in this protocol are easy to invert,
//! and inverting either produces a client that fails with no useful message:
//!
//! - A marshal dict stores **`(value, key)`**, not `(key, value)`.
//! - An old-style instance is `(class-name, state)` where the class name is a
//!   **whitelisted dotted path** (`docs/reference/object-wrapper.md`).
//!
//! Writing them once here means a service implementation never has to think
//! about either.

use eve_marshal::{ObjectForm, PyValue};
use std::time::UNIX_EPOCH;

/// A Python 2 `str` from a Rust string.
pub fn bstr(s: &str) -> PyValue {
    PyValue::Bytes(s.as_bytes().to_vec())
}

/// Build a marshal dict from logical `(key, value)` pairs.
///
/// **The wire order is value-then-key**, confirmed from the dict handler at
/// `0x1802dd1d0`, so `PyValue::Dict` stores `(value, key)`. The inversion is
/// written down once, here.
pub fn pydict(kv: Vec<(&str, PyValue)>) -> PyValue {
    PyValue::Dict(pypairs(kv))
}

/// Build a marshal dict from `(key, value)` pairs of any `PyValue` keys (ids,
/// tuples), flipping each to the stored `(value, key)`.
///
/// Use it wherever the key is not a `str` ([`pydict`] covers those): a dict
/// built as `PyValue::Dict(vec![(key, value)])` reaches the client with the
/// value as the key (`KeyError`, or ints where objects were expected).
pub fn dict_kv(pairs: impl IntoIterator<Item = (PyValue, PyValue)>) -> PyValue {
    PyValue::Dict(pairs.into_iter().map(|(k, v)| (v, k)).collect())
}

/// The same inversion, as bare pairs.
///
/// For the places that need `Vec<(PyValue, PyValue)>` rather than a
/// `PyValue::Dict` -- `eve_proto::user_error`'s argument dict is the one that
/// forced this. Without it every error that carries arguments would hand-write
/// `(value, key)` at the call site, which is precisely the mistake that sent
/// `{500: 'maxBlueprintResults'}` and poisoned a service for a whole session.
/// The inversion stays written down once.
pub fn pypairs(kv: Vec<(&str, PyValue)>) -> Vec<(PyValue, PyValue)> {
    kv.into_iter().map(|(k, v)| (v, bstr(k))).collect()
}

/// Look a key up in a marshal dict. Accepts a `str` or a `unicode` key.
pub fn dict_get<'a>(v: &'a PyValue, key: &[u8]) -> Option<&'a PyValue> {
    match v {
        PyValue::Dict(entries) => pairs_get(entries, key),
        _ => None,
    }
}

/// Look a key up in dict **entries**, for callers holding the pairs rather
/// than the [`PyValue::Dict`] around them -- a call's `kwargs` is the one that
/// forced this.
///
/// **The one definition of "the key is the second element", and it is one
/// because a second one was wrong for months.** A marshal dict is stored in
/// wire order and each pair is `(value, key)`: the dict handler does
/// `A = ReadObject(); B = ReadObject(); PyDict_SetItem(dict, B, A)`.
/// [`dict_get`] had this right from the start; `space_state::kwarg` was
/// written separately, matched the name against the *value* slot, and made
/// `CmdAlignTo` silently do nothing while every unit test passed --
/// `docs/port-tracker.md` §39. [`pypairs`] is the same inversion on the way
/// out, and its own comment records an earlier instance that "poisoned a
/// service for a whole session".
///
/// Three hand-rolled spellings of this is how the third one gets it wrong, so
/// new readers should call this rather than write a `find`.
pub fn pairs_get<'a>(entries: &'a [(PyValue, PyValue)], key: &[u8]) -> Option<&'a PyValue> {
    entries
        .iter()
        .find(|(_, k)| as_bytes(k) == Some(key))
        .map(|(value, _)| value)
}

/// A Python 2 `str` or `unicode` as bytes.
pub fn as_bytes(v: &PyValue) -> Option<&[u8]> {
    match v {
        PyValue::Bytes(b) => Some(b),
        PyValue::Unicode(s) => Some(s.as_bytes()),
        _ => None,
    }
}

/// The whitelisted name of `KeyVal`, the client's general attribute bag.
pub const KEYVAL_CLASS: &str = "utillib.KeyVal";

/// A `utillib.KeyVal` carrying these attributes.
///
/// **Why this works, established from the client's own bytecode:** `KeyVal`
/// is declared in `utillib/__init__.py` with an **empty bases tuple**, so it
/// is an old-style class and marshals as tag `0x17`. It defines
/// `__init__`, `__getitem__`, `__setitem__`, `update`, `copy`, `get`, `Get`
/// and `Set` — but **no `__setstate__`**. So the reader builds it with
/// `PyInstance_NewRaw` (no `__init__`) and then does
/// `instance.__dict__.update(state)`, which means the state is simply a dict
/// of attributes and every key becomes an attribute.
///
/// That matters more than it looks: most of what the character-select screen
/// reads is attribute access on rows, so a `KeyVal` stands in for a row
/// without needing the rowset layer at all.
/// One optional id argument, out of a call's positional args.
///
/// `None` and `0` both mean "the player picked nothing", which is the
/// client's own sentinel in several windows -- `ItemReprocessor.Reprocess`
/// passes `(None, None)` and the industry wizard passes `0` for "any
/// facility". Treating `0` as an id would turn "nothing" into a lookup for
/// item 0, which fails in a different and less obvious place.
///
/// Lived in `service/reprocess.rs` until `service/industry.rs` needed the
/// same rule; a second copy is how argument parsing drifts between arms.
pub fn opt_id(arg: Option<&PyValue>) -> Option<i64> {
    match arg {
        Some(PyValue::Int(n)) if *n != 0 => Some(*n),
        _ => None,
    }
}

pub fn keyval(attrs: Vec<(&str, PyValue)>) -> PyValue {
    instance(KEYVAL_CLASS, attrs)
}

/// Any old-style instance carrying these attributes.
///
/// The `KeyVal` reasoning above generalises: a Python 2 class declared with an
/// **empty bases tuple** is old-style, marshals as tag `0x17`, and — when it
/// has no `__setstate__` — is rebuilt with `PyInstance_NewRaw` followed by
/// `instance.__dict__.update(state)`. So the state is just an attribute dict
/// and no constructor runs.
///
/// Two conditions, both worth checking before using this for a new class:
///
/// - the dotted name **must be in `globalsWhitelist`**, or the client raises
///   `HACKER WARNING! object %s.%s is not in whitelist`. `eve_rowset::is_whitelisted`
///   knows the list.
/// - the class must genuinely have no `__setstate__`, or it would be called
///   with our dict instead of the dict being installed.
///
/// Note that double-underscore attribute names are mangled by the *compiler*,
/// so a class storing `self.__x` has an attribute named `_ClassName__x`. The
/// names here are whatever `__init__` actually assigns.
pub fn instance(class: &str, attrs: Vec<(&str, PyValue)>) -> PyValue {
    // **The first condition above, enforced rather than described.** An
    // unlisted dotted name does not produce a missing attribute or an odd
    // value: the client aborts the whole read with `HACKER WARNING! object
    // %s.%s is not in whitelist` -- a refusal that looks nothing like a typo
    // of ours, in a log line that accuses us of something else entirely. The
    // list is already here with its own tests, so a class-name typo can be a
    // failing test instead.
    //
    // `debug_assert` and not `assert`: in a release build a wrong name is
    // still better sent than panicked on, because a frame the client refuses
    // is evidence and a dead connection is not.
    debug_assert!(
        eve_rowset::is_whitelisted(class.as_bytes()),
        "{class} is not in the client's globalsWhitelist; it would abort the \
         read with HACKER WARNING rather than ignore the value"
    );
    PyValue::Instance {
        class: class.as_bytes().to_vec(),
        state: Box::new(pydict(attrs)),
    }
}

/// A new-style instance: tag `0x23`, header `((Global(class),), state)`.
///
/// **The shape, and why it is not [`instance`].** A class with a non-empty
/// bases tuple (ultimately `object`) is new-style and marshals as `0x23`, not
/// `0x17`. The client calls `header[0][0].__new__(*header[0])` and then
/// `__dict__.update(header[1])`, so **`__init__` and its validators never
/// run**: the server owns every invariant `__init__` would have enforced
/// (`docs/content/C2-live-surface.md` §1.2, `docs/archive/marshal-tags.md`). [V]
/// `CharacterSkillEntry` has gone to the client in exactly this form since
/// before this helper existed (`skill.rs`, `SkillEntry::to_pyvalue`).
///
/// The keys are the instance's `__dict__` names, which is whatever its
/// `__init__` assigns: underscored for most (`_typeID`, `_occupierID`), plain
/// for the `explorationscanner` site-info classes (`position`, `targetID`).
/// Read them off the `__init__` body; do not guess.
///
/// The list and dict streams are always empty here: none of the classes this
/// was written for carries list or dict items, and a class that does is not a
/// plain state bag and should be built by hand with a comment saying why.
///
/// The class name must be in `globalsWhitelist`, enforced as in
/// [`instance`] and for the same reason.
///
/// ```
/// use eve_marshal::{ObjectForm, PyValue};
/// use eve_server::pyutil::{dict_get, new_style};
///
/// let v = new_style(
///     "explorationscanner.common.cosmicAnomalyInfo.CosmicAnomalyInfo",
///     vec![("targetID", eve_server::pyutil::bstr("XYZ-001"))],
/// );
/// let PyValue::ObjectEx { form, header, .. } = &v else { panic!() };
/// assert_eq!(*form, ObjectForm::New);
/// let PyValue::Tuple(parts) = &**header else { panic!() };
/// assert!(dict_get(&parts[1], b"targetID").is_some());
/// ```
pub fn new_style(class: &str, state: Vec<(&str, PyValue)>) -> PyValue {
    debug_assert!(
        eve_rowset::is_whitelisted(class.as_bytes()),
        "{class} is not in the client's globalsWhitelist; it would abort the \
         read with HACKER WARNING rather than ignore the value"
    );
    PyValue::ObjectEx {
        form: ObjectForm::New,
        header: Box::new(PyValue::Tuple(vec![
            // The class sits inside its own tuple: element 0 of the header is
            // the argument tuple handed to `__new__`, whose first item is the
            // class itself.
            PyValue::Tuple(vec![PyValue::Global(class.as_bytes().to_vec())]),
            pydict(state),
        ])),
        list: Vec::new(),
        dict: Vec::new(),
    }
}

/// Windows FILETIME now: 100-ns ticks since 1601-01-01 UTC.
///
/// This is `blue.os.GetWallclockTimeNow()`. The client uses it to set its own
/// clock (`connection.SynchronizeClock` takes three good samples of five), so
/// it is one of the few answers that has to be a real value rather than a
/// placeholder — a wrong clock desynchronises everything timed afterwards.
pub fn filetime_now() -> i64 {
    #[cfg(test)]
    let t = crate::test_clock::system_now();
    #[cfg(not(test))]
    let t = std::time::SystemTime::now();
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs().saturating_add(FILETIME_EPOCH_OFFSET_SECS);
    let ticks = secs.saturating_mul(10_000_000) + u64::from(d.subsec_nanos()) / 100;
    i64::try_from(ticks).unwrap_or(i64::MAX)
}

/// Seconds between the Windows FILETIME epoch (1601-01-01 UTC) and the Unix
/// epoch. `machoNet.GetTime` returns 100-ns ticks since the former.
const FILETIME_EPOCH_OFFSET_SECS: u64 = 11_644_473_600;

// ---- CachedMethodCallResult (lifted from service/market.rs by MAP-STATS) ----

/// The whitelisted class name, `eve-rowset/src/whitelist.rs:85`.
pub(crate) const CACHED_METHOD_CALL_RESULT: &str =
    "carbon.common.script.net.objectCaching.CachedMethodCallResult";

/// `GetOrders`' answer, wrapped so the client's order cache can ever change.
///
/// **Without this the market window cannot update, whatever the book says.**
/// `RefreshOrderCache` (marketsvc.py:846) re-fetches on every call but only
/// *stores* when the version moved:
///
/// ```text
/// orders = self.GetMarketProxy().GetOrders(typeID)                         # 856
/// version = objectCaching.GetCachedMethodCallVersion(None, 'marketProxy',
///                                                    'GetOrders', (typeID,)) # 857
/// if typeID in self.orderCache and self.orderCache[typeID][2] == version:   # 859-860
///     return                                                                # 861
/// ```
///
/// and `GetCachedMethodCallVersion` (objectCaching.py:527) is `0` for any
/// call the server never marked cachable. A plain answer is therefore read
/// once per type per session and every later book is thrown away. **[V]**
///
/// So the answer is a `CachedMethodCallResult`, which the client recognises
/// at ServiceCallGPCS.py:335 and hands to `CacheMethodCall` (:336) before
/// unwrapping it with `GetResult` (:337). Its state is the `__getstate__`
/// triple (objectCaching.py:860-864): `(details, blue.marshal.Save(result),
/// [wallclock, version])`. The version is adler32 of those bytes, which is
/// what the client's own constructor computes (:857) -- so it changes
/// exactly when the book does.
///
/// `details = {'versionCheck': 'always'}`: `__ShouldVersionCheck` returns 1
/// for it unconditionally (objectCaching.py:745), so every call still comes
/// to us -- the traffic of today's plain answer -- and the cache is used only
/// for its version. **[A]** on the choice: the real server's period is
/// unknown, and anything longer would show one player's new order to
/// another player late. **Choose carefully if this changes**:
/// `CacheMethodCall` stores the first `details` it sees per method and keeps
/// them (objectCaching.py:661), and the client persists them to disk.
///
/// `bytes` is the answer already marshalled, `version` its adler32 (FX8:
/// [`get_orders`] keeps both).
pub(crate) fn cached_bytes(bytes: Vec<u8>, version: i64, now: i64) -> PyValue {
    PyValue::Instance {
        class: CACHED_METHOD_CALL_RESULT.as_bytes().to_vec(),
        state: Box::new(PyValue::Tuple(vec![
            crate::pyutil::pydict(vec![("versionCheck", crate::pyutil::bstr("always"))]),
            PyValue::Bytes(bytes),
            // A list, not a tuple: `UpdateVersionCheckPeriod` assigns into
            // `version[0]`.
            PyValue::List(vec![PyValue::Int(now), PyValue::Int(version)]),
        ])),
    }
}

/// zlib's adler32, the checksum `CachedMethodCallResult.__init__` uses.
pub(crate) fn adler32(bytes: &[u8]) -> i64 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in bytes.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    i64::from((b << 16) | a)
}

#[cfg(test)]
mod tests {
    use super::dict_kv;

    #[test]
    fn dict_kv_takes_key_value_and_stores_value_key() {
        let d = dict_kv([(PyValue::Int(7), PyValue::Bytes(b"x".to_vec()))]);
        assert_eq!(
            d,
            PyValue::Dict(vec![(PyValue::Bytes(b"x".to_vec()), PyValue::Int(7))])
        );
        let kv = d.dict_as_kv().unwrap();
        assert_eq!(kv[0].0, &PyValue::Int(7));
    }

    use super::*;

    #[test]
    fn pydict_stores_value_then_key() {
        let PyValue::Dict(entries) = pydict(vec![("k", PyValue::Int(7))]) else {
            panic!("not a dict")
        };
        assert_eq!(entries[0].0, PyValue::Int(7), "slot 0 is the value");
        assert_eq!(entries[0].1, bstr("k"), "slot 1 is the key");
    }

    #[test]
    fn keyval_is_an_instance_whose_state_is_an_attribute_dict() {
        let v = keyval(vec![("characterID", PyValue::Int(90_000_001))]);
        let PyValue::Instance { class, state } = &v else {
            panic!("KeyVal must be a 0x17 instance")
        };
        assert_eq!(String::from_utf8(class.clone()).unwrap(), "utillib.KeyVal");
        assert_eq!(
            dict_get(state, b"characterID"),
            Some(&PyValue::Int(90_000_001))
        );
    }

    #[test]
    fn the_keyval_class_name_is_the_whitelisted_one() {
        // utillib.KeyVal appears verbatim in globalsWhitelist. A name that is
        // not in that list is refused with "HACKER WARNING!", so this string
        // is load-bearing and must not be "tidied".
        assert_eq!(KEYVAL_CLASS, "utillib.KeyVal");
    }

    #[test]
    fn dict_lookup_accepts_a_unicode_key() {
        let d = PyValue::Dict(vec![(PyValue::Int(3), PyValue::Unicode("k".to_owned()))]);
        assert_eq!(dict_get(&d, b"k"), Some(&PyValue::Int(3)));
    }

    /// **The slot, pinned, because getting it backwards cost a whole feature.**
    ///
    /// A marshal dict pair is `(value, key)` -- the dict handler reads the
    /// value first and the key second. `space_state::kwarg` matched the name
    /// against the value slot and `CmdAlignTo` did nothing at all, silently,
    /// while every unit test of it passed: each hand-built its kwargs in the
    /// opposite order from the wire, so the test and the code were wrong in
    /// the same direction. `docs/port-tracker.md` §39.
    ///
    /// This asserts the asymmetry rather than just a successful lookup, which
    /// is what makes it a guard: a flipped implementation satisfies "finds
    /// `k`" if the pair is symmetric, so the two values are deliberately
    /// different types and the wrong-way lookup is asserted to fail.
    #[test]
    fn a_dict_key_is_the_second_element_of_each_pair() {
        // `{"dstID": 60012526}` exactly as the wire carries it.
        let entries = vec![(PyValue::Int(60_012_526), bstr("dstID"))];
        assert_eq!(
            pairs_get(&entries, b"dstID"),
            Some(&PyValue::Int(60_012_526)),
            "the key is the second element and the value is the first"
        );
        assert_eq!(
            pairs_get(&entries, b"60012526"),
            None,
            "and the value is not a key: a reader that matched the first slot              would be the §39 defect"
        );
        // The same through `dict_get`, since they must not drift apart again.
        let d = PyValue::Dict(entries);
        assert_eq!(dict_get(&d, b"dstID"), Some(&PyValue::Int(60_012_526)));
        // And `pypairs` builds what these read: one inversion, stated once.
        assert_eq!(
            pairs_get(
                &pypairs(vec![("minRange", PyValue::Real(1.5))]),
                b"minRange"
            ),
            Some(&PyValue::Real(1.5)),
            "the builder and the reader have to agree, or an outgoing dict is              unreadable by our own code"
        );
    }
    /// A site-info class, built with invented values, comes out as a `0x23`
    /// object whose state keys are the plain `__init__` names. [V] The tag
    /// byte is read off the encoder rather than trusted from the enum.
    #[test]
    fn a_cosmic_anomaly_info_encodes_as_0x23_with_plain_keys() {
        const CLASS: &str = "explorationscanner.common.cosmicAnomalyInfo.CosmicAnomalyInfo";
        let v = new_style(
            CLASS,
            vec![
                (
                    "position",
                    PyValue::Tuple(vec![
                        PyValue::Real(1.0),
                        PyValue::Real(2.0),
                        PyValue::Real(3.0),
                    ]),
                ),
                ("targetID", bstr("XYZ-001")),
                ("difficulty", PyValue::Int(2)),
                ("instanceID", PyValue::Int(900_001)),
            ],
        );
        let mut w = eve_marshal::Writer::new();
        eve_marshal::save(&v, &mut w);
        let bytes = w.into_bytes();
        assert_eq!(
            bytes[0] & eve_marshal::tag::TYPE_MASK,
            eve_marshal::tag::code::OBJECT_23,
            "new-style: tag 0x23"
        );
        let PyValue::ObjectEx {
            form,
            header,
            list,
            dict,
        } = &v
        else {
            panic!("not an ObjectEx")
        };
        assert_eq!(*form, ObjectForm::New);
        assert!(list.is_empty() && dict.is_empty());
        let PyValue::Tuple(parts) = &**header else {
            panic!("header is (classtuple, state)")
        };
        assert_eq!(
            parts[0],
            PyValue::Tuple(vec![PyValue::Global(CLASS.as_bytes().to_vec())])
        );
        for key in ["position", "targetID", "difficulty", "instanceID"] {
            assert!(
                dict_get(&parts[1], key.as_bytes()).is_some(),
                "plain key {key}"
            );
            assert!(
                dict_get(&parts[1], format!("_{key}").as_bytes()).is_none(),
                "explorationscanner keys are not underscored"
            );
        }
    }
}
