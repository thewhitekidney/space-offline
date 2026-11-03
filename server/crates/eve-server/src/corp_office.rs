//! Corporation offices: the state and the rules, the pure half (CORP-O; lane
//! note `docs/lanes/corp-o.md`).
//!
//! A player corporation **rents one office per NPC station** and keeps its
//! stuff in the office's **seven hangar divisions** (flags `flagCorpSAG1..7`
//! = 115..=121 **[V]**). A division is not a place of its own: it is the
//! `flagID` of an item whose `locationID` is the office item, owned by the
//! corporation. Who may look into division N and who may take from it are
//! two role bits per division (`corpHangarQueryRolesByFlag` and
//! `corpHangarTakeRolesByFlag`, `appConst.py`) **[V]**, and the client's own
//! container controller applies them the same way (`BaseCorpContainer`:
//! `CheckCanQuery`, `CheckCanTake`, `SetAccess` makes a window view-only
//! without the take bit) **[V]**.
//!
//! **The office is an item.** Type 27 (`typeOffice`) **[V]** owned by the
//! corporation, in the station, so its id is the `officeID` the client
//! passes to `GetInventoryFromId` and its contents are found like any other
//! item's (`item::Slot`'s one address space). The item store persists it; the
//! one delta kind below holds what is *about* the rental: who rents where,
//! until when, and whether it lapsed.
//!
//! ```text
//! corpoff  <office item id> -> corp station solarsystem paid_until impounded_at rented_at
//! ```
//!
//! `paid_until` and `rented_at` are wall FILETIMEs made from a game-clock
//! span at the moment of the bill (`game_clock::span`, the lane `clock`'s
//! rule), so `--time-scale` shortens a rental period and a restart at
//! another scale keeps the date it was given. `impounded_at` is 0 while the
//! office is live; an impounded office keeps its contents and stays an item,
//! with `flagID` 6 (`flagImpounded`, which the client tests for on an office
//! item: `reprocessing/ui/controller.py`) **[V]**.
//!
//! The wire, the wallet and the pushes are `service/corp_office.rs`. Every
//! test here uses invented ids.

use crate::delta::Deltas;
use crate::item::ItemType;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND_OFFICE: &str = "corpoff";
/// Every kind, for the registry row.
pub const KINDS: [&str; 1] = [KIND_OFFICE];

// ---------------------------------------------------------------------------
// What the client names
// ---------------------------------------------------------------------------

/// `typeOffice` **[V]** (`inventorycommon/const.py`).
pub const TYPE_OFFICE: i64 = 27;
/// The office item's flag while the rental is live. **[I]**: nothing in the
/// client reads it (it tests only for [`FLAG_IMPOUNDED`]), so any flag that
/// is not a hangar and not a corp division will do; `flagOfficeFolder` (2)
/// **[V]** is the nearest name the client has. Falsified by a window that
/// lists the office as a loose item.
pub const FLAG_OFFICE: i64 = 2;
/// `flagImpounded` **[V]**: what an office item's flag becomes when the
/// rental lapses with items still in it.
pub const FLAG_IMPOUNDED: i64 = 6;
/// `flagCorpDeliveries` **[V]**: the corporation's deliveries hangar at a
/// station (`invCtrl.StationCorpDeliveries.locationFlag`); an item is there
/// when `flagID == 62`, `locationID` is the station and `ownerID` the
/// corporation.
pub const FLAG_CORP_DELIVERIES: i64 = 62;
/// `containerCorpMarket` **[V]**: the deliveries window's inventory id
/// (`GetInventory(containerCorpMarket, corpID)`).
pub const CONTAINER_CORP_MARKET: i64 = 10_012;
/// `flagCorpSAG1..7` **[V]**: division 1 is flag 115.
pub const FLAG_SAG: [i64; DIVISIONS] = [115, 116, 117, 118, 119, 120, 121];
/// Seven hangar divisions (`corpDivisionsByFlag`) **[V]**.
pub const DIVISIONS: usize = 7;

