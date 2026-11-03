//! The client's warfare-buff collections (K-BOOST, `docs/lanes/k-boost.md`).
//!
//! Pure data and no behaviour: [`crate::dbuff`] applies them. The files are
//! `dbuffs/collections.tsv` and `dbuffs/modifiers.tsv`
//! (`docs/content/schema.md`), written by `tools/content_dbuffs.py` from the
//! client's `dbuffcollections.static`. What the fields mean is the client's
//! own `dbuff` package [V `dbuff/storage.py`, `common/registry.py`,
//! `common/aggregators.py`, `client/uiFormatters.py`]; each reader below says
//! which. A bundle without the files (made before this packet) loads as an
//! empty [`Dbuffs`]: no burst then does anything.

use crate::content::{Boxed, Bundle, Sources, TableSpec};
use std::collections::BTreeMap;

/// The registration row; the coordinator adds it to `content_tables::TABLES`.
pub const TABLE: TableSpec = TableSpec {
    name: "dbuffs",
    load: load_table,
    fixture: || Box::new(Dbuffs::default()) as Boxed,
};

/// The bundle's rows, or none (a pre-K-BOOST bundle, or no bundle).
fn load_table(src: &Sources) -> Result<Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => Dbuffs::from_bundle(b)?,
        None => Dbuffs::default(),
    }))
}

/// The files, and their header lines copied from `docs/content/schema.md`.
pub const COLLECTIONS: (&str, &str) = (
    "dbuffs/collections.tsv",
    "#collection_id\toperation\taggregate\tshow\tname\tdescription",
);
pub const MODIFIERS: (&str, &str) = (
    "dbuffs/modifiers.tsv",
    "#collection_id\tkind\tattribute_id\tgroup_id\tskill_id",
);

/// `OPERATION_IDS_BY_NAME` [V `dbuff/storage.py`] onto `dogma.const`'s
/// `dgmAss*` ids [V `dogma/const.pyc:8-18`], as [`crate::dogma::op`] has them.
fn operation(name: &str) -> Option<i64> {
    use crate::dogma::op;
    Some(match name {
        "PreAssignment" => op::PRE_ASSIGN,
        "PreMul" => op::PRE_MUL,
        "PreDiv" => op::PRE_DIV,
        "ModAdd" => op::MOD_ADD,
        "ModSub" => op::MOD_SUB,
        "PostMul" => op::POST_MUL,
        "PostDiv" => op::POST_DIV,
        "PostPercent" => op::POST_PERCENT,
        "PostAssignment" => op::POST_ASSIGN,
        _ => return None,
    })
}

/// How overlapping buffs of one collection aggregate [V
/// `dbuff/common/aggregators.py` `AGGREGATOR_CLASS_BY_MODE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aggregate {
    Minimum,
    Maximum,
}

/// How the buff bar shows the value [V `dbuff/client/uiFormatters.py`]:
/// `ShowInverted` negates it, `Hide` shows none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    Normal,
    Inverted,
    Hide,
}

/// One collection: the operator, the aggregation and every modifier. A
/// buff's output value is each modifier's value [V
/// `common/registry.py` `ModifierApplier.ApplyToDogmaItem`].
#[derive(Debug, Clone, PartialEq)]
pub struct Collection {
    pub id: i64,
    /// A [`crate::dogma::op`] id.
    pub operation: i64,
    pub aggregate: Aggregate,
    pub show: Show,
    pub name: String,
    /// Attributes of the ship itself (`itemModifiers`).
    pub item: Vec<i64>,
    /// Attributes of everything fitted (`locationModifiers`).
    pub location: Vec<i64>,
    /// `(attribute, group)` of what is fitted (`locationGroupModifiers`).
    pub group: Vec<(i64, i64)>,
    /// `(attribute, skill)` of what is fitted and needs the skill
    /// (`locationRequiredSkillModifiers`).
    pub skill: Vec<(i64, i64)>,
}

impl Collection {
    /// Whether it moves anything at all.
    pub fn is_empty(&self) -> bool {
        self.item.is_empty()
            && self.location.is_empty()
            && self.group.is_empty()
            && self.skill.is_empty()
    }
}

/// The tables.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dbuffs {
    collections: BTreeMap<i64, Collection>,
}

