//! **The medals wire** (lane `soc-medals`, #166): the `corporationSvc` calls
//! `services/medals.py` makes, answered from `crate::corp_medals`. The
//! method names are routed here by `service/corp.rs`'s `corporationSvc` arm.
//!
//! | call | answer |
//! | --- | --- |
//! | `CreateMedal(title, description, graphics[, True])` | `None`; pushes `OnCorporationMedalAdded(medalID)` to the corp |
//! | `GetAllCorpMedals(corpID)` | `[medals, graphics]`: a list of `KeyVal` rows and a `CRowset` |
//! | `GetMedalsReceived(charID)` | `[awards, graphics]`: the same two shapes |
//! | `GetMedalDetails(medalID)` | `KeyVal(info=[KeyVal], graphics=CRowset)` |
//! | `GetRecipientsOfMedal(medalID)` | a list of `KeyVal` rows |
//! | `GiveMedalToCharacters(medalID, ids, reason[, True])` | `None`; type-3 notification and `OnMedalIssued` to each recipient |
//! | `SetMedalStatus({medalID: status})` | `None`; `OnMedalStatusChanged` to the caller |
//!
//! **Why two shapes.** The first half of each pair is only iterated and its
//! attributes read, so a list of `KeyVal` is enough and cannot trip on a
//! `DBRow` quirk (`GetRecipientsOfMedal` even assigns to its rows,
//! `recipient.reason = recipient.reason`). The second half is the one the
//! client calls a **method** on: `charSheetUtil` does
//! `.Filter('medalID').get(id)` and `uix.FormatMedalData` does
//! `.Filter('part').get(part)` **[V]**. A `CRowset` gives a `CFilterRowset`,
//! a `dict` whose `get` takes one argument; the pure-Python `Rowset`'s
//! `FilterRowset.get(key, val)` takes two and raises on that one-argument
//! call. So that half is a `CRowset` with its columns declared.
//!
//! **[A]** `GetMedalDetails(...).graphics` is never read by the client files
//! checked (`infoContentBase` reads `.info[0]` only), so its shape is a
//! `CRowset` of `medalID part graphic color` by analogy with the other
//! rowsets. Falsifier: a client path that reads `.graphics` another way.
//! `GetRecipientsOfMedal` answers one row per award, as the corp window
//! groups equal `recepientID`s itself (its own spelling) **[V]**.
//!
//! Only members of the medal's corp receive it, as the client's menu offers
//! the award on a same-corp pilot only (`isPilotInSameCorpAs`) **[V]**; a call
//! naming more than [`cm::MAX_RECIPIENTS`] pilots is refused whole.

use super::*;
use crate::corp_medals::{self as cm, Award, Graphic, Medal, Refusal};
use crate::notifications::{self as notif, Value};
use eve_marshal::{Column, DbRowDescriptor, DbType, DbValue};

/// A medal push: the event, its arguments, and who hears it.
#[derive(Debug, Clone, PartialEq)]
pub struct MedalPush {
    pub event: &'static str,
    pub args: Vec<PyValue>,
    pub to: Vec<i64>,
}

impl crate::notify::Push for MedalPush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (self.event, self.args.clone())
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

fn tell(reg: &mut Registry, to: Vec<i64>, event: &'static str, args: Vec<PyValue>) {
    if !to.is_empty() {
        reg.schedule(std::time::Duration::ZERO, MedalPush { event, args, to });
    }
}

fn raise(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
}

