//! Owned by the scan lane (docs/lanes/scan.md).
//!
//! **Scanning: the directional scanner and probes** (P5.10). Two halves,
//! one module, because both are "which balls in this system can this pilot
//! know about, and how well":
//!
//! - **The directional scanner** -- `ConeScan` on the system scan manager,
//!   answered synchronously with every ball in range and cone. Cheap, and the
//!   first thing a PvP pilot presses.
//! - **Probe scanning** -- probes launched from a launcher
//!   (`dogmaIM.LaunchProbes`), moved and scanned with `RequestScans`, the
//!   results pushed back with a signal strength, and a perfect result warped
//!   to with `CmdWarpToStuff('scan', targetID)`. Scoped to **finding other
//!   ships**: no cosmic signature exists in this world yet, so there is
//!   nothing else for a probe to find (`docs/lanes/scan.md` §5 says what a
//!   site would need).
//!
//! Every call and push shape below is read off the client's own bytecode;
//! the file:line is beside each. The **server's** side of a probe scan --
//! how strong a signal is, how far off an uncertain result sits -- is not in
//! the client at all, and every number of that kind is marked **[A]**.
//!
//! **No probe is a ball.** The client keeps probes in `probeTracker.probeData`
//! from `OnNewProbe` and draws them on the map from there; the one ballpark
//! read (`scanSvc.RecoverProbes:380`, `GetBall(id).surfaceDist`) is inside a
//! `try/except AttributeError` **[V]**. So `destiny.rs` is untouched: a probe
//! is a record here and a `KeyVal` on the wire, nothing else.

use crate::content::Content;
use crate::model::Target;
use crate::notify::Push;
use crate::pyutil::{as_bytes, dict_get, keyval};
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// One AU in metres, `probescanning/const.pyc` `AU` **[V]**.
pub const AU: f64 = 149_597_870_700.0;

/// The directional scanner's fallback range in AU:
/// `directionalScanUtil._MAX_RANGE_AU = 14.3` **[V]**. The client clamps the
/// slider to this itself; the server clamps too, so a modified client cannot
/// see further.
pub const MAX_DSCAN_AU: f64 = 14.3;

/// Whether `system` forbids the directional scanner: the bundle's
/// `disallow_scanning` column (`is_scanning_suppressed`,
/// `solar_systems.py:113` **[V]**). A system the table lacks is not
/// suppressed.
pub fn scanning_suppressed(content: &crate::content::Content, system: i64) -> bool {
    content
        .try_get::<crate::celestial::Universe>()
        .and_then(|u| u.system(system))
        .is_some_and(|s| s.disallow_scanning)
}

/// `MAX_PROBES`, `probescanning/const.pyc` **[V]**: the most probes one pilot
/// may have out. The client's formations are built for it.
pub const MAX_PROBES: usize = 8;

/// `probeStateIdle`, `probescanning/const.pyc` **[V]**. What a new probe's
/// `state` says; `probeTracker.AddProbe:48` sets it again itself.
pub const PROBE_STATE_IDLE: i64 = 1;

/// The certainty ladder, `probescanning/const.pyc` **[V]**:
/// `probeResultPerfect`, `probeResultInformative`, `probeResultGood`,
/// `probeResultUnusable`. The client gates on the first and third itself
/// (`util.IsPerfectResult`, `results._SetIdentifiedState`); **[A]** that the
/// server discloses group at "good", type at "informative" and the item at
/// "perfect", and drops anything below "unusable".
pub const RESULT_PERFECT: f64 = 1.0;
pub const RESULT_INFORMATIVE: f64 = 0.75;
pub const RESULT_GOOD: f64 = 0.25;
pub const RESULT_UNUSABLE: f64 = 0.001;

/// `probeScanGroupShips`, `probescanning/const.pyc` **[V]**: the scan group a
/// ship result is filed under. `scanHandler.ClearCombatTargets:132` reads it
/// by key, and `COMBAT_TARGETS` includes it.
pub const SCAN_GROUP_SHIPS: i64 = 4;

/// `groupScanProbeLauncher` is 481 and a scanner probe's group is
/// `groupScannerProbe` 479 (`inventorycommon/const.pyc:925`) **[V]**; the
/// launchers' own rows say 481 **[V-data]** (`types.fsdbinary`, 17938 and
/// 18639).
pub const GROUP_SCAN_PROBE_LAUNCHER: i64 = 481;
pub const GROUP_SCANNER_PROBE: i64 = 479;

/// `CmdWarpToStuff`'s subject for a scan result: `menusvc._WarpXToScanResult`
/// sends `('scan', scanResultID)` when the result is not a known site
/// (`menusvc.pyc:1700`) **[V]**, and the id is the result's own `id` -- a
/// string, which is why this cannot go through `pyutil::opt_id`.
pub const WARP_SUBJECT_SCAN: &[u8] = b"scan";

/// How long one probe scan takes, in milliseconds.
///
/// `SCAN_DURATION_ATTRIBUTE_ID` is 73 (`probescanning/const.pyc`) **[V]**,
/// the `duration` attribute, and both scan probe launchers carry
/// `duration 10000` **[V-data]** (`tools/dogma.py type 17938`, `18639`).
/// Skills and implants that shorten it are not modelled.
pub const SCAN_DURATION_MS: i64 = 10_000;

/// Group ids for the system's furniture, `inventorycommon/const.pyc`
/// (`groupSun` 6, `groupPlanet` 7, `groupMoon` 8, `groupAsteroidBelt` 9,
/// `groupStargate` 10, `groupStation` 15, `groupWreck` 186) **[V]**.
///
/// **The d-scan window filters on these** (`directionalScannerPalette.
/// UpdateScroll:800`, `celestialRec.groupID not in filters`), so a wrong one
/// is not a wrong label: it is a row counted as "filtered" and never shown.
pub const GROUP_SUN: i64 = 6;
pub const GROUP_PLANET: i64 = 7;
pub const GROUP_MOON: i64 = 8;
pub const GROUP_ASTEROID_BELT: i64 = 9;
pub const GROUP_STARGATE: i64 = 10;
pub const GROUP_STATION: i64 = 15;
pub const GROUP_WRECK: i64 = 186;

/// Probe ids, from a range of their own **[A]**.
///
/// A probe is not a store item (it is consumed out of the launcher and comes
/// back as a fresh charge on recovery), so it cannot take an id from the
/// store's allocator, and must not collide with one. Above every item id
/// this server mints; `probeTracker` formats it with `%d`, so an int.
pub const PROBE_ID_BASE: i64 = 9_000_000_000_000;

/// One scanner probe type's numbers: the probe's own attributes ([`attr`]),
/// read by [`probe_type_in`] from the loaded type data (CP58e, ruling 3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProbeType {
    pub type_id: i64,
    /// `baseSensorStrength`.
    pub strength: f64,
    /// `baseScanRange`, in AU: the smallest of the eight range steps
    /// (`probeTracker.GetScanRangeStepsByTypeID:248`, `baseScanRange *
    /// rangeFactor ** i * AU` for `i` in `range(8)`) **[V]**.
    pub base_range_au: f64,
    /// `rangeFactor`.
    pub range_factor: f64,
    /// `probeCanScanShips` -- present (1) on the combat probe and absent on
    /// the core probe.
    pub can_scan_ships: bool,
    /// `explosionDelay`, in milliseconds: how long a probe lives.
    pub lifetime_ms: i64,
}

/// The two scanner probes as they were hand-copied (P5.10).
///
/// **Retired by CP58e (O3, ruling 3)**: this module reads
/// [`probe_type_in`]. They stay only for [`probe_type`] and [`certainty`],
/// which `crate::signal_tracker` still calls without `Content`; X2 deletes
/// all three once it calls [`certainty_in`].
pub const PROBE_TYPES: &[ProbeType] = &[
    // Core Scanner Probe I: finds sites, **not ships**.
    ProbeType {
        type_id: 30013,
        strength: 40.0,
        base_range_au: 0.25,
        range_factor: 2.0,
        can_scan_ships: false,
        lifetime_ms: 4_000_000,
    },
    // Combat Scanner Probe I: finds ships.
    ProbeType {
        type_id: 30028,
        strength: 20.0,
        base_range_au: 0.5,
        range_factor: 2.0,
        can_scan_ships: true,
        lifetime_ms: 4_000_000,
    },
];

/// A retired [`PROBE_TYPES`] row (X2 deletes it). New code asks
/// [`probe_type_in`].
pub fn probe_type(type_id: i64) -> Option<&'static ProbeType> {
    PROBE_TYPES.iter().find(|t| t.type_id == type_id)
}

