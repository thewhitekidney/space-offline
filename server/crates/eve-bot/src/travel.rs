//! **Gate travel** (R-SOCIETY SOC-A3, the travel lane): a bot plans a route
//! over the stargate graph, flies it -- undock, warp to the gate, jump,
//! repeat, dock -- and fetches what its home station does not sell.
//!
//! Why: in the closed economy the NPC sells blueprints only at the stations
//! of the corporations that list them (O5-a, E5), and skillbooks only at
//! school stations, so a builder whose home station lists no blueprint found
//! no ask and never built, and the miners then had no buyer (tracker §83,
//! the closed soak's follow-up). The market already shows the whole
//! region's asks (`marketProxy.GetOrders` answers by region); what was
//! missing was the trip.
//!
//! - **The map** ([`Map`]): the stargate graph, which gate leads where,
//!   which system each station is in, and each system's security -- from the
//!   celestial catalogue (`--celestials`, or `--content`'s) and the bundle's
//!   `universe/systems.tsv` ([`Map::open`]).
//! - **The route** ([`Map::route`]): the shortest path in jumps, never
//!   through a system under the security floor ([`SECURITY_FLOOR`]), pure.
//! - **The way** ([`aim`], [`next_move`]): one move at a time, pure, the
//!   courier's station moves (`crate::courier::next_move`) plus the gate:
//!   in another system than the target, the next gate on the route is the
//!   ball to reach, and inside the client's jump range it is jumped
//!   ([`Conn::stargate_jump`]).
//! - **The trip** ([`go`]): the moves, flown over a [`Conn`].
//! - **The errand** ([`find`], [`fetch`]): where a type is sold within
//!   reach ([`sources`], capped by distance and by value, [`max_jumps`]),
//!   then go there, buy, load the hold (or board the hull bought), come
//!   home and unload.
//!
//! `crate::acquire` (books, hulls, modules, rounds) and `crate::builder`
//! (the blueprint ladder) call [`find`] and [`fetch`] when their own
//! station has no ask.
//!
//! **Every rule here is [A] policy** -- what a player would do -- except the
//! client's own call shapes and gates, which the verbs cite.

use crate::driver::Graph;
use crate::verbs::Book;
use crate::{Conn, DOCK_RANGE, Error, MIN_WARP_DISTANCE, Result};
use eve_server::celestial::{Catalogue, Kind, Universe, pseudo_security};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};

/// **The security floor** **[A]**: a bot routes only through systems whose
/// security, as the client rounds it ([`pseudo_security`]), is at least
/// this -- high-sec, `SecurityClassFromLevel`'s own boundary. `eve-bots
/// --security-floor F` sets it; SOC-A1's `risk` trait will set it per bot.
/// Falsified by bots that never reach a seller every human reaches through
/// one low-sec system (too high), or by bots shot in low-sec on errands
/// (too low).
pub const SECURITY_FLOOR: f64 = 0.45;

/// A floor that lets every system through, including the ones whose
/// security nothing says.
pub const ANY_SECURITY: f64 = f64::NEG_INFINITY;

/// **The longest errand**, in jumps **[A]**: a player fetches a blueprint
/// from a few systems away, not across the region. Falsified by builders
/// that never climb because every seller is farther (too low), or by trips
/// that keep a bot out of its loop for an hour (too high).
pub const TRIP_MAX_JUMPS: u32 = 5;

/// **What a jump must be worth** **[A]**: an errand for goods worth `v` ISK
/// goes at most `1 + v / TRIP_ISK_PER_JUMP` jumps (and never more than
/// [`TRIP_MAX_JUMPS`]): a cheap book is fetched from next door, a blueprint
/// from farther. Falsified by bots that cross the region for a box of
/// rounds (too low), or that never fetch the skillbook their career needs
/// from the school two jumps away (too high).
pub const TRIP_ISK_PER_JUMP: f64 = 10_000.0;

/// The most moves [`go`] makes inside one system before it gives up
/// **[A]**: undock, bind, warp, approach, jump or dock, with slack.
pub const MOVES_PER_SYSTEM: usize = 10;

/// How long a warp or an approach may take before it counts as stuck: the
/// courier's ceiling ([`crate::courier::TRAVEL_CEILING`]).
pub const TRAVEL_CEILING: Duration = crate::courier::TRAVEL_CEILING;

/// How long [`go`] waits for each burst while a warp or approach runs.
const POLL: Duration = Duration::from_millis(250);

// ---- the map -------------------------------------------------------------------

/// What a bot knows of the cluster to route over: the gate graph, each
/// gate pair, each station's system and each system's security.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Map {
    graph: Graph,
    /// `(from system, to system) -> (gate in from, its partner in to)`.
    gates: BTreeMap<(i64, i64), (i64, i64)>,
    /// `station -> system`.
    stations: BTreeMap<i64, i64>,
    /// `system -> security`, unrounded.
    security: BTreeMap<i64, f64>,
    /// `system -> region` (FE-TRADE): where a bot's `GetOrders` answers
    /// from once it is docked there.
    regions: BTreeMap<i64, i64>,
    /// `system -> name`, from the bundle's universe (CHATTER-2: the bot's
    /// chat says where it is). Display only.
    names: BTreeMap<i64, String>,
    /// PULL-LEASE: the systems with an asteroid belt, when the map was
    /// built from a catalogue (`belts_known`); see [`Map::has_belt`].
    belts: BTreeSet<i64>,
    belts_known: bool,
    /// FW-4: `station -> the faction its owner corporation belongs to`, from
    /// the bundle (`stations/stations.tsv` owner, `npc/corporations.tsv`
    /// faction). A station of no faction is absent. Empty without a bundle.
    station_factions: BTreeMap<i64, i64>,
}

impl Map {
    pub fn new() -> Map {
        Map::default()
    }

    /// One gate: `gate` in `from` leads to `partner` in `to`. One way; a
    /// catalogue has both rows.
    pub fn with_gate(mut self, from: i64, gate: i64, to: i64, partner: i64) -> Map {
        if from == to || from == 0 || to == 0 {
            return self;
        }
        self.graph.entry(from).or_default().insert(to);
        self.graph.entry(to).or_default();
        self.gates.entry((from, to)).or_insert((gate, partner));
        self
    }

