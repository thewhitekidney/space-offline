//! **The ESS main bank** (lanes `ess-1`, issue #89, and `ess-2`, issue #90;
//! spec `handback/r-pve-next/report.md` §3). ESS-1 is the bank and the cut;
//! ESS-2 is the ball, the link and the theft (main bank only, owner decision
//! #96).
//!
//! The Encounter Surveillance System takes a cut of every NPC bounty paid in
//! the system it sits in and keeps it in a bank. The client reads the bank
//! through `essMgr.GetDataForClientSolarSystem` (the info panel and the
//! Agency) and is told of a change with `OnSystemESSDataChanged`.
//!
//! # What this lane does
//!
//! - **Where an ESS is.** [`has_ess`]: one per sov-null constellation, in its
//!   lowest-numbered sov-claimable system (owner decision #96, rule chosen so
//!   50 bots find it). Pure, from the universe table, so nothing is stored for
//!   the placement. Sov null only **[A]**.
//! - **The cut.** `npc::bounty::book_kill` splits a bounty with [`split`]:
//!   the pilot's part joins the pending payout as before, the bank's part is
//!   [`deposit`]ed and never reaches the pilot. A system with no ESS takes
//!   the old path unchanged.
//! - **The bank.** Two delta kinds: [`KIND`] (`system -> value last_autopay
//!   next_autopay`) and [`SHARE_KIND`] (`system character -> amount`, what
//!   each pilot has put in since the last autopayment).
//! - **The autopayment.** Every [`AUTOPAY_INTERVAL_SECS`] each contributor
//!   gets [`MAIN_BANK_PAY_RATIO`] of their share back as a ref-155 journal
//!   row; the rest stays in the bank for ESS-2's thieves. Runs from
//!   `npc::bounty::tick`, on the stored timestamps, so a late tick does what
//!   an early one would.
//!
//! # ESS-2: the ball, the link, the theft
//!
//! - **The ball.** The ESS is a *local* ball ([`ball`], id [`ess_id`]) at
//!   [`ess_position`], on grid only for a pilot near it; the **beacon**
//!   ([`beacon_ball`], id [`beacon_id`]) is a *global* ball 2 km off it, which
//!   the panel's warp button warps to (`beaconID`). The client shows the
//!   button only while the beacon is in its ballpark and the ESS is not
//!   (`warpToEssStatefulButtonController`), hence two balls. Neither is an
//!   item; both are derived from the universe table, so nothing is stored.
//! - **The link.** `essMgr.AttemptLinkToMainBank` ([`link`]) refuses with the
//!   client's own error codes ([`Refusal`]); a link lasts
//!   [`LINK_DURATION_SECS`]. [`tick`] breaks it when the linker leaves the
//!   system, changes ship or is more than [`MAX_LINK_DISTANCE`] away, and
//!   completes it: the whole bank is paid to the linker (ref 155), the shares
//!   are voided, and the theft is booked in the ledger ([`THEFT_KIND`]).
//!   Links are not persisted [A]: a restart ends them.
//! - **Pushes.** A link event (start, break, success) pushes at once and
//!   skips the [`PUSH_EVERY_SECS`] throttle.
//! - **The Agency.** [`agency_data`] and [`system_details`].
//!
//! # Marks
//!
//! `[V]` read in the client sources; `[A]` assumed with its
//! falsifier. No `[A]` value is written into a test.
//!
//! - **[V]** the data dict (keys read in `out/research/_w/src`, which is older
//!   than build 3662168; a grep of `out/pyc`, the 3662168 decompile, found the
//!   same keys present, but the lines were not re-read there): `essID`, `mainBankLink` (falsy, or `linkID`,
//!   `characterID`, `startedAt`, `completesAt`), `mainValue`, `reserveValue`,
//!   `reserveBankPulsesTotal`, `reserveBankPulsesRemaining`,
//!   `reserveBankLastPulseInitiated`, `reserveBankActiveLinks`
//!   (`dynamicResourceSvc._UpdateEssStateProvider`), and the `.get` keys
//!   `lastAutopayment`, `nextScheduledAutopayment` (`infoPanelESS`) and
//!   `beaconID` (`warpToEssStatefulButtonController`).
//! - **[A]** [`BOUNTIES_CUT`] is 25 percent (the owner's figure; the client
//!   shows whatever `GetDynamicResourceSettings` says). Falsified by a
//!   reference server's settings payload or the figure on a real
//!   `essGameplayTooltip`.
//! - **[A]** [`MAIN_BANK_PAY_RATIO`] and [`AUTOPAY_INTERVAL_SECS`]: the
//!   tooltip text says the main bank pays part of it back on an interval and
//!   keeps the rest; the figures are ours. Falsified as above.
//! - **[A]** [`PUSH_EVERY_SECS`]: a deposit marks the system and `tick` pushes
//!   at most once per system per 15 s, so a busy system does not rebuild an
//!   open panel on every kill. Falsified by a panel that looks stale.
//! - **[A]** `bufferTimeSeconds` ([`BUFFER_TIME_SECS`]) is the cooldown after
//!   a theft: the ESS takes no new link for that long. Falsified by a
//!   reference ESS that can be hacked again at once.
//! - **[A]** [`ess_id`] and [`beacon_id`] are stand-in ids, the ESS type is read
//!   from the `npc/components.tsv` row that lists `essBracket` **[V-data]**
//!   ([`ess_type`]; no row, no ball), and the beacon is a plain
//!   [`TYPE_BEACON`] ball. Falsified by a client that
//!   throws on an id, shows the warp button disabled with a beacon in the
//!   ballpark, or labels the beacon wrongly.
//! - **[A]** [`ess_position`]: 300 000 km past the star's surface, on the sun
//!   warp-in direction, so it is never inside the star nor on the hub. Falsified by a client that cannot warp there.
//! - **[A]** [`LINK_DURATION_SECS`]; the bank is paid whole and the unpaid
//!   shares are lost on a theft. Falsified by a reference payout.
//! - **[A]** a theft pays with ref 155 (the autopayment's row; the client
//!   has no theft ref). Falsified by a journal that labels it wrongly.
//! - **[V]** `dynamicresources/common/ess/const.py`: the error codes, the
//!   unlink reasons, 10 km, list 231 (`ESS_LINKABLE_TYPES_LIST`); the
//!   pushes `OnESSMainBankLinkNotification(systemID)`,
//!   `OnMainBankPlayerHackSuccess(dict)` and
//!   `OnMainBankPlayerLinkDisconnected(dict)` (`dynamicResourceSvc`).
//! - **[A]** the autopayment pays no corporation tax and credits offline
//!   pilots too. Falsified by a reference journal.

use crate::damage::BlueTime;
use crate::destiny::{Ball, FLAGS_STATIC_BALL, Mode, NO_FORMATION, Slim};
use crate::notify::{Addressed, Push};
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// Delta kind: `system -> value last_autopay next_autopay` (ISK in the main
/// bank, FILETIMEs). Claimed in `delta_kinds::KINDS`.
pub const KIND: &str = "essbank";

/// Delta kind: `system character -> amount`, the part of the bank a pilot
/// put in since the last autopayment. Claimed in `delta_kinds::KINDS`.
pub const SHARE_KIND: &str = "esscon";

/// **[A]** the share of every bounty paid in an ESS system that goes to the
/// bank (owner decision #96). Falsifier in the module header.
pub const BOUNTIES_CUT: f64 = 0.25;

/// **[A]** the part of a pilot's contribution the autopayment returns to
/// them; the rest stays in the bank. `mainBankPayRatio` in the settings.
pub const MAIN_BANK_PAY_RATIO: f64 = 0.5;

/// **[A]** seconds between autopayments: `autopaymentIntervalSeconds`.
pub const AUTOPAY_INTERVAL_SECS: i64 = 10_800;

/// **[A]** the least gap between two `OnSystemESSDataChanged` pushes about
/// one system when only deposits changed it (an autopayment pushes at once).
pub const PUSH_EVERY_SECS: i64 = 15;

/// **[A]** `bufferTimeSeconds`: the cooldown after a theft, in which the ESS
/// takes no new link.
pub const BUFFER_TIME_SECS: i64 = 600;

/// `appConst.refESSMainBankAutopayment` **[V]**.
pub const REF_ESS_AUTOPAYMENT: i64 = 155;

/// **[A]** the first stand-in ESS item id: `ESS_ID_BASE + systemID`. Player
/// items count up from `item::MIN_PLAYER_ITEM`, NPCs from 7e9, sites 8.1e9
/// and 8.7e9, sov stand-in hubs 8.9e9, abyss sites 8.95e9, escrow 9e9; this
/// sits far above all of them, just over [`BEACON_ID_BASE`]'s band (a system
/// id is below [`SYSTEM_ID_END`]). Falsifier in the module header.
pub const ESS_ID_BASE: i64 = 3_100_000_000_000;

const _: () = assert!(BEACON_ID_BASE + SYSTEM_ID_END <= ESS_ID_BASE);
const _: () = assert!(ESS_ID_BASE > crate::abyss::SITE_END);

const FILETIME_SECOND: i64 = 10_000_000;

/// The ESS item id of `system`.
pub fn ess_id(system: i64) -> i64 {
    ESS_ID_BASE + system
}

/// **The system that holds the ESS of `constellation`**: its lowest-numbered
/// sov-claimable system (`sov::claimable_system`: null security, no faction,
/// not a wormhole). `None` when the constellation has none.
///
/// The rule, per constellation: [`all_ess`] is the same in one pass, and the
/// tests hold the two together.
#[cfg(test)]
pub fn ess_system(u: &crate::celestial::Universe, constellation: i64) -> Option<i64> {
    u.systems()
        .filter(|s| s.constellation_id == constellation)
        .filter(|s| crate::sov::claimable_system(s, u.wormhole_class(s.id)))
        .map(|s| s.id)
        .min()
}

/// Whether `system` holds an ESS. Scans the universe: the world's own check
/// is [`has_ess_in`], which asks a table built once.
#[cfg(test)]
pub fn has_ess(u: &crate::celestial::Universe, system: i64) -> bool {
    u.system(system)
        .is_some_and(|s| ess_system(u, s.constellation_id) == Some(system))
}

fn cents(isk: f64) -> f64 {
    (isk * 100.0).round() / 100.0
}

