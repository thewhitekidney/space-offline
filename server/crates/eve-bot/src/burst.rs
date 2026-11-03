//! **The fleet director's command burst** (K-BOOST, wave K). Lane note
//! `docs/lanes/k-boost.md`.
//!
//! A [`crate::fleet::Director`] that owns a command burst module and its
//! charges fits it, onlines it, loads a charge and runs it, so the fleet
//! around him is boosted (`eve-server/src/dbuff.rs`). **One decision point**:
//! [`Director::burst`] reads what the director holds, plans one [`Step`]
//! ([`plan`], pure), does it and answers it; a caller (the society thread, a
//! test) calls it again on its next turn until it answers [`Step::Running`]
//! or [`Step::Lacks`].
//!
//! **Nothing is made from nowhere**: the burst and its charges are what the
//! director *owns* (hangar or fitted). A missing one is [`Step::Lacks`],
//! naming it, for the closed economy's own acquire path to buy
//! ([`KIT`], fed to [`crate::brain::acquire_if_due`] like every role's kit);
//! this file never calls a GM verb.
//!
//! **Functions touched**: none outside this file. [`Director::burst`] is a
//! new method on [`Director`] (its fields are public); `lib.rs` gets one
//! `pub mod burst;` line (coordinator diff).
//!
//! What is the client's [V] and what is policy [A]:
//!
//! - the burst module's group, the effect each kind runs and the charge
//!   group each kind takes are **[V-data]** (`types/typedata.tsv`: the five
//!   `moduleBonusWarfareLink*` effects 6732-6736, `chargeGroup1` 604 on the
//!   module); the module's own 604 says which charge group it takes, so a
//!   charge is matched by group, never by a list of types here;
//! - **[A]** the director runs the burst whenever it is ready and in space,
//!   and leaves it running (`repeat` 1000, until stopped, as the client's
//!   own button sends, `smb:1565`): the fleet's boost is kept up while he
//!   flies. Falsified by a director whose fleet wants the burst off (a
//!   stop is `Conn::deactivate` on the module).

use crate::acquire::Kit;
use crate::fleet::Director;
use crate::{Error, ItemRow, Result};

/// The group of every command burst module **[V-data]**.
pub const GROUP_BURST: i64 = 1770;
/// `chargeGroup1` [V `dogmaattributes[604]`]: the charge group a module takes.
pub const ATTR_CHARGE_GROUP: i64 = 604;
/// `categoryCharge` [V].
pub const CATEGORY_CHARGE: i64 = 8;
/// `activate`'s `repeat`: until stopped (the client sends 1000, `smb:1565`).
pub const REPEAT: i64 = 1_000;

/// The burst effects the client names [V `dogma/const.pyc`]: `(charge group
/// the kind takes, effect name, effect id)` **[V-data]** (the groups are the
/// `chargeGroup1` of the modules of that effect in `typedata`).
pub const KINDS: [(i64, &str, i64); 6] = [
    (1769, "moduleBonusWarfareLinkShield", 6733),
    (1774, "moduleBonusWarfareLinkArmor", 6732),
    (1772, "moduleBonusWarfareLinkSkirmish", 6734),
    (1773, "moduleBonusWarfareLinkInfo", 6735),
    (4905, "moduleBonusWarfareLinkInfo", 6735),
    (1771, "moduleBonusWarfareLinkMining", 6736),
];

/// Every burst charge type with its group **[V-data]** (`types/typedata.tsv`:
/// category 8 types carrying effect 6737). A loaded charge is not an item
/// the director can list (`item::is_loaded_charge`), so what a module holds
/// is asked of the types of its group ([`held`]).
pub const CHARGES: [(i64, i64); 19] = [
    (42_694, 1769),
    (42_695, 1769),
    (42_696, 1769),
    (42_829, 1771),
    (42_830, 1771),
    (42_831, 1771),
    (90_733, 1771),
    (42_838, 1772),
    (42_839, 1772),
    (42_840, 1772),
    (42_835, 1773),
    (42_836, 1773),
    (42_837, 1773),
    (42_832, 1774),
    (42_833, 1774),
    (42_834, 1774),
    (89_612, 4905),
    (89_613, 4905),
    (89_614, 4905),
];

