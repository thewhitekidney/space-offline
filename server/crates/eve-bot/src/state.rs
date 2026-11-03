//! **A bot's state file** (R-SOCIETY §2.4, SOC-A4): one `<character>.tsv`
//! per bot in a bot-side directory (`eve-bots --state DIR`, SOC-A2). It is
//! never in the world file, and the server never sees it.
//!
//! **The format** is std-only text, one row per line, fields split by a
//! tab, and `\`, tab and newline escaped in text fields:
//!
//! ```text
//! mind    1                       the format and its version
//! name    <character>
//! seed    <trait seed>            the traits are drawn again from (name, seed)
//! career  <career>  <since>
//! pending <career>  <wins>        optional
//! switch  <from>  <to>  <at>      oldest first
//! goal    <kind>  <args...>       the stack, active first
//! income  <career>  <isk/h>  <hours>
//! regard  <who>  <score>  <at>   (a `grudge <group> <score> <at>` row of an
//!                                 older file loads as regard `-score`)
//! corp    <corp>  <joined>  <left or ->
//! home    <station>               optional (SOC-SIG)
//! end     <rows between the first and this one>
//! ```
//!
//! **It is written by temp-and-rename** ([`save`]), so a crash leaves the
//! old file or the new one. A file that still fails to read -- no `end`
//! row, a count that does not match, a field that does not parse, another
//! character's name -- is **torn**, and [`load_or_seed`] falls back to a
//! fresh mind from the seed and says why.

use crate::career::{Career, Signals};
use crate::mind::{Goal, Ledger, Mind, Regard, Stint, Switch, Traits};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// The format's version, the `mind` row's field.
pub const VERSION: u32 = 1;

/// Where a mind came from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// No state file: seeded.
    Fresh,
    /// Read back from the state file.
    Restored,
    /// The file was torn or unreadable: seeded, and this is why.
    Torn(String),
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\t' => o.push_str("\\t"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            c => o.push(c),
        }
    }
    o
}

fn unesc(s: &str) -> Result<String, String> {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            o.push(c);
            continue;
        }
        match it.next() {
            Some('\\') => o.push('\\'),
            Some('t') => o.push('\t'),
            Some('n') => o.push('\n'),
            Some('r') => o.push('\r'),
            other => return Err(format!("bad escape {other:?}")),
        }
    }
    Ok(o)
}

/// The state file's name for character `name`: letters, digits and `-`
/// as they are, a space as `_`, every other byte as `%XX`.
pub fn file_name(name: &str) -> String {
    let mut o = String::new();
    for b in name.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' => o.push(char::from(b)),
            b' ' => o.push('_'),
            b => {
                let _ = write!(o, "%{b:02X}");
            }
        }
    }
    o.push_str(".tsv");
    o
}

/// The state file of `name` in `dir`.
pub fn path_for(dir: &Path, name: &str) -> PathBuf {
    dir.join(file_name(name))
}

fn goal_row(g: &Goal) -> String {
    match g {
        Goal::Rung(t) => format!("Rung\t{t}"),
        Goal::Skill(t, l) => format!("Skill\t{t}\t{l}"),
        Goal::Wallet(i) => format!("Wallet\t{i}"),
        Goal::FoundCorp => "FoundCorp".to_owned(),
        Goal::JoinCorp(p) => format!("JoinCorp\t{}", esc(p)),
        Goal::Claim(s) => format!("Claim\t{s}"),
        Goal::Avenge(g) => format!("Avenge\t{g}"),
    }
}

/// The file's text.
pub fn render(m: &Mind) -> String {
    let mut rows = vec![
        format!("name\t{}", esc(&m.name)),
        format!("seed\t{}", m.seed),
        format!("career\t{}\t{}", esc(&m.career), m.since),
    ];
    if let Some((c, w)) = &m.pending {
        rows.push(format!("pending\t{}\t{w}", esc(c)));
    }
    for s in &m.history {
        rows.push(format!(
            "switch\t{}\t{}\t{}",
            esc(&s.from),
            esc(&s.to),
            s.at
        ));
    }
    for g in &m.goals {
        rows.push(format!("goal\t{}", goal_row(g)));
    }
    for (c, l) in &m.income {
        rows.push(format!("income\t{}\t{}\t{}", esc(c), l.isk_h, l.hours));
    }
    for g in &m.regard {
        rows.push(format!("regard\t{}\t{}\t{}", g.who, g.score, g.at));
    }
    for s in &m.corps {
        let left = s.left.map_or("-".to_owned(), |l| l.to_string());
        rows.push(format!("corp\t{}\t{}\t{left}", s.corp, s.joined));
    }
    if let Some(h) = m.home {
        rows.push(format!("home\t{h}"));
    }
    let mut o = format!("mind\t{VERSION}\n");
    for r in &rows {
        o.push_str(r);
        o.push('\n');
    }
    let _ = writeln!(o, "end\t{}", rows.len());
    o
}

