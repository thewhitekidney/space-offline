//! Skills: a subsystem beside the item store, not a feature of it.
//!
//! tracker #5 -- *"skills buy and inject non functional: i believe the whole
//! skill system server-sided is non functional"*. They were right.
//!
//! # Why skills are not items, even though they look like items
//!
//! Every id suggests they should be: `flagSkill` is 7, `categorySkill` is 16,
//! `attributeSkillLevel` is 280 and `attributeSkillPoints` is 276, all real
//! and all in the client. **But the client cannot learn a skill from the item
//! store.** `godma.StateManager.UpdateItem` drops any item whose category is
//! not one of eleven and whose group is not `groupCharacter`, and category 16
//! is not in that tuple -- so a skill row sent through
//! `charMgr.ListIncludingContainers` or `dogmaIM.GetAllInfo` is logged
//! "Godma location received unexpected and unwanted item" and discarded. The
//! eleven are enumerated in that function at `godma.py:1874`-`:1884`:
//! entity, starbase, sovereignty structure, ship, module, charge, drone,
//! subsystem, fighter, structure, structure module.
//!
//! So the client's entire model of trained skills is `skillsvc`'s caches,
//! filled from `skillMgr2.GetMySkillHandler()`. An *uninjected skillbook* is
//! an ordinary hangar item -- that is the inject path, which takes item ids
//! and consumes the item. Purchase takes **type** ids, costs ISK and creates
//! no item at all.
//!
//! # The wire shape, and why it is a `0x23`
//!
//! `characterskills.common.character_skill_entry.CharacterSkillEntry` is in
//! `blue.marshal.globalsWhitelist` verbatim, so we may send the real class.
//! It is new-style, so it is a **`0x23`** ObjectEx: the client does
//! `cls.__new__(cls)` and then updates `__dict__`, which is why the state
//! keys are the **underscore-prefixed private names** and not the properties.
//! `__init__` is never called.
//!
//! It must be a mutable instance and not a `DBRow`:
//! `_UpdateMySkillsAndReturnLevelsTrained` does `skillInfo.timeStamp =
//! timeStamp`, a plain `setattr` on a field the class does not declare. An
//! instance `__dict__` takes it; a packed `DBRow` refuses `STORE_ATTR`.
//!
//! Two of the class's own properties **raise** on access --
//! `skillLevel` and `skillPoints` both raise `DeprecationWarning`, which is an
//! `Exception` subclass in Python 2. The readable names are
//! `trainedSkillLevel` and `trainedSkillPoints`. That is why the private field
//! below is `_skillPoints` while the property that works is
//! `trainedSkillPoints`: the names are deliberately crossed and copying the
//! wrong one is a raise, not a blank.
//!
//! # `rank` must be >= 1
//!
//! `GetSPForLevelRaw(rank, level)` is `ceil(rank * 250 * 2 ** (2.5 * (level -
//! 1)))` and `GetLevelProgress` divides by `nextLevelPoints -
//! baseLevelPoints`. At rank 0 both are zero and the skill-queue entry's
//! progress bar raises `ZeroDivisionError`.
//!
//! # And in `Content` (CP59a, ruling 3)
//!
//! The skill [`Table`] is also a content table ([`TABLE`], plan §1.5): the
//! bundle's `types/skills.tsv`, or the invented [`FIXTURE`] without a
//! bundle, read through `reg.content()` with the `*_in(&Content, ..)`
//! accessors ([`find_in`], [`required_in`], [`item_types_in`],
//! [`find_any_in`], [`all_in`]). The skillbook items live in the same table,
//! so a table built with no file ([`Table::fallback`]) carries the fallback
//! books itself.
//!
//! The rules that read the table have `_in` twins too (CP59b, W18):
//! [`Skills::starter_in`], [`Skills::grant_in`], [`Skills::purchase_in`],
//! [`rate_for_in`] and the [`Queue`] methods `prune_in`, `rows_in`,
//! `settle_in`, `pause_in`, `save_in`, over a private `Lookup`. Every
//! caller reads `reg.content()` (or a `&Content` it was handed); the free
//! originals, `install` and the process `OnceLock` are gone (X2b), and
//! `--skills` overrides the `Content` table ([`override_in`]).

use std::collections::BTreeMap;

use eve_marshal::PyValue;

use crate::content::{Boxed, Content, Origin, Sources, TableSpec};

/// A skill the sandbox knows about.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillType {
    pub type_id: i64,
    pub name: &'static str,
    /// The skill's group (`types.fsdbinary` `groupID`) **[V]**. Only the item
    /// store reads it: an uninjected skillbook is an item, and an item row
    /// carries its group ([`item_types_in`]).
    pub group_id: i64,
    /// `attributeSkillTimeConstant` (275), the training-time multiplier.
    ///
    /// **Every value here is the client's own.** It used to be 1 for every
    /// skill, which was a stopgap and said so: the client computes rank
    /// itself from FSD, in `skillsvc.GetSkillRank` ->
    /// `godma.GetTypeAttribute(typeID, 275)`, so what we send is compared
    /// against its own and a mismatch shows as two different training times
    /// for one skill in two panels. `typedogma.fsdbinary` had no decoded
    /// columns at the time; it does now (`docs/re/dogma.md` §12), and these
    /// were read out of it. **[V]**
    ///
    /// **Eight of the twenty-three skills that predate this were not 1**, so
    /// the stopgap was wrong a third of the time: the four racial frigates,
    /// Hull Upgrades and Electronic Warfare at 2, Astrometrics and Capacitor
    /// Management at 3. The eighteen resource-processing skills added with
    /// reprocessing run as high as 12.
    ///
    /// Must be >= 1 regardless of the table: see the module header. 0 is a
    /// `ZeroDivisionError` in the client's own progress bar.
    pub rank: i64,
    /// `primaryAttribute` (180): the character attribute (164-168) that
    /// counts in full toward this skill's training rate. Read out of
    /// `typedogma` with `tools/dogma.py type <id>`, like `rank` **[V]**. The
    /// client reads the same value in `skillsvc.GetPrimarySkillAttribute`, so
    /// a wrong one here is a server end time that disagrees with the client's
    /// own countdown.
    pub primary: i64,
    /// `secondaryAttribute` (181), which counts half. Same source **[V]**.
    pub secondary: i64,
}

/// The skills the sandbox knows about.
///
/// **Every id and name below was read out of the client's own
/// `types.fsdbinary` and `groups.fsdbinary`** -- filtered to `categoryID == 16`
/// and resolved through `localization_fsd_en-us.pickle` -- rather than
/// remembered. That matters here more than it does for items: a wrong skill
/// type id is not a wrong row, it is a skill the client will not render at all,
/// and "3426 is CPU Management" is exactly the sort of thing that is easy to
/// half-remember.
pub const SKILLS: &[SkillType] = &[
    SkillType {
        type_id: 3300,
        name: "Gunnery",
        group_id: 255,
        rank: 1,
        primary: 167,
        secondary: 168,
    },
    SkillType {
        type_id: 3301,
        name: "Small Hybrid Turret",
        group_id: 255,
        rank: 1,
        primary: 167,
        secondary: 168,
    },
    SkillType {
        type_id: 3302,
        name: "Small Projectile Turret",
        group_id: 255,
        rank: 1,
        primary: 167,
        secondary: 168,
    },
    SkillType {
        type_id: 3327,
        name: "Spaceship Command",
        group_id: 257,
        rank: 1,
        primary: 167,
        secondary: 168,
    },
    SkillType {
        type_id: 3328,
        name: "Gallente Frigate",
        group_id: 257,
        rank: 2,
        primary: 167,
        secondary: 168,
    },
    SkillType {
        type_id: 3329,
        name: "Minmatar Frigate",
        group_id: 257,
        rank: 2,
        primary: 167,
        secondary: 168,
    },
    SkillType {
        type_id: 3330,
        name: "Caldari Frigate",
        group_id: 257,
        rank: 2,
        primary: 167,
        secondary: 168,
    },
    SkillType {
        type_id: 3331,
        name: "Amarr Frigate",
        group_id: 257,
        rank: 2,
        primary: 167,
        secondary: 168,
    },
    SkillType {
        type_id: 3355,
        name: "Social",
        group_id: 278,
        rank: 1,
        primary: 164,
        secondary: 165,
    },
    // The gate on "Create Corporation": `MemberCanCreateCorporation` asks
    // `skills.GetSkill(typeCorporationManagement)` and opens no form without
    // it (`bco_members.py`). `typeCorporationManagement = 3363`
    // (`inventorycommon/const.py`); rank, primary (memory) and secondary
    // (charisma) from `tools/dogma.py type 3363`. **[V]**
    SkillType {
        type_id: 3363,
        name: "Corporation Management",
        group_id: 266,
        rank: 1,
        primary: 166,
        secondary: 164,
    },
    SkillType {
        type_id: 3380,
        name: "Industry",
        group_id: 268,
        rank: 1,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 3386,
        name: "Mining",
        group_id: 1218,
        rank: 1,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 3392,
        name: "Mechanics",
        group_id: 1210,
        rank: 1,
        primary: 165,
        secondary: 166,
    },
    SkillType {
        type_id: 3394,
        name: "Hull Upgrades",
        group_id: 1210,
        rank: 2,
        primary: 165,
        secondary: 166,
    },
    SkillType {
        type_id: 3402,
        name: "Science",
        group_id: 270,
        rank: 1,
        primary: 165,
        secondary: 166,
    },
    SkillType {
        type_id: 3412,
        name: "Astrometrics",
        group_id: 1217,
        rank: 3,
        primary: 165,
        secondary: 166,
    },
    SkillType {
        type_id: 3413,
        name: "Power Grid Management",
        group_id: 1216,
        rank: 1,
        primary: 165,
        secondary: 166,
    },
    SkillType {
        type_id: 3416,
        name: "Shield Operation",
        group_id: 1209,
        rank: 1,
        primary: 165,
        secondary: 166,
    },
    SkillType {
        type_id: 3418,
        name: "Capacitor Management",
        group_id: 1216,
        rank: 3,
        primary: 165,
        secondary: 166,
    },
    SkillType {
        type_id: 3426,
        name: "CPU Management",
        group_id: 1216,
        rank: 1,
        primary: 165,
        secondary: 166,
    },
    SkillType {
        type_id: 3427,
        name: "Electronic Warfare",
        group_id: 272,
        rank: 2,
        primary: 165,
        secondary: 166,
    },
    SkillType {
        type_id: 3436,
        name: "Drones",
        group_id: 273,
        rank: 1,
        primary: 166,
        secondary: 167,
    },
    SkillType {
        type_id: 3443,
        name: "Trade",
        group_id: 274,
        rank: 1,
        primary: 168,
        secondary: 164,
    },
    SkillType {
        type_id: 3449,
        name: "Navigation",
        group_id: 275,
        rank: 1,
        primary: 165,
        secondary: 167,
    },
    // --- resource processing ---------------------------------------------
    //
    // Eighteen, added when reprocessing landed. Without them every yield is
    // the bare station 0.5: `reprocess.rs` asks `Skills` for each of these
    // and a skill the sandbox does not know about trains to nothing.
    //
    // Both columns are the client's: the ids and names through
    // `types.typeNameID` and the localisation corpus, the ranks through
    // `typedogma` attribute 275. All **[V]**.
    SkillType {
        type_id: 3385,
        name: "Reprocessing",
        group_id: 1218,
        rank: 1,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 3389,
        name: "Reprocessing Efficiency",
        group_id: 1218,
        rank: 3,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 12189,
        name: "Mercoxit Ore Processing",
        group_id: 1218,
        rank: 5,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 12195,
        name: "Veldspar Processing",
        group_id: 1218,
        rank: 1,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 12196,
        name: "Scrapmetal Processing",
        group_id: 1218,
        rank: 5,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 18025,
        name: "Ice Processing",
        group_id: 1218,
        rank: 5,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 46152,
        name: "Ubiquitous Moon Ore Processing",
        group_id: 1218,
        rank: 4,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 46153,
        name: "Common Moon Ore Processing",
        group_id: 1218,
        rank: 5,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 46154,
        name: "Uncommon Moon Ore Processing",
        group_id: 1218,
        rank: 6,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 46155,
        name: "Rare Moon Ore Processing",
        group_id: 1218,
        rank: 7,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 46156,
        name: "Exceptional Moon Ore Processing",
        group_id: 1218,
        rank: 8,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 60377,
        name: "Simple Ore Processing",
        group_id: 1218,
        rank: 3,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 60378,
        name: "Coherent Ore Processing",
        group_id: 1218,
        rank: 6,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 60379,
        name: "Variegated Ore Processing",
        group_id: 1218,
        rank: 9,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 60380,
        name: "Complex Ore Processing",
        group_id: 1218,
        rank: 11,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 60381,
        name: "Abyssal Ore Processing",
        group_id: 1218,
        rank: 12,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 90040,
        name: "Erratic Ore Processing",
        group_id: 1218,
        rank: 11,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 90398,
        name: "Unrefined Minerals Processing",
        group_id: 1218,
        rank: 11,
        primary: 166,
        secondary: 165,
    },
    // --- manufacturing ---------------------------------------------------
    //
    // The three skills that modify a manufacturing job. 3380 Industry was
    // already here; these were not, so `industry::max_slots` read level 0 for
    // all three and every pilot had exactly one slot and no time bonus.
    //
    // Ranks are attribute 275 from `typedogma`, like the rest. **[V]**
    SkillType {
        type_id: 3387,
        name: "Mass Production",
        group_id: 268,
        rank: 2,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 3388,
        name: "Advanced Industry",
        group_id: 268,
        rank: 3,
        primary: 166,
        secondary: 165,
    },
    SkillType {
        type_id: 24625,
        name: "Advanced Mass Production",
        group_id: 268,
        rank: 8,
        primary: 166,
        secondary: 165,
    },
    // Invention and science (P5.4): rank, primary (intelligence 165) and
    // secondary (memory 166) from `typedogma` via `tools/dogma.py type <id>`
    // **[V]** (`docs/lanes/industry.md` §12.6).
    SkillType {
        type_id: 3403,
        name: "Research",
        group_id: 270,
        rank: 1,
        primary: 165,
        secondary: 166,
    },
    SkillType {
        type_id: 3406,
        name: "Laboratory Operation",
        group_id: 270,
        rank: 1,
        primary: 165,
        secondary: 166,
    },
    SkillType {
        type_id: 3409,
        name: "Metallurgy",
        group_id: 270,
        rank: 3,
        primary: 165,
        secondary: 166,
    },
    SkillType {
        type_id: 11445,
        name: "Minmatar Starship Engineering",
        group_id: 270,
        rank: 5,
        primary: 165,
        secondary: 166,
    },
    SkillType {
        type_id: 11452,
        name: "Mechanical Engineering",
        group_id: 270,
        rank: 5,
        primary: 165,
        secondary: 166,
    },
    SkillType {
        type_id: 21791,
        name: "Minmatar Encryption Methods",
        group_id: 270,
        rank: 5,
        primary: 165,
        secondary: 166,
    },
    SkillType {
        type_id: 24270,
        name: "Scientific Networking",
        group_id: 270,
        rank: 3,
        primary: 165,
        secondary: 166,
    },
    SkillType {
        type_id: 24624,
        name: "Advanced Laboratory Operation",
        group_id: 270,
        rank: 8,
        primary: 165,
        secondary: 166,
    },
];

