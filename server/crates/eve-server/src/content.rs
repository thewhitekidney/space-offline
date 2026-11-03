//! Loaded data, as one typed map: `TypeId -> table`.
//!
//! W10.5 seam S7(b), `docs/content/plan.md` §1.5. Each data module defines
//! its own table type, with a `load(&Sources) -> Result<Table, String>` and
//! a `fixture() -> Table` made of **invented** rows, and registers it with
//! one [`TableSpec`] row in [`crate::content_tables::TABLES`]. Code reads a
//! table with [`Content::get`]; the `Registry` holds one `Arc<Content>`,
//! reached through `reg.content()`.
//!
//! [`load`] calls every table's `load`, **with or without a bundle** (IP1,
//! amendment CA1): a client-data table with no bundle returns its fixture,
//! and an authored table reads [`Sources::authored`] either way. A table
//! that cannot load returns an error naming its file, and [`load`] turns it
//! into a startup error naming the table, never a panic. Each table prints
//! one startup line naming its source (CA4, [`Origin`]).
//!
//! [`Content::fixture`] is every registered table's fixture, and it is what
//! `Registry::new` uses, so every test runs on invented rows with no setup.
//! **The bundle is needed to play, never to test.** A test that needs other
//! rows calls the pure function with its own table, or builds a `Content`
//! with [`Content::insert`] and hands it to `Registry::with_content`.
//!
//! No new `OnceLock`: this is where the existing ones move (CP58-CP60).

use crate::authored;
use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One table, boxed. What [`TableSpec`]'s constructors return.
pub type Boxed = Box<dyn Any + Send + Sync>;

/// An extracted content bundle on disk (`out/content/`, plan §1.2).
///
/// Only the directory: [`Bundle::open`] checks the manifest before handing
/// one out, and returns the parsed [`Manifest`] beside it. A table's `load`
/// reads its files with [`Bundle::data_lines`] or through [`Bundle::path`].
#[derive(Debug, Clone)]
pub struct Bundle {
    /// The bundle's root, the directory holding `MANIFEST.tsv`.
    pub dir: PathBuf,
}

/// The manifest's file name, at the bundle's root (`schema.md` §1.2).
pub const MANIFEST: &str = "MANIFEST.tsv";

/// The one manifest `format` major this server reads (`schema.md` §1.2: a
/// grammar or encoding change bumps it; a header change bumps only the
/// minor, which the per-file header check catches instead).
pub const FORMAT_MAJOR: u32 = 1;

/// The five phase-1 files the existing loaders read, each overridable by its
/// old flag (plan §1.3).
pub const CELESTIALS: &str = "universe/celestials.tsv";
/// `--belt-rocks`' file in the bundle.
pub const ROCKS: &str = "belts/rocks.tsv";
/// `--planets`' file in the bundle.
pub const PI_STATICS: &str = "planets/pi-statics.tsv";
/// `--typedata`' file in the bundle.
pub const TYPEDATA: &str = "types/typedata.tsv";
/// `--skills`' file in the bundle.
pub const SKILLS: &str = "types/skills.tsv";

/// Printed at startup, and again on every login, by a server running with
/// no bundle (plan §1.5).
pub const NO_BUNDLE: &str = "[content] NO BUNDLE: running on invented fixture rows; \
                             use tools/up.sh to play";

/// One `file` record of the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRecord {
    /// Relative to the bundle root, `/`-separated.
    pub path: String,
    /// Data lines: every non-empty line not starting with `#`.
    pub rows: u64,
    /// The writer's sha256. Not checked at startup (C1 §5.4: the row count
    /// is, for speed) **[A]**: falsified by a corrupted file that keeps its
    /// line count and still loads. `up.sh` re-extracts on a source change.
    pub sha256: String,
    /// The `res:` paths it was made from.
    pub sources: Vec<String>,
}

/// `MANIFEST.tsv`, parsed (`docs/content/schema.md` §1.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// `(MAJOR, MINOR)`.
    pub format: (u32, u32),
    /// The client build the bundle was extracted from.
    pub client_build: i64,
    /// The extractor hash (sha256 hex over `tools/content*.py` and readers).
    pub extractor: String,
    /// `all`, or the `--systems` list.
    pub systems: String,
    /// Format 1.1's `data_hash` (`schema.md` §1.2): sha256 over the `file`
    /// records sorted by path, so it names the data and not the build that
    /// was read. `None` for a 1.0 bundle. [`Manifest::parse`] checks it
    /// against the records.
    pub data_hash: Option<String>,
    /// Every `file` record, in write order.
    pub files: Vec<FileRecord>,
    /// Every `input` record: `(relpath under content/, sha256)`.
    pub inputs: Vec<(String, String)>,
    /// Every `source` record: `(res path, md5)`.
    pub sources: Vec<(String, String)>,
}

