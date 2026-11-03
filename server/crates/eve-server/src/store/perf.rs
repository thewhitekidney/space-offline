//! PERF-SAVE: how long a world save holds the world lock, at the size of
//! the stage-B soak (50 characters with full hangars, a full killmail log,
//! contacts, alliances, sov rows, a market and industry jobs).
//!
//! Release only, explicit only:
//! `tools/dev.sh cargo test --release -p eve-server perf_save -- --ignored --nocapture`
//!
//! Every id and row is invented (no client data): the sizes are what count.

use super::*;
use std::fmt::Write as _;

pub(super) const CHARS: i64 = 50;
pub(super) const ITEMS_PER_CHAR: i64 = 600;
pub(super) const JOURNAL_PER_CHAR: i64 = 500;
pub(super) const JOBS_PER_CHAR: i64 = 40;
pub(super) const KILLMAILS: i64 = 3_000;
pub(super) const CONTACTS_PER_CHAR: i64 = 100;
pub(super) const OPEN_ORDERS: i64 = 3_000;
pub(super) const CLOSED_ORDERS: i64 = 6_000;
pub(super) const TRADES: i64 = 20_000;
pub(super) const ALLIANCES: i64 = 20;
pub(super) const SOV_ROWS: i64 = 500;

/// The upper bound the lock hold must stay under at this size, in release
/// [A]: a clone of the world is memory bandwidth, a few hundred ms at most;
/// falsified by the number this test prints.
pub(super) const LOCK_HOLD_BOUND: Duration = Duration::from_millis(500);

