//! Alliances over the wire (SOC-C1): `allianceRegistry`, the alliance half of
//! `corpRegistry`, the pushes, and the `allianceid` session key.
//!
//! The state and its rules are `crate::alliance`; this file reads the client
//! and answers it. The client reading behind every arm is in
//! `docs/lanes/soc-c1.md` §2.
//!
//! **Two routes to one service.** `allianceRegistry` is reached unbound
//! (`sm.RemoteSvc('allianceRegistry').GetAlliance(allianceID)`, the info
//! window, the rankings) and through the moniker `eveMoniker.GetAlliance`,
//! which binds on `(session.allianceid, 1)` **[V]** (`eveMoniker.py`). A
//! bound call carries that tuple in `Request::bind_params`; an unbound one
//! names the alliance in its first argument.

use super::*;
use crate::alliance::{
    ALLIANCE_CREATION_COST, APP_ACCEPTED, APP_REJECTED, Alliance, Alliances, Application,
    REF_ALLIANCE_REGISTRATION_FEE,
};

/// The services dispatched here.
pub(crate) const SERVICES: &[&str] = &["allianceRegistry"];

/// The `corpRegistry` methods that are about alliances. `corp::corp_registry`
/// hands these to [`corp_registry`].
pub(crate) const CORP_REGISTRY_METHODS: &[&str] = &[
    "CreateAlliance",
    "GetSuggestedAllianceShortNames",
    "ApplyToJoinAlliance",
    "GetAllianceApplications",
    "DeleteAllianceApplication",
];

fn refuse(key: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, vec![]))
}

fn refuse_with(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
}

/// `CrpAccessDenied {reason}`, the client's own refusal for a role it lacks
/// (`roleChecks.py`) **[V]**. The reason is plain text: the dialog formats
/// whatever it is given.
fn access_denied(reason: &str) -> ServiceError {
    refuse_with(
        "CrpAccessDenied",
        vec![("reason", PyValue::Unicode(reason.to_owned()))],
    )
}

fn bad_args(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

fn text_arg(v: Option<&PyValue>) -> Option<String> {
    match v? {
        PyValue::Unicode(s) => Some(s.clone()),
        PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }
}

fn int_arg(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        _ => None,
    }
}

/// Positional `i`, or the keyword `name`.
fn arg<'a>(req: &'a Request<'_>, i: usize, name: &str) -> Option<&'a PyValue> {
    req.args.get(i).or_else(|| {
        // The wire pairs are `(value, key)`: `pairs_get` is the one reader
        // that knows it (tracker §39; found again by CORP-W).
        crate::pyutil::pairs_get(req.kwargs, name.as_bytes())
    })
}

