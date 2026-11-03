//! The signal tracker: the sensor overlay's anomalies and signatures.
//!
//! W12 packet CP17 (`docs/content/plan.md` §6.2). Spec: C2 §2.1-§2.4, and
//! CCP's own server half, shipped in the client as
//! `explorationscanner/signalTracker` [V]. That file is mirrored here:
//!
//! - `scanMgr.SignalTrackerRegister` marks the caller new ([`register`]);
//! - each tick ([`tick`]) narrowcasts the batched
//!   `OnSignalTracker{Anomaly,Signature}Update(ssid, added, removed)` to the
//!   active pilots, then `OnSignalTrackerFullState(ssid, fullState)` to the
//!   new ones, who become active;
//! - a site appearing ([`add_site`]) or going ([`remove_site`]) is batched,
//!   and singlecast to the system at once as `OnCosmicAnomalyAdded(KeyVal)`,
//!   `OnCosmicSignatureAdded(targetID)` or
//!   `OnCosmicAnomalyRemoved(instanceID, dungeonID, ssid)`;
//! - `GetFullState` answers [`full_state`] synchronously (the docked path).
//!
//! Sites come from CP21's population stage; with none, every answer is
//! CCP's own empty, `({}, {}, {}, {})`. Static sites and structures are
//! always empty dicts: nothing here makes either.
//!
//! W14 packet CP29 puts the sites on the rest of the wire (C2 §2.4, §3):
//! the Agency's `dungeonInstanceCacheMgr` lists ([`agency_instances`],
//! [`agency_counts`]), the probe scanner's signature results
//! ([`signatures`], a `scan_sources` row) and warp to a scanned-down
//! signature ([`signature_target`], a `scan` warp row).
//!
//! **One deliberate deviation.** CCP leaves a site added and removed in the
//! same tick in both halves of the update. The client applies `removed`
//! before `added` (`spacesitecontroller.ProcessSiteDataUpdate:162-168`
//! [V]), so that would leave a ghost; a remove here also withdraws the
//! pending add.

use crate::content::Content;
use crate::model::Target;
use crate::pyutil::{bstr, keyval, new_style};
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// `groupCosmicAnomaly` and `groupCosmicSignature`; the tracker drops a site
/// whose ball is in any other group [V] (`inventorycommon/const`,
/// `signalTracker.AddDungeonToCorrectGrouping`).
pub const GROUP_ANOMALY: i64 = 885;
pub const GROUP_SIGNATURE: i64 = 502;

const ANOMALY_CLASS: &str = "explorationscanner.common.cosmicAnomalyInfo.CosmicAnomalyInfo";
const SIGNATURE_CLASS: &str = "explorationscanner.common.signatureInfo.SignatureInfo";

/// The two batched kinds, by index into [`Tracked`]'s arrays.
const ANOMALY: usize = 0;
const SIGNATURE: usize = 1;
const UPDATES: [&str; 2] = [
    "OnSignalTrackerAnomalyUpdate",
    "OnSignalTrackerSignatureUpdate",
];

/// One live site, as CP21 hands it over (C2 §2.4's instance record). The
/// dungeon's name, archetype, faction, difficulty and entry type are read
/// from `dungeon_data` at emit time, so they are not here.
#[derive(Debug, Clone, PartialEq)]
pub struct Site {
    /// The site ball's item id (`signatureID`): the key in every dict.
    pub site_id: i64,
    pub instance_id: i64,
    pub dungeon_id: i64,
    pub solar_system_id: i64,
    pub position: [f64; 3],
    /// The ball's group: [`GROUP_ANOMALY`] or [`GROUP_SIGNATURE`].
    pub group_id: i64,
    pub is_scannable: bool,
    pub scan_strength_attribute: i64,
    pub scan_strength_value: f64,
    pub signature_radius: f64,
    /// `ABC-123`, stable per instance.
    pub target_id: String,
    /// `dungeon_data::allowed_types`, or empty when unrestricted: the client
    /// reads `not allowedTypes or typeID in allowedTypes` and iterates it
    /// (`agencyNew/contentPieces/baseContentPiece:351,368` [V]), so a list,
    /// never `None`.
    pub allowed_types: Vec<i64>,
}

/// The world slot: per system, its sites, its pilots and this tick's deltas.
#[derive(Debug, Default)]
pub struct Tracker {
    systems: BTreeMap<i64, Tracked>,
    /// Per character, the signatures their probes have a perfect result
    /// on: the only ones [`signature_target`] warps them to.
    resolved: BTreeMap<i64, BTreeSet<i64>>,
    /// [`cold_counts`]' answer, kept until the world's seed or delta rows
    /// change. Behind a lock because the readers hold only `&Registry`.
    cold: std::sync::Mutex<Vec<ColdCache>>,
}

/// What a want's cold-system counts were computed from, and the counts.
#[derive(Debug)]
struct ColdCache {
    seed: u64,
    rev: u64,
    want: (bool, Vec<i64>),
    counts: std::sync::Arc<BTreeMap<i64, i64>>,
}

#[derive(Debug, Default)]
struct Tracked {
    sites: BTreeMap<i64, Site>,
    new_clients: BTreeSet<i64>,
    active: BTreeSet<i64>,
    added: [BTreeMap<i64, PyValue>; 2],
    removed: [BTreeSet<i64>; 2],
}

/// Registered in `world_ext::SLOTS` by the coordinator. Builds empty; reads
/// no deltas (sites are CP21's, and it persists them).
pub const SLOT: SlotSpec = SlotSpec {
    name: "signal_tracker",
    make,
};

fn make(_: &Content) -> Boxed {
    Box::new(Tracker::default())
}

/// The push: one event, its arguments, and the characters it is for.
#[derive(Debug)]
pub struct TrackerPush {
    pub event: &'static str,
    pub args: Vec<PyValue>,
    pub to: Vec<i64>,
}

impl crate::notify::Push for TrackerPush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (self.event, self.args.clone())
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `CalculateDeviationForSignal` [V] (`signalTracker`): 8 AU below a signal
/// of 2, 4 AU above 20, linear between, where the signal is
/// `sigRadius / scanStrength`.
pub fn deviation(sig_radius: f64, scan_strength: f64) -> f64 {
    let (min_dev, max_dev) = (8.0 * crate::scan::AU, 4.0 * crate::scan::AU);
    let s = sig_radius / scan_strength;
    if s < 2.0 {
        min_dev
    } else if s > 20.0 {
        max_dev
    } else {
        max_dev + (1.0 - (s - 2.0) / 18.0) * (min_dev - max_dev)
    }
}

/// `sensorsuite/common.MapDungeonDifficulty` [V]: below 1 (or `None`) is 1.
fn difficulty(d: Option<i64>) -> i64 {
    d.filter(|&d| d >= 1).unwrap_or(1)
}

/// Which dict a site goes in, if any [V]: scannable, an anomaly or
/// signature ball, and a signature only with a positive strength.
fn kind(s: &Site) -> Option<usize> {
    match s.group_id {
        _ if !s.is_scannable => None,
        GROUP_ANOMALY => Some(ANOMALY),
        GROUP_SIGNATURE if s.scan_strength_value > 0.0 => Some(SIGNATURE),
        _ => None,
    }
}

