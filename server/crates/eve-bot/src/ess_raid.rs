//! **The hunter and the ESS main bank** (master plan 3.5, lane
//! `bot-filaments`; server `eve_server::ess`, owner decision #96).
//!
//! # The server's side **[V]**
//!
//! A system of sov null holds one Encounter Surveillance System. The ratters
//! of the system feed its main bank: 25 percent of every NPC bounty paid
//! there goes into the bank and the pilot is paid half of their share back
//! every three hours (`ess::split`, `ess::tick`). **A ratting bot feeds it
//! with no step of its own**: the server's bounty path does. A pilot who
//! links to the bank ([`essMgr.AttemptLinkToMainBank`]) within 10 km of the
//! ESS and holds the link for 300 s is paid the whole bank; the unpaid shares
//! are lost and the ESS takes no new link for 600 s. The client's panel
//! reads `essMgr.GetDataForClientSolarSystem` (`mainValue`, `mainBankLink`,
//! `beaconID`, ...) and warps to the `beaconID` ball.
//!
//! # What a hunter does
//!
//! [`decide`] is one pure function from what the bot knows to one [`Plan`]:
//!
//! 1. read the system's ESS once ([`Plan::Read`]); no ESS, no raid;
//! 2. skip it when the bot is timid ([`MIN_RISK`]), the bank is poorer than
//!    its greed asks ([`bank_floor`]: the greedy take less), someone is
//!    linked, the ESS is cooling, or more pilots sit in Local than its nerve
//!    tolerates ([`tolerance`]: nobody defends it);
//! 3. warp to the beacon, link once it has landed, hold the link, and read
//!    the bank now and then; unlink and leave when hostile pilots come to the
//!    grid beyond what its nerve tolerates.
//!
//! Every number is **[A]** policy (falsifier: bots that never raid, or that
//! die at the ESS every time).

use crate::brain::{self, Act, Memory, Observation};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use std::time::{Duration, Instant};

/// **[A]** The Risk below which a bot never robs a bank.
pub const MIN_RISK: f64 = 0.3;
/// **[A]** The bank a content bot (greed 0) and a greedy one (greed 1) find
/// worth the five minutes in the open, ISK.
pub const FLOOR_CONTENT: f64 = 400_000_000.0;
pub const FLOOR_GREEDY: f64 = 40_000_000.0;
/// **[A]** The server's cooldown after a theft (`ess::BUFFER_TIME_SECS`,
/// 600 s; the module is private to the server crate, so mirrored).
pub const COOLDOWN: Duration = Duration::from_secs(600);
/// **[A]** How long a landed warp settles before the link is asked for.
pub const SETTLE: Duration = Duration::from_secs(3);
/// **[A]** A warp that has not landed after this long is given up.
pub const WARP_PATIENCE: Duration = Duration::from_secs(180);
/// **[A]** How often a held link re-reads the bank.
pub const HOLD_READ_EVERY: Duration = Duration::from_secs(20);
/// **[A]** Refused links after which a bot stops trying (per run).
pub const MAX_REFUSED: u32 = 3;
/// `ESS_MAX_LINK_DISTANCE_METERS` **[V]**.
pub const LINK_RANGE: f64 = 10_000.0;

/// The ESS as the client's panel reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct EssView {
    pub system: i64,
    pub beacon: i64,
    /// `essID`: the ESS ball.
    pub ess: i64,
    /// `mainValue`, ISK.
    pub value: f64,
    /// `mainBankLink.characterID`, if anyone is linked.
    pub linked_by: Option<i64>,
    pub read_at: Instant,
    /// The surface gap from the ship to the ESS ball, when it is on the grid.
    pub gap: Option<f64>,
}

/// Where a raid stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stage {
    #[default]
    Idle,
    /// The warp to the beacon is ordered.
    Warped,
    /// The link is on.
    Linking,
}

/// What a hunter remembers of ESS banks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mind {
    pub view: Option<EssView>,
    /// Systems read and found without an ESS.
    pub none: Vec<i64>,
    pub stage: Stage,
    pub stage_at: Option<Instant>,
    pub cooled_until: Option<Instant>,
    pub refused: u32,
    /// Raids finished (the link ran its course).
    pub raids: u32,
    /// Reads asked and not answered (a failed read must not loop).
    pub asked: u32,
}

