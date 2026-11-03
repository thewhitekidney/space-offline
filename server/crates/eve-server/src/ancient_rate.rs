//! **What the ancient cans really give** (lane T3-PRICE, packet T3-ANCIENT-RATE,
//! `docs/lanes/t3-supply.md` lane 5): pure instrumentation, no behaviour
//! change. A tally, per can kind (`Data`, `Relic`, `Ancient`), of
//!
//! - cans **reached** (a hacking game started on one; a second try on the same
//!   can counts again) and **won** (its loot rolled);
//! - the **items per can by class and type**: stacks and units rolled;
//! - the **salvage per type** (class `salvage`), the T3 kit's biggest need
//!   (19 types, about 124 of each per kit): the per-type spread may leave one
//!   type short long before the mean says done.
//!
//! It prints one `[ancient]` line at each rollover and, when
//! `EVE_ECON_TSV` is set, appends rows to the same file `econ_report` writes
//! (`kind` = `ancient_reached`, `ancient_won`, `ancient_item`,
//! `ancient_salvage`; `key` = kind or `kind/class/type`; `n` = cans or
//! stacks; `isk` holds the units). Counts are since the server started: the
//! tally is RAM only (like `hacking_loot::Loot`) and no delta kind is added.
//!
//! The hooks are two one-line calls: [`reached`] from `hacking::begin` and
//! [`won`] from `hacking_loot::on_won`.

use crate::hacking_loot::{Kind, Stack, Table};
use crate::service::Registry;
use std::collections::BTreeMap;

/// The registration row; the coordinator adds it to `world_ext::SLOTS`.
pub const SLOT: crate::world_ext::SlotSpec = crate::world_ext::SlotSpec {
    name: "ancient_rate",
    make: |_| Box::new(Tally::default()),
};

/// The class of loot that is salvage (`hacking_loot::TIER_QUANTITY_CLASSES`).
pub const SALVAGE_CLASS: &str = "salvage";
/// The class of an item a site's own loot row gave (no class in `site-loot.tsv`).
pub const SITE_CLASS: &str = "site";

/// `(kind, class, type)`.
type ItemKey = (String, String, i64);

/// The counters.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Tally {
    reached: BTreeMap<String, u64>,
    won: BTreeMap<String, u64>,
    /// `(stacks, units)` per item.
    items: BTreeMap<ItemKey, (u64, i64)>,
}

impl Tally {
    /// A game started on a can of `kind`.
    pub fn reach(&mut self, kind: Kind) {
        *self.reached.entry(format!("{kind:?}")).or_default() += 1;
    }

    /// A can of `kind` was won and rolled `items` as `(class, type, units)`.
    pub fn win(&mut self, kind: Kind, items: &[(String, i64, i64)]) {
        let k = format!("{kind:?}");
        *self.won.entry(k.clone()).or_default() += 1;
        for (class, type_id, units) in items {
            let e = self
                .items
                .entry((k.clone(), class.clone(), *type_id))
                .or_default();
            e.0 += 1;
            e.1 += units;
        }
    }

