//! **SESSION-GM**: moving a character from one place to another *while the
//! client stays connected*, and logging a character off to the select screen.
//!
//! Both are session changes the client already follows for other reasons, so
//! nothing here invents a client behaviour:
//!
//! - **Relocation** (`/tr me <id>`, `/devhome`) is the change a dock, an
//!   undock or a stargate jump sends. [`Where`] is the four-key picture of a
//!   place (`stationid`, `solarsystemid`, `solarsystemid2` and the
//!   constellation/region pair) and [`change`] is the diff between two of
//!   them, so a station-to-station move is `{stationid, ...}`, a landing in
//!   space from a station is exactly [`Registry::undock_change`]'s two keys,
//!   a landing in a station from space is [`Registry::dock_change`]'s two
//!   keys, and a system-to-system landing is [`crate::jump::session_change`]'s.
//!   Pod revival (`clones::move_into_fresh_capsule`) builds the same diff for
//!   the same reason. **[V]** for what each key does on the client:
//!   `gameui.OnSessionChanged` routes on `stationid` first, then `structureid`,
//!   then `solarsystemid` (`gameui.py:443-451`).
//! - **Landing in space** arms the jump module's arrival
//!   ([`crate::jump::arm_arrival`]) so the `beyonce` bind that follows the
//!   change seeds the roster at the chosen spot, as a gate arrival does.
//! - **Leaving a place** leaves it properly: the old station's guests hear
//!   `OnCharNoLongerInStation`, the old space's observers hear `RemoveBalls`,
//!   and Local follows [`Registry::present_system`], which reads the roster
//!   (a jumper's system), not the home station's.
//! - **Log off** (`userSvc.UserLogOffCharacter`, the Escape menu's "Log Off
//!   Character", `gameui.LogOffCharacter:473-481`): the client wraps the call
//!   in `PerformSessionChange('logoff', ...)`, treats a *truthy return* as
//!   permission to run `DoCharacterLogoff` and logs "Character logoff denied"
//!   on anything else, which left it in a stuck session change **[V]**. The
//!   answer is the whole of it: no session change goes out. The client's
//!   own `DoCharacterLogoff` clears its session, and a change sent before
//!   the answer raised in crimewatch and dogma and tripped
//!   `ClearCharacterDependantAttributes` on a `charid` of None (fci
//!   `upwell` reds). **[A]** that the client's own clear leaves a state
//!   where picking the same character works; falsified by the same
//!   character, picked again, being dropped as unchanged.

use super::Registry;
use crate::clones::StationRef;
use crate::pyutil::pypairs;
use eve_marshal::PyValue;

/// A place, as the session keys see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Where {
    /// `stationid`: docked.
    pub station: Option<i64>,
    /// `structureid`: docked in an Upwell structure (U4). Then `system` is
    /// set too, as `eveSessions` builds it.
    pub structure: Option<i64>,
    /// `solarsystemid`: in space.
    pub system: Option<i64>,
    /// `solarsystemid2`: the system, docked or not.
    pub system2: i64,
    pub constellation: i64,
    pub region: i64,
}

/// The session change from one place to another, or `None` when no key moves.
/// Only keys whose value moves are sent: a pair whose new value equals the
/// client's current one is dropped by the client, and a `shipid` in a change
/// with `solarsystemid` going away takes `invCache`'s early return
/// (`Registry::dock_change`'s note) -- the ship does not change here **[V]**.
pub(crate) fn change(old: &Where, new: &Where) -> Option<Vec<(PyValue, PyValue)>> {
    let opt = |v: Option<i64>| v.map_or(PyValue::None, PyValue::Int);
    let pair = |a: PyValue, b: PyValue| PyValue::Tuple(vec![a, b]);
    let mut kv: Vec<(&str, PyValue)> = Vec::new();
    if old.station != new.station {
        kv.push(("stationid", pair(opt(old.station), opt(new.station))));
    }
    if old.structure != new.structure {
        kv.push(("structureid", pair(opt(old.structure), opt(new.structure))));
    }
    if old.system != new.system {
        kv.push(("solarsystemid", pair(opt(old.system), opt(new.system))));
    }
    if old.system2 != new.system2 {
        kv.push((
            "solarsystemid2",
            pair(PyValue::Int(old.system2), PyValue::Int(new.system2)),
        ));
    }
    if old.constellation != new.constellation {
        kv.push((
            "constellationid",
            pair(
                PyValue::Int(old.constellation),
                PyValue::Int(new.constellation),
            ),
        ));
    }
    if old.region != new.region {
        kv.push((
            "regionid",
            pair(PyValue::Int(old.region), PyValue::Int(new.region)),
        ));
    }
    (!kv.is_empty()).then(|| pypairs(kv))
}