fn opt(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

fn vec3(p: [f64; 3]) -> PyValue {
    PyValue::Tuple(p.iter().map(|&x| PyValue::Real(x)).collect())
}

/// `CosmicAnomalyInfo.__dict__`, in `__init__` order (`SiteInfo`'s five,
/// then the subclass's eight) [V].
fn anomaly_state(reg: &Registry, s: &Site) -> Vec<(&'static str, PyValue)> {
    let d = reg
        .content()
        .try_get::<crate::dungeon_data::Table>()
        .and_then(|t| t.dungeons.get(&s.dungeon_id));
    vec![
        ("position", vec3(s.position)),
        ("targetID", bstr(&s.target_id)),
        (
            "difficulty",
            PyValue::Int(difficulty(d.and_then(|d| d.difficulty))),
        ),
        ("dungeonID", PyValue::Int(s.dungeon_id)),
        ("archetypeID", opt(d.map(|d| d.archetype))),
        ("siteID", PyValue::Int(s.instance_id)),
        ("instanceID", PyValue::Int(s.instance_id)),
        ("dungeonNameID", opt(d.and_then(|d| d.name_id))),
        ("factionID", opt(d.and_then(|d| d.faction))),
        (
            "scanStrengthAttribute",
            PyValue::Int(s.scan_strength_attribute),
        ),
        (
            "allowedTypes",
            PyValue::List(s.allowed_types.iter().map(|&t| PyValue::Int(t)).collect()),
        ),
        ("entryObjectTypeID", opt(d.and_then(|d| d.entry_type))),
        ("solarSystemID", PyValue::Int(s.solar_system_id)),
    ]
}

/// `SignatureInfo.__dict__`, in `__init__` order [V]: `dungeonID` and
/// `archetypeID` are always `None`, so a signature does not give itself away.
pub fn signature_info(position: [f64; 3], target_id: &str, diff: i64, dev: f64) -> PyValue {
    new_style(
        SIGNATURE_CLASS,
        vec![
            ("position", vec3(position)),
            ("targetID", bstr(target_id)),
            ("difficulty", PyValue::Int(diff)),
            ("dungeonID", PyValue::None),
            ("archetypeID", PyValue::None),
            ("deviation", PyValue::Real(dev)),
        ],
    )
}

/// A signature's blurred position: the true one plus
/// `RandomVector(random() * deviation)` [V]. CCP caches it per site so every pilot sees the same
/// circle; here it is a pure function of the world seed and the site [A],
/// so it also survives a restart. Falsified by nothing the client checks.
pub fn blurred(seed: u64, s: &Site, dev: f64) -> [f64; 3] {
    let ids = [s.site_id as u64, s.instance_id as u64];
    let mut r = crate::seed::stream(seed, crate::seed::Kind::Sig, &ids);
    let (z, phi, len) = (
        2.0 * r.unit() - 1.0,
        std::f64::consts::TAU * r.unit(),
        r.unit() * dev,
    );
    let ring = (1.0 - z * z).sqrt();
    let dir = [ring * phi.cos(), ring * phi.sin(), z];
    std::array::from_fn(|i| s.position[i] + dir[i] * len)
}

/// The info object for `s`, and which dict it belongs in.
fn info(reg: &Registry, s: &Site) -> Option<(usize, PyValue)> {
    let k = kind(s)?;
    Some((
        k,
        if k == ANOMALY {
            new_style(ANOMALY_CLASS, anomaly_state(reg, s))
        } else {
            let dev = deviation(s.signature_radius, s.scan_strength_value);
            let diff = reg
                .content()
                .try_get::<crate::dungeon_data::Table>()
                .and_then(|t| t.dungeons.get(&s.dungeon_id))
                .and_then(|d| d.difficulty);
            let at = blurred(reg.world().seed, s, dev);
            signature_info(at, &s.target_id, difficulty(diff), dev)
        },
    ))
}

/// Where the character is (`Registry::session_system`).
fn system_of(reg: &Registry, actor: i64) -> Option<i64> {
    reg.session_system(actor)
}

fn dict(entries: impl IntoIterator<Item = (i64, PyValue)>) -> PyValue {
    PyValue::Dict(
        entries
            .into_iter()
            .map(|(k, v)| (v, PyValue::Int(k)))
            .collect(),
    )
}

fn system_full_state(reg: &Registry, ssid: i64) -> PyValue {
    let mut parts = [Vec::new(), Vec::new()];
    if let Some(t) = reg.ext::<Tracker>().systems.get(&ssid) {
        for (&id, s) in &t.sites {
            if let Some((k, v)) = info(reg, s) {
                parts[k].push((id, v));
            }
        }
    }
    let [anomalies, signatures] = parts;
    PyValue::Tuple(vec![dict(anomalies), dict(signatures), dict([]), dict([])])
}

/// `GetFullState`: `(anomalies, signatures, staticSites, structures)`, four
/// dicts keyed by site ball id, for the caller's system [V]
/// (`sensorSuiteService.OnSignalTrackerFullState`, `UNPACK_SEQUENCE 4`).
pub fn full_state(reg: &Registry, actor: Option<i64>) -> PyValue {
    let ssid = actor.and_then(|a| system_of(reg, a)).unwrap_or(0);
    system_full_state(reg, ssid)
}

/// `SignalTrackerRegister`: the caller is new in its system, and leaves any
/// other; the full state goes out on the next [`tick`]. The client discards
/// the answer, so it is `None`.
pub fn register(reg: &mut Registry, actor: Option<i64>) -> PyValue {
    if let Some((a, ssid)) = actor.and_then(|a| Some((a, system_of(reg, a)?))) {
        let t = reg.ext_mut::<Tracker>();
        for s in t.systems.values_mut() {
            s.new_clients.remove(&a);
            s.active.remove(&a);
        }
        t.systems.entry(ssid).or_default().new_clients.insert(a);
    }
    PyValue::None
}

/// Everyone in the system, for the singlecasts (`SinglecastBySolarSystemID2`).
fn to_system(reg: &mut Registry, ssid: i64, event: &'static str, args: Vec<PyValue>) {
    let to = reg
        .roster()
        .in_system(ssid)
        .iter()
        .map(|p| p.character_id)
        .collect();
    reg.schedule(Duration::ZERO, TrackerPush { event, args, to });
}

/// A site appeared (`OnDungeonInstanceAdded`). Batched for the next tick and
/// singlecast to the system now.
pub fn add_site(reg: &mut Registry, site: Site) {
    let (ssid, id) = (site.solar_system_id, site.site_id);
    let tracked = info(reg, &site);
    let keyval_state = (kind(&site) == Some(ANOMALY)).then(|| anomaly_state(reg, &site));
    let target = bstr(&site.target_id);
    let t = reg.ext_mut::<Tracker>().systems.entry(ssid).or_default();
    t.sites.insert(id, site);
    let Some((k, v)) = tracked else { return };
    t.added[k].insert(id, v);
    match keyval_state {
        Some(state) => to_system(reg, ssid, "OnCosmicAnomalyAdded", vec![keyval(state)]),
        None => to_system(reg, ssid, "OnCosmicSignatureAdded", vec![target]),
    }
}

/// A site went (`OnDungeonInstanceRemoved`).
pub fn remove_site(reg: &mut Registry, ssid: i64, site_id: i64) {
    for done in reg.ext_mut::<Tracker>().resolved.values_mut() {
        done.remove(&site_id);
    }
    let Some(t) = reg.ext_mut::<Tracker>().systems.get_mut(&ssid) else {
        return;
    };
    let Some(site) = t.sites.remove(&site_id) else {
        return;
    };
    let Some(k) = kind(&site) else { return };
    t.added[k].remove(&site_id);
    t.removed[k].insert(site_id);
    if k == ANOMALY {
        let args = vec![
            PyValue::Int(site.instance_id),
            PyValue::Int(site.dungeon_id),
            PyValue::Int(ssid),
        ];
        to_system(reg, ssid, "OnCosmicAnomalyRemoved", args);
    }
}

/// The tick stage (`DoUpdateTick`): deltas to the active pilots, then the
/// full state to the new ones. A pilot no longer in the system is dropped
/// first; they register again on their next `OnEnterSpace`.
pub fn tick(w: &mut Registry, _now: Instant, _ft: crate::damage::BlueTime) {
    let systems: Vec<i64> = w.ext::<Tracker>().systems.keys().copied().collect();
    let mut out = Vec::new();
    for ssid in systems {
        let here: BTreeSet<i64> = w
            .roster()
            .in_system(ssid)
            .iter()
            .map(|p| p.character_id)
            .collect();
        let Some(t) = w.ext_mut::<Tracker>().systems.get_mut(&ssid) else {
            continue;
        };
        t.new_clients.retain(|c| here.contains(c));
        t.active.retain(|c| here.contains(c));
        for (k, event) in UPDATES.iter().enumerate() {
            let added = std::mem::take(&mut t.added[k]);
            let removed = std::mem::take(&mut t.removed[k]);
            if (added.is_empty() && removed.is_empty()) || t.active.is_empty() {
                continue;
            }
            let removed = PyValue::List(removed.into_iter().map(PyValue::Int).collect());
            let args = vec![PyValue::Int(ssid), dict(added), removed];
            out.push(TrackerPush {
                event,
                args,
                to: t.active.iter().copied().collect(),
            });
        }
        let fresh = std::mem::take(&mut t.new_clients);
        t.active.extend(&fresh);
        if t.sites.is_empty() && t.active.is_empty() {
            w.ext_mut::<Tracker>().systems.remove(&ssid);
        }
        if !fresh.is_empty() {
            let args = vec![PyValue::Int(ssid), system_full_state(w, ssid)];
            let to = fresh.into_iter().collect();
            out.push(TrackerPush {
                event: "OnSignalTrackerFullState",
                args,
                to,
            });
        }
    }
    for p in out {
        w.schedule(Duration::ZERO, p);
    }
}

/// The second `item` warp row (S8): an anomaly's site id, which has no ball,
/// in the warper's system. Signatures are warped to by `scan`, and the
/// client resolves `item` targets over anomalies and structures only
/// (`sensorSuiteService.GetPositionalSiteItemIDFromTargetID:995-999` [V]).
pub fn anomaly_target(w: &Registry, actor: i64, id: i64, _raw: Option<&PyValue>) -> Option<Target> {
    let s = w
        .ext::<Tracker>()
        .systems
        .get(&system_of(w, actor)?)?
        .sites
        .get(&id)?;
    (kind(s) == Some(ANOMALY)).then_some(Target {
        position: s.position,
        radius: 0.0,
        warp_in: None,
    })
}

// ---------------------------------------------------------------------------
// CP29: the Agency, the probe scanner and warp-to-site
// ---------------------------------------------------------------------------

/// `dunArchetypeCombatSites`, `dunArchetypeOreAnomaly` and
/// `dunArchetypeIceBelt` [V] (`eve/common/lib/appConst`): the
/// `ARCHETYPE_IDS` of `anomalyTracker`'s combat, ore and ice trackers.
pub const ARCHETYPE_COMBAT: i64 = 24;
pub const ARCHETYPE_ORE: i64 = 27;
pub const ARCHETYPE_ICE: i64 = 28;

/// `probeScanGroupSignatures` [V] (`probescanning/const`): the scan group a
/// signature result is filed under.
pub const SCAN_GROUP_SIGNATURES: i64 = 3;

/// Which sites an Agency list counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// Anomalies whose dungeon has one of these archetypes:
    /// `anomalyTracker.is_site_relevant` is `entry_type_id ==
    /// typeCosmicAnomaly and archetype_id in self.ARCHETYPE_IDS` [V].
    Anomalies(&'static [i64]),
    /// Every signature (`GetSignatureInstances`).
    Signatures,
}

/// `GetCombatAnomalyInstances` and `GetCombatAnomaliesCount`.
pub const COMBAT: Want = Want::Anomalies(&[ARCHETYPE_COMBAT]);
/// `GetOreAnomalyInstances` and `GetOreAnomaliesCount`.
pub const ORE: Want = Want::Anomalies(&[ARCHETYPE_ORE]);
/// `GetIceBeltInstances` and `GetIceBeltsCount`.
pub const ICE: Want = Want::Anomalies(&[ARCHETYPE_ICE]);

/// `GetFactionWarfareInstances`: the four complexes (`dunArchetypeComplex*`
/// [V], the same four `fw_lp::LP_BY_ARCHETYPE` pays).
pub const FW: Want = Want::Anomalies(&[33, 34, 35, 36]);

impl Want {
    /// Whether a site with this entry and dungeon counts. An anomaly's
    /// dungeon must be in the catalogue: the client indexes `GetDungeon` by
    /// every row's `dungeonID` (`siteSystemContentPiece` [V], C2 §3).
    fn takes(self, reg: &Registry, anomaly: bool, dungeon_id: i64) -> bool {
        match self {
            Want::Signatures => !anomaly,
            Want::Anomalies(archetypes) => {
                anomaly
                    && reg
                        .content()
                        .try_get::<crate::dungeon_data::Table>()
                        .and_then(|t| t.dungeons.get(&dungeon_id))
                        .is_some_and(|d| archetypes.contains(&d.archetype))
            }
        }
    }
}

/// `{solarSystemID: n}` for every system with at least one wanted site.
///
/// A system the tracker holds (warm or hot) counts its live sites. A cold
/// one counts population's slots without resolving them: each slot's draw
/// at the generation its `site` delta names (0 with none), and a `done`
/// slot is gone. **[A]** a done slot that has passed its `respawn_s` still
/// counts as gone until its system warms; falsified by a pilot who finds
/// more sites on arrival than the Agency showed.
fn counts(reg: &Registry, want: Want) -> BTreeMap<i64, i64> {
    let t = reg.ext::<Tracker>();
    let mut out = BTreeMap::new();
    for (&ssid, tracked) in &t.systems {
        let n = tracked
            .sites
            .values()
            .filter(|s| kind(s).is_some_and(|k| want.takes(reg, k == ANOMALY, s.dungeon_id)))
            .count();
        out.insert(ssid, n as i64);
    }
    for (&ssid, &n) in cold_counts(reg, want).iter() {
        out.entry(ssid).or_insert(n);
    }
    out.retain(|_, n| *n > 0);
    out
}

/// The cold-system half of [`counts`]: every universe system's slot count as
/// if the tracker did not hold it (the caller skips the ones it does).
/// Walking every system's slots and draws is the cost of an Agency call, and
/// the answer moves only with the seed and the `site` delta rows, so one
/// want's answer is kept under that kind's revision (the 14-22 ms per call PA-0 found).
pub(crate) fn cold_counts(reg: &Registry, want: Want) -> std::sync::Arc<BTreeMap<i64, i64>> {
    use std::sync::Arc;
    let key = match want {
        Want::Anomalies(a) => (true, a.to_vec()),
        Want::Signatures => (false, vec![]),
    };
    // `compute_cold` reads the seed, the fixed prepared tables and the
    // population `site` rows, and nothing else, so only that kind's
    // revision keys it (the global one moved with every `account` write).
    let (seed, rev) = (
        reg.world().seed,
        reg.deltas().kind_revision(crate::population::KIND),
    );
    let slot = &reg.ext::<Tracker>().cold;
    let hit = slot
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|c| (c.seed, c.rev, &c.want) == (seed, rev, &key))
        .map(|c| Arc::clone(&c.counts));
    if let Some(hit) = hit {
        return hit;
    }
    let counts = Arc::new(compute_cold(reg, want));
    let mut held = slot.lock().unwrap_or_else(|e| e.into_inner());
    held.retain(|c| (c.seed, c.rev) == (seed, rev));
    held.push(ColdCache {
        seed,
        rev,
        want: key,
        counts: Arc::clone(&counts),
    });
    counts
}

