//! **The share-link wire** (lane `s1-share-links`): `overviewPresetMgr` and
//! `sharedSettingsMgr`. The store is `crate::share_links`.
//!
//! | call | answer |
//! | --- | --- |
//! | `overviewPresetMgr.StoreLinkAndGetID(data)` | `KeyVal(hashvalue, sqID)` or `None` |
//! | `overviewPresetMgr.GetStoredPreset((hash, sqID))` | the YAML text or `None` |
//! | `sharedSettingsMgr.StoreSettingLinkAndGetID(typeID, data)` | `KeyVal(hashvalue, sqID, settingTypeID)` or `None` |
//! | `sharedSettingsMgr.GetStoredSharedSetting((hash, sqID, typeID))` | the YAML text or `None` |
//!
//! `None` from a store is the client's `OverviewProfileLoadingError` or
//! `ItemFilterLoadingError`; from a fetch it is the same error on open.

use super::*;
use crate::share_links as links;

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &["overviewPresetMgr", "sharedSettingsMgr"];

fn keyval(link: Option<links::Link>, ty: Option<i64>) -> PyValue {
    let Some((hash, sq)) = link else {
        return PyValue::None;
    };
    let mut attrs = vec![
        ("hashvalue", PyValue::Unicode(hash)),
        ("sqID", PyValue::Int(sq)),
    ];
    if let Some(t) = ty {
        attrs.push(("settingTypeID", PyValue::Int(t)));
    }
    crate::pyutil::keyval(attrs)
}

/// `(hash, sqID, rest...)` out of the key tuple the link handler builds. A
/// hash holding `//` could not have come from `overviewPreset:<hash>//<sq>`.
fn parse_key(arg: Option<&PyValue>) -> Option<(String, i64, Option<i64>)> {
    let (PyValue::Tuple(t) | PyValue::List(t)) = arg? else {
        return None;
    };
    let hash = match t.first()? {
        PyValue::Unicode(s) => s.clone(),
        PyValue::Bytes(b) => String::from_utf8(b.clone()).ok()?,
        _ => return None,
    };
    if hash.is_empty() || hash.contains("//") {
        return None;
    }
    let PyValue::Int(sq) = t.get(1)? else {
        return None;
    };
    let ty = match t.get(2) {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    };
    Some((hash, *sq, ty))
}

fn yaml_value(y: Option<String>) -> PyValue {
    y.map_or(PyValue::None, PyValue::Unicode)
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match (req.service, req.method) {
        ("overviewPresetMgr", "StoreLinkAndGetID") => {
            let link = match (req.actor, req.args.first()) {
                (Some(ch), Some(data)) => links::store(reg, ch, links::TYPE_OVERVIEW, data),
                _ => None,
            };
            Ok(keyval(link, None))
        }
        ("overviewPresetMgr", "GetStoredPreset") => Ok(yaml_value(
            parse_key(req.args.first())
                .and_then(|(h, sq, _)| links::fetch(reg, &h, sq, links::TYPE_OVERVIEW)),
        )),
        ("sharedSettingsMgr", "StoreSettingLinkAndGetID") => {
            let ty = match req.args.first() {
                Some(PyValue::Int(t))
                    if matches!(*t, links::TYPE_ITEMS | links::TYPE_BROADCAST) =>
                {
                    Some(*t)
                }
                _ => None,
            };
            let link = match (req.actor, ty, req.args.get(1)) {
                (Some(ch), Some(t), Some(data)) => links::store(reg, ch, t, data),
                _ => None,
            };
            Ok(keyval(link, ty))
        }
        ("sharedSettingsMgr", "GetStoredSharedSetting") => Ok(yaml_value(
            parse_key(req.args.first()).and_then(|(h, sq, ty)| links::fetch(reg, &h, sq, ty?)),
        )),
        ("overviewPresetMgr" | "sharedSettingsMgr", _) => Err(reg.no_method(req)),
        // Unreachable: `SERVICES` above gates the call.
        (other, _) => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: i64 = 90_000_001;

    fn call(reg: &mut Registry, service: &str, method: &str, args: &[PyValue]) -> PyValue {
        dispatch(
            reg,
            &Request {
                service,
                method,
                args,
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        )
        .expect("answered")
    }

    fn attr(kv: &PyValue, name: &str) -> PyValue {
        let PyValue::Instance { state, .. } = kv else {
            panic!("a KeyVal, got {kv:?}")
        };
        let PyValue::Dict(d) = &**state else {
            panic!("a state dict")
        };
        d.iter()
            .find(|(_, k)| {
                *k == PyValue::Unicode(name.to_owned())
                    || *k == PyValue::Bytes(name.as_bytes().to_vec())
            })
            .map(|(v, _)| v.clone())
            .unwrap_or_else(|| panic!("no attribute {name}"))
    }

    fn data() -> PyValue {
        PyValue::Dict(vec![(
            PyValue::Tuple(vec![PyValue::Unicode("f".to_owned()), PyValue::Int(1)]),
            PyValue::Unicode("filter".to_owned()),
        )])
    }

    #[test]
    fn the_overview_wire_round_trips() {
        let mut reg = Registry::new(SandboxConfig::default());
        let kv = call(
            &mut reg,
            "overviewPresetMgr",
            "StoreLinkAndGetID",
            &[data()],
        );
        let key = PyValue::Tuple(vec![attr(&kv, "hashvalue"), attr(&kv, "sqID")]);
        assert_eq!(
            call(&mut reg, "overviewPresetMgr", "GetStoredPreset", &[key]),
            PyValue::Unicode(r#"[["filter", ["f", 1]]]"#.to_owned())
        );
        let bad = PyValue::Tuple(vec![PyValue::Unicode("ab//c".to_owned()), PyValue::Int(1)]);
        assert_eq!(
            call(&mut reg, "overviewPresetMgr", "GetStoredPreset", &[bad]),
            PyValue::None
        );
    }

    #[test]
    fn the_settings_wire_carries_its_type() {
        let mut reg = Registry::new(SandboxConfig::default());
        let kv = call(
            &mut reg,
            "sharedSettingsMgr",
            "StoreSettingLinkAndGetID",
            &[PyValue::Int(3), data()],
        );
        assert_eq!(attr(&kv, "settingTypeID"), PyValue::Int(3));
        let key = |ty| {
            PyValue::Tuple(vec![
                attr(&kv, "hashvalue"),
                attr(&kv, "sqID"),
                PyValue::Int(ty),
            ])
        };
        assert!(matches!(
            call(
                &mut reg,
                "sharedSettingsMgr",
                "GetStoredSharedSetting",
                &[key(3)]
            ),
            PyValue::Unicode(_)
        ));
        assert_eq!(
            call(
                &mut reg,
                "sharedSettingsMgr",
                "GetStoredSharedSetting",
                &[key(2)]
            ),
            PyValue::None
        );
        // An unknown type, or no actor, stores nothing.
        assert_eq!(
            call(
                &mut reg,
                "sharedSettingsMgr",
                "StoreSettingLinkAndGetID",
                &[PyValue::Int(9), data()]
            ),
            PyValue::None
        );
    }
}
