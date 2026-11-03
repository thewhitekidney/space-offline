//! **Probe scanning, and the explorer** (R-SOCIETY SOC-B3): a bot fits a
//! probe launcher, launches core probes, scans a cosmic signature down to a
//! perfect result and warps to it -- every step a player verb
//! (`verbs.rs`, "probe scanning"), against the server's scanner
//! (`eve_server::scan`, `eve_server::signal_tracker`, `docs/lanes/scan.md`).
//!
//! **The API other careers use** is [`find`]: in space, with a launcher
//! fitted, `find(conn, Kind::Data)` scans the system and answers the first
//! signature of that kind it resolved, or `None`. [`warp_to`] then lands on
//! it. The hacker (SOC-B4), the gas huffer (SOC-B2) and the wormhole diver
//! call these two; [`find_with`] takes any rule.
//!
//! How a signature is found, the way a player does it:
//!
//! 1. **The overlay** ([`Conn::sensor_overlay`]): `GetFullState` lists the
//!    system's signatures with a blurred position and a deviation (4-8 AU on
//!    TQ, `signal_tracker::deviation` [V]). The nearest first.
//! 2. **The probes** ([`Scanner::open`]): the ones still out
//!    (`ReconnectToLostProbes`), else a launch of up to [`MAX_PROBES`]; an
//!    empty launcher is loaded from the
//!    hold first.
//! 3. **Narrowing** ([`Scanner::pinpoint`]): all probes on the estimate at
//!    the smallest range step that covers its deviation ([`first_step`]);
//!    each result moves the probes onto its new estimate one step smaller,
//!    a lost signal one step larger ([`next_step`]), until the result is
//!    perfect (`certainty >= 1` and a point, `util.IsPerfectResult` [V]) or
//!    [`MAX_SCANS`] run out.
//! 4. **The kind** ([`Kind`]) is the result's `strengthAttributeID`, once
//!    the server discloses it (the coordinator's server hunk, at
//!    `RESULT_GOOD` [A]); a signature of a kind the caller does not want is
//!    dropped as soon as its kind is known.
//! 5. **Warp** ([`warp_to`]): `CmdWarpToStuff('scan', targetID)`, which the
//!    server resolves only after the caller's own perfect result.
//!
//! **The explorer** ([`ROLE`], career `explorer`): docked, it fits a
//! launcher and loads probes ([`outfit`]); in space, each [`Act::Explore`]
//! scans down one signature it has not reported, warps to it, saves its
//! location in the personal folder and books it on the [`LEDGER`]. A system
//! with nothing new is left: the probes come home, and with a map
//! ([`crate::travel::Trips`]) the explorer jumps to a neighbour within
//! [`ROAM_JUMPS`] of its home that nobody scanned out in the last
//! [`REVISIT`]; with none left, or no map, it docks at home. Hacking a site
//! is SOC-B4's.
//!
//! Every rule here is **[A]** policy except the call shapes, which the verbs
//! cite.

use crate::brain::{self, Act, ActKind, Homing, Memory, Observation, SHORT_WAIT};
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::travel::{Map, Trips};
use crate::{Conn, Error, ItemRow, MIN_WARP_DISTANCE, ObjectRef, Result};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ---- constants -------------------------------------------------------------------

/// `groupScanProbeLauncher` and `groupScannerProbe` **[V]**
/// (`inventorycommon/const`; the server's `scan::GROUP_*`): how the bot
/// tells a launcher and a probe in a listing.
pub const GROUP_LAUNCHER: i64 = 481;
pub const GROUP_PROBE: i64 = 479;

/// The most probes out at once, the server's `scan::MAX_PROBES` **[A]**
/// (its own; TQ's eight): what one launch asks for.
pub const MAX_PROBES: i64 = 8;

/// `probeStateIdle` **[V]** (`scan::PROBE_STATE_IDLE`).
pub const PROBE_STATE_IDLE: i64 = 1;

/// `util.IsPerfectResult:21`'s threshold **[V]**.
pub const RESULT_PERFECT: f64 = 1.0;

/// **Core Probe Launcher I** and **Core Scanner Probe I** **[V-data]**
/// (`eve_server::item::SPAWNABLE`, `docs/lanes/scan.md` §2): what the
/// explorer's setup gives and its kit buys. The scanning itself goes by
/// group, so any launcher and probe will do.
pub const CORE_LAUNCHER: i64 = 17_938;
pub const CORE_PROBE: i64 = 30_013;

/// The explorer's hull **[A]**: a frigate the servers' tables know, with a
/// high slot to spare. Falsified by a launcher that does not fit it.
pub const HULL: i64 = brain::MERLIN;

/// Probes the setup gives and the kit buys: a launch and a spare load
/// **[A]**.
pub const SETUP_PROBES: i64 = 16;

/// Probes kept in the hold for a reload in space **[A]**: one launch.
pub const SPARE_PROBES: i64 = MAX_PROBES;

/// `rangeFactor` of both scanner probes **[V-data]** (`docs/lanes/scan.md`
/// §2): each range step doubles the last. The client reads it from the
/// type; the bot has no type data, so it is named here.
pub const RANGE_FACTOR: f64 = 2.0;

/// The range steps, `range(8)` in `probeTracker.GetScanRangeStepsByTypeID
/// :248` **[V]**; step 1 is the probe's base range.
pub const RANGE_STEPS: u32 = 8;

/// Scans spent on one signature before it is given up **[A]**: eight steps
/// down and a few lost signals. Falsified by an explorer that gives up on
/// signatures a player resolves.
pub const MAX_SCANS: u32 = 12;

/// How far short of a signature to land, metres **[A]**: on it.
pub const WARP_RANGE: f64 = 0.0;

/// Within this of a signature's point the explorer has arrived **[A]**:
/// closer than the client's own minimum warp.
pub const ARRIVED: f64 = MIN_WARP_DISTANCE;

/// How far from its home the explorer roams, in jumps **[A]**: "nearby
/// systems". Falsified by explorers that never leave home (too low), or
/// that are out of their session window for most of it (too high).
pub const ROAM_JUMPS: u32 = 2;

/// How long a system an explorer scanned out stays off every explorer's
/// list **[A]**: sites respawn on the population's clock (minutes to
/// hours). Falsified by explorers queueing at one system, or by systems
/// whose new signatures nobody reports for hours.
pub const REVISIT: Duration = Duration::from_secs(3_600);

/// The folder the client makes when it has none (`bookmarkSvc.py:407`);
/// the label is localized there, so this text is **[A]**.
pub const FOLDER_NAME: &str = "Personal Locations";

/// How long a warp or an approach may take: the travel lane's ceiling.
pub const TRAVEL_CEILING: Duration = crate::travel::TRAVEL_CEILING;

/// How long a reload in space may keep the launcher from launching
/// **[A]**: the launchers' `reloadTime` is 10 s [V-data]; twice that.
pub const RELOAD_CEILING: Duration = Duration::from_secs(20);

const POLL: Duration = Duration::from_millis(250);

// ---- what a signature is --------------------------------------------------------

/// The scan-strength attributes **[V]** (`dogma/const`; the client's
/// archetype table `probescanning/explorationSites.pyc:16-29`, which the
/// server holds as `population::SCAN_ATTRIBUTES`, and
/// `wormhole::SCAN_ATTRIBUTE`).
pub const SCAN_RADAR: i64 = 208;
pub const SCAN_LADAR: i64 = 209;
pub const SCAN_MAGNETOMETRIC: i64 = 210;
pub const SCAN_GRAVIMETRIC: i64 = 211;
pub const SCAN_ALL: i64 = 1_136;
pub const SCAN_WORMHOLE: i64 = 1_908;