impl Dbuffs {
    /// The files from a checked bundle; an absent file is empty.
    pub fn from_bundle(b: &Bundle) -> Result<Dbuffs, String> {
        Dbuffs::parse(
            &b.data_lines(COLLECTIONS.0, COLLECTIONS.1)?,
            &b.data_lines(MODIFIERS.0, MODIFIERS.1)?,
        )
    }

    /// From each file's data lines. A short row, a bad number, an unknown
    /// operation, aggregate, show or kind, a repeated collection, or a
    /// modifier naming a collection the first file lacks is an error naming
    /// the line.
    pub fn parse<S: AsRef<str>>(collections: &[S], modifiers: &[S]) -> Result<Dbuffs, String> {
        let mut t = Dbuffs::default();
        for (i, line) in collections.iter().enumerate() {
            let bad = |why: &str| {
                format!(
                    "{}: data line {}: {why}: {:?}",
                    COLLECTIONS.0,
                    i + 1,
                    line.as_ref()
                )
            };
            let f: Vec<&str> = line.as_ref().split('\t').collect();
            let [id, op, aggregate, show, name, _note] = f[..] else {
                return Err(bad("want 6 columns"));
            };
            let c = Collection {
                id: id.parse().map_err(|_| bad("not an integer"))?,
                operation: operation(op).ok_or_else(|| bad("unknown operation"))?,
                aggregate: match aggregate {
                    "Minimum" => Aggregate::Minimum,
                    "Maximum" => Aggregate::Maximum,
                    _ => return Err(bad("unknown aggregate")),
                },
                show: match show {
                    "ShowNormal" => Show::Normal,
                    "ShowInverted" => Show::Inverted,
                    "Hide" => Show::Hide,
                    _ => return Err(bad("unknown show")),
                },
                name: name.to_owned(),
                item: Vec::new(),
                location: Vec::new(),
                group: Vec::new(),
                skill: Vec::new(),
            };
            if t.collections.insert(c.id, c).is_some() {
                return Err(bad("collection twice"));
            }
        }
        for (i, line) in modifiers.iter().enumerate() {
            let bad = |why: &str| {
                format!(
                    "{}: data line {}: {why}: {:?}",
                    MODIFIERS.0,
                    i + 1,
                    line.as_ref()
                )
            };
            let f: Vec<&str> = line.as_ref().split('\t').collect();
            let [id, kind, attribute, group, skill] = f[..] else {
                return Err(bad("want 5 columns"));
            };
            let int = |s: &str| s.parse::<i64>().map_err(|_| bad("not an integer"));
            let id = int(id)?;
            let attribute = int(attribute)?;
            let c = t
                .collections
                .get_mut(&id)
                .ok_or_else(|| bad("unknown collection"))?;
            match kind {
                "item" => c.item.push(attribute),
                "location" => c.location.push(attribute),
                "group" => c.group.push((attribute, int(group)?)),
                "skill" => c.skill.push((attribute, int(skill)?)),
                _ => return Err(bad("unknown kind")),
            }
        }
        Ok(t)
    }

    pub fn is_empty(&self) -> bool {
        self.collections.is_empty()
    }

    pub fn len(&self) -> usize {
        self.collections.len()
    }

    pub fn collection(&self, id: i64) -> Option<&Collection> {
        self.collections.get(&id)
    }

