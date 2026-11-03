//! Our own authored tables: `content/*.tsv`, read from `--authored DIR`
//! with a compiled-in fallback.
//!
//! W11 packet CP8, `docs/content/plan.md` §1.4 and §2. Class C data (plan
//! §2.1) is what **we invent**: policy and tuning tables such as
//! `content/population.tsv`. It lives in the repo, never in `out/content/`,
//! and never holds a client row.
//!
//! Each consumer compiles its own file in with `include_str!`, so the binary
//! always carries the shipped defaults, and asks a [`Dir`] for it:
//!
//! - a file found under the `--authored` directory **wins** over the
//!   compiled-in copy, so an owner can retune without a rebuild;
//! - a file the directory does not have falls back to the compiled-in copy;
//! - **the startup line names the source used for each file**
//!   ([`File::source_line`]), so nobody has to guess which copy is live.
//!
//! The format is `docs/content/schema.md` §1.1's authored form: UTF-8,
//! tab-separated; lines starting with `#` are comments and may appear
//! anywhere; the **header line is the first line that is neither empty nor a
//! comment**, compared byte for byte with the one the consumer pins from
//! `schema.md` §4. A header mismatch makes the whole file an error. A row
//! with the wrong number of fields, or a field its consumer cannot parse, is
//! **skipped and logged** with its file and line number.
//!
//! This module knows no file and no column: it is the reader every consumer
//! shares. Tests may parse a shipped file for well-formedness, never assert a
//! value in it (plan §1.4).
//!
//! ```
//! use eve_server::authored::{Dir, Source};
//!
//! const HEADER: &str = "key\tcount\twhy";
//! const SHIPPED: &str = "# invented example\nkey\tcount\twhy\nalpha\t2\t[A] invented\n";
//!
//! let file = Dir::compiled_only().load("example.tsv", SHIPPED, HEADER).unwrap();
//! assert_eq!(file.source, Source::Compiled);
//! let counts: Vec<(String, i64)> =
//!     file.parse_rows(|r| Ok((r.field(0).to_string(), r.i64(1)?)));
//! assert_eq!(counts, [("alpha".to_string(), 2)]);
//! ```

use std::fmt;
use std::path::{Path, PathBuf};

/// The default `--authored` directory: the repo's top-level `content/`,
/// relative to the working directory. `tools/up.sh` passes it explicitly.
pub const DEFAULT_DIR: &str = "content";

/// Where authored files are looked for on disk, if anywhere.
///
/// A plain value, not a process-wide setting (plan §1.5: no new
/// `OnceLock`): the binary builds one from `--authored` and hands it to
/// whatever builds the tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dir {
    root: Option<PathBuf>,
}

impl Default for Dir {
    /// [`DEFAULT_DIR`].
    fn default() -> Self {
        Self::new(DEFAULT_DIR)
    }
}

/// Which copy of a file was used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Read from this path under the `--authored` directory.
    Disk(PathBuf),
    /// The `include_str!` copy built into the binary.
    Compiled,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Disk(p) => write!(f, "disk {}", p.display()),
            Source::Compiled => f.write_str("compiled-in"),
        }
    }
}

/// One data row: its 1-based line number in the file, and its fields in
/// header order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// 1-based line number, for error lines.
    pub line: usize,
    fields: Vec<String>,
}

/// One authored file, parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct File {
    /// The path relative to the authored directory, for example
    /// `population.tsv` or `econ/hubs.tsv`.
    pub rel: String,
    /// Which copy was read.
    pub source: Source,
    /// Every well-formed row, in file order.
    pub rows: Vec<Row>,
    /// One line per row that was skipped, already printed to stdout.
    pub skipped: Vec<String>,
}