    /// `station` is in `system`.
    pub fn with_station(mut self, station: i64, system: i64) -> Map {
        self.stations.insert(station, system);
        self
    }

    /// `station`'s owner corporation belongs to `faction`.
    pub fn with_station_faction(mut self, station: i64, faction: i64) -> Map {
        self.station_factions.insert(station, faction);
        self
    }

    /// Whether the map knows which faction owns any station (a bundle was
    /// read); without it a bot cannot tell where a militia office is.
    pub fn knows_station_factions(&self) -> bool {
        !self.station_factions.is_empty()
    }

    /// The faction that owns `station`, if any.
    pub fn faction_of_station(&self, station: i64) -> Option<i64> {
        self.station_factions.get(&station).copied()
    }

    /// Every station of `faction`, ascending.
    pub fn stations_of_faction(&self, faction: i64) -> Vec<i64> {
        self.station_factions
            .iter()
            .filter(|(st, f)| **f == faction && self.stations.contains_key(*st))
            .map(|(st, _)| *st)
            .collect()
    }

    /// `system` has an asteroid belt; the map then knows its belts.
    pub fn with_belt(mut self, system: i64) -> Map {
        self.belts.insert(system);
        self.belts_known = true;
        self
    }

    /// Whether `system` has a belt to mine. `true` when the map does not
    /// know belts at all (built without a catalogue): no information is not
    /// "no belt".
    pub fn has_belt(&self, system: i64) -> bool {
        !self.belts_known || self.belts.contains(&system)
    }

    /// `system`'s security, unrounded.
    pub fn with_security(mut self, system: i64, security: f64) -> Map {
        self.security.insert(system, security);
        self
    }

    /// `system` is in `region`.
    pub fn with_region(mut self, system: i64, region: i64) -> Map {
        if region != 0 {
            self.regions.insert(system, region);
        }
        self
    }

    /// From a catalogue: every gate whose partner is loaded (the server
    /// refuses a jump to an unloaded one, `cmd_stargate_jump`), every
    /// station with a system; and each system's security from `universe`.
    pub fn from_catalogue(cat: &Catalogue, universe: Option<&Universe>) -> Map {
        let mut map = Map::new();
        map.belts_known = true;
        for c in cat.served() {
            let Some(system) = cat.system_of(c.id).filter(|s| *s != 0) else {
                continue;
            };
            match c.kind {
                Kind::Station => {
                    map = map.with_station(c.id, system);
                    if let Some(row) = cat.system(system) {
                        map = map.with_region(system, row.region_id);
                    }
                }
                Kind::Belt => map = map.with_belt(system),
                Kind::Gate => {
                    if let (Some((_, partner)), Some((_, to))) = (cat.gate(c.id), cat.jump_of(c.id))
                    {
                        map = map.with_gate(system, c.id, to, partner.id);
                    }
                }
                _ => {}
            }
        }
        if let Some(u) = universe {
            for s in u.systems() {
                map = map.with_security(s.id, s.security);
                if !s.name.is_empty() {
                    map.names.insert(s.id, s.name.clone());
                }
            }
        }
        map
    }

    /// `eve-bots`: the catalogue of `--celestials FILE`, else of `--content
    /// DIR`, with `--content`'s universe for security; `None` with neither.
    /// Without a universe every system's security is unknown, and only
    /// [`ANY_SECURITY`] routes.
    pub fn open(
        celestials: Option<&Path>,
        content: Option<&Path>,
    ) -> std::result::Result<Option<Map>, String> {
        let bundle = match content {
            Some(dir) => Some(eve_server::content::Bundle::open(dir)?.0),
            None => None,
        };
        let path = match (celestials, &bundle) {
            (Some(p), _) => Some(p.to_path_buf()),
            (None, Some(b)) => b.path(eve_server::content::CELESTIALS),
            (None, None) => None,
        };
        let Some(path) = path else {
            return Ok(None);
        };
        let cat = eve_server::load_celestials(&path)?;
        let universe = match &bundle {
            Some(b) => Some(Universe::from_bundle(b)?),
            None => None,
        };
        let mut map = Map::from_catalogue(&cat, universe.as_ref());
        if let Some(b) = &bundle {
            // `stations/stations.tsv`: `station_id system region owner ...`.
            let header = "#station_id\tsystem_id\tregion_id\towner_id\toperation_id\tservices\treproc_eff\treproc_take";
            let lines = b.data_lines("stations/stations.tsv", header)?;
            let npc = eve_server::npc_data::Table::from_bundle(b)?;
            for line in &lines {
                let f: Vec<&str> = line.split('\t').collect();
                let (Some(id), Some(owner)) = (
                    f.first().and_then(|x| x.parse::<i64>().ok()),
                    f.get(3).and_then(|x| x.parse::<i64>().ok()),
                ) else {
                    continue;
                };
                if let Some(faction) = npc.corporations.get(&owner).and_then(|c| c.faction_id) {
                    map = map.with_station_faction(id, faction);
                }
            }
        }
        Ok(Some(map))
    }

    /// A system's name, when the bundle's universe gave one.
    pub fn name_of(&self, system: i64) -> Option<&str> {
        self.names.get(&system).map(String::as_str)
    }

    /// A map that knows one system's name (tests).
    pub fn with_name(mut self, system: i64, name: &str) -> Map {
        self.names.insert(system, name.to_owned());
        self
    }

    /// The system a station is in.
    pub fn system_of(&self, station: i64) -> Option<i64> {
        self.stations.get(&station).copied()
    }

    /// The region a system is in, if the map knows it (FE-TRADE).
    pub fn region_of(&self, system: i64) -> Option<i64> {
        self.regions.get(&system).copied()
    }

    /// The region a station is in (its system's).
    pub fn region_of_station(&self, station: i64) -> Option<i64> {
        self.region_of(self.system_of(station)?)
    }

    /// Every station on the map in `system`, ascending (SOC-B9).
    pub fn stations_in(&self, system: i64) -> Vec<i64> {
        self.stations
            .iter()
            .filter(|(_, s)| **s == system)
            .map(|(st, _)| *st)
            .collect()
    }

    /// The gate in `from` that leads to `to`, and its partner.
    pub fn gate(&self, from: i64, to: i64) -> Option<(i64, i64)> {
        self.gates.get(&(from, to)).copied()
    }