/// What a signature is, by its scan-strength attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// Radar: data sites (hacking, datacores).
    Data,
    /// Magnetometric: relic sites.
    Relic,
    /// Ladar: gas sites.
    Gas,
    Wormhole,
    /// All strengths: combat signatures.
    Combat,
    /// Gravimetric: ore signatures.
    Ore,
    /// Not disclosed yet, or an attribute this table lacks.
    Unknown,
}

impl Kind {
    pub fn from_strength_attribute(attribute: Option<i64>) -> Kind {
        match attribute {
            Some(SCAN_RADAR) => Kind::Data,
            Some(SCAN_MAGNETOMETRIC) => Kind::Relic,
            Some(SCAN_LADAR) => Kind::Gas,
            Some(SCAN_ALL) => Kind::Combat,
            Some(SCAN_GRAVIMETRIC) => Kind::Ore,
            Some(a) if a == SCAN_WORMHOLE => Kind::Wormhole,
            _ => Kind::Unknown,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Data => "data",
            Kind::Relic => "relic",
            Kind::Gas => "gas",
            Kind::Wormhole => "wormhole",
            Kind::Combat => "combat",
            Kind::Ore => "ore",
            Kind::Unknown => "unknown",
        }
    }
}

/// A signature scanned down to a perfect result.
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    pub solar_system_id: i64,
    /// `ABC-123`: the id `CmdWarpToStuff('scan', ...)` takes.
    pub target_id: String,
    pub kind: Kind,
    /// The site's point (a perfect result's `data`).
    pub position: [f64; 3],
}

// ---- reading the wire --------------------------------------------------------------

/// An instance's attribute dict: an old-style `KeyVal`'s state, or a
/// new-style object's (`pyutil::new_style`: header `((class,), state)`).
fn state(v: &PyValue) -> Option<&PyValue> {
    match v {
        PyValue::Instance { state, .. } => Some(state.as_ref()),
        PyValue::ObjectEx { header, .. } => match header.as_ref() {
            PyValue::Tuple(h) => h.get(1),
            _ => None,
        },
        PyValue::Dict(_) => Some(v),
        _ => None,
    }
}

pub(crate) fn get<'a>(v: &'a PyValue, key: &str) -> Option<&'a PyValue> {
    eve_server::pyutil::dict_get(state(v)?, key.as_bytes())
}

fn num(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Real(x) => Some(*x),
        PyValue::Int(i) => Some(*i as f64),
        _ => None,
    }
}

pub(crate) fn int(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(i) => Some(*i),
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
    PyValue::Tuple(p.iter().map(|&x| PyValue::Real(x)).collect())
}

/// One probe, as `OnNewProbe` gave it and `RequestScans` sends it back
/// (`eve_server::scan::Probe::to_keyval` [V]).
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeState {
    pub probe_id: i64,
    pub type_id: i64,
    /// Where it is, and where the next scan puts it.
    pub pos: [f64; 3],
    pub scan_range: f64,
    pub range_step: u32,
    pub state: i64,
    pub expiry: i64,
}

impl ProbeState {
    pub fn from_value(v: &PyValue) -> Option<ProbeState> {
        Some(ProbeState {
            probe_id: int(get(v, "probeID"))?,
            type_id: int(get(v, "typeID")).unwrap_or(0),
            pos: vec3(get(v, "destination")).or_else(|| vec3(get(v, "pos")))?,
            scan_range: num(get(v, "scanRange"))?,
            range_step: int(get(v, "rangeStep"))
                .and_then(|s| u32::try_from(s).ok())
                .filter(|s| (1..=RANGE_STEPS).contains(s))?,
            state: int(get(v, "state")).unwrap_or(PROBE_STATE_IDLE),
            expiry: int(get(v, "expiry")).unwrap_or(0),
        })
    }

    /// The `KeyVal` the client sends back, `destination` set
    /// (`probeTracker.GetProbesForScanning:306` [V]).
    pub fn to_value(&self) -> PyValue {
        eve_server::pyutil::keyval(vec![
            ("probeID", PyValue::Int(self.probe_id)),
            ("typeID", PyValue::Int(self.type_id)),
            ("pos", tuple3(self.pos)),
            ("destination", tuple3(self.pos)),
            ("scanRange", PyValue::Real(self.scan_range)),
            ("rangeStep", PyValue::Int(i64::from(self.range_step))),
            ("state", PyValue::Int(self.state)),
            ("expiry", PyValue::Int(self.expiry)),
        ])
    }

    /// Its step-1 range, metres: `scanRange / RANGE_FACTOR^(step - 1)`.
    pub fn base_range(&self) -> f64 {
        self.scan_range / RANGE_FACTOR.powi(self.range_step as i32 - 1)
    }
}

/// One probe result (`results.Result.AddEntry:37-49` [V]).
#[derive(Debug, Clone, PartialEq)]
pub struct ScanResult {
    pub target_id: String,
    pub scan_group_id: i64,
    pub certainty: f64,
    /// The estimate's centre.
    pub pos: [f64; 3],
    /// `data` as a point: only a perfect result has one.
    pub point: Option<[f64; 3]>,
    /// `data` as a sphere's radius, metres.
    pub deviation: Option<f64>,
    pub strength_attribute: Option<i64>,
}

impl ScanResult {
    pub fn from_value(v: &PyValue) -> Option<ScanResult> {
        let target_id = get(v, "id").and_then(crate::text)?;
        let data = get(v, "data");
        Some(ScanResult {
            target_id,
            scan_group_id: int(get(v, "scanGroupID")).unwrap_or(0),
            certainty: num(get(v, "certainty")).unwrap_or(0.0),
            pos: vec3(get(v, "pos"))?,
            point: vec3(data),
            deviation: num(data),
            strength_attribute: int(get(v, "strengthAttributeID")),
        })
    }

    /// `util.IsPerfectResult:21`: `certainty >= 1.0 and isinstance(data,
    /// tuple)` **[V]**.
    pub fn is_perfect(&self) -> bool {
        self.certainty >= RESULT_PERFECT && self.point.is_some()
    }

    /// The kind, once disclosed.
    pub fn kind(&self) -> Option<Kind> {
        self.strength_attribute
            .map(|a| Kind::from_strength_attribute(Some(a)))
    }
}

/// One signature on the sensor overlay (`SignatureInfo`, `signal_tracker::
/// signature_info` [V]).
#[derive(Debug, Clone, PartialEq)]
pub struct OverlaySite {
    pub site_id: i64,
    pub target_id: String,
    /// Blurred.
    pub position: [f64; 3],
    pub deviation: f64,
}

impl OverlaySite {
    pub fn from_value(site_id: i64, v: &PyValue) -> Option<OverlaySite> {
        Some(OverlaySite {
            site_id,
            target_id: get(v, "targetID").and_then(crate::text)?,
            position: vec3(get(v, "position"))?,
            deviation: num(get(v, "deviation")).unwrap_or(0.0),
        })
    }
}

// ---- narrowing, pure -------------------------------------------------------------

/// The range of `step` for probes whose step 1 is `base` metres.
pub fn range_of(base: f64, step: u32) -> f64 {
    base * RANGE_FACTOR.powi(step.clamp(1, RANGE_STEPS) as i32 - 1)
}

