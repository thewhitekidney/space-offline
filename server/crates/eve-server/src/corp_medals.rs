//! **Corporation medals** (lane `soc-medals`, #166; `r-corp-social` §6): a
//! corp's directors design a medal, award it to pilots, and a pilot decides
//! who sees it. The wire is `service/corp_medals.rs`; this is the store and
//! the rules, with no registry.
//!
//! Two delta kinds carry the state:
//!
//! | kind | key | value |
//! | --- | --- | --- |
//! | [`KIND`] | `m id` | `corp creator date title description` then `part graphic color` per graphic |
//! | [`KIND`] | `next` | `n`, the next medal id |
//! | [`AWARD_KIND`] | `medal char n` | `issuer date reason status` |
//!
//! An award is a row of its own: the same pilot may receive the same medal
//! again, and the client counts those (`GetMedalEntry` shows "awarded N
//! times"), so `n` is that pilot's 1-based count of that medal. `status` is
//! the client's own `appConst.medalStatus`: 1 deleted, 2 private, 3 public
//! **[V]**. `date` is a FILETIME.
//!
//! Medals are free: no ISK moves and the confirm dialogs
//! (`ConfirmCreatingMedal`, `ConfirmGivingMedal`) never fire. That is the
//! coordinator's default pending the owner (report §6 decision 4).
//!
//! **The trap** is `uix.FormatMedalData`: it does `data.Filter('part').get(p)`
//! for parts 1 (ribbon) and 2 (medal) and iterates the result, so a medal with
//! no rows for either part raises in the client. [`validate_graphics`] refuses
//! such a medal at creation. A graphic name needs exactly one dot because the
//! client does `label, icon = row.graphic.split('.')`, and the icon half must
//! be `digits_digits` with a non-zero first number because `MapLogo` does
//! `iconNum.split('_')` and `int()` on the parts **[V]**.

use crate::delta::Deltas;

/// The medals and their counter.
pub const KIND: &str = "corpmedal";
/// The awards.
pub const AWARD_KIND: &str = "medalgot";

/// Client limits (`appConst.medalMaxNameLength` and the description pair, the
/// create window's own bounds) **[V]**.
pub const MIN_TITLE: usize = 3;
pub const MAX_TITLE: usize = 100;
pub const MIN_DESCRIPTION: usize = 10;
pub const MAX_DESCRIPTION: usize = 1000;
/// The reason prompt's `maxLength` (`medals.GiveMedalToCharacters`) **[V]**.
pub const MAX_REASON: usize = 200;
/// Recipients in one call. Not the client's: a bound on one call's work.
pub const MAX_RECIPIENTS: usize = 100;
/// Graphics in one medal. Not the client's; the picker offers a handful.
pub const MAX_GRAPHICS: usize = 20;

/// `appConst.medalStatus` **[V]**.
pub mod status {
    pub const DELETED: i64 = 1;
    pub const PRIVATE: i64 = 2;
    pub const PUBLIC: i64 = 3;
}

/// The status a new award starts at **[A: private, as the character sheet's
/// Decorations tab lets a pilot make a medal public afterwards; falsifier: a
/// retail medal showing on another pilot's sheet before its owner chose]**.
pub const DEFAULT_STATUS: i64 = status::PRIVATE;

/// Graphic parts (`uix.FormatMedalData`): 1 ribbon, 2 medal **[V]**.
pub const PART_RIBBON: i64 = 1;
pub const PART_MEDAL: i64 = 2;

/// `appConst.corpRoleDirector` and `corpRolePersonnelManager`: the two roles
/// the client's own gate lets create and award (`medals.CreateMedal`)
/// **[V]**. The second is `service::corp::CORP_ROLE_PERSONNEL_MANAGER`, which
/// the wire module's tests pin this to.
pub const ROLE_DIRECTOR: i64 = 1;
pub const ROLE_PERSONNEL_MANAGER: i64 = 128;

