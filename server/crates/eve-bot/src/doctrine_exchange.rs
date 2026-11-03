//! **The counter matrix** (DOC-1, R-DOCTRINE section 3): no hand table, a
//! pure function of two doctrines and a range.
//!
//! A Lanchester square-law exchange. For fleets `A` (`n_a` ships) and `B`:
//!
//! ```text
//! alpha(X -> Y) = sum_i sum_j  s_i s_j  sum_t  dps_i[t] * apply(i, j, r) * (1 - jam)
//!                                        / ( ehp_j[t] + rep_j[t] * T_FIGHT )
//! ER(A, B, r)   = ln( n_a^2 alpha(A->B) / (n_b^2 alpha(B->A)) ) / 2
//! ```
//!
//! `alpha` is the fraction of one target killed per second by one shooter,
//! averaged over the role mixes; `ER > 0` means `A` wins, `+1` is about
//! 2.7 times. `apply` is the server's own function for the weapon:
//! [`eve_server::combat::Application`] for turrets (the very function the
//! engine rolls with, DOC-0), [`eve_server::missile::damage_factor`] and the
//! missile's fuse for launchers; ewar is [`eve_server::ewar`]'s falloff and
//! jam rule. **The range term is switchable** ([`Features::range`]): off, a
//! turret always hits and no range is dictated, which is the world before
//! DOC-0 and still the right model of weapons the engine does not roll.
//!
//! **Who dictates the range**: the side with `SPEED_EDGE` more line speed,
//! unless the other side's tackle can catch it and pin it (a web or scram
//! kills the edge); then the pinner fights from where its tackle reaches. The
//! dictator picks, of the three bands, the one that is best for it; the other
//! side gets the middle band. Because the exchange is computed from `A`'s and
//! `B`'s viewpoint with the same function, the matrix is antisymmetric in
//! sign by construction.

use crate::doctrine::{BRAWL_TACKLE, Delivery, Doctrine, GRID_CAP, Gun, Profile, RES_FLOOR, Role};
use eve_server::drone_skills::Aim;
use eve_server::missile::Aoe;
use eve_server::{ewar, missile};

// ---- the named constants. Each [A] has its falsifier ----------------------

/// The standard fight, seconds: reps and neuts count over it **[A]**.
/// Falsified by soak fights that last far longer or shorter.
pub const T_FIGHT: f64 = 90.0;
/// A target is repaired by at most this many logi at once **[A]**.
pub const LOGI_PER_TARGET: f64 = 3.0;
/// A logi whose repairer reaches under this repairs only itself **[A]**.
pub const LOGI_REACH_M: f64 = 20_000.0;
/// A side is faster by this multiple of line speed to dictate the range
/// **[A]**. Falsified by a soak fight of two rock-paper-scissors doctrines
/// whose result disagrees with the sign.
pub const SPEED_EDGE: f64 = 1.15;
/// The three engagement bands, metres **[A]**: close, middle, far.
pub const BANDS_M: [f64; 3] = [3_000.0, 15_000.0, 45_000.0];
/// The band the side that does not dictate gets (the middle one).
pub const DEFAULT_BAND: usize = 1;
/// A fleet in orbit cannot cross the line of sight faster than this angular
/// speed, rad/s **[A]**.
pub const ANG_CAP: f64 = 0.6;
/// The most of its damage a jam can take **[A]**.
pub const JAM_CAP: f64 = 0.95;
/// Ewar falloff as a share of its range **[A]**.
pub const EWAR_FALLOFF: f64 = 0.5;
/// Enemy ships one smartbomb volley reaches in a fight at the close band,
/// against the one target a gun picks **[A]**. Falsified by `eve-arena` on
/// a bomber against a swarm disagreeing with the sign.
pub const BOMB_VICTIMS: f64 = 3.0;
/// The share of a bomb volley that is not held back for friends **[A]**
/// (a bot bombs only when no friend is in range; a human does not).
pub const BOMB_FRIENDLY: f64 = 0.7;
/// The least a side's kill rate counts as, so the logarithm exists.
const ALPHA_FLOOR: f64 = 1e-12;

/// Which parts of the model are on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Features {
    /// Turret application (range, falloff, tracking), missile reach, lock
    /// range and the dictated range. Off until DOC-0, on now.
    pub range: bool,
}

impl Default for Features {
    fn default() -> Features {
        Features { range: true }
    }
}

/// One side of an exchange.
#[derive(Debug, Clone, Copy)]
pub struct Side<'a> {
    pub d: &'a Doctrine,
    /// Ships on grid.
    pub n: f64,
}

