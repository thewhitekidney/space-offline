//! **The pure rules** of the offline agents (E1, R-ECON §1.5 / E12): what a
//! logged-off career produces and consumes over a span, as functions of plain
//! numbers. Nothing here reads a `Registry`; [`super::settle`] and the
//! per-career effect modules read the world, call these, and apply the
//! answer through the real wallet, market, store and ledger paths.
//!
//! **E12's rule: a settlement is never a privilege.** An agent cannot do what
//! its pilot could not, so every rate below is bounded above by the live
//! rate of the same rig (a miner's m3 an hour by its lasers' cycles and its
//! hold's trips; a ratter's kills by its guns' damage against the squad's hit
//! points and by the rounds it owns), then cut by [`EFFICIENCY`], then by the
//! [`DUTY`] share of the offline time it would be working, then by the
//! [`DAY_HOURS_CAP`] of work a game day. A bot online for its whole session
//! window and a bot settled for the rest of the day therefore never exceed
//! what the same bot could live.
//!
//! **Every constant is [A]** (policy, what a player does when they are away
//! from the keyboard), each with the falsifier named at the constant. The
//! `[settle]` line and the daily `[econ]` line are where they are read.

/// A game hour in seconds. The agents think in game time; the caller turns
/// wall spans into game spans with `GameClock::elapsed`.
pub const HOUR_S: f64 = 3_600.0;

/// The share of an offline span a career is at work **[A]**. The rest is
/// the pilot's own life: sleep, other games, the timetable's gaps between
/// sessions. Falsified by `[settle]` ISK per bot-day beating the live
/// ISK per bot-day of the same career in the `[soc]` ledger (too high).
pub const DUTY: f64 = 0.5;

/// A settled hour pays this fraction of a live hour **[A]**: no attention, no
/// reaction to a ganker or a price move, longer trips. Falsified as
/// [`DUTY`] is.
pub const EFFICIENCY: f64 = 0.6;

/// The most game hours of work one agent is settled for in one game day
/// **[A]**: under the longest live window (`SESSION_HOURS.1`, 10 h), so a
/// bot's live and settled hours together stay near a player's day.
/// Falsified by an agent's `[settle]` totals beating a live bot's
/// `[soc]` income per day.
pub const DAY_HOURS_CAP: f64 = 8.0;

/// The longest span one settlement stands for, game hours **[A]**: a
/// character offline for a week was not working for a week. The excess is
/// forfeited, not banked. Falsified by an owner who wants a returning bot to
/// find a week of work done.
pub const MAX_SPAN_H: f64 = 24.0;

/// The shortest span worth settling, game seconds **[A]**: below it the
/// stacks would round to nothing and the wallet and the log would churn.
pub const MIN_SPAN_S: f64 = 900.0;

/// Whether `x` is a usable positive amount: finite and above zero. The rules
/// take their inputs from tables and wallets, so a NaN or an infinity is
/// refused here, once, rather than at every comparison.
pub fn positive(x: f64) -> bool {
    x.is_finite() && x > 0.0
}

/// The work, in game hours, of an offline span of `offline_s` game seconds
/// when `used_today_h` of today's [`DAY_HOURS_CAP`] is already spent.
pub fn active_hours(offline_s: f64, used_today_h: f64) -> f64 {
    if !offline_s.is_finite() || offline_s <= 0.0 {
        return 0.0;
    }
    let span_h = offline_s.min(MAX_SPAN_H * HOUR_S) / HOUR_S;
    let left = (DAY_HOURS_CAP - used_today_h.max(0.0)).max(0.0);
    (span_h * DUTY).min(left)
}

// ---- mining -------------------------------------------------------------------

/// What a trip costs besides mining, game seconds **[A]**: undock, warp to
/// the belt, warp home, dock, unload. Falsified by a live miner's measured
/// m3 an hour (the `[soc]` ledger) far from [`mined_m3`] at full efficiency.
pub const TRIP_OVERHEAD_S: f64 = 300.0;

