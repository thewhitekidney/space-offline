//! **The attention bubble** (lane `bubble`, `docs/lanes/bubble.md`): keep
//! the bots where a human is, and shed some of them while the server is
//! slow.
//!
//! Fifty bots spread over the universe are a crowd nobody sees and a load
//! everybody feels. The planner [`Bubble::plan`] decides three things, once
//! every [`WAKE_GAP_MS`]:
//!
//! - **Pull.** A system with a human in it for [`SETTLE_MS`] and fewer than
//!   [`MIN_PRESENT`] awake bots (counting those already on the way) gets the
//!   nearest idle bot sent there. **By the gates**: the order is the same
//!   [`crate::driver::Handle::bring`] a GM's `/bots bring` is, and the bot
//!   flies the route itself. Nothing teleports.
//! - **Lease, not rehome** (lane `pull-lease`). Only the roles that can
//!   work in a human's system are pulled ([`RoleClass`]: miner, foreman,
//!   hauler, fighter), and the one the system lacks most comes first. The
//!   bot is *lent* ([`crate::driver::Handle::lend`]): its home stays, and
//!   it is recalled [`RETURN_AFTER_MS`] after the last human left the
//!   system, or after [`LEASE_MAX_MS`]. Traders, builders and the rest
//!   stay where their markets and supply chains are.
//! - **Shed.** While the server's tick p95 is over [`SHED_P95_MS`], one bot
//!   per fresh p95 sample is released: it goes home, docks and logs off
//!   through the ordinary session end. Farthest from every human first;
//!   never one in a human's system or on its way to one, never one in a
//!   fight or a warp, never one woken less than [`AWAKE_MIN_MS`] ago.
//! - **Re-admit.** After the p95 has stayed under [`READMIT_P95_MS`] for
//!   [`READMIT_AFTER_MS`], released bots wake again, one per
//!   [`WAKE_GAP_MS`], nearest to a human first. A bot that fails to come
//!   online [`WAKE_FAILS`] times in a row is left alone for
//!   [`BACKOFF_FIRST_MS`], doubling each time it gives up again.
//!
//! # No paper
//!
//! EvEJS credits sleeping personas a "paper income" from the mean of the
//! awake ones. **This module grants nothing.** Its only outputs are
//! [`Order`]s that move a bot, release it or wake it. A released bot's
//! absence is settled by the server's offline settlement exactly as a
//! timetable log-off is: at the log-off the bot says `econAgent.Park`
//! ([`crate::offline`]), and the server settles only what the real hull,
//! modules, rounds and skills can do. A bot without a career the server has
//! a rule for earns nothing while away. A bot that is shed and re-admitted
//! gets no more, and no less, than the same absence by timetable.
//!
//! # Where the inputs come from
//!
//! The server's clock writes `bubble.tsv` into the control directory every
//! health window (`eve_server::bot_control::BubbleState`: human systems and
//! the p95 of the clock's iteration). [`Runner`] reads it, ignores one older
//! than [`SAMPLE_STALE_S`], and does nothing without it (no humans known is
//! not "no humans").
//!
//! # EvEJS
//!
//! The numbers are EvEJS's (`ev:brain/README.md:165-200,243-260`,
//! `scheduler/pull.mts`, `ownerSettle.mts`, `wakeBackoff.mts`), taken as
//! hints; every one is **[A]** and the falsifier is on it.

use crate::brain::{ActKind, Place};
use crate::driver::Handle;
use crate::travel::Map;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Awake bots a human's system should hold **[A]** (EvEJS: 3). Falsified by
/// the owner finding the system still empty (more) or a crowd (fewer).
pub const MIN_PRESENT: usize = 3;
/// How long a human must have been in a system before bots are sent
/// **[A]** (EvEJS: 45 s): a gate hop is not a destination. Falsified by
/// bots arriving after the owner has already left.
pub const SETTLE_MS: u64 = 45_000;
/// Shed while the p95 of the clock's iteration is over this **[A]** (EvEJS:
/// 60 ms of tick). Falsified by a soak that lags at a lower p95, or never
/// reaches it.
pub const SHED_P95_MS: f64 = 60.0;
/// Re-admit only when the p95 is at or under this **[A]**: the gap below
/// [`SHED_P95_MS`] is the hysteresis, so one wake does not tip it back.
pub const READMIT_P95_MS: f64 = 40.0;
/// How long the p95 must stay under [`READMIT_P95_MS`] **[A]**.
pub const READMIT_AFTER_MS: u64 = 60_000;
/// The planner's step, and the gap between wakes **[A]** (EvEJS: 2 s).
pub const WAKE_GAP_MS: u64 = 2_000;
/// No release within this long of a wake **[A]** (EvEJS: 60 s): the
/// anti-flap rule. Falsified by `[bubble]` lines showing a bot woken and
/// shed in a loop.
pub const AWAKE_MIN_MS: u64 = 60_000;
/// Failed wakes in a row before the planner gives a bot up **[A]** (EvEJS:
/// 4).
pub const WAKE_FAILS: u32 = 4;
/// The first give-up's length **[A]** (EvEJS: 10 min), doubled at each
/// further give-up, [`BACKOFF_DOUBLINGS`] times at most.
pub const BACKOFF_FIRST_MS: u64 = 600_000;
pub const BACKOFF_DOUBLINGS: u32 = 8;
/// A woken bot has this long to be logged on before the wake counts as
/// failed **[A]**: the login stagger plus a world load.
pub const WAKE_DEADLINE_MS: u64 = 90_000;
/// The farthest a bot is pulled, in jumps **[A]**: "in-window" for a pull.
pub const MAX_PULL_JUMPS: u32 = 6;
/// A bot pulled is not pulled again for this long **[A]**: a human who
/// keeps moving must not drag the same bot around.
pub const PULL_COOLDOWN_MS: u64 = 300_000;
/// A lease ends this long after the last human left its system **[A]**.
/// Falsified by the owner finding the systems he visits empty after the
/// bots return (longer), or the bots idle in a dead system (shorter).
pub const RETURN_AFTER_MS: u64 = 600_000;
/// A lease ends at the latest this long after it began **[A]**: a human who
/// lives in one system must not hold a bot from its work for good.
pub const LEASE_MAX_MS: u64 = 3 * 3600 * 1000;
/// Seconds a jump takes with the gate wait **[A]** (EvEJS: 44 s a jump and
/// 30 s of landing), for the ETA in the ledger only.
pub const JUMP_ETA_S: u64 = 44;
pub const LAND_ETA_S: u64 = 30;
/// A bubble file older than this is no information.
pub const SAMPLE_STALE_S: u64 = 60;
/// The ledger line is printed at least this often while a system is short.
pub const LEDGER_EVERY: Duration = Duration::from_secs(30);

