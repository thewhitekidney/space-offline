//! The NPC data loader: every `npc/` bundle file the server reads, as one
//! [`Table`], plus the pure lookups over it (C6 R1-R3).
//!
//! W12 packet CP20 (`docs/content/plan.md` §6.2), spec C6 R1-R3, §5.1 and
//! packet N2; headers frozen in `docs/content/schema.md` (X1). Class B data
//! (client facts): with no bundle the table is [`Table::fixture`], whose rows
//! are invented. Each file is optional (an absent file is an empty map); a
//! file that is there with the wrong header or a bad row is an error naming
//! it, which `content::load` turns into a startup error.
//!
//! What this module answers:
//! - [`Table::pirate`] (R3): the pirate faction whose rats a system holds,
//!   by the system's `SystemRow.region_id` (CA9) and `npc/regions.tsv`;
//! - [`Table::home_pirate`]: an offering faction's home pirate
//!   (`npc/home-pirate.tsv`), for CP19 and CP48;
//! - [`Table::profile`] (R2, R8): what a type fights with, read from its own
//!   `typedata.tsv` attributes and effects, plus `npc/types.tsv`'s names;
//! - the raw rows (factions, corporations, bounties, loot pools) for CP22.
//!
//! The `NpcType` an NPC flies is still built in `npc.rs`
//! (`npc::derive_loaded` from these rows; `npc::derive` for the hand
//! fallback), because typedata is a process static installed after
//! `content::load` (CA7: CP58a retires it).

use crate::content::{Boxed, Bundle, Sources, TableSpec};
use std::collections::{BTreeMap, BTreeSet};

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "npc_data",
    load: load_table,
    fixture: || Box::new(Table::fixture()) as Boxed,
};

/// The bundle's rows, or the fixture with no bundle (client data, class B).
fn load_table(src: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => crate::wormhole_sites::with_sleepers(
            Table::from_bundle(b)?,
            &crate::wormhole_sites::sleeper_rows(src)?,
            crate::wormhole_sites::site_ids(src)?,
        ),
        None => Table::fixture(),
    }))
}

/// The bundle files and their header lines, copied from `schema.md` (X1).
pub const FACTIONS: (&str, &str) = (
    "npc/factions.tsv",
    "#faction_id\tcorporation_id\tmilitia_corporation_id\tsystem_id\tmember_races\towner_corp_id\tgroups\twrecks\tdamage_profile\tammo_groups\ttank\ttags\tbase_prefix\telite_prefix\tcommander_prefix\tdeadspace_stems\tofficers\tsof_faction\tname",
);
pub const CORPORATIONS: (&str, &str) = (
    "npc/corporations.tsv",
    "#corp_id\tfaction_id\tstation_id\tsystem_id\tfriend_id\tenemy_id\ttax_rate\tmin_security\tmain_activity_id\tsize\textent\tlp_offer_tables\tticker\tname",
);
pub const TYPES: (&str, &str) = (
    "npc/types.tsv",
    "#type_id\tgroup_id\tfaction_id\tfaction_src\tcontext\tclass\ttier\talias_of\tneeds\trole",
);
pub const COMPONENTS: (&str, &str) = ("npc/components.tsv", "#type_id\tcomponent");
pub const REACTIVE_SPAWN: (&str, &str) = (
    "npc/reactive-spawn.tsv",
    "#type_id\tscope\tkey\tspawn_table_id\tinitial_fleet_type_id\towner_id\tfleet_counter_table_id\tbounty_payout_multiplier\tfriendly_response_threshold",
);
pub const BOUNTY: (&str, &str) = ("npc/bounty.tsv", "#type_id\tbounty\tmultipliers");
pub const REGIONS: (&str, &str) = ("npc/regions.tsv", "#region_id\tfaction_id\tweight\trule");
pub const HOME_PIRATE: (&str, &str) = ("npc/home-pirate.tsv", "#faction_id\tpirate_faction_id");
pub const LOOT: (&str, &str) = (
    "npc/loot.tsv",
    "#faction_id\tsize\tkind\ttier\ttype_id\tmeta_level",
);
/// NW-7: each police force, one row per faction; written by `tools/content_police.py`.
pub const POLICE: (&str, &str) = ("npc/police.tsv", "#faction_id\twarning_key\ttype_ids");

/// The role of each CONCORD ship type (`concord::kind_for`); written by
/// `tools/content_police.py`.
pub const CONCORD_ROLES: (&str, &str) = ("npc/concord.tsv", "#role\ttype_id");

/// The type group of the sentry guns (`groupSentryGun`), a protocol constant.
pub const SENTRY_GROUP: i64 = 99;

/// One `npc/factions.tsv` row (C6 R1). `sof_faction` is checked for shape and
/// not kept: nothing reads it. `member_races` is kept for NW-8's sentry type.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Faction {
    pub faction_id: i64,
    pub corporation_id: i64,
    pub militia_corporation_id: Option<i64>,
    /// The HQ system (R3 step 0, applied by the extractor).
    pub system_id: i64,
    /// The race ids of the faction's members (`member_races`), in file order.
    pub member_races: Vec<i64>,
    /// R1's owner of the faction's roaming spawner; pirate rows only.
    pub owner_corp_id: Option<i64>,
    pub groups: Vec<i64>,
    /// `class -> wreck type`, the modal `types.wreckTypeID` per class.
    pub wrecks: BTreeMap<String, i64>,
    /// em, explosive, kinetic, thermal.
    pub damage_profile: Vec<f64>,
    /// `size -> charge group`.
    pub ammo_groups: BTreeMap<i64, i64>,
    pub tank: Option<String>,
    pub tags: Vec<i64>,
    pub base_prefix: Option<String>,
    pub elite_prefix: Option<String>,
    pub commander_prefix: Option<String>,
    pub deadspace_stems: Vec<String>,
    pub officers: Vec<String>,
    pub name: String,
}

impl Faction {
    /// Who owns the faction's ships on the wire: R1's owner where the
    /// extractor found one, else the executor corporation.
    pub fn owner(&self) -> i64 {
        self.owner_corp_id.unwrap_or(self.corporation_id)
    }
}

/// One `npc/corporations.tsv` row, the columns a server reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Corporation {
    pub corp_id: i64,
    pub faction_id: Option<i64>,
    pub station_id: Option<i64>,
    pub system_id: Option<i64>,
    pub ticker: String,
    pub name: String,
}

/// C6 R2's prefix rung.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Base,
    Elite,
    Commander,
    Officer,
    Deadspace,
}

impl Tier {
    pub(crate) fn parse(s: &str) -> Result<Tier, String> {
        Ok(match s {
            "base" => Tier::Base,
            "elite" => Tier::Elite,
            "commander" => Tier::Commander,
            "officer" => Tier::Officer,
            "deadspace" => Tier::Deadspace,
            _ => return Err(format!("bad tier {s:?}")),
        })
    }
}

/// C6 R8's role, CCP's vocabulary, first match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Logistic,
    Tackler,
    Ewar,
    Dps,
}

impl Role {
    pub(crate) fn parse(s: &str) -> Result<Role, String> {
        Ok(match s {
            "LOGISTIC" => Role::Logistic,
            "TACKLER" => Role::Tackler,
            "EWAR" => Role::Ewar,
            "DPS" => Role::Dps,
            _ => return Err(format!("bad role {s:?}")),
        })
    }
}

/// C6 R2's capability set: what a type needs the server to implement before
/// a spawner may draw it (`needs ⊆ SUPPORTED`, CP22).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Needs(pub u8);