pub(crate) fn compute_cold(reg: &Registry, want: Want) -> BTreeMap<i64, i64> {
    use crate::population::{KIND, Population};
    let mut out = BTreeMap::new();
    let Some(universe) = reg.content().try_get::<crate::celestial::Universe>() else {
        return out;
    };
    let (prep, seed) = (reg.ext::<Population>().prepared(), reg.world().seed);
    let deltas = reg.deltas().of(KIND);
    for sys in universe.systems() {
        let mut n = 0;
        for slot in prep.slots(seed, sys.id, 0) {
            let delta = deltas.get(&prep.delta_key(sys.id, slot.row, slot.k));
            if delta.is_some_and(|v| v.get(1).is_some_and(|w| w == "done")) {
                continue;
            }
            let generation = delta.and_then(|v| v.first()?.parse().ok()).unwrap_or(0);
            let anomaly = prep.rows()[slot.row].entry == crate::dungeon_data::Entry::Anomaly;
            if prep
                .draw(seed, sys.id, slot.row, slot.k, generation)
                .is_some_and(|d| want.takes(reg, anomaly, d))
            {
                n += 1;
            }
        }
        if n > 0 {
            out.insert(sys.id, n);
        }
    }
    out
}

fn int_dict(counts: impl IntoIterator<Item = (i64, i64)>) -> Vec<(PyValue, PyValue)> {
    counts
        .into_iter()
        .map(|(ssid, n)| (PyValue::Int(n), PyValue::Int(ssid)))
        .collect()
}