/// The division (0-based) a hangar flag is, if it is one of the seven.
/// `flagCorpGoalDeliveries` (184) is the projects' deliveries hangar, not a
/// division, and is not here.
pub fn division_of_flag(flag: i64) -> Option<usize> {
    FLAG_SAG.iter().position(|f| *f == flag)
}

/// The item type the office is, for `item::find_in`: Office, group 16,
/// category 3 (`categoryStation`) **[V]** (the build's type table; the
/// category is also `inventorycommon/const.py`'s). Not grantable: only a
/// rental mints one.
pub const OFFICE_TYPES: &[ItemType] = &[ItemType {
    type_id: TYPE_OFFICE,
    name: "Office",
    group_id: 16,
    category_id: 3,
    singleton: true,
    volume: 0.0,
    capacity: 0.0,
}];

// ---------------------------------------------------------------------------
// Roles
// ---------------------------------------------------------------------------

/// `appConst.corpRoleDirector` **[V]**: the client offers "unrent" to a
/// director (`baseController.CanUnrent`).
pub const ROLE_DIRECTOR: i64 = 1;
/// `appConst.corpRoleAccountant` **[V]**.
pub const ROLE_ACCOUNTANT: i64 = 256;
/// `appConst.corpRoleJuniorAccountant` **[V]**.
pub const ROLE_JUNIOR_ACCOUNTANT: i64 = 4_503_599_627_370_496;
/// `appConst.corpRoleTrader` **[V]**.
pub const ROLE_TRADER: i64 = 18_014_398_509_481_984;
/// `appConst.corpRoleCanRentOffice` **[V]**: the Rent Office button needs it
/// (`baseController.CanRent`).
pub const ROLE_CAN_RENT_OFFICE: i64 = 562_949_953_421_312;
/// `appConst.corpRoleHangarCanTake1..7`, by division **[V]**.
pub const ROLE_HANGAR_TAKE: [i64; DIVISIONS] =
    [8_192, 16_384, 32_768, 65_536, 131_072, 262_144, 524_288];
/// `appConst.corpRoleHangarCanQuery1..7`, by division **[V]**.
pub const ROLE_HANGAR_QUERY: [i64; DIVISIONS] = [
    1_048_576, 2_097_152, 4_194_304, 8_388_608, 16_777_216, 33_554_432, 67_108_864,
];
/// All seven take roles and all seven query roles.
pub const ALL_HANGAR_ROLES: i64 = ROLE_HANGAR_TAKE[0]
    | ROLE_HANGAR_TAKE[1]
    | ROLE_HANGAR_TAKE[2]
    | ROLE_HANGAR_TAKE[3]
    | ROLE_HANGAR_TAKE[4]
    | ROLE_HANGAR_TAKE[5]
    | ROLE_HANGAR_TAKE[6]
    | ROLE_HANGAR_QUERY[0]
    | ROLE_HANGAR_QUERY[1]
    | ROLE_HANGAR_QUERY[2]
    | ROLE_HANGAR_QUERY[3]
    | ROLE_HANGAR_QUERY[4]
    | ROLE_HANGAR_QUERY[5]
    | ROLE_HANGAR_QUERY[6];
/// What a founding CEO holds beside the roles it already has, so the client
/// builds the Rent Office button and opens every division: both checks are
/// plain bits (`baseController.CanRent`, `BaseCorpContainer.CheckCanQuery`)
/// and director implies neither **[V]**. **[A]** that Tranquility gives a CEO
/// every role; falsified by a CEO who cannot open a division on the real
/// server.
pub const CEO_OFFICE_ROLES: i64 = ROLE_CAN_RENT_OFFICE | ALL_HANGAR_ROLES;