/// The share of a system's ore (units present in its rocks) one settlement
/// may take **[A]**, so no single agent clears a belt. The rest of the
/// bound is the field itself: a rock is taken from the field, and the field
/// is reset in full only by the daily full reset, the rollover's
/// `asteroid::on_rollover` (R-ECON §1.5: nothing is minted). Falsified
/// by a human's belt found empty after agents ran.
pub const ROCK_SHARE: f64 = 0.5;

/// What a ship mines with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MiningRig {
    /// Modules cutting at once.
    pub lasers: u32,
    /// `miningAmount`, m3 a cycle, per module.
    pub amount_m3: f64,
    /// A cycle, seconds.
    pub cycle_s: f64,
    /// Ore the hull holds, m3 (mining hold and cargo).
    pub hold_m3: f64,
}

impl MiningRig {
    /// m3 a second while cutting.
    pub fn m3_per_s(&self) -> f64 {
        if self.cycle_s > 0.0 && self.amount_m3 > 0.0 {
            f64::from(self.lasers) * self.amount_m3 / self.cycle_s
        } else {
            0.0
        }
    }
}

/// **The m3 a rig mines in `active_h` hours** of work: trips of a full hold,
/// each [`TRIP_OVERHEAD_S`] longer than it takes to fill, times
/// [`EFFICIENCY`]. Never more than the lasers cut running without stop.
pub fn mined_m3(rig: &MiningRig, active_h: f64) -> f64 {
    let rate = rig.m3_per_s();
    if rate <= 0.0 || rig.hold_m3 <= 0.0 || !positive(active_h) {
        return 0.0;
    }
    let trip_s = rig.hold_m3 / rate + TRIP_OVERHEAD_S;
    let m3 = active_h * HOUR_S * rig.hold_m3 / trip_s * EFFICIENCY;
    m3.min(active_h * HOUR_S * rate)
}

/// One rock an agent may cut: its id, ore units present, and one unit's
/// volume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RockView {
    pub id: i64,
    pub quantity: i64,
    pub unit_m3: f64,
}

/// **Which rocks give how many units**: walk `rocks` in the order given
/// (grouped by belt, rotated to the miner's own belt first, `gather::belt_first`), cutting each at
/// its own unit volume until the `m3` is spent or [`ROCK_SHARE`] of the
/// units present has been taken. Answers `(rock id, units)`; a rock is cut
/// to nothing before the next is started, as a miner finishes a rock.
pub fn cut_plan(rocks: &[RockView], m3: f64) -> Vec<(i64, i64)> {
    let present: i64 = rocks.iter().map(|r| r.quantity.max(0)).sum();
    let mut share_left = (present as f64 * ROCK_SHARE).floor() as i64;
    let mut m3_left = m3;
    let mut out = Vec::new();
    for r in rocks {
        if share_left < 1 || !positive(m3_left) {
            break;
        }
        if r.quantity < 1 || !positive(r.unit_m3) {
            continue;
        }
        let by_m3 = (m3_left / r.unit_m3 + 1e-9).floor() as i64;
        let take = by_m3.min(r.quantity).min(share_left);
        if take < 1 {
            continue;
        }
        out.push((r.id, take));
        share_left -= take;
        m3_left -= take as f64 * r.unit_m3;
    }
    out
}

// ---- fighting -----------------------------------------------------------------

/// The share of a gun's listed damage that lands **[A]**: live `combat.rs`
/// assumes "every shot hits, for the full amount" before the NPC's resists,
/// range and tracking, which this stands in for. Falsified by a live
/// ratter's measured kills an hour below [`fight`]'s at the same fit.
pub const HIT_EFFICIENCY: f64 = 0.6;

/// What one squad costs besides shooting, game seconds **[A]**: warp to the
/// belt, the wreck and the loot, the way back. Falsified as [`TRIP_OVERHEAD_S`].
pub const SQUAD_OVERHEAD_S: f64 = 150.0;

/// The most squads one settlement draws **[A]**: a bound on the loop, well
/// above [`DAY_HOURS_CAP`] of work at any real fit.
pub const MAX_SQUADS: usize = 400;

