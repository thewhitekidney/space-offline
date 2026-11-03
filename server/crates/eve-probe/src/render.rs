//! Turning a [`PyValue`] into something a person can read.
//!
//! Two different jobs, and conflating them is why transcripts are usually
//! useless:
//!
//! - [`value`] prints a value the way Python would, truncated. It answers
//!   "what came back".
//! - [`shape`] prints the value's *structure* — container kinds, row classes,
//!   column names and types, counts — with no data in it. It answers "what is
//!   this service's reply shaped like", which is the question a schema
//!   summary exists to answer and the only one that survives the run.
//!
//! Both are budgeted. A single `CallRsp` in `/tmp/evejs-session.pcap` is
//! several megabytes, and a renderer without a cap turns a transcript into
//! something no one will open.

use eve_marshal::{DbRow, DbRowDescriptor, DbValue, ObjectForm, PyValue};

/// How much of one value to print before giving up on it.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// Maximum nesting depth before collapsing to `...`.
    pub depth: usize,
    /// Maximum items printed per container.
    pub items: usize,
    /// Maximum bytes/characters printed per string.
    pub chars: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            depth: 6,
            items: 8,
            chars: 96,
        }
    }
}

impl Budget {
    /// A budget for a sample row: deeper and wider, because one row is the
    /// whole point of printing a sample at all.
    ///
    /// The depth matters more than it looks. A `CachedMethodCallResult`
    /// wrapping a substream wrapping a `Rowset` whose header is a
    /// `DBRowDescriptor` whose columns are `(name, type)` pairs is seven
    /// levels down, and at depth 8 the column names — the thing the whole
    /// summary exists for — print as `(..., ...)`.
    pub fn sample() -> Self {
        Budget {
            depth: 14,
            items: 64,
            chars: 160,
        }
    }
}

/// Render a value the way a Python `repr` would, truncated to `budget`.
pub fn value(v: &PyValue, budget: Budget) -> String {
    let mut out = String::new();
    write_value(v, budget, 0, &mut out);
    out
}

fn ellipsis(out: &mut String, shown: usize, total: usize) {
    if shown < total {
        out.push_str(&format!(", ... {} more", total - shown));
    }
}

fn write_value(v: &PyValue, b: Budget, depth: usize, out: &mut String) {
    if depth > b.depth {
        out.push_str("...");
        return;
    }
    match v {
        PyValue::None => out.push_str("None"),
        PyValue::Bool(x) => out.push_str(if *x { "True" } else { "False" }),
        PyValue::Int(i) | PyValue::Long(i) => out.push_str(&i.to_string()),
        PyValue::Real(f) => out.push_str(&format!("{f}")),
        PyValue::Bytes(s) => {
            out.push('\'');
            out.push_str(&escape(s, b.chars));
            out.push('\'');
        }
        PyValue::Unicode(s) => {
            out.push_str("u'");
            out.push_str(&escape(s.as_bytes(), b.chars));
            out.push('\'');
        }
        PyValue::Tuple(items) => {
            out.push('(');
            write_items(items, b, depth, out);
            out.push(')');
        }
        PyValue::List(items) => {
            out.push('[');
            write_items(items, b, depth, out);
            out.push(']');
        }
        PyValue::Dict(entries) => {
            out.push('{');
            let n = entries.len().min(b.items);
            for (i, (val, key)) in entries.iter().take(n).enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(key, b, depth + 1, out);
                out.push_str(": ");
                write_value(val, b, depth + 1, out);
            }
            ellipsis(out, n, entries.len());
            out.push('}');
        }
        PyValue::Callback(child) => {
            // A bound-object reference: the child is a nested marshal stream
            // of `(objectID, refID)`. Decode it, because an opaque blob here
            // hides the one thing worth seeing.
            match bound_object(child) {
                Some((id, refid)) => out.push_str(&format!("<bound {id} ref={refid}>")),
                None => {
                    out.push_str("<callback ");
                    write_value(child, b, depth + 1, out);
                    out.push('>');
                }
            }
        }
        PyValue::Instance { class, state } => {
            out.push('<');
            out.push_str(&String::from_utf8_lossy(class));
            out.push(' ');
            write_value(state, b, depth + 1, out);
            out.push('>');
        }
        PyValue::ObjectEx {
            form,
            header,
            list,
            dict,
        } => {
            let tag = match form {
                ObjectForm::Call => "obj",
                ObjectForm::New => "obj2",
            };
            out.push_str(&format!("<{tag} "));
            out.push_str(&class_of(header).unwrap_or_else(|| "?".into()));
            if let Some(args) = args_of(header) {
                out.push('(');
                write_items(args, b, depth, out);
                out.push(')');
            }
            if !list.is_empty() {
                out.push_str(&format!(" +{} appended", list.len()));
            }
            if !dict.is_empty() {
                out.push_str(&format!(" +{} keyed", dict.len()));
            }
            out.push('>');
        }
        PyValue::Global(name) => {
            out.push_str("<global ");
            out.push_str(&String::from_utf8_lossy(name));
            out.push('>');
        }
        PyValue::DbRow(row) => out.push_str(&dbrow(row, b)),
        PyValue::SubStream(bytes) => match super::decode_stream(bytes) {
            Ok(inner) => {
                out.push_str("<substream ");
                write_value(&inner, b, depth + 1, out);
                out.push('>');
            }
            Err(e) => out.push_str(&format!(
                "<substream {} bytes, undecodable: {e}>",
                bytes.len()
            )),
        },
    }
}