/// **Split a bounty** into `(pilot, bank)`: the bank's part is `cut` of the
/// amount in whole cents, the pilot keeps the rest, so the two always add up
/// to the input. An amount that is not a positive number is the pilot's
/// untouched.
pub fn split(amount: f64, cut: f64) -> (f64, f64) {
    if !amount.is_finite() || amount <= 0.0 {
        return (amount, 0.0);
    }
    let total = (amount * 100.0).round();
    let bank = (total * cut.clamp(0.0, 1.0)).round();
    ((total - bank) / 100.0, bank / 100.0)
}

/// One bank row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bank {
    pub value: f64,
    pub last_autopay: BlueTime,
    pub next_autopay: BlueTime,
}

impl Bank {
    fn from_row(v: &[String]) -> Option<Bank> {
        let [value, last, next] = v else { return None };
        let value: f64 = value.parse().ok()?;
        value.is_finite().then_some(())?;
        Some(Bank {
            value: value.max(0.0),
            last_autopay: last.parse().ok()?,
            next_autopay: next.parse().ok()?,
        })
    }

    fn to_row(self) -> Vec<String> {
        vec![
            self.value.to_string(),
            self.last_autopay.to_string(),
            self.next_autopay.to_string(),
        ]
    }

    /// A bank that has never been touched, at `now`.
    fn fresh(now: BlueTime) -> Bank {
        Bank {
            value: 0.0,
            last_autopay: now,
            next_autopay: now + AUTOPAY_INTERVAL_SECS * FILETIME_SECOND,
        }
    }
}

fn bank_key(system: i64) -> Vec<String> {
    vec![system.to_string()]
}

fn share_key(system: i64, character: i64) -> Vec<String> {
    vec![system.to_string(), character.to_string()]
}

/// The bank of `system`, or a fresh empty one when nothing was ever put in.
pub fn bank(reg: &Registry, system: i64, now: BlueTime) -> Bank {
    reg.deltas()
        .of(KIND)
        .get(&bank_key(system))
        .and_then(|v| Bank::from_row(v))
        .unwrap_or_else(|| Bank::fresh(now))
}

/// What `character` has put into `system`'s bank since the last autopayment.
pub fn share(reg: &Registry, system: i64, character: i64) -> f64 {
    reg.deltas()
        .of(SHARE_KIND)
        .get(&share_key(system, character))
        .and_then(|v| v.first()?.parse().ok())
        .unwrap_or(0.0)
}

/// **Put `amount` into `system`'s bank on `character`'s account**, and mark
/// the system for a push (`tick` sends it, throttled). Not for a system
/// without an ESS: the caller checks.
pub fn deposit(reg: &mut Registry, system: i64, character: i64, amount: f64, now: BlueTime) {
    if !amount.is_finite() || amount <= 0.0 {
        return;
    }
    let mut b = bank(reg, system, now);
    b.value = cents(b.value + amount);
    let mine = cents(share(reg, system, character) + amount);
    reg.deltas_mut().put(KIND, bank_key(system), b.to_row());
    reg.deltas_mut().put(
        SHARE_KIND,
        share_key(system, character),
        vec![mine.to_string()],
    );
    reg.ess_told_mut(system).1 = true;
}

/// `/essdeposit <isk>`: put ISK into the ESS bank of the caller's system, on
/// the caller's account. A GM way to fund the bank, which otherwise fills only
/// from bounty kills.
pub(crate) fn gm_deposit_text(reg: &mut Registry, actor: Option<i64>, args: &[&str]) -> String {
    let system = actor.and_then(|who| reg.session_system(who));
    gm_deposit_in(reg, system, actor, args, crate::pyutil::filetime_now())
}

/// [`gm_deposit_text`] with the caller's system and the clock given.
fn gm_deposit_in(
    reg: &mut Registry,
    system: Option<i64>,
    actor: Option<i64>,
    args: &[&str],
    now: BlueTime,
) -> String {
    let (Some(system), Some(who)) = (system, actor) else {
        return "no ESS: you are not in a system".to_owned();
    };
    let amount = match args {
        [one] => one
            .parse::<f64>()
            .ok()
            .filter(|a| a.is_finite() && *a > 0.0),
        _ => None,
    };
    let Some(amount) = amount else {
        return "usage: /essdeposit <isk> (a number above 0)".to_owned();
    };
    if !has_ess_in(reg, system) {
        return format!("no ESS in system {system}");
    }
    deposit(reg, system, who, amount, now);
    format!("deposited {amount} ISK into the ESS bank of {system}")
}

/// **The data dict** the client reads for `system` **[V]** (module header).
/// `mainBankLink` is an empty dict, which is falsy and which the info
/// panel's `.get('mainBankLink', {}).get(...)` calls survive; ESS-2 fills it
/// while a pilot is linked.
pub fn data(reg: &Registry, system: i64, now: BlueTime) -> PyValue {
    let b = bank(reg, system, now);
    let link = match reg.ext::<State>().links.get(&system) {
        Some(l) => crate::pyutil::pydict(vec![
            ("linkID", PyValue::Int(l.link_id)),
            ("characterID", PyValue::Int(l.character)),
            ("startedAt", PyValue::Int(l.started)),
            ("completesAt", PyValue::Int(l.completes)),
        ]),
        None => PyValue::Dict(vec![]),
    };
    crate::pyutil::pydict(vec![
        ("essID", PyValue::Int(ess_id(system))),
        ("beaconID", PyValue::Int(beacon_id(system))),
        ("mainBankLink", link),
        ("mainValue", PyValue::Real(b.value)),
        ("reserveValue", PyValue::Real(0.0)),
        ("reserveBankPulsesTotal", PyValue::Int(0)),
        ("reserveBankPulsesRemaining", PyValue::Int(0)),
        ("reserveBankLastPulseInitiated", PyValue::None),
        ("reserveBankActiveLinks", PyValue::Int(0)),
        ("lastAutopayment", PyValue::Int(b.last_autopay)),
        ("nextScheduledAutopayment", PyValue::Int(b.next_autopay)),
    ])
}

/// Push the new data to the pilots in `system`, on the clock, and note it as
/// told.
fn tell(reg: &mut Registry, system: i64, now: BlueTime) {
    *reg.ess_told_mut(system) = (now, false);
    let to = crate::dungeon::audience(reg, system);
    if to.is_empty() {
        return;
    }
    let data = data(reg, system, now);
    reg.schedule(
        Duration::ZERO,
        Addressed {
            to: Some(to),
            inner: Box::new(SystemDataChanged { system, data }),
        },
    );
}

/// `OnSystemESSDataChanged(systemID, data)` **[V]**
/// (`dynamicResourceSvc.py:160-165`): primes the data cache when the system
/// is the pilot's current one.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemDataChanged {
    pub system: i64,
    pub data: PyValue,
}

impl Push for SystemDataChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnSystemESSDataChanged",
            vec![PyValue::Int(self.system), self.data.clone()],
        )
    }
}

/// **The world-clock step**: push the banks that changed (at most one push
/// per system per [`PUSH_EVERY_SECS`]), then pay every autopayment that has
/// come due.
/// Called from `npc::bounty::tick`.
pub fn tick(reg: &mut Registry, now: BlueTime) {
    tick_links(reg, now);
    for (system, last) in reg.ess_dirty() {
        if now - last >= PUSH_EVERY_SECS * FILETIME_SECOND {
            tell(reg, system, now);
        }
    }
    let due: Vec<i64> = reg
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            let b = Bank::from_row(v)?;
            if b.next_autopay > now {
                return None;
            }
            k.first()?.parse().ok()
        })
        .collect();
    for system in due {
        autopay(reg, system, now);
    }
}

/// ISK in all main banks: the cut that is held, not yet paid back. Nothing
/// else shows it until ESS-2 (the unpaid half of each contribution stays here).
pub fn held(reg: &Registry) -> f64 {
    reg.deltas()
        .of(KIND)
        .values()
        .filter_map(|v| Bank::from_row(v))
        .map(|b| b.value)
        .sum()
}

/// Pay each contributor of `system` [`MAIN_BANK_PAY_RATIO`] of their share
/// and take it out of the bank. A contributor who is gone is dropped unpaid
/// and their share stays in the bank.
fn autopay(reg: &mut Registry, system: i64, now: BlueTime) {
    let key = system.to_string();
    let shares: Vec<(Vec<String>, i64, f64)> = reg
        .deltas()
        .of(SHARE_KIND)
        .iter()
        .filter(|(k, _)| k.first() == Some(&key))
        .filter_map(|(k, v)| Some((k.clone(), k.get(1)?.parse().ok()?, v.first()?.parse().ok()?)))
        .collect();
    let mut b = bank(reg, system, now);
    let mut paid = 0.0;
    for (row, character, amount) in shares {
        reg.deltas_mut().del(SHARE_KIND, &row);
        let pay = cents(amount * MAIN_BANK_PAY_RATIO).min(b.value - paid);
        if pay > 0.0 && credit(reg, character, system, pay, now, "") {
            paid += pay;
        }
    }
    b.value = cents((b.value - paid).max(0.0));
    b.last_autopay = now;
    b.next_autopay = now + AUTOPAY_INTERVAL_SECS * FILETIME_SECOND;
    reg.deltas_mut().put(KIND, bank_key(system), b.to_row());
    tell(reg, system, now);
}

/// Credit `amount` to `character`: the balance, one ref-155 row (reference
/// the system) and `OnAccountChange`. `false` when the character is gone.
fn credit(
    reg: &mut Registry,
    character: i64,
    system: i64,
    amount: f64,
    now: BlueTime,
    why: &str,
) -> bool {
    let Some(c) = reg.character_mut(character) else {
        return false;
    };
    c.balance += amount;
    let balance = c.balance;
    reg.journal_mut().record(
        now,
        REF_ESS_AUTOPAYMENT,
        crate::npc::bounty::BOUNTY_PAYER,
        character,
        system,
        amount,
        balance,
        why,
    );
    reg.mark_dirty();
    reg.schedule(
        Duration::ZERO,
        Addressed {
            to: Some(vec![character]),
            inner: Box::new(crate::notify::AccountChange {
                owner_id: character,
                balance,
            }),
        },
    );
    true
}

// ---------------------------------------------------------------------------
// ESS-2: the ball, the beacon, the link, the theft
// ---------------------------------------------------------------------------

/// Delta kind: `system thief -> amount`, what a pilot has taken from the
/// system's main bank in all (the theft ledger the history window reads).
/// Claimed in `delta_kinds::KINDS`.
pub const THEFT_KIND: &str = "essthf";

