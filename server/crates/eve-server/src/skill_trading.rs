//! **Skill trading** (lane l4-injectors, issue #30): CCP's injector and
//! extractor mechanics, served through `skillMgr2::skillHandler`. Core,
//! because it is the client's own feature: the window, the banner, the menu
//! entries and the constants are all in the shipped client. What the economy
//! does with it (where injectors come from, how often) is `eve-mod-econ`.
//!
//! The calls, from `skillsvc.py:935`-`:978` **[V]**:
//!
//! | call | args | answer |
//! |---|---|---|
//! | `CheckInjectionConstraints` | `itemID, quantity` | nothing; **raises** are the point |
//! | `GetDiminishedSpFromInjectors` | `typeID, quantity, nonDiminishingRemaining` | an int, the points |
//! | `InjectSkillpoints` | `itemID, quantity` | nothing |
//! | `SplitSkillInjector` | `itemID, quantity` | nothing; large to small |
//! | `CombineSkillInjector` | `itemID, quantity` | nothing; small to large |
//! | `ExtractSkills` | `{typeID: points}, extractorItemID, token` | nothing |
//!
//! - **Injection** adds points to the **free pool** ([`crate::free_sp::grant`]);
//!   the pilot then applies them (the client opens the apply window). The
//!   table of the constants is `appConst.SKILL_TRADING_*` **[V]**: a large
//!   injector is [`BUCKET`], and the points per injector fall as the pilot's
//!   total rises ([`per_large`]). **[A]** The total is trained plus unallocated
//!   points, and it is **re-read after each injector** of a stack. Falsifier:
//!   the injector window's number for a stack straddling a boundary on TQ.
//! - **Split and combine**: one large is [`DIVISOR`] small. Combine takes a
//!   whole number of fives; the rest of a quantity stays where it is.
//! - **Extraction** removes exactly [`BUCKET`] points from the named skills
//!   and turns one extractor into one large injector, in the same stack
//!   address. The client chooses the levels; the server re-checks every
//!   rule it knows (below). **[A]** The access token is ignored. Falsifier:
//!   TQ refusing a stale token.
//! - **Never from nowhere.** Extraction only moves points: they leave skills
//!   and sit in an injector. The `sptrade` ledger ([`KIND`]) counts what is
//!   minted by a source (`eve-mod-econ` drops) and what only moved
//!   (`extraction`, `injection`).
//! - **Non-diminishing injections** stay 0 (`nonDiminishingInjectionMgr`
//!   answers 0): there is no source for them.
//! - **[A]** An injector is used from the station hangar or the pilot's ship,
//!   like a skillbook. Extraction needs the pilot docked. The client also
//!   offers it only in a capsule; the server does not make that a rule.
//!   Falsifier: a TQ refusal for either.
//!
//! Extraction refuses (the client's own checks, repeated, because a bot or a
//! modified client skips them): not docked; the extractor not the pilot's, not
//! in their station or ship, or not an extractor; trained plus free points under
//! [`MIN_TO_EXTRACT`]; a sum that is not [`BUCKET`]; a skill the pilot lacks, a
//! zero or too large amount; a skill in the queue; a skill that a held or
//! queued skill still needs at the level it needs.

use crate::item::{Edit, Item, ItemType, Slot};
use crate::module::Deltas;
use crate::service::{Registry, Request, ServiceError};
use crate::skill::{MAX_LEVEL, level_for_points, sp_for_level};
use eve_marshal::PyValue;

/// The delta kind: `ledger key -> minted moved`, whole points. The ledger of
/// skill points that enter and leave injectors.
pub const KIND: &str = "sptrade";

/// `typeSkillExtractor` **[V]** (`inventorycommon/const.py`).
pub const TYPE_EXTRACTOR: i64 = 40_519;
/// `typeSkillInjector`, the large one **[V]**.
pub const TYPE_LARGE: i64 = 40_520;
/// `typeSmallSkillInjector` **[V]**.
pub const TYPE_SMALL: i64 = 45_635;
/// `groupSkillInjectors` **[V]**.
pub const GROUP_INJECTORS: i64 = 1_739;

