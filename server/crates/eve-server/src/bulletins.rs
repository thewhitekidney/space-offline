//! **Corporation and alliance bulletins** (SMALL-SOCIAL; lane note
//! `docs/lanes/small-social.md`): the notice board on the corporation Home
//! tab and the alliance Home tab. One store for both, with ids unique across
//! corporations and alliances (the client finds a bulletin by id in the
//! corporation's list and then in the alliance's, `GetBulletin:886-905`).
//!
//! The client (`base_corporation.py`, `all_cso.py`, `corp_ui_bulletins.py`)
//! [V]:
//!
//! - `corpRegistry.GetBulletins()` and the alliance moniker's
//!   `GetBulletins()` (`GetCorpBulletins:862`, `GetAllianceBulletins:221`) ->
//!   an iterable of rows; each is wrapped in `utillib.KeyVal(row)` (`:854`,
//!   which takes a `KeyVal`, a dict or a `DBRow`) and read for `bulletinID`,
//!   `ownerID`, `editCharacterID`, `editDateTime`, `title`, `body` and
//!   `sortOrder` (`:857`, `:874-882`, `:914`, `corp_ui_bulletins:91`). Sorted
//!   client-side by `(sortOrder, -editDateTime)`. The owner id tells the
//!   client whether a bulletin is the alliance's (`ownerID == allianceid`).
//! - `AddBulletin(title, body)` posts (`:936-938`); an edit is the same call
//!   with `bulletinID=` and `editDateTime=` as **keywords** (`:944-946`); an
//!   edit catches a `UserError` from it (`:947`) and shows it. The answer is
//!   dropped.
//! - `DeleteBulletin(bulletinID)` (`:926-928`), answer dropped.
//! - `UpdateBulletinOrder(ids)` (`:953`, `all_cso:228`): a list of
//!   bulletin ids in display order.
//! - The client allows the edit, delete and add links only to a pilot with
//!   `corpRoleChatManager` (`:878`, `:911`, `:920`), and stops at
//!   `MAX_NUM_BULLETINS = 10` per list (`:39`, `:932`) [V].
//!
//! The server side is ours. EvEJS (`corpRegistryRuntime.js` bulletins,
//! `allianceRegistryRuntime.js:927-1028`) keeps rows of those columns, appends
//! with `sortOrder` = the current count, and orders from the id list; it has
//! no edit-conflict check, which we add: the localisation holds
//! `CrpBulletinShotgun` ("someone has edited this bulletin from underneath
//! you") and the client passes the `editDateTime` it read, so a mismatch
//! raises that. The ids are ours.
//!
//! **[A]** who may post: a member of the corporation with the director role,
//! the CEO, or `corpRoleChatManager` (`2^55`, `appConst.py:363`); for an
//! alliance, the same in its **executor** corporation. The client checks only
//! the chat-manager role; the director and CEO are added because a director
//! of a founded corporation holds every role on Tranquility. Falsified by a
//! retail capture of a non-executor chat manager posting to an alliance.
//! **[A]** the server cap is the client's 10 per owner, refused with a
//! `CustomNotify` (the client's own cap is a dialog, not a `UserError`).
//!
//! Stored as delta kind [`KIND`]: `id -> owner create_char create_dt
//! edit_char edit_dt sort_order title body`, and `next -> n`.

use crate::pyutil::keyval;
use crate::service::{Registry, Request, ServiceError};
use eve_marshal::PyValue;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "bulletin";

/// `MAX_NUM_BULLETINS` in `base_corporation.py:39` [V].
pub const MAX_PER_OWNER: usize = 10;
/// **[A]** tuning: the edit form's fields are not capped in the client we
/// read; these bound what a bulletin can make the world file hold.
pub const MAX_TITLE_CHARS: usize = 100;
pub const MAX_BODY_CHARS: usize = 8_000;
/// `appConst.corpRoleChatManager` [V].
pub const ROLE_CHAT_MANAGER: i64 = 36_028_797_018_963_968;

/// Whose board a call is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Corp,
    Alliance,
}

