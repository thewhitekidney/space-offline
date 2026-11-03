//! **The fleet finder and the fleet watchlist** (GAPS-2, X-GAPS item 6;
//! lane note `docs/lanes/gaps-2.md`).
//!
//! The fleets themselves are in memory only (`crate::fleet`: "nothing is
//! persisted"), so everything here is too: adverts, join requests and the
//! watchlist are *about* a fleet and die with it. They live in a world slot
//! ([`SLOT`]), not in deltas -- a delta that outlived its fleet would be a
//! row for a thing that is gone.
//!
//! # The finder -- `fleetProxy` (`fleetSvc.py:2131-2697`) [V]
//!
//! | call | client | answer |
//! |---|---|---|
//! | `AddFleetFinderAdvert(adInfoData)` | `:2675` | the advert dict (own form), or `None` |
//! | `RemoveFleetFinderAdvert()` | `:2684` | the removed advert, or `None` |
//! | `GetMyFleetFinderAdvert()` | `:2689` | the advert, or `None` |
//! | `UpdateAdvertInfo(numMembers, allowedDiff)` | `:2693` | the advert, or `None` |
//! | `UpdateAdvertAllowedEntities(info)` | `:2697` | the advert, or `None` |
//! | `UpdateFleetAdvertWithNewLeader(info)` | `:2680` | the advert, or `None` |
//! | `GetAvailableFleetAds()` | `:2131` | `{fleetID: advert}` |
//! | `ApplyToJoinFleet(fleetID, autoAccept)` | `:2149` | truthy raises `FleetApplicationReceived` client-side (`:2150-2151`) |
//!
//! Every answer that is an advert is built into a `FleetAdvertObject(**d)`
//! (`:2677`), whose constructor `setattr`s each key (`fleetAdvertObject.
//! BaseFleetAdvertObject.__init__:22-23`) [V], so the dict's keys are the
//! attribute names. The 24 of them are EvEJS's `buildAdvertDict`, from
//! retail recordings **[I]** (facts only); each is a property or plain
//! attribute of that class [V]: `fleetID`, `leader` (a `KeyVal` of `charID`,
//! `corpID`, `allianceID`, `warFactionID`, `securityStatus`), `solarSystemID`,
//! `numMembers`, `advertTime`, `dateCreated`, `fleetName`, `description`,
//! `inviteScope`, `activityValue`, `useAdvanceOptions`, `newPlayerFriendly`
//! and the advanced options (`public_*`, `membergroups_*`, `joinNeedsApproval`,
//! `hideInfo`, `updateOnBossChange`, `advertJoinLimit`). The four entity sets
//! are Python `set`s in the own-advert form and `None` in the list form
//! (EvEJS from a recording **[I]**): the client reads sets off its own
//! advert only (`fleetFinder_allowedEntities`).
//!
//! **What this server decides [A]:**
//!
//! - who may list a fleet: its boss, in a fleet that exists;
//! - who sees an advert in the list: a closed advert (scope 0) to nobody but
//!   its boss; corp / alliance scopes to the leader's corporation or
//!   alliance; a public scope (8 or 16) to everybody. Standing and security
//!   minimums, militia scope and the entity sets are kept and sent back but
//!   not applied;
//! - `ApplyToJoinFleet`: an advert that needs approval makes a join request
//!   (`OnFleetJoinRequest` to the boss, `GetJoinRequests`, `RejectJoinRequest`;
//!   the boss accepts by inviting, as the client's request window does) and
//!   answers truthy; any other advert invites the applicant at once and
//!   answers falsy, and the client's `expectingInvite` accepts it
//!   (`:2148`).
//!
//! Falsified by a retail capture of the finder list seen by someone the
//! scope should hide.
//!
//! # The watchlist -- `fleetMgr.AddToWatchlist(newChars, current)` and
//! `RemoveFromWatchlist(charID, current)` (`:764`, `:814`) [V]
//!
//! The client keeps the panel; the server keeps the set so it can send the
//! members' health to the right pilots (there is no damage feed yet: the
//! rows are kept, not used). `ForceLeaveFleet()` (`:433`) is the client
//! leaving a fleet it thinks it is not in: the server leaves it.

use crate::notify::Push;
use crate::pyutil::{keyval, pydict};
use crate::service::{Registry, ServiceError};
use eve_marshal::{ObjectForm, PyValue};
use std::collections::{BTreeMap, BTreeSet};

/// `evefleet/const`: `INVITE_CLOSED` 0, `INVITE_CORP` 1, `INVITE_ALLIANCE` 2,
/// `INVITE_MILITIA` 4, `INVITE_PUBLIC` 8, `INVITE_PUBLIC_OPEN` 16 [V].
pub const INVITE_CLOSED: i64 = 0;
pub const INVITE_CORP: i64 = 1;
pub const INVITE_ALLIANCE: i64 = 2;
pub const INVITE_PUBLIC: i64 = 8;
pub const INVITE_PUBLIC_OPEN: i64 = 16;
/// `FLEETNAME_MAXLENGTH` and `FLEETDESC_MAXLENGTH` [V].
pub const NAME_MAX: usize = 32;
pub const DESC_MAX: usize = 250;
/// `fleetAdvertObject.MAX_ALLOWED_ENTITIES` [V].
pub const MAX_ENTITIES: usize = 1_000;
/// `evefleet.MAX_DAMAGE_SENDERS`, the watchlist's size [V].
pub const MAX_WATCHED: usize = 100;

/// The world slot. **Coordinator-only** registration (`world_ext::SLOTS`).
pub const SLOT: crate::world_ext::SlotSpec = crate::world_ext::SlotSpec {
    name: "fleet_finder",
    make: |_| Box::new(Finder::default()),
};

/// The boss, as an advert shows them.
#[derive(Debug, Clone, PartialEq)]
pub struct Leader {
    pub char_id: i64,
    pub corp_id: i64,
    pub alliance_id: Option<i64>,
    pub war_faction_id: Option<i64>,
    pub security_status: f64,
}

