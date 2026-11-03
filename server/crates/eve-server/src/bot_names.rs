//! **Character names for bots** (NAMES, S2 of the EVEJS-BOTS report). A bot is
//! known to the control link, the token file and the roster by its **key**
//! (`hulka3`: group plus number, also its account name); the name a player
//! sees in Local, the overview and the fleet window is a character name from
//! `content/bots/names.tsv`, picked here.
//!
//! The pick is a pure function of the key: it starts at a hash of the group
//! and the number and walks on to the first name nobody has, so a re-run
//! gives a bot the name it had and two bots never share one. A world that
//! already has the bot's character keeps its name (this module is asked only
//! when a character is created); wiping the world is how new names show.

use std::sync::OnceLock;

/// The shipped pool, compiled in (`content/bots/names.tsv`).
const SHIPPED: &str = include_str!("../../../../content/bots/names.tsv");

/// Whether `name` may be a bot's character name: the server's rule for any
/// visible name (`start::name_shape`) and the project's two more: at most
/// two spaces, and not all digits.
pub fn valid(name: &str) -> bool {
    crate::start::name_shape(name) == crate::start::NAME_VALID
        && name.matches(' ').count() <= 2
        && !name.chars().all(|c| c.is_ascii_digit())
}

/// Parse a pool: `flavour TAB name` rows, `#` comments and blank lines
/// skipped. A bad or repeated (ignoring case) name is an error naming the
/// line.
pub fn parse(text: &str) -> Result<Vec<String>, String> {
    Ok(parse_rows(text)?.into_iter().map(|(_, n)| n).collect())
}

/// [`parse`] keeping each name's flavour (the first column).
fn parse_rows(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let at = i + 1;
        let (flavour, name) = line
            .split_once('\t')
            .ok_or_else(|| format!("names.tsv line {at}: want flavour TAB name"))?;
        if !valid(name) {
            return Err(format!("names.tsv line {at}: {name:?} is not a valid name"));
        }
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(format!("names.tsv line {at}: {name:?} is listed twice"));
        }
        out.push((flavour.to_owned(), name.to_owned()));
    }
    Ok(out)
}

/// The shipped pool. A broken file is a build-time mistake a unit test
/// catches; here it degrades to no names (every bot keeps its key).
pub fn pool() -> &'static [String] {
    static POOL: OnceLock<Vec<String>> = OnceLock::new();
    POOL.get_or_init(|| parse(SHIPPED).unwrap_or_default())
}

/// The shipped names of one flavour (`amarr`, `caldari`, `gallente`,
/// `minmatar` are the racial ones), in file order; empty for an unknown
/// flavour. The character creator's Randomize button draws from these.
pub fn pool_of(flavour: &str) -> &'static [String] {
    static BY: OnceLock<std::collections::BTreeMap<String, Vec<String>>> = OnceLock::new();
    BY.get_or_init(|| {
        let mut m: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for (f, n) in parse_rows(SHIPPED).unwrap_or_default() {
            m.entry(f).or_default().push(n);
        }
        m
    })
    .get(flavour)
    .map_or(&[], Vec::as_slice)
}

/// A bot key split into its group and number: `hulka3` is `("hulka", 3)`; a
/// key with no trailing digits (or only digits) is `(key, 0)`.
pub fn split_key(key: &str) -> (&str, u64) {
    let group = key.trim_end_matches(|c: char| c.is_ascii_digit());
    if group.is_empty() || group.len() == key.len() {
        return (key, 0);
    }
    (group, key[group.len()..].parse().unwrap_or(0))
}

/// splitmix64's finaliser over an FNV-1a hash: short keys that differ in one
/// digit land far apart in the pool.
fn start_of(group: &str, index: u64) -> u64 {
    let mut h = group.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    h = (h ^ index.wrapping_mul(0x9e37_79b9_7f4a_7c15)).wrapping_add(0x9e37_79b9_7f4a_7c15);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^ (h >> 31)
}

