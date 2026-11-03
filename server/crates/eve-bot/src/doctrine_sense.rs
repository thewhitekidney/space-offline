//! **What a bot can see of a battle, and what a leader makes of it** (DOC-3;
//! R-DOCTRINE sections 5 and 6). Pure: no socket, no clock (every function
//! takes `now_s`), no dice but a hash of the inputs.
//!
//! Every input is something the client itself is given **[V]**:
//!
//! | sense | where it comes from | here |
//! |---|---|---|
//! | d-scan | `ConeScan` rows, hull types, no owner | [`Sighting::scan`] (`hunt::Sweep::hulls`) |
//! | the grid | ballpark slims: type, corp, alliance | [`Sighting::grid`] (`hunt::Pilot`) |
//! | Local | MUC occupant presences with `eve_user_data` | [`Sighting::local`] (`chat::Roster`) |
//! | damage taken | `OnDamageMessage` (`damageAttributes`, `weapon`) | [`parse_damage`], [`DamageTally`] |
//! | killmails | `GetRecentShipKillsAndLosses`: every attacker's hull and weapon, the victim's items in `killBlob` | [`kill_evidence`], [`Memory::note_kill`] |
//!
//! Three things come out:
//!
//! 1. [`observe`]: the [`Picture`], merged from every member's [`Sighting`]
//!    on a hall tick (intel share, hull counts, owner shares from Local,
//!    noise, reliability, blindness).
//! 2. [`Memory`]: what each enemy group was seen to field, decaying with a
//!    half-life; [`infer`] reads a group's likely doctrine from it.
//! 3. [`DamageTally`]: the damage-type split of what has hit me (feeds
//!    alternate ammo and tank swaps in DOC-4/5).
//!
//! **No behaviour changes here.** Nothing calls `observe` yet but the tests;
//! bots post their [`Sighting`] and kill evidence to the [`Board`], and the
//! leader (DOC-4) will ask it.
//!
//! **Where the spec and this differ**, in one place: the residual of a hull
//! is `scan - mine - the grid's exact count` (R-DOCTRINE 6.2 rule 1 replaces
//! the scan by the grid for a hull on the grid; this keeps the off-grid
//! remainder of the same hull, which the replacement would drop). It is the
//! spec's number whenever everything is on the grid. And reliability is
//! `min(1, seen / OBS_RELIABLE_SEEN)`: the spec's formula is written
//! inverted, its prose ("more distinct hulls and observers push it up") is
//! this.

use crate::doctrine::{Doctrine, Family, Layer};
use crate::doctrine_fit::{Kind, Library};
use crate::hull_choice::Class;
use crate::society::{self, Group};
use eve_marshal::PyValue;
use eve_server::damage::attr;
use eve_server::group_contacts::Affiliation;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Mutex;

// ---- constants -------------------------------------------------------------

/// Seconds between hall updates in a battle (R-DOCTRINE 11) **[A]**.
/// Falsified by a leader that acts on a picture older than its own fleet's
/// fight.
pub const BATTLE_TICK_S: u64 = 30;

/// Distinct hulls (plus extra observers) that make a picture fully reliable
/// (R-DOCTRINE 11) **[A]**. Falsified by a picture rated 1.0 that misses the
/// later exact count by more than 2x.
pub const OBS_RELIABLE_SEEN: f64 = 6.0;

/// A sighting older than this is not in the picture **[A]**: three battle
/// ticks. Falsified by a picture that lags a fleet's arrival.
pub const SIGHTING_STALE_S: u64 = 3 * BATTLE_TICK_S;

/// A directional scan older than this is not a view **[A]**. Falsified by
/// scans that outlive the ships they counted.
pub const SCAN_STALE_S: u64 = 600;

/// Remembered hulls below this many ships (after decay) are forgotten
/// **[A]**.
pub const MEMORY_FLOOR: f64 = 0.5;

/// How many doctrines [`infer`] returns.
pub const INFER_TOP: usize = 3;

/// The confidence a single kill report gives a group **[A]**: one hull, one
/// fit. Falsified by a counter chosen on one kill that loses.
pub const KILL_CONF: f64 = 0.2;

/// Bounds, so hostile input cannot grow a bot: the kill evidence kept, the
/// attacker groups tallied, the sightings a board holds.
pub const MAX_EVIDENCE: usize = 64;
pub const MAX_ATTACKERS: usize = 16;
pub const MAX_SIGHTINGS: usize = 256;

/// The group of ships whose owner Local cannot tell us.
pub const UNATTRIBUTED: Group = 0;

/// Seconds in a game day (the memory's half-life is in days).
pub const DAY_S: f64 = 86_400.0;

/// **The observation knobs** (R-DOCTRINE 10): exactly the observation group
/// of the difficulty table. [`Self::NORMAL`] is the spec's normal column;
/// DOC-8's `Difficulty` will produce one per level (`at`). Reaction,
/// commitment and the rest are not here; relation constants live in
/// `relations::Temper`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SenseKnobs {
    /// Relative sigma of an enemy count estimate.
    pub obs_noise: f64,
    /// Share of members' sightings that reach the leader per tick.
    pub intel_share: f64,
    /// Seconds between d-scans by a scout in a contested system.
    pub scan_every_s: u64,
    /// Enemy groups remembered.
    pub enemy_memory: usize,
    /// Memory decay, days.
    pub half_life_days: f64,
    /// Confidence in memory with no scout.
    pub blind_conf: f64,
}

impl SenseKnobs {
    pub const EASY: SenseKnobs = SenseKnobs {
        obs_noise: 0.50,
        intel_share: 0.25,
        scan_every_s: 600,
        enemy_memory: 2,
        half_life_days: 4.0,
        blind_conf: 0.30,
    };
    pub const NORMAL: SenseKnobs = SenseKnobs {
        obs_noise: 0.25,
        intel_share: 0.50,
        scan_every_s: 300,
        enemy_memory: 4,
        half_life_days: 14.0,
        blind_conf: 0.50,
    };
    pub const HARD: SenseKnobs = SenseKnobs {
        obs_noise: 0.10,
        intel_share: 0.80,
        scan_every_s: 120,
        enemy_memory: 8,
        half_life_days: 21.0,
        blind_conf: 0.65,
    };
    pub const BRUTAL: SenseKnobs = SenseKnobs {
        obs_noise: 0.0,
        intel_share: 1.0,
        scan_every_s: 60,
        enemy_memory: 16,
        half_life_days: 30.0,
        blind_conf: 0.80,
    };

    /// Level 1 easy .. 4 brutal; 0 and 2 are normal (level 0 turns the
    /// doctrine layer off elsewhere).
    pub fn at(level: u8) -> SenseKnobs {
        match level {
            1 => Self::EASY,
            3 => Self::HARD,
            4.. => Self::BRUTAL,
            _ => Self::NORMAL,
        }
    }
}

impl Default for SenseKnobs {
    fn default() -> Self {
        Self::NORMAL
    }
}

// ---- dice ------------------------------------------------------------------

fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn hash(seed: u64, parts: &[u64]) -> u64 {
    parts.iter().fold(mix(seed), |h, p| mix(h ^ mix(*p)))
}

/// A draw in `[0, 1)` from the hash of the inputs.
fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1_u64 << 53) as f64
}

/// A draw from about `N(0, 1)` (Irwin-Hall: twelve uniforms less six),
/// deterministic in the inputs.
fn normal(seed: u64, parts: &[u64]) -> f64 {
    let mut h = hash(seed, parts);
    let mut sum = 0.0;
    for _ in 0..12 {
        h = mix(h);
        sum += unit(h);
    }
    sum - 6.0
}

// ---- what the senses give --------------------------------------------------

/// A ship on the grid, from its slim: exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridShip {
    pub hull: i64,
    pub group: Group,
}

/// The grid's pilots as sightings (`hunt::Pilot`: type and owner).
pub fn grid_of(pilots: &[crate::hunt::Pilot]) -> Vec<GridShip> {
    pilots
        .iter()
        .filter_map(|p| {
            Some(GridShip {
                hull: p.type_id?,
                group: society::group_of(&p.who),
            })
        })
        .collect()
}

/// Local's head count by owner group (the presences' corp and alliance).
pub fn local_counts(roster: &crate::chat::Roster) -> BTreeMap<Group, u32> {
    let mut out = BTreeMap::new();
    for m in roster.members() {
        let g = society::group_of(&Affiliation {
            character_id: m.char_id,
            corporation_id: m.corp,
            alliance_id: m.alliance,
        });
        *out.entry(g).or_insert(0) += 1;
    }
    out
}