    /// `system`'s security, unrounded, when the map knows it (CAP-L: the
    /// cyno alt's destination is not high sec).
    pub fn security(&self, system: i64) -> Option<f64> {
        self.security.get(&system).copied()
    }

    /// **May a route pass through `system`** at `floor`: its security, as
    /// the client rounds it, is at least the floor. A system whose security
    /// nothing says passes only [`ANY_SECURITY`].
    pub fn allowed(&self, system: i64, floor: f64) -> bool {
        if floor == ANY_SECURITY {
            return true;
        }
        self.security
            .get(&system)
            .is_some_and(|&s| pseudo_security(s) >= floor)
    }

    /// **The shortest route** from `from` to `to`, both ends included, in
    /// jumps; never through a system [`Self::allowed`] refuses, the
    /// destination included (the start is where the bot already is). Ties
    /// go to the lower system id at each step, so a route is the same every
    /// run. `None`: no such route.
    pub fn route(&self, from: i64, to: i64, floor: f64) -> Option<Vec<i64>> {
        self.route_clear(from, to, floor, None)
    }

    /// **Is `system` in the net of faction police for a pilot of `status`**
    /// (BOT-LAW, `sec_status::band_attacked_in`): the police attack that
    /// status at this security. A system whose security nothing says is not.
    pub fn in_police_net(&self, system: i64, status: f64) -> bool {
        self.security.get(&system).is_some_and(|&s| {
            eve_server::sec_status::police_attack_here(status, pseudo_security(s))
        })
    }

    /// [`Self::route`] that also never enters a system where police attack
    /// `status` (`None`: no such rule). The destination counts; the start
    /// is where the bot already is.
    pub fn route_clear(
        &self,
        from: i64,
        to: i64,
        floor: f64,
        status: Option<f64>,
    ) -> Option<Vec<i64>> {
        let barred = |n: i64| status.is_some_and(|st| self.in_police_net(n, st));
        if from == to {
            return Some(vec![from]);
        }
        // A path may pass net systems while it is unbroken net from the start
        // (a pilot inside the net must be able to leave it); once it has left,
        // the bar holds, and it always holds on the destination. A net system
        // enters `back` only along such a path, so it is the prefix.
        let in_prefix =
            |s: i64, back: &BTreeMap<i64, i64>| barred(s) && (s == from || back.contains_key(&s));
        let mut back: BTreeMap<i64, i64> = BTreeMap::new();
        let mut queue = VecDeque::from([from]);
        while let Some(s) = queue.pop_front() {
            for &n in self.graph.get(&s).into_iter().flatten() {
                if n == from || back.contains_key(&n) || !self.allowed(n, floor) {
                    continue;
                }
                if barred(n) && (n == to || !in_prefix(s, &back)) {
                    continue;
                }
                back.insert(n, s);
                if n == to {
                    let mut path = vec![to];
                    let mut at = to;
                    while let Some(&p) = back.get(&at) {
                        path.push(p);
                        at = p;
                    }
                    path.reverse();
                    return Some(path);
                }
                queue.push_back(n);
            }
        }
        None
    }

    /// The jumps of [`Self::route`].
    pub fn jumps(&self, from: i64, to: i64, floor: f64) -> Option<u32> {
        self.jumps_clear(from, to, floor, None)
    }

    /// The jumps of [`Self::route_clear`].
    pub fn jumps_clear(&self, from: i64, to: i64, floor: f64, status: Option<f64>) -> Option<u32> {
        self.route_clear(from, to, floor, status)
            .map(|r| u32::try_from(r.len() - 1).unwrap_or(u32::MAX))
    }

    /// Every system the map knows, in id order (PROG-NULL: where rare ore
    /// is cut).
    pub fn systems(&self) -> Vec<i64> {
        let mut all: std::collections::BTreeSet<i64> = self.graph.keys().copied().collect();
        all.extend(self.security.keys().copied());
        all.into_iter().collect()
    }

    /// The systems one gate from `system`, in id order (SOC-B3's roam).
    pub fn neighbours(&self, system: i64) -> Vec<i64> {
        self.graph
            .get(&system)
            .map(|n| n.iter().copied().collect())
            .unwrap_or_default()
    }
}

/// A map and the floor to route it at: what an errand needs.
#[derive(Debug, Clone, Copy)]
pub struct Trips<'a> {
    pub map: &'a Map,
    pub floor: f64,
}

// ---- where it is sold ------------------------------------------------------------

/// The farthest an errand for goods worth `value` ISK goes (module header,
/// [`TRIP_ISK_PER_JUMP`]).
pub fn max_jumps(value: f64) -> u32 {
    let by_value = (value.max(0.0) / TRIP_ISK_PER_JUMP).floor();
    let by_value = if by_value >= f64::from(TRIP_MAX_JUMPS) {
        TRIP_MAX_JUMPS
    } else {
        by_value as u32
    };
    (1 + by_value).min(TRIP_MAX_JUMPS)
}

/// One station that sells a type, within reach.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Source {
    pub station: i64,
    pub system: i64,
    pub jumps: u32,
    /// Its cheapest ask under the cap, ISK per unit.
    pub price: f64,
    /// The units on offer there under the cap.
    pub units: i64,
}