/// What a bot is, for the planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Released by the bubble: logged off or on its way to it.
    Released,
    /// Logged on and not released.
    Awake,
    /// Neither: not yet logged on, or off by its own timetable.
    Dark,
}

/// What a bot can do for a human's system, for the pull. Everything else
/// (trader, builder, reactor, PI, CEO, hostile, sov, capital) stays home
/// **[A]**: their work is in their own markets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleClass {
    Miner,
    Foreman,
    Hauler,
    /// Escort, hunter, missions, sites.
    Fighter,
    Other,
}

impl RoleClass {
    /// The class of the role named `name` ([`crate::roles::RoleSpec::name`]).
    pub fn of(name: &str) -> RoleClass {
        match name {
            "mine" => RoleClass::Miner,
            "foreman" => RoleClass::Foreman,
            "freighter" => RoleClass::Hauler,
            "escort" | "hunter" | "missions" | "sites" => RoleClass::Fighter,
            _ => RoleClass::Other,
        }
    }

    /// Whether the pull may take a bot of this class.
    pub fn pullable(self) -> bool {
        self != RoleClass::Other
    }
}

/// One bot as the planner sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotView {
    pub name: String,
    pub phase: Phase,
    /// Where it is; `None` in a session change (and so never moved).
    pub system: Option<i64>,
    /// In a fight or a warp: never released, never pulled.
    pub busy: bool,
    /// Its timetable has a window open (a wake that cannot log on is not
    /// asked for).
    pub in_window: bool,
    pub role: RoleClass,
    /// A mining module is fitted (a miner without one is no use at a belt).
    pub can_mine_now: bool,
    /// The station it is on loan to, if any: never pulled again.
    pub leased_to: Option<i64>,
}

/// One server sample: the p95 and where the humans are.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// Counts windows; a new value is a new sample.
    pub seq: u64,
    pub p95_ms: f64,
    pub humans: Vec<i64>,
    /// Human systems known to have no asteroid belt: no miner is pulled
    /// there. Empty: belts everywhere.
    pub no_belts: Vec<i64>,
}

/// What the planner asks for. Only movement and log-on/off: nothing here
/// can put ISK or an item anywhere (module header).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Order {
    /// Fly to `system` by the gates.
    Pull {
        bot: String,
        system: i64,
        jumps: u32,
        eta_s: u64,
        why: String,
    },
    /// Go home, dock, log off.
    Release { bot: String, why: String },
    /// Log on again.
    Wake { bot: String, why: String },
    /// End a lease: fly home (PULL-LEASE).
    Return { bot: String, why: String },
}

/// One human system's fill, for the ledger line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fill {
    pub system: i64,
    pub wanted: usize,
    pub present: usize,
    /// `(bot, seconds to go)`.
    pub en_route: Vec<(String, u64)>,
    /// Seconds of settling left; `None` once it is over.
    pub settling_s: Option<u64>,
}

impl Fill {
    pub fn short(&self) -> bool {
        self.present + self.en_route.len() < self.wanted
    }

    /// `[bubble] fill 30000001: wanted 3 present 1 en route 1 (botA eta 80 s) settling 20 s`
    pub fn line(&self) -> String {
        let en: Vec<String> = self
            .en_route
            .iter()
            .map(|(b, s)| format!("{b} eta {s} s"))
            .collect();
        let mut out = format!(
            "[bubble] fill {}: wanted {} present {} en route {}",
            self.system,
            self.wanted,
            self.present,
            self.en_route.len()
        );
        if !en.is_empty() {
            out.push_str(&format!(" ({})", en.join(", ")));
        }
        if let Some(s) = self.settling_s {
            out.push_str(&format!(" settling {s} s"));
        }
        out
    }
}

/// One planner step's answer.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Plan {
    pub orders: Vec<Order>,
    pub ledger: Vec<Fill>,
}

/// A bot on loan.
#[derive(Debug, Clone, Copy)]
struct Lease {
    system: i64,
    since: u64,
    /// When the last human left the system; `None` while one is there.
    empty_since: Option<u64>,
}

/// A bot's wake record.
#[derive(Debug, Default, Clone, Copy)]
struct Wakes {
    fails: u32,
    giveups: u32,
    /// No wake before this (ms).
    until: u64,
}

/// The planner and what it remembers between steps.
#[derive(Debug, Default)]
pub struct Bubble {
    human_since: BTreeMap<i64, u64>,
    /// `bot -> (system, arrives_at, give_up_at)`.
    en_route: BTreeMap<String, (i64, u64, u64)>,
    pulled_at: BTreeMap<String, u64>,
    /// `bot -> lease`.
    leases: BTreeMap<String, Lease>,
    woke_at: BTreeMap<String, u64>,
    /// `bot -> deadline`.
    waking: BTreeMap<String, u64>,
    wakes: BTreeMap<String, Wakes>,
    /// Bots the bubble released: the only ones it ever wakes.
    shed: BTreeSet<String>,
    last_wake: Option<u64>,
    last_seq: Option<u64>,
    healthy_since: Option<u64>,
}

/// Jumps between two systems; `None`: no route.
pub type Dist<'a> = &'a dyn Fn(i64, i64) -> Option<u32>;

impl Bubble {
    pub fn new() -> Bubble {
        Bubble::default()
    }

    /// How many bots the bubble holds released now.
    pub fn shed_count(&self) -> usize {
        self.shed.len()
    }

    /// Whether `bot` is held off by a give-up at `now`.
    pub fn backed_off(&self, bot: &str, now: u64) -> bool {
        self.wakes.get(bot).is_some_and(|w| now < w.until)
    }

