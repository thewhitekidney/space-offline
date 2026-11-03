//! The Sovereignty Hub in space: the launch, the ball, the GM capture and
//! the stand-in fuel (SOC-D2; lane note `docs/lanes/sov-d2.md`, spec
//! R-SOV §1.2, §5.1, §6.2, §6.7, §9 D2).
//!
//! **What a hub is here.** One item of [`sov::TYPE_INFRASTRUCTURE_HUB`],
//! moved out of a pilot's cargo by "Launch for Corporation" (`ship.Drop`,
//! [`launch`]) and owned from then on by [`sov::NEUTRAL_HUB_OWNER`], the
//! type's `deploy.defaultOwner` **[V-data]**. It keeps its item id **[I]**
//! (R-SOV §6.2 step 3), sits in the item store at its solar system
//! ([`FLAG_IN_SPACE`]) and is a ball in **every** park of that system: the
//! type is `physics.isAlwaysGlobal` **[V-data]**, so it is built by
//! `destiny::scene` beside the stations and gates ([`balls_in`]) and never
//! enters `grid::local_balls`.
//!
//! **Who holds it is D1's ledger, not the item.** The `sov` row names the
//! hub id, its state and its holder; this module adds one row of its own
//! per hub, kind [`KIND`] (`delta_kinds::KINDS`):
//!
//! ```text
//! sovhub <systemID> -> hubID x y z launcherID launchTime offlineDays
//! ```
//!
//! `x y z` is where the ball is (fixed at launch, so a later catalogue
//! cannot move it), `launcherID` the pilot or `-` for a GM hub, and
//! `offlineDays` the rollovers the hub has spent without a day's fuel
//! ([`on_rollover`]). The item row stays owned by the neutral owner for the
//! hub's whole life **[I]**: every reader of "who owns the hub" -- the
//! slim's `ownerID`, the structure rows, Show Info's owner button through
//! the slim (`infoContentBase.GetOwnerIDToShow`) **[V]** -- reads the claim.
//!
//! **A neutral hub becomes a claim by entosis** (D3a). Until D3a lands,
//! `/sov capture` ([`capture`]) makes the claim the way D3a's completion
//! will: the alliance's executor corp, `claim_time = now`, D1's three
//! pushes, a `sovact` row, and `OnSlimItemChange` on the hub.
//!
//! **Stand-in fuel** (owner decision S-2, R-SOV §6.7). The Equinox fuel
//! window is gateway-only **[V]** and no menu or drag in this client puts
//! an item into a hub: no bracket or overview row has a drop handler
//! (`inSpaceBracket`, `bracket`) and the only hub menus open gateway
//! windows (`celestialCheckers.OfferOpenSovFuelDepositWindow`) **[V]**. So
//! a hub's fuel bay is real items -- fuel-block stacks ([`GROUP_FUEL_BLOCK`]
//! **[V]**) located in the hub at [`FLAG_HUB_FUEL`] -- filled from the
//! pilot's own hold by a console line ([`refuel`]): `/sovfuel` for a GM
//! session and `/boostsov` for anyone, which the client sends to
//! `publicQaToolsServer` with no role check (`publicQaToolsClient
//! .allowedSlashCommands` **[V]**). Each claimed hub burns [`FUEL_PER_DAY`]
//! blocks a game day at the rollover, so `--time-scale N` burns N times as
//! fast in wall time (lane CLOCK); the blocks leave the economy as the
//! ledger's destroyed goods. Out of fuel, the hub goes `offline`; after
//! [`GRACE_DAYS`] offline days the claim lapses and the hub is gone.

use crate::delta::Deltas;
use crate::destiny::{Ball, FLAGS_STATIC_BALL, Mode, NO_FORMATION, Slim};
use crate::item::{Edit, Item, Slot};
use crate::notify::Push;
use crate::pyutil::bstr;
use crate::service::Registry;
use crate::sov::{self, Claim, HubState};
use eve_marshal::PyValue;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "sovhub";

/// `inventorycommon.const.groupInfrastructureHub` **[V]**.
pub const GROUP_INFRASTRUCTURE_HUB: i64 = 1_012;
/// `inventorycommon.const.categorySovereigntyStructure` **[V]**: what
/// `itemCheckers.OfferLaunchForCorp` tests (`IsSovereigntyStructure`).
pub const CATEGORY_SOVEREIGNTY_STRUCTURE: i64 = 40;
/// `inventorycommon.const.groupFuelBlock` **[V]**: every fuel block, the
/// four racial ones counted alike (R-SOV §6.7).
pub const GROUP_FUEL_BLOCK: i64 = 1_136;

/// Where a launched hub is in the store: its solar system, flag 0, as a
/// wreck or a launched drone is (`combat::FLAG_IN_SPACE`).
pub const FLAG_IN_SPACE: i64 = crate::combat::FLAG_IN_SPACE;
/// `invConst.flagSpecializedFuelBay` **[V]**: the flag of the hub's fuel
/// stacks. No client window lists them (the module header), so the flag
/// only names the bay for `grep` and for a later real one **[I]**.
pub const FLAG_HUB_FUEL: i64 = 133;

/// `invConst.flagCargo`, `flagFleetHangar` and `flagColonyResourcesHold`
/// **[V]**: `OfferLaunchForCorp` offers the launch from the cargo hold, the
/// fleet hangar and the infrastructure hangar (`IsInInfrastructureHangar`
/// tests `flagColonyResourcesHold`) **[V]**.
pub const FLAG_CARGO: i64 = 5;
pub const FLAG_FLEET_HANGAR: i64 = 155;
pub const FLAG_INFRASTRUCTURE_HANGAR: i64 = 185;
/// Where a launch may take the hub from.
pub const LAUNCH_FLAGS: &[i64] = &[FLAG_CARGO, FLAG_FLEET_HANGAR, FLAG_INFRASTRUCTURE_HANGAR];
/// Where `/sovfuel` takes blocks from: the launch's holds and the ship's
/// own fuel bay **[I]**, where a pilot who carries fuel keeps it.
pub const FUEL_SOURCE_FLAGS: &[i64] = &[
    FLAG_CARGO,
    FLAG_FLEET_HANGAR,
    FLAG_INFRASTRUCTURE_HANGAR,
    FLAG_HUB_FUEL,
];

/// `corpRoleStationManager` **[V]** (`appConst`).
pub const CORP_ROLE_STATION_MANAGER: i64 = 2_048;

/// The hub type's radius **[V-data]** (R-SOV §1.1, `types`), for a world
/// whose type table lacks the type; the table's own value wins.
pub const HUB_RADIUS: f64 = 45_000.0;

/// **[A]** where a hub goes: this far above the sun's warp-in point
/// (`eveCfg.GetSunWarpInPoint` **[V]**, `warp_in::sun_warp_in`), EvEJS's
/// local policy (a fact, R-SOV §6.2 step 2). The hub's
/// `fixedStarDistance` has no client reader **[V]**, so placement is ours.
/// Falsified by a human who finds retail hubs elsewhere.
pub const HUB_ABOVE_WARP_IN: f64 = 100_000.0;

/// **[A]** a launch is refused unless the ship is this close to where the
/// hub will be, centre to centre (R-SOV §6.2 step 2). Falsified by retail
/// launching at the pilot's own position (`deployAtRange` 1,250 m
/// **[V-data]** suggests it does).
pub const LAUNCH_RANGE: f64 = 1_000_000.0;

/// `maxCargoContainerTransferDistance` **[V]** (R-SOV §2): how far from
/// the hub's surface `/sovfuel` reaches.
pub const TRANSFER_RANGE: f64 = 2_500.0;

/// **[A]** fuel blocks a claimed hub burns per game day, at the rollover
/// (R-SOV §6.7). Falsified by the `[econ]` line's fuel-block sink proving
/// the blocks irrelevant (nobody ever runs short) or ruinous (no alliance
/// keeps a hub fuelled).
pub const FUEL_PER_DAY: i64 = 240;

/// **[A]** offline game days before a hub's claim lapses and the hub is
/// removed (R-SOV §6.7). Falsified as [`FUEL_PER_DAY`]: an owner who wants
/// a forgotten hub to linger longer, or not at all.
pub const GRACE_DAYS: u32 = 2;

/// **[A]** rollover (game) days a launched hub may stay neutral before it
/// is removed, freeing the system for another launch (a never-captured hub
/// burns nothing and would otherwise block `HasHub` for ever). Counted in
/// rollover days, so `--time-scale` shortens it. Falsified by an alliance
/// that cannot finish a capture in this time, or by squatters seen
/// blocking a system for longer.
pub const NEUTRAL_HUB_DAYS: u32 = 7;

/// `entosisConst` `STRUCTURES_UPDATED` **[V]** (R-SOV §2): the
/// `whatChanged` code of a hub row whose state changed.
pub const STRUCTURES_UPDATED: i64 = 1;

/// One blue FILETIME day.
const DAY: i64 = crate::seed::DAY;

// ---------------------------------------------------------------------------
// The hub rows
// ---------------------------------------------------------------------------

/// One launched hub.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hub {
    pub system_id: i64,
    pub hub_id: i64,
    pub position: [f64; 3],
    /// The pilot who launched it; `None` for a GM hub (`/sov set`).
    pub launcher_id: Option<i64>,
    pub launch_time: i64,
    /// Rollover days spent without a day's fuel ([`on_rollover`]), counted
    /// from the day after the one the fuel ran out; while the hub is
    /// neutral, the rollover days since it was launched.
    pub offline_days: u32,
}

fn opt(v: Option<i64>) -> String {
    v.map_or_else(|| "-".to_owned(), |v| v.to_string())
}

fn parse_row(key: &[String], v: &[String]) -> Option<Hub> {
    let num = |i: usize| v.get(i)?.parse::<f64>().ok();
    Some(Hub {
        system_id: key.first()?.parse().ok()?,
        hub_id: v.first()?.parse().ok()?,
        position: [num(1)?, num(2)?, num(3)?],
        launcher_id: match v.get(4)?.as_str() {
            "-" => None,
            s => Some(s.parse().ok()?),
        },
        launch_time: v.get(5)?.parse().ok()?,
        offline_days: v.get(6).and_then(|s| s.parse().ok()).unwrap_or(0),
    })
}

/// `system_id`'s hub, if one was launched there. A row that does not parse
/// is logged and read as absent, as D1 reads its own.
pub fn hub(d: &Deltas, system_id: i64) -> Option<Hub> {
    let key = vec![system_id.to_string()];
    let v = d.of(KIND).get(&key)?;
    let h = parse_row(&key, v);
    if h.is_none() {
        crate::errln!("[sov_hub] skipping row {key:?}: {v:?}");
    }
    h
}

/// Every hub, in system order.
pub fn all(d: &Deltas) -> Vec<Hub> {
    d.of(KIND)
        .iter()
        .filter_map(|(k, v)| parse_row(k, v))
        .collect()
}

/// The hub whose item id is `hub_id`. Reads only the id field of each row
/// before the match, because the slim builders ask on every batch.
pub fn by_id(d: &Deltas, hub_id: i64) -> Option<Hub> {
    let want = hub_id.to_string();
    d.of(KIND)
        .iter()
        .find(|(_, v)| v.first() == Some(&want))
        .and_then(|(k, v)| parse_row(k, v))
}

/// Write one hub row.
pub fn put(d: &mut Deltas, h: &Hub) {
    d.put(
        KIND,
        vec![h.system_id.to_string()],
        vec![
            h.hub_id.to_string(),
            h.position[0].to_string(),
            h.position[1].to_string(),
            h.position[2].to_string(),
            opt(h.launcher_id),
            h.launch_time.to_string(),
            h.offline_days.to_string(),
        ],
    );
}