fn num<T: std::str::FromStr>(f: Option<&&str>, what: &str) -> Result<T, String> {
    f.and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad {what}"))
}

fn text(f: Option<&&str>, what: &str) -> Result<String, String> {
    unesc(f.ok_or_else(|| format!("no {what}"))?)
}

/// Parse a state file's text. The traits are drawn again from the stored
/// name and seed.
pub fn parse(src: &str) -> Result<Mind, String> {
    let lines: Vec<&str> = src.lines().collect();
    if lines.first() != Some(&format!("mind\t{VERSION}").as_str()) {
        return Err("no mind header of this version".into());
    }
    let last = lines.last().copied().unwrap_or_default();
    let n: usize = last
        .strip_prefix("end\t")
        .and_then(|n| n.parse().ok())
        .ok_or("no end row")?;
    if n + 2 != lines.len() {
        return Err(format!(
            "end says {n} rows, the file has {}",
            lines.len().saturating_sub(2)
        ));
    }
    let mut name = None;
    let mut seed = None;
    let mut career = None;
    let mut m = Mind {
        name: String::new(),
        seed: 0,
        traits: Traits([0.0; crate::mind::TRAIT_COUNT]),
        career: String::new(),
        since: 0,
        pending: None,
        history: Vec::new(),
        goals: Vec::new(),
        income: Default::default(),
        regard: Vec::new(),
        corps: Vec::new(),
        home: None,
    };
    for line in &lines[1..lines.len() - 1] {
        let f: Vec<&str> = line.split('\t').collect();
        let (g1, g2, g3) = (f.get(1), f.get(2), f.get(3));
        match f[0] {
            "name" => name = Some(text(g1, "name")?),
            "seed" => seed = Some(num::<u64>(g1, "seed")?),
            "career" => career = Some((text(g1, "career")?, num::<u64>(g2, "since")?)),
            "pending" => m.pending = Some((text(g1, "pending")?, num(g2, "wins")?)),
            "switch" => m.history.push(Switch {
                from: text(g1, "from")?,
                to: text(g2, "to")?,
                at: num(g3, "at")?,
            }),
            "goal" => m.goals.push(match f.get(1).copied() {
                Some("Rung") => Goal::Rung(num(g2, "rung")?),
                Some("Skill") => Goal::Skill(num(g2, "skill")?, num(g3, "level")?),
                Some("Wallet") => Goal::Wallet(num(g2, "wallet")?),
                Some("FoundCorp") => Goal::FoundCorp,
                Some("JoinCorp") => Goal::JoinCorp(text(g2, "profile")?),
                Some("Claim") => Goal::Claim(num(g2, "system")?),
                Some("Avenge") => Goal::Avenge(num(g2, "group")?),
                other => return Err(format!("unknown goal {other:?}")),
            }),
            "income" => {
                let l = Ledger {
                    isk_h: num(g2, "isk/h")?,
                    hours: num(g3, "hours")?,
                };
                m.income.insert(text(g1, "income")?, l);
            }
            "regard" => m.regard.push(Regard {
                who: num(g1, "who")?,
                score: num(g2, "score")?,
                at: num(g3, "at")?,
            }),
            // An older file: a grudge was a positive magnitude.
            "grudge" => m.regard.push(Regard {
                who: num(g1, "group")?,
                score: -num::<f64>(g2, "score")?,
                at: num(g3, "at")?,
            }),
            "corp" => m.corps.push(Stint {
                corp: num(g1, "corp")?,
                joined: num(g2, "joined")?,
                left: match g3.copied() {
                    Some("-") => None,
                    _ => Some(num(g3, "left")?),
                },
            }),
            "home" => m.home = Some(num(g1, "home")?),
            other => return Err(format!("unknown row {other:?}")),
        }
    }
    m.name = name.ok_or("no name row")?;
    m.seed = seed.ok_or("no seed row")?;
    (m.career, m.since) = career.ok_or("no career row")?;
    m.traits = Traits::draw(&m.name, m.seed);
    Ok(m)
}