/// `dungeonInstanceCacheMgr.Get{CombatAnomaly,OreAnomaly,IceBelt}Instances`:
/// `{solarSystemID: [row, ...] | n}` (`anomalyTracker._unpack` [V]). The
/// caller's own system gets rows, every other system a count (C2 §3, the
/// policy [A]: a pilot knows only the rows of the system they are in;
/// falsified if the Agency's distance filter needs remote positions).
///
/// A row is a `KeyVal` of the anomaly's `CosmicAnomalyInfo` state: a
/// superset of the `siteID`, `factionID`, `dungeonNameID`, `position` and
/// `dungeonID` the Agency piece reads, and of the `instanceID` the tracker's
/// cache reads (`anomalyTracker._get_cached_site` [V]).
pub fn agency_instances(reg: &Registry, actor: Option<i64>, want: Want) -> PyValue {
    let own = actor.and_then(|a| system_of(reg, a));
    let mut out = int_dict(
        counts(reg, want)
            .into_iter()
            .filter(|(s, _)| Some(*s) != own),
    );
    if let Some(ssid) = own {
        let rows: Vec<PyValue> = reg
            .ext::<Tracker>()
            .systems
            .get(&ssid)
            .into_iter()
            .flat_map(|t| t.sites.values())
            .filter(|s| kind(s) == Some(ANOMALY) && want.takes(reg, true, s.dungeon_id))
            .map(|s| keyval(anomaly_state(reg, s)))
            .collect();
        if !rows.is_empty() {
            out.push((PyValue::List(rows), PyValue::Int(ssid)));
        }
    }
    PyValue::Dict(out)
}

/// `GetFactionWarfareInstances` (FW-6): the FW complexes (archetypes 33-36,
/// `dunArchetypeComplex*` [V]) as `{solarSystemID: [row, ...]}`. **Rows
/// only, never the ints [`agency_instances`] gives other systems**: the FW
/// tracker's `_unpack` iterates each value and reads `.factionID` off every
/// element [V], so an int is a `TypeError`. The systems are the ones the
/// tracker holds (warm or hot, the caller's own among them); a cold system
/// is omitted **[A]**. Falsified by the Agency missing complexes pilots
/// find on arrival. The client itself shows only the occupier's complexes,
/// so the attacker's stay off the Agency; that is left to it.
pub fn fw_instances(reg: &Registry) -> PyValue {
    let mut out = Vec::new();
    for (&ssid, tracked) in &reg.ext::<Tracker>().systems {
        let rows: Vec<PyValue> = tracked
            .sites
            .values()
            .filter(|s| kind(s) == Some(ANOMALY) && FW.takes(reg, true, s.dungeon_id))
            .map(|s| keyval(anomaly_state(reg, s)))
            .collect();
        if !rows.is_empty() {
            out.push((PyValue::List(rows), PyValue::Int(ssid)));
        }
    }
    PyValue::Dict(out)
}

