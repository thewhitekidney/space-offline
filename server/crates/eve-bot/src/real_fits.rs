//! **What players fit** (killmail fits): the table `tools/killfits.py`
//! writes to `out/bots/fits.tsv` from real killmails, read so a bot can seed
//! its fit from a fit people fly instead of the greedy one alone.
//!
//! The table is optional: absent, nothing changes ([`load`] gives `None`).
//! It is CCP game data and stays under `out/`; tests use invented rows.
//!
//! Format, tab-separated, `#` starts a comment line: `hull_type_id fit_hash
//! count share_of_hull hi med lo rig sub charges drones hull_name`, where a
//! slot column is comma-separated type ids (a module repeated per copy) and
//! `charges` is `weapon:charge` pairs. Only the first ten columns are read.
//!
//! **Nothing from nowhere**: [`RealFits::usable`] keeps a fit only when the
//! hull, every module and every charge pass the caller's `has` (the
//! catalog's `Makeable`), so a fit with a part no builder makes is never
//! offered; the caller still gates the fit on the paper evaluation
//! (`doctrine_fit::profile_of`, `Profile::ok`).

use std::collections::BTreeMap;
use std::path::Path;

/// One fit of one hull, as counted.
#[derive(Debug, Clone, PartialEq)]
pub struct RealFit {
    pub hull: i64,
    pub hash: String,
    /// Killmails with exactly this fit.
    pub count: u32,
    /// `count` over every killmail of the hull.
    pub share: f64,
    /// `(module type, copies)`, every rack together, ascending by type.
    pub modules: Vec<(i64, u8)>,
    /// `(weapon type, charge type)`: the charge most often loaded in it.
    pub charges: Vec<(i64, i64)>,
}

impl RealFit {
    /// The charge of the first weapon `is_gun` accepts.
    pub fn charge_for(&self, is_gun: impl Fn(i64) -> bool) -> Option<i64> {
        self.charges.iter().find(|(w, _)| is_gun(*w)).map(|c| c.1)
    }
}

/// Every fit, by hull, most common first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RealFits {
    pub by_hull: BTreeMap<i64, Vec<RealFit>>,
}

fn ids(col: &str) -> Result<Vec<i64>, String> {
    col.split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<i64>().map_err(|e| format!("type id {s:?}: {e}")))
        .collect()
}

/// Read the table's text. A malformed line is an error naming it.
pub fn parse(text: &str) -> Result<RealFits, String> {
    let mut out = RealFits::default();
    for (n, line) in text.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let row = (|| -> Result<RealFit, String> {
            let c: Vec<&str> = line.split('\t').collect();
            if c.len() < 10 {
                return Err(format!("{} columns, want at least 10", c.len()));
            }
            let num = |i: usize| c[i].parse::<f64>().map_err(|e| format!("column {i}: {e}"));
            let mut modules: BTreeMap<i64, u8> = BTreeMap::new();
            for col in &c[4..9] {
                for t in ids(col)? {
                    let e = modules.entry(t).or_insert(0);
                    *e = e.saturating_add(1);
                }
            }
            let mut charges = Vec::new();
            for pair in c[9].split(',').filter(|s| !s.is_empty()) {
                let (w, ch) = pair
                    .split_once(':')
                    .ok_or_else(|| format!("charge pair {pair:?}"))?;
                let w = w.parse::<i64>().map_err(|e| format!("weapon {w:?}: {e}"))?;
                let ch = ch
                    .parse::<i64>()
                    .map_err(|e| format!("charge {ch:?}: {e}"))?;
                charges.push((w, ch));
            }
            Ok(RealFit {
                hull: num(0)? as i64,
                hash: c[1].to_owned(),
                count: num(2)? as u32,
                share: num(3)?,
                modules: modules.into_iter().collect(),
                charges,
            })
        })()
        .map_err(|e| format!("line {}: {e}", n + 1))?;
        out.by_hull.entry(row.hull).or_default().push(row);
    }
    for fits in out.by_hull.values_mut() {
        fits.sort_by(|a, b| b.count.cmp(&a.count).then(a.hash.cmp(&b.hash)));
    }
    Ok(out)
}

/// The table at `path`; `Ok(None)` when there is no file (today's
/// behaviour), an error when it is there and unreadable.
pub fn load(path: &Path) -> Result<Option<RealFits>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