/// The first step for an estimate this far off: the smallest whose range
/// covers `deviation`, else the largest.
pub fn first_step(base: f64, deviation: f64) -> u32 {
    (1..=RANGE_STEPS)
        .find(|&s| range_of(base, s) >= deviation)
        .unwrap_or(RANGE_STEPS)
}

/// The next step: one smaller after the target was heard (never below 1),
/// one larger after it was lost; `None` when nothing larger is left.
pub fn next_step(step: u32, heard: bool) -> Option<u32> {
    if heard {
        Some(step.saturating_sub(1).max(1))
    } else {
        (step < RANGE_STEPS).then_some(step + 1)
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

// ---- the scanner -----------------------------------------------------------------

/// The scan manager and the probes out.
#[derive(Debug, Clone)]
pub struct Scanner {
    pub manager: ObjectRef,
    pub probes: Vec<ProbeState>,
    /// Step 1's range, metres.
    pub base: f64,
    /// Scans made, for the act line.
    pub scans: u32,
}

fn is_launcher(r: &ItemRow) -> bool {
    r.group_id == GROUP_LAUNCHER
}

fn is_probe(r: &ItemRow) -> bool {
    r.group_id == GROUP_PROBE
}

fn in_space(c: &Conn, what: &str) -> Result<i64> {
    if c.session().station_id().is_some() {
        return Err(Error::NotSent(format!("{what}: docked")));
    }
    c.session()
        .solar_system_id()
        .ok_or_else(|| Error::NotSent(format!("{what}: in space with no system")))
}

impl Scanner {
    /// The probes still out, else a launch (step 2 of the module header).
    pub fn open(c: &mut Conn) -> Result<Scanner> {
        in_space(c, "probes")?;
        let manager = c.system_scan_manager()?;
        let mut probes = c.reconnect_probes(&manager)?;
        if probes.is_empty() {
            probes = launch(c)?;
        }
        let base = probes
            .first()
            .map(ProbeState::base_range)
            .ok_or_else(|| Error::Refused("probes: none out after a launch".to_owned()))?;
        Ok(Scanner {
            manager,
            probes,
            base,
            scans: 0,
        })
    }

    /// Every probe at `centre` at `step`, and one scan.
    pub fn scan(&mut self, c: &mut Conn, centre: [f64; 3], step: u32) -> Result<Vec<ScanResult>> {
        let range = range_of(self.base, step);
        for p in &mut self.probes {
            p.pos = centre;
            p.scan_range = range;
            p.range_step = step.clamp(1, RANGE_STEPS);
        }
        self.scans += 1;
        c.request_scans(&self.manager, &self.probes)
    }

    /// Narrow `target_id` from `centre`, `deviation` off, to a perfect
    /// result (step 3). `None` when it is lost, or when `drop` says so of a
    /// partial result (its kind is known and unwanted).
    pub fn pinpoint(
        &mut self,
        c: &mut Conn,
        target_id: &str,
        mut centre: [f64; 3],
        deviation: f64,
        drop: impl Fn(&ScanResult) -> bool,
    ) -> Result<Option<ScanResult>> {
        let mut step = first_step(self.base, deviation);
        for _ in 0..MAX_SCANS {
            let results = self.scan(c, centre, step)?;
            let hit = results.into_iter().find(|r| r.target_id == target_id);
            if let Some(r) = &hit {
                if r.is_perfect() {
                    return Ok(hit);
                }
                if drop(r) {
                    return Ok(None);
                }
                centre = r.pos;
            }
            match next_step(step, hit.is_some()) {
                Some(s) => step = s,
                None => return Ok(None),
            }
        }
        Ok(None)
    }

    /// Bring every probe home (`RecoverProbes`). Returns how many came.
    pub fn recover(&mut self, c: &mut Conn) -> Result<usize> {
        let ids: Vec<i64> = self.probes.iter().map(|p| p.probe_id).collect();
        if ids.is_empty() {
            return Ok(0);
        }
        let back = c.recover_probes(&self.manager, &ids)?;
        self.probes.retain(|p| !back.contains(&p.probe_id));
        Ok(back.len())
    }
}

/// Launch from the fitted launcher; an empty one is loaded from the hold
/// first, and a reload in space is waited out ([`RELOAD_CEILING`]).
fn launch(c: &mut Conn) -> Result<Vec<ProbeState>> {
    let launcher = c
        .fitted()?
        .into_iter()
        .find(is_launcher)
        .ok_or_else(|| Error::NotSent("probes: no probe launcher fitted".to_owned()))?;
    let wanted = MAX_PROBES;
    match c.launch_probes(launcher.item_id, wanted) {
        Ok(p) => return Ok(p),
        Err(Error::Refused(_)) => {}
        Err(e) => return Err(e),
    }
    let ship = c
        .session()
        .ship_id()
        .ok_or_else(|| Error::NotSent("probes: no ship".to_owned()))?;
    let stack = c
        .cargo()?
        .into_iter()
        .find(is_probe)
        .ok_or_else(|| Error::NotSent("probes: none loaded and none in the hold".to_owned()))?;
    c.load_ammo(
        &[(launcher.item_id, launcher.flag_id)],
        &[stack.item_id],
        stack.type_id,
        ship,
    )?;
    let deadline = Instant::now() + RELOAD_CEILING;
    loop {
        match c.launch_probes(launcher.item_id, wanted) {
            Ok(p) => return Ok(p),
            Err(Error::Refused(why)) if Instant::now() >= deadline => {
                return Err(Error::Refused(format!("probes: after a reload, {why}")));
            }
            Err(Error::Refused(_)) => {
                c.pump(Duration::from_secs(1))?;
            }
            Err(e) => return Err(e),
        }
    }
}

// ---- the API ---------------------------------------------------------------------

/// **Find a signature of `kind`** in this system: scan the overlay's
/// signatures, nearest first, to the first perfect result of that kind.
/// In space, with a launcher fitted. The probes stay out.
pub fn find(c: &mut Conn, kind: Kind) -> Result<Option<Signature>> {
    let mut scanner = Scanner::open(c)?;
    find_with(c, &mut scanner, |_, k| k.is_none_or(|k| k == kind))
}

/// [`find`] with any rule: `accept(targetID, kind)` is asked first with
/// `None` (skip a signature without scanning it), then with its kind once
/// disclosed, and last with the perfect result's.
pub fn find_with(
    c: &mut Conn,
    scanner: &mut Scanner,
    accept: impl Fn(&str, Option<Kind>) -> bool,
) -> Result<Option<Signature>> {
    find_seen(c, scanner, accept, |_| {})
}

/// A signature the scan met and did not stop on: its kind was known and
/// `accept` refused it. The roaming careers book these on the ledger
/// ([`Ledger::note`]) for whoever works them (Y-ROAM).
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub target_id: String,
    pub kind: Kind,
    /// The perfect result's point when `exact`; else the last estimate.
    pub position: [f64; 3],
    pub exact: bool,
}

/// [`find_with`], and `seen` is told of every signature the scan met whose
/// kind came out and which `accept` refused, in the order met.
pub fn find_seen(
    c: &mut Conn,
    scanner: &mut Scanner,
    accept: impl Fn(&str, Option<Kind>) -> bool,
    seen: impl FnMut(Seen),
) -> Result<Option<Signature>> {
    let system = in_space(c, "find")?;
    let here = c
        .world_now()
        .ego_ball()
        .map(|b| b.motion.position)
        .unwrap_or([0.0; 3]);
    let mut overlay = c.sensor_overlay(&scanner.manager)?;
    overlay.retain(|s| accept(&s.target_id, None));
    overlay.sort_by(|a, b| {
        distance(here, a.position)
            .total_cmp(&distance(here, b.position))
            .then(a.site_id.cmp(&b.site_id))
    });
    let seen = std::cell::RefCell::new(seen);
    for site in overlay {
        let t = site.target_id.as_str();
        let got = scanner.pinpoint(c, t, site.position, site.deviation, |r| match r.kind() {
            Some(k) if !accept(t, Some(k)) => {
                (seen.borrow_mut())(Seen {
                    target_id: t.to_owned(),
                    kind: k,
                    position: r.point.unwrap_or(r.pos),
                    exact: false,
                });
                true
            }
            _ => false,
        })?;
        let Some(r) = got else { continue };
        let kind = r.kind().unwrap_or(Kind::Unknown);
        if accept(t, Some(kind)) {
            return Ok(Some(Signature {
                solar_system_id: system,
                target_id: r.target_id,
                kind,
                position: r.point.unwrap_or(r.pos),
            }));
        }
        (seen.borrow_mut())(Seen {
            target_id: r.target_id,
            kind,
            position: r.point.unwrap_or(r.pos),
            exact: true,
        });
    }
    Ok(None)
}

fn warping(c: &mut Conn) -> bool {
    c.world_now().ego_ball().is_some_and(|b| b.warp_commanded())
}

fn gap(c: &mut Conn, ball: i64) -> Option<f64> {
    let w = c.world_now();
    w.surface_distance(w.ego?, ball)
}

/// Read pushes until `done` holds or [`TRAVEL_CEILING`] passes.
fn settle(c: &mut Conn, what: &str, mut done: impl FnMut(&mut Conn) -> bool) -> Result<()> {
    let deadline = Instant::now() + TRAVEL_CEILING;
    while !done(c) {
        if Instant::now() >= deadline {
            return Err(Error::Refused(format!(
                "{what}: still moving after the ceiling"
            )));
        }
        c.pump(POLL)?;
    }
    Ok(())
}

/// **Warp to a scanned-down signature** and land ([`ARRIVED`]).
///
/// A signature already within [`ARRIVED`] is not warped to: the server
/// refuses a warp inside the minimum distance (WARP-MIN), and by this
/// function's own definition the bot has arrived.
pub fn warp_to(c: &mut Conn, sig: &Signature) -> Result<()> {
    if c.world_now()
        .ego_ball()
        .is_some_and(|b| distance(b.motion.position, sig.position) <= ARRIVED)
    {
        return Ok(());
    }
    match c.warp_to_scan_result(&sig.target_id, WARP_RANGE) {
        Ok(_) => {}
        // The server's minimum-warp refusal (`CustomNotify`, the only
        // key this call can raise besides a scramble's own): the true point
        // is within the minimum warp distance, so the bot is on it already,
        // whatever the blurred estimate in `sig` says. [A] falsified by a
        // second `CustomNotify` on this call.
        Err(Error::Refused(m)) if m.contains("CustomNotify") => return Ok(()),
        Err(e) => return Err(e),
    }
    settle(c, "warp to signature", |c| !warping(c))?;
    let at = c.world_now().ego_ball().map(|b| b.motion.position);
    match at {
        Some(p) if distance(p, sig.position) <= ARRIVED => Ok(()),
        other => Err(Error::Refused(format!(
            "warp to {}: landed at {other:?}, not on {:?}",
            sig.target_id, sig.position
        ))),
    }
}

/// **Save the signature's location** (on it) in the personal folder.
/// Returns the bookmark's id.
pub fn bookmark(c: &mut Conn, sig: &Signature) -> Result<i64> {
    let folder = c.personal_folder(FOLDER_NAME)?;
    let name = format!("{} {}", sig.target_id, sig.kind.name());
    c.save_location(folder, &name, "")
}

// ---- the ledger ------------------------------------------------------------------

/// A server and a system, or a server and a character.
pub type Pitch = (SocketAddr, i64);

/// One signature reported.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub kind: Kind,
    pub position: [f64; 3],
    pub by: i64,
    pub bookmark: Option<i64>,
    pub at: Instant,
    /// The position is a perfect result's point (else the last estimate of
    /// a signature seen and left, [`Ledger::note`]).
    pub exact: bool,
}