/// Write `m` to its file in `dir` by temp-and-rename, and return the path.
pub fn save(dir: &Path, m: &Mind) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = path_for(dir, &m.name);
    let tmp = path.with_extension("tsv.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(render(m).as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// The mind of `name`: read from `dir` when its file is there and whole,
/// else seeded from `(name, seed)` ([`Mind::seeded`]).
pub fn load_or_seed(
    dir: &Path,
    name: &str,
    seed: u64,
    table: &[Career],
    s: &Signals,
    now: u64,
) -> (Mind, Source) {
    let fresh = || Mind::seeded(name, seed, table, s, now);
    let src = match std::fs::read(path_for(dir, name)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (fresh(), Source::Fresh),
        Err(e) => return (fresh(), Source::Torn(e.to_string())),
        Ok(b) => b,
    };
    let parsed = String::from_utf8(src)
        .map_err(|_| "not UTF-8".to_owned())
        .and_then(|t| parse(&t))
        .and_then(|m| {
            if m.name == name {
                Ok(m)
            } else {
                Err(format!("the file names {:?}", m.name))
            }
        });
    match parsed {
        Ok(m) => (m, Source::Restored),
        Err(why) => (fresh(), Source::Torn(why)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::career::CAREERS;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("eve-bot-state-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// An invented mind with every row kind filled.
    fn full() -> Mind {
        let mut m = Mind::seeded(
            "Invented\tPilot \\ Ünï",
            11,
            CAREERS,
            &Signals::default(),
            1000,
        );
        m.pending = Some(("a\tb".into(), 2));
        m.history.push(Switch {
            from: "x".into(),
            to: "y".into(),
            at: 5,
        });
        m.goals = vec![
            Goal::Rung(1),
            Goal::Skill(2, 3),
            Goal::Wallet(4),
            Goal::FoundCorp,
            Goal::JoinCorp("calm\nfolk".into()),
            Goal::Claim(5),
            Goal::Avenge(6),
        ];
        m.record("x", 1234.5678, 0.3);
        m.record("y", 0.1, 1e-3);
        m.regard.push(Regard {
            who: 7,
            score: -0.125,
            at: 8,
        });
        m.corps.push(Stint {
            corp: 9,
            joined: 10,
            left: Some(11),
        });
        m.corps.push(Stint {
            corp: 12,
            joined: 13,
            left: None,
        });
        m.home = Some(14);
        m
    }

    #[test]
    fn a_state_file_round_trips() {
        let m = full();
        assert_eq!(parse(&render(&m)), Ok(m.clone()));
        let dir = scratch("rt");
        let p = save(&dir, &m).unwrap();
        assert_eq!(p, path_for(&dir, &m.name));
        assert!(!p.with_extension("tsv.tmp").exists());
        let (back, src) = load_or_seed(&dir, &m.name, 999, CAREERS, &Signals::default(), 0);
        assert_eq!(src, Source::Restored);
        assert_eq!(back, m, "the stored seed wins over the run's");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn state_file_roundtrip_with_regard() {
        let mut m = full();
        m.regard.push(Regard {
            who: 31,
            score: 2.5,
            at: 99,
        });
        let text = render(&m);
        assert!(text.contains("\nregard\t31\t2.5\t99\n"));
        assert!(!text.contains("grudge"));
        assert_eq!(parse(&text), Ok(m));
    }

    #[test]
    fn old_grudge_rows_load_as_regard() {
        let m = full();
        let text = render(&m);
        // An older file: the regard rows are grudge rows (positive magnitudes).
        let old = text.replace("regard\t7\t-0.125\t8", "grudge\t7\t0.125\t8");
        assert_ne!(old, text);
        assert_eq!(parse(&old), Ok(m));
    }

    #[test]
    fn a_torn_file_falls_back_to_the_seed() {
        let m = full();
        let dir = scratch("torn");
        let p = save(&dir, &m).unwrap();
        let whole = std::fs::read_to_string(&p).unwrap();
        let seeded = Mind::seeded(&m.name, 3, CAREERS, &Signals::default(), 50);
        let torn = [
            whole[..whole.len() / 2].to_owned(),       // cut mid-write
            whole.replace("end\t", "end\t9"),          // the count is wrong
            whole.replace("seed\t11", "seed\televen"), // a field does not parse
            whole.replacen("mind\t1", "mind\t0", 1),   // another version
            String::new(),                             // empty
            render(&Mind::seeded(
                "Someone Else",
                1,
                CAREERS,
                &Signals::default(),
                0,
            )),
        ];
        for t in torn {
            std::fs::write(&p, &t).unwrap();
            let (back, src) = load_or_seed(&dir, &m.name, 3, CAREERS, &Signals::default(), 50);
            assert!(matches!(src, Source::Torn(_)), "{t:?} read as {src:?}");
            assert_eq!(back, seeded);
        }
        std::fs::remove_file(&p).unwrap();
        assert_eq!(
            load_or_seed(&dir, &m.name, 3, CAREERS, &Signals::default(), 50).1,
            Source::Fresh
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_names_are_safe_and_distinct() {
        assert_eq!(file_name("Ab-9 x"), "Ab-9_x.tsv");
        assert_ne!(file_name("a b"), file_name("a_b"));
        for n in ["../up", "a/b", "c:\\d", ".hidden"] {
            let f = file_name(n);
            assert!(
                !f.contains('/') && !f.contains('\\') && !f.starts_with('.'),
                "{f}"
            );
        }
    }
}
