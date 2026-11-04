//! **Real fits seed the doctrines** (killmail fits, items 4 and 5): the fits
//! players fly ([`crate::real_fits`]) built into [`RoleFit`]s on the paper
//! evaluation, so [`crate::doctrine_fit::catalog_in`] weighs them beside the
//! greedy fits and a bot can fly one for variety.
//!
//! **Nothing from nowhere**: a row is built only when the library's
//! `Makeable` has its hull, every module and every charge
//! ([`RealFits::usable`]); it is kept only when the engine passes it
//! ([`Profile::ok`](crate::doctrine::Profile::ok)).

use crate::doctrine::{DOCTRINE_LEVEL, Family, Layer, RoleFit, layer_of};
use crate::doctrine_fit::{
    DRONES_ACTIVE, HullRow, Kind, Library, Part, ROUNDS_ALTERNATE, ROUNDS_PRIMARY, choose_rounds,
    profile_with_drones, skills_for,
};
use crate::hull_choice::Class;
use crate::real_fits::{RealFit, RealFits};
use std::collections::BTreeMap;

/// Real fits taken per DPS hull **[A]**; falsified by a soak where seeded
/// doctrines are never picked (raise it, or the fits are too weak).
pub const SEED_FITS_PER_HULL: usize = 5;

/// One built real fit.
#[derive(Debug, Clone)]
pub struct Seed {
    pub class: Class,
    pub fit: RoleFit,
    pub count: u32,
    pub hash: String,
}

type Group = (RealFits, BTreeMap<String, Seed>);

/// The built fits, by `(hull, family, layer)`, with the table of each
/// group kept so [`RealFits::pick`] draws among them by count.
#[derive(Debug, Clone, Default)]
pub struct RealSeeds {
    groups: BTreeMap<(i64, Family, Layer), Group>,
}

impl RealSeeds {
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// Every seed with its family and layer.
    pub fn iter(&self) -> impl Iterator<Item = (Family, Layer, &Seed)> + '_ {
        self.groups
            .iter()
            .flat_map(|((_, f, l), (_, m))| m.values().map(move |s| (*f, *l, s)))
    }

    /// One fit of `hull` for a doctrine of `family` and `layer`, drawn by
    /// count with `roll` in `[0, 1)` ([`RealFits::pick`]).
    pub fn pick(&self, hull: i64, family: Family, layer: Layer, roll: f64) -> Option<&RoleFit> {
        let (fits, built) = self.groups.get(&(hull, family, layer))?;
        let f = fits.pick(hull, &|_| true, roll)?;
        built.get(&f.hash).map(|s| &s.fit)
    }
}

/// Why a real fit is not built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dropped {
    /// No gun, launcher or flyable combat drone: logistics, tackle, a bare
    /// hull (no family).
    NoWeapon,
    /// A weapon with no round in the row and none the greedy choice finds.
    NoRound,
    /// The engine does not hold the fit.
    NoProfile,
    /// The fit fails CPU, power or capacitor.
    Fails,
}

impl Dropped {
    pub fn name(self) -> &'static str {
        match self {
            Dropped::NoWeapon => "no weapon or combat drone",
            Dropped::NoRound => "no round for the weapon",
            Dropped::NoProfile => "the engine does not hold it",
            Dropped::Fails => "fails cpu, power or capacitor",
        }
    }
}

/// **The drones a real fit flies**: of the carried drones the library knows
/// as combat drones, the most damaging first, as many as the hull's
/// bandwidth and [`DRONES_ACTIVE`] allow, grouped by type. Empty for a hull
/// with no bay or bandwidth, or no combat drone among them.
fn flight(lib: &Library<'_>, hull: &HullRow, carried: &[(i64, u8)]) -> Vec<(i64, u8)> {
    if hull.drone_bay <= 0.0 || hull.bandwidth <= 0.0 {
        return Vec::new();
    }
    let mut units: Vec<&Part> = carried
        .iter()
        .filter_map(|(t, n)| {
            let p = lib
                .parts
                .iter()
                .find(|p| p.type_id == *t && p.kind == Kind::Drone)?;
            Some(std::iter::repeat_n(p, usize::from(*n)))
        })
        .flatten()
        .collect();
    units.sort_by(|a, b| {
        b.magnitude
            .total_cmp(&a.magnitude)
            .then(a.type_id.cmp(&b.type_id))
    });
    let (mut bandwidth, mut volume) = (0.0, 0.0);
    let mut out: BTreeMap<i64, u8> = BTreeMap::new();
    for p in units.into_iter().take(DRONES_ACTIVE) {
        if bandwidth + p.bandwidth > hull.bandwidth || volume + p.volume > hull.drone_bay {
            continue;
        }
        bandwidth += p.bandwidth;
        volume += p.volume;
        *out.entry(p.type_id).or_insert(0) += 1;
    }
    out.into_iter().collect()
}

