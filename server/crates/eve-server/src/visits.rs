//! **The map's personal answers** (lane `map-1b`, EVEJS-GAP svcC S5): the
//! systems a pilot has been to, their corpmates, their agents, and the three
//! overlays this server has nothing to say for.
//!
//! The client asks `map.GetSolarSystemVisits`, `GetMyExtraMapInfo`,
//! `GetMyExtraMapInfoAgents`, `GetIndustryCostModifier`,
//! `GetSystemsInIncursions` and `GetAllRoamingWeatherSystems`; each raised
//! on a missing method before this module.
//!
//! **Visits** are the delta kind [`KIND`]: `char system -> visits
//! last_filetime`. [`on_session_change`] is called from
//! `Registry::queue_session_change` and counts a change that holds
//! `solarsystemid2 = (old, new)` with `new > 0 && new != old`: gates,
//! wormholes, jump drives and bridges alike. **Bounded**: a row per
//! (character, system), so at most the number of systems per human, and
//! **bot characters are skipped** (a bot never opens the map; 500-1000 bots
//! jumping would otherwise write a row per system each). Logging in to a
//! system is not a visit [A]; falsifier: TQ's visited filter lighting the
//! login system at once.

use crate::service::Registry;
use eve_marshal::{Column, DbRowDescriptor, DbType, DbValue, PyValue};

/// The delta kind: `char system -> visits last_filetime`.
pub const KIND: &str = "visit";

fn key(character: i64, system: i64) -> Vec<String> {
    vec![character.to_string(), system.to_string()]
}

/// Count one visit to `system`, keeping the later time.
pub fn record(w: &mut Registry, character: i64, system: i64, at: i64) {
    let (visits, last) = w
        .deltas()
        .of(KIND)
        .get(&key(character, system))
        .map(|v| {
            let n = |i: usize| v.get(i).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
            (n(0), n(1))
        })
        .unwrap_or((0, 0));
    w.deltas_mut().put(
        KIND,
        key(character, system),
        vec![(visits + 1).to_string(), last.max(at).to_string()],
    );
}

/// `(last_filetime, system, visits)` for one character, in system-key order.
pub fn of(w: &Registry, character: i64) -> Vec<(i64, i64, i64)> {
    let first = character.to_string();
    w.deltas()
        .of(KIND)
        .range(vec![first.clone()]..)
        .take_while(|(k, _)| k.first() == Some(&first))
        .filter_map(|(k, v)| {
            let system = k.get(1)?.parse().ok()?;
            let visits = v.first()?.parse().ok()?;
            let last = v.get(1)?.parse().ok()?;
            Some((last, system, visits))
        })
        .collect()
}

/// **The producer**: called with every queued session change. Records a visit
/// when the change moves the character into another system.
pub fn on_session_change(w: &mut Registry, character: i64, change: &[(PyValue, PyValue)]) {
    let Some(PyValue::Tuple(t)) = crate::pyutil::pairs_get(change, b"solarsystemid2") else {
        return;
    };
    let [PyValue::Int(old), PyValue::Int(new)] = t[..] else {
        return;
    };
    if new <= 0 || new == old || w.character(character).is_none_or(|c| c.is_bot) {
        return;
    }
    record(w, character, new, crate::pyutil::filetime_now());
}

fn rowset(columns: &[(&str, DbType)]) -> eve_rowset::CRowset {
    let columns = columns
        .iter()
        .map(|(name, db_type)| Column {
            name: name.as_bytes().to_vec(),
            db_type: *db_type,
        })
        .collect();
    eve_rowset::CRowset::new(DbRowDescriptor::new(columns).expect("the map columns are valid"))
}

/// I4 columns saturate rather than failing the row.
fn i4(n: i64) -> DbValue {
    DbValue::Int(n.clamp(i64::from(i32::MIN), i64::from(i32::MAX)))
}

fn visits_rowset(w: &Registry, actor: Option<i64>) -> eve_rowset::CRowset {
    let mut rs = rowset(&[
        ("lastDateTime", DbType::FileTime),
        ("solarSystemID", DbType::I4),
        ("visits", DbType::I4),
    ]);
    for (last, system, visits) in actor.map(|c| of(w, c)).unwrap_or_default() {
        rs.push_values(vec![DbValue::Int(last), i4(system), i4(visits)])
            .expect("the visit row matches its descriptor");
    }
    rs
}

/// `map.GetSolarSystemVisits()`.
pub fn visits_answer(w: &Registry, actor: Option<i64>) -> PyValue {
    visits_rowset(w, actor).to_pyvalue()
}

/// Where a corpmate is (`Registry::session_system`).
fn system_of(w: &Registry, character: i64) -> Option<i64> {
    w.session_system(character)
}