/// Scanner probe attribute ids (`dogma/const`) **[V]**.
pub mod attr {
    /// `explosionDelay`, ms: how long a probe lives.
    pub const EXPLOSION_DELAY: i64 = 281;
    /// `baseScanRange`, AU.
    pub const BASE_SCAN_RANGE: i64 = 1370;
    /// `baseSensorStrength`.
    pub const BASE_SENSOR_STRENGTH: i64 = 1371;
    /// `rangeFactor`.
    pub const RANGE_FACTOR: i64 = 1373;
    /// `probeCanScanShips`: present (1) on a probe that finds ships.
    pub const PROBE_CAN_SCAN_SHIPS: i64 = 1413;
}

/// A scanner probe (group [`GROUP_SCANNER_PROBE`]) by type id, from the
/// loaded type data (CP58e, ruling 3); `None` for any other type.
pub fn probe_type_in(content: &Content, type_id: i64) -> Option<ProbeType> {
    let table = crate::typedata::installed_in(content).table();
    let row = table.get(type_id)?;
    if row.group_id != GROUP_SCANNER_PROBE {
        return None;
    }
    let a = |id| table.attribute(type_id, id);
    Some(ProbeType {
        type_id,
        strength: a(attr::BASE_SENSOR_STRENGTH),
        base_range_au: a(attr::BASE_SCAN_RANGE),
        range_factor: a(attr::RANGE_FACTOR),
        can_scan_ships: row
            .attribute(attr::PROBE_CAN_SCAN_SHIPS)
            .is_some_and(|v| v != 0.0),
        lifetime_ms: a(attr::EXPLOSION_DELAY) as i64,
    })
}

/// A type's group: the loaded type data's row (with a bundle that is every
/// category, wrecks included), then the hand rows `crate::item::find_in`
/// keeps.
pub fn group_of(content: &Content, type_id: i64) -> Option<i64> {
    crate::typedata::installed_in(content)
        .table()
        .get(type_id)
        .map(|r| r.group_id)
        .or_else(|| crate::item::find_in(content, type_id).map(|t| t.group_id))
}

/// Whether a module of this type launches probes ([`group_of`]).
pub fn is_probe_launcher_in(content: &Content, type_id: i64) -> bool {
    group_of(content, type_id) == Some(GROUP_SCAN_PROBE_LAUNCHER)
}

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