impl Manifest {
    /// Parse the manifest's text. Checks the grammar only; [`Bundle::open`]
    /// checks it against the build and the files.
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let mut format = None;
        let mut client_build = None;
        let mut extractor = None;
        let mut systems = None;
        let mut data_hash = None;
        let mut files = Vec::new();
        let mut inputs = Vec::new();
        let mut sources = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let n = i + 1;
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            let want = |k: usize| -> Result<(), String> {
                if f.len() == k {
                    Ok(())
                } else {
                    Err(format!(
                        "line {n}: '{}' needs {} fields, has {}",
                        f[0],
                        k - 1,
                        f.len() - 1
                    ))
                }
            };
            let once = |slot: &mut Option<String>, v: &str| -> Result<(), String> {
                if slot.replace(v.to_owned()).is_some() {
                    return Err(format!("line {n}: a second '{}' record", f[0]));
                }
                Ok(())
            };
            match f[0] {
                "format" => {
                    want(2)?;
                    once(&mut format, f[1])?;
                }
                "client_build" => {
                    want(2)?;
                    once(&mut client_build, f[1])?;
                }
                "extractor" => {
                    want(2)?;
                    once(&mut extractor, f[1])?;
                }
                "systems" => {
                    want(2)?;
                    once(&mut systems, f[1])?;
                }
                "data_hash" => {
                    want(2)?;
                    once(&mut data_hash, f[1])?;
                }
                "file" => {
                    // `sources` may be empty, and a writer may then drop the
                    // trailing field.
                    if f.len() != 4 && f.len() != 5 {
                        want(5)?;
                    }
                    let path = relpath(f[1]).map_err(|e| format!("line {n}: {e}"))?;
                    let rows = f[2]
                        .parse()
                        .map_err(|e| format!("line {n}: rows '{}': {e}", f[2]))?;
                    let sources = match f.get(4) {
                        Some(s) if !s.is_empty() => s.split(',').map(str::to_owned).collect(),
                        _ => Vec::new(),
                    };
                    if files.iter().any(|r: &FileRecord| r.path == path) {
                        return Err(format!("line {n}: {path} is listed twice"));
                    }
                    files.push(FileRecord {
                        path,
                        rows,
                        sha256: f[3].to_owned(),
                        sources,
                    });
                }
                "input" => {
                    want(3)?;
                    inputs.push((f[1].to_owned(), f[2].to_owned()));
                }
                "source" => {
                    want(3)?;
                    sources.push((f[1].to_owned(), f[2].to_owned()));
                }
                other => return Err(format!("line {n}: unknown record kind '{other}'")),
            }
        }
        let missing = |k: &str| format!("no '{k}' record");
        let format_s = format.ok_or_else(|| missing("format"))?;
        let format = format_s
            .split_once('.')
            .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
            .ok_or_else(|| format!("format '{format_s}': expected MAJOR.MINOR"))?;
        let build_s = client_build.ok_or_else(|| missing("client_build"))?;
        let client_build = build_s
            .parse()
            .map_err(|e| format!("client_build '{build_s}': {e}"))?;
        if let Some(h) = &data_hash {
            let want = data_hash_of(&files);
            if *h != want {
                return Err(format!(
                    "data_hash {h} does not match its file records ({want})"
                ));
            }
        }
        Ok(Manifest {
            format,
            client_build,
            extractor: extractor.ok_or_else(|| missing("extractor"))?,
            systems: systems.ok_or_else(|| missing("systems"))?,
            data_hash,
            files,
            inputs,
            sources,
        })
    }

    /// The startup line (plan §1.3): `format`, `client_build`, `extractor`.
    pub fn startup_line(&self, dir: &Path) -> String {
        format!(
            "[content] bundle {}: format {}.{}, client_build {}, extractor {}, systems {}, \
             data_hash {}, {} files",
            dir.display(),
            self.format.0,
            self.format.1,
            self.client_build,
            self.extractor,
            self.systems,
            self.data_hash
                .as_deref()
                .map_or("none", |h| &h[..h.len().min(8)]),
            self.files.len()
        )
    }

    /// The record for `path`, if the bundle lists it.
    pub fn file(&self, path: &str) -> Option<&FileRecord> {
        self.files.iter().find(|r| r.path == path)
    }
}