/// The effect a module taking charges of `charge_group` runs, `None` for a
/// group no burst takes.
pub fn effect_of(charge_group: i64) -> Option<(&'static str, i64)> {
    KINDS
        .iter()
        .find(|k| k.0 == charge_group)
        .map(|k| (k.1, k.2))
}

/// **The closed economy's kit** for the burst **[A]**: a Shield Command
/// Burst I and ten Shield Harmonizing Charges, with the two skills the
/// module asks for (Leadership and Shield Command, `requiredSkill1/2`
/// **[V-data]**). Shield, because a fleet's first need is its shield
/// resists; falsified by a director whose fleet fights armour-first.
pub const KIT: Kit = Kit {
    hull: None,
    modules: Some((42_529, 1)),
    ammo: Some((42_695, KIT_CHARGES)),
    skills: &[(3_348, 1), (3_350, 1)],
    more: &[],
};
/// Charges the kit buys **[A]**: one is used per cycle (`chargeRate` 1
/// **[V-data]**), a minute each; ten is ten minutes of boost.
pub const KIT_CHARGES: i64 = 10;

/// One burst module the director holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Module {
    pub item_id: i64,
    pub type_id: i64,
    /// Where it sits: a module flag when fitted, the hangar's when not.
    pub flag_id: i64,
    pub fitted: bool,
    pub online: bool,
    pub running: bool,
    /// Its `chargeGroup1`, 0 when not read (an unfitted module).
    pub charge_group: i64,
    /// Rounds loaded.
    pub loaded: i64,
}

/// One charge stack the director holds in the hangar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Charge {
    pub item_id: i64,
    pub type_id: i64,
    pub group_id: i64,
    pub quantity: i64,
}

/// What the director lacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lack {
    /// No command burst module at all.
    Module,
    /// A fitted burst with no charge of its group in the hangar.
    Charge { group: i64 },
    /// SKILLGATE: the module is owned but the pilot's skills do not fit it
    /// yet ([`step`] queues them); only [`step`] answers it, never [`plan`].
    Skill,
}

/// One step of the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Nothing to do: it lacks something it must own.
    Lacks(Lack),
    /// Docked: fit the module `item`.
    Fit { item: i64 },
    /// Docked: online the fitted module.
    Online { item: i64 },
    /// Docked: load `charges` (stacks of `type_id`) into the module.
    Load {
        module: i64,
        flag: i64,
        charges: Vec<i64>,
        type_id: i64,
    },
    /// In space: run the module's effect.
    Run {
        module: i64,
        effect: (&'static str, i64),
    },
    /// Docked, loaded and online: ready for space.
    Ready,
    /// In space and not ready (a module that was never loaded or onlined):
    /// nothing can be done until he docks.
    Dock,
    /// Already running.
    Running,
}