/// What the acting member holds in the corporation, for the office.
///
/// `roles` is the client's `session.corprole` at a station that is neither
/// the HQ nor the base: `rolesAtAll | rolesAtOther` **[V]**
/// (`sessionChecker.canTakeFromCorpDivision` uses `rolesAtOther` there, and
/// this server has no HQ or base station **[A]**: `corp.rs` ignores
/// `baseID`). **No director shortcut**: the client's hangar checks are the
/// bits alone, so a director without the bit sees a view-only window and the
/// server agrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing {
    pub roles: i64,
}

impl Standing {
    /// May rent an office.
    pub fn can_rent(&self) -> bool {
        self.roles & ROLE_CAN_RENT_OFFICE != 0
    }

    /// May give up an office (and trash what it impounded).
    pub fn can_unrent(&self) -> bool {
        self.roles & ROLE_DIRECTOR != 0
    }

    /// May pay the impound release and have the items back: the directors
    /// (`OfficesPanel._load_buttons` shows the button to
    /// `corpRoleDirector` only) **[V]**.
    pub fn can_release(&self) -> bool {
        self.can_unrent()
    }

    /// May look into the division behind `flag`.
    pub fn can_query(&self, flag: i64) -> bool {
        division_of_flag(flag).is_some_and(|d| self.roles & ROLE_HANGAR_QUERY[d] != 0)
    }

    /// May put into and take out of the division behind `flag`. The client
    /// makes a window with no take bit view-only (`SetAccess`) **[V]**, which
    /// also stops a drop, so the same bit gates both ways.
    pub fn can_take(&self, flag: i64) -> bool {
        division_of_flag(flag).is_some_and(|d| self.roles & ROLE_HANGAR_TAKE[d] != 0)
    }

    /// Whether the client lists the deliveries hangar at all:
    /// `HasCorpDeliveryRoles` is accountant, junior accountant or trader
    /// **[V]** (`inventorysvc.py`).
    pub fn sees_deliveries(&self) -> bool {
        self.roles & (ROLE_ACCOUNTANT | ROLE_JUNIOR_ACCOUNTANT | ROLE_TRADER) != 0
    }

    /// Whether items in the deliveries hangar are unlocked for this member:
    /// accountant or trader (`itemLocking._CanTakeFromDeliveries`) **[V]**.
    pub fn can_take_deliveries(&self) -> bool {
        self.roles & (ROLE_ACCOUNTANT | ROLE_TRADER) != 0
    }
}

// ---------------------------------------------------------------------------
// The rental
// ---------------------------------------------------------------------------

/// `appConst.rentalPeriodOffice`, in days **[V]**: the dialog tells the
/// player `duration = rentalPeriodOffice * DAY` (`OfficesPanel._rent_office`).
pub const RENT_PERIOD_DAYS: i64 = 30;

/// The rent for one period, ISK **[A]**. **Why it could not be found:** the
/// client never sees a price. `GetPriceQuote` is a server answer and
/// `AskPayOfficeRentalFee` shows whatever it returns (`cost`); the figure
/// lives in the station's `officeRentalCost`, which this build's
/// `stations/stations.tsv` does not carry (owner, operation, services and the
/// reprocessing columns only). EvEJS hard-codes 10,000 for every station
/// (`docs/evejs/review/static-data.md`), a placeholder of theirs. 100,000 is
/// a tenth of the corporation fee's order, so a bot corp that has banked a
/// few days' tax can pay it, and a lapsed rent is something a friends' corp
/// notices. Falsified by the owner's reading of a real station's quote.
pub const OFFICE_RENT_ISK: f64 = 100_000.0;

/// Offices a station holds **[A]**. **Why it could not be found:** the client
/// only asks (`GetEmptyOfficeCount`, shown as `NumAvailableOffices`) and the
/// count is a station property this build's data does not carry. Falsified
/// by the owner wanting a scarcer or a roomier station; at 50 bots in a
/// handful of corps this is not a limit that bites.
pub const OFFICES_PER_STATION: usize = 16;