/// Where a relocation goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Dest {
    /// Docked in this station.
    Station(StationRef),
    /// In space in `system`, at rest at `spot`.
    Space { system: i64, spot: [f64; 3] },
}

/// How far from a pilot or ship a `/tr` to it lands, metres. `[A]`: close
/// enough to see it, far enough not to share its ball. Falsified by a landing
/// the client draws inside the target's model.
pub const BESIDE: f64 = 3_000.0;

impl Registry {
    /// The system a character is in *now*, for Local and the like: the
    /// roster's if they are in space, the jump's destination while they are
    /// between a gate and its bind, else the station they are docked at.
    ///
    /// `Character::solar_system_id` alone is the station's system -- a
    /// stargate jump never moves it -- so Local read from it left a jumper in
    /// the system they came from.
    pub(crate) fn present_system(&self, character_id: i64) -> Option<i64> {
        if let Some(p) = self.roster.get(character_id) {
            return Some(p.solar_system_id);
        }
        if let Some((_, to)) = self.jumps.destinations().find(|(c, _)| *c == character_id) {
            return Some(to);
        }
        if let Some(spot) = crate::logoff_spot::get(self, character_id) {
            return Some(spot.system);
        }
        self.character(character_id)?.solar_system_id
    }

    /// Log a character off to the selection screen **on this connection**.
    ///
    /// The connection stays: its session entry loses the character, and what
    /// a closed session does for the last window of a character runs here
    /// (the logoff stamp, which orders the select screen, and
    /// [`crate::session_hooks::ON_LOGOFF`]: modules stop, contact presence,
    /// the station guest frame, the hold on a ship in space, the fleet).
    /// Returns whether this connection had a character; no session change is
    /// sent, the client clears its own.
    pub(crate) fn character_log_off(&mut self, client_id: i64) -> bool {
        let Some(entry) = self
            .sessions
            .entries()
            .iter()
            .find(|e| e.client_id == client_id)
            .cloned()
        else {
            return false;
        };
        let Some(who) = entry.character_id else {
            return false;
        };
        self.sessions.insert(crate::session_table::SessionEntry {
            character_id: None,
            ..entry
        });
        if !self.sessions.is_online(who) {
            self.stamp_logoff(who);
            crate::session_hooks::fire(&crate::module::plan().on_logoff, self, who);
        }
        self.dirty = true;
        true
    }

    /// Where `actor` is now, as session keys. `None` for an unknown one.
    pub(crate) fn where_now(&self, actor: i64) -> Option<Where> {
        let c = self.character(actor)?;
        // In the roster, or about to be: a login at a log-off spot is in its
        // system from the session change, before the bind seats the ship.
        let here = self
            .roster
            .get(actor)
            .map(|p| p.solar_system_id)
            .or_else(|| crate::logoff_spot::get(self, actor).map(|s| s.system));
        let system2 = here.or(c.solar_system_id)?;
        let (constellation, region) = match here {
            Some(s) => crate::jump::system_row(self, actor, s)
                .map_or((c.constellation_id, c.region_id), |r| {
                    (r.constellation_id, r.region_id)
                }),
            None => (c.constellation_id, c.region_id),
        };
        let docked = self.docked_station(actor);
        let structure = docked.filter(|id| crate::structure_dock::is_structure(self, *id));
        Some(Where {
            station: docked.filter(|_| structure.is_none()),
            structure,
            // Docked in a structure the pilot is still in the system.
            system: here.or(structure.map(|_| system2)),
            system2,
            constellation,
            region,
        })
    }

