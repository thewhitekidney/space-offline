//! Data and relic hacking: the pure engine and its wire format (R-HACK
//! packet HK1, lane `hk1`; the spec is gitignored because it holds client
//! ids).
//!
//! **The client is only a view [V].** It holds no rules, no board generator
//! and no node numbers. It draws whatever `OnHackingTurnComplete(events)`
//! says and sends four kinds of input to `hackingMgr`: a tile click, a
//! utility used on a tile, a utility used on the virus, and quit. Everything
//! here is the server's: the board, the rules, every number.
//!
//! The engine (HK1) is pure: [`Board`], [`Game`], [`Event`] and [`Hacks`]'
//! game methods know nothing of the world, sessions or dogma. **The world
//! side** (HK2, lane `hk2`) is at the end of the file: the [`SLOT`], the
//! analyzer ([`check_activation`] from `modules::activate`, the game opened
//! by [`on_completed`] at the end of its cycle), the inputs from
//! `service/hacking.rs` ([`input`]), the can's `hackingSecurityState`
//! ([`security_state`], sent on the site's slim and by [`CanSlimChange`]),
//! and the interruption stage ([`tick`]). The loot is HK3's: **a hacked can
//! opens empty** until then.
//!
//! ## The pieces
//!
//! - [`Rules`]: every gameplay number. Each is an assumption **[A]** with
//!   its falsifier in `docs/lanes/hk1.md`; [`Rules::SHIPPED`] is the tuned
//!   set, and tests build their own invented `Rules`.
//! - [`Board`]: the hex board, generated from a seed ([`Board::generate`])
//!   or laid out by hand ([`Board::from_layout`]).
//! - [`Game`]: one game. [`Game::start`] gives the start batch, and each
//!   input ([`Game::click`], [`Game::use_on_tile`], [`Game::use_on_virus`],
//!   [`Game::quit`]) gives that turn's events.
//! - [`Event`]: one entry of the push. [`Event::to_pyvalue`] builds the dict
//!   the client subscripts, and [`TurnComplete`] is the push itself.
//! - [`Hacks`]: the games by character, the cans' states and failures; the
//!   world slot.
//!
//! ## The wire, and its traps [V]
//!
//! Each event is a **real dict** `{'eventID': int, 'eventData': dict}` with
//! byte-string keys: the client calls `TileData(**eventData)` and
//! `UtilityElementData(index=i, **element)`, and Python 2's `**` wants `str`
//! keys. A coordinate is an `(x, y)` **tuple**, because it is a dict key in
//! the client's `tileDataByCoord`.
//!
//! - A tile record has **exactly** the eight keys `TileData.__init__`
//!   accepts; a ninth is a `TypeError` on `TILE_CREATED`.
//! - A utility-slot record **never** carries `index`: the client passes
//!   `index=` itself, so a second one is a `TypeError`.
//! - A hidden tile goes out as `TYPE_NONE` with no subtype, strength or
//!   coherence, so the wire never leaks what is under it.
//! - Only the five defense subtypes the UI has icons for exist here
//!   ([`Defense`]); any other is a `KeyError` in `GetIconTexturePath`.

use crate::content::Content;
use crate::damage::BlueTime;
use crate::modules::Cycle;
use crate::notify::Push;
use crate::pyutil::pydict;
use crate::seed::Stream;
use crate::service::{Registry, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, VecDeque};

/// A board coordinate, `(x, y)`: `x` the column, `y` the row.
pub type Coord = (i32, i32);

// ---------------------------------------------------------------------------
// Client constants [V: hackingcommon/hackingConstants]
// ---------------------------------------------------------------------------

/// `GRID_MAX_COLUMNS` [V].
pub const GRID_MAX_COLUMNS: i32 = 10;
/// `GRID_MAX_ROWS` [V].
pub const GRID_MAX_ROWS: i32 = 9;

/// Tile types [V].
pub const TYPE_NONE: i64 = -1;
pub const TYPE_SEGMENT: i64 = 0;
pub const TYPE_VIRUS: i64 = 1;
pub const TYPE_CORE: i64 = 2;
pub const TYPE_DEFENSESOFTWARE: i64 = 3;
pub const TYPE_UTILITYELEMENT: i64 = 4;
pub const TYPE_UTILITYELEMENTTILE: i64 = 5;
pub const TYPE_DATACACHE: i64 = 6;

/// The "no subtype" value, for an empty slot or a node without one [V for
/// slots; [A] for segments and caches, whose subtype the UI never reads].
pub const SUBTYPE_NONE: i64 = -1;

/// Event ids [V].
pub const EVENT_GAME_LOST: i64 = 0;
pub const EVENT_GAME_WON: i64 = 1;
pub const EVENT_GAME_START: i64 = 2;
pub const EVENT_VIRUS_CREATED: i64 = 3;
pub const EVENT_OBJECT_KILLED: i64 = 4;
pub const EVENT_GAME_STYLE: i64 = 5;
pub const EVENT_ACK: i64 = 6;
pub const EVENT_TILE_FLIPPED: i64 = 100;
pub const EVENT_TILE_CREATED: i64 = 101;
pub const EVENT_TILE_BLOCKED: i64 = 102;
pub const EVENT_UE_PICKEDUP: i64 = 200;
pub const EVENT_UE_REMOVED: i64 = 201;
pub const EVENT_UE_INUSE: i64 = 202;
pub const EVENT_DATACACHE_OPEN: i64 = 300;
pub const EVENT_ATTACK: i64 = 301;
pub const EVENT_KERNALROT: i64 = 302;
pub const EVENT_SELFREPAIR: i64 = 303;
pub const EVENT_SECONDARYVECTOR: i64 = 304;
pub const EVENT_POLYMORPHICSHIELDHIT: i64 = 305;
pub const EVENT_HONEYPOT_STRENGTH: i64 = 306;
pub const EVENT_HONEYPOT_HEALING: i64 = 307;
pub const EVENT_DISTANCEINDICATOR: i64 = 308;

/// The eight keys `TileData.__init__` accepts, and the only keys a tile
/// record ever carries [V].
pub const TILE_KEYS: [&str; 8] = [
    "id",
    "type",
    "subtype",
    "coord",
    "blocked",
    "hidden",
    "strength",
    "coherence",
];

/// The keys a utility-slot record may carry [V]. Never `index`.
pub const SLOT_KEYS: [&str; 3] = ["id", "subtype", "info"];

/// The virus's id in its records [A]: tiles are numbered from 1, so 0 never
/// clashes. Falsified by a client error naming the virus id.
pub const VIRUS_ID: i64 = 0;

/// The most utility slots the client can use: keys 1, 2 and 3 click slots
/// 0..2 [V `hackingWindow.OnKeyDown`], and no analyzer has more [V].
pub const MAX_SLOTS: usize = 3;

/// The odd-r neighbour offsets `(dx, dy)` of a tile on an **odd** row [V].
pub const ODD_ROW_OFFSETS: [Coord; 6] = [(0, -1), (1, -1), (-1, 0), (1, 0), (0, 1), (1, 1)];
/// The odd-r neighbour offsets `(dx, dy)` of a tile on an **even** row [V].
pub const EVEN_ROW_OFFSETS: [Coord; 6] = [(-1, -1), (0, -1), (-1, 0), (1, 0), (-1, 1), (0, 1)];

/// The six neighbouring coordinates of `c` in the client's odd-r layout,
/// unfiltered by any board.
pub fn neighbours(c: Coord) -> [Coord; 6] {
    let offsets = if c.1.rem_euclid(2) == 1 {
        ODD_ROW_OFFSETS
    } else {
        EVEN_ROW_OFFSETS
    };
    offsets.map(|(dx, dy)| (c.0 + dx, c.1 + dy))
}

// ---------------------------------------------------------------------------
// Node kinds
// ---------------------------------------------------------------------------

/// Which window style and audio the game uses [V `GAMETYPE_*`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// Data hacking, `GAMETYPE_HACKING` 0.
    Hacking,
    /// Relic hacking, `GAMETYPE_ARCHEOLOGY` 1.
    Archeology,
}

impl Style {
    pub fn wire(self) -> i64 {
        match self {
            Style::Hacking => 0,
            Style::Archeology => 1,
        }
    }
}

/// The core's level [V subtypes].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreLevel {
    Low,
    Medium,
    High,
}

impl CoreLevel {
    /// `CORE_HIGH` 10, `CORE_LOW` 11, `CORE_MEDIUM` 12 [V].
    pub fn subtype(self) -> i64 {
        match self {
            CoreLevel::High => 10,
            CoreLevel::Low => 11,
            CoreLevel::Medium => 12,
        }
    }

    fn index(self) -> usize {
        match self {
            CoreLevel::Low => 0,
            CoreLevel::Medium => 1,
            CoreLevel::High => 2,
        }
    }
}

/// The five defense subsystems the UI can draw [V]. There is deliberately no
/// way to name any other subtype.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Defense {
    /// Anti-Virus, `ANTIVIRUS` 100: high strength.
    AntiVirus,
    /// Firewall, `FIREWALL` 101: high coherence.
    Firewall,
    /// Virus Suppressor, `HONEYPOT_STRENGTH` 102: lowers the virus's
    /// strength while it is revealed and alive.
    Suppressor,
    /// Restoration Node, `HONEYPOT_HEALING` 103: heals the other revealed
    /// defenses each turn.
    Restoration,
    /// Disruptor, `DISRUPTOR` 104: corrupts nearby distance indicators.
    Disruptor,
}

impl Defense {
    /// Every drawable defense, in [`Rules::defense_weights`] order.
    pub const ALL: [Defense; 5] = [
        Defense::Firewall,
        Defense::AntiVirus,
        Defense::Restoration,
        Defense::Suppressor,
        Defense::Disruptor,
    ];

    /// The wire subtype [V].
    pub fn subtype(self) -> i64 {
        match self {
            Defense::AntiVirus => 100,
            Defense::Firewall => 101,
            Defense::Suppressor => 102,
            Defense::Restoration => 103,
            Defense::Disruptor => 104,
        }
    }

    fn index(self) -> usize {
        Defense::ALL.iter().position(|&d| d == self).unwrap_or(0)
    }
}

/// The subtypes the client can draw for a defense, for tests and checks.
pub const DRAWABLE_DEFENSE_SUBTYPES: [i64; 5] = [100, 101, 102, 103, 104];

/// The four utilities [V subtypes].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Utility {
    /// `SELFREPAIR` 0: used on the virus, heals it over turns.
    SelfRepair,
    /// `KERNALROT` 1: used on a node, halves its coherence at once.
    KernelRot,
    /// `SECONDARYVECTOR` 2: used on a node, damages it over turns.
    SecondaryVector,
    /// `POLYMORPHICSHIELD` 3: used on the virus, absorbs counter-hits.
    PolymorphicShield,
}

impl Utility {
    /// Every utility, in [`Rules::utility_weights`] order.
    pub const ALL: [Utility; 4] = [
        Utility::SelfRepair,
        Utility::KernelRot,
        Utility::SecondaryVector,
        Utility::PolymorphicShield,
    ];

    pub fn subtype(self) -> i64 {
        match self {
            Utility::SelfRepair => 0,
            Utility::KernelRot => 1,
            Utility::SecondaryVector => 2,
            Utility::PolymorphicShield => 3,
        }
    }

    fn on_virus(self) -> bool {
        matches!(self, Utility::SelfRepair | Utility::PolymorphicShield)
    }
}

/// What a board cell holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// An empty node: passable once revealed.
    Segment,
    /// The system core: kill it to win.
    Core {
        level: CoreLevel,
        coherence: i64,
        strength: i64,
    },
    /// A defense subsystem.
    Defense {
        kind: Defense,
        coherence: i64,
        strength: i64,
    },
    /// A data cache; a click opens it into what it holds (a segment, a
    /// defense or a utility tile), rolled at generation.
    Cache(Box<Node>),
    /// A utility on the board, waiting to be picked up.
    Utility(Utility),
}

impl Node {
    fn coherence(&self) -> i64 {
        match self {
            Node::Core { coherence, .. } | Node::Defense { coherence, .. } => *coherence,
            _ => 0,
        }
    }

    fn strength(&self) -> i64 {
        match self {
            Node::Core { strength, .. } | Node::Defense { strength, .. } => *strength,
            _ => 0,
        }
    }

    fn set_coherence(&mut self, v: i64) {
        if let Node::Core { coherence, .. } | Node::Defense { coherence, .. } = self {
            *coherence = v.max(0);
        }
    }

    fn is_target(&self) -> bool {
        matches!(self, Node::Core { .. } | Node::Defense { .. })
    }