    /// The `[ancient]` line: reached and won per kind, then per kind and
    /// class the units per won can, then the salvage units by type.
    pub fn line(&self) -> String {
        let join = |m: &BTreeMap<String, u64>| {
            m.iter()
                .map(|(k, n)| format!("{k}={n}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        let mut s = format!(
            "[ancient] reached {{{}}} won {{{}}}",
            join(&self.reached),
            join(&self.won)
        );
        let mut classes: BTreeMap<(&str, &str), i64> = BTreeMap::new();
        for ((k, c, _), (_, units)) in &self.items {
            *classes.entry((k, c)).or_default() += units;
        }
        s.push_str(" | units per won can:");
        for ((k, c), units) in &classes {
            let cans = self.won.get(*k).copied().unwrap_or(1).max(1);
            s.push_str(&format!(" {k}/{c}={:.1}", *units as f64 / cans as f64));
        }
        s.push_str(" | salvage units by type:");
        for ((k, c, t), (_, units)) in &self.items {
            if c == SALVAGE_CLASS {
                s.push_str(&format!(" {k}/{t}={units}"));
            }
        }
        s
    }

    /// The TSV rows (no header) for `day`, in `econ_report::TSV_HEADER`'s
    /// columns.
    pub fn tsv_rows(&self, day: u32) -> Vec<String> {
        let mut v = Vec::new();
        for (k, n) in &self.reached {
            v.push(format!("{day}\tall\tancient_reached\t{k}\t{n}\t0\t0\t0\t0"));
        }
        for (k, n) in &self.won {
            v.push(format!("{day}\tall\tancient_won\t{k}\t{n}\t0\t0\t0\t0"));
        }
        for ((k, c, t), (stacks, units)) in &self.items {
            let kind = if c == SALVAGE_CLASS {
                "ancient_salvage"
            } else {
                "ancient_item"
            };
            v.push(format!(
                "{day}\tall\t{kind}\t{k}/{c}/{t}\t{stacks}\t0\t{units}\t0\t0"
            ));
        }
        v
    }
}

/// The class of `type_id` among the table's entries of `kind`, else
/// [`SITE_CLASS`] (a site-loot row).
pub fn class_of(table: &Table, kind: Kind, type_id: i64) -> String {
    table
        .entries
        .iter()
        .find(|e| e.kind == kind && e.type_id == type_id)
        .map_or_else(|| SITE_CLASS.to_owned(), |e| e.class.clone())
}

/// A hacking game started on a can of type `can_type` in `system`.
pub fn reached(w: &mut Registry, can_type: i64, system: i64) {
    let kind = crate::hacking_loot::can_kind(w, can_type, system);
    w.ext_mut::<Tally>().reach(kind);
}

/// A can of `kind` was won and rolled `stacks`.
pub fn won(w: &mut Registry, kind: Kind, stacks: &[Stack]) {
    let items: Vec<(String, i64, i64)> = match w.content().try_get::<Table>() {
        Some(t) => stacks
            .iter()
            .map(|x| (class_of(t, kind, x.type_id), x.type_id, x.quantity))
            .collect(),
        None => Vec::new(),
    };
    w.ext_mut::<Tally>().win(kind, &items);
}

/// The rollover step: print the line and append the rows to the
/// `EVE_ECON_TSV` file. After `econ_report`. Its count is the cans won.
pub fn on_rollover(reg: &mut Registry, d: &crate::rollover::Day) -> usize {
    let t = reg.ext::<Tally>().clone();
    if t.reached.is_empty() && t.won.is_empty() {
        return 0;
    }
    crate::outln!("{}", t.line());
    if let Some(path) = std::env::var_os(crate::econ_report::TSV_ENV) {
        crate::econ_report::append_tsv_queued(path.into(), &t.tsv_rows(d.day));
    }
    t.won.values().sum::<u64>() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids.
    const SALVAGE_A: i64 = 990_901;
    const SALVAGE_B: i64 = 990_902;
    const CORE: i64 = 990_903;

    fn item(class: &str, t: i64, n: i64) -> (String, i64, i64) {
        (class.to_owned(), t, n)
    }

    #[test]
    fn cans_reached_and_won_are_counted_per_kind() {
        let mut t = Tally::default();
        t.reach(Kind::Ancient);
        t.reach(Kind::Ancient);
        t.reach(Kind::Data);
        t.win(Kind::Ancient, &[]);
        assert_eq!(t.reached["Ancient"], 2);
        assert_eq!(t.reached["Data"], 1);
        assert_eq!(t.won["Ancient"], 1);
        assert!(
            t.line()
                .starts_with("[ancient] reached {Ancient=2,Data=1} won {Ancient=1}")
        );
    }

    #[test]
    fn items_and_salvage_are_kept_by_class_and_type() {
        let mut t = Tally::default();
        t.win(
            Kind::Ancient,
            &[item("salvage", SALVAGE_A, 8), item("datacore", CORE, 3)],
        );
        t.win(Kind::Ancient, &[item("salvage", SALVAGE_A, 4)]);
        t.win(Kind::Ancient, &[item("salvage", SALVAGE_B, 2)]);
        assert_eq!(
            t.items[&("Ancient".into(), "salvage".into(), SALVAGE_A)],
            (2, 12)
        );
        let line = t.line();
        assert!(line.contains(&format!("Ancient/{SALVAGE_A}=12")), "{line}");
        assert!(line.contains(&format!("Ancient/{SALVAGE_B}=2")), "{line}");
        // 3 cans won, 14 salvage units: 4.7 a can.
        assert!(line.contains("Ancient/salvage=4.7"), "{line}");
        assert!(!line.contains(&format!("Ancient/{CORE}=")), "{line}");
    }

    #[test]
    fn the_tsv_rows_use_the_report_columns() {
        let mut t = Tally::default();
        t.reach(Kind::Relic);
        t.win(Kind::Relic, &[item("salvage", SALVAGE_A, 5)]);
        let rows = t.tsv_rows(7);
        assert!(rows.contains(&"7\tall\tancient_reached\tRelic\t1\t0\t0\t0\t0".to_owned()));
        assert!(rows.contains(&"7\tall\tancient_won\tRelic\t1\t0\t0\t0\t0".to_owned()));
        assert!(rows.contains(&format!(
            "7\tall\tancient_salvage\tRelic/salvage/{SALVAGE_A}\t1\t0\t5\t0\t0"
        )));
        for r in rows {
            assert_eq!(
                r.split('\t').count(),
                crate::econ_report::TSV_HEADER.split('\t').count()
            );
        }
    }

    #[test]
    fn a_type_the_table_does_not_name_is_a_site_item() {
        let t = Table::fixture();
        assert_eq!(class_of(&t, Kind::Data, 990_601), "datacore");
        assert_eq!(class_of(&t, Kind::Data, 1), SITE_CLASS);
    }
}
