//! **The freighter hauler** (X-FREIGHT, R-CAP packet CAP-L; lane note
//! `docs/lanes/cap-l.md`): a bot that owns a freighter and hauls bulk goods
//! between stations by gate. The hull is the only difference from
//! [`crate::trader`]: the trader's own cycle ([`crate::trader::run`]) probes
//! the books, picks the best deal the hold allows and flies it, so a
//! freighter's hold (hundreds of thousands of m3) makes the deals big, and
//! the hauler's own rules only decide **what it may fly and what is worth its
//! trip**:
//!
//! - it flies a freighter it **owns** ([`pick`]), boarded from its hangar or
//!   bought on the market ([`crate::capital::get_hull`]); **never GM**
//!   (nothing from nowhere): without one it says `MISSING`;
//! - it **arbitrages between regions** (FE-TRADE): the trader's cycle plans on
//!   the desk's price memory of other regions ([`crate::price_memory`]), and a
//!   freighter's hold makes a spread over the round trip and the higher
//!   [`MIN_PROFIT`] worth a long haul; it never speculates ([`run`] turns the
//!   desk's speculation off for the cycle);
//! - it trades only **bulk** goods ([`bulk_goods`]: a unit of at least
//!   [`BULK_M3`]), since a few thousand units of a one-m3 good do not fill a
//!   freighter's hold, and only where the profit pays the trip
//!   ([`costs`]: a higher floor and a dearer jump than the trader's);
//! - it stays above [`crate::travel::SECURITY_FLOOR`] (the policy's floor): a
//!   freighter is a juicy target, and no cyno is used (a jump freighter
//!   hauls by gate too: the jump pair is [`crate::capital`]'s).
//!
//! Every number is **[A]** policy.

use crate::brain::{self, Act, Memory, Observation};
use crate::capital::{self, FREIGHTER_GROUPS};
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::trader::{self, Costs, Setup, SkillUse, Traded};
use crate::travel::Trips;
use crate::verbs::ItemRow;
use crate::{Conn, Error, Result};
use std::time::Duration;

/// How often a hauler runs one cycle, docked **[A]**: a freighter's trip is
/// long, so it looks less often than a trader.
pub const HAUL_EVERY: (Duration, Duration) =
    (Duration::from_secs(10 * 60), Duration::from_secs(25 * 60));

/// The smallest unit volume a hauler trades, m3 **[A]**. Falsified by a
/// hauler whose hold stays under a tenth full, or one that finds no good.
pub const BULK_M3: f64 = 1.0;
/// The least profit a freighter trip must clear, ISK **[A]**: a million, 50
/// times the trader's floor, since the hull is slow and dear to lose.
pub const MIN_PROFIT: f64 = 1_000_000.0;
/// The cost of one jump, ISK **[A]**: ten times the trader's; a freighter
/// aligns slowly and pays for the risk.
pub const ISK_PER_JUMP: f64 = 150_000.0;

/// What the freighter trader's cycle is judged by.
pub fn costs() -> Costs {
    Costs {
        isk_per_jump: ISK_PER_JUMP,
        min_profit: MIN_PROFIT,
        ..Costs::default()
    }
}

/// **The goods a freighter hauls**: the trade goods of `content` whose unit
/// volume is at least [`BULK_M3`], by type id.
pub fn bulk_goods(content: &eve_server::content::Content) -> Vec<i64> {
    bulk_of(content, trader::trade_goods(content))
}

/// Of `types`, those whose unit volume is at least [`BULK_M3`].
pub fn bulk_of(content: &eve_server::content::Content, types: Vec<i64>) -> Vec<i64> {
    types
        .into_iter()
        .filter(|&t| eve_server::item::find_in(content, t).is_some_and(|i| i.volume >= BULK_M3))
        .collect()
}

/// **The freighter the hauler flies** from `hangar`: the active ship if it is
/// one, else the owned one with the lowest item id.
pub fn pick(hangar: &[ItemRow], active: Option<i64>) -> Option<&ItemRow> {
    hangar
        .iter()
        .find(|r| Some(r.item_id) == active && FREIGHTER_GROUPS.contains(&r.group_id))
        .or_else(|| capital::owned_hull(hangar, &FREIGHTER_GROUPS))
}

