//! **What a bot knows about an NPC type** (CP46, C6 §5.2 and packet N7).
//!
//! The bot is a player, and a player's client holds the same static data, so
//! a bot may read the content bundle's `npc/types.tsv` and `typedata.tsv`
//! without cheating [I: everything in them is in the client]. This module
//! turns them into one small [`NpcProfile`] per NPC type: its C6 R8 `role`
//! and `class` (the two columns `schema.md` lists CP46 as reading), its hit
//! points and its damage per second. [`crate::escort`] reads it for target
//! priority and for retreat.
//!
//! **Pure, and given, not global.** [`Profiles`] is built once at startup
//! (`eve-bots --content DIR`, [`Profiles::open`]) and handed to every bot in
//! its [`crate::brain::Policy`] behind an `Arc`, so the brain stays a pure
//! function of its memory and observation, and nothing here is a process
//! static. A bot with no bundle has no profiles and plays as before.
//!
//! **What the numbers mean** is **[I]**, as C6 marks it: every attribute id
//! is the one the server's own `npc::derive_as` reads for the same type
//! (hull 263 + 265 + 9; gun 114/116/117/118 times 64 every 51 ms) or
//! `npc_data::Table::profile` reads (missile 507/212/506), so a bot's
//! estimate is the server's model of the rat, not a second one. What the
//! escort does with them is **[A]** (`escort.rs`).

use eve_server::content::{Bundle, Content, TYPEDATA};
use eve_server::npc_data::{self, Role};
use eve_server::typedata;
use std::collections::BTreeMap;
use std::path::Path;

/// C6 R2's `class` of a hauler rat (`npc/types.tsv`; C6 §2.1's class list)
/// **[V-data]**: the escort leaves one for last (C6 §5.2).
pub const HAULER: &str = "Hauler";

/// One NPC type, as a bot sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct NpcProfile {
    /// C6 R8's role, CCP's vocabulary (`npc/types.tsv`).
    pub role: Role,
    /// C6 R2's class, verbatim (`Frigate`, `Hauler`, ...).
    pub class: String,
    /// Shield capacity plus armour plus hull, raw: no resists. **[I]**; `0`
    /// with no typedata.
    pub hp: f64,
    /// Turret plus missile damage per second, raw: every shot lands, no
    /// resists. **[I]**; `0` with no typedata.
    pub dps: f64,
}

impl NpcProfile {
    /// A scrambler: the fleet cannot leave while it lives.
    pub fn is_tackler(&self) -> bool {
        self.role == Role::Tackler
    }

    /// A hauler rat: no guns worth the time while anything else is left.
    pub fn is_hauler(&self) -> bool {
        self.class == HAULER
    }
}

/// Every NPC type a bot can name, by `type_id`.
#[derive(Clone, Default, PartialEq)]
pub struct Profiles {
    types: BTreeMap<i64, NpcProfile>,
}

/// A count, never the rows: a policy is printed whole in a few logs.
impl std::fmt::Debug for Profiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Profiles({} types)", self.types.len())
    }
}

impl FromIterator<(i64, NpcProfile)> for Profiles {
    fn from_iter<I: IntoIterator<Item = (i64, NpcProfile)>>(rows: I) -> Profiles {
        Profiles {
            types: rows.into_iter().collect(),
        }
    }
}

impl Profiles {
    /// The type's profile, if the bundle lists it.
    pub fn get(&self, type_id: i64) -> Option<&NpcProfile> {
        self.types.get(&type_id)
    }

