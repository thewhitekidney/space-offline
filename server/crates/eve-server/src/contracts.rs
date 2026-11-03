//! Contracts: item exchange, courier and auction between players (packet
//! P5.8 and W9-L6, `docs/lanes/contracts.md`).
//!
//! This module is the **model**: what a contract is, how it is kept on disk,
//! how the client's `ContractStruct` is read, and every shape the client reads
//! back. The `contractProxy` arms that move ISK and items live in
//! `service/wallet.rs`, because only a `service` submodule can reach the
//! journal and the world's private fields -- the split `MarketBook` and
//! `service/market.rs` already use.
//!
//! # What exists
//!
//! **Item exchange.** An issuer puts items from a station hangar into
//! escrow, names a price the acceptor pays and/or a reward the issuer pays,
//! and may ask for items in return. The contract is public or private to one
//! named character. Accepting moves everything in one item plan, so either
//! all of it happens or none does. Deleting an outstanding contract -- which is
//! also how an expired one is cleared -- gives the escrow back.
//!
//! **Courier.** The issuer's goods go into escrow and the reward leaves the
//! issuer's wallet, as for an item exchange. Accepting holds the
//! [`Contract::collateral`] from the acceptor and hands them **one
//! package** -- a `typePlasticWrap` item ([`TYPE_PLASTIC_WRAP`]) in their
//! hangar at the start station, with the goods inside it still owned by
//! escrow. Delivering it (`CompleteContract(id, conStatusFinished)`, which is
//! what both the details window's Complete button and the package's "Deliver"
//! menu entry send) at the destination station unpacks the goods into the
//! issuer's hangar there and pays the courier the reward and the collateral
//! back. Failing it (`CompleteContract(id, conStatusFailed)`: the courier at
//! any time, the issuer once it is overdue) pays the collateral and the
//! reward to the issuer and leaves the goods with the courier.
//!
//! **Auction.** `price` is the starting bid and `collateral` the buyout
//! (`contracts.PlaceBid` reads them that way, **[V]**). A bid holds its ISK;
//! being outbid refunds it. A buyout settles at once. Otherwise, once the
//! auction has expired, the winner and the issuer each claim their half
//! (`FinishAuction(id, isIssuer)`, the details window's Get Items / Get Money
//! buttons, **[V]**) -- so no clock is needed: the client's own model is that
//! settlement happens when someone asks for it.
//!
//! Loan contracts and corporation contracts are refused with an honest
//! `CustomNotify`.
//!
//! # Where the shapes come from
//!
//! All `[V]` reads below are from the V24.01 client, decompiled:
//!
//! - `contracts.py` (`svc.contracts`), the service every window calls.
//! - `contractsDetailsWnd.py`, the details window: the one reader of nearly
//!   every contract attribute.
//! - `contractPanels.py`, the My Contracts tab.
//! - `contractsearch.py`, the Search tab.
//! - `createContractWnd.py` and `contracts/contractStruct.py`, the wizard.
//!
//! Line numbers in `file:line` form are bytecode line numbers from
//! `tools/dis.py`, which the decompiled text does not carry; where only a
//! function is named, the read is in that function.

use crate::item::{Slot, escrow_location};
use crate::pyutil::keyval;
use eve_marshal::PyValue;

// ---- client constants, `eve/common/lib/appConst.py` [V] --------------------

/// `conTypeItemExchange`.
pub const TYPE_ITEM_EXCHANGE: i64 = 1;
/// `conTypeAuction`.
pub const TYPE_AUCTION: i64 = 2;
/// `conTypeCourier`.
pub const TYPE_COURIER: i64 = 3;
/// `contractscommon.CONTYPE_AUCTIONANDITEMECHANGE`: the search tab's and the
/// My Contracts filter's "auction or item exchange".
pub const TYPE_AUCTION_AND_ITEM_EXCHANGE: i64 = 10;
/// `contractscommon.CONTYPE_ALL`.
pub const TYPE_ALL: i64 = 11;

/// `conStatusOutstanding`.
pub const STATUS_OUTSTANDING: i64 = 0;
/// `conStatusInProgress`.
pub const STATUS_IN_PROGRESS: i64 = 1;
/// `conStatusFinishedIssuer`: an auction whose issuer has collected the
/// money but whose winner has not yet collected the items.
pub const STATUS_FINISHED_ISSUER: i64 = 2;
/// `conStatusFinishedContractor`: the other half of that.
pub const STATUS_FINISHED_CONTRACTOR: i64 = 3;
/// `conStatusFinished`.
pub const STATUS_FINISHED: i64 = 4;
/// `conStatusRejected`.
pub const STATUS_REJECTED: i64 = 6;
/// `conStatusFailed`.
pub const STATUS_FAILED: i64 = 7;
/// `conStatusDeleted`.
pub const STATUS_DELETED: i64 = 8;

/// `conAvailPublic`.
pub const AVAIL_PUBLIC: i64 = 0;
/// `conAvailMyself`.
pub const AVAIL_MYSELF: i64 = 1;
/// `conAvailMyCorp` (appConst disassembly, **[V]**).
pub const AVAIL_CORP: i64 = 2;
/// `conAvailMyAlliance`.
pub const AVAIL_ALLIANCE: i64 = 3;

/// `SORT_*` (`contractscommon`, **[V]**): the search's `sortBy`.
pub const SORT_PRICE: i64 = 1;
pub const SORT_EXPIRED: i64 = 2;
pub const SORT_STATION_ID: i64 = 3;
pub const SORT_SOLARSYSTEM_ID: i64 = 4;
pub const SORT_REGION_ID: i64 = 5;
pub const SORT_CONSTELLATION_ID: i64 = 6;
pub const SORT_CONTRACT_TYPE: i64 = 7;
pub const SORT_REWARD: i64 = 8;
pub const SORT_COLLATERAL: i64 = 9;
pub const SORT_VOLUME: i64 = 10;
pub const SORT_ASSIGNEEID: i64 = 11;

/// `corpRoleContractManager` (1 << 56): who may reject a contract assigned
/// to the corporation or its alliance. **[A]** EVE knowledge, falsifier: a
/// retail member without the role rejecting one.
pub const ROLE_CONTRACT_MANAGER: i64 = 1 << 56;

/// Who is asking: the character, their player corporation (0 in an NPC
/// one) and their alliance (0 for none). What a corp or alliance assignment
/// is matched against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Who {
    pub me: i64,
    pub corp: i64,
    pub alliance: i64,
}