#[derive(Debug, Default)]
struct Book {
    found: BTreeMap<Pitch, BTreeMap<String, Found>>,
    done: BTreeMap<Pitch, Instant>,
    homes: BTreeMap<Pitch, i64>,
}

/// **What the explorers of one process have reported**: per server and
/// system, the signatures (by target id), when the system was last scanned
/// out, and each explorer's home station. Bot-side, like
/// `site_runner::Board`: the explorers' own notes, and where a hacker can
/// look for a data site before it scans ([`Ledger::of_kind`]).
#[derive(Debug)]
pub struct Ledger {
    inner: Mutex<Option<Book>>,
}

impl Default for Ledger {
    fn default() -> Ledger {
        Ledger::new()
    }
}

impl Ledger {
    pub const fn new() -> Ledger {
        Ledger {
            inner: Mutex::new(None),
        }
    }

    fn with<T>(&self, f: impl FnOnce(&mut Book) -> T) -> T {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        f(guard.get_or_insert_with(Book::default))
    }

    pub fn record(
        &self,
        pitch: Pitch,
        sig: &Signature,
        by: i64,
        bookmark: Option<i64>,
        at: Instant,
    ) {
        self.with(|b| {
            b.found.entry(pitch).or_default().insert(
                sig.target_id.clone(),
                Found {
                    kind: sig.kind,
                    position: sig.position,
                    by,
                    bookmark,
                    at,
                    exact: true,
                },
            );
        });
    }

    /// **Book a signature seen and left** (Y-ROAM): its kind is known, its
    /// point may not be. Never replaces a perfect record.
    pub fn note(&self, pitch: Pitch, seen: &Seen, by: i64, at: Instant) {
        self.with(|b| {
            let e = b.found.entry(pitch).or_default();
            if e.get(&seen.target_id)
                .is_some_and(|f| f.exact && !seen.exact)
            {
                return;
            }
            e.insert(
                seen.target_id.clone(),
                Found {
                    kind: seen.kind,
                    position: seen.position,
                    by,
                    bookmark: None,
                    at,
                    exact: seen.exact,
                },
            );
        });
    }

    pub fn knows(&self, pitch: Pitch, target_id: &str) -> bool {
        self.with(|b| {
            b.found
                .get(&pitch)
                .is_some_and(|f| f.contains_key(target_id))
        })
    }

    /// Every signature reported on `pitch`.
    pub fn found(&self, pitch: Pitch) -> Vec<(String, Found)> {
        self.with(|b| {
            b.found
                .get(&pitch)
                .map(|f| f.iter().map(|(t, x)| (t.clone(), x.clone())).collect())
                .unwrap_or_default()
        })
    }

    /// `(system, targetID)` of every signature of `kind` reported on
    /// `server`.
    pub fn of_kind(&self, server: SocketAddr, kind: Kind) -> Vec<(i64, String)> {
        self.with(|b| {
            b.found
                .iter()
                .filter(|((s, _), _)| *s == server)
                .flat_map(|((_, system), f)| {
                    f.iter()
                        .filter(|(_, x)| x.kind == kind)
                        .map(|(t, _)| (*system, t.clone()))
                })
                .collect()
        })
    }

    pub fn mark_done(&self, pitch: Pitch, at: Instant) {
        self.with(|b| b.done.insert(pitch, at));
    }