/// **One member's view** of a system at a time.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sighting {
    /// The pilot's character id.
    pub observer: i64,
    pub system: i64,
    pub at_s: u64,
    /// The full-sphere d-scan: pilots' ships by hull, no owner. `None`: no
    /// fresh scan.
    pub scan: Option<BTreeMap<i64, u32>>,
    /// Ships on the observer's grid (the observer's own excluded).
    pub grid: Vec<GridShip>,
    /// Local's head count by owner group. `None`: no chat.
    pub local: Option<BTreeMap<Group, u32>>,
    /// An `EnemySpotted` flag: this report always reaches the leader.
    pub spotted: bool,
}

impl Sighting {
    /// Does the sighting show hulls (a scan or a grid)?
    pub fn sees_hulls(&self) -> bool {
        self.scan.is_some() || !self.grid.is_empty()
    }
}

/// My group's (and allies') ships in the system, by hull: what the hall's
/// roster knows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Strength {
    pub hulls: BTreeMap<i64, u32>,
}

impl Strength {
    pub fn ships(&self) -> u32 {
        self.hulls.values().sum()
    }
}

/// Where an enemy's number came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Slims on a grid: exact.
    Grid,
    /// The d-scan residual shared out by Local: an estimate.
    Scan,
    /// Remembered, not seen.
    Memory,
}

/// One enemy group in the picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Enemy {
    pub group: Group,
    /// `(hull type, ships)`, ascending by hull.
    pub hulls: Vec<(i64, u32)>,
    pub source: Source,
    /// 0..1: how far a leader may trust the row.
    pub conf: f64,
}

impl Enemy {
    pub fn ships(&self) -> u32 {
        self.hulls.iter().map(|h| h.1).sum()
    }
}

/// **The picture** (R-DOCTRINE 6.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub system: i64,
    pub at_s: u64,
    pub mine: Strength,
    pub theirs: Vec<Enemy>,
    /// 0..1, [`OBS_RELIABLE_SEEN`]; 0 when blind.
    pub reliability: f64,
    /// No shared sighting showed hulls: `theirs` is memory at
    /// [`SenseKnobs::blind_conf`].
    pub blind: bool,
    /// Sightings that reached the leader.
    pub observers: usize,
}

impl Picture {
    pub fn group(&self, g: Group) -> Option<&Enemy> {
        self.theirs.iter().find(|e| e.group == g)
    }

    /// All enemy ships.
    pub fn ships(&self) -> u32 {
        self.theirs.iter().map(Enemy::ships).sum()
    }
}

/// What [`observe`] needs besides the sightings.
pub struct Inputs<'a> {
    pub system: i64,
    pub now_s: u64,
    /// The hall tick number, so a noise draw is fixed within a tick.
    pub tick: u64,
    pub seed: u64,
    pub mine: &'a Strength,
    /// Is this group mine or an ally's (never counted as an enemy)?
    pub is_mine: &'a dyn Fn(Group) -> bool,
    pub memory: &'a Memory,
    pub knobs: SenseKnobs,
}

/// **Observe** (R-DOCTRINE 6.2): merge the members' sightings of `system`
/// into a picture.
///
/// 1. Only fresh sightings of this system count, and only
///    [`SenseKnobs::intel_share`] of them reach the leader per tick (a
///    hash of tick and observer decides; a `spotted` one always does).
/// 2. Per owner and hull, the grid count is the most any observer saw
///    (exact). Per hull, the scan count is the most any observer scanned.
/// 3. The residual of a hull is its scan minus mine minus the grid's count.
///    It is shared out by Local's head counts (not mine), clamped to the
///    residual, and each share gets noise `1 + obs_noise * N(0,1)`.
/// 4. With nothing to see the picture is `blind`: the memory, at
///    `blind_conf` of what was remembered.
pub fn observe(reports: &[Sighting], inp: &Inputs<'_>) -> Picture {
    let shared: Vec<&Sighting> = reports
        .iter()
        .filter(|r| r.system == inp.system && inp.now_s.saturating_sub(r.at_s) <= SIGHTING_STALE_S)
        .filter(|r| {
            r.spotted
                || unit(hash(inp.seed, &[inp.tick, r.observer as u64])) < inp.knobs.intel_share
        })
        .collect();
    let seers = shared.iter().filter(|r| r.sees_hulls()).count();
    if seers == 0 {
        return blind_picture(inp, shared.len());
    }

    // 2. Merge by max over observers.
    let mut grid: BTreeMap<(Group, i64), u32> = BTreeMap::new();
    let mut scan: BTreeMap<i64, u32> = BTreeMap::new();
    let mut local: BTreeMap<Group, u32> = BTreeMap::new();
    for r in &shared {
        let mut per: BTreeMap<(Group, i64), u32> = BTreeMap::new();
        for s in r.grid.iter().filter(|s| !(inp.is_mine)(s.group)) {
            *per.entry((s.group, s.hull)).or_insert(0) += 1;
        }
        for (k, n) in per {
            let e = grid.entry(k).or_insert(0);
            *e = (*e).max(n);
        }
        for (h, n) in r.scan.iter().flatten() {
            let e = scan.entry(*h).or_insert(0);
            *e = (*e).max(*n);
        }
        for (g, n) in r
            .local
            .iter()
            .flatten()
            .filter(|(g, _)| !(inp.is_mine)(**g))
        {
            let e = local.entry(*g).or_insert(0);
            *e = (*e).max(*n);
        }
    }

    // 3. Residuals shared by Local, noised.
    let mut grid_total: BTreeMap<i64, u32> = BTreeMap::new();
    for ((_, h), n) in &grid {
        *grid_total.entry(*h).or_insert(0) += n;
    }
    let local_all: f64 = local.values().map(|n| f64::from(*n)).sum();
    let mut rows: BTreeMap<Group, BTreeMap<i64, u32>> = BTreeMap::new();
    let mut estimated: BTreeSet<Group> = BTreeSet::new();
    for ((g, h), n) in &grid {
        *rows.entry(*g).or_default().entry(*h).or_insert(0) += n;
    }
    for (h, n) in &scan {
        let mine = inp.mine.hulls.get(h).copied().unwrap_or(0);
        let on_grid = grid_total.get(h).copied().unwrap_or(0);
        let residual = f64::from(n.saturating_sub(mine).saturating_sub(on_grid));
        if residual <= 0.0 {
            continue;
        }
        let shares: Vec<(Group, f64)> = if local_all > 0.0 {
            local
                .iter()
                .map(|(g, c)| {
                    (
                        *g,
                        (residual * f64::from(*c) / local_all).clamp(0.0, residual),
                    )
                })
                .collect()
        } else {
            vec![(UNATTRIBUTED, residual)]
        };
        for (g, raw) in shares {
            let z = normal(inp.seed, &[inp.tick, g as u64, *h as u64]);
            let est = (raw * (1.0 + inp.knobs.obs_noise * z)).round().max(0.0) as u32;
            if est > 0 {
                *rows.entry(g).or_default().entry(*h).or_insert(0) += est;
                estimated.insert(g);
            }
        }
    }

    // 5. Reliability: distinct hulls seen, one more per extra observer.
    let mut hulls_seen: BTreeSet<i64> = scan.keys().copied().collect();
    hulls_seen.extend(grid.keys().map(|k| k.1));
    let seen = hulls_seen.len() as f64 + (seers as f64 - 1.0);
    let reliability = (seen / OBS_RELIABLE_SEEN).clamp(0.0, 1.0);

    let theirs = rows
        .into_iter()
        .map(|(group, h)| {
            let exact = !estimated.contains(&group);
            Enemy {
                group,
                hulls: h.into_iter().collect(),
                source: if exact { Source::Grid } else { Source::Scan },
                conf: if exact { 1.0 } else { reliability },
            }
        })
        .collect();
    Picture {
        system: inp.system,
        at_s: inp.now_s,
        mine: inp.mine.clone(),
        theirs,
        reliability,
        blind: false,
        observers: shared.len(),
    }
}

/// A leader with no scout: the memory only, at `blind_conf` of what was
/// remembered (6.2 rule 5).
fn blind_picture(inp: &Inputs<'_>, observers: usize) -> Picture {
    let theirs = inp
        .memory
        .groups
        .values()
        .filter(|m| !(inp.is_mine)(m.group))
        .filter_map(|m| {
            let hulls: Vec<(i64, u32)> = m
                .seen
                .iter()
                .map(|(h, s)| (*h, s.strength(inp.now_s, &inp.knobs).round() as u32))
                .filter(|(_, n)| *n > 0)
                .collect();
            (!hulls.is_empty()).then_some(Enemy {
                group: m.group,
                hulls,
                source: Source::Memory,
                conf: m.conf * inp.knobs.blind_conf,
            })
        })
        .collect();
    Picture {
        system: inp.system,
        at_s: inp.now_s,
        mine: inp.mine.clone(),
        theirs,
        reliability: 0.0,
        blind: true,
        observers,
    }
}

