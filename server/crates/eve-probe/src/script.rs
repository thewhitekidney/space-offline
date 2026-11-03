//! The session as data, not code.
//!
//! The point of a probe is to ask a server a different question tomorrow, and
//! a rebuild between questions is a tax on exactly the thing it is for. So
//! the sequence of calls lives in a text file.
//!
//! # Grammar
//!
//! One directive per line; `#` starts a comment; blank lines are ignored.
//!
//! ```text
//! note  <free text>                        a heading in the transcript
//! call  <service>.<method> [args...]       a plain service call
//! bind  <service> <bindParams> <method> [args...]
//!                                          MachoResolveObject + MachoBindObject
//!                                          with the first call fused in
//! on    <objectID-or-label> <method> [args...]
//!                                          a call on an object a `bind` returned
//! drain <ms>                               read and record whatever arrives
//! ```
//!
//! `bind` takes the bindParams as one literal — usually a tuple, e.g.
//! `(90000001, 1)` for `(charID, groupCharacter)` — and labels the resulting
//! object with the service name, so a later `on` can name it.
//!
//! # Argument literals
//!
//! | literal | value |
//! |---|---|
//! | `none` / `true` / `false` | `None` / `True` / `False` |
//! | `42`, `-1` | int |
//! | `1.5` | real |
//! | `"text"`, `'text'` | Python 2 `str` (bytes) |
//! | `u"text"` | `unicode` |
//! | `[a, b]`, `(a, b)` | list, tuple |
//! | `{"k": v}` | dict |
//!
//! A one-element tuple needs a trailing comma — `(5,)` — exactly as in
//! Python, because `(5)` is five.

use eve_marshal::PyValue;

/// One line of a script.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Note(String),
    Call {
        service: String,
        method: String,
        args: Vec<PyValue>,
    },
    Bind {
        service: String,
        bind_params: PyValue,
        method: String,
        args: Vec<PyValue>,
    },
    On {
        label: String,
        method: String,
        args: Vec<PyValue>,
    },
    Drain {
        millis: u64,
    },
}

/// A parse failure, with the line it happened on.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptError {
    pub line: usize,
    pub message: String,
}

impl core::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ScriptError {}

/// Parse a whole script.
pub fn parse(text: &str) -> Result<Vec<Step>, ScriptError> {
    let mut steps = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let trimmed = strip_comment(raw).trim();
        if trimmed.is_empty() {
            continue;
        }
        steps.push(parse_line(trimmed).map_err(|message| ScriptError { line, message })?);
    }
    Ok(steps)
}

/// `#` outside a quoted string starts a comment. Doing this with `split('#')`
/// would truncate `call x.y "a#b"`, which is a real argument shape.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;
    for (i, &b) in bytes.iter().enumerate() {
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == b'#' => return &line[..i],
            None => {}
        }
    }
    line
}

fn parse_line(line: &str) -> Result<Step, String> {
    let (verb, rest) = split_word(line);
    match verb {
        "note" => Ok(Step::Note(rest.trim().to_string())),
        "drain" => {
            let millis: u64 = rest
                .trim()
                .parse()
                .map_err(|_| format!("drain wants a whole number of milliseconds, got {rest:?}"))?;
            Ok(Step::Drain { millis })
        }
        "call" => {
            let (target, args_src) = split_word(rest.trim());
            let (service, method) = target
                .split_once('.')
                .ok_or_else(|| format!("call wants <service>.<method>, got {target:?}"))?;
            if service.is_empty() || method.is_empty() {
                return Err(format!("call wants <service>.<method>, got {target:?}"));
            }
            Ok(Step::Call {
                service: service.to_string(),
                method: method.to_string(),
                args: parse_args(args_src)?,
            })
        }
        "bind" => {
            let (service, rest) = split_word(rest.trim());
            if service.is_empty() {
                return Err("bind wants a service name".into());
            }
            let mut p = Parser::new(rest);
            p.skip_space();
            let bind_params = p.value()?;
            p.skip_space();
            let (method, args_src) = split_word(p.remainder());
            if method.is_empty() {
                return Err("bind wants a method to fuse into the bind".into());
            }
            Ok(Step::Bind {
                service: service.to_string(),
                bind_params,
                method: method.to_string(),
                args: parse_args(args_src)?,
            })
        }
        "on" => {
            let (label, rest) = split_word(rest.trim());
            let (method, args_src) = split_word(rest.trim());
            if label.is_empty() || method.is_empty() {
                return Err("on wants <object> <method>".into());
            }
            Ok(Step::On {
                label: label.to_string(),
                method: method.to_string(),
                args: parse_args(args_src)?,
            })
        }
        other => Err(format!("unknown directive {other:?}")),
    }
}

/// Take the leading word. A word ends at whitespace **or a comma**, because
/// `call a.b, 1` is a natural thing to write and a splitter that only knows
/// whitespace silently calls a method named `b,`.
fn split_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(|c: char| c.is_whitespace() || c == ',') {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    }
}