/// **[A]** Unanswered reads after which the bank is left alone.
pub const MAX_ASKED: u32 = 1;

/// Why a bank is left alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    NoEss,
    Timid,
    Refused,
    Occupied,
    Cooling,
    Poor,
    Defended,
}

/// One step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    Skip(Why),
    Read,
    Warp {
        beacon: i64,
    },
    /// Landed or landing: wait.
    Settle,
    Link,
    Hold,
    Unlink,
    /// The link is over (done, broken, or lost to another).
    Done,
}

/// What [`decide`] reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Brief<'a> {
    pub me: i64,
    pub risk: f64,
    pub greed: f64,
    pub aggression: f64,
    pub known_none: bool,
    pub view: Option<&'a EssView>,
    pub stage: Stage,
    /// Time in the stage.
    pub in_stage: Duration,
    pub age: Duration,
    pub cooling: bool,
    pub refused: u32,
    /// Pilots in Local besides the bot (0 when unknown **[A]**).
    pub others_local: usize,
    /// Hostile pilots on the grid now.
    pub grid_hostiles: usize,
    pub warping: bool,
}

/// The bank a bot of this greed finds worth a raid.
pub fn bank_floor(greed: f64) -> f64 {
    let g = greed.clamp(0.0, 1.0);
    FLOOR_CONTENT + (FLOOR_GREEDY - FLOOR_CONTENT) * g
}

/// How many other pilots a bot tolerates in Local, or hostile on the grid,
/// before it leaves the bank alone: none for the timid.
pub fn tolerance(risk: f64, aggression: f64) -> usize {
    (risk.clamp(0.0, 1.0) * 2.0 + aggression.clamp(0.0, 1.0) - 0.5).max(0.0) as usize
}

/// The one pure decision.
pub fn decide(b: &Brief<'_>) -> Plan {
    if b.refused >= MAX_REFUSED {
        return Plan::Skip(Why::Refused);
    }
    if b.known_none {
        return Plan::Skip(Why::NoEss);
    }
    if b.risk < MIN_RISK {
        return Plan::Skip(Why::Timid);
    }
    let tol = tolerance(b.risk, b.aggression);
    if b.stage == Stage::Linking {
        if b.grid_hostiles > tol {
            return Plan::Unlink;
        }
        if b.age >= HOLD_READ_EVERY || b.view.is_none() {
            return Plan::Read;
        }
        return match b.view.and_then(|v| v.linked_by) {
            Some(who) if who == b.me => Plan::Hold,
            _ => Plan::Done,
        };
    }
    let Some(v) = b.view else {
        return Plan::Read;
    };
    if v.linked_by.is_some() {
        return Plan::Skip(Why::Occupied);
    }
    if b.cooling {
        return Plan::Skip(Why::Cooling);
    }
    if v.value < bank_floor(b.greed) {
        return Plan::Skip(Why::Poor);
    }
    if b.others_local > tol || b.grid_hostiles > tol {
        return Plan::Skip(Why::Defended);
    }
    match b.stage {
        Stage::Warped if b.in_stage >= WARP_PATIENCE => Plan::Warp { beacon: v.beacon },
        Stage::Warped if b.warping || b.in_stage < SETTLE => Plan::Settle,
        Stage::Warped => Plan::Link,
        _ => Plan::Warp { beacon: v.beacon },
    }
}

impl Mind {
    fn in_stage(&self, now: Instant) -> Duration {
        self.stage_at
            .map_or(Duration::ZERO, |t| now.saturating_duration_since(t))
    }

    fn enter(&mut self, s: Stage, now: Instant) {
        self.stage = s;
        self.stage_at = Some(now);
    }

    /// A read came back: the bank, or no ESS here.
    pub fn on_read(&mut self, system: i64, view: Option<EssView>) {
        self.asked = 0;
        match view {
            Some(v) => self.view = Some(v),
            None => {
                self.view = None;
                if !self.none.contains(&system) {
                    self.none.push(system);
                }
            }
        }
    }