impl Side<'_> {
    fn roles(&self) -> impl Iterator<Item = (Role, f64, &Profile)> {
        self.d.roles.iter().map(|(r, s, f)| (*r, *s, &f.profile))
    }

    /// The line's speed: the first damage dealer's.
    pub fn line_speed(&self) -> f64 {
        self.roles()
            .find(|r| r.0 == Role::Dps)
            .map_or(0.0, |r| r.2.speed)
    }

    fn tackle_share(&self) -> f64 {
        self.roles()
            .filter(|(r, _, p)| *r == Role::Tackle && (p.ewar.pin_range > 0.0 || p.ewar.scram))
            .map(|r| r.1)
            .sum()
    }

    /// The fastest tackle hull and the range its web or scram reaches.
    fn tackle(&self) -> Option<(f64, f64)> {
        self.roles()
            .filter(|(r, _, p)| *r == Role::Tackle && (p.ewar.pin_range > 0.0 || p.ewar.scram))
            .map(|(_, _, p)| (p.speed, p.ewar.pin_range))
            .max_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// Whether a tackle hull of this side carries a scrambler (a disruptor
    /// does not stop a micro jump drive).
    fn tackle_scram(&self) -> bool {
        self.roles()
            .any(|(r, _, p)| r == Role::Tackle && p.ewar.scram)
    }

    /// Whether the line (first damage dealer) carries a micro jump drive.
    fn line_mjd(&self) -> bool {
        self.roles()
            .find(|r| r.0 == Role::Dps)
            .is_some_and(|r| r.2.mjd)
    }

    /// Whether this side can catch and hold `other`'s line. A line with a
    /// jump drive is held only by a scrambler.
    fn pins(&self, other: &Side) -> bool {
        self.tackle_share() >= BRAWL_TACKLE
            && self.tackle().is_some_and(|(s, _)| s >= other.line_speed())
            && (!other.line_mjd() || self.tackle_scram())
    }

    fn web(&self) -> f64 {
        self.roles()
            .map(|(_, _, p)| p.ewar.web_slow)
            .fold(0.0, f64::max)
    }

    fn paint(&self) -> (f64, f64) {
        self.roles()
            .map(|(_, _, p)| (p.ewar.paint, p.ewar.paint_range))
            .fold((0.0, 0.0), |a, b| if b.0 > a.0 { b } else { a })
    }

    fn damp(&self) -> (f64, f64) {
        self.roles()
            .map(|(_, _, p)| (p.ewar.damp, p.ewar.damp_range))
            .fold((0.0, 0.0), |a, b| if b.0 > a.0 { b } else { a })
    }
}

/// The share of the falloff curve of an ewar module at `r`.
fn reach(r: f64, range: f64) -> f64 {
    if range <= 0.0 {
        return 0.0;
    }
    ewar::falloff(r, range, EWAR_FALLOFF * range)
}

/// The strongest sensor type of a hull and its strength.
fn sensor(p: &Profile) -> (usize, f64) {
    p.sensor
        .iter()
        .copied()
        .enumerate()
        .fold((0, 0.0), |a, b| if b.1 > a.1 { b } else { a })
}

/// The share of `x`'s damage that `y`'s ECM takes away at `r`: each
/// jammer's chance is its strength for `x`'s sensor type over `x`'s sensor
/// strength ([`ewar::jam_chance`]'s ratio), times its falloff; the jammers
/// per target are `y`'s jammer ships over `x`'s ships.
fn jam_loss(x: &Profile, n_x: f64, y: &Side, r: f64) -> f64 {
    let (kind, strength) = sensor(x);
    if strength <= 0.0 || n_x <= 0.0 {
        return 0.0;
    }
    let mut keep = 1.0;
    for (_, share, p) in y.roles() {
        let e = &p.ewar;
        if e.ecm_range <= 0.0 {
            continue;
        }
        let chance =
            (e.ecm[kind] / strength).clamp(0.0, 1.0) * ewar::falloff(r, e.ecm_range, e.ecm_falloff);
        let jammers = share * y.n;
        keep *= (1.0 - chance).powf(jammers / n_x);
    }
    (1.0 - keep).min(JAM_CAP)
}

