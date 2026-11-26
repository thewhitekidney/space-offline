//! Values as the client's node-graph engine sees them: Python 2 objects.
//!
//! A node-graph parameter arrives as a string, an int, a float or a list
//! (the FSD variant arms, `docs/re/node-graphs.md` section 2), and the engine
//! compares them with `nodegraph.common.util.compare_values`, which first runs
//! `_type_casting_madness` over both sides. This module is that function and
//! the `ast.literal_eval` subset it leans on, so a graph compares here exactly
//! as it does in the client **[V]** (read on build 3707965, decompiled
//! `nodegraph/common/util.py`). Anything the subset cannot parse falls back
//! to the raw string, which is what `literal_eval`'s callers do on
//! `ValueError`.

use std::cmp::Ordering;
use std::fmt;

/// A Python value, as far as node graphs use one.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Value>),
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Str(s.to_string())
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Value::None)
    }

    /// Python truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Value::None => false,
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Bool(b) => Some(i64::from(*b)),
            Value::Float(f) if f.fract() == 0.0 => Some(*f as i64),
            Value::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    /// The text of a string value; `None` for anything else.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    fn kind(&self) -> u8 {
        match self {
            Value::None => 0,
            Value::Bool(_) => 1,
            Value::Int(_) => 2,
            Value::Float(_) => 3,
            Value::Str(_) => 4,
            Value::List(_) => 5,
        }
    }

    /// Python `==`: numbers compare across int/float/bool.
    pub fn py_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::List(a), Value::List(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.py_eq(y))
            }
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::None, Value::None) => true,
            (a, b) if a.is_number() && b.is_number() => a.as_float() == b.as_float(),
            _ => false,
        }
    }

    fn is_number(&self) -> bool {
        matches!(self, Value::Bool(_) | Value::Int(_) | Value::Float(_))
    }

    /// Python 2 ordering for the comparison operators; None when the two
    /// cannot be ordered meaningfully (the caller's comparison is then False).
    fn py_cmp(&self, other: &Value) -> Option<Ordering> {
        match (self, other) {
            (a, b) if a.is_number() && b.is_number() => a.as_float()?.partial_cmp(&b.as_float()?),
            (Value::Str(a), Value::Str(b)) => Some(a.cmp(b)),
            (Value::List(a), Value::List(b)) => {
                for (x, y) in a.iter().zip(b) {
                    match x.py_cmp(y)? {
                        Ordering::Equal => continue,
                        o => return Some(o),
                    }
                }
                Some(a.len().cmp(&b.len()))
            }
            _ => None,
        }
    }

    /// `a in b` for the `in` operator: list membership or a substring.
    fn py_in(&self, container: &Value) -> bool {
        match container {
            Value::List(items) => items.iter().any(|x| self.py_eq(x)),
            Value::Str(s) => match self {
                Value::Str(sub) => s.contains(sub.as_str()),
                _ => false,
            },
            // `a in (b or [])`: a falsy b is an empty list.
            Value::None => false,
            _ => false,
        }
    }
}

impl fmt::Display for Value {
    /// Python `str()`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::None => write!(f, "None"),
            Value::Bool(b) => write!(f, "{}", if *b { "True" } else { "False" }),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => {
                if x.fract() == 0.0 && x.abs() < 1e16 {
                    write!(f, "{x:.1}")
                } else {
                    write!(f, "{x}")
                }
            }
            Value::Str(s) => write!(f, "{s}"),
            Value::List(items) => {
                write!(f, "[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    match v {
                        Value::Str(s) => write!(f, "'{s}'")?,
                        other => write!(f, "{other}")?,
                    }
                }
                write!(f, "]")
            }
        }
    }
}

// -- ast.literal_eval, the subset node graphs use ------------------------------

/// `ast.literal_eval(text)`: ints, floats, quoted strings, True/False/None
/// and lists or tuples of those. `None` where Python raises.
pub fn literal_eval(text: &str) -> Option<Value> {
    let mut p = Lit {
        s: text.trim().as_bytes(),
        i: 0,
    };
    let v = p.value().ok()?;
    p.ws();
    (p.i == p.s.len()).then_some(v)
}

struct Lit<'a> {
    s: &'a [u8],
    i: usize,
}