/// `Get{CombatAnomalies,OreAnomalies,IceBelts}Count` and
/// `GetSignatureInstances`: `{solarSystemID: n}` over every system, ints and
/// never rows (C2 §3). The client lets its own known rows override the
/// count (`BaseContentProviderCosmicAnomaliesWithUnknown` updates the
/// unknown dict with the known one; `ContentProviderSignatures` replaces the
/// own system with the sensor suite's sites [V]).
pub fn agency_counts(reg: &Registry, want: Want) -> PyValue {
    PyValue::Dict(int_dict(counts(reg, want)))
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// **The scan source** (`scan_sources::SOURCES`): every signature in the
/// probes' system that they hear, as a `scanGroupID` 3 result.
///
/// The certainty is `scan::certainty` over the site's signature radius,
/// from every probe type **[A]** (the core and combat probes both hear
/// sites; falsified by a combat probe that never resolves a signature on
/// TQ). Below perfect the result is a sphere around a point off the truth
/// by at most half its radius, the radius being the smallest hearing range
/// shrunk by the certainty **[A]**, as `scan_ships` does. At
/// `RESULT_PERFECT` it is the true position, and the scanning character may
/// warp to it ([`signature_target`]).
///
/// Anomalies are not results: the client injects them into the probe window
/// itself (`scanSvc` `InjectScannerResults`, C2 §2.3 [V]).
pub fn signatures(
    w: &mut Registry,
    character_id: i64,
    probes: &[crate::scan::Probe],
) -> Vec<crate::scan::ProbeResult> {
    use crate::scan::{ProbeResult, RESULT_PERFECT, RESULT_UNUSABLE};
    let Some(system) = probes.first().map(|p| p.solar_system_id) else {
        return vec![];
    };
    let hearing: Vec<_> = probes
        .iter()
        .filter(|p| p.solar_system_id == system)
        .cloned()
        .collect();
    let seed = w.world().seed;
    let (mut out, mut perfect) = (Vec::new(), Vec::new());
    let t = w.ext::<Tracker>();
    for s in t
        .systems
        .get(&system)
        .into_iter()
        .flat_map(|t| t.sites.values())
    {
        if kind(s) != Some(SIGNATURE) {
            continue;
        }
        // The loaded type data's probe numbers (CP58e), or the retired
        // hand rows where the world has none (X2 retires the second).
        let cert =
            crate::scan::certainty_in(w.content(), &hearing, s.position, s.signature_radius).max(
                crate::scan::certainty(&hearing, s.position, s.signature_radius),
            );
        if cert < RESULT_UNUSABLE {
            continue;
        }
        let (pos, deviation) = if cert >= RESULT_PERFECT {
            perfect.push(s.site_id);
            (s.position, 0.0)
        } else {
            let smallest = hearing
                .iter()
                .filter(|q| distance(s.position, q.position) <= q.scan_range)
                .map(|q| q.scan_range)
                .fold(f64::INFINITY, f64::min);
            let dev = smallest * (1.0 - cert.clamp(0.0, 1.0));
            (blurred(seed, s, dev * 0.5), dev)
        };
        out.push(ProbeResult {
            target_id: s.target_id.clone(),
            scan_group_id: SCAN_GROUP_SIGNATURES,
            group_id: GROUP_SIGNATURE,
            type_id: crate::dungeon_data::ENTRY_SIGNATURE,
            item_id: s.site_id,
            certainty: cert,
            pos,
            deviation,
            // What kind of site it is, from `RESULT_GOOD` (SOC-B3).
            strength_attribute: Some(s.scan_strength_attribute).filter(|&a| a > 0),
        });
    }
    if !perfect.is_empty() {
        let done = w.ext_mut::<Tracker>().resolved.entry(character_id);
        done.or_default().extend(perfect);
    }
    out
}

/// **The `scan` warp row** after the ships' one: `CmdWarpToStuff('scan',
/// targetID)` for a signature, which is how the client warps to one,
/// because `GetPositionalSiteItemIDFromTargetID` searches anomalies and
/// structures only (`menusvc._WarpXToScanResult:1697-1704`,
/// `sensorSuiteService:995-999` [V], C2 §2.3). It resolves only a signature
/// in the warper's system on which their own probes reached
/// `RESULT_PERFECT`: below it the client shows no warp
/// (`util.IsPerfectResult` [V]), and the server refuses one too.
pub fn signature_target(
    w: &Registry,
    actor: i64,
    _id: i64,
    raw: Option<&PyValue>,
) -> Option<Target> {
    let wanted = crate::pyutil::as_bytes(raw?)?;
    let t = w.ext::<Tracker>();
    let done = t.resolved.get(&actor)?;
    let s = t
        .systems
        .get(&system_of(w, actor)?)?
        .sites
        .values()
        .find(|s| kind(s) == Some(SIGNATURE) && s.target_id.as_bytes() == wanted)?;
    done.contains(&s.site_id).then_some(Target {
        position: s.position,
        radius: 0.0,
        warp_in: None,
    })
}

/// GM: mark every signature in the actor's system as resolved for them, as if
/// their probes had reached `RESULT_PERFECT`, so [`signature_target`] warps
/// them to it. The number marked; `None` when they are in no system.
pub fn resolve_all(w: &mut Registry, actor: i64) -> Option<usize> {
    let system = system_of(w, actor)?;
    let ids: Vec<i64> = w
        .ext::<Tracker>()
        .systems
        .get(&system)
        .into_iter()
        .flat_map(|t| t.sites.values())
        .filter(|s| kind(s) == Some(SIGNATURE))
        .map(|s| s.site_id)
        .collect();
    let n = ids.len();
    w.ext_mut::<Tracker>()
        .resolved
        .entry(actor)
        .or_default()
        .extend(ids);
    Some(n)
}

/// `/sigresolve`: [`resolve_all`], and the count back.
pub fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    _args: &[&str],
) -> Result<PyValue, crate::service::ServiceError> {
    let say = |t: String| Ok(crate::pyutil::bstr(&t));
    match actor.and_then(|a| resolve_all(reg, a)) {
        Some(n) => say(format!("resolved {n} signatures")),
        None => say("no character in space".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};
    use crate::pyutil::dict_get;
    use crate::space_state::{Motion, Presence};

    const PILOT: i64 = 1_000_001;
    const SYSTEM: i64 = 30_000_001;

    fn world() -> Registry {
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![Character {
                character_id: PILOT,
                ship_id: 100_000_001,
                solar_system_id: Some(SYSTEM),
                ..Character::default()
            }],
            ..SandboxConfig::default()
        });
        let p = Presence::arriving(
            PILOT,
            100_000_001,
            SYSTEM,
            Motion::arrived([0.0; 3], [0.0; 3]),
        );
        reg.roster_mut().arrive(p);
        reg
    }

    fn site(id: i64, group_id: i64) -> Site {
        Site {
            site_id: id,
            instance_id: id + 1,
            dungeon_id: 999_101,
            solar_system_id: SYSTEM,
            position: [1.0e9, 2.0e9, 3.0e9],
            group_id,
            is_scannable: true,
            scan_strength_attribute: 9_001,
            scan_strength_value: 40.0,
            signature_radius: 100.0,
            target_id: "ABC-123".to_owned(),
            allowed_types: vec![],
        }
    }

    type Sent = (&'static str, Vec<PyValue>, Option<Vec<i64>>);

    fn due(reg: &mut Registry) -> Vec<Sent> {
        let later = Instant::now() + Duration::from_secs(1);
        reg.take_due(later)
            .into_iter()
            .map(|p| {
                let (e, a) = p.to_push();
                (e, a, p.audience())
            })
            .collect()
    }

    fn state_keys(v: &PyValue) -> Vec<String> {
        let PyValue::ObjectEx { header, .. } = v else {
            panic!("0x23: {v:?}")
        };
        let PyValue::Tuple(h) = &**header else {
            panic!("header")
        };
        let PyValue::Dict(d) = &h[1] else {
            panic!("state dict")
        };
        d.iter()
            .map(|(_, k)| match k {
                PyValue::Bytes(b) => String::from_utf8(b.clone()).unwrap(),
                other => panic!("plain str keys: {other:?}"),
            })
            .collect()
    }

    /// `anomalies, signatures, staticSites, structures = fullState` [V]:
    /// arity four, every part a dict keyed by site id, the values the
    /// whitelisted classes with their `__init__` names in order.
    #[test]
    fn full_state_has_arity_four_with_dict_values() {
        let mut reg = world();
        add_site(&mut reg, site(7001, GROUP_ANOMALY));
        add_site(&mut reg, site(7002, GROUP_SIGNATURE));
        let mut sleeper = site(7003, GROUP_SIGNATURE);
        sleeper.scan_strength_value = 0.0;
        add_site(&mut reg, sleeper);
        add_site(&mut reg, site(7004, 310));
        let PyValue::Tuple(parts) = full_state(&reg, Some(PILOT)) else {
            panic!("tuple")
        };
        assert_eq!(parts.len(), 4);
        let PyValue::Dict(anoms) = &parts[0] else {
            panic!("dict")
        };
        let PyValue::Dict(sigs) = &parts[1] else {
            panic!("dict")
        };
        assert_eq!(
            anoms.len(),
            1,
            "a zero-strength signature and a beacon are dropped"
        );
        assert_eq!(sigs.len(), 1);
        assert_eq!(anoms[0].1, PyValue::Int(7001), "keyed by site ball id");
        assert_eq!(
            state_keys(&anoms[0].0),
            [
                "position",
                "targetID",
                "difficulty",
                "dungeonID",
                "archetypeID",
                "siteID",
                "instanceID",
                "dungeonNameID",
                "factionID",
                "scanStrengthAttribute",
                "allowedTypes",
                "entryObjectTypeID",
                "solarSystemID",
            ]
        );
        assert_eq!(parts[2], PyValue::Dict(vec![]));
        assert_eq!(parts[3], PyValue::Dict(vec![]));
        assert_eq!(
            full_state(&reg, None),
            full_state(&Registry::new(SandboxConfig::default()), None)
        );
    }

    /// `SignatureInfo` byte for byte, assembled by hand from the marshal
    /// opcodes and the class's `__init__` (position, targetID, difficulty,
    /// dungeonID=None, archetypeID=None, deviation) [V].
    #[test]
    fn a_signature_info_encodes_byte_for_byte() {
        let v = signature_info([1.5, 2.5, 3.5], "ABC-123", 2, 4.0);
        let mut w = eve_marshal::Writer::new();
        eve_marshal::save(&v, &mut w);
        let s = |out: &mut Vec<u8>, t: &str| {
            out.extend([0x10, t.len() as u8]);
            out.extend(t.as_bytes());
        };
        let r = |out: &mut Vec<u8>, x: f64| {
            out.push(0x0A);
            out.extend(x.to_le_bytes());
        };
        let mut e = vec![0x23, 0x2C, 0x25, 0x02, SIGNATURE_CLASS.len() as u8];
        e.extend(SIGNATURE_CLASS.as_bytes());
        e.extend([0x16, 6, 0x14, 3]);
        for x in [1.5, 2.5, 3.5] {
            r(&mut e, x);
        }
        s(&mut e, "position");
        s(&mut e, "ABC-123");
        s(&mut e, "targetID");
        e.extend([0x06, 2]);
        s(&mut e, "difficulty");
        e.push(0x01);
        s(&mut e, "dungeonID");
        e.push(0x01);
        s(&mut e, "archetypeID");
        r(&mut e, 4.0);
        s(&mut e, "deviation");
        e.extend([0x2D, 0x2D]);
        assert_eq!(w.as_slice(), &e[..]);
    }

    /// At s = 1, 11 and 30, `CalculateDeviationForSignal` gives 8, 6 and 4
    /// AU [V]; and the blur stays inside that radius and is stable.
    #[test]
    fn deviation_equals_the_clients_at_three_signals() {
        let au = crate::scan::AU;
        assert_eq!(deviation(10.0, 10.0), 8.0 * au);
        assert!((deviation(110.0, 10.0) - 6.0 * au).abs() < 1.0);
        assert_eq!(deviation(300.0, 10.0), 4.0 * au);
        let s = site(7002, GROUP_SIGNATURE);
        let p = blurred(9, &s, au);
        let d: f64 = (0..3)
            .map(|i| (p[i] - s.position[i]).powi(2))
            .sum::<f64>()
            .sqrt();
        assert!(d <= au);
        assert_eq!(p, blurred(9, &s, au));
    }

    /// A registered pilot gets the full state once, on the tick; then an add
    /// and a remove inside one tick are one `AnomalyUpdate`, with the add
    /// withdrawn (the client removes before it adds [V]), plus the two
    /// singlecasts.
    #[test]
    fn an_add_then_a_remove_gives_one_anomaly_update() {
        let mut reg = world();
        assert_eq!(register(&mut reg, Some(PILOT)), PyValue::None);
        tick(&mut reg, Instant::now(), 0);
        let first = due(&mut reg);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].0, "OnSignalTrackerFullState");
        assert_eq!(first[0].1[0], PyValue::Int(SYSTEM));
        assert_eq!(first[0].2, Some(vec![PILOT]));

        add_site(&mut reg, site(7001, GROUP_ANOMALY));
        remove_site(&mut reg, SYSTEM, 7001);
        tick(&mut reg, Instant::now(), 0);
        let pushes = due(&mut reg);
        let names: Vec<&str> = pushes.iter().map(|p| p.0).collect();
        assert_eq!(
            names,
            [
                "OnCosmicAnomalyAdded",
                "OnCosmicAnomalyRemoved",
                "OnSignalTrackerAnomalyUpdate"
            ]
        );
        let PyValue::Instance { state, .. } = &pushes[0].1[0] else {
            panic!("a KeyVal")
        };
        assert_eq!(dict_get(state, b"targetID"), Some(&bstr("ABC-123")));
        assert_eq!(
            pushes[1].1,
            [
                PyValue::Int(7002),
                PyValue::Int(999_101),
                PyValue::Int(SYSTEM)
            ]
        );
        let update = &pushes[2].1;
        assert_eq!(update[1], PyValue::Dict(vec![]));
        assert_eq!(update[2], PyValue::List(vec![PyValue::Int(7001)]));
        tick(&mut reg, Instant::now(), 0);
        assert!(due(&mut reg).is_empty(), "nothing pending, nothing sent");
    }

    /// The anomaly's id has no ball, so the S8 `item` row that resolves
    /// balls misses and this row, after it, answers.
    #[test]
    fn anomaly_target_resolves_an_id_with_no_ball() {
        fn ball(w: &Registry, _: i64, id: i64, _: Option<&PyValue>) -> Option<Target> {
            w.ball_target(id)
        }
        let mut reg = world();
        add_site(&mut reg, site(7001, GROUP_ANOMALY));
        add_site(&mut reg, site(7002, GROUP_SIGNATURE));
        assert_eq!(reg.ball_target(7001), None);
        let want = Some(Target {
            position: [1.0e9, 2.0e9, 3.0e9],
            radius: 0.0,
            warp_in: None,
        });
        assert_eq!(anomaly_target(&reg, PILOT, 7001, None), want);
        assert_eq!(
            anomaly_target(&reg, PILOT, 7002, None),
            None,
            "signatures go by scan"
        );
        let table: &[(&[u8], crate::warp_subject::Resolver)] =
            &[(b"item", ball), (b"item", anomaly_target)];
        assert_eq!(
            crate::warp_subject::resolve(table, &reg, b"item", PILOT, 7001, None),
            want
        );
    }

    // ---- CP29 ----

    use crate::celestial::{Catalogue, Universe};
    use crate::dungeon_data::{Dungeon, Entry};
    use crate::population::{self, Population, Prepared};
    use crate::scan::{PROBE_TYPES, Probe, ProbeResult, RESULT_PERFECT};
    use std::sync::Arc;

    /// The universe fixture's high-sec and null-sec systems.
    const HIGH: i64 = 39_000_001;
    const NULL: i64 = 39_000_003;
    const PIRATE: i64 = 599_029;
    const ORE_DUNGEON: i64 = 999_304;
    const ICE_DUNGEON: i64 = 999_305;
    const FW_DUNGEON: i64 = 999_306;

    /// Invented dungeons: a combat anomaly and a combat signature the
    /// population fixture's combat rows draw, and an ore and an ice anomaly
    /// no fixture row draws (they are added by hand).
    fn dungeons() -> crate::dungeon_data::Table {
        let d = |id, archetype, faction, difficulty, entry: Entry| Dungeon {
            id,
            archetype,
            faction,
            difficulty: Some(difficulty),
            entry_type: Some(entry.type_id()),
            name_id: Some(id + 1),
            ..Dungeon::default()
        };
        let list = [
            d(999_301, ARCHETYPE_COMBAT, Some(PIRATE), 1, Entry::Anomaly),
            d(999_302, ARCHETYPE_COMBAT, Some(PIRATE), 2, Entry::Anomaly),
            d(999_303, ARCHETYPE_COMBAT, Some(PIRATE), 2, Entry::Signature),
            d(ORE_DUNGEON, ARCHETYPE_ORE, None, 1, Entry::Anomaly),
            d(ICE_DUNGEON, ARCHETYPE_ICE, None, 1, Entry::Anomaly),
            d(FW_DUNGEON, 33, Some(PIRATE), 1, Entry::Anomaly),
        ];
        crate::dungeon_data::Table {
            dungeons: list.into_iter().map(|d| (d.id, d)).collect(),
            ..crate::dungeon_data::Table::default()
        }
    }

    /// A planet and a moon per fixture system, for the anchors.
    fn bodies() -> Catalogue {
        let mut text = String::new();
        for (n, s) in Universe::fixture().systems().enumerate() {
            let x = 1.0e11 * (n as f64 + 1.0);
            text += &format!(
                "planet\t{}\t990011\t6000000.0\t{x}\t0.0\t0.0\t{}\n",
                40_000_000 + n,
                s.id
            );
            text += &format!(
                "moon\t{}\t990012\t1000000.0\t{x}\t4.0e8\t0.0\t{}\n",
                40_100_000 + n,
                s.id
            );
        }
        Catalogue::parse(&text)
    }

    /// The population fixture over [`dungeons`], every region's rats
    /// [`PIRATE`], and the pilot undocked in [`HIGH`]: the system is warm
    /// and population's stage has handed its sites to the tracker.
    fn populated() -> Registry {
        let mut c = Content::fixture();
        c.insert("universe", Universe::fixture());
        c.insert("dungeons", dungeons());
        c.insert("population", population::Table::fixture());
        let mut reg = Registry::new(SandboxConfig {
            celestials: bodies(),
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(c));
        let prep = Prepared::new(
            &population::Table::fixture(),
            &Universe::fixture(),
            &dungeons(),
            &|_| Some(PIRATE),
        );
        *reg.ext_mut::<Population>() = Population::new(prep);
        reg.deltas_mut().set_world(crate::delta::World {
            gen_version: population::GEN_VERSION,
            ..crate::delta::World::FIXTURE
        });
        let p = Presence::arriving(
            PILOT,
            100_000_001,
            HIGH,
            Motion::arrived([0.0; 3], [0.0; 3]),
        );
        reg.roster_mut().arrive(p);
        let now = reg.world().birth;
        crate::liveness::refresh(&mut reg, Instant::now(), now);
        population::tick(&mut reg, Instant::now(), now);
        reg
    }

    fn live(reg: &Registry, system: i64) -> Vec<population::Site> {
        population::sites(reg, system, reg.world().birth)
            .sites
            .into_iter()
            .filter(|s| s.state == population::State::Live)
            .collect()
    }

    fn entries(v: &PyValue) -> &[(PyValue, PyValue)] {
        let PyValue::Dict(d) = v else {
            panic!("a dict: {v:?}")
        };
        d
    }

    fn of(v: &PyValue, ssid: i64) -> Option<&PyValue> {
        entries(v)
            .iter()
            .find(|(_, k)| *k == PyValue::Int(ssid))
            .map(|(v, _)| v)
    }

    /// Population's stage adds its live sites to the tracker, and a
    /// completion removes one: the registered pilot gets the full state
    /// with them, then an update whose `removed` is the completed site.
    #[test]
    fn population_add_and_remove_produce_tracker_deltas() {
        let mut reg = populated();
        let want: BTreeSet<i64> = live(&reg, HIGH).iter().map(|s| s.site_id).collect();
        assert!(!want.is_empty());
        let held: BTreeSet<i64> = reg.ext::<Tracker>().systems[&HIGH]
            .sites
            .keys()
            .copied()
            .collect();
        assert_eq!(held, want);
        register(&mut reg, Some(PILOT));
        tick(&mut reg, Instant::now(), 0);
        let full = due(&mut reg)
            .into_iter()
            .find(|p| p.0 == "OnSignalTrackerFullState")
            .expect("the full state");
        let PyValue::Tuple(parts) = &full.1[1] else {
            panic!("the 4-tuple")
        };
        let shown = entries(&parts[0]).len() + entries(&parts[1]).len();
        assert_eq!(shown, want.len());

        let gone = live(&reg, HIGH)[0].clone();
        let now = reg.world().birth;
        assert!(population::complete(&mut reg, HIGH, gone.site_id, now));
        population::tick(&mut reg, Instant::now(), now);
        tick(&mut reg, Instant::now(), 0);
        let event = match gone.entry {
            Entry::Anomaly => "OnSignalTrackerAnomalyUpdate",
            Entry::Signature => "OnSignalTrackerSignatureUpdate",
        };
        let update = due(&mut reg)
            .into_iter()
            .find(|p| p.0 == event)
            .expect("the update");
        assert_eq!(update.1[2], PyValue::List(vec![PyValue::Int(gone.site_id)]));
        assert_eq!(update.2, Some(vec![PILOT]));
    }

    /// Per archetype [V shapes]: the own system is a list of rows with the
    /// fields the Agency reads, every other system an int; the counts are
    /// ints for every system; and a cold system's count equals population's
    /// own resolution of it.
    #[test]
    fn agency_rows_for_the_own_system_and_ints_for_others() {
        let mut reg = populated();
        let mut extra = |id, dungeon_id| {
            let mut s = site(id, GROUP_ANOMALY);
            (s.solar_system_id, s.dungeon_id) = (HIGH, dungeon_id);
            add_site(&mut reg, s);
        };
        extra(7101, ORE_DUNGEON);
        extra(7102, ICE_DUNGEON);
        let combat = agency_instances(&reg, Some(PILOT), COMBAT);
        let Some(PyValue::List(rows)) = of(&combat, HIGH) else {
            panic!("rows for the own system: {combat:?}")
        };
        let anoms = live(&reg, HIGH)
            .iter()
            .filter(|s| s.entry == Entry::Anomaly && s.archetype == ARCHETYPE_COMBAT)
            .count();
        assert_eq!(rows.len(), anoms);
        for row in rows {
            let PyValue::Instance { state, .. } = row else {
                panic!("a KeyVal")
            };
            for key in [
                "siteID",
                "instanceID",
                "factionID",
                "dungeonNameID",
                "position",
                "dungeonID",
            ] {
                assert!(dict_get(state, key.as_bytes()).is_some(), "{key}");
            }
        }
        let cold = live(&reg, NULL)
            .iter()
            .filter(|s| s.entry == Entry::Anomaly && s.archetype == ARCHETYPE_COMBAT)
            .count();
        assert!(cold > 0);
        assert_eq!(of(&combat, NULL), Some(&PyValue::Int(cold as i64)));
        let counts = agency_counts(&reg, COMBAT);
        assert_eq!(of(&counts, NULL), Some(&PyValue::Int(cold as i64)));
        assert_eq!(of(&counts, HIGH), Some(&PyValue::Int(anoms as i64)));
        assert!(
            entries(&counts)
                .iter()
                .all(|(n, _)| matches!(n, PyValue::Int(_)))
        );

        let only = |w: Want| match of(&agency_instances(&reg, Some(PILOT), w), HIGH) {
            Some(PyValue::List(rows)) => rows.len(),
            other => panic!("{other:?}"),
        };
        assert_eq!((only(ORE), only(ICE)), (1, 1), "one archetype each");
        let sigs = live(&reg, HIGH)
            .iter()
            .filter(|s| s.entry == Entry::Signature)
            .count();
        let sig_counts = agency_counts(&reg, Want::Signatures);
        assert_eq!(
            of(&sig_counts, HIGH).cloned(),
            (sigs > 0).then_some(PyValue::Int(sigs as i64))
        );
        assert!(
            entries(&sig_counts)
                .iter()
                .all(|(n, _)| matches!(n, PyValue::Int(_)))
        );
    }

    /// FW-6: `GetFactionWarfareInstances` is `{ssid: [KeyVal, ...]}`, never
    /// an int value (the FW tracker iterates each value), and holds only the
    /// complexes.
    #[test]
    fn fw_instances_are_rows_of_complexes_only() {
        let mut reg = populated();
        assert!(entries(&fw_instances(&reg)).is_empty());
        let mut fw = site(7301, GROUP_ANOMALY);
        (fw.solar_system_id, fw.dungeon_id) = (NULL, FW_DUNGEON);
        add_site(&mut reg, fw);
        let mut ore = site(7302, GROUP_ANOMALY);
        (ore.solar_system_id, ore.dungeon_id) = (NULL, ORE_DUNGEON);
        add_site(&mut reg, ore);
        let v = fw_instances(&reg);
        assert_eq!(entries(&v).len(), 1);
        let Some(PyValue::List(rows)) = of(&v, NULL) else {
            panic!("rows: {v:?}")
        };
        assert_eq!(rows.len(), 1);
        let PyValue::Instance { state, .. } = &rows[0] else {
            panic!("a KeyVal")
        };
        assert_eq!(
            dict_get(state, b"dungeonID"),
            Some(&PyValue::Int(FW_DUNGEON))
        );
        assert!(dict_get(state, b"factionID").is_some());
    }

    /// The client indexes `GetDungeon` by every row's `dungeonID`, so a site
    /// whose dungeon is not in the catalogue is in no list and no count.
    #[test]
    fn every_agency_dungeon_id_exists_in_the_catalogue() {
        let mut reg = populated();
        let mut stray = site(7201, GROUP_ANOMALY);
        stray.solar_system_id = HIGH;
        add_site(&mut reg, stray);
        let cat = dungeons();
        for want in [COMBAT, ORE, ICE] {
            let v = agency_instances(&reg, Some(PILOT), want);
            for (rows, _) in entries(&v) {
                let PyValue::List(rows) = rows else { continue };
                for row in rows {
                    let PyValue::Instance { state, .. } = row else {
                        panic!("a KeyVal")
                    };
                    let Some(PyValue::Int(d)) = dict_get(state, b"dungeonID") else {
                        panic!("dungeonID")
                    };
                    assert!(cat.dungeons.contains_key(d), "{d}");
                }
            }
        }
    }

    fn probes(n: usize, at: [f64; 3], range: f64) -> Vec<Probe> {
        (0..n as i64)
            .map(|i| Probe {
                probe_id: 9_000 + i,
                type_id: PROBE_TYPES[0].type_id,
                owner_id: PILOT,
                solar_system_id: SYSTEM,
                position: at,
                scan_range: range,
                range_step: 0,
                expiry: i64::MAX,
            })
            .collect()
    }

    fn scan_warp(w: &Registry, actor: i64, _: i64, raw: Option<&PyValue>) -> Option<Target> {
        crate::scan::warp_target(w, actor, raw?)
    }

    /// `/sigresolve` makes every signature in the pilot's system warpable
    /// without probes, and an anomaly stays unresolved.
    #[test]
    fn resolve_all_makes_signatures_warpable() {
        let mut reg = world();
        let sig = site(7002, GROUP_SIGNATURE);
        add_site(&mut reg, sig.clone());
        add_site(&mut reg, site(7001, GROUP_ANOMALY));
        let raw = bstr(&sig.target_id);
        assert!(signature_target(&reg, PILOT, 0, Some(&raw)).is_none());
        assert_eq!(resolve_all(&mut reg, PILOT), Some(1));
        assert!(signature_target(&reg, PILOT, 0, Some(&raw)).is_some());
    }

    /// A signature is a result of its scan source, through the source
    /// table, and the `scan` warp row resolves it exactly from the first
    /// scan whose certainty reaches `RESULT_PERFECT`, where its result is
    /// the exact point `util.IsPerfectResult` needs [V]; never for another
    /// pilot, nor for an anomaly.
    #[test]
    fn a_signature_is_a_probe_result_and_warpable_once_perfect() {
        let mut reg = world();
        let sig = site(7002, GROUP_SIGNATURE);
        add_site(&mut reg, sig.clone());
        add_site(&mut reg, site(7001, GROUP_ANOMALY));
        let kind = &PROBE_TYPES[0];
        let table: &[(&[u8], crate::warp_subject::Resolver)] =
            &[(b"scan", scan_warp), (b"scan", signature_target)];
        let raw = bstr(&sig.target_id);
        let warp = |reg: &Registry, who| {
            crate::warp_subject::resolve(table, reg, b"scan", who, 0, Some(&raw))
        };
        let sources: &[crate::scan_sources::Source] = &[crate::scan::scan_ships, signatures];
        let mut perfect = None;
        'search: for n in 1..=crate::scan::MAX_PROBES {
            for step in (0..8).rev() {
                let range = kind.base_range_au * crate::scan::AU * kind.range_factor.powi(step);
                let ps = probes(n, sig.position, range);
                let found: Vec<ProbeResult> =
                    crate::scan_sources::run(sources, &mut reg, PILOT, &ps);
                assert!(
                    found.iter().all(|r| r.target_id == sig.target_id),
                    "signatures only"
                );
                let Some(r) = found.first() else {
                    assert_eq!(warp(&reg, PILOT), None);
                    continue;
                };
                assert_eq!(r.scan_group_id, SCAN_GROUP_SIGNATURES);
                if r.certainty < RESULT_PERFECT {
                    assert_eq!(warp(&reg, PILOT), None, "{n} probes, step {step}");
                    continue;
                }
                perfect = Some(r.clone());
                break 'search;
            }
        }
        let r = perfect.expect("eight probes on top of it reach perfect");
        let PyValue::Instance { state, .. } = r.to_keyval() else {
            panic!("a KeyVal")
        };
        assert_eq!(
            dict_get(&state, b"data"),
            Some(&vec3(sig.position)),
            "a point"
        );
        assert_eq!(
            dict_get(&state, b"itemID"),
            Some(&PyValue::Int(sig.site_id))
        );
        let at = warp(&reg, PILOT).expect("warpable once perfect");
        assert_eq!(at.position, sig.position);
        assert_eq!(warp(&reg, PILOT + 1), None, "another pilot has no result");
        remove_site(&mut reg, SYSTEM, sig.site_id);
        assert_eq!(warp(&reg, PILOT), None, "a gone site");
    }
}