    /// **One step.** `now` is milliseconds on any monotonic clock; `sample`
    /// is `None` when the server's file is missing or stale (then nothing is
    /// asked, but a wake in flight is still judged).
    pub fn plan(
        &mut self,
        now: u64,
        sample: Option<&Sample>,
        bots: &[BotView],
        dist: Dist<'_>,
    ) -> Plan {
        let mut plan = Plan::default();
        let view: BTreeMap<&str, &BotView> = bots.iter().map(|b| (b.name.as_str(), b)).collect();

        // 1. Wakes in flight: logged on is success, the deadline is failure.
        let waking: Vec<(String, u64)> = self.waking.iter().map(|(k, v)| (k.clone(), *v)).collect();
        for (bot, deadline) in waking {
            match view.get(bot.as_str()) {
                Some(b) if b.phase == Phase::Awake => {
                    self.waking.remove(&bot);
                    self.wakes.remove(&bot);
                    self.shed.remove(&bot);
                }
                Some(_) if now < deadline => {}
                Some(_) => {
                    self.waking.remove(&bot);
                    let w = self.wakes.entry(bot.clone()).or_default();
                    w.fails += 1;
                    let mut why = format!("wake {} of {WAKE_FAILS} failed", w.fails);
                    if w.fails >= WAKE_FAILS {
                        w.fails = 0;
                        w.giveups += 1;
                        let shift = (w.giveups - 1).min(BACKOFF_DOUBLINGS);
                        w.until = now + (BACKOFF_FIRST_MS << shift);
                        why = format!(
                            "gave up after {WAKE_FAILS} failed wakes, back off {} s",
                            (BACKOFF_FIRST_MS << shift) / 1000
                        );
                    }
                    // Back to sleep, so a bot stuck connecting stops trying.
                    plan.orders.push(Order::Release { bot, why });
                    self.last_wake = Some(now);
                }
                None => {
                    self.waking.remove(&bot);
                }
            }
        }

        let Some(sample) = sample else {
            return plan;
        };
        let humans: BTreeSet<i64> = sample.humans.iter().copied().collect();

        // 2. Who has been where, since when.
        self.human_since.retain(|s, _| humans.contains(s));
        for &s in &humans {
            self.human_since.entry(s).or_insert(now);
        }

        // 3. Pulls that arrived, died or timed out.
        self.en_route.retain(
            |bot, &mut (system, _, give_up)| match view.get(bot.as_str()) {
                Some(b) if b.phase == Phase::Awake => {
                    b.system != Some(system) && now < give_up && humans.contains(&system)
                }
                _ => false,
            },
        );

        // 4. The p95 and its hysteresis, once per fresh sample.
        let fresh = self.last_seq != Some(sample.seq);
        self.last_seq = Some(sample.seq);
        if fresh {
            if sample.p95_ms > SHED_P95_MS {
                self.healthy_since = None;
            } else if sample.p95_ms <= READMIT_P95_MS {
                self.healthy_since.get_or_insert(now);
            } else {
                self.healthy_since = None;
            }
        }
        let nearest_human = |b: &BotView| -> u32 {
            b.system
                .and_then(|s| humans.iter().filter_map(|&h| dist(s, h)).min())
                .unwrap_or(u32::MAX)
        };
        let protected = |b: &BotView| -> bool {
            b.system.is_some_and(|s| humans.contains(&s))
                || self
                    .en_route
                    .get(&b.name)
                    .is_some_and(|&(sys, _, _)| humans.contains(&sys))
        };

        // 5. Shed: one per fresh over-budget sample.
        let mut moved_load = false;
        if fresh && sample.p95_ms > SHED_P95_MS {
            let mut cands: Vec<&BotView> = bots
                .iter()
                .filter(|b| {
                    b.phase == Phase::Awake
                        && !b.busy
                        && b.system.is_some()
                        && !protected(b)
                        && self
                            .woke_at
                            .get(&b.name)
                            .is_none_or(|&t| now.saturating_sub(t) >= AWAKE_MIN_MS)
                })
                .collect();
            // Farthest first, then by name.
            cands.sort_by(|a, b| {
                nearest_human(b)
                    .cmp(&nearest_human(a))
                    .then_with(|| a.name.cmp(&b.name))
            });
            if let Some(b) = cands.first() {
                let far = nearest_human(b);
                let from_humans = if far == u32::MAX {
                    "no route to a human".to_owned()
                } else {
                    format!("{far} jumps from the nearest human")
                };
                plan.orders.push(Order::Release {
                    bot: b.name.clone(),
                    why: format!(
                        "tick p95 {:.1} ms over {SHED_P95_MS:.0}; {from_humans}",
                        sample.p95_ms
                    ),
                });
                self.shed.insert(b.name.clone());
                self.woke_at.remove(&b.name);
                moved_load = true;
            }
        }

        // 6. Re-admit: healthy for a while, one per wake gap.
        let healthy = sample.p95_ms <= READMIT_P95_MS
            && self
                .healthy_since
                .is_some_and(|t| now.saturating_sub(t) >= READMIT_AFTER_MS);
        let gap_ok = self
            .last_wake
            .is_none_or(|t| now.saturating_sub(t) >= WAKE_GAP_MS);
        if healthy && gap_ok && !moved_load {
            let mut cands: Vec<&BotView> = bots
                .iter()
                .filter(|b| {
                    b.phase == Phase::Released
                        && b.in_window
                        && self.shed.contains(&b.name)
                        && !self.waking.contains_key(&b.name)
                        && !self.backed_off(&b.name, now)
                })
                .collect();
            cands.sort_by(|a, b| {
                nearest_human(a)
                    .cmp(&nearest_human(b))
                    .then_with(|| a.name.cmp(&b.name))
            });
            if let Some(b) = cands.first() {
                plan.orders.push(Order::Wake {
                    bot: b.name.clone(),
                    why: format!(
                        "tick p95 {:.1} ms, healthy for {} s",
                        sample.p95_ms,
                        now.saturating_sub(self.healthy_since.unwrap_or(now)) / 1000
                    ),
                });
                self.waking.insert(b.name.clone(), now + WAKE_DEADLINE_MS);
                self.woke_at.insert(b.name.clone(), now);
                self.last_wake = Some(now);
            }
        }

        // 7. Pull: at most one a step; the longest-waiting short system first.
        let mut order: Vec<(u64, i64)> = self.human_since.iter().map(|(&s, &t)| (t, s)).collect();
        order.sort();
        let mut pulled = false;
        for &(since, system) in &order {
            if pulled || now.saturating_sub(since) < SETTLE_MS {
                continue;
            }
            let present = bots
                .iter()
                .filter(|b| b.phase == Phase::Awake && b.system == Some(system))
                .count();
            let inbound = self
                .en_route
                .values()
                .filter(|&&(s, _, _)| s == system)
                .count();
            if present + inbound >= MIN_PRESENT {
                continue;
            }
            // The class mix the system holds: present and on the way.
            let mut mix: BTreeMap<u8, usize> = BTreeMap::new();
            let class_of = |name: &str| view.get(name).map_or(RoleClass::Other, |b| b.role);
            for b in bots
                .iter()
                .filter(|b| b.phase == Phase::Awake && b.system == Some(system))
            {
                *mix.entry(b.role as u8).or_default() += 1;
            }
            for (name, &(s, _, _)) in &self.en_route {
                if s == system {
                    *mix.entry(class_of(name) as u8).or_default() += 1;
                }
            }
            let belts = !sample.no_belts.contains(&system);
            let mut cands: Vec<(usize, u32, &BotView)> = bots
                .iter()
                .filter(|b| {
                    b.phase == Phase::Awake
                        && !b.busy
                        && b.role.pullable()
                        && (b.role != RoleClass::Miner || (belts && b.can_mine_now))
                        && b.leased_to.is_none()
                        && !self.leases.contains_key(&b.name)
                        && b.system.is_some_and(|s| !humans.contains(&s))
                        && !self.en_route.contains_key(&b.name)
                        && self
                            .pulled_at
                            .get(&b.name)
                            .is_none_or(|&t| now.saturating_sub(t) >= PULL_COOLDOWN_MS)
                })
                .filter_map(|b| {
                    let j = dist(b.system?, system)?;
                    (j <= MAX_PULL_JUMPS).then_some((
                        mix.get(&(b.role as u8)).copied().unwrap_or(0),
                        j,
                        b,
                    ))
                })
                .collect();
            cands.sort_by(|a, b| {
                a.0.cmp(&b.0)
                    .then_with(|| a.1.cmp(&b.1))
                    .then_with(|| a.2.name.cmp(&b.2.name))
            });
            if let Some(&(_, jumps, b)) = cands.first() {
                let eta = u64::from(jumps) * JUMP_ETA_S + LAND_ETA_S;
                plan.orders.push(Order::Pull {
                    bot: b.name.clone(),
                    system,
                    jumps,
                    eta_s: eta,
                    why: format!(
                        "{present} present and {inbound} on the way of {MIN_PRESENT}; \
                         human there {} s",
                        now.saturating_sub(since) / 1000
                    ),
                });
                self.en_route.insert(
                    b.name.clone(),
                    (system, now + eta * 1000, now + eta * 2000 + 60_000),
                );
                self.pulled_at.insert(b.name.clone(), now);
                self.leases.insert(
                    b.name.clone(),
                    Lease {
                        system,
                        since: now,
                        empty_since: None,
                    },
                );
                pulled = true;
            }
        }

        // 7b. Leases end: no human in the system for RETURN_AFTER_MS, or too old.
        let mut over = Vec::new();
        for (bot, l) in self.leases.iter_mut() {
            if humans.contains(&l.system) {
                l.empty_since = None;
            } else {
                l.empty_since.get_or_insert(now);
            }
            let gone = l
                .empty_since
                .is_some_and(|t| now.saturating_sub(t) >= RETURN_AFTER_MS);
            let old = now.saturating_sub(l.since) >= LEASE_MAX_MS;
            if gone || old {
                let why = if gone {
                    format!(
                        "no human in system {} for {} s",
                        l.system,
                        RETURN_AFTER_MS / 1000
                    )
                } else {
                    format!("lease older than {} s", LEASE_MAX_MS / 1000)
                };
                over.push((bot.clone(), why));
            }
        }
        for (bot, why) in over {
            self.leases.remove(&bot);
            self.en_route.remove(&bot);
            plan.orders.push(Order::Return { bot, why });
        }

        // 8. The ledger.
        for &(since, system) in &order {
            let settling = SETTLE_MS.saturating_sub(now.saturating_sub(since));
            plan.ledger.push(Fill {
                system,
                wanted: MIN_PRESENT,
                present: bots
                    .iter()
                    .filter(|b| b.phase == Phase::Awake && b.system == Some(system))
                    .count(),
                en_route: self
                    .en_route
                    .iter()
                    .filter(|&(_, &(s, _, _))| s == system)
                    .map(|(b, &(_, at, _))| (b.clone(), at.saturating_sub(now) / 1000))
                    .collect(),
                settling_s: (settling > 0).then_some(settling.div_ceil(1000)),
            });
        }
        plan
    }
}

