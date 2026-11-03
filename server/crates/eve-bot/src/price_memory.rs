//! **A trader's price memory** (FE-TRADE, R-FLUIDECON §3.6): what a bot has
//! *seen* of other regions' books, and its own slow anchor of each type.
//!
//! `marketProxy.GetOrders` answers for the region of the station the caller
//! is docked in, so a bot learns a region's book only by being there. Every
//! docked read is written here ([`PriceMemory::record`]): the best ask and
//! the best bid of a `(region, type)` with the station and the depth, and the
//! day. A remembered bid is **aged** when a trader plans on it
//! ([`PriceMemory::recall`]): its price falls by [`MEMORY_DECAY`] a day and
//! it is forgotten after [`MEMORY_MAX_DAYS`]. No omniscience: a region never
//! visited is not in the memory, and a trader confirms with a live read on
//! arrival (`trader::confirm`).
//!
//! The same file holds the **slow anchor** of each `(region, type)`, a
//! 20-day EWMA of the bot's perceived value `V` ([`PriceMemory::observe`]),
//! which the speculator buys below and sells above, and the speculator's
//! **holdings** ([`Hold`]) and cool-downs. [`Desk`] is the whole of it: one
//! per bot, in the bot's `Memory`, persisted in its state directory
//! (`<state>/<character>.desk.tsv`, temp-and-rename, a torn file reads as an
//! empty desk) so a restart keeps what the bot knew.
//!
//! **Every number is [A].**

use crate::speculate::SpecKnobs;
use crate::verbs::Book;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// How much a remembered price is discounted a day **[A]** (the spec's
/// `FE_MEMORY_DECAY`): a bid seen ten days ago is worth 60 % of what was
/// seen. Falsified by traders flying to a region whose live bid is far under
/// the memory more than a third of the time (raise), or never flying beyond
/// home though the tape shows a spread (lower).
pub const MEMORY_DECAY: f64 = 0.05;

/// A memory older than this many days is forgotten **[A]**. Falsified as
/// [`MEMORY_DECAY`] is.
pub const MEMORY_MAX_DAYS: i64 = 30;

/// Orders within this fraction of the best price, at the best station, make
/// up the depth a memory records **[A]**. Falsified by traders that plan
/// hold-fulls into a bid that was one unit deep.
pub const DEPTH_BAND: f64 = 0.02;

/// The slow anchor's span, days (alpha 1 / span) **[A]** (the spec's 20-day
/// EWMA). Falsified by speculators that never find a type under its anchor
/// (too short) or that buy a slide that goes on for weeks (too long).
pub const ANCHOR_DAYS: f64 = 20.0;

/// One side of a remembered book.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quote {
    pub price: f64,
    pub station: i64,
    /// Units on offer at that station within [`DEPTH_BAND`] of the price.
    pub units: i64,
}

/// What was seen of one `(region, type)` book, and when.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Seen {
    pub ask: Option<Quote>,
    pub bid: Option<Quote>,
    /// The day it was seen.
    pub day: i64,
}

/// A `(region, type)`'s slow anchor of `V`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    /// The 20-day EWMA over finished days (today's `V` is not in it yet).
    pub ewma: f64,
    /// The latest `V` and the one of the day before.
    pub v: f64,
    pub prev: f64,
    /// The day of `v`.
    pub day: i64,
    /// Days of observation.
    pub n: u32,
}

impl Anchor {
    /// `V` is not falling day on day.
    pub fn trend_up(&self) -> bool {
        self.v >= self.prev
    }
}

/// A speculative holding: what was bought for stock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hold {
    pub units: i64,
    /// ISK paid in all.
    pub cost: f64,
    /// The day it was bought.
    pub day: i64,
    /// The station whose hangar holds it.
    pub station: i64,
    pub region: i64,
}

/// The price memory: books seen and anchors.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PriceMemory {
    by: BTreeMap<(i64, i64), Seen>,
    anchors: BTreeMap<(i64, i64), Anchor>,
}

/// The best quote of one side of `orders`, with the depth at its station.
fn best_side(orders: &[crate::verbs::BookOrder], bid: bool) -> Option<Quote> {
    let live = || {
        orders
            .iter()
            .filter(|o| o.bid == bid && o.vol_remaining > 0 && o.price > 0.0)
    };
    let best = live().min_by(|a, b| {
        let (x, y) = if bid {
            (b.price, a.price)
        } else {
            (a.price, b.price)
        };
        x.total_cmp(&y).then(a.station_id.cmp(&b.station_id))
    })?;
    let band = best.price * DEPTH_BAND;
    let units = live()
        .filter(|o| o.station_id == best.station_id && (o.price - best.price).abs() <= band)
        .map(|o| o.vol_remaining)
        .sum();
    Some(Quote {
        price: best.price,
        station: best.station_id,
        units,
    })
}

