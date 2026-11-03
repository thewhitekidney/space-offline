//! **R-REL-5: coalitions, wars and the sov view of both** (R-RELATIONS §2.6,
//! §3.5; R-WAR §10, the bot half C8-f).
//!
//! Three things, all on the hall's data and the client's own calls:
//!
//! 1. **Coalitions** (`Bloc`, derived by [`relations::blocs`] from mutual
//!    blue stances). They form because two bot alliances court each other
//!    ([`HallState::coalition_tick`]): the bloc's lead (or a lone alliance)
//!    proposes -- a daily nudge on its own view of a friendly rival, which
//!    the executor's contact sync turns into a +10 contact and a mail -- and
//!    the other side accepts when it sees the blue set on it and its own
//!    view is not hostile. They break by [`HallState::betray`] (a blue that
//!    shoots a blue) and by the lead **expelling** the lighter side of a
//!    broken pair. Members adopt the bloc's enemies after the difficulty's
//!    lag ([`relations::Temper::lag_hours`]): until then the coalition term
//!    of `R_eff` is withheld for that target ([`HallState::r_eff_of`]).
//! 2. **Wars.** A war is the server's (`war.rs`, C8). The bot half reads
//!    `warRegistry.GetWars` into [`WarRow`]s, decides with the pure
//!    [`war_acts`] and acts through the CEO's connection ([`crate::war_bot`]):
//!    declare (corp or alliance, fee from the corp wallet) or a mutual invite
//!    between two bot groups; open its war to allies and accept a blue one;
//!    a blue group offers to ally with a defender; surrender and accepting a
//!    surrender; letting the bill lapse when the relation has recovered.
//!    Shooting a war target in high sec is legal (`war::legal`), so
//!    [`HallState::at_war`] makes a pilot prey there ([`crate::hunt`]).
//! 3. **Sov.** [`HallState::coalition_relation`], [`HallState::bloc_enemy`]
//!    and [`HallState::bloc_mate`] are what `sov_bot::plan` reads.
//!
//! **Gates.** Declaring and inviting are off unless `eve-bots --society-wars`
//! ([`State::enabled`]); a declaration on a group of humans also needs
//! `--hostile` ([`State::humans`]); a group holding a `--friends` pilot is
//! never a target. Answering a war someone else made (open to allies, accept
//! an ally, surrender) is always on.
//!
//! **Every number here is [A]**, each with its falsifier at the constant;
//! no test holds one (they name the consts).

use crate::corpmind::HallState;
use crate::mind::Trait;
use crate::relations::{self, Bloc, Viewer};
use crate::society::{self, FRIEND_AT, Group, Stance, Stance5};
use eve_marshal::PyValue;
use eve_server::group_contacts::Affiliation;
use std::collections::{BTreeMap, BTreeSet};

// ---- policy [A] ---------------------------------------------------------------------

/// A view at or under this makes a group a war target **[A]** (R-WAR §10).
/// Falsified by bots at war with groups they barely met (too high) or never
/// at war with a rival that hunts them for days (too low).
pub const WAR_AT: f64 = -6.0;
/// The CEO's aggression needed to declare **[A]**. Falsified by the wars all
/// coming from one or two bots.
pub const WAR_AGGRESSION: f64 = 0.6;
/// The wallet, in fees, a declarer holds first **[A]**: the fee and the first
/// bill. Falsified by declared wars lapsing unpaid in week two.
pub const WAR_RESERVE: f64 = 2.0;
/// The wars one owner declares at once **[A]**. Falsified by a bot corp with
/// no fleet left for its own belts.
pub const BOT_MAX_WARS: usize = 2;
/// A war must have run this many game-days before a bot ends it **[A]**.
pub const WAR_MIN_DAYS: f64 = 3.0;
/// A side that lost this many times the ISK it destroyed offers to
/// surrender **[A]**. Falsified by surrender offers during winning wars.
pub const SURRENDER_LOSS: f64 = 3.0;
/// The offer, as a share of the fee to declare **[A]**.
pub const SURRENDER_ISK_SHARE: f64 = 0.2;
/// A view above this accepts a surrender **[A]**.
pub const PEACE_AT: f64 = -3.0;
/// The war bill is paid while the view is at or under `WAR_AT + PEACE_BAND`
/// **[A]**; otherwise it lapses and the war ends.
pub const PEACE_BAND: f64 = society::BAND;
/// A blue group helps a defender at this aggression **[A]**.
pub const ALLY_AGGRESSION: f64 = 0.5;
/// How often an owner's CEO runs the war pass, game-days **[A]**.
pub const WAR_EVERY_DAYS: f64 = 0.5;
/// A target that refused a declaration is left alone this long, game-days
/// **[A]**.
pub const DECLARE_RETRY_DAYS: f64 = 2.0;
/// An offer (ally or surrender) is not repeated within this, game-days **[A]**.
pub const OFFER_RETRY_DAYS: f64 = 1.0;
/// A courting alliance moves its view of a friendly rival this much a game
/// day **[A]**. Falsified by blocs that never form among friends (too low)
/// or form out of a single trade (too high).
pub const K_PROPOSE: f64 = 0.8;
/// The least own view at which a group answers a blue set on it **[A]**.
pub const ACCEPT_BLUE_AT: f64 = 1.0;
/// The lead's shove on a member it expels **[A]**: a betrayal's.
pub const K_EXPEL: f64 = relations::K_BETRAY_K;
/// The records a hall keeps of wars and of lost ISK **[A]** (state bound).
pub const WARS_KEEP: usize = 200;
pub const LEDGER_KEEP: usize = 256;