pub(crate) fn members_rowset(w: &Registry, actor: Option<i64>) -> Option<eve_rowset::CRowset> {
    let me = actor?;
    let (corp, _) = w.corp_membership(me)?;
    let mut rs = rowset(&[("characterID", DbType::I4), ("locationID", DbType::I4)]);
    for id in w.corporations().members_of(corp) {
        if id == me || !w.sessions().is_online(id) {
            continue;
        }
        if let Some(system) = system_of(w, id) {
            rs.push_values(vec![i4(id), i4(system)])
                .expect("the member row matches its descriptor");
        }
    }
    Some(rs)
}

/// `map.GetMyExtraMapInfo()`: `None` in an NPC corporation [A: TQ shows no
/// NPC-corp members], else every online corpmate but the caller at their
/// system [A: TQ may list offline members at their last system].
pub fn members_answer(w: &Registry, actor: Option<i64>) -> PyValue {
    members_rowset(w, actor).map_or(PyValue::None, |r| r.to_pyvalue())
}

fn agents_rowset(actor: Option<i64>, w: &Registry) -> eve_rowset::CRowset {
    let mut rs = rowset(&[("fromID", DbType::I4), ("rank", DbType::R5)]);
    if let Some(me) = actor {
        for (from, standing) in w.standings.toward(me) {
            rs.push_values(vec![i4(from), DbValue::Real(standing / 10.0)])
                .expect("the agent row matches its descriptor");
        }
    }
    rs
}

/// `map.GetMyExtraMapInfoAgents()`: one row per standing toward the caller,
/// `rank = standing / 10` (the client multiplies it back by 10).
pub fn agents_answer(w: &Registry, actor: Option<i64>) -> PyValue {
    agents_rowset(actor, w).to_pyvalue()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};
    use crate::session_table::SessionEntry;

    const A: i64 = 30_900_001;
    const B: i64 = 30_900_002;
    const ME: i64 = 90_000_001;
    const MATE: i64 = 90_000_002;
    const BOT: i64 = 90_000_004;

    fn world() -> Registry {
        let chars = [ME, MATE, BOT]
            .into_iter()
            .map(|id| Character {
                character_id: id,
                ship_id: id + 100,
                solar_system_id: Some(A),
                is_bot: id == BOT,
                ..Character::default()
            })
            .collect();
        let mut reg = Registry::new(SandboxConfig {
            characters: chars,
            ..SandboxConfig::default()
        });
        for (n, id) in [ME, MATE, BOT].into_iter().enumerate() {
            let n = n as i64 + 1;
            reg.session_opened(SessionEntry {
                client_id: n,
                session_id: n,
                user_id: n,
                character_id: Some(id),
            });
        }
        reg
    }

    fn change(old: i64, new: i64) -> Vec<(PyValue, PyValue)> {
        crate::jump::session_change(
            Some(crate::celestial::SystemRow {
                id: old,
                constellation_id: 0,
                region_id: 0,
            }),
            Some(crate::celestial::SystemRow {
                id: new,
                constellation_id: 0,
                region_id: 0,
            }),
        )
    }

    #[test]
    fn a_system_change_records_one_visit_and_another_key_none() {
        let mut reg = world();
        reg.queue_session_change(ME, change(A, B));
        assert_eq!(of(&reg, ME).len(), 1);
        assert_eq!(of(&reg, ME)[0].1, B);
        assert_eq!(of(&reg, ME)[0].2, 1);
        reg.queue_session_change(
            ME,
            crate::pyutil::pypairs(vec![("shipid", PyValue::Int(5))]),
        );
        assert_eq!(of(&reg, ME).len(), 1);
        // The same system on both sides is not a visit.
        reg.queue_session_change(ME, change(B, B));
        assert_eq!(of(&reg, ME)[0].2, 1);
    }

    #[test]
    fn two_visits_count_two_and_keep_the_later_time() {
        let mut reg = world();
        record(&mut reg, ME, B, 500);
        record(&mut reg, ME, B, 300);
        assert_eq!(of(&reg, ME), vec![(500, B, 2)]);
        assert!(of(&reg, MATE).is_empty());
        let rs = visits_rowset(&reg, Some(ME));
        assert_eq!(rs.len(), 1);
        assert_eq!(rs.rows()[0].values()[2], DbValue::Int(2));
    }

    #[test]
    fn a_bot_leaves_no_visits() {
        let mut reg = world();
        reg.queue_session_change(BOT, change(A, B));
        assert!(of(&reg, BOT).is_empty());
        assert!(reg.deltas().of(KIND).is_empty());
    }

    #[test]
    fn agents_rank_is_the_standing_over_ten() {
        let mut reg = world();
        reg.standings.set(ME, 3_010_001, 5.0);
        let rs = agents_rowset(Some(ME), &reg);
        assert_eq!(rs.len(), 1);
        assert_eq!(rs.rows()[0].values()[0], DbValue::Int(3_010_001));
        assert_eq!(rs.rows()[0].values()[1], DbValue::Real(0.5));
    }
}