/// **The decision**: what the director does next with `modules` (fitted
/// first, then the hangar's) and the hangar's `charges`. Pure.
pub fn plan(modules: &[Module], charges: &[Charge], docked: bool) -> Step {
    // The module in hand: one already running, else a fitted one, else the
    // first in the hangar.
    let module = modules
        .iter()
        .find(|m| m.running)
        .or_else(|| modules.iter().find(|m| m.fitted))
        .or_else(|| modules.first());
    let Some(m) = module else {
        return Step::Lacks(Lack::Module);
    };
    if m.running {
        return Step::Running;
    }
    if !docked {
        // Fitting, onlining and loading are station work.
        return match (
            m.fitted && m.online && m.loaded > 0,
            effect_of(m.charge_group),
        ) {
            (true, Some(effect)) => Step::Run {
                module: m.item_id,
                effect,
            },
            _ => Step::Dock,
        };
    }
    if !m.fitted {
        return Step::Fit { item: m.item_id };
    }
    if !m.online {
        return Step::Online { item: m.item_id };
    }
    if m.loaded > 0 {
        return Step::Ready;
    }
    let mine: Vec<&Charge> = charges
        .iter()
        .filter(|c| c.group_id == m.charge_group && c.quantity > 0)
        .collect();
    let Some(first) = mine.first() else {
        return Step::Lacks(Lack::Charge {
            group: m.charge_group,
        });
    };
    Step::Load {
        module: m.item_id,
        flag: m.flag_id,
        // The stacks of one type, the first's.
        charges: mine
            .iter()
            .filter(|c| c.type_id == first.type_id)
            .map(|c| c.item_id)
            .collect(),
        type_id: first.type_id,
    }
}

/// **Rounds loaded** in the module at `flag`: the first non-empty count of
/// `ItemGetInfo((ship, flag, type))` over the burst charges of `group`
/// ([`CHARGES`]); 0 for none.
fn held(conn: &mut crate::Conn, ship: i64, flag: i64, group: i64) -> Result<i64> {
    for (t, g) in CHARGES {
        if g != group {
            continue;
        }
        let n = conn.loaded_rounds(ship, flag, t)?;
        if n > 0 {
            return Ok(n);
        }
    }
    Ok(0)
}

/// **The one decision point**: read what the pilot owns, plan one step
/// ([`plan`]), do it, and answer it. A refusal of the step is an
/// `Err(Error::Refused)`; nothing is bought or made here. (PROG-MINE: a
/// free function over the connection, so the foreman role runs it too;
/// [`Director::burst`] is this on its own connection.)
pub fn step(conn: &mut crate::Conn) -> Result<Step> {
    let step = peek(conn)?;
    match &step {
        Step::Fit { item } => {
            if crate::acquire::fit_auto_gated(conn, &[*item])?.is_err() {
                return Ok(Step::Lacks(Lack::Skill));
            }
        }
        Step::Online { item } => {
            if crate::acquire::set_online_gated(conn, *item)?.is_err() {
                return Ok(Step::Lacks(Lack::Skill));
            }
        }
        Step::Load {
            module,
            flag,
            charges,
            type_id,
        } => {
            let station = conn
                .session()
                .station_id()
                .ok_or_else(|| Error::NotSent("burst: loading is station work".to_owned()))?;
            conn.load_ammo(&[(*module, *flag)], charges, *type_id, station)?;
        }
        Step::Run { module, effect } => {
            conn.activate(*module, *effect, None, REPEAT)?;
        }
        Step::Lacks(_) | Step::Ready | Step::Dock | Step::Running => {}
    }
    Ok(step)
}

/// **What [`step`] would do**, without doing it: read what the pilot owns
/// and plan ([`plan`]).
pub fn peek(conn: &mut crate::Conn) -> Result<Step> {
    let docked = conn.session().station_id().is_some();
    let ship = conn
        .session()
        .ship_id()
        .ok_or_else(|| Error::NotSent("burst: the session names no ship".to_owned()))?;
    let on_ship = conn.ship_items(ship, None)?;
    let hangar = if docked {
        conn.hangar_items()?
    } else {
        Vec::new()
    };
    let mut modules = Vec::new();
    for row in on_ship
        .iter()
        .filter(|r| r.is_fitted() && r.group_id == GROUP_BURST)
    {
        let info = conn.module_row(row.item_id)?;
        let group = info.attribute(ATTR_CHARGE_GROUP).unwrap_or(0.0) as i64;
        let loaded = held(conn, ship, row.flag_id, group)?;
        let effect = effect_of(group);
        modules.push(Module {
            item_id: row.item_id,
            type_id: row.type_id,
            flag_id: row.flag_id,
            fitted: true,
            online: info.effects.contains(&crate::EFFECT_ONLINE),
            running: effect.is_some_and(|(_, id)| info.effects.contains(&id)),
            charge_group: group,
            loaded,
        });
    }
    let unfitted = |r: &&ItemRow| r.group_id == GROUP_BURST && !r.is_fitted();
    modules.extend(hangar.iter().filter(unfitted).map(|r| Module {
        item_id: r.item_id,
        type_id: r.type_id,
        flag_id: r.flag_id,
        fitted: false,
        online: false,
        running: false,
        charge_group: 0,
        loaded: 0,
    }));
    let charges: Vec<Charge> = hangar
        .iter()
        .filter(|r| r.category_id == CATEGORY_CHARGE)
        .map(|r| Charge {
            item_id: r.item_id,
            type_id: r.type_id,
            group_id: r.group_id,
            quantity: r.quantity,
        })
        .collect();
    Ok(plan(&modules, &charges, docked))
}