/// `refContractRewardRefund`, `refContractReward`, `refContractPrice`,
/// `refContractRewardAdded`. **[V]** ids; which of them the real server books
/// for which leg is **[A]**: the price both ways as `refContractPrice`, the
/// reward into escrow as `refContractRewardAdded`, out of it as
/// `refContractReward`, back as `refContractRewardRefund`.
pub const REF_REWARD_REFUND: i64 = 66;
pub const REF_REWARD: i64 = 68;
pub const REF_PRICE: i64 = 71;
pub const REF_REWARD_ADDED: i64 = 83;
/// `refContractAuctionBid`, `refContractAuctionBidRefund`,
/// `refContractCollateral`, `refContractAuctionSold`,
/// `refContractCollateralRefund`, `refContractCollateralPayout` (**[V]** ids,
/// `appConst`). Which leg books which is **[A]**: a bid held as
/// `AuctionBid` and returned as `AuctionBidRefund`, the winning bid paid to
/// the issuer as `AuctionSold`; collateral held as `Collateral`, returned to
/// the courier as `CollateralRefund`, paid to the issuer on a failure as
/// `CollateralPayout`.
pub const REF_AUCTION_BID: i64 = 63;
pub const REF_AUCTION_BID_REFUND: i64 = 64;
pub const REF_COLLATERAL: i64 = 65;
pub const REF_AUCTION_SOLD: i64 = 67;
pub const REF_COLLATERAL_REFUND: i64 = 69;
pub const REF_COLLATERAL_PAYOUT: i64 = 70;

/// `contractscommon.MAX_AMOUNT` **[V]**: the most ISK a contract may name.
pub const MAX_AMOUNT: f64 = 10_000_000_000_000.0;
/// `contractscommon.EXPIRE_TIMES`' largest entry, 28 days in minutes **[V]**.
pub const MAX_EXPIRE_MINUTES: i64 = 28 * 1440;
/// `contractscommon.CONTRACTS_PER_PAGE` **[V]**: the search page size, and
/// `startNum = page * CONTRACTS_PER_PAGE` (contractsearch.py `SearchContracts`).
pub const CONTRACTS_PER_PAGE: usize = 100;
/// `contractscommon.MAX_CONTRACTS_PER_SEARCH` **[V]**, sent back as
/// `maxResults` -- the client prints `"%s+"` once `numFound` reaches it.
pub const MAX_CONTRACTS_PER_SEARCH: i64 = 1000;
/// `contractscommon.MAX_NUM_ITEMS` **[V]**: items one contract may carry.
pub const MAX_NUM_ITEMS: usize = 500;

/// `contractscommon.const_conCourierMaxVolume` **[V]**: the most m3 one
/// courier package may hold. The client checks it in the wizard
/// (`createContractWnd.py:1734` raises `ConExceedsMaxVolume`); so do we.
pub const COURIER_MAX_VOLUME: f64 = 1_200_000.0;

/// `typePlasticWrap` (`inventorycommon/const.pyc`, **[V]**): the courier
/// package. `crate::item::COURIER_TYPES` carries its row.
pub const TYPE_PLASTIC_WRAP: i64 = 3468;

/// One minute of blue time (`const.MIN`): 100 ns ticks.
pub const MINUTE: i64 = 600_000_000;
/// One day (`const.DAY`). A courier is overdue at
/// `dateAccepted + DAY * numDays` (`contractsDetailsWnd.Init` and
/// `contracts.FailContract`, **[V]**).
pub const DAY: i64 = 1440 * MINUTE;

/// The owner of everything in contract escrow: owner 1, "the system", the
/// same owner the market's escrow uses (`service/market.rs`). Not a
/// character, so `Registry::apply_items` tells nobody about it.
pub const ESCROW_OWNER: i64 = 1;

/// The first contract id this server hands out. **[A]**: any value works for
/// the client, which only ever compares and links them; it must stay below
/// the market's escrow ticket base (10^12), because the escrow location of a
/// contract is `item::escrow_location(contract_id)` and the market's tickets
/// start there.
pub const FIRST_CONTRACT_ID: i64 = 200_000_000;

// ---- the model ---------------------------------------------------------------

/// One line of a contract: an item the issuer gives (`in_crate`) or a type
/// the issuer asks for (`!in_crate`).
///
/// `inCrate` is the client's own split (`contractsDetailsWnd.InsertItemList`
/// and `contractscommon.GetContractTitle`, **[V]**): `True` lines are what the
/// acceptor gets, `False` lines what the acceptor pays.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// The escrowed item, or `None` for a requested type.
    pub item_id: Option<i64>,
    pub type_id: i64,
    pub quantity: i64,
    pub in_crate: bool,
    /// The item this one sits in (a ship's fitting, a container's cargo), or
    /// 0 for a top-level item. The client groups by it.
    pub parent_id: i64,
    pub flag_id: i64,
}

/// One bid on an auction. `contract.bids` is read **highest first**:
/// `bids[0].amount` is the current bid and `bids[0].bidderID` the high bidder
/// (`contracts.PlaceBid`, `contractsDetailsWnd.Init`, **[V]**).
#[derive(Debug, Clone, PartialEq)]
pub struct Bid {
    pub bidder_id: i64,
    pub amount: f64,
    pub date: i64,
}

/// One contract.
#[derive(Debug, Clone, PartialEq)]
pub struct Contract {
    pub contract_id: i64,
    /// `conType*`.
    pub kind: i64,
    pub issuer_id: i64,
    pub issuer_corp_id: i64,
    /// 0 for a public contract, else the one character, corporation or
    /// alliance (per `availability`) who may accept.
    pub assignee_id: i64,
    /// `conAvail*`: who `assignee_id` is. Persisted as a trailing field; a
    /// contract saved before it is a character assignment when it has an
    /// assignee.
    pub availability: i64,
    /// 0 until accepted.
    pub acceptor_id: i64,
    pub start_station_id: i64,
    pub start_system_id: i64,
    pub start_constellation_id: i64,
    pub start_region_id: i64,
    /// A courier's destination. An item exchange or an auction has none and
    /// repeats the start here, **[A]** -- see [`Contract::row`].
    pub end_station_id: i64,
    pub end_system_id: i64,
    pub end_constellation_id: i64,
    pub end_region_id: i64,
    /// ISK the acceptor pays the issuer. An auction's starting bid.
    pub price: f64,
    /// ISK the issuer pays the acceptor. Held in escrow while outstanding.
    pub reward: f64,
    /// A courier's collateral (held from the courier while in progress), or
    /// an auction's buyout price (0 for none) -- the client reads one
    /// attribute both ways (`contracts.PlaceBid`, **[V]**).
    pub collateral: f64,
    /// The courier package minted on accept, or 0.
    pub crate_id: i64,
    /// An auction's bids, highest first.
    pub bids: Vec<Bid>,
    /// m3 of the escrowed items.
    pub volume: f64,
    pub date_issued: i64,
    pub date_expired: i64,
    /// 0 until accepted. **Never `None` on the wire**: the details window
    /// does `c.dateAccepted + const.DAY * c.numDays` unconditionally in
    /// `Init`, for every contract type (**[V]**).
    pub date_accepted: i64,
    /// 0 until closed. Never `None`: `contractPanels._GetListEntry` sorts on
    /// `-contract.dateCompleted` (**[V]**).
    pub date_completed: i64,
    /// Courier days to complete; 0 for an item exchange. Same `Init` line.
    pub num_days: i64,
    /// `conStatus*`.
    pub status: i64,
    /// The issuer's description, the column the client calls "Info by
    /// issuer".
    pub title: String,
    pub lines: Vec<Line>,
}

