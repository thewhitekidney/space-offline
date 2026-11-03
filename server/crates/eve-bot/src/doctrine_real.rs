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
    Kind, Library, ROUNDS_ALTERNATE, ROUNDS_PRIMARY, choose_rounds, profile_of, skills_for,
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

/// **One real fit on the paper**: modules from the row, the row's charge in
/// the guns (the greedy choice when it names none) and the best alternate,
/// the doctrine's skills, the engine's numbers. `None` when the fit has no
/// gun (no family), the engine does not hold it or it fails
/// CPU, power or capacitor.
pub fn build(lib: &Library<'_>, real: &RealFit) -> Option<(Family, RoleFit)> {
    let gun = real
        .modules
        .iter()
        .map(|m| m.0)
        .find(|t| matches!(lib.kind_of(*t), Some(Kind::Gun(..))))?;
    let Some(Kind::Gun(family, _)) = lib.kind_of(gun) else {
        return None;
    };
    let is_gun = |w: i64| matches!(lib.kind_of(w), Some(Kind::Gun(..)));
    let (greedy, alt) = choose_rounds(lib, gun).map_or((None, None), |(p, a)| (Some(p), a));
    let primary = real.charge_for(is_gun).or(greedy)?;
    let alt = alt.filter(|a| *a != primary && lib.makeable.has(*a));
    let types: Vec<i64> = std::iter::once(real.hull)
        .chain(real.modules.iter().map(|m| m.0))
        .chain([primary])
        .chain(alt)
        .collect();
    let skills = skills_for(lib.inst.table(), &types, DOCTRINE_LEVEL);
    let profile = profile_of(lib, real.hull, &real.modules, Some(primary), &skills)?;
    if !profile.ok() {
        return None;
    }
    let guns: u32 = real
        .modules
        .iter()
        .filter(|m| is_gun(m.0))
        .map(|m| u32::from(m.1))
        .sum();
    let mut charges = vec![(primary, ROUNDS_PRIMARY * guns.max(1))];
    if let Some(a) = alt {
        charges.push((a, ROUNDS_ALTERNATE * guns.max(1)));
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
    Some((
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