impl Director {
    /// [`step`] on the director's connection.
    pub fn burst(&mut self) -> Result<Step> {
        step(&mut self.conn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids; the groups stand for any burst charge group.
    const MOD_A: i64 = 990_592_001;
    const MOD_B: i64 = 990_592_002;
    const TYPE: i64 = 990_592_011;
    const FLAG: i64 = 27;
    const HANGAR: i64 = 4;
    const GROUP: i64 = 990_592_021;
    const OTHER_GROUP: i64 = 990_592_022;
    const CHARGE_T: i64 = 990_592_031;
    const CHARGE_U: i64 = 990_592_032;

    fn hangar_module(item: i64) -> Module {
        Module {
            item_id: item,
            type_id: TYPE,
            flag_id: HANGAR,
            fitted: false,
            online: false,
            running: false,
            charge_group: 0,
            loaded: 0,
        }
    }

    fn ready(item: i64) -> Module {
        Module {
            item_id: item,
            flag_id: FLAG,
            fitted: true,
            online: true,
            charge_group: KINDS[0].0,
            loaded: 5,
            ..hangar_module(item)
        }
    }

    fn charge(item: i64, type_id: i64, group: i64, quantity: i64) -> Charge {
        Charge {
            item_id: item,
            type_id,
            group_id: group,
            quantity,
        }
    }

    #[test]
    fn every_kind_names_its_effect_and_the_group_it_takes() {
        for (group, name, id) in KINDS {
            assert_eq!(effect_of(group), Some((name, id)));
            assert!(name.starts_with("moduleBonusWarfareLink"));
        }
        assert_eq!(effect_of(GROUP), None, "a group no burst takes");
        // The five effects the server's `dbuff` pulses are the ones named
        // here (the expedition and info kinds share one).
        let mut ids: Vec<i64> = KINDS.iter().map(|k| k.2).collect();
        ids.sort_unstable();
        ids.dedup();
        let mut want = eve_server::dbuff::EFFECTS.to_vec();
        want.sort_unstable();
        assert_eq!(ids, want);
    }

    #[test]
    fn every_charge_is_of_a_kind_and_every_kind_has_charges() {
        for (t, g) in CHARGES {
            assert!(effect_of(g).is_some(), "charge {t}: group {g} is a kind");
        }
        for (g, ..) in KINDS {
            assert!(CHARGES.iter().any(|c| c.1 == g), "group {g} has a charge");
        }
        let mut ids: Vec<i64> = CHARGES.iter().map(|c| c.0).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CHARGES.len(), "no type twice");
        // The kit's charge is one the director can find loaded.
        assert!(CHARGES.iter().any(|c| Some(c.0) == KIT.ammo.map(|a| a.0)));
    }

    #[test]
    fn a_director_with_no_burst_lacks_one_and_buys_it() {
        assert_eq!(plan(&[], &[], true), Step::Lacks(Lack::Module));
        assert_eq!(plan(&[], &[], false), Step::Lacks(Lack::Module));
        // The closed economy's kit names a module and charges to buy.
        assert!(KIT.modules.is_some() && KIT.ammo.is_some());
    }

    #[test]
    fn docked_it_fits_then_onlines_then_loads_then_is_ready() {
        let charges = [charge(1, CHARGE_T, KINDS[0].0, 10)];
        let mut m = hangar_module(MOD_A);
        assert_eq!(plan(&[m], &charges, true), Step::Fit { item: MOD_A });
        m.fitted = true;
        m.flag_id = FLAG;
        m.charge_group = KINDS[0].0;
        assert_eq!(plan(&[m], &charges, true), Step::Online { item: MOD_A });
        m.online = true;
        assert_eq!(
            plan(&[m], &charges, true),
            Step::Load {
                module: MOD_A,
                flag: FLAG,
                charges: vec![1],
                type_id: CHARGE_T,
            }
        );
        m.loaded = 3;
        assert_eq!(plan(&[m], &charges, true), Step::Ready);
    }

    #[test]
    fn it_loads_only_a_charge_of_the_modules_group_and_one_type() {
        let mut m = ready(MOD_A);
        m.loaded = 0;
        let charges = [
            charge(1, CHARGE_T, OTHER_GROUP, 99),
            charge(2, CHARGE_U, KINDS[0].0, 4),
            charge(3, CHARGE_T, KINDS[0].0, 6),
            charge(4, CHARGE_U, KINDS[0].0, 2),
            charge(5, CHARGE_T, KINDS[0].0, 0),
        ];
        assert_eq!(
            plan(&[m], &charges, true),
            Step::Load {
                module: MOD_A,
                flag: FLAG,
                charges: vec![2, 4],
                type_id: CHARGE_U,
            },
            "the first stack's type, every stack of it, none empty or of another group"
        );
    }

    #[test]
    fn with_no_charge_of_its_group_it_lacks_one() {
        let mut m = ready(MOD_A);
        m.loaded = 0;
        let charges = [
            charge(1, CHARGE_T, OTHER_GROUP, 99),
            charge(2, CHARGE_T, KINDS[0].0, 0),
        ];
        assert_eq!(
            plan(&[m], &charges, true),
            Step::Lacks(Lack::Charge { group: KINDS[0].0 })
        );
    }

    #[test]
    fn in_space_a_ready_burst_runs_and_a_running_one_is_left_alone() {
        let m = ready(MOD_A);
        assert_eq!(
            plan(&[m], &[], false),
            Step::Run {
                module: MOD_A,
                effect: (KINDS[0].1, KINDS[0].2),
            }
        );
        let mut running = m;
        running.running = true;
        assert_eq!(plan(&[running], &[], false), Step::Running);
        assert_eq!(plan(&[running], &[], true), Step::Running, "docked too");
    }

    #[test]
    fn in_space_an_unready_burst_waits_for_the_station() {
        let mut m = ready(MOD_A);
        m.loaded = 0;
        assert_eq!(plan(&[m], &[], false), Step::Dock);
        let mut m = ready(MOD_A);
        m.online = false;
        assert_eq!(plan(&[m], &[], false), Step::Dock);
        // A charge group no burst takes cannot be run.
        let mut m = ready(MOD_A);
        m.charge_group = GROUP;
        assert_eq!(plan(&[m], &[], false), Step::Dock);
    }

    #[test]
    fn the_fitted_one_is_preferred_over_a_spare_in_the_hangar() {
        let spare = hangar_module(MOD_A);
        let fitted = ready(MOD_B);
        assert_eq!(
            plan(&[spare, fitted], &[], false),
            Step::Run {
                module: MOD_B,
                effect: (KINDS[0].1, KINDS[0].2),
            }
        );
        assert_eq!(plan(&[spare, fitted], &[], true), Step::Ready);
    }
}
