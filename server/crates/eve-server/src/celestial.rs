//! The solar system's own furniture: the star, its planets, their moons and
//! belts, the stations and the gates.
//!
//! **A celestial the client cannot see as a ball is a celestial nobody can
//! right-click.** That is the whole reason this module exists, and it was
//! found at a keyboard rather than by reading: a player could warp to the
//! station and to nothing else, and the station was the only celestial this
//! server had ever put in the ballpark (`destiny::station_ball`). The client
//! renders planets from its own static data, so the scene looked complete --
//! but with no ball there was no bracket, no right-click menu and therefore
//! no `CmdWarpToStuff` to answer. Nothing appeared in any log on either side,
//! because nothing happened.
//!
//! ## Where the numbers come from, and why not from here
//!
//! `tools/celestials.py` reads one system's contents out of the client's own
//! `res:/staticdata/solarsystemcontent.static` and writes a TSV into `out/`,
//! which is gitignored. The server loads that file.
//!
//! The obvious alternative was a table in this file beside
//! `destiny::hull::HULLS`, and it is the one option that was **not**
//! available: the positions are CCP's data, and this project's standing rule
//! is that no extracted client data goes in the repository. So what ships is
//! a reader, and the data stays on the disk of whoever legally obtained the
//! client. The owner chose this shape on 2026-10-22 from three options
//! written up in `docs/port-tracker.md` §32.4.
//!
//! **The positions must be CCP's exactly, not plausible.** The client already
//! knows where every celestial in the system is, and draws it there. A ball
//! we place anywhere else gives a bracket in one place and a model in
//! another.
//!
//! ## What the server declines to serve, and why
//!
//! The extract is faithful -- every celestial, including the kinds we do not
//! put in the park:
//!
//! - (**the star is served**, as a global ball like a planet. It was once
//!   left out because its position *is* the system origin, where our one
//!   station sat; every catalogued station has its real position since FX4,
//!   so the reason is gone, and without a sun ball the client could not warp
//!   to it and the Sovereignty Hub (`sov_hub::hub_position`) had no warp
//!   target. A sandbox station with no catalogue row still sits at the origin
//!   [A: only a test universe has one; falsified by a live system whose
//!   station row is missing]);
//! - **a stargate with no partner in the extract.** A gate's slim item must
//!   carry `jumps = [row(toCelestialID, locationID)]` or
//!   `movementFunctions.DockOrJumpOrActivateGate:410` does `slimItem.jumps[0]`
//!   with no guard and raises `IndexError` **[V]** (`docs/re/jump-gates.md`
//!   §10). An extract written before the `dest` column has no partner to put
//!   there, so its gates stay out of the park exactly as they always were.
//!   A gate *with* a `dest` is served, with the row (P3.3, `crate::jump`).
//!
//! Keeping the filter here rather than in the tool means the extract stays a
//! complete record and the policy lives in one place.
//!
//! ## More than one system (P3.3)
//!
//! The file format grew two columns and one kind of comment, all of them
//! invisible to a reader that predates them (`docs/lanes/stargates.md` §3.2):
//!
//! | column / line | meaning | absent in an old file |
//! |---|---|---|
//! | 8 `system` | the row's solar system | the `# solarSystemID\t<id>` header, else 0 |
//! | 9 `dest` | a gate's partner gate, `0` for anything else | no partner: the gate is not served |
//! | 10 `dest_system` | the partner gate's solar system, `0` for anything else | taken from the partner's own row, if loaded |
//! | `#system\t<id>\t<constellation>\t<region>` | the jump's session change keys | no row: [`Catalogue::system`] is `None` |
//!
//! **"The destination is loaded" means the partner's row is in this file**,
//! and nothing else: [`Catalogue::gate`] answers both rows or neither.
//!
//! Column 10 is one more than `docs/lanes/stargates.md` §3.2 asked for, and
//! for a reason it did not reach: every gate is served, including the ones
//! leading out of the loaded systems, and each one's slim needs
//! `jumps = [(toCelestialID, locationID)]` -- the *system* the partner is in.
//! For an unloaded partner nothing else in the file knows it, and the client
//! labels the gate and preloads the destination scene from that number
//! (`subwaySvc.py:82`).
//!
//! ## The universe table (CP3b)
//!
//! [`Universe`] is the one owner of a system's security, security band,
//! region, constellation and wormhole class (`docs/content/plan.md` §3.2),
//! read from the bundle's `universe/systems.tsv`, `universe/regions.tsv` and
//! `universe/constellations.tsv` (`docs/content/schema.md` §2) and
//! registered as [`UNIVERSE`]. With no bundle it is [`Universe::fixture`],
//! invented rows only. Code reads it through `reg.content().get::<Universe>()`.
//!
//! The [`Catalogue`]'s `#system` lines still carry region and constellation
//! for the jump's session change, as before; they come from the same
//! `systems.static` columns, so the two cannot disagree on one extract.

use crate::content::{Boxed, Bundle, Sources, TableSpec};
use std::collections::BTreeMap;

/// What kind of thing a celestial is.
///
/// Parsed from the extract's first column. An unknown word is **kept** as
/// [`Kind::Other`] rather than dropped: a future client build that adds a
/// kind should leave a trace in the catalogue instead of silently shrinking
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Star,
    Planet,
    Moon,
    Belt,
    Station,
    Gate,
    Other,
}

impl Kind {
    fn parse(word: &str) -> Kind {
        match word {
            "star" => Kind::Star,
            "planet" => Kind::Planet,
            "moon" => Kind::Moon,
            "belt" => Kind::Belt,
            "station" => Kind::Station,
            "gate" => Kind::Gate,
            _ => Kind::Other,
        }
    }

    /// Whether this server puts one of these in the ballpark. The module
    /// header has the argument for each exclusion -- and a gate is served
    /// only with a partner, which is a fact of the row rather than the kind:
    /// [`Catalogue::served_in`] applies it.
    pub fn is_served(self) -> bool {
        matches!(
            self,
            Kind::Star | Kind::Planet | Kind::Moon | Kind::Belt | Kind::Gate | Kind::Station
        )
    }
}

/// One celestial.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Celestial {
    pub id: i64,
    pub type_id: i64,
    /// Metres. **Zero for an asteroid belt**, which is a field of rocks
    /// rather than a body and carries no radius in the client's own data --
    /// the warp standoff does the work there.
    pub radius: f64,
    pub position: [f64; 3],
    pub kind: Kind,
}

/// One solar system's identity, from a `#system` line: what a jump's session
/// change needs beyond the system id itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemRow {
    pub id: i64,
    pub constellation_id: i64,
    pub region_id: i64,
}

/// A catalogue row: the celestial and the two columns P3.3 added.
///
/// **Beside [`Celestial`], not inside it**, because `Celestial` is built by
/// struct literal in other modules' tests (`crate::warp_in`), and a new field
/// there would be an edit to files this lane does not own for a fact those
/// tests do not care about.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Row {
    celestial: Celestial,
    system: i64,
    dest: Option<i64>,
    dest_system: Option<i64>,
}

impl Row {
    /// The module header's policy, per row: a gate needs somewhere to go.
    fn is_served(&self, rows: &BTreeMap<i64, Row>) -> bool {
        self.celestial.kind.is_served()
            && (self.celestial.kind != Kind::Gate || self.jump(rows).is_some())
    }

    /// `(toCelestialID, locationID)` for a gate: its own column, else the
    /// partner's row.
    fn jump(&self, rows: &BTreeMap<i64, Row>) -> Option<(i64, i64)> {
        let dest = self.dest?;
        let system = self
            .dest_system
            .or_else(|| rows.get(&dest).map(|p| p.system))
            .filter(|s| *s != 0)?;
        Some((dest, system))
    }
}