    fn defense(&self) -> Option<Defense> {
        match self {
            Node::Defense { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    /// `(type, subtype)` on the wire, once revealed.
    fn wire_kind(&self) -> (i64, i64) {
        match self {
            Node::Segment => (TYPE_SEGMENT, SUBTYPE_NONE),
            Node::Core { level, .. } => (TYPE_CORE, level.subtype()),
            Node::Defense { kind, .. } => (TYPE_DEFENSESOFTWARE, kind.subtype()),
            Node::Cache(_) => (TYPE_DATACACHE, SUBTYPE_NONE),
            Node::Utility(u) => (TYPE_UTILITYELEMENTTILE, u.subtype()),
        }
    }
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

/// A defense's numbers: `coherence = coh_base + coh_per_tier * tier`, the
/// same for strength.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeStats {
    pub coh_base: i64,
    pub coh_per_tier: i64,
    pub str_base: i64,
    pub str_per_tier: i64,
}

impl NodeStats {
    pub const fn flat(coh: i64, str: i64) -> Self {
        NodeStats {
            coh_base: coh,
            coh_per_tier: 0,
            str_base: str,
            str_per_tier: 0,
        }
    }

    fn at(&self, tier: u8) -> (i64, i64) {
        let t = i64::from(tier);
        (
            (self.coh_base + self.coh_per_tier * t).max(1),
            (self.str_base + self.str_per_tier * t).max(0),
        )
    }
}

/// Every gameplay number. **Each is an [A]** (the client carries none [V]);
/// the falsifiers are in `docs/lanes/hk1.md`. Arrays indexed by tier are
/// `[tier 1, tier 2, tier 3, tier 4]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    /// The board box `(columns, rows)` per tier, at most 10 by 9 [V bound].
    pub dims: [(i32, i32); 4],
    /// The share of the box removed as holes, in percent.
    pub hole_percent: u32,
    /// Defenses placed per tier.
    pub defenses: [u32; 4],
    /// Data caches placed per tier.
    pub caches: [u32; 4],
    /// Utility tiles placed per tier.
    pub utilities: [u32; 4],
    /// The defense mix, in [`Defense::ALL`] order.
    pub defense_weights: [u32; 5],
    /// Up to this tier only Firewalls and Anti-Virus are placed.
    pub basic_defense_max_tier: u8,
    /// Each defense's numbers, in [`Defense::ALL`] order.
    pub defense_stats: [NodeStats; 5],
    /// The core level per tier.
    pub core_level: [CoreLevel; 4],
    /// `(coherence, strength)` of the core, low, medium, high.
    pub core_stats: [(i64, i64); 3],
    /// What a cache opens into: `[defense, utility, empty]` weights.
    pub cache_weights: [u32; 3],
    /// The utility mix on the board and in caches, in [`Utility::ALL`]
    /// order.
    pub utility_weights: [u32; 4],
    /// The fewest hex steps from the start to anything that is not a
    /// segment.
    pub min_start_distance: u32,
    /// Coherence a Restoration Node gives each other revealed defense per
    /// turn.
    pub restoration_heal: i64,
    /// Strength each revealed, live Suppressor takes from the virus (the
    /// virus never drops below 1).
    pub suppressor_amount: i64,
    /// Secondary Vector: damage per turn, and turns.
    pub secondary_vector: (i64, u32),
    /// Self Repair: coherence per turn, and turns.
    pub self_repair: (i64, u32),
    /// Counter-hits a Polymorphic Shield absorbs.
    pub shield_hits: u32,
    /// The largest distance indicator shown.
    pub distance_cap: i64,
    /// Failures after which the can is destroyed.
    pub failures_to_destroy: u32,
}

impl Rules {
    /// The shipped rules: every value an [A], seeded from community tables
    /// [D] and to be tuned in the morning test (`docs/lanes/hk1.md`).
    pub const SHIPPED: Rules = Rules {
        dims: [(6, 5), (7, 6), (8, 7), (10, 9)],
        hole_percent: 15,
        defenses: [2, 3, 5, 7],
        caches: [2, 3, 3, 4],
        utilities: [1, 1, 2, 2],
        defense_weights: [35, 35, 10, 10, 10],
        basic_defense_max_tier: 2,
        defense_stats: [
            // Firewall
            NodeStats {
                coh_base: 60,
                coh_per_tier: 10,
                str_base: 10,
                str_per_tier: 0,
            },
            // Anti-Virus
            NodeStats {
                coh_base: 20,
                coh_per_tier: 10,
                str_base: 20,
                str_per_tier: 5,
            },
            // Restoration
            NodeStats::flat(40, 10),
            // Suppressor
            NodeStats::flat(40, 10),
            // Disruptor
            NodeStats::flat(50, 10),
        ],
        core_level: [
            CoreLevel::Low,
            CoreLevel::Medium,
            CoreLevel::Medium,
            CoreLevel::High,
        ],
        core_stats: [(40, 10), (70, 20), (100, 30)],
        cache_weights: [40, 40, 20],
        utility_weights: [1, 1, 1, 1],
        min_start_distance: 2,
        restoration_heal: 10,
        suppressor_amount: 10,
        secondary_vector: (20, 3),
        self_repair: (10, 3),
        shield_hits: 2,
        distance_cap: 5,
        failures_to_destroy: 2,
    };

    fn tier_index(tier: u8) -> usize {
        usize::from(tier.clamp(1, 4) - 1)
    }

    fn defense_node(&self, kind: Defense, tier: u8) -> Node {
        let (coherence, strength) = self.defense_stats[kind.index()].at(tier);
        Node::Defense {
            kind,
            coherence,
            strength,
        }
    }

    fn core_node(&self, tier: u8) -> Node {
        let level = self.core_level[Self::tier_index(tier)];
        let (coherence, strength) = self.core_stats[level.index()];
        Node::Core {
            level,
            coherence: coherence.max(1),
            strength: strength.max(0),
        }
    }

    fn roll_defense(&self, tier: u8, s: &mut Stream) -> Node {
        let mut w = self.defense_weights;
        if tier <= self.basic_defense_max_tier {
            for d in [
                Defense::Restoration,
                Defense::Suppressor,
                Defense::Disruptor,
            ] {
                w[d.index()] = 0;
            }
        }
        let kind = Defense::ALL[pick(&w, s)];
        self.defense_node(kind, tier)
    }

    fn roll_utility(&self, s: &mut Stream) -> Utility {
        Utility::ALL[pick(&self.utility_weights, s)]
    }

    fn roll_cache(&self, tier: u8, s: &mut Stream) -> Node {
        match pick(&self.cache_weights, s) {
            0 => self.roll_defense(tier, s),
            1 => Node::Utility(self.roll_utility(s)),
            _ => Node::Segment,
        }
    }

    /// The `info` (hint amount) of a held utility [V `GetHint`].
    fn info(&self, u: Utility) -> Option<i64> {
        match u {
            Utility::SelfRepair => Some(self.self_repair.0),
            Utility::SecondaryVector => Some(self.secondary_vector.0),
            Utility::KernelRot | Utility::PolymorphicShield => None,
        }
    }
}

/// A weighted pick; index 0 when every weight is 0.
fn pick(weights: &[u32], s: &mut Stream) -> usize {
    let total: u64 = weights.iter().map(|&w| u64::from(w)).sum();
    if total == 0 {
        return 0;
    }
    let mut r = s.range(0, total);
    for (i, &w) in weights.iter().enumerate() {
        if r < u64::from(w) {
            return i;
        }
        r -= u64::from(w);
    }
    weights.len() - 1
}

fn shuffle<T>(v: &mut [T], s: &mut Stream) {
    for i in (1..v.len()).rev() {
        let j = s.range(0, i as u64 + 1) as usize;
        v.swap(i, j);
    }
}

/// The seed of a can's `counter`-th board: splitmix over the can id and the
/// counter, so two games on one can differ and a replay is exact.
pub fn board_seed(can_id: i64, counter: u64) -> u64 {
    /// A salt so this never shares a stream with `seed::Kind` users.
    const SALT: u64 = 0x4841_434B_0000_0001;
    crate::seed::mix(SALT, &[can_id as u64, counter])
}

// ---------------------------------------------------------------------------
// The board
// ---------------------------------------------------------------------------

/// One board cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    /// The tile's id on the wire: 1.. in coordinate order [A].
    pub id: i64,
    pub coord: Coord,
    pub node: Node,
    pub hidden: bool,
    pub blocked: bool,
    /// The coherence a heal may restore up to: the node's coherence when it
    /// became a defense.
    max_coherence: i64,
}

impl Tile {
    /// The tile's record as the wire shows it: a hidden tile leaks nothing.
    pub fn record(&self) -> TileRec {
        let (kind, subtype, strength, coherence) = if self.hidden {
            (TYPE_NONE, SUBTYPE_NONE, 0, 0)
        } else {
            let (k, s) = self.node.wire_kind();
            (k, s, self.node.strength(), self.node.coherence())
        };
        TileRec {
            id: self.id,
            kind,
            subtype,
            coord: self.coord,
            blocked: self.blocked,
            hidden: self.hidden,
            strength,
            coherence,
        }
    }

    fn live_defense(&self) -> Option<Defense> {
        if self.hidden {
            None
        } else {
            self.node.defense()
        }
    }

    fn passable(&self) -> bool {
        !self.hidden && self.node == Node::Segment
    }
}

/// The hex board: the cells that exist (holes are absent), the start and
/// the core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Board {
    pub cols: i32,
    pub rows: i32,
    pub tiles: Vec<Tile>,
    pub start: Coord,
    pub core: Coord,
    index: BTreeMap<Coord, usize>,
}

impl Board {
    /// A board laid out by hand: exactly the `cells` listed. The tile at
    /// `start` is revealed and must be a segment; everything else starts
    /// hidden. `None` when a cell is out of bounds, listed twice, the start
    /// is missing or not a segment, or there is not exactly one core.
    pub fn from_layout(
        cols: i32,
        rows: i32,
        start: Coord,
        cells: Vec<(Coord, Node)>,
    ) -> Option<Board> {
        if !(1..=GRID_MAX_COLUMNS).contains(&cols) || !(1..=GRID_MAX_ROWS).contains(&rows) {
            return None;
        }
        let mut sorted: BTreeMap<Coord, Node> = BTreeMap::new();
        for (c, n) in cells {
            if c.0 < 0 || c.1 < 0 || c.0 >= cols || c.1 >= rows || sorted.insert(c, n).is_some() {
                return None;
            }
        }
        if sorted.get(&start) != Some(&Node::Segment) {
            return None;
        }
        let cores: Vec<Coord> = sorted
            .iter()
            .filter(|(_, n)| matches!(n, Node::Core { .. }))
            .map(|(c, _)| *c)
            .collect();
        let [core] = cores[..] else {
            return None;
        };
        let mut index = BTreeMap::new();
        let tiles = sorted
            .into_iter()
            .enumerate()
            .map(|(i, (coord, node))| {
                index.insert(coord, i);
                Tile {
                    id: i as i64 + 1,
                    coord,
                    max_coherence: node.coherence(),
                    hidden: coord != start,
                    blocked: false,
                    node,
                }
            })
            .collect();
        Some(Board {
            cols,
            rows,
            tiles,
            start,
            core,
            index,
        })
    }

    /// Generate the board of a `tier` (1..4) can from `seed`.
    ///
    /// The box is `rules.dims[tier]`, clamped to the client's 10 by 9 [V].
    /// The start is a revealed segment in the leftmost column; holes are cut
    /// while the rest stays connected; the core goes at the farthest BFS
    /// distance from the start or one short of it; defenses, caches and
    /// utilities go on cells at least `min_start_distance` away. All [A].
    pub fn generate(rules: &Rules, tier: u8, seed: u64) -> Board {
        let tier = tier.clamp(1, 4);
        let ti = Rules::tier_index(tier);
        let (cols, rows) = rules.dims[ti];
        let cols = cols.clamp(2, GRID_MAX_COLUMNS);
        let rows = rows.clamp(1, GRID_MAX_ROWS);
        let mut s = Stream::new(seed);
        let start = (0, s.range(0, rows as u64) as i32);

        let mut cells: Vec<Coord> = (0..rows)
            .flat_map(|y| (0..cols).map(move |x| (x, y)))
            .collect();
        let holes = cells.len() * rules.hole_percent.min(90) as usize / 100;
        let mut order = cells.clone();
        shuffle(&mut order, &mut s);
        let mut removed = 0;
        for c in order {
            if removed >= holes {
                break;
            }
            if c == start {
                continue;
            }
            let rest: Vec<Coord> = cells.iter().copied().filter(|&x| x != c).collect();
            if bfs(&rest, start).len() == rest.len() {
                cells = rest;
                removed += 1;
            }
        }

        let dist = bfs(&cells, start);
        let max_d = dist.values().copied().max().unwrap_or(0);
        let floor = max_d.saturating_sub(1).max(1.min(max_d));
        let mut far: Vec<Coord> = dist
            .iter()
            .filter(|&(_, &d)| d >= floor)
            .map(|(c, _)| *c)
            .collect();
        far.sort();
        let core = far[s.range(0, far.len() as u64) as usize];

        let mut free: Vec<Coord> = dist
            .iter()
            .filter(|&(&c, &d)| c != core && d >= rules.min_start_distance)
            .map(|(c, _)| *c)
            .collect();
        free.sort();
        shuffle(&mut free, &mut s);
        let mut free = free.into_iter();

        let mut nodes: BTreeMap<Coord, Node> = cells.iter().map(|&c| (c, Node::Segment)).collect();
        nodes.insert(core, rules.core_node(tier));
        for _ in 0..rules.defenses[ti] {
            let Some(c) = free.next() else { break };
            nodes.insert(c, rules.roll_defense(tier, &mut s));
        }
        for _ in 0..rules.caches[ti] {
            let Some(c) = free.next() else { break };
            let inner = rules.roll_cache(tier, &mut s);
            nodes.insert(c, Node::Cache(Box::new(inner)));
        }
        for _ in 0..rules.utilities[ti] {
            let Some(c) = free.next() else { break };
            nodes.insert(c, Node::Utility(rules.roll_utility(&mut s)));
        }
        Board::from_layout(cols, rows, start, nodes.into_iter().collect())
            .expect("a generated board is a valid layout")
    }

    /// The tile at `c`.
    pub fn tile(&self, c: Coord) -> Option<&Tile> {
        self.index.get(&c).map(|&i| &self.tiles[i])
    }

    fn at(&self, c: Coord) -> Option<usize> {
        self.index.get(&c).copied()
    }

