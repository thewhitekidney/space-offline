//! **The hall's doctrine layer** (DOC-4): the glue between the pure leader
//! ([`crate::doctrine_lead`]), the pure theatre machine
//! ([`crate::doctrine_theatre`]) and the hall's members, books and stances.
//!
//! - [`HallState::doctrine_daily`] (from the hall's daily tick) runs each
//!   leader's [`daily`] choice. **Who leads** (R-DOCTRINE 4.1): an alliance
//!   whose executor corp has a bot CEO is led by that CEO and its book is the
//!   alliance's; an alliance without one leaves each bot corp its own book; a
//!   corp outside any alliance is led by its CEO. A coalition has no
//!   doctrine.
//! - [`HallState::theatre_tick`] (every `BATTLE_TICK_S`, by whoever drives
//!   the hall) reads the [`Board`], builds each leader's picture, feeds the
//!   memory, steps every theatre and queues the commander's actions
//!   ([`HallState::take_directives`]); the fleet commander executes them on
//!   its own connection (`fleet::orders_for`, [`crate::corpmind::command`]).
//!   Calls for backup are settled into relation events.
//! - **What DOC-5 publishes** into [`HallState::readiness`]: each bot's
//!   skills, ship, system and window ([`Readiness`]). Until then a bot is an
//!   online fighter at the staging system with a hull in the corp hangar and
//!   unknown skills.

use crate::corpmind::HallState;
use crate::doctrine::{Doctrine, Role};
use crate::doctrine_exchange::{Features, Matrix, matrix};
use crate::doctrine_lead::{
    self, BUDGET_DEFAULT, BUDGET_SHARE, DoctrineBook, Fancy, Inputs, LeadKnobs, Pilot,
    SUPPLY_UNKNOWN, daily,
};
use crate::doctrine_sense::{self, BATTLE_TICK_S, Board, Lens, SenseKnobs, Strength};
use crate::doctrine_theatre::{
    self, Action, Ally, Asset, CONF_MIN, Call, Clock, Ctx, Foe, Ship, Theatre, TheatreKnobs,
    er_with, foes_of, settle,
};
use crate::mind::Trait;
use crate::society::{self, Group, Stance5};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// `HallState::first` kind for kill evidence already folded into a book.
pub const OBS_DOC_KILL: u8 = 40;
/// Jumps assumed to a theatre when the bot has not reported them **[A]**.
pub const DEFAULT_JUMPS: u32 = 2;
/// Days of training per missing skill level, when the hall has no
/// skill-points table **[A]** (DOC-5 supplies the real figure).
pub const TRAIN_DAYS_PER_LEVEL: f64 = 1.0;
/// Calls kept open at most, and how long they wait for settlement.
pub const CALLS_KEEP: usize = 16;
pub const CALL_LIFE_S: u64 = 3_600;
/// The ISK a single hull is worth when a kill carries no value **[A]**.
pub const KILL_UNIT: f64 = 1.0;

/// **The catalog and what is computed from it once**: handed to the hall at
/// startup (`Hall::with_doctrine`).
#[derive(Debug, Clone)]
pub struct DoctrineWorld {
    pub catalog: Vec<Doctrine>,
    pub matrix: Matrix,
    pub lens: Lens,
    pub feat: Features,
    /// The real fits built for the catalog's hulls, for a bot's own pick.
    pub seeds: crate::doctrine_real::RealSeeds,
}

impl DoctrineWorld {
    pub fn new(catalog: Vec<Doctrine>, lens: Lens, feat: Features) -> DoctrineWorld {
        let matrix = matrix(&catalog, feat);
        DoctrineWorld {
            catalog,
            matrix,
            lens,
            feat,
            seeds: Default::default(),
        }
    }

    /// **The world from the client's static data** (`eve-bots --content`):
    /// the catalog over what the world can make (`Makeable::from_sources`:
    /// NPC-sold, builder-made, found as a data-site copy or dropped; every
    /// type only when the recipes or the exchange are absent), priced at
    /// `P_ref`, and the lens over every part so a foe's unmakeable kit is
    /// still recognised. `None` without type data in the content.
    pub fn from_content(
        content: &eve_server::content::Content,
        feat: Features,
    ) -> Option<DoctrineWorld> {
        Self::from_content_with(content, feat, None)
    }

    /// [`DoctrineWorld::from_content`] seeded with what players fly
    /// (`real`, `eve-bots --real-fits`).
    pub fn from_content_with(
        content: &eve_server::content::Content,
        feat: Features,
        real: Option<&crate::real_fits::RealFits>,
    ) -> Option<DoctrineWorld> {
        let installed = content.try_get::<eve_server::typedata::Installed>()?;
        let prices = content.try_get::<eve_server::econ_prices::Table>();
        let price = |t: i64| {
            prices
                .and_then(|p| p.prices.get(&t))
                .map_or(0.0, |p| p.ref_price)
        };
        let makeable = Self::makeable_of(content);
        let src = crate::doctrine::Source {
            installed,
            makeable: &makeable,
            price: &price,
            limits: crate::doctrine_fit::Limits::default(),
            real_fits: real,
        };
        let (catalog, seeds) = crate::doctrine::catalog_seeded(&src);
        // The lens keeps every part: an enemy's unmakeable kit is still
        // recognised when it is seen.
        let all = crate::doctrine::Makeable::all();
        let lib = crate::doctrine_fit::Library::build(installed, &all, &price, src.limits);
        let mut world = DoctrineWorld::new(catalog, Lens::of_library(&lib), feat);
        world.seeds = seeds;
        Some(world)
    }

