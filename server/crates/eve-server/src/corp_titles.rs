//! Corporation titles and the member search, the pure half (CORP-T; lane
//! note `docs/lanes/corp-t.md`).
//!
//! A corp has sixteen title slots. A title is one bit of a member's
//! `titleMask` (`titleID = 1 << n`, `n` in 0..16: the client numbers the
//! unnamed ones "Untitled n+1" from `int(math.log(titleID, 2)) + 1`) **[V]**
//! (`bco_titles.GetTitles`). Each slot has a name and the same eight role
//! masks a member row carries; a member holding a title holds its roles on
//! top of their own (the edit-member dialog's "role by title" column is
//! computed client-side from the member's `titleMask` and the titles'
//! masks, so a member row keeps its **direct** roles and the title roles
//! are never folded into it) **[V]** (`corp_dlg_edit_member`).
//!
//! Two things live here, neither needing the registry:
//!
//! - [`TitleBook`]: every corp's edited titles, and the arithmetic that
//!   folds a member's titles into their masks ([`TitleBook::held`],
//!   [`merge`]).
//! - [`matches`]: the "find member in role" query
//!   (`corpRegistry.GetMemberIDsByQuery`), evaluated for one member.
//!
//! The wire, the pushes and the registry are `service/corp.rs`.

use crate::corp_roles::{CORP_ROLE_DIRECTOR, MASK_NAMES, Masks};
use std::collections::BTreeMap;

/// Title slots per corp: the client's `title1..title16` **[V]**
/// (`base_corporation.GetInfoWindowDataForChar`).
pub const TITLE_COUNT: usize = 16;

/// The title name edit's `maxLength=100` **[V]**
/// (`corp_ui_member_titles.CorpTitleEntry`).
pub const NAME_MAX: usize = 100;

/// The id of title slot `n` (0-based).
pub const fn title_id(n: usize) -> i64 {
    1 << n
}

/// Whether `id` is one of the sixteen title ids (a single bit in range).
pub fn is_title_id(id: i64) -> bool {
    id > 0 && id.count_ones() == 1 && id.trailing_zeros() < TITLE_COUNT as u32
}

/// The mask of every title bit.
pub const ALL_TITLES: i64 = (1 << TITLE_COUNT) - 1;

/// One title: a name (empty until a director names it) and the eight masks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Title {
    pub name: String,
    pub masks: Masks,
}

impl Title {
    /// A title as a director may set it: the name trimmed and cut to
    /// [`NAME_MAX`] characters, and **no director bit in any mask** -- the
    /// client strips it itself before saving (`roles &= ~corpRoleDirector`)
    /// **[V]**, and the server holds the same line for a caller that did
    /// not, so a title can never make a director.
    pub fn sanitized(name: &str, masks: Masks) -> Title {
        let name: String = name.trim().chars().take(NAME_MAX).collect();
        Title {
            name,
            masks: masks.map(|m| m & !CORP_ROLE_DIRECTOR),
        }
    }

    /// Whether this is an unedited slot: no name, no roles.
    pub fn is_default(&self) -> bool {
        self.name.is_empty() && self.masks == [0; 8]
    }
}

/// The columns of a `GetTitles` row, in order **[V]**
/// (`corp_ui_member_titles.SaveChanges` sends them as a `Rowset` in this
/// order, and `OnTitleChanged` keys its change by these names).
pub const COLUMNS: [&str; 10] = [
    "titleID",
    "titleName",
    MASK_NAMES[0],
    MASK_NAMES[1],
    MASK_NAMES[2],
    MASK_NAMES[3],
    MASK_NAMES[4],
    MASK_NAMES[5],
    MASK_NAMES[6],
    MASK_NAMES[7],
];

/// Every corp's edited titles. A slot nobody edited is the default
/// [`Title`], and is not stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TitleBook {
    titles: BTreeMap<(i64, i64), Title>,
}

impl TitleBook {
    /// The record tag this book reads and writes.
    pub const RECORD: &'static str = "corptitle";

    /// One title of one corp, the default when it was never edited.
    pub fn get(&self, corp: i64, id: i64) -> Title {
        self.titles.get(&(corp, id)).cloned().unwrap_or_default()
    }

