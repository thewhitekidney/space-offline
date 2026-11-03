//! Personal skill plans for the public gateway (`crate::gateway`).
//!
//! The client keeps a pilot's skill plans and their milestones on the
//! gateway, not in machoNet (`skills/skillplan/grpc/personalSkillPlanRequest`,
//! `personalMilestoneRequest`, the messengers beside them) **[V]**. Before
//! this module every call got an empty reply, and the plan window could not
//! save anything.
//!
//! What the client does, read from its own code **[V]**:
//!
//! - `GetAll` -> the plan ids; each is then read with `Get` (its
//!   `Attributes`: name, description, `SkillRequirement` list of
//!   `(skill type, level)`).
//! - `Create` -> the new id; the client has already checked its own limits
//!   (`skillPlanConst`: 10 plans, 150 skills, name 50, description 1000,
//!   5 milestones). The server refuses past them as well, with `412`
//!   (`SkillPlansErrorInvalidData`).
//! - `GetActive` -> the tracked plan. **"None" is the all-zero uuid**, not
//!   an empty message: `personalSkillPlanRequest.GetActive` does
//!   `uuid.UUID(bytes=response.skill_plan.uuid)`, which raises on an empty
//!   `bytes`. `SetActive` sends the zero uuid to untrack.
//! - `GetShared` -> another pilot's plan by id (a plan link in chat), or
//!   `404`.
//! - Corporation plans are not served: reads answer `404`/empty, writes `403`
//!   **[A]** (falsifier: a corp skill-plan tab that shows an error banner
//!   instead of an empty list).
//!
//! **Stored** as delta kind [`KIND`] (`delta_kinds::KINDS`), one pilot's rows
//! keyed by the pilot first:
//!
//! - `char "p" plan -> name description requirements` where requirements is
//!   `type:level;type:level`.
//! - `char "a" -> plan`, the tracked plan.
//! - `char "m" milestone -> plan train_type skill_type skill_level
//!   description`, a milestone; a milestone trains either to a type
//!   (`train_type`) or to a skill level (`skill_type`, `skill_level`).
//!
//! Ids are 16 bytes: the pilot's id then a serial one past the highest
//! stored, both big-endian. The client only compares them.

use crate::delta::Deltas;
use crate::gateway::{Ground, Reply, field, get_bytes, get_uint, put_bytes, put_varint};

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "skillplan";

/// `skillPlanConst.MAX_PERSONAL_PLANS` **[V]**.
pub const MAX_PLANS: usize = 10;
/// `skillPlanConst.MAX_NUM_SKILLS` **[V]**.
pub const MAX_SKILLS: usize = 150;
/// `skillPlanConst.MAX_LEN_NAME` **[V]**.
pub const MAX_NAME: usize = 50;
/// `skillPlanConst.MAX_LEN_DESC` **[V]**.
pub const MAX_DESC: usize = 1000;
/// `skillPlanConst.MAX_NUM_MILESTONES` **[V]**.
pub const MAX_MILESTONES: usize = 5;

const PKG: &str = "eve_public.character.skill.plan.";
const CORP_PKG: &str = "eve_public.corporation.skill.plan.";

fn reply(status: u32, payload: Option<(String, Vec<u8>)>) -> Reply {
    Reply { status, payload }
}

fn ok(base: &str, value: Vec<u8>) -> Reply {
    reply(200, Some((format!("{base}Response"), value)))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok())
        .collect()
}

/// The uuid inside an `Identifier { bytes uuid = 1 }`, as lowercase hex.
fn ident(msg: Option<&[u8]>) -> Option<String> {
    let id = get_bytes(msg?, 1)?;
    (id.len() == 16).then(|| hex(&id))
}

fn identifier(h: &str) -> Vec<u8> {
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &unhex(h).unwrap_or_default());
    out
}

/// A pilot's rows of one shape, by key.
fn rows<'a>(d: &'a Deltas, who: &str, tag: &str) -> Vec<(&'a [String], &'a Vec<String>)> {
    d.of(KIND)
        .iter()
        .filter(|(k, _)| k.len() >= 3 && k[0] == who && k[1] == tag)
        .map(|(k, v)| (k.as_slice(), v))
        .collect()
}

fn key(who: &str, tag: &str, id: &str) -> Vec<String> {
    vec![who.to_owned(), tag.to_owned(), id.to_owned()]
}

