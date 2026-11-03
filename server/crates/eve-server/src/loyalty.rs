//! Loyalty points: the per-corporation LP wallet and the LP store's offers.
//!
//! W15 packet CP37b (`docs/content/plan.md` §6.2), spec C4 §4-§5 (K5) and
//! C5 §5.6. The offers are CP37a's `agents/lp-offers.tsv`
//! (`docs/content/schema.md`), each corporation selling the union of its
//! `npc/corporations.tsv` `lp_offer_tables`.
//!
//! - **The wallet** is the `lp` delta kind (plan §4.2): `char corp ->
//!   balance`, never rolled over. A zero balance has no row. [`balances`] is
//!   `LPSvc.GetAllMyCharacterWalletLPBalances`' list of `(corpID, balance)`
//!   [V] (`loyaltyPointsWalletSvc.py:35`); every change pushes the 4-argument
//!   `OnLPChange(changeType, issuerCorpID, lpBefore, lpAfter)` [V]
//!   (`loyaltyPointsWalletSvc.py:67`, `findings/push-events.tsv`).
//! - **Credit** comes from missions: [`on_complete`] is a
//!   `mission_hooks::HOOKS` row on `Change::Completed` and credits
//!   `mission_rewards::reward(..).lp` (the row's `fixed_lp` when set, the
//!   Omega value, erratum 13 [A]) to the agent's corporation.
//! - **The store**: [`SLOT`] holds each corporation's offers, merged across
//!   its tables. Two tables of one corporation that sell the same product
//!   for the same requirements (CP37a: the five universal tables) show it
//!   once, at the cheaper price, under the lower offer id [A] (falsifier: a
//!   Tranquility store listing one product twice). The service arms are in
//!   `service/wallet.rs`.
//!
//! With no bundle the offer table is empty, not the fixture: invented offers
//! served to a client would be a lie (the same rule as `econ_prices`).

use crate::content::{Boxed, Bundle, Content, Sources, TableSpec};
use crate::mission_hooks::MissionEvent;
use crate::service::Registry;
use crate::world_ext::SlotSpec;
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "loyalty",
    load: load_table,
    fixture: || Box::new(Table::fixture()) as Boxed,
};

/// The registration row; the coordinator adds it to `world_ext::SLOTS`.
/// Built from content only (never from deltas, CA11).
pub const SLOT: SlotSpec = SlotSpec {
    name: "loyalty",
    make: |c| Box::new(Store::build(c)) as Boxed,
};

/// The delta kind this module owns: `char corp -> balance` (plan §4.2).
pub const KIND: &str = "lp";

/// CP37a's file and its header line, copied from `schema.md`.
pub const OFFERS: (&str, &str) = (
    "agents/lp-offers.tsv",
    "#table_id\toffer_id\ttype_id\tqty\tlp_cost\tisk_cost\treq_items",
);

/// `refPaymentToLPStore` [V] (`eve/common/lib/appConst.pyc`, module
/// constant): the journal entry type of an LP store's ISK part.
pub const REF_PAYMENT_TO_LP_STORE: i64 = 26;

/// `maxLoyaltyStoreBulkOffers` [V] (`appConst.pyc`): the most offers one
/// `TakeOfferForCharacter` may redeem.
pub const MAX_BULK_OFFERS: i64 = 100;

/// `OnLPChange`'s first argument. The handler discards it [V]
/// (`_changeType`); the value is ours [A] (falsifier: a client that reads it).
pub const CHANGE_MISSION: &str = "mission";
/// The same, for a store purchase.
pub const CHANGE_STORE: &str = "store";

/// The bundle's rows, or empty with no bundle (see the module doc).
fn load_table(src: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => Table::from_bundle(b)?,
        None => Table::default(),
    }))
}

/// One offer: `typeID × qty` for `lpCost` LP, `iskCost` ISK and the
/// required items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub offer_id: i64,
    pub type_id: i64,
    pub qty: i64,
    pub lp_cost: i64,
    pub isk_cost: i64,
    /// `(type_id, qty)`, ascending by type.
    pub req_items: Vec<(i64, i64)>,
}

/// The offers by table, and each corporation's tables.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub offers: BTreeMap<i64, Vec<Offer>>,
    pub corp_tables: BTreeMap<i64, Vec<i64>>,
}