fn write_items(items: &[PyValue], b: Budget, depth: usize, out: &mut String) {
    let n = items.len().min(b.items);
    for (i, item) in items.iter().take(n).enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_value(item, b, depth + 1, out);
    }
    ellipsis(out, n, items.len());
}

fn escape(bytes: &[u8], cap: usize) -> String {
    let mut s = String::new();
    for &byte in bytes.iter().take(cap) {
        match byte {
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            b'\t' => s.push_str("\\t"),
            b'\'' => s.push_str("\\'"),
            b'\\' => s.push_str("\\\\"),
            0x20..=0x7E => s.push(byte as char),
            other => s.push_str(&format!("\\x{other:02x}")),
        }
    }
    if bytes.len() > cap {
        s.push_str(&format!("...+{}", bytes.len() - cap));
    }
    s
}

fn dbrow(row: &DbRow, b: Budget) -> String {
    let mut s = String::from("DBRow(");
    for (i, (col, val)) in row
        .descriptor()
        .columns()
        .iter()
        .zip(row.values())
        .take(b.items)
        .enumerate()
    {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&String::from_utf8_lossy(&col.name));
        s.push('=');
        s.push_str(&dbvalue(val, b));
    }
    let shown = row.values().len().min(b.items);
    if shown < row.values().len() {
        s.push_str(&format!(", ... {} more", row.values().len() - shown));
    }
    s.push(')');
    s
}

fn dbvalue(v: &DbValue, b: Budget) -> String {
    match v {
        DbValue::Null => "None".into(),
        DbValue::Bool(x) => if *x { "True" } else { "False" }.into(),
        DbValue::Int(i) => i.to_string(),
        DbValue::Real(f) => format!("{f}"),
        DbValue::Object(o) => value(o, b),
    }
}

/// The dotted class name of an `ObjectEx` header, if it has one.
///
/// `0x22`'s header is `(callable, args)`; `0x23`'s is `((cls, *args), state?)`
/// — so the class is one level deeper. Both forms appear in replies, which is
/// why this is not a one-liner.
pub fn class_of(header: &PyValue) -> Option<String> {
    let PyValue::Tuple(h) = header else {
        return None;
    };
    let callable = match h.first()? {
        PyValue::Global(name) => return Some(String::from_utf8_lossy(name).into_owned()),
        PyValue::Tuple(inner) => inner.first()?,
        _ => return None,
    };
    match callable {
        PyValue::Global(name) => Some(String::from_utf8_lossy(name).into_owned()),
        _ => None,
    }
}