/// The observed-set kinds this module owns (`Hall::first`).
pub const KIND_BETRAY: u8 = 8;

// ---- the wars the hall knows ---------------------------------------------------------

/// One ally of a war, as `warRegistry.GetWars` lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllyRow {
    pub id: i64,
    pub started: i64,
    pub finished: i64,
}

/// **A war as the client holds it** (the fields the bot reads of the 17).
#[derive(Debug, Clone, PartialEq)]
pub struct WarRow {
    pub id: i64,
    pub by: i64,
    pub against: i64,
    /// FILETIME the war turns hostile.
    pub started: i64,
    /// FILETIME it finished, or will (a cooldown still counts as war).
    pub finished: Option<i64>,
    pub mutual: bool,
    pub open: bool,
    pub allies: Vec<AllyRow>,
    /// The bot clock when a bot first read it.
    pub seen: u64,
}

impl WarRow {
    /// A `GetWars` value (a `KeyVal`) read as a row; `seen` is the bot clock.
    pub fn from_value(v: &PyValue, seen: u64) -> Option<WarRow> {
        let id = crate::observe::int(v, "warID")?;
        let by = crate::observe::int(v, "declaredByID")?;
        let against = crate::observe::int(v, "againstID")?;
        let flag = |k: &str| {
            matches!(
                crate::observe::val(v, k),
                Some(PyValue::Bool(true)) | Some(PyValue::Int(1))
            )
        };
        let mut allies = Vec::new();
        if let Some(PyValue::Dict(entries)) = crate::observe::val(v, "allies") {
            for (a, _) in entries {
                if let (Some(id), Some(started), Some(finished)) = (
                    crate::observe::int(a, "allyID"),
                    crate::observe::int(a, "timeStarted"),
                    crate::observe::int(a, "timeFinished"),
                ) {
                    allies.push(AllyRow {
                        id,
                        started,
                        finished,
                    });
                }
            }
        }
        Some(WarRow {
            id,
            by,
            against,
            started: crate::observe::int(v, "timeStarted").unwrap_or(i64::MAX),
            finished: crate::observe::int(v, "timeFinished"),
            mutual: flag("mutual"),
            open: flag("openForAllies"),
            allies,
            seen,
        })
    }

    /// Not finished: spooling up, hostile, or in its cooldown.
    pub fn active(&self, ft: i64) -> bool {
        self.finished.is_none_or(|f| ft < f)
    }

    /// `IsInHostileState`: started, and not over (cooldown counts).
    pub fn hostile(&self, ft: i64) -> bool {
        self.active(ft) && ft > self.started
    }

    pub fn involves(&self, owner: i64) -> bool {
        self.by == owner || self.against == owner || self.allies.iter().any(|a| a.id == owner)
    }

    fn ally_active(&self, owner: i64, ft: i64) -> bool {
        self.allies
            .iter()
            .any(|a| a.id == owner && a.started < ft && ft < a.finished)
    }

    /// The other principal, for `owner` one of the two.
    pub fn other(&self, owner: i64) -> i64 {
        if self.by == owner {
            self.against
        } else {
            self.by
        }
    }
}

/// **`war_between`**, mirrored from the server (`war::war_between`): the war
/// in which an owner of `a` is the aggressor and an owner of `b` the
/// defender or its active ally, or the other way round, in a hostile state.
pub fn war_between<'a>(
    wars: impl IntoIterator<Item = &'a WarRow>,
    a: &[i64],
    b: &[i64],
    ft: i64,
) -> Option<i64> {
    for w in wars {
        if !w.hostile(ft) {
            continue;
        }
        for (aggr, other) in [(a, b), (b, a)] {
            if !aggr.contains(&w.by) {
                continue;
            }
            if other.contains(&w.against) || other.iter().any(|o| w.ally_active(*o, ft)) {
                return Some(w.id);
            }
        }
    }
    None
}