impl Table {
    /// `agents/lp-offers.tsv` and the `lp_offer_tables` column of
    /// `npc/corporations.tsv`; an absent file is empty.
    pub fn from_bundle(b: &Bundle) -> Result<Table, String> {
        let (orel, oheader) = OFFERS;
        let (crel, cheader) = crate::npc_data::CORPORATIONS;
        Table::parse(&b.data_lines(orel, oheader)?, &b.data_lines(crel, cheader)?)
    }

    /// The data lines of both files. A bad line is an error naming it.
    pub fn parse<S: AsRef<str>>(offer_lines: &[S], corp_lines: &[S]) -> Result<Table, String> {
        let mut t = Table::default();
        for (i, line) in offer_lines.iter().enumerate() {
            let line = line.as_ref();
            let bad = || format!("{}: data line {}: {line:?}", OFFERS.0, i + 1);
            let (table, offer) = parse_offer(line).ok_or_else(bad)?;
            t.offers.entry(table).or_default().push(offer);
        }
        for (i, line) in corp_lines.iter().enumerate() {
            let line = line.as_ref();
            let f: Vec<&str> = line.split('\t').collect();
            let parsed = (|| {
                let corp = f.first()?.parse::<i64>().ok()?;
                let tables = f
                    .get(11)?
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.parse::<i64>().ok())
                    .collect::<Option<Vec<_>>>()?;
                Some((corp, tables))
            })();
            let (corp, tables) = parsed.ok_or_else(|| {
                format!(
                    "{}: data line {}: {line:?}",
                    crate::npc_data::CORPORATIONS.0,
                    i + 1
                )
            })?;
            if !tables.is_empty() {
                t.corp_tables.insert(corp, tables);
            }
        }
        Ok(t)
    }

    /// Invented rows (ids in the test bands), never a client value. Corp
    /// 1999101 has tables 9901 and 9902, which share one product; corp
    /// 1999102 has 9902 only.
    pub fn fixture() -> Table {
        Table::parse(
            &[
                "9901\t99010000\t990001\t1\t2500\t2500000\t990002=1",
                "9901\t99010001\t990003\t10\t100\t0\t",
                "9902\t99020000\t990003\t10\t100\t0\t",
                "9902\t99020001\t990004\t1\t400\t40000\t990005=2;990006=1",
            ],
            &[
                "1999101\t599001\t\t\t\t\t0.1\t0\t5\t\t\t9901,9902\tAAA\tFixture Corp A",
                "1999102\t599001\t\t\t\t\t0.1\t0\t5\t\t\t9902\tBBB\tFixture Corp B",
                "1999103\t599001\t\t\t\t\t0.1\t0\t5\t\t\t\tCCC\tFixture Corp C",
            ],
        )
        .unwrap_or_else(|e| unreachable!("the fixture parses: {e}"))
    }

    /// A corporation's offers: the union of its tables, merged (module doc),
    /// by ascending offer id.
    pub fn corp_offers(&self, corp: i64) -> Vec<Offer> {
        type Product = (i64, i64, Vec<(i64, i64)>);
        let mut best: BTreeMap<Product, Offer> = BTreeMap::new();
        let tables = self.corp_tables.get(&corp).map_or(&[][..], |v| &v[..]);
        for o in tables.iter().filter_map(|t| self.offers.get(t)).flatten() {
            let key = (o.type_id, o.qty, o.req_items.clone());
            let rank = |o: &Offer| (o.lp_cost, o.isk_cost, o.offer_id);
            match best.get(&key) {
                Some(have) if rank(have) <= rank(o) => {}
                _ => {
                    best.insert(key, o.clone());
                }
            }
        }
        let mut out: Vec<Offer> = best.into_values().collect();
        out.sort_by_key(|o| o.offer_id);
        out
    }
}

fn parse_offer(line: &str) -> Option<(i64, Offer)> {
    let f: Vec<&str> = line.split('\t').collect();
    let [table, offer, type_id, qty, lp, isk, req] = f[..] else {
        return None;
    };
    let int = |s: &str| s.parse::<i64>().ok();
    let mut req_items = req
        .split(';')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((int(k)?, int(v)?))
        })
        .collect::<Option<Vec<_>>>()?;
    req_items.sort_unstable();
    let o = Offer {
        offer_id: int(offer)?,
        type_id: int(type_id)?,
        qty: int(qty)?,
        lp_cost: int(lp)?,
        isk_cost: int(isk)?,
        req_items,
    };
    (o.qty > 0 && o.lp_cost >= 0 && o.isk_cost >= 0 && o.req_items.iter().all(|r| r.1 > 0))
        .then_some((int(table)?, o))
}