// ---------------------------------------------------------------------
// The runner: the impure end
// ---------------------------------------------------------------------

/// Reads the server's file, builds the views, runs the planner and applies
/// its orders to the bots' [`Handle`]s.
pub struct Runner {
    planner: Bubble,
    file: PathBuf,
    map: Arc<Map>,
    floor: f64,
    handles: Vec<Handle>,
    started: Instant,
    next: Instant,
    last_ledger: BTreeMap<i64, Instant>,
    no_station: BTreeSet<i64>,
}

impl Runner {
    /// `dir` is the control directory the server writes `bubble.tsv` into.
    pub fn new(dir: &std::path::Path, map: Arc<Map>, floor: f64, handles: Vec<Handle>) -> Runner {
        let now = Instant::now();
        for h in &handles {
            h.watch();
        }
        Runner {
            planner: Bubble::new(),
            file: dir.join(eve_server::bot_control::BUBBLE_FILE),
            map,
            floor,
            handles,
            started: now,
            next: now,
            last_ledger: BTreeMap::new(),
            no_station: BTreeSet::new(),
        }
    }

    /// The sample on disk, `None` when absent, unreadable or stale.
    fn read(&mut self, unix_now: u64) -> Option<Sample> {
        let text = std::fs::read_to_string(&self.file).ok()?;
        let st = eve_server::bot_control::BubbleState::parse(&text)?;
        if unix_now.saturating_sub(st.unix_s) > SAMPLE_STALE_S {
            return None;
        }
        let mut humans = Vec::new();
        for (s, _) in st.humans {
            if self.map.stations_in(s).is_empty() {
                if self.no_station.insert(s) {
                    println!("[bubble] system {s} has a human and no station on the map: no pull");
                }
            } else {
                humans.push(s);
            }
        }
        Some(Sample {
            seq: st.seq,
            p95_ms: st.p95_ms,
            no_belts: humans
                .iter()
                .copied()
                .filter(|&s| !self.map.has_belt(s))
                .collect(),
            humans,
        })
    }