fn num(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Real(x) => Some(*x),
        PyValue::Int(i) => Some(*i as f64),
        PyValue::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

fn vec3(v: Option<&PyValue>) -> Option<[f64; 3]> {
    match v? {
        PyValue::Tuple(xs) | PyValue::List(xs) if xs.len() == 3 => {
            Some([num(xs.first())?, num(xs.get(1))?, num(xs.get(2))?])
        }
        _ => None,
    }
}

fn tuple3(p: [f64; 3]) -> PyValue {
    PyValue::Tuple(p.iter().map(|x| PyValue::Real(*x)).collect())
}

fn bad_args(method: &str, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: "scanMgr::systemScanMgr".to_owned(),
        method: method.to_owned(),
        why: why.to_owned(),
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn len(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

// ---------------------------------------------------------------------------
// The directional scanner
// ---------------------------------------------------------------------------

/// One ball the scanner can see: what the d-scan row and the probe result
/// both need.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seen {
    pub id: i64,
    pub type_id: i64,
    pub group_id: i64,
    pub position: [f64; 3],
}

/// Whether `at` is inside the cone.
///
/// `angle` is the **whole** cone, in radians: the slider stores
/// `math.radians(slider.GetValue())` (`directionalScannerPalette.
/// EndSetAngleSliderValue:749`) and the cone is drawn with `tan(angle * 0.5)`
/// either side of the axis (`directionalScanHandler.ShowCone:277`) **[V]**.
/// 360 degrees is a sphere (`DrawDirectionalScanCone:245-248`), which the
/// half-angle test reaches by itself at `half >= pi`.
///
/// **[A]** the distance is centre to centre, and a range is inclusive.
pub fn in_cone(
    origin: [f64; 3],
    direction: [f64; 3],
    angle: f64,
    range: f64,
    at: [f64; 3],
) -> bool {
    let v = sub(at, origin);
    let d = len(v);
    if d > range {
        return false;
    }
    let half = angle * 0.5;
    if half >= std::f64::consts::PI {
        return true;
    }
    let n = len(direction);
    if d == 0.0 || n == 0.0 {
        // On top of the scanner: no direction to be outside of. A zero
        // direction is the client's "no camera" case, which it refuses
        // itself (`DirectionalScan:48`, `if not direction: return`).
        return d == 0.0;
    }
    let cos = (v[0] * direction[0] + v[1] * direction[1] + v[2] * direction[2]) / (d * n);
    cos >= half.cos()
}

/// Every ball in a solar system that this server puts in *some* ballpark, in
/// `destiny::scene`'s order: ships (players and NPCs), celestials, rocks,
/// wrecks, and an uncatalogued home station -- the whole system, not the
/// scanner's grid (`crate::grid`). **A ball off the scanner's grid is still
/// a result**, as in real EVE: the palette primes `cfg.evelocations` for
/// every result id not in the park, falls back to the type name on the
/// `KeyError`, and shows the distance as `-` when `GetBall` is `None`
/// (`directionalScannerPalette.UpdateScroll:784-795`, `GetScrollEntry:826`,
/// `:833`) **[V]** (`docs/lanes/grid-research.md` §0.4).
///
/// `w.advance_system` first, so ship positions are now and not as of their
/// last command. The caller's own station is added at the origin of their
/// home system only when the catalogue has no row for it.
pub fn balls_in_system(w: &mut Registry, solar_system_id: i64, actor: i64) -> Vec<Seen> {
    w.advance_system(solar_system_id);
    let mut out = Vec::new();
    for p in w.roster().in_system(solar_system_id) {
        // CLOAK-1: a cloaked ship is on nobody's d-scan but its own.
        if p.character_id != actor && crate::cloak::is_cloaked(w, p.ship_id) {
            continue;
        }
        if let Some(n) = w.npcs().get(p.character_id) {
            out.push(Seen {
                id: p.ship_id,
                type_id: n.kind.type_id,
                // The NPC's own row (`npc::NpcType::group_id`).
                group_id: n.kind.group_id,
                position: p.motion.position,
            });
            continue;
        }
        let Some(c) = w.character(p.character_id) else {
            continue;
        };
        out.push(Seen {
            id: p.ship_id,
            type_id: c.ship_type_id,
            group_id: item_group(w.content(), c.ship_type_id),
            position: p.motion.position,
        });
    }
    for c in w.celestials().served_in(solar_system_id) {
        out.push(Seen {
            id: c.id,
            type_id: c.type_id,
            group_id: celestial_group(w.content(), c.kind, c.type_id),
            position: c.position,
        });
    }
    for r in w.rocks().in_system(solar_system_id) {
        out.push(Seen {
            id: r.id,
            type_id: r.type_id,
            group_id: item_group(w.content(), r.type_id),
            position: r.position,
        });
    }
    for wr in w.combat().wrecks() {
        if wr.solar_system_id != solar_system_id {
            continue;
        }
        out.push(Seen {
            id: wr.wreck_id,
            type_id: wr.type_id,
            group_id: group_of(w.content(), wr.type_id).unwrap_or(GROUP_WRECK),
            position: wr.position,
        });
    }
    // A catalogued station came in with the furniture above (FX4); only an
    // uncatalogued home station, at the origin, is left to add.
    if let Some(c) = w.character(actor)
        && c.solar_system_id == Some(solar_system_id)
        && let Some(station_id) = c.station_id
        && !w.celestials().is_served(station_id)
    {
        out.push(Seen {
            id: station_id,
            type_id: c.station_type_id,
            group_id: GROUP_STATION,
            position: [0.0; 3],
        });
    }
    out
}

/// A type's group ([`group_of`]). **0 for a type the server has no row
/// for** -- which the d-scan window then counts as filtered rather than
/// showing under a wrong group.
fn item_group(content: &Content, type_id: i64) -> i64 {
    group_of(content, type_id).unwrap_or(0)
}

fn celestial_group(content: &Content, kind: crate::celestial::Kind, type_id: i64) -> i64 {
    use crate::celestial::Kind;
    match kind {
        Kind::Star => GROUP_SUN,
        Kind::Planet => GROUP_PLANET,
        Kind::Moon => GROUP_MOON,
        Kind::Belt => GROUP_ASTEROID_BELT,
        Kind::Gate => GROUP_STARGATE,
        Kind::Station => GROUP_STATION,
        Kind::Other => item_group(content, type_id),
    }
}

/// `ConeScan(scanAngle, scanRange, x, y, z)` on the system scan manager.
///
/// **Five positionals** -- `directionalScanSvc.DirectionalScan:54` is
/// `GetScanMan().ConeScan(scanAngle, scanRange, *direction)`, a
/// `CALL_FUNCTION_VAR 2` over a three-vector **[V]**: the angle in radians
/// (`directionalScanUtil.GetScanAngle:102`), the range in metres
/// (`GetScanRangeInMeters:62`, kilometres times 1000), and the camera's
/// negated look-at direction (`GetScanDirection:77-78`).
///
/// **The answer is a list of objects with `.id`, `.typeID` and `.groupID`**
/// -- the three attributes every consumer reads **[V]**:
/// `directionalScannerPalette.UpdateScroll:791-800` (`.id`, `.groupID`, and
/// `[:]` on the list itself), `GetScrollEntry:819-833` (`.typeID`, `.id`),
/// and `scanHologramHandler.GetTypeIDsToDisplay:115-116`. **[A]** a
/// `KeyVal` per row; nothing reads anything else off them.
///
/// **[A]** the scanner's own ship is left out: the scan is of what is
/// *around* you.
pub fn cone_scan(
    w: &mut Registry,
    actor: Option<i64>,
    args: &[PyValue],
) -> Result<PyValue, ServiceError> {
    let (Some(angle), Some(range)) = (num(args.first()), num(args.get(1))) else {
        return Err(bad_args("ConeScan", "(scanAngle, scanRange, x, y, z)"));
    };
    let (Some(x), Some(y), Some(z)) = (num(args.get(2)), num(args.get(3)), num(args.get(4))) else {
        return Err(bad_args("ConeScan", "(scanAngle, scanRange, x, y, z)"));
    };
    let Some(actor) = actor else {
        return Ok(PyValue::List(vec![]));
    };
    w.advance_character(actor);
    let Some(here) = w.roster().get(actor).copied() else {
        // Docked, or between systems: the window is closed while docked
        // (`OnDirectionalScannerDocked`), so an empty list is the truth.
        return Ok(PyValue::List(vec![]));
    };
    // **[A]** a suppressed system answers an empty list, as a docked scanner
    // does, not an error the client would show as a traceback. Falsifier:
    // the client's own refusal path for `disallowScanning` being a raise
    // (`solar_systems.py:113`'s caller).
    if scanning_suppressed(w.content(), here.solar_system_id) {
        return Ok(PyValue::List(vec![]));
    }
    // The client never sends more than `MAX_DSCAN_AU`; a larger range is a
    // modified client's, and is cut to it. NaN is cut to nothing.
    let range = if range.is_nan() {
        0.0
    } else {
        range.min(MAX_DSCAN_AU * AU)
    };
    let seen = balls_in_system(w, here.solar_system_id, actor);
    let rows = seen
        .into_iter()
        .filter(|s| s.id != here.ship_id)
        .filter(|s| in_cone(here.motion.position, [x, y, z], angle, range, s.position))
        .map(|s| {
            keyval(vec![
                ("id", PyValue::Int(s.id)),
                ("typeID", PyValue::Int(s.type_id)),
                ("groupID", PyValue::Int(s.group_id)),
            ])
        })
        .collect();
    Ok(PyValue::List(rows))
}

// ---------------------------------------------------------------------------
// Probes
// ---------------------------------------------------------------------------

/// One probe in space.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub probe_id: i64,
    pub type_id: i64,
    /// The character who launched it, and the only one told about it.
    pub owner_id: i64,
    pub solar_system_id: i64,
    pub position: [f64; 3],
    /// Metres; the client sends it back on every `RequestScans`.
    pub scan_range: f64,
    pub range_step: i64,
    /// Blue time it dies at.
    pub expiry: i64,
}

impl Probe {
    /// The object `OnNewProbe` and `OnSystemScanStarted` carry.
    ///
    /// Every attribute the client reads off one **[V]**: `probeID`, `typeID`
    /// and `pos` (`probeTracker.AddProbe:44-47`), `rangeStep` and
    /// `scanRange` (`:45-46`, which it overwrites), `destination` and `state`
    /// (`SetProbeDestination`, `UpdateProbeState:153`), and `expiry` --
    /// unguarded in `probeScannerPalette:1119`
    /// (`gametime.GetSecondsUntilSimTime(data.expiry)`), so a blue time and
    /// never `None`. **Written to** by the client (`AddProbe:45-47`), so a
    /// `KeyVal` and not a tuple.
    pub fn to_keyval(&self) -> PyValue {
        keyval(vec![
            ("probeID", PyValue::Int(self.probe_id)),
            ("typeID", PyValue::Int(self.type_id)),
            ("pos", tuple3(self.position)),
            ("destination", tuple3(self.position)),
            ("scanRange", PyValue::Real(self.scan_range)),
            ("rangeStep", PyValue::Int(self.range_step)),
            ("state", PyValue::Int(PROBE_STATE_IDLE)),
            ("expiry", PyValue::Int(self.expiry)),
        ])
    }
}

/// Every probe in the world, and what each pilot was last told.
///
/// **Not persisted**, like `crate::modules::Modules`: a restart empties
/// space, and a probe is a thing in space.
#[derive(Debug, Default)]
pub struct Probes {
    probes: BTreeMap<i64, Probe>,
    next: i64,
    /// Per scanning character, the result ids of their last scan -- what the
    /// next scan's `absentTargets` is measured against.
    last_results: BTreeMap<i64, Vec<String>>,
}

impl Probes {
    pub fn get(&self, probe_id: i64) -> Option<&Probe> {
        self.probes.get(&probe_id)
    }

    /// This pilot's live probes, in id order.
    pub fn owned_by(&self, owner_id: i64) -> impl Iterator<Item = &Probe> {
        self.probes.values().filter(move |p| p.owner_id == owner_id)
    }

    fn mint(&mut self) -> i64 {
        self.next += 1;
        PROBE_ID_BASE + self.next
    }

    /// Forget every probe whose time is up. The client is told by the
    /// [`RemoveProbe`] scheduled at launch.
    fn prune(&mut self, now: i64) {
        self.probes.retain(|_, p| p.expiry > now);
    }
}

/// `OnNewProbe(probe)` -- `scanSvc.OnNewProbe(self, probe)` **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct NewProbe {
    pub to: Vec<i64>,
    pub probe: PyValue,
}

impl Push for NewProbe {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnNewProbe", vec![self.probe.clone()])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnRemoveProbe(probeID)` -- `scanSvc.OnRemoveProbe(self, probeID)` **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoveProbe {
    pub to: Vec<i64>,
    pub probe_id: i64,
}

impl Push for RemoveProbe {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnRemoveProbe", vec![PyValue::Int(self.probe_id)])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnSystemScanStarted(startTime, durationMs, probes)` **[V]**
/// (`scanSvc.pyc:295`). `startTime` is sim time: the results window compares
/// it with `GetSimTime()` against `duration` (`scanResults.pyc:250`).
/// `probes` is a **dict** `{probeID: probe}`: `probeTracker.
/// SetProbesAsScanning:316` walks it with `iteritems()` and reads `.pos`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanStarted {
    pub to: Vec<i64>,
    pub start_time: i64,
    pub duration_ms: i64,
    pub probes: Vec<(i64, PyValue)>,
}