impl PriceMemory {
    /// **Write down a docked read**: `book` is `region`'s book of `type_id`
    /// as `day` saw it. An empty book is remembered as empty (a region that
    /// stopped buying is news too).
    pub fn record(&mut self, region: i64, type_id: i64, book: &Book, day: i64) {
        if region == 0 {
            return;
        }
        // The dev seller's 100 ISK ask (`--dev`) is not a price to remember.
        let orders: Vec<_> = book
            .asks
            .iter()
            .chain(book.bids.iter())
            .filter(|o| !eve_server::econ_exchange::is_dev(o.order_id))
            .copied()
            .collect();
        self.by.insert(
            (region, type_id),
            Seen {
                ask: best_side(&orders, false),
                bid: best_side(&orders, true),
                day,
            },
        );
    }

    /// The raw memory of a `(region, type)`.
    pub fn seen(&self, region: i64, type_id: i64) -> Option<&Seen> {
        self.by.get(&(region, type_id))
    }

    /// **What a trader plans on** for `region`'s bid of `type_id` on `day`:
    /// the remembered best bid, its price aged by [`MEMORY_DECAY`] a day;
    /// `None` when nothing was seen, the region had no bid, or the memory is
    /// older than [`MEMORY_MAX_DAYS`].
    pub fn recall_bid(&self, region: i64, type_id: i64, day: i64) -> Option<Quote> {
        let seen = self.by.get(&(region, type_id))?;
        let age = (day - seen.day).max(0);
        if age > MEMORY_MAX_DAYS {
            return None;
        }
        let q = seen.bid?;
        Some(Quote {
            price: q.price * (1.0 - MEMORY_DECAY).powi(i32::try_from(age).unwrap_or(i32::MAX)),
            ..q
        })
    }

    /// Every region with a usable bid for `type_id`, other than `here`:
    /// `(region, aged bid)`, by region id.
    pub fn bids_elsewhere(&self, here: i64, type_id: i64, day: i64) -> Vec<(i64, Quote)> {
        self.by
            .keys()
            .filter(|(r, t)| *t == type_id && *r != here)
            .filter_map(|&(r, t)| self.recall_bid(r, t, day).map(|q| (r, q)))
            .collect()
    }

    /// **Fold one `V` into the anchor** of `(region, type)`: the first sight
    /// starts it, a later day moves the 20-day EWMA by `1 / ANCHOR_DAYS` of
    /// the gap from the *day before's* `V` (a day counts once, however many
    /// cycles read it), the same day only refreshes `v`. A non-positive `V`
    /// is ignored.
    pub fn observe(&mut self, region: i64, type_id: i64, v: f64, day: i64) {
        if region == 0 || v <= 0.0 || !v.is_finite() {
            return;
        }
        match self.anchors.get_mut(&(region, type_id)) {
            None => {
                self.anchors.insert(
                    (region, type_id),
                    Anchor {
                        ewma: v,
                        v,
                        prev: v,
                        day,
                        n: 1,
                    },
                );
            }
            Some(a) if day > a.day => {
                // Days with no read are carried at the last V.
                let gap = (day - a.day).min(ANCHOR_DAYS as i64);
                for _ in 0..gap {
                    a.ewma += (a.v - a.ewma) / ANCHOR_DAYS;
                }
                a.prev = a.v;
                a.v = v;
                a.day = day;
                a.n = a.n.saturating_add(1);
            }
            Some(a) => {
                if day == a.day {
                    a.v = v;
                }
            }
        }
    }

    /// The anchor of `(region, type)`.
    pub fn anchor(&self, region: i64, type_id: i64) -> Option<&Anchor> {
        self.anchors.get(&(region, type_id))
    }

    /// Forget memories past [`MEMORY_MAX_DAYS`] (so the file stays small).
    pub fn prune(&mut self, day: i64) {
        self.by.retain(|_, s| day - s.day <= MEMORY_MAX_DAYS);
        self.anchors
            .retain(|_, a| day - a.day <= 4 * MEMORY_MAX_DAYS);
    }

    /// How many `(region, type)` books are remembered.
    pub fn len(&self) -> usize {
        self.by.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by.is_empty()
    }
}

// ---- the desk --------------------------------------------------------------------

/// The file's format version, the `desk` row's field.
pub const VERSION: u32 = 1;