    /// The indices of the board tiles next to tile `i`.
    fn adjacent(&self, i: usize) -> Vec<usize> {
        neighbours(self.tiles[i].coord)
            .iter()
            .filter_map(|c| self.at(*c))
            .collect()
    }

    /// Hex steps from `from` to every board cell reachable over board cells.
    pub fn distances(&self, from: Coord) -> BTreeMap<Coord, u32> {
        let cells: Vec<Coord> = self.tiles.iter().map(|t| t.coord).collect();
        bfs(&cells, from)
    }
}

/// Breadth-first hex distances from `from` over `cells`.
fn bfs(cells: &[Coord], from: Coord) -> BTreeMap<Coord, u32> {
    let set: std::collections::BTreeSet<Coord> = cells.iter().copied().collect();
    let mut dist = BTreeMap::new();
    if !set.contains(&from) {
        return dist;
    }
    dist.insert(from, 0);
    let mut q = VecDeque::from([from]);
    while let Some(c) = q.pop_front() {
        let d = dist[&c];
        for n in neighbours(c) {
            if set.contains(&n) && !dist.contains_key(&n) {
                dist.insert(n, d + 1);
                q.push_back(n);
            }
        }
    }
    dist
}

// ---------------------------------------------------------------------------
// Records and events
// ---------------------------------------------------------------------------

fn coord_value(c: Coord) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Int(i64::from(c.0)),
        PyValue::Int(i64::from(c.1)),
    ])
}

/// A tile record: the creation record and every tile update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileRec {
    pub id: i64,
    pub kind: i64,
    pub subtype: i64,
    pub coord: Coord,
    pub blocked: bool,
    pub hidden: bool,
    pub strength: i64,
    pub coherence: i64,
}

impl TileRec {
    /// The dict: exactly [`TILE_KEYS`].
    pub fn to_pyvalue(&self) -> PyValue {
        pydict(vec![
            ("id", PyValue::Int(self.id)),
            ("type", PyValue::Int(self.kind)),
            ("subtype", PyValue::Int(self.subtype)),
            ("coord", coord_value(self.coord)),
            ("blocked", PyValue::Bool(self.blocked)),
            ("hidden", PyValue::Bool(self.hidden)),
            ("strength", PyValue::Int(self.strength)),
            ("coherence", PyValue::Int(self.coherence)),
        ])
    }
}

/// The virus's record [V]: `{'id', 'type': 1, 'strength', 'coherence'}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirusRec {
    pub strength: i64,
    pub coherence: i64,
}

impl VirusRec {
    pub fn to_pyvalue(&self) -> PyValue {
        pydict(vec![
            ("id", PyValue::Int(VIRUS_ID)),
            ("type", PyValue::Int(TYPE_VIRUS)),
            ("strength", PyValue::Int(self.strength)),
            ("coherence", PyValue::Int(self.coherence)),
        ])
    }
}

/// A utility-slot record: keys [`SLOT_KEYS`], **never `index`** [V].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotRec {
    pub id: Option<i64>,
    pub subtype: i64,
    pub info: Option<i64>,
}

impl SlotRec {
    pub const EMPTY: SlotRec = SlotRec {
        id: None,
        subtype: SUBTYPE_NONE,
        info: None,
    };

    pub fn to_pyvalue(&self) -> PyValue {
        let opt = |v: Option<i64>| v.map_or(PyValue::None, PyValue::Int);
        pydict(vec![
            ("id", opt(self.id)),
            ("subtype", PyValue::Int(self.subtype)),
            ("info", opt(self.info)),
        ])
    }
}

fn inventory_value(inv: &[SlotRec]) -> PyValue {
    PyValue::List(inv.iter().map(SlotRec::to_pyvalue).collect())
}

/// One entry of `OnHackingTurnComplete`'s list [V shapes, by handler].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    GameLost,
    GameWon,
    /// Must follow every `TILE_CREATED` [V].
    GameStart {
        module_type_id: Option<i64>,
    },
    VirusCreated {
        virus: VirusRec,
        inventory: Vec<SlotRec>,
    },
    /// `OBJECT_KILLED` of the virus.
    VirusKilled(VirusRec),
    /// `OBJECT_KILLED` of a tile.
    TileKilled(TileRec),
    GameStyle(Style),
    /// A no-op turn: an invalid input.
    Ack,
    TileFlipped(TileRec),
    TileCreated(TileRec),
    TileBlocked {
        coord: Coord,
        blocked: bool,
    },
    UtilityPickedUp {
        coord: Coord,
        replacement: TileRec,
        inventory: Vec<SlotRec>,
    },
    /// `UE_REMOVED`; the inventory is **nested** `{'inventory': [...]}` [V].
    UtilityRemoved {
        index: usize,
        inventory: Vec<SlotRec>,
    },
    UtilityInUse {
        index: usize,
        value: bool,
    },
    CacheOpened(TileRec),
    Attack {
        attacker: VirusRec,
        defender: TileRec,
    },
    KernelRot(TileRec),
    SelfRepair {
        index: usize,
        remaining: u32,
        target: VirusRec,
    },
    SecondaryVector {
        tile: TileRec,
        index: usize,
        remaining: u32,
    },
    ShieldHit {
        index: usize,
        remaining: u32,
    },
    SuppressorStrength(VirusRec),
    RestorationHeal {
        source: Coord,
        healed: TileRec,
        amount: i64,
    },
    Distance {
        coord: Coord,
        distance: i64,
    },
}

/// A tile record with extra keys, for the update events that carry them
/// (`SECONDARYVECTOR`). These go through `TileData.Update(**kw)`, which
/// sets any attribute [V].
fn tile_with(rec: &TileRec, extra: Vec<(&str, PyValue)>) -> PyValue {
    let PyValue::Dict(mut pairs) = rec.to_pyvalue() else {
        unreachable!("a tile record is a dict")
    };
    pairs.extend(crate::pyutil::pypairs(extra));
    PyValue::Dict(pairs)
}

impl Event {
    /// The wire `eventID` [V].
    pub fn id(&self) -> i64 {
        match self {
            Event::GameLost => EVENT_GAME_LOST,
            Event::GameWon => EVENT_GAME_WON,
            Event::GameStart { .. } => EVENT_GAME_START,
            Event::VirusCreated { .. } => EVENT_VIRUS_CREATED,
            Event::VirusKilled(_) | Event::TileKilled(_) => EVENT_OBJECT_KILLED,
            Event::GameStyle(_) => EVENT_GAME_STYLE,
            Event::Ack => EVENT_ACK,
            Event::TileFlipped(_) => EVENT_TILE_FLIPPED,
            Event::TileCreated(_) => EVENT_TILE_CREATED,
            Event::TileBlocked { .. } => EVENT_TILE_BLOCKED,
            Event::UtilityPickedUp { .. } => EVENT_UE_PICKEDUP,
            Event::UtilityRemoved { .. } => EVENT_UE_REMOVED,
            Event::UtilityInUse { .. } => EVENT_UE_INUSE,
            Event::CacheOpened(_) => EVENT_DATACACHE_OPEN,
            Event::Attack { .. } => EVENT_ATTACK,
            Event::KernelRot(_) => EVENT_KERNALROT,
            Event::SelfRepair { .. } => EVENT_SELFREPAIR,
            Event::SecondaryVector { .. } => EVENT_SECONDARYVECTOR,
            Event::ShieldHit { .. } => EVENT_POLYMORPHICSHIELDHIT,
            Event::SuppressorStrength(_) => EVENT_HONEYPOT_STRENGTH,
            Event::RestorationHeal { .. } => EVENT_HONEYPOT_HEALING,
            Event::Distance { .. } => EVENT_DISTANCEINDICATOR,
        }
    }

    /// The `eventData` dict.
    pub fn data(&self) -> PyValue {
        let idx = |i: &usize| PyValue::Int(*i as i64);
        match self {
            Event::GameLost | Event::GameWon | Event::Ack => pydict(vec![]),
            Event::GameStart { module_type_id } => pydict(vec![(
                "moduleTypeID",
                module_type_id.map_or(PyValue::None, PyValue::Int),
            )]),
            Event::VirusCreated { virus, inventory } => {
                let PyValue::Dict(mut pairs) = virus.to_pyvalue() else {
                    unreachable!("a virus record is a dict")
                };
                pairs.extend(crate::pyutil::pypairs(vec![(
                    "inventory",
                    inventory_value(inventory),
                )]));
                PyValue::Dict(pairs)
            }
            Event::SuppressorStrength(v) => pydict(vec![("virus", v.to_pyvalue())]),
            Event::VirusKilled(v) => v.to_pyvalue(),
            Event::TileKilled(t)
            | Event::TileFlipped(t)
            | Event::TileCreated(t)
            | Event::CacheOpened(t)
            | Event::KernelRot(t) => t.to_pyvalue(),
            Event::GameStyle(s) => pydict(vec![("style", PyValue::Int(s.wire()))]),
            Event::TileBlocked { coord, blocked } => pydict(vec![
                ("coord", coord_value(*coord)),
                ("blocked", PyValue::Bool(*blocked)),
            ]),
            Event::UtilityPickedUp {
                coord,
                replacement,
                inventory,
            } => pydict(vec![
                ("coord", coord_value(*coord)),
                ("replacementObject", replacement.to_pyvalue()),
                ("inventory", inventory_value(inventory)),
            ]),
            Event::UtilityRemoved { index, inventory } => pydict(vec![
                ("index", idx(index)),
                (
                    "inventory",
                    pydict(vec![("inventory", inventory_value(inventory))]),
                ),
            ]),
            Event::UtilityInUse { index, value } => pydict(vec![
                ("index", idx(index)),
                ("value", PyValue::Bool(*value)),
            ]),
            Event::Attack { attacker, defender } => pydict(vec![
                ("attackerResult", attacker.to_pyvalue()),
                ("defenderResult", defender.to_pyvalue()),
            ]),
            Event::SelfRepair {
                index,
                remaining,
                target,
            } => pydict(vec![
                ("index", idx(index)),
                ("durationRemaining", PyValue::Int(i64::from(*remaining))),
                ("target", target.to_pyvalue()),
            ]),
            Event::SecondaryVector {
                tile,
                index,
                remaining,
            } => tile_with(
                tile,
                vec![
                    ("index", idx(index)),
                    ("durationRemaining", PyValue::Int(i64::from(*remaining))),
                ],
            ),
            Event::ShieldHit { index, remaining } => pydict(vec![
                ("index", idx(index)),
                ("durationRemaining", PyValue::Int(i64::from(*remaining))),
            ]),
            Event::RestorationHeal {
                source,
                healed,
                amount,
            } => pydict(vec![
                ("coord", coord_value(*source)),
                ("healedObject", healed.to_pyvalue()),
                ("healingAmount", PyValue::Int(*amount)),
            ]),
            Event::Distance { coord, distance } => pydict(vec![
                ("coord", coord_value(*coord)),
                ("distanceIndicator", PyValue::Int(*distance)),
            ]),
        }
    }

    /// `{'eventID': id, 'eventData': data}` [V].
    pub fn to_pyvalue(&self) -> PyValue {
        pydict(vec![
            ("eventID", PyValue::Int(self.id())),
            ("eventData", self.data()),
        ])
    }
}

/// `OnHackingTurnComplete(events)`, to the hacking character.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnComplete {
    pub to: i64,
    pub events: Vec<Event>,
}

impl Push for TurnComplete {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnHackingTurnComplete",
            vec![PyValue::List(
                self.events.iter().map(Event::to_pyvalue).collect(),
            )],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

// ---------------------------------------------------------------------------
// The game
// ---------------------------------------------------------------------------

/// The virus as the analyzer makes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirusStats {
    pub coherence: i64,
    pub strength: i64,
    pub slots: usize,
}

impl VirusStats {
    /// From the analyzer's modified attributes [V]: `virusCoherence` (1909),
    /// `virusStrength` (1910) and `virusElementSlots` (1911), as
    /// `dogma::module_attribute` gives them. Rounded; coherence and strength
    /// at least 1, slots at most [`MAX_SLOTS`].
    pub fn from_attributes(coherence: f64, strength: f64, slots: f64) -> VirusStats {
        let round = |v: f64| if v.is_finite() { v.round() as i64 } else { 0 };
        VirusStats {
            coherence: round(coherence).max(1),
            strength: round(strength).max(1),
            slots: round(slots).clamp(0, MAX_SLOTS as i64) as usize,
        }
    }
}

/// Where a game stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Running,
    Won,
    Lost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Active {
    remaining: u32,
    target: Option<Coord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Empty,
    Held {
        id: i64,
        kind: Utility,
        active: Option<Active>,
    },
}

/// One hacking game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    pub board: Board,
    rules: Rules,
    style: Style,
    module_type_id: Option<i64>,
    coherence: i64,
    base_strength: i64,
    strength: i64,
    /// The most coherence a repair restores to: the HUD's bar maximum,
    /// 4/3 of the initial coherence [V `VirusInfo`].
    coherence_cap: i64,
    slots: Vec<Slot>,
    outcome: Outcome,
    turns: u32,
}

impl Game {
    pub fn new(
        board: Board,
        rules: &Rules,
        virus: VirusStats,
        style: Style,
        module_type_id: Option<i64>,
    ) -> Game {
        let slots = virus.slots.min(MAX_SLOTS);
        Game {
            board,
            rules: rules.clone(),
            style,
            module_type_id,
            coherence: virus.coherence,
            base_strength: virus.strength,
            strength: virus.strength,
            coherence_cap: virus.coherence * 4 / 3,
            slots: vec![Slot::Empty; slots],
            outcome: Outcome::Running,
            turns: 0,
        }
    }