    /// **What the world can make** (nothing from nowhere): the sources the
    /// content tables give. `Makeable::all()` only when there are no
    /// recipes or no exchange to ask.
    pub fn makeable_of(content: &eve_server::content::Content) -> crate::doctrine::Makeable {
        use eve_server::{econ_exchange, industry};
        let recipes = content
            .try_get::<industry::Table>()
            .filter(|t| !t.blueprints.is_empty());
        let sold = content
            .try_get::<econ_exchange::Table>()
            .map(|t| t.asked_types());
        let (Some(recipes), Some(sold)) = (recipes, sold) else {
            return crate::doctrine::Makeable::all();
        };
        let copies = crate::doctrine_parts::copies_of(content);
        let dropped = crate::doctrine_parts::dropped_of(content);
        let sourcing = crate::builder::Sourcing::new(content, recipes);
        crate::doctrine::Makeable::from_sources(&crate::doctrine_parts::Sources {
            recipes,
            sold: Some(&sold),
            dropped: Some(&dropped),
            copies: Some(&copies),
            sourcing: Some(&sourcing),
        })
    }

    pub fn find(&self, key: &str) -> Option<usize> {
        self.catalog.iter().position(|d| d.key == key)
    }
}

impl PartialEq for DoctrineWorld {
    fn eq(&self, o: &DoctrineWorld) -> bool {
        self.catalog == o.catalog
    }
}

/// **What a bot reports of itself for the doctrine layer** (DOC-5 fills it
/// at each turn; `HallState::report_readiness`).
#[derive(Debug, Clone, PartialEq)]
pub struct Readiness {
    /// Skill levels; `None`: not reported.
    pub skills: Option<BTreeMap<i64, i64>>,
    /// Where the bot is; `None`: at the staging system.
    pub system: Option<i64>,
    pub online: bool,
    /// Seconds to its next window when offline.
    pub wake_in_s: Option<u64>,
    /// The doctrine ship it holds; `None`: a hull in the corp hangar.
    pub ship: Option<Ship>,
    /// Jumps to systems it knows the way to.
    pub jumps_to: BTreeMap<i64, u32>,
    pub at: u64,
}

impl Default for Readiness {
    fn default() -> Readiness {
        Readiness {
            skills: None,
            system: None,
            online: true,
            wake_in_s: None,
            ship: None,
            jumps_to: BTreeMap::new(),
            at: 0,
        }
    }
}

/// A doctrine leader: the group the book belongs to, the character that
/// decides, and the bot corps it commands.
#[derive(Debug, Clone, PartialEq)]
pub struct Leader {
    pub group: Group,
    pub ceo: i64,
    pub name: String,
    pub corps: Vec<i64>,
}

/// Is a career one that flies fighting ships?
pub fn is_fighter(career: &str) -> bool {
    matches!(
        society::family_of(career),
        Some(society::Family::Rat | society::Family::Fight)
    )
}

impl HallState {
    /// **The leaders** (module header), by group.
    pub fn doctrine_leaders(&self) -> Vec<Leader> {
        let bots = self.bot_corps();
        let ceo_of = |corp: i64| {
            self.corps
                .get(&corp)
                .and_then(|c| c.ceo)
                .filter(|c| self.members.contains_key(c))
        };
        let name_of = |ceo: i64| {
            self.members
                .get(&ceo)
                .map(|m| m.name.clone())
                .unwrap_or_default()
        };
        let mut out = Vec::new();
        let alliances: BTreeSet<i64> = bots
            .iter()
            .filter_map(|c| self.corps.get(c).and_then(|m| m.alliance))
            .collect();
        for al in alliances {
            let corps: Vec<i64> = bots
                .iter()
                .copied()
                .filter(|c| self.corps[c].alliance == Some(al))
                .collect();
            let exec_ceo = self
                .alliances
                .get(&al)
                .and_then(|a| a.executor)
                .and_then(ceo_of);
            match exec_ceo {
                Some(ceo) => out.push(Leader {
                    group: al,
                    ceo,
                    name: name_of(ceo),
                    corps,
                }),
                None => {
                    for c in corps {
                        if let Some(ceo) = ceo_of(c) {
                            out.push(Leader {
                                group: c,
                                ceo,
                                name: name_of(ceo),
                                corps: vec![c],
                            });
                        }
                    }
                }
            }
        }
        for c in bots {
            if self.corps[&c].alliance.is_some() {
                continue;
            }
            if let Some(ceo) = ceo_of(c) {
                out.push(Leader {
                    group: c,
                    ceo,
                    name: name_of(ceo),
                    corps: vec![c],
                });
            }
        }
        out.sort_by_key(|l| l.group);
        out
    }