/// The component `npc/components.tsv` lists on the ESS type **[V-data]**
/// (with `essLink` and `essRegistration`). The type id itself is bundle
/// data and is read at run time ([`ess_type`]); no row, no ball.
pub const COMPONENT_ESS: &str = "essBracket";

/// **[A]** the beacon's type: type 15, which the client names "Asteroid
/// Belt" and gives belt audio. Chosen because it is a plain model-less
/// warpable; no better one is known (the only "beacon" types in the data
/// are cynosural, with effects, or blueprints). Falsified by the overview
/// or the sound giving it away: the human check on the first ESS visit.
pub const TYPE_BEACON: i64 = 15;

/// **[A]** the first beacon id: `BEACON_ID_BASE + systemID`, clear of every
/// other band (the trade tickets sit at 2e12, mail lists and market escrow
/// at 1e12).
pub const BEACON_ID_BASE: i64 = 3_000_000_000_000;

/// The system ids a stand-in id can name: known space and a little more.
const SYSTEM_ID_END: i64 = 40_000_000;

/// `ESS_MAX_LINK_DISTANCE_METERS` **[V]** (`dynamicresources/common/ess/const.py`).
pub const MAX_LINK_DISTANCE: f64 = 10_000.0;

/// `ESS_LINKABLE_TYPES_LIST` **[V]**: the typelist of hulls that may link.
pub const LINKABLE_LIST: i64 = 231;

/// **[A]** the least gap between two warnings to one system's contributors:
/// 30 s, as the review says the client's notification adapter throttles it.
/// Falsified by the adapter's constant, if it differs.
pub const WARN_EVERY_SECS: i64 = 30;

/// **[A]** how long a link takes to break into the bank. Falsified by a
/// reference ESS or the owner's feel for it.
pub const LINK_DURATION_SECS: i64 = 300;

/// **[A]** the ESS ball's radius when the type table has none for it.
const ESS_RADIUS: f64 = 500.0;
/// **[A]** the beacon ball's radius.
const BEACON_RADIUS: f64 = 100.0;
/// **[A]** how far outside the star's surface the ESS sits, along the sun
/// warp-in direction (the hub sits 100 km out on the same line): 300 000 km,
/// so it is never inside the star, whatever the star's radius, and never
/// near the hub.
const ESS_CLEARANCE: f64 = 300_000_000.0;
/// **[A]** where the beacon is, from the ESS: close enough that a pilot who
/// warps to it lands on the ESS's grid and inside link range.
const BEACON_OFFSET: [f64; 3] = [2_000.0, 0.0, 0.0];

/// `UNLINKED_*` **[V]**: why a link ended.
pub const UNLINKED_MANUAL: i64 = 0;
pub const UNLINKED_PROXIMITY: i64 = 1;
pub const UNLINKED_SHIP_CHANGE: i64 = 2;

/// `notificationTypeEssMainBankLink` **[V]**.
pub const NOTIFICATION_LINK: i64 = 1015;

/// `SAFE_LOGOFF_FAILED_LINKED_TO_ESS` **[V]**: the label the safe-logoff
/// dialog shows while a pilot is linked.
pub const SAFE_LOGOFF_LINKED: &str = "UI/Inflight/SafeLogoff/ConditionLinkedToESS";

/// One link in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link {
    pub link_id: i64,
    pub character: i64,
    pub ship: i64,
    pub started: BlueTime,
    pub completes: BlueTime,
}

/// The world slot's state: the links (not persisted **[A]**), the cooldowns,
/// and the table of ESS systems, built on first use.
#[derive(Debug, Default)]
pub struct State {
    links: BTreeMap<i64, Link>,
    next_link: i64,
    cooled: BTreeMap<i64, BlueTime>,
    /// When the contributors of a system may next be warned (type 1015).
    warned: BTreeMap<i64, BlueTime>,
    systems: std::sync::OnceLock<BTreeSet<i64>>,
    /// The ESS type, once looked up (`None` inside: the bundle has none).
    ess_type: std::sync::OnceLock<Option<i64>>,
}

/// The slot (coordinator: `world_ext::SLOTS`).
pub const SLOT: SlotSpec = SlotSpec {
    name: "ess",
    make: |_| Box::new(State::default()) as Boxed,
};

/// Every system that holds an ESS: the lowest sov-claimable one of each
/// constellation. The same rule as [`ess_system`], in one pass.
pub fn all_ess(u: &crate::celestial::Universe) -> BTreeSet<i64> {
    let mut lowest: BTreeMap<i64, i64> = BTreeMap::new();
    for s in u.systems() {
        if crate::sov::claimable_system(s, u.wormhole_class(s.id)) {
            let e = lowest.entry(s.constellation_id).or_insert(s.id);
            *e = (*e).min(s.id);
        }
    }
    lowest.into_values().collect()
}

/// [`has_ess`] for a world: the table is built once, so the scene and the
/// grid can ask on every call.
pub fn has_ess_in(reg: &Registry, system: i64) -> bool {
    ess_systems(reg).contains(&system)
}

/// Every ESS system of the world's universe (empty without one).
pub fn ess_systems(reg: &Registry) -> &BTreeSet<i64> {
    reg.ext::<State>().systems.get_or_init(|| {
        reg.content()
            .try_get::<crate::celestial::Universe>()
            .map(all_ess)
            .unwrap_or_default()
    })
}

/// The beacon id of `system`.
pub fn beacon_id(system: i64) -> i64 {
    BEACON_ID_BASE + system
}

/// The system an ESS ball id names.
fn system_of_ess(id: i64) -> Option<i64> {
    let s = id.checked_sub(ESS_ID_BASE)?;
    (1..SYSTEM_ID_END).contains(&s).then_some(s)
}

/// The system a beacon id names.
fn system_of_beacon(id: i64) -> Option<i64> {
    let s = id.checked_sub(BEACON_ID_BASE)?;
    (1..SYSTEM_ID_END).contains(&s).then_some(s)
}

fn offset(p: [f64; 3], by: [f64; 3]) -> [f64; 3] {
    [p[0] + by[0], p[1] + by[1], p[2] + by[2]]
}

/// Where the ESS of `system` is **[A]**: [`ESS_CLEARANCE`] past the star's
/// surface, in the direction of the sun warp-in point (the client's own
/// `cos`/`sin` of the radius, `warp_in::sun_warp_in`), so it is outside the
/// star for any radius.
pub fn ess_position(reg: &Registry, system: i64) -> [f64; 3] {
    let (star, radius) = crate::sov_hub::star_in(reg, system);
    let d = radius + ESS_CLEARANCE;
    [
        star[0] + d * radius.cos(),
        star[1] + radius / 5.0,
        star[2] - d * radius.sin(),
    ]
}

/// The type of the ESS ball: the lowest type id whose `npc/components.tsv`
/// rows list [`COMPONENT_ESS`] **[V-data]**. `None` without the table or
/// the row, and then the system has no ball.
pub fn ess_type(reg: &Registry) -> Option<i64> {
    *reg.ext::<State>().ess_type.get_or_init(|| {
        let t = reg.content().try_get::<crate::npc_data::Table>()?;
        t.components
            .iter()
            .find(|(_, c)| c == COMPONENT_ESS)
            .map(|(ty, _)| *ty)
    })
}

/// Whether `system` has an ESS **with a ball to show** ([`ess_type`]).
fn built(reg: &Registry, system: i64) -> bool {
    has_ess_in(reg, system) && ess_type(reg).is_some()
}

/// Where the warp beacon of `system` is **[A]**.
pub fn beacon_position(reg: &Registry, system: i64) -> [f64; 3] {
    offset(ess_position(reg, system), BEACON_OFFSET)
}

fn ess_radius(reg: &Registry) -> f64 {
    crate::typedata::installed_in(reg.content())
        .table()
        .get(ess_type(reg).unwrap_or(0))
        .map(|r| r.radius)
        .filter(|r| *r > 0.0)
        .unwrap_or(ESS_RADIUS)
}

fn static_ball(id: i64, radius: f64, position: [f64; 3]) -> Ball {
    Ball {
        ball_id: id,
        mode: Mode::Stop,
        radius: radius as f32,
        position,
        flags: FLAGS_STATIC_BALL,
        mass: 0.0,
        cloak: 0,
        harmonic: -1,
        corporation_id: i32::try_from(crate::sov::NEUTRAL_HUB_OWNER).unwrap_or(-1),
        alliance_id: -1,
        physics: None,
        formation_id: NO_FORMATION,
    }
}

fn static_slim(id: i64, type_id: i64) -> Slim {
    Slim {
        item_id: id,
        type_id,
        owner_id: crate::sov::NEUTRAL_HUB_OWNER,
        char_id: None,
        corp_id: Some(crate::sov::NEUTRAL_HUB_OWNER),
        response_thresholds: None,
    }
}

/// The ESS ball of `system`: a body that never moves.
pub fn ball(reg: &Registry, system: i64) -> Ball {
    static_ball(ess_id(system), ess_radius(reg), ess_position(reg, system))
}

/// The ESS ball and slim of `system`, when it has one: a **local** ball
/// (`grid::local_balls`), so a pilot is sent it only on its grid.
pub fn balls_in(w: &Registry, system: i64) -> Vec<(Ball, Slim)> {
    let Some(ty) = ess_type(w).filter(|_| has_ess_in(w, system)) else {
        return Vec::new();
    };
    vec![(ball(w, system), static_slim(ess_id(system), ty))]
}

/// The warp beacon of `system`, when it has an ESS: a **global** ball, in
/// every pilot's ballpark, so the panel can warp to it.
pub fn beacon_balls_in(w: &Registry, system: i64) -> Vec<(Ball, Slim)> {
    if !built(w, system) {
        return Vec::new();
    }
    let id = beacon_id(system);
    vec![(
        static_ball(id, BEACON_RADIUS, beacon_position(w, system)),
        static_slim(id, TYPE_BEACON),
    )]
}

/// The beacon's display name **[A]**: `<system> - ESS Beacon`. The client
/// splits a location name on " - " when it labels a warp target; an unnamed
/// slim hints `None` into `cfg.evelocations` and the warp raises. Falsified by
/// a warp to the beacon that still raises, or a bracket that reads oddly.
pub fn beacon_name(w: &Registry, system: i64) -> String {
    let sys = w
        .content()
        .try_get::<crate::celestial::Universe>()
        .and_then(|u| u.system(system))
        .map(|s| s.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| system.to_string());
    format!("{sys} - ESS Beacon")
}

