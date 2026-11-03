//! **Where a pilot is is not `Character::solar_system_id`.** That field is
//! "my station's system": only docking, a clone jump and seating write it, so
//! once undocked it names the home system, not the one the pilot is in. Code
//! that wants where the pilot *is* reads `Registry::session_system`
//! (`docs/lanes/stale-system.md`).
//!
//! A source scan: a line in production code that reads
//! `.solar_system_id` off a character (a `character(` / `characters` call on
//! the line, or the receiver named `c` or `ch`) must be inside a file's
//! budget below. A new read fails the test until it moves to the helper or
//! its budget is raised here with the reason.

use std::path::{Path, PathBuf};

/// Reads of the station's system that are right as they are, per file
/// (station-paired meaning, docked-only context, the writers). Counted, not
/// listed by line, so an edit that only shifts lines does not trip it.
const ALLOW: &[(&str, usize)] = &[
    // the writers and "my station" rows (dock, undock/dock change, assets)
    ("service.rs", 11),
    ("store.rs", 2),
    ("clones.rs", 2),
    ("service/relocate.rs", 2),
    ("service/agents.rs", 2),
    // the undock seed: the roster wins, a jumper is refused
    ("destiny.rs", 3),
    // death: roster first, the pod goes to the home hangar
    ("combat.rs", 6),
    ("killmail.rs", 1),
    ("concord.rs", 1),
    ("bot_control.rs", 2),
    ("jump.rs", 1),
    ("map_stats.rs", 1),
    ("scan.rs", 1),
    ("security_office.rs", 1),
    ("crimewatch.rs", 1),
    ("live_status.rs", 1),
    ("liveness.rs", 1),
    ("chat.rs", 1),
    ("econ_agents/gather.rs", 1),
    ("econ_agents/combat.rs", 1),
    ("service/corp_office.rs", 1),
    ("service/market.rs", 1),
    ("service/wallet.rs", 1),
    ("service/world.rs", 1),
    ("war.rs", 1),
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if name != "tests" && !name.contains("bench") {
                sources(&p, out);
            }
        } else if name.ends_with(".rs")
            && name != "tests.rs"
            && !name.ends_with("_tests.rs")
            && name != "location_lint.rs"
            && !name.contains("bench")
        {
            out.push(p);
        }
    }
}

/// The identifier ending right before `at` in `line`.
fn ident_before(line: &str, at: usize) -> &str {
    let head = &line[..at];
    let start = head
        .rfind(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
        .map_or(0, |i| i + 1);
    &head[start..]
}

/// The hit lines of one file's production code.
fn hits(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut lines = src.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim();
        if t == "#[cfg(test)]" {
            // `mod NAME {` next: drop the block by brace depth.
            if lines
                .peek()
                .is_some_and(|n| n.trim_start().starts_with("mod ") && n.trim_end().ends_with('{'))
            {
                let mut depth = 0i32;
                for l in lines.by_ref() {
                    depth += l.matches('{').count() as i32;
                    depth -= l.matches('}').count() as i32;
                    if depth <= 0 && l.contains('}') {
                        break;
                    }
                }
            }
            continue;
        }
        if t.starts_with("//") {
            continue;
        }
        let Some(at) = line.find(".solar_system_id") else {
            continue;
        };
        let recv = ident_before(line, at);
        if line.contains("character(") || line.contains("characters") || recv == "c" || recv == "ch"
        {
            out.push(t.to_owned());
        }
    }
    out
}

#[test]
fn reads_of_the_stations_system_stay_within_budget() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&root, &mut files);
    assert!(files.len() > 50, "the scan found {} files", files.len());
    let mut bad = Vec::new();
    let mut under = Vec::new();
    for f in files {
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let found = hits(&std::fs::read_to_string(&f).unwrap());
        let budget = ALLOW.iter().find(|(p, _)| *p == rel).map_or(0, |(_, n)| *n);
        if found.len() > budget {
            bad.push(format!(
                "{rel}: {} reads, budget {budget}\n    {}",
                found.len(),
                found.join("\n    ")
            ));
        } else if found.len() < budget {
            under.push(format!("{rel}: {} of {budget}", found.len()));
        }
    }
    if !under.is_empty() {
        eprintln!("under budget (tighten ALLOW): {}", under.join(", "));
    }
    assert!(
        bad.is_empty(),
        "a read of Character::solar_system_id (the station's system); use \
         Registry::session_system for where the pilot is, or raise the budget \
         in location_lint.rs with the reason:\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_scan_counts_what_it_should() {
    let src = "fn a(r: &R) { r.character(1).and_then(|c| c.solar_system_id); }\n\
               // c.solar_system_id\n\
               fn b(p: &P) { p.solar_system_id; }\n\
               #[cfg(test)]\n\
               mod tests {\n    fn t(c: &C) { c.solar_system_id; }\n}\n\
               fn d(ch: &C) { ch.solar_system_id; }\n";
    assert_eq!(hits(src).len(), 2);
}
