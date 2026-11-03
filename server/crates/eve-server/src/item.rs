//! The item store, and the handful of static type facts a spawn needs.
//!
//! # Why the type table is here and not derived
//!
//! **The client reads `groupID` and `categoryID` off our row and never
//! re-derives them from `typeID`.** `godma.StateManager.UpdateItem` proves it
//! by using both sources five lines apart: `evetypes.GetGroupID/GetCategoryID`
//! for a sublocation, and `item.groupID`/`item.categoryID` off our row for
//! everything else -- and it compares them nowhere. Thirteen more consumers
//! read the row, including the Ship-Hangar/Item-Hangar split
//! (`invControllers.py:1331`/`:1445`), whether a ship model is drawn
//! (`hangarView.py:725`) and whether a weapon offers an ammo slot
//! (`eveCfg.py:411`). Meanwhile the name, icon, volume, cargo capacity, dogma
//! attributes and 3D model all come from the client's own FSD, keyed by
//! `typeID`.
//!
//! So a row that contradicts the client's static data does not raise and is
//! not ignored -- **it is believed, and the UI diverges silently.** That is
//! why these values are not invented: every row below came from the client's
//! own `types.fsdbinary` and `groups.fsdbinary`, read by `tools/fsd_dump.py`,
//! and is recorded with its provenance in `findings/spawn-types.tsv`.
//!
//! This is the `static.db` side of the split in `docs/reference/world-state.md` §1:
//! **regenerable, safe to delete, and nothing in the world may depend on a
//! value living only here.** A curated two dozen types is the documented
//! stopgap for exactly this, and the reader that would replace it now exists
//! (`tools/fsd.py`) -- it is Python, and teaching the server to read FSD
//! directly is a separate job with its own design question, because
//! `server/deps.allow` is empty and stays empty.
//!
//! `cfg.invtypes` does **not** exist in this build -- zero hits across 12,774
//! files -- so there is no client-side table we could ask about instead.

use crate::content::Content;
use crate::model::FLAG_HANGAR;

/// The static facts about one type that a row we send has to carry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemType {
    pub type_id: i64,
    pub name: &'static str,
    pub group_id: i64,
    pub category_id: i64,
    /// Whether one of these is a distinct object (a ship) or a stack of
    /// interchangeable ones (ammo, ore).
    ///
    /// Not cosmetic: `singleton` must be truthy for `UpdateItem` and for
    /// `ShipPicker`, and a stack of ships or a singleton round of ammo is a
    /// shape the client has no sensible rendering for. [`Store::spawn_in`]
    /// turns a quantity of a singleton type into that many separate items
    /// rather than letting the combination through.
    pub singleton: bool,
    /// Cubic metres one unit occupies, from `types.fsdbinary`.
    pub volume: f64,
    /// Cubic metres this type can *hold*. Zero for anything that is not a
    /// container.
    ///
    /// **Read from the double at `+16`, which the `loader-pyd` lane
    /// corrected**: an earlier pass had that field labelled `volume`, and it
    /// validated perfectly on its own sample because a ship's volume and its
    /// cargo capacity are both plausible numbers. A capsule's capacity is
    /// `0.0` -- a pod genuinely has no cargo hold.
    pub capacity: f64,
}