fn opt(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

// ---------------------------------------------------------------------------
// Who is asking
// ---------------------------------------------------------------------------

/// The acting character as a player-corp member.
#[derive(Debug, Clone, Copy)]
struct Me {
    character_id: i64,
    corporation_id: i64,
    director: bool,
    ceo: bool,
}

fn me(reg: &Registry, actor: Option<i64>) -> Option<Me> {
    let character_id = actor?;
    let m = reg.corporations.membership(character_id)?;
    let ceo = reg
        .corporations
        .get(m.corporation_id)
        .is_some_and(|c| c.ceo_id == character_id);
    Some(Me {
        character_id,
        corporation_id: m.corporation_id,
        director: ceo || m.roles & corp::CORP_ROLE_DIRECTOR != 0,
        ceo,
    })
}

/// The acting character, required to be a director of a player corp.
fn director(reg: &Registry, actor: Option<i64>) -> Result<Me, ServiceError> {
    me(reg, actor)
        .filter(|m| m.director)
        .ok_or_else(|| access_denied("Only a director of a player corporation can do that."))
}

/// The alliance a call is about: the bind's `(allianceID, isMaster)`, or the
/// actor's own.
fn bound_alliance(reg: &Registry, req: &Request<'_>) -> Option<i64> {
    match req.bind_params {
        Some(PyValue::Tuple(t)) => t.first().and_then(|v| int_arg(Some(v))),
        Some(PyValue::Int(i)) => Some(*i),
        _ => None,
    }
    .or_else(|| {
        req.actor
            .and_then(|a| crate::alliance::of_character(reg, a))
    })
}

/// The acting director's alliance, required to be the one the call is
/// about, and their corp to be its executor. **[A]** that only the
/// executor's directors answer applications, kick and edit: Tranquility's
/// rule is server-side. Falsified by a capture of a non-executor director
/// doing one of these.
fn executor_director(
    reg: &Registry,
    req: &Request<'_>,
    st: &Alliances,
) -> Result<(Me, i64), ServiceError> {
    let me = director(reg, req.actor)?;
    let mine = st
        .member(me.corporation_id)
        .map(|m| m.alliance_id)
        .ok_or_else(|| refuse("CantConfigureNotInAlliance"))?;
    if bound_alliance(reg, req).is_some_and(|a| a != mine) {
        return Err(refuse("CantConfigureNotInAlliance"));
    }
    if st.get(mine).and_then(|a| a.executor_corp_id) != Some(me.corporation_id) {
        return Err(refuse("NeedToBeInExecutorCorp"));
    }
    Ok((me, mine))
}

// ---------------------------------------------------------------------------
// Audiences and the session
// ---------------------------------------------------------------------------

/// Every character in every member corp of an alliance.
fn alliance_characters(reg: &Registry, st: &Alliances, alliance_id: i64) -> Vec<i64> {
    st.members_of(alliance_id)
        .into_iter()
        .flat_map(|(corp, _)| reg.corporations.members_of(corp))
        .collect()
}

/// A corp's directors, the CEO included.
fn corp_directors(reg: &Registry, corporation_id: i64) -> Vec<i64> {
    let ceo = reg.corporations.get(corporation_id).map(|c| c.ceo_id);
    reg.corporations
        .members_of(corporation_id)
        .into_iter()
        .filter(|id| {
            Some(*id) == ceo
                || reg
                    .corporations
                    .membership(*id)
                    .is_some_and(|m| m.roles & corp::CORP_ROLE_DIRECTOR != 0)
        })
        .collect()
}

/// Who hears about an application: the applicant's directors and the
/// directors of every member corp (the Applications tab is a director's,
/// `ShowAllianceApplications`) **[V]**.
fn application_audience(reg: &Registry, st: &Alliances, alliance_id: i64, corp: i64) -> Vec<i64> {
    let mut to = corp_directors(reg, corp);
    for (member, _) in st.members_of(alliance_id) {
        to.extend(corp_directors(reg, member));
    }
    to.sort_unstable();
    to.dedup();
    to
}

/// Move every member of `corporation_id` into `alliance_id` (or out): the
/// character field the slims and the login read, and the `allianceid`
/// session key, whose change is what `svc.alliance.OnSessionChanged` binds
/// the moniker on and every alliance cache drops on **[V]**.
fn set_corp_alliance(reg: &mut Registry, corporation_id: i64, alliance_id: Option<i64>) {
    for id in reg.corporations.members_of(corporation_id) {
        let old = reg.character(id).and_then(|c| c.alliance_id);
        if let Some(c) = reg.character_mut(id) {
            c.alliance_id = alliance_id;
        }
        reg.queue_session_change(id, session_change(old, alliance_id));
    }
}

/// `allianceid`, as `(old, new)`.
pub(crate) fn session_change(old: Option<i64>, new: Option<i64>) -> Vec<(PyValue, PyValue)> {
    crate::pyutil::pypairs(vec![(
        "allianceid",
        PyValue::Tuple(vec![opt(old), opt(new)]),
    )])
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// The alliance row's columns: every name read off it in the alliance Home
/// tab (`all_ui_home.ShowDetails`), the info window
/// (`infosvc.UpdateDataAlliance`), the rankings (`all_ui_rankings`) and
/// `ApplyToAllianceWnd` **[V]**. A `blue.DBRow`, because
/// `AllianceO.LoadAlliance` does `CRowset(alliance.__header__, [])` **[V]**.
const ALLIANCE_COLUMNS: &[(&str, DbType)] = &[
    ("allianceID", DbType::I4),
    ("allianceName", DbType::WStr),
    ("shortName", DbType::WStr),
    ("executorCorpID", DbType::I4),
    ("creatorCorpID", DbType::I4),
    ("creatorCharID", DbType::I4),
    ("warFactionID", DbType::I4),
    ("description", DbType::WStr),
    ("url", DbType::WStr),
    ("startDate", DbType::FileTime),
    ("memberCount", DbType::I4),
    ("dictatorial", DbType::Bool),
    ("allowWar", DbType::Bool),
    ("deleted", DbType::Bool),
    ("currentCapital", DbType::I4),
    ("currentPrimeHour", DbType::I4),
    ("newPrimeHour", DbType::I4),
    ("newPrimeHourValidAfter", DbType::FileTime),
];

/// `GetMembers`' columns: `allianceID` and `corporationID` (`GetEntry`),
/// `chosenExecutorID` and `startDate` (`all_ui_members.__GetLabel`) **[V]**.
const MEMBER_COLUMNS: &[(&str, DbType)] = &[
    ("allianceID", DbType::I4),
    ("corporationID", DbType::I4),
    ("chosenExecutorID", DbType::I4),
    ("startDate", DbType::FileTime),
];

/// An application's columns: `allianceID`, `corporationID`, `state`,
/// `applicationText` (`all_ui_applications`) **[V]**; `applicationDateTime`
/// **[I]**, the corp applications' own name for the date.
const APPLICATION_COLUMNS: &[(&str, DbType)] = &[
    ("allianceID", DbType::I4),
    ("corporationID", DbType::I4),
    ("applicationText", DbType::WStr),
    ("applicationDateTime", DbType::FileTime),
    ("state", DbType::I4),
];

fn descriptor(columns: &[(&str, DbType)]) -> DbRowDescriptor {
    DbRowDescriptor::from_pairs(columns).expect("a fixed, valid layout")
}

fn db_opt(v: Option<i64>) -> DbValue {
    v.map_or(DbValue::Null, DbValue::Int)
}

fn db_text(s: &str) -> DbValue {
    DbValue::Object(PyValue::Unicode(s.to_owned()))
}

fn alliance_values(a: &Alliance, member_count: usize, allow_war: bool) -> Vec<DbValue> {
    vec![
        DbValue::Int(a.alliance_id),
        db_text(&a.name),
        db_text(&a.short_name),
        db_opt(a.executor_corp_id),
        DbValue::Int(a.creator_corp_id),
        DbValue::Int(a.creator_char_id),
        // No militia: alliances in factional warfare are `facwar_mgr`'s.
        DbValue::Null,
        db_text(&a.description),
        db_text(&a.url),
        DbValue::Int(a.start_date),
        DbValue::Int(member_count as i64),
        DbValue::Bool(a.dictatorial),
        // The war permit (C8): the executor corp's.
        DbValue::Bool(allow_war),
        DbValue::Bool(a.deleted),
        // No sovereignty yet (Stage D): no capital, no prime hour.
        DbValue::Null,
        DbValue::Null,
        DbValue::Null,
        DbValue::Null,
    ]
}

fn alliance_row(st: &Alliances, alliance_id: i64, allow_war: bool) -> Option<DbRow> {
    let a = st.get(alliance_id)?;
    DbRow::new(
        descriptor(ALLIANCE_COLUMNS),
        alliance_values(a, st.members_of(alliance_id).len(), allow_war),
    )
    .ok()
}

fn member_values(corp: i64, m: &crate::alliance::Member) -> Vec<DbValue> {
    vec![
        DbValue::Int(m.alliance_id),
        DbValue::Int(corp),
        db_opt(m.chosen_executor),
        DbValue::Int(m.start_date),
    ]
}

fn application_values(a: &Application) -> Vec<DbValue> {
    vec![
        DbValue::Int(a.alliance_id),
        DbValue::Int(a.corporation_id),
        db_text(&a.text),
        DbValue::Int(a.date),
        DbValue::Int(a.state),
    ]
}

/// A `CRowset` of these rows, or that rowset `.Index(column)`ed: the client
/// keeps the members and applications in a `CIndexedRowset` and reads its
/// `.columns` and `.header` to apply an add push (`OnAllianceMemberChanged`)
/// **[V]**, so the header must be a real descriptor.
fn rowset(columns: &[(&str, DbType)], rows: Vec<Vec<DbValue>>, index: Option<&str>) -> PyValue {
    let mut rs = eve_rowset::CRowset::new(descriptor(columns));
    for r in rows {
        if rs.push_values(r).is_err() {
            // A row not matching its own fixed layout is a bug above; leave
            // it out rather than fail the whole answer.
            continue;
        }
    }
    match index {
        Some(col) => match rs.index(col) {
            Ok(ix) => ix.to_pyvalue(),
            Err(_) => rs.to_pyvalue(),
        },
        None => rs.to_pyvalue(),
    }
}

/// `GetAlliancePublicInfo`: attribute reads only (`.warFactionID`,
/// `.executorCorpID`, `.description`, `.deleted` and the info window's
/// `UpdateDataAlliance` set) **[V]**, so a `KeyVal` with the alliance row's
/// names.
fn public_info(st: &Alliances, alliance_id: i64, allow_war: bool) -> Option<PyValue> {
    let a = st.get(alliance_id)?;
    let values = alliance_values(a, st.members_of(alliance_id).len(), allow_war);
    let attrs: Vec<(&str, PyValue)> = ALLIANCE_COLUMNS
        .iter()
        .zip(values)
        .map(|((name, _), v)| {
            let v = match v {
                DbValue::Null => PyValue::None,
                DbValue::Bool(b) => PyValue::Bool(b),
                DbValue::Int(n) => PyValue::Int(n),
                DbValue::Real(r) => PyValue::Real(r),
                DbValue::Object(o) => o,
            };
            (*name, v)
        })
        .collect();
    Some(keyval(attrs))
}

/// A `{column: (old, new)}` change.
fn change(pairs: Vec<(&str, PyValue, PyValue)>) -> PyValue {
    pydict(
        pairs
            .into_iter()
            .map(|(k, old, new)| (k, PyValue::Tuple(vec![old, new])))
            .collect(),
    )
}

/// Every column, `(None, value)` for an add or `(value, None)` for a
/// removal: `GetAddRemoveFromChange` decides add or remove from whether
/// every old (new) value is `None`, and an add must carry exactly the
/// rowset's columns (`len(change) != len(columns)` is ignored) **[V]**.
fn whole_row_change(columns: &[(&str, DbType)], values: Vec<DbValue>, add: bool) -> PyValue {
    change(
        columns
            .iter()
            .zip(values)
            .map(|((name, _), v)| {
                let v = match v {
                    DbValue::Null => PyValue::None,
                    DbValue::Bool(b) => PyValue::Bool(b),
                    DbValue::Int(n) => PyValue::Int(n),
                    DbValue::Real(r) => PyValue::Real(r),
                    DbValue::Object(o) => o,
                };
                if add {
                    (*name, PyValue::None, v)
                } else {
                    (*name, v, PyValue::None)
                }
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// The pushes
// ---------------------------------------------------------------------------

/// One alliance notification: `OnAllianceChanged(allianceID, change)`,
/// `OnAllianceMemberChanged(allianceID, corpID, change)` or
/// `OnAllianceApplicationChanged(allianceID, corpID, change)`, the three
/// handlers in `all_cso` and `bco_alliance` **[V]**.
///
/// An update change always carries its key column as `(id, id)`: an update
/// whose every old value is `None` (a first executor vote) reads as an add to
/// `GetAddRemoveFromChange` and is dropped for its column count **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct AllianceNotice {
    pub to: Vec<i64>,
    pub event: &'static str,
    pub args: Vec<PyValue>,
}

impl crate::notify::Push for AllianceNotice {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (self.event, self.args.clone())
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

fn notify(reg: &mut Registry, to: Vec<i64>, event: &'static str, args: Vec<PyValue>) {
    if !to.is_empty() {
        reg.push(AllianceNotice { to, event, args });
    }
}

fn tell_application(
    reg: &mut Registry,
    st: &Alliances,
    alliance_id: i64,
    corp: i64,
    change: PyValue,
) {
    let to = application_audience(reg, st, alliance_id, corp);
    notify(
        reg,
        to,
        "OnAllianceApplicationChanged",
        vec![PyValue::Int(alliance_id), PyValue::Int(corp), change],
    );
}

fn tell_executor(
    reg: &mut Registry,
    st: &Alliances,
    alliance_id: i64,
    (old, new): (Option<i64>, Option<i64>),
) {
    let to = alliance_characters(reg, st, alliance_id);
    let c = change(vec![
        (
            "allianceID",
            PyValue::Int(alliance_id),
            PyValue::Int(alliance_id),
        ),
        ("executorCorpID", opt(old), opt(new)),
    ]);
    notify(
        reg,
        to,
        "OnAllianceChanged",
        vec![PyValue::Int(alliance_id), c],
    );
}

// ---------------------------------------------------------------------------
// corpRegistry's alliance arms
// ---------------------------------------------------------------------------

/// The `corpRegistry` arms in [`CORP_REGISTRY_METHODS`].
pub(crate) fn corp_registry(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    match req.method {
        "CreateAlliance" => create_alliance(reg, req),
        // `SetShortNameWnd` reads `.shortName` off each and skips `None`
        // **[V]**.
        "GetSuggestedAllianceShortNames" => {
            let name = text_arg(arg(req, 0, "allianceName")).unwrap_or_default();
            let st = Alliances::load(reg.deltas());
            Ok(PyValue::List(
                st.suggest_short_names(&name)
                    .into_iter()
                    .map(|s| keyval(vec![("shortName", PyValue::Unicode(s))]))
                    .collect(),
            ))
        }
        "ApplyToJoinAlliance" => apply_to_join(reg, req),
        // `CorpAllianceO.GetAllianceApplications`: a `CIndexedRowset` keyed
        // by `allianceID`, subscripted by alliance and patched by
        // `OnAllianceApplicationChanged` through `.columns`/`.header` **[V]**.
        // Only a director sees them (`ShowMyApplications`) **[V]**.
        "GetAllianceApplications" => {
            let st = Alliances::load(reg.deltas());
            let rows = director(reg, req.actor)
                .map(|me| {
                    st.applications_from(me.corporation_id)
                        .into_iter()
                        .map(application_values)
                        .collect()
                })
                .unwrap_or_default();
            Ok(rowset(APPLICATION_COLUMNS, rows, Some("allianceID")))
        }
        "DeleteAllianceApplication" => {
            let me = director(reg, req.actor)?;
            let alliance_id = int_arg(arg(req, 0, "allianceID"))
                .ok_or_else(|| bad_args(req, "allianceID is not an id"))?;
            let mut st = Alliances::load(reg.deltas());
            let gone = st
                .withdraw(me.corporation_id, alliance_id)
                .map_err(refuse)?;
            st.save(reg.deltas_mut());
            let c = whole_row_change(APPLICATION_COLUMNS, application_values(&gone), false);
            tell_application(reg, &st, alliance_id, me.corporation_id, c);
            Ok(PyValue::None)
        }
        _ => Err(reg.no_method(req)),
    }
}

/// `corpRegistry.CreateAlliance(allianceName, shortName, description, url)`
/// -- `bco_alliance.CreateAlliance`, called inside
/// `sessionMgr.PerformSessionChange('alliance.addalliance', ...)` and its
/// return discarded (`CreateAllianceWnd.Confirm_thread`) **[V]**. So the
/// effect is the `allianceid` session change, and a refusal must be a raise.
///
/// The Create button is offered only to the corp's CEO
/// (`FormAlliancesHome.construct_button_group`) **[V]**; refusals are the
/// client's keys: `OnlyActiveCEOCanCreateAlliance`,
/// `AllianceCreateFailCorpInAlliance`, the name and short-name keys, and
/// `NotEnoughMoney {amount, balance}`.
fn create_alliance(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let me = me(reg, req.actor)
        .filter(|m| m.ceo)
        .ok_or_else(|| refuse("OnlyActiveCEOCanCreateAlliance"))?;
    let name = text_arg(arg(req, 0, "allianceName"))
        .ok_or_else(|| bad_args(req, "allianceName is not a string"))?;
    let short = text_arg(arg(req, 1, "shortName"))
        .ok_or_else(|| bad_args(req, "shortName is not a string"))?;
    let description = text_arg(arg(req, 2, "description")).unwrap_or_default();
    let url = text_arg(arg(req, 3, "url")).unwrap_or_default();
    let (name, short) = (name.trim().to_owned(), short.trim().to_owned());

    let mut st = Alliances::load(reg.deltas());
    if st.member(me.corporation_id).is_some() {
        return Err(refuse("AllianceCreateFailCorpInAlliance"));
    }
    st.check_names(&name, &short).map_err(refuse)?;
    // CORP-W: the fee comes out of the corporation's Master Wallet, not the
    // CEO's own (closes SOC-C1's [A]). `NotEnoughMoneyCorp` names the
    // division and the corp.
    corp_wallet::check_fee(
        reg,
        me.corporation_id,
        crate::corp_wallet::MASTER_KEY,
        ALLIANCE_CREATION_COST,
    )?;
    let now = crate::pyutil::filetime_now();
    let alliance_id = st
        .create(
            me.corporation_id,
            me.character_id,
            &name,
            &short,
            &description,
            &url,
            now,
        )
        .map_err(refuse)?;
    st.save(reg.deltas_mut());
    // The counterparty is the CEO's station owner, as for a corp's fee
    // (`[A]`: Tranquility books it to CONCORD).
    corp_wallet::charge_fee(
        reg,
        me.corporation_id,
        crate::corp_wallet::MASTER_KEY,
        ALLIANCE_CREATION_COST,
        REF_ALLIANCE_REGISTRATION_FEE,
        me.character_id,
        alliance_id,
    );
    set_corp_alliance(reg, me.corporation_id, Some(alliance_id));
    Ok(PyValue::None)
}

/// `corpRegistry.ApplyToJoinAlliance(allianceID, applicationText)`, from
/// `corpui.ApplyToJoinAlliance` -- which already demands a director who is
/// the CEO and a corp in no alliance (`CheckCanApplyToJoinAlliance`) **[V]**.
fn apply_to_join(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let me = me(reg, req.actor)
        .filter(|m| m.ceo)
        .ok_or_else(|| access_denied("Only the active CEO can apply to an alliance."))?;
    let alliance_id = int_arg(arg(req, 0, "allianceID"))
        .ok_or_else(|| bad_args(req, "allianceID is not an id"))?;
    let text = text_arg(arg(req, 1, "applicationText")).unwrap_or_default();
    // A corporation with wars of its own cannot apply (C8).
    crate::war_deal::may_join_alliance(reg, me.corporation_id, alliance_id, false)
        .map_err(refuse)?;
    let mut st = Alliances::load(reg.deltas());
    let now = crate::pyutil::filetime_now();
    let replaced = st
        .apply(me.corporation_id, alliance_id, &text, now)
        .map_err(refuse)?;
    st.save(reg.deltas_mut());
    let Some(new) = st.application(alliance_id, me.corporation_id).cloned() else {
        return Ok(PyValue::None);
    };
    let c = match replaced {
        None => whole_row_change(APPLICATION_COLUMNS, application_values(&new), true),
        Some(old) => change(vec![
            (
                "corporationID",
                PyValue::Int(new.corporation_id),
                PyValue::Int(new.corporation_id),
            ),
            (
                "applicationText",
                PyValue::Unicode(old.text),
                PyValue::Unicode(new.text.clone()),
            ),
            (
                "applicationDateTime",
                PyValue::Int(old.date),
                PyValue::Int(new.date),
            ),
            ("state", PyValue::Int(old.state), PyValue::Int(new.state)),
        ]),
    };
    tell_application(reg, &st, alliance_id, me.corporation_id, c);
    Ok(PyValue::None)
}

// ---------------------------------------------------------------------------
// allianceRegistry
// ---------------------------------------------------------------------------

/// Whole days from `start` to `now`, both FILETIMEs; never negative.
fn days_since(start: i64, now: i64) -> i64 {
    (now - start).max(0) / crate::seed::DAY
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    // SOC-C2 first: alliance contacts, labels and relationships
    // (`crate::group_contacts`), given the executor corp of the alliance the
    // call is about, whose directors may edit them.
    // SEC-FIX 1: the executor of the CALLER's own alliance, never of the
    // bound id (the bind's id is client-supplied: a made-up one used to
    // match no alliance and skip the check). `None` refuses every write.
    let executor_corp = req
        .actor
        .and_then(|a| crate::alliance::of_character(reg, a))
        .and_then(|id| {
            Alliances::load(reg.deltas())
                .get(id)
                .and_then(|a| a.executor_corp_id)
        });
    if let Some(answer) = crate::group_contacts::alliance_registry(reg, req, executor_corp) {
        return answer;
    }
    // SOC-D1: the sov arms (claims, prime hour), given the bound alliance.
    let bound = bound_alliance(reg, req);
    if let Some(answer) = super::sov::alliance_registry(reg, req, bound) {
        return answer;
    }
    // C8: declaring war for the alliance, and its bills.
    if let Some(answer) = super::war::alliance_registry(reg, req) {
        return answer;
    }
    match req.method {
        // `(allianceID, corpID)` -> whole days the corp has been in the
        // alliance, compared `>= min days` by the donate panels **[V]**
        // (`corpDonateEMPanel`, `personalDonateEMPanel`). 0 for a corp that
        // is not a member. [A] whole days of wall FILETIME since
        // `Member::start_date`, not game days; falsified by a donate panel
        // that stays closed at the right age at another time scale.
        "GetDaysInAlliance" => {
            let (Some(alliance), Some(corp)) = (
                int_arg(arg(req, 0, "allianceID")),
                int_arg(arg(req, 1, "corporationID")),
            ) else {
                return Err(bad_args(req, "(allianceID, corporationID)"));
            };
            let st = Alliances::load(reg.deltas());
            let days = st
                .member(corp)
                .filter(|m| m.alliance_id == alliance)
                .map_or(0, |m| {
                    days_since(m.start_date, crate::pyutil::filetime_now())
                });
            Ok(PyValue::Int(days))
        }
        // Unbound with the id (`AllianceO.GetAlliance` for another alliance,
        // `ApplyToAllianceWnd`), bound with none for one's own **[V]**.
        "GetAlliance" => {
            let id = int_arg(arg(req, 0, "allianceID")).or_else(|| bound_alliance(reg, req));
            let st = Alliances::load(reg.deltas());
            id.and_then(|id| alliance_row(&st, id, crate::war::permit(reg, id)))
                .map(|r| PyValue::DbRow(Box::new(r)))
                .ok_or_else(|| refuse("AllianceNotFound"))
        }
        "GetAlliancePublicInfo" => {
            let id = int_arg(arg(req, 0, "allianceID")).or_else(|| bound_alliance(reg, req));
            let st = Alliances::load(reg.deltas());
            id.and_then(|id| public_info(&st, id, crate::war::permit(reg, id)))
                .ok_or_else(|| refuse("AllianceNotFound"))
        }
        // The info window's Members tab iterates for `.corporationID`
        // (`GetAllianceMembersSubContent`) **[V]**.
        "GetAllianceMembers" => {
            let id = int_arg(arg(req, 0, "allianceID")).or_else(|| bound_alliance(reg, req));
            let st = Alliances::load(reg.deltas());
            let rows = id
                .map(|id| {
                    st.members_of(id)
                        .iter()
                        .map(|(c, m)| member_values(*c, m))
                        .collect()
                })
                .unwrap_or_default();
            Ok(rowset(MEMBER_COLUMNS, rows, None))
        }
        // The Rankings tab: iterated for `allianceID`, `allianceName`,
        // `executorCorpID`, `shortName`, `startDate`, `memberCount` **[V]**.
        // Open alliances by member count; `maxLen` 0 is "show all".
        "GetRankedAlliances" => {
            let max = int_arg(arg(req, 0, "maxLen")).unwrap_or(100);
            let st = Alliances::load(reg.deltas());
            let mut ranked: Vec<(usize, &Alliance)> = st
                .all()
                .filter(|a| !a.deleted)
                .map(|a| (st.members_of(a.alliance_id).len(), a))
                .collect();
            ranked.sort_by(|x, y| y.0.cmp(&x.0).then(x.1.alliance_id.cmp(&y.1.alliance_id)));
            if max > 0 {
                ranked.truncate(usize::try_from(max).unwrap_or(usize::MAX));
            }
            let rows = ranked
                .into_iter()
                .map(|(n, a)| alliance_values(a, n, crate::war::permit(reg, a.alliance_id)))
                .collect();
            Ok(rowset(ALLIANCE_COLUMNS, rows, None))
        }
        // A corporation's alliance history (`GetAllianceHistorySubContent`):
        // newest first, `allianceID` `None` for "left", and **the last entry
        // is the corporation's founding**, read only for `.startDate` **[V]**.
        // `.deleted` marks an alliance since closed. Empty for an NPC corp.
        "GetEmploymentRecord" => {
            let corp = int_arg(arg(req, 0, "corporationID"));
            let st = Alliances::load(reg.deltas());
            let founded = corp.and_then(|c| reg.corporations.get(c).map(|c| c.create_date));
            let Some((corp, founded)) = corp.zip(founded) else {
                return Ok(PyValue::List(vec![]));
            };
            let mut rows: Vec<PyValue> = st
                .history_of(corp)
                .into_iter()
                .map(|s| {
                    let deleted = s
                        .alliance_id
                        .and_then(|a| st.get(a))
                        .is_some_and(|a| a.deleted);
                    keyval(vec![
                        ("allianceID", opt(s.alliance_id)),
                        ("startDate", PyValue::Int(s.date)),
                        ("deleted", PyValue::Bool(deleted)),
                    ])
                })
                .collect();
            rows.push(keyval(vec![
                ("allianceID", PyValue::None),
                ("startDate", PyValue::Int(founded)),
                ("deleted", PyValue::Bool(false)),
            ]));
            Ok(PyValue::List(rows))
        }
        // Bound. `AllianceMembersO.GetMembers` caches a `CIndexedRowset`
        // keyed by `corporationID` **[V]**.
        "GetMembers" => {
            let st = Alliances::load(reg.deltas());
            let rows = bound_alliance(reg, req)
                .map(|id| {
                    st.members_of(id)
                        .iter()
                        .map(|(c, m)| member_values(*c, m))
                        .collect()
                })
                .unwrap_or_default();
            Ok(rowset(MEMBER_COLUMNS, rows, Some("corporationID")))
        }
        // Bound. `AllianceApplicationsO.GetApplications`: keyed by
        // `corporationID`, rejected ones included (the client filters)
        // **[V]**. For a member corp's director only.
        "GetApplications" => {
            let st = Alliances::load(reg.deltas());
            let alliance = bound_alliance(reg, req);
            let allowed = director(reg, req.actor)
                .ok()
                .is_some_and(|me| st.member(me.corporation_id).map(|m| m.alliance_id) == alliance);
            let rows = match alliance.filter(|_| allowed) {
                Some(id) => st
                    .applications_to(id)
                    .into_iter()
                    .map(application_values)
                    .collect(),
                None => Vec::new(),
            };
            Ok(rowset(APPLICATION_COLUMNS, rows, Some("corporationID")))
        }
        "UpdateApplication" => update_application(reg, req),
        "DeclareExecutorSupport" => declare_support(reg, req),
        "DeleteMember" => delete_member(reg, req),
        "UpdateAlliance" => update_alliance(reg, req),
        // The Bulletins tab, iterated (`GetAllianceBulletins`) **[V]**, and
        // its writes: `crate::bulletins` (SMALL-SOCIAL).
        "GetBulletins" | "AddBulletin" | "DeleteBulletin" | "UpdateBulletinOrder" => {
            let owner = bound_alliance(reg, req);
            crate::bulletins::call(reg, req, crate::bulletins::Scope::Alliance, owner)
                .unwrap_or_else(|| Err(reg.no_method(req)))
        }
        _ => Err(reg.no_method(req)),
    }
}

/// `UpdateApplication(corpID, applicationText, state)` from
/// `AllianceViewApplication`, offered for a new application only, with
/// `allianceApplicationAccepted` or `...Rejected` **[V]**.
fn update_application(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let corp =
        int_arg(arg(req, 0, "corpID")).ok_or_else(|| bad_args(req, "corpID is not an id"))?;
    let text = text_arg(arg(req, 1, "applicationText"));
    let state =
        int_arg(arg(req, 2, "state")).ok_or_else(|| bad_args(req, "state is not an int"))?;
    if !matches!(state, APP_ACCEPTED | APP_REJECTED) {
        return Err(bad_args(req, "state is neither accepted nor rejected"));
    }
    let mut st = Alliances::load(reg.deltas());
    let (_, alliance_id) = executor_director(reg, req, &st)?;
    if state == APP_ACCEPTED {
        // Not with wars running that the alliance would inherit or fight (C8).
        crate::war_deal::may_join_alliance(reg, corp, alliance_id, true).map_err(refuse)?;
    }
    let now = crate::pyutil::filetime_now();
    let done = st
        .answer(alliance_id, corp, state, text.as_deref(), now)
        .map_err(refuse)?;
    st.save(reg.deltas_mut());
    let c = change(vec![
        ("corporationID", PyValue::Int(corp), PyValue::Int(corp)),
        (
            "applicationText",
            PyValue::Unicode(done.before.text.clone()),
            PyValue::Unicode(done.after.text.clone()),
        ),
        (
            "state",
            PyValue::Int(done.before.state),
            PyValue::Int(done.after.state),
        ),
    ]);
    tell_application(reg, &st, alliance_id, corp, c);
    if done.joined {
        joined(reg, &st, alliance_id, corp);
    }
    Ok(PyValue::None)
}

/// A corp is now a member: its pilots' session and field, and every
/// member's member list.
fn joined(reg: &mut Registry, st: &Alliances, alliance_id: i64, corp: i64) {
    set_corp_alliance(reg, corp, Some(alliance_id));
    // The wars declared on the corp are the alliance's now (C8).
    crate::war_deal::on_corp_joined_alliance(reg, corp, alliance_id);
    if let Some(m) = st.member(corp) {
        let to = alliance_characters(reg, st, alliance_id);
        let c = whole_row_change(MEMBER_COLUMNS, member_values(corp, &m), true);
        notify(
            reg,
            to,
            "OnAllianceMemberChanged",
            vec![PyValue::Int(alliance_id), PyValue::Int(corp), c],
        );
    }
}

/// `DeclareExecutorSupport(corpID)`: the caller's corp backs `corpID`
/// (`all_ui_members.DeclareSupportForm`, directors only) **[V]**.
fn declare_support(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let me = director(reg, req.actor)?;
    let candidate =
        int_arg(arg(req, 0, "corpID")).ok_or_else(|| bad_args(req, "corpID is not an id"))?;
    let mut st = Alliances::load(reg.deltas());
    let done = st
        .declare_support(me.corporation_id, candidate, crate::pyutil::filetime_now())
        .map_err(refuse)?;
    st.save(reg.deltas_mut());
    let Some(alliance_id) = st.member(me.corporation_id).map(|m| m.alliance_id) else {
        return Ok(PyValue::None);
    };
    let to = alliance_characters(reg, &st, alliance_id);
    let c = change(vec![
        (
            "corporationID",
            PyValue::Int(me.corporation_id),
            PyValue::Int(me.corporation_id),
        ),
        (
            "chosenExecutorID",
            opt(done.before),
            PyValue::Int(candidate),
        ),
    ]);
    notify(
        reg,
        to,
        "OnAllianceMemberChanged",
        vec![
            PyValue::Int(alliance_id),
            PyValue::Int(me.corporation_id),
            c,
        ],
    );
    if let Some(ex) = done.executor_change {
        tell_executor(reg, &st, alliance_id, ex);
    }
    Ok(PyValue::None)
}

/// `DeleteMember(corpID)`: "Quit Alliance" on one's own corp, "Kick Member"
/// on another (`all_ui_members.GetMemberMenu`) **[V]**. Quitting needs a
/// director of the corp; kicking a director of the executor **[A]**.
fn delete_member(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let target =
        int_arg(arg(req, 0, "corpID")).ok_or_else(|| bad_args(req, "corpID is not an id"))?;
    let mut st = Alliances::load(reg.deltas());
    let me = director(reg, req.actor)?;
    if target != me.corporation_id {
        let (_, mine) = executor_director(reg, req, &st)?;
        if st.member(target).map(|m| m.alliance_id) != Some(mine) {
            return Err(refuse("CantConfigureNotInAlliance"));
        }
    }
    let now = crate::pyutil::filetime_now();
    let left = st.leave(target, now).map_err(refuse)?;
    st.save(reg.deltas_mut());
    let alliance_id = left.alliance_id;
    // The remaining members, and the leaver's own pilots, whose cached list
    // drops on the session change anyway.
    let mut to = alliance_characters(reg, &st, alliance_id);
    to.extend(reg.corporations.members_of(target));
    let c = whole_row_change(MEMBER_COLUMNS, member_values(target, &left.member), false);
    notify(
        reg,
        to,
        "OnAllianceMemberChanged",
        vec![PyValue::Int(alliance_id), PyValue::Int(target), c],
    );
    if let Some(ex) = left.executor_change {
        tell_executor(reg, &st, alliance_id, ex);
    }
    set_corp_alliance(reg, target, None);
    // C8: the last corp out closes the alliance and its wars end; otherwise
    // the leaver takes the alliance's wars with it.
    if left.closed {
        crate::war::on_alliance_closed(reg, alliance_id);
    } else {
        crate::war_deal::on_corp_left_alliance(reg, target, alliance_id);
    }
    Ok(PyValue::None)
}

/// `UpdateAlliance(description, url)` from `EditAllianceWnd`, offered to
/// directors **[V]**; the executor's **[A]**, as for applications.
fn update_alliance(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let description = text_arg(arg(req, 0, "description")).unwrap_or_default();
    let url = text_arg(arg(req, 1, "url")).unwrap_or_default();
    let mut st = Alliances::load(reg.deltas());
    let (_, alliance_id) = executor_director(reg, req, &st)?;
    let (old_description, old_url) = st.update(alliance_id, &description, &url).map_err(refuse)?;
    st.save(reg.deltas_mut());
    // What was stored, after the caps (SEC-FIX 4), not what was sent.
    let (description, url) = st
        .get(alliance_id)
        .map(|a| (a.description.clone(), a.url.clone()))
        .unwrap_or_default();
    let to = alliance_characters(reg, &st, alliance_id);
    let c = change(vec![
        (
            "allianceID",
            PyValue::Int(alliance_id),
            PyValue::Int(alliance_id),
        ),
        (
            "description",
            PyValue::Unicode(old_description),
            PyValue::Unicode(description),
        ),
        ("url", PyValue::Unicode(old_url), PyValue::Unicode(url)),
    ]);
    notify(
        reg,
        to,
        "OnAllianceChanged",
        vec![PyValue::Int(alliance_id), c],
    );
    Ok(PyValue::None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Character;

    const ALICE: i64 = 90_000_201;
    const BOB: i64 = 90_000_202;
    const CAROL: i64 = 90_000_203;

    fn registry() -> Registry {
        crate::testkit::registry_with(|c| {
            c.characters = vec![
                Character {
                    character_id: ALICE,
                    name: "Alice".to_owned(),
                    ship_id: 100_000_201,
                    balance: 5_000_000_000.0,
                    ..Character::default()
                },
                Character {
                    character_id: BOB,
                    name: "Bob".to_owned(),
                    ship_id: 100_000_202,
                    balance: 5_000_000.0,
                    ..Character::default()
                },
                Character {
                    character_id: CAROL,
                    name: "Carol".to_owned(),
                    ship_id: 100_000_203,
                    balance: 5_000_000.0,
                    ..Character::default()
                },
            ];
        })
    }

    fn call(
        reg: &mut Registry,
        service: &str,
        method: &str,
        actor: i64,
        args: Vec<PyValue>,
        bind: Option<&PyValue>,
    ) -> Result<PyValue, ServiceError> {
        let req = Request {
            service,
            method,
            args: &args,
            kwargs: &[],
            bind_params: bind,
            actor: Some(actor),
        };
        reg.dispatch(&req)
    }

    fn uni(s: &str) -> PyValue {
        PyValue::Unicode(s.to_owned())
    }

    /// The message key of a raised `UserError`, or the error's text.
    fn key_of(e: &ServiceError) -> String {
        if let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e
            && let PyValue::Tuple(parts) = header.as_ref()
            && let Some(PyValue::Tuple(args)) = parts.get(1)
            && let Some(PyValue::Bytes(b)) = args.first()
        {
            return String::from_utf8_lossy(b).into_owned();
        }
        format!("{e}")
    }

    /// A player corp with `who` as CEO, through the real arm.
    fn found_corp(reg: &mut Registry, who: i64, name: &str, ticker: &str) -> i64 {
        call(
            reg,
            "corpRegistry",
            "AddCorporation",
            who,
            vec![uni(name), uni(ticker), uni(""), uni("")],
            None,
        )
        .expect("founds a corp");
        reg.corporations
            .membership(who)
            .expect("in the new corp")
            .corporation_id
    }

    fn changes_for(reg: &mut Registry, who: i64) -> Vec<Vec<(PyValue, PyValue)>> {
        reg.take_session_changes()
            .into_iter()
            .filter(|(id, _)| *id == who)
            .map(|(_, c)| c)
            .collect()
    }

    fn alliance_in(change: &[(PyValue, PyValue)]) -> Option<PyValue> {
        crate::pyutil::pairs_get(change, b"allianceid").cloned()
    }

    /// `CreateAlliance`, with the CEO's corporation Master Wallet holding the
    /// fee first (CORP-W: the fee comes out of the corporation's wallet).
    fn create(reg: &mut Registry, who: i64) -> Result<PyValue, ServiceError> {
        if let Some(m) = reg.corporations.membership(who) {
            crate::corp_wallet::adjust(
                reg.deltas_mut(),
                m.corporation_id,
                crate::corp_wallet::MASTER_KEY,
                crate::corp_wallet::to_cents(ALLIANCE_CREATION_COST),
            )
            .expect("a division takes a credit");
        }
        create_poor(reg, who)
    }

    /// `CreateAlliance` as it stands, whatever the wallet holds.
    fn create_poor(reg: &mut Registry, who: i64) -> Result<PyValue, ServiceError> {
        call(
            reg,
            "corpRegistry",
            "CreateAlliance",
            who,
            vec![uni("Invented Pact"), uni("INVP"), uni("about"), uni("")],
            None,
        )
    }

    #[test]
    fn a_ceo_creates_an_alliance_pays_and_gets_the_session_key() {
        let mut reg = registry();
        let corp = found_corp(&mut reg, ALICE, "Alpha Invented", "ALPH");
        let _ = reg.take_session_changes();
        let before = reg.character(ALICE).map(|c| c.balance).unwrap_or(0.0);
        create(&mut reg, ALICE).expect("creates");
        // CORP-W: the corporation pays, not the CEO.
        let after = reg.character(ALICE).map(|c| c.balance).unwrap_or(0.0);
        assert_eq!(before, after, "the CEO's own wallet is untouched");
        assert_eq!(
            crate::corp_wallet::balance(reg.deltas(), corp, crate::corp_wallet::MASTER_KEY),
            0,
            "the corporation's Master Wallet held exactly the fee"
        );
        let id = crate::alliance::of_corp(reg.deltas(), corp).expect("a member");
        assert_eq!(reg.character(ALICE).and_then(|c| c.alliance_id), Some(id));
        let changes = changes_for(&mut reg, ALICE);
        assert_eq!(
            changes.last().and_then(|c| alliance_in(c)),
            Some(PyValue::Tuple(vec![PyValue::None, PyValue::Int(id)]))
        );
        // The bound row reads back.
        let bind = PyValue::Tuple(vec![PyValue::Int(id), PyValue::Int(1)]);
        let row = call(
            &mut reg,
            "allianceRegistry",
            "GetAlliance",
            ALICE,
            vec![],
            Some(&bind),
        )
        .expect("answers");
        assert!(matches!(row, PyValue::DbRow(_)));
        // The owner and the short name are known to the client's caches.
        assert_eq!(
            crate::alliance::name_of(reg.deltas(), id).as_deref(),
            Some("Invented Pact")
        );
    }

    #[test]
    fn create_refusals_use_the_clients_keys() {
        let mut reg = registry();
        // Not a CEO of a player corp.
        let e = create(&mut reg, BOB).expect_err("refused");
        assert!(
            key_of(&e).contains("OnlyActiveCEOCanCreateAlliance"),
            "{e:?}"
        );
        // A CEO whose corporation cannot pay, however rich the CEO is.
        found_corp(&mut reg, BOB, "Bravo Invented", "BRAV");
        let e = create_poor(&mut reg, BOB).expect_err("refused");
        assert!(key_of(&e).contains("NotEnoughMoneyCorp"), "{e:?}");
        // Twice.
        found_corp(&mut reg, ALICE, "Alpha Invented", "ALPH");
        create(&mut reg, ALICE).expect("creates");
        let e = create(&mut reg, ALICE).expect_err("refused");
        assert!(
            key_of(&e).contains("AllianceCreateFailCorpInAlliance"),
            "{e:?}"
        );
    }

    #[test]
    fn a_second_corp_applies_is_accepted_and_its_pilots_join() {
        let mut reg = registry();
        let a = found_corp(&mut reg, ALICE, "Alpha Invented", "ALPH");
        let b = found_corp(&mut reg, BOB, "Bravo Invented", "BRAV");
        create(&mut reg, ALICE).expect("creates");
        let id = crate::alliance::of_corp(reg.deltas(), a).expect("a member");
        let _ = reg.take_session_changes();
        let _ = reg.take_pending();

        call(
            &mut reg,
            "corpRegistry",
            "ApplyToJoinAlliance",
            BOB,
            vec![PyValue::Int(id), uni("let us in")],
            None,
        )
        .expect("applies");
        // Alice (the executor's CEO) sees it on the bound object.
        let bind = PyValue::Tuple(vec![PyValue::Int(id), PyValue::Int(1)]);
        let apps = call(
            &mut reg,
            "allianceRegistry",
            "GetApplications",
            ALICE,
            vec![],
            Some(&bind),
        )
        .expect("answers");
        assert!(format!("{apps:?}").contains("let us in"));
        // Bob may not answer his own application.
        let e = call(
            &mut reg,
            "allianceRegistry",
            "UpdateApplication",
            BOB,
            vec![PyValue::Int(b), uni(""), PyValue::Int(APP_ACCEPTED)],
            Some(&bind),
        )
        .expect_err("refused");
        assert!(key_of(&e).contains("CantConfigureNotInAlliance"), "{e:?}");

        call(
            &mut reg,
            "allianceRegistry",
            "UpdateApplication",
            ALICE,
            vec![PyValue::Int(b), uni("welcome"), PyValue::Int(APP_ACCEPTED)],
            Some(&bind),
        )
        .expect("accepts");
        assert_eq!(crate::alliance::of_corp(reg.deltas(), b), Some(id));
        assert_eq!(reg.character(BOB).and_then(|c| c.alliance_id), Some(id));
        let changes = changes_for(&mut reg, BOB);
        assert_eq!(
            changes.last().and_then(|c| alliance_in(c)),
            Some(PyValue::Tuple(vec![PyValue::None, PyValue::Int(id)]))
        );
        let members = call(
            &mut reg,
            "allianceRegistry",
            "GetMembers",
            ALICE,
            vec![],
            Some(&bind),
        )
        .expect("answers");
        let text = format!("{members:?}");
        assert!(text.contains(&a.to_string()) && text.contains(&b.to_string()));

        // Bob's corp quits: the key goes back to None.
        call(
            &mut reg,
            "allianceRegistry",
            "DeleteMember",
            BOB,
            vec![PyValue::Int(b)],
            Some(&bind),
        )
        .expect("quits");
        assert_eq!(reg.character(BOB).and_then(|c| c.alliance_id), None);
        let changes = changes_for(&mut reg, BOB);
        assert_eq!(
            changes.last().and_then(|c| alliance_in(c)),
            Some(PyValue::Tuple(vec![PyValue::Int(id), PyValue::None]))
        );
        // The history shows the stint, newest first, then the founding.
        let h = call(
            &mut reg,
            "allianceRegistry",
            "GetEmploymentRecord",
            ALICE,
            vec![PyValue::Int(b)],
            None,
        )
        .expect("answers");
        match h {
            PyValue::List(v) => assert_eq!(v.len(), 3),
            other => panic!("not a list: {other:?}"),
        }
    }

    /// Alice's corp (the executor) and Bob's in one alliance, through the
    /// real arms. Returns `(a, b, alliance_id)`.
    fn two_corp_alliance(reg: &mut Registry) -> (i64, i64, i64) {
        let a = found_corp(reg, ALICE, "Alpha Invented", "ALPH");
        let b = found_corp(reg, BOB, "Bravo Invented", "BRAV");
        create(reg, ALICE).expect("creates");
        let id = crate::alliance::of_corp(reg.deltas(), a).expect("a member");
        call(
            reg,
            "corpRegistry",
            "ApplyToJoinAlliance",
            BOB,
            vec![PyValue::Int(id), uni("")],
            None,
        )
        .expect("applies");
        let bind = PyValue::Tuple(vec![PyValue::Int(id), PyValue::Int(1)]);
        call(
            reg,
            "allianceRegistry",
            "UpdateApplication",
            ALICE,
            vec![PyValue::Int(b), uni(""), PyValue::Int(APP_ACCEPTED)],
            Some(&bind),
        )
        .expect("accepts");
        (a, b, id)
    }

    /// SEC-FIX 1: a non-executor director edits the alliance contact book
    /// with a made-up bind id. The executor check used the bound id's
    /// executor, which was `None` for an id no alliance has: check skipped.
    #[test]
    fn a_non_executor_director_cannot_edit_the_alliance_book_through_a_made_up_bind() {
        let mut reg = registry();
        let (_, _, id) = two_corp_alliance(&mut reg);
        // Carol is a real owner, so only the executor check can refuse.
        let args = vec![PyValue::Int(CAROL), PyValue::Int(10)];
        for bind_id in [id + 5_000, id] {
            let bind = PyValue::Tuple(vec![PyValue::Int(bind_id), PyValue::Int(1)]);
            let r = call(
                &mut reg,
                "allianceRegistry",
                "AddAllianceContact",
                BOB,
                args.clone(),
                Some(&bind),
            );
            assert!(r.is_err(), "bind {bind_id}: {r:?}");
        }
        let bind = PyValue::Tuple(vec![PyValue::Int(id + 5_000), PyValue::Int(1)]);
        call(
            &mut reg,
            "allianceRegistry",
            "AddAllianceContact",
            ALICE,
            args,
            Some(&bind),
        )
        .expect("the executor's director still can");
    }

    #[test]
    fn a_pilot_joining_a_corp_in_an_alliance_takes_the_alliance_with_them() {
        let mut reg = registry();
        let a = found_corp(&mut reg, ALICE, "Alpha Invented", "ALPH");
        create(&mut reg, ALICE).expect("creates");
        let id = crate::alliance::of_corp(reg.deltas(), a).expect("a member");
        call(
            &mut reg,
            "corpRegistry",
            "InsertInvitation",
            ALICE,
            vec![PyValue::Int(CAROL)],
            None,
        )
        .expect("invites");
        let app = reg
            .corporations
            .applications
            .values()
            .find(|x| x.character_id == CAROL)
            .map(|x| x.application_id)
            .expect("an invitation");
        let _ = reg.take_session_changes();
        call(
            &mut reg,
            "corpRegistry",
            "UpdateApplicationOffer",
            CAROL,
            // `(applicationID, characterID, corporationID, text, status)`,
            // status 2 = accepted by the character.
            vec![
                PyValue::Int(app),
                PyValue::Int(CAROL),
                PyValue::Int(a),
                uni(""),
                PyValue::Int(2),
            ],
            None,
        )
        .expect("accepts the invitation");
        assert_eq!(reg.character(CAROL).and_then(|c| c.alliance_id), Some(id));
        let changes = changes_for(&mut reg, CAROL);
        assert!(
            changes
                .iter()
                .any(|c| alliance_in(c).is_some_and(|v| format!("{v:?}").contains(&id.to_string()))),
            "{changes:?}"
        );
    }

    #[test]
    fn unknown_alliances_and_strangers_are_refused_cleanly() {
        let mut reg = registry();
        let e = call(
            &mut reg,
            "allianceRegistry",
            "GetAlliance",
            ALICE,
            vec![PyValue::Int(crate::alliance::MIN_PLAYER_ALLIANCE_ID + 77)],
            None,
        )
        .expect_err("refused");
        assert!(key_of(&e).contains("AllianceNotFound"), "{e:?}");
        let ranked = call(
            &mut reg,
            "allianceRegistry",
            "GetRankedAlliances",
            ALICE,
            vec![PyValue::Int(0)],
            None,
        )
        .expect("answers");
        assert!(!matches!(ranked, PyValue::None));
        let e = call(
            &mut reg,
            "allianceRegistry",
            "DeclareExecutorSupport",
            ALICE,
            vec![PyValue::Int(1)],
            None,
        )
        .expect_err("refused");
        assert!(key_of(&e).contains("CrpAccessDenied"), "{e:?}");
    }

    #[test]
    fn alliances_survive_the_world_file() {
        let mut reg = registry();
        let a = found_corp(&mut reg, ALICE, "Alpha Invented", "ALPH");
        create(&mut reg, ALICE).expect("creates");
        let id = crate::alliance::of_corp(reg.deltas(), a).expect("a member");
        let dir = crate::testkit::scratch_dir("alliance-world");
        let store = crate::store::FileStore::new(dir.path().join("world.txt"));
        crate::store::Store::save(&store, &reg.to_snapshot()).expect("write the world");
        let back = crate::store::Store::load(&store)
            .expect("read the world")
            .expect("a world was written");
        let mut fresh = registry();
        assert_eq!(fresh.character(ALICE).and_then(|c| c.alliance_id), None);
        fresh.apply_snapshot(back);
        assert_eq!(fresh.character(ALICE).and_then(|c| c.alliance_id), Some(id));
        assert_eq!(crate::alliance::of_corp(fresh.deltas(), a), Some(id));
    }
}

#[cfg(test)]
mod days_tests {
    use super::days_since;

    #[test]
    fn whole_days_never_negative() {
        let day = crate::seed::DAY;
        assert_eq!(days_since(0, 3 * day + 5), 3);
        assert_eq!(days_since(0, day - 1), 0);
        assert_eq!(days_since(10 * day, day), 0);
    }
}
