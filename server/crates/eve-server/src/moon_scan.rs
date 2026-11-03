//! **The moon survey probe and the moon scan** (R-MOON packet MN-D).
//!
//! A pilot with a Survey Probe Launcher (group 1226) fitted and online, a
//! survey probe loaded (group 492) and the skills the probe asks for, activates
//! the launcher. A probe leaves the ship; the client's Moon Analysis window
//! opens (`OnNewMoonProbe`), shows the probe's gauge filling to its `delay`,
//! and at the end of it the server tells the window what the moon it flew at
//! is made of (`OnMoonScanComplete(moonID, {oreTypeID: fraction})`). Nothing
//! else is needed on the client's side: `MoonScanSvc` and `MoonScanner` are
//! the client's own and take only these four pushes **[V]**
//! (`parklife/moonscan.py`, `ui/inflight/scannerFiles/moonScanner.py`,
//! `ui/inflight/moonscan.py`).
//!
//! # What the client does with each push **[V]**
//!
//! - `OnNewMoonProbe(probe)`: `probe` is a `KeyVal` with `probeID, typeID,
//!   pos, maxScanRange, expiry, delay`. The service numbers it, the window
//!   opens, and a row with a gauge `1 - timeLeft / (delay * MSEC)` counts
//!   down to `expiry` (a blue time, read against `GetSimTime`). `delay` is in
//!   milliseconds (the probe type's `explosionDelay`, 281).
//! - `OnMoonScanComplete(moonID, results)`: one group per moon, one row per
//!   ore, each `clamp(round(fraction, 2), 0, 1)`; the type name and colour are
//!   the client's own (`evetypes`, `GetOreColor`).
//! - `OnScanNoMaterials(moonID)`: a moon with nothing in it is stored as `{}`;
//!   a celestial that is **not** a moon raises `CustomNotify` ("survey probe
//!   hit a non-moon").
//! - `OnRemoveMoonProbe(probeID)`: the row goes. A jump clears all of them
//!   client-side (`OnSessionChanged`).
//!
//! # What the server decides
//!
//! The client has no say in the result. Which celestial a probe finds, and
//! what a moon is made of, are ours:
//!
//! - **The aim.** A survey probe flies straight, in the direction the ship was
//!   going (or going to) when it launched; the client's own list of "which
//!   moon" is a ray from the probe along its velocity within `maxScanRange`
//!   (`FindMoonForProbe` [V]), and the server runs the same ray against the
//!   system's celestials ([`aim`]): the nearest sphere on the ray within the
//!   probe's `maxScanRange` (attribute 2697 [V-data]). A ship with no heading
//!   scans the nearest celestial in range instead **[A]**.
//! - **The result.** [`crate::moon::composition`] of the moon: the *full*
//!   makeup, not only the ore of the belt that happens to be lit (a scan shows
//!   what a moon is made of, which is the point of scanning one; R-MOON §4.2).
//!   The key is the base ore (`oreBasicType`), the value a fraction at two
//!   places ([`crate::moon::Share::fraction`]).
//! - **When.** At the probe's `expiry`, `explosionDelay` after launch **[A]**:
//!   the literal reading of the probe's gauge, and of the launcher's default
//!   effect being a missile's (R-MOON U-MN-3). It is a flight, so it runs on
//!   real time, not on the slow game clock (`--time-scale` leaves physics
//!   alone); the stage's time is the world clock's.
//!
//! # What is not done
//!
//! The probe is a record, not a ball in the park, as the scan probes of
//! [`crate::scan`] are. The client therefore never runs `FindMoonForProbe`
//! and the window's "destination" column reads "unknown" until the scan is
//! complete; nothing is drawn in space. **[A]** that the client is content
//! with that; falsifier: H-MN-3, the window opening with the probe row.
//!
//! The launcher's own cycle, one round and the shared `useMissiles` effect
//! are `crate::modules`'; it calls [`check_activation`] and [`on_activated`]
//! and clamps the launcher to one cycle ([`cycles`]).

use crate::celestial::{Kind, Universe};
use crate::content::Content;
use crate::damage::BlueTime;
use crate::modules::ModuleType;
use crate::moon::Share;
use crate::moon_belt::MoonBelts;
use crate::notify::Push;
use crate::pyutil::keyval;
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::time::{Duration, Instant};

/// `groupSurveyProbeLauncher` and `groupSurveyProbe` **[V]**
/// (`inventorycommon/const`, as `moonscan.py` imports the first).
pub const GROUP_SURVEY_LAUNCHER: i64 = 1226;
pub const GROUP_SURVEY_PROBE: i64 = 492;

/// `maxScanRange` (2697) and `explosionDelay` (281) **[V-data]**: the probe's
/// reach in metres and its life in milliseconds.
pub const ATTR_MAX_SCAN_RANGE: i64 = 2697;
pub const ATTR_EXPLOSION_DELAY: i64 = 281;

/// `chargeGroup1`, `2`, `3` and `4` **[V-data]**. The survey launcher names
/// its probes in `chargeGroup2` (605 = 492) and carries no `chargeGroup1`,
/// so a "takes charges" test reading 604 alone would refuse it
/// ([`takes_charges`]; [`accepts_group`] reads all four). There the other
/// three are read **only for a `useMissiles` launcher**: 14 ancillary shield
/// boosters and 12 ancillary armor repairers carry 605 and must stay as they
/// are, usable with or without paste. (The torpedo launchers carry 606 alone,
/// so this also lets them take their torpedoes **[V-data]**.)
pub const CHARGE_GROUP_ATTRS: [i64; 4] = [604, 605, 606, 609];

/// `useMissiles` **[V-data]** (`dogmaeffects[101]`).
const EFFECT_USE_MISSILES: i64 = 101;

/// The first survey probe id, and one past the last. Clear of every other
/// range this server mints (a test lists them): scan probes 9.0e12, missiles
/// 9.5e12, command nodes 9.6e12, moon rocks 9.7e12, NPC orders 1.0e13.
pub const ID_BASE: i64 = 9_800_000_000_000;
pub const ID_END: i64 = 9_900_000_000_000;

