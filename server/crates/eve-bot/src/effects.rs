//! **The visible effects of the relations model** (R-REL-3, R-RELATIONS
//! §3.1-3.5 and §4): what a bot *does* with the number [`relations::r_eff`]
//! gives. The decisions here are pure (invented numbers in, a verdict out);
//! the wire functions are thin.
//!
//! | effect | decision | wire |
//! |---|---|---|
//! | contacts, graded | [`Stance5::standing`] per pair (the hall's `tick`) | `Plan::Contacts` (corp and alliance books), [`personal_contacts`] (the bot's own book) |
//! | recruit | [`may_invite`] | `Plan::Invite` (`corpRegistry.InsertInvitation`) |
//! | accept an applicant | [`accepts`] | `Plan::Recruit` |
//! | reprice or refuse | [`counterparty_gate`], [`Gate`] | [`crate::contracts::take_underpriced_gated`], [`crate::contracts::plan_offers_for`] |
//! | help | [`help_decision`] | `Observation::help`, the brain's rule 2c |
//!
//! ## The seam for R-REL-4 (the human side)
//!
//! Everything here reads one number per human: `R_eff` from
//! [`HallState::r_eff_of`](crate::corpmind::HallState::r_eff_of), which already
//! composes the human prior, `--friends` and the facts R-REL-2 records. R-REL-4
//! adds only inputs: [`HallState::note_fleet_hours`](crate::corpmind::HallState::note_fleet_hours)
//! (F5, the shared-fleet clock) and the staging mail for Allied; the effects
//! then follow without a change here. `HallState::humans` is the list of pilots
//! the bots have seen that no bot flies.
//!
//! Every number is **[A]**, with its falsifier at its constant; no test holds
//! one (tests name the consts).

use crate::corpmind::Hall;
use crate::mind::{Mind, Regard, Trait};
use crate::relations::{self, Temper};
use crate::society::{self, ALLY_AT, ENEMY_AT, FRIEND_AT, Stance5, WARY_AT};
use crate::{Conn, Result};
use eve_marshal::PyValue;
use eve_server::group_contacts::Affiliation;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

// ---- contacts -------------------------------------------------------------------------

/// A bot's own book carries a pilot at `|P|` at or above this **[A]**.
/// Falsified by the overview colouring pilots the owner thinks of as
/// strangers (too low), or by a known rival still grey (too high).
pub const PERSONAL_CONTACT_AT: f64 = 4.0;
/// A personal contact is kept while `|P|` stays at or above this **[A]**
/// (the hysteresis; the same half-band the groups use).
pub const PERSONAL_LEAVE: f64 = 2.0;
/// Personal contacts a bot holds at most **[A]** (the client's own book is
/// far larger; this is a state bound).
pub const MAX_PERSONAL_CONTACTS: usize = 40;

/// Is `id` a character (not a corp, an alliance or an NPC)? Regards keyed by
/// a group (a killer's corp) have no place in a personal book.
pub fn is_character(id: i64) -> bool {
    id >= 90_000_000 && !society::is_player_corp(id) && !society::PLAYER_ALLIANCES.contains(&id)
}

/// **A personal contact's standing** for regard `p`, given the standing the
/// book holds (`cur`): enter at [`PERSONAL_CONTACT_AT`] as +-5 (+-10 at the
/// group thresholds), leave below [`PERSONAL_LEAVE`], and an allied (+-10)
/// entry drops to +-5 only below `ALLY_AT - BAND`. `None`: no contact.
pub fn personal_standing(cur: Option<f64>, p: f64) -> Option<f64> {
    let sign = if p >= 0.0 { 1.0 } else { -1.0 };
    let m = p.abs();
    let held = cur.filter(|c| c.signum() == sign && *c != 0.0);
    match held {
        Some(c) if c.abs() >= society::BLUE_STANDING => {
            if m >= ALLY_AT - society::BAND {
                Some(sign * society::BLUE_STANDING)
            } else if m >= PERSONAL_LEAVE {
                Some(sign * 5.0)
            } else {
                None
            }
        }
        Some(_) => {
            if m >= ALLY_AT {
                Some(sign * society::BLUE_STANDING)
            } else if m >= PERSONAL_LEAVE {
                Some(sign * 5.0)
            } else {
                None
            }
        }
        None => {
            if m >= ALLY_AT {
                Some(sign * society::BLUE_STANDING)
            } else if m >= PERSONAL_CONTACT_AT {
                Some(sign * 5.0)
            } else {
                None
            }
        }
    }
}