impl Contract {
    /// Where this contract's items wait: one escrow location per contract,
    /// owned by [`ESCROW_OWNER`].
    pub fn escrow_slot(&self) -> Slot {
        escrow_slot(self.contract_id)
    }

    /// Past its expiry. An expired contract is still outstanding -- its items
    /// stay in escrow until the issuer deletes it, which is what the client
    /// models: `ContractsSvc.DeleteContract` skips its confirmation for a
    /// contract whose `dateExpired` has passed (**[V]**), and the expired list
    /// is the "requires attention" filter.
    pub fn is_expired(&self, now: i64) -> bool {
        now > self.date_expired
    }

    /// Can be accepted (or bid on) right now.
    pub fn is_open(&self, now: i64) -> bool {
        self.status == STATUS_OUTSTANDING && !self.is_expired(now)
    }

    /// A courier in progress past `dateAccepted + DAY * numDays`, the same
    /// sum the details window and `FailContract` make (**[V]**), at the game
    /// clock's pace ([`Contract::due_at`]).
    pub fn is_overdue(&self, now: i64, clock: crate::game_clock::GameClock) -> bool {
        self.status == STATUS_IN_PROGRESS && now > self.due_at(clock)
    }

    /// A courier's deadline: `dateAccepted + clock.span(DAY * numDays)`
    /// (lane `clock`). Derived from the stored acceptance, so a restart at
    /// another scale re-derives it. **[A]** the details window computes its
    /// own `dateAccepted + numDays` and shows the nominal deadline at a scale
    /// above 1; the neocom row ([`Contract::in_progress_row`]) carries this
    /// one. Falsified by a courier failed early by a shown deadline.
    pub fn due_at(&self, clock: crate::game_clock::GameClock) -> i64 {
        self.date_accepted + clock.span(DAY * self.num_days)
    }

    /// An auction's high bid, if any.
    pub fn top_bid(&self) -> Option<&Bid> {
        self.bids.first()
    }

    /// The smallest bid the client's own dialog offers:
    /// `max(price, current + max(int(0.1 * price), 1000))`, capped at the
    /// buyout when there is one (`contracts.PlaceBid:684-695`, **[V]**). The
    /// server enforcing the same floor is **[A]**.
    pub fn min_bid(&self) -> f64 {
        let current = self.top_bid().map_or(0.0, |b| b.amount);
        let mut step = current + ((0.1 * self.price) as i64).max(1000) as f64;
        if self.collateral > 0.0 {
            step = step.min(self.collateral);
        }
        self.price.max(step).trunc()
    }

    /// The ISK this contract holds right now, whoever it came from: an
    /// outstanding or in-progress reward, a courier's collateral, an
    /// auction's high bid until its issuer has collected it. **The one
    /// definition every conservation test adds to the wallets**, so a leg
    /// that forgets to move money shows up as ISK created or lost.
    pub fn escrow_isk(&self) -> f64 {
        let mut held = 0.0;
        if matches!(self.status, STATUS_OUTSTANDING | STATUS_IN_PROGRESS) {
            held += self.reward;
        }
        if self.kind == TYPE_COURIER && self.status == STATUS_IN_PROGRESS {
            held += self.collateral;
        }
        if self.kind == TYPE_AUCTION
            && !matches!(self.status, STATUS_FINISHED_ISSUER | STATUS_FINISHED)
        {
            held += self.top_bid().map_or(0.0, |b| b.amount);
        }
        held
    }

    /// Is this contract assigned to `who`: the character, their corporation
    /// or their alliance, as `availability` says.
    pub fn assigned_to(&self, who: &Who) -> bool {
        if self.assignee_id == 0 {
            return false;
        }
        match self.availability {
            AVAIL_CORP => who.corp == self.assignee_id,
            AVAIL_ALLIANCE => who.alliance == self.assignee_id,
            _ => who.me == self.assignee_id,
        }
    }

    /// May `who` see this contract at all. **[A]**: public contracts are
    /// everyone's; a private one is its issuer's, assignee's (or the
    /// assigned corporation's or alliance's) and acceptor's.
    pub fn visible_to(&self, who: &Who) -> bool {
        self.assignee_id == 0
            || self.assigned_to(who)
            || self.issuer_id == who.me
            || self.acceptor_id == who.me
    }

    /// The contract as the client reads `c.<attr>`: a `KeyVal`, because
    /// every reader in the five files above uses attribute access and nothing
    /// indexes it. The attribute list is the union of every `c.X` read in
    /// those files (**[V]**); each value's type is set by its most demanding
    /// reader, noted inline.
    pub fn row(&self) -> PyValue {
        // Only a courier has a destination; the others' `end*` repeat the
        // start. **[A]**, and the safe one: `contractsearch` does
        // `cfg.evelocations.Get(c.endSolarSystemID).name` for every result,
        // whatever its type.
        keyval(vec![
            ("contractID", PyValue::Int(self.contract_id)),
            ("type", PyValue::Int(self.kind)),
            ("issuerID", PyValue::Int(self.issuer_id)),
            ("issuerCorpID", PyValue::Int(self.issuer_corp_id)),
            // `[c.issuerID, c.issuerCorpID][c.forCorp]` -- an index, so a
            // bool or an int, never None (contracts.py OnContractAssigned).
            ("forCorp", PyValue::Bool(false)),
            // `not not c.availability` picks the private branch
            // (GetBasicInfoText).
            (
                "availability",
                PyValue::Int(if self.assignee_id == 0 {
                    AVAIL_PUBLIC
                } else {
                    self.availability
                }),
            ),
            ("assigneeID", PyValue::Int(self.assignee_id)),
            // `c.acceptorID > 0` (GetBasicInfoText).
            ("acceptorID", PyValue::Int(self.acceptor_id)),
            ("acceptorWalletKey", PyValue::None),
            ("issuerWalletKey", PyValue::None),
            ("startStationID", PyValue::Int(self.start_station_id)),
            ("startSolarSystemID", PyValue::Int(self.start_system_id)),
            ("startRegionID", PyValue::Int(self.start_region_id)),
            ("endStationID", PyValue::Int(self.end_station_id)),
            ("endSolarSystemID", PyValue::Int(self.end_system_id)),
            ("endRegionID", PyValue::Int(self.end_region_id)),
            // Floats: `c.price > 0`, `int(c.price)`, `FmtISK...(c.price)`.
            ("price", PyValue::Real(self.price)),
            ("reward", PyValue::Real(self.reward)),
            // `c.collateral > 0` and `FmtISK(c.collateral)` (details window,
            // `PlaceBid`, `AcceptContract`'s courier confirmation).
            ("collateral", PyValue::Real(self.collateral)),
            ("volume", PyValue::Real(self.volume)),
            // FILETIMEs: large enough to marshal as INT64 and arrive as a
            // `long`, which `FmtDate` needs (`model::Character::create_date`'s
            // doc has the trap).
            ("dateIssued", PyValue::Int(self.date_issued)),
            ("dateExpired", PyValue::Int(self.date_expired)),
            ("dateAccepted", PyValue::Int(self.date_accepted)),
            ("dateCompleted", PyValue::Int(self.date_completed)),
            ("numDays", PyValue::Int(self.num_days)),
            ("status", PyValue::Int(self.status)),
            // `c.title.lower()` (contractsearch sort key) and
            // `desc == ''` (GetBasicInfoText).
            ("title", PyValue::Unicode(self.title.clone())),
        ])
    }

