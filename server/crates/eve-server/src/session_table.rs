//! Who is connected right now.
//!
//! **Ephemeral by design.** Nothing here is persisted: `docs/reference/world-state.md`
//! §2 lists the session and its derived attributes as state a restart must
//! rebuild rather than restore, and a stored "who is online" is a list of
//! people who are not.
//!
//! The distinction this module exists to keep is between **identity** and
//! **presence**. A character id names exactly one character, so
//! `Registry::character` answers an `Option`. A character may be logged in
//! more than once, so everything here answers a `Vec` or a count. EvEJS
//! models it the same way and for the same reason: `sessionsByCharacterID` is
//! a `characterID -> Set<session>` (`services/chat/sessionRegistry.js:21`)
//! and `findSessionByCharacterID` *chooses* among them (`:194-212`). An
//! `Option` there would silently drop one.

use std::collections::HashMap;

/// One live connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    /// The key. **Per connection**, not per account -- see
    /// [`crate::session::Session::client_id`]. Two connections of one account
    /// have two of these, which is the whole reason this table can hold them
    /// both.
    pub client_id: i64,
    pub session_id: i64,
    /// The account.
    pub user_id: i64,
    /// `None` between connect and `charUnboundMgr.SelectCharacterID`.
    pub character_id: Option<i64>,
}

/// Every live connection, by client id.
///
/// A `Vec` in connection order, plus two private indexes (PA-1) so the clock
/// drain's per-audience-member `of_character` is not a scan over every
/// session: `by_client` (client id -> position) and `by_character`
/// (character id -> positions, ascending, i.e. entry order -- the order the
/// scan returned). Every mutation goes through this impl; the cheap
/// mutations (`insert` of a new id, `set_character`) update both
/// incrementally and the rare ones (`remove`, replacing an entry) rebuild
/// them. A debug build checks them against the scan after every mutation
/// ([`SessionTable::check_index`]).
#[derive(Debug, Clone, Default)]
pub struct SessionTable {
    entries: Vec<SessionEntry>,
    by_client: HashMap<i64, usize>,
    by_character: HashMap<i64, Vec<usize>>,
}

impl SessionTable {
    /// On connect. Replaces any entry with the same client id rather than
    /// appending a second -- a duplicate client id is impossible by the
    /// minting formula, and if it ever happened, two entries for one
    /// connection is the worse failure.
    pub fn insert(&mut self, entry: SessionEntry) {
        match self.by_client.get(&entry.client_id) {
            Some(&i) => {
                self.entries[i] = entry;
                self.rebuild();
            }
            None => {
                let i = self.entries.len();
                self.by_client.insert(entry.client_id, i);
                if let Some(c) = entry.character_id {
                    self.by_character.entry(c).or_default().push(i);
                }
                self.entries.push(entry);
            }
        }
        self.check_index();
    }

    fn rebuild(&mut self) {
        self.by_client.clear();
        self.by_character.clear();
        for (i, e) in self.entries.iter().enumerate() {
            self.by_client.insert(e.client_id, i);
            if let Some(c) = e.character_id {
                self.by_character.entry(c).or_default().push(i);
            }
        }
    }

    /// Debug-only: both indexes equal what a scan finds.
    fn check_index(&self) {
        #[cfg(debug_assertions)]
        {
            let mut by_client = HashMap::new();
            let mut by_character: HashMap<i64, Vec<usize>> = HashMap::new();
            for (i, e) in self.entries.iter().enumerate() {
                by_client.insert(e.client_id, i);
                if let Some(c) = e.character_id {
                    by_character.entry(c).or_default().push(i);
                }
            }
            assert_eq!(self.by_client, by_client, "session by_client index");
            assert_eq!(
                self.by_character, by_character,
                "session by_character index"
            );
        }
    }

    /// On disconnect, from `server::serve_one`'s exit path -- explicitly, on
    /// every path, for the same reason and with the same discipline as
    /// `Wire::close`. A connection left in this table is a character the
    /// lobby says is standing there.
    pub fn remove(&mut self, client_id: i64) {
        if self.by_client.contains_key(&client_id) {
            self.entries.retain(|e| e.client_id != client_id);
            self.rebuild();
        }
        self.check_index();
    }