// ---- damage taken ----------------------------------------------------------

/// The damage types in [`DamageTally::by_type`]'s order: attributes 114,
/// 116, 117, 118 **[V]** (`damage::attr`).
pub const TYPE_ATTRS: [i64; 4] = [
    attr::EM_DAMAGE,
    attr::EXPLOSIVE_DAMAGE,
    attr::KINETIC_DAMAGE,
    attr::THERMAL_DAMAGE,
];
pub const TYPE_NAMES: [&str; 4] = ["em", "explosive", "kinetic", "thermal"];

/// One `OnDamageMessage`, read.
#[derive(Debug, Clone, PartialEq)]
pub struct DamageHit {
    /// `false`: my own shot (`attackType 'me'`).
    pub incoming: bool,
    /// The attacker's ball (`attackerID`).
    pub attacker: Option<i64>,
    /// A weapon **type** id.
    pub weapon: Option<i64>,
    /// `damage` after resistances (the client's number).
    pub damage: f64,
    /// `hitQuality`; 0 is a miss.
    pub quality: i64,
    /// The `damageAttributes` split, in [`TYPE_ATTRS`] order. `None` when
    /// the message does not carry it.
    pub by_type: Option<[f64; 4]>,
}

fn field<'a>(v: &'a PyValue, key: &str) -> Option<&'a PyValue> {
    eve_server::pyutil::dict_get(crate::attributes(v), key.as_bytes())
}

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        _ => None,
    }
}

fn real(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Real(r) => Some(*r),
        PyValue::Int(n) => Some(*n as f64),
        _ => None,
    }
}

fn text(v: Option<&PyValue>) -> Option<String> {
    match v? {
        PyValue::Bytes(b) => String::from_utf8(b.clone()).ok(),
        PyValue::Unicode(s) => Some(s.clone()),
        _ => None,
    }
}

/// **Read an `OnDamageMessage`** (`uilog.py` `AddCombatMessageFromDict`
/// **[V]**): `attackType` defaults to `'me'`; `damageAttributes` is
/// `{attributeID: amount}`; `weapon` a type id. Whatever is missing stays
/// `None`; a dict with no `damage` is no message.
pub fn parse_damage(args: &[PyValue]) -> Option<DamageHit> {
    let d = args.first()?;
    let kind = text(field(d, "attackType")).unwrap_or_else(|| "me".to_owned());
    let by_type = match field(d, "damageAttributes") {
        Some(PyValue::Dict(entries)) => {
            let mut out = [0.0; 4];
            let mut any = false;
            for (amount, key) in entries {
                if let (PyValue::Int(k), Some(a)) = (key, real(Some(amount)))
                    && let Some(i) = TYPE_ATTRS.iter().position(|t| t == k)
                {
                    out[i] = a.max(0.0);
                    any = true;
                }
            }
            any.then_some(out)
        }
        _ => None,
    };
    Some(DamageHit {
        incoming: kind != "me",
        attacker: int(field(d, "attackerID")),
        weapon: int(field(d, "weapon")),
        damage: real(field(d, "damage"))?,
        quality: int(field(d, "hitQuality")).unwrap_or(0),
        by_type,
    })
}

/// **Damage taken, by type** (feeds alternate ammo and tank swaps): the
/// split of every landed hit, and by weapon type.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DamageTally {
    /// Em, explosive, kinetic, thermal: the hit's damage shared out by the
    /// split it carried.
    pub by_type: [f64; 4],
    /// Damage of hits that carried no split.
    pub untyped: f64,
    pub by_weapon: BTreeMap<i64, f64>,
    pub hits: u32,
}

impl DamageTally {
    /// Count a hit (misses and my own shots are not damage taken).
    pub fn record(&mut self, h: &DamageHit) {
        if !h.incoming || h.quality == 0 || h.damage <= 0.0 {
            return;
        }
        self.hits += 1;
        match h.by_type {
            Some(split) if split.iter().sum::<f64>() > 0.0 => {
                let total: f64 = split.iter().sum();
                for (slot, part) in self.by_type.iter_mut().zip(split) {
                    *slot += h.damage * part / total;
                }
            }
            _ => self.untyped += h.damage,
        }
        if let Some(w) = h.weapon
            && (self.by_weapon.len() < MAX_ATTACKERS || self.by_weapon.contains_key(&w))
        {
            *self.by_weapon.entry(w).or_insert(0.0) += h.damage;
        }
    }

    /// The typed damage as shares summing to 1; `None` with none typed.
    pub fn share(&self) -> Option<[f64; 4]> {
        let total: f64 = self.by_type.iter().sum();
        (total > 0.0).then(|| self.by_type.map(|x| x / total))
    }

    /// Index into [`TYPE_NAMES`] of the type dealing most.
    pub fn dominant(&self) -> Option<usize> {
        self.share()?
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(&a.0)))
            .map(|(i, _)| i)
    }

    /// Age the tally (`k` in 0..1 per step), so old fights fade.
    pub fn decay(&mut self, k: f64) {
        for x in &mut self.by_type {
            *x *= k;
        }
        self.untyped *= k;
        for x in self.by_weapon.values_mut() {
            *x *= k;
        }
    }
}

// ---- killmails -------------------------------------------------------------

/// One attacker on a kill report: the group, hull and weapon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttackerEvidence {
    pub group: Group,
    pub hull: Option<i64>,
    pub weapon: Option<i64>,
}

/// One kill report, read for what it says of fits.
#[derive(Debug, Clone, PartialEq)]
pub struct KillEvidence {
    pub kill_id: i64,
    pub victim_group: Group,
    pub victim_hull: i64,
    /// Every attacker with a pilot (an NPC has none and is not a group).
    pub attackers: Vec<AttackerEvidence>,
    /// The victim's items from `killBlob`: `(type, flag)`, contents too.
    pub items: Vec<(i64, i64)>,
}

/// The items in a `killBlob` (`<i t=".." f=".." ..>` tags inside
/// `<items>`), as `(type, flag)`. No XML library: the server writes
/// exactly this shape (`Report::blob`). Bounded.
pub fn blob_items(blob: &[u8]) -> Vec<(i64, i64)> {
    const MAX_ITEMS: usize = 512;
    let s = String::from_utf8_lossy(blob);
    let Some(items) = s.split("<items>").nth(1) else {
        return Vec::new();
    };
    let num = |tag: &str, key: &str| -> Option<i64> {
        let at = tag.find(&format!("{key}=\""))? + key.len() + 2;
        tag[at..].split('"').next()?.parse().ok()
    };
    items
        .split("<i ")
        .skip(1)
        .take(MAX_ITEMS)
        .filter_map(|tag| {
            let tag = tag.split('>').next()?;
            Some((num(tag, "t")?, num(tag, "f")?))
        })
        .collect()
}

/// **A kill row as evidence** (`GetRecentShipKillsAndLosses` /
/// `GetKillMail` row; `attackers` is REL-FIX's list, the final blow
/// first). `None` for a victim with no pilot (an NPC) or no ship type.
pub fn kill_evidence(row: &PyValue) -> Option<KillEvidence> {
    let victim = int(field(row, "victimCharacterID")).filter(|c| *c > 0)?;
    let aff = |c, corp, al| Affiliation {
        character_id: c,
        corporation_id: corp,
        alliance_id: al,
    };
    let victim_group = society::group_of(&aff(
        victim,
        int(field(row, "victimCorporationID")).unwrap_or(0),
        int(field(row, "victimAllianceID")).filter(|a| *a > 0),
    ));
    let mut attackers = Vec::new();
    if let Some(PyValue::List(list)) = field(row, "attackers") {
        for a in list.iter().take(MAX_EVIDENCE) {
            let Some(c) = int(field(a, "characterID")).filter(|c| *c > 0) else {
                continue;
            };
            attackers.push(AttackerEvidence {
                group: society::group_of(&aff(
                    c,
                    int(field(a, "corporationID")).unwrap_or(0),
                    int(field(a, "allianceID")).filter(|x| *x > 0),
                )),
                hull: int(field(a, "shipTypeID")).filter(|t| *t > 0),
                weapon: int(field(a, "weaponTypeID")).filter(|t| *t > 0),
            });
        }
    }
    let items = match field(row, "killBlob") {
        Some(PyValue::Bytes(b)) => blob_items(b),
        _ => Vec::new(),
    };
    Some(KillEvidence {
        kill_id: int(field(row, "killID"))?,
        victim_group,
        victim_hull: int(field(row, "victimShipTypeID"))?,
        attackers,
        items,
    })
}

