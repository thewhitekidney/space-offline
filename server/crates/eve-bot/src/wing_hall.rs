//! **Where is the FC?** The wing's shared book (plan-fc-follow §2.1, FC-1).
//!
//! A wing follows one human. Every wing bot needs to know his system and
//! station, and the only place the server tells a member that is the fleet
//! composition (`fleet.GetFleetComposition()`, FC-0 made it live). Asking
//! once per bot per turn would be a call per bot per 100 ms, so the wing
//! shares: **one bot per FC takes a lease, asks, and publishes** into this
//! hall; the rest read.
//!
//! - [`WingHall::take_lease`]: `true` for the one bot that should ask now
//!   (the book is older than [`FC_POLL`] and nobody else holds a fresh
//!   lease). A bot that took the lease and died is replaced after
//!   [`LEASE_TTL`].
//! - [`WingHall::publish`] / [`WingHall::release`]: the answer, or the miss.
//! - [`WingHall::row`]: a member's last published row, while it is no older
//!   than [`MAX_AGE`].
//! - [`parse_composition`]: the composition reply as [`Row`]s (pure).
//!
//! One hall per `eve-bots` process, like the gang and sov halls.

use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How often one bot per FC asks for the composition **[A]**. Falsified by a
/// wing that lags the FC by visibly more than a gate jump (shorten), or by
/// the server's `GetFleetComposition` showing up in a profile (lengthen).
pub const FC_POLL: Duration = Duration::from_secs(2);
/// How long a lease holds before another bot may take over **[A]**: a few
/// polls, longer than one call takes.
pub const LEASE_TTL: Duration = Duration::from_secs(8);
/// A published row older than this is not believed **[A]**: the poller may
/// be gone. Falsified by a wing that holds still while the FC moves.
pub const MAX_AGE: Duration = Duration::from_secs(10);

/// How long a bot's post is believed **[A]**: a few turns' worth, so a bot
/// that left stops being counted (and cannot stay the lead). Falsified by a
/// status line that counts a bot twice, or a wing with no lead for a moment.
pub const POST_TTL: Duration = Duration::from_secs(6);

/// Where a wing bot is, for the lead's status line (FC-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pos {
    /// In space on the FC's grid.
    Grid,
    Docked,
    /// Anywhere else: in space off his grid, or in transit.
    Away,
}

/// How many bots stand where, from the fresh posts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    pub grid: u32,
    pub docked: u32,
    pub away: u32,
}

/// One member's row of the composition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub char_id: i64,
    pub system: Option<i64>,
    pub station: Option<i64>,
    pub ship_type: Option<i64>,
}

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    }
}

/// **The composition reply as rows** (pure): a list of `KeyVal`s with
/// `characterID`, `solarSystemID`, `stationID` and `shipTypeID`. Anything
/// that is not a row with a character id is skipped.
pub fn parse_composition(v: &PyValue) -> Vec<Row> {
    let PyValue::List(items) = v else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let PyValue::Dict(entries) = crate::attributes(item) else {
                return None;
            };
            let get = |key: &str| {
                entries
                    .iter()
                    .find(|(_, k)| crate::text(k).as_deref() == Some(key))
                    .map(|(value, _)| value)
            };
            Some(Row {
                char_id: int(get("characterID"))?,
                system: int(get("solarSystemID")),
                station: int(get("stationID")),
                ship_type: int(get("shipTypeID")),
            })
        })
        .collect()
}

#[derive(Debug, Default)]
struct Book {
    rows: BTreeMap<i64, Row>,
    polled: Option<Instant>,
    lease: Option<(i64, Instant)>,
    /// FC-3: each wing bot's last post.
    posts: BTreeMap<i64, (Pos, Instant)>,
}

/// The books of the process, one per FC.
#[derive(Debug, Default)]
pub struct WingHall {
    books: Mutex<BTreeMap<i64, Book>>,
}

impl WingHall {
    pub fn new() -> WingHall {
        WingHall::default()
    }