/// What the impound release costs, in rental periods **[A]**. **Why it could
/// not be found:** `GetImpoundReleasePrice` is a server answer; the client
/// shows it (`CrpJunkAcceptCost`, `cost`). One period's rent is the lowest
/// price that makes letting a rental lapse cost something. Falsified by a
/// corp that finds it cheaper to let offices lapse than to pay rent.
pub const IMPOUND_RELEASE_PERIODS: f64 = 1.0;

/// Wall FILETIME ticks in a game-day at scale 1 (`seed::DAY`).
pub const DAY_TICKS: i64 = crate::seed::DAY;

/// One office rental.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Office {
    pub office_id: i64,
    pub corp: i64,
    pub station: i64,
    pub system: i64,
    /// The rent is paid up to this wall FILETIME.
    pub paid_until: i64,
    /// 0 while live; when the rental lapsed or was given up with items in it.
    pub impounded_at: i64,
    pub rented_at: i64,
}

impl Office {
    pub fn live(&self) -> bool {
        self.impounded_at == 0
    }

    /// Whether a bill is owed at `now`.
    pub fn due(&self, now: i64) -> bool {
        self.live() && now >= self.paid_until
    }

    fn row(&self) -> Vec<String> {
        vec![
            self.corp.to_string(),
            self.station.to_string(),
            self.system.to_string(),
            self.paid_until.to_string(),
            self.impounded_at.to_string(),
            self.rented_at.to_string(),
        ]
    }

    fn from_row(office_id: i64, v: &[String]) -> Option<Office> {
        let n = |i: usize| v.get(i)?.parse::<i64>().ok();
        Some(Office {
            office_id,
            corp: n(0)?,
            station: n(1)?,
            system: n(2)?,
            paid_until: n(3)?,
            impounded_at: n(4)?,
            rented_at: n(5)?,
        })
    }
}

fn key(office_id: i64) -> Vec<String> {
    vec![office_id.to_string()]
}

/// Every office, live and impounded, by id.
pub fn all(d: &Deltas) -> Vec<Office> {
    d.of(KIND_OFFICE)
        .iter()
        .filter_map(|(k, v)| Office::from_row(k.first()?.parse().ok()?, v))
        .collect()
}

/// One office by its item id.
pub fn get(d: &Deltas, office_id: i64) -> Option<Office> {
    Office::from_row(office_id, d.of(KIND_OFFICE).get(&key(office_id))?)
}

/// Write an office.
pub fn put(d: &mut Deltas, o: &Office) {
    d.put(KIND_OFFICE, key(o.office_id), o.row());
}

/// Remove an office row.
pub fn remove(d: &mut Deltas, office_id: i64) {
    d.del(KIND_OFFICE, &key(office_id));
}

/// The corporation's live offices, by id.
pub fn live_of(d: &Deltas, corp: i64) -> Vec<Office> {
    all(d)
        .into_iter()
        .filter(|o| o.corp == corp && o.live())
        .collect()
}

/// The corporation's live office at `station`, if it rents one.
pub fn live_at(d: &Deltas, corp: i64, station: i64) -> Option<Office> {
    all(d)
        .into_iter()
        .find(|o| o.corp == corp && o.station == station && o.live())
}

/// The corporation's impounded office at `station`, if one lapsed there.
pub fn impounded_at(d: &Deltas, corp: i64, station: i64) -> Option<Office> {
    all(d)
        .into_iter()
        .find(|o| o.corp == corp && o.station == station && !o.live())
}