/// The slot flags of modules: low 11..=18, mid 19..=26, high 27..=34 **[V]**
/// (`const.flagLoSlot0`..`flagHiSlot7`).
pub const MODULE_FLAGS: std::ops::RangeInclusive<i64> = 11..=34;

/// **A utility a killmail fit showed** (BP-COUNTER): what the leader reads
/// off an enemy hull beyond its guns and tank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Util {
    Point,
    Scram,
    Bomb,
    Mjd,
    Track,
    Guide,
    Ecm,
    Damp,
    Bubble,
    Cloak,
}

/// **What the doctrine layer knows about types**: hull classes and part
/// kinds, built from a [`Library`] (build it over `Makeable::all()` to
/// recognise parts the world cannot make but an enemy may carry).
#[derive(Debug, Clone, Default)]
pub struct Lens {
    pub hull_class: BTreeMap<i64, Class>,
    pub module_kind: BTreeMap<i64, Kind>,
}

impl Lens {
    pub fn of_library(lib: &Library<'_>) -> Lens {
        Lens {
            hull_class: lib.hulls.iter().map(|h| (h.type_id, h.class)).collect(),
            module_kind: lib.parts.iter().map(|p| (p.type_id, p.kind)).collect(),
        }
    }

    /// The weapon family a type (a gun, or a launcher) belongs to.
    pub fn family_of(&self, weapon: i64) -> Option<Family> {
        match self.module_kind.get(&weapon)? {
            Kind::Gun(f, _) => Some(*f),
            _ => None,
        }
    }

    /// BP-COUNTER: the utility a module gives, if it gives one the counter
    /// rules read. A web, a painter, a neut and the tank have none; a
    /// bubble is not a part kind, so [`Util::Bubble`] is never read off a fit.
    pub fn util_of(&self, module: i64) -> Option<Util> {
        Some(match self.module_kind.get(&module)? {
            Kind::Disruptor => Util::Point,
            Kind::Scram => Util::Scram,
            Kind::Smartbomb(_) => Util::Bomb,
            Kind::Mjd => Util::Mjd,
            Kind::TrackDisrupt => Util::Track,
            Kind::GuideDisrupt => Util::Guide,
            Kind::Ecm => Util::Ecm,
            Kind::Damp => Util::Damp,
            Kind::Cloak => Util::Cloak,
            _ => return None,
        })
    }

    /// The tank layer a module adds to, if it is tank.
    pub fn layer_of(&self, module: i64) -> Option<Layer> {
        match self.module_kind.get(&module)? {
            Kind::ShieldExtender | Kind::ShieldBooster => Some(Layer::Shield),
            Kind::Plate | Kind::ArmourRepairer => Some(Layer::Armour),
            Kind::Hardener(l) => Some(*l),
            _ => None,
        }
    }
}

/// **The best ewar numbers the catalog knows**, field by field (BP-COUNTER):
/// what a utility seen on a killmail is given when the inferred doctrine does
/// not carry it. The catalog's own parts, so nothing is invented.
pub fn reference_ewar(catalog: &[Doctrine]) -> crate::doctrine::Ewar {
    let mut r = crate::doctrine::Ewar::default();
    let up = |a: &mut f64, b: f64| *a = a.max(b);
    for (_, _, f) in catalog.iter().flat_map(|d| d.roles.iter()) {
        let e = &f.profile.ewar;
        up(&mut r.pin_range, e.pin_range);
        up(&mut r.point_range, e.point_range);
        up(&mut r.damp, e.damp);
        up(&mut r.damp_range, e.damp_range);
        for k in 0..4 {
            up(&mut r.ecm[k], e.ecm[k]);
            up(&mut r.bomb[k], e.bomb[k]);
        }
        up(&mut r.ecm_range, e.ecm_range);
        up(&mut r.ecm_falloff, e.ecm_falloff);
        up(&mut r.track_cut, e.track_cut);
        up(&mut r.opt_cut, e.opt_cut);
        up(&mut r.track_range, e.track_range);
        up(&mut r.track_falloff, e.track_falloff);
        up(&mut r.guide_cut, e.guide_cut);
        up(&mut r.guide_range, e.guide_range);
        up(&mut r.guide_falloff, e.guide_falloff);
        up(&mut r.bomb_range, e.bomb_range);
    }
    r
}

/// **The doctrine as the enemy has been seen to fly it** (BP-COUNTER): a clone
/// of the inferred `d` whose fits also carry the utility `util` shows, with
/// the numbers of `reference`. `read` in `[0, 1]` is how much of it the
/// leader believes: it scales the strengths (a bomb volley, a cut, a jam)
/// and `0` returns `d` unchanged. A number the fit already has is kept when
/// larger. A bubble and a cloak change who can leave and who is engaged, not
/// the damage, so the exchange has no term for them (the report's §5).
pub fn apply_utility(
    d: &Doctrine,
    util: &BTreeMap<Util, u32>,
    reference: &crate::doctrine::Ewar,
    read: f64,
) -> Doctrine {
    let r = read.clamp(0.0, 1.0);
    let mut out = d.clone();
    if r <= 0.0 {
        return out;
    }
    for (_, _, f) in &mut out.roles {
        let e = &mut f.profile.ewar;
        let up = |a: &mut f64, b: f64| *a = a.max(b);
        for (u, n) in util {
            if *n == 0 {
                continue;
            }
            match u {
                Util::Point => up(&mut e.point_range, reference.point_range),
                Util::Scram => {
                    e.scram = true;
                    up(&mut e.pin_range, reference.pin_range);
                }
                Util::Bomb => {
                    for k in 0..4 {
                        up(&mut e.bomb[k], reference.bomb[k] * r);
                    }
                    up(&mut e.bomb_range, reference.bomb_range);
                }
                Util::Mjd => f.profile.mjd = true,
                Util::Track => {
                    up(&mut e.track_cut, reference.track_cut * r);
                    up(&mut e.opt_cut, reference.opt_cut * r);
                    up(&mut e.track_range, reference.track_range);
                    up(&mut e.track_falloff, reference.track_falloff);
                }
                Util::Guide => {
                    up(&mut e.guide_cut, reference.guide_cut * r);
                    up(&mut e.guide_range, reference.guide_range);
                    up(&mut e.guide_falloff, reference.guide_falloff);
                }
                Util::Ecm => {
                    for k in 0..4 {
                        up(&mut e.ecm[k], reference.ecm[k] * r);
                    }
                    up(&mut e.ecm_range, reference.ecm_range);
                    up(&mut e.ecm_falloff, reference.ecm_falloff);
                }
                Util::Damp => {
                    up(&mut e.damp, reference.damp * r);
                    up(&mut e.damp_range, reference.damp_range);
                }
                Util::Bubble | Util::Cloak => {}
            }
        }
    }
    out
}

/// A fit read off a killmail.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FitSeen {
    pub weapons: BTreeSet<i64>,
    pub tanks: BTreeMap<Layer, u32>,
    pub utility: BTreeMap<Util, u32>,
}

/// **The victim's fit** from the items it died with: its weapons and the
/// tank layers its modules add to. Only module slots count (a hold's
/// contents are not a fit).
pub fn fit_of(ev: &KillEvidence, lens: &Lens) -> FitSeen {
    let mut out = FitSeen::default();
    for (t, flag) in &ev.items {
        if !MODULE_FLAGS.contains(flag) {
            continue;
        }
        if lens.family_of(*t).is_some() {
            out.weapons.insert(*t);
        }
        if let Some(l) = lens.layer_of(*t) {
            *out.tanks.entry(l).or_insert(0) += 1;
        }
        if let Some(u) = lens.util_of(*t) {
            *out.utility.entry(u).or_insert(0) += 1;
        }
    }
    out
}

// ---- memory ----------------------------------------------------------------

/// What was seen of one hull of a group.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Seen {
    /// The most ships seen at once (as of `last_s`).
    pub n_max: u32,
    pub last_s: u64,
    pub weapons: BTreeSet<i64>,
    pub tanks: BTreeMap<Layer, u32>,
    /// BP-COUNTER: the utility modules a killmail showed on this hull, the
    /// most of each on one ship.
    pub utility: BTreeMap<Util, u32>,
}

