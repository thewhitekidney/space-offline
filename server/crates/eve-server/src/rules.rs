//! **World rules a module may change** (`docs/lanes/pr-seam-1.md`).
//!
//! A `Copy` struct on [`crate::model::SandboxConfig`], read as a field load,
//! no lock. **`Default` is today's behaviour, 1:1**: the core test
//! `default_is_1to1` pins every field to the constant it replaced, so a
//! server built with [`crate::module::Module::NONE`] changes nothing. A
//! module's `rules` function (run once at startup, in list order) and a
//! module's command-line flags edit this value at startup; after the world
//! has loaded, only [`set_boot`] and [`set_live`] write it (the world
//! module's stored settings and its `/world` command).
//!
//! A field is added here only together with the call site that reads it.
//! The spec's other starting fields (`school_books`, `orphan_books`,
//! `insurance_market_cap`) are not here yet: the first two are read where the
//! exchange table is built, which holds no config; the last has no constant
//! today to default to.

use crate::service::Registry;

/// The rules. See the module note.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rules {
    /// Multiplies the skill training pace on top of `--training-scale`
    /// (`skill_queue::training_pace`). `1.0` is no change.
    pub training_mult: f64,
    /// The ISK a new character is credited (`start::fund_new_character`);
    /// a dev server still gets its own balance.
    pub start_isk: f64,
    /// The unallocated skill points a new character is granted
    /// (`start::fund_new_character` -> `free_sp::grant`). `0` is none.
    pub start_sp: i64,
    /// How long an Upwell structure takes to anchor, in seconds
    /// (`structure::Env::anchoring_secs`, read by `structure_space`). A
    /// live setting (`/world anchortime`): it applies to structures
    /// anchored from then on. The owner's default is an hour (U3, #63); the
    /// client's own is a day.
    pub upwell_anchor_secs: i64,
    /// How long a character waits between "prepare for deletion" and the
    /// delete, in seconds (`start::prepare_delete`). A live setting (`/world
    /// deletetime`): a character already waiting keeps the time it was given.
    /// The client's own wait is days; the default is an hour (owner default,
    /// SOC-CHARDEL, reversible); CI runs with 0.
    pub char_delete_secs: i64,
}

/// The default [`Rules::char_delete_secs`]: one hour. **[A]** the owner's
/// answer is pending; falsifier: the owner picks another wait.
pub const DEFAULT_CHAR_DELETE_SECS: i64 = 3_600;
/// The bounds of [`Rules::char_delete_secs`]: none to 30 days. **[A]** only a
/// sanity bound.
pub const CHAR_DELETE_SECS_RANGE: std::ops::RangeInclusive<i64> = 0..=2_592_000;

/// The default [`Rules::upwell_anchor_secs`]: one hour (owner, 2026-10-30).
pub const DEFAULT_UPWELL_ANCHOR_SECS: i64 = 3_600;
/// The bounds of [`Rules::upwell_anchor_secs`]: a minute to a week.
/// **[A]** only a sanity bound; falsifier: the owner wants a shorter or
/// longer anchoring.
pub const UPWELL_ANCHOR_SECS_RANGE: std::ops::RangeInclusive<i64> = 60..=604_800;

/// The most `start_sp` may be. **[A]** a cap that keeps the pool far from
/// overflow; falsifier: the owner wants more.
pub const MAX_START_SP: i64 = 1_000_000_000;

impl Default for Rules {
    fn default() -> Self {
        Rules {
            training_mult: 1.0,
            start_isk: crate::start::STARTING_BALANCE,
            start_sp: 0,
            upwell_anchor_secs: DEFAULT_UPWELL_ANCHOR_SECS,
            char_delete_secs: DEFAULT_CHAR_DELETE_SECS,
        }
    }
}

impl Rules {
    /// Refuse a value the server cannot run on: `training_mult` as
    /// `GameClock::with_training` bounds the scale (1 to
    /// `MAX_TRAINING_SCALE`, finite), `start_isk` finite and not negative.
    pub fn validate(&self) -> Result<(), String> {
        let max = f64::from(crate::game_clock::MAX_TRAINING_SCALE);
        if !(1.0..=max).contains(&self.training_mult) {
            return Err(format!(
                "rules.training_mult {} is outside 1..={max}",
                self.training_mult
            ));
        }
        if !self.start_isk.is_finite() || self.start_isk < 0.0 {
            return Err(format!(
                "rules.start_isk {} is not a non-negative number",
                self.start_isk
            ));
        }
        if !(0..=MAX_START_SP).contains(&self.start_sp) {
            return Err(format!(
                "rules.start_sp {} is outside 0..={MAX_START_SP}",
                self.start_sp
            ));
        }
        if !UPWELL_ANCHOR_SECS_RANGE.contains(&self.upwell_anchor_secs) {
            return Err(format!(
                "rules.upwell_anchor_secs {} is outside {}..={}",
                self.upwell_anchor_secs,
                UPWELL_ANCHOR_SECS_RANGE.start(),
                UPWELL_ANCHOR_SECS_RANGE.end()
            ));
        }
        if !CHAR_DELETE_SECS_RANGE.contains(&self.char_delete_secs) {
            return Err(format!(
                "rules.char_delete_secs {} is outside {}..={}",
                self.char_delete_secs,
                CHAR_DELETE_SECS_RANGE.start(),
                CHAR_DELETE_SECS_RANGE.end()
            ));
        }
        Ok(())
    }
}