// ---------------------------------------------------------------------------
// Placement and the ball
// ---------------------------------------------------------------------------

/// Where a hub goes for a star at `star` of radius `star_radius`: the
/// client's own sun warp-in point **[V]** and [`HUB_ABOVE_WARP_IN`] up
/// **[A]**. A translation of the star, so a test can aim it.
pub fn hub_position(star: [f64; 3], star_radius: f64) -> [f64; 3] {
    let w = crate::warp_in::sun_warp_in(star, star_radius);
    [w[0], w[1] + HUB_ABOVE_WARP_IN, w[2]]
}

/// `system`'s star, `(position, radius)`, as the catalogue has it; the
/// origin and radius 0 when it has none (see [`position_in`]).
pub fn star_in(reg: &Registry, system: i64) -> ([f64; 3], f64) {
    reg.celestials()
        .star_of(system)
        .map_or(([0.0; 3], 0.0), |s| (s.position, s.radius))
}

/// [`hub_position`] for `system`'s star as the catalogue has it. **[A]** a
/// system whose star the catalogue lacks (no `--celestials`, or a test's
/// universe) takes its star at the origin with radius 0: system
/// coordinates are star-centred **[I]**, and the point is still off the
/// star. Falsified only by a missing star row, which the extract never has.
pub fn position_in(reg: &Registry, system: i64) -> [f64; 3] {
    match reg.celestials().star_of(system) {
        Some(s) => hub_position(s.position, s.radius),
        None => hub_position([0.0; 3], 0.0),
    }
}

/// The hub type's radius: the type table's, else [`HUB_RADIUS`].
pub fn hub_radius(content: &crate::content::Content) -> f64 {
    content
        .try_get::<crate::typedata::Installed>()
        .and_then(|t| t.table().get(sov::TYPE_INFRASTRUCTURE_HUB))
        .map(|r| r.radius)
        .filter(|r| *r > 0.0)
        .unwrap_or(HUB_RADIUS)
}

/// The owner a hub's slim and ball carry: the claim's corp and alliance,
/// or the neutral owner and none.
fn owners(d: &Deltas, h: &Hub) -> (i64, Option<i64>) {
    match sov::claim(d, h.system_id).filter(|c| c.hub_id == h.hub_id) {
        Some(c) => (c.corporation_id, c.alliance_id),
        None => (sov::NEUTRAL_HUB_OWNER, None),
    }
}

/// The hub's ball: a body that never moves, like a customs office
/// (`planet::CustomsOffice::ball`) -- not free, `Mode::Stop`, no mass --
/// carrying its holder's corp and alliance for the logo **[V]**
/// (`sovereigntyHub.LogoLoader` reads them off the slim; the ball's own
/// pair is what `AddBalls2` writes for every ball).
pub fn ball(reg: &Registry, h: &Hub) -> Ball {
    let (corp, alliance) = owners(reg.deltas(), h);
    Ball {
        ball_id: h.hub_id,
        mode: Mode::Stop,
        radius: hub_radius(reg.content()) as f32,
        position: h.position,
        flags: FLAGS_STATIC_BALL,
        mass: 0.0,
        cloak: 0,
        harmonic: -1,
        corporation_id: i32::try_from(corp).unwrap_or(-1),
        alliance_id: alliance.and_then(|a| i32::try_from(a).ok()).unwrap_or(-1),
        physics: None,
        formation_id: NO_FORMATION,
    }
}

/// The hub's five slim attributes (R-SOV §5.1): `ownerID` and `corpID` are
/// the holder's corp, [`sov::NEUTRAL_HUB_OWNER`] while neutral **[V-data]**;
/// no pilot. The rest ride in [`slim_extras`].
pub fn slim(reg: &Registry, h: &Hub) -> Slim {
    let (corp, _) = owners(reg.deltas(), h);
    Slim {
        item_id: h.hub_id,
        type_id: sov::TYPE_INFRASTRUCTURE_HUB,
        owner_id: corp,
        char_id: None,
        corp_id: Some(corp),
        response_thresholds: None,
    }
}

/// The hub's extra slim keys (R-SOV §5.1): `allianceID` (`None` while
/// neutral; the logo loader and the entosis bracket read it), and
/// `entosis_score` and `nameID`, both `None` until D3a scores a capture.
fn extras_of(d: &Deltas, h: &Hub) -> Vec<(&'static str, PyValue)> {
    let (_, alliance) = owners(d, h);
    vec![
        ("allianceID", alliance.map_or(PyValue::None, PyValue::Int)),
        // SOC-D3a: the score the entosis engine last decided (None until
        // a link has scored on this hub).
        ("entosis_score", crate::entosis::slim_score(d, h.hub_id)),
        ("nameID", PyValue::None),
    ]
}

/// The `slim_extra::Source` row (coordinator: `slim_extra::SOURCES`).
pub fn slim_extras(w: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    if w.deltas().of(KIND).is_empty() {
        return Vec::new();
    }
    all(w.deltas())
        .iter()
        .filter(|h| ids.contains(&h.hub_id))
        .map(|h| (h.hub_id, extras_of(w.deltas(), h)))
        .collect()
}

/// `system`'s hub ball and slim, for `destiny::scene` (coordinator: one
/// line there). Global: it is in no `grid::local_balls`, so every park in
/// the system keeps it.
pub fn balls_in(w: &Registry, system: i64) -> Vec<(Ball, Slim)> {
    hub(w.deltas(), system)
        .map(|h| (ball(w, &h), slim(w, &h)))
        .into_iter()
        .collect()
}

/// Where the hub `ball_id` is, for `Registry::ball_target` (coordinator:
/// one line there): approach, orbit, warp to. A sov structure is category
/// 40, so the client gives it no warp-in point (`HasWarpInPoint`) **[V]**.
pub fn target(w: &Registry, ball_id: i64) -> Option<crate::model::Target> {
    let h = by_id(w.deltas(), ball_id)?;
    Some(crate::model::Target {
        position: h.position,
        radius: hub_radius(w.content()),
        warp_in: None,
    })
}

/// The characters in space in `system`: who sees a ball there.
fn in_space(w: &Registry, system: i64) -> Vec<i64> {
    w.roster()
        .in_system(system)
        .into_iter()
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .collect()
}

/// `AddBalls2` of a hub to **every** park in its system, not only the
/// parks whose grid the point is on: the type is always-global
/// **[V-data]**. A `BallsAdded` declares `GridRole::Adds`, which the drain
/// would cut to the pilots within `GRID_ENTER`; this one declares
/// `Spawns` with nothing needed, which the drain sends whole and adds to
/// every modelled park (`grid::filter`).
#[derive(Debug, Clone, PartialEq)]
pub struct HubAdded(pub crate::destiny::BallsAdded);

impl Push for HubAdded {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        self.0.to_push()
    }

    fn audience(&self) -> Option<Vec<i64>> {
        self.0.audience()
    }

    fn restamp(&mut self, stamp: u32) {
        self.0.restamp(stamp);
    }

    fn cancel_tag(&self) -> Option<crate::notify::CancelTag> {
        self.0.cancel_tag()
    }

    fn grid(&self) -> crate::grid::GridRole {
        let adds = match self.0.grid() {
            crate::grid::GridRole::Adds(placed) => placed.into_iter().map(|(id, _)| id).collect(),
            _ => Vec::new(),
        };
        crate::grid::GridRole::Spawns {
            needs: Vec::new(),
            adds,
        }
    }
}

/// Tell every pilot in the hub's system it is there.
fn announce_added(reg: &mut Registry, h: &Hub) {
    let to = in_space(reg, h.system_id);
    if to.is_empty() {
        return;
    }
    let stamp = reg.next_destiny_stamp();
    let added = crate::destiny::BallsAdded::new(
        to,
        stamp,
        crate::destiny::Park {
            current_time: stamp,
            balls: vec![ball(reg, h)],
        },
        vec![(slim(reg, h), None)],
    )
    .map(|a| a.with_extras(slim_extras(reg, &[h.hub_id])));
    if let Some(a) = added {
        reg.push(HubAdded(a));
    }
}