/// **The personal-book calls**: `book` is the bot's book as read
/// (`contactID -> standing`), `regards` its decayed regards (`who -> P`),
/// `friends` the `--friends` file's floor (the merge keeps them, never lowers them, [`crate::contact_plan`]). Answers
/// `(contact, Some(standing))` to set or edit and `(contact, None)` to remove.
///
/// Only characters are set, at most [`MAX_PERSONAL_CONTACTS`] (the strongest
/// first); only a contact the bot holds a regard for is ever removed, so a
/// contact somebody else put in the book stays.
pub fn personal_plan(
    book: &BTreeMap<i64, f64>,
    regards: &BTreeMap<i64, f64>,
    friends: &BTreeMap<i64, f64>,
) -> Vec<(i64, Option<f64>)> {
    use crate::contact_plan::{ContactPlan, Source};
    let mut want: Vec<(i64, f64, f64)> = regards
        .iter()
        .filter(|(who, _)| is_character(**who) && !friends.contains_key(who))
        .filter_map(|(who, p)| {
            personal_standing(book.get(who).copied(), *p).map(|s| (*who, s, p.abs()))
        })
        .collect();
    want.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));
    let held_elsewhere = book
        .keys()
        .filter(|k| !regards.contains_key(k) || friends.contains_key(k))
        .count();
    want.truncate(MAX_PERSONAL_CONTACTS.saturating_sub(held_elsewhere));
    // REVIEW-FIX2: the tier's wants and the `--friends` floor go through the
    // one merge; only it says what the book holds.
    let mut plan = ContactPlan::new();
    for (who, s, _) in &want {
        plan.claim(0, *who, *s, Source::Tier);
    }
    for (who, s) in friends {
        plan.claim(0, *who, *s, Source::Friends);
    }
    plan.diff(0, book, &|who| {
        regards.contains_key(&who) && is_character(who)
    })
}

/// The regards as `who -> P`, decayed to `now` (the input of [`personal_plan`]).
pub fn regards_now(list: &[Regard], now: u64, day: u64, half_days: f64) -> BTreeMap<i64, f64> {
    list.iter()
        .map(|r| (r.who, relations::regard_decayed(r, now, day, half_days)))
        .collect()
}

/// How often a bot reads and tends its own book, game seconds **[A]**.
/// Falsified by a rival staying grey for an evening after a kill (too long).
pub const PERSONAL_EVERY: u64 = 10 * 60;