/// The `slim_extra::Source` row (`slim_extra::SOURCES`): a beacon's slim
/// carries its `name`.
pub fn slim_extras(w: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    ids.iter()
        .filter_map(|id| {
            let system = system_of_beacon(*id).filter(|s| built(w, *s))?;
            Some((
                *id,
                vec![("name", PyValue::Unicode(beacon_name(w, system)))],
            ))
        })
        .collect()
}

/// Where the ESS or its beacon `ball_id` is, for `Registry::ball_target`:
/// approach, orbit, warp to. **[A]** no warp-in point (neither is a body of
/// 90 km).
pub fn target(w: &Registry, ball_id: i64) -> Option<crate::model::Target> {
    let (system, radius, position) = if let Some(s) = system_of_ess(ball_id) {
        (s, ess_radius(w), ess_position(w, s))
    } else {
        let s = system_of_beacon(ball_id)?;
        (s, BEACON_RADIUS, beacon_position(w, s))
    };
    built(w, system).then_some(crate::model::Target {
        position,
        radius,
        warp_in: None,
    })
}

/// Why `link` said no: the client's own `LINK_ERROR_*` cases **[V]** (offline,
/// not on grid, out of range, invalid ship, occupied, already linked).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not in space.
    NotInSpace,
    /// The system has no ESS.
    NoEss,
    /// Someone else is linked.
    Occupied,
    /// The pilot is already linked.
    Already,
    /// The hull is not in list 231.
    BadShip,
    /// The ESS is not on the pilot's grid.
    NotOnGrid,
    /// On grid, but over [`MAX_LINK_DISTANCE`] away.
    OutOfRange,
    /// A theft happened less than [`BUFFER_TIME_SECS`] ago.
    Cooling,
}

impl Refusal {
    /// The words of the dialog the refusal raises.
    pub fn text(self) -> &'static str {
        match self {
            Refusal::NotInSpace => "You must be in space to link to an ESS.",
            Refusal::NoEss => "There is no ESS in this system.",
            Refusal::Occupied => "Another pilot is already linked to this ESS.",
            Refusal::Already => "You are already linked to this ESS.",
            Refusal::BadShip => "This ship cannot link to an ESS.",
            Refusal::NotOnGrid => "The ESS is not on your grid.",
            Refusal::OutOfRange => "You are too far from the ESS to link to it.",
            Refusal::Cooling => "The ESS was just emptied and cannot be linked yet.",
        }
    }
}

/// Whether a hull of this type, group and category may link, by list
/// [`LINKABLE_LIST`]. No list, nothing to check against: any hull.
pub fn ship_may_link(list: Option<&crate::dungeon_data::TypeList>, t: i64, g: i64, c: i64) -> bool {
    list.is_none_or(|l| l.contains(t, g, c))
}

fn hull_may_link(reg: &Registry, ship: i64, who: i64) -> bool {
    let list = reg
        .content()
        .try_get::<crate::dungeon_data::Table>()
        .and_then(|t| t.typelists.get(&LINKABLE_LIST));
    let ty = crate::jump_drive::ship_type_of(reg, ship, who);
    let (group, cat) = crate::typedata::installed_in(reg.content())
        .table()
        .get(ty)
        .map_or((0, 0), |r| (r.group_id, r.category_id));
    ship_may_link(list, ty, group, cat)
}

/// Surface-to-surface distance from the ship of `who` (type of `ship`) at
/// `at` to the ESS of `system`: the client's `GetSurfaceDist` takes both
/// radii off the centre distance.
fn range_to_ess(reg: &Registry, system: i64, at: [f64; 3], ship: i64, who: i64) -> f64 {
    let ty = crate::jump_drive::ship_type_of(reg, ship, who);
    let ship_radius = crate::typedata::installed_in(reg.content())
        .table()
        .get(ty)
        .map_or(0.0, |r| r.radius);
    (crate::space_state::distance(at, ess_position(reg, system)) - ess_radius(reg) - ship_radius)
        .max(0.0)
}

/// Where `who`'s ship is now.
fn position_of(reg: &mut Registry, who: i64) -> Option<[f64; 3]> {
    reg.advance_character(who);
    reg.roster().get(who).map(|p| p.motion.position)
}

/// Who is linked to `system`'s ESS, if anyone.
pub fn link_of(reg: &Registry, system: i64) -> Option<Link> {
    reg.ext::<State>().links.get(&system).copied()
}

/// Whether `who` is linked to an ESS: safe logoff refuses while they are.
pub fn linked(reg: &Registry, who: i64) -> bool {
    reg.ext::<State>()
        .links
        .values()
        .any(|l| l.character == who)
}

/// **`essMgr.AttemptLinkToMainBank`**: link `who`'s ship to the ESS of the
/// system it is in, in the client's own order of checks
/// (`dynamicResourceSvc.CheckLinkEligibility`): occupied, hull, grid, range.
/// Answers the link id; the pushes go out at once.
pub fn link(reg: &mut Registry, who: i64, now: BlueTime) -> Result<i64, Refusal> {
    let Some(p) = reg.roster().get(who).copied() else {
        return Err(Refusal::NotInSpace);
    };
    let system = p.solar_system_id;
    if !has_ess_in(reg, system) {
        return Err(Refusal::NoEss);
    }
    if let Some(l) = link_of(reg, system) {
        return Err(if l.character == who {
            Refusal::Already
        } else {
            Refusal::Occupied
        });
    }
    if linked(reg, who) {
        return Err(Refusal::Already);
    }
    if reg
        .ext::<State>()
        .cooled
        .get(&system)
        .is_some_and(|until| now < *until)
    {
        return Err(Refusal::Cooling);
    }
    if !hull_may_link(reg, p.ship_id, who) {
        return Err(Refusal::BadShip);
    }
    let at = position_of(reg, who).unwrap_or(p.motion.position);
    if !crate::grid::on_grid(at, ess_position(reg, system), false) {
        return Err(Refusal::NotOnGrid);
    }
    if range_to_ess(reg, system, at, p.ship_id, who) > MAX_LINK_DISTANCE {
        return Err(Refusal::OutOfRange);
    }
    let st = reg.ext_mut::<State>();
    st.next_link += 1;
    let l = Link {
        link_id: st.next_link,
        character: who,
        ship: p.ship_id,
        started: now,
        completes: now + LINK_DURATION_SECS * FILETIME_SECOND,
    };
    st.links.insert(system, l);
    to_system(reg, system, &[], LinkStarted { system });
    warn_contributors(reg, system, who, now);
    tell(reg, system, now);
    Ok(l.link_id)
}

/// **`essMgr.RequestMainBankUnlink`**: end `who`'s own link. `false` when
/// they had none.
pub fn unlink(reg: &mut Registry, who: i64, now: BlueTime) -> bool {
    let Some(system) = reg
        .ext::<State>()
        .links
        .iter()
        .find(|(_, l)| l.character == who)
        .map(|(s, _)| *s)
    else {
        return false;
    };
    end_link(reg, system, UNLINKED_MANUAL, now);
    true
}

/// **The pilot's last session closed** (`session_hooks::ON_LOGOFF`): their
/// link ends (as a manual unlink), so an offline pilot cannot finish a
/// theft. A hook and not an online check on the tick, because a bot has no
/// session to be online with.
pub fn on_logoff(reg: &mut Registry, who: i64) {
    unlink(reg, who, crate::pyutil::filetime_now());
}

/// `OnESSMainBankLinkNotification(systemID)`: the clients drop their cached
/// data and fetch it again, and show the "ESS is being hacked" notification.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkStarted {
    pub system: i64,
}

impl Push for LinkStarted {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnESSMainBankLinkNotification",
            vec![PyValue::Int(self.system)],
        )
    }
}

/// `OnMainBankPlayerHackSuccess(hack_data)`.
#[derive(Debug, Clone, PartialEq)]
pub struct HackSuccess {
    pub system: i64,
    pub link_id: i64,
    pub character: i64,
}

impl Push for HackSuccess {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnMainBankPlayerHackSuccess",
            vec![crate::pyutil::pydict(vec![
                ("solarSystemID", PyValue::Int(self.system)),
                ("linkID", PyValue::Int(self.link_id)),
                ("characterID", PyValue::Int(self.character)),
            ])],
        )
    }
}

/// `OnMainBankPlayerLinkDisconnected(data)`.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkDisconnected {
    pub system: i64,
    pub link_id: i64,
    pub character: i64,
    pub reason: i64,
}

impl Push for LinkDisconnected {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnMainBankPlayerLinkDisconnected",
            vec![crate::pyutil::pydict(vec![
                ("solarSystemID", PyValue::Int(self.system)),
                ("linkID", PyValue::Int(self.link_id)),
                ("characterID", PyValue::Int(self.character)),
                ("reason", PyValue::Int(self.reason)),
            ])],
        )
    }
}

/// Push `inner` to the pilots in `system` and to `also` (the linker may have
/// left the system, and their window still has to close).
fn to_system(reg: &mut Registry, system: i64, also: &[i64], inner: impl Push + 'static) {
    let mut to = crate::dungeon::audience(reg, system);
    for c in also {
        if !to.contains(c) && reg.character(*c).is_some() {
            to.push(*c);
        }
    }
    if to.is_empty() {
        return;
    }
    reg.schedule(
        Duration::ZERO,
        Addressed {
            to: Some(to),
            inner: Box::new(inner),
        },
    );
}

/// Tell each pilot with a share in the bank who is not in the system that
/// it is being hacked (type 1015); the ones in the system get the
/// notification from their client, on `OnESSMainBankLinkNotification`.
///
/// At most once per [`WARN_EVERY_SECS`] per system, as the client's own
/// adapter throttles it: a link and unlink loop must not flood a pilot's
/// notification list.
fn warn_contributors(reg: &mut Registry, system: i64, thief: i64, now: BlueTime) {
    let st = reg.ext_mut::<State>();
    if st.warned.get(&system).is_some_and(|until| now < *until) {
        return;
    }
    st.warned
        .insert(system, now + WARN_EVERY_SECS * FILETIME_SECOND);
    let key = system.to_string();
    let owners: Vec<i64> = reg
        .deltas()
        .of(SHARE_KIND)
        .iter()
        .filter(|(k, _)| k.first() == Some(&key))
        .filter_map(|(k, _)| k.get(1)?.parse().ok())
        .filter(|c| *c != thief)
        .collect();
    for c in owners {
        let here = reg
            .roster()
            .get(c)
            .is_some_and(|p| p.solar_system_id == system);
        if !here {
            crate::notifications::notify(
                reg,
                c,
                NOTIFICATION_LINK,
                crate::notifications::data([(
                    "solarSystemID",
                    crate::notifications::Value::Int(system),
                )]),
            );
        }
    }
}

