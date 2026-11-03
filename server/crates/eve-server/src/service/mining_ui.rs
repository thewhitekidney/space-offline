//! The mining windows' services (MINE-UI, EVEJS-GAP svcC S3 and S4):
//! `miningScanMgr` (the survey scanner's quantities) and
//! `characterMiningLedger` / `corpMiningLedger` (the Mining Ledger window).
//!
//! The pushes that feed them (`OnOreMined`, `OnOreMinedByOther`) are
//! `crate::mining`'s; the ledger's rows are `crate::mining_ledger`'s.

use super::*;

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] =
    &["miningScanMgr", "characterMiningLedger", "corpMiningLedger"];

/// The columns `GetCharacterLogs` rows are read by **[V]**
/// (`ledgerEntry.py:24-41`, `ledgerUtil.py:100-145`). **`solarsystemID` has a
/// lower-case `s`** (`ledgerEntry.py:30`).
const LOG_COLUMNS: &[(&str, DbType)] = &[
    ("eventDate", DbType::FileTime),
    ("characterID", DbType::I4),
    ("corporationID", DbType::I4),
    ("solarsystemID", DbType::I4),
    ("typeID", DbType::I4),
    ("quantity", DbType::I4),
    ("quantityWasted", DbType::I4),
];

fn clamp_i4(v: i64) -> i64 {
    v.clamp(0, i64::from(i32::MAX))
}

/// `characterMiningLedger.GetCharacterLogs`: the caller's rows as a `CRowset`.
fn character_logs(reg: &Registry, actor: i64) -> PyValue {
    let mut rs = eve_rowset::CRowset::new(
        DbRowDescriptor::from_pairs(LOG_COLUMNS).expect("a fixed, valid layout"),
    );
    for r in crate::mining_ledger::logs(reg.deltas(), actor, crate::pyutil::filetime_now()) {
        let _ = rs.push_values(vec![
            DbValue::Int(crate::mining_ledger::event_time(r.day)),
            DbValue::Int(r.character_id),
            DbValue::Int(r.corporation_id),
            DbValue::Int(r.solar_system_id),
            DbValue::Int(r.type_id),
            DbValue::Int(clamp_i4(r.quantity)),
            DbValue::Int(clamp_i4(r.wasted)),
        ]);
    }
    rs.to_pyvalue()
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match (req.service, req.method) {
        // `None` makes the client fall back to its tier-1 scan.
        ("miningScanMgr", "perform_scan") => Ok(req
            .actor
            .and_then(|a| crate::mining::survey(reg, a))
            .map_or(PyValue::None, |rows| {
                PyValue::List(
                    rows.into_iter()
                        .map(|(rock, ty, q)| {
                            PyValue::Tuple(vec![
                                PyValue::Int(rock),
                                PyValue::Int(ty),
                                PyValue::Int(q),
                            ])
                        })
                        .collect(),
                )
            })),
        ("characterMiningLedger", "GetCharacterLogs") => match req.actor {
            Some(a) => Ok(character_logs(reg, a)),
            None => Ok(PyValue::List(Vec::new())),
        },
        // There are no Upwell observers here, so empty is the truth; the
        // window iterates it for the corp tab (`ledgerSvc.py:28-38`).
        ("corpMiningLedger", "GetObserversWithMiningEvents" | "GetObserverLedger") => {
            Ok(PyValue::List(Vec::new()))
        }
        _ => Err(reg.no_method(req)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mining_ledger as ml;

    const SYS: i64 = 30_000_001;

    fn call(reg: &mut Registry, service: &str, method: &str, actor: Option<i64>) -> PyValue {
        dispatch(
            reg,
            &Request {
                service,
                method,
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor,
            },
        )
        .expect("answered")
    }

    fn registry_with_rows() -> Registry {
        let mut reg = crate::testkit::registry();
        let who = reg.config.characters[0].character_id;
        let now = crate::pyutil::filetime_now();
        ml::record(reg.deltas_mut(), who, 7, SYS, 1230, 5, 1, now);
        ml::record(reg.deltas_mut(), who, 7, SYS, 1228, 9, 0, now);
        ml::record(
            reg.deltas_mut(),
            who,
            7,
            SYS,
            1228,
            2,
            0,
            now - 91 * crate::seed::DAY,
        );
        reg
    }

    #[test]
    fn the_log_columns_are_exactly_the_seven_the_client_reads() {
        let names: Vec<&str> = LOG_COLUMNS.iter().map(|c| c.0).collect();
        assert_eq!(
            names,
            [
                "eventDate",
                "characterID",
                "corporationID",
                "solarsystemID",
                "typeID",
                "quantity",
                "quantityWasted"
            ]
        );
    }

    /// The answer is a `CRowset` with the seven columns, one row per ledger
    /// row inside the window; the pilot's own rows only.
    #[test]
    fn get_character_logs_answers_a_crowset_of_the_window() {
        let mut reg = registry_with_rows();
        let who = reg.config.characters[0].character_id;
        let rs = call(
            &mut reg,
            "characterMiningLedger",
            "GetCharacterLogs",
            Some(who),
        );
        let text = format!("{rs:?}");
        assert_eq!(
            ml::logs(reg.deltas(), who, crate::pyutil::filetime_now()).len(),
            2
        );
        let again = call(
            &mut reg,
            "characterMiningLedger",
            "GetCharacterLogs",
            Some(who + 1),
        );
        assert_ne!(
            format!("{again:?}"),
            text,
            "another pilot has none of these rows"
        );
    }

    /// The corp tab's calls answer empty lists: no Upwell observers here.
    #[test]
    fn the_corp_calls_answer_empty_lists() {
        let mut reg = crate::testkit::registry();
        for m in ["GetObserversWithMiningEvents", "GetObserverLedger"] {
            assert_eq!(
                call(&mut reg, "corpMiningLedger", m, Some(1)),
                PyValue::List(Vec::new())
            );
        }
    }

    /// With no scanner tier the survey is `None` (the tier-1 fallback).
    #[test]
    fn perform_scan_without_a_scanner_is_none() {
        let mut reg = crate::testkit::registry();
        assert_eq!(
            call(&mut reg, "miningScanMgr", "perform_scan", None),
            PyValue::None
        );
    }
}
