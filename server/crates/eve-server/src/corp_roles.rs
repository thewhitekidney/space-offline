//! Corp roles and CEO succession, the pure half (SOC-C3; R-SOCIETY §3.1,
//! §3.2 "CEO succession"; lane note `docs/lanes/soc-c3.md`).
//!
//! Three questions, each answered without the registry so it can be tested
//! with invented ids:
//!
//! - **May this member make this role edit?** ([`check_edit`]) The client's
//!   roles window (`corp_ui_member_newroles`), the edit-member dialog
//!   (`corp_dlg_edit_member`) and the task-management tab
//!   (`corp_ui_member_view_task_mgt`) all reach the server with the eight
//!   role masks a member row carries. The rules mirror what the client
//!   itself lets a pilot grant (`bco_members.GetMyGrantableRoles`) **[V]**.
//! - **Who may take over as CEO?** ([`potential_ceos`]) The answer the
//!   client counts (`GetNumberOfPotentialCEOs`, `len(...)`) and lists
//!   (`ResignFromCEO(None)`'s result, iterated as character ids) **[V]**.
//! - **What does `ResignFromCEO(newCeoID)` do?** ([`resign`]) Hand over,
//!   list the candidates, or close the corp.
//!
//! The wire, the state and the pushes are `service/corp.rs`.

/// `appConst.corpRoleDirector = 1` **[V]**.
pub const CORP_ROLE_DIRECTOR: i64 = 1;

/// The eight role masks of one member, in the client's column order
/// (`corp_ui_member_newroles.GetRowHeader`, after `characterID, title,
/// divisionID, squadronID`) **[V]**.
pub const MASK_NAMES: [&str; 8] = [
    "roles",
    "grantableRoles",
    "rolesAtHQ",
    "grantableRolesAtHQ",
    "rolesAtBase",
    "grantableRolesAtBase",
    "rolesAtOther",
    "grantableRolesAtOther",
];

/// Index of `roles` in [`MASK_NAMES`] and in a [`Masks`] array.
pub const ROLES: usize = 0;
/// Index of `grantableRoles`.
pub const GRANTABLE_ROLES: usize = 1;

/// The eight masks, indexed as [`MASK_NAMES`]. Even indices are held roles,
/// odd ones the matching grantable roles.
pub type Masks = [i64; 8];

/// The index of a mask column by its client name.
pub fn mask_index(name: &str) -> Option<usize> {
    MASK_NAMES.iter().position(|n| *n == name)
}

/// What the actor is to the corp, for [`check_edit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    /// The CEO holding director: `UserIsActiveCEO` **[V]** (`corprole &
    /// corpRoleDirector` and `ceoID == charid`). May grant every role,
    /// director included.
    ActiveCeo,
    /// Holds `corpRoleDirector`. May grant every role but director
    /// (`GetMyGrantableRoles` leaves the director bit to the active CEO)
    /// **[V]**.
    Director,
    /// Anybody else: may grant what their own grantable masks hold
    /// (`GetMyGrantableRoles`' last branch) **[V]**. The four grantable
    /// masks, in the order roles, HQ, base, other.
    Member([i64; 4]),
}

/// One requested change to a member, every field `None` for "leave it".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Edit {
    pub masks: [Option<i64>; 8],
    pub title_mask: Option<i64>,
    /// The free-text title (`title`), sent by the edit dialog only for a CEO
    /// or a director (`isCEOorEq`) **[V]**.
    pub title: Option<String>,
    /// `blockRoles`: "Remove all roles" sends `1` for the caller
    /// (`base_corporation.RemoveAllRoles`) **[V]**.
    pub block_roles: Option<bool>,
}

impl Edit {
    /// The masks after the edit.
    pub fn apply_masks(&self, old: &Masks) -> Masks {
        let mut out = *old;
        for (i, v) in self.masks.iter().enumerate() {
            if let Some(v) = v {
                out[i] = *v;
            }
        }
        out
    }
}

/// Why an edit was refused. `service/corp.rs` turns each into the client's
/// own message key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// Somebody else's `blockRoles`: only the member sets their own **[A]**.
    NotYourChoice,
    /// The CEO's roles are the CEO's: the edit dialog returns early for
    /// `playerIsCEO and not userIsCEO` **[V]**.
    CeoUntouchable,
    /// The member blocks roles and the edit adds some: `CrpRolesDenied`,
    /// "{member} is not willing to accept new roles" **[V]** (the key and
    /// text); **[I]** that this is where it is raised.
    BlocksRoles,
    /// The director bit moves only for the active CEO **[V]**.
    DirectorOnlyByCeo,
    /// A bit outside the actor's grantable masks, a grantable mask or a
    /// title changed by a plain member **[A]**.
    NotGrantable,
}

