//! The wallet journal: an append-forever record of where the ISK went.
//!
//! tracker issue #1 -- *"isk: works, but shows no transaction history"*. The
//! balance has been live since `/giveisk` landed; what was missing is any
//! record of how it got that way.
//!
//! # Three things here are load-bearing and none of them is obvious
//!
//! **`amount` and `balance` are ISK x 10,000.** The columns are `DBTYPE_CY`
//! and the client's column getter divides by 10,000.0 on read (confirmed,
//! `docs/reference/dbrow.md` §3). Sending ISK directly would show every figure as
//! 1/10,000th of itself -- a wrong number that still looks like a number,
//! which is the worst kind.
//!
//! **The date must never be zero.** A `FILETIME` column in a packed `DBRow`
//! is read as UI8 and arrives as a Python `long` whatever its magnitude, so
//! it dodges the `FormatDateTime` magnitude trap that bites every loose
//! integer. But the branch it then takes feeds `time.strftime` a year-1601
//! struct, CPython 2 refuses years before 1900, and the resulting
//! **`ValueError` is not caught** by the panel's `except AttributeError`
//! (`transactionsPanel.py:170`). So a zero date does not blank one cell -- it
//! takes down the whole Journal tab. Mitigation is free: there is no honest
//! reason to write an entry with no date.
//!
//! **The container must stay a `CRowset`.** Every row is handed to
//! `utillib.KeyVal(row)`, which accepts a dict, a `blue.DBRow` or another
//! KeyVal and raises `TypeError` otherwise. And answering as a list of KeyVals
//! -- a shape every *reader* would accept -- would make `transactionDate` a
//! loose integer under 2^31, which is exactly the magnitude trap above: a
//! blank field, a traceback in the client's log, and nothing in ours.
//!
//! # The ledger log (E4, `R-ECON` §1.4 P2)
//!
//! Journal rows, market trades and closed market orders are **not** in the
//! world snapshot once a [`Log`] is attached ([`open`]): each row is
//! appended, as it is made, to a daily file `<world>.ledger/YYYY-MM-DD.tsv`
//! by a `ledger-writer` thread of its own (never E2's saver channel, whose
//! texts are whole worlds and coalesce). A load reads the files back, so
//! world + log equals the RAM state. Retention runs at the rollover
//! ([`on_rollover`], after `econ_ledger`'s fold) with one rule for RAM and
//! files ([`Rules`]): a row with no human party goes [`BOT_LEDGER_DAYS`]
//! after its day once folded; a human row is kept [`HUMAN_LEDGER_MONTHS`].
//! A market row is folded once its day is in `mkthist` (E5: the
//! price-history reader takes `mkthist` for every folded day), so a bot's
//! goes on the same `BOT_LEDGER_DAYS`; a human's keeps CP42's RAM window
//! (`HISTORY_DAYS`) and the log's six months.
//!
//! # What is deliberately not here
//!
//! Market transactions are a **different table** (`transactions_for` and
//! `corp::transactions` in `service/market.rs`), and a market trade books a row in both. A GM grant books
//! only a journal entry, because no item changed hands.

/// `refGMCashTransfer`. Verified three ways: the client's own
/// `accountingentrytypes.fsdbinary` (218 rows, read by `tools/fsd.py`), and
/// independently by two lanes deriving `entryTypeNameID == 59792 +
/// entryTypeID` from the localisation corpus, where 59795 is "GM Cash
/// Transfer".
pub const REF_GM_CASH_TRANSFER: i64 = 3;

/// The offset from an entry type id to its localised name's message id.
///
/// **Only valid for entry types 0..=108.** Read off the client's own
/// `accountingentrytypes.fsdbinary` (218 rows, `tools/fsd_dump.py rows
/// accountingentrytypes`), which carries `entryTypeID` and `entryTypeNameID`
/// as two columns of the same row: `entryTypeNameID == 59792 + entryTypeID`
/// holds for **all 106 rows with an id <= 108, and for no row above it**.
///
/// It matters because `entryTypeNameID: None` **blanks the Type column**:
/// `GetStaticData` overwrites `entryTypeName` with
/// `GetByMessageID(entryTypeNameID)`, and that is `''` for `None`.
pub const ENTRY_TYPE_NAME_ID_BASE: i64 = 59_792;

/// The highest entry type id for which [`ENTRY_TYPE_NAME_ID_BASE`] holds.
///
/// # This used to say 120, and 120 was wrong
///
/// The *message* block really is contiguous from 59792 to 59912 -- 121 ids,
/// no holes worth the name -- and reading that block as 121 consecutive
/// *entry type* ids is the mistake this constant was born with. The block is
/// shared by three disjoint runs of entry type ids:
///
/// | messages | entry types | what they are |
/// |---|---|---|
/// | 59792..=59900 | 0..=108 | the ISK wallet |
/// | 59901..=59910 | 10001..=10010 | the DUST 514 / console ISK block |
/// | 59911..=59912 | 11001..=11002 | the AUR block |
///
/// So entry type 119 is *not* 59911 "Modify AUR" and 120 is *not* 59912
/// "Respec payment"; those two messages belong to 11001 and 11002. 119 is
/// 503029 "Bid Reimbursement" and 120 is 503484 "Industry Facility Tax", per
/// the client's own table. The old cap therefore did not blank anything -- it
/// printed a **real string for the wrong entry type**, twelve times over
/// (109..=120), which is the one failure mode worse than an empty cell
/// because nothing about it looks broken.
pub const ENTRY_TYPE_NAME_ID_MAX: i64 = 108;

/// Entry type ids inside `0..=108` that the client's table does not define.
///
/// Not an oversight on our side: `accountingentrytypes.fsdbinary` has no row
/// for them, and the corpus has no message at `59792 + id` either (59892,
/// 59896 and 59897 are all absent from en-us). Two independent sources agree
/// the ids are vacant, so [`entry_type_name_id`] answers `None` rather than
/// hand the client an id it would render as the literal `[no messageID:
/// 59892]`.
const ENTRY_TYPE_RUN_GAPS: [i64; 3] = [100, 104, 105];