    /// The link was asked for.
    pub fn on_link(&mut self, ok: bool, now: Instant) {
        if ok {
            self.enter(Stage::Linking, now);
            self.view = None; // read again at once
        } else {
            self.refused += 1;
            self.cooled_until = Some(now + COOLDOWN);
            self.enter(Stage::Idle, now);
        }
    }

    /// The bot let go of the link.
    pub fn on_unlink(&mut self, now: Instant) {
        self.cooled_until = Some(now + COOLDOWN);
        self.enter(Stage::Idle, now);
    }
}

/// The brain's step, in space with no prey: `Some(acts)` when the ESS asks
/// for the turn.
pub fn step(m: &mut Memory, o: &Observation, system: i64) -> Option<Vec<Act>> {
    let park = o.park.as_ref()?;
    let e = &mut m.hunt.ess;
    // A view of another system is no view.
    if e.view.as_ref().is_some_and(|v| v.system != system) {
        e.view = None;
        if e.stage != Stage::Idle {
            e.enter(Stage::Idle, o.now);
        }
    }
    let view = e.view.clone();
    let tol_hostiles = o.hunt.pilots.iter().filter(|p| p.hostile).count();
    let brief = Brief {
        me: o.me,
        risk: o.hunt.temper.risk,
        greed: o.hunt.mood.greed,
        aggression: o.hunt.temper.aggression,
        known_none: e.none.contains(&system),
        view: view.as_ref(),
        stage: e.stage,
        in_stage: e.in_stage(o.now),
        age: view.as_ref().map_or(Duration::MAX, |v| {
            o.now.saturating_duration_since(v.read_at)
        }),
        cooling: e.cooled_until.is_some_and(|t| o.now < t),
        refused: e.refused,
        others_local: o
            .threat
            .local
            .as_ref()
            .map_or(0, |l| l.iter().filter(|c| **c != o.me).count()),
        grid_hostiles: tol_hostiles,
        warping: park.ego_warping,
    };
    let plan = decide(&brief);
    let op = |op: EssAct| vec![Act::Hunt(crate::hunt::HuntAct::Ess(op))];
    match plan {
        Plan::Skip(_) => None,
        Plan::Read if e.asked >= MAX_ASKED => {
            // The reads fail: give the raid up rather than ask for ever.
            if e.stage != Stage::Idle {
                e.on_unlink(o.now);
            }
            e.asked = 0;
            e.refused = MAX_REFUSED;
            None
        }
        Plan::Read => {
            e.asked += 1;
            Some(op(EssAct::Read { system }))
        }
        Plan::Warp { beacon } => {
            e.enter(Stage::Warped, o.now);
            Some(vec![Act::WarpToItem {
                id: beacon,
                range: 0.0,
            }])
        }
        Plan::Settle | Plan::Hold => Some(vec![Act::Wait(brain::SHORT_WAIT)]),
        Plan::Link => Some(op(EssAct::Link)),
        Plan::Unlink => Some(op(EssAct::Unlink)),
        Plan::Done => {
            e.raids += 1;
            e.on_unlink(o.now);
            None
        }
    }
}

/// What the hunter asks the driver to do at an ESS.
#[derive(Debug, Clone, PartialEq)]
pub enum EssAct {
    Read { system: i64 },
    Link,
    Unlink,
}

impl EssAct {
    pub fn describe(&self) -> (String, Vec<i64>) {
        match self {
            EssAct::Read { system } => (
                format!("essMgr.GetDataForClientSolarSystem in {system}"),
                vec![*system],
            ),
            EssAct::Link => ("essMgr.AttemptLinkToMainBank".to_owned(), vec![]),
            EssAct::Unlink => ("essMgr.RequestMainBankUnlink".to_owned(), vec![]),
        }
    }
}