    /// The contract's lines, as `contract.items` is read: attribute access on
    /// each, `.inCrate`, `.itemTypeID`, `max(1, .quantity)`, `.parentID > 0`,
    /// `.copy == 1`, `.damage`, `.flagID`, and three blueprint figures read
    /// `or 0` (contractsDetailsWnd `InsertItemList`, `GetItemDamageText`,
    /// **[V]**).
    pub fn line_rows(&self) -> Vec<PyValue> {
        self.lines
            .iter()
            .map(|l| {
                keyval(vec![
                    ("itemID", l.item_id.map_or(PyValue::None, PyValue::Int)),
                    ("itemTypeID", PyValue::Int(l.type_id)),
                    ("quantity", PyValue::Int(l.quantity)),
                    ("inCrate", PyValue::Bool(l.in_crate)),
                    ("parentID", PyValue::Int(l.parent_id)),
                    ("flagID", PyValue::Int(l.flag_id)),
                    ("copy", PyValue::Int(0)),
                    ("damage", PyValue::Int(0)),
                    ("materialLevel", PyValue::Int(0)),
                    ("productivityLevel", PyValue::Int(0)),
                    ("licensedProductionRunsRemaining", PyValue::Int(0)),
                ])
            })
            .collect()
    }

    /// `GetContract`'s answer, and one `SearchContracts` result: an object
    /// with `.contract`, `.items` and `.bids` (contracts.py `GetContract`
    /// stores it and every caller reads `.contract`; `contractsearch` reads
    /// `_c.items`, `_c.bids`, `_c.contract`; the details window does
    /// `len(contract.bids)`, **[V]**).
    pub fn details(&self) -> PyValue {
        keyval(vec![
            ("contract", self.row()),
            ("items", PyValue::List(self.line_rows())),
            ("bids", PyValue::List(self.bid_rows())),
        ])
    }

    /// `contract.bids`: highest first, each read `.amount` and `.bidderID`
    /// (`contracts.PlaceBid`, `contractsDetailsWnd.Init:195/274`, **[V]**).
    /// `bidDateTime` is **[A]**, carried for the bid history list.
    pub fn bid_rows(&self) -> Vec<PyValue> {
        self.bids
            .iter()
            .map(|b| {
                keyval(vec![
                    ("contractID", PyValue::Int(self.contract_id)),
                    ("bidderID", PyValue::Int(b.bidder_id)),
                    ("amount", PyValue::Real(b.amount)),
                    ("bidDateTime", PyValue::Int(b.date)),
                ])
            })
            .collect()
    }

    /// One entry of `GetLoginInfo().inProgress`: `NeocomBlink` reads
    /// `.contractID`, `.startStationID`, `.endStationID`, `.expires` and keeps
    /// them as the courier bookmarks menu (`contracts.py:273-275`, **[V]**).
    pub fn in_progress_row(&self, clock: crate::game_clock::GameClock) -> PyValue {
        keyval(vec![
            ("contractID", PyValue::Int(self.contract_id)),
            ("startStationID", PyValue::Int(self.start_station_id)),
            ("endStationID", PyValue::Int(self.end_station_id)),
            ("expires", PyValue::Int(self.due_at(clock))),
        ])
    }

    /// The tuple `CollectMyPageInfo.outstandingContracts` carries: read **by
    /// position**, `contract[0..3]` as (issuerID, issuerCorpID, assigneeID,
    /// type) in `contractPanels.MyContractsStartPage.Init` (**[V]**).
    pub fn outstanding_tuple(&self) -> PyValue {
        PyValue::Tuple(vec![
            PyValue::Int(self.issuer_id),
            PyValue::Int(self.issuer_corp_id),
            PyValue::Int(self.assignee_id),
            PyValue::Int(self.kind),
        ])
    }
}

/// The escrow slot of contract `contract_id`.
pub fn escrow_slot(contract_id: i64) -> Slot {
    Slot::new(
        ESCROW_OWNER,
        escrow_location(contract_id),
        crate::model::FLAG_HANGAR,
    )
}

/// The contract-list envelope: an object with `contracts` and `items`.
///
/// `GetContractListForOwner`, `GetMyExpiredContractList` and `GetMyBids` all
/// answer this, because all three branches of
/// `contractPanels._GetContractsToShow` converge on
/// `return _contracts.contracts, _contracts.items` (**[V]**).
///
/// `contracts` is a **list** -- iterated, `len()`ed and subscripted `[0]` and
/// `[-1]` by `UpdatePagination`. `items` is a **dict keyed by contractID** of
/// line lists, read only as `itemsByContractID.get(contract.contractID, [])`.
/// Deliberately not a `CRowset`: nothing calls `.Index()` on it.
pub fn list_page<'a>(contracts: impl IntoIterator<Item = &'a Contract>) -> PyValue {
    let mut rows = Vec::new();
    let mut items = Vec::new();
    for c in contracts {
        rows.push(c.row());
        // Marshal dict entries are `(value, key)`.
        items.push((PyValue::List(c.line_rows()), PyValue::Int(c.contract_id)));
    }
    keyval(vec![
        ("contracts", PyValue::List(rows)),
        ("items", PyValue::Dict(items)),
    ])
}

// ---- pushes --------------------------------------------------------------------

/// One of the contract pushes, addressed to one character.
///
/// Every name here is in `ContractsSvc.__notifyevents__` and every handler
/// takes one argument, the contract id (contracts.py:98-106 and
/// `OnContractAssigned` .. `OnContractBuyout`, **[V]**). `OnContractAssigned`
/// calls `GetContract` and blinks the neocom with "assigned to you";
/// `OnContractAccepted` raises the "contract accepted" notification; the
/// other five each show a `ContractMessage*` line. Who gets which is **[A]**:
/// the assignee is told it was assigned, the issuer that it was accepted,
/// completed, rejected or bought out, the other party that a courier failed,
/// and the previous high bidder that they were outbid -- the only readings of
/// those handlers that make sense.
#[derive(Debug, Clone, PartialEq)]
pub struct ContractEvent {
    pub to: i64,
    pub event: &'static str,
    pub contract_id: i64,
}