/// The types a sandbox can grant.
///
/// Deliberately small and deliberately boring: a few ships across classes and
/// races, a few modules, one round of ammo, one mineral. Enough to test
/// hangars, fitting, stacking and the ship/item split without pretending to
/// ship EVE's item database.
pub const SPAWNABLE: &[ItemType] = &[
    // The capsule. **In the table for its stats as much as for granting
    // it**: `used_volume` and `container_capacity` look every item's type up
    // here, so leaving the pod out made the station hangar report zero usage
    // and a pod's own hold report zero capacity *for the wrong reason* -- an
    // unknown type rather than a type whose capacity is genuinely zero. A
    // test passing because a lookup failed is the kind of green that hides
    // things.
    //
    // Granting a second pod is harmless, which is why it is simply spawnable
    // rather than hidden behind another list.
    ItemType {
        type_id: 670,
        name: "Capsule",
        group_id: 29,
        category_id: 6,
        singleton: true,
        volume: 1000.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 587,
        name: "Rifter",
        group_id: 25,
        category_id: 6,
        singleton: true,
        volume: 27289.0,
        capacity: 140.0,
    },
    ItemType {
        type_id: 603,
        name: "Merlin",
        group_id: 25,
        category_id: 6,
        singleton: true,
        volume: 16500.0,
        capacity: 150.0,
    },
    ItemType {
        type_id: 597,
        name: "Punisher",
        group_id: 25,
        category_id: 6,
        singleton: true,
        volume: 28600.0,
        capacity: 135.0,
    },
    ItemType {
        type_id: 594,
        name: "Incursus",
        group_id: 25,
        category_id: 6,
        singleton: true,
        volume: 29500.0,
        capacity: 165.0,
    },
    ItemType {
        type_id: 16242,
        name: "Thrasher",
        group_id: 420,
        category_id: 6,
        singleton: true,
        volume: 43000.0,
        capacity: 400.0,
    },
    ItemType {
        type_id: 16240,
        name: "Catalyst",
        group_id: 420,
        category_id: 6,
        singleton: true,
        volume: 55000.0,
        capacity: 450.0,
    },
    ItemType {
        type_id: 629,
        name: "Rupture",
        group_id: 26,
        category_id: 6,
        singleton: true,
        volume: 96000.0,
        capacity: 450.0,
    },
    ItemType {
        type_id: 621,
        name: "Caracal",
        group_id: 26,
        category_id: 6,
        singleton: true,
        volume: 92000.0,
        capacity: 450.0,
    },
    ItemType {
        type_id: 2006,
        name: "Omen",
        group_id: 26,
        category_id: 6,
        singleton: true,
        volume: 118000.0,
        capacity: 400.0,
    },
    ItemType {
        type_id: 24702,
        name: "Hurricane",
        group_id: 419,
        category_id: 6,
        singleton: true,
        volume: 216000.0,
        capacity: 425.0,
    },
    ItemType {
        type_id: 16227,
        name: "Ferox",
        group_id: 419,
        category_id: 6,
        singleton: true,
        volume: 252000.0,
        capacity: 475.0,
    },
    ItemType {
        type_id: 641,
        name: "Megathron",
        group_id: 27,
        category_id: 6,
        singleton: true,
        volume: 470000.0,
        capacity: 845.0,
    },
    ItemType {
        type_id: 642,
        name: "Apocalypse",
        group_id: 27,
        category_id: 6,
        singleton: true,
        volume: 470000.0,
        capacity: 845.0,
    },
    ItemType {
        type_id: 639,
        name: "Tempest",
        group_id: 27,
        category_id: 6,
        singleton: true,
        volume: 450000.0,
        capacity: 780.0,
    },
    ItemType {
        type_id: 648,
        name: "Badger",
        group_id: 28,
        category_id: 6,
        singleton: true,
        volume: 250000.0,
        capacity: 3900.0,
    },
    ItemType {
        type_id: 657,
        name: "Iteron Mark V",
        group_id: 28,
        category_id: 6,
        singleton: true,
        volume: 275000.0,
        capacity: 5800.0,
    },
    ItemType {
        type_id: 1944,
        name: "Bestower",
        group_id: 28,
        category_id: 6,
        singleton: true,
        volume: 260000.0,
        capacity: 4800.0,
    },
    ItemType {
        type_id: 601,
        name: "Ibis",
        group_id: 237,
        category_id: 6,
        singleton: true,
        volume: 15000.0,
        capacity: 125.0,
    },
    ItemType {
        type_id: 484,
        name: "125mm Gatling AutoCannon I",
        group_id: 55,
        category_id: 7,
        singleton: false,
        volume: 5.0,
        capacity: 0.5,
    },
    // `alice_with_a_gun`'s gun and the spec's worked example
    // (`docs/re/modules.md` §6.3): group 55, volume 5.0, **capacity 0.3**
    // -- 120 rounds of S ammo -- from `types` [V] (`tools/dogma.py`,
    // 2026-10-24). Here so a charge can be counted against it.
    ItemType {
        type_id: 486,
        name: "200mm AutoCannon I",
        group_id: 55,
        category_id: 7,
        singleton: false,
        volume: 5.0,
        capacity: 0.3,
    },
    ItemType {
        type_id: 483,
        name: "Miner I",
        group_id: 54,
        category_id: 7,
        singleton: false,
        volume: 5.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 377,
        name: "Small Shield Extender I",
        group_id: 38,
        category_id: 7,
        singleton: false,
        volume: 5.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 21853,
        name: "Civilian Armor Repairer",
        group_id: 62,
        category_id: 7,
        singleton: false,
        volume: 5.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 21857,
        name: "1MN Civilian Afterburner",
        group_id: 46,
        category_id: 7,
        singleton: false,
        volume: 5.0,
        capacity: 0.0,
    },
    // Probe scanning (P5.10, `crate::scan`): the two scan probe launchers
    // (group 481) and the two probes (group 479, category 8). Every number
    // from `types.fsdbinary` [V-data] on 2026-10-25. The core launcher holds
    // 0.8 m3 -- eight core probes at 0.1 -- and **no combat probe** (1.0
    // m3), which is CCP's own rule: ships are found with the expanded one.
    ItemType {
        type_id: 17938,
        name: "Core Probe Launcher I",
        group_id: 481,
        category_id: 7,
        singleton: false,
        volume: 5.0,
        capacity: 0.8,
    },
    ItemType {
        type_id: 18639,
        name: "Expanded Probe Launcher I",
        group_id: 481,
        category_id: 7,
        singleton: false,
        volume: 5.0,
        capacity: 8.0,
    },
    ItemType {
        type_id: 30013,
        name: "Core Scanner Probe I",
        group_id: 479,
        category_id: 8,
        singleton: false,
        volume: 0.1,
        capacity: 0.0,
    },
    ItemType {
        type_id: 30028,
        name: "Combat Scanner Probe I",
        group_id: 479,
        category_id: 8,
        singleton: false,
        volume: 1.0,
        capacity: 0.0,
    },
    // The civilian turrets: the only guns that shoot without a charge, so
    // the only ones `crate::combat` can fire (P2.2b). Group 53 for all four,
    // rail and blaster included; volume 5.0, capacity 0.0 -- `types` [V].
    ItemType {
        type_id: 3634,
        name: "Civilian Gatling Pulse Laser",
        group_id: 53,
        category_id: 7,
        singleton: false,
        volume: 5.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 3636,
        name: "Civilian Gatling Autocannon",
        group_id: 53,
        category_id: 7,
        singleton: false,
        volume: 5.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 3638,
        name: "Civilian Gatling Railgun",
        group_id: 53,
        category_id: 7,
        singleton: false,
        volume: 5.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 3640,
        name: "Civilian Light Electron Blaster",
        group_id: 53,
        category_id: 7,
        singleton: false,
        volume: 5.0,
        capacity: 0.0,
    },
    // **The four light combat drones** (`crate::drones`). Group 100 "Combat
    // Drone", category 18 `categoryDrone`, 5 m3 each, from `types.fsdbinary`
    // / `groups.fsdbinary` on build 3619879 **[V-data]**
    // (`out/drones/types.py`). Stackable: drones sit in a hangar or a bay as
    // one stack, and a launch splits one off (`drones::launch`).
    ItemType {
        type_id: 2203,
        name: "Acolyte I",
        group_id: 100,
        category_id: 18,
        singleton: false,
        volume: 5.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 2454,
        name: "Hobgoblin I",
        group_id: 100,
        category_id: 18,
        singleton: false,
        volume: 5.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 2464,
        name: "Hornet I",
        group_id: 100,
        category_id: 18,
        singleton: false,
        volume: 5.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 2486,
        name: "Warrior I",
        group_id: 100,
        category_id: 18,
        singleton: false,
        volume: 5.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 185,
        name: "EMP S",
        group_id: 83,
        category_id: 8,
        singleton: false,
        volume: 0.0025,
        capacity: 0.0,
    },
    // A second projectile charge, so a gun can be *re*loaded with a
    // different type (`crate::modules::load_ammo`, P2.3). Group 83
    // `Projectile Ammo`, category 8, volume 0.0025, capacity 0.0 -- `types`
    // [V], read with `tools/dogma.py`'s `types` table on 2026-10-24.
    ItemType {
        type_id: 184,
        name: "Phased Plasma S",
        group_id: 83,
        category_id: 8,
        singleton: false,
        volume: 0.0025,
        capacity: 0.0,
    },
    // Mission cargo: the four `Group of Elite Slaves` that mission 4832 tells
    // the player to load. Here rather than in `mission.rs` because the type
    // table is the type table -- `Store::spawn` refuses anything not in it,
    // and that refusal is what caught the mission's accept step shipping a
    // briefing whose cargo did not exist.
    //
    // Read, not guessed: groupID 283 `Livestock`, categoryID 17, volume 60.0,
    // `portionSize` 1, from `types.fsdbinary` and `groups.fsdbinary`.
    ItemType {
        type_id: 2665,
        name: "Group of Elite Slaves",
        group_id: 283,
        category_id: 17,
        // `missions.fsdbinary` gives 4832 `objectiveSingleton: 0`, so the
        // cargo stacks -- four units in one item rather than four items.
        singleton: false,
        volume: 60.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 34,
        name: "Tritanium",
        group_id: 18,
        category_id: 4,
        singleton: false,
        volume: 0.01,
        capacity: 0.0,
    },
    // **The other seven minerals, because `Edit::Create` refuses a type this
    // table does not know** and a reprocess produces up to eight of them.
    // Tritanium alone was enough while the only producer was `/createitem`.
    // All eight are group 18 `Mineral`, category 4 `Material`, volume 0.01 --
    // read from `types.fsdbinary`.
    ItemType {
        type_id: 35,
        name: "Pyerite",
        group_id: 18,
        category_id: 4,
        singleton: false,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 36,
        name: "Mexallon",
        group_id: 18,
        category_id: 4,
        singleton: false,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 37,
        name: "Isogen",
        group_id: 18,
        category_id: 4,
        singleton: false,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 38,
        name: "Nocxium",
        group_id: 18,
        category_id: 4,
        singleton: false,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 39,
        name: "Zydrine",
        group_id: 18,
        category_id: 4,
        singleton: false,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 40,
        name: "Megacyte",
        group_id: 18,
        category_id: 4,
        singleton: false,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 11399,
        name: "Morphite",
        group_id: 18,
        category_id: 4,
        singleton: false,
        volume: 0.01,
        capacity: 0.0,
    },
    // Ore, so there is something to refine. **Volume is per unit and a
    // portion is 100 units** (`portionSize` in `types.fsdbinary`), which is
    // why 1,000 Veldspar is ten portions and 100 m3 -- the refinery quotes
    // per portion and the hangar charges per unit, and conflating them is a
    // capacity bug that only shows up on a full hold.
    ItemType {
        type_id: 1230,
        name: "Veldspar",
        group_id: 462,
        category_id: 25,
        singleton: false,
        volume: 0.1,
        capacity: 0.0,
    },
    ItemType {
        type_id: 18,
        name: "Plagioclase",
        group_id: 458,
        category_id: 25,
        singleton: false,
        volume: 0.35,
        capacity: 0.0,
    },
    // **Every other ore a belt holds**, so a mining cycle has a type to
    // create (`Edit::Create` refuses one the table does not know). The 36
    // types in the client's whole `asteroidcountbytypeidbeltid.static`, less
    // Veldspar and Plagioclase above: `groupID`, `volume` and the name's
    // `typeNameID` from `types.fsdbinary` (`tools/dogma.py`), the name from
    // the en-us localisation (`tools/loctable.py`), every group category 25
    // in `groups.fsdbinary` (`tools/fsd_dump.py raw`) -- build 3619879,
    // 2026-10-24 **[V]**. Every one has `portionSize` 100. Note the grades:
    // this build names them "II-Grade" and "III-Grade", not the old
    // "Concentrated"/"Dense".
    ItemType {
        type_id: 20,
        name: "Kernite",
        group_id: 457,
        category_id: 25,
        singleton: false,
        volume: 1.2,
        capacity: 0.0,
    },
    ItemType {
        type_id: 21,
        name: "Hedbergite",
        group_id: 454,
        category_id: 25,
        singleton: false,
        volume: 3.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 22,
        name: "Arkonor",
        group_id: 450,
        category_id: 25,
        singleton: false,
        volume: 16.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 1223,
        name: "Bistot",
        group_id: 451,
        category_id: 25,
        singleton: false,
        volume: 16.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 1224,
        name: "Pyroxeres",
        group_id: 459,
        category_id: 25,
        singleton: false,
        volume: 0.3,
        capacity: 0.0,
    },
    ItemType {
        type_id: 1226,
        name: "Jaspet",
        group_id: 456,
        category_id: 25,
        singleton: false,
        volume: 2.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 1227,
        name: "Omber",
        group_id: 469,
        category_id: 25,
        singleton: false,
        volume: 0.6,
        capacity: 0.0,
    },
    ItemType {
        type_id: 1228,
        name: "Scordite",
        group_id: 460,
        category_id: 25,
        singleton: false,
        volume: 0.15,
        capacity: 0.0,
    },
    ItemType {
        type_id: 1231,
        name: "Hemorphite",
        group_id: 455,
        category_id: 25,
        singleton: false,
        volume: 3.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 11396,
        name: "Mercoxit",
        group_id: 468,
        category_id: 25,
        singleton: false,
        volume: 40.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17425,
        name: "Arkonor II-Grade",
        group_id: 450,
        category_id: 25,
        singleton: false,
        volume: 16.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17426,
        name: "Arkonor III-Grade",
        group_id: 450,
        category_id: 25,
        singleton: false,
        volume: 16.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17428,
        name: "Bistot II-Grade",
        group_id: 451,
        category_id: 25,
        singleton: false,
        volume: 16.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17429,
        name: "Bistot III-Grade",
        group_id: 451,
        category_id: 25,
        singleton: false,
        volume: 16.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17440,
        name: "Hedbergite II-Grade",
        group_id: 454,
        category_id: 25,
        singleton: false,
        volume: 3.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17441,
        name: "Hedbergite III-Grade",
        group_id: 454,
        category_id: 25,
        singleton: false,
        volume: 3.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17444,
        name: "Hemorphite II-Grade",
        group_id: 455,
        category_id: 25,
        singleton: false,
        volume: 3.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17445,
        name: "Hemorphite III-Grade",
        group_id: 455,
        category_id: 25,
        singleton: false,
        volume: 3.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17448,
        name: "Jaspet II-Grade",
        group_id: 456,
        category_id: 25,
        singleton: false,
        volume: 2.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17449,
        name: "Jaspet III-Grade",
        group_id: 456,
        category_id: 25,
        singleton: false,
        volume: 2.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17452,
        name: "Kernite II-Grade",
        group_id: 457,
        category_id: 25,
        singleton: false,
        volume: 1.2,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17453,
        name: "Kernite III-Grade",
        group_id: 457,
        category_id: 25,
        singleton: false,
        volume: 1.2,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17455,
        name: "Plagioclase II-Grade",
        group_id: 458,
        category_id: 25,
        singleton: false,
        volume: 0.35,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17456,
        name: "Plagioclase III-Grade",
        group_id: 458,
        category_id: 25,
        singleton: false,
        volume: 0.35,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17459,
        name: "Pyroxeres II-Grade",
        group_id: 459,
        category_id: 25,
        singleton: false,
        volume: 0.3,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17460,
        name: "Pyroxeres III-Grade",
        group_id: 459,
        category_id: 25,
        singleton: false,
        volume: 0.3,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17463,
        name: "Scordite II-Grade",
        group_id: 460,
        category_id: 25,
        singleton: false,
        volume: 0.15,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17464,
        name: "Scordite III-Grade",
        group_id: 460,
        category_id: 25,
        singleton: false,
        volume: 0.15,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17470,
        name: "Veldspar II-Grade",
        group_id: 462,
        category_id: 25,
        singleton: false,
        volume: 0.1,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17471,
        name: "Veldspar III-Grade",
        group_id: 462,
        category_id: 25,
        singleton: false,
        volume: 0.1,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17867,
        name: "Omber II-Grade",
        group_id: 469,
        category_id: 25,
        singleton: false,
        volume: 0.6,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17868,
        name: "Omber III-Grade",
        group_id: 469,
        category_id: 25,
        singleton: false,
        volume: 0.6,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17869,
        name: "Mercoxit II-Grade",
        group_id: 468,
        category_id: 25,
        singleton: false,
        volume: 40.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 17870,
        name: "Mercoxit III-Grade",
        group_id: 468,
        category_id: 25,
        singleton: false,
        volume: 40.0,
        capacity: 0.0,
    },
    ItemType {
        type_id: 62528,
        name: "Compressed Plagioclase",
        group_id: 458,
        category_id: 25,
        singleton: false,
        volume: 0.0035,
        capacity: 0.0,
    },
    // --- blueprints -------------------------------------------------------
    //
    // Twenty-three, added with industry manufacturing. A blueprint is an
    // *item* and `blueprintID` in every client read is an itemID, so without
    // these `Edit::Create` cannot mint one and no job can ever be installed.
    //
    // Every field read from the client's own `types.fsdbinary`: all 23 are
    // `volume` 0.01, `capacity` 0.0, `portionSize` 1 and published, and
    // `category_id` is 9 (`inventorycommon.const.categoryBlueprint`). **[V]**
    //
    // **`group_id` is the blueprint's own group, not the product's** -- 105
    // Frigate Blueprint, 106 Cruiser Blueprint, and so on. The client reads
    // `groupID` off our row and never re-derives it (see this file's header),
    // so a product's group here would be believed and wrong.
    //
    // `singleton: true` is **[I]** on the flag and **[V]** on the
    // consequence: `industryCommon.BlueprintInstance` derives
    // `singleton = data.quantity < 0` from the blueprint row, and a stack of
    // originals is not something the client can render.
    ItemType {
        type_id: 687,
        name: "Caracal Blueprint",
        group_id: 106,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 691,
        name: "Rifter Blueprint",
        group_id: 105,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 693,
        name: "Tempest Blueprint",
        group_id: 107,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 785,
        name: "Miner I Blueprint",
        group_id: 134,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 819,
        name: "125mm Gatling AutoCannon I Blueprint",
        group_id: 135,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 886,
        name: "EMP S Blueprint",
        group_id: 165,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 941,
        name: "Incursus Blueprint",
        group_id: 105,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 944,
        name: "Punisher Blueprint",
        group_id: 105,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 950,
        name: "Merlin Blueprint",
        group_id: 105,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 974,
        name: "Rupture Blueprint",
        group_id: 106,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 983,
        name: "Badger Blueprint",
        group_id: 108,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 992,
        name: "Iteron Mark V Blueprint",
        group_id: 108,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 995,
        name: "Megathron Blueprint",
        group_id: 107,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 996,
        name: "Apocalypse Blueprint",
        group_id: 107,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 1010,
        name: "Small Shield Extender I Blueprint",
        group_id: 118,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 1945,
        name: "Bestower Blueprint",
        group_id: 108,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 2007,
        name: "Omen Blueprint",
        group_id: 106,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 16228,
        name: "Ferox Blueprint",
        group_id: 489,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 16241,
        name: "Catalyst Blueprint",
        group_id: 487,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 16243,
        name: "Thrasher Blueprint",
        group_id: 487,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 21854,
        name: "Civilian Armor Repairer Blueprint",
        group_id: 142,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 21858,
        name: "1MN Civilian Afterburner Blueprint",
        group_id: 126,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 24703,
        name: "Hurricane Blueprint",
        group_id: 489,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    // Round 2 of industry (P5.4): what inventing from a Rifter blueprint copy
    // eats and makes, read from `types.fsdbinary` / `groups.fsdbinary`
    // **[V]**. The invention rows themselves come from the bundle's
    // `industry/blueprints.tsv` (`industry::Table`, CP33b). Datacores are a
    // stack (group 333, category 17), a blueprint is one object.
    ItemType {
        type_id: 11372,
        name: "Wolf Blueprint",
        group_id: 105,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 11401,
        name: "Jaguar Blueprint",
        group_id: 105,
        category_id: 9,
        singleton: true,
        volume: 0.01,
        capacity: 0.0,
    },
    ItemType {
        type_id: 20172,
        name: "Datacore - Minmatar Starship Engineering",
        group_id: 333,
        category_id: 17,
        singleton: false,
        volume: 0.1,
        capacity: 0.0,
    },
    ItemType {
        type_id: 20424,
        name: "Datacore - Mechanical Engineering",
        group_id: 333,
        category_id: 17,
        singleton: false,
        volume: 0.1,
        capacity: 0.0,
    },
];

/// The wrecks a destroyed ship can leave: **types the store may hold but
/// nobody may grant.** One row per wreck in `crate::combat::WRECKS`, and one
/// per NPC type's `crate::npc::NpcType::wreck_type_id`.
///
/// Apart from [`SPAWNABLE`] because `/createitem` (`find_type`) reads that
/// table alone, and a wreck in a station hangar is a shape the client has no
/// rendering for; [`find_in`] reads both, because `Edit::Create` and the volume
/// and capacity lookups must know a wreck once `crate::combat` has made one.
///
/// Every number is the client's own, read from `types.fsdbinary` and
/// `groups.fsdbinary` with `tools/fsd.py` (`cfsd_rows`) against build 3619879
/// on 2026-10-24 **[V]**: all eighteen are `groupWreck` 186, whose category is
/// `categoryCelestial` 2, with `volume` and `capacity` both 27,500 m3 and
/// `radius` 14 m. Names as `tools/dogma.py` gave them for P2.2b.
/// `singleton` because a wreck is one object with contents.
/// **The can a jettison makes** (GAPS-2, `crate::jettison`): type 23, "Cargo
/// Container", `groupCargoContainer` 12 in `categoryCelestial` 2, `volume`
/// and `capacity` both 55,000 m3, radius 14 m, read from `types.fsdbinary`
/// and `groups.fsdbinary` against build 3619879 with `tools/fsd.py` on
/// 2026-10-27 **[V]**. Apart from [`SPAWNABLE`] for [`WRECK_TYPES`]'s reason
/// (nobody may grant one) and because the bundle's typedata carries no
/// group-12 type at all; [`find_in`] reads it so the store can mint and
/// hold one. A container is a singleton: one object with contents.
pub const JETCAN_TYPES: &[ItemType] = &[ItemType {
    type_id: 23,
    name: "Cargo Container",
    group_id: 12,
    category_id: 2,
    singleton: true,
    volume: 55_000.0,
    capacity: 55_000.0,
}];

pub const WRECK_TYPES: &[ItemType] = &[
    ItemType {
        type_id: 26550,
        name: "Minmatar Frigate Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26528,
        name: "Gallente Frigate Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26484,
        name: "Amarr Frigate Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26510,
        name: "Caldari Rookie ship Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26506,
        name: "Caldari Frigate Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26495,
        name: "Caldari Cruiser Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26539,
        name: "Minmatar Cruiser Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26536,
        name: "Minmatar Battleship Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26514,
        name: "Gallente Battleship Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26470,
        name: "Amarr Battleship Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26507,
        name: "Caldari Hauler Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26529,
        name: "Gallente Hauler Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26485,
        name: "Amarr Hauler Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26473,
        name: "Amarr Cruiser Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26491,
        name: "Caldari Battlecruiser Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26518,
        name: "Gallente Destroyer Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26540,
        name: "Minmatar Destroyer Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    ItemType {
        type_id: 26535,
        name: "Minmatar Battlecruiser Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    // The NPC wreck: `npc::ANGEL_HIJACKER.wreck_type_id`, a store item since
    // W9-L1 so its loot can be taken. `types` row 26561 (`tools/dogma.py`,
    // build 3619879, 2026-10-25): group 186, volume and capacity 27,500 m3,
    // radius 14 m -- the same as the eighteen above **[V]**.
    ItemType {
        type_id: 26561,
        name: "Angel Small Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
    // The Angel cruisers' wreck (`npc::ANGEL_MEDIUM_WRECK`), for belt rats'
    // loot. `types` row 26562 (`tools/dogma.py`, build 3619879,
    // 2026-10-25): group 186, volume and capacity 27,500 m3, radius 14 m
    // **[V]**, the same as 26561.
    ItemType {
        type_id: 26562,
        name: "Angel Medium Wreck",
        group_id: 186,
        category_id: 2,
        singleton: true,
        volume: 27_500.0,
        capacity: 27_500.0,
    },
];

/// The courier package: what a courier contract's goods travel in
/// (`crate::contracts`, `service/wallet.rs`).
///
/// `typePlasticWrap` = 3468 (`inventorycommon/const.pyc`) **[V]**; group 649
/// `groupFreightContainer`, category 2, `volume` and `capacity` 0.01 m3, read
/// from `types.fsdbinary` / `groups.fsdbinary` with `tools/fsd.py` against
/// build 3619879 on 2026-10-25 **[V]**. Group 649 is in
/// `invConst.playerDeployedContainers`, which `itemCheckers.OfferBreakContract`
/// requires before it offers "Deliver" on the package, and so is `singleton`
/// (`IsSingleton`, same checker) **[V]**. Not in [`SPAWNABLE`]: a package is
/// minted by a contract, never granted.
pub const COURIER_TYPES: &[ItemType] = &[ItemType {
    type_id: 3468,
    name: "Plastic Wrap",
    group_id: 649,
    category_id: 2,
    singleton: true,
    volume: 0.01,
    capacity: 0.01,
}];

/// The categories a repackage is allowed to touch.
///
/// **Not our rule.** `repackaging/__init__.py` ships in this client and is the
/// whole of CCP's repackage validation -- `CheckCanRepackage`,
/// `CanRepackageType`, `CanRepackageInLocation` -- so the policy below is read
/// rather than invented. `REPACKABLE_CATEGORIES` is the twelve-tuple built at
/// `repackaging/__init__.py:8`-`:19`, and every value is **[V]** from
/// `inventorycommon/const.pyc`:
///
/// `categoryStarbase` 23, `categoryShip` 6, `categoryDrone` 18,
/// `categoryModule` 7, `categorySubSystem` 32,
/// `categorySovereigntyStructure` 40, `categoryDeployable` 22,
/// `categoryStructure` 65, `categoryStructureModule` 66, `categoryFighter` 87,
/// `categoryAncientRelic` 34, `categoryInfrastructureUpgrade` 39.
///
/// Note what is **not** here: `categoryCharge` (8), `categoryMaterial` (4),
/// `categoryBlueprint` (9) and `categoryAsteroid` (25). Those are never
/// singletons in the first place, so there is nothing to unassemble.
pub const REPACKABLE_CATEGORIES: &[i64] = &[23, 6, 18, 7, 32, 40, 22, 65, 66, 87, 34, 39];

/// The one group carved out of [`REPACKABLE_CATEGORIES`].
///
/// `REPACKABLE_CATEGORIES_GROUP_EXCEPTIONS = {groupCapsule}`
/// (`repackaging/__init__.py:22`), `groupCapsule` being 29. **It is the item
/// every character in this sandbox already owns**: `Registry::new` seeds a
/// capsule as the active ship, so without this line "Repackage" on the pod
/// would have been the first thing anyone tried and the first thing to go
/// wrong. CCP's own rule saves us from having to notice that.
pub const REPACKABLE_CATEGORY_GROUP_EXCEPTIONS: &[i64] = &[29];

/// Groups that repackage whatever their category is.
///
/// `REPACKABLE_GROUPS` at `repackaging/__init__.py:26`-`:35`, all **[V]**:
/// `groupCargoContainer` 12, `groupSecureCargoContainer` 340,
/// `groupAuditLogSecureContainer` 448, `groupFreightContainer` 649,
/// `groupTool` 332, `groupMobileWarpDisruptor` 361,
/// `groupPolymerReactionFormulas` 1889, `groupCompositeReactionFormulas` 1888,
/// `groupBiochemicalReactionFormulas` 1890,
/// `groupMolecularForgedReactionFormulas` 4097.
///
/// None of them is in [`SPAWNABLE`] today. They are here because the list is
/// the client's and a half-copied list is worse than none: the day a cargo
/// container becomes spawnable, this is already right.
pub const REPACKABLE_GROUPS: &[i64] = &[12, 340, 448, 649, 332, 361, 1889, 1888, 1890, 4097];

/// Can a type be repackaged at all?
///
/// `CanRepackageType(typeID)` (`repackaging/__init__.py:82`), minus one test
/// we cannot make: `evetypes.IsDynamicType(typeID)` refuses abyssal/mutated
/// modules, and nothing in [`SPAWNABLE`] is one -- mutaplasmid output is an
/// ordinary type id flagged `isDynamicType` and is minted, never spawned. Said here rather
/// than silently skipped.
///
/// **Takes the group and category rather than the type id**, because that is
/// what we have: the client derives both from its own FSD by `typeID`, and our
/// [`Item`] carries them because the client reads them off our row and never
/// re-derives them (this module's header). Same source, one lookup fewer, and
/// it keeps the check usable for an item whose type is not in our table.
pub fn can_repackage_type(group_id: i64, category_id: i64) -> bool {
    if REPACKABLE_CATEGORIES.contains(&category_id)
        && !REPACKABLE_CATEGORY_GROUP_EXCEPTIONS.contains(&group_id)
    {
        return true;
    }
    REPACKABLE_GROUPS.contains(&group_id)
}

/// Is this flag a fitting slot -- i.e. is the item fitted to something?
///
/// `inventorycommon/util.IsShipFittingFlag` is `flagID in const.fittingFlags
/// or flagID == const.flagHiddenModifers`, and `fittingFlags`
/// (`inventorycommon/const.pyc`, source line 2916) is built as
/// `loSlotFlags + medSlotFlags + hiSlotFlags + subsystemSlotFlags +
/// rigSlotFlags + serviceSlotFlags`. Decoded, all **[V]**:
///
/// | family | flags |
/// |---|---|
/// | `flagLoSlot0..7` | 11-18 |
/// | `flagMedSlot0..7` | 19-26 |
/// | `flagHiSlot0..7` | 27-34 |
/// | `flagSubSystemSlot0..3` | 125-128 |
/// | `flagRigSlot0..2` | 92-94 |
/// | `flagServiceSlot0..7` | 164-171 |
/// | `flagHiddenModifers` | 156 |
///
/// **`rigSlotFlags` holds three entries, not eight**, although
/// `flagRigSlot0..7` (92-99) all exist as constants. So by the client's own
/// definition a module in rig slot 4 is *not* in a fitting flag. That is
/// copied rather than corrected: this predicate exists to agree with
/// `CanRepackageInLocation`, and a server that is stricter than the client
/// here would refuse a repackage the client had already offered. Noted as the
/// client's quirk, in case something else ever needs the honest range.
pub fn is_ship_fitting_flag(flag_id: i64) -> bool {
    matches!(flag_id, 11..=34 | 92..=94 | 125..=128 | 156 | 164..=171)
}

// ---------------------------------------------------------------------------
// Per-flag hold capacity (CAP-B)
// ---------------------------------------------------------------------------

/// One of a ship's holds, as `inventoryFlagsCommon.inventoryFlagData` rows it
/// [V]: the flag, the dogma attribute that is its capacity, and what it takes.
///
/// The client's `ShouldAllowAdd` reads the row as: a type is refused unless
/// its category is in `categories` (and its group is not in `blocked`), or
/// its group is in `groups`; a row with neither list takes anything
/// ([`hold_accepts`]). A row with an `allowTypes` list in the client is not
/// `exact` here: the list is not modelled, so [`hold_accepts`] gives no
/// answer for it. The fighter bay (158) and the tubes are not here: they are
/// the fighter lanes'.
#[derive(Debug, Clone, Copy)]
pub struct Hold {
    pub flag: i64,
    /// The capacity attribute (`inventoryFlagData[flag]['attribute']`) [V].
    pub attribute: i64,
    pub categories: Option<&'static [i64]>,
    pub groups: Option<&'static [i64]>,
    pub blocked: &'static [i64],
    pub exact: bool,
}

const fn hold(
    flag: i64,
    attribute: i64,
    categories: Option<&'static [i64]>,
    groups: Option<&'static [i64]>,
    blocked: &'static [i64],
    exact: bool,
) -> Hold {
    Hold {
        flag,
        attribute,
        categories,
        groups,
        blocked,
        exact,
    }
}

/// `flagSpecializedFuelBay` [V] (`inventorycommon/const.py:167`).
pub const FLAG_FUEL_BAY: i64 = 133;
/// `attributeSpecialFuelBayCapacity` [V] (`dogma/const.py`).
pub const ATTR_FUEL_BAY_CAPACITY: i64 = 1549;
/// `groupIceProduct` [V]: the one group the fuel bay takes
/// (`inventoryFlagData[flagSpecializedFuelBay]['allowGroups']`). Isotopes,
/// Liquid Ozone, Heavy Water and Strontium Clathrates are in it; the
/// research spec's "isotopes + ozone + strontium + heavy water" list is the
/// group's members, not a separate rule.
pub const GROUP_ICE_PRODUCT: i64 = 423;

/// The holds [V]: `inventoryFlagsCommon.inventoryFlagData` of build 3619879,
/// every row but the fighter bay. Cargo (5) is the `capacity` attribute (38)
/// and takes anything.
pub const HOLDS: &[Hold] = &[
    hold(5, 38, None, None, &[], true),
    hold(87, 283, Some(&[18]), None, &[], true), // drone bay
    hold(90, 908, Some(&[6]), None, &[29], true), // ship maintenance bay
    hold(155, 912, None, None, &[], true),       // fleet hangar
    hold(
        FLAG_FUEL_BAY,
        ATTR_FUEL_BAY_CAPACITY,
        None,
        Some(&[GROUP_ICE_PRODUCT]),
        &[],
        true,
    ),
    hold(
        134,
        1556,
        Some(&[25]),
        Some(&[711, 4168, 4915]),
        &[4094, 519, 4714],
        true,
    ), // general mining hold (`crate::mining_hold`)
    hold(135, 1557, None, Some(&[711, 4168]), &[], true), // gas
    hold(136, 1558, None, Some(&[18, 4932]), &[], true),  // mineral
    hold(137, 1559, None, Some(&[966, 754, 355]), &[], true), // salvage
    hold(138, 1560, Some(&[6]), None, &[], true),         // ship hold
    hold(143, 1573, Some(&[8]), None, &[], false),        // ammo (+ one type)
    hold(148, 1646, None, Some(&[1027]), &[], true),      // command center
    hold(149, 1653, Some(&[43, 42]), None, &[], false),
    hold(151, 1770, Some(&[43, 17, 4]), None, &[], true), // material bay
    hold(154, 1804, None, None, &[], false),              // quafe (types only)
    hold(174, 2467, None, None, &[], false),              // corpse bay (types only)
    hold(176, 2657, None, Some(&[303, 313, 712]), &[], true), // booster
    hold(177, 2675, Some(&[32]), None, &[], true),        // subsystem
    hold(179, 3020, None, Some(&[25, 324, 1527, 893]), &[29], true), // frigate escape
    hold(181, 3136, None, Some(&[465, 2022, 903]), &[], true), // ice
    hold(
        182,
        3227,
        Some(&[25]),
        Some(&[4915]),
        &[4094, 519, 903, 2022, 465, 4714],
        true,
    ), // asteroid
    hold(183, 5325, None, Some(&[1246]), &[], true),      // mobile depot
    hold(185, 5646, None, None, &[], false),              // colony resources
    hold(186, 5693, None, Some(&[427, 18]), &[], true),   // moon material bay
    hold(188, 5944, None, None, &[], false),              // expedition hold
];

/// One flag's hold row, if it is a hold [`HOLDS`] knows.
pub fn hold_of(flag_id: i64) -> Option<&'static Hold> {
    HOLDS.iter().find(|h| h.flag == flag_id)
}

/// A ship type's capacity in one hold, m3: the flag's capacity attribute
/// from the type's own dogma (cargo's is the type's `capacity`, as the
/// client measures it, `invCache.py:1380`) [V]. Zero for a flag that is not
/// a hold or a hull without one. [A] no skill, rig or module modifies it
/// here (`crate::fitting::cargo_capacity_in` has the modified cargo).
pub fn hold_capacity_in(content: &Content, ship_type_id: i64, flag_id: i64) -> f64 {
    let Some(h) = hold_of(flag_id) else {
        return 0.0;
    };
    if flag_id == 5 {
        return find_in(content, ship_type_id).map_or(0.0, |t| t.capacity);
    }
    crate::typedata::attribute_in(content, ship_type_id, h.attribute)
        .filter(|c| c.is_finite() && *c > 0.0)
        .unwrap_or(0.0)
}

/// `ShouldAllowAdd(flag, category, group, ...) is None` [V]
/// (`inventoryFlagsCommon.py:394`), for the holds whose row is `exact`;
/// `None` for any other flag.
pub fn hold_accepts(flag_id: i64, category_id: i64, group_id: i64) -> Option<bool> {
    let h = hold_of(flag_id).filter(|h| h.exact)?;
    Some(match (h.categories, h.groups) {
        (None, None) => true,
        (cats, groups) => {
            // The client's order: an allowed category is then tested against
            // the blocked groups only; any other category falls to the
            // allowed groups.
            if cats.is_some_and(|c| c.contains(&category_id)) {
                !h.blocked.contains(&group_id)
            } else {
                groups.is_some_and(|g| g.contains(&group_id))
            }
        }
    })
}

/// `categoryCharge` **[V]** (`inventorycommon/const.pyc`; the same constant
/// [`REPACKABLE_CATEGORIES`]' note leaves out).
pub const CATEGORY_CHARGE: i64 = 8;

/// **A charge loaded into a module**: a charge-category item whose address is
/// the ship itself at the module's fitting flag.
///
/// That is how the store holds it, because it is what it is -- rounds that
/// left the hangar and have not been fired yet, conserved and persisted like
/// any other stack. **It is not how the client sees it.** To the client a
/// loaded charge is a *sublocation*, keyed `(shipID, flagID, typeID)` with
/// its count in attribute 805 and no item id of its own
/// (`docs/re/modules.md` §6.1, `godma:1863`) [V], and every client path that
/// meets one tests `type(itemID) is tuple` (`invCache.py` `Add`/`MultiAdd`,
/// `godma.IsSubLocation`) [V]. So an item for which this is true is never
/// sent as an item row: `Registry::apply_items` leaves its `OnItemChange`
/// out, `invbroker` listings leave it out, and `crate::modules` tells godma
/// about it in the sublocation's own shape instead.
pub fn is_loaded_charge(i: &Item) -> bool {
    i.category_id == CATEGORY_CHARGE && is_ship_fitting_flag(i.flag_id)
}

/// **One type by id, from `content`** (CP58b, ruling 3): the hand rows
/// ([`SPAWNABLE`], [`WRECK_TYPES`], [`COURIER_TYPES`]), PI's command centres
/// and commodities (`crate::planet::item_types_in`), the skillbooks
/// (`crate::skill::item_types_in`), then every type the bundle's typedata
/// holds (`crate::typedata::item_in`). **The one lookup a caller with a
/// registry uses** (the process-wide `find` went in X2b): a PI commodity
/// has its volume here (tracker §77). **The hand rows first**: they are the
/// client's numbers too, and they are what every test was written against.
pub fn find_in(content: &Content, type_id: i64) -> Option<&ItemType> {
    SPAWNABLE
        .iter()
        .chain(WRECK_TYPES)
        .chain(JETCAN_TYPES)
        .chain(COURIER_TYPES)
        .chain(crate::corp_office::OFFICE_TYPES)
        .chain(crate::planet::item_types_in(content))
        .chain(crate::skill::item_types_in(content))
        .chain(crate::nes::item_types_in(content))
        .chain(crate::skill_trading::item_types_in(content))
        .find(|t| t.type_id == type_id)
        .or_else(|| crate::typedata::item_in(content, type_id))
        // After typedata, and by index: the SKIN licences are ~12k rows.
        .or_else(|| crate::skins::item_in(content, type_id))
}

/// **A grantable type by id or by name, from `content`** (CP58b), by name
/// case-insensitively: `/spawn rifter` is what a person types and
/// `/spawn 587` is what a script does. [`SPAWNABLE`], PI's types (a command
/// centre has to be grantable for a colony to be startable) and the
/// skillbooks, then every **published** type the bundle's typedata holds
/// (`crate::typedata::grantable_in`): an unpublished type is CCP's test or
/// NPC-only row, and stays reachable by [`find_in`] alone. A wreck is never
/// granted.
pub fn find_type_in<'c>(content: &'c Content, needle: &str) -> Option<&'c ItemType> {
    let grantable = || {
        SPAWNABLE
            .iter()
            .chain(crate::planet::item_types_in(content))
            .chain(crate::skill::item_types_in(content))
            .chain(crate::nes::item_types_in(content))
            .chain(crate::skill_trading::item_types_in(content))
            .chain(crate::skins::item_types_in(content))
    };
    let hand = if let Ok(id) = needle.parse::<i64>() {
        grantable().find(|t| t.type_id == id)
    } else {
        grantable().find(|t| t.name.eq_ignore_ascii_case(needle))
    };
    hand.or_else(|| crate::typedata::grantable_in(content, needle))
}