/// `SKILL_TRADING_BUCKET_SIZE`: the points one large injector holds, and
/// what one extraction takes **[V]**.
pub const BUCKET: i64 = 500_000;
/// `SKILL_TRADING_FREE_ZONE` **[V]**: below this total, no diminishing.
pub const FREE_ZONE: i64 = 5_000_000;
/// `SKILL_TRADING_LOW_DIMINISH` **[V]**: up to this total, the first cut.
pub const LOW_LIMIT: i64 = 50_000_000;
/// `SKILL_TRADING_MEDIUM_DIMINISH` **[V]**: up to this total, the second.
pub const MEDIUM_LIMIT: i64 = 80_000_000;
/// `SKILL_TRADING_MINIMUM_SP_TO_EXTRACT` **[V]**.
pub const MIN_TO_EXTRACT: i64 = FREE_ZONE + BUCKET;
/// `SKILL_TRADING_MAXIMUM_INJECT_STACK_SIZE` **[V]**.
pub const MAX_STACK: i64 = 10_000;
/// `SKILL_TRADING_SMALL_INJECTOR_DIVISOR` **[V]**: small injectors in a large.
pub const DIVISOR: i64 = 5;

/// The bundle's file for the three item types and its header
/// (`tools/content_deploy.py`, the `deploy` section): the store's `types.tsv`
/// has the extractor only, and an item of an unknown type cannot be created.
pub const ROWS_FILE: &str = "deploy/injectors.tsv";
pub const ROWS_HEADER: &str = "#type_id\tgroup_id\tcategory_id\tsingleton\tvolume\tname";

/// The item rows of the extractor and the two injectors, from the bundle.
#[derive(Debug, Clone, Default)]
pub struct Rows(Vec<ItemType>);

impl Rows {
    /// Build from the bundle's data lines; a line that is not six fields of
    /// the right kind is an error naming it.
    pub fn parse<S: AsRef<str>>(lines: &[S]) -> Result<Rows, String> {
        let mut out = Vec::new();
        for (n, l) in lines.iter().enumerate() {
            let bad = || format!("{ROWS_FILE} data line {}: {:?}", n + 1, l.as_ref());
            let f: Vec<&str> = l.as_ref().split('\t').collect();
            let [id, group, category, singleton, volume, name] = f[..] else {
                return Err(bad());
            };
            out.push(ItemType {
                type_id: id.parse().map_err(|_| bad())?,
                // Once per type at startup: the table is loaded once.
                name: Box::leak(name.to_owned().into_boxed_str()),
                group_id: group.parse().map_err(|_| bad())?,
                category_id: category.parse().map_err(|_| bad())?,
                singleton: singleton == "1",
                volume: volume.parse().map_err(|_| bad())?,
                capacity: 0.0,
            });
        }
        Ok(Rows(out))
    }
}

/// The three item types, for [`crate::item::find_in`], from `content`
/// (empty when the bundle has no `deploy/injectors.tsv`).
pub fn item_types_in(content: &crate::content::Content) -> &[ItemType] {
    content.try_get::<Rows>().map_or(&[][..], |r| &r.0[..])
}

/// This table's registration row: one line in `content_tables::TABLES`.
pub const TABLE: crate::content::TableSpec = crate::content::TableSpec {
    name: "injectors",
    load: boxed_load,
    fixture: boxed_fixture,
};

fn boxed_load(src: &crate::content::Sources) -> Result<crate::content::Boxed, String> {
    let Some(b) = src.bundle else {
        return Ok(Box::new(Rows::default()));
    };
    let lines = b.data_lines(ROWS_FILE, ROWS_HEADER)?;
    if lines.is_empty() {
        src.note(crate::content::Origin::Fixture);
    }
    Ok(Box::new(Rows::parse(&lines)?))
}

/// Invented rows for tests: the ids are the client's constants above, the
/// names, groups and volumes are not its rows.
pub mod fixture {
    use super::*;

    pub fn rows() -> Vec<String> {
        [
            (TYPE_EXTRACTOR, "Fixture Extractor"),
            (TYPE_LARGE, "Fixture Large Injector"),
            (TYPE_SMALL, "Fixture Small Injector"),
        ]
        .iter()
        .map(|(id, name)| format!("{id}\t{GROUP_INJECTORS}\t17\t0\t0.5\t{name}"))
        .collect()
    }
}

fn boxed_fixture() -> crate::content::Boxed {
    Box::new(Rows::parse(&fixture::rows()).expect("the fixture rows are well formed"))
}

/// Points one large injector gives a pilot whose total is `total` **[A]**
/// (the tiers are the client's constants **[V]**; reading them by the total at
/// the moment of each injector is the assumption in the header).
pub fn per_large(total: i64) -> i64 {
    // The cuts are 20 %, 40 % and 70 % of the bucket.
    let (num, den) = match total {
        t if t < FREE_ZONE => (1, 1),
        t if t < LOW_LIMIT => (4, 5),
        t if t < MEDIUM_LIMIT => (3, 5),
        _ => (3, 10),
    };
    BUCKET * num / den
}

