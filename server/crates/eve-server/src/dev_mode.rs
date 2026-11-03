//! **DEV-MODE**: the slash commands a `eve-server --dev` world adds, and the
//! dev marker on the state directory. Nothing here runs on a normal server:
//! each command answers "dev only" unless [`crate::model::SandboxConfig::dev`]
//! is set, so a non-dev server has no way to grant a skill by this path.
//!
//! - `/devskills all5` -- every skill at V.
//! - `/devskills ids <id,id,..>` -- these skills and their prerequisites, at V.
//! - `/devskills hull <typeID>` -- the skills a hull requires (and theirs), at V.
//! - `/devcorp <ticker>` -- join the dev corporation `<ticker>`, founding it
//!   (no fee, the caller its CEO) if it does not exist. Sides of a roster are
//!   corporations: same side is allied, another side is not.
//! - `/devhome <system name|id>` -- dock the caller, at once and with no
//!   reconnect, at a station of that system (`/tr`'s machinery).
//!
//! The marker: a dev world file `W` has `W.dev` beside it. `eve-server --dev`
//! refuses a world file that exists without the marker, and an ordinary
//! `eve-server` refuses one that has it ([`check_marker`]).

use crate::pyutil::bstr;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The level a dev grant gives.
pub const LEVEL: i64 = 5;

/// Why a command is refused on a normal server.
const DEV_ONLY: &str = "dev only: start the server with --dev";

/// The marker file of a world file.
pub fn marker_of(world: &Path) -> PathBuf {
    let mut s = world.as_os_str().to_owned();
    s.push(".dev");
    PathBuf::from(s)
}

/// **The dev marker rule** (pure over the filesystem): `--dev` on a world
/// file that exists and is not marked is refused (it is somebody's normal
/// save), and a normal run on a marked file is refused (it is a dev world,
/// with a market no real world has). A `--dev` start marks the file's
/// directory entry, creating the directory.
pub fn check_marker(world: &Path, dev: bool) -> Result<(), String> {
    let marker = marker_of(world);
    match (dev, marker.exists(), world.exists()) {
        (true, false, true) => Err(format!(
            "--dev: {} exists and is not a dev world (no {}); refusing to touch a normal save. \
             Use another --world, or none: --dev defaults to out/dev/world.dat",
            world.display(),
            marker.display()
        )),
        (true, false, false) => {
            if let Some(dir) = world.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::write(&marker, "dev world\n").map_err(|e| format!("{}: {e}", marker.display()))
        }
        (true, true, _) => Ok(()),
        (false, true, _) => Err(format!(
            "{} is a dev world ({} exists); start it with --dev, or use another --world",
            world.display(),
            marker.display()
        )),
        (false, false, _) => Ok(()),
    }
}

/// The skills `ids` and everything they require, at [`LEVEL`], on `actor`.
/// Returns how many entries were set.
fn grant(reg: &mut Registry, actor: i64, ids: &BTreeSet<i64>) -> usize {
    let mut all = BTreeSet::new();
    let mut todo: Vec<i64> = ids.iter().copied().collect();
    while let Some(id) = todo.pop() {
        if !all.insert(id) {
            continue;
        }
        todo.extend(
            crate::skill::required_in(reg.content(), id)
                .iter()
                .map(|&(s, _)| s),
        );
    }
    let types: Vec<_> = all
        .iter()
        .filter_map(|&id| crate::skill::find_in(reg.content(), id).cloned())
        .collect();
    let Some(s) = reg.skills_mut(actor) else {
        return 0;
    };
    for t in &types {
        s.grant_type(t, LEVEL);
    }
    let n = types.len();
    reg.mark_dirty();
    reg.push(crate::skill::ForcedRefresh);
    n
}