/// A trader's whole book-keeping: the memory, the speculator's holdings and
/// cool-downs, its knobs, and where it is saved.
#[derive(Debug, Clone, PartialEq)]
pub struct Desk {
    pub memory: PriceMemory,
    /// Holdings by type.
    pub holds: BTreeMap<i64, Hold>,
    /// A type sold out of a holding waits until this day to be bought again
    /// (a speculator does not buy its own listing back).
    pub cool: BTreeMap<i64, i64>,
    pub knobs: SpecKnobs,
    /// A game day in seconds ([`crate::driver::DAY_SECS`], shorter in a
    /// compressed soak).
    pub day_secs: u64,
    path: Option<PathBuf>,
    attached: bool,
    dirty: bool,
}

impl Default for Desk {
    fn default() -> Desk {
        Desk {
            memory: PriceMemory::default(),
            holds: BTreeMap::new(),
            cool: BTreeMap::new(),
            knobs: SpecKnobs::NORMAL,
            day_secs: 86_400,
            path: None,
            attached: false,
            dirty: false,
        }
    }
}

/// The file of `name` under `dir`.
pub fn path_for(dir: &Path, name: &str) -> PathBuf {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join(format!("{safe}.desk.tsv"))
}

impl Desk {
    /// The game day of Unix second `secs`.
    pub fn day_of(&self, secs: u64) -> i64 {
        i64::try_from(secs / self.day_secs.max(1)).unwrap_or(0)
    }

    /// Today by the wall clock.
    pub fn today(&self) -> i64 {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.day_of(secs)
    }