/// **Apply the rules once the world has loaded, before anyone is funded.**
/// Validates first; a refused value changes nothing. No queue is running a
/// changed rate yet, so nothing is frozen or announced.
pub fn set_boot(reg: &mut Registry, new: Rules) -> Result<(), String> {
    new.validate()?;
    *reg.rules_mut() = new;
    Ok(())
}

/// **Change the rules in a running world.** Validates first and returns
/// before any change on a refused value. A changed `training_mult` is applied
/// through [`crate::skill_queue::retime_all`], so time already trained is not
/// re-priced and every queue, online or not, runs at the new rate from now;
/// the online pilots are then told to re-read their skills
/// (`OnSkillForcedRefresh`). `note` goes to the online humans as a
/// `CustomNotify` message, unless it is empty.
///
/// `start_isk` and `start_sp` are read when a character is created, so they
/// change new characters only.
pub fn set_live(reg: &mut Registry, new: Rules, note: &str) -> Result<(), String> {
    new.validate()?;
    let retime = reg.config().rules.training_mult != new.training_mult;
    if retime {
        crate::skill_queue::retime_all(reg, |reg| *reg.rules_mut() = new);
    } else {
        *reg.rules_mut() = new;
    }
    reg.mark_dirty();
    let online = reg.sessions().online_characters();
    if retime && !online.is_empty() {
        reg.schedule(
            std::time::Duration::ZERO,
            crate::notify::Addressed {
                to: Some(online.clone()),
                inner: Box::new(crate::skill::ForcedRefresh),
            },
        );
    }
    if !note.is_empty() {
        let humans: Vec<i64> = online
            .into_iter()
            .filter(|id| !reg.character(*id).is_some_and(|c| c.is_bot))
            .collect();
        if !humans.is_empty() {
            reg.schedule(
                std::time::Duration::ZERO,
                crate::shutdown::ShutdownMessage {
                    text: note.to_owned(),
                    to: humans,
                },
            );
        }
    }
    Ok(())
}