/// Whether a member holding `roles` (or the CEO) may create and award.
pub fn may_manage(roles: i64, is_ceo: bool) -> bool {
    is_ceo || roles & (ROLE_DIRECTOR | ROLE_PERSONNEL_MANAGER) != 0
}

/// Why a call is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    TitleTooShort,
    TitleTooLong,
    DescriptionTooShort,
    DescriptionTooLong,
    /// The graphics list is missing a part, has a part that is neither 1
    /// nor 2, or a graphic name without exactly one dot.
    BadGraphics,
}

/// One row of a medal's picture: `part` 1 or 2, `graphic` as
/// `<label>.<digits>_<digits>`, and the colour, a packed ARGB value the client
/// reads as signed (-1 is skipped by the ribbon builder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Graphic {
    pub part: i64,
    pub graphic: String,
    pub color: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Medal {
    pub id: i64,
    pub corp: i64,
    pub creator: i64,
    pub date: i64,
    pub title: String,
    pub description: String,
    pub graphics: Vec<Graphic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Award {
    pub medal: i64,
    pub character: i64,
    /// This pilot's count of this medal, from 1.
    pub n: i64,
    pub issuer: i64,
    pub date: i64,
    pub reason: String,
    pub status: i64,
}

/// Cut `s` to at most `max` characters.
pub fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// The title the store keeps, or why it is refused. Surrounding blanks are
/// not part of a name.
pub fn validate_title(title: &str) -> Result<String, Refusal> {
    let t = title.trim();
    match t.chars().count() {
        n if n < MIN_TITLE => Err(Refusal::TitleTooShort),
        n if n > MAX_TITLE => Err(Refusal::TitleTooLong),
        _ => Ok(t.to_owned()),
    }
}

/// The description the store keeps, or why it is refused.
pub fn validate_description(description: &str) -> Result<String, Refusal> {
    let d = description.trim();
    match d.chars().count() {
        n if n < MIN_DESCRIPTION => Err(Refusal::DescriptionTooShort),
        n if n > MAX_DESCRIPTION => Err(Refusal::DescriptionTooLong),
        _ => Ok(d.to_owned()),
    }
}

/// A medal needs at least one row for each of the two parts, every part must
/// be one of them, and every graphic name must be `label.digits_digits` with
/// a non-zero first number (see the module's trap).
pub fn validate_graphics(graphics: &[Graphic]) -> Result<(), Refusal> {
    if graphics.len() > MAX_GRAPHICS {
        return Err(Refusal::BadGraphics);
    }
    let mut have = [false, false];
    for g in graphics {
        match g.part {
            PART_RIBBON => have[0] = true,
            PART_MEDAL => have[1] = true,
            _ => return Err(Refusal::BadGraphics),
        }
        if !graphic_name_ok(&g.graphic) {
            return Err(Refusal::BadGraphics);
        }
    }
    if have == [true, true] {
        Ok(())
    } else {
        Err(Refusal::BadGraphics)
    }
}

/// `label.digits_digits`, the first number not zero: what `MapLogo` can parse.
fn graphic_name_ok(name: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let Some((label, icon)) = name.split_once('.') else {
        return false;
    };
    let Some((a, b)) = icon.split_once('_') else {
        return false;
    };
    !label.is_empty()
        && digits(a)
        && digits(b)
        && a.parse::<u32>().is_ok_and(|n| n != 0)
        && b.parse::<u32>().is_ok()
}

fn medal_key(id: i64) -> Vec<String> {
    vec!["m".to_owned(), id.to_string()]
}

fn award_key(medal: i64, character: i64, n: i64) -> Vec<String> {
    vec![medal.to_string(), character.to_string(), n.to_string()]
}

impl Medal {
    fn row(&self) -> Vec<String> {
        let mut v = vec![
            self.corp.to_string(),
            self.creator.to_string(),
            self.date.to_string(),
            self.title.clone(),
            self.description.clone(),
        ];
        for g in &self.graphics {
            v.push(g.part.to_string());
            v.push(g.graphic.clone());
            v.push(g.color.to_string());
        }
        v
    }

    fn parse(id: i64, v: &[String]) -> Option<Medal> {
        let n = |i: usize| v.get(i)?.parse::<i64>().ok();
        let rest = v.get(5..)?;
        if rest.len() % 3 != 0 {
            return None;
        }
        let graphics = rest
            .chunks(3)
            .map(|c| {
                Some(Graphic {
                    part: c[0].parse().ok()?,
                    graphic: c[1].clone(),
                    color: c[2].parse().ok()?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Medal {
            id,
            corp: n(0)?,
            creator: n(1)?,
            date: n(2)?,
            title: v.get(3)?.clone(),
            description: v.get(4)?.clone(),
            graphics,
        })
    }
}

impl Award {
    fn row(&self) -> Vec<String> {
        vec![
            self.issuer.to_string(),
            self.date.to_string(),
            self.reason.clone(),
            self.status.to_string(),
        ]
    }

    fn parse(key: &[String], v: &[String]) -> Option<Award> {
        let k = |i: usize| key.get(i)?.parse::<i64>().ok();
        Some(Award {
            medal: k(0)?,
            character: k(1)?,
            n: k(2)?,
            issuer: v.first()?.parse().ok()?,
            date: v.get(1)?.parse().ok()?,
            reason: v.get(2)?.clone(),
            status: v.get(3)?.parse().ok()?,
        })
    }
}

/// Take the next medal id (1, 2, ...).
fn alloc_id(d: &mut Deltas) -> i64 {
    let key = vec!["next".to_owned()];
    let n = d
        .of(KIND)
        .get(&key)
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(1);
    d.put(KIND, key, vec![(n + 1).to_string()]);
    n
}

/// Make a medal for `corp`. The caller has checked the role and every
/// `validate_*`; the text is stored as given.
pub fn create(
    d: &mut Deltas,
    corp: i64,
    creator: i64,
    date: i64,
    title: String,
    description: String,
    graphics: Vec<Graphic>,
) -> Medal {
    let m = Medal {
        id: alloc_id(d),
        corp,
        creator,
        date,
        title,
        description,
        graphics,
    };
    d.put(KIND, medal_key(m.id), m.row());
    m
}

pub fn get(d: &Deltas, id: i64) -> Option<Medal> {
    Medal::parse(id, d.of(KIND).get(&medal_key(id))?)
}

/// One corp's medals, oldest id first.
pub fn of_corp(d: &Deltas, corp: i64) -> Vec<Medal> {
    let mut v: Vec<Medal> = d
        .of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 2 && k[0] == "m")
        .filter_map(|(k, row)| Medal::parse(k[1].parse().ok()?, row))
        .filter(|m| m.corp == corp)
        .collect();
    v.sort_by_key(|m| m.id);
    v
}

fn all_awards(d: &Deltas) -> impl Iterator<Item = Award> + '_ {
    d.of(AWARD_KIND)
        .iter()
        .filter_map(|(k, v)| Award::parse(k, v))
}

/// Give `medal` to `character`; every call is a new award, so a second one is
/// counted. Returns the award.
///
/// **[A]** A further award takes the status of the pilot's newest award of
/// that medal that is not deleted, else [`DEFAULT_STATUS`]: the Decorations
/// tab reads one status from the first row and `HasChanged` compares against
/// it, so a mixed set could never be saved. Falsifier: retail showing a
/// repeat award private while the earlier ones are public.
pub fn give(
    d: &mut Deltas,
    medal: i64,
    character: i64,
    issuer: i64,
    date: i64,
    reason: &str,
) -> Award {
    let mine: Vec<Award> = all_awards(d)
        .filter(|a| a.medal == medal && a.character == character)
        .collect();
    let n = mine.iter().map(|a| a.n).max().unwrap_or(0) + 1;
    let status = mine
        .iter()
        .filter(|a| a.status != status::DELETED)
        .max_by_key(|a| a.n)
        .map_or(DEFAULT_STATUS, |a| a.status);
    let a = Award {
        medal,
        character,
        n,
        issuer,
        date,
        reason: clip(reason, MAX_REASON),
        status,
    };
    d.put(AWARD_KIND, award_key(medal, character, n), a.row());
    a
}

/// Every award `character` holds, by medal then count.
pub fn received(d: &Deltas, character: i64) -> Vec<Award> {
    let mut v: Vec<Award> = all_awards(d).filter(|a| a.character == character).collect();
    v.sort_by_key(|a| (a.medal, a.n));
    v
}

/// Every award of one medal, by recipient then count.
pub fn recipients(d: &Deltas, medal: i64) -> Vec<Award> {
    let mut v: Vec<Award> = all_awards(d).filter(|a| a.medal == medal).collect();
    v.sort_by_key(|a| (a.character, a.n));
    v
}

/// How many times `medal` has been awarded.
pub fn award_count(d: &Deltas, medal: i64) -> usize {
    all_awards(d).filter(|a| a.medal == medal).count()
}

/// Set the status of every award `character` holds of `medal`. Returns
/// whether the pilot holds it. A status outside 1 to 3 changes nothing, and a
/// deleted award stays deleted unless the new status is 1 (publishing must
/// not bring back what the pilot removed).
pub fn set_status(d: &mut Deltas, character: i64, medal: i64, new: i64) -> bool {
    let mine: Vec<Award> = all_awards(d)
        .filter(|a| a.medal == medal && a.character == character)
        .collect();
    if mine.is_empty() {
        return false;
    }
    if !(status::DELETED..=status::PUBLIC).contains(&new) {
        return true;
    }
    for mut a in mine {
        if a.status == status::DELETED && new != status::DELETED {
            continue;
        }
        a.status = new;
        d.put(AWARD_KIND, award_key(a.medal, a.character, a.n), a.row());
    }
    true
}

/// What `viewer` may see of `owner`'s awards: the owner sees all of them (the
/// Decorations tab lists the deleted too), anyone else only the public ones.
pub fn visible_to(awards: Vec<Award>, owner: i64, viewer: i64) -> Vec<Award> {
    if owner == viewer {
        awards
    } else {
        awards
            .into_iter()
            .filter(|a| a.status == status::PUBLIC)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORP: i64 = 98_000_701;
    const ALICE: i64 = 90_000_711;
    const BOB: i64 = 90_000_712;
    const CAROL: i64 = 90_000_713;
    const WHEN: i64 = 134_000_000_000_000_000;

    fn g(part: i64, name: &str) -> Graphic {
        Graphic {
            part,
            graphic: name.to_owned(),
            color: 4,
        }
    }

    fn both() -> Vec<Graphic> {
        vec![g(1, "a.1_1"), g(2, "b.2_3")]
    }

    fn deltas() -> Deltas {
        Deltas::default()
    }

    fn make(d: &mut Deltas) -> Medal {
        create(
            d,
            CORP,
            ALICE,
            WHEN,
            "Rock Breaker".to_owned(),
            "For breaking rocks".to_owned(),
            both(),
        )
    }

    #[test]
    fn a_medal_needs_both_parts() {
        assert_eq!(validate_graphics(&both()), Ok(()));
        assert_eq!(
            validate_graphics(&[g(1, "a.1_1")]),
            Err(Refusal::BadGraphics),
            "no medal part: FormatMedalData iterates None"
        );
        assert_eq!(
            validate_graphics(&[g(2, "b.2_3")]),
            Err(Refusal::BadGraphics),
            "no ribbon part"
        );
        assert_eq!(validate_graphics(&[]), Err(Refusal::BadGraphics));
        assert_eq!(
            validate_graphics(&[g(1, "a.1_1"), g(3, "b.2_3")]),
            Err(Refusal::BadGraphics),
            "part 3 is not a part"
        );
        for bad in [
            "nodot",
            "two.dots.1_1",
            ".1_1",
            "a.",
            "a.x.1_1",
            "a.1",
            "a.1_",
            "a._1",
            "a.x_y",
            "a.0_5",
            "a.1_2_3",
            "a.-1_2",
            "a.99999999999_1",
        ] {
            assert_eq!(
                validate_graphics(&[g(1, bad), g(2, "b.2_3")]),
                Err(Refusal::BadGraphics),
                "{bad:?}: MapLogo needs label.digits_digits"
            );
        }
    }

    #[test]
    fn the_text_limits_are_the_clients() {
        assert_eq!(validate_title("ab"), Err(Refusal::TitleTooShort));
        assert_eq!(validate_title("  ab  "), Err(Refusal::TitleTooShort));
        assert_eq!(validate_title(&"x".repeat(101)), Err(Refusal::TitleTooLong));
        assert_eq!(validate_title("  abc "), Ok("abc".to_owned()));
        assert_eq!(
            validate_description("too short"),
            Err(Refusal::DescriptionTooShort)
        );
        assert_eq!(
            validate_description(&"x".repeat(1001)),
            Err(Refusal::DescriptionTooLong)
        );
        assert!(validate_description(&"x".repeat(1000)).is_ok());
    }

    #[test]
    fn only_director_or_personnel_creates() {
        assert!(may_manage(ROLE_DIRECTOR, false));
        assert!(may_manage(ROLE_PERSONNEL_MANAGER, false));
        assert!(may_manage(0, true), "the CEO");
        assert!(!may_manage(0, false));
        assert!(
            !may_manage(2 | 4 | 256, false),
            "other roles do not manage medals"
        );
    }

    #[test]
    fn a_second_award_is_counted() {
        let mut d = deltas();
        let m = make(&mut d);
        let first = give(&mut d, m.id, BOB, ALICE, WHEN, "for rocks");
        let second = give(&mut d, m.id, BOB, ALICE, WHEN + 1, "for more rocks");
        give(&mut d, m.id, CAROL, ALICE, WHEN, "");
        assert_eq!((first.n, second.n), (1, 2));
        assert_eq!(received(&d, BOB).len(), 2, "one row per award");
        assert_eq!(received(&d, CAROL).len(), 1);
        assert_eq!(award_count(&d, m.id), 3);
        assert_eq!(recipients(&d, m.id).len(), 3);
        // The tenth award sorts after the ninth by number, not by text.
        for _ in 0..9 {
            give(&mut d, m.id, BOB, ALICE, WHEN, "");
        }
        let ns: Vec<i64> = received(&d, BOB).iter().map(|a| a.n).collect();
        assert_eq!(ns, (1..=11).collect::<Vec<_>>());
    }

    #[test]
    fn status_filters_public_and_private() {
        let mut d = deltas();
        let m = make(&mut d);
        let n = make(&mut d);
        give(&mut d, m.id, BOB, ALICE, WHEN, "a");
        give(&mut d, m.id, BOB, ALICE, WHEN, "b");
        give(&mut d, n.id, BOB, ALICE, WHEN, "c");
        // Privacy is asserted from an explicit status, not the [A] default.
        for medal in [m.id, n.id] {
            assert!(set_status(&mut d, BOB, medal, status::PRIVATE));
        }
        let mine = received(&d, BOB);
        assert!(
            visible_to(mine.clone(), BOB, ALICE).is_empty(),
            "a private award is the owner's alone"
        );
        assert_eq!(visible_to(mine, BOB, BOB).len(), 3);
        assert!(set_status(&mut d, BOB, m.id, status::PUBLIC));
        let seen = visible_to(received(&d, BOB), BOB, ALICE);
        assert_eq!(
            seen.len(),
            2,
            "both awards of the public medal, not the other"
        );
        assert!(seen.iter().all(|a| a.medal == m.id));
        // Deleted is the owner's alone.
        assert!(set_status(&mut d, BOB, m.id, status::DELETED));
        assert!(visible_to(received(&d, BOB), BOB, ALICE).is_empty());
        assert_eq!(visible_to(received(&d, BOB), BOB, BOB).len(), 3);
        // A medal one does not hold, and a status that is not one.
        assert!(!set_status(&mut d, CAROL, m.id, status::PUBLIC));
        assert!(set_status(&mut d, BOB, n.id, 9));
        assert_eq!(
            received(&d, BOB)
                .iter()
                .find(|a| a.medal == n.id)
                .unwrap()
                .status,
            status::PRIVATE,
            "9 changed nothing"
        );
    }

    #[test]
    fn a_new_award_follows_the_pilots_status_for_that_medal() {
        let mut d = deltas();
        let m = make(&mut d);
        give(&mut d, m.id, BOB, ALICE, WHEN, "a");
        give(&mut d, m.id, BOB, ALICE, WHEN, "b");
        set_status(&mut d, BOB, m.id, status::PUBLIC);
        let third = give(&mut d, m.id, BOB, ALICE, WHEN, "c");
        assert_eq!(third.status, status::PUBLIC, "joins the public set");
        // Deleted awards are skipped; publishing does not revive them.
        set_status(&mut d, BOB, m.id, status::DELETED);
        let fourth = give(&mut d, m.id, BOB, ALICE, WHEN, "d");
        assert_eq!(fourth.status, DEFAULT_STATUS, "nothing live to follow");
        set_status(&mut d, BOB, m.id, status::PUBLIC);
        let st: Vec<i64> = received(&d, BOB).iter().map(|a| a.status).collect();
        assert_eq!(st, vec![1, 1, 1, status::PUBLIC]);
        // Another pilot's set is not Bob's.
        assert_eq!(
            give(&mut d, m.id, CAROL, ALICE, WHEN, "").status,
            DEFAULT_STATUS
        );
    }

    #[test]
    fn rows_survive_a_restart() {
        let mut d = deltas();
        let m = create(
            &mut d,
            CORP,
            ALICE,
            WHEN,
            "Tab\tand\nline".to_owned(),
            "Quotes \"here\" and a tab\t.".to_owned(),
            both(),
        );
        give(&mut d, m.id, BOB, ALICE, WHEN + 5, "because\tof it");
        set_status(&mut d, BOB, m.id, status::PUBLIC);
        // The world file keeps rows verbatim; rebuild the store from them.
        let mut again = deltas();
        for kind in [KIND, AWARD_KIND] {
            for (k, v) in d.of(kind) {
                again.put(kind, k.clone(), v.clone());
            }
        }
        assert_eq!(get(&again, m.id), Some(m.clone()));
        assert_eq!(of_corp(&again, CORP), vec![m.clone()]);
        let got = received(&again, BOB);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].reason, "because\tof it");
        assert_eq!(got[0].status, status::PUBLIC);
        let next = make(&mut again);
        assert_eq!(next.id, m.id + 1, "the counter is kept too");
    }

    #[test]
    fn a_reason_is_cut_at_the_prompts_length() {
        let mut d = deltas();
        let m = make(&mut d);
        let a = give(&mut d, m.id, BOB, ALICE, WHEN, &"r".repeat(500));
        assert_eq!(a.reason.chars().count(), MAX_REASON);
    }

    #[test]
    fn a_damaged_row_is_skipped_not_fatal() {
        let mut d = deltas();
        let m = make(&mut d);
        d.put(
            KIND,
            vec!["m".to_owned(), "99".to_owned()],
            vec!["x".to_owned()],
        );
        d.put(AWARD_KIND, vec!["a".to_owned()], vec![]);
        assert_eq!(of_corp(&d, CORP), vec![m]);
        assert!(received(&d, BOB).is_empty());
    }
}