/// The `data_hash` of a set of `file` records (`schema.md` §1.2): sha256 over
/// one `path<TAB>sha256<LF>` line per record, sorted by path. The same rule
/// as `tools/content.py`'s `data_hash`.
pub fn data_hash_of(files: &[FileRecord]) -> String {
    let mut recs: Vec<(&str, &str)> = files
        .iter()
        .map(|r| (r.path.as_str(), r.sha256.as_str()))
        .collect();
    recs.sort_unstable();
    let mut text = String::new();
    for (p, h) in recs {
        text.push_str(p);
        text.push('\t');
        text.push_str(h);
        text.push('\n');
    }
    crate::accounts::sha256(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A manifest path, refused when it could leave the bundle.
fn relpath(p: &str) -> Result<String, String> {
    let bad = p.is_empty()
        || p.starts_with('/')
        || p.contains('\\')
        || p.split('/').any(|c| c.is_empty() || c == "." || c == "..");
    if bad {
        return Err(format!("file path '{p}' is not a plain relative path"));
    }
    Ok(p.to_owned())
}

/// The manifest's row count of a file's text: every non-empty line that
/// does not start with `#` (`schema.md` §1.2, which covers both the
/// headed files and the kind-tagged ones).
fn count_rows(text: &str) -> u64 {
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .count() as u64
}

/// Why a bundle's data may serve the accepted builds (BA-4): its
/// `client_build` is one of them, or its `data_hash` is one the registry
/// proved for one of them (`hash_of` is `eve_marshal::build::entry`'s
/// `data_hash`, a parameter so a test can register an invented one).
/// `None` when neither holds.
fn bundle_accepted(
    m: &Manifest,
    accepted: &[u32],
    hash_of: &dyn Fn(u32) -> Option<&'static str>,
) -> Option<String> {
    if accepted.iter().any(|b| i64::from(*b) == m.client_build) {
        return Some(format!("client_build {} is accepted", m.client_build));
    }
    let h = m.data_hash.as_deref()?;
    accepted
        .iter()
        .find(|b| hash_of(**b) == Some(h))
        .map(|b| format!("data_hash equals build {b}'s registered one"))
}

impl Bundle {
    /// [`Bundle::open_accepting`] the registry's default set
    /// (`eve_marshal::build::accepted`: every `pinned` or `equivalent`
    /// build). For the tools and bots, which have no `--client-build`.
    pub fn open(dir: &Path) -> Result<(Bundle, Manifest), String> {
        Self::open_accepting(dir, &eve_marshal::build::accepted())
    }

    /// Open `dir`, refusing a bundle this server cannot trust (plan §1.3,
    /// C1 §5.4): a `format` major other than [`FORMAT_MAJOR`], a bundle
    /// extracted from a build outside `accepted` whose `data_hash` is not
    /// registered for one inside it (BA-4), and a listed file that is
    /// missing or has the wrong row count. Every error names `dir`, and a
    /// file error names the file.
    pub fn open_accepting(dir: &Path, accepted: &[u32]) -> Result<(Bundle, Manifest), String> {
        let at = |e: String| format!("--content {}: {e}", dir.display());
        let mpath = dir.join(MANIFEST);
        let text = std::fs::read_to_string(&mpath).map_err(|e| {
            at(format!(
                "{MANIFEST}: {e}. Extract one with tools/up.sh (tools/content.py)"
            ))
        })?;
        let m = Manifest::parse(&text).map_err(|e| at(format!("{MANIFEST}: {e}")))?;
        if m.format.0 != FORMAT_MAJOR {
            return Err(at(format!(
                "format {}.{}: this server reads format {FORMAT_MAJOR}.x. Re-extract \
                 with tools/up.sh",
                m.format.0, m.format.1
            )));
        }
        let hash_of = |b: u32| eve_marshal::build::entry(b).and_then(|e| e.data_hash);
        if bundle_accepted(&m, accepted, &hash_of).is_none() {
            return Err(at(format!(
                "client_build {}: not one of the accepted client builds ({}), and \
                 its data_hash ({}) is not registered for any of them. Extract from \
                 an accepted client, or prove the data equal (tools/content.py \
                 --check --against) and register it",
                m.client_build,
                accepted
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                m.data_hash.as_deref().unwrap_or("none: format 1.0")
            )));
        }
        for rec in &m.files {
            let p = dir.join(&rec.path);
            let text = std::fs::read_to_string(&p)
                .map_err(|e| at(format!("{} is listed but unreadable: {e}", rec.path)))?;
            let rows = count_rows(&text);
            if rows != rec.rows {
                return Err(at(format!(
                    "{} has {rows} rows; the manifest says {}. Re-extract with \
                     tools/up.sh",
                    rec.path, rec.rows
                )));
            }
        }
        Ok((
            Bundle {
                dir: dir.to_path_buf(),
            },
            m,
        ))
    }

    /// `rel` under the bundle, if that file exists.
    pub fn path(&self, rel: &str) -> Option<PathBuf> {
        let p = self.dir.join(rel);
        p.is_file().then_some(p)
    }

    /// The data lines of an optional file with a header line
    /// (`schema.md` §1.1): `Ok(vec![])` when the file is absent, an error
    /// naming the file when line 1 is not exactly `header`, and otherwise
    /// every later non-empty line that does not start with `#`.
    ///
    /// `header` is the line as `schema.md` prints it, `#` included.
    pub fn data_lines(&self, rel: &str, header: &str) -> Result<Vec<String>, String> {
        let Some(p) = self.path(rel) else {
            return Ok(Vec::new());
        };
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let mut lines = text.lines();
        let found = lines.next().unwrap_or("");
        if found != header {
            return Err(format!(
                "{}: header is\n  {found}\nexpected\n  {header}\nRe-extract with tools/up.sh",
                p.display()
            ));
        }
        Ok(lines
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_owned)
            .collect())
    }
}

/// Where one of the five phase-1 files comes from (plan §1.3): the explicit
/// flag wins, then the bundle's file when it has one, then nothing (the
/// server's baked fallback).
pub fn pick(flag: Option<&Path>, bundle: Option<&Bundle>, rel: &str) -> Option<PathBuf> {
    match flag {
        Some(p) => Some(p.to_path_buf()),
        None => bundle.and_then(|b| b.path(rel)),
    }
}

/// What the server starts on: a checked bundle (or none) and its tables.
pub struct Loaded {
    /// `None` without `--content`.
    pub bundle: Option<Bundle>,
    /// Every registered table.
    pub content: Content,
    /// The lines to print at startup: the bundle's (or [`NO_BUNDLE`]), then
    /// one per table naming its source (CA4).
    pub lines: Vec<String>,
}

/// [`load_for`] with the registry's default accepted builds.
pub fn load(dir: Option<&Path>, authored: &authored::Dir) -> Result<Loaded, String> {
    load_for(dir, authored, &eve_marshal::build::accepted())
}

/// Open `--content DIR` if one is given, accepting a bundle of any build in
/// `accepted` (`--client-build`, BA-4), then build every registered table
/// from it and from `authored` (plan §1.3-§1.5, IP1).
///
/// With no bundle, every table's `load` still runs: a client-data table
/// gives its fixture, and an authored table reads `authored`. The first
/// table that fails stops the server with an error naming the table.
pub fn load_for(
    dir: Option<&Path>,
    authored: &authored::Dir,
    accepted: &[u32],
) -> Result<Loaded, String> {
    load_with(&crate::module::plan().tables, dir, authored, accepted)
}

/// [`load_for`] over any spec list, so a test can use local tables.
fn load_with(
    specs: &[TableSpec],
    dir: Option<&Path>,
    authored: &authored::Dir,
    accepted: &[u32],
) -> Result<Loaded, String> {
    let Some(dir) = dir else {
        let (content, tables) = Content::load_with(specs, &Sources::new(None, authored))?;
        let mut lines = vec![NO_BUNDLE.to_owned()];
        lines.extend(tables);
        return Ok(Loaded {
            bundle: None,
            content,
            lines,
        });
    };
    let (bundle, m) = Bundle::open_accepting(dir, accepted)?;
    let mut lines = vec![m.startup_line(dir)];
    let sha8 = |s: &str| s.chars().take(8).collect::<String>();
    for rec in &m.files {
        // C1 §5.4's per-file line.
        let section = rec.path.split('/').next().unwrap_or("");
        lines.push(format!(
            "content: {section} {} {} (bundle {})",
            rec.path,
            rec.rows,
            sha8(&rec.sha256)
        ));
    }
    let (content, tables) = Content::load_with(specs, &Sources::new(Some(&bundle), authored))?;
    lines.extend(tables);
    Ok(Loaded {
        bundle: Some(bundle),
        content,
        lines,
    })
}

/// Where one table's rows came from: the word its startup line prints
/// (V7 proposal 4, CA4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Read from the bundle (`out/content/`).
    Bundle,
    /// The generated rows compiled into the binary, whatever the bundle
    /// holds (reprocess and industry until CP32b and CP33b).
    Baked,
    /// An authored file read from this path under `--authored`.
    AuthoredDisk(PathBuf),
    /// An authored file's compiled-in copy.
    AuthoredCompiled(String),
    /// The table's invented rows: a client-data table with no bundle.
    Fixture,
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Origin::Bundle => f.write_str("bundle"),
            Origin::Baked => f.write_str("baked"),
            Origin::AuthoredDisk(p) => write!(f, "authored:disk {}", p.display()),
            Origin::AuthoredCompiled(rel) => write!(f, "authored:compiled {rel}"),
            Origin::Fixture => f.write_str("fixture"),
        }
    }
}