/// **[A]** how many survey probes one pilot may have out at once. The client
/// states no limit; this one only keeps a held key from spending a hold of
/// probes in one go. Falsifier: a retail capture of a ninth probe.
pub const MAX_OUT: usize = 8;

/// **[A]** a ship with no heading (stopped, or following something) scans the
/// nearest celestial in range. Falsifier: H-MN-3, a stopped ship's probe
/// that names a moon the pilot did not mean.
pub const NEAREST_WHEN_STOPPED: bool = true;

fn is_missile_launcher(t: &ModuleType) -> bool {
    t.effect.is_some_and(|e| e.effect_id == EFFECT_USE_MISSILES)
}

/// Whether a module takes charges at all: `chargeGroup1` named, or, for a
/// `useMissiles` launcher, any of the other three. `crate::modules::
/// takes_charges` reads this.
pub fn takes_charges(t: &ModuleType) -> bool {
    let named = |a: &i64| t.attribute(*a).is_some_and(|g| g > 0.0);
    named(&CHARGE_GROUP_ATTRS[0])
        || (is_missile_launcher(t) && CHARGE_GROUP_ATTRS[1..].iter().any(named))
}

/// Whether a charge can be LOADED into this module: any of the four
/// chargeGroup attributes names a group. Wider than [`takes_charges`], which
/// keeps 604 alone off a launcher so an ancillary booster still counts as
/// running empty; loading its paste or cap boosters (named in 605: a Capital
/// Ancillary Shield Booster names group 87 there [V-data]) must still work.
/// `crate::modules::load_one` reads this.
pub fn can_load(t: &ModuleType) -> bool {
    CHARGE_GROUP_ATTRS
        .iter()
        .any(|a| t.attribute(*a).is_some_and(|g| g > 0.0))
}

/// Whether `group` is one this module's charge groups name: **any of the
/// four, for every module**. Only [`takes_charges`] limits itself to 604 off
/// a launcher (an ancillary booster runs without paste); "does this charge
/// fit" must read all four, or a turret refuses the advanced ammo it names in
/// `chargeGroup2` (Barrage L, group 372, in an 800mm Repeating Cannon: 604 =
/// 83, 605 = 372 [V-data]). `crate::modules::load_charge` reads this.
pub fn accepts_group(t: &ModuleType, group: i64) -> bool {
    CHARGE_GROUP_ATTRS
        .iter()
        .any(|a| t.attribute(*a) == Some(group as f64))
}

/// Whether `type_id` is a survey probe launcher in `content`.
pub fn is_survey_launcher_in(content: &Content, type_id: i64) -> bool {
    crate::scan::group_of(content, type_id) == Some(GROUP_SURVEY_LAUNCHER)
}

/// Whether `type_id` is a survey probe in `content`.
pub fn is_survey_probe_in(content: &Content, type_id: i64) -> bool {
    crate::scan::group_of(content, type_id) == Some(GROUP_SURVEY_PROBE)
}

/// How many cycles a launch may repeat: one for a survey launcher (one probe
/// leaves on activation, so a second cycle would spend a round and launch
/// nothing), the client's own `repeat` for every other module.
pub fn cycles(content: &Content, module_type: i64, repeat: i64) -> i64 {
    if is_survey_launcher_in(content, module_type) {
        1
    } else {
        repeat
    }
}

// ---------------------------------------------------------------------------
// The aim
// ---------------------------------------------------------------------------

/// One celestial a probe can find: its id, centre, radius and whether it is
/// a moon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    pub id: i64,
    pub centre: [f64; 3],
    pub radius: f64,
    pub moon: bool,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// **Which body a probe finds**: with a heading, the nearest sphere the ray
/// from `origin` along `heading` enters within `range` metres (the client's
/// `GetClosestIntersectionForRay` [V]); without one, the nearest body whose
/// surface is within `range` ([`NEAREST_WHEN_STOPPED`]). A ray that starts
/// inside a sphere finds it at distance zero. `None` when nothing is in reach.
pub fn aim(
    origin: [f64; 3],
    heading: Option<[f64; 3]>,
    range: f64,
    bodies: &[Body],
) -> Option<i64> {
    let mut best: Option<(f64, i64)> = None;
    let mut consider = |gap: f64, id: i64| {
        if gap <= range && best.is_none_or(|(g, i)| gap < g || (gap == g && id < i)) {
            best = Some((gap, id));
        }
    };
    for b in bodies {
        let v = sub(b.centre, origin);
        match heading {
            Some(h) => {
                let along = dot(v, h);
                let off2 = (dot(v, v) - along * along).max(0.0);
                let r2 = b.radius * b.radius;
                if off2 > r2 {
                    continue;
                }
                let entry = along - (r2 - off2).sqrt();
                // Behind the probe: the sphere is behind it altogether.
                if along + (r2 - off2).sqrt() < 0.0 {
                    continue;
                }
                consider(entry.max(0.0), b.id);
            }
            None if NEAREST_WHEN_STOPPED => {
                consider((dot(v, v).sqrt() - b.radius).max(0.0), b.id);
            }
            None => {}
        }
    }
    best.map(|(_, id)| id)
}

/// The direction a ship is **facing**: where it has been told to go (a goto
/// point, which is what the helm turns the ship toward, however fast it has
/// turned), else the way it is moving. `None` for a ship that is stopped, or
/// orbiting, following or warping with no velocity. **[A]** that a survey
/// probe leaves down the ship's facing; falsifier: H-MN-3, a probe that finds
/// a different moon than the one the ship is pointed at.
pub fn heading_of(m: &crate::space_state::Motion) -> Option<[f64; 3]> {
    if let crate::destiny::Mode::Goto(p) = m.mode
        && let Some(u) = crate::space_state::unit(sub(p, m.position))
    {
        return Some(u);
    }
    crate::space_state::unit(m.velocity)
}