/// **Tend the bot's own contact book** (the wire half of [`personal_plan`]):
/// `charMgr.GetContactList`, then `AddContact` / `EditContactsRelationshipID`
/// / `DeleteContacts`, the client's own calls **[V]** (`contacts.rs`). Returns
/// one line per change. A bot that took the turn recently does nothing.
pub fn personal_contacts(
    conn: &mut Conn,
    hall: &Hall,
    mind: &Arc<Mutex<Mind>>,
    now: u64,
) -> Result<Vec<String>> {
    let Some(me) = conn.session().char_id() else {
        return Ok(Vec::new());
    };
    let (day, friends) = {
        let mut st = hall.lock();
        if st
            .personal_tried
            .get(&me)
            .is_some_and(|t| now.saturating_sub(*t) < PERSONAL_EVERY)
        {
            return Ok(Vec::new());
        }
        st.personal_tried.insert(me, now);
        (st.day, st.friends.clone())
    };
    let regards = {
        let m = mind.lock().unwrap_or_else(|p| p.into_inner());
        let half = relations::personal_half_days(m.traits.get(Trait::Loyalty), &Temper::NORMAL);
        regards_now(&m.regard, now, day, half)
    };
    let out = conn.call("charMgr", "GetContactList", vec![])?;
    if out.refusal().is_some() {
        return Ok(Vec::new());
    }
    let Some(v) = out.value() else {
        return Ok(Vec::new());
    };
    let book: BTreeMap<i64, f64> = crate::observe::list(&v, "addresses")
        .unwrap_or_default()
        .iter()
        .filter_map(|row| {
            Some((
                crate::observe::int(row, "contactID")?,
                crate::observe::real(row, "relationshipID")?,
            ))
        })
        .collect();
    let mut lines = Vec::new();
    let mut gone = Vec::new();
    for (who, standing) in personal_plan(&book, &regards, &friends) {
        match standing {
            Some(s) => {
                let call = if book.contains_key(&who) {
                    conn.call(
                        "charMgr",
                        "EditContactsRelationshipID",
                        vec![PyValue::List(vec![PyValue::Int(who)]), PyValue::Real(s)],
                    )?
                } else {
                    conn.call(
                        "charMgr",
                        "AddContact",
                        vec![PyValue::Int(who), PyValue::Real(s), PyValue::Bool(false)],
                    )?
                };
                lines.push(match call.refusal() {
                    Some(key) => format!("personal contact {who} {s:+}: refused {key}"),
                    None => {
                        hall.lock().act(format!("{me} contacts {who} {s:+}"));
                        format!("personal contact {who} {s:+}: ok")
                    }
                });
            }
            None => gone.push(who),
        }
    }
    if !gone.is_empty() {
        let ids = PyValue::List(gone.iter().map(|g| PyValue::Int(*g)).collect());
        let call = conn.call("charMgr", "DeleteContacts", vec![ids])?;
        lines.push(match call.refusal() {
            Some(key) => format!("personal contacts clear {gone:?}: refused {key}"),
            None => format!("personal contacts clear {gone:?}: ok"),
        });
    }
    Ok(lines)
}

// ---- recruiting and accepting -----------------------------------------------------------

/// A pilot at `R_eff` this or above is invited to a corp **[A]**. Falsified
/// by friends saying they get invites from strangers, or never.
pub const INVITE_AT: f64 = 3.5;
/// ... and has flown in a fleet with the corp's people this many game hours
/// **[A]** (F5; R-REL-4 feeds it).
pub const INVITE_FLEET_HOURS: f64 = 2.0;
/// A pilot is invited again no sooner than this many game days **[A]**.
pub const INVITE_EVERY_DAYS: f64 = 3.0;
/// The fit a friendly applicant needs **[A]**; falsified by a friend being
/// refused a corp that has room.
pub const ACCEPT_FIT_FRIEND: f64 = 0.2;
/// The fit a wary applicant needs **[A]**.
pub const ACCEPT_FIT_WARY: f64 = 0.7;
/// A human corp joins a bot alliance when `C_eff` toward it reaches
/// `ALLY_AT - ALLIANCE_SLACK` **[A]**.
pub const ALLIANCE_SLACK: f64 = 1.0;

/// **Should the CEO invite this pilot?** `r` is `R_eff` toward them, `hours`
/// the shared fleet hours, `in_player_corp` whether they already belong to a
/// player corp (then the answer is no: a corp does not poach).
pub fn may_invite(r: f64, hours: f64, in_player_corp: bool) -> bool {
    !in_player_corp && r >= INVITE_AT && hours >= INVITE_FLEET_HOURS
}

/// **The CEO's verdict on an applicant** (R-RELATIONS §3.2): `stance` is the
/// CEO's reading of the applicant ([`stance_of`] of `R_eff`, or the corp's
/// settled stance when that is worse), `fit` the profile fit.
/// A hostile applicant is refused; a wary one needs [`ACCEPT_FIT_WARY`]; a
/// friendly one [`ACCEPT_FIT_FRIEND`]; anyone else the corp's own bar
/// (`base`, [`society::ceo_accepts`] without its enemy test).
pub fn accepts(stance: Stance5, fit: f64, base: bool) -> bool {
    match stance {
        Stance5::Hostile => false,
        Stance5::Wary => fit >= ACCEPT_FIT_WARY && base,
        Stance5::Friendly | Stance5::Allied => fit >= ACCEPT_FIT_FRIEND,
        Stance5::Neutral => base,
    }
}

