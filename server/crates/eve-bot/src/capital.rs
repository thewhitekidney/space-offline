//! **Bots fly capitals** (R-CAP packet CAP-L; lane note `docs/lanes/cap-l.md`).
//!
//! Two careers make a **capital pair**: a *capital pilot* (a dreadnought,
//! carrier, command carrier, force auxiliary or lancer it OWNS) and its
//! *cyno alt* (a ship with a cynosural field generator fitted). They are one
//! fleet of two, paired by [`crate::brain::Policy::partner`]:
//!
//! 1. the alt (the fleet's commander) forms the fleet with the partner,
//!    stocks ozone, undocks and flies the gates to the **target system**
//!    ([`choose_dest`]: a non-high-sec system inside the hull's jump range),
//!    lights the cyno ([`light`]) and holds it until the partner lands on its
//!    grid or [`HOLD`] runs out, then puts it out;
//! 2. the pilot, told of the beacon by `OnFleetJumpBeaconModuleChange`
//!    ([`Beacons`], folded in by the driver), checks range, fuel and the
//!    jump timers ([`cap_turn`]), stocks the fuel bay while docked
//!    ([`stock_bay`]), undocks and jumps ([`jump_to_beacon`], CAP-E);
//! 3. the next leg goes back the other way: the alt flies home and lights
//!    there, the pilot jumps home and docks.
//!
//! **Fuel logistics** are the pilot's own ([`stock_bay`]): at a station with
//! the hull's isotope (and the alt's ozone) on sale, it buys what the next
//! legs need plus [`FUEL_MARGIN`] and moves it into the fuel bay (flag 133).
//! **Fatigue** is respected two ways: the pilot jumps only when
//! `jumpTimers.GetTimers` says the activation timer is over ([`Timers`]),
//! and the alt starts no leg before the pair's projected fatigue
//! ([`Pace`], the server's own formulas) has fallen under
//! [`FATIGUE_CEILING_S`].
//!
//! **Nothing from nowhere** (owner rule): neither career is ever served by
//! GM kit. [`lift_blocks`] opens them for a bot only if it owns the hull (or
//! has the ISK for one on the market) and has a partner; the role setups give
//! nothing. Everything here is **[A]** policy except the client's call shapes
//! and the server's formulas, which are cited from CAP-D, CAP-E and CAP-F.

use crate::brain::{self, Act, Memory, Observation};
use crate::career::Signals;
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::travel::Map;
use crate::verbs::{Book, ItemRow};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_server::celestial::Centers;
use eve_server::jump_fatigue as fatigue;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

// ---- the client's and the server's constants ---------------------------------

/// `categoryShip` **[V]**.
pub const CATEGORY_SHIP: i64 = 6;
/// The hull groups a capital pilot flies: Dreadnought 485, Carrier 547,
/// Command Carrier 5120, Force Auxiliary 1538, Lancer Dreadnought 4594
/// **[V]** (`typedata::CAPITAL_GROUPS`, less the groups with no cyno jump:
/// Titans, Supercarriers and Capital Industrials are left to a later packet).
pub const JUMPER_GROUPS: [i64; 5] = [485, 547, 5120, 1538, 4594];
/// Freighter 513 and Jump Freighter 902 **[V]** (`typedata`'s note: industrial
/// class, not capital). The hauler flies the first by gate.
pub const FREIGHTER_GROUPS: [i64; 2] = [513, 902];
/// The ship cargo hold's flag **[V]**.
pub const CARGO: i64 = crate::verbs::FLAG_CARGO;
/// The ship fuel bay's flag **[V]** (`item::FLAG_FUEL_BAY`).
pub const FUEL_BAY: i64 = eve_server::item::FLAG_FUEL_BAY;
/// `jumpDriveConsumptionType`, `jumpDriveRange`, `jumpDriveConsumptionAmount`
/// **[V]** (`jump_drive.rs`).
pub const ATTR_FUEL_TYPE: i64 = eve_server::jump_drive::ATTR_FUEL_TYPE;
pub const ATTR_RANGE: i64 = eve_server::jump_drive::ATTR_RANGE;
pub const ATTR_PER_LY: i64 = eve_server::jump_drive::ATTR_FUEL_AMOUNT;
/// The cyno effect's id and name **[V]** (`cyno.rs`, the client's own).
pub const CYNO_EFFECT_ID: i64 = eve_server::cyno::EFFECT_CYNOSURAL_GENERATION;
pub const CYNO_EFFECT: &str = "cynosuralGeneration";
/// A cyno module's fuel type and quantity per cycle **[V-data]**.
pub const ATTR_OZONE_TYPE: i64 = eve_server::cyno::ATTR_FUEL_TYPE;
pub const ATTR_OZONE_QTY: i64 = eve_server::cyno::ATTR_FUEL_QUANTITY;
/// The push naming a fleet member's lit field **[V]** (CAP-D).
pub const BEACON_EVENT: &str = "OnFleetJumpBeaconModuleChange";
/// `repeat` of a module that runs until stopped **[V]** (the client's 1000).
pub const REPEAT: i64 = 1_000;

// ---- policy [A] ------------------------------------------------------------------

/// How long the alt keeps a field lit waiting for its partner **[A]**: one
/// cyno cycle (10 minutes at the plain module). Falsified by a pilot that lands
/// a minute too late (too short) or an alt that burns ozone for an idle hour
/// (too long).
pub const HOLD: Duration = Duration::from_secs(600);
/// The least security a cyno cannot be lit in: high sec **[A]** (CAP-D A1; the
/// client rounds 0.45 up to 0.5).
pub const HIGH_SEC: f64 = 0.45;
/// The range the alt assumes the pilot's drive has, ly **[A]**: it picks a
/// destination inside this times [`RANGE_USE`]; the pilot checks its own range
/// and refuses a field too far. Falsified by a pilot that always refuses.
pub const PAIR_RANGE_LY: f64 = 3.5;
/// The share of the range a destination may use **[A]**.
pub const RANGE_USE: f64 = 0.9;
/// The longest gate route the alt flies to its target **[A]**.
pub const ALT_MAX_JUMPS: u32 = 6;
/// The fuel a pilot keeps in the bay beyond the next legs' need: a quarter
/// **[A]**. Falsified by a pilot stopped by `JumpNotEnoughCharge`.
pub const FUEL_MARGIN: f64 = 0.25;
/// The legs of fuel the pilot stocks: out and back **[A]**.
pub const FUEL_LEGS: i64 = 2;
/// The share of the wallet a fuel purchase may take **[A]**.
pub const FUEL_WALLET_SHARE: f64 = 0.5;
/// The ozone cycles the alt stocks for a pair of legs **[A]**: one hold each.
pub const OZONE_CYCLES: i64 = 2;
/// The pair's fatigue ceiling, game seconds **[A]**: no leg starts while its
/// projection stands above it (an hour). Falsified by a pair that sits idle
/// for hours, or one that reaches the 5 h cap.
pub const FATIGUE_CEILING_S: f64 = 3_600.0;
/// The fatigue multiplier the alt assumes for the pilot's hull **[A]**: 1.0
/// (CAP-F's default; the true one is attribute 1971, which the alt cannot
/// read off a ship it does not fly).
pub const ASSUMED_MULTIPLIER: f64 = 1.0;
/// How often a pair member takes a turn **[A]**.
pub const TURN_EVERY: (Duration, Duration) = (Duration::from_secs(20), Duration::from_secs(45));
/// Hops between the pair's rests are left to [`Pace`]; this is the rest after
/// a finished round trip **[A]**: a quarter of a game day.
pub const REST: Duration = Duration::from_secs(6 * 60 * 60);