/// The skills a brand-new character starts with, and the level each is at.
///
/// Seeded so that the Skills window and the character sheet have something in
/// them before anything can be bought -- otherwise step one of testing this
/// is indistinguishable from it being broken. Roughly a real starter set.
const STARTER: &[(i64, i64)] = &[
    (3300, 2), // Gunnery
    (3301, 1), // Small Hybrid Turret
    (3327, 2), // Spaceship Command
    (3330, 1), // Caldari Frigate
    (3392, 2), // Mechanics
    (3402, 1), // Science
    (3413, 2), // Power Grid Management
    (3418, 2), // Capacitor Management
    (3426, 2), // CPU Management
    (3449, 1), // Navigation
];

// ---------------------------------------------------------------------------
// Every skill: the table `--skills` loads (W9 lane L3)
//
// `SKILLS` above is 53 hand-copied rows, and a skill outside it could not be
// bought, injected or queued. The client has 512 published skills. They are
// CCP's data, so they are not copied into the repository: `tools/skills.py`
// reads them out of the player's own client into `out/skills.tsv`, and the
// server loads that file at startup with `--skills`, exactly as `--planets`
// loads PI's statics. Without the flag, `SKILLS` is the whole table, which is
// what every test runs against.
// ---------------------------------------------------------------------------

/// Cubic metres one skillbook occupies: 0.01 for every one of the 53 rows of
/// [`SKILLS`], read out of `types.fsdbinary` **[V]**. Only the fallback
/// table uses it; a loaded table carries each type's own volume.
pub const SKILLBOOK_VOLUME: f64 = 0.01;

/// `categorySkill` (`inventorycommon/const.pyc`) **[V]**: the category of
/// every skill type, and of the item an uninjected skillbook is.
pub const CATEGORY_SKILL: i64 = 16;

/// The skill table `tools/skills.py` writes: every published skill with its
/// rank, attributes, prerequisites and the item-store row of its book.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    skills: Vec<SkillType>,
    items: Vec<crate::item::ItemType>,
    required: BTreeMap<i64, Vec<(i64, i64)>>,
}

impl Table {
    /// Read `out/skills.tsv`.
    pub fn load(path: &std::path::Path) -> Result<Table, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Table::parse(&text)
    }

    /// Parse the TSV. Strict: a line we cannot read is refused with its line
    /// number, because a skill silently missing is one the player cannot
    /// buy, and nothing would say why.
    ///
    /// ```text
    /// skill  typeID groupID rank primary secondary volume required name
    /// ```
    ///
    /// `required` is `typeID=level` joined by `;`, possibly empty.
    ///
    /// **Names are leaked**, once per skill: [`SkillType::name`] is
    /// `&'static str` because [`SKILLS`] is a `const`, and a table loaded at
    /// startup lives for the whole process anyway. About 15 KB.
    pub fn parse(text: &str) -> Result<Table, String> {
        let mut t = Table::default();
        for (n, line) in text.lines().enumerate() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            let at = |why: String| format!("line {}: {why}", n + 1);
            if f.first() != Some(&"skill") {
                return Err(at(format!("unknown record {:?}", f.first())));
            }
            if f.len() != 9 {
                return Err(at(format!("{} fields, want 9", f.len())));
            }
            let int = |i: usize| -> Result<i64, String> {
                f[i].parse::<i64>()
                    .map_err(|_| at(format!("field {i} ({:?}) is not an integer", f[i])))
            };
            let type_id = int(1)?;
            let rank = int(3)?;
            // `rank` 0 is a `ZeroDivisionError` in the client's own progress
            // bar (module header), so an extract that says so is refused
            // rather than served.
            if rank < 1 {
                return Err(at(format!("skill {type_id} has rank {rank}")));
            }
            let volume: f64 = f[6]
                .parse()
                .map_err(|_| at(format!("volume {:?} is not a number", f[6])))?;
            let mut required = Vec::new();
            for pair in f[7].split(';').filter(|p| !p.is_empty()) {
                let parsed = pair
                    .split_once('=')
                    .and_then(|(a, b)| Some((a.parse::<i64>().ok()?, b.parse::<i64>().ok()?)));
                required.push(parsed.ok_or_else(|| at(format!("requirement {pair:?}")))?);
            }
            let name: &'static str = Box::leak(f[8].to_owned().into_boxed_str());
            let skill = SkillType {
                type_id,
                name,
                group_id: int(2)?,
                rank,
                primary: int(4)?,
                secondary: int(5)?,
            };
            t.items.push(skillbook(&skill, volume));
            t.skills.push(skill);
            if !required.is_empty() {
                t.required.insert(type_id, required);
            }
        }
        // A skill only `SKILLS` has -- 12195, unpublished, which reprocessing
        // still names -- keeps working with a table loaded.
        for s in SKILLS {
            if !t.skills.iter().any(|l| l.type_id == s.type_id) {
                t.items.push(skillbook(s, SKILLBOOK_VOLUME));
                t.skills.push(s.clone());
            }
        }
        Ok(t)
    }

    /// The table with no file: [`SKILLS`] and their books
    /// ([`SKILLBOOK_VOLUME`]), no prerequisites. What every free function
    /// answers with nothing installed.
    pub fn fallback() -> Table {
        Table::parse("").expect("an empty file parses")
    }

    /// How many skills, and how many of them have prerequisites.
    pub fn counts(&self) -> (usize, usize) {
        (self.skills.len(), self.required.len())
    }
}

/// The item-store row for a skill's book.
const fn skillbook(s: &SkillType, volume: f64) -> crate::item::ItemType {
    crate::item::ItemType {
        type_id: s.type_id,
        name: s.name,
        group_id: s.group_id,
        category_id: CATEGORY_SKILL,
        singleton: false,
        volume,
        capacity: 0.0,
    }
}

/// [`find_any_in`]'s body, over any skill list.
fn find_any_from<'a>(
    mut skills: impl Iterator<Item = &'a SkillType>,
    needle: &str,
) -> Option<&'a SkillType> {
    match needle.parse::<i64>() {
        Ok(id) => skills.find(|s| s.type_id == id),
        Err(_) => skills.find(|s| s.name.eq_ignore_ascii_case(needle)),
    }
}

/// Where a rule below reads the skill table: `Content` (the `_in` rules,
/// CP59b; the process `OnceLock` path went in X2b). A test may hand a rule
/// any table through it.
trait Lookup {
    fn skill(&self, type_id: i64) -> Option<&SkillType>;
    fn requires(&self, type_id: i64) -> &[(i64, i64)];
}

impl Lookup for Content {
    fn skill(&self, type_id: i64) -> Option<&SkillType> {
        find_in(self, type_id)
    }
    fn requires(&self, type_id: i64) -> &[(i64, i64)] {
        required_in(self, type_id)
    }
}

// ---------------------------------------------------------------------------
// The content table (CP59a)
// ---------------------------------------------------------------------------

/// The registration row in `content_tables::TABLES` (coordinator-only). The
/// table is a [`Table`]: the skills, their prerequisites and their books.
pub const TABLE: TableSpec = TableSpec {
    name: "skills",
    load,
    fixture: || Box::new(fixture()) as Boxed,
};