impl Seen {
    /// The ships still believed in at `now_s`: `n_max` halved every
    /// `half_life_days`.
    pub fn strength(&self, now_s: u64, k: &SenseKnobs) -> f64 {
        let age_days = now_s.saturating_sub(self.last_s) as f64 / DAY_S;
        f64::from(self.n_max) * 0.5_f64.powf(age_days / k.half_life_days.max(1e-9))
    }
}

/// **One enemy group** as remembered.
#[derive(Debug, Clone, PartialEq)]
pub struct EnemyMemory {
    pub group: Group,
    pub seen: BTreeMap<i64, Seen>,
    /// The confidence of the last picture that showed it.
    pub conf: f64,
    /// The damage types it has dealt to us (the split of what hit).
    pub damage: [f64; 4],
    pub last_battle_s: u64,
}

impl EnemyMemory {
    pub fn new(group: Group) -> EnemyMemory {
        EnemyMemory {
            group,
            seen: BTreeMap::new(),
            conf: 0.0,
            damage: [0.0; 4],
            last_battle_s: 0,
        }
    }

    fn see(&mut self, hull: i64, n: u32, now_s: u64, k: &SenseKnobs) -> &mut Seen {
        let s = self.seen.entry(hull).or_default();
        let held = s.strength(now_s, k);
        s.n_max = f64::from(n).max(held).ceil() as u32;
        s.last_s = now_s;
        s
    }
}

/// **The enemy memory** (R-DOCTRINE 5): per group, bounded by
/// [`SenseKnobs::enemy_memory`], decaying with its half-life.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Memory {
    pub groups: BTreeMap<Group, EnemyMemory>,
}

impl Memory {
    /// A picture seen: the counts, and the confidence.
    pub fn note_picture(&mut self, p: &Picture, k: &SenseKnobs) {
        if p.blind {
            return;
        }
        for e in &p.theirs {
            if e.group == UNATTRIBUTED {
                continue;
            }
            let m = self
                .groups
                .entry(e.group)
                .or_insert_with(|| EnemyMemory::new(e.group));
            for (h, n) in &e.hulls {
                m.see(*h, *n, p.at_s, k);
            }
            m.conf = e.conf;
            m.last_battle_s = p.at_s;
        }
        self.prune(p.at_s, k);
    }

    /// A kill report: the victim's hull, weapons and tank, and each
    /// attacker's hull and weapon (groups that are not mine).
    pub fn note_kill(
        &mut self,
        ev: &KillEvidence,
        lens: &Lens,
        now_s: u64,
        is_mine: &dyn Fn(Group) -> bool,
        k: &SenseKnobs,
    ) {
        if !is_mine(ev.victim_group) {
            let fit = fit_of(ev, lens);
            let m = self
                .groups
                .entry(ev.victim_group)
                .or_insert_with(|| EnemyMemory::new(ev.victim_group));
            let s = m.see(ev.victim_hull, 1, now_s, k);
            s.weapons.extend(fit.weapons);
            for (l, n) in fit.tanks {
                let e = s.tanks.entry(l).or_insert(0);
                *e = (*e).max(n);
            }
            for (u, n) in fit.utility {
                let e = s.utility.entry(u).or_insert(0);
                *e = (*e).max(n);
            }
            m.last_battle_s = now_s;
            m.conf = m.conf.max(KILL_CONF);
        }
        for a in ev.attackers.iter().filter(|a| !is_mine(a.group)) {
            let m = self
                .groups
                .entry(a.group)
                .or_insert_with(|| EnemyMemory::new(a.group));
            if let Some(h) = a.hull {
                let s = m.see(h, 1, now_s, k);
                s.weapons.extend(a.weapon);
            }
            m.last_battle_s = now_s;
            m.conf = m.conf.max(KILL_CONF);
        }
        self.prune(now_s, k);
    }

    /// Damage a group dealt to us (from a [`DamageTally`] of its ships).
    pub fn note_damage(&mut self, group: Group, t: &DamageTally, now_s: u64) {
        let m = self
            .groups
            .entry(group)
            .or_insert_with(|| EnemyMemory::new(group));
        for (slot, x) in m.damage.iter_mut().zip(t.by_type) {
            *slot += x;
        }
        m.last_battle_s = m.last_battle_s.max(now_s);
    }

    /// Forget what has decayed below [`MEMORY_FLOOR`], empty groups, and
    /// all but the [`SenseKnobs::enemy_memory`] most recent groups.
    pub fn prune(&mut self, now_s: u64, k: &SenseKnobs) {
        for m in self.groups.values_mut() {
            m.seen.retain(|_, s| s.strength(now_s, k) >= MEMORY_FLOOR);
        }
        self.groups.retain(|_, m| !m.seen.is_empty());
        while self.groups.len() > k.enemy_memory {
            let Some(oldest) = self
                .groups
                .values()
                .min_by_key(|m| (m.last_battle_s, m.group))
                .map(|m| m.group)
            else {
                break;
            };
            self.groups.remove(&oldest);
        }
    }
}

/// **An enemy group's likely doctrine** (R-DOCTRINE 5): the nearest
/// doctrine cells to what was seen, by cosine on the `(hull class, weapon
/// family, tank layer)` vector, as `(key, share)` with shares summing to 1,
/// best first, at most [`INFER_TOP`]; and `conf`, the share of the ships
/// (decayed) whose hull the lens knows and the catalog has a cell for.
///
/// A hull with no weapon seen matches every family of its class; with no
/// tank seen, every layer. Several doctrines in one cell share it equally
/// (range and stance are not observable from a kill).
pub fn infer(
    m: &EnemyMemory,
    now_s: u64,
    knobs: &SenseKnobs,
    lens: &Lens,
    catalog: &[Doctrine],
) -> (Vec<(String, f64)>, f64) {
    type Cell = (Class, Family, Layer);
    let cells: BTreeSet<Cell> = catalog
        .iter()
        .map(|d| (d.class, d.family, d.layer))
        .collect();
    let mut v: BTreeMap<Cell, f64> = BTreeMap::new();
    let (mut matched, mut all) = (0.0, 0.0);
    for (hull, s) in &m.seen {
        let w = s.strength(now_s, knobs);
        all += w;
        let Some(class) = lens.hull_class.get(hull).copied() else {
            continue;
        };
        let fams: BTreeSet<Family> = s
            .weapons
            .iter()
            .filter_map(|w| lens.family_of(*w))
            .collect();
        let layer_w = |l: Layer| -> f64 {
            if s.tanks.is_empty() {
                1.0
            } else {
                f64::from(s.tanks.get(&l).copied().unwrap_or(0))
            }
        };
        let fit: Vec<(Cell, f64)> = cells
            .iter()
            .filter(|c| c.0 == class && (fams.is_empty() || fams.contains(&c.1)))
            .map(|c| (*c, layer_w(c.2)))
            .filter(|(_, lw)| *lw > 0.0)
            .collect();
        let total: f64 = fit.iter().map(|(_, lw)| lw).sum();
        if total <= 0.0 {
            continue;
        }
        matched += w;
        for (c, lw) in fit {
            *v.entry(c).or_insert(0.0) += w * lw / total;
        }
    }
    let norm = v.values().map(|x| x * x).sum::<f64>().sqrt();
    if norm <= 0.0 {
        return (Vec::new(), 0.0);
    }
    let mut scored: Vec<(String, f64)> = catalog
        .iter()
        .filter_map(|d| {
            let c = v.get(&(d.class, d.family, d.layer)).copied()?;
            Some((d.key.clone(), c / norm))
        })
        .collect();
    let sum: f64 = scored.iter().map(|x| x.1).sum();
    for x in &mut scored {
        x.1 /= sum;
    }
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.truncate(INFER_TOP);
    (scored, if all > 0.0 { matched / all } else { 0.0 })
}

// ---- a bot's own sense, and the board the leader reads ---------------------

/// **One bot's accumulated sense**: the damage it took by attacker group
/// (`None`: an NPC or an unknown ball), the kill reports it read.
#[derive(Debug, Clone, Default)]
pub struct Sensor {
    pub incoming: BTreeMap<Option<Group>, DamageTally>,
    /// Kill reports read, oldest first, bounded; ids seen are not repeated.
    pub kills: VecDeque<KillEvidence>,
    /// Evidence not yet handed to the [`Board`].
    fresh: Vec<KillEvidence>,
}

impl Sensor {
    /// A damage message arrived; `group` is the attacker ball's owner when
    /// the park knows it.
    pub fn on_damage(&mut self, hit: &DamageHit, group: Option<Group>) {
        if !hit.incoming {
            return;
        }
        if self.incoming.len() >= MAX_ATTACKERS && !self.incoming.contains_key(&group) {
            return;
        }
        self.incoming.entry(group).or_default().record(hit);
    }