/// How often a pilot without a hull looks at the market for one **[A]**: a
/// pass reads a book per hull type. Falsified by a bot that never sees a
/// hull come on sale (too seldom) or `[live]` full of book reads (too often).
pub const SHOP_EVERY: Duration = Duration::from_secs(30 * 60);
/// A wallet under this does not read the price list for a hull **[A]**: no
/// capital or freighter is anywhere near (a hundred million ISK; the least
/// is several times that). Saves a call at every decision for almost every bot.
pub const SHOP_WALLET_FLOOR: f64 = 100_000_000.0;
/// The share of the wallet a hull may cost **[A]**: all but a cushion for
/// fuel and the broker.
pub const HULL_WALLET_SHARE: f64 = 0.9;

// ---- the beacon book --------------------------------------------------------------

/// One lit field, as the fleet is told of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Beacon {
    pub char_id: i64,
    pub system: i64,
    pub ball: i64,
    pub type_id: i64,
}

/// The fields the pilot's fleet holds, by the lighter's character id
/// (`fleetSvc.activeModuleBeacons`, CAP-D).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Beacons(BTreeMap<i64, Beacon>);

impl Beacons {
    /// **A push, folded in**: `OnFleetJumpBeaconModuleChange(charID,
    /// solarsystemID, beaconID, typeID, active)` **[V]**. Anything else is
    /// ignored.
    pub fn on_push(&mut self, event: &str, args: &[PyValue]) {
        if event != BEACON_EVENT {
            return;
        }
        let int = |i: usize| match args.get(i) {
            Some(PyValue::Int(n)) => Some(*n),
            _ => None,
        };
        let on = match args.get(4) {
            Some(PyValue::Bool(b)) => *b,
            Some(PyValue::Int(n)) => *n != 0,
            _ => return,
        };
        let (Some(char_id), Some(system), Some(ball), Some(type_id)) =
            (int(0), int(1), int(2), int(3))
        else {
            return;
        };
        if on {
            self.0.insert(
                char_id,
                Beacon {
                    char_id,
                    system,
                    ball,
                    type_id,
                },
            );
        } else {
            self.0.remove(&char_id);
        }
    }

    /// The field `char_id` has lit.
    pub fn of(&self, char_id: i64) -> Option<Beacon> {
        self.0.get(&char_id).copied()
    }

    /// Forget every field: a fleet left or disbanded.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

// ---- the pair's memory ---------------------------------------------------------------

/// What a pair member remembers across turns (the driver keeps it in
/// [`Memory::capital`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mind {
    pub beacons: Beacons,
    /// The system the pair started in: the capital's home.
    pub home: Option<i64>,
    /// The far system, chosen once ([`choose_dest`]).
    pub dest: Option<i64>,
    /// The pair's projected fatigue and when it was last set ([`Pace`]).
    pub pace: Pace,
    /// Legs flown, and refusals in a row (a pilot that cannot jump rests).
    pub legs: u32,
    pub refusals: u32,
    /// The ozone and fuel types read off the hull and module, once.
    pub fuel: Option<Fuel>,
    /// No hull shopping before this ([`SHOP_EVERY`]).
    pub shop_next: Option<Instant>,
    /// The station the pair started at, to dock at when home again.
    pub home_station: Option<i64>,
}

/// What the hull and the module burn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fuel {
    /// The isotope: its type, ly range and units per ly.
    pub isotope: i64,
    pub range_ly: f64,
    pub per_ly: f64,
}

// ---- pure: owning a hull ---------------------------------------------------------------

/// A ship the pilot owns in a hangar: assembled, in `groups`.
pub fn owned_hull<'a>(hangar: &'a [ItemRow], groups: &[i64]) -> Option<&'a ItemRow> {
    hangar
        .iter()
        .filter(|r| r.category_id == CATEGORY_SHIP && r.singleton && groups.contains(&r.group_id))
        .min_by_key(|r| r.item_id)
}

/// The facts a career's gate reads.
#[derive(Debug, Clone, Copy)]
pub struct Means<'a> {
    pub hangar: &'a [ItemRow],
    /// The ship the pilot sits in: its group, when known.
    pub active_group: Option<i64>,
    pub wallet: Option<f64>,
    /// The cheapest jumper and the cheapest freighter on the price list, ISK
    /// ([`cheapest_hull`]); `None`: not read (the wallet is too small to
    /// bother, [`SHOP_WALLET_FLOOR`]) or none listed.
    pub jumper_price: Option<f64>,
    pub freighter_price: Option<f64>,
    /// A partner is configured ([`crate::brain::Policy::partner`]).
    pub partner: bool,
}

impl Means<'_> {
    fn holds(&self, groups: &[i64]) -> bool {
        self.active_group.is_some_and(|g| groups.contains(&g))
            || owned_hull(self.hangar, groups).is_some()
    }

    /// Owns a jumper, or can buy one.
    pub fn can_fly_capital(&self) -> bool {
        self.holds(&JUMPER_GROUPS) || self.can_buy(self.jumper_price)
    }

    /// Owns a freighter, or can buy one.
    pub fn can_haul(&self) -> bool {
        self.holds(&FREIGHTER_GROUPS) || self.can_buy(self.freighter_price)
    }

    fn can_buy(&self, price: Option<f64>) -> bool {
        matches!((self.wallet, price), (Some(w), Some(p)) if w >= p / HULL_WALLET_SHARE)
    }
}

