//! **The Mining Ledger** (MINE-UI, EVEJS-GAP svcC S4): what each character
//! has mined, per UTC day, solar system and ore type, for the Mining Ledger
//! window (`characterMiningLedger.GetCharacterLogs`).
//!
//! State is one delta kind, [`KIND`], so a save keeps it: key
//! `character day system type`, value `quantity wasted corporation`.
//!
//! **[A]** One row per (character, day, system, type), a day's mining
//! summed. The client bins `eventDate` per day either way
//! (`ledgerUtil.py:100-103`), so a per-event log shows the same window.
//! Falsified by a TQ ledger showing two rows for one day, system and type.
//!
//! **Retention.** The window shows [`WINDOW_DAYS`] days (`NUM_DAYS_BACK`,
//! `ledgerUtil.py:14` [V]); older rows are no use to anyone, so [`prune`]
//! (a rollover step) deletes them and [`logs`] never returns them. Bots
//! mine too, so the table is bounded by `characters x 90 x systems x types`
//! rather than by how long the server has run.

use crate::damage::BlueTime;
use crate::delta::Deltas;

/// `character day system type -> quantity wasted corporation`.
pub const KIND: &str = "mledger";

/// Days the Mining Ledger window shows **[V]** (`NUM_DAYS_BACK`).
pub const WINDOW_DAYS: i64 = 90;

/// Blue ticks in a day.
const DAY: i64 = crate::seed::DAY;

/// The day number of an instant.
pub fn day_of(at: BlueTime) -> i64 {
    at.div_euclid(DAY)
}

/// The instant the client is told for a day's row: its noon **[A]**, so
/// every time zone bins it on the same day. Falsified by a client in a far
/// time zone showing the row on the wrong date.
pub fn event_time(day: i64) -> BlueTime {
    day * DAY + DAY / 2
}

/// One row of the ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub character_id: i64,
    pub corporation_id: i64,
    pub day: i64,
    pub solar_system_id: i64,
    pub type_id: i64,
    pub quantity: i64,
    pub wasted: i64,
}

fn key(character: i64, day: i64, system: i64, type_id: i64) -> Vec<String> {
    vec![
        character.to_string(),
        day.to_string(),
        system.to_string(),
        type_id.to_string(),
    ]
}

fn row_of(k: &[String], v: &[String]) -> Option<Row> {
    let n = |s: &String| s.parse::<i64>().ok();
    Some(Row {
        character_id: n(k.first()?)?,
        day: n(k.get(1)?)?,
        solar_system_id: n(k.get(2)?)?,
        type_id: n(k.get(3)?)?,
        quantity: n(v.first()?)?,
        wasted: n(v.get(1)?)?,
        corporation_id: n(v.get(2)?)?,
    })
}

/// Add `quantity` (and `wasted`) mined by `character` at `at` to the day's
/// row. A cycle that mined nothing records nothing. The row keeps the
/// corporation of the **latest** mining, which is the one the corp tab
/// would count it for.
#[allow(clippy::too_many_arguments)]
pub fn record(
    d: &mut Deltas,
    character: i64,
    corp: i64,
    system: i64,
    type_id: i64,
    quantity: i64,
    wasted: i64,
    at: BlueTime,
) {
    if quantity <= 0 && wasted <= 0 {
        return;
    }
    let k = key(character, day_of(at), system, type_id);
    let (q0, w0) = d
        .of(KIND)
        .get(&k)
        .and_then(|v| {
            Some((
                v.first()?.parse::<i64>().ok()?,
                v.get(1)?.parse::<i64>().ok()?,
            ))
        })
        .unwrap_or((0, 0));
    d.put(
        KIND,
        k,
        vec![
            q0.saturating_add(quantity.max(0)).to_string(),
            w0.saturating_add(wasted.max(0)).to_string(),
            corp.to_string(),
        ],
    );
}

/// `character`'s rows inside the window at `now`, oldest day first.
pub fn logs(d: &Deltas, character: i64, now: BlueTime) -> Vec<Row> {
    let first = day_of(now) - WINDOW_DAYS;
    let id = character.to_string();
    let mut out: Vec<Row> = d
        .of(KIND)
        .range(vec![id.clone()]..)
        .take_while(|(k, _)| k.first() == Some(&id))
        .filter_map(|(k, v)| row_of(k, v))
        .filter(|r| r.day >= first)
        .collect();
    // Keys sort their days as text; the window wants them as numbers.
    out.sort_by_key(|r| (r.day, r.solar_system_id, r.type_id));
    out
}