/// One item in the world.
///
/// The fields are `item_columns()`'s ten real ones; `customInfo` is always
/// NULL on the wire and `stacksize` is `quantity`, so neither is stored. See
/// `Registry::item_row` for the row this becomes.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub item_id: i64,
    pub type_id: i64,
    pub owner_id: i64,
    pub location_id: i64,
    pub flag_id: i64,
    pub quantity: i64,
    pub group_id: i64,
    pub category_id: i64,
    pub singleton: bool,
}

/// The lowest id a player-owned item may have.
///
/// **Not cosmetic.** `idCheckers.IsSystemOrNPC(id)` is `id < 90000000` and
/// `cfg.Recordset._Prime` routes such keys through the client's *local* sqlite
/// fetcher before it ever asks us, so an item below this is simply invisible.
/// `IsPlayerItem` is `100000000 <= id < minFakeItem`, and it gates the SKIN
/// lookup. Do not reason by analogy from `character_id` 90000001: that is a
/// valid character id and specifically not a player item.
pub const MIN_PLAYER_ITEM: i64 = 100_000_000;

/// `OnItemChange` column indices, `carbon/common/lib/const` and
/// `inventorycommon/const` **[V]**. See [`Change::previous_columns`].
pub const IX_OWNER_ID: i64 = 2;
pub const IX_LOCATION_ID: i64 = 3;
pub const IX_FLAG: i64 = 4;
pub const IX_STACK_SIZE: i64 = 9;
pub const IX_SINGLETON: i64 = 10;

/// Where an item lives: who owns it, what it is inside, and which slot.
///
/// **The whole address, and nothing else.** This is the one design EvEJS gets
/// right and gets right expensively: its `itemCustody.js` declares fourteen
/// "custody ref kinds" and every one of them collapses to this triple
/// (`itemCustody.js:44-59, :390-481`), the kind being a routing tag rather
/// than a second address space. Fitted modules are not a separate table
/// either -- they are items whose `flag_id` falls in a fitting range.
///
/// So there is no item-location *model* to build. There is an address, and
/// operations that change it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub owner_id: i64,
    pub location_id: i64,
    pub flag_id: i64,
}

impl Slot {
    pub fn new(owner_id: i64, location_id: i64, flag_id: i64) -> Self {
        Slot {
            owner_id,
            location_id,
            flag_id,
        }
    }
}

impl Item {
    /// This item's address.
    pub fn slot(&self) -> Slot {
        Slot::new(self.owner_id, self.location_id, self.flag_id)
    }
}

/// The base of the **one** escrow location band.
///
/// One band, deliberately. EvEJS hand-partitions eight of them -- 9.2e9
/// market, 9.3e9 contract, 9.4e9 reprocessing, 9.5e9 missions, 9.6e9
/// planetary customs, 9.7e9 industry, 9.8e9 jump-bridge fuel, 1.02e10
/// market seed -- and then carries a **second, incompatible convention**
/// beside it: "installed in an industry job" is the magic sentinel
/// `locationID = 2003` (`industryConstants.js:90`), tested against at nine
/// sites. Two conventions for one concept in one codebase is a thing to fix
/// on the way in rather than inherit.
///
/// `2003` is also the worse of the two choices for a reason their code does
/// not mention: it is **below** the 90,000,000 `IsSystemOrNPC` threshold
/// documented at [`MIN_PLAYER_ITEM`], so the client would answer questions
/// about that location from its own local sqlite before ever asking us.
/// Every escrow location we mint sits above it.
pub const ESCROW_BASE: i64 = 9_000_000_000;

/// The escrow location for one ticket -- a market order, a contract, a job.
///
/// The caller supplies a ticket id unique within its own subsystem; escrow is
/// a *location*, so two subsystems reusing one number would silently share a
/// bucket. Callers therefore namespace their tickets, not their bands.
pub fn escrow_location(ticket_id: i64) -> i64 {
    ESCROW_BASE + ticket_id
}

/// Is this a location we minted for escrow?
pub fn is_escrow(location_id: i64) -> bool {
    location_id >= ESCROW_BASE
}

/// One intended change to the store.
///
/// A list of these is a **plan**: [`Store::apply_in`] either performs all of them
/// or none, and that is where our atomicity comes from.
///
/// EvEJS has no transaction, so it reaches the same place the long way: a
/// durable receipt ledger stored *inside* the items table
/// (`itemStore.js:86, :5461`), a whole-table sync fence, per-domain
/// compensating sagas, a reentrancy guard, and an explicit "I do not know"
/// state for when the in-memory snapshot runs ahead of SQLite. **None of that
/// is needed here** and none of it should be ported: it is scaffolding around
/// a missing primitive that we have. Their market fill is an eight-step saga
/// in which ISK and items move at steps 1-5 and the order is decremented at
/// step 6, so a crash between leaves the buyer paid and holding goods while
/// the order still shows open. A plan cannot do that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Move an item whole, to any address.
    Move { item_id: i64, to: Slot },
    /// Take `quantity` off a stack into a new item at `to`.
    ///
    /// Splitting the whole stack is a [`Edit::Move`] written the long way and
    /// is accepted as one, because a caller computing `quantity` from a UI
    /// drag should not have to special-case the boundary.
    Split {
        item_id: i64,
        quantity: i64,
        to: Slot,
    },
    /// Fold `from` into `into`. Both must be the same type, same address, and
    /// not singletons.
    Merge { from: i64, into: i64 },
    /// Consume part of a stack. `quantity` is what is *removed*.
    Consume { item_id: i64, quantity: i64 },
    /// Add `quantity` units to an existing stack: [`Edit::Consume`]'s
    /// inverse.
    ///
    /// **What a mining cycle is**: ore arrives in a hold that may already
    /// hold that ore, and a fresh stack per cycle would be a cargo window of
    /// forty Veldspar rows. `Create` cannot merge into an existing stack in
    /// the same plan -- its id does not exist until phase two -- so the
    /// caller picks: `Grow` the stack that is there, or `Create` the first
    /// one. Refused on a singleton (one object is not a stack) and on a
    /// count that would overflow.
    Grow { item_id: i64, quantity: i64 },
    /// Remove an item entirely.
    Destroy { item_id: i64 },
    /// Bring `quantity` units of `type_id` into existence at `to`.
    ///
    /// **The variant the two-phase API was missing.** Every other `Edit`
    /// needs an item that already exists, so a consume-and-produce operation
    /// -- a reprocess, a compression, an industry job's output, a mission
    /// reward -- could not be written as one plan, and splitting it across
    /// two calls is exactly the non-atomic saga `apply` exists to make
    /// unnecessary.
    ///
    /// [`Store::spawn_in`] is not a substitute: it clamps to `1..=1000`, forces
    /// `FLAG_HANGAR`, takes its type as a string, and sits outside `apply` --
    /// so it would cap a Veldspar refine at 1,000 Tritanium *and* break
    /// atomicity.
    ///
    /// Phase one validates the quantity and that the type is known; phase two
    /// mints and pushes, neither of which can fail.
    Create {
        type_id: i64,
        quantity: i64,
        to: Slot,
    },
    /// [`Edit::Create`], but packaged: a singleton type (a ship) comes as one
    /// packaged stack, not `quantity` assembled objects. What a market buy
    /// delivers -- the client names an assembled ship through
    /// `cfg.evelocations`, which a live `OnItemChange` never primes.
    CreatePackaged {
        type_id: i64,
        quantity: i64,
        to: Slot,
    },
    /// Unassemble an item: `singleton` true -> false.
    ///
    /// **The only `Edit` that changes an item's identity rather than its
    /// address or its count**, and the client treats that as a different
    /// thing: `singleton` is index 10 of our row, `godma.UpdateItem`'s skip3
    /// drops a packaged non-charge so a repackaged ship loses its dogma
    /// attributes, and `undockingSvc.ShipPicker` filters on it so a packaged
    /// ship stops being boardable. All three are the correct consequences of
    /// repackaging, which is why flipping one flag is the whole of it.
    ///
    /// What it does **not** do is move the item's contents out, although a
    /// real repackage does. That is a [`Edit::Move`] per content item, in the
    /// same plan, by the caller -- see `service/inventory.rs`'s
    /// `repackagingSvc` arm. Keeping it out here is deliberate: the store
    /// does not know which hangar the contents should land in, and a plan
    /// that moved them implicitly would be a plan whose `Change` list did not
    /// explain itself.
    Repackage { item_id: i64 },
    /// Take **one unit** of an item to `to`, assembled (`singleton` false ->
    /// true). The inverse of [`Edit::Repackage`], and what fitting a module
    /// and `ship.AssembleShip` both are (`docs/lanes/fitting.md` §5).
    ///
    /// **One unit, because an assembled item is one object.** `/createitem`
    /// grants modules as stacks, and `godma.UpdateItem`'s skip3 drops a
    /// non-singleton module from the HUD [V] -- so a fit is "take one off the
    /// stack, put it in the slot, assembled", and that is one edit rather
    /// than a `Split` followed by a flip the caller could forget.
    ///
    /// - quantity 1: the item itself moves (a [`Change::Moved`], only if the
    ///   address changed) and flips ([`Change::Assembled`]);
    /// - quantity N > 1: one unit is split off **as a new id** at `to`,
    ///   already a singleton ([`Change::QuantityChanged`] on the source, then
    ///   [`Change::Created`]).
    ///
    /// Refused on an item that is already a singleton, and on a type the
    /// table does not know -- the same reason `Create` refuses one.
    Assemble { item_id: i64, to: Slot },
}