impl Dir {
    /// Look for files under `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: Some(root.into()),
        }
    }

    /// Never read the disk: every file is its compiled-in copy. For tests
    /// and for a binary started without a repo checkout beside it.
    pub fn compiled_only() -> Self {
        Self { root: None }
    }

    /// The directory searched, if any.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Read `rel`: from disk when the directory has it, else `compiled`.
    ///
    /// `header` is the file's header line from `schema.md` §4, without a
    /// line end. Prints the source line and one line per skipped row.
    ///
    /// # Errors
    ///
    /// A file that exists on disk but cannot be read (or is not UTF-8), and
    /// a header that is missing or differs from `header`. A shipped
    /// compiled-in copy with a wrong header is a bug its consumer's
    /// well-formedness test catches.
    pub fn load(&self, rel: &str, compiled: &str, header: &str) -> Result<File, String> {
        let file = self.read(rel, compiled, header)?;
        for line in file.lines() {
            crate::outln!("{line}");
        }
        Ok(file)
    }

    /// [`Dir::load`] without the printing: what the tests call.
    fn read(&self, rel: &str, compiled: &str, header: &str) -> Result<File, String> {
        let on_disk = match &self.root {
            Some(root) => {
                let path = root.join(rel);
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let text = String::from_utf8(bytes)
                            .map_err(|e| format!("authored {}: not UTF-8: {e}", path.display()))?;
                        Some((text, path))
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => return Err(format!("authored {}: {e}", path.display())),
                }
            }
            None => None,
        };
        let (text, source) = match on_disk {
            Some((text, path)) => (text, Source::Disk(path)),
            None => (compiled.to_string(), Source::Compiled),
        };
        let what = match &source {
            Source::Disk(p) => p.display().to_string(),
            Source::Compiled => format!("{rel} (compiled-in)"),
        };
        let (rows, skipped) = parse(&text, header).map_err(|e| format!("authored {what}: {e}"))?;
        let skipped = skipped
            .into_iter()
            .map(|s| format!("[authored] {what}: {s}"))
            .collect();
        Ok(File {
            rel: rel.to_string(),
            source,
            rows,
            skipped,
        })
    }
}

impl File {
    /// The startup line: which copy of this file is live, and how many rows
    /// it gave.
    pub fn source_line(&self) -> String {
        format!(
            "[authored] {}: {} ({} rows, {} skipped)",
            self.rel,
            self.source,
            self.rows.len(),
            self.skipped.len()
        )
    }

    /// The source line, then every skipped-row line: what [`Dir::load`]
    /// prints.
    pub fn lines(&self) -> Vec<String> {
        std::iter::once(self.source_line())
            .chain(self.skipped.iter().cloned())
            .collect()
    }

    /// Turn every row into a `T`; a row `f` rejects is skipped, printed and
    /// recorded in [`File::skipped`], like a row with the wrong field count.
    pub fn parse_rows<T>(&self, f: impl Fn(&Row) -> Result<T, String>) -> Vec<T> {
        self.parse_rows_logged(f).0
    }

    /// [`File::parse_rows`], returning the skip lines instead of only
    /// printing them.
    pub fn parse_rows_logged<T>(
        &self,
        f: impl Fn(&Row) -> Result<T, String>,
    ) -> (Vec<T>, Vec<String>) {
        let mut out = Vec::with_capacity(self.rows.len());
        let mut skipped = Vec::new();
        for row in &self.rows {
            match f(row) {
                Ok(t) => out.push(t),
                Err(e) => {
                    let line = format!(
                        "[authored] {} ({}): line {}: skipped: {e}",
                        self.rel, self.source, row.line
                    );
                    crate::outln!("{line}");
                    skipped.push(line);
                }
            }
        }
        (out, skipped)
    }
}

impl Row {
    /// Field `i` as written. Every row has exactly the header's field count,
    /// so an index below that count never panics.
    ///
    /// # Panics
    ///
    /// When `i` is not below the header's column count: a consumer bug.
    pub fn field(&self, i: usize) -> &str {
        &self.fields[i]
    }

    /// Every field, in header order.
    pub fn fields(&self) -> &[String] {
        &self.fields
    }

    /// Field `i` as an `i64` (`schema.md` §1.1).
    pub fn i64(&self, i: usize) -> Result<i64, String> {
        let v = self.field(i);
        v.parse()
            .map_err(|e| format!("column {} {v:?} is not an i64: {e}", i + 1))
    }

    /// Field `i` as an `f64`.
    pub fn f64(&self, i: usize) -> Result<f64, String> {
        let v = self.field(i);
        v.parse()
            .map_err(|e| format!("column {} {v:?} is not an f64: {e}", i + 1))
    }