/// Entry types whose name ids were allocated **individually**, not from the
/// run.
///
/// # Why there are two mechanisms at all
///
/// Entry types 0..=108 are the 2003-era wallet, and their names were
/// allocated as one contiguous message block ([`ENTRY_TYPE_NAME_ID_BASE`]).
/// Everything added since got a message id from whatever the localisation
/// tool happened to be handing out that patch, so the ids here are scattered
/// from 234,494 to 1,046,599 with no arithmetic relating them to the entry
/// type. They can only be looked up.
///
/// # Why the cap must not simply be raised
///
/// `59792 + id` past the run lands either on an unrelated string (59913 is
/// `UI/Jukebox/Play`) or on nothing at all. A missing message is the louder
/// failure: the client renders the literal text `[no messageID: 59941]` into
/// the Type column, so raising the cap to cover entry type 149 would turn an
/// empty cell into visible garbage. (That is the same shape as the
/// unresolvable `UserError` key this project shipped once before, which
/// produced a fatal modal.) The answer is a lookup table, not a bigger cap.
///
/// # Provenance
///
/// Both columns come from one row of the client's own
/// `accountingentrytypes.fsdbinary` -- `entryTypeID` at +0 and
/// `entryTypeNameID` at +32, per `findings/pyd-schemas.tsv` -- so this is
/// build 3619879 truth and not a reconstruction. Every row was then checked
/// against the shipped localisation corpus (`tools/loctable.py id`): all
/// 112 message ids resolve to a real en-us string, and 49 of them carry a
/// `UI/Wallet/EntryTypeNames/<label>` path, which is the independent
/// cross-check. The comment on each row is that label (or, where the
/// namespace has none, the fsd `name` column) and the English text the client
/// will actually print.
///
/// Sorted by entry type id; [`entry_type_name_id`] binary-searches it.
const ENTRY_TYPE_NAME_IDS: [(i64, i64); 112] = [
    (109, 234494), // Lottery Give Away (no EntryTypeNames label; fsd name) -- 'Lottery Give Away'
    (111, 263475), // Aurum Token exchanged for Aur (no EntryTypeNames label; fsd name) -- 'Aurum Token exchanged for Aur'
    (112, 277685), // Datacore Fee (no EntryTypeNames label; fsd name) -- 'Datacore Fee'
    (113, 279064), // WarSurrenderWalletEntry -- 'War Surrender Fee'
    (114, 279063), // WarAllyWalletEntry -- 'War Ally Contract'
    (115, 285170), // bountyReimbursedTypeName -- 'Bounty Reimbursement'
    (116, 285169), // killRightFeeTypeName -- 'Kill Right'
    (117, 287844), // SecurityTagProcessingFee -- 'Fee for processing one or more security tags'
    (118, 503028), // IndustryTeamBid -- 'Industry Team Bid'
    (119, 503029), // IndustryTeamBidReimbursement -- 'Bid Reimbursement'
    (120, 503484), // industryFacilityTaxName -- 'Industry Facility Tax'
    (122, 507741), // infrastructureHubBillName -- 'Infrastructure Hub bill'
    (123, 510859), // assetSafetyTaxRef -- 'Asset safety recovery tax'
    (124, 514021), // opportunityRewardName -- 'Opportunity Reward'
    (125, 515360), // projectDiscoveryRewardName -- 'Project Discovery reward'
    (126, 515396), // projectDiscoveryRewardTax -- 'Corporation tax on Project Discovery rewards'
    (127, 312131), // ReprocessingTax -- 'Reprocessing Tax'
    (128, 312132), // JumpCloneActivationFee -- 'Jump Clone Activation Fee'
    (129, 522816), // operationRewardName -- 'Mission Reward'
    (131, 529365), // resouceWarsRewardName -- 'Mining Site Protection Money'
    (132, 530052), // DuelWagerEscrow -- 'Duel wager escrow payment'
    (133, 530053), // DuelWagerPayment -- 'Duel wager reward'
    (134, 530054), // DuelWagerRefund -- 'Duel wager refund'
    (135, 317046), // RefEntryTypeReactions -- 'Reactions'
    (136, 531920), // externalTradeFreeze -- 'External Trade Freeze'
    (137, 531921), // externalTradeThaw -- 'External Trade Thaw'
    (138, 531922), // externalTradeDelivery -- 'External Trade Delivery'
    (139, 539336), // seasonChallengeRewardName -- 'Challenge Completed'
    (140, 543036), // StructureGateJumpPaymentName -- 'Jump Bridge Fee'
    (141, 543737), // SkillPurchaseName -- 'Skill Purchase'
    (142, 545149), // itemTraderPaymentName -- 'Item Trade'
    (143, 552172), // HyperNetEntryFee -- 'HyperNode transaction'
    (144, 552175), // HyperNetPayout -- 'Completed HyperNet Relay Payout'
    (145, 552173), // HyperNetRelayFee -- 'HyperNet Relay Fee'
    (146, 552174), // HyperNetExpired -- 'HyperNode refund for expired HyperNet Offer'
    (147, 552388), // redeemedIskToken -- 'Redeemed ISK token'
    (148, 559010), // DailyChallengeRewardName -- 'Daily Challenge Completed'
    (149, 559228), // marketProviderTax -- 'SCC surcharge'
    (155, 571898), // ESS Main Bank Autopayment (no EntryTypeNames label; fsd name) -- 'ESS Escrow Payment'
    (156, 584301), // First Day ISK Bonus (no EntryTypeNames label; fsd name) -- 'First Day ISK Bonus'
    (166, 644856), // Under constructions (no EntryTypeNames label; fsd name) -- 'Building Materials Delivered'
    (167, 662530), // Alignment Based Gate Toll (no EntryTypeNames label; fsd name) -- 'Gate Toll'
    (168, 669909), // Alignment Based Zarzakh Stargate Toll (no EntryTypeNames label; fsd name) -- 'Zarzakh Stargate Toll'
    (170, 682461), // Corporation Projects Payment (no EntryTypeNames label; fsd name) -- 'Corporation Projects'
    (172, 697061), // Insurgency corruption contribution ISK reward (no EntryTypeNames label; fsd name) -- 'Insurgency corruption contribution reward'
    (173, 697062), // Insurgency suppression contribution ISK reward (no EntryTypeNames label; fsd name) -- 'Insurgency suppression contribution reward'
    (174, 698549), // Daily Goal Entry type (no EntryTypeNames label; fsd name) -- 'Daily Goal Reward'
    (175, 699236), // Daily Goal tax Entry type (no EntryTypeNames label; fsd name) -- 'Daily Goal Reward Tax'
    (178, 712104), // Cosmetic Market Component Item Purchase (no EntryTypeNames label; fsd name) -- 'Paragon Hub Design Element'
    (179, 720962), // Cosmetic Market Skin Sale Broker Fee (no EntryTypeNames label; fsd name) -- 'Paragon Hub Listing Fee'
    (180, 720963), // Cosmetic Market Skin Purchase (no EntryTypeNames label; fsd name) -- 'Paragon Hub SKIN purchase'
    (181, 720964), // Cosmetic Market Skin Sale (no EntryTypeNames label; fsd name) -- 'Paragon Hub SKIN sale'
    (182, 720965), // Cosmetic Market Skin Sale Tax (no EntryTypeNames label; fsd name) -- 'Paragon Hub Vendor Fee'
    (183, 721956), // Cosmetic Market Skin Transaction (no EntryTypeNames label; fsd name) -- 'Paragon Hub SKIN transaction'
    (184, 1010920), // Fee for SovHub to claim a local Skyhook (no EntryTypeNames label; fsd name) -- 'Skyhook claim fee'
    (185, 1011296), // Air Career Program Reward (no EntryTypeNames label; fsd name) -- 'AIR Career Program Reward'
    (186, 1011633), // Freelance Job Creation - Base Fee (no EntryTypeNames label; fsd name) -- 'Freelance Jobs Duration Fee'
    (187, 1011635), // Freelance Job Creation - Broadcasting Fee (no EntryTypeNames label; fsd name) -- 'Freelance Jobs Broadcasting Fee'
    (188, 1011637), // Freelance Job Creation - Rewards Deposit (no EntryTypeNames label; fsd name) -- 'Freelance Jobs Reward Escrow'
    (189, 1011653), // Freelance Job Contribution Payouts (no EntryTypeNames label; fsd name) -- 'Freelance Jobs Reward'
    (190, 1011655), // Freelance Job Closing Refunds (no EntryTypeNames label; fsd name) -- 'Freelance Jobs Escrow Refund'
    (191, 1014042), // Freelance Job Contribution Payouts Tax (no EntryTypeNames label; fsd name) -- 'Freelance Jobs Reward Corporation Tax'
    (192, 1017441), // PLEX Order Fee Cancellation Refund (no EntryTypeNames label; fsd name) -- 'GM Cash Transfer'
    (194, 1038647), // Market Security Tax (no EntryTypeNames label; fsd name) -- 'Exordium Market Tax'
    (195, 1039203), // Npc Bounty security Tax (no EntryTypeNames label; fsd name) -- 'Exordium Bounty Tax'
    (196, 1039622), // Industry Security Tax (no EntryTypeNames label; fsd name) -- 'Exordium Industrial Tax'
    (197, 1039790), // AGENTMISSIONSECURITYTAX (no EntryTypeNames label; fsd name) -- 'Exordium Mission Agent Tax'
    (198, 1040362), // Achievement (no EntryTypeNames label; fsd name) -- 'Achievement'
    (199, 1040363), // Achievement Category (no EntryTypeNames label; fsd name) -- 'Achievement'
    (200, 1041757), // CampaignObjectiveIskReward (no EntryTypeNames label; fsd name) -- 'Campaign Objective Reward'
    (201, 1046599), // CampaignObjectiveIskRewardTax (no EntryTypeNames label; fsd name) -- 'Campaign Objective Reward Tax'
    (10001, 59901), // Modify ISK (no EntryTypeNames label; fsd name) -- 'Modify ISK'
    (10002, 59902), // Primary Marketplace Purchase (no EntryTypeNames label; fsd name) -- 'Primary Marketplace Purchase'
    (10003, 59903), // Battle Reward (no EntryTypeNames label; fsd name) -- 'Battle Reward'
    (10004, 59904), // New Character Starting Funds (no EntryTypeNames label; fsd name) -- 'New Character Starting Funds'
    (10005, 59905), // Corporation Account Withdrawal (no EntryTypeNames label; fsd name) -- 'Corporation Account Withdrawal'
    (10006, 59906), // Corporation Account Deposit (no EntryTypeNames label; fsd name) -- 'Corporation Account Deposit'
    (10007, 59907), // Battle WP Win Reward (no EntryTypeNames label; fsd name) -- 'Battle WP Win Reward'
    (10008, 59908), // Battle WP Loss Reward (no EntryTypeNames label; fsd name) -- 'Battle WP Loss Reward'
    (10009, 59909), // Battle Win Reward (no EntryTypeNames label; fsd name) -- 'Battle Win Reward'
    (10010, 59910), // Battle Loss Reward (no EntryTypeNames label; fsd name) -- 'Battle Loss Reward'
    (10011, 570207), // Reset ISK for Character Reset (no EntryTypeNames label; fsd name) -- 'DUST Reset ISK for Character Reset'
    (10012, 283741), // District Contract Deposit (no EntryTypeNames label; fsd name) -- 'District Contract Deposit'
    (10013, 283742), // District Contract Deposit Refund (no EntryTypeNames label; fsd name) -- 'District Contract Deposit Refund'
    (10014, 283743), // District Contract Collateral (no EntryTypeNames label; fsd name) -- 'District Contract Collateral'
    (10015, 283744), // District Contract Collateral Refund (no EntryTypeNames label; fsd name) -- 'District Contract Collateral Refund'
    (10016, 283745), // District Contract Reward (no EntryTypeNames label; fsd name) -- 'District Contract Reward'
    (10017, 287027), // District Clone Transportation (no EntryTypeNames label; fsd name) -- 'District Clone Transportation'
    (10018, 287029), // District Clone Transportation Refund (no EntryTypeNames label; fsd name) -- 'District Clone Transportation Refund'
    (10019, 287031), // District Infrastructure (no EntryTypeNames label; fsd name) -- 'District Infrastructure'
    (10020, 287033), // District Clone Sales (no EntryTypeNames label; fsd name) -- 'District Clone Sales'
    (10021, 287035), // District Clone Purchase (no EntryTypeNames label; fsd name) -- 'District Clone Purchase'
    (10022, 288205), // Biomass Reward (no EntryTypeNames label; fsd name) -- 'Biomass Reward'
    (10023, 306849), // KillSwapReward -- 'ISK Swap Reward'
    (11001, 59911),  // Modify AUR (no EntryTypeNames label; fsd name) -- 'Modify AUR'
    (11002, 59912),  // Respec payment (no EntryTypeNames label; fsd name) -- 'Respec payment'
    (11003, 570204), // Entitlement (no EntryTypeNames label; fsd name) -- 'DUST Entitlement'
    (11004, 570205), // Reset Reimbursement (no EntryTypeNames label; fsd name) -- 'DUST Reset Reimbursement'
    (11005, 570206), // Reset AUR for Character Reset (no EntryTypeNames label; fsd name) -- 'DUST AUR for Character Reset'
    (12001, 505817), // dustDailyMissionCP -- 'Command Points from completing Daily Missions'
    (12002, 505819), // dustWarbargeCP -- 'Command points from warbarge'
    (12003, 505821), // dustDonationCP -- 'Command points from donation'
    (12004, 505823), // dustConsumeCPForClonePacks -- 'Command Point cost of purchasing Clone Packs'
    (12005, 505825), // dustConsumeCPForMovingClones -- 'Command Point cost of moving Clones'
    (12006, 505829), // dustConsumeCPForSellingClones -- 'Command Point cost of selling Clones'
    (12007, 505827), // dustConsumeCPForChangingReinforcement -- 'Command Point cost of changing Reinforcement Timers'
    (12008, 505831), // dustConsumeCPForChangingSurfaceInfrastructure -- 'Command Point cost of changing Surface Infrastructures'
    (13001, 505873), // dustDailyMissionDK -- 'District Kredits from DUST daily mission fininshed'
    (13002, 505875), // dustPlanetaryConquestDK -- 'District Kredits from Planetary Conquest activity'
    (13003, 505877), // dustConsumeDKForPurchase -- 'District Kredit cost of purchasing items'
    (13004, 505879), // dustConsumeDKForReroll -- 'Consuming District Kredits for rerolling market'
    (13005, 505960), // dustSellingClonesDK -- 'District Kredit increase because of selling clones'
];