/// A world of the soak's size, as a loaded `Snapshot`.
pub(super) fn big_snapshot() -> Snapshot {
    let mut s = Snapshot::default();
    for c in 0..CHARS {
        let id = 90_000_000 + c;
        s.characters.push(CharacterRecord {
            character_id: id,
            ship_id: 100_000_000 + c,
            owner_user_id: None,
            name: format!("Bot {c}"),
            station: None,
            identity: None,
        });
        s.balances.push((id, 1_000_000.5 + c as f64));
        s.skills.push((
            id,
            (0..60)
                .map(|k| SkillEntry {
                    type_id: 3300 + k,
                    level: 3,
                    points: 1415 + k,
                    rank: 1,
                })
                .collect(),
        ));
        for k in 0..ITEMS_PER_CHAR {
            s.items.push(Item {
                item_id: 200_000_000 + c * 10_000 + k,
                type_id: 34 + k % 500,
                owner_id: id,
                location_id: 60_000_000 + c % 10,
                flag_id: 4,
                quantity: 1000 + k,
                group_id: 18,
                category_id: 4,
                singleton: false,
            });
        }
        for k in 0..JOURNAL_PER_CHAR {
            s.journal.push(Entry {
                transaction_id: c * 10_000 + k,
                date: 133_000_000_000_000_000 + k,
                entry_type_id: 10,
                owner_id_1: id,
                owner_id_2: 1_000_000 + k,
                reference_id: k,
                amount: 12_345.67,
                balance: 1_000_000.5,
                description: "invented market sale".to_owned(),
            });
        }
        for k in 0..JOBS_PER_CHAR {
            s.jobs.push(Job {
                job_id: c * 1_000 + k,
                activity_id: 1,
                blueprint_id: 300_000_000 + k,
                blueprint_type_id: 21_858,
                blueprint_location_id: 60_000_000,
                blueprint_location_flag_id: 4,
                blueprint_copy: false,
                facility_id: 60_000_000,
                owner_id: id,
                status: crate::industry::Status::Installed,
                installer_id: id,
                completed_character_id: None,
                solar_system_id: 30_000_001,
                station_id: 60_000_000,
                start_date: 133_000_000_000_000_000,
                end_date: 133_000_003_000_000_000,
                pause_date: None,
                runs: 2,
                licensed_runs: 10,
                successful_runs: 2,
                cost: 4_321,
                time_in_seconds: 600,
                probability: 1.0,
                product_type_id: 21_857,
                optional_type_id: None,
                optional_type_id2: None,
                output_location_id: 60_000_000,
                output_flag_id: 4,
            });
        }
    }
    s.next_item_id = 900_000_000;
    s.next_transaction_id = 900_000;
    s.next_job_id = 900_000;
    let mut d = Deltas::default();
    d.set_world(World {
        seed: 0x0123_4567_89ab_cdef,
        birth: 133_000_000_000_000_000,
        day: 3,
        last_rollover: 133_000_002_000_000_000,
        gen_version: 7,
    });
    let row = |f: &[String]| f.to_vec();
    for k in 0..KILLMAILS {
        // Twelve fields; the attackers and the items are the bulk, a few
        // hundred bytes each in a real report.
        let attackers = (0..6)
            .map(|a| {
                format!(
                    "{}:{}:{}:{}:{}",
                    90_000_000 + a,
                    600 + a,
                    1_000 + a,
                    12_000 + a,
                    99
                )
            })
            .collect::<Vec<_>>()
            .join("|");
        let items = (0..40)
            .map(|i| format!("{}:{}:{}:{}", 34 + i, 5, 100 + i, i % 2))
            .collect::<Vec<_>>()
            .join("|");
        d.put_verbatim(
            "killmail",
            vec![k.to_string()],
            row(&[
                "133000000000000000".into(),
                "30000001".into(),
                "90000001".into(),
                "98000001".into(),
                "-".into(),
                "587".into(),
                "1234.5".into(),
                "1000000".into(),
                "500000".into(),
                "90000002:600:1000:12000:99".into(),
                attackers,
                items,
            ]),
        );
    }
    for a in 0..ALLIANCES {
        d.put_verbatim(
            "alliance",
            vec![(99_000_000 + a).to_string()],
            row(&[
                "1".into(),
                "2".into(),
                "3".into(),
                "133000000000000000".into(),
            ]),
        );
        for c in 0..10 {
            d.put_verbatim(
                "alliancecorp",
                vec![(98_000_000 + a * 10 + c).to_string()],
                row(&[
                    (99_000_000 + a).to_string(),
                    "133000000000000000".into(),
                    "0".into(),
                ]),
            );
        }
    }
    for r in 0..SOV_ROWS {
        d.put_verbatim(
            "sov",
            vec![(30_000_000 + r).to_string()],
            row(&[
                "99000001".into(),
                "98000001".into(),
                "7".into(),
                "1".into(),
                "133000000000000000".into(),
            ]),
        );
    }
    s.deltas = d;
    // Contacts and the market have no public builder: their records are
    // appended as text, as a saved file holds them, and parsed back.
    let mut text = render(&s);
    for c in 0..CHARS {
        for k in 0..CONTACTS_PER_CHAR {
            let _ = writeln!(
                text,
                "contact\t{}\t{}\t5\t1\t0",
                90_000_000 + c,
                91_000_000 + k
            );
        }
    }
    let _ = writeln!(text, "mktnext\t{}\t{}", 1_000_000, 1_000_000);
    for o in 0..OPEN_ORDERS + CLOSED_ORDERS {
        let state = if o < OPEN_ORDERS { 0 } else { 1 };
        let _ = writeln!(
            text,
            "mktorder\t{o}\t{}\t{}\t10000002\t30000001\t60000000\t12.5\t1000\t{}\t1\t0\t133000000000000000\t90\t32767\t0\t{state}\t-1",
            90_000_000 + o % CHARS,
            34 + o % 500,
            if state == 0 { 500 } else { 0 },
        );
    }
    for t in 0..TRADES {
        let _ = writeln!(
            text,
            "mkttrade\t{t}\t133000000000000000\t{}\t10\t12.5\t60000000\t{}\t{}\t10000002",
            34 + t % 500,
            90_000_000 + t % CHARS,
            90_000_000 + (t + 1) % CHARS,
        );
    }
    parse(&text).expect("the invented world parses")
}

fn registry_of(s: Snapshot) -> crate::service::Registry {
    let mut reg = crate::service::Registry::new(crate::model::SandboxConfig::default());
    reg.apply_snapshot(s);
    reg
}

