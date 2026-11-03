//! Money: the wallet, loyalty points, contracts and the market.
//!
//! Split out of `service.rs`, which had grown to 7374 lines in one file --
//! and that, rather than anything about the work itself, was what capped
//! parallel implementation at one agent. A lane working on this subsystem
//! now opens one file and cannot collide with a lane working on another.
//!
//! The arms are unchanged by the move: `self` became the `reg` argument and
//! nothing else. Helpers deliberately stayed in the parent for now, because
//! several are shared across domains and moving those is where a split of
//! this kind goes wrong.

use super::*;
use crate::contracts::{self as con, Contract};

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &["account", "LPSvc", "LPStoreMgr", "contractProxy"];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    // The corporation's half of the Wallet window, the give-ISK calls and
    // the division list (CORP-W); the personal arms below answer the rest.
    if req.service == "account"
        && let Some(answer) = super::corp_wallet::account(reg, req)
    {
        return answer;
    }
    match req.service {
        // The wallet. `walletSvc.GetWealth` -> `GetCashBalance(0)`, where
        // the argument is the *corp* flag: 0 personal, 1 corporate (the
        // corporate form takes an `accountKey` and is wrapped in
        // `except UserError`).
        //
        // A **float**. It reaches the character sheet's net-worth
        // progression bar and the wallet window, and the net-worth side
        // is forgiving -- `set_value` is `reg.value = value or
        // reg.INITIAL_VALUE`, so `None` quietly became a zero rather
        // than a crash. Forgiving is not the same as correct: the
        // service's cache guard is `if reg._wealth is None`, so `None`
        // also meant the call was re-issued on every read. The same `is
        // None` tell as `GetMySecurityStatus`.
        //
        // The balance is the character's own, so a GM grant of ISK has a
        // single place to change and one notification to push. That is
        // the next piece, not this one.
        "account" => match req.method {
            // The first argument is the **corp flag**, not an id:
            // `GetCashBalance(0)` is this character's wallet and
            // `GetCashBalance(1, accountKey)` is the corporation's. Only
            // the corp form has a caller that catches `UserError`, so the
            // personal form must never raise -- it runs in `WalletSvc.Run`
            // with no handler, where a raise is a service-startup failure
            // rather than an empty wallet.
            "GetCashBalance" => {
                let corp = matches!(req.args.first(), Some(PyValue::Int(1)));
                Ok(PyValue::Real(if corp {
                    0.0
                } else {
                    req.actor
                        .and_then(|id| reg.character(id))
                        .map(|c| c.balance)
                        .unwrap_or(0.0)
                }))
            }
            // The wallet journal -- there is no `GetJournal`. A
            // **CRowset**, and here that is forced rather than preferred,
            // for a reason that has nothing to do with reading: every row
            // is handed to `utillib.KeyVal(row)`, whose `__init__` accepts
            // a dict, a `blue.DBRow` or another KeyVal and raises
            // `TypeError` on anything else. A `util.Row` out of an
            // old-style Rowset is rejected on the spot.
            //
            // **The four arguments are read now, where they used to be
            // ignored.** `(accountKey, year, month, isCorp)`: an absent or
            // null `(year, month)` is the window's own default, which the
            // real server answers as the last thirty days, and a concrete
            // pair is that calendar month. Answering every row regardless of
            // the range would have been simpler and would have shown a grant
            // made today under whatever month the player picked -- a lie that
            // only surfaces once someone trusts it.
            //
            // Every matching row goes out in one answer. The client pages at
            // 100 locally, so there is nothing for us to page.
            "GetTransactions" => {
                let arg = |i: usize| match req.args.get(i) {
                    Some(PyValue::Int(v)) => Some(*v),
                    _ => None,
                };
                // **No actor, no rows.** Not "every row": the journal is
                // one table for the whole world and this call used to read
                // all of it, which handed each player everyone else's wallet
                // history. An empty journal is the right answer for a caller
                // with no character, and the window renders it.
                let Some(actor) = req.actor else {
                    return Ok(transactions_rowset(&[]));
                };
                let entries =
                    reg.journal()
                        .select(crate::pyutil::filetime_now(), arg(1), arg(2), actor);
                Ok(transactions_rowset(&entries))
            }
            // The reference-type table, and **the one answer here that
            // latches**. `Account.GetStaticData` stores whatever comes
            // back under `reg.data[name]` and never retries, so a wrong
            // first answer survives until a UI refresh or a character
            // change.
            //
            // It cannot be a rowset of either family, and again not
            // because of how it is read: the client does
            // `setattr(rec, 'entryTypeName', translated)` on every row.
            // `Row.__setattr__` writes through `header.index(key)` and
            // raises `AttributeError` for an undeclared column, and
            // `blue.DBRow` is a fixed native schema. `utillib.KeyVal`
            // takes arbitrary attributes, so a list of KeyVals is the
            // shape that survives being written to.
            //
            // Two entries rather than none: these are the types a GM ISK
            // grant needs the moment the journal has rows in it, and
            // because this call latches, "add them later" means "add them
            // and then relog".
            "GetEntryTypes" => Ok(PyValue::List({
                let mut rows = vec![
                    entry_type(REF_GM_CASH_TRANSFER, "GM cash transfer"),
                    entry_type(REF_PLAYER_DONATION, "Player donation"),
                    // The market's four (`service/market.rs`). Without them every
                    // market row is dropped from the Journal tab, silently
                    // (`docs/reference/wallet-ledger.md` §5.3).
                    entry_type(market::REF_MARKET_TRANSACTION, "Market transaction"),
                    entry_type(market::REF_MARKET_ESCROW, "Market escrow"),
                    entry_type(market::REF_BROKER_FEE, "Brokers fee"),
                    // E5 (CP9): the sales tax.
                    entry_type(market::REF_TRANSACTION_TAX, "Transaction tax"),
                    // The contracts' four (`crate::contracts`), for the same
                    // reason: an entry type missing here drops its rows.
                    entry_type(con::REF_PRICE, "Contract price"),
                    entry_type(con::REF_REWARD, "Contract reward"),
                    entry_type(con::REF_REWARD_ADDED, "Contract reward added"),
                    entry_type(con::REF_REWARD_REFUND, "Contract reward refund"),
                    // Couriers' and auctions' six (W9-L6), same reason.
                    entry_type(con::REF_COLLATERAL, "Contract collateral"),
                    entry_type(con::REF_COLLATERAL_REFUND, "Contract collateral refund"),
                    entry_type(con::REF_COLLATERAL_PAYOUT, "Contract collateral payout"),
                    entry_type(con::REF_AUCTION_BID, "Contract auction bid"),
                    entry_type(con::REF_AUCTION_BID_REFUND, "Contract auction bid refund"),
                    entry_type(con::REF_AUCTION_SOLD, "Contract auction sold"),
                    // A new character's starting funds (P-START-2): every
                    // created character's first row.
                    entry_type(
                        crate::start::REF_NEW_CHARACTER_STARTING_FUNDS,
                        "New Character Starting Funds",
                    ),
                ];
                // CORP-W: every other ref the economy ledger knows (the
                // bounties, the agents, the corporation refs, and the 92..94
                // tax rows the client derives), so none is dropped.
                super::corp_wallet::complete_entry_types(&mut rows);
                rows
            })),
            // Iterated with `enumerate`, so never None. Unreachable for
            // an NPC-corp pilot with no roles -- the corp wallet tab is
            // not built at all -- but being right costs nothing.
            "GetWalletDivisionsInfo" => Ok(PyValue::List(vec![])),
            // `None` is the answer and not a placeholder: the caller
            // coerces it to -1, which is its own "unset" value.
            "GetDefaultContactCost" | "SetContactCost" => Ok(PyValue::None),
            _ => Err(reg.no_method(req)),
        },
        // Loyalty points, and **the first answer of ours to cost frames
        // rather than correctness.**
        //
        //     if reg._characterLPBalanceByIssuerCorpID is None:
        //         loyaltyPoints = reg._GetRemoteLPService().GetAllMyCharacterWalletLPBalances()
        //         reg._characterLPBalanceByIssuerCorpID = {row[0]: row[1] for row in loyaltyPoints}
        //
        // Two failures compounding, and together they are a live lock.
        // `None` makes the dict comprehension raise `TypeError`, so the
        // assignment never happens -- and the guard is `is None`, so the
        // cache stays empty and the *next* read issues the call again.
        // Opening the wallet put it in a tight loop that the player could
        // see as dropped frames, which is a new symptom for us: every
        // earlier bad answer was either a traceback or silence.
        //
        // The shape is an **iterable of 2-sequences** `(issuerCorpID,
        // balance)`, indexed positionally. `[]` is right -- a new
        // character has no loyalty points -- and it caches, because `{}`
        // is not `None`.
        //
        // The corporate twin has the same code with its own cache, and it
        // will loop the same way the moment the corp tab is opened.
        //
        // CP37b: the character's rows are `crate::loyalty`'s wallet, as
        // `(corpID, balance)` tuples. Corporation LP wallets are not
        // modelled, so the twin stays `[]`.
        "LPSvc" => match req.method {
            "GetAllMyCharacterWalletLPBalances" => {
                let me = req.actor.unwrap_or(0);
                Ok(PyValue::List(
                    crate::loyalty::balances(reg, me)
                        .into_iter()
                        .map(|(corp, lp)| {
                            PyValue::Tuple(vec![PyValue::Int(corp), PyValue::Int(lp)])
                        })
                        .collect(),
                ))
            }
            // GAPS-2: a corporation's LP wallet, filled by transfers.
            "GetAllMyCorporationWalletLPBalances" => {
                Ok(crate::lp_transfer::corp_balances_value(reg, req.actor))
            }
            _ => match super::gaps2::lp_svc(reg, req) {
                Some(answer) => answer,
                None => Err(reg.no_method(req)),
            },
        },
        // The LP store (CP37b, C4 §5.1 [V]). `GetAvailableOffersFromCorp` is
        // cached per corporation behind an `is None` check
        // (`lpStoreSvc.py:140-144`), so a corporation with no store answers
        // `[]`, never `None`. `TakeOfferForCharacter` is truth-tested
        // (`lpStoreSvc.py:315-324`): truthy shows `LPStoreOfferAccepted`,
        // and every refusal here is a falsy `False` with nothing moved [A]
        // (falsifier: a Tranquility capture raising a named `UserError`).
        // `TakeOfferForCorporation` is deferred with corporation wallets.
        "LPStoreMgr" => match req.method {
            "GetAvailableOffersFromCorp" => {
                let corp = int_arg(req.args.first());
                let store = reg.ext::<crate::loyalty::Store>();
                Ok(PyValue::List(
                    store
                        .offers(corp.unwrap_or(0))
                        .iter()
                        .map(|o| offer_keyval(corp.unwrap_or(0), o))
                        .collect(),
                ))
            }
            "TakeOfferForCharacter" => {
                let (corp, offer, n) = (
                    int_arg(req.args.first()),
                    int_arg(req.args.get(1)),
                    int_arg(req.args.get(2)),
                );
                let ok = match (req.actor, corp, offer, n) {
                    (Some(me), Some(corp), Some(offer), Some(n)) => {
                        take_offer(reg, me, corp, offer, n)
                    }
                    _ => false,
                };
                Ok(PyValue::Bool(ok))
            }
            _ => Err(reg.no_method(req)),
        },
        // Contracts: item exchange between players, `crate::contracts` for
        // the model and [`contract_proxy`] below for the arms.
        "contractProxy" => contract_proxy(reg, req),
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

// ---- the LP store ------------------------------------------------------------

fn int_arg(v: Option<&PyValue>) -> Option<i64> {
    match v {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    }
}

/// One offer as the store reads it: a `KeyVal` with the eight keys of C4
/// §5.1 [V] (`lpStoreSvc.py:526`, `:538`; `lpstoreRequirement.py:45-123`).
/// `reqItems` is a list of `(typeID, qty)` (`:372`); `akCost` is 0, as no
/// offer costs analysis kredits. `requiredStandings` is optional and left
/// out.
fn offer_keyval(corp: i64, o: &crate::loyalty::Offer) -> PyValue {
    crate::pyutil::keyval(vec![
        ("offerID", PyValue::Int(o.offer_id)),
        ("corpID", PyValue::Int(corp)),
        ("typeID", PyValue::Int(o.type_id)),
        ("qty", PyValue::Int(o.qty)),
        ("lpCost", PyValue::Int(o.lp_cost)),
        ("iskCost", PyValue::Int(o.isk_cost)),
        ("akCost", PyValue::Int(0)),
        (
            "reqItems",
            PyValue::List(
                o.req_items
                    .iter()
                    .map(|&(t, q)| PyValue::Tuple(vec![PyValue::Int(t), PyValue::Int(q)]))
                    .collect(),
            ),
        ),
    ])
}

/// Redeem `n` of `corp`'s offer `offer_id` for `me`: take `n ×` the LP, the
/// ISK and the required items, and create `n × qty` of the product in the
/// station hangar. All or nothing; `false` refuses.
///
/// [A], each falsified by a Tranquility store that differs: the character
/// must be docked (any station); required items come from that station's
/// hangar, packaged stacks only, lowest item id first.
fn take_offer(reg: &mut Registry, me: i64, corp: i64, offer_id: i64, n: i64) -> bool {
    use crate::item::{Edit, Slot};
    if !(1..=crate::loyalty::MAX_BULK_OFFERS).contains(&n) {
        return false;
    }
    let Some(o) = reg
        .ext::<crate::loyalty::Store>()
        .offer(corp, offer_id)
        .cloned()
    else {
        return false;
    };
    let Some(ch) = reg.character(me) else {
        return false;
    };
    let balance = ch.balance;
    let Some(station) = reg.docked_station(me) else {
        return false;
    };
    let (Some(lp), Some(isk), Some(qty)) = (
        o.lp_cost.checked_mul(n),
        o.isk_cost.checked_mul(n),
        o.qty.checked_mul(n),
    ) else {
        return false;
    };
    if crate::loyalty::balance(reg, me, corp) < lp || balance < isk as f64 {
        return false;
    }
    let hangar = Slot::new(me, station, crate::model::FLAG_HANGAR);
    let mut edits = Vec::new();
    for &(type_id, each) in &o.req_items {
        let Some(mut need) = each.checked_mul(n) else {
            return false;
        };
        let mut stacks: Vec<_> = reg
            .store()
            .items()
            .iter()
            .filter(|i| {
                i.type_id == type_id
                    && !i.singleton
                    && Slot::new(i.owner_id, i.location_id, i.flag_id) == hangar
            })
            .map(|i| (i.item_id, i.quantity))
            .collect();
        stacks.sort_unstable();
        for (item_id, have) in stacks {
            if need == 0 {
                break;
            }
            if have <= need {
                edits.push(Edit::Destroy { item_id });
                need -= have;
            } else {
                edits.push(Edit::Consume {
                    item_id,
                    quantity: need,
                });
                need = 0;
            }
        }
        if need > 0 {
            return false;
        }
    }
    edits.push(Edit::Create {
        type_id: o.type_id,
        quantity: qty,
        to: hangar,
    });
    if reg.apply_items(&edits).is_err() {
        return false;
    }
    crate::loyalty::adjust(reg, me, corp, -lp, crate::loyalty::CHANGE_STORE);
    if isk > 0 {
        reg.charge(
            me,
            isk as f64,
            crate::loyalty::REF_PAYMENT_TO_LP_STORE,
            corp,
            offer_id,
        );
    }
    true
}

// ---- contracts ---------------------------------------------------------------
//
// `crate::contracts` is the model and has the `[V]` sources; these are the
// arms, here because moving ISK needs the journal and the world's fields.
// Everything below answers the empty world exactly as the old placeholders
// did -- the same lists, dicts and zeros -- so a character with no contracts
// sees no change at all.

/// The `contractProxy` service.
///
/// Fires automatically at login -- `NeocomBlink` is started by
/// `OnNotificationUiReady` and calls `GetLoginInfo`. Every list method here
/// answers an **object**, never a container, because every caller reads it by
/// attribute name.
fn contract_proxy(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let now = crate::pyutil::filetime_now();
    let arg = |i: usize| req.args.get(i);
    let kw = |name: &str| crate::pyutil::pairs_get(req.kwargs, name.as_bytes());
    match req.method {
        // Two attributes, both laundered through `float(x or 0)`
        // (contracts.py `GetMyContractEscrow`), behind a bare
        // `except Exception` whose fallback `playerprogression` commits to a
        // 12-hour disk cache -- so the object must exist; the values are
        // free. `iskEscrow` is the reward ISK this character has in escrow.
        // `itemsEscrow` is the escrowed items' *value*, which needs prices we
        // do not keep: 0.0, **[A]**. Couriers' collateral and auction bids
        // count as well: both are this character's ISK, held.
        "GetMyContractEscrow" => {
            let isk: f64 = req.actor.map_or(0.0, |me| {
                reg.contracts.all().iter().map(|c| held_from(c, me)).sum()
            });
            Ok(keyval(vec![
                ("iskEscrow", PyValue::Real(isk)),
                ("itemsEscrow", PyValue::Real(0.0)),
            ]))
        }
        // The contracts window's start page. Without it the window cannot
        // open at all: `contractsWnd.py(56) ApplyAttributes` -> `SelectTab` ->
        // `Load(key)` -> `contractPanels.py(90) Init` -> `contracts.py(884)
        // CollectMyPageInfo`. See [`page_info`].
        "CollectMyPageInfo" => Ok(page_info(reg, req.actor, now)),
        // The create wizard's counter, and a **live lock** until it was
        // answered: `createContractWnd.GetNumContractsAndLimits` caches the
        // answer behind `is None` and reads it inside a `try` whose
        // `finally: return` swallows the traceback, so a `None` re-issued the
        // call on every item checkbox tick with nothing in the log
        // (`1792433234-0002.evecap`). Four attributes, all **numbers**:
        // `UpdateNumberOfItems` does `maxNumContracts - currentNum` outside
        // the `try`. `myCharTotal` counts toward a contract inside one's own
        // corp and `nonCorpForMyChar` outside it; we have one count, so both
        // carry it, **[A]**. Corporation contracts do not exist here.
        "NumOutstandingContracts" => {
            let n = req.actor.map_or(0, |me| outstanding_issued_by(reg, me));
            Ok(keyval(vec![
                ("myCharTotal", PyValue::Int(n)),
                ("nonCorpForMyChar", PyValue::Int(n)),
                ("myCorpTotal", PyValue::Int(0)),
                ("nonCorpForMyCorp", PyValue::Int(0)),
            ]))
        }
        // The My Contracts tab. **Four positional arguments and two named
        // ones**, `contractPanels.py(551)`:
        //
        //     _contracts = sm.ProxySvc('contractProxy').GetContractListForOwner(
        //             ownerID, filtStatus, contractType, issuedBy,
        //             num=RESULTS_PER_PAGE, startContractID=startContractID)
        //     return _contracts.contracts, _contracts.items     # 555
        //
        // `filtStatus` only reaches us as 0, 1 or 4: RequiresAttention (-1)
        // and BidOnBy (10) branch to the two methods below. `issuedBy` is the
        // Action combo, whose options are `IssuedToBy -> None`,
        // `IssuedBy -> False`, `IssuedTo -> True`
        // (`contractPanels.ConstructFilters`, **[V]**). `None` throws at 555,
        // and raising is worse than answering: `DoFetchContracts` catches
        // `UserError`, shows "no contracts found", then bare-`raise`s.
        "GetContractListForOwner" => {
            let Some(me) = req.actor else {
                return Ok(con::list_page(std::iter::empty()));
            };
            // **[A]** Only one's own list: an owner name typed into the
            // filter that is someone else, or a corporation, answers empty.
            if con::int(arg(0)) != me {
                return Ok(con::list_page(std::iter::empty()));
            }
            let status = con::int(arg(1));
            let kind = con::num(arg(2)).map(|k| k as i64);
            let issued_by = match arg(3) {
                Some(PyValue::Bool(b)) => Some(*b),
                Some(PyValue::Int(n)) => Some(*n != 0),
                _ => None,
            };
            let limit = match con::num(kw("num")) {
                Some(n) if n >= 1.0 => n as usize,
                _ => con::CONTRACTS_PER_PAGE,
            };
            let start = con::num(kw("startContractID")).map(|n| n as i64);
            let w = who(reg, me);
            let rows: Vec<&Contract> = reg
                .contracts
                .newest_first()
                .filter(|c| match issued_by {
                    None => c.issuer_id == me || c.assigned_to(&w) || c.acceptor_id == me,
                    Some(false) => c.issuer_id == me,
                    Some(true) => c.assigned_to(&w) || c.acceptor_id == me,
                })
                .filter(|c| status_matches(c, status, now))
                .filter(|c| kind_matches(c.kind, kind))
                // **[A]** The page boundary is exclusive. The client keeps a
                // page's first and last id (`UpdatePagination`) and passes
                // the previous page's last as the next page's start, so
                // exclusive is right going forward; going back to a page
                // other than the first loses its top row.
                .filter(|c| start.is_none_or(|s| c.contract_id < s))
                .take(limit)
                .collect();
            Ok(con::list_page(rows))
        }
        // "Requires attention": the Status filter's -1 reads `.mySelf` /
        // `.myCorp` off the client's own cache, which `contracts.py(914)`
        // fills from **two** calls, `(False)` then `(True)`. Ours is the
        // issuer's expired, still-outstanding contracts -- the ones whose
        // escrow is waiting for a Delete. **[A]**, consistent with
        // `DeleteContract` skipping its confirmation for exactly those.
        "GetMyExpiredContractList" => {
            let for_corp = con::truthy(arg(0));
            let rows: Vec<&Contract> = match req.actor {
                Some(me) if !for_corp => reg
                    .contracts
                    .newest_first()
                    .filter(|c| needs_attention(c, me, now, crate::game_clock::of(reg)))
                    .collect(),
                _ => Vec::new(),
            };
            Ok(con::list_page(rows))
        }
        // "Bid on by me": `contracts.py(929)` stores our answer as `.list`
        // and `contractPanels.py(549)` reads it to line 555 -- the same
        // envelope as the other lists. Every auction this character has bid
        // on, open or closed, newest first; `GetMyBids(True)` (for the corp)
        // is empty.
        "GetMyBids" => {
            let rows: Vec<&Contract> = match req.actor {
                Some(me) if !con::truthy(arg(0)) => reg
                    .contracts
                    .newest_first()
                    .filter(|c| c.bids.iter().any(|b| b.bidder_id == me))
                    .collect(),
                _ => Vec::new(),
            };
            Ok(con::list_page(rows))
        }
        // An **object with three attributes**: `kv.assignedToMe` iterated
        // reading `.issuerID` and `.contractID`, `kv.needsAttention` handed
        // to `DispatchAttentionNotification`, `len(kv.inProgress)`
        // (contracts.py `NeocomBlink`, **[V]**).
        //
        // `needsAttention` is what the client turns into its own "contracts
        // need attention" notification (type 1007, `DispatchAttentionNotif
        // ication`, `NeocomBlink:268`, **[V]**): the client builds and
        // scatters it itself at login from `len(needsAttention)`, so the
        // server pushes none (it would show twice). Its first row is read
        // both `[1]` (`isForCorp`) and `.contractID`, which a `DBRow` of
        // (contractID, isForCorp) satisfies ([`attention_rows`]). `inProgress`
        // is the couriers this character is carrying: `NeocomBlink` keeps
        // them as the pickup / delivery bookmarks
        // ([`Contract::in_progress_row`]).
        "GetLoginInfo" => {
            let assigned: Vec<PyValue> = req.actor.map_or_else(Vec::new, |me| {
                let w = who(reg, me);
                reg.contracts
                    .newest_first()
                    .filter(|c| c.assigned_to(&w) && c.is_open(now))
                    .map(Contract::row)
                    .collect()
            });
            let carrying: Vec<PyValue> = req.actor.map_or_else(Vec::new, |me| {
                reg.contracts
                    .newest_first()
                    .filter(|c| {
                        c.kind == con::TYPE_COURIER
                            && c.status == con::STATUS_IN_PROGRESS
                            && c.acceptor_id == me
                    })
                    .map(|c| c.in_progress_row(crate::game_clock::of(reg)))
                    .collect()
            });
            Ok(keyval(vec![
                ("assignedToMe", PyValue::List(assigned)),
                (
                    "needsAttention",
                    PyValue::List(attention_rows(reg, req.actor, now)),
                ),
                ("inProgress", PyValue::List(carrying)),
            ]))
        }
        // One contract, as `.contract`, `.items`, `.bids`. An unknown or
        // hidden one raises `ConContractNotFound`, the client's own key for
        // it (contracts.py `AcceptContract`, **[V]**), rather than answering
        // `None`: `AcceptContract` reads `contract.contract` *before* its own
        // `if not contract` test.
        "GetContract" => {
            let id = con::int(arg(0));
            match (req.actor, reg.contracts.get(id)) {
                (Some(me), Some(c)) if c.visible_to(&who(reg, me)) => Ok(c.details()),
                _ => Err(con_error("ConContractNotFound", vec![])),
            }
        }
        "SearchContracts" => Ok(search(reg, req, now)),
        // The wizard's item picker, `createContractWnd.py(2058)` in
        // `WriteSelectItems`: iterated, reading `.itemID`, `.typeID`,
        // `.groupID`, `.categoryID`, `.singleton`, `.stacksize`, `.flagID`
        // (**[V]**) -- an inventory row
        // has every one, so the hangar's own rows are sent. The client skips
        // the active ship itself. Corporation hangars are not modelled: empty.
        "GetItemsInDockableLocation" => {
            let station = con::int(arg(0));
            let rows = match req.actor {
                Some(me) if !con::truthy(arg(1)) => reg
                    .store()
                    .at(crate::item::Slot::new(
                        me,
                        station,
                        crate::model::FLAG_HANGAR,
                    ))
                    .map(|i| reg.item_row_value(i))
                    .collect(),
                _ => Vec::new(),
            };
            Ok(PyValue::List(rows))
        }
        // A ship's or container's contents, for the wizard's "this is not
        // empty" warning (`createContractWnd.py(1712)` in `FinishStep1`,
        // read `.typeID`, `.itemID`, `.stacksize`, and `or []`).
        "GetItemsInContainer" => {
            let container = con::int(arg(1));
            let rows = match req.actor {
                Some(me) if owns(reg, me, container) => contents(reg.store(), container)
                    .iter()
                    .filter(|i| i.location_id == container)
                    .map(|i| reg.item_row_value(i))
                    .collect(),
                _ => Vec::new(),
            };
            Ok(PyValue::List(rows))
        }
        // `{containerID: count}`, read with `.itervalues()` and `len()`
        // (`createContractWnd.py(1677)`, **[V]**).
        "GetNumItemsInContainers" => {
            let ids: Vec<i64> = match arg(1) {
                Some(PyValue::List(v)) | Some(PyValue::Tuple(v)) => {
                    v.iter().map(|x| con::int(Some(x))).collect()
                }
                _ => Vec::new(),
            };
            let entries = match req.actor {
                Some(me) => ids
                    .into_iter()
                    .filter(|id| owns(reg, me, *id))
                    .map(|id| {
                        let n = contents(reg.store(), id).len() as i64;
                        // `(value, key)`.
                        (PyValue::Int(n), PyValue::Int(id))
                    })
                    .collect(),
                None => Vec::new(),
            };
            Ok(PyValue::Dict(entries))
        }
        "CreateContract" => create_contract(reg, req, now),
        "AcceptContract" => accept_contract(reg, req, now),
        // `if self.GetContractProxySvc().DeleteContract(contractID):` then
        // `OnDeleteContracts` (contracts.py `DeleteContract`, **[V]**): a
        // truthy answer is what closes the details window.
        "DeleteContract" => {
            let me = req
                .actor
                .ok_or_else(|| con_error("ConContractNotFound", vec![]))?;
            delete_contract(reg, me, con::int(arg(0)), now).map(|()| PyValue::Bool(true))
        }
        // `deleted, failed = ...DeleteMultipleContracts(contractIDs)`, both
        // `len()`ed and `deleted` scattered as a list (contracts.py
        // `DeleteMultipleContracts`, **[V]**).
        "DeleteMultipleContracts" => {
            let me = req
                .actor
                .ok_or_else(|| con_error("ConContractNotFound", vec![]))?;
            let ids: Vec<i64> = match arg(0) {
                Some(PyValue::List(v)) | Some(PyValue::Tuple(v)) => {
                    v.iter().map(|x| con::int(Some(x))).collect()
                }
                _ => Vec::new(),
            };
            let (mut deleted, mut failed) = (Vec::new(), Vec::new());
            for id in ids {
                match delete_contract(reg, me, id, now) {
                    Ok(()) => deleted.push(PyValue::Int(id)),
                    Err(_) => failed.push(PyValue::Int(id)),
                }
            }
            Ok(PyValue::Tuple(vec![
                PyValue::List(deleted),
                PyValue::List(failed),
            ]))
        }
        // `CompleteContract(contractID, status)`: one method, three verbs.
        // `conStatusFinished` from the Complete button and the package's
        // Deliver entry, `conStatusFailed` from Fail, `conStatusRejected`
        // from Reject (contracts.py `CompleteContract:594`,
        // `FailContract:628`, `RejectContract:659`, **[V]**). The details
        // window does `if CompleteContract(id): self.Init()`, so the answer
        // is truthy.
        "CompleteContract" => {
            let me = req
                .actor
                .ok_or_else(|| con_error("ConContractNotFound", vec![]))?;
            let id = con::int(arg(0));
            if !reg
                .contracts
                .get(id)
                .is_some_and(|c| c.visible_to(&who(reg, me)))
            {
                return Err(con_error("ConContractNotFound", vec![]));
            }
            match con::int(arg(1)) {
                con::STATUS_FINISHED => deliver_courier(reg, me, id, now),
                con::STATUS_FAILED => fail_courier(reg, me, id, now),
                con::STATUS_REJECTED => reject_contract(reg, me, id, now),
                _ => Err(con_error("ConContractInvalid", vec![])),
            }
            .map(|()| PyValue::Bool(true))
        }
        // `PlaceBid(contractID, bid, forCorp)`, and `return not not retval`
        // (contracts.py `DoPlaceBid:803`, `PlaceBid:748`, **[V]**).
        "PlaceBid" => {
            let me = req
                .actor
                .ok_or_else(|| con_error("ConContractNotFound", vec![]))?;
            let bid = con::num(arg(1)).unwrap_or(0.0);
            place_bid(reg, me, con::int(arg(0)), bid, con::truthy(arg(2)), now)
                .map(|()| PyValue::Bool(true))
        }
        // `FinishAuction(contractID, isIssuer)`: the Get Money (issuer) and
        // Get Items (winner) buttons (contractentry.py:268/274,
        // contractsDetailsWnd `GetItems`/`GetMoney`, **[V]**); a truthy
        // answer removes the row.
        "FinishAuction" => {
            let me = req
                .actor
                .ok_or_else(|| con_error("ConContractNotFound", vec![]))?;
            finish_auction(reg, me, con::int(arg(0)), con::truthy(arg(1)), now)
                .map(|()| PyValue::Bool(true))
        }
        // The courier package's contract: `None`, or an object whose
        // `.contractID` the Deliver and Find menu entries use and whose
        // contract the wizard reads `.endStationID` off
        // (contracts.py:1044-1067, **[V]**). The contract row is that object;
        // a package this character cannot see the contract of is `None`,
        // which the client turns into `ConContractNotFoundForCrate` itself.
        "GetCourierContractFromItemID" => {
            let item = con::int(arg(0));
            Ok(match req.actor {
                Some(me) => reg
                    .contracts
                    .all()
                    .iter()
                    .find(|c| c.crate_id == item && item != 0 && c.visible_to(&who(reg, me)))
                    .map_or(PyValue::None, Contract::row),
                None => PyValue::None,
            })
        }
        // `DeleteNotification(contractID, forCorp)`, truthy to drop the row
        // (contractentry.py `Dismiss:262`, **[V]**). Nothing in this client
        // ever sets the `canDismiss` node attribute that offers it -- it is
        // only read (`ContractEntry.GetMenu:129`) -- so this is answered for
        // completeness and changes nothing.
        "DeleteNotification" => Ok(PyValue::Bool(true)),
        // `ForgetAuctionContract(contractID, forCorp)` -> `DeleteContract
        // Notification(contractID, forCorp)`, answer dropped (`POP_TOP`),
        // then the client clears its own expired list (contracts.py:1031,
        // **[V]**). The menu entry is only offered on a requires-attention
        // auction the caller did not issue and, if they won it, has already
        // claimed (`contractentry._OfferForgetAuctionContract`, **[V]**).
        // [`needs_attention`] lists an auction for its winner only until they
        // collect, and never for a loser, so the entry is unreachable here:
        // answered for completeness, and it changes nothing.
        "DeleteContractNotification" => Ok(PyValue::None),
        // `GM_ExpireContract(contractID)`, from the "GM - Expire now" menu
        // entry that `ContractEntry.GetMenu` adds only for `ROLE_GML`
        // (contractentry.py:157-160 and `_ExpireContract:235`, **[V]**); the
        // answer is dropped. It ends the contract's life now, so the issuer's
        // Delete (which returns the escrow) is offered. The client is the
        // role gate (`Request` carries no role, and every sandbox session has
        // `ROLE_GML`); the server still limits it to contracts the caller can
        // see, which is every contract the menu is reachable from.
        "GM_ExpireContract" => {
            let me = req
                .actor
                .ok_or_else(|| con_error("ConContractNotFound", vec![]))?;
            let id = con::int(arg(0));
            if !reg
                .contracts
                .get(id)
                .is_some_and(|c| c.visible_to(&who(reg, me)))
            {
                return Err(con_error("ConContractNotFound", vec![]));
            }
            if let Some(live) = reg.contracts.get_mut(id) {
                live.date_expired = live.date_expired.min(now - 1);
            }
            reg.dirty = true;
            Ok(PyValue::None)
        }
        // The wizard's "split stack" on an item row:
        // `SplitStack(stationID, itemID, qty, forCorp, flag)`, then
        // `OnItemSplit` and a refresh that re-lists the hangar
        // (contracts.py:829-837, contractentry.py:508-513, **[V]**).
        "SplitStack" => {
            let me = req
                .actor
                .ok_or_else(|| con_error("ConContractNotFound", vec![]))?;
            split_stack(
                reg,
                me,
                con::int(arg(0)),
                con::int(arg(1)),
                con::int(arg(2)),
            )
            .map(|()| PyValue::Bool(true))
        }
        _ => Err(reg.no_method(req)),
    }
}

/// The contracts window's start page, as a `KeyVal`.
///
/// **Ten fields, and the list matters more than the values.** Three are read
/// arithmetically by `contracts.py(884) CollectMyPageInfo`:
///
/// ```text
/// mpi.numContractsLeft        = max(maxNumContracts     - mpi.numOutstandingContractsNonCorp, 0)
/// mpi.numContractsLeftForCorp = max(maxNumContractsCorp - mpi.numOutstandingContractsForCorp, 0)
/// mpi.numContractsLeftInCorp  = max(MAX_NUM_CONTRACTS   - mpi.numOutstandingContracts, 0)
/// ```
///
/// so they must be **real ints**. `outstandingContracts` is iterated by
/// `contractPanels.Init`, and `None` there pops a modal `ConNotReady` at the
/// player every time the window opens; its entries are read **by position**
/// as (issuerID, issuerCorpID, assigneeID, contractType) and counted as
/// "assigned to me" when `[2] == session.charid`. The other six are compared
/// `> 0`.
///
/// **Five fields are deliberately absent**: `numContractsLeft`,
/// `numContractsLeftForCorp`, `numContractsTotal`, `numContractsLeftInCorp`
/// and `timeout` are assigned by `CollectMyPageInfo` itself from the
/// character's Contracting skills. That is also why this is a `KeyVal` and
/// not a row: the answer must accept attribute assignment.
fn page_info(reg: &Registry, actor: Option<i64>, now: i64) -> PyValue {
    let (mine, assigned, attention, bidding, carrying) = match actor {
        Some(me) => (
            outstanding_issued_by(reg, me),
            reg.contracts
                .newest_first()
                .filter(|c| c.assigned_to(&who(reg, me)) && c.is_open(now))
                .map(Contract::outstanding_tuple)
                .collect(),
            reg.contracts
                .all()
                .iter()
                .filter(|c| needs_attention(c, me, now, crate::game_clock::of(reg)))
                .count() as i64,
            reg.contracts
                .all()
                .iter()
                .filter(|c| c.is_open(now) && c.bids.iter().any(|b| b.bidder_id == me))
                .count() as i64,
            reg.contracts
                .all()
                .iter()
                .filter(|c| {
                    c.status == con::STATUS_IN_PROGRESS
                        && (c.acceptor_id == me || c.issuer_id == me)
                })
                .count() as i64,
        ),
        None => (0, Vec::new(), 0, 0, 0),
    };
    keyval(vec![
        ("numOutstandingContracts", PyValue::Int(mine)),
        ("numOutstandingContractsNonCorp", PyValue::Int(mine)),
        ("numOutstandingContractsForCorp", PyValue::Int(0)),
        ("outstandingContracts", PyValue::List(assigned)),
        ("numRequiresAttention", PyValue::Int(attention)),
        ("numRequiresAttentionCorp", PyValue::Int(0)),
        ("numBiddingOn", PyValue::Int(bidding)),
        ("numBiddingOnCorp", PyValue::Int(0)),
        ("numInProgress", PyValue::Int(carrying)),
        ("numInProgressCorp", PyValue::Int(0)),
    ])
}

/// Contracts `me` issued that still hold escrow, expired or not, or are
/// being carried: the count the wizard's "N of M contracts" limit is about.
fn outstanding_issued_by(reg: &Registry, me: i64) -> i64 {
    reg.contracts
        .all()
        .iter()
        .filter(|c| {
            c.issuer_id == me
                && matches!(c.status, con::STATUS_OUTSTANDING | con::STATUS_IN_PROGRESS)
        })
        .count() as i64
}

/// `GetLoginInfo().needsAttention`: one `DBRow(contractID, isForCorp)` per
/// contract [`needs_attention`] names for `actor`, newest first. A real row
/// rather than a `KeyVal` because the client reads the first one by
/// position (`[1]`) and by name (`.contractID`).
fn attention_rows(reg: &Registry, actor: Option<i64>, now: i64) -> Vec<PyValue> {
    let Some(me) = actor else {
        return Vec::new();
    };
    let clock = crate::game_clock::of(reg);
    reg.contracts
        .newest_first()
        .filter(|c| needs_attention(c, me, now, clock))
        .filter_map(|c| {
            let columns = ["contractID", "isForCorp"]
                .into_iter()
                .zip([DbType::I8, DbType::I4])
                .map(|(name, db_type)| Column {
                    name: name.as_bytes().to_vec(),
                    db_type,
                })
                .collect();
            let row = DbRow::new(
                DbRowDescriptor::new(columns).ok()?,
                vec![DbValue::Int(c.contract_id), DbValue::Int(0)],
            )
            .ok()?;
            Some(PyValue::DbRow(Box::new(row)))
        })
        .collect()
}

/// Something `me` has to act on, **[A]**: an issuer's expired contract
/// waiting for a Delete (or, for an auction with bids, a Get Money); an
/// expired auction's winner who has not collected the items; an issuer
/// whose courier is overdue and can be failed.
fn needs_attention(c: &Contract, me: i64, now: i64, clock: crate::game_clock::GameClock) -> bool {
    let expired_auction_won = c.kind == con::TYPE_AUCTION
        && c.is_expired(now)
        && c.top_bid().is_some_and(|b| b.bidder_id == me)
        && matches!(
            c.status,
            con::STATUS_OUTSTANDING | con::STATUS_FINISHED_ISSUER
        );
    (c.issuer_id == me && c.status == con::STATUS_OUTSTANDING && c.is_expired(now))
        || expired_auction_won
        || (c.issuer_id == me && c.is_overdue(now, clock))
}

/// The ISK of `me`'s that `c` is holding: the reward they put up as its
/// issuer, the collateral they put up as its courier, the bid they hold as
/// its high bidder. `GetMyContractEscrow`'s figure.
fn held_from(c: &Contract, me: i64) -> f64 {
    let mut held = 0.0;
    if c.issuer_id == me && matches!(c.status, con::STATUS_OUTSTANDING | con::STATUS_IN_PROGRESS) {
        held += c.reward;
    }
    if c.kind == con::TYPE_COURIER && c.status == con::STATUS_IN_PROGRESS && c.acceptor_id == me {
        held += c.collateral;
    }
    if c.kind == con::TYPE_AUCTION
        && !matches!(c.status, con::STATUS_FINISHED_ISSUER | con::STATUS_FINISHED)
    {
        held += c
            .top_bid()
            .filter(|b| b.bidder_id == me)
            .map_or(0.0, |b| b.amount);
    }
    held
}

/// The My Contracts Status filter. 0 is outstanding and **[A]** excludes the
/// expired ones, which "requires attention" lists instead; 4 is the
/// Finished filter and **[A]** takes every closed status but Deleted.
fn status_matches(c: &Contract, filter: i64, now: i64) -> bool {
    match filter {
        con::STATUS_OUTSTANDING => c.is_open(now),
        con::STATUS_FINISHED => !matches!(
            c.status,
            con::STATUS_OUTSTANDING | con::STATUS_IN_PROGRESS | con::STATUS_DELETED
        ),
        other => c.status == other,
    }
}

/// A contract-type filter: `None` or `CONTYPE_ALL` is every type and
/// `CONTYPE_AUCTIONANDITEMECHANGE` is two of them (`contractscommon`, **[V]**).
fn kind_matches(kind: i64, filter: Option<i64>) -> bool {
    match filter {
        None | Some(con::TYPE_ALL) => true,
        Some(con::TYPE_AUCTION_AND_ITEM_EXCHANGE) => {
            kind == con::TYPE_ITEM_EXCHANGE || kind == con::TYPE_AUCTION
        }
        Some(k) => kind == k,
    }
}

/// The Search tab: `ret.contracts`, `ret.numFound`, `ret.searchTime`,
/// `ret.maxResults`, and each result `.contract`, `.items`, `.bids`
/// (contractsearch.py `SearchContracts`, **[V]**). `searchTime` is divided by
/// `float(const.SEC)` for a log line, so 0 is fine.
///
/// Every argument is a keyword (`contractsearch.py:1316`, **[V]**). The
/// filters honoured: availability (public, mine, corporation, alliance),
/// type, start and end location (station, system, constellation or region),
/// security class of the start system, item types, category, group and name,
/// issuer, ranges on price, reward, collateral and volume, `excludeTrade`,
/// `excludeMultiple` and `excludeNoBuyout`. **Not honoured:** `description`
/// (the client always sends None) and `searchHint` (a line does not keep
/// whether it is a blueprint original or a copy). A `sortBy` (a `SORT_*` of
/// `contractscommon`) sorts, `sortDir` 0 ascending and 1 descending, ties
/// newest first; with no `sortBy` the results are newest first.
fn search(reg: &Registry, req: &Request<'_>, now: i64) -> PyValue {
    let kw = |name: &str| crate::pyutil::pairs_get(req.kwargs, name.as_bytes());
    let opt = |name: &str| con::num(kw(name));
    let me = req.actor.unwrap_or(0);
    let w = who(reg, me);
    let availability = con::int(kw("availability"));
    let kind = opt("contractType").map(|k| k as i64);
    let location = con::int(kw("locationID"));
    let end_location = con::int(kw("endLocationID"));
    let issuer = con::int(kw("issuerID"));
    let range = |lo: &str, hi: &str| (opt(lo), opt(hi));
    let (price, reward) = (
        range("minPrice", "maxPrice"),
        range("minReward", "maxReward"),
    );
    let (collateral, volume) = (
        range("minCollateral", "maxCollateral"),
        range("minVolume", "maxVolume"),
    );
    let within = |v: f64, r: (Option<f64>, Option<f64>)| {
        r.0.is_none_or(|lo| v >= lo) && r.1.is_none_or(|hi| v <= hi)
    };
    let exclude_trade = con::truthy(kw("excludeTrade"));
    let exclude_multiple = con::truthy(kw("excludeMultiple"));
    let exclude_no_buyout = con::truthy(kw("excludeNoBuyout"));
    // A set on the client; whatever container it arrives as, its members
    // (or a dict's keys -- `itemTypes = {typeID: None}` is one spelling).
    let ids_of = |name: &str| -> Option<Vec<i64>> {
        match kw(name) {
            Some(PyValue::List(v)) | Some(PyValue::Tuple(v)) => {
                Some(v.iter().map(|x| con::int(Some(x))).collect())
            }
            Some(d @ PyValue::Dict(_)) => d
                .dict_as_kv()
                .map(|kv| kv.iter().map(|(k, _)| con::int(Some(k))).collect()),
            _ => None,
        }
    };
    let types = ids_of("itemTypes");
    let security = ids_of("securityClasses");
    let category = con::int(kw("itemCategoryID"));
    let group = con::int(kw("itemGroupID"));
    let type_name = match kw("itemTypeName") {
        Some(v @ (PyValue::Unicode(_) | PyValue::Bytes(_))) => con::text(Some(v)).to_lowercase(),
        _ => String::new(),
    };
    let content = reg.content();
    // (group, category) of a type: the loaded type data, else the hand rows.
    let class_of = |t: i64| -> Option<(i64, i64)> {
        crate::typedata::installed_in(content)
            .table()
            .get(t)
            .map(|r| (r.group_id, r.category_id))
            .or_else(|| crate::item::find_in(content, t).map(|i| (i.group_id, i.category_id)))
    };
    let start = con::int(kw("startNum")).max(0) as usize;
    let mut found: Vec<&Contract> = reg
        .contracts
        .newest_first()
        .filter(|c| c.is_open(now))
        .filter(|c| match availability {
            con::AVAIL_PUBLIC => c.assignee_id == 0,
            con::AVAIL_MYSELF => c.availability == con::AVAIL_MYSELF && c.assigned_to(&w),
            con::AVAIL_CORP => c.availability == con::AVAIL_CORP && c.assigned_to(&w),
            con::AVAIL_ALLIANCE => c.availability == con::AVAIL_ALLIANCE && c.assigned_to(&w),
            _ => false,
        })
        .filter(|c| kind_matches(c.kind, kind))
        .filter(|c| {
            location == 0
                || [
                    c.start_station_id,
                    c.start_system_id,
                    c.start_constellation_id,
                    c.start_region_id,
                ]
                .contains(&location)
        })
        // Only a courier has a destination (the client sends the end
        // location for couriers only, `contractsearch.py:1646-1655`).
        .filter(|c| {
            end_location == 0
                || (c.kind == con::TYPE_COURIER
                    && [
                        c.end_station_id,
                        c.end_system_id,
                        c.end_constellation_id,
                        c.end_region_id,
                    ]
                    .contains(&end_location))
        })
        // **[A]** a system the catalogue lacks matches no class; falsified
        // by a security-filtered search that should list it.
        .filter(|c| {
            security.as_ref().is_none_or(|classes| {
                content
                    .get::<crate::celestial::Universe>()
                    .security_class(c.start_system_id)
                    .is_some_and(|s| classes.contains(&(s as i64)))
            })
        })
        .filter(|c| issuer == 0 || c.issuer_id == issuer || c.issuer_corp_id == issuer)
        .filter(|c| within(c.price, price))
        .filter(|c| within(c.reward, reward))
        .filter(|c| within(c.collateral, collateral))
        .filter(|c| within(c.volume, volume))
        .filter(|c| !exclude_trade || c.lines.iter().all(|l| l.in_crate))
        .filter(|c| !exclude_multiple || c.lines.iter().filter(|l| l.in_crate).count() <= 1)
        // An auction's buyout is `collateral` here; any other kind has none
        // to be missing.
        .filter(|c| !exclude_no_buyout || c.kind != con::TYPE_AUCTION || c.collateral > 0.0)
        .filter(|c| {
            types
                .as_ref()
                .is_none_or(|t| c.lines.iter().any(|l| l.in_crate && t.contains(&l.type_id)))
        })
        // Category, group and name, for the lines on offer. The name only
        // when the client sent no `itemTypes`: it resolves the name to
        // ids itself, and a type this server has no name for must not
        // hide a match it already made.
        .filter(|c| {
            (category == 0 && group == 0 && (type_name.is_empty() || types.is_some()))
                || c.lines.iter().filter(|l| l.in_crate).any(|l| {
                    let class = class_of(l.type_id);
                    (category == 0 || class.is_some_and(|k| k.1 == category))
                        && (group == 0 || class.is_some_and(|k| k.0 == group))
                        && (type_name.is_empty()
                            || types.is_some()
                            || crate::item::find_in(content, l.type_id)
                                .is_some_and(|i| i.name.to_lowercase().contains(&type_name)))
                })
        })
        .collect();
    if let Some(sort_by) = opt("sortBy").map(|n| n as i64) {
        let descending = con::int(kw("sortDir")) != 0;
        let key = |c: &Contract| -> f64 {
            match sort_by {
                con::SORT_PRICE => c.price,
                con::SORT_EXPIRED => c.date_expired as f64,
                con::SORT_STATION_ID => c.start_station_id as f64,
                con::SORT_SOLARSYSTEM_ID => c.start_system_id as f64,
                con::SORT_REGION_ID => c.start_region_id as f64,
                con::SORT_CONSTELLATION_ID => c.start_constellation_id as f64,
                con::SORT_CONTRACT_TYPE => c.kind as f64,
                con::SORT_REWARD => c.reward,
                con::SORT_COLLATERAL => c.collateral,
                con::SORT_VOLUME => c.volume,
                con::SORT_ASSIGNEEID => c.assignee_id as f64,
                _ => c.contract_id as f64,
            }
        };
        found.sort_by(|a, b| {
            let order = key(a).total_cmp(&key(b));
            let order = if descending { order.reverse() } else { order };
            order.then(b.contract_id.cmp(&a.contract_id))
        });
    }
    let page: Vec<PyValue> = found
        .iter()
        .skip(start)
        .take(con::CONTRACTS_PER_PAGE)
        .map(|c| c.details())
        .collect();
    keyval(vec![
        ("contracts", PyValue::List(page)),
        ("numFound", PyValue::Int(found.len() as i64)),
        ("searchTime", PyValue::Int(0)),
        ("maxResults", PyValue::Int(con::MAX_CONTRACTS_PER_SEARCH)),
    ])
}

/// A refusal under one of the client's own message keys. Every key used here
/// is a dialog row in the client (`findings/user-errors.tsv`, **[V]**), and
/// every caller is `except UserError: raise`, so the player reads it.
fn con_error(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
}

/// A refusal for something this server does not do, said plainly.
fn unsupported(text: &str) -> ServiceError {
    con_error("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

fn not_enough_money(amount: f64, balance: f64) -> ServiceError {
    con_error(
        "NotEnoughMoney",
        vec![
            ("amount", PyValue::Real(amount)),
            ("balance", PyValue::Real(balance)),
        ],
    )
}

/// Does `me` own item `id`?
fn owns(reg: &Registry, me: i64, id: i64) -> bool {
    reg.store().get(id).is_some_and(|i| i.owner_id == me)
}

/// Everything inside item `id`, at any depth: a ship's fitting and cargo, a
/// container's contents, and theirs.
fn contents(store: &crate::item::Store, id: i64) -> Vec<crate::item::Item> {
    let mut out = Vec::new();
    let mut open = vec![id];
    while let Some(parent) = open.pop() {
        for i in store.items().iter().filter(|i| i.location_id == parent) {
            out.push(i.clone());
            open.push(i.item_id);
        }
    }
    out
}

/// Add `delta` ISK to a character's wallet, journal it, and tell **them**.
///
/// `service/market.rs`' `move_isk` with the contract's reference types:
/// `Registry::charge` tells the acting session, which is wrong for the issuer
/// paid inside the acceptor's call. Journal owners are the payer then the
/// payee, **[A]** like the market's.
fn pay(
    reg: &mut Registry,
    owner: i64,
    delta: f64,
    entry_type_id: i64,
    counterparty: i64,
    contract_id: i64,
    now: i64,
) {
    let delta = (delta * 100.0).round() / 100.0;
    if delta == 0.0 {
        return;
    }
    let Some(c) = reg.character_mut(owner) else {
        return;
    };
    c.balance = ((c.balance + delta) * 100.0).round() / 100.0;
    let balance = c.balance;
    let (from, to) = if delta < 0.0 {
        (owner, counterparty)
    } else {
        (counterparty, owner)
    };
    reg.dirty = true;
    reg.journal.record(
        now,
        entry_type_id,
        from,
        to,
        contract_id,
        delta,
        balance,
        "",
    );
    reg.push(crate::notify::Addressed {
        to: Some(vec![owner]),
        inner: Box::new(crate::notify::AccountChange {
            owner_id: owner,
            balance,
        }),
    });
}

/// The edits that take everything directly inside `container` to `to`, and
/// re-own everything inside *those* to `to.owner_id` where it sits: a ship
/// keeps its fitting and cargo, a container its contents.
///
/// `container` is an escrow location or a courier package; both hold their
/// goods at the top level and the goods' own contents beneath.
fn release_from(reg: &Registry, container: i64, to: crate::item::Slot) -> Vec<crate::item::Edit> {
    use crate::item::{Edit, Slot};
    let mut edits = Vec::new();
    let top: Vec<i64> = reg
        .store()
        .items()
        .iter()
        .filter(|i| i.location_id == container)
        .map(|i| i.item_id)
        .collect();
    for id in top {
        edits.push(Edit::Move { item_id: id, to });
        for inner in contents(reg.store(), id) {
            edits.push(Edit::Move {
                item_id: inner.item_id,
                to: Slot::new(to.owner_id, inner.location_id, inner.flag_id),
            });
        }
    }
    edits
}

/// The edits that take a contract's escrow out to `owner`'s hangar at the
/// contract's station.
fn release_escrow(reg: &Registry, c: &Contract, owner: i64) -> Vec<crate::item::Edit> {
    release_from(
        reg,
        c.escrow_slot().location_id,
        crate::item::Slot::new(owner, c.start_station_id, crate::model::FLAG_HANGAR),
    )
}

/// Who `me` is to a contract: themselves, their player corporation and
/// their alliance.
fn who(reg: &Registry, me: i64) -> con::Who {
    let corp = reg.corp_membership(me).map_or(0, |m| m.0);
    let alliance = (corp != 0)
        .then(|| crate::alliance::of_corp(reg.deltas(), corp))
        .flatten()
        .or_else(|| crate::alliance::of_character(reg, me))
        .unwrap_or(0);
    con::Who { me, corp, alliance }
}

/// Every character a corporation or alliance assignment reaches, in id
/// order. An alliance is its member corporations' members.
fn assignment_members(reg: &Registry, assignee: i64, availability: i64) -> Vec<i64> {
    let corps: Vec<i64> = if availability == con::AVAIL_ALLIANCE {
        crate::alliance::Alliances::load(reg.deltas())
            .members_of(assignee)
            .into_iter()
            .map(|(corp, _)| corp)
            .collect()
    } else {
        vec![assignee]
    };
    let mut out: Vec<i64> = corps
        .into_iter()
        .flat_map(|c| reg.corporations().members_of(c))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Where a station is: (solar system, constellation, region). The world's
/// station table first (`Registry::station_ref`: a seeded station or any one
/// the catalogue places), so a station nobody is docked at still resolves;
/// then, for a seeded world with no bundle, a character based there.
fn station_place(reg: &Registry, me: i64, station: i64) -> Option<(i64, i64, i64)> {
    // An Upwell structure is a place a contract can start or end (U5):
    // `station_ref` does not know it (it is no station to `CmdDock`).
    if let Some(s) = crate::structure_dock::place_ref(reg, station) {
        return Some((s.solar_system_id, s.constellation_id, s.region_id));
    }
    reg.character(me)
        .filter(|c| c.station_id == Some(station))
        .or_else(|| {
            reg.config()
                .characters
                .iter()
                .find(|c| c.station_id == Some(station))
        })
        .map(|c| {
            (
                c.solar_system_id.unwrap_or(0),
                c.constellation_id,
                c.region_id,
            )
        })
}

/// `CreateContract(contractInfo, confirm=None)` -> **a list of contract ids**:
/// `ShowCreatedContracts` does `len(contractIDs)` and `contractIDs[0]`
/// (createContractWnd.py, **[V]**).
///
/// Item exchange, courier and auction. The issuer's items, from their hangar
/// at the start station, move into this contract's escrow location in one
/// plan, and a reward leaves their wallet into the contract. No broker's
/// fee, sales tax or deposit is charged, **[A]**: the wizard's confirmation
/// page shows the fees `contractscommon.CalcContractFees` computes
/// client-side, and a server for a handful of friends charging them would be
/// ISK leaving the world for no one.
///
/// **No docked gate, [A].** The wizard lists every station holding the
/// character's assets (`invCache.ListStations`, `WriteSelectItems`) and the
/// item check below is what matters: the items must be in *this* character's
/// hangar at *that* station.
///
/// What each type reads from the `ContractStruct` (**[V]**, the wizard's
/// keywords, `createContractWnd.CreateContract:1144-1160`):
///
/// - item exchange: `price`, `reward`, `itemList`, `requestItemTypeList`;
/// - courier: `destinationID`, `numDays` (days to deliver), `reward`,
///   `collateral`, `itemList`;
/// - auction: `price` (the starting bid), `collateral` (the buyout, 0 for
///   none), `itemList`.
fn create_contract(
    reg: &mut Registry,
    req: &Request<'_>,
    now: i64,
) -> Result<PyValue, ServiceError> {
    use crate::item::{Edit, Slot};
    let Some(me) = req.actor else {
        return Err(unsupported("Select a character first."));
    };
    let Some(mut d) = req.args.first().and_then(con::parse_draft) else {
        // Not something the wizard can send: it always builds a
        // `ContractStruct`. A malformed call, not a refusal to explain.
        return Err(ServiceError::BadArguments {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
            why: "the first argument is not a ContractStruct".to_owned(),
        });
    };
    let (courier, auction) = (d.kind == con::TYPE_COURIER, d.kind == con::TYPE_AUCTION);
    if !matches!(
        d.kind,
        con::TYPE_ITEM_EXCHANGE | con::TYPE_COURIER | con::TYPE_AUCTION
    ) {
        return Err(unsupported(
            "This server has item exchange, courier and auction contracts, not loans.",
        ));
    }
    if d.for_corp || d.multi_contract {
        return Err(unsupported("This server has personal contracts only."));
    }
    // What a type does not use is dropped rather than refused, **[A]**: the
    // wizard keeps every page's values in one `data`, so a courier drafted
    // after an abandoned item exchange can still carry its price.
    if courier {
        d.price = 0.0;
        d.requested.clear();
    } else {
        d.end_station_id = 0;
        d.num_days = 0;
    }
    if auction {
        d.reward = 0.0;
        d.requested.clear();
    } else if !courier {
        d.collateral = 0.0;
    }
    // The assignee: nobody, a character in this world, or the issuer's own
    // player corporation or alliance (the client offers no other).
    let mine = who(reg, me);
    let availability = if d.assignee_id == 0 {
        con::AVAIL_PUBLIC
    } else if reg.character(d.assignee_id).is_some() {
        con::AVAIL_MYSELF
    } else if mine.corp != 0 && d.assignee_id == mine.corp {
        con::AVAIL_CORP
    } else if mine.alliance != 0 && d.assignee_id == mine.alliance {
        con::AVAIL_ALLIANCE
    } else {
        return Err(unsupported(
            "Contracts can only be assigned to a character on this server, or to your own corporation or alliance.",
        ));
    };
    if d.assignee_id == me {
        return Err(con_error("ConContractSameIssuerAndAcceptor", vec![]));
    }
    if d.minutes_expire <= 0 {
        return Err(con_error("ConDurationZero", vec![]));
    }
    let minutes = d.minutes_expire.min(con::MAX_EXPIRE_MINUTES);
    for amount in [d.price, d.reward, d.collateral] {
        if !(0.0..=con::MAX_AMOUNT).contains(&amount) {
            return Err(con_error("ConContractInvalid", vec![]));
        }
    }
    if d.items.is_empty() && d.requested.is_empty() && d.price == 0.0 && d.reward == 0.0 {
        return Err(con_error("ConNoItems", vec![]));
    }
    // A courier or an auction is *of* something.
    if (courier || auction) && d.items.is_empty() {
        return Err(con_error("ConNoItems", vec![]));
    }
    // `ConAuctionBuyoutTooSmall`: the client raises `RuntimeError('Buyout is
    // lower than starting bid!')` on such a contract (`PlaceBid:736`).
    if auction && d.collateral > 0.0 && d.collateral < d.price {
        return Err(con_error("ConAuctionBuyoutTooSmall", vec![]));
    }
    if d.items.len() > con::MAX_NUM_ITEMS || d.requested.len() > con::MAX_NUM_ITEMS {
        return Err(con_error(
            "ConTooManyItems",
            vec![
                ("max", PyValue::Int(con::MAX_NUM_ITEMS as i64)),
                (
                    "num",
                    PyValue::Int(d.items.len().max(d.requested.len()) as i64),
                ),
            ],
        ));
    }
    let Some(issuer) = reg.character(me).cloned() else {
        return Err(unsupported("Select a character first."));
    };
    let station = if d.start_station_id != 0 {
        d.start_station_id
    } else {
        match reg.docked_station(me) {
            Some(s) => s,
            None => return Err(con_error("ConNotInStation", vec![])),
        }
    };
    // Where the station is: the world's station table, else a pilot's home.
    // A station this server cannot place is refused rather than stored with
    // zeros, **[A]** (falsifier: a retail contract at a station the client
    // lists but the server lacks).
    let Some((system, constellation, region)) = station_place(reg, me, station) else {
        return Err(con_error("ConNotInStation", vec![]));
    };
    // A courier's destination. **Only a station this server can place**,
    // because every reader of the contract resolves `endSolarSystemID` --
    // `contractsearch` names it for every result -- and a 0 there is a
    // lookup of nothing. The same station as the start is `ConDestinationSame`
    // when the world's station table knows it; **a world with no table (a
    // seeded one, one dockable station) allows it, [A]**, or couriers there
    // would be impossible rather than wrong.
    let (end_station, end_place) = if courier {
        if d.end_station_id == 0 {
            return Err(unsupported(
                "A courier contract needs a destination station: drag one onto Ship To.",
            ));
        }
        if d.num_days < 1 {
            return Err(con_error("ConDurationZero", vec![]));
        }
        // The same station is refused once the world has another to go to
        // (the table or the catalogue); alone, it is allowed, **[A]**.
        if d.end_station_id == station
            && (reg.stations.keys().any(|k| *k != station)
                || reg
                    .config()
                    .celestials
                    .served()
                    .any(|c| c.kind == crate::celestial::Kind::Station && c.id != station))
        {
            return Err(con_error("ConDestinationSame", vec![]));
        }
        match station_place(reg, me, d.end_station_id) {
            Some(p) => (d.end_station_id, p),
            None => {
                return Err(unsupported(
                    "This server does not know that destination station.",
                ));
            }
        }
    } else {
        (station, (system, constellation, region))
    };
    if d.reward > 0.0 && issuer.balance < d.reward {
        return Err(not_enough_money(d.reward, issuer.balance));
    }
    // The items: the issuer's own, in the hangar at that station, not the
    // ship they are sitting in, each named once, each quantity a part of
    // the stack. `ConCannotTradeItemSanity` for anything else.
    let hangar = Slot::new(me, station, crate::model::FLAG_HANGAR);
    let mut seen = Vec::new();
    let mut volume = 0.0;
    for &(item_id, qty) in &d.items {
        if item_id == issuer.ship_id {
            return Err(con_error("ConCannotTradeCurrentShip", vec![]));
        }
        let Some(item) = reg.store().get(item_id) else {
            return Err(con_error("ConCannotTradeItemSanity", vec![]));
        };
        if item.slot() != hangar || qty < 1 || qty > item.quantity || seen.contains(&item_id) {
            return Err(con_error("ConCannotTradeItemSanity", vec![]));
        }
        seen.push(item_id);
        // `Store::used_volume`'s sum, taken before anything moves so a
        // refusal moves nothing.
        volume += crate::item::find_in(reg.content(), item.type_id).map_or(0.0, |t| t.volume)
            * qty as f64;
    }
    if courier && volume > con::COURIER_MAX_VOLUME {
        return Err(con_error(
            "ConExceedsMaxVolume",
            vec![
                ("max", PyValue::Real(con::COURIER_MAX_VOLUME)),
                ("vol", PyValue::Real(volume)),
            ],
        ));
    }
    let id = reg.contracts.mint();
    let escrow = con::escrow_slot(id);
    let mut edits = Vec::new();
    for &(item_id, qty) in &d.items {
        let Some(item) = reg.store().get(item_id).cloned() else {
            return Err(con_error("ConCannotTradeItemSanity", vec![]));
        };
        if qty == item.quantity {
            edits.push(Edit::Move {
                item_id,
                to: escrow,
            });
            for inner in contents(reg.store(), item_id) {
                edits.push(Edit::Move {
                    item_id: inner.item_id,
                    to: Slot::new(con::ESCROW_OWNER, inner.location_id, inner.flag_id),
                });
            }
        } else {
            edits.push(Edit::Split {
                item_id,
                quantity: qty,
                to: escrow,
            });
        }
    }
    reg.apply_items(&edits)
        .map_err(|_| con_error("ConCannotTradeItemSanity", vec![]))?;
    // The lines are read off what is in escrow now, so a split's new id is
    // the one recorded. Top-level lines carry flag 0, **[A]**: the details
    // window prints `GetShipFlagLocationName(item.flagID)` for any truthy
    // flag, which is right for a fitted module and noise for a hangar stack.
    let mut lines = Vec::new();
    let top: Vec<crate::item::Item> = reg.store().at(escrow).cloned().collect();
    for item in &top {
        lines.push(con::Line {
            item_id: Some(item.item_id),
            type_id: item.type_id,
            quantity: item.quantity,
            in_crate: true,
            parent_id: 0,
            flag_id: 0,
        });
        for inner in contents(reg.store(), item.item_id) {
            lines.push(con::Line {
                item_id: Some(inner.item_id),
                type_id: inner.type_id,
                quantity: inner.quantity,
                in_crate: true,
                parent_id: inner.location_id,
                flag_id: inner.flag_id,
            });
        }
    }
    for &(type_id, quantity) in &d.requested {
        if quantity >= 1 {
            lines.push(con::Line {
                item_id: None,
                type_id,
                quantity,
                in_crate: false,
                parent_id: 0,
                flag_id: 0,
            });
        }
    }
    let volume =
        reg.store()
            .used_volume_in(reg.content(), escrow.location_id, crate::model::FLAG_HANGAR);
    pay(
        reg,
        me,
        -d.reward,
        con::REF_REWARD_ADDED,
        con::ESCROW_OWNER,
        id,
        now,
    );
    let cents = |v: f64| (v * 100.0).round() / 100.0;
    reg.contracts.insert(Contract {
        contract_id: id,
        kind: d.kind,
        issuer_id: me,
        issuer_corp_id: issuer.corporation_id,
        assignee_id: d.assignee_id,
        availability,
        acceptor_id: 0,
        start_station_id: station,
        start_system_id: system,
        start_constellation_id: constellation,
        start_region_id: region,
        end_station_id: end_station,
        end_system_id: end_place.0,
        end_constellation_id: end_place.1,
        end_region_id: end_place.2,
        price: cents(d.price),
        reward: cents(d.reward),
        collateral: cents(d.collateral),
        crate_id: 0,
        bids: Vec::new(),
        volume,
        date_issued: now,
        // Lane clock: the expiry the issuer chose, at the game clock's pace.
        date_expired: now + crate::game_clock::span(reg, minutes * con::MINUTE),
        date_accepted: 0,
        date_completed: 0,
        num_days: d.num_days,
        status: con::STATUS_OUTSTANDING,
        title: d.title,
        lines,
    });
    reg.dirty = true;
    match availability {
        con::AVAIL_PUBLIC => {}
        con::AVAIL_MYSELF => reg.push(con::ContractEvent::assigned(d.assignee_id, id)),
        // Every member of the corporation, or of every corporation in the
        // alliance; the push reaches those online. Not the issuer.
        _ => {
            for member in assignment_members(reg, d.assignee_id, availability) {
                if member != me {
                    reg.push(con::ContractEvent::assigned(member, id));
                }
            }
        }
    }
    Ok(PyValue::List(vec![PyValue::Int(id)]))
}

/// `AcceptContract(contractID, forCorp)` -> the contract row: the caller reads
/// `contract.type` and, for a courier, `contractID`, `startStationID`,
/// `endStationID`, `dateAccepted`, `numDays` (contracts.py `AcceptContract`,
/// **[V]**).
///
/// One item plan moves the escrow to the acceptor's hangar at the contract's
/// station and the requested items from that hangar to the issuer's, so
/// either all of it happens or nothing does. ISK is checked before and moved
/// after. **No docked gate, [A]**, the same as creating: the client's own
/// accept path checks only that the station exists (`CheckEndpointAccess`).
/// A courier is [`accept_courier`]; an auction is bid on, not accepted (the
/// client raises `RuntimeError('Invalid contract type')` before it would
/// call this for one).
fn accept_contract(
    reg: &mut Registry,
    req: &Request<'_>,
    now: i64,
) -> Result<PyValue, ServiceError> {
    use crate::item::{Edit, Slot};
    let Some(me) = req.actor else {
        return Err(con_error("ConContractNotFound", vec![]));
    };
    if con::truthy(req.args.get(1)) {
        return Err(unsupported("This server has personal contracts only."));
    }
    let id = con::int(req.args.first());
    let c = match reg.contracts.get(id) {
        Some(c) if c.visible_to(&who(reg, me)) => c.clone(),
        _ => return Err(con_error("ConContractNotFound", vec![])),
    };
    if c.status != con::STATUS_OUTSTANDING {
        return Err(con_error("ConContractNotOutstanding", vec![]));
    }
    if c.is_expired(now) {
        return Err(con_error("ConContractExpired", vec![]));
    }
    if c.issuer_id == me {
        return Err(con_error("ConContractSameIssuerAndAcceptor", vec![]));
    }
    if c.assignee_id != 0 && !c.assigned_to(&who(reg, me)) {
        return Err(con_error("ConNotYourContract", vec![]));
    }
    if c.kind == con::TYPE_AUCTION {
        return Err(unsupported("An auction is bid on, not accepted."));
    }
    let Some(acceptor) = reg.character(me).cloned() else {
        return Err(con_error("ConContractNotFound", vec![]));
    };
    if c.kind == con::TYPE_COURIER {
        return accept_courier(reg, me, &c, acceptor.balance, now);
    }
    if c.price > 0.0 && acceptor.balance < c.price {
        return Err(not_enough_money(c.price, acceptor.balance));
    }
    let mut edits = release_escrow(reg, &c, me);
    // What the issuer asked for, out of the acceptor's hangar at the
    // contract's station. Stackable items only -- an assembled ship or a
    // container with contents is not a unit of a type -- and never the ship
    // the acceptor is sitting in. **[A]** on both.
    let hangar = Slot::new(me, c.start_station_id, crate::model::FLAG_HANGAR);
    let to_issuer = Slot::new(c.issuer_id, c.start_station_id, crate::model::FLAG_HANGAR);
    for want in c.lines.iter().filter(|l| !l.in_crate) {
        let mut left = want.quantity;
        for item in reg.store().at(hangar) {
            if left == 0 {
                break;
            }
            if item.type_id != want.type_id || item.singleton || item.item_id == acceptor.ship_id {
                continue;
            }
            let take = left.min(item.quantity);
            edits.push(if take == item.quantity {
                Edit::Move {
                    item_id: item.item_id,
                    to: to_issuer,
                }
            } else {
                Edit::Split {
                    item_id: item.item_id,
                    quantity: take,
                    to: to_issuer,
                }
            });
            left -= take;
        }
        if left > 0 {
            let name = crate::item::find_in(reg.content(), want.type_id)
                .map_or_else(|| format!("type {}", want.type_id), |t| t.name.to_owned());
            return Err(con_error(
                "ConReturnItemsMissingNonSingleton",
                vec![
                    ("station", PyValue::Int(c.start_station_id)),
                    ("example", PyValue::Unicode(name)),
                ],
            ));
        }
    }
    reg.apply_items(&edits)
        .map_err(|_| con_error("ConContractNotOutstanding", vec![]))?;
    pay(reg, me, -c.price, con::REF_PRICE, c.issuer_id, id, now);
    pay(reg, c.issuer_id, c.price, con::REF_PRICE, me, id, now);
    pay(
        reg,
        me,
        c.reward,
        con::REF_REWARD,
        con::ESCROW_OWNER,
        id,
        now,
    );
    let row = {
        let Some(live) = reg.contracts.get_mut(id) else {
            return Err(con_error("ConContractNotFound", vec![]));
        };
        live.acceptor_id = me;
        live.status = con::STATUS_FINISHED;
        live.date_accepted = now;
        live.date_completed = now;
        live.row()
    };
    reg.dirty = true;
    reg.push(con::ContractEvent::accepted(c.issuer_id, id));
    Ok(row)
}

/// Accepting a courier: hold the collateral, and hand the courier **one
/// package** in their hangar at the pickup station with the goods inside.
///
/// The package is a `typePlasticWrap` item, `singleton`, owned by the
/// courier; the goods inside stay **owned by escrow** ([`con::ESCROW_OWNER`]),
/// so the courier carries them without owning them and `apply_items` tells
/// nobody about them. Two plans, because a `Create`'s id does not exist until
/// it is applied: mint the package, then move the goods in; if the second
/// fails the package is destroyed again and nothing else has moved.
///
/// **[A]**: the package's volume is the type's 0.01 m3, not the goods'. The
/// client warns about a package that will not fit the active ship before it
/// accepts (`ConCourierDoesNotFit`, `AcceptContract:502-508`, **[V]**), and
/// the server enforcing hold capacity on a package is not done.
fn accept_courier(
    reg: &mut Registry,
    me: i64,
    c: &Contract,
    balance: f64,
    now: i64,
) -> Result<PyValue, ServiceError> {
    use crate::item::{Change, Edit, Slot};
    if c.collateral > 0.0 && balance < c.collateral {
        return Err(not_enough_money(c.collateral, balance));
    }
    let pickup = Slot::new(me, c.start_station_id, crate::model::FLAG_HANGAR);
    let changes = reg
        .apply_items(&[Edit::Create {
            type_id: con::TYPE_PLASTIC_WRAP,
            quantity: 1,
            to: pickup,
        }])
        .map_err(|_| unsupported("This server could not wrap the courier package."))?;
    let Some(package) = changes.iter().find_map(|ch| match ch {
        Change::Created { item_id, .. } => Some(*item_id),
        _ => None,
    }) else {
        return Err(unsupported(
            "This server could not wrap the courier package.",
        ));
    };
    let inside = Slot::new(con::ESCROW_OWNER, package, crate::model::FLAG_HANGAR);
    let wrap = release_from(reg, c.escrow_slot().location_id, inside);
    if let Err(_why) = reg.apply_items(&wrap) {
        let _ = reg.apply_items(&[Edit::Destroy { item_id: package }]);
        return Err(con_error("ConContractNotOutstanding", vec![]));
    }
    pay(
        reg,
        me,
        -c.collateral,
        con::REF_COLLATERAL,
        con::ESCROW_OWNER,
        c.contract_id,
        now,
    );
    let row = {
        let Some(live) = reg.contracts.get_mut(c.contract_id) else {
            return Err(con_error("ConContractNotFound", vec![]));
        };
        live.acceptor_id = me;
        live.status = con::STATUS_IN_PROGRESS;
        live.date_accepted = now;
        live.crate_id = package;
        live.row()
    };
    reg.dirty = true;
    reg.push(con::ContractEvent::accepted(c.issuer_id, c.contract_id));
    Ok(row)
}

/// A courier contract `me` may act on as its courier or issuer.
fn courier_in_progress(reg: &Registry, me: i64, id: i64) -> Result<Contract, ServiceError> {
    let c = match reg.contracts.get(id) {
        Some(c) if c.visible_to(&who(reg, me)) => c.clone(),
        _ => return Err(con_error("ConContractNotFound", vec![])),
    };
    if c.kind != con::TYPE_COURIER {
        return Err(con_error("ConContractInvalid", vec![]));
    }
    if c.status != con::STATUS_IN_PROGRESS {
        return Err(con_error("ConContractNotInProgress", vec![]));
    }
    Ok(c)
}

/// The station an item is in, following its locations up through the items
/// that hold it, and whether every one of those is `owner`'s. `None` for an
/// item in space or anywhere else that is not under a station.
fn station_holding(reg: &Registry, item_id: i64, owner: i64) -> Option<i64> {
    let mut at = reg.store().get(item_id)?;
    // Deep enough for a package in a container in a ship in a hangar.
    for _ in 0..8 {
        if at.owner_id != owner {
            return None;
        }
        match reg.store().get(at.location_id) {
            Some(parent) => at = parent,
            None => {
                return (at.flag_id == crate::model::FLAG_HANGAR).then_some(at.location_id);
            }
        }
    }
    None
}

/// Delivering a courier (`CompleteContract(id, conStatusFinished)`, by its
/// courier): the package must be at the destination station -- in the
/// courier's hangar there, or inside something of theirs that is
/// (`ConCrateNotFound` otherwise, **[A]** on the "inside something"). The
/// goods go to the issuer's hangar at the destination, the package is gone,
/// and the courier is paid the reward and gets the collateral back.
fn deliver_courier(reg: &mut Registry, me: i64, id: i64, now: i64) -> Result<(), ServiceError> {
    use crate::item::{Edit, Slot};
    let c = courier_in_progress(reg, me, id)?;
    if c.acceptor_id != me {
        return Err(con_error("ConNotYourContract", vec![]));
    }
    if station_holding(reg, c.crate_id, me) != Some(c.end_station_id) {
        return Err(con_error(
            "ConCrateNotFound",
            vec![("location", PyValue::Int(c.end_station_id))],
        ));
    }
    let to = Slot::new(c.issuer_id, c.end_station_id, crate::model::FLAG_HANGAR);
    let mut edits = release_from(reg, c.crate_id, to);
    edits.push(Edit::Destroy {
        item_id: c.crate_id,
    });
    reg.apply_items(&edits)
        .map_err(|_| con_error("ConCourierItemsMissing", vec![]))?;
    pay(
        reg,
        me,
        c.reward,
        con::REF_REWARD,
        con::ESCROW_OWNER,
        id,
        now,
    );
    pay(
        reg,
        me,
        c.collateral,
        con::REF_COLLATERAL_REFUND,
        con::ESCROW_OWNER,
        id,
        now,
    );
    if let Some(live) = reg.contracts.get_mut(id) {
        live.status = con::STATUS_FINISHED;
        live.date_completed = now;
        live.crate_id = 0;
    }
    reg.dirty = true;
    reg.push(con::ContractEvent::completed(c.issuer_id, id));
    Ok(())
}

/// Failing a courier (`CompleteContract(id, conStatusFailed)`): by its
/// courier at any time, by its issuer once it is overdue -- the details
/// window offers Fail to exactly those two (`Init:179-184`, **[V]**). The
/// issuer gets the collateral and the reward back; the courier keeps the
/// goods. **[A]**, how they keep them: the package is unwrapped where it
/// stands and its goods become the courier's, there -- the client's own way
/// is `BreakPlasticWrap` on the package, which the inventory does not
/// answer. A package that no longer exists (its ship was destroyed) settles
/// the ISK alone; anything still addressed to it goes to the courier's
/// hangar at the pickup station.
fn fail_courier(reg: &mut Registry, me: i64, id: i64, now: i64) -> Result<(), ServiceError> {
    use crate::item::{Edit, Slot};
    let c = courier_in_progress(reg, me, id)?;
    let by_courier = c.acceptor_id == me;
    let by_issuer = c.issuer_id == me && c.is_overdue(now, crate::game_clock::of(reg));
    if !(by_courier || by_issuer) {
        return Err(con_error("ConNotYourContract", vec![]));
    }
    let to = match reg.store().get(c.crate_id) {
        Some(package) => Slot::new(c.acceptor_id, package.location_id, package.flag_id),
        None => Slot::new(c.acceptor_id, c.start_station_id, crate::model::FLAG_HANGAR),
    };
    let mut edits = release_from(reg, c.crate_id, to);
    if reg.store().get(c.crate_id).is_some() {
        edits.push(Edit::Destroy {
            item_id: c.crate_id,
        });
    }
    reg.apply_items(&edits)
        .map_err(|_| con_error("ConCourierItemsMissing", vec![]))?;
    pay(
        reg,
        c.issuer_id,
        c.collateral,
        con::REF_COLLATERAL_PAYOUT,
        con::ESCROW_OWNER,
        id,
        now,
    );
    pay(
        reg,
        c.issuer_id,
        c.reward,
        con::REF_REWARD_REFUND,
        con::ESCROW_OWNER,
        id,
        now,
    );
    if let Some(live) = reg.contracts.get_mut(id) {
        live.status = con::STATUS_FAILED;
        live.date_completed = now;
        live.crate_id = 0;
    }
    reg.dirty = true;
    let other = if by_courier {
        c.issuer_id
    } else {
        c.acceptor_id
    };
    reg.push(con::ContractEvent::failed(other, id));
    Ok(())
}

/// `BreakPlasticWrap()` on a courier package: the courier breaks its seal
/// (`invItemFunctions.Break:609`, after the client's `ConfirmBreakCourierPackage`
/// -- "Breaking the seal on this package will make it impossible to complete
/// any contract associated with the package") [V]. That is the courier
/// failing the contract it carries: [`fail_courier`], found through the
/// package. A package with no contract in progress is answered in words.
pub(crate) fn break_package(
    reg: &mut Registry,
    me: i64,
    package: &crate::item::Item,
) -> Result<PyValue, ServiceError> {
    if package.owner_id != me {
        return Err(con_error("ConNotYourContract", vec![]));
    }
    let id = reg
        .contracts
        .all()
        .iter()
        .find(|c| c.crate_id == package.item_id && c.status == con::STATUS_IN_PROGRESS)
        .map(|c| c.contract_id)
        .ok_or_else(|| unsupported("No contract in progress is waiting for that package."))?;
    fail_courier(reg, me, id, crate::pyutil::filetime_now())?;
    Ok(PyValue::None)
}

/// Rejecting a private contract (`CompleteContract(id, conStatusRejected)`,
/// by its assignee): nothing moves; the issuer is told and gets the escrow
/// back with Delete, which the details window offers for a rejected
/// contract (`Init:424-426`, **[V]**). An auction somebody has bid on is not
/// rejectable -- the window hides Reject from a high bidder -- **[A]** that
/// nobody else may either.
fn reject_contract(reg: &mut Registry, me: i64, id: i64, now: i64) -> Result<(), ServiceError> {
    let c = match reg.contracts.get(id) {
        Some(c) if c.visible_to(&who(reg, me)) => c.clone(),
        _ => return Err(con_error("ConContractNotFound", vec![])),
    };
    if !c.assigned_to(&who(reg, me)) {
        return Err(con_error("ConNotYourContract", vec![]));
    }
    // A corporation's or alliance's contract is rejected for it by a member
    // with the contract manager role; a personal one by the assignee.
    if matches!(c.availability, con::AVAIL_CORP | con::AVAIL_ALLIANCE)
        && reg
            .corp_membership(me)
            .is_none_or(|(_, roles)| roles & con::ROLE_CONTRACT_MANAGER == 0)
    {
        return Err(con_error("ConNotYourContract", vec![]));
    }
    if c.status != con::STATUS_OUTSTANDING {
        return Err(con_error("ConContractNotOutstanding", vec![]));
    }
    if !c.bids.is_empty() {
        return Err(con_error("ConAuctionHasBids", vec![]));
    }
    if let Some(live) = reg.contracts.get_mut(id) {
        live.status = con::STATUS_REJECTED;
        live.date_completed = now;
    }
    reg.dirty = true;
    reg.push(con::ContractEvent::rejected(c.issuer_id, id));
    Ok(())
}

/// A bid on an auction. The bid's ISK is held from the bidder at once and
/// the previous high bid refunded (to whoever made it, the same bidder
/// raising included). A bid at or over the buyout is a buyout: it is
/// clamped to the buyout and settles there and then -- the goods to the
/// bidder's hangar at the auction's station, the money to the issuer.
///
/// The floor is the client's own [`Contract::min_bid`]; below it is
/// `ConBidTooLow(bid, minBid)`, the one key `PlaceBid`'s handler branches
/// on, and it re-opens the dialog with the floor filled in (**[V]**,
/// `PlaceBid:741-744`).
fn place_bid(
    reg: &mut Registry,
    me: i64,
    id: i64,
    bid: f64,
    for_corp: bool,
    now: i64,
) -> Result<(), ServiceError> {
    if for_corp {
        return Err(unsupported("This server has personal contracts only."));
    }
    let c = match reg.contracts.get(id) {
        Some(c) if c.visible_to(&who(reg, me)) => c.clone(),
        _ => return Err(con_error("ConContractNotFound", vec![])),
    };
    if c.kind != con::TYPE_AUCTION {
        return Err(con_error("ConContractInvalid", vec![]));
    }
    if c.status != con::STATUS_OUTSTANDING {
        return Err(con_error("ConContractNotOutstanding", vec![]));
    }
    if c.is_expired(now) {
        return Err(con_error("ConContractExpired", vec![]));
    }
    if c.issuer_id == me {
        return Err(con_error("ConContractSameIssuerAndAcceptor", vec![]));
    }
    if c.assignee_id != 0 && !c.assigned_to(&who(reg, me)) {
        return Err(con_error("ConNotYourContract", vec![]));
    }
    let buyout = c.collateral > 0.0 && bid >= c.collateral;
    let bid = if buyout { c.collateral } else { bid.trunc() };
    if !(0.0..=con::MAX_AMOUNT).contains(&bid) {
        return Err(con_error("ConContractInvalid", vec![]));
    }
    let floor = c.min_bid();
    if bid < floor {
        return Err(con_error(
            "ConBidTooLow",
            vec![
                ("bid", PyValue::Real(bid)),
                ("minBid", PyValue::Real(floor)),
            ],
        ));
    }
    let previous = c.top_bid().cloned();
    let balance = reg.character(me).map_or(0.0, |ch| ch.balance);
    let own = previous
        .as_ref()
        .filter(|p| p.bidder_id == me)
        .map_or(0.0, |p| p.amount);
    if balance + own < bid {
        return Err(not_enough_money(bid, balance + own));
    }
    // A buyout's goods move first: if that plan is refused, no ISK has.
    if buyout {
        let edits = release_escrow(reg, &c, me);
        reg.apply_items(&edits)
            .map_err(|_| con_error("ConContractNotOutstanding", vec![]))?;
    }
    if let Some(p) = &previous {
        pay(
            reg,
            p.bidder_id,
            p.amount,
            con::REF_AUCTION_BID_REFUND,
            con::ESCROW_OWNER,
            id,
            now,
        );
    }
    pay(
        reg,
        me,
        -bid,
        con::REF_AUCTION_BID,
        con::ESCROW_OWNER,
        id,
        now,
    );
    if buyout {
        pay(reg, c.issuer_id, bid, con::REF_AUCTION_SOLD, me, id, now);
    }
    if let Some(live) = reg.contracts.get_mut(id) {
        live.bids.insert(
            0,
            con::Bid {
                bidder_id: me,
                amount: bid,
                date: now,
            },
        );
        if buyout {
            live.acceptor_id = me;
            live.status = con::STATUS_FINISHED;
            live.date_accepted = now;
            live.date_completed = now;
        }
    }
    reg.dirty = true;
    if let Some(p) = previous.filter(|p| p.bidder_id != me) {
        reg.push(con::ContractEvent::outbid(p.bidder_id, id));
    }
    if buyout {
        reg.push(con::ContractEvent::buyout(c.issuer_id, id));
    }
    Ok(())
}

/// Collecting an expired auction's half: the issuer the winning bid
/// (`isIssuer`), the winner the goods, in either order; the contract is
/// Finished once both have (`conStatusFinishedIssuer` /
/// `conStatusFinishedContractor` in between, the statuses the details window
/// tests to hide each button again, **[V]**).
fn finish_auction(
    reg: &mut Registry,
    me: i64,
    id: i64,
    is_issuer: bool,
    now: i64,
) -> Result<(), ServiceError> {
    let c = match reg.contracts.get(id) {
        Some(c) if c.visible_to(&who(reg, me)) || c.bids.iter().any(|b| b.bidder_id == me) => {
            c.clone()
        }
        _ => return Err(con_error("ConContractNotFound", vec![])),
    };
    if c.kind != con::TYPE_AUCTION {
        return Err(con_error("ConContractInvalid", vec![]));
    }
    let Some(top) = c.top_bid().cloned() else {
        return Err(con_error("ConContractNotOutstanding", vec![]));
    };
    if !c.is_expired(now) {
        return Err(con_error("ConContractNotOutstanding", vec![]));
    }
    let next = if is_issuer {
        if c.issuer_id != me {
            return Err(con_error("ConNotYourContract", vec![]));
        }
        match c.status {
            con::STATUS_OUTSTANDING => con::STATUS_FINISHED_ISSUER,
            con::STATUS_FINISHED_CONTRACTOR => con::STATUS_FINISHED,
            _ => return Err(con_error("ConAuctionAlreadyClaimed", vec![])),
        }
    } else {
        if top.bidder_id != me {
            return Err(con_error("ConNotYourContract", vec![]));
        }
        match c.status {
            con::STATUS_OUTSTANDING => con::STATUS_FINISHED_CONTRACTOR,
            con::STATUS_FINISHED_ISSUER => con::STATUS_FINISHED,
            _ => return Err(con_error("ConAuctionAlreadyClaimed", vec![])),
        }
    };
    if is_issuer {
        pay(
            reg,
            me,
            top.amount,
            con::REF_AUCTION_SOLD,
            top.bidder_id,
            id,
            now,
        );
    } else {
        let edits = release_escrow(reg, &c, me);
        reg.apply_items(&edits)
            .map_err(|_| con_error("ConContractNotOutstanding", vec![]))?;
    }
    if let Some(live) = reg.contracts.get_mut(id) {
        live.status = next;
        live.acceptor_id = top.bidder_id;
        if live.date_accepted == 0 {
            live.date_accepted = now;
        }
        if next == con::STATUS_FINISHED {
            live.date_completed = now;
        }
    }
    reg.dirty = true;
    Ok(())
}

/// The wizard's split: `qty` off a stack in `me`'s hangar at `station`, as a
/// new stack beside it. `ConSplitStackIllegalQty` for a quantity that leaves
/// nothing behind or takes nothing, **[A]** on the key's reading.
fn split_stack(
    reg: &mut Registry,
    me: i64,
    station: i64,
    item_id: i64,
    qty: i64,
) -> Result<(), ServiceError> {
    let hangar = crate::item::Slot::new(me, station, crate::model::FLAG_HANGAR);
    let Some(item) = reg.store().get(item_id) else {
        return Err(con_error("ConCannotTradeItemSanity", vec![]));
    };
    if item.slot() != hangar || item.singleton {
        return Err(con_error("ConCannotTradeItemSanity", vec![]));
    }
    if qty < 1 || qty >= item.quantity {
        return Err(con_error("ConSplitStackIllegalQty", vec![]));
    }
    reg.apply_items(&[crate::item::Edit::Split {
        item_id,
        quantity: qty,
        to: hangar,
    }])
    .map_err(|_| con_error("ConSplitStackIllegalQty", vec![]))?;
    Ok(())
}

/// Delete an outstanding contract: the escrow goes back to the issuer's
/// hangar at its station and the reward to their wallet. Also how an expired
/// contract is cleared. Only the issuer may; anything not outstanding (or
/// rejected) is refused, **[A]** -- the details window offers Delete for
/// exactly those two statuses (`contractsDetailsWnd.Init`, **[V]**), and for
/// an auction only while it has no bids (`Init:278`, **[V]**), which is what
/// `ConAuctionHasBids` says. A courier being carried is not outstanding.
fn delete_contract(reg: &mut Registry, me: i64, id: i64, now: i64) -> Result<(), ServiceError> {
    let c = match reg.contracts.get(id) {
        Some(c) if c.visible_to(&who(reg, me)) => c.clone(),
        _ => return Err(con_error("ConContractNotFound", vec![])),
    };
    if c.issuer_id != me {
        return Err(con_error("ConNotYourContract", vec![]));
    }
    if c.status != con::STATUS_OUTSTANDING && c.status != con::STATUS_REJECTED {
        return Err(con_error("ConContractNotOutstanding", vec![]));
    }
    if !c.bids.is_empty() {
        return Err(con_error("ConAuctionHasBids", vec![]));
    }
    let edits = release_escrow(reg, &c, me);
    reg.apply_items(&edits)
        .map_err(|_| con_error("ConContractNotOutstanding", vec![]))?;
    pay(
        reg,
        me,
        c.reward,
        con::REF_REWARD_REFUND,
        con::ESCROW_OWNER,
        id,
        now,
    );
    if let Some(live) = reg.contracts.get_mut(id) {
        live.status = con::STATUS_DELETED;
        live.date_completed = now;
    }
    reg.dirty = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(unused_imports)]
    use super::*;

    /// Tests for this subsystem belong here, not in the parent. That is the
    /// other half of why the split exists: two lanes adding a method each
    /// would otherwise contend for one `mod tests`.
    /// Every service this module advertises must reach an arm in *this*
    /// module's `dispatch`.
    ///
    /// It used to check the name against a hand-maintained list in the
    /// parent, which is now derived from these very lists -- so that check
    /// had become a tautology. This probes the dispatcher instead, with a
    /// method that cannot exist: `NoSuchMethod` means we got to the service,
    /// `NoSuchService` means the name is advertised and falls straight
    /// through, which is a refused call, which is a hung client.
    #[test]
    fn every_service_here_is_routed_to_this_module() {
        let mut reg = Registry::new(SandboxConfig::default());
        for s in SERVICES {
            let req = Request {
                service: s,
                method: "__no_such_method_exists__",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: None,
            };
            match dispatch(&mut reg, &req) {
                Err(ServiceError::NoSuchMethod { .. }) => {}
                other => panic!(
                    "{s} is advertised by this module but its dispatch answers {other:?} \
                     to a method that cannot exist"
                ),
            }
        }
    }

    // ---- contracts ---------------------------------------------------------
    //
    // These assert the client's shapes (`[V]` in `crate::contracts`) and the
    // invariants an exchange must keep -- items and ISK are neither created
    // nor lost, and nothing moves on a refusal. Which refusal key and which
    // status a contract ends in are `[A]` choices and deliberately not
    // asserted.

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;
    const CAROL: i64 = 90_000_003;
    const STATION: i64 = 60_003_760;
    const TRITANIUM: i64 = 34;
    const PYERITE: i64 = 35;
    const RIFTER: i64 = 587;

    /// Three pilots based in one station, each with a million ISK.
    fn world() -> Registry {
        let pilot = |id: i64, name: &str, ship: i64| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            station_id: Some(STATION),
            solar_system_id: Some(30_000_142),
            region_id: 10_000_002,
            balance: 1_000_000.0,
            ..Character::default()
        };
        Registry::new(SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice", 100_000_001),
                pilot(BOB, "Bob", 100_000_002),
                pilot(CAROL, "Carol", 100_000_003),
            ],
            ..SandboxConfig::default()
        })
    }

    fn hangar(who: i64) -> crate::item::Slot {
        crate::item::Slot::new(who, STATION, crate::model::FLAG_HANGAR)
    }

    /// Create `quantity` of a type at `to` and answer the new id.
    fn make(reg: &mut Registry, type_id: i64, quantity: i64, to: crate::item::Slot) -> i64 {
        let changes = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id,
                quantity,
                to,
            }])
            .expect("a known type");
        let _ = reg.take_pending();
        changes
            .iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .expect("a created item")
    }

    fn units(reg: &Registry, who: i64, type_id: i64) -> i64 {
        reg.store()
            .at(hangar(who))
            .filter(|i| i.type_id == type_id)
            .map(|i| i.quantity)
            .sum()
    }

    /// Every unit of a type in the world, wherever it is.
    fn world_units(reg: &Registry, type_id: i64) -> i64 {
        reg.store()
            .items()
            .iter()
            .filter(|i| i.type_id == type_id)
            .map(|i| i.quantity)
            .sum()
    }

    fn balance(reg: &Registry, who: i64) -> f64 {
        reg.character(who).map_or(0.0, |c| c.balance)
    }

    /// Every ISK in the world: the wallets plus whatever the contracts hold
    /// (`Contract::escrow_isk`: rewards, collateral, high bids).
    fn world_isk(reg: &Registry) -> f64 {
        let wallets: f64 = reg.config().characters.iter().map(|c| c.balance).sum();
        let escrow: f64 = reg.contracts.all().iter().map(Contract::escrow_isk).sum();
        wallets + escrow
    }

    fn proxy(
        reg: &mut Registry,
        who: i64,
        method: &str,
        args: &[PyValue],
        kwargs: &[(PyValue, PyValue)],
    ) -> Result<PyValue, ServiceError> {
        dispatch(
            reg,
            &Request {
                service: "contractProxy",
                method,
                args,
                kwargs,
                bind_params: None,
                actor: Some(who),
            },
        )
    }

    /// The wizard's `ContractStruct`, its seventeen keywords
    /// (createContractWnd.py `CreateContract`, contractStruct.py).
    fn draft(
        assignee: i64,
        items: &[(i64, i64)],
        requested: &[(i64, i64)],
        price: f64,
        reward: f64,
    ) -> PyValue {
        draft_of(
            con::TYPE_ITEM_EXCHANGE,
            assignee,
            items,
            requested,
            [price, reward, 0.0],
            (0, 0),
        )
    }

    /// Any type's `ContractStruct`: `[price, reward, collateral]` and
    /// `(destinationID, numDays)`.
    fn draft_of(
        kind: i64,
        assignee: i64,
        items: &[(i64, i64)],
        requested: &[(i64, i64)],
        isk: [f64; 3],
        courier: (i64, i64),
    ) -> PyValue {
        draft_at(STATION, kind, assignee, items, requested, isk, courier)
    }

    /// [`draft_of`] starting at `start`.
    fn draft_at(
        start: i64,
        kind: i64,
        assignee: i64,
        items: &[(i64, i64)],
        requested: &[(i64, i64)],
        isk: [f64; 3],
        courier: (i64, i64),
    ) -> PyValue {
        let [price, reward, collateral] = isk;
        let (destination, days) = courier;
        let pairs = |v: &[(i64, i64)]| {
            PyValue::List(
                v.iter()
                    .map(|(a, b)| PyValue::List(vec![PyValue::Int(*a), PyValue::Int(*b)]))
                    .collect(),
            )
        };
        crate::pyutil::pydict(vec![
            ("contractType", PyValue::Int(kind)),
            ("isPrivate", PyValue::Bool(assignee != 0)),
            ("assignedToID", PyValue::Int(assignee)),
            ("minutesExpire", PyValue::Int(1440)),
            ("numDays", PyValue::Int(days)),
            ("startStationID", PyValue::Int(start)),
            ("destinationID", PyValue::Int(destination)),
            ("price", PyValue::Real(price)),
            ("reward", PyValue::Real(reward)),
            ("collateral", PyValue::Real(collateral)),
            ("title", PyValue::Unicode("from Alice".to_owned())),
            ("description", PyValue::Unicode(String::new())),
            ("itemList", pairs(items)),
            ("startStationDivision", PyValue::Int(4)),
            ("requestItemTypeList", pairs(requested)),
            ("forCorp", PyValue::Bool(false)),
            ("multiContract", PyValue::Bool(false)),
        ])
    }

    /// `CreateContract` answers a list of ids; the one id.
    fn create(reg: &mut Registry, who: i64, d: PyValue) -> i64 {
        let got = proxy(reg, who, "CreateContract", &[d], &[]).expect("created");
        let PyValue::List(ids) = got else {
            panic!("ShowCreatedContracts does len() and [0] on this: {got:?}")
        };
        assert_eq!(ids.len(), 1);
        con::int(ids.first())
    }

    fn accept(reg: &mut Registry, who: i64, id: i64) -> Result<PyValue, ServiceError> {
        proxy(
            reg,
            who,
            "AcceptContract",
            &[PyValue::Int(id), PyValue::Bool(false)],
            &[],
        )
    }

    /// The whole trade: Alice sells 1,000 Tritanium for 5,000 ISK, Bob finds
    /// it in a search and buys it. The goods and the money each change hands
    /// exactly once, and nothing in the world is created or lost.
    #[test]
    fn a_public_item_exchange_moves_goods_one_way_and_isk_the_other() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 1000, hangar(ALICE));
        let (isk, trit) = (world_isk(&reg), world_units(&reg, TRITANIUM));

        let id = create(
            &mut reg,
            ALICE,
            draft(0, &[(stack, 1000)], &[], 5000.0, 0.0),
        );
        assert_eq!(
            units(&reg, ALICE, TRITANIUM),
            0,
            "the goods left the hangar"
        );
        assert_eq!(
            world_units(&reg, TRITANIUM),
            trit,
            "into escrow, not nowhere"
        );

        // Bob can see it in the Search tab: `ret.contracts[i].contract`.
        let found = proxy(
            &mut reg,
            BOB,
            "SearchContracts",
            &[],
            &crate::pyutil::pypairs(vec![
                ("availability", PyValue::Int(con::AVAIL_PUBLIC)),
                (
                    "contractType",
                    PyValue::Int(con::TYPE_AUCTION_AND_ITEM_EXCHANGE),
                ),
            ]),
        )
        .unwrap();
        let Some(PyValue::List(results)) = con::attr(&found, "contracts") else {
            panic!("ret.contracts is a list: {found:?}")
        };
        assert_eq!(results.len(), 1);
        let row = con::attr(&results[0], "contract").expect(".contract");
        assert_eq!(con::attr(row, "contractID"), Some(&PyValue::Int(id)));
        assert_eq!(con::attr(&found, "numFound"), Some(&PyValue::Int(1)));

        let _ = reg.take_pending();
        let back = accept(&mut reg, BOB, id).expect("accepted");
        assert_eq!(con::attr(&back, "contractID"), Some(&PyValue::Int(id)));
        assert!(con::attr(&back, "type").is_some(), "the caller reads .type");

        assert_eq!(units(&reg, BOB, TRITANIUM), 1000);
        assert_eq!(world_units(&reg, TRITANIUM), trit);
        assert_eq!(balance(&reg, BOB), 1_000_000.0 - 5000.0);
        assert_eq!(balance(&reg, ALICE), 1_000_000.0 + 5000.0);
        assert_eq!(world_isk(&reg), isk);

        // Alice is told about her money in her own session, not Bob's.
        let pushes = reg.take_pending();
        assert!(
            pushes
                .iter()
                .any(|(_, p)| p.to_push().0 == "OnAccountChange"
                    && p.audience() == Some(vec![ALICE])),
            "the issuer's wallet push must be addressed to the issuer: {pushes:?}"
        );

        // It cannot be bought twice.
        assert!(accept(&mut reg, CAROL, id).is_err());
        assert_eq!(world_units(&reg, TRITANIUM), trit);
        assert_eq!(world_isk(&reg), isk);
    }

    /// Part of a stack: the rest stays home, and the part is what moves.
    #[test]
    fn contracting_part_of_a_stack_leaves_the_rest() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 1000, hangar(ALICE));
        let id = create(&mut reg, ALICE, draft(0, &[(stack, 300)], &[], 0.0, 0.0));
        assert_eq!(units(&reg, ALICE, TRITANIUM), 700);
        accept(&mut reg, BOB, id).unwrap();
        assert_eq!(units(&reg, BOB, TRITANIUM), 300);
        assert_eq!(world_units(&reg, TRITANIUM), 1000);
    }

    /// A private contract is the assignee's alone: hidden from a third
    /// pilot, refused to them, and announced to the assignee with
    /// `OnContractAssigned(contractID)`.
    #[test]
    fn a_private_contract_is_offered_to_its_assignee_only() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let id = create(&mut reg, ALICE, draft(BOB, &[(stack, 10)], &[], 0.0, 0.0));
        let pushes = reg.take_pending();
        assert!(
            pushes.iter().any(|(_, p)| {
                p.to_push() == ("OnContractAssigned", vec![PyValue::Int(id)])
                    && p.audience() == Some(vec![BOB])
            }),
            "{pushes:?}"
        );

        assert!(proxy(&mut reg, CAROL, "GetContract", &[PyValue::Int(id)], &[]).is_err());
        assert!(accept(&mut reg, CAROL, id).is_err());
        assert_eq!(units(&reg, CAROL, TRITANIUM), 0);

        // Bob sees it on his start page: `contract[2] == session.charid`.
        let page = proxy(&mut reg, BOB, "CollectMyPageInfo", &[], &[]).unwrap();
        let Some(PyValue::List(assigned)) = con::attr(&page, "outstandingContracts") else {
            panic!("outstandingContracts is iterated")
        };
        let PyValue::Tuple(t) = &assigned[0] else {
            panic!("read by position")
        };
        assert_eq!(t[2], PyValue::Int(BOB));
        // And at login, `kv.assignedToMe[i].contractID`.
        let login = proxy(&mut reg, BOB, "GetLoginInfo", &[], &[]).unwrap();
        let Some(PyValue::List(rows)) = con::attr(&login, "assignedToMe") else {
            panic!("assignedToMe is iterated")
        };
        assert_eq!(con::attr(&rows[0], "contractID"), Some(&PyValue::Int(id)));

        accept(&mut reg, BOB, id).unwrap();
        assert_eq!(units(&reg, BOB, TRITANIUM), 10);
    }

    /// Deleting returns everything: the goods to the hangar and the reward
    /// to the wallet.
    #[test]
    fn deleting_an_outstanding_contract_returns_its_escrow() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 50, hangar(ALICE));
        let isk = world_isk(&reg);
        let id = create(&mut reg, ALICE, draft(0, &[(stack, 50)], &[], 0.0, 2500.0));
        assert_eq!(
            balance(&reg, ALICE),
            1_000_000.0 - 2500.0,
            "the reward is held"
        );
        assert_eq!(world_isk(&reg), isk);

        // Only the issuer may.
        assert!(proxy(&mut reg, BOB, "DeleteContract", &[PyValue::Int(id)], &[]).is_err());
        let ok = proxy(&mut reg, ALICE, "DeleteContract", &[PyValue::Int(id)], &[]).unwrap();
        assert_eq!(ok, PyValue::Bool(true), "a truthy answer closes the window");
        assert_eq!(units(&reg, ALICE, TRITANIUM), 50);
        assert_eq!(balance(&reg, ALICE), 1_000_000.0);
        assert_eq!(world_isk(&reg), isk);
        assert!(
            accept(&mut reg, BOB, id).is_err(),
            "a deleted contract is gone"
        );
    }

    /// The reward goes to whoever accepts: the "want to buy" shape, where
    /// the issuer pays ISK for items they asked for.
    #[test]
    fn a_reward_for_requested_items_swaps_isk_for_goods() {
        let mut reg = world();
        let _ = make(&mut reg, PYERITE, 40, hangar(BOB));
        let isk = world_isk(&reg);
        let id = create(&mut reg, ALICE, draft(0, &[], &[(PYERITE, 25)], 0.0, 900.0));
        accept(&mut reg, BOB, id).unwrap();
        assert_eq!(units(&reg, ALICE, PYERITE), 25);
        assert_eq!(units(&reg, BOB, PYERITE), 15);
        assert_eq!(balance(&reg, BOB), 1_000_000.0 + 900.0);
        assert_eq!(balance(&reg, ALICE), 1_000_000.0 - 900.0);
        assert_eq!(world_isk(&reg), isk);
    }

    /// An acceptor short of the requested items, or of the price, is
    /// refused and **nothing** moves -- one plan, all or none.
    #[test]
    fn a_refused_accept_moves_nothing() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 100, hangar(ALICE));
        let _ = make(&mut reg, PYERITE, 5, hangar(BOB));
        let id = create(
            &mut reg,
            ALICE,
            draft(0, &[(stack, 100)], &[(PYERITE, 6)], 0.0, 0.0),
        );
        assert!(accept(&mut reg, BOB, id).is_err());
        assert_eq!(units(&reg, BOB, PYERITE), 5);
        assert_eq!(units(&reg, BOB, TRITANIUM), 0);

        let stack = make(&mut reg, TRITANIUM, 100, hangar(ALICE));
        let id = create(
            &mut reg,
            ALICE,
            draft(0, &[(stack, 100)], &[], 2_000_000.0, 0.0),
        );
        assert!(accept(&mut reg, BOB, id).is_err(), "Bob has a million");
        assert_eq!(balance(&reg, BOB), 1_000_000.0);
        assert_eq!(units(&reg, BOB, TRITANIUM), 0);
    }

    /// Items not in the issuer's hangar at that station -- someone else's,
    /// or more than the stack holds -- are refused, and nothing moves.
    #[test]
    fn only_your_own_hangar_can_be_contracted() {
        let mut reg = world();
        let bobs = make(&mut reg, TRITANIUM, 10, hangar(BOB));
        let isk = world_isk(&reg);
        assert!(
            proxy(
                &mut reg,
                ALICE,
                "CreateContract",
                &[draft(0, &[(bobs, 10)], &[], 0.0, 0.0)],
                &[]
            )
            .is_err()
        );
        let mine = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        assert!(
            proxy(
                &mut reg,
                ALICE,
                "CreateContract",
                &[draft(0, &[(mine, 11)], &[], 0.0, 0.0)],
                &[]
            )
            .is_err()
        );
        assert_eq!(units(&reg, BOB, TRITANIUM), 10);
        assert_eq!(units(&reg, ALICE, TRITANIUM), 10);
        assert_eq!(world_isk(&reg), isk);
        assert!(reg.contracts.all().is_empty());
    }

    /// An expired contract cannot be accepted; its issuer gets the goods back
    /// by deleting it, which the client offers without a confirmation for
    /// exactly that case (contracts.py `DeleteContract`).
    #[test]
    fn an_expired_contract_is_refused_and_returns_on_delete() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let id = create(&mut reg, ALICE, draft(0, &[(stack, 10)], &[], 0.0, 0.0));
        reg.contracts.get_mut(id).unwrap().date_expired = 1;
        assert!(accept(&mut reg, BOB, id).is_err());
        assert_eq!(units(&reg, BOB, TRITANIUM), 0);
        proxy(&mut reg, ALICE, "DeleteContract", &[PyValue::Int(id)], &[]).unwrap();
        assert_eq!(units(&reg, ALICE, TRITANIUM), 10);
    }

    /// A ship travels with what is in it, and all of it changes owner.
    #[test]
    fn a_ship_is_traded_with_its_cargo() {
        let mut reg = world();
        let ship = make(&mut reg, RIFTER, 1, hangar(ALICE));
        let cargo = make(
            &mut reg,
            TRITANIUM,
            7,
            crate::item::Slot::new(ALICE, ship, crate::fitting::FLAG_CARGO),
        );
        let id = create(&mut reg, ALICE, draft(0, &[(ship, 1)], &[], 0.0, 0.0));
        // The details window lists the cargo as a child of the ship.
        let d = proxy(&mut reg, BOB, "GetContract", &[PyValue::Int(id)], &[]).unwrap();
        let Some(PyValue::List(lines)) = con::attr(&d, "items") else {
            panic!(".items")
        };
        assert!(
            lines
                .iter()
                .any(|l| con::attr(l, "parentID") == Some(&PyValue::Int(ship)))
        );

        accept(&mut reg, BOB, id).unwrap();
        let s = reg.store().get(ship).unwrap();
        assert_eq!((s.owner_id, s.location_id), (BOB, STATION));
        let c = reg.store().get(cargo).unwrap();
        assert_eq!((c.owner_id, c.location_id), (BOB, ship));
    }

    /// The ship you are sitting in is not for sale.
    #[test]
    fn the_active_ship_cannot_be_contracted() {
        let mut reg = world();
        let active = reg.character(ALICE).unwrap().ship_id;
        assert!(
            proxy(
                &mut reg,
                ALICE,
                "CreateContract",
                &[draft(0, &[(active, 1)], &[], 0.0, 0.0)],
                &[]
            )
            .is_err()
        );
        assert!(reg.contracts.all().is_empty());
    }

    /// My Contracts: the envelope's `contracts` is a list and `items` a dict
    /// keyed by contract id (contractPanels.py:555 and `.get(contractID, [])`).
    #[test]
    fn my_contracts_lists_what_i_issued() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let id = create(&mut reg, ALICE, draft(0, &[(stack, 10)], &[], 1.0, 0.0));
        let page = proxy(
            &mut reg,
            ALICE,
            "GetContractListForOwner",
            &[
                PyValue::Int(ALICE),
                PyValue::Int(con::STATUS_OUTSTANDING),
                PyValue::None,
                PyValue::None,
            ],
            &crate::pyutil::pypairs(vec![("num", PyValue::Int(100))]),
        )
        .unwrap();
        let Some(PyValue::List(rows)) = con::attr(&page, "contracts") else {
            panic!("contracts is a list")
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(con::attr(&rows[0], "contractID"), Some(&PyValue::Int(id)));
        let items = con::attr(&page, "items")
            .and_then(PyValue::dict_as_kv)
            .unwrap();
        assert_eq!(items[0].0, &PyValue::Int(id));

        // Bob's own list does not have it: he is neither issuer nor assignee.
        let bobs = proxy(
            &mut reg,
            BOB,
            "GetContractListForOwner",
            &[
                PyValue::Int(BOB),
                PyValue::Int(0),
                PyValue::None,
                PyValue::None,
            ],
            &[],
        )
        .unwrap();
        assert_eq!(con::attr(&bobs, "contracts"), Some(&PyValue::List(vec![])));
    }

    /// Escrow is persisted: a world written and read back still has the
    /// contract, and it can still be accepted.
    #[test]
    fn an_outstanding_contract_survives_a_restart() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let id = create(&mut reg, ALICE, draft(0, &[(stack, 10)], &[], 0.0, 0.0));
        let snap = reg.to_snapshot();
        let mut again = world();
        again.apply_snapshot(snap);
        accept(&mut again, BOB, id).unwrap();
        assert_eq!(units(&again, BOB, TRITANIUM), 10);
    }

    // ---- couriers and auctions (W9-L6) ----------------------------------
    //
    // The same rule as above: shapes the client reads and conservation --
    // every unit and every ISK is somewhere after every step, and a refusal
    // moves nothing. Where the goods of a failed courier end up, which key a
    // refusal carries (bar the one the client itself branches on) and which
    // status a contract lands in are `[A]` and not asserted.

    const ELSEWHERE: i64 = 60_000_004;

    /// The client's key out of a refusal (`eveexceptions.UserError` args[0]).
    fn raised_key(err: &ServiceError) -> Option<String> {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = err else {
            return None;
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            return None;
        };
        let PyValue::Tuple(args) = &parts[1] else {
            return None;
        };
        crate::pyutil::as_bytes(&args[0]).map(|b| String::from_utf8_lossy(b).into_owned())
    }

    fn courier_from_alice(
        reg: &mut Registry,
        items: &[(i64, i64)],
        reward: f64,
        collateral: f64,
        destination: i64,
    ) -> i64 {
        create(
            reg,
            ALICE,
            draft_of(
                con::TYPE_COURIER,
                0,
                items,
                &[],
                [0.0, reward, collateral],
                (destination, 3),
            ),
        )
    }

    fn auction_from_alice(
        reg: &mut Registry,
        items: &[(i64, i64)],
        start: f64,
        buyout: f64,
    ) -> i64 {
        create(
            reg,
            ALICE,
            draft_of(
                con::TYPE_AUCTION,
                0,
                items,
                &[],
                [start, 0.0, buyout],
                (0, 0),
            ),
        )
    }

    fn complete(
        reg: &mut Registry,
        who: i64,
        id: i64,
        status: i64,
    ) -> Result<PyValue, ServiceError> {
        proxy(
            reg,
            who,
            "CompleteContract",
            &[PyValue::Int(id), PyValue::Int(status)],
            &[],
        )
    }

    fn bid(reg: &mut Registry, who: i64, id: i64, amount: f64) -> Result<PyValue, ServiceError> {
        proxy(
            reg,
            who,
            "PlaceBid",
            &[
                PyValue::Int(id),
                PyValue::Int(amount as i64),
                PyValue::Bool(false),
            ],
            &[],
        )
    }

    fn finish(
        reg: &mut Registry,
        who: i64,
        id: i64,
        is_issuer: bool,
    ) -> Result<PyValue, ServiceError> {
        proxy(
            reg,
            who,
            "FinishAuction",
            &[PyValue::Int(id), PyValue::Bool(is_issuer)],
            &[],
        )
    }

    /// The courier package in `who`'s hangar at `station`, if there is one.
    fn package_at(reg: &Registry, who: i64, station: i64) -> Option<crate::item::Item> {
        reg.store()
            .at(crate::item::Slot::new(
                who,
                station,
                crate::model::FLAG_HANGAR,
            ))
            .find(|i| i.type_id == con::TYPE_PLASTIC_WRAP)
            .cloned()
    }

    /// Carol is based somewhere else, which makes that a station this world
    /// can place -- a destination.
    fn world_with_a_second_station() -> Registry {
        let mut reg = world();
        let carol = reg.character_mut(CAROL).unwrap();
        carol.station_id = Some(ELSEWHERE);
        carol.solar_system_id = Some(30_000_144);
        reg
    }

    /// The whole run: Alice ships 1,000 Tritanium, Bob carries it. On
    /// accept Bob holds one package -- the client's `typePlasticWrap`, a
    /// singleton in a player-deployed-container group, which is what offers
    /// "Deliver" on it -- and Alice's goods are in nobody's hangar; on
    /// delivery they are Alice's again and Bob has his collateral back plus
    /// the reward. Nothing is created or lost at any step.
    #[test]
    fn a_courier_carries_the_goods_in_a_package_and_pays_on_delivery() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 1000, hangar(ALICE));
        let (isk, trit) = (world_isk(&reg), world_units(&reg, TRITANIUM));

        let id = courier_from_alice(&mut reg, &[(stack, 1000)], 10_000.0, 50_000.0, STATION);
        assert_eq!(units(&reg, ALICE, TRITANIUM), 0);
        assert_eq!(world_units(&reg, TRITANIUM), trit);
        assert_eq!(balance(&reg, ALICE), 1_000_000.0 - 10_000.0);
        assert_eq!(world_isk(&reg), isk);

        let row = accept(&mut reg, BOB, id).expect("accepted");
        // `AcceptContract` keeps these for the courier bookmarks.
        assert_eq!(
            con::attr(&row, "type"),
            Some(&PyValue::Int(con::TYPE_COURIER))
        );
        assert_eq!(
            con::attr(&row, "endStationID"),
            Some(&PyValue::Int(STATION))
        );
        assert_eq!(con::attr(&row, "numDays"), Some(&PyValue::Int(3)));
        assert!(matches!(
            con::attr(&row, "dateAccepted"),
            Some(PyValue::Int(_))
        ));
        assert_eq!(
            con::attr(&row, "collateral"),
            Some(&PyValue::Real(50_000.0))
        );

        let package = package_at(&reg, BOB, STATION).expect("Bob holds the package");
        assert!(
            package.singleton,
            "itemCheckers.OfferBreakContract: IsSingleton"
        );
        assert_eq!(
            package.group_id, 649,
            "in invConst.playerDeployedContainers"
        );
        assert_eq!(package.owner_id, BOB);
        assert_eq!(units(&reg, BOB, TRITANIUM), 0, "carried, not owned");
        assert_eq!(units(&reg, ALICE, TRITANIUM), 0);
        assert_eq!(world_units(&reg, TRITANIUM), trit);
        assert_eq!(balance(&reg, BOB), 1_000_000.0 - 50_000.0);
        assert_eq!(world_isk(&reg), isk);

        // The package's contract, as the Deliver menu entry asks for it.
        let info = proxy(
            &mut reg,
            BOB,
            "GetCourierContractFromItemID",
            &[PyValue::Int(package.item_id)],
            &[],
        )
        .unwrap();
        assert_eq!(con::attr(&info, "contractID"), Some(&PyValue::Int(id)));
        let none = proxy(
            &mut reg,
            BOB,
            "GetCourierContractFromItemID",
            &[PyValue::Int(stack)],
            &[],
        )
        .unwrap();
        assert_eq!(none, PyValue::None, "the client tests `is None`");

        // `NeocomBlink` keeps it as a courier bookmark.
        let login = proxy(&mut reg, BOB, "GetLoginInfo", &[], &[]).unwrap();
        let Some(PyValue::List(carrying)) = con::attr(&login, "inProgress") else {
            panic!("inProgress is iterated: {login:?}")
        };
        assert_eq!(carrying.len(), 1);
        assert_eq!(
            con::attr(&carrying[0], "contractID"),
            Some(&PyValue::Int(id))
        );

        let done = complete(&mut reg, BOB, id, con::STATUS_FINISHED).expect("delivered");
        assert_eq!(
            done,
            PyValue::Bool(true),
            "`if CompleteContract(id): Init()`"
        );
        assert_eq!(units(&reg, ALICE, TRITANIUM), 1000);
        assert_eq!(world_units(&reg, TRITANIUM), trit);
        assert_eq!(world_units(&reg, con::TYPE_PLASTIC_WRAP), 0, "unwrapped");
        assert_eq!(balance(&reg, BOB), 1_000_000.0 + 10_000.0);
        assert_eq!(balance(&reg, ALICE), 1_000_000.0 - 10_000.0);
        assert_eq!(world_isk(&reg), isk);

        // Delivered once.
        assert!(complete(&mut reg, BOB, id, con::STATUS_FINISHED).is_err());
        assert_eq!(balance(&reg, BOB), 1_000_000.0 + 10_000.0);
        assert_eq!(world_isk(&reg), isk);
    }

    /// A package is delivered where the contract says, not where it was
    /// picked up: refused at the pickup (nothing moves), accepted once the
    /// package is in the courier's hangar at the destination.
    #[test]
    fn a_courier_is_delivered_at_its_destination_only() {
        let mut reg = world_with_a_second_station();
        let stack = make(&mut reg, TRITANIUM, 40, hangar(ALICE));
        let (isk, trit) = (world_isk(&reg), world_units(&reg, TRITANIUM));
        let id = courier_from_alice(&mut reg, &[(stack, 40)], 1_000.0, 5_000.0, ELSEWHERE);
        assert_eq!(
            con::attr(&reg.contracts.get(id).unwrap().row(), "endSolarSystemID"),
            Some(&PyValue::Int(30_000_144))
        );
        accept(&mut reg, BOB, id).unwrap();
        let package = package_at(&reg, BOB, STATION).unwrap();
        let before = balance(&reg, BOB);

        assert!(complete(&mut reg, BOB, id, con::STATUS_FINISHED).is_err());
        assert_eq!(balance(&reg, BOB), before);
        assert!(package_at(&reg, BOB, STATION).is_some());
        assert_eq!(world_units(&reg, TRITANIUM), trit);

        // Bob flies it over.
        reg.apply_items(&[crate::item::Edit::Move {
            item_id: package.item_id,
            to: crate::item::Slot::new(BOB, ELSEWHERE, crate::model::FLAG_HANGAR),
        }])
        .unwrap();
        complete(&mut reg, BOB, id, con::STATUS_FINISHED).unwrap();
        let there = crate::item::Slot::new(ALICE, ELSEWHERE, crate::model::FLAG_HANGAR);
        assert_eq!(
            reg.store()
                .at(there)
                .filter(|i| i.type_id == TRITANIUM)
                .map(|i| i.quantity)
                .sum::<i64>(),
            40
        );
        assert_eq!(world_units(&reg, TRITANIUM), trit);
        assert_eq!(world_isk(&reg), isk);
    }

    /// A courier who gives up loses the collateral to the issuer, and the
    /// issuer's reward comes back; nothing is created or lost.
    #[test]
    fn a_failed_courier_pays_its_collateral_to_the_issuer() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let (isk, trit) = (world_isk(&reg), world_units(&reg, TRITANIUM));
        let id = courier_from_alice(&mut reg, &[(stack, 10)], 2_000.0, 30_000.0, STATION);
        accept(&mut reg, BOB, id).unwrap();
        complete(&mut reg, BOB, id, con::STATUS_FAILED).expect("failed");
        assert_eq!(balance(&reg, BOB), 1_000_000.0 - 30_000.0);
        assert_eq!(balance(&reg, ALICE), 1_000_000.0 + 30_000.0);
        assert_eq!(world_units(&reg, TRITANIUM), trit);
        assert_eq!(world_isk(&reg), isk);
        assert!(complete(&mut reg, BOB, id, con::STATUS_FAILED).is_err());
        assert_eq!(world_isk(&reg), isk);
    }

    /// `BreakPlasticWrap()` on the package (#57): the courier's "Break
    /// Contract" is the courier failing the contract, whose goods become the
    /// courier's where the package stands. Not the carrier's to break: Alice,
    /// the issuer, is refused and nothing moves; a package with no contract
    /// in progress is answered in words.
    #[test]
    fn breaking_the_seal_fails_the_courier_contract() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let (isk, trit) = (world_isk(&reg), world_units(&reg, TRITANIUM));
        let id = courier_from_alice(&mut reg, &[(stack, 10)], 2_000.0, 30_000.0, STATION);
        accept(&mut reg, BOB, id).unwrap();
        let package = package_at(&reg, BOB, STATION).expect("Bob holds the package");
        let bind = PyValue::Tuple(vec![PyValue::Int(package.item_id), PyValue::Int(STATION)]);
        let break_it = |reg: &mut Registry, who: i64| {
            reg.dispatch(&Request {
                service: "invbroker::inventory",
                method: "BreakPlasticWrap",
                args: &[],
                kwargs: &[],
                bind_params: Some(&bind),
                actor: Some(who),
            })
        };
        let err = break_it(&mut reg, ALICE).unwrap_err();
        assert_eq!(raised_key(&err).as_deref(), Some("ConNotYourContract"));
        assert!(package_at(&reg, BOB, STATION).is_some(), "still wrapped");

        assert_eq!(break_it(&mut reg, BOB).unwrap(), PyValue::None);
        assert!(package_at(&reg, BOB, STATION).is_none(), "unwrapped");
        assert_eq!(
            units(&reg, BOB, TRITANIUM),
            10,
            "the goods are the courier's"
        );
        assert_eq!(balance(&reg, BOB), 1_000_000.0 - 30_000.0);
        assert_eq!(balance(&reg, ALICE), 1_000_000.0 + 30_000.0);
        assert_eq!(world_units(&reg, TRITANIUM), trit);
        assert_eq!(world_isk(&reg), isk);
        assert!(complete(&mut reg, BOB, id, con::STATUS_FINISHED).is_err());
    }

    /// Overdue -- `dateAccepted + DAY * numDays` in the past -- the issuer
    /// may fail it (the details window offers them Fail then), and is paid
    /// the collateral.
    #[test]
    fn an_overdue_courier_can_be_failed_by_its_issuer() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let (isk, trit) = (world_isk(&reg), world_units(&reg, TRITANIUM));
        let id = courier_from_alice(&mut reg, &[(stack, 10)], 0.0, 25_000.0, STATION);
        accept(&mut reg, BOB, id).unwrap();
        reg.contracts.get_mut(id).unwrap().date_accepted = 1;
        complete(&mut reg, ALICE, id, con::STATUS_FAILED).expect("failed by the issuer");
        assert_eq!(balance(&reg, ALICE), 1_000_000.0 + 25_000.0);
        assert_eq!(balance(&reg, BOB), 1_000_000.0 - 25_000.0);
        assert_eq!(world_units(&reg, TRITANIUM), trit);
        assert_eq!(world_isk(&reg), isk);
    }

    /// A courier who cannot put up the collateral is refused, and nothing
    /// moves: no package, the goods still in escrow, no ISK.
    #[test]
    fn a_courier_without_the_collateral_moves_nothing() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let id = courier_from_alice(&mut reg, &[(stack, 10)], 0.0, 5_000_000.0, STATION);
        let (isk, trit) = (world_isk(&reg), world_units(&reg, TRITANIUM));
        let err = accept(&mut reg, BOB, id).unwrap_err();
        assert_eq!(raised_key(&err).as_deref(), Some("NotEnoughMoney"));
        assert!(package_at(&reg, BOB, STATION).is_none());
        assert_eq!(world_units(&reg, con::TYPE_PLASTIC_WRAP), 0);
        assert_eq!(balance(&reg, BOB), 1_000_000.0);
        assert_eq!(world_units(&reg, TRITANIUM), trit);
        assert_eq!(world_isk(&reg), isk);
    }

    /// A courier in flight survives a restart: the package, the collateral
    /// and the contract all come back, and it can still be delivered.
    #[test]
    fn a_courier_in_progress_survives_a_restart() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let id = courier_from_alice(&mut reg, &[(stack, 10)], 500.0, 1_000.0, STATION);
        accept(&mut reg, BOB, id).unwrap();
        let isk = world_isk(&reg);
        let snap = reg.to_snapshot();
        let mut again = world();
        again.apply_snapshot(snap);
        assert_eq!(world_isk(&again), isk);
        complete(&mut again, BOB, id, con::STATUS_FINISHED).unwrap();
        assert_eq!(units(&again, ALICE, TRITANIUM), 10);
        assert_eq!(world_isk(&again), isk);
    }

    /// An auction: Bob bids, Carol's low bid is refused with the key the
    /// client's dialog re-opens on, Carol outbids Bob and Bob's ISK comes
    /// back; after expiry Carol collects the goods and Alice the money.
    #[test]
    fn an_auction_holds_the_high_bid_and_settles_both_halves() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let (isk, trit) = (world_isk(&reg), world_units(&reg, TRITANIUM));
        let id = auction_from_alice(&mut reg, &[(stack, 10)], 10_000.0, 0.0);
        assert_eq!(world_units(&reg, TRITANIUM), trit);

        assert_eq!(
            bid(&mut reg, BOB, id, 10_000.0).unwrap(),
            PyValue::Bool(true)
        );
        assert_eq!(balance(&reg, BOB), 1_000_000.0 - 10_000.0);
        assert_eq!(world_isk(&reg), isk);

        // Under the client's own floor (10% over, at least 1,000).
        let low = bid(&mut reg, CAROL, id, 10_500.0).unwrap_err();
        assert_eq!(raised_key(&low).as_deref(), Some("ConBidTooLow"));
        assert_eq!(balance(&reg, CAROL), 1_000_000.0);
        assert_eq!(world_isk(&reg), isk);

        bid(&mut reg, CAROL, id, 11_000.0).unwrap();
        assert_eq!(balance(&reg, BOB), 1_000_000.0, "outbid, refunded");
        assert_eq!(balance(&reg, CAROL), 1_000_000.0 - 11_000.0);
        assert_eq!(world_isk(&reg), isk);
        let shown = reg.contracts.get(id).unwrap().details();
        let Some(PyValue::List(bids)) = con::attr(&shown, "bids") else {
            panic!("bids")
        };
        assert_eq!(con::attr(&bids[0], "bidderID"), Some(&PyValue::Int(CAROL)));

        reg.contracts.get_mut(id).unwrap().date_expired = 1;
        assert!(bid(&mut reg, BOB, id, 50_000.0).is_err(), "closed");
        assert_eq!(world_isk(&reg), isk);

        finish(&mut reg, CAROL, id, false).expect("the goods");
        assert_eq!(units(&reg, CAROL, TRITANIUM), 10);
        assert_eq!(world_units(&reg, TRITANIUM), trit);
        assert_eq!(world_isk(&reg), isk);
        finish(&mut reg, ALICE, id, true).expect("the money");
        assert_eq!(balance(&reg, ALICE), 1_000_000.0 + 11_000.0);
        assert_eq!(world_isk(&reg), isk);

        // Each half once.
        assert!(finish(&mut reg, ALICE, id, true).is_err());
        assert!(finish(&mut reg, CAROL, id, false).is_err());
        assert_eq!(units(&reg, CAROL, TRITANIUM), 10);
        assert_eq!(world_isk(&reg), isk);
    }

    /// Bidding the buyout settles there and then.
    #[test]
    fn a_buyout_settles_at_once() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 5, hangar(ALICE));
        let (isk, trit) = (world_isk(&reg), world_units(&reg, TRITANIUM));
        let id = auction_from_alice(&mut reg, &[(stack, 5)], 10_000.0, 40_000.0);
        bid(&mut reg, BOB, id, 12_000.0).unwrap();
        bid(&mut reg, CAROL, id, 40_000.0).unwrap();
        assert_eq!(units(&reg, CAROL, TRITANIUM), 5);
        assert_eq!(balance(&reg, CAROL), 1_000_000.0 - 40_000.0);
        assert_eq!(balance(&reg, ALICE), 1_000_000.0 + 40_000.0);
        assert_eq!(balance(&reg, BOB), 1_000_000.0);
        assert_eq!(world_units(&reg, TRITANIUM), trit);
        assert_eq!(world_isk(&reg), isk);
        assert!(bid(&mut reg, BOB, id, 40_000.0).is_err(), "sold");
        assert_eq!(world_isk(&reg), isk);
    }

    /// "Bid on by me" is the usual envelope, with the auction in it.
    #[test]
    fn my_bids_lists_what_i_bid_on() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 5, hangar(ALICE));
        let id = auction_from_alice(&mut reg, &[(stack, 5)], 1_000.0, 0.0);
        bid(&mut reg, BOB, id, 1_000.0).unwrap();
        let mine = proxy(&mut reg, BOB, "GetMyBids", &[PyValue::Bool(false)], &[]).unwrap();
        let Some(PyValue::List(rows)) = con::attr(&mine, "contracts") else {
            panic!("contracts is a list: {mine:?}")
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(con::attr(&rows[0], "contractID"), Some(&PyValue::Int(id)));
        assert!(con::attr(&mine, "items").is_some());
    }

    /// The wizard's split leaves two stacks of the same total.
    #[test]
    fn split_stack_makes_two_stacks_of_the_same_total() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 100, hangar(ALICE));
        let got = proxy(
            &mut reg,
            ALICE,
            "SplitStack",
            &[
                PyValue::Int(STATION),
                PyValue::Int(stack),
                PyValue::Int(30),
                PyValue::Bool(false),
                PyValue::Int(crate::model::FLAG_HANGAR),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(got, PyValue::Bool(true), "`if ret:` refreshes the wizard");
        let mut stacks: Vec<i64> = reg
            .store()
            .at(hangar(ALICE))
            .filter(|i| i.type_id == TRITANIUM)
            .map(|i| i.quantity)
            .collect();
        stacks.sort_unstable();
        assert_eq!(stacks, vec![30, 70]);
    }

    /// `Dismiss` removes the row only on a truthy answer.
    #[test]
    fn delete_notification_is_truthy() {
        let mut reg = world();
        let got = proxy(
            &mut reg,
            ALICE,
            "DeleteNotification",
            &[PyValue::Int(1), PyValue::Bool(false)],
            &[],
        )
        .unwrap();
        assert_eq!(got, PyValue::Bool(true));
    }

    // ---- the LP store (CP37b) ---------------------------------------------

    /// An invented store corporation and offer.
    const LP_CORP: i64 = 1_999_301;
    const LP_TABLE: i64 = 9_931;
    const LP_OFFER: i64 = 99_310_000;

    /// [`world`] with one store: [`LP_CORP`] sells [`LP_OFFER`], Pyerite
    /// for LP, ISK and Tritanium (invented costs).
    fn lp_world() -> Registry {
        let offer = format!("{LP_TABLE}\t{LP_OFFER}\t{PYERITE}\t5\t300\t2000\t{TRITANIUM}=10");
        let corp = format!("{LP_CORP}\t\t\t\t\t\t0\t0\t0\t\t\t{LP_TABLE}\tLPC\tStore");
        let t = crate::loyalty::Table::parse(&[offer], &[corp]).expect("parses");
        let mut c = crate::content::Content::fixture();
        c.insert("loyalty", t);
        world().with_content(std::sync::Arc::new(c))
    }

    fn lp_call(
        reg: &mut Registry,
        who: i64,
        service: &str,
        method: &str,
        args: &[PyValue],
    ) -> PyValue {
        dispatch(
            reg,
            &Request {
                service,
                method,
                args,
                kwargs: &[],
                bind_params: None,
                actor: Some(who),
            },
        )
        .expect("answers")
    }

    fn the_offer(reg: &Registry) -> crate::loyalty::Offer {
        reg.ext::<crate::loyalty::Store>()
            .offer(LP_CORP, LP_OFFER)
            .cloned()
            .expect("the offer")
    }

    fn take(reg: &mut Registry, who: i64, n: i64) -> PyValue {
        let args = [
            PyValue::Int(LP_CORP),
            PyValue::Int(LP_OFFER),
            PyValue::Int(n),
        ];
        lp_call(reg, who, "LPStoreMgr", "TakeOfferForCharacter", &args)
    }

    /// The balance list is `(corpID, balance)` pairs [V]; a new character's
    /// is empty, which caches (`{}` is not `None`).
    #[test]
    fn lp_balances_are_corp_balance_pairs() {
        let mut reg = lp_world();
        let m = "GetAllMyCharacterWalletLPBalances";
        assert_eq!(
            lp_call(&mut reg, ALICE, "LPSvc", m, &[]),
            PyValue::List(vec![])
        );
        crate::loyalty::adjust(&mut reg, ALICE, LP_CORP, 70, crate::loyalty::CHANGE_MISSION);
        assert_eq!(
            lp_call(&mut reg, ALICE, "LPSvc", m, &[]),
            PyValue::List(vec![PyValue::Tuple(vec![
                PyValue::Int(LP_CORP),
                PyValue::Int(70)
            ])])
        );
        assert_eq!(
            lp_call(&mut reg, BOB, "LPSvc", m, &[]),
            PyValue::List(vec![])
        );
    }

    /// An offer is a `KeyVal` with the 8 keys [V], `reqItems` a list of
    /// `(typeID, qty)`; a corporation with no store answers `[]`.
    #[test]
    fn an_offer_is_a_keyval_with_the_eight_keys() {
        let mut reg = lp_world();
        let m = "GetAvailableOffersFromCorp";
        let got = lp_call(&mut reg, ALICE, "LPStoreMgr", m, &[PyValue::Int(LP_CORP)]);
        let PyValue::List(offers) = got else {
            panic!("a list: {got:?}")
        };
        assert_eq!(offers.len(), 1);
        let PyValue::Instance { state, .. } = &offers[0] else {
            panic!("a KeyVal: {:?}", offers[0])
        };
        let o = the_offer(&reg);
        let get = |k: &str| crate::pyutil::dict_get(state, k.as_bytes()).cloned();
        for (k, want) in [
            ("offerID", o.offer_id),
            ("corpID", LP_CORP),
            ("typeID", o.type_id),
            ("qty", o.qty),
            ("lpCost", o.lp_cost),
            ("iskCost", o.isk_cost),
            ("akCost", 0),
        ] {
            assert_eq!(get(k), Some(PyValue::Int(want)), "{k}");
        }
        let req: Vec<PyValue> = o
            .req_items
            .iter()
            .map(|&(t, q)| PyValue::Tuple(vec![PyValue::Int(t), PyValue::Int(q)]))
            .collect();
        assert_eq!(get("reqItems"), Some(PyValue::List(req)));
        let PyValue::Dict(keys) = &**state else {
            panic!("a dict state")
        };
        assert_eq!(keys.len(), 8);
        let none = lp_call(
            &mut reg,
            ALICE,
            "LPStoreMgr",
            m,
            &[PyValue::Int(LP_CORP + 1)],
        );
        assert_eq!(none, PyValue::List(vec![]));
    }

    /// Redeeming `n` offers takes `n ×` the LP, the ISK and the required
    /// items, and creates `n × qty` of the product in the hangar.
    #[test]
    fn take_offer_debits_and_creates() {
        let mut reg = lp_world();
        let o = the_offer(&reg);
        let n = 2;
        let (req_type, req_each) = o.req_items[0];
        make(&mut reg, req_type, req_each * n + 1, hangar(ALICE));
        crate::loyalty::adjust(
            &mut reg,
            ALICE,
            LP_CORP,
            o.lp_cost * n + 7,
            crate::loyalty::CHANGE_MISSION,
        );
        let isk = balance(&reg, ALICE);
        let had = units(&reg, ALICE, o.type_id);
        let _ = reg.take_pending();

        assert_eq!(take(&mut reg, ALICE, n), PyValue::Bool(true));
        assert_eq!(crate::loyalty::balance(&reg, ALICE, LP_CORP), 7);
        assert_eq!(balance(&reg, ALICE), isk - (o.isk_cost * n) as f64);
        assert_eq!(units(&reg, ALICE, req_type), 1);
        assert_eq!(units(&reg, ALICE, o.type_id) - had, o.qty * n);
        let names: Vec<&str> = reg
            .take_pending()
            .iter()
            .map(|(_, p)| p.to_push().0)
            .collect();
        assert!(names.contains(&"OnLPChange"), "{names:?}");
        assert!(names.contains(&"OnAccountChange"), "{names:?}");
    }

    /// The LP store is a station window: `Character::station_id` stays set
    /// on undock, so a redeem from space answers falsy and moves nothing
    /// (`docs/lanes/stale-system.md`). **[A]** retail refuses it undocked.
    #[test]
    fn take_offer_needs_the_pilot_to_be_docked() {
        let mut reg = lp_world();
        let o = the_offer(&reg);
        let (req_type, req_each) = o.req_items[0];
        make(&mut reg, req_type, req_each, hangar(ALICE));
        crate::loyalty::adjust(
            &mut reg,
            ALICE,
            LP_CORP,
            o.lp_cost,
            crate::loyalty::CHANGE_MISSION,
        );
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                ALICE,
                ALICE + 1_000,
                7_002,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        assert_eq!(take(&mut reg, ALICE, 1), PyValue::Bool(false), "in space");
        assert_eq!(crate::loyalty::balance(&reg, ALICE, LP_CORP), o.lp_cost);
        reg.roster_mut().leave(ALICE);
        assert_eq!(take(&mut reg, ALICE, 1), PyValue::Bool(true), "docked");
    }

    /// Too little LP (or a missing required item) answers falsy, and
    /// nothing moves.
    #[test]
    fn too_little_lp_answers_falsy() {
        let mut reg = lp_world();
        let o = the_offer(&reg);
        let (req_type, req_each) = o.req_items[0];
        make(&mut reg, req_type, req_each, hangar(ALICE));
        crate::loyalty::adjust(
            &mut reg,
            ALICE,
            LP_CORP,
            o.lp_cost - 1,
            crate::loyalty::CHANGE_MISSION,
        );
        let isk = balance(&reg, ALICE);
        let _ = reg.take_pending();
        assert_eq!(take(&mut reg, ALICE, 1), PyValue::Bool(false));
        assert_eq!(crate::loyalty::balance(&reg, ALICE, LP_CORP), o.lp_cost - 1);
        assert_eq!(balance(&reg, ALICE), isk);
        assert_eq!(units(&reg, ALICE, req_type), req_each);
        assert!(reg.take_pending().is_empty());

        crate::loyalty::adjust(&mut reg, ALICE, LP_CORP, 1, crate::loyalty::CHANGE_MISSION);
        assert_eq!(
            take(&mut reg, ALICE, 2),
            PyValue::Bool(false),
            "two need twice the LP and the items"
        );
        assert_eq!(take(&mut reg, ALICE, 0), PyValue::Bool(false));
        assert_eq!(take(&mut reg, ALICE, 1), PyValue::Bool(true));
    }

    /// The store reads the station hangar's packaged stacks only [V]
    /// (`lpStoreSvc.py:287-296`: `List(flagHangar)`, `not item.singleton`).
    /// A requirement is met from several stacks, lowest id first; stacks in
    /// the ship, at another station or assembled count for nothing; and ISK
    /// short of `n x iskCost` refuses with nothing moved.
    #[test]
    fn required_items_come_from_the_hangars_packaged_stacks_only() {
        let mut reg = lp_world();
        let o = the_offer(&reg);
        let (req_type, each) = o.req_items[0];
        let n = 2;
        crate::loyalty::adjust(
            &mut reg,
            ALICE,
            LP_CORP,
            o.lp_cost * n,
            crate::loyalty::CHANGE_MISSION,
        );
        // Three hangar stacks that together cover `n x each` ...
        let first = make(&mut reg, req_type, each, hangar(ALICE));
        make(&mut reg, req_type, each, hangar(ALICE));
        make(&mut reg, req_type, 3, hangar(ALICE));
        // ... and stacks the store must not see.
        let elsewhere = crate::item::Slot::new(ALICE, STATION + 1, crate::model::FLAG_HANGAR);
        make(&mut reg, req_type, 1_000, elsewhere);
        let in_ship = crate::item::Slot::new(ALICE, 100_000_001, 5);
        make(&mut reg, req_type, 1_000, in_ship);
        let _ = reg.take_pending();

        assert_eq!(take(&mut reg, ALICE, n), PyValue::Bool(true));
        assert_eq!(units(&reg, ALICE, req_type), 3, "exactly 2 x each taken");
        assert!(
            reg.store().items().iter().all(|i| i.item_id != first),
            "the lowest id went first"
        );
        let names: Vec<&str> = reg
            .take_pending()
            .iter()
            .map(|(_, p)| p.to_push().0)
            .collect();
        assert!(names.contains(&"OnLPChange"), "{names:?}");

        // Only the foreign stacks remain for a second redeem: refused.
        crate::loyalty::adjust(
            &mut reg,
            ALICE,
            LP_CORP,
            o.lp_cost,
            crate::loyalty::CHANGE_MISSION,
        );
        let lp = crate::loyalty::balance(&reg, ALICE, LP_CORP);
        assert_eq!(take(&mut reg, ALICE, 1), PyValue::Bool(false));
        assert_eq!(crate::loyalty::balance(&reg, ALICE, LP_CORP), lp);
    }

    #[test]
    fn too_little_isk_refuses_and_moves_nothing() {
        let mut reg = lp_world();
        let o = the_offer(&reg);
        let (req_type, each) = o.req_items[0];
        make(&mut reg, req_type, each, hangar(ALICE));
        crate::loyalty::adjust(
            &mut reg,
            ALICE,
            LP_CORP,
            o.lp_cost,
            crate::loyalty::CHANGE_MISSION,
        );
        let drain = balance(&reg, ALICE) - (o.isk_cost - 1) as f64;
        assert!(reg.charge(ALICE, drain, 1, 0, 0).is_some());
        let isk = balance(&reg, ALICE);
        assert_eq!(take(&mut reg, ALICE, 1), PyValue::Bool(false));
        assert_eq!(balance(&reg, ALICE), isk);
        assert_eq!(units(&reg, ALICE, req_type), each);
        assert_eq!(crate::loyalty::balance(&reg, ALICE, LP_CORP), o.lp_cost);
        assert_eq!(
            take(&mut reg, ALICE, crate::loyalty::MAX_BULK_OFFERS + 1),
            PyValue::Bool(false),
            "past the bulk limit"
        );
    }

    // ---- X-GAPS: small calls a player hits ------------------------------------

    /// Any service, as `who`.
    fn via(
        reg: &mut Registry,
        who: i64,
        service: &str,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        })
    }

    /// `GM_ExpireContract` ends the contract's life now: nobody can accept
    /// it, its issuer is told to act on it (`GetLoginInfo.needsAttention`,
    /// which the client turns into its own notification) and Delete returns
    /// the escrow. An id nobody can see is the client's own refusal key.
    #[test]
    fn gm_expire_contract_expires_now_and_the_issuer_must_act() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let id = create(&mut reg, ALICE, draft(0, &[(stack, 10)], &[], 0.0, 0.0));
        let login = |reg: &mut Registry| {
            let got = proxy(reg, ALICE, "GetLoginInfo", &[], &[]).unwrap();
            let Some(PyValue::List(rows)) = con::attr(&got, "needsAttention") else {
                panic!("needsAttention is a list")
            };
            rows.clone()
        };
        assert!(login(&mut reg).is_empty(), "a live contract needs nothing");

        let got = proxy(&mut reg, BOB, "GM_ExpireContract", &[PyValue::Int(id)], &[]).unwrap();
        assert_eq!(got, PyValue::None, "the caller drops the answer");
        assert!(
            reg.contracts
                .get(id)
                .unwrap()
                .is_expired(crate::pyutil::filetime_now())
        );
        assert!(accept(&mut reg, BOB, id).is_err(), "expired, so closed");

        // The row the client reads `[1]` (isForCorp) and `.contractID` of.
        let rows = login(&mut reg);
        assert_eq!(rows.len(), 1);
        let PyValue::DbRow(row) = &rows[0] else {
            panic!("a DBRow answers both [1] and .contractID: {:?}", rows[0])
        };
        assert_eq!(
            row.descriptor()
                .columns()
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>(),
            vec![b"contractID".to_vec(), b"isForCorp".to_vec()]
        );
        assert_eq!(row.values()[0], DbValue::Int(id));
        assert_eq!(row.values()[1], DbValue::Int(0));

        proxy(&mut reg, ALICE, "DeleteContract", &[PyValue::Int(id)], &[]).unwrap();
        assert_eq!(units(&reg, ALICE, TRITANIUM), 10);
        assert!(login(&mut reg).is_empty(), "deleted, so nothing is left");

        let gone = proxy(
            &mut reg,
            BOB,
            "GM_ExpireContract",
            &[PyValue::Int(999)],
            &[],
        )
        .unwrap_err();
        assert_eq!(raised_key(&gone).as_deref(), Some("ConContractNotFound"));
    }

    /// `ForgetAuctionContract`'s call is answered and changes nothing.
    #[test]
    fn delete_contract_notification_is_answered() {
        let mut reg = world();
        let got = proxy(
            &mut reg,
            ALICE,
            "DeleteContractNotification",
            &[PyValue::Int(1), PyValue::Bool(false)],
            &[],
        )
        .unwrap();
        assert_eq!(got, PyValue::None);
    }

    /// `corpmgr.GetCorporationIDForCharacter(charID)`, what the contract
    /// wizard asks for a private contract's assignee (createContractWnd.py
    /// :1539): an int, the character's corporation, the NPC corporation for
    /// a character the world does not know.
    #[test]
    fn the_wizard_asks_a_character_for_their_corporation() {
        let mut reg = world();
        reg.character_mut(BOB).unwrap().corporation_id = 98_000_777;
        let got = via(
            &mut reg,
            ALICE,
            "corpmgr",
            "GetCorporationIDForCharacter",
            &[PyValue::Int(BOB)],
        )
        .unwrap();
        assert_eq!(got, PyValue::Int(98_000_777));
        let stranger = via(
            &mut reg,
            ALICE,
            "corpmgr",
            "GetCorporationIDForCharacter",
            &[PyValue::Int(1)],
        )
        .unwrap();
        assert_eq!(stranger, PyValue::Int(crate::model::NPC_CORPORATION_ID));
    }

    /// `invbroker.SplitStack(stationID, itemID, qty, ownerID)`: the new
    /// stack sits beside the old one and nothing is made or lost.
    #[test]
    fn invbroker_split_stack_splits_a_hangar_stack() {
        let mut reg = world();
        let stack = make(&mut reg, TRITANIUM, 100, hangar(ALICE));
        let args = |item: i64, qty: i64| {
            [
                PyValue::Int(STATION),
                PyValue::Int(item),
                PyValue::Int(qty),
                PyValue::Int(ALICE),
            ]
        };
        let got = via(&mut reg, ALICE, "invbroker", "SplitStack", &args(stack, 30)).unwrap();
        assert_eq!(got, PyValue::None, "the caller drops the answer");
        let mut stacks: Vec<i64> = reg
            .store()
            .at(hangar(ALICE))
            .filter(|i| i.type_id == TRITANIUM)
            .map(|i| i.quantity)
            .collect();
        stacks.sort_unstable();
        assert_eq!(stacks, vec![30, 70]);

        // Somebody else's stack, an unknown id and a quantity that takes
        // everything are refused, and nothing moves.
        let theirs = make(&mut reg, TRITANIUM, 10, hangar(BOB));
        let refused =
            via(&mut reg, ALICE, "invbroker", "SplitStack", &args(theirs, 3)).unwrap_err();
        assert_eq!(
            raised_key(&refused).as_deref(),
            Some("CannotSplitStackNotInHangar")
        );
        assert!(
            via(
                &mut reg,
                ALICE,
                "invbroker",
                "SplitStack",
                &args(999_999, 3)
            )
            .is_err()
        );
        assert!(via(&mut reg, ALICE, "invbroker", "SplitStack", &args(stack, 70)).is_err());
        assert_eq!(units(&reg, ALICE, TRITANIUM), 100);
        assert_eq!(units(&reg, BOB, TRITANIUM), 10);
    }

    /// `invbroker.TrashItems(itemIDs, locationID)` destroys the caller's
    /// hangar items and answers `[]`; the boarded ship and other people's
    /// items stay.
    #[test]
    fn invbroker_trash_items_destroys_the_callers_hangar_items() {
        let mut reg = world();
        let mine = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let theirs = make(&mut reg, TRITANIUM, 10, hangar(BOB));
        let boarded = reg.character(ALICE).unwrap().ship_id;
        let ids = PyValue::List(vec![
            PyValue::Int(mine),
            PyValue::Int(theirs),
            PyValue::Int(boarded),
        ]);
        let got = via(
            &mut reg,
            ALICE,
            "invbroker",
            "TrashItems",
            &[ids.clone(), PyValue::Int(STATION)],
        )
        .unwrap();
        assert_eq!(got, PyValue::List(vec![]), "no errors");
        assert!(reg.store().get(mine).is_none());
        assert!(reg.store().get(theirs).is_some(), "not Alice's");
        assert!(reg.store().get(boarded).is_some(), "she is sitting in it");
        // Nothing left that can go: a refusal, not silence.
        assert!(
            via(
                &mut reg,
                ALICE,
                "invbroker",
                "TrashItems",
                &[ids, PyValue::Int(STATION)],
            )
            .is_err()
        );
    }

    /// The global container's `ListStationItems(stationID)` and
    /// `InvalidateStationItemsCache(stationID)`.
    #[test]
    fn the_global_container_lists_one_stations_items() {
        let mut reg = world();
        let mine = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let elsewhere = make(
            &mut reg,
            PYERITE,
            5,
            crate::item::Slot::new(ALICE, 60_000_001, crate::model::FLAG_HANGAR),
        );
        let theirs = make(&mut reg, TRITANIUM, 10, hangar(BOB));
        let got = via(
            &mut reg,
            ALICE,
            "charMgr",
            "ListStationItems",
            &[PyValue::Int(STATION)],
        )
        .unwrap();
        let PyValue::List(rows) = got else {
            panic!("a list of item rows")
        };
        let ids: Vec<i64> = rows
            .iter()
            .filter_map(|r| match r {
                PyValue::DbRow(row) => match row.values()[0] {
                    DbValue::Int(id) => Some(id),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert!(ids.contains(&mine));
        assert!(!ids.contains(&elsewhere), "another station");
        assert!(!ids.contains(&theirs), "another owner");
        assert_eq!(
            via(
                &mut reg,
                ALICE,
                "charMgr",
                "InvalidateStationItemsCache",
                &[PyValue::Int(STATION)],
            )
            .unwrap(),
            PyValue::None
        );
    }

    // ---- B8: world stations, corp / alliance assignment, search --------------

    /// A station row for the world's table: invented ids only.
    fn station_row(
        id: i64,
        system: i64,
        constellation: i64,
        region: i64,
    ) -> crate::clones::StationRef {
        crate::clones::StationRef {
            station_id: id,
            owner_id: 1,
            type_id: 1,
            operation_id: 1,
            solar_system_id: system,
            constellation_id: constellation,
            region_id: region,
        }
    }

    /// Stations 7 (system 3) and 8 (system 4) in the table; every pilot is
    /// based at `STATION`, which is not in it.
    fn table_world() -> Registry {
        let mut reg = world();
        reg.stations.insert(7, station_row(7, 3, 2, 1));
        reg.stations.insert(8, station_row(8, 4, 2, 1));
        reg
    }

    fn hangar_at(who: i64, station: i64) -> crate::item::Slot {
        crate::item::Slot::new(who, station, crate::model::FLAG_HANGAR)
    }

    #[test]
    fn a_courier_resolves_a_station_nobody_is_docked_at() {
        let mut reg = table_world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar_at(ALICE, 7));
        let id = create(
            &mut reg,
            ALICE,
            draft_at(
                7,
                con::TYPE_COURIER,
                0,
                &[(stack, 10)],
                &[],
                [0.0, 100.0, 500.0],
                (8, 3),
            ),
        );
        let c = reg.contracts.get(id).unwrap();
        assert_eq!((c.start_system_id, c.start_region_id), (3, 1));
        assert_eq!(
            (c.end_system_id, c.end_constellation_id, c.end_region_id),
            (4, 2, 1)
        );
        // The same station is refused where the table knows it.
        let stack = make(&mut reg, TRITANIUM, 10, hangar_at(ALICE, 7));
        let err = proxy(
            &mut reg,
            ALICE,
            "CreateContract",
            &[draft_at(
                7,
                con::TYPE_COURIER,
                0,
                &[(stack, 10)],
                &[],
                [0.0, 100.0, 500.0],
                (7, 3),
            )],
            &[],
        )
        .unwrap_err();
        assert_eq!(raised_key(&err).as_deref(), Some("ConDestinationSame"));
        // An item exchange at 8 has 8's system.
        let stack = make(&mut reg, PYERITE, 10, hangar_at(ALICE, 8));
        let id = create(
            &mut reg,
            ALICE,
            draft_at(
                8,
                con::TYPE_ITEM_EXCHANGE,
                0,
                &[(stack, 10)],
                &[],
                [5.0, 0.0, 0.0],
                (0, 0),
            ),
        );
        assert_eq!(reg.contracts.get(id).unwrap().start_system_id, 4);
    }

    #[test]
    fn a_start_station_the_world_cannot_place_is_refused() {
        let mut reg = table_world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar_at(ALICE, 9));
        let err = proxy(
            &mut reg,
            ALICE,
            "CreateContract",
            &[draft_at(
                9,
                con::TYPE_ITEM_EXCHANGE,
                0,
                &[(stack, 10)],
                &[],
                [5.0, 0.0, 0.0],
                (0, 0),
            )],
            &[],
        )
        .unwrap_err();
        assert_eq!(raised_key(&err).as_deref(), Some("ConNotInStation"));
        assert!(reg.contracts.all().is_empty());
    }

    /// Alice (11) and Bob (12) in corp 21, Carol (13) in corp 22; alliance 31
    /// holds corp 21. Characters keep the test ids, corps are small ids.
    fn corp_world() -> Registry {
        let mut reg = world();
        reg.corporations.enrol_for_test(ALICE, 21, 0);
        reg.corporations.enrol_for_test(BOB, 21, 0);
        reg.corporations.enrol_for_test(CAROL, 22, 0);
        reg.deltas_mut().put(
            crate::alliance::KIND_MEMBER,
            vec!["21".to_owned()],
            vec!["31".to_owned(), "0".to_owned(), "-".to_owned()],
        );
        reg
    }

    fn search_kw(reg: &mut Registry, who: i64, kw: Vec<(&str, PyValue)>) -> Vec<i64> {
        let found = proxy(
            reg,
            who,
            "SearchContracts",
            &[],
            &crate::pyutil::pypairs(kw),
        )
        .unwrap();
        let Some(PyValue::List(rows)) = con::attr(&found, "contracts") else {
            panic!("ret.contracts is a list")
        };
        rows.iter()
            .map(|r| con::int(con::attr(con::attr(r, "contract").unwrap(), "contractID")))
            .collect()
    }

    fn rejected(reg: &mut Registry, who: i64, id: i64) -> Result<PyValue, ServiceError> {
        proxy(
            reg,
            who,
            "CompleteContract",
            &[PyValue::Int(id), PyValue::Int(con::STATUS_REJECTED)],
            &[],
        )
    }

    #[test]
    fn a_contract_can_be_assigned_to_a_corporation() {
        let mut reg = corp_world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let _ = reg.take_pending();
        let id = create(&mut reg, ALICE, draft(21, &[(stack, 10)], &[], 0.0, 0.0));
        assert_eq!(reg.contracts.get(id).unwrap().availability, con::AVAIL_CORP);
        // Bob, a member, is told; Alice, the issuer, is not.
        let pushes = reg.take_pending();
        let told = |who: i64| {
            pushes.iter().any(|(_, p)| {
                p.to_push().0 == "OnContractAssigned" && p.audience() == Some(vec![who])
            })
        };
        assert!(told(BOB) && !told(ALICE) && !told(CAROL));
        // Bob finds it under "My Corporation"; Carol's corp does not.
        assert_eq!(
            search_kw(
                &mut reg,
                BOB,
                vec![("availability", PyValue::Int(con::AVAIL_CORP))]
            ),
            vec![id]
        );
        assert!(
            search_kw(
                &mut reg,
                CAROL,
                vec![("availability", PyValue::Int(con::AVAIL_CORP))]
            )
            .is_empty()
        );
        assert!(
            search_kw(
                &mut reg,
                BOB,
                vec![("availability", PyValue::Int(con::AVAIL_PUBLIC))]
            )
            .is_empty()
        );
        let row = reg.contracts.get(id).unwrap().row();
        assert_eq!(
            con::attr(&row, "availability"),
            Some(&PyValue::Int(con::AVAIL_CORP))
        );
        let err = accept(&mut reg, CAROL, id).unwrap_err();
        assert_eq!(raised_key(&err).as_deref(), Some("ConContractNotFound"));
        accept(&mut reg, BOB, id).expect("a member accepts");
        assert_eq!(units(&reg, BOB, TRITANIUM), 10);
    }

    #[test]
    fn a_contract_can_be_assigned_to_an_alliance() {
        let mut reg = corp_world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let id = create(&mut reg, ALICE, draft(31, &[(stack, 10)], &[], 0.0, 0.0));
        assert_eq!(
            reg.contracts.get(id).unwrap().availability,
            con::AVAIL_ALLIANCE
        );
        assert_eq!(
            search_kw(
                &mut reg,
                BOB,
                vec![("availability", PyValue::Int(con::AVAIL_ALLIANCE))]
            ),
            vec![id]
        );
        assert!(
            search_kw(
                &mut reg,
                BOB,
                vec![("availability", PyValue::Int(con::AVAIL_CORP))]
            )
            .is_empty()
        );
        assert!(
            search_kw(
                &mut reg,
                CAROL,
                vec![("availability", PyValue::Int(con::AVAIL_ALLIANCE))]
            )
            .is_empty()
        );
    }

    #[test]
    fn only_your_own_corporation_or_alliance_can_be_assigned() {
        let mut reg = corp_world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        // Corp 22 is Carol's, not Alice's.
        assert!(
            proxy(
                &mut reg,
                ALICE,
                "CreateContract",
                &[draft(22, &[(stack, 10)], &[], 0.0, 0.0)],
                &[]
            )
            .is_err()
        );
        assert!(reg.contracts.all().is_empty());
    }

    #[test]
    fn rejecting_a_corporation_contract_needs_the_contract_manager_role() {
        let mut reg = corp_world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let id = create(&mut reg, ALICE, draft(21, &[(stack, 10)], &[], 0.0, 0.0));
        let err = rejected(&mut reg, BOB, id).unwrap_err();
        assert_eq!(raised_key(&err).as_deref(), Some("ConNotYourContract"));
        reg.corporations
            .enrol_for_test(BOB, 21, con::ROLE_CONTRACT_MANAGER);
        let _ = reg.take_pending();
        rejected(&mut reg, BOB, id).expect("with the role");
        assert_eq!(reg.contracts.get(id).unwrap().status, con::STATUS_REJECTED);
        let pushes = reg.take_pending();
        assert!(pushes.iter().any(
            |(_, p)| p.to_push().0 == "OnContractRejected" && p.audience() == Some(vec![ALICE])
        ));
    }

    #[test]
    fn an_assignment_survives_the_world_file() {
        let mut reg = corp_world();
        let stack = make(&mut reg, TRITANIUM, 10, hangar(ALICE));
        let id = create(&mut reg, ALICE, draft(21, &[(stack, 10)], &[], 0.0, 0.0));
        let mut text = String::new();
        reg.contracts.render(&mut text);
        let mut back = con::Contracts::default();
        for line in text.lines() {
            back.parse_record(&line.split('\t').collect::<Vec<_>>())
                .unwrap();
        }
        assert_eq!(back.get(id).unwrap().availability, con::AVAIL_CORP);
        // A line from before the field: a character assignment.
        let old: Vec<&str> = text
            .lines()
            .filter(|l| l.starts_with("contract\t"))
            .collect();
        let cut = old[0].rsplit_once('\t').unwrap().0;
        let mut older = con::Contracts::default();
        older
            .parse_record(&cut.split('\t').collect::<Vec<_>>())
            .unwrap();
        assert_eq!(older.get(id).unwrap().availability, con::AVAIL_MYSELF);
    }

    /// Public item exchanges at `STATION`, one per price.
    fn priced(reg: &mut Registry, prices: &[f64]) -> Vec<i64> {
        prices
            .iter()
            .map(|p| {
                let stack = make(reg, TRITANIUM, 5, hangar(ALICE));
                create(reg, ALICE, draft(0, &[(stack, 5)], &[], *p, 0.0))
            })
            .collect()
    }

    #[test]
    fn search_sorts_by_the_asked_column() {
        let mut reg = world();
        let ids = priced(&mut reg, &[30.0, 10.0, 20.0]);
        let public = |sort: Vec<(&'static str, PyValue)>| {
            let mut kw = vec![("availability", PyValue::Int(con::AVAIL_PUBLIC))];
            kw.extend(sort);
            kw
        };
        let cheapest = search_kw(
            &mut reg,
            BOB,
            public(vec![
                ("sortBy", PyValue::Int(con::SORT_PRICE)),
                ("sortDir", PyValue::Int(0)),
            ]),
        );
        assert_eq!(cheapest, vec![ids[1], ids[2], ids[0]]);
        let dearest = search_kw(
            &mut reg,
            BOB,
            public(vec![
                ("sortBy", PyValue::Int(con::SORT_PRICE)),
                ("sortDir", PyValue::Int(1)),
            ]),
        );
        assert_eq!(dearest, vec![ids[0], ids[2], ids[1]]);
        // Oldest and newest first by id.
        let oldest = search_kw(
            &mut reg,
            BOB,
            public(vec![
                ("sortBy", PyValue::Int(0)),
                ("sortDir", PyValue::Int(0)),
            ]),
        );
        assert_eq!(oldest, ids);
        // No sort: newest first, as before.
        let plain = search_kw(&mut reg, BOB, public(vec![]));
        assert_eq!(plain, vec![ids[2], ids[1], ids[0]]);
    }

    #[test]
    fn search_filters_couriers_by_reward_and_destination() {
        let mut reg = table_world();
        let mk = |reg: &mut Registry, reward: f64, to: i64| {
            let stack = make(reg, TRITANIUM, 10, hangar_at(ALICE, 7));
            create(
                reg,
                ALICE,
                draft_at(
                    7,
                    con::TYPE_COURIER,
                    0,
                    &[(stack, 10)],
                    &[],
                    [0.0, reward, 100.0],
                    (to, 3),
                ),
            )
        };
        reg.stations.insert(9, station_row(9, 5, 2, 1));
        let cheap = mk(&mut reg, 1.0, 8);
        let dear = mk(&mut reg, 9.0, 9);
        let public = || ("availability", PyValue::Int(con::AVAIL_PUBLIC));
        let courier = || ("contractType", PyValue::Int(con::TYPE_COURIER));
        assert_eq!(
            search_kw(
                &mut reg,
                BOB,
                vec![public(), courier(), ("minReward", PyValue::Real(5.0))]
            ),
            vec![dear]
        );
        assert_eq!(
            search_kw(
                &mut reg,
                BOB,
                vec![public(), courier(), ("endLocationID", PyValue::Int(4))]
            ),
            vec![cheap]
        );
        assert_eq!(
            search_kw(
                &mut reg,
                BOB,
                vec![public(), courier(), ("maxCollateral", PyValue::Real(50.0))]
            ),
            Vec::<i64>::new()
        );
    }
}