    /// **The book that governs a corp**: its alliance's when the alliance has
    /// one, else its own. What DOC-5 reads to choose a hull.
    pub fn doctrine_of(&self, corp: i64) -> Option<&DoctrineBook> {
        let al = self.corps.get(&corp).and_then(|c| c.alliance);
        al.and_then(|a| self.dbooks.get(&a))
            .or_else(|| self.dbooks.get(&corp))
    }

    /// The catalog entry a corp is meant to fly.
    pub fn doctrine_key_of(&self, corp: i64) -> Option<&str> {
        self.doctrine_of(corp).and_then(|b| b.current.as_deref())
    }

    /// A bot reports its readiness.
    pub fn report_readiness(&mut self, char_id: i64, r: Readiness) {
        self.readiness.insert(char_id, r);
    }

    fn fighters(&self, l: &Leader) -> Vec<(i64, &crate::corpmind::Member)> {
        self.members
            .iter()
            .filter(|(_, m)| l.corps.contains(&m.corp) && is_fighter(&m.career))
            .map(|(c, m)| (*c, m))
            .collect()
    }

    fn view(&self, l: &Leader, target: Group) -> f64 {
        if society::PLAYER_ALLIANCES.contains(&l.group) {
            self.alliance_relation(l.group, target)
        } else {
            self.relations.get(l.group, target)
        }
    }

    fn is_allied(&self, l: &Leader, other: Group) -> bool {
        other == l.group
            || l.corps.contains(&other)
            || matches!(
                self.stances.get5(l.group, other),
                Stance5::Friendly | Stance5::Allied
            )
    }

    fn knobs(&self) -> (LeadKnobs, TheatreKnobs, SenseKnobs) {
        (self.knobs.lead, self.knobs.theatre, self.knobs.sense)
    }

    /// **The daily choice for every leader** (R-DOCTRINE 4). Returns the
    /// `[doc]` lines. Nothing without a [`DoctrineWorld`].
    pub fn doctrine_daily(&mut self, now: u64) -> Vec<String> {
        let Some(world) = self.doctrine.clone() else {
            return Vec::new();
        };
        let (lead, _, sense) = self.knobs();
        let mut lines = Vec::new();
        for l in self.doctrine_leaders() {
            let (pilots, wallet) = {
                let f = self.fighters(&l);
                let pilots: Vec<Pilot> = f
                    .iter()
                    .map(|(c, _)| Pilot {
                        levels: self.readiness.get(c).and_then(|r| r.skills.clone()),
                        human: false,
                    })
                    .collect();
                let wallet: f64 = f
                    .iter()
                    .filter_map(|(c, _)| self.pool.wallets.get(c).map(|w| w.0))
                    .sum();
                (pilots, wallet)
            };
            let traits = self
                .members
                .get(&l.ceo)
                .map(|m| m.traits)
                .unwrap_or(crate::mind::Traits([0.5; crate::mind::TRAIT_COUNT]));
            let seed = self.doctrine_seed;
            let day = self.day;
            let book = self
                .dbooks
                .entry(l.group)
                .or_insert_with(|| DoctrineBook::new(Fancy::draw(&l.name, seed)));
            let train = |p: &Pilot, lack: &[(i64, i64)]| -> f64 {
                let have = p.levels.as_ref();
                lack.iter()
                    .map(|(s, need)| {
                        let h = have.and_then(|m| m.get(s)).copied().unwrap_or(0);
                        f64::from((need - h).max(1) as i32) * TRAIN_DAYS_PER_LEVEL
                    })
                    .sum()
            };
            let supply = |_: &Doctrine| SUPPLY_UNKNOWN;
            let rearm = |_: &str| 0.0;
            let inp = Inputs {
                now_s: now,
                day_s: day,
                seed,
                group: l.group,
                catalog: &world.catalog,
                matrix: &world.matrix,
                lens: &world.lens,
                sense,
                pilots: &pilots,
                train_days: &train,
                supply: &supply,
                rearm: &rearm,
                budget: if wallet > 0.0 {
                    BUDGET_SHARE * wallet
                } else {
                    BUDGET_DEFAULT
                },
                traits,
            };
            let d = daily(book, &inp, &lead);
            let line = doctrine_lead::line(book, &inp, &lead, &d);
            if matches!(
                d,
                doctrine_lead::Decision::First { .. } | doctrine_lead::Decision::Switch { .. }
            ) {
                self.dirty = true;
                self.act(line.clone());
            }
            lines.push(line);
        }
        lines
    }

    /// The corp members of a leader, as the machine's assets for `system`.
    fn assets_for(&self, l: &Leader, system: i64) -> Vec<Asset> {
        self.fighters(l)
            .iter()
            .map(|(c, _)| {
                let r = self.readiness.get(c).cloned().unwrap_or_default();
                let jumps = match r.system {
                    Some(s) if s == system => Some(0),
                    _ => r.jumps_to.get(&system).copied().or(Some(DEFAULT_JUMPS)),
                };
                Asset {
                    char_id: *c,
                    online: r.online,
                    wake_in_s: r.wake_in_s,
                    jumps,
                    ship: r.ship.unwrap_or(Ship::Hangar),
                }
            })
            .collect()
    }

