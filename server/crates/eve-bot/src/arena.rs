//! **The arena** (DOC-9; R-DOCTRINE section 9 G8, section 12): two fleets of
//! given doctrines of equal spend fight on one grid in the real combat
//! engine -- turret application, missiles, ewar, remote repair as the server
//! built them -- and report who won, the ISK each side destroyed and how
//! long it took. Repeated over seeds it answers G8: does the counter the
//! matrix picked win more often than chance?
//!
//! **How it runs.** Each skirmish binds a fresh in-process server on a free
//! loopback port (`Server::bind`, as every socket test does) with one sandbox
//! character per ship, builds every ship through the ordinary client calls
//! (GM `/createitem` and `/giveskill` for the parts and the skills, then
//! `FitAuto`, `SetModuleOnline`, `LoadAmmo`, undock), puts the two fleets
//! [`START_SEP_M`] apart and drives every pilot with plain calls: lock the
//! side's primary, fire the guns and ewar at it, repair a friend, orbit at
//! the fit's reach. Nothing here is a bot's brain: the harness stands in for
//! a well-run fleet (one primary, no losses of nerve), so the result is the
//! engine's verdict on the two fits, which is what the matrix predicts.
//!
//! **Not the soak.** A GM spawns the parts in the arena's private server;
//! nothing of it touches a society run's economy (the nothing-from-nowhere
//! rule is about the soak world).
//!
//! The `[arena]` lines: [`skirmish_line`] per fight, [`pair_line`] per
//! matchup; `tools/doctrine-report.py` reads them.

use crate::brain::{unit, xorshift};
use crate::doctrine::{Doctrine, Role, RoleFit};
use crate::doctrine_exchange::{Features, Matrix, counts};
use crate::driver::DEV_TOKEN;
use crate::verbs::ItemRow;
use crate::{Conn, fit_plan};
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::server::{Config, Server};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Ships a side fields at most. The engine's grid holds more; this keeps
/// one skirmish a handful of connections **[A]**. Falsifier: a counter that
/// wins at 6 a side and loses at 12 (run `--cap 12`).
pub const ARENA_CAP: usize = 6;
/// Metres between the fleets when the fight opens (the matrix's middle
/// band is 15 km) **[A]**.
pub const START_SEP_M: f64 = 10_000.0;
/// Jitter on the separation, a share either way, drawn from the seed.
pub const SEP_JITTER: f64 = 0.2;
/// Metres from the station the nearer fleet is put at (`/tp`'s floor is
/// the undock offset, 20 km).
pub const ARENA_KM: f64 = 25.0;
/// A skirmish that has not resolved by then is a draw (wall seconds; the
/// engine runs in real time).
pub const LIMIT_S: u64 = 600;
/// Seconds between a pilot's decisions.
pub const TICK_MS: u64 = 400;
/// Rounds of a charge a fit loads at most per gun.
pub const ROUNDS_CAP: i64 = 2_000;
/// The orbit is this share of the fit's reach, never under [`ORBIT_MIN_M`]
/// **[A]**.
pub const ORBIT_SHARE: f64 = 0.8;
pub const ORBIT_MIN_M: f64 = 500.0;
/// How much of its reach a ship holding a dictated band stays inside **[A]**
/// (a missile's reach is its fuse, a turret's optimal plus half its falloff).
pub const KITE_SHARE: f64 = 0.95;
/// Seconds between looks at whether a gun has stopped or run dry.
pub const RELOAD_LOOK_S: u64 = 3;
/// Enemy ships locked ahead (the primary included) **[A]**: a fit holds
/// `maxLockedTargets` locks, 5 on the invented hull.
pub const PRELOCK: usize = 3;
/// ... and never past this share of the lock range **[A]**.
pub const LOCK_SHARE: f64 = 0.8;
/// The arena's home system (the sandbox's) and its first ids.
pub const SYSTEM: i64 = 30_000_001;
const FIRST_CHARACTER: i64 = 91_000_001;
const FIRST_SHIP: i64 = 101_000_001;
/// `repeat` for a module that runs until stopped.
const FOREVER: i64 = 1_000;

/// One side of a skirmish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    A,
    B,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::A => "A",
            Side::B => "B",
        }
    }

    pub fn other(self) -> Side {
        match self {
            Side::A => Side::B,
            Side::B => Side::A,
        }
    }

    fn ix(self) -> usize {
        self as usize
    }
}

/// A fleet: the doctrine and how many ships.
#[derive(Debug, Clone, Copy)]
pub struct Fleet<'a> {
    pub doctrine: &'a Doctrine,
    pub n: usize,
}

/// What one skirmish came to.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// `None`: a draw (both gone together, or the limit).
    pub winner: Option<Side>,
    /// Ships each side started with and lost.
    pub fielded: [usize; 2],
    pub lost: [usize; 2],
    /// ISK each side lost (the fits' paper cost, `RoleFit::cost`).
    pub isk_lost: [f64; 2],
    /// Wall seconds from the first shot to the resolution.
    pub secs: f64,
    pub timed_out: bool,
    /// Hit points of damage each side's guns did (the damage messages'
    /// own `damage`, after resistances), shots fired and shots that hit.
    pub dealt: [f64; 2],
    pub shots: [u32; 2],
    pub hits: [u32; 2],
}

impl Outcome {
    /// ISK `side` destroyed.
    pub fn destroyed_by(&self, side: Side) -> f64 {
        self.isk_lost[side.other().ix()]
    }
}

// ---- the pure parts -------------------------------------------------------