/// A fresh id for `who`: the pilot, then one past the highest serial stored.
fn mint(d: &Deltas, who: i64) -> String {
    let serial = d
        .of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 3)
        .filter_map(|(k, _)| u64::from_str_radix(k[2].get(16..32)?, 16).ok())
        .max()
        .map_or(1, |m| m + 1);
    let mut b = (who as u64).to_be_bytes().to_vec();
    b.extend_from_slice(&serial.to_be_bytes());
    hex(&b)
}

// ---- skill requirements --------------------------------------------------

/// `SkillRequirement { skilltype.Identifier skill_type = 1; uint32 level = 2 }`
/// -> `(type, level)`.
fn requirement(msg: &[u8]) -> Option<(u64, u64)> {
    let ty = get_uint(&get_bytes(msg, 1)?, 1)?;
    Some((ty, get_uint(msg, 2).unwrap_or(0)))
}

fn requirements(msgs: &[Vec<u8>]) -> Option<String> {
    let mut out = Vec::new();
    for m in msgs {
        let (t, l) = requirement(m)?;
        if t == 0 || l > 5 {
            return None;
        }
        out.push(format!("{t}:{l}"));
    }
    (out.len() <= MAX_SKILLS).then(|| out.join(";"))
}

fn requirement_msg(t: u64, l: u64) -> Vec<u8> {
    let mut ty = Vec::new();
    put_varint(&mut ty, 1 << 3);
    put_varint(&mut ty, t);
    let mut out = Vec::new();
    put_bytes(&mut out, 1, &ty);
    put_varint(&mut out, 2 << 3);
    put_varint(&mut out, l);
    out
}

/// `skill.plan.Attributes { name = 1; requirements = 2; description = 3 }`.
fn attributes(row: &[String]) -> Vec<u8> {
    let [name, desc, reqs] = row else {
        return Vec::new();
    };
    let mut out = Vec::new();
    put_bytes(&mut out, 1, name.as_bytes());
    for r in reqs.split(';').filter(|r| !r.is_empty()) {
        if let Some((t, l)) = r.split_once(':')
            && let (Ok(t), Ok(l)) = (t.parse(), l.parse())
        {
            put_bytes(&mut out, 2, &requirement_msg(t, l));
        }
    }
    put_bytes(&mut out, 3, desc.as_bytes());
    out
}

/// The strings a request's `Attributes`/name/description carry, checked
/// against the client's limits; `None` is a `412`.
fn text(raw: Option<Vec<u8>>, max: usize) -> Option<String> {
    let s = String::from_utf8(raw.unwrap_or_default()).ok()?;
    (s.chars().count() <= max).then_some(s)
}

// ---- the milestone row ---------------------------------------------------

fn milestone_msg(id: &str, row: &[String]) -> Vec<u8> {
    let [plan, train, skill, level, desc] = row else {
        return Vec::new();
    };
    let num = |s: &String| s.parse::<u64>().unwrap_or(0);
    let mut data = Vec::new();
    put_bytes(&mut data, 1, &identifier(plan));
    if num(train) != 0 {
        let mut t = Vec::new();
        put_varint(&mut t, 1 << 3);
        put_varint(&mut t, num(train));
        put_bytes(&mut data, 2, &t);
    } else if num(skill) != 0 {
        put_bytes(&mut data, 3, &requirement_msg(num(skill), num(level)));
    }
    put_bytes(&mut data, 4, desc.as_bytes());
    let mut m = Vec::new();
    put_bytes(&mut m, 1, &identifier(id));
    put_bytes(&mut m, 2, &data);
    m
}

// ---- the handlers --------------------------------------------------------