/// What a table's `load` reads from: the bundle, if there is one, and the
/// authored directory, which always exists (it may be
/// [`authored::Dir::compiled_only`]).
///
/// A table says where its rows came from with [`Sources::note`];
/// [`Sources::authored`] notes for it. A table that notes nothing is
/// reported as [`Origin::Bundle`] with a bundle and [`Origin::Fixture`]
/// without one.
pub struct Sources<'a> {
    /// `None` without `--content`: a client-data table then returns its
    /// fixture.
    pub bundle: Option<&'a Bundle>,
    /// `--authored`, or its default.
    pub authored: &'a authored::Dir,
    noted: RefCell<Vec<Origin>>,
}

impl<'a> Sources<'a> {
    /// The sources for one load.
    pub fn new(bundle: Option<&'a Bundle>, authored: &'a authored::Dir) -> Self {
        Sources {
            bundle,
            authored,
            noted: RefCell::new(Vec::new()),
        }
    }

    /// Record where (some of) the current table's rows came from.
    pub fn note(&self, origin: Origin) {
        self.noted.borrow_mut().push(origin);
    }

    /// Read the authored file `rel` through [`authored::Dir::load`] (which
    /// prints its own per-file line) and note its source.
    pub fn authored(
        &self,
        rel: &str,
        compiled: &str,
        header: &str,
    ) -> Result<authored::File, String> {
        let file = self.authored.load(rel, compiled, header)?;
        self.note(match &file.source {
            authored::Source::Disk(p) => Origin::AuthoredDisk(p.clone()),
            authored::Source::Compiled => Origin::AuthoredCompiled(rel.to_owned()),
        });
        Ok(file)
    }

    /// The notes since the last call, or the default origin.
    fn take(&self) -> Vec<Origin> {
        let noted = std::mem::take(&mut *self.noted.borrow_mut());
        if !noted.is_empty() {
            return noted;
        }
        vec![match self.bundle {
            Some(_) => Origin::Bundle,
            None => Origin::Fixture,
        }]
    }
}

/// One registered table: how to load it, and how to build it from invented
/// rows. A row of [`crate::content_tables::TABLES`].
#[derive(Clone, Copy)]
pub struct TableSpec {
    /// The table's name, for the startup line and for errors.
    pub name: &'static str,
    /// The real rows, from the bundle and the authored directory (CA1).
    /// Called with or without a bundle; with none, a client-data table
    /// returns its fixture. An absent optional file gives an empty table
    /// (plan §1.3); a file that is there but wrong is an `Err` naming the
    /// file, which [`load`] reports with the table's name.
    pub load: fn(&Sources) -> Result<Boxed, String>,
    /// Invented rows, for tests. Never a client value
    /// (`docs/content/plan.md` §10, the house rules).
    pub fixture: fn() -> Boxed,
}