/// `/devskills all5 | ids <id,..> | hull <typeID>`.
pub fn skills(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    if !reg.config().dev {
        return Ok(bstr(DEV_ONLY));
    }
    let Some(actor) = actor else {
        return Ok(bstr("no character to change"));
    };
    let ids: BTreeSet<i64> = match args {
        ["all5"] => crate::skill::all_in(reg.content())
            .map(|s| s.type_id)
            .collect(),
        ["ids", list] => list
            .split(',')
            .filter_map(|w| w.trim().parse().ok())
            .collect(),
        ["hull", t] => {
            let Ok(t) = t.parse::<i64>() else {
                return Ok(bstr("usage: /devskills hull <typeID>"));
            };
            let table = crate::typedata::installed_in(reg.content()).table();
            crate::drone_skills::required_skills(table, t)
                .into_iter()
                .map(|(s, _)| s)
                .collect()
        }
        _ => {
            return Ok(bstr(
                "usage: /devskills all5 | ids <id,id,..> | hull <typeID>",
            ));
        }
    };
    let n = grant(reg, actor, &ids);
    Ok(bstr(&format!("dev: {n} skills at level {LEVEL}.")))
}

/// `/devcorp <ticker>`.
pub fn corp(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    if !reg.config().dev {
        return Ok(bstr(DEV_ONLY));
    }
    let (Some(actor), [ticker]) = (actor, args) else {
        return Ok(bstr("usage: /devcorp <ticker>"));
    };
    Ok(bstr(&crate::service::corp::dev_join(reg, actor, ticker)))
}

/// `/devhome <system name|id>`.
pub fn home(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    if !reg.config().dev {
        return Ok(bstr(DEV_ONLY));
    }
    let Some(actor) = actor else {
        return Ok(bstr("no character to move"));
    };
    if args.is_empty() {
        return Ok(bstr("usage: /devhome <system name|id>"));
    }
    let want = args.join(" ");
    let system = match want.parse::<i64>() {
        Ok(id) => Some(id),
        Err(_) => reg
            .content()
            .try_get::<crate::celestial::Universe>()
            .and_then(|u| {
                u.systems()
                    .find(|s| s.name.eq_ignore_ascii_case(&want))
                    .map(|s| s.id)
            }),
    };
    let Some(system) = system else {
        return Ok(bstr(&format!("no such system '{want}'")));
    };
    let station = reg
        .config()
        .celestials
        .served_in(system)
        .find(|c| c.kind == crate::celestial::Kind::Station)
        .map(|c| c.id);
    let Some(station) = station.and_then(|id| reg.station_ref(id)) else {
        return Ok(bstr(&format!("system {system} has no station")));
    };
    // The same teleport as `/tr` (`service::relocate`): the session change
    // the client follows, the old station's guests and Local left, the new
    // ones entered. No reconnect, from a station or from space.
    // (`seat_in_system` below is BOT-ACCOUNTS' offline seating of a spawned
    // bot that is not logged in yet.)
    Ok(bstr(
        &match reg.relocate(actor, crate::service::relocate::Dest::Station(station)) {
            Ok(line) => format!("dev: {line}"),
            Err(e) => format!("dev: {e}"),
        },
    ))
}