/// Points one injector of `type_id` gives at `total`: a small is a fifth.
pub fn per_injector(type_id: i64, total: i64) -> i64 {
    if type_id == TYPE_SMALL {
        per_large(total) / DIVISOR
    } else {
        per_large(total)
    }
}

/// Points `quantity` injectors of `type_id` give a pilot whose total starts at
/// `total`, **re-reading the total after each one**.
pub fn diminished(type_id: i64, quantity: i64, total: i64) -> i64 {
    let mut now = total;
    let mut given = 0i64;
    for _ in 0..quantity.clamp(0, MAX_STACK) {
        let p = per_injector(type_id, now);
        given = given.saturating_add(p);
        now = now.saturating_add(p);
    }
    given
}

// ---- the ledger ----

fn led_key(key: &str) -> Vec<String> {
    vec![key.to_owned()]
}

/// `(minted, moved)` under `key`.
pub fn ledger_of(d: &Deltas, key: &str) -> (i64, i64) {
    d.of(KIND)
        .get(&led_key(key))
        .map(|v| {
            let n = |i: usize| v.get(i).and_then(|s| s.parse().ok()).unwrap_or(0);
            (n(0), n(1))
        })
        .unwrap_or((0, 0))
}

/// Count points **made** by a source under `key` (`drop:<source>`): the
/// injector exists with its points, so the points come from there and nowhere
/// else.
pub fn ledger_minted(d: &mut Deltas, key: &str, points: i64) {
    let (m, v) = ledger_of(d, key);
    d.put(
        KIND,
        led_key(key),
        vec![(m + points).to_string(), v.to_string()],
    );
}

/// Count points that only **moved** under `key` (`extraction`, `injection`).
pub fn ledger_moved(d: &mut Deltas, key: &str, points: i64) {
    let (m, v) = ledger_of(d, key);
    d.put(
        KIND,
        led_key(key),
        vec![m.to_string(), (v + points).to_string()],
    );
}

// ---- helpers ----

fn bad(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

fn refuse(label: &str, fields: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(label, crate::pyutil::pypairs(fields)))
}

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v {
        Some(PyValue::Int(i)) => Some(*i),
        Some(PyValue::Real(r)) if r.fract() == 0.0 => Some(*r as i64),
        _ => None,
    }
}

/// The pilot's total for the table: trained points plus the unallocated pool.
fn total_of(reg: &Registry, id: i64) -> i64 {
    reg.skills.get(&id).map_or(0, |s| s.total_points()) + crate::free_sp::of(reg, id)
}

/// The item `item_id`, if the pilot owns it, it is at least `quantity` of an
/// injector of one of `types`, and it is in their station or their ship.
fn own_stack<'r>(
    reg: &'r Registry,
    req: &Request<'_>,
    actor: i64,
    item_id: i64,
    types: &[i64],
) -> Result<&'r Item, ServiceError> {
    let c = reg
        .character(actor)
        .ok_or_else(|| bad(req, "no such character"))?;
    reg.store()
        .get(item_id)
        .filter(|i| i.owner_id == actor && types.contains(&i.type_id))
        .filter(|i| Some(i.location_id) == c.station_id || i.location_id == c.ship_id)
        .ok_or_else(|| bad(req, &format!("item {item_id} is not yours to use here")))
}

/// Edits that put `quantity` of `type_id` at `slot`: grow the stack there if
/// there is one, else create it.
fn add_to(reg: &Registry, slot: Slot, type_id: i64, quantity: i64) -> Edit {
    match reg.store().at(slot).find(|i| i.type_id == type_id) {
        Some(i) => Edit::Grow {
            item_id: i.item_id,
            quantity,
        },
        None => Edit::Create {
            type_id,
            quantity,
            to: slot,
        },
    }
}

/// Edits that take `quantity` off `item`.
fn take_from(item: &Item, quantity: i64) -> Edit {
    if quantity >= item.quantity {
        Edit::Destroy {
            item_id: item.item_id,
        }
    } else {
        Edit::Consume {
            item_id: item.item_id,
            quantity,
        }
    }
}

/// The client's checks before an injection (`CheckInjectionConstraints`), as
/// the labels it knows: a quantity the stack lacks is
/// `InconsistentInjectorQuantity`, one over [`MAX_STACK`]
/// `TooManyInjectorsInjected`.
fn check_quantity(item: &Item, quantity: i64) -> Result<(), ServiceError> {
    if quantity > MAX_STACK {
        return Err(refuse("TooManyInjectorsInjected", vec![]));
    }
    if quantity < 1 || quantity > item.quantity {
        return Err(refuse("InconsistentInjectorQuantity", vec![]));
    }
    Ok(())
}