/// The localised name's message id for an entry type, or `None`.
///
/// Two mechanisms, because the data has two: the arithmetic run for the old
/// entry types, then [`ENTRY_TYPE_NAME_IDS`] for the individually allocated
/// ones. See those items for why.
///
/// **`None` is the correct answer for anything not in either**, and it stays
/// that way. A `None` blanks the Type column; a guessed id either prints
/// another entry type's name or prints `[no messageID: N]`. An empty cell is
/// the only one of the three that does not lie.
pub fn entry_type_name_id(entry_type_id: i64) -> Option<i64> {
    if (0..=ENTRY_TYPE_NAME_ID_MAX).contains(&entry_type_id) {
        if ENTRY_TYPE_RUN_GAPS.contains(&entry_type_id) {
            return None;
        }
        return Some(ENTRY_TYPE_NAME_ID_BASE + entry_type_id);
    }
    ENTRY_TYPE_NAME_IDS
        .binary_search_by_key(&entry_type_id, |&(id, _)| id)
        .ok()
        .map(|i| ENTRY_TYPE_NAME_IDS[i].1)
}

/// 100-nanosecond ticks in one day.
const TICKS_PER_DAY: i64 = 864_000_000_000;

/// Days from 1601-01-01 (the FILETIME epoch) to 1970-01-01 (the Unix epoch).
const DAYS_1601_TO_1970: i64 = 134_774;

/// One journal entry.
///
/// The fields are the nine columns of `transactions_rowset`, in order, with
/// `amount` and `balance` held as **ISK** and scaled on the way out -- so
/// that the x10,000 lives in exactly one place instead of at every call site.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub transaction_id: i64,
    /// Blue FILETIME: 100ns ticks since 1601. Never zero; see the module docs.
    pub date: i64,
    pub entry_type_id: i64,
    /// Who the ISK came from. For a GM grant this is the system, not a player.
    pub owner_id_1: i64,
    /// Who it went to.
    pub owner_id_2: i64,
    pub reference_id: i64,
    /// ISK, signed. Positive is money in.
    pub amount: f64,
    /// The balance *after* this entry, so the journal reads as a running
    /// total.
    ///
    /// **Whose balance is the writer's to say, and it is only well defined
    /// for a one-sided entry.** Every entry today moves money between the
    /// system and one character, so this is that character's balance and
    /// [`Journal::select`]'s owner filter is enough to make every column
    /// right. The first entry that debits one player and credits another
    /// cannot carry one number for both, and at that point this field has to
    /// be recomputed per owner at select time. Recorded here rather than
    /// discovered then.
    pub balance: f64,
    pub description: String,
}

impl Entry {
    /// Whose wallet this row is: the payer for a debit, the payee for a
    /// credit. See [`Journal::select`].
    pub fn wallet(&self) -> i64 {
        if self.amount < 0.0 {
            self.owner_id_1
        } else {
            self.owner_id_2
        }
    }
}

/// The journal, newest last, plus the id allocator.
#[derive(Debug, Clone, Default)]
pub struct Journal {
    entries: Vec<Entry>,
    next_id: i64,
    /// Where each new row is appended (E4). Detached in tests and in a
    /// world with no `--world` file: the rows then ride the snapshot as
    /// they always did.
    log: Log,
}

impl Journal {
    pub fn new() -> Self {
        Journal {
            entries: Vec::new(),
            // Any positive value works; starting at 1 makes the first entry
            // readable in a log. The client reads `transactionID` only as an
            // identity and a sort key.
            next_id: 1,
            log: Log::default(),
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn next_id(&self) -> i64 {
        self.next_id
    }

    /// A journal read back from disk.
    pub fn restore(entries: Vec<Entry>, next_id: i64) -> Self {
        let highest = entries.iter().map(|e| e.transaction_id).max().unwrap_or(0);
        Journal {
            entries,
            next_id: next_id.max(highest + 1).max(1),
            log: Log::default(),
        }
    }

    /// The rows the world snapshot carries: none once a [`Log`] holds them
    /// (E4), every row otherwise.
    pub fn snapshot_rows(&self) -> Vec<Entry> {
        if self.log.is_attached() {
            Vec::new()
        } else {
            self.entries.clone()
        }
    }

    /// Never hand out an id below `next_id`: a snapshot whose rows are in
    /// the log still carries the allocator (and its C7 §3.4 leap).
    pub fn raise_next_id(&mut self, next_id: i64) {
        self.next_id = self.next_id.max(next_id);
    }

    /// The log every new row is appended to.
    pub fn log(&self) -> &Log {
        &self.log
    }

    /// Keep only the rows `keep` says; returns how many went.
    fn retain(&mut self, keep: impl Fn(&Entry) -> bool) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| keep(e));
        before - self.entries.len()
    }