    /// The only writer of character identity onto an entry. Called from
    /// `dispatch::GpsSession::handle_select_character` and nowhere else, in
    /// the same breath as the write to `Session::character_id`: both, or
    /// neither.
    ///
    /// Silent if the client id is unknown. That is deliberate -- the
    /// alternative is inventing an entry for a connection the table never saw
    /// open, which would outlive the connection because nothing would ever
    /// remove it.
    pub fn set_character(&mut self, client_id: i64, character_id: i64) {
        if let Some(&i) = self.by_client.get(&client_id) {
            let old = self.entries[i].character_id.replace(character_id);
            if old != Some(character_id) {
                if let Some(o) = old
                    && let Some(v) = self.by_character.get_mut(&o)
                {
                    v.retain(|&p| p != i);
                    if v.is_empty() {
                        self.by_character.remove(&o);
                    }
                }
                let v = self.by_character.entry(character_id).or_default();
                let at = v.partition_point(|&p| p < i);
                v.insert(at, i);
            }
        }
        self.check_index();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }

    /// Every live session for this character.
    ///
    /// **`Vec`, not `Option`.** A duplicate login is a real state and the one
    /// this answer exists to describe; see the module docs.
    pub fn of_character(&self, character_id: i64) -> Vec<&SessionEntry> {
        self.by_character
            .get(&character_id)
            .map(|v| v.iter().map(|&i| &self.entries[i]).collect())
            .unwrap_or_default()
    }

    /// Every live session of this account (NET-TAKEOVER). A scan: one login
    /// at a time asks, and the table is the size of the online population.
    pub fn of_user(&self, user_id: i64) -> Vec<&SessionEntry> {
        self.entries
            .iter()
            .filter(|e| e.user_id == user_id)
            .collect()
    }

    pub fn is_online(&self, character_id: i64) -> bool {
        self.by_character.contains_key(&character_id)
    }