    pub fn outcome(&self) -> Outcome {
        self.outcome
    }

    /// Valid turns played.
    pub fn turns(&self) -> u32 {
        self.turns
    }

    pub fn rules(&self) -> &Rules {
        &self.rules
    }

    /// The virus as the client sees it now.
    pub fn virus(&self) -> VirusRec {
        VirusRec {
            strength: self.strength,
            coherence: self.coherence,
        }
    }

    /// The slots as the client sees them now.
    pub fn inventory(&self) -> Vec<SlotRec> {
        self.slots
            .iter()
            .map(|s| match *s {
                Slot::Empty => SlotRec::EMPTY,
                Slot::Held { id, kind, .. } => SlotRec {
                    id: Some(id),
                    subtype: kind.subtype(),
                    info: self.rules.info(kind),
                },
            })
            .collect()
    }

    /// The start batch, in the order the client needs [V-derived]:
    /// `GAME_STYLE`, one `TILE_CREATED` per node, `VIRUS_CREATED`,
    /// `GAME_START`.
    pub fn start(&self) -> Vec<Event> {
        let mut ev = vec![Event::GameStyle(self.style)];
        ev.extend(
            self.board
                .tiles
                .iter()
                .map(|t| Event::TileCreated(t.record())),
        );
        ev.push(Event::VirusCreated {
            virus: self.virus(),
            inventory: self.inventory(),
        });
        ev.push(Event::GameStart {
            module_type_id: self.module_type_id,
        });
        ev
    }

    /// A tile click with no utility selected.
    pub fn click(&mut self, coord: Coord) -> Vec<Event> {
        if self.outcome != Outcome::Running {
            return vec![Event::Ack];
        }
        let Some(i) = self.board.at(coord) else {
            return vec![Event::Ack];
        };
        let mut ev = Vec::new();
        let t = &self.board.tiles[i];
        if t.hidden {
            if t.blocked || !self.reachable(i) {
                return vec![Event::Ack];
            }
            self.reveal(i, &mut ev);
        } else {
            match &t.node {
                Node::Segment => return vec![Event::Ack],
                Node::Cache(_) => self.open_cache(i, &mut ev),
                Node::Utility(u) => {
                    let u = *u;
                    let Some(k) = self.slots.iter().position(|s| *s == Slot::Empty) else {
                        return vec![Event::Ack];
                    };
                    self.slots[k] = Slot::Held {
                        id: self.board.tiles[i].id,
                        kind: u,
                        active: None,
                    };
                    self.board.tiles[i].node = Node::Segment;
                    ev.push(Event::UtilityPickedUp {
                        coord,
                        replacement: self.board.tiles[i].record(),
                        inventory: self.inventory(),
                    });
                }
                Node::Core { .. } | Node::Defense { .. } => self.attack(i, &mut ev),
            }
        }
        self.finish_turn(ev)
    }

    /// A target utility (Kernel Rot, Secondary Vector) in slot `index` used
    /// on the revealed defense or core at `coord`.
    pub fn use_on_tile(&mut self, index: usize, coord: Coord) -> Vec<Event> {
        if self.outcome != Outcome::Running {
            return vec![Event::Ack];
        }
        let Some(Slot::Held {
            id,
            kind,
            active: None,
        }) = self.slots.get(index).copied()
        else {
            return vec![Event::Ack];
        };
        let Some(i) = self.board.at(coord) else {
            return vec![Event::Ack];
        };
        let t = &self.board.tiles[i];
        if kind.on_virus() || t.hidden || !t.node.is_target() {
            return vec![Event::Ack];
        }
        let mut ev = Vec::new();
        match kind {
            Utility::KernelRot => {
                let c = t.node.coherence();
                self.board.tiles[i].node.set_coherence(c - c / 2);
                ev.push(Event::KernelRot(self.board.tiles[i].record()));
                self.slots[index] = Slot::Empty;
                ev.push(Event::UtilityRemoved {
                    index,
                    inventory: self.inventory(),
                });
            }
            _ => {
                self.slots[index] = Slot::Held {
                    id,
                    kind,
                    active: Some(Active {
                        remaining: self.rules.secondary_vector.1,
                        target: Some(coord),
                    }),
                };
                ev.push(Event::UtilityInUse { index, value: true });
            }
        }
        self.finish_turn(ev)
    }

    /// A virus utility (Self Repair, Polymorphic Shield) in slot `index`.
    pub fn use_on_virus(&mut self, index: usize) -> Vec<Event> {
        if self.outcome != Outcome::Running {
            return vec![Event::Ack];
        }
        let Some(Slot::Held {
            id,
            kind,
            active: None,
        }) = self.slots.get(index).copied()
        else {
            return vec![Event::Ack];
        };
        let remaining = match kind {
            Utility::SelfRepair => self.rules.self_repair.1,
            Utility::PolymorphicShield => self.rules.shield_hits,
            _ => return vec![Event::Ack],
        };
        self.slots[index] = Slot::Held {
            id,
            kind,
            active: Some(Active {
                remaining,
                target: None,
            }),
        };
        self.finish_turn(vec![Event::UtilityInUse { index, value: true }])
    }

    /// The player quit, or the game was interrupted: lost.
    pub fn quit(&mut self) -> Vec<Event> {
        if self.outcome != Outcome::Running {
            return Vec::new();
        }
        self.outcome = Outcome::Lost;
        vec![Event::GameLost]
    }

    // -- the rules ----------------------------------------------------------

    /// A hidden tile can be revealed iff a neighbour is a revealed segment.
    fn reachable(&self, i: usize) -> bool {
        self.board
            .adjacent(i)
            .iter()
            .any(|&j| self.board.tiles[j].passable())
    }

    fn reveal(&mut self, i: usize, ev: &mut Vec<Event>) {
        self.board.tiles[i].hidden = false;
        let t = &self.board.tiles[i];
        ev.push(Event::TileFlipped(t.record()));
        if t.node != Node::Segment {
            return;
        }
        let disrupted = self.board.adjacent(i).iter().any(|&j| {
            let n = &self.board.tiles[j].node;
            n.defense() == Some(Defense::Disruptor)
        });
        if disrupted {
            return;
        }
        if let Some(&d) = self.board.distances(t.coord).get(&self.board.core) {
            ev.push(Event::Distance {
                coord: t.coord,
                distance: i64::from(d).min(self.rules.distance_cap),
            });
        }
    }

    fn open_cache(&mut self, i: usize, ev: &mut Vec<Event>) {
        let t = &mut self.board.tiles[i];
        let Node::Cache(inner) = std::mem::replace(&mut t.node, Node::Segment) else {
            return;
        };
        t.node = *inner;
        t.max_coherence = t.node.coherence();
        ev.push(Event::CacheOpened(t.record()));
    }

    fn shield_slot(&self) -> Option<usize> {
        self.slots.iter().position(|s| {
            matches!(
                s,
                Slot::Held {
                    kind: Utility::PolymorphicShield,
                    active: Some(Active { remaining: 1.., .. }),
                    ..
                }
            )
        })
    }

    fn attack(&mut self, i: usize, ev: &mut Vec<Event>) {
        let node = &mut self.board.tiles[i].node;
        node.set_coherence(node.coherence() - self.strength);
        let survived = node.coherence() > 0;
        let counter = node.strength();
        let mut shield = None;
        if survived {
            if let Some(k) = self.shield_slot() {
                if let Slot::Held {
                    active: Some(a), ..
                } = &mut self.slots[k]
                {
                    a.remaining -= 1;
                    shield = Some(Event::ShieldHit {
                        index: k,
                        remaining: a.remaining,
                    });
                }
            } else {
                self.coherence = (self.coherence - counter).max(0);
            }
        }
        ev.push(Event::Attack {
            attacker: self.virus(),
            defender: self.board.tiles[i].record(),
        });
        ev.extend(shield);
        if !survived {
            self.kill(i, ev);
        } else if self.coherence == 0 {
            ev.push(Event::VirusKilled(self.virus()));
            ev.push(Event::GameLost);
            self.outcome = Outcome::Lost;
        }
    }

    /// A node at 0: a defense becomes a segment; the core wins the game.
    fn kill(&mut self, i: usize, ev: &mut Vec<Event>) {
        let t = &mut self.board.tiles[i];
        if matches!(t.node, Node::Core { .. }) {
            t.node.set_coherence(0);
            ev.push(Event::TileKilled(t.record()));
            ev.push(Event::GameWon);
            self.outcome = Outcome::Won;
        } else {
            t.node = Node::Segment;
            t.max_coherence = 0;
            ev.push(Event::TileKilled(t.record()));
        }
    }

    /// Hidden tiles next to a revealed, live defense are blocked; emit each
    /// change.
    fn update_blocks(&mut self, ev: &mut Vec<Event>) {
        for i in 0..self.board.tiles.len() {
            let t = &self.board.tiles[i];
            let blocked = t.hidden
                && self
                    .board
                    .adjacent(i)
                    .iter()
                    .any(|&j| self.board.tiles[j].live_defense().is_some());
            if blocked != t.blocked {
                self.board.tiles[i].blocked = blocked;
                ev.push(Event::TileBlocked {
                    coord: self.board.tiles[i].coord,
                    blocked,
                });
            }
        }
    }

    fn finish_turn(&mut self, mut ev: Vec<Event>) -> Vec<Event> {
        self.turns += 1;
        if self.outcome == Outcome::Running {
            self.update_blocks(&mut ev);
            self.end_of_turn(&mut ev);
        }
        if self.outcome == Outcome::Running {
            self.update_blocks(&mut ev);
        }
        ev
    }