/// `OnSlimItemChange(hubID, slim)` in a `DoDestinyUpdate`, the slim as an
/// instance (`michelle.py:2006` stores it as it comes) **[V]**: a new
/// holder reloads the hub's logos (`SovereigntyHub.OnSlimItemUpdated`)
/// **[V]**, and D3a's `entosis_score` rides the same push (R-SOV §4).
#[derive(Debug, Clone, PartialEq)]
pub struct HubSlimChange {
    pub to: Vec<i64>,
    pub stamp: u32,
    pub slim: Slim,
    pub extras: Vec<(&'static str, PyValue)>,
}

impl HubSlimChange {
    /// The slim as the client stores it.
    pub fn slim_value(&self) -> PyValue {
        let mut pairs = vec![
            ("itemID", PyValue::Int(self.slim.item_id)),
            ("typeID", PyValue::Int(self.slim.type_id)),
            ("ownerID", PyValue::Int(self.slim.owner_id)),
            ("charID", PyValue::None),
            (
                "corpID",
                self.slim.corp_id.map_or(PyValue::None, PyValue::Int),
            ),
        ];
        pairs.extend(self.extras.iter().cloned());
        crate::pyutil::instance("foo.SlimItem", pairs)
    }
}

impl Push for HubSlimChange {
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let entry = PyValue::Tuple(vec![
            PyValue::Int(i64::from(self.stamp)),
            PyValue::Tuple(vec![
                bstr("OnSlimItemChange"),
                PyValue::Tuple(vec![PyValue::Int(self.slim.item_id), self.slim_value()]),
            ]),
        ]);
        (
            "DoDestinyUpdate",
            vec![PyValue::List(vec![entry]), PyValue::Int(0)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Names(vec![self.slim.item_id])
    }
}

/// Push `system`'s hub slim, as it now reads, to the pilots in space
/// there. Called after any holder change: the GM capture, `/sov set` and
/// `/sov clear` (D1's arm, a coordinator line), and a lapse.
pub fn claim_changed(reg: &mut Registry, system: i64) {
    let Some(h) = hub(reg.deltas(), system) else {
        return;
    };
    let to = in_space(reg, system);
    if to.is_empty() {
        return;
    }
    let stamp = reg.next_destiny_stamp();
    let push = HubSlimChange {
        to,
        stamp,
        slim: slim(reg, &h),
        extras: extras_of(reg.deltas(), &h),
    };
    reg.push(push);
}

/// `{hubID: [STRUCTURES_UPDATED]}`: the `changes` of a hub-row push (a
/// list works where the client tests `in` **[I]**, R-SOV §4).
fn changed(hub_id: i64) -> PyValue {
    PyValue::Dict(vec![(
        PyValue::List(vec![PyValue::Int(STRUCTURES_UPDATED)]),
        PyValue::Int(hub_id),
    )])
}

// ---------------------------------------------------------------------------
// Launch
// ---------------------------------------------------------------------------

/// Why a launch was refused, in words for a `CustomNotify` **[A]** (the
/// client's own deploy messages are the server's to choose; none is read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    NotInSpace,
    NotInHold,
    NotAHub,
    Warping,
    NoAlliance,
    NoRole,
    NotClaimable,
    HasHub,
    TooFar { metres: i64 },
    Store(String),
}

impl Refused {
    pub fn text(&self) -> String {
        match self {
            Refused::NotInSpace => "You can only launch that in space, in your ship.".to_owned(),
            Refused::NotInHold => {
                "That is not in your ship's cargo hold, fleet hangar or infrastructure hangar."
                    .to_owned()
            }
            Refused::NotAHub => "This server can only launch a Sovereignty Hub this way.".to_owned(),
            Refused::Warping => "You cannot launch anything while in warp.".to_owned(),
            Refused::NoAlliance => {
                "A Sovereignty Hub can only be launched for a corporation in an alliance."
                    .to_owned()
            }
            Refused::NoRole => {
                "Only a director, the CEO or a station manager can launch a Sovereignty Hub for the corporation."
                    .to_owned()
            }
            Refused::NotClaimable => {
                "Sovereignty cannot be claimed here: only null-sec without an NPC faction."
                    .to_owned()
            }
            Refused::HasHub => "This system already has a Sovereignty Hub.".to_owned(),
            Refused::TooFar { metres } => format!(
                "A Sovereignty Hub is launched above the sun's warp-in point; fly within {} km of it (you are {} km away).",
                (LAUNCH_RANGE / 1000.0) as i64,
                metres / 1000
            ),
            Refused::Store(e) => format!("The launch failed: {e}"),
        }
    }
}

/// Whether `system` is claimable in the loaded universe (D1's rule).
fn claimable(reg: &Registry, system: i64) -> bool {
    reg.content()
        .try_get::<crate::celestial::Universe>()
        .is_some_and(|u| sov::claimable(u, system))
}

/// The checks of one hub launch, in the order a pilot meets them. `Ok`
/// with the item, the system and where the hub goes.
fn check_launch(
    reg: &mut Registry,
    actor: i64,
    item_id: i64,
) -> Result<(Item, i64, [f64; 3]), Refused> {
    let c = reg.character(actor).cloned().ok_or(Refused::NotInSpace)?;
    reg.advance_character(actor);
    let here = reg
        .roster()
        .get(actor)
        .copied()
        .filter(|p| p.ship_id == c.ship_id)
        .ok_or(Refused::NotInSpace)?;
    let item = reg
        .store()
        .get(item_id)
        .cloned()
        .filter(|i| {
            i.owner_id == actor && i.location_id == c.ship_id && LAUNCH_FLAGS.contains(&i.flag_id)
        })
        .ok_or(Refused::NotInHold)?;
    if item.type_id != sov::TYPE_INFRASTRUCTURE_HUB {
        return Err(Refused::NotAHub);
    }
    if here.motion.warp_active() {
        return Err(Refused::Warping);
    }
    if c.alliance_id.is_none() {
        return Err(Refused::NoAlliance);
    }
    if !crate::service::sov_hub::may_launch(reg, actor) {
        return Err(Refused::NoRole);
    }
    let system = here.solar_system_id;
    if !claimable(reg, system) {
        return Err(Refused::NotClaimable);
    }
    if sov::claim(reg.deltas(), system).is_some() || hub(reg.deltas(), system).is_some() {
        return Err(Refused::HasHub);
    }
    let at = position_in(reg, system);
    let d = crate::space_state::distance(here.motion.position, at);
    if d > LAUNCH_RANGE {
        return Err(Refused::TooFar { metres: d as i64 });
    }
    Ok((item, system, at))
}

/// Put a hub item that is already in space at `system` into the ledgers:
/// its own row, D1's neutral claim row (or the hub id in a claim that
/// waited on a stand-in), the ball for every pilot there, and the
/// structure rows.
fn place(reg: &mut Registry, system: i64, hub_id: i64, launcher: Option<i64>, now: i64) -> Hub {
    let h = Hub {
        system_id: system,
        hub_id,
        position: position_in(reg, system),
        launcher_id: launcher,
        launch_time: now,
        offline_days: 0,
    };
    put(reg.deltas_mut(), &h);
    let row = match sov::claim(reg.deltas(), system) {
        Some(c) => Claim { hub_id, ..c },
        None => Claim {
            system_id: system,
            alliance_id: None,
            corporation_id: sov::NEUTRAL_HUB_OWNER,
            hub_id,
            state: HubState::Neutral,
            claim_time: None,
            state_until: None,
            rest: Vec::new(),
        },
    };
    sov::put(reg.deltas_mut(), &row);
    announce_added(reg, &h);
    h
}

/// The new item id among a plan's changes, else `fallback` (a whole-stack
/// move keeps its id).
fn created(changes: &[crate::item::Change], fallback: i64) -> i64 {
    changes
        .iter()
        .find_map(|ch| match ch {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .unwrap_or(fallback)
}

/// Launch one hub from `actor`'s hold: the checks, the item into space,
/// the rows, the ball, the pushes. The new hub's id.
pub fn launch_one(reg: &mut Registry, actor: i64, item_id: i64) -> Result<i64, Refused> {
    let (item, system, _) = check_launch(reg, actor, item_id)?;
    let to = Slot::new(sov::NEUTRAL_HUB_OWNER, system, FLAG_IN_SPACE);
    let edit = if item.quantity > 1 {
        Edit::Split {
            item_id,
            quantity: 1,
            to,
        }
    } else {
        Edit::Move { item_id, to }
    };
    let changes = reg
        .apply_items(&[edit])
        .map_err(|e| Refused::Store(e.to_string()))?;
    let hub_id = created(&changes, item_id);
    let now = crate::pyutil::filetime_now();
    let h = place(reg, system, hub_id, Some(actor), now);
    let alliances: Vec<i64> = reg
        .character(actor)
        .and_then(|c| c.alliance_id)
        .into_iter()
        .collect();
    sov::push_structures(reg, system, &alliances, Some(changed(h.hub_id)));
    crate::errln!(
        "[sov] hub {} launched in {system} by {actor} at {:?}",
        h.hub_id,
        h.position
    );
    Ok(h.hub_id)
}

/// One refusal in a launch answer: `(label, {args})`, which
/// `eveMisc.LaunchFromShip` raises as `UserError(label, args)` **[V]**.
fn refusal(text: &str) -> PyValue {
    PyValue::Tuple(vec![
        bstr("CustomNotify"),
        PyValue::Dict(crate::pyutil::pypairs(vec![("notify", bstr(text))])),
    ])
}

/// `ship.Drop([(itemID, qty)], whoseBehalfID, ignoreWarning)` (R-SOV
/// §1.2): `{itemID: [newItemID]}` for a launched hub and `{itemID:
/// [(label, args)]}` for a refusal, the shape `LaunchFromShip` sorts by
/// type **[V]**. One hub per stack per call: the client sends `maxQty=1`
/// for "Launch for Corporation" (`menusvc`) **[V]**.
pub fn launch(reg: &mut Registry, actor: Option<i64>, items: &[(i64, i64)]) -> PyValue {
    let mut answer: Vec<(PyValue, PyValue)> = Vec::new();
    for &(item_id, _) in items {
        let entry = match actor {
            None => refusal(&Refused::NotInSpace.text()),
            Some(a) => match launch_one(reg, a, item_id) {
                Ok(id) => PyValue::Int(id),
                Err(r) => refusal(&r.text()),
            },
        };
        answer.push((PyValue::List(vec![entry]), PyValue::Int(item_id)));
    }
    PyValue::Dict(answer)
}

/// **The GM hub** (`/sov set` on a system with no hub; a coordinator line
/// in D1's arm): an item made where the hub goes, in the ledgers as if
/// launched by nobody. A claim D1 made on a stand-in id is moved onto it,
/// and its holder's clients are told (the claim cache keeps a hub id for
/// ever otherwise). The hub's id, or `None` when the type table lacks the
/// hub type (a test's fixture): the claim then keeps D1's stand-in.
pub fn ensure_hub(reg: &mut Registry, system: i64) -> Option<i64> {
    if let Some(h) = hub(reg.deltas(), system) {
        return Some(h.hub_id);
    }
    crate::item::find_in(reg.content(), sov::TYPE_INFRASTRUCTURE_HUB)?;
    let changes = reg
        .apply_items_quiet(&[Edit::Create {
            type_id: sov::TYPE_INFRASTRUCTURE_HUB,
            quantity: 1,
            to: Slot::new(sov::NEUTRAL_HUB_OWNER, system, FLAG_IN_SPACE),
        }])
        .map_err(|e| crate::errln!("[sov_hub] no GM hub in {system}: {e}"))
        .ok()?;
    let hub_id = created(&changes, 0);
    if hub_id == 0 {
        return None;
    }
    let before = sov::claim(reg.deltas(), system);
    let now = crate::pyutil::filetime_now();
    place(reg, system, hub_id, None, now);
    if let Some(holder) = before.as_ref().and_then(Claim::holder) {
        sov::announce(
            reg,
            &sov::Change {
                system_id: system,
                old: Some(holder),
                new: Some(holder),
            },
        );
    }
    Some(hub_id)
}

// ---------------------------------------------------------------------------
// Capture (the GM stand-in for D3a's completion)
// ---------------------------------------------------------------------------

/// **A neutral hub captured by `owner`** (`(allianceID, executorCorpID)`):
/// the GM shortcut. It is the very function the entosis engine calls when a
/// capture completes (`entosis::claim_neutral`, SOC-D3a), so the claim path
/// is the same whether a link or a GM made it. Refused for a system with no
/// hub, or whose hub already has a holder (an attack on a held hub
/// reinforces it).
pub fn capture(reg: &mut Registry, system: i64, owner: (i64, i64)) -> Result<sov::Change, String> {
    crate::entosis::claim_neutral(reg, system, owner)
}

// ---------------------------------------------------------------------------
// Fuel
// ---------------------------------------------------------------------------

/// The fuel stacks in a hub's bay.
pub fn fuel_stacks(reg: &Registry, hub_id: i64) -> Vec<Item> {
    reg.store()
        .items()
        .iter()
        .filter(|i| {
            i.location_id == hub_id && i.flag_id == FLAG_HUB_FUEL && i.group_id == GROUP_FUEL_BLOCK
        })
        .cloned()
        .collect()
}

/// The blocks in a hub's bay, every type counted alike.
pub fn fuel_stock(reg: &Registry, hub_id: i64) -> i64 {
    fuel_stacks(reg, hub_id).iter().map(|i| i.quantity).sum()
}

/// What one rollover of `days` days does to a stock: whole days paid,
/// days not paid, blocks burned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Burn {
    pub burned: i64,
    pub covered: u32,
    pub uncovered: u32,
}

/// **The burn rule**: each day costs [`FUEL_PER_DAY`]. The days the stock
/// pays are burned; a day it cannot pay in full burns what is left and the
/// hub is out of fuel for it and every day after.
pub fn burn(stock: i64, days: u32) -> Burn {
    let can_pay = (stock.max(0) / FUEL_PER_DAY).clamp(0, i64::from(u32::MAX)) as u32;
    let covered = can_pay.min(days);
    let uncovered = days - covered;
    let burned = if uncovered == 0 {
        i64::from(days) * FUEL_PER_DAY
    } else {
        stock.max(0)
    };
    Burn {
        burned,
        covered,
        uncovered,
    }
}

/// The blocks burned over a wall span `[last, now]` of a world born at
/// `birth`, on `clock`: one [`FUEL_PER_DAY`] per rollover day the clock owes
/// (`GameClock::days_owed`), so scale N burns N times as fast.
pub fn burned_between(
    clock: crate::game_clock::GameClock,
    birth: i64,
    last: i64,
    now: i64,
    stock: i64,
) -> Burn {
    burn(stock, clock.days_owed(birth, last, now))
}

/// The wall time a stock lasts on `clock`: its game days of burn, scaled
/// (`GameClock::span`). What `/sovfuel` prints.
pub fn wall_time_left(clock: crate::game_clock::GameClock, stock: i64) -> i64 {
    let game = i128::from(stock.max(0)) * i128::from(DAY) / i128::from(FUEL_PER_DAY);
    clock.span(game.clamp(0, i128::from(i64::MAX)) as i64)
}

/// Take `n` blocks out of a hub's bay, smallest stacks first, without
/// telling anyone (no character owns them). The ledger counts them as
/// destroyed goods (`econ_ledger::goods_of`).
fn consume(reg: &mut Registry, hub_id: i64, n: i64) {
    let mut stacks = fuel_stacks(reg, hub_id);
    stacks.sort_by_key(|i| (i.quantity, i.item_id));
    let mut left = n;
    let mut edits = Vec::new();
    for s in stacks {
        if left <= 0 {
            break;
        }
        let take = left.min(s.quantity);
        edits.push(if take == s.quantity {
            Edit::Destroy { item_id: s.item_id }
        } else {
            Edit::Consume {
                item_id: s.item_id,
                quantity: take,
            }
        });
        left -= take;
    }
    if !edits.is_empty()
        && let Err(e) = reg.apply_items_quiet(&edits)
    {
        crate::errln!("[sov_hub] burn in hub {hub_id} failed: {e}");
    }
}

/// Fold a hub's bay into one stack per type, quietly (one merge a plan:
/// two merges into one stack would conflict).
fn tidy(reg: &mut Registry, hub_id: i64) {
    loop {
        let stacks = fuel_stacks(reg, hub_id);
        let pair = stacks.iter().enumerate().find_map(|(i, a)| {
            stacks[i + 1..]
                .iter()
                .find(|b| b.type_id == a.type_id && !a.singleton && !b.singleton)
                .map(|b| (b.item_id, a.item_id))
        });
        let Some((from, into)) = pair else { return };
        if reg
            .apply_items_quiet(&[Edit::Merge { from, into }])
            .is_err()
        {
            return;
        }
    }
}

/// Set a hub's state (and its offline count), and tell the system and the
/// holder when the state changed.
fn set_state(reg: &mut Registry, h: &Hub, state: HubState, offline_days: u32) {
    let Some(c) = sov::claim(reg.deltas(), h.system_id) else {
        return;
    };
    if h.offline_days != offline_days {
        put(reg.deltas_mut(), &Hub { offline_days, ..*h });
    }
    if c.state == state {
        return;
    }
    // SOC-D3a: a reinforced hub or a hub in its node fight counts its offline
    // days but keeps the campaign's state; the fuel step must not erase it.
    if matches!(c.state, HubState::Reinforced | HubState::NodeFight)
        && matches!(state, HubState::Offline | HubState::Claimed)
    {
        return;
    }
    sov::put(reg.deltas_mut(), &Claim { state, ..c.clone() });
    let alliances: Vec<i64> = c.alliance_id.into_iter().collect();
    sov::push_structures(reg, h.system_id, &alliances, Some(changed(h.hub_id)));
    crate::errln!(
        "[sov] hub {} in {} is now {}",
        h.hub_id,
        h.system_id,
        state.as_str()
    );
}

/// **The claim lapses and the hub goes**: the holder lost (a `sovact` row
/// and D1's pushes), the fuel and the hub item destroyed, both rows
/// dropped, the ball taken out of every park in the system.
pub fn lapse(reg: &mut Registry, h: &Hub, now: i64) {
    // SOC-D3a: its links stop, its score and campaign go.
    crate::entosis::forget(reg, h.system_id, h.hub_id);
    let change = sov::set_claim(reg.deltas_mut(), h.system_id, None, now);
    let mut edits: Vec<Edit> = fuel_stacks(reg, h.hub_id)
        .iter()
        .map(|i| Edit::Destroy { item_id: i.item_id })
        .collect();
    if reg.store().get(h.hub_id).is_some() {
        edits.push(Edit::Destroy { item_id: h.hub_id });
    }
    if !edits.is_empty()
        && let Err(e) = reg.apply_items_quiet(&edits)
    {
        crate::errln!("[sov_hub] hub {} not destroyed: {e}", h.hub_id);
    }
    let key = [h.system_id.to_string()];
    reg.deltas_mut().del(KIND, &key);
    reg.deltas_mut().del(sov::KIND, &key);
    let to = in_space(reg, h.system_id);
    let stamp = reg.next_destiny_stamp();
    if let Some(rm) = crate::destiny::BallsRemoved::new(to, stamp, vec![h.hub_id]) {
        // SOC-D3b: scheduled, not pushed. `lapse` runs inside `on_the_clock`
        // (the rollover, the campaign's end), which filters `pending` through
        // the grid once as it takes it and again as the clock sends it; a
        // `RemoveBalls` is dropped by the second pass (the park model no
        // longer holds the ball). Scheduled, it is filtered once.
        reg.schedule(std::time::Duration::ZERO, rm);
    }
    if let Some(change) = change {
        sov::announce(reg, &change);
    } else {
        sov::push_structures(reg, h.system_id, &[], None);
    }
    crate::errln!(
        "[sov] hub {} in {} lapsed after {} offline days",
        h.hub_id,
        h.system_id,
        h.offline_days
    );
}

/// Run `f` and send what it pushed **by the clock**: the rollover runs on
/// the world tick, whose drain is `take_due`, not `take_pending` (the
/// house pattern, `drones::apply_held`). What was already pending belongs
/// to somebody's turn and is put back untouched.
pub(crate) fn on_the_clock<T>(reg: &mut Registry, f: impl FnOnce(&mut Registry) -> T) -> T {
    let before = reg.take_pending();
    let out = f(reg);
    let ours = reg.take_pending();
    for (after, p) in before {
        reg.push_later(after, crate::notify::Addressed { to: None, inner: p });
    }
    for (_, p) in ours {
        reg.schedule(
            std::time::Duration::ZERO,
            crate::notify::Addressed { to: None, inner: p },
        );
    }
    out
}

/// **The rollover step** (`rollover_steps::STEPS`, a coordinator row):
/// every **claimed** hub burns [`FUEL_PER_DAY`] for each of `d.days`; a
/// neutral hub burns nothing. A hub that cannot pay a day goes `offline`
/// and counts it; at [`GRACE_DAYS`] offline days its claim lapses. A hub
/// that paid every day is online again. Its count is the blocks burned.
pub fn on_rollover(reg: &mut Registry, d: &crate::rollover::Day) -> usize {
    on_the_clock(reg, |reg| {
        let mut total = 0i64;
        for h in all(reg.deltas()) {
            let Some(c) = sov::claim(reg.deltas(), h.system_id) else {
                continue;
            };
            if c.hub_id != h.hub_id {
                continue;
            }
            if !c.is_claimed() {
                // A neutral hub burns nothing but is not kept for ever.
                if c.state == HubState::Neutral {
                    let age = h.offline_days.saturating_add(d.days);
                    if age >= NEUTRAL_HUB_DAYS {
                        lapse(
                            reg,
                            &Hub {
                                offline_days: age,
                                ..h
                            },
                            d.now,
                        );
                    } else {
                        put(
                            reg.deltas_mut(),
                            &Hub {
                                offline_days: age,
                                ..h
                            },
                        );
                    }
                }
                continue;
            }
            let b = burn(fuel_stock(reg, h.hub_id), d.days);
            consume(reg, h.hub_id, b.burned);
            total += b.burned;
            if b.uncovered == 0 {
                set_state(reg, &h, online_state(c.state), 0);
                continue;
            }
            // **[A]** The day the fuel runs out is the day the hub goes
            // offline (count 0); the claim lapses GRACE_DAYS full days
            // later. A hub that runs out inside a catch-up rollover keeps
            // at least one live rollover before it can lapse, so the owner
            // has a window to refuel. Falsified by an owner who loses a hub
            // after downtime without having had a chance to fuel it.
            let days = if c.state == HubState::Offline {
                h.offline_days.saturating_add(b.uncovered)
            } else {
                (b.uncovered - 1).min(GRACE_DAYS.saturating_sub(1))
            };
            if days >= GRACE_DAYS {
                lapse(
                    reg,
                    &Hub {
                        offline_days: days,
                        ..h
                    },
                    d.now,
                );
            } else {
                set_state(reg, &h, HubState::Offline, days);
            }
        }
        usize::try_from(total).unwrap_or(usize::MAX)
    })
}

/// The state a fuelled hub returns to: `claimed` from `offline`, and any
/// other state (D3's) left alone.
fn online_state(s: HubState) -> HubState {
    if s == HubState::Offline {
        HubState::Claimed
    } else {
        s
    }
}

/// What a refuel did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refuelled {
    pub hub_id: i64,
    pub moved: i64,
    pub stock: i64,
    pub online_again: bool,
}

/// **`/sovfuel <qty>` and `/boostsov <qty>`**: move up to `qty` fuel
/// blocks (every block in the hold for `None`) from `actor`'s ship into
/// the local hub's bay. The pilot must be in space, not warping, within
/// [`TRANSFER_RANGE`] of the hub's surface, and in the alliance that holds
/// it (R-SOV §6.7). An offline hub holding a day's fuel again is online.
pub fn refuel(reg: &mut Registry, actor: i64, qty: Option<i64>) -> Result<Refuelled, String> {
    let c = reg
        .character(actor)
        .cloned()
        .ok_or_else(|| "no character".to_owned())?;
    reg.advance_character(actor);
    let here = reg
        .roster()
        .get(actor)
        .copied()
        .filter(|p| p.ship_id == c.ship_id)
        .ok_or_else(|| "You must be in space, in your ship, beside the hub.".to_owned())?;
    if here.motion.warp_active() {
        return Err("You cannot transfer fuel while in warp.".to_owned());
    }
    let system = here.solar_system_id;
    let h = hub(reg.deltas(), system)
        .ok_or_else(|| "There is no Sovereignty Hub in this system.".to_owned())?;
    let claim = sov::claim(reg.deltas(), system).filter(|c| c.hub_id == h.hub_id);
    let Some(holder) = claim.as_ref().and_then(Claim::holder) else {
        return Err("This hub has no owner yet: it burns no fuel until it is claimed.".to_owned());
    };
    if c.alliance_id != Some(holder) {
        return Err("Only the alliance that holds this hub can fuel it.".to_owned());
    }
    let hull = crate::destiny::hull::of_in(reg.content(), c.ship_type_id);
    let gap = crate::space_state::distance(here.motion.position, h.position)
        - hub_radius(reg.content())
        - f64::from(hull.radius);
    if gap > TRANSFER_RANGE {
        return Err(format!(
            "You are {:.1} km from the hub; fuel transfers within {:.1} km of it.",
            gap / 1000.0,
            TRANSFER_RANGE / 1000.0
        ));
    }
    let mut stacks: Vec<Item> = reg
        .store()
        .items()
        .iter()
        .filter(|i| {
            i.owner_id == actor
                && i.location_id == c.ship_id
                && FUEL_SOURCE_FLAGS.contains(&i.flag_id)
                && i.group_id == GROUP_FUEL_BLOCK
        })
        .cloned()
        .collect();
    stacks.sort_by_key(|i| i.item_id);
    let have: i64 = stacks.iter().map(|i| i.quantity).sum();
    if have <= 0 {
        return Err("You carry no fuel blocks.".to_owned());
    }
    let want = qty.unwrap_or(have);
    if want <= 0 {
        return Err("Give a number of blocks above zero.".to_owned());
    }
    let to = Slot::new(sov::NEUTRAL_HUB_OWNER, h.hub_id, FLAG_HUB_FUEL);
    let mut left = want.min(have);
    let mut edits = Vec::new();
    for s in &stacks {
        if left <= 0 {
            break;
        }
        let take = left.min(s.quantity);
        edits.push(if take == s.quantity {
            Edit::Move {
                item_id: s.item_id,
                to,
            }
        } else {
            Edit::Split {
                item_id: s.item_id,
                quantity: take,
                to,
            }
        });
        left -= take;
    }
    let moved = want.min(have) - left;
    reg.apply_items(&edits).map_err(|e| e.to_string())?;
    tidy(reg, h.hub_id);
    let stock = fuel_stock(reg, h.hub_id);
    let state = claim.map_or(HubState::Claimed, |c| c.state);
    let online_again = state == HubState::Offline && stock >= FUEL_PER_DAY;
    if online_again {
        set_state(reg, &h, HubState::Claimed, 0);
    }
    crate::errln!(
        "[sov] {actor} fuelled hub {} with {moved} blocks: {stock} in the bay",
        h.hub_id
    );
    Ok(Refuelled {
        hub_id: h.hub_id,
        moved,
        stock,
        online_again,
    })
}

/// One line on a hub's fuel: stock, days and wall time left, state.
pub fn status_text(reg: &Registry, system: i64) -> String {
    let Some(h) = hub(reg.deltas(), system) else {
        return format!("system {system}: no Sovereignty Hub");
    };
    let stock = fuel_stock(reg, h.hub_id);
    let clock = crate::game_clock::of(reg);
    let state = sov::claim(reg.deltas(), system)
        .filter(|c| c.hub_id == h.hub_id)
        .map_or("neutral", |c| c.state.as_str());
    let wall = wall_time_left(clock, stock);
    format!(
        "hub {} in {system} ({state}): {stock} fuel blocks, {} game days at {FUEL_PER_DAY} a day ({:.1} wall hours at time scale {}){}",
        h.hub_id,
        stock / FUEL_PER_DAY,
        wall as f64 / (3_600.0 * 10_000_000.0),
        clock.scale(),
        if h.offline_days > 0 {
            format!(
                "; offline {} of {GRACE_DAYS} days before the claim lapses",
                h.offline_days
            )
        } else {
            String::new()
        }
    )
}

/// `/sovfuel burn <days>` (GM): run the rollover's fuel step for `days`
/// days now, without the rest of the rollover. For human checks.
pub fn burn_now(reg: &mut Registry, days: u32) -> usize {
    let now = crate::pyutil::filetime_now();
    let day = crate::rollover::Day {
        day: reg.world().day,
        days: days.max(1),
        now,
    };
    on_rollover(reg, &day)
}

/// **Tests only**: one invented world for this module's tests and
/// `service::sov_hub`'s. Every id and number here is invented; the client
/// constants appear only by name.
#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use crate::space_state::{Motion, Presence};