/// The argument tuple of an `ObjectEx` header, for either form.
pub fn args_of(header: &PyValue) -> Option<&[PyValue]> {
    let PyValue::Tuple(h) = header else {
        return None;
    };
    match h.first()? {
        // 0x22: (callable, args)
        PyValue::Global(_) => match h.get(1) {
            Some(PyValue::Tuple(args)) => Some(args),
            _ => None,
        },
        // 0x23: ((cls, *args), state?) -- the class is args[0], so drop it.
        PyValue::Tuple(inner) => Some(&inner[1..]),
        _ => None,
    }
}

/// Decode a `0x19` callback child into `(objectID, refID)`.
///
/// The child is a byte string holding a whole second marshal stream, which is
/// how a bound-object reference travels (`docs/reference/bound-objects.md`). A probe has
/// to follow these to call methods on a bound object, so decoding them in the
/// renderer is not cosmetic: it is how the object id reaches the transcript.
pub fn bound_object(child: &PyValue) -> Option<(String, i64)> {
    let bytes = match child {
        PyValue::Bytes(b) | PyValue::SubStream(b) => b,
        _ => return None,
    };
    let inner = super::decode_stream(bytes).ok()?;
    let PyValue::Tuple(t) = inner else {
        return None;
    };
    if t.len() != 2 {
        return None;
    }
    let id = match &t[0] {
        PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        PyValue::Unicode(s) => s.clone(),
        _ => return None,
    };
    let refid = match &t[1] {
        PyValue::Int(i) => *i,
        _ => 0,
    };
    Some((id, refid))
}

/// The structure of a value, with no data in it.
///
/// This is what a schema summary is made of. It names containers, counts
/// them, and for anything row-shaped it names the columns and their types:
///
/// ```text
/// tuple(4)[
///   list(1)[ <utillib.KeyVal {characterID, characterName, ...}> ],
///   tuple(2)[None, None],
///   list(0),
///   list(0)]
/// ```
pub fn shape(v: &PyValue) -> String {
    let mut out = String::new();
    write_shape(v, 0, &mut out);
    out
}

const SHAPE_DEPTH: usize = 7;
const SHAPE_ITEMS: usize = 6;
/// EvEJS advertises 143 services in one dict, so this has to clear that.
const DICT_KEYS: usize = 400;