/// **The per-bot gate** (nothing from nowhere): `eve-bots` blocks the three
/// careers for everybody ([`crate::career::block_capital`]); this opens each
/// for a bot that can do it. The alt needs a partner and a ship of its own
/// (the alt's hull is a cheap one, so it is not priced: any ship in the
/// hangar will do, and the role asks for a fitted generator itself).
pub fn lift_blocks(s: &mut Signals, m: &Means<'_>) {
    if m.partner && m.can_fly_capital() {
        s.blocked.remove("capital_pilot");
    }
    if m.partner && owned_hull(m.hangar, &[]).is_none() && has_any_ship(m.hangar) {
        s.blocked.remove("cyno_alt");
    }
    if m.can_haul() {
        s.blocked.remove("freighter_hauler");
    }
}

fn has_any_ship(hangar: &[ItemRow]) -> bool {
    hangar
        .iter()
        .any(|r| r.category_id == CATEGORY_SHIP && r.singleton)
}

// ---- pure: fuel ---------------------------------------------------------------------------

/// What one jump of `ly` costs: the server's own rule, `max(long(ly x
/// perLY), 1)` **[V]**.
pub fn fuel_for(ly: f64, per_ly: f64) -> i64 {
    eve_server::jump_drive::fuel_required(ly, per_ly)
}

/// **How many units to buy**: the fuel `legs` jumps of `ly` take, plus the
/// margin, less what the bay and the hold already hold; never negative.
pub fn fuel_to_buy(have: i64, ly: f64, per_ly: f64, legs: i64) -> i64 {
    let need = fuel_for(ly, per_ly) as f64 * legs as f64 * (1.0 + FUEL_MARGIN);
    (need.ceil() as i64 - have).max(0)
}

/// What a purchase may spend: [`FUEL_WALLET_SHARE`] of the wallet.
pub fn fuel_budget(wallet: f64) -> f64 {
    wallet.max(0.0) * FUEL_WALLET_SHARE
}

/// **How many hours of income** a bot with `wallet` needs to afford `price`
/// at `isk_h` ISK an hour (the lane note's arithmetic); `None` when it earns
/// nothing, 0 when it already can.
pub fn hours_to_afford(wallet: f64, price: f64, isk_h: f64) -> Option<f64> {
    if wallet >= price {
        return Some(0.0);
    }
    (isk_h > 0.0).then(|| (price - wallet) / isk_h)
}

// ---- pure: the destination --------------------------------------------------------------

/// One system the alt could light in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub system: i64,
    /// Light years from home.
    pub ly: f64,
    pub security: f64,
    /// Gate jumps the alt flies from home (`None`: no route).
    pub gates: Option<u32>,
}

/// **The target system**: not high sec, inside [`RANGE_USE`] of `range_ly`,
/// reachable by at most [`ALT_MAX_JUMPS`] gates, and the one whose gate route
/// is **longest** (the jump is worth most where the walk is long); a tie goes
/// to the nearer light-year distance, then the lower id. `None` when no system
/// fits.
pub fn choose_dest(home: i64, cands: &[Candidate], range_ly: f64) -> Option<i64> {
    cands
        .iter()
        .filter(|c| c.system != home && c.security < HIGH_SEC)
        .filter(|c| c.ly > 0.0 && c.ly <= range_ly * RANGE_USE)
        .filter_map(|c| {
            c.gates
                .filter(|g| (1..=ALT_MAX_JUMPS).contains(g))
                .map(|g| (c, g))
        })
        .max_by(|(a, ga), (b, gb)| {
            ga.cmp(gb)
                .then(b.ly.total_cmp(&a.ly))
                .then(b.system.cmp(&a.system))
        })
        .map(|(c, _)| c.system)
}

/// The candidates round `home`: every system within `range_ly` by the
/// centres, with its security and the gate route from the map.
pub fn candidates(home: i64, centers: &Centers, map: &Map, range_ly: f64) -> Vec<Candidate> {
    centers
        .systems_within_ly(home, range_ly)
        .into_iter()
        .map(|(system, ly)| Candidate {
            system,
            ly,
            security: map.security(system).unwrap_or(1.0),
            gates: map.jumps(home, system, crate::travel::ANY_SECURITY),
        })
        .collect()
}

// ---- pure: fatigue --------------------------------------------------------------------------

/// **The pair's projected fatigue**, kept by the alt so it starts no leg the
/// pilot cannot answer: the server's own formulas ([`fatigue::fatigue_after`],
/// [`fatigue::activation_after`]) at [`ASSUMED_MULTIPLIER`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pace {
    /// Fatigue and activation remaining when set, game seconds.
    pub fatigue_s: f64,
    pub activation_s: f64,
    pub set_at: Option<Instant>,
}

impl Pace {
    /// Seconds left of each at `now`: they run down in real time.
    pub fn remaining(&self, now: Instant) -> (f64, f64) {
        let gone = self
            .set_at
            .map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f64());
        (
            (self.fatigue_s - gone).max(0.0),
            (self.activation_s - gone).max(0.0),
        )
    }

    /// The state after a jump of `ly` at `now`.
    pub fn after_jump(&self, now: Instant, ly: f64) -> Pace {
        let (f, a) = self.remaining(now);
        Pace {
            fatigue_s: fatigue::fatigue_after(f, ly, ASSUMED_MULTIPLIER),
            activation_s: fatigue::activation_after(a, ly, ASSUMED_MULTIPLIER),
            set_at: Some(now),
        }
    }

    /// **How long before a jump of `ly` is fine**: the activation timer, and
    /// until the fatigue the jump would leave is under [`FATIGUE_CEILING_S`].
    pub fn wait_for(&self, now: Instant, ly: f64) -> Duration {
        let (f, a) = self.remaining(now);
        let after = fatigue::fatigue_after(f, ly, ASSUMED_MULTIPLIER);
        let over = (after - FATIGUE_CEILING_S).max(0.0);
        Duration::from_secs_f64(a.max(over))
    }
}

/// `jumpTimers.GetTimers` as the pilot reads it: wall FILETIMEs
/// (`jump_fatigue::Timers::wire`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Timers {
    pub activation_until: i64,
    pub fatigue_until: i64,
}