    /// A claimable null-sec system and a high-sec one.
    pub const SYSTEM: i64 = 30_620_001;
    pub const HIGH: i64 = 30_620_002;
    pub const STAR: i64 = 40_620_001;
    pub const STAR_AT: [f64; 3] = [3.0e9, -2.0e8, 7.5e9];
    pub const STAR_RADIUS: f64 = 4.1e8;
    /// Alice: CEO of `CORP`, which founded alliance A. Bob: in `CORP`, no
    /// roles. Carol: CEO of `LONE`, in no alliance. Dave: CEO of `RIVAL`,
    /// which founded alliance D.
    pub const ALICE: i64 = 90_620_001;
    pub const BOB: i64 = 90_620_002;
    pub const CAROL: i64 = 90_620_003;
    pub const DAVE: i64 = 90_620_004;
    pub const CORP: i64 = 98_620_001;
    pub const LONE: i64 = 98_620_002;
    pub const RIVAL: i64 = 98_620_003;
    pub const SHIP: i64 = 100_620_000;
    /// One hub in Alice's cargo, a stack of two more, a hub in Carol's and
    /// Bob's, and fuel in Alice's and Bob's.
    pub const HUB_ITEM: i64 = 100_620_100;
    pub const HUB_PAIR: i64 = 100_620_101;
    pub const CAROL_HUB: i64 = 100_620_102;
    pub const BOB_HUB: i64 = 100_620_103;
    pub const DAVE_HUB: i64 = 100_620_104;
    pub const ALICE_FUEL: i64 = 100_620_200;
    pub const BOB_FUEL: i64 = 100_620_201;
    pub const DAVE_FUEL: i64 = 100_620_202;
    pub const FUEL_TYPE: i64 = 990_620_001;
    pub const OTHER_FUEL_TYPE: i64 = 990_620_002;
    /// Alice's fuel: three days and a bit.
    pub const ALICE_BLOCKS: i64 = 3 * FUEL_PER_DAY + 7;
    pub const HUB_TEST_RADIUS: f64 = 20_000.0;

