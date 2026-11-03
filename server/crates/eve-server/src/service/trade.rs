//! **`trademgr`** -- the wire of direct trade (lane GAP-TRADE; the model is
//! [`crate::trade`], the client flow is `pvptradesvc.py` and
//! `pvptradewnd.py`).
//!
//! `trademgr.InitiateTrade` returns a **world-minted** bound object, the
//! `tradeSession`; its methods are the arms under [`crate::trade::BOUND`].
//! The same reference reaches the other pilot inside `OnTradeInitiate`, and
//! `dispatch.rs` resolves it from `Registry::world_objects`.

use super::*;
use crate::pyutil::{keyval, pairs_get};
use crate::trade::{self as tr, Refusal};

pub(crate) const SERVICES: &[&str] = &["trademgr", tr::BOUND];

fn bad_args(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

fn refuse(r: Refusal) -> ServiceError {
    ServiceError::Raise(match r {
        Refusal::Key(key) => eve_proto::user_error(key, vec![]),
        Refusal::NotEnoughMoney { amount, balance } => eve_proto::user_error(
            "NotEnoughMoney",
            crate::pyutil::pypairs(vec![
                ("amount", PyValue::Real(amount)),
                ("balance", PyValue::Real(balance)),
            ]),
        ),
        Refusal::Words(text) => eve_proto::user_error(
            "CustomNotify",
            crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
        ),
    })
}

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    }
}