/// What `y`'s disruptors take off `x`'s shooter at `r`, as
/// `(optimal and falloff cut, tracking cut, missile cut)`: each disruptor
/// ship cuts by its strength times its falloff, and the disruptors per
/// target are `y`'s ships of that role over `x`'s ships (the shape of
/// [`jam_loss`]).
fn disrupt(n_x: f64, y: &Side, r: f64) -> (f64, f64, f64) {
    if n_x <= 0.0 {
        return (0.0, 0.0, 0.0);
    }
    let (mut opt, mut track, mut guide) = (1.0, 1.0, 1.0);
    for (_, share, p) in y.roles() {
        let e = &p.ewar;
        let ships = share * y.n / n_x;
        if e.track_range > 0.0 {
            let w = ewar::falloff(r, e.track_range, e.track_falloff);
            opt *= (1.0 - e.opt_cut * w).powf(ships);
            track *= (1.0 - e.track_cut * w).powf(ships);
        }
        if e.guide_range > 0.0 {
            guide *=
                (1.0 - e.guide_cut * ewar::falloff(r, e.guide_range, e.guide_falloff)).powf(ships);
        }
    }
    (1.0 - opt, 1.0 - track, 1.0 - guide)
}

/// Capacitor drained per second from one target by `x`'s neuts at `r`
/// (all of them on one target: focus fire).
fn neut_pressure(x: &Side, r: f64) -> f64 {
    x.roles()
        .map(|(_, s, p)| s * x.n * p.ewar.neut * reach(r, p.ewar.neut_range))
        .sum()
}

/// The fraction of an active tank a neut pressure shuts down.
fn neut_cut(pressure: f64, regen: f64) -> f64 {
    if pressure <= 0.0 {
        0.0
    } else if regen <= 0.0 {
        1.0
    } else {
        (pressure / regen).clamp(0.0, 1.0)
    }
}

/// **Effective hit points** of the side's line against pure damage of each
/// type: the fit's own EHP plus what its local repair and the fleet's logi
/// give over [`T_FIGHT`]; a `neut_pressure` (capacitor per second on one
/// target) shuts the active part down in proportion to the fit's recharge.
pub fn effective_ehp(side: &Side<'_>, neut_pressure: f64) -> [f64; 4] {
    let Some((_, _, line)) = side.roles().find(|r| r.0 == Role::Dps) else {
        return [0.0; 4];
    };
    effective_for(side, line, neut_pressure)
}

fn effective_for(side: &Side<'_>, target: &Profile, neut_pressure_on: f64) -> [f64; 4] {
    let base = target.ehp();
    let cut = neut_cut(neut_pressure_on, target.cap_regen);
    let local = target.rep_ehp(
        target.rep_local[0] * T_FIGHT * (1.0 - cut),
        target.rep_local[1] * T_FIGHT * (1.0 - cut),
    );
    // Logi: the pool the focus target can draw on.
    let mut pool = 0.0;
    let mut best: f64 = 0.0;
    let mut layer = None;
    for (_, share, p) in side.roles() {
        if p.remote_rep <= 0.0 || p.remote_range < LOGI_REACH_M {
            continue;
        }
        let rate = p.remote_rep * (1.0 - neut_cut(neut_pressure_on, p.cap_regen));
        pool += rate * share * side.n;
        best = best.max(rate);
        layer = layer.or(p.remote_layer);
    }
    let on_target = pool.min(LOGI_PER_TARGET * best) * T_FIGHT;
    let remote = match layer {
        Some(crate::doctrine::Layer::Shield) => target.rep_ehp(on_target, 0.0),
        Some(_) => target.rep_ehp(0.0, on_target),
        None => [0.0; 4],
    };
    [0, 1, 2, 3].map(|i| base[i] + local[i] + remote[i])
}

/// What the fight around one shot adds: the transversal speed across the
/// line of sight, the signature a painter adds, the share of speed webs take
/// off the target, and the share of the shooter's lock range dampers take.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Env {
    pub transversal: f64,
    pub paint: f64,
    pub web: f64,
    pub lock_cut: f64,
    /// The shares a tracking disruptor takes off the shooter's optimal and
    /// falloff, and off its tracking speed.
    pub opt_cut: f64,
    pub track_cut: f64,
    /// The share a guidance disruptor takes off a missile's reach and
    /// explosion velocity.
    pub guide_cut: f64,
}