/// The war entities of a pilot: its player corp and its alliance.
pub fn owners_of(a: &Affiliation) -> Vec<i64> {
    let mut v = Vec::new();
    if society::is_player_corp(a.corporation_id) {
        v.push(a.corporation_id);
    }
    v.extend(a.alliance_id);
    v
}

/// Is `g` something a war can be declared on: a player corp or alliance.
pub fn warable(g: Group) -> bool {
    society::is_player_corp(g) || society::PLAYER_ALLIANCES.contains(&g)
}

// ---- state ------------------------------------------------------------------------------

/// **What the hall remembers for R-REL-5.** Nothing here is persisted: the
/// wars are read again from the server, the lag restarts with a restart.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    /// `--society-wars`: bots may declare and invite.
    pub enabled: bool,
    /// `--hostile`: a group of humans may be declared on.
    pub humans: bool,
    /// When `(bloc lead, target)` first read as an enemy of the bloc.
    pub enemy_since: BTreeMap<(Group, Group), u64>,
    /// The wars bots have read, by war id.
    pub wars: BTreeMap<i64, WarRow>,
    /// When each owner last ran the war pass.
    pub war_tried: BTreeMap<i64, u64>,
    /// `(owner, target)` -> when a declaration or invite was last tried.
    pub declare_tried: BTreeMap<(i64, i64), u64>,
    /// `(owner, war)` -> when an ally or surrender offer was last made.
    pub offered: BTreeMap<(i64, i64), u64>,
    /// The war-bill auto-pay each owner was last set to (default on).
    pub autopay: BTreeMap<i64, bool>,
    /// ISK lost: `(victim group, killer group)` -> ISK.
    pub ledger: BTreeMap<(Group, Group), f64>,
    /// `(from, to, kind)` -> the game-day index it last courted or answered.
    pub courted: BTreeMap<(Group, Group, u8), u64>,
    /// `(lead, member)` -> when the lead expelled the member.
    pub expelled: BTreeMap<(Group, Group), u64>,
}

impl State {
    /// Remember an ISK loss (a kill, folded into the matrix).
    pub fn lose(&mut self, victim: Group, killer: Group, isk: f64) {
        if victim == killer || !isk.is_finite() || isk <= 0.0 {
            return;
        }
        *self.ledger.entry((victim, killer)).or_default() += isk;
        while self.ledger.len() > LEDGER_KEEP {
            self.ledger.pop_first();
        }
    }

    /// Is the owner's war pass due (cadence only)?
    pub fn due(&self, owner: i64, now: u64, day: u64) -> bool {
        crate::corp_ops::due(
            self.war_tried.get(&owner).copied(),
            now,
            WAR_EVERY_DAYS,
            day,
        )
    }
}

// ---- the pure war decision --------------------------------------------------------------

/// One negotiation the owner is party to (`GetNegotiations`).
#[derive(Debug, Clone, PartialEq)]
pub struct Neg {
    pub id: i64,
    pub war: i64,
    /// `NEG_ALLY_OFFER` 0, `NEG_SURRENDER_OFFER` 2.
    pub kind: i64,
    pub from: i64,
    pub to: i64,
    /// `STATE_NEW` 0.
    pub state: i64,
    pub isk: f64,
}

/// The negotiation kinds (`war_deal`), [V] from the server's own constants.
pub const NEG_ALLY: i64 = eve_server::war_deal::NEG_ALLY_OFFER;
pub const NEG_SURRENDER: i64 = eve_server::war_deal::NEG_SURRENDER_OFFER;
pub const NEG_NEW: i64 = eve_server::war_deal::STATE_NEW;

/// A group the owner might go to war with.
#[derive(Debug, Clone, PartialEq)]
pub struct Cand {
    pub group: Group,
    /// The owner's composed view.
    pub view: f64,
    /// A group some bot runs.
    pub bot: bool,
    /// That bot group's view of the owner (0 for a human group).
    pub back: f64,
    /// The bloc's quorum agrees (or there is no bloc) and this owner is the
    /// one who declares for it.
    pub quorum_ok: bool,
    /// A `--friends` pilot flies for it.
    pub friend: bool,
}