/// The panel's dict as a view; `None` is "no ESS here".
pub fn parse(system: i64, v: &PyValue, now: Instant) -> Option<EssView> {
    let d = crate::attributes(v);
    let int = |k: &str| match crate::dict_get(d, k) {
        Some(PyValue::Int(i)) => Some(*i),
        _ => None,
    };
    let real = |k: &str| match crate::dict_get(d, k) {
        Some(PyValue::Real(r)) => Some(*r),
        Some(PyValue::Int(i)) => Some(*i as f64),
        _ => None,
    };
    let value = real("mainValue")?;
    Some(EssView {
        system,
        beacon: int("beaconID")?,
        ess: int("essID").unwrap_or(0),
        value,
        linked_by: crate::dict_get(d, "mainBankLink").and_then(|l| {
            match crate::dict_get(l, "characterID") {
                Some(PyValue::Int(c)) => Some(*c),
                _ => None,
            }
        }),
        read_at: now,
        gap: None,
    })
}

impl Conn {
    /// Read the system's ESS (`None`: no ESS here).
    pub fn ess_read(&mut self, system: i64) -> Result<Option<EssView>> {
        let out = self.call("essMgr", "GetDataForClientSolarSystem", vec![])?;
        if let Some(key) = out.refusal() {
            return Err(Error::Refused(format!(
                "GetDataForClientSolarSystem: {key}"
            )));
        }
        let Some(v) = out.value() else {
            return Ok(None);
        };
        let mut view = parse(system, &v, Instant::now());
        if let Some(view) = &mut view {
            let ess = view.ess;
            let w = self.world_now();
            view.gap = w.ego.and_then(|e| w.surface_distance(e, ess));
        }
        Ok(view)
    }