/// The training pace in force: the clock's pace times `training_mult`.
pub fn training_pace_of(reg: &Registry) -> f64 {
    crate::game_clock::of(reg).training_pace() * reg.config().rules.training_mult
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field of the default is the value the code used before the
    /// field existed.
    #[test]
    fn default_is_1to1() {
        let r = Rules::default();
        assert_eq!(r.training_mult, 1.0);
        assert_eq!(r.start_isk, crate::start::STARTING_BALANCE);
        assert_eq!(r.start_isk, 1_000_000.0);
        assert_eq!(r.start_sp, 0);
        // New in U3: the owner's choice, an hour, not a retail constant.
        assert_eq!(r.upwell_anchor_secs, 3_600);
        for bad in [0, 59, 604_801, -5] {
            let r = Rules {
                upwell_anchor_secs: bad,
                ..Rules::default()
            };
            assert!(r.validate().is_err(), "{bad}");
        }
        assert_eq!(crate::model::SandboxConfig::default().rules, r);
        assert_eq!(r.validate(), Ok(()));
    }

    #[test]
    fn validate_refuses_what_the_clock_would() {
        for bad in [0.0, 0.5, f64::NAN, f64::INFINITY, 10_001.0, -2.0] {
            let r = Rules {
                training_mult: bad,
                ..Rules::default()
            };
            assert!(r.validate().is_err(), "{bad}");
        }
        let ok = Rules {
            training_mult: 10_000.0,
            ..Rules::default()
        };
        assert_eq!(ok.validate(), Ok(()));
        for bad in [-1.0, f64::NAN, f64::INFINITY] {
            let r = Rules {
                start_isk: bad,
                ..Rules::default()
            };
            assert!(r.validate().is_err(), "{bad}");
        }
        // The cap's value is an [A]; only the sign and a small value are pinned.
        let r = Rules {
            start_sp: -1,
            ..Rules::default()
        };
        assert!(r.validate().is_err());
        let r = Rules {
            start_sp: 5,
            ..Rules::default()
        };
        assert_eq!(r.validate(), Ok(()));
    }

    // set_live: the live change of the rules, on invented pilots.

    use crate::model::SandboxConfig;
    use crate::service::Request;
    use eve_marshal::PyValue;

    const FRIGATE: i64 = 3329;

    fn pilot(reg: &Registry) -> i64 {
        reg.config().characters[0].character_id
    }

    fn call(reg: &mut Registry, method: &str, args: &[PyValue], kw: &[(PyValue, PyValue)]) {
        let actor = Some(pilot(reg));
        reg.dispatch(&Request {
            service: "skillMgr2::skillHandler",
            method,
            args,
            kwargs: kw,
            bind_params: None,
            actor,
        })
        .expect("answered");
    }

    /// A world whose pilot has a queue with one frigate level in training.
    fn training_world() -> Registry {
        let mut reg = Registry::new(SandboxConfig::default());
        call(
            &mut reg,
            "PurchaseSkills",
            &[PyValue::List(vec![PyValue::Int(FRIGATE)])],
            &[],
        );
        let info = PyValue::Dict(vec![(
            PyValue::Tuple(vec![PyValue::Int(FRIGATE), PyValue::Int(1)]),
            PyValue::Int(0),
        )]);
        let kw = [(PyValue::Bool(true), crate::pyutil::bstr("activate"))];
        call(&mut reg, "SaveNewQueue", &[info], &kw);
        reg.take_pending();
        reg
    }

    fn names(reg: &mut Registry) -> Vec<&'static str> {
        reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(1))
            .iter()
            .map(|p| p.to_push().0)
            .collect()
    }

    fn online(reg: &mut Registry, id: i64, bot: bool) {
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: 1,
            session_id: 1,
            user_id: 1,
            character_id: Some(id),
        });
        reg.character_mut(id).expect("pilot").is_bot = bot;
    }

    fn remaining(reg: &Registry) -> i64 {
        let id = pilot(reg);
        let q = reg.skill_queues.get(id).expect("queue");
        let attr = crate::skill_queue::paced_attribute(reg, id);
        let rows = q.rows_in(reg.content(), &reg.skills[&id], &attr);
        rows[0].end.expect("an end") - crate::pyutil::filetime_now()
    }

    #[test]
    fn a_live_training_change_retimes_an_offline_queue_and_stores_nothing_else() {
        let mut reg = training_world();
        let before = remaining(&reg);
        let new = Rules {
            training_mult: 4.0,
            ..Rules::default()
        };
        set_live(&mut reg, new, "faster").expect("applies");
        assert_eq!(reg.config().rules, new);
        assert_eq!(training_pace_of(&reg), 4.0);
        let after = remaining(&reg);
        assert!(
            (after - before / 4).abs() < before / 50,
            "{after} vs {before}"
        );
        // Nobody is online: the clock's pushes go nowhere, but are made.
        let n = names(&mut reg);
        assert!(n.contains(&"OnNewSkillQueueSaved"), "{n:?}");
        assert!(!n.contains(&"OnSkillForcedRefresh"), "{n:?}");
        assert!(!n.contains(&"OnRemoteMessage"), "{n:?}");
    }

    #[test]
    fn an_online_human_is_told_and_a_bot_is_not_sent_the_note() {
        let mut reg = training_world();
        let id = pilot(&reg);
        online(&mut reg, id, false);
        let new = Rules {
            training_mult: 2.0,
            ..Rules::default()
        };
        set_live(&mut reg, new, "faster").expect("applies");
        let n = names(&mut reg);
        for want in [
            "OnNewSkillQueueSaved",
            "OnSkillForcedRefresh",
            "OnRemoteMessage",
        ] {
            assert!(n.contains(&want), "{want} in {n:?}");
        }
        let mut reg = training_world();
        online(&mut reg, id, true);
        set_live(&mut reg, new, "faster").expect("applies");
        let n = names(&mut reg);
        assert!(n.contains(&"OnSkillForcedRefresh"), "{n:?}");
        assert!(!n.contains(&"OnRemoteMessage"), "{n:?}");
    }

    #[test]
    fn a_paused_queue_stays_paused() {
        let mut reg = training_world();
        let id = pilot(&reg);
        reg.skill_queues.entry(id).start = None;
        let new = Rules {
            training_mult: 3.0,
            ..Rules::default()
        };
        set_live(&mut reg, new, "").expect("applies");
        assert!(!reg.skill_queues.get(id).expect("queue").is_training());
    }

    #[test]
    fn a_refused_value_changes_nothing() {
        let mut reg = training_world();
        let id = pilot(&reg);
        let before = remaining(&reg);
        for bad in [
            Rules {
                training_mult: 0.0,
                ..Rules::default()
            },
            Rules {
                start_isk: -1.0,
                ..Rules::default()
            },
            Rules {
                start_sp: -1,
                ..Rules::default()
            },
        ] {
            assert!(set_live(&mut reg, bad, "x").is_err());
            assert!(set_boot(&mut reg, bad).is_err());
        }
        assert_eq!(reg.config().rules, Rules::default());
        assert!((remaining(&reg) - before).abs() < before / 50);
        assert!(reg.skill_queues.get(id).expect("queue").is_training());
        assert!(names(&mut reg).is_empty());
    }

    #[test]
    fn a_start_isk_change_touches_no_existing_balance() {
        let mut reg = training_world();
        let id = pilot(&reg);
        let balance = reg.character(id).expect("pilot").balance;
        let new = Rules {
            start_isk: 5.0,
            start_sp: 7,
            ..Rules::default()
        };
        set_live(&mut reg, new, "").expect("applies");
        assert_eq!(reg.character(id).expect("pilot").balance, balance);
        assert_eq!(reg.config().rules.start_isk, 5.0);
        assert_eq!(crate::free_sp::of(&reg, id), 0);
    }
}