/// Two ascending id lists as one ascending sequence: a system's own rows and
/// the rows served everywhere (system `0`), in the id order
/// [`Catalogue::served_in`] promises.
struct Merged<'a> {
    a: &'a [i64],
    b: &'a [i64],
}

impl Iterator for Merged<'_> {
    type Item = i64;

    fn next(&mut self) -> Option<i64> {
        let take_a = match (self.a.first(), self.b.first()) {
            (Some(x), Some(y)) => x <= y,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => return None,
        };
        let side = if take_a { &mut self.a } else { &mut self.b };
        let (first, rest) = side.split_first()?;
        *side = rest;
        Some(*first)
    }
}

/// Every celestial in the systems the extract holds.
///
/// Empty is the ordinary case, not an error: a server started without
/// `--celestials` has no catalogue, every celestial lookup misses, and the
/// behaviour is exactly what it was before this module existed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalogue {
    rows: BTreeMap<i64, Row>,
    systems: BTreeMap<i64, SystemRow>,
    /// **The served ids of each system, in id order** (lane perf-fight):
    /// built once by [`Self::parse`], the only writer of `rows`, so
    /// [`Self::served_in`] reads one system instead of walking every row of
    /// the universe -- about 490,000 of them in a full bundle, on every
    /// bind, every site anchor and every scan.
    served_by_system: BTreeMap<i64, Vec<i64>>,
    /// **Each system's first star row** (its id), built with the rows, so
    /// [`Self::star_of`] is one lookup, not a scan of the universe.
    stars: BTreeMap<i64, i64>,
}

impl Catalogue {
    /// Read an extract written by `tools/celestials.py`.
    pub fn load(path: &std::path::Path) -> std::io::Result<Catalogue> {
        Ok(Catalogue::parse(&std::fs::read_to_string(path)?))
    }

    /// The same from text, which is what the tests use.
    ///
    /// **A bad line is skipped, not fatal.** The catalogue is a convenience
    /// read from a generated file; refusing to start because one row is
    /// malformed would take the whole server down for a cosmetic feature.
    /// Comments begin with `#`, which is how the tool writes its header --
    /// and its `#system` lines, the one comment that is read.
    ///
    /// **Seven, eight or ten columns.** The single-system format had seven
    /// and a `# solarSystemID` header; `--all` added `system`; P3.3 added
    /// `dest` and `dest_system`. Every one of them still parses, and a test
    /// pins them.
    pub fn parse(text: &str) -> Catalogue {
        let mut rows = BTreeMap::new();
        let mut systems = BTreeMap::new();
        // The single-system file's header names the system once, for every
        // row; a longer row's own column wins over it.
        let mut header_system = 0;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix('#') {
                let f: Vec<&str> = rest.split('\t').map(str::trim).collect();
                match f.as_slice() {
                    ["system", id, constellation, region, ..] => {
                        if let (Ok(id), Ok(constellation_id), Ok(region_id)) = (
                            id.parse::<i64>(),
                            constellation.parse::<i64>(),
                            region.parse::<i64>(),
                        ) {
                            systems.insert(
                                id,
                                SystemRow {
                                    id,
                                    constellation_id,
                                    region_id,
                                },
                            );
                        }
                    }
                    ["solarSystemID", id, ..] => {
                        header_system = id.parse::<i64>().unwrap_or(0);
                    }
                    _ => {}
                }
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 7 {
                continue;
            }
            let (Ok(id), Ok(type_id)) = (f[1].parse::<i64>(), f[2].parse::<i64>()) else {
                continue;
            };
            let Ok(radius) = f[3].parse::<f64>() else {
                continue;
            };
            let (Ok(x), Ok(y), Ok(z)) = (
                f[4].parse::<f64>(),
                f[5].parse::<f64>(),
                f[6].parse::<f64>(),
            ) else {
                continue;
            };
            if ![radius, x, y, z].iter().all(|v| v.is_finite()) {
                continue;
            }
            let system = f
                .get(7)
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(header_system);
            // `0` is "no partner", which is what every non-gate row says.
            let dest = f
                .get(8)
                .and_then(|v| v.parse::<i64>().ok())
                .filter(|d| *d != 0);
            let dest_system = f
                .get(9)
                .and_then(|v| v.parse::<i64>().ok())
                .filter(|d| *d != 0);
            rows.insert(
                id,
                Row {
                    celestial: Celestial {
                        id,
                        type_id,
                        radius,
                        position: [x, y, z],
                        kind: Kind::parse(f[0]),
                    },
                    system,
                    dest,
                    dest_system,
                },
            );
        }
        let mut served_by_system: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
        for (id, row) in &rows {
            if row.is_served(&rows) {
                served_by_system.entry(row.system).or_default().push(*id);
            }
        }
        let mut stars: BTreeMap<i64, i64> = BTreeMap::new();
        for (id, row) in &rows {
            if row.celestial.kind == Kind::Star {
                stars.entry(row.system).or_insert(*id);
            }
        }
        Catalogue {
            rows,
            systems,
            served_by_system,
            stars,
        }
    }

    pub fn get(&self, id: i64) -> Option<&Celestial> {
        self.rows.get(&id).map(|r| &r.celestial)
    }

    /// Whether this celestial goes in a park: [`Kind::is_served`], and for a
    /// gate a partner too. `Registry::ball_target` and anything else that
    /// must agree with the park asks this rather than the kind.
    pub fn is_served(&self, id: i64) -> bool {
        self.rows.get(&id).is_some_and(|r| r.is_served(&self.rows))
    }

    /// The ones that go in a park, every system, in id order.
    pub fn served(&self) -> impl Iterator<Item = &Celestial> {
        self.rows
            .values()
            .filter(|r| r.is_served(&self.rows))
            .map(|r| &r.celestial)
    }

    /// The ones that go in **this system's** park, in id order so that two
    /// bootstraps of the same world build the same blob.
    ///
    /// A row with system `0` -- a seven-column file with no header -- is
    /// served in every system, which is exactly the one-system behaviour
    /// such a file had before there was a second.
    pub fn served_in(&self, system: i64) -> impl Iterator<Item = &Celestial> {
        let none: &[i64] = &[];
        let own = self
            .served_by_system
            .get(&system)
            .map_or(none, Vec::as_slice);
        let everywhere = match system {
            // System `0`'s own rows are the "everywhere" rows; never twice.
            0 => none,
            _ => self.served_by_system.get(&0).map_or(none, Vec::as_slice),
        };
        Merged {
            a: own,
            b: everywhere,
        }
        .filter_map(|id| self.rows.get(&id).map(|r| &r.celestial))
    }

    /// The solar system a celestial is in, `0` when the file does not say.
    pub fn system_of(&self, id: i64) -> Option<i64> {
        self.rows.get(&id).map(|r| r.system)
    }

    /// A gate and its partner, `(gate, partner)`, when **both** rows are
    /// loaded. `None` for anything that is not a gate, a gate with no
    /// partner column, and a partner outside this extract -- which is what
    /// "this gate's destination is not loaded" means.
    pub fn gate(&self, id: i64) -> Option<(&Celestial, &Celestial)> {
        let row = self.rows.get(&id)?;
        if row.celestial.kind != Kind::Gate {
            return None;
        }
        let partner = self.rows.get(&row.dest?)?;
        Some((&row.celestial, &partner.celestial))
    }