/// May `actor` make `edit` to `target`?
///
/// - A member may always **remove** their own roles and set their own
///   `blockRoles` (`RemoveAllRoles` zeroes every mask and sets the flag)
///   **[V]** for the call, **[A]** that the server allows it to anybody.
/// - Nobody but the CEO edits the CEO.
/// - Nobody adds roles to a member who blocks them.
/// - The active CEO may change everything; a director everything but the
///   director bit; anybody else only bits inside their grantable masks,
///   and never grantable masks or titles **[A]**.
pub fn check_edit(
    actor: i64,
    authority: Authority,
    target: i64,
    target_is_ceo: bool,
    target_blocks: bool,
    old: &Masks,
    edit: &Edit,
) -> Result<(), Denied> {
    let new = edit.apply_masks(old);
    let adds = (0..8).any(|i| new[i] & !old[i] != 0);
    let titles_change = edit.title.is_some() || edit.title_mask.is_some();
    if new == *old && !titles_change && edit.block_roles.is_none() {
        // Nothing moves: a task-management action whose targets already
        // hold (or lack) the role, the CEO among them.
        return Ok(());
    }
    if edit.block_roles.is_some() && actor != target {
        return Err(Denied::NotYourChoice);
    }
    if actor == target && !adds && !titles_change {
        return Ok(());
    }
    if target_is_ceo && actor != target {
        return Err(Denied::CeoUntouchable);
    }
    if target_blocks && adds && actor != target {
        return Err(Denied::BlocksRoles);
    }
    let director_moves = (old[ROLES] ^ new[ROLES]) & CORP_ROLE_DIRECTOR != 0
        || (old[GRANTABLE_ROLES] ^ new[GRANTABLE_ROLES]) & CORP_ROLE_DIRECTOR != 0;
    match authority {
        Authority::ActiveCeo => Ok(()),
        Authority::Director if director_moves => Err(Denied::DirectorOnlyByCeo),
        Authority::Director => Ok(()),
        Authority::Member(grantable) => {
            if titles_change {
                return Err(Denied::NotGrantable);
            }
            for (slot, may) in grantable.iter().enumerate() {
                let (held, grant) = (2 * slot, 2 * slot + 1);
                if old[grant] != new[grant] {
                    return Err(Denied::NotGrantable);
                }
                if (old[held] ^ new[held]) & !may != 0 {
                    return Err(Denied::NotGrantable);
                }
            }
            Ok(())
        }
    }
}

/// One member as [`potential_ceos`] and [`resign`] see them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub character_id: i64,
    pub director: bool,
    pub blocks_roles: bool,
    /// Blue FILETIME the member joined: the tenure.
    pub start_date: i64,
}

/// The members who may take over from `ceo`, best first.
///
/// **[I]** on who: every other member who does not block roles. The
/// client's own words for "nobody qualifies" are "None of the remaining
/// members of your corporation are willing to take over as CEO"
/// (`CrpDestroyNonEmptyCorpWarning`) **[V]**, and "willing" is the
/// language of `blockRoles` ("{member} is not willing to accept new roles",
/// `CrpRolesDenied`) **[V]**. Falsified by Tranquility also requiring a
/// skill (Corporation Management) of a successor.
///
/// **[A]** on the order: directors first, then the longest-serving, then
/// the lowest id. It is the order the client's picker lists them in, and
/// the bots' succession rule (R-SOCIETY §3.2: "the longest-tenured
/// director takes over") is the first entry.
pub fn potential_ceos(ceo: i64, members: &[Candidate]) -> Vec<i64> {
    let mut out: Vec<&Candidate> = members
        .iter()
        .filter(|m| m.character_id != ceo && !m.blocks_roles)
        .collect();
    out.sort_by_key(|m| (!m.director, m.start_date, m.character_id));
    out.into_iter().map(|m| m.character_id).collect()
}

/// What `ResignFromCEO(newCeoID)` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resign {
    /// `newCeoID` was `None` and somebody can take over: the answer is
    /// their ids, which the client lists for the CEO to pick from, then
    /// calls again with the pick (`base_corporation.ResignFromCEO`) **[V]**.
    Candidates(Vec<i64>),
    /// Hand the seat to this member.
    HandOver(i64),
    /// Close the corp: the CEO was alone, or nobody is willing.
    Close,
}