/// Answer a skill-plan request. `None` when `ty` is not one of ours.
pub fn handle(ty: &str, body: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    if let Some(op) = ty.strip_prefix(CORP_PKG) {
        // Corporation plans: not served. Reads find nothing, writes refuse.
        let base = ty.strip_suffix("Request")?;
        return Some(match op {
            "GetAllRequest" => ok(base, Vec::new()),
            "GetRequest" | "GetSharedRequest" => reply(404, None),
            _ if op.contains("milestone.GetAll") => ok(base, Vec::new()),
            _ => reply(403, None),
        });
    }
    let op = ty.strip_prefix(PKG)?;
    let base = ty.strip_suffix("Request")?;
    if character <= 0 {
        // Requests sent before a pilot is active are buffered by the client,
        // so this is a stray; refuse rather than file under pilot 0.
        return Some(reply(403, None));
    }
    let who = character.to_string();
    let b = body;
    Some(match op {
        "GetAllRequest" => g.read(|d| {
            let mut out = Vec::new();
            for (k, _) in rows(d, &who, "p") {
                put_bytes(&mut out, 1, &identifier(&k[2]));
            }
            ok(base, out)
        }),
        "GetRequest" => {
            let Some(id) = ident(get_bytes(b, 1).as_deref()) else {
                return Some(reply(400, None));
            };
            g.read(|d| match d.of(KIND).get(&key(&who, "p", &id)) {
                Some(row) => {
                    let mut out = Vec::new();
                    put_bytes(&mut out, 1, &attributes(row));
                    ok(base, out)
                }
                None => reply(404, None),
            })
        }
        "GetSharedRequest" => {
            let Some(id) = ident(get_bytes(b, 1).as_deref()) else {
                return Some(reply(400, None));
            };
            g.read(|d| {
                let found = d
                    .of(KIND)
                    .iter()
                    .find(|(k, _)| k.len() == 3 && k[1] == "p" && k[2] == id);
                match found {
                    Some((_, row)) => {
                        let mut out = Vec::new();
                        put_bytes(&mut out, 1, &attributes(row));
                        ok(base, out)
                    }
                    None => reply(404, None),
                }
            })
        }
        "CreateRequest" => {
            let a = get_bytes(b, 1).unwrap_or_default();
            let name = text(get_bytes(&a, 1), MAX_NAME).filter(|n| !n.trim().is_empty());
            let desc = text(get_bytes(&a, 3), MAX_DESC);
            let reqs = requirements(&field(&a, 2));
            let (Some(name), Some(desc), Some(reqs)) = (name, desc, reqs) else {
                return Some(reply(412, None));
            };
            g.write(|d| {
                if rows(d, &who, "p").len() >= MAX_PLANS {
                    return reply(412, None);
                }
                let id = mint(d, character);
                d.put(KIND, key(&who, "p", &id), vec![name, desc, reqs]);
                let mut out = Vec::new();
                put_bytes(&mut out, 1, &identifier(&id));
                ok(base, out)
            })
        }
        "SetNameRequest" | "SetDescriptionRequest" | "SetSkillRequirementsRequest" => {
            let Some(id) = ident(get_bytes(b, 1).as_deref()) else {
                return Some(reply(400, None));
            };
            let slot = match op {
                "SetNameRequest" => 0,
                "SetDescriptionRequest" => 1,
                _ => 2,
            };
            let value = match slot {
                0 => text(get_bytes(b, 2), MAX_NAME).filter(|n| !n.trim().is_empty()),
                1 => text(get_bytes(b, 2), MAX_DESC),
                _ => requirements(&field(b, 2)),
            };
            let Some(value) = value else {
                return Some(reply(412, None));
            };
            g.write(|d| {
                let k = key(&who, "p", &id);
                let Some(mut row) = d.of(KIND).get(&k).cloned() else {
                    return reply(404, None);
                };
                if row.len() != 3 {
                    return reply(500, None);
                }
                row[slot] = value;
                d.put(KIND, k, row);
                ok(base, Vec::new())
            })
        }
        "DeleteRequest" => {
            let Some(id) = ident(get_bytes(b, 1).as_deref()) else {
                return Some(reply(400, None));
            };
            g.write(|d| {
                if d.del(KIND, &key(&who, "p", &id)).is_none() {
                    return reply(404, None);
                }
                let ak = vec![who.clone(), "a".to_owned()];
                if d.of(KIND).get(&ak).is_some_and(|r| r.first() == Some(&id)) {
                    d.del(KIND, &ak);
                }
                let gone: Vec<Vec<String>> = rows(d, &who, "m")
                    .into_iter()
                    .filter(|(_, v)| v.first() == Some(&id))
                    .map(|(k, _)| k.to_vec())
                    .collect();
                for k in gone {
                    d.del(KIND, &k);
                }
                ok(base, Vec::new())
            })
        }
        "GetActiveRequest" => g.read(|d| {
            let active = d
                .of(KIND)
                .get(&vec![who.clone(), "a".to_owned()])
                .and_then(|r| r.first().cloned());
            let mut out = Vec::new();
            match active.and_then(|id| Some((d.of(KIND).get(&key(&who, "p", &id))?, id))) {
                Some((row, id)) => {
                    put_bytes(&mut out, 1, &identifier(&id));
                    put_bytes(&mut out, 2, &attributes(row));
                }
                // No tracked plan is the zero uuid (module doc).
                None => put_bytes(&mut out, 1, &identifier(&hex(&[0; 16]))),
            }
            ok(base, out)
        }),
        "SetActiveRequest" => {
            let Some(id) = ident(get_bytes(b, 1).as_deref()) else {
                return Some(reply(400, None));
            };
            g.write(|d| {
                let ak = vec![who.clone(), "a".to_owned()];
                if id.bytes().all(|c| c == b'0') {
                    d.del(KIND, &ak);
                } else if d.of(KIND).contains_key(&key(&who, "p", &id)) {
                    d.put(KIND, ak, vec![id]);
                } else {
                    return reply(404, None);
                }
                ok(base, Vec::new())
            })
        }
        _ => return milestone(op, base, b, character, g),
    })
}