    /// **Attach the desk to its file once**: read `<dir>/<name>.desk.tsv` if
    /// there is one (a missing or torn file is an empty desk; the reason is
    /// returned), and keep the path for [`Desk::save`]. Later calls do
    /// nothing. `dir == None`: the desk lives for this run only.
    pub fn attach(&mut self, dir: Option<&Path>, name: &str, day_secs: u64) -> Option<String> {
        if self.attached {
            return None;
        }
        self.attached = true;
        self.day_secs = day_secs.max(1);
        let dir = dir?;
        let path = path_for(dir, name);
        let mut why = None;
        match std::fs::read_to_string(&path) {
            Ok(src) => match parse(&src) {
                Ok((memory, holds, cool)) => {
                    self.memory = memory;
                    self.holds = holds;
                    self.cool = cool;
                }
                Err(e) => why = Some(format!("{}: {e}; starting an empty desk", path.display())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => why = Some(format!("{}: {e}; starting an empty desk", path.display())),
        }
        self.path = Some(path);
        why
    }

    /// Mark the desk changed (a record, a hold).
    pub fn touch(&mut self) {
        self.dirty = true;
    }

    /// Write the file if it changed and there is a path; temp-and-rename.
    pub fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if !self.dirty {
            return Ok(());
        }
        let tmp = path.with_extension("tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(render(&self.memory, &self.holds, &self.cool).as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path)?;
        self.dirty = false;
        Ok(())
    }

    /// ISK held in speculative stock, at cost.
    pub fn held_cost(&self) -> f64 {
        self.holds.values().map(|h| h.cost).sum()
    }
}

fn q_out(q: &Option<Quote>) -> String {
    match q {
        Some(q) => format!("{}\t{}\t{}", q.price, q.station, q.units),
        None => "-\t-\t-".to_owned(),
    }
}

/// The desk's file text.
pub fn render(
    memory: &PriceMemory,
    holds: &BTreeMap<i64, Hold>,
    cool: &BTreeMap<i64, i64>,
) -> String {
    let mut rows = Vec::new();
    for (&(r, t), s) in &memory.by {
        rows.push(format!(
            "mem\t{r}\t{t}\t{}\t{}\t{}",
            s.day,
            q_out(&s.ask),
            q_out(&s.bid)
        ));
    }
    for (&(r, t), a) in &memory.anchors {
        rows.push(format!(
            "anchor\t{r}\t{t}\t{}\t{}\t{}\t{}\t{}",
            a.ewma, a.v, a.prev, a.day, a.n
        ));
    }
    for (&t, h) in holds {
        rows.push(format!(
            "hold\t{t}\t{}\t{}\t{}\t{}\t{}",
            h.units, h.cost, h.day, h.station, h.region
        ));
    }
    for (&t, &d) in cool {
        rows.push(format!("cool\t{t}\t{d}"));
    }
    let mut out = String::new();
    let _ = writeln!(out, "desk\t{VERSION}");
    for r in &rows {
        let _ = writeln!(out, "{r}");
    }
    let _ = writeln!(out, "end\t{}", rows.len());
    out
}

fn num<T: std::str::FromStr>(f: Option<&&str>, what: &str) -> Result<T, String> {
    f.ok_or_else(|| format!("missing {what}"))?
        .parse()
        .map_err(|_| format!("bad {what}"))
}

fn quote(f: &[&str]) -> Result<Option<Quote>, String> {
    if f.first() == Some(&"-") {
        return Ok(None);
    }
    Ok(Some(Quote {
        price: num(f.first(), "price")?,
        station: num(f.get(1), "station")?,
        units: num(f.get(2), "units")?,
    }))
}

/// The memory, holdings and cool-downs of a desk file; any row that does not
/// read, a missing `end` row or a count that does not match is an error (the
/// file is torn).
#[allow(clippy::type_complexity)]
pub fn parse(src: &str) -> Result<(PriceMemory, BTreeMap<i64, Hold>, BTreeMap<i64, i64>), String> {
    let mut lines = src.lines();
    let head: Vec<&str> = lines.next().ok_or("empty")?.split('\t').collect();
    if head.first() != Some(&"desk") || head.get(1) != Some(&"1") {
        return Err("not a desk file of this version".to_owned());
    }
    let mut mem = PriceMemory::default();
    let mut holds = BTreeMap::new();
    let mut cool = BTreeMap::new();
    let mut rows = 0usize;
    let mut ended = false;
    for line in lines {
        let f: Vec<&str> = line.split('\t').collect();
        match f[0] {
            "mem" if f.len() == 10 => {
                let (r, t): (i64, i64) = (num(f.get(1), "region")?, num(f.get(2), "type")?);
                mem.by.insert(
                    (r, t),
                    Seen {
                        day: num(f.get(3), "day")?,
                        ask: quote(&f[4..7])?,
                        bid: quote(&f[7..10])?,
                    },
                );
                rows += 1;
            }
            "anchor" if f.len() == 8 => {
                let (r, t): (i64, i64) = (num(f.get(1), "region")?, num(f.get(2), "type")?);
                mem.anchors.insert(
                    (r, t),
                    Anchor {
                        ewma: num(f.get(3), "ewma")?,
                        v: num(f.get(4), "v")?,
                        prev: num(f.get(5), "prev")?,
                        day: num(f.get(6), "day")?,
                        n: num(f.get(7), "n")?,
                    },
                );
                rows += 1;
            }
            "hold" if f.len() == 7 => {
                holds.insert(
                    num(f.get(1), "type")?,
                    Hold {
                        units: num(f.get(2), "units")?,
                        cost: num(f.get(3), "cost")?,
                        day: num(f.get(4), "day")?,
                        station: num(f.get(5), "station")?,
                        region: num(f.get(6), "region")?,
                    },
                );
                rows += 1;
            }
            "cool" if f.len() == 3 => {
                cool.insert(num(f.get(1), "type")?, num(f.get(2), "day")?);
                rows += 1;
            }
            "end" if f.len() == 2 => {
                let n: usize = num(f.get(1), "count")?;
                if n != rows {
                    return Err(format!("end says {n} rows, read {rows}"));
                }
                ended = true;
            }
            _ => return Err(format!("unreadable row {line:?}")),
        }
    }
    if !ended {
        return Err("no end row".to_owned());
    }
    Ok((mem, holds, cool))
}

#[cfg(test)]
mod tests {
    //! Invented ids and prices only.
    use super::*;
    use crate::verbs::BookOrder;

    const RA: i64 = 10_000_901;
    const RB: i64 = 10_000_902;
    const T: i64 = 990_800_001;
    const S1: i64 = 60_800_001;
    const S2: i64 = 60_800_002;

    fn order(station: i64, price: f64, units: i64, bid: bool) -> BookOrder {
        BookOrder {
            order_id: 1,
            type_id: T,
            station_id: station,
            price,
            vol_remaining: units,
            bid,
        }
    }

    fn book() -> Book {
        Book {
            asks: vec![order(S1, 120.0, 5, false), order(S2, 110.0, 7, false)],
            bids: vec![
                order(S1, 100.0, 10, true),
                order(S1, 99.5, 4, true),
                order(S1, 80.0, 99, true),
                order(S2, 90.0, 3, true),
            ],
        }
    }

    #[test]
    fn a_docked_read_is_remembered_with_station_and_depth() {
        let mut m = PriceMemory::default();
        m.record(RA, T, &book(), 3);
        let s = m.seen(RA, T).expect("seen");
        assert_eq!(s.day, 3);
        let ask = s.ask.expect("ask");
        assert_eq!((ask.price, ask.station, ask.units), (110.0, S2, 7));
        let bid = s.bid.expect("bid");
        // The 80 bid is not within the depth band of 100.
        assert_eq!((bid.price, bid.station, bid.units), (100.0, S1, 14));
        assert!(m.seen(RB, T).is_none(), "a region never visited is unknown");
    }

    #[test]
    fn a_memory_is_aged_and_then_forgotten() {
        let mut m = PriceMemory::default();
        m.record(RA, T, &book(), 0);
        assert_eq!(m.recall_bid(RA, T, 0).map(|q| q.price), Some(100.0));
        let aged = m.recall_bid(RA, T, 10).expect("aged").price;
        assert!(aged < 100.0 && aged > 0.0);
        assert!((aged - 100.0 * (1.0 - MEMORY_DECAY).powi(10)).abs() < 1e-9);
        assert!(m.recall_bid(RA, T, MEMORY_MAX_DAYS + 1).is_none());
    }

    #[test]
    fn an_empty_book_is_remembered_as_no_bid() {
        let mut m = PriceMemory::default();
        m.record(RA, T, &book(), 0);
        m.record(RA, T, &Book::default(), 1);
        assert!(m.recall_bid(RA, T, 1).is_none());
    }

    #[test]
    fn other_regions_exclude_here() {
        let mut m = PriceMemory::default();
        m.record(RA, T, &book(), 0);
        m.record(RB, T, &book(), 0);
        let there = m.bids_elsewhere(RA, T, 0);
        assert_eq!(there.iter().map(|(r, _)| *r).collect::<Vec<_>>(), vec![RB]);
    }

    #[test]
    fn the_anchor_counts_each_day_once_and_follows_slowly() {
        let mut m = PriceMemory::default();
        m.observe(RA, T, 100.0, 0);
        m.observe(RA, T, 100.0, 0);
        assert_eq!(m.anchor(RA, T).map(|a| a.n), Some(1));
        for d in 1..=5 {
            m.observe(RA, T, 50.0, d);
        }
        let a = m.anchor(RA, T).expect("anchor");
        assert_eq!(a.n, 6);
        // Five days at half the value moved a 20-day anchor only part way.
        assert!(a.ewma < 100.0 && a.ewma > 75.0, "{}", a.ewma);
        m.observe(RA, T, 0.0, 6);
        assert_eq!(m.anchor(RA, T).map(|a| a.n), Some(6), "a bad V is ignored");
    }

    #[test]
    fn the_desk_round_trips_and_a_torn_file_is_empty() {
        let mut d = Desk::default();
        d.memory.record(RA, T, &book(), 4);
        d.memory.observe(RA, T, 100.0, 4);
        d.holds.insert(
            T,
            Hold {
                units: 12,
                cost: 1234.5,
                day: 4,
                station: S1,
                region: RA,
            },
        );
        d.cool.insert(T + 1, 9);
        let text = render(&d.memory, &d.holds, &d.cool);
        let (m, h, c) = parse(&text).expect("reads back");
        assert_eq!(
            (m, h, c),
            (d.memory.clone(), d.holds.clone(), d.cool.clone())
        );
        let torn: String = text.lines().take(2).collect::<Vec<_>>().join("\n");
        assert!(parse(&torn).is_err());
        assert!(parse("garbage").is_err());
    }

    #[test]
    fn attach_reads_once_and_save_writes_atomically() {
        let dir = std::env::temp_dir().join(format!("desk-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let mut d = Desk::default();
        assert_eq!(d.attach(Some(&dir), "Invented Trader", 100), None);
        assert_eq!(d.day_secs, 100);
        d.memory.record(RA, T, &book(), 2);
        d.touch();
        d.save().expect("saved");
        let mut e = Desk::default();
        assert_eq!(e.attach(Some(&dir), "Invented Trader", 100), None);
        assert_eq!(e.memory, d.memory);
        // A second attach changes nothing.
        e.memory = PriceMemory::default();
        e.attach(Some(&dir), "Invented Trader", 100);
        assert!(e.memory.is_empty());
        std::fs::write(path_for(&dir, "Torn One"), "desk\t1\nmem\tx\n").expect("write");
        let mut t = Desk::default();
        assert!(t.attach(Some(&dir), "Torn One", 100).is_some());
        assert!(t.memory.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_day_follows_the_game_day_length() {
        let d = Desk {
            day_secs: 100,
            ..Desk::default()
        };
        assert_eq!(d.day_of(250), 2);
    }
}
