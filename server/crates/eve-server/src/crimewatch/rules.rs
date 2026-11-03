//! **The crime rules, pure** (CW-1): who becomes a suspect or a criminal for
//! what, in which security band, and which safety setting allows it.
//!
//! Every rule here is the client's own, read from `crimewatchSvc.py`
//! (build 3619879) **[V]** -- the client asks the same questions before it
//! lets a module fire, so a server that answers them differently is a
//! server whose players see one thing and get another:
//!
//! - **attacking** (`GetSafetyLevelRestrictionForAttackingTarget`): nothing
//!   below low sec; nothing against a target the attacker `CanAttackFreely`;
//!   in high sec a criminal act unless the target's group is one CONCORD
//!   ignores (`DoesConcordReact`, `corruptionsuppression/systemEffects.py`:
//!   a capsule always, a ship or a drone always); in low sec a criminal act
//!   against a **capsule** and a suspect act against anything else;
//! - **`CanAttackFreely`**: one's own; the target criminally flagged
//!   (suspect or criminal); a limited engagement with its owner; an outlaw
//!   owner (`IsOutlawStatus`, security status rounded to one decimal `<=
//!   -5`); an enemy militia; a player corporation's own member while its
//!   friendly fire is legal; a war target -- `[A]` none yet, C8's to add;
//! - **assisting** (`GetSafetyLevelRestrictionForAidingTarget`): a criminal
//!   target makes a criminal, a suspect target a suspect, anything else
//!   nothing;
//! - **taking from a container without loot rights** (`HaveLootRight` and
//!   `CheckCanTakeItems`): a suspect act, unless the owner is free to
//!   attack (`HaveLootRight` asks `CanAttackFreely` of the container);
//! - **the safety a crime needs** (`CheckUnsafe`: refused when `required <
//!   safety`): a criminal act needs safety `None` (red), a suspect act
//!   `Partial` (yellow) or lower, anything else passes at `Full` (green).
//!
//! The one `[A]` beyond the client's own rules is
//! [`Target::in_engagement`]: assisting a pilot who is in any limited
//! engagement is a suspect act, TQ's rule ("This action would trigger a
//! Limited Engagement", 517301, is the refusal's text). Falsified by a
//! client that never gates assistance on an engagement.

/// What an act makes its doer. `Suspect < Criminal`: a criminal flag
/// outranks a suspect one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CrimeKind {
    Suspect,
    Criminal,
}

impl CrimeKind {
    /// `crimewatch/const.py`'s outcome numbers **[V]**:
    /// `crimewatchOutcomeSuspect` 1, `crimewatchOutcomeCriminal` 2.
    pub fn outcome(self) -> i64 {
        match self {
            CrimeKind::Suspect => 1,
            CrimeKind::Criminal => 2,
        }
    }

    /// The safety setting the act needs, at most.
    pub fn required_safety(self) -> Safety {
        match self {
            CrimeKind::Suspect => Safety::Partial,
            CrimeKind::Criminal => Safety::None,
        }
    }
}

/// The security band a rule reads: the client's security class with safe
/// sec folded into high (`is_high_sec_or_higher`) **[V]**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    /// `securityClassZeroSec`: null sec, wormholes, and every system this
    /// server declares `0.0`. No crimes at all (`is_low_sec_or_higher` is
    /// the guard on the whole tree).
    Zero,
    Low,
    High,
}

impl Space {
    pub fn of(class: crate::celestial::SecurityClass) -> Space {
        use crate::celestial::SecurityClass as C;
        match class {
            C::Zero => Space::Zero,
            C::Low => Space::Low,
            C::High | C::Safe => Space::High,
        }
    }
}

/// The ship safety setting, `crimewatch/const.py` **[V]**:
/// `shipSafetyLevelNone` 0 (red), `Partial` 1 (yellow), `Full` 2 (green).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Safety {
    None = 0,
    Partial = 1,
    Full = 2,
}