/// One fleet's advert. The entity sets and standings are kept as sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Advert {
    pub fleet_id: i64,
    pub leader: Leader,
    pub solar_system_id: i64,
    pub num_members: i64,
    pub advert_time: i64,
    pub date_created: i64,
    pub fleet_name: String,
    pub description: String,
    pub invite_scope: i64,
    pub activity_value: Option<i64>,
    pub use_advance_options: bool,
    pub new_player_friendly: bool,
    pub public_min_standing: Option<f64>,
    pub public_min_security: Option<f64>,
    pub public_allowed: BTreeSet<i64>,
    pub public_disallowed: BTreeSet<i64>,
    pub membergroups_min_standing: Option<f64>,
    pub membergroups_min_security: Option<f64>,
    pub membergroups_allowed: BTreeSet<i64>,
    pub membergroups_disallowed: BTreeSet<i64>,
    pub join_needs_approval: bool,
    pub hide_info: bool,
    pub update_on_boss_change: bool,
    pub advert_join_limit: Option<i64>,
}

/// A join request waiting for the boss.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinRequest {
    pub char_id: i64,
    pub corp_id: i64,
}

/// Every advert, request and watchlist.
#[derive(Debug, Default)]
pub struct Finder {
    adverts: BTreeMap<i64, Advert>,
    requests: BTreeMap<i64, BTreeMap<i64, JoinRequest>>,
    watch: BTreeMap<i64, BTreeSet<i64>>,
}

// ---------------------------------------------------------------------------
// Reading the client's dict
// ---------------------------------------------------------------------------

/// The value of `key` in a dict or a `KeyVal`/instance carrying one.
fn field<'a>(v: &'a PyValue, key: &str) -> Option<&'a PyValue> {
    match v {
        PyValue::Instance { state, .. } => crate::pyutil::dict_get(state, key.as_bytes()),
        other => crate::pyutil::dict_get(other, key.as_bytes()),
    }
}

fn int_of(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        PyValue::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

fn real_of(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Int(n) => Some(*n as f64),
        PyValue::Real(r) => Some(*r),
        _ => None,
    }
}

fn bool_of(v: Option<&PyValue>, default: bool) -> bool {
    match v {
        Some(PyValue::Bool(b)) => *b,
        Some(PyValue::Int(n)) => *n != 0,
        _ => default,
    }
}