/// Parse a whitespace- or comma-separated argument list.
pub fn parse_args(src: &str) -> Result<Vec<PyValue>, String> {
    let mut p = Parser::new(src);
    let mut args = Vec::new();
    loop {
        p.skip_separators();
        if p.at_end() {
            return Ok(args);
        }
        args.push(p.value()?);
    }
}

/// Parse one literal, for callers that want exactly one.
pub fn parse_value(src: &str) -> Result<PyValue, String> {
    let mut p = Parser::new(src);
    p.skip_space();
    let v = p.value()?;
    p.skip_separators();
    if !p.at_end() {
        return Err(format!("trailing text after literal: {:?}", p.remainder()));
    }
    Ok(v)
}

struct Parser<'a> {
    src: &'a str,
    at: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Parser { src, at: 0 }
    }

    fn remainder(&self) -> &'a str {
        &self.src[self.at..]
    }

    fn at_end(&self) -> bool {
        self.at >= self.src.len()
    }

    fn peek(&self) -> Option<u8> {
        self.src.as_bytes().get(self.at).copied()
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b) if b.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    fn skip_separators(&mut self) {
        while matches!(self.peek(), Some(b) if b.is_ascii_whitespace() || b == b',') {
            self.at += 1;
        }
    }

    fn eat(&mut self, want: u8) -> bool {
        if self.peek() == Some(want) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Result<PyValue, String> {
        self.skip_space();
        match self.peek() {
            None => Err("expected a value, found end of line".into()),
            Some(b'[') => {
                let items = self.sequence(b'[', b']')?;
                Ok(PyValue::List(items))
            }
            Some(b'(') => {
                let items = self.sequence(b'(', b')')?;
                Ok(PyValue::Tuple(items))
            }
            Some(b'{') => self.dict(),
            Some(b'"') | Some(b'\'') => {
                let s = self.string()?;
                Ok(PyValue::Bytes(s))
            }
            Some(b'u') if matches!(self.src.as_bytes().get(self.at + 1), Some(b'"' | b'\'')) => {
                self.at += 1;
                let s = self.string()?;
                String::from_utf8(s)
                    .map(PyValue::Unicode)
                    .map_err(|_| "unicode literal is not valid UTF-8".to_string())
            }
            _ => self.bare(),
        }
    }

    fn sequence(&mut self, open: u8, close: u8) -> Result<Vec<PyValue>, String> {
        if !self.eat(open) {
            return Err(format!("expected {:?}", open as char));
        }
        let mut items = Vec::new();
        loop {
            self.skip_separators();
            if self.eat(close) {
                return Ok(items);
            }
            if self.at_end() {
                return Err(format!("unterminated {:?}", open as char));
            }
            items.push(self.value()?);
        }
    }

    fn dict(&mut self) -> Result<PyValue, String> {
        if !self.eat(b'{') {
            return Err("expected '{'".into());
        }
        // Entries are stored in wire order, which is (value, key).
        let mut entries = Vec::new();
        loop {
            self.skip_separators();
            if self.eat(b'}') {
                return Ok(PyValue::Dict(entries));
            }
            if self.at_end() {
                return Err("unterminated '{'".into());
            }
            let key = self.value()?;
            self.skip_space();
            if !self.eat(b':') {
                return Err("expected ':' in dict".into());
            }
            let value = self.value()?;
            entries.push((value, key));
        }
    }

    fn string(&mut self) -> Result<Vec<u8>, String> {
        let quote = self.peek().ok_or("expected a quote")?;
        self.at += 1;
        let mut out = Vec::new();
        loop {
            let b = self.peek().ok_or("unterminated string")?;
            self.at += 1;
            if b == quote {
                return Ok(out);
            }
            if b == b'\\' {
                let esc = self.peek().ok_or("unterminated escape")?;
                self.at += 1;
                out.push(match esc {
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'0' => 0,
                    other => other,
                });
            } else {
                out.push(b);
            }
        }
    }

    fn bare(&mut self) -> Result<PyValue, String> {
        let start = self.at;
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() || b"[](){},:".contains(&b) {
                break;
            }
            self.at += 1;
        }
        let word = &self.src[start..self.at];
        match word {
            "" => Err("expected a value".into()),
            "none" | "None" => Ok(PyValue::None),
            "true" | "True" => Ok(PyValue::Bool(true)),
            "false" | "False" => Ok(PyValue::Bool(false)),
            _ => {
                if let Ok(i) = word.parse::<i64>() {
                    return Ok(PyValue::Int(i));
                }
                if let Ok(f) = word.parse::<f64>() {
                    return Ok(PyValue::Real(f));
                }
                Err(format!(
                    "{word:?} is not a literal; quote it for a string, \
                     or use none/true/false/a number"
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(line: &str) -> Step {
        let steps = parse(line).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(steps.len(), 1, "{line:?}");
        steps.into_iter().next().expect("one step")
    }

    #[test]
    fn a_plain_call_with_no_arguments() {
        assert_eq!(
            one("call charUnboundMgr.GetCharacterSelectionData"),
            Step::Call {
                service: "charUnboundMgr".into(),
                method: "GetCharacterSelectionData".into(),
                args: Vec::new(),
            }
        );
    }

    #[test]
    fn arguments_may_be_separated_by_commas_or_spaces() {
        let spaces = one("call config.GetMultiOwnersEx [90000001]");
        let commas = one("call config.GetMultiOwnersEx, [90000001]");
        assert_eq!(spaces, commas);
        let Step::Call { args, .. } = spaces else {
            panic!("not a call")
        };
        assert_eq!(args, vec![PyValue::List(vec![PyValue::Int(90_000_001)])]);
    }

    #[test]
    fn every_literal_kind() {
        let Step::Call { args, .. } =
            one(r#"call x.y none true false 42 -1 1.5 "str" u"uni" [1, 2] (3,) {"k": 7}"#)
        else {
            panic!("not a call")
        };
        assert_eq!(
            args,
            vec![
                PyValue::None,
                PyValue::Bool(true),
                PyValue::Bool(false),
                PyValue::Int(42),
                PyValue::Int(-1),
                PyValue::Real(1.5),
                PyValue::Bytes(b"str".to_vec()),
                PyValue::Unicode("uni".into()),
                PyValue::List(vec![PyValue::Int(1), PyValue::Int(2)]),
                PyValue::Tuple(vec![PyValue::Int(3)]),
                PyValue::Dict(vec![(PyValue::Int(7), PyValue::Bytes(b"k".to_vec()))]),
            ]
        );
    }

    /// Dict entries are stored `(value, key)` because that is the wire order.
    /// Storing them the natural way round would encode every dict backwards
    /// and the mistake would only show up as a server-side `KeyError`.
    #[test]
    fn dict_entries_are_stored_in_wire_order() {
        let v = parse_value(r#"{"userid": 5}"#).expect("dict");
        assert_eq!(
            v,
            PyValue::Dict(vec![(PyValue::Int(5), PyValue::Bytes(b"userid".to_vec()))])
        );
        assert_eq!(
            v.dict_as_kv().expect("a dict"),
            vec![(&PyValue::Bytes(b"userid".to_vec()), &PyValue::Int(5))]
        );
    }

    #[test]
    fn bind_takes_its_params_as_one_literal() {
        assert_eq!(
            one("bind populationCap (90000001, 1) GetCharacterLoadSlot 90000001"),
            Step::Bind {
                service: "populationCap".into(),
                bind_params: PyValue::Tuple(vec![PyValue::Int(90_000_001), PyValue::Int(1)]),
                method: "GetCharacterLoadSlot".into(),
                args: vec![PyValue::Int(90_000_001)],
            }
        );
    }

    #[test]
    fn notes_drains_and_object_calls() {
        assert_eq!(
            one("note stage 4: market"),
            Step::Note("stage 4: market".into())
        );
        assert_eq!(one("drain 2500"), Step::Drain { millis: 2500 });
        assert_eq!(
            one("on populationCap MoveCharacterToNewSystem 1, 2, 3"),
            Step::On {
                label: "populationCap".into(),
                method: "MoveCharacterToNewSystem".into(),
                args: vec![PyValue::Int(1), PyValue::Int(2), PyValue::Int(3)],
            }
        );
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let steps = parse("# a comment\n\n  \ncall a.b   # trailing\ndrain 1\n").expect("parses");
        assert_eq!(steps.len(), 2);
    }

    /// A `#` inside a string is data. `split('#')` would eat the argument,
    /// and the failure would be a server-side one four layers away.
    #[test]
    fn a_hash_inside_a_string_is_not_a_comment() {
        let Step::Call { args, .. } = one(r#"call a.b "x#y""#) else {
            panic!("not a call")
        };
        assert_eq!(args, vec![PyValue::Bytes(b"x#y".to_vec())]);
    }

    #[test]
    fn errors_name_their_line() {
        let e = parse("call a.b\nwiggle\n").expect_err("unknown directive");
        assert_eq!(e.line, 2);
        assert!(e.message.contains("wiggle"), "{}", e.message);

        let e = parse("call nodot\n").expect_err("missing method");
        assert_eq!(e.line, 1);

        assert!(parse("call a.b [1, 2\n").is_err(), "unterminated list");
        assert!(
            parse("call a.b bare\n").is_err(),
            "bare word is not a literal"
        );
        assert!(parse("drain soon\n").is_err());
        assert!(parse("bind svc\n").is_err());
    }

    /// Escapes exist so a script can send a byte string with a NUL or a
    /// newline in it, which the handshake and several calls do.
    #[test]
    fn string_escapes() {
        assert_eq!(
            parse_value(r#""a\nb\0c\\d\"e""#).expect("escapes"),
            PyValue::Bytes(b"a\nb\0c\\d\"e".to_vec())
        );
    }
}