/// **The roster of `n` ships**: the doctrine's roles in proportion to their
/// shares (largest remainder), damage dealers first, so a fleet of one is a
/// damage dealer.
pub fn roster(d: &Doctrine, n: usize) -> Vec<&RoleFit> {
    let n = n.max(1);
    let total: f64 = d.roles.iter().map(|(_, s, _)| s).sum();
    if d.roles.is_empty() || total <= 0.0 {
        return Vec::new();
    }
    let mut rows: Vec<(usize, usize, f64)> = d
        .roles
        .iter()
        .enumerate()
        .map(|(i, (_, s, _))| {
            let exact = s / total * n as f64;
            (i, exact.floor() as usize, exact - exact.floor())
        })
        .collect();
    let mut left = n.saturating_sub(rows.iter().map(|r| r.1).sum());
    let mut by_rest: Vec<usize> = (0..rows.len()).collect();
    by_rest.sort_by(|&a, &b| rows[b].2.total_cmp(&rows[a].2).then(a.cmp(&b)));
    for i in by_rest {
        if left == 0 {
            break;
        }
        rows[i].1 += 1;
        left -= 1;
    }
    // A damage dealer is always on the field.
    if !rows.iter().any(|r| d.roles[r.0].0 == Role::Dps && r.1 > 0)
        && let Some(dps) = rows.iter().position(|r| d.roles[r.0].0 == Role::Dps)
        && let Some(most) = (0..rows.len()).max_by_key(|&i| rows[i].1)
        && rows[most].1 > 0
    {
        rows[most].1 -= 1;
        rows[dps].1 += 1;
    }
    let mut out: Vec<&RoleFit> = Vec::new();
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by_key(|&i| (d.roles[i].0 != Role::Dps, i));
    for i in order {
        for _ in 0..rows[i].1 {
            out.push(&d.roles[i].2);
        }
    }
    out
}

/// **Ships each side fields** for `a` against `b`: the matrix's equal-spend
/// counts ([`counts`]) scaled so the larger side is at most `cap`, the
/// ratio kept.
pub fn fielded(a: &Doctrine, b: &Doctrine, cap: usize) -> (usize, usize) {
    let (na, nb) = counts(a, b);
    let cap = cap.max(1) as f64;
    let k = (cap / na.max(nb)).min(1.0);
    let n = |x: f64| ((x * k).round() as usize).clamp(1, cap as usize);
    (n(na), n(nb))
}

/// **The counter the matrix names** for catalog entry `i`: the entry whose
/// score against `i` is highest, with that score. `None` for a catalog of
/// one.
pub fn counter_of(m: &Matrix, i: usize) -> Option<(usize, f64)> {
    (0..m.er.len())
        .filter(|&j| j != i)
        .map(|j| (j, m.er[j][i]))
        .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
}

/// Median of `v`; `NaN` for none. Infinite values order as they are.
/// **The band each side holds** (ENGINE-FIX): the side that dictates the
/// range in `doctrine_exchange::exchange` flies at the band the exchange
/// gave it; the other side, and both when nobody dictates, close in as they
/// always did ([`hold_distance`] with no band). Metres, indexed by [`Side`].
pub fn bands(a: Fleet<'_>, b: Fleet<'_>, feat: Features) -> [Option<f64>; 2] {
    use crate::doctrine_exchange::{BANDS_M, Dictator, Side as XSide, exchange};
    if !feat.range {
        return [None, None];
    }
    let x = exchange(
        &XSide {
            d: a.doctrine,
            n: a.n as f64,
        },
        &XSide {
            d: b.doctrine,
            n: b.n as f64,
        },
        feat,
    );
    let m = BANDS_M[x.band.min(BANDS_M.len() - 1)];
    match x.dictator {
        Dictator::A => [Some(m), None],
        Dictator::B => [None, Some(m)],
        Dictator::Nobody => [None, None],
    }
}

/// **How far from its primary a ship holds**, metres: the dictated `band`
/// when it has one -- a kiter flies away from a closing brawler to keep it,
/// so it asks for the band itself, within the reach of its weapon and the
/// lock range -- else [`ORBIT_SHARE`] of its reach, as a brawler closes.
pub fn hold_distance(reach: f64, lock_range: f64, band: Option<f64>) -> f64 {
    let mut hold = match band {
        Some(b) => b.min(reach * KITE_SHARE),
        None => reach * ORBIT_SHARE,
    };
    if lock_range > 0.0 {
        hold = hold.min(lock_range * LOCK_SHARE);
    }
    hold.max(ORBIT_MIN_M)
}

pub fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let m = s.len() / 2;
    if s.len() % 2 == 1 {
        s[m]
    } else if s[m - 1].is_infinite() || s[m].is_infinite() {
        if s[m - 1] == s[m] { s[m] } else { s[m - 1] }
    } else {
        (s[m - 1] + s[m]) / 2.0
    }
}

/// `P(X >= k)` for `X ~ Binomial(n, 1/2)`: the chance a coin-flip engine
/// gives the counter `k` wins of `n` decisive fights.
pub fn binomial_tail(k: usize, n: usize) -> f64 {
    if k == 0 {
        return 1.0;
    }
    if k > n {
        return 0.0;
    }
    // log-space so n up to a few hundred does not overflow.
    let mut ln_fact = vec![0.0_f64; n + 1];
    for i in 1..=n {
        ln_fact[i] = ln_fact[i - 1] + (i as f64).ln();
    }
    (k..=n)
        .map(|x| {
            (ln_fact[n] - ln_fact[x] - ln_fact[n - x] - n as f64 * std::f64::consts::LN_2).exp()
        })
        .sum::<f64>()
        .min(1.0)
}

/// **The tally of one matchup** over its skirmishes, from the point of view
/// of the side the matrix predicted would win.
#[derive(Debug, Clone, PartialEq)]
pub struct Tally {
    pub predicted: Side,
    pub n: usize,
    pub wins_predicted: usize,
    pub wins_other: usize,
    pub draws: usize,
    /// Per fight, ISK the predicted side destroyed over ISK the other did;
    /// when neither side lost a ship (a timeout), the damage the predicted
    /// side dealt over the other's, and the fight is counted in `by_damage`.
    pub isk_ratio: Vec<f64>,
    pub by_damage: usize,
    pub secs: Vec<f64>,
}

impl Tally {
    pub fn of(outcomes: &[Outcome], predicted: Side) -> Tally {
        let mut t = Tally {
            predicted,
            n: outcomes.len(),
            wins_predicted: 0,
            wins_other: 0,
            draws: 0,
            isk_ratio: Vec::new(),
            by_damage: 0,
            secs: Vec::new(),
        };
        for o in outcomes {
            match o.winner {
                Some(w) if w == predicted => t.wins_predicted += 1,
                Some(_) => t.wins_other += 1,
                None => t.draws += 1,
            }
            let (x, y) = (o.destroyed_by(predicted), o.destroyed_by(predicted.other()));
            let ratio = |x: f64, y: f64| {
                if x == 0.0 && y == 0.0 {
                    1.0
                } else if y == 0.0 {
                    f64::INFINITY
                } else {
                    x / y
                }
            };
            if x == 0.0 && y == 0.0 && o.dealt != [0.0; 2] {
                t.by_damage += 1;
                t.isk_ratio.push(ratio(
                    o.dealt[predicted.ix()],
                    o.dealt[predicted.other().ix()],
                ));
            } else {
                t.isk_ratio.push(ratio(x, y));
            }
            t.secs.push(o.secs);
        }
        t
    }