/// One bulletin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bulletin {
    pub id: i64,
    pub owner: i64,
    pub create_char: i64,
    pub create_dt: i64,
    pub edit_char: i64,
    pub edit_dt: i64,
    pub sort_order: i64,
    pub title: String,
    pub body: String,
}

/// Why a write was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The actor may not post on this board.
    Denied,
    /// No such bulletin on this board.
    NotFound,
    /// The board is full.
    TooMany,
    /// The edit was made against a copy that is no longer current.
    Shotgun,
    /// Empty title or body.
    Empty,
}

fn id_key(id: i64) -> Vec<String> {
    vec![id.to_string()]
}

fn next_key() -> Vec<String> {
    vec!["next".to_owned()]
}

fn parse(id: i64, v: &[String]) -> Option<Bulletin> {
    let n = |i: usize| v.get(i).and_then(|s| s.parse::<i64>().ok());
    Some(Bulletin {
        id,
        owner: n(0)?,
        create_char: n(1)?,
        create_dt: n(2)?,
        edit_char: n(3)?,
        edit_dt: n(4)?,
        sort_order: n(5)?,
        title: v.get(6).cloned().unwrap_or_default(),
        body: v.get(7).cloned().unwrap_or_default(),
    })
}

fn row(b: &Bulletin) -> Vec<String> {
    vec![
        b.owner.to_string(),
        b.create_char.to_string(),
        b.create_dt.to_string(),
        b.edit_char.to_string(),
        b.edit_dt.to_string(),
        b.sort_order.to_string(),
        b.title.clone(),
        b.body.clone(),
    ]
}

/// The bulletins of `owner` (a corporation or alliance id), in the order the
/// client shows them: `(sortOrder, -editDateTime)`.
pub fn list(reg: &Registry, owner: i64) -> Vec<Bulletin> {
    let mut out: Vec<Bulletin> = reg
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| match k.as_slice() {
            [id] => parse(id.parse().ok()?, v),
            _ => None,
        })
        .filter(|b| b.owner == owner)
        .collect();
    out.sort_by_key(|b| (b.sort_order, -b.edit_dt, b.id));
    out
}

/// One bulletin by id, whoever owns it.
pub fn get(reg: &Registry, id: i64) -> Option<Bulletin> {
    parse(id, reg.deltas().of(KIND).get(&id_key(id))?)
}

fn next_id(reg: &mut Registry) -> i64 {
    let n = reg
        .deltas()
        .of(KIND)
        .get(&next_key())
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(1);
    reg.deltas_mut()
        .put(KIND, next_key(), vec![(n + 1).to_string()]);
    n
}

/// Whether `ch` holds the right to post: director, CEO or chat manager of
/// their corporation.
fn posts(reg: &Registry, ch: i64) -> Option<i64> {
    let m = reg.corporations().membership(ch)?;
    let ceo = reg
        .corporations()
        .get(m.corporation_id)
        .is_some_and(|c| c.ceo_id == ch);
    let roles = m.roles | m.roles_at_other;
    (ceo || roles & crate::service::corp::CORP_ROLE_DIRECTOR != 0 || roles & ROLE_CHAT_MANAGER != 0)
        .then_some(m.corporation_id)
}

/// The board `ch` reads for `scope`: their corporation's id, or their
/// alliance's.
pub fn board_of(reg: &Registry, ch: i64, scope: Scope) -> Option<i64> {
    match scope {
        Scope::Corp => reg.corporations().membership(ch).map(|m| m.corporation_id),
        Scope::Alliance => crate::alliance::of_character(reg, ch),
    }
}

/// Whether `ch` may post, edit, delete and order on `owner`'s board.
pub fn may_post(reg: &Registry, ch: i64, scope: Scope, owner: i64) -> bool {
    let Some(corp) = posts(reg, ch) else {
        return false;
    };
    match scope {
        Scope::Corp => corp == owner,
        Scope::Alliance => {
            let st = crate::alliance::Alliances::load(reg.deltas());
            board_of(reg, ch, scope) == Some(owner)
                && st.get(owner).and_then(|a| a.executor_corp_id) == Some(corp)
        }
    }
}