impl Needs {
    pub const TURRET: Needs = Needs(1);
    pub const MISSILE: Needs = Needs(1 << 1);
    pub const SCRAM: Needs = Needs(1 << 2);
    pub const WEB: Needs = Needs(1 << 3);
    pub const EWAR: Needs = Needs(1 << 4);
    pub const NEUT: Needs = Needs(1 << 5);
    pub const SELF_REP: Needs = Needs(1 << 6);
    pub const REMOTE_REP: Needs = Needs(1 << 7);
    const NAMES: [(&'static str, Needs); 8] = [
        ("TURRET", Needs::TURRET),
        ("MISSILE", Needs::MISSILE),
        ("SCRAM", Needs::SCRAM),
        ("WEB", Needs::WEB),
        ("EWAR", Needs::EWAR),
        ("NEUT", Needs::NEUT),
        ("SELF_REP", Needs::SELF_REP),
        ("REMOTE_REP", Needs::REMOTE_REP),
    ];

    /// Every bit of `other` is set here.
    pub fn contains(self, other: Needs) -> bool {
        self.0 & other.0 == other.0
    }

    /// Both sets.
    pub fn union(self, other: Needs) -> Needs {
        Needs(self.0 | other.0)
    }

    /// `TURRET,SCRAM`; an unknown name is an error.
    pub(crate) fn parse(s: &str) -> Result<Needs, String> {
        list(s).try_fold(Needs::default(), |acc, n| {
            Self::NAMES
                .iter()
                .find(|(name, _)| *name == n)
                .map(|(_, b)| acc.union(*b))
                .ok_or_else(|| format!("bad need {n:?}"))
        })
    }
}

/// One `npc/types.tsv` row.
#[derive(Debug, Clone, PartialEq)]
pub struct NpcTypeRow {
    pub type_id: i64,
    pub group_id: i64,
    pub faction_id: i64,
    /// `Asteroid`, `Deadspace`, ... (C6 §2.1).
    pub context: String,
    /// `Frigate`, `Commander Frigate`, ... (verbatim); the key of
    /// [`Faction::wrecks`].
    pub class: String,
    pub tier: Tier,
    pub alias_of: Option<i64>,
    pub needs: Needs,
    pub role: Role,
}

/// One `npc/reactive-spawn.tsv` row. Loaded, not yet acted on (C3 §8.3: belt
/// rats are not reactive spawns).
#[derive(Debug, Clone, PartialEq)]
pub struct ReactiveSpawn {
    pub type_id: i64,
    pub scope: String,
    pub key: Option<i64>,
    pub spawn_table_id: Option<i64>,
    pub owner_id: Option<i64>,
}

/// One `npc/bounty.tsv` row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bounty {
    /// Attribute 481 `entityKillBounty`.
    pub bounty: f64,
    /// `owner -> multiplier`.
    pub multipliers: BTreeMap<i64, f64>,
}

/// Which C6 R3 step produced a `npc/regions.tsv` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    /// The override file (`content/npc/region-pirates.tsv`).
    Override,
    /// Steps 0-4, first match.
    Step(u8),
    /// A region one hop from a pirate's HQ or sov region.
    Secondary,
}

impl Rule {
    fn parse(s: &str) -> Result<Rule, String> {
        Ok(match s {
            "override" => Rule::Override,
            "secondary" => Rule::Secondary,
            "0" | "1" | "2" | "3" | "4" => Rule::Step(int(s)? as u8),
            _ => return Err(format!("bad rule {s:?}")),
        })
    }
}

/// One `npc/regions.tsv` row, under its region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionPirate {
    pub faction_id: i64,
    pub weight: f64,
    pub rule: Rule,
}

/// One `npc/loot.tsv` row (C6 R7's pools).
#[derive(Debug, Clone, PartialEq)]
pub struct Loot {
    pub faction_id: i64,
    pub size: i64,
    pub kind: String,
    pub tier: Tier,
    pub type_id: i64,
    pub meta_level: Option<i64>,
}

/// One `npc/police.tsv` row: the faction's chat warning and its group-182
/// types, the tackling lead found by effects (`concord::police_kinds`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Police {
    pub faction_id: i64,
    pub warning: String,
    pub types: Vec<i64>,
}

/// Every `npc/` row the server reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub factions: BTreeMap<i64, Faction>,
    pub corporations: BTreeMap<i64, Corporation>,
    pub types: BTreeMap<i64, NpcTypeRow>,
    /// `(type_id, component)` presence.
    pub components: BTreeSet<(i64, String)>,
    pub reactive: Vec<ReactiveSpawn>,
    pub bounty: BTreeMap<i64, Bounty>,
    /// `region -> its pirates`, in file order.
    pub regions: BTreeMap<i64, Vec<RegionPirate>>,
    /// `offering faction -> home pirate`.
    pub home_pirate: BTreeMap<i64, i64>,
    pub loot: Vec<Loot>,
    /// NW-7: `faction -> its police` (`npc/police.tsv`); the row of the CONCORD
    /// faction is the default force.
    pub police: BTreeMap<i64, Police>,
    /// `role -> CONCORD type` (`npc/concord.tsv`); a role the file lacks is
    /// flown as an invented fixture.
    pub concord_roles: BTreeMap<String, i64>,
    /// Y-WH: the J-space dungeons whose squads are Sleepers
    /// (`wormhole_sites::squads`), because a Sleeper pays no bounty and
    /// `npc_gen`'s budgeted fill cannot draw one. Empty without a bundle.
    pub sleeper_dungeons: BTreeSet<i64>,
    /// NW-8: the group-99 sentry gun types as `(race_id, type_id)`, ascending,
    /// read from `types/market.tsv` (rows the market does not list included).
    /// Empty without a bundle.
    pub sentry_types: Vec<(i64, i64)>,
}

/// What a type fights with, read from its own attributes and effects
/// (C6 R2, R8). Every attribute and effect id is C6 R2's or §8's **[V]**;
/// what each number *means* to a brain is **[I]**, as C6 marks it.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub type_id: i64,
    pub faction_id: i64,
    pub tier: Tier,
    pub role: Role,
    /// `npc/types.tsv`'s `needs`, plus what the type's own dogma shows.
    pub needs: Needs,
    pub missile: Option<Missile>,
    pub scram: Option<Scram>,
    /// Attribute 512, the web's chance; `None` at zero.
    pub web_chance: Option<f64>,
    pub self_rep: Option<SelfRep>,
    /// Attributes 475/476: the first-shot window, ms.
    pub attack_delay_ms: (f64, f64),
    /// Attribute 665: the leash, metres (0 = none).
    pub chase_max: f64,
    /// Attribute 1648 is 1.0: the type switches targets.
    pub target_switch: bool,
    /// Attribute 1416: the switch timer, ms.
    pub switch_timer_ms: f64,
    /// Attribute 1651 `AI_ChanceToNotTargetSwitch`: the chance a due switch
    /// is skipped, `[0, 1]` (K-RATAI). Zero on all but three types.
    pub switch_skip_chance: f64,
    /// A remote repairer on the hull (K-RATAI): effect 3852 or 3855 with
    /// its own attributes, or the behaviour pair 6741/6742.
    pub remote_rep: Option<RemoteRep>,
    /// A web of the `behavior*` family, effect 6743 (K-RATAI).
    pub web_behaviour: Option<WebBehaviour>,
    /// An energy neutralizer on the hull: effect 6691 or 6756 (NEUT-NOS).
    pub neut: Option<Neut>,
    /// Attribute 1650 is 1.0: prefers targets by signature.
    pub prefer_signature: bool,
    /// `npc/bounty.tsv`, else attribute 481.
    pub bounty: f64,
}

/// A missile launcher on the hull.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Missile {
    /// Attribute 507, the missile type.
    pub type_id: i64,
    /// Attribute 212.
    pub multiplier: f64,
    /// Attribute 506.
    pub period_ms: f64,
}

/// A warp scrambler on the hull: effect 563, 5928 (the entity scrambler that
/// also blocks a microwarpdrive, K-RATAI) or 6745 (`behaviorWarpScramble`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scram {
    /// Attributes 103, 105, 504 (563, 5928) or 2507, 2509 and no chance
    /// (6745: `1.0`, **[A]**, see [`dogma::BEHAVIOUR_CHANCE`]).
    pub range: f64,
    pub strength: f64,
    pub chance: f64,
    /// The cycle in ms: 505, or 2506 for 6745.
    pub duration_ms: f64,
    /// Effect 5928 or 6745 also blocks a microwarpdrive (R-TACKLE §4.1).
    /// Read, not yet acted on: the activation guard is another lane's.
    pub blocks_mwd: bool,
}