impl Timers {
    /// The tuple `(activation, fatigue, lastUpdated)` **[V]**.
    pub fn from_value(v: &PyValue) -> Option<Timers> {
        let PyValue::Tuple(t) = v else {
            return None;
        };
        let int = |i: usize| match t.get(i) {
            Some(PyValue::Int(n)) => Some(*n),
            _ => None,
        };
        Some(Timers {
            activation_until: int(0)?,
            fatigue_until: int(1)?,
        })
    }

    /// The activation timer left at `now_ft` (100 ns ticks), as a wait.
    pub fn activation_wait(&self, now_ft: i64) -> Duration {
        let ticks = (self.activation_until - now_ft).max(0);
        Duration::from_secs_f64(ticks as f64 / 1.0e7)
    }
}

// ---- pure: the two turns ----------------------------------------------------------------------

/// What the pilot sees at a turn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CapFacts {
    pub docked: bool,
    pub system: i64,
    /// A field the partner has lit, if any.
    pub beacon: Option<Beacon>,
    /// Light years to it, when the centres know both systems.
    pub ly: Option<f64>,
    pub fuel: Option<Fuel>,
    /// Isotopes held in the bay and the hold.
    pub held: i64,
    /// The activation timer left.
    pub timer: Duration,
    /// The pilot's home system, once it has one.
    pub home: Option<i64>,
}

/// What the pilot does this turn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CapStep {
    /// Nothing can be done: why.
    Missing(&'static str),
    /// Docked: buy this many isotopes and bay them.
    Stock(i64),
    /// A beacon is lit but the jump is not yet possible: why.
    Wait(&'static str),
    /// Jump to the field.
    Jump(Beacon),
    /// At home with nothing lit: dock if in space, else rest.
    Settle,
}

/// **The pilot's turn** (pure). The order mirrors the server's refusals
/// ([`eve_server::jump_drive`]): a field to jump to, the system not the
/// pilot's own, the range, the timer, then the fuel. Stocking comes first
/// while docked with no field, so the bay is full before the call comes.
pub fn cap_turn(f: &CapFacts) -> CapStep {
    let Some(fuel) = f.fuel else {
        return CapStep::Missing("the hull's drive could not be read (no jump drive?)");
    };
    if f.docked && f.beacon.is_none() {
        let ly = f.ly.unwrap_or(fuel.range_ly);
        let buy = fuel_to_buy(f.held, ly, fuel.per_ly, FUEL_LEGS);
        if buy > 0 {
            return CapStep::Stock(buy);
        }
    }
    let Some(b) = f.beacon else {
        return if f.home == Some(f.system) {
            CapStep::Settle
        } else {
            CapStep::Wait("no field lit yet")
        };
    };
    if b.system == f.system {
        return CapStep::Wait("the field is in this system");
    }
    match f.ly {
        None => return CapStep::Missing("no distance to the field (no map of the stars)"),
        Some(ly) if ly > fuel.range_ly => return CapStep::Missing("the field is out of range"),
        Some(ly) if f.held < fuel_for(ly, fuel.per_ly) => {
            return CapStep::Missing("not enough isotopes for the jump");
        }
        Some(_) => {}
    }
    if !f.timer.is_zero() {
        return CapStep::Wait("the jump timer is still running");
    }
    CapStep::Jump(b)
}

/// What the alt sees at a turn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AltFacts {
    pub docked: bool,
    pub system: i64,
    /// The partner is in the alt's fleet.
    pub partnered: bool,
    /// The alt has a cyno module fitted.
    pub has_cyno: bool,
    /// The alt's own field is lit.
    pub lit: bool,
    pub ozone_held: i64,
    pub ozone_needed: i64,
    /// Where this leg lands, once chosen.
    pub target: Option<i64>,
    /// How long before the pair may jump again.
    pub pace_wait: Duration,
}

/// What the alt does this turn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AltStep {
    Missing(&'static str),
    /// Form the fleet and invite the partner.
    Form,
    /// Buy this much ozone and keep it in the hold.
    Stock(i64),
    /// Too soon after the last hop: rest this long.
    Rest(Duration),
    /// Fly to the target and light.
    Leg(i64),
    /// Already at the target with a field lit: hold, then douse.
    Hold,
}

/// **The alt's turn** (pure).
pub fn alt_turn(f: &AltFacts) -> AltStep {
    if !f.has_cyno {
        return AltStep::Missing("a ship with a cynosural field generator it owns");
    }
    if !f.partnered {
        return AltStep::Form;
    }
    if f.lit {
        return AltStep::Hold;
    }
    if f.docked && f.ozone_held < f.ozone_needed {
        return AltStep::Stock(f.ozone_needed - f.ozone_held);
    }
    if !f.pace_wait.is_zero() {
        return AltStep::Rest(f.pace_wait);
    }
    match f.target {
        None => AltStep::Missing("a target system inside the jump range"),
        Some(t) if t == f.system => AltStep::Leg(t),
        Some(t) => AltStep::Leg(t),
    }
}

/// The leg's target from where the capital is: away from home, then home.
pub fn leg_target(at: i64, home: i64, dest: i64) -> i64 {
    if at == home { dest } else { home }
}

// ---- the roles ------------------------------------------------------------------------------------

/// The `--loop capital` row: the pilot of a capital pair.
pub static ROLE: RoleSpec = RoleSpec {
    name: "capital",
    greeting: "o7 -- capital on your cyno",
    civilian: false,
    docked,
    in_space,
    banner,
    setup,
};

/// The `--loop cyno-alt` row: the pair's alt.
pub static ALT_ROLE: RoleSpec = RoleSpec {
    name: "cyno-alt",
    greeting: "o7 -- lighting for the capital",
    civilian: true,
    docked,
    in_space,
    banner,
    setup,
};

/// Whether `role` is a pair's alt.
pub fn is_alt(role: &RoleSpec) -> bool {
    std::ptr::eq(role, &ALT_ROLE)
}

/// Either side, docked or in space: one [`Act::Capital`] when its time has
/// come, then a wait. The decisions are [`cap_turn`] and [`alt_turn`], which
/// need the connection, so the act reads the facts itself.
fn turn_act(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if o.failures.values().any(|&n| n >= brain::GIVE_UP_AFTER) {
        return vec![Act::Reconnect];
    }
    if m.policy.partner.is_none() {
        return vec![
            Act::Missing(MISSING_PARTNER),
            Act::Wait(brain::MISSING_WAIT),
        ];
    }
    m.homing = None;
    if let Some(next) = m.market_next
        && o.now < next
    {
        return vec![Act::Wait(next - o.now)];
    }
    let next = o.now + m.jitter(TURN_EVERY);
    m.market_next = Some(next);
    vec![Act::Capital, Act::Wait(next - o.now)]
}

fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    turn_act(m, o)
}

fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if o.park.is_none() {
        return vec![Act::Wait(brain::SHORT_WAIT)];
    }
    turn_act(m, o)
}

/// What a pair member lacks without a partner.
pub const MISSING_PARTNER: &str = "a partner character (the other half of the capital pair)";

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, reconnects {}",
        s.name, s.phase, s.reconnects
    )
}

/// **Nothing from nowhere**: no slash line, whatever is lacking.
fn setup(_: Lacks) -> Vec<String> {
    Vec::new()
}

// ---- the verbs --------------------------------------------------------------------------------------

/// What the executor is given.
#[derive(Debug, Clone, Copy)]
pub struct Ctx<'a> {
    pub map: Option<&'a Map>,
    pub floor: f64,
    pub content: Option<&'a eve_server::content::Content>,
    pub alt: bool,
}

fn settle(c: &mut Conn, what: &str, mut done: impl FnMut(&mut Conn) -> bool) -> Result<()> {
    let deadline = Instant::now() + crate::travel::TRAVEL_CEILING;
    while !done(c) {
        if Instant::now() >= deadline {
            return Err(Error::Refused(format!(
                "{what}: not over after the ceiling"
            )));
        }
        c.pump(Duration::from_millis(250))?;
    }
    Ok(())
}

/// Bind the park after an undock or a jump and wait for its first state.
pub fn bind(c: &mut Conn) -> Result<()> {
    if c.ballpark().is_none() {
        let system = c
            .session()
            .solar_system_id()
            .ok_or_else(|| Error::NotSent("bind: not in space".to_owned()))?;
        c.bind_ballpark(system)?;
    }
    settle(c, "bind", |c| c.world().has_state())
}

/// The units of `type_id` held at `flag` of the active ship.
pub fn held_at(c: &mut Conn, ship: i64, flag: i64, type_id: i64) -> Result<i64> {
    Ok(c.ship_items(ship, Some(flag))?
        .iter()
        .filter(|r| r.type_id == type_id)
        .map(|r| r.quantity)
        .sum())
}

/// The drive of hull type `hull` from the type table: the isotope, the range
/// and the burn per light year, **unmodified by skills** **[A]** (the ship's
/// `ItemGetInfo` serves only the HUD's attributes, so the modified 867 and 868
/// are not on the wire; Jump Drive Calibration only lengthens the range and
/// Jump Fuel Conservation only cuts the burn, so the table's numbers are the
/// safe side of both). Falsified by a pilot that stocks more than it burns.
pub fn drive_of(content: &eve_server::content::Content, hull: i64) -> Option<Fuel> {
    let a = |id| eve_server::typedata::attribute_in(content, hull, id);
    match (a(ATTR_FUEL_TYPE), a(ATTR_RANGE), a(ATTR_PER_LY)) {
        (Some(t), Some(r), Some(p)) if t > 0.0 && r > 0.0 => Some(Fuel {
            isotope: t.round() as i64,
            range_ly: r,
            per_ly: p,
        }),
        _ => None,
    }
}

/// **Read the drive** of the active ship: the type table's ([`drive_of`]) for
/// the hull the docked hangar lists it as; else the ship's own `ItemGetInfo`
/// (a server that serves the jump attributes there).
pub fn read_fuel(
    c: &mut Conn,
    content: Option<&eve_server::content::Content>,
    ship: i64,
) -> Result<Option<Fuel>> {
    if let Some(content) = content
        && c.session().station_id().is_some()
        && let Some(hull) = c
            .hangar_items()?
            .into_iter()
            .find(|r| r.item_id == ship)
            .map(|r| r.type_id)
        && let Some(f) = drive_of(content, hull)
    {
        return Ok(Some(f));
    }
    let isotope = c.ship_attribute(ship, ATTR_FUEL_TYPE)?;
    let range = c.ship_attribute(ship, ATTR_RANGE)?;
    let per_ly = c.ship_attribute(ship, ATTR_PER_LY)?;
    Ok(match (isotope, range, per_ly) {
        (Some(t), Some(r), Some(p)) if t > 0.0 && r > 0.0 => Some(Fuel {
            isotope: t.round() as i64,
            range_ly: r,
            per_ly: p,
        }),
        _ => None,
    })
}

/// `jumpTimers.GetTimers` **[V]** (CAP-F).
pub fn read_timers(c: &mut Conn) -> Result<Timers> {
    let out = c.call("jumpTimers", "GetTimers", vec![])?;
    out.value()
        .as_ref()
        .and_then(Timers::from_value)
        .ok_or_else(|| Error::Protocol("jumpTimers.GetTimers did not answer a 3-tuple".to_owned()))
}

/// **Buy `units` of `type_id` at the docked station** (asks cheapest first,
/// inside `budget`) and return what was bought. A refusal ends the buying.
pub fn buy_units(c: &mut Conn, book: &Book, type_id: i64, units: i64, budget: f64) -> Result<i64> {
    let station = c
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("buy fuel: not docked".to_owned()))?;
    let plan = crate::acquire::plan(book, station, None, budget, units);
    let mut bought = 0;
    for (ask, k) in plan {
        match c.buy(
            station,
            type_id,
            ask.price,
            k,
            0,
            crate::pricing::BROKER_FEE,
        ) {
            Ok(row) => bought += k - row.vol_remaining.clamp(0, k),
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => break,
        }
    }
    Ok(bought)
}

/// **Move hangar stacks of `type_id` into a ship's slot**: the ship
/// inventory's `Add(itemID, stationID, qty, flag)` (the drag onto a hold,
/// `invControllers.py:267`) **[V]**, with `flag` the fuel bay's 133 (CAP-B) or
/// the cargo's. Returns the units moved.
pub fn load_into(c: &mut Conn, ship: i64, type_id: i64, flag: i64, max: i64) -> Result<i64> {
    let station = c
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("load: not docked".to_owned()))?;
    let inventory = c.open_ship(ship)?;
    let mut left = max;
    let mut moved = 0;
    for row in c.hangar_items()? {
        if left <= 0 {
            break;
        }
        if row.type_id != type_id || row.singleton || row.quantity <= 0 {
            continue;
        }
        let n = row.quantity.min(left);
        let out = c.call_on(
            &inventory,
            "Add",
            vec![PyValue::Int(row.item_id), PyValue::Int(station)],
            vec![
                (PyValue::Int(n), eve_server::pyutil::bstr("qty")),
                (PyValue::Int(flag), eve_server::pyutil::bstr("flag")),
            ],
        )?;
        if out.refusal().is_some() {
            break;
        }
        left -= n;
        moved += n;
    }
    Ok(moved)
}