fn text_of(v: Option<&PyValue>, max: usize) -> String {
    let s = match v {
        Some(PyValue::Unicode(s)) => s.clone(),
        Some(PyValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
        _ => String::new(),
    };
    s.chars().take(max).collect()
}

/// The ints of a Python `set` (tag 0x22 around `__builtin__.set`), a list or
/// a tuple; `None` for `None` or anything else.
fn set_of(v: Option<&PyValue>) -> Option<BTreeSet<i64>> {
    fn ints(v: &PyValue, out: &mut BTreeSet<i64>) {
        match v {
            PyValue::Int(i) => {
                out.insert(*i);
            }
            PyValue::List(items) | PyValue::Tuple(items) => {
                for item in items {
                    ints(item, out);
                }
            }
            PyValue::ObjectEx { header, .. } => ints(header, out),
            _ => {}
        }
    }
    match v? {
        PyValue::None => None,
        other => {
            let mut out = BTreeSet::new();
            ints(other, &mut out);
            Some(out.into_iter().take(MAX_ENTITIES).collect())
        }
    }
}

/// A Python `set` of ints, the way `fleetAdvertObject` and EvEJS's
/// `buildPythonSet` have it.
pub(crate) fn py_set(values: &BTreeSet<i64>) -> PyValue {
    PyValue::ObjectEx {
        form: ObjectForm::Call,
        header: Box::new(PyValue::Tuple(vec![
            PyValue::Global(b"__builtin__.set".to_vec()),
            PyValue::Tuple(vec![PyValue::List(
                values.iter().map(|i| PyValue::Int(*i)).collect(),
            )]),
        ])),
        list: Vec::new(),
        dict: Vec::new(),
    }
}

impl Advert {
    /// The advert as the client builds a `FleetAdvertObject` from it.
    /// `sets` is whether the four entity sets are sent as sets (the
    /// own-advert form) or as `None` (the list form).
    pub fn to_value(&self, sets: bool) -> PyValue {
        let opt_i = |v: Option<i64>| v.map_or(PyValue::None, PyValue::Int);
        let opt_r = |v: Option<f64>| v.map_or(PyValue::None, PyValue::Real);
        let entities = |s: &BTreeSet<i64>| if sets { py_set(s) } else { PyValue::None };
        pydict(vec![
            ("fleetID", PyValue::Int(self.fleet_id)),
            (
                "leader",
                keyval(vec![
                    ("charID", PyValue::Int(self.leader.char_id)),
                    ("corpID", PyValue::Int(self.leader.corp_id)),
                    ("allianceID", opt_i(self.leader.alliance_id)),
                    ("warFactionID", opt_i(self.leader.war_faction_id)),
                    ("securityStatus", PyValue::Real(self.leader.security_status)),
                ]),
            ),
            ("solarSystemID", PyValue::Int(self.solar_system_id)),
            ("numMembers", PyValue::Int(self.num_members)),
            ("advertTime", PyValue::Int(self.advert_time)),
            ("dateCreated", PyValue::Int(self.date_created)),
            ("fleetName", PyValue::Unicode(self.fleet_name.clone())),
            ("description", PyValue::Unicode(self.description.clone())),
            ("inviteScope", PyValue::Int(self.invite_scope)),
            ("activityValue", opt_i(self.activity_value)),
            ("useAdvanceOptions", PyValue::Bool(self.use_advance_options)),
            ("newPlayerFriendly", PyValue::Bool(self.new_player_friendly)),
            ("public_minStanding", opt_r(self.public_min_standing)),
            ("public_minSecurity", opt_r(self.public_min_security)),
            ("public_allowedEntities", entities(&self.public_allowed)),
            (
                "public_disallowedEntities",
                entities(&self.public_disallowed),
            ),
            (
                "membergroups_minStanding",
                opt_r(self.membergroups_min_standing),
            ),
            (
                "membergroups_minSecurity",
                opt_r(self.membergroups_min_security),
            ),
            (
                "membergroups_allowedEntities",
                entities(&self.membergroups_allowed),
            ),
            (
                "membergroups_disallowedEntities",
                entities(&self.membergroups_disallowed),
            ),
            ("joinNeedsApproval", PyValue::Bool(self.join_needs_approval)),
            ("hideInfo", PyValue::Bool(self.hide_info)),
            (
                "updateOnBossChange",
                PyValue::Bool(self.update_on_boss_change),
            ),
            ("advertJoinLimit", opt_i(self.advert_join_limit)),
        ])
    }

    /// Take the advanced options (and nothing else) from the client's dict.
    fn read_options(&mut self, v: &PyValue) {
        if let Some(x) = set_of(field(v, "public_allowedEntities")) {
            self.public_allowed = x;
        }
        if let Some(x) = set_of(field(v, "public_disallowedEntities")) {
            self.public_disallowed = x;
        }
        if let Some(x) = set_of(field(v, "membergroups_allowedEntities")) {
            self.membergroups_allowed = x;
        }
        if let Some(x) = set_of(field(v, "membergroups_disallowedEntities")) {
            self.membergroups_disallowed = x;
        }
    }

    /// Take everything the creator sets from `adInfoObject.GetData()`.
    fn read(&mut self, v: &PyValue) {
        self.fleet_name = text_of(field(v, "fleetName"), NAME_MAX);
        self.description = text_of(field(v, "description"), DESC_MAX);
        self.invite_scope = int_of(field(v, "inviteScope")).unwrap_or(INVITE_CLOSED);
        self.activity_value = int_of(field(v, "activityValue"));
        self.use_advance_options = bool_of(field(v, "useAdvanceOptions"), false);
        self.new_player_friendly = bool_of(field(v, "newPlayerFriendly"), false);
        self.public_min_standing = real_of(field(v, "public_minStanding"));
        self.public_min_security = real_of(field(v, "public_minSecurity"));
        self.membergroups_min_standing = real_of(field(v, "membergroups_minStanding"));
        self.membergroups_min_security = real_of(field(v, "membergroups_minSecurity"));
        self.join_needs_approval = bool_of(field(v, "joinNeedsApproval"), false);
        self.hide_info = bool_of(field(v, "hideInfo"), false);
        self.update_on_boss_change = bool_of(field(v, "updateOnBossChange"), true);
        self.advert_join_limit = int_of(field(v, "advertJoinLimit"));
        self.read_options(v);
    }

    /// Whether `viewer` is shown this advert in the finder.
    pub fn visible_to(&self, reg: &Registry, viewer: i64) -> bool {
        if viewer == self.leader.char_id {
            return true;
        }
        let s = self.invite_scope;
        if s & (INVITE_PUBLIC | INVITE_PUBLIC_OPEN) != 0 {
            return true;
        }
        let Some(c) = reg.character(viewer) else {
            return false;
        };
        if s & INVITE_CORP != 0 && c.corporation_id == self.leader.corp_id {
            return true;
        }
        s & INVITE_ALLIANCE != 0
            && self.leader.alliance_id.is_some()
            && crate::alliance::of_corp(reg.deltas(), c.corporation_id) == self.leader.alliance_id
    }
}

// ---------------------------------------------------------------------------
// The world's side
// ---------------------------------------------------------------------------

fn raise(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

/// The fleet `ch` is the boss of, and its boss record.
fn boss_fleet(reg: &Registry, ch: i64) -> Option<i64> {
    let f = reg.fleets().fleet_of(ch)?;
    f.boss().filter(|b| b.char_id == ch).map(|_| f.fleet_id)
}

fn leader_of(reg: &Registry, ch: i64) -> Leader {
    let corp = reg.character(ch).map_or(0, |c| c.corporation_id);
    Leader {
        char_id: ch,
        corp_id: corp,
        alliance_id: crate::alliance::of_corp(reg.deltas(), corp),
        war_faction_id: crate::service::facwar_mgr::war_faction(reg, ch),
        security_status: crate::npc::bounty::security_status(reg, ch),
    }
}

/// Drop what belongs to a fleet that is gone.
fn sweep(reg: &mut Registry) {
    let gone: Vec<i64> = reg
        .ext::<Finder>()
        .adverts
        .keys()
        .chain(reg.ext::<Finder>().requests.keys())
        .copied()
        .filter(|id| reg.fleets().fleet(*id).is_none())
        .collect();
    if gone.is_empty() {
        return;
    }
    let f = reg.ext_mut::<Finder>();
    for id in gone {
        f.adverts.remove(&id);
        f.requests.remove(&id);
    }
}

/// `AddFleetFinderAdvert(adInfoData)`. The caller must be the boss of a
/// fleet; a fleet has one advert, replaced by a second add.
pub fn add(reg: &mut Registry, actor: Option<i64>, data: &PyValue) -> Result<Advert, ServiceError> {
    let Some(me) = actor else {
        return Err(raise("No character is selected."));
    };
    let Some(fleet_id) = boss_fleet(reg, me) else {
        return Err(raise("Only the boss of a fleet can list it."));
    };
    let mut advert = blank_advert(reg, me, fleet_id, INVITE_CLOSED);
    advert.read(data);
    reg.ext_mut::<Finder>()
        .adverts
        .insert(fleet_id, advert.clone());
    reg.fleets_mut().set_registered(fleet_id, true);
    Ok(advert)
}

/// A new advert for `fleet_id` led by `me`, with the given scope and the
/// defaults a creator who touched no option gets. Keeps the creation time of
/// an advert the fleet already has.
fn blank_advert(reg: &Registry, me: i64, fleet_id: i64, invite_scope: i64) -> Advert {
    let now = crate::pyutil::filetime_now();
    let members = reg
        .fleets()
        .fleet(fleet_id)
        .map_or(0, |f| f.members.len() as i64);
    // The session's system, not the roster's: a docked boss is not in the
    // roster, and an advert with system 0 makes the client's `GetFleetEntry`
    // raise `RecordNotFound` on the location lookup.
    let system = reg.session_system(me).unwrap_or(0);
    let created = reg
        .ext::<Finder>()
        .adverts
        .get(&fleet_id)
        .map_or(now, |a| a.date_created);
    Advert {
        fleet_id,
        leader: leader_of(reg, me),
        solar_system_id: system,
        num_members: members,
        advert_time: now,
        date_created: created,
        fleet_name: String::new(),
        description: String::new(),
        invite_scope,
        activity_value: None,
        use_advance_options: false,
        new_player_friendly: false,
        public_min_standing: None,
        public_min_security: None,
        public_allowed: BTreeSet::new(),
        public_disallowed: BTreeSet::new(),
        membergroups_min_standing: None,
        membergroups_min_security: None,
        membergroups_allowed: BTreeSet::new(),
        membergroups_disallowed: BTreeSet::new(),
        join_needs_approval: false,
        hide_info: false,
        update_on_boss_change: true,
        advert_join_limit: None,
    }
}

/// FC-0: make sure the fleet `fc` bosses has an advert that corp and alliance
/// mates may apply to without approval (so an `apply` is answered by an
/// invite from the boss at once). An existing advert is kept untouched.
/// Returns `(fleet id, created)`; `Err` when `fc` is in no fleet or is not its
/// boss.
pub fn ensure_open_advert(reg: &mut Registry, fc: i64) -> Result<(i64, bool), String> {
    let Some(fleet_id) = boss_fleet(reg, fc) else {
        return Err("Only the boss of a fleet can list it.".to_owned());
    };
    sweep(reg);
    if reg.ext::<Finder>().adverts.contains_key(&fleet_id) {
        return Ok((fleet_id, false));
    }
    let advert = blank_advert(reg, fc, fleet_id, INVITE_CORP | INVITE_ALLIANCE);
    reg.ext_mut::<Finder>().adverts.insert(fleet_id, advert);
    reg.fleets_mut().set_registered(fleet_id, true);
    Ok((fleet_id, true))
}

/// `RemoveFleetFinderAdvert()`: the boss's advert out of the list.
pub fn remove(reg: &mut Registry, actor: Option<i64>) -> Option<Advert> {
    let fleet_id = boss_fleet(reg, actor?)?;
    let gone = reg.ext_mut::<Finder>().adverts.remove(&fleet_id);
    reg.fleets_mut().set_registered(fleet_id, false);
    gone
}

/// `GetMyFleetFinderAdvert()`: the advert of the fleet `actor` is in
/// (any member reads it, `GetCurrentAdvertForMyFleet:2172`).
pub fn mine(reg: &Registry, actor: Option<i64>) -> Option<Advert> {
    let fleet_id = reg.fleets().fleet_of(actor?)?.fleet_id;
    reg.ext::<Finder>().adverts.get(&fleet_id).cloned()
}

/// `UpdateAdvertInfo(numMembers, allowedDiff)` and the two entity updates:
/// the boss's advert, with the member count, system, and entity sets the
/// call carries. `new_leader` makes the caller the advert's leader
/// (`UpdateFleetAdvertWithNewLeader`).
pub fn update(
    reg: &mut Registry,
    actor: Option<i64>,
    num_members: Option<i64>,
    options: Option<&PyValue>,
    new_leader: bool,
) -> Option<Advert> {
    let me = actor?;
    let fleet_id = boss_fleet(reg, me)?;
    let leader = new_leader.then(|| leader_of(reg, me));
    let system = reg.session_system(me);
    let members = reg.fleets().fleet(fleet_id).map(|f| f.members.len() as i64);
    let f = reg.ext_mut::<Finder>();
    let a = f.adverts.get_mut(&fleet_id)?;
    if let Some(l) = leader {
        a.leader = l;
    }
    a.num_members = num_members.or(members).unwrap_or(a.num_members);
    if let Some(s) = system {
        a.solar_system_id = s;
    }
    if let Some(o) = options {
        a.read_options(o);
    }
    Some(a.clone())
}

/// `GetAvailableFleetAds()`: the adverts `viewer` sees, in the list form.
pub fn available(reg: &mut Registry, viewer: Option<i64>) -> PyValue {
    sweep(reg);
    let viewer = viewer.unwrap_or(0);
    let mut rows: Vec<(i64, PyValue)> = reg
        .ext::<Finder>()
        .adverts
        .values()
        .filter(|a| a.visible_to(reg, viewer))
        .map(|a| (a.fleet_id, a.to_value(false)))
        .collect();
    rows.sort_by_key(|(id, _)| *id);
    PyValue::Dict(
        rows.into_iter()
            .map(|(id, v)| (v, PyValue::Int(id)))
            .collect(),
    )
}

/// `ApplyToJoinFleet(fleetID, autoAccept)` -> whether an application is now
/// waiting for the boss (truthy makes the client say so).
pub fn apply(reg: &mut Registry, actor: Option<i64>, fleet_id: i64) -> Result<bool, ServiceError> {
    sweep(reg);
    let Some(me) = actor else {
        return Err(raise("No character is selected."));
    };
    let Some(advert) = reg.ext::<Finder>().adverts.get(&fleet_id).cloned() else {
        return Err(raise("That fleet is no longer advertised."));
    };
    if !advert.visible_to(reg, me) {
        return Err(raise("That fleet is not open to you."));
    }
    if reg.fleets().fleet_of(me).is_some() {
        return Err(ServiceError::Raise(eve_proto::user_error(
            "FleetYouAreAlreadyInFleet",
            Vec::new(),
        )));
    }
    let boss = advert.leader.char_id;
    if advert.join_needs_approval {
        let corp = reg.character(me).map_or(0, |c| c.corporation_id);
        let info = JoinRequest {
            char_id: me,
            corp_id: corp,
        };
        reg.ext_mut::<Finder>()
            .requests
            .entry(fleet_id)
            .or_default()
            .insert(me, info.clone());
        reg.push(JoinRequestPush {
            to: boss,
            request: info,
        });
        return Ok(true);
    }
    match reg
        .fleets_mut()
        .invite(boss, fleet_id, me, None, None, None)
    {
        Ok((_, effects)) => {
            reg.fleet_apply(effects);
            Ok(false)
        }
        Err(e) => Err(ServiceError::Raise(e.to_user_error())),
    }
}

/// `GetJoinRequests()`: `{charID: info}` for the caller's fleet (the boss
/// reads it), without those who have joined a fleet meanwhile.
pub fn requests_value(reg: &mut Registry, actor: Option<i64>) -> PyValue {
    sweep(reg);
    let Some(fleet_id) = actor
        .and_then(|a| reg.fleets().fleet_of(a))
        .map(|f| f.fleet_id)
    else {
        return PyValue::Dict(Vec::new());
    };
    let waiting: Vec<JoinRequest> = reg
        .ext::<Finder>()
        .requests
        .get(&fleet_id)
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default();
    let (open, joined): (Vec<_>, Vec<_>) = waiting
        .into_iter()
        .partition(|r| reg.fleets().fleet_of(r.char_id).is_none());
    if !joined.is_empty()
        && let Some(m) = reg.ext_mut::<Finder>().requests.get_mut(&fleet_id)
    {
        for r in joined {
            m.remove(&r.char_id);
        }
    }
    PyValue::Dict(
        open.into_iter()
            .map(|r| (r.to_value(), PyValue::Int(r.char_id)))
            .collect(),
    )
}

/// `RejectJoinRequest(charID)`: the boss turns an applicant away; the
/// boss's window is refreshed (`OnJoinRequestUpdate`).
pub fn reject(reg: &mut Registry, actor: Option<i64>, char_id: i64) {
    let Some(me) = actor else { return };
    let Some(fleet_id) = boss_fleet(reg, me) else {
        return;
    };
    let had = reg
        .ext_mut::<Finder>()
        .requests
        .get_mut(&fleet_id)
        .and_then(|m| m.remove(&char_id))
        .is_some();
    if had {
        let value = requests_value(reg, Some(me));
        reg.push(JoinRequestUpdate { to: me, value });
    }
}

impl JoinRequest {
    /// What `OnFleetJoinRequest` and `GetJoinRequests` carry: a `KeyVal`
    /// with `charID` and `corpID` (`OnFleetJoinRequest:1534-1535`,
    /// `fleetJoinRequestWnd.LoadJoinRequests:43-56`) [V].
    pub fn to_value(&self) -> PyValue {
        keyval(vec![
            ("charID", PyValue::Int(self.char_id)),
            ("corpID", PyValue::Int(self.corp_id)),
        ])
    }
}

/// `OnFleetJoinRequest(info)` to the boss (`fleetSvc.py:1533`) [V].
#[derive(Debug, Clone, PartialEq)]
pub struct JoinRequestPush {
    pub to: i64,
    pub request: JoinRequest,
}

impl Push for JoinRequestPush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnFleetJoinRequest", vec![self.request.to_value()])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnJoinRequestUpdate(joinRequests)` to the boss (`fleetSvc.py:1543`) [V].
#[derive(Debug, Clone, PartialEq)]
pub struct JoinRequestUpdate {
    pub to: i64,
    pub value: PyValue,
}

impl Push for JoinRequestUpdate {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnJoinRequestUpdate", vec![self.value.clone()])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

// ---------------------------------------------------------------------------
// The watchlist
// ---------------------------------------------------------------------------

/// Who `ch` watches, in their fleet: members of the fleet they are in now.
pub fn watched(reg: &Registry, ch: i64) -> Vec<i64> {
    let Some(f) = reg.fleets().fleet_of(ch) else {
        return Vec::new();
    };
    reg.ext::<Finder>()
        .watch
        .get(&ch)
        .map(|s| {
            s.iter()
                .copied()
                .filter(|c| f.members.contains_key(c))
                .collect()
        })
        .unwrap_or_default()
}

/// `AddToWatchlist(newChars, current)`: those of `new` who are in the
/// caller's fleet, up to [`MAX_WATCHED`].
pub fn watch(reg: &mut Registry, ch: i64, new: &[i64]) {
    let Some(f) = reg.fleets().fleet_of(ch) else {
        return;
    };
    let members: BTreeSet<i64> = f.members.keys().copied().collect();
    let mut set: BTreeSet<i64> = watched(reg, ch).into_iter().collect();
    for c in new {
        if set.len() >= MAX_WATCHED {
            break;
        }
        if members.contains(c) && *c != ch {
            set.insert(*c);
        }
    }
    reg.ext_mut::<Finder>().watch.insert(ch, set);
}

/// `RemoveFromWatchlist(charID, current)`.
pub fn unwatch(reg: &mut Registry, ch: i64, gone: i64) {
    if let Some(s) = reg.ext_mut::<Finder>().watch.get_mut(&ch) {
        s.remove(&gone);
        if s.is_empty() {
            reg.ext_mut::<Finder>().watch.remove(&ch);
        }
    }
}

/// `ForceLeaveFleet()`: the caller leaves whatever fleet the server has
/// them in. `false` when it has none.
pub fn force_leave(reg: &mut Registry, ch: i64) -> bool {
    let Some(fleet_id) = reg.fleets().resolve_for(ch) else {
        return false;
    };
    match reg.fleets_mut().leave(ch, fleet_id) {
        Ok(effects) => {
            reg.fleet_apply(effects);
            reg.ext_mut::<Finder>().watch.remove(&ch);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};
    use crate::service::Request;

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;
    const CAROL: i64 = 90_000_003;

    fn world() -> Registry {
        let pilot = |id: i64, name: &str| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: id + 10_000_000,
            ..Character::default()
        };
        Registry::new(SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice"),
                pilot(BOB, "Bob"),
                pilot(CAROL, "Carol"),
            ],
            ..SandboxConfig::default()
        })
    }

    fn call(
        reg: &mut Registry,
        who: i64,
        service: &str,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        let bind = PyValue::None;
        reg.dispatch(&Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: Some(&bind),
            actor: Some(who),
        })
    }

    /// Alice creates a fleet, as `fleetSvc.CreateFleet` does.
    fn alices_fleet(reg: &mut Registry) -> i64 {
        call(reg, ALICE, "fleetObjectHandler", "CreateFleet", &[]).expect("create");
        call(
            reg,
            ALICE,
            "fleetObjectHandler",
            "Init",
            &[PyValue::Int(670)],
        )
        .expect("init");
        reg.fleets().fleet_of(ALICE).expect("her fleet").fleet_id
    }

    fn join(reg: &mut Registry, fleet: i64, who: i64) {
        reg.fleets_mut()
            .invite(ALICE, fleet, who, None, None, None)
            .expect("invited");
        reg.fleets_mut()
            .accept_invite(who, fleet, None, None, None)
            .expect("seated");
    }

    /// The `adInfoObject.GetData()` a creator sends: text, scope and the
    /// advanced options, the four sets as Python sets.
    fn ad(scope: i64, approval: bool) -> PyValue {
        pydict(vec![
            ("fleetName", PyValue::Unicode("Rats and Friends".to_owned())),
            (
                "description",
                PyValue::Unicode("Bring a frigate".to_owned()),
            ),
            ("inviteScope", PyValue::Int(scope)),
            ("activityValue", PyValue::Int(3)),
            ("useAdvanceOptions", PyValue::Bool(true)),
            ("newPlayerFriendly", PyValue::Bool(true)),
            ("public_minStanding", PyValue::None),
            ("public_minSecurity", PyValue::Real(-5.0)),
            (
                "public_allowedEntities",
                py_set(&BTreeSet::from([BOB, 1_000_501])),
            ),
            ("public_disallowedEntities", py_set(&BTreeSet::new())),
            ("membergroups_minStanding", PyValue::None),
            ("membergroups_minSecurity", PyValue::None),
            ("membergroups_allowedEntities", py_set(&BTreeSet::new())),
            ("membergroups_disallowedEntities", py_set(&BTreeSet::new())),
            ("joinNeedsApproval", PyValue::Bool(approval)),
            ("hideInfo", PyValue::Bool(false)),
            ("updateOnBossChange", PyValue::Bool(true)),
            ("advertJoinLimit", PyValue::None),
        ])
    }

    fn get<'a>(v: &'a PyValue, key: &str) -> &'a PyValue {
        field(v, key).unwrap_or_else(|| panic!("no {key} in {v:?}"))
    }

    /// The boss lists the fleet: the answer is the advert (24 keys, the
    /// entity sets as sets), the fleet is marked registered, the boss's
    /// own read gives it back; a stranger cannot list it.
    #[test]
    fn the_boss_lists_the_fleet() {
        let mut reg = world();
        let fleet = alices_fleet(&mut reg);
        let e = call(
            &mut reg,
            BOB,
            "fleetProxy",
            "AddFleetFinderAdvert",
            &[ad(INVITE_PUBLIC, false)],
        );
        assert!(e.is_err(), "Bob is in no fleet");
        let got = call(
            &mut reg,
            ALICE,
            "fleetProxy",
            "AddFleetFinderAdvert",
            &[ad(INVITE_PUBLIC, false)],
        )
        .expect("listed");
        let PyValue::Dict(keys) = &got else {
            panic!("a dict")
        };
        assert_eq!(keys.len(), 24, "the 24 keys of the advert");
        assert_eq!(get(&got, "fleetID"), &PyValue::Int(fleet));
        assert_eq!(
            get(&got, "fleetName"),
            &PyValue::Unicode("Rats and Friends".to_owned())
        );
        assert_eq!(get(&got, "inviteScope"), &PyValue::Int(INVITE_PUBLIC));
        assert_eq!(get(&got, "numMembers"), &PyValue::Int(1));
        assert!(
            matches!(
                get(&got, "public_allowedEntities"),
                PyValue::ObjectEx { .. }
            ),
            "a set"
        );
        let leader = get(&got, "leader");
        let PyValue::Instance { state, .. } = leader else {
            panic!("a KeyVal")
        };
        assert_eq!(
            crate::pyutil::dict_get(state, b"charID"),
            Some(&PyValue::Int(ALICE))
        );
        assert!(reg.fleets().fleet(fleet).unwrap().options.is_registered);
        let mine =
            call(&mut reg, ALICE, "fleetProxy", "GetMyFleetFinderAdvert", &[]).expect("mine");
        assert_eq!(get(&mine, "fleetID"), &PyValue::Int(fleet));
        // A member who is not the boss reads it too; a pilot outside, not.
        join(&mut reg, fleet, BOB);
        let bobs =
            call(&mut reg, BOB, "fleetProxy", "GetMyFleetFinderAdvert", &[]).expect("his fleet's");
        assert_eq!(get(&bobs, "fleetID"), &PyValue::Int(fleet));
        assert_eq!(
            call(&mut reg, CAROL, "fleetProxy", "GetMyFleetFinderAdvert", &[]).expect("none"),
            PyValue::None
        );
        // Removed: the answer is the advert, then nothing.
        let removed = call(
            &mut reg,
            ALICE,
            "fleetProxy",
            "RemoveFleetFinderAdvert",
            &[],
        )
        .expect("removed");
        assert_eq!(get(&removed, "fleetID"), &PyValue::Int(fleet));
        assert!(!reg.fleets().fleet(fleet).unwrap().options.is_registered);
        assert_eq!(
            call(&mut reg, ALICE, "fleetProxy", "GetMyFleetFinderAdvert", &[]).expect("none"),
            PyValue::None
        );
    }

    /// A boss who is docked is not in the roster, and the advert still
    /// carries the system their station is in -- 0 made the client's
    /// `GetFleetEntry` raise `RecordNotFound` on the location lookup.
    #[test]
    fn a_docked_boss_advert_carries_the_station_system() {
        let mut reg = world();
        let fleet = alices_fleet(&mut reg);
        reg.character_mut(ALICE).expect("alice").solar_system_id = Some(30_000_777);
        assert!(reg.roster().get(ALICE).is_none(), "docked: not in space");
        call(
            &mut reg,
            ALICE,
            "fleetProxy",
            "AddFleetFinderAdvert",
            &[ad(INVITE_CLOSED, false)],
        )
        .expect("listed");
        assert_eq!(
            reg.ext::<Finder>().adverts[&fleet].solar_system_id,
            30_000_777
        );
    }

    /// The list: a closed advert is the boss's alone, a public one is
    /// everybody's and carries `None` for the entity sets; a fleet that is
    /// gone takes its advert with it.
    #[test]
    fn the_list_shows_what_the_scope_allows() {
        let mut reg = world();
        let fleet = alices_fleet(&mut reg);
        call(
            &mut reg,
            ALICE,
            "fleetProxy",
            "AddFleetFinderAdvert",
            &[ad(INVITE_CLOSED, false)],
        )
        .expect("listed closed");
        let list = |reg: &mut Registry, who: i64| {
            let PyValue::Dict(d) =
                call(reg, who, "fleetProxy", "GetAvailableFleetAds", &[]).expect("answered")
            else {
                panic!("a dict")
            };
            d
        };
        assert_eq!(list(&mut reg, ALICE).len(), 1, "the boss sees their own");
        assert!(list(&mut reg, BOB).is_empty());
        call(
            &mut reg,
            ALICE,
            "fleetProxy",
            "AddFleetFinderAdvert",
            &[ad(INVITE_PUBLIC, false)],
        )
        .expect("relisted public");
        let seen = list(&mut reg, BOB);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].1, PyValue::Int(fleet), "keyed by the fleet id");
        assert_eq!(get(&seen[0].0, "public_allowedEntities"), &PyValue::None);
        assert_eq!(
            get(&seen[0].0, "fleetName"),
            &PyValue::Unicode("Rats and Friends".to_owned())
        );
        let effects = reg.fleets_mut().disband(ALICE, fleet).expect("disbanded");
        reg.fleet_apply(effects);
        assert!(list(&mut reg, BOB).is_empty(), "no fleet, no advert");
    }

    /// Applying to an open advert invites the applicant at once and answers
    /// falsy (the client's `expectingInvite` accepts); an approval advert
    /// makes a request: truthy, the boss told, `GetJoinRequests` lists it,
    /// and `RejectJoinRequest` removes it and tells the boss's window.
    #[test]
    fn applying_invites_or_asks_the_boss() {
        let mut reg = world();
        let fleet = alices_fleet(&mut reg);
        call(
            &mut reg,
            ALICE,
            "fleetProxy",
            "AddFleetFinderAdvert",
            &[ad(INVITE_PUBLIC, false)],
        )
        .expect("open advert");
        let _ = reg.take_pending();
        let apply = |reg: &mut Registry, who: i64| {
            call(
                reg,
                who,
                "fleetProxy",
                "ApplyToJoinFleet",
                &[PyValue::Int(fleet), PyValue::Bool(true)],
            )
        };
        assert_eq!(apply(&mut reg, BOB).expect("applied"), PyValue::Bool(false));
        assert!(reg.fleets().invite_of(BOB).is_some(), "invited");

        // Carol, to an advert that wants approval.
        call(
            &mut reg,
            ALICE,
            "fleetProxy",
            "AddFleetFinderAdvert",
            &[ad(INVITE_PUBLIC, true)],
        )
        .expect("approval advert");
        let _ = reg.take_pending();
        assert_eq!(
            apply(&mut reg, CAROL).expect("applied"),
            PyValue::Bool(true)
        );
        let pushed: Vec<(&'static str, Option<Vec<i64>>)> = reg
            .take_pending()
            .iter()
            .map(|(_, p)| (p.to_push().0, p.audience()))
            .collect();
        assert!(
            pushed.contains(&("OnFleetJoinRequest", Some(vec![ALICE]))),
            "{pushed:?}"
        );
        let PyValue::Dict(reqs) = call(
            &mut reg,
            ALICE,
            "fleetObjectHandler",
            "GetJoinRequests",
            &[],
        )
        .expect("requests") else {
            panic!("a dict")
        };
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].1, PyValue::Int(CAROL));
        assert_eq!(get(&reqs[0].0, "charID"), &PyValue::Int(CAROL));
        call(
            &mut reg,
            ALICE,
            "fleetObjectHandler",
            "RejectJoinRequest",
            &[PyValue::Int(CAROL)],
        )
        .expect("rejected");
        let pushed: Vec<&'static str> = reg
            .take_pending()
            .iter()
            .map(|(_, p)| p.to_push().0)
            .collect();
        assert!(pushed.contains(&"OnJoinRequestUpdate"), "{pushed:?}");
        let PyValue::Dict(reqs) = call(
            &mut reg,
            ALICE,
            "fleetObjectHandler",
            "GetJoinRequests",
            &[],
        )
        .expect("requests") else {
            panic!("a dict")
        };
        assert!(reqs.is_empty());
        // Someone already in a fleet cannot apply to another.
        let e = apply(&mut reg, ALICE);
        assert!(e.is_err());
    }

    /// The boss updates the advert: members and entity sets, and the
    /// leader on a change of boss.
    #[test]
    fn the_advert_follows_the_fleet() {
        let mut reg = world();
        let fleet = alices_fleet(&mut reg);
        call(
            &mut reg,
            ALICE,
            "fleetProxy",
            "AddFleetFinderAdvert",
            &[ad(INVITE_PUBLIC, false)],
        )
        .expect("listed");
        join(&mut reg, fleet, BOB);
        let diff = crate::pyutil::keyval(vec![]);
        let got = call(
            &mut reg,
            ALICE,
            "fleetProxy",
            "UpdateAdvertInfo",
            &[PyValue::Int(2), diff],
        )
        .expect("updated");
        assert_eq!(get(&got, "numMembers"), &PyValue::Int(2));
        let info = pydict(vec![(
            "public_allowedEntities",
            py_set(&BTreeSet::from([CAROL])),
        )]);
        let got = call(
            &mut reg,
            ALICE,
            "fleetProxy",
            "UpdateAdvertAllowedEntities",
            &[info],
        )
        .expect("updated");
        let PyValue::ObjectEx { header, .. } = get(&got, "public_allowedEntities") else {
            panic!("a set")
        };
        assert!(format!("{header:?}").contains(&CAROL.to_string()));
        // The fleet changes hands.
        let (_, effects) = reg
            .fleets_mut()
            .make_leader(ALICE, fleet, BOB)
            .expect("Bob leads");
        reg.fleet_apply(effects);
        let got = call(
            &mut reg,
            BOB,
            "fleetProxy",
            "UpdateFleetAdvertWithNewLeader",
            &[pydict(vec![])],
        )
        .expect("updated");
        let PyValue::Instance { state, .. } = get(&got, "leader") else {
            panic!("a KeyVal")
        };
        assert_eq!(
            crate::pyutil::dict_get(state, b"charID"),
            Some(&PyValue::Int(BOB))
        );
        // Alice is no boss now: her update finds no advert of hers.
        let got = call(
            &mut reg,
            ALICE,
            "fleetProxy",
            "UpdateAdvertInfo",
            &[PyValue::Int(2), PyValue::None],
        )
        .expect("answered");
        assert_eq!(got, PyValue::None);
    }

    /// The watchlist keeps members of the caller's fleet only, and forgets
    /// them when they leave; `ForceLeaveFleet` takes the caller out.
    #[test]
    fn the_watchlist_and_force_leave() {
        let mut reg = world();
        let fleet = alices_fleet(&mut reg);
        join(&mut reg, fleet, BOB);
        let list = |ids: &[i64]| PyValue::List(ids.iter().map(|i| PyValue::Int(*i)).collect());
        call(
            &mut reg,
            ALICE,
            "fleetMgr",
            "AddToWatchlist",
            &[list(&[BOB, CAROL]), list(&[])],
        )
        .expect("added");
        assert_eq!(watched(&reg, ALICE), vec![BOB], "Carol is in no fleet");
        call(
            &mut reg,
            ALICE,
            "fleetMgr",
            "RemoveFromWatchlist",
            &[PyValue::Int(BOB), list(&[])],
        )
        .expect("removed");
        assert!(watched(&reg, ALICE).is_empty());
        call(
            &mut reg,
            ALICE,
            "fleetMgr",
            "AddToWatchlist",
            &[list(&[BOB]), list(&[])],
        )
        .expect("added");
        // Bob leaves on his own: the row is no longer shown.
        call(&mut reg, BOB, "fleetMgr", "ForceLeaveFleet", &[]).expect("left");
        assert!(reg.fleets().fleet_of(BOB).is_none());
        assert!(watched(&reg, ALICE).is_empty());
        // A pilot in no fleet: a no-op, not a raise.
        assert_eq!(
            call(&mut reg, CAROL, "fleetMgr", "ForceLeaveFleet", &[]).expect("answered"),
            PyValue::None
        );
    }

    // ------------------------------------------------------------- FC-0

    /// `ensure_open_advert`: created once, kept after, refused for a member
    /// who is not the boss, and a corp-mate's `apply` is answered by an
    /// invite from the boss with no approval step.
    #[test]
    fn ensure_open_advert_creates_keeps_and_refuses() {
        let mut reg = world();
        let fleet = alices_fleet(&mut reg);
        join(&mut reg, fleet, BOB);
        assert!(
            ensure_open_advert(&mut reg, BOB).is_err(),
            "a member who is not the boss"
        );
        assert!(
            ensure_open_advert(&mut reg, CAROL).is_err(),
            "a pilot in no fleet"
        );
        assert_eq!(ensure_open_advert(&mut reg, ALICE), Ok((fleet, true)));
        let made = mine(&reg, Some(ALICE)).expect("advert");
        assert_eq!(made.invite_scope, INVITE_CORP | INVITE_ALLIANCE);
        assert!(!made.join_needs_approval);
        assert_eq!(made.leader.char_id, ALICE);
        assert!(
            reg.fleets()
                .fleet(fleet)
                .expect("fleet")
                .options
                .is_registered
        );
        assert_eq!(ensure_open_advert(&mut reg, ALICE), Ok((fleet, false)));
        assert_eq!(mine(&reg, Some(ALICE)), Some(made), "kept untouched");

        // A boss-made closed advert is kept too, not widened.
        call(
            &mut reg,
            ALICE,
            "fleetProxy",
            "AddFleetFinderAdvert",
            &[ad(INVITE_CLOSED, true)],
        )
        .expect("closed advert");
        assert_eq!(ensure_open_advert(&mut reg, ALICE), Ok((fleet, false)));
        assert_eq!(
            mine(&reg, Some(ALICE)).expect("advert").invite_scope,
            INVITE_CLOSED
        );
    }

    #[test]
    fn a_corp_mate_applies_to_the_ensured_advert_and_is_invited() {
        let mut reg = world();
        let fleet = alices_fleet(&mut reg);
        ensure_open_advert(&mut reg, ALICE).expect("advert");
        let _ = reg.take_pending();
        let answer = call(
            &mut reg,
            CAROL,
            "fleetProxy",
            "ApplyToJoinFleet",
            &[PyValue::Int(fleet), PyValue::Bool(true)],
        )
        .expect("applied");
        assert_eq!(answer, PyValue::Bool(false), "no approval wanted");
        assert!(reg.fleets().invite_of(CAROL).is_some(), "invited at once");
    }
}