impl Safety {
    /// The number the client sends and is sent.
    pub fn wire(self) -> i64 {
        self as i64
    }

    /// A number from the client; anything outside `0..=2` is `None`.
    pub fn from_wire(v: i64) -> Option<Safety> {
        match v {
            0 => Some(Safety::None),
            1 => Some(Safety::Partial),
            2 => Some(Safety::Full),
            _ => Option::None,
        }
    }

    /// `not CheckUnsafe(required)`: the act is allowed when its required
    /// level is not below the setting (`crimewatchSvc.CheckUnsafe`) **[V]**.
    pub fn allows(self, crime: Option<CrimeKind>) -> bool {
        crime.is_none_or(|c| c.required_safety() >= self)
    }
}

/// What the rules know about the other side of an act.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Target {
    /// Owned by a capsuleer: a pilot's ship, capsule or drone. An NPC, a
    /// rock or a structure owned by nobody is not (and attacking one is no
    /// crime here; CONCORD's own ships and the gates are CONCORD-1's).
    pub capsuleer: bool,
    /// A capsule (`groupCapsule`): a criminal act to attack in low sec too.
    pub capsule: bool,
    /// The actor's own.
    pub own: bool,
    /// The owner's criminal flag.
    pub flag: Option<CrimeKind>,
    /// A limited engagement between the actor and the owner.
    pub engaged: bool,
    /// The owner is in **any** limited engagement (assistance only).
    pub in_engagement: bool,
    /// `IsOutlawStatus` of the owner's security status.
    pub outlaw: bool,
    /// The owner's militia is the actor's enemy (`IsCombatEnemyFaction`).
    pub fw_enemy: bool,
    /// The same player corporation, with friendly fire legal now.
    pub corp_friendly_fire: bool,
    /// At war (`warRelationshipAtWarCanFight`). `[A]` always `false` until
    /// wars exist (packet C8).
    pub at_war: bool,
}

impl Target {
    /// `crimewatchSvc.CanAttackFreely` for a capsuleer's item **[V]**.
    pub fn free(&self) -> bool {
        self.own
            || self.flag.is_some()
            || self.engaged
            || self.outlaw
            || self.fw_enemy
            || self.corp_friendly_fire
            || self.at_war
    }
}

/// One act the rules judge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// An offensive module, a drone's attack, a volley.
    Attack(Target),
    /// A remote repair or any other `isAssistance` effect.
    Assist(Target),
    /// A take from a container the taker has no loot rights to; the target
    /// is the container's owner.
    Theft(Target),
}

/// **The pure verdict**: what `act` in `space` makes its doer, `None` for
/// nothing. The seam CONCORD-1 and SEC-1 read through
/// [`crate::crimewatch::CrimeEvent`].
pub fn crime(space: Space, act: &Act) -> Option<CrimeKind> {
    if space == Space::Zero {
        return None;
    }
    match act {
        Act::Attack(t) => {
            if !t.capsuleer || t.free() {
                return None;
            }
            match space {
                Space::High => Some(CrimeKind::Criminal),
                _ if t.capsule => Some(CrimeKind::Criminal),
                _ => Some(CrimeKind::Suspect),
            }
        }
        Act::Assist(t) => {
            if !t.capsuleer || t.own {
                return None;
            }
            match t.flag {
                Some(k) => Some(k),
                None if t.in_engagement => Some(CrimeKind::Suspect),
                None => None,
            }
        }
        Act::Theft(owner) => (!owner.free()).then_some(CrimeKind::Suspect),
    }
}

/// Does an attack open (or keep open) a **limited engagement** between the
/// attacker and the target's owner?
///
/// `[A]`, TQ's rule: attacking a criminally flagged capsuleer engages the
/// two, so the flagged one may shoot back without a new crime (the
/// engagement is in its `CanAttackFreely`); and any aggression between two
/// pilots already engaged keeps their engagement going. Never below low
/// sec, where there are no flags to answer. Falsified by a client whose
/// flagged pilot is refused (or flagged) for returning fire.
pub fn engages(space: Space, t: &Target) -> bool {
    space != Space::Zero && t.capsuleer && !t.own && (t.engaged || t.flag.is_some())
}