/// What actually happened, in the order it happened.
///
/// Returned so the caller can push notifications **without re-deriving what
/// changed**, which is the bug this shape exists to prevent: five subsystems'
/// mutators discard their return value (`docs/archive/roadmap.md`), so the client
/// learns about an item only from a notification, and a notification built
/// from a second guess at the diff is a notification that drifts.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Moved {
        item_id: i64,
        from: Slot,
        to: Slot,
    },
    Created {
        item_id: i64,
        at: Slot,
    },
    QuantityChanged {
        item_id: i64,
        from: i64,
        to: i64,
    },
    Destroyed {
        item_id: i64,
        from: Slot,
        /// The item as it was just before it was removed, so a caller can
        /// tell the client its row left (`Registry::apply_items`).
        item: Box<Item>,
    },
    /// The item was unassembled. It did not move and its count did not change.
    ///
    /// Carries `at` anyway, so [`Change::slots`] has a location to invalidate:
    /// the whole of what the client learns about a repackage is the new row
    /// plus a dropped cache (`docs/reference/item-spawn.md` §2.1), and a `Change` that
    /// named no location would have made the repackage invisible until the
    /// next dock.
    Repackaged {
        item_id: i64,
        at: Slot,
    },
    /// The item was assembled where it now is: [`Edit::Assemble`] on a
    /// stack of one. A move, if there was one, is its own `Moved` before it.
    Assembled {
        item_id: i64,
        at: Slot,
    },
}

impl Change {
    /// Every address this change touched, for cache invalidation.
    ///
    /// A move touches two; everything else touches one, and
    /// `QuantityChanged` touches none it can name -- the item did not go
    /// anywhere, so the caller already knows where it is. Returning a `Vec`
    /// rather than a pair keeps the caller from having to know which is
    /// which: `InvalidateLocations` is idempotent, so an extra address is
    /// harmless and a missing one is a stale window.
    pub fn slots(&self) -> Vec<Slot> {
        match self {
            Change::Moved { from, to, .. } => vec![*from, *to],
            Change::Created { at, .. } => vec![*at],
            Change::Destroyed { from, .. } => vec![*from],
            Change::Repackaged { at, .. } | Change::Assembled { at, .. } => vec![*at],
            Change::QuantityChanged { .. } => Vec::new(),
        }
    }

    /// The item's columns *before* this change, as `(column index, old
    /// value)` -- the `change` dict of `OnItemChange`.
    ///
    /// **The client rebuilds the previous row from this dict and nothing
    /// else**, then asks of every open window "was it here, is it here"
    /// (`inventorysvc.py:126-140`); `invCache.py:1432` and
    /// `clientDogmaIM.py:83,86` do the same. Until 2026-10-24 every push said
    /// `{ixLocationID: 0}`, so a moved item never left its source window and
    /// an unfitted module never left the fitting window
    /// (`docs/lanes/fitting.md` §4). Indices from `carbon/common/lib/const`
    /// and `inventorycommon/const` **[V]**: owner 2, location 3, flag 4,
    /// stack size 9, singleton 10.
    pub fn previous_columns(&self) -> Vec<(i64, i64)> {
        match self {
            // A spawn has no previous state; location 0 is one that fails
            // `IsItemHere` everywhere, which is what makes it an arrival.
            Change::Created { .. } => vec![(IX_LOCATION_ID, 0)],
            Change::Moved { from, to, .. } => {
                let mut v = vec![(IX_LOCATION_ID, from.location_id), (IX_FLAG, from.flag_id)];
                if from.owner_id != to.owner_id {
                    v.push((IX_OWNER_ID, from.owner_id));
                }
                v
            }
            // `ixStackSize`, which both readers remap to `ixQuantity` [V].
            Change::QuantityChanged { from, .. } => vec![(IX_STACK_SIZE, *from)],
            Change::Repackaged { .. } => vec![(IX_SINGLETON, 1)],
            // The inverse: it was packaged (`docs/lanes/fitting.md` §4).
            Change::Assembled { .. } => vec![(IX_SINGLETON, 0)],
            // Not sent as a row (the snapshot goes through
            // `Registry::tell_destroyed`, which moves it to the junkyard), but
            // the old place is stated rather than left to a wildcard.
            Change::Destroyed { from, .. } => {
                vec![(IX_LOCATION_ID, from.location_id), (IX_FLAG, from.flag_id)]
            }
        }
    }

    /// The item this change is about.
    pub fn item_id(&self) -> i64 {
        match self {
            Change::Moved { item_id, .. }
            | Change::Created { item_id, .. }
            | Change::QuantityChanged { item_id, .. }
            | Change::Repackaged { item_id, .. }
            | Change::Assembled { item_id, .. }
            | Change::Destroyed { item_id, .. } => *item_id,
        }
    }
}

/// Why a plan was refused. **No edit in the plan has been applied.**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    NoSuchItem(i64),
    /// A quantity that is not a positive number of units the stack has.
    BadQuantity {
        item_id: i64,
        want: i64,
        have: i64,
    },
    /// `Merge` across types, addresses, or involving a singleton.
    NotMergeable {
        from: i64,
        into: i64,
        why: &'static str,
    },
    /// Two edits in one plan tried to change the same item.
    ///
    /// Refused rather than ordered. A plan is a set of intentions, and two
    /// intentions about one item mean the caller has not decided what it
    /// wants -- guessing an order here is how a "harmless" reordering turns
    /// into a lost stack later.
    Conflict(i64),
    /// `Create` for a type the type table does not know.
    ///
    /// Refused rather than defaulted. A row with an invented `groupID` or
    /// `categoryID` is **believed** by the client -- it builds a dogma item
    /// from it and renders the wrong thing with no error anywhere -- which is
    /// the argument this module's header makes about every other guess.
    UnknownType(i64),
    /// `Repackage` on something that cannot be unassembled.
    ///
    /// `why` is one of CCP's own reasons, read off
    /// `repackaging/__init__.py:46`-`:69`. The *key* the player sees is the
    /// caller's to choose, not the store's: a store that knew dialog names
    /// would be a store that had to be edited every time a window changed.
    NotRepackageable {
        item_id: i64,
        why: &'static str,
    },
    /// `Assemble` on something that is already one object.
    NotAssemblable {
        item_id: i64,
        why: &'static str,
    },
}

impl core::fmt::Display for EditError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EditError::NoSuchItem(id) => write!(f, "no item {id}"),
            EditError::BadQuantity {
                item_id,
                want,
                have,
            } => write!(f, "item {item_id} has {have}, not {want}"),
            EditError::NotMergeable { from, into, why } => {
                write!(f, "cannot merge {from} into {into}: {why}")
            }
            EditError::UnknownType(t) => write!(f, "no type {t} in the type table"),
            EditError::NotRepackageable { item_id, why } => {
                write!(f, "cannot repackage {item_id}: {why}")
            }
            EditError::NotAssemblable { item_id, why } => {
                write!(f, "cannot assemble {item_id}: {why}")
            }
            EditError::Conflict(id) => {
                write!(f, "two edits in one plan touch item {id}")
            }
        }
    }
}

/// Everything the world owns, and the allocator for new ids.
#[derive(Debug, Clone, Default)]
pub struct Store {
    items: Vec<Item>,
    next_id: i64,
    /// Where things are, derived from `items` (PERF-STAGES): built on the
    /// first lookup after a change that moved or removed an item, kept in
    /// step by the changes that only append or only change a quantity.
    /// **Derived, never a second truth**: every `&mut self` method that
    /// changes what the index records resets it (see [`Store::reindex`]).
    index: std::sync::OnceLock<ItemIndex>,
}

/// A multiply-shift hasher for the integer keys of [`ItemIndex`]: ids are
/// not attacker-chosen here, and SipHash dominated the lookups it serves.
#[derive(Default, Clone, Copy)]
pub(crate) struct IdHasher(u64);

impl std::hash::Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0.rotate_left(5) ^ u64::from(*b)).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }
    fn write_i64(&mut self, n: i64) {
        self.0 = (self.0 ^ n as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.0 ^= self.0 >> 29;
    }
}

pub(crate) type IdMap<V> =
    std::collections::HashMap<i64, V, std::hash::BuildHasherDefault<IdHasher>>;

/// Positions in `Store::items`: by item id (the first of a duplicated id,
/// as `find` answers) and by location (ascending, so a walk is in the
/// store's own order).
#[derive(Debug, Clone, Default)]
struct ItemIndex {
    by_id: IdMap<usize>,
    by_location: IdMap<Vec<usize>>,
}

impl core::fmt::Debug for IdHasher {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "IdHasher")
    }
}

impl ItemIndex {
    fn of(items: &[Item]) -> Self {
        let mut ix = ItemIndex::default();
        ix.by_id.reserve(items.len());
        for (pos, it) in items.iter().enumerate() {
            ix.push(pos, it);
        }
        ix
    }

    /// Record `it`, now at `pos` (the end of the store).
    fn push(&mut self, pos: usize, it: &Item) {
        self.by_id.entry(it.item_id).or_insert(pos);
        self.by_location
            .entry(it.location_id)
            .or_default()
            .push(pos);
    }
}

/// Why a spawn was refused.
///
/// Refusing with a reason rather than clamping: `/spawn` is a GM tool and the
/// person running it is the only one who can fix a bad request, so the one
/// thing it must never do is silently grant something else.
#[derive(Debug, Clone, PartialEq)]
pub enum SpawnError {
    UnknownType(String),
    /// A ship is one object. Asking for five in one stack is a shape the
    /// client cannot draw, and the honest answer is five separate items --
    /// which is what `spawn` does, so this is only returned for a quantity
    /// that makes no sense at all.
    BadQuantity(i64),
}

impl core::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SpawnError::UnknownType(s) => write!(
                f,
                "no such type '{s}' -- /createitem with no arguments lists \
                 the known ones"
            ),
            SpawnError::BadQuantity(n) => write!(f, "quantity {n} is not between 1 and 1000"),
        }
    }
}

impl Store {
    /// A store holding nothing, with the allocator primed.
    pub fn new() -> Self {
        Store {
            items: Vec::new(),
            next_id: MIN_PLAYER_ITEM,
            index: std::sync::OnceLock::new(),
        }
    }

    /// The position index, built on first use.
    fn ix(&self) -> &ItemIndex {
        self.index.get_or_init(|| ItemIndex::of(&self.items))
    }

    /// Forget the index: the next lookup rebuilds it. Called by everything
    /// that moves or removes an item.
    fn reindex(&mut self) {
        self.index = std::sync::OnceLock::new();
    }

    /// `item` was just pushed at the end: keep a built index true.
    fn indexed_push(&mut self) {
        let pos = self.items.len() - 1;
        if let Some(ix) = self.index.get_mut() {
            ix.push(pos, &self.items[pos]);
        }
    }

    /// Hand out the next item id.
    ///
    /// Monotonic and never reused *within a process*. Across a restart it
    /// starts over, which is a real hazard rather than a theoretical one --
    /// the client caches aggressively and a reused id hands it a stale cached
    /// object. `docs/reference/world-state.md` §2 item 4 names id sequences as world
    /// state in their own right, and the `Store` seam is where that gets
    /// fixed; until then the world dies with the process, so the client's
    /// cache dies with it too.
    fn mint(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The allocator's next value, for the snapshot.
    pub fn next_id(&self) -> i64 {
        self.next_id
    }

    /// A store read back from disk.
    ///
    /// The allocator is restored too, and that is the point: see `mint`'s
    /// comment for why reusing an item id across a restart hands a cached
    /// client a stale object. `max` guards against a snapshot whose allocator
    /// somehow trails its own items.
    pub fn restore(items: Vec<Item>, next_id: i64) -> Self {
        let highest = items.iter().map(|i| i.item_id).max().unwrap_or(0);
        Store {
            items,
            next_id: next_id.max(highest + 1).max(MIN_PLAYER_ITEM),
            index: std::sync::OnceLock::new(),
        }
    }

    /// Cubic metres used by everything at one location under one flag.
    ///
    /// `volume * quantity`, from the type table. A type the table does not
    /// know contributes **zero rather than a guess** -- understating the
    /// usage shows a hangar as emptier than it is, which is visible and
    /// harmless, where a guessed volume would be an arithmetic error the
    /// client propagates into every free-space calculation.
    ///
    /// Every type `content` knows counts ([`find_in`]): a PI commodity
    /// counts its own volume (tracker §77).
    pub fn used_volume_in(&self, content: &Content, location_id: i64, flag_id: i64) -> f64 {
        self.ix()
            .by_location
            .get(&location_id)
            .into_iter()
            .flatten()
            .map(|&pos| &self.items[pos])
            .filter(|i| i.flag_id == flag_id)
            .map(|i| find_in(content, i.type_id).map_or(0.0, |t| t.volume) * i.quantity as f64)
            .sum()
    }

    /// Everything one owner has, anywhere.
    pub fn owned_by(&self, owner_id: i64) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(move |i| i.owner_id == owner_id)
    }

    /// Find an item by id.
    pub fn get(&self, item_id: i64) -> Option<&Item> {
        let found = self.ix().by_id.get(&item_id).map(|&pos| &self.items[pos]);
        debug_assert_eq!(
            found.map(|i| i as *const Item),
            self.items
                .iter()
                .find(|i| i.item_id == item_id)
                .map(|i| i as *const Item),
            "item index out of step for {item_id}"
        );
        found
    }

    /// Everything at one address.
    pub fn at(&self, slot: Slot) -> impl Iterator<Item = &Item> {
        self.ix()
            .by_location
            .get(&slot.location_id)
            .into_iter()
            .flatten()
            .map(move |&pos| &self.items[pos])
            .filter(move |i| i.slot() == slot)
    }

    /// Everything *inside* one item, at any flag.
    ///
    /// An item's contents are the items whose `location_id` is its id -- the
    /// one address space, per [`Slot`]'s comment, so a fitted module and a
    /// crate's contents are found the same way. Wider than [`Store::at`] on
    /// purpose: a ship's contents span the cargo hold, every fitting slot and
    /// the drone bay, and a repackage has to account for all of them.
    pub fn contents_of(&self, item_id: i64) -> impl Iterator<Item = &Item> {
        self.ix()
            .by_location
            .get(&item_id)
            .into_iter()
            .flatten()
            .map(move |&pos| &self.items[pos])
    }