/// Invented skills, for the content fixture and the tests: ids from
/// 990_020_001, groups from 990_020_100, never a client value. The third
/// needs the first at level 2 and the second at 1. In the bundle's format
/// (`docs/content/schema.md`, `types/skills.tsv`).
pub const FIXTURE: &str = "\
# invented skill rows (CP59a fixture)
skill\t990020001\t990020100\t1\t165\t166\t0.01\t\tFixture Skill One
skill\t990020002\t990020100\t2\t167\t168\t0.01\t\tFixture Skill Two
skill\t990020003\t990020101\t5\t164\t165\t0.02\t990020001=2;990020002=1\tFixture Skill Three
";

/// [`FIXTURE`], parsed. Like every [`Table::parse`], it also carries the
/// [`SKILLS`] rows the rows do not name, so the built-in skills answer in a
/// test registry just as they do in a server with no `--skills`.
pub fn fixture() -> Table {
    Table::parse(FIXTURE).expect("the skill fixture parses")
}

/// The bundle's `types/skills.tsv`, or [`FIXTURE`] without a bundle.
///
/// A bundle without the file is [`Table::fallback`]: [`SKILLS`] alone, what
/// a server with no skills file has always had. A file that is
/// there but malformed, or adds no skill beyond [`SKILLS`], is an `Err`
/// naming it, for the reason `cli.rs` refuses the same file.
fn load(src: &Sources) -> Result<Boxed, String> {
    let Some(bundle) = src.bundle else {
        return Ok(Box::new(fixture()));
    };
    src.note(Origin::Bundle);
    let Some(path) = bundle.path(crate::content::SKILLS) else {
        return Ok(Box::new(Table::fallback()));
    };
    Ok(Box::new(from_file(&path)?))
}

/// One extract file, parsed; an error names the file.
///
/// The refusal tests what `cli.rs` means by "no skills beyond
/// the built-in ones": every row is a [`SKILLS`] row. (The flag path counts
/// rows instead, which a file of invented rows smaller than [`SKILLS`]
/// would fail.)
fn from_file(path: &std::path::Path) -> Result<Table, String> {
    let table = Table::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if table
        .skills
        .iter()
        .all(|t| SKILLS.iter().any(|s| s.type_id == t.type_id))
    {
        return Err(format!(
            "{}: no skills beyond the built-in ones. Re-extract with tools/up.sh",
            path.display()
        ));
    }
    Ok(table)
}

/// The content table. Panics, as `Content::get` does, when [`TABLE`] is not
/// registered.
pub fn table_in(content: &Content) -> &Table {
    content.get::<Table>()
}

/// Every skill `content` knows: the loaded table, or [`SKILLS`] without one
/// ([`Table::fallback`]).
pub fn all_in(content: &Content) -> impl Iterator<Item = &SkillType> {
    table_in(content).skills.iter()
}

/// One skill by type id.
pub fn find_in(content: &Content, type_id: i64) -> Option<&SkillType> {
    all_in(content).find(|s| s.type_id == type_id)
}

/// The skills, and the level of each, that `type_id` needs before it can be
/// trained -- `requiredskillsfortypes`, the same table the client's
/// `skills.GetRequiredSkills` reads **[V]**. Empty without a loaded table:
/// [`SKILLS`] carries none, so the fallback checks nothing rather than
/// guessing.
pub fn required_in(content: &Content, type_id: i64) -> &[(i64, i64)] {
    table_in(content)
        .required
        .get(&type_id)
        .map_or(&[], Vec::as_slice)
}

/// Every skillbook as an item type, for the item store's type lookup
/// (`item::find_in`): a book in a hangar is an ordinary stackable item of
/// category 16 until it is injected. The table's own books, so a table
/// built with no file ([`Table::fallback`]) carries the fallback items
/// itself.
pub fn item_types_in(content: &Content) -> &[crate::item::ItemType] {
    &table_in(content).items
}

/// Look a skill up by id or by name, case-insensitively.
///
/// Both, for the same reason `item::find_type_in` takes both: `/giveskill
/// gunnery` is what a person types and `/giveskill 3300` is what a script
/// does.
pub fn find_any_in<'a>(content: &'a Content, needle: &str) -> Option<&'a SkillType> {
    find_any_from(all_in(content), needle)
}

/// `--skills FILE`: replace the bundle's (or the fixture's) table with this
/// file's, before the world is built (CP59b, as `planet::override_in`).
/// Returns the startup line. Refused as [`TABLE`]'s `load` refuses a bundle
/// file: malformed, or no skill beyond [`SKILLS`].
pub fn override_in(content: &mut Content, path: &std::path::Path) -> Result<String, String> {
    let t = from_file(path).map_err(|e| format!("--skills {e}"))?;
    let (n, with_prerequisites) = t.counts();
    content.insert(TABLE.name, t);
    Ok(format!(
        "skills: {n} skills, {with_prerequisites} with prerequisites, from {}",
        path.display()
    ))
}

/// Skill points for a level, as the client computes them.
///
/// `ceil(rank * 250 * 2 ** (2.5 * (level - 1)))`, from
/// `characterskills/util.GetSPForLevelRaw`. Mirrored rather than invented so
/// that the progress bar we feed agrees with the one the client draws from the
/// same formula.
pub fn sp_for_level(rank: i64, level: i64) -> i64 {
    if level <= 0 {
        return 0;
    }
    let sp = rank as f64 * 250.0 * 2f64.powf(2.5 * (level - 1) as f64);
    sp.ceil() as i64
}

/// The level a skill holds at `points`: the highest level whose
/// [`sp_for_level`] is at most `points`, capped at [`MAX_LEVEL`].
///
/// `characterskills/util.GetSkillLevelRaw` **[V]** compares integers the same
/// way, so one point short of a level is still the level below.
pub fn level_for_points(rank: i64, points: i64) -> i64 {
    (1..=MAX_LEVEL)
        .take_while(|&l| sp_for_level(rank, l) <= points)
        .last()
        .unwrap_or(0)
}

/// The whitelisted dotted name of the client's skill-entry class.
const ENTRY_CLASS: &str = "characterskills.common.character_skill_entry.CharacterSkillEntry";

/// One trained skill.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillEntry {
    pub type_id: i64,
    pub level: i64,
    pub points: i64,
    pub rank: i64,
}

impl SkillEntry {
    /// The `CharacterSkillEntry` the client expects, as a `0x23` ObjectEx,
    /// through [`crate::pyutil::new_style`].
    pub fn to_pyvalue(&self) -> PyValue {
        crate::pyutil::new_style(
            ENTRY_CLASS,
            vec![
                ("_typeID", PyValue::Int(self.type_id)),
                ("_trainedSkillLevel", PyValue::Int(self.level)),
                // `_skillPoints` is the private name; the property that
                // reads it without raising is `trainedSkillPoints`.
                ("_skillPoints", PyValue::Int(self.points)),
                ("_skillRank", PyValue::Int(self.rank)),
                // `None` unless an expert system is running, which we do
                // not implement. `effectiveSkillLevel` is derived as
                // `max(trainedSkillLevel, virtualSkillLevel)`, and `max`
                // against `None` in Python 2 picks the integer.
                ("_virtualSkillLevel", PyValue::None),
            ],
        )
    }
}

/// Why a purchase was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum BuyError {
    NotASkill(i64),
    AlreadyKnown(i64),
}

/// One character's skills.
#[derive(Debug, Clone, Default)]
pub struct Skills {
    entries: BTreeMap<i64, SkillEntry>,
}

impl Skills {
    /// An empty set, for a character with no history.
    pub fn new() -> Self {
        Skills {
            entries: BTreeMap::new(),
        }
    }

    /// The starter set, so there is something to look at, ranks from
    /// `content`.
    pub fn starter_in(content: &Content) -> Self {
        Self::starter_with(content)
    }

    fn starter_with(table: &(impl Lookup + ?Sized)) -> Self {
        let mut s = Skills::new();
        for &(type_id, level) in STARTER {
            let rank = table.skill(type_id).map(|t| t.rank).unwrap_or(1);
            s.entries.insert(
                type_id,
                SkillEntry {
                    type_id,
                    level,
                    points: sp_for_level(rank, level),
                    rank,
                },
            );
        }
        s
    }

    pub fn entries(&self) -> impl Iterator<Item = &SkillEntry> {
        self.entries.values()
    }

    /// One skill, if the character has it injected.
    pub fn get(&self, type_id: i64) -> Option<&SkillEntry> {
        self.entries.get(&type_id)
    }

    /// One skill, mutably. Training moves `points` and `level` through it.
    pub fn get_mut(&mut self, type_id: i64) -> Option<&mut SkillEntry> {
        self.entries.get_mut(&type_id)
    }