    /// Take `actor`'s ship out of space for everyone watching (the docker's
    /// own park is torn down by the client's session change): locks dropped,
    /// the roster left, the grid forgotten, `RemoveBalls` to the characters
    /// who held the ball. The order is `Registry::dock_change`'s and the
    /// `CmdDock` arm's: observers are read before the presence goes.
    pub(crate) fn leave_space(&mut self, actor: i64) {
        let Some(here) = self.roster.get(actor).copied() else {
            return;
        };
        let watching: Vec<i64> = self
            .roster
            .others_in_system(here.solar_system_id, actor)
            .into_iter()
            .filter(|id| self.character(*id).is_some())
            .collect();
        crate::targeting::left_space(
            self,
            actor,
            crate::targeting::Reason::Unlocked,
            crate::targeting::Delivery::WithAnswer,
        );
        self.roster.leave(actor);
        crate::grid::forget(self, actor);
        let stamp = self.next_destiny_stamp();
        if let Some(removed) =
            crate::destiny::BallsRemoved::new(watching, stamp, vec![here.ship_id])
        {
            self.push(removed);
        }
    }

    /// **Move `actor` to `dest` now, with no reconnect.** `Ok` carries a line
    /// for the console. Refused, with nothing changed, mid-jump or between an
    /// undock and its bind (the roster does not know them yet), and for a
    /// landing in space without a live connection to send the change to.
    pub(crate) fn relocate(&mut self, actor: i64, dest: Dest) -> Result<String, String> {
        let old = self.where_now(actor).ok_or("no such character")?;
        if self.jumps.contains(actor) {
            return Err("you are between systems; try again in a moment".into());
        }
        let live = !self.sessions.of_character(actor).is_empty();
        let here = self.roster.get(actor).copied();
        if old.station.is_none() && old.structure.is_none() && here.is_none() {
            return Err("you have not arrived anywhere yet; try again in a moment".into());
        }
        match dest {
            Dest::Station(st) => {
                if old.station == Some(st.station_id) {
                    return Ok(format!("already docked in station {}", st.station_id));
                }
                // Built before anything moves: it reads "docked now".
                let departing = old
                    .station
                    .and_then(|_| self.station_guest_frame(actor, false));
                let from_structure = old.structure;
                self.leave_space(actor);
                if !self.dock_at(actor, st) {
                    return Err(format!(
                        "could not move your ship to station {}",
                        st.station_id
                    ));
                }
                if let Some(frame) = departing {
                    self.push(frame);
                }
                if let Some(id) = from_structure {
                    crate::structure_dock::left_by_teleport(self, actor, id);
                }
                let new = Where {
                    station: Some(st.station_id),
                    structure: None,
                    system: None,
                    system2: st.solar_system_id,
                    constellation: st.constellation_id,
                    region: st.region_id,
                };
                if live {
                    if let Some(c) = change(&old, &new) {
                        self.queue_session_change(actor, c);
                    }
                    self.announce_station_guest(actor, true);
                }
                self.dirty = true;
                Ok(format!(
                    "docked in station {} in system {}",
                    st.station_id, st.solar_system_id
                ))
            }
            Dest::Space { system, spot } => {
                if !live {
                    return Err("no connected client to land in space".into());
                }
                let row = crate::jump::system_row(self, actor, system)
                    .ok_or_else(|| format!("system {system} is not loaded on this server"))?;
                if here.is_some_and(|p| p.solar_system_id == system) {
                    self.teleport_within(actor, spot);
                    return Ok(format!("moved within system {system}"));
                }
                // Docked in a structure whose client holds a park, and the
                // spot is in the same system: nothing re-binds, so the ship
                // is seated now like an undock, then moved (U4-FIX).
                let in_place = old.structure.is_some()
                    && self.session_system(actor) == Some(system)
                    && crate::structure_dock::has_live_park(self, actor);
                if in_place {
                    crate::structure_dock::undock_in_place(self, actor);
                    self.teleport_within(actor, spot);
                } else {
                    let departing = old
                        .station
                        .and_then(|_| self.station_guest_frame(actor, false));
                    let from_structure = old.structure;
                    self.leave_space(actor);
                    if let Some(frame) = departing {
                        self.push(frame);
                    }
                    if let Some(id) = from_structure {
                        crate::structure_dock::left_by_teleport(self, actor, id);
                    }
                    crate::jump::arm_arrival(self, actor, system, spot);
                }
                let new = Where {
                    station: None,
                    structure: None,
                    system: Some(system),
                    system2: system,
                    constellation: row.constellation_id,
                    region: row.region_id,
                };
                if let Some(c) = change(&old, &new) {
                    self.queue_session_change(actor, c);
                }
                Ok(format!("landing in space in system {system}"))
            }
        }
    }