    /// **One battle tick for every leader** (R-DOCTRINE 7): read the board,
    /// build the picture of each system the leader's fighters sit in, feed
    /// its memory, step its theatres and settle its calls. Returns the
    /// `[doc]` lines; the commander's actions wait in
    /// [`HallState::take_directives`].
    pub fn theatre_tick(&mut self, now: u64, board: &Board) -> Vec<String> {
        let Some(world) = self.doctrine.clone() else {
            return Vec::new();
        };
        let (_, tk, sense) = self.knobs();
        let clock = Clock { day_s: self.day };
        let tick = now / BATTLE_TICK_S;
        let mut lines = Vec::new();
        let evidence = board.evidence();
        let leaders = self.doctrine_leaders();
        for l in &leaders {
            if self
                .dbooks
                .get(&l.group)
                .is_none_or(|b| b.current.is_none())
            {
                continue;
            }
            let mine_groups: BTreeSet<Group> = l.corps.iter().copied().chain([l.group]).collect();
            let allied: BTreeSet<Group> = leaders
                .iter()
                .filter(|o| self.is_allied(l, o.group))
                .flat_map(|o| o.corps.iter().copied().chain([o.group]))
                .collect();
            let is_mine = |g: Group| mine_groups.contains(&g) || allied.contains(&g);
            // Kill evidence, once each, into the memory and the ledger.
            for ev in &evidence {
                if !self.first(OBS_DOC_KILL, l.group, ev.kill_id) {
                    continue;
                }
                let lost = is_mine(ev.victim_group);
                let killed = !lost && ev.attackers.iter().any(|a| is_mine(a.group));
                if let Some(b) = self.dbooks.get_mut(&l.group) {
                    b.memory.note_kill(ev, &world.lens, now, &is_mine, &sense);
                    if lost || killed {
                        b.losses.note(
                            if lost { KILL_UNIT } else { 0.0 },
                            if killed { KILL_UNIT } else { 0.0 },
                            now,
                            self.day,
                        );
                    }
                }
                if lost {
                    for t in self.theatres.entry(l.group).or_default().iter_mut() {
                        t.losses.note(now, KILL_UNIT, 0.0);
                    }
                } else if killed {
                    for t in self.theatres.entry(l.group).or_default().iter_mut() {
                        t.losses.note(now, 0.0, KILL_UNIT);
                    }
                }
            }
            let systems: BTreeSet<i64> = self
                .fighters(l)
                .iter()
                .filter_map(|(c, _)| self.readiness.get(c).and_then(|r| r.system))
                .chain(
                    self.theatres
                        .get(&l.group)
                        .into_iter()
                        .flatten()
                        .map(|t| t.system),
                )
                .collect();
            let cur_key = self.dbooks[&l.group].current.clone().unwrap_or_default();
            let Some(ci) = world.find(&cur_key) else {
                continue;
            };
            for system in systems {
                let sightings = board.sightings(system, now);
                let strength = Strength::default();
                let picture = {
                    let book = &self.dbooks[&l.group];
                    doctrine_sense::observe(
                        &sightings,
                        &doctrine_sense::Inputs {
                            system,
                            now_s: now,
                            tick,
                            seed: self.doctrine_seed,
                            mine: &strength,
                            is_mine: &is_mine,
                            memory: &book.memory,
                            knobs: sense,
                        },
                    )
                };
                let hostile = |g: Group| {
                    g != doctrine_sense::UNATTRIBUTED
                        && !is_mine(g)
                        && self.view(l, g) <= society::WARY_AT
                };
                let (comps, conf, counter) = {
                    let book = &self.dbooks[&l.group];
                    let (comps, conf) = foes_of(
                        &picture,
                        &book.memory,
                        &world.lens,
                        &sense,
                        &world.catalog,
                        &hostile,
                    );
                    let foes: Vec<Foe<'_>> = comps
                        .iter()
                        .map(|(i, n)| Foe {
                            doc: &world.catalog[*i],
                            ships: *n,
                        })
                        .collect();
                    let n = doctrine_lead::fleet_n(&world.catalog[ci], self.fighters(l).len());
                    let counter = best_counter(&world, ci, &foes, n, tk.engage_er);
                    (comps, conf, counter)
                };
                if !picture.blind
                    && let Some(b) = self.dbooks.get_mut(&l.group)
                {
                    b.memory.note_picture(&picture, &sense);
                }
                let existing = self
                    .theatres
                    .get(&l.group)
                    .is_some_and(|v| v.iter().any(|t| t.system == system));
                if !existing {
                    let open = self.theatres.get(&l.group).map_or(0, |v| v.len());
                    if comps.is_empty() || open >= tk.max_theatres || conf < CONF_MIN {
                        continue;
                    }
                    self.theatres
                        .entry(l.group)
                        .or_default()
                        .push(Theatre::new(system, now));
                }
                let assets = self.assets_for(l, system);
                let foes: Vec<Foe<'_>> = comps
                    .iter()
                    .map(|(i, n)| Foe {
                        doc: &world.catalog[*i],
                        ships: *n,
                    })
                    .collect();
                let ctx = Ctx {
                    now_s: now,
                    tick,
                    seed: self.doctrine_seed,
                    clock,
                    knobs: &tk,
                    mine: &world.catalog[ci],
                    foes: &foes,
                    conf,
                    assets: &assets,
                    contested: !comps.is_empty(),
                    counter: counter.as_deref(),
                    feat: world.feat,
                };
                let th = self
                    .theatres
                    .get_mut(&l.group)
                    .and_then(|v| v.iter_mut().find(|t| t.system == system))
                    .expect("opened");
                let before = th.state;
                let actions = doctrine_theatre::step(th, &ctx);
                let after = th.state;
                if before != after || !actions.is_empty() {
                    lines.push(format!(
                        "[doc] theatre group={} system={} {} -> {} conf={:.2} foes={:.0} actions={}",
                        l.group,
                        system,
                        before.name(),
                        after.name(),
                        conf,
                        foes.iter().map(|f| f.ships).sum::<f64>(),
                        actions.len()
                    ));
                }
                for a in actions {
                    self.apply_action(l, system, now, a);
                }
            }
            // Drop theatres that went quiet.
            if let Some(v) = self.theatres.get_mut(&l.group) {
                v.retain(|t| {
                    !(matches!(t.state, doctrine_theatre::State::Watch)
                        && now.saturating_sub(t.changed_s) > 6 * 3_600)
                });
            }
        }
        lines.extend(self.settle_calls(now, &leaders, &world, tk));
        lines
    }