    /// Skills read back from disk.
    pub fn restore(entries: Vec<SkillEntry>) -> Self {
        Skills {
            entries: entries.into_iter().map(|e| (e.type_id, e)).collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Total trained skill points, which is what the character sheet shows.
    pub fn total_points(&self) -> i64 {
        self.entries.values().map(|e| e.points).sum()
    }

    /// `{typeID: CharacterSkillEntry}`, the shape both read calls want.
    pub fn to_pyvalue(&self) -> PyValue {
        // **Marshal dict entries are `(value, key)`, not `(key, value)`.**
        // `pyutil::pydict` encodes that inversion once, but it only takes
        // `&str` keys and these keys are type ids -- so the inversion is
        // written out here, deliberately, with the value first. Getting this
        // backwards is the bug that sent `{500: 'maxBlueprintResults'}` last
        // round and poisoned a service for the session.
        PyValue::Dict(
            self.entries
                .values()
                .map(|e| (e.to_pyvalue(), PyValue::Int(e.type_id)))
                .collect(),
        )
    }

    /// Set a skill to a level outright, as a GM grant.
    ///
    /// Not a purchase: no validation against what the character already has,
    /// because the point of the command is to put the world in a state for
    /// testing. The points follow the level through the client's own formula,
    /// so the progress bar agrees with the number beside it.
    pub fn grant_in(&mut self, content: &Content, type_id: i64, level: i64) -> Option<&SkillEntry> {
        let t = find_in(content, type_id)?;
        Some(self.grant_type(t, level))
    }

    /// [`Self::grant_in`] for a skill already looked up: for a caller that
    /// cannot hold `reg.content()` and the skill sheet at once (`/giveskill`
    /// finds the row with [`find_any_in`] first).
    pub fn grant_type(&mut self, t: &SkillType, level: i64) -> &SkillEntry {
        let type_id = t.type_id;
        let level = level.clamp(0, 5);
        self.entries.insert(
            type_id,
            SkillEntry {
                type_id,
                level,
                points: sp_for_level(t.rank, level),
                rank: t.rank,
            },
        );
        &self.entries[&type_id]
    }

    /// Forget a skill entirely. `true` if it was there.
    pub fn remove(&mut self, type_id: i64) -> bool {
        self.entries.remove(&type_id).is_some()
    }

    /// Buy skills, all or nothing.
    ///
    /// **All or nothing is not a preference.** `buy_missing_skills` waits on
    /// the *request* list rather than on our answer, so declining one id of
    /// several leaves the client spinning on the ones we declined, exactly as
    /// if we had declined all of them. Validate everything first, then commit.
    pub fn purchase_in(
        &mut self,
        content: &Content,
        type_ids: &[i64],
    ) -> Result<Vec<i64>, BuyError> {
        self.purchase_with(content, type_ids)
    }

    fn purchase_with(
        &mut self,
        table: &(impl Lookup + ?Sized),
        type_ids: &[i64],
    ) -> Result<Vec<i64>, BuyError> {
        for &id in type_ids {
            if table.skill(id).is_none() {
                return Err(BuyError::NotASkill(id));
            }
            if self.entries.contains_key(&id) {
                return Err(BuyError::AlreadyKnown(id));
            }
        }
        let mut bought = Vec::new();
        for &id in type_ids {
            let rank = table.skill(id).map(|t| t.rank).unwrap_or(1);
            // Level 0 with no points: bought, injected, untrained. That is
            // what a freshly injected skillbook is, and the client renders it
            // as "Level 0" rather than as missing.
            self.entries.insert(
                id,
                SkillEntry {
                    type_id: id,
                    level: 0,
                    points: 0,
                    rank,
                },
            );
            bought.push(id);
        }
        Ok(bought)
    }
}

/// `OnSkillForcedRefresh()`, with **no arguments at all**.
///
/// `SkillsSvc.OnSkillForcedRefresh` calls `Reset()`, which sets the three
/// skill caches back to `None` and so forces the next read to be a real
/// re-ask. That is the only thing that can end the purchase spin described on
/// `Registry::purchase_skills`.
///
/// Zero arguments means zero new marshal shapes, which is why this was the
/// push to ship first. `OnServerSkillsChanged` is the better one -- it carries
/// the changed entries and avoids seven re-asks -- and it needs a monotonic
/// timestamp whose magnitude is load-bearing, so it is a separate step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForcedRefresh;

impl crate::notify::Push for ForcedRefresh {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnSkillForcedRefresh", vec![])
    }
}

// ===========================================================================
// Skill training: the queue, real time, completion (W8 lane E)
//
// `docs/lanes/skill-training.md` is the park note: every call, what it
// answers, and what is `[A]`. The arms are in `service/character.rs`'s
// `Skill training` section; everything here is the model, which knows no
// `Registry`.
//
// **The model in one paragraph.** A queue is a list of `(typeID, toLevel)`
// plus one number: the FILETIME at which entry 0 started training, or `None`
// when paused. Every other time is *derived* -- entry `i` starts when entry
// `i-1` ends, and an end is the start plus the client's own training-time
// formula. Nothing is ticking a counter, so a restart that keeps the start
// time loses nothing (`docs/reference/world-state.md` §2 item 7 named exactly this),
// and a skill that finished while the server was down is credited by the
// first settle after it comes back. The skill being trained keeps the points
// it had *when it started*; the client estimates the rest from
// `trainingStartTime` itself, and that is not a shortcut: it is the contract.
// `get_skill_points_trained_at_sample_time` (`characterskills/
// skill_training.py:157`) adds the elapsed SP to `trainedSkillPoints`, so a
// server that moved `trainedSkillPoints` live would be counted twice.
// ===========================================================================

/// `carbon.common.lib.const.MIN`: one minute in blue FILETIME ticks. `SEC` is
/// `10000000` and `MIN` is `SEC * 60` (`carbon/common/lib/const.py:278`-
/// `:279`) **[V]**.
pub const MIN: i64 = 60 * 10_000_000;

/// `characterskills.MAX_SKILL_LEVEL` (`characterskills/const.py:3`) **[V]**.
pub const MAX_LEVEL: i64 = 5;

/// `SKILLQUEUE_MAX_NUM_SKILLS` (`characterskills/queue.py`, module body)
/// **[V]**. The client enforces it before it saves; we refuse past it rather
/// than truncate, so a queue cannot shrink without the player seeing why.
pub const MAX_QUEUE_ENTRIES: usize = 150;

/// Skill points per minute: `primary + secondary / 2.0`.
///
/// `characterskills/util.GetSkillPointsPerMinute` (`util.py:36`-`:37`)
/// **[V]**, fed by `skillsvc.GetSkillpointsPerMinute` (`skillsvc.py:782`)
/// with the two attributes the skill names in `typedogma` (180 and 181, the
/// [`SkillType::primary`] and [`SkillType::secondary`] columns). At a new
/// character's attributes (`service::character::STARTING_ATTRIBUTES`) that
/// is 30 SP a minute, or 29.5 / 29 for a skill with charisma secondary /
/// primary.
///
/// No alpha halving: `GetCharacterAttribute` multiplies by
/// `ALPHA_TRAINING_MULTIPLIER` only when the clone is not omega
/// (`skillsvc.py:266`), and `GetCloneGrade` answers omega
/// (`service/base.rs`).
pub fn sp_per_minute(primary: f64, secondary: f64) -> f64 {
    primary + secondary / 2.0
}

/// How long `sp` skill points take, in FILETIME ticks.
///
/// `long(float(remaining_skillpoints) / float(skillpoints_per_minute) *
/// MIN)` -- `SkillTrainingTimeCalculator._get_training_time_given_skill_
/// points_to_train`, `characterskills/skill_training.py:140` **[V]**. Same
/// operation order, same truncation, so our end time and the one the client
/// computes for its own queue panel agree to the tick. Boosters are the
/// loop above that line; `crate::boosters` makes them part of the rate, so
/// the rate this takes already has them.
pub fn training_ticks(sp: i64, rate: f64) -> i64 {
    if sp <= 0 || rate <= 0.0 {
        return 0;
    }
    (sp as f64 / rate * MIN as f64) as i64
}

/// Skill points trained in `elapsed` ticks: `time / float(MIN) * rate`,
/// truncated by the caller's `int(...)` -- `get_skill_points_trained_at_
/// sample_time`, `skill_training.py:179` **[V]**. The inverse of
/// [`training_ticks`], used when a pause credits a partial level.
pub fn sp_trained(elapsed: i64, rate: f64) -> i64 {
    if elapsed <= 0 || rate <= 0.0 {
        return 0;
    }
    (elapsed as f64 / MIN as f64 * rate) as i64
}

/// The training rate of one skill for a character whose attribute values
/// `attribute` answers. `None` for a skill the sandbox does not know, which
/// cannot be trained because it has no primary attribute to read.
pub fn rate_for_in(content: &Content, type_id: i64, attribute: &dyn Fn(i64) -> f64) -> Option<f64> {
    rate_with(content, type_id, attribute)
}

fn rate_with(
    table: &(impl Lookup + ?Sized),
    type_id: i64,
    attribute: &dyn Fn(i64) -> f64,
) -> Option<f64> {
    let t = table.skill(type_id)?;
    Some(sp_per_minute(attribute(t.primary), attribute(t.secondary)))
}

/// Where a level starts counting from: the skill's own points if it is the
/// very next level, else the floor of the level below.
///
/// `characterskills/queue.GetQueueEntry` (`queue.py:21`) **[V]**:
/// `trainingStartSP = currentSkill.trainedSkillPoints if
/// currentSkill.trainedSkillLevel == skillLevel - 1 else
/// GetSPForLevelRaw(currentSkill.skillRank, skillLevel - 1)`. So level 3
/// queued behind level 2 of the same skill starts from level 2's floor.
fn start_sp(skill: &SkillEntry, to_level: i64) -> i64 {
    if skill.level == to_level - 1 {
        skill.points
    } else {
        sp_for_level(skill.rank, to_level - 1)
    }
}

/// One queue entry as the client reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueRow {
    pub position: i64,
    pub type_id: i64,
    pub to_level: i64,
    pub start_sp: i64,
    pub destination_sp: i64,
    /// A blue FILETIME, or `None` while the queue is paused.
    pub start: Option<i64>,
    pub end: Option<i64>,
}

impl QueueRow {
    /// A `utillib.KeyVal` with the seven fields of
    /// `characterskills/queue.SkillQueueEntry` (`queue.py:45`-`:49`) **[V]**,
    /// which is what the client builds for itself in `GetQueueEntry`.
    ///
    /// Mutable, not a `DBRow`: `OnSkillQueuePausedServer` assigns
    /// `skillEntry.trainingStartTime = skillEntry.trainingEndTime = None`
    /// (`skillQueueSvc.py:1224`). Times are FILETIME magnitude or `None`,
    /// never 0: `SkillInTraining` (`skillQueueSvc.py:1506`) reads a truthy
    /// `queue[0].trainingEndTime` as "training", and 0 is "ended in 1601".
    pub fn to_pyvalue(&self) -> PyValue {
        let time = |t: Option<i64>| t.map_or(PyValue::None, PyValue::Int);
        crate::pyutil::keyval(vec![
            ("queuePosition", PyValue::Int(self.position)),
            ("trainingStartTime", time(self.start)),
            ("trainingEndTime", time(self.end)),
            ("trainingTypeID", PyValue::Int(self.type_id)),
            ("trainingToLevel", PyValue::Int(self.to_level)),
            ("trainingStartSP", PyValue::Int(self.start_sp)),
            ("trainingDestinationSP", PyValue::Int(self.destination_sp)),
        ])
    }
}

/// Why a queue was refused. Each maps to a label the client raises for the
/// same condition itself (`skillQueueSvc.CheckCanInsertSkillAtPosition`), so
/// reaching one means the client's own check was bypassed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueError {
    /// `QueueSkillNotUploaded`: the character does not have the skill.
    NotInjected(i64),
    /// `QueueCannotPlaceSkillLevelsOutOfOrder`: level N without N-1 trained
    /// or ahead of it, or the same level twice.
    OutOfOrder(i64, i64),
    /// A level outside 1..=5.
    BadLevel(i64, i64),
    /// More than [`MAX_QUEUE_ENTRIES`].
    TooLong(usize),
    /// `QueueCannotPlaceSkillBeforeRequirements`: `(skill, prerequisite)`,
    /// the prerequisite neither trained far enough nor queued far enough
    /// ahead of it. See [`unmet_requirement`].
    BeforeRequirements(i64, i64),
}

/// The first prerequisite of `type_id` that is neither trained to its level
/// nor queued to it earlier in `ahead`, or `None` when all are met.
///
/// `skillQueueSvc.CheckCanInsertSkillAtPosition`, `skillQueueSvc.py:313`-
/// `:328` **[V]**, mirrored so that a queue the client would refuse is
/// refused here too and one it accepts is accepted:
///
/// - checked only for a skill with **no points at all**
///   (`mySkill.trainedSkillPoints == 0`, `:313`). A skill already partly
///   trained is never re-judged, so a GM `/removeskill` of a prerequisite
///   cannot strand a skill half-way;
/// - met when the prerequisite's `trainedSkillLevel >= requiredLevel`
///   (`:326`), or when it is queued at `>= requiredLevel` *before* this
///   entry (`:321`).
///
/// `requirements` is the skill's `(typeID, level)` list from
/// [`required_in`]; a parameter so the rule is testable without a loaded table.
pub fn unmet_requirement(
    skills: &Skills,
    ahead: &[(i64, i64)],
    type_id: i64,
    requirements: &[(i64, i64)],
) -> Option<i64> {
    if skills.get(type_id).is_some_and(|s| s.points != 0) {
        return None;
    }
    requirements
        .iter()
        .find(|&&(req, level)| {
            let trained = skills.get(req).is_some_and(|s| s.level >= level);
            let queued = ahead.iter().any(|&(t, l)| t == req && l >= level);
            !trained && !queued
        })
        .map(|&(req, _)| req)
}

/// One character's training queue.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Queue {
    /// `(typeID, toLevel)`, in training order.
    pub entries: Vec<(i64, i64)>,
    /// When entry 0 started training, or `None` while paused.
    pub start: Option<i64>,
}