/// Delete every row older than the window at `now`. Answers the rows
/// deleted (the rollover step's count).
pub fn prune(d: &mut Deltas, now: BlueTime) -> usize {
    let first = day_of(now) - WINDOW_DAYS;
    let old: Vec<Vec<String>> = d
        .of(KIND)
        .keys()
        .filter(|k| {
            k.get(1)
                .and_then(|s| s.parse::<i64>().ok())
                .is_none_or(|day| day < first)
        })
        .cloned()
        .collect();
    for k in &old {
        d.del(KIND, k);
    }
    old.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 133_000_000_000_000_000;
    const CHAR: i64 = 1_000_001;
    const SYS: i64 = 30_000_001;
    const VELD: i64 = 1230;
    const SCORD: i64 = 1228;

    #[test]
    fn two_cycles_of_one_ore_in_one_system_on_one_day_make_one_row() {
        let mut d = Deltas::default();
        record(&mut d, CHAR, 7, SYS, VELD, 5, 0, T0);
        record(&mut d, CHAR, 7, SYS, VELD, 6, 2, T0 + 600_000_000);
        let rows = logs(&d, CHAR, T0);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].quantity, rows[0].wasted), (11, 2));
        assert_eq!(rows[0].corporation_id, 7);
    }

    #[test]
    fn a_second_ore_type_or_system_makes_a_second_row_and_nothing_mined_none() {
        let mut d = Deltas::default();
        record(&mut d, CHAR, 7, SYS, VELD, 5, 0, T0);
        record(&mut d, CHAR, 7, SYS, SCORD, 5, 0, T0);
        record(&mut d, CHAR, 7, SYS + 1, VELD, 5, 0, T0);
        record(&mut d, CHAR, 7, SYS, VELD, 0, 0, T0);
        assert_eq!(logs(&d, CHAR, T0).len(), 3);
        // another pilot's rows are not ours
        record(&mut d, CHAR + 1, 7, SYS, VELD, 9, 0, T0);
        assert_eq!(logs(&d, CHAR, T0).len(), 3);
        assert_eq!(logs(&d, CHAR + 1, T0).len(), 1);
        // 1 is a prefix of 10 as text, never as a key
        record(&mut d, CHAR * 10, 7, SYS, VELD, 9, 0, T0);
        assert_eq!(logs(&d, CHAR, T0).len(), 3);
    }

    #[test]
    fn a_row_past_the_window_is_not_returned_and_prune_removes_it() {
        let mut d = Deltas::default();
        record(&mut d, CHAR, 7, SYS, VELD, 5, 0, T0);
        record(&mut d, CHAR, 7, SYS, SCORD, 5, 0, T0 - 91 * DAY);
        record(&mut d, CHAR, 7, SYS, 1224, 5, 0, T0 - 80 * DAY);
        let rows = logs(&d, CHAR, T0);
        assert_eq!(rows.len(), 2);
        assert!(rows.windows(2).all(|w| w[0].day <= w[1].day));
        assert_eq!(prune(&mut d, T0), 1);
        assert_eq!(d.of(KIND).len(), 2);
        assert_eq!(prune(&mut d, T0), 0);
    }

    #[test]
    fn the_table_is_bounded_however_long_the_server_runs() {
        let mut d = Deltas::default();
        for day in 0..400 {
            let at = T0 + day * DAY;
            record(&mut d, CHAR, 7, SYS, VELD, 1, 0, at);
            prune(&mut d, at);
        }
        assert!(d.of(KIND).len() as i64 <= WINDOW_DAYS + 1);
    }

    #[test]
    fn the_event_time_falls_on_its_own_day() {
        let at = event_time(day_of(T0));
        assert_eq!(day_of(at), day_of(T0));
    }

    /// A restart keeps the ledger: the rows go to the store file and come
    /// back, and the window still reads them.
    #[test]
    fn the_ledger_survives_a_restart() {
        let mut d = Deltas::default();
        record(&mut d, CHAR, 7, SYS, VELD, 5, 1, T0);
        record(&mut d, CHAR, 7, SYS, SCORD, 8, 0, T0 - 3 * DAY);
        let snap = crate::store::Snapshot {
            deltas: d.clone(),
            ..Default::default()
        };
        let dir = std::env::temp_dir().join(format!("eve-mledger-{}", std::process::id()));
        let store = crate::store::FileStore::new(dir.join("world.txt"));
        crate::store::Store::save(&store, &snap).expect("saves");
        let back = crate::store::Store::load(&store)
            .expect("loads")
            .expect("a world");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(back.deltas.of(KIND), d.of(KIND));
        let mut reg = crate::service::Registry::new(crate::model::SandboxConfig::default());
        reg.apply_snapshot(back);
        let rows = logs(reg.deltas(), CHAR, T0);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[1].quantity, rows[1].wasted, rows[1].corporation_id),
            (5, 1, 7)
        );
    }
}