/// **Stock the fuel bay** of the docked ship with `units` of `type_id`: the
/// hangar's own first, then bought at this station's asks inside
/// [`fuel_budget`]; the bay takes it, the hold the rest. Returns `(bought,
/// bayed, held)`.
pub fn stock_bay(c: &mut Conn, ship: i64, type_id: i64, units: i64) -> Result<(i64, i64, i64)> {
    let in_hangar: i64 = c
        .hangar_items()?
        .iter()
        .filter(|r| r.type_id == type_id && !r.singleton)
        .map(|r| r.quantity)
        .sum();
    let mut bought = 0;
    if in_hangar < units {
        let wallet = c.cash_balance()?;
        let book = c.market_orders(type_id)?;
        bought = buy_units(c, &book, type_id, units - in_hangar, fuel_budget(wallet))?;
    }
    let bayed = load_into(c, ship, type_id, FUEL_BAY, units)?;
    let held = if bayed < units {
        load_into(c, ship, type_id, CARGO, units - bayed)?
    } else {
        0
    };
    Ok((bought, bayed, held))
}

/// **The fitted cyno module** of the active ship, by the type's effects
/// (`typedata`'s `has_effect`, effect 2857 **[V]**).
pub fn cyno_module(
    c: &mut Conn,
    content: &eve_server::content::Content,
) -> Result<Option<ItemRow>> {
    let table = eve_server::typedata::installed_in(content).table();
    Ok(c.fitted()?.into_iter().find(|r| {
        table
            .get(r.type_id)
            .is_some_and(|t| t.has_effect(CYNO_EFFECT_ID))
    }))
}

/// **Light the cyno**: `dogmaIM.Activate(module, 'cynosuralGeneration',
/// None, repeat)` **[V]** (CAP-D). Evidence: `OnGodmaShipEffect` start.
pub fn light(c: &mut Conn, module: i64) -> Result<()> {
    c.activate(module, (CYNO_EFFECT, CYNO_EFFECT_ID), None, REPEAT)
        .map(|_| ())
}

/// **Put the cyno out**: `dogmaIM.Deactivate` (the module stops at its cycle's
/// end, CAP-D).
pub fn douse(c: &mut Conn, module: i64) -> Result<()> {
    c.deactivate(module, CYNO_EFFECT).map(|_| ())
}

/// **Jump to a fleet member's field** (CAP-E) **[V]**: bound
/// `beyonce.CmdJumpToFleetModuleBeacon(charID, beaconID, solarsystemID)`,
/// answer discarded; the evidence is the `solarsystemid` session change. A
/// refusal is the server's key (`JumpTooFar`, `JumpNotEnoughCharge`,
/// `JumpActivationTimer`, ...).
pub fn jump_to_beacon(c: &mut Conn, b: &Beacon) -> Result<()> {
    let from = c.session().solar_system_id();
    let mut out = c.bound_call(
        "CmdJumpToFleetModuleBeacon",
        vec![
            PyValue::Int(b.char_id),
            PyValue::Int(b.ball),
            PyValue::Int(b.system),
        ],
    )?;
    if let Some(key) = out.refusal() {
        return Err(Error::Refused(format!(
            "CmdJumpToFleetModuleBeacon({}, {}, {}): {key}",
            b.char_id, b.ball, b.system
        )));
    }
    c.await_session_change(
        &mut out,
        eve_server::jump::JUMP_DELAY + Duration::from_secs(6),
    )?;
    let now = c.session().solar_system_id();
    if out.session_changed && now.is_some() && now != from {
        return bind(c);
    }
    Err(Error::Refused(format!(
        "CmdJumpToFleetModuleBeacon({}): no solarsystemid session change",
        b.ball
    )))
}

/// Whether `char_id`'s ship is in this park.
pub fn on_grid(c: &Conn, char_id: i64) -> bool {
    c.world().ship_of(char_id).is_some()
}

/// The light years between two systems, when the content has centres.
pub fn ly_between(content: Option<&eve_server::content::Content>, a: i64, b: i64) -> Option<f64> {
    content?.try_get::<Centers>()?.ly(a, b)
}

// ---- owning the hull: boarding, and buying it on the market --------------------------------

/// The hull types in `content` whose group is in `groups` (ships only).
pub fn hull_types(content: &eve_server::content::Content, groups: &[i64]) -> Vec<i64> {
    eve_server::typedata::installed_in(content)
        .table()
        .types()
        .filter(|t| t.category_id == CATEGORY_SHIP && groups.contains(&t.group_id))
        .map(|t| t.type_id)
        .collect()
}

/// **The cheapest hull of `groups` on the price list**, ISK: the gate's price
/// of "can I buy one" (the server's adjusted prices, [`Conn::adjusted_prices`]).
pub fn cheapest_hull(
    content: &eve_server::content::Content,
    groups: &[i64],
    prices: &[(i64, f64)],
) -> Option<f64> {
    let types = hull_types(content, groups);
    prices
        .iter()
        .filter(|(t, p)| *p > 0.0 && types.contains(t))
        .map(|&(_, p)| p)
        .min_by(f64::total_cmp)
}