    /// A gate's `(toCelestialID, locationID)`, loaded or not: the slim's
    /// `jumps` row. `None` for anything that is not a gate, and for a gate
    /// whose destination system nothing in the file names.
    pub fn jump_of(&self, id: i64) -> Option<(i64, i64)> {
        let row = self.rows.get(&id)?;
        if row.celestial.kind != Kind::Gate {
            return None;
        }
        row.jump(&self.rows)
    }

    /// A system's constellation and region, from its `#system` line.
    pub fn system(&self, id: i64) -> Option<&SystemRow> {
        self.systems.get(&id)
    }

    /// `system`'s star (a served ball since the sun fix): where
    /// SOC-D2 puts a Sovereignty Hub (`sov_hub::position_in`). `None` when
    /// the catalogue has no star row for it.
    pub fn star_of(&self, system: i64) -> Option<&Celestial> {
        self.stars
            .get(&system)
            .and_then(|id| self.rows.get(id))
            .map(|r| &r.celestial)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

// ---------------------------------------------------------------------------
// The universe table (CP3b)
// ---------------------------------------------------------------------------

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const UNIVERSE: TableSpec = TableSpec {
    name: "universe",
    load: load_universe,
    fixture: || Box::new(Universe::fixture()) as Boxed,
};

/// The bundle's rows, or the fixture with no bundle (client data, class B).
fn load_universe(src: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => Universe::from_bundle(b)?,
        None => Universe::fixture(),
    }))
}

/// The systems file in the bundle (`schema.md` §2).
pub const SYSTEMS: &str = "universe/systems.tsv";
/// The regions file in the bundle.
pub const REGIONS: &str = "universe/regions.tsv";
/// The constellations file in the bundle.
pub const CONSTELLATIONS: &str = "universe/constellations.tsv";

/// `universe/systems.tsv`'s header, copied from `docs/content/schema.md`.
pub const SYSTEMS_HEADER: &str = "#system_id\tregion_id\tconstellation_id\tfaction_id\towner_faction\tsecurity\tsecurity_class\thub\tborder\tfringe\tcorridor\tregional\tinternational\twormhole_class_id\tdisallow_scanning\tdscan_range\tsun_type_id\tname";
/// `universe/regions.tsv`'s header, copied from `docs/content/schema.md`.
pub const REGIONS_HEADER: &str =
    "#region_id\tnebula_id\tfaction_id\twormhole_class_id\tneighbours\tname";
/// `universe/constellations.tsv`'s header, copied from `docs/content/schema.md`.
pub const CONSTELLATIONS_HEADER: &str =
    "#constellation_id\tregion_id\tfaction_id\twormhole_class_id\tneighbours\tname";

/// The client's security class, `eveuniverse/security.py`
/// (`securityClassZeroSec 0`, `LowSec 1`, `HighSec 2`, `SafeSec 3`) **[V]**
/// (`docs/re/damage-death.md` §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SecurityClass {
    Zero = 0,
    Low = 1,
    High = 2,
    Safe = 3,
}

impl SecurityClass {
    /// `SecurityClassFromLevel(level)` **[V]**: `ZeroSec` for `level <= 0.0`,
    /// `LowSec` below `0.45`, `HighSec` below `0.95`, else `SafeSec`.
    pub fn from_level(level: f64) -> SecurityClass {
        if level <= 0.0 {
            SecurityClass::Zero
        } else if level < 0.45 {
            SecurityClass::Low
        } else if level < 0.95 {
            SecurityClass::High
        } else {
            SecurityClass::Safe
        }
    }

    /// `is_low_sec_or_higher(c)`, `c >= 1` **[V]**: the guard in front of
    /// the client's whole crimewatch tree (`crimewatchSvc.py:472`).
    pub fn is_low_sec_or_higher(self) -> bool {
        self >= SecurityClass::Low
    }
}

/// `calculate_pseudo_security` **[V]** (`securitySvc.py:24-27`): anything in
/// `(0.0, 0.05)` is lifted to `0.05`, everything else is itself. The client
/// rounds a system's security up this way before it classes it.
pub fn pseudo_security(security: f64) -> f64 {
    if 0.0 < security && security < 0.05 {
        0.05
    } else {
        security
    }
}

/// The population band (C3 §3): high, low and null by security class, and
/// wormhole and void space by id range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    High,
    Low,
    Null,
    Wormhole,
    Void,
}

/// **[I]** Wormhole space is the `31xxxxxx` system ids, and abyssal space the
/// `32xxxxxx` (C3 §3, from `findings/content-C3-bands.tsv`); the client's
/// `mapAbyssalSystem`, `mapVoidSystem` and `mapHiddenSystem` are 32, 34 and
/// 36 million. Falsified by a `31xxxxxx` system with no wormhole class, or a
/// known-space system inside any of these ranges.
pub const WORMHOLE_SYSTEMS: std::ops::Range<i64> = 31_000_000..32_000_000;
/// See [`WORMHOLE_SYSTEMS`].
pub const VOID_SYSTEMS: std::ops::Range<i64> = 32_000_000..33_000_000;
/// The client's `mapVoidSystem` range (`34xxxxxx`): not known space, though the
/// bundle gives them a null security.
pub const VOID_SPACE_SYSTEMS: std::ops::Range<i64> = 34_000_000..35_000_000;
/// The client's `mapHiddenSystem` range (`36xxxxxx`).
pub const HIDDEN_SYSTEMS: std::ops::Range<i64> = 36_000_000..37_000_000;

/// One `universe/systems.tsv` row, every column.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct System {
    pub id: i64,
    pub region_id: i64,
    pub constellation_id: i64,
    pub faction_id: Option<i64>,
    pub owner_faction: Option<i64>,
    /// Unrounded, as `solarsystemcontent` ships it. Class it with
    /// [`System::security_class`], never by comparing this directly.
    pub security: f64,
    pub security_class: Option<String>,
    pub hub: bool,
    pub border: bool,
    pub fringe: bool,
    pub corridor: bool,
    pub regional: bool,
    pub international: bool,
    pub wormhole_class_id: Option<i64>,
    pub disallow_scanning: bool,
    pub dscan_range: Option<f64>,
    pub sun_type_id: Option<i64>,
    /// For display only; nothing is derived from it (ruling 2).
    pub name: String,
}

impl System {
    /// The class the client computes for an unmodified system:
    /// `SecurityClassFromLevel(pseudoSecurity)` **[V]** (`securitySvc.py:76`,
    /// `:86`).
    ///
    /// **[A]** `security` (from `solarsystemcontent`) classes the same as the
    /// `pseudoSecurity` the client reads from `systems.static`. Falsified by a
    /// system whose two classes differ; CP3b's lane note records that the
    /// per-class counts over the bundle equal `docs/re/damage-death.md`
    /// §7.2.1's census of `systems.static`.
    pub fn security_class(&self) -> SecurityClass {
        SecurityClass::from_level(pseudo_security(self.security))
    }

    /// The population band (C3 §3).
    pub fn band(&self) -> Band {
        if WORMHOLE_SYSTEMS.contains(&self.id) {
            return Band::Wormhole;
        }
        if VOID_SYSTEMS.contains(&self.id)
            || VOID_SPACE_SYSTEMS.contains(&self.id)
            || HIDDEN_SYSTEMS.contains(&self.id)
        {
            return Band::Void;
        }
        match self.security_class() {
            SecurityClass::Zero => Band::Null,
            SecurityClass::Low => Band::Low,
            SecurityClass::High | SecurityClass::Safe => Band::High,
        }
    }
}

