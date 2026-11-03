//! **The calendar** (lane `gap-calendar`, EVEJS-GAP svcA B12): personal, corp
//! and alliance events, their invitees and RSVPs. The wire is
//! `service/calendar.rs`; this is the store and the rules, with no registry.
//!
//! State is the delta kind [`KIND`], so it rides the world file with no
//! store arm:
//!
//! | key | value |
//! | --- | --- |
//! | `ev id` | `owner creator scope date duration title description importance auto deleted modified invitees` |
//! | `resp id char` | `status` |
//! | `next` | `n` |
//!
//! `scope` is 0 personal, 1 corp, 2 alliance; `date` and `modified` are blue
//! FILETIMEs; `duration` is minutes or empty; `auto` is the auto event type
//! or empty; `invitees` is a comma list of character ids. A deleted event
//! keeps its row (the client renders `isDeleted`).
//!
//! Limits and response values are the client's own `appConst` **[V]**
//! (`eveCalendarsvc`): title 40, description 500, invitees 50, 12 months
//! ahead; responses 0 none, 2 declined, 3 undecided, 4 accepted, 5 tentative.
//! Where the client has no key for a limit (description, invitees) we cut
//! the text **[A: the client's own window caps both, so a longer one is a
//! client we do not know; falsifier: a capture of a longer description]**.

use crate::delta::Deltas;

/// The delta kind.
pub const KIND: &str = "calendar";

pub const MAX_TITLE: usize = 40;
pub const MAX_DESCRIPTION: usize = 500;
pub const MAX_INVITEES: usize = 50;
pub const MAX_MONTHS: i64 = 12;

/// Responses (`appConst.eventResponse*`) **[V]**.
pub mod resp {
    pub const NONE: i32 = 0;
    pub const DECLINED: i32 = 2;
    pub const UNDECIDED: i32 = 3;
    pub const ACCEPTED: i32 = 4;
    pub const TENTATIVE: i32 = 5;
}

/// A corp or alliance event needs this corp role bit **[A: our CEO or
/// director carries it, see `Viewer::may_write`; falsifier: a CEO cannot see
/// the Corp tag in the New Event window]**.
pub const ROLE_EVENTS: i64 = 1 << 55;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Personal,
    Corp,
    Alliance,
}

impl Scope {
    fn code(self) -> &'static str {
        match self {
            Scope::Personal => "0",
            Scope::Corp => "1",
            Scope::Alliance => "2",
        }
    }
    fn from_code(s: &str) -> Option<Scope> {
        match s {
            "0" => Some(Scope::Personal),
            "1" => Some(Scope::Corp),
            "2" => Some(Scope::Alliance),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub id: i64,
    /// The character, corporation or alliance the event belongs to.
    pub owner_id: i64,
    pub creator_id: i64,
    pub scope: Scope,
    pub date_time: i64,
    pub duration: Option<i64>,
    pub title: String,
    pub description: String,
    pub importance: i64,
    pub auto_event_type: Option<i64>,
    pub deleted: bool,
    pub modified: i64,
    pub invitees: Vec<i64>,
}

fn opt(s: &str) -> Option<i64> {
    s.parse().ok()
}

fn opt_str(v: Option<i64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_default()
}

impl Event {
    fn row(&self) -> Vec<String> {
        vec![
            self.owner_id.to_string(),
            self.creator_id.to_string(),
            self.scope.code().to_owned(),
            self.date_time.to_string(),
            opt_str(self.duration),
            self.title.clone(),
            self.description.clone(),
            self.importance.to_string(),
            opt_str(self.auto_event_type),
            u8::from(self.deleted).to_string(),
            self.modified.to_string(),
            self.invitees
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(","),
        ]
    }

    fn parse(id: i64, v: &[String]) -> Option<Event> {
        let f = |i: usize| v.get(i).map(String::as_str).unwrap_or("");
        Some(Event {
            id,
            owner_id: f(0).parse().ok()?,
            creator_id: f(1).parse().ok()?,
            scope: Scope::from_code(f(2))?,
            date_time: f(3).parse().ok()?,
            duration: opt(f(4)),
            title: f(5).to_owned(),
            description: f(6).to_owned(),
            importance: f(7).parse().unwrap_or(0),
            auto_event_type: opt(f(8)),
            deleted: f(9) == "1",
            modified: f(10).parse().unwrap_or(0),
            invitees: f(11).split(',').filter_map(opt).collect(),
        })
    }
}

fn ev_key(id: i64) -> Vec<String> {
    vec!["ev".to_owned(), id.to_string()]
}

fn resp_key(id: i64, ch: i64) -> Vec<String> {
    vec!["resp".to_owned(), id.to_string(), ch.to_string()]
}

pub fn get(d: &Deltas, id: i64) -> Option<Event> {
    Event::parse(id, d.of(KIND).get(&ev_key(id))?)
}

pub fn put(d: &mut Deltas, e: &Event) {
    d.put(KIND, ev_key(e.id), e.row());
}

/// Every event, in id-string order.
pub fn all(d: &Deltas) -> Vec<Event> {
    d.of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 2 && k[0] == "ev")
        .filter_map(|(k, v)| Event::parse(k[1].parse().ok()?, v))
        .collect()
}