/// A web of effect 6743 (`npcBehaviorWebifier`): attributes 2500 range, 2499
/// cycle and 20 `speedFactor`; it has no chance attribute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WebBehaviour {
    pub range: f64,
    pub strength: f64,
    pub duration_ms: f64,
}

/// An energy neutralizer on the hull (NEUT-NOS): effect 6691
/// (`entityEnergyNeutralizerFalloff`: 942 cycle, 98 range, 97 amount, 931
/// chance) or 6756 (`npcBehaviorEnergyNeutralizer`: 2519 cycle, 2520 range,
/// 2521 falloff, 97 amount, no chance attribute). The effect rows' own
/// duration, range and falloff attributes name these [V, typedata];
/// that 6756 drains attribute 97 is **[A]** (the behaviour effects have no
/// modifier rows and 2522 is its `dischargeAttributeID`, the capacitor
/// need). Falsified by a 6756 rat whose neut drains nothing like its 97.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Neut {
    /// Metres, optimal.
    pub range: f64,
    /// Metres of falloff past the range (6756 only; 0 is a hard edge).
    pub falloff: f64,
    /// GJ drained per cycle at full weight.
    pub amount: f64,
    pub duration_ms: f64,
    /// The chance a cycle runs, `[0, 1]`.
    pub chance: f64,
}

/// A remote repairer on a hull, in the server's units (K-RATAI).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RemoteRep {
    pub armor: bool,
    /// Hit points per cycle.
    pub amount: f64,
    pub period_ms: f64,
    /// The chance a cycle runs, `[0, 1]`.
    pub chance: f64,
    /// An ally is repaired while its layer's fraction is below this.
    pub threshold: f64,
    pub range: f64,
    pub max_targets: usize,
}

/// A self-repairer on the hull.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelfRep {
    pub armor: bool,
    /// Shield 637/636/639, armor 631/630/638.
    pub amount: f64,
    pub period_ms: f64,
    pub delay_chance: f64,
}

/// The dogma ids [`Table::profile`] reads (C6 R2, R8, §8) **[V]**.
pub mod dogma {
    pub const MISSILE_TYPE: i64 = 507;
    pub const MISSILE_MULTIPLIER: i64 = 212;
    pub const MISSILE_PERIOD: i64 = 506;
    pub const SCRAM_RANGE: i64 = 103;
    pub const SCRAM_STRENGTH: i64 = 105;
    pub const SCRAM_CHANCE: i64 = 504;
    pub const WEB_CHANCE: i64 = 512;
    pub const SHIELD_REP_AMOUNT: i64 = 637;
    pub const SHIELD_REP_PERIOD: i64 = 636;
    pub const SHIELD_REP_DELAY_CHANCE: i64 = 639;
    pub const ARMOR_REP_AMOUNT: i64 = 631;
    pub const ARMOR_REP_PERIOD: i64 = 630;
    pub const ARMOR_REP_DELAY_CHANCE: i64 = 638;
    pub const ATTACK_DELAY_MIN: i64 = 475;
    pub const ATTACK_DELAY_MAX: i64 = 476;
    pub const CHASE_MAX: i64 = 665;
    pub const TARGET_SWITCH: i64 = 1648;
    pub const SWITCH_TIMER: i64 = 1416;
    pub const PREFER_SIGNATURE: i64 = 1650;
    pub const BOUNTY: i64 = 481;
    /// The hull damage attributes (em, explosive, kinetic, thermal): a
    /// turret entity carries them on itself.
    pub const DAMAGE: [i64; 4] = [114, 116, 117, 118];
    /// Effect `warpScrambleForEntity` (C6 §8).
    pub const EFFECT_SCRAM: i64 = 563;
    /// `warpScrambleTargetMWDBlockActivationForEntity` [V-data].
    pub const EFFECT_SCRAM_MWD: i64 = 5928;
    /// `behaviorWarpScramble`: 2506 cycle, 2507 range, 2509 strength.
    pub const EFFECT_BEHAVIOUR_SCRAM: i64 = 6745;
    pub const BEHAVIOUR_SCRAM_DURATION: i64 = 2506;
    pub const BEHAVIOUR_SCRAM_RANGE: i64 = 2507;
    pub const BEHAVIOUR_SCRAM_STRENGTH: i64 = 2509;
    pub const SCRAM_DURATION: i64 = 505;
    /// `npcBehaviorWebifier`: 2499 cycle, 2500 range, 20 speedFactor.
    pub const EFFECT_BEHAVIOUR_WEB: i64 = 6743;
    pub const BEHAVIOUR_WEB_DURATION: i64 = 2499;
    pub const BEHAVIOUR_WEB_RANGE: i64 = 2500;
    pub const SPEED_FACTOR: i64 = 20;
    /// `entityEnergyNeutralizerFalloff` (NEUT-NOS): 942 cycle, 98 range,
    /// 931 chance; the amount is `energyNeutralizerAmount` (97).
    pub const EFFECT_NEUT: i64 = 6691;
    pub const NEUT_DURATION: i64 = 942;
    pub const NEUT_RANGE: i64 = 98;
    pub const NEUT_CHANCE: i64 = 931;
    pub const NEUT_AMOUNT: i64 = 97;
    /// `npcBehaviorEnergyNeutralizer`: 2519 cycle, 2520 range, 2521 falloff.
    pub const EFFECT_BEHAVIOUR_NEUT: i64 = 6756;
    pub const BEHAVIOUR_NEUT_DURATION: i64 = 2519;
    pub const BEHAVIOUR_NEUT_RANGE: i64 = 2520;
    pub const BEHAVIOUR_NEUT_FALLOFF: i64 = 2521;
    /// The effects whose own `dischargeAttributeID` is a capacitor need the
    /// NPC pays per cycle (NEUT-NOS): its repairers, tackle and neut.
    pub const CAP_SHIELD_REP: [i64; 6] = [2192, 2193, 2194, 876, 5371, 6990];
    pub const CAP_ARMOR_REP: [i64; 6] = [2195, 2196, 2197, 878, 5370, 6884];
    pub const CAP_ASSIST_SHIELD: [i64; 2] = [3855, 6742];
    pub const CAP_ASSIST_ARMOR: [i64; 2] = [3852, 6741];
    pub const CAP_SCRAM: [i64; 3] = [563, 5928, 6745];
    pub const CAP_WEB: [i64; 2] = [575, 6743];
    pub const CAP_NEUT: [i64; 2] = [6691, 6756];
    /// **[A]** The chance of a cycle of a `behavior*` effect, which has no
    /// chance attribute: the client's `ActivateCombatEffectsOnTarget` takes
    /// none (`docs/re/npc-behaviors.md` §2.8). Falsified by a capture of a
    /// behaviour tackler that skips cycles.
    pub const BEHAVIOUR_CHANCE: f64 = 1.0;
    /// `AI_ChanceToNotTargetSwitch`.
    pub const SWITCH_SKIP_CHANCE: i64 = 1651;
    /// `npcRemoteArmorRepair*` (effect 3852): chance, cycle, amount,
    /// threshold, max targets; `npcAssistanceRange` is shared.
    pub const EFFECT_REMOTE_ARMOR: i64 = 3852;
    pub const REMOTE_ARMOR_CHANCE: i64 = 1453;
    pub const REMOTE_ARMOR_DURATION: i64 = 1454;
    pub const REMOTE_ARMOR_AMOUNT: i64 = 1455;
    pub const REMOTE_ARMOR_THRESHOLD: i64 = 1456;
    pub const REMOTE_ARMOR_MAX_TARGETS: i64 = 1501;
    /// `npcRemoteShieldBoost*` (effect 3855).
    pub const EFFECT_REMOTE_SHIELD: i64 = 3855;
    pub const REMOTE_SHIELD_DURATION: i64 = 1458;
    pub const REMOTE_SHIELD_CHANCE: i64 = 1459;
    pub const REMOTE_SHIELD_AMOUNT: i64 = 1460;
    pub const REMOTE_SHIELD_THRESHOLD: i64 = 1462;
    pub const REMOTE_SHIELD_MAX_TARGETS: i64 = 1502;
    pub const ASSISTANCE_RANGE: i64 = 1464;
    /// `behaviorRemoteArmorRepairer` (6741): cycle 2491, range 2492, the
    /// amount `armorDamageAmount` (84).
    pub const EFFECT_BEHAVIOUR_ARMOR: i64 = 6741;
    pub const BEHAVIOUR_ARMOR_DURATION: i64 = 2491;
    pub const BEHAVIOUR_ARMOR_RANGE: i64 = 2492;
    pub const ARMOR_DAMAGE_AMOUNT: i64 = 84;
    /// `behaviorRemoteShieldBooster` (6742): cycle 2495, range 2496, the
    /// amount `shieldBonus` (68).
    pub const EFFECT_BEHAVIOUR_SHIELD: i64 = 6742;
    pub const BEHAVIOUR_SHIELD_DURATION: i64 = 2495;
    pub const BEHAVIOUR_SHIELD_RANGE: i64 = 2496;
    pub const SHIELD_BONUS: i64 = 68;
    /// **[A]** The threshold of a `behavior*` remote repairer, which has
    /// none: repair any ally that is damaged at all. Falsified by a capture
    /// of a logistics NPC that waits for a deeper wound.
    pub const BEHAVIOUR_THRESHOLD: f64 = 1.0;
    /// C6 R8's remote-repair effects.
    pub const EFFECTS_REMOTE_REP: [i64; 6] = [6741, 6742, 3852, 3855, 6165, 6687];
}

