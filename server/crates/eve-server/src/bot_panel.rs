//! **BOT-PANEL**: the in-game bot panel of a `eve-server --dev` world.
//!
//! The client cannot be changed, so the panel is a window it already has: the
//! **agent dialogue**. In a dev world one agent per station is the "Bot
//! Control" agent. Talking to it (`agentMgr.DoAction`) answers with HTML as
//! `agentSaysMsg` and one admin link per action; a click on a link is
//! `DoAction(actionID)` again, which is a button press.
//!
//! **What the client does with the answer [V]** (`agentDialogueWindow.py`,
//! decompiled build 3619879): `agentSaysMsg` is `(msgInfo, contentID)`;
//! `agents.ProcessMessage:614-618` returns a raw-string `msgInfo` verbatim, so
//! the HTML we send is the body (`_GetBriefingHTML`, which wraps it in
//! `<html><body>`). Each `(actionID, actionData)` with a **string**
//! `actionData` is drawn as `<a href="localsvc:method=AgentDoAction&agentID=..
//! &actionID=..">text</a>` (`_GetAdminOptions:353-359`, `GetAdminHTML`), a
//! numbered list when there are several; the click reaches `DoAction`
//! (`endpoints.py:116`). A window button, by contrast, can only carry one of
//! the client's own labels (`LABEL_BY_BUTTON_TYPE`), so **links are the custom
//! buttons**. The first-action auto-call fires only for integer
//! `RequestMission`/`ViewMission` actions, which we never send.
//!
//! **Which agent.** A real NPC agent, so the client already knows its name
//! and portrait (an invented id cannot be named without client data): per
//! station, the lowest-id level-1 basic agent in the mission pool, else the
//! lowest id there; with no pool, the no-bundle station agent. Dev worlds
//! only, and that agent's missions are not served there.
//!
//! **Spawning is not here.** A press turns into `/bots ...` command lines
//! ([`lines_for`], from `content/bots/dev/presets.tsv`) that go through the
//! slash dispatcher as the caller ([`issue`]) -- the one function the
//! coordinator joins to lane BOT-CTL (`/bots` has no row in `slash::COMMANDS`
//! until that lane lands, and the panel then shows "not implemented yet").

use crate::pyutil::{bstr, pydict};
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;

/// The shipped preset table.
const SHIPPED: &str = include_str!("../../../../content/bots/dev/presets.tsv");

/// The corporation ticker (and so the side word) of a hostile side.
pub const FOE_SIDE: &str = "FOES";
/// The side word the allied side takes when the caller has no dev corporation.
pub const ALLY_SIDE: &str = "ALLY";

/// Action ids; the client only echoes them.
pub const ACT_REFRESH: i64 = 200;
pub const ACT_BRING: i64 = 201;
pub const ACT_DESPAWN: i64 = 202;
/// Preset `i` (in table order) is `ACT_PRESET + i`.
pub const ACT_PRESET: i64 = 100;

/// Most `/bots` lines kept for the panel and the tests.
const KEEP_ISSUED: usize = 200;
/// The farthest a roaming gang is placed.
const NEAR_MAX: u32 = 3;

/// Where a row's bots are seated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Where {
    Here,
    Near(u32),
}

/// One `presets.tsv` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub preset: String,
    pub label: String,
    pub prefix: String,
    pub ally: bool,
    pub role: String,
    pub count: u32,
    pub skills: String,
    pub ship: String,
    pub at: Where,
}

/// One button: a key, its label, its rows.
#[derive(Debug, Clone)]
pub struct Preset {
    pub key: String,
    pub label: String,
    pub rows: Vec<Row>,
}