impl Queue {
    /// Training right now: a start time and something to train.
    pub fn is_training(&self) -> bool {
        self.start.is_some() && !self.entries.is_empty()
    }

    /// Drop every entry that can no longer train: a skill the character no
    /// longer has (a GM `/removeskill`), a skill with no known attributes, or
    /// a level already reached (a GM `/giveskill` past it). `true` if
    /// anything went.
    ///
    /// **[A]** the next entry inherits the dropped head's start time rather
    /// than starting now. It only arises from a GM command, and starting now
    /// would need a clock this function deliberately does not take.
    pub fn prune_in(&mut self, content: &Content, skills: &Skills) -> bool {
        self.prune_with(content, skills)
    }

    fn prune_with(&mut self, table: &(impl Lookup + ?Sized), skills: &Skills) -> bool {
        let before = self.entries.len();
        self.entries.retain(|&(t, lvl)| {
            table.skill(t).is_some()
                && (1..=MAX_LEVEL).contains(&lvl)
                && skills.get(t).is_some_and(|s| s.level < lvl)
        });
        if self.entries.is_empty() {
            self.start = None;
        }
        self.entries.len() != before
    }

    /// The queue as the client reads it, times derived from [`Self::start`].
    /// Call [`Self::prune_in`] first; an entry whose skill is missing is skipped.
    pub fn rows_in(
        &self,
        content: &Content,
        skills: &Skills,
        attribute: &dyn Fn(i64) -> f64,
    ) -> Vec<QueueRow> {
        self.rows_with(content, skills, attribute)
    }

    fn rows_with(
        &self,
        table: &(impl Lookup + ?Sized),
        skills: &Skills,
        attribute: &dyn Fn(i64) -> f64,
    ) -> Vec<QueueRow> {
        let mut at = self.start;
        let mut rows = Vec::new();
        for &(type_id, to_level) in &self.entries {
            let Some(skill) = skills.get(type_id) else {
                continue;
            };
            let from = start_sp(skill, to_level);
            let destination = sp_for_level(skill.rank, to_level);
            let (start, end) = match (at, rate_with(table, type_id, attribute)) {
                (Some(s), Some(rate)) => {
                    (Some(s), Some(s + training_ticks(destination - from, rate)))
                }
                _ => (None, None),
            };
            rows.push(QueueRow {
                position: rows.len() as i64,
                type_id,
                to_level,
                start_sp: from,
                destination_sp: destination,
                start,
                end,
            });
            at = end;
        }
        rows
    }

    /// Train every level whose end time has passed, oldest first. Returns
    /// the type ids whose level moved, each once.
    ///
    /// The next entry starts at the previous one's **end**, not at `now`, so
    /// a settle that runs late (a slow tick, or the first one after a
    /// restart) loses no time -- a character offline for a week comes back
    /// with a week of training, as on TQ.
    pub fn settle_in(
        &mut self,
        content: &Content,
        skills: &mut Skills,
        now: i64,
        attribute: &dyn Fn(i64) -> f64,
    ) -> Vec<i64> {
        self.settle_with(content, skills, now, attribute)
    }

    fn settle_with(
        &mut self,
        table: &(impl Lookup + ?Sized),
        skills: &mut Skills,
        now: i64,
        attribute: &dyn Fn(i64) -> f64,
    ) -> Vec<i64> {
        self.prune_with(table, skills);
        let mut trained = Vec::new();
        while let (Some(start), Some(&(type_id, to_level))) = (self.start, self.entries.first()) {
            let (Some(skill), Some(rate)) = (
                skills.get_mut(type_id),
                rate_with(table, type_id, attribute),
            ) else {
                self.entries.remove(0);
                continue;
            };
            if skill.level >= to_level {
                self.entries.remove(0);
                continue;
            }
            let destination = sp_for_level(skill.rank, to_level);
            let end = start + training_ticks(destination - start_sp(skill, to_level), rate);
            if end > now {
                break;
            }
            skill.level = to_level;
            skill.points = skill.points.max(destination);
            if !trained.contains(&type_id) {
                trained.push(type_id);
            }
            self.entries.remove(0);
            self.start = Some(end);
        }
        if self.entries.is_empty() {
            self.start = None;
        }
        trained
    }

    /// Stop training, crediting the head skill with what it trained so far.
    /// Returns its type id when its points moved, so the caller can tell the
    /// client.
    ///
    /// Capped one point short of the level: reaching it is [`Self::settle_in`]'s
    /// job, and a skill holding its next level's points at its old level is
    /// a state the client's `GetSkillLevelRaw` would read as the higher one.
    pub fn pause_in(
        &mut self,
        content: &Content,
        skills: &mut Skills,
        now: i64,
        attribute: &dyn Fn(i64) -> f64,
    ) -> Option<i64> {
        self.pause_with(content, skills, now, attribute)
    }

    fn pause_with(
        &mut self,
        table: &(impl Lookup + ?Sized),
        skills: &mut Skills,
        now: i64,
        attribute: &dyn Fn(i64) -> f64,
    ) -> Option<i64> {
        let start = self.start.take()?;
        let &(type_id, to_level) = self.entries.first()?;
        let rate = rate_with(table, type_id, attribute)?;
        let skill = skills.get_mut(type_id)?;
        if skill.level != to_level - 1 {
            return None;
        }
        let ceiling = sp_for_level(skill.rank, to_level) - 1;
        let points = (skill.points + sp_trained(now - start, rate))
            .min(ceiling)
            .max(skill.points);
        if points == skill.points {
            return None;
        }
        skill.points = points;
        Some(type_id)
    }

    /// Replace the queue with what the client saved.
    ///
    /// Call [`Self::settle_in`] first, so that a level which finished a moment
    /// ago is credited before the new list is judged.
    ///
    /// `activate` **does not pause.** The queue panel saves with
    /// `activate=not self.IsTraining()` (`ApplySkillQueueIfNotActive`) and
    /// with `activate=isTraining` (`SaveSkillQueue`), so a save *while
    /// training* arrives with either value and means "keep going"; the pause
    /// button is `AbortTraining` first and a save after it
    /// (`skillQueuePanelNew.PauseTraining`) **[V]** call sites, **[I]**
    /// meaning. So: training stays training; a paused queue starts only when
    /// `activate` asks.
    ///
    /// While training, a save that keeps the same head keeps its start time
    /// (the common case: appending). A new head credits the old one's
    /// progress ([`Self::pause_in`]) and starts now. Returns the credited type
    /// id, if any.
    ///
    /// **[A]** a level the character already has is dropped rather than
    /// refused. It arises when a level completes between the client building
    /// its list and us receiving it; refusing would roll the player's edit
    /// back (`CommitTransaction`'s `except`, `skillQueueSvc.py:227`) over a
    /// race they cannot see.
    pub fn save_in(
        &mut self,
        content: &Content,
        skills: &mut Skills,
        wanted: &[(i64, i64)],
        activate: bool,
        now: i64,
        attribute: &dyn Fn(i64) -> f64,
    ) -> Result<Option<i64>, QueueError> {
        self.save_with(content, skills, wanted, activate, now, attribute)
    }

    fn save_with(
        &mut self,
        table: &(impl Lookup + ?Sized),
        skills: &mut Skills,
        wanted: &[(i64, i64)],
        activate: bool,
        now: i64,
        attribute: &dyn Fn(i64) -> f64,
    ) -> Result<Option<i64>, QueueError> {
        if wanted.len() > MAX_QUEUE_ENTRIES {
            return Err(QueueError::TooLong(wanted.len()));
        }
        let mut kept: Vec<(i64, i64)> = Vec::new();
        for &(type_id, level) in wanted {
            if !(1..=MAX_LEVEL).contains(&level) {
                return Err(QueueError::BadLevel(type_id, level));
            }
            let Some(skill) = skills
                .get(type_id)
                .filter(|_| table.skill(type_id).is_some())
            else {
                return Err(QueueError::NotInjected(type_id));
            };
            if level <= skill.level {
                continue;
            }
            let prior_ready = level - 1 <= skill.level || kept.contains(&(type_id, level - 1));
            if !prior_ready || kept.contains(&(type_id, level)) {
                return Err(QueueError::OutOfOrder(type_id, level));
            }
            if let Some(missing) =
                unmet_requirement(skills, &kept, type_id, table.requires(type_id))
            {
                return Err(QueueError::BeforeRequirements(type_id, missing));
            }
            kept.push((type_id, level));
        }
        let mut credited = None;
        if self.is_training() {
            if kept.first() != self.entries.first() {
                credited = self.pause_with(table, skills, now, attribute);
                self.start = Some(now);
            }
        } else {
            self.start = activate.then_some(now);
        }
        self.entries = kept;
        if self.entries.is_empty() {
            self.start = None;
        }
        Ok(credited)
    }
}

/// Every character's queue. Persisted, opaque to `store.rs` like
/// `service::Clones`: this type renders and parses its own records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Queues {
    by_character: BTreeMap<i64, Queue>,
}

impl Queues {
    /// The record tags this type reads and writes in the world file.
    pub const RECORDS: &'static [&'static str] = &["skillqueue"];

    pub fn get(&self, character_id: i64) -> Option<&Queue> {
        self.by_character.get(&character_id)
    }

    /// This character's queue, created empty on first use.
    pub fn entry(&mut self, character_id: i64) -> &mut Queue {
        self.by_character.entry(character_id).or_default()
    }

    /// Characters with something training, for the clock.
    pub fn training(&self) -> Vec<i64> {
        self.by_character
            .iter()
            .filter(|(_, q)| q.is_training())
            .map(|(id, _)| *id)
            .collect()
    }

    /// Append this type's records to a world file being written.
    ///
    /// ```text
    /// skillqueue <char> <start FILETIME, or -1 paused> (<typeID> <toLevel>)*
    /// ```
    ///
    /// One line per character with a non-empty queue, so a world with no
    /// training reads back identical to one written before this existed.
    pub fn render(&self, out: &mut String) {
        use std::fmt::Write as _;
        for (id, q) in &self.by_character {
            if q.entries.is_empty() {
                continue;
            }
            let _ = write!(out, "skillqueue\t{id}\t{}", q.start.unwrap_or(-1));
            for (t, lvl) in &q.entries {
                let _ = write!(out, "\t{t}\t{lvl}");
            }
            out.push('\n');
        }
    }

    /// Read one record whose tag is in [`Queues::RECORDS`]. Strict, like the
    /// clones: a line we cannot read is training the player would silently
    /// lose, so the world is refused and the line named.
    pub fn parse_record(&mut self, f: &[&str]) -> Result<(), String> {
        if f.first().copied() != Some("skillqueue") {
            return Err(format!("{:?} is not a skill queue record", f.first()));
        }
        let num = |i: usize| -> Result<i64, String> {
            f.get(i)
                .and_then(|v| v.parse::<i64>().ok())
                .ok_or_else(|| format!("field {i} is not an integer"))
        };
        if f.len() < 3 || !(f.len() - 3).is_multiple_of(2) {
            return Err(format!("{} fields; want 3 plus pairs", f.len()));
        }
        let character_id = num(1)?;
        let start = match num(2)? {
            -1 => None,
            t => Some(t),
        };
        let mut entries = Vec::new();
        for i in (3..f.len()).step_by(2) {
            entries.push((num(i)?, num(i + 1)?));
        }
        self.by_character
            .insert(character_id, Queue { entries, start });
        Ok(())
    }
}