    /// The predicted side's share of the decisive fights.
    pub fn win_rate(&self) -> f64 {
        let d = self.wins_predicted + self.wins_other;
        if d == 0 {
            0.5
        } else {
            self.wins_predicted as f64 / d as f64
        }
    }

    /// Chance of at least this many predicted wins by luck alone.
    pub fn p_chance(&self) -> f64 {
        binomial_tail(self.wins_predicted, self.wins_predicted + self.wins_other)
    }

    /// G8's per-matchup test: the counter's median ISK ratio is above 1.
    pub fn counter_works(&self) -> bool {
        median(&self.isk_ratio) > 1.0
    }
}

/// `[arena] skirmish ...`: one fight.
pub fn skirmish_line(a: &str, b: &str, seed: u64, o: &Outcome) -> String {
    format!(
        "[arena] skirmish a={a} b={b} seed={seed} winner={} n_a={} n_b={} lost_a={} lost_b={} \
         isk_lost_a={:.0} isk_lost_b={:.0} dealt_a={:.0} dealt_b={:.0} shots_a={} shots_b={} \
         hits_a={} hits_b={} secs={:.1}{}",
        o.winner.map_or("none", Side::name),
        o.fielded[0],
        o.fielded[1],
        o.lost[0],
        o.lost[1],
        o.isk_lost[0],
        o.isk_lost[1],
        o.dealt[0],
        o.dealt[1],
        o.shots[0],
        o.shots[1],
        o.hits[0],
        o.hits[1],
        o.secs,
        if o.timed_out { " timeout=1" } else { "" }
    )
}

/// `[arena] pair ...`: one matchup, `a` the doctrine and `b` the counter the
/// matrix named for it (`er` is `b`'s score against `a`, so `predicted=B`).
pub fn pair_line(a: &str, b: &str, er: f64, range: bool, t: &Tally) -> String {
    format!(
        "[arena] pair a={a} b={b} predicted={} er={er:+.2} range={} n={} wins_predicted={} \
         wins_other={} draws={} win_rate={:.2} p_chance={:.3} isk_ratio_median={} by_damage={} \
         secs_median={:.1}",
        t.predicted.name(),
        if range { "on" } else { "off" },
        t.n,
        t.wins_predicted,
        t.wins_other,
        t.draws,
        t.win_rate(),
        t.p_chance(),
        fmt_ratio(median(&t.isk_ratio)),
        t.by_damage,
        median(&t.secs),
    )
}

fn fmt_ratio(x: f64) -> String {
    if x.is_infinite() {
        "inf".to_owned()
    } else if x.is_nan() {
        "nan".to_owned()
    } else {
        format!("{x:.2}")
    }
}

/// `[arena] summary ...`: the whole run, G8's number.
pub fn summary_line(tallies: &[Tally], feat: Features) -> String {
    let medians: Vec<f64> = tallies.iter().map(|t| median(&t.isk_ratio)).collect();
    let works = tallies.iter().filter(|t| t.counter_works()).count();
    let fights: usize = tallies.iter().map(|t| t.n).sum();
    format!(
        "[arena] summary pairs={} fights={fights} range={} counter_works={works} \
         median_of_medians={}",
        tallies.len(),
        if feat.range { "on" } else { "off" },
        fmt_ratio(median(&medians)),
    )
}

// ---- the engine run -------------------------------------------------------

/// What the harness needs to start a skirmish.
#[derive(Clone)]
pub struct Rig {
    /// The tables the server runs on (typedata with the fits' parts).
    pub content: Arc<Content>,
    pub sep_m: f64,
    pub limit: Duration,
    /// The matrix's features: with `range` the side that dictates the range
    /// holds the band the exchange gives it (ENGINE-FIX).
    pub features: Features,
}

impl Rig {
    pub fn new(content: Arc<Content>) -> Rig {
        Rig {
            content,
            sep_m: START_SEP_M,
            limit: Duration::from_secs(LIMIT_S),
            features: Features::default(),
        }
    }
}

static TRACE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `--trace`: print what the harness did (deaths, refused calls).
pub fn set_trace(on: bool) {
    TRACE.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// A line on stdout when tracing is on: what the harness did.
fn trace(line: &str) {
    if TRACE.load(std::sync::atomic::Ordering::Relaxed) {
        println!("[arena] trace {line}");
    }
}

/// How a fitted module is driven.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Use {
    /// A targeted offensive effect (guns, launchers, ewar, neuts): the
    /// side's primary.
    Enemy,
    /// A targeted friendly effect (remote repair): a ship-mate.
    Friend,
    /// An effect on the ship itself (hardener, local repair, afterburner,
    /// a smartbomb: it needs no target and lands on whatever is in reach).
    Own,
    /// A module the harness never runs: a micro jump drive or a cloak
    /// would take the ship out of the fight; the doctrine reads their
    /// effect on who can be held (`doctrine_exchange`), not on the arena.
    Idle,
}