fn item_and_quantity(req: &Request<'_>) -> Result<(i64, i64), ServiceError> {
    match (int(req.args.first()), int(req.args.get(1))) {
        (Some(i), Some(q)) => Ok((i, q)),
        _ => Err(bad(req, "(itemID, quantity)")),
    }
}

// ---- the calls ----

/// `CheckInjectionConstraints(itemID, quantity)`.
pub(crate) fn check_injection(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Ok(PyValue::None);
    };
    let (item_id, quantity) = item_and_quantity(req)?;
    let item = own_stack(reg, req, actor, item_id, &[TYPE_LARGE, TYPE_SMALL])?;
    check_quantity(item, quantity)?;
    Ok(PyValue::None)
}

/// `GetDiminishedSpFromInjectors(typeID, quantity, nonDiminishingRemaining)`:
/// the points the stack would give this pilot now. The third argument is
/// ignored: non-diminishing injections are 0.
pub(crate) fn diminished_sp(reg: &Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let (Some(type_id), Some(quantity)) = (int(req.args.first()), int(req.args.get(1))) else {
        return Err(bad(req, "(typeID, quantity, nonDiminishing)"));
    };
    let total = req.actor.map_or(0, |id| total_of(reg, id));
    Ok(PyValue::Int(diminished(type_id, quantity, total)))
}

/// `InjectSkillpoints(itemID, quantity)`: consume the injectors, grant the
/// points to the free pool.
pub(crate) fn inject(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Ok(PyValue::None);
    };
    let (item_id, quantity) = item_and_quantity(req)?;
    let item = own_stack(reg, req, actor, item_id, &[TYPE_LARGE, TYPE_SMALL])?.clone();
    check_quantity(&item, quantity)?;
    let points = diminished(item.type_id, quantity, total_of(reg, actor));
    reg.apply_items(&[take_from(&item, quantity)])
        .map_err(|e| bad(req, &format!("consuming the injectors: {e:?}")))?;
    let d = reg.deltas_mut();
    ledger_moved(d, "injection", points);
    crate::free_sp::grant(reg, actor, points);
    Ok(PyValue::None)
}

/// `SplitSkillInjector(itemID, quantity)`: `quantity` large become
/// `quantity * 5` small, where they were.
pub(crate) fn split(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Ok(PyValue::None);
    };
    let (item_id, quantity) = item_and_quantity(req)?;
    let item = own_stack(reg, req, actor, item_id, &[TYPE_LARGE])?.clone();
    check_quantity(&item, quantity)?;
    let edits = [
        take_from(&item, quantity),
        add_to(reg, item.slot(), TYPE_SMALL, quantity * DIVISOR),
    ];
    reg.apply_items(&edits)
        .map_err(|e| bad(req, &format!("splitting: {e:?}")))?;
    Ok(PyValue::None)
}

/// `CombineSkillInjector(itemID, quantity)`: every five small of `quantity`
/// become one large; what is not a multiple of five is left.
pub(crate) fn combine(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Ok(PyValue::None);
    };
    let (item_id, quantity) = item_and_quantity(req)?;
    let item = own_stack(reg, req, actor, item_id, &[TYPE_SMALL])?.clone();
    let larges = quantity / DIVISOR;
    if larges < 1 {
        return Err(refuse(
            "CombineSkillInjectorTooFewInjectors",
            vec![
                ("minQuantity", PyValue::Int(DIVISOR)),
                ("injectorType", PyValue::Int(TYPE_SMALL)),
            ],
        ));
    }
    let used = larges * DIVISOR;
    if used > item.quantity {
        return Err(refuse("InconsistentInjectorQuantity", vec![]));
    }
    let edits = [
        take_from(&item, used),
        add_to(reg, item.slot(), TYPE_LARGE, larges),
    ];
    reg.apply_items(&edits)
        .map_err(|e| bad(req, &format!("combining: {e:?}")))?;
    Ok(PyValue::None)
}