/// **Where a type is sold within reach** (pure): every station other than
/// `home` with an ask at most `cap` per unit (`None`: any price), whose
/// system the map knows and routes to from `from` at `floor` within
/// [`max_jumps`] of the value of `units` there; nearest first, then
/// cheapest, then by station id.
pub fn sources(
    book: &Book,
    trips: Trips<'_>,
    from: i64,
    home: i64,
    cap: Option<f64>,
    units: i64,
) -> Vec<Source> {
    let mut by_station: BTreeMap<i64, (f64, i64)> = BTreeMap::new();
    for a in &book.asks {
        if a.bid || a.station_id == home || a.vol_remaining <= 0 || a.price <= 0.0 {
            continue;
        }
        if cap.is_some_and(|c| a.price > c) {
            continue;
        }
        let e = by_station.entry(a.station_id).or_insert((a.price, 0));
        e.0 = e.0.min(a.price);
        e.1 += a.vol_remaining;
    }
    let mut out: Vec<Source> = by_station
        .into_iter()
        .filter_map(|(station, (price, on_offer))| {
            let system = trips.map.system_of(station)?;
            let jumps = trips.map.jumps(from, system, trips.floor)?;
            let value = price * units.min(on_offer).max(1) as f64;
            (jumps <= max_jumps(value)).then_some(Source {
                station,
                system,
                jumps,
                price,
                units: on_offer,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.jumps
            .cmp(&b.jumps)
            .then(a.price.total_cmp(&b.price))
            .then(a.station.cmp(&b.station))
    });
    out
}

/// A station in a region and the way there (FE-TRADE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Landing {
    pub station: i64,
    pub system: i64,
    pub jumps: u32,
}

/// **The nearest station in `region`** (pure), routable from `from` at the
/// floor within `max_jumps`: fewest jumps, then the lowest station id.
/// `prefer` names a station that wins a tie on jumps (the one a remembered
/// book showed). `None` when the map knows no such station, or none is
/// reachable: a trader does not guess at a region it cannot route to.
pub fn find_in_region(
    trips: Trips<'_>,
    from: i64,
    region: i64,
    max_jumps: u32,
    prefer: Option<i64>,
) -> Option<Landing> {
    let mut best: Option<Landing> = None;
    for (&station, &system) in &trips.map.stations {
        if trips.map.region_of(system) != Some(region) {
            continue;
        }
        let Some(jumps) = trips.map.jumps(from, system, trips.floor) else {
            continue;
        };
        if jumps > max_jumps {
            continue;
        }
        let cand = Landing {
            station,
            system,
            jumps,
        };
        let wins = best.is_none_or(|b| {
            (cand.jumps, Some(cand.station) != prefer, cand.station)
                < (b.jumps, Some(b.station) != prefer, b.station)
        });
        if wins {
            best = Some(cand);
        }
    }
    best
}

// ---- the way, one move at a time -------------------------------------------------

/// What to fly at next, from where the bot is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aim {
    /// The target station, in this system.
    Station(i64),
    /// The next gate on the route, its partner, and the system it leads to.
    Gate { gate: i64, partner: i64, to: i64 },
    /// No route at the floor, or the map does not know the station.
    Unrouted,
}

/// **The next thing to reach** (pure): in the target's system, the
/// station; elsewhere, the gate to the route's next system.
pub fn aim(map: &Map, system: i64, station: i64, floor: f64) -> Aim {
    aim_clear(map, system, station, floor, None)
}

/// [`aim`] by [`Map::route_clear`].
pub fn aim_clear(map: &Map, system: i64, station: i64, floor: f64, status: Option<f64>) -> Aim {
    let Some(dest) = map.system_of(station) else {
        return Aim::Unrouted;
    };
    if dest == system {
        return Aim::Station(station);
    }
    let Some(route) = map.route_clear(system, dest, floor, status) else {
        return Aim::Unrouted;
    };
    match map.gate(system, route[1]) {
        Some((gate, partner)) => Aim::Gate {
            gate,
            partner,
            to: route[1],
        },
        None => Aim::Unrouted,
    }
}

/// Where the bot is, as far as the next move needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Here {
    /// The station docked in.
    pub docked_at: Option<i64>,
    /// In space with the ballpark bound.
    pub bound: bool,
    /// The ship is in warp.
    pub warping: bool,
    /// The surface distance to the aim's ball, when both are in the park.
    pub gap: Option<f64>,
}

/// One step of a trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Arrived,
    Undock,
    Bind,
    /// Still in warp: let it land.
    Wait,
    Warp(i64),
    Approach(i64),
    Dock(i64),
    Jump {
        gate: i64,
        partner: i64,
    },
    /// The aim's ball is not in this park.
    NotHere(i64),
    /// No route ([`Aim::Unrouted`]).
    NoRoute,
}

/// **The next move toward `station`** (pure). Docked there: arrived;
/// docked elsewhere: undock; in space: bind, let a warp land, then fly at
/// the [`aim`]: warp from beyond [`MIN_WARP_DISTANCE`], approach, and dock
/// inside [`DOCK_RANGE`] or jump inside
/// [`eve_server::jump::MAX_JUMP_DISTANCE`] (the client's own gates).
pub fn next_move(here: &Here, station: i64, aim: Aim) -> Move {
    match here.docked_at {
        Some(s) if s == station => return Move::Arrived,
        Some(_) => return Move::Undock,
        None => {}
    }
    if !here.bound {
        return Move::Bind;
    }
    if here.warping {
        return Move::Wait;
    }
    let (ball, reach) = match aim {
        Aim::Unrouted => return Move::NoRoute,
        Aim::Station(s) => (s, DOCK_RANGE),
        Aim::Gate { gate, .. } => (gate, eve_server::jump::MAX_JUMP_DISTANCE),
    };
    match here.gap {
        None => Move::NotHere(ball),
        Some(g) if g >= MIN_WARP_DISTANCE => Move::Warp(ball),
        Some(g) if g < reach => match aim {
            Aim::Gate { gate, partner, .. } => Move::Jump { gate, partner },
            _ => Move::Dock(ball),
        },
        Some(_) => Move::Approach(ball),
    }
}

// ---- the trip, flown -------------------------------------------------------------

/// What a [`go`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trip {
    pub moves: Vec<Move>,
    /// Gates jumped.
    pub jumps: u32,
}

/// The system the bot is in, docked or not.
fn system_now(c: &Conn) -> Option<i64> {
    c.session().solar_system_id2()
}

