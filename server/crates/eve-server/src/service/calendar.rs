//! **The calendar wire** (lane `gap-calendar`): `calendarProxy` (reads) and
//! `calendarMgr` (writes), the two services `eveCalendarsvc` calls **[V]**.
//! The store and the rules are `crate::calendar`.
//!
//! | call | answer |
//! | --- | --- |
//! | `calendarProxy.GetEventList(month, year)` | `[ [KeyVal row, ...] ]`, one inner list |
//! | `calendarProxy.GetEventDetails(eventID, ownerID)` | `KeyVal(eventText, creatorID)` |
//! | `calendarMgr.CreatePersonalEvent(dateTime, duration, title, description, important, invitees)` | the event id |
//! | `calendarMgr.Create{Corporation,Alliance}Event(dateTime, duration, title, description, important)` | the event id |
//! | `calendarMgr.Edit{Personal,Corporation,Alliance}Event(eventID, oldDateTime, dateTime, duration, title, description, important)` | `None` |
//! | `calendarMgr.DeleteEvent(eventID, ownerID)` | `None` (a soft delete) |
//! | `calendarMgr.SendEventResponse(eventID, ownerID, response)` | `None` |
//! | `calendarMgr.UpdateEventParticipants(eventID, charsToAdd, charsToRemove)` | `None` |
//! | `calendarMgr.GetResponsesForCharacter()` | `[KeyVal(eventID, status)]` |
//! | `calendarMgr.GetResponsesToEvent(eventID, ownerID)` | `[KeyVal(characterID, status)]` |
//!
//! Pushes go to the people who can see the event, as character ids (the
//! transport writes to those online). `OnEventResponseByExternal` goes to the
//! **responder only**: it makes the receiver's own RSVP what it says, so a
//! broadcast would overwrite everybody's answer with one person's.

use super::*;
use crate::calendar::{self as cal, Event, Refusal, Scope, Viewer, resp};

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &["calendarProxy", "calendarMgr"];

fn refuse(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
}

fn refusal(r: Refusal) -> ServiceError {
    let args = match r {
        Refusal::TooFarIntoFuture => vec![("numMonths", PyValue::Int(cal::MAX_MONTHS))],
        _ => vec![],
    };
    refuse(r.key(), args)
}

/// A calendar push: the event, its arguments, and who hears it.
#[derive(Debug, Clone, PartialEq)]
pub struct CalendarPush {
    pub event: &'static str,
    pub args: Vec<PyValue>,
    pub to: Vec<i64>,
}

impl crate::notify::Push for CalendarPush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (self.event, self.args.clone())
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

fn tell(reg: &mut Registry, to: Vec<i64>, event: &'static str, args: Vec<PyValue>) {
    if !to.is_empty() {
        reg.schedule(std::time::Duration::ZERO, CalendarPush { event, args, to });
    }
}

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        PyValue::Bool(b) => Some(i64::from(*b)),
        PyValue::Real(r) => Some(*r as i64),
        _ => None,
    }
}