    /// A move inside the system the pilot is in: the ball is placed and
    /// stopped (`/tp`'s instruments, `SetBallPosition` and
    /// `SetBallVelocity`, which skip the client's state reset), and the grid
    /// is brought up to it.
    fn teleport_within(&mut self, actor: i64, spot: [f64; 3]) {
        let Some(here) = self.roster.get(actor).copied() else {
            return;
        };
        let actions = vec![
            crate::destiny::Action::SetBallPosition {
                ball: here.ship_id,
                point: spot,
            },
            crate::destiny::Action::SetBallVelocity {
                ball: here.ship_id,
                velocity: [0.0; 3],
            },
            crate::destiny::Action::Stop { ball: here.ship_id },
        ];
        if let Some(p) = self.roster.get_mut(actor) {
            for action in &actions {
                p.motion.after(*action);
            }
        }
        let stamp = self.next_destiny_stamp();
        let mut to = vec![actor];
        to.extend(self.roster.others_in_system(here.solar_system_id, actor));
        if let Some(update) = crate::destiny::ActionUpdate::new(to, stamp, actions) {
            self.push(update);
        }
        crate::grid::reconcile_in(self, here.solar_system_id);
    }

    /// What `/tr me <id>` means: a station, a system, a celestial, a ship or
    /// a character. `Err` is a line for the console.
    pub(crate) fn tr_target(&mut self, actor: i64, id: i64) -> Result<Dest, String> {
        // A station: docked inside.
        if let Some(st) = self.station_ref(id) {
            return Ok(Dest::Station(st));
        }
        // A system: the sun's warp-in point, else a station there.
        let known_system = self.celestials().system(id).is_some()
            || self.celestials().served_in(id).next().is_some();
        if known_system {
            if let Some(star) = self.celestials().star_of(id) {
                let spot = crate::warp_in::sun_warp_in(star.position, star.radius);
                return Ok(Dest::Space { system: id, spot });
            }
            let station = self
                .celestials()
                .served_in(id)
                .find(|c| c.kind == crate::celestial::Kind::Station)
                .map(|c| c.id)
                .and_then(|s| self.station_ref(s));
            return match station {
                Some(st) => Ok(Dest::Station(st)),
                None => Err(format!("system {id} has no star or station to land at")),
            };
        }
        // A celestial: beside it, where a gate arrival would be.
        if let Some(c) = self.celestials().get(id).copied() {
            let system = self
                .celestials()
                .system_of(id)
                .filter(|s| *s != 0)
                .ok_or_else(|| format!("{id} is in no system this server holds"))?;
            let index = self
                .roster
                .others_in_system(system, actor)
                .into_iter()
                .filter(|p| self.character(*p).is_some())
                .count();
            return Ok(Dest::Space {
                system,
                spot: crate::jump::arrival_position(&c, index),
            });
        }
        // A character, or the ship they fly (or any ship in space).
        let by_character = self.roster.get(id).map(|p| p.character_id);
        let pilot = by_character.or_else(|| self.roster.by_ship(id).map(|p| p.character_id));
        if let Some(pilot) = pilot {
            if pilot == actor {
                return Err("that is you".into());
            }
            let system = self.roster.get(pilot).map(|p| p.solar_system_id);
            if let Some(system) = system {
                self.advance_system(system);
            }
            if let Some(p) = self.roster.get(pilot).copied() {
                let spot = [
                    p.motion.position[0] + BESIDE,
                    p.motion.position[1],
                    p.motion.position[2],
                ];
                return Ok(Dest::Space {
                    system: p.solar_system_id,
                    spot,
                });
            }
        }
        if let Some(c) = self.character(id)
            && id != actor
        {
            // Docked, or not online: the station they are based at.
            return StationRef::of(c)
                .map(Dest::Station)
                .ok_or_else(|| format!("{id} is nowhere"));
        }
        Err(format!(
            "no station, system, celestial, ship or character {id}"
        ))
    }
}