/// **Get into a hull of `groups` it owns**: the one it sits in, else one in
/// the hangar (boarded), else one bought at this station's asks inside
/// [`HULL_WALLET_SHARE`] of the wallet (once per [`SHOP_EVERY`]). **Never GM**
/// (nothing from nowhere). Returns what was done, or why not.
pub fn get_hull(
    c: &mut Conn,
    content: &eve_server::content::Content,
    groups: &[i64],
    shop_next: &mut Option<Instant>,
) -> Result<std::result::Result<String, String>> {
    let rows = c.hangar_items()?;
    let active = c.session().ship_id();
    if rows
        .iter()
        .any(|r| Some(r.item_id) == active && groups.contains(&r.group_id))
    {
        return Ok(Ok("flying one".to_owned()));
    }
    // SKILLGATE: only a hull the pilot's skills board; the lack of an owned
    // one is queued ahead of the rest. A refused read filters nothing.
    let levels = match crate::acquire::skill_levels(c) {
        Ok(l) => Some(l),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(_) => None,
    };
    let flyable = |t: i64| {
        levels
            .as_ref()
            .is_none_or(|l| crate::acquire::flyable(l, t))
    };
    let ready: Vec<ItemRow> = rows
        .iter()
        .filter(|r| flyable(r.type_id))
        .copied()
        .collect();
    if let Some(r) = owned_hull(&ready, groups) {
        c.board(r.item_id)?;
        return Ok(Ok(format!("boarded {}", r.item_id)));
    }
    if let (Some(r), Some(l)) = (owned_hull(&rows, groups), levels.as_ref()) {
        let lack = crate::acquire::lacking_of(l, r.type_id);
        let missing = crate::acquire::learn_urgent_soft(c, &lack)?;
        return Ok(Err(format!(
            "owns hull {} but lacks skills {lack:?}{}",
            r.type_id,
            crate::acquire::missing_note(&missing)
        )));
    }
    let now = Instant::now();
    if shop_next.is_some_and(|t| now < t) {
        return Ok(Err("owns none, not shopping yet".to_owned()));
    }
    *shop_next = Some(now + SHOP_EVERY);
    let station = c
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("shop: not docked".to_owned()))?;
    let wallet = c.cash_balance()?;
    let budget = wallet * HULL_WALLET_SHARE;
    // The cheapest hull on sale the pilot may fly; failing that the cheapest
    // on sale at all, whose lack is queued and which is not bought yet.
    let mut best: Option<(i64, f64)> = None;
    let mut best_any: Option<(i64, f64)> = None;
    for t in hull_types(content, groups) {
        let book = c.market_orders(t)?;
        if let Some(a) = book
            .asks
            .iter()
            .filter(|a| !a.bid && a.station_id == station && a.vol_remaining > 0)
            .min_by(|a, b| a.price.total_cmp(&b.price))
            && a.price <= budget
        {
            if best_any.is_none_or(|(_, p)| a.price < p) {
                best_any = Some((t, a.price));
            }
            if flyable(t) && best.is_none_or(|(_, p)| a.price < p) {
                best = Some((t, a.price));
            }
        }
    }
    let Some((t, price)) = best.or(best_any) else {
        return Ok(Err(format!(
            "owns none and none is on sale here inside {budget:.0} ISK"
        )));
    };
    if !flyable(t) {
        let lack = levels
            .as_ref()
            .map(|l| crate::acquire::lacking_of(l, t))
            .unwrap_or_default();
        let missing = crate::acquire::learn_urgent_soft(c, &lack)?;
        return Ok(Err(format!(
            "hull {t} is on sale but the pilot lacks skills {lack:?}{}",
            crate::acquire::missing_note(&missing)
        )));
    }
    match c.buy(station, t, price, 1, 0, crate::pricing::BROKER_FEE) {
        Ok(_) => {}
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(e) => return Ok(Err(format!("buying hull {t}: {e}"))),
    }
    let bought = c
        .hangar_items()?
        .into_iter()
        .find(|r| r.type_id == t && r.singleton);
    match bought {
        Some(r) => {
            c.board(r.item_id)?;
            Ok(Ok(format!(
                "bought hull {t} for {price:.0} ISK and boarded it"
            )))
        }
        None => Ok(Err(format!("bought hull {t} but it is not in the hangar"))),
    }
}

// ---- the executor -------------------------------------------------------------------------------------

/// **One turn of a pair member**, over the connection: read the facts, take
/// [`cap_turn`] or [`alt_turn`], do the step. Returns the act's note. A step
/// that cannot be done is a note; only a dead connection (or a trip that
/// cannot be flown) is the error.
pub fn turn(c: &mut Conn, m: &mut Memory, ctx: &Ctx<'_>) -> Result<String> {
    if ctx.alt {
        alt(c, m, ctx)
    } else {
        pilot(c, m, ctx)
    }
}

fn pilot(c: &mut Conn, m: &mut Memory, ctx: &Ctx<'_>) -> Result<String> {
    if c.session().station_id().is_some()
        && let Some(content) = ctx.content
    {
        match get_hull(c, content, &JUMPER_GROUPS, &mut m.capital.shop_next)? {
            Ok(how) if how != "flying one" => return Ok(format!("capital: {how}")),
            Ok(_) => {}
            Err(why) => return Ok(format!("capital: {why}")),
        }
    }
    let ship = c
        .session()
        .ship_id()
        .ok_or_else(|| Error::NotSent("capital: no ship".to_owned()))?;
    let system = c
        .session()
        .solar_system_id2()
        .ok_or_else(|| Error::NotSent("capital: no system".to_owned()))?;
    let docked = c.session().station_id().is_some();
    if m.capital.home.is_none() {
        m.capital.home = Some(system);
    }
    if m.capital.home_station.is_none() {
        m.capital.home_station = c.session().station_id();
    }
    if m.capital.fuel.is_none() {
        m.capital.fuel = read_fuel(c, ctx.content, ship)?;
    }
    let fuel = m.capital.fuel;
    let partner = m.policy.partner.unwrap_or(0);
    let beacon = m.capital.beacons.of(partner);
    let held = match fuel {
        Some(f) => held_at(c, ship, FUEL_BAY, f.isotope)? + held_at(c, ship, CARGO, f.isotope)?,
        None => 0,
    };
    let timer = if beacon.is_some() {
        read_timers(c)?.activation_wait(eve_server::pyutil::filetime_now())
    } else {
        Duration::ZERO
    };
    let ly = beacon.and_then(|b| ly_between(ctx.content, system, b.system));
    let facts = CapFacts {
        docked,
        system,
        beacon,
        ly,
        fuel,
        held,
        timer,
        home: m.capital.home,
    };
    match cap_turn(&facts) {
        CapStep::Missing(why) => {
            m.capital.refusals += 1;
            Ok(format!("capital: missing {why}"))
        }
        CapStep::Wait(why) => Ok(format!("capital: waiting, {why}")),
        CapStep::Settle => {
            if let Some(home) = c.session().station_id() {
                return Ok(format!("capital: docked at {home}"));
            }
            Ok(dock_home(c, m))
        }
        CapStep::Stock(units) => {
            let Some(f) = fuel else {
                return Ok("capital: no fuel type".to_owned());
            };
            let (bought, bayed, hold) = stock_bay(c, ship, f.isotope, units)?;
            Ok(format!(
                "capital: stocked {units} isotopes: bought {bought}, bay {bayed}, hold {hold}"
            ))
        }
        CapStep::Jump(b) => {
            if docked {
                c.undock_repaired()?;
            }
            bind(c)?;
            match jump_to_beacon(c, &b) {
                Ok(()) => {
                    m.capital.legs += 1;
                    m.capital.refusals = 0;
                    Ok(format!(
                        "capital: jumped to {} ({} legs)",
                        b.system, m.capital.legs
                    ))
                }
                Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
                Err(e) => {
                    m.capital.refusals += 1;
                    Ok(format!("capital: jump refused: {e}"))
                }
            }
        }
    }
}