fn plain(w: &str) -> bool {
    !w.is_empty()
        && w.len() <= 40
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Parse a preset table. `#` comments and blank lines are skipped.
pub fn parse(text: &str) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let n = n + 1;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 9 {
            return Err(format!(
                "presets line {n}: want 9 tab-separated columns, got {}",
                f.len()
            ));
        }
        for w in [f[0], f[2], f[4], f[6], f[7]] {
            if !plain(w) {
                return Err(format!("presets line {n}: {w:?} is not a plain word"));
            }
        }
        if !f[2].chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(format!("presets line {n}: prefix is letters and digits"));
        }
        let ally = match f[3] {
            "ally" => true,
            "foe" => false,
            other => return Err(format!("presets line {n}: side {other:?} is ally or foe")),
        };
        let count: u32 = f[5]
            .parse()
            .ok()
            .filter(|c| (1..=200).contains(c))
            .ok_or_else(|| format!("presets line {n}: count {:?} is 1..=200", f[5]))?;
        let at = match f[8] {
            "here" => Where::Here,
            w => w
                .strip_prefix("near")
                .and_then(|k| k.parse::<u32>().ok())
                .filter(|k| (1..=NEAR_MAX).contains(k))
                .map(Where::Near)
                .ok_or_else(|| format!("presets line {n}: where {w:?} is here or near1..near3"))?,
        };
        rows.push(Row {
            preset: f[0].to_owned(),
            label: f[1].to_owned(),
            prefix: f[2].to_owned(),
            ally,
            role: f[4].to_owned(),
            count,
            skills: f[6].to_owned(),
            ship: f[7].to_owned(),
            at,
        });
    }
    Ok(rows)
}

/// The rows grouped into buttons, in file order.
pub fn presets_of(rows: Vec<Row>) -> Vec<Preset> {
    let mut out: Vec<Preset> = Vec::new();
    for r in rows {
        match out.last_mut().filter(|p| p.key == r.preset) {
            Some(p) => p.rows.push(r),
            None => out.push(Preset {
                key: r.preset.clone(),
                label: r.label.clone(),
                rows: vec![r],
            }),
        }
    }
    out
}

/// The shipped buttons.
pub fn shipped() -> Vec<Preset> {
    presets_of(parse(SHIPPED).expect("content/bots/dev/presets.tsv parses"))
}

/// The `/bots spawn` lines of one preset press. `serial` makes the group
/// names unique; `system(at)` resolves a row's place to a system id.
pub fn lines_for(
    p: &Preset,
    serial: u64,
    ally_side: &str,
    system: &dyn Fn(Where) -> i64,
) -> Vec<String> {
    p.rows
        .iter()
        .map(|r| {
            format!(
                "/bots spawn {}{serial} {} {} {} {} {} {}",
                r.prefix,
                if r.ally { ally_side } else { FOE_SIDE },
                r.role,
                r.count,
                r.skills,
                r.ship,
                system(r.at)
            )
        })
        .collect()
}

/// A group this panel started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawned {
    pub owner: i64,
    pub group: String,
    pub ally: bool,
}

/// The panel's world state (never persisted: the bots live in `eve-bots`).
#[derive(Default)]
pub struct Panel {
    serial: u64,
    groups: Vec<Spawned>,
    issued: Vec<String>,
}

impl Panel {
    /// Every `/bots` line issued, oldest first (capped).
    pub fn issued(&self) -> &[String] {
        &self.issued
    }
    pub fn groups(&self) -> &[Spawned] {
        &self.groups
    }
}

/// The slot, for `world_ext::SLOTS` (coordinator).
pub const SLOT: SlotSpec = SlotSpec {
    name: "bot_panel",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(Panel::default())
}

/// The agent that is the panel in `station`, given the pool's agents there
/// as `(id, level, agent_type)`.
pub fn pick_agent(there: &[(i64, i64, i64)]) -> Option<i64> {
    let basic = |a: &&(i64, i64, i64)| a.1 == 1 && a.2 == crate::mission_pool::TYPE_BASIC;
    there
        .iter()
        .filter(basic)
        .map(|a| a.0)
        .min()
        .or_else(|| there.iter().map(|a| a.0).min())
}

/// Whether `agent` is the Bot Control agent of its station in this world.
pub fn is_panel_agent(reg: &Registry, agent: i64) -> bool {
    if !reg.config().dev {
        return false;
    }
    let Some(pool) = reg.content().try_get::<crate::mission_pool::Table>() else {
        return agent == crate::mission::AGENT_ID;
    };
    let Some(station) = pool.agent(agent).and_then(|a| a.station) else {
        // The no-bundle station agent is not in a pool that lacks it.
        return agent == crate::mission::AGENT_ID && pool.agent(agent).is_none();
    };
    let there: Vec<_> = pool
        .agents()
        .filter(|a| a.station == Some(station))
        .map(|a| (a.id, a.level, a.agent_type))
        .collect();
    pick_agent(&there) == Some(agent)
}

