//! **Where a bot's hull numbers come from** (FX10).
//!
//! CP58b retired the baked hull table: with no typedata installed,
//! `destiny::hull::find` answers from the **invented** `hull::FIXTURE`, and
//! the bot process installs none. So a bot reading `hull::find` for its lock
//! range was reading made-up numbers (`docs/lanes/cp58b.md`, "Risk for the
//! test round"). This module is the bot's answer, in order of trust:
//!
//! 1. **The server's word.** The ship's dogma row (`dogmaIM.ItemGetInfo`
//!    on the ship, `service.rs::capsule_dogma_row`) carries attribute 76,
//!    `maxTargetRange`, exactly as the client is told it: fitted, and with
//!    the server's pod floor (`POD_TARGETING`) applied. The server's lock
//!    sweep (`Registry::max_target_range_of`) enforces the same number, so
//!    a bot that reads it cannot be refused for range by a disagreement.
//! 2. **The bundle** (`eve-bots --content DIR`, [`open`]): the client's own
//!    static data, which a player's client also holds [I: every row of
//!    `types/typedata.tsv` is client data], through
//!    `destiny::hull::find_in`, as the server reads it.
//! 3. **The invented fixture**, with no bundle: today's behaviour, and the
//!    binary says so once at startup ([`NO_CONTENT`]).
//!
//! The park (`park.rs`) needs none of this: every ball moves with its own
//! record's numbers from the destiny blob, the server's truth (FX6, FX10).

use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::destiny::hull::{self, Hull};
use eve_server::industry;
use eve_server::typedata::Installed;
use std::path::Path;

/// The Badger **[V-data]** (type 648, group 28, Caldari industrial, cargo
/// 3,900 m3, two high slots): the PI farmer's hull ([`crate::pi::BADGER`]).
/// (The ice miner no longer flies it: ICE-HULL, `docs/lanes/ice-hull.md`.)
pub const BADGER: i64 = 648;

/// Printed once by `eve-bots` when it runs without `--content`.
pub const NO_CONTENT: &str = "eve-bots: no --content: hull numbers are INVENTED \
    (destiny::hull::FIXTURE); a bot whose ship's attribute 76 cannot be read \
    locks against made-up ranges";

/// Where a number came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The ship's dogma row, as the server serves it to the client.
    Server,
    /// The bundle's typedata (`--content`).
    Bundle,
    /// The invented fixture: no bundle.
    Invented,
    /// Nothing knows this ship (or no ship): no lock range.
    Unknown,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Source::Server => "server",
            Source::Bundle => "bundle",
            Source::Invented => "invented",
            Source::Unknown => "unknown",
        })
    }
}

/// `--content DIR`: every table the server would load from that bundle
/// (`eve_server::content::load`, authored rows compiled in), so
/// `hull::find_in` answers exactly as it does on the server. Errors name the
/// directory.
pub fn open(dir: &Path) -> Result<Content, String> {
    eve_server::content::load(Some(dir), &eve_server::authored::Dir::compiled_only())
        .map(|l| l.content)
        .map_err(|e| format!("--content {}: {e}", dir.display()))
}

/// This hull's numbers: the bundle's through `hull::find_in` when `content`
/// is given, else [`invented`]. `None` for a type neither knows.
pub fn find(content: Option<&Content>, type_id: i64) -> Option<(Hull, Source)> {
    match content {
        Some(c) => hull::find_in(c, type_id).map(|h| {
            let from = if c.is_bundle() {
                Source::Bundle
            } else {
                Source::Invented
            };
            (*h, from)
        }),
        None => invented(type_id).map(|h| (*h, Source::Invented)),
    }
}

/// **With no `--content`**: what `hull::find_in` answers on a content with
/// no bundle and no typedata rows (X2a; before it, the process-wide
/// `hull::find` with nothing installed, which is the same): the hand NPC
/// table, the drone table, then the invented `hull::FIXTURE`.
fn invented(type_id: i64) -> Option<&'static Hull> {
    eve_server::npc::hull(type_id)
        .or_else(|| eve_server::drones::hull(type_id))
        .or_else(|| hull::FIXTURE.iter().find(|h| h.type_id == type_id))
}

/// **The ship's lock range**, metres, and where it came from: the server's
/// attribute 76 when the bot read it, else [`find`]'s `max_target_range`,
/// else `0` (no lock: the brain then locks nothing it cannot see is near).
pub fn lock_range(
    server: Option<f64>,
    content: Option<&Content>,
    type_id: Option<i64>,
) -> (f64, Source) {
    if let Some(r) = server.filter(|r| *r > 0.0) {
        return (r, Source::Server);
    }
    type_id
        .and_then(|t| find(content, t))
        .map_or((0.0, Source::Unknown), |(h, from)| {
            (h.max_target_range, from)
        })
}