/// One squad's hit points, summed over its members, and its size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SquadStat {
    pub ehp: f64,
    pub members: usize,
}

/// What a guns' fit throws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Battery {
    /// Damage a second of every gun together, after the server's own
    /// per-shot arithmetic (`combat::volley_fitted`) and before
    /// [`HIT_EFFICIENCY`].
    pub dps: f64,
    /// Rounds fired a second by every gun together.
    pub shots_per_s: f64,
}

/// What a fight settlement did.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Fight {
    /// Squads killed.
    pub squads: usize,
    /// Rounds fired.
    pub rounds: i64,
    /// Seconds used, overhead included.
    pub seconds: f64,
    /// What stopped it.
    pub stopped: Stop,
}

/// Why a fight settlement ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stop {
    /// The work hours ran out.
    #[default]
    Time,
    /// The next squad needed more rounds than are left.
    Ammo,
    /// No damage at all: nothing was fought.
    Unarmed,
    /// [`MAX_SQUADS`].
    Cap,
}

/// **The fight, pure**: kill squads in turn, each taking `ehp / (dps x
/// HIT_EFFICIENCY)` seconds plus [`SQUAD_OVERHEAD_S`] and the rounds those
/// seconds fire, until `active_s` seconds of work or `rounds_have` rounds are
/// gone. `draw(k)` is the k-th squad (seeded by the caller). Time is then
/// scaled by [`EFFICIENCY`], i.e. a settled hour kills as many squads as
/// [`EFFICIENCY`] of a live hour would.
pub fn fight(
    b: &Battery,
    rounds_have: i64,
    active_s: f64,
    mut draw: impl FnMut(usize) -> SquadStat,
) -> (Fight, Vec<SquadStat>) {
    let mut out = Fight::default();
    let mut killed = Vec::new();
    let eff_dps = b.dps * HIT_EFFICIENCY;
    if !positive(eff_dps) || !positive(active_s) {
        out.stopped = Stop::Unarmed;
        return (out, killed);
    }
    let budget = active_s * EFFICIENCY;
    for k in 0..MAX_SQUADS {
        let s = draw(k);
        let kill_s = s.ehp.max(0.0) / eff_dps;
        let rounds = (kill_s * b.shots_per_s).ceil().max(1.0) as i64;
        let took = kill_s + SQUAD_OVERHEAD_S;
        if out.seconds + took > budget {
            out.stopped = Stop::Time;
            return (out, killed);
        }
        if out.rounds + rounds > rounds_have {
            out.stopped = Stop::Ammo;
            return (out, killed);
        }
        out.seconds += took;
        out.rounds += rounds;
        out.squads += 1;
        killed.push(s);
    }
    out.stopped = Stop::Cap;
    (out, killed)
}

// ---- building -----------------------------------------------------------------

/// How long one job should run, game seconds **[A]**: the live builder's
/// `JOB_TARGET` (two hours). The runs are that over the recipe's time per
/// run, at least one. Falsified by jobs that sit finished for hours (too
/// short) or tie a slot for a day (too long).
pub const JOB_TARGET_S: i64 = 2 * 3_600;

/// The share of the wallet a builder's materials may cost per settlement
/// **[A]**: the live builder's `WALLET_SHARE`. Falsified by builders short
/// of the install fee.
pub const BUILDER_WALLET_SHARE: f64 = 0.5;

/// The least a build must earn over its materials **[A]**, as a share of
/// them: the live margin rule's floor in the same words. Falsified by
/// builders whose ledger is negative while they build.
pub const BUILD_MARGIN: f64 = 0.05;

/// The most a builder pays for a material, as a share of its `P_ref`
/// **[A]**: the live `BUY_CAP_OF_MKT`. Falsified by builders that never
/// find materials to buy.
pub const MATERIAL_CAP_OF_REF: f64 = 1.0;

/// How many days a listed ask rests **[A]**: the live builder's `ORDER_DAYS`.
pub const LIST_DAYS: i64 = 14;