    /// Kill rows read: new reports are kept once and queued for the board.
    pub fn on_kill_rows(&mut self, rows: &[PyValue]) {
        for ev in rows.iter().filter_map(kill_evidence) {
            if self.kills.iter().any(|k| k.kill_id == ev.kill_id) {
                continue;
            }
            if self.kills.len() >= MAX_EVIDENCE {
                self.kills.pop_front();
            }
            self.kills.push_back(ev.clone());
            self.fresh.push(ev);
        }
    }

    /// The evidence read since the last call.
    pub fn take_fresh(&mut self) -> Vec<KillEvidence> {
        std::mem::take(&mut self.fresh)
    }

    /// The damage taken, every attacker summed.
    pub fn taken(&self) -> DamageTally {
        let mut out = DamageTally::default();
        for t in self.incoming.values() {
            for (a, b) in out.by_type.iter_mut().zip(t.by_type) {
                *a += b;
            }
            out.untyped += t.untyped;
            out.hits += t.hits;
            for (w, x) in &t.by_weapon {
                *out.by_weapon.entry(*w).or_insert(0.0) += x;
            }
        }
        out
    }
}

/// **Where members post their sightings and the leader reads the picture**
/// (shared, `Arc<Board>` in `driver::Society::board`). The leader (DOC-4)
/// calls [`Board::sightings`] and [`observe`]; nothing does yet.
#[derive(Debug, Default)]
pub struct Board {
    inner: Mutex<BoardInner>,
}

#[derive(Debug, Default)]
struct BoardInner {
    sightings: BTreeMap<i64, Sighting>,
    evidence: VecDeque<KillEvidence>,
}

impl Board {
    fn lock(&self) -> std::sync::MutexGuard<'_, BoardInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A member's latest sighting replaces its last.
    pub fn post(&self, s: Sighting) {
        let mut b = self.lock();
        if b.sightings.len() >= MAX_SIGHTINGS && !b.sightings.contains_key(&s.observer) {
            return;
        }
        b.sightings.insert(s.observer, s);
    }

    /// Kill evidence a member read (each report kept once).
    pub fn post_kills(&self, kills: Vec<KillEvidence>) {
        let mut b = self.lock();
        for ev in kills {
            if b.evidence.iter().any(|k| k.kill_id == ev.kill_id) {
                continue;
            }
            if b.evidence.len() >= MAX_EVIDENCE {
                b.evidence.pop_front();
            }
            b.evidence.push_back(ev);
        }
    }

    /// Fresh sightings of `system`.
    pub fn sightings(&self, system: i64, now_s: u64) -> Vec<Sighting> {
        self.lock()
            .sightings
            .values()
            .filter(|s| s.system == system && now_s.saturating_sub(s.at_s) <= SIGHTING_STALE_S)
            .cloned()
            .collect()
    }

    /// The kill evidence held, oldest first.
    pub fn evidence(&self) -> Vec<KillEvidence> {
        self.lock().evidence.iter().cloned().collect()
    }

    /// Drop a member's sighting (it logged off or left).
    pub fn forget(&self, observer: i64) {
        self.lock().sightings.remove(&observer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctrine::testkit;

    // Invented ids; the groups sit in the player-corp range so that
    // `group_of` names the corp.
    const SYS: i64 = 970_500_001;
    const H_FRIG: i64 = 970_510_001;
    const H_CRUI: i64 = 970_510_002;
    const H_BATT: i64 = 970_510_003;
    const G_ME: Group = 98_000_001;
    const G_A: Group = 98_000_002;
    const G_B: Group = 98_000_003;
    const SEED: u64 = 970_000_000;
    const NOW: u64 = 10_000_000;

    fn is_mine(g: Group) -> bool {
        g == G_ME
    }

    fn scan(rows: &[(i64, u32)]) -> BTreeMap<i64, u32> {
        rows.iter().copied().collect()
    }

    fn sighting(observer: i64, s: Option<&[(i64, u32)]>) -> Sighting {
        Sighting {
            observer,
            system: SYS,
            at_s: NOW,
            scan: s.map(scan),
            ..Sighting::default()
        }
    }

    fn inputs<'a>(
        mine: &'a Strength,
        memory: &'a Memory,
        knobs: SenseKnobs,
        tick: u64,
    ) -> Inputs<'a> {
        Inputs {
            system: SYS,
            now_s: NOW,
            tick,
            seed: SEED,
            mine,
            is_mine: &is_mine,
            memory,
            knobs,
        }
    }

    /// The knobs with the dice off, so a count is exactly the arithmetic.
    fn exact() -> SenseKnobs {
        SenseKnobs {
            obs_noise: 0.0,
            intel_share: 1.0,
            ..SenseKnobs::NORMAL
        }
    }

    fn lens() -> Lens {
        let mut l = Lens::default();
        l.hull_class.insert(H_FRIG, Class::Frigate);
        l.hull_class.insert(H_CRUI, Class::Cruiser);
        l.hull_class.insert(H_BATT, Class::Battleship);
        l.module_kind
            .insert(970_530_001, Kind::Gun(Family::Hybrid, 2));
        l.module_kind
            .insert(970_530_002, Kind::Gun(Family::Projectile, 2));
        l.module_kind.insert(970_530_003, Kind::Plate);
        l.module_kind.insert(970_530_004, Kind::ShieldExtender);
        l
    }

    #[test]
    fn d_scan_counts_minus_mine_is_the_residual() {
        let mine = Strength {
            hulls: scan(&[(H_CRUI, 4)]),
        };
        let mem = Memory::default();
        let r = [sighting(1, Some(&[(H_CRUI, 10), (H_FRIG, 3)]))];
        let p = observe(&r, &inputs(&mine, &mem, exact(), 1));
        assert!(!p.blind);
        // No Local: the whole residual is one unattributed group.
        let e = p.group(UNATTRIBUTED).expect("a group");
        assert_eq!(e.hulls, vec![(H_FRIG, 3), (H_CRUI, 6)], "10 - 4 mine");
        assert_eq!(e.source, Source::Scan);
        // A hull entirely mine is no enemy.
        let r = [sighting(1, Some(&[(H_CRUI, 4)]))];
        let p = observe(&r, &inputs(&mine, &mem, exact(), 1));
        assert_eq!(p.ships(), 0);
    }

    #[test]
    fn the_grid_is_exact_and_the_scan_keeps_only_the_rest() {
        let mine = Strength::default();
        let mem = Memory::default();
        let mut s = sighting(1, Some(&[(H_CRUI, 10)]));
        s.grid = vec![
            GridShip {
                hull: H_CRUI,
                group: G_A,
            };
            3
        ];
        s.grid.push(GridShip {
            hull: H_CRUI,
            group: G_ME,
        });
        let p = observe(&[s], &inputs(&mine, &mem, exact(), 1));
        let a = p.group(G_A).expect("A on the grid");
        assert_eq!(a.hulls, vec![(H_CRUI, 3)]);
        assert_eq!(a.source, Source::Grid, "exact");
        assert_eq!(a.conf, 1.0);
        assert!(p.group(G_ME).is_none(), "mine is not an enemy");
        // 10 scanned, 3 seen on the grid: 7 more, unattributed.
        assert_eq!(p.group(UNATTRIBUTED).unwrap().hulls, vec![(H_CRUI, 7)]);
    }

    #[test]
    fn owner_shares_follow_local() {
        let mine = Strength::default();
        let mem = Memory::default();
        let mut s = sighting(1, Some(&[(H_BATT, 12)]));
        // A has three heads in Local to B's one; mine is not counted.
        s.local = Some([(G_A, 3), (G_B, 1), (G_ME, 40)].into_iter().collect());
        let p = observe(&[s], &inputs(&mine, &mem, exact(), 1));
        assert_eq!(p.group(G_A).unwrap().hulls, vec![(H_BATT, 9)]);
        assert_eq!(p.group(G_B).unwrap().hulls, vec![(H_BATT, 3)]);
        assert!(p.group(G_ME).is_none());
        assert!(p.ships() <= 12, "never more than the residual");
    }