    pub fn collections(&self) -> impl Iterator<Item = &Collection> {
        self.collections.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids (the 99xxxx band); no client data.
    const C_ITEM: i64 = 990_701;
    const C_GROUP: i64 = 990_703;
    const C_SKILL: i64 = 990_704;
    const ATTR_A: i64 = 990_711;
    const ATTR_B: i64 = 990_712;
    const GROUP_G: i64 = 990_721;
    const SKILL_S: i64 = 990_731;

    fn collections() -> Vec<String> {
        vec![
            format!("{C_ITEM}\tPostPercent\tMinimum\tShowInverted\tAlpha\titem"),
            format!("{C_GROUP}\tPostMul\tMaximum\tShowNormal\t\tgroup"),
            format!("{C_SKILL}\tModAdd\tMaximum\tHide\tGamma\tskill"),
        ]
    }

    fn modifiers() -> Vec<String> {
        vec![
            format!("{C_ITEM}\titem\t{ATTR_A}\t\t"),
            format!("{C_ITEM}\titem\t{ATTR_B}\t\t"),
            format!("{C_GROUP}\tlocation\t{ATTR_A}\t\t"),
            format!("{C_GROUP}\tgroup\t{ATTR_B}\t{GROUP_G}\t"),
            format!("{C_SKILL}\tskill\t{ATTR_A}\t\t{SKILL_S}"),
        ]
    }

    #[test]
    fn the_four_kinds_and_the_enums_parse() {
        let t = Dbuffs::parse(&collections(), &modifiers()).unwrap();
        assert_eq!(t.len(), 3);
        let i = t.collection(C_ITEM).unwrap();
        assert_eq!(i.operation, crate::dogma::op::POST_PERCENT);
        assert_eq!(i.aggregate, Aggregate::Minimum);
        assert_eq!(i.show, Show::Inverted);
        assert_eq!(i.name, "Alpha");
        assert_eq!(i.item, [ATTR_A, ATTR_B]);
        let g = t.collection(C_GROUP).unwrap();
        assert_eq!(g.operation, crate::dogma::op::POST_MUL);
        assert_eq!(g.location, [ATTR_A]);
        assert_eq!(g.group, [(ATTR_B, GROUP_G)]);
        let s = t.collection(C_SKILL).unwrap();
        assert_eq!(s.show, Show::Hide);
        assert_eq!(s.skill, [(ATTR_A, SKILL_S)]);
        assert!(!s.is_empty());
        assert!(t.collection(1).is_none());
    }

    #[test]
    fn every_operation_name_maps_to_the_dogma_operator() {
        use crate::dogma::op;
        let names = [
            ("PreAssignment", op::PRE_ASSIGN),
            ("PreMul", op::PRE_MUL),
            ("PreDiv", op::PRE_DIV),
            ("ModAdd", op::MOD_ADD),
            ("ModSub", op::MOD_SUB),
            ("PostMul", op::POST_MUL),
            ("PostDiv", op::POST_DIV),
            ("PostPercent", op::POST_PERCENT),
            ("PostAssignment", op::POST_ASSIGN),
        ];
        for (n, o) in names {
            assert_eq!(operation(n), Some(o), "{n}");
            assert!(crate::dogma::PRECEDENCE.contains(&o), "{n} is applied");
        }
        assert_eq!(operation("Sideways"), None);
    }

    #[test]
    fn a_bad_row_is_refused_with_its_line() {
        let none: Vec<String> = Vec::new();
        for (row, why) in [
            ("1\tPostPercent\tMinimum\tShowNormal\tx", "want 6 columns"),
            (
                "x\tPostPercent\tMinimum\tShowNormal\tx\ty",
                "not an integer",
            ),
            (
                "1\tSideways\tMinimum\tShowNormal\tx\ty",
                "unknown operation",
            ),
            (
                "1\tPostPercent\tMedian\tShowNormal\tx\ty",
                "unknown aggregate",
            ),
            ("1\tPostPercent\tMinimum\tMaybe\tx\ty", "unknown show"),
        ] {
            let e = Dbuffs::parse(&[row.to_owned()], &none).unwrap_err();
            assert!(e.contains(why) && e.contains("data line 1"), "{e}");
        }
        let twice = vec![collections()[0].clone(), collections()[0].clone()];
        assert!(
            Dbuffs::parse(&twice, &none)
                .unwrap_err()
                .contains("collection twice")
        );
        for (row, why) in [
            (format!("{C_ITEM}\titem\t{ATTR_A}\t"), "want 5 columns"),
            (format!("{C_ITEM}\tweird\t{ATTR_A}\t\t"), "unknown kind"),
            (
                format!("{C_GROUP}\titem\t{ATTR_A}\t\t"),
                "unknown collection",
            ),
            (format!("{C_ITEM}\tgroup\t{ATTR_A}\t\t"), "not an integer"),
        ] {
            let e = Dbuffs::parse(&collections()[..1], &[row]).unwrap_err();
            assert!(e.contains(why), "{e}");
        }
    }

    #[test]
    fn an_empty_bundle_is_an_empty_table() {
        let none: Vec<String> = Vec::new();
        let t = Dbuffs::parse(&none, &none).unwrap();
        assert!(t.is_empty());
        assert_eq!(t, Dbuffs::default());
    }

    #[test]
    fn the_header_lines_match_the_extractor() {
        assert_eq!(
            COLLECTIONS.1.matches('\t').count() + 1,
            6,
            "six columns in collections.tsv"
        );
        assert_eq!(
            MODIFIERS.1.matches('\t').count() + 1,
            5,
            "five columns in modifiers.tsv"
        );
    }
}
