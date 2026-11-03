//! The world's changes against what the generators make: `world =
//! generate(seed) + deltas`.
//!
//! W10.5 seam S7(c), `docs/content/plan.md` §4.2. A `Deltas` map is
//! `kind -> (key fields -> value fields)`, all raw strings, so that
//! `store.rs` saves and loads every row **verbatim** and a kind no module
//! knows survives a round trip (CP5: the `delta` record).
//!
//! It also carries the one [`World`] record: the inputs `generate` takes
//! (seed, birth, day). It lives here rather than in its own `Registry` field
//! so that the world file's two C7 records travel together through
//! `to_snapshot` and `apply_snapshot`.
//!
//! **The map is the single truth.** A module reads `reg.deltas().of(kind)`
//! and writes through `put` and `del`; no module keeps a private copy. Keys
//! are client ids that stay stable, never row positions (ruling 4).

use std::collections::BTreeMap;

/// One kind's rows: key fields to value fields.
pub type Rows = BTreeMap<Vec<String>, Vec<String>>;

/// What [`Deltas::of`] answers for a kind with no rows.
static EMPTY: Rows = BTreeMap::new();

/// The `world` record: what every generator is a function of
/// (`docs/content/C7-liveness.md` §3.2, plan §4.2).
///
/// Times are FILETIMEs. `day` is advanced by the rollover (CP25), never
/// derived here; `gen_version` is the generators' version the deltas were
/// written against (the const is `population.rs`'s, CP21), so that bumping it
/// drops the generated kinds' deltas in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct World {
    pub seed: u64,
    pub birth: i64,
    pub day: u32,
    pub last_rollover: i64,
    pub gen_version: u32,
}

impl World {
    /// The world every [`crate::service::Registry::new`] starts with, so
    /// that `reg.world()` is never `None` (review CA6). A loaded or minted
    /// record replaces it before the server serves anyone (`server.rs`), so
    /// only tests and tools that build a registry by hand run on it.
    ///
    /// Invented values, none of them an [A]: a fixed seed, so a test's
    /// generated world is the same on every run, born at 2026-01-01 00:00 UTC
    /// with no rollover yet.
    pub const FIXTURE: World = World {
        seed: 0x5eed_0000_f1c5_7e57,
        birth: 134_116_992_000_000_000,
        day: 0,
        last_rollover: 134_116_992_000_000_000,
        gen_version: 0,
    };

    /// A new world born at `now`, with a seed from
    /// [`crate::seed::draw_world_seed`], the one seed draw (review CA5).
    ///
    /// `day` is 0 because `day_of` counts from the first 11:00 UTC after
    /// birth, which has not happened yet. `last_rollover` is the birth: there
    /// is nothing before it to catch up on [A] (falsified by a rollover step
    /// that treats "no rollover yet" differently from "one at birth").
    /// `gen_version` is 0, below any real generator version, so the first
    /// generator to load records its own [A] (falsified by a generator that
    /// treats 0 as a real version).
    pub fn mint(now: i64) -> World {
        World {
            seed: crate::seed::draw_world_seed(),
            birth: now,
            day: 0,
            last_rollover: now,
            gen_version: 0,
        }
    }
}

/// Every delta row, by kind, and the world record.
#[derive(Debug, Default, Clone)]
pub struct Deltas {
    kinds: BTreeMap<String, Rows>,
    world: Option<World>,
    /// A process-unique stamp of the last row change (0 = never changed),
    /// so a derived cache can tell a stale answer without comparing rows.
    /// Not part of equality.
    rev: u64,
    /// The same stamp per kind, so a cache of one kind's rows is not
    /// stale because an unrelated kind was written. Not part of equality.
    kind_rev: BTreeMap<String, u64>,
}

impl PartialEq for Deltas {
    fn eq(&self, other: &Self) -> bool {
        self.kinds == other.kinds && self.world == other.world
    }
}