    #[test]
    fn a_killmail_gives_the_enemy_fit() {
        let blob = "<attackers></attackers><items>\
             <i t=\"970530001\" f=\"27\" s=\"1\" d=\"0\" x=\"1\"><i t=\"970540001\" f=\"27\" s=\"0\" d=\"0\" x=\"1\"></i></i>\
             <i t=\"970530003\" f=\"11\" s=\"1\" d=\"1\" x=\"0\"></i>\
             <i t=\"970530003\" f=\"12\" s=\"1\" d=\"0\" x=\"1\"></i>\
             <i t=\"970530002\" f=\"5\" s=\"1\" d=\"0\" x=\"1\"></i>\
             </items>";
        let attacker = |c: i64, corp: i64, hull: i64, weapon: i64| {
            eve_server::pyutil::pydict(vec![
                ("characterID", PyValue::Int(c)),
                ("corporationID", PyValue::Int(corp)),
                ("allianceID", PyValue::None),
                ("shipTypeID", PyValue::Int(hull)),
                ("weaponTypeID", PyValue::Int(weapon)),
            ])
        };
        let row = eve_server::pyutil::pydict(vec![
            ("killID", PyValue::Int(970_550_001)),
            ("victimCharacterID", PyValue::Int(970_560_001)),
            ("victimCorporationID", PyValue::Int(G_A)),
            ("victimAllianceID", PyValue::None),
            ("victimShipTypeID", PyValue::Int(H_CRUI)),
            (
                "attackers",
                PyValue::List(vec![
                    attacker(970_560_002, G_ME, 970_510_009, 970_530_002),
                    // An NPC has no character and is no group.
                    attacker(0, 1_000_001, 970_510_010, 970_530_002),
                ]),
            ),
            ("killBlob", PyValue::Bytes(blob.as_bytes().to_vec())),
        ]);
        let ev = kill_evidence(&row).expect("a pilot died");
        assert_eq!(ev.victim_group, G_A, "the corp (a player corp is a group)");
        assert_eq!(ev.attackers.len(), 1, "the NPC is skipped");
        assert_eq!(ev.items.len(), 5, "contents too");
        let fit = fit_of(&ev, &lens());
        assert_eq!(
            fit.weapons,
            BTreeSet::from([970_530_001]),
            "the hold's gun is no fit"
        );
        assert_eq!(fit.tanks.get(&Layer::Armour), Some(&2));
        assert_eq!(fit.tanks.get(&Layer::Shield), None);

        let mut mem = Memory::default();
        let k = SenseKnobs::NORMAL;
        mem.note_kill(&ev, &lens(), NOW, &is_mine, &k);
        let m = mem.groups.get(&G_A).expect("the victim's group");
        let s = m.seen.get(&H_CRUI).expect("its hull");
        assert!(s.weapons.contains(&970_530_001));
        assert_eq!(s.tanks.get(&Layer::Armour), Some(&2));
        assert!(!mem.groups.contains_key(&G_ME), "mine is not remembered");
    }

    fn picture_of(group: Group, hulls: Vec<(i64, u32)>, at_s: u64) -> Picture {
        Picture {
            system: SYS,
            at_s,
            mine: Strength::default(),
            theirs: vec![Enemy {
                group,
                hulls,
                source: Source::Grid,
                conf: 1.0,
            }],
            reliability: 1.0,
            blind: false,
            observers: 1,
        }
    }

    #[test]
    fn memory_decays() {
        let k = SenseKnobs::NORMAL;
        let mut mem = Memory::default();
        mem.note_picture(&picture_of(G_A, vec![(H_CRUI, 8), (H_FRIG, 1)], NOW), &k);
        let s = mem.groups[&G_A].seen[&H_CRUI].clone();
        assert_eq!(s.strength(NOW, &k), 8.0);
        let half = NOW + (k.half_life_days * DAY_S) as u64;
        assert!(
            (s.strength(half, &k) - 4.0).abs() < 1e-6,
            "halves per half-life"
        );
        // The lone frigate is below the floor after two half-lives; the
        // cruisers are not.
        let later = NOW + (2.0 * k.half_life_days * DAY_S) as u64;
        mem.prune(later, &k);
        assert!(mem.groups[&G_A].seen.contains_key(&H_CRUI));
        assert!(!mem.groups[&G_A].seen.contains_key(&H_FRIG));
        mem.prune(NOW + (20.0 * k.half_life_days * DAY_S) as u64, &k);
        assert!(mem.groups.is_empty(), "everything fades");
    }

    #[test]
    fn memory_is_capped_to_the_newest_groups() {
        let k = SenseKnobs::EASY;
        let mut mem = Memory::default();
        let n = k.enemy_memory as i64 + 3;
        for i in 0..n {
            mem.note_picture(
                &picture_of(G_A + 100 + i, vec![(H_CRUI, 3)], NOW + i as u64),
                &k,
            );
        }
        assert_eq!(mem.groups.len(), k.enemy_memory);
        assert!(mem.groups.contains_key(&(G_A + 100 + n - 1)));
        assert!(!mem.groups.contains_key(&(G_A + 100)), "the oldest went");
    }

    #[test]
    fn noise_scales_with_difficulty() {
        let mine = Strength::default();
        let mem = Memory::default();
        let r = [sighting(1, Some(&[(H_CRUI, 100)]))];
        let spread = |k: SenseKnobs| -> f64 {
            let k = SenseKnobs {
                intel_share: 1.0,
                ..k
            };
            let xs: Vec<f64> = (0..200)
                .map(|t| {
                    let p = observe(&r, &inputs(&mine, &mem, k, t));
                    f64::from(p.group(UNATTRIBUTED).map_or(0, Enemy::ships))
                })
                .collect();
            let mean = xs.iter().sum::<f64>() / xs.len() as f64;
            (xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / xs.len() as f64).sqrt()
        };
        let (e, n, h, b) = (
            spread(SenseKnobs::EASY),
            spread(SenseKnobs::NORMAL),
            spread(SenseKnobs::HARD),
            spread(SenseKnobs::BRUTAL),
        );
        assert!(e > n && n > h && h > b, "{e} {n} {h} {b}");
        assert_eq!(b, 0.0, "brutal has no noise: the exact count");
        // The same tick gives the same estimate.
        let k = SenseKnobs::NORMAL;
        let a = observe(&r, &inputs(&mine, &mem, k, 7));
        let c = observe(&r, &inputs(&mine, &mem, k, 7));
        assert_eq!(a, c);
    }

    #[test]
    fn a_blind_leader_halves_confidence() {
        let k = SenseKnobs::NORMAL;
        let mine = Strength::default();
        let mut mem = Memory::default();
        let mut seen = observe(
            &[sighting(1, Some(&[(H_CRUI, 6)]))],
            &inputs(&mine, &mem, exact(), 1),
        );
        assert!(!seen.blind);
        seen.theirs[0].group = G_A;
        mem.note_picture(&seen, &k);
        let seen_conf = seen.theirs[0].conf;
        // Nobody online with a view: the memory, at `blind_conf`.
        let blind = observe(&[], &inputs(&mine, &mem, k, 2));
        assert!(blind.blind);
        assert_eq!(blind.reliability, 0.0);
        let e = blind.group(G_A).expect("from memory");
        assert_eq!(e.source, Source::Memory);
        assert_eq!(e.hulls, vec![(H_CRUI, 6)]);
        assert!((e.conf - seen_conf * k.blind_conf).abs() < 1e-12);
        // A report with no hulls in it (Local only) is still blind.
        let mut local_only = sighting(1, None);
        local_only.local = Some([(G_A, 5)].into_iter().collect());
        assert!(observe(&[local_only], &inputs(&mine, &mem, exact(), 2)).blind);
    }

    #[test]
    fn intel_share_decides_how_many_sightings_reach_the_leader() {
        let mine = Strength::default();
        let mem = Memory::default();
        let reports: Vec<Sighting> = (1..=200)
            .map(|o| sighting(o, Some(&[(H_CRUI, 5)])))
            .collect();
        let reached = |share: f64| {
            let k = SenseKnobs {
                intel_share: share,
                ..SenseKnobs::NORMAL
            };
            observe(&reports, &inputs(&mine, &mem, k, 3)).observers
        };
        assert_eq!(reached(1.0), 200);
        assert_eq!(reached(0.0), 0);
        let (lo, hi) = (
            reached(SenseKnobs::EASY.intel_share),
            reached(SenseKnobs::HARD.intel_share),
        );
        assert!(lo < hi, "{lo} {hi}");
        // A spotted report always arrives; a stale one never does.
        let mut flagged = sighting(999, Some(&[(H_CRUI, 5)]));
        flagged.spotted = true;
        let mut stale = sighting(998, Some(&[(H_CRUI, 5)]));
        stale.spotted = true;
        stale.at_s = NOW - SIGHTING_STALE_S - 1;
        let k = SenseKnobs {
            intel_share: 0.0,
            ..SenseKnobs::NORMAL
        };
        let p = observe(&[flagged, stale], &inputs(&mine, &mem, k, 3));
        assert_eq!(p.observers, 1);
    }

