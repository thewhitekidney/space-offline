//! The settings table, the startup merge, the `/world` command and the flags.

use crate::KIND;
use eve_marshal::PyValue;
use eve_proto::user_error;
use eve_server::module::Flag;
use eve_server::rules::{self, Rules};
use eve_server::service::{Registry, ServiceError};

/// When a setting may change. A world is made once; a first-launch setting is
/// seeded by the flags then and refused by `/world` after.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Changes with `/world <name> <value>` while the world runs.
    Live,
    /// Seeded by the flags when the world is made; `/world` refuses it after.
    FirstLaunch,
}

/// One world setting. `get` and `set` go through `f64` so one stored shape
/// serves them all; a setting that is whole numbers refuses a fraction.
#[derive(Clone, Copy)]
pub struct Setting {
    /// What the GM types and the row key.
    pub name: &'static str,
    pub usage: &'static str,
    pub kind: Kind,
    pub get: fn(&Rules) -> f64,
    pub set: fn(&mut Rules, f64) -> Result<(), String>,
}

fn whole(v: f64) -> Result<i64, String> {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 9.0e15 {
        Ok(v as i64)
    } else {
        Err(format!("{v} is not a whole number"))
    }
}

/// Every setting, in the order `/world` lists them.
pub const SETTINGS: &[Setting] = &[
    Setting {
        name: "training",
        usage: "skill training speed, times the clock's pace; applies to every queue at once",
        kind: Kind::Live,
        get: |r| r.training_mult,
        set: |r, v| {
            r.training_mult = v;
            Ok(())
        },
    },
    Setting {
        name: "startisk",
        usage: "ISK a new character starts with; first-launch, set by --world-start-isk when the world is made",
        kind: Kind::FirstLaunch,
        get: |r| r.start_isk,
        set: |r, v| {
            r.start_isk = v;
            Ok(())
        },
    },
    Setting {
        name: "startsp",
        usage: "unallocated skill points a new character starts with (whole number); first-launch, set by --world-start-sp when the world is made",
        kind: Kind::FirstLaunch,
        get: |r| r.start_sp as f64,
        set: |r, v| {
            r.start_sp = whole(v)?;
            Ok(())
        },
    },
    Setting {
        name: "anchortime",
        usage: "seconds an Upwell structure takes to anchor (whole number, 60 to 604800); applies to structures anchored from then on",
        kind: Kind::Live,
        get: |r| r.upwell_anchor_secs as f64,
        set: |r, v| {
            r.upwell_anchor_secs = whole(v)?;
            Ok(())
        },
    },
    Setting {
        name: "deletetime",
        usage: "seconds a character waits between asking to be deleted and the delete (whole number, 0 to 2592000); applies from the next ask",
        kind: Kind::Live,
        get: |r| r.char_delete_secs as f64,
        set: |r, v| {
            r.char_delete_secs = whole(v)?;
            Ok(())
        },
    },
];

fn key(name: &str) -> Vec<String> {
    vec![name.to_owned()]
}

/// The `world_loaded` hook. Start from the rules the flags settled; for each
/// setting a stored row overrides, and an absent row stores what the world
/// runs on. An unreadable row refuses startup; a row this build does not know
/// is kept and logged.
pub fn startup(reg: &mut Registry) -> Result<(), String> {
    let mut now = reg.config().rules;
    for s in SETTINGS {
        let stored = reg.deltas().of(KIND).get(&key(s.name)).cloned();
        match stored {
            Some(row) => {
                let raw = row.first().map_or("", String::as_str);
                let v: f64 = raw.parse().map_err(|_| {
                    format!(
                        "world setting {}: stored value {raw:?} is not a number",
                        s.name
                    )
                })?;
                let cur = (s.get)(&now);
                if cur != v {
                    println!(
                        "[world] {}: the flags say {cur}, the world says {v}; \
                         the world wins{}",
                        s.name,
                        if s.kind == Kind::Live {
                            format!(" (/world {} <value> changes it)", s.name)
                        } else {
                            " (a first-launch setting)".to_owned()
                        }
                    );
                }
                (s.set)(&mut now, v).map_err(|e| format!("world setting {}: {e}", s.name))?;
            }
            None => {
                let v = (s.get)(&now);
                reg.deltas_mut().put(KIND, key(s.name), vec![v.to_string()]);
            }
        }
    }
    let unknown: Vec<String> = reg
        .deltas()
        .of(KIND)
        .keys()
        .filter_map(|k| k.first())
        .filter(|n| !SETTINGS.iter().any(|s| s.name == n.as_str()))
        .cloned()
        .collect();
    for n in unknown {
        println!("[world] stored setting {n:?} is not known to this build; kept");
    }
    rules::set_boot(reg, now)?;
    println!(
        "[world] training {} startisk {} startsp {} anchortime {} deletetime {}",
        now.training_mult,
        now.start_isk,
        now.start_sp,
        now.upwell_anchor_secs,
        now.char_delete_secs
    );
    Ok(())
}