    /// The turn's end, in order: timed utilities, Restoration heals, the
    /// Suppressors' effect, then expired utilities are removed [D order].
    fn end_of_turn(&mut self, ev: &mut Vec<Event>) {
        for k in 0..self.slots.len() {
            let Slot::Held {
                kind,
                active: Some(mut a),
                ..
            } = self.slots[k]
            else {
                continue;
            };
            match kind {
                Utility::SelfRepair if a.remaining > 0 => {
                    self.coherence = (self.coherence + self.rules.self_repair.0)
                        .min(self.coherence_cap.max(self.coherence));
                    a.remaining -= 1;
                    ev.push(Event::SelfRepair {
                        index: k,
                        remaining: a.remaining,
                        target: self.virus(),
                    });
                }
                Utility::SecondaryVector if a.remaining > 0 => {
                    let target = a.target.and_then(|c| self.board.at(c));
                    match target {
                        Some(i) if self.board.tiles[i].node.is_target() => {
                            let node = &mut self.board.tiles[i].node;
                            node.set_coherence(node.coherence() - self.rules.secondary_vector.0);
                            let dead = node.coherence() == 0;
                            a.remaining -= 1;
                            ev.push(Event::SecondaryVector {
                                tile: self.board.tiles[i].record(),
                                index: k,
                                remaining: a.remaining,
                            });
                            if dead {
                                a.remaining = 0;
                                self.kill(i, ev);
                            }
                        }
                        _ => a.remaining = 0,
                    }
                }
                _ => {}
            }
            if let Slot::Held { active, .. } = &mut self.slots[k] {
                *active = Some(a);
            }
            if self.outcome != Outcome::Running {
                return;
            }
        }

        let healers: Vec<usize> = (0..self.board.tiles.len())
            .filter(|&i| self.board.tiles[i].live_defense() == Some(Defense::Restoration))
            .collect();
        for h in healers {
            for j in 0..self.board.tiles.len() {
                let t = &self.board.tiles[j];
                if j == h || t.live_defense().is_none() {
                    continue;
                }
                let amount = self
                    .rules
                    .restoration_heal
                    .min(t.max_coherence - t.node.coherence());
                if amount > 0 {
                    let c = t.node.coherence();
                    self.board.tiles[j].node.set_coherence(c + amount);
                    ev.push(Event::RestorationHeal {
                        source: self.board.tiles[h].coord,
                        healed: self.board.tiles[j].record(),
                        amount,
                    });
                }
            }
        }

        let suppressors = self
            .board
            .tiles
            .iter()
            .filter(|t| t.live_defense() == Some(Defense::Suppressor))
            .count() as i64;
        let strength = (self.base_strength - suppressors * self.rules.suppressor_amount).max(1);
        if strength != self.strength {
            self.strength = strength;
            ev.push(Event::SuppressorStrength(self.virus()));
        }

        for k in 0..self.slots.len() {
            if let Slot::Held {
                active: Some(Active { remaining: 0, .. }),
                ..
            } = self.slots[k]
            {
                self.slots[k] = Slot::Empty;
                ev.push(Event::UtilityRemoved {
                    index: k,
                    inventory: self.inventory(),
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

/// A can's `hackingSecurityState` [V consts].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanState {
    Secure,
    BeingHacked,
    Hacked,
}

impl CanState {
    pub fn wire(self) -> i64 {
        match self {
            CanState::Secure => 0,
            CanState::BeingHacked => 1,
            CanState::Hacked => 2,
        }
    }
}

/// A game that ended this turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ended {
    pub can: i64,
    pub outcome: Outcome,
    /// The failure that destroys the can ([`Rules::failures_to_destroy`]).
    pub destroy: bool,
}

/// What one input produced: the events to push, and the game that ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub events: Vec<Event>,
    pub ended: Option<Ended>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Running {
    can: i64,
    game: Game,
}

/// The games by character, the cans' states and their failures. One game
/// per character, one hacker per can [A]. It is the world slot ([`SLOT`],
/// HK2), and carries the world side's bookkeeping beside the games: the
/// analyzers armed and not yet started, where each running game was
/// started from, who owns a hacked can, the game counter and the rules.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hacks {
    games: BTreeMap<i64, Running>,
    cans: BTreeMap<i64, CanState>,
    failures: BTreeMap<i64, u32>,
    /// Analyzers activated on a can whose game has not started, by
    /// character ([`check_activation`] arms, [`on_completed`] fires).
    armed: BTreeMap<i64, Armed>,
    /// Where each running game was started from, by character: what
    /// [`tick`] checks for an interruption.
    links: BTreeMap<i64, Link>,
    /// A hacked can's new owner, the hacker [I].
    owners: BTreeMap<i64, i64>,
    /// Games started so far: half of each board's seed.
    counter: u64,
    /// `None` is [`Rules::SHIPPED`].
    rules: Option<Rules>,
    /// Boards to use, in order, before generating any (tests, HK4's harness).
    scripted: VecDeque<Board>,
}

impl Hacks {
    pub fn can_state(&self, can: i64) -> CanState {
        self.cans.get(&can).copied().unwrap_or(CanState::Secure)
    }

    pub fn failures(&self, can: i64) -> u32 {
        self.failures.get(&can).copied().unwrap_or(0)
    }

    pub fn game(&self, character: i64) -> Option<&Game> {
        self.games.get(&character).map(|r| &r.game)
    }

    /// Start `game` on `can` for `character`. A game the character already
    /// runs is lost first, in the same batch: the client opens a new window
    /// only after the old game ended [V].
    pub fn start(&mut self, character: i64, can: i64, game: Game) -> Turn {
        let old = self.input(character, Game::quit);
        let mut events = old.events;
        events.extend(game.start());
        self.cans.insert(can, CanState::BeingHacked);
        self.games.insert(character, Running { can, game });
        Turn {
            events,
            ended: old.ended,
        }
    }

    pub fn click(&mut self, character: i64, coord: Coord) -> Turn {
        self.input_or_ack(character, |g| g.click(coord))
    }

    pub fn use_on_tile(&mut self, character: i64, index: usize, coord: Coord) -> Turn {
        self.input_or_ack(character, |g| g.use_on_tile(index, coord))
    }

    pub fn use_on_virus(&mut self, character: i64, index: usize) -> Turn {
        self.input_or_ack(character, |g| g.use_on_virus(index))
    }

    /// Quit, or any interruption (warp, range, dock, jump, disconnect,
    /// death): a failure.
    pub fn quit(&mut self, character: i64) -> Turn {
        self.input(character, Game::quit)
    }

    fn input_or_ack(&mut self, character: i64, f: impl FnOnce(&mut Game) -> Vec<Event>) -> Turn {
        if self.games.contains_key(&character) {
            self.input(character, f)
        } else {
            Turn {
                events: vec![Event::Ack],
                ended: None,
            }
        }
    }

    fn input(&mut self, character: i64, f: impl FnOnce(&mut Game) -> Vec<Event>) -> Turn {
        let Some(r) = self.games.get_mut(&character) else {
            return Turn {
                events: Vec::new(),
                ended: None,
            };
        };
        let events = f(&mut r.game);
        let outcome = r.game.outcome();
        if outcome == Outcome::Running {
            return Turn {
                events,
                ended: None,
            };
        }
        let Running { can, game } = self.games.remove(&character).expect("present above");
        let mut destroy = false;
        if outcome == Outcome::Won {
            self.cans.insert(can, CanState::Hacked);
        } else {
            let n = self.failures.entry(can).or_insert(0);
            *n += 1;
            destroy = *n >= game.rules().failures_to_destroy;
            if destroy {
                self.cans.remove(&can);
                self.failures.remove(&can);
            } else {
                self.cans.insert(can, CanState::Secure);
            }
        }
        Turn {
            events,
            ended: Some(Ended {
                can,
                outcome,
                destroy,
            }),
        }
    }

    /// The rules boards are generated with: [`Rules::SHIPPED`] unless
    /// [`Hacks::set_rules`] replaced them.
    pub fn rules(&self) -> &Rules {
        self.rules.as_ref().unwrap_or(&Rules::SHIPPED)
    }

    /// Generate every later board, and play every later game, by `rules`.
    pub fn set_rules(&mut self, rules: Rules) {
        self.rules = Some(rules);
    }

    /// Use `board` for the next game started instead of generating one
    /// (tests, and HK4's solver harness).
    pub fn script_next(&mut self, board: Board) {
        self.scripted.push_back(board);
    }

    /// Who owns a hacked can: the character who hacked it [I].
    pub fn owner(&self, can: i64) -> Option<i64> {
        self.owners.get(&can).copied()
    }

    /// The character whose game is on `can`, if any.
    pub fn hacker_of(&self, can: i64) -> Option<i64> {
        self.games
            .iter()
            .find(|(_, r)| r.can == can)
            .map(|(c, _)| *c)
    }

    /// The analyzer armed by `character`, if its game has not started.
    pub fn armed(&self, character: i64) -> Option<Armed> {
        self.armed.get(&character).copied()
    }

    /// The board for the next game on `can`: a scripted one, else one
    /// generated from `board_seed(can, counter)`.
    fn next_board(&mut self, tier: u8, can: i64) -> Board {
        let n = self.counter;
        self.counter += 1;
        match self.scripted.pop_front() {
            Some(b) => b,
            None => Board::generate(self.rules(), tier, board_seed(can, n)),
        }
    }
}

// ---------------------------------------------------------------------------
// The world side (HK2): the slot, the analyzer, the slim, the service
// ---------------------------------------------------------------------------

/// The registration row; the coordinator adds it to `world_ext::SLOTS`.
/// Builds empty and reads no deltas: a site is not persisted, so neither is
/// anything about its cans [A, R-HACK §2 "Persistence"].
pub const SLOT: SlotSpec = SlotSpec {
    name: "hacking",
    make: make_slot,
};

fn make_slot(_: &Content) -> Boxed {
    Box::new(Hacks::default())
}

/// `doHacking` (1738), the data and relic analyzers' effect: category 2, guid
/// `effects.TargetScan` [V `dogmaeffects`].
pub const EFFECT_DO_HACKING: i64 = 1738;
/// `hackable` (1927): 1 on every can a player may hack [V content].
pub const ATTR_HACKABLE: i64 = 1927;
/// `tierDifficulty` (1919), 1..4 [V content].
pub const ATTR_TIER_DIFFICULTY: i64 = 1919;
/// `requiredSkill1..3` (182, 183, 184) [V `dogmaattributes`]. A can names
/// its skill in the first; an analyzer carries its skills in any of the
/// three (an integrated analyzer has both).
pub const ATTR_REQUIRED_SKILLS: [i64; 3] = [182, 183, 184];
/// `maxRange` (54) [V].
pub const ATTR_MAX_RANGE: i64 = 54;
/// `virusCoherence`, `virusStrength`, `virusElementSlots` (1909..1911) [V].
pub const ATTR_VIRUS_COHERENCE: i64 = 1909;
pub const ATTR_VIRUS_STRENGTH: i64 = 1910;
pub const ATTR_VIRUS_SLOTS: i64 = 1911;
/// Archaeology (13278) [V]: a can that requires it is a relic can, and its
/// game has the relic style.
pub const SKILL_ARCHAEOLOGY: i64 = 13278;
/// The can's slim key: 0 secure, 1 being hacked, 2 hacked [V consts].
pub const KEY_SECURITY_STATE: &str = "hackingSecurityState";

/// **[A] H-HACK-1: the game opens at the end of the analyzer's one cycle**
/// (10 s for a Data Analyzer I [V attr 73]), not on activation. Falsified
/// by a human who sees TQ-like play open the window on activation; flip
/// this and [`tick`] starts the game as soon as the analyzer runs.
pub const START_ON_CYCLE_END: bool = true;

/// **[A]** How far past the analyzer's `maxRange` a hacker may drift
/// before the game is lost, metres. Falsified by a game lost to a drift the
/// client's range readout still shows inside range.
pub const RANGE_SLACK: f64 = 500.0;

/// The "can" of a GM's practice board (`StartNewGameInstance`): no can.
pub const PRACTICE_CAN: i64 = 0;

/// An analyzer activated on a can, waiting for its game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Armed {
    pub can: i64,
    pub ship_id: i64,
    pub module_id: i64,
    pub module_type_id: i64,
}

/// Where a running game was started from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Link {
    can: i64,
    ship_id: i64,
    module_id: i64,
    module_type_id: i64,
    system: i64,
}

/// How this turn's pushes leave: with the answer to the call that caused
/// them, or on the clock because nobody asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deliver {
    Answer,
    Clock,
}

fn send(w: &mut Registry, via: Deliver, p: impl Push + 'static) {
    match via {
        Deliver::Answer => w.push(p),
        Deliver::Clock => w.schedule(std::time::Duration::ZERO, p),
    }
}

/// One attribute of a loaded type, from the world's typedata.
fn type_attr(w: &Registry, type_id: i64, attribute_id: i64) -> Option<f64> {
    w.content()
        .try_get::<crate::typedata::Installed>()?
        .table()
        .get(type_id)?
        .attribute(attribute_id)
}

/// Whether a type is a hackable can: `hackable` = 1 [V]. Group 1207 and
/// group 306 alike: the attribute decides, not the group (R-HACK §6 [A]).
pub fn is_hackable_type(w: &Registry, type_id: i64) -> bool {
    type_attr(w, type_id, ATTR_HACKABLE).is_some_and(|v| v >= 0.5)
}

/// A placed hackable object, and its solar system: a site object whose type
/// is hackable, still in space.
pub fn can(w: &Registry, id: i64) -> Option<(crate::site::Placed, i64)> {
    let (p, system) = crate::site::object(w, id)?;
    (p.rock.is_none() && is_hackable_type(w, p.type_id)).then_some((p, system))
}

/// Whether `targeting::lock` may lock `id` from `system`.
pub fn is_lockable(w: &Registry, id: i64, system: i64) -> bool {
    can(w, id).is_some_and(|(_, s)| s == system)
}

/// An analyzer's attribute: the dogma sheet's modified value, else the
/// type's own.
fn module_value(w: &Registry, ship_id: i64, module_id: i64, type_id: i64, a: i64) -> Option<f64> {
    crate::dogma::module_attribute(w, ship_id, module_id, a)
        .filter(|v| *v != 0.0)
        .or_else(|| type_attr(w, type_id, a))
        .or_else(|| crate::modules::find_in(w.content(), type_id).and_then(|t| t.attribute(a)))
}

/// The surface-to-surface gap from `character`'s ship to `p`, the ship
/// carried to now first.
fn gap(w: &mut Registry, character: i64, p: &crate::site::Placed) -> Option<f64> {
    w.advance_character(character);
    let here = w.roster().get(character).copied()?;
    let ship_type = w.character(character).map_or(0, |c| c.ship_type_id);
    let hull = crate::destiny::hull::of_in(w.content(), ship_type);
    Some(
        crate::space_state::distance(here.motion.position, p.position)
            - p.radius
            - f64::from(hull.radius),
    )
}