/// **Application** of one shooter profile on one target at `r` metres with
/// `transversal` m/s across the line of sight: the share of its damage that
/// lands (0 to 1). Turrets by [`eve_server::combat::Application`]; missiles
/// by their fuse and [`missile::damage_factor`] on the target's signature
/// and speed; a shooter that cannot lock at `r` lands nothing.
pub fn apply(x: &Profile, y: &Profile, r: f64, env: Env, feat: Features) -> f64 {
    let sig = y.sig * (1.0 + env.paint);
    if feat.range && x.lock_range > 0.0 && r > x.lock_range * (1.0 - env.lock_cut) {
        return 0.0;
    }
    match x.delivery {
        Delivery::None => 0.0,
        Delivery::Turret(g) => {
            if !feat.range {
                1.0
            } else {
                let g = Gun {
                    optimal: g.optimal * (1.0 - env.opt_cut),
                    falloff: g.falloff * (1.0 - env.opt_cut),
                    tracking: g.tracking * (1.0 - env.track_cut),
                    ..g
                };
                g.application().chance(Aim {
                    distance: r,
                    transversal: env.transversal,
                    signature: sig,
                })
            }
        }
        Delivery::Missile(f) => {
            if feat.range && r > f.reach * (1.0 - env.guide_cut) {
                return 0.0;
            }
            let aoe = Aoe {
                velocity: f.aoe.velocity * (1.0 - env.guide_cut),
                cloud: f.aoe.cloud * (1.0 + env.guide_cut),
                ..f.aoe
            };
            missile::damage_factor(sig, y.speed * (1.0 - env.web), aoe)
        }
    }
}

/// The transversal speed at `r`: whoever moves fastest after the other
/// side's webs, capped by the angular speed of an orbit at `r`.
fn transversal(a: &Side, b: &Side, r: f64) -> f64 {
    let v = (a.line_speed() * (1.0 - b.web())).max(b.line_speed() * (1.0 - a.web()));
    v.min(r * ANG_CAP)
}

/// `alpha(x -> y)` at `r`: the fraction of one `y` ship killed per second
/// by one `x` ship, averaged over both mixes.
pub fn alpha(x: &Side, y: &Side, r: f64, feat: Features) -> f64 {
    let tv = transversal(x, y, r);
    let (paint, paint_range) = x.paint();
    let paint = paint * reach(r, paint_range);
    let web = x.web();
    let (damp, damp_range) = y.damp();
    let damp = damp * reach(r, damp_range);
    let pressure = neut_pressure(x, r);
    let mut total = 0.0;
    for (_, si, pi) in x.roles() {
        let bomb_dps: f64 = pi.ewar.bomb.iter().sum();
        if pi.total_dps() <= 0.0 && bomb_dps <= 0.0 {
            continue;
        }
        let loss = jam_loss(pi, x.n * si, y, r);
        let (opt_cut, track_cut, guide_cut) = disrupt(x.n * si, y, r);
        for (_, sj, pj) in y.roles() {
            let eff = effective_for(y, pj, pressure);
            let env = Env {
                transversal: tv,
                paint,
                web,
                lock_cut: damp,
                opt_cut,
                track_cut,
                guide_cut,
            };
            let app = apply(pi, pj, r, env, feat) * (1.0 - loss);
            let kill: f64 = pi
                .dps
                .iter()
                .zip(eff)
                .map(|(d, e)| d * app / e.max(RES_FLOOR))
                .sum();
            // A smartbomb needs no lock and no aim; it lands on every ship in
            // its (short) reach, so it counts only in the close band.
            let bomb: f64 = if pi.ewar.bomb_range >= r {
                pi.ewar
                    .bomb
                    .iter()
                    .zip(eff)
                    .map(|(d, e)| d / e.max(RES_FLOOR))
                    .sum::<f64>()
                    * BOMB_VICTIMS.min(y.n)
                    * BOMB_FRIENDLY
            } else {
                0.0
            };
            total += si * sj * (kill + bomb);
        }
    }
    total
}

/// **The exchange score** at one band: `0` even, `+1` is `A` ahead by 2.7
/// times in square-law strength.
pub fn er_at(a: &Side, b: &Side, band: usize, feat: Features) -> f64 {
    let r = BANDS_M[band.min(BANDS_M.len() - 1)];
    let ab = (a.n * a.n * alpha(a, b, r, feat)).max(ALPHA_FLOOR);
    let ba = (b.n * b.n * alpha(b, a, r, feat)).max(ALPHA_FLOOR);
    0.5 * (ab / ba).ln()
}

/// Who chooses the band, and which bands it may choose from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dictator {
    A,
    B,
    /// Nobody: both fight in the middle band.
    Nobody,
}

/// The bands a pin at `pin_range` metres can hold: those no farther than it
/// (the closest always).
fn bands_within(pin_range: f64) -> Vec<usize> {
    let ok: Vec<usize> = (0..BANDS_M.len())
        .filter(|i| BANDS_M[*i] <= pin_range)
        .collect();
    if ok.is_empty() { vec![0] } else { ok }
}

