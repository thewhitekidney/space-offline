//! World-event tales: the `OnTaleData` push and the Job Board rows.
//!
//! Packet CP51 (`docs/content/plan.md` §6.2, W13), from
//! `docs/content/C2-live-surface.md` §7 and packet K8. A world event is the
//! cheapest "something is happening here" lever: a tale that names an
//! `objectiveChainID` gets the client's full objective tracker from its own
//! `objectivechains` static data, for the cost of one push.
//!
//! **Two shapes, two consumers.** [V]
//! - `OnTaleData(solarSystemID, {taleID: tale})`, where each `tale` is read by
//!   **attribute** (`taleclient/tale_service:106`, `getattr(tale,
//!   'objectiveChainID', None)`; `taleclient/active_tale:97-113`, `update`).
//!   The handler drops the whole push unless `solarSystemID` is the session's
//!   `solarsystemid2` (`tale_service:79`).
//! - `taleMgr.GetGlobalWorldEventTales()`, a list of **dicts** that the Job
//!   Board string-subscripts (`jobboard/client/features/world_events/provider:39-40`,
//!   `job:35-63`).
//!
//! **The rows come from [`Table`]**, read with `reg.content().try_get`. No
//! loader is registered yet (the CP51 row registers nothing), so a server
//! today has no tales and both answers are empty, which is true. A later
//! packet adds the authored file and its `TABLES` row; nothing here changes.

use crate::pyutil::{keyval, pydict};
use eve_marshal::PyValue;

/// `talecommon.const.templateClass.worldEvents`. [V] (`talecommon/const:27`)
pub const WORLD_EVENTS: i64 = 9;

/// One active tale. Field names follow the client's attribute names.
#[derive(Debug, Clone, PartialEq)]
pub struct Tale {
    pub tale_id: i64,
    pub template_id: i64,
    pub template_class_id: i64,
    /// `None` means the client ignores the tale (`tale_service:106`), so
    /// [`sendable`] keeps it off the wire altogether.
    pub objective_chain_id: Option<i64>,
    /// The system the tale is active in: `ActiveTale.solar_system_id`.
    pub location_id: i64,
    pub manager_solar_system_id: i64,
    /// A localization message id; `job:63` passes it to `GetByMessageID`.
    pub name_id: i64,
    pub description_id: Option<i64>,
    pub aggressor_faction_id: Option<i64>,
    /// FILETIME, or `None` for open-ended.
    pub end_time: Option<i64>,
    /// `{solarSystemID: [sceneType, ...]}`; `active_tale:108` calls `.get`
    /// on it, so it is always a dict, possibly empty.
    pub scene_types: Vec<(i64, Vec<i64>)>,
}

/// Whether the client would use the tale at all (`tale_service:106`).
pub fn sendable(t: &Tale) -> bool {
    t.objective_chain_id.is_some()
}

fn opt(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

/// The tale as the push carries it: an attribute bag.
///
/// [A] `utillib.KeyVal`: the client only reads and sets attributes on it
/// (`active_tale:75`, `self._tale.influence = value`), and no tale class is
/// in `globalsWhitelist`. Falsified by an `AttributeError` or a whitelist
/// refusal on `OnTaleData` in the client log.
pub fn to_keyval(t: &Tale) -> PyValue {
    let scenes = t
        .scene_types
        .iter()
        .map(|(ssid, kinds)| {
            let kinds = kinds.iter().copied().map(PyValue::Int).collect();
            (PyValue::List(kinds), PyValue::Int(*ssid))
        })
        .collect();
    keyval(vec![
        ("taleID", PyValue::Int(t.tale_id)),
        ("templateID", PyValue::Int(t.template_id)),
        ("templateClassID", PyValue::Int(t.template_class_id)),
        ("objectiveChainID", opt(t.objective_chain_id)),
        ("locationID", PyValue::Int(t.location_id)),
        (
            "managerSolarSystemID",
            PyValue::Int(t.manager_solar_system_id),
        ),
        ("templateNameID", PyValue::Int(t.name_id)),
        ("templateDescriptionID", opt(t.description_id)),
        ("aggressorFactionID", opt(t.aggressor_faction_id)),
        ("endTime", opt(t.end_time)),
        // `PyValue::Dict` stores `(value, key)`; see `pyutil::pydict`.
        ("sceneTypesByLocationID", PyValue::Dict(scenes)),
    ])
}

/// The tale as one Job Board row: the keys of the client's own
/// `_details_from_active_tale` (`provider:87`), every subscripted one present
/// (`job:35-63`).
pub fn job_row(t: &Tale) -> PyValue {
    pydict(vec![
        ("taleID", PyValue::Int(t.tale_id)),
        ("templateID", PyValue::Int(t.template_id)),
        (
            "managerSolarSystemID",
            PyValue::Int(t.manager_solar_system_id),
        ),
        ("influence", PyValue::None),
        ("templateNameID", PyValue::Int(t.name_id)),
        ("templateDescriptionID", opt(t.description_id)),
        ("endTime", opt(t.end_time)),
        ("aggressorFactionID", opt(t.aggressor_faction_id)),
    ])
}

/// The active tales. Empty until a loader is registered (see the module doc).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub tales: Vec<Tale>,
}

impl Table {
    /// Every world event the client would render, for the Job Board.
    pub fn global_rows(&self) -> PyValue {
        PyValue::List(
            self.tales
                .iter()
                .filter(|t| sendable(t) && t.template_class_id == WORLD_EVENTS)
                .map(job_row)
                .collect(),
        )
    }