fn custom(text: &str) -> ServiceError {
    raise("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

fn denied(reason: &str) -> ServiceError {
    raise(
        "CrpAccessDenied",
        vec![("reason", PyValue::Unicode(reason.to_owned()))],
    )
}

fn refusal(r: Refusal) -> ServiceError {
    match r {
        Refusal::TitleTooLong => raise(
            "MedalNameTooLong",
            vec![("maxLength", crate::pyutil::bstr(&cm::MAX_TITLE.to_string()))],
        ),
        Refusal::DescriptionTooLong => raise(
            "MedalDescriptionTooLong",
            vec![(
                "maxLength",
                crate::pyutil::bstr(&cm::MAX_DESCRIPTION.to_string()),
            )],
        ),
        Refusal::TitleTooShort => custom("The medal's name is too short."),
        Refusal::DescriptionTooShort => custom("The medal's description is too short."),
        Refusal::BadGraphics => custom("A medal needs both a ribbon and a medal picture."),
    }
}

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        PyValue::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

fn text(v: Option<&PyValue>) -> Option<String> {
    match v? {
        PyValue::Unicode(s) => Some(s.clone()),
        PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }
}

fn arg<'a>(req: &'a Request<'_>, i: usize, name: &str) -> Option<&'a PyValue> {
    req.args
        .get(i)
        .or_else(|| crate::pyutil::pairs_get(req.kwargs, name.as_bytes()))
}

/// The graphics argument: a list of `[part, 'type.icon', color]`. Anything
/// that is not that shape refuses the whole medal, since dropping a row
/// silently could leave a part missing.
fn graphics_arg(v: Option<&PyValue>) -> Result<Vec<Graphic>, Refusal> {
    let Some(PyValue::List(rows) | PyValue::Tuple(rows)) = v else {
        return Err(Refusal::BadGraphics);
    };
    rows.iter()
        .map(|row| {
            let (PyValue::List(f) | PyValue::Tuple(f)) = row else {
                return Err(Refusal::BadGraphics);
            };
            Ok(Graphic {
                part: int(f.first()).ok_or(Refusal::BadGraphics)?,
                graphic: text(f.get(1)).ok_or(Refusal::BadGraphics)?,
                // The colour is a packed ARGB the client reads as signed;
                // the wire column is I4, so wrap to 32 bits at intake (an
                // out-of-range value would panic every later read). One the
                // picker left out is -1, which the ribbon builder skips.
                color: int(f.get(2)).map_or(-1, |n| i64::from(n as i32)),
            })
        })
        .collect()
}

/// The caller's corp and whether they may manage its medals.
fn manager(reg: &Registry, me: i64) -> Result<i64, ServiceError> {
    const NEED: &str = "You need the Director or Personnel Manager role to do that.";
    let m = reg
        .corporations()
        .membership(me)
        .ok_or_else(|| denied(NEED))?;
    let is_ceo = reg
        .corporations()
        .get(m.corporation_id)
        .is_some_and(|c| c.ceo_id == me);
    if cm::may_manage(m.roles | m.roles_at_other, is_ceo) {
        Ok(m.corporation_id)
    } else {
        Err(denied(NEED))
    }
}

fn in_corp(reg: &Registry, me: i64, corp: i64) -> bool {
    reg.corporations()
        .membership(me)
        .is_some_and(|m| m.corporation_id == corp)
}

fn describe(columns: &[(&str, DbType)]) -> DbRowDescriptor {
    DbRowDescriptor::new(
        columns
            .iter()
            .map(|(name, db_type)| Column {
                name: name.as_bytes().to_vec(),
                db_type: *db_type,
            })
            .collect(),
    )
    .expect("the medal columns are a valid layout")
}

/// The rowset the client calls `.Filter` on: `medalID part graphic color`
/// for each of `medals`, empty but declared when there are none.
fn graphics_rowset(medals: &[Medal]) -> eve_rowset::CRowset {
    let mut rs = eve_rowset::CRowset::new(describe(&[
        ("medalID", DbType::I4),
        ("part", DbType::I4),
        ("graphic", DbType::WStr),
        ("color", DbType::I4),
    ]));
    for m in medals {
        for g in &m.graphics {
            rs.push_values(vec![
                DbValue::Int(m.id),
                DbValue::Int(g.part),
                DbValue::Object(PyValue::Unicode(g.graphic.clone())),
                DbValue::Int(g.color),
            ])
            .expect("the graphic row matches its descriptor");
        }
    }
    rs
}

fn ustr(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

/// The award rows a viewer may see, and the distinct medals they name: one
/// set of graphics per medal, however often it was won.
fn received_parts(reg: &Registry, owner: i64, viewer: i64) -> (Vec<PyValue>, Vec<Medal>) {
    let awards = cm::visible_to(cm::received(reg.deltas(), owner), owner, viewer);
    let mut medals: Vec<Medal> = Vec::new();
    let mut rows = Vec::new();
    for a in &awards {
        let Some(m) = cm::get(reg.deltas(), a.medal) else {
            continue;
        };
        rows.push(crate::pyutil::keyval(vec![
            ("medalID", PyValue::Int(m.id)),
            ("status", PyValue::Int(a.status)),
            ("title", ustr(&m.title)),
            ("description", ustr(&m.description)),
            ("issuerID", PyValue::Int(a.issuer)),
            ("date", PyValue::Int(a.date)),
            ("reason", ustr(&a.reason)),
        ]));
        if !medals.iter().any(|x| x.id == m.id) {
            medals.push(m);
        }
    }
    (rows, medals)
}

/// `[awards, graphics]` for the awards a viewer may see.
fn received_pair(reg: &Registry, owner: i64, viewer: i64) -> PyValue {
    let (rows, medals) = received_parts(reg, owner, viewer);
    PyValue::List(vec![
        PyValue::List(rows),
        graphics_rowset(&medals).to_pyvalue(),
    ])
}

fn corp_pair(reg: &Registry, corp: i64) -> PyValue {
    let medals = cm::of_corp(reg.deltas(), corp);
    let rows = medals
        .iter()
        .map(|m| {
            crate::pyutil::keyval(vec![
                ("medalID", PyValue::Int(m.id)),
                ("title", ustr(&m.title)),
                ("description", ustr(&m.description)),
                ("date", PyValue::Int(m.date)),
                ("creatorID", PyValue::Int(m.creator)),
                // The client's own spelling.
                (
                    "noRecepients",
                    PyValue::Int(cm::award_count(reg.deltas(), m.id) as i64),
                ),
            ])
        })
        .collect();
    PyValue::List(vec![
        PyValue::List(rows),
        graphics_rowset(&medals).to_pyvalue(),
    ])
}

fn details(reg: &Registry, id: i64) -> Result<PyValue, ServiceError> {
    let m = cm::get(reg.deltas(), id).ok_or_else(|| custom("That medal does not exist."))?;
    let info = crate::pyutil::keyval(vec![
        ("medalID", PyValue::Int(m.id)),
        ("ownerID", PyValue::Int(m.corp)),
        ("title", ustr(&m.title)),
        ("description", ustr(&m.description)),
        (
            "numberOfRecipients",
            PyValue::Int(cm::award_count(reg.deltas(), m.id) as i64),
        ),
    ]);
    Ok(crate::pyutil::keyval(vec![
        ("info", PyValue::List(vec![info])),
        (
            "graphics",
            graphics_rowset(std::slice::from_ref(&m)).to_pyvalue(),
        ),
    ]))
}

fn recipients(reg: &Registry, me: i64, id: i64) -> PyValue {
    let Some(m) = cm::get(reg.deltas(), id).filter(|m| in_corp(reg, me, m.corp)) else {
        return PyValue::List(vec![]);
    };
    PyValue::List(
        cm::recipients(reg.deltas(), m.id)
            .iter()
            .map(|a: &Award| {
                crate::pyutil::keyval(vec![
                    // The client's own spelling.
                    ("recepientID", PyValue::Int(a.character)),
                    ("issuerID", PyValue::Int(a.issuer)),
                    ("date", PyValue::Int(a.date)),
                    ("reason", ustr(&a.reason)),
                ])
            })
            .collect(),
    )
}

fn create(reg: &mut Registry, req: &Request<'_>, me: i64) -> Result<PyValue, ServiceError> {
    let corp = manager(reg, me)?;
    let title =
        cm::validate_title(&text(arg(req, 0, "title")).unwrap_or_default()).map_err(refusal)?;
    let description =
        cm::validate_description(&text(arg(req, 1, "description")).unwrap_or_default())
            .map_err(refusal)?;
    let graphics = graphics_arg(arg(req, 2, "graphics")).map_err(refusal)?;
    cm::validate_graphics(&graphics).map_err(refusal)?;
    let now = crate::pyutil::filetime_now();
    let m = cm::create(
        reg.deltas_mut(),
        corp,
        me,
        now,
        title,
        description,
        graphics,
    );
    let members = reg.corporations().members_of(corp);
    tell(
        reg,
        members,
        "OnCorporationMedalAdded",
        vec![PyValue::Int(m.id)],
    );
    Ok(PyValue::None)
}

/// The ids argument: one int (`medals.py` sends a bare id for a single
/// recipient, though it wraps it first) or a list of them, without repeats.
fn recipient_ids(v: Option<&PyValue>) -> Vec<i64> {
    let mut out: Vec<i64> = Vec::new();
    let mut push = |n: i64| {
        if !out.contains(&n) {
            out.push(n);
        }
    };
    match v {
        Some(PyValue::List(items) | PyValue::Tuple(items)) => {
            items
                .iter()
                .filter_map(|x| int(Some(x)))
                .for_each(&mut push);
        }
        other => {
            if let Some(n) = int(other) {
                push(n);
            }
        }
    }
    out
}

fn give(reg: &mut Registry, req: &Request<'_>, me: i64) -> Result<PyValue, ServiceError> {
    let corp = manager(reg, me)?;
    let id = int(arg(req, 0, "medalID")).unwrap_or(0);
    let m = cm::get(reg.deltas(), id)
        .filter(|m| m.corp == corp)
        .ok_or_else(|| custom("That medal does not belong to your corporation."))?;
    let reason = text(arg(req, 2, "reason")).unwrap_or_default();
    let now = crate::pyutil::filetime_now();
    let mut told = Vec::new();
    let ids = recipient_ids(arg(req, 1, "recipientID"));
    if ids.len() > cm::MAX_RECIPIENTS {
        return Err(custom(&format!(
            "A medal can be given to at most {} pilots at once.",
            cm::MAX_RECIPIENTS
        )));
    }
    for c in ids {
        if !in_corp(reg, c, corp) {
            continue;
        }
        cm::give(reg.deltas_mut(), m.id, c, me, now, &reason);
        told.push(c);
    }
    for c in &told {
        notif::notify_from(
            reg,
            *c,
            notif::kind::CHAR_MEDAL,
            corp,
            notif::data([
                ("medalID", Value::Int(m.id)),
                ("corpID", Value::Int(corp)),
                ("reason", Value::Text(reason.clone())),
            ]),
        );
    }
    tell(reg, told, "OnMedalIssued", vec![PyValue::Int(m.id)]);
    // The corp window's recipient count and list have moved.
    let members = reg.corporations().members_of(corp);
    tell(
        reg,
        members,
        "OnCorporationMedalAdded",
        vec![PyValue::Int(m.id)],
    );
    Ok(PyValue::None)
}

fn set_status(reg: &mut Registry, req: &Request<'_>, me: i64) -> Result<PyValue, ServiceError> {
    let Some(entries) = arg(req, 0, "statusdict").and_then(PyValue::dict_as_kv) else {
        return Ok(PyValue::None);
    };
    let mut changed = false;
    for (k, v) in entries {
        if let (Some(medal), Some(status)) = (int(Some(k)), int(Some(v))) {
            changed |= cm::set_status(reg.deltas_mut(), me, medal, status);
        }
    }
    if changed {
        tell(reg, vec![me], "OnMedalStatusChanged", vec![]);
    }
    Ok(PyValue::None)
}

/// `corporationSvc`'s medal calls; the router in `corp.rs` sends only these.
pub(super) fn corporation_svc(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(me) = req.actor else {
        // No character: the shapes with nothing in them, so the arity holds.
        return match req.method {
            "GetMedalsReceived" | "GetAllCorpMedals" => Ok(PyValue::List(vec![
                PyValue::List(vec![]),
                graphics_rowset(&[]).to_pyvalue(),
            ])),
            "GetRecipientsOfMedal" => Ok(PyValue::List(vec![])),
            _ => Err(reg.no_method(req)),
        };
    };
    match req.method {
        "CreateMedal" => create(reg, req, me),
        "GiveMedalToCharacters" => give(reg, req, me),
        "SetMedalStatus" => set_status(reg, req, me),
        "GetMedalsReceived" => {
            let owner = int(arg(req, 0, "characterID")).unwrap_or(me);
            Ok(received_pair(reg, owner, me))
        }
        "GetAllCorpMedals" => {
            let corp = int(arg(req, 0, "corpID")).unwrap_or(0);
            if in_corp(reg, me, corp) {
                Ok(corp_pair(reg, corp))
            } else {
                Ok(PyValue::List(vec![
                    PyValue::List(vec![]),
                    graphics_rowset(&[]).to_pyvalue(),
                ]))
            }
        }
        "GetMedalDetails" => details(reg, int(arg(req, 0, "medalID")).unwrap_or(0)),
        "GetRecipientsOfMedal" => Ok(recipients(
            reg,
            me,
            int(arg(req, 0, "medalID")).unwrap_or(0),
        )),
        _ => Err(reg.no_method(req)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids: nothing here is a client row.
    const ALICE: i64 = 90_000_721;
    const BOB: i64 = 90_000_722;
    const CAROL: i64 = 90_000_723;
    const CORP: i64 = 98_000_721;
    const OTHER_CORP: i64 = 98_000_722;

    fn pilot(id: i64, name: &str) -> crate::model::Character {
        crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: id + 100_000_000,
            ..crate::model::Character::default()
        }
    }

    fn enrol(reg: &mut Registry, who: i64, corp: i64, roles: i64) {
        let (who, corp, roles) = (who.to_string(), corp.to_string(), roles.to_string());
        reg.corporations
            .parse_record(&["corpmember", &who, &corp, &roles, "1"])
            .expect("a membership record");
    }

    /// ALICE director of the corp, BOB a plain member, CAROL in another corp.
    fn world() -> Registry {
        let mut reg = crate::testkit::registry_with(|c| {
            c.characters = vec![
                pilot(ALICE, "Alice"),
                pilot(BOB, "Bob"),
                pilot(CAROL, "Carol"),
            ];
        });
        enrol(&mut reg, ALICE, CORP, cm::ROLE_DIRECTOR);
        enrol(&mut reg, BOB, CORP, 0);
        enrol(&mut reg, CAROL, OTHER_CORP, cm::ROLE_PERSONNEL_MANAGER);
        reg
    }

    fn call(
        reg: &mut Registry,
        who: i64,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service: "corporationSvc",
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

    fn pic(parts: &[(i64, &str)]) -> PyValue {
        PyValue::List(
            parts
                .iter()
                .map(|(p, g)| PyValue::List(vec![i(*p), u(g), i(3)]))
                .collect(),
        )
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
        crate::pyutil::dict_get(state, name.as_bytes())
            .unwrap_or_else(|| panic!("no {name} on {v:?}"))
    }

    fn make(reg: &mut Registry, who: i64) -> Result<PyValue, ServiceError> {
        call(
            reg,
            who,
            "CreateMedal",
            &[
                u("Rock Breaker"),
                u("For breaking a great many rocks"),
                pic(&[(1, "a.1_1"), (2, "b.2_3")]),
            ],
        )
    }

    fn give_to(
        reg: &mut Registry,
        who: i64,
        medal: i64,
        to: &[i64],
    ) -> Result<PyValue, ServiceError> {
        call(
            reg,
            who,
            "GiveMedalToCharacters",
            &[
                i(medal),
                PyValue::List(to.iter().map(|c| i(*c)).collect()),
                u("good rocks"),
            ],
        )
    }

    fn pushes(reg: &mut Registry) -> Vec<(&'static str, Vec<i64>)> {
        reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(1))
            .iter()
            .map(|p| (p.to_push().0, p.audience().unwrap_or_default()))
            .collect()
    }

    /// The two halves of a `[rows, graphics]` answer: the row list and the
    /// graphics `CRowset`'s `(part, graphic)` pairs per medal id.
    fn pair(v: &PyValue) -> Vec<PyValue> {
        let PyValue::List(halves) = v else {
            panic!("UNPACK_SEQUENCE 2 wants a sequence: {v:?}")
        };
        assert_eq!(halves.len(), 2, "the arity is pinned at three call sites");
        let PyValue::List(rows) = &halves[0] else {
            panic!("rows")
        };
        assert!(
            matches!(&halves[1], PyValue::ObjectEx { .. }),
            "the second half is the CRowset the client calls Filter on"
        );
        rows.clone()
    }

    #[test]
    fn the_personnel_manager_bit_is_the_corps() {
        assert_eq!(
            cm::ROLE_PERSONNEL_MANAGER,
            super::super::corp::CORP_ROLE_PERSONNEL_MANAGER
        );
        assert_eq!(cm::ROLE_DIRECTOR, super::super::corp::CORP_ROLE_DIRECTOR);
    }

    #[test]
    fn only_director_or_personnel_creates() {
        let mut reg = world();
        let err = make(&mut reg, BOB).expect_err("a plain member");
        assert_eq!(key_of(&err), "CrpAccessDenied");
        assert!(cm::of_corp(reg.deltas(), CORP).is_empty());
        make(&mut reg, ALICE).expect("a director");
        assert_eq!(cm::of_corp(reg.deltas(), CORP).len(), 1);
        // A personnel manager of another corp makes it for theirs.
        make(&mut reg, CAROL).expect("a personnel manager");
        assert_eq!(cm::of_corp(reg.deltas(), OTHER_CORP).len(), 1);
        assert_eq!(cm::of_corp(reg.deltas(), CORP).len(), 1);
        // And a pilot with no corp cannot.
        let mut lone = crate::testkit::registry_with(|c| c.characters = vec![pilot(ALICE, "A")]);
        assert_eq!(
            key_of(&make(&mut lone, ALICE).expect_err("no corp")),
            "CrpAccessDenied"
        );
        // Giving needs the role too.
        let id = cm::of_corp(reg.deltas(), CORP)[0].id;
        assert_eq!(
            key_of(&give_to(&mut reg, BOB, id, &[BOB]).expect_err("plain")),
            "CrpAccessDenied"
        );
        // Another corp's medal is not theirs to give.
        assert!(give_to(&mut reg, CAROL, id, &[BOB]).is_err());
        assert!(cm::received(reg.deltas(), BOB).is_empty());
    }

    #[test]
    fn a_medal_needs_both_parts() {
        let mut reg = world();
        for bad in [
            pic(&[(1, "a.1_1")]),
            pic(&[(2, "b.2_3")]),
            pic(&[(1, "a.1_1"), (2, "nodot")]),
            pic(&[(1, "a.1_1"), (2, "x.y")]),
            pic(&[(1, "a.0_1"), (2, "b.2_3")]),
            PyValue::None,
            PyValue::List(vec![PyValue::List(vec![i(1)])]),
        ] {
            let err = call(
                &mut reg,
                ALICE,
                "CreateMedal",
                &[u("Rock Breaker"), u("For breaking a great many rocks"), bad],
            )
            .expect_err("refused");
            assert_eq!(key_of(&err), "CustomNotify");
        }
        assert!(cm::of_corp(reg.deltas(), CORP).is_empty(), "nothing stored");
        assert!(pushes(&mut reg).is_empty(), "nothing said");
        // The limits keep the client's own message keys.
        let err = call(
            &mut reg,
            ALICE,
            "CreateMedal",
            &[
                u(&"x".repeat(101)),
                u("a long enough description"),
                pic(&[(1, "r.1_2"), (2, "m.3_4")]),
            ],
        )
        .expect_err("too long");
        assert_eq!(key_of(&err), "MedalNameTooLong");
    }

    #[test]
    fn a_second_award_is_counted() {
        let mut reg = world();
        make(&mut reg, ALICE).expect("made");
        let id = cm::of_corp(reg.deltas(), CORP)[0].id;
        give_to(&mut reg, ALICE, id, &[BOB]).expect("given");
        give_to(&mut reg, ALICE, id, &[BOB, ALICE]).expect("given again");
        let got = call(&mut reg, BOB, "GetMedalsReceived", &[i(BOB)]).expect("answered");
        let rows = pair(&got);
        assert_eq!(rows.len(), 2, "one row per award: the client counts them");
        assert!(rows.iter().all(|r| attr(r, "medalID") == &i(id)));
        // One set of graphics, not one per award.
        let (_, medals) = received_parts(&reg, BOB, BOB);
        assert_eq!(medals.len(), 1, "Bob holds one medal, twice");
        assert_eq!(
            graphics_rowset(&medals).len(),
            2,
            "a ribbon row and a medal row"
        );
        // The corp window counts the awards: Bob twice, Alice once.
        let corp_rows = pair(&call(&mut reg, ALICE, "GetAllCorpMedals", &[i(CORP)]).unwrap());
        assert_eq!(attr(&corp_rows[0], "noRecepients"), &i(3));
        let who = call(&mut reg, ALICE, "GetRecipientsOfMedal", &[i(id)]).unwrap();
        let PyValue::List(who) = who else {
            panic!("list")
        };
        assert_eq!(who.len(), 3);
        assert!(who.iter().any(|r| attr(r, "recepientID") == &i(ALICE)));
        // Someone outside the corp is told nothing of who holds it.
        let none = call(&mut reg, CAROL, "GetRecipientsOfMedal", &[i(id)]).unwrap();
        assert_eq!(none, PyValue::List(vec![]));
        let none = call(&mut reg, CAROL, "GetAllCorpMedals", &[i(CORP)]).unwrap();
        assert_eq!(pair(&none).len(), 0);
    }

    #[test]
    fn status_filters_public_and_private() {
        let mut reg = world();
        make(&mut reg, ALICE).expect("made");
        let id = cm::of_corp(reg.deltas(), CORP)[0].id;
        give_to(&mut reg, ALICE, id, &[BOB]).expect("given");
        // Privacy is asserted from an explicit status, not the [A] default.
        call(
            &mut reg,
            BOB,
            "SetMedalStatus",
            &[PyValue::Dict(vec![(i(cm::status::PRIVATE), i(id))])],
        )
        .expect("private");
        let seen_by = |reg: &mut Registry, viewer| {
            pair(&call(reg, viewer, "GetMedalsReceived", &[i(BOB)]).expect("answered"))
        };
        assert_eq!(seen_by(&mut reg, BOB).len(), 1, "his own, private");
        assert_eq!(seen_by(&mut reg, ALICE).len(), 0, "private to others");
        let _ = pushes(&mut reg);
        call(
            &mut reg,
            BOB,
            "SetMedalStatus",
            &[PyValue::Dict(vec![(i(cm::status::PUBLIC), i(id))])],
        )
        .expect("set");
        assert_eq!(
            pushes(&mut reg),
            vec![("OnMedalStatusChanged", vec![BOB])],
            "only the holder is told"
        );
        let public = seen_by(&mut reg, ALICE);
        assert_eq!(public.len(), 1, "now public");
        assert_eq!(attr(&public[0], "status"), &i(cm::status::PUBLIC));
        // Setting a medal one does not hold changes nothing and says nothing.
        call(
            &mut reg,
            CAROL,
            "SetMedalStatus",
            &[PyValue::Dict(vec![(i(cm::status::PUBLIC), i(id))])],
        )
        .expect("ignored");
        assert!(pushes(&mut reg).is_empty());
        // Deleted: gone for others, still listed for him (status 1).
        call(
            &mut reg,
            BOB,
            "SetMedalStatus",
            &[PyValue::Dict(vec![(i(cm::status::DELETED), i(id))])],
        )
        .expect("deleted");
        assert_eq!(seen_by(&mut reg, ALICE).len(), 0);
        assert_eq!(seen_by(&mut reg, BOB).len(), 1);
    }

    #[test]
    fn too_many_recipients_refuse_the_call() {
        let mut reg = world();
        make(&mut reg, ALICE).expect("made");
        let id = cm::of_corp(reg.deltas(), CORP)[0].id;
        let _ = pushes(&mut reg);
        let many: Vec<i64> = (1..=cm::MAX_RECIPIENTS as i64 + 1).collect();
        let err = give_to(&mut reg, ALICE, id, &many).expect_err("too many");
        assert_eq!(key_of(&err), "CustomNotify");
        assert!(cm::recipients(reg.deltas(), id).is_empty(), "none given");
        assert!(pushes(&mut reg).is_empty());
        // The same pilot listed twice counts once.
        give_to(&mut reg, ALICE, id, &[BOB, BOB]).expect("given");
        assert_eq!(cm::received(reg.deltas(), BOB).len(), 1);
    }

    #[test]
    fn a_colour_is_wrapped_to_the_wire_column() {
        let mut reg = world();
        let rows = |c: &[PyValue]| {
            PyValue::List(vec![
                PyValue::List(
                    vec![i(1), u("a.1_1")]
                        .into_iter()
                        .chain(c.iter().cloned())
                        .collect(),
                ),
                PyValue::List(vec![i(2), u("b.2_3"), i(-330_271)]),
            ])
        };
        for (given, stored) in [
            (vec![i(-330_271)], -330_271),
            (vec![i(4_294_967_295)], -1),
            (vec![i(i64::MAX)], -1),
            (vec![], -1),
        ] {
            call(
                &mut reg,
                ALICE,
                "CreateMedal",
                &[
                    u("Rock Breaker"),
                    u("For breaking a great many rocks"),
                    rows(&given),
                ],
            )
            .expect("made");
            let m = cm::of_corp(reg.deltas(), CORP).pop().unwrap();
            assert_eq!(m.graphics[0].color, stored, "{given:?}");
            assert_eq!(m.graphics[1].color, -330_271, "negative survives");
            // Reading it back must not panic on the I4 column.
            let _ = graphics_rowset(std::slice::from_ref(&m));
        }
    }

    #[test]
    fn medal_details_feed_the_notification() {
        let mut reg = world();
        make(&mut reg, ALICE).expect("made");
        let id = cm::of_corp(reg.deltas(), CORP)[0].id;
        assert_eq!(
            pushes(&mut reg),
            vec![("OnCorporationMedalAdded", vec![ALICE, BOB])],
            "the corp hears of a new medal, and only the corp"
        );
        give_to(&mut reg, ALICE, id, &[BOB, CAROL, 1]).expect("given");
        let told = pushes(&mut reg);
        assert!(told.contains(&("OnMedalIssued", vec![BOB])), "{told:?}");
        // Carol is in another corp and the unknown id 1 is nobody's: neither
        // is given the medal.
        assert!(cm::received(reg.deltas(), CAROL).is_empty());
        assert!(cm::received(reg.deltas(), 1).is_empty());
        // Type 3 reaches the recipient with the keys its formatter reads.
        let mine = crate::notifications::of(reg.deltas(), BOB);
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].type_id, 3);
        let get = |k: &str| {
            mine[0]
                .data
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("medalID"), Some(Value::Int(id)));
        assert_eq!(get("corpID"), Some(Value::Int(CORP)));
        // The body label reads `{reason}` from the data.
        assert_eq!(get("reason"), Some(Value::Text("good rocks".into())));
        // The formatter's own call: `GetMedalDetails(medalID).info[0]`.
        let d = call(&mut reg, BOB, "GetMedalDetails", &[i(id)]).expect("details");
        let PyValue::List(info) = attr(&d, "info") else {
            panic!("info is indexed with [0]")
        };
        assert_eq!(attr(&info[0], "title"), &u("Rock Breaker"));
        assert_eq!(
            attr(&info[0], "description"),
            &u("For breaking a great many rocks")
        );
        assert_eq!(attr(&info[0], "ownerID"), &i(CORP));
        assert_eq!(attr(&info[0], "numberOfRecipients"), &i(1));
        // A medal that does not exist is a refusal, not an empty `info`.
        assert!(call(&mut reg, BOB, "GetMedalDetails", &[i(9999)]).is_err());
    }

    #[test]
    fn rows_survive_a_restart() {
        let mut reg = world();
        make(&mut reg, ALICE).expect("made");
        let id = cm::of_corp(reg.deltas(), CORP)[0].id;
        give_to(&mut reg, ALICE, id, &[BOB]).expect("given");
        call(
            &mut reg,
            BOB,
            "SetMedalStatus",
            &[PyValue::Dict(vec![(i(cm::status::PUBLIC), i(id))])],
        )
        .expect("set");
        use crate::store::Store;
        let dir = crate::testkit::scratch_dir("medals-restart");
        let files = crate::store::FileStore::new(dir.join("world.dat"));
        files.save(&reg.to_snapshot()).expect("saves");
        let snap = files.load().expect("loads").expect("a world");
        let mut again = world();
        again.apply_snapshot(snap);
        // Memberships are the corp module's own record, not this lane's.
        enrol(&mut again, ALICE, CORP, cm::ROLE_DIRECTOR);
        let rows = pair(&call(&mut again, ALICE, "GetMedalsReceived", &[i(BOB)]).unwrap());
        assert_eq!(rows.len(), 1, "Bob's public medal is still his");
        assert_eq!(attr(&rows[0], "title"), &u("Rock Breaker"));
        assert_eq!(attr(&rows[0], "reason"), &u("good rocks"));
        make(&mut again, ALICE).expect("made again");
        let ids: Vec<i64> = cm::of_corp(again.deltas(), CORP)
            .iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, vec![id, id + 1], "the counter was kept");
    }

    #[test]
    fn the_character_sheet_survives_no_medals() {
        // `UNPACK_SEQUENCE 2` and a `.Filter('medalID')` on the second half
        // even for a pilot with nothing, and with no character at all.
        let mut reg = world();
        let rows = pair(&call(&mut reg, BOB, "GetMedalsReceived", &[i(BOB)]).unwrap());
        assert!(rows.is_empty());
        let none = reg
            .dispatch(&Request {
                service: "corporationSvc",
                method: "GetMedalsReceived",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: None,
            })
            .unwrap();
        assert_eq!(pair(&none).len(), 0);
        let columns: Vec<String> = graphics_rowset(&[])
            .descriptor()
            .columns()
            .iter()
            .map(|c| String::from_utf8_lossy(&c.name).into_owned())
            .collect();
        for want in ["medalID", "part", "graphic", "color"] {
            assert!(columns.iter().any(|c| c == want), "{want} must be declared");
        }
    }
}