    /// All sixteen, in id order.
    pub fn all(&self, corp: i64) -> Vec<(i64, Title)> {
        (0..TITLE_COUNT)
            .map(|n| (title_id(n), self.get(corp, title_id(n))))
            .collect()
    }

    /// Store a title (a default one is dropped, keeping the book small).
    pub fn set(&mut self, corp: i64, id: i64, title: Title) {
        if title.is_default() {
            self.titles.remove(&(corp, id));
        } else {
            self.titles.insert((corp, id), title);
        }
    }

    /// Forget every title of a corp (it closed).
    pub fn clear_corp(&mut self, corp: i64) {
        self.titles.retain(|(c, _), _| *c != corp);
    }

    pub fn is_empty(&self) -> bool {
        self.titles.is_empty()
    }

    /// The roles a `title_mask` grants in this corp: the OR of the masks of
    /// every title bit set, mask by mask.
    pub fn held(&self, corp: i64, title_mask: i64) -> Masks {
        let mut out = [0; 8];
        for ((c, id), t) in &self.titles {
            if *c == corp && title_mask & id != 0 {
                for (o, m) in out.iter_mut().zip(t.masks) {
                    *o |= m;
                }
            }
        }
        out
    }

    /// Append the records, one per stored title:
    /// `corptitle <corp> <titleID> <name> <8 masks>`.
    pub fn render(&self, out: &mut String) {
        use std::fmt::Write as _;
        for ((corp, id), t) in &self.titles {
            let _ = write!(
                out,
                "{}\t{corp}\t{id}\t{}",
                Self::RECORD,
                crate::store::esc(&t.name)
            );
            for m in t.masks {
                let _ = write!(out, "\t{m}");
            }
            out.push('\n');
        }
    }

    /// Read one `corptitle` record. Strict, like the corp records.
    pub fn parse_record(&mut self, f: &[&str]) -> Result<(), String> {
        let num = |i: usize| -> Result<i64, String> {
            f.get(i)
                .and_then(|v| v.parse::<i64>().ok())
                .ok_or_else(|| format!("field {i} is not an integer"))
        };
        let corp = num(1)?;
        let id = num(2)?;
        if !is_title_id(id) {
            return Err(format!("{id} is not a title id"));
        }
        let name = f
            .get(3)
            .map(|v| crate::store::unesc(v))
            .ok_or("field 3 is missing")?;
        let mut masks = [0; 8];
        for (i, m) in masks.iter_mut().enumerate() {
            *m = num(4 + i)?;
        }
        self.set(corp, id, Title { name, masks });
        Ok(())
    }
}

/// A member's masks with their titles' roles on top: the roles they can
/// use. Every mask is the OR of the two.
pub fn merge(direct: &Masks, from_titles: &Masks) -> Masks {
    let mut out = *direct;
    for (o, t) in out.iter_mut().zip(from_titles) {
        *o |= t;
    }
    out
}

// ---------------------------------------------------------------------------
// The member search
// ---------------------------------------------------------------------------

/// `LOGICAL_OPERATOR_OR` and `_AND` **[V]** (`corp_ui_member_find`).
pub const JOIN_OR: i64 = 1;
pub const JOIN_AND: i64 = 2;

/// The comparison operators **[V]** (`corp_ui_member_find`).
pub const OP_EQUAL: i64 = 1;
pub const OP_GREATER: i64 = 2;
pub const OP_GREATER_OR_EQUAL: i64 = 3;
pub const OP_LESS: i64 = 4;
pub const OP_LESS_OR_EQUAL: i64 = 5;
pub const OP_NOT_EQUAL: i64 = 6;
pub const OP_HAS_BIT: i64 = 7;
pub const OP_NOT_HAS_BIT: i64 = 8;
pub const OP_CONTAINS: i64 = 9;
pub const OP_LIKE: i64 = 10;
pub const OP_STARTS_WITH: i64 = 11;
pub const OP_ENDS_WITH: i64 = 12;
pub const OP_IS: i64 = 13;

/// What a clause compares against.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Text(String),
}