/// `OnServerSkillsChanged(skillInfos, event, timeStamp)`, to one character.
///
/// `skillsvc.OnServerSkillsChanged` (`skillsvc.py:186`) **[V]**: writes both
/// skill caches, hands the dict to `skillqueue.OnServerSkillsChanged` (which
/// drops the trained level from the client's queue, `skillQueueSvc.py:1179`),
/// and -- when a level went up -- plays `msg_SkillTrained_play` (`:191`) and
/// raises the "skill trained" notification. That is the whole "the client
/// is notified" of a completed level; nothing else is needed.
///
/// - `skillInfos`: `{typeID: CharacterSkillEntry}`. Never an entry with
///   `trainedSkillPoints = None` -- that branch is "remove this skill"
///   (`_UpdateMySkillsAndReturnLevelsTrained`, `skillsvc.py:205`).
/// - `event`: `None`. A name would be re-scattered, and must exist.
/// - `timeStamp`: a FILETIME, increasing. The handler skips an entry whose
///   cached `timeStamp` is greater (`skillsvc.py:213`), silently, so a small
///   or repeated value is a lost push. `filetime_now` at send time.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillsChanged {
    pub character_id: i64,
    pub entries: Vec<SkillEntry>,
    pub time_stamp: i64,
}

impl crate::notify::Push for SkillsChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        // `(value, key)` on the wire, as in `Skills::to_pyvalue`.
        let infos = PyValue::Dict(
            self.entries
                .iter()
                .map(|e| (e.to_pyvalue(), PyValue::Int(e.type_id)))
                .collect(),
        );
        (
            "OnServerSkillsChanged",
            vec![infos, PyValue::None, PyValue::Int(self.time_stamp)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character_id])
    }
}

/// `OnSkillQueuePausedServer()`, no arguments, to one character.
///
/// The handler (`skillQueueSvc.py:1212`) blanks every entry's times and
/// plays `skill_training_stop_play`; nothing on the client emits it
/// (`findings/push-events.tsv`, `local_emitters=0`), so a pause the server
/// performed is otherwise silent. **[A]** that TQ sent it after
/// `AbortTraining`: the client's own pause button calls `AbortTraining` and
/// then saves, and this is the only server-only event that says "paused".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueuePaused {
    pub character_id: i64,
}

impl crate::notify::Push for QueuePaused {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnSkillQueuePausedServer", vec![])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character_id])
    }
}

/// `OnFreeSkillPointsChanged(newFreeSkillPoints)`, one scalar, to one
/// character.
///
/// `skillsvc.OnFreeSkillPointsChanged` -> `SetFreeSkillPoints` **[V]**. The
/// client's own apply calls discard the server's answer for the queue and
/// skills paths, so this push is the only thing that moves its pool to the new
/// number; sent after every apply, and after every grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreeSkillPointsChanged {
    pub character_id: i64,
    pub points: i64,
}

impl crate::notify::Push for FreeSkillPointsChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnFreeSkillPointsChanged", vec![PyValue::Int(self.points)])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character_id])
    }
}

/// `OnNewSkillQueueSaved(newQueue)`, to one character: the whole queue,
/// replaced. `rows` is the `KeyVal` list `GetSkillQueueAndFreePoints` serves.
///
/// `skillQueueSvc` replaces its queue and scatters `OnSkillQueueChanged`
/// **[V, read]**. **[A]** the queue panel redraws from it; falsifier: a
/// scenario that shows the old end time after a pace change.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueSaved {
    pub character_id: i64,
    pub rows: PyValue,
}

impl crate::notify::Push for QueueSaved {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnNewSkillQueueSaved", vec![self.rows.clone()])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character_id])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_marshal::ObjectForm;

    /// The skill table the free functions read with nothing installed
    /// ([`Table::fallback`]: [`SKILLS`], no prerequisites), as a `Content`:
    /// what the tests below read through the `_in` twins (X2a).
    fn builtin() -> Content {
        let mut c = Content::default();
        c.insert(TABLE.name, Table::fallback());
        c
    }

    /// The ranks are the client's, and this is what says so.
    ///
    /// Eight of these were 1 until `typedogma` could be read, and a rank of 1
    /// is not a wrong *shape* -- the client renders it and the skill queue
    /// works -- so nothing failed. It showed only as two different training
    /// times for one skill in two panels, which is exactly the kind of thing
    /// a stopgap can sit in for months. Spot values, from attribute 275:
    /// `typedogma` is where they came from and `docs/re/dogma.md` §12 is how.
    #[test]
    fn the_ranks_are_the_clients_own_and_not_all_one() {
        let c = builtin();
        for (type_id, want) in [
            (3300, 1),   // Gunnery
            (3328, 2),   // Gallente Frigate
            (3394, 2),   // Hull Upgrades
            (3412, 3),   // Astrometrics
            (3418, 3),   // Capacitor Management
            (3427, 2),   // Electronic Warfare
            (3385, 1),   // Reprocessing
            (3389, 3),   // Reprocessing Efficiency
            (60381, 12), // Abyssal Ore Processing, the highest here
        ] {
            let s = find_in(&c, type_id).expect("a skill this test names must exist");
            assert_eq!(s.rank, want, "{} ({type_id})", s.name);
        }
        assert!(
            SKILLS.iter().filter(|s| s.rank != 1).count() >= 20,
            "if nearly every rank is 1 again, the table has been reset to the stopgap"
        );
    }