/// `r` read as a stance with no memory (a verdict, not a transition).
pub fn stance_of(r: f64) -> Stance5 {
    if r >= ALLY_AT {
        Stance5::Allied
    } else if r >= FRIEND_AT {
        Stance5::Friendly
    } else if r <= ENEMY_AT {
        Stance5::Hostile
    } else if r <= WARY_AT {
        Stance5::Wary
    } else {
        Stance5::Neutral
    }
}

/// Does a human corp's standing admit it to a bot alliance?
pub fn alliance_admits_human(c_eff: f64) -> bool {
    c_eff >= ALLY_AT - ALLIANCE_SLACK
}

// ---- contracts ----------------------------------------------------------------------------

/// A wary counterparty pays this multiple of the ask **[A]**. Falsified by
/// wary buyers still buying everything (too low).
pub const WARY_PREMIUM: f64 = 1.2;
/// An allied counterparty pays this multiple of the ask **[A]**, bounded
/// by the seller's floor. Falsified by allies being the builders' worst
/// customers by margin.
pub const ALLY_DISCOUNT: f64 = 0.9;
/// A friendly buyer's wants count this much more in a builder's need **[A]**.
pub const FRIEND_NEED_BONUS: f64 = 1.25;

/// What a bot does with a counterparty (R-RELATIONS §3.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gate {
    /// `R_eff <= ENEMY_AT`: no deal either way.
    Refuse,
    /// Wary: sells at the premium; buys only at a deeper bargain.
    Premium(f64),
    Normal,
    /// Allied: sells at a small discount.
    Discount(f64),
}

/// **The counterparty gate** for `R_eff` `r`.
pub fn counterparty_gate(r: f64) -> Gate {
    match stance_of(r) {
        Stance5::Hostile => Gate::Refuse,
        Stance5::Wary => Gate::Premium(WARY_PREMIUM),
        Stance5::Allied => Gate::Discount(ALLY_DISCOUNT),
        _ => Gate::Normal,
    }
}

impl Gate {
    /// The price asked of this counterparty for goods asked at `ask` and
    /// worth at least `floor` to the seller (never below it, never above
    /// the premium); `None`: refused.
    pub fn ask(self, ask: f64, floor: f64) -> Option<f64> {
        match self {
            Gate::Refuse => None,
            Gate::Premium(m) => Some(ask * m),
            Gate::Normal => Some(ask),
            Gate::Discount(m) => Some((ask * m).max(floor).min(ask)),
        }
    }

    /// The share of the reference value this counterparty's offer may cost a
    /// buyer whose normal bar is `bar`: a wary seller must be cheaper by the
    /// premium it would charge; `None`: refused.
    pub fn bar(self, bar: f64) -> Option<f64> {
        match self {
            Gate::Refuse => None,
            Gate::Premium(m) => Some(bar / m),
            _ => Some(bar),
        }
    }
}

/// This bot's regard list and loyalty, copied out of its mind (so no lock is
/// held while the hall's is).
fn own_regard(mind: &Arc<Mutex<Mind>>) -> (Vec<Regard>, f64) {
    let m = mind.lock().unwrap_or_else(|p| p.into_inner());
    (m.regard.clone(), m.traits.get(Trait::Loyalty))
}

/// **The trader's accept through the gate** (the wire half of
/// [`counterparty_gate`]): [`crate::contracts::take_underpriced`] with each
/// seller read as `R_eff` toward them. A hostile seller's contracts are not
/// taken; a wary one's must be cheaper by the premium.
pub fn take_underpriced_as(
    conn: &mut Conn,
    hall: &Hall,
    mind: &Arc<Mutex<Mind>>,
    now: u64,
) -> Result<crate::contracts::Taken> {
    let prices: BTreeMap<i64, f64> = conn.adjusted_prices()?.into_iter().collect();
    let corp = conn.session().get_i64("corpid").unwrap_or(0);
    let (regard, loyalty) = own_regard(mind);
    let gate_of = |issuer: i64, aff: Option<crate::fleet::Affiliation>| {
        let a = Affiliation {
            character_id: issuer,
            corporation_id: aff.map_or(0, |a| a.corp),
            alliance_id: aff.and_then(|a| a.alliance),
        };
        counterparty_gate(hall.lock().r_eff_of(corp, &regard, loyalty, &a, now))
    };
    crate::contracts::take_underpriced_gated(conn, &prices, Some(&gate_of))
}