/// One search term: `[property, operator, value]` for the first,
/// `[join, property, operator, value]` for the rest **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct Clause {
    /// `None` on the first term.
    pub join: Option<i64>,
    /// A [`PROPERTIES`] name.
    pub property: String,
    pub operator: i64,
    pub value: Value,
}

/// The property names the search form offers **[V]**
/// (`corp_ui_member_find.PROPERTY_LIST`).
pub const PROPERTIES: [&str; 12] = [
    "roles",
    "rolesAtHQ",
    "rolesAtBase",
    "rolesAtOther",
    "grantableRoles",
    "grantableRolesAtHQ",
    "grantableRolesAtBase",
    "grantableRolesAtOther",
    "baseID",
    "startDateTime",
    "characterID",
    "titleMask",
];

/// One member as the search sees them.
#[derive(Debug, Clone, PartialEq)]
pub struct Subject {
    pub character_id: i64,
    pub name: String,
    /// The member row's own masks.
    pub direct: Masks,
    /// What their titles add.
    pub from_titles: Masks,
    pub title_mask: i64,
    pub start_date: i64,
    /// Whether the member holds Director (or is the CEO): the role the
    /// client treats as implying the rest.
    pub director: bool,
}

/// Which masks a role term reads, from the two checkboxes of the search
/// menu: "Include implied roles" and "Search titles" **[V]** for their
/// existence (`corp_ui_member_find.GetMenuForQueryOptions`).
///
/// **[A]** on their meaning, from the labels alone: *implied* counts a
/// director as holding every role, and *search titles* counts the roles
/// a member's titles grant. Falsified by a Tranquility capture of a
/// query's answer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    pub include_implied: bool,
    pub search_titles: bool,
}

fn compare(op: i64, left: i64, right: i64) -> Option<bool> {
    Some(match op {
        OP_EQUAL => left == right,
        OP_NOT_EQUAL => left != right,
        OP_GREATER => left > right,
        OP_GREATER_OR_EQUAL => left >= right,
        OP_LESS => left < right,
        OP_LESS_OR_EQUAL => left <= right,
        OP_HAS_BIT => left & right == right,
        OP_NOT_HAS_BIT => left & right != right,
        _ => return None,
    })
}

/// SQL `LIKE` with `%` (any run) and `_` (one character), case-blind.
fn like(text: &str, pattern: &str) -> bool {
    fn go(t: &[char], p: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('%') => (0..=t.len()).any(|i| go(&t[i..], &p[1..])),
            Some('_') => !t.is_empty() && go(&t[1..], &p[1..]),
            Some(c) => t.first() == Some(c) && go(&t[1..], &p[1..]),
        }
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    go(&t, &p)
}

fn text_op(op: i64, name: &str, needle: &str) -> bool {
    let (n, q) = (name.to_lowercase(), needle.to_lowercase());
    match op {
        OP_CONTAINS => n.contains(&q),
        OP_LIKE => like(name, needle),
        OP_STARTS_WITH => n.starts_with(&q),
        OP_ENDS_WITH => n.ends_with(&q),
        OP_IS => n == q,
        _ => false,
    }
}

/// Whether one term holds for one member. An unknown property, a value of
/// the wrong kind or an operator that does not fit the property is `false`.
pub fn term(subject: &Subject, c: &Clause, opts: Options) -> bool {
    let Some(slot) = MASK_NAMES.iter().position(|n| *n == c.property) else {
        return match (c.property.as_str(), &c.value) {
            ("characterID", Value::Text(t)) => text_op(c.operator, &subject.name, t),
            ("characterID", Value::Int(id)) => {
                compare(c.operator, subject.character_id, *id).unwrap_or(false)
            }
            ("titleMask", Value::Int(v)) => {
                compare(c.operator, subject.title_mask, *v).unwrap_or(false)
            }
            ("startDateTime", Value::Int(v)) => {
                compare(c.operator, subject.start_date, *v).unwrap_or(false)
            }
            // No bases exist here: every member's `baseID` is `None`, which
            // equals nothing a picker can offer.
            ("baseID", Value::Int(_)) => c.operator == OP_NOT_EQUAL,
            _ => false,
        };
    };
    let Value::Int(want) = c.value else {
        return false;
    };
    let mut mask = subject.direct[slot];
    if opts.search_titles {
        mask |= subject.from_titles[slot];
    }
    // Director implies every held role, not the grantable ones' own masks.
    if opts.include_implied && subject.director && slot % 2 == 0 {
        mask = i64::MAX;
    }
    compare(c.operator, mask, want).unwrap_or(false)
}