/// What the war pass of one owner is shown.
pub struct WarCtx<'a> {
    pub owner: i64,
    pub ft: i64,
    pub now: u64,
    pub day: u64,
    pub aggression: f64,
    /// The paying wallet (the executor corp's Master Wallet).
    pub wallet: f64,
    pub enabled: bool,
    pub humans_ok: bool,
    /// Every war the owner is in.
    pub wars: &'a [WarRow],
    /// Running wars open for allies (of any owners), from the registry.
    pub open_wars: &'a [WarRow],
    pub negs: &'a [Neg],
    /// Owners whose mutual-war invite is pending for this owner.
    pub invites: &'a [i64],
    pub cands: &'a [Cand],
    pub view: &'a dyn Fn(i64) -> f64,
    /// Is the owner allied (Friendly or better) to the group?
    pub blue: &'a dyn Fn(i64) -> bool,
    /// ISK the owner lost to the group / the group lost to the owner.
    pub lost: &'a dyn Fn(i64) -> f64,
    pub destroyed: &'a dyn Fn(i64) -> f64,
    /// The auto-pay the owner is set to now.
    pub autopay: bool,
    /// Does the owner have any blue bot group that could ally with it?
    pub has_mates: bool,
    /// Has the lag passed for a bloc member to join this war?
    pub lag_ok: &'a dyn Fn(&WarRow) -> bool,
    /// `(owner, war)` offers made, and when.
    pub offered: &'a BTreeMap<(i64, i64), u64>,
}

/// One thing the war pass does.
#[derive(Debug, Clone, PartialEq)]
pub enum WarAct {
    /// `DeclareWarAgainst` on a group.
    Declare(Group),
    /// `SendInviteByPlayer` to a bot group (no fee, no bill).
    Mutual(Group),
    /// `SetOpenForAllies(war, true)`.
    OpenAllies(i64),
    /// `CreateWarAllyOffer(war, 0, defender, ..)`.
    OfferAlly { war: i64, defender: Group },
    /// `AcceptAllyNegotiation(neg)`.
    AcceptAlly(i64),
    /// `CreateSurrenderNegotiation(war, isk, ..)`, to the other principal.
    OfferSurrender { war: i64, isk: f64, to: Group },
    /// `AcceptSurrender(neg)`, offered by `from`.
    AcceptSurrender { neg: i64, from: Group },
    /// `RespondToInviteByPlayer(from, accepts)`.
    Answer { from: i64, accept: bool },
    /// `SendAutomaticPaySettings`: pay the war bills, or let them lapse.
    AutoPay(bool),
}

fn aged(w: &WarRow, now: u64, day: u64) -> bool {
    now.saturating_sub(w.seen) >= (WAR_MIN_DAYS * day as f64) as u64
}

fn recently(
    map: &BTreeMap<(i64, i64), u64>,
    key: (i64, i64),
    now: u64,
    day: u64,
    days: f64,
) -> bool {
    map.get(&key)
        .is_some_and(|t| now.saturating_sub(*t) < (days * day as f64) as u64)
}