/// **A seller's surplus, with an ally in mind**: [`crate::contracts::offer_surplus_for`]
/// where the buyer is the most trusted human the hall has seen, if `R_eff`
/// toward them reaches the allied bar (their stack is offered at the
/// discount, privately; the rest stays public).
pub fn offer_surplus_as(
    conn: &mut Conn,
    hall: &Hall,
    mind: &Arc<Mutex<Mind>>,
    career: crate::contracts::Career,
    now: u64,
) -> Result<String> {
    let corp = conn.session().get_i64("corpid").unwrap_or(0);
    let (regard, loyalty) = own_regard(mind);
    let buyer = {
        let st = hall.lock();
        st.humans
            .iter()
            .map(|(id, s)| {
                let a = Affiliation {
                    character_id: *id,
                    corporation_id: s.corp,
                    alliance_id: s.alliance,
                };
                (*id, st.r_eff_of(corp, &regard, loyalty, &a, now))
            })
            .filter(|(_, r)| *r >= ALLY_AT)
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
            .map(|(id, r)| crate::contracts::Buyer {
                id,
                gate: counterparty_gate(r),
            })
    };
    crate::contracts::offer_surplus_for(conn, career, buyer)
}

/// A need multiplied up for a friendly buyer's want (`signal::product_need`'s
/// bonus, R-RELATIONS §3.3).
pub fn friend_need(need: f64, buyer_r: f64) -> f64 {
    if buyer_r >= FRIEND_AT {
        need * FRIEND_NEED_BONUS
    } else {
        need
    }
}

/// REVIEW-FIX2: **the friends' wants** (pure): for each type a listed
/// want-to-buy asks for, the highest regard `r_of` gives its issuers.
pub fn friend_wants_of(
    listings: &[crate::contracts::Listing],
    r_of: &dyn Fn(i64) -> f64,
) -> BTreeMap<i64, f64> {
    let mut out: BTreeMap<i64, f64> = BTreeMap::new();
    for l in listings {
        if l.wants.is_empty() {
            continue;
        }
        let r = r_of(l.issuer_id);
        for &(t, _) in &l.wants {
            let e = out.entry(t).or_insert(f64::MIN);
            *e = e.max(r);
        }
    }
    out
}

/// REVIEW-FIX2: the public want-to-buy contracts at the bot's station, read
/// as `R_eff` toward each issuer ([`friend_wants_of`]); `signal::apply_buyers`
/// hands the result to the builder's product need.
pub fn friend_wants(
    conn: &mut Conn,
    hall: &Hall,
    mind: &Arc<Mutex<Mind>>,
    now: u64,
) -> Result<BTreeMap<i64, f64>> {
    let Some(station) = conn.session().station_id() else {
        return Ok(BTreeMap::new());
    };
    let listings = crate::contracts::search_here(conn, station)?;
    let corp = conn.session().get_i64("corpid").unwrap_or(0);
    let (regard, loyalty) = own_regard(mind);
    let st = hall.lock();
    Ok(friend_wants_of(&listings, &|issuer| {
        let a = Affiliation {
            character_id: issuer,
            corporation_id: 0,
            alliance_id: None,
        };
        st.r_eff_of(corp, &regard, loyalty, &a, now)
    }))
}

// ---- help ----------------------------------------------------------------------------------

/// A `NeedBackup` from a pilot at `R_eff` this or above is answered **[A]**.
pub const HELP_AT: f64 = 3.0;
/// ... if the bot is no further than this many jumps **[A]**.
pub const HELP_JUMPS: u32 = 3;
/// A loyal bot (trait) lowers the odds bar by this x loyalty **[A]**.
pub const LOYAL_ODDS: f64 = 0.5;