impl Push for ScanStarted {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnSystemScanStarted",
            vec![
                PyValue::Int(self.start_time),
                PyValue::Int(self.duration_ms),
                // Wire order: `(value, key)`.
                PyValue::Dict(
                    self.probes
                        .iter()
                        .map(|(id, p)| (p.clone(), PyValue::Int(*id)))
                        .collect(),
                ),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnSystemScanStopped(probes, results, absentTargets)` **[V]**
/// (`scanSvc.pyc:301`). `probes` is iterated for ids
/// (`scanHandler.OnSystemScanStopped:51`, `for pID in probes`); `results` is
/// iterated for `.certainty` and handed to `ResultsHistory.RegisterResults`;
/// `absentTargets` is star-expanded into `ClearResults(*absentTargets)`
/// (`:63`). An empty `results` raises the client's own "no results" dialog
/// (`scanSvc:308-309`).
#[derive(Debug, Clone, PartialEq)]
pub struct ScanStopped {
    pub to: Vec<i64>,
    pub probe_ids: Vec<i64>,
    pub results: Vec<PyValue>,
    pub absent: Vec<String>,
}

impl Push for ScanStopped {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnSystemScanStopped",
            vec![
                PyValue::List(self.probe_ids.iter().map(|i| PyValue::Int(*i)).collect()),
                PyValue::List(self.results.clone()),
                PyValue::List(
                    self.absent
                        .iter()
                        .map(|s| PyValue::Bytes(s.as_bytes().to_vec()))
                        .collect(),
                ),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `dogmaIM.LaunchProbes(moduleID, numProbes)` -- the probe scanner window's
/// launch, `scanSvc.FindModuleAndLaunchProbes:558` `dogmaLM.LaunchProbes(
/// module.itemID, numProbes)` **[V]**, reached from `MoveProbesToFormation`
/// when the formation needs more probes than are out. **The return is
/// discarded** (`POP_TOP`).
///
/// The client has already checked the launcher is online and loaded
/// (`:554`); we check again, because the call can be sent by anything.
/// Launches `min(numProbes, rounds loaded, MAX_PROBES - probes out)` at the
/// ship's position **[A]** -- the probes then move where the formation puts
/// them on the next `RequestScans`.
pub fn launch_probes(
    w: &mut Registry,
    actor: Option<i64>,
    args: &[PyValue],
    now: i64,
) -> Result<PyValue, ServiceError> {
    let (Some(PyValue::Int(module_id)), wanted) = (args.first(), args.get(1)) else {
        return Err(ServiceError::BadArguments {
            service: "dogmaIM".to_owned(),
            method: "LaunchProbes".to_owned(),
            why: "(moduleID, numProbes)".to_owned(),
        });
    };
    let wanted = match wanted {
        Some(PyValue::Int(n)) => *n,
        _ => MAX_PROBES as i64,
    };
    let Some(character_id) = actor else {
        return Ok(PyValue::None);
    };
    launch(w, character_id, *module_id, wanted, now);
    Ok(PyValue::None)
}

/// Launch up to `wanted` probes out of `module_id`. Returns how many went.
///
/// The shared half of [`launch_probes`] and of activating the launcher from
/// the HUD (`docs/lanes/scan.md` §3, the `modules.rs` hook).
pub fn launch(w: &mut Registry, character_id: i64, module_id: i64, wanted: i64, now: i64) -> usize {
    w.advance_character(character_id);
    let Some(here) = w.roster().get(character_id).copied() else {
        return 0;
    };
    let Some(module) = w.item(module_id).cloned() else {
        return 0;
    };
    if module.location_id != here.ship_id || !is_probe_launcher_in(w.content(), module.type_id) {
        return 0;
    }
    let Some(charge) = crate::modules::loaded(w.store(), here.ship_id, module.flag_id).cloned()
    else {
        return 0;
    };
    let Some(kind) = probe_type_in(w.content(), charge.type_id) else {
        return 0;
    };
    w.scan_mut().prune(now);
    let out = w.scan().owned_by(character_id).count();
    let n = wanted
        .min(charge.quantity)
        .min(MAX_PROBES.saturating_sub(out) as i64)
        .max(0);
    if n == 0 {
        return 0;
    }
    if w.apply_items_quiet(&[crate::item::Edit::Consume {
        item_id: charge.item_id,
        quantity: n,
    }])
    .is_err()
    {
        return 0;
    }
    // The HUD's count, the same push a gun's fired round sends.
    w.push(crate::modules::ChargeQuantity {
        to: vec![character_id],
        owner_id: character_id,
        ship_id: here.ship_id,
        flag_id: module.flag_id,
        type_id: charge.type_id,
        time: now,
        new_value: charge.quantity - n,
        old_value: charge.quantity,
    });
    // `probeTracker.lastRangeStepUsed` starts at 7 and `AddProbe` reads
    // `steps[lastRangeStepUsed - 1]` (`:28`, `:46`) **[V]**; it overwrites
    // both, and these are what it would write on a fresh session.
    let range_step = 7;
    let scan_range = kind.base_range_au * kind.range_factor.powi(range_step as i32 - 1) * AU;
    let expiry = now + kind.lifetime_ms * 10_000;
    for _ in 0..n {
        let probe_id = w.scan_mut().mint();
        let probe = Probe {
            probe_id,
            type_id: kind.type_id,
            owner_id: character_id,
            solar_system_id: here.solar_system_id,
            position: here.motion.position,
            scan_range,
            range_step,
            expiry,
        };
        let kv = probe.to_keyval();
        w.scan_mut().probes.insert(probe_id, probe);
        w.push(NewProbe {
            to: vec![character_id],
            probe: kv,
        });
        w.schedule(
            Duration::from_millis(kind.lifetime_ms.unsigned_abs()),
            RemoveProbe {
                to: vec![character_id],
                probe_id,
            },
        );
    }
    n as usize
}

/// What a `RequestScans` probe says about itself: `destination`, `scanRange`
/// and `rangeStep` off the `KeyVal` the client sends back.
fn probe_request(v: &PyValue) -> (Option<[f64; 3]>, Option<f64>, Option<i64>) {
    let state = match v {
        PyValue::Instance { state, .. } => state.as_ref(),
        other => other,
    };
    let destination =
        vec3(dict_get(state, b"destination")).or_else(|| vec3(dict_get(state, b"pos")));
    let range = num(dict_get(state, b"scanRange"));
    let step = match dict_get(state, b"rangeStep") {
        Some(PyValue::Int(s)) => Some(*s),
        _ => None,
    };
    (destination, range, step)
}

/// `RequestScans(probes)` -- `scanSvc.RequestScans:251` **[V]**: `probes` is
/// `probeTracker.GetProbesForScanning()`, a dict `{probeID: probe}` of the
/// idle ones (`:306`), or `None` when there are none. **The return is
/// discarded.**
///
/// What happens, in order:
///
/// 1. each owned probe **jumps** to its `destination` and takes the
///    `scanRange` it was given **[A]** -- the real probes warp there first
///    (`OnProbeWarpStart`, `probeStateWarping`), which is a delay this does
///    not model;
/// 2. `OnSystemScanStarted` goes out now, with the moved probes;
/// 3. the results are computed **now**, from where every ship is now
///    **[A]** -- the real scan reads the world at its end -- and
///    `OnSystemScanStopped` is scheduled [`SCAN_DURATION_MS`] later.
pub fn request_scans(
    w: &mut Registry,
    actor: Option<i64>,
    args: &[PyValue],
    now: i64,
) -> Result<PyValue, ServiceError> {
    let Some(character_id) = actor else {
        return Ok(PyValue::None);
    };
    w.scan_mut().prune(now);
    let mut moved: Vec<i64> = Vec::new();
    if let Some(PyValue::Dict(entries)) = args.first() {
        for (value, key) in entries {
            let PyValue::Int(probe_id) = key else {
                continue;
            };
            let (destination, range, step) = probe_request(value);
            let kind = w
                .scan()
                .get(*probe_id)
                .and_then(|p| probe_type_in(w.content(), p.type_id));
            let Some(p) = w.scan_mut().probes.get_mut(probe_id) else {
                continue;
            };
            if p.owner_id != character_id {
                continue;
            }
            if let Some(d) = destination {
                p.position = d;
            }
            if let Some(kind) = kind {
                let lo = kind.base_range_au * AU;
                let hi = lo * kind.range_factor.powi(7);
                if let Some(r) = range.filter(|r| r.is_finite()) {
                    p.scan_range = r.clamp(lo, hi);
                }
            }
            if let Some(s) = step {
                p.range_step = s;
            }
            moved.push(*probe_id);
        }
    }
    let probes: Vec<Probe> = moved
        .iter()
        .filter_map(|id| w.scan().get(*id).cloned())
        .collect();
    w.push(ScanStarted {
        to: vec![character_id],
        start_time: now,
        duration_ms: SCAN_DURATION_MS,
        probes: probes.iter().map(|p| (p.probe_id, p.to_keyval())).collect(),
    });
    // Every registered source's results, in table order (W10.5 seam S8).
    let results = crate::scan_sources::run(&crate::module::plan().scan, w, character_id, &probes);
    let ids: Vec<String> = results.iter().map(|r| r.target_id.clone()).collect();
    let before = w
        .scan_mut()
        .last_results
        .insert(character_id, ids.clone())
        .unwrap_or_default();
    let absent = before.into_iter().filter(|id| !ids.contains(id)).collect();
    w.schedule(
        Duration::from_millis(SCAN_DURATION_MS.unsigned_abs()),
        ScanStopped {
            to: vec![character_id],
            probe_ids: moved,
            results: results.iter().map(ProbeResult::to_keyval).collect(),
            absent,
        },
    );
    Ok(PyValue::None)
}

/// `RecoverProbes(probeIDs)` -- answers **the ids that are coming home**,
/// which `probeTracker.RecoverProbes:231-235` iterates and compares in
/// length with what it asked for, raising `NotAllProbesReturnedSuccessfully`
/// itself on a shortfall **[V]**.
///
/// **[A]** recovery is instant: each probe is gone at once
/// (`OnRemoveProbe`) and is back in the cargo hold as a charge, merged into
/// a stack of its type if there is one.
pub fn recover_probes(
    w: &mut Registry,
    actor: Option<i64>,
    args: &[PyValue],
    now: i64,
) -> Result<PyValue, ServiceError> {
    let Some(character_id) = actor else {
        return Ok(PyValue::List(vec![]));
    };
    w.scan_mut().prune(now);
    let ids: Vec<i64> = match args.first() {
        Some(PyValue::List(xs) | PyValue::Tuple(xs)) => xs
            .iter()
            .filter_map(|x| match x {
                PyValue::Int(i) => Some(*i),
                _ => None,
            })
            .collect(),
        _ => vec![],
    };
    let ship_id = w.roster().get(character_id).map(|p| p.ship_id);
    let mut back = Vec::new();
    for id in ids {
        if w.scan().get(id).is_none_or(|p| p.owner_id != character_id) {
            continue;
        }
        let Some(p) = w.scan_mut().probes.remove(&id) else {
            continue;
        };
        if let Some(ship_id) = ship_id {
            return_to_cargo(w, character_id, ship_id, p.type_id);
        }
        w.push(RemoveProbe {
            to: vec![character_id],
            probe_id: id,
        });
        back.push(PyValue::Int(id));
    }
    Ok(PyValue::List(back))
}

fn return_to_cargo(w: &mut Registry, character_id: i64, ship_id: i64, type_id: i64) {
    let stack = w
        .store()
        .contents_of(ship_id)
        .find(|i| i.flag_id == crate::fitting::FLAG_CARGO && i.type_id == type_id && !i.singleton)
        .map(|i| i.item_id);
    let edit = match stack {
        Some(item_id) => crate::item::Edit::Grow {
            item_id,
            quantity: 1,
        },
        None => crate::item::Edit::Create {
            type_id,
            quantity: 1,
            to: crate::item::Slot::new(character_id, ship_id, crate::fitting::FLAG_CARGO),
        },
    };
    // A refusal (an unknown type) loses the probe rather than the call:
    // the client has already let go of it.
    let _ = w.apply_items(&[edit]);
}

/// `DestroyProbe(probeID)` -- `scanSvc.DestroyProbe:319`, passed as the
/// callback to `probeTracker.DestroyProbe`, which deletes its own record
/// after (`:173-174`) **[V]**. Return discarded. Nothing is pushed: the
/// client already forgot it.
pub fn destroy_probe(w: &mut Registry, actor: Option<i64>, args: &[PyValue]) -> PyValue {
    if let (Some(character_id), Some(PyValue::Int(id))) = (actor, args.first())
        && w.scan()
            .get(*id)
            .is_some_and(|p| p.owner_id == character_id)
    {
        w.scan_mut().probes.remove(id);
    }
    PyValue::None
}

/// `ReconnectToLostProbes()` -- `scanSvc.ReconnectToLostProbes:331`, return
/// discarded **[V]**. **[A]** re-announces every live probe this pilot has in
/// the system they are in, which is what a session change emptied
/// (`probeTracker.Refresh`).
pub fn reconnect(w: &mut Registry, actor: Option<i64>, now: i64) -> PyValue {
    let Some(character_id) = actor else {
        return PyValue::None;
    };
    w.scan_mut().prune(now);
    let Some(system) = w.roster().get(character_id).map(|p| p.solar_system_id) else {
        return PyValue::None;
    };
    let mine: Vec<PyValue> = w
        .scan()
        .owned_by(character_id)
        .filter(|p| p.solar_system_id == system)
        .map(Probe::to_keyval)
        .collect();
    for probe in mine {
        w.push(NewProbe {
            to: vec![character_id],
            probe,
        });
    }
    PyValue::None
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// One probe result, before it is a `KeyVal`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeResult {
    /// The result's `id`: a target id string.
    pub target_id: String,
    pub scan_group_id: i64,
    pub group_id: i64,
    pub type_id: i64,
    pub item_id: i64,
    pub certainty: f64,
    /// Where the result is centred. The true position at a perfect result.
    pub pos: [f64; 3],
    /// The uncertainty sphere's radius, metres; unused at a perfect result.
    pub deviation: f64,
    /// A site's scan-strength attribute (SOC-B3), disclosed from
    /// [`RESULT_GOOD`] as `strengthAttributeID` -- what tells a data site
    /// from a relic site in the window **[A]** for the threshold (TQ names
    /// the site's kind at 25 %; falsified by a client that shows it earlier
    /// or later). `None` for a ship.
    pub strength_attribute: Option<i64>,
}

impl ProbeResult {
    /// The entry `results.Result.AddEntry` reads **[V]** (`results.pyc:37-49`):
    /// `.id`, `.data` (by `getattr` with a default), `.scanGroupID`,
    /// `.certainty`, `.pos`, and through `_AddPersistableAttribute` -- a
    /// `getattr(entry, name, None)` that skips a `None` -- `itemID`,
    /// `typeID`, `groupID`, `strengthAttributeID`, `dungeonNameID`,
    /// `dungeonID`, `archetypeID` and `factionID`, and `difficulty`.
    ///
    /// **`data` decides the shape on the map** (`Result._GetPosition:95-106`):
    /// a **tuple** is an exact point, a **float** is a sphere of that radius
    /// around `.pos`. And `util.IsPerfectResult:21` is `certainty >= 1.0 and
    /// isinstance(data, tuple)` -- so a perfect result *must* carry a tuple or
    /// the client never treats it as found (no warp button,
    /// `scannerListEntries.Load:270` reads `result.isPerfect`).
    ///
    /// **[A]** what is disclosed at each certainty: the ladder on
    /// [`RESULT_GOOD`].
    pub fn to_keyval(&self) -> PyValue {
        let perfect = self.certainty >= RESULT_PERFECT;
        let opt = |show: bool, v: i64| if show { PyValue::Int(v) } else { PyValue::None };
        let data = if perfect {
            tuple3(self.pos)
        } else {
            PyValue::Real(self.deviation)
        };
        keyval(vec![
            ("id", PyValue::Bytes(self.target_id.as_bytes().to_vec())),
            ("scanGroupID", PyValue::Int(self.scan_group_id)),
            ("groupID", opt(self.certainty >= RESULT_GOOD, self.group_id)),
            (
                "typeID",
                opt(self.certainty >= RESULT_INFORMATIVE, self.type_id),
            ),
            ("itemID", opt(perfect, self.item_id)),
            (
                "certainty",
                PyValue::Real(self.certainty.min(RESULT_PERFECT)),
            ),
            ("pos", tuple3(self.pos)),
            ("data", data),
            (
                "strengthAttributeID",
                self.strength_attribute
                    .filter(|_| self.certainty >= RESULT_GOOD)
                    .map_or(PyValue::None, PyValue::Int),
            ),
            ("dungeonID", PyValue::None),
            ("dungeonNameID", PyValue::None),
            ("archetypeID", PyValue::None),
            ("factionID", PyValue::None),
            ("difficulty", PyValue::None),
        ])
    }
}

/// A ship's scan target id, `ABC-123` -- the shape of CCP's
/// `scanMgr.GetScanTargetID` (`signalTracker.pyc:265`, a
/// "randomlyGeneratedTargetID", `:329`) **[I]** for the format. A pure
/// function of the ship id **[A]**, so a warp can be resolved without
/// remembering which scan produced it.
pub fn target_id(ship_id: i64) -> String {
    let n = ship_id.unsigned_abs();
    let mut letters = n / 1000;
    let mut s = String::new();
    for _ in 0..3 {
        s.push(char::from(b'A' + (letters % 26) as u8));
        letters /= 26;
    }
    format!("{s}-{:03}", n % 1000)
}

/// Signal strength, **all [A]** -- the server's formula is not in the
/// client, and nothing in the client checks one.
///
/// For each probe that hears the target (centre distance `d` within its
/// range `R`): `strength * (base / R) * (sigRadius / 100) * (1 - d / R)`,
/// summed over probes and divided by [`STRENGTH_REF`]. So a probe is
/// stronger the smaller its range (each range step halves it, as the range
/// factor doubles the range), the bigger the target, and the closer to the
/// probe's centre; and more probes add. Calibrated so that four combat
/// probes at their smallest range, on top of a frigate (35 m), reach a
/// perfect result, and the same four at 32 AU see only that *something* is
/// there.
pub fn certainty_in(
    content: &Content,
    probes: &[Probe],
    target: [f64; 3],
    signature_radius: f64,
) -> f64 {
    certainty_by(
        |t| probe_type_in(content, t),
        probes,
        target,
        signature_radius,
    )
}

/// [`certainty_in`] over the retired [`PROBE_TYPES`], for
/// `crate::signal_tracker` until it passes `Content` (X2 deletes this).
pub fn certainty(probes: &[Probe], target: [f64; 3], signature_radius: f64) -> f64 {
    certainty_by(|t| probe_type(t).copied(), probes, target, signature_radius)
}

fn certainty_by(
    kind_of: impl Fn(i64) -> Option<ProbeType>,
    probes: &[Probe],
    target: [f64; 3],
    signature_radius: f64,
) -> f64 {
    let mut sum = 0.0;
    for p in probes {
        let Some(kind) = kind_of(p.type_id) else {
            continue;
        };
        let r = p.scan_range;
        if r <= 0.0 {
            continue;
        }
        let d = len(sub(target, p.position));
        if d > r {
            continue;
        }
        let base = kind.base_range_au * AU;
        sum += kind.strength * (base / r) * (signature_radius / 100.0) * (1.0 - d / r);
    }
    sum / STRENGTH_REF
}

/// The divisor in [`certainty`] **[A]**.
pub const STRENGTH_REF: f64 = 20.0;

/// How far off an uncertain result is centred, and how big its sphere
/// is **[A]**: the sphere is the smallest hearing range shrunk by the
/// certainty, and the centre is moved off the truth by up to half of it, in
/// a direction fixed by the target and the scan -- so it moves between
/// scans, as the real one does, and the truth is always inside the sphere.
fn smear(truth: [f64; 3], smallest_range: f64, certainty: f64, seed: u64) -> ([f64; 3], f64) {
    let deviation = smallest_range * (1.0 - certainty.clamp(0.0, 1.0));
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut unit = || {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut x = z;
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^= x >> 31;
        (x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    let dir = [unit(), unit(), unit()];
    let n = len(dir).max(1e-9);
    let off = deviation * 0.5 * unit().abs();
    (
        [
            truth[0] + dir[0] / n * off,
            truth[1] + dir[1] / n * off,
            truth[2] + dir[2] / n * off,
        ],
        deviation,
    )
}

/// Every other pilot's ship these probes can hear.
///
/// Only probes that can scan ships count ([`ProbeType::can_scan_ships`],
/// the combat probe's `probeCanScanShips` **[V-data]**). Only **players'**
/// ships -- bots are characters, so they count -- and never the scanner's
/// own **[A]**: an NPC would be `probeScanGroupNPCs`, which this does not
/// claim to know the rules of.
pub fn scan_ships(w: &mut Registry, character_id: i64, probes: &[Probe]) -> Vec<ProbeResult> {
    let Some(system) = probes.first().map(|p| p.solar_system_id) else {
        return vec![];
    };
    let hearing: Vec<Probe> = probes
        .iter()
        .filter(|p| p.solar_system_id == system)
        .filter(|p| probe_type_in(w.content(), p.type_id).is_some_and(|k| k.can_scan_ships))
        .cloned()
        .collect();
    if hearing.is_empty() {
        return vec![];
    }
    w.advance_system(system);
    let scan_no = w.scan().last_results.len() as u64;
    let mut out = Vec::new();
    for p in w.roster().in_system(system) {
        if p.character_id == character_id || w.npcs().get(p.character_id).is_some() {
            continue;
        }
        // CLOAK-1: nor does a probe find one.
        if crate::cloak::is_cloaked(w, p.ship_id) {
            continue;
        }
        let Some(c) = w.character(p.character_id) else {
            continue;
        };
        let hull = crate::destiny::hull::of_in(w.content(), c.ship_type_id);
        let truth = p.motion.position;
        let cert = certainty_in(w.content(), &hearing, truth, hull.signature_radius);
        if cert < RESULT_UNUSABLE {
            continue;
        }
        let smallest = hearing
            .iter()
            .filter(|q| len(sub(truth, q.position)) <= q.scan_range)
            .map(|q| q.scan_range)
            .fold(f64::INFINITY, f64::min);
        let (pos, deviation) = if cert >= RESULT_PERFECT {
            (truth, 0.0)
        } else {
            smear(
                truth,
                smallest,
                cert,
                p.ship_id.unsigned_abs() ^ (scan_no << 32),
            )
        };
        out.push(ProbeResult {
            target_id: target_id(p.ship_id),
            scan_group_id: SCAN_GROUP_SHIPS,
            group_id: item_group(w.content(), c.ship_type_id),
            type_id: c.ship_type_id,
            item_id: p.ship_id,
            certainty: cert,
            pos,
            deviation,
            strength_attribute: None,
        });
    }
    out
}

/// Where `CmdWarpToStuff('scan', targetID)` goes: the ship whose
/// [`target_id`] it is, in the caller's system, **where it is now**
/// **[A]**. `None` for anything else, which `warp` answers by doing nothing.
pub fn warp_target(w: &Registry, actor: i64, destination: &PyValue) -> Option<Target> {
    let wanted = as_bytes(destination)?;
    let here = w.roster().get(actor)?;
    let p = w
        .roster()
        .in_system(here.solar_system_id)
        .into_iter()
        .find(|p| p.ship_id != here.ship_id && target_id(p.ship_id).as_bytes() == wanted)?;
    w.character_target(p.character_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};
    use crate::space_state::{Motion, Presence};

    const ALICE: i64 = 1_000_001;
    const BOB: i64 = 1_000_002;
    /// `typedata::FIXTURE`'s hull.
    const FRIGATE: i64 = 990_000_001;
    /// Invented rows (CP58e): a probe launcher, a probe that finds ships
    /// and one that does not. **Not the client's numbers.**
    const LAUNCHER: i64 = 990_581_001;
    const COMBAT_PROBE: i64 = 990_581_010;
    const CORE_PROBE: i64 = 990_581_011;
    const ROWS: &str = "\
type\t990581001\t481\t7\t1\t1\t5.0\t1.0\t0.0\t1.0\t0\t\t\tFixture Probe Launcher
type\t990581010\t479\t8\t1\t1\t1.0\t0.0\t1.0\t1.0\t0\t\t281=600000.0;1370=1.0;1371=10.0;1373=3.0;1413=1.0\tFixture Ship Probe
type\t990581011\t479\t8\t1\t1\t1.0\t0.0\t1.0\t1.0\t0\t\t281=600000.0;1370=0.5;1371=30.0;1373=3.0\tFixture Site Probe
";

    fn content() -> Content {
        let mut c = Content::fixture();
        let text = format!("{}{ROWS}", crate::typedata::FIXTURE);
        let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    fn world() -> Registry {
        let alice = Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: 100_000_001,
            ship_type_id: FRIGATE,
            ..Character::default()
        };
        let bob = Character {
            character_id: BOB,
            name: "Bob".to_owned(),
            ship_id: 100_000_002,
            ship_type_id: FRIGATE,
            ..Character::default()
        };
        Registry::new(SandboxConfig {
            characters: vec![alice, bob],
            ..SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content()))
    }

    fn put(reg: &mut Registry, who: i64, ship: i64, at: [f64; 3]) {
        let system = reg
            .character(who)
            .and_then(|c| c.solar_system_id)
            .expect("a home system");
        reg.roster_mut().arrive(Presence::arriving(
            who,
            ship,
            system,
            Motion::arrived(at, [0.0; 3]),
        ));
    }

    fn attr<'a>(kv: &'a PyValue, name: &str) -> Option<&'a PyValue> {
        match kv {
            PyValue::Instance { state, .. } => dict_get(state, name.as_bytes()),
            _ => None,
        }
    }

    /// **The cone is the whole angle, and 360 degrees is a sphere** -- the
    /// client draws `tan(angle / 2)` either side of the axis
    /// (`directionalScanHandler.ShowCone:277`) and a sphere at 360
    /// (`DrawDirectionalScanCone:248`).
    #[test]
    fn the_cone_is_the_whole_angle_and_a_full_turn_is_a_sphere() {
        let o = [0.0; 3];
        let ahead = [1.0, 0.0, 0.0];
        let quarter = std::f64::consts::FRAC_PI_2;
        // 40 degrees off-axis: inside a 90-degree cone (45 each side).
        let off40 = [40f64.to_radians().cos(), 40f64.to_radians().sin(), 0.0];
        assert!(in_cone(o, ahead, quarter, 10.0, off40));
        // 50 degrees off-axis is outside it.
        let off50 = [50f64.to_radians().cos(), 50f64.to_radians().sin(), 0.0];
        assert!(!in_cone(o, ahead, quarter, 10.0, off50));
        // Directly behind: only a full turn sees it.
        assert!(!in_cone(
            o,
            ahead,
            std::f64::consts::PI,
            10.0,
            [-1.0, 0.0, 0.0]
        ));
        assert!(in_cone(
            o,
            ahead,
            std::f64::consts::TAU,
            10.0,
            [-1.0, 0.0, 0.0]
        ));
        // And the range bounds all of it.
        assert!(!in_cone(
            o,
            ahead,
            std::f64::consts::TAU,
            0.5,
            [1.0, 0.0, 0.0]
        ));
    }

    /// **A range past 14.3 AU is cut to it** (`_MAX_RANGE_AU`).
    #[test]
    fn cone_scan_clamps_to_14_3_au() {
        let mut reg = world();
        put(&mut reg, ALICE, 100_000_001, [0.0; 3]);
        put(
            &mut reg,
            BOB,
            100_000_002,
            [(MAX_DSCAN_AU + 5.0) * AU, 0.0, 0.0],
        );
        let ask = |reg: &mut Registry, au: f64| {
            let args = [
                PyValue::Real(std::f64::consts::TAU),
                PyValue::Real(au * AU),
                PyValue::Real(1.0),
                PyValue::Real(0.0),
                PyValue::Real(0.0),
            ];
            let PyValue::List(rows) = cone_scan(reg, Some(ALICE), &args).expect("answered") else {
                unreachable!("a list")
            };
            rows.iter()
                .filter(|r| attr(r, "id") == Some(&PyValue::Int(100_000_002)))
                .count()
        };
        // A modified client asks for 100 AU and still does not see 19.3.
        assert_eq!(ask(&mut reg, 100.0), 0);
        put(
            &mut reg,
            BOB,
            100_000_002,
            [(MAX_DSCAN_AU - 1.0) * AU, 0.0, 0.0],
        );
        assert_eq!(ask(&mut reg, 100.0), 1);
        // The honest range still bounds it.
        assert_eq!(ask(&mut reg, 1.0), 0);
    }

    /// **A system with `disallowScanning` answers nothing.**
    #[test]
    fn cone_scan_refuses_a_suppressed_system() {
        let mut reg = world();
        put(&mut reg, ALICE, 100_000_001, [0.0; 3]);
        put(&mut reg, BOB, 100_000_002, [1000.0, 0.0, 0.0]);
        let system = reg
            .character(ALICE)
            .and_then(|c| c.solar_system_id)
            .unwrap();
        let args = [
            PyValue::Real(std::f64::consts::TAU),
            PyValue::Real(AU),
            PyValue::Real(1.0),
            PyValue::Real(0.0),
            PyValue::Real(0.0),
        ];
        let count = |reg: &mut Registry| match cone_scan(reg, Some(ALICE), &args) {
            Ok(PyValue::List(rows)) => rows
                .iter()
                .filter(|r| attr(r, "id") == Some(&PyValue::Int(100_000_002)))
                .count(),
            other => panic!("not a list: {other:?}"),
        };
        assert_eq!(count(&mut reg), 1);
        assert!(!scanning_suppressed(reg.content(), system));
        let row =
            format!("{system}\t19000001\t29000001\t\t\t0.5\t\t0\t0\t0\t0\t0\t0\t\t1\t\t\tFixture");
        let universe = crate::celestial::Universe::parse(&[row], &[], &[]).unwrap();
        let mut c = content();
        c.insert("universe", universe);
        let mut reg = reg.with_content(std::sync::Arc::new(c));
        assert!(scanning_suppressed(reg.content(), system));
        assert_eq!(count(&mut reg), 0);
    }

    /// **`ConeScan` answers rows with `.id`, `.typeID` and `.groupID`** --
    /// the three attributes `directionalScannerPalette` and
    /// `scanHologramHandler` read -- for the other ship in the cone, and not
    /// for the scanner's own.
    #[test]
    fn a_cone_scan_answers_the_other_ship_with_id_type_and_group() {
        let mut reg = world();
        put(&mut reg, ALICE, 100_000_001, [0.0; 3]);
        put(&mut reg, BOB, 100_000_002, [10_000.0, 0.0, 0.0]);
        let args = [
            PyValue::Real(std::f64::consts::FRAC_PI_2),
            PyValue::Int(1_000_000),
            PyValue::Real(1.0),
            PyValue::Real(0.0),
            PyValue::Real(0.0),
        ];
        let PyValue::List(rows) = cone_scan(&mut reg, Some(ALICE), &args).expect("answered") else {
            panic!("a list: the palette slices it");
        };
        let bob: Vec<_> = rows
            .iter()
            .filter(|r| attr(r, "id") == Some(&PyValue::Int(100_000_002)))
            .collect();
        assert_eq!(bob.len(), 1, "{rows:?}");
        assert_eq!(attr(bob[0], "typeID"), Some(&PyValue::Int(FRIGATE)));
        let row = crate::typedata::installed_in(reg.content())
            .table()
            .get(FRIGATE)
            .expect("the fixture hull");
        assert_eq!(attr(bob[0], "groupID"), Some(&PyValue::Int(row.group_id)));
        assert!(
            !rows
                .iter()
                .any(|r| attr(r, "id") == Some(&PyValue::Int(100_000_001))),
            "not the scanner's own ship"
        );

        // Pointed the other way, Bob is not there.
        let behind = [
            PyValue::Real(std::f64::consts::FRAC_PI_2),
            PyValue::Int(1_000_000),
            PyValue::Real(-1.0),
            PyValue::Real(0.0),
            PyValue::Real(0.0),
        ];
        let PyValue::List(rows) = cone_scan(&mut reg, Some(ALICE), &behind).expect("answered")
        else {
            panic!("a list");
        };
        assert!(
            !rows
                .iter()
                .any(|r| attr(r, "id") == Some(&PyValue::Int(100_000_002)))
        );
    }

    /// Five positionals or a refusal: `ConeScan(scanAngle, scanRange, *dir)`.
    #[test]
    fn a_cone_scan_needs_five_numbers() {
        let mut reg = world();
        assert!(cone_scan(&mut reg, Some(ALICE), &[PyValue::Real(1.0)]).is_err());
    }

    /// **A perfect result carries a tuple, anything less a float** --
    /// `util.IsPerfectResult` is `certainty >= 1.0 and isinstance(data,
    /// tuple)`, and `Result._GetPosition` reads a float as a sphere around
    /// `.pos`.
    #[test]
    fn a_perfect_result_is_a_point_and_a_partial_one_a_sphere() {
        let base = ProbeResult {
            target_id: target_id(100_000_002),
            scan_group_id: SCAN_GROUP_SHIPS,
            group_id: 25,
            type_id: FRIGATE,
            item_id: 100_000_002,
            certainty: RESULT_PERFECT,
            pos: [1.0, 2.0, 3.0],
            deviation: 0.0,
            strength_attribute: None,
        };
        let perfect = base.to_keyval();
        assert_eq!(
            attr(&perfect, "data"),
            Some(&PyValue::Tuple(vec![
                PyValue::Real(1.0),
                PyValue::Real(2.0),
                PyValue::Real(3.0)
            ]))
        );
        assert_eq!(attr(&perfect, "certainty"), Some(&PyValue::Real(1.0)));
        assert_eq!(attr(&perfect, "scanGroupID"), Some(&PyValue::Int(4)));
        assert!(matches!(attr(&perfect, "id"), Some(PyValue::Bytes(_))));

        let partial = ProbeResult {
            certainty: 0.5,
            deviation: 1234.0,
            ..base
        }
        .to_keyval();
        assert_eq!(attr(&partial, "data"), Some(&PyValue::Real(1234.0)));
    }

    /// The four pushes, each with the arity of the handler it lands in
    /// (`scanSvc.pyc` signatures).
    #[test]
    fn the_scan_pushes_have_their_handlers_arity() {
        let p = Probe {
            probe_id: PROBE_ID_BASE + 1,
            type_id: COMBAT_PROBE,
            owner_id: ALICE,
            solar_system_id: 30_000_001,
            position: [0.0; 3],
            scan_range: AU,
            range_step: 2,
            expiry: 1,
        };
        let (n, a) = NewProbe {
            to: vec![ALICE],
            probe: p.to_keyval(),
        }
        .to_push();
        assert_eq!((n, a.len()), ("OnNewProbe", 1));
        let (n, a) = RemoveProbe {
            to: vec![ALICE],
            probe_id: p.probe_id,
        }
        .to_push();
        assert_eq!((n, a.len()), ("OnRemoveProbe", 1));
        let (n, a) = ScanStarted {
            to: vec![ALICE],
            start_time: 0,
            duration_ms: SCAN_DURATION_MS,
            probes: vec![(p.probe_id, p.to_keyval())],
        }
        .to_push();
        assert_eq!((n, a.len()), ("OnSystemScanStarted", 3));
        assert!(
            matches!(a[2], PyValue::Dict(_)),
            "iteritems() is called on it"
        );
        let (n, a) = ScanStopped {
            to: vec![ALICE],
            probe_ids: vec![p.probe_id],
            results: vec![],
            absent: vec![],
        }
        .to_push();
        assert_eq!((n, a.len()), ("OnSystemScanStopped", 3));
        // Every attribute the client reads off a probe is on it.
        for name in [
            "probeID",
            "typeID",
            "pos",
            "destination",
            "scanRange",
            "rangeStep",
            "state",
            "expiry",
        ] {
            assert!(attr(&p.to_keyval(), name).is_some(), "{name}");
        }
    }

    /// A target id round-trips through `warp_target` to the ship it names.
    #[test]
    fn a_scan_result_id_warps_to_its_ship() {
        let mut reg = world();
        put(&mut reg, ALICE, 100_000_001, [0.0; 3]);
        put(&mut reg, BOB, 100_000_002, [5.0e9, 0.0, 0.0]);
        let id = PyValue::Bytes(target_id(100_000_002).into_bytes());
        let t = warp_target(&reg, ALICE, &id).expect("Bob's result resolves");
        assert_eq!(t.position, [5.0e9, 0.0, 0.0]);
        let mine = PyValue::Bytes(target_id(100_000_001).into_bytes());
        assert!(warp_target(&reg, ALICE, &mine).is_none(), "not to yourself");
    }

    /// **Launch takes what is loaded, and recovery brings it home.** Three
    /// ship probes in a launcher, eight asked for: three go out, each
    /// announced with `OnNewProbe`, and the launcher's count drops to zero.
    /// Recovering one answers its id, tells the client `OnRemoveProbe` and
    /// puts it back on the cargo hold's stack.
    #[test]
    fn launching_spends_the_loaded_probes_and_recovery_returns_them() {
        const HIGH0: i64 = 27;
        const LAUNCHER_ITEM: i64 = 100_000_501;
        const LOADED: i64 = 100_000_502;
        const SPARE: i64 = 100_000_503;
        let mut reg = world();
        let ship = 100_000_001;
        put(&mut reg, ALICE, ship, [0.0; 3]);
        let row = |item_id: i64, type_id: i64, flag_id: i64, quantity: i64| {
            let t = crate::typedata::item_in(reg.content(), type_id).expect("a test row");
            crate::item::Item {
                item_id,
                type_id,
                owner_id: ALICE,
                location_id: ship,
                flag_id,
                quantity,
                group_id: t.group_id,
                category_id: t.category_id,
                singleton: t.singleton,
            }
        };
        let mut items = reg.store().items().to_vec();
        items.push(row(LAUNCHER_ITEM, LAUNCHER, HIGH0, 1));
        items.push(row(LOADED, COMBAT_PROBE, HIGH0, 3));
        items.push(row(SPARE, COMBAT_PROBE, crate::fitting::FLAG_CARGO, 1));
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_000_600,
            ..crate::store::Snapshot::default()
        });
        let _ = reg.take_pending();

        launch_probes(
            &mut reg,
            Some(ALICE),
            &[PyValue::Int(LAUNCHER_ITEM), PyValue::Int(8)],
            0,
        )
        .expect("answered");
        let names: Vec<_> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push().0)
            .collect();
        assert_eq!(
            names.iter().filter(|n| **n == "OnNewProbe").count(),
            3,
            "{names:?}"
        );
        assert_eq!(reg.scan().owned_by(ALICE).count(), 3);
        assert!(
            crate::modules::loaded(reg.store(), ship, HIGH0).is_none(),
            "all three rounds left the launcher"
        );

        let probe = reg.scan().owned_by(ALICE).next().expect("out").probe_id;
        let back = recover_probes(
            &mut reg,
            Some(ALICE),
            &[PyValue::List(vec![PyValue::Int(probe)])],
            0,
        )
        .expect("answered");
        assert_eq!(back, PyValue::List(vec![PyValue::Int(probe)]));
        assert_eq!(reg.scan().owned_by(ALICE).count(), 2);
        assert!(
            reg.take_pending()
                .into_iter()
                .any(|(_, p)| p.to_push() == ("OnRemoveProbe", vec![PyValue::Int(probe)]))
        );
        assert_eq!(reg.store().get(SPARE).map(|i| i.quantity), Some(2));
    }