/// Take the next event id (1, 2, ...).
pub fn alloc_id(d: &mut Deltas) -> i64 {
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

pub fn status(d: &Deltas, id: i64, ch: i64) -> i32 {
    d.of(KIND)
        .get(&resp_key(id, ch))
        .and_then(|v| v.first())
        .and_then(|s| s.parse().ok())
        .unwrap_or(resp::NONE)
}

pub fn set_status(d: &mut Deltas, id: i64, ch: i64, s: i32) {
    d.put(KIND, resp_key(id, ch), vec![s.to_string()]);
}

/// `(character, status)` for one event, in id-string order.
pub fn responses_to(d: &Deltas, id: i64) -> Vec<(i64, i32)> {
    d.of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 3 && k[0] == "resp" && k[1] == id.to_string())
        .filter_map(|(k, v)| Some((k[2].parse().ok()?, v.first()?.parse().ok()?)))
        .collect()
}

/// `(event, status)` for one character.
pub fn responses_of(d: &Deltas, ch: i64) -> Vec<(i64, i32)> {
    d.of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 3 && k[0] == "resp" && k[2] == ch.to_string())
        .filter_map(|(k, v)| Some((k[1].parse().ok()?, v.first()?.parse().ok()?)))
        .collect()
}

/// A change in a date moves every responder but the creator who had
/// answered (3, 4 or 5) back to undecided (the client's own
/// `OnEditCalendarEvent` does the same locally) **[V]**.
pub fn reset_responses(d: &mut Deltas, e: &Event) {
    for (ch, s) in responses_to(d, e.id) {
        if ch != e.creator_id && matches!(s, resp::UNDECIDED..=resp::TENTATIVE) {
            set_status(d, e.id, ch, resp::UNDECIDED);
        }
    }
}

/// Who is asking: the character and the corp and alliance they belong to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewer {
    pub character: i64,
    pub corp: Option<i64>,
    pub alliance: Option<i64>,
}

impl Viewer {
    /// The client's `GetEventFlag` visibility: own, invited, own corp, own
    /// alliance.
    pub fn can_see(&self, e: &Event) -> bool {
        match e.scope {
            Scope::Personal => e.owner_id == self.character || e.invitees.contains(&self.character),
            Scope::Corp => self.corp == Some(e.owner_id),
            Scope::Alliance => self.alliance == Some(e.owner_id),
        }
    }
}

/// Why a call is refused, as the client's message key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Refusal {
    NoSuchEvent,
    CannotModify,
    CannotEditDeleted,
    CannotEditPast,
    MustSpecifyTitle,
    TitleTooLong,
    CannotPlanThePast,
    TooFarIntoFuture,
}

impl Refusal {
    pub fn key(self) -> &'static str {
        match self {
            Refusal::NoSuchEvent => "CalendarNoSuchEvent",
            Refusal::CannotModify => "CalendarEventCannotModify",
            Refusal::CannotEditDeleted => "CalendarCannotEditDeleted",
            Refusal::CannotEditPast => "CalendarCannotEditPast",
            Refusal::MustSpecifyTitle => "CalendarEventMustSpecifyTitle",
            Refusal::TitleTooLong => "CalendarTitleTooLong",
            Refusal::CannotPlanThePast => "CalendarCannotPlanThePast",
            Refusal::TooFarIntoFuture => "CalendarTooFarIntoFuture",
        }
    }
}

/// The checks a create and an edit share. `now` is a FILETIME. Returns the
/// trimmed title.
pub fn validate(title: &str, date_time: i64, now: i64) -> Result<String, Refusal> {
    let t = title.trim();
    if t.is_empty() {
        return Err(Refusal::MustSpecifyTitle);
    }
    if t.chars().count() > MAX_TITLE {
        return Err(Refusal::TitleTooLong);
    }
    if date_time <= now {
        return Err(Refusal::CannotPlanThePast);
    }
    let months = |ft: i64| {
        let (y, m, _) = crate::ledger::civil_from_filetime(ft);
        y * 12 + m
    };
    if months(date_time) - months(now) > MAX_MONTHS {
        return Err(Refusal::TooFarIntoFuture);
    }
    Ok(t.to_owned())
}

/// Whether the UTC `(year, month)` of the event is the one asked for.
pub fn in_month(e: &Event, year: i64, month: i64) -> bool {
    let (y, m, _) = crate::ledger::civil_from_filetime(e.date_time);
    (y, m) == (year, month)
}

/// Cut a description to [`MAX_DESCRIPTION`] characters.
pub fn clip_description(s: &str) -> String {
    s.chars().take(MAX_DESCRIPTION).collect()
}

/// Dedupe `ids`, drop `creator`, keep the first [`MAX_INVITEES`].
pub fn clean_invitees(ids: &[i64], creator: i64) -> Vec<i64> {
    let mut out: Vec<i64> = Vec::new();
    for id in ids {
        if *id != creator && !out.contains(id) && out.len() < MAX_INVITEES {
            out.push(*id);
        }
    }
    out
}