impl Table {
    /// Every `npc/` file under `b`; an absent file is empty.
    pub fn from_bundle(b: &Bundle) -> Result<Table, String> {
        Table::from_files(|rel| match std::fs::read_to_string(b.dir.join(rel)) {
            Ok(t) => Ok(Some(t)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{rel}: {e}")),
        })
    }

    /// The same over any reader of `rel -> text`, for tests.
    pub fn from_files(
        read: impl Fn(&str) -> Result<Option<String>, String>,
    ) -> Result<Table, String> {
        let mut t = Table::default();
        let file = |(rel, header): (&str, &str)| -> Result<String, String> {
            let text = read(rel)?.unwrap_or_else(|| format!("{header}\n"));
            check_header(rel, &text, header)?;
            Ok(text)
        };
        for f in rows(FACTIONS, &file(FACTIONS)?)? {
            let f = faction(&f)?;
            t.factions.insert(f.faction_id, f);
        }
        if let Some(text) = read(crate::econ_exchange::MARKET.0)? {
            check_header(
                crate::econ_exchange::MARKET.0,
                &text,
                crate::econ_exchange::MARKET.1,
            )?;
            for f in rows(crate::econ_exchange::MARKET, &text)? {
                if int(f[1])? == SENTRY_GROUP
                    && let Some(race) = opt(f[10])?
                {
                    t.sentry_types.push((race, int(f[0])?));
                }
            }
            t.sentry_types.sort_unstable();
        }
        for f in rows(CORPORATIONS, &file(CORPORATIONS)?)? {
            let c = Corporation {
                corp_id: int(f[0])?,
                faction_id: opt(f[1])?,
                station_id: opt(f[2])?,
                system_id: opt(f[3])?,
                ticker: f[12].to_owned(),
                name: f[13].to_owned(),
            };
            t.corporations.insert(c.corp_id, c);
        }
        for f in rows(TYPES, &file(TYPES)?)? {
            let r = NpcTypeRow {
                type_id: int(f[0])?,
                group_id: int(f[1])?,
                faction_id: int(f[2])?,
                context: f[4].to_owned(),
                class: f[5].to_owned(),
                tier: Tier::parse(f[6])?,
                alias_of: opt(f[7])?,
                needs: Needs::parse(f[8])?,
                role: Role::parse(f[9])?,
            };
            t.types.insert(r.type_id, r);
        }
        for f in rows(COMPONENTS, &file(COMPONENTS)?)? {
            t.components.insert((int(f[0])?, f[1].to_owned()));
        }
        for f in rows(REACTIVE_SPAWN, &file(REACTIVE_SPAWN)?)? {
            t.reactive.push(ReactiveSpawn {
                type_id: int(f[0])?,
                scope: f[1].to_owned(),
                key: opt(f[2])?,
                spawn_table_id: opt(f[3])?,
                owner_id: opt(f[5])?,
            });
        }
        for f in rows(BOUNTY, &file(BOUNTY)?)? {
            let multipliers = map(f[2], |k, v| Ok((int(k)?, float(v)?)))?;
            t.bounty.insert(
                int(f[0])?,
                Bounty {
                    bounty: float(f[1])?,
                    multipliers,
                },
            );
        }
        for f in rows(REGIONS, &file(REGIONS)?)? {
            t.regions.entry(int(f[0])?).or_default().push(RegionPirate {
                faction_id: int(f[1])?,
                weight: float(f[2])?,
                rule: Rule::parse(f[3])?,
            });
        }
        for f in rows(HOME_PIRATE, &file(HOME_PIRATE)?)? {
            t.home_pirate.insert(int(f[0])?, int(f[1])?);
        }
        for f in rows(LOOT, &file(LOOT)?)? {
            t.loot.push(Loot {
                faction_id: int(f[0])?,
                size: int(f[1])?,
                kind: f[2].to_owned(),
                tier: Tier::parse(f[3])?,
                type_id: int(f[4])?,
                meta_level: opt(f[5])?,
            });
        }
        for f in rows(POLICE, &file(POLICE)?)? {
            let at = |e: String| format!("{}: {e}", POLICE.0);
            let p = Police {
                faction_id: int(f[0]).map_err(at)?,
                warning: f[1].to_owned(),
                types: list(f[2]).map(int).collect::<Result<_, _>>().map_err(at)?,
            };
            t.police.insert(p.faction_id, p);
        }
        for f in rows(CONCORD_ROLES, &file(CONCORD_ROLES)?)? {
            let id = int(f[1]).map_err(|e| format!("{}: {e}", CONCORD_ROLES.0))?;
            t.concord_roles.insert(f[0].to_owned(), id);
        }
        Ok(t)
    }

    /// Invented rows (ids from `schema.md`'s unused ranges), never a client
    /// value: two pirate factions 599011 and 599012, an empire 599002, five
    /// regions 19000101-19000105, three types 990300001-990300003.
    pub fn fixture() -> Table {
        let text = |(rel, header): (&str, &str)| {
            let body = match rel {
                "npc/factions.tsv" => {
                    "599011\t1999011\t\t39000011\t\t1999019\t99111\tFrigate=990380;Cruiser=990381\t0.1,0.4,0.4,0.1\t1=99211\tshield\t\tFixa\tArch Fixa\tDread Fixa\t\t\t\tFixture Cartel\n\
                     599012\t1999012\t\t39000012\t\t\t99112\tFrigate=990382\t0.5,0.0,0.0,0.5\t\tarmor\t\tFixb\t\t\t\t\t\tFixture Syndicate\n\
                     599002\t1999002\t1999003\t39000002\t1\t\t\t\t\t\t\t\t\t\t\t\t\t\tFixture Empire\n"
                }
                "npc/corporations.tsv" => {
                    "1999011\t599011\t\t\t\t\t0.0\t\t5\tM\tR\t\tFXA\tFixture Corp\n"
                }
                "npc/types.tsv" => {
                    "990300001\t99111\t599011\tgroup\tAsteroid\tFrigate\tbase\t\tTURRET\tDPS\n\
                     990300002\t99111\t599011\ttype\tAsteroid\tFrigate\telite\t\tTURRET,SCRAM\tTACKLER\n\
                     990300003\t99112\t599012\tgroup\tAsteroid\tCruiser\tbase\t990300001\tMISSILE\tDPS\n"
                }
                "npc/bounty.tsv" => "990300001\t1500.0\t1999011=0.5\n",
                "npc/regions.tsv" => {
                    "19000101\t599011\t1.0\t1\n\
                     19000102\t599012\t1.0\t2\n\
                     19000102\t599011\t0.25\tsecondary\n\
                     19000103\t599011\t1.0\t3\n\
                     19000104\t599011\t0.5\t3\n\
                     19000104\t599012\t2.0\toverride\n"
                }
                "npc/home-pirate.tsv" => "599002\t599011\n",
                "npc/loot.tsv" => "599011\t1\tweapon\tbase\t990390\t0\n",
                "npc/police.tsv" => "599002\tchatFixturePoliceWarning\t990330011,990330012\n",
                _ => "",
            };
            Ok(Some(format!("{header}\n{body}")))
        };
        match Table::from_files(|rel| {
            let all = [
                FACTIONS,
                CORPORATIONS,
                TYPES,
                COMPONENTS,
                REACTIVE_SPAWN,
                BOUNTY,
                REGIONS,
                HOME_PIRATE,
                LOOT,
                POLICE,
                CONCORD_ROLES,
            ];
            all.into_iter()
                .find(|(r, _)| *r == rel)
                .map_or(Ok(None), text)
        }) {
            Ok(t) => t,
            Err(e) => unreachable!("the invented fixture parses: {e}"),
        }
    }

    /// **R3: the pirate a region's belts hold**, the region's heaviest row.
    /// The extractor has already run R3's steps 0-4 (one primary row per
    /// region), added the secondaries and applied the override file, so the
    /// runtime rule is: the largest weight; on a tie, the override, then the
    /// earliest step, then a secondary; then the lowest faction id. `None`
    /// for a region with no pirates (wormholes, Pochven, Abyssal: C6 R3).
    pub fn region_pirate(&self, region_id: i64) -> Option<i64> {
        self.regions
            .get(&region_id)?
            .iter()
            .max_by(|a, b| {
                a.weight
                    .total_cmp(&b.weight)
                    .then(b.rule.cmp(&a.rule))
                    .then(b.faction_id.cmp(&a.faction_id))
            })
            .map(|p| p.faction_id)
    }

    /// Every pirate a region holds, with its weight, for a weighted draw
    /// (CP22, CP30).
    pub fn pirates(&self, region_id: i64) -> &[RegionPirate] {
        self.regions.get(&region_id).map_or(&[], Vec::as_slice)
    }

    /// **`pirate(system)`**: [`Self::region_pirate`] of the system's region,
    /// read from the catalogue's `SystemRow.region_id` (CA9). `None` for a
    /// system the catalogue does not know.
    pub fn pirate(&self, systems: &crate::celestial::Catalogue, system_id: i64) -> Option<i64> {
        self.region_pirate(systems.system(system_id)?.region_id)
    }

    /// The home pirate of an offering faction (`npc/home-pirate.tsv`).
    pub fn home_pirate(&self, faction_id: i64) -> Option<i64> {
        self.home_pirate.get(&faction_id).copied()
    }

    /// Whether `npc/components.tsv` lists `component` on the type.
    pub fn has_component(&self, type_id: i64, component: &str) -> bool {
        self.components.contains(&(type_id, component.to_owned()))
    }

    /// **R2: what a type fights with**, from `npc/types.tsv` and the type's
    /// own attributes and effects in `td`. `None` for a type `npc/types.tsv`
    /// does not list, or that `td` lacks.
    ///
    /// `needs` is the row's column plus what the dogma shows: TURRET for any
    /// hull damage, MISSILE for attribute 507 > 0, SCRAM for effect 563,
    /// WEB for attribute 512 > 0, SELF_REP for a repair amount > 0 and
    /// REMOTE_REP for a remote-repair effect.
    pub fn profile(&self, td: &crate::typedata::Table, type_id: i64) -> Option<Profile> {
        use dogma as d;
        let row = self.types.get(&type_id)?;
        let tr = td.get(type_id)?;
        let a = |id: i64| td.attribute(type_id, id);
        let pos = |v: f64| (v > 0.0).then_some(v);
        let missile = pos(a(d::MISSILE_TYPE)).map(|m| Missile {
            type_id: m as i64,
            multiplier: a(d::MISSILE_MULTIPLIER),
            period_ms: a(d::MISSILE_PERIOD),
        });
        let has = |e: i64| tr.effects.contains(&e);
        let scram = if has(d::EFFECT_SCRAM) || has(d::EFFECT_SCRAM_MWD) {
            Some(Scram {
                range: a(d::SCRAM_RANGE),
                strength: a(d::SCRAM_STRENGTH),
                chance: a(d::SCRAM_CHANCE),
                duration_ms: a(d::SCRAM_DURATION),
                blocks_mwd: has(d::EFFECT_SCRAM_MWD),
            })
        } else if has(d::EFFECT_BEHAVIOUR_SCRAM) {
            Some(Scram {
                range: a(d::BEHAVIOUR_SCRAM_RANGE),
                strength: a(d::BEHAVIOUR_SCRAM_STRENGTH),
                chance: d::BEHAVIOUR_CHANCE,
                duration_ms: a(d::BEHAVIOUR_SCRAM_DURATION),
                blocks_mwd: true,
            })
        } else {
            None
        };
        let web_behaviour = has(d::EFFECT_BEHAVIOUR_WEB).then(|| WebBehaviour {
            range: a(d::BEHAVIOUR_WEB_RANGE),
            strength: a(d::SPEED_FACTOR),
            duration_ms: a(d::BEHAVIOUR_WEB_DURATION),
        });
        let neut = if has(d::EFFECT_NEUT) {
            Some(Neut {
                range: a(d::NEUT_RANGE),
                falloff: 0.0,
                amount: a(d::NEUT_AMOUNT),
                duration_ms: a(d::NEUT_DURATION),
                chance: a(d::NEUT_CHANCE).clamp(0.0, 1.0),
            })
        } else if has(d::EFFECT_BEHAVIOUR_NEUT) {
            Some(Neut {
                range: a(d::BEHAVIOUR_NEUT_RANGE),
                falloff: a(d::BEHAVIOUR_NEUT_FALLOFF).max(0.0),
                amount: a(d::NEUT_AMOUNT),
                duration_ms: a(d::BEHAVIOUR_NEUT_DURATION),
                chance: d::BEHAVIOUR_CHANCE,
            })
        } else {
            None
        }
        .filter(|n| n.amount > 0.0 && n.range > 0.0 && n.duration_ms > 0.0);
        let remote = |armor: bool,
                      amount: i64,
                      duration: i64,
                      chance: f64,
                      threshold: f64,
                      range: f64,
                      max: i64| RemoteRep {
            armor,
            amount: a(amount),
            period_ms: a(duration),
            chance: chance.clamp(0.0, 1.0),
            threshold,
            range,
            max_targets: (a(max).max(1.0)) as usize,
        };
        let remote_rep = if has(d::EFFECT_REMOTE_ARMOR) {
            Some(remote(
                true,
                d::REMOTE_ARMOR_AMOUNT,
                d::REMOTE_ARMOR_DURATION,
                a(d::REMOTE_ARMOR_CHANCE),
                a(d::REMOTE_ARMOR_THRESHOLD),
                a(d::ASSISTANCE_RANGE),
                d::REMOTE_ARMOR_MAX_TARGETS,
            ))
        } else if has(d::EFFECT_REMOTE_SHIELD) {
            Some(remote(
                false,
                d::REMOTE_SHIELD_AMOUNT,
                d::REMOTE_SHIELD_DURATION,
                a(d::REMOTE_SHIELD_CHANCE),
                a(d::REMOTE_SHIELD_THRESHOLD),
                a(d::ASSISTANCE_RANGE),
                d::REMOTE_SHIELD_MAX_TARGETS,
            ))
        } else if has(d::EFFECT_BEHAVIOUR_ARMOR) {
            Some(remote(
                true,
                d::ARMOR_DAMAGE_AMOUNT,
                d::BEHAVIOUR_ARMOR_DURATION,
                d::BEHAVIOUR_CHANCE,
                d::BEHAVIOUR_THRESHOLD,
                a(d::BEHAVIOUR_ARMOR_RANGE),
                0,
            ))
        } else if has(d::EFFECT_BEHAVIOUR_SHIELD) {
            Some(remote(
                false,
                d::SHIELD_BONUS,
                d::BEHAVIOUR_SHIELD_DURATION,
                d::BEHAVIOUR_CHANCE,
                d::BEHAVIOUR_THRESHOLD,
                a(d::BEHAVIOUR_SHIELD_RANGE),
                0,
            ))
        } else {
            None
        }
        .filter(|r| r.amount > 0.0 && r.period_ms > 0.0 && r.range > 0.0);
        let rep = |armor, amount, period, delay| {
            pos(a(amount)).map(|amount| SelfRep {
                armor,
                amount,
                period_ms: a(period),
                delay_chance: a(delay),
            })
        };
        let self_rep = rep(
            false,
            d::SHIELD_REP_AMOUNT,
            d::SHIELD_REP_PERIOD,
            d::SHIELD_REP_DELAY_CHANCE,
        )
        .or_else(|| {
            rep(
                true,
                d::ARMOR_REP_AMOUNT,
                d::ARMOR_REP_PERIOD,
                d::ARMOR_REP_DELAY_CHANCE,
            )
        });
        let web_chance = pos(a(d::WEB_CHANCE));
        let mut needs = row.needs;
        for (has, n) in [
            (d::DAMAGE.iter().any(|id| a(*id) > 0.0), Needs::TURRET),
            (missile.is_some(), Needs::MISSILE),
            (scram.is_some(), Needs::SCRAM),
            (web_chance.is_some() || web_behaviour.is_some(), Needs::WEB),
            (self_rep.is_some(), Needs::SELF_REP),
            (neut.is_some(), Needs::NEUT),
            (
                d::EFFECTS_REMOTE_REP.iter().any(|e| tr.effects.contains(e)),
                Needs::REMOTE_REP,
            ),
        ] {
            if has {
                needs = needs.union(n);
            }
        }
        Some(Profile {
            type_id,
            faction_id: row.faction_id,
            tier: row.tier,
            role: row.role,
            needs,
            missile,
            scram,
            web_chance,
            web_behaviour,
            neut,
            remote_rep,
            self_rep,
            attack_delay_ms: (a(d::ATTACK_DELAY_MIN), a(d::ATTACK_DELAY_MAX)),
            chase_max: a(d::CHASE_MAX),
            target_switch: a(d::TARGET_SWITCH) == 1.0,
            switch_timer_ms: a(d::SWITCH_TIMER),
            switch_skip_chance: a(d::SWITCH_SKIP_CHANCE).clamp(0.0, 1.0),
            prefer_signature: a(d::PREFER_SIGNATURE) == 1.0,
            bounty: self
                .bounty
                .get(&type_id)
                .map_or_else(|| a(d::BOUNTY), |b| b.bounty),
        })
    }
}

// ---------------------------------------------------------------------------
// Parsing (`schema.md` §1.1): header byte for byte, exact column counts
// ---------------------------------------------------------------------------

/// Line 1 must be exactly `header`.
fn check_header(rel: &str, text: &str, header: &str) -> Result<(), String> {
    let found = text.lines().next().unwrap_or("");
    if found == header {
        Ok(())
    } else {
        Err(format!(
            "{rel}: header is\n  {found}\nexpected\n  {header}\nRe-extract with tools/up.sh"
        ))
    }
}

/// The data rows after the header, each split into exactly the header's
/// column count.
fn rows<'a>((rel, header): (&str, &str), text: &'a str) -> Result<Vec<Vec<&'a str>>, String> {
    let n = header.split('\t').count();
    text.lines()
        .enumerate()
        .skip(1)
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'))
        .map(|(i, l)| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() == n {
                Ok(f)
            } else {
                Err(format!(
                    "{rel}:{}: {} columns, expected {n}",
                    i + 1,
                    f.len()
                ))
            }
        })
        .collect()
}