    fn with<R>(&self, f: impl FnOnce(&mut BTreeMap<i64, Book>) -> R) -> R {
        f(&mut self.books.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// **Should `me` ask now?** `true` for one bot per FC per [`FC_POLL`]:
    /// the book is old enough and no other bot holds a lease younger than
    /// [`LEASE_TTL`]. The `true` answer takes the lease.
    pub fn take_lease(&self, fc: i64, me: i64, now: Instant) -> bool {
        self.with(|books| {
            let book = books.entry(fc).or_default();
            if book
                .polled
                .is_some_and(|t| now.saturating_duration_since(t) < FC_POLL)
            {
                return false;
            }
            if let Some((holder, at)) = book.lease
                && holder != me
                && now.saturating_duration_since(at) < LEASE_TTL
            {
                return false;
            }
            book.lease = Some((me, now));
            true
        })
    }

    /// The lease holder's answer: the rows, at `now`. Frees the lease.
    pub fn publish(&self, fc: i64, rows: Vec<Row>, now: Instant) {
        self.with(|books| {
            let book = books.entry(fc).or_default();
            book.rows = rows.into_iter().map(|r| (r.char_id, r)).collect();
            book.polled = Some(now);
            book.lease = None;
        });
    }

    /// The lease holder's miss (the call failed): free the lease and wait a
    /// poll before anyone asks again; the old rows age out on their own.
    pub fn release(&self, fc: i64, now: Instant) {
        self.with(|books| {
            let book = books.entry(fc).or_default();
            book.polled = Some(now);
            book.lease = None;
        });
    }

    /// FC-3: `me` says where it is (every turn; cheap).
    pub fn post(&self, fc: i64, me: i64, pos: Pos, now: Instant) {
        self.with(|books| {
            let book = books.entry(fc).or_default();
            book.posts.insert(me, (pos, now));
        });
    }

    /// FC-3: the **lead** of FC `fc`'s wing: the lowest character id still
    /// posting. It alone acknowledges a command and answers a status.
    pub fn lead(&self, fc: i64, now: Instant) -> Option<i64> {
        self.with(|books| {
            let book = books.get(&fc)?;
            book.posts
                .iter()
                .filter(|(_, (_, at))| now.saturating_duration_since(*at) < POST_TTL)
                .map(|(who, _)| *who)
                .min()
        })
    }

    /// FC-3: where the wing's bots are, from the fresh posts.
    pub fn tally(&self, fc: i64, now: Instant) -> Tally {
        self.with(|books| {
            let mut t = Tally::default();
            let Some(book) = books.get(&fc) else {
                return t;
            };
            for (pos, at) in book.posts.values() {
                if now.saturating_duration_since(*at) < POST_TTL {
                    match pos {
                        Pos::Grid => t.grid += 1,
                        Pos::Docked => t.docked += 1,
                        Pos::Away => t.away += 1,
                    }
                }
            }
            t
        })
    }

    /// `who`'s row in FC `fc`'s book, while the book is fresh.
    pub fn row(&self, fc: i64, who: i64, now: Instant) -> Option<Row> {
        self.with(|books| {
            let book = books.get(&fc)?;
            let fresh = book
                .polled
                .is_some_and(|t| now.saturating_duration_since(t) < MAX_AGE);
            fresh.then(|| book.rows.get(&who).copied()).flatten()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_server::pyutil::keyval;

    const FC: i64 = 1;
    const A: i64 = 2;
    const B: i64 = 3;

    fn entry(who: i64, system: Option<i64>, station: Option<i64>) -> PyValue {
        let opt = |v: Option<i64>| v.map_or(PyValue::None, PyValue::Int);
        keyval(vec![
            ("characterID", PyValue::Int(who)),
            ("solarSystemID", opt(system)),
            ("stationID", opt(station)),
            ("shipTypeID", PyValue::Int(5)),
            ("skills", PyValue::List(vec![])),
        ])
    }

    #[test]
    fn a_composition_reply_is_read_into_rows() {
        let reply = PyValue::List(vec![
            entry(FC, Some(10), None),
            entry(A, Some(11), Some(61)),
            PyValue::Int(3),
        ]);
        let rows = parse_composition(&reply);
        assert_eq!(rows.len(), 2, "a non-row is skipped: {rows:?}");
        assert_eq!(
            rows[0],
            Row {
                char_id: FC,
                system: Some(10),
                station: None,
                ship_type: Some(5)
            }
        );
        assert_eq!(rows[1].station, Some(61));
        assert!(parse_composition(&PyValue::None).is_empty());
    }

    #[test]
    fn one_bot_per_period_holds_the_lease_and_a_stale_lease_is_taken_over() {
        let hall = WingHall::new();
        let t0 = Instant::now();
        // The first asks; the others wait while its lease is fresh.
        assert!(hall.take_lease(FC, A, t0));
        assert!(!hall.take_lease(FC, B, t0 + Duration::from_millis(100)));
        // The holder may ask again if it never published.
        assert!(hall.take_lease(FC, A, t0 + Duration::from_millis(200)));
        // It published: nobody asks inside the period.
        hall.publish(FC, vec![], t0 + Duration::from_millis(300));
        assert!(!hall.take_lease(FC, B, t0 + Duration::from_secs(1)));
        // After the period, one bot (the first to ask) takes it.
        let later = t0 + FC_POLL + Duration::from_secs(1);
        assert!(hall.take_lease(FC, B, later));
        assert!(!hall.take_lease(FC, A, later + Duration::from_millis(10)));
        // B died holding it: after the TTL, A takes over.
        assert!(hall.take_lease(FC, A, later + LEASE_TTL + Duration::from_millis(1)));
        // Another FC has a book of its own.
        assert!(hall.take_lease(99, B, later));
    }

    #[test]
    fn the_lead_is_the_lowest_fresh_poster_and_the_tally_counts_fresh_posts() {
        let hall = WingHall::new();
        let t0 = Instant::now();
        assert_eq!(hall.lead(FC, t0), None);
        hall.post(FC, B, Pos::Grid, t0);
        hall.post(FC, A, Pos::Docked, t0);
        assert_eq!(hall.lead(FC, t0), Some(A));
        assert_eq!(
            hall.tally(FC, t0),
            Tally {
                grid: 1,
                docked: 1,
                away: 0
            }
        );
        // A stops posting: B leads once A's post is stale.
        let later = t0 + POST_TTL - Duration::from_secs(1);
        hall.post(FC, B, Pos::Away, later);
        assert_eq!(hall.lead(FC, later), Some(A));
        let stale = t0 + POST_TTL + Duration::from_secs(1);
        assert_eq!(hall.lead(FC, stale), Some(B));
        assert_eq!(hall.tally(FC, stale).away, 1);
        // Another FC's wing is its own.
        assert_eq!(hall.lead(99, t0), None);
    }

    #[test]
    fn a_row_is_believed_only_while_fresh() {
        let hall = WingHall::new();
        let t0 = Instant::now();
        assert_eq!(hall.row(FC, FC, t0), None, "nothing published yet");
        hall.publish(
            FC,
            vec![Row {
                char_id: FC,
                system: Some(10),
                station: None,
                ship_type: None,
            }],
            t0,
        );
        assert_eq!(
            hall.row(FC, FC, t0 + Duration::from_secs(1))
                .map(|r| r.system),
            Some(Some(10))
        );
        assert_eq!(hall.row(FC, A, t0), None, "he is not in the book");
        assert_eq!(
            hall.row(FC, FC, t0 + MAX_AGE + Duration::from_secs(1)),
            None
        );
        // A miss re-arms the period without erasing the rows.
        hall.release(FC, t0 + Duration::from_secs(5));
        assert!(!hall.take_lease(FC, A, t0 + Duration::from_secs(6)));
    }
}