    #[test]
    fn reliability_grows_with_hulls_and_observers() {
        let mine = Strength::default();
        let mem = Memory::default();
        let one = observe(
            &[sighting(1, Some(&[(H_CRUI, 5)]))],
            &inputs(&mine, &mem, exact(), 1),
        );
        let two = observe(
            &[
                sighting(1, Some(&[(H_CRUI, 5), (H_FRIG, 2)])),
                sighting(2, Some(&[(H_CRUI, 4)])),
            ],
            &inputs(&mine, &mem, exact(), 1),
        );
        assert!(one.reliability > 0.0 && one.reliability < two.reliability);
        assert!(two.reliability <= 1.0);
        // The most any observer scanned, not the sum.
        assert_eq!(two.ships(), 7);
    }

    #[test]
    fn infer_reads_hulls_weapons_and_tank_against_the_catalog() {
        let k = SenseKnobs::NORMAL;
        let mk = |key: &str, class, family, layer| Doctrine {
            key: key.to_owned(),
            class,
            family,
            layer,
            ..testkit::brick_doc()
        };
        let cat = vec![
            mk("cru-hyb-arm", Class::Cruiser, Family::Hybrid, Layer::Armour),
            mk("cru-hyb-shd", Class::Cruiser, Family::Hybrid, Layer::Shield),
            mk(
                "cru-prj-arm",
                Class::Cruiser,
                Family::Projectile,
                Layer::Armour,
            ),
            mk(
                "bs-prj-arm",
                Class::Battleship,
                Family::Projectile,
                Layer::Armour,
            ),
        ];
        let mut m = EnemyMemory::new(G_A);
        m.seen.insert(
            H_CRUI,
            Seen {
                n_max: 8,
                last_s: NOW,
                weapons: BTreeSet::from([970_530_001]),
                tanks: BTreeMap::from([(Layer::Armour, 2)]),
                utility: BTreeMap::new(),
            },
        );
        let (top, conf) = infer(&m, NOW, &k, &lens(), &cat);
        assert_eq!(top[0].0, "cru-hyb-arm");
        assert!(top[0].1 > 0.99, "{top:?}");
        assert_eq!(conf, 1.0);
        // Only a hull seen: every cruiser cell shares it; a hull the lens
        // does not know lowers the confidence.
        m.seen.get_mut(&H_CRUI).unwrap().weapons.clear();
        m.seen.get_mut(&H_CRUI).unwrap().tanks.clear();
        m.seen.insert(
            970_510_099,
            Seen {
                n_max: 8,
                last_s: NOW,
                ..Seen::default()
            },
        );
        let (top, conf) = infer(&m, NOW, &k, &lens(), &cat);
        assert_eq!(top.len(), INFER_TOP);
        assert!(top.iter().all(|t| t.0.starts_with("cru-")), "{top:?}");
        assert!((conf - 0.5).abs() < 1e-9, "{conf}");
        assert!((top.iter().map(|t| t.1).sum::<f64>() - 1.0).abs() < 1e-9);
        // Nothing known: no doctrine, no confidence.
        assert_eq!(
            infer(&EnemyMemory::new(G_B), NOW, &k, &lens(), &cat),
            (vec![], 0.0)
        );
    }

    fn message(split: Option<&[(i64, f64)]>, weapon: Option<i64>, quality: i64) -> PyValue {
        let mut kv = vec![
            ("hitQuality", PyValue::Int(quality)),
            ("isBanked", PyValue::Bool(false)),
            ("damage", PyValue::Real(40.0)),
            ("attackType", eve_server::pyutil::bstr("otherPlayerWeapons")),
            ("source", PyValue::Int(970_600_001)),
            ("attackerID", PyValue::Int(970_600_001)),
        ];
        if let Some(s) = split {
            kv.push((
                "damageAttributes",
                PyValue::Dict(
                    s.iter()
                        .map(|(a, x)| (PyValue::Real(*x), PyValue::Int(*a)))
                        .collect(),
                ),
            ));
        }
        if let Some(w) = weapon {
            kv.push(("weapon", PyValue::Int(w)));
        }
        eve_server::pyutil::pydict(kv)
    }

    #[test]
    fn a_damage_message_is_read_into_the_type_counter() {
        let em_kin = [
            (attr::EM_DAMAGE, 10.0),
            (attr::EXPLOSIVE_DAMAGE, 0.0),
            (attr::KINETIC_DAMAGE, 30.0),
            (attr::THERMAL_DAMAGE, 0.0),
        ];
        let hit = parse_damage(&[message(Some(&em_kin), Some(970_530_001), 3)]).unwrap();
        assert!(hit.incoming);
        assert_eq!(hit.weapon, Some(970_530_001));
        assert_eq!(hit.attacker, Some(970_600_001));
        assert_eq!(hit.by_type, Some([10.0, 0.0, 30.0, 0.0]));
        let mut t = DamageTally::default();
        t.record(&hit);
        t.record(&hit);
        // A miss, and my own shot, are not damage taken.
        t.record(&parse_damage(&[message(Some(&em_kin), None, 0)]).unwrap());
        let mut mine = hit.clone();
        mine.incoming = false;
        t.record(&mine);
        assert_eq!(t.hits, 2);
        let s = t.share().unwrap();
        assert!((s[0] - 0.25).abs() < 1e-12 && (s[2] - 0.75).abs() < 1e-12);
        assert_eq!(t.dominant(), Some(2));
        assert_eq!(t.by_weapon[&970_530_001], 80.0);
        // No split: the weapon is still counted, the type is unknown.
        let bare = parse_damage(&[message(None, Some(970_530_002), 3)]).unwrap();
        assert_eq!(bare.by_type, None);
        let mut u = DamageTally::default();
        u.record(&bare);
        assert_eq!(u.share(), None);
        assert_eq!(u.untyped, 40.0);
        assert_eq!(parse_damage(&[]), None);
    }

    #[test]
    fn the_sensor_keeps_each_kill_once_and_the_board_the_latest_sighting() {
        let row = |id: i64| {
            eve_server::pyutil::pydict(vec![
                ("killID", PyValue::Int(id)),
                ("victimCharacterID", PyValue::Int(970_560_001)),
                ("victimCorporationID", PyValue::Int(G_A)),
                ("victimShipTypeID", PyValue::Int(H_CRUI)),
            ])
        };
        let mut s = Sensor::default();
        s.on_kill_rows(&[row(1), row(2)]);
        s.on_kill_rows(&[row(2), row(3)]);
        assert_eq!(s.kills.len(), 3);
        assert_eq!(s.take_fresh().len(), 3);
        assert!(s.take_fresh().is_empty());

        let b = Board::default();
        b.post(sighting(1, Some(&[(H_CRUI, 1)])));
        b.post(sighting(1, Some(&[(H_CRUI, 2)])));
        b.post(Sighting {
            at_s: NOW - 10 * SIGHTING_STALE_S,
            ..sighting(2, None)
        });
        let got = b.sightings(SYS, NOW);
        assert_eq!(got.len(), 1, "one per observer, none stale");
        assert_eq!(got[0].scan.as_ref().unwrap()[&H_CRUI], 2);
        b.post_kills(s.kills.iter().cloned().collect());
        b.post_kills(s.kills.iter().cloned().collect());
        assert_eq!(b.evidence().len(), 3);
        b.forget(1);
        assert!(b.sightings(SYS, NOW).is_empty());
    }

    #[test]
    fn levels_are_monotone_and_normal_is_the_default() {
        let ks = [
            SenseKnobs::EASY,
            SenseKnobs::NORMAL,
            SenseKnobs::HARD,
            SenseKnobs::BRUTAL,
        ];
        for w in ks.windows(2) {
            assert!(w[0].obs_noise > w[1].obs_noise);
            assert!(w[0].intel_share < w[1].intel_share);
            assert!(w[0].scan_every_s > w[1].scan_every_s);
            assert!(w[0].enemy_memory < w[1].enemy_memory);
            assert!(w[0].half_life_days < w[1].half_life_days);
            assert!(w[0].blind_conf < w[1].blind_conf);
        }
        assert_eq!(SenseKnobs::at(2), SenseKnobs::NORMAL);
        assert_eq!(SenseKnobs::default(), SenseKnobs::NORMAL);
        assert_eq!(SenseKnobs::at(1), SenseKnobs::EASY);
        assert_eq!(SenseKnobs::at(9), SenseKnobs::BRUTAL);
    }
}