/// **The pushes that move one of this ship's own attributes**, as
/// `(attribute, new value)`: `OnModuleAttributeChange(owner, item, attribute,
/// time, new, old, ..)` and its batch `OnModuleAttributeChanges([(name,
/// owner, item, attribute, time, new, old, ..)])`. A push about another item
/// is not ours. The server sends them when a system's effect beacon, a
/// booster or a fitted module moves the ship's lock range or lock count, so a
/// bot reads attribute 76 once per ship and again on every push.
pub fn ship_attribute_changes(event: &str, args: &[PyValue], ship: i64) -> Vec<(i64, f64)> {
    let one = |t: &[PyValue], at: usize| match (t.get(at), t.get(at + 1), crate::real_at(t, at + 3))
    {
        (Some(PyValue::Int(item)), Some(PyValue::Int(a)), Some(new)) if *item == ship => {
            Some((*a, new))
        }
        _ => None,
    };
    match event {
        "OnModuleAttributeChange" => one(args, 1).into_iter().collect(),
        "OnModuleAttributeChanges" => match args.first() {
            Some(PyValue::List(changes)) => changes
                .iter()
                .filter_map(|c| match c {
                    PyValue::Tuple(t) => one(t, 2),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

// ---- the capital build classes (CAP-C) ------------------------------------------

/// `groupCapitalConstructionComponents` **[V-data]** (group 873): what a T1
/// capital hull is built from, plus the three Neurolink cells.
pub const GROUP_CAPITAL_COMPONENT: i64 = 873;
/// Advanced Capital Construction Components **[V-data]** (group 913): what a
/// T2 capital adds to its T1 hull.
pub const GROUP_ADVANCED_COMPONENT: i64 = 913;
/// The capital hull intermediates **[V-data]** (group 334): the Neurolink
/// conduits, Auto-Integrity Preservation Seals, Life Support Backup Units;
/// every T1 capital hull takes a few of each (R-CAP §1.2).
pub const GROUP_INTERMEDIATE: i64 = 334;
/// Freighters **[V-data]** (group 513): industrial class, not capital
/// (`typedata::CAPITAL_GROUPS`), but built from the same components, and the
/// cheapest hull on the capital ladder.
pub const GROUP_FREIGHTER: i64 = 513;
/// Sovereignty Hubs **[V-data]** (group 1012,
/// `eve_server::sov_hub::GROUP_INFRASTRUCTURE_HUB`): built from capital
/// components and PI P4 goods (D-HUB).
pub const GROUP_SOV_HUB: i64 = eve_server::sov_hub::GROUP_INFRASTRUCTURE_HUB;
/// Composite reaction products **[V-data]** (group 429): Reinforced Carbon
/// Fiber, Tungsten Carbide, Fullerides, ... what a reactor makes from moon
/// materials.
pub const GROUP_COMPOSITE: i64 = 429;
/// Hybrid polymers **[V-data]** (group 974): a reactor's output too.
pub const GROUP_POLYMER: i64 = 974;
/// The neurolink reaction products **[V-data]** (group 4096).
pub const GROUP_NEUROLINK_REACTION: i64 = 4096;
/// Zero-Point Condensate's group **[V-data]** (1996): no source anywhere
/// (R-CAP U-CAP-5); the components that take it are skipped.
pub const GROUP_ABYSSAL: i64 = 1996;
/// `groupCommodities` **[V-data]** (526): the eleven Neurolink commodities
/// are in it, and they come from site loot (lane CAP-LOOT).
pub const GROUP_COMMODITY: i64 = 526;
/// Planetary commodities by tier **[V-data]**: P1 (1042), P2 (1034), P3
/// (1040), P4 (1041). `pi::P1_TYPES` is the farmer's basket of the first.
pub const GROUP_P1: i64 = 1042;
pub const GROUP_P2: i64 = 1034;
pub const GROUP_P3: i64 = 1040;
pub const GROUP_P4: i64 = 1041;

/// Hybrid Tech Components **[V-data]** (group 964): what a Tech 3 hull, a
/// subsystem or a tactical destroyer is built from (T3-F). Their blueprints
/// are NPC-sold originals, so a builder climbs to them like any component.
pub const GROUP_HYBRID_COMPONENT: i64 = 964;
/// Ancient Salvage **[V-data]** (group 966): the other half of a Hybrid
/// Tech Component's recipe. Only ancient-site cans drop it
/// (`content_explore.py`, class `ancient_salvage`), so it is the explorers'
/// loot like the Neurolink commodities.
pub const GROUP_ANCIENT_SALVAGE: i64 = 966;
/// What a type is, for the one question "does any career make it": a
/// builder's input classes (CAP-C).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Input {
    /// A mineral: `supply::Supply` rules on it, per system.
    Mineral,
    /// PI P1 or P2: the farmers' goods.
    PiBasic,
    PiP3,
    PiP4,
    /// A reaction product (composite, polymer, neurolink): a reactor's.
    Reaction,
    /// A commodity that only site loot yields.
    Loot,
    /// A capital component, intermediate or advanced component: a builder
    /// makes it from its own recipe.
    Built,
    /// Zero-Point Condensate: nothing makes it.
    Abyssal,
    /// Anything else (ice products, gas, datacores, ...): not gated.
    Other,
}

/// A type's group: the price table's (it covers planetary goods, which the
/// type data does not), else the type data's.
pub fn group_of(content: &Content, type_id: i64) -> Option<i64> {
    content
        .try_get::<eve_server::econ_prices::Table>()
        .and_then(|t| t.prices.get(&type_id))
        .map(|p| p.group_id)
        .or_else(|| {
            content
                .try_get::<Installed>()
                .and_then(|t| t.item(type_id))
                .map(|t| t.group_id)
        })
}

/// The class of the input `type_id`, by its group.
pub fn input_of(content: &Content, type_id: i64) -> Input {
    match group_of(content, type_id) {
        Some(crate::builder::GROUP_MINERAL) => Input::Mineral,
        Some(GROUP_P1 | GROUP_P2) => Input::PiBasic,
        Some(GROUP_P3) => Input::PiP3,
        Some(GROUP_P4) => Input::PiP4,
        Some(GROUP_COMPOSITE | GROUP_POLYMER | GROUP_NEUROLINK_REACTION) => Input::Reaction,
        Some(GROUP_COMMODITY | GROUP_ANCIENT_SALVAGE) => Input::Loot,
        Some(
            GROUP_CAPITAL_COMPONENT
            | GROUP_ADVANCED_COMPONENT
            | GROUP_INTERMEDIATE
            | GROUP_HYBRID_COMPONENT,
        ) => Input::Built,
        Some(GROUP_ABYSSAL) => Input::Abyssal,
        _ => Input::Other,
    }
}

/// Whether `type_id` is a hull the capital ladder builds: a capital ship or
/// a freighter, by its group in the type data.
pub fn is_capital_build_hull(content: &Content, type_id: i64) -> bool {
    content
        .try_get::<Installed>()
        .and_then(|t| t.item(type_id))
        .is_some_and(|t| {
            t.category_id == eve_server::typedata::CATEGORY_SHIP
                && (eve_server::typedata::is_capital_group(t.group_id)
                    || t.group_id == GROUP_FREIGHTER)
        })
}

/// **The capital rungs** (CAP-C), after the T1 ladder: from the recipes, by
/// what each blueprint makes, no id in the source. Each rung is a set of
/// interchangeable originals; a builder holds one (a builder specialises, and
/// the market joins them: a hull builder's standing bids are the
/// `promote_demanded` signal a component builder builds by).
///
/// 1. capital components (group 873);
/// 2. the hull intermediates (group 334);
/// 3. **T1** capital hulls and freighters: a hull whose recipe holds another
///    capital hull is a T2 one (Lancer, Command Carrier, Jump Freighter),
///    made from a copy, not bought;
/// 4. advanced components (group 913);
/// 5. **Sovereignty Hubs** (group 1012, D-HUB), only when the item store can
///    create one (`item::find_in`): the shipped type data lacked the hub's
///    row until D-HUB's extractor fix, and a job for an unknown product is
///    refused by the server, so a rung for it would be a rung nobody could
///    build.
///
/// Empty rungs are left out. Whether a blueprint's inputs can be got is
/// `builder::Sourcing`'s question, not this one's.
pub fn capital_rungs(content: &Content, recipes: &industry::Table) -> Vec<Vec<i64>> {
    let mut rungs: [Vec<i64>; 5] = Default::default();
    for bp in &recipes.blueprints {
        let product = bp.product_type_id;
        let at = match group_of(content, product) {
            Some(GROUP_CAPITAL_COMPONENT) => 0,
            Some(GROUP_INTERMEDIATE) => 1,
            Some(GROUP_ADVANCED_COMPONENT) => 3,
            Some(GROUP_SOV_HUB) if eve_server::item::find_in(content, product).is_some() => 4,
            _ if is_capital_build_hull(content, product)
                && !bp
                    .materials
                    .iter()
                    .any(|&(m, _)| is_capital_build_hull(content, m)) =>
            {
                2
            }
            _ => continue,
        };
        rungs[at].push(bp.blueprint_type_id);
    }
    rungs
        .into_iter()
        .map(|mut r| {
            r.sort_unstable();
            r.dedup();
            r
        })
        .filter(|r| !r.is_empty())
        .collect()
}

/// **The Tech 3 rungs** (T3-F), after the T1 ladder *and* the capital rungs
/// ([`crate::builder::ladder_with`]): the one thing a builder can buy toward a
/// Tech 3 build is the originals of the Hybrid Tech Components (group 964),
/// found in the recipes like [`capital_rungs`], no id in the source. The hull,
/// subsystem and tactical destroyer blueprints are **not** a rung: no NPC
/// corporation sells them (nothing from nowhere); a builder gets them as
/// copies from a relic's invention and builds them from there
/// ([`crate::builder::t2_candidates`], last). Empty when no recipe makes a
/// Hybrid Tech Component.
pub fn t3_rungs(content: &Content, recipes: &industry::Table) -> Vec<Vec<i64>> {
    let mut rung: Vec<i64> = recipes
        .blueprints
        .iter()
        .filter(|bp| group_of(content, bp.product_type_id) == Some(GROUP_HYBRID_COMPONENT))
        .map(|bp| bp.blueprint_type_id)
        .collect();
    rung.sort_unstable();
    rung.dedup();
    if rung.is_empty() {
        Vec::new()
    } else {
        vec![rung]
    }
}

/// **The warship rungs** (PROG-PVE), after the T1 ladder and before the
/// capital ones: the T1 battlecruiser blueprints, then the T1 battleship
/// ones, found in the recipes by what each makes -- a product that is a
/// battlecruiser or battleship of [`crate::hull_choice::HULLS`], the hulls
/// the combat bots know how to fly and fit. Each rung is a set of
/// interchangeable originals, as the T1 ones are (a race's blueprints are
/// sold in that race's space). Empty rungs are left out.
///
/// The bots' demand for these hulls reaches the builders the way a cruiser's
/// does: `hull_choice::want_bid`'s standing bid and
/// `builder::promote_demanded`. **[A]** that a station within reach sells
/// the blueprints (the E5 exchange's trade lists); falsified by a soak whose
/// builders report no seller for these rungs, after which the bots buy no
/// battlecruiser and stay cruisers.
pub fn warship_rungs(recipes: &industry::Table) -> Vec<Vec<i64>> {
    use crate::hull_choice::{Class, HULLS};
    let mut rungs: [Vec<i64>; 2] = Default::default();
    for bp in &recipes.blueprints {
        let at = match HULLS.iter().find(|h| h.type_id == bp.product_type_id) {
            Some(h) if h.class == Class::Battlecruiser => 0,
            Some(h) if h.class == Class::Battleship => 1,
            _ => continue,
        };
        rungs[at].push(bp.blueprint_type_id);
    }
    rungs
        .into_iter()
        .map(|mut r| {
            r.sort_unstable();
            r.dedup();
            r
        })
        .filter(|r| !r.is_empty())
        .collect()
}

// ---- the mining ladder's rungs (PROG-MINE) -----------------------------------------

/// Strip miners **[V-data]** (group 464): the barge's and the exhumer's
/// laser.
pub const GROUP_STRIP_MINER: i64 = 464;
/// Mining barges (group 463), exhumers (543) and industrial command ships
/// (941: the Orca, the Porpoise) **[V-data]**.
pub const GROUP_BARGE: i64 = 463;
pub const GROUP_EXHUMER: i64 = 543;
pub const GROUP_COMMAND_SHIP: i64 = 941;

/// **The mining rungs** (PROG-MINE), found by what a blueprint makes like
/// [`capital_rungs`] and chained *before* them (a barge is tens of millions
/// of ISK, a freighter hundreds): the Tech I strip miners; the Tech I
/// mining barges; the industrial command ships. A recipe that takes another
/// hull of the class as an input is a Tech II one -- an exhumer takes its
/// barge -- and is **invented from a copy**, not bought: the builder's lab
/// ([`crate::builder::t2_candidates`]) makes it once it holds a barge
/// original, so the rung beneath it is what puts an exhumer in the world.
/// Empty rungs are left out. Whether a blueprint's inputs can be got is
/// [`crate::builder::Sourcing`]'s question; whether an original is on sale
/// within reach is the exchange's (ORE sells the barges' far from the
/// schools, `docs/lanes/prog-mine.md`).
pub fn mining_rungs(content: &Content, recipes: &industry::Table) -> Vec<Vec<i64>> {
    let mut rungs: [Vec<i64>; 3] = Default::default();
    for bp in &recipes.blueprints {
        let group = group_of(content, bp.product_type_id);
        let at = match group {
            Some(GROUP_STRIP_MINER) => 0,
            Some(GROUP_BARGE) => 1,
            Some(GROUP_COMMAND_SHIP) => 2,
            _ => continue,
        };
        let takes_a_peer = bp.materials.iter().any(|&(m, _)| {
            matches!(
                group_of(content, m),
                Some(GROUP_STRIP_MINER | GROUP_BARGE | GROUP_EXHUMER | GROUP_COMMAND_SHIP)
            )
        });
        if !takes_a_peer {
            rungs[at].push(bp.blueprint_type_id);
        }
    }
    rungs
        .into_iter()
        .map(|mut r| {
            r.sort_unstable();
            r.dedup();
            r
        })
        .filter(|r| !r.is_empty())
        .collect()
}

/// **The rungs of the weapons and the tank** (PROG-PVE2): for each weapon
/// system the bots fire ([`crate::fit_plan::products_of`]: its three guns and
/// every T1 round of its groups) and for the tank ([`crate::tank::products`]),
/// one rung of the blueprints whose product is one of them, found in the
/// recipes by what they make; no blueprint id is in the source. Empty rungs
/// are left out. They go after the hull rungs, so a builder that has climbed
/// to a cruiser makes the guns and rounds the cruiser's pilots need.
///
/// **[A]** that a station within reach sells the blueprints (the E5
/// exchange's trade lists); falsified by a soak whose builders report no
/// seller for these rungs, after which hybrid, laser and missile bots buy
/// nothing a builder made and live on what their kills drop.
pub fn arms_rungs(recipes: &industry::Table) -> Vec<Vec<i64>> {
    use crate::hull_choice::Weapon;
    let groups: [Vec<i64>; 5] = [
        crate::fit_plan::products_of(Weapon::Projectile),
        crate::fit_plan::products_of(Weapon::Hybrid),
        crate::fit_plan::products_of(Weapon::Laser),
        crate::fit_plan::products_of(Weapon::Missile),
        crate::tank::products(),
    ];
    groups
        .iter()
        .map(|products| {
            let mut r: Vec<i64> = recipes
                .blueprints
                .iter()
                .filter(|bp| products.contains(&bp.product_type_id))
                .map(|bp| bp.blueprint_type_id)
                .collect();
            r.sort_unstable();
            r.dedup();
            r
        })
        .filter(|r| !r.is_empty())
        .collect()
}

/// DOC-7: the rungs of the parts the leaders' doctrines want
/// ([`crate::doctrine_parts::doctrine_rungs`]), found in the recipes by what
/// each blueprint makes, like [`warship_rungs`].
pub use crate::doctrine_parts::doctrine_rungs;

/// **An invented capital world** for the ladder and sourcing tests (here
/// and in `builder`): types in the client's own groups, ids in a band no
/// table knows. Returns the content (type data and prices) and the recipes.
#[cfg(test)]
pub(crate) mod world {
    use super::*;
    use eve_server::typedata::{Installed, Table};

    pub const MINERAL: i64 = 990_710_001;
    pub const P2: i64 = 990_710_002;
    pub const P4: i64 = 990_710_003;
    pub const COMPOSITE: i64 = 990_710_004;
    pub const LOOT: i64 = 990_710_005;
    pub const ZERO_POINT: i64 = 990_710_006;
    /// Products.
    pub const COMPONENT_OK: i64 = 990_710_101;
    pub const COMPONENT_P4: i64 = 990_710_102;
    pub const COMPONENT_RCF: i64 = 990_710_103;
    pub const CONDUIT: i64 = 990_710_104;
    pub const ADVANCED: i64 = 990_710_105;
    pub const CARRIER: i64 = 990_710_201;
    pub const FREIGHTER: i64 = 990_710_202;
    pub const LANCER: i64 = 990_710_203;
    pub const BATTLESHIP: i64 = 990_710_204;
    pub const GHOST_PART: i64 = 990_710_205;
    /// A Sovereignty Hub the item store knows, and one it does not (the
    /// shipped bundle's case before the extractor fix).
    pub const HUB: i64 = 990_710_206;
    pub const HUB_UNKNOWN: i64 = 990_710_207;
    /// Blueprints (`product + 1_000`).
    pub const fn bp(product: i64) -> i64 {
        product + 1_000
    }

    fn row(id: i64, group: i64, category: i64, name: &str) -> String {
        format!(
            "type\t{id}\t{group}\t{category}\t1\t1\t1000.0\t0.0\t1000.0\t10.0\t0\t\t37=1.0\t{name}\n"
        )
    }

    /// `(content, recipes)`: a hull needs the OK component and the conduit;
    /// a second carrier needs a component that takes a P4 good, a third a
    /// component that takes a composite; a freighter takes only the OK
    /// component; a Lancer takes the carrier; a battleship is no capital.
    pub fn capital() -> (Content, industry::Table) {
        capital_world(false)
    }

    /// [`capital`], and with two hub recipes (a hub takes the OK component
    /// and a P4 good): [`HUB`] is in the item store, [`HUB_UNKNOWN`] is not.
    pub fn hub_world() -> (Content, industry::Table) {
        capital_world(true)
    }

    fn capital_world(hubs: bool) -> (Content, industry::Table) {
        let mut text = String::new();
        if hubs {
            text.push_str(&row(HUB, GROUP_SOV_HUB, 40, "Hub"));
        }
        for (id, group, category, name) in [
            (COMPONENT_OK, GROUP_CAPITAL_COMPONENT, 17, "Component OK"),
            (COMPONENT_P4, GROUP_CAPITAL_COMPONENT, 17, "Component P4"),
            (COMPONENT_RCF, GROUP_CAPITAL_COMPONENT, 17, "Component RCF"),
            (CONDUIT, GROUP_INTERMEDIATE, 17, "Conduit"),
            (ADVANCED, GROUP_ADVANCED_COMPONENT, 17, "Advanced"),
            (CARRIER, 547, 6, "Carrier"),
            (FREIGHTER, GROUP_FREIGHTER, 6, "Freighter"),
            (LANCER, 4594, 6, "Lancer"),
            (BATTLESHIP, 27, 6, "Battleship"),
            (
                GHOST_PART,
                GROUP_CAPITAL_COMPONENT,
                17,
                "Part nobody recipes",
            ),
        ] {
            text.push_str(&row(id, group, category, name));
        }
        let mut c = Content::fixture();
        c.insert(
            "typedata",
            Installed::build(Table::parse(&text).expect("the invented rows parse")),
        );
        let prices: Vec<String> = [
            (HUB, GROUP_SOV_HUB, 40),
            (HUB_UNKNOWN, GROUP_SOV_HUB, 40),
            (MINERAL, crate::builder::GROUP_MINERAL, 4),
            (P2, GROUP_P2, 43),
            (P4, GROUP_P4, 43),
            (COMPOSITE, GROUP_COMPOSITE, 4),
            (LOOT, GROUP_COMMODITY, 17),
            (ZERO_POINT, GROUP_ABYSSAL, 4),
        ]
        .iter()
        .map(|(id, group, cat)| format!("{id}\t100.0\tbase\t1.0\t{cat}\t{group}\t\t1"))
        .collect();
        c.insert(
            "econ_prices",
            eve_server::econ_prices::Table::parse(&prices).expect("prices parse"),
        );
        let recipe = |product: i64, time: i64, materials: &[i64]| -> Vec<String> {
            let b = bp(product);
            let mut v = vec![format!("{b}\t1\tproduct\t{product}\t1\t\t{time}\t1")];
            v.extend(
                materials
                    .iter()
                    .map(|m| format!("{b}\t1\tmaterial\t{m}\t10\t\t{time}\t1")),
            );
            v
        };
        let mut hub_rows = Vec::new();
        if hubs {
            hub_rows.extend(recipe(HUB, 120_000, &[COMPONENT_OK, P4]));
            hub_rows.extend(recipe(HUB_UNKNOWN, 120_000, &[COMPONENT_OK, P4]));
        }
        let rows: Vec<String> = [
            hub_rows,
            recipe(COMPONENT_OK, 16_000, &[MINERAL, P2]),
            recipe(COMPONENT_P4, 16_000, &[MINERAL, P4]),
            recipe(COMPONENT_RCF, 16_000, &[MINERAL, COMPOSITE]),
            recipe(CONDUIT, 1_800, &[P2, LOOT]),
            recipe(ADVANCED, 750, &[COMPOSITE]),
            recipe(CARRIER, 1_800_000, &[COMPONENT_OK, CONDUIT]),
            recipe(FREIGHTER, 1_800_000, &[COMPONENT_OK]),
            recipe(LANCER, 1_800_000, &[CARRIER, ADVANCED]),
            recipe(BATTLESHIP, 10_000, &[MINERAL]),
        ]
        .concat();
        let recipes = industry::Table::parse(&rows, &eve_server::econ_prices::Table::default())
            .expect("the recipes parse");
        (c, recipes)
    }
}

#[cfg(test)]
mod tests {
    //! Invented rows only: a typedata written here, its numbers chosen to
    //! differ from every `hull::FIXTURE` row.
    use super::*;
    use eve_server::typedata::{Installed, Table};

    /// An invented ship type, in no fixture.
    const SHIP: i64 = 990_777_001;
    /// An invented range, unlike any fixture row's.
    const RANGE: f64 = 12_345.0;

    fn content() -> Content {
        let text = format!(
            "type\t{SHIP}\t25\t6\t1\t1\t20000.0\t77.0\t1500000.0\t33.0\t0\t\t\
             37=210.0;70=2.5;76={RANGE};192=4.0;564=500.0\tInvented Hull\n"
        );
        let mut c = Content::fixture();
        c.insert(
            "typedata",
            Installed::build(Table::parse(&text).expect("the invented row parses")),
        );
        c
    }

    #[test]
    fn with_content_a_hull_reads_its_rows_numbers() {
        let c = content();
        let (h, _) = find(Some(&c), SHIP).expect("the row is a hull");
        assert_eq!(h.max_target_range, RANGE);
        assert_eq!((h.radius, h.max_velocity, h.agility), (33.0, 210.0, 2.5));
        assert_eq!(lock_range(None, Some(&c), Some(SHIP)).0, RANGE);
        assert!(
            hull::FIXTURE.iter().all(|f| f.max_target_range != RANGE),
            "the test's number is not a fixture's"
        );
    }

    #[test]
    fn without_content_the_fixture_answers_and_says_it_is_invented() {
        let f = hull::FIXTURE[0];
        assert_eq!(
            lock_range(None, None, Some(f.type_id)),
            (f.max_target_range, Source::Invented)
        );
        assert_eq!(lock_range(None, None, Some(SHIP)), (0.0, Source::Unknown));
        assert!(NO_CONTENT.contains("INVENTED"));
        assert_eq!(Source::Invented.to_string(), "invented");
    }

    #[test]
    fn the_servers_attribute_wins_over_any_table() {
        let c = content();
        let served = RANGE * 2.0;
        assert_eq!(
            lock_range(Some(served), Some(&c), Some(SHIP)),
            (served, Source::Server)
        );
        // A zero is the attribute's default, not a range (`hull.rs`, 76).
        assert_eq!(lock_range(Some(0.0), Some(&c), Some(SHIP)).0, RANGE);
    }

    #[test]
    fn opening_a_missing_bundle_names_the_directory() {
        let e = open(Path::new("/nonexistent/fx10-bundle")).expect_err("no bundle there");
        assert!(e.contains("/nonexistent/fx10-bundle"), "{e}");
    }

    // ---- CAP-C: the capital rungs and the input classes ----------------------

    use super::world::*;

    #[test]
    fn the_capital_rungs_are_found_by_what_a_blueprint_makes() {
        let (c, recipes) = capital();
        let rungs = capital_rungs(&c, &recipes);
        let ok = |p: i64| bp(p);
        assert_eq!(
            rungs,
            vec![
                vec![ok(COMPONENT_OK), ok(COMPONENT_P4), ok(COMPONENT_RCF)],
                vec![ok(CONDUIT)],
                vec![ok(CARRIER), ok(FREIGHTER)],
                vec![ok(ADVANCED)],
            ],
            "components, intermediates, T1 hulls, advanced components, in that order"
        );
        let flat: Vec<i64> = rungs.concat();
        assert!(!flat.contains(&ok(LANCER)), "a T2 hull is made from a copy");
        assert!(
            !flat.contains(&ok(BATTLESHIP)),
            "a battleship is no capital"
        );
    }

    #[test]
    fn a_hub_the_item_store_knows_is_the_last_capital_rung() {
        let (c, recipes) = hub_world();
        let rungs = capital_rungs(&c, &recipes);
        assert_eq!(
            rungs.last(),
            Some(&vec![bp(HUB)]),
            "the hub follows the advanced components"
        );
        assert_eq!(rungs.len(), 5);
        assert!(
            !rungs.concat().contains(&bp(HUB_UNKNOWN)),
            "a hub the server cannot create holds no rung"
        );
        assert!(
            !capital_rungs(&capital().0, &capital().1)
                .concat()
                .contains(&bp(HUB)),
            "no hub recipe, no hub rung"
        );
    }

    #[test]
    fn with_no_capital_recipes_there_are_no_capital_rungs() {
        let (c, _) = capital();
        let none =
            industry::Table::parse(&[] as &[&str], &eve_server::econ_prices::Table::default())
                .expect("an empty table parses");
        assert!(capital_rungs(&c, &none).is_empty());
        // No type data at all: nothing is a capital, and nothing panics.
        let (_, recipes) = capital();
        assert!(capital_rungs(&Content::fixture(), &recipes).is_empty());
    }

    // ---- PROG-MINE: the mining rungs ---------------------------------------------

    /// Invented types in the client's mining groups, and their recipes: a
    /// strip miner and a barge from minerals, an exhumer from its barge, a
    /// command ship from minerals, a hull that is none of them.
    mod mining {
        use super::*;

        pub const MINERAL: i64 = 990_720_001;
        pub const STRIP: i64 = 990_720_101;
        pub const T2_STRIP: i64 = 990_720_102;
        pub const BARGE: i64 = 990_720_201;
        pub const EXHUMER: i64 = 990_720_202;
        pub const COMMAND: i64 = 990_720_301;
        pub const FRIGATE: i64 = 990_720_401;

        pub fn world() -> (Content, industry::Table) {
            let row = |id: i64, group: i64, cat: i64| {
                format!(
                    "type\t{id}\t{group}\t{cat}\t1\t1\t1000.0\t0.0\t1000.0\t10.0\t0\t\t37=1.0\tInvented {id}\n"
                )
            };
            let mut text = String::new();
            for (id, group, cat) in [
                (STRIP, GROUP_STRIP_MINER, 7),
                (T2_STRIP, GROUP_STRIP_MINER, 7),
                (BARGE, GROUP_BARGE, 6),
                (EXHUMER, GROUP_EXHUMER, 6),
                (COMMAND, GROUP_COMMAND_SHIP, 6),
                (FRIGATE, 25, 6),
            ] {
                text.push_str(&row(id, group, cat));
            }
            let mut c = Content::fixture();
            c.insert(
                "typedata",
                eve_server::typedata::Installed::build(
                    eve_server::typedata::Table::parse(&text).expect("the rows parse"),
                ),
            );
            let recipe = |product: i64, materials: &[i64]| -> Vec<String> {
                let b = product + 1_000;
                let mut v = vec![format!("{b}\t1\tproduct\t{product}\t1\t\t600\t10")];
                v.extend(
                    materials
                        .iter()
                        .map(|m| format!("{b}\t1\tmaterial\t{m}\t10\t\t600\t10")),
                );
                v
            };
            let rows: Vec<String> = [
                recipe(STRIP, &[MINERAL]),
                recipe(T2_STRIP, &[STRIP]),
                recipe(BARGE, &[MINERAL]),
                recipe(EXHUMER, &[BARGE]),
                recipe(COMMAND, &[MINERAL]),
                recipe(FRIGATE, &[MINERAL]),
            ]
            .concat();
            let recipes = industry::Table::parse(&rows, &eve_server::econ_prices::Table::default())
                .expect("the recipes parse");
            (c, recipes)
        }
    }

    #[test]
    fn the_mining_rungs_are_strip_miners_then_barges_then_command_ships() {
        use mining::*;
        let (c, recipes) = world();
        let b = |p: i64| p + 1_000;
        assert_eq!(
            mining_rungs(&c, &recipes),
            vec![vec![b(STRIP)], vec![b(BARGE)], vec![b(COMMAND)]],
            "T1 only, cheapest first"
        );
        let flat = mining_rungs(&c, &recipes).concat();
        assert!(
            !flat.contains(&b(EXHUMER)) && !flat.contains(&b(T2_STRIP)),
            "a Tech II recipe takes its Tech I and is invented from a copy"
        );
        assert!(!flat.contains(&b(FRIGATE)), "a frigate is no mining hull");
        // No type data, no rungs; no recipes, no rungs.
        assert!(mining_rungs(&Content::fixture(), &recipes).is_empty());
        let none =
            industry::Table::parse(&[] as &[&str], &eve_server::econ_prices::Table::default())
                .expect("an empty table parses");
        assert!(mining_rungs(&c, &none).is_empty());
    }

    #[test]
    fn an_input_is_classed_by_its_group() {
        let (c, _) = capital();
        assert_eq!(input_of(&c, MINERAL), Input::Mineral);
        assert_eq!(input_of(&c, P2), Input::PiBasic);
        assert_eq!(input_of(&c, P4), Input::PiP4);
        assert_eq!(input_of(&c, COMPOSITE), Input::Reaction);
        assert_eq!(input_of(&c, LOOT), Input::Loot);
        assert_eq!(input_of(&c, ZERO_POINT), Input::Abyssal);
        for built in [COMPONENT_OK, CONDUIT, ADVANCED] {
            assert_eq!(input_of(&c, built), Input::Built);
        }
        assert_eq!(
            input_of(&c, 990_799_999),
            Input::Other,
            "unknown: not gated"
        );
        assert_eq!(input_of(&c, CARRIER), Input::Other);
    }

    /// PROG-PVE: invented blueprints for the table's own hull types.
    #[test]
    fn the_warship_rungs_are_the_battlecruiser_then_the_battleship_blueprints() {
        use crate::hull_choice::{Class, HULLS};
        let hull = |class| {
            HULLS
                .iter()
                .filter(|h| h.class == class)
                .map(|h| h.type_id)
                .collect::<Vec<_>>()
        };
        let (bcs, bss, cruisers) = (
            hull(Class::Battlecruiser),
            hull(Class::Battleship),
            hull(Class::Cruiser),
        );
        let bp = |i: i64| 990_800_000 + i;
        let mut rows = Vec::new();
        let mut add = |b: i64, product: i64| {
            rows.push(format!("{b}\t1\tproduct\t{product}\t1\t\t1000\t1"));
        };
        add(bp(2), bss[0]);
        add(bp(1), bcs[0]);
        add(bp(3), bcs[1]);
        add(bp(4), cruisers[0]);
        add(bp(5), 990_800_999);
        let recipes = industry::Table::parse(&rows, &eve_server::econ_prices::Table::default())
            .expect("the recipes parse");
        assert_eq!(
            warship_rungs(&recipes),
            vec![vec![bp(1), bp(3)], vec![bp(2)]],
            "battlecruisers, then battleships; cruisers and strangers are not"
        );
        let none = industry::Table::parse(
            &Vec::<String>::new(),
            &eve_server::econ_prices::Table::default(),
        )
        .expect("empty");
        assert!(warship_rungs(&none).is_empty());
    }

    /// PROG-PVE2: one rung per weapon system and one for the tank, found by
    /// what the recipes make.
    #[test]
    fn the_arms_rungs_are_one_per_weapon_system_and_one_for_the_tank() {
        use crate::hull_choice::Weapon;
        let bp = |i: i64| 990_810_000 + i;
        let first = |w: Weapon| crate::fit_plan::products_of(w)[0];
        let mut rows = Vec::new();
        let mut add = |b: i64, product: i64| {
            rows.push(format!("{b}\t1\tproduct\t{product}\t1\t\t1000\t1"));
        };
        add(bp(1), first(Weapon::Laser));
        add(bp(2), first(Weapon::Laser));
        add(bp(3), first(Weapon::Hybrid));
        add(bp(4), crate::tank::products()[0]);
        add(bp(5), 990_810_999);
        let recipes = industry::Table::parse(&rows, &eve_server::econ_prices::Table::default())
            .expect("the recipes parse");
        let rungs = arms_rungs(&recipes);
        assert_eq!(rungs.len(), 3, "projectile and missile have no maker here");
        assert!(rungs.contains(&vec![bp(1), bp(2)]), "{rungs:?}");
        assert!(rungs.contains(&vec![bp(3)]));
        assert!(rungs.contains(&vec![bp(4)]));
        assert!(!rungs.iter().flatten().any(|b| *b == bp(5)));
        let none = industry::Table::parse(
            &Vec::<String>::new(),
            &eve_server::econ_prices::Table::default(),
        )
        .expect("empty");
        assert!(arms_rungs(&none).is_empty());
    }

    // ---- T3-F: the bundle's Tech 3 trees ---------------------------------------

    /// `groupStrategicCruiser`, `categorySubSystem` **[V-data]** (the server's
    /// `subsystems` module is private).
    const GROUP_STRATEGIC_CRUISER: i64 = 963;
    const CATEGORY_SUBSYSTEM: i64 = 32;
    /// `groupTacticalDestroyer` **[V-data]**.
    const GROUP_TACTICAL_DESTROYER: i64 = 1305;

    /// **Every Tech 3 recipe has all its types** (`missing_type` clean), read
    /// from a real bundle: set `EVE_TEST_BUNDLE` to `out/content`
    /// (`tools/dev.sh env EVE_TEST_BUNDLE=/repo/out/content cargo test ...`),
    /// otherwise it passes without running, like the other bundle checks.
    /// No id is named: the roots are every blueprint whose product is a
    /// strategic cruiser, a tactical destroyer or a subsystem, and the walk
    /// follows every material that some recipe or formula makes. Asserted:
    /// each of the three kinds has recipes, each recipe's product and
    /// materials are types the item store can create, and every Hybrid Tech
    /// Component the trees use has a recipe and an NPC-buyable original
    /// shape (a manufacturing row) -- so the chain ends in what bots supply.
    #[test]
    fn every_t3_bp_has_all_its_types() {
        let Some(dir) = std::env::var_os("EVE_TEST_BUNDLE") else {
            return;
        };
        let content = open(Path::new(&dir)).expect("the bundle loads");
        let table: &industry::Table = content.get();
        let kind = |p: i64| {
            eve_server::item::find_in(&content, p).and_then(|i| {
                if i.group_id == GROUP_STRATEGIC_CRUISER {
                    Some(0)
                } else if i.group_id == GROUP_TACTICAL_DESTROYER {
                    Some(1)
                } else if i.category_id == CATEGORY_SUBSYSTEM {
                    Some(2)
                } else {
                    None
                }
            })
        };
        let mut seen = std::collections::BTreeSet::new();
        let mut count = [0usize; 3];
        let mut stack: Vec<i64> = Vec::new();
        for bp in &table.blueprints {
            if let Some(k) = kind(bp.product_type_id) {
                count[k] += 1;
                stack.push(bp.blueprint_type_id);
            }
        }
        assert!(
            count.iter().all(|&n| n > 0),
            "T3 recipes by kind: {count:?}"
        );
        let mut components = 0;
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let bp = table.lookup(id).expect("a walked blueprint");
            assert_eq!(
                industry::missing_type(&content, bp),
                None,
                "blueprint {id}: a type the item store lacks"
            );
            assert!(bp.time > 0 && bp.product_quantity > 0, "blueprint {id}");
            for &(m, _) in &bp.materials {
                if group_of(&content, m) == Some(GROUP_HYBRID_COMPONENT) {
                    components += 1;
                }
                let maker = table
                    .by_product(m)
                    .or_else(|| table.reactions.iter().find(|r| r.product_type_id == m));
                if let Some(sub) = maker {
                    stack.push(sub.blueprint_type_id);
                }
            }
        }
        assert!(components > 0, "the trees use Hybrid Tech Components");
        assert!(
            !t3_rungs(&content, table).is_empty(),
            "and a builder can climb to them"
        );
    }

    #[test]
    fn a_push_about_the_ships_own_attribute_is_read_and_one_about_another_item_is_not() {
        use PyValue::{Int, Real};
        let one = vec![
            Int(1),
            Int(500),
            Int(76),
            Int(0),
            Real(9_000.0),
            Real(30_000.0),
        ];
        assert_eq!(
            ship_attribute_changes("OnModuleAttributeChange", &one, 500),
            vec![(76, 9_000.0)]
        );
        assert!(ship_attribute_changes("OnModuleAttributeChange", &one, 501).is_empty());
        let batch = vec![PyValue::List(vec![
            PyValue::Tuple(vec![
                PyValue::None,
                Int(1),
                Int(500),
                Int(192),
                Int(0),
                Int(5),
                Int(6),
            ]),
            PyValue::Tuple(vec![
                PyValue::None,
                Int(1),
                Int(777),
                Int(76),
                Int(0),
                Real(1.0),
                Real(2.0),
            ]),
        ])];
        assert_eq!(
            ship_attribute_changes("OnModuleAttributeChanges", &batch, 500),
            vec![(192, 5.0)]
        );
        assert!(ship_attribute_changes("OnSomething", &one, 500).is_empty());
    }
}