    /// Append one entry and return its id.
    ///
    /// `date` is taken rather than read from the clock here so that the
    /// caller -- and a test -- decides, which is the seam
    /// `docs/archive/architecture-sweep.md` wants for everything time-shaped.
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &mut self,
        date: i64,
        entry_type_id: i64,
        owner_id_1: i64,
        owner_id_2: i64,
        reference_id: i64,
        amount: f64,
        balance: f64,
        description: &str,
    ) -> i64 {
        let transaction_id = self.next_id;
        self.next_id += 1;
        let e = Entry {
            transaction_id,
            date,
            entry_type_id,
            owner_id_1,
            owner_id_2,
            reference_id,
            amount,
            balance,
            description: description.to_owned(),
        };
        self.log.append(date, entry_line(&e));
        self.entries.push(e);
        transaction_id
    }

    /// The entries the wallet asked for.
    ///
    /// The client sends `(accountKey, year, month, isCorp)`. `(None, None)`
    /// means "recent" -- the default the window opens on -- and a concrete
    /// `(year, month)` is that calendar month. We answer every matching row in
    /// one go rather than paging, because the client pages at 100 locally.
    ///
    /// `now` is passed in for the same reason `record` takes a date.
    /// `owner` is **not optional and not a convenience.** This method took
    /// `(now, year, month)` and nothing else, so `account.GetTransactions`
    /// returned every entry in the world to every caller -- player B read
    /// player A's whole wallet history, with a running balance to match. It
    /// rendered, it looked authoritative, and nothing logged.
    pub fn select(
        &self,
        now: i64,
        year: Option<i64>,
        month: Option<i64>,
        owner: i64,
    ) -> Vec<&Entry> {
        // **The wallet a row belongs to, not either party to it.** Every
        // writer records one row per wallet that moved, with that wallet's
        // `balance`: a debit names its wallet as the payer (`owner_id_1`), a
        // credit as the payee (`owner_id_2`) -- `Registry::charge`, the
        // `/giveisk` grant and `service::market::move_isk` all do. This used
        // to accept either party, so in a trade between two characters each
        // saw the other's row *and the other's running balance* (found by the
        // P5.1 market lane, the first writer with a character on both sides).
        let mine = |e: &&Entry| e.wallet() == owner;
        match (year, month) {
            (Some(y), Some(m)) => self
                .entries
                .iter()
                .filter(mine)
                .filter(|e| {
                    let (ey, em, _) = civil_from_filetime(e.date);
                    ey == y && em == m
                })
                .collect(),
            // The window's own default. Thirty days back from now, which is
            // what the real server answers for an absent range.
            _ => {
                let cutoff = now - 30 * TICKS_PER_DAY;
                self.entries
                    .iter()
                    .filter(mine)
                    .filter(|e| e.date >= cutoff)
                    .collect()
            }
        }
    }
}

/// `(year, month, day)` from a blue FILETIME.
///
/// Written out because `std` has no calendar and `server/deps.allow` is empty
/// and stays empty. The algorithm is Howard Hinnant's `civil_from_days`, which
/// is exact for the whole proleptic Gregorian range and needs no tables.
///
/// It exists so that a player picking "March 2003" in the wallet's date combo
/// is answered honestly. Answering every row regardless of the range would
/// have been three lines, and would have shown today's grant under any month
/// the player chose -- a lie that only shows up when someone trusts it.
pub fn civil_from_filetime(ft: i64) -> (i64, i64, i64) {
    // Floor division, because a negative FILETIME is before 1601 and `/`
    // truncates towards zero.
    let ticks = ft;
    let mut days = ticks / TICKS_PER_DAY;
    if ticks % TICKS_PER_DAY < 0 {
        days -= 1;
    }
    civil_from_days(days - DAYS_1601_TO_1970)
}

/// `(year, month, day)` from days since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---- the ledger log (E4) -------------------------------------------------------

/// Days a row with no human party outlives its own UTC day, once
/// `econ_ledger` has folded it (owner decision O5-g: "a day after the
/// `[econ]` fold") **[A]**. Falsified by a GM or an `[econ]` question about
/// a bot's day that is already gone, or by a soak whose log still grows.
pub const BOT_LEDGER_DAYS: i64 = 1;

/// Calendar months a human row is kept before the current one: the
/// wallet's date combo offers the last 30 days plus five previous months
/// (`docs/reference/wallet-ledger.md`) **[V]**, so six months back covers every month
/// it can ask for.
pub const HUMAN_LEDGER_MONTHS: i64 = 6;

/// The record tag of a journal row, in the log as it was in the world file.
pub const JOURNAL: &str = "journal";

/// `<world>.ledger`: the log's directory beside the world file.
pub fn dir_of(world: &std::path::Path) -> std::path::PathBuf {
    let mut name = world.file_name().unwrap_or_default().to_os_string();
    name.push(".ledger");
    world.with_file_name(name)
}

/// One journal row as a log line: the world file's old `journal` record
/// (tag, nine fields, the description escaped), newline included.
pub fn entry_line(e: &Entry) -> String {
    format!(
        "{JOURNAL}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        e.transaction_id,
        e.date,
        e.entry_type_id,
        e.owner_id_1,
        e.owner_id_2,
        e.reference_id,
        e.amount,
        e.balance,
        crate::store::esc(&e.description)
    )
}

/// The inverse of [`entry_line`] for one line split on tabs.
fn parse_entry(f: &[&str]) -> Option<Entry> {
    let int = |i: usize| f.get(i)?.parse::<i64>().ok();
    let real = |i: usize| f.get(i)?.parse::<f64>().ok();
    (f.first() == Some(&JOURNAL)).then_some(())?;
    Some(Entry {
        transaction_id: int(1)?,
        date: int(2)?,
        entry_type_id: int(3)?,
        owner_id_1: int(4)?,
        owner_id_2: int(5)?,
        reference_id: int(6)?,
        amount: real(7)?,
        balance: real(8)?,
        description: crate::store::unesc(f.get(9).copied().unwrap_or("")),
    })
}

/// Which retention rule a row falls under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A journal row: folded once `econ_ledger`'s cursor has passed it.
    Journal,
    /// A trade or a closed order: folded by the CP42 rollover, and held in
    /// RAM while the market's window does.
    Market,
    /// C6B: a kill's line ([`crate::killmail::LOG_TAG`]): nothing folds it,
    /// so it goes by its date alone.
    Kill,
}

/// What retention needs to know of one log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub kind: Kind,
    pub id: i64,
    /// The row's date: a journal row's or a trade's own, a closed order's
    /// last state change (its issue date when it has none).
    pub date: i64,
    /// The characters whose row it is: a journal row's wallet
    /// ([`Entry::wallet`]), a trade's buyer and seller, an order's owner.
    pub owners: Vec<i64>,
    /// A closed order still holding escrow: kept, as the market keeps it.
    pub held: bool,
}

/// Read a log line's [`Meta`]: `journal` ([`entry_line`]) and the market's
/// `mkttrade` and `mktorder` records (`service/market.rs`, the world file's
/// own layout). `None` for anything else, which retention never drops.
pub fn meta(line: &str) -> Option<Meta> {
    let f: Vec<&str> = line.trim_end_matches('\n').split('\t').collect();
    let int = |i: usize| f.get(i)?.parse::<i64>().ok();
    let real = |i: usize| f.get(i)?.parse::<f64>().ok();
    match *f.first()? {
        JOURNAL => {
            let e = parse_entry(&f)?;
            Some(Meta {
                kind: Kind::Journal,
                id: e.transaction_id,
                date: e.date,
                owners: vec![e.wallet()],
                held: false,
            })
        }
        "mkttrade" => Some(Meta {
            kind: Kind::Market,
            id: int(1)?,
            date: int(2)?,
            owners: vec![int(7)?, int(8)?],
            held: false,
        }),
        "mktorder" => Some(Meta {
            kind: Kind::Market,
            id: int(1)?,
            date: match int(17)? {
                -1 => int(12)?,
                v => v,
            },
            owners: vec![int(2)?],
            held: real(15)? != 0.0,
        }),
        crate::killmail::LOG_TAG => Some(Meta {
            kind: Kind::Kill,
            id: int(1)?,
            date: int(2)?,
            // The victim, and the final blow when a character: a kill with a
            // human on it is kept as long as the human's other rows.
            owners: [int(4)?].into_iter().chain(int(6)).collect(),
            held: false,
        }),
        _ => None,
    }
}

/// Midnight UTC of the day `t` falls in.
fn day_start(t: i64) -> i64 {
    t.div_euclid(TICKS_PER_DAY) * TICKS_PER_DAY
}

/// `year * 12 + month - 1` of a FILETIME.
fn month_index(t: i64) -> i64 {
    let (y, m, _) = civil_from_filetime(t);
    y * 12 + m - 1
}

/// The retention rule, one for RAM and for the files: what a rollover at
/// `now` drops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    pub now: i64,
    /// Every character that is not a bot.
    pub humans: std::collections::BTreeSet<i64>,
    /// `econ_ledger`'s cursor: journal rows at or below it are folded.
    pub cursor: i64,
    /// The first instant whose market rows are not yet folded into
    /// `mkthist` (`Registry::market_ledger_from`: today's midnight, E5).
    pub market_from: i64,
}

impl Rules {
    /// The rules of a rollover at `now` in `reg`.
    pub fn of(reg: &crate::service::Registry, now: i64) -> Self {
        Rules {
            now,
            humans: reg
                .config()
                .characters
                .iter()
                .filter(|c| !c.is_bot)
                .map(|c| c.character_id)
                .collect(),
            cursor: crate::econ_ledger::cursor(reg),
            market_from: reg.market_ledger_from(now),
        }
    }