/// What the help decision reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HelpIn {
    /// `R_eff` toward the caller.
    pub r: f64,
    /// The odds with this bot added ([`crate::hunt::odds`]); `None`: not
    /// known (the bot cannot see the fight), which counts as enough for a
    /// friend (a bot goes to see).
    pub odds: Option<f64>,
    /// [`crate::hunt::required_odds`] at the bot's aggression.
    pub required: f64,
    /// Jumps to the caller; `None`: unreachable.
    pub jumps: Option<u32>,
    /// In a rung of its own fight.
    pub busy: bool,
    /// The bot's loyalty trait, `0..=1`.
    pub loyalty: f64,
}

/// **Does the bot go?** (pure) A caller is helped when `R_eff >= HELP_AT`,
/// within [`HELP_JUMPS`], not busy, and the odds (with the helper) reach the
/// bar the bot's aggression sets, lowered by [`LOYAL_ODDS`] x loyalty.
pub fn help_decision(i: &HelpIn) -> bool {
    if i.busy || i.r < HELP_AT {
        return false;
    }
    if i.jumps.is_none_or(|j| j > HELP_JUMPS) {
        return false;
    }
    let bar = i.required - LOYAL_ODDS * i.loyalty.clamp(0.0, 1.0);
    i.odds.is_none_or(|o| o >= bar)
}

