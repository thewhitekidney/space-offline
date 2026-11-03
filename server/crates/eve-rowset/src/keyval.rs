//! `utillib.KeyVal` — the general attribute bag, used everywhere in the
//! client's service replies.
//!
//! **Confirmed** from `packages/utillib/__init__.pyc`:
//!
//! ```python
//! class KeyVal:
//!     __passbyvalue__ = 1
//!     def __init__(self, dictLikeObject=None, **kw):
//!         self.__dict__ = kw
//!         ...
//! ```
//!
//! An empty bases tuple makes it **old-style** in Python 2.7, so it goes on
//! the wire as tag `0x17`: a dotted class-name string, then the state. It
//! defines neither `__getstate__` nor `__setstate__`, so the reader's
//! fallback applies and the state is a dict the instance's `__dict__` is
//! updated from (`0x1802de321`/`0x1802de335`). `__init__` is never called —
//! the reader uses `PyInstance_NewRaw` — which is why the `dictLikeObject`
//! parameter is irrelevant to us.
//!
//! Note the class has **no `__guid__`**, and a `__guid__` would not be the
//! name on the wire anyway: the `0x17` writer builds
//! `__module__ + "." + __name__` (`0x1802e0142`).

use crate::whitelist;
use eve_marshal::PyValue;

/// The dotted name the `0x17` class-name string must carry.
///
/// **Confirmed**: `utillib.KeyVal` is an entry of the client's whitelist, and
/// the class lives in `packages/utillib/__init__.py`, whose `__module__` is
/// `utillib`.
pub const KEY_VAL_CLASS: &[u8] = b"utillib.KeyVal";

/// An attribute bag that marshals as a `utillib.KeyVal`.
///
/// Attributes are kept in insertion order. Setting the same key twice
/// replaces the value rather than emitting it twice, because the client
/// applies the state with `PyDict_Update` and a duplicate key would silently
/// win or lose depending on order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeyVal {
    entries: Vec<(Vec<u8>, PyValue)>,
}

impl KeyVal {
    pub fn new() -> Self {
        KeyVal::default()
    }

    /// Set one attribute, replacing any previous value for that name.
    pub fn set(&mut self, key: &str, value: PyValue) -> &mut Self {
        let key = key.as_bytes();
        match self.entries.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => self.entries.push((key.to_vec(), value)),
        }
        self
    }

    /// Builder form of [`KeyVal::set`].
    #[must_use]
    pub fn with(mut self, key: &str, value: PyValue) -> Self {
        self.set(key, value);
        self
    }

    pub fn get(&self, key: &str) -> Option<&PyValue> {
        self.entries
            .iter()
            .find(|(k, _)| k == key.as_bytes())
            .map(|(_, v)| v)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Attribute names, in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &[u8]> {
        self.entries.iter().map(|(k, _)| k.as_slice())
    }

    /// The `0x17` instance.
    pub fn to_pyvalue(&self) -> PyValue {
        // PyValue::Dict carries entries in WIRE order, which for tag 0x16 is
        // (value, key) -- the opposite of the 0x22 setitem stream. Getting
        // this round the wrong way is the classic bug in this format, so the
        // flip happens exactly here and nowhere else in this crate.
        let state = PyValue::Dict(
            self.entries
                .iter()
                .map(|(k, v)| (v.clone(), PyValue::Bytes(k.clone())))
                .collect(),
        );
        debug_assert!(whitelist::is_whitelisted(KEY_VAL_CLASS));
        PyValue::Instance {
            class: KEY_VAL_CLASS.to_vec(),
            state: Box::new(state),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_marshal::{Decoder, Reader, Writer, save, tag};

    #[test]
    fn a_keyval_is_an_old_style_instance_with_a_dict_state() {
        let kv = KeyVal::new()
            .with("userName", PyValue::Unicode("pilot".into()))
            .with("characterSlots", PyValue::Int(3));
        let mut w = Writer::new();
        save(&kv.to_pyvalue(), &mut w);

        // Hand-written from docs/reference/object-wrapper.md §8: tag, the dotted class
        // name as a 0x10 short string, then the state object. No terminator.
        let mut want = vec![
            tag::code::INSTANCE,
            tag::code::STRING_SHORT,
            KEY_VAL_CLASS.len() as u8,
        ];
        want.extend_from_slice(KEY_VAL_CLASS);
        want.extend_from_slice(&[
            tag::code::DICT,
            2, // pair count
            // pair 1: VALUE then KEY, which is tag 0x16's order
            tag::code::UNICODE_UTF8,
            5,
            b'p',
            b'i',
            b'l',
            b'o',
            b't',
            tag::code::STRING_SHORT,
            8,
            b'u',
            b's',
            b'e',
            b'r',
            b'N',
            b'a',
            b'm',
            b'e',
            // pair 2
            tag::code::INT8,
            3,
            tag::code::STRING_SHORT,
            14,
            b'c',
            b'h',
            b'a',
            b'r',
            b'a',
            b'c',
            b't',
            b'e',
            b'r',
            b'S',
            b'l',
            b'o',
            b't',
            b's',
        ]);
        assert_eq!(w.into_bytes(), want);
    }

    #[test]
    fn a_keyval_round_trips_through_the_codec() {
        let kv = KeyVal::new()
            .with("a", PyValue::None)
            .with("b", PyValue::Bool(true));
        let v = kv.to_pyvalue();
        let mut w = Writer::new();
        save(&v, &mut w);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(Decoder::new().load(&mut r).unwrap(), v);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn setting_a_key_twice_replaces_rather_than_duplicates() {
        let mut kv = KeyVal::new();
        kv.set("x", PyValue::Int(1));
        kv.set("x", PyValue::Int(2));
        assert_eq!(kv.len(), 1);
        assert_eq!(kv.get("x"), Some(&PyValue::Int(2)));
    }

    #[test]
    fn an_empty_keyval_still_carries_an_empty_dict() {
        // The reader calls PyDict_Update unconditionally, so the state has to
        // be a dict even when there is nothing in it.
        let mut w = Writer::new();
        save(&KeyVal::new().to_pyvalue(), &mut w);
        let bytes = w.into_bytes();
        assert_eq!(bytes[bytes.len() - 2], tag::code::DICT);
        assert_eq!(bytes[bytes.len() - 1], 0, "pair count");
    }
}