    /// Whether a human row dated `date` is past the client's window.
    pub fn human_expired(&self, date: i64) -> bool {
        month_index(date) < month_index(self.now) - HUMAN_LEDGER_MONTHS
    }

    /// Whether a bot row's day ended [`BOT_LEDGER_DAYS`] or more ago.
    pub fn bot_expired(&self, date: i64) -> bool {
        day_start(date) + (BOT_LEDGER_DAYS + 1) * TICKS_PER_DAY <= self.now
    }

    /// Whether the row `m` describes goes.
    pub fn drops(&self, m: &Meta) -> bool {
        if m.owners.iter().any(|o| self.humans.contains(o)) {
            return self.human_expired(m.date);
        }
        self.bot_expired(m.date)
            && match m.kind {
                Kind::Journal => m.id <= self.cursor,
                Kind::Market => !m.held && m.date < self.market_from,
                Kind::Kill => true,
            }
    }

    /// [`Self::drops`] for a journal row in RAM.
    pub fn drops_entry(&self, e: &Entry) -> bool {
        self.drops(&Meta {
            kind: Kind::Journal,
            id: e.transaction_id,
            date: e.date,
            owners: vec![e.wallet()],
            held: false,
        })
    }
}

/// What the writer thread is asked to do, in order.
enum Msg {
    /// Append one line (newline included) to the file of `date`'s day.
    Row { date: i64, line: String },
    /// Rewrite every file without the rows the rules drop.
    Prune(Rules),
    /// Answer once every message before this one is on disk.
    Sync(std::sync::mpsc::Sender<()>),
}

/// The writer thread's handle. Dropping the last one closes the channel and
/// joins the thread, so every row sent is written first.
struct Writer {
    tx: Option<std::sync::mpsc::Sender<Msg>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Writer {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The append-only ledger log: a handle on its writer thread, shared by the
/// journal and the market book. Detached (the default) it drops every row,
/// and the snapshot carries them instead.
///
/// Not state: never compared (two books are equal whatever their logs) and
/// cloned by sharing.
#[derive(Clone, Default)]
pub struct Log(Option<std::sync::Arc<Writer>>);

impl std::fmt::Debug for Log {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.is_attached() {
            "Log(attached)"
        } else {
            "Log(detached)"
        })
    }
}

impl PartialEq for Log {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Log {
    /// Start a writer thread appending under `dir`.
    pub fn spawn(dir: &std::path::Path) -> std::io::Result<Log> {
        std::fs::create_dir_all(dir)?;
        let (tx, rx) = std::sync::mpsc::channel();
        let dir = dir.to_path_buf();
        let thread = std::thread::Builder::new()
            .name("ledger-writer".to_owned())
            .spawn(move || run(&dir, &rx))?;
        Ok(Log(Some(std::sync::Arc::new(Writer {
            tx: Some(tx),
            thread: Some(thread),
        }))))
    }

    pub fn is_attached(&self) -> bool {
        self.0.is_some()
    }

    fn send(&self, m: Msg) -> bool {
        self.0
            .as_ref()
            .and_then(|w| w.tx.as_ref())
            .is_some_and(|tx| tx.send(m).is_ok())
    }

    /// Append `line` (newline included) to the file of `date`'s day.
    pub fn append(&self, date: i64, line: String) {
        self.send(Msg::Row { date, line });
    }

    /// Drop, from every file, the rows `rules` drops.
    pub fn prune(&self, rules: Rules) {
        self.send(Msg::Prune(rules));
    }

    /// Wait until every row sent so far is on disk.
    pub fn sync(&self) {
        let (ack, done) = std::sync::mpsc::channel();
        if self.send(Msg::Sync(ack)) {
            let _ = done.recv();
        }
    }
}

/// `YYYY-MM-DD.tsv`: the file of `date`'s UTC day.
pub fn file_name(date: i64) -> String {
    let (y, m, d) = civil_from_filetime(date);
    format!("{y:04}-{m:02}-{d:02}.tsv")
}

/// The writer thread: drain what is queued, append it day by day with one
/// `fsync` a file, and run prunes and syncs in their place in the order.
fn run(dir: &std::path::Path, rx: &std::sync::mpsc::Receiver<Msg>) {
    let mut pending: std::collections::BTreeMap<String, String> = Default::default();
    let mut failed = false;
    let mut flush = |pending: &mut std::collections::BTreeMap<String, String>| {
        for (name, text) in std::mem::take(pending) {
            if let Err(e) = append(&dir.join(&name), &text) {
                if !failed {
                    crate::errln!(
                        "[ledger] cannot append to {name}: {e} (rows are lost until it can)"
                    );
                }
                failed = true;
            } else {
                failed = false;
            }
        }
    };
    while let Ok(first) = rx.recv() {
        let mut next = Some(first);
        while let Some(m) = next {
            match m {
                Msg::Row { date, line } => {
                    pending.entry(file_name(date)).or_default().push_str(&line)
                }
                Msg::Prune(rules) => {
                    flush(&mut pending);
                    match prune_dir(dir, &rules) {
                        Ok(n) if n > 0 => crate::outln!("[ledger] pruned {n} rows"),
                        Ok(_) => {}
                        Err(e) => crate::errln!("[ledger] prune failed: {e}"),
                    }
                }
                Msg::Sync(ack) => {
                    flush(&mut pending);
                    let _ = ack.send(());
                }
            }
            next = rx.try_recv().ok();
        }
        flush(&mut pending);
    }
}

fn append(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_data()
}

/// The log's day files, oldest first.
fn day_files(dir: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
    let mut v: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "tsv"))
        .collect();
    v.sort();
    Ok(v)
}

/// Rewrite each file that holds a row `rules` drops (tmp + fsync +
/// rename), removing it when nothing is left. A line [`meta`] cannot read
/// is kept: retention never destroys what it does not understand.
fn prune_dir(dir: &std::path::Path, rules: &Rules) -> std::io::Result<usize> {
    let mut dropped = 0;
    for path in day_files(dir)? {
        let text = std::fs::read_to_string(&path)?;
        let mut kept = String::with_capacity(text.len());
        let mut n = 0;
        for line in text.split_inclusive('\n') {
            if meta(line).is_some_and(|m| rules.drops(&m)) {
                n += 1;
            } else {
                kept.push_str(line);
            }
        }
        if n == 0 {
            continue;
        }
        dropped += n;
        if kept.is_empty() {
            std::fs::remove_file(&path)?;
        } else {
            let tmp = path.with_extension("tsv.tmp");
            {
                use std::io::Write as _;
                let mut f = std::fs::File::create(&tmp)?;
                f.write_all(kept.as_bytes())?;
                f.sync_all()?;
            }
            std::fs::rename(&tmp, &path)?;
        }
    }
    if dropped > 0
        && let Ok(d) = std::fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    Ok(dropped)
}

/// Every line of every day file, oldest file first. A file whose last line
/// was torn by a crash mid-append gets its newline now, so the next row
/// appended cannot glue onto it; the torn line itself fails to parse and is
/// skipped by its reader.
fn read_lines(dir: &std::path::Path) -> std::io::Result<Vec<String>> {
    let mut lines = Vec::new();
    for path in day_files(dir)? {
        let text = std::fs::read_to_string(&path)?;
        if !text.is_empty() && !text.ends_with('\n') {
            append(&path, "\n")?;
        }
        lines.extend(text.lines().filter(|l| !l.is_empty()).map(str::to_owned));
    }
    Ok(lines)
}

/// The file in a log's directory naming the world it belongs to.
const STAMP: &str = "world";