/// `round(securityStatus, 1) <= outlawSecurityStatus` (`eveCfg.IsOutlawStatus`,
/// `appConst.outlawSecurityStatus = -5`) **[V]**.
pub fn is_outlaw(security_status: f64) -> bool {
    (security_status * 10.0).round() / 10.0 <= -5.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pilot() -> Target {
        Target {
            capsuleer: true,
            ..Target::default()
        }
    }

    /// **Attacking a plain pilot** is a criminal act in high sec, a suspect
    /// act in low sec and nothing in zero sec; a capsule is criminal in low
    /// sec too (`GetSafetyLevelRestrictionForAttackingTarget`,
    /// `DoesConcordReact`) [V].
    #[test]
    fn an_attack_on_a_plain_pilot_by_band() {
        let ship = Act::Attack(pilot());
        let pod = Act::Attack(Target {
            capsule: true,
            ..pilot()
        });
        assert_eq!(crime(Space::High, &ship), Some(CrimeKind::Criminal));
        assert_eq!(crime(Space::High, &pod), Some(CrimeKind::Criminal));
        assert_eq!(crime(Space::Low, &ship), Some(CrimeKind::Suspect));
        assert_eq!(crime(Space::Low, &pod), Some(CrimeKind::Criminal));
        assert_eq!(crime(Space::Zero, &ship), None);
        assert_eq!(crime(Space::Zero, &pod), None);
    }

    /// **Every `CanAttackFreely` reason** clears an attack in every band [V]:
    /// one's own, a flag, an engagement, an outlaw, an enemy militia, a
    /// corp mate under friendly fire, a war.
    #[test]
    fn a_free_target_is_no_crime() {
        let reasons = [
            Target {
                own: true,
                ..pilot()
            },
            Target {
                flag: Some(CrimeKind::Suspect),
                ..pilot()
            },
            Target {
                flag: Some(CrimeKind::Criminal),
                ..pilot()
            },
            Target {
                engaged: true,
                ..pilot()
            },
            Target {
                outlaw: true,
                ..pilot()
            },
            Target {
                fw_enemy: true,
                ..pilot()
            },
            Target {
                corp_friendly_fire: true,
                ..pilot()
            },
            Target {
                at_war: true,
                ..pilot()
            },
        ];
        for t in reasons {
            for space in [Space::High, Space::Low] {
                assert_eq!(crime(space, &Act::Attack(t)), None, "{t:?} in {space:?}");
                assert_eq!(
                    crime(space, &Act::Attack(Target { capsule: true, ..t })),
                    None,
                    "the pod of {t:?}"
                );
            }
        }
    }

    /// An NPC, or anything nobody flies, is no crime to shoot.
    #[test]
    fn a_non_capsuleer_target_is_no_crime() {
        for space in [Space::High, Space::Low, Space::Zero] {
            assert_eq!(crime(space, &Act::Attack(Target::default())), None);
        }
    }

    /// **Assisting** inherits the target's flag: a criminal makes a
    /// criminal, a suspect a suspect, in low and high sec alike
    /// (`GetSafetyLevelRestrictionForAidingTarget`) [V]; an engaged pilot a
    /// suspect [A]; a clean pilot, one's own ship or zero sec nothing.
    #[test]
    fn assistance_inherits_the_flag() {
        let crim = Target {
            flag: Some(CrimeKind::Criminal),
            ..pilot()
        };
        let susp = Target {
            flag: Some(CrimeKind::Suspect),
            ..pilot()
        };
        let engaged = Target {
            in_engagement: true,
            ..pilot()
        };
        for space in [Space::High, Space::Low] {
            assert_eq!(crime(space, &Act::Assist(crim)), Some(CrimeKind::Criminal));
            assert_eq!(crime(space, &Act::Assist(susp)), Some(CrimeKind::Suspect));
            assert_eq!(
                crime(space, &Act::Assist(engaged)),
                Some(CrimeKind::Suspect)
            );
            assert_eq!(crime(space, &Act::Assist(pilot())), None);
            assert_eq!(
                crime(space, &Act::Assist(Target { own: true, ..crim })),
                None
            );
        }
        assert_eq!(crime(Space::Zero, &Act::Assist(crim)), None);
    }

    /// **Theft** is a suspect act in low and high sec, nothing in zero sec,
    /// and nothing from an owner one may attack freely (`HaveLootRight`
    /// asks `CanAttackFreely`) [V].
    #[test]
    fn theft_is_a_suspect_act_unless_the_owner_is_free() {
        assert_eq!(
            crime(Space::High, &Act::Theft(pilot())),
            Some(CrimeKind::Suspect)
        );
        assert_eq!(
            crime(Space::Low, &Act::Theft(pilot())),
            Some(CrimeKind::Suspect)
        );
        assert_eq!(crime(Space::Zero, &Act::Theft(pilot())), None);
        let flagged = Target {
            flag: Some(CrimeKind::Suspect),
            ..pilot()
        };
        assert_eq!(crime(Space::High, &Act::Theft(flagged)), None);
        let engaged = Target {
            engaged: true,
            ..pilot()
        };
        assert_eq!(crime(Space::Low, &Act::Theft(engaged)), None);
    }

    /// **The safety table** (`CheckUnsafe`: unsafe when `required < safety`)
    /// [V]: green allows no crime, yellow allows a suspect act, red allows
    /// both; nothing is ever refused for an act that is no crime.
    #[test]
    fn the_safety_table() {
        use CrimeKind::*;
        let cases = [
            (Safety::Full, None, true),
            (Safety::Full, Some(Suspect), false),
            (Safety::Full, Some(Criminal), false),
            (Safety::Partial, None, true),
            (Safety::Partial, Some(Suspect), true),
            (Safety::Partial, Some(Criminal), false),
            (Safety::None, None, true),
            (Safety::None, Some(Suspect), true),
            (Safety::None, Some(Criminal), true),
        ];
        for (safety, crime, allowed) in cases {
            assert_eq!(safety.allows(crime), allowed, "{safety:?} {crime:?}");
        }
        for v in 0..=2 {
            assert_eq!(Safety::from_wire(v).map(Safety::wire), Some(v));
        }
        assert_eq!(Safety::from_wire(3), Option::None);
        assert_eq!(Safety::from_wire(-1), Option::None);
    }

    /// An engagement opens on an attack at a flagged pilot and keeps going
    /// between two engaged pilots, never in zero sec or at an NPC [A].
    #[test]
    fn engagements_open_on_flagged_targets_only() {
        let flagged = Target {
            flag: Some(CrimeKind::Suspect),
            ..pilot()
        };
        let engaged = Target {
            engaged: true,
            ..pilot()
        };
        assert!(engages(Space::High, &flagged));
        assert!(engages(Space::Low, &engaged));
        assert!(!engages(Space::Zero, &flagged));
        assert!(!engages(Space::High, &pilot()));
        assert!(!engages(Space::High, &Target::default()));
        assert!(!engages(
            Space::High,
            &Target {
                own: true,
                ..flagged
            }
        ));
    }

    /// The outlaw line is the client's rounding: -4.95 rounds to -5.0 and
    /// is an outlaw, -4.94 is not [V].
    #[test]
    fn the_outlaw_line_rounds_to_one_decimal() {
        assert!(is_outlaw(-5.0));
        assert!(is_outlaw(-4.95));
        assert!(is_outlaw(-10.0));
        assert!(!is_outlaw(-4.94));
        assert!(!is_outlaw(0.0));
    }
}