fn clean(title: &str, body: &str) -> Result<(String, String), Refusal> {
    let (title, body) = (title.trim(), body.trim());
    if title.is_empty() || body.is_empty() {
        return Err(Refusal::Empty);
    }
    Ok((
        title.chars().take(MAX_TITLE_CHARS).collect(),
        body.chars().take(MAX_BODY_CHARS).collect(),
    ))
}

/// Post a bulletin; its id.
pub fn add(
    reg: &mut Registry,
    ch: i64,
    scope: Scope,
    owner: i64,
    title: &str,
    body: &str,
) -> Result<i64, Refusal> {
    if !may_post(reg, ch, scope, owner) {
        return Err(Refusal::Denied);
    }
    let (title, body) = clean(title, body)?;
    let have = list(reg, owner);
    if have.len() >= MAX_PER_OWNER {
        return Err(Refusal::TooMany);
    }
    let now = crate::pyutil::filetime_now();
    let id = next_id(reg);
    let b = Bulletin {
        id,
        owner,
        create_char: ch,
        create_dt: now,
        edit_char: ch,
        edit_dt: now,
        sort_order: have.iter().map(|b| b.sort_order + 1).max().unwrap_or(0),
        title,
        body,
    };
    reg.deltas_mut().put(KIND, id_key(id), row(&b));
    Ok(id)
}

/// Edit a bulletin. `seen` is the `editDateTime` the editor read: a
/// different stored one means someone edited meanwhile.
pub fn edit(
    reg: &mut Registry,
    ch: i64,
    scope: Scope,
    owner: i64,
    id: i64,
    (title, body): (&str, &str),
    seen: Option<i64>,
) -> Result<(), Refusal> {
    if !may_post(reg, ch, scope, owner) {
        return Err(Refusal::Denied);
    }
    let mut b = get(reg, id)
        .filter(|b| b.owner == owner)
        .ok_or(Refusal::NotFound)?;
    if seen.is_some_and(|s| s != b.edit_dt) {
        return Err(Refusal::Shotgun);
    }
    (b.title, b.body) = clean(title, body)?;
    b.edit_char = ch;
    // Strictly later than the copy the next editor may hold.
    b.edit_dt = crate::pyutil::filetime_now().max(b.edit_dt + 1);
    reg.deltas_mut().put(KIND, id_key(id), row(&b));
    Ok(())
}

/// Delete a bulletin.
pub fn delete(
    reg: &mut Registry,
    ch: i64,
    scope: Scope,
    owner: i64,
    id: i64,
) -> Result<(), Refusal> {
    if !may_post(reg, ch, scope, owner) {
        return Err(Refusal::Denied);
    }
    get(reg, id)
        .filter(|b| b.owner == owner)
        .ok_or(Refusal::NotFound)?;
    reg.deltas_mut().del(KIND, &id_key(id));
    Ok(())
}

/// `UpdateBulletinOrder(ids)`: position in the list becomes the sort order;
/// ids that are not on this board are ignored, the others keep their place
/// after the listed ones.
pub fn order(
    reg: &mut Registry,
    ch: i64,
    scope: Scope,
    owner: i64,
    ids: &[i64],
) -> Result<(), Refusal> {
    if !may_post(reg, ch, scope, owner) {
        return Err(Refusal::Denied);
    }
    let mut rest = list(reg, owner);
    let mut sorted = Vec::new();
    for id in ids {
        if let Some(p) = rest.iter().position(|b| b.id == *id) {
            sorted.push(rest.remove(p));
        }
    }
    sorted.extend(rest);
    for (i, mut b) in sorted.into_iter().enumerate() {
        b.sort_order = i as i64;
        reg.deltas_mut().put(KIND, id_key(b.id), row(&b));
    }
    Ok(())
}