/// Make `dir` this world's log: a directory stamped with another world's
/// seed and birth (the world file was deleted to start over, and its log
/// was left behind) is moved aside to `<dir>.stale-<birth>` rather than
/// loaded into a world whose characters it never knew.
fn claim(dir: &std::path::Path, world: &crate::delta::World) -> std::io::Result<()> {
    let stamp = format!("{} {}\n", world.seed, world.birth);
    let path = dir.join(STAMP);
    match std::fs::read_to_string(&path) {
        Ok(s) if s == stamp => return Ok(()),
        Ok(s) => {
            let birth = s.split_whitespace().nth(1).unwrap_or("unknown");
            let mut aside = dir.as_os_str().to_os_string();
            aside.push(format!(".stale-{birth}"));
            std::fs::rename(dir, &aside)?;
            crate::outln!(
                "[ledger] {} belonged to another world; moved to {}",
                dir.display(),
                std::path::Path::new(&aside).display()
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(&path, stamp)
}

/// **Attach the ledger log at `dir` to `reg`** (`server.rs`, after the
/// snapshot is applied and before anything is journalled).
///
/// 1. Every row in the files is read: journal rows by id, the last line of
///    an id winning; market lines go to the book
///    (`Registry::market_ledger_open`).
/// 2. **A pre-E4 world migrates**: a journal row, trade or closed order the
///    snapshot holds and the log does not is appended to the log, and the
///    world is marked dirty so the next save drops it from the file.
/// 3. RAM keeps what it would hold had the process never stopped: the
///    rows [`Rules`] would have dropped at the last rollover are left out.
/// 4. From here on every new row is appended as it is made.
///
/// Returns the rows read from the files.
pub fn open(reg: &mut crate::service::Registry, dir: &std::path::Path) -> std::io::Result<usize> {
    claim(dir, reg.world())?;
    let lines = read_lines(dir)?;
    let read = lines.len();
    let log = Log::spawn(dir)?;
    let mut journal: std::collections::BTreeMap<i64, Entry> = Default::default();
    let mut market = Vec::new();
    let mut torn = 0;
    for line in &lines {
        let f: Vec<&str> = line.split('\t').collect();
        match f.first().copied() {
            Some(JOURNAL) => match parse_entry(&f) {
                Some(e) => {
                    journal.insert(e.transaction_id, e);
                }
                None => torn += 1,
            },
            Some("mkttrade" | "mktorder") => market.push(line.as_str()),
            // SOC-C6: the economy's copy of a kill; its report is a delta row.
            Some(crate::killmail::LOG_TAG) => {}
            _ => torn += 1,
        }
    }
    let mut migrated = 0;
    for e in reg.journal().entries() {
        if let std::collections::btree_map::Entry::Vacant(v) = journal.entry(e.transaction_id) {
            log.append(e.date, entry_line(e));
            v.insert(e.clone());
            migrated += 1;
        }
    }
    let at = reg.world().last_rollover;
    let (m_migrated, m_torn) = reg.market_ledger_open(&market, &log, at);
    migrated += m_migrated;
    torn += m_torn;
    let rules = Rules::of(reg, at);
    let kept: Vec<Entry> = journal
        .into_values()
        .filter(|e| !rules.drops_entry(e))
        .collect();
    let next = reg.journal().next_id();
    let j = reg.journal_mut();
    *j = Journal::restore(kept, next);
    j.log = log;
    if migrated > 0 {
        reg.mark_dirty();
    }
    crate::outln!(
        "[ledger] {}: {read} rows read, {migrated} migrated from the world file, {torn} unreadable lines skipped",
        dir.display()
    );
    Ok(read)
}

/// **The rollover step** (`rollover_steps::STEPS`, after `econ_ledger`):
/// drop from RAM the journal rows [`Rules`] drops at `d.now`, and ask the
/// writer to drop them, and the market's, from the files. The market's RAM
/// rows went in the CP42 step before this one. Its count is the RAM rows
/// dropped.
pub fn on_rollover(reg: &mut crate::service::Registry, d: &crate::rollover::Day) -> usize {
    let rules = Rules::of(reg, d.now);
    let dropped = reg.journal_mut().retain(|e| !rules.drops_entry(e));
    if dropped > 0 {
        reg.mark_dirty();
    }
    reg.journal().log().prune(rules);
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2003-05-06, the day EVE launched.
    ///
    /// **Read off `Character::default()` rather than written out here**, so
    /// that this is a cross-check against a FILETIME the codebase already
    /// relies on instead of two copies of one number. That distinction earned
    /// itself immediately: the first draft of this test hardcoded a value ten
    /// days adrift, and the disagreement looked at first like a bug in the
    /// calendar rather than in the test.
    fn eve_launch() -> i64 {
        crate::model::Character::default().create_date_time
    }

    #[test]
    fn the_calendar_agrees_with_dates_we_already_assert_elsewhere() {
        let launch = eve_launch();
        assert_eq!(civil_from_filetime(launch), (2003, 5, 6));
        // The epoch itself, and the Unix epoch, which pin both constants.
        assert_eq!(civil_from_filetime(0), (1601, 1, 1));
        assert_eq!(
            civil_from_filetime(DAYS_1601_TO_1970 * TICKS_PER_DAY),
            (1970, 1, 1)
        );
        // A leap day, which is where an off-by-one in the era arithmetic
        // would show up and nowhere else.
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn a_month_query_answers_that_month_and_not_another() {
        let mut j = Journal::new();
        j.record(
            eve_launch(),
            REF_GM_CASH_TRANSFER,
            1,
            2,
            0,
            100.0,
            100.0,
            "",
        );
        // One year later, to the tick.
        let later = eve_launch() + 365 * TICKS_PER_DAY;
        j.record(later, REF_GM_CASH_TRANSFER, 1, 2, 0, 50.0, 150.0, "");

        assert_eq!(j.select(later, Some(2003), Some(5), 2).len(), 1);
        assert_eq!(j.select(later, Some(2004), Some(5), 2).len(), 1);
        assert_eq!(
            j.select(later, Some(2003), Some(6), 2).len(),
            0,
            "a month with no entries is empty, not everything"
        );
    }

    /// **The leak, as a test.** `select` took no owner at all, so
    /// `account.GetTransactions` handed every player the whole world's wallet
    /// history -- with a running balance column to match. Nothing logged and
    /// it rendered as though it were theirs.
    #[test]
    fn the_journal_only_answers_the_owners_own_rows() {
        const SYSTEM: i64 = 1;
        const ALICE: i64 = 90_000_001;
        const BOB: i64 = 90_000_002;
        let now = eve_launch();
        let mut j = Journal::new();
        // Each grant carries the *recipient's* balance after it, which is
        // what makes the filter sufficient: see `Entry::balance`.
        j.record(now, REF_GM_CASH_TRANSFER, SYSTEM, ALICE, 0, 1.0, 1.0, "a1");
        j.record(now, REF_GM_CASH_TRANSFER, SYSTEM, BOB, 0, 5.0, 5.0, "b1");
        j.record(now, REF_GM_CASH_TRANSFER, SYSTEM, ALICE, 0, 1.0, 2.0, "a2");

        let alice: Vec<&str> = j
            .select(now, None, None, ALICE)
            .iter()
            .map(|e| e.description.as_str())
            .collect();
        assert_eq!(alice, vec!["a1", "a2"], "Bob's grant is not Alice's row");
        // And the running total reads correctly for each of them, which a
        // row filter alone would not guarantee if an entry were two-sided.
        assert_eq!(
            j.select(now, None, None, ALICE).last().map(|e| e.balance),
            Some(2.0)
        );
        assert_eq!(
            j.select(now, None, None, BOB).last().map(|e| e.balance),
            Some(5.0)
        );
        assert!(
            j.select(now, None, None, 404).is_empty(),
            "an owner with no entries gets none, never everyone's"
        );
        // A transfer is two rows, one per wallet, and **each party sees only
        // their own**: Bob must not read Alice's debit, which carries Alice's
        // balance.
        j.record(now, REF_GM_CASH_TRANSFER, ALICE, BOB, 0, -1.0, 1.0, "t");
        j.record(now, REF_GM_CASH_TRANSFER, ALICE, BOB, 0, 1.0, 6.0, "t");
        assert_eq!(j.select(now, None, None, ALICE).len(), 3);
        let bobs = j.select(now, None, None, BOB);
        assert_eq!(bobs.len(), 2);
        assert_eq!(
            bobs.last().map(|e| e.balance),
            Some(6.0),
            "Bob's row carries Bob's balance, never Alice's"
        );
    }

    #[test]
    fn the_default_range_is_thirty_days_and_excludes_older_rows() {
        let mut j = Journal::new();
        let now = eve_launch();
        j.record(now, REF_GM_CASH_TRANSFER, 1, 2, 0, 1.0, 1.0, "today");
        j.record(
            now - 29 * TICKS_PER_DAY,
            REF_GM_CASH_TRANSFER,
            1,
            2,
            0,
            1.0,
            2.0,
            "inside",
        );
        j.record(
            now - 31 * TICKS_PER_DAY,
            REF_GM_CASH_TRANSFER,
            1,
            2,
            0,
            1.0,
            3.0,
            "outside",
        );
        let got = j.select(now, None, None, 2);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|e| e.description != "outside"));
    }

    #[test]
    fn ids_are_positive_and_unique_and_the_order_is_oldest_first() {
        let mut j = Journal::new();
        let a = j.record(eve_launch(), 3, 1, 2, 0, 1.0, 1.0, "a");
        let b = j.record(eve_launch(), 3, 1, 2, 0, 1.0, 2.0, "b");
        assert!(a > 0 && b > a);
        assert_eq!(
            j.entries()
                .iter()
                .map(|e| e.description.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn the_gm_grant_entry_type_resolves_to_a_name_the_client_ships() {
        // 59795 is "GM Cash Transfer" in the client's own corpus.
        assert_eq!(entry_type_name_id(REF_GM_CASH_TRANSFER), Some(59_795));
    }

    #[test]
    fn the_arithmetic_run_holds_at_both_of_its_ends_and_stops_at_the_right_one() {
        // 0 "Undefined" and 108 "PLEX sold for Aurum" are the first and last
        // rows of the run in `accountingentrytypes.fsdbinary`.
        assert_eq!(entry_type_name_id(0), Some(59_792));
        assert_eq!(entry_type_name_id(108), Some(59_900));
        // 109 is where the arithmetic dies. Its real name id is 234494
        // "Lottery Give Away"; `59792 + 109` is 59901 "Modify ISK", which
        // belongs to entry type 10001.
        assert_eq!(entry_type_name_id(109), Some(234_494));
        assert_eq!(entry_type_name_id(10_001), Some(59_901));
    }

    /// The regression this file was opened for.
    ///
    /// The cap used to be 120, so these twelve ids answered `59792 + id` --
    /// twelve real strings attached to the wrong entry type. 119 printed
    /// "Modify AUR" and 120 printed "Respec payment", which are 11001 and
    /// 11002.
    #[test]
    fn the_twelve_entry_types_the_old_cap_mislabelled_now_answer_their_own_names() {
        for (id, want) in [
            (109, 234_494),
            (111, 263_475),
            (112, 277_685),
            (113, 279_064),
            (114, 279_063),
            (115, 285_170),
            (116, 285_169),
            (117, 287_844),
            (118, 503_028),
            (119, 503_029),
            (120, 503_484),
        ] {
            assert_eq!(
                entry_type_name_id(id),
                Some(want),
                "entry type {id} must not fall back to the arithmetic run"
            );
            assert_ne!(entry_type_name_id(id), Some(ENTRY_TYPE_NAME_ID_BASE + id));
        }
        // 110 has no row in the client's table at all, so it is unknown --
        // not 59902, which belongs to entry type 10002.
        assert_eq!(entry_type_name_id(110), None);
    }

    #[test]
    fn every_row_of_the_table_is_reachable_through_the_lookup() {
        for (id, name_id) in ENTRY_TYPE_NAME_IDS {
            assert_eq!(
                entry_type_name_id(id),
                Some(name_id),
                "entry type {id} is in the table but did not resolve"
            );
        }
        // The highest one, spelled out, so a silent truncation of the array
        // fails here too: 13005 is the DUST "Selling Clones DK" entry.
        assert_eq!(entry_type_name_id(13_005), Some(505_960));
        // And the one the task was raised over: 149 "SCC surcharge".
        assert_eq!(entry_type_name_id(149), Some(559_228));
    }

    #[test]
    fn the_table_is_sorted_by_id_and_has_no_duplicates() {
        // Both are load-bearing: `binary_search_by_key` silently returns the
        // wrong row on an unsorted array, and a duplicate id means two
        // sources disagreed and nobody noticed.
        for w in ENTRY_TYPE_NAME_IDS.windows(2) {
            assert!(
                w[0].0 < w[1].0,
                "entry type ids {} and {} are out of order or duplicated",
                w[0].0,
                w[1].0
            );
        }
    }

    #[test]
    fn the_table_and_the_arithmetic_run_do_not_overlap_or_contradict() {
        for (id, name_id) in ENTRY_TYPE_NAME_IDS {
            assert!(
                id > ENTRY_TYPE_NAME_ID_MAX,
                "entry type {id} is inside the run and must not also be in the table"
            );
            // Nothing in the table is just the formula in disguise; if one
            // row were, it would belong in the run instead.
            assert_ne!(
                name_id,
                ENTRY_TYPE_NAME_ID_BASE + id,
                "entry type {id} restates the arithmetic run"
            );
        }
    }

    #[test]
    fn an_entry_type_we_cannot_name_is_none_rather_than_a_guess() {
        // Negative, the gaps inside the run, the first id past the run with
        // no row, and ids beyond the end of the table.
        for id in [-1, 100, 104, 105, 110, 121, 130, 202, 999, 20_000] {
            assert_eq!(
                entry_type_name_id(id),
                None,
                "entry type {id} has no name we can prove, so it must blank the \
                 Type column rather than print someone else's name"
            );
        }
    }

    // ---- the ledger log (E4) -----------------------------------------------

    use crate::model::{Character, SandboxConfig};
    use crate::service::Registry;

    /// Invented ids.
    const HUMAN: i64 = 90_000_001;
    const BOT: i64 = 90_000_002;

    /// The FILETIME of `y-m-d 00:00` UTC (Hinnant's `days_from_civil`).
    fn ft(y: i64, m: i64, d: i64) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        (era * 146_097 + doe - 719_468 + DAYS_1601_TO_1970) * TICKS_PER_DAY
    }

    fn entry(id: i64, date: i64, wallet: i64) -> Entry {
        Entry {
            transaction_id: id,
            date,
            entry_type_id: REF_GM_CASH_TRANSFER,
            owner_id_1: 1,
            owner_id_2: wallet,
            reference_id: 0,
            amount: 5.0,
            balance: 5.0,
            description: "a\tb".to_owned(),
        }
    }

    fn rules(now: i64, cursor: i64) -> Rules {
        Rules {
            now,
            humans: [HUMAN].into(),
            cursor,
            market_from: now - 30 * TICKS_PER_DAY,
        }
    }

    /// A human and a bot, both docked nowhere: enough for the rules.
    fn world() -> Registry {
        Registry::new(SandboxConfig {
            characters: vec![
                Character {
                    character_id: HUMAN,
                    ship_id: 100_000_001,
                    ..Character::default()
                },
                Character {
                    character_id: BOT,
                    ship_id: 100_000_002,
                    is_bot: true,
                    ..Character::default()
                },
            ],
            ..SandboxConfig::default()
        })
    }

    /// Every line of every file of the log, oldest file first.
    fn on_disk(dir: &std::path::Path) -> Vec<String> {
        read_lines(dir).expect("reads")
    }

    #[test]
    fn the_test_calendar_agrees_with_the_one_under_test() {
        assert_eq!(civil_from_filetime(ft(2026, 3, 1)), (2026, 3, 1));
        assert_eq!(civil_from_filetime(ft(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(file_name(ft(2026, 10, 26) + 5), "2026-10-26.tsv");
    }

    #[test]
    fn a_journal_line_round_trips_and_names_its_wallet() {
        let e = entry(7, ft(2026, 1, 2), BOT);
        let line = entry_line(&e);
        let f: Vec<&str> = line.trim_end().split('\t').collect();
        assert_eq!(parse_entry(&f), Some(e.clone()));
        let m = meta(&line).expect("a journal line");
        assert_eq!(
            (m.kind, m.id, m.date, m.owners),
            (Kind::Journal, 7, e.date, vec![BOT])
        );
        assert_eq!(meta("mkthist\t1\t2\t3"), None, "not a row retention drops");
    }

    /// A bot row goes once its day ended `BOT_LEDGER_DAYS` ago **and**
    /// `econ_ledger` has folded it; not a tick before either.
    #[test]
    fn a_bot_row_is_dropped_after_bot_ledger_days_and_after_its_day_is_folded() {
        let day = ft(2026, 5, 10);
        let row = entry(3, day + 15 * 3_600 * 10_000_000, BOT);
        let gone_at = day + (BOT_LEDGER_DAYS + 1) * TICKS_PER_DAY;
        assert!(!rules(gone_at - 1, 3).drops_entry(&row), "too young");
        assert!(rules(gone_at, 3).drops_entry(&row));
        assert!(!rules(gone_at, 2).drops_entry(&row), "not folded yet");
        // A bot's market row also waits for the market's own window.
        let trade = Meta {
            kind: Kind::Market,
            id: 9,
            date: row.date,
            owners: vec![BOT, 1_000_001],
            held: false,
        };
        let mut r = rules(gone_at, 0);
        r.market_from = day;
        assert!(!r.drops(&trade), "still in the market's RAM window");
        r.market_from = day + TICKS_PER_DAY;
        assert!(r.drops(&trade));
        assert!(
            !r.drops(&Meta {
                held: true,
                ..trade
            }),
            "escrow held"
        );
    }

    /// A human row is kept for the current month and `HUMAN_LEDGER_MONTHS`
    /// before it, whatever `econ_ledger`'s cursor says.
    #[test]
    fn a_human_row_is_kept_for_human_ledger_months() {
        let row = entry(1, ft(2026, 1, 31), HUMAN);
        let kept_until = ft(2026, 1 + HUMAN_LEDGER_MONTHS, 28);
        assert!(!rules(kept_until, i64::MAX).drops_entry(&row));
        let first_gone = ft(2026, 2 + HUMAN_LEDGER_MONTHS, 1);
        assert!(rules(first_gone, i64::MAX).drops_entry(&row));
        // A trade with a human on one side is a human row.
        let trade = Meta {
            kind: Kind::Market,
            id: 4,
            date: row.date,
            owners: vec![BOT, HUMAN],
            held: false,
        };
        assert!(!rules(kept_until, 0).drops(&trade));
        assert!(rules(first_gone, 0).drops(&trade));
    }

    /// Rows land in the file of their day; a prune rewrites only what it
    /// drops from, and removes a file left empty.
    #[test]
    fn the_writer_appends_by_day_and_prunes_on_disk() {
        let dir = crate::testkit::scratch_dir("e4-prune");
        let log = Log::spawn(dir.as_ref()).expect("spawns");
        let (d1, d2) = (ft(2026, 5, 10), ft(2026, 5, 11));
        let rows = [entry(1, d1, BOT), entry(2, d1, HUMAN), entry(3, d2, BOT)];
        for e in &rows {
            log.append(e.date, entry_line(e));
        }
        log.sync();
        let names: Vec<String> = day_files(dir.as_ref())
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["2026-05-10.tsv", "2026-05-11.tsv"]);
        // Day 2 is still young; day 1's bot row goes, its human row stays.
        log.prune(rules(d2 + (BOT_LEDGER_DAYS + 1) * TICKS_PER_DAY - 1, 3));
        log.sync();
        let lines = on_disk(dir.as_ref());
        let ids: Vec<i64> = lines.iter().map(|l| meta(l).unwrap().id).collect();
        assert_eq!(ids, [2, 3]);
        // Then day 2's: its file is gone.
        log.prune(rules(d2 + (BOT_LEDGER_DAYS + 1) * TICKS_PER_DAY, 3));
        log.sync();
        assert_eq!(day_files(dir.as_ref()).unwrap().len(), 1);
    }

    /// Dropping the last handle joins the writer, so every row sent is on
    /// disk without a `sync`.
    #[test]
    fn dropping_the_log_writes_every_row_sent() {
        let dir = crate::testkit::scratch_dir("e4-drop");
        let log = Log::spawn(dir.as_ref()).expect("spawns");
        for id in 1..=50 {
            log.append(
                ft(2026, 5, 10),
                entry_line(&entry(id, ft(2026, 5, 10), HUMAN)),
            );
        }
        drop(log);
        assert_eq!(on_disk(dir.as_ref()).len(), 50);
    }

    /// A journal row goes to the log and never into the snapshot; the
    /// snapshot plus the log reloads to the same journal, allocator
    /// included.
    #[test]
    fn journal_rows_go_to_the_log_and_a_world_plus_log_reload_equals_ram() {
        let dir = crate::testkit::scratch_dir("e4-journal");
        let mut w = world();
        open(&mut w, dir.as_ref()).expect("opens");
        let now = crate::pyutil::filetime_now();
        w.journal_mut()
            .record(now, 3, 1, HUMAN, 0, 10.0, 10.0, "one");
        w.journal_mut()
            .record(now, 3, 1, BOT, 0, 20.0, 20.0, "two\tthree");
        let snap = w.to_snapshot();
        assert!(snap.journal.is_empty(), "never in the snapshot");
        w.journal().log().sync();
        let mut back = world();
        back.apply_snapshot(snap);
        open(&mut back, dir.as_ref()).expect("reopens");
        assert_eq!(back.journal().entries(), w.journal().entries());
        assert_eq!(back.journal().next_id(), w.journal().next_id());
        // And a row made after the reload is logged too.
        back.journal_mut()
            .record(now, 3, 1, HUMAN, 0, 1.0, 11.0, "");
        back.journal().log().sync();
        assert_eq!(on_disk(dir.as_ref()).len(), 3);
    }

    /// A world file from before E4 carries its journal rows: they load, go
    /// into the log, and the next snapshot drops them.
    #[test]
    fn a_pre_e4_world_with_journal_rows_in_the_snapshot_loads() {
        let dir = crate::testkit::scratch_dir("e4-migrate");
        let mut old = world();
        let now = crate::pyutil::filetime_now();
        old.journal_mut()
            .record(now, 3, 1, HUMAN, 0, 10.0, 10.0, "before E4");
        let snap = old.to_snapshot();
        assert_eq!(
            snap.journal.len(),
            1,
            "a detached journal rides the snapshot"
        );
        let file = dir.as_ref().join("world.txt");
        let store = crate::store::FileStore::new(&file);
        crate::store::Store::save(&store, &snap).expect("saves");
        let loaded = crate::store::Store::load(&store).unwrap().expect("a world");
        let mut w = world();
        w.apply_snapshot(loaded);
        let ledger = dir_of(&file);
        assert_eq!(ledger, dir.as_ref().join("world.txt.ledger"));
        open(&mut w, &ledger).expect("opens");
        assert_eq!(w.journal().entries(), old.journal().entries());
        assert!(w.to_snapshot().journal.is_empty());
        w.journal().log().sync();
        assert_eq!(
            on_disk(&ledger),
            [entry_line(&old.journal().entries()[0]).trim_end()]
        );
        // Opening again migrates nothing twice.
        let mut again = world();
        again.apply_snapshot(crate::store::Store::load(&store).unwrap().unwrap());
        open(&mut again, &ledger).expect("reopens");
        again.journal().log().sync();
        assert_eq!(on_disk(&ledger).len(), 1);
        assert_eq!(again.journal().entries(), old.journal().entries());
    }

    /// A log left behind by a deleted world is moved aside, not loaded
    /// into the new one.
    #[test]
    fn another_worlds_log_is_moved_aside() {
        let root = crate::testkit::scratch_dir("e4-claim");
        let dir = root.as_ref().join("world.txt.ledger");
        let mut w = world();
        open(&mut w, &dir).expect("opens");
        w.journal_mut()
            .record(crate::pyutil::filetime_now(), 3, 1, HUMAN, 0, 1.0, 1.0, "");
        w.journal().log().sync();
        let mut other = world();
        let mut record = *other.world();
        record.seed ^= 1;
        other.deltas_mut().set_world(record);
        open(&mut other, &dir).expect("opens");
        assert!(other.journal().entries().is_empty());
        let birth = record.birth;
        assert!(
            root.as_ref()
                .join(format!("world.txt.ledger.stale-{birth}"))
                .is_dir()
        );
    }

    /// A line torn by a crash mid-append is skipped, and the next row
    /// appended after it still reads back.
    #[test]
    fn a_torn_last_line_is_skipped_and_the_next_row_reads_back() {
        let dir = crate::testkit::scratch_dir("e4-torn");
        let now = crate::pyutil::filetime_now();
        std::fs::write(dir.as_ref().join(file_name(now)), "journal\t1\t2").unwrap();
        let mut w = world();
        open(&mut w, dir.as_ref()).expect("opens");
        assert!(w.journal().entries().is_empty());
        let id = w.journal_mut().record(now, 3, 1, HUMAN, 0, 1.0, 1.0, "");
        w.journal().log().sync();
        let mut back = world();
        open(&mut back, dir.as_ref()).expect("reopens");
        let ids: Vec<i64> = back
            .journal()
            .entries()
            .iter()
            .map(|e| e.transaction_id)
            .collect();
        assert_eq!(ids, [id]);
    }

    /// Through the rollover's step: RAM drops a folded bot row past
    /// `BOT_LEDGER_DAYS` and keeps the human's, and so does the file.
    #[test]
    fn the_rollover_step_drops_the_same_rows_from_ram_and_disk() {
        let dir = crate::testkit::scratch_dir("e4-step");
        let mut w = world();
        open(&mut w, dir.as_ref()).expect("opens");
        let day = ft(2026, 5, 10);
        let bot = w.journal_mut().record(day, 3, 1, BOT, 0, 1.0, 1.0, "");
        let human = w.journal_mut().record(day, 3, 1, HUMAN, 0, 1.0, 1.0, "");
        w.deltas_mut().put(
            crate::econ_ledger::KIND,
            vec!["cursor".to_owned()],
            vec![human.to_string()],
        );
        let now = day + (BOT_LEDGER_DAYS + 1) * TICKS_PER_DAY;
        let d = crate::rollover::Day {
            day: 1,
            days: 1,
            now,
        };
        assert_eq!(on_rollover(&mut w, &d), 1);
        let ids: Vec<i64> = w
            .journal()
            .entries()
            .iter()
            .map(|e| e.transaction_id)
            .collect();
        assert_eq!(ids, [human]);
        w.journal().log().sync();
        let on: Vec<i64> = on_disk(dir.as_ref())
            .iter()
            .map(|l| meta(l).unwrap().id)
            .collect();
        assert_eq!(on, [human]);
        assert_ne!(bot, human);
    }
}