fn text(v: Option<&PyValue>) -> String {
    match v {
        Some(PyValue::Unicode(s)) => s.clone(),
        Some(PyValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
        _ => String::new(),
    }
}

fn ids(v: Option<&PyValue>) -> Vec<i64> {
    match v {
        Some(PyValue::List(v) | PyValue::Tuple(v)) => {
            v.iter().filter_map(|x| int(Some(x))).collect()
        }
        _ => Vec::new(),
    }
}

fn arg<'a>(req: &'a Request<'_>, i: usize, name: &str) -> Option<&'a PyValue> {
    req.args
        .get(i)
        .or_else(|| crate::pyutil::pairs_get(req.kwargs, name.as_bytes()))
}

fn viewer(reg: &Registry, me: i64) -> Viewer {
    let corp = reg.corporations().membership(me).map(|m| m.corporation_id);
    Viewer {
        character: me,
        corp,
        alliance: corp.and_then(|c| crate::alliance::of_corp(reg.deltas(), c)),
    }
}

/// Whether `me` may write for their corp or alliance: the CEO, a director,
/// or a holder of [`cal::ROLE_EVENTS`] **[A]**.
fn may_write_shared(reg: &Registry, me: i64) -> bool {
    let Some(m) = reg.corporations().membership(me) else {
        return false;
    };
    let roles = m.roles | m.roles_at_other;
    roles & (cal::ROLE_EVENTS | crate::corp_roles::CORP_ROLE_DIRECTOR) != 0
        || reg
            .corporations()
            .get(m.corporation_id)
            .is_some_and(|c| c.ceo_id == me)
}

fn may_modify(reg: &Registry, me: i64, e: &Event) -> bool {
    match e.scope {
        Scope::Personal => e.owner_id == me,
        _ => viewer(reg, me).can_see(e) && may_write_shared(reg, me),
    }
}

/// The characters who can see `e`.
fn audience(reg: &Registry, e: &Event) -> Vec<i64> {
    match e.scope {
        Scope::Personal => {
            let mut v = vec![e.owner_id];
            v.extend(e.invitees.iter().copied());
            v
        }
        Scope::Corp => reg.corporations().members_of(e.owner_id),
        Scope::Alliance => {
            let all = crate::alliance::Alliances::load(reg.deltas());
            all.members_of(e.owner_id)
                .iter()
                .flat_map(|(corp, _)| reg.corporations().members_of(*corp))
                .collect()
        }
    }
}

fn opt_py(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

fn row(e: &Event) -> PyValue {
    crate::pyutil::keyval(vec![
        ("eventID", PyValue::Int(e.id)),
        ("ownerID", PyValue::Int(e.owner_id)),
        ("eventDateTime", PyValue::Int(e.date_time)),
        ("eventDuration", opt_py(e.duration)),
        ("eventTitle", PyValue::Unicode(e.title.clone())),
        ("importance", PyValue::Int(e.importance)),
        ("dateModified", PyValue::Int(e.modified)),
        ("isDeleted", PyValue::Bool(e.deleted)),
        ("autoEventType", opt_py(e.auto_event_type)),
    ])
}

/// `OnNewCalendarEvent(eventID, ownerID, eventDateTime, eventDuration,
/// eventTitle, importance, autoEventType, doBlink)` **[V]**.
fn new_args(e: &Event) -> Vec<PyValue> {
    vec![
        PyValue::Int(e.id),
        PyValue::Int(e.owner_id),
        PyValue::Int(e.date_time),
        opt_py(e.duration),
        PyValue::Unicode(e.title.clone()),
        PyValue::Int(e.importance),
        opt_py(e.auto_event_type),
        PyValue::Bool(true),
    ]
}

fn visible(reg: &Registry, me: i64, id: i64) -> Result<Event, ServiceError> {
    cal::get(reg.deltas(), id)
        .filter(|e| viewer(reg, me).can_see(e))
        .ok_or_else(|| refusal(Refusal::NoSuchEvent))
}

fn create(
    reg: &mut Registry,
    req: &Request<'_>,
    me: i64,
    scope: Scope,
) -> Result<PyValue, ServiceError> {
    let now = crate::pyutil::filetime_now();
    let date_time = int(arg(req, 0, "dateTime")).unwrap_or(0);
    let title = cal::validate(&text(arg(req, 2, "title")), date_time, now).map_err(refusal)?;
    let v = viewer(reg, me);
    let owner_id = match scope {
        Scope::Personal => me,
        Scope::Corp => v.corp.ok_or_else(|| refusal(Refusal::CannotModify))?,
        Scope::Alliance => v.alliance.ok_or_else(|| refusal(Refusal::CannotModify))?,
    };
    if scope != Scope::Personal && !may_write_shared(reg, me) {
        return Err(refusal(Refusal::CannotModify));
    }
    let invitees = if scope == Scope::Personal {
        let wanted = cal::clean_invitees(&ids(arg(req, 5, "invitees")), me);
        wanted
            .into_iter()
            .filter(|c| reg.character(*c).is_some())
            .collect()
    } else {
        Vec::new()
    };
    let e = Event {
        id: cal::alloc_id(reg.deltas_mut()),
        owner_id,
        creator_id: me,
        scope,
        date_time,
        duration: int(arg(req, 1, "duration")).filter(|d| *d > 0),
        title,
        description: cal::clip_description(&text(arg(req, 3, "description"))),
        importance: i64::from(int(arg(req, 4, "important")).unwrap_or(0) != 0),
        auto_event_type: None,
        deleted: false,
        modified: now,
        invitees,
    };
    cal::put(reg.deltas_mut(), &e);
    if scope == Scope::Personal {
        cal::set_status(reg.deltas_mut(), e.id, me, resp::ACCEPTED);
    }
    for c in &e.invitees {
        cal::set_status(reg.deltas_mut(), e.id, *c, resp::UNDECIDED);
    }
    // A personal event is added locally by its creator; a shared one is not,
    // so its creator hears it with everybody else.
    let to = match scope {
        Scope::Personal => e.invitees.clone(),
        _ => audience(reg, &e),
    };
    tell(reg, to, "OnNewCalendarEvent", new_args(&e));
    Ok(PyValue::Int(e.id))
}

fn edit(reg: &mut Registry, req: &Request<'_>, me: i64) -> Result<PyValue, ServiceError> {
    let id = int(arg(req, 0, "eventID")).unwrap_or(0);
    let mut e = visible(reg, me, id)?;
    if !may_modify(reg, me, &e) {
        return Err(refusal(Refusal::CannotModify));
    }
    if e.deleted {
        return Err(refusal(Refusal::CannotEditDeleted));
    }
    let now = crate::pyutil::filetime_now();
    if e.date_time <= now {
        return Err(refusal(Refusal::CannotEditPast));
    }
    let date_time = int(arg(req, 2, "dateTime")).unwrap_or(e.date_time);
    let title = cal::validate(&text(arg(req, 4, "title")), date_time, now).map_err(refusal)?;
    let old = e.date_time;
    e.date_time = date_time;
    e.duration = int(arg(req, 3, "duration")).filter(|d| *d > 0);
    e.title = title;
    e.description = cal::clip_description(&text(arg(req, 5, "description")));
    e.importance = i64::from(int(arg(req, 6, "important")).unwrap_or(0) != 0);
    e.modified = now;
    cal::put(reg.deltas_mut(), &e);
    if old != date_time {
        cal::reset_responses(reg.deltas_mut(), &e);
    }
    let args = vec![
        PyValue::Int(e.id),
        PyValue::Int(e.owner_id),
        PyValue::Int(old),
        PyValue::Int(e.date_time),
        opt_py(e.duration),
        PyValue::Unicode(e.title.clone()),
        PyValue::Int(e.importance),
        opt_py(e.auto_event_type),
    ];
    let to = audience(reg, &e);
    tell(reg, to, "OnEditCalendarEvent", args);
    Ok(PyValue::None)
}

fn delete(reg: &mut Registry, req: &Request<'_>, me: i64) -> Result<PyValue, ServiceError> {
    let id = int(arg(req, 0, "eventID")).unwrap_or(0);
    let mut e = visible(reg, me, id)?;
    if !may_modify(reg, me, &e) {
        return Err(refusal(Refusal::CannotModify));
    }
    if !e.deleted {
        e.deleted = true;
        e.modified = crate::pyutil::filetime_now();
        cal::put(reg.deltas_mut(), &e);
        let to = audience(reg, &e);
        tell(
            reg,
            to,
            "OnRemoveCalendarEvent",
            vec![
                PyValue::Int(e.id),
                PyValue::Int(e.date_time),
                PyValue::Bool(true),
            ],
        );
    }
    Ok(PyValue::None)
}

fn respond(reg: &mut Registry, req: &Request<'_>, me: i64) -> Result<PyValue, ServiceError> {
    let id = int(arg(req, 0, "eventID")).unwrap_or(0);
    let e = visible(reg, me, id)?;
    let status = int(arg(req, 2, "response")).unwrap_or(0);
    if !(2..=5).contains(&status) {
        return Ok(PyValue::None);
    }
    cal::set_status(reg.deltas_mut(), id, me, status as i32);
    // For the responder's other sessions only: see the module doc.
    let kv = crate::pyutil::keyval(vec![
        ("ownerID", PyValue::Int(e.owner_id)),
        ("flag", PyValue::Int(e.owner_id)),
        ("eventDateTime", PyValue::Int(e.date_time)),
    ]);
    tell(
        reg,
        vec![me],
        "OnEventResponseByExternal",
        vec![PyValue::Int(id), kv, PyValue::Int(status)],
    );
    Ok(PyValue::None)
}

fn participants(reg: &mut Registry, req: &Request<'_>, me: i64) -> Result<PyValue, ServiceError> {
    let id = int(arg(req, 0, "eventID")).unwrap_or(0);
    let mut e = visible(reg, me, id)?;
    if e.scope != Scope::Personal || e.owner_id != me {
        return Err(refusal(Refusal::CannotModify));
    }
    if e.deleted {
        return Err(refusal(Refusal::CannotEditDeleted));
    }
    let remove = ids(arg(req, 2, "charsToRemove"));
    let mut removed = Vec::new();
    for c in remove {
        if let Some(at) = e.invitees.iter().position(|x| *x == c) {
            e.invitees.remove(at);
            removed.push(c);
        }
    }
    let mut added = Vec::new();
    for c in ids(arg(req, 1, "charsToAdd")) {
        if c != me
            && !e.invitees.contains(&c)
            && e.invitees.len() < cal::MAX_INVITEES
            && reg.character(c).is_some()
        {
            e.invitees.push(c);
            added.push(c);
        }
    }
    cal::put(reg.deltas_mut(), &e);
    for c in &removed {
        cal::set_status(reg.deltas_mut(), id, *c, resp::NONE);
    }
    for c in &added {
        cal::set_status(reg.deltas_mut(), id, *c, resp::UNDECIDED);
    }
    tell(reg, added, "OnNewCalendarEvent", new_args(&e));
    tell(
        reg,
        removed,
        "OnRemoveCalendarEvent",
        vec![
            PyValue::Int(e.id),
            PyValue::Int(e.date_time),
            PyValue::Bool(false),
        ],
    );
    Ok(PyValue::None)
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(me) = req.actor else {
        return match (req.service, req.method) {
            ("calendarProxy", "GetEventList") => Ok(PyValue::List(vec![])),
            _ => Err(reg.no_method(req)),
        };
    };
    match (req.service, req.method) {
        ("calendarProxy", "GetEventList") => {
            let month = int(arg(req, 0, "month")).unwrap_or(0);
            let year = int(arg(req, 1, "year")).unwrap_or(0);
            let v = viewer(reg, me);
            let rows = cal::all(reg.deltas())
                .iter()
                .filter(|e| v.can_see(e) && cal::in_month(e, year, month))
                .map(row)
                .collect();
            // A list of row lists: the client iterates both.
            Ok(PyValue::List(vec![PyValue::List(rows)]))
        }
        ("calendarProxy", "GetEventDetails") => {
            let e = visible(reg, me, int(arg(req, 0, "eventID")).unwrap_or(0))?;
            Ok(crate::pyutil::keyval(vec![
                ("eventText", PyValue::Unicode(e.description)),
                ("creatorID", PyValue::Int(e.creator_id)),
            ]))
        }
        ("calendarMgr", "CreatePersonalEvent") => create(reg, req, me, Scope::Personal),
        ("calendarMgr", "CreateCorporationEvent") => create(reg, req, me, Scope::Corp),
        ("calendarMgr", "CreateAllianceEvent") => create(reg, req, me, Scope::Alliance),
        ("calendarMgr", "EditPersonalEvent" | "EditCorporationEvent" | "EditAllianceEvent") => {
            edit(reg, req, me)
        }
        ("calendarMgr", "DeleteEvent") => delete(reg, req, me),
        ("calendarMgr", "SendEventResponse") => respond(reg, req, me),
        ("calendarMgr", "UpdateEventParticipants") => participants(reg, req, me),
        ("calendarMgr", "GetResponsesForCharacter") => {
            let v = viewer(reg, me);
            let rows = cal::responses_of(reg.deltas(), me)
                .into_iter()
                .filter(|(_, s)| *s != resp::NONE)
                .filter(|(id, _)| cal::get(reg.deltas(), *id).is_some_and(|e| v.can_see(&e)))
                .map(|(id, s)| {
                    crate::pyutil::keyval(vec![
                        ("eventID", PyValue::Int(id)),
                        ("status", PyValue::Int(i64::from(s))),
                    ])
                })
                .collect();
            Ok(PyValue::List(rows))
        }
        ("calendarMgr", "GetResponsesToEvent") => {
            let id = int(arg(req, 0, "eventID")).unwrap_or(0);
            visible(reg, me, id)?;
            let rows = cal::responses_to(reg.deltas(), id)
                .into_iter()
                .filter(|(_, s)| *s != resp::NONE)
                .map(|(c, s)| {
                    crate::pyutil::keyval(vec![
                        ("characterID", PyValue::Int(c)),
                        ("status", PyValue::Int(i64::from(s))),
                    ])
                })
                .collect();
            Ok(PyValue::List(rows))
        }
        _ => Err(reg.no_method(req)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids: nothing here is a client row.
    const ALICE: i64 = 90_000_701;
    const BOB: i64 = 90_000_702;
    const CAROL: i64 = 90_000_703;
    const DAY: i64 = 864_000_000_000;

    fn pilot(id: i64, name: &str) -> crate::model::Character {
        crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: id + 100_000_000,
            ship_type_id: 990_640_001,
            ..crate::model::Character::default()
        }
    }

    fn enrol(reg: &mut Registry, who: i64, corp: i64, roles: i64) {
        let (who, corp, roles) = (who.to_string(), corp.to_string(), roles.to_string());
        reg.corporations
            .parse_record(&["corpmember", &who, &corp, &roles, "1"])
            .expect("a membership record");
    }

    /// ALICE (corp 10, role bit), CAROL (corp 10, no role), BOB (nobody's).
    fn world() -> Registry {
        crate::test_clock::freeze(std::time::Duration::from_secs(1_800_000_000));
        let mut reg = crate::testkit::registry_with(|c| {
            c.characters = vec![
                pilot(ALICE, "Alice"),
                pilot(BOB, "Bob"),
                pilot(CAROL, "Carol"),
            ];
        });
        enrol(&mut reg, ALICE, 10, cal::ROLE_EVENTS);
        enrol(&mut reg, CAROL, 10, 0);
        reg
    }

    fn call(
        reg: &mut Registry,
        who: i64,
        service: &str,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        })
    }

    fn i(n: i64) -> PyValue {
        PyValue::Int(n)
    }

    fn u(s: &str) -> PyValue {
        PyValue::Unicode(s.to_owned())
    }

    fn key_of(e: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            panic!("not a raised exception: {e:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("header")
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("args")
        };
        match &args[0] {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            other => panic!("the key is a str: {other:?}"),
        }
    }

    fn attr<'a>(v: &'a PyValue, name: &str) -> &'a PyValue {
        let PyValue::Instance { state, .. } = v else {
            panic!("not a KeyVal: {v:?}")
        };
        state
            .dict_as_kv()
            .and_then(|kv| {
                kv.into_iter().find_map(|(k, val)| {
                    (crate::pyutil::as_bytes(k) == Some(name.as_bytes())).then_some(val)
                })
            })
            .unwrap_or_else(|| panic!("no {name} on {v:?}"))
    }

    fn now() -> i64 {
        crate::pyutil::filetime_now()
    }

    fn personal(reg: &mut Registry, who: i64, when: i64, title: &str, invitees: &[i64]) -> i64 {
        let inv = PyValue::List(invitees.iter().map(|c| i(*c)).collect());
        let got = call(
            reg,
            who,
            "calendarMgr",
            "CreatePersonalEvent",
            &[i(when), i(60), u(title), u(""), i(0), inv],
        )
        .expect("created");
        let PyValue::Int(id) = got else {
            panic!("an id")
        };
        id
    }

    fn pushes(reg: &mut Registry) -> Vec<(&'static str, Vec<i64>)> {
        reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(1))
            .iter()
            .map(|p| (p.to_push().0, p.audience().unwrap_or_default()))
            .collect()
    }

    fn month_of(when: i64) -> (i64, i64) {
        let (y, m, _) = crate::ledger::civil_from_filetime(when);
        (m, y)
    }

    fn list(reg: &mut Registry, who: i64, when: i64) -> Vec<PyValue> {
        let (m, y) = month_of(when);
        let PyValue::List(outer) =
            call(reg, who, "calendarProxy", "GetEventList", &[i(m), i(y)]).expect("answered")
        else {
            panic!("a list")
        };
        outer
            .into_iter()
            .flat_map(|inner| match inner {
                PyValue::List(rows) => rows,
                _ => vec![],
            })
            .collect()
    }

    fn statuses(reg: &mut Registry, who: i64, id: i64, owner: i64) -> Vec<(i64, i64)> {
        let PyValue::List(rows) = call(
            reg,
            who,
            "calendarMgr",
            "GetResponsesToEvent",
            &[i(id), i(owner)],
        )
        .expect("answered") else {
            panic!("a list")
        };
        rows.iter()
            .map(|r| match (attr(r, "characterID"), attr(r, "status")) {
                (PyValue::Int(c), PyValue::Int(s)) => (*c, *s),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    /// (a) A personal event with an invitee: the invitee sees it, the
    /// stranger does not, and only the invitee hears it.
    #[test]
    fn a_personal_event_reaches_its_invitees_only() {
        let mut reg = world();
        let when = now() + DAY;
        let id = personal(&mut reg, ALICE, when, "Op", &[BOB]);
        assert_eq!(pushes(&mut reg), vec![("OnNewCalendarEvent", vec![BOB])]);
        let bob = list(&mut reg, BOB, when);
        assert_eq!(bob.len(), 1);
        assert_eq!(attr(&bob[0], "ownerID"), &i(ALICE));
        assert_eq!(attr(&bob[0], "eventID"), &i(id));
        assert_eq!(list(&mut reg, ALICE, when).len(), 1);
        assert!(list(&mut reg, CAROL, when).is_empty());
        let e = call(
            &mut reg,
            CAROL,
            "calendarProxy",
            "GetEventDetails",
            &[i(id), i(ALICE)],
        )
        .expect_err("not hers");
        assert_eq!(key_of(&e), "CalendarNoSuchEvent");
    }

    /// (b) An RSVP shows to the owner, and the external-response push goes to
    /// the responder alone.
    #[test]
    fn an_rsvp_is_seen_by_the_owner_and_not_broadcast() {
        let mut reg = world();
        let id = personal(&mut reg, ALICE, now() + DAY, "Op", &[BOB]);
        let _ = pushes(&mut reg);
        call(
            &mut reg,
            BOB,
            "calendarMgr",
            "SendEventResponse",
            &[i(id), i(ALICE), i(4)],
        )
        .expect("answered");
        assert_eq!(
            pushes(&mut reg),
            vec![("OnEventResponseByExternal", vec![BOB])]
        );
        let got = statuses(&mut reg, ALICE, id, ALICE);
        assert!(
            got.contains(&(BOB, 4)) && got.contains(&(ALICE, 4)),
            "{got:?}"
        );
        // A stranger's answer is refused; a bad value is ignored.
        let e = call(
            &mut reg,
            CAROL,
            "calendarMgr",
            "SendEventResponse",
            &[i(id), i(ALICE), i(4)],
        )
        .expect_err("not invited");
        assert_eq!(key_of(&e), "CalendarNoSuchEvent");
        call(
            &mut reg,
            BOB,
            "calendarMgr",
            "SendEventResponse",
            &[i(id), i(ALICE), i(9)],
        )
        .expect("ignored");
        assert!(statuses(&mut reg, ALICE, id, ALICE).contains(&(BOB, 4)));
    }

    /// (c) A new date resets the others to undecided and tells the viewers;
    /// a deleted event cannot be edited.
    #[test]
    fn a_new_date_resets_responses() {
        let mut reg = world();
        let when = now() + DAY;
        let id = personal(&mut reg, ALICE, when, "Op", &[BOB]);
        call(
            &mut reg,
            BOB,
            "calendarMgr",
            "SendEventResponse",
            &[i(id), i(ALICE), i(4)],
        )
        .expect("answered");
        let _ = pushes(&mut reg);
        let edit = |reg: &mut Registry, date: i64| {
            call(
                reg,
                ALICE,
                "calendarMgr",
                "EditPersonalEvent",
                &[i(id), i(when), i(date), i(60), u("Op 2"), u(""), i(1)],
            )
        };
        edit(&mut reg, when + DAY).expect("edited");
        let got = statuses(&mut reg, ALICE, id, ALICE);
        assert!(
            got.contains(&(BOB, 3)) && got.contains(&(ALICE, 4)),
            "{got:?}"
        );
        let p = pushes(&mut reg);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].0, "OnEditCalendarEvent");
        // Bob may not edit; the past is refused; a deleted event is final.
        let e = call(
            &mut reg,
            BOB,
            "calendarMgr",
            "EditPersonalEvent",
            &[i(id), i(when), i(when), i(60), u("x"), u(""), i(0)],
        )
        .expect_err("not his");
        assert_eq!(key_of(&e), "CalendarEventCannotModify");
        let e = edit(&mut reg, now() - DAY).expect_err("the past");
        assert_eq!(key_of(&e), "CalendarCannotPlanThePast");
        call(
            &mut reg,
            ALICE,
            "calendarMgr",
            "DeleteEvent",
            &[i(id), i(ALICE)],
        )
        .expect("deleted");
        let p = pushes(&mut reg);
        assert_eq!(p[0].0, "OnRemoveCalendarEvent");
        let e = edit(&mut reg, when + DAY).expect_err("deleted");
        assert_eq!(key_of(&e), "CalendarCannotEditDeleted");
        // The row stays, struck through.
        let rows = list(&mut reg, BOB, when + DAY);
        assert_eq!(attr(&rows[0], "isDeleted"), &PyValue::Bool(true));
    }

    /// (d) A corp event needs the role; its push reaches every member, the
    /// creator included.
    #[test]
    fn a_corp_event_needs_the_role_and_reaches_the_corp() {
        let mut reg = world();
        let when = now() + DAY;
        let args = [i(when), i(60), u("Corp op"), u(""), i(0)];
        let e = call(
            &mut reg,
            CAROL,
            "calendarMgr",
            "CreateCorporationEvent",
            &args,
        )
        .expect_err("no role");
        assert_eq!(key_of(&e), "CalendarEventCannotModify");
        call(
            &mut reg,
            ALICE,
            "calendarMgr",
            "CreateCorporationEvent",
            &args,
        )
        .expect("created");
        let p = pushes(&mut reg);
        assert_eq!(p.len(), 1);
        let mut to = p[0].1.clone();
        to.sort_unstable();
        assert_eq!(to, vec![ALICE, CAROL]);
        assert_eq!(list(&mut reg, CAROL, when).len(), 1);
        assert!(list(&mut reg, BOB, when).is_empty());
    }

    /// (e) The client's own validation keys.
    #[test]
    fn the_client_keys_come_back() {
        let mut reg = world();
        let when = now() + DAY;
        let new = |reg: &mut Registry, date: i64, title: &str| {
            call(
                reg,
                ALICE,
                "calendarMgr",
                "CreatePersonalEvent",
                &[i(date), i(60), u(title), u(""), i(0), PyValue::List(vec![])],
            )
            .expect_err("refused")
        };
        assert_eq!(
            key_of(&new(&mut reg, when, &"x".repeat(41))),
            "CalendarTitleTooLong"
        );
        assert_eq!(
            key_of(&new(&mut reg, when, "  ")),
            "CalendarEventMustSpecifyTitle"
        );
        assert_eq!(
            key_of(&new(&mut reg, now() - DAY, "Op")),
            "CalendarCannotPlanThePast"
        );
        assert_eq!(
            key_of(&new(&mut reg, now() + 400 * DAY, "Op")),
            "CalendarTooFarIntoFuture"
        );
    }

    /// Participants can be added and removed; the removed hear it.
    #[test]
    fn participants_come_and_go() {
        let mut reg = world();
        let when = now() + DAY;
        let id = personal(&mut reg, ALICE, when, "Op", &[BOB]);
        let _ = pushes(&mut reg);
        let add = PyValue::List(vec![i(CAROL), i(ALICE)]);
        let gone = PyValue::List(vec![i(BOB)]);
        call(
            &mut reg,
            ALICE,
            "calendarMgr",
            "UpdateEventParticipants",
            &[i(id), add, gone],
        )
        .expect("updated");
        assert_eq!(
            pushes(&mut reg),
            vec![
                ("OnNewCalendarEvent", vec![CAROL]),
                ("OnRemoveCalendarEvent", vec![BOB])
            ]
        );
        assert!(list(&mut reg, BOB, when).is_empty());
        assert_eq!(list(&mut reg, CAROL, when).len(), 1);
        let PyValue::List(mine) = call(
            &mut reg,
            CAROL,
            "calendarMgr",
            "GetResponsesForCharacter",
            &[],
        )
        .unwrap() else {
            panic!("a list")
        };
        assert_eq!(attr(&mine[0], "status"), &i(3));
    }

    /// (f) Events, responses and the id counter survive a restart.
    #[test]
    fn a_restart_keeps_events_responses_and_the_counter() {
        let mut reg = world();
        let when = now() + DAY;
        let id = personal(&mut reg, ALICE, when, "Op\twith\ttabs", &[BOB]);
        call(
            &mut reg,
            BOB,
            "calendarMgr",
            "SendEventResponse",
            &[i(id), i(ALICE), i(5)],
        )
        .expect("answered");
        use crate::store::Store;
        let dir = crate::testkit::scratch_dir("calendar-restart");
        let files = crate::store::FileStore::new(dir.join("world.dat"));
        files.save(&reg.to_snapshot()).expect("saves");
        let snap = files.load().expect("loads").expect("a world");
        let mut again = world();
        again.apply_snapshot(snap);
        let rows = list(&mut again, BOB, when);
        assert_eq!(attr(&rows[0], "eventTitle"), &u("Op\twith\ttabs"));
        assert!(statuses(&mut again, ALICE, id, ALICE).contains(&(BOB, 5)));
        let next = personal(&mut again, ALICE, when, "Next", &[]);
        assert_eq!(next, id + 1);
    }
}