/// The `{typeID: points}` dict of `ExtractSkills`, in order (a dict is
/// `(value, key)` on the wire).
fn extraction_dict(req: &Request<'_>) -> Result<Vec<(i64, i64)>, ServiceError> {
    let Some(PyValue::Dict(entries)) = req.args.first() else {
        return Err(bad(req, "(skills, itemID, token)"));
    };
    let mut out: Vec<(i64, i64)> = Vec::new();
    for (value, key) in entries {
        let (Some(t), Some(p)) = (int(Some(key)), int(Some(value))) else {
            return Err(bad(req, "a skill entry that is not ints"));
        };
        if out.iter().any(|e| e.0 == t) {
            return Err(bad(req, "a skill named twice"));
        }
        out.push((t, p));
    }
    Ok(out)
}

/// Why the extraction of `take` (`(typeID, points)`) from this pilot is not
/// allowed, if it is not. Pure over the registry; nothing changes.
fn extraction_refusal(
    reg: &Registry,
    req: &Request<'_>,
    actor: i64,
    take: &[(i64, i64)],
) -> Option<ServiceError> {
    let skills = reg.skills.get(&actor)?;
    let queued: Vec<i64> = reg
        .skill_queues
        .get(actor)
        .map(|q| q.entries.iter().map(|e| e.0).collect())
        .unwrap_or_default();
    if total_of(reg, actor) < MIN_TO_EXTRACT {
        return Some(refuse(
            "SkillExtractionNotEnoughSP",
            vec![
                ("limit", PyValue::Int(MIN_TO_EXTRACT)),
                ("extractor", PyValue::Int(TYPE_EXTRACTOR)),
            ],
        ));
    }
    if take.iter().map(|t| t.1).sum::<i64>() != BUCKET {
        return Some(bad(
            req,
            &format!("an extraction takes exactly {BUCKET} points"),
        ));
    }
    let after = |t: i64| {
        let held = skills.get(t).map_or(0, |s| s.points);
        held - take.iter().find(|x| x.0 == t).map_or(0, |x| x.1)
    };
    for &(t, p) in take {
        let Some(s) = skills.get(t) else {
            return Some(bad(req, &format!("the pilot has no skill {t}")));
        };
        if p <= 0 || p > s.points {
            return Some(bad(req, &format!("{p} points is not available in {t}")));
        }
        if queued.contains(&t) {
            return Some(bad(req, &format!("skill {t} is in the queue")));
        }
        // A skill that a held or queued skill needs may not fall under the
        // level it needs. The client keeps the skill at exactly that level.
        for d in skills.entries() {
            if after(d.type_id) <= 0 && !queued.contains(&d.type_id) {
                continue;
            }
            for &(need, level) in crate::skill::required_in(reg.content(), d.type_id) {
                if need == t && after(t) < sp_for_level(s.rank, level.min(MAX_LEVEL)) {
                    return Some(bad(
                        req,
                        &format!("skill {d} needs {t} at level {level}", d = d.type_id),
                    ));
                }
            }
        }
        for &d in &queued {
            for &(need, level) in crate::skill::required_in(reg.content(), d) {
                if need == t && after(t) < sp_for_level(s.rank, level.min(MAX_LEVEL)) {
                    return Some(bad(
                        req,
                        &format!("queued skill {d} needs {t} at level {level}"),
                    ));
                }
            }
        }
    }
    None
}