/// **Fly to `station` and dock** by [`next_move`]: undock, bind, warp to
/// the route's next gate, approach it, jump, bind the next system's park,
/// and so on to the station. Already docked there: nothing.
pub fn go(c: &mut Conn, trips: Trips<'_>, station: i64) -> Result<Trip> {
    let Trips { map, floor } = trips;
    // BOT-LAW: a pilot the police would attack keeps out of their net.
    let status = c.police_status();
    let start = system_now(c).ok_or_else(|| Error::NotSent("travel: no system".to_owned()))?;
    let dest = map
        .system_of(station)
        .ok_or_else(|| Error::NotSent(format!("travel: station {station} is on no map")))?;
    let Some(jumps) = map.jumps_clear(start, dest, floor, status) else {
        return Err(Error::NotSent(format!(
            "travel: no route from {start} to {dest} at security {floor}"
        )));
    };
    let budget = MOVES_PER_SYSTEM * (jumps as usize + 1);
    let mut trip = Trip::default();
    for _ in 0..budget {
        let system = system_now(c).ok_or_else(|| Error::NotSent("travel: no system".to_owned()))?;
        let target = aim_clear(map, system, station, floor, status);
        let ball = match target {
            Aim::Station(s) => Some(s),
            Aim::Gate { gate, .. } => Some(gate),
            Aim::Unrouted => None,
        };
        let here = Here {
            docked_at: c.session().station_id(),
            bound: c.ballpark().is_some() && c.world().has_state(),
            warping: warping(c),
            gap: ball.and_then(|b| gap(c, b)),
        };
        let step = next_move(&here, station, target);
        trip.moves.push(step);
        match step {
            Move::Arrived => return Ok(trip),
            Move::Undock => {
                c.undock_repaired()?;
            }
            Move::Bind => {
                let system = c
                    .session()
                    .solar_system_id()
                    .ok_or_else(|| Error::NotSent("travel: in space with no system".into()))?;
                if c.ballpark().is_none() {
                    c.bind_ballpark(system)?;
                }
                settle(c, |c| c.world().has_state())?;
            }
            Move::Wait => settle(c, |c| !warping(c))?,
            Move::Warp(id) => {
                c.warp_to_item(id, 0.0)?;
                settle(c, |c| {
                    !warping(c) && gap(c, id).is_some_and(|g| g < MIN_WARP_DISTANCE)
                })?;
            }
            Move::Approach(id) => {
                let reach = match target {
                    Aim::Gate { .. } => eve_server::jump::MAX_JUMP_DISTANCE,
                    _ => DOCK_RANGE,
                };
                c.follow(id, 0.0)?;
                settle(c, |c| gap(c, id).is_some_and(|g| g < reach))?;
            }
            Move::Dock(id) => {
                c.dock(id)?;
            }
            Move::Jump { gate, partner } => {
                c.stargate_jump(gate, partner)?;
                trip.jumps += 1;
            }
            Move::NotHere(id) => {
                return Err(Error::NotSent(format!(
                    "travel: {id} is not in system {system}'s park"
                )));
            }
            Move::NoRoute => {
                return Err(Error::NotSent(format!(
                    "travel: no route from {system} to station {station}"
                )));
            }
        }
    }
    Err(Error::Refused(format!(
        "travel: not docked at {station} after {budget} moves ({:?})",
        trip.moves
    )))
}

fn gap(c: &mut Conn, target: i64) -> Option<f64> {
    let world = c.world_now();
    world.surface_distance(world.ego?, target)
}

fn warping(c: &mut Conn) -> bool {
    c.world_now().ego_ball().is_some_and(|b| b.warp_commanded())
}

/// Read pushes until `done` holds or [`TRAVEL_CEILING`] passes.
fn settle(c: &mut Conn, mut done: impl FnMut(&mut Conn) -> bool) -> Result<()> {
    let deadline = Instant::now() + TRAVEL_CEILING;
    while !done(c) {
        if Instant::now() >= deadline {
            return Err(Error::Refused(
                "travel: still moving after the travel ceiling".into(),
            ));
        }
        c.pump(POLL)?;
    }
    Ok(())
}

// ---- the errand ------------------------------------------------------------------

/// How the goods come home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Haul {
    /// In the hold: a book, a blueprint, modules, rounds.
    Cargo,
    /// A hull: assembled there, boarded and flown home.
    Fly,
}

/// What a [`fetch`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fetched {
    /// The station it went to.
    pub station: i64,
    /// Gates jumped, out and back.
    pub jumps: u32,
    /// `(typeID, units, ISK)` per fill there.
    pub bought: Vec<(i64, i64, f64)>,
    /// Units hauled home: unloaded into the home hangar, or the hull flown.
    pub home: i64,
    /// Why something was not done: a refused buy, load or unload.
    pub notes: Vec<String>,
}

impl Fetched {
    pub fn units(&self) -> i64 {
        self.bought.iter().map(|&(_, n, _)| n).sum()
    }

    /// The act line's note.
    pub fn summary(&self) -> String {
        let mut s = format!(
            "trip to station {} ({} jumps): bought {} units, {} home",
            self.station,
            self.jumps,
            self.units(),
            self.home
        );
        for n in &self.notes {
            s.push_str("; ");
            s.push_str(n);
        }
        s
    }
}

/// **The nearest seller within reach** ([`sources`]) that the wallet
/// covers, from the book the server shows (the region's). Docked only.
pub fn find(
    c: &mut Conn,
    trips: Trips<'_>,
    type_id: i64,
    units: i64,
    cap: Option<f64>,
) -> Result<Option<Source>> {
    let home = c
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("travel: not docked".to_owned()))?;
    let Some(system) = system_now(c) else {
        return Err(Error::NotSent("travel: no system".to_owned()));
    };
    let book = c.market_orders(type_id)?;
    let wallet = c.cash_balance()?;
    Ok(sources(&book, trips, system, home, cap, units)
        .into_iter()
        .find(|s| s.price <= wallet))
}

/// A refusal is a note and the errand goes on; a broken connection ends it.
fn noted<T>(r: Result<T>, notes: &mut Vec<String>, what: &str) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => {
            notes.push(format!("{what}: {e}"));
            Ok(None)
        }
    }
}