    /// **Queue one action for a commander** (what [`HallState::theatre_tick`]
    /// does for each action the machine answers; public so a caller can
    /// raise a call for backup by hand). Updates the books it touches.
    pub fn enqueue(&mut self, commander: i64, group: Group, system: i64, now: u64, a: Action) {
        let l = Leader {
            group,
            ceo: commander,
            name: String::new(),
            corps: Vec::new(),
        };
        self.apply_action(&l, system, now, a);
    }

    fn apply_action(&mut self, l: &Leader, system: i64, now: u64, a: Action) {
        match &a {
            Action::Escalate => {
                let day = self.day;
                if let Some(b) = self.dbooks.get_mut(&l.group) {
                    b.escalate(now, day);
                }
            }
            Action::Rebuild { counter: Some(k) } => {
                self.stock_wanted.insert(l.group, k.clone());
            }
            Action::NeedBackup { .. } => {
                if self.calls.len() >= CALLS_KEEP {
                    self.calls.remove(0);
                }
                self.calls.push(Call::new(l.group, system, now));
            }
            _ => {}
        }
        self.directives.entry(l.ceo).or_default().push(a);
    }

    /// **The commander's pending actions**, oldest first (cleared).
    pub fn take_directives(&mut self, commander: i64) -> Vec<Action> {
        self.directives.remove(&commander).unwrap_or_default()
    }

    /// Open calls for backup become events (R-DOCTRINE 7.4).
    fn settle_calls(
        &mut self,
        now: u64,
        leaders: &[Leader],
        world: &Arc<DoctrineWorld>,
        tk: TheatreKnobs,
    ) -> Vec<String> {
        let clock = Clock { day_s: self.day };
        let mut lines = Vec::new();
        let mut calls = std::mem::take(&mut self.calls);
        for call in &mut calls {
            let Some(asker) = leaders.iter().find(|l| l.group == call.asker) else {
                continue;
            };
            let allies: Vec<Ally> = leaders
                .iter()
                .filter(|o| o.group != asker.group && self.is_allied(asker, o.group))
                .map(|o| {
                    let fighters = self.fighters(o);
                    let aggression = if fighters.is_empty() {
                        0.0
                    } else {
                        fighters
                            .iter()
                            .map(|(_, m)| m.traits.get(Trait::Aggression))
                            .sum::<f64>()
                            / fighters.len() as f64
                    };
                    let assets = self.assets_for(o, call.system);
                    let av = doctrine_theatre::available(
                        &assets,
                        &clock,
                        &tk,
                        self.doctrine_seed,
                        now / BATTLE_TICK_S,
                    );
                    let arrived = fighters.iter().any(|(c, _)| {
                        self.readiness
                            .get(c)
                            .is_some_and(|r| r.system == Some(call.system) && r.at >= call.at_s)
                    });
                    Ally {
                        group: o.group,
                        r_to_asker: self.view(o, asker.group),
                        aggression,
                        available: av.len(),
                        er: self.er_of(world, o, call, av.len()),
                        arrived,
                    }
                })
                .collect();
            for e in settle(call, &allies, now, 0.0) {
                lines.push(format!("[doc] help {e:?}"));
                self.record(&e);
            }
        }
        calls.retain(|c| now.saturating_sub(c.at_s) <= CALL_LIFE_S);
        self.calls = calls;
        lines
    }