/// One `universe/regions.tsv` row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Region {
    pub id: i64,
    /// `regions.static.nebulaID`: the graphic id of the region's nebula, which
    /// a wormhole slim names as `nebulaType` (R-WH §3).
    pub nebula_id: Option<i64>,
    pub faction_id: Option<i64>,
    pub wormhole_class_id: Option<i64>,
    pub neighbours: Vec<i64>,
    pub name: String,
}

/// One `universe/constellations.tsv` row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Constellation {
    pub id: i64,
    pub region_id: i64,
    pub faction_id: Option<i64>,
    pub wormhole_class_id: Option<i64>,
    pub neighbours: Vec<i64>,
    pub name: String,
}

/// Every system, region and constellation the bundle holds.
///
/// An absent file gives no rows of that kind (plan §1.3), and every lookup
/// on it misses; callers keep their pre-CP3b rule for a missing system.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Universe {
    systems: BTreeMap<i64, System>,
    regions: BTreeMap<i64, Region>,
    constellations: BTreeMap<i64, Constellation>,
}

/// A row's fields, checked against the header's width.
fn fields<'a>(rel: &str, i: usize, line: &'a str, header: &str) -> Result<Vec<&'a str>, String> {
    let f: Vec<&str> = line.split('\t').collect();
    let want = header.split('\t').count();
    if f.len() != want {
        return Err(format!(
            "{rel}: data row {}: {} fields, the header has {want}. Re-extract with tools/up.sh",
            i + 1,
            f.len()
        ));
    }
    Ok(f)
}

/// Typed field readers for one row; every error names the file, the row and
/// the column (`schema.md` §1.1's types).
struct Cols<'a> {
    rel: &'a str,
    row: usize,
    f: Vec<&'a str>,
}

impl Cols<'_> {
    fn err(&self, c: usize, what: &str) -> String {
        format!(
            "{}: data row {}: column {} {:?} is not {what}",
            self.rel,
            self.row + 1,
            c + 1,
            self.f[c]
        )
    }
    fn i64(&self, c: usize) -> Result<i64, String> {
        self.f[c].parse().map_err(|_| self.err(c, "an i64"))
    }
    fn opt_i64(&self, c: usize) -> Result<Option<i64>, String> {
        if self.f[c].is_empty() {
            return Ok(None);
        }
        self.i64(c).map(Some)
    }
    fn f64(&self, c: usize) -> Result<f64, String> {
        self.f[c]
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| self.err(c, "a finite f64"))
    }
    fn opt_f64(&self, c: usize) -> Result<Option<f64>, String> {
        if self.f[c].is_empty() {
            return Ok(None);
        }
        self.f64(c).map(Some)
    }
    fn bool(&self, c: usize) -> Result<bool, String> {
        match self.f[c] {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => Err(self.err(c, "0 or 1")),
        }
    }
    fn opt_str(&self, c: usize) -> Option<String> {
        (!self.f[c].is_empty()).then(|| self.f[c].to_owned())
    }
    fn list_i64(&self, c: usize) -> Result<Vec<i64>, String> {
        if self.f[c].is_empty() {
            return Ok(Vec::new());
        }
        self.f[c]
            .split(',')
            .map(|x| x.parse().map_err(|_| self.err(c, "a list<i64>")))
            .collect()
    }
}

/// Every row of `lines` through `row`, as `Cols`.
fn parse_rows<T>(
    rel: &str,
    header: &str,
    lines: &[String],
    row: impl Fn(&Cols) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let f = fields(rel, i, line, header)?;
            row(&Cols { rel, row: i, f })
        })
        .collect()
}

impl Universe {
    /// The three files from a checked bundle (`content::Bundle::open`). An
    /// absent file is empty; a wrong header or a malformed row is an error
    /// naming the file, which stops the server: a stale bundle must fail
    /// loudly (`schema.md` §1.1).
    pub fn from_bundle(b: &Bundle) -> Result<Universe, String> {
        Universe::parse(
            &b.data_lines(SYSTEMS, SYSTEMS_HEADER)?,
            &b.data_lines(REGIONS, REGIONS_HEADER)?,
            &b.data_lines(CONSTELLATIONS, CONSTELLATIONS_HEADER)?,
        )
    }

    /// The same from each file's data lines (header and comments removed).
    pub fn parse(
        systems: &[String],
        regions: &[String],
        constellations: &[String],
    ) -> Result<Universe, String> {
        let systems = parse_rows(SYSTEMS, SYSTEMS_HEADER, systems, |c| {
            Ok(System {
                id: c.i64(0)?,
                region_id: c.i64(1)?,
                constellation_id: c.i64(2)?,
                faction_id: c.opt_i64(3)?,
                owner_faction: c.opt_i64(4)?,
                security: c.f64(5)?,
                security_class: c.opt_str(6),
                hub: c.bool(7)?,
                border: c.bool(8)?,
                fringe: c.bool(9)?,
                corridor: c.bool(10)?,
                regional: c.bool(11)?,
                international: c.bool(12)?,
                wormhole_class_id: c.opt_i64(13)?,
                disallow_scanning: c.bool(14)?,
                dscan_range: c.opt_f64(15)?,
                sun_type_id: c.opt_i64(16)?,
                name: c.f[17].to_owned(),
            })
        })?;
        let regions = parse_rows(REGIONS, REGIONS_HEADER, regions, |c| {
            Ok(Region {
                id: c.i64(0)?,
                nebula_id: c.opt_i64(1)?,
                faction_id: c.opt_i64(2)?,
                wormhole_class_id: c.opt_i64(3)?,
                neighbours: c.list_i64(4)?,
                name: c.f[5].to_owned(),
            })
        })?;
        let constellations =
            parse_rows(CONSTELLATIONS, CONSTELLATIONS_HEADER, constellations, |c| {
                Ok(Constellation {
                    id: c.i64(0)?,
                    region_id: c.i64(1)?,
                    faction_id: c.opt_i64(2)?,
                    wormhole_class_id: c.opt_i64(3)?,
                    neighbours: c.list_i64(4)?,
                    name: c.f[5].to_owned(),
                })
            })?;
        Ok(Universe {
            systems: systems.into_iter().map(|s| (s.id, s)).collect(),
            regions: regions.into_iter().map(|r| (r.id, r)).collect(),
            constellations: constellations.into_iter().map(|c| (c.id, c)).collect(),
        })
    }

    /// Invented rows only (`schema.md` §1.1's unused id ranges), never a
    /// client value: two regions, two constellations, and one system per
    /// security case the client distinguishes.
    pub fn fixture() -> Universe {
        let system = |id, constellation_id, region_id, security| System {
            id,
            region_id,
            constellation_id,
            security,
            name: format!("Fixture {id}"),
            ..System::default()
        };
        let mut wh = system(39_000_006, 29_000_002, 19_000_002, -1.0);
        wh.wormhole_class_id = Some(990_001);
        let systems = [
            system(39_000_001, 29_000_001, 19_000_001, 0.75),
            system(39_000_002, 29_000_001, 19_000_001, 0.25),
            system(39_000_003, 29_000_002, 19_000_002, -0.25),
            // Exactly zero: zero-sec. Just above zero: lifted to low sec.
            system(39_000_004, 29_000_002, 19_000_002, 0.0),
            system(39_000_005, 29_000_002, 19_000_002, 0.01),
            wh,
        ];
        let region = |id, neighbour| Region {
            id,
            // invented, one per fixture region
            nebula_id: Some(id - 18_000_000),
            neighbours: vec![neighbour],
            name: format!("Fixture region {id}"),
            ..Region::default()
        };
        let constellation = |id, region_id| Constellation {
            id,
            region_id,
            name: format!("Fixture constellation {id}"),
            ..Constellation::default()
        };
        Universe {
            systems: systems.into_iter().map(|s| (s.id, s)).collect(),
            regions: [
                region(19_000_001, 19_000_002),
                region(19_000_002, 19_000_001),
            ]
            .into_iter()
            .map(|r| (r.id, r))
            .collect(),
            constellations: [
                constellation(29_000_001, 19_000_001),
                constellation(29_000_002, 19_000_002),
            ]
            .into_iter()
            .map(|c| (c.id, c))
            .collect(),
        }
    }