/// **Fetch `units` of `type_id` from `source`** and come home: a hold first
/// when the goods ride in one (a pilot in a capsule boards or is given its
/// rookie ship, [`crate::acquire::rookie_ship`]); the trip there; the buys,
/// cheapest ask first under `cap` and inside the wallet; the hold loaded
/// (or the hull assembled and boarded); the trip home, **always**, bought
/// or not; the hold unloaded into the home hangar. A refusal on the way is
/// a note; a trip that cannot be flown is the error.
pub fn fetch(
    c: &mut Conn,
    trips: Trips<'_>,
    source: &Source,
    type_id: i64,
    units: i64,
    cap: Option<f64>,
    haul: Haul,
) -> Result<Fetched> {
    let home = c
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("travel: not docked".to_owned()))?;
    let mut got = Fetched {
        station: source.station,
        ..Fetched::default()
    };
    if haul == Haul::Cargo
        && crate::acquire::flying_capsule(c)?
        && let Err(why) = crate::acquire::rookie_ship(c, home)?
    {
        return Err(Error::NotSent(format!(
            "travel: a capsule has no hold, and no rookie ship ({why})"
        )));
    }
    got.jumps += go(c, trips, source.station)?.jumps;

    let book = noted(c.market_orders(type_id), &mut got.notes, "book")?;
    let wallet = c.cash_balance()?;
    let plan = book
        .map(|b| crate::acquire::plan(&b, source.station, cap, wallet, units))
        .unwrap_or_default();
    for (ask, k) in plan {
        let Some(row) = noted(
            c.buy(
                source.station,
                type_id,
                ask.price,
                k,
                0,
                crate::builder::BROKER_FEE,
            ),
            &mut got.notes,
            "buy",
        )?
        else {
            break;
        };
        let filled = k - row.vol_remaining.clamp(0, k);
        if filled > 0 {
            got.bought
                .push((type_id, filled, filled as f64 * ask.price));
        }
    }
    if got.units() > 0 {
        match haul {
            Haul::Cargo => {
                for row in c.hangar_items()? {
                    if row.type_id == type_id && !row.is_fitted() {
                        noted(c.load_cargo(row.item_id, None), &mut got.notes, "load")?;
                    }
                }
            }
            Haul::Fly => board_bought(c, source.station, type_id, &mut got)?,
        }
    }

    got.jumps += go(c, trips, home)?.jumps;
    match haul {
        Haul::Cargo => {
            for row in c.cargo()? {
                if row.type_id == type_id
                    && noted(c.unload(row.item_id, None), &mut got.notes, "unload")?.is_some()
                {
                    got.home += row.quantity;
                }
            }
        }
        Haul::Fly => {
            let ship = c.session().ship_id();
            let flying = c
                .hangar_items()?
                .iter()
                .any(|r| Some(r.item_id) == ship && r.type_id == type_id);
            if flying {
                got.home = 1;
            }
        }
    }
    Ok(got)
}