/// **The war pass of one owner** (pure; R-WAR §10, the bot half). In order:
/// answer what is asked of it (surrender, ally, mutual invites), then end
/// what has run its course (surrender, let the bill lapse), then help a
/// blue defender, then declare at most one war.
pub fn war_acts(c: &WarCtx) -> Vec<WarAct> {
    let mut out = Vec::new();
    let mine = |w: &&WarRow| w.by == c.owner || w.against == c.owner;

    // 1. Answers.
    for n in c
        .negs
        .iter()
        .filter(|n| n.state == NEG_NEW && n.to == c.owner)
    {
        match n.kind {
            // The one who receives the money ends the war when the
            // relation has healed, or when it is losing as well.
            NEG_SURRENDER
                if (c.view)(n.from) >= PEACE_AT || (c.lost)(n.from) > (c.destroyed)(n.from) =>
            {
                out.push(WarAct::AcceptSurrender {
                    neg: n.id,
                    from: n.from,
                });
            }
            NEG_ALLY if (c.blue)(n.from) => out.push(WarAct::AcceptAlly(n.id)),
            _ => {}
        }
    }
    let running_by = c
        .wars
        .iter()
        .filter(|w| w.by == c.owner && w.active(c.ft))
        .count();
    for &from in c.invites {
        let red = (c.view)(from) <= society::ENEMY_AT;
        let human = !c.cands.iter().any(|x| x.group == from && x.bot);
        let ok = c.enabled
            && c.aggression >= WAR_AGGRESSION
            && red
            && running_by < BOT_MAX_WARS
            && (!human || c.humans_ok);
        if ok {
            out.push(WarAct::Answer { from, accept: true });
        } else if (c.view)(from) > PEACE_AT {
            out.push(WarAct::Answer {
                from,
                accept: false,
            });
        }
    }

    // 2. Ending: a surrender offer from the side that is losing badly, and
    // the bill left to lapse once the relation has recovered.
    let mut pay_wanted = false;
    let mut by_me = false;
    for w in c.wars.iter().filter(mine).filter(|w| !w.mutual) {
        if !w.hostile(c.ft) {
            continue;
        }
        let other = w.other(c.owner);
        if w.by == c.owner {
            by_me = true;
            pay_wanted |= (c.view)(other) <= WAR_AT + PEACE_BAND || !aged(w, c.now, c.day);
        }
        let isk = SURRENDER_ISK_SHARE * eve_server::war::COST_OF_WAR;
        let (lost, killed) = ((c.lost)(other), (c.destroyed)(other));
        let open_offer = c
            .negs
            .iter()
            .any(|n| n.war == w.id && n.kind == NEG_SURRENDER && n.state == NEG_NEW);
        if aged(w, c.now, c.day)
            && lost > 0.0
            && lost >= SURRENDER_LOSS * killed
            && c.wallet >= isk
            && !open_offer
            && !recently(c.offered, (c.owner, w.id), c.now, c.day, OFFER_RETRY_DAYS)
        {
            out.push(WarAct::OfferSurrender {
                war: w.id,
                isk,
                to: other,
            });
        }
    }
    if by_me && pay_wanted != c.autopay {
        out.push(WarAct::AutoPay(pay_wanted));
    }

    // 3. Allies: open an attacked war when there is someone to ask, and
    // help a blue defender.
    if c.has_mates {
        for w in c
            .wars
            .iter()
            .filter(|w| w.against == c.owner && !w.mutual && w.active(c.ft) && !w.open)
        {
            out.push(WarAct::OpenAllies(w.id));
        }
    }
    if c.aggression >= ALLY_AGGRESSION
        && let Some(w) = c.open_wars.iter().find(|w| {
            w.open
                && !w.mutual
                && w.active(c.ft)
                && !w.involves(c.owner)
                && (c.blue)(w.against)
                && (c.view)(w.by) <= society::ENEMY_AT
                && (c.lag_ok)(w)
                && !recently(c.offered, (c.owner, w.id), c.now, c.day, OFFER_RETRY_DAYS)
        })
    {
        out.push(WarAct::OfferAlly {
            war: w.id,
            defender: w.against,
        });
    }

    // 4. Declare: at most one, on the worst enemy.
    if c.enabled && c.aggression >= WAR_AGGRESSION && running_by < BOT_MAX_WARS {
        let pick = c
            .cands
            .iter()
            .filter(|x| x.quorum_ok && !x.friend && x.view <= WAR_AT)
            .filter(|x| x.bot || c.humans_ok)
            .filter(|x| {
                !c.wars
                    .iter()
                    .chain(c.open_wars.iter())
                    .any(|w| w.involves(x.group) && w.involves(c.owner) && w.active(c.ft))
            })
            .min_by(|a, b| a.view.total_cmp(&b.view).then(a.group.cmp(&b.group)));
        if let Some(x) = pick {
            if x.bot && x.back <= society::ENEMY_AT {
                out.push(WarAct::Mutual(x.group));
            } else if c.wallet >= eve_server::war::COST_OF_WAR * WAR_RESERVE {
                out.push(WarAct::Declare(x.group));
            }
        }
    }
    out
}

// ---- the hall's side ---------------------------------------------------------------------

/// Game-hours in `secs` bot seconds.
fn hours(secs: u64, day: u64) -> f64 {
    secs as f64 / day.max(1) as f64 * 24.0
}

impl HallState {
    /// The hall's clock: when the matrix last decayed.
    pub fn clock(&self) -> u64 {
        self.relations.at.unwrap_or(0)
    }

    /// The groups some bot runs: its alliance, else the corp.
    pub fn bot_groups(&self) -> BTreeSet<Group> {
        self.bot_corps()
            .into_iter()
            .map(|c| self.group_of_corp(c))
            .collect()
    }

    /// The **blocs** now (`Hall::blocs`, R-RELATIONS §2.6).
    pub fn blocs(&self, now: u64) -> Vec<Bloc> {
        self.with_layers(now, |l| l.blocs.to_vec())
    }

    /// An owner's composed view of a group: an alliance's `A_eff + K`, a
    /// corp's `C_eff`.
    pub fn owner_view(&self, owner: Group, t: Group, now: u64) -> f64 {
        self.with_layers(now, |l| {
            if society::PLAYER_ALLIANCES.contains(&owner) {
                relations::a_eff_k(l, owner, t)
            } else {
                let v = Viewer {
                    corp: owner,
                    alliance: self.corps.get(&owner).and_then(|c| c.alliance),
                };
                relations::c_eff(l, &v, t)
            }
        })
    }