    pub fn ship(who: i64) -> i64 {
        SHIP + (who - ALICE) + 1
    }

    /// The type rows: the fixture's, a hub row under the hub's type id with
    /// invented numbers, and two invented fuel blocks.
    pub fn content() -> crate::content::Content {
        let mut c = crate::content::Content::fixture();
        let rows = format!(
            "{}type\t{}\t{}\t{}\t1\t1\t10.0\t0.0\t1000.0\t{HUB_TEST_RADIUS}\t0\t\t\tInvented Hub Row\n\
             type\t{FUEL_TYPE}\t{GROUP_FUEL_BLOCK}\t4\t1\t40\t5.0\t0.0\t1.0\t1.0\t0\t\t\tInvented Fuel Block\n\
             type\t{OTHER_FUEL_TYPE}\t{GROUP_FUEL_BLOCK}\t4\t1\t40\t5.0\t0.0\t1.0\t1.0\t0\t\t\tInvented Other Fuel Block\n",
            crate::typedata::FIXTURE,
            sov::TYPE_INFRASTRUCTURE_HUB,
            GROUP_INFRASTRUCTURE_HUB,
            CATEGORY_SOVEREIGNTY_STRUCTURE,
        );
        let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
        c.insert("typedata", crate::typedata::Installed::build(table));
        let sys = |id: i64, sec: &str, name: &str| {
            format!("{id}\t10620001\t20620001\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\t{name}")
        };
        let u = crate::celestial::Universe::parse(
            &[
                sys(SYSTEM, "-0.45", "Invented Null"),
                sys(HIGH, "0.9", "Invented High"),
            ],
            &["10620001\t\t\t\t\tInvented Region".to_owned()],
            &["20620001\t10620001\t\t\t\tInvented Constellation".to_owned()],
        )
        .expect("the test universe parses");
        c.insert("universe", u);
        c
    }

    fn item(
        c: &crate::content::Content,
        item_id: i64,
        type_id: i64,
        owner: i64,
        flag_id: i64,
        quantity: i64,
    ) -> Item {
        let t = crate::item::find_in(c, type_id).expect("a test type");
        Item {
            item_id,
            type_id,
            owner_id: owner,
            location_id: ship(owner),
            flag_id,
            quantity,
            group_id: t.group_id,
            category_id: t.category_id,
            singleton: false,
        }
    }

    /// Put `who` in space at `at`, at rest.
    pub fn place_at(reg: &mut Registry, who: i64, at: [f64; 3]) {
        let _ = reg.roster_mut().leave(who);
        reg.roster_mut().arrive(Presence::arriving(
            who,
            ship(who),
            SYSTEM,
            Motion::arrived(at, [0.0; 3]),
        ));
    }

    /// `at` moved by `d`.
    pub fn off(at: [f64; 3], d: [f64; 3]) -> [f64; 3] {
        [at[0] + d[0], at[1] + d[1], at[2] + d[2]]
    }

    /// Where a pilot can launch from but not fuel: 200 km off the point.
    pub fn launch_spot(reg: &Registry) -> [f64; 3] {
        off(position_in(reg, SYSTEM), [0.0, 0.0, 200_000.0])
    }

    /// Where a pilot can fuel: half a kilometre off the hub's surface.
    pub fn fuel_spot(reg: &Registry) -> [f64; 3] {
        off(
            position_in(reg, SYSTEM),
            [0.0, 0.0, hub_radius(reg.content()) + 500.0],
        )
    }