/// Whether a job worth `value` ISK of product for `cost` ISK of materials
/// clears [`BUILD_MARGIN`].
pub fn worth_building(value: f64, cost: f64) -> bool {
    value.is_finite() && cost.is_finite() && cost > 0.0 && value >= cost * (1.0 + BUILD_MARGIN)
}

/// The runs a builder plans for a recipe of `seconds` a run.
pub fn planned_runs(seconds: i64) -> i64 {
    (JOB_TARGET_S / seconds.max(1)).max(1)
}

/// The most runs, up to `planned`, for which `ok(runs)` holds, when `ok` only
/// ever turns false as runs grow (a cost, a shortfall): the live builder's
/// halving search.
pub fn most_runs(planned: i64, ok: impl Fn(i64) -> bool) -> i64 {
    let (mut lo, mut hi) = (0, planned.max(0));
    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        if ok(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

// ---- trading ------------------------------------------------------------------

/// The most of its wallet a trader puts in one run **[A]**: the live
/// trader's `WALLET_SHARE`. Falsified by traders that stall on
/// `NotEnoughMoney`.
pub const TRADER_WALLET_SHARE: f64 = 0.6;

/// Runs a settled trader makes an hour of work **[A]**: a round trip of
/// about ten minutes (`trader::TRADE_MAX_JUMPS` jumps out, across and home).
/// Falsified by `[settle]` trader ISK an hour above the live trader's.
pub const TRADER_RUNS_PER_H: f64 = 4.0;

/// The most trade-good turnover one trader settles in a game day, ISK
/// **[A]**: `TIER_A_STOCK` units a station a day at a modest base price
/// bounds the NPC side, and this is the same bound seen from the hauler.
/// Falsified by a trader that empties an NPC order every day.
pub const TRADER_TURNOVER_DAY: f64 = 60_000_000.0;

/// **The edge of the NPC trade-goods loop**: buying at `ask` x base and
/// selling at `bid` x base, the sales tax comes off the sale. Per ISK
/// spent: `bid x (1 - tax) / ask - 1`. At Accounting 0 this is negative
/// (`trader.rs`'s header: 0.92 < 0.94 of base), so an unskilled trader's
/// settlement earns nothing; the rule is told no case, it finds it.
pub fn trade_edge(ask: f64, bid: f64, tax: f64) -> f64 {
    if !positive(ask) || !positive(bid) {
        return 0.0;
    }
    bid * (1.0 - tax) / ask - 1.0
}

/// One settled trade run: ISK spent, ISK sold for, and the sales tax.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Trades {
    pub spent: f64,
    pub revenue: f64,
    pub tax: f64,
}

/// **The trader's runs**: `wallet` ISK, `active_h` hours of work and
/// `turnover_left` of the day's [`TRADER_TURNOVER_DAY`], at the loop's
/// `ask` / `bid` (x base) and the pilot's sales `tax`. No trade at a
/// non-positive edge. Each run spends [`TRADER_WALLET_SHARE`] of the wallet
/// (compounding is the next settlement's), runs are
/// [`TRADER_RUNS_PER_H`] x [`EFFICIENCY`] an hour, the whole bounded by
/// `turnover_left`.
pub fn trade_runs(
    wallet: f64,
    active_h: f64,
    turnover_left: f64,
    ask: f64,
    bid: f64,
    tax: f64,
) -> Trades {
    let edge = trade_edge(ask, bid, tax);
    if !positive(edge) || !positive(wallet) || !positive(active_h) || !positive(turnover_left) {
        return Trades::default();
    }
    let runs = (active_h * TRADER_RUNS_PER_H * EFFICIENCY).floor();
    let per_run = wallet * TRADER_WALLET_SHARE;
    let spent = (runs * per_run).min(turnover_left);
    if !positive(spent) {
        return Trades::default();
    }
    let sold = spent * bid / ask;
    Trades {
        spent,
        revenue: sold,
        tax: sold * tax,
    }
}

// ---- selling ------------------------------------------------------------------

/// The least a settled sale takes for a type with no NPC floor, as a share
/// of `P_ref` **[A]**: the bot price clamp's low end (`pricing::BOT_PRICE_MIN`
/// in `eve-bot`, which the server cannot import). Falsified as that is.
pub const SELL_FLOOR_OF_REF: f64 = 0.5;

/// The lowest price a settled sale accepts for a stack whose reference price
/// is `p_ref`: [`SELL_FLOOR_OF_REF`] of it, to the cent, never below a cent.
pub fn sell_floor(p_ref: f64) -> Option<f64> {
    if !(p_ref.is_finite() && p_ref > 0.0) {
        return None;
    }
    let f = (p_ref * SELL_FLOOR_OF_REF * 100.0).floor() / 100.0;
    (f >= 0.01).then_some(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RIG: MiningRig = MiningRig {
        lasers: 2,
        amount_m3: 10.0,
        cycle_s: 60.0,
        hold_m3: 200.0,
    };

    #[test]
    fn work_is_a_share_of_the_span_and_a_cap_a_day() {
        let day = 24.0 * HOUR_S;
        // A short span: DUTY of it.
        assert!((active_hours(4.0 * HOUR_S, 0.0) - 4.0 * DUTY).abs() < 1e-12);
        // A span past MAX_SPAN_H is cut at it before DUTY, then at the day.
        let long = active_hours(10.0 * day, 0.0);
        assert!((long - (MAX_SPAN_H * DUTY).min(DAY_HOURS_CAP)).abs() < 1e-12);
        // What the day has used comes off the cap, and never below zero.
        assert!((active_hours(day, DAY_HOURS_CAP - 1.0) - 1.0).abs() < 1e-12);
        assert_eq!(active_hours(day, DAY_HOURS_CAP + 3.0), 0.0);
        // Garbage in, nothing out.
        assert_eq!(active_hours(f64::NAN, 0.0), 0.0);
        assert_eq!(active_hours(-5.0, 0.0), 0.0);
    }

    #[test]
    fn a_rig_never_mines_more_than_its_lasers_cut() {
        let h = 3.0;
        let m3 = mined_m3(&RIG, h);
        assert!(positive(m3));
        assert!(m3 <= h * HOUR_S * RIG.m3_per_s(), "{m3}");
        // The trips and the efficiency cost something: strictly below the
        // continuous rate, and linear in the hours.
        assert!(m3 < h * HOUR_S * RIG.m3_per_s() * EFFICIENCY + 1e-9);
        assert!((mined_m3(&RIG, 2.0 * h) - 2.0 * m3).abs() < 1e-6);
    }

    #[test]
    fn an_unarmed_or_holdless_rig_mines_nothing() {
        for rig in [
            MiningRig { lasers: 0, ..RIG },
            MiningRig {
                amount_m3: 0.0,
                ..RIG
            },
            MiningRig {
                cycle_s: 0.0,
                ..RIG
            },
            MiningRig {
                hold_m3: 0.0,
                ..RIG
            },
        ] {
            assert_eq!(mined_m3(&rig, 5.0), 0.0, "{rig:?}");
        }
        assert_eq!(mined_m3(&RIG, 0.0), 0.0);
        assert_eq!(mined_m3(&RIG, f64::NAN), 0.0);
    }

    #[test]
    fn a_bigger_hold_means_fewer_trips_and_more_ore() {
        let small = mined_m3(
            &MiningRig {
                hold_m3: 50.0,
                ..RIG
            },
            2.0,
        );
        let big = mined_m3(
            &MiningRig {
                hold_m3: 5_000.0,
                ..RIG
            },
            2.0,
        );
        assert!(big > small);
    }

    fn rocks() -> Vec<RockView> {
        vec![
            RockView {
                id: 1,
                quantity: 100,
                unit_m3: 0.5,
            },
            RockView {
                id: 2,
                quantity: 100,
                unit_m3: 0.5,
            },
            RockView {
                id: 3,
                quantity: 1_000,
                unit_m3: 1.0,
            },
        ]
    }

    #[test]
    fn the_cut_walks_the_rocks_in_order_and_spends_the_volume() {
        // 30 m3 at 0.5 a unit is 60 units, all from the first rock.
        let plan = cut_plan(&rocks(), 30.0);
        assert_eq!(plan, vec![(1, 60)]);
        // 80 m3: the first rock gives its 100 units (50 m3), the second 60.
        let plan = cut_plan(&rocks(), 80.0);
        assert_eq!(plan, vec![(1, 100), (2, 60)]);
        let m3: f64 = plan
            .iter()
            .map(|&(id, n)| n as f64 * rocks()[id as usize - 1].unit_m3)
            .sum();
        assert!(m3 <= 80.0 + 1e-9);
    }

    #[test]
    fn the_cut_leaves_the_rest_of_the_belt_alone() {
        // Ask for far more than the belt: ROCK_SHARE of the units present.
        let plan = cut_plan(&rocks(), 1.0e9);
        let taken: i64 = plan.iter().map(|&(_, n)| n).sum();
        let present: i64 = rocks().iter().map(|r| r.quantity).sum();
        assert_eq!(taken, (present as f64 * ROCK_SHARE).floor() as i64);
        assert!(taken < present);
    }

    #[test]
    fn the_cut_skips_empty_rocks_and_survives_garbage() {
        let rocks = vec![
            RockView {
                id: 1,
                quantity: 0,
                unit_m3: 0.5,
            },
            RockView {
                id: 2,
                quantity: 10,
                unit_m3: 0.0,
            },
            RockView {
                id: 3,
                quantity: 10,
                unit_m3: f64::NAN,
            },
            RockView {
                id: 4,
                quantity: 10,
                unit_m3: 1.0,
            },
        ];
        // Only rock 4 is cuttable; ROCK_SHARE of 30 present is 15, rock 4
        // holds 10.
        assert_eq!(cut_plan(&rocks, 100.0), vec![(4, 10)]);
        assert!(cut_plan(&[], 100.0).is_empty());
        assert!(cut_plan(&rocks, f64::NAN).is_empty());
        assert!(cut_plan(&rocks, 0.0).is_empty());
    }

    const BATTERY: Battery = Battery {
        dps: 50.0,
        shots_per_s: 0.5,
    };

    fn squad(ehp: f64) -> SquadStat {
        SquadStat { ehp, members: 3 }
    }

    #[test]
    fn a_fight_kills_squads_until_the_hours_run_out() {
        // 1,200 ehp at 50 x HIT_EFFICIENCY dps.
        let kill_s = 1_200.0 / (BATTERY.dps * HIT_EFFICIENCY);
        let one = kill_s + SQUAD_OVERHEAD_S;
        let active_s = 10.5 * one / EFFICIENCY;
        let (f, killed) = fight(&BATTERY, i64::MAX, active_s, |_| squad(1_200.0));
        assert_eq!(f.squads, 10);
        assert_eq!(killed.len(), 10);
        assert_eq!(f.stopped, Stop::Time);
        assert!(f.seconds <= active_s * EFFICIENCY + 1e-9);
        assert_eq!(f.rounds, 10 * (kill_s * BATTERY.shots_per_s).ceil() as i64);
    }

    #[test]
    fn a_fight_stops_when_the_rounds_do() {
        let (f, _) = fight(&BATTERY, 25, 1.0e6, |_| squad(1_200.0));
        let per = (1_200.0 / (BATTERY.dps * HIT_EFFICIENCY) * BATTERY.shots_per_s).ceil() as i64;
        assert_eq!(f.squads as i64, 25 / per);
        assert_eq!(f.stopped, Stop::Ammo);
        assert!(f.rounds <= 25);
    }

    #[test]
    fn an_unarmed_ship_fights_nothing() {
        let none = Battery {
            dps: 0.0,
            shots_per_s: 0.0,
        };
        let (f, killed) = fight(&none, 1_000, 1.0e6, |_| squad(1.0));
        assert_eq!((f.squads, f.stopped), (0, Stop::Unarmed));
        assert!(killed.is_empty());
        let (f, _) = fight(&BATTERY, 1_000, 0.0, |_| squad(1.0));
        assert_eq!(f.stopped, Stop::Unarmed);
    }

    #[test]
    fn a_fight_is_bounded_by_the_live_kill_rate() {
        // Whatever the squads, a settled hour kills no more than
        // 1 / (kill_s + overhead) x 3600 x EFFICIENCY squads.
        let (f, _) = fight(&BATTERY, i64::MAX, 3.0 * HOUR_S, |_| squad(600.0));
        let live_per_h = HOUR_S / (600.0 / (BATTERY.dps * HIT_EFFICIENCY) + SQUAD_OVERHEAD_S);
        assert!(f.squads as f64 <= 3.0 * live_per_h * EFFICIENCY + 1e-9);
    }

    #[test]
    fn the_cap_ends_a_runaway_draw() {
        let (f, _) = fight(&BATTERY, i64::MAX, 1.0e12, |_| squad(0.0));
        assert_eq!(f.squads, MAX_SQUADS);
        assert_eq!(f.stopped, Stop::Cap);
    }

    #[test]
    fn the_npc_loop_loses_at_the_tax_an_unskilled_pilot_pays() {
        // Invented factors: the shape the live header describes.
        assert!(trade_edge(0.94, 1.00, 0.08) < 0.0);
        assert!(trade_edge(0.94, 1.00, 0.036) > 0.0);
        assert_eq!(trade_edge(0.0, 1.0, 0.0), 0.0);
        assert_eq!(trade_edge(1.0, f64::NAN, 0.0), 0.0);
    }

    #[test]
    fn a_losing_loop_trades_nothing_and_a_paying_one_is_bounded() {
        let none = trade_runs(1.0e9, 8.0, 1.0e9, 0.94, 1.0, 0.08);
        assert_eq!(none, Trades::default());
        let t = trade_runs(1.0e7, 8.0, 1.0e12, 0.94, 1.0, 0.036);
        assert!(positive(t.spent));
        // Net of the tax it is the edge on what was spent.
        let net = t.revenue - t.tax - t.spent;
        assert!((net - t.spent * trade_edge(0.94, 1.0, 0.036)).abs() < 1e-6 * t.spent);
        // The turnover cap binds.
        let capped = trade_runs(1.0e12, 8.0, 5.0e6, 0.94, 1.0, 0.036);
        assert!((capped.spent - 5.0e6).abs() < 1e-6);
        // No wallet, no work, no day left: nothing.
        assert_eq!(
            trade_runs(0.0, 8.0, 1.0e9, 0.94, 1.0, 0.0),
            Trades::default()
        );
        assert_eq!(
            trade_runs(1.0e9, 0.0, 1.0e9, 0.94, 1.0, 0.0),
            Trades::default()
        );
        assert_eq!(
            trade_runs(1.0e9, 8.0, 0.0, 0.94, 1.0, 0.0),
            Trades::default()
        );
    }

    #[test]
    fn a_build_must_clear_its_margin() {
        assert!(worth_building(105.0, 100.0));
        assert!(!worth_building(104.99, 100.0));
        assert!(!worth_building(50.0, 0.0));
        assert!(!worth_building(f64::NAN, 10.0));
    }

    #[test]
    fn runs_fill_the_job_target_at_least_once() {
        assert_eq!(planned_runs(JOB_TARGET_S / 4), 4);
        assert_eq!(planned_runs(JOB_TARGET_S * 3), 1);
        assert_eq!(planned_runs(0), JOB_TARGET_S);
    }

    #[test]
    fn the_halving_search_finds_the_last_run_that_holds() {
        for last in 0..=9 {
            assert_eq!(most_runs(9, |r| r <= last), last);
        }
        assert_eq!(most_runs(0, |_| true), 0);
        assert_eq!(most_runs(5, |_| true), 5);
    }

    #[test]
    fn the_sell_floor_is_a_share_of_the_reference_price() {
        assert_eq!(sell_floor(10.0), Some(5.0));
        assert_eq!(sell_floor(0.0), None);
        assert_eq!(sell_floor(f64::NAN), None);
        assert_eq!(sell_floor(0.01), None, "half a cent is no price");
    }
}