/// Break the link on `system` for `reason`, and say so.
fn end_link(reg: &mut Registry, system: i64, reason: i64, now: BlueTime) {
    let Some(l) = reg.ext_mut::<State>().links.remove(&system) else {
        return;
    };
    to_system(
        reg,
        system,
        &[l.character],
        LinkDisconnected {
            system,
            link_id: l.link_id,
            character: l.character,
            reason,
        },
    );
    tell(reg, system, now);
}

/// **The link's end in success**: the whole bank to the linker, once. The
/// shares are void (their autopayment never comes), the theft is booked,
/// and the ESS cools for [`BUFFER_TIME_SECS`].
fn steal(reg: &mut Registry, system: i64, l: Link, now: BlueTime) {
    reg.ext_mut::<State>().links.remove(&system);
    let mut b = bank(reg, system, now);
    let loot = cents(b.value);
    b.value = 0.0;
    reg.deltas_mut().put(KIND, bank_key(system), b.to_row());
    let key = system.to_string();
    let shares: Vec<Vec<String>> = reg
        .deltas()
        .of(SHARE_KIND)
        .keys()
        .filter(|k| k.first() == Some(&key))
        .cloned()
        .collect();
    for row in shares {
        reg.deltas_mut().del(SHARE_KIND, &row);
    }
    if loot > 0.0 && credit(reg, l.character, system, loot, now, "ESS main bank theft") {
        let row = vec![system.to_string(), l.character.to_string()];
        let total = cents(stolen(reg, system, l.character) + loot);
        reg.deltas_mut()
            .put(THEFT_KIND, row, vec![total.to_string()]);
    }
    reg.ext_mut::<State>()
        .cooled
        .insert(system, now + BUFFER_TIME_SECS * FILETIME_SECOND);
    to_system(
        reg,
        system,
        &[l.character],
        HackSuccess {
            system,
            link_id: l.link_id,
            character: l.character,
        },
    );
    tell(reg, system, now);
}

/// What `character` has taken from `system`'s main bank, in all.
pub fn stolen(reg: &Registry, system: i64, character: i64) -> f64 {
    reg.deltas()
        .of(THEFT_KIND)
        .get(&share_key(system, character))
        .and_then(|v| v.first()?.parse().ok())
        .unwrap_or(0.0)
}

/// **`essMgr.GetMainBankTheftsForClientSolarSystem`**: the ledger of
/// `system`, the biggest thief first, as the history window reads it
/// (`characterID`, `amount`).
pub fn thefts(reg: &Registry, system: i64) -> PyValue {
    let key = system.to_string();
    let mut rows: Vec<(i64, f64)> = reg
        .deltas()
        .of(THEFT_KIND)
        .iter()
        .filter(|(k, _)| k.first() == Some(&key))
        .filter_map(|(k, v)| Some((k.get(1)?.parse().ok()?, v.first()?.parse().ok()?)))
        .collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    PyValue::List(
        rows.into_iter()
            .map(|(c, amount)| {
                crate::pyutil::pydict(vec![
                    ("characterID", PyValue::Int(c)),
                    ("amount", PyValue::Real(amount)),
                ])
            })
            .collect(),
    )
}

/// The links' step: break the ones whose pilot left the system, changed
/// ship or drifted out of range; complete the ones whose time is up.
fn tick_links(reg: &mut Registry, now: BlueTime) {
    let links: Vec<(i64, Link)> = reg
        .ext::<State>()
        .links
        .iter()
        .map(|(s, l)| (*s, *l))
        .collect();
    for (system, l) in links {
        let here = reg
            .roster()
            .get(l.character)
            .copied()
            .filter(|p| p.solar_system_id == system);
        let Some(p) = here else {
            end_link(reg, system, UNLINKED_PROXIMITY, now);
            continue;
        };
        if p.ship_id != l.ship {
            end_link(reg, system, UNLINKED_SHIP_CHANGE, now);
            continue;
        }
        let at = position_of(reg, l.character).unwrap_or(p.motion.position);
        if range_to_ess(reg, system, at, l.ship, l.character) > MAX_LINK_DISTANCE {
            end_link(reg, system, UNLINKED_PROXIMITY, now);
            continue;
        }
        if now >= l.completes {
            steal(reg, system, l, now);
        }
    }
}

/// `dynamicResourceCacheMgr.GetESSAgencyData`: `{systemID: {...}}` for every
/// ESS, the keys `contentProviderESS` reads. `currentOutput` is the
/// bounty multiplier, 1.0 **[A]** (no dynamic bounty system), and the
/// reserve bank is not built (owner decision #96).
pub fn agency_data(reg: &Registry, now: BlueTime) -> PyValue {
    PyValue::Dict(
        ess_systems(reg)
            .iter()
            .map(|s| {
                (
                    PyValue::Int(*s),
                    crate::pyutil::pydict(vec![
                        ("currentOutput", PyValue::Real(1.0)),
                        ("mainValue", PyValue::Real(bank(reg, *s, now).value)),
                        ("reserveValue", PyValue::Real(0.0)),
                        ("vaultOpen", PyValue::Bool(false)),
                    ]),
                )
            })
            .collect(),
    )
}

/// The client's `ESS_AUTOPAY_STATE_*` **[V]**: the Agency gauge. **[A]**
/// the thirds: more than two thirds of the interval left is long, more than
/// one third medium.
pub fn autopay_state(b: &Bank, now: BlueTime) -> i64 {
    let left = (b.next_autopay - now).max(0) as f64;
    let whole = (AUTOPAY_INTERVAL_SECS * FILETIME_SECOND) as f64;
    if left > whole * 2.0 / 3.0 {
        0
    } else if left > whole / 3.0 {
        1
    } else {
        2
    }
}