/// `ExtractSkills(skills, extractorItemID, token)`: take [`BUCKET`] points off
/// the named skills, consume one extractor, make one large injector.
pub(crate) fn extract(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Ok(PyValue::None);
    };
    let take = extraction_dict(req)?;
    let Some(extractor_id) = int(req.args.get(1)) else {
        return Err(bad(req, "(skills, itemID, token)"));
    };
    if reg.docked_station(actor).is_none() {
        return Err(refuse(
            "SkillExtractorNotDockedInStation",
            vec![("extractor", PyValue::Int(TYPE_EXTRACTOR))],
        ));
    }
    let extractor = own_stack(reg, req, actor, extractor_id, &[TYPE_EXTRACTOR])?.clone();
    if let Some(e) = extraction_refusal(reg, req, actor, &take) {
        return Err(e);
    }
    // One extractor in, one injector out, in the extractor's own address.
    let edits = [
        take_from(&extractor, 1),
        add_to(reg, extractor.slot(), TYPE_LARGE, 1),
    ];
    reg.apply_items(&edits)
        .map_err(|e| bad(req, &format!("trading the extractor: {e:?}")))?;
    // Freeze the head so what it trained until now is credited before the
    // points move; the thaw tells the client about every changed skill.
    let mut frozen = crate::skill_queue::freeze_training(reg, actor);
    let mut moved = Vec::new();
    if let Some(skills) = reg.skills.get_mut(&actor) {
        for &(t, p) in &take {
            if let Some(s) = skills.get_mut(t) {
                s.points -= p;
                s.level = level_for_points(s.rank, s.points);
                moved.push(t);
            }
        }
    }
    reg.mark_dirty();
    frozen.note(&moved);
    crate::skill_queue::thaw_training(reg, actor, frozen);
    ledger_moved(reg.deltas_mut(), "extraction", BUCKET);
    crate::brain::announce(reg, actor);
    Ok(PyValue::None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FLAG_HANGAR, SandboxConfig};

    const FRIGATE: i64 = 3329;

    fn actor(reg: &Registry) -> i64 {
        reg.config().characters[0].character_id
    }

    fn call(reg: &mut Registry, method: &str, args: &[PyValue]) -> Result<PyValue, ServiceError> {
        let actor = Some(actor(reg));
        reg.dispatch(&Request {
            service: "skillMgr2::skillHandler",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor,
        })
    }

    fn label(e: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            return format!("{e:?}");
        };
        let PyValue::Tuple(h) = &**header else {
            panic!("a call header")
        };
        let PyValue::Tuple(args) = &h[1] else {
            panic!("user error args")
        };
        let PyValue::Bytes(b) = &args[0] else {
            panic!("a label")
        };
        String::from_utf8(b.clone()).expect("utf8")
    }

    /// A pilot who owns Minmatar Frigate, with so many points in it that the
    /// pilot's trained total is `total` (the sandbox pilot starts with some
    /// skills, so the total is not the frigate's alone).
    fn world(total: i64) -> Registry {
        let mut reg = Registry::new(SandboxConfig::default());
        call(
            &mut reg,
            "PurchaseSkills",
            &[PyValue::List(vec![PyValue::Int(FRIGATE)])],
        )
        .expect("bought");
        let id = actor(&reg);
        let skills = reg.skills.get_mut(&id).expect("skills");
        let others = skills.total_points();
        skills.get_mut(FRIGATE).expect("owned").points = (total - others).max(0);
        reg.take_pending();
        reg
    }

    fn hangar(reg: &Registry) -> Slot {
        let id = actor(reg);
        Slot::new(
            id,
            reg.character(id).unwrap().station_id.unwrap(),
            FLAG_HANGAR,
        )
    }

    /// Put `quantity` of `type_id` in the hangar; returns the item id.
    fn give(reg: &mut Registry, type_id: i64, quantity: i64) -> i64 {
        let to = hangar(reg);
        reg.apply_items(&[Edit::Create {
            type_id,
            quantity,
            to,
        }])
        .expect("created");
        reg.take_pending();
        count_item(reg, type_id).0
    }

    /// `(item id, quantity)` of the hangar's stack of a type, `(0, 0)` if none.
    fn count_item(reg: &Registry, type_id: i64) -> (i64, i64) {
        reg.store()
            .at(hangar(reg))
            .filter(|i| i.type_id == type_id)
            .map(|i| (i.item_id, i.quantity))
            .next()
            .unwrap_or((0, 0))
    }

    fn ints(v: &[i64]) -> Vec<PyValue> {
        v.iter().map(|i| PyValue::Int(*i)).collect()
    }

    fn extract_args(item: i64, skill: i64, points: i64) -> Vec<PyValue> {
        vec![
            PyValue::Dict(vec![(PyValue::Int(points), PyValue::Int(skill))]),
            PyValue::Int(item),
            PyValue::None,
        ]
    }

    /// The three item rows are the table's: found by id in the content, with
    /// the row's fields; a bundle without the file has none; a bad line names
    /// itself.
    #[test]
    fn the_item_rows_are_read_from_the_table() {
        let c = crate::content::Content::fixture();
        for id in [TYPE_EXTRACTOR, TYPE_LARGE, TYPE_SMALL] {
            let t = crate::item::find_in(&c, id).expect("a row");
            assert_eq!(
                (t.group_id, t.category_id, t.volume),
                (GROUP_INJECTORS, 17, 0.5)
            );
            assert!(!t.singleton);
        }
        assert!(item_types_in(&crate::content::Content::default()).is_empty());
        assert!(Rows::parse(&["1\t2\t3"]).is_err());
        assert!(Rows::parse(&["1\t2\t3\t0\tx\tName"]).is_err());
        let one = Rows::parse(&["1\t2\t3\t1\t0.25\tName"]).unwrap();
        assert_eq!(
            (one.0[0].name, one.0[0].singleton, one.0[0].volume),
            ("Name", true, 0.25)
        );
    }

    #[test]
    fn the_table_changes_at_each_boundary() {
        assert_eq!(per_large(0), BUCKET);
        assert_eq!(per_large(FREE_ZONE - 1), BUCKET);
        assert_eq!(per_large(FREE_ZONE), 400_000);
        assert_eq!(per_large(LOW_LIMIT - 1), 400_000);
        assert_eq!(per_large(LOW_LIMIT), 300_000);
        assert_eq!(per_large(MEDIUM_LIMIT - 1), 300_000);
        assert_eq!(per_large(MEDIUM_LIMIT), 150_000);
        assert_eq!(per_large(i64::MAX), 150_000);
        // A small injector is a fifth of a large one at every tier.
        for t in [0, FREE_ZONE, LOW_LIMIT, MEDIUM_LIMIT] {
            assert_eq!(per_injector(TYPE_SMALL, t) * DIVISOR, per_large(t));
        }
    }

    #[test]
    fn a_stack_is_priced_one_injector_at_a_time() {
        // One short of the free zone: the first injector is full, which lifts
        // the total over the line, so the next two are cut.
        let start = FREE_ZONE - 1;
        assert_eq!(diminished(TYPE_LARGE, 1, start), 500_000);
        assert_eq!(
            diminished(TYPE_LARGE, 3, start),
            500_000 + 400_000 + 400_000
        );
        // Not what pricing the whole stack at the start total would give.
        assert_ne!(diminished(TYPE_LARGE, 3, start), 3 * per_large(start));
        assert_eq!(diminished(TYPE_LARGE, 0, 0), 0);
        // Across the second boundary.
        let near = LOW_LIMIT - 100_000;
        assert_eq!(diminished(TYPE_LARGE, 2, near), 400_000 + 300_000);
    }

    #[test]
    fn injection_consumes_the_stack_and_fills_the_pool() {
        let mut reg = world(1_000);
        let id = actor(&reg);
        let stack = give(&mut reg, TYPE_LARGE, 3);
        assert_eq!(
            call(
                &mut reg,
                "GetDiminishedSpFromInjectors",
                &ints(&[TYPE_LARGE, 2, 0])
            ),
            Ok(PyValue::Int(1_000_000))
        );
        call(&mut reg, "InjectSkillpoints", &ints(&[stack, 2])).expect("injected");
        assert_eq!(crate::free_sp::of(&reg, id), 1_000_000);
        assert_eq!(count_item(&reg, TYPE_LARGE).1, 1);
        assert_eq!(ledger_of(reg.deltas(), "injection"), (0, 1_000_000));
        // The pool counts toward the next price: 1M + 1k, nowhere near a cut.
        call(&mut reg, "InjectSkillpoints", &ints(&[stack, 1])).expect("injected");
        assert_eq!(crate::free_sp::of(&reg, id), 1_500_000);
        assert_eq!(count_item(&reg, TYPE_LARGE), (0, 0));
    }

    #[test]
    fn injection_refuses_what_the_stack_cannot_give() {
        let mut reg = world(0);
        let stack = give(&mut reg, TYPE_LARGE, 3);
        for (q, want) in [
            (0, "InconsistentInjectorQuantity"),
            (4, "InconsistentInjectorQuantity"),
            (MAX_STACK + 1, "TooManyInjectorsInjected"),
        ] {
            for m in ["CheckInjectionConstraints", "InjectSkillpoints"] {
                let e = call(&mut reg, m, &ints(&[stack, q])).unwrap_err();
                assert_eq!(label(&e), want, "{m} {q}");
            }
        }
        assert_eq!(count_item(&reg, TYPE_LARGE).1, 3);
        assert_eq!(
            call(&mut reg, "CheckInjectionConstraints", &ints(&[stack, 3])),
            Ok(PyValue::None)
        );
        // A skillbook (any other type) is not an injector.
        let other = give(&mut reg, TYPE_EXTRACTOR, 1);
        assert!(call(&mut reg, "InjectSkillpoints", &ints(&[other, 1])).is_err());
    }

    #[test]
    fn split_and_combine_trade_one_for_five() {
        let mut reg = world(0);
        let large = give(&mut reg, TYPE_LARGE, 2);
        call(&mut reg, "SplitSkillInjector", &ints(&[large, 2])).expect("split");
        assert_eq!(count_item(&reg, TYPE_LARGE), (0, 0));
        let (small, n) = count_item(&reg, TYPE_SMALL);
        assert_eq!(n, 2 * DIVISOR);
        // Too few for one large.
        let e = call(&mut reg, "CombineSkillInjector", &ints(&[small, 4])).unwrap_err();
        assert_eq!(label(&e), "CombineSkillInjectorTooFewInjectors");
        // 7 asked: one large made, five used, the rest stays.
        call(&mut reg, "CombineSkillInjector", &ints(&[small, 7])).expect("combined");
        assert_eq!(count_item(&reg, TYPE_LARGE).1, 1);
        assert_eq!(count_item(&reg, TYPE_SMALL).1, DIVISOR);
        // More than the stack has.
        let e = call(&mut reg, "CombineSkillInjector", &ints(&[small, 15])).unwrap_err();
        assert_eq!(label(&e), "InconsistentInjectorQuantity");
        // A small one cannot be split, a large one cannot be combined.
        assert!(call(&mut reg, "SplitSkillInjector", &ints(&[small, 1])).is_err());
    }

    #[test]
    fn extraction_conserves_points_and_swaps_the_item() {
        let mut reg = world(MIN_TO_EXTRACT);
        let id = actor(&reg);
        let extractor = give(&mut reg, TYPE_EXTRACTOR, 2);
        let before = reg.skills[&id].total_points();
        call(
            &mut reg,
            "ExtractSkills",
            &extract_args(extractor, FRIGATE, BUCKET),
        )
        .expect("done");
        let after = reg.skills[&id].total_points();
        assert_eq!(before - after, BUCKET);
        assert_eq!(count_item(&reg, TYPE_EXTRACTOR).1, 1);
        assert_eq!(count_item(&reg, TYPE_LARGE).1, 1);
        assert_eq!(ledger_of(reg.deltas(), "extraction"), (0, BUCKET));
        // The level follows the points.
        let s = reg.skills[&id].get(FRIGATE).unwrap().clone();
        assert_eq!(s.level, level_for_points(s.rank, s.points));
        // Nothing was made: the injector's worth is what left the skill.
        assert_eq!(per_large(0), BUCKET);
    }

    #[test]
    fn extraction_refuses_each_thing_it_should() {
        let mut reg = world(MIN_TO_EXTRACT - 1);
        let id = actor(&reg);
        let extractor = give(&mut reg, TYPE_EXTRACTOR, 1);
        let go = |reg: &mut Registry, skill: i64, points: i64| {
            call(
                reg,
                "ExtractSkills",
                &extract_args(extractor, skill, points),
            )
        };
        // Under the minimum.
        let e = go(&mut reg, FRIGATE, BUCKET).unwrap_err();
        assert_eq!(label(&e), "SkillExtractionNotEnoughSP");
        // The free pool counts toward the minimum.
        crate::free_sp::grant(&mut reg, id, 1);
        assert!(go(&mut reg, FRIGATE, BUCKET - 1).is_err(), "not the bucket");
        assert!(go(&mut reg, FRIGATE + 1, BUCKET).is_err(), "not held");
        assert!(go(&mut reg, FRIGATE, 0).is_err());
        // A queued skill is not extracted from.
        reg.skill_queues.entry(id).entries.push((FRIGATE, 1));
        assert!(go(&mut reg, FRIGATE, BUCKET).is_err(), "queued");
        reg.skill_queues.entry(id).entries.clear();
        // In space.
        let ship = reg.character(id).unwrap().ship_id;
        let system = reg.character(id).unwrap().solar_system_id.unwrap();
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                id,
                ship,
                system,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        let e = go(&mut reg, FRIGATE, BUCKET).unwrap_err();
        assert_eq!(label(&e), "SkillExtractorNotDockedInStation");
        // Nothing moved through all of that.
        assert_eq!(reg.skills[&id].total_points(), MIN_TO_EXTRACT - 1);
        assert_eq!(count_item(&reg, TYPE_EXTRACTOR).1, 1);
        assert_eq!(ledger_of(reg.deltas(), "extraction"), (0, 0));
    }

    #[test]
    fn the_ledger_counts_minted_and_moved_apart() {
        let mut reg = world(0);
        ledger_minted(reg.deltas_mut(), "drop:t", 500_000);
        ledger_minted(reg.deltas_mut(), "drop:t", 100_000);
        ledger_moved(reg.deltas_mut(), "drop:t", 7);
        assert_eq!(ledger_of(reg.deltas(), "drop:t"), (600_000, 7));
        assert_eq!(ledger_of(reg.deltas(), "none"), (0, 0));
    }

    #[test]
    fn the_item_types_are_known_to_the_store() {
        let reg = world(0);
        for t in [TYPE_EXTRACTOR, TYPE_LARGE, TYPE_SMALL] {
            assert!(crate::item::find_in(reg.content(), t).is_some(), "{t}");
        }
    }
}