/// The corporations with a live office at `station`, in id order, once each.
pub fn corps_at(d: &Deltas, station: i64) -> Vec<i64> {
    let mut v: Vec<i64> = all(d)
        .into_iter()
        .filter(|o| o.station == station && o.live())
        .map(|o| o.corp)
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Offices still free at `station` (`GetEmptyOfficeCount`).
pub fn free_at(d: &Deltas, station: i64) -> usize {
    let used = all(d)
        .into_iter()
        .filter(|o| o.station == station && o.live())
        .count();
    OFFICES_PER_STATION.saturating_sub(used)
}

/// The next date the rent is paid to, from the one it was paid to: periods
/// are counted from the old due date, not from the day the bill was paid, so
/// a rollover that comes late does not shift every later bill. `period` is
/// the length of one period in wall ticks (`game_clock::span`).
pub fn next_paid_until(paid_until: i64, now: i64, period: i64) -> i64 {
    let period = period.max(1);
    let mut next = paid_until.saturating_add(period);
    // A server that was down for many periods bills one, not all of them.
    if next <= now {
        next = now.saturating_add(period);
    }
    next
}

/// The impound release price, ISK.
pub fn release_price() -> f64 {
    OFFICE_RENT_ISK * IMPOUND_RELEASE_PERIODS
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids throughout.
    const CORP: i64 = 98_000_001;
    const OTHER: i64 = 98_000_002;
    const STATION: i64 = 69_000_001;

    fn office(id: i64, corp: i64, station: i64) -> Office {
        Office {
            office_id: id,
            corp,
            station,
            system: 39_000_001,
            paid_until: 1_000,
            impounded_at: 0,
            rented_at: 10,
        }
    }

    #[test]
    fn a_division_is_a_flag_and_the_flags_are_the_clients() {
        assert_eq!(division_of_flag(FLAG_SAG[0]), Some(0));
        assert_eq!(division_of_flag(FLAG_SAG[6]), Some(6));
        assert_eq!(division_of_flag(FLAG_SAG[6] + 1), None);
        assert_eq!(division_of_flag(114), None);
        // 184 is the projects' deliveries hangar, not a division.
        assert_eq!(division_of_flag(184), None);
        assert_eq!(division_of_flag(FLAG_CORP_DELIVERIES), None);
    }

    #[test]
    fn take_and_query_are_one_bit_each_per_division() {
        let s = |roles| Standing { roles };
        let second = FLAG_SAG[1];
        assert!(!s(0).can_take(second) && !s(0).can_query(second));
        assert!(s(ROLE_HANGAR_TAKE[1]).can_take(second));
        assert!(!s(ROLE_HANGAR_TAKE[1]).can_query(second));
        assert!(s(ROLE_HANGAR_QUERY[1]).can_query(second));
        assert!(!s(ROLE_HANGAR_QUERY[1]).can_take(second));
        // A division's bits are its own.
        assert!(!s(ROLE_HANGAR_TAKE[1] | ROLE_HANGAR_QUERY[1]).can_take(FLAG_SAG[2]));
        // Director buys nothing in a hangar: the client's check is the bit.
        assert!(!s(ROLE_DIRECTOR).can_take(second));
        assert!(!s(ROLE_DIRECTOR).can_query(second));
        // Not a division, not allowed.
        assert!(!s(ALL_HANGAR_ROLES).can_take(FLAG_CORP_DELIVERIES));
        assert!(s(CEO_OFFICE_ROLES).can_take(FLAG_SAG[6]));
        assert!(s(CEO_OFFICE_ROLES).can_query(FLAG_SAG[0]));
        assert!(s(CEO_OFFICE_ROLES).can_rent());
    }

    #[test]
    fn the_role_masks_do_not_overlap_each_other() {
        let mut seen = 0i64;
        for r in ROLE_HANGAR_TAKE.iter().chain(&ROLE_HANGAR_QUERY).chain(&[
            ROLE_CAN_RENT_OFFICE,
            ROLE_DIRECTOR,
            ROLE_ACCOUNTANT,
        ]) {
            assert_eq!(r.count_ones(), 1, "{r}");
            assert_eq!(seen & r, 0, "{r}");
            seen |= r;
        }
    }

    #[test]
    fn rent_unrent_and_deliveries_follow_the_clients_checks() {
        let s = |roles| Standing { roles };
        assert!(s(ROLE_CAN_RENT_OFFICE).can_rent() && !s(ROLE_CAN_RENT_OFFICE).can_unrent());
        assert!(s(ROLE_DIRECTOR).can_unrent() && !s(ROLE_DIRECTOR).can_rent());
        assert!(s(ROLE_JUNIOR_ACCOUNTANT).sees_deliveries());
        assert!(!s(ROLE_JUNIOR_ACCOUNTANT).can_take_deliveries());
        assert!(s(ROLE_ACCOUNTANT).can_take_deliveries());
        assert!(s(ROLE_TRADER).can_take_deliveries());
        assert!(!s(ROLE_DIRECTOR).sees_deliveries());
    }

    #[test]
    fn an_office_row_round_trips_and_lives_in_the_one_kind() {
        let mut d = Deltas::default();
        let o = office(100_000_001, CORP, STATION);
        put(&mut d, &o);
        assert_eq!(get(&d, 100_000_001), Some(o));
        assert_eq!(all(&d), vec![o]);
        assert_eq!(d.of(KIND_OFFICE).len(), 1);
        remove(&mut d, 100_000_001);
        assert_eq!(get(&d, 100_000_001), None);
        assert!(all(&d).is_empty());
    }

    #[test]
    fn a_corp_has_one_live_office_per_station_and_slots_run_out() {
        let mut d = Deltas::default();
        put(&mut d, &office(100_000_001, CORP, STATION));
        assert_eq!(
            live_at(&d, CORP, STATION).map(|o| o.office_id),
            Some(100_000_001)
        );
        assert_eq!(live_at(&d, OTHER, STATION), None);
        assert_eq!(live_at(&d, CORP, STATION + 1), None);
        assert_eq!(corps_at(&d, STATION), vec![CORP]);
        assert_eq!(free_at(&d, STATION), OFFICES_PER_STATION - 1);
        assert_eq!(free_at(&d, STATION + 1), OFFICES_PER_STATION);
        // An impounded office is not live, holds no slot and is not listed.
        let mut lapsed = office(100_000_002, OTHER, STATION);
        lapsed.impounded_at = 5;
        put(&mut d, &lapsed);
        assert_eq!(live_at(&d, OTHER, STATION), None);
        assert_eq!(
            impounded_at(&d, OTHER, STATION).map(|o| o.office_id),
            Some(100_000_002)
        );
        assert_eq!(corps_at(&d, STATION), vec![CORP]);
        assert_eq!(free_at(&d, STATION), OFFICES_PER_STATION - 1);
        assert_eq!(live_of(&d, OTHER), vec![]);
        for i in 0..OFFICES_PER_STATION as i64 {
            put(&mut d, &office(100_000_100 + i, 98_100_000 + i, STATION));
        }
        assert_eq!(free_at(&d, STATION), 0);
    }

    #[test]
    fn a_bill_is_due_at_the_date_and_never_for_an_impounded_office() {
        let o = office(1, CORP, STATION);
        assert!(!o.due(o.paid_until - 1));
        assert!(o.due(o.paid_until));
        let mut lapsed = o;
        lapsed.impounded_at = 1;
        assert!(!lapsed.due(o.paid_until + 1));
    }

    #[test]
    fn periods_are_counted_from_the_old_date_and_a_long_gap_bills_one() {
        let p = RENT_PERIOD_DAYS * DAY_TICKS;
        assert_eq!(next_paid_until(1_000, 1_000, p), 1_000 + p);
        // A rollover a day late does not move the cycle.
        assert_eq!(next_paid_until(1_000, 1_000 + DAY_TICKS, p), 1_000 + p);
        // Three periods down: one bill, paid up from now.
        let now = 1_000 + 3 * p + 5;
        assert_eq!(next_paid_until(1_000, now, p), now + p);
        // A zero period is not an infinite loop.
        assert_eq!(next_paid_until(1_000, 1_000, 0), 1_001);
    }

    #[test]
    fn the_office_type_is_a_station_category_singleton() {
        assert_eq!(OFFICE_TYPES.len(), 1);
        let t = &OFFICE_TYPES[0];
        assert_eq!(
            (t.type_id, t.category_id, t.singleton),
            (TYPE_OFFICE, 3, true)
        );
    }
}