/// Who dictates, and the bands open to them.
pub fn dictator(a: &Side, b: &Side) -> (Dictator, Vec<usize>) {
    let all: Vec<usize> = (0..BANDS_M.len()).collect();
    let (sa, sb) = (a.line_speed(), b.line_speed());
    let a_pins = a.pins(b);
    let b_pins = b.pins(a);
    if sa >= sb * SPEED_EDGE && !b_pins {
        (Dictator::A, all)
    } else if sb >= sa * SPEED_EDGE && !a_pins {
        (Dictator::B, all)
    } else if b_pins && !a_pins {
        (Dictator::B, bands_within(b.tackle().map_or(0.0, |t| t.1)))
    } else if a_pins && !b_pins {
        (Dictator::A, bands_within(a.tackle().map_or(0.0, |t| t.1)))
    } else {
        (Dictator::Nobody, vec![contested_band(a, b)])
    }
}

/// **The band nobody dictates** (ENGINE-FIX): with no speed edge and no pin,
/// the arena's fleets do not meet in the middle. The shorter-reached side
/// closes to inside its own reach (a brawler on an afterburner is as quick as
/// the kiter it chases) and the longer-reached side cannot get away, so the
/// fight is at the farthest band the shorter reach still covers. Measured:
/// the first matrix put such pairs at 15 km, where a 3.7 km brawler hits
/// nothing and the score ran to +15; in the engine the brawlers closed and
/// the same pairs came out near even (`docs/lanes/engine-fix.md`). **[A]**;
/// falsified by an arena campaign in which the longer-reached side wins
/// no-edge pairs it was predicted to lose.
pub fn contested_band(a: &Side, b: &Side) -> usize {
    let reach = |s: &Side| {
        s.roles()
            .find(|r| r.0 == Role::Dps)
            .map_or(0.0, |r| r.2.reach())
    };
    let (ra, rb) = (reach(a), reach(b));
    let shorter = match (ra > 0.0, rb > 0.0) {
        (true, true) => ra.min(rb),
        (true, false) => ra,
        (false, true) => rb,
        (false, false) => return DEFAULT_BAND,
    };
    (0..BANDS_M.len())
        .rev()
        .find(|i| BANDS_M[*i] <= shorter)
        .unwrap_or(0)
}

/// The result of one exchange.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Exchange {
    pub er: f64,
    pub band: usize,
    pub dictator: Dictator,
}

/// **The exchange**: the dictator picks the band best for it among those
/// open to it (the first of equals); with the range term off, or nobody
/// dictating, the middle band.
pub fn exchange(a: &Side, b: &Side, feat: Features) -> Exchange {
    let (who, open) = if feat.range {
        dictator(a, b)
    } else {
        (Dictator::Nobody, vec![DEFAULT_BAND])
    };
    let mut best = (open[0], er_at(a, b, open[0], feat));
    for &band in &open[1..] {
        let er = er_at(a, b, band, feat);
        let better = match who {
            Dictator::A => er > best.1,
            Dictator::B => er < best.1,
            Dictator::Nobody => false,
        };
        if better {
            best = (band, er);
        }
    }
    Exchange {
        er: best.1,
        band: best.0,
        dictator: who,
    }
}

/// Ships each side fields at equal spend, the cheaper side up to the grid.
pub fn counts(a: &Doctrine, b: &Doctrine) -> (f64, f64) {
    let (ca, cb) = (a.cost_per_ship(), b.cost_per_ship());
    if ca <= 0.0 || cb <= 0.0 {
        return (a.fleet.target as f64, b.fleet.target as f64);
    }
    let budget = (a.fleet.target as f64 * ca).max(b.fleet.target as f64 * cb);
    let n = |c: f64| (budget / c).clamp(1.0, GRID_CAP as f64);
    (n(ca), n(cb))
}

/// The score of doctrine `a` against `b` at equal spend.
pub fn versus(a: &Doctrine, b: &Doctrine, feat: Features) -> Exchange {
    let (na, nb) = counts(a, b);
    exchange(&Side { d: a, n: na }, &Side { d: b, n: nb }, feat)
}

/// The score at one band at equal spend.
pub fn versus_at(a: &Doctrine, b: &Doctrine, band: usize, feat: Features) -> f64 {
    let (na, nb) = counts(a, b);
    er_at(&Side { d: a, n: na }, &Side { d: b, n: nb }, band, feat)
}

/// **The matrix**: `er[i][j]` is catalog `i` against `j` (positive: `i`
/// wins) at the range the dictator takes; `band[i][j]` that range.
#[derive(Debug, Clone, PartialEq)]
pub struct Matrix {
    pub er: Vec<Vec<f64>>,
    pub band: Vec<Vec<usize>>,
}