/// `dynamicResourceCacheMgr.GetESSSystemDetails(systemID)`: the five keys
/// `essSystemInfoContainer` reads. The reserve bank is not built, so its
/// three are the inactive values.
pub fn system_details(reg: &Registry, system: i64, now: BlueTime) -> PyValue {
    let b = bank(reg, system, now);
    crate::pyutil::pydict(vec![
        (
            "autopaymentWindowState",
            PyValue::Int(autopay_state(&b, now)),
        ),
        (
            "activeLinks",
            PyValue::Int(i64::from(link_of(reg, system).is_some())),
        ),
        ("totalPulses", PyValue::Int(0)),
        ("reserveTimeWindowState", PyValue::Int(0)),
        ("reserveTimeChosen", PyValue::Int(0)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::celestial::Universe;

    const T0: BlueTime = 130_000_000_000_000_000;
    const PA: i64 = 5_001;
    const PB: i64 = 5_002;

    /// The fixture's constellation 29_000_002 has null systems 39_000_003
    /// and 39_000_004 (zero security), a low-sec one and a wormhole.
    const ESS: i64 = 39_000_003;
    const OTHER_NULL: i64 = 39_000_004;
    const LOW: i64 = 39_000_005;
    const HIGH: i64 = 39_000_001;

    fn reg_fresh() -> Registry {
        reg()
    }

    /// An invented ESS type, listed with `essBracket` in the test's table.
    const ESS_TYPE: i64 = 990_914;

    fn content_with_ess_type() -> crate::content::Content {
        let mut content = crate::content::Content::fixture();
        let mut npc = crate::npc_data::Table::fixture();
        npc.components.insert((ESS_TYPE, COMPONENT_ESS.to_owned()));
        content.insert("npc_data", npc);
        content
    }

    fn reg() -> Registry {
        reg_in(
            content_with_ess_type(),
            crate::celestial::Catalogue::default(),
        )
    }

    fn reg_in(
        content: crate::content::Content,
        celestials: crate::celestial::Catalogue,
    ) -> Registry {
        crate::testkit::registry_with(|c| {
            c.celestials = celestials;
            c.characters = [PA, PB]
                .into_iter()
                .enumerate()
                .map(|(n, id)| crate::model::Character {
                    character_id: id,
                    ship_id: 6_001 + n as i64,
                    name: format!("Pilot {id}"),
                    ..crate::model::Character::default()
                })
                .collect();
        })
        .with_content(std::sync::Arc::new(content))
    }

    /// `split` always adds up to its input, to the cent, and the bank gets
    /// the cut.
    #[test]
    fn split_sums_to_the_input() {
        for amount in [0.01, 1.0, 99.99, 1234.57, 1_000_003.31] {
            let (pilot, bank) = split(amount, 0.4);
            assert!(((pilot + bank) - amount).abs() < 1e-9, "{amount}");
            assert!(pilot >= 0.0 && bank >= 0.0);
            assert_eq!(cents(pilot), pilot);
            assert_eq!(cents(bank), bank);
        }
        assert_eq!(split(100.0, 0.4), (60.0, 40.0));
        assert_eq!(split(100.0, 0.0), (100.0, 0.0));
        assert_eq!(split(100.0, 1.0), (0.0, 100.0));
        // A cut outside 0..=1 is clamped, never a negative payout.
        assert_eq!(split(100.0, 7.0), (0.0, 100.0));
        // Not a payable amount: untouched.
        assert_eq!(split(0.0, 0.4), (0.0, 0.0));
        assert!(split(f64::NAN, 0.4).0.is_nan());
    }

    /// One ESS per constellation, in its lowest sov-claimable system: not
    /// high-sec, low-sec or J-space, and the second null system has none.
    #[test]
    fn one_ess_per_sov_null_constellation() {
        let u = Universe::fixture();
        assert_eq!(ess_system(&u, 29_000_002), Some(ESS));
        assert!(has_ess(&u, ESS));
        assert!(!has_ess(&u, OTHER_NULL));
        assert!(!has_ess(&u, LOW));
        assert!(!has_ess(&u, HIGH));
        assert!(!has_ess(&u, 39_000_006));
        assert_eq!(ess_system(&u, 29_000_001), None);
        assert!(!has_ess(&u, 1));
    }

    /// The panel dict has every key the client subscripts.
    #[test]
    fn the_data_dict_has_every_subscripted_key() {
        let reg = reg();
        let d = data(&reg, ESS, T0);
        for key in [
            "essID",
            "mainBankLink",
            "mainValue",
            "reserveValue",
            "reserveBankPulsesTotal",
            "reserveBankPulsesRemaining",
            "reserveBankLastPulseInitiated",
            "reserveBankActiveLinks",
        ] {
            assert!(
                crate::pyutil::dict_get(&d, key.as_bytes()).is_some(),
                "{key}"
            );
        }
    }

    /// A deposit raises the bank and the depositor's share, nobody else's,
    /// and the pilots in the system are told.
    #[test]
    fn a_deposit_fills_the_bank_and_the_share() {
        let mut reg = reg();
        deposit(&mut reg, ESS, PA, 25.0, T0);
        deposit(&mut reg, ESS, PB, 10.0, T0);
        deposit(&mut reg, ESS, PA, 5.5, T0);
        assert_eq!(bank(&reg, ESS, T0).value, 40.5);
        assert_eq!(share(&reg, ESS, PA), 30.5);
        assert_eq!(share(&reg, ESS, PB), 10.0);
        assert_eq!(bank(&reg, OTHER_NULL, T0).value, 0.0);
        deposit(&mut reg, ESS, PA, -3.0, T0);
        deposit(&mut reg, ESS, PA, f64::NAN, T0);
        assert_eq!(bank(&reg, ESS, T0).value, 40.5);
    }

    /// `/essdeposit` funds the bank and the caller's share; a system without
    /// an ESS and a bad amount change nothing.
    #[test]
    fn the_gm_deposit_funds_the_bank_or_refuses() {
        let mut reg = reg();
        let said = gm_deposit_in(&mut reg, Some(ESS), Some(PA), &["1000"], T0);
        assert!(said.starts_with("deposited"), "{said}");
        assert_eq!(bank(&reg, ESS, T0).value, 1000.0);
        assert_eq!(share(&reg, ESS, PA), 1000.0);
        for (system, args) in [
            (Some(LOW), vec!["500"]),
            (Some(ESS), vec!["nan"]),
            (Some(ESS), vec!["-1"]),
            (Some(ESS), vec![]),
            (None, vec!["500"]),
        ] {
            let said = gm_deposit_in(&mut reg, system, Some(PA), &args, T0);
            assert!(!said.starts_with("deposited"), "{said}");
        }
        assert_eq!(bank(&reg, ESS, T0).value, 1000.0);
        assert_eq!(bank(&reg, LOW, T0).value, 0.0);
        assert_eq!(share(&reg, ESS, PA), 1000.0);
    }

    /// **The bank survives a restart**: the balance and the shares are delta
    /// rows, a snapshot carries them, and the world that loads it pays the
    /// autopayment from them.
    #[test]
    fn a_restart_keeps_the_bank_and_pays_from_it() {
        let mut reg = reg();
        deposit(&mut reg, ESS, PA, 12.34, T0);
        deposit(&mut reg, ESS, PB, 20.0, T0);
        assert!(crate::delta_kinds::is_registered(KIND));
        assert!(crate::delta_kinds::is_registered(SHARE_KIND));
        let snap = reg.to_snapshot();

        let mut fresh = reg_fresh();
        assert_eq!(bank(&fresh, ESS, T0).value, 0.0);
        fresh.apply_snapshot(snap);
        let b = bank(&fresh, ESS, T0);
        assert_eq!(b.value, 32.34);
        assert_eq!(share(&fresh, ESS, PA), 12.34);
        assert_eq!(share(&fresh, ESS, PB), 20.0);
        let a0 = fresh.character(PA).expect("exists").balance;
        tick(&mut fresh, b.next_autopay);
        let got = fresh.character(PA).expect("exists").balance - a0;
        assert!((got - 12.34 * MAIN_BANK_PAY_RATIO).abs() < 1e-9);
        assert_eq!(share(&fresh, ESS, PA), 0.0);
        assert_eq!(share(&fresh, ESS, PB), 0.0);
        assert!(bank(&fresh, ESS, T0).value < 32.34);
    }

    /// **N deposits give one push**: a deposit only marks the system, `tick`
    /// sends once, and a deposit inside the gap waits for it.
    #[test]
    fn many_deposits_make_one_push() {
        use crate::space_state::{Motion, Presence};
        let mut reg = reg();
        reg.roster_mut().arrive(Presence::arriving(
            PA,
            6_001,
            ESS,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        let _ = reg.take_pending();
        for _ in 0..20 {
            deposit(&mut reg, ESS, PA, 1.0, T0);
        }
        assert_eq!(reg.scheduled_len(), 0, "a deposit sends nothing");
        let sec = FILETIME_SECOND;
        tick(&mut reg, T0 + 20 * sec);
        assert_eq!(reg.scheduled_len(), 1);
        tick(&mut reg, T0 + 21 * sec);
        assert_eq!(reg.scheduled_len(), 1, "nothing new to say");
        deposit(&mut reg, ESS, PA, 1.0, T0 + 22 * sec);
        tick(&mut reg, T0 + 25 * sec);
        assert_eq!(reg.scheduled_len(), 1, "inside the gap");
        tick(&mut reg, T0 + (20 + PUSH_EVERY_SECS) * sec);
        assert_eq!(reg.scheduled_len(), 2, "after the gap");
    }

    /// `held` adds up the banks.
    #[test]
    fn held_counts_the_banks() {
        let mut reg = reg();
        deposit(&mut reg, ESS, PA, 10.0, T0);
        deposit(&mut reg, OTHER_NULL, PB, 5.0, T0);
        assert_eq!(held(&reg), 15.0);
    }

    /// Before the interval nothing is paid; after it each contributor is
    /// paid their ratio of their own share, the bank loses what was paid,
    /// the journal has a ref-155 row, and the next one is an interval on.
    #[test]
    fn the_autopayment_pays_each_share_by_the_ratio() {
        let mut reg = reg();
        deposit(&mut reg, ESS, PA, 100.0, T0);
        deposit(&mut reg, ESS, PB, 40.0, T0);
        let before = |reg: &Registry, id| reg.character(id).expect("exists").balance;
        let (a0, b0) = (before(&reg, PA), before(&reg, PB));
        tick(&mut reg, T0 + 1);
        assert_eq!(before(&reg, PA), a0);
        assert_eq!(bank(&reg, ESS, T0).value, 140.0);
        let due = T0 + AUTOPAY_INTERVAL_SECS * FILETIME_SECOND;
        tick(&mut reg, due);
        assert!((before(&reg, PA) - a0 - 100.0 * MAIN_BANK_PAY_RATIO).abs() < 1e-9);
        assert!((before(&reg, PB) - b0 - 40.0 * MAIN_BANK_PAY_RATIO).abs() < 1e-9);
        let b = bank(&reg, ESS, due);
        assert!((b.value - 140.0 * (1.0 - MAIN_BANK_PAY_RATIO)).abs() < 1e-9);
        assert_eq!(b.last_autopay, due);
        assert_eq!(
            b.next_autopay,
            due + AUTOPAY_INTERVAL_SECS * FILETIME_SECOND
        );
        assert_eq!(share(&reg, ESS, PA), 0.0);
        // Paid once: the same instant again pays nothing more.
        let a1 = before(&reg, PA);
        tick(&mut reg, due);
        assert_eq!(before(&reg, PA), a1);
    }

    /// A contributor who is gone is dropped unpaid; their share stays in
    /// the bank.
    #[test]
    fn a_missing_contributor_leaves_the_isk_in_the_bank() {
        let mut reg = reg();
        deposit(&mut reg, ESS, 12_345, 80.0, T0);
        tick(&mut reg, T0 + AUTOPAY_INTERVAL_SECS * FILETIME_SECOND);
        assert_eq!(bank(&reg, ESS, T0).value, 80.0);
        assert_eq!(share(&reg, ESS, 12_345), 0.0);
    }

    /// **A kill in an ESS system pays the pilot the rest**, and the bank
    /// the cut; the two add up to what the system paid.
    #[test]
    fn a_kill_in_an_ess_system_is_split() {
        use crate::npc::bounty::{BOUNTY_PERIOD, book_kill, pending};
        let mut reg = reg();
        let now = crate::pyutil::filetime_now();
        let full = crate::econ_dbs::payout(&reg, ESS, 1000.0, now);
        let got = book_kill(&mut reg, PA, ESS, 1, 1000.0, BOUNTY_PERIOD);
        let (pilot, cut) = split(full, BOUNTIES_CUT);
        assert_eq!(got, pilot);
        assert_eq!(pending(&reg, PA, ESS).expect("owed").amount, pilot);
        assert_eq!(bank(&reg, ESS, now).value, cut);
        assert_eq!(share(&reg, ESS, PA), cut);
        assert!(((pilot + cut) - full).abs() < 1e-9);
        assert!(cut > 0.0, "the cut is not nothing");
    }

    /// **No ESS means an unchanged payout**: a low-sec, a high-sec and a
    /// second null system of the same constellation pay in full and store
    /// no bank.
    #[test]
    fn a_system_without_an_ess_pays_in_full() {
        use crate::npc::bounty::{BOUNTY_PERIOD, book_kill, pending};
        let mut reg = reg();
        let now = crate::pyutil::filetime_now();
        for system in [LOW, HIGH, OTHER_NULL] {
            let full = crate::econ_dbs::payout(&reg, system, 1000.0, now);
            let got = book_kill(&mut reg, PA, system, 1, 1000.0, BOUNTY_PERIOD);
            assert_eq!(got, full, "{system}");
            assert_eq!(pending(&reg, PA, system).expect("owed").amount, full);
        }
        assert!(reg.deltas().of(KIND).is_empty());
        assert!(reg.deltas().of(SHARE_KIND).is_empty());
    }

    // ---- ESS-2 -----------------------------------------------------------

    const SEC: i64 = FILETIME_SECOND;

    /// Put `who` in `system` at `at`, in their own ship.
    fn place(reg: &mut Registry, who: i64, system: i64, by: f64) {
        use crate::space_state::{Motion, Presence};
        let p = ess_position(reg, ESS);
        let at = [p[0] + by, p[1], p[2]];
        let ship = reg.character(who).expect("exists").ship_id;
        reg.roster_mut().arrive(Presence::arriving(
            who,
            ship,
            system,
            Motion::arrived(at, [0.0; 3]),
        ));
    }

    fn done() -> BlueTime {
        T0 + LINK_DURATION_SECS * SEC
    }

    /// The table of ESS systems is the universe rule, built once.
    #[test]
    fn the_table_agrees_with_the_rule() {
        let reg = reg();
        let u = Universe::fixture();
        for s in u.systems() {
            assert_eq!(has_ess_in(&reg, s.id), has_ess(&u, s.id), "{}", s.id);
        }
        assert!(has_ess_in(&reg, ESS));
        assert_eq!(ess_systems(&reg).len(), all_ess(&u).len());
    }

    /// The beacon's slim carries a `name` through the slim-extra sources;
    /// the ESS ball and a non-ESS id get none.
    #[test]
    fn the_beacon_slim_is_named() {
        let reg = reg();
        let ex = crate::slim_extra::for_slims(&reg, &[beacon_id(ESS), ess_id(ESS), beacon_id(LOW)]);
        let mine: Vec<_> = ex.iter().filter(|(id, _)| *id == beacon_id(ESS)).collect();
        assert_eq!(mine.len(), 1);
        let name = mine[0]
            .1
            .iter()
            .find(|(k, _)| *k == "name")
            .map(|(_, v)| v.clone());
        let Some(PyValue::Unicode(n)) = name else {
            panic!("no name: {ex:?}");
        };
        assert!(n.ends_with(" - ESS Beacon") && n.len() > " - ESS Beacon".len());
        assert!(
            ex.iter()
                .all(|(id, _)| *id != ess_id(ESS) && *id != beacon_id(LOW))
        );
    }

    /// **The ball and the beacon**: one ESS ball (local) and one beacon (global)
    /// in an ESS system, none elsewhere, with different ids, the beacon inside
    /// link range of the ESS, and both resolve as targets.
    #[test]
    fn the_ess_has_a_ball_and_a_warp_beacon() {
        let reg = reg();
        let local = balls_in(&reg, ESS);
        let global = beacon_balls_in(&reg, ESS);
        assert_eq!(local.len(), 1);
        assert_eq!(global.len(), 1);
        assert_eq!(local[0].0.ball_id, ess_id(ESS));
        assert_eq!(local[0].1.type_id, ESS_TYPE);
        assert_eq!(global[0].0.ball_id, beacon_id(ESS));
        assert_ne!(ess_id(ESS), beacon_id(ESS));
        let gap = crate::space_state::distance(local[0].0.position, global[0].0.position);
        assert!(
            gap < MAX_LINK_DISTANCE,
            "a pilot who warps to the beacon can link"
        );
        for system in [OTHER_NULL, LOW, HIGH] {
            assert!(balls_in(&reg, system).is_empty());
            assert!(beacon_balls_in(&reg, system).is_empty());
        }
        assert_eq!(
            target(&reg, ess_id(ESS)).expect("a target").position,
            local[0].0.position
        );
        assert_eq!(
            target(&reg, beacon_id(ESS)).expect("a target").position,
            global[0].0.position
        );
        assert!(target(&reg, ess_id(OTHER_NULL)).is_none());
        assert!(target(&reg, beacon_id(HIGH)).is_none());
        assert!(target(&reg, 12).is_none());
        // The panel's `beaconID` is the beacon, `essID` the ESS.
        let d = data(&reg, ESS, T0);
        assert_eq!(
            crate::pyutil::dict_get(&d, b"beaconID"),
            Some(&PyValue::Int(beacon_id(ESS)))
        );
    }

    /// A link: it appears in the panel's dict with its pilot and its span,
    /// and the pilots in the system are told **at once**, with no throttle.
    #[test]
    fn a_link_shows_in_the_panel_and_pushes_at_once() {
        let mut reg = reg();
        place(&mut reg, PA, ESS, 1_000.0);
        let _ = reg.take_pending();
        let id = link(&mut reg, PA, T0).expect("links");
        let d = data(&reg, ESS, T0);
        let l = crate::pyutil::dict_get(&d, b"mainBankLink").expect("key");
        assert_eq!(
            crate::pyutil::dict_get(l, b"characterID"),
            Some(&PyValue::Int(PA))
        );
        assert_eq!(
            crate::pyutil::dict_get(l, b"linkID"),
            Some(&PyValue::Int(id))
        );
        assert_eq!(
            crate::pyutil::dict_get(l, b"startedAt"),
            Some(&PyValue::Int(T0))
        );
        assert_eq!(
            crate::pyutil::dict_get(l, b"completesAt"),
            Some(&PyValue::Int(done()))
        );
        assert_eq!(reg.scheduled_len(), 2, "the event and the new data");
        assert!(linked(&reg, PA));
        assert!(!linked(&reg, PB));
    }

    /// **Each refusal**, in the client's order.
    #[test]
    fn each_link_refusal() {
        let mut reg = reg();
        // Not in space.
        assert_eq!(link(&mut reg, PA, T0), Err(Refusal::NotInSpace));
        // No ESS here.
        place(&mut reg, PA, OTHER_NULL, 0.0);
        assert_eq!(link(&mut reg, PA, T0), Err(Refusal::NoEss));
        // Not on the grid, then on it but out of range.
        place(&mut reg, PA, ESS, crate::grid::GRID_ENTER * 2.0);
        assert_eq!(link(&mut reg, PA, T0), Err(Refusal::NotOnGrid));
        place(&mut reg, PA, ESS, MAX_LINK_DISTANCE * 3.0);
        assert_eq!(link(&mut reg, PA, T0), Err(Refusal::OutOfRange));
        // In range: a link; a second link by the same pilot, and one by
        // another while it stands.
        place(&mut reg, PA, ESS, 500.0);
        place(&mut reg, PB, ESS, 800.0);
        assert!(link(&mut reg, PA, T0).is_ok());
        assert_eq!(link(&mut reg, PA, T0), Err(Refusal::Already));
        assert_eq!(link(&mut reg, PB, T0), Err(Refusal::Occupied));
        assert_eq!(link_of(&reg, ESS).expect("one link").character, PA);
    }

    /// The hull check: list 231 admits by type, group or category, and with no
    /// list there is nothing to check against.
    #[test]
    fn only_the_listed_hulls_link() {
        let list = crate::dungeon_data::TypeList {
            inc_groups: vec![7],
            exc_types: vec![9],
            ..Default::default()
        };
        assert!(ship_may_link(Some(&list), 8, 7, 0));
        assert!(!ship_may_link(Some(&list), 8, 6, 0));
        assert!(!ship_may_link(Some(&list), 9, 7, 0));
        assert!(ship_may_link(None, 8, 6, 0));
    }

    /// **Range break**: a linker who drifts out of range loses the link, is
    /// told why, and is paid nothing.
    #[test]
    fn leaving_range_breaks_the_link() {
        let mut reg = reg();
        deposit(&mut reg, ESS, PB, 100.0, T0);
        place(&mut reg, PA, ESS, 1_000.0);
        link(&mut reg, PA, T0).expect("links");
        let a0 = reg.character(PA).expect("exists").balance;
        let _ = reg.take_pending();
        tick(&mut reg, T0 + 5 * SEC);
        assert!(link_of(&reg, ESS).is_some(), "in range: it stands");
        let told = reg.scheduled_len();
        place(&mut reg, PA, ESS, MAX_LINK_DISTANCE * 2.0);
        tick(&mut reg, T0 + 10 * SEC);
        assert!(link_of(&reg, ESS).is_none());
        assert_eq!(
            reg.scheduled_len(),
            told + 2,
            "the disconnect and the new data"
        );
        tick(&mut reg, done() + SEC);
        assert_eq!(reg.character(PA).expect("exists").balance, a0);
        assert_eq!(bank(&reg, ESS, T0).value, 100.0);
    }

    /// Leaving the system, and changing ship, break it the same way.
    #[test]
    fn leaving_or_changing_ship_breaks_the_link() {
        let mut reg = reg();
        place(&mut reg, PA, ESS, 1_000.0);
        link(&mut reg, PA, T0).expect("links");
        reg.roster_mut().set_ship_id(PA, 7_777);
        tick(&mut reg, T0 + SEC);
        assert!(link_of(&reg, ESS).is_none(), "a new ship");

        place(&mut reg, PA, ESS, 1_000.0);
        link(&mut reg, PA, T0 + 2 * SEC).expect("links again");
        reg.roster_mut().leave(PA);
        tick(&mut reg, T0 + 3 * SEC);
        assert!(link_of(&reg, ESS).is_none(), "docked or gone");
    }

    /// A manual unlink ends the link; a second has nothing to end.
    #[test]
    fn a_manual_unlink() {
        let mut reg = reg();
        place(&mut reg, PA, ESS, 1_000.0);
        link(&mut reg, PA, T0).expect("links");
        assert!(unlink(&mut reg, PA, T0 + SEC));
        assert!(!unlink(&mut reg, PA, T0 + SEC));
        assert!(!linked(&reg, PA));
        // The way is free for another.
        place(&mut reg, PB, ESS, 1_000.0);
        assert!(link(&mut reg, PB, T0 + 2 * SEC).is_ok());
    }

    /// **The theft pays once and empties the bank**: the whole bank to the
    /// linker, the shares void, the ledger booked, the ESS cooling, and the
    /// autopayment after it has nothing to pay.
    #[test]
    fn a_theft_pays_once_and_empties_the_bank() {
        let mut reg = reg();
        deposit(&mut reg, ESS, PA, 100.0, T0);
        deposit(&mut reg, ESS, PA, 20.5, T0);
        place(&mut reg, PB, ESS, 1_000.0);
        link(&mut reg, PB, T0).expect("links");
        let (a0, b0) = (
            reg.character(PA).expect("exists").balance,
            reg.character(PB).expect("exists").balance,
        );
        tick(&mut reg, done() - SEC);
        assert!(link_of(&reg, ESS).is_some(), "not yet");
        assert_eq!(reg.character(PB).expect("exists").balance, b0);
        tick(&mut reg, done());
        assert!(link_of(&reg, ESS).is_none());
        assert_eq!(reg.character(PB).expect("exists").balance, b0 + 120.5);
        assert_eq!(bank(&reg, ESS, done()).value, 0.0);
        assert_eq!(share(&reg, ESS, PA), 0.0);
        assert_eq!(stolen(&reg, ESS, PB), 120.5);
        // Paid once.
        tick(&mut reg, done() + SEC);
        assert_eq!(reg.character(PB).expect("exists").balance, b0 + 120.5);
        // The contributor's autopayment finds nothing.
        let next = bank(&reg, ESS, done()).next_autopay;
        tick(&mut reg, next);
        assert_eq!(reg.character(PA).expect("exists").balance, a0);
        assert_eq!(reg.character(PB).expect("exists").balance, b0 + 120.5);
        // The ledger.
        let rows = thefts(&reg, ESS);
        let PyValue::List(rows) = rows else {
            panic!("a list")
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(
            crate::pyutil::dict_get(&rows[0], b"characterID"),
            Some(&PyValue::Int(PB))
        );
        assert_eq!(
            crate::pyutil::dict_get(&rows[0], b"amount"),
            Some(&PyValue::Real(120.5))
        );
        assert_eq!(thefts(&reg, OTHER_NULL), PyValue::List(vec![]));
    }

    /// After a theft the ESS cools for the buffer time, then links again.
    #[test]
    fn a_stolen_ess_cools_before_the_next_link() {
        let mut reg = reg();
        deposit(&mut reg, ESS, PA, 50.0, T0);
        place(&mut reg, PB, ESS, 1_000.0);
        link(&mut reg, PB, T0).expect("links");
        tick(&mut reg, done());
        assert_eq!(link(&mut reg, PB, done() + SEC), Err(Refusal::Cooling));
        assert!(link(&mut reg, PB, done() + (BUFFER_TIME_SECS + 1) * SEC).is_ok());
    }

    /// The theft ledger and the bank survive a restart.
    #[test]
    fn the_ledger_survives_a_restart() {
        let mut reg = reg();
        assert!(crate::delta_kinds::is_registered(THEFT_KIND));
        deposit(&mut reg, ESS, PA, 30.0, T0);
        place(&mut reg, PB, ESS, 1_000.0);
        link(&mut reg, PB, T0).expect("links");
        tick(&mut reg, done());
        deposit(&mut reg, ESS, PA, 5.0, done());
        let snap = reg.to_snapshot();
        let mut fresh = reg_fresh();
        fresh.apply_snapshot(snap);
        assert_eq!(stolen(&fresh, ESS, PB), 30.0);
        assert_eq!(bank(&fresh, ESS, done()).value, 5.0);
        // Links are not persisted: a restart ends them.
        assert!(link_of(&fresh, ESS).is_none());
    }

    /// **Safe logoff refuses while linked**, with the client's own label.
    #[test]
    fn a_linked_pilot_cannot_log_off_safely() {
        let mut reg = reg();
        place(&mut reg, PA, ESS, 1_000.0);
        let labels = |reg: &mut Registry| match crate::safe_logoff::start(reg, Some(PA), T0)
            .expect("answers")
        {
            PyValue::List(l) => l,
            other => panic!("{other:?}"),
        };
        assert!(labels(&mut reg).is_empty(), "free to go");
        link(&mut reg, PA, T0).expect("links");
        assert_eq!(
            labels(&mut reg),
            vec![PyValue::Unicode(SAFE_LOGOFF_LINKED.to_owned())]
        );
    }

    /// The Agency: every ESS system with the four keys, and the details
    /// with the five.
    #[test]
    fn the_agency_lists_every_ess() {
        let mut reg = reg();
        deposit(&mut reg, ESS, PA, 70.0, T0);
        let PyValue::Dict(rows) = agency_data(&reg, T0) else {
            panic!("a dict")
        };
        assert_eq!(rows.len(), ess_systems(&reg).len());
        let mine = rows
            .iter()
            .find(|(k, _)| *k == PyValue::Int(ESS))
            .map(|(_, v)| v)
            .expect("the ESS system");
        for key in ["currentOutput", "mainValue", "reserveValue", "vaultOpen"] {
            assert!(
                crate::pyutil::dict_get(mine, key.as_bytes()).is_some(),
                "{key}"
            );
        }
        assert_eq!(
            crate::pyutil::dict_get(mine, b"mainValue"),
            Some(&PyValue::Real(70.0))
        );
        let d = system_details(&reg, ESS, T0);
        for key in [
            "autopaymentWindowState",
            "activeLinks",
            "totalPulses",
            "reserveTimeWindowState",
            "reserveTimeChosen",
        ] {
            assert!(
                crate::pyutil::dict_get(&d, key.as_bytes()).is_some(),
                "{key}"
            );
        }
    }

    /// The gauge falls from long to medium to short as the autopayment nears.
    #[test]
    fn the_autopay_gauge_steps_down() {
        let b = Bank::fresh(T0);
        let at =
            |frac: f64| autopay_state(&b, T0 + (AUTOPAY_INTERVAL_SECS as f64 * frac) as i64 * SEC);
        // The end states are the client's (long at the start, short at the
        // end); where the steps fall between is our [A] thirds, not asserted.
        assert_eq!(at(0.0), 0);
        assert_eq!(at(2.0), 2);
        let mut last = 0;
        for n in 0..=40 {
            let s = at(f64::from(n) / 20.0);
            assert!(s >= last, "the gauge never steps back up");
            last = s;
        }
    }

    /// A star of `radius` in the ESS system.
    fn reg_with_star(radius: f64) -> Registry {
        reg_in(
            content_with_ess_type(),
            crate::celestial::Catalogue::parse(&format!(
                "star\t6200010\t9900\t{radius}\t1000\t2000\t3000\t{ESS}\t0\t0\n"
            )),
        )
    }

    /// **The ESS is never inside its star**, nor on the hub: the sun warp-in
    /// direction is `cos`/`sin` of the radius, so a large radius whose sine is
    /// near 1 once put the old fixed offset inside the star.
    #[test]
    fn the_ess_is_outside_the_star() {
        for radius in [
            0.0,
            2.0e6,
            6.96e8,
            1.0e9,
            1_000_000_000.0 + 1.0e7,
            std::f64::consts::FRAC_PI_2 + 2.0 * std::f64::consts::PI * 100_000_000.0,
            std::f64::consts::PI * 100_000_000.5,
        ] {
            let reg = reg_with_star(radius);
            let star = [1000.0, 2000.0, 3000.0];
            let at = ess_position(&reg, ESS);
            let gap = crate::space_state::distance(at, star);
            assert!(gap > radius * 1.01 + 1.0e8, "radius {radius}: {gap}");
            let hub = crate::sov_hub::position_in(&reg, ESS);
            assert!(
                crate::space_state::distance(at, hub) > 1.0e8,
                "radius {radius}: off the hub"
            );
            assert!(
                crate::space_state::distance(beacon_position(&reg, ESS), star) > radius,
                "radius {radius}: the beacon too"
            );
        }
    }

    /// No `essBracket` row in the bundle: no ball, no beacon, no target.
    #[test]
    fn no_ess_type_row_means_no_ball() {
        let reg = reg_in(
            crate::content::Content::fixture(),
            crate::celestial::Catalogue::default(),
        );
        assert_eq!(ess_type(&reg), None);
        assert!(has_ess_in(&reg, ESS), "the bank is still there");
        assert!(balls_in(&reg, ESS).is_empty());
        assert!(beacon_balls_in(&reg, ESS).is_empty());
        assert!(target(&reg, ess_id(ESS)).is_none());
        assert!(target(&reg, beacon_id(ESS)).is_none());
    }

    /// The ESS band is clear of the abyss sites and the beacons.
    #[test]
    fn the_ess_ids_keep_to_their_own_band() {
        assert!(ess_id(ESS) > crate::abyss::SITE_END);
        assert!(!crate::abyss::is_abyss_site(ess_id(ESS)));
        assert_ne!(ess_id(ESS), beacon_id(ESS));
        assert_eq!(system_of_ess(ess_id(ESS)), Some(ESS));
        assert_eq!(system_of_beacon(beacon_id(ESS)), Some(ESS));
        assert_eq!(system_of_ess(beacon_id(ESS)), None);
    }

    /// Range is surface to surface: the ship's own radius counts too.
    #[test]
    fn range_takes_off_both_radii() {
        let reg = reg();
        let ship = 6_001;
        let at = [0.0; 3];
        let centre = crate::space_state::distance(at, ess_position(&reg, ESS));
        let r = range_to_ess(&reg, ESS, at, ship, PA);
        assert!(r <= centre - ess_radius(&reg));
        assert!(r >= 0.0);
    }

    /// **A hard logoff ends the link** through the real session hooks, and a
    /// bot (no session) is unlinked by the hook itself.
    #[test]
    fn a_logoff_ends_the_link() {
        use crate::session_table::SessionEntry;
        let mut reg = reg();
        deposit(&mut reg, ESS, PB, 100.0, T0);
        place(&mut reg, PA, ESS, 1_000.0);
        let client = 10_000_000_001;
        reg.session_opened(SessionEntry {
            client_id: client,
            session_id: client,
            user_id: 1,
            character_id: None,
        });
        reg.session_selected_character(client, PA);
        link(&mut reg, PA, T0).expect("links");
        assert!(linked(&reg, PA));
        reg.session_closed(client);
        assert!(!linked(&reg, PA), "the logoff hook ended it");
        assert!(link_of(&reg, ESS).is_none());
        tick(&mut reg, done() + SEC);
        assert_eq!(bank(&reg, ESS, T0).value, 100.0, "nothing was stolen");
        // A pilot with no session at all.
        place(&mut reg, PB, ESS, 1_000.0);
        link(&mut reg, PB, T0 + 2 * SEC).expect("links");
        on_logoff(&mut reg, PB);
        assert!(!linked(&reg, PB));
    }

    /// **Contributors are warned once per window**, not on every link: a
    /// link and unlink loop must not flood their notification list.
    #[test]
    fn the_warning_is_throttled() {
        let mut reg = reg();
        deposit(&mut reg, ESS, PB, 100.0, T0);
        place(&mut reg, PA, ESS, 1_000.0);
        let before = crate::notifications::of(reg.deltas(), PB).len();
        for n in 0..5 {
            link(&mut reg, PA, T0 + n * SEC).expect("links");
            assert!(unlink(&mut reg, PA, T0 + n * SEC));
        }
        assert_eq!(crate::notifications::of(reg.deltas(), PB).len() - before, 1);
        link(&mut reg, PA, T0 + (WARN_EVERY_SECS + 1) * SEC).expect("links");
        assert_eq!(crate::notifications::of(reg.deltas(), PB).len() - before, 2);
    }
}