    pub fn system(&self, id: i64) -> Option<&System> {
        self.systems.get(&id)
    }

    pub fn region(&self, id: i64) -> Option<&Region> {
        self.regions.get(&id)
    }

    pub fn constellation(&self, id: i64) -> Option<&Constellation> {
        self.constellations.get(&id)
    }

    /// Every system, in id order.
    pub fn systems(&self) -> impl Iterator<Item = &System> {
        self.systems.values()
    }

    /// The client's class for a system, `None` when the table lacks it.
    pub fn security_class(&self, system: i64) -> Option<SecurityClass> {
        self.system(system).map(System::security_class)
    }

    /// The population band, `None` when the table lacks the system.
    pub fn band(&self, system: i64) -> Option<Band> {
        self.system(system).map(System::band)
    }

    /// The system's wormhole class: its own column, else its
    /// constellation's, else its region's. **[A]** the fallback order, from
    /// the files' nesting; falsified by a system whose client-side class
    /// (`wormholeClassID` on its map entry) differs from this.
    pub fn wormhole_class(&self, system: i64) -> Option<i64> {
        let s = self.system(system)?;
        s.wormhole_class_id
            .or_else(|| {
                self.constellation(s.constellation_id)
                    .and_then(|c| c.wormhole_class_id)
            })
            .or_else(|| self.region(s.region_id).and_then(|r| r.wormhole_class_id))
    }

    /// The graphic id of the nebula of the region a system is in
    /// (`regions.static.nebulaID`), `None` when the bundle lacks it.
    pub fn nebula_of(&self, system: i64) -> Option<i64> {
        let s = self.system(system)?;
        self.region(s.region_id).and_then(|r| r.nebula_id)
    }

    pub fn len(&self) -> usize {
        self.systems.len()
    }

    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }
}

/// The centres file in the bundle (format 1.2, `schema.md` §2).
pub const CENTERS: &str = "universe/centers.tsv";
/// `universe/centers.tsv`'s header, copied from `docs/content/schema.md`.
pub const CENTERS_HEADER: &str = "#kind\tid\tx\ty\tz";

/// The registration row for [`Centers`]; the coordinator adds it to
/// `content_tables::TABLES`.
pub const CENTERS_TABLE: TableSpec = TableSpec {
    name: "centers",
    load: load_centers,
    fixture: || Box::new(Centers::default()) as Boxed,
};

/// The bundle's centres, or none (a pre-1.2 bundle, or no bundle).
fn load_centers(src: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => Centers::from_bundle(b)?,
        None => Centers::default(),
    }))
}

/// Metres in the client's light-year: `appConst.LIGHTYEAR = 9460000000000000.0`
/// **[V]** (not the 9.4607e15 of the real year). The server must use the
/// client's own number, or a jump the UI calls 3.5 ly is refused here.
pub const LIGHTYEAR: f64 = 9.46e15;

/// Known space: `30000000 <= id <= 30999999` (`inventorycommon/const.py`
/// **[V]**). The client's `GetLightYearDistance` answers only for these.
pub const KNOWN_SPACE: std::ops::RangeInclusive<i64> = 30_000_000..=30_999_999;

/// The galaxy-frame centre, in metres, of every system, constellation and
/// region (`universe/centers.tsv`). Built before format 1.2 a bundle has no
/// file and this is empty: every distance answers `None`, "unknown".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Centers {
    systems: BTreeMap<i64, [f64; 3]>,
    constellations: BTreeMap<i64, [f64; 3]>,
    regions: BTreeMap<i64, [f64; 3]>,
}

impl Centers {
    /// The file from a checked bundle; absent is empty, not an error.
    pub fn from_bundle(b: &Bundle) -> Result<Centers, String> {
        Centers::parse(&b.data_lines(CENTERS, CENTERS_HEADER)?)
    }

    /// From the file's data lines. An unknown kind, a short row or a
    /// non-finite position is an error naming the row.
    pub fn parse(lines: &[String]) -> Result<Centers, String> {
        let rows = parse_rows(CENTERS, CENTERS_HEADER, lines, |c| {
            Ok((
                c.f[0].to_owned(),
                c.i64(1)?,
                [c.f64(2)?, c.f64(3)?, c.f64(4)?],
            ))
        })?;
        let mut out = Centers::default();
        for (kind, id, at) in rows {
            match kind.as_str() {
                "s" => out.systems.insert(id, at),
                "c" => out.constellations.insert(id, at),
                "r" => out.regions.insert(id, at),
                other => return Err(format!("{CENTERS}: unknown kind {other:?}")),
            };
        }
        Ok(out)
    }

    pub fn system(&self, id: i64) -> Option<[f64; 3]> {
        self.systems.get(&id).copied()
    }

    pub fn constellation(&self, id: i64) -> Option<[f64; 3]> {
        self.constellations.get(&id).copied()
    }

    pub fn region(&self, id: i64) -> Option<[f64; 3]> {
        self.regions.get(&id).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }

    /// **Light years between two systems**, the client's
    /// `GetLightYearDistance` **[V]** (`solar_systems.py:171-181`): the
    /// distance of the centres over [`LIGHTYEAR`]. `None` unless both are
    /// known-space systems this file holds.
    pub fn ly(&self, a: i64, b: i64) -> Option<f64> {
        if !KNOWN_SPACE.contains(&a) || !KNOWN_SPACE.contains(&b) {
            return None;
        }
        let (p, q) = (self.system(a)?, self.system(b)?);
        let d2: f64 = (0..3).map(|i| (p[i] - q[i]) * (p[i] - q[i])).sum();
        Some(d2.sqrt() / LIGHTYEAR)
    }

    /// Every known-space system within `max` ly of `from`, itself left out,
    /// nearest first (ties by id). Inclusive of `max`. A linear scan: about
    /// 5,500 square roots a query **[I]**.
    pub fn systems_within_ly(&self, from: i64, max: f64) -> Vec<(i64, f64)> {
        let mut out: Vec<(i64, f64)> = self
            .systems
            .keys()
            .filter(|&&s| s != from)
            .filter_map(|&s| self.ly(from, s).map(|d| (s, d)))
            .filter(|&(_, d)| d <= max)
            .collect();
        out.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        out
    }
}

#[cfg(test)]
mod centers_tests {
    use super::*;