/// Dock at the pair's home station, from space in the home system. A refusal
/// (CAP-K's policy admits a capital only at a null-sec station) is a note.
fn dock_home(c: &mut Conn, m: &Memory) -> String {
    let Some(station) = m.capital.home_station else {
        return "capital: at home, in space (no home station)".to_owned();
    };
    match bind(c).and_then(|()| crate::probe::dock_at(c, station)) {
        Ok(()) => format!("capital: docked at {station}"),
        Err(e) => format!("capital: at home, could not dock at {station}: {e}"),
    }
}

fn alt(c: &mut Conn, m: &mut Memory, ctx: &Ctx<'_>) -> Result<String> {
    let Some(content) = ctx.content else {
        return Ok("cyno alt: no type table (eve-bots --content)".to_owned());
    };
    let ship = c
        .session()
        .ship_id()
        .ok_or_else(|| Error::NotSent("cyno alt: no ship".to_owned()))?;
    let system = c
        .session()
        .solar_system_id2()
        .ok_or_else(|| Error::NotSent("cyno alt: no system".to_owned()))?;
    let docked = c.session().station_id().is_some();
    let partner = m.policy.partner.unwrap_or(0);
    if m.capital.home.is_none() {
        m.capital.home = Some(system);
    }
    if m.capital.home_station.is_none() {
        m.capital.home_station = c.session().station_id();
    }
    let home = m.capital.home.unwrap_or(system);
    let module = cyno_module(c, content)?;
    let ozone = module.and_then(|r| {
        let t = eve_server::typedata::attribute_in(content, r.type_id, ATTR_OZONE_TYPE)?;
        let q = eve_server::typedata::attribute_in(content, r.type_id, ATTR_OZONE_QTY)?;
        Some((t.round() as i64, q.round() as i64))
    });
    let (ozone_type, per_cycle) = ozone.unwrap_or((0, 0));
    let ozone_held = if ozone_type > 0 {
        held_at(c, ship, FUEL_BAY, ozone_type)? + held_at(c, ship, CARGO, ozone_type)?
    } else {
        0
    };
    if m.capital.dest.is_none()
        && let (Some(centers), Some(map)) = (content.try_get::<Centers>(), ctx.map)
    {
        let cands = candidates(home, centers, map, PAIR_RANGE_LY);
        m.capital.dest = choose_dest(home, &cands, PAIR_RANGE_LY);
    }
    let target = m.capital.dest.map(|d| leg_target(system, home, d));
    let hop_ly = target
        .and_then(|t| ly_between(ctx.content, system, t))
        .unwrap_or(0.0);
    let facts = AltFacts {
        docked,
        system,
        partnered: m.fleet.joined.contains(&partner),
        has_cyno: module.is_some(),
        lit: false,
        ozone_held,
        ozone_needed: per_cycle * OZONE_CYCLES,
        target,
        pace_wait: m.capital.pace.wait_for(Instant::now(), hop_ly),
    };
    match alt_turn(&facts) {
        AltStep::Missing(why) => Ok(format!("cyno alt: missing {why}")),
        AltStep::Rest(d) => Ok(format!(
            "cyno alt: resting {} s for the pair's timers",
            d.as_secs()
        )),
        AltStep::Hold => Ok("cyno alt: holding".to_owned()),
        AltStep::Form => {
            let orders = m.fleet.form_fleet(&[partner]);
            let mut done = 0;
            for order in orders {
                let result = crate::fleet::execute(c, &m.fleet, &order);
                m.fleet.on_done(&order, &result);
                match result {
                    Ok(_) => done += 1,
                    Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                    Err(e) => return Ok(format!("cyno alt: fleet: {e}")),
                }
            }
            Ok(format!(
                "cyno alt: fleet orders {done}, waiting for {partner}"
            ))
        }
        AltStep::Stock(units) => {
            let (bought, bayed, hold) = stock_bay(c, ship, ozone_type, units)?;
            Ok(format!(
                "cyno alt: stocked {units} ozone: bought {bought}, bay {bayed}, hold {hold}"
            ))
        }
        AltStep::Leg(target) => {
            let Some(map) = ctx.map else {
                return Ok("cyno alt: no map to fly".to_owned());
            };
            let Some(module) = module else {
                return Ok("cyno alt: no module".to_owned());
            };
            if docked {
                // A failed repair is a note, never a stuck undock.
                if let Ok(note) = c.repair_ship()
                    && !note.is_empty()
                {
                    eprintln!("bot undock: {note}");
                }
                c.undock_with(&[(module.flag_id, module.item_id)])?;
            }
            bind(c)?;
            let entered = if system == target {
                Vec::new()
            } else {
                crate::probe::jump_route(c, map, ctx.floor, target)?
            };
            light(c, module.item_id)?;
            let until = Instant::now() + HOLD;
            let met = {
                let mut met = false;
                while Instant::now() < until {
                    if on_grid(c, partner) {
                        met = true;
                        break;
                    }
                    c.pump(Duration::from_millis(500))?;
                }
                met
            };
            let _ = douse(c, module.item_id);
            let now = Instant::now();
            m.capital.pace = m.capital.pace.after_jump(now, hop_ly);
            m.capital.legs += 1;
            // Home again: dock, and the next leg waits on the pair's pace.
            let docked_note = if target == home {
                format!("; {}", dock_home(c, m).replace("capital:", "alt:"))
            } else {
                String::new()
            };
            Ok(format!(
                "cyno alt: leg to {target} over {} gates, partner {}{docked_note}",
                entered.len(),
                if met { "landed" } else { "did not come" }
            ))
        }
    }
}

#[cfg(test)]
mod tests;