    /// The push for a session arriving in `system`, or `None` when no tale
    /// there is sendable.
    pub fn data_for(&self, system: i64) -> Option<TaleData> {
        let tales: Vec<Tale> = self
            .tales
            .iter()
            .filter(|t| sendable(t) && t.location_id == system)
            .cloned()
            .collect();
        (!tales.is_empty()).then_some(TaleData { system, tales })
    }
}

/// Queue `OnTaleData` for the acting session, which has just arrived in
/// `system`. The caller is the session-change hook; with no [`Table`]
/// registered this does nothing.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the arrival hook calls it (docs/lanes/cp51.md, For the next packet); no packet yet"
    )
)]
pub fn on_enter_system(reg: &mut crate::service::Registry, system: i64) {
    let push = reg
        .content()
        .try_get::<Table>()
        .and_then(|t| t.data_for(system));
    if let Some(push) = push {
        reg.push(push);
    }
}

/// `OnTaleData(solarSystemID, {taleID: tale})` (`tale_service:75-82`). [V]
#[derive(Debug, Clone, PartialEq)]
pub struct TaleData {
    pub system: i64,
    pub tales: Vec<Tale>,
}

impl crate::notify::Push for TaleData {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let data = self
            .tales
            .iter()
            .filter(|t| sendable(t))
            .map(|t| (to_keyval(t), PyValue::Int(t.tale_id)))
            .collect();
        (
            "OnTaleData",
            vec![PyValue::Int(self.system), PyValue::Dict(data)],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::Push;
    use crate::pyutil::dict_get;

    /// Invented rows: ids from no client table.
    fn tale(id: i64, chain: Option<i64>) -> Tale {
        Tale {
            tale_id: id,
            template_id: 7,
            template_class_id: WORLD_EVENTS,
            objective_chain_id: chain,
            location_id: 100,
            manager_solar_system_id: 101,
            name_id: 5,
            description_id: None,
            aggressor_faction_id: None,
            end_time: None,
            scene_types: vec![(100, vec![1])],
        }
    }

    fn fixture() -> Table {
        Table {
            tales: vec![tale(1, Some(40)), tale(2, None)],
        }
    }

    fn attrs(v: &PyValue) -> &PyValue {
        let PyValue::Instance { state, .. } = v else {
            panic!("not an instance: {v:?}")
        };
        state
    }

    #[test]
    fn a_tale_without_an_objective_chain_is_never_sent() {
        let t = fixture();
        let (_, args) = t.data_for(100).expect("tale 1 is sendable").to_push();
        let PyValue::Dict(data) = &args[1] else {
            panic!()
        };
        assert_eq!(data.len(), 1);
        assert_eq!(data[0].1, PyValue::Int(1));
        let PyValue::List(rows) = t.global_rows() else {
            panic!()
        };
        assert_eq!(rows.len(), 1);
        let only_unsendable = Table {
            tales: vec![tale(2, None)],
        };
        assert!(only_unsendable.data_for(100).is_none());
        // Even a push built by hand drops it.
        let by_hand = TaleData {
            system: 100,
            tales: vec![tale(2, None)],
        };
        assert_eq!(by_hand.to_push().1[1], PyValue::Dict(vec![]));
    }

    /// `tale_service:79` drops a push for another system, so it is not built.
    #[test]
    fn the_push_is_for_the_arrival_system_only() {
        assert!(fixture().data_for(999).is_none());
        let (name, args) = fixture().data_for(100).unwrap().to_push();
        assert_eq!(name, "OnTaleData");
        assert_eq!(args[0], PyValue::Int(100));
    }

    /// The attributes the client reads without a default
    /// (`active_tale:99-108`, `tale_service:108`), and the two the row names.
    #[test]
    fn the_values_carry_tale_id_and_manager_solar_system_id() {
        let (_, args) = fixture().data_for(100).unwrap().to_push();
        let PyValue::Dict(data) = &args[1] else {
            panic!()
        };
        let state = attrs(&data[0].0);
        for key in [
            "taleID",
            "templateID",
            "templateClassID",
            "locationID",
            "managerSolarSystemID",
            "sceneTypesByLocationID",
            "objectiveChainID",
        ] {
            assert!(dict_get(state, key.as_bytes()).is_some(), "{key}");
        }
        assert_eq!(dict_get(state, b"taleID"), Some(&PyValue::Int(1)));
        assert_eq!(
            dict_get(state, b"managerSolarSystemID"),
            Some(&PyValue::Int(101))
        );
        let PyValue::List(rows) = fixture().global_rows() else {
            panic!()
        };
        // `job:35-63` subscripts these, so each must be present.
        for key in [
            "taleID",
            "templateID",
            "managerSolarSystemID",
            "aggressorFactionID",
            "templateNameID",
        ] {
            assert!(dict_get(&rows[0], key.as_bytes()).is_some(), "{key}");
        }
    }

    #[test]
    fn the_job_board_lists_world_events_only() {
        let mut t = fixture();
        t.tales[0].template_class_id = WORLD_EVENTS - 1;
        assert_eq!(t.global_rows(), PyValue::List(vec![]));
    }

    #[test]
    fn entering_a_system_queues_its_tales() {
        let mut content = crate::content::Content::fixture();
        content.insert("tales", fixture());
        let mut reg = crate::service::Registry::new(Default::default())
            .with_content(std::sync::Arc::new(content));
        on_enter_system(&mut reg, 999);
        assert!(reg.take_pending().is_empty());
        on_enter_system(&mut reg, 100);
        let pending = reg.take_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1.to_push().0, "OnTaleData");
    }
}