fn scratch_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("eve-perf-save-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).expect("scratch");
    d
}

/// The numbers, per section, printed for the lane note.
#[test]
#[ignore = "release-mode benchmark: run explicitly"]
fn perf_save_sections() {
    let snap = big_snapshot();
    let reg = registry_of(snap);
    let dir = scratch_dir("sections");
    let path = dir.join("world.dat");
    let store = FileStore::new(&path);

    for round in 0..3 {
        let t = Instant::now();
        let snapshot = reg.to_snapshot();
        let clone = t.elapsed();
        let t = Instant::now();
        let text = store.render(&snapshot);
        let render = t.elapsed();
        let t = Instant::now();
        store.write_rendered(&text, false).expect("writes");
        let write = t.elapsed();
        crate::outln!(
            "[perf-save] round {round}: to_snapshot {clone:?} | render {render:?} | \
             write(tmp+fsync+rename) {write:?} | {} bytes | drop {:?}",
            text.len(),
            {
                let t = Instant::now();
                drop(snapshot);
                t.elapsed()
            }
        );
    }
    // Per-section render, to see what dominates.
    let s = reg.to_snapshot();
    let section = |name: &str, f: &dyn Fn(&mut String)| {
        let mut out = String::new();
        let t = Instant::now();
        f(&mut out);
        crate::outln!(
            "[perf-save]   {name:<12} {:>10?} {:>10} bytes",
            t.elapsed(),
            out.len()
        );
    };
    section("items", &|o| {
        for i in &s.items {
            let _ = writeln!(
                o,
                "item\t{}\t{}\t{}\t{}\t{}\t{}",
                i.item_id, i.type_id, i.owner_id, i.location_id, i.flag_id, i.quantity
            );
        }
    });
    section("contacts", &|o| s.contacts.render(o));
    section("market", &|o| s.market.render(o));
    section("deltas", &|o| {
        for (kind, rows) in s.deltas.kinds() {
            for (key, value) in rows {
                let mut f = vec!["delta".to_owned(), esc(kind), key.len().to_string()];
                f.extend(key.iter().chain(value).map(|v| esc(v)));
                o.push_str(&f.join("\t"));
                o.push('\n');
            }
        }
    });
    let t = Instant::now();
    let c = s.clone();
    crate::outln!("[perf-save]   Snapshot::clone {:?}", t.elapsed());
    drop(c);
    let _ = fs::remove_dir_all(&dir);
}

/// The lock hold of one clock save at the benchmark size: the clone
/// (`to_snapshot`) and the hand-over to a real saver, which renders and
/// writes on its own thread. Must stay under [`LOCK_HOLD_BOUND`].
#[test]
#[ignore = "release-mode benchmark: run explicitly"]
fn perf_save_lock_hold_is_bounded() {
    let reg = registry_of(big_snapshot());
    let dir = scratch_dir("hold");
    let path = dir.join("world.dat");
    let saver = Saver::spawn(FileStore::new(&path), None).expect("spawns");
    let mut worst = Duration::ZERO;
    for _ in 0..10 {
        // What `push.rs` does inside the world lock.
        let t = Instant::now();
        let snapshot = reg.to_snapshot();
        let cost = saver.save_owned(snapshot).expect("handed over");
        let held = t.elapsed();
        assert_eq!(cost.render, Duration::ZERO);
        worst = worst.max(held);
        saver.flush();
    }
    let writes = saver.take_writes();
    crate::outln!(
        "[perf-save] lock hold worst of 10: {worst:?}; saver off the lock: \
         render+write max {:?}, {} bytes a save",
        writes.max,
        writes.bytes / writes.writes.max(1)
    );
    assert!(
        worst < LOCK_HOLD_BOUND,
        "the lock hold {worst:?} passed {LOCK_HOLD_BOUND:?}"
    );
    assert!(
        writes.max < Duration::from_secs(1),
        "the off-lock saver took {:?}: incremental saves are due",
        writes.max
    );
    saver.close();
    let _ = fs::remove_dir_all(&dir);
}