impl ContractEvent {
    pub fn assigned(to: i64, contract_id: i64) -> Self {
        ContractEvent {
            to,
            event: "OnContractAssigned",
            contract_id,
        }
    }

    pub fn accepted(to: i64, contract_id: i64) -> Self {
        ContractEvent {
            to,
            event: "OnContractAccepted",
            contract_id,
        }
    }

    pub fn completed(to: i64, contract_id: i64) -> Self {
        ContractEvent {
            to,
            event: "OnContractCompleted",
            contract_id,
        }
    }

    pub fn failed(to: i64, contract_id: i64) -> Self {
        ContractEvent {
            to,
            event: "OnContractFailed",
            contract_id,
        }
    }

    pub fn rejected(to: i64, contract_id: i64) -> Self {
        ContractEvent {
            to,
            event: "OnContractRejected",
            contract_id,
        }
    }

    pub fn outbid(to: i64, contract_id: i64) -> Self {
        ContractEvent {
            to,
            event: "OnContractOutbid",
            contract_id,
        }
    }

    pub fn buyout(to: i64, contract_id: i64) -> Self {
        ContractEvent {
            to,
            event: "OnContractBuyout",
            contract_id,
        }
    }
}

impl crate::notify::Push for ContractEvent {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (self.event, vec![PyValue::Int(self.contract_id)])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

// ---- the book ----------------------------------------------------------------

/// Every contract the world has seen, and the id allocator.
///
/// Closed contracts stay: the Finished filter of My Contracts is their only
/// reader, and it is supposed to show them.
#[derive(Debug, Clone, PartialEq)]
pub struct Contracts {
    contracts: Vec<Contract>,
    next_id: i64,
}

impl Default for Contracts {
    fn default() -> Self {
        Contracts {
            contracts: Vec::new(),
            next_id: FIRST_CONTRACT_ID,
        }
    }
}

impl Contracts {
    /// Hand out a contract id.
    pub fn mint(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn get(&self, contract_id: i64) -> Option<&Contract> {
        self.contracts.iter().find(|c| c.contract_id == contract_id)
    }

    pub fn get_mut(&mut self, contract_id: i64) -> Option<&mut Contract> {
        self.contracts
            .iter_mut()
            .find(|c| c.contract_id == contract_id)
    }

    pub fn insert(&mut self, c: Contract) {
        self.next_id = self.next_id.max(c.contract_id + 1);
        self.contracts.push(c);
    }

    /// Newest first: the client pages by contract id, keeping the first and
    /// last id of each page (`contractPanels.UpdatePagination`, **[V]**).
    pub fn newest_first(&self) -> impl Iterator<Item = &Contract> {
        self.contracts.iter().rev()
    }

    pub fn all(&self) -> &[Contract] {
        &self.contracts
    }

    /// The record tags this type writes into the world file. Same contract
    /// as `world::Bookmarks::RECORDS` and `MarketBook::RECORDS`: `store.rs`
    /// hands every line whose tag is listed here to [`Contracts::parse_record`].
    pub(crate) const RECORDS: &'static [&'static str] =
        &["connext", "contract", "conline", "conbid"];

    /// The durable form. **Persisted because escrow is**: an outstanding
    /// contract holds items that have left a hangar and ISK that has left a
    /// wallet. A world that kept those and dropped the contract would strand
    /// both. Nothing at all for a world nobody has contracted in, so an older
    /// world file reads back identical.
    ///
    /// The courier and auction fields (destination, collateral, package,
    /// bids) came later than the first world files: they sit **after** the
    /// title on a `contract` line and in their own `conbid` lines, and a
    /// line without them reads back as a contract that never had any.
    pub(crate) fn render(&self, out: &mut String) {
        use std::fmt::Write as _;
        if self.contracts.is_empty() {
            return;
        }
        let _ = writeln!(out, "connext\t{}", self.next_id);
        for c in &self.contracts {
            let _ = writeln!(
                out,
                "contract\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                c.contract_id,
                c.kind,
                c.issuer_id,
                c.issuer_corp_id,
                c.assignee_id,
                c.acceptor_id,
                c.start_station_id,
                c.start_system_id,
                c.start_constellation_id,
                c.start_region_id,
                c.price,
                c.reward,
                c.volume,
                c.date_issued,
                c.date_expired,
                c.date_accepted,
                c.date_completed,
                c.num_days,
                c.status,
                c.lines.len(),
                crate::store::esc(&c.title),
                c.end_station_id,
                c.end_system_id,
                c.end_constellation_id,
                c.end_region_id,
                c.collateral,
                c.crate_id,
                c.availability,
            );
            for l in &c.lines {
                let _ = writeln!(
                    out,
                    "conline\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    c.contract_id,
                    l.item_id.unwrap_or(-1),
                    l.type_id,
                    l.quantity,
                    l.in_crate as u8,
                    l.parent_id,
                    l.flag_id,
                );
            }
            for b in &c.bids {
                let _ = writeln!(
                    out,
                    "conbid\t{}\t{}\t{}\t{}",
                    c.contract_id, b.bidder_id, b.amount, b.date,
                );
            }
        }
    }