    /// No id twice. A duplicate is not a compile error and `find` would
    /// silently answer with the first, so the second row's rank would never
    /// be used by anything.
    #[test]
    fn every_skill_id_appears_once() {
        let mut ids: Vec<i64> = SKILLS.iter().map(|s| s.type_id).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "SKILLS has a duplicate type id");
    }

    #[test]
    fn the_starter_set_has_points_and_every_id_is_a_known_skill() {
        let s = Skills::starter_in(&builtin());
        assert_eq!(s.len(), STARTER.len());
        assert!(
            s.total_points() > 0,
            "a character sheet showing 0 SP is indistinguishable from a broken one"
        );
        for e in s.entries() {
            assert!(
                find_in(&builtin(), e.type_id).is_some(),
                "starter skill {} is not in SKILLS",
                e.type_id
            );
            assert!(e.rank >= 1, "rank 0 raises ZeroDivisionError in the UI");
        }
    }

    /// The formula is the client's own, so a level we send and a level it
    /// computes have to agree. Rank 1 level 1 is 250 SP, which is the one
    /// value in this formula that is common knowledge.
    #[test]
    fn skill_points_match_the_clients_own_formula() {
        assert_eq!(sp_for_level(1, 1), 250);
        assert_eq!(sp_for_level(1, 2), 1415);
        assert_eq!(sp_for_level(2, 1), 500);
        assert_eq!(sp_for_level(1, 0), 0);
        // Monotonic, because `GetLevelProgress` divides by the difference
        // between consecutive levels and a non-positive gap divides by zero.
        for rank in 1..=5 {
            for level in 1..5 {
                assert!(
                    sp_for_level(rank, level + 1) > sp_for_level(rank, level),
                    "rank {rank} level {level}"
                );
            }
        }
    }

    #[test]
    fn a_purchase_is_all_or_nothing() {
        let mut s = Skills::new();
        // One bad id in the list declines the whole request, because the
        // client waits on the ids it asked for rather than the ones we
        // granted -- a partial answer is the same spin as a refusal.
        assert_eq!(
            s.purchase_in(&builtin(), &[3300, 999_999]),
            Err(BuyError::NotASkill(999_999))
        );
        assert!(s.is_empty(), "nothing is granted when anything is refused");

        assert_eq!(
            s.purchase_in(&builtin(), &[3300, 3301])
                .expect("both are skills")
                .len(),
            2
        );
        assert_eq!(s.len(), 2);
        assert_eq!(
            s.purchase_in(&builtin(), &[3301]),
            Err(BuyError::AlreadyKnown(3301)),
            "buying a skill twice is a mistake worth naming, not a no-op"
        );
    }

    #[test]
    fn a_bought_skill_is_injected_at_level_zero() {
        let mut s = Skills::new();
        s.purchase_in(&builtin(), &[3300]).expect("bought");
        let e = s.entries().next().expect("one entry");
        assert_eq!((e.level, e.points), (0, 0));
        assert!(e.rank >= 1);
    }

    /// **The switch to `pyutil::new_style` changed no byte on the wire.**
    ///
    /// The expected value is the hand-built `0x23` literal `to_pyvalue`
    /// returned before the helper existed, frozen here verbatim. That literal
    /// is the one the client accepted [V], so the comparison is on the
    /// encoded bytes, not on the enum: the bytes are what the client sees.
    /// Invented ids, because the shape is the subject.
    #[test]
    fn the_entry_encodes_byte_identically_to_the_hand_built_form() {
        let before = |e: &SkillEntry| PyValue::ObjectEx {
            form: ObjectForm::New,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::Tuple(vec![PyValue::Global(
                    b"characterskills.common.character_skill_entry.CharacterSkillEntry".to_vec(),
                )]),
                crate::pyutil::pydict(vec![
                    ("_typeID", PyValue::Int(e.type_id)),
                    ("_trainedSkillLevel", PyValue::Int(e.level)),
                    ("_skillPoints", PyValue::Int(e.points)),
                    ("_skillRank", PyValue::Int(e.rank)),
                    ("_virtualSkillLevel", PyValue::None),
                ]),
            ])),
            list: Vec::new(),
            dict: Vec::new(),
        };
        let encode = |v: &PyValue| {
            let mut w = eve_marshal::Writer::new();
            eve_marshal::save(v, &mut w);
            w.into_bytes()
        };
        // Values across the integer encodings: the cached constants, int8,
        // int16 and int32.
        for e in [
            SkillEntry {
                type_id: 900_001,
                level: 0,
                points: 0,
                rank: 1,
            },
            SkillEntry {
                type_id: 900_002,
                level: 5,
                points: 1_280_000,
                rank: 16,
            },
            SkillEntry {
                type_id: 900_003,
                level: -1,
                points: 250,
                rank: 3,
            },
        ] {
            assert_eq!(e.to_pyvalue(), before(&e));
            assert_eq!(encode(&e.to_pyvalue()), encode(&before(&e)));
        }
    }

    /// The state keys are the **private** names, because the client builds the
    /// instance with `__new__` and updates `__dict__` -- `__init__` never
    /// runs. And the dict is `(value, key)` on the wire.
    #[test]
    fn the_entry_marshals_as_a_new_style_object_with_private_state_keys() {
        let e = SkillEntry {
            type_id: 3300,
            level: 3,
            points: 8000,
            rank: 1,
        };
        let PyValue::ObjectEx {
            form,
            header,
            list,
            dict,
        } = e.to_pyvalue()
        else {
            panic!("a CharacterSkillEntry is an ObjectEx")
        };
        assert_eq!(form, ObjectForm::New, "new-style class: tag 0x23, not 0x22");
        assert!(list.is_empty() && dict.is_empty());
        let PyValue::Tuple(parts) = *header else {
            panic!("header is (classtuple, state)")
        };
        let PyValue::Tuple(cls) = &parts[0] else {
            panic!("the class lives in an inner tuple for the 0x23 form")
        };
        assert_eq!(
            cls[0],
            PyValue::Global(
                b"characterskills.common.character_skill_entry.CharacterSkillEntry".to_vec()
            ),
            "only a globalsWhitelist name survives the client's loader"
        );
        // Read by name through `dict_get`, never by destructuring the pair
        // positionally -- a positional read agrees with the inversion bug
        // instead of catching it.
        let state = &parts[1];
        for key in [
            "_typeID",
            "_trainedSkillLevel",
            "_skillPoints",
            "_skillRank",
            "_virtualSkillLevel",
        ] {
            assert!(
                crate::pyutil::dict_get(state, key.as_bytes()).is_some(),
                "missing state key {key}"
            );
        }
        assert_eq!(
            crate::pyutil::dict_get(state, b"_typeID"),
            Some(&PyValue::Int(3300))
        );
        assert_eq!(
            crate::pyutil::dict_get(state, b"_skillPoints"),
            Some(&PyValue::Int(8000))
        );
        assert_eq!(
            crate::pyutil::dict_get(state, b"skillPoints"),
            None,
            "the public `skillPoints` property raises on access; only the \
             private name belongs in the state"
        );
    }

    // --- skill training --------------------------------------------------

    /// Every attribute at 20.0, which is what `GetAttributes` answers.
    fn twenty(_attribute_id: i64) -> f64 {
        20.0
    }

    const T0: i64 = 134_000_000_000_000_000; // a FILETIME in 2025
    const MINMATAR_FRIGATE: i64 = 3329; // rank 2, perception / willpower

    fn bought(type_id: i64) -> Skills {
        let mut s = Skills::new();
        s.purchase_in(&builtin(), &[type_id])
            .expect("a known skill");
        s
    }

    /// `primary + secondary / 2.0` (`characterskills/util.py:36`), and the
    /// time is `remaining / rate * MIN` (`skill_training.py:140`).
    #[test]
    fn the_rate_and_the_time_are_the_clients_formulas() {
        assert_eq!(sp_per_minute(20.0, 20.0), 30.0);
        assert_eq!(sp_per_minute(27.0, 21.0), 37.5);
        assert_eq!(MIN, 600_000_000);
        // 300 SP at 30 SP a minute is ten minutes.
        assert_eq!(training_ticks(300, 30.0), 10 * MIN);
        assert_eq!(sp_trained(10 * MIN, 30.0), 300);
        assert_eq!(training_ticks(0, 30.0), 0);
    }

    /// The table carries the attribute pair for every skill, and each is one
    /// of the five character attributes (164-168): a 0 here would be the
    /// client's `SkillAttributeMissingError`.
    #[test]
    fn every_skill_names_two_real_character_attributes() {
        for s in SKILLS {
            for a in [s.primary, s.secondary] {
                assert!((164..=168).contains(&a), "{} ({}): {a}", s.name, s.type_id);
            }
            assert_ne!(s.primary, s.secondary, "{}", s.name);
        }
        let c = builtin();
        let mf = find_in(&c, MINMATAR_FRIGATE).expect("in the table");
        assert_eq!((mf.primary, mf.secondary), (167, 168));
    }

    /// Rows chain: entry 1 starts when entry 0 ends, and a second level of
    /// the same skill counts from the first level's floor (`queue.py:21`).
    #[test]
    fn queue_rows_chain_and_count_from_the_right_floor() {
        let skills = bought(MINMATAR_FRIGATE);
        let q = Queue {
            entries: vec![(MINMATAR_FRIGATE, 1), (MINMATAR_FRIGATE, 2)],
            start: Some(T0),
        };
        let rows = q.rows_in(&builtin(), &skills, &twenty);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].start_sp, rows[0].destination_sp), (0, 500));
        assert_eq!(rows[0].start, Some(T0));
        assert_eq!(rows[0].end, Some(T0 + training_ticks(500, 30.0)));
        assert_eq!(rows[1].start, rows[0].end);
        assert_eq!(
            (rows[1].start_sp, rows[1].destination_sp),
            (500, sp_for_level(2, 2))
        );
        assert_eq!(rows[1].position, 1);
    }

    /// Paused means `None` times, not 0: `SkillInTraining` reads a truthy
    /// end time as "training".
    #[test]
    fn a_paused_queue_has_no_times() {
        let skills = bought(MINMATAR_FRIGATE);
        let q = Queue {
            entries: vec![(MINMATAR_FRIGATE, 1)],
            start: None,
        };
        let row = &q.rows_in(&builtin(), &skills, &twenty)[0];
        assert_eq!((row.start, row.end), (None, None));
        let PyValue::Instance { state, .. } = row.to_pyvalue() else {
            panic!("a KeyVal is an old-style instance")
        };
        assert_eq!(
            crate::pyutil::dict_get(&state, b"trainingEndTime"),
            Some(&PyValue::None)
        );
    }

    #[test]
    fn the_row_carries_the_seven_skill_queue_entry_fields() {
        let skills = bought(MINMATAR_FRIGATE);
        let q = Queue {
            entries: vec![(MINMATAR_FRIGATE, 1)],
            start: Some(T0),
        };
        let PyValue::Instance { state, .. } =
            q.rows_in(&builtin(), &skills, &twenty)[0].to_pyvalue()
        else {
            panic!("a KeyVal is an old-style instance")
        };
        for key in [
            "queuePosition",
            "trainingStartTime",
            "trainingEndTime",
            "trainingTypeID",
            "trainingToLevel",
            "trainingStartSP",
            "trainingDestinationSP",
        ] {
            assert!(
                crate::pyutil::dict_get(&state, key.as_bytes()).is_some(),
                "missing {key}"
            );
        }
    }

    #[test]
    fn settle_trains_a_level_when_its_time_comes_and_not_before() {
        let mut skills = bought(MINMATAR_FRIGATE);
        let mut q = Queue {
            entries: vec![(MINMATAR_FRIGATE, 1), (MINMATAR_FRIGATE, 2)],
            start: Some(T0),
        };
        let end = T0 + training_ticks(500, 30.0);
        assert!(
            q.settle_in(&builtin(), &mut skills, end - 1, &twenty)
                .is_empty()
        );
        assert_eq!(skills.get(MINMATAR_FRIGATE).map(|s| s.level), Some(0));

        assert_eq!(
            q.settle_in(&builtin(), &mut skills, end, &twenty),
            vec![MINMATAR_FRIGATE]
        );
        let s = skills.get(MINMATAR_FRIGATE).expect("still there");
        assert_eq!((s.level, s.points), (1, 500));
        assert_eq!(q.entries, vec![(MINMATAR_FRIGATE, 2)]);
        assert_eq!(
            q.start,
            Some(end),
            "the next level starts at the end, not at now"
        );
    }

    /// Offline training: one settle long after both ends credits both.
    #[test]
    fn a_late_settle_loses_no_time() {
        let mut skills = bought(MINMATAR_FRIGATE);
        let mut q = Queue {
            entries: vec![(MINMATAR_FRIGATE, 1), (MINMATAR_FRIGATE, 2)],
            start: Some(T0),
        };
        let trained = q.settle_in(&builtin(), &mut skills, T0 + 1000 * MIN, &twenty);
        assert_eq!(trained, vec![MINMATAR_FRIGATE], "once, though two levels");
        assert_eq!(skills.get(MINMATAR_FRIGATE).map(|s| s.level), Some(2));
        assert!(q.entries.is_empty() && q.start.is_none());
    }

    #[test]
    fn a_pause_credits_what_was_trained_and_stops_the_clock() {
        let mut skills = bought(MINMATAR_FRIGATE);
        let mut q = Queue {
            entries: vec![(MINMATAR_FRIGATE, 1)],
            start: Some(T0),
        };
        assert_eq!(
            q.pause_in(&builtin(), &mut skills, T0 + 5 * MIN, &twenty),
            Some(MINMATAR_FRIGATE)
        );
        assert_eq!(skills.get(MINMATAR_FRIGATE).map(|s| s.points), Some(150));
        assert_eq!(q.start, None);
        assert_eq!(q.entries.len(), 1, "a pause keeps the queue");
        // Resumed, the level needs only what is left.
        q.start = Some(T0 + 100 * MIN);
        assert_eq!(
            q.rows_in(&builtin(), &skills, &twenty)[0].end,
            Some(T0 + 100 * MIN + training_ticks(350, 30.0))
        );
    }

    #[test]
    fn save_refuses_what_the_client_would_have_refused() {
        let mut skills = bought(MINMATAR_FRIGATE);
        let mut q = Queue::default();
        assert_eq!(
            q.save_in(
                &builtin(),
                &mut skills,
                &[(MINMATAR_FRIGATE, 2)],
                true,
                T0,
                &twenty
            ),
            Err(QueueError::OutOfOrder(MINMATAR_FRIGATE, 2))
        );
        assert_eq!(
            q.save_in(&builtin(), &mut skills, &[(3331, 1)], true, T0, &twenty),
            Err(QueueError::NotInjected(3331))
        );
        assert_eq!(
            q.save_in(
                &builtin(),
                &mut skills,
                &[(MINMATAR_FRIGATE, 6)],
                true,
                T0,
                &twenty
            ),
            Err(QueueError::BadLevel(MINMATAR_FRIGATE, 6))
        );
        assert_eq!(q, Queue::default(), "a refused save changes nothing");
    }

    #[test]
    fn save_activates_a_paused_queue_only_when_asked() {
        let mut skills = bought(MINMATAR_FRIGATE);
        let mut q = Queue::default();
        q.save_in(
            &builtin(),
            &mut skills,
            &[(MINMATAR_FRIGATE, 1)],
            false,
            T0,
            &twenty,
        )
        .expect("valid");
        assert!(!q.is_training());
        q.save_in(
            &builtin(),
            &mut skills,
            &[(MINMATAR_FRIGATE, 1)],
            true,
            T0 + MIN,
            &twenty,
        )
        .expect("valid");
        assert_eq!(q.start, Some(T0 + MIN));
    }

    /// Appending behind the skill in training must not restart it: the
    /// client's progress bar counts from `trainingStartTime`.
    #[test]
    fn appending_keeps_the_head_s_start_time() {
        let mut skills = bought(MINMATAR_FRIGATE);
        let mut q = Queue::default();
        q.save_in(
            &builtin(),
            &mut skills,
            &[(MINMATAR_FRIGATE, 1)],
            true,
            T0,
            &twenty,
        )
        .expect("valid");
        let credited = q
            .save_in(
                &builtin(),
                &mut skills,
                &[(MINMATAR_FRIGATE, 1), (MINMATAR_FRIGATE, 2)],
                false,
                T0 + 3 * MIN,
                &twenty,
            )
            .expect("valid");
        assert_eq!(credited, None);
        assert_eq!(q.start, Some(T0));
        assert_eq!(q.entries.len(), 2);
    }

    #[test]
    fn the_queue_survives_the_world_file() {
        let mut qs = Queues::default();
        *qs.entry(90_000_001) = Queue {
            entries: vec![(MINMATAR_FRIGATE, 1), (3300, 3)],
            start: Some(T0),
        };
        *qs.entry(90_000_002) = Queue {
            entries: vec![(3300, 1)],
            start: None,
        };
        qs.entry(90_000_003); // empty: not written
        let mut text = String::new();
        qs.render(&mut text);
        assert_eq!(text.lines().count(), 2);
        let mut back = Queues::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            assert!(Queues::RECORDS.contains(&f[0]));
            back.parse_record(&f).expect("our own line");
        }
        assert_eq!(back.get(90_000_001), qs.get(90_000_001));
        assert_eq!(back.get(90_000_002), qs.get(90_000_002));
        assert!(
            back.parse_record(&["skillqueue", "1", "-1", "3300"])
                .is_err()
        );
    }

    /// `(skillInfos, event, timeStamp)`, `skillsvc.py:186`.
    #[test]
    fn the_completion_push_is_on_server_skills_changed() {
        use crate::notify::Push as _;
        let p = SkillsChanged {
            character_id: 7,
            entries: vec![SkillEntry {
                type_id: MINMATAR_FRIGATE,
                level: 1,
                points: 500,
                rank: 2,
            }],
            time_stamp: T0,
        };
        let (event, args) = p.to_push();
        assert_eq!(event, "OnServerSkillsChanged");
        assert_eq!(args.len(), 3);
        let PyValue::Dict(infos) = &args[0] else {
            panic!("skillInfos is a dict")
        };
        assert_eq!(infos[0].1, PyValue::Int(MINMATAR_FRIGATE), "(value, key)");
        assert_eq!(args[1], PyValue::None);
        assert_eq!(args[2], PyValue::Int(T0));
        assert_eq!(p.audience(), Some(vec![7]));
    }

    // --- the loaded table (W9 L3) -------------------------------------------

    /// Two rows in `tools/skills.py`'s format. The numbers are the format's,
    /// not claims about the game.
    const TSV: &str = "# comment\n\
        skill\t9001\t255\t3\t165\t166\t0.01\t\tFirst Test Skill\n\
        skill\t9002\t257\t2\t167\t168\t0.01\t9001=4;3300=1\tSecond Test Skill\n";

    #[test]
    fn the_table_parses_its_own_format() {
        let t = Table::parse(TSV).expect("parses");
        let s = t.skills.iter().find(|s| s.type_id == 9002).expect("row");
        assert_eq!(
            (s.name, s.group_id, s.rank, s.primary, s.secondary),
            ("Second Test Skill", 257, 2, 167, 168)
        );
        assert_eq!(t.required.get(&9002), Some(&vec![(9001, 4), (3300, 1)]));
        assert!(!t.required.contains_key(&9001));
        let book = t.items.iter().find(|i| i.type_id == 9001).expect("a book");
        assert_eq!(
            (book.category_id, book.group_id, book.singleton),
            (CATEGORY_SKILL, 255, false)
        );
    }

    /// A skill only the hand-copied rows have stays known with a table loaded.
    #[test]
    fn a_loaded_table_keeps_every_hand_copied_skill() {
        let t = Table::parse(TSV).expect("parses");
        for s in SKILLS {
            assert!(
                t.skills.iter().any(|l| l.type_id == s.type_id),
                "{}",
                s.name
            );
            assert!(t.items.iter().any(|i| i.type_id == s.type_id), "{}", s.name);
        }
        assert_eq!(t.counts(), (SKILLS.len() + 2, 1));
    }

    #[test]
    fn a_bad_line_is_refused_with_its_number() {
        let rank0 = "skill\t9001\t255\t0\t165\t166\t0.01\t\tZero\n";
        assert!(Table::parse(rank0).unwrap_err().starts_with("line 1"));
        let short = "# x\nskill\t9001\t255\n";
        assert!(Table::parse(short).unwrap_err().starts_with("line 2"));
        let req = "skill\t9001\t255\t1\t165\t166\t0.01\t3300\tBad\n";
        assert!(Table::parse(req).is_err());
    }

    /// Without a table every hand-copied skill is still a book the item store
    /// can hold.
    #[test]
    fn every_known_skill_has_a_skillbook_item_type() {
        let c = builtin();
        for s in all_in(&c) {
            let t = item_types_in(&c)
                .iter()
                .find(|t| t.type_id == s.type_id)
                .expect("a book");
            assert_eq!(t.category_id, CATEGORY_SKILL);
            assert_eq!(t.group_id, s.group_id);
        }
    }

    /// `CheckCanInsertSkillAtPosition`'s prerequisite rule, with Minmatar
    /// Frigate's own requirement from `requiredskillsfortypes` (Spaceship
    /// Command 3327 at 1) **[V]**.
    #[test]
    fn a_prerequisite_is_met_by_training_or_by_queueing_it_first() {
        const SPACESHIP_COMMAND: i64 = 3327;
        let req = [(SPACESHIP_COMMAND, 1)];
        let mut s = Skills::new();
        s.purchase_in(&builtin(), &[MINMATAR_FRIGATE, SPACESHIP_COMMAND])
            .expect("bought");
        // Neither trained nor queued.
        assert_eq!(
            unmet_requirement(&s, &[], MINMATAR_FRIGATE, &req),
            Some(SPACESHIP_COMMAND)
        );
        // Queued ahead of it.
        assert_eq!(
            unmet_requirement(&s, &[(SPACESHIP_COMMAND, 1)], MINMATAR_FRIGATE, &req),
            None
        );
        // Trained.
        s.grant_in(&builtin(), SPACESHIP_COMMAND, 1);
        assert_eq!(unmet_requirement(&s, &[], MINMATAR_FRIGATE, &req), None);
        // A skill with points is never re-judged (`:313`).
        let mut t = Skills::new();
        t.purchase_in(&builtin(), &[MINMATAR_FRIGATE])
            .expect("bought");
        t.get_mut(MINMATAR_FRIGATE).expect("there").points = 10;
        assert_eq!(unmet_requirement(&t, &[], MINMATAR_FRIGATE, &req), None);
    }

    /// CP59a: the fallback table's own books: one per [`SKILLS`] row, in
    /// order, each the row's book at [`SKILLBOOK_VOLUME`].
    #[test]
    fn the_fallback_items_come_from_the_same_table() {
        let t = Table::fallback();
        assert_eq!(t.items.len(), SKILLS.len());
        assert_eq!(t.skills.as_slice(), SKILLS);
        for (book, s) in t.items.iter().zip(&t.skills) {
            assert_eq!(*book, skillbook(s, SKILLBOOK_VOLUME));
            assert_eq!(book.category_id, CATEGORY_SKILL);
        }
        assert!(t.required.is_empty());
    }

    /// The invented rows answer through the accessors, prerequisites and
    /// books included; the built-in rows ride along (as with any parse).
    #[test]
    fn the_fixture_answers_its_invented_rows() {
        let mut c = Content::default();
        c.insert(TABLE.name, fixture());
        let three = find_in(&c, 990_020_003).expect("an invented row");
        assert_eq!(three.name, "Fixture Skill Three");
        assert_eq!(
            required_in(&c, 990_020_003),
            &[(990_020_001, 2), (990_020_002, 1)]
        );
        assert_eq!(required_in(&c, 990_020_001), &[] as &[(i64, i64)]);
        assert_eq!(
            find_any_in(&c, "fixture skill two").map(|s| s.type_id),
            Some(990_020_002)
        );
        let book = item_types_in(&c)
            .iter()
            .find(|i| i.type_id == 990_020_003)
            .expect("its book");
        assert_eq!(book.volume, 0.02);
        assert_eq!(book.category_id, CATEGORY_SKILL);
        assert!(SKILLS.iter().all(|s| find_in(&c, s.type_id) == Some(s)));
        assert_eq!(item_types_in(&c).len(), all_in(&c).count());
    }

    /// Without a bundle the table is the fixture; the registered spec's
    /// fixture is the same table.
    #[test]
    fn without_a_bundle_the_content_table_is_the_fixture() {
        let dir = crate::authored::Dir::compiled_only();
        let src = Sources::new(None, &dir);
        let loaded = (TABLE.load)(&src).expect("loads");
        let loaded = loaded.downcast_ref::<Table>().expect("a Table");
        let fixed = (TABLE.fixture)();
        let fixed = fixed.downcast_ref::<Table>().expect("a Table");
        assert_eq!(loaded, fixed);
        assert_eq!(loaded, &fixture());
    }

    /// The registration line (`content_tables::TABLES`) puts it in every
    /// test registry's `Content`.
    #[test]
    fn the_fixture_content_carries_the_skill_table() {
        let c = Content::fixture();
        assert_eq!(table_in(&c), &fixture());
    }

    #[test]
    fn the_in_rules_read_the_content_table_not_the_builtin_one() {
        // A rule that ignored `content` would refuse the fixture's invented
        // skill, which only the content table has.
        let c = Content::fixture();
        let (one, three) = (990_020_001, 990_020_003);
        let mut s = Skills::new();
        assert_eq!(s.purchase_in(&c, &[three]), Ok(vec![three]));
        assert_eq!(
            s.get(three).map(|e| e.rank),
            find_in(&c, three).map(|t| t.rank)
        );
        assert!(rate_for_in(&c, three, &twenty).is_some());
        let mut q = Queue::default();
        // The fixture's third skill needs the first at level 2.
        assert_eq!(
            q.save_in(&c, &mut s, &[(three, 1)], true, T0, &twenty),
            Err(QueueError::BeforeRequirements(three, one))
        );
        assert_eq!(s.grant_in(&c, one, 2).map(|e| e.level), Some(2));
        s.grant_in(&c, 990_020_002, 1);
        assert_eq!(
            q.save_in(&c, &mut s, &[(three, 1)], true, T0, &twenty),
            Ok(None)
        );
        assert_eq!(q.rows_in(&c, &s, &twenty).len(), 1);
        assert!(!q.prune_in(&c, &s), "a content skill is not pruned");
    }

    #[test]
    fn the_skills_flag_replaces_the_content_table() {
        let dir = std::env::temp_dir().join(format!("eve-skill-cp59b-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.tsv");
        std::fs::write(&good, FIXTURE).unwrap();
        let mut c = Content::default();
        c.insert(TABLE.name, Table::fallback());
        assert!(find_in(&c, 990_020_001).is_none());
        let line = override_in(&mut c, &good).expect("overrides");
        assert!(line.contains(&good.display().to_string()), "{line}");
        assert_eq!(table_in(&c), &fixture());
        let bad = dir.join("bad.tsv");
        std::fs::write(&bad, "type\t1\n").unwrap();
        let e = override_in(&mut c, &bad).expect_err("refused");
        assert!(e.starts_with("--skills ") && e.contains("bad.tsv"), "{e}");
        assert_eq!(table_in(&c), &fixture(), "a refused file changes nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bundle_file_parses_and_a_bad_one_is_an_error_naming_it() {
        let dir = std::env::temp_dir().join(format!("eve-skill-cp59a-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.tsv");
        std::fs::write(&good, FIXTURE).unwrap();
        assert_eq!(
            from_file(&good).expect("the fixture file parses"),
            fixture()
        );
        let builtin = dir.join("builtin.tsv");
        let s = &SKILLS[0];
        std::fs::write(
            &builtin,
            format!(
                "skill\t{}\t{}\t{}\t{}\t{}\t0.01\t\t{}\n",
                s.type_id, s.group_id, s.rank, s.primary, s.secondary, s.name
            ),
        )
        .unwrap();
        let rank0 = dir.join("rank0.tsv");
        std::fs::write(
            &rank0,
            "skill\t990020009\t990020100\t0\t165\t166\t0.01\t\tX\n",
        )
        .unwrap();
        let bad = dir.join("bad.tsv");
        std::fs::write(&bad, "type\t1\n").unwrap();
        for p in [&builtin, &rank0, &bad, &dir.join("absent.tsv")] {
            let e = from_file(p).expect_err("refused");
            assert!(e.contains(&p.display().to_string()), "{e}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