/// Every loaded table, keyed by its type.
#[derive(Default)]
pub struct Content {
    tables: BTreeMap<TypeId, (&'static str, Boxed)>,
    /// Loaded with a bundle. Read by [`Self::is_bundle`], for the login
    /// warning (plan §1.5).
    bundle: bool,
}

/// The table names and where they came from; the tables themselves are
/// `dyn Any`. Lets `server::Config`, which is `Debug`, hold an
/// `Arc<Content>`.
impl std::fmt::Debug for Content {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Content")
            .field("bundle", &self.bundle)
            .field(
                "tables",
                &self.tables.values().map(|(n, _)| n).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Content {
    /// Every table in [`crate::content_tables::TABLES`], from invented rows.
    pub fn fixture() -> Self {
        Self::build(&crate::module::plan().tables, |spec| (spec.fixture)())
    }

    /// Every spec's `load`, each called once, and one startup line per
    /// table naming its source (CA4). The first error stops the load and
    /// names the table.
    fn load_with(specs: &[TableSpec], src: &Sources) -> Result<(Self, Vec<String>), String> {
        let mut lines = Vec::with_capacity(specs.len());
        let mut c = Self::try_build(specs, |spec| {
            src.take();
            let table =
                (spec.load)(src).map_err(|e| format!("[content] table {}: {e}", spec.name))?;
            let origins: Vec<String> = src.take().iter().map(Origin::to_string).collect();
            lines.push(format!(
                "[content] table {}: {}",
                spec.name,
                origins.join(", ")
            ));
            Ok(table)
        })?;
        c.bundle = src.bundle.is_some();
        Ok((c, lines))
    }

    /// Whether these tables were loaded with a bundle. `false` for the
    /// fixture and for the server with no `--content` (plan §1.5).
    pub fn is_bundle(&self) -> bool {
        self.bundle
    }

    /// Mark the tables as a bundle's, for a test of what a bundle changes.
    #[cfg(test)]
    pub fn mark_bundle(&mut self) {
        self.bundle = true;
    }

    /// One table per spec, each constructor called once.
    ///
    /// Two specs producing the same type would make one of them unreachable
    /// by [`Content::get`], so that is an error naming both ([`Self::build`]
    /// panics on it, [`Self::try_build`] and `module::build` return it): a
    /// registration mistake, caught the first time any test builds a
    /// `Registry`, and at startup for a module's tables.
    fn build(specs: &[TableSpec], make: impl Fn(&TableSpec) -> Boxed) -> Self {
        match Self::try_build(specs, |spec| Ok(make(spec))) {
            Ok(c) => c,
            Err(e) => panic!("{e}"),
        }
    }

    /// [`Self::build`], stopping at the first constructor error.
    fn try_build(
        specs: &[TableSpec],
        mut make: impl FnMut(&TableSpec) -> Result<Boxed, String>,
    ) -> Result<Self, String> {
        let mut out = Content::default();
        for spec in specs {
            let table = make(spec)?;
            let key = Any::type_id(&*table);
            if let Some((other, _)) = out.tables.insert(key, (spec.name, table)) {
                return Err(format!(
                    "content tables {other} and {} have the same type; \
                     Content::get could only ever return one of them",
                    spec.name
                ));
            }
        }
        Ok(out)
    }

    /// The fixture content of exactly `specs`, or the type clash. Reads no
    /// plan, so `module::build` can use it while the plan is being made.
    pub(crate) fn try_fixture(specs: &[TableSpec]) -> Result<Self, String> {
        Self::try_build(specs, |spec| Ok((spec.fixture)()))
    }

    /// The table of type `T`.
    ///
    /// # Panics
    ///
    /// When no registered table has that type: a module reading a table it
    /// never registered is a bug that every test would hit, not a state a
    /// player can reach.
    pub fn get<T: Any>(&self) -> &T {
        self.try_get().unwrap_or_else(|| {
            panic!(
                "no content table of type {}: add its TableSpec to \
                 content_tables::TABLES",
                std::any::type_name::<T>()
            )
        })
    }

    /// The table of type `T`, if one is registered.
    pub fn try_get<T: Any>(&self) -> Option<&T> {
        self.tables
            .get(&TypeId::of::<T>())
            .and_then(|(_, t)| t.downcast_ref())
    }

    /// Put a table in, replacing any table of the same type. For a test that
    /// needs rows the fixture does not have.
    pub fn insert<T: Any + Send + Sync>(&mut self, name: &'static str, table: T) {
        self.tables
            .insert(TypeId::of::<T>(), (name, Box::new(table)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Rocks(Vec<i64>);
    #[derive(Debug, PartialEq)]
    struct Agents(Vec<&'static str>);

    fn rocks() -> Boxed {
        Box::new(Rocks(vec![1, 2]))
    }
    fn agents() -> Boxed {
        Box::new(Agents(vec!["invented"]))
    }
    fn none(_: &Sources) -> Result<Boxed, String> {
        unreachable!("a fixture build never loads")
    }

    /// `Content::fixture` builds every row of `TABLES`, each under its own
    /// type, so `Registry::new` can serve every registered table with no
    /// bundle. Checked on the real table and on a local one, because the
    /// real one is empty until W11 registers into it.
    #[test]
    fn fixture_builds_every_registered_table() {
        let real = Content::fixture();
        assert_eq!(real.tables.len(), crate::content_tables::TABLES.len());
        for spec in crate::content_tables::TABLES {
            let key = Any::type_id(&*(spec.fixture)());
            assert!(real.tables.contains_key(&key), "{} is missing", spec.name);
        }

        let local = [
            TableSpec {
                name: "rocks",
                load: none,
                fixture: rocks,
            },
            TableSpec {
                name: "agents",
                load: none,
                fixture: agents,
            },
        ];
        let c = Content::build(&local, |spec| (spec.fixture)());
        assert_eq!(c.tables.len(), local.len());
        assert_eq!(c.get::<Rocks>(), &Rocks(vec![1, 2]));
        assert_eq!(c.get::<Agents>(), &Agents(vec!["invented"]));
        assert!(c.try_get::<String>().is_none());
    }

    // ---- the bundle (CP3). Every row below is invented; the build is the
    // registry's reference [V], never a number written here.

    use std::sync::atomic::{AtomicUsize, Ordering};

    fn build() -> i64 {
        i64::from(eve_marshal::build::REFERENCE)
    }

    /// A fresh, empty directory under the system temp dir, removed on drop.
    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tmp() -> Tmp {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "eve-content-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Tmp(d)
    }

    const ROCKS_HEAD: &str =
        "#belt_id\tordinal\ttype_id\tcount\tsystem\tgrowth\tunit_size\tmax_radius";

    /// A synthetic bundle: one headed file with two rows and a comment, and
    /// one kind-tagged file with one row. `format` and `build` are the
    /// manifest's values; `rocks_rows` is what the manifest claims.
    fn bundle(format: &str, build: i64, rocks_rows: u64) -> Tmp {
        let t = tmp();
        std::fs::create_dir_all(t.0.join("belts")).unwrap();
        std::fs::create_dir_all(t.0.join("types")).unwrap();
        std::fs::write(
            t.0.join(ROCKS),
            format!(
                "{ROCKS_HEAD}\n49000001\t7\t990003\t12\t39000001\t1.5\t0.1\t16255.0\n\
                 # a comment\n49000001\t7\t990004\t3\t39000001\t1.5\t0.1\t16255.0\n"
            ),
        )
        .unwrap();
        // The typedata table (CP58a) builds from this file: invented rows
        // that parse, under a banner.
        let typedata = format!("# banner\n# banner\n{}", crate::typedata::FIXTURE);
        std::fs::write(t.0.join(TYPEDATA), &typedata).unwrap();
        std::fs::write(
            t.0.join(MANIFEST),
            format!(
                "# eve-offline content bundle. CCP's data: never commit this directory.\n\
                 format\t{format}\nclient_build\t{build}\nextractor\t{x}\nsystems\tall\n\
                 file\t{ROCKS}\t{rocks_rows}\t{x}\tres:/invented/a.static\n\
                 file\t{TYPEDATA}\t{typedata_rows}\t{x}\t\n\
                 source\tres:/invented/a.static\t{m}\n",
                x = "ab".repeat(32),
                m = "cd".repeat(16),
                typedata_rows = count_rows(&typedata),
            ),
        )
        .unwrap();
        t
    }

    #[test]
    fn open_accepts_a_good_bundle_of_any_minor() {
        let t = bundle("1.7", build(), 2);
        let (b, m) = Bundle::open(&t.0).unwrap();
        assert_eq!(b.dir, t.0);
        assert_eq!(m.format, (FORMAT_MAJOR, 7));
        assert_eq!(m.client_build, build());
        assert_eq!(m.file(ROCKS).unwrap().rows, 2);
        assert_eq!(m.file(TYPEDATA).unwrap().sources, Vec::<String>::new());
        assert_eq!(m.sources.len(), 1);
        let line = m.startup_line(&t.0);
        for want in [
            "format 1.7",
            &format!("client_build {}", build()),
            &"ab".repeat(32),
        ] {
            assert!(line.contains(want), "{line} lacks {want}");
        }
    }

    #[test]
    fn open_refuses_a_wrong_format_major() {
        let t = bundle(&format!("{}.0", FORMAT_MAJOR + 1), build(), 2);
        let e = Bundle::open(&t.0).unwrap_err();
        assert!(e.contains("format"), "{e}");
    }

    #[test]
    fn open_refuses_a_wrong_build() {
        let unregistered = eve_marshal::build::BUILDS
            .iter()
            .map(|e| i64::from(e.build))
            .max()
            .unwrap()
            + 1;
        let t = bundle("1.0", unregistered, 2);
        let e = Bundle::open(&t.0).unwrap_err();
        assert!(e.contains("client_build"), "{e}");
    }

    #[test]
    fn open_accepts_a_bundle_of_every_accepted_build_and_only_those_chosen() {
        for b in eve_marshal::build::accepted() {
            let t = bundle("1.0", i64::from(b), 2);
            let (_, m) = Bundle::open(&t.0).unwrap();
            assert_eq!(m.client_build, i64::from(b));
            // `--client-build` naming only another build refuses it.
            let others: Vec<u32> = eve_marshal::build::accepted()
                .into_iter()
                .filter(|o| *o != b)
                .collect();
            if !others.is_empty() {
                assert!(Bundle::open_accepting(&t.0, &others).is_err());
            }
        }
    }

    /// Invented: a build number and a data_hash registered for it.
    const INVENTED_BUILD: u32 = 77;
    const INVENTED_HASH: &str = "c872ac9e60959d5caf47923836677cc047ee23074e117d08a81654bccda2ba24";

    #[test]
    fn a_bundle_of_another_build_is_accepted_by_a_registered_data_hash() {
        let hash_of = |b: u32| (b == INVENTED_BUILD).then_some(INVENTED_HASH);
        // Extracted from build 1 (not accepted); its data_hash is the one
        // registered for the accepted INVENTED_BUILD.
        let m = Manifest::parse(&manifest_1_1(INVENTED_HASH)).unwrap();
        assert_eq!(m.client_build, 1);
        let why = bundle_accepted(&m, &[INVENTED_BUILD], &hash_of).unwrap();
        assert!(why.contains("data_hash"), "{why}");
        assert_eq!(bundle_accepted(&m, &[INVENTED_BUILD + 1], &hash_of), None);
        // A 1.0 bundle has no data_hash, so only its build can admit it.
        let mut old = m.clone();
        old.data_hash = None;
        assert_eq!(bundle_accepted(&old, &[INVENTED_BUILD], &hash_of), None);
        assert!(bundle_accepted(&old, &[1], &hash_of).is_some());
    }

    #[test]
    fn open_refuses_a_missing_listed_file_naming_it() {
        let t = bundle("1.0", build(), 2);
        std::fs::remove_file(t.0.join(TYPEDATA)).unwrap();
        let e = Bundle::open(&t.0).unwrap_err();
        assert!(e.contains(TYPEDATA), "{e}");
    }

    #[test]
    fn open_refuses_a_row_count_mismatch() {
        let t = bundle("1.0", build(), 3);
        let e = Bundle::open(&t.0).unwrap_err();
        assert!(e.contains(ROCKS) && e.contains("rows"), "{e}");
    }

    #[test]
    fn open_refuses_no_manifest_and_a_bad_grammar() {
        let t = tmp();
        assert!(Bundle::open(&t.0).unwrap_err().contains(MANIFEST));
        for bad in [
            "format\t1.0\nformat\t1.0\n",
            "format\t1.0\nclient_build\t1\nextractor\tx\nsystems\tall\nwhat\t1\n",
            "format\t1.0\nclient_build\t1\nextractor\tx\nsystems\tall\n\
             file\t../escape.tsv\t0\tx\t\n",
            "client_build\t1\nextractor\tx\nsystems\tall\n",
        ] {
            assert!(Manifest::parse(bad).is_err(), "{bad:?} parsed");
        }
    }

    /// A 1.1 manifest of two invented records, with `data_hash` set to `h`.
    fn manifest_1_1(h: &str) -> String {
        format!(
            "format\t1.1\nclient_build\t1\nextractor\tx\nsystems\tall\ndata_hash\t{h}\n\
             file\tb.tsv\t1\t11\t\nfile\ta.tsv\t2\t22\t\n"
        )
    }

    #[test]
    fn data_hash_agrees_with_content_py_and_is_checked_against_the_records() {
        // The same vector as tools/test_content.py's rule: sha256 over
        // "a.tsv\t22\nb.tsv\t11\n" (sorted by path, whatever the order).
        const PY_VECTOR: &str = "c872ac9e60959d5caf47923836677cc047ee23074e117d08a81654bccda2ba24";
        let m = Manifest::parse(&manifest_1_1(PY_VECTOR)).unwrap();
        assert_eq!(m.data_hash.as_deref(), Some(PY_VECTOR));
        assert_eq!(data_hash_of(&m.files), PY_VECTOR);
        let e = Manifest::parse(&manifest_1_1(&"0".repeat(64))).unwrap_err();
        assert!(e.contains("data_hash"), "{e}");
    }

    #[test]
    fn a_1_0_bundle_has_no_data_hash_and_still_opens() {
        let t = bundle("1.0", build(), 2);
        let (_, m) = Bundle::open(&t.0).unwrap();
        assert_eq!(m.data_hash, None);
    }

    #[test]
    fn a_flag_overrides_the_bundle_file() {
        let t = bundle("1.0", build(), 2);
        let (b, _) = Bundle::open(&t.0).unwrap();
        let flag = Path::new("/elsewhere/typedata.tsv");
        assert_eq!(
            pick(Some(flag), Some(&b), TYPEDATA),
            Some(flag.to_path_buf())
        );
        assert_eq!(pick(None, Some(&b), TYPEDATA), Some(t.0.join(TYPEDATA)));
        assert_eq!(pick(None, Some(&b), SKILLS), None, "absent from the bundle");
        assert_eq!(pick(Some(flag), None, TYPEDATA), Some(flag.to_path_buf()));
        assert_eq!(pick(None, None, TYPEDATA), None);
    }

    #[derive(Debug, PartialEq)]
    struct Belts(Vec<String>);
    fn belts_from(src: &Sources) -> Result<Boxed, String> {
        let b = src.bundle.expect("these tests pass a bundle");
        Ok(Box::new(Belts(b.data_lines(ROCKS, ROCKS_HEAD)?)))
    }

    /// The bundle at `t` loaded over `specs`, with no authored directory.
    fn load_bundle(specs: &[TableSpec], b: &Bundle) -> Result<Content, String> {
        let dir = authored::Dir::compiled_only();
        Content::load_with(specs, &Sources::new(Some(b), &dir)).map(|(c, _)| c)
    }

    #[test]
    fn an_absent_optional_file_gives_an_empty_table() {
        let t = bundle("1.0", build(), 2);
        let (b, _) = Bundle::open(&t.0).unwrap();
        let spec = [TableSpec {
            name: "belts",
            load: belts_from,
            fixture: rocks,
        }];
        let c = load_bundle(&spec, &b).unwrap();
        assert_eq!(c.get::<Belts>().0.len(), 2, "comments are not rows");

        std::fs::remove_file(t.0.join(ROCKS)).unwrap();
        let c = load_bundle(&spec, &b).unwrap();
        assert_eq!(c.get::<Belts>(), &Belts(Vec::new()));
    }

    #[test]
    fn a_wrong_header_is_an_error_naming_the_file() {
        let t = bundle("1.0", build(), 2);
        let (b, _) = Bundle::open(&t.0).unwrap();
        let e = b.data_lines(ROCKS, "#belt_id\tordinal").unwrap_err();
        assert!(e.contains(ROCKS), "{e}");
    }

    static CALLS_A: AtomicUsize = AtomicUsize::new(0);
    static CALLS_B: AtomicUsize = AtomicUsize::new(0);
    fn count_a(_: &Sources) -> Result<Boxed, String> {
        CALLS_A.fetch_add(1, Ordering::SeqCst);
        Ok(rocks())
    }
    fn count_b(_: &Sources) -> Result<Boxed, String> {
        CALLS_B.fetch_add(1, Ordering::SeqCst);
        Ok(agents())
    }

    #[test]
    fn from_bundle_calls_each_table_once() {
        let b = Bundle {
            dir: PathBuf::from("/nonexistent"),
        };
        let specs = [
            TableSpec {
                name: "a",
                load: count_a,
                fixture: rocks,
            },
            TableSpec {
                name: "b",
                load: count_b,
                fixture: agents,
            },
        ];
        let c = load_bundle(&specs, &b).unwrap();
        assert_eq!(CALLS_A.load(Ordering::SeqCst), 1);
        assert_eq!(CALLS_B.load(Ordering::SeqCst), 1);
        assert!(c.is_bundle());
        assert_eq!(c.get::<Rocks>(), &Rocks(vec![1, 2]));
        // The real registration list too: every entry, under its own type.
        let real = load_bundle(crate::content_tables::TABLES, &b).unwrap();
        assert_eq!(real.tables.len(), crate::content_tables::TABLES.len());
    }

    #[test]
    fn no_bundle_means_fixture_content_and_the_warning() {
        let dir = authored::Dir::compiled_only();
        let l = load(None, &dir).unwrap();
        assert!(l.bundle.is_none());
        assert!(!l.content.is_bundle());
        assert_eq!(l.content.tables.len(), Content::fixture().tables.len());
        assert_eq!(l.lines[0], NO_BUNDLE);
        assert_eq!(
            l.lines.len(),
            1 + crate::content_tables::TABLES.len(),
            "the warning, then one line per table: {:?}",
            l.lines
        );
        assert!(NO_BUNDLE.contains("NO BUNDLE") && NO_BUNDLE.contains("tools/up.sh"));

        let t = bundle("1.0", build(), 2);
        let l = load(Some(&t.0), &dir).unwrap();
        assert!(l.content.is_bundle());
        assert!(!l.lines.iter().any(|x| x == NO_BUNDLE));
        assert!(l.lines.iter().any(|x| x.contains(ROCKS)));
    }

    // ---- IP1: the authored directory and the error path (CA1, CA4).

    /// An invented authored file and its header (no client value).
    const NOTES: &str = "notes.tsv";
    const NOTES_HEAD: &str = "key\tcount\twhy";
    const NOTES_SHIPPED: &str = "key\tcount\twhy\nshipped\t1\t[A] invented\n";

    #[derive(Debug, PartialEq)]
    struct Notes(Vec<(String, i64)>);
    fn notes(src: &Sources) -> Result<Boxed, String> {
        let f = src.authored(NOTES, NOTES_SHIPPED, NOTES_HEAD)?;
        Ok(Box::new(Notes(
            f.parse_rows(|r| Ok((r.field(0).to_owned(), r.i64(1)?))),
        )))
    }

    static SEEN_ROOT: std::sync::Mutex<Vec<Option<PathBuf>>> = std::sync::Mutex::new(Vec::new());
    fn record_root(src: &Sources) -> Result<Boxed, String> {
        SEEN_ROOT
            .lock()
            .unwrap()
            .push(src.authored.root().map(Path::to_path_buf));
        Ok(rocks())
    }
    fn fails(_: &Sources) -> Result<Boxed, String> {
        Err("agents/invented.tsv: expected header".to_owned())
    }

    #[test]
    fn load_hands_every_table_the_authored_dir() {
        let t = tmp();
        let dir = authored::Dir::new(&t.0);
        let specs = [
            TableSpec {
                name: "first",
                load: record_root,
                fixture: rocks,
            },
            TableSpec {
                name: "second",
                load: notes,
                fixture: agents,
            },
        ];
        let b = bundle("1.0", build(), 2);
        let with = load_with(&specs, Some(&b.0), &dir, &eve_marshal::build::accepted()).unwrap();
        let without = load_with(&specs, None, &dir, &eve_marshal::build::accepted()).unwrap();
        assert_eq!(
            *SEEN_ROOT.lock().unwrap(),
            vec![Some(t.0.clone()), Some(t.0.clone())],
            "called with and without a bundle, each time with the given dir"
        );
        for l in [&with, &without] {
            for name in ["first", "second"] {
                let line = format!("[content] table {name}: ");
                assert!(
                    l.lines.iter().any(|x| x.starts_with(&line)),
                    "{:?}",
                    l.lines
                );
            }
        }
        // A table that notes nothing is reported by what it was given.
        assert!(
            with.lines
                .contains(&"[content] table first: bundle".to_owned())
        );
        assert!(
            without
                .lines
                .contains(&"[content] table first: fixture".to_owned())
        );
        assert!(with.content.is_bundle() && !without.content.is_bundle());
    }

    #[test]
    fn a_table_error_is_a_startup_error_naming_the_table() {
        let specs = [
            TableSpec {
                name: "fine",
                load: |_| Ok(rocks()),
                fixture: rocks,
            },
            TableSpec {
                name: "broken",
                load: fails,
                fixture: agents,
            },
        ];
        let dir = authored::Dir::compiled_only();
        let e = load_with(&specs, None, &dir, &eve_marshal::build::accepted())
            .err()
            .expect("an error, not a panic");
        assert!(
            e.contains("table broken") && e.contains("agents/invented.tsv"),
            "{e}"
        );
        let b = bundle("1.0", build(), 2);
        let e = load_with(&specs, Some(&b.0), &dir, &eve_marshal::build::accepted())
            .err()
            .expect("an error");
        assert!(e.contains("table broken"), "{e}");
    }

    #[test]
    fn no_bundle_still_reads_authored_tables() {
        let specs = [TableSpec {
            name: "notes",
            load: notes,
            fixture: agents,
        }];
        let t = tmp();
        std::fs::write(
            t.0.join(NOTES),
            "# an owner's edit\nkey\tcount\twhy\nedited\t3\t[A] invented\n",
        )
        .unwrap();
        let l = load_with(
            &specs,
            None,
            &authored::Dir::new(&t.0),
            &eve_marshal::build::accepted(),
        )
        .unwrap();
        assert_eq!(l.content.get::<Notes>(), &Notes(vec![("edited".into(), 3)]));
        let disk = Origin::AuthoredDisk(t.0.join(NOTES)).to_string();
        assert!(
            l.lines.contains(&format!("[content] table notes: {disk}")),
            "{:?}",
            l.lines
        );

        // No file on disk: the compiled-in copy, and the line says so.
        let empty = tmp();
        let l = load_with(
            &specs,
            None,
            &authored::Dir::new(&empty.0),
            &eve_marshal::build::accepted(),
        )
        .unwrap();
        assert_eq!(
            l.content.get::<Notes>(),
            &Notes(vec![("shipped".into(), 1)])
        );
        assert!(
            l.lines
                .contains(&format!("[content] table notes: authored:compiled {NOTES}"))
        );
    }
}