    /// What `sov_bot` reads: the alliance's view of a holder through its
    /// coalition.
    pub fn coalition_relation(&self, alliance: Group, holder: Group) -> f64 {
        self.owner_view(alliance, holder, self.clock())
    }

    /// Is `t` an enemy of `owner`'s bloc, and has the lag passed?
    pub fn bloc_enemy(&self, owner: Group, t: Group) -> bool {
        let now = self.clock();
        let Some(lead) = self.with_layers(now, |l| l.bloc_of(owner).map(|b| b.lead)) else {
            return false;
        };
        self.coal.enemy_since.get(&(lead, t)).is_some_and(|since| {
            relations::lag_elapsed(hours(now.saturating_sub(*since), self.day), &self.temper)
        })
    }

    /// Are the two groups members of one bloc?
    pub fn bloc_mate(&self, a: Group, b: Group) -> bool {
        a != b
            && self.with_layers(self.clock(), |l| {
                l.bloc_of(a).is_some_and(|bl| bl.contains(b))
            })
    }

    /// Is `viewer`'s coalition term for `t` still within the lag? (`R_eff`
    /// withholds it then: [`HallState::r_eff_of`].)
    pub fn lagging(&self, lead: Group, t: Group, now: u64) -> bool {
        self.coal.enemy_since.get(&(lead, t)).is_some_and(|since| {
            !relations::lag_elapsed(hours(now.saturating_sub(*since), self.day), &self.temper)
        })
    }

    // ---- the tick ----

    /// **The coalition tick** (end of [`HallState::tick`]): the bloc enemy
    /// lists, the courting, the expulsions. Act lines.
    pub fn coalition_tick(&mut self, now: u64) -> Vec<String> {
        let mut lines = Vec::new();
        let day_idx = now / self.day.max(1);

        // 1. The enemy list of each bloc, and when each entry appeared.
        let targets: Vec<Group> = self
            .relations
            .r
            .keys()
            .chain(self.alliance_own.r.keys())
            .map(|(_, t)| *t)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let temper = self.temper;
        let (blocs, enemies) = self.with_layers(now, |l| {
            let e: Vec<(Group, Vec<Group>)> = l
                .blocs
                .iter()
                .map(|b| (b.lead, relations::enemies(l, b, &targets, &temper)))
                .collect();
            (l.blocs.to_vec(), e)
        });
        let live: BTreeSet<(Group, Group)> = enemies
            .iter()
            .flat_map(|(lead, ts)| ts.iter().map(move |t| (*lead, *t)))
            .collect();
        for k in &live {
            if !self.coal.enemy_since.contains_key(k) {
                self.coal.enemy_since.insert(*k, now);
                let line = format!("coalition {}: {} is an enemy of the bloc", k.0, k.1);
                self.act(line.clone());
                lines.push(line);
            }
        }
        self.coal.enemy_since.retain(|k, _| live.contains(k));

        // 2. Courting between bot groups.
        let groups: Vec<Group> = self.bot_groups().into_iter().collect();
        for &a in &groups {
            for &b in &groups {
                if a == b || self.inside_pair(a, b) {
                    continue;
                }
                let (va, vb) = (self.owner_view(a, b, now), self.owner_view(b, a, now));
                let a_blue = self.stances.get(a, b) == Stance::Blue;
                let b_blue = self.stances.get(b, a) == Stance::Blue;
                let lead_a = blocs
                    .iter()
                    .find(|bl| bl.contains(a))
                    .is_none_or(|bl| bl.lead == a);
                // A proposal: a friendly rival, not yet allied.
                if lead_a
                    && (FRIEND_AT..society::ALLY_AT).contains(&va)
                    && vb > society::WARY_AT
                    && !(a_blue && b_blue)
                    && self.coal.courted.get(&(a, b, 0)) != Some(&day_idx)
                {
                    self.coal.courted.insert((a, b, 0), day_idx);
                    self.nudge(a, b, K_PROPOSE);
                    let line = format!("coalition: {a} proposes blue to {b} (view {va:.1})");
                    self.act(line.clone());
                    lines.push(line);
                }
                // An acceptance: the other side set blue on me and I do not
                // hold it against them.
                if self
                    .books
                    .get(&(a, b))
                    .is_some_and(|s| *s >= society::BLUE_STANDING)
                    && vb >= ACCEPT_BLUE_AT
                    && !b_blue
                    && self.coal.courted.get(&(b, a, 1)) != Some(&day_idx)
                {
                    self.coal.courted.insert((b, a, 1), day_idx);
                    self.nudge(b, a, relations::K_ACCEPT);
                    let line = format!("coalition: {b} accepts {a}'s blue (view {vb:.1})");
                    self.act(line.clone());
                    lines.push(line);
                }
            }
        }

        // 3. Expulsions: the lead clears its blue to the lighter side of a
        // pair in its bloc that turned on each other.
        for bl in &blocs {
            let ms: Vec<Group> = bl.members.keys().copied().collect();
            for (i, &x) in ms.iter().enumerate() {
                for &y in &ms[i + 1..] {
                    let broken = self.stances.get5(x, y) == Stance5::Hostile
                        || self.stances.get5(y, x) == Stance5::Hostile;
                    if !broken {
                        continue;
                    }
                    let (wx, wy) = (bl.members[&x], bl.members[&y]);
                    let loser = if wx < wy || (wx == wy && x > y) { x } else { y };
                    if loser == bl.lead || self.stances.get(bl.lead, loser) != Stance::Blue {
                        continue;
                    }
                    self.stances
                        .settle_forced(bl.lead, loser, Stance::Neutral, now);
                    self.stances.graded.insert(
                        (bl.lead, loser),
                        society::Held5 {
                            stance: Stance5::Neutral,
                            since: Some(now),
                        },
                    );
                    self.nudge(bl.lead, loser, -K_EXPEL);
                    self.coal.expelled.insert((bl.lead, loser), now);
                    let line = format!(
                        "coalition {}: expels {loser} (it and {} turned on each other)",
                        bl.lead,
                        if loser == x { y } else { x }
                    );
                    self.act(line.clone());
                    lines.push(line);
                }
            }
        }
        lines
    }