/// One factions row; an error names the file.
fn faction(f: &[&str]) -> Result<Faction, String> {
    let at = |e: String| format!("{}: {e}", FACTIONS.0);
    let s = |v: &str| (!v.is_empty()).then(|| v.to_owned());
    let member_races: Vec<i64> = list(f[4]).map(int).collect::<Result<_, _>>().map_err(at)?;
    Ok(Faction {
        faction_id: int(f[0]).map_err(at)?,
        corporation_id: int(f[1]).map_err(at)?,
        militia_corporation_id: opt(f[2]).map_err(at)?,
        system_id: int(f[3]).map_err(at)?,
        member_races,
        owner_corp_id: opt(f[5]).map_err(at)?,
        groups: list(f[6]).map(int).collect::<Result<_, _>>().map_err(at)?,
        wrecks: map(f[7], |k, v| Ok((k.to_owned(), int(v)?))).map_err(at)?,
        damage_profile: list(f[8])
            .map(float)
            .collect::<Result<_, _>>()
            .map_err(at)?,
        ammo_groups: map(f[9], |k, v| Ok((int(k)?, int(v)?))).map_err(at)?,
        tank: s(f[10]),
        tags: list(f[11]).map(int).collect::<Result<_, _>>().map_err(at)?,
        base_prefix: s(f[12]),
        elite_prefix: s(f[13]),
        commander_prefix: s(f[14]),
        deadspace_stems: list(f[15]).map(str::to_owned).collect(),
        officers: list(f[16]).map(str::to_owned).collect(),
        name: f[18].to_owned(),
    })
}