    // Invented ids and positions only.
    fn lines(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|r| r.to_string()).collect()
    }

    fn fixture() -> Centers {
        Centers::parse(&lines(&[
            "s\t30999001\t0.0\t0.0\t0.0",
            "s\t30999002\t9.46e15\t0.0\t0.0",
            "s\t30999003\t0.0\t2.838e16\t0.0",
            "s\t30999004\t0.0\t0.0\t9.46e16",
            "s\t31999001\t1.0\t0.0\t0.0",
            "c\t20999001\t1.0\t2.0\t3.0",
            "r\t10999001\t4.0\t5.0\t6.0",
        ]))
        .unwrap()
    }

    #[test]
    fn centers_file_parses_three_kinds() {
        let c = fixture();
        assert_eq!(c.system(30_999_002), Some([9.46e15, 0.0, 0.0]));
        assert_eq!(c.constellation(20_999_001), Some([1.0, 2.0, 3.0]));
        assert_eq!(c.region(10_999_001), Some([4.0, 5.0, 6.0]));
        assert_eq!(c.region(30_999_001), None);
    }

    #[test]
    fn centers_missing_file_is_none_not_error() {
        let empty = Centers::parse(&[]).unwrap();
        assert!(empty.is_empty());
        assert_eq!(empty.ly(30_999_001, 30_999_002), None);
        assert!(empty.systems_within_ly(30_999_001, 10.0).is_empty());
    }

    #[test]
    fn centers_refuse_a_bad_row() {
        assert!(Centers::parse(&lines(&["x\t1\t0.0\t0.0\t0.0"])).is_err());
        assert!(Centers::parse(&lines(&["s\t1\t0.0\t0.0"])).is_err());
        assert!(Centers::parse(&lines(&["s\t1\tinf\t0.0\t0.0"])).is_err());
    }

    #[test]
    fn ly_of_a_system_to_itself_is_zero() {
        assert_eq!(fixture().ly(30_999_001, 30_999_001), Some(0.0));
    }

    #[test]
    fn ly_uses_9_46e15_not_9_4607e15() {
        assert_eq!(fixture().ly(30_999_001, 30_999_002), Some(1.0));
        assert_eq!(fixture().ly(30_999_002, 30_999_001), Some(1.0));
    }

    #[test]
    fn ly_refuses_a_non_known_space_system() {
        let c = fixture();
        assert_eq!(c.ly(30_999_001, 31_999_001), None);
        assert_eq!(c.ly(31_999_001, 30_999_001), None);
        assert_eq!(c.ly(30_999_001, 30_999_777), None);
    }

    #[test]
    fn systems_within_ly_is_inclusive_and_sorted() {
        let c = fixture();
        let got = c.systems_within_ly(30_999_001, 3.0);
        assert_eq!(
            got.iter().map(|g| g.0).collect::<Vec<_>>(),
            [30_999_002, 30_999_003]
        );
        assert_eq!(got[0].1, 1.0);
        assert_eq!(got[1].1, 3.0);
        // 3.0 ly exactly is inside; 10 ly reaches the far one; wormhole space never.
        assert_eq!(c.systems_within_ly(30_999_001, 10.0).len(), 3);
        assert_eq!(c.systems_within_ly(30_999_001, 0.5).len(), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `star_of` is the first star row of the system by id, as the scan it
    /// replaced was, and `None` for a system with none.
    #[test]
    fn star_of_is_the_lowest_star_of_the_system() {
        let c = Catalogue::parse(
            "star\t7\t1\t5.0\t1\t2\t3\t100\t0\t0\n\
             star\t3\t1\t9.0\t4\t5\t6\t100\t0\t0\n\
             star\t5\t1\t2.0\t7\t8\t9\t200\t0\t0\n\
             planet\t4\t1\t2.0\t7\t8\t9\t300\t0\t0\n",
        );
        assert_eq!(c.star_of(100).map(|s| s.id), Some(3));
        assert_eq!(c.star_of(200).map(|s| s.id), Some(5));
        assert!(c.star_of(300).is_none());
        assert!(c.star_of(400).is_none());
    }

    /// The real extract's shape, abbreviated. Tab-separated, as the tool
    /// writes it.
    const SAMPLE: &str = "\
# solarSystemID\t30000001
#kind\tid\ttype_id\tradius\tx\ty\tz
star\t40000001\t45041\t63350000.0\t0.0\t0.0\t0.0
planet\t40000002\t11\t5060000.0\t161891117336.0\t21288951986.0\t-73529712226.0
moon\t40000004\t14\t270000.0\t162088094286.0\t21314854783.0\t-73598621491.0
belt\t40000003\t15\t0.0\t161967513600.0\t21288837120.0\t-73505464320.0
station\t60014437\t2502\t0.0\t331522498560.0\t43594997760.0\t-586328186880.0
gate\t50000056\t29624\t0.0\t331516354560.0\t43597455360.0\t-586353991680.0
";

    /// Every row is read, and five kinds reach the park **from a
    /// seven-column file**: the star too (the sun fix), a global ball like a
    /// planet.
    ///
    /// The one exclusion is the content of this test, and it has a reason
    /// that is not "we did not get to it": a gate this file names no partner
    /// for could only be served without a `jumps` row, which makes the client
    /// raise `IndexError` on a double-click. Asserted as kinds rather than
    /// ids so that adding a planet to the extract does not touch it.
    #[test]
    fn the_catalogue_keeps_everything_and_the_park_takes_the_star_too() {
        let c = Catalogue::parse(SAMPLE);
        assert_eq!(c.len(), 6, "every row is kept, including the unserved");
        let served: Vec<Kind> = c.served().map(|x| x.kind).collect();
        assert_eq!(served.len(), 5);
        assert!(
            served.contains(&Kind::Star),
            "the sun is a ball: the client warps to it, and the hub launches at its warp-in point"
        );
        assert!(
            !served.contains(&Kind::Gate),
            "a gate with no partner has no jumps row, and that raises IndexError on double-click"
        );
        assert!(
            served.contains(&Kind::Station),
            "every station at its own position (FX4): no station, no right-click warp to it"
        );
    }

    /// A celestial's position is read exactly, because the client already
    /// knows it.
    ///
    /// The client draws these from its own static data, so a ball placed
    /// anywhere else gives a bracket in one place and a model in another.
    /// That makes exact parsing the property, and these are large numbers
    /// where a lazy `f32` would quietly lose metres.
    #[test]
    fn positions_are_read_exactly() {
        let c = Catalogue::parse(SAMPLE);
        let planet = c.get(40_000_002).expect("the first planet");
        assert_eq!(
            planet.position,
            [161_891_117_336.0, 21_288_951_986.0, -73_529_712_226.0]
        );
        assert_eq!(planet.radius, 5_060_000.0);
        assert_eq!(planet.type_id, 11);
        assert_eq!(c.get(40_000_003).expect("the belt").radius, 0.0);
    }

    /// A malformed line costs that line and nothing else.
    ///
    /// The catalogue is a generated convenience; refusing to start over one
    /// bad row would take the server down for a cosmetic feature.
    #[test]
    fn a_bad_row_does_not_take_the_catalogue_with_it() {
        let c = Catalogue::parse(
            "planet\t1\t11\t1.0\t0.0\t0.0\t0.0\n\
             planet\tnot-a-number\t11\t1.0\t0.0\t0.0\t0.0\n\
             short\t2\n\
             planet\t3\t11\t1.0\tnan\t0.0\t0.0\n\
             moon\t4\t14\t1.0\t5.0\t6.0\t7.0\n",
        );
        assert_eq!(c.len(), 2, "the good rows, and only those: {c:?}");
        assert!(c.get(1).is_some() && c.get(4).is_some());
        assert!(c.get(3).is_none(), "a NaN position is not a position");
    }

    /// No catalogue is the ordinary case and behaves as before.
    #[test]
    fn an_absent_catalogue_is_empty_rather_than_an_error() {
        let c = Catalogue::default();
        assert!(c.is_empty());
        assert_eq!(c.get(40_000_002), None);
        assert_eq!(c.served().count(), 0);
        assert_eq!(Catalogue::parse("").len(), 0);
        assert_eq!(Catalogue::parse("# only a comment\n").len(), 0);
    }

    /// Two systems in the P3.3 format, **synthetic coordinates throughout**
    /// (the no-client-data rule). System 1 in constellation 10 / region 100;
    /// system 2 in constellation 20 / region 100. Gate 51 in system 1 and
    /// gate 52 in system 2 are partners; gate 53 in system 2 leads to a
    /// gate (99, in system 9) this file does not hold.
    const TWO: &str = "\
#kind\tid\ttype_id\tradius\tx\ty\tz\tsystem\tdest\tdest_system
#system\t1\t10\t100
#system\t2\t20\t100
star\t11\t6\t1000.0\t0.0\t0.0\t0.0\t1\t0\t0
planet\t12\t11\t500.0\t100000.0\t0.0\t0.0\t1\t0\t0
gate\t51\t29624\t2473.0\t5000000.0\t0.0\t0.0\t1\t52\t2
star\t21\t6\t1000.0\t0.0\t0.0\t0.0\t2\t0\t0
planet\t22\t11\t500.0\t0.0\t100000.0\t0.0\t2\t0\t0
gate\t52\t29624\t2473.0\t0.0\t-5000000.0\t0.0\t2\t51\t1
gate\t53\t17\t2323.0\t0.0\t0.0\t5000000.0\t2\t99\t9
";

    /// **Every format the tool has ever written still parses** -- the pin the
    /// P3.3 brief asked for. Seven columns with the single-system header,
    /// eight from the old `--all`, nine, and ten from today's tool: the same planet reads the
    /// same in all three, and the system comes from the column when there is
    /// one and from the header when there is not.
    #[test]
    fn seven_eight_and_nine_column_extracts_all_parse() {
        let seven = "# solarSystemID\t7\n#kind\tid\ttype_id\tradius\tx\ty\tz\n\
                     planet\t12\t11\t500.0\t1.0\t2.0\t3.0\n";
        let eight = "#kind\tid\ttype_id\tradius\tx\ty\tz\tsystem\n\
                     planet\t12\t11\t500.0\t1.0\t2.0\t3.0\t8\n";
        let nine = "#kind\tid\ttype_id\tradius\tx\ty\tz\tsystem\tdest\n\
                    planet\t12\t11\t500.0\t1.0\t2.0\t3.0\t9\t0\n";
        let ten = "#kind\tid\ttype_id\tradius\tx\ty\tz\tsystem\tdest\tdest_system\n\
                   #system\t10\t1\t2\n\
                   planet\t12\t11\t500.0\t1.0\t2.0\t3.0\t10\t0\t0\n";
        for (text, system) in [(seven, 7), (eight, 8), (nine, 9), (ten, 10)] {
            let c = Catalogue::parse(text);
            assert_eq!(c.len(), 1, "{text}");
            let p = c.get(12).expect("the planet");
            assert_eq!(p.position, [1.0, 2.0, 3.0]);
            assert_eq!(p.radius, 500.0);
            assert_eq!(c.system_of(12), Some(system));
            assert_eq!(c.served_in(system).count(), 1);
        }
        // A seven-column file with no header at all: system 0, served
        // everywhere, which is what it was before a second system existed.
        let bare = Catalogue::parse("planet\t12\t11\t500.0\t1.0\t2.0\t3.0\n");
        assert_eq!(bare.system_of(12), Some(0));
        assert_eq!(bare.served_in(30_000_001).count(), 1);
    }

    /// The `#system` lines are read, and only they: the header comment and
    /// the column line are not systems.
    #[test]
    fn the_system_lines_are_read() {
        let c = Catalogue::parse(TWO);
        assert_eq!(
            c.system(2),
            Some(&SystemRow {
                id: 2,
                constellation_id: 20,
                region_id: 100
            })
        );
        assert_eq!(c.system(1).map(|s| s.constellation_id), Some(10));
        assert_eq!(c.system(3), None);
        assert_eq!(c.len(), 7, "the comments are not rows");
    }

    /// A gate's `dest` resolves to a row in **the other** system, and a
    /// partner outside the file is "not loaded" rather than an error.
    #[test]
    fn a_gate_resolves_its_partner_in_the_other_system() {
        let c = Catalogue::parse(TWO);
        let (gate, partner) = c.gate(51).expect("51 and 52 are both loaded");
        assert_eq!(gate.id, 51);
        assert_eq!(partner.id, 52);
        assert_eq!(c.system_of(partner.id), Some(2));
        assert_eq!(c.gate(52).map(|(_, p)| p.id), Some(51));
        assert_eq!(c.gate(53), None, "its partner is not in this file");
        assert_eq!(
            c.jump_of(53),
            Some((99, 9)),
            "but its slim can still say where it goes"
        );
        assert_eq!(c.jump_of(51), Some((52, 2)));
        assert_eq!(c.gate(12), None, "a planet is not a gate");
    }

    /// **Each system's park holds its own furniture and none of the
    /// other's**, and a gate is served when it can say where it goes --
    /// loaded or not, because its slim's `jumps` row needs only the ids.
    #[test]
    fn each_system_serves_only_its_own_furniture() {
        let c = Catalogue::parse(TWO);
        let one: Vec<i64> = c.served_in(1).map(|x| x.id).collect();
        let two: Vec<i64> = c.served_in(2).map(|x| x.id).collect();
        assert_eq!(one, vec![11, 12, 51]);
        assert_eq!(two, vec![21, 22, 52, 53]);
        assert!(
            !one.contains(&21) && !two.contains(&11),
            "each star in its own system only"
        );
        assert!(c.is_served(51) && c.is_served(11), "gates yes, stars too");
    }

    /// **The per-system index answers what the walk over every row did**
    /// (lane perf-fight): a system's own served rows and the rows served
    /// everywhere (system `0`), interleaved by id, each once, and nothing
    /// unserved -- for every system in the file, one it does not hold, and
    /// system `0` itself.
    #[test]
    fn served_in_matches_the_walk_over_every_row() {
        let text = format!(
            "{TWO}planet\t5\t11\t500.0\t1.0\t2.0\t3.0\t0\t0\t0\n\
             planet\t30\t11\t500.0\t1.0\t2.0\t3.0\t0\t0\t0\n\
             sun\t31\t11\t500.0\t1.0\t2.0\t3.0\t0\t0\t0\n"
        );
        let c = Catalogue::parse(&text);
        let walk = |system: i64| -> Vec<i64> {
            c.rows
                .values()
                .filter(|r| r.is_served(&c.rows) && (r.system == system || r.system == 0))
                .map(|r| r.celestial.id)
                .collect()
        };
        for system in [0, 1, 2, 3] {
            let indexed: Vec<i64> = c.served_in(system).map(|x| x.id).collect();
            assert_eq!(indexed, walk(system), "system {system}");
        }
        assert_eq!(
            c.served_in(1).map(|x| x.id).collect::<Vec<_>>(),
            vec![5, 11, 12, 30, 51],
            "the everywhere rows interleave by id"
        );
    }

    /// A nine-column gate whose partner is not loaded names no system, so
    /// it cannot have a `jumps` row and is not served; one whose partner is
    /// loaded takes the system from the partner's row.
    #[test]
    fn a_nine_column_gate_needs_its_partner_row_for_a_system() {
        let c = Catalogue::parse(
            "gate\t51\t29624\t2473.0\t1.0\t0.0\t0.0\t1\t52\n\
             gate\t52\t29624\t2473.0\t2.0\t0.0\t0.0\t2\t51\n\
             gate\t53\t29624\t2473.0\t3.0\t0.0\t0.0\t2\t99\n",
        );
        assert_eq!(c.jump_of(51), Some((52, 2)));
        assert_eq!(c.jump_of(53), None);
        assert!(c.is_served(51) && !c.is_served(53));
    }

    // ---- the universe table (CP3b). Every row is invented (`schema.md`
    // §1.1's example ranges).

    /// `schema.md`'s invented example rows, one per file, plus a second
    /// system with a different wormhole class and no faction.
    fn lines(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|r| (*r).to_owned()).collect()
    }
    const SYS_ROW: &str = "39000001\t19000001\t29000001\t599001\t\t0.4567\tB1\t0\t1\t0\t0\t0\t0\t\t0\t\t990009\tExample Prime";
    const SYS_WH: &str = "39000002\t19000001\t29000001\t\t\t-1.0\t\t0\t0\t0\t0\t0\t0\t990004\t1\t2.5\t\tExample Deep";
    const REGION_ROW: &str = "19000001\t990011\t599001\t990007\t19000002,19000003\tExample Reach";
    const CONST_ROW: &str = "29000001\t19000001\t\t\t29000002\tExample Cluster";

    /// A fixture `systems.tsv` gives security, region and wormhole class --
    /// the packet's first test.
    #[test]
    fn a_systems_row_gives_security_region_and_wormhole_class() {
        let u = Universe::parse(
            &lines(&[SYS_ROW, SYS_WH]),
            &lines(&[REGION_ROW]),
            &lines(&[CONST_ROW]),
        )
        .unwrap();
        let s = u.system(39_000_001).expect("the example system");
        assert_eq!(s.security, 0.4567);
        assert_eq!((s.region_id, s.constellation_id), (19_000_001, 29_000_001));
        assert_eq!((s.faction_id, s.owner_faction), (Some(599_001), None));
        assert_eq!(s.security_class.as_deref(), Some("B1"));
        assert!(s.border && !s.hub);
        assert_eq!((s.sun_type_id, s.dscan_range), (Some(990_009), None));
        assert_eq!(s.security_class(), SecurityClass::High);
        // The system's own wormhole class wins; with none, the
        // constellation's (none here), then the region's.
        assert_eq!(u.wormhole_class(39_000_002), Some(990_004));
        assert_eq!(u.wormhole_class(39_000_001), Some(990_007));
        let deep = u.system(39_000_002).unwrap();
        assert!(deep.disallow_scanning);
        assert_eq!(deep.dscan_range, Some(2.5));
        assert_eq!(u.security_class(39_000_002), Some(SecurityClass::Zero));
        assert_eq!(
            u.region(19_000_001).map(|r| r.neighbours.clone()),
            Some(vec![19_000_002, 19_000_003])
        );
        assert_eq!(
            u.constellation(29_000_001).map(|c| c.region_id),
            Some(19_000_001)
        );
        // the nebula is the system's region's, from the new column
        assert_eq!(u.nebula_of(39_000_001), Some(990_011));
        assert_eq!(u.nebula_of(39_000_009), None);
        assert_eq!(u.system(39_000_009), None);
        assert_eq!(u.wormhole_class(39_000_009), None);
    }

    /// `SecurityClassFromLevel` and `calculate_pseudo_security`, transcribed
    /// [V] (`docs/re/damage-death.md` §7.2.1): zero-sec is `<= 0.0`, and
    /// anything just above zero is lifted to `0.05`, which is low sec.
    #[test]
    fn the_client_security_rule_is_transcribed() {
        assert_eq!(SecurityClass::from_level(0.0), SecurityClass::Zero);
        assert_eq!(SecurityClass::from_level(-0.5), SecurityClass::Zero);
        assert_eq!(SecurityClass::from_level(0.44), SecurityClass::Low);
        assert_eq!(SecurityClass::from_level(0.45), SecurityClass::High);
        assert_eq!(SecurityClass::from_level(0.95), SecurityClass::Safe);
        assert_eq!(pseudo_security(0.01), 0.05);
        assert_eq!(pseudo_security(0.0), 0.0);
        assert_eq!(pseudo_security(0.05), 0.05);
        assert_eq!(pseudo_security(-0.01), -0.01);
        assert!(!SecurityClass::Zero.is_low_sec_or_higher());
        assert!(SecurityClass::Low.is_low_sec_or_higher());
        assert!(SecurityClass::Safe.is_low_sec_or_higher());
        // The fixture's near-zero system is low sec, not zero.
        let u = Universe::fixture();
        for s in u.systems() {
            assert_eq!(
                s.security_class() == SecurityClass::Zero,
                s.security <= 0.0,
                "{s:?}"
            );
        }
    }

    /// The band follows the class, and the id ranges take wormhole and void
    /// space first (C3 §3).
    #[test]
    fn the_band_follows_the_class_and_the_id_ranges() {
        let at = |id: i64, security: f64| System {
            id,
            security,
            ..System::default()
        };
        assert_eq!(at(39_000_001, 0.95).band(), Band::High);
        assert_eq!(at(39_000_001, 0.5).band(), Band::High);
        assert_eq!(at(39_000_001, 0.01).band(), Band::Low);
        assert_eq!(at(39_000_001, 0.0).band(), Band::Null);
        assert_eq!(at(WORMHOLE_SYSTEMS.start, -1.0).band(), Band::Wormhole);
        assert_eq!(at(VOID_SYSTEMS.start, -1.0).band(), Band::Void);
        assert_eq!(at(VOID_SPACE_SYSTEMS.start, -1.0).band(), Band::Void);
        assert_eq!(at(HIDDEN_SYSTEMS.start, 1.0).band(), Band::Void);
    }

    /// A bad header or a malformed row is an error naming the file, never a
    /// panic; an absent file is an empty table.
    #[test]
    fn a_bad_universe_file_is_an_error_naming_it() {
        let e = Universe::parse(&lines(&["39000001\t1"]), &[], &[]).unwrap_err();
        assert!(e.contains(SYSTEMS), "{e}");
        let bad = SYS_ROW.replace("0.4567", "high");
        let e = Universe::parse(&lines(&[&bad]), &[], &[]).unwrap_err();
        assert!(e.contains(SYSTEMS) && e.contains("column 6"), "{e}");
        let e = Universe::parse(&[], &lines(&["19000001\t\t\tx,y\tName"]), &[]).unwrap_err();
        assert!(e.contains(REGIONS), "{e}");

        let dir = std::env::temp_dir().join(format!("eve-cp3b-universe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("universe")).unwrap();
        let b = Bundle { dir: dir.clone() };
        let empty = Universe::from_bundle(&b);
        std::fs::write(
            dir.join(SYSTEMS),
            format!("{SYSTEMS_HEADER}\n# a comment\n{SYS_ROW}\n"),
        )
        .unwrap();
        let one = Universe::from_bundle(&b);
        std::fs::write(dir.join(REGIONS), "#region_id\tname\n1\tx\n").unwrap();
        let authored = crate::authored::Dir::compiled_only();
        let spec = (UNIVERSE.load)(&Sources::new(Some(&b), &authored)).err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(empty.unwrap().is_empty(), "no files, no rows");
        assert_eq!(one.unwrap().len(), 1);
        assert!(spec.is_some_and(|e| e.contains(REGIONS) && e.contains("header")));
    }

    /// No bundle gives the fixture, through the registered spec.
    #[test]
    fn no_bundle_gives_the_fixture() {
        let authored = crate::authored::Dir::compiled_only();
        let t = (UNIVERSE.load)(&Sources::new(None, &authored)).unwrap();
        assert_eq!(t.downcast_ref::<Universe>(), Some(&Universe::fixture()));
        assert!(!Universe::fixture().is_empty());
    }
}