fn milestone(op: &str, base: &str, b: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    let op = op.strip_prefix("milestone.")?;
    let who = character.to_string();
    Some(match op {
        "GetAllRequest" => {
            let Some(plan) = ident(get_bytes(b, 1).as_deref()) else {
                return Some(reply(400, None));
            };
            g.read(|d| {
                if !d.of(KIND).contains_key(&key(&who, "p", &plan)) {
                    return reply(404, None);
                }
                let mut out = Vec::new();
                for (k, row) in rows(d, &who, "m") {
                    if row.first() == Some(&plan) {
                        put_bytes(&mut out, 1, &milestone_msg(&k[2], row));
                    }
                }
                ok(base, out)
            })
        }
        "CreateRequest" => {
            let m = get_bytes(b, 1).unwrap_or_default();
            let Some(plan) = ident(get_bytes(&m, 1).as_deref()) else {
                return Some(reply(400, None));
            };
            let train = get_bytes(&m, 2).and_then(|t| get_uint(&t, 1)).unwrap_or(0);
            let skill = get_bytes(&m, 3).and_then(|s| requirement(&s));
            let (st, sl) = skill.unwrap_or((0, 0));
            if (train == 0) == (st == 0) || sl > 5 {
                return Some(reply(412, None));
            }
            let Some(desc) = text(get_bytes(&m, 4), MAX_DESC) else {
                return Some(reply(412, None));
            };
            g.write(|d| {
                if !d.of(KIND).contains_key(&key(&who, "p", &plan)) {
                    return reply(404, None);
                }
                let mine = rows(d, &who, "m")
                    .iter()
                    .filter(|(_, v)| v.first() == Some(&plan))
                    .count();
                if mine >= MAX_MILESTONES {
                    return reply(412, None);
                }
                let id = mint(d, character);
                d.put(
                    KIND,
                    key(&who, "m", &id),
                    vec![
                        plan,
                        train.to_string(),
                        st.to_string(),
                        sl.to_string(),
                        desc,
                    ],
                );
                let mut out = Vec::new();
                put_bytes(&mut out, 1, &identifier(&id));
                ok(base, out)
            })
        }
        "DeleteRequest" => {
            let Some(id) = ident(get_bytes(b, 1).as_deref()) else {
                return Some(reply(400, None));
            };
            g.write(|d| match d.del(KIND, &key(&who, "m", &id)) {
                Some(_) => ok(base, Vec::new()),
                None => reply(404, None),
            })
        }
        "SetDescriptionRequest" => {
            let Some(id) = ident(get_bytes(b, 1).as_deref()) else {
                return Some(reply(400, None));
            };
            let Some(desc) = text(get_bytes(b, 2), MAX_DESC) else {
                return Some(reply(412, None));
            };
            g.write(|d| {
                let k = key(&who, "m", &id);
                let Some(mut row) = d.of(KIND).get(&k).cloned() else {
                    return reply(404, None);
                };
                if row.len() != 5 {
                    return reply(500, None);
                }
                row[4] = desc;
                d.put(KIND, k, row);
                ok(base, Vec::new())
            })
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::{get_bytes, get_uint, put_bytes, put_varint};
    use std::sync::{Arc, RwLock};

    const ALICE: i64 = 9;
    const BOB: i64 = 4;

    fn ground() -> Ground {
        Ground::Server(Arc::new(RwLock::new(crate::testkit::registry())))
    }

    fn call(g: &Ground, who: i64, op: &str, body: &[u8]) -> (u32, Vec<u8>) {
        let ty = format!("{PKG}{op}Request");
        let r = handle(&ty, body, who, g).expect("a skill-plan class");
        (r.status, r.payload.map(|p| p.1).unwrap_or_default())
    }

    fn id_msg(h: &str) -> Vec<u8> {
        let mut m = Vec::new();
        put_bytes(&mut m, 1, &identifier(h));
        m
    }

    fn plan_attrs(name: &str, desc: &str, reqs: &[(u64, u64)]) -> Vec<u8> {
        let mut a = Vec::new();
        put_bytes(&mut a, 1, name.as_bytes());
        for &(t, l) in reqs {
            put_bytes(&mut a, 2, &requirement_msg(t, l));
        }
        put_bytes(&mut a, 3, desc.as_bytes());
        a
    }

    fn create(g: &Ground, who: i64, name: &str, reqs: &[(u64, u64)]) -> (u32, String) {
        let mut b = Vec::new();
        put_bytes(&mut b, 1, &plan_attrs(name, "d", reqs));
        let (s, v) = call(g, who, "Create", &b);
        (s, ident(get_bytes(&v, 1).as_deref()).unwrap_or_default())
    }

    fn all(g: &Ground, who: i64) -> Vec<String> {
        let (_, v) = call(g, who, "GetAll", &[]);
        crate::gateway::field(&v, 1)
            .iter()
            .map(|i| hex(&get_bytes(i, 1).unwrap()))
            .collect()
    }

    #[test]
    fn a_plan_is_created_listed_read_edited_and_deleted() {
        let g = ground();
        assert!(all(&g, ALICE).is_empty());
        let (s, plan) = create(&g, ALICE, "Frigate basics", &[(101, 3), (102, 5)]);
        assert_eq!((s, plan.len()), (200, 32));
        assert_eq!(all(&g, ALICE), vec![plan.clone()]);
        // Get returns what was saved, requirements in order.
        let (s, v) = call(&g, ALICE, "Get", &id_msg(&plan));
        assert_eq!(s, 200);
        let a = get_bytes(&v, 1).unwrap();
        assert_eq!(get_bytes(&a, 1).unwrap(), b"Frigate basics");
        assert_eq!(get_bytes(&a, 3).unwrap(), b"d");
        let reqs: Vec<_> = crate::gateway::field(&a, 2)
            .iter()
            .map(|r| requirement(r).unwrap())
            .collect();
        assert_eq!(reqs, vec![(101, 3), (102, 5)]);
        // Edit the name, the description and the requirements.
        let edit = |text: &[u8]| {
            let mut b = id_msg(&plan);
            put_bytes(&mut b, 2, text);
            b
        };
        assert_eq!(call(&g, ALICE, "SetName", &edit(b"Renamed")).0, 200);
        assert_eq!(call(&g, ALICE, "SetDescription", &edit(b"new text")).0, 200);
        let mut b = id_msg(&plan);
        put_bytes(&mut b, 2, &requirement_msg(103, 1));
        assert_eq!(call(&g, ALICE, "SetSkillRequirements", &b).0, 200);
        let (_, v) = call(&g, ALICE, "Get", &id_msg(&plan));
        let a = get_bytes(&v, 1).unwrap();
        assert_eq!(get_bytes(&a, 1).unwrap(), b"Renamed");
        assert_eq!(get_bytes(&a, 3).unwrap(), b"new text");
        assert_eq!(crate::gateway::field(&a, 2).len(), 1);
        // Delete.
        assert_eq!(call(&g, ALICE, "Delete", &id_msg(&plan)).0, 200);
        assert!(all(&g, ALICE).is_empty());
        assert_eq!(call(&g, ALICE, "Get", &id_msg(&plan)).0, 404);
        assert_eq!(call(&g, ALICE, "Delete", &id_msg(&plan)).0, 404);
    }

    #[test]
    fn plans_belong_to_their_pilot_but_a_link_reads_anyones() {
        let g = ground();
        let (_, plan) = create(&g, ALICE, "Mine", &[(101, 1)]);
        assert!(all(&g, BOB).is_empty());
        assert_eq!(call(&g, BOB, "Get", &id_msg(&plan)).0, 404);
        assert_eq!(call(&g, BOB, "Delete", &id_msg(&plan)).0, 404);
        assert_eq!(call(&g, BOB, "GetShared", &id_msg(&plan)).0, 200);
        assert_eq!(all(&g, ALICE).len(), 1);
        // Ids do not collide across pilots.
        let (_, other) = create(&g, BOB, "Theirs", &[]);
        assert_ne!(plan, other);
        // No pilot, no plans.
        assert_eq!(call(&g, 0, "GetAll", &[]).0, 403);
    }

    #[test]
    fn no_tracked_plan_is_the_zero_uuid_and_tracking_follows_the_plan() {
        let g = ground();
        let none = |v: &[u8]| ident(get_bytes(v, 1).as_deref()).unwrap();
        let (s, v) = call(&g, ALICE, "GetActive", &[]);
        assert_eq!((s, none(&v)), (200, "0".repeat(32)));
        assert_eq!(get_bytes(&v, 2), None);
        let (_, plan) = create(&g, ALICE, "Tracked", &[(101, 2)]);
        assert_eq!(call(&g, ALICE, "SetActive", &id_msg(&plan)).0, 200);
        let (_, v) = call(&g, ALICE, "GetActive", &[]);
        assert_eq!(none(&v), plan);
        assert_eq!(
            get_bytes(&get_bytes(&v, 2).unwrap(), 1).unwrap(),
            b"Tracked"
        );
        // Untrack with the zero uuid; a plan that is not there is a 404.
        assert_eq!(
            call(&g, ALICE, "SetActive", &id_msg(&"0".repeat(32))).0,
            200
        );
        assert_eq!(none(&call(&g, ALICE, "GetActive", &[]).1), "0".repeat(32));
        assert_eq!(
            call(&g, ALICE, "SetActive", &id_msg(&"1".repeat(32))).0,
            404
        );
        // Deleting the tracked plan untracks it.
        call(&g, ALICE, "SetActive", &id_msg(&plan));
        call(&g, ALICE, "Delete", &id_msg(&plan));
        assert_eq!(none(&call(&g, ALICE, "GetActive", &[]).1), "0".repeat(32));
    }

    #[test]
    fn bad_plans_are_refused_with_412() {
        let g = ground();
        assert_eq!(create(&g, ALICE, "  ", &[]).0, 412);
        assert_eq!(create(&g, ALICE, &"n".repeat(MAX_NAME + 1), &[]).0, 412);
        assert_eq!(create(&g, ALICE, "x", &[(0, 1)]).0, 412);
        assert_eq!(create(&g, ALICE, "x", &[(101, 6)]).0, 412);
        let many: Vec<(u64, u64)> = (1..=MAX_SKILLS as u64 + 1).map(|t| (t, 1)).collect();
        assert_eq!(create(&g, ALICE, "x", &many).0, 412);
        for i in 0..MAX_PLANS {
            assert_eq!(create(&g, ALICE, &format!("p{i}"), &[]).0, 200);
        }
        assert_eq!(create(&g, ALICE, "one too many", &[]).0, 412);
        // Another pilot has their own ten.
        assert_eq!(create(&g, BOB, "fine", &[]).0, 200);
        assert!(all(&g, ALICE).len() == MAX_PLANS);
    }

    #[test]
    fn milestones_are_created_listed_limited_and_deleted_with_their_plan() {
        let g = ground();
        let (_, plan) = create(&g, ALICE, "With goals", &[(101, 3)]);
        let milestone = |attrs: Vec<u8>| {
            let mut m = Vec::new();
            put_bytes(&mut m, 1, &identifier(&plan));
            m.extend(attrs);
            let mut b = Vec::new();
            put_bytes(&mut b, 1, &m);
            b
        };
        let mut train = Vec::new();
        let mut t = Vec::new();
        put_varint(&mut t, 1 << 3);
        put_varint(&mut t, 555);
        put_bytes(&mut train, 2, &t);
        let mut skill = Vec::new();
        put_bytes(&mut skill, 3, &requirement_msg(101, 4));
        let (s, v) = call(&g, ALICE, "milestone.Create", &milestone(train.clone()));
        assert_eq!(s, 200);
        let first = ident(get_bytes(&v, 1).as_deref()).unwrap();
        assert_eq!(
            call(&g, ALICE, "milestone.Create", &milestone(skill)).0,
            200
        );
        // Neither, or a plan that is not there, is refused.
        assert_eq!(
            call(&g, ALICE, "milestone.Create", &milestone(vec![])).0,
            412
        );
        assert_eq!(
            call(&g, BOB, "milestone.Create", &milestone(train.clone())).0,
            404
        );
        let (s, v) = call(&g, ALICE, "milestone.GetAll", &id_msg(&plan));
        assert_eq!(s, 200);
        let list = crate::gateway::field(&v, 1);
        assert_eq!(list.len(), 2);
        let data = |m: &Vec<u8>| get_bytes(m, 2).unwrap();
        let trained: Vec<_> = list
            .iter()
            .filter_map(|m| get_bytes(&data(m), 2).and_then(|t| get_uint(&t, 1)))
            .collect();
        assert_eq!(trained, vec![555]);
        let skills: Vec<_> = list
            .iter()
            .filter_map(|m| get_bytes(&data(m), 3).and_then(|s| requirement(&s)))
            .collect();
        assert_eq!(skills, vec![(101, 4)]);
        // A description sticks.
        let mut b = id_msg(&first);
        put_bytes(&mut b, 2, b"why");
        assert_eq!(call(&g, ALICE, "milestone.SetDescription", &b).0, 200);
        let (_, v) = call(&g, ALICE, "milestone.GetAll", &id_msg(&plan));
        assert!(
            crate::gateway::field(&v, 1)
                .iter()
                .any(|m| get_bytes(&data(m), 4).as_deref() == Some(b"why".as_slice()))
        );
        // The limit is five.
        for _ in 0..MAX_MILESTONES - 2 {
            assert_eq!(
                call(&g, ALICE, "milestone.Create", &milestone(train.clone())).0,
                200
            );
        }
        assert_eq!(
            call(&g, ALICE, "milestone.Create", &milestone(train.clone())).0,
            412
        );
        // Delete one, then the plan takes the rest.
        assert_eq!(call(&g, ALICE, "milestone.Delete", &id_msg(&first)).0, 200);
        assert_eq!(call(&g, ALICE, "milestone.Delete", &id_msg(&first)).0, 404);
        call(&g, ALICE, "Delete", &id_msg(&plan));
        let left = g.read(|d| rows(d, &ALICE.to_string(), "m").len());
        assert_eq!(left, 0);
    }

    #[test]
    fn corporation_plans_read_empty_and_refuse_writes() {
        let g = ground();
        let get = |op: &str| {
            let ty = format!("{CORP_PKG}{op}Request");
            handle(&ty, &[], ALICE, &g).unwrap().status
        };
        assert_eq!(get("GetAll"), 200);
        assert_eq!(get("Get"), 404);
        assert_eq!(get("GetShared"), 404);
        assert_eq!(get("Create"), 403);
        assert_eq!(get("Delete"), 403);
        assert_eq!(get("milestone.GetAll"), 200);
        // Not ours at all.
        assert!(handle("eve_public.invented.GetRequest", &[], ALICE, &g).is_none());
    }

    #[test]
    fn plans_survive_a_save_and_load_of_the_deltas() {
        let g = ground();
        let (_, plan) = create(&g, ALICE, "kept\tname", &[(101, 5)]);
        call(&g, ALICE, "SetActive", &id_msg(&plan));
        let rows = g.read(|d| d.of(KIND).clone());
        assert_eq!(rows.len(), 2);
        let fresh = ground();
        fresh.write(|d| {
            for (k, v) in rows {
                d.put(KIND, k, v);
            }
        });
        assert_eq!(all(&fresh, ALICE), vec![plan.clone()]);
        let (_, v) = call(&fresh, ALICE, "GetActive", &[]);
        assert_eq!(ident(get_bytes(&v, 1).as_deref()).unwrap(), plan);
        // A new id after a reload does not reuse the old one.
        let (_, next) = create(&fresh, ALICE, "next", &[]);
        assert_ne!(next, plan);
    }
}