    /// Field `i` as a `bool`: exactly `0` or `1`.
    pub fn bool(&self, i: usize) -> Result<bool, String> {
        match self.field(i) {
            "0" => Ok(false),
            "1" => Ok(true),
            v => Err(format!("column {} {v:?} is not 0 or 1", i + 1)),
        }
    }

    /// Field `i` as a `list<i64>`: `a,b,c`, and the empty field is the empty
    /// list.
    pub fn list_i64(&self, i: usize) -> Result<Vec<i64>, String> {
        let v = self.field(i);
        if v.is_empty() {
            return Ok(Vec::new());
        }
        v.split(',')
            .map(|x| {
                x.parse()
                    .map_err(|e| format!("column {} element {x:?} is not an i64: {e}", i + 1))
            })
            .collect()
    }
}

/// Parse an authored file's text against its pinned header line.
///
/// Returns the well-formed rows and one message per skipped row (a row whose
/// field count differs from the header's). A line end is `\n`, with an
/// optional `\r` before it. Empty and whitespace-only lines are ignored.
///
/// # Errors
///
/// No header line, or a header line that differs from `header`; the message
/// names the expected line and the line found (`schema.md` §1.1).
pub fn parse(text: &str, header: &str) -> Result<(Vec<Row>, Vec<String>), String> {
    let width = header.split('\t').count();
    let mut found_header = false;
    let mut rows = Vec::new();
    let mut skipped = Vec::new();
    for (i, raw) in text.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if !found_header {
            if line != header {
                return Err(format!(
                    "line {}: header mismatch: expected {header:?}, found {line:?}",
                    i + 1
                ));
            }
            found_header = true;
            continue;
        }
        let fields: Vec<String> = line.split('\t').map(str::to_string).collect();
        if fields.len() != width {
            skipped.push(format!(
                "line {}: skipped: {} fields, the header has {width}",
                i + 1,
                fields.len()
            ));
            continue;
        }
        rows.push(Row {
            line: i + 1,
            fields,
        });
    }
    if !found_header {
        return Err(format!("no header line: expected {header:?}"));
    }
    Ok((rows, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every row here is invented: this module knows no shipped file.
    const HEADER: &str = "key\tcount\tlist\twhy";
    const COMPILED: &str = "# [A] invented preamble\n\
        \n\
        key\tcount\tlist\twhy\n\
        # a comment between rows\n\
        alpha\t1\t1,2\t[A] invented\n\
        beta\t2\t\t[A] invented\n";

    /// A unique scratch directory under the system temp dir.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eve-authored-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Comments may come before the header and between rows; the header is
    /// the first non-empty, non-comment line; blank lines are ignored.
    #[test]
    fn header_and_comment_parsing() {
        let (rows, skipped) = parse(COMPILED, HEADER).unwrap();
        assert!(skipped.is_empty());
        let keys: Vec<&str> = rows.iter().map(|r| r.field(0)).collect();
        assert_eq!(keys, ["alpha", "beta"]);
        assert_eq!(rows[0].line, 5);
        assert_eq!(rows[0].list_i64(2).unwrap(), [1, 2]);
        assert!(rows[1].list_i64(2).unwrap().is_empty());

        // CRLF line ends parse the same.
        let crlf = COMPILED.replace('\n', "\r\n");
        assert_eq!(parse(&crlf, HEADER).unwrap().0, rows);
    }

    /// The header is compared byte for byte, and the error names both lines.
    #[test]
    fn a_wrong_or_missing_header_is_an_error() {
        let e = parse("# c\nkey\tcount\twhy\n", HEADER).unwrap_err();
        assert!(e.contains("header mismatch"), "{e}");
        assert!(e.contains(&format!("{HEADER:?}")), "{e}");
        assert!(e.contains("\"key\\tcount\\twhy\""), "{e}");
        // A `#` header is the bundle form, not the authored one.
        assert!(parse(&format!("#{HEADER}\n"), HEADER).is_err());
        assert!(parse("# only comments\n\n", HEADER).is_err());
    }

    /// A row with the wrong field count is skipped and logged with its line;
    /// the rows around it survive.
    #[test]
    fn a_malformed_row_is_skipped_and_logged() {
        let text = format!("{HEADER}\nalpha\t1\t\t[A] x\nshort\t1\nbeta\t2\t\t[A] x\n");
        let (rows, skipped) = parse(&text, HEADER).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("line 3"), "{}", skipped[0]);

        let file = Dir::compiled_only().read("bad.tsv", &text, HEADER).unwrap();
        assert_eq!(file.skipped.len(), 1);
        assert!(file.skipped[0].contains("bad.tsv"), "{}", file.skipped[0]);
        assert!(file.lines().iter().any(|l| l.contains("line 3")));
        assert!(file.source_line().contains("1 skipped"));
    }

    /// A field the consumer cannot parse skips that row, logged, too.
    #[test]
    fn a_row_the_consumer_rejects_is_skipped_and_logged() {
        let text = format!("{HEADER}\nalpha\t1\t\t[A] x\nbeta\tlots\t\t[A] x\n");
        let file = Dir::compiled_only()
            .read("typed.tsv", &text, HEADER)
            .unwrap();
        let (counts, skipped) = file.parse_rows_logged(|r| r.i64(1));
        assert_eq!(counts, [1]);
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("typed.tsv"), "{}", skipped[0]);
        assert!(skipped[0].contains("line 3"), "{}", skipped[0]);
    }

    /// A file under the `--authored` directory wins over the compiled-in
    /// copy, including in a subdirectory, and the source line says so.
    #[test]
    fn the_authored_file_wins_over_the_compiled_one() {
        let dir = scratch("wins");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let path = dir.join("sub/t.tsv");
        std::fs::write(&path, format!("{HEADER}\ngamma\t3\t\t[A] disk\n")).unwrap();

        let file = Dir::new(&dir).read("sub/t.tsv", COMPILED, HEADER).unwrap();
        assert_eq!(file.source, Source::Disk(path.clone()));
        let keys: Vec<&str> = file.rows.iter().map(|r| r.field(0)).collect();
        assert_eq!(keys, ["gamma"]);
        let line = file.source_line();
        assert!(line.contains("sub/t.tsv"), "{line}");
        assert!(line.contains(&format!("disk {}", path.display())), "{line}");

        // A bad header on disk is an error, never a silent fallback.
        std::fs::write(&path, "nope\n").unwrap();
        let e = Dir::new(&dir)
            .read("sub/t.tsv", COMPILED, HEADER)
            .unwrap_err();
        assert!(e.contains(&path.display().to_string()), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A file the directory lacks, or no directory at all, falls back to the
    /// compiled-in copy, and the source line says so.
    #[test]
    fn a_missing_file_falls_back_to_the_compiled_one() {
        let dir = scratch("missing");
        let compiled = parse(COMPILED, HEADER).unwrap().0;
        for d in [
            Dir::new(&dir),
            Dir::new(dir.join("absent")),
            Dir::compiled_only(),
        ] {
            let file = d.read("t.tsv", COMPILED, HEADER).unwrap();
            assert_eq!(file.source, Source::Compiled);
            assert_eq!(file.rows, compiled);
            let line = file.source_line();
            assert!(line.contains("t.tsv: compiled-in"), "{line}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The column helpers accept exactly `schema.md` §1.1's encodings.
    #[test]
    fn typed_fields_follow_the_schema_encodings() {
        let text = "a\tb\tc\td\n-3\t1e-05\t1\t7,-8\nx\t-3.0\t2\t7,\n";
        let (rows, _) = parse(text, "a\tb\tc\td").unwrap();
        assert_eq!(rows[0].i64(0), Ok(-3));
        assert_eq!(rows[0].f64(1), Ok(1e-05));
        assert_eq!(rows[0].bool(2), Ok(true));
        assert_eq!(rows[0].list_i64(3), Ok(vec![7, -8]));
        assert!(rows[1].i64(0).is_err());
        assert_eq!(rows[1].f64(1), Ok(-3.0));
        assert!(rows[1].bool(2).is_err());
        assert!(rows[1].list_i64(3).is_err());
    }
}