fn int(s: &str) -> Result<i64, String> {
    s.parse().map_err(|_| format!("bad integer {s:?}"))
}

fn opt(s: &str) -> Result<Option<i64>, String> {
    (!s.is_empty()).then(|| int(s)).transpose()
}

fn float(s: &str) -> Result<f64, String> {
    s.parse().map_err(|_| format!("bad float {s:?}"))
}

/// `a,b,c`; the empty field is the empty list.
fn list(s: &str) -> impl Iterator<Item = &str> {
    s.split(',').filter(|v| !v.is_empty())
}

/// `k=v;k=v`; the empty field is the empty map.
fn map<K: Ord, V>(
    s: &str,
    kv: impl Fn(&str, &str) -> Result<(K, V), String>,
) -> Result<BTreeMap<K, V>, String> {
    s.split(';')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p
                .split_once('=')
                .ok_or_else(|| format!("bad map entry {p:?}"))?;
            kv(k, v)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// NW-8: `member_races` is kept and the group-99 rows of `types/market.tsv`
    /// become `(race, type)`, unpublished or not; invented ids.
    #[test]
    fn member_races_and_sentry_types_are_read() {
        let faction = "990100\t990110\t\t990120\t7,9\t\t\t\t\t\t\t\t\t\t\t\t\t\tFixture\n";
        let market = |rows: &[&str]| {
            let mut t = format!("{}\n", crate::econ_exchange::MARKET.1);
            for r in rows {
                t.push_str(r);
                t.push('\n');
            }
            t
        };
        let t = Table::from_files(|rel| {
            Ok(match rel {
                "npc/factions.tsv" => Some(format!("{}\n{faction}", FACTIONS.1)),
                "types/market.tsv" => Some(market(&[
                    "990202\t99\t11\t\t0.0\t\t\t\t\t\t7\t\t1\t0",
                    "990200\t99\t11\t\t0.0\t\t\t\t\t\t9\t\t1\t0",
                    "990300\t98\t11\t\t0.0\t\t\t\t\t\t9\t\t1\t0",
                    "990301\t99\t11\t\t0.0\t\t\t\t\t\t\t\t1\t0",
                ])),
                _ => None,
            })
        })
        .expect("parses");
        assert_eq!(t.factions[&990_100].member_races, vec![7, 9]);
        assert_eq!(t.sentry_types, vec![(7, 990_202), (9, 990_200)]);
        let none = Table::from_files(|_| Ok(None)).expect("parses");
        assert!(none.sentry_types.is_empty());
    }

    /// `schema.md`'s invented example row for each file, after its header.
    const EXAMPLES: &[((&str, &str), &str)] = &[
        (
            FACTIONS,
            "599001\t1999001\t\t39000001\t1\t1999002\t99101,99102\tFrigate=990080;Cruiser=990081\t0.1,0.4,0.4,0.1\t1=99201;2=99202\tshield\t990090,990091\tExample\tArch Example\tDread Example\tExi,Exior,Exum,Exatis,Ex\tExa\texamplecorp\tExample Cartel",
        ),
        (
            CORPORATIONS,
            "1999001\t599001\t69000001\t39000001\t\t\t0.0\t0.0\t5\tM\tR\t9901,9902\tEXC\tExample Corporation",
        ),
        (
            TYPES,
            "990100\t99101\t599001\tgroup\tAsteroid\tFrigate\tbase\t\tTURRET,SCRAM\tTACKLER",
        ),
        (COMPONENTS, "990100\tentityStandings"),
        (
            REACTIVE_SPAWN,
            "990110\ttype\t\t9001\t\t1999001\t901\t1.0\t0.0",
        ),
        (BOUNTY, "990100\t12500.0\t1999001=0.5"),
        (REGIONS, "19000001\t599001\t1.0\t2"),
        (HOME_PIRATE, "599002\t599001"),
        (LOOT, "599001\t1\tweapon\tbase\t990120\t0"),
    ];

    fn from(files: &[((&str, &str), String)]) -> Result<Table, String> {
        Table::from_files(|rel| {
            Ok(files
                .iter()
                .find(|((r, _), _)| *r == rel)
                .map(|(_, t)| t.clone()))
        })
    }

    /// Every schema example parses under its header, and every file's
    /// row lands where a reader looks for it.
    #[test]
    fn the_schema_examples_parse() {
        let files: Vec<_> = EXAMPLES
            .iter()
            .map(|(f, row)| (*f, format!("{}\n# a comment\n{row}\n", f.1)))
            .collect();
        let t = from(&files).expect("the schema examples parse");
        let f = &t.factions[&599_001];
        assert_eq!(f.owner(), 1_999_002);
        assert_eq!(f.wrecks["Cruiser"], 990_081);
        assert_eq!(f.deadspace_stems.len(), 5);
        assert_eq!(t.types[&990_100].needs, Needs::TURRET.union(Needs::SCRAM));
        assert_eq!(t.types[&990_100].role, Role::Tackler);
        assert!(t.has_component(990_100, "entityStandings"));
        assert_eq!(t.reactive[0].owner_id, Some(1_999_001));
        assert_eq!(t.bounty[&990_100].multipliers[&1_999_001], 0.5);
        assert_eq!(t.corporations[&1_999_001].faction_id, Some(599_001));
        assert_eq!(t.loot[0].type_id, 990_120);
        assert_eq!(t.region_pirate(19_000_001), Some(599_001));
    }

    /// An absent file is an empty table, never an error (plan §1.3).
    #[test]
    fn absent_files_give_an_empty_table() {
        assert_eq!(from(&[]).expect("no files"), Table::default());
    }

    /// **A bad header is rejected**, naming the file: a stale bundle fails
    /// loudly (`schema.md`: loaders compare byte for byte). So is a row with
    /// the wrong column count, and an unknown enum value.
    #[test]
    fn a_bad_header_is_rejected() {
        let swapped = "#region_id\tweight\tfaction_id\trule\n".to_owned();
        let e = from(&[(REGIONS, swapped)]).expect_err("a reordered header");
        assert!(e.contains("npc/regions.tsv") && e.contains("header"), "{e}");
        let short = format!("{}\n19000001\t599001\t1.0\n", REGIONS.1);
        assert!(from(&[(REGIONS, short)]).is_err());
        let role = format!(
            "{}\n990100\t99101\t599001\tgroup\tAsteroid\tFrigate\tbase\t\tTURRET\tHEALER\n",
            TYPES.1
        );
        assert!(from(&[(TYPES, role)]).is_err());
    }

    /// **R3 on fixture regions**: a step-1 region; a step-2 region whose
    /// secondary does not displace its primary; a step-3 region; an override
    /// row replacing the derived step; no rows means no pirates. Then
    /// `pirate(system)` reads the region from the catalogue.
    #[test]
    fn pirate_follows_the_r3_rows() {
        let t = Table::fixture();
        assert_eq!(t.region_pirate(19_000_101), Some(599_011));
        assert_eq!(t.region_pirate(19_000_102), Some(599_012));
        assert_eq!(t.pirates(19_000_102).len(), 2, "the secondary is kept");
        assert_eq!(t.region_pirate(19_000_103), Some(599_011));
        assert_eq!(t.region_pirate(19_000_104), Some(599_012));
        assert_eq!(t.region_pirate(19_000_105), None);
        let cat = crate::celestial::Catalogue::parse(
            "#system\t39000101\t29000101\t19000102\n#system\t39000105\t29000105\t19000105\n",
        );
        assert_eq!(t.pirate(&cat, 39_000_101), Some(599_012));
        assert_eq!(t.pirate(&cat, 39_000_105), None);
        assert_eq!(t.pirate(&cat, 39_000_999), None, "an unknown system");
    }

    /// A tie on weight goes to the earlier step, never to a secondary.
    #[test]
    fn a_weight_tie_goes_to_the_earlier_rule() {
        let text = format!(
            "{}\n1\t599012\t1.0\tsecondary\n1\t599013\t1.0\t3\n1\t599011\t1.0\t4\n",
            REGIONS.1
        );
        let t = from(&[(REGIONS, text)]).expect("parses");
        assert_eq!(t.region_pirate(1), Some(599_013));
    }

    /// `home_pirate` reads `npc/home-pirate.tsv`.
    #[test]
    fn home_pirate_reads_its_file() {
        let t =
            from(&[(HOME_PIRATE, format!("{}\n599002\t599012\n", HOME_PIRATE.1))]).expect("parses");
        assert_eq!(t.home_pirate(599_002), Some(599_012));
        assert_eq!(t.home_pirate(599_012), None);
        assert_eq!(Table::fixture().home_pirate(599_002), Some(599_011));
    }

    /// NW-7: `police` reads `npc/police.tsv`, and a bad row names the file.
    #[test]
    fn police_reads_its_file() {
        let text = format!(
            "{}\n599002\tchatFixWarning\t990330011,990330012\n",
            POLICE.1
        );
        let t = from(&[(POLICE, text)]).expect("parses");
        let p = &t.police[&599_002];
        assert_eq!(p.warning, "chatFixWarning");
        assert_eq!(p.types, vec![990_330_011, 990_330_012]);
        assert!(Table::fixture().police.contains_key(&599_002));
        let bad = format!("{}\n599002\tchatFixWarning\tx\n", POLICE.1);
        assert!(
            from(&[(POLICE, bad)])
                .expect_err("rejects")
                .contains("npc/police.tsv")
        );
    }

    /// `concord_roles` reads `npc/concord.tsv`, is empty without it, and a bad
    /// row names the file.
    #[test]
    fn concord_roles_read_their_file() {
        let text = format!("{}\nofficer\t990330003\n", CONCORD_ROLES.1);
        let t = from(&[(CONCORD_ROLES, text)]).expect("parses");
        assert_eq!(t.concord_roles["officer"], 990_330_003);
        assert!(Table::fixture().concord_roles.is_empty());
        let bad = format!("{}\nofficer\tx\n", CONCORD_ROLES.1);
        assert!(
            from(&[(CONCORD_ROLES, bad)])
                .expect_err("rejects")
                .contains("npc/concord.tsv")
        );
    }

    /// Invented typedata for [`Table::fixture`]'s types: a turret frigate
    /// with a shield repairer and the leash numbers, a scrambler (effect
    /// 563) with a web chance, and a missile cruiser (attribute 507 > 0).
    const TYPEDATA: &str = "\
attribute\t114\t0.0\temDamage
type\t990300001\t99111\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t10\t10\t114=2.0;475=1000.0;476=3000.0;481=99.0;636=5000.0;637=20.0;639=0.25;665=40000.0;1416=15000.0;1648=1.0;1650=1.0\tFixture A
type\t990300002\t99111\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t10\t10,563\t103=9000.0;105=2.0;504=0.5;512=0.3\tFixture B
type\t990300003\t99112\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t10\t10\t212=1.5;506=8000.0;507=990399.0\tFixture C
";

    fn typedata() -> crate::typedata::Table {
        crate::typedata::Table::parse(TYPEDATA).expect("the invented typedata parses")
    }

    /// **Derived fields from a fixture row**: every number comes back from
    /// the type's own attribute, the names from `npc/types.tsv`, and the
    /// bounty from `npc/bounty.tsv` where it has a row.
    #[test]
    fn a_profile_reads_its_own_attributes() {
        let p = Table::fixture()
            .profile(&typedata(), 990_300_001)
            .expect("a profile");
        assert_eq!(
            (p.faction_id, p.tier, p.role),
            (599_011, Tier::Base, Role::Dps)
        );
        assert_eq!(p.attack_delay_ms, (1000.0, 3000.0));
        assert_eq!(p.chase_max, 40_000.0);
        assert!(p.target_switch && p.prefer_signature);
        assert_eq!(p.switch_timer_ms, 15_000.0);
        let rep = p.self_rep.expect("a shield repairer");
        assert_eq!(
            (rep.armor, rep.amount, rep.period_ms, rep.delay_chance),
            (false, 20.0, 5000.0, 0.25)
        );
        assert_eq!(p.bounty, 1500.0, "the bounty file wins over attribute 481");
        assert!(p.needs.contains(Needs::TURRET.union(Needs::SELF_REP)));
        assert!(p.missile.is_none() && p.scram.is_none());
        assert!(Table::fixture().profile(&typedata(), 990_399_999).is_none());
    }

    /// **`needs ∋ SCRAM` for effect 563** [V, C6 §8], with the scram's own
    /// numbers; a web chance sets WEB.
    #[test]
    fn effect_563_needs_scram() {
        let p = Table::fixture()
            .profile(&typedata(), 990_300_002)
            .expect("a profile");
        assert!(p.needs.contains(Needs::SCRAM) && p.needs.contains(Needs::WEB));
        let s = p.scram.expect("a scrambler");
        assert_eq!((s.range, s.strength, s.chance), (9000.0, 2.0, 0.5));
        assert_eq!(p.web_chance, Some(0.3));
    }

    /// **`missile.is_some()` when attribute 507 > 0** [V, C6 §8].
    #[test]
    fn attribute_507_gives_a_missile() {
        let p = Table::fixture()
            .profile(&typedata(), 990_300_003)
            .expect("a profile");
        let m = p.missile.expect("a launcher");
        assert_eq!(
            (m.type_id, m.multiplier, m.period_ms),
            (990_399, 1.5, 8000.0)
        );
        assert!(p.needs.contains(Needs::MISSILE));
        assert!(!p.needs.contains(Needs::TURRET), "no hull damage");
    }

    /// Invented typedata for K-RATAI: one fixture type per tackle or
    /// remote-repair effect, carrying the client's own effect ids and
    /// attribute ids with made-up numbers.
    const RATAI: &str = "\
attribute\t1454\t10000.0\tnpcRemoteArmorRepairDuration
type\t990300001\t99111\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t10\t5928,3852\t103=11000.0;105=1.0;504=0.7;505=4000.0;512=0.0;1453=0.6;1455=33.0;1456=0.4;1464=22000.0;1501=2.0;1651=0.25\tRatai A
type\t990300002\t99111\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t10\t6745,6743,3855\t2506=3000.0;2507=12000.0;2509=2.0;2499=2500.0;2500=9000.0;20=-45.0;1458=7000.0;1459=0.8;1460=44.0;1462=0.6;1464=21000.0;1502=3.0\tRatai B
type\t990300003\t99112\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t10\t6741\t2491=6000.0;2492=18000.0;84=55.0\tRatai C
";

    fn ratai(id: i64) -> Profile {
        let td = crate::typedata::Table::parse(RATAI).expect("the invented rows parse");
        Table::fixture().profile(&td, id).expect("a profile")
    }

    /// **5928 reads the numbers of 563** (103, 105, 504, 505), is a
    /// scrambler for `needs`, and carries the MWD-block flag; 1651 and the
    /// armour repairer (3852) come from their own attributes.
    #[test]
    fn effect_5928_and_3852_read_their_own_attributes() {
        let p = ratai(990_300_001);
        let s = p.scram.expect("a scrambler");
        assert_eq!(
            (s.range, s.strength, s.chance, s.duration_ms, s.blocks_mwd),
            (11000.0, 1.0, 0.7, 4000.0, true)
        );
        assert!(p.needs.contains(Needs::SCRAM));
        assert_eq!(p.switch_skip_chance, 0.25);
        let r = p.remote_rep.expect("a remote armour repairer");
        assert_eq!(
            (r.armor, r.amount, r.period_ms, r.chance),
            (true, 33.0, 10_000.0, 0.6),
            "1454's own default is the cycle"
        );
        assert_eq!((r.threshold, r.range, r.max_targets), (0.4, 22000.0, 2));
        assert!(p.needs.contains(Needs::REMOTE_REP));
    }

    /// **6745 and 6743 read the `behavior*` attributes**, and 3855 the shield
    /// pair, whose cycle and max targets are not the armour pair's.
    #[test]
    fn behaviour_effects_read_their_own_attributes() {
        let p = ratai(990_300_002);
        let s = p.scram.expect("a behaviour scrambler");
        assert_eq!(
            (s.range, s.strength, s.duration_ms, s.chance),
            (12000.0, 2.0, 3000.0, dogma::BEHAVIOUR_CHANCE)
        );
        let w = p.web_behaviour.expect("a behaviour web");
        assert_eq!(
            (w.range, w.strength, w.duration_ms),
            (9000.0, -45.0, 2500.0)
        );
        assert!(p.needs.contains(Needs::WEB));
        let r = p.remote_rep.expect("a remote shield booster");
        assert_eq!(
            (
                r.armor,
                r.amount,
                r.period_ms,
                r.chance,
                r.threshold,
                r.max_targets
            ),
            (false, 44.0, 7000.0, 0.8, 0.6, 3)
        );
        assert_eq!(p.switch_skip_chance, 0.0);
    }

    /// **6741** repairs with `armorDamageAmount` over its own range, any
    /// damage at all ([`dogma::BEHAVIOUR_THRESHOLD`]).
    #[test]
    fn effect_6741_repairs_with_the_armour_damage_amount() {
        let r = ratai(990_300_003).remote_rep.expect("a repairer");
        assert_eq!(
            (r.armor, r.amount, r.period_ms, r.range, r.max_targets),
            (true, 55.0, 6000.0, 18000.0, 1)
        );
        assert_eq!(
            (r.chance, r.threshold),
            (dogma::BEHAVIOUR_CHANCE, dogma::BEHAVIOUR_THRESHOLD)
        );
    }
}