/// The `--loop freighter` row: a civilian that hauls bulk goods by gate.
pub static ROLE: RoleSpec = RoleSpec {
    name: "freighter",
    greeting: "o7 -- hauling bulk, not fighting",
    civilian: true,
    docked,
    in_space,
    banner,
    setup,
};

/// What the hauler lacks without a freighter it owns.
pub const MISSING_HULL: &str = "a freighter it owns (bought on the market; never given)";

/// Docked: a freighter first (boarded, else `Act::Haul`'s own shopping), a
/// map, then one [`Act::Haul`] when its time has come and a wait.
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if o.failures.values().any(|&n| n >= brain::GIVE_UP_AFTER) {
        return vec![Act::Reconnect];
    }
    m.homing = None;
    let active = o.ship.map(|s| s.ship_id);
    // SKILLGATE: a freighter the pilot's skills do not board yet is skipped.
    let usable: Vec<crate::ItemRow> = o
        .hangar
        .iter()
        .filter(|r| Some(r.item_id) == active || !m.combat.untrained.contains(&r.type_id))
        .copied()
        .collect();
    let flying = pick(&usable, active).is_some_and(|r| Some(r.item_id) == active);
    if !flying && let Some(r) = pick(&usable, active) {
        return vec![Act::Board(r.item_id)];
    }
    if m.policy.travel.is_none() {
        return vec![
            Act::Missing(trader::MISSING_MAP),
            Act::Wait(brain::MISSING_WAIT),
        ];
    }
    if let Some(next) = m.market_next
        && o.now < next
    {
        return vec![Act::Wait(next - o.now)];
    }
    let next = o.now + m.jitter(HAUL_EVERY);
    m.market_next = Some(next);
    // Not flying one: the act shops, or says what is missing.
    vec![Act::Haul, Act::Wait(next - o.now)]
}

/// In space: home, the miner's way.
fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    match &o.park {
        Some(park) => brain::go_home(m, o, park),
        None => vec![Act::Wait(brain::SHORT_WAIT)],
    }
}

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, reconnects {}",
        s.name, s.phase, s.reconnects
    )
}

/// **Nothing from nowhere**: no slash line.
fn setup(_: Lacks) -> Vec<String> {
    Vec::new()
}