/// Every star, planet and moon the world serves in `system`.
fn bodies_of(w: &Registry, system: i64) -> Vec<Body> {
    w.celestials()
        .served_in(system)
        .filter(|c| matches!(c.kind, Kind::Moon | Kind::Planet | Kind::Star))
        .map(|c| Body {
            id: c.id,
            centre: c.position,
            radius: c.radius,
            moon: c.kind == Kind::Moon,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The result
// ---------------------------------------------------------------------------

/// **A moon's makeup**: [`crate::moon::composition`] over the world's seed,
/// the moon's band and the ore table the belts are made from. Empty for a
/// moon the band allows no ore in (a wormhole's), or a world with no moon
/// ore in its type data. `None` for a body the world does not place.
pub fn makeup(w: &Registry, moon_id: i64) -> Option<Vec<Share>> {
    let system = w.celestials().system_of(moon_id)?;
    let band = w.content().try_get::<Universe>()?.band(system)?;
    let ores = w.ext::<MoonBelts>().ores();
    Some(crate::moon::composition(
        w.world().seed,
        moon_id,
        band,
        ores,
    ))
}

/// The `results` dict of `OnMoonScanComplete`: `{oreTypeID: fraction}`.
pub fn results(shares: &[Share]) -> PyValue {
    PyValue::Dict(
        shares
            .iter()
            .map(|s| (PyValue::Real(s.fraction()), PyValue::Int(s.basic)))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// The pushes
// ---------------------------------------------------------------------------

/// `OnNewMoonProbe(probe)` **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct NewMoonProbe {
    pub to: i64,
    pub probe: PyValue,
}

impl Push for NewMoonProbe {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnNewMoonProbe", vec![self.probe.clone()])
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnRemoveMoonProbe(probeID)` **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoveMoonProbe {
    pub to: i64,
    pub probe_id: i64,
}

impl Push for RemoveMoonProbe {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnRemoveMoonProbe", vec![PyValue::Int(self.probe_id)])
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnMoonScanComplete(moonID, results)` **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct MoonScanComplete {
    pub to: i64,
    pub moon_id: i64,
    pub results: PyValue,
}

impl Push for MoonScanComplete {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnMoonScanComplete",
            vec![PyValue::Int(self.moon_id), self.results.clone()],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnScanNoMaterials(moonID)` **[V]**: an empty moon, or (to the client) a
/// body that is not a moon at all.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanNoMaterials {
    pub to: i64,
    pub body_id: i64,
}

impl Push for ScanNoMaterials {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnScanNoMaterials", vec![PyValue::Int(self.body_id)])
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

// ---------------------------------------------------------------------------
// The probes in flight
// ---------------------------------------------------------------------------

/// One survey probe out.
#[derive(Debug, Clone, PartialEq)]
pub struct Survey {
    pub probe_id: i64,
    pub owner: i64,
    pub system: i64,
    pub type_id: i64,
    /// What the ray found at launch: the body to report on.
    pub found: Option<i64>,
    pub expiry: BlueTime,
}

/// Every survey probe out, and the id counter. **Not persisted**, as the scan
/// probes are not: a restart empties space, and a probe is a thing in space.
#[derive(Debug, Default)]
pub struct Probes {
    out: Vec<Survey>,
    next: i64,
}

impl Probes {
    pub fn len(&self) -> usize {
        self.out.len()
    }

    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }

    pub fn owned_by(&self, owner: i64) -> impl Iterator<Item = &Survey> {
        self.out.iter().filter(move |s| s.owner == owner)
    }

    fn mint(&mut self) -> i64 {
        let span = ID_END - ID_BASE;
        loop {
            let id = ID_BASE + self.next.rem_euclid(span);
            self.next = (self.next + 1).rem_euclid(span);
            if !self.out.iter().any(|s| s.probe_id == id) {
                return id;
            }
        }
    }
}

/// The [`Probes`] world slot (`world_ext::SLOTS`).
pub const SLOT: SlotSpec = SlotSpec {
    name: "moon_scan",
    make: |_| Box::new(Probes::default()) as Boxed,
};

// ---------------------------------------------------------------------------
// Activation
// ---------------------------------------------------------------------------

/// The `UE_TYPEID`-free refusal: our own words (`CustomNotify`).
fn refuse(text: &str) -> crate::service::ServiceError {
    crate::modules::user_error("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

/// **The checks a survey launcher's activation owes**, for a module
/// `crate::modules::activate` has already found fitted, online and loaded:
/// the pilot has the skills the loaded probe asks for, and not
/// [`MAX_OUT`] probes out. Any other module passes untouched.
pub fn check_activation(
    w: &Registry,
    character_id: i64,
    ship_id: i64,
    module_type: i64,
    flag_id: i64,
) -> Result<(), crate::service::ServiceError> {
    if !is_survey_launcher_in(w.content(), module_type) {
        return Ok(());
    }
    let Some(probe) = crate::modules::loaded(w.store(), ship_id, flag_id) else {
        return Ok(());
    };
    if !is_survey_probe_in(w.content(), probe.type_id) {
        return Err(refuse("That launcher takes survey probes."));
    }
    if let Some((skill, level)) = crate::drone_skills::missing_skill(w, character_id, probe.type_id)
    {
        let name = crate::item::find_in(w.content(), skill).map_or("a skill", |t| t.name);
        return Err(refuse(&format!(
            "Survey probes need {name} at level {level}."
        )));
    }
    if w.ext::<Probes>().owned_by(character_id).count() >= MAX_OUT {
        return Err(refuse("You already have the most survey probes out."));
    }
    Ok(())
}

/// **A survey launcher went active**: the round is spent (`crate::modules`
/// took it), and the probe leaves. Returns the probe's id, or `None` for a
/// module that is not a survey launcher, a charge that is not a survey probe
/// or a ship not in space.
pub fn on_activated(
    w: &mut Registry,
    character_id: i64,
    module_type: i64,
    charge_type: Option<i64>,
    now: BlueTime,
) -> Option<i64> {
    if !is_survey_launcher_in(w.content(), module_type) {
        return None;
    }
    let charge_type = charge_type.filter(|t| is_survey_probe_in(w.content(), *t))?;
    w.advance_character(character_id);
    let here = w.roster().get(character_id).copied()?;
    let delay_ms = crate::typedata::attribute_in(w.content(), charge_type, ATTR_EXPLOSION_DELAY)
        .filter(|d| *d > 0.0)? as i64;
    let range =
        crate::typedata::attribute_in(w.content(), charge_type, ATTR_MAX_SCAN_RANGE).unwrap_or(0.0);
    let bodies = bodies_of(w, here.solar_system_id);
    let found = aim(
        here.motion.position,
        heading_of(&here.motion),
        range,
        &bodies,
    );
    // `MSEC` is 10,000 blue ticks **[V]** (`appConst`).
    let expiry = now + delay_ms * 10_000;
    let probe_id = w.ext_mut::<Probes>().mint();
    w.ext_mut::<Probes>().out.push(Survey {
        probe_id,
        owner: character_id,
        system: here.solar_system_id,
        type_id: charge_type,
        found,
        expiry,
    });
    let pos = PyValue::Tuple(
        here.motion
            .position
            .iter()
            .map(|x| PyValue::Real(*x))
            .collect(),
    );
    let probe = keyval(vec![
        ("probeID", PyValue::Int(probe_id)),
        ("typeID", PyValue::Int(charge_type)),
        ("pos", pos),
        ("maxScanRange", PyValue::Real(range)),
        ("expiry", PyValue::Int(expiry)),
        ("delay", PyValue::Int(delay_ms)),
    ]);
    w.push(NewMoonProbe {
        to: character_id,
        probe,
    });
    Some(probe_id)
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// **The stage**: every probe whose time is up reports and goes. A pilot who
/// has left the system since is not told (the client dropped the probe at the
/// jump, `OnSessionChanged`); one who docked still is.
pub fn tick(w: &mut Registry, _: Instant, now: BlueTime) {
    if w.ext::<Probes>().is_empty() {
        return;
    }
    let (due, keep): (Vec<Survey>, Vec<Survey>) = std::mem::take(&mut w.ext_mut::<Probes>().out)
        .into_iter()
        .partition(|s| s.expiry <= now);
    // Probes launched while reporting (none today) are kept too.
    let slot = w.ext_mut::<Probes>();
    let mut keep = keep;
    keep.append(&mut slot.out);
    slot.out = keep;
    for s in due {
        finish(w, &s);
    }
}

fn finish(w: &mut Registry, s: &Survey) {
    let here = w.session_system(s.owner);
    if here != Some(s.system) {
        return;
    }
    if let Some(body) = s.found {
        let moon = w
            .celestials()
            .get(body)
            .is_some_and(|c| c.kind == Kind::Moon);
        let shares = if moon {
            makeup(w, body).unwrap_or_default()
        } else {
            Vec::new()
        };
        if shares.is_empty() {
            // A moon with no ore, or a body that is no moon: the client
            // stores `{}` for the first and raises the second.
            w.schedule(
                Duration::ZERO,
                ScanNoMaterials {
                    to: s.owner,
                    body_id: body,
                },
            );
        } else {
            w.schedule(
                Duration::ZERO,
                MoonScanComplete {
                    to: s.owner,
                    moon_id: body,
                    results: results(&shares),
                },
            );
        }
    }
    w.schedule(
        Duration::ZERO,
        RemoveMoonProbe {
            to: s.owner,
            probe_id: s.probe_id,
        },
    );
}

// ---------------------------------------------------------------------------
// The Agency's list
// ---------------------------------------------------------------------------

/// `dunArchetypeOreAnomaly` **[V]** (`appConst`): the archetype the client's
/// ore tracker keeps, `crate::signal_tracker::ARCHETYPE_ORE`.
pub const ARCHETYPE_ORE: i64 = crate::signal_tracker::ARCHETYPE_ORE;

/// The id of the ore site a belt shows as: one past the moon rock band, by
/// the belt's sequence. Nothing else mints it (a test lists the bands).
pub fn site_id(seq: u64) -> i64 {
    crate::moon_belt::MOON_END + seq as i64
}

/// `ABC-123`, stable per site: three letters and three digits off the id.
fn target_id(site: i64) -> String {
    let mut n = site.unsigned_abs();
    let mut out = String::new();
    for _ in 0..3 {
        out.push((b'A' + (n % 26) as u8) as char);
        n /= 26;
    }
    out.push('-');
    for _ in 0..3 {
        out.push((b'0' + (n % 10) as u8) as char);
        n /= 10;
    }
    out
}

/// **The dungeon a moon belt wears on the Agency**: the lowest-numbered
/// dungeon of the ore anomaly archetype in the catalogue. The Agency card
/// takes its name and its icon from the dungeon the row names
/// (`dungeonNameID`, `dungeonID`), and the client indexes its own dungeon
/// data by that id, so a made-up id would break the list; an ore site's is
/// the nearest honest one. **[A]** the name reads as an ordinary ore site.
/// Falsifier: H-MN-3, the Agency card. `None` without a catalogue.
fn ore_dungeon(w: &Registry) -> Option<i64> {
    w.content()
        .try_get::<crate::dungeon_data::Table>()?
        .dungeons
        .values()
        .find(|d| d.archetype == ARCHETYPE_ORE)
        .map(|d| d.id)
}

/// **A lit moon belt appears on the Agency's ore list** (and in the probe
/// scanner's anomaly list, which the client fills from the same tracker):
/// a site on the moon, warpable to by its id like any anomaly
/// (`crate::signal_tracker::anomaly_target`). Nothing without an ore
/// dungeon in the catalogue.
pub fn show_site(w: &mut Registry, system: i64, seq: u64, centre: [f64; 3]) {
    let Some(dungeon_id) = ore_dungeon(w) else {
        return;
    };
    let site_id = site_id(seq);
    crate::signal_tracker::add_site(
        w,
        crate::signal_tracker::Site {
            site_id,
            instance_id: site_id,
            dungeon_id,
            solar_system_id: system,
            position: centre,
            group_id: crate::signal_tracker::GROUP_ANOMALY,
            is_scannable: true,
            scan_strength_attribute: crate::population::scan_attribute(ARCHETYPE_ORE),
            scan_strength_value: 0.0,
            signature_radius: 0.0,
            target_id: target_id(site_id),
            allowed_types: Vec::new(),
        },
    );
}

/// The belt is gone; so is its site.
pub fn hide_site(w: &mut Registry, system: i64, seq: u64) {
    crate::signal_tracker::remove_site(w, system, site_id(seq));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::celestial::{Catalogue, Universe};
    use crate::fitting::Rack;
    use crate::model::{Character, SandboxConfig};
    use crate::modules::ModuleType;
    use crate::moon::tests as moon_fixture;
    use crate::space_state::{Motion, Presence};
    use std::sync::Arc;

    const ALICE: i64 = 90_000_001;
    const ALICE_SHIP: i64 = 100_000_001;
    /// Invented systems: a null-sec one, a wormhole.
    const NULL: i64 = 39_000_003;
    const WORMHOLE: i64 = 31_000_001;
    /// Invented moons (and a planet) of those systems.
    const MOON_A: i64 = 40_300_001;
    const MOON_B: i64 = 40_300_002;
    const PLANET: i64 = 40_300_010;
    const WH_MOON: i64 = 40_300_020;
    /// Invented rows: a survey launcher, a probe and the skill it asks for.
    const LAUNCHER: i64 = 990_611_001;
    const PROBE: i64 = 990_611_010;
    const SKILL: i64 = 990_611_100;
    const FRIGATE: i64 = 990_000_001;
    /// The probe's invented life, in milliseconds.
    const DELAY_MS: i64 = 150_000;
    const REACH: f64 = 1.0e8;
    const MOON_RADIUS: f64 = 1_500_000.0;
    const LAUNCHER_ITEM: i64 = 100_000_501;
    const LOADED: i64 = 100_000_502;
    const HIGH0: i64 = 27;

    fn rows() -> String {
        format!(
            "{}\n{}\n\
             type\t{LAUNCHER}\t{GROUP_SURVEY_LAUNCHER}\t7\t1\t1\t5.0\t10.0\t0.0\t1.0\t0\t\t605={GROUP_SURVEY_PROBE}.0;56=1.0\tFixture Survey Launcher\n\
             type\t{PROBE}\t{GROUP_SURVEY_PROBE}\t8\t1\t1\t5.0\t0.0\t1.0\t30.0\t0\t\t182={SKILL}.0;277=3.0;37=30000.0;281={DELAY_MS}.0;{ATTR_MAX_SCAN_RANGE}={REACH}\tFixture Survey Probe\n",
            crate::typedata::FIXTURE,
            moon_fixture::fixture_text()
        )
    }

    fn universe() -> Universe {
        let row = |id: i64, sec: f64| {
            format!(
                "{id}\t19000001\t29000001\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture {id}"
            )
        };
        Universe::parse(
            &[row(NULL, -0.5), row(WORMHOLE, -0.99)],
            &["19000001\t\t\t\t\tFixture region".to_owned()],
            &["29000001\t19000001\t\t\t\tFixture constellation".to_owned()],
        )
        .expect("the invented rows parse")
    }

    fn content() -> Content {
        let mut c = Content::fixture();
        c.insert("universe", universe());
        let table = crate::typedata::Table::parse(&rows()).expect("the rows parse");
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    /// Moons at +x (`MOON_A` 4e7 m out, `MOON_B` 9e7 m out), a planet at
    /// +y and a wormhole moon, all in invented systems.
    fn world() -> Registry {
        let line = |kind: &str, id: i64, r: f64, at: [f64; 3], system: i64| {
            format!(
                "{kind}\t{id}\t990021\t{r}\t{}\t{}\t{}\t{system}\n",
                at[0], at[1], at[2]
            )
        };
        let text = [
            line("moon", MOON_A, MOON_RADIUS, [4.0e7, 0.0, 0.0], NULL),
            line("moon", MOON_B, MOON_RADIUS, [9.0e7, 0.0, 0.0], NULL),
            line("planet", PLANET, 6.0e6, [0.0, 3.0e7, 0.0], NULL),
            line("moon", WH_MOON, MOON_RADIUS, [4.0e7, 0.0, 0.0], WORMHOLE),
        ]
        .concat();
        let alice = Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: ALICE_SHIP,
            ship_type_id: FRIGATE,
            solar_system_id: Some(NULL),
            ..Character::default()
        };
        Registry::new(SandboxConfig {
            characters: vec![alice],
            celestials: Catalogue::parse(&text),
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(content()))
    }

    fn arrive(reg: &mut Registry, system: i64, velocity: [f64; 3]) {
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            system,
            Motion::arrived([0.0; 3], velocity),
        ));
    }

    fn drain(reg: &mut Registry) -> Vec<(&'static str, Vec<PyValue>)> {
        let _ = reg.take_pending();
        reg.take_due(Instant::now() + Duration::from_secs(600))
            .into_iter()
            .map(|p| p.to_push())
            .collect()
    }

    fn pending(reg: &mut Registry) -> Vec<(&'static str, Vec<PyValue>)> {
        reg.take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push())
            .collect()
    }

    fn train(reg: &mut Registry, skills: &[(i64, i64)]) {
        let entries = skills
            .iter()
            .map(|&(type_id, level)| crate::skill::SkillEntry {
                type_id,
                level,
                points: 0,
                rank: 1,
            })
            .collect();
        *reg.skills_mut(ALICE).expect("Alice has skills") = crate::skill::Skills::restore(entries);
    }

    fn load(reg: &mut Registry) {
        let row = |item_id: i64, type_id: i64, flag_id: i64, quantity: i64, category: i64| {
            let g = if category == 7 {
                GROUP_SURVEY_LAUNCHER
            } else {
                GROUP_SURVEY_PROBE
            };
            crate::item::Item {
                item_id,
                type_id,
                owner_id: ALICE,
                location_id: ALICE_SHIP,
                flag_id,
                quantity,
                group_id: g,
                category_id: category,
                singleton: category == 7,
            }
        };
        let mut items = reg.store().items().to_vec();
        items.push(row(LAUNCHER_ITEM, LAUNCHER, HIGH0, 1, 7));
        items.push(row(LOADED, PROBE, HIGH0, 2, 8));
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_000_600,
            ..crate::store::Snapshot::default()
        });
    }

    fn body(id: i64, x: f64, y: f64, r: f64, moon: bool) -> Body {
        Body {
            id,
            centre: [x, y, 0.0],
            radius: r,
            moon,
        }
    }

    const EAST: [f64; 3] = [1.0, 0.0, 0.0];

    // ---- the aim ----

    #[test]
    fn a_ray_finds_the_nearest_sphere_it_enters_within_range() {
        let near = body(1, 4.0e7, 0.0, MOON_RADIUS, true);
        let far = body(2, 9.0e7, 0.0, MOON_RADIUS, true);
        let o = [0.0; 3];
        assert_eq!(aim(o, Some(EAST), REACH, &[far, near]), Some(1));
        // Range 5e7 reaches only the near one; 3e7 reaches neither.
        assert_eq!(aim(o, Some(EAST), 5.0e7, &[far, near]), Some(1));
        assert_eq!(aim(o, Some(EAST), 3.0e7, &[far, near]), None);
        // The far one alone is found at its surface, inside the range.
        assert_eq!(aim(o, Some(EAST), REACH, &[far]), Some(2));
    }

    #[test]
    fn a_ray_misses_a_sphere_beside_it_and_behind_it() {
        let o = [0.0; 3];
        let aside = body(1, 4.0e7, 2.0 * MOON_RADIUS, MOON_RADIUS, true);
        let behind = body(2, -4.0e7, 0.0, MOON_RADIUS, true);
        assert_eq!(aim(o, Some(EAST), REACH, &[aside, behind]), None);
        // Grazing: inside the radius by a metre is a hit.
        let graze = body(3, 4.0e7, MOON_RADIUS - 1.0, MOON_RADIUS, true);
        assert_eq!(aim(o, Some(EAST), REACH, &[graze]), Some(3));
    }

    #[test]
    fn a_ray_that_starts_inside_a_sphere_finds_it_at_once() {
        let inside = body(1, 1.0e5, 0.0, MOON_RADIUS, true);
        assert_eq!(aim([0.0; 3], Some(EAST), 1.0, &[inside]), Some(1));
    }

    #[test]
    fn with_no_heading_the_nearest_surface_in_range_is_found() {
        let a = body(1, 4.0e7, 0.0, MOON_RADIUS, true);
        let b = body(2, 0.0, 3.0e7, 6.0e6, false);
        let o = [0.0; 3];
        assert_eq!(aim(o, None, REACH, &[a, b]), Some(2));
        assert_eq!(aim(o, None, 1.0e6, &[a, b]), None);
        assert_eq!(aim(o, None, REACH, &[]), None);
    }

    #[test]
    fn heading_is_the_goto_point_else_the_velocity() {
        let moving = Motion::arrived([1.0, 2.0, 3.0], [0.0, 5.0, 0.0]);
        assert_eq!(heading_of(&moving), Some([0.0, 1.0, 0.0]));
        let mut goto = Motion::arrived([1.0, 0.0, 0.0], [0.0; 3]);
        goto.mode = crate::destiny::Mode::Goto([1.0, 0.0, 7.0]);
        assert_eq!(heading_of(&goto), Some([0.0, 0.0, 1.0]));
        // Turning: the ship still drifts the old way, and faces the new.
        goto.velocity = [-9.0, 0.0, 0.0];
        assert_eq!(heading_of(&goto), Some([0.0, 0.0, 1.0]));
        assert_eq!(heading_of(&Motion::arrived([0.0; 3], [0.0; 3])), None);
    }

    // ---- the module side ----

    #[test]
    fn a_survey_launcher_takes_its_probes_from_chargegroup2() {
        let effect = crate::modules::Effect {
            effect_id: EFFECT_USE_MISSILES,
            name: "useMissiles",
            category: 1,
            guid: "",
            is_offensive: false,
            duration_attribute: 51,
            warp_safe: None,
        };
        let launcher = ModuleType {
            type_id: LAUNCHER,
            name: "x",
            effect: Some(effect),
            attributes: &[(605, GROUP_SURVEY_PROBE as f64)],
            rack: Rack::High,
            hardpoint: None,
        };
        assert!(takes_charges(&launcher));
        assert!(accepts_group(&launcher, GROUP_SURVEY_PROBE));
        assert!(!accepts_group(&launcher, GROUP_SURVEY_PROBE + 1));
        let gun = ModuleType {
            attributes: &[(604, 83.0)],
            effect: None,
            ..launcher
        };
        assert!(takes_charges(&gun) && accepts_group(&gun, 83));
        // A turret's advanced ammo is in its chargeGroup2.
        let turret = ModuleType {
            attributes: &[(604, 83.0), (605, 372.0)],
            effect: None,
            ..launcher
        };
        assert!(accepts_group(&turret, 83) && accepts_group(&turret, 372));
        assert!(!accepts_group(&turret, 373));
        // A module that is no launcher keeps reading chargeGroup1 alone for
        // "takes charges": an ancillary booster names 605 and still runs with
        // no charge. Its paste still fits.
        let booster = ModuleType {
            attributes: &[(605, 83.0)],
            effect: None,
            ..launcher
        };
        assert!(!takes_charges(&booster) && accepts_group(&booster, 83));
        // ...and it can be loaded: the load path must not ask takes_charges.
        assert!(can_load(&booster));
        let bare = ModuleType {
            attributes: &[],
            ..launcher
        };
        assert!(!takes_charges(&bare) && !accepts_group(&bare, 83));
        assert!(!can_load(&bare));
    }

    #[test]
    fn a_survey_launcher_cycles_once_and_nothing_else_is_touched() {
        let c = content();
        assert_eq!(cycles(&c, LAUNCHER, 1000), 1);
        assert_eq!(cycles(&c, FRIGATE, 1000), 1000);
        assert!(is_survey_launcher_in(&c, LAUNCHER));
        assert!(is_survey_probe_in(&c, PROBE));
        assert!(!is_survey_probe_in(&c, LAUNCHER));
    }

    #[test]
    fn the_ids_are_in_a_band_nothing_else_uses() {
        const {
            assert!(ID_BASE >= crate::moon_belt::MOON_END + crate::moon_belt::SEQS as i64);
            assert!(ID_END <= crate::econ_exchange::NPC_ORDER_BASE);
            assert!(ID_BASE > crate::missile::ID_END);
            assert!(ID_BASE > crate::scan::PROBE_ID_BASE);
        }
        let mut p = Probes::default();
        let a = p.mint();
        assert_eq!(a, ID_BASE);
        assert_eq!(p.mint(), ID_BASE + 1);
        // The ore site's id sits between the moon rocks and the probes.
        assert!(site_id(0) >= crate::moon_belt::MOON_END && site_id(0) < ID_BASE);
        assert!(!crate::moon_belt::is_moon_id(site_id(0)));
    }

    #[test]
    fn a_launch_without_the_skill_is_refused() {
        let mut reg = world();
        arrive(&mut reg, NULL, EAST);
        load(&mut reg);
        let go = |reg: &Registry| check_activation(reg, ALICE, ALICE_SHIP, LAUNCHER, HIGH0);
        assert!(go(&reg).is_err(), "no Astrometrics");
        train(&mut reg, &[(SKILL, 2)]);
        assert!(go(&reg).is_err(), "level 2 of 3");
        train(&mut reg, &[(SKILL, 3)]);
        assert!(go(&reg).is_ok());
        // Another module is none of its business.
        assert!(check_activation(&reg, ALICE, ALICE_SHIP, FRIGATE, HIGH0).is_ok());
    }

    #[test]
    fn a_launcher_with_a_wrong_charge_or_none_is_left_to_the_modules() {
        let mut reg = world();
        arrive(&mut reg, NULL, EAST);
        // Nothing loaded: `NoCharges` is the modules' to raise, not ours.
        assert!(check_activation(&reg, ALICE, ALICE_SHIP, LAUNCHER, HIGH0).is_ok());
    }

    #[test]
    fn the_ninth_probe_is_refused() {
        let mut reg = world();
        arrive(&mut reg, NULL, EAST);
        load(&mut reg);
        train(&mut reg, &[(SKILL, 3)]);
        for _ in 0..MAX_OUT {
            assert!(
                on_activated(&mut reg, ALICE, LAUNCHER, Some(PROBE), 0).is_some(),
                "probe out"
            );
        }
        assert_eq!(reg.ext::<Probes>().owned_by(ALICE).count(), MAX_OUT);
        assert!(check_activation(&reg, ALICE, ALICE_SHIP, LAUNCHER, HIGH0).is_err());
    }

    // ---- the scan ----

    fn probe_row(pushes: &[(&str, Vec<PyValue>)]) -> PyValue {
        let (_, args) = pushes
            .iter()
            .find(|(n, _)| *n == "OnNewMoonProbe")
            .expect("the probe is announced");
        args[0].clone()
    }

    fn field(kv: &PyValue, name: &str) -> Option<PyValue> {
        let PyValue::Instance { state, .. } = kv else {
            return None;
        };
        crate::pyutil::dict_get(state, name.as_bytes()).cloned()
    }

    #[test]
    fn a_probe_is_announced_with_the_fields_the_client_reads() {
        let mut reg = world();
        arrive(&mut reg, NULL, EAST);
        let now = 123_456;
        let id = on_activated(&mut reg, ALICE, LAUNCHER, Some(PROBE), now).expect("away");
        let row = probe_row(&pending(&mut reg));
        assert_eq!(field(&row, "probeID"), Some(PyValue::Int(id)));
        assert_eq!(field(&row, "typeID"), Some(PyValue::Int(PROBE)));
        assert_eq!(
            field(&row, "expiry"),
            Some(PyValue::Int(now + DELAY_MS * 10_000))
        );
        assert_eq!(field(&row, "delay"), Some(PyValue::Int(DELAY_MS)));
        assert_eq!(field(&row, "maxScanRange"), Some(PyValue::Real(REACH)));
        assert!(matches!(field(&row, "pos"), Some(PyValue::Tuple(p)) if p.len() == 3));
        assert!((ID_BASE..ID_END).contains(&id));
    }

    #[test]
    fn a_probe_reports_the_moons_whole_makeup_when_its_time_is_up() {
        let mut reg = world();
        arrive(&mut reg, NULL, EAST);
        let now = 1_000;
        on_activated(&mut reg, ALICE, LAUNCHER, Some(PROBE), now).expect("away");
        let probe_id = ID_BASE;
        let _ = pending(&mut reg);
        let expiry = now + DELAY_MS * 10_000;
        tick(&mut reg, Instant::now(), expiry - 1);
        assert!(drain(&mut reg).is_empty(), "not before its time");
        tick(&mut reg, Instant::now(), expiry);
        let sent = drain(&mut reg);
        let names: Vec<_> = sent.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["OnMoonScanComplete", "OnRemoveMoonProbe"]);
        // The near moon, with its whole composition from the world's seed.
        let want = crate::moon::composition(
            reg.world().seed,
            MOON_A,
            crate::celestial::Band::Null,
            &moon_fixture::table(),
        );
        assert!(!want.is_empty());
        assert_eq!(sent[0].1[0], PyValue::Int(MOON_A));
        assert_eq!(sent[0].1[1], results(&want));
        let PyValue::Dict(rows) = &sent[0].1[1] else {
            panic!("a dict");
        };
        let total: f64 = rows
            .iter()
            .map(|(v, _)| if let PyValue::Real(f) = v { *f } else { 0.0 })
            .sum();
        assert!((total - 1.0).abs() < 1e-9, "fractions sum to one: {total}");
        assert_eq!(sent[1].1, vec![PyValue::Int(probe_id)]);
        assert!(reg.ext::<Probes>().is_empty(), "the probe is gone");
        tick(&mut reg, Instant::now(), expiry + 1);
        assert!(drain(&mut reg).is_empty(), "nothing twice");
    }

    #[test]
    fn the_result_does_not_depend_on_which_belt_is_lit() {
        let reg = world();
        let a = makeup(&reg, MOON_A).expect("placed");
        assert_eq!(a, makeup(&reg, MOON_A).expect("placed"));
        assert_ne!(makeup(&reg, MOON_B), None);
        assert!(makeup(&reg, 1).is_none(), "a body the world does not place");
    }

    #[test]
    fn a_planet_in_the_way_is_a_non_moon() {
        let mut reg = world();
        arrive(&mut reg, NULL, [0.0, 1.0, 0.0]);
        on_activated(&mut reg, ALICE, LAUNCHER, Some(PROBE), 0).expect("away");
        let _ = pending(&mut reg);
        tick(&mut reg, Instant::now(), DELAY_MS * 10_000);
        let sent = drain(&mut reg);
        assert_eq!(sent[0], ("OnScanNoMaterials", vec![PyValue::Int(PLANET)]));
        assert_eq!(sent.len(), 2);
    }

    #[test]
    fn a_wormhole_moon_holds_nothing() {
        let mut reg = world();
        arrive(&mut reg, WORMHOLE, EAST);
        reg.character_mut(ALICE).expect("alice").solar_system_id = Some(WORMHOLE);
        on_activated(&mut reg, ALICE, LAUNCHER, Some(PROBE), 0).expect("away");
        let _ = pending(&mut reg);
        tick(&mut reg, Instant::now(), DELAY_MS * 10_000);
        let sent = drain(&mut reg);
        assert_eq!(sent[0], ("OnScanNoMaterials", vec![PyValue::Int(WH_MOON)]));
    }

    #[test]
    fn a_probe_that_finds_nothing_is_only_removed() {
        let mut reg = world();
        arrive(&mut reg, NULL, [-1.0, 0.0, 0.0]);
        on_activated(&mut reg, ALICE, LAUNCHER, Some(PROBE), 0).expect("away");
        let _ = pending(&mut reg);
        tick(&mut reg, Instant::now(), DELAY_MS * 10_000);
        let sent = drain(&mut reg);
        let names: Vec<_> = sent.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["OnRemoveMoonProbe"]);
    }

    #[test]
    fn a_pilot_who_jumped_hears_nothing() {
        let mut reg = world();
        arrive(&mut reg, NULL, EAST);
        on_activated(&mut reg, ALICE, LAUNCHER, Some(PROBE), 0).expect("away");
        let _ = pending(&mut reg);
        arrive(&mut reg, WORMHOLE, EAST);
        tick(&mut reg, Instant::now(), DELAY_MS * 10_000);
        assert!(drain(&mut reg).is_empty());
        assert!(reg.ext::<Probes>().is_empty());
    }

    #[test]
    fn a_charge_that_is_no_survey_probe_launches_nothing() {
        let mut reg = world();
        arrive(&mut reg, NULL, EAST);
        assert!(on_activated(&mut reg, ALICE, LAUNCHER, Some(FRIGATE), 0).is_none());
        assert!(on_activated(&mut reg, ALICE, LAUNCHER, None, 0).is_none());
        assert!(on_activated(&mut reg, ALICE, FRIGATE, Some(PROBE), 0).is_none());
        assert!(reg.ext::<Probes>().is_empty());
    }

    // ---- the Agency ----

    #[test]
    fn a_lit_belt_is_an_ore_site_and_goes_with_the_belt() {
        use crate::dungeon_data::{Dungeon, ENTRY_ANOMALY, Table};
        let mut dungeons = Table::fixture();
        dungeons.dungeons.insert(
            990_611_900,
            Dungeon {
                id: 990_611_900,
                archetype: ARCHETYPE_ORE,
                entry_type: Some(ENTRY_ANOMALY),
                name_id: Some(1),
                ..Dungeon::default()
            },
        );
        let mut c = content();
        c.insert("dungeon_data", dungeons);
        let mut reg = world().with_content(Arc::new(c));
        arrive(&mut reg, NULL, EAST);
        // Lay a belt out: every day until a window lights a moon.
        let mut lit = false;
        for day in 0..40u32 {
            let w = crate::delta::World {
                day,
                ..*reg.world()
            };
            reg.deltas_mut().set_world(w);
            let now = Instant::now();
            crate::liveness::refresh(&mut reg, now, 0);
            crate::moon_belt::tick(&mut reg, now, 0);
            if reg.ext::<MoonBelts>().iter().next().is_some() {
                lit = true;
                break;
            }
        }
        assert!(lit, "some day lights a moon");
        let belt = reg.ext::<MoonBelts>().iter().next().cloned().expect("lit");
        let rows = |reg: &Registry| match crate::signal_tracker::agency_instances(
            reg,
            Some(ALICE),
            crate::signal_tracker::ORE,
        ) {
            PyValue::Dict(d) => d
                .into_iter()
                .find(|(_, k)| *k == PyValue::Int(NULL))
                .map(|(v, _)| v),
            _ => None,
        };
        let Some(PyValue::List(list)) = rows(&reg) else {
            panic!("the Agency lists the lit belt's site");
        };
        assert_eq!(list.len(), 1);
        assert_eq!(
            field(&list[0], "siteID"),
            Some(PyValue::Int(site_id(belt.seq)))
        );
        assert_eq!(
            field(&list[0], "dungeonID"),
            Some(PyValue::Int(990_611_900))
        );
        // The site warps like any anomaly, to the moon.
        let t = crate::signal_tracker::anomaly_target(&reg, ALICE, site_id(belt.seq), None)
            .expect("warpable");
        let moon = reg.celestials().get(belt.moon_id).expect("the moon");
        assert_eq!(t.position, moon.position);
        // The belt goes and so does the site.
        assert!(crate::moon_belt::vacate(&mut reg, belt.moon_id, true));
        assert!(rows(&reg).is_none());
    }

    #[test]
    fn without_an_ore_dungeon_in_the_catalogue_there_is_no_site() {
        let mut reg = world();
        show_site(&mut reg, NULL, 0, [0.0; 3]);
        assert!(crate::signal_tracker::anomaly_target(&reg, ALICE, site_id(0), None).is_none());
    }

    #[test]
    fn target_ids_are_stable_and_shaped_like_the_clients() {
        let t = target_id(site_id(7));
        assert_eq!(t, target_id(site_id(7)));
        assert_ne!(t, target_id(site_id(8)));
        let b = t.as_bytes();
        assert_eq!(b.len(), 7);
        assert!(b[..3].iter().all(u8::is_ascii_uppercase));
        assert_eq!(b[3], b'-');
        assert!(b[4..].iter().all(u8::is_ascii_digit));
    }
}
