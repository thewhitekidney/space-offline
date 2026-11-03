//! **The chat directory** (PA-11): what chat needs from the world, published
//! by the world, so no chat thread ever takes the world lock.
//!
//! Chat used to read the world for the reconcile pass, the login's presence,
//! the auth poll and every `eve_user_data` iq; behind the clock's write lock
//! that wait reached 11 s in a live run. Now the world side, which already
//! holds the lock, builds a [`ChatDirectory`] and swaps it into the hub's
//! [`Published`]; chat reads the swapped-in `Arc`.
//!
//! ## Who publishes
//!
//! - [`tick`], the `chat_dir` stage: a full [`build`] every
//!   [`crate::chat::RECONCILE_EVERY`] (jumps, corp, alliance and fleet
//!   changes are not edges, so they arrive here);
//! - [`on_login`] / [`on_logoff`], the `session_hooks` rows: one character,
//!   at the first session's selection and the last one's close;
//! - [`refresh`], the `ResyncSystemChannelAccess` arm, before it touches the
//!   hub;
//! - `Registry::set_chat`, so a world given a hub always has a directory.
//!
//! ## Lock order
//!
//! world -> hub; world -> directory (write); hub -> directory (read). The
//! directory lock is a **leaf**: nothing is acquired while holding it, and
//! readers clone the `Arc` and drop it. **Every world-side change to a
//! character's membership updates the directory before it touches the hub,
//! and the reconcile pass reads the directory while holding the hub**, so an
//! older directory can never undo a newer `sync_character` (a stale entry
//! equals what the hub already holds, and the world thread then applies the
//! new one).

use crate::chat::{Membership, UserData, membership, user_data};
use crate::damage::BlueTime;
use crate::service::Registry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

/// What chat needs from the world, published by the world.
#[derive(Debug, Default)]
pub struct ChatDirectory {
    /// Every online character's derived membership, keyed by id.
    pub members: BTreeMap<i64, Arc<Membership>>,
    /// `eve_user_data` for **every** character the world knows, online or
    /// not: the iq answers for any jid. Shared between incremental publishes;
    /// replaced only by a full build.
    pub users: Arc<HashMap<i64, UserData>>,
    /// Bumped by every publish. The reconcile thread waits on it.
    pub generation: u64,
}

impl ChatDirectory {
    pub fn is_online(&self, c: i64) -> bool {
        self.members.contains_key(&c)
    }

    pub fn member(&self, c: i64) -> Option<Membership> {
        self.members.get(&c).map(|m| (**m).clone())
    }

    /// Same answer as [`user_data`] on the world it was built from.
    pub fn user_data(&self, c: i64) -> UserData {
        self.users.get(&c).copied().unwrap_or_default()
    }
}

/// The hub's copy: a leaf lock. Nothing is ever taken while holding it.
pub struct Published {
    cur: RwLock<Arc<ChatDirectory>>,
    gen_: Mutex<u64>,
    bumped: Condvar,
    /// When the last full build ran (the stage's cadence).
    built: Mutex<Option<Instant>>,
}