    /// Scanned out less than [`REVISIT`] before `now`.
    pub fn done_recently(&self, pitch: Pitch, now: Instant) -> bool {
        self.with(|b| {
            b.done
                .get(&pitch)
                .is_some_and(|t| now.saturating_duration_since(*t) < REVISIT)
        })
    }

    /// The first station `who` was outfitted at is its home.
    pub fn set_home(&self, who: Pitch, station: i64) {
        self.with(|b| {
            b.homes.entry(who).or_insert(station);
        });
    }

    pub fn home(&self, who: Pitch) -> Option<i64> {
        self.with(|b| b.homes.get(&who).copied())
    }
}

/// The process's ledger: [`Act::Explore`]'s.
pub static LEDGER: Ledger = Ledger::new();

/// **Where to roam next** (pure): a neighbour of `here` the floor allows,
/// within [`ROAM_JUMPS`] of `home`, not `done`; the lowest id first.
pub fn next_system(
    map: &Map,
    floor: f64,
    here: i64,
    home: i64,
    done: impl Fn(i64) -> bool,
) -> Option<i64> {
    map.neighbours(here)
        .into_iter()
        .filter(|&s| map.allowed(s, floor))
        .filter(|&s| map.jumps(home, s, floor).is_some_and(|j| j <= ROAM_JUMPS))
        .find(|&s| !done(s))
}

// ---- flying ----------------------------------------------------------------------

/// Bound in this system, with a state.
pub(crate) fn bound(c: &mut Conn) -> Result<()> {
    if c.ballpark().is_none() {
        let system = in_space(c, "bind")?;
        c.bind_ballpark(system)?;
    }
    settle(c, "bind", |c| c.world().has_state())
}

/// Within `reach` of `ball`'s surface: warp from beyond
/// [`MIN_WARP_DISTANCE`], else approach.
pub(crate) fn reach(c: &mut Conn, ball: i64, reach: f64) -> Result<()> {
    bound(c)?;
    settle(c, "land", |c| !warping(c))?;
    let g =
        gap(c, ball).ok_or_else(|| Error::NotSent(format!("fly: {ball} is not in the park")))?;
    if g >= MIN_WARP_DISTANCE {
        c.warp_to_item(ball, 0.0)?;
        settle(c, "warp", |c| {
            !warping(c) && gap(c, ball).is_some_and(|g| g < MIN_WARP_DISTANCE)
        })?;
    }
    if gap(c, ball).is_some_and(|g| g >= reach) {
        c.follow(ball, 0.0)?;
        settle(c, "approach", |c| gap(c, ball).is_some_and(|g| g < reach))?;
    }
    Ok(())
}

/// **Jump to a neighbouring system** over its gate, and bind there.
pub fn jump_to(c: &mut Conn, map: &Map, to: i64) -> Result<()> {
    let here = in_space(c, "jump")?;
    let (gate, partner) = map
        .gate(here, to)
        .ok_or_else(|| Error::NotSent(format!("jump: no gate from {here} to {to}")))?;
    reach(c, gate, eve_server::jump::MAX_JUMP_DISTANCE)?;
    c.stargate_jump(gate, partner)?;
    bound(c)
}

/// **Bring every probe this character has out in this system home**
/// (`ReconnectToLostProbes` finds them, `RecoverProbes` collects them): a
/// player picks the probes up before a gate. Returns how many came; zero
/// with none out. In space.
pub fn recover_all(c: &mut Conn) -> Result<usize> {
    in_space(c, "recover")?;
    let manager = c.system_scan_manager()?;
    let probes = c.reconnect_probes(&manager)?;
    if probes.is_empty() {
        return Ok(0);
    }
    let ids: Vec<i64> = probes.iter().map(|p| p.probe_id).collect();
    Ok(c.recover_probes(&manager, &ids)?.len())
}

/// **Dock at a station in this system.**
pub fn dock_at(c: &mut Conn, station: i64) -> Result<()> {
    reach(c, station, crate::DOCK_RANGE)?;
    c.dock(station).map(|_| ())
}

// ---- outfitting ------------------------------------------------------------------

/// What [`outfit`] left fitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outfit {
    pub launcher: i64,
    /// Probes in the launcher.
    pub loaded: i64,
    /// Probes in the hold.
    pub spare: i64,
}

/// What [`outfit`]'s NotSent says when the launcher waits on a skill
/// (SKILLGATE; [`crate::acquire::fit_auto_gated`]'s note).
const WAITS_ON_SKILLS: &str = "fit waits on skills";

/// **Docked: fit a launcher, online it, load probes, stock a spare load.**
/// Each step skipped when done; the hangar's first launcher and probe stack.
pub fn outfit(c: &mut Conn) -> Result<Outfit> {
    let station = c
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("outfit: not docked".to_owned()))?;
    let ship = c
        .session()
        .ship_id()
        .ok_or_else(|| Error::NotSent("outfit: no ship".to_owned()))?;
    let launcher = match c.fitted()?.into_iter().find(is_launcher) {
        Some(l) => l,
        None => {
            let spare = c
                .hangar_items()?
                .into_iter()
                .find(is_launcher)
                .ok_or_else(|| {
                    Error::NotSent("outfit: no probe launcher fitted or in the hangar".to_owned())
                })?;
            // SKILLGATE: a launcher the skills do not fit yet is not a fault.
            crate::acquire::fit_auto_gated(c, &[spare.item_id])?
                .map_err(|note| Error::NotSent(format!("outfit: {note}")))?
                .into_iter()
                .find(is_launcher)
                .ok_or_else(|| Error::Refused("outfit: the launcher did not fit".to_owned()))?
        }
    };
    if !c.is_online(launcher.item_id)? {
        crate::acquire::set_online_gated(c, launcher.item_id)?
            .map_err(|note| Error::NotSent(format!("outfit: {note}")))?;
    }
    let hangar: Vec<ItemRow> = c.hangar_items()?.into_iter().filter(is_probe).collect();
    let cargo: Vec<ItemRow> = c.cargo()?.into_iter().filter(is_probe).collect();
    let probe = hangar
        .first()
        .or(cargo.first())
        .map(|r| r.type_id)
        .ok_or_else(|| {
            Error::NotSent("outfit: no scanner probes in the hangar or the hold".to_owned())
        })?;
    let mut loaded = c.loaded_rounds(ship, launcher.flag_id, probe)?;
    let stacks: Vec<i64> = hangar
        .iter()
        .filter(|r| r.type_id == probe)
        .map(|r| r.item_id)
        .collect();
    if loaded < SPARE_PROBES && !stacks.is_empty() {
        match c.load_ammo(
            &[(launcher.item_id, launcher.flag_id)],
            &stacks,
            probe,
            station,
        ) {
            Ok(n) => loaded = n,
            // Full already, or the stack is spoken for: what is loaded holds.
            Err(Error::Refused(_)) if loaded > 0 => {}
            Err(e) => return Err(e),
        }
    }
    let carried: i64 = c
        .cargo()?
        .iter()
        .filter(|r| r.type_id == probe)
        .map(|r| r.quantity)
        .sum();
    let mut spare = carried;
    if carried < SPARE_PROBES
        && let Some(s) = c.hangar_items()?.into_iter().find(|r| r.type_id == probe)
    {
        let n = (SPARE_PROBES - carried).min(s.quantity);
        if c.load_cargo(s.item_id, Some(n)).is_ok() {
            spare += n;
        }
    }
    if loaded <= 0 {
        return Err(Error::NotSent(
            "outfit: the launcher holds no probes".to_owned(),
        ));
    }
    Ok(Outfit {
        launcher: launcher.item_id,
        loaded,
        spare,
    })
}