    /// Read one record whose tag is in [`Contracts::RECORDS`].
    ///
    /// **Strict**, like `MarketBook::parse_record`: a contract line we cannot
    /// read is escrow that would silently vanish, so the world is refused and
    /// the line named instead.
    pub(crate) fn parse_record(&mut self, f: &[&str]) -> Result<(), String> {
        let int = |i: usize| -> Result<i64, String> {
            f.get(i)
                .and_then(|v| v.parse::<i64>().ok())
                .ok_or_else(|| format!("field {i} is not an integer"))
        };
        let real = |i: usize| -> Result<f64, String> {
            f.get(i)
                .and_then(|v| v.parse::<f64>().ok())
                .ok_or_else(|| format!("field {i} is not a number"))
        };
        // A field a pre-courier world file does not have: absent reads as
        // `default`, present must parse.
        let int_or = |i: usize, default: i64| -> Result<i64, String> {
            match f.get(i) {
                None => Ok(default),
                Some(_) => int(i),
            }
        };
        let real_or = |i: usize| -> Result<f64, String> {
            match f.get(i) {
                None => Ok(0.0),
                Some(_) => real(i),
            }
        };
        match f.first().copied() {
            Some("connext") => self.next_id = self.next_id.max(int(1)?),
            Some("contract") => {
                let c = Contract {
                    contract_id: int(1)?,
                    kind: int(2)?,
                    issuer_id: int(3)?,
                    issuer_corp_id: int(4)?,
                    assignee_id: int(5)?,
                    acceptor_id: int(6)?,
                    start_station_id: int(7)?,
                    start_system_id: int(8)?,
                    start_constellation_id: int(9)?,
                    start_region_id: int(10)?,
                    price: real(11)?,
                    reward: real(12)?,
                    volume: real(13)?,
                    date_issued: int(14)?,
                    date_expired: int(15)?,
                    date_accepted: int(16)?,
                    date_completed: int(17)?,
                    num_days: int(18)?,
                    status: int(19)?,
                    // Field 20 is the line count, written for a human
                    // reading the file; the `conline` records are the data.
                    title: crate::store::unesc(f.get(21).copied().unwrap_or("")),
                    end_station_id: int_or(22, int(7)?)?,
                    end_system_id: int_or(23, int(8)?)?,
                    end_constellation_id: int_or(24, int(9)?)?,
                    end_region_id: int_or(25, int(10)?)?,
                    collateral: real_or(26)?,
                    crate_id: int_or(27, 0)?,
                    availability: int_or(28, i64::from(int(5)? != 0))?,
                    bids: Vec::new(),
                    lines: Vec::new(),
                };
                if self.get(c.contract_id).is_some() {
                    return Err(format!("contract {} appears twice", c.contract_id));
                }
                self.insert(c);
            }
            Some("conline") => {
                let id = int(1)?;
                let line = Line {
                    item_id: match int(2)? {
                        -1 => None,
                        v => Some(v),
                    },
                    type_id: int(3)?,
                    quantity: int(4)?,
                    in_crate: int(5)? != 0,
                    parent_id: int(6)?,
                    flag_id: int(7)?,
                };
                // `render` writes a contract before its lines.
                self.get_mut(id)
                    .ok_or_else(|| format!("line for unknown contract {id}"))?
                    .lines
                    .push(line);
            }
            Some("conbid") => {
                let id = int(1)?;
                let bid = Bid {
                    bidder_id: int(2)?,
                    amount: real(3)?,
                    date: int(4)?,
                };
                // Written highest first, so appending keeps the order.
                self.get_mut(id)
                    .ok_or_else(|| format!("bid for unknown contract {id}"))?
                    .bids
                    .push(bid);
            }
            other => return Err(format!("{other:?} is not a contract record")),
        }
        Ok(())
    }
}

// ---- reading the client's ContractStruct ----------------------------------------

/// What `CreateContract` asked for: the client's `ContractStruct`, read.
///
/// `createContractWnd.CreateContract` builds it with seventeen keywords and
/// `ContractsSvc.CreateContract` passes it through unchanged as the one
/// positional argument (**[V]**, both files). `ContractStruct` is a new-style
/// class with no `__reduce__` of its own, so it reaches us as a `0x23`
/// `ObjectEx` whose state is the instance `__dict__` **[I]** -- this reader
/// also takes a plain dict or an old-style instance, so a test can build
/// either and the shape question stays in [`attr`].
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub kind: i64,
    /// 0 for public. The wizard sends `assigneeID`, or 0 when no name was
    /// given, as `assignedToID`; `isPrivate` is `not not assigneeID` and adds
    /// nothing.
    pub assignee_id: i64,
    pub minutes_expire: i64,
    pub num_days: i64,
    pub start_station_id: i64,
    pub end_station_id: i64,
    pub price: f64,
    pub reward: f64,
    pub collateral: f64,
    pub title: String,
    /// `[[itemID, quantity], ...]`: `items.append([i, self.data.items[i][1]])`.
    pub items: Vec<(i64, i64)>,
    /// `[[typeID, quantity], ...]`: `map(list, self.data.reqItems.items())`.
    pub requested: Vec<(i64, i64)>,
    pub for_corp: bool,
    pub multi_contract: bool,
}

/// One attribute of a client object: a dict, an old-style instance's state,
/// or an `ObjectEx`'s `__reduce_ex__` state (header element 1).
pub fn attr<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    match v {
        PyValue::Dict(_) => crate::pyutil::dict_get(v, name.as_bytes()),
        PyValue::Instance { state, .. } => crate::pyutil::dict_get(state, name.as_bytes()),
        PyValue::ObjectEx { header, .. } => match header.as_ref() {
            PyValue::Tuple(parts) => parts
                .get(1)
                .and_then(|state| crate::pyutil::dict_get(state, name.as_bytes())),
            _ => None,
        },
        _ => None,
    }
}

/// A number the client may send as an int, a long or a float.
pub fn num(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Int(n) => Some(*n as f64),
        PyValue::Real(r) => Some(*r),
        PyValue::Bool(b) => Some(*b as i64 as f64),
        _ => None,
    }
}

/// An id or count; `None` and absent read as 0, which is what every one of
/// these fields means by it (`getattr(self.data, 'assigneeID', 0)` and its
/// siblings, and the copy-contract path sets `assigneeID = None`).
pub fn int(v: Option<&PyValue>) -> i64 {
    num(v).map_or(0, |n| n as i64)
}

pub fn truthy(v: Option<&PyValue>) -> bool {
    match v {
        Some(PyValue::Bool(b)) => *b,
        Some(PyValue::Int(n)) => *n != 0,
        _ => false,
    }
}