    /// `ER` of an ally's available ships against the enemy of the theatre
    /// the call came from (its own doctrine, else the asker's).
    fn er_of(&self, world: &DoctrineWorld, ally: &Leader, call: &Call, n: usize) -> f64 {
        let key = self
            .dbooks
            .get(&ally.group)
            .and_then(|b| b.current.clone())
            .or_else(|| self.dbooks.get(&call.asker).and_then(|b| b.current.clone()));
        let Some(i) = key.and_then(|k| world.find(&k)) else {
            return 0.0;
        };
        let Some(book) = self.dbooks.get(&call.asker) else {
            return 0.0;
        };
        let (_, _, sense) = self.knobs();
        let enemies = doctrine_lead::enemy_mix(
            book,
            &Inputs {
                now_s: call.at_s,
                day_s: self.day,
                seed: 0,
                group: call.asker,
                catalog: &world.catalog,
                matrix: &world.matrix,
                lens: &world.lens,
                sense,
                pilots: &[],
                train_days: &|_, _| 0.0,
                supply: &|_| 1.0,
                rearm: &|_| 0.0,
                budget: 1.0,
                traits: crate::mind::Traits([0.5; crate::mind::TRAIT_COUNT]),
            },
        );
        if enemies.is_empty() {
            return 0.0;
        }
        let foes: Vec<Foe<'_>> = enemies
            .iter()
            .filter_map(|e| {
                e.mix.first().map(|(j, _)| Foe {
                    doc: &world.catalog[*j],
                    ships: e.weight * 10.0,
                })
            })
            .collect();
        er_with(&world.catalog[i], n as f64, &foes, world.feat)
    }
}