/// The world slot: every corporation's merged offers.
#[derive(Debug, Default)]
pub struct Store {
    by_corp: BTreeMap<i64, Vec<Offer>>,
}

impl Store {
    fn build(c: &Content) -> Store {
        let Some(t) = c.try_get::<Table>() else {
            return Store::default();
        };
        let by_corp = t
            .corp_tables
            .keys()
            .map(|&corp| (corp, t.corp_offers(corp)))
            .filter(|(_, v)| !v.is_empty())
            .collect();
        Store { by_corp }
    }

    /// `corp`'s offers, empty for a corporation with no store.
    pub fn offers(&self, corp: i64) -> &[Offer] {
        self.by_corp.get(&corp).map_or(&[], |v| &v[..])
    }

    /// One of `corp`'s offers.
    pub fn offer(&self, corp: i64, offer_id: i64) -> Option<&Offer> {
        self.offers(corp).iter().find(|o| o.offer_id == offer_id)
    }
}

fn key(ch: i64, corp: i64) -> Vec<String> {
    vec![ch.to_string(), corp.to_string()]
}

/// `ch`'s LP with `corp`; 0 when there is no row.
pub fn balance(reg: &Registry, ch: i64, corp: i64) -> i64 {
    reg.deltas()
        .of(KIND)
        .get(&key(ch, corp))
        .and_then(|v| v.first())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Every nonzero balance of `ch`, as `(corp, balance)` by corporation.
pub fn balances(reg: &Registry, ch: i64) -> Vec<(i64, i64)> {
    let me = ch.to_string();
    reg.deltas()
        .of(KIND)
        .iter()
        .filter(|(k, _)| k.first() == Some(&me))
        .filter_map(|(k, v)| {
            let corp = k.get(1)?.parse().ok()?;
            let bal: i64 = v.first()?.parse().ok()?;
            (bal != 0).then_some((corp, bal))
        })
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .collect()
}

/// Add `delta` LP (negative debits) to `ch`'s wallet with `corp`, and push
/// `OnLPChange` to `ch`. Refused (nothing changes, `None`) when the balance
/// would go below zero or overflow; `delta == 0` changes and pushes nothing.
/// Returns `(before, after)`.
pub fn adjust(
    reg: &mut Registry,
    ch: i64,
    corp: i64,
    delta: i64,
    change: &'static str,
) -> Option<(i64, i64)> {
    let before = balance(reg, ch, corp);
    let after = before.checked_add(delta).filter(|a| *a >= 0)?;
    if delta == 0 {
        return Some((before, after));
    }
    let d = reg.deltas_mut();
    if after == 0 {
        d.del(KIND, &key(ch, corp));
    } else {
        d.put(KIND, key(ch, corp), vec![after.to_string()]);
    }
    reg.push(LpChange {
        character_id: ch,
        change,
        corp_id: corp,
        before,
        after,
    });
    Some((before, after))
}

/// `OnLPChange(changeType, issuerCorpID, lpBefore, lpAfter)` [V], to one
/// character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LpChange {
    pub character_id: i64,
    pub change: &'static str,
    pub corp_id: i64,
    pub before: i64,
    pub after: i64,
}

impl crate::notify::Push for LpChange {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnLPChange",
            vec![
                crate::pyutil::bstr(self.change),
                PyValue::Int(self.corp_id),
                PyValue::Int(self.before),
                PyValue::Int(self.after),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character_id])
    }
}

/// The `mission_hooks::HOOKS` row for `Change::Completed`: credit the
/// mission's LP to the agent's corporation. An agent or mission with no pool
/// row (the built-in starter) earns none [A].
pub fn on_complete(reg: &mut Registry, ev: &MissionEvent) {
    let Some(pool) = reg.content().try_get::<crate::mission_pool::Table>() else {
        return;
    };
    let (Some(agent), Some(m)) = (
        pool.agent(ev.agent_id).cloned(),
        pool.mission(ev.mission_id).cloned(),
    ) else {
        return;
    };
    let lp = crate::mission_rewards::reward(reg, &agent, &m).lp;
    if lp > 0 {
        adjust(reg, ev.character_id, agent.corp, lp, CHANGE_MISSION);
    }
}