// ---- the explorer's act ----------------------------------------------------------

/// What one [`explore`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Explored {
    pub outfit: Option<Outfit>,
    pub found: Option<Signature>,
    pub bookmark: Option<i64>,
    /// Why the location was not saved, when it was not.
    pub not_saved: Option<String>,
    pub scans: u32,
    pub recovered: usize,
    pub jumped_to: Option<i64>,
    pub docked_at: Option<i64>,
    /// The system looked bad and the nerve left it unscanned (Y-ROAM).
    pub avoided: Option<crate::explore_roam::Threat>,
    /// Signatures met, not taken, and booked for others (Y-ROAM).
    pub booked_others: usize,
    /// SKILLGATE: no launcher the pilot's skills fit is fitted; the caller
    /// waits and does not undock.
    pub waits_on_skills: bool,
}

impl Explored {
    /// The act line's note.
    pub fn summary(&self) -> String {
        let mut s = Vec::new();
        if let Some(o) = self.outfit {
            s.push(format!(
                "outfitted: launcher {}, {} probes loaded, {} spare",
                o.launcher, o.loaded, o.spare
            ));
        }
        if self.waits_on_skills {
            s.push("a launcher waits on skills".to_owned());
        }
        if let Some(sig) = &self.found {
            s.push(format!(
                "[explore] {} signature {} in {} after {} scans",
                sig.kind.name(),
                sig.target_id,
                sig.solar_system_id,
                self.scans
            ));
        }
        match (self.bookmark, &self.not_saved) {
            (Some(b), _) => s.push(format!("bookmark {b}")),
            (None, Some(why)) => s.push(format!("not bookmarked ({why})")),
            _ => {}
        }
        if self.recovered > 0 {
            s.push(format!("{} probes recovered", self.recovered));
        }
        if let Some(t) = self.avoided {
            s.push(format!("avoided: {t:?}"));
        }
        if self.booked_others > 0 {
            s.push(format!("{} others booked", self.booked_others));
        }
        if let Some(to) = self.jumped_to {
            s.push(format!("roamed to {to}"));
        }
        if let Some(at) = self.docked_at {
            s.push(format!("home to {at}"));
        }
        if s.is_empty() {
            "nothing".to_owned()
        } else {
            s.join("; ")
        }
    }
}

/// **One [`Act::Explore`]** (module header): docked, [`outfit`]; in space,
/// one new signature scanned down, warped to, saved and booked on
/// `ledger`, else roam or go home. The plain explorer: no nerve, so the
/// travel lane's floor and no avoiding ([`explore_with`] has both).
pub fn explore(c: &mut Conn, ledger: &Ledger, trips: Option<Trips<'_>>) -> Result<Explored> {
    explore_with(c, ledger, trips, None)
}

/// **Jump along the route to `to`** at `floor` (else the shortest route at
/// any security: the way back is the way out), one gate at a time, the
/// probes picked up first. Returns the systems entered.
pub fn jump_route(c: &mut Conn, map: &Map, floor: f64, to: i64) -> Result<Vec<i64>> {
    let from = in_space(c, "route")?;
    let route = map
        .route(from, to, floor)
        .or_else(|| map.route(from, to, crate::travel::ANY_SECURITY))
        .ok_or_else(|| Error::NotSent(format!("route: no way from {from} to {to}")))?;
    let mut entered = Vec::new();
    for &next in route.iter().skip(1) {
        match recover_all(c) {
            Ok(_) | Err(Error::NotSent(_) | Error::Refused(_)) => {}
            Err(e) => return Err(e),
        }
        jump_to(c, map, next)?;
        entered.push(next);
    }
    Ok(entered)
}

/// **What the ledger and the danger book say of `system`** for a roaming
/// explorer (Y-ROAM): unworked data and relic signatures, whether nobody
/// scanned it out lately, and what was last seen there.
pub fn intel_of(
    ledger: &Ledger,
    server: SocketAddr,
    system: i64,
    now: Instant,
    worked: impl Fn(&str) -> bool,
) -> crate::explore_roam::Intel {
    let booked = [Kind::Data, Kind::Relic]
        .iter()
        .flat_map(|k| ledger.of_kind(server, *k))
        .filter(|(s, t)| *s == system && !worked(t))
        .count();
    crate::explore_roam::Intel {
        booked,
        fresh: !ledger.done_recently((server, system), now),
        threat: crate::explore_roam::DANGER.recent((server, system), now),
    }
}

/// [`explore`] by a mind's nerve (Y-ROAM): with `nerve`,
///
/// - the floor is the nerve's ([`crate::explore_roam::floor_for`]), not the
///   travel lane's: a bold explorer goes into low-sec;
/// - on arriving it **looks** ([`crate::explore_roam::look`]) and, if the
///   nerve avoids what it saw, books the danger and moves on without
///   scanning;
/// - the signatures it meets and leaves are booked too ([`Ledger::note`]);
/// - it roams by [`crate::explore_roam::next_stop`], up to
///   [`crate::explore_roam::ROAM_JUMPS`] gates from home, and goes home
///   when nothing is worth a visit.
pub fn explore_with(
    c: &mut Conn,
    ledger: &Ledger,
    trips: Option<Trips<'_>>,
    nerve: Option<crate::explore_roam::Nerve>,
) -> Result<Explored> {
    let server = c.socket()?.peer_addr()?;
    let me = c
        .session()
        .char_id()
        .ok_or_else(|| Error::NotSent("explore: no character".to_owned()))?;
    if let Some(station) = c.session().station_id() {
        ledger.set_home((server, me), station);
        // SKILLGATE: a launcher the skills do not fit yet leaves it unfitted,
        // not failed; the skill is queued and it tries again next visit.
        let (fitted, waits) = match outfit(c) {
            Ok(o) => (Some(o), false),
            Err(Error::NotSent(w)) if w.contains(WAITS_ON_SKILLS) => (None, true),
            Err(e) => return Err(e),
        };
        return Ok(Explored {
            outfit: fitted,
            waits_on_skills: waits,
            ..Explored::default()
        });
    }
    let system = in_space(c, "explore")?;
    let pitch = (server, system);
    let mut done = Explored::default();
    let mut scanner = None;
    let threat = match (nerve, trips) {
        (Some(_), Some(t)) => match crate::explore_roam::look(c, Some(t.map)) {
            Ok(t) => t,
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => crate::explore_roam::Threat::Clear,
        },
        _ => crate::explore_roam::Threat::Clear,
    };
    let avoided = nerve.is_some_and(|n| crate::explore_roam::avoid(threat, n));
    if avoided {
        done.avoided = Some(threat);
    } else {
        let mut sc = Scanner::open(c)?;
        if !ledger.done_recently(pitch, Instant::now()) {
            let mut left: Vec<Seen> = Vec::new();
            let found = find_seen(c, &mut sc, |t, _| !ledger.knows(pitch, t), |s| left.push(s))?;
            done.scans = sc.scans;
            for s in &left {
                ledger.note(pitch, s, me, Instant::now());
            }
            done.booked_others = left.len();
            if let Some(sig) = found {
                warp_to(c, &sig)?;
                match bookmark(c, &sig) {
                    Ok(b) => done.bookmark = Some(b),
                    Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                    Err(e) => done.not_saved = Some(e.to_string()),
                }
                ledger.record(pitch, &sig, me, done.bookmark, Instant::now());
                done.found = Some(sig);
                return Ok(done);
            }
            ledger.mark_done(pitch, Instant::now());
        }
        scanner = Some(sc);
    }
    if let Some(mut sc) = scanner {
        done.recovered = sc.recover(c)?;
    }
    let home = ledger.home((server, me));
    match (trips, home) {
        (Some(trips), Some(home)) => {
            let now = Instant::now();
            let home_system = trips.map.system_of(home);
            let roam = match (nerve, home_system) {
                (Some(n), Some(hs)) => {
                    crate::explore_roam::next_stop(trips.map, n, hs, system, |s| {
                        intel_of(ledger, server, s, now, |_| false)
                    })
                }
                (None, Some(hs)) => next_system(trips.map, trips.floor, system, hs, |s| {
                    ledger.done_recently((server, s), now)
                }),
                (_, None) => None,
            };
            let floor = nerve.map_or(trips.floor, crate::explore_roam::floor_for);
            if let Some(to) = roam {
                jump_route(c, trips.map, floor, to)?;
                done.jumped_to = Some(to);
            } else {
                let by = Trips {
                    map: trips.map,
                    floor,
                };
                match crate::travel::go(c, by, home) {
                    Ok(_) => {}
                    // The floor may be what stops the way home: the way
                    // home is the way out, at any security.
                    Err(Error::NotSent(_)) if nerve.is_some() => {
                        let any = Trips {
                            map: trips.map,
                            floor: crate::travel::ANY_SECURITY,
                        };
                        crate::travel::go(c, any, home)?;
                    }
                    Err(e) => return Err(e),
                }
                done.docked_at = Some(home);
            }
            Ok(done)
        }
        (None, Some(home)) => {
            dock_at(c, home)?;
            done.docked_at = Some(home);
            Ok(done)
        }
        (_, None) => Err(Error::Refused(format!(
            "explore: nothing new in {system}, and no home known"
        ))),
    }
}