/// A reply the GM can see: a raised `CustomInfo` (the slash result window is
/// not open on a client), `info` first as `gm.rs` does.
fn reply(text: &str) -> Result<PyValue, ServiceError> {
    Err(ServiceError::Raise(user_error(
        "CustomInfo",
        vec![(
            PyValue::Bytes(text.as_bytes().to_vec()),
            PyValue::Bytes(b"info".to_vec()),
        )],
    )))
}

fn listing(reg: &Registry) -> String {
    let r = reg.config().rules;
    let mut out = String::from("World settings:");
    for s in SETTINGS {
        out.push_str(&format!(
            "\n/world {} = {}{}  ({})",
            s.name,
            (s.get)(&r),
            if s.kind == Kind::FirstLaunch {
                " [first-launch, fixed]"
            } else {
                ""
            },
            s.usage
        ));
    }
    out.push_str(&format!(
        "\nSkills train {}x now.",
        rules::training_pace_of(reg)
    ));
    out
}

/// `/world` lists (first-launch settings marked); `/world <name> <value>` changes a setting and stores it.
/// The slash console's GM gate has already run. A bad name or value answers
/// with the usage and changes nothing.
pub fn slash(
    reg: &mut Registry,
    _actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let [name, value] = args else {
        return reply(&listing(reg));
    };
    let Some(s) = SETTINGS.iter().find(|s| s.name.eq_ignore_ascii_case(name)) else {
        return reply(&format!("No setting {name:?}.\n{}", listing(reg)));
    };
    if s.kind == Kind::FirstLaunch {
        return reply(&format!(
            "{} is a first-launch setting: it was fixed when this world was made (--world-{}) and cannot change now.\n{}",
            s.name,
            if s.name == "startisk" {
                "start-isk"
            } else {
                "start-sp"
            },
            listing(reg)
        ));
    }
    let cleaned: String = value.chars().filter(|c| *c != ',' && *c != '_').collect();
    let Ok(v) = cleaned.parse::<f64>() else {
        return reply(&format!("{value:?} is not a number.\n{}", listing(reg)));
    };
    let mut new = reg.config().rules;
    if let Err(e) = (s.set)(&mut new, v) {
        return reply(&format!("{}: {e}", s.name));
    }
    let note = match s.name {
        "training" => format!("Skill training speed is now {v}x the base pace."),
        "anchortime" => format!("Upwell structures now take {v} seconds to anchor."),
        "deletetime" => format!("A character now waits {v} seconds before it can be deleted."),
        other => format!("{other} is now {v}."),
    };
    if let Err(e) = rules::set_live(reg, new, &note) {
        return reply(&format!("{}: {e}", s.name));
    }
    reg.deltas_mut().put(KIND, key(s.name), vec![v.to_string()]);
    println!("[world] {} set to {v}", s.name);
    reply(&format!("{note}\n{}", listing(reg)))
}

fn number(v: Option<&str>) -> Result<f64, String> {
    v.ok_or("needs a value")?
        .parse::<f64>()
        .map_err(|_| "not a number".to_owned())
}

fn flag_training(r: &mut Rules, v: Option<&str>) -> Result<(), String> {
    r.training_mult = number(v)?;
    Ok(())
}

fn flag_start_isk(r: &mut Rules, v: Option<&str>) -> Result<(), String> {
    r.start_isk = number(v)?;
    Ok(())
}

fn flag_start_sp(r: &mut Rules, v: Option<&str>) -> Result<(), String> {
    r.start_sp = v
        .ok_or("needs a value")?
        .parse::<i64>()
        .map_err(|_| "not a whole number".to_owned())?;
    Ok(())
}

fn flag_anchor_time(r: &mut Rules, v: Option<&str>) -> Result<(), String> {
    r.upwell_anchor_secs = v
        .ok_or("needs a value")?
        .parse::<i64>()
        .map_err(|_| "not a whole number".to_owned())?;
    Ok(())
}

fn flag_delete_time(r: &mut Rules, v: Option<&str>) -> Result<(), String> {
    r.char_delete_secs = v
        .ok_or("needs a value")?
        .parse::<i64>()
        .map_err(|_| "not a whole number".to_owned())?;
    Ok(())
}

/// The startup flags. They seed a new world; a stored value wins afterwards.
pub const FLAGS: &[Flag] = &[
    Flag {
        name: "--world-training",
        takes_value: true,
        usage: "skill training speed multiplier for a new world (default 1)",
        apply: flag_training,
    },
    Flag {
        name: "--world-start-isk",
        takes_value: true,
        usage: "ISK a new character starts with, in a new world",
        apply: flag_start_isk,
    },
    Flag {
        name: "--world-start-sp",
        takes_value: true,
        usage: "unallocated skill points a new character starts with, in a new world",
        apply: flag_start_sp,
    },
    Flag {
        name: "--world-anchor-time",
        takes_value: true,
        usage: "seconds an Upwell structure takes to anchor, in a new world (default 3600)",
        apply: flag_anchor_time,
    },
    Flag {
        name: "--world-delete-time",
        takes_value: true,
        usage: "seconds a character waits before it can be deleted, in a new world (default 3600)",
        apply: flag_delete_time,
    },
];