/// `/tr me <id>` and `/tr me pos=x,y,z`, GM only (the slash console's gate,
/// `service/gm.rs`, is `ROLE_GML`).
///
/// The client's GM menu sends `/tr me <itemID>` for a celestial, a ship, a
/// character or a solar system, and the abyss and poser tools send
/// `pos=x,y,z` and `offset=x,y,z` (`menusvc.py:982-996`, `svc_poser.py:131-134`,
/// `devFunctions.SlashCmdTr`) **[V]**. `offset=` is not handled: it is
/// relative to a second argument this server has no meaning for yet.
pub(crate) fn tr(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, super::ServiceError> {
    let say = |s: &str| Ok(crate::pyutil::bstr(s));
    let Some(actor) = actor else {
        return say("no character");
    };
    let ["me", what, ..] = args else {
        return say(
            "usage: /tr me <stationID|systemID|celestialID|shipID|characterID> | /tr me pos=x,y,z",
        );
    };
    let dest = if let Some(p) = what.strip_prefix("pos=") {
        let xyz: Vec<f64> = p.split(',').filter_map(|w| w.trim().parse().ok()).collect();
        let [x, y, z] = xyz[..] else {
            return say("usage: /tr me pos=x,y,z");
        };
        let Some(here) = reg.roster().get(actor).copied() else {
            return say("you are not in space");
        };
        Dest::Space {
            system: here.solar_system_id,
            spot: [x, y, z],
        }
    } else {
        let Ok(id) = what.parse::<i64>() else {
            return say(&format!("not an id: {what}"));
        };
        match reg.tr_target(actor, id) {
            Ok(d) => d,
            Err(e) => return say(&format!("tr: {e}")),
        }
    };
    match reg.relocate(actor, dest) {
        Ok(line) => say(&format!("tr: {line}")),
        Err(e) => say(&format!("tr: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};
    use crate::session_table::SessionEntry;

    const ALICE: i64 = 1_000_001;
    const BOB: i64 = 1_000_002;
    const ALICE_SHIP: i64 = 100_000_001;
    const BOB_SHIP: i64 = 100_000_002;
    const A: i64 = 30_000_001;
    const B: i64 = 30_000_003;
    const STATION_A: i64 = 60_000_001;
    const STATION_B: i64 = 60_000_002;
    const PLANET_B: i64 = 22;
    const GATE_A: i64 = 51;

    fn catalogue() -> crate::celestial::Catalogue {
        crate::celestial::Catalogue::parse(&format!(
            "#system\t{A}\t10\t100\n\
             #system\t{B}\t11\t101\n\
             planet\t12\t11\t500.0\t100000000.0\t0.0\t0.0\t{A}\t0\t0\n\
             gate\t{GATE_A}\t29624\t2473.0\t5000000000.0\t0.0\t0.0\t{A}\t52\t{B}\n\
             star\t30\t6\t696000000.0\t0.0\t0.0\t0.0\t{B}\t0\t0\n\
             planet\t{PLANET_B}\t11\t500.0\t0.0\t100000000.0\t0.0\t{B}\t0\t0\n"
        ))
    }

    fn station(id: i64, system: i64, con: i64, region: i64) -> StationRef {
        StationRef {
            station_id: id,
            owner_id: 1_999_001,
            type_id: 990_001,
            operation_id: 1,
            solar_system_id: system,
            constellation_id: con,
            region_id: region,
        }
    }

    /// Alice and Bob, both docked at station A (their home), both connected;
    /// station B is known in system B.
    fn world() -> Registry {
        let home = station(STATION_A, A, 10, 100);
        let pilot = |id, name: &str, ship| {
            let mut c = Character {
                character_id: id,
                name: name.to_owned(),
                ship_id: ship,
                ..Character::default()
            };
            crate::store::StationRecord {
                station_id: home.station_id,
                owner_id: home.owner_id,
                type_id: home.type_id,
                operation_id: home.operation_id,
                solar_system_id: home.solar_system_id,
                constellation_id: home.constellation_id,
                region_id: home.region_id,
            }
            .seat(&mut c);
            c
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice", ALICE_SHIP),
                pilot(BOB, "Bob", BOB_SHIP),
            ],
            celestials: catalogue(),
            ..SandboxConfig::default()
        });
        reg.stations.insert(STATION_A, home);
        reg.stations
            .insert(STATION_B, station(STATION_B, B, 11, 101));
        for (client, who) in [(1, ALICE), (2, BOB)] {
            reg.session_opened(SessionEntry {
                client_id: client,
                session_id: client,
                user_id: client,
                character_id: Some(who),
            });
        }
        reg
    }

    fn said(v: PyValue) -> String {
        match v {
            PyValue::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
            other => format!("{other:?}"),
        }
    }

    fn keys(change: &[(PyValue, PyValue)]) -> Vec<String> {
        let mut k: Vec<String> = change
            .iter()
            .map(|(_, k)| match k {
                PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                other => panic!("key {other:?}"),
            })
            .collect();
        k.sort();
        k
    }

    fn pair_of(change: &[(PyValue, PyValue)], key: &str) -> (PyValue, PyValue) {
        let (v, _) = change
            .iter()
            .find(|(_, k)| *k == crate::pyutil::bstr(key))
            .unwrap_or_else(|| panic!("no {key}"));
        let PyValue::Tuple(p) = v else { panic!("pair") };
        (p[0].clone(), p[1].clone())
    }

    fn undocked(reg: &mut Registry, who: i64) {
        crate::destiny::on_ballpark_bind(reg, Some(who));
        let _ = reg.take_pending();
    }

    #[test]
    fn a_log_off_clears_the_session_and_stamps_the_logoff() {
        let mut reg = world();
        let before = reg.character(ALICE).unwrap().logoff_date;
        assert!(reg.character_log_off(1), "a character was logged off");
        assert!(
            !reg.sessions().is_online(ALICE),
            "no character on that connection"
        );
        assert_eq!(reg.sessions().len(), 2, "the connection stays");
        assert!(
            reg.character(ALICE).unwrap().logoff_date > before,
            "stamped"
        );
        assert!(!reg.character_log_off(1), "nothing to log off twice");
        // The same character can be selected again on the same connection.
        reg.session_selected_character(1, ALICE);
        assert!(reg.sessions().is_online(ALICE));
    }

    #[test]
    fn a_second_window_keeps_the_character_online_through_a_log_off() {
        let mut reg = world();
        reg.session_opened(SessionEntry {
            client_id: 3,
            session_id: 3,
            user_id: 1,
            character_id: Some(ALICE),
        });
        let before = reg.character(ALICE).unwrap().logoff_date;
        assert!(reg.character_log_off(1));
        assert!(reg.sessions().is_online(ALICE));
        assert_eq!(reg.character(ALICE).unwrap().logoff_date, before);
    }

    #[test]
    fn the_diff_is_what_dock_undock_and_jump_send() {
        let docked = Where {
            station: Some(1),
            structure: None,
            system: None,
            system2: 5,
            constellation: 6,
            region: 7,
        };
        let space = Where {
            station: None,
            system: Some(5),
            ..docked
        };
        assert_eq!(
            keys(&change(&docked, &space).unwrap()),
            ["solarsystemid", "stationid"]
        );
        assert_eq!(
            keys(&change(&space, &docked).unwrap()),
            ["solarsystemid", "stationid"]
        );
        assert!(change(&docked, &docked).is_none());
        let far = Where {
            system: Some(8),
            system2: 8,
            constellation: 9,
            region: 7,
            ..space
        };
        assert_eq!(
            keys(&change(&space, &far).unwrap()),
            ["constellationid", "solarsystemid", "solarsystemid2"]
        );
    }

    #[test]
    fn tr_to_a_station_docks_you_there_and_moves_the_guest_lists() {
        let mut reg = world();
        let line = tr(&mut reg, Some(ALICE), &["me", &STATION_B.to_string()]).unwrap();
        assert!(said(line).contains("docked in station"), "docked");
        let c = reg.character(ALICE).unwrap();
        assert_eq!(c.station_id, Some(STATION_B));
        assert_eq!(c.solar_system_id, Some(B));
        assert_eq!(reg.docked_station(ALICE), Some(STATION_B));
        // The session change: station and system moved, the system is not
        // "in space".
        let queued = reg.take_session_changes();
        assert_eq!(queued.len(), 1);
        let (who, change) = &queued[0];
        assert_eq!(*who, ALICE);
        assert_eq!(
            pair_of(change, "stationid"),
            (PyValue::Int(STATION_A), PyValue::Int(STATION_B))
        );
        assert_eq!(
            pair_of(change, "solarsystemid2"),
            (PyValue::Int(A), PyValue::Int(B))
        );
        assert!(keys(change).contains(&"regionid".to_owned()));
        assert!(!keys(change).contains(&"solarsystemid".to_owned()));
        // Bob, left in station A, hears Alice leave; nobody is in B to hear
        // her arrive.
        let sent = reg.take_pending();
        let leaving: Vec<_> = sent
            .iter()
            .map(|(_, p)| p.to_push())
            .filter(|(name, _)| *name == "OnCharNoLongerInStation")
            .collect();
        assert_eq!(leaving.len(), 1, "one departure frame");
        assert_eq!(reg.characters_at_station(STATION_A).len(), 1, "Bob alone");
        assert_eq!(reg.characters_at_station(STATION_B).len(), 1, "Alice alone");
    }

    #[test]
    fn arriving_where_someone_is_docked_tells_them() {
        let mut reg = world();
        tr(&mut reg, Some(ALICE), &["me", &STATION_B.to_string()]).unwrap();
        let _ = reg.take_pending();
        let _ = reg.take_session_changes();
        tr(&mut reg, Some(BOB), &["me", &STATION_B.to_string()]).unwrap();
        let sent = reg.take_pending();
        let names: Vec<_> = sent.iter().map(|(_, p)| p.to_push().0).collect();
        assert!(names.contains(&"OnCharNowInStation"), "{names:?}");
    }

    #[test]
    fn tr_to_a_system_lands_in_space_and_the_bind_seeds_the_spot() {
        let mut reg = world();
        let line = said(tr(&mut reg, Some(ALICE), &["me", &B.to_string()]).unwrap());
        assert!(line.contains("landing"), "{line}");
        // Jump-style: not docked, not in the roster, mid-arrival.
        assert!(reg.docked_station(ALICE).is_none());
        let queued = reg.take_session_changes();
        let (_, change) = &queued[0];
        assert_eq!(
            pair_of(change, "stationid"),
            (PyValue::Int(STATION_A), PyValue::None)
        );
        assert_eq!(
            pair_of(change, "solarsystemid"),
            (PyValue::None, PyValue::Int(B))
        );
        // Local follows at once: the destination, not the home.
        assert_eq!(reg.present_system(ALICE), Some(B));
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let p = reg.roster().get(ALICE).copied().expect("in space");
        assert_eq!(p.solar_system_id, B);
        assert_eq!(reg.present_system(ALICE), Some(B));
    }

    #[test]
    fn tr_to_a_celestial_lands_beside_it_and_from_space_the_change_is_the_jumps() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        let _ = reg.take_session_changes();
        let _ = tr(&mut reg, Some(ALICE), &["me", &PLANET_B.to_string()]).unwrap();
        let queued = reg.take_session_changes();
        let (_, change) = &queued[0];
        assert_eq!(
            keys(change),
            [
                "constellationid",
                "regionid",
                "solarsystemid",
                "solarsystemid2"
            ]
        );
        assert_eq!(
            pair_of(change, "solarsystemid"),
            (PyValue::Int(A), PyValue::Int(B))
        );
        assert!(reg.roster().get(ALICE).is_none(), "off the old grid");
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let planet = reg.celestials().get(PLANET_B).unwrap().position;
        let spot = reg.roster().get(ALICE).unwrap().motion.position;
        let d = crate::space_state::distance(planet, spot);
        assert!(d < 2_000.0 + 500.0 + 1.0, "beside the planet: {d}");
    }

    #[test]
    fn tr_to_a_character_in_space_lands_beside_them_and_to_a_docked_one_docks() {
        let mut reg = world();
        undocked(&mut reg, BOB);
        let _ = reg.take_session_changes();
        // Alice, docked in A, to Bob in space in A: undock-style change.
        let _ = tr(&mut reg, Some(ALICE), &["me", &BOB.to_string()]).unwrap();
        let queued = reg.take_session_changes();
        let (_, change) = &queued[0];
        assert_eq!(keys(change), ["solarsystemid", "stationid"]);
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let a = reg.roster().get(ALICE).unwrap().motion.position;
        let b = reg.roster().get(BOB).unwrap().motion.position;
        let d = crate::space_state::distance(a, b);
        assert!((d - BESIDE).abs() < 100.0, "{d}");
        // By the ship id too.
        let t = reg.tr_target(ALICE, BOB_SHIP).unwrap();
        assert!(matches!(t, Dest::Space { system, .. } if system == A));
        assert!(reg.tr_target(ALICE, ALICE).is_err(), "not yourself");
    }

    #[test]
    fn tr_from_space_to_a_station_leaves_the_roster_and_removes_the_ball() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        undocked(&mut reg, BOB);
        let _ = reg.take_session_changes();
        let _ = tr(&mut reg, Some(ALICE), &["me", &STATION_B.to_string()]).unwrap();
        assert!(reg.roster().get(ALICE).is_none());
        assert_eq!(reg.docked_station(ALICE), Some(STATION_B));
        let queued = reg.take_session_changes();
        let (_, change) = &queued[0];
        assert_eq!(
            pair_of(change, "solarsystemid"),
            (PyValue::Int(A), PyValue::None)
        );
        let sent = reg.take_pending();
        assert!(
            sent.iter().any(|(_, p)| p.to_push().0 == "DoDestinyUpdate"),
            "Bob is told the ball left"
        );
    }

    #[test]
    fn tr_within_a_system_moves_the_ball_with_no_session_change() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        let _ = reg.take_session_changes();
        let _ = tr(&mut reg, Some(ALICE), &["me", "pos=1000,2000,3000"]).unwrap();
        assert!(reg.take_session_changes().is_empty());
        let p = reg.roster().get(ALICE).unwrap().motion.position;
        assert_eq!(p, [1000.0, 2000.0, 3000.0]);
    }

    #[test]
    fn tr_refuses_nonsense_and_changes_nothing() {
        let mut reg = world();
        for args in [
            vec!["me"],
            vec!["you", "1"],
            vec!["me", "abc"],
            vec!["me", "99"],
        ] {
            let _ = tr(&mut reg, Some(ALICE), &args).unwrap();
        }
        assert_eq!(reg.docked_station(ALICE), Some(STATION_A));
        assert!(reg.take_session_changes().is_empty());
    }

    #[test]
    fn a_pilot_who_jumped_is_in_the_destination_local() {
        let mut reg = world();
        undocked(&mut reg, ALICE);
        reg.roster_mut().leave(ALICE);
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                ALICE,
                ALICE_SHIP,
                B,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        let rooms = crate::chat::system_rooms(&reg, ALICE);
        assert!(rooms.contains(&format!("local_{B}")), "{rooms:?}");
        assert!(!rooms.contains(&format!("local_{A}")), "{rooms:?}");
    }
}