/// The name for bot `index` of `group` from `pool`: the pool entry at the
/// key's hash, or the next one `taken` does not refuse. `None` for an empty
/// or used-up pool (the caller keeps the key).
pub fn pick(
    pool: &[String],
    group: &str,
    index: u64,
    taken: impl Fn(&str) -> bool,
) -> Option<String> {
    let n = pool.len();
    let first = usize::try_from(start_of(group, index) % n.max(1) as u64).ok()?;
    (0..n)
        .map(|k| &pool[(first + k) % n])
        .find(|name| !taken(name))
        .cloned()
}

/// [`pick`] from the shipped pool for a whole key (`hulka3`).
pub fn pick_for_key(key: &str, taken: impl Fn(&str) -> bool) -> Option<String> {
    let (group, index) = split_key(key);
    pick(pool(), group, index, taken)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_pool_is_big_valid_and_unique() {
        let pool = parse(SHIPPED).expect("names.tsv parses");
        assert!(pool.len() >= 1200, "only {} names", pool.len());
        let mut seen = std::collections::BTreeSet::new();
        for n in &pool {
            assert!(valid(n), "{n:?}");
            assert!(n.len() <= 24 && n.matches(' ').count() <= 2, "{n:?}");
            assert!(seen.insert(n.to_ascii_lowercase()), "{n:?} twice");
        }
        assert_eq!(pool.as_slice(), super::pool());
    }

    #[test]
    fn the_pool_has_no_name_the_server_keeps_for_itself() {
        for n in pool() {
            assert!(
                !crate::accounts::TOOL_TOKENS
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case(n)),
                "{n:?}"
            );
        }
    }

    #[test]
    fn a_pool_with_a_bad_or_repeated_name_is_refused() {
        assert!(parse("nick\tOk Name\nnick\tok name\n").is_err());
        assert!(parse("nick\ttoo  many\n").is_err());
        assert!(parse("nick\ta b c d\n").is_err());
        assert!(parse("nick\tbad!\n").is_err());
        assert!(parse("nick\tab\n").is_err());
        assert!(parse("nick\t12345\n").is_err());
        assert!(parse("# only a comment\n\nnick\tFine Pilot\n").is_ok());
    }

    #[test]
    fn each_race_has_its_own_names() {
        for f in ["amarr", "caldari", "gallente", "minmatar"] {
            assert!(pool_of(f).len() >= 100, "{f}");
            assert!(pool_of(f).iter().all(|n| pool().contains(n)));
        }
        assert!(pool_of("amarr") != pool_of("caldari"));
        assert!(pool_of("no such flavour").is_empty());
    }

    #[test]
    fn keys_split_into_group_and_number() {
        assert_eq!(split_key("hulka3"), ("hulka", 3));
        assert_eq!(split_key("redf12"), ("redf", 12));
        assert_eq!(split_key("boss"), ("boss", 0));
        assert_eq!(split_key("42"), ("42", 0));
    }

    #[test]
    fn a_pick_is_stable_and_walks_past_a_taken_name() {
        let first = pick_for_key("hulka3", |_| false).expect("a name");
        assert_eq!(pick_for_key("hulka3", |_| false), Some(first.clone()));
        let next = pick_for_key("hulka3", |n| n == first).expect("a name");
        assert_ne!(next, first);
        assert!(valid(&next));
    }

    #[test]
    fn a_roster_gets_distinct_names_even_when_every_start_collides() {
        let mut given: Vec<String> = Vec::new();
        for group in ["hulka", "redf", "bluec"] {
            for i in 1..=60 {
                let key = format!("{group}{i}");
                let name = pick_for_key(&key, |n| given.iter().any(|g| g.eq_ignore_ascii_case(n)))
                    .expect("a name");
                given.push(name);
            }
        }
        let set: std::collections::BTreeSet<_> =
            given.iter().map(|n| n.to_ascii_lowercase()).collect();
        assert_eq!(set.len(), given.len());
        // Same keys, same names, on another run.
        let again = pick_for_key("redf7", |n| given[..60 + 6].iter().any(|g| g == n));
        assert!(again.is_some());
        let tiny = vec!["Only One".to_owned()];
        assert_eq!(pick(&tiny, "g", 1, |_| false).as_deref(), Some("Only One"));
        assert_eq!(pick(&tiny, "g", 1, |n| n == "Only One"), None);
        assert_eq!(pick(&[], "g", 1, |_| false), None);
    }
}