/// Seat `actor` (docked) at a station of `system`, the way `/devhome` does
/// (also BOT-ACCOUNTS' seating of a spawned bot). Returns the station id.
/// GANG-SUMMON: `prefer` is a station of `system` to seat in when it is one
/// (the caller's own); else the first station the catalogue lists.
pub(crate) fn seat_in_system(
    reg: &mut Registry,
    actor: i64,
    system: i64,
    prefer: Option<i64>,
) -> Result<i64, String> {
    let preferred = prefer.filter(|p| {
        reg.config()
            .celestials
            .served_in(system)
            .any(|c| c.kind == crate::celestial::Kind::Station && c.id == *p)
    });
    let station = preferred.or_else(|| {
        reg.config()
            .celestials
            .served_in(system)
            .find(|c| c.kind == crate::celestial::Kind::Station)
            .map(|c| c.id)
    });
    let Some(station) = station.and_then(|id| reg.station_ref(id)) else {
        return Err(format!("system {system} has no station"));
    };
    let Some(c) = reg.character_mut(actor) else {
        return Err("no character to move".to_owned());
    };
    if c.station_id.is_none() {
        return Err("dock first".to_owned());
    }
    crate::store::StationRecord {
        station_id: station.station_id,
        owner_id: station.owner_id,
        type_id: station.type_id,
        operation_id: station.operation_id,
        solar_system_id: station.solar_system_id,
        constellation_id: station.constellation_id,
        region_id: station.region_id,
    }
    .seat(c);
    reg.mark_dirty();
    Ok(station.station_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};

    const ANN: i64 = 90_000_501;
    const BEN: i64 = 90_000_502;
    const CAL: i64 = 90_000_503;

    fn registry(dev: bool) -> Registry {
        let mut config = SandboxConfig {
            dev,
            ..SandboxConfig::default()
        };
        config.characters = [(ANN, 1), (BEN, 2), (CAL, 3)]
            .map(|(id, n)| Character {
                character_id: id,
                name: format!("Invented {n}"),
                ship_id: 100_000_500 + n,
                ..Character::default()
            })
            .to_vec();
        Registry::new(config)
    }

    fn said(v: PyValue) -> String {
        match v {
            PyValue::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
            other => format!("{other:?}"),
        }
    }

    fn level_sum(reg: &Registry, who: i64) -> i64 {
        reg.skills_for(Some(who))
            .map_or(0, |s| s.entries().map(|e| e.level).sum())
    }

    #[test]
    fn every_dev_command_is_refused_on_a_normal_server() {
        let mut reg = registry(false);
        let before = level_sum(&reg, ANN);
        for args in [
            skills(&mut reg, Some(ANN), &["all5"]),
            corp(&mut reg, Some(ANN), &["RED"]),
            home(&mut reg, Some(ANN), &["1"]),
        ] {
            assert!(said(args.unwrap()).starts_with("dev only"));
        }
        assert_eq!(level_sum(&reg, ANN), before);
        assert!(reg.corporations().all().all(|c| c.ticker != "RED"));
    }

    #[test]
    fn devskills_all5_puts_every_skill_at_five() {
        let mut reg = registry(true);
        let n = crate::skill::all_in(reg.content()).count() as i64;
        assert!(n > 0);
        skills(&mut reg, Some(ANN), &["all5"]).unwrap();
        assert_eq!(level_sum(&reg, ANN), n * LEVEL);
        // ids: a listed skill, and what it requires.
        let some = crate::skill::all_in(reg.content()).next().unwrap().type_id;
        skills(&mut reg, Some(BEN), &["ids", &some.to_string()]).unwrap();
        assert!(level_sum(&reg, BEN) >= LEVEL);
    }

    #[test]
    fn devcorp_makes_a_side_one_corporation() {
        let mut reg = registry(true);
        corp(&mut reg, Some(ANN), &["RED"]).unwrap();
        corp(&mut reg, Some(BEN), &["RED"]).unwrap();
        corp(&mut reg, Some(CAL), &["BLU"]).unwrap();
        let of = |id| reg.character(id).unwrap().corporation_id;
        assert_eq!(of(ANN), of(BEN), "one side, one corporation");
        assert_ne!(of(ANN), of(CAL), "another side, another");
        assert!(crate::service::corp::is_player_corp(&reg, of(ANN)));
    }

    #[test]
    fn the_marker_keeps_dev_and_normal_worlds_apart() {
        // A directory of this call's own: the process id alone repeats across
        // containers and reruns, and the remove below would then delete
        // another run's files. The guard removes it even when an assert fails.
        struct Dir(PathBuf);
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "eve-dev-marker-{}-{nanos}-{}",
            std::process::id(),
            CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let guard = Dir(dir.clone());
        std::fs::create_dir_all(&dir).unwrap();
        let normal = dir.join("normal.dat");
        std::fs::write(&normal, "x").unwrap();
        assert!(
            check_marker(&normal, true).is_err(),
            "--dev on a normal save"
        );
        assert!(check_marker(&normal, false).is_ok());
        let dev = dir.join("sub").join("dev.dat");
        assert!(
            check_marker(&dev, true).is_ok(),
            "a fresh dev world is marked"
        );
        assert!(marker_of(&dev).exists());
        std::fs::write(&dev, "x").unwrap();
        assert!(check_marker(&dev, true).is_ok());
        assert!(
            check_marker(&dev, false).is_err(),
            "a normal run on a dev world"
        );
        drop(guard);
    }
}