    fn view(&self, h: &Handle, unix_now: u64) -> BotView {
        let obs = h.observation();
        let system = match obs.as_ref().map(|o| o.place) {
            Some(Place::Docked { station }) => self.map.system_of(station),
            Some(Place::InSpace { system, .. }) => Some(system),
            _ => None,
        };
        let live = h.is_live();
        let fighting = obs.as_ref().is_some_and(|o| {
            matches!(o.place, Place::InSpace { .. })
                && (!o.locked.is_empty() || !o.pending.is_empty() || o.health.is_some())
        });
        let moving = h.last_entry().is_some_and(|e| {
            matches!(
                e.kind,
                ActKind::WarpToItem
                    | ActKind::WarpToMember
                    | ActKind::Fire
                    | ActKind::Hunt
                    | ActKind::Gang
                    | ActKind::Raid
                    | ActKind::Dive
            )
        });
        BotView {
            name: h.name.clone(),
            phase: if h.released() {
                Phase::Released
            } else if live {
                Phase::Awake
            } else {
                Phase::Dark
            },
            system,
            busy: obs.as_ref().is_some_and(|o| o.place == Place::Transit)
                || (live && (fighting || moving)),
            in_window: h.in_window(unix_now),
            role: RoleClass::of(h.role_name()),
            can_mine_now: obs.as_ref().is_some_and(|o| o.miner.is_some()),
            leased_to: h.leased_to(),
        }
    }

    /// Run one step if its time has come; the lines to print (decisions and
    /// the fill ledger).
    pub fn step(&mut self, unix_now: u64) -> Vec<String> {
        let at = Instant::now();
        if at < self.next {
            return Vec::new();
        }
        self.next = at + Duration::from_millis(WAKE_GAP_MS);
        let now = u64::try_from(at.duration_since(self.started).as_millis()).unwrap_or(u64::MAX);
        let sample = self.read(unix_now);
        let views: Vec<BotView> = self
            .handles
            .iter()
            .map(|h| self.view(h, unix_now))
            .collect();
        let (map, floor) = (Arc::clone(&self.map), self.floor);
        let dist = move |a: i64, b: i64| map.jumps(a, b, floor);
        let plan = self.planner.plan(now, sample.as_ref(), &views, &dist);
        let mut lines = Vec::new();
        for o in &plan.orders {
            lines.push(self.apply(o));
        }
        for f in &plan.ledger {
            let due = self
                .last_ledger
                .get(&f.system)
                .is_none_or(|t| at.duration_since(*t) >= LEDGER_EVERY);
            if f.short() && due {
                self.last_ledger.insert(f.system, at);
                lines.push(f.line());
            }
        }
        lines
    }

    fn handle(&self, bot: &str) -> Option<&Handle> {
        self.handles.iter().find(|h| h.name == bot)
    }