    /// Is `b` inside `a` or the other way (an alliance and its own member)?
    fn inside_pair(&self, a: Group, b: Group) -> bool {
        let member_of =
            |c: Group, al: Group| self.corps.get(&c).and_then(|m| m.alliance) == Some(al);
        member_of(a, b) || member_of(b, a)
    }

    /// Move `holder`'s view of `subject` by `d` in the corp matrix, where
    /// `holder`'s bot corps keep it (coarse stances read it).
    pub fn nudge(&mut self, holder: Group, subject: Group, d: f64) {
        let t = self.target(subject);
        for v in self.viewers(holder) {
            self.relations.add(v, t, d);
        }
        self.dirty = true;
    }

    /// **A betrayal** (R-RELATIONS §2.5): a blue shot a blue. The victim's
    /// stance toward the killer's group flips to hostile at once, the pair is
    /// marked, and every other member of the victim's bloc takes the spill.
    pub fn betray(&mut self, victim: &Affiliation, killer: &Affiliation, now: u64) -> Vec<String> {
        let (vg, kg) = (society::group_of(victim), society::group_of(killer));
        if vg == kg || self.viewers(vg).is_empty() {
            return Vec::new();
        }
        let spill = self.with_layers(now, |l| {
            relations::betrayal_spill(l, victim, killer, &self.temper)
        });
        self.apply_deltas(&spill);
        let mut owners = vec![vg];
        if self.bot_corps().contains(&victim.corporation_id) && victim.corporation_id != vg {
            owners.push(victim.corporation_id);
        }
        for o in owners {
            self.stances.mark_betrayed(o, kg, now);
            self.stances.settle_forced(o, kg, Stance::Red, now);
            self.stances.graded.insert(
                (o, kg),
                society::Held5 {
                    stance: Stance5::Hostile,
                    since: Some(now),
                },
            );
        }
        let line = format!("betrayal: {kg} shot {vg}; the stance is hostile at once");
        self.act(line.clone());
        vec![line]
    }

    // ---- wars ----

    /// Remember a war a bot read (merges: the first sighting's time stays).
    pub fn note_war(&mut self, mut w: WarRow) {
        if let Some(old) = self.coal.wars.get(&w.id) {
            w.seen = old.seen;
        }
        self.coal.wars.insert(w.id, w);
        while self.coal.wars.len() > WARS_KEEP {
            // The oldest war (the lowest id) goes first.
            self.coal.wars.pop_first();
        }
        self.dirty = true;
    }

    /// **Are the two pilots at war** (a hostile war between their sides)?
    pub fn at_war(&self, me: &Affiliation, other: &Affiliation, ft: i64) -> bool {
        if self.coal.wars.is_empty() {
            return false;
        }
        war_between(
            self.coal.wars.values(),
            &owners_of(me),
            &owners_of(other),
            ft,
        )
        .is_some()
    }