/// **One hauler cycle, docked**: be in a freighter (own one, board one, or buy
/// one inside the wallet, [`capital::get_hull`]); then the trader's cycle over
/// the bulk goods with the freighter's [`costs`]. Without a hull it is `Ok`
/// with a note and no trade.
pub fn run(
    c: &mut Conn,
    trips: Trips<'_>,
    content: Option<&eve_server::content::Content>,
    offset: usize,
    shop_next: &mut Option<std::time::Instant>,
    desk: &mut crate::price_memory::Desk,
) -> Result<Traded> {
    let mut got = Traded::default();
    let Some(content) = content else {
        got.notes
            .push("no type table (eve-bots --content): no volumes, no hold".to_owned());
        return Ok(got);
    };
    if c.session().station_id().is_none() {
        return Err(Error::NotSent("haul: not docked".to_owned()));
    }
    match capital::get_hull(c, content, &FREIGHTER_GROUPS, shop_next)? {
        Ok(how) => got.notes.push(format!("freighter: {how}")),
        Err(why) => {
            got.notes.push(format!("freighter: {why}"));
            return Ok(got);
        }
    }
    // The trade goods and every watched type (minerals, dear hulls, modules,
    // charges) that fills a freighter's hold (a unit of a cubic metre or more).
    let index = c.adjusted_prices().unwrap_or_default();
    let mut goods = bulk_goods(content);
    goods.extend(bulk_of(content, trader::watch_set(content, &index)));
    goods.sort_unstable();
    goods.dedup();
    if goods.is_empty() {
        got.notes.push("no bulk good in the type table".to_owned());
        return Ok(got);
    }
    let setup = Setup {
        trips,
        content: Some(content),
        types: &goods,
        offset,
        couriers: false,
        costs: costs(),
        skills: SkillUse::Own,
    };
    // A freighter does not speculate: it carries.
    let knobs = desk.knobs;
    desk.knobs = knobs.off();
    let traded = trader::run_with(c, &setup, desk);
    desk.knobs = knobs;
    let mut traded = traded?;
    traded.notes.extend(got.notes);
    Ok(traded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Place, Policy, ShipInfo};
    use std::time::Instant;

    fn row(id: i64, group: i64, singleton: bool) -> ItemRow {
        ItemRow {
            item_id: id,
            type_id: 990_700_000 + group,
            owner_id: 1,
            location_id: 2,
            flag_id: 4,
            quantity: 1,
            group_id: group,
            category_id: capital::CATEGORY_SHIP,
            singleton,
        }
    }

    fn obs(hangar: Vec<ItemRow>, active: Option<i64>) -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.place = Place::Docked { station: 9 };
        o.hangar = hangar;
        o.ship = active.map(|ship_id| ShipInfo {
            ship_id,
            type_id: Some(990_700_513),
        });
        o
    }

    fn memory(map: bool) -> Memory {
        let mut p = Policy::human();
        p.role = &ROLE;
        if map {
            p.travel = Some(std::sync::Arc::new(crate::travel::Map::new()));
        }
        Memory::new("Invented Hauler", p)
    }

    #[test]
    fn it_picks_the_active_freighter_else_an_owned_one() {
        let hangar = vec![row(30, 513, true), row(20, 902, true), row(10, 485, true)];
        assert_eq!(pick(&hangar, Some(30)).map(|r| r.item_id), Some(30));
        assert_eq!(pick(&hangar, Some(10)).map(|r| r.item_id), Some(20));
        assert_eq!(pick(&hangar, None).map(|r| r.item_id), Some(20));
        assert!(pick(&[row(10, 485, true)], None).is_none());
        // A packaged hull is not a ship it can fly.
        assert!(pick(&[row(30, 513, false)], None).is_none());
    }

    #[test]
    fn a_hauler_with_no_hull_shops_and_is_never_given_one() {
        assert!(
            setup(Lacks {
                ship: true,
                module: false,
                ammo: false,
                isk: true
            })
            .is_empty()
        );
        let mut m = memory(true);
        let acts = docked(&mut m, &obs(vec![], None));
        assert_eq!(acts.first(), Some(&Act::Haul), "{acts:?}");
        assert!(
            !acts
                .iter()
                .any(|a| matches!(a, Act::GmSetup { .. } | Act::Acquire { .. }))
        );
    }

    #[test]
    fn an_owned_freighter_in_the_hangar_is_boarded_first() {
        let mut m = memory(true);
        let acts = docked(&mut m, &obs(vec![row(30, 513, true)], Some(5)));
        assert_eq!(acts, vec![Act::Board(30)]);
    }

    #[test]
    fn flying_the_freighter_with_a_map_it_hauls_then_waits() {
        let mut m = memory(true);
        let acts = docked(&mut m, &obs(vec![row(30, 513, true)], Some(30)));
        assert_eq!(acts.first(), Some(&Act::Haul));
        assert!(matches!(acts.get(1), Some(Act::Wait(_))));
        // Its time has not come again.
        let again = docked(&mut m, &obs(vec![row(30, 513, true)], Some(30)));
        assert!(matches!(again.as_slice(), [Act::Wait(_)]));
    }

    #[test]
    fn with_no_map_it_says_so() {
        let mut m = memory(false);
        let acts = docked(&mut m, &obs(vec![row(30, 513, true)], Some(30)));
        assert!(matches!(acts.first(), Some(Act::Missing(w)) if *w == trader::MISSING_MAP));
    }

    #[test]
    fn the_freighters_costs_are_dearer_than_the_traders() {
        let (f, t) = (costs(), Costs::default());
        assert!(f.min_profit > t.min_profit && f.isk_per_jump > t.isk_per_jump);
        assert_eq!((f.sales_tax, f.broker_fee), (t.sales_tax, t.broker_fee));
    }

    #[test]
    fn only_bulk_goods_are_hauled() {
        let content = eve_server::content::Content::fixture();
        // The fixture has no commodity rows in the trade groups: nothing, not
        // a panic.
        assert!(bulk_goods(&content).is_empty());
    }

    #[test]
    fn a_failing_hauler_reconnects() {
        let mut m = memory(true);
        let mut o = obs(vec![], None);
        o.failures
            .insert(brain::ActKind::Haul, brain::GIVE_UP_AFTER);
        assert_eq!(docked(&mut m, &o), vec![Act::Reconnect]);
    }
}