/// The hull bought, assembled and boarded, there.
fn board_bought(c: &mut Conn, station: i64, hull: i64, got: &mut Fetched) -> Result<()> {
    match crate::acquire::assemble_hull(c, station, hull)? {
        Ok(true) => {}
        Ok(false) => {
            got.notes
                .push(format!("hull {hull}: bought, not in the hangar"));
            return Ok(());
        }
        Err(why) => {
            got.notes.push(why);
            return Ok(());
        }
    }
    let Some(row) = c
        .hangar_items()?
        .into_iter()
        .find(|r| r.type_id == hull && r.singleton)
    else {
        got.notes
            .push(format!("hull {hull}: assembled, not in the hangar"));
        return Ok(());
    };
    noted(c.board(row.item_id), &mut got.notes, "board")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verbs::BookOrder;

    // Invented ids only.
    const S1: i64 = 39_000_001;
    const S2: i64 = 39_000_002;
    const S3: i64 = 39_000_003;
    const S4: i64 = 39_000_004;
    const S5: i64 = 39_000_005;
    const HIGH: f64 = 0.9;
    const LOW: f64 = 0.2;

    fn gate_of(from: i64, to: i64) -> i64 {
        59_000_000 + (from % 1000) * 1000 + to % 1000
    }

    /// Both directions of a gate pair.
    fn link(m: Map, a: i64, b: i64) -> Map {
        m.with_gate(a, gate_of(a, b), b, gate_of(b, a)).with_gate(
            b,
            gate_of(b, a),
            a,
            gate_of(a, b),
        )
    }

    /// S1 - S2 - S3 - S4, and a shortcut S1 - S5 - S4 through S5. S5's
    /// security is `s5`; every other system is high.
    fn graph(s5: f64) -> Map {
        let mut m = Map::new();
        for (a, b) in [(S1, S2), (S2, S3), (S3, S4), (S1, S5), (S5, S4)] {
            m = link(m, a, b);
        }
        for s in [S1, S2, S3, S4] {
            m = m.with_security(s, HIGH);
        }
        m.with_security(S5, s5)
    }

    /// A route never enters a system where the police attack the pilot's
    /// status (the band table, not a number written here), and the start is
    /// exempt, and a pilot already in the net may leave it however deep.
    #[test]
    fn a_route_keeps_out_of_the_police_net() {
        use eve_server::sec_status::band_attacked_in;
        let m = graph(LOW);
        // -3.0: attacked at 0.8 and up, so high-sec S2 and S3 are barred.
        let st = -3.0;
        assert!(band_attacked_in(st).is_some_and(|b| b <= HIGH));
        assert!(m.in_police_net(S2, st));
        assert!(!m.in_police_net(S5, st), "low-sec is outside the net");
        assert_eq!(m.route_clear(S1, S3, ANY_SECURITY, Some(st)), None);
        // S4 is high-sec too: the destination counts. S5 is clear.
        assert_eq!(m.route_clear(S1, S4, ANY_SECURITY, Some(st)), None);
        assert_eq!(
            m.route_clear(S1, S5, ANY_SECURITY, Some(st)),
            Some(vec![S1, S5])
        );
        // The destination counts. A pilot inside the net leaves through it,
        // one jump deep (S2) or two (S3 by S4), but never into it again.
        assert_eq!(
            m.route_clear(S2, S5, ANY_SECURITY, Some(st)),
            Some(vec![S2, S1, S5])
        );
        assert_eq!(
            m.route_clear(S3, S5, ANY_SECURITY, Some(st)),
            Some(vec![S3, S4, S5])
        );
        assert_eq!(m.route_clear(S2, S4, ANY_SECURITY, Some(st)), None);
        assert_eq!(m.route_clear(S5, S1, ANY_SECURITY, Some(st)), None);
        // A clean pilot, or one nobody reads, routes as ever.
        assert_eq!(
            m.route_clear(S1, S3, ANY_SECURITY, None),
            m.route(S1, S3, ANY_SECURITY)
        );
        assert_eq!(
            m.route_clear(S1, S3, ANY_SECURITY, Some(0.0)),
            m.route(S1, S3, ANY_SECURITY)
        );
        assert_eq!(m.jumps_clear(S1, S5, ANY_SECURITY, Some(st)), Some(1));
        // The aim follows the same rule.
        let m = m.with_station(70_000_001, S3);
        assert_eq!(
            aim_clear(&m, S1, 70_000_001, ANY_SECURITY, Some(st)),
            Aim::Unrouted
        );
        assert!(matches!(
            aim_clear(&m, S1, 70_000_001, ANY_SECURITY, None),
            Aim::Gate { to: S2, .. }
        ));
    }

    #[test]
    fn the_route_is_the_shortest_path_over_the_gates() {
        let m = graph(HIGH);
        assert_eq!(m.route(S1, S4, SECURITY_FLOOR), Some(vec![S1, S5, S4]));
        assert_eq!(m.route(S1, S3, SECURITY_FLOOR), Some(vec![S1, S2, S3]));
        assert_eq!(m.route(S4, S1, SECURITY_FLOOR), Some(vec![S4, S5, S1]));
        assert_eq!(m.route(S2, S2, SECURITY_FLOOR), Some(vec![S2]));
        assert_eq!(m.jumps(S1, S4, SECURITY_FLOOR), Some(2));
        assert_eq!(m.gate(S1, S5), Some((gate_of(S1, S5), gate_of(S5, S1))));
    }

    /// **Never through a band the policy forbids**: with S5 under the
    /// floor the route goes the long way; with S5 the only way, there is
    /// none; at [`ANY_SECURITY`] it goes through.
    #[test]
    fn the_route_never_passes_a_system_under_the_floor() {
        let m = graph(LOW);
        let r = m.route(S1, S4, SECURITY_FLOOR).expect("the long way");
        assert_eq!(r, vec![S1, S2, S3, S4]);
        assert!(r.iter().all(|s| m.allowed(*s, SECURITY_FLOOR)));
        assert_eq!(m.route(S1, S5, SECURITY_FLOOR), None, "the destination too");
        assert_eq!(m.route(S1, S4, ANY_SECURITY), Some(vec![S1, S5, S4]));

        let cut = link(link(Map::new(), S1, S5), S5, S4)
            .with_security(S1, HIGH)
            .with_security(S4, HIGH)
            .with_security(S5, LOW);
        assert_eq!(cut.route(S1, S4, SECURITY_FLOOR), None);
        assert_eq!(cut.route(S1, S4, ANY_SECURITY), Some(vec![S1, S5, S4]));
    }

    /// A system with no gates (a wormhole, or one of the known-space systems
    /// the map gives none) is isolated: no route, no neighbours, no panic.
    #[test]
    fn an_isolated_system_has_no_route() {
        let m = graph(HIGH).with_security(S3 + 100, HIGH);
        assert_eq!(m.route(S1, S3 + 100, ANY_SECURITY), None);
        assert_eq!(m.route(S3 + 100, S1, ANY_SECURITY), None);
        assert_eq!(
            m.route(S3 + 100, S3 + 100, SECURITY_FLOOR),
            Some(vec![S3 + 100])
        );
        assert!(m.neighbours(S3 + 100).is_empty());
    }

    /// A system whose security nothing says is refused at any real floor.
    #[test]
    fn an_unknown_security_is_refused_except_at_any() {
        let m = link(Map::new(), S1, S2).with_security(S1, HIGH);
        assert_eq!(m.route(S1, S2, SECURITY_FLOOR), None);
        assert_eq!(m.route(S1, S2, ANY_SECURITY), Some(vec![S1, S2]));
    }

    /// The floor compares the client's rounding: a security just above
    /// zero is lifted to 0.05 ([`pseudo_security`]).
    #[test]
    fn the_floor_reads_the_clients_rounding() {
        let m = link(Map::new(), S1, S2).with_security(S2, 0.01);
        assert!(m.allowed(S2, 0.05));
        assert!(!m.allowed(S2, 0.06));
    }

    #[test]
    fn a_map_reads_gates_stations_and_systems_from_a_catalogue() {
        let (a, b) = (S1, S2);
        let (ga, gb) = (gate_of(a, b), gate_of(b, a));
        let cat = Catalogue::parse(&format!(
            "#system\t{a}\t20000001\t10000001\n\
             #system\t{b}\t20000001\t10000001\n\
             station\t69000001\t1\t500.0\t0.0\t0.0\t0.0\t{b}\t0\t0\n\
             gate\t{ga}\t1\t2473.0\t1.0e7\t0.0\t0.0\t{a}\t{gb}\t{b}\n\
             gate\t{gb}\t1\t2473.0\t0.0\t0.0\t1.0e7\t{b}\t{ga}\t{a}\n\
             gate\t59999999\t1\t2473.0\t0.0\t1.0e7\t0.0\t{a}\t59999998\t{S3}\n"
        ));
        let m = Map::from_catalogue(&cat, None);
        assert_eq!(m.system_of(69_000_001), Some(b));
        assert_eq!(m.gate(a, b), Some((ga, gb)));
        assert_eq!(m.gate(b, a), Some((gb, ga)));
        assert_eq!(m.gate(a, S3), None, "a partner not loaded is no road");
        assert_eq!(m.route(a, b, ANY_SECURITY), Some(vec![a, b]));
    }

    #[test]
    fn the_aim_is_the_station_here_else_the_next_gate() {
        let m = graph(HIGH).with_station(69_000_004, S4);
        assert_eq!(
            aim(&m, S4, 69_000_004, SECURITY_FLOOR),
            Aim::Station(69_000_004)
        );
        assert_eq!(
            aim(&m, S1, 69_000_004, SECURITY_FLOOR),
            Aim::Gate {
                gate: gate_of(S1, S5),
                partner: gate_of(S5, S1),
                to: S5
            }
        );
        assert_eq!(aim(&m, S1, 69_000_999, SECURITY_FLOOR), Aim::Unrouted);
        let low = graph(LOW).with_station(69_000_005, S5);
        assert_eq!(aim(&low, S1, 69_000_005, SECURITY_FLOOR), Aim::Unrouted);
    }

    /// The way, as a table: each row is one state and the move it calls
    /// for. The distances are the client's own gates.
    #[test]
    fn the_way_is_undock_bind_warp_approach_then_jump_or_dock() {
        const HOME: i64 = 69_000_001;
        const AWAY: i64 = 69_000_002;
        let gate = Aim::Gate {
            gate: 1,
            partner: 2,
            to: S2,
        };
        let station = Aim::Station(AWAY);
        let space = |gap: Option<f64>| Here {
            docked_at: None,
            bound: true,
            warping: false,
            gap,
        };
        let docked = |s| Here {
            docked_at: Some(s),
            bound: false,
            warping: false,
            gap: None,
        };
        let jump_reach = eve_server::jump::MAX_JUMP_DISTANCE;
        let rows = [
            (docked(AWAY), gate, Move::Arrived),
            (docked(HOME), gate, Move::Undock),
            (
                Here {
                    bound: false,
                    ..space(None)
                },
                gate,
                Move::Bind,
            ),
            (
                Here {
                    warping: true,
                    ..space(Some(1e9))
                },
                gate,
                Move::Wait,
            ),
            (space(Some(MIN_WARP_DISTANCE)), gate, Move::Warp(1)),
            (space(Some(jump_reach)), gate, Move::Approach(1)),
            (
                space(Some(jump_reach - 1.0)),
                gate,
                Move::Jump {
                    gate: 1,
                    partner: 2,
                },
            ),
            (space(None), gate, Move::NotHere(1)),
            (space(Some(MIN_WARP_DISTANCE)), station, Move::Warp(AWAY)),
            (space(Some(DOCK_RANGE)), station, Move::Approach(AWAY)),
            (space(Some(0.0)), station, Move::Dock(AWAY)),
            (space(Some(0.0)), Aim::Unrouted, Move::NoRoute),
        ];
        for (here, aim, want) in rows {
            assert_eq!(next_move(&here, AWAY, aim), want, "{here:?} {aim:?}");
        }
    }

    fn ask(id: i64, station: i64, price: f64, vol: i64) -> BookOrder {
        BookOrder {
            order_id: id,
            type_id: 990_000_001,
            station_id: station,
            price,
            vol_remaining: vol,
            bid: false,
        }
    }

    #[test]
    fn the_reach_grows_with_the_value_up_to_the_cap() {
        assert_eq!(max_jumps(0.0), 1);
        assert_eq!(max_jumps(TRIP_ISK_PER_JUMP - 1.0), 1);
        assert_eq!(max_jumps(TRIP_ISK_PER_JUMP), 2);
        assert_eq!(max_jumps(TRIP_ISK_PER_JUMP * 1e9), TRIP_MAX_JUMPS);
    }

    /// Sellers: nearest first, then cheapest; never home, never over the
    /// cap, never off the map, never beyond the value's reach.
    #[test]
    fn the_sellers_are_nearest_first_inside_the_cap_and_the_reach() {
        const HOME: i64 = 69_000_001;
        let m = graph(HIGH)
            .with_station(HOME, S1)
            .with_station(69_000_002, S2)
            .with_station(69_000_003, S3)
            .with_station(69_000_005, S5);
        let trips = Trips {
            map: &m,
            floor: SECURITY_FLOOR,
        };
        let far = TRIP_ISK_PER_JUMP * 2.0;
        let book = Book {
            asks: vec![
                ask(1, HOME, far, 1),
                ask(2, 69_000_003, far, 1),
                ask(3, 69_000_002, far * 2.0, 1),
                ask(4, 69_000_005, far * 1.5, 1),
                ask(5, 69_000_404, 1.0, 1),
                ask(6, 69_000_002, far * 9.0, 1),
            ],
            bids: vec![],
        };
        let got = sources(&book, trips, S1, HOME, Some(far * 3.0), 1);
        let stations: Vec<i64> = got.iter().map(|s| s.station).collect();
        assert_eq!(stations, vec![69_000_005, 69_000_002, 69_000_003]);
        assert_eq!(got[1].price, far * 2.0, "the cheapest ask under the cap");
        assert_eq!(got[2].jumps, 2);

        let cheap = Book {
            asks: vec![ask(7, 69_000_003, 1.0, 1), ask(8, 69_000_002, 1.0, 1)],
            bids: vec![],
        };
        let got = sources(&cheap, trips, S1, HOME, None, 1);
        assert_eq!(
            got.iter().map(|s| s.station).collect::<Vec<_>>(),
            vec![69_000_002],
            "a one-ISK item is fetched from next door, not two jumps away"
        );
    }

    /// **A region's nearest station** (FE-TRADE): fewest jumps, the preferred
    /// station on a tie, the lowest id after; none past the jump limit or
    /// under the floor.
    #[test]
    fn find_in_region_is_the_nearest_routable_station_of_the_region() {
        const RA: i64 = 10_000_901;
        const RB: i64 = 10_000_902;
        let mut m = graph(HIGH);
        for (st, sys, region) in [
            (61_000_001, S1, RA),
            (61_000_002, S3, RB),
            (61_000_003, S3, RB),
            (61_000_004, S4, RB),
        ] {
            m = m.with_station(st, sys).with_region(sys, region);
        }
        m = m.with_region(S2, RA);
        let t = Trips {
            map: &m,
            floor: SECURITY_FLOOR,
        };
        let near = find_in_region(t, S1, RB, 8, None).expect("RB is reachable");
        assert_eq!((near.station, near.system, near.jumps), (61_000_002, S3, 2));
        let pref = find_in_region(t, S1, RB, 8, Some(61_000_003)).expect("preferred wins a tie");
        assert_eq!(pref.station, 61_000_003);
        assert_eq!(
            find_in_region(t, S1, RB, 1, None),
            None,
            "past the jump limit"
        );
        assert_eq!(
            find_in_region(t, S1, 10_000_999, 8, None),
            None,
            "an unknown region"
        );
        assert_eq!(m.region_of_station(61_000_004), Some(RB));
        // Under the floor there is no way through.
        let low = graph(LOW).with_station(61_000_009, S4).with_region(S4, RB);
        let lt = Trips {
            map: &low,
            floor: 0.95,
        };
        assert_eq!(find_in_region(lt, S1, RB, 8, None), None);
    }

    #[test]
    fn a_system_name_is_read_back() {
        let map = Map::new().with_name(30_000_777, "Testville");
        assert_eq!(map.name_of(30_000_777), Some("Testville"));
        assert_eq!(map.name_of(1), None);
    }
}