fn write_shape(v: &PyValue, depth: usize, out: &mut String) {
    if depth > SHAPE_DEPTH {
        out.push_str("...");
        return;
    }
    match v {
        PyValue::None => out.push_str("None"),
        PyValue::Bool(_) => out.push_str("bool"),
        PyValue::Int(_) => out.push_str("int"),
        PyValue::Long(_) => out.push_str("long"),
        PyValue::Real(_) => out.push_str("real"),
        PyValue::Bytes(_) => out.push_str("str"),
        PyValue::Unicode(_) => out.push_str("unicode"),
        PyValue::Tuple(items) => {
            out.push_str(&format!("tuple({})", items.len()));
            write_shape_items(items, depth, out);
        }
        PyValue::List(items) => {
            out.push_str(&format!("list({})", items.len()));
            write_shape_items(items, depth, out);
        }
        PyValue::Dict(entries) => {
            out.push_str(&format!("dict({})", entries.len()));
            // Dict keys are not truncated as hard as list items are: a
            // service advertisement, a config blob or a session dict IS its
            // key set, and a summary that stops at six of them is useless.
            let keys: Vec<String> = entries
                .iter()
                .take(DICT_KEYS)
                .map(|(_, k)| match k {
                    PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                    PyValue::Unicode(s) => s.clone(),
                    other => shape(other),
                })
                .collect();
            if !keys.is_empty() {
                out.push_str(&format!("{{{}}}", keys.join(", ")));
            }
        }
        PyValue::Callback(child) => match bound_object(child) {
            Some((id, _)) => out.push_str(&format!("<bound-object {id}>")),
            None => out.push_str("<callback>"),
        },
        PyValue::Instance { class, state } => {
            out.push_str(&format!("<{} ", String::from_utf8_lossy(class)));
            write_shape(state, depth + 1, out);
            out.push('>');
        }
        PyValue::ObjectEx {
            header, list, dict, ..
        } => {
            let class = class_of(header).unwrap_or_else(|| "?".into());
            out.push_str(&format!("<{class}"));
            if let Some(cols) = columns_of(v) {
                out.push_str(&format!(" cols[{}]", cols.join(", ")));
            }
            if let Some(args) = args_of(header) {
                // The descriptor is already reported as `cols`, so skip it
                // and show what the remaining arguments are shaped like.
                let interesting: Vec<&PyValue> = args
                    .iter()
                    .filter(|a| DbRowDescriptor::from_pyvalue(a).is_err())
                    .collect();
                if !interesting.is_empty() {
                    out.push_str(" args(");
                    for (i, a) in interesting.iter().take(SHAPE_ITEMS).enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        write_shape(a, depth + 1, out);
                    }
                    out.push(')');
                }
            }
            if !list.is_empty() {
                out.push_str(&format!(" +{}appended", list.len()));
            }
            if !dict.is_empty() {
                out.push_str(&format!(" +{}keyed", dict.len()));
            }
            out.push('>');
        }
        PyValue::Global(name) => {
            out.push_str(&format!("<global {}>", String::from_utf8_lossy(name)))
        }
        PyValue::DbRow(row) => {
            let cols: Vec<String> = row
                .descriptor()
                .columns()
                .iter()
                .map(|c| format!("{}:{:?}", String::from_utf8_lossy(&c.name), c.db_type))
                .collect();
            out.push_str(&format!("DBRow[{}]", cols.join(", ")));
        }
        PyValue::SubStream(bytes) => match super::decode_stream(bytes) {
            Ok(inner) => {
                out.push_str("substream<");
                write_shape(&inner, depth + 1, out);
                out.push('>');
            }
            Err(_) => out.push_str("substream<undecodable>"),
        },
    }
}

fn write_shape_items(items: &[PyValue], depth: usize, out: &mut String) {
    if items.is_empty() {
        return;
    }
    // A homogeneous container is described once rather than n times: a
    // 2,000-row rowset whose shape is printed 2,000 times says nothing.
    let first = shape(&items[0]);
    if items.len() > 1 && items.iter().all(|i| shape(i) == first) {
        out.push_str(&format!("[{first} x{}]", items.len()));
        return;
    }
    out.push('[');
    let n = items.len().min(SHAPE_ITEMS);
    for (i, item) in items.iter().take(n).enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_shape(item, depth + 1, out);
    }
    ellipsis(out, n, items.len());
    out.push(']');
}

/// `name:TYPE` for every column of a value that carries a row descriptor.
///
/// Covers the three places a descriptor hides: a `blue.DBRowDescriptor`
/// itself, any `ObjectEx` whose arguments contain one (every `CRowset`,
/// `CIndexedRowset`, `CFilterRowset` and `Row`), and a packed `DBRow`.
pub fn columns_of(v: &PyValue) -> Option<Vec<String>> {
    let named = |d: &DbRowDescriptor| {
        d.columns()
            .iter()
            .map(|c| format!("{}:{:?}", String::from_utf8_lossy(&c.name), c.db_type))
            .collect::<Vec<_>>()
    };
    match v {
        PyValue::DbRow(row) => Some(named(row.descriptor())),
        PyValue::ObjectEx { header, .. } => {
            if let Ok(d) = DbRowDescriptor::from_pyvalue(v) {
                return Some(named(&d));
            }
            let args = args_of(header)?;
            for a in args {
                if let Ok(d) = DbRowDescriptor::from_pyvalue(a) {
                    return Some(named(&d));
                }
                // `Row(header, line)` and the `(header, data)` tuple form
                // carry a plain list of column names instead.
                if let Some(names) = plain_header(a) {
                    return Some(names);
                }
            }
            None
        }
        PyValue::Tuple(t) if t.len() == 2 => plain_header(&t[0]),
        _ => None,
    }
}