    /// Every character with a live session, **deduplicated**, in the order
    /// they first connected.
    ///
    /// A connection that has not selected a character is not in this list.
    /// That is the point: it has no character to be, and it is the first
    /// thing a naive guest list gets wrong.
    pub fn online_characters(&self) -> Vec<i64> {
        let mut out: Vec<i64> = Vec::new();
        for e in &self.entries {
            if let Some(id) = e.character_id
                && !out.contains(&id)
            {
                out.push(id);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;

    fn entry(client_id: i64) -> SessionEntry {
        SessionEntry {
            client_id,
            session_id: client_id,
            user_id: 1,
            character_id: None,
        }
    }

    /// The window between connect and `SelectCharacterID` is a real state
    /// and it is not a person standing in a station.
    #[test]
    fn a_session_with_no_character_is_not_online_as_anyone() {
        let mut t = SessionTable::default();
        t.insert(entry(10_000_000_001));
        assert_eq!(t.len(), 1, "the connection is tracked");
        assert!(t.online_characters().is_empty());
        assert!(!t.is_online(ALICE));
    }

    #[test]
    fn selecting_a_character_puts_them_online() {
        let mut t = SessionTable::default();
        t.insert(entry(10_000_000_001));
        t.set_character(10_000_000_001, ALICE);
        assert!(t.is_online(ALICE));
        assert_eq!(t.online_characters(), vec![ALICE]);
        assert_eq!(t.of_character(ALICE).len(), 1);
    }

    /// **Two sessions, one guest.** The client's docked guest panel appends
    /// every `OnCharNowInStation` without de-duping and its removal walks the
    /// live node list while mutating it, so it cannot clear a duplicate row
    /// afterwards (evejs `services/_shared/guestLists.js:62-74`, which
    /// records the same conclusion from the same client). So the de-dup has
    /// to happen here, and `of_character` still has to report both -- those
    /// are different questions.
    #[test]
    fn a_duplicate_login_is_two_sessions_and_one_guest() {
        let mut t = SessionTable::default();
        t.insert(entry(10_000_000_001));
        t.insert(entry(20_000_000_001));
        t.set_character(10_000_000_001, ALICE);
        t.set_character(20_000_000_001, ALICE);
        assert_eq!(t.of_character(ALICE).len(), 2, "both connections are real");
        assert_eq!(t.online_characters(), vec![ALICE], "but one guest");
    }

    #[test]
    fn two_characters_are_two_guests_in_connection_order() {
        let mut t = SessionTable::default();
        t.insert(entry(10_000_000_001));
        t.insert(entry(20_000_000_001));
        t.set_character(10_000_000_001, ALICE);
        t.set_character(20_000_000_001, BOB);
        assert_eq!(t.online_characters(), vec![ALICE, BOB]);
    }

    #[test]
    fn removing_a_session_takes_its_guest_with_it() {
        let mut t = SessionTable::default();
        t.insert(entry(10_000_000_001));
        t.set_character(10_000_000_001, ALICE);
        t.remove(10_000_000_001);
        assert!(t.is_empty());
        assert!(!t.is_online(ALICE), "a disconnected character is offline");
        assert!(t.online_characters().is_empty());
    }

    /// Only one of a character's two sessions ending leaves them online,
    /// which is the case a `remove` keyed on character id would get wrong.
    #[test]
    fn one_of_two_sessions_ending_leaves_the_character_online() {
        let mut t = SessionTable::default();
        t.insert(entry(10_000_000_001));
        t.insert(entry(20_000_000_001));
        t.set_character(10_000_000_001, ALICE);
        t.set_character(20_000_000_001, ALICE);
        t.remove(10_000_000_001);
        assert!(t.is_online(ALICE));
        assert_eq!(t.of_character(ALICE).len(), 1);
    }

    /// A `set_character` for a connection the table never saw must not
    /// invent one: nothing would ever remove it, so the lobby would show a
    /// character who is not there, forever.
    #[test]
    fn setting_a_character_on_an_unknown_connection_does_nothing() {
        let mut t = SessionTable::default();
        t.set_character(99, ALICE);
        assert!(t.is_empty());
        assert!(!t.is_online(ALICE));
    }

    #[test]
    fn inserting_the_same_connection_twice_is_one_entry() {
        let mut t = SessionTable::default();
        t.insert(entry(10_000_000_001));
        t.insert(entry(10_000_000_001));
        assert_eq!(t.len(), 1);
    }

    /// PA-1: after every mutation kind the indexes equal the scan
    /// (`check_index` asserts it inside each mutator in a debug build), and
    /// `of_character` keeps entry order.
    #[test]
    fn indexes_stay_in_step_through_every_mutation_kind() {
        let mut t = SessionTable::default();
        for k in 0..6 {
            t.insert(entry(100 + k));
        }
        // selected out of connection order
        t.set_character(104, ALICE);
        t.set_character(101, ALICE);
        t.set_character(103, BOB);
        let ids: Vec<i64> = t.of_character(ALICE).iter().map(|e| e.client_id).collect();
        assert_eq!(ids, vec![101, 104], "entry order, not selection order");
        // change character on a live session
        t.set_character(101, BOB);
        assert_eq!(t.of_character(ALICE).len(), 1);
        assert_eq!(t.of_character(BOB).len(), 2);
        // replace an entry (same client id, new character)
        t.insert(SessionEntry {
            character_id: Some(ALICE),
            ..entry(100)
        });
        assert_eq!(t.of_character(ALICE).len(), 2);
        // remove from the middle shifts positions
        t.remove(102);
        t.remove(9_999);
        let ids: Vec<i64> = t.of_character(ALICE).iter().map(|e| e.client_id).collect();
        assert_eq!(ids, vec![100, 104]);
        t.remove(100);
        t.remove(104);
        assert!(!t.is_online(ALICE));
        assert!(t.is_online(BOB));
        t.check_index();
    }

    /// PA-1 benchmark: 500 sessions, one lookup per session, indexed vs the
    /// scan it replaced. `cargo test --release -- --ignored session_bench --nocapture`.
    #[test]
    #[ignore]
    fn session_bench_of_character_500() {
        let mut t = SessionTable::default();
        for k in 0..500 {
            t.insert(entry(1_000 + k));
            t.set_character(1_000 + k, 5_000 + k);
        }
        let rounds = 400;
        let t0 = std::time::Instant::now();
        let mut n = 0;
        for _ in 0..rounds {
            for k in 0..500 {
                n += t.of_character(5_000 + k).len();
            }
        }
        let indexed = t0.elapsed();
        let t0 = std::time::Instant::now();
        let mut m = 0;
        for _ in 0..rounds {
            for k in 0..500 {
                m += t
                    .entries()
                    .iter()
                    .filter(|e| e.character_id == Some(5_000 + k))
                    .count();
            }
        }
        let scan = t0.elapsed();
        assert_eq!(n, m);
        crate::errln!(
            "of_character x{}: indexed {:?}, scan {:?} ({:.0}x)",
            rounds * 500,
            indexed,
            scan,
            scan.as_secs_f64() / indexed.as_secs_f64().max(1e-9)
        );
    }
}