    pub fn len(&self) -> usize {
        self.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    /// One profile per `npc/types.tsv` row; hit points and damage from
    /// `td` when given (else `0`). Pure.
    pub fn build(npc: &npc_data::Table, td: Option<&typedata::Table>) -> Profiles {
        npc.types
            .iter()
            .map(|(&id, row)| {
                let (hp, dps) = td
                    .filter(|t| t.get(id).is_some())
                    .map_or((0.0, 0.0), |t| (hp(t, id), dps(npc, t, id)));
                (
                    id,
                    NpcProfile {
                        role: row.role,
                        class: row.class.clone(),
                        hp,
                        dps,
                    },
                )
            })
            .collect()
    }

    /// `eve-bots --content DIR`: the bundle checked as the server checks it
    /// ([`Bundle::open`]), then its `npc/` rows and its typedata (optional:
    /// without it every number is `0`). Errors name the directory.
    pub fn open(dir: &Path) -> Result<Profiles, String> {
        let (bundle, _) = Bundle::open(dir)?;
        let at = |e: String| format!("--content {}: {e}", dir.display());
        let npc = npc_data::Table::from_bundle(&bundle).map_err(at)?;
        let td = match bundle.path(TYPEDATA) {
            Some(p) => Some(typedata::Table::load(&p).map_err(|e| at(format!("{TYPEDATA}: {e}")))?),
            None => None,
        };
        Ok(Profiles::build(&npc, td.as_ref()))
    }

    /// The same rows from a [`Content`] already loaded
    /// ([`crate::hulls::open`], FX10), so `eve-bots` reads the bundle once.
    /// A bundle without typedata has an empty table here: every number `0`,
    /// as [`Self::open`] gives.
    pub fn from_content(content: &Content) -> Profiles {
        Profiles::build(
            content.get::<npc_data::Table>(),
            Some(typedata::installed_in(content).table()),
        )
    }
}

/// Shield, armour and hull: `npc::derive_as`'s 263, 265 and 9.
fn hp(td: &typedata::Table, id: i64) -> f64 {
    use typedata::attr as t;
    [t::SHIELD_CAPACITY, t::ARMOR_HP, t::HP]
        .iter()
        .map(|&a| td.attribute(id, a))
        .sum()
}

/// The four damage attributes of `id`, summed.
fn volley(td: &typedata::Table, id: i64) -> f64 {
    npc_data::dogma::DAMAGE
        .iter()
        .map(|&a| td.attribute(id, a))
        .sum()
}

/// `amount` every `period_ms`, per second; `0` for a period that is not
/// positive.
fn per_second(amount: f64, period_ms: f64) -> f64 {
    if period_ms > 0.0 {
        amount * 1_000.0 / period_ms
    } else {
        0.0
    }
}

/// The gun (`npc::derive_as`: the hull's damage times 64, every 51 ms) plus
/// the launcher (`npc_data::Table::profile`: the missile type's damage times
/// 212, every 506 ms).
fn dps(npc: &npc_data::Table, td: &typedata::Table, id: i64) -> f64 {
    let gun = per_second(
        volley(td, id) * td.attribute(id, typedata::attr::DAMAGE_MULTIPLIER),
        td.attribute(id, eve_server::npc::attr::SPEED),
    );
    let missile = npc
        .profile(td, id)
        .and_then(|p| p.missile)
        .map_or(0.0, |m| {
            per_second(volley(td, m.type_id) * m.multiplier, m.period_ms)
        });
    gun + missile
}

#[cfg(test)]
mod tests {
    //! Invented rows only: `npc_data::Table::fixture()`'s types and a
    //! typedata written here. Expected numbers come from the inputs.
    use super::*;

    /// The fixture's DPS frigate and its TACKLER (`npc_data::Table::fixture`).
    const DPS_TYPE: i64 = 990_300_001;
    const TACKLER_TYPE: i64 = 990_300_002;
    /// Its missile cruiser, and an invented missile.
    const MISSILE_TYPE: i64 = 990_300_003;
    const MISSILE: i64 = 990_399_001;

    fn typedata() -> typedata::Table {
        let line = |id: i64, attrs: &str| {
            format!("type\t{id}\t99111\t11\t0\t1\t1.0\t0.0\t1000.0\t20.0\t0\t\t{attrs}\tRat {id}\n")
        };
        let text = [
            line(
                DPS_TYPE,
                "9=30.0;51=2000.0;64=2.0;114=5.0;117=5.0;263=50.0;265=20.0",
            ),
            line(TACKLER_TYPE, "9=10.0;263=5.0;265=5.0"),
            line(MISSILE_TYPE, "506=4000.0;507=990399001.0;212=3.0"),
            line(MISSILE, "116=10.0;118=10.0"),
        ]
        .concat();
        typedata::Table::parse(&text).expect("the invented typedata parses")
    }

    #[test]
    fn a_profile_reads_role_class_hp_and_dps_from_the_rows() {
        let p = Profiles::build(&npc_data::Table::fixture(), Some(&typedata()));
        let dps = p.get(DPS_TYPE).expect("listed");
        assert!(!dps.is_tackler() && !dps.is_hauler());
        assert_eq!(dps.class, "Frigate");
        assert_eq!(dps.hp, 50.0 + 20.0 + 30.0);
        assert_eq!(dps.dps, (5.0 + 5.0) * 2.0 * 1_000.0 / 2_000.0);
        assert!(p.get(TACKLER_TYPE).expect("listed").is_tackler());
        assert_eq!(
            p.get(MISSILE_TYPE).expect("listed").dps,
            (10.0 + 10.0) * 3.0 * 1_000.0 / 4_000.0
        );
        assert_eq!(p.get(MISSILE), None, "a missile is not an NPC type");
    }

    /// FX10: built from a `Content`, the profiles are the fixture tables'
    /// own -- the same rows [`Profiles::build`] reads.
    #[test]
    fn from_content_reads_the_contents_npc_and_type_tables() {
        let c = Content::fixture();
        let p = Profiles::from_content(&c);
        assert_eq!(
            p,
            Profiles::build(
                c.get::<npc_data::Table>(),
                Some(typedata::installed_in(&c).table())
            )
        );
        assert_eq!(p.len(), npc_data::Table::fixture().types.len());
    }

    #[test]
    fn without_typedata_the_roles_load_and_every_number_is_zero() {
        let p = Profiles::build(&npc_data::Table::fixture(), None);
        assert_eq!(p.len(), npc_data::Table::fixture().types.len());
        let t = p.get(TACKLER_TYPE).expect("listed");
        assert!(t.is_tackler());
        assert_eq!((t.hp, t.dps), (0.0, 0.0));
        assert_eq!(format!("{p:?}"), format!("Profiles({} types)", p.len()));
    }
}