/// **The brain's rule for a friend's call** (rule 2c): when the driver
/// weighed a `NeedBackup` and said go ([`crate::brain::Observation::help`]),
/// an armed bot in space, not already homing or hunting, warps to the
/// caller. `None`: the role decides.
pub fn help_acts(
    m: &crate::brain::Memory,
    o: &crate::brain::Observation,
) -> Option<Vec<crate::brain::Act>> {
    let char_id = o.help?;
    if o.guns.is_empty()
        || m.homing.is_some()
        || m.hunt.prey.is_some()
        || !matches!(o.place, crate::brain::Place::InSpace { bound: true, .. })
    {
        return None;
    }
    Some(vec![crate::brain::Act::WarpToMember {
        char_id,
        range: crate::escort::WARP_STANDOFF,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: i64 = 90_000_811;
    const B: i64 = 90_000_812;
    const CORP: i64 = 98_000_811;

    #[test]
    fn a_character_is_not_a_corp_or_an_alliance() {
        assert!(is_character(A));
        assert!(!is_character(CORP));
        assert!(!is_character(99_000_811));
        assert!(!is_character(1_000_044));
    }

    #[test]
    fn a_personal_contact_enters_leaves_and_flips_with_its_band() {
        // Enters as +5 at the contact threshold, +10 at the ally one.
        assert_eq!(personal_standing(None, PERSONAL_CONTACT_AT), Some(5.0));
        assert_eq!(personal_standing(None, ALLY_AT), Some(10.0));
        assert_eq!(personal_standing(None, PERSONAL_CONTACT_AT - 0.1), None);
        assert_eq!(personal_standing(None, -ALLY_AT), Some(-10.0));
        assert_eq!(personal_standing(None, -PERSONAL_CONTACT_AT), Some(-5.0));
        // Held at +5, it survives a dip under the entry bar.
        assert_eq!(
            personal_standing(Some(5.0), PERSONAL_CONTACT_AT - 1.0),
            Some(5.0)
        );
        assert_eq!(personal_standing(Some(5.0), PERSONAL_LEAVE - 0.1), None);
        // Held at +10, it drops to +5 only under the band.
        assert_eq!(
            personal_standing(Some(10.0), ALLY_AT - society::BAND),
            Some(10.0)
        );
        assert_eq!(
            personal_standing(Some(10.0), ALLY_AT - society::BAND - 0.1),
            Some(5.0)
        );
        // A sign flip is a fresh entry: no hysteresis across zero.
        assert_eq!(
            personal_standing(Some(5.0), -PERSONAL_LEAVE - 0.5),
            None,
            "a +5 contact on a mildly negative regard goes, it does not turn red"
        );
        assert_eq!(personal_standing(Some(5.0), -ALLY_AT), Some(-10.0));
    }

    #[test]
    fn a_rival_becomes_a_red_contact_and_a_forgotten_one_is_removed() {
        let regards: BTreeMap<i64, f64> = [(A, -(ENEMY_AT.abs() + 1.0)), (B, 0.5)].into();
        let book: BTreeMap<i64, f64> = [(B, 5.0), (77, 10.0)].into();
        let plan = personal_plan(&book, &regards, &BTreeMap::new());
        assert!(plan.contains(&(A, Some(-10.0))), "{plan:?}");
        assert!(plan.contains(&(B, None)), "{plan:?}");
        assert!(
            !plan.iter().any(|(who, _)| *who == 77),
            "a contact with no regard behind it is somebody else's: {plan:?}"
        );
        // Settled: asking again changes nothing.
        let settled: BTreeMap<i64, f64> = [(A, -10.0), (77, 10.0)].into();
        assert!(personal_plan(&settled, &regards, &BTreeMap::new()).is_empty());
    }

    #[test]
    fn a_protected_friend_and_a_group_regard_are_left_alone() {
        let regards: BTreeMap<i64, f64> = [(A, -9.0), (CORP, -9.0), (B, 9.0)].into();
        let book: BTreeMap<i64, f64> = [(A, 3.0)].into();
        let plan = personal_plan(&book, &regards, &[(A, 3.0)].into());
        assert_eq!(plan, vec![(B, Some(10.0))], "{plan:?}");
    }

    #[test]
    fn the_book_is_capped_and_the_strongest_win() {
        let mut regards = BTreeMap::new();
        for i in 0..(MAX_PERSONAL_CONTACTS as i64 + 5) {
            // Stronger as i grows, all above the entry bar.
            regards.insert(90_100_000 + i, PERSONAL_CONTACT_AT + 0.01 * i as f64);
        }
        let plan = personal_plan(&BTreeMap::new(), &regards, &BTreeMap::new());
        assert_eq!(plan.len(), MAX_PERSONAL_CONTACTS);
        assert!(
            plan.iter().all(|(who, _)| *who >= 90_100_005),
            "the five weakest are the ones left out"
        );
    }

    #[test]
    fn a_stranger_is_not_invited_a_friend_after_two_fleet_hours_is() {
        assert!(!may_invite(
            INVITE_AT + 1.0,
            INVITE_FLEET_HOURS - 0.1,
            false
        ));
        assert!(!may_invite(
            INVITE_AT - 0.1,
            INVITE_FLEET_HOURS + 1.0,
            false
        ));
        assert!(may_invite(INVITE_AT, INVITE_FLEET_HOURS, false));
        assert!(!may_invite(INVITE_AT + 3.0, INVITE_FLEET_HOURS, true));
    }

    #[test]
    fn applicants_are_judged_by_their_stance() {
        // Hostile: refused whatever the fit. Wary: only a very good fit.
        assert!(!accepts(Stance5::Hostile, 1.0, true));
        assert!(!accepts(Stance5::Wary, ACCEPT_FIT_WARY - 0.01, true));
        assert!(accepts(Stance5::Wary, ACCEPT_FIT_WARY, true));
        // Friendly: a low fit will do, even where the base bar says no.
        assert!(accepts(Stance5::Friendly, ACCEPT_FIT_FRIEND, false));
        assert!(!accepts(Stance5::Friendly, ACCEPT_FIT_FRIEND - 0.01, true));
        assert!(accepts(Stance5::Allied, ACCEPT_FIT_FRIEND, false));
        // Neutral: the corp's own bar.
        assert!(accepts(Stance5::Neutral, 0.0, true));
        assert!(!accepts(Stance5::Neutral, 1.0, false));
        // The reading of a number.
        assert_eq!(stance_of(ENEMY_AT), Stance5::Hostile);
        assert_eq!(stance_of(WARY_AT), Stance5::Wary);
        assert_eq!(stance_of(0.0), Stance5::Neutral);
        assert_eq!(stance_of(FRIEND_AT), Stance5::Friendly);
        assert_eq!(stance_of(ALLY_AT), Stance5::Allied);
    }

    #[test]
    fn a_human_corp_is_admitted_to_an_alliance_by_its_standing() {
        assert!(alliance_admits_human(ALLY_AT));
        assert!(alliance_admits_human(ALLY_AT - ALLIANCE_SLACK));
        assert!(!alliance_admits_human(ALLY_AT - ALLIANCE_SLACK - 0.1));
    }

    #[test]
    fn a_wary_counterparty_pays_the_premium() {
        let g = counterparty_gate(WARY_AT);
        assert_eq!(g, Gate::Premium(WARY_PREMIUM));
        let ask = g.ask(1000.0, 500.0).expect("sold");
        assert!((ask - 1000.0 * WARY_PREMIUM).abs() < 1e-9);
        // And as a seller to a buyer bot, it must be cheaper by the same.
        let bar = g.bar(0.7).expect("considered");
        assert!((bar - 0.7 / WARY_PREMIUM).abs() < 1e-9);
    }

    #[test]
    fn a_hostile_counterparty_is_refused() {
        let g = counterparty_gate(ENEMY_AT);
        assert_eq!(g, Gate::Refuse);
        assert_eq!(g.ask(1000.0, 0.0), None);
        assert_eq!(g.bar(0.7), None);
    }

    #[test]
    fn an_allied_buyer_gets_the_discount_bounded_by_cost() {
        let g = counterparty_gate(ALLY_AT);
        assert_eq!(g, Gate::Discount(ALLY_DISCOUNT));
        let ask = g.ask(1000.0, 0.0).expect("sold");
        assert!((ask - 1000.0 * ALLY_DISCOUNT).abs() < 1e-9);
        // The floor holds, and the discount never becomes a premium.
        assert_eq!(g.ask(1000.0, 980.0), Some(980.0));
        assert_eq!(g.ask(1000.0, 1200.0), Some(1000.0));
        // A neutral and a friendly counterparty pay the plain ask.
        assert_eq!(counterparty_gate(0.0).ask(1000.0, 0.0), Some(1000.0));
        assert_eq!(counterparty_gate(FRIEND_AT).ask(1000.0, 0.0), Some(1000.0));
    }

    #[test]
    fn a_friends_want_counts_for_more() {
        assert!(friend_need(1.0, FRIEND_AT) > 1.0);
        assert_eq!(friend_need(1.0, 0.0), 1.0);
    }

    fn help(r: f64) -> HelpIn {
        HelpIn {
            r,
            odds: Some(1.0),
            required: 1.0,
            jumps: Some(0),
            busy: false,
            loyalty: 0.0,
        }
    }

    #[test]
    fn a_needbackup_from_a_friend_is_answered_when_odds_allow() {
        assert!(help_decision(&help(HELP_AT)));
        assert!(!help_decision(&help(HELP_AT - 0.1)), "not a friend");
        assert!(!help_decision(&HelpIn {
            busy: true,
            ..help(HELP_AT)
        }));
        assert!(!help_decision(&HelpIn {
            jumps: Some(HELP_JUMPS + 1),
            ..help(HELP_AT)
        }));
        assert!(!help_decision(&HelpIn {
            jumps: None,
            ..help(HELP_AT)
        }));
        // Bad odds stay home, a loyal bot goes at worse odds, and an unseen
        // fight is gone to see.
        let bad = HelpIn {
            odds: Some(0.8),
            ..help(HELP_AT)
        };
        assert!(!help_decision(&bad));
        assert!(help_decision(&HelpIn {
            loyalty: 1.0,
            ..bad
        }));
        assert!(help_decision(&HelpIn {
            odds: None,
            ..help(HELP_AT)
        }));
    }

    /// REVIEW-FIX2: want-to-buy contracts become per-type regard.
    #[test]
    fn review_fix2_friend_wants_take_the_best_issuer_per_type() {
        let l = |issuer, wants: &[(i64, i64)]| crate::contracts::Listing {
            contract_id: 1,
            issuer_id: issuer,
            station_id: 1,
            price: 1.0,
            gives: Vec::new(),
            wants: wants.to_vec(),
            asks_more: true,
        };
        let r = |who: i64| if who == A { FRIEND_AT } else { 0.0 };
        let got = friend_wants_of(&[l(A, &[(7, 1)]), l(B, &[(7, 2), (8, 1)]), l(A, &[])], &r);
        assert_eq!(got.get(&7), Some(&FRIEND_AT));
        assert_eq!(got.get(&8), Some(&0.0));
        assert_eq!(got.len(), 2);
    }
}