/// Item ids from a single id or a list of them.
fn ids(v: Option<&PyValue>) -> Vec<i64> {
    match v {
        Some(PyValue::Int(n)) => vec![*n],
        Some(PyValue::List(items) | PyValue::Tuple(items)) => {
            items.iter().filter_map(|i| int(Some(i))).collect()
        }
        _ => Vec::new(),
    }
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(me) = req.actor else {
        return Err(bad_args(req, "no character"));
    };
    match (req.service, req.method) {
        ("trademgr", "InitiateTrade") => match int(req.args.first()) {
            Some(other) => tr::initiate(reg, me, other).map_err(refuse),
            None => Err(bad_args(req, "(charID,)")),
        },
        (tr::BOUND, "List") => {
            let t = tr::live(reg, me).map_err(refuse)?;
            Ok(keyval(vec![
                (
                    "traders",
                    PyValue::List(t.traders.iter().map(|c| PyValue::Int(*c)).collect()),
                ),
                ("tradeContainerID", PyValue::Int(t.container_id)),
                ("items", PyValue::List(tr::item_rows(reg, t.container_id))),
            ]))
        }
        // The trade container as an inventory item, for the controller's
        // `GetItem`. The shape is [P] (EvEJS answers the same fields).
        (tr::BOUND, "GetSelfInvItem") => {
            let t = tr::live(reg, me).map_err(refuse)?;
            let station_type = reg.character(me).map_or(0, |c| c.station_type_id);
            Ok(keyval(vec![
                ("itemID", PyValue::Int(t.container_id)),
                ("typeID", PyValue::Int(station_type)),
                ("ownerID", PyValue::Int(me)),
                ("locationID", PyValue::Int(t.station_id)),
                ("flagID", PyValue::Int(crate::model::FLAG_HANGAR)),
                ("stacksize", PyValue::Int(1)),
                ("quantity", PyValue::Int(-1)),
                ("singleton", PyValue::Int(1)),
            ]))
        }
        // `Add(itemID, sourceLocation, qty=, flag=)`.
        (tr::BOUND, "Add") => {
            let Some(item) = int(req.args.first()) else {
                return Err(bad_args(req, "(itemID, sourceLocation, qty=, flag=)"));
            };
            let qty = pairs_get(req.kwargs, b"qty")
                .or_else(|| req.args.get(2))
                .and_then(|v| int(Some(v)));
            tr::add(reg, me, &[item], qty).map_err(refuse)?;
            Ok(PyValue::None)
        }
        // `MultiAdd(itemIDs, sourceLocation, flag=)`.
        (tr::BOUND, "MultiAdd") => {
            tr::add(reg, me, &ids(req.args.first()), None).map_err(refuse)?;
            Ok(PyValue::None)
        }
        (tr::BOUND, "OfferMoney") => {
            let amount = match req.args.first() {
                Some(PyValue::Real(r)) => *r,
                Some(PyValue::Int(n)) => *n as f64,
                _ => return Err(bad_args(req, "(amount,)")),
            };
            tr::offer_money(reg, me, amount).map_err(refuse)?;
            Ok(PyValue::None)
        }
        (tr::BOUND, "MakeOffer") => {
            let Some(manifest) = req.args.first() else {
                return Err(bad_args(req, "(manifest,)"));
            };
            tr::make_offer(reg, me, &tr::parse_manifest(manifest)).map_err(refuse)?;
            Ok(PyValue::None)
        }
        (tr::BOUND, "Abort") => {
            tr::abort(reg, me);
            Ok(PyValue::None)
        }
        _ => Err(reg.no_method(req)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{Item, Slot};
    use crate::model::{Character, FLAG_HANGAR};
    use crate::pyutil::{keyval, pypairs};
    use crate::session_table::SessionEntry;

    // Invented ids throughout.
    const ALICE: i64 = 90_000_901;
    const BOB: i64 = 90_000_902;
    const CAROL: i64 = 90_000_903;
    const ALICE_SHIP: i64 = 100_000_901;
    const BOB_SHIP: i64 = 100_000_902;
    const CAROL_SHIP: i64 = 100_000_903;
    const STATION: i64 = 60_000_901;
    const OTHER_STATION: i64 = 60_000_902;
    const ITEM: i64 = 100_000_911;
    const STACK: i64 = 100_000_912;
    const BOBS_ITEM: i64 = 100_000_913;
    const MODULE: i64 = 990_901_001;
    const ORE: i64 = 990_901_002;

    fn pilot(id: i64, name: &str, ship: i64, station: i64) -> Character {
        Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            station_id: Some(station),
            balance: 1_000.0,
            ..Character::default()
        }
    }

    fn item(item_id: i64, type_id: i64, owner: i64, quantity: i64, singleton: bool) -> Item {
        Item {
            item_id,
            type_id,
            owner_id: owner,
            location_id: STATION,
            flag_id: FLAG_HANGAR,
            quantity,
            group_id: 1,
            category_id: 1,
            singleton,
        }
    }

    /// Alice, Bob (both docked at STATION, online) and Carol (docked
    /// elsewhere, online). Alice holds a module and 50 ore, Bob a module.
    fn world() -> Registry {
        let mut reg = crate::testkit::registry_with(|c| {
            c.characters = vec![
                pilot(ALICE, "Alice", ALICE_SHIP, STATION),
                pilot(BOB, "Bob", BOB_SHIP, STATION),
                pilot(CAROL, "Carol", CAROL_SHIP, OTHER_STATION),
            ];
        });
        let mut items = reg.store().items().to_vec();
        items.push(item(ITEM, MODULE, ALICE, 1, true));
        items.push(item(STACK, ORE, ALICE, 50, false));
        items.push(item(BOBS_ITEM, MODULE, BOB, 1, true));
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_000_950,
            ..crate::store::Snapshot::default()
        });
        for (i, who) in [ALICE, BOB, CAROL].into_iter().enumerate() {
            reg.session_opened(SessionEntry {
                client_id: i as i64 + 1,
                session_id: i as i64 + 1,
                user_id: who,
                character_id: Some(who),
            });
        }
        let _ = reg.take_pending();
        reg
    }

    fn call(
        reg: &mut Registry,
        actor: i64,
        service: &str,
        method: &str,
        args: &[PyValue],
        kwargs: &[(PyValue, PyValue)],
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service,
            method,
            args,
            kwargs,
            bind_params: None,
            actor: Some(actor),
        })
    }

    fn sess(
        reg: &mut Registry,
        actor: i64,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        call(reg, actor, tr::BOUND, method, args, &[])
    }

    fn key(e: ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            panic!("not a raise");
        };
        let PyValue::Tuple(h) = *header else { panic!() };
        let PyValue::Tuple(a) = &h[1] else { panic!() };
        match &a[0] {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            other => panic!("{other:?}"),
        }
    }

    fn start(reg: &mut Registry) -> i64 {
        call(
            reg,
            ALICE,
            "trademgr",
            "InitiateTrade",
            &[PyValue::Int(BOB)],
            &[],
        )
        .expect("initiate");
        reg.trades.of(ALICE).expect("open").container_id
    }

    fn pushes(reg: &mut Registry) -> Vec<(&'static str, Vec<i64>, Vec<PyValue>)> {
        reg.take_pending()
            .into_iter()
            .map(|(_, p)| {
                let (n, a) = p.to_push();
                (n, p.audience().unwrap_or_default(), a)
            })
            .collect()
    }

    fn manifest(container: i64, money: [f64; 2], ids: &[i64]) -> PyValue {
        keyval(vec![
            ("tradeContainerID", PyValue::Int(container)),
            (
                "money",
                PyValue::List(money.iter().map(|m| PyValue::Real(*m)).collect()),
            ),
            (
                "tradeItems",
                PyValue::List(
                    ids.iter()
                        .map(|i| keyval(vec![("itemID", PyValue::Int(*i))]))
                        .collect(),
                ),
            ),
        ])
    }

    fn at(reg: &Registry, id: i64) -> Slot {
        reg.store().get(id).expect("item").slot()
    }

    #[test]
    fn initiating_returns_a_world_reference_and_tells_the_other_pilot() {
        let mut reg = world();
        let r = call(
            &mut reg,
            ALICE,
            "trademgr",
            "InitiateTrade",
            &[PyValue::Int(BOB)],
            &[],
        )
        .expect("initiate");
        assert!(matches!(r, PyValue::Callback(_)), "a reference");
        let t = reg.trades.of(BOB).expect("both are in it");
        assert_eq!(t.traders, [ALICE, BOB]);
        assert!(
            reg.world_object(&t.object_id).is_some(),
            "minted by the world"
        );
        let sent = pushes(&mut reg);
        let init = sent
            .iter()
            .find(|p| p.0 == "OnTradeInitiate")
            .expect("pushed");
        assert_eq!(init.1, vec![BOB]);
        assert_eq!(init.2[0], PyValue::Int(ALICE));
        assert_eq!(init.2[1], r, "the same reference");
    }

    #[test]
    fn pilots_not_docked_together_are_refused() {
        let mut reg = world();
        let e = call(
            &mut reg,
            ALICE,
            "trademgr",
            "InitiateTrade",
            &[PyValue::Int(CAROL)],
            &[],
        )
        .unwrap_err();
        assert_eq!(key(e), "CantTradeDifferentLocation");
        let e = call(
            &mut reg,
            ALICE,
            "trademgr",
            "InitiateTrade",
            &[PyValue::Int(ALICE)],
            &[],
        )
        .unwrap_err();
        assert_eq!(key(e), "CustomNotify");
        start(&mut reg);
        let e = call(
            &mut reg,
            BOB,
            "trademgr",
            "InitiateTrade",
            &[PyValue::Int(ALICE)],
            &[],
        )
        .unwrap_err();
        assert_eq!(key(e), "CustomNotify", "one trade each");
    }

    #[test]
    fn list_and_the_container_item_answer_both_pilots() {
        let mut reg = world();
        let c = start(&mut reg);
        for who in [ALICE, BOB] {
            let l = sess(&mut reg, who, "List", &[]).expect("list");
            let PyValue::Instance { state, .. } = l else {
                panic!()
            };
            let PyValue::Dict(d) = *state else { panic!() };
            assert_eq!(pairs_get(&d, b"tradeContainerID"), Some(&PyValue::Int(c)));
            assert_eq!(
                pairs_get(&d, b"traders"),
                Some(&PyValue::List(vec![PyValue::Int(ALICE), PyValue::Int(BOB)]))
            );
        }
        assert!(sess(&mut reg, BOB, "GetSelfInvItem", &[]).is_ok());
    }

    #[test]
    fn adding_stages_an_item_and_tells_the_other_pilot() {
        let mut reg = world();
        let c = start(&mut reg);
        let _ = pushes(&mut reg);
        sess(
            &mut reg,
            ALICE,
            "Add",
            &[PyValue::Int(ITEM), PyValue::Int(STATION)],
        )
        .expect("add");
        assert_eq!(at(&reg, ITEM), Slot::new(ALICE, c, FLAG_HANGAR));
        let row = reg
            .deltas()
            .of(tr::KIND)
            .get(&vec![ITEM.to_string()])
            .cloned();
        assert_eq!(
            row,
            Some(vec![
                STATION.to_string(),
                FLAG_HANGAR.to_string(),
                ALICE.to_string(),
                c.to_string()
            ])
        );
        let sent = pushes(&mut reg);
        assert!(
            sent.iter()
                .any(|p| p.0 == "OnItemChange" && p.1 == vec![BOB]),
            "Bob is told of the row: {sent:?}"
        );
        assert!(sent.iter().any(|p| p.0 == "OnTradeOffer"));
        assert!(sent.iter().any(|p| p.0 == "OnPrimingNeededForTradeItems"));
        // Bob sees it in List().
        let PyValue::Instance { state, .. } = sess(&mut reg, BOB, "List", &[]).unwrap() else {
            panic!()
        };
        let PyValue::Dict(d) = *state else { panic!() };
        let Some(PyValue::List(rows)) = pairs_get(&d, b"items") else {
            panic!()
        };
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn what_may_not_be_added_is_refused() {
        let mut reg = world();
        start(&mut reg);
        let add = |reg: &mut Registry, who, id| {
            sess(reg, who, "Add", &[PyValue::Int(id), PyValue::Int(STATION)])
                .map_err(key)
                .err()
        };
        assert_eq!(
            add(&mut reg, ALICE, ALICE_SHIP).as_deref(),
            Some("ConCannotTradeCurrentShip")
        );
        assert_eq!(
            add(&mut reg, ALICE, BOBS_ITEM).as_deref(),
            Some("CantTradeFromCorpHangar")
        );
        assert_eq!(
            add(&mut reg, ALICE, 100_000_999).as_deref(),
            Some("CantTradeDifferentLocation")
        );
        // Elsewhere than the station hangar.
        let mut items = reg.store().items().to_vec();
        items
            .iter_mut()
            .find(|i| i.item_id == ITEM)
            .unwrap()
            .flag_id = 5;
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_000_950,
            ..crate::store::Snapshot::default()
        });
        assert_eq!(
            add(&mut reg, ALICE, ITEM).as_deref(),
            Some("CantTradeDifferentLocation")
        );
    }

    #[test]
    fn a_partial_quantity_splits_the_stack() {
        let mut reg = world();
        let c = start(&mut reg);
        let kw = pypairs(vec![("qty", PyValue::Int(20))]);
        call(
            &mut reg,
            ALICE,
            tr::BOUND,
            "Add",
            &[PyValue::Int(STACK), PyValue::Int(STATION)],
            &kw,
        )
        .expect("add");
        assert_eq!(reg.store().get(STACK).unwrap().quantity, 30);
        let staged: Vec<_> = reg.store().contents_of(c).collect();
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].quantity, 20);
        assert_eq!(staged[0].owner_id, ALICE);
        let id = staged[0].item_id;
        assert!(
            reg.deltas()
                .of(tr::KIND)
                .contains_key(&vec![id.to_string()])
        );
        // Cancelling puts the 20 back as a stack of their own, at the source.
        sess(&mut reg, BOB, "Abort", &[]).expect("abort");
        assert_eq!(at(&reg, id), Slot::new(ALICE, STATION, FLAG_HANGAR));
        assert!(reg.deltas().of(tr::KIND).is_empty());
    }

    #[test]
    fn money_above_the_balance_is_refused_and_an_offer_is_broadcast() {
        let mut reg = world();
        let c = start(&mut reg);
        let e = sess(&mut reg, ALICE, "OfferMoney", &[PyValue::Real(5_000.0)]).unwrap_err();
        assert_eq!(key(e), "NotEnoughMoney");
        let e = sess(&mut reg, ALICE, "OfferMoney", &[PyValue::Real(-1.0)]).unwrap_err();
        assert_eq!(key(e), "NotEnoughMoney");
        let _ = pushes(&mut reg);
        sess(&mut reg, ALICE, "OfferMoney", &[PyValue::Real(500.0)]).expect("offer");
        let sent = pushes(&mut reg);
        let m = sent
            .iter()
            .find(|p| p.0 == "OnTradeMoneyOffer")
            .expect("pushed");
        assert_eq!(m.1, vec![ALICE, BOB]);
        assert_eq!(m.2[0], PyValue::Int(c));
        assert_eq!(
            m.2[1],
            PyValue::List(vec![PyValue::Real(500.0), PyValue::Real(0.0)])
        );
    }

    /// Both sides offer, both accept: the swap happens, once.
    #[test]
    fn a_trade_completes_with_items_and_isk_both_ways() {
        let mut reg = world();
        let c = start(&mut reg);
        sess(
            &mut reg,
            ALICE,
            "Add",
            &[PyValue::Int(ITEM), PyValue::Int(STATION)],
        )
        .unwrap();
        sess(
            &mut reg,
            BOB,
            "Add",
            &[PyValue::Int(BOBS_ITEM), PyValue::Int(STATION)],
        )
        .unwrap();
        sess(&mut reg, ALICE, "OfferMoney", &[PyValue::Real(500.0)]).unwrap();
        let _ = pushes(&mut reg);
        let before = reg.journal.entries().len();
        let m = manifest(c, [500.0, 0.0], &[ITEM, BOBS_ITEM]);
        sess(&mut reg, ALICE, "MakeOffer", std::slice::from_ref(&m)).unwrap();
        assert!(reg.trades.of(ALICE).is_some(), "one accept is not a trade");
        sess(&mut reg, BOB, "MakeOffer", std::slice::from_ref(&m)).unwrap();
        assert!(reg.trades.is_empty());
        assert_eq!(at(&reg, ITEM), Slot::new(BOB, STATION, FLAG_HANGAR));
        assert_eq!(at(&reg, BOBS_ITEM), Slot::new(ALICE, STATION, FLAG_HANGAR));
        assert_eq!(reg.character(ALICE).unwrap().balance, 500.0);
        assert_eq!(reg.character(BOB).unwrap().balance, 1_500.0);
        assert!(reg.deltas().of(tr::KIND).is_empty(), "no staging left");
        let rows: Vec<_> = reg.journal.entries()[before..]
            .iter()
            .filter(|e| e.entry_type_id == tr::REF_PLAYER_TRADING)
            .collect();
        assert_eq!(rows.len(), 2, "one row per pilot");
        // Each row is in its own pilot's journal: the payee (Bob) sees the
        // credit, the payer (Alice) sees only her debit.
        let now = crate::pyutil::filetime_now();
        let bobs = reg.journal.select(now, None, None, BOB);
        assert_eq!(bobs.len(), 1, "the payee sees one row");
        assert!(bobs[0].amount > 0.0 && bobs[0].wallet() == BOB);
        let alices = reg.journal.select(now, None, None, ALICE);
        assert_eq!(alices.len(), 1, "the payer sees only her debit");
        assert!(alices[0].amount < 0.0 && alices[0].wallet() == ALICE);
        assert!((rows[0].amount + rows[1].amount).abs() < 1e-9, "a transfer");
        assert_eq!(
            crate::econ_ledger::classify(rows[0]),
            crate::econ_ledger::Class::Moved
        );
        let sent = pushes(&mut reg);
        let done = sent
            .iter()
            .find(|p| p.0 == "OnTradeComplete")
            .expect("completed");
        assert_eq!(done.1, vec![ALICE, BOB]);
        assert!(sent.iter().any(|p| p.0 == "OnTradeOffer"
            && p.2[1] == PyValue::List(vec![PyValue::Int(1), PyValue::Int(1)])));
        // The closing windows call Abort on a finished trade: not an error.
        assert!(sess(&mut reg, ALICE, "Abort", &[]).is_ok());
    }

    #[test]
    fn any_change_after_an_accept_resets_both_accepts() {
        let mut reg = world();
        let c = start(&mut reg);
        sess(
            &mut reg,
            ALICE,
            "Add",
            &[PyValue::Int(ITEM), PyValue::Int(STATION)],
        )
        .unwrap();
        let m = manifest(c, [0.0, 0.0], &[ITEM]);
        sess(&mut reg, ALICE, "MakeOffer", std::slice::from_ref(&m)).unwrap();
        assert_eq!(reg.trades.of(ALICE).unwrap().accepted, [true, false]);
        // Bob adds something: Alice's accept is gone.
        sess(
            &mut reg,
            BOB,
            "Add",
            &[PyValue::Int(BOBS_ITEM), PyValue::Int(STATION)],
        )
        .unwrap();
        assert_eq!(reg.trades.of(ALICE).unwrap().accepted, [false, false]);
        // And so does an ISK offer.
        let m = manifest(c, [0.0, 0.0], &[ITEM, BOBS_ITEM]);
        sess(&mut reg, BOB, "MakeOffer", std::slice::from_ref(&m)).unwrap();
        assert_eq!(reg.trades.of(ALICE).unwrap().accepted, [false, true]);
        sess(&mut reg, ALICE, "OfferMoney", &[PyValue::Real(10.0)]).unwrap();
        assert_eq!(reg.trades.of(ALICE).unwrap().accepted, [false, false]);
    }

    #[test]
    fn a_stale_manifest_resets_and_tells_the_caller() {
        let mut reg = world();
        let c = start(&mut reg);
        sess(
            &mut reg,
            ALICE,
            "Add",
            &[PyValue::Int(ITEM), PyValue::Int(STATION)],
        )
        .unwrap();
        let _ = pushes(&mut reg);
        // Bob's window has not seen the item yet.
        let stale = manifest(c, [0.0, 0.0], &[BOBS_ITEM]);
        sess(&mut reg, BOB, "MakeOffer", &[stale]).unwrap();
        assert_eq!(reg.trades.of(BOB).unwrap().accepted, [false, false]);
        let sent = pushes(&mut reg);
        let r = sent
            .iter()
            .find(|p| p.0 == "OnTradeOfferReset")
            .expect("reset");
        assert_eq!(r.1, vec![BOB]);
        assert_eq!(r.2, vec![PyValue::Int(c), PyValue::Bool(true)]);
        // Wrong money is stale too.
        let stale = manifest(c, [9.0, 0.0], &[ITEM]);
        sess(&mut reg, BOB, "MakeOffer", &[stale]).unwrap();
        assert_eq!(reg.trades.of(BOB).unwrap().accepted, [false, false]);
    }

    #[test]
    fn abort_returns_everything_and_cancels_both_windows() {
        let mut reg = world();
        let c = start(&mut reg);
        sess(
            &mut reg,
            ALICE,
            "Add",
            &[PyValue::Int(ITEM), PyValue::Int(STATION)],
        )
        .unwrap();
        sess(
            &mut reg,
            BOB,
            "Add",
            &[PyValue::Int(BOBS_ITEM), PyValue::Int(STATION)],
        )
        .unwrap();
        sess(&mut reg, ALICE, "OfferMoney", &[PyValue::Real(100.0)]).unwrap();
        let _ = pushes(&mut reg);
        sess(&mut reg, BOB, "Abort", &[]).unwrap();
        assert!(reg.trades.is_empty());
        assert_eq!(at(&reg, ITEM), Slot::new(ALICE, STATION, FLAG_HANGAR));
        assert_eq!(at(&reg, BOBS_ITEM), Slot::new(BOB, STATION, FLAG_HANGAR));
        assert!(reg.deltas().of(tr::KIND).is_empty());
        assert_eq!(
            reg.character(ALICE).unwrap().balance,
            1_000.0,
            "ISK never left"
        );
        let sent = pushes(&mut reg);
        let cancel = sent
            .iter()
            .find(|p| p.0 == "OnTradeCancel")
            .expect("cancel");
        assert_eq!(cancel.1, vec![ALICE, BOB]);
        assert_eq!(cancel.2, vec![PyValue::Int(c)]);
        // A new trade can start.
        start(&mut reg);
    }

    #[test]
    fn undock_and_logoff_end_the_trade() {
        let mut reg = world();
        start(&mut reg);
        sess(
            &mut reg,
            ALICE,
            "Add",
            &[PyValue::Int(ITEM), PyValue::Int(STATION)],
        )
        .unwrap();
        crate::trade::on_leave(&mut reg, Some(BOB));
        assert!(reg.trades.is_empty());
        assert_eq!(at(&reg, ITEM), Slot::new(ALICE, STATION, FLAG_HANGAR));

        start(&mut reg);
        sess(
            &mut reg,
            BOB,
            "Add",
            &[PyValue::Int(BOBS_ITEM), PyValue::Int(STATION)],
        )
        .unwrap();
        reg.session_closed(2); // Bob's connection
        assert!(reg.trades.is_empty(), "the logoff hook cancelled it");
        assert_eq!(at(&reg, BOBS_ITEM), Slot::new(BOB, STATION, FLAG_HANGAR));
        assert!(reg.scheduled_count() > 0, "the cancel went on the clock");
    }

    #[test]
    fn a_pilot_who_left_the_station_cancels_on_the_next_call() {
        let mut reg = world();
        start(&mut reg);
        sess(
            &mut reg,
            ALICE,
            "Add",
            &[PyValue::Int(ITEM), PyValue::Int(STATION)],
        )
        .unwrap();
        reg.character_mut(BOB).unwrap().station_id = Some(OTHER_STATION);
        let e = sess(&mut reg, ALICE, "OfferMoney", &[PyValue::Real(1.0)]).unwrap_err();
        assert_eq!(key(e), "CantTradeDifferentLocation");
        assert!(reg.trades.is_empty());
        assert_eq!(at(&reg, ITEM), Slot::new(ALICE, STATION, FLAG_HANGAR));
    }

    /// **Atomicity.** The item plan fails at its last edit (a poisoned edit
    /// the test slips in), so nothing in it moves, and no ISK is taken.
    #[test]
    fn a_failure_mid_swap_moves_nothing() {
        let mut reg = world();
        let c = start(&mut reg);
        sess(
            &mut reg,
            ALICE,
            "Add",
            &[PyValue::Int(ITEM), PyValue::Int(STATION)],
        )
        .unwrap();
        sess(
            &mut reg,
            BOB,
            "Add",
            &[PyValue::Int(BOBS_ITEM), PyValue::Int(STATION)],
        )
        .unwrap();
        sess(&mut reg, ALICE, "OfferMoney", &[PyValue::Real(500.0)]).unwrap();
        let m = manifest(c, [500.0, 0.0], &[ITEM, BOBS_ITEM]);
        let journal = reg.journal.entries().len();
        let _ = pushes(&mut reg);
        tr::POISON.with(|p| p.set(true));
        sess(&mut reg, ALICE, "MakeOffer", std::slice::from_ref(&m)).unwrap();
        sess(&mut reg, BOB, "MakeOffer", std::slice::from_ref(&m)).unwrap();
        tr::POISON.with(|p| p.set(false));
        assert_eq!(at(&reg, ITEM), Slot::new(ALICE, c, FLAG_HANGAR));
        assert_eq!(at(&reg, BOBS_ITEM), Slot::new(BOB, c, FLAG_HANGAR));
        assert_eq!(reg.character(ALICE).unwrap().balance, 1_000.0);
        assert_eq!(reg.character(BOB).unwrap().balance, 1_000.0);
        assert_eq!(reg.journal.entries().len(), journal, "no ISK row");
        let t = reg.trades.of(ALICE).expect("the trade stays open");
        assert_eq!(t.accepted, [false, false]);
        assert_eq!(reg.deltas().of(tr::KIND).len(), 2, "still staged");
        let sent = pushes(&mut reg);
        let r = sent
            .iter()
            .find(|p| p.0 == "OnTradeOfferReset")
            .expect("told");
        assert_eq!(
            r.2[1],
            PyValue::Bool(false),
            "ItemExchangeFailed, not a manifest error"
        );
        assert!(!sent.iter().any(|p| p.0 == "OnTradeComplete"));
        // The same trade then completes once the fault is gone.
        sess(&mut reg, ALICE, "MakeOffer", std::slice::from_ref(&m)).unwrap();
        sess(&mut reg, BOB, "MakeOffer", std::slice::from_ref(&m)).unwrap();
        assert!(reg.trades.is_empty());
        assert_eq!(at(&reg, ITEM), Slot::new(BOB, STATION, FLAG_HANGAR));
    }

    /// An accept that cannot be paid for moves neither items nor ISK.
    #[test]
    fn a_balance_that_fell_short_moves_nothing() {
        let mut reg = world();
        let c = start(&mut reg);
        sess(
            &mut reg,
            ALICE,
            "Add",
            &[PyValue::Int(ITEM), PyValue::Int(STATION)],
        )
        .unwrap();
        sess(&mut reg, ALICE, "OfferMoney", &[PyValue::Real(500.0)]).unwrap();
        reg.character_mut(ALICE).unwrap().balance = 100.0; // spent elsewhere
        let m = manifest(c, [500.0, 0.0], &[ITEM]);
        sess(&mut reg, ALICE, "MakeOffer", std::slice::from_ref(&m)).unwrap();
        sess(&mut reg, BOB, "MakeOffer", std::slice::from_ref(&m)).unwrap();
        assert_eq!(at(&reg, ITEM), Slot::new(ALICE, c, FLAG_HANGAR));
        assert_eq!(reg.character(ALICE).unwrap().balance, 100.0);
        assert_eq!(reg.character(BOB).unwrap().balance, 1_000.0);
        assert!(reg.trades.of(ALICE).is_some());
    }

    /// Item rows in `tradeItems` (the window sends its row objects) are read
    /// by `itemID`.
    #[test]
    fn a_manifest_of_item_rows_is_read() {
        let mut reg = world();
        let c = start(&mut reg);
        sess(
            &mut reg,
            ALICE,
            "Add",
            &[PyValue::Int(ITEM), PyValue::Int(STATION)],
        )
        .unwrap();
        let row = reg.item_row_value(&reg.store().get(ITEM).cloned().unwrap());
        let m = keyval(vec![
            ("tradeContainerID", PyValue::Int(c)),
            (
                "money",
                PyValue::List(vec![PyValue::Int(0), PyValue::Int(0)]),
            ),
            ("tradeItems", PyValue::List(vec![row])),
        ]);
        let parsed = tr::parse_manifest(&m);
        assert_eq!(parsed.container_id, Some(c));
        assert_eq!(parsed.item_ids, [ITEM].into_iter().collect());
        sess(&mut reg, BOB, "MakeOffer", &[m]).unwrap();
        assert_eq!(reg.trades.of(BOB).unwrap().accepted, [false, true]);
    }

    /// **A restart leaves no stuck items.** The world is saved with a trade
    /// open, then loaded into a fresh registry: every staged item is back at
    /// its source, owned by whoever offered it, and the rows are gone.
    #[test]
    fn a_reload_sends_staged_items_home() {
        let mut reg = world();
        let c = start(&mut reg);
        let kw = pypairs(vec![("qty", PyValue::Int(20))]);
        sess(
            &mut reg,
            ALICE,
            "Add",
            &[PyValue::Int(ITEM), PyValue::Int(STATION)],
        )
        .unwrap();
        call(
            &mut reg,
            ALICE,
            tr::BOUND,
            "Add",
            &[PyValue::Int(STACK), PyValue::Int(STATION)],
            &kw,
        )
        .unwrap();
        sess(
            &mut reg,
            BOB,
            "Add",
            &[PyValue::Int(BOBS_ITEM), PyValue::Int(STATION)],
        )
        .unwrap();
        assert_eq!(reg.store().contents_of(c).count(), 3);
        let snap = reg.to_snapshot();

        let mut fresh = crate::testkit::registry_with(|cfg| {
            cfg.characters = vec![
                pilot(ALICE, "Alice", ALICE_SHIP, STATION),
                pilot(BOB, "Bob", BOB_SHIP, STATION),
            ];
        });
        fresh.apply_snapshot(snap);
        assert!(fresh.trades.is_empty());
        assert_eq!(fresh.store().contents_of(c).count(), 0, "nothing stuck");
        assert!(fresh.deltas().of(tr::KIND).is_empty());
        assert_eq!(at(&fresh, ITEM), Slot::new(ALICE, STATION, FLAG_HANGAR));
        assert_eq!(at(&fresh, BOBS_ITEM), Slot::new(BOB, STATION, FLAG_HANGAR));
        let ore: i64 = fresh
            .store()
            .items()
            .iter()
            .filter(|i| i.type_id == ORE && i.owner_id == ALICE)
            .map(|i| i.quantity)
            .sum();
        assert_eq!(ore, 50, "the split stack is whole again, as two stacks");
        assert!(fresh.take_dirty(), "the load changed what is on disk");
        assert!(fresh.take_pending().is_empty(), "a load tells nobody");
    }
}