/// **One real fit on the paper**: modules from the row, the row's charge in
/// the guns (the greedy choice when it names none) and the best alternate,
/// the drones it flies, the doctrine's skills, the engine's numbers. A gun
/// or launcher gives its family (rockets and the T2
/// launchers included, see `Library::kind_of`); a fit with only drones is
/// [`Family::Drone`] and is judged on the drone profile ([`profile_with_drones`]). `Err` names why
/// not: no weapon, no round, the engine does not hold it, or it fails CPU,
/// power or capacitor.
pub fn try_build(lib: &Library<'_>, real: &RealFit) -> Result<(Family, RoleFit), Dropped> {
    let is_gun = |w: i64| matches!(lib.kind_of(w), Some(Kind::Gun(..)));
    let gun = real.modules.iter().map(|m| m.0).find(|t| is_gun(*t));
    // The hull row gives the bay and the bandwidth; a hull the library does
    // not list flies no drones (a gun fit of it still builds).
    let drones = lib
        .hulls
        .iter()
        .find(|h| h.type_id == real.hull)
        .map_or_else(Vec::new, |h| flight(lib, h, &real.drones));
    let family = match gun.and_then(|g| lib.kind_of(g)) {
        Some(Kind::Gun(family, _)) => family,
        _ if !drones.is_empty() => Family::Drone,
        _ => return Err(Dropped::NoWeapon),
    };
    let (primary, alt) = match gun {
        Some(g) => {
            let (greedy, alt) = choose_rounds(lib, g).map_or((None, None), |(p, a)| (Some(p), a));
            let primary = real.charge_for(is_gun).or(greedy).ok_or(Dropped::NoRound)?;
            (
                Some(primary),
                alt.filter(|a| *a != primary && lib.makeable.has(*a)),
            )
        }
        None => (None, None),
    };
    let types: Vec<i64> = std::iter::once(real.hull)
        .chain(real.modules.iter().map(|m| m.0))
        .chain(primary)
        .chain(alt)
        .chain(drones.iter().map(|d| d.0))
        .collect();
    let skills = skills_for(lib.inst.table(), &types, DOCTRINE_LEVEL);
    let profile = profile_with_drones(lib, real.hull, &real.modules, primary, &skills, &drones)
        .ok_or(Dropped::NoProfile)?;
    if !profile.ok() {
        return Err(Dropped::Fails);
    }
    let guns: u32 = real
        .modules
        .iter()
        .filter(|m| is_gun(m.0))
        .map(|m| u32::from(m.1))
        .sum();
    let mut charges = Vec::new();
    if let Some(p) = primary {
        charges.push((p, ROUNDS_PRIMARY * guns.max(1)));
    }
    if let Some(a) = alt {
        charges.push((a, ROUNDS_ALTERNATE * guns.max(1)));
    }
    for &(d, n) in &drones {
        charges.push((d, u32::from(n)));
    }
    let cost = lib.price(real.hull)
        + real
            .modules
            .iter()
            .map(|m| lib.price(m.0) * f64::from(m.1))
            .sum::<f64>()
        + charges
            .iter()
            .map(|c| lib.price(c.0) * f64::from(c.1))
            .sum::<f64>();
    Ok((
        family,
        RoleFit {
            hull: real.hull,
            modules: real.modules.clone(),
            charges,
            skills,
            profile,
            cost,
        },
    ))
}

/// [`try_build`] without the reason.
pub fn build(lib: &Library<'_>, real: &RealFit) -> Option<(Family, RoleFit)> {
    try_build(lib, real).ok()
}

/// **The seeds of a library**: for each DPS hull up to
/// [`SEED_FITS_PER_HULL`] usable real fits ([`RealFits::usable`] over the
/// library's `Makeable`), built and kept when they pass. Empty without a
/// table in `lib.real_fits`.
pub fn seeds(lib: &Library<'_>) -> RealSeeds {
    let mut out = RealSeeds::default();
    let Some(table) = lib.real_fits else {
        return out;
    };
    let has = |t: i64| lib.makeable.has(t);
    for h in lib.hulls_for(crate::doctrine::Role::Dps) {
        for real in table.usable(h.type_id, &has).take(SEED_FITS_PER_HULL) {
            let Some((family, fit)) = build(lib, real) else {
                continue;
            };
            let layer = layer_of(&fit.profile.defence);
            let e = out.groups.entry((h.type_id, family, layer)).or_default();
            e.0.by_hull.entry(h.type_id).or_default().push(real.clone());
            e.1.insert(
                real.hash.clone(),
                Seed {
                    class: h.class,
                    fit,
                    count: real.count,
                    hash: real.hash.clone(),
                },
            );
        }
    }
    out
}