/// Source of [`Deltas::revision`] stamps.
static NEXT_REV: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Deltas {
    /// Changes whenever any row is put or deleted (to a value no other
    /// `Deltas` state in this process has had), so `(revision)` keys a cache
    /// of anything computed from the rows.
    pub fn revision(&self) -> u64 {
        self.rev
    }

    /// [`Self::revision`] for one kind: changes only when a row of `kind`
    /// is put or deleted (0 = never written in this state).
    pub fn kind_revision(&self, kind: &str) -> u64 {
        self.kind_rev.get(kind).copied().unwrap_or(0)
    }

    fn touch(&mut self, kind: &str) {
        self.rev = NEXT_REV.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match self.kind_rev.get_mut(kind) {
            Some(r) => *r = self.rev,
            None => {
                self.kind_rev.insert(kind.to_owned(), self.rev);
            }
        }
    }

    /// The world record, `None` until one is loaded or minted.
    pub fn world(&self) -> Option<&World> {
        self.world.as_ref()
    }

    /// The world record, mutably (the rollover advances `day`).
    pub fn world_mut(&mut self) -> Option<&mut World> {
        self.world.as_mut()
    }

    /// Replace the world record.
    pub fn set_world(&mut self, world: World) {
        self.world = Some(world);
    }

    /// Every kind with at least one row, in kind order: what `store.rs`
    /// writes.
    pub fn kinds(&self) -> impl Iterator<Item = (&str, &Rows)> {
        self.kinds.iter().map(|(k, rows)| (k.as_str(), rows))
    }

    /// Every row of one kind, empty when there are none.
    pub fn of(&self, kind: &str) -> &Rows {
        self.kinds.get(kind).unwrap_or(&EMPTY)
    }

    /// Set one row, returning the value it replaced.
    ///
    /// # Panics
    ///
    /// In a debug build, when `kind` has no row in
    /// [`crate::delta_kinds::KINDS`]: a module writes only the kind it
    /// claimed there (review A6), so a typo or a second claim fails its
    /// first test instead of sharing rows in silence.
    pub fn put(&mut self, kind: &str, key: Vec<String>, value: Vec<String>) -> Option<Vec<String>> {
        debug_assert!(
            crate::delta_kinds::is_registered(kind),
            "delta kind {kind:?} is not in delta_kinds::KINDS"
        );
        self.put_verbatim(kind, key, value)
    }

    /// [`Self::put`] with no check on the kind: for `store.rs`'s load only,
    /// which keeps a kind this build does not know verbatim (CP5), so a file
    /// from a newer build loses nothing.
    pub fn put_verbatim(
        &mut self,
        kind: &str,
        key: Vec<String>,
        value: Vec<String>,
    ) -> Option<Vec<String>> {
        self.touch(kind);
        self.kinds
            .entry(kind.to_owned())
            .or_default()
            .insert(key, value)
    }

    /// Remove one row, returning its value. A kind left with no rows is
    /// dropped, so an empty kind and an absent one are the same state.
    pub fn del(&mut self, kind: &str, key: &[String]) -> Option<Vec<String>> {
        self.touch(kind);
        let rows = self.kinds.get_mut(kind)?;
        let old = rows.remove(key);
        if rows.is_empty() {
            self.kinds.remove(kind);
        }
        old
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(fields: &[&str]) -> Vec<String> {
        fields.iter().map(|f| (*f).to_owned()).collect()
    }

    /// PERF-HOLDS: a kind's revision moves only when a row of that kind is
    /// put or deleted.
    #[test]
    fn a_kind_revision_moves_only_with_its_own_rows() {
        let mut d = Deltas::default();
        assert_eq!(d.kind_revision("rock"), 0);
        d.put("rock", v(&["1"]), v(&["2"]));
        let rock = d.kind_revision("rock");
        assert_ne!(rock, 0);
        d.put("lp", v(&["1"]), v(&["2"]));
        assert_eq!(d.kind_revision("rock"), rock);
        d.put("rock", v(&["1"]), v(&["3"]));
        assert_ne!(d.kind_revision("rock"), rock);
        let rock = d.kind_revision("rock");
        d.del("lp", &v(&["1"]));
        assert_eq!(d.kind_revision("rock"), rock);
        d.del("rock", &v(&["1"]));
        assert_ne!(d.kind_revision("rock"), rock);
    }

    /// What goes in through `put` comes out of `of` unchanged, a second
    /// `put` replaces it, and `del` takes it out and leaves the kind as if
    /// it had never been written. Kinds do not see each other's rows.
    #[test]
    fn put_get_del_round_trip() {
        let mut d = Deltas::default();
        assert!(d.of("rock").is_empty());

        assert_eq!(d.put("rock", v(&["9001", "3"]), v(&["17", "1.5"])), None);
        assert_eq!(d.put("lp", v(&["9001", "3"]), v(&["40"])), None);
        assert_eq!(
            d.of("rock").get(&v(&["9001", "3"])),
            Some(&v(&["17", "1.5"]))
        );

        assert_eq!(
            d.put("rock", v(&["9001", "3"]), v(&["17", "0.5"])),
            Some(v(&["17", "1.5"]))
        );
        assert_eq!(d.of("rock").len(), 1);
        assert_eq!(d.of("lp").get(&v(&["9001", "3"])), Some(&v(&["40"])));

        assert_eq!(d.del("rock", &v(&["9001", "3"])), Some(v(&["17", "0.5"])));
        assert_eq!(d.del("rock", &v(&["9001", "3"])), None);
        assert!(d.of("rock").is_empty());
        assert_eq!(d.del("lp", &v(&["9001", "3"])), Some(v(&["40"])));
        assert_eq!(d, Deltas::default());
    }

    /// A module may write only a kind registered in `delta_kinds::KINDS`;
    /// the load path still keeps any kind.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "not in delta_kinds::KINDS")]
    fn put_of_an_unregistered_kind_panics_in_debug() {
        let mut d = Deltas::default();
        assert_eq!(d.put_verbatim("not-a-kind-yet", v(&["1"]), v(&["2"])), None);
        assert_eq!(d.of("not-a-kind-yet").len(), 1);
        d.put("not-a-kind-yet", v(&["1"]), v(&["3"]));
    }
}