    /// The world: four pilots in space in [`SYSTEM`] at the launch spot,
    /// their corps and alliances, the items, the star.
    pub fn world() -> (Registry, i64, i64) {
        let pilot = |id: i64, name: &str| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship(id),
            solar_system_id: Some(SYSTEM),
            ..crate::model::Character::default()
        };
        let content = std::sync::Arc::new(content());
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice"),
                pilot(BOB, "Bob"),
                pilot(CAROL, "Carol"),
                pilot(DAVE, "Dave"),
            ],
            celestials: crate::celestial::Catalogue::parse(&format!(
                "star\t{STAR}\t990620010\t{STAR_RADIUS}\t{}\t{}\t{}\t{SYSTEM}\t0\t0\n",
                STAR_AT[0], STAR_AT[1], STAR_AT[2]
            )),
            ..crate::model::SandboxConfig::default()
        })
        .with_content(content.clone());
        let mut items = reg.store().items().to_vec();
        let hub = sov::TYPE_INFRASTRUCTURE_HUB;
        items.extend([
            item(&content, HUB_ITEM, hub, ALICE, FLAG_CARGO, 1),
            item(&content, HUB_PAIR, hub, ALICE, FLAG_FLEET_HANGAR, 2),
            item(&content, CAROL_HUB, hub, CAROL, FLAG_CARGO, 1),
            item(&content, BOB_HUB, hub, BOB, FLAG_CARGO, 1),
            item(&content, DAVE_HUB, hub, DAVE, FLAG_CARGO, 1),
            item(
                &content,
                ALICE_FUEL,
                FUEL_TYPE,
                ALICE,
                FLAG_CARGO,
                ALICE_BLOCKS,
            ),
            item(&content, BOB_FUEL, OTHER_FUEL_TYPE, BOB, FLAG_CARGO, 10),
            item(&content, DAVE_FUEL, FUEL_TYPE, DAVE, FLAG_CARGO, 10),
        ]);
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_621_000,
            ..crate::store::Snapshot::default()
        });
        crate::service::sov_hub::test_corp(&mut reg, CORP, ALICE, &[(ALICE, 0), (BOB, 0)]);
        crate::service::sov_hub::test_corp(&mut reg, LONE, CAROL, &[(CAROL, 0)]);
        crate::service::sov_hub::test_corp(&mut reg, RIVAL, DAVE, &[(DAVE, 0)]);
        let mut st = crate::alliance::Alliances::load(reg.deltas());
        let a = st
            .create(CORP, ALICE, "Invented Sov Holders", "ISH", "", "", 1)
            .expect("an alliance");
        let d = st
            .create(RIVAL, DAVE, "Invented Rivals", "IRV", "", "", 1)
            .expect("a second alliance");
        st.save(reg.deltas_mut());
        for (who, corp, alliance) in [
            (ALICE, CORP, Some(a)),
            (BOB, CORP, Some(a)),
            (CAROL, LONE, None),
            (DAVE, RIVAL, Some(d)),
        ] {
            let c = reg.character_mut(who).expect("a pilot");
            c.corporation_id = corp;
            c.alliance_id = alliance;
        }
        let spot = launch_spot(&reg);
        for (n, who) in [ALICE, BOB, CAROL, DAVE].into_iter().enumerate() {
            place_at(&mut reg, who, spot);
            let n = n as i64 + 1;
            reg.session_opened(crate::session_table::SessionEntry {
                client_id: n,
                session_id: n,
                user_id: n,
                character_id: Some(who),
            });
        }
        let _ = reg.take_pending();
        (reg, a, d)
    }

    /// Every push queued for the caller's flush: `(name, args, audience)`.
    /// One push: `(name, args, audience)`.
    pub type Sent = (&'static str, Vec<PyValue>, Option<Vec<i64>>);

    pub fn pending(reg: &mut Registry) -> Vec<Sent> {
        reg.take_pending()
            .into_iter()
            .map(|(_, p)| {
                let (n, a) = p.to_push();
                (n, a, p.audience())
            })
            .collect()
    }

    /// Every push the clock would send now.
    pub fn due(reg: &mut Registry) -> Vec<Sent> {
        reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(60))
            .into_iter()
            .map(|p| {
                let (n, a) = p.to_push();
                (n, a, p.audience())
            })
            .collect()
    }

    /// Launch Alice's single hub; its id.
    pub fn launched(reg: &mut Registry) -> i64 {
        launch_one(reg, ALICE, HUB_ITEM).expect("Alice launches")
    }

    /// The executor pair of alliance `a`.
    pub fn owner(reg: &Registry, a: i64) -> (i64, i64) {
        let exec = crate::alliance::Alliances::load(reg.deltas())
            .get(a)
            .and_then(|x| x.executor_corp_id)
            .expect("an executor");
        (a, exec)
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;

    /// Every string and number in `v`, bytes decoded, for `contains`.
    fn text(v: &PyValue) -> String {
        let mut out = String::new();
        fn walk(v: &PyValue, out: &mut String) {
            match v {
                PyValue::Bytes(b) => out.push_str(&String::from_utf8_lossy(b)),
                PyValue::Unicode(u) => out.push_str(u),
                PyValue::Int(i) => out.push_str(&i.to_string()),
                PyValue::List(l) | PyValue::Tuple(l) => l.iter().for_each(|x| walk(x, out)),
                PyValue::Dict(d) => d.iter().for_each(|(a, b)| {
                    walk(a, out);
                    walk(b, out);
                }),
                PyValue::Instance { state, .. } => walk(state, out),
                other => out.push_str(&format!("{other:?}")),
            }
            out.push(' ');
        }
        walk(v, &mut out);
        out
    }

    /// The entry of a `Drop` answer for `item`.
    fn entry(answer: &PyValue, item: i64) -> Vec<PyValue> {
        let PyValue::Dict(pairs) = answer else {
            panic!("a dict: {answer:?}")
        };
        match pairs.iter().find(|(_, k)| *k == PyValue::Int(item)) {
            Some((PyValue::List(v), _)) => v.clone(),
            other => panic!("no list for {item}: {other:?}"),
        }
    }

    #[test]
    fn a_hub_goes_above_the_suns_warp_in_point() {
        let (reg, _, _) = world();
        let w = crate::warp_in::sun_warp_in(STAR_AT, STAR_RADIUS);
        assert_eq!(
            position_in(&reg, SYSTEM),
            [w[0], w[1] + HUB_ABOVE_WARP_IN, w[2]]
        );
        // No star row: the origin stands in for it.
        let w0 = crate::warp_in::sun_warp_in([0.0; 3], 0.0);
        assert_eq!(
            position_in(&reg, HIGH),
            [w0[0], w0[1] + HUB_ABOVE_WARP_IN, w0[2]]
        );
        // A translation of the star, so a test can aim it.
        let a = hub_position([0.0; 3], STAR_RADIUS);
        let b = hub_position(STAR_AT, STAR_RADIUS);
        for i in 0..3 {
            assert!((b[i] - a[i] - STAR_AT[i]).abs() < 1e-3);
        }
    }

    #[test]
    fn the_burn_pays_whole_days_and_a_short_day_empties_the_bay() {
        assert_eq!(
            burn(2 * FUEL_PER_DAY + 3, 1),
            Burn {
                burned: FUEL_PER_DAY,
                covered: 1,
                uncovered: 0
            }
        );
        assert_eq!(
            burn(2 * FUEL_PER_DAY + 3, 2),
            Burn {
                burned: 2 * FUEL_PER_DAY,
                covered: 2,
                uncovered: 0
            }
        );
        // Three days owed, two paid: the rest burns and one day is unpaid.
        assert_eq!(
            burn(2 * FUEL_PER_DAY + 3, 3),
            Burn {
                burned: 2 * FUEL_PER_DAY + 3,
                covered: 2,
                uncovered: 1
            }
        );
        assert_eq!(
            burn(0, 2),
            Burn {
                burned: 0,
                covered: 0,
                uncovered: 2
            }
        );
        assert_eq!(burn(FUEL_PER_DAY - 1, 1).uncovered, 1);
    }

    #[test]
    fn fuel_burns_n_times_as_fast_in_wall_time_at_time_scale_n() {
        use crate::game_clock::GameClock;
        const N: u32 = 6;
        let scaled = GameClock::new(N).expect("a valid scale");
        let birth = 134_000_000_000_000_000;
        let last = birth + 5 * DAY + 3_600 * 10_000_000;
        let stock = 100 * FUEL_PER_DAY;
        // One wall day: one game day at scale 1, N at scale N.
        let real = burned_between(GameClock::REAL, birth, last, last + DAY, stock);
        let fast = burned_between(scaled, birth, last, last + DAY, stock);
        assert_eq!(real.burned, FUEL_PER_DAY);
        assert_eq!(fast.burned, i64::from(N) * FUEL_PER_DAY);
        assert_eq!((real.uncovered, fast.uncovered), (0, 0));
        // Less than a compressed day burns nothing at either scale.
        let short = scaled.day() - 1;
        assert_eq!(
            burned_between(scaled, birth, last, last + short, stock).burned,
            0
        );
        // What a stock lasts: N times shorter in wall time.
        let days = 4;
        let wall_real = wall_time_left(GameClock::REAL, days * FUEL_PER_DAY);
        let wall_fast = wall_time_left(scaled, days * FUEL_PER_DAY);
        assert_eq!(wall_real, days * DAY);
        assert_eq!(wall_fast, days * DAY / i64::from(N));
        assert_eq!(wall_time_left(scaled, 0), 0);
    }

    #[test]
    fn drop_returns_the_new_id_keyed_by_the_source_item() {
        let (mut reg, _, _) = world();
        // A single hub keeps its id.
        let answer = launch(&mut reg, Some(ALICE), &[(HUB_ITEM, 1)]);
        assert_eq!(entry(&answer, HUB_ITEM), vec![PyValue::Int(HUB_ITEM)]);
        // The refusal form: a `(label, {args})` tuple, which
        // `LaunchFromShip` raises as a `UserError` [V].
        let answer = launch(&mut reg, Some(ALICE), &[(HUB_PAIR, 1)]);
        let e = entry(&answer, HUB_PAIR);
        let PyValue::Tuple(t) = &e[0] else {
            panic!("a tuple: {e:?}")
        };
        assert_eq!(t[0], bstr("CustomNotify"));
        assert!(matches!(t[1], PyValue::Dict(_)));
        assert!(text(&t[1]).contains("already has"), "{t:?}");
        // A stack splits: the new id is not the stack's, the stack keeps one.
        let (mut reg, _, _) = world();
        let answer = launch(&mut reg, Some(ALICE), &[(HUB_PAIR, 1)]);
        let PyValue::Int(new) = entry(&answer, HUB_PAIR)[0] else {
            panic!("an id: {answer:?}")
        };
        assert_ne!(new, HUB_PAIR);
        assert_eq!(reg.store().get(HUB_PAIR).map(|i| i.quantity), Some(1));
        assert_eq!(hub(reg.deltas(), SYSTEM).map(|h| h.hub_id), Some(new));
        // Not a hub.
        let answer = launch(&mut reg, Some(ALICE), &[(ALICE_FUEL, 1)]);
        assert!(text(&answer).contains("only launch a Sovereignty Hub"));
    }

    #[test]
    fn dropped_hub_is_neutral_and_owned_by_the_deploy_default_owner() {
        let (mut reg, _, _) = world();
        let id = launched(&mut reg);
        let i = reg.store().get(id).expect("the hub is still an item");
        assert_eq!(
            (i.owner_id, i.location_id, i.flag_id),
            (sov::NEUTRAL_HUB_OWNER, SYSTEM, FLAG_IN_SPACE)
        );
        let c = sov::claim(reg.deltas(), SYSTEM).expect("a hub row");
        assert_eq!(c.hub_id, id);
        assert_eq!(c.state, HubState::Neutral);
        assert_eq!(c.alliance_id, None);
        assert_eq!(c.corporation_id, sov::NEUTRAL_HUB_OWNER);
        assert_eq!(sov::holder(reg.deltas(), SYSTEM), None);
        assert_eq!(sov::claim_info(Some(&c)), PyValue::None, "no claim yet");
        let h = hub(reg.deltas(), SYSTEM).expect("a sovhub row");
        assert_eq!(h.position, position_in(&reg, SYSTEM));
        assert_eq!(h.launcher_id, Some(ALICE));
        // The structure row names the hub, neutral.
        let rows = text(&sov::structure_rows(reg.deltas(), SYSTEM));
        assert!(rows.contains(&id.to_string()), "{rows}");
        // Told: the ball to every pilot in the system, the structure rows.
        let sent = pending(&mut reg);
        let add = sent
            .iter()
            .find(|(n, a, _)| *n == "DoDestinyUpdate" && text(&a[0]).contains("AddBalls2"))
            .expect("an AddBalls2");
        let mut to = add.2.clone().unwrap_or_default();
        to.sort_unstable();
        assert_eq!(to, vec![ALICE, BOB, CAROL, DAVE]);
        assert!(
            sent.iter()
                .any(|(n, _, _)| *n == "OnSolarSystemSovStructuresUpdated")
        );
        // The hold lost it: an item change for Alice.
        assert!(
            sent.iter()
                .any(|(n, _, to)| *n == "OnItemChange" && to.as_deref() == Some(&[ALICE][..]))
        );
    }

    #[test]
    fn drop_refuses_without_alliance_role_or_in_a_claimed_system_or_in_warp() {
        let (mut reg, _, _) = world();
        assert_eq!(
            launch_one(&mut reg, CAROL, CAROL_HUB),
            Err(Refused::NoAlliance)
        );
        assert_eq!(launch_one(&mut reg, BOB, BOB_HUB), Err(Refused::NoRole));
        assert_eq!(
            launch_one(&mut reg, ALICE, BOB_HUB),
            Err(Refused::NotInHold),
            "not her hold"
        );
        // Too far from the point.
        let far = off(position_in(&reg, SYSTEM), [0.0, LAUNCH_RANGE + 1.0, 0.0]);
        place_at(&mut reg, ALICE, far);
        assert!(matches!(
            launch_one(&mut reg, ALICE, HUB_ITEM),
            Err(Refused::TooFar { .. })
        ));
        // In warp.
        let spot = launch_spot(&reg);
        place_at(&mut reg, ALICE, spot);
        if let Some(p) = reg.roster_mut().get_mut(ALICE) {
            p.motion.mode = Mode::Warp {
                goto: [1.0e12, 0.0, 0.0],
                effect_stamp: 0,
                total_distance: 0.0,
                stop_distance: 0.0,
                warp_factor: 3_000,
            };
        }
        assert_eq!(launch_one(&mut reg, ALICE, HUB_ITEM), Err(Refused::Warping));
        // Docked: not in space.
        let _ = reg.roster_mut().leave(ALICE);
        assert_eq!(
            launch_one(&mut reg, ALICE, HUB_ITEM),
            Err(Refused::NotInSpace)
        );
        place_at(&mut reg, ALICE, spot);
        // A claimed system (D1's GM claim on a stand-in) takes no hub.
        let mine = crate::alliance::of_corp(reg.deltas(), CORP).expect("allied");
        let pair = owner(&reg, mine);
        sov::set_claim(reg.deltas_mut(), SYSTEM, Some(pair), 1);
        assert_eq!(launch_one(&mut reg, ALICE, HUB_ITEM), Err(Refused::HasHub));
        // Nothing moved by any refusal.
        assert_eq!(
            reg.store().get(HUB_ITEM).map(|i| i.location_id),
            Some(ship(ALICE))
        );
        // High-sec is not claimable.
        let (mut reg, _, _) = world();
        let _ = reg.roster_mut().leave(ALICE);
        let at = position_in(&reg, HIGH);
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                ALICE,
                ship(ALICE),
                HIGH,
                crate::space_state::Motion::arrived(at, [0.0; 3]),
            ));
        assert_eq!(
            launch_one(&mut reg, ALICE, HUB_ITEM),
            Err(Refused::NotClaimable)
        );
    }

    #[test]
    fn second_hub_in_a_system_is_refused() {
        let (mut reg, _, _) = world();
        launched(&mut reg);
        assert_eq!(launch_one(&mut reg, ALICE, HUB_PAIR), Err(Refused::HasHub));
        assert_eq!(launch_one(&mut reg, DAVE, DAVE_HUB), Err(Refused::HasHub));
        assert_eq!(all(reg.deltas()).len(), 1);
    }

    #[test]
    fn hub_slim_has_owner_corp_and_alliance_and_no_score_when_idle() {
        let (mut reg, a, _) = world();
        let id = launched(&mut reg);
        let scene = crate::destiny::scene(&reg, SYSTEM, ship(ALICE), None, 1);
        let hub_ball = scene
            .park
            .balls
            .iter()
            .find(|b| b.ball_id == id)
            .expect("the hub is a ball in the scene");
        assert_eq!(hub_ball.position, position_in(&reg, SYSTEM));
        assert_eq!(f64::from(hub_ball.radius), HUB_TEST_RADIUS);
        assert_eq!(hub_ball.alliance_id, -1);
        let s = scene
            .slims
            .iter()
            .find(|s| s.item_id == id)
            .copied()
            .expect("a slim");
        assert_eq!(s.type_id, sov::TYPE_INFRASTRUCTURE_HUB);
        assert_eq!(s.owner_id, sov::NEUTRAL_HUB_OWNER);
        assert_eq!(s.corp_id, Some(sov::NEUTRAL_HUB_OWNER));
        assert_eq!(s.char_id, None);
        let extras = |reg: &Registry| -> Vec<(&'static str, PyValue)> {
            slim_extras(reg, &[id, ship(ALICE)])
                .into_iter()
                .find(|(i, _)| *i == id)
                .map(|(_, e)| e)
                .expect("extras for the hub only")
        };
        let e = extras(&reg);
        for k in ["allianceID", "entosis_score", "nameID"] {
            assert_eq!(
                e.iter().find(|(n, _)| *n == k).map(|(_, v)| v),
                Some(&PyValue::None),
                "{k} while neutral"
            );
        }
        assert!(slim_extras(&reg, &[ship(ALICE)]).is_empty());
        // Captured: the holder's corp and alliance, and an idle score (SOC-D3a:
        // the client ignores a `None` score, so a hub that has been scored on
        // answers a rate-0 tuple).
        let (alliance, exec) = owner(&reg, a);
        capture(&mut reg, SYSTEM, (alliance, exec)).expect("captured");
        let s = slim(&reg, &hub(reg.deltas(), SYSTEM).unwrap());
        assert_eq!((s.owner_id, s.corp_id), (exec, Some(exec)));
        let e = extras(&reg);
        assert_eq!(
            e.iter().find(|(n, _)| *n == "allianceID").map(|(_, v)| v),
            Some(&PyValue::Int(alliance))
        );
        let Some(PyValue::Tuple(score)) = e
            .iter()
            .find(|(n, _)| *n == "entosis_score")
            .map(|(_, v)| v)
        else {
            panic!("an idle score tuple")
        };
        assert_eq!(score[3], PyValue::Int(alliance), "the defender");
        let b = ball(&reg, &hub(reg.deltas(), SYSTEM).unwrap());
        assert_eq!(i64::from(b.alliance_id), alliance);
    }

    #[test]
    fn every_park_in_the_system_gets_the_hub_however_far() {
        let (mut reg, _, _) = world();
        // Dave's park is modelled (a state reached him) and he is far
        // beyond any grid from the point.
        crate::destiny::on_ballpark_bind(&mut reg, Some(DAVE));
        let _ = reg.take_pending();
        assert!(reg.ext::<crate::grid::Parks>().of(DAVE).is_some());
        let far = off(
            position_in(&reg, SYSTEM),
            [0.0, 0.0, 50.0 * crate::grid::GRID_ENTER],
        );
        place_at(&mut reg, DAVE, far);
        let id = launched(&mut reg);
        let sent = pending(&mut reg);
        assert!(
            sent.iter().any(|(n, a, to)| *n == "DoDestinyUpdate"
                && text(&a[0]).contains("AddBalls2")
                && to.as_ref().is_some_and(|t| t.contains(&DAVE))),
            "{sent:?}"
        );
        assert!(
            reg.ext::<crate::grid::Parks>()
                .of(DAVE)
                .is_some_and(|p| p.contains(&id)),
            "his park now holds it"
        );
        // A later state keeps it: it is not a local ball.
        assert!(crate::grid::grid_of(&reg, SYSTEM, ship(DAVE), far).contains(&id));
        assert!(
            !crate::grid::local_balls(&reg, SYSTEM)
                .iter()
                .any(|(b, _)| *b == id)
        );
        // Approach and warp resolve it.
        let t = reg.ball_target(id).expect("a target");
        assert_eq!(t.position, position_in(&reg, SYSTEM));
        assert_eq!(t.radius, HUB_TEST_RADIUS);
        assert_eq!(t.warp_in, None);
    }

    #[test]
    fn gm_sov_claim_skips_capture_and_pushes_the_claim() {
        let (mut reg, a, d) = world();
        let pair = owner(&reg, a);
        assert!(capture(&mut reg, SYSTEM, pair).is_err(), "no hub yet");
        let id = launched(&mut reg);
        let _ = reg.take_pending();
        let change = capture(&mut reg, SYSTEM, pair).expect("captured");
        assert_eq!((change.old, change.new), (None, Some(a)));
        let c = sov::claim(reg.deltas(), SYSTEM).expect("claimed");
        assert_eq!(
            (c.hub_id, c.alliance_id, c.corporation_id, c.state),
            (id, Some(a), pair.1, HubState::Claimed)
        );
        assert!(c.claim_time.is_some());
        assert_eq!(sov::recent_activity(reg.deltas(), i64::MAX / 2).len(), 0);
        let sent = pending(&mut reg);
        for name in [
            "ProcessSovStatusChanged",
            "OnSovereigntyChanged",
            "OnSolarSystemSovStructuresUpdated",
        ] {
            assert!(sent.iter().any(|(n, _, _)| *n == name), "{name}: {sent:?}");
        }
        let slim = sent
            .iter()
            .find(|(n, a, _)| *n == "DoDestinyUpdate" && text(&a[0]).contains("OnSlimItemChange"))
            .expect("OnSlimItemChange");
        assert!(text(&slim.1[0]).contains(&a.to_string()));
        // The claim info names the real hub.
        let info = text(&sov::claim_info(Some(&c)));
        assert!(info.contains(&id.to_string()), "{info}");
        // A held hub is not captured again, by anyone.
        let rival = owner(&reg, d);
        assert!(capture(&mut reg, SYSTEM, rival).is_err());
        assert_eq!(sov::holder(reg.deltas(), SYSTEM), Some(a));
    }

    #[test]
    fn gm_set_makes_a_real_hub_and_moves_a_stand_in_claim_onto_it() {
        let (mut reg, a, _) = world();
        let pair = owner(&reg, a);
        sov::set_claim(reg.deltas_mut(), SYSTEM, Some(pair), 1).expect("a stand-in claim");
        assert_eq!(
            sov::claim(reg.deltas(), SYSTEM).map(|c| c.hub_id),
            Some(sov::stand_in_hub_id(SYSTEM))
        );
        let id = ensure_hub(&mut reg, SYSTEM).expect("a GM hub");
        assert!(reg.store().get(id).is_some());
        let c = sov::claim(reg.deltas(), SYSTEM).expect("the claim");
        assert_eq!((c.hub_id, c.holder()), (id, Some(a)));
        assert_eq!(hub(reg.deltas(), SYSTEM).map(|h| h.launcher_id), Some(None));
        // The holder's caches hear the new id.
        let sent = pending(&mut reg);
        assert!(sent.iter().any(|(n, _, _)| *n == "ProcessSovStatusChanged"));
        // Asked again: the same hub.
        assert_eq!(ensure_hub(&mut reg, SYSTEM), Some(id));
        // A world whose type table lacks the hub keeps the stand-in.
        let mut bare = Registry::new(crate::model::SandboxConfig::default());
        assert_eq!(ensure_hub(&mut bare, SYSTEM), None);
    }

    #[test]
    fn sovfuel_moves_blocks_from_cargo_within_transfer_range_only() {
        let (mut reg, a, _) = world();
        let id = launched(&mut reg);
        let near = fuel_spot(&reg);
        place_at(&mut reg, ALICE, near);
        assert!(
            refuel(&mut reg, ALICE, Some(5))
                .unwrap_err()
                .contains("no owner yet"),
            "a neutral hub burns nothing"
        );
        let pair = owner(&reg, a);
        capture(&mut reg, SYSTEM, pair).expect("captured");
        // Out of range.
        let spot = launch_spot(&reg);
        place_at(&mut reg, ALICE, spot);
        assert!(
            refuel(&mut reg, ALICE, Some(5))
                .unwrap_err()
                .contains("km from the hub")
        );
        place_at(&mut reg, ALICE, near);
        let _ = reg.take_pending();
        let r = refuel(&mut reg, ALICE, Some(FUEL_PER_DAY)).expect("fuelled");
        assert_eq!(
            (r.hub_id, r.moved, r.stock),
            (id, FUEL_PER_DAY, FUEL_PER_DAY)
        );
        assert_eq!(
            reg.store().get(ALICE_FUEL).map(|i| i.quantity),
            Some(ALICE_BLOCKS - FUEL_PER_DAY)
        );
        assert!(
            pending(&mut reg)
                .iter()
                .any(|(n, _, to)| *n == "OnItemChange" && to.as_deref() == Some(&[ALICE][..]))
        );
        // The rest, then more than she has: what she has.
        let r = refuel(&mut reg, ALICE, None).expect("fuelled");
        assert_eq!(r.stock, ALICE_BLOCKS);
        assert!(
            !reg.store()
                .items()
                .iter()
                .any(|i| i.location_id == ship(ALICE) && i.group_id == GROUP_FUEL_BLOCK),
            "her hold is empty of fuel"
        );
        assert!(
            refuel(&mut reg, ALICE, Some(1))
                .unwrap_err()
                .contains("no fuel")
        );
        // One stack per type in the bay.
        assert_eq!(fuel_stacks(&reg, id).len(), 1);
        // A member of the holding alliance, another block type, counts alike.
        place_at(&mut reg, BOB, near);
        let r = refuel(&mut reg, BOB, Some(4)).expect("Bob fuels");
        assert_eq!(r.stock, ALICE_BLOCKS + 4);
        assert_eq!(fuel_stacks(&reg, id).len(), 2);
        // Another alliance may not.
        place_at(&mut reg, DAVE, near);
        assert!(
            refuel(&mut reg, DAVE, Some(1))
                .unwrap_err()
                .contains("Only the alliance")
        );
        // The fuel belongs to the hub, not to whoever brought it.
        assert!(
            fuel_stacks(&reg, id)
                .iter()
                .all(|i| i.owner_id == sov::NEUTRAL_HUB_OWNER && i.flag_id == FLAG_HUB_FUEL)
        );
    }

    #[test]
    fn fuel_burns_at_rollover_and_an_empty_hub_goes_offline_then_lapses() {
        let (mut reg, a, _) = world();
        let id = launched(&mut reg);
        let day = |reg: &Registry, days: u32| crate::rollover::Day {
            day: reg.world().day + 1,
            days,
            now: crate::pyutil::filetime_now(),
        };
        // A neutral hub burns nothing.
        assert_eq!(
            {
                let dd = day(&reg, 1);
                on_rollover(&mut reg, &dd)
            },
            0
        );
        let pair = owner(&reg, a);
        capture(&mut reg, SYSTEM, pair).expect("captured");
        {
            let at = fuel_spot(&reg);
            place_at(&mut reg, ALICE, at)
        };
        refuel(&mut reg, ALICE, Some(FUEL_PER_DAY + 1)).expect("fuelled");
        let _ = reg.take_pending();
        // Day one is paid.
        assert_eq!(
            {
                let dd = day(&reg, 1);
                on_rollover(&mut reg, &dd)
            },
            usize::try_from(FUEL_PER_DAY).unwrap()
        );
        assert_eq!(fuel_stock(&reg, id), 1);
        assert_eq!(
            sov::claim(reg.deltas(), SYSTEM).map(|c| c.state),
            Some(HubState::Claimed)
        );
        // Day two is not: the last block burns and the hub is offline, its
        // claim still held.
        assert_eq!(
            {
                let dd = day(&reg, 1);
                on_rollover(&mut reg, &dd)
            },
            1
        );
        assert_eq!(fuel_stock(&reg, id), 0);
        let c = sov::claim(reg.deltas(), SYSTEM).expect("still a row");
        assert_eq!(c.state, HubState::Offline);
        assert_eq!(c.holder(), Some(a));
        assert_eq!(hub(reg.deltas(), SYSTEM).map(|h| h.offline_days), Some(0));
        let sent = due(&mut reg);
        assert!(
            sent.iter()
                .any(|(n, _, _)| *n == "OnSolarSystemSovStructuresUpdated"),
            "the offline edge goes by the clock: {sent:?}"
        );
        // A day's fuel brings it back.
        let r = refuel(&mut reg, ALICE, Some(FUEL_PER_DAY)).expect("fuelled");
        assert!(r.online_again);
        assert_eq!(
            sov::claim(reg.deltas(), SYSTEM).map(|c| c.state),
            Some(HubState::Claimed)
        );
        assert_eq!(hub(reg.deltas(), SYSTEM).map(|h| h.offline_days), Some(0));
        // Burnt out again, then left: it lapses after the grace days.
        {
            let dd = day(&reg, 1);
            on_rollover(&mut reg, &dd)
        };
        let mut offline = 0;
        while hub(reg.deltas(), SYSTEM).is_some() {
            {
                let dd = day(&reg, 1);
                on_rollover(&mut reg, &dd)
            };
            offline += 1;
            assert!(offline <= GRACE_DAYS + 1, "it never lapsed");
        }
        // The first of these runs out (offline), then GRACE_DAYS full days.
        assert_eq!(offline, GRACE_DAYS + 1);
        assert_eq!(sov::claim(reg.deltas(), SYSTEM), None);
        assert_eq!(sov::holder(reg.deltas(), SYSTEM), None);
        assert!(reg.store().get(id).is_none(), "the hub item is gone");
        assert!(fuel_stacks(&reg, id).is_empty());
        let log = sov::recent_activity(reg.deltas(), crate::pyutil::filetime_now());
        assert_eq!(
            log.last().map(|x| (x.owner, x.old_owner)),
            Some((None, Some(a)))
        );
        let sent = due(&mut reg);
        for name in ["ProcessSovStatusChanged", "OnSovereigntyChanged"] {
            assert!(sent.iter().any(|(n, _, _)| *n == name), "{name}: {sent:?}");
        }
        assert!(
            sent.iter()
                .any(|(n, a, _)| *n == "DoDestinyUpdate" && text(&a[0]).contains("RemoveBalls")),
            "the ball leaves every park"
        );
        // The system is open for a new hub.
        {
            let at = launch_spot(&reg);
            place_at(&mut reg, ALICE, at)
        };
        assert!(launch_one(&mut reg, ALICE, HUB_PAIR).is_ok());
    }

    #[test]
    fn a_catch_up_rollover_burns_every_owed_day() {
        let (mut reg, a, _) = world();
        let id = launched(&mut reg);
        {
            let p = owner(&reg, a);
            capture(&mut reg, SYSTEM, p)
        }
        .expect("captured");
        {
            let at = fuel_spot(&reg);
            place_at(&mut reg, ALICE, at)
        };
        refuel(&mut reg, ALICE, None).expect("fuelled");
        let now = crate::pyutil::filetime_now();
        let burned = on_rollover(
            &mut reg,
            &crate::rollover::Day {
                day: 9,
                days: 3,
                now,
            },
        );
        assert_eq!(burned, usize::try_from(3 * FUEL_PER_DAY).unwrap());
        assert_eq!(fuel_stock(&reg, id), ALICE_BLOCKS - 3 * FUEL_PER_DAY);
        // The fuel left the economy as destroyed goods, and nothing minted.
        let goods = crate::econ_ledger::goods_today(&reg);
        assert_eq!(goods.total(), (0, 3 * FUEL_PER_DAY), "{goods:?}");
    }

    #[test]
    fn a_hub_its_fuel_and_its_claim_survive_the_world_file() {
        let (mut reg, a, _) = world();
        let id = launched(&mut reg);
        {
            let p = owner(&reg, a);
            capture(&mut reg, SYSTEM, p)
        }
        .expect("captured");
        {
            let at = fuel_spot(&reg);
            place_at(&mut reg, ALICE, at)
        };
        refuel(&mut reg, ALICE, Some(50)).expect("fuelled");
        let dir = std::env::temp_dir().join(format!(
            "eve-sov-hub-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("a directory to write in");
        let store = crate::store::FileStore::new(dir.join("world.txt"));
        crate::store::Store::save(&store, &reg.to_snapshot()).expect("write the world");
        let snap = crate::store::Store::load(&store)
            .expect("read the world")
            .expect("a world was written");
        let _ = std::fs::remove_dir_all(&dir);
        let (mut fresh, _, _) = world();
        fresh.apply_snapshot(snap);
        assert_eq!(hub(fresh.deltas(), SYSTEM), hub(reg.deltas(), SYSTEM));
        assert_eq!(sov::holder(fresh.deltas(), SYSTEM), Some(a));
        assert_eq!(
            sov::claim(fresh.deltas(), SYSTEM).map(|c| c.hub_id),
            Some(id)
        );
        assert_eq!(fuel_stock(&fresh, id), 50);
        assert!(
            crate::destiny::scene(&fresh, SYSTEM, ship(ALICE), None, 1)
                .park
                .balls
                .iter()
                .any(|b| b.ball_id == id),
            "the ball is back"
        );
    }

    fn roll(reg: &mut Registry, days: u32) {
        let dd = crate::rollover::Day {
            day: reg.world().day + 1,
            days,
            now: crate::pyutil::filetime_now(),
        };
        on_rollover(reg, &dd);
    }

    /// A captured hub holding exactly `days` days of fuel.
    fn fuelled_for(reg: &mut Registry, a: i64, days: i64) -> i64 {
        let id = launched(reg);
        let pair = owner(reg, a);
        capture(reg, SYSTEM, pair).expect("captured");
        let at = fuel_spot(reg);
        place_at(reg, ALICE, at);
        refuel(reg, ALICE, Some(days * FUEL_PER_DAY)).expect("fuelled");
        id
    }

    #[test]
    fn the_claim_lapses_grace_days_after_the_hub_went_offline() {
        let (mut reg, a, _) = world();
        fuelled_for(&mut reg, a, 1);
        roll(&mut reg, 1); // paid
        roll(&mut reg, 1); // runs out: offline, day zero
        assert!(hub(reg.deltas(), SYSTEM).is_some());
        for _ in 1..GRACE_DAYS {
            roll(&mut reg, 1);
            assert!(hub(reg.deltas(), SYSTEM).is_some(), "lapsed early");
        }
        roll(&mut reg, 1);
        assert!(hub(reg.deltas(), SYSTEM).is_none(), "GRACE_DAYS full days");
    }

    #[test]
    fn a_hub_that_runs_out_in_a_catch_up_gets_a_live_rollover_to_refuel() {
        let (mut reg, a, _) = world();
        fuelled_for(&mut reg, a, 1);
        roll(&mut reg, GRACE_DAYS + 5);
        assert!(hub(reg.deltas(), SYSTEM).is_some(), "lapsed on the spot");
        assert_eq!(
            sov::claim(reg.deltas(), SYSTEM).map(|c| c.state),
            Some(HubState::Offline)
        );
        roll(&mut reg, GRACE_DAYS);
        assert!(hub(reg.deltas(), SYSTEM).is_none());
    }

    #[test]
    fn a_neutral_hub_is_removed_after_neutral_hub_days_and_frees_the_system() {
        let (mut reg, _, _) = world();
        let id = launched(&mut reg);
        for _ in 1..NEUTRAL_HUB_DAYS {
            roll(&mut reg, 1);
            assert!(hub(reg.deltas(), SYSTEM).is_some(), "removed early");
        }
        roll(&mut reg, 1);
        assert!(hub(reg.deltas(), SYSTEM).is_none());
        assert!(sov::claim(reg.deltas(), SYSTEM).is_none());
        assert!(reg.store().get(id).is_none(), "the item is destroyed");
    }

    #[test]
    fn a_captured_hub_is_not_removed_as_neutral() {
        let (mut reg, a, _) = world();
        launched(&mut reg);
        roll(&mut reg, NEUTRAL_HUB_DAYS - 1);
        let pair = owner(&reg, a);
        capture(&mut reg, SYSTEM, pair).expect("captured");
        roll(&mut reg, 1);
        assert!(hub(reg.deltas(), SYSTEM).is_some());
    }

    #[test]
    fn the_fuel_step_runs_before_the_ledger_folds_the_day() {
        let names: Vec<&str> = crate::rollover_steps::STEPS
            .iter()
            .map(|s| s.name)
            .collect();
        let fuel = names
            .iter()
            .position(|n| *n == "sov_fuel")
            .expect("registered");
        let ledger = names
            .iter()
            .position(|n| *n == "econ_ledger")
            .expect("the ledger");
        assert!(fuel < ledger, "{names:?}");
        assert!(crate::delta_kinds::is_registered(KIND));
    }
}