    fn apply(&self, o: &Order) -> String {
        match o {
            Order::Pull {
                bot,
                system,
                jumps,
                eta_s,
                why,
            } => {
                let station = self.map.stations_in(*system).first().copied();
                match (self.handle(bot), station) {
                    (Some(h), Some(st)) => {
                        h.lend(st);
                        format!(
                            "[bubble] lease {bot} -> system {system} station {st}: {jumps} jumps, \
                             eta {eta_s} s ({why})"
                        )
                    }
                    _ => format!("[bubble] pull {bot} -> system {system}: no handle or station"),
                }
            }
            Order::Return { bot, why } => {
                if let Some(h) = self.handle(bot) {
                    h.recall();
                }
                format!("[bubble] return {bot}: {why}")
            }
            Order::Release { bot, why } => {
                if let Some(h) = self.handle(bot) {
                    h.release();
                }
                format!("[bubble] shed {bot}: {why}")
            }
            Order::Wake { bot, why } => {
                if let Some(h) = self.handle(bot) {
                    h.wake();
                }
                format!("[bubble] readmit {bot}: {why}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented systems on a line: 1 - 2 - 3 - 4 - 5 - 6 - 7 - 8 - 9.
    fn line(a: i64, b: i64) -> Option<u32> {
        Some(u32::try_from((a - b).abs()).unwrap())
    }

    fn bot(name: &str, system: i64) -> BotView {
        BotView {
            name: name.to_owned(),
            phase: Phase::Awake,
            system: Some(system),
            busy: false,
            in_window: true,
            role: RoleClass::Miner,
            can_mine_now: true,
            leased_to: None,
        }
    }

    fn sample(seq: u64, p95: f64, humans: &[i64]) -> Sample {
        Sample {
            seq,
            p95_ms: p95,
            humans: humans.to_vec(),
            no_belts: Vec::new(),
        }
    }

    fn pulls(p: &Plan) -> Vec<(&str, i64)> {
        p.orders
            .iter()
            .filter_map(|o| match o {
                Order::Pull { bot, system, .. } => Some((bot.as_str(), *system)),
                _ => None,
            })
            .collect()
    }

    fn releases(p: &Plan) -> Vec<&str> {
        p.orders
            .iter()
            .filter_map(|o| match o {
                Order::Release { bot, .. } => Some(bot.as_str()),
                _ => None,
            })
            .collect()
    }

    fn wakes(p: &Plan) -> Vec<&str> {
        p.orders
            .iter()
            .filter_map(|o| match o {
                Order::Wake { bot, .. } => Some(bot.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_pull_waits_for_the_settle_delay_and_takes_the_nearest() {
        let mut b = Bubble::new();
        let bots = [bot("far", 9), bot("near", 6), bot("mid", 8)];
        let s = sample(1, 5.0, &[4]);
        let p = b.plan(0, Some(&s), &bots, &line);
        assert!(pulls(&p).is_empty(), "the human just arrived");
        assert_eq!(p.ledger[0].settling_s, Some(45));
        let p = b.plan(SETTLE_MS - 1, Some(&s), &bots, &line);
        assert!(pulls(&p).is_empty());
        let p = b.plan(SETTLE_MS, Some(&s), &bots, &line);
        assert_eq!(pulls(&p), vec![("near", 4)]);
        match &p.orders[0] {
            Order::Pull { jumps, eta_s, .. } => {
                assert_eq!(*jumps, 2);
                assert_eq!(*eta_s, 2 * JUMP_ETA_S + LAND_ETA_S);
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn pulls_count_those_on_the_way_and_stop_at_the_minimum() {
        let mut b = Bubble::new();
        let mut bots = vec![
            bot("a", 7),
            bot("b", 8),
            bot("c", 9),
            bot("d", 2),
            bot("here", 1),
        ];
        let s = sample(1, 5.0, &[1]);
        b.plan(0, Some(&s), &bots, &line);
        let mut sent = Vec::new();
        for k in 1..=6 {
            let p = b.plan(SETTLE_MS + k * WAKE_GAP_MS, Some(&s), &bots, &line);
            for (bot, _) in pulls(&p) {
                sent.push(bot.to_owned());
            }
        }
        // One present, so exactly two more are asked for, nearest first.
        assert_eq!(sent, vec!["d", "a"]);
        // The ledger shows them on the way.
        let p = b.plan(SETTLE_MS + 7 * WAKE_GAP_MS, Some(&s), &bots, &line);
        assert_eq!(p.ledger[0].present, 1);
        assert_eq!(p.ledger[0].en_route.len(), 2);
        assert!(!p.ledger[0].short());
        assert!(p.ledger[0].line().contains("en route 2"));
        // They arrive: present 3, nobody on the way.
        bots[3].system = Some(1);
        bots[0].system = Some(1);
        let p = b.plan(SETTLE_MS + 8 * WAKE_GAP_MS, Some(&s), &bots, &line);
        assert_eq!(p.ledger[0].present, 3);
        assert!(p.ledger[0].en_route.is_empty());
        assert!(pulls(&p).is_empty());
    }

    #[test]
    fn a_busy_or_unreachable_bot_is_never_pulled_and_a_pulled_one_rests() {
        let mut b = Bubble::new();
        let mut busy = bot("busy", 2);
        busy.busy = true;
        let bots = [busy, bot("distant", 40), bot("ok", 5)];
        let s = sample(1, 5.0, &[1]);
        b.plan(0, Some(&s), &bots, &line);
        let p = b.plan(SETTLE_MS, Some(&s), &bots, &line);
        assert_eq!(pulls(&p), vec![("ok", 1)]);
        // The human moves to 9; `ok` already pulled within the cooldown.
        let s2 = sample(2, 5.0, &[9]);
        let mut bots2 = bots.to_vec();
        bots2[2].system = Some(8);
        b.plan(SETTLE_MS + 1, Some(&s2), &bots2, &line);
        let p = b.plan(2 * SETTLE_MS + 1, Some(&s2), &bots2, &line);
        assert!(pulls(&p).iter().all(|(n, _)| *n != "ok"));
        assert!(pulls(&p).iter().all(|(n, _)| *n != "distant"), "40 jumps");
    }

    #[test]
    fn shedding_goes_farthest_first_one_per_sample_and_spares_the_bubble() {
        let mut b = Bubble::new();
        let mut fighting = bot("fighting", 9);
        fighting.busy = true;
        let bots = [
            bot("beside", 1),
            bot("mid", 4),
            bot("far", 8),
            fighting,
            bot("zfar", 8),
        ];
        let hot = |seq| sample(seq, 90.0, &[1]);
        let p = b.plan(0, Some(&hot(1)), &bots, &line);
        assert_eq!(
            releases(&p),
            vec!["far"],
            "farthest, ties by name, never busy"
        );
        // The same sample again sheds nothing more.
        let p = b.plan(WAKE_GAP_MS, Some(&hot(1)), &bots, &line);
        assert!(releases(&p).is_empty());
        let mut bots = bots.to_vec();
        bots[2].phase = Phase::Released;
        let p = b.plan(10_000, Some(&hot(2)), &bots, &line);
        assert_eq!(releases(&p), vec!["zfar"]);
        bots[4].phase = Phase::Released;
        let p = b.plan(20_000, Some(&hot(3)), &bots, &line);
        assert_eq!(releases(&p), vec!["mid"]);
        bots[1].phase = Phase::Released;
        let p = b.plan(30_000, Some(&hot(4)), &bots, &line);
        assert!(
            releases(&p).is_empty(),
            "the one beside the human and the fighter stay"
        );
        assert_eq!(b.shed_count(), 3);
    }

    #[test]
    fn a_bot_on_its_way_to_a_human_is_not_shed() {
        let mut b = Bubble::new();
        let bots = [bot("walker", 5), bot("other", 7)];
        let s = sample(1, 5.0, &[3]);
        b.plan(0, Some(&s), &bots, &line);
        let p = b.plan(SETTLE_MS, Some(&s), &bots, &line);
        assert_eq!(pulls(&p), vec![("walker", 3)]);
        let hot = sample(2, 99.0, &[3]);
        let p = b.plan(SETTLE_MS + 1, Some(&hot), &bots, &line);
        assert_eq!(releases(&p), vec!["other"]);
    }

    #[test]
    fn readmit_waits_for_a_healthy_spell_then_wakes_one_per_gap_nearest_first() {
        let mut b = Bubble::new();
        let mut bots = vec![bot("a", 2), bot("b", 6), bot("c", 9)];
        // Shed all three over three samples (humans at 1, nobody protected).
        for (k, who) in ["c", "b", "a"].iter().enumerate() {
            let p = b.plan(
                k as u64 * 10_000,
                Some(&sample(k as u64 + 1, 90.0, &[1])),
                &bots,
                &line,
            );
            assert_eq!(releases(&p), vec![*who]);
            let i = bots.iter().position(|x| x.name == *who).unwrap();
            bots[i].phase = Phase::Released;
        }
        // Healthy samples start at t = 30 s; nothing wakes for 60 s.
        let t0 = 30_000;
        let p = b.plan(t0, Some(&sample(4, 20.0, &[1])), &bots, &line);
        assert!(wakes(&p).is_empty());
        let p = b.plan(
            t0 + READMIT_AFTER_MS - 1,
            Some(&sample(5, 20.0, &[1])),
            &bots,
            &line,
        );
        assert!(wakes(&p).is_empty());
        // The in-between band neither sheds nor re-admits and resets the spell.
        let p = b.plan(
            t0 + READMIT_AFTER_MS,
            Some(&sample(6, 50.0, &[1])),
            &bots,
            &line,
        );
        assert!(wakes(&p).is_empty() && releases(&p).is_empty());
        let t1 = t0 + READMIT_AFTER_MS + 1_000;
        b.plan(t1, Some(&sample(7, 20.0, &[1])), &bots, &line);
        let t2 = t1 + READMIT_AFTER_MS;
        let p = b.plan(t2, Some(&sample(8, 20.0, &[1])), &bots, &line);
        assert_eq!(wakes(&p), vec!["a"], "nearest the human first");
        // One per gap.
        let p = b.plan(t2 + 1, Some(&sample(8, 20.0, &[1])), &bots, &line);
        assert!(wakes(&p).is_empty());
        bots[0].phase = Phase::Awake;
        let p = b.plan(t2 + WAKE_GAP_MS, Some(&sample(8, 20.0, &[1])), &bots, &line);
        assert_eq!(wakes(&p), vec!["b"]);
    }

    #[test]
    fn a_woken_bot_is_not_shed_again_within_the_hysteresis() {
        let mut b = Bubble::new();
        let mut bots = vec![bot("a", 8)];
        let p = b.plan(0, Some(&sample(1, 90.0, &[1])), &bots, &line);
        assert_eq!(releases(&p), vec!["a"]);
        bots[0].phase = Phase::Released;
        b.plan(1_000, Some(&sample(2, 20.0, &[1])), &bots, &line);
        let t = 1_000 + READMIT_AFTER_MS;
        let p = b.plan(t, Some(&sample(3, 20.0, &[1])), &bots, &line);
        assert_eq!(wakes(&p), vec!["a"]);
        bots[0].phase = Phase::Awake;
        // Trouble again 10 s later: too soon to release it.
        let p = b.plan(t + 10_000, Some(&sample(4, 99.0, &[1])), &bots, &line);
        assert!(releases(&p).is_empty());
        let p = b.plan(t + AWAKE_MIN_MS, Some(&sample(5, 99.0, &[1])), &bots, &line);
        assert_eq!(releases(&p), vec!["a"]);
    }

    #[test]
    fn a_bot_that_will_not_wake_is_given_up_with_a_doubling_backoff() {
        let mut b = Bubble::new();
        let mut bots = vec![bot("sleepy", 8)];
        b.plan(0, Some(&sample(1, 90.0, &[1])), &bots, &line);
        bots[0].phase = Phase::Released;
        let mut now = 0u64;
        let mut seq = 2u64;
        let mut backoffs = Vec::new();
        for _round in 0..3 {
            let mut fails = 0;
            while fails < WAKE_FAILS {
                // Wait until it is asked to wake.
                let mut woke = false;
                for _ in 0..4_000 {
                    now += 5_000;
                    seq += 1;
                    let p = b.plan(now, Some(&sample(seq, 20.0, &[1])), &bots, &line);
                    if !wakes(&p).is_empty() {
                        woke = true;
                        break;
                    }
                }
                assert!(woke, "asked to wake");
                // It never comes online: the deadline passes.
                now += WAKE_DEADLINE_MS;
                seq += 1;
                let p = b.plan(now, Some(&sample(seq, 20.0, &[1])), &bots, &line);
                assert_eq!(releases(&p), vec!["sleepy"], "put back to sleep");
                fails += 1;
            }
            assert!(b.backed_off("sleepy", now));
            backoffs.push(b.wakes["sleepy"].until - now);
            // Within the back-off nothing is asked.
            let until = b.wakes["sleepy"].until;
            while now + 5_000 < until {
                now += 5_000;
                seq += 1;
                let p = b.plan(now, Some(&sample(seq, 20.0, &[1])), &bots, &line);
                assert!(wakes(&p).is_empty());
            }
            now = until;
        }
        assert_eq!(
            backoffs,
            vec![BACKOFF_FIRST_MS, 2 * BACKOFF_FIRST_MS, 4 * BACKOFF_FIRST_MS]
        );
    }

    #[test]
    fn a_successful_wake_clears_the_failures() {
        let mut b = Bubble::new();
        let mut bots = vec![bot("a", 8)];
        b.plan(0, Some(&sample(1, 90.0, &[1])), &bots, &line);
        bots[0].phase = Phase::Released;
        b.plan(1_000, Some(&sample(2, 20.0, &[1])), &bots, &line);
        let t = 1_000 + READMIT_AFTER_MS;
        let p = b.plan(t, Some(&sample(3, 20.0, &[1])), &bots, &line);
        assert_eq!(wakes(&p), vec!["a"]);
        // Too slow once.
        let p = b.plan(
            t + WAKE_DEADLINE_MS,
            Some(&sample(4, 20.0, &[1])),
            &bots,
            &line,
        );
        assert_eq!(releases(&p), vec!["a"]);
        assert_eq!(b.wakes["a"].fails, 1);
        let t = t + WAKE_DEADLINE_MS + WAKE_GAP_MS;
        let p = b.plan(t, Some(&sample(5, 20.0, &[1])), &bots, &line);
        assert_eq!(wakes(&p), vec!["a"]);
        bots[0].phase = Phase::Awake;
        b.plan(t + 3_000, Some(&sample(5, 20.0, &[1])), &bots, &line);
        assert!(!b.wakes.contains_key("a"));
        assert_eq!(b.shed_count(), 0);
    }

    #[test]
    fn without_a_sample_nothing_is_asked() {
        let mut b = Bubble::new();
        let bots = [bot("a", 8)];
        for t in 0..10 {
            let p = b.plan(t * 100_000, None, &bots, &line);
            assert!(p.orders.is_empty() && p.ledger.is_empty());
        }
    }

    #[test]
    fn a_bot_that_is_not_ours_to_wake_is_left_alone() {
        // Released by someone else (never shed by the planner) or with a
        // closed timetable window.
        let mut b = Bubble::new();
        let mut theirs = bot("theirs", 8);
        theirs.phase = Phase::Released;
        let mut bots = vec![theirs];
        b.plan(0, Some(&sample(1, 20.0, &[1])), &bots, &line);
        let p = b.plan(
            READMIT_AFTER_MS + 1,
            Some(&sample(2, 20.0, &[1])),
            &bots,
            &line,
        );
        assert!(wakes(&p).is_empty());
        // Ours, but its window is closed.
        let mut b = Bubble::new();
        bots = vec![bot("ours", 8)];
        b.plan(0, Some(&sample(1, 90.0, &[1])), &bots, &line);
        bots[0].phase = Phase::Released;
        bots[0].in_window = false;
        b.plan(1_000, Some(&sample(2, 20.0, &[1])), &bots, &line);
        let p = b.plan(
            READMIT_AFTER_MS + 1_000,
            Some(&sample(3, 20.0, &[1])),
            &bots,
            &line,
        );
        assert!(wakes(&p).is_empty());
    }

    /// **No paper**: every order is a move, a log-off or a log-on. This match
    /// has no arm that could carry ISK or an item, and adding an order kind
    /// fails to compile until someone says here what it does to the economy.
    fn of(name: &str, system: i64, role: RoleClass) -> BotView {
        BotView {
            role,
            ..bot(name, system)
        }
    }

    fn returns(p: &Plan) -> Vec<&str> {
        p.orders
            .iter()
            .filter_map(|o| match o {
                Order::Return { bot, .. } => Some(bot.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_trader_is_never_pulled_even_when_nearest() {
        let mut b = Bubble::new();
        let bots = [
            of("trader", 2, RoleClass::Other),
            of("miner", 5, RoleClass::Miner),
        ];
        let s = sample(1, 5.0, &[1]);
        b.plan(0, Some(&s), &bots, &line);
        let p = b.plan(SETTLE_MS, Some(&s), &bots, &line);
        assert_eq!(pulls(&p), vec![("miner", 1)]);
        assert_eq!(RoleClass::of("trader"), RoleClass::Other);
        assert_eq!(RoleClass::of("mine"), RoleClass::Miner);
        assert_eq!(RoleClass::of("freighter"), RoleClass::Hauler);
    }

    #[test]
    fn the_role_the_system_lacks_is_pulled_first() {
        let mut b = Bubble::new();
        let bots = [
            of("m1", 1, RoleClass::Miner),
            of("m2", 1, RoleClass::Miner),
            of("miner3", 2, RoleClass::Miner),
            of("hauler", 6, RoleClass::Hauler),
        ];
        let s = sample(1, 5.0, &[1]);
        b.plan(0, Some(&s), &bots, &line);
        let p = b.plan(SETTLE_MS, Some(&s), &bots, &line);
        assert_eq!(pulls(&p), vec![("hauler", 1)], "before a third miner");
    }

    #[test]
    fn a_system_without_belts_pulls_no_miner_and_a_dry_miner_stays() {
        let mut b = Bubble::new();
        let mut dry = of("dry", 2, RoleClass::Miner);
        dry.can_mine_now = false;
        let bots = [
            dry,
            of("miner", 3, RoleClass::Miner),
            of("escort", 7, RoleClass::Fighter),
        ];
        let mut s = sample(1, 5.0, &[1]);
        s.no_belts = vec![1];
        b.plan(0, Some(&s), &bots, &line);
        let p = b.plan(SETTLE_MS, Some(&s), &bots, &line);
        assert_eq!(pulls(&p), vec![("escort", 1)]);
        // With belts, the dry miner still stays; the other is the nearest.
        let mut b = Bubble::new();
        let s = sample(1, 5.0, &[1]);
        b.plan(0, Some(&s), &bots, &line);
        let p = b.plan(SETTLE_MS, Some(&s), &bots, &line);
        assert_eq!(pulls(&p), vec![("miner", 1)]);
    }

    #[test]
    fn a_lease_returns_after_the_human_has_been_gone_a_while() {
        let mut b = Bubble::new();
        let bots = [of("miner", 3, RoleClass::Miner)];
        let here = sample(1, 5.0, &[1]);
        b.plan(0, Some(&here), &bots, &line);
        let p = b.plan(SETTLE_MS, Some(&here), &bots, &line);
        assert_eq!(pulls(&p).len(), 1);
        // The human leaves (system 9 now); the clock starts.
        let away = sample(2, 5.0, &[9]);
        let t0 = SETTLE_MS + 10_000;
        let p = b.plan(t0, Some(&away), &bots, &line);
        assert!(returns(&p).is_empty());
        let p = b.plan(t0 + RETURN_AFTER_MS - 1, Some(&away), &bots, &line);
        assert!(returns(&p).is_empty());
        let p = b.plan(t0 + RETURN_AFTER_MS, Some(&away), &bots, &line);
        assert_eq!(returns(&p), vec!["miner"]);
        let p = b.plan(t0 + RETURN_AFTER_MS + 1, Some(&away), &bots, &line);
        assert!(returns(&p).is_empty(), "once");
    }

    #[test]
    fn a_returning_human_stops_the_clock_and_a_lease_has_a_ceiling() {
        let mut b = Bubble::new();
        let bots = [of("miner", 3, RoleClass::Miner)];
        let here = sample(1, 5.0, &[1]);
        let away = sample(2, 5.0, &[9]);
        b.plan(0, Some(&here), &bots, &line);
        b.plan(SETTLE_MS, Some(&here), &bots, &line);
        b.plan(SETTLE_MS + 1_000, Some(&away), &bots, &line);
        // He is back before the time is up: nothing at the old deadline.
        let back = SETTLE_MS + RETURN_AFTER_MS / 2;
        b.plan(back, Some(&here), &bots, &line);
        let p = b.plan(
            SETTLE_MS + RETURN_AFTER_MS + 2_000,
            Some(&here),
            &bots,
            &line,
        );
        assert!(returns(&p).is_empty());
        // He stays: the ceiling ends it anyway.
        let p = b.plan(SETTLE_MS + LEASE_MAX_MS, Some(&here), &bots, &line);
        assert_eq!(returns(&p), vec!["miner"]);
    }

    #[test]
    fn a_leased_bot_is_not_pulled_to_a_second_system() {
        let mut b = Bubble::new();
        let mut bots = vec![of("miner", 3, RoleClass::Miner)];
        let one = sample(1, 5.0, &[1]);
        b.plan(0, Some(&one), &bots, &line);
        let p = b.plan(SETTLE_MS, Some(&one), &bots, &line);
        assert_eq!(pulls(&p), vec![("miner", 1)]);
        // It arrives and the cooldown passes; a human also appears in 8.
        bots[0].system = Some(1);
        bots[0].leased_to = Some(60);
        let two = sample(2, 5.0, &[1, 8]);
        b.plan(SETTLE_MS + 1, Some(&two), &bots, &line);
        bots[0].system = Some(4);
        let t = SETTLE_MS + PULL_COOLDOWN_MS + 2 * SETTLE_MS;
        b.plan(t, Some(&two), &bots, &line);
        let p = b.plan(t + SETTLE_MS, Some(&two), &bots, &line);
        assert!(pulls(&p).is_empty(), "{p:?}");
    }

    #[test]
    fn no_order_carries_isk_or_items() {
        let all = [
            Order::Pull {
                bot: "a".into(),
                system: 1,
                jumps: 1,
                eta_s: 1,
                why: String::new(),
            },
            Order::Release {
                bot: "a".into(),
                why: String::new(),
            },
            Order::Wake {
                bot: "a".into(),
                why: String::new(),
            },
            Order::Return {
                bot: "a".into(),
                why: String::new(),
            },
        ];
        for o in &all {
            let effect = match o {
                Order::Pull { .. } => "travel by the gates",
                Order::Release { .. } => "log off through the session end (Park settles)",
                Order::Wake { .. } => "log on",
                Order::Return { .. } => "travel by the gates",
            };
            assert!(!effect.contains("isk") && !effect.contains("item"));
        }
    }
}