impl Lit<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn value(&mut self) -> Result<Value, ()> {
        self.ws();
        let c = *self.s.get(self.i).ok_or(())?;
        match c {
            b'[' | b'(' => self.list(if c == b'[' { b']' } else { b')' }),
            b'\'' | b'"' => self.string(c),
            b'-' | b'+' | b'0'..=b'9' | b'.' => self.number(),
            _ => {
                let start = self.i;
                while self.i < self.s.len()
                    && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_')
                {
                    self.i += 1;
                }
                match &self.s[start..self.i] {
                    b"True" => Ok(Value::Bool(true)),
                    b"False" => Ok(Value::Bool(false)),
                    b"None" => Ok(Value::None),
                    _ => Err(()),
                }
            }
        }
    }

    fn list(&mut self, close: u8) -> Result<Value, ()> {
        self.i += 1;
        let mut items = Vec::new();
        loop {
            self.ws();
            if self.s.get(self.i) == Some(&close) {
                self.i += 1;
                return Ok(Value::List(items));
            }
            items.push(self.value()?);
            self.ws();
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(&c) if c == close => {}
                _ => return Err(()),
            }
        }
    }

    fn string(&mut self, quote: u8) -> Result<Value, ()> {
        self.i += 1;
        let mut out = Vec::new();
        while let Some(&c) = self.s.get(self.i) {
            self.i += 1;
            if c == quote {
                return String::from_utf8(out).map(Value::Str).map_err(|_| ());
            }
            if c == b'\\' {
                let e = *self.s.get(self.i).ok_or(())?;
                self.i += 1;
                out.push(match e {
                    b'n' => b'\n',
                    b't' => b'\t',
                    other => other,
                });
            } else {
                out.push(c);
            }
        }
        Err(())
    }

    fn number(&mut self) -> Result<Value, ()> {
        let start = self.i;
        while self.i < self.s.len()
            && matches!(
                self.s[self.i],
                b'0'..=b'9' | b'.' | b'-' | b'+' | b'e' | b'E'
            )
        {
            self.i += 1;
        }
        let t = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| ())?;
        if let Ok(i) = t.parse::<i64>() {
            return Ok(Value::Int(i));
        }
        t.parse::<f64>().map(Value::Float).map_err(|_| ())
    }
}

/// `BlackboardWriteNode`: a string is `literal_eval`ed, and kept as it is
/// when that fails.
pub fn eval_or_keep(v: &Value) -> Value {
    match v {
        Value::Str(s) => literal_eval(s).unwrap_or_else(|| v.clone()),
        other => other.clone(),
    }
}

// -- compare_values -------------------------------------------------------------

/// `nodegraph.common.util.compare_values`.
pub fn compare_values(a: &Value, b: &Value, operator: Option<&str>, flipped: bool) -> bool {
    let (mut a, mut b) = type_casting_madness(a.clone(), b.clone());
    if flipped {
        std::mem::swap(&mut a, &mut b);
    }
    match operator.unwrap_or("equalTo") {
        "greaterThan" => a.py_cmp(&b) == Some(Ordering::Greater),
        "lessThan" => a.py_cmp(&b) == Some(Ordering::Less),
        "equalTo" => a.py_eq(&b),
        "notEqualTo" => !a.py_eq(&b),
        "in" => a.py_in(&b),
        "notIn" => !a.py_in(&b),
        "greaterThanOrEqual" => matches!(a.py_cmp(&b), Some(Ordering::Greater | Ordering::Equal)),
        "lessThanOrEqual" => matches!(a.py_cmp(&b), Some(Ordering::Less | Ordering::Equal)),
        // An unknown operator is a KeyError in the client, caught: False.
        _ => false,
    }
}

fn is_bool_like(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Int(1) => Some(true),
        Value::Int(0) => Some(false),
        Value::Float(f) if *f == 1.0 => Some(true),
        Value::Float(f) if *f == 0.0 => Some(false),
        Value::Str(s) if s == "1" || s == "True" => Some(true),
        Value::Str(s) if s == "0" || s == "False" => Some(false),
        _ => None,
    }
}

fn type_casting_madness(a: Value, b: Value) -> (Value, Value) {
    // _correct_for_special_types
    let none_str = |v: Value| {
        if v == Value::str("None") {
            Value::None
        } else {
            v
        }
    };
    let (mut a, mut b) = (none_str(a), none_str(b));
    if let (Some(x), Some(y)) = (is_bool_like(&a), is_bool_like(&b)) {
        return (Value::Bool(x), Value::Bool(y));
    }
    if a.is_none() || b.is_none() {
        return (a, b);
    }
    // _try_to_cast_to_iterable
    let iterable = |v: Value| match &v {
        Value::Str(s)
            if (s.starts_with('[') && s.ends_with(']'))
                || (s.starts_with('{') && s.ends_with('}')) =>
        {
            literal_eval(s).unwrap_or(v)
        }
        _ => v,
    };
    a = iterable(a);
    b = iterable(b);
    // _cast_to_the_other_type_if_possible (the bool arm; FSD strings are
    // byte strings, so the unicode arm does not fire for graph values)
    if matches!(b, Value::Bool(_)) {
        a = Value::Bool(a.truthy());
    }
    if matches!(a, Value::Bool(_)) {
        b = Value::Bool(b.truthy());
    }
    if a.kind() != b.kind() {
        let a_eval = evaluated(&a);
        match (cast_like(&a_eval, &a), cast_like(&a_eval, &b)) {
            (Some(x), Some(y)) => return (x, y),
            _ => return (a_eval, evaluated(&b)),
        }
    }
    (a, b)
}