/// The best counter to the foes among the catalog (not the current
/// doctrine): the entry whose `ER` with a fleet of `n` clears
/// `engage_er`. `None` when none does.
pub fn best_counter(
    world: &DoctrineWorld,
    current: usize,
    foes: &[Foe<'_>],
    n: f64,
    engage_er: f64,
) -> Option<String> {
    if foes.is_empty() {
        return None;
    }
    let mut best: Option<(f64, usize)> = None;
    for (i, d) in world.catalog.iter().enumerate() {
        if i == current || d.share(Role::Dps) <= 0.0 {
            continue;
        }
        let er = er_with(d, n, foes, world.feat);
        if er >= engage_er && best.is_none_or(|b| er > b.0) {
            best = Some((er, i));
        }
    }
    best.map(|(_, i)| world.catalog[i].key.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::career::Band;
    use crate::corpmind::{CorpMind, Member};
    use crate::doctrine::testkit::invented_catalog;
    use crate::doctrine::{Family, Layer, Range, Stance};
    use crate::doctrine_fit::Kind;
    use crate::doctrine_sense::{EnemyMemory, GridShip, Seen, Sighting};
    use crate::hull_choice::Class;
    use crate::mind::{TRAIT_COUNT, Traits};
    use crate::society::{Held5, Profile, Satisfaction};

    const DAY: u64 = 1_000;
    const C1: i64 = 98_000_301;
    const C2: i64 = 98_000_302;
    const C3: i64 = 98_000_303;
    const AL: i64 = 99_000_301;
    const FOE: Group = 98_700_301;
    const SYS: i64 = 30_000_301;
    const ENEMY_HULL: i64 = 977;
    const ENEMY_GUN: i64 = 988;
    const ENEMY_PLATE: i64 = 989;
    const NOW: u64 = 10 * DAY;

    fn member(name: &str, corp: i64, career: &str) -> Member {
        Member {
            name: name.to_owned(),
            traits: Traits([0.6; TRAIT_COUNT]),
            career: career.to_owned(),
            band: Band::High,
            corp,
            alliance: None,
            last_turn: Some(NOW),
            satisfaction: Satisfaction::default(),
            applied: None,
            salt: 0,
        }
    }

    fn corp(id: i64, ceo: i64) -> CorpMind {
        CorpMind {
            id,
            ceo: Some(ceo),
            profile: Profile::of("ratter", Band::High),
            founded: 0,
            alliance: None,
            name: format!("Invented {id}"),
            applied_alliance: None,
            salt: 0,
        }
    }

    /// The invented catalog with labels the lens can match.
    fn world() -> Arc<DoctrineWorld> {
        let mut cat = invented_catalog();
        let labels = [
            (Layer::Shield, Range::Long, Family::Hybrid, Class::Cruiser),
            (
                Layer::Armour,
                Range::Short,
                Family::Projectile,
                Class::Cruiser,
            ),
            (Layer::Shield, Range::Short, Family::Hybrid, Class::Cruiser),
            (
                Layer::Shield,
                Range::Medium,
                Family::Laser,
                Class::Battleship,
            ),
            (
                Layer::Armour,
                Range::Medium,
                Family::Missile,
                Class::Frigate,
            ),
            (
                Layer::Armour,
                Range::Short,
                Family::Projectile,
                Class::Destroyer,
            ),
        ];
        for (d, (l, r, f, k)) in cat.iter_mut().zip(labels) {
            d.layer = l;
            d.range = r;
            d.family = f;
            d.class = k;
            d.stance = Stance::Skirmish;
            d.fleet.target = 10;
            for role in &mut d.roles {
                role.2.cost = 100.0;
            }
        }
        let mut lens = Lens::default();
        lens.hull_class.insert(ENEMY_HULL, Class::Cruiser);
        lens.module_kind
            .insert(ENEMY_GUN, Kind::Gun(Family::Projectile, 2));
        lens.module_kind.insert(ENEMY_PLATE, Kind::Plate);
        Arc::new(DoctrineWorld::new(cat, lens, Features::default()))
    }

    /// `n` ratters in `corp` (ids from `base`), the first its CEO.
    fn crew(st: &mut HallState, corp_id: i64, base: i64, n: i64) {
        for i in 0..n {
            st.members
                .insert(base + i, member(&format!("P{base}-{i}"), corp_id, "ratter"));
        }
        st.corps.insert(corp_id, corp(corp_id, base));
    }

    fn awake(n: i64) -> HallState {
        let mut st = HallState::new(DAY);
        crew(&mut st, C2, 90_300_000, n);
        st.doctrine = Some(world());
        st.every = 0;
        st
    }

    #[test]
    fn leaders_follow_the_executor_and_a_corp_outside_an_alliance_leads_itself() {
        let mut st = HallState::new(DAY);
        crew(&mut st, C1, 90_100_000, 2);
        crew(&mut st, C2, 90_200_000, 2);
        crew(&mut st, C3, 90_300_000, 2);
        for c in [C1, C2] {
            st.corps.get_mut(&c).unwrap().alliance = Some(AL);
        }
        // The alliance has no known executor: each bot corp leads itself.
        st.alliances.insert(AL, Default::default());
        st.doctrine = Some(world());
        let g: Vec<Group> = st.doctrine_leaders().iter().map(|l| l.group).collect();
        assert_eq!(g, vec![C1, C2, C3]);
        // With a bot executor the alliance has one leader, its CEO.
        st.alliances.get_mut(&AL).unwrap().executor = Some(C2);
        let l = st.doctrine_leaders();
        assert_eq!(l.iter().map(|l| l.group).collect::<Vec<_>>(), vec![C3, AL]);
        let al = l.iter().find(|l| l.group == AL).unwrap();
        assert_eq!(al.ceo, 90_200_000);
        assert_eq!(al.corps, vec![C1, C2]);
        // An executor with no bot CEO: back to corp level.
        st.corps.get_mut(&C2).unwrap().ceo = None;
        let g: Vec<Group> = st.doctrine_leaders().iter().map(|l| l.group).collect();
        assert_eq!(g, vec![C1, C3]);
        // The book that governs a corp is its alliance's when there is one.
        st.corps.get_mut(&C2).unwrap().ceo = Some(90_200_000);
        st.doctrine_daily(NOW);
        assert!(st.dbooks.contains_key(&AL) && st.dbooks.contains_key(&C3));
        assert!(!st.dbooks.contains_key(&C1));
        assert_eq!(st.doctrine_of(C1), st.dbooks.get(&AL));
        assert_eq!(st.doctrine_of(C3), st.dbooks.get(&C3));
    }

    #[test]
    fn the_daily_tick_gives_each_leader_a_book_that_persists() {
        let mut st = awake(6);
        let lines = st.doctrine_daily(NOW);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].starts_with("[doc] lead group=98000302"),
            "{}",
            lines[0]
        );
        let b = st.dbooks.get(&C2).expect("a book");
        assert!(b.current.is_some() && b.since == NOW);
        let key = b.current.clone();
        assert!(st.acts.iter().any(|a| a.contains("decision=First")));
        // A second tick keeps it.
        st.doctrine_daily(NOW + DAY);
        assert_eq!(st.dbooks[&C2].current, key);
        // Round trip through the state file.
        let text = st.render();
        let back = HallState::parse(&text, DAY).expect("parses");
        let (a, b) = (&st.dbooks[&C2], &back.dbooks[&C2]);
        assert_eq!(
            (&a.current, a.since, a.fancy),
            (&b.current, b.since, b.fancy)
        );
        assert_eq!(a.last_switch, b.last_switch);
        // No catalog: the layer sleeps.
        let mut asleep = awake(6);
        asleep.doctrine = None;
        assert!(asleep.doctrine_daily(NOW).is_empty() && asleep.dbooks.is_empty());
    }

    /// A hall with a leader that flies the kiters, an enemy group it is
    /// hostile to and has met (brick: armour, projectile cruisers), and a
    /// board that shows `ships` of them on the grid at `SYS`.
    fn at_war(n: i64, ships: usize) -> (HallState, Board) {
        let mut st = awake(n);
        st.doctrine_daily(NOW);
        let w = st.doctrine.clone().unwrap();
        st.dbooks.get_mut(&C2).unwrap().current = Some(w.catalog[0].key.clone());
        let mut e = EnemyMemory::new(FOE);
        e.conf = 0.9;
        e.seen.insert(
            ENEMY_HULL,
            Seen {
                n_max: 4,
                last_s: NOW,
                weapons: [ENEMY_GUN].into(),
                tanks: [(Layer::Armour, 3)].into(),
                utility: BTreeMap::new(),
            },
        );
        st.dbooks.get_mut(&C2).unwrap().memory.groups.insert(FOE, e);
        st.relations.add(C2, FOE, -5.0);
        for i in 0..n {
            st.report_readiness(
                90_300_000 + i,
                Readiness {
                    system: Some(SYS),
                    ship: Some(Ship::Staged),
                    at: NOW,
                    ..Readiness::default()
                },
            );
        }
        let board = Board::default();
        post(&board, NOW, ships);
        (st, board)
    }

    fn post(board: &Board, at_s: u64, ships: usize) {
        board.post(Sighting {
            observer: 90_300_000,
            system: SYS,
            at_s,
            scan: None,
            grid: (0..ships)
                .map(|_| GridShip {
                    hull: ENEMY_HULL,
                    group: FOE,
                })
                .collect(),
            local: None,
            spotted: true,
        });
    }

    /// Tick every 30 s from `from` for `secs`, the board kept fresh.
    fn run(st: &mut HallState, board: &Board, from: u64, secs: u64, ships: usize) -> Vec<Action> {
        let mut out = Vec::new();
        let mut t = from;
        while t < from + secs {
            post(board, t, ships);
            st.theatre_tick(t, board);
            out.extend(st.take_directives(90_300_000));
            t += 30;
        }
        out
    }

    #[test]
    fn a_hostile_fleet_on_the_grid_opens_a_theatre_and_the_leader_commits() {
        let (mut st, board) = at_war(8, 6);
        let acts = run(&mut st, &board, NOW + 30, 1_200, 6);
        assert!(
            acts.iter()
                .any(|a| matches!(a, Action::Scout { system } if *system == SYS)),
            "{acts:?}"
        );
        let who: Vec<i64> = acts
            .iter()
            .filter_map(|a| match a {
                Action::Commit { who } => Some(who.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert!(!who.is_empty(), "{acts:?}");
        assert!(who.len() < 8, "not everyone: {who:?}");
        let th = &st.theatres[&C2];
        assert_eq!(th.len(), 1);
        assert_eq!(th[0].system, SYS);
    }

    #[test]
    fn a_friendly_or_unknown_group_is_no_foe() {
        let (mut st, board) = at_war(8, 6);
        st.relations.r.clear();
        let acts = run(&mut st, &board, NOW + 30, 600, 6);
        assert!(acts.is_empty(), "{acts:?}");
        assert!(st.theatres.get(&C2).is_none_or(|v| v.is_empty()));
    }

    #[test]
    fn max_theatres_bounds_the_fronts() {
        let (mut st, board) = at_war(8, 6);
        // A second system with enemies; the normal leader holds one theatre.
        let sys2 = SYS + 1;
        st.report_readiness(
            90_300_001,
            Readiness {
                system: Some(sys2),
                ship: Some(Ship::Staged),
                at: NOW,
                ..Readiness::default()
            },
        );
        board.post(Sighting {
            observer: 90_300_001,
            system: sys2,
            at_s: NOW + 30,
            grid: vec![
                GridShip {
                    hull: ENEMY_HULL,
                    group: FOE
                };
                4
            ],
            spotted: true,
            ..Sighting::default()
        });
        post(&board, NOW + 30, 6);
        st.theatre_tick(NOW + 30, &board);
        assert_eq!(st.theatres[&C2].len(), TheatreKnobs::NORMAL.max_theatres);
    }

    #[test]
    fn a_call_that_goes_unanswered_costs_the_ally_the_askers_trust() {
        let mut st = HallState::new(DAY);
        crew(&mut st, C2, 90_300_000, 4);
        crew(&mut st, C3, 90_400_000, 4);
        st.doctrine = Some(world());
        st.doctrine_daily(NOW);
        // C3 is C2's friend and views C2 as one (so it could be asked).
        st.stances.graded.insert(
            (C2, C3),
            Held5 {
                stance: Stance5::Friendly,
                since: Some(0),
            },
        );
        st.relations
            .add(C3, C2, crate::doctrine_theatre::HELP_AT + 1.0);
        st.calls.push(Call::new(C2, SYS, NOW));
        let board = Board::default();
        // Before the wait nothing happens.
        st.theatre_tick(NOW + crate::doctrine_theatre::HELP_WAIT_S - 60, &board);
        assert_eq!(st.relations.get(C2, C3), 0.0);
        // After it, the asker's view of the absent ally falls; the ally's does not.
        st.theatre_tick(NOW + crate::doctrine_theatre::HELP_WAIT_S + 30, &board);
        assert!(st.relations.get(C2, C3) <= -society::K_ABANDON + 1e-9);
        assert!(st.relations.get(C3, C2) >= crate::doctrine_theatre::HELP_AT + 1.0 - 1e-9);
        // The settled ally is not charged twice.
        let before = st.relations.get(C2, C3);
        st.theatre_tick(NOW + crate::doctrine_theatre::HELP_WAIT_S + 120, &board);
        assert_eq!(st.relations.get(C2, C3), before);
    }

    #[test]
    fn an_ally_that_arrives_earns_both_views() {
        let mut st = HallState::new(DAY);
        crew(&mut st, C2, 90_300_000, 4);
        crew(&mut st, C3, 90_400_000, 4);
        st.doctrine = Some(world());
        st.doctrine_daily(NOW);
        st.stances.graded.insert(
            (C2, C3),
            Held5 {
                stance: Stance5::Friendly,
                since: Some(0),
            },
        );
        st.relations
            .add(C3, C2, crate::doctrine_theatre::HELP_AT + 1.0);
        st.calls.push(Call::new(C2, SYS, NOW));
        // One of C3's pilots is on the caller's grid after the call.
        st.report_readiness(
            90_400_001,
            Readiness {
                system: Some(SYS),
                at: NOW + 60,
                ..Readiness::default()
            },
        );
        st.theatre_tick(NOW + 90, &Board::default());
        assert!(
            st.relations.get(C2, C3) > 0.0
                && st.relations.get(C3, C2) > crate::doctrine_theatre::HELP_AT + 1.0
        );
    }
}