    /// **A probe type is its row in the loaded type data** (CP58e, ruling
    /// 3): every number is the row's own attribute, `probeCanScanShips`
    /// only where the row carries it, and a launcher is no probe.
    #[test]
    fn a_probe_type_is_read_from_the_loaded_type_data() {
        let c = content();
        let t = crate::typedata::installed_in(&c).table();
        let k = probe_type_in(&c, COMBAT_PROBE).expect("a probe");
        let a = |id| t.attribute(COMBAT_PROBE, id);
        assert_eq!(k.strength, a(attr::BASE_SENSOR_STRENGTH));
        assert_eq!(k.base_range_au, a(attr::BASE_SCAN_RANGE));
        assert_eq!(k.range_factor, a(attr::RANGE_FACTOR));
        assert_eq!(k.lifetime_ms, a(attr::EXPLOSION_DELAY) as i64);
        assert!(k.can_scan_ships);
        assert!(
            !probe_type_in(&c, CORE_PROBE)
                .expect("a probe")
                .can_scan_ships
        );
        assert!(probe_type_in(&c, LAUNCHER).is_none());
        assert!(is_probe_launcher_in(&c, LAUNCHER));
        assert!(!is_probe_launcher_in(&c, COMBAT_PROBE));
        // Without a bundle there is no probe at all.
        assert!(probe_type_in(&Content::fixture(), COMBAT_PROBE).is_none());
    }