fn bulletin_value(b: &Bulletin) -> PyValue {
    keyval(vec![
        ("bulletinID", PyValue::Int(b.id)),
        ("ownerID", PyValue::Int(b.owner)),
        ("createCharacterID", PyValue::Int(b.create_char)),
        ("createDateTime", PyValue::Int(b.create_dt)),
        ("editCharacterID", PyValue::Int(b.edit_char)),
        ("editDateTime", PyValue::Int(b.edit_dt)),
        ("title", PyValue::Unicode(b.title.clone())),
        ("body", PyValue::Unicode(b.body.clone())),
        ("sortOrder", PyValue::Int(b.sort_order)),
    ])
}

/// `GetBulletins()`: the board's rows, for a reader on it; nothing for
/// anyone else.
pub fn list_value(reg: &Registry, ch: Option<i64>, scope: Scope, owner: Option<i64>) -> PyValue {
    let rows = match (ch, owner) {
        (Some(ch), Some(owner)) if board_of(reg, ch, scope) == Some(owner) => {
            list(reg, owner).iter().map(bulletin_value).collect()
        }
        _ => Vec::new(),
    };
    PyValue::List(rows)
}

fn user_error(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
}

fn refusal(r: Refusal) -> ServiceError {
    match r {
        Refusal::Denied => user_error(
            "CrpAccessDenied",
            vec![(
                "reason",
                PyValue::Unicode("Only a director or a communications officer can do that.".into()),
            )],
        ),
        Refusal::NotFound => user_error("CorpBulletinNotFound", vec![]),
        Refusal::Shotgun => user_error("CrpBulletinShotgun", vec![]),
        Refusal::Empty => user_error("CorpBulletinMustFillIn", vec![]),
        Refusal::TooMany => user_error(
            "CustomNotify",
            vec![(
                "notify",
                crate::pyutil::bstr("You have reached the limit of open bulletins."),
            )],
        ),
    }
}

fn arg<'a>(req: &'a Request<'_>, i: usize, name: &str) -> Option<&'a PyValue> {
    req.args
        .get(i)
        .or_else(|| crate::pyutil::pairs_get(req.kwargs, name.as_bytes()))
        .filter(|v| !matches!(v, PyValue::None))
}

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
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