    /// The merges that would stack everything at one address.
    ///
    /// This is "Stack All" as a plan: group the items at `slot` by type, and
    /// for each group of two or more fold every member into the one with the
    /// **lowest item id**. Feed the result to [`Store::apply_in`] and the whole
    /// thing is one atomic operation with one `Change` list.
    ///
    /// Three rules, each from the client:
    ///
    /// - **Singletons are excluded.** `singleton` means "this is one assembled
    ///   object"; two of them are not interchangeable units and
    ///   `docs/reference/item-spawn.md` §6.2 refuses `singleton` with `stacksize > 1`
    ///   outright. [`Store::apply_in`] would refuse such a merge anyway -- this
    ///   filter is what keeps us from *proposing* it.
    /// - **One group, one survivor.** `invCacheContainer.StackAll`
    ///   (`invCache.py:1141`) only calls the server at all once it has seen
    ///   two unlocked, non-negative items sharing a `typeID`, so a plan that
    ///   merged pairwise and left a second survivor would disagree with the
    ///   client's own test.
    /// - **The oldest id survives**, rather than the biggest stack. Both are
    ///   defensible and the choice has to be *a* rule, because the merged-away
    ///   ids are gone from the client's cache and the survivor's is the one it
    ///   keeps; lowest-id is the only one of the two that does not change
    ///   answer when the quantities do.
    ///
    /// An empty plan means there was nothing to stack, which is a legitimate
    /// answer and not an error.
    pub fn stack_plan(&self, slot: Slot) -> Vec<Edit> {
        // `(type_id, survivor_id, sources)`, in first-seen order so the plan
        // is reproducible. A `Vec` rather than a map because a hangar holds
        // tens of items, not thousands, and the ordering is worth more here
        // than the asymptotics.
        let mut groups: Vec<(i64, Vec<i64>)> = Vec::new();
        for item in self.items.iter().filter(|i| i.slot() == slot) {
            if item.singleton {
                continue;
            }
            match groups.iter_mut().find(|(t, _)| *t == item.type_id) {
                Some((_, ids)) => ids.push(item.item_id),
                None => groups.push((item.type_id, vec![item.item_id])),
            }
        }
        let mut plan = Vec::new();
        for (_, mut ids) in groups {
            if ids.len() < 2 {
                continue;
            }
            ids.sort_unstable();
            let into = ids[0];
            for from in ids.into_iter().skip(1) {
                plan.push(Edit::Merge { from, into });
            }
        }
        plan
    }

    /// Apply every edit in the plan, or none of them.
    ///
    /// Two phases, and the split is the whole point. **Phase one resolves**
    /// every edit against the current store into a `Resolved` form holding
    /// indices and pre-computed numbers, returning `Err` on the first thing
    /// that does not add up. **Phase two performs** the resolved list, and
    /// cannot fail -- there is no fallible operation left in it, no `?`, and
    /// nothing to roll back.
    ///
    /// That is why this needs no undo log, no receipt table and no
    /// compensating saga: a plan that would fail halfway never starts. The
    /// cost is that phase one must be complete enough that phase two is
    /// genuinely infallible, so every index, quantity and merge check is
    /// settled before a single byte of the store changes.
    ///
    /// Capacity is **not** checked here. Container volume is policy that needs
    /// the type table and the container's own capacity
    /// (`Registry::container_capacity`), and a store that silently enforced
    /// it would refuse GM edits that are supposed to work. Callers that care
    /// check [`Store::used_volume_in`] first. That is a deliberate line, not an
    /// omission.
    ///
    /// The types are every one `content` holds ([`find_in`]): what
    /// `Registry::apply_items` calls (CP58b).
    pub fn apply_in(
        &mut self,
        content: &Content,
        edits: &[Edit],
    ) -> Result<Vec<Change>, EditError> {
        self.apply_by(|id| find_in(content, id).copied(), edits)
    }

    /// The two phases, with `lookup` as the type table.
    fn apply_by(
        &mut self,
        lookup: impl Fn(i64) -> Option<ItemType>,
        edits: &[Edit],
    ) -> Result<Vec<Change>, EditError> {
        enum Resolved {
            Move {
                idx: usize,
                to: Slot,
            },
            Split {
                idx: usize,
                quantity: i64,
                to: Slot,
            },
            Merge {
                from: usize,
                into: usize,
                quantity: i64,
            },
            Consume {
                idx: usize,
                quantity: i64,
            },
            Grow {
                idx: usize,
                quantity: i64,
            },
            Destroy {
                idx: usize,
            },
            Create {
                t: ItemType,
                quantity: i64,
                to: Slot,
                packaged: bool,
            },
            Repackage {
                idx: usize,
            },
            Assemble {
                idx: usize,
                to: Slot,
            },
        }

        // A free function over the slice rather than a closure over `self`:
        // a closure would hold a borrow of `self` that phase two needs
        // mutably, and the resulting error points at the loop rather than at
        // the closure that caused it.
        fn index_of(items: &[Item], id: i64) -> Result<usize, EditError> {
            items
                .iter()
                .position(|i| i.item_id == id)
                .ok_or(EditError::NoSuchItem(id))
        }

        // One edit per item, and `Split` counts against its source. Collected
        // as we resolve so the conflict is reported with the id rather than
        // "something overlapped".
        let mut touched: Vec<i64> = Vec::with_capacity(edits.len());
        // **The one shared claim, and "Stack All" is why.**
        //
        // Stacking a hangar with three stacks of EMP S is two merges into one
        // survivor, and the exclusive `claim` below refused that as
        // `Conflict` -- "two edits in one plan touch item X" -- which was
        // true of the letter and wrong about the plan: two merges into one
        // target are not two intentions about one item, they are one
        // intention written per source. The alternative was one `apply` per
        // pair, which is exactly the non-atomic saga this method exists to
        // make unnecessary, and would have emitted one push per pair.
        //
        // So a merge *target* may be claimed any number of times, and the
        // exclusivity that matters is kept: a target may not also be a
        // source, moved, split, consumed, destroyed or repackaged in the same
        // plan, because `claim` refuses an id already held as a target and
        // `claim_merge_target` refuses one already held exclusively.
        let mut merge_targets: Vec<i64> = Vec::new();
        fn claim(id: i64, touched: &mut Vec<i64>, merge_targets: &[i64]) -> Result<(), EditError> {
            if touched.contains(&id) || merge_targets.contains(&id) {
                return Err(EditError::Conflict(id));
            }
            touched.push(id);
            Ok(())
        }
        fn claim_merge_target(
            id: i64,
            touched: &[i64],
            merge_targets: &mut Vec<i64>,
        ) -> Result<(), EditError> {
            if touched.contains(&id) {
                return Err(EditError::Conflict(id));
            }
            if !merge_targets.contains(&id) {
                merge_targets.push(id);
            }
            Ok(())
        }

        let mut plan: Vec<Resolved> = Vec::with_capacity(edits.len());
        for edit in edits {
            match *edit {
                Edit::Move { item_id, to } => {
                    claim(item_id, &mut touched, &merge_targets)?;
                    plan.push(Resolved::Move {
                        idx: index_of(&self.items, item_id)?,
                        to,
                    });
                }
                Edit::Split {
                    item_id,
                    quantity,
                    to,
                } => {
                    claim(item_id, &mut touched, &merge_targets)?;
                    let idx = index_of(&self.items, item_id)?;
                    let have = self.items[idx].quantity;
                    if quantity <= 0 || quantity > have {
                        return Err(EditError::BadQuantity {
                            item_id,
                            want: quantity,
                            have,
                        });
                    }
                    // Splitting the whole stack is a move. Accepted rather
                    // than refused: a caller computing this from a drag
                    // should not have to special-case the boundary.
                    if quantity == have {
                        plan.push(Resolved::Move { idx, to });
                    } else {
                        plan.push(Resolved::Split { idx, quantity, to });
                    }
                }
                Edit::Merge { from, into } => {
                    // Before `claim`, deliberately. A merge of an item into
                    // itself is ONE edit naming one item twice, and `claim`
                    // would report it as `Conflict` -- "two edits touch this
                    // item" -- which describes a plan the caller did not
                    // write. An error should name what the caller did wrong.
                    // Found by the test below, which is why it enumerates
                    // every way a merge can be wrong rather than the three
                    // obvious ones.
                    if from == into {
                        return Err(EditError::NotMergeable {
                            from,
                            into,
                            why: "an item cannot merge into itself",
                        });
                    }
                    claim(from, &mut touched, &merge_targets)?;
                    claim_merge_target(into, &touched, &mut merge_targets)?;
                    let fi = index_of(&self.items, from)?;
                    let ii = index_of(&self.items, into)?;
                    let (a, b) = (&self.items[fi], &self.items[ii]);
                    let why = if a.type_id != b.type_id {
                        Some("different types")
                    } else if a.slot() != b.slot() {
                        Some("different locations")
                    } else if a.singleton || b.singleton {
                        Some("singletons do not stack")
                    } else {
                        None
                    };
                    if let Some(why) = why {
                        return Err(EditError::NotMergeable { from, into, why });
                    }
                    plan.push(Resolved::Merge {
                        from: fi,
                        into: ii,
                        quantity: a.quantity,
                    });
                }
                Edit::Consume { item_id, quantity } => {
                    claim(item_id, &mut touched, &merge_targets)?;
                    let idx = index_of(&self.items, item_id)?;
                    let have = self.items[idx].quantity;
                    if quantity <= 0 || quantity > have {
                        return Err(EditError::BadQuantity {
                            item_id,
                            want: quantity,
                            have,
                        });
                    }
                    if quantity == have {
                        plan.push(Resolved::Destroy { idx });
                    } else {
                        plan.push(Resolved::Consume { idx, quantity });
                    }
                }
                Edit::Grow { item_id, quantity } => {
                    claim(item_id, &mut touched, &merge_targets)?;
                    let idx = index_of(&self.items, item_id)?;
                    let it = &self.items[idx];
                    if it.singleton {
                        return Err(EditError::NotMergeable {
                            from: item_id,
                            into: item_id,
                            why: "singletons do not stack",
                        });
                    }
                    if quantity <= 0 || it.quantity.checked_add(quantity).is_none() {
                        return Err(EditError::BadQuantity {
                            item_id,
                            want: quantity,
                            have: it.quantity,
                        });
                    }
                    plan.push(Resolved::Grow { idx, quantity });
                }
                Edit::Destroy { item_id } => {
                    claim(item_id, &mut touched, &merge_targets)?;
                    plan.push(Resolved::Destroy {
                        idx: index_of(&self.items, item_id)?,
                    });
                }
                Edit::Create {
                    type_id,
                    quantity,
                    to,
                }
                | Edit::CreatePackaged {
                    type_id,
                    quantity,
                    to,
                } => {
                    let packaged = matches!(edit, Edit::CreatePackaged { .. });
                    // **No `claim`**, deliberately: a `Create` names no
                    // existing item, so it cannot collide with another edit,
                    // and two `Create`s of one type in one plan are two
                    // stacks -- which a caller may legitimately want and
                    // `Conflict` would wrongly refuse.
                    let t = lookup(type_id).ok_or(EditError::UnknownType(type_id))?;
                    if quantity <= 0 {
                        return Err(EditError::BadQuantity {
                            item_id: 0,
                            want: quantity,
                            have: 0,
                        });
                    }
                    plan.push(Resolved::Create {
                        t,
                        quantity,
                        to,
                        packaged,
                    });
                }
                Edit::Repackage { item_id } => {
                    claim(item_id, &mut touched, &merge_targets)?;
                    let idx = index_of(&self.items, item_id)?;
                    let it = &self.items[idx];
                    // `CheckCanRepackage` in order, minus the two tests that
                    // need state we do not have: `IsNPC(ownerID)` --
                    // `9999 < ownerID < 90000000`, and every owner here is a
                    // player character at 90000001 or above
                    // (`MIN_PLAYER_ITEM`'s comment, same id bands) -- and the
                    // damage tests, because no item in this server has
                    // `armorDamage` or `damage` to be non-zero.
                    let why = if !it.singleton {
                        // `RepackageItemNotSingleton`. Reported rather than
                        // ignored: whether "already packaged" is an error or
                        // a no-op is the *caller's* question -- "Repackage"
                        // over a mixed selection wants to skip it, a single
                        // deliberate repackage wants to be told -- and the
                        // store cannot tell which it is.
                        Some("it is already packaged")
                    } else if is_ship_fitting_flag(it.flag_id) {
                        // `CanRepackageInLocation`'s first test.
                        Some("it is fitted")
                    } else if !can_repackage_type(it.group_id, it.category_id) {
                        // `CanRepackageType`. This is the arm a capsule takes.
                        Some("this type cannot be unassembled")
                    } else {
                        None
                    };
                    if let Some(why) = why {
                        return Err(EditError::NotRepackageable { item_id, why });
                    }
                    plan.push(Resolved::Repackage { idx });
                }
                Edit::Assemble { item_id, to } => {
                    claim(item_id, &mut touched, &merge_targets)?;
                    let idx = index_of(&self.items, item_id)?;
                    let it = &self.items[idx];
                    if it.singleton {
                        return Err(EditError::NotAssemblable {
                            item_id,
                            why: "it is already assembled",
                        });
                    }
                    if lookup(it.type_id).is_none() {
                        return Err(EditError::UnknownType(it.type_id));
                    }
                    // A stack of zero is not a thing to take one unit from;
                    // the store never makes one, and refusing here keeps
                    // phase two's `quantity - 1` honest.
                    if it.quantity < 1 {
                        return Err(EditError::BadQuantity {
                            item_id,
                            want: 1,
                            have: it.quantity,
                        });
                    }
                    plan.push(Resolved::Assemble { idx, to });
                }
            }
        }

        // ---- phase two: nothing below here can fail ----
        //
        // Removals are collected and done last, in descending index order, so
        // that no index resolved in phase one is invalidated while the rest of
        // the plan is still being performed. Mutating in place first and
        // deleting afterwards is what keeps phase two index-stable.
        let mut changes = Vec::with_capacity(plan.len());
        let mut remove: Vec<usize> = Vec::new();
        let mut minted: Vec<Item> = Vec::new();

        for step in &plan {
            match *step {
                Resolved::Move { idx, to } => {
                    let it = &mut self.items[idx];
                    let from = it.slot();
                    it.owner_id = to.owner_id;
                    it.location_id = to.location_id;
                    it.flag_id = to.flag_id;
                    changes.push(Change::Moved {
                        item_id: it.item_id,
                        from,
                        to,
                    });
                }
                Resolved::Split { idx, quantity, to } => {
                    let src = self.items[idx].clone();
                    let was = src.quantity;
                    self.items[idx].quantity = was - quantity;
                    changes.push(Change::QuantityChanged {
                        item_id: src.item_id,
                        from: was,
                        to: was - quantity,
                    });
                    let item_id = self.mint();
                    minted.push(Item {
                        item_id,
                        type_id: src.type_id,
                        owner_id: to.owner_id,
                        location_id: to.location_id,
                        flag_id: to.flag_id,
                        quantity,
                        group_id: src.group_id,
                        category_id: src.category_id,
                        singleton: src.singleton,
                    });
                    changes.push(Change::Created { item_id, at: to });
                }
                Resolved::Merge {
                    from,
                    into,
                    quantity,
                } => {
                    let gone = self.items[from].item_id;
                    let at = self.items[from].slot();
                    let snapshot = Box::new(self.items[from].clone());
                    let was = self.items[into].quantity;
                    let now = was + quantity;
                    self.items[into].quantity = now;
                    let into_id = self.items[into].item_id;
                    // **One `QuantityChanged` per survivor, not one per
                    // merge.** Three stacks folding into one is two merges,
                    // and pushing a change each would report
                    // `100 -> 150` and `150 -> 200` for the same item -- two
                    // rows on the wire for one item and, worse, a `from`
                    // that is an intermediate the client never saw. Callers
                    // build pushes straight off this list (`Registry::apply_items`),
                    // so the list has to describe the net effect.
                    let mut folded = false;
                    for c in changes.iter_mut() {
                        if let Change::QuantityChanged { item_id, to, .. } = c
                            && *item_id == into_id
                        {
                            *to = now;
                            folded = true;
                            break;
                        }
                    }
                    if !folded {
                        changes.push(Change::QuantityChanged {
                            item_id: into_id,
                            from: was,
                            to: now,
                        });
                    }
                    changes.push(Change::Destroyed {
                        item_id: gone,
                        from: at,
                        item: snapshot,
                    });
                    remove.push(from);
                }
                Resolved::Consume { idx, quantity } => {
                    let it = &mut self.items[idx];
                    let was = it.quantity;
                    it.quantity = was - quantity;
                    changes.push(Change::QuantityChanged {
                        item_id: it.item_id,
                        from: was,
                        to: was - quantity,
                    });
                }
                Resolved::Grow { idx, quantity } => {
                    let it = &mut self.items[idx];
                    let was = it.quantity;
                    it.quantity = was + quantity;
                    changes.push(Change::QuantityChanged {
                        item_id: it.item_id,
                        from: was,
                        to: was + quantity,
                    });
                }
                Resolved::Destroy { idx } => {
                    let it = &self.items[idx];
                    changes.push(Change::Destroyed {
                        item_id: it.item_id,
                        from: it.slot(),
                        item: Box::new(it.clone()),
                    });
                    remove.push(idx);
                }
                Resolved::Create {
                    t,
                    quantity,
                    to,
                    packaged,
                } => {
                    // A singleton type is `quantity` separate objects, the
                    // same rule `spawn` applies and for the same reason: the
                    // client has no rendering for a stack of ships.
                    let singleton = t.singleton && !packaged;
                    let (runs, per) = if singleton {
                        (quantity, 1)
                    } else {
                        (1, quantity)
                    };
                    for _ in 0..runs {
                        let item_id = self.mint();
                        // **`minted`, not `self.items`** -- the same reason
                        // `Split` uses it. Phase one resolved indices into
                        // `self.items`, and pushing here would be harmless
                        // only until the first plan that also removes
                        // something. Appended after the removals, below.
                        minted.push(Item {
                            item_id,
                            type_id: t.type_id,
                            owner_id: to.owner_id,
                            location_id: to.location_id,
                            flag_id: to.flag_id,
                            quantity: per,
                            group_id: t.group_id,
                            category_id: t.category_id,
                            singleton,
                        });
                        changes.push(Change::Created { item_id, at: to });
                    }
                }
                Resolved::Repackage { idx } => {
                    let it = &mut self.items[idx];
                    it.singleton = false;
                    changes.push(Change::Repackaged {
                        item_id: it.item_id,
                        at: it.slot(),
                    });
                }
                Resolved::Assemble { idx, to } if self.items[idx].quantity == 1 => {
                    let it = &mut self.items[idx];
                    let from = it.slot();
                    if from != to {
                        it.owner_id = to.owner_id;
                        it.location_id = to.location_id;
                        it.flag_id = to.flag_id;
                        changes.push(Change::Moved {
                            item_id: it.item_id,
                            from,
                            to,
                        });
                    }
                    it.singleton = true;
                    changes.push(Change::Assembled {
                        item_id: it.item_id,
                        at: to,
                    });
                }
                Resolved::Assemble { idx, to } => {
                    // One unit off the stack, as a new object. `minted`, for
                    // the reason `Split` gives.
                    let src = self.items[idx].clone();
                    self.items[idx].quantity = src.quantity - 1;
                    changes.push(Change::QuantityChanged {
                        item_id: src.item_id,
                        from: src.quantity,
                        to: src.quantity - 1,
                    });
                    let item_id = self.mint();
                    minted.push(Item {
                        item_id,
                        type_id: src.type_id,
                        owner_id: to.owner_id,
                        location_id: to.location_id,
                        flag_id: to.flag_id,
                        quantity: 1,
                        group_id: src.group_id,
                        category_id: src.category_id,
                        singleton: true,
                    });
                    changes.push(Change::Created { item_id, at: to });
                }
            }
        }

        remove.sort_unstable();
        remove.dedup();
        // Only a removal or a change of location invalidates the positions;
        // a quantity change leaves them true and a mint appends.
        let moved = !remove.is_empty()
            || plan
                .iter()
                .any(|s| matches!(s, Resolved::Move { .. } | Resolved::Assemble { .. }));
        for idx in remove.into_iter().rev() {
            self.items.remove(idx);
        }
        if moved {
            self.reindex();
        }
        for it in minted {
            self.items.push(it);
            self.indexed_push();
        }
        Ok(changes)
    }