/// Compute the whole matrix once: only `i < j` is computed and the other
/// half is its negation, so the sign is antisymmetric by construction.
pub fn matrix(cat: &[Doctrine], feat: Features) -> Matrix {
    let n = cat.len();
    let mut er = vec![vec![0.0; n]; n];
    let mut band = vec![vec![DEFAULT_BAND; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let x = versus(&cat[i], &cat[j], feat);
            er[i][j] = x.er;
            er[j][i] = -x.er;
            band[i][j] = x.band;
            band[j][i] = x.band;
        }
    }
    Matrix { er, band }
}

/// Doctrines that do not lose to any other at any band (the rock-paper-
/// scissors property says there are none). Indices into `cat`.
pub fn dominant(cat: &[Doctrine], feat: Features) -> Vec<usize> {
    (0..cat.len())
        .filter(|&i| {
            (0..cat.len()).filter(|&j| j != i).all(|j| {
                let bands: &[usize] = if feat.range {
                    &[0, 1, 2]
                } else {
                    &[DEFAULT_BAND]
                };
                bands
                    .iter()
                    .all(|&b| versus_at(&cat[i], &cat[j], b, feat) >= 0.0)
            })
        })
        .collect()
}

/// **The `[doc] matrix` line**: the catalog size, for each of the five most
/// fielded doctrines its three best counters, and the dominance check.
/// `fielded` is `(catalog index, ships)`; with none, the first five.
pub fn matrix_line(
    cat: &[Doctrine],
    m: &Matrix,
    fielded: &[(usize, u32)],
    feat: Features,
) -> String {
    let mut top: Vec<(usize, u32)> = fielded
        .iter()
        .copied()
        .filter(|f| f.0 < cat.len())
        .collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    if top.is_empty() {
        top = (0..cat.len().min(5)).map(|i| (i, 0)).collect();
    }
    top.truncate(5);
    let mut parts = Vec::new();
    for (i, _) in top {
        let mut counters: Vec<(usize, f64)> = (0..cat.len())
            .filter(|&j| j != i && m.er[j][i] > 0.0)
            .map(|j| (j, m.er[j][i]))
            .collect();
        counters.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        counters.truncate(3);
        let list: Vec<String> = counters
            .iter()
            .map(|(j, e)| format!("{}({:+.2})", cat[*j].key, e))
            .collect();
        parts.push(format!(
            "{} <- {}",
            cat[i].key,
            if list.is_empty() {
                "none".to_owned()
            } else {
                list.join(",")
            }
        ));
    }
    let dom = dominant(cat, feat);
    let dom = if dom.is_empty() {
        "none".to_owned()
    } else {
        dom.iter()
            .map(|i| cat[*i].key.clone())
            .collect::<Vec<_>>()
            .join(",")
    };
    format!(
        "[doc] matrix: catalog={} bands={} range={} counters: {}; dominant: {}",
        cat.len(),
        BANDS_M.len(),
        if feat.range { "on" } else { "off" },
        if parts.is_empty() {
            "none".to_owned()
        } else {
            parts.join(" | ")
        },
        dom
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctrine::testkit::*;

    const ON: Features = Features { range: true };
    const OFF: Features = Features { range: false };

    fn missile_ship() -> Profile {
        let mut p = brawler(flat_tank(2000.0, 2000.0, 1000.0));
        p.delivery = Delivery::Missile(crate::doctrine::Flight {
            reach: 30_000.0,
            aoe: Aoe {
                cloud: 100.0,
                velocity: 150.0,
                drf: 0.5,
            },
        });
        p.lock_range = 80_000.0;
        p
    }

    /// A ship whose only job is a disruptor.
    fn disruptor_ship(td: bool) -> Profile {
        let mut p = Profile::blank(flat_tank(1000.0, 1000.0, 500.0));
        p.speed = 200.0;
        if td {
            p.ewar.track_cut = 0.5;
            p.ewar.opt_cut = 0.5;
            p.ewar.track_range = 30_000.0;
            p.ewar.track_falloff = 10_000.0;
        } else {
            p.ewar.guide_cut = 0.5;
            p.ewar.guide_range = 30_000.0;
            p.ewar.guide_falloff = 10_000.0;
        }
        p
    }

    fn shot_at(x: &Profile, against: &Side, r: f64) -> f64 {
        let n_x = 10.0;
        let (opt_cut, track_cut, guide_cut) = disrupt(n_x, against, r);
        let env = Env {
            opt_cut,
            track_cut,
            guide_cut,
            ..Env::default()
        };
        apply(x, &swarm(), r, env, ON)
    }

    #[test]
    fn a_td_hurts_turrets_not_missiles() {
        let td = with_role(brick_doc(), Role::Ewar, 0.3, disruptor_ship(true), 100.0);
        let against = Side { d: &td, n: 10.0 };
        let turret = brick();
        let missile = missile_ship();
        let plain = Side {
            d: &brick_doc(),
            n: 10.0,
        };
        let r = 8_000.0;
        assert!(shot_at(&turret, &against, r) < shot_at(&turret, &plain, r));
        assert_eq!(shot_at(&missile, &against, r), shot_at(&missile, &plain, r));
    }

    #[test]
    fn a_gd_hurts_missiles_not_turrets() {
        let gd = with_role(brick_doc(), Role::Ewar, 0.3, disruptor_ship(false), 100.0);
        let against = Side { d: &gd, n: 10.0 };
        let plain = Side {
            d: &brick_doc(),
            n: 10.0,
        };
        let r = 8_000.0;
        let missile = missile_ship();
        assert!(shot_at(&missile, &against, r) < shot_at(&missile, &plain, r));
        let turret = brick();
        assert_eq!(shot_at(&turret, &against, r), shot_at(&turret, &plain, r));
        // Past the cut reach the missile flies no farther.
        assert_eq!(shot_at(&missile, &against, 28_000.0), 0.0);
    }

    #[test]
    fn bombs_count_only_in_the_close_band() {
        let mut bomber = brick();
        bomber.dps = [0.0; 4];
        bomber.ewar.bomb = [0.0, 0.0, 100.0, 0.0];
        bomber.ewar.bomb_range = 4_000.0;
        let bomb = doc(bomber, 100.0);
        let target = doc(swarm(), 100.0);
        let (b, t) = (
            Side { d: &bomb, n: 10.0 },
            Side {
                d: &target,
                n: 10.0,
            },
        );
        let close = alpha(&b, &t, BANDS_M[0], ON);
        let mid = alpha(&b, &t, BANDS_M[1], ON);
        assert!(close > 0.0, "{close}");
        assert_eq!(mid, 0.0);
    }

    #[test]
    fn a_scram_holds_an_mjd_line_and_a_disruptor_does_not() {
        let mut jumper = kiter();
        jumper.mjd = true;
        let jump = doc(jumper, 100.0);
        let scram = with_role(brick_doc(), Role::Tackle, 0.2, pinner(), 100.0);
        let mut soft = pinner();
        soft.ewar.scram = false;
        soft.ewar.point_range = 9_000.0;
        let point = with_role(brick_doc(), Role::Tackle, 0.2, soft, 100.0);
        fn side(d: &Doctrine) -> Side<'_> {
            Side { d, n: 10.0 }
        }
        let (j, s, p) = (side(&jump), side(&scram), side(&point));
        assert!(s.pins(&j), "a scrambler holds a jump drive");
        assert!(!p.pins(&j), "a disruptor does not");
        // The same line without a drive is held by either.
        let plain = doc(kiter(), 100.0);
        assert!(p.pins(&side(&plain)));
    }

    #[test]
    fn all_em_loses_to_an_em_resist_tank() {
        // Same hit points and the same raw damage per second; only the
        // damage type and the tank's resonance for it differ.
        let fire = |dps: [f64; 4], tank| {
            let mut p = brawler(tank);
            p.dps = dps;
            doc(p, 100.0)
        };
        let em_hard = {
            let mut d = flat_tank(2000.0, 2000.0, 1000.0);
            d.shield_resonance.em = 0.2;
            d.armor_resonance.em = 0.2;
            d
        };
        let plain = flat_tank(2000.0, 2000.0, 1000.0);
        let em = [300.0, 0.0, 0.0, 0.0];
        let kin = [0.0, 0.0, 300.0, 0.0];
        // Kinetic against kinetic, tanks alike for it: even.
        let even = versus(&fire(kin, plain), &fire(kin, em_hard), ON).er;
        assert!(even.abs() < 1e-6, "{even}");
        // EM into the EM-resist tank loses; the margin is the resonance.
        let lose = versus(&fire(em, plain), &fire(kin, em_hard), ON).er;
        assert!(lose < -0.5, "{lose}");
        // The same fleet firing kinetic is not hurt by the EM resist.
        assert!(lose < even);
    }

    #[test]
    fn a_faster_longer_ranged_fleet_wins_unless_tackled() {
        let kite = doc(kiter(), 100.0);
        let brick = doc(brick(), 100.0);
        let v = versus(&kite, &brick, ON);
        assert_eq!(v.dictator, Dictator::A);
        assert_eq!(v.band, 2, "the kiter fights from the far band");
        assert!(v.er > 0.0, "the kiter wins: {}", v.er);
        // The same brick with a quick tackle that can pin: the edge dies.
        let tackled = with_role(brick_doc(), Role::Tackle, 0.2, pinner(), 100.0);
        let v = versus(&kite, &tackled, ON);
        assert_ne!(v.dictator, Dictator::A, "the speed edge is gone");
        assert!(
            v.er < 0.0,
            "the brawlers win once they hold the kite: {}",
            v.er
        );
    }

    /// ENGINE-FIX: with no speed edge and no pin nobody dictates, and the
    /// fight is at the farthest band the shorter reach covers -- not in the
    /// middle, where the brawler hits nothing and the score ran away.
    #[test]
    fn without_a_speed_edge_the_fight_is_where_the_shorter_reach_covers() {
        let brick_p = brick();
        let mut slow_kite = kiter();
        slow_kite.speed = brick_p.speed;
        let kite = doc(slow_kite, 100.0);
        let brick = doc(brick_p, 100.0);
        let v = versus(&kite, &brick, ON);
        assert_eq!(v.dictator, Dictator::Nobody);
        let reach = brick.roles[0].2.profile.reach();
        let want = (0..BANDS_M.len())
            .rev()
            .find(|i| BANDS_M[*i] <= reach)
            .unwrap_or(0);
        assert_eq!(v.band, want, "the brawler's reach {reach} picks the band");
        assert!(v.band < DEFAULT_BAND || reach >= BANDS_M[DEFAULT_BAND]);
    }

    #[test]
    fn large_guns_against_small_fast_hulls_collapse() {
        let big = battleship();
        let small = swarm();
        let t = 15_000.0 * ANG_CAP;
        let env = |tv| Env {
            transversal: tv,
            ..Env::default()
        };
        let hits = apply(&big, &small, 15_000.0, env(t.min(small.speed)), ON);
        let easy = apply(&big, &battleship(), 15_000.0, env(0.0), ON);
        assert!(
            hits < 0.05 * easy,
            "tracking vs signature: {hits} vs {easy}"
        );
        // At equal spend the swarm beats the blob.
        let v = versus(&doc(big, 800.0), &doc(small, 50.0), ON);
        assert!(v.er < 0.0, "{}", v.er);
    }

    #[test]
    fn logi_raises_effective_ehp() {
        let plain = doc(brick(), 100.0);
        let logi = with_role(brick_doc(), Role::Logi, 0.2, logi_ship(), 100.0);
        let side = |d| Side { d, n: 10.0 };
        let (p, l) = (side(&plain), side(&logi));
        let base = effective_ehp(&p, 0.0);
        let with = effective_ehp(&l, 0.0);
        let rep = logi_ship().remote_rep;
        for t in 0..4 {
            assert!(with[t] > base[t], "type {t}");
        }
        assert!(with[2] - base[2] >= rep * T_FIGHT * 0.99 / 4.0);
        // A neut-heavy enemy turns the logi's cycles off.
        let neutered = effective_ehp(&l, 1.0e6);
        for t in 0..4 {
            assert!(neutered[t] < with[t]);
        }
    }

    #[test]
    fn the_matrix_is_antisymmetric_in_sign() {
        let cat = invented_catalog();
        let m = matrix(&cat, ON);
        for i in 0..cat.len() {
            for j in 0..cat.len() {
                if i == j {
                    continue;
                }
                assert_eq!(m.er[i][j].signum(), -m.er[j][i].signum(), "{i} {j}");
                // And computed from the other side it is the same number.
                let rev = versus(&cat[j], &cat[i], ON);
                assert!((rev.er + m.er[i][j]).abs() < 1e-9, "{i} {j}");
            }
        }
    }

    #[test]
    fn without_range_the_range_term_is_off() {
        let kite = doc(kiter(), 100.0);
        let brick = doc(brick(), 100.0);
        let e: Vec<f64> = (0..3).map(|b| versus_at(&kite, &brick, b, OFF)).collect();
        assert!(e.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-9), "{e:?}");
        let v = versus(&kite, &brick, OFF);
        assert_eq!(v.dictator, Dictator::Nobody);
        assert_eq!(v.band, DEFAULT_BAND);
        // With it on the same pair is decided by range.
        assert!(versus(&kite, &brick, ON).er > v.er);
    }

    #[test]
    fn no_hand_made_doctrine_dominates() {
        let cat = invented_catalog();
        assert!(dominant(&cat, ON).is_empty(), "{:?}", dominant(&cat, ON));
        let line = matrix_line(&cat, &matrix(&cat, ON), &[], ON);
        assert!(line.starts_with("[doc] matrix: catalog="), "{line}");
        assert!(line.ends_with("dominant: none"), "{line}");
    }
}