/// A text field, `str` or `unicode`.
pub fn text(v: Option<&PyValue>) -> String {
    match v {
        Some(PyValue::Unicode(s)) => s.clone(),
        Some(PyValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
        _ => String::new(),
    }
}

/// `[[a, b], ...]` as pairs. Inner lists or tuples; anything else skipped.
pub fn pairs(v: Option<&PyValue>) -> Vec<(i64, i64)> {
    let outer = match v {
        Some(PyValue::List(x)) | Some(PyValue::Tuple(x)) => x.as_slice(),
        _ => &[],
    };
    outer
        .iter()
        .filter_map(|p| match p {
            PyValue::List(x) | PyValue::Tuple(x) if x.len() >= 2 => {
                Some((int(x.first()), int(x.get(1))))
            }
            _ => None,
        })
        .collect()
}

/// Read a `ContractStruct`. `None` when the value carries no `contractType`
/// at all, which is not something the wizard can send.
pub fn parse_draft(v: &PyValue) -> Option<Draft> {
    let a = |name: &str| attr(v, name);
    a("contractType")?;
    Some(Draft {
        kind: int(a("contractType")),
        assignee_id: int(a("assignedToID")),
        minutes_expire: int(a("minutesExpire")),
        num_days: int(a("numDays")),
        start_station_id: int(a("startStationID")),
        end_station_id: int(a("destinationID")),
        price: num(a("price")).unwrap_or(0.0),
        reward: num(a("reward")).unwrap_or(0.0),
        collateral: num(a("collateral")).unwrap_or(0.0),
        title: text(a("title")),
        items: pairs(a("itemList")),
        requested: pairs(a("requestItemTypeList")),
        for_corp: truthy(a("forCorp")),
        multi_contract: truthy(a("multiContract")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Contract {
        Contract {
            contract_id: FIRST_CONTRACT_ID,
            kind: TYPE_ITEM_EXCHANGE,
            issuer_id: 90_000_001,
            issuer_corp_id: 1_000_044,
            assignee_id: 90_000_002,
            availability: AVAIL_MYSELF,
            acceptor_id: 0,
            start_station_id: 60_003_760,
            start_system_id: 30_000_142,
            start_constellation_id: 20_000_020,
            start_region_id: 10_000_002,
            end_station_id: 60_003_760,
            end_system_id: 30_000_142,
            end_constellation_id: 20_000_020,
            end_region_id: 10_000_002,
            price: 1_250.5,
            reward: 0.0,
            collateral: 0.0,
            crate_id: 0,
            bids: Vec::new(),
            volume: 0.01,
            date_issued: 134_000_000_000_000_000,
            date_expired: 134_000_000_000_000_000 + 1440 * MINUTE,
            date_accepted: 0,
            date_completed: 0,
            num_days: 0,
            status: STATUS_OUTSTANDING,
            title: "tabs\tand %percent\nlines".to_owned(),
            lines: vec![
                Line {
                    item_id: Some(100_000_050),
                    type_id: 34,
                    quantity: 1000,
                    in_crate: true,
                    parent_id: 0,
                    flag_id: 4,
                },
                Line {
                    item_id: None,
                    type_id: 35,
                    quantity: 7,
                    in_crate: false,
                    parent_id: 0,
                    flag_id: 0,
                },
            ],
        }
    }

    /// Escrow is persisted, so the book must read back exactly -- including
    /// a title with the three characters the line format reserves.
    #[test]
    fn the_book_survives_its_own_world_file() {
        let mut book = Contracts::default();
        let id = book.mint();
        let mut c = sample();
        c.contract_id = id;
        book.insert(c);
        let mut out = String::new();
        book.render(&mut out);
        let mut back = Contracts::default();
        for line in out.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            assert!(Contracts::RECORDS.contains(&f[0]), "{line}");
            back.parse_record(&f).unwrap();
        }
        assert_eq!(back, book);
        assert_eq!(
            back.mint(),
            id + 1,
            "the allocator must not hand out {id} again"
        );
    }

    /// A courier in progress and an auction with bids carry the fields the
    /// first world files did not have; both read back exactly, bids in their
    /// highest-first order.
    #[test]
    fn courier_and_auction_fields_survive_the_world_file() {
        let mut book = Contracts::default();
        let mut courier = sample();
        courier.contract_id = book.mint();
        courier.kind = TYPE_COURIER;
        courier.end_station_id = 60_000_004;
        courier.end_system_id = 30_000_003;
        courier.collateral = 2_500_000.25;
        courier.reward = 10_000.0;
        courier.crate_id = 100_000_777;
        courier.status = STATUS_IN_PROGRESS;
        courier.num_days = 3;
        book.insert(courier);
        let mut auction = sample();
        auction.contract_id = book.mint();
        auction.kind = TYPE_AUCTION;
        auction.collateral = 90_000.0;
        auction.bids = vec![
            Bid {
                bidder_id: 90_000_003,
                amount: 20_000.0,
                date: 2,
            },
            Bid {
                bidder_id: 90_000_002,
                amount: 10_000.0,
                date: 1,
            },
        ];
        book.insert(auction);
        let mut out = String::new();
        book.render(&mut out);
        let mut back = Contracts::default();
        for line in out.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            assert!(Contracts::RECORDS.contains(&f[0]), "{line}");
            back.parse_record(&f).unwrap();
        }
        assert_eq!(back, book);
    }

    /// A `contract` line written before couriers existed -- 22 fields,
    /// ending at the title -- still reads, as a contract with no
    /// destination of its own, no collateral and no package.
    #[test]
    fn a_pre_courier_contract_line_still_reads() {
        let mut out = String::new();
        let mut book = Contracts::default();
        let mut c = sample();
        c.contract_id = book.mint();
        book.insert(c.clone());
        book.render(&mut out);
        let mut back = Contracts::default();
        for line in out.lines() {
            let mut f: Vec<&str> = line.split('\t').collect();
            if f[0] == "contract" {
                f.truncate(22);
            }
            back.parse_record(&f).unwrap();
        }
        let got = back.get(c.contract_id).unwrap();
        assert_eq!(got.end_station_id, c.start_station_id);
        assert_eq!(got.end_region_id, c.start_region_id);
        assert_eq!(got.collateral, 0.0);
        assert_eq!(got.crate_id, 0);
    }

    /// The client's own minimum bid (`contracts.PlaceBid:684-695`): the
    /// starting price, then 10% of it (at least 1,000) over the high bid,
    /// never above the buyout.
    #[test]
    fn the_minimum_bid_is_the_client_s_formula() {
        let mut c = sample();
        c.kind = TYPE_AUCTION;
        c.price = 50_000.0;
        assert_eq!(c.min_bid(), 50_000.0);
        c.bids.push(Bid {
            bidder_id: 1,
            amount: 60_000.0,
            date: 0,
        });
        assert_eq!(c.min_bid(), 65_000.0);
        c.collateral = 62_000.0;
        assert_eq!(c.min_bid(), 62_000.0);
        c.price = 100.0;
        c.collateral = 0.0;
        c.bids.clear();
        assert_eq!(c.min_bid(), 1_000.0, "max(int(0.1 * 100), 1000) over 0");
    }

    /// `NeocomBlink` keeps `[startStationID, endStationID, expires]` per
    /// in-progress courier, keyed by `.contractID`.
    #[test]
    fn an_in_progress_row_has_what_neocom_blink_reads() {
        let mut c = sample();
        c.kind = TYPE_COURIER;
        c.date_accepted = 1_000;
        c.num_days = 2;
        let row = c.in_progress_row(crate::game_clock::GameClock::REAL);
        for name in ["contractID", "startStationID", "endStationID"] {
            assert!(attr(&row, name).is_some(), "{name}");
        }
        assert_eq!(attr(&row, "expires"), Some(&PyValue::Int(1_000 + 2 * DAY)));
    }

    /// Lane clock: at scale N a courier is overdue N times sooner, and the
    /// neocom row carries that deadline.
    #[test]
    fn at_scale_n_a_courier_is_overdue_n_times_sooner() {
        let clock = crate::game_clock::GameClock::new(8).unwrap();
        let mut c = sample();
        c.kind = TYPE_COURIER;
        c.status = STATUS_IN_PROGRESS;
        c.date_accepted = 1_000;
        c.num_days = 2;
        let due = 1_000 + 2 * DAY / 8;
        assert!(!c.is_overdue(due, clock));
        assert!(c.is_overdue(due + 1, clock));
        assert!(!c.is_overdue(due + 1, crate::game_clock::GameClock::REAL));
        let row = c.in_progress_row(clock);
        assert_eq!(attr(&row, "expires"), Some(&PyValue::Int(due)));
    }

    #[test]
    fn an_empty_book_writes_nothing() {
        let mut out = String::new();
        Contracts::default().render(&mut out);
        assert!(out.is_empty());
    }

    /// The details window does `c.dateAccepted + const.DAY * c.numDays` for
    /// every contract and the list sorts on `-contract.dateCompleted`, so
    /// those must be integers on an outstanding contract; `GetBasicInfoText`
    /// and `OnContractAssigned` read the rest.
    #[test]
    fn a_contract_row_carries_every_attribute_the_client_reads() {
        let row = sample().row();
        for name in [
            "contractID",
            "type",
            "issuerID",
            "issuerCorpID",
            "forCorp",
            "availability",
            "assigneeID",
            "acceptorID",
            "acceptorWalletKey",
            "startStationID",
            "startSolarSystemID",
            "startRegionID",
            "endStationID",
            "endSolarSystemID",
            "endRegionID",
            "price",
            "reward",
            "collateral",
            "volume",
            "dateIssued",
            "dateExpired",
            "dateAccepted",
            "dateCompleted",
            "numDays",
            "status",
            "title",
        ] {
            assert!(attr(&row, name).is_some(), "the contract row lacks {name}");
        }
        for name in ["dateAccepted", "dateCompleted", "numDays", "acceptorID"] {
            assert!(
                matches!(attr(&row, name), Some(PyValue::Int(_))),
                "{name} is used in arithmetic or a comparison and must be an int"
            );
        }
    }

    #[test]
    fn details_is_contract_items_and_bids() {
        let d = sample().details();
        assert!(attr(&d, "contract").is_some());
        let Some(PyValue::List(items)) = attr(&d, "items") else {
            panic!("items must be a list");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(attr(&items[0], "inCrate"), Some(&PyValue::Bool(true)));
        assert_eq!(attr(&items[1], "inCrate"), Some(&PyValue::Bool(false)));
        assert_eq!(attr(&d, "bids"), Some(&PyValue::List(vec![])));
    }

    /// `bids[0].amount` and `bids[0].bidderID`, highest first.
    #[test]
    fn bids_are_read_amount_and_bidder_highest_first() {
        let mut c = sample();
        c.kind = TYPE_AUCTION;
        c.bids = vec![
            Bid {
                bidder_id: 7,
                amount: 3.0,
                date: 0,
            },
            Bid {
                bidder_id: 8,
                amount: 2.0,
                date: 0,
            },
        ];
        let d = c.details();
        let Some(PyValue::List(bids)) = attr(&d, "bids") else {
            panic!("bids is a list")
        };
        assert_eq!(attr(&bids[0], "amount"), Some(&PyValue::Real(3.0)));
        assert_eq!(attr(&bids[0], "bidderID"), Some(&PyValue::Int(7)));
    }

    /// The envelope's `items` is keyed by contract id, so the client's
    /// `.get(contract.contractID, [])` finds the lines.
    #[test]
    fn the_list_page_keys_items_by_contract_id() {
        let c = sample();
        let page = list_page([&c]);
        let Some(items) = attr(&page, "items") else {
            panic!("no items");
        };
        let kv = items.dict_as_kv().expect("items is a dict");
        assert_eq!(kv.len(), 1);
        assert_eq!(kv[0].0, &PyValue::Int(c.contract_id));
    }

    /// The wizard's seventeen keywords, as the `0x23` ObjectEx a new-style
    /// instance marshals to: `((cls,), state)`.
    #[test]
    fn a_contract_struct_reads_from_its_reduce_state() {
        let state = crate::pyutil::pydict(vec![
            ("contractType", PyValue::Int(TYPE_ITEM_EXCHANGE)),
            ("isPrivate", PyValue::Bool(true)),
            ("assignedToID", PyValue::Int(90_000_002)),
            ("minutesExpire", PyValue::Int(1440)),
            ("numDays", PyValue::Int(0)),
            ("startStationID", PyValue::Int(60_003_760)),
            ("destinationID", PyValue::Int(0)),
            ("price", PyValue::Int(5000)),
            ("reward", PyValue::Int(0)),
            ("collateral", PyValue::Int(0)),
            ("title", PyValue::Unicode("for Bob".to_owned())),
            ("description", PyValue::Unicode(String::new())),
            (
                "itemList",
                PyValue::List(vec![PyValue::List(vec![
                    PyValue::Int(100_000_050),
                    PyValue::Int(10),
                ])]),
            ),
            ("startStationDivision", PyValue::Int(4)),
            (
                "requestItemTypeList",
                PyValue::List(vec![PyValue::List(vec![PyValue::Int(35), PyValue::Int(2)])]),
            ),
            ("forCorp", PyValue::Bool(false)),
            ("multiContract", PyValue::Bool(false)),
        ]);
        let v = PyValue::ObjectEx {
            form: eve_marshal::ObjectForm::New,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::Tuple(vec![PyValue::Global(
                    b"contracts.contractStruct.ContractStruct".to_vec(),
                )]),
                state,
            ])),
            list: vec![],
            dict: vec![],
        };
        let d = parse_draft(&v).expect("a ContractStruct");
        assert_eq!(d.kind, TYPE_ITEM_EXCHANGE);
        assert_eq!(d.assignee_id, 90_000_002);
        assert_eq!(d.minutes_expire, 1440);
        assert_eq!(d.price, 5000.0);
        assert_eq!(d.title, "for Bob");
        assert_eq!(d.items, vec![(100_000_050, 10)]);
        assert_eq!(d.requested, vec![(35, 2)]);
        assert!(!d.for_corp && !d.multi_contract);
    }

    #[test]
    fn a_none_assignee_is_public() {
        let v = crate::pyutil::pydict(vec![
            ("contractType", PyValue::Int(TYPE_ITEM_EXCHANGE)),
            ("assignedToID", PyValue::None),
        ]);
        assert_eq!(parse_draft(&v).unwrap().assignee_id, 0);
        assert_eq!(parse_draft(&PyValue::None), None);
    }

    #[test]
    fn escrow_sits_in_the_one_band_and_below_the_market_s_tickets() {
        let slot = escrow_slot(FIRST_CONTRACT_ID);
        assert!(crate::item::is_escrow(slot.location_id));
        assert!(slot.location_id < escrow_location(1_000_000_000_000));
        assert_eq!(slot.owner_id, ESCROW_OWNER);
    }
}