    /// Put an existing item in, with its id already chosen.
    ///
    /// Used to seed the capsule, whose id is the character's `ship_id` and
    /// therefore not ours to allocate.
    ///
    /// **The allocator moves past it, and that is not tidiness.** This method
    /// pushed the item and left `next_id` alone, so a world seeded with a
    /// capsule at `ship_id` 100000001 -- which is [`MIN_PLAYER_ITEM`] + 1, the
    /// fixture's own value -- handed out **100000001 again** on the second
    /// [`Store::mint`] of its life. Two items, one id.
    ///
    /// Nothing had caught it because nothing had minted twice in a world that
    /// also had a capsule: `/giveitem` mints once per call, and every test
    /// that mints repeatedly builds a bare `Store::new()` with no seeded ship.
    /// The first thing to reach it was a manufacturing job, which mints a
    /// blueprint and a mineral stack in one `apply` -- and the second id came
    /// back pointing at the player's capsule, so the job happily planned to
    /// consume **the ship** as Tritanium. `Store::get` returns the first match
    /// for a duplicated id, so every later operation on 100000001 was a coin
    /// flip between the two.
    ///
    /// Reprocessing was one step away from the same thing in a real client: it
    /// creates one output item per material type, so any refine yielding two
    /// or more minerals would have duplicated an id silently.
    ///
    /// Same `max(id + 1)` guard as [`Store::restore`], which had it from the
    /// start and for exactly this reason.
    pub fn insert(&mut self, item: Item) {
        self.next_id = self.next_id.max(item.item_id + 1);
        self.items.push(item);
        self.indexed_push();
    }

    /// Grant `quantity` of a type to an owner, in their hangar.
    ///
    /// A singleton type becomes `quantity` separate items, because that is
    /// what a singleton means; a stackable type becomes one item with a
    /// quantity. Returns the ids granted, newest last.
    ///
    /// The type is looked up in `content` ([`find_type_in`]) and comes back
    /// by value: it lives in `content`, not in a static.
    pub fn spawn_in(
        &mut self,
        content: &Content,
        type_id_or_name: &str,
        quantity: i64,
        owner_id: i64,
        location_id: i64,
    ) -> Result<(Vec<i64>, ItemType), SpawnError> {
        let t = *find_type_in(content, type_id_or_name)
            .ok_or_else(|| SpawnError::UnknownType(type_id_or_name.to_owned()))?;
        let ids = self.grant(&t, quantity, owner_id, location_id)?;
        Ok((ids, t))
    }

    /// `quantity` of `t` into `owner_id`'s hangar at `location_id`: one item
    /// per unit for a singleton, one stack otherwise.
    fn grant(
        &mut self,
        t: &ItemType,
        quantity: i64,
        owner_id: i64,
        location_id: i64,
    ) -> Result<Vec<i64>, SpawnError> {
        if !(1..=1000).contains(&quantity) {
            return Err(SpawnError::BadQuantity(quantity));
        }
        let mut ids = Vec::new();
        let runs = if t.singleton { quantity } else { 1 };
        let per = if t.singleton { 1 } else { quantity };
        for _ in 0..runs {
            let item_id = self.mint();
            self.items.push(Item {
                item_id,
                type_id: t.type_id,
                owner_id,
                location_id,
                flag_id: FLAG_HANGAR,
                quantity: per,
                group_id: t.group_id,
                category_id: t.category_id,
                singleton: t.singleton,
            });
            self.indexed_push();
            ids.push(item_id);
        }
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    /// CAP-B: the hold table is `inventoryFlagData`'s [V]: one row per flag,
    /// the fuel bay's capacity is attribute 1549 and it takes the Ice
    /// Product group only; the mining hold's row agrees with
    /// `crate::mining_hold::accepts`; a flag the table does not model gives
    /// no answer.
    #[test]
    fn the_hold_table_reads_as_the_clients() {
        for (n, h) in HOLDS.iter().enumerate() {
            assert!(
                HOLDS[n + 1..].iter().all(|o| o.flag != h.flag),
                "{}",
                h.flag
            );
        }
        assert_eq!(
            hold_of(FLAG_FUEL_BAY).map(|h| h.attribute),
            Some(ATTR_FUEL_BAY_CAPACITY)
        );
        assert_eq!(
            hold_accepts(FLAG_FUEL_BAY, 17, GROUP_ICE_PRODUCT),
            Some(true)
        );
        assert_eq!(
            hold_accepts(FLAG_FUEL_BAY, 25, GROUP_ICE_PRODUCT + 1),
            Some(false)
        );
        assert_eq!(hold_accepts(5, 1, 1), Some(true), "cargo takes anything");
        assert_eq!(
            hold_accepts(87, 18, 1),
            Some(true),
            "drones in the drone bay"
        );
        assert_eq!(hold_accepts(87, 8, 1), Some(false));
        assert_eq!(
            hold_accepts(90, 6, 29),
            Some(false),
            "no capsule in a maintenance bay"
        );
        assert_eq!(hold_accepts(90, 6, 25), Some(true));
        assert_eq!(hold_accepts(999, 1, 1), None);
        assert_eq!(
            hold_accepts(143, 8, 1),
            None,
            "an allowTypes row is not modelled"
        );
        for (cat, group) in [
            (25, 1),
            (25, 4094),
            (25, 711),
            (17, 711),
            (17, 4915),
            (6, 1),
        ] {
            assert_eq!(
                hold_accepts(crate::mining_hold::FLAG_GENERAL_MINING_HOLD, cat, group),
                Some(crate::mining_hold::accepts(cat, group)),
                "({cat}, {group})"
            );
        }
    }

    use super::*;

    /// The invented content every store test reads its types from: the
    /// hand rows, then the fixture's PI, skill and typedata tables.
    fn fx() -> Content {
        Content::fixture()
    }

    // ---- CP58b: lookups through `Content` ---------------------------------

    /// A PI type of the fixture content with a volume, and a typedata-only
    /// type (one no hand row has).
    fn content_types(c: &Content) -> (ItemType, ItemType) {
        let pi = *crate::planet::item_types_in(c)
            .iter()
            .find(|t| {
                t.volume > 0.0
                    && SPAWNABLE
                        .iter()
                        .chain(WRECK_TYPES)
                        .chain(COURIER_TYPES)
                        .chain(crate::skill::item_types_in(c))
                        .all(|h| h.type_id != t.type_id)
            })
            .expect("the PI fixture has a commodity with a volume");
        let td = *crate::typedata::installed_in(c)
            .table()
            .types()
            .filter(|r| r.published && SPAWNABLE.iter().all(|h| h.type_id != r.type_id))
            .find_map(|r| crate::typedata::item_in(c, r.type_id))
            .expect("the typedata fixture has a published item");
        (pi, td)
    }

    /// `find_in` knows the hand rows, PI's types, the skillbooks and the
    /// content's typedata; the hand row wins a shared id.
    #[test]
    fn find_in_sees_every_content_table() {
        let c = Content::fixture();
        let (pi, td) = content_types(&c);
        assert_eq!(find_in(&c, pi.type_id), Some(&pi));
        assert_eq!(
            find_in(&c, td.type_id),
            crate::typedata::item_in(&c, td.type_id)
        );
        for h in SPAWNABLE.iter().chain(WRECK_TYPES).chain(COURIER_TYPES) {
            assert_eq!(find_in(&c, h.type_id), Some(h));
        }
        for book in crate::skill::item_types_in(&c) {
            assert!(find_in(&c, book.type_id).is_some());
        }
        assert!(find_in(&c, -1).is_none());
    }

    /// `find_type_in` grants by id or name, case-insensitively, from the
    /// same tables; a wreck is never granted.
    #[test]
    fn find_type_in_grants_by_id_or_name() {
        let c = Content::fixture();
        let (pi, td) = content_types(&c);
        assert_eq!(find_type_in(&c, &pi.type_id.to_string()), Some(&pi));
        assert_eq!(
            find_type_in(&c, &td.name.to_ascii_uppercase()).map(|t| t.type_id),
            Some(td.type_id)
        );
        assert_eq!(
            find_type_in(&c, &td.type_id.to_string()).map(|t| t.type_id),
            Some(td.type_id)
        );
        for w in WRECK_TYPES {
            assert!(find_type_in(&c, &w.type_id.to_string()).is_none());
        }
        assert!(find_type_in(&c, "").is_none());
    }

    /// Tracker §77 closed for a store reader with content: a PI commodity
    /// counts its own volume.
    #[test]
    fn used_volume_in_counts_a_pi_commodity() {
        let c = Content::fixture();
        let (pi, _) = content_types(&c);
        let mut store = Store::new();
        let to = Slot::new(90_000_001, 60_000_001, FLAG_HANGAR);
        store
            .apply_in(
                &c,
                &[Edit::Create {
                    type_id: pi.type_id,
                    quantity: 3,
                    to,
                }],
            )
            .expect("apply_in knows the PI type");
        assert_eq!(
            store.used_volume_in(&c, to.location_id, to.flag_id),
            pi.volume * 3.0
        );
    }

    /// `spawn_in` grants a type only the content's typedata has.
    #[test]
    fn spawn_in_grants_a_content_type() {
        let c = Content::fixture();
        let (_, td) = content_types(&c);
        let mut store = Store::new();
        let (ids, t) = store
            .spawn_in(&c, &td.name.to_ascii_lowercase(), 2, 90_000_001, 60_000_001)
            .expect("granted");
        assert_eq!(t, td);
        let runs = if td.singleton { 2 } else { 1 };
        assert_eq!(ids.len(), runs);
        assert!(
            ids.iter()
                .all(|id| store.get(*id).map(|i| i.type_id) == Some(td.type_id))
        );
        assert!(matches!(
            store.spawn_in(&c, "no such type at all", 1, 1, 1),
            Err(SpawnError::UnknownType(_))
        ));
    }

    #[test]
    fn a_ship_spawns_as_separate_objects_and_ammo_as_one_stack() {
        let mut s = Store::new();
        let (ships, t) = s
            .spawn_in(&fx(), "Rifter", 3, 90000001, 60012526)
            .expect("rifter");
        assert_eq!(ships.len(), 3, "a singleton type is N objects, not a stack");
        assert!(t.singleton);
        assert!(s.items().iter().all(|i| i.quantity == 1));

        let (ammo, t) = s
            .spawn_in(&fx(), "EMP S", 500, 90000001, 60012526)
            .expect("ammo");
        assert_eq!(ammo.len(), 1, "a stackable type is one item");
        assert!(!t.singleton);
        let stack = s
            .items()
            .iter()
            .find(|i| i.item_id == ammo[0])
            .expect("found");
        assert_eq!(stack.quantity, 500);
    }

    #[test]
    fn every_id_is_a_player_item_and_none_repeats() {
        let mut s = Store::new();
        let (ids, _) = s
            .spawn_in(&fx(), "587", 10, 90000001, 60012526)
            .expect("by id");
        assert!(
            ids.iter().all(|&id| id >= MIN_PLAYER_ITEM),
            "an id below minPlayerItem is routed to the client's own sqlite \
             and never reaches us: {ids:?}"
        );
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "ids must not repeat");
    }

    #[test]
    fn the_group_and_category_come_from_the_table_not_from_the_caller() {
        let mut s = Store::new();
        s.spawn_in(&fx(), "Rifter", 1, 90000001, 60012526)
            .expect("rifter");
        let i = &s.items()[0];
        // Published EVE values, and the reason the table exists: the client
        // believes these rather than deriving them.
        assert_eq!(
            (i.group_id, i.category_id),
            (25, crate::model::CATEGORY_SHIP)
        );
    }

    #[test]
    fn a_bad_request_is_refused_with_a_reason_rather_than_clamped() {
        let mut s = Store::new();
        assert!(matches!(
            s.spawn_in(&fx(), "Nyx", 1, 1, 1),
            Err(SpawnError::UnknownType(_))
        ));
        assert!(matches!(
            s.spawn_in(&fx(), "Rifter", 0, 1, 1),
            Err(SpawnError::BadQuantity(0))
        ));
        assert!(matches!(
            s.spawn_in(&fx(), "Rifter", 9999, 1, 1),
            Err(SpawnError::BadQuantity(9999))
        ));
        assert!(s.items().is_empty(), "a refused spawn grants nothing");
    }

    #[test]
    fn a_type_resolves_by_id_and_by_name_whatever_the_case() {
        let c = Content::fixture();
        let find_type = |needle| find_type_in(&c, needle);
        assert_eq!(find_type("587"), find_type("rifter"));
        assert_eq!(find_type("RIFTER"), find_type("Rifter"));
        assert!(find_type("").is_none());
    }

    // ---- the mutation API ----

    const HANGAR: i64 = 60_014_437;

    fn stack(store: &mut Store, type_id: i64, quantity: i64, slot: Slot) -> i64 {
        let id = store.mint();
        store.insert(Item {
            item_id: id,
            type_id,
            owner_id: slot.owner_id,
            location_id: slot.location_id,
            flag_id: slot.flag_id,
            quantity,
            group_id: 0,
            category_id: 0,
            singleton: false,
        });
        id
    }

    fn hangar() -> Slot {
        Slot::new(1, HANGAR, 4)
    }

    /// **The position index equals a scan after every kind of edit**
    /// (PERF-STAGES): `get`, `contents_of` and `at` answer from it, and a
    /// move, a destroy, a merge, a grow, a split and a create each leave it
    /// true, with the store's own order kept.
    #[test]
    fn the_position_index_matches_a_scan_after_every_edit() {
        fn check(s: &Store, probes: &[i64]) {
            for id in probes.iter().copied().chain([7, 999_999_999]) {
                let scan = s.items().iter().find(|i| i.item_id == id);
                assert_eq!(s.get(id), scan, "get {id}");
                let scan: Vec<&Item> = s.items().iter().filter(|i| i.location_id == id).collect();
                assert_eq!(s.contents_of(id).collect::<Vec<_>>(), scan, "contents {id}");
            }
            for slot in [hangar(), Slot::new(1, HANGAR, 5), Slot::new(2, HANGAR, 4)] {
                let scan: Vec<&Item> = s.items().iter().filter(|i| i.slot() == slot).collect();
                assert_eq!(s.at(slot).collect::<Vec<_>>(), scan, "at {slot:?}");
            }
        }
        let mut s = Store::new();
        let a = stack(&mut s, 34, 10, hangar());
        let b = stack(&mut s, 34, 20, hangar());
        let c = stack(&mut s, 35, 5, Slot::new(2, HANGAR, 4));
        let probes = [a, b, c, HANGAR, a + 1000];
        check(&s, &probes);
        let at5 = Slot::new(1, HANGAR, 5);
        let plan: [&[Edit]; 6] = [
            &[Edit::Grow {
                item_id: a,
                quantity: 3,
            }],
            &[Edit::Create {
                type_id: 34,
                quantity: 4,
                to: hangar(),
            }],
            &[Edit::Split {
                item_id: b,
                quantity: 5,
                to: at5,
            }],
            &[Edit::Move {
                item_id: c,
                to: hangar(),
            }],
            &[Edit::Merge { from: b, into: a }],
            &[Edit::Destroy { item_id: c }],
        ];
        for edits in plan {
            s.apply_in(&fx(), edits).unwrap();
            check(&s, &probes);
        }
        let ids: Vec<i64> = s.items().iter().map(|i| i.item_id).collect();
        check(&s, &ids);
        s.insert(Item {
            item_id: 5_000,
            type_id: 34,
            owner_id: 1,
            location_id: HANGAR,
            flag_id: 4,
            quantity: 1,
            group_id: 0,
            category_id: 0,
            singleton: false,
        });
        check(&s, &[5_000, HANGAR]);
        let copy = s.clone();
        check(&copy, &[5_000, HANGAR]);
    }