/// A `(header, data)` reply's header: a list or tuple of column-name strings.
fn plain_header(v: &PyValue) -> Option<Vec<String>> {
    let items = match v {
        PyValue::List(i) | PyValue::Tuple(i) => i,
        _ => return None,
    };
    if items.is_empty() {
        return None;
    }
    let mut names = Vec::with_capacity(items.len());
    for i in items {
        match i {
            PyValue::Bytes(b) => names.push(String::from_utf8_lossy(b).into_owned()),
            PyValue::Unicode(s) => names.push(s.clone()),
            _ => return None,
        }
    }
    Some(names)
}

/// How many rows a container holds, for a schema line's row count.
pub fn row_count(v: &PyValue) -> Option<usize> {
    match v {
        PyValue::List(items) => Some(items.len()),
        PyValue::Dict(entries) => Some(entries.len()),
        PyValue::ObjectEx { header, list, .. } => {
            if !list.is_empty() {
                return Some(list.len());
            }
            let args = args_of(header)?;
            args.iter().find_map(|a| match a {
                PyValue::List(items) => Some(items.len()),
                PyValue::Dict(entries) => Some(entries.len()),
                _ => None,
            })
        }
        _ => None,
    }
}

/// The first row of a container, for a schema line's sample.
pub fn first_row(v: &PyValue) -> Option<&PyValue> {
    match v {
        PyValue::List(items) => items.first(),
        PyValue::ObjectEx { header, list, .. } => {
            if let Some(first) = list.first() {
                return Some(first);
            }
            let args = args_of(header)?;
            args.iter().find_map(|a| match a {
                PyValue::List(items) => items.first(),
                _ => None,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_marshal::{DbType, Writer, save, write_empty_header};

    fn stream(v: &PyValue) -> Vec<u8> {
        let mut w = Writer::new();
        write_empty_header(&mut w);
        save(v, &mut w);
        w.into_bytes()
    }

    #[test]
    fn scalars_read_like_python() {
        let b = Budget::default();
        assert_eq!(value(&PyValue::None, b), "None");
        assert_eq!(value(&PyValue::Bool(true), b), "True");
        assert_eq!(value(&PyValue::Int(-7), b), "-7");
        assert_eq!(value(&PyValue::Bytes(b"hi".to_vec()), b), "'hi'");
        assert_eq!(value(&PyValue::Unicode("hi".into()), b), "u'hi'");
    }

    #[test]
    fn non_printable_bytes_are_escaped_not_dropped() {
        let v = PyValue::Bytes(vec![0x00, 0x41, 0x0A, 0xFF]);
        assert_eq!(value(&v, Budget::default()), "'\\x00A\\n\\xff'");
    }

    /// The budget is the difference between a transcript and a core dump.
    #[test]
    fn containers_and_strings_are_truncated() {
        let big = PyValue::List((0..100).map(PyValue::Int).collect());
        let rendered = value(&big, Budget::default());
        assert!(rendered.ends_with("... 92 more]"), "{rendered}");
        let long = PyValue::Bytes(vec![b'x'; 500]);
        assert!(value(&long, Budget::default()).contains("...+404"));
    }

    /// Depth is capped, so a self-similar structure cannot run away.
    #[test]
    fn depth_is_capped() {
        let mut v = PyValue::Int(1);
        for _ in 0..40 {
            v = PyValue::Tuple(vec![v]);
        }
        let rendered = value(&v, Budget::default());
        assert!(rendered.contains("..."), "{rendered}");
        assert!(rendered.len() < 200, "len {}", rendered.len());
    }

    /// Dict entries are `(value, key)` on the wire. Printing them the other
    /// way round is the easiest mistake in this format and produces a
    /// transcript that looks fine and reads backwards.
    #[test]
    fn dict_entries_print_key_first() {
        let d = PyValue::Dict(vec![(PyValue::Int(5), PyValue::Bytes(b"userid".to_vec()))]);
        assert_eq!(value(&d, Budget::default()), "{'userid': 5}");
        assert_eq!(shape(&d), "dict(1){userid}");
    }

    /// A bound-object reference is the thing a probe must notice, so it has
    /// to survive rendering as an id rather than as an opaque blob.
    #[test]
    fn a_bound_object_reference_renders_as_its_id() {
        let inner = stream(&PyValue::Tuple(vec![
            PyValue::Bytes(b"N=99:3".to_vec()),
            PyValue::Int(12345),
        ]));
        let v = PyValue::Callback(Box::new(PyValue::Bytes(inner)));
        assert_eq!(value(&v, Budget::default()), "<bound N=99:3 ref=12345>");
        assert_eq!(shape(&v), "<bound-object N=99:3>");
        assert_eq!(
            bound_object(&PyValue::Bytes(b"not a stream".to_vec())),
            None
        );
    }

    /// A descriptor's column names and types are the deliverable of this
    /// lane, so extract them from each of the three places they hide.
    #[test]
    fn columns_come_out_of_a_descriptor_a_rowset_and_a_packed_row() {
        let desc = DbRowDescriptor::from_pairs(&[("itemID", DbType::I4), ("name", DbType::WStr)])
            .expect("descriptor");
        assert_eq!(
            columns_of(&desc.to_pyvalue()),
            Some(vec!["itemID:I4".into(), "name:WStr".into()])
        );

        let mut rowset = eve_rowset::CRowset::new(desc.clone());
        rowset
            .push_values(vec![
                DbValue::Int(42),
                DbValue::Object(PyValue::Unicode("a".into())),
            ])
            .expect("row");
        let as_value = rowset.to_pyvalue();
        assert_eq!(
            columns_of(&as_value),
            Some(vec!["itemID:I4".into(), "name:WStr".into()])
        );
        assert_eq!(row_count(&as_value), Some(1));

        let row =
            DbRow::new(desc, vec![DbValue::Int(1), DbValue::Object(PyValue::None)]).expect("row");
        assert_eq!(
            columns_of(&PyValue::DbRow(Box::new(row))),
            Some(vec!["itemID:I4".into(), "name:WStr".into()])
        );
    }

    /// The `(header, data)` tuple that `cfg.Recordset` accepts names its
    /// columns as plain strings. It is the single most common reply shape in
    /// the capture corpus, so it must not read as `tuple(2)`.
    #[test]
    fn the_header_data_tuple_names_its_columns() {
        let v = PyValue::Tuple(vec![
            PyValue::List(vec![
                PyValue::Bytes(b"ownerID".to_vec()),
                PyValue::Bytes(b"ownerName".to_vec()),
            ]),
            PyValue::List(vec![PyValue::List(vec![
                PyValue::Int(90000001),
                PyValue::Unicode("Capsuleer".into()),
            ])]),
        ]);
        assert_eq!(
            columns_of(&v),
            Some(vec!["ownerID".into(), "ownerName".into()])
        );
    }

    /// A long homogeneous list collapses to one shape and a count, which is
    /// what makes a schema summary readable at all.
    #[test]
    fn a_homogeneous_list_is_described_once() {
        let rows = PyValue::List(
            (0..500)
                .map(|i| PyValue::Tuple(vec![PyValue::Int(i), PyValue::Unicode("x".into())]))
                .collect(),
        );
        assert_eq!(shape(&rows), "list(500)[tuple(2)[int, unicode] x500]");
    }

    /// An undecodable substream must report itself rather than vanish or
    /// panic -- it is a real case (a truncated or compressed inner stream).
    #[test]
    fn an_undecodable_substream_says_so() {
        let v = PyValue::SubStream(vec![0xFF, 0xFF]);
        assert!(value(&v, Budget::default()).contains("undecodable"));
        assert_eq!(shape(&v), "substream<undecodable>");
    }
}