/// Whether a member satisfies the whole query: terms joined left to right
/// with no precedence, as the form lists them **[A]**. An empty query
/// matches nobody.
pub fn matches(subject: &Subject, query: &[Clause], opts: Options) -> bool {
    let mut it = query.iter();
    let Some(first) = it.next() else {
        return false;
    };
    let mut acc = term(subject, first, opts);
    for c in it {
        let t = term(subject, c, opts);
        acc = match c.join {
            Some(JOIN_AND) => acc && t,
            _ => acc || t,
        };
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROLE_A: i64 = 1 << 20;
    const ROLE_B: i64 = 1 << 21;

    fn clause(join: Option<i64>, property: &str, operator: i64, value: Value) -> Clause {
        Clause {
            join,
            property: property.to_owned(),
            operator,
            value,
        }
    }

    fn subject() -> Subject {
        Subject {
            character_id: 7,
            name: "Invented Pilot".to_owned(),
            direct: [ROLE_A, 0, 0, 0, 0, 0, 0, 0],
            from_titles: [ROLE_B, 0, 0, 0, 0, 0, 0, 0],
            title_mask: 4,
            start_date: 1000,
            director: false,
        }
    }

    #[test]
    fn title_ids_are_the_sixteen_single_bits() {
        assert!(is_title_id(1) && is_title_id(1 << 15));
        assert!(!is_title_id(0) && !is_title_id(3) && !is_title_id(1 << 16) && !is_title_id(-1));
        assert_eq!(title_id(0), 1);
        assert_eq!(ALL_TITLES, 0xFFFF);
    }

    #[test]
    fn a_title_never_carries_the_director_bit_and_its_name_is_cut() {
        let t = Title::sanitized(
            &format!("  {}  ", "x".repeat(150)),
            [CORP_ROLE_DIRECTOR | ROLE_A; 8],
        );
        assert_eq!(t.name.chars().count(), NAME_MAX);
        assert!(t.masks.iter().all(|m| *m == ROLE_A));
    }

    #[test]
    fn a_member_holds_the_roles_of_every_title_bit_they_have() {
        let mut book = TitleBook::default();
        book.set(
            5,
            1,
            Title {
                name: "A".into(),
                masks: [ROLE_A, 0, 0, 0, 0, 0, ROLE_A, 0],
            },
        );
        book.set(
            5,
            4,
            Title {
                name: "B".into(),
                masks: [ROLE_B, 0, 0, 0, 0, 0, 0, 0],
            },
        );
        book.set(
            6,
            1,
            Title {
                name: "other corp".into(),
                masks: [1 << 30; 8],
            },
        );
        assert_eq!(book.held(5, 0), [0; 8]);
        assert_eq!(book.held(5, 1), [ROLE_A, 0, 0, 0, 0, 0, ROLE_A, 0]);
        assert_eq!(book.held(5, 5)[0], ROLE_A | ROLE_B);
        assert_eq!(book.held(5, 2), [0; 8]);
        assert_eq!(merge(&[ROLE_B; 8], &book.held(5, 1))[0], ROLE_A | ROLE_B);
    }

    #[test]
    fn all_sixteen_slots_answer_and_a_default_is_not_stored() {
        let mut book = TitleBook::default();
        assert_eq!(book.all(5).len(), TITLE_COUNT);
        book.set(
            5,
            2,
            Title {
                name: "x".into(),
                masks: [0; 8],
            },
        );
        assert!(!book.is_empty());
        book.set(5, 2, Title::default());
        assert!(book.is_empty());
        book.set(
            5,
            2,
            Title {
                name: "x".into(),
                masks: [0; 8],
            },
        );
        book.clear_corp(5);
        assert!(book.is_empty());
    }

    #[test]
    fn titles_survive_their_records() {
        let mut book = TitleBook::default();
        book.set(
            5,
            8,
            Title {
                name: "Tab\tname\nline".into(),
                masks: [1, 2, 3, 4, 5, 6, 7, 8],
            },
        );
        let mut text = String::new();
        book.render(&mut text);
        let mut back = TitleBook::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            back.parse_record(&f).unwrap();
        }
        assert_eq!(back, book);
        assert!(
            back.parse_record(&[
                "corptitle",
                "5",
                "3",
                "x",
                "0",
                "0",
                "0",
                "0",
                "0",
                "0",
                "0",
                "0"
            ])
            .is_err()
        );
    }

    #[test]
    fn a_role_term_reads_direct_roles_unless_titles_or_implied_are_asked_for() {
        let s = subject();
        let a = clause(None, "roles", OP_HAS_BIT, Value::Int(ROLE_A));
        let b = clause(None, "roles", OP_HAS_BIT, Value::Int(ROLE_B));
        let plain = Options::default();
        assert!(term(&s, &a, plain));
        assert!(!term(&s, &b, plain));
        assert!(term(
            &s,
            &b,
            Options {
                search_titles: true,
                ..plain
            }
        ));
        let mut d = subject();
        d.director = true;
        assert!(!term(&d, &b, plain));
        assert!(term(
            &d,
            &b,
            Options {
                include_implied: true,
                ..plain
            }
        ));
        let lacks = clause(None, "roles", OP_NOT_HAS_BIT, Value::Int(ROLE_B));
        assert!(term(&s, &lacks, plain));
    }

    #[test]
    fn terms_join_left_to_right() {
        let s = subject();
        let has_a = clause(None, "roles", OP_HAS_BIT, Value::Int(ROLE_A));
        let has_b = |j| clause(Some(j), "roles", OP_HAS_BIT, Value::Int(ROLE_B));
        let o = Options::default();
        assert!(!matches(&s, &[has_a.clone(), has_b(JOIN_AND)], o));
        assert!(matches(&s, &[has_a.clone(), has_b(JOIN_OR)], o));
        assert!(!matches(&s, &[], o));
    }

    #[test]
    fn name_titles_and_dates_compare() {
        let s = subject();
        let o = Options::default();
        let name = |op, t: &str| clause(None, "characterID", op, Value::Text(t.into()));
        assert!(matches(&s, &[name(OP_CONTAINS, "pilot")], o));
        assert!(matches(&s, &[name(OP_STARTS_WITH, "invent")], o));
        assert!(matches(&s, &[name(OP_ENDS_WITH, "PILOT")], o));
        assert!(matches(&s, &[name(OP_IS, "invented pilot")], o));
        assert!(!matches(&s, &[name(OP_IS, "invented")], o));
        assert!(matches(&s, &[name(OP_LIKE, "inv%_lot")], o));
        assert!(!matches(&s, &[name(OP_LIKE, "inv_lot")], o));
        let title = clause(None, "titleMask", OP_HAS_BIT, Value::Int(4));
        assert!(matches(&s, &[title], o));
        let title = clause(None, "titleMask", OP_NOT_HAS_BIT, Value::Int(4));
        assert!(!matches(&s, &[title], o));
        let since = |op, v| clause(None, "startDateTime", op, Value::Int(v));
        assert!(matches(&s, &[since(OP_GREATER_OR_EQUAL, 1000)], o));
        assert!(!matches(&s, &[since(OP_GREATER, 1000)], o));
        assert!(matches(&s, &[since(OP_LESS, 2000)], o));
        let unknown = clause(None, "nonsense", OP_EQUAL, Value::Int(1));
        assert!(!matches(&s, &[unknown], o));
        // No bases: nothing is at any base.
        assert!(!matches(
            &s,
            &[clause(None, "baseID", OP_EQUAL, Value::Int(9))],
            o
        ));
        assert!(matches(
            &s,
            &[clause(None, "baseID", OP_NOT_EQUAL, Value::Int(9))],
            o
        ));
    }
}
