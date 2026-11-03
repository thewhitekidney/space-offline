//! **Seams for world events** (lane l6b-mining-events, issue #33): what a
//! module needs from the core to run an event in space (a rich belt, an
//! ambush) without the core knowing about the event.
//!
//! Everything here is CCP's mechanics or plain plumbing; *which* events
//! happen, how often and what they pay is `eve-mod-econ::motherlode`.
//!
//! - [`message`]: `OnServerMessage(text)` to pilots (the client's
//!   `gameui.OnServerMessage`, which shows the text in a dialog [V]).
//! - [`notice`]: `OnCCPNotification(title, subtitle, text, typeID)` to pilots:
//!   the client's own toast in the notification stack and an entry in its
//!   centre, which never takes the screen (`ccpNotificationAdapter`, build
//!   3662168 [V]). For news that is no reason to stop a pilot mid-cycle.
//! - [`say_in_local`]: a line in a system's Local, spoken by an online bot that is
//!   there (the only speaker the server has; a bot hears it as a Local line).
//! - [`near`], [`system_facts`], [`belt_centre`]: the map.
//! - [`ambush`], [`clear_ambush`]: one extra squad of the system's pirates at
//!   a belt or a site, and taking it back out.
//! - [`rock_site`]: which population row a rock in an anomaly was drawn by.

use crate::celestial::{Band, Kind as CelKind};
use crate::damage::BlueTime;
use crate::notify::Push;
use crate::pyutil::bstr;
use crate::service::Registry;
use eve_marshal::PyValue;
use std::time::Duration;

/// `OnServerMessage(msg)`: `gameui.OnServerMessage` takes a string (or a
/// `(label, kwargs)` pair) and raises the `ServerMessage` dialog with it [V].
#[derive(Debug, Clone)]
pub struct ServerMessage {
    pub to: Vec<i64>,
    pub text: String,
}