impl Default for Published {
    fn default() -> Self {
        Published {
            cur: RwLock::new(Arc::new(ChatDirectory::default())),
            gen_: Mutex::new(0),
            bumped: Condvar::new(),
            built: Mutex::new(None),
        }
    }
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Published {
    /// The latest directory. A read lock, an `Arc` clone, and the lock drops.
    pub fn current(&self) -> Arc<ChatDirectory> {
        let g = self.cur.read().unwrap_or_else(|e| e.into_inner());
        Arc::clone(&g)
    }

    fn swap(&self, mut d: ChatDirectory) {
        let mut gen_ = locked(&self.gen_);
        *gen_ += 1;
        d.generation = *gen_;
        {
            let mut w = self.cur.write().unwrap_or_else(|e| e.into_inner());
            *w = Arc::new(d);
        }
        drop(gen_);
        self.bumped.notify_all();
    }

    /// Block until a publish newer than `seen` or `timeout`; returns the
    /// generation now current.
    pub fn wait_newer(&self, seen: u64, timeout: Duration) -> u64 {
        let g = locked(&self.gen_);
        let (g, _) = self
            .bumped
            .wait_timeout_while(g, timeout, |now| *now <= seen)
            .unwrap_or_else(|e| e.into_inner());
        *g
    }
}

/// Full build from the world. O(online + characters). Does not use
/// `SessionTable::online_characters`, which is quadratic.
pub fn build(reg: &Registry) -> ChatDirectory {
    let mut seen: HashSet<i64> = HashSet::new();
    let mut members = BTreeMap::new();
    for e in reg.sessions().entries() {
        if let Some(c) = e.character_id
            && seen.insert(c)
            && let Some(m) = membership(reg, c)
        {
            members.insert(c, Arc::new(m));
        }
    }
    let users = reg
        .config()
        .characters
        .iter()
        .map(|c| (c.character_id, user_data(reg, c.character_id)))
        .collect();
    ChatDirectory {
        members,
        users: Arc::new(users),
        generation: 0,
    }
}

/// Build and publish now (no-op without a hub).
pub fn publish(reg: &Registry) {
    publish_at(reg, Instant::now());
}

fn publish_at(reg: &Registry, now: Instant) {
    let Some(hub) = reg.chat() else { return };
    let p = hub.directory();
    p.swap(build(reg));
    *locked(&p.built) = Some(now);
}

/// Move the player channels changed over XMPP, which cannot write a delta
/// itself, into the world's deltas. Shutdown calls it too, so a change after
/// the last tick is saved.
pub fn drain_dirty(reg: &mut Registry) {
    let Some(hub) = reg.chat().cloned() else {
        return;
    };
    for (key, row) in hub.take_dirty() {
        match row {
            Some(row) => {
                reg.deltas_mut().put(crate::chat::KIND, key, row);
            }
            None => {
                reg.deltas_mut().del(crate::chat::KIND, &key);
            }
        }
    }
}

/// The tick stage: a full publish when `RECONCILE_EVERY` has passed since the
/// last one.
pub fn tick(reg: &mut Registry, now: Instant, _ft: BlueTime) {
    let Some(hub) = reg.chat().cloned() else {
        return;
    };
    drain_dirty(reg);
    let due = locked(&hub.directory().built)
        .is_none_or(|t| now.saturating_duration_since(t) >= crate::chat::RECONCILE_EVERY);
    if due {
        publish_at(reg, now);
    }
}

/// Replace (or remove) one character's entry and publish; `users` is shared.
fn update(reg: &Registry, c: i64, now: Option<Membership>) {
    let Some(hub) = reg.chat() else { return };
    let p = hub.directory();
    let cur = p.current();
    let mut members = cur.members.clone();
    match now {
        Some(m) => members.insert(c, Arc::new(m)),
        None => members.remove(&c),
    };
    p.swap(ChatDirectory {
        members,
        users: Arc::clone(&cur.users),
        generation: 0,
    });
}

/// `session_hooks::ON_LOGIN` row.
pub fn on_login(reg: &mut Registry, c: i64) {
    refresh(reg, c);
}

/// `session_hooks::ON_LOGOFF` row (last).
pub fn on_logoff(reg: &mut Registry, c: i64) {
    update(reg, c, None);
}

/// Incremental refresh of one character from the live world.
pub fn refresh(reg: &Registry, c: i64) {
    if reg.chat().is_some() {
        update(reg, c, membership(reg, c));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{ChatConfig, ChatHub, derive_all};
    use crate::model::{Character, SandboxConfig};
    use crate::session_table::SessionEntry;

    const SYS1: i64 = 30_000_001;
    const SYS2: i64 = 30_000_003;

    fn pilot(id: i64, corp: i64, alliance: Option<i64>, ss: i64) -> Character {
        Character {
            character_id: id,
            ship_id: 100_000_000 + id % 1_000_000,
            corporation_id: corp,
            alliance_id: alliance,
            solar_system_id: Some(ss),
            ..Character::default()
        }
    }

    fn login(reg: &mut Registry, client: i64, c: i64) {
        reg.session_opened(SessionEntry {
            client_id: client,
            session_id: client,
            user_id: 1,
            character_id: None,
        });
        reg.session_selected_character(client, c);
    }

    fn fixture() -> Registry {
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                pilot(90_000_001, 98_000_001, Some(99_000_001), SYS1),
                pilot(90_000_002, 98_000_001, Some(99_000_001), SYS1),
                pilot(90_000_003, 98_000_002, None, SYS2),
                pilot(90_000_004, 98_000_002, None, SYS2),
                pilot(90_000_005, 98_000_003, None, SYS1),
            ],
            ..SandboxConfig::default()
        });
        reg.set_chat(ChatHub::new(ChatConfig::default()));
        login(&mut reg, 10, 90_000_001);
        login(&mut reg, 11, 90_000_002);
        login(&mut reg, 12, 90_000_003);
        // The same character in a second window: the dedupe.
        login(&mut reg, 13, 90_000_001);
        login(&mut reg, 14, 90_000_004);
        reg
    }

    #[test]
    fn the_directory_matches_the_live_derivation() {
        let reg = fixture();
        let dir = build(&reg);
        let live = derive_all(&reg);
        let got: BTreeMap<i64, Membership> = dir
            .members
            .iter()
            .map(|(c, m)| (*c, (**m).clone()))
            .collect();
        assert_eq!(got, live);
        assert_eq!(dir.members.len(), 4, "one character, two windows: once");
        for c in reg.config().characters.iter().map(|c| c.character_id) {
            assert_eq!(dir.user_data(c), user_data(&reg, c), "user data of {c}");
        }
        assert_eq!(dir.user_data(1), UserData::default(), "unknown id");
        assert!(!dir.is_online(90_000_005), "the offline pilot");
    }

    #[test]
    fn login_and_logoff_publish_without_a_tick() {
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![pilot(90_000_001, 98_000_001, None, SYS1)],
            ..SandboxConfig::default()
        });
        reg.set_chat(ChatHub::new(ChatConfig::default()));
        let hub = Arc::clone(reg.chat().unwrap());
        let online = |h: &ChatHub| h.directory().current().is_online(90_000_001);
        assert!(!online(&hub));
        login(&mut reg, 10, 90_000_001);
        assert!(online(&hub), "the login edge publishes");
        login(&mut reg, 11, 90_000_001);
        reg.session_closed(10);
        assert!(online(&hub), "closing one of two windows is not an edge");
        reg.session_closed(11);
        assert!(!online(&hub), "the last window closing publishes");
    }

    #[test]
    fn the_stage_publishes_on_its_cadence_only() {
        let mut reg = fixture();
        let hub = Arc::clone(reg.chat().unwrap());
        let g0 = hub.directory().current().generation;
        let t = Instant::now();
        // `set_chat` published just now, so the stage is not yet due.
        tick(&mut reg, t, BlueTime::default());
        assert_eq!(hub.directory().current().generation, g0, "not due");
        tick(
            &mut reg,
            t + Duration::from_millis(1100),
            BlueTime::default(),
        );
        let g1 = hub.directory().current().generation;
        assert!(g1 > g0, "due builds");
        tick(
            &mut reg,
            t + Duration::from_millis(1300),
            BlueTime::default(),
        );
        assert_eq!(hub.directory().current().generation, g1, "not due yet");
        tick(
            &mut reg,
            t + Duration::from_millis(2300),
            BlueTime::default(),
        );
        assert!(hub.directory().current().generation > g1, "due again");
    }

    #[test]
    fn wait_newer_wakes_on_a_swap_and_times_out_without_one() {
        let p = Arc::new(Published::default());
        assert_eq!(p.wait_newer(0, Duration::from_millis(30)), 0);
        let q = Arc::clone(&p);
        let h = std::thread::spawn(move || q.wait_newer(0, Duration::from_secs(5)));
        std::thread::sleep(Duration::from_millis(50));
        p.swap(ChatDirectory::default());
        assert_eq!(h.join().unwrap(), 1);
    }

    #[test]
    fn a_world_without_chat_publishes_nothing() {
        let mut reg = Registry::new(SandboxConfig::default());
        publish(&reg);
        on_login(&mut reg, 1);
        on_logoff(&mut reg, 1);
        tick(&mut reg, Instant::now(), BlueTime::default());
        refresh(&reg, 1);
    }

    /// `cargo test --release -p eve-server chat_dir_bench -- --ignored --nocapture`
    #[test]
    #[ignore = "a timing, not a gate"]
    fn chat_dir_bench() {
        for online in [500usize, 1000] {
            let mut characters: Vec<Character> = (0..online + 2000)
                .map(|i| {
                    pilot(
                        91_000_000 + i as i64,
                        98_000_000 + (i as i64 % 40),
                        (i % 3 == 0).then_some(99_000_000 + (i as i64 % 5)),
                        SYS1 + (i as i64 % 3),
                    )
                })
                .collect();
            for c in &mut characters {
                c.ship_id = 190_000_000 + c.character_id % 1_000_000;
            }
            let mut reg = Registry::new(SandboxConfig {
                characters,
                ..SandboxConfig::default()
            });
            reg.set_chat(ChatHub::new(ChatConfig::default()));
            for i in 0..online {
                login(&mut reg, 500_000_000 + i as i64, 91_000_000 + i as i64);
            }
            let n = 20;
            let t = Instant::now();
            for _ in 0..n {
                std::hint::black_box(build(&reg));
            }
            let full = t.elapsed() / n;
            let t = Instant::now();
            for _ in 0..n {
                refresh(&reg, 91_000_007);
            }
            let inc = t.elapsed() / n;
            println!(
                "chat_dir_bench online={online} characters={} build={full:?} incremental={inc:?}",
                online + 2000
            );
        }
    }
}