fn custom_notify(text: &str) -> ServiceError {
    crate::modules::user_error("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

/// **What `modules::activate` asks before an analyzer starts** (effect
/// [`EFFECT_DO_HACKING`]), and on success arms the game.
///
/// The client checks only that *a* target is locked [V `smb:1541`], so the
/// rest is ours, each refusal **[A]** in its key:
///
/// - not a hackable can in this system -> `DeniedActivateTargetModuleDisallowed`;
/// - not locked -> `DeniedActivateTargetNotPresent` (a lock lost in flight);
/// - being hacked or hacked -> a `CustomNotify`;
/// - the can's `requiredSkill1` is none of the analyzer's -> the "not this
///   module" key (a relic can and a data analyzer) [I: the can's skill
///   decides, R-HACK §1.6];
/// - beyond the analyzer's modified `maxRange` (surface distance) ->
///   `TargetTooFar`.
pub fn check_activation(
    w: &mut Registry,
    character_id: i64,
    ship_id: i64,
    module: &crate::modules::ModuleType,
    module_id: i64,
    target_id: Option<i64>,
) -> Result<(), ServiceError> {
    use crate::modules::{type_arg, user_error};
    let disallowed = || {
        user_error(
            "DeniedActivateTargetModuleDisallowed",
            vec![("moduleTypeID", type_arg(module.type_id))],
        )
    };
    let here = w.roster().get(character_id).copied();
    let found = target_id.and_then(|t| can(w, t).map(|(p, s)| (t, p, s)));
    let Some((can_id, p, system)) = found else {
        return Err(disallowed());
    };
    if here.is_none_or(|h| h.solar_system_id != system) {
        return Err(disallowed());
    }
    if !w.locks().targets_of(ship_id).contains(&can_id) {
        return Err(user_error("DeniedActivateTargetNotPresent", vec![]));
    }
    let h = w.ext::<Hacks>();
    let other = h
        .armed
        .iter()
        .any(|(c, a)| *c != character_id && a.can == can_id);
    match h.can_state(can_id) {
        CanState::Hacked => return Err(custom_notify("That container has already been hacked.")),
        CanState::BeingHacked => {
            return Err(custom_notify("Someone is already hacking that container."));
        }
        CanState::Secure if other => {
            return Err(custom_notify("Someone is already hacking that container."));
        }
        CanState::Secure => {}
    }
    if let Some(need) = type_attr(w, p.type_id, ATTR_REQUIRED_SKILLS[0]).filter(|v| *v > 0.0) {
        let has = ATTR_REQUIRED_SKILLS.iter().any(|a| {
            module_value(w, ship_id, module_id, module.type_id, *a)
                .is_some_and(|v| (v - need).abs() < 0.5)
        });
        if !has {
            return Err(disallowed());
        }
    }
    let range = module_value(w, ship_id, module_id, module.type_id, ATTR_MAX_RANGE).unwrap_or(0.0);
    let gap = gap(w, character_id, &p).unwrap_or(f64::INFINITY);
    if gap > range {
        let name = crate::item::find_in(w.content(), p.type_id).map_or("container", |t| t.name);
        return Err(user_error(
            "TargetTooFar",
            crate::module_range::too_far_pairs(module.type_id, gap, name),
        ));
    }
    w.ext_mut::<Hacks>().armed.insert(
        character_id,
        Armed {
            can: can_id,
            ship_id,
            module_id,
            module_type_id: module.type_id,
        },
    );
    Ok(())
}

/// **The cycle-completion hook** (`mining::tick` hands every completed
/// cycle here before mining takes its own): an analyzer's first cycle
/// that ended starts its game ([`START_ON_CYCLE_END`]). Any other cycle is
/// not this module's.
pub fn on_completed(w: &mut Registry, done: &[Cycle]) {
    if !START_ON_CYCLE_END {
        return;
    }
    for c in done
        .iter()
        .filter(|c| c.effect_id == EFFECT_DO_HACKING && c.index == 0)
    {
        let Some(a) = w.ext::<Hacks>().armed(c.character_id) else {
            continue;
        };
        if a.module_id != c.module_id || Some(a.can) != c.target_id {
            continue;
        }
        w.ext_mut::<Hacks>().armed.remove(&c.character_id);
        begin(w, c.character_id, a, Deliver::Clock);
    }
}

/// Start the game an analyzer armed, if the hacker and the can are still
/// where they were and nobody else got there first.
fn begin(w: &mut Registry, character: i64, a: Armed, via: Deliver) {
    let Some(here) = w.roster().get(character).copied() else {
        return;
    };
    if here.ship_id != a.ship_id {
        return;
    }
    let Some((p, system)) = can(w, a.can) else {
        return;
    };
    if system != here.solar_system_id || w.ext::<Hacks>().can_state(a.can) != CanState::Secure {
        return;
    }
    let value = |w: &Registry, attr| {
        module_value(w, a.ship_id, a.module_id, a.module_type_id, attr).unwrap_or(0.0)
    };
    let virus = VirusStats::from_attributes(
        value(w, ATTR_VIRUS_COHERENCE),
        value(w, ATTR_VIRUS_STRENGTH),
        value(w, ATTR_VIRUS_SLOTS),
    );
    let tier = type_attr(w, p.type_id, ATTR_TIER_DIFFICULTY)
        .map_or(1, |v| v.round().clamp(1.0, 4.0) as u8);
    let style =
        if type_attr(w, p.type_id, ATTR_REQUIRED_SKILLS[0]) == Some(SKILL_ARCHAEOLOGY as f64) {
            Style::Archeology
        } else {
            Style::Hacking
        };
    let link = Link {
        can: a.can,
        ship_id: a.ship_id,
        module_id: a.module_id,
        module_type_id: a.module_type_id,
        system,
    };
    open(w, character, link, tier, style, virus, via);
    crate::ancient_rate::reached(w, p.type_id, system);
    crate::outln!(
        "[hack] {character} starts on can {} (type {}, tier {tier}, virus {}/{} x{})",
        a.can,
        p.type_id,
        virus.coherence,
        virus.strength,
        virus.slots
    );
}

/// Open a game for `character` on `link.can`: any game they run is lost
/// first, in the same batch [V: the client opens a window only after the
/// old game ended]; then the start batch, and the can's slim goes to 1.
fn open(
    w: &mut Registry,
    character: i64,
    link: Link,
    tier: u8,
    style: Style,
    virus: VirusStats,
    via: Deliver,
) {
    let old = w.ext_mut::<Hacks>().quit(character);
    if let Some(e) = old.ended {
        ended(w, character, e, via);
    }
    let h = w.ext_mut::<Hacks>();
    let board = h.next_board(tier, link.can);
    let game = Game::new(
        board,
        &h.rules().clone(),
        virus,
        style,
        Some(link.module_type_id),
    );
    h.links.insert(character, link);
    let new = h.start(character, link.can, game);
    let mut events = old.events;
    events.extend(new.events);
    send(
        w,
        via,
        TurnComplete {
            to: character,
            events,
        },
    );
    if link.can != PRACTICE_CAN {
        slim_changed(w, link.can, via);
    }
}

/// A player's input, from `hackingMgr`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Click(Coord),
    UseOnTile(usize, Coord),
    UseOnVirus(usize),
    Quit,
}

/// **One `hackingMgr` input**: the turn's events to the hacker, with the
/// answer, and the end of the game if it ended. Input with no game is one
/// `ACK` (quit with none is nothing).
pub fn input(w: &mut Registry, character: i64, i: Input) {
    let h = w.ext_mut::<Hacks>();
    let turn = match i {
        Input::Click(c) => h.click(character, c),
        Input::UseOnTile(n, c) => h.use_on_tile(character, n, c),
        Input::UseOnVirus(n) => h.use_on_virus(character, n),
        Input::Quit => h.quit(character),
    };
    settle(w, character, turn, Deliver::Answer);
}

/// A malformed input: one `ACK`, as the engine answers an invalid move.
pub fn ack(w: &mut Registry, character: i64) {
    w.push(TurnComplete {
        to: character,
        events: vec![Event::Ack],
    });
}

/// **The GM's practice board** (`StartNewGameInstance(None, ...)`, the
/// client's `/openhacking` [V]): a game on no can, with the virus the
/// client asked for, never interrupted and never counted.
pub fn practice(w: &mut Registry, character: i64, style: Style, tier: u8, virus: VirusStats) {
    let ship_id = w.roster().get(character).map_or(0, |p| p.ship_id);
    let system = w.roster().get(character).map_or(0, |p| p.solar_system_id);
    let link = Link {
        can: PRACTICE_CAN,
        ship_id,
        module_id: 0,
        module_type_id: 0,
        system,
    };
    open(
        w,
        character,
        link,
        tier.clamp(1, 4),
        style,
        virus,
        Deliver::Answer,
    );
}

fn settle(w: &mut Registry, character: i64, turn: Turn, via: Deliver) {
    if !turn.events.is_empty() {
        send(
            w,
            via,
            TurnComplete {
                to: character,
                events: turn.events,
            },
        );
    }
    if let Some(e) = turn.ended {
        ended(w, character, e, via);
    }
}

/// A game ended: the can is hacked (state 2, owner the hacker, its loot
/// rolled into it by `hacking_loot::on_won`), back to secure, or -- on the
/// failure that destroys it -- out of space.
fn ended(w: &mut Registry, character: i64, e: Ended, via: Deliver) {
    let h = w.ext_mut::<Hacks>();
    h.links.remove(&character);
    if e.can == PRACTICE_CAN {
        h.cans.remove(&PRACTICE_CAN);
        h.failures.remove(&PRACTICE_CAN);
        return;
    }
    crate::outln!(
        "[hack] {character} {} can {}{}",
        if e.outcome == Outcome::Won {
            "won"
        } else {
            "lost"
        },
        e.can,
        if e.destroy { "; it is destroyed" } else { "" }
    );
    match e.outcome {
        Outcome::Won => {
            h.owners.insert(e.can, character);
            crate::site::set_owner(w, e.can, character);
            // HK3: the can becomes the hacker's loot container.
            crate::hacking_loot::on_won(w, e.can, character);
            slim_changed(w, e.can, via);
        }
        _ if e.destroy => destroy(w, e.can, via),
        _ => slim_changed(w, e.can, via),
    }
}

/// **The second failure destroys the can** [D]: out of the site, out of
/// every park in the system (`RemoveBalls`; **[A]** no explosion FX), and
/// every lock on it closed as `Exploding`.
fn destroy(w: &mut Registry, can_id: i64, via: Deliver) {
    let Some((_, system)) = can(w, can_id) else {
        return;
    };
    crate::site::destroy_object(w, can_id);
    w.ext_mut::<Hacks>().owners.remove(&can_id);
    let to = crate::dungeon::audience(w, system);
    let stamp = w.next_destiny_stamp();
    if let Some(p) = crate::destiny::BallsRemoved::new(to, stamp, vec![can_id]) {
        send(w, via, p);
    }
    for locker_ship in w.locks().targeters_of(can_id) {
        let Some(locker) = w.roster().by_ship(locker_ship).map(|p| p.character_id) else {
            w.locks_mut().remove(locker_ship, can_id);
            continue;
        };
        let unlock = |w: &mut Registry| {
            crate::targeting::unlock(w, locker, can_id, crate::targeting::Reason::Exploding)
        };
        match via {
            Deliver::Answer => unlock(w),
            Deliver::Clock => on_clock(w, unlock),
        }
    }
}

/// Run `f`, and send what it put on `pending` from the clock instead
/// (`targeting::unlock` answers through `pending`, which only a payload
/// drains); what was there before goes back untouched.
fn on_clock(w: &mut Registry, f: impl FnOnce(&mut Registry)) {
    use crate::notify::Addressed;
    let before = w.take_pending();
    f(w);
    let ours = w.take_pending();
    for (after, p) in before {
        w.push_later(after, Addressed { to: None, inner: p });
    }
    for (_, p) in ours {
        w.schedule(std::time::Duration::ZERO, Addressed { to: None, inner: p });
    }
}

/// `OnSlimItemChange` for `can_id` to everyone in its system: its slim with
/// every source's extras, `hackingSecurityState` among them.
fn slim_changed(w: &mut Registry, can_id: i64, via: Deliver) {
    let Some((p, system)) = can(w, can_id) else {
        return;
    };
    let to = crate::dungeon::audience(w, system);
    if to.is_empty() {
        return;
    }
    let extras: Vec<(&'static str, PyValue)> = crate::slim_extra::for_slims(w, &[can_id])
        .into_iter()
        .filter(|(id, _)| *id == can_id)
        .flat_map(|(_, pairs)| pairs)
        .collect();
    let stamp = w.next_destiny_stamp();
    send(
        w,
        via,
        CanSlimChange {
            to,
            stamp,
            slim: crate::site::slim(&p),
            extras,
        },
    );
}

/// The can's state on the wire, for the slim builders: `None` for
/// anything that is not a placed hackable object.
pub fn security_state(w: &Registry, id: i64) -> Option<i64> {
    can(w, id)?;
    Some(w.ext::<Hacks>().can_state(id).wire())
}

/// A site object's slim item, changed: `OnSlimItemChange(itemID, newSlim)`
/// in a `DoDestinyUpdate`, the slim in its **instance** form (`foo.SlimItem`)
/// as `destiny::SlimItemChange` sends a wreck's [V `michelle.py:2006`
/// stores it as it is]. A sibling of that push rather than a widening of it:
/// the wreck's is built in modules this lane does not own.
///
/// `inSpaceBracket` reads `hackingSecurityState` from it and picks the
/// scatter icon; `inventorysvc.OnSlimItemChange` drops the floating-cargo
/// location cache when the state is not 2 [V].
#[derive(Debug, Clone, PartialEq)]
pub struct CanSlimChange {
    pub to: Vec<i64>,
    pub stamp: u32,
    pub slim: crate::destiny::Slim,
    pub extras: Vec<(&'static str, PyValue)>,
}

impl CanSlimChange {
    /// The slim as the client stores it.
    pub fn slim_value(&self) -> PyValue {
        let opt = |v: Option<i64>| v.map_or(PyValue::None, PyValue::Int);
        let mut pairs = vec![
            ("itemID", PyValue::Int(self.slim.item_id)),
            ("typeID", PyValue::Int(self.slim.type_id)),
            ("ownerID", PyValue::Int(self.slim.owner_id)),
            ("charID", opt(self.slim.char_id)),
            ("corpID", opt(self.slim.corp_id)),
        ];
        pairs.extend(self.extras.iter().cloned());
        crate::pyutil::instance("foo.SlimItem", pairs)
    }
}

impl Push for CanSlimChange {
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let entry = PyValue::Tuple(vec![
            PyValue::Int(i64::from(self.stamp)),
            PyValue::Tuple(vec![
                crate::pyutil::bstr("OnSlimItemChange"),
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

/// Why a running game is lost, if it is: **[A]** each of these ends the game
/// as a failure (R-HACK §2 "Quit / interruption"; falsified by TQ footage
/// of a game surviving one).
fn interrupted(w: &mut Registry, character: i64, l: &Link) -> Option<&'static str> {
    if l.can == PRACTICE_CAN {
        return None;
    }
    let Some(here) = w.roster().get(character).copied() else {
        return Some("left space");
    };
    if here.ship_id != l.ship_id {
        return Some("changed ship");
    }
    if here.solar_system_id != l.system {
        return Some("left the system");
    }
    if here.motion.warp_active() {
        return Some("warped");
    }
    let Some((p, _)) = can(w, l.can) else {
        return Some("the can is gone");
    };
    if !w.locks().targets_of(l.ship_id).contains(&l.can) {
        return Some("lost the lock");
    }
    let range =
        module_value(w, l.ship_id, l.module_id, l.module_type_id, ATTR_MAX_RANGE).unwrap_or(0.0);
    if gap(w, character, &p).is_none_or(|g| g > range + RANGE_SLACK) {
        return Some("out of range");
    }
    None
}

/// **The stage** (after `mining`, which hands this module its completed
/// cycles first):
///
/// - an armed analyzer that stopped without its cycle completing (lock
///   lost, capacitor, a dock) is forgotten; with [`START_ON_CYCLE_END`]
///   off, one that runs starts its game here;
/// - a game whose hacker left space, changed ship or system, warped, lost
///   the lock or drifted out of range, or whose can is gone, is lost;
/// - state about cans no longer in space is dropped, and their loot
///   (`hacking_loot::tick`).
pub fn tick(w: &mut Registry, _now: std::time::Instant, _ft: BlueTime) {
    let armed: Vec<(i64, Armed)> = w
        .ext::<Hacks>()
        .armed
        .iter()
        .map(|(c, a)| (*c, *a))
        .collect();
    for (c, a) in armed {
        let runs = w
            .modules()
            .running(a.module_id)
            .is_some_and(|r| r.effect.effect_id == EFFECT_DO_HACKING && r.character_id == c);
        if !runs {
            w.ext_mut::<Hacks>().armed.remove(&c);
        } else if !START_ON_CYCLE_END {
            w.ext_mut::<Hacks>().armed.remove(&c);
            begin(w, c, a, Deliver::Clock);
        }
    }
    let links: Vec<(i64, Link)> = w
        .ext::<Hacks>()
        .links
        .iter()
        .map(|(c, l)| (*c, *l))
        .collect();
    for (c, l) in links {
        if let Some(why) = interrupted(w, c, &l) {
            crate::outln!("[hack] {c} {why}: the game on can {} is lost", l.can);
            let turn = w.ext_mut::<Hacks>().quit(c);
            settle(w, c, turn, Deliver::Clock);
        }
    }
    let gone: Vec<i64> = {
        let h = w.ext::<Hacks>();
        h.cans
            .keys()
            .chain(h.failures.keys())
            .chain(h.owners.keys())
            .copied()
            .filter(|id| *id != PRACTICE_CAN && h.hacker_of(*id).is_none())
            .filter(|id| crate::site::object(w, *id).is_none())
            .collect()
    };
    let h = w.ext_mut::<Hacks>();
    for id in gone {
        h.cans.remove(&id);
        h.failures.remove(&id);
        h.owners.remove(&id);
    }
    // HK3: the loot of cans no longer in space goes with them.
    crate::hacking_loot::tick(w);
}

// ---------------------------------------------------------------------------
// Tests: every number invented; no [A] value appears in one.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pyutil::dict_get;

    /// Invented rules: none of these is a shipped value's claim.
    fn rules() -> Rules {
        Rules {
            dims: [(5, 4), (6, 5), (7, 6), (9, 8)],
            hole_percent: 20,
            defenses: [2, 3, 4, 6],
            caches: [1, 2, 3, 3],
            utilities: [1, 2, 2, 3],
            defense_weights: [3, 3, 2, 2, 2],
            basic_defense_max_tier: 1,
            defense_stats: [
                NodeStats::flat(17, 3),
                NodeStats::flat(9, 6),
                NodeStats::flat(13, 2),
                NodeStats::flat(11, 2),
                NodeStats::flat(12, 2),
            ],
            core_level: [
                CoreLevel::Low,
                CoreLevel::Low,
                CoreLevel::Medium,
                CoreLevel::High,
            ],
            core_stats: [(7, 5), (19, 7), (29, 9)],
            cache_weights: [1, 1, 1],
            utility_weights: [1, 1, 1, 1],
            min_start_distance: 2,
            restoration_heal: 4,
            suppressor_amount: 2,
            secondary_vector: (6, 2),
            self_repair: (5, 2),
            shield_hits: 2,
            distance_cap: 4,
            failures_to_destroy: 3,
        }
    }

    fn virus(coh: i64, str: i64) -> VirusStats {
        VirusStats {
            coherence: coh,
            strength: str,
            slots: 3,
        }
    }

    fn def(kind: Defense, coherence: i64, strength: i64) -> Node {
        Node::Defense {
            kind,
            coherence,
            strength,
        }
    }

    fn core(coherence: i64, strength: i64) -> Node {
        Node::Core {
            level: CoreLevel::Low,
            coherence,
            strength,
        }
    }

    /// A straight even row: start at (0,0), then `nodes` at (1,0), (2,0)...
    fn row(nodes: Vec<Node>) -> Board {
        let mut cells = vec![((0, 0), Node::Segment)];
        cells.extend(
            nodes
                .into_iter()
                .enumerate()
                .map(|(i, n)| ((i as i32 + 1, 0), n)),
        );
        Board::from_layout(GRID_MAX_COLUMNS, 3, (0, 0), cells).expect("layout")
    }

    fn game(board: Board, v: VirusStats) -> Game {
        Game::new(board, &rules(), v, Style::Hacking, Some(4242))
    }

    fn keys(v: &PyValue) -> Vec<String> {
        let PyValue::Dict(pairs) = v else {
            panic!("not a dict: {v:?}")
        };
        pairs
            .iter()
            .map(|(_, k)| match k {
                PyValue::Bytes(b) => String::from_utf8(b.clone()).unwrap(),
                other => panic!("key is not a byte string: {other:?}"),
            })
            .collect()
    }

    /// Walk every dict in `v`: each key a byte string, each tile-shaped
    /// dict with only tile keys, each slot dict without `index`.
    fn check_shapes(v: &PyValue) {
        match v {
            PyValue::Dict(pairs) => {
                let ks = keys(v);
                // A tile record, but not a SECONDARYVECTOR update, which
                // goes through `TileData.Update` and may add keys [V].
                let update = ks.iter().any(|k| k == "durationRemaining");
                if ks.iter().any(|k| k == "hidden") && !update {
                    for k in &ks {
                        assert!(TILE_KEYS.contains(&k.as_str()), "tile key {k}");
                    }
                }
                if let Some(PyValue::List(slots)) = dict_get(v, b"inventory") {
                    for s in slots {
                        let sk = keys(s);
                        assert!(!sk.iter().any(|k| k == "index"), "slot with index");
                        for k in &sk {
                            assert!(SLOT_KEYS.contains(&k.as_str()), "slot key {k}");
                        }
                    }
                }
                if let Some(c) = dict_get(v, b"coord") {
                    assert!(
                        matches!(c, PyValue::Tuple(t) if t.len() == 2),
                        "coord {c:?}"
                    );
                }
                for (val, _) in pairs {
                    check_shapes(val);
                }
            }
            PyValue::List(xs) | PyValue::Tuple(xs) => xs.iter().for_each(check_shapes),
            _ => {}
        }
    }

    fn ids(ev: &[Event]) -> Vec<i64> {
        ev.iter().map(Event::id).collect()
    }

    // 16. Neighbour offsets match odd-r [V].
    #[test]
    fn neighbour_offsets_are_odd_r() {
        assert_eq!(
            neighbours((3, 1)),
            [(3, 0), (4, 0), (2, 1), (4, 1), (3, 2), (4, 2)]
        );
        assert_eq!(
            neighbours((3, 2)),
            [(2, 1), (3, 1), (2, 2), (4, 2), (2, 3), (3, 3)]
        );
        // Symmetric: every neighbour has us as a neighbour.
        for y in 0..GRID_MAX_ROWS {
            for x in 0..GRID_MAX_COLUMNS {
                for n in neighbours((x, y)) {
                    assert!(neighbours(n).contains(&(x, y)), "{:?} {:?}", (x, y), n);
                }
            }
        }
    }

    // 1. The start batch order.
    #[test]
    fn start_batch_order() {
        let g = game(row(vec![Node::Segment, core(7, 1)]), virus(50, 5));
        let ev = g.start();
        let n = g.board.tiles.len();
        assert_eq!(ev.len(), n + 3);
        assert_eq!(ev[0], Event::GameStyle(Style::Hacking));
        assert!(ev[1..=n].iter().all(|e| e.id() == EVENT_TILE_CREATED));
        assert_eq!(ev[n + 1].id(), EVENT_VIRUS_CREATED);
        assert_eq!(
            ev[n + 2],
            Event::GameStart {
                module_type_id: Some(4242)
            }
        );
        let v = ev[n + 1].to_pyvalue();
        let data = dict_get(&v, b"eventData").unwrap();
        let Some(PyValue::List(inv)) = dict_get(data, b"inventory") else {
            panic!()
        };
        assert_eq!(inv.len(), 3);
        assert_eq!(inv[0], SlotRec::EMPTY.to_pyvalue());
    }

    // 2. A TILE_CREATED dict has exactly the 8 keys; slots never hold `index`.
    #[test]
    fn tile_and_slot_key_sets() {
        let b = Board::generate(&rules(), 4, 77);
        let g = game(b, virus(50, 5));
        for e in g.start() {
            let v = e.to_pyvalue();
            assert_eq!(keys(&v), vec!["eventID", "eventData"]);
            assert!(matches!(dict_get(&v, b"eventID"), Some(PyValue::Int(_))));
            if e.id() == EVENT_TILE_CREATED {
                let mut k = keys(dict_get(&v, b"eventData").unwrap());
                k.sort();
                let mut want: Vec<String> = TILE_KEYS.iter().map(|s| s.to_string()).collect();
                want.sort();
                assert_eq!(k, want, "a TILE_CREATED with a key outside the eight");
            }
            check_shapes(&v);
        }
        let held = SlotRec {
            id: Some(3),
            subtype: 2,
            info: Some(9),
        };
        let mut k = keys(&held.to_pyvalue());
        k.sort();
        assert_eq!(k, vec!["id", "info", "subtype"]);
        let removed = Event::UtilityRemoved {
            index: 1,
            inventory: vec![held, SlotRec::EMPTY],
        };
        check_shapes(&removed.to_pyvalue());
        // UE_REMOVED nests the inventory [V].
        let d = removed.data();
        let nested = dict_get(&d, b"inventory").unwrap();
        assert!(matches!(dict_get(nested, b"inventory"), Some(PyValue::List(l)) if l.len() == 2));
    }

    // 3. A hidden tile leaks nothing.
    #[test]
    fn hidden_tile_leaks_nothing() {
        let g = game(
            row(vec![def(Defense::Firewall, 9, 9), core(7, 1)]),
            virus(50, 5),
        );
        let rec = g.board.tile((1, 0)).unwrap().record();
        assert_eq!(
            (
                rec.kind,
                rec.subtype,
                rec.strength,
                rec.coherence,
                rec.hidden
            ),
            (TYPE_NONE, SUBTYPE_NONE, 0, 0, true)
        );
        let start = g.board.tile((0, 0)).unwrap().record();
        assert_eq!((start.kind, start.hidden), (TYPE_SEGMENT, false));
    }

    // 4. A non-adjacent or blocked tile: ACK, nothing changes.
    #[test]
    fn invalid_clicks_ack() {
        let mut g = game(
            row(vec![
                def(Defense::Firewall, 9, 1),
                Node::Segment,
                core(7, 1),
            ]),
            virus(50, 5),
        );
        let before = g.clone();
        assert_eq!(g.click((3, 0)), vec![Event::Ack]);
        assert_eq!(g.click((0, 0)), vec![Event::Ack]); // a revealed segment
        assert_eq!(g.click((8, 8)), vec![Event::Ack]); // off the board
        assert_eq!(g, before);
        g.click((1, 0)); // reveal the firewall: (2,0) becomes blocked
        assert!(g.board.tile((2, 0)).unwrap().blocked);
        let before = g.clone();
        assert_eq!(g.click((2, 0)), vec![Event::Ack]);
        assert_eq!(g, before);
    }

    // 5. Revealing a defense blocks its hidden neighbours; killing it unblocks.
    #[test]
    fn defense_blocks_and_unblocks() {
        let mut g = game(
            row(vec![def(Defense::AntiVirus, 4, 1), core(7, 1)]),
            virus(50, 5),
        );
        let ev = g.click((1, 0));
        assert_eq!(
            ev,
            vec![
                Event::TileFlipped(g.board.tile((1, 0)).unwrap().record()),
                Event::TileBlocked {
                    coord: (2, 0),
                    blocked: true
                },
            ]
        );
        let ev = g.click((1, 0));
        assert_eq!(
            ids(&ev),
            vec![EVENT_ATTACK, EVENT_OBJECT_KILLED, EVENT_TILE_BLOCKED]
        );
        assert_eq!(
            ev[2],
            Event::TileBlocked {
                coord: (2, 0),
                blocked: false
            }
        );
        assert_eq!(g.board.tile((1, 0)).unwrap().node, Node::Segment);
    }

    // 6. Attack arithmetic: counter-hit only if the node survives.
    #[test]
    fn attack_arithmetic() {
        let mut g = game(
            row(vec![def(Defense::Firewall, 12, 4), core(7, 1)]),
            virus(50, 5),
        );
        g.click((1, 0));
        let ev = g.click((1, 0));
        let Event::Attack { attacker, defender } = ev[0] else {
            panic!("{ev:?}")
        };
        assert_eq!((defender.coherence, attacker.coherence), (7, 46));
        g.click((1, 0)); // 7 -> 2, virus 46 -> 42
        let ev = g.click((1, 0)); // 2 -> 0: dead, no counter-hit
        let Event::Attack { attacker, defender } = ev[0] else {
            panic!("{ev:?}")
        };
        assert_eq!((defender.coherence, attacker.coherence), (0, 42));
        assert_eq!(ev[1].id(), EVENT_OBJECT_KILLED);
    }

    fn with_utility(nodes: Vec<Node>, u: Utility, v: VirusStats) -> Game {
        // (1,0) holds the utility, picked up on the second click.
        let mut all = vec![Node::Utility(u)];
        all.extend(nodes);
        let mut g = game(row(all), v);
        g.click((1, 0));
        let ev = g.click((1, 0));
        assert_eq!(ev[0].id(), EVENT_UE_PICKEDUP, "{ev:?}");
        g
    }

    // 7. Kernel Rot halves; a Secondary Vector ticks n turns, then UE_REMOVED.
    #[test]
    fn kernel_rot_and_secondary_vector() {
        let mut g = with_utility(
            vec![def(Defense::Firewall, 15, 1), core(7, 1)],
            Utility::KernelRot,
            virus(50, 5),
        );
        g.click((2, 0));
        let ev = g.use_on_tile(0, (2, 0));
        assert_eq!(ids(&ev), vec![EVENT_KERNALROT, EVENT_UE_REMOVED]);
        assert_eq!(g.board.tile((2, 0)).unwrap().node.coherence(), 8);
        assert_eq!(g.inventory()[0], SlotRec::EMPTY);
        assert_eq!(g.use_on_tile(0, (2, 0)), vec![Event::Ack]);

        let mut g = with_utility(
            vec![def(Defense::Firewall, 30, 1), core(7, 1)],
            Utility::SecondaryVector,
            virus(50, 5),
        );
        g.click((2, 0));
        assert_eq!(g.inventory()[0].info, Some(rules().secondary_vector.0));
        let ev = g.use_on_tile(0, (2, 0));
        assert_eq!(ids(&ev), vec![EVENT_UE_INUSE, EVENT_SECONDARYVECTOR]);
        assert_eq!(g.board.tile((2, 0)).unwrap().node.coherence(), 24);
        let ev = g.click((2, 0)); // attack 24 -> 19, then the last tick -> 13
        assert_eq!(
            ids(&ev),
            vec![EVENT_ATTACK, EVENT_SECONDARYVECTOR, EVENT_UE_REMOVED]
        );
        assert_eq!(g.board.tile((2, 0)).unwrap().node.coherence(), 13);
        check_shapes(&ev[1].to_pyvalue());
        let Event::UtilityRemoved { inventory, .. } = &ev[2] else {
            panic!()
        };
        assert_eq!(inventory[0], SlotRec::EMPTY);
    }

    // 8. The shield absorbs n counter-hits.
    #[test]
    fn shield_absorbs_hits() {
        let mut g = with_utility(
            vec![def(Defense::Firewall, 100, 7), core(7, 1)],
            Utility::PolymorphicShield,
            virus(50, 5),
        );
        g.click((2, 0));
        g.use_on_virus(0);
        let ev = g.click((2, 0));
        assert_eq!(ids(&ev), vec![EVENT_ATTACK, EVENT_POLYMORPHICSHIELDHIT]);
        assert_eq!(g.virus().coherence, 50);
        let ev = g.click((2, 0));
        assert_eq!(
            ids(&ev),
            vec![EVENT_ATTACK, EVENT_POLYMORPHICSHIELDHIT, EVENT_UE_REMOVED]
        );
        assert_eq!(g.virus().coherence, 50);
        g.click((2, 0));
        assert_eq!(g.virus().coherence, 43);
    }

    // 9. Restoration heals the others, not itself.
    #[test]
    fn restoration_heals_others() {
        // The firewall at (1,0); the healer at (0,3), reached down the
        // left edge without touching the firewall's neighbours.
        let cells = vec![
            ((0, 0), Node::Segment),
            ((1, 0), def(Defense::Firewall, 20, 1)),
            ((0, 1), Node::Segment),
            ((0, 2), Node::Segment),
            ((0, 3), def(Defense::Restoration, 10, 1)),
            ((5, 4), core(7, 1)),
        ];
        let b = Board::from_layout(6, 5, (0, 0), cells).unwrap();
        let mut g = game(b, virus(50, 5));
        g.click((0, 1));
        g.click((0, 2));
        g.click((1, 0));
        g.click((1, 0)); // firewall 20 -> 15
        let ev = g.click((0, 3)); // reveal the healer: heals the firewall
        let heal = ev
            .iter()
            .find(|e| e.id() == EVENT_HONEYPOT_HEALING)
            .expect("a heal");
        let Event::RestorationHeal {
            source,
            healed,
            amount,
        } = heal
        else {
            panic!()
        };
        assert_eq!(
            (*source, healed.coord, healed.coherence, *amount),
            ((0, 3), (1, 0), 19, 4)
        );
        g.click((0, 3)); // healer 10 -> 5; firewall to its max 20
        assert_eq!(g.board.tile((1, 0)).unwrap().node.coherence(), 20);
        assert_eq!(g.board.tile((0, 3)).unwrap().node.coherence(), 5);
    }

    // 10. The Suppressor lowers strength while alive and restores it on death.
    #[test]
    fn suppressor_strength() {
        let mut g = game(
            row(vec![def(Defense::Suppressor, 4, 1), core(7, 1)]),
            virus(50, 5),
        );
        let ev = g.click((1, 0));
        assert_eq!(
            ev.iter().find(|e| e.id() == EVENT_HONEYPOT_STRENGTH),
            Some(&Event::SuppressorStrength(VirusRec {
                strength: 3,
                coherence: 50
            }))
        );
        g.click((1, 0)); // 4 -> 1
        let ev = g.click((1, 0)); // dead
        assert!(ev.contains(&Event::SuppressorStrength(VirusRec {
            strength: 5,
            coherence: 49
        })));
    }

    // 11. Core death wins; virus death loses (a full scripted win and loss).
    #[test]
    fn scripted_win() {
        let cells = vec![
            ((0, 0), Node::Segment),
            (
                (1, 0),
                Node::Cache(Box::new(Node::Utility(Utility::SelfRepair))),
            ),
            ((2, 0), Node::Segment),
            ((3, 0), core(12, 3)),
        ];
        let b = Board::from_layout(5, 2, (0, 0), cells).unwrap();
        let mut h = Hacks::default();
        let t = h.start(9001, 555, game(b, virus(20, 5)));
        assert_eq!(t.ended, None);
        assert_eq!(h.can_state(555), CanState::BeingHacked);
        let mut all = t.events;
        all.extend(h.click(9001, (1, 0)).events); // reveal the cache
        let t = h.click(9001, (1, 0)); // open: a utility
        assert_eq!(ids(&t.events), vec![EVENT_DATACACHE_OPEN]);
        all.extend(t.events);
        let t = h.click(9001, (1, 0)); // pick up
        assert_eq!(ids(&t.events), vec![EVENT_UE_PICKEDUP]);
        all.extend(t.events);
        let t = h.click(9001, (2, 0)); // reveal a segment: a distance
        assert_eq!(
            t.events[1],
            Event::Distance {
                coord: (2, 0),
                distance: 1
            }
        );
        all.extend(t.events);
        all.extend(h.click(9001, (3, 0)).events); // reveal the core
        all.extend(h.click(9001, (3, 0)).events); // 12 -> 7, virus 17
        all.extend(h.click(9001, (3, 0)).events); // 7 -> 2, virus 14
        all.extend(h.use_on_virus(9001, 0).events); // repair 14 -> 19
        let t = h.click(9001, (3, 0)); // 2 -> 0
        assert_eq!(
            ids(&t.events),
            vec![EVENT_ATTACK, EVENT_OBJECT_KILLED, EVENT_GAME_WON]
        );
        assert_eq!(
            t.ended,
            Some(Ended {
                can: 555,
                outcome: Outcome::Won,
                destroy: false
            })
        );
        all.extend(t.events);
        assert_eq!(h.can_state(555), CanState::Hacked);
        assert!(h.game(9001).is_none());
        for e in &all {
            check_shapes(&e.to_pyvalue());
        }
        // Inputs after the end: ACK.
        assert_eq!(h.click(9001, (3, 0)).events, vec![Event::Ack]);
    }

    #[test]
    fn scripted_loss() {
        let mut g = game(row(vec![core(100, 9)]), virus(20, 5));
        g.click((1, 0));
        g.click((1, 0)); // virus 11
        g.click((1, 0)); // virus 2
        let ev = g.click((1, 0)); // virus 0
        assert_eq!(
            ids(&ev),
            vec![EVENT_ATTACK, EVENT_OBJECT_KILLED, EVENT_GAME_LOST]
        );
        assert_eq!(
            ev[1],
            Event::VirusKilled(VirusRec {
                strength: 5,
                coherence: 0
            })
        );
        assert_eq!(g.outcome(), Outcome::Lost);
    }

    // 12. Quit sends GAME_LOST; failures accumulate to destroying the can.
    #[test]
    fn quit_loses_and_failures_destroy() {
        let mk = || game(row(vec![core(7, 1)]), virus(20, 5));
        let mut h = Hacks::default();
        let destroy_at = rules().failures_to_destroy;
        for n in 1..=destroy_at {
            h.start(1, 77, mk());
            let t = h.quit(1);
            assert_eq!(t.events, vec![Event::GameLost]);
            let ended = t.ended.unwrap();
            assert_eq!(ended.destroy, n == destroy_at);
            if n < destroy_at {
                assert_eq!((h.can_state(77), h.failures(77)), (CanState::Secure, n));
            }
        }
        assert_eq!(h.failures(77), 0);
        assert_eq!(h.quit(1).events, Vec::<Event>::new());
        assert_eq!(h.click(1, (1, 0)).events, vec![Event::Ack]);
    }

    // 13. A second start for the same character first ends the old game.
    #[test]
    fn second_start_ends_the_old_game() {
        let mk = || game(row(vec![core(7, 1)]), virus(20, 5));
        let mut h = Hacks::default();
        h.start(1, 10, mk());
        let t = h.start(1, 11, mk());
        assert_eq!(t.events[0], Event::GameLost);
        assert_eq!(t.events[1].id(), EVENT_GAME_STYLE);
        assert_eq!(t.ended.map(|e| e.can), Some(10));
        assert_eq!(h.failures(10), 1);
        assert_eq!(h.can_state(11), CanState::BeingHacked);
    }

    // 14. A generated board: connected, in bounds, one core, nothing at
    // distance 1, only drawable defenses (caches included).
    #[test]
    fn generated_board_invariants() {
        let r = rules();
        for tier in 1..=4u8 {
            for seed in 0..60u64 {
                let b = Board::generate(&r, tier, seed);
                assert!(b.cols <= GRID_MAX_COLUMNS && b.rows <= GRID_MAX_ROWS);
                let d = b.distances(b.start);
                assert_eq!(d.len(), b.tiles.len(), "connected");
                assert!(d.contains_key(&b.core), "core reachable");
                let cores = b
                    .tiles
                    .iter()
                    .filter(|t| matches!(t.node, Node::Core { .. }))
                    .count();
                assert_eq!(cores, 1);
                for t in &b.tiles {
                    assert!(
                        t.coord.0 < b.cols
                            && t.coord.1 < b.rows
                            && t.coord.0 >= 0
                            && t.coord.1 >= 0
                    );
                    assert_eq!(t.hidden, t.coord != b.start);
                    if d[&t.coord] < r.min_start_distance {
                        assert_eq!(t.node, Node::Segment, "{:?} too close", t.coord);
                    }
                    let inner = match &t.node {
                        Node::Cache(n) => n.as_ref(),
                        n => n,
                    };
                    if let Node::Defense { kind, .. } = inner {
                        assert!(DRAWABLE_DEFENSE_SUBTYPES.contains(&kind.subtype()));
                        if tier <= r.basic_defense_max_tier {
                            assert!(matches!(kind, Defense::Firewall | Defense::AntiVirus));
                        }
                    }
                }
                assert_eq!(b.start.0, 0);
            }
        }
    }

    // 15. Determinism.
    #[test]
    fn seeds_are_deterministic() {
        let r = rules();
        assert_eq!(Board::generate(&r, 3, 5), Board::generate(&r, 3, 5));
        assert_ne!(Board::generate(&r, 3, 5), Board::generate(&r, 3, 6));
        assert_ne!(board_seed(1, 0), board_seed(1, 1));
        assert_ne!(board_seed(1, 0), board_seed(2, 0));
    }

    #[test]
    fn disruptor_hides_the_distance() {
        let cells = vec![
            ((0, 0), Node::Segment),
            ((1, 0), Node::Segment),
            ((1, 1), def(Defense::Disruptor, 5, 1)),
            ((3, 0), core(7, 1)),
            ((2, 0), Node::Segment),
        ];
        let b = Board::from_layout(5, 3, (0, 0), cells).unwrap();
        let mut g = game(b, virus(50, 5));
        let ev = g.click((1, 0));
        assert!(
            !ev.iter().any(|e| e.id() == EVENT_DISTANCEINDICATOR),
            "{ev:?}"
        );
    }

    #[test]
    fn virus_from_attributes() {
        assert_eq!(
            VirusStats::from_attributes(41.6, 19.4, 7.0),
            VirusStats {
                coherence: 42,
                strength: 19,
                slots: MAX_SLOTS
            }
        );
        assert_eq!(VirusStats::from_attributes(f64::NAN, -3.0, -1.0).slots, 0);
    }

    #[test]
    fn push_shape() {
        let p = TurnComplete {
            to: 31,
            events: vec![Event::Ack],
        };
        let (name, args) = p.to_push();
        assert_eq!(name, "OnHackingTurnComplete");
        assert_eq!(args, vec![PyValue::List(vec![Event::Ack.to_pyvalue()])]);
        assert_eq!(p.audience(), Some(vec![31]));
    }

    #[test]
    fn shipped_rules_generate_valid_boards() {
        for tier in 1..=4 {
            let b = Board::generate(&Rules::SHIPPED, tier, 1);
            assert_eq!(b.distances(b.start).len(), b.tiles.len());
        }
    }
}