// ---- the role --------------------------------------------------------------------

/// Scans down signatures and reports them (SOC-B3). A civilian.
pub static ROLE: RoleSpec = RoleSpec {
    name: "explore",
    greeting: "o7 -- out scanning, will share what I find",
    civilian: true,
    docked,
    in_space: space,
    banner,
    setup,
};

/// What the closed economy buys for the explorer (`acquire::kit_of`, by
/// the coordinator's row): the hull, a core launcher and probes. No skill:
/// this server checks none to fit or online one **[A]** (falsified by a
/// fitting refusal for a missing Astrometrics).
pub const KIT: crate::acquire::Kit = crate::acquire::Kit {
    hull: Some(HULL),
    modules: Some((CORE_LAUNCHER, 1)),
    ammo: Some((CORE_PROBE, SETUP_PROBES)),
    skills: &[],
    more: &[],
};

fn setup(lacks: Lacks) -> Vec<String> {
    let mut lines = Vec::new();
    if lacks.ship {
        lines.push(format!("/createitem {HULL}"));
    }
    if lacks.module {
        lines.push(format!("/createitem {CORE_LAUNCHER}"));
    }
    if lacks.ammo {
        lines.push(format!("/createitem {CORE_PROBE} {SETUP_PROBES}"));
    }
    lines
}

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, reconnects {}",
        s.name, s.phase, s.reconnects
    )
}

/// Docked: a hull (board one, or ask), then -- after an [`Act::Explore`]
/// failed and the hangar lacks a launcher or probes -- ask for them; then
/// dwell, outfit and undock.
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    m.claim = None;
    m.homing = None;
    m.stop_sent = false;
    if o.failures.values().any(|&n| n >= brain::GIVE_UP_AFTER) {
        return vec![Act::Reconnect];
    }
    let active = o.ship.map(|s| s.ship_id);
    let in_capsule = o
        .ship
        .is_none_or(|s| s.type_id.is_none_or(|t| t == brain::CAPSULE));
    let launcher = o.hangar.iter().any(is_launcher);
    let probes = o.hangar.iter().chain(o.cargo.iter()).any(is_probe);
    if in_capsule {
        if let Some(hull) = o.hangar.iter().find(|r| {
            r.category_id == crate::acquire::CATEGORY_SHIP
                && r.type_id != brain::CAPSULE
                && Some(r.item_id) != active
                && !m.combat.untrained.contains(&r.type_id)
        }) {
            return vec![Act::Board(hull.item_id)];
        }
        return brain::gm_or_missing(m, true, !launcher, !probes, "a hull in the hangar");
    }
    if o.failed(ActKind::Explore) > 0 && (!launcher || !probes) {
        return brain::gm_or_missing(
            m,
            false,
            !launcher,
            !probes,
            "a probe launcher and scanner probes in the hangar",
        );
    }
    let acts = brain::dwell_then_undock(m, o);
    if acts == [Act::Undock] {
        return vec![Act::Explore, Act::Undock];
    }
    acts
}