    /// **The d-scan files an NPC under its own type's group**
    /// (`npc::NpcType::group_id`), and still sees it off the scanner's grid.
    #[test]
    fn a_cone_scan_files_an_npc_under_its_own_group() {
        let mut reg = world();
        put(&mut reg, ALICE, 100_000_001, [0.0; 3]);
        let system = reg
            .character(ALICE)
            .and_then(|c| c.solar_system_id)
            .expect("a home system");
        let far = 10.0 * crate::grid::GRID_ENTER;
        let rat = crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, system, [far, 0.0, 0.0])
            .expect("an NPC");
        let ship = reg.roster().get(rat).expect("in space").ship_id;
        let args = [
            PyValue::Real(std::f64::consts::FRAC_PI_2),
            PyValue::Real(far * 2.0),
            PyValue::Real(1.0),
            PyValue::Real(0.0),
            PyValue::Real(0.0),
        ];
        let PyValue::List(rows) = cone_scan(&mut reg, Some(ALICE), &args).expect("answered") else {
            panic!("a list");
        };
        let row = rows
            .iter()
            .find(|r| attr(r, "id") == Some(&PyValue::Int(ship)))
            .expect("the NPC, off grid");
        assert_eq!(
            attr(row, "groupID"),
            Some(&PyValue::Int(crate::npc::FIXTURE_RAT.group_id))
        );
    }

    /// `RequestScans` with no probes still ends: a start now and a stop on
    /// the clock, both to the scanner.
    #[test]
    fn a_scan_request_starts_now_and_stops_on_the_clock() {
        let mut reg = world();
        put(&mut reg, ALICE, 100_000_001, [0.0; 3]);
        let _ = reg.take_pending();
        let before = reg.scheduled_count();
        request_scans(&mut reg, Some(ALICE), &[PyValue::None], 0).expect("answered");
        let names: Vec<_> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push().0)
            .collect();
        assert_eq!(names, vec!["OnSystemScanStarted"]);
        assert_eq!(reg.scheduled_count(), before + 1, "the stop waits");
    }
}