impl Push for ServerMessage {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnServerMessage", vec![bstr(&self.text)])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `notificationconst.notificationTypeCCPServerNews` [V]: a CCP-news type,
/// which the toast draws with its own icon and, on a click, opens the text
/// in a **non-modal** `CustomInfo` window (`notificationCenter.OnEntryClick`).
pub const NOTICE_TYPE: i64 = 171;

/// `OnCCPNotification(title, subtitle, text, notificationTypeID)`: the client's
/// `CCPNotificationAdapter` (registered by `notificationUI`) builds a
/// notification from the three strings and scatters
/// `OnNewNotificationReceived`, which stacks a toast bottom-right and files it
/// in the notification centre. `language` is left off, which shows it in any
/// language [V]. A click calls `notificationMgr.LogNotificationInteraction`,
/// which `service::notifications` answers [V].
#[derive(Debug, Clone)]
pub struct Notice {
    pub to: Vec<i64>,
    pub title: String,
    pub subtitle: String,
    pub text: String,
}

impl Push for Notice {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnCCPNotification",
            vec![
                bstr(&self.title),
                bstr(&self.subtitle),
                bstr(&self.text),
                PyValue::Int(NOTICE_TYPE),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// The characters in space or docked in `system` that are online: the roster
/// less NPCs. A docked pilot is not in the roster, so those come from the
/// characters' own system.
pub fn pilots_in(reg: &Registry, system: i64) -> Vec<i64> {
    reg.roster()
        .in_system(system)
        .into_iter()
        .map(|p| p.character_id)
        .filter(|id| reg.character(*id).is_some())
        .collect()
}

/// The online characters whose present system is `system`, in space or
/// docked: who a system-wide message is for.
pub fn online_in(reg: &Registry, system: i64) -> Vec<i64> {
    reg.sessions()
        .online_characters()
        .into_iter()
        .filter(|id| reg.present_system(*id) == Some(system))
        .collect()
}

/// The system `character` is in now, in space or docked.
pub fn system_of(reg: &Registry, character: i64) -> Option<i64> {
    reg.present_system(character)
}

/// The lowest-numbered belt of `system`, if the catalogue has one.
pub fn first_belt(reg: &mut Registry, system: i64) -> Option<i64> {
    crate::belts::belts_in(reg, system)
        .into_iter()
        .map(|(id, _)| id)
        .min()
}

/// Queue `text` as an `OnServerMessage` to `to`, on the clock. Nothing for an
/// empty audience.
pub fn message(reg: &mut Registry, to: Vec<i64>, text: &str) {
    if to.is_empty() {
        return;
    }
    reg.schedule(
        Duration::ZERO,
        ServerMessage {
            to,
            text: text.to_owned(),
        },
    );
}

/// Queue a [`Notice`] to `to`, on the clock: a toast and a centre entry, no
/// dialog. Nothing for an empty audience.
pub fn notice(reg: &mut Registry, to: Vec<i64>, title: &str, subtitle: &str, text: &str) {
    if to.is_empty() {
        return;
    }
    reg.schedule(
        Duration::ZERO,
        Notice {
            to,
            title: title.to_owned(),
            subtitle: subtitle.to_owned(),
            text: text.to_owned(),
        },
    );
}

/// Say `text` in `system`'s Local as the lowest-numbered **bot** in space
/// there (a bot is a derived member of its Local without a stream). `false`
/// when chat is off or no bot is there.
pub fn say_in_local(reg: &Registry, system: i64, text: &str) -> bool {
    let Some(hub) = reg.chat() else { return false };
    let Some(speaker) = online_in(reg, system)
        .into_iter()
        .filter(|id| reg.character(*id).is_some_and(|c| c.is_bot))
        .min()
    else {
        return false;
    };
    let rooms = crate::chat::system_rooms(reg, speaker);
    let Some(room) = rooms.first() else {
        return false;
    };
    hub.say(room, speaker, text)
}

/// The systems within `jumps` gates of `origin`, nearest first, `origin`
/// not among them.
pub fn near(reg: &Registry, origin: i64, jumps: u32) -> Vec<i64> {
    let cat = reg.celestials();
    let hops = |system: i64| -> Vec<i64> {
        let mut out: Vec<i64> = cat
            .served_in(system)
            .filter(|c| c.kind == CelKind::Gate)
            .filter(|c| cat.system_of(c.id) == Some(system))
            .filter_map(|c| cat.jump_of(c.id))
            .map(|(_, to)| to)
            .filter(|to| *to != system)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    };
    crate::escalation::within(origin, jumps, &hops)
        .into_iter()
        .skip(1)
        .collect()
}

/// What a system is, for an event that cares: its name, region and band.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemFacts {
    pub name: String,
    pub region_id: i64,
    pub band: Band,
}

/// [`SystemFacts`] of `system`, `None` when the universe table lacks it.
pub fn system_facts(reg: &Registry, system: i64) -> Option<SystemFacts> {
    let u = reg.content().get::<crate::celestial::Universe>();
    let s = u.system(system)?;
    Some(SystemFacts {
        name: s.name.clone(),
        region_id: s.region_id,
        band: s.band(),
    })
}

/// Where belt `belt_id` of `system` is, `None` for a belt the catalogue does
/// not have.
pub fn belt_centre(reg: &mut Registry, system: i64, belt_id: i64) -> Option<[f64; 3]> {
    crate::belts::belts_in(reg, system)
        .into_iter()
        .find(|(id, _)| *id == belt_id)
        .map(|(_, at)| at)
}

/// One extra squad of `system`'s pirates round `centre`, drawn for `salt`
/// (see [`crate::belts::wave`]). The ids, empty when none could be made.
pub fn ambush(
    reg: &mut Registry,
    system: i64,
    belt_id: i64,
    centre: [f64; 3],
    salt: u64,
) -> Vec<i64> {
    crate::belts::wave(reg, system, belt_id, centre, salt)
}

/// Take an [`ambush`]'s survivors out of space.
pub fn clear_ambush(reg: &mut Registry, system: i64, ids: &[i64]) {
    crate::belts::clear_wave(reg, system, ids);
}

/// Whether NPC `id` is still in space.
pub fn in_space(reg: &Registry, id: i64) -> bool {
    reg.roster().get(id).is_some()
}

/// The anomaly a rock belongs to and the population row that drew it.
#[derive(Debug, Clone, PartialEq)]
pub struct RockSite {
    pub site_id: i64,
    pub system: i64,
    pub anchor: [f64; 3],
    /// The `population.tsv` key of the slot.
    pub row_key: String,
}

/// The site `rock_id` is placed in, if it is a site's rock (a belt rock is
/// not), and the population row key of that site, as of `now`.
pub fn rock_site(reg: &Registry, rock_id: i64, now: BlueTime) -> Option<RockSite> {
    let inst = reg.ext::<crate::site::Sites>().of_item(rock_id)?;
    let (site_id, system, anchor) = (inst.site_id, inst.system, inst.anchor);
    let resolved = crate::population::sites(reg, system, now);
    let site = resolved.sites.iter().find(|s| s.site_id == site_id)?;
    let row_key = reg
        .ext::<crate::population::Population>()
        .prepared()
        .rows()
        .get(site.row)?
        .key
        .clone();
    Some(RockSite {
        site_id,
        system,
        anchor,
        row_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_message_is_a_plain_string_to_its_audience() {
        let m = ServerMessage {
            to: vec![7, 8],
            text: "hello".to_owned(),
        };
        let (name, args) = m.to_push();
        assert_eq!(name, "OnServerMessage");
        assert_eq!(args, vec![bstr("hello")]);
        assert_eq!(m.audience(), Some(vec![7, 8]));
    }

    #[test]
    fn a_notice_is_the_clients_ccp_notification_with_three_strings_and_a_type() {
        let n = Notice {
            to: vec![7],
            title: "t".to_owned(),
            subtitle: "s".to_owned(),
            text: "x".to_owned(),
        };
        let (name, args) = n.to_push();
        assert_eq!(name, "OnCCPNotification");
        assert_eq!(
            args,
            vec![bstr("t"), bstr("s"), bstr("x"), PyValue::Int(NOTICE_TYPE)]
        );
        assert_eq!(n.audience(), Some(vec![7]));
    }
}