/// In space: explore; home after [`brain::GIVE_UP_AFTER`] failed explores
/// or when the driver says so.
fn space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let Some(park) = &o.park else {
        return vec![Act::Wait(SHORT_WAIT)];
    };
    if o.failures
        .iter()
        .any(|(k, &n)| k.is_travel() && n >= brain::GIVE_UP_AFTER)
    {
        return vec![Act::Reconnect];
    }
    if m.homing.is_none() && o.failed(ActKind::Explore) >= brain::GIVE_UP_AFTER {
        m.homing = Some(Homing::Recovery);
    }
    if m.homing.is_some() {
        return brain::go_home(m, o, park);
    }
    if park.ego_warping {
        return vec![Act::Wait(SHORT_WAIT)];
    }
    vec![Act::Explore]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Ball, ParkView, Place, Policy, ShipInfo};

    /// Metres in an AU, as the server rounds it.
    const AU: f64 = 149_597_870_700.0;

    #[test]
    fn a_kind_is_its_strength_attribute_and_unknown_otherwise() {
        let pairs = [
            (SCAN_RADAR, Kind::Data),
            (SCAN_MAGNETOMETRIC, Kind::Relic),
            (SCAN_LADAR, Kind::Gas),
            (SCAN_ALL, Kind::Combat),
            (SCAN_GRAVIMETRIC, Kind::Ore),
            (SCAN_WORMHOLE, Kind::Wormhole),
        ];
        for (a, k) in pairs {
            assert_eq!(Kind::from_strength_attribute(Some(a)), k);
        }
        assert_eq!(Kind::from_strength_attribute(None), Kind::Unknown);
        assert_eq!(Kind::from_strength_attribute(Some(-1)), Kind::Unknown);
        // The server's archetype table maps onto the same kinds.
        for (archetype, attr) in eve_server::population::SCAN_ATTRIBUTES {
            assert_eq!(
                eve_server::population::scan_attribute(archetype),
                attr,
                "{archetype}"
            );
            assert_ne!(Kind::from_strength_attribute(Some(attr)), Kind::Unknown);
        }
    }

    #[test]
    fn steps_cover_the_deviation_and_narrow_or_widen() {
        let base = 0.5 * AU;
        assert_eq!(range_of(base, 1), base);
        assert_eq!(range_of(base, 3), base * RANGE_FACTOR * RANGE_FACTOR);
        assert_eq!(first_step(base, 0.0), 1);
        let s = first_step(base, 7.0 * AU);
        assert!(range_of(base, s) >= 7.0 * AU && range_of(base, s - 1) < 7.0 * AU);
        assert_eq!(first_step(base, 1e9 * AU), RANGE_STEPS);
        assert_eq!(next_step(4, true), Some(3));
        assert_eq!(next_step(1, true), Some(1));
        assert_eq!(next_step(4, false), Some(5));
        assert_eq!(next_step(RANGE_STEPS, false), None);
    }

    /// A result `KeyVal` as `scan::ProbeResult::to_keyval` writes one.
    fn result(certainty: f64, data: PyValue, strength: PyValue) -> PyValue {
        eve_server::pyutil::keyval(vec![
            ("id", PyValue::Bytes(b"ABC-123".to_vec())),
            ("scanGroupID", PyValue::Int(3)),
            ("certainty", PyValue::Real(certainty)),
            ("pos", tuple3([1.0, 2.0, 3.0])),
            ("data", data),
            ("strengthAttributeID", strength),
        ])
    }

    /// Results, probes and the overlay read back: a perfect result is a
    /// point with its kind, a partial one a sphere.
    #[test]
    fn results_probes_and_the_overlay_read_back_from_the_servers_shapes() {
        let perfect = result(1.0, tuple3([1.0, 2.0, 3.0]), PyValue::Int(SCAN_RADAR));
        let r = ScanResult::from_value(&perfect).expect("reads");
        assert!(r.is_perfect());
        assert_eq!(r.point, Some([1.0, 2.0, 3.0]));
        assert_eq!(r.kind(), Some(Kind::Data));
        let partial = result(0.1, PyValue::Real(5.0), PyValue::None);
        let r = ScanResult::from_value(&partial).expect("reads");
        assert!(!r.is_perfect());
        assert_eq!(r.deviation, Some(5.0));
        assert_eq!(r.kind(), None, "not disclosed below RESULT_GOOD");

        let p = ProbeState {
            probe_id: 9,
            type_id: 8,
            pos: [4.0, 5.0, 6.0],
            scan_range: 4.0 * AU,
            range_step: 3,
            state: 1,
            expiry: 10,
        };
        let back = ProbeState::from_value(&p.to_value()).expect("reads");
        assert_eq!(back, p);
        assert!((back.base_range() - AU).abs() < 1.0);

        let info = eve_server::signal_tracker::signature_info([7.0, 8.0, 9.0], "XYZ-001", 1, 3.0);
        let s = OverlaySite::from_value(5, &info).expect("reads");
        assert_eq!(s.target_id, "XYZ-001");
        assert_eq!(s.position, [7.0, 8.0, 9.0]);
        assert_eq!(s.deviation, 3.0);
    }

    #[test]
    fn the_ledger_knows_what_was_reported_and_what_was_scanned_out() {
        let ledger = Ledger::new();
        let server: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let sig = Signature {
            solar_system_id: 11,
            target_id: "AAA-111".to_owned(),
            kind: Kind::Relic,
            position: [0.0; 3],
        };
        let now = Instant::now();
        assert!(!ledger.knows((server, 11), "AAA-111"));
        ledger.record((server, 11), &sig, 1, Some(5), now);
        assert!(ledger.knows((server, 11), "AAA-111"));
        assert!(!ledger.knows((server, 12), "AAA-111"));
        assert_eq!(
            ledger.of_kind(server, Kind::Relic),
            [(11, "AAA-111".to_owned())]
        );
        assert!(ledger.of_kind(server, Kind::Data).is_empty());
        assert!(!ledger.done_recently((server, 11), now));
        ledger.mark_done((server, 11), now);
        assert!(ledger.done_recently((server, 11), now));
        assert!(!ledger.done_recently((server, 11), now + REVISIT));
        ledger.set_home((server, 1), 60);
        ledger.set_home((server, 1), 61);
        assert_eq!(ledger.home((server, 1)), Some(60), "the first station");
    }

    /// Invented: a line of systems 1 - 2 - 3 - 4, all allowed.
    #[test]
    fn roaming_stays_near_home_and_skips_what_is_done() {
        let map = Map::new()
            .with_gate(1, 101, 2, 201)
            .with_gate(2, 201, 1, 101)
            .with_gate(2, 202, 3, 301)
            .with_gate(3, 301, 2, 202)
            .with_gate(3, 302, 4, 401)
            .with_gate(4, 401, 3, 302);
        let any = crate::travel::ANY_SECURITY;
        assert_eq!(next_system(&map, any, 1, 1, |_| false), Some(2));
        assert_eq!(next_system(&map, any, 2, 1, |s| s == 1), Some(3));
        let far = 1 + ROAM_JUMPS as i64;
        assert_eq!(
            next_system(&map, any, far, 1, |s| s < far),
            None,
            "beyond ROAM_JUMPS of home"
        );
        assert_eq!(next_system(&map, any, 2, 1, |_| true), None);
    }

    fn memory() -> Memory {
        Memory::new(
            "Explorer",
            Policy {
                role: &ROLE,
                ..Policy::fast()
            },
        )
    }

    fn row(item_id: i64, group_id: i64, category_id: i64) -> ItemRow {
        ItemRow {
            item_id,
            type_id: item_id + 1_000,
            owner_id: 1,
            location_id: 60,
            flag_id: 4,
            quantity: 1,
            group_id,
            category_id,
            singleton: false,
        }
    }

    fn docked_obs() -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.place = Place::Docked { station: 60 };
        o.ship = Some(ShipInfo {
            ship_id: 5,
            type_id: Some(HULL),
        });
        o
    }

    #[test]
    fn docked_it_boards_a_hull_asks_for_gear_and_outfits_on_the_way_out() {
        let mut m = memory();
        let mut o = docked_obs();
        o.ship = Some(ShipInfo {
            ship_id: 5,
            type_id: Some(brain::CAPSULE),
        });
        o.hangar = vec![row(7, 25, crate::acquire::CATEGORY_SHIP)];
        assert_eq!(docked(&mut m, &o), [Act::Board(7)]);
        o.hangar.clear();
        assert_eq!(
            docked(&mut m, &o),
            [Act::GmSetup {
                ship: true,
                module: true,
                ammo: true
            }]
        );

        let mut m = memory();
        let mut o = docked_obs();
        assert_eq!(
            docked(&mut m, &o),
            [Act::Explore, Act::Undock],
            "a fast policy has no dwell"
        );
        o.failures.insert(ActKind::Explore, 1);
        o.hangar = vec![row(8, GROUP_PROBE, 8)];
        assert_eq!(
            docked(&mut m, &o),
            [Act::GmSetup {
                ship: false,
                module: true,
                ammo: false
            }]
        );
    }

    fn in_space_obs() -> Observation {
        let mut o = docked_obs();
        o.place = Place::InSpace {
            system: 30,
            bound: true,
        };
        let ego = Ball {
            id: 5,
            position: [0.0; 3],
            radius: 30.0,
        };
        o.park = Some(ParkView {
            ego,
            ego_warping: false,
            rocks: Vec::new(),
            belts: Vec::new(),
            station: Some(Ball {
                id: 60,
                position: [1e9, 0.0, 0.0],
                radius: 5_000.0,
            }),
            boss: None,
            taken: BTreeMap::new(),
            hostiles: Vec::new(),
            wrecks: Vec::new(),
            ego_tether: None,
        });
        o
    }

    #[test]
    fn in_space_it_explores_and_goes_home_after_repeated_failures() {
        let mut m = memory();
        let mut o = in_space_obs();
        assert_eq!(space(&mut m, &o), [Act::Explore]);
        o.failures.insert(ActKind::Explore, brain::GIVE_UP_AFTER);
        assert_eq!(space(&mut m, &o), [Act::WarpToItem { id: 60, range: 0.0 }]);
        assert_eq!(m.homing, Some(Homing::Recovery));
    }
}