    /// Is any war of `me`'s sides in a hostile state?
    pub fn war_on(&self, me: &Affiliation, ft: i64) -> bool {
        let mine = owners_of(me);
        self.coal
            .wars
            .values()
            .any(|w| w.hostile(ft) && mine.iter().any(|o| w.involves(*o)))
    }

    /// Does the owner have a war worth a pass (running, any side)?
    pub fn has_war(&self, owner: i64, ft: i64) -> bool {
        self.coal
            .wars
            .values()
            .any(|w| w.involves(owner) && w.active(ft))
    }

    /// Is the owner's war pass due, and is there something for it to do (the
    /// wars switch is on, or a war involves it)?
    pub fn war_pass_due(&self, owner: Group, now: u64) -> bool {
        self.coal.due(owner, now, self.day)
            && (self.coal.enabled || self.has_war(owner, eve_server::pyutil::filetime_now()))
    }

    /// The wars an owner is in (clones).
    pub fn wars_of(&self, owner: i64) -> Vec<WarRow> {
        self.coal
            .wars
            .values()
            .filter(|w| w.involves(owner))
            .cloned()
            .collect()
    }

    /// Does a `--friends` pilot fly for the group?
    pub fn group_has_friend(&self, g: Group) -> bool {
        self.humans.iter().any(|(c, s)| {
            self.friends.contains_key(c)
                && s.alliance
                    .filter(|a| *a == g)
                    .or(society::is_player_corp(s.corp)
                        .then_some(s.corp)
                        .filter(|c| *c == g))
                    .is_some()
        })
    }

    /// **The groups an owner might declare on**, with its view and the
    /// bloc's quorum (R-RELATIONS §3.5): the worst of its views first.
    pub fn war_candidates(&self, owner: Group, now: u64) -> Vec<Cand> {
        let mine: BTreeSet<Group> = {
            let mut s: BTreeSet<Group> = self.viewers(owner).into_iter().collect();
            s.insert(owner);
            s.extend(self.corps.get(&owner).and_then(|c| c.alliance));
            s
        };
        let viewers = self.viewers(owner);
        let subjects: BTreeSet<Group> = self
            .relations
            .r
            .keys()
            .filter(|(v, _)| viewers.contains(v))
            .map(|(_, t)| *t)
            .chain(
                self.alliance_own
                    .r
                    .keys()
                    .filter(|(h, _)| *h == owner)
                    .map(|(_, t)| *t),
            )
            .filter(|t| warable(*t) && !mine.contains(t))
            .collect();
        let bots = self.bot_groups();
        let (lead_ok, enemy_set): (bool, BTreeSet<Group>) =
            self.with_layers(now, |l| match l.bloc_of(owner) {
                Some(b) => (
                    b.lead == owner,
                    relations::enemies(
                        l,
                        b,
                        &subjects.iter().copied().collect::<Vec<_>>(),
                        &self.temper,
                    )
                    .into_iter()
                    .collect(),
                ),
                None => (true, subjects.clone()),
            });
        subjects
            .into_iter()
            .map(|g| {
                let bot = bots.contains(&g);
                Cand {
                    group: g,
                    view: self.owner_view(owner, g, now),
                    bot,
                    back: if bot {
                        self.owner_view(g, owner, now)
                    } else {
                        0.0
                    },
                    quorum_ok: lead_ok && enemy_set.contains(&g),
                    friend: self.group_has_friend(g),
                }
            })
            .collect()
    }

    /// The CEO's aggression, for the war gate.
    pub fn ceo_aggression(&self, corp: i64) -> f64 {
        self.corps
            .get(&corp)
            .and_then(|c| c.ceo)
            .and_then(|ceo| self.members.get(&ceo))
            .map_or(0.5, |m| m.traits.get(Trait::Aggression))
    }

    /// ISK `owner`'s groups lost to `g`, and ISK `g` lost to them.
    pub fn war_ledger(&self, owner: Group, g: Group) -> (f64, f64) {
        let lost = self.coal.ledger.get(&(owner, g)).copied().unwrap_or(0.0);
        let killed = self.coal.ledger.get(&(g, owner)).copied().unwrap_or(0.0);
        (lost, killed)
    }

    /// Is `owner` allied (a Friendly or better stance) to `g`?
    pub fn owner_blue(&self, owner: Group, g: Group) -> bool {
        self.stances.get5(owner, g).is_allied()
    }

    /// Are there blue bot groups that could ally with `owner`?
    pub fn has_mates(&self, owner: Group) -> bool {
        self.bot_groups()
            .into_iter()
            .any(|g| g != owner && self.owner_blue(owner, g))
    }
}

#[cfg(test)]
mod tests;