    #[test]
    fn move_reports_both_ends() {
        let mut s = Store::new();
        let id = stack(&mut s, 34, 10, hangar());
        let to = Slot::new(1, HANGAR, 5);
        let got = s
            .apply_in(&fx(), &[Edit::Move { item_id: id, to }])
            .unwrap();
        assert_eq!(
            got,
            vec![Change::Moved {
                item_id: id,
                from: hangar(),
                to
            }]
        );
        assert_eq!(s.get(id).unwrap().slot(), to);
    }

    #[test]
    fn split_leaves_both_stacks_and_names_the_new_one() {
        let mut s = Store::new();
        let id = stack(&mut s, 34, 10, hangar());
        let to = Slot::new(1, HANGAR, 5);
        let got = s
            .apply_in(
                &fx(),
                &[Edit::Split {
                    item_id: id,
                    quantity: 4,
                    to,
                }],
            )
            .unwrap();
        assert_eq!(s.get(id).unwrap().quantity, 6);
        let new_id = match got[1] {
            Change::Created { item_id, .. } => item_id,
            ref c => panic!("expected Created, got {c:?}"),
        };
        assert_ne!(new_id, id);
        assert_eq!(s.get(new_id).unwrap().quantity, 4);
        assert_eq!(s.get(new_id).unwrap().slot(), to);
    }

    #[test]
    fn splitting_the_whole_stack_is_a_move() {
        // The boundary a caller computing `quantity` from a UI drag hits, and
        // the reason Split accepts it instead of refusing.
        let mut s = Store::new();
        let id = stack(&mut s, 34, 10, hangar());
        let to = Slot::new(1, HANGAR, 5);
        let got = s
            .apply_in(
                &fx(),
                &[Edit::Split {
                    item_id: id,
                    quantity: 10,
                    to,
                }],
            )
            .unwrap();
        assert_eq!(got.len(), 1, "one move, not a split plus an empty stack");
        assert!(matches!(got[0], Change::Moved { .. }));
        assert_eq!(s.items().len(), 1);
    }

    #[test]
    fn merge_folds_and_destroys() {
        let mut s = Store::new();
        let a = stack(&mut s, 34, 10, hangar());
        let b = stack(&mut s, 34, 5, hangar());
        let got = s
            .apply_in(&fx(), &[Edit::Merge { from: b, into: a }])
            .unwrap();
        assert_eq!(s.get(a).unwrap().quantity, 15);
        assert!(s.get(b).is_none());
        assert!(got.iter().any(|c| matches!(
            c,
            Change::QuantityChanged {
                from: 10,
                to: 15,
                ..
            }
        )));
        assert!(
            got.iter()
                .any(|c| c.item_id() == b && matches!(c, Change::Destroyed { .. }))
        );
    }

    #[test]
    fn merge_refuses_the_four_ways_it_can_be_wrong() {
        let mut s = Store::new();
        let a = stack(&mut s, 34, 10, hangar());
        let other_type = stack(&mut s, 35, 10, hangar());
        let other_slot = stack(&mut s, 34, 10, Slot::new(1, HANGAR, 5));
        let single = s.mint();
        s.insert(Item {
            item_id: single,
            type_id: 34,
            owner_id: 1,
            location_id: HANGAR,
            flag_id: 4,
            quantity: 1,
            group_id: 0,
            category_id: 0,
            singleton: true,
        });
        for (from, into) in [(other_type, a), (other_slot, a), (single, a), (a, a)] {
            assert!(
                matches!(
                    s.apply_in(&fx(), &[Edit::Merge { from, into }]),
                    Err(EditError::NotMergeable { .. })
                ),
                "merge {from} -> {into} should be refused"
            );
        }
    }

    #[test]
    fn consume_to_zero_destroys_rather_than_leaving_an_empty_stack() {
        let mut s = Store::new();
        let id = stack(&mut s, 34, 7, hangar());
        let got = s
            .apply_in(
                &fx(),
                &[Edit::Consume {
                    item_id: id,
                    quantity: 7,
                }],
            )
            .unwrap();
        assert!(matches!(got[0], Change::Destroyed { .. }));
        assert!(s.get(id).is_none());
    }

    /// `Grow` is `Consume` backwards: one `QuantityChanged` naming the old
    /// and new count, and refused for a singleton, a non-positive count and
    /// an overflow.
    #[test]
    fn grow_adds_to_a_stack_and_refuses_what_is_not_one() {
        let mut s = Store::new();
        let id = stack(&mut s, 1230, 100, hangar());
        let got = s
            .apply_in(
                &fx(),
                &[Edit::Grow {
                    item_id: id,
                    quantity: 33,
                }],
            )
            .unwrap();
        assert_eq!(
            got,
            vec![Change::QuantityChanged {
                item_id: id,
                from: 100,
                to: 133
            }]
        );
        assert_eq!(s.get(id).map(|i| i.quantity), Some(133));
        for bad in [0, -1, i64::MAX] {
            assert!(
                s.apply_in(
                    &fx(),
                    &[Edit::Grow {
                        item_id: id,
                        quantity: bad
                    }]
                )
                .is_err(),
                "{bad}"
            );
        }
        let ship = s.spawn_in(&fx(), "Rifter", 1, 1, HANGAR).unwrap().0[0];
        assert!(matches!(
            s.apply_in(
                &fx(),
                &[Edit::Grow {
                    item_id: ship,
                    quantity: 1
                }]
            ),
            Err(EditError::NotMergeable { .. })
        ));
        assert_eq!(
            s.get(id).map(|i| i.quantity),
            Some(133),
            "refusals change nothing"
        );
    }

    #[test]
    fn a_plan_that_cannot_finish_changes_nothing() {
        // The property the whole two-phase split exists for.
        let mut s = Store::new();
        let id = stack(&mut s, 34, 10, hangar());
        let before = s.items().to_vec();
        let to = Slot::new(1, HANGAR, 5);
        let err = s
            .apply_in(
                &fx(),
                &[
                    Edit::Move { item_id: id, to },
                    Edit::Move {
                        item_id: 999_999,
                        to,
                    },
                ],
            )
            .unwrap_err();
        assert_eq!(err, EditError::NoSuchItem(999_999));
        assert_eq!(
            s.items(),
            &before[..],
            "the first move must not have landed"
        );
    }

    #[test]
    fn two_edits_on_one_item_are_refused_not_ordered() {
        let mut s = Store::new();
        let id = stack(&mut s, 34, 10, hangar());
        let to = Slot::new(1, HANGAR, 5);
        assert_eq!(
            s.apply_in(
                &fx(),
                &[
                    Edit::Move { item_id: id, to },
                    Edit::Destroy { item_id: id }
                ]
            ),
            Err(EditError::Conflict(id))
        );
        assert_eq!(s.get(id).unwrap().slot(), hangar());
    }

    #[test]
    fn multi_step_plan_keeps_indices_valid_across_removals() {
        // Destroy + merge + move in one plan: both removals shift indices that
        // later steps resolved, which is what phase two defers them for.
        let mut s = Store::new();
        let doomed = stack(&mut s, 34, 1, hangar());
        let a = stack(&mut s, 35, 10, hangar());
        let b = stack(&mut s, 35, 5, hangar());
        let mover = stack(&mut s, 36, 2, hangar());
        let to = Slot::new(1, HANGAR, 5);
        s.apply_in(
            &fx(),
            &[
                Edit::Destroy { item_id: doomed },
                Edit::Merge { from: b, into: a },
                Edit::Move { item_id: mover, to },
            ],
        )
        .unwrap();
        assert!(s.get(doomed).is_none());
        assert!(s.get(b).is_none());
        assert_eq!(s.get(a).unwrap().quantity, 15);
        assert_eq!(s.get(mover).unwrap().slot(), to);
        assert_eq!(s.items().len(), 2);
    }

    #[test]
    fn bad_quantity_names_what_the_stack_actually_has() {
        let mut s = Store::new();
        let id = stack(&mut s, 34, 3, hangar());
        for q in [0, -1, 4] {
            assert_eq!(
                s.apply_in(
                    &fx(),
                    &[Edit::Consume {
                        item_id: id,
                        quantity: q
                    }]
                ),
                Err(EditError::BadQuantity {
                    item_id: id,
                    want: q,
                    have: 3
                })
            );
        }
    }

    #[test]
    fn escrow_locations_sit_above_the_client_s_local_lookup_threshold() {
        // The hazard EvEJS's locationID 2003 walks into: below 90_000_000 the
        // client answers from its own sqlite before asking us.
        for ticket in [0, 1, 999_999] {
            let loc = escrow_location(ticket);
            assert!(loc > 90_000_000, "escrow {loc} is locally resolvable");
            assert!(is_escrow(loc));
        }
        assert!(!is_escrow(HANGAR));
        assert!(!is_escrow(2003));
    }

    /// **The whole point of `Edit::Create`: consume and produce in one
    /// plan.** This is what a reprocess is, and before the variant existed
    /// the produce half had to go through `Store::spawn` -- outside `apply`,
    /// so a failure between the two halves left the ore gone and no minerals.
    #[test]
    fn a_consume_and_produce_plan_is_one_atomic_operation() {
        let mut s = Store::new();
        let ore = stack(&mut s, 1230, 1000, hangar());
        let changes = s
            .apply_in(
                &fx(),
                &[
                    Edit::Destroy { item_id: ore },
                    Edit::Create {
                        type_id: 34,
                        quantity: 4150,
                        to: hangar(),
                    },
                    Edit::Create {
                        type_id: 35,
                        quantity: 830,
                        to: hangar(),
                    },
                ],
            )
            .expect("the plan is valid");
        assert_eq!(changes.len(), 3);
        assert!(s.get(ore).is_none(), "the ore is gone");
        let have: Vec<(i64, i64)> = s.items().iter().map(|i| (i.type_id, i.quantity)).collect();
        assert!(have.contains(&(34, 4150)), "{have:?}");
        assert!(have.contains(&(35, 830)), "{have:?}");
    }

    /// **Phase one refuses before anything happens**, which is the guarantee
    /// the whole two-phase shape exists for. An unknown type in the produce
    /// half must not leave the ore consumed.
    #[test]
    fn an_unknown_type_refuses_the_whole_plan_and_keeps_the_input() {
        let mut s = Store::new();
        let ore = stack(&mut s, 1230, 1000, hangar());
        let got = s.apply_in(
            &fx(),
            &[
                Edit::Destroy { item_id: ore },
                Edit::Create {
                    type_id: 999_999,
                    quantity: 1,
                    to: hangar(),
                },
            ],
        );
        assert_eq!(got, Err(EditError::UnknownType(999_999)));
        assert_eq!(
            s.get(ore).map(|i| i.quantity),
            Some(1000),
            "the ore must still be there: phase one refused"
        );
    }

    /// Two `Create`s of one type are two stacks, not a `Conflict`.
    ///
    /// `Create` names no existing item, so it deliberately does not `claim`
    /// -- and a caller producing two separate stacks of Tritanium in one plan
    /// is doing something legitimate that `Conflict` would wrongly refuse.
    #[test]
    fn two_creates_of_one_type_are_two_stacks() {
        let mut s = Store::new();
        let changes = s
            .apply_in(
                &fx(),
                &[
                    Edit::Create {
                        type_id: 34,
                        quantity: 5,
                        to: hangar(),
                    },
                    Edit::Create {
                        type_id: 34,
                        quantity: 7,
                        to: hangar(),
                    },
                ],
            )
            .expect("two stacks is a legitimate plan");
        assert_eq!(changes.len(), 2);
        let mut q: Vec<i64> = s.items().iter().map(|i| i.quantity).collect();
        q.sort_unstable();
        assert_eq!(q, vec![5, 7]);
    }

    /// A singleton type is `quantity` separate objects, the same rule
    /// `spawn` applies: the client has no rendering for a stack of ships.
    #[test]
    fn creating_singletons_makes_one_item_each() {
        let mut s = Store::new();
        s.apply_in(
            &fx(),
            &[Edit::Create {
                type_id: 587, // Rifter: a singleton
                quantity: 3,
                to: hangar(),
            }],
        )
        .expect("valid");
        let ships: Vec<&Item> = s.items().iter().filter(|i| i.singleton).collect();
        assert_eq!(ships.len(), 3, "three hulls, not one stack of three");
        assert!(ships.iter().all(|i| i.quantity == 1));
    }

    /// A market buy: the hulls come as one packaged stack, never assembled.
    #[test]
    fn creating_packaged_singletons_makes_one_packaged_stack() {
        let mut s = Store::new();
        s.apply_in(
            &fx(),
            &[Edit::CreatePackaged {
                type_id: 587,
                quantity: 3,
                to: hangar(),
            }],
        )
        .expect("valid");
        assert_eq!(s.items().len(), 1, "one stack");
        assert!(!s.items()[0].singleton, "packaged");
        assert_eq!(s.items()[0].quantity, 3);
    }

    /// A non-positive quantity is refused, like every other `Edit`.
    #[test]
    fn creating_nothing_is_refused() {
        let mut s = Store::new();
        for q in [0, -1] {
            assert!(matches!(
                s.apply_in(
                    &fx(),
                    &[Edit::Create {
                        type_id: 34,
                        quantity: q,
                        to: hangar(),
                    }]
                ),
                Err(EditError::BadQuantity { .. })
            ));
        }
        assert!(s.items().is_empty());
    }

    /// **Indices stay valid across a plan that both creates and removes.**
    ///
    /// Phase one resolves indices into `self.items`; phase two must not push
    /// a created item into that vec while later steps still hold indices
    /// into it. `Create` therefore goes through `minted`, appended after the
    /// removals -- the same reason `Split` does. A version that pushed
    /// directly passed every test above and would have corrupted this one.
    #[test]
    fn create_does_not_disturb_indices_resolved_in_phase_one() {
        let mut s = Store::new();
        let a = stack(&mut s, 34, 10, hangar());
        let b = stack(&mut s, 35, 20, hangar());
        let c = stack(&mut s, 36, 30, hangar());
        let to = Slot::new(1, HANGAR, 5);
        s.apply_in(
            &fx(),
            &[
                Edit::Destroy { item_id: a },
                Edit::Create {
                    type_id: 37,
                    quantity: 1,
                    to: hangar(),
                },
                Edit::Move { item_id: c, to },
                Edit::Consume {
                    item_id: b,
                    quantity: 5,
                },
            ],
        )
        .expect("valid");
        assert!(s.get(a).is_none());
        assert_eq!(s.get(b).map(|i| i.quantity), Some(15), "b was consumed");
        assert_eq!(s.get(c).map(|i| i.slot()), Some(to), "c moved");
        assert_eq!(
            s.items().iter().filter(|i| i.type_id == 37).count(),
            1,
            "and the new item exists"
        );
    }

    #[test]
    fn an_empty_plan_is_a_no_op_not_an_error() {
        let mut s = Store::new();
        stack(&mut s, 34, 1, hangar());
        assert_eq!(s.apply_in(&fx(), &[]), Ok(Vec::new()));
        assert_eq!(s.items().len(), 1);
    }