/// Why a resignation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResignRefused {
    /// `CanNotDestroyCorpInAlliance` **[V]**: the client raises it itself
    /// for a lone CEO in an alliance (`__ResignFromCEO`). **[A]** that the
    /// server raises it for every close of a corp in an alliance, the
    /// "nobody willing" one included: the client does not guard that
    /// branch.
    InAlliance,
    /// The pick is not a member of the corp, or is the CEO.
    NotAMember,
    /// The pick blocks roles.
    Unwilling,
}

/// The rule behind `ResignFromCEO(newCeoID)`.
pub fn resign(
    ceo: i64,
    pick: Option<i64>,
    members: &[Candidate],
    in_alliance: bool,
) -> Result<Resign, ResignRefused> {
    match pick {
        Some(id) => {
            let Some(m) = members.iter().find(|m| m.character_id == id && id != ceo) else {
                return Err(ResignRefused::NotAMember);
            };
            if m.blocks_roles {
                return Err(ResignRefused::Unwilling);
            }
            Ok(Resign::HandOver(id))
        }
        None => {
            let candidates = potential_ceos(ceo, members);
            if !candidates.is_empty() {
                Ok(Resign::Candidates(candidates))
            } else if in_alliance {
                Err(ResignRefused::InAlliance)
            } else {
                Ok(Resign::Close)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CEO: i64 = 90_000_501;
    const DIR: i64 = 90_000_502;
    const PEON: i64 = 90_000_503;
    /// An invented non-director role bit.
    const SOME_ROLE: i64 = 1 << 7;
    const OTHER_ROLE: i64 = 1 << 8;

    fn set(i: usize, v: i64) -> Edit {
        let mut e = Edit::default();
        e.masks[i] = Some(v);
        e
    }

    fn member(id: i64, director: bool, blocks: bool, start: i64) -> Candidate {
        Candidate {
            character_id: id,
            director,
            blocks_roles: blocks,
            start_date: start,
        }
    }

    #[test]
    fn mask_names_are_the_clients_row_columns_in_order() {
        assert_eq!(mask_index("roles"), Some(ROLES));
        assert_eq!(mask_index("grantableRoles"), Some(GRANTABLE_ROLES));
        assert_eq!(mask_index("grantableRolesAtOther"), Some(7));
        assert_eq!(mask_index("titleMask"), None);
    }

    #[test]
    fn the_active_ceo_grants_anything_including_director() {
        let old = [0; 8];
        let e = set(ROLES, CORP_ROLE_DIRECTOR | SOME_ROLE);
        assert_eq!(
            check_edit(CEO, Authority::ActiveCeo, PEON, false, false, &old, &e),
            Ok(())
        );
    }

    #[test]
    fn a_director_grants_everything_but_director() {
        let old = [0; 8];
        assert_eq!(
            check_edit(
                DIR,
                Authority::Director,
                PEON,
                false,
                false,
                &old,
                &set(ROLES, SOME_ROLE)
            ),
            Ok(())
        );
        assert_eq!(
            check_edit(
                DIR,
                Authority::Director,
                PEON,
                false,
                false,
                &old,
                &set(ROLES, CORP_ROLE_DIRECTOR)
            ),
            Err(Denied::DirectorOnlyByCeo)
        );
        // Taking director away is the CEO's too.
        let mut held = [0; 8];
        held[ROLES] = CORP_ROLE_DIRECTOR;
        assert_eq!(
            check_edit(
                DIR,
                Authority::Director,
                PEON,
                false,
                false,
                &held,
                &set(ROLES, 0)
            ),
            Err(Denied::DirectorOnlyByCeo)
        );
    }

    #[test]
    fn nobody_but_the_ceo_edits_the_ceo() {
        let old = [0; 8];
        assert_eq!(
            check_edit(
                DIR,
                Authority::Director,
                CEO,
                true,
                false,
                &old,
                &set(ROLES, SOME_ROLE)
            ),
            Err(Denied::CeoUntouchable)
        );
        assert_eq!(
            check_edit(
                CEO,
                Authority::ActiveCeo,
                CEO,
                true,
                false,
                &old,
                &set(ROLES, SOME_ROLE)
            ),
            Ok(())
        );
    }

    #[test]
    fn a_member_who_blocks_roles_gets_none_but_may_lose_them() {
        let mut old = [0; 8];
        old[ROLES] = SOME_ROLE;
        assert_eq!(
            check_edit(
                CEO,
                Authority::ActiveCeo,
                PEON,
                false,
                true,
                &old,
                &set(ROLES, SOME_ROLE | OTHER_ROLE)
            ),
            Err(Denied::BlocksRoles)
        );
        assert_eq!(
            check_edit(
                CEO,
                Authority::ActiveCeo,
                PEON,
                false,
                true,
                &old,
                &set(ROLES, 0)
            ),
            Ok(())
        );
    }

    #[test]
    fn anyone_may_drop_their_own_roles_and_block_new_ones() {
        let mut old = [0; 8];
        old[ROLES] = SOME_ROLE;
        old[GRANTABLE_ROLES] = SOME_ROLE;
        let mut e = Edit {
            masks: [Some(0); 8],
            block_roles: Some(true),
            ..Edit::default()
        };
        assert_eq!(
            check_edit(
                PEON,
                Authority::Member([0; 4]),
                PEON,
                false,
                false,
                &old,
                &e
            ),
            Ok(())
        );
        // ...but not give themselves one.
        e.masks[ROLES] = Some(SOME_ROLE | OTHER_ROLE);
        assert_eq!(
            check_edit(
                PEON,
                Authority::Member([0; 4]),
                PEON,
                false,
                false,
                &old,
                &e
            ),
            Err(Denied::NotGrantable)
        );
        // Nobody blocks roles for somebody else.
        let e = Edit {
            block_roles: Some(true),
            ..Edit::default()
        };
        assert_eq!(
            check_edit(CEO, Authority::ActiveCeo, PEON, false, false, &old, &e),
            Err(Denied::NotYourChoice)
        );
    }

    #[test]
    fn a_plain_member_grants_only_inside_their_grantable_masks() {
        let old = [0; 8];
        let grants = Authority::Member([SOME_ROLE, 0, 0, 0]);
        assert_eq!(
            check_edit(
                PEON,
                grants,
                DIR,
                false,
                false,
                &old,
                &set(ROLES, SOME_ROLE)
            ),
            Ok(())
        );
        assert_eq!(
            check_edit(
                PEON,
                grants,
                DIR,
                false,
                false,
                &old,
                &set(ROLES, OTHER_ROLE)
            ),
            Err(Denied::NotGrantable)
        );
        // At HQ needs the HQ grant.
        assert_eq!(
            check_edit(PEON, grants, DIR, false, false, &old, &set(2, SOME_ROLE)),
            Err(Denied::NotGrantable)
        );
        assert_eq!(
            check_edit(
                PEON,
                grants,
                DIR,
                false,
                false,
                &old,
                &set(GRANTABLE_ROLES, SOME_ROLE)
            ),
            Err(Denied::NotGrantable)
        );
        let titled = Edit {
            title: Some("Boss".to_owned()),
            ..Edit::default()
        };
        assert_eq!(
            check_edit(PEON, grants, DIR, false, false, &old, &titled),
            Err(Denied::NotGrantable)
        );
    }

    #[test]
    fn potential_ceos_are_the_willing_others_directors_and_veterans_first() {
        let members = [
            member(CEO, true, false, 1),
            member(PEON, false, false, 2),
            member(DIR, true, false, 9),
            member(90_000_504, false, true, 0),
            member(90_000_505, false, false, 2),
        ];
        assert_eq!(potential_ceos(CEO, &members), vec![DIR, PEON, 90_000_505]);
    }

    #[test]
    fn resigning_lists_hands_over_or_closes() {
        let alone = [member(CEO, true, false, 1)];
        assert_eq!(resign(CEO, None, &alone, false), Ok(Resign::Close));
        assert_eq!(
            resign(CEO, None, &alone, true),
            Err(ResignRefused::InAlliance)
        );

        let two = [member(CEO, true, false, 1), member(PEON, false, false, 2)];
        assert_eq!(
            resign(CEO, None, &two, true),
            Ok(Resign::Candidates(vec![PEON]))
        );
        assert_eq!(
            resign(CEO, Some(PEON), &two, true),
            Ok(Resign::HandOver(PEON))
        );
        assert_eq!(
            resign(CEO, Some(CEO), &two, false),
            Err(ResignRefused::NotAMember)
        );
        assert_eq!(
            resign(CEO, Some(90_000_999), &two, false),
            Err(ResignRefused::NotAMember)
        );

        // Nobody willing: the corp closes, members and all.
        let unwilling = [member(CEO, true, false, 1), member(PEON, false, true, 2)];
        assert_eq!(resign(CEO, None, &unwilling, false), Ok(Resign::Close));
        assert_eq!(
            resign(CEO, Some(PEON), &unwilling, false),
            Err(ResignRefused::Unwilling)
        );
    }
}