#[derive(Debug, Clone, Copy)]
struct Mod {
    id: i64,
    effect: (&'static str, i64),
    uses: Use,
    /// The target it runs on now.
    on: Option<i64>,
    /// A stop was asked and the module has not started again.
    stopping: bool,
    /// Where it sits, and the round it was loaded with (a gun).
    flag: i64,
    round: Option<i64>,
}

struct Pilot {
    conn: Conn,
    side: Side,
    ship: i64,
    /// `seed/name`, for the trace.
    tag: String,
    cost: f64,
    reach: f64,
    lock_range: f64,
    /// The band this side holds, when it dictates the range.
    band: Option<f64>,
    mods: Vec<Mod>,
    /// The enemy ship locked and fired on.
    primary: Option<i64>,
    /// When the guns were last looked at, and the reloads made.
    looked: Option<Instant>,
    reloads: u32,
    /// Ships a lock was asked for (the primary and the next in line).
    asked: Vec<i64>,
    dead: bool,
}

fn gm(c: &mut Conn, line: &str) -> Result<(), String> {
    gm_reply(c, line).map(|_| ())
}

/// [`gm`], and what the command answered (a refusal is an answer, not an
/// error: `/giveskill` of a skill the table lacks says so in its text).
fn gm_reply(c: &mut Conn, line: &str) -> Result<Option<PyValue>, String> {
    let reply = c
        .call(
            "slash",
            "SlashCmd",
            vec![PyValue::Bytes(line.as_bytes().to_vec())],
        )
        .map_err(|e| format!("{line}: {e}"))?;
    // Each creation pushes two notifications; a reply that waits behind more
    // than 32 unread frames is given up on, so read what is there.
    settle(c);
    Ok(reply.value())
}

/// **The bundle's skills into the arena's content** (DAMAGE-GAP): the sandbox
/// fixture knows a handful of skills, so `/giveskill` of every other one the
/// doctrine lists answered "no such skill" (unread) and each arena pilot flew
/// without its missile, gunnery and support skills -- flight time, explosion
/// radius and velocity, damage -- while the paper assumed them. `true` when
/// `dir` has a `types/skills.tsv` and it was installed.
pub fn install_bundle_skills(content: &mut Content, dir: &std::path::Path) -> Result<bool, String> {
    let file = dir.join("types/skills.tsv");
    if !file.is_file() {
        return Ok(false);
    }
    eve_server::skill::override_in(content, &file).map(|_| true)
}

/// The text of a `/giveskill` answer that says the skill was not given: the
/// arena flew every pilot unskilled for want of one (DAMAGE-GAP).
fn skill_refusal(reply: &Option<PyValue>) -> Option<String> {
    let text = match reply {
        Some(PyValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
        Some(PyValue::Unicode(s)) => s.clone(),
        other => format!("{other:?}"),
    };
    (text.contains("no such skill") || text.contains("no skills") || text.contains("no character"))
        .then(|| text.chars().take(80).collect())
}

/// Read the notifications a call pushed, until the line is quiet.
fn settle(c: &mut Conn) {
    for _ in 0..20 {
        if c.pump(Duration::from_millis(40))
            .map_or(true, |e| e.is_empty())
        {
            break;
        }
    }
}

fn hangar_of(c: &mut Conn, type_id: i64) -> Result<Vec<ItemRow>, String> {
    Ok(c.hangar_items()
        .map_err(|e| format!("hangar: {e}"))?
        .into_iter()
        .filter(|r| r.type_id == type_id)
        .collect())
}

/// **Build one ship the ordinary way**: skills, hull, parts, `FitAuto`,
/// online, rounds. Leaves the pilot docked in it. The driven modules.
fn outfit(c: &mut Conn, content: &Content, fit: &RoleFit) -> Result<Vec<Mod>, String> {
    for &(skill, level) in &fit.skills {
        let reply = gm_reply(c, &format!("/giveskill {skill} {level}"))?;
        // A skill the bundle's table lacks (a few type rows are skills the
        // table does not list) is told, not fatal; a table that lacks the
        // whole set is the fixture's, and the arena loads the bundle's.
        if let Some(why) = skill_refusal(&reply) {
            trace(&format!("/giveskill {skill} {level}: {why}"));
        }
    }
    gm(c, &format!("/createitem {}", fit.hull))?;
    let hull = hangar_of(c, fit.hull)?
        .into_iter()
        .find(|r| r.category_id == crate::acquire::CATEGORY_SHIP)
        .ok_or_else(|| format!("no hull {} in the hangar", fit.hull))?;
    c.board(hull.item_id).map_err(|e| format!("board: {e}"))?;
    // One item per module: `/createitem <type> <n>` makes a stack of n and
    // `FitAuto` fits the stack as ONE module, so every ship went to sea with
    // one gun and one launcher (ENGINE-FIX).
    for &(m, n) in &fit.modules {
        for _ in 0..n {
            gm(c, &format!("/createitem {m}"))?;
        }
    }
    let all = c.hangar_items().map_err(|e| format!("hangar: {e}"))?;
    let ids = fit_plan::fit_ids(fit, &all, &[]);
    // A few at a time, reading what each move pushes: a reply that waits
    // behind more than 32 frames is given up on, and ten modules move more.
    let mut fitted = Vec::new();
    for chunk in ids.chunks(3) {
        fitted.extend(c.fit_auto(chunk).map_err(|e| format!("fit: {e}"))?);
        settle(c);
    }
    trace(&format!(
        "fit wants {:?}, asked {} items, fitted {}",
        fit.modules,
        ids.len(),
        fitted.len()
    ));
    for r in &fitted {
        if !c.is_online(r.item_id).map_err(|e| format!("online: {e}"))? {
            c.set_online(r.item_id)
                .map_err(|e| format!("online {}: {e}", r.type_id))?;
        }
    }
    let station = c.session().station_id().ok_or("not docked")?;
    // Rounds into the guns, the first charge the gun takes.
    let guns: Vec<(i64, i64, i64)> = fitted
        .iter()
        .filter(|r| {
            crate::modules::find(Some(content), r.type_id)
                .is_some_and(|(t, _)| t.hardpoint.is_some())
        })
        .map(|r| (r.item_id, r.flag_id, r.type_id))
        .collect();
    let mut round_of: std::collections::BTreeMap<i64, i64> = Default::default();
    for &(gun, flag, _) in &guns {
        let mut loaded = None;
        for &(round, n) in &fit.charges {
            let qty = i64::from(n).clamp(1, ROUNDS_CAP);
            gm(c, &format!("/createitem {round} {qty}"))?;
            let ids: Vec<i64> = hangar_of(c, round)?.iter().map(|r| r.item_id).collect();
            if c.load_ammo(&[(gun, flag)], &ids, round, station).is_ok() {
                loaded = Some(round);
                break;
            }
        }
        round_of.insert(
            gun,
            loaded.ok_or_else(|| format!("no round loads into gun {gun}"))?,
        );
    }
    // What the guns did not take goes in the hold, for reloads in space.
    for round in round_of
        .values()
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
    {
        for stack in hangar_of(c, round)? {
            c.load_cargo(stack.item_id, None)
                .map_err(|e| format!("cargo: {e}"))?;
        }
    }
    let mut mods = Vec::new();
    for r in &fitted {
        let Some((t, _)) = crate::modules::find(Some(content), r.type_id) else {
            continue;
        };
        let Some(e) = t.effect else { continue };
        if e.effect_id == eve_server::modules::EFFECT_ONLINE {
            continue;
        }
        // A launcher's `useMissiles` is an activation effect in the table
        // but needs a target like any weapon: a hardpoint module is always
        // aimed at the enemy.
        let uses = if t.hardpoint.is_some() {
            Use::Enemy
        } else if e.effect_id == eve_server::dogma::effect::MICRO_JUMP_DRIVE
            || eve_server::cloak::is_cloak_effect(e.effect_id)
        {
            Use::Idle
        } else {
            match (
                e.category == eve_server::modules::CATEGORY_TARGET,
                e.is_offensive,
            ) {
                (true, true) => Use::Enemy,
                (true, false) => Use::Friend,
                (false, _) => Use::Own,
            }
        };
        mods.push(Mod {
            id: r.item_id,
            effect: (e.name, e.effect_id),
            uses,
            on: None,
            stopping: false,
            flag: r.flag_id,
            round: round_of.get(&r.item_id).copied(),
        });
    }
    Ok(mods)
}

fn connect(addr: SocketAddr, name: &str) -> Result<Conn, String> {
    let mut c = Conn::connect(addr, Duration::from_secs(10)).map_err(|e| e.to_string())?;
    c.handshake_as(DEV_TOKEN).map_err(|e| e.to_string())?;
    c.select_character_named(name).map_err(|e| e.to_string())?;
    Ok(c)
}

/// A skirmish's roster entry before the server runs.
struct Slot<'a> {
    side: Side,
    name: String,
    fit: &'a RoleFit,
    character: i64,
    ship: i64,
}

/// **One skirmish** in a server of its own. `seed` draws the separation's
/// jitter and each side's focus order, so a matchup repeated over seeds
/// samples the engine rather than replaying one fight.
pub fn skirmish(rig: &Rig, a: Fleet<'_>, b: Fleet<'_>, seed: u64) -> Result<Outcome, String> {
    let mut rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let rosters = [roster(a.doctrine, a.n), roster(b.doctrine, b.n)];
    if rosters.iter().any(Vec::is_empty) {
        return Err("a doctrine with no roles".to_owned());
    }
    let mut slots: Vec<Slot<'_>> = Vec::new();
    for (s, side) in [Side::A, Side::B].into_iter().enumerate() {
        for (i, fit) in rosters[s].iter().enumerate() {
            let k = slots.len() as i64;
            slots.push(Slot {
                side,
                name: format!("Arena{}{}", side.name(), i + 1),
                fit,
                character: FIRST_CHARACTER + k,
                ship: FIRST_SHIP + k,
            });
        }
    }

    let mut config = Config::new("127.0.0.1:0".parse().map_err(|_| "addr")?);
    config.concord = false;
    config.sandbox.characters = slots
        .iter()
        .map(|s| eve_server::model::Character {
            character_id: s.character,
            name: s.name.clone(),
            ship_id: s.ship,
            corporation_id: 98_900_000 + s.side.ix() as i64,
            balance: 1.0e9,
            ..eve_server::model::Character::default()
        })
        .collect();
    config.content = Some(rig.content.clone());
    let server = Arc::new(Server::bind(config).map_err(|e| format!("bind: {e}"))?);
    let addr = server.local_addr().map_err(|e| e.to_string())?;
    let runner = {
        let server = server.clone();
        std::thread::spawn(move || {
            let _ = server.run();
        })
    };
    let bands = bands(a, b, rig.features);
    let result = fight(rig, &slots, addr, &mut rng, seed, bands);
    // Stop the server, and wake its accept loop.
    server.stop().request("arena skirmish over");
    let _ = std::net::TcpStream::connect(addr);
    let _ = runner.join();
    result
}

fn fight(
    rig: &Rig,
    slots: &[Slot<'_>],
    addr: SocketAddr,
    rng: &mut u64,
    seed: u64,
    bands: [Option<f64>; 2],
) -> Result<Outcome, String> {
    let content = &*rig.content;
    // Build and undock everyone, then place the fleets.
    let mut pilots: Vec<Pilot> = Vec::new();
    for s in slots {
        let mut c = connect(addr, &s.name).map_err(|e| format!("{}: {e}", s.name))?;
        c.keep_pushes();
        let mods = outfit(&mut c, content, s.fit).map_err(|e| format!("{}: {e}", s.name))?;
        let ship = c
            .session()
            .ship_id()
            .ok_or_else(|| format!("{}: no ship", s.name))?;
        trace(&format!(
            "{seed}/{} paper: dps={:.0} ehp={:.0} speed={:.0} sig={:.0} reach={:.0} lock={:.0}",
            s.name,
            s.fit.profile.total_dps(),
            s.fit.profile.ehp().iter().sum::<f64>() / 4.0,
            s.fit.profile.speed,
            s.fit.profile.sig,
            s.fit.profile.reach(),
            s.fit.profile.lock_range,
        ));
        trace(&format!(
            "{seed}/{} fitted: {}",
            s.name,
            mods.iter()
                .map(|m| format!("{}:{:?}", m.effect.0, m.uses))
                .collect::<Vec<_>>()
                .join(" ")
        ));
        pilots.push(Pilot {
            conn: c,
            side: s.side,
            ship,
            tag: format!("{seed}/{}", s.name),
            cost: s.fit.cost,
            reach: s.fit.profile.reach(),
            lock_range: s.fit.profile.lock_range,
            band: bands[s.side.ix()],
            mods,
            primary: None,
            looked: None,
            reloads: 0,
            asked: Vec::new(),
            dead: false,
        });
    }
    let jitter = 1.0 + SEP_JITTER * (2.0 * unit(xorshift(rng)) - 1.0);
    let sep_km = rig.sep_m * jitter / 1_000.0;
    for p in &mut pilots {
        p.conn.undock().map_err(|e| format!("undock: {e}"))?;
        p.conn
            .enter_space(SYSTEM)
            .map_err(|e| format!("enter space: {e}"))?;
        let km = ARENA_KM + if p.side == Side::B { sep_km } else { 0.0 };
        gm(&mut p.conn, &format!("/tp {km}"))?;
    }
    // Each side's focus order: a draw per ship, so seeds differ.
    let mut order: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
    for (i, p) in pilots.iter().enumerate() {
        order[p.side.ix()].push(i);
    }
    for o in &mut order {
        for i in (1..o.len()).rev() {
            let j = (xorshift(rng) % (i as u64 + 1)) as usize;
            o.swap(i, j);
        }
    }
    // Ships own-effects run from the start.
    for p in &mut pilots {
        for m in p.mods.iter_mut().filter(|m| m.uses == Use::Own) {
            let _ = p.conn.activate(m.id, m.effect, None, FOREVER);
        }
    }

    let began = Instant::now();
    let mut out = Outcome {
        winner: None,
        fielded: [order[0].len(), order[1].len()],
        lost: [0, 0],
        isk_lost: [0.0, 0.0],
        secs: 0.0,
        timed_out: false,
        dealt: [0.0; 2],
        shots: [0; 2],
        hits: [0; 2],
    };
    let mut last_look = 0;
    loop {
        // Deaths: a pilot whose session ship is no longer the hull is out.
        for p in &mut pilots {
            if p.dead {
                continue;
            }
            // The server carries a ship forward when its pilot calls: a
            // throttle call is the heartbeat (and the throttle is full).
            let _ = p.conn.set_speed_fraction(1.0);
            let _ = p.conn.pump(Duration::from_millis(5));
            for (name, args) in p.conn.take_pushes() {
                if name == "OnDamageMessage"
                    && let Some((dmg, hit)) = outgoing_shot(&args)
                {
                    trace(&format!("{} shot dmg={dmg:.1} hit={hit}", p.tag));
                    out.dealt[p.side.ix()] += dmg;
                    out.shots[p.side.ix()] += 1;
                    out.hits[p.side.ix()] += u32::from(hit);
                }
            }
            if p.conn.session().ship_id() != Some(p.ship) {
                p.dead = true;
                trace(&format!(
                    "t={:.1} {} lost ship {}",
                    began.elapsed().as_secs_f64(),
                    p.tag,
                    p.ship
                ));
                out.lost[p.side.ix()] += 1;
                out.isk_lost[p.side.ix()] += p.cost;
            }
        }
        let alive = |side: Side| order[side.ix()].iter().any(|&i| !pilots[i].dead);
        let (a_up, b_up) = (alive(Side::A), alive(Side::B));
        if !(a_up && b_up) {
            out.winner = match (a_up, b_up) {
                (true, false) => Some(Side::A),
                (false, true) => Some(Side::B),
                _ => None,
            };
            break;
        }
        if began.elapsed() >= rig.limit {
            out.timed_out = true;
            break;
        }
        for i in 0..pilots.len() {
            if pilots[i].dead {
                continue;
            }
            let side = pilots[i].side;
            let foes: Vec<i64> = order[side.other().ix()]
                .iter()
                .filter(|&&j| !pilots[j].dead)
                .map(|&j| pilots[j].ship)
                .collect();
            let mates: Vec<i64> = order[side.ix()]
                .iter()
                .filter(|&&j| j != i && !pilots[j].dead)
                .map(|&j| pilots[j].ship)
                .collect();
            drive(&mut pilots[i], &foes, &mates, i);
        }
        if TRACE.load(std::sync::atomic::Ordering::Relaxed)
            && began.elapsed().as_secs() / 10 > last_look
        {
            last_look = began.elapsed().as_secs() / 10;
            for p in pilots.iter_mut().filter(|p| !p.dead) {
                let target = p.primary;
                let w = p.conn.world_now();
                let me = w.ego_ball().map(|e| e.motion.position);
                let tether = w.ego_ball().and_then(|e| e.motion.tether());
                let speed = w.ego_ball().map(|e| {
                    let v = e.motion.velocity;
                    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
                });
                let them = target.and_then(|t| {
                    w.balls()
                        .find(|b| b.ball_id == t)
                        .map(|b| b.motion.position)
                });
                let d = me
                    .zip(them)
                    .map(|(a, b)| eve_server::space_state::distance(a, b));
                trace(&format!(
                    "t={:.0} {} primary={target:?} distance={d:?} speed={speed:?} tether={tether:?}",
                    began.elapsed().as_secs_f64(),
                    p.tag
                ));
            }
        }
        std::thread::sleep(Duration::from_millis(TICK_MS));
    }
    out.secs = began.elapsed().as_secs_f64();
    Ok(out)
}

/// **Keep the guns shooting**: a module that stopped by itself (its target
/// died, or its magazine ran dry) is marked idle, and an empty gun is
/// reloaded from the hold, as a pilot does -- every [`RELOAD_LOOK_S`].
fn reload(p: &mut Pilot) {
    if p.looked
        .is_some_and(|t| t.elapsed().as_secs() < RELOAD_LOOK_S)
    {
        return;
    }
    p.looked = Some(Instant::now());
    let Ok(ship) = p.conn.session().ship_id().ok_or(()) else {
        return;
    };
    for i in 0..p.mods.len() {
        let m = p.mods[i];
        if m.uses != Use::Enemy {
            continue;
        }
        let running = p
            .conn
            .active_effects(m.id)
            .is_ok_and(|e| e.contains(&m.effect.1));
        if !running {
            p.mods[i].on = None;
        }
        let Some(round) = m.round else { continue };
        let left = p.conn.loaded_rounds(ship, m.flag, round).unwrap_or(1);
        if left > 0 {
            continue;
        }
        trace(&format!("{} gun {} is dry", p.tag, m.id));
        // Dry: the module may still be cycling on nothing. Reload, then
        // start it again.
        p.mods[i].on = None;
        let held: Vec<i64> = p
            .conn
            .cargo()
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.type_id == round)
            .map(|r| r.item_id)
            .collect();
        if p.conn
            .load_ammo(&[(m.id, m.flag)], &held, round, ship)
            .is_ok()
        {
            p.reloads += 1;
            let _ = p.conn.deactivate(m.id, m.effect.0);
            p.mods[i].stopping = true;
        } else {
            trace(&format!("{} reload {}: none held", p.tag, m.id));
        }
    }
}

/// **One of this pilot's own shots** in an `OnDamageMessage`: its damage
/// and whether it hit (`attackType` `me`; a miss has `hitQuality` 0).
fn outgoing_shot(args: &[PyValue]) -> Option<(f64, bool)> {
    let PyValue::Dict(pairs) = args.first()? else {
        return None;
    };
    let get = |key: &str| {
        pairs
            .iter()
            .find(|(_, k)| matches!(k, PyValue::Bytes(b) if b == key.as_bytes()))
            .map(|(v, _)| v)
    };
    if !matches!(get("attackType"), Some(PyValue::Bytes(b)) if b == b"me") {
        return None;
    }
    let damage = match get("damage") {
        Some(PyValue::Real(x)) => *x,
        Some(PyValue::Int(x)) => *x as f64,
        _ => 0.0,
    };
    let hit = matches!(get("hitQuality"), Some(PyValue::Int(q)) if *q > 0);
    Some((damage, hit))
}

/// **One pilot's turn**: keep the side's primary locked and shot at, a
/// friend repaired, the ship orbiting at the fit's reach.
fn drive(p: &mut Pilot, foes: &[i64], mates: &[i64], index: usize) {
    let Some(&primary) = foes.first() else { return };
    reload(p);
    // Lock the next ships in line ahead of time, as a fleet does: a lock
    // takes seconds, and the primary changes when one dies.
    for &foe in foes.iter().take(PRELOCK) {
        if !p.asked.contains(&foe) && foe != primary {
            let _ = p.conn.lock_target(foe);
            p.asked.push(foe);
        }
    }
    let idle = p
        .mods
        .iter()
        .any(|m| m.uses == Use::Enemy && m.on.is_none());
    if p.primary != Some(primary) || idle {
        let locked = p.conn.lock_target(primary);
        if let Err(e) = &locked {
            trace(&format!("{} lock {primary}: {e}", p.tag));
        }
        if locked.is_err() {
            // Out of lock range: close in and try again next turn.
            let _ = p.conn.follow(primary, ORBIT_MIN_M);
            let _ = p.conn.set_speed_fraction(1.0);
            return;
        }
        let mut all_on = true;
        for m in p.mods.iter_mut().filter(|m| m.uses == Use::Enemy) {
            if m.on == Some(primary) {
                continue;
            }
            // A running module stops at the end of its cycle; the new
            // start may be refused until then and is tried again next turn.
            let old = m.on.take();
            // A module whose target is dead has already stopped (the server
            // ends it with the lock); asking for a stop would only make it
            // finish the cycle and refuse the restart (ENGINE-FIX).
            let asked_stop = old.is_some_and(|t| foes.contains(&t));
            if asked_stop {
                m.stopping = true;
                let r = p.conn.deactivate(m.id, m.effect.0);
                trace(&format!(
                    "{} deactivate {}: {:?}",
                    p.tag,
                    m.id,
                    r.map(|o| o.refusal())
                ));
            }
            match p.conn.activate(m.id, m.effect, Some(primary), FOREVER) {
                Ok(_) => {
                    m.on = Some(primary);
                    m.stopping = false;
                }
                // Its start was missed the first time: it is running.
                Err(e) if !asked_stop && !m.stopping && e.to_string().contains("AlreadyActive") => {
                    m.on = Some(primary);
                }
                Err(e) => {
                    all_on = false;
                    trace(&format!("{} module {}: {e}", p.tag, m.id));
                }
            }
        }
        // Inside the gun's reach and inside the lock range, or the target
        // is lost.
        let hold = hold_distance(p.reach, p.lock_range, p.band);
        let o = p.conn.orbit(primary, hold);
        // Full throttle: a fresh ship's speed fraction is zero.
        let _ = p.conn.set_speed_fraction(1.0);
        trace(&format!(
            "{} orbit {primary} at {hold:.0}: {:?}",
            p.tag,
            o.map(|o| o.refusal())
        ));
        if all_on {
            p.primary = Some(primary);
        }
    }
    if mates.is_empty() {
        return;
    }
    let friend = mates[index % mates.len()];
    for m in p.mods.iter_mut().filter(|m| m.uses == Use::Friend) {
        if m.on == Some(friend) {
            continue;
        }
        if m.on.is_some() {
            let _ = p.conn.deactivate(m.id, m.effect.0);
        }
        let _ = p.conn.lock_target(friend);
        m.on = p
            .conn
            .activate(m.id, m.effect, Some(friend), FOREVER)
            .ok()
            .map(|_| friend);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skill_the_table_lacks_is_a_refusal_and_a_grant_is_not() {
        let said = |t: &str| Some(PyValue::Bytes(t.as_bytes().to_vec()));
        assert!(skill_refusal(&said("no such skill '990000001' -- known: A, B")).is_some());
        assert!(skill_refusal(&said("no skills for this character")).is_some());
        assert_eq!(
            skill_refusal(&said("Invented Skill is now level 4 (9 SP).")),
            None
        );
    }

    #[test]
    fn a_directory_with_no_skills_file_installs_nothing() {
        let dir = std::env::temp_dir().join(format!("eve-arena-noskills-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut content = Content::fixture();
        assert_eq!(install_bundle_skills(&mut content, &dir), Ok(false));
        let _ = std::fs::remove_dir_all(&dir);
    }
    use crate::doctrine::testkit::*;
    use crate::doctrine_exchange::matrix;

    fn out(winner: Option<Side>, isk: [f64; 2]) -> Outcome {
        Outcome {
            winner,
            fielded: [3, 3],
            lost: [1, 2],
            isk_lost: isk,
            secs: 60.0,
            timed_out: false,
            dealt: [0.0; 2],
            shots: [0; 2],
            hits: [0; 2],
        }
    }

    #[test]
    fn a_dictated_band_is_held_inside_the_reach_and_the_lock_and_a_brawler_closes() {
        // A kiter with a long reach holds the band; a short reach or a
        // short lock cuts it.
        assert_eq!(hold_distance(60_000.0, 90_000.0, Some(15_000.0)), 15_000.0);
        assert_eq!(
            hold_distance(10_000.0, 90_000.0, Some(15_000.0)),
            10_000.0 * KITE_SHARE
        );
        assert_eq!(
            hold_distance(60_000.0, 10_000.0, Some(15_000.0)),
            10_000.0 * LOCK_SHARE
        );
        // No band: a share of the reach, never under the floor.
        assert_eq!(hold_distance(10_000.0, 0.0, None), 10_000.0 * ORBIT_SHARE);
        assert_eq!(hold_distance(100.0, 0.0, None), ORBIT_MIN_M);
    }

    #[test]
    fn nobody_holds_a_band_with_the_range_term_off() {
        let d = doc(brick(), 100.0);
        let f = Fleet { doctrine: &d, n: 2 };
        assert_eq!(bands(f, f, Features { range: false }), [None, None]);
        // Equal doctrines: nobody dictates.
        assert_eq!(bands(f, f, Features::default()), [None, None]);
    }

    #[test]
    fn the_roster_follows_shares_and_leads_with_damage() {
        let d = with_role(brick_doc(), Role::Logi, 0.2, logi_ship(), 100.0);
        let r = roster(&d, 10);
        assert_eq!(r.len(), 10);
        let logi = r.iter().filter(|f| f.profile.remote_rep > 0.0).count();
        assert_eq!(logi, 2, "a fifth of ten");
        assert!(r[0].profile.remote_rep == 0.0, "damage first");
        // One ship is a damage dealer.
        assert_eq!(roster(&d, 1).len(), 1);
        assert!(roster(&d, 1)[0].profile.remote_rep == 0.0);
    }

    #[test]
    fn equal_spend_is_scaled_to_the_cap_keeping_the_ratio() {
        let cheap = doc(brick(), 100.0);
        let dear = doc(brick(), 400.0);
        let (ca, cb) = counts(&cheap, &dear);
        assert!(ca > cb);
        let (na, nb) = fielded(&cheap, &dear, 6);
        assert!(na <= 6 && nb <= 6 && na >= 1 && nb >= 1);
        assert!(na > nb, "{na} {nb}");
        let (x, y) = fielded(&cheap, &cheap, 6);
        assert_eq!(x, y);
    }

    #[test]
    fn the_counter_is_the_best_scorer_against_the_doctrine() {
        let cat = invented_catalog();
        let m = matrix(&cat, Features::default());
        for i in 0..cat.len() {
            let (j, er) = counter_of(&m, i).expect("a counter");
            assert_ne!(i, j);
            assert!((0..cat.len()).filter(|&k| k != i).all(|k| m.er[k][i] <= er));
        }
    }

    #[test]
    fn a_tally_counts_wins_draws_and_isk() {
        let fights = [
            out(Some(Side::B), [300.0, 100.0]),
            out(Some(Side::B), [200.0, 0.0]),
            out(Some(Side::A), [0.0, 100.0]),
            out(None, [100.0, 100.0]),
        ];
        let t = Tally::of(&fights, Side::B);
        assert_eq!((t.wins_predicted, t.wins_other, t.draws), (2, 1, 1));
        assert!((t.win_rate() - 2.0 / 3.0).abs() < 1e-9);
        // B destroyed 100/0=inf, 200/... A lost 300 -> B destroyed 300 of A's, A destroyed 100.
        assert_eq!(t.isk_ratio[0], 3.0);
        assert!(t.isk_ratio[1].is_infinite());
        assert_eq!(t.isk_ratio[3], 1.0);
        assert!(t.counter_works());
        let line = pair_line("x", "y", 0.5, true, &t);
        assert!(line.starts_with("[arena] pair a=x b=y predicted=B er=+0.50 range=on n=4"));
        assert!(skirmish_line("x", "y", 7, &fights[0]).contains("winner=B"));
    }

    #[test]
    fn a_timeout_is_rated_by_the_damage_dealt() {
        let mut slow = out(None, [0.0, 0.0]);
        slow.lost = [0, 0];
        slow.dealt = [100.0, 300.0];
        let t = Tally::of(&[slow], Side::B);
        assert_eq!((t.draws, t.by_damage), (1, 1));
        assert_eq!(t.isk_ratio, vec![3.0]);
        assert!(t.counter_works());
        // No damage either way is even, not a win.
        let t = Tally::of(&[out(None, [0.0, 0.0])], Side::B);
        assert_eq!((t.by_damage, t.isk_ratio.clone()), (0, vec![1.0]));
        assert!(!t.counter_works());
    }

    #[test]
    fn an_outgoing_damage_message_is_read_by_attack_type_and_quality() {
        let msg = |kind: &str, q: i64, dmg: f64| {
            PyValue::Dict(vec![
                (PyValue::Int(q), PyValue::Bytes(b"hitQuality".to_vec())),
                (PyValue::Real(dmg), PyValue::Bytes(b"damage".to_vec())),
                (
                    PyValue::Bytes(kind.as_bytes().to_vec()),
                    PyValue::Bytes(b"attackType".to_vec()),
                ),
            ])
        };
        assert_eq!(outgoing_shot(&[msg("me", 2, 42.5)]), Some((42.5, true)));
        assert_eq!(outgoing_shot(&[msg("me", 0, 0.0)]), Some((0.0, false)));
        assert_eq!(
            outgoing_shot(&[msg("otherPlayer", 2, 9.0)]),
            None,
            "a hit taken is not one dealt"
        );
        assert_eq!(outgoing_shot(&[]), None);
    }

    #[test]
    fn luck_alone_rarely_gives_nine_of_ten() {
        assert!((binomial_tail(0, 10) - 1.0).abs() < 1e-12);
        assert!((binomial_tail(5, 10) - 0.623_046_875).abs() < 1e-9);
        assert!((binomial_tail(9, 10) - 11.0 / 1024.0).abs() < 1e-9);
        assert_eq!(binomial_tail(11, 10), 0.0);
        assert!((median(&[3.0, 1.0, 2.0]) - 2.0).abs() < 1e-12);
        assert!(median(&[]).is_nan());
    }
}