    /// A seeded item's id is never handed out again.
    ///
    /// **The property is "no two items ever share an id", not "`insert`
    /// updates a field".** The bug this pins was found by a manufacturing job
    /// planning to consume the player's capsule as Tritanium, because the
    /// capsule is seeded at `MIN_PLAYER_ITEM + 1` and the second mint of the
    /// world returned that same id. Reverting the one-line guard in
    /// [`Store::insert`] makes this fail on the first assertion.
    #[test]
    fn a_seeded_id_is_never_minted_again() {
        let mut s = Store::new();
        // The capsule, exactly as `Registry::new` seeds it: an id the caller
        // chose, one past the floor.
        let seeded = MIN_PLAYER_ITEM + 1;
        s.insert(Item {
            item_id: seeded,
            type_id: 670,
            owner_id: 90_000_001,
            location_id: 60_012_526,
            flag_id: FLAG_HANGAR,
            quantity: 1,
            group_id: 29,
            category_id: 6,
            singleton: true,
        });
        // Two mints, which is what one manufacturing install does.
        let (a, _) = s
            .spawn_in(&fx(), "Tritanium", 100, 90_000_001, 60_012_526)
            .expect("minerals");
        let (b, _) = s
            .spawn_in(&fx(), "Tritanium", 100, 90_000_001, 60_012_526)
            .expect("more minerals");
        let mut ids: Vec<i64> = s.items().iter().map(|i| i.item_id).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            before,
            "two items share an id: {:?}",
            s.items()
                .iter()
                .map(|i| (i.item_id, i.type_id))
                .collect::<Vec<_>>()
        );
        assert!(
            !a.contains(&seeded) && !b.contains(&seeded),
            "the capsule's id was handed out to a mineral stack"
        );
        assert_eq!(
            s.get(seeded).map(|i| i.type_id),
            Some(670),
            "and the capsule is still the thing that id names"
        );
    }
    /// **Three stacks fold into one, in one plan, with one survivor.**
    ///
    /// The regression this pins is `Conflict`: before merge targets got their
    /// own claim, two merges naming the same survivor were refused as "two
    /// edits in one plan touch item X", so "Stack All" over three stacks
    /// could not be expressed atomically at all.
    ///
    /// Also pins the `Change` list, because callers build pushes straight off
    /// it (`Registry::apply_items`): **one** `QuantityChanged` for the
    /// survivor describing the net 100 -> 300, not one per merge with an
    /// intermediate `from`.
    #[test]
    fn stack_all_folds_every_stack_of_a_type_into_the_oldest_id() {
        let mut s = Store::new();
        let owner = 90_000_001;
        let station = 60_012_526;
        let slot = Slot::new(owner, station, FLAG_HANGAR);
        let mut ids = Vec::new();
        for _ in 0..3 {
            let (made, _) = s
                .spawn_in(&fx(), "EMP S", 100, owner, station)
                .expect("ammo");
            ids.extend(made);
        }
        ids.sort_unstable();

        let plan = s.stack_plan(slot);
        assert_eq!(plan.len(), 2, "three stacks are two merges: {plan:?}");
        assert!(
            plan.iter()
                .all(|e| matches!(e, Edit::Merge { into, .. } if *into == ids[0])),
            "every merge names the lowest id as the survivor: {plan:?}"
        );

        let changes = s
            .apply_in(&fx(), &plan)
            .expect("one atomic plan, not two calls");
        let quantity_changes: Vec<&Change> = changes
            .iter()
            .filter(|c| matches!(c, Change::QuantityChanged { .. }))
            .collect();
        assert_eq!(
            quantity_changes.len(),
            1,
            "one net change for the survivor, not one per merge: {changes:?}"
        );
        assert_eq!(
            *quantity_changes[0],
            Change::QuantityChanged {
                item_id: ids[0],
                from: 100,
                to: 300,
            }
        );
        assert_eq!(s.at(slot).count(), 1, "one stack is left");
        assert_eq!(s.get(ids[0]).map(|i| i.quantity), Some(300));
        assert!(s.get(ids[1]).is_none() && s.get(ids[2]).is_none());
    }

    /// `stack_plan` never proposes a merge `apply` would refuse.
    ///
    /// Two Rifters are two assembled hulls, and `docs/reference/item-spawn.md` §6.2
    /// refuses `singleton` with `stacksize > 1` outright -- so they must not
    /// appear in the plan at all. One stack of a type is likewise nothing to
    /// do, and an empty plan is a legitimate answer.
    #[test]
    fn stack_plan_leaves_singletons_and_lone_stacks_alone() {
        let mut s = Store::new();
        let owner = 90_000_001;
        let station = 60_012_526;
        let slot = Slot::new(owner, station, FLAG_HANGAR);
        s.spawn_in(&fx(), "Rifter", 2, owner, station)
            .expect("ships");
        s.spawn_in(&fx(), "Tritanium", 50, owner, station)
            .expect("mineral");
        assert_eq!(
            s.stack_plan(slot),
            Vec::new(),
            "two ships and one mineral stack have nothing to stack"
        );

        // A different address is a different stack, even for the same type.
        s.spawn_in(&fx(), "Tritanium", 50, owner, 60_000_001)
            .expect("elsewhere");
        assert_eq!(
            s.stack_plan(slot),
            Vec::new(),
            "stacking is per address: a stack in another station is not here"
        );
    }

    /// A merge target may not also be moved, and the error says which item.
    #[test]
    fn a_merge_target_cannot_be_edited_by_the_same_plan() {
        let mut s = Store::new();
        let owner = 90_000_001;
        let station = 60_012_526;
        let (a, _) = s.spawn_in(&fx(), "EMP S", 10, owner, station).expect("a");
        let (b, _) = s.spawn_in(&fx(), "EMP S", 10, owner, station).expect("b");
        let (a, b) = (a[0], b[0]);
        let elsewhere = Slot::new(owner, 60_000_001, FLAG_HANGAR);
        assert_eq!(
            s.apply_in(
                &fx(),
                &[
                    Edit::Merge { from: b, into: a },
                    Edit::Move {
                        item_id: a,
                        to: elsewhere
                    },
                ]
            ),
            Err(EditError::Conflict(a))
        );
        assert_eq!(
            s.apply_in(
                &fx(),
                &[
                    Edit::Move {
                        item_id: a,
                        to: elsewhere
                    },
                    Edit::Merge { from: b, into: a },
                ]
            ),
            Err(EditError::Conflict(a)),
            "and in either order"
        );
    }

    /// **Repackaging flips exactly one flag, and leaves everything else.**
    ///
    /// `singleton` is index 10 of the row the client reads, and all three
    /// consequences the client draws from it -- the Ship Hangar entry stays
    /// (that is `categoryID`), dogma drops the item (`godma.UpdateItem`
    /// skip3) and `ShipPicker` stops offering it -- follow from the flag
    /// alone. So the assertion is that nothing *else* moved.
    #[test]
    fn repackaging_clears_singleton_and_nothing_else() {
        let mut s = Store::new();
        let owner = 90_000_001;
        let station = 60_012_526;
        let (ships, _) = s
            .spawn_in(&fx(), "Rifter", 1, owner, station)
            .expect("rifter");
        let id = ships[0];
        let before = s.get(id).cloned().expect("spawned");
        assert!(before.singleton);

        let changes = s
            .apply_in(&fx(), &[Edit::Repackage { item_id: id }])
            .expect("packs");
        assert_eq!(
            changes,
            vec![Change::Repackaged {
                item_id: id,
                at: before.slot()
            }]
        );
        let after = s.get(id).cloned().expect("still there");
        assert_eq!(
            after,
            Item {
                singleton: false,
                ..before
            },
            "a repackage changes the singleton flag and no other column"
        );
        // And the `Change` names an address, because the invalidation push is
        // built from `Change::slots()` and a repackage nobody is told about
        // is invisible until the next dock (`docs/reference/item-spawn.md` §2).
        assert_eq!(changes[0].slots(), vec![before.slot()]);
    }

    /// The capsule cannot be repackaged, and it is **CCP's rule** that says so.
    ///
    /// `REPACKABLE_CATEGORIES_GROUP_EXCEPTIONS = {groupCapsule}`
    /// (`repackaging/__init__.py:22`). It matters because the pod is the one
    /// item every character in this sandbox already owns, so "Repackage" on
    /// it is the first thing anyone would try.
    #[test]
    fn a_capsule_is_not_repackageable_although_a_ship_is() {
        assert!(
            !can_repackage_type(29, 6),
            "groupCapsule is the one carve-out from categoryShip"
        );
        assert!(can_repackage_type(25, 6), "a frigate repackages");
        assert!(can_repackage_type(55, 7), "a module repackages");
        assert!(
            !can_repackage_type(83, 8),
            "categoryCharge is not in REPACKABLE_CATEGORIES -- ammo is never \
             assembled, so there is nothing to unassemble"
        );
        assert!(
            !can_repackage_type(105, 9),
            "nor is categoryBlueprint, although a blueprint is a singleton"
        );
        assert!(
            can_repackage_type(12, 2),
            "groupCargoContainer repackages whatever its category is"
        );

        let mut s = Store::new();
        let (pods, _) = s
            .spawn_in(&fx(), "Capsule", 1, 90_000_001, 60_012_526)
            .expect("pod");
        assert_eq!(
            s.apply_in(&fx(), &[Edit::Repackage { item_id: pods[0] }]),
            Err(EditError::NotRepackageable {
                item_id: pods[0],
                why: "this type cannot be unassembled"
            })
        );
        assert!(
            s.get(pods[0]).expect("untouched").singleton,
            "a refused plan applies nothing"
        );
    }

    /// The other two refusals, and the fitting-flag table behind one of them.
    #[test]
    fn repackaging_refuses_a_packaged_item_and_a_fitted_one() {
        let mut s = Store::new();
        let owner = 90_000_001;
        let station = 60_012_526;
        let (ammo, _) = s
            .spawn_in(&fx(), "EMP S", 10, owner, station)
            .expect("ammo");
        assert_eq!(
            s.apply_in(&fx(), &[Edit::Repackage { item_id: ammo[0] }]),
            Err(EditError::NotRepackageable {
                item_id: ammo[0],
                why: "it is already packaged"
            }),
            "`RepackageItemNotSingleton`, reported rather than swallowed: \
             whether it is an error is the caller's question"
        );

        // `IsShipFittingFlag`, transcribed from `const.fittingFlags`.
        for flag in [11, 18, 19, 26, 27, 34, 92, 94, 125, 128, 156, 164, 171] {
            assert!(is_ship_fitting_flag(flag), "flag {flag} is a fitting slot");
        }
        for flag in [0, 4, 5, 10, 35, 57, 87, 90, 91, 95, 99, 124, 155, 172] {
            assert!(
                !is_ship_fitting_flag(flag),
                "flag {flag} is not a fitting slot"
            );
        }

        let (modules, _) = s
            .spawn_in(&fx(), "Miner I", 1, owner, station)
            .expect("miner");
        let fitted = modules[0];
        s.apply_in(
            &fx(),
            &[
                Edit::Move {
                    item_id: fitted,
                    to: Slot::new(owner, 100_000_000, 27),
                },
                Edit::Repackage { item_id: fitted },
            ],
        )
        .expect_err("a plan may not move a module into a slot and repackage it");
        // Fitted *and* already a stack, so the first test it fails is the
        // singleton one -- assert the fitted case on a singleton instead.
        let (ships, _) = s
            .spawn_in(&fx(), "Rifter", 1, owner, station)
            .expect("rifter");
        let ship = ships[0];
        s.apply_in(
            &fx(),
            &[Edit::Move {
                item_id: ship,
                to: Slot::new(owner, 100_000_000, 27),
            }],
        )
        .expect("moved into a high slot");
        assert_eq!(
            s.apply_in(&fx(), &[Edit::Repackage { item_id: ship }]),
            Err(EditError::NotRepackageable {
                item_id: ship,
                why: "it is fitted"
            })
        );
    }

    /// Contents come out in the same plan as the repackage.
    ///
    /// Not a store rule -- `Edit::Repackage` deliberately does not move
    /// anything -- but the composition is the thing `Store::apply` exists to
    /// make possible, and `service/inventory.rs` depends on it being atomic.
    #[test]
    fn a_repackage_and_its_contents_move_are_one_atomic_plan() {
        let mut s = Store::new();
        let owner = 90_000_001;
        let station = 60_012_526;
        let (ships, _) = s
            .spawn_in(&fx(), "Rifter", 1, owner, station)
            .expect("rifter");
        let ship = ships[0];
        let (ammo, _) = s
            .spawn_in(&fx(), "EMP S", 100, owner, station)
            .expect("ammo");
        let cargo = ammo[0];
        let hangar = Slot::new(owner, station, FLAG_HANGAR);
        s.apply_in(
            &fx(),
            &[Edit::Move {
                item_id: cargo,
                to: Slot::new(owner, ship, 5),
            }],
        )
        .expect("stowed");
        assert_eq!(s.contents_of(ship).count(), 1);

        s.apply_in(
            &fx(),
            &[
                Edit::Move {
                    item_id: cargo,
                    to: hangar,
                },
                Edit::Repackage { item_id: ship },
            ],
        )
        .expect("one plan");
        assert_eq!(s.contents_of(ship).count(), 0, "the hold was emptied");
        assert_eq!(s.get(cargo).map(|i| i.slot()), Some(hangar));
        assert!(!s.get(ship).expect("still here").singleton);
    }

    /// **A move tells the client where the item was**, or its old window
    /// keeps it: `inventorysvc.py:126-140` rebuilds the previous row from
    /// the `change` dict alone (`docs/lanes/fitting.md` §4). The indices are
    /// CCP's `ixLocationID` 3, `ixFlag` 4, `ixOwnerID` 2 [V].
    #[test]
    fn a_move_reports_its_real_previous_location_and_flag() {
        let from = Slot::new(1, 60_000_001, 4);
        let to = Slot::new(1, 140_000_001, 27);
        let c = Change::Moved {
            item_id: 7,
            from,
            to,
        };
        assert_eq!(c.previous_columns(), vec![(3, 60_000_001), (4, 4)]);
        let given = Change::Moved {
            item_id: 7,
            from,
            to: Slot::new(2, 60_000_001, 4),
        };
        assert!(
            given.previous_columns().contains(&(2, 1)),
            "a change of owner names the old owner"
        );
        assert_eq!(
            Change::Created { item_id: 7, at: to }.previous_columns(),
            vec![(3, 0)],
            "a spawn keeps the fabricated location that fails IsItemHere"
        );
    }

    // ---- Assemble (`docs/lanes/fitting.md` §5, tests 1-3) ----

    /// A packaged 125mm Gatling AutoCannon I stack, the module `/createitem`
    /// grants -- a real type, because `Assemble` refuses an unknown one.
    fn guns(s: &mut Store, quantity: i64) -> i64 {
        let t = *find_in(&Content::fixture(), 484).expect("484 is spawnable");
        let id = s.mint();
        s.insert(Item {
            item_id: id,
            type_id: t.type_id,
            owner_id: 1,
            location_id: HANGAR,
            flag_id: 4,
            quantity,
            group_id: t.group_id,
            category_id: t.category_id,
            singleton: false,
        });
        id
    }

    /// `flagHiSlot0` [V] (`inventorycommon/const`).
    fn hi_slot_0() -> Slot {
        Slot::new(1, 140_000_001, 27)
    }

    /// Test 1: from a stack of five, one unit leaves as a **new** object in
    /// the slot, and the stack keeps four.
    #[test]
    fn assembling_from_a_stack_splits_one_assembled_unit_off() {
        let mut s = Store::new();
        let src = guns(&mut s, 5);
        let got = s
            .apply_in(
                &fx(),
                &[Edit::Assemble {
                    item_id: src,
                    to: hi_slot_0(),
                }],
            )
            .expect("assembled");
        assert_eq!(s.get(src).map(|i| i.quantity), Some(4));
        let fitted: Vec<&Item> = s.at(hi_slot_0()).collect();
        assert_eq!(fitted.len(), 1);
        assert_ne!(fitted[0].item_id, src, "a new id, not the stack's");
        assert!(fitted[0].singleton);
        assert_eq!(fitted[0].quantity, 1);
        assert!(matches!(
            got.as_slice(),
            [
                Change::QuantityChanged { from: 5, to: 4, .. },
                Change::Created { .. }
            ]
        ));
    }

    /// Test 2: a stack of one is the item itself, moved and flipped; an
    /// assembled item cannot be assembled again, and the refusal changes
    /// nothing.
    #[test]
    fn assembling_a_single_unit_moves_and_flips_it_and_twice_is_refused() {
        let mut s = Store::new();
        let id = guns(&mut s, 1);
        let got = s
            .apply_in(
                &fx(),
                &[Edit::Assemble {
                    item_id: id,
                    to: hi_slot_0(),
                }],
            )
            .expect("assembled");
        let it = s.get(id).expect("same id");
        assert!(it.singleton);
        assert_eq!(it.slot(), hi_slot_0());
        assert!(matches!(
            got.as_slice(),
            [Change::Moved { .. }, Change::Assembled { .. }]
        ));
        assert_eq!(got[1].previous_columns(), vec![(IX_SINGLETON, 0)]);

        let before = s.items().to_vec();
        assert!(matches!(
            s.apply_in(
                &fx(),
                &[Edit::Assemble {
                    item_id: id,
                    to: hangar(),
                }]
            ),
            Err(EditError::NotAssemblable { .. })
        ));
        assert_eq!(
            s.items(),
            before.as_slice(),
            "a refused plan changes nothing"
        );
    }

    /// Test 3: `Repackage` undoes `Assemble`.
    #[test]
    fn assemble_then_repackage_round_trips_singleton() {
        let mut s = Store::new();
        let id = guns(&mut s, 1);
        s.apply_in(
            &fx(),
            &[Edit::Assemble {
                item_id: id,
                to: hangar(),
            }],
        )
        .expect("assembled in place");
        assert!(s.get(id).expect("here").singleton);
        s.apply_in(&fx(), &[Edit::Repackage { item_id: id }])
            .expect("repackaged");
        assert!(!s.get(id).expect("here").singleton);
    }

    /// A loaded charge is a charge at a fitting flag, and nothing else is:
    /// not the gun in the same slot, not the same rounds in the hangar or
    /// the cargo hold. `categoryCharge` 8 and the fitting flags are [V].
    #[test]
    fn only_a_charge_at_a_fitting_flag_is_loaded() {
        let rounds = |flag_id: i64, category_id: i64| Item {
            item_id: 100_000_500,
            type_id: 185,
            owner_id: 90_000_001,
            location_id: 100_000_001,
            flag_id,
            quantity: 100,
            group_id: 83,
            category_id,
            singleton: false,
        };
        assert!(is_loaded_charge(&rounds(27, CATEGORY_CHARGE)), "hi slot 0");
        assert!(is_loaded_charge(&rounds(19, CATEGORY_CHARGE)), "med slot 0");
        assert!(!is_loaded_charge(&rounds(5, CATEGORY_CHARGE)), "cargo");
        assert!(!is_loaded_charge(&rounds(4, CATEGORY_CHARGE)), "hangar");
        assert!(!is_loaded_charge(&rounds(27, 7)), "the module itself");
        let c = Content::fixture();
        let group = |id| find_in(&c, id).map(|t| t.group_id);
        assert_eq!(group(184), group(185));
    }
}