/// `_get_evaluated_value`.
fn evaluated(v: &Value) -> Value {
    match v {
        Value::Str(s) => match literal_eval(s) {
            Some(Value::List(items)) => Value::List(items.iter().map(evaluated).collect()),
            Some(x) => x,
            None => v.clone(),
        },
        Value::List(items) => Value::List(items.iter().map(evaluated).collect()),
        other => other.clone(),
    }
}

/// `type(template)(v)`, None where Python raises.
fn cast_like(template: &Value, v: &Value) -> Option<Value> {
    match template {
        Value::Int(_) => match v {
            Value::Float(f) => Some(Value::Int(f.trunc() as i64)),
            Value::Str(s) => s.trim().parse().ok().map(Value::Int),
            Value::Int(_) | Value::Bool(_) => v.as_int().map(Value::Int),
            _ => None,
        },
        Value::Float(_) => v.as_float().map(Value::Float),
        Value::Str(_) => Some(Value::Str(v.to_string())),
        Value::Bool(_) => Some(Value::Bool(v.truthy())),
        Value::List(_) => match v {
            Value::List(_) => Some(v.clone()),
            Value::Str(s) => Some(Value::List(
                s.chars().map(|c| Value::Str(c.to_string())).collect(),
            )),
            _ => None,
        },
        Value::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_eval_parses_the_graph_subset() {
        assert_eq!(literal_eval("12"), Some(Value::Int(12)));
        assert_eq!(literal_eval("2.5"), Some(Value::Float(2.5)));
        assert_eq!(literal_eval("'a b'"), Some(Value::str("a b")));
        assert_eq!(
            literal_eval("[1, 'x', None]"),
            Some(Value::List(vec![
                Value::Int(1),
                Value::str("x"),
                Value::None
            ]))
        );
        assert_eq!(
            literal_eval("[67,]"),
            Some(Value::List(vec![Value::Int(67)]))
        );
        assert!(literal_eval("1.3.01_some_task").is_none());
    }

    #[test]
    fn an_int_checkpoint_equals_its_string_literal() {
        assert!(compare_values(
            &Value::Int(44),
            &Value::str("44"),
            None,
            false
        ));
        assert!(!compare_values(
            &Value::Int(44),
            &Value::str("45"),
            None,
            false
        ));
    }

    #[test]
    fn in_casts_a_string_list() {
        let list = Value::str("[44, 85, 86]");
        assert!(compare_values(&Value::Int(85), &list, Some("in"), false));
        assert!(!compare_values(&Value::Int(12), &list, Some("in"), false));
        assert!(!compare_values(&Value::None, &list, Some("in"), false));
    }

    #[test]
    fn greater_than_compares_numbers_after_casting() {
        assert!(compare_values(
            &Value::Int(3),
            &Value::str("0"),
            Some("greaterThan"),
            false
        ));
        assert!(!compare_values(
            &Value::None,
            &Value::str("0"),
            Some("greaterThan"),
            false
        ));
    }

    #[test]
    fn bool_likes_become_bools_and_none_strings_become_none() {
        assert!(compare_values(
            &Value::Int(1),
            &Value::str("True"),
            None,
            false
        ));
        assert!(compare_values(
            &Value::str("None"),
            &Value::None,
            None,
            false
        ));
    }

    #[test]
    fn task_names_compare_as_strings() {
        let t = Value::str("1.3.01_some_task");
        assert!(compare_values(
            &t,
            &Value::str("1.3.01_some_task"),
            None,
            false
        ));
        assert!(!compare_values(
            &t,
            &Value::str("1.3.02_other"),
            None,
            false
        ));
    }

    #[test]
    fn eval_or_keep_keeps_what_does_not_parse() {
        assert_eq!(
            eval_or_keep(&Value::str("[1,2]")),
            Value::List(vec![Value::Int(1), Value::Int(2)])
        );
        assert_eq!(
            eval_or_keep(&Value::str("free text")),
            Value::str("free text")
        );
    }
}