fn bad_args(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

/// The wire arms: `None` for a method that is not a bulletin call, so the
/// caller's own `no_method` stays the answer. `owner` is the board the call
/// is about (the actor's corporation, or the bound alliance).
pub fn call(
    reg: &mut Registry,
    req: &Request<'_>,
    scope: Scope,
    owner: Option<i64>,
) -> Option<Result<PyValue, ServiceError>> {
    let r = match req.method {
        "GetBulletins" => Ok(list_value(reg, req.actor, scope, owner)),
        "AddBulletin" | "UpdateBulletinOrder" | "DeleteBulletin" => write(reg, req, scope, owner),
        _ => return None,
    };
    Some(r)
}

fn write(
    reg: &mut Registry,
    req: &Request<'_>,
    scope: Scope,
    owner: Option<i64>,
) -> Result<PyValue, ServiceError> {
    let (Some(ch), Some(owner)) = (req.actor, owner) else {
        return Err(refusal(Refusal::Denied));
    };
    match req.method {
        // `AddBulletin(title, body[, bulletinID=, editDateTime=])`.
        "AddBulletin" => {
            let (Some(title), Some(body)) = (text(arg(req, 0, "title")), text(arg(req, 1, "body")))
            else {
                return Err(bad_args(req, "(title, body[, bulletinID, editDateTime])"));
            };
            match int(arg(req, 2, "bulletinID")) {
                Some(id) => edit(
                    reg,
                    ch,
                    scope,
                    owner,
                    id,
                    (&title, &body),
                    int(arg(req, 3, "editDateTime")),
                ),
                None => add(reg, ch, scope, owner, &title, &body).map(|_| ()),
            }
        }
        "DeleteBulletin" => match int(arg(req, 0, "bulletinID")) {
            Some(id) => delete(reg, ch, scope, owner, id),
            None => return Err(bad_args(req, "(bulletinID,)")),
        },
        _ => {
            let ids: Vec<i64> = match req.args.first() {
                Some(PyValue::List(l) | PyValue::Tuple(l)) => {
                    l.iter().filter_map(|v| int(Some(v))).collect()
                }
                _ => return Err(bad_args(req, "(bulletinIDs,)")),
            };
            order(reg, ch, scope, owner, &ids)
        }
    }
    .map(|()| PyValue::None)
    .map_err(refusal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};
    use crate::service::Request;

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;
    const CAROL: i64 = 90_000_003;

    fn call_as(
        reg: &mut Registry,
        who: i64,
        method: &str,
        args: &[PyValue],
        kwargs: &[(PyValue, PyValue)],
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service: "corpRegistry",
            method,
            args,
            kwargs,
            bind_params: None,
            actor: Some(who),
        })
    }

    /// Alice founds a corporation (CEO), Bob joins it with no roles, Carol
    /// founds her own.
    fn world() -> (Registry, i64, i64) {
        let pilot = |id: i64, name: &str| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: id + 1,
            balance: 5_000_000.0,
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice"),
                pilot(BOB, "Bob"),
                pilot(CAROL, "Carol"),
            ],
            ..SandboxConfig::default()
        });
        let u = |s: &str| PyValue::Unicode(s.to_owned());
        let found = |reg: &mut Registry, who: i64, name: &str, ticker: &str| {
            let args = vec![
                u(name),
                u(ticker),
                u("We mine."),
                u("http://"),
                PyValue::Real(0.0),
                PyValue::Int(419),
                PyValue::None,
                PyValue::None,
                PyValue::None,
                PyValue::None,
                PyValue::None,
                PyValue::None,
                PyValue::Bool(true),
                PyValue::Bool(false),
                PyValue::Real(0.0),
            ];
            call_as(reg, who, "AddCorporation", &args, &[]).expect("founded");
            reg.character(who).expect("a pilot").corporation_id
        };
        let mine = found(&mut reg, ALICE, "Board Mining", "BRDM");
        let theirs = found(&mut reg, CAROL, "Board Other", "BRDO");
        reg.corporations_mut().enrol_for_test(BOB, mine, 0);
        (reg, mine, theirs)
    }

    fn u(s: &str) -> PyValue {
        PyValue::Unicode(s.to_owned())
    }

    fn rows(v: PyValue) -> usize {
        let PyValue::List(l) = v else {
            panic!("a list")
        };
        l.len()
    }

    #[test]
    fn a_director_posts_and_members_see_it() {
        let (mut reg, mine, theirs) = world();
        call_as(
            &mut reg,
            ALICE,
            "AddBulletin",
            &[u("Fleet"), u("Tonight")],
            &[],
        )
        .expect("posted");
        let b = list(&reg, mine);
        assert_eq!(b.len(), 1);
        assert_eq!((b[0].title.as_str(), b[0].edit_char), ("Fleet", ALICE));
        assert_eq!(
            rows(call_as(&mut reg, BOB, "GetBulletins", &[], &[]).unwrap()),
            1
        );
        assert_eq!(
            rows(call_as(&mut reg, CAROL, "GetBulletins", &[], &[]).unwrap()),
            0
        );
        assert!(list(&reg, theirs).is_empty());
    }

    #[test]
    fn a_non_director_is_refused_and_a_chat_manager_is_not() {
        let (mut reg, mine, _) = world();
        let e = call_as(&mut reg, BOB, "AddBulletin", &[u("x"), u("y")], &[]).unwrap_err();
        assert_eq!(format!("{e:?}"), format!("{:?}", refusal(Refusal::Denied)));
        // Another corporation's CEO does not post on ours either.
        assert!(add(&mut reg, CAROL, Scope::Corp, mine, "x", "y").is_err());
        assert!(list(&reg, mine).is_empty());
        reg.corporations_mut()
            .member_mut(BOB)
            .expect("a member")
            .roles = ROLE_CHAT_MANAGER;
        assert!(add(&mut reg, BOB, Scope::Corp, mine, "x", "y").is_ok());
    }

    #[test]
    fn edit_by_keyword_conflict_delete_and_order() {
        let (mut reg, mine, _) = world();
        let a = add(&mut reg, ALICE, Scope::Corp, mine, "One", "1").unwrap();
        let b = add(&mut reg, ALICE, Scope::Corp, mine, "Two", "2").unwrap();
        assert_ne!(a, b);
        let seen = get(&reg, a).unwrap().edit_dt;
        let kw = vec![
            (PyValue::Int(a), PyValue::Bytes(b"bulletinID".to_vec())),
            (PyValue::Int(seen), PyValue::Bytes(b"editDateTime".to_vec())),
        ];
        call_as(&mut reg, ALICE, "AddBulletin", &[u("One!"), u("1!")], &kw).expect("edited");
        let e = get(&reg, a).unwrap();
        assert_eq!(e.title, "One!");
        assert!(e.edit_dt > seen);
        // A second editor with the old copy loses.
        let err = call_as(&mut reg, ALICE, "AddBulletin", &[u("Late"), u("x")], &kw).unwrap_err();
        assert_eq!(
            format!("{err:?}"),
            format!("{:?}", refusal(Refusal::Shotgun))
        );
        assert_eq!(list(&reg, mine).len(), 2, "an edit is not a duplicate");
        // Order, then delete.
        call_as(
            &mut reg,
            ALICE,
            "UpdateBulletinOrder",
            &[PyValue::List(vec![PyValue::Int(b), PyValue::Int(a)])],
            &[],
        )
        .unwrap();
        assert_eq!(list(&reg, mine)[0].id, b);
        call_as(&mut reg, ALICE, "DeleteBulletin", &[PyValue::Int(b)], &[]).unwrap();
        assert_eq!(list(&reg, mine).len(), 1);
        let err = call_as(&mut reg, ALICE, "DeleteBulletin", &[PyValue::Int(b)], &[]).unwrap_err();
        assert_eq!(
            format!("{err:?}"),
            format!("{:?}", refusal(Refusal::NotFound))
        );
        let err = call_as(&mut reg, BOB, "DeleteBulletin", &[PyValue::Int(a)], &[]).unwrap_err();
        assert_eq!(
            format!("{err:?}"),
            format!("{:?}", refusal(Refusal::Denied))
        );
    }

    #[test]
    fn ids_are_unique_across_boards_and_the_cap_holds() {
        let (mut reg, mine, theirs) = world();
        let a = add(&mut reg, ALICE, Scope::Corp, mine, "A", "a").unwrap();
        let c = add(&mut reg, CAROL, Scope::Corp, theirs, "C", "c").unwrap();
        assert_ne!(a, c);
        assert_eq!(get(&reg, c).unwrap().owner, theirs);
        for i in 1..MAX_PER_OWNER {
            add(&mut reg, ALICE, Scope::Corp, mine, &format!("T{i}"), "b").unwrap();
        }
        assert_eq!(
            add(&mut reg, ALICE, Scope::Corp, mine, "T", "b"),
            Err(Refusal::TooMany)
        );
        assert_eq!(
            add(&mut reg, ALICE, Scope::Corp, mine, " ", "b"),
            Err(Refusal::Empty)
        );
    }

    #[test]
    fn bulletins_survive_a_save_and_load() {
        let (mut reg, mine, _) = world();
        let id = add(&mut reg, ALICE, Scope::Corp, mine, "Tab\there", "a\nb %").unwrap();
        let dir = std::env::temp_dir().join(format!(
            "eve-bulletins-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("a directory");
        let store = crate::store::FileStore::new(dir.join("world.txt"));
        crate::store::Store::save(&store, &reg.to_snapshot()).expect("write");
        let snap = crate::store::Store::load(&store)
            .expect("read")
            .expect("a world");
        let _ = std::fs::remove_dir_all(&dir);
        let (mut fresh, _, _) = world();
        fresh.apply_snapshot(snap);
        let b = get(&fresh, id).expect("kept");
        assert_eq!((b.title.as_str(), b.body.as_str()), ("Tab\there", "a\nb %"));
        // The counter survived: the next id is a new one.
        assert_ne!(
            add(&mut fresh, ALICE, Scope::Corp, mine, "N", "n").unwrap(),
            id
        );
    }
}