    /// `essMgr.AttemptLinkToMainBank` and `RequestMainBankUnlink`.
    pub fn ess_call(&mut self, method: &str) -> Result<()> {
        let out = self.call("essMgr", method, vec![])?;
        match out.refusal() {
            Some(key) => Err(Error::Refused(format!("{method}: {key}"))),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    //! Invented ids and values; the constants by name.
    use super::*;

    const ME: i64 = 90_000_201;
    const SYS: i64 = 30_000_201;

    fn view(value: f64, linked_by: Option<i64>) -> EssView {
        EssView {
            system: SYS,
            beacon: 3_000_000_000_201,
            ess: 3_100_000_000_201,
            value,
            linked_by,
            read_at: Instant::now(),
            gap: None,
        }
    }

    fn brief(v: Option<&EssView>) -> Brief<'_> {
        Brief {
            me: ME,
            risk: 0.8,
            greed: 0.5,
            aggression: 0.5,
            known_none: false,
            view: v,
            stage: Stage::Idle,
            in_stage: Duration::ZERO,
            age: Duration::ZERO,
            cooling: false,
            refused: 0,
            others_local: 0,
            grid_hostiles: 0,
            warping: false,
        }
    }

    #[test]
    fn an_unread_system_is_read_first_and_one_without_an_ess_is_skipped() {
        assert_eq!(decide(&brief(None)), Plan::Read);
        let b = Brief {
            known_none: true,
            ..brief(None)
        };
        assert_eq!(decide(&b), Plan::Skip(Why::NoEss));
    }

    #[test]
    fn a_rich_undefended_bank_is_warped_to_then_linked_after_it_lands() {
        let v = view(bank_floor(0.5) * 2.0, None);
        let b = brief(Some(&v));
        assert_eq!(decide(&b), Plan::Warp { beacon: v.beacon });
        let landing = Brief {
            stage: Stage::Warped,
            warping: true,
            ..brief(Some(&v))
        };
        assert_eq!(decide(&landing), Plan::Settle);
        let landed = Brief {
            stage: Stage::Warped,
            in_stage: SETTLE,
            ..brief(Some(&v))
        };
        assert_eq!(decide(&landed), Plan::Link);
        // A warp that never landed is ordered again.
        let lost = Brief {
            stage: Stage::Warped,
            in_stage: WARP_PATIENCE,
            ..brief(Some(&v))
        };
        assert_eq!(decide(&lost), Plan::Warp { beacon: v.beacon });
    }

    #[test]
    fn the_greedy_take_a_poorer_bank_than_the_content() {
        let v = view((FLOOR_CONTENT + FLOOR_GREEDY) / 2.0, None);
        let content = Brief {
            greed: 0.0,
            ..brief(Some(&v))
        };
        let greedy = Brief {
            greed: 1.0,
            ..brief(Some(&v))
        };
        assert_eq!(decide(&content), Plan::Skip(Why::Poor));
        assert_eq!(decide(&greedy), Plan::Warp { beacon: v.beacon });
    }

    #[test]
    fn a_defended_occupied_or_cooling_bank_is_left_and_the_timid_never_try() {
        let v = view(FLOOR_CONTENT * 2.0, None);
        let tol = tolerance(0.8, 0.5);
        let defended = Brief {
            others_local: tol + 1,
            ..brief(Some(&v))
        };
        assert_eq!(decide(&defended), Plan::Skip(Why::Defended));
        let on_grid = Brief {
            grid_hostiles: tol + 1,
            ..brief(Some(&v))
        };
        assert_eq!(decide(&on_grid), Plan::Skip(Why::Defended));
        let taken = view(FLOOR_CONTENT * 2.0, Some(ME + 1));
        assert_eq!(decide(&brief(Some(&taken))), Plan::Skip(Why::Occupied));
        let cooling = Brief {
            cooling: true,
            ..brief(Some(&v))
        };
        assert_eq!(decide(&cooling), Plan::Skip(Why::Cooling));
        let timid = Brief {
            risk: MIN_RISK / 2.0,
            ..brief(Some(&v))
        };
        assert_eq!(decide(&timid), Plan::Skip(Why::Timid));
        let refused = Brief {
            refused: MAX_REFUSED,
            ..brief(Some(&v))
        };
        assert_eq!(decide(&refused), Plan::Skip(Why::Refused));
    }

    #[test]
    fn nerve_sets_how_many_defenders_it_tolerates() {
        assert_eq!(tolerance(0.1, 0.1), 0);
        assert!(tolerance(0.9, 0.9) > tolerance(0.5, 0.5));
    }

    #[test]
    fn a_held_link_is_held_read_again_and_dropped_when_hostiles_come() {
        let mine = view(FLOOR_CONTENT, Some(ME));
        let linking = Brief {
            stage: Stage::Linking,
            ..brief(Some(&mine))
        };
        assert_eq!(decide(&linking), Plan::Hold);
        let stale = Brief {
            age: HOLD_READ_EVERY,
            ..linking.clone()
        };
        assert_eq!(decide(&stale), Plan::Read);
        let tol = tolerance(0.8, 0.5);
        let hostile = Brief {
            grid_hostiles: tol + 1,
            ..linking.clone()
        };
        assert_eq!(decide(&hostile), Plan::Unlink);
        // The bank no longer lists the bot: the raid is over.
        let paid = view(0.0, None);
        let over = Brief {
            stage: Stage::Linking,
            ..brief(Some(&paid))
        };
        assert_eq!(decide(&over), Plan::Done);
    }

    #[test]
    fn the_panel_dict_is_read() {
        let d = eve_server::pyutil::pydict(vec![
            ("essID", PyValue::Int(1)),
            ("beaconID", PyValue::Int(777)),
            (
                "mainBankLink",
                eve_server::pyutil::pydict(vec![("characterID", PyValue::Int(ME))]),
            ),
            ("mainValue", PyValue::Real(123.0)),
        ]);
        let v = parse(SYS, &d, Instant::now()).expect("a view");
        assert_eq!((v.beacon, v.value, v.linked_by), (777, 123.0, Some(ME)));
        let free = eve_server::pyutil::pydict(vec![
            ("beaconID", PyValue::Int(778)),
            ("mainBankLink", PyValue::Dict(vec![])),
            ("mainValue", PyValue::Real(5.0)),
        ]);
        assert_eq!(
            parse(SYS, &free, Instant::now()).map(|v| v.linked_by),
            Some(None)
        );
    }

    #[test]
    fn a_refused_link_cools_the_bank_and_counts() {
        let now = Instant::now();
        let mut m = Mind::default();
        m.on_link(false, now);
        assert_eq!(m.refused, 1);
        assert!(m.cooled_until.is_some_and(|t| t > now));
        m.on_link(true, now);
        assert_eq!(m.stage, Stage::Linking);
    }
}