impl RealFits {
    /// The hull's fits that carry at least one module and whose hull,
    /// modules and charges all pass `has`, most common first.
    pub fn usable<'a>(
        &'a self,
        hull: i64,
        has: &'a dyn Fn(i64) -> bool,
    ) -> impl Iterator<Item = &'a RealFit> + 'a {
        self.by_hull
            .get(&hull)
            .into_iter()
            .flatten()
            .filter(move |f| {
                has(f.hull)
                    && !f.modules.is_empty()
                    && f.modules.iter().all(|m| has(m.0))
                    && f.charges.iter().all(|c| has(c.1))
            })
    }

    /// One usable fit drawn by count: `roll` in `[0, 1)` walks the
    /// cumulative counts, so a fit twice as common is drawn twice as often.
    /// `None` when the hull has no usable fit.
    pub fn pick<'a>(
        &'a self,
        hull: i64,
        has: &'a dyn Fn(i64) -> bool,
        roll: f64,
    ) -> Option<&'a RealFit> {
        let fits: Vec<&RealFit> = self.usable(hull, has).collect();
        let total: u64 = fits.iter().map(|f| u64::from(f.count)).sum();
        if total == 0 {
            return None;
        }
        let mut at = roll.clamp(0.0, 1.0) * total as f64;
        for f in &fits {
            at -= f64::from(f.count);
            if at < 0.0 {
                return Some(f);
            }
        }
        fits.last().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids: hull 9001, modules 100-104, charges 200-201.
    const TABLE: &str = "#hull_type_id\tfit_hash\tcount\tshare_of_hull\thi\tmed\tlo\trig\tsub\tcharges\tdrones\thull_name\n\
9001\taaa\t30\t0.3000\t100,100,100\t101\t102,102\t103\t\t100:200\t\tTest Hull\n\
9001\tbbb\t10\t0.1000\t100,100,100\t101\t104,102\t\t\t100:201\t\tTest Hull\n\
9001\tccc\t60\t0.6000\t\t\t\t\t\t\t\tTest Hull\n";

    #[test]
    fn parse_merges_racks_and_sorts_by_count() {
        let t = parse(TABLE).unwrap();
        let fits = &t.by_hull[&9001];
        assert_eq!(
            fits.iter().map(|f| f.hash.as_str()).collect::<Vec<_>>(),
            ["ccc", "aaa", "bbb"]
        );
        assert_eq!(
            fits[1].modules,
            vec![(100, 3), (101, 1), (102, 2), (103, 1)]
        );
        assert_eq!(fits[1].charges, vec![(100, 200)]);
        assert_eq!(fits[1].charge_for(|w| w == 100), Some(200));
        assert_eq!(fits[1].charge_for(|w| w == 101), None);
    }

    #[test]
    fn usable_drops_empty_fits_and_unmakeable_parts() {
        let t = parse(TABLE).unwrap();
        let all = |_: i64| true;
        let got: Vec<&str> = t.usable(9001, &all).map(|f| f.hash.as_str()).collect();
        assert_eq!(got, ["aaa", "bbb"]);
        let no_104 = |x: i64| x != 104;
        let got: Vec<&str> = t.usable(9001, &no_104).map(|f| f.hash.as_str()).collect();
        assert_eq!(got, ["aaa"]);
        let no_charge = |x: i64| x != 200;
        let got: Vec<&str> = t
            .usable(9001, &no_charge)
            .map(|f| f.hash.as_str())
            .collect();
        assert_eq!(got, ["bbb"]);
        let no_hull = |x: i64| x != 9001;
        assert_eq!(t.usable(9001, &no_hull).count(), 0);
        assert_eq!(t.usable(9002, &all).count(), 0);
    }

    #[test]
    fn pick_draws_by_count() {
        let t = parse(TABLE).unwrap();
        let all = |_: i64| true;
        // aaa 30 and bbb 10 of 40: rolls below 0.75 draw aaa.
        assert_eq!(t.pick(9001, &all, 0.0).unwrap().hash, "aaa");
        assert_eq!(t.pick(9001, &all, 0.74).unwrap().hash, "aaa");
        assert_eq!(t.pick(9001, &all, 0.76).unwrap().hash, "bbb");
        assert_eq!(t.pick(9001, &all, 1.0).unwrap().hash, "bbb");
        assert!(t.pick(9002, &all, 0.5).is_none());
    }

    #[test]
    fn a_bad_line_is_named_and_a_missing_file_is_none() {
        let e = parse("9001\tx\t1\n").unwrap_err();
        assert!(e.starts_with("line 1:"), "{e}");
        let e = parse("9001\tx\t1\t0.1\tq\t\t\t\t\t\n").unwrap_err();
        assert!(e.contains("type id"), "{e}");
        assert_eq!(load(Path::new("/nonexistent/probe/fits.tsv")), Ok(None));
    }
}