fn neighbours(reg: &Registry, system: i64) -> Vec<i64> {
    let cat = reg.celestials();
    let mut out: Vec<i64> = cat
        .served_in(system)
        .filter(|c| c.kind == crate::celestial::Kind::Gate)
        .filter(|c| cat.system_of(c.id) == Some(system))
        .filter_map(|c| cat.jump_of(c.id))
        .map(|(_, to)| to)
        .filter(|to| *to != system)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// A system 1..=`jumps` gates from `origin` (stable in `serial`), else `origin`.
pub(crate) fn nearby(reg: &Registry, origin: i64, jumps: u32, serial: u64) -> i64 {
    nearby_where(reg, origin, jumps, serial, &|_| true)
}

/// [`nearby`] among the systems `ok` accepts.
pub(crate) fn nearby_where(
    reg: &Registry,
    origin: i64,
    jumps: u32,
    serial: u64,
    ok: &dyn Fn(i64) -> bool,
) -> i64 {
    let near = crate::escalation::within(origin, jumps, &|s| neighbours(reg, s));
    let rest: Vec<i64> = near[1..].iter().copied().filter(|s| ok(*s)).collect();
    match &rest[..] {
        [] => origin,
        rest => rest[(serial as usize).wrapping_mul(7) % rest.len()],
    }
}

/// Whether a bot can be seated in `system`: it has a station, or the world
/// has no stations at all (a bare test world, where nothing seats anyone).
pub(crate) fn has_station(reg: &Registry, system: i64) -> bool {
    let is_station = |c: &&crate::celestial::Celestial| c.kind == crate::celestial::Kind::Station;
    let cat = &reg.config().celestials;
    cat.served_in(system)
        .any(|c| c.kind == crate::celestial::Kind::Station)
        || !cat.served().any(|c| is_station(&c))
}

fn say(v: PyValue) -> String {
    match v {
        PyValue::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
        other => format!("{other:?}"),
    }
}

/// **The one seam to BOT-CTL.** Run one `/bots ...` line as the caller through
/// the slash dispatcher, and keep the line. Until `/bots` has its
/// `slash::COMMANDS` row the answer is "not implemented yet: ...".
pub fn issue(reg: &mut Registry, actor: Option<i64>, line: &str) -> String {
    let p = reg.ext_mut::<Panel>();
    p.issued.push(line.to_owned());
    if p.issued.len() > KEEP_ISSUED {
        p.issued.remove(0);
    }
    match reg.slash(actor, line) {
        Ok(v) => say(v),
        Err(e) => format!("{e:?}"),
    }
}

/// The caller's side word: their own dev corporation's ticker when they are in
/// one, else they join `ALLY` (dev only; leaves any other corporation).
pub(crate) fn seat_ally(reg: &mut Registry, actor: i64) -> String {
    let corp = reg.character(actor).map(|c| c.corporation_id);
    let own = corp.and_then(|id| {
        reg.corporations()
            .all()
            .find(|c| c.corporation_id == id)
            .map(|c| c.ticker.clone())
    });
    match own {
        Some(t) if t.len() <= 5 && t.chars().all(|c| c.is_ascii_alphanumeric()) => t,
        _ => {
            crate::service::corp::dev_join(reg, actor, ALLY_SIDE);
            ALLY_SIDE.to_owned()
        }
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', "<br>")
}

/// A press. Returns the lines of what happened, for the panel's notice.
fn press(reg: &mut Registry, actor: i64, action: i64) -> Vec<String> {
    let table = shipped();
    if let Some(p) = usize::try_from(action - ACT_PRESET)
        .ok()
        .and_then(|i| table.get(i))
    {
        let ally_side = if p.rows.iter().any(|r| r.ally) {
            seat_ally(reg, actor)
        } else {
            ALLY_SIDE.to_owned()
        };
        let serial = {
            let st = reg.ext_mut::<Panel>();
            if st.serial == 0 {
                st.serial = crate::pyutil::filetime_now().unsigned_abs() / 10_000_000 % 100_000 + 1;
            }
            st.serial += 1;
            st.serial
        };
        let here = reg.session_system(actor).unwrap_or(0);
        let lines = lines_for(p, serial, &ally_side, &|at| match at {
            Where::Here => here,
            Where::Near(k) => nearby(reg, here, k, serial),
        });
        let mut out = vec![format!("{}:", p.label)];
        for (line, row) in lines.iter().zip(&p.rows) {
            let reply = issue(reg, Some(actor), line);
            let group = line.split(' ').nth(2).unwrap_or("").to_owned();
            reg.ext_mut::<Panel>().groups.push(Spawned {
                owner: actor,
                group,
                ally: row.ally,
            });
            out.push(format!("{} x{}: {}", row.role, row.count, reply.trim()));
        }
        return out;
    }
    match action {
        ACT_BRING => {
            let groups: Vec<String> = reg
                .ext::<Panel>()
                .groups
                .iter()
                .filter(|g| g.owner == actor && g.ally)
                .map(|g| g.group.clone())
                .collect();
            if groups.is_empty() {
                return vec!["You have no allied groups to bring.".to_owned()];
            }
            groups
                .iter()
                .map(|g| {
                    format!(
                        "{g}: {}",
                        issue(reg, Some(actor), &format!("/bots bring {g}")).trim()
                    )
                })
                .collect()
        }
        ACT_DESPAWN => {
            let reply = issue(reg, Some(actor), "/bots despawn all");
            reg.ext_mut::<Panel>().groups.clear();
            vec![format!("Despawn all: {}", reply.trim())]
        }
        _ => Vec::new(),
    }
}

/// The panel's HTML (the client wraps it in `<html><body>`).
pub fn body(notice: &[String], list: &str, ally_side: &str) -> String {
    let mut h = String::from("<b>Bot Control</b> (dev server)<br>");
    h.push_str(&format!(
        "Allied bots join your side ({}); hostile bots are side {FOE_SIDE}.<br>\
         Roaming gangs start 1-3 jumps away and fan out to find you.<br>",
        html_escape(ally_side)
    ));
    if !notice.is_empty() {
        h.push_str("<br><b>Last press</b><br>");
        for l in notice {
            h.push_str(&html_escape(l));
            h.push_str("<br>");
        }
    }
    h.push_str("<br><b>Bots</b><br>");
    let list = list.trim();
    h.push_str(&if list.is_empty() {
        "none".to_owned()
    } else {
        html_escape(list)
    });
    h.push_str("<br>");
    h
}

/// The actions the window draws as links, `(actionID, text)`.
pub fn actions() -> Vec<(i64, String)> {
    let mut out: Vec<(i64, String)> = shipped()
        .iter()
        .enumerate()
        .map(|(i, p)| (ACT_PRESET + i as i64, p.label.clone()))
        .collect();
    out.push((ACT_BRING, "Bring my allies to my station".to_owned()));
    out.push((ACT_DESPAWN, "Despawn all bots".to_owned()));
    out.push((ACT_REFRESH, "Refresh the bot list".to_owned()));
    out
}

/// `agentMgr.DoAction` for the panel agent: `None` opens it, a number is a
/// press. The shape is the one `mission::reply` documents: `((agentSaysMsg,
/// availableActions), lastActionInfo)`, `agentSaysMsg = (html, None)`.
pub fn answer(
    reg: &mut Registry,
    actor: Option<i64>,
    agent: i64,
    action: Option<i64>,
) -> Option<PyValue> {
    if !is_panel_agent(reg, agent) {
        return None;
    }
    let notice = match (actor.filter(|a| reg.character(*a).is_some()), action) {
        (Some(a), Some(id)) => press(reg, a, id),
        _ => Vec::new(),
    };
    let list = if actor.is_some() {
        issue(reg, actor, "/bots list")
    } else {
        String::new()
    };
    let side = actor
        .and_then(|a| reg.character(a))
        .and_then(|c| {
            let id = c.corporation_id;
            reg.corporations()
                .all()
                .find(|k| k.corporation_id == id)
                .map(|k| k.ticker.clone())
        })
        .unwrap_or_else(|| ALLY_SIDE.to_owned());
    let html = body(&notice, &list, &side);
    let links = actions()
        .into_iter()
        .map(|(id, text)| PyValue::Tuple(vec![PyValue::Int(id), bstr(&text)]))
        .collect();
    Some(PyValue::Tuple(vec![
        PyValue::Tuple(vec![
            PyValue::Tuple(vec![bstr(&html), PyValue::None]),
            PyValue::List(links),
        ]),
        // All five keys are subscripted by the window.
        pydict(vec![
            ("missionCompleted", PyValue::Int(0)),
            ("missionDeclined", PyValue::Int(0)),
            ("missionQuit", PyValue::Int(0)),
            ("missionCantReplay", PyValue::Int(0)),
            ("loyaltyPoints", PyValue::Int(0)),
        ]),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission_pool as pool;
    use crate::model::SandboxConfig;

    const BASIC_LOW: i64 = 3_999_501;
    const BASIC_HIGH: i64 = 3_999_502;
    const LEVEL_3: i64 = 3_999_100;
    const OTHER: i64 = 3_999_700;

    fn agent(id: i64, level: i64, station: i64, agent_type: i64) -> pool::Agent {
        pool::Agent {
            id,
            agent_type,
            division: 24,
            level,
            corp: 1_999_001,
            faction: 599_001,
            station: Some(station),
            system: Some(39_000_001),
            ..pool::Agent::default()
        }
    }

    /// A dev (or normal) world whose home station holds invented agents.
    fn world(dev: bool) -> (Registry, i64) {
        let base = Registry::new(SandboxConfig::default());
        let me = base.config().characters[0].character_id;
        let station = base.character(me).unwrap().station_id.unwrap();
        let table = pool::Table::new(
            vec![
                agent(LEVEL_3, 3, station, 8),
                agent(BASIC_HIGH, 1, station, pool::TYPE_BASIC),
                agent(BASIC_LOW, 1, station, pool::TYPE_BASIC),
                agent(OTHER, 1, 69_000_777, pool::TYPE_BASIC),
            ],
            vec![],
            pool::Policy {
                parents: &[],
                drones: 599_021,
            },
        );
        let mut content = crate::content::Content::fixture();
        content.insert("mission_pool", table);
        let config = SandboxConfig {
            dev,
            ..SandboxConfig::default()
        };
        let reg = Registry::new(config).with_content(std::sync::Arc::new(content));
        (reg, me)
    }

    fn html_of(v: &PyValue) -> String {
        let PyValue::Tuple(top) = v else {
            panic!("{v:?}")
        };
        let PyValue::Tuple(inner) = &top[0] else {
            panic!("{v:?}")
        };
        let PyValue::Tuple(says) = &inner[0] else {
            panic!("{v:?}")
        };
        say(says[0].clone())
    }

    fn links_of(v: &PyValue) -> Vec<(i64, String)> {
        let PyValue::Tuple(top) = v else {
            panic!("{v:?}")
        };
        let PyValue::Tuple(inner) = &top[0] else {
            panic!("{v:?}")
        };
        let PyValue::List(l) = &inner[1] else {
            panic!("{v:?}")
        };
        l.iter()
            .map(|a| {
                let PyValue::Tuple(t) = a else {
                    panic!("{a:?}")
                };
                let PyValue::Int(id) = t[0] else {
                    panic!("{a:?}")
                };
                (id, say(t[1].clone()))
            })
            .collect()
    }

    #[test]
    fn the_shipped_table_parses_and_every_button_is_a_known_kind() {
        let table = shipped();
        let keys: Vec<&str> = table.iter().map(|p| p.key.as_str()).collect();
        for want in [
            "ally-miners",
            "ally-escorts",
            "ally-pve",
            "ally-industry",
            "foe-gang-small",
            "foe-gang-medium",
            "foe-gang-large",
            "foe-campers",
            "foe-dictor",
        ] {
            assert!(keys.contains(&want), "{want} in {keys:?}");
        }
        let miners = table.iter().find(|p| p.key == "ally-miners").unwrap();
        assert_eq!(miners.rows.iter().map(|r| r.count).sum::<u32>(), 11);
        assert!(miners.rows.iter().any(|r| r.ship == "Orca"));
        for p in table.iter().filter(|p| p.key.starts_with("foe-gang")) {
            assert!(
                p.rows.iter().all(|r| r.role == "roamgang" && !r.ally),
                "{}",
                p.key
            );
        }
    }

    #[test]
    fn a_bad_table_is_refused() {
        assert!(parse("a\tb\n").is_err());
        let ok = "k\tL\tp1\tally\tmine\t3\trole\tHulk\there";
        assert_eq!(parse(ok).unwrap().len(), 1);
        for bad in [
            ok.replace("ally", "friend"),
            ok.replace("\t3\t", "\t0\t"),
            ok.replace("here", "near9"),
            ok.replace("p1", "p 1"),
        ] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_preset_becomes_bots_spawn_lines() {
        let table = shipped();
        let p = table.iter().find(|p| p.key == "ally-miners").unwrap();
        let lines = lines_for(p, 7, "RED", &|_| 30_000_042);
        assert_eq!(
            lines,
            [
                "/bots spawn ao7 RED foreman 1 role Orca 30000042",
                "/bots spawn am7 RED mine 10 role Hulk 30000042",
            ]
        );
        let g = table.iter().find(|p| p.key == "foe-gang-small").unwrap();
        let far = lines_for(g, 8, "RED", &|at| match at {
            Where::Near(3) => 30_000_099,
            _ => 30_000_042,
        });
        assert_eq!(
            far,
            ["/bots spawn hs8 FOES roamgang 4 role Rifter 30000099"]
        );
    }

    #[test]
    fn the_panel_agent_is_one_per_station_and_only_on_a_dev_server() {
        let (dev, me) = world(true);
        let _ = me;
        assert!(
            is_panel_agent(&dev, BASIC_LOW),
            "lowest level-1 basic agent"
        );
        for other in [BASIC_HIGH, LEVEL_3] {
            assert!(!is_panel_agent(&dev, other), "{other}");
        }
        assert!(is_panel_agent(&dev, OTHER), "its own station's agent");
        let (normal, _) = world(false);
        assert!(!is_panel_agent(&normal, BASIC_LOW));
        assert_eq!(pick_agent(&[(9, 3, 8), (5, 2, 8)]), Some(5));
        assert_eq!(pick_agent(&[]), None);
    }

    #[test]
    fn opening_the_agent_returns_the_html_and_every_button() {
        let (mut reg, me) = world(true);
        let v = answer(&mut reg, Some(me), BASIC_LOW, None).expect("the panel agent answers");
        let html = html_of(&v);
        assert!(html.contains("Bot Control"), "{html}");
        assert!(html.contains("Bots"), "{html}");
        let links = links_of(&v);
        let texts: Vec<&str> = links.iter().map(|l| l.1.as_str()).collect();
        for want in [
            "allied miners",
            "allied escorts",
            "allied PvE",
            "industry and trade",
            "small hostile roaming gang",
            "medium hostile roaming gang",
            "large hostile roaming gang",
            "campers on the gate",
            "Bring my allies",
            "Despawn all",
            "Refresh",
        ] {
            assert!(
                texts.iter().any(|t| t.contains(want)),
                "{want} in {texts:?}"
            );
        }
        let mut ids: Vec<i64> = links.iter().map(|l| l.0).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), links.len(), "action ids are unique");
        // The client's unpack: lastActionInfo carries all five keys.
        let PyValue::Tuple(top) = &v else { panic!() };
        let PyValue::Dict(info) = &top[1] else {
            panic!()
        };
        assert_eq!(info.len(), 5);
        // Opening reads the list through the dispatcher and presses nothing.
        assert_eq!(reg.ext::<Panel>().issued(), ["/bots list"]);
    }

    #[test]
    fn a_normal_server_leaves_the_agent_to_the_missions() {
        let (mut reg, me) = world(false);
        assert!(answer(&mut reg, Some(me), BASIC_LOW, None).is_none());
        assert!(reg.ext::<Panel>().issued().is_empty());
    }

    fn press_id(key: &str) -> i64 {
        ACT_PRESET + shipped().iter().position(|p| p.key == key).unwrap() as i64
    }

    #[test]
    fn a_press_spawns_in_the_system_the_caller_is_in() {
        let (mut reg, me) = world(true);
        let home = reg.character(me).unwrap().solar_system_id.unwrap();
        let other = home + 7_001;
        let ship = reg.character(me).unwrap().ship_id;
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                me,
                ship,
                other,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        answer(&mut reg, Some(me), BASIC_LOW, Some(press_id("ally-miners"))).unwrap();
        let issued = reg.ext::<Panel>().issued().to_vec();
        assert!(issued[0].ends_with(&format!(" {other}")), "{issued:?}");
        assert!(!issued[0].ends_with(&format!(" {home}")), "{issued:?}");
    }

    #[test]
    fn a_press_issues_the_expected_bots_lines_as_the_caller() {
        let (mut reg, me) = world(true);
        let system = reg.character(me).unwrap().solar_system_id.unwrap();
        let v = answer(&mut reg, Some(me), BASIC_LOW, Some(press_id("ally-miners"))).unwrap();
        let issued = reg.ext::<Panel>().issued().to_vec();
        // The press joined the caller to the allied dev corporation first.
        let side = ALLY_SIDE;
        assert_eq!(issued.len(), 3, "{issued:?}");
        assert!(
            issued[0].starts_with("/bots spawn ao")
                && issued[0].ends_with(&format!(" {side} foreman 1 role Orca {system}")),
            "{issued:?}"
        );
        assert!(
            issued[1].ends_with(&format!(" {side} mine 10 role Hulk {system}")),
            "{issued:?}"
        );
        assert_eq!(issued[2], "/bots list");
        let groups = reg.ext::<Panel>().groups().to_vec();
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|g| g.owner == me && g.ally));
        assert!(html_of(&v).contains("Last press"));
        // A second press never reuses a group name.
        answer(&mut reg, Some(me), BASIC_LOW, Some(press_id("ally-miners"))).unwrap();
        let names: Vec<String> = reg
            .ext::<Panel>()
            .groups()
            .iter()
            .map(|g| g.group.clone())
            .collect();
        let mut dedup = names.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(dedup.len(), names.len(), "{names:?}");
    }

    #[test]
    fn a_hostile_press_needs_no_corporation_and_bring_and_despawn_map_to_commands() {
        let (mut reg, me) = world(true);
        answer(&mut reg, Some(me), BASIC_LOW, Some(press_id("foe-campers"))).unwrap();
        let first = reg.ext::<Panel>().issued()[0].clone();
        assert!(
            first.starts_with("/bots spawn hc") && first.contains(" FOES hunter 4 role Rifter "),
            "{first}"
        );
        // No allied group yet: bring says so and issues nothing.
        let v = answer(&mut reg, Some(me), BASIC_LOW, Some(ACT_BRING)).unwrap();
        assert!(html_of(&v).contains("no allied groups"));
        assert!(
            !reg.ext::<Panel>()
                .issued()
                .iter()
                .any(|l| l.starts_with("/bots bring"))
        );
        answer(
            &mut reg,
            Some(me),
            BASIC_LOW,
            Some(press_id("ally-escorts")),
        )
        .unwrap();
        let group = reg.ext::<Panel>().groups().last().unwrap().group.clone();
        answer(&mut reg, Some(me), BASIC_LOW, Some(ACT_BRING)).unwrap();
        assert!(
            reg.ext::<Panel>()
                .issued()
                .contains(&format!("/bots bring {group}"))
        );
        answer(&mut reg, Some(me), BASIC_LOW, Some(ACT_DESPAWN)).unwrap();
        assert!(
            reg.ext::<Panel>()
                .issued()
                .contains(&"/bots despawn all".to_owned())
        );
        assert!(reg.ext::<Panel>().groups().is_empty());
    }

    /// Through the real `agentMgr` dispatcher, bound to the agent as the client does.
    #[test]
    fn the_agent_mgr_routes_the_panel_agent_and_leaves_the_others() {
        use crate::service::Request;
        let (mut reg, me) = world(true);
        let mut call = |method: &str, bound: i64, args: &[PyValue]| {
            let bind = PyValue::Int(bound);
            reg.dispatch(&Request {
                service: "agentMgr",
                method,
                args,
                kwargs: &[],
                bind_params: Some(&bind),
                actor: Some(me),
            })
            .unwrap()
        };
        let open = call("DoAction", BASIC_LOW, &[PyValue::None]);
        assert!(html_of(&open).contains("Bot Control"));
        assert_eq!(
            call("GetMissionBriefingInfo", BASIC_LOW, &[]),
            PyValue::None
        );
        assert_eq!(
            call("GetMissionObjectiveInfo", BASIC_LOW, &[]),
            PyValue::None
        );
        let press = call("DoAction", BASIC_LOW, &[PyValue::Int(ACT_DESPAWN)]);
        assert!(html_of(&press).contains("Despawn all"));
        // Another agent in the same station still talks missions.
        let other = call("DoAction", BASIC_HIGH, &[PyValue::None]);
        assert!(!html_of(&other).contains("Bot Control"));
    }

    #[test]
    fn nearby_stays_home_without_a_gate_graph() {
        let (reg, _) = world(true);
        assert_eq!(nearby(&reg, 30_000_001, 3, 5), 30_000_001);
    }

    #[test]
    fn the_html_escapes_the_bot_list() {
        let h = body(&[], "a <b> & c\nd", "RED");
        assert!(h.contains("a &lt;b&gt; &amp; c<br>d"), "{h}");
    }
}