/// `/givelp <corpID> <amount>`: a GM credit of LP with one corporation (a
/// negative amount takes it back; a balance never goes below zero). There was
/// no way to hold LP except completing a mission, which a client-CI scenario
/// cannot do alone (lane `fci-catchup`). Pushes `OnLPChange` like a mission
/// credit; the change type is the mission's [A] (falsifier: the wallet window
/// drawing a GM credit with the wrong label).
pub fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    words: &[&str],
) -> Result<PyValue, crate::service::ServiceError> {
    let say = |s: &str| Ok(crate::pyutil::bstr(s));
    let (Some(me), [corp, amount]) = (actor, words) else {
        return say("usage: /givelp <corpID> <amount>");
    };
    let (Ok(corp), Ok(amount)) = (corp.parse::<i64>(), amount.parse::<i64>()) else {
        return say("usage: /givelp <corpID> <amount>");
    };
    match adjust(reg, me, corp, amount, CHANGE_MISSION) {
        Some((before, after)) => say(&format!("LP with {corp}: {before} -> {after}")),
        None => say("refused: the balance cannot go below zero"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission::Change;
    use crate::mission_pool as pool;
    use crate::model::SandboxConfig;
    use std::sync::Arc;

    const ME: i64 = 90_000_001;
    const CORP_A: i64 = 1_999_101;
    const CORP_B: i64 = 1_999_102;

    fn world() -> Registry {
        let mut c = Content::fixture();
        c.insert("loyalty", Table::fixture());
        Registry::new(SandboxConfig::default()).with_content(Arc::new(c))
    }

    /// A push as a test reads it: name, arguments, audience.
    type Seen = (&'static str, Vec<PyValue>, Option<Vec<i64>>);

    fn pushes(reg: &mut Registry) -> Vec<Seen> {
        reg.take_pending()
            .into_iter()
            .map(|(_, p)| {
                let (name, args) = p.to_push();
                (name, args, p.audience())
            })
            .collect()
    }

    /// schema.md's invented example row parses under its header.
    #[test]
    fn the_schema_example_row_parses() {
        let t = Table::parse(
            &["9901\t99010000\t990001\t1\t2500\t2500000\t990002=1"],
            &[] as &[&str],
        )
        .expect("parses");
        let o = &t.offers[&9901][0];
        assert_eq!((o.offer_id, o.type_id, o.qty), (99_010_000, 990_001, 1));
        assert_eq!(o.req_items, vec![(990_002, 1)]);
        assert!(Table::parse(&["9901\tx\t1\t1\t1\t1\t"], &[] as &[&str]).is_err());
    }

    /// A corporation sells the union of its tables; a product two of its
    /// tables share is listed once, under the lower offer id.
    #[test]
    fn a_corporation_sells_its_tables_merged() {
        let t = Table::fixture();
        let a = t.corp_offers(CORP_A);
        let mut types: Vec<_> = a.iter().map(|o| (o.type_id, o.qty)).collect();
        types.sort_unstable();
        types.dedup();
        assert_eq!(types.len(), a.len(), "no product twice");
        let all: usize = t.corp_tables[&CORP_A]
            .iter()
            .map(|tb| t.offers[tb].len())
            .sum();
        assert!(a.len() < all, "the shared product is merged");
        let shared = t.offers[&9902][0].clone();
        let kept = a
            .iter()
            .find(|o| (o.type_id, o.qty) == (shared.type_id, shared.qty))
            .expect("listed");
        assert!(kept.offer_id < shared.offer_id);
        assert_eq!(t.corp_offers(CORP_B), t.offers[&9902]);
        assert!(t.corp_offers(1_999_103).is_empty());
        assert!(world().ext::<Store>().offers(1_999_103).is_empty());
        assert_eq!(world().ext::<Store>().offers(CORP_A), &a[..]);
    }

    /// No bundle serves no invented offers.
    #[test]
    fn no_bundle_loads_no_offers() {
        let dir = crate::authored::Dir::compiled_only();
        let src = Sources::new(None, &dir);
        let loaded = (TABLE.load)(&src).expect("loads");
        assert_eq!(loaded.downcast_ref::<Table>(), Some(&Table::default()));
    }

    /// A credit and a debit move the balance, each pushing the 4-argument
    /// `OnLPChange` to the character; an overdraft changes nothing.
    #[test]
    fn adjust_moves_the_balance_and_pushes_on_lp_change() {
        let mut reg = world();
        assert_eq!(
            adjust(&mut reg, ME, CORP_A, 500, CHANGE_MISSION),
            Some((0, 500))
        );
        assert_eq!(
            adjust(&mut reg, ME, CORP_A, -200, CHANGE_STORE),
            Some((500, 300))
        );
        assert_eq!(balances(&reg, ME), vec![(CORP_A, 300)]);
        let p = pushes(&mut reg);
        assert_eq!(p.len(), 2);
        for (name, args, to) in &p {
            assert_eq!(*name, "OnLPChange");
            assert_eq!(args.len(), 4);
            assert_eq!(args[1], PyValue::Int(CORP_A));
            assert_eq!(to.as_deref(), Some(&[ME][..]));
        }
        assert_eq!(p[1].1[2..], [PyValue::Int(500), PyValue::Int(300)]);
        assert_eq!(adjust(&mut reg, ME, CORP_A, -301, CHANGE_STORE), None);
        assert_eq!(balance(&reg, ME, CORP_A), 300);
        assert!(pushes(&mut reg).is_empty());
        adjust(&mut reg, ME, CORP_A, -300, CHANGE_STORE);
        assert!(balances(&reg, ME).is_empty(), "a zero balance has no row");
    }

    /// The wallet is the `lp` delta: a world rebuilt from the rows has the
    /// same balances.
    #[test]
    fn a_balance_survives_a_restart() {
        let mut reg = world();
        adjust(&mut reg, ME, CORP_B, 42, CHANGE_MISSION);
        let mut again = world();
        for (k, v) in reg.deltas().of(KIND).clone() {
            again.deltas_mut().put(KIND, k, v);
        }
        assert_eq!(balances(&again, ME), balances(&reg, ME));
    }

    /// A completion credits the mission's `fixed_lp` to the agent's
    /// corporation and pushes `OnLPChange`; any other change credits nothing.
    #[test]
    fn a_completion_credits_fixed_lp_to_the_agents_corporation() {
        let fixture = pool::Table::fixture();
        let agent = fixture.agents().next().expect("an agent").clone();
        let mut missions: Vec<_> = fixture.missions().cloned().collect();
        missions[0].fixed_lp = 1234;
        let m = missions[0].clone();
        let t = pool::Table::new(
            fixture.agents().cloned().collect(),
            missions,
            pool::Policy::TRANQUILITY,
        );
        let mut c = Content::fixture();
        c.insert("mission_pool", t);
        let mut reg = Registry::new(SandboxConfig::default()).with_content(Arc::new(c));
        let ev = |change| MissionEvent {
            character_id: ME,
            agent_id: agent.id,
            mission_id: m.id,
            change,
        };
        crate::mission_hooks::fire(
            &[(Change::Completed, on_complete)],
            &mut reg,
            &ev(Change::Accepted),
        );
        assert_eq!(balance(&reg, ME, agent.corp), 0);
        crate::mission_hooks::fire(
            &[(Change::Completed, on_complete)],
            &mut reg,
            &ev(Change::Completed),
        );
        assert_eq!(balance(&reg, ME, agent.corp), m.fixed_lp);
        let p = pushes(&mut reg);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].0, "OnLPChange");
        assert_eq!(p[0].1.len(), 4);
        assert_eq!(p[0].1[1], PyValue::Int(agent.corp));
        assert_eq!(p[0].1[3], PyValue::Int(m.fixed_lp));
    }

    /// `/givelp` credits, debits and refuses to go below zero.
    #[test]
    fn givelp_credits_and_never_overdraws() {
        let mut reg = world();
        let say = |r: Result<PyValue, crate::service::ServiceError>| match r {
            Ok(PyValue::Bytes(b)) => String::from_utf8_lossy(&b).into_owned(),
            other => format!("{other:?}"),
        };
        assert!(say(slash(&mut reg, Some(ME), &[&CORP_A.to_string(), "500"])).contains("0 -> 500"));
        assert_eq!(balance(&reg, ME, CORP_A), 500);
        assert!(
            say(slash(&mut reg, Some(ME), &[&CORP_A.to_string(), "-200"])).contains("500 -> 300")
        );
        assert!(say(slash(&mut reg, Some(ME), &[&CORP_A.to_string(), "-301"])).contains("refused"));
        assert_eq!(balance(&reg, ME, CORP_A), 300);
        assert!(say(slash(&mut reg, Some(ME), &["x"])).contains("usage"));
    }
}
