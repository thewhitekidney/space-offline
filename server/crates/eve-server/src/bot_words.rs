//! **BOT-ADMIN: the words of `/bots`.** What the owner types, case does not
//! matter: `/bots 10 allied miners`, `/bots 6 hostile gang in Some System`,
//! `/bots pause miners3x`. [`parse`] is pure: it turns the words into a
//! [`Cmd`], and `bot_control` resolves the caller-dependent parts (their
//! system, their side, a fresh group name) and sends the explicit
//! `spawn <group> <side> <role> <count> <skills> <ship> <system id>` line
//! that `eve_bot::control` has always understood. That explicit form still
//! works as typed ([`Cmd::Raw`]).
//!
//! EvEJS's `/order` grammar (`ev:brain/src/orders/grammar.mts`) was the hint:
//! free word order, number words, synonyms. Ours is smaller: the verbs are
//! the ones the owner types, kinds map to roles in [`KINDS`] (the roles and
//! hulls of the Bot Control panel's `presets.tsv`), and an unknown word is
//! an error that names it, never a guess.

/// How many bots a bare `/bots miners` spawns.
pub const DEFAULT_COUNT: u32 = 5;

/// What a spawn asks for, apart from the caller's own system and side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    /// `help`, a bare `/bots`.
    Help,
    /// The old explicit form (or `list`, `despawn`, `bring`, `side`): the
    /// words go to `bot_control` as they are.
    Raw,
    /// A natural spawn.
    Spawn(SpawnReq),
    /// `pause <group|bot|all>`.
    Pause(String),
    /// `resume <group|bot|all>`.
    Resume(String),
    /// `inspect <bot>`.
    Inspect(String),
    /// `stop-all`.
    StopAll,
    /// `bring [group]` (no group: every group): docked, to the caller's station.
    Bring(String),
    /// GANG-SUMMON: `come [group]`, `to me`, `here` (no group: every group):
    /// the bots fly to the caller wherever he is, docked or in space.
    Come(String),
    /// `despawn <group|all>`.
    Despawn(String),
    /// FC-3: words for the wing (`follow`, `hold`, `order <group> primary x`):
    /// to the caller's wing, or (`target`) to one group.
    Order {
        target: Option<String>,
        words: Vec<String>,
    },
    /// FC-3: `join`: the caller's wing looks for his fleet now.
    Join,
    /// Words that mean nothing: the reason, then the usage is shown.
    Bad(String),
}

/// Where a natural spawn goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum At {
    /// The caller's system.
    Here,
    /// One to three jumps from the caller's system.
    Near,
    /// A system id or name (the words joined by a space).
    System(String),
}

/// One natural spawn, before the caller's context is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnReq {
    pub count: u32,
    pub kind: &'static Kind,
    pub hostile: bool,
    pub at: At,
    /// A group name the caller chose (`named x`).
    pub group: Option<String>,
    /// GANG-SUMMON: a roaming gang that hunts the caller (`hunt me`, `after
    /// me`): true by default for a hostile gang spawned `here` or `near`.
    pub quarry: bool,
    /// GANG-SUMMON: a roaming gang that goes to the caller and stays on his
    /// grid (`with me`): for an allied gang, which does not hunt him.
    pub escort: bool,
}

/// What a kind word stands for: a role and its hull, as in the panel.
#[derive(Debug, PartialEq, Eq)]
pub struct Kind {
    /// The words (singular, plural, synonyms), lower case.
    pub words: &'static [&'static str],
    /// How the reply names the bots.
    pub label: &'static str,
    /// The start of the generated group name.
    pub stem: &'static str,
    /// A `--loop` role name.
    pub role: &'static str,
    /// `all5`, `role` or a plan name.
    pub skills: &'static str,
    /// A hull name, or `-` for the role's own choice; a gang's hull grows
    /// with its size ([`Kind::ship_for`]).
    pub ship: &'static str,
}

impl Kind {
    /// The hull for `count` bots: a roaming gang is frigates up to five,
    /// cruisers up to ten and battlecruisers beyond (the panel's three
    /// gang buttons).
    pub fn ship_for(&self, count: u32) -> &'static str {
        // BP-CAMP: a roaming kind with a hull of its own (a dictor) keeps it.
        if self.role == "roamgang" && self.ship == "Rifter" {
            return match count {
                0..=5 => "Rifter",
                6..=10 => "Stabber",
                _ => "Hurricane",
            };
        }
        // FC-1: a wing is cruisers up to five, battlecruisers beyond.
        if matches!(self.role, "wing" | "sovfleet") {
            return match count {
                0..=5 => "Stabber",
                _ => "Hurricane",
            };
        }
        self.ship
    }
}

/// The kinds, from `content/bots/dev/presets.tsv`'s roles. **[A]** the
/// synonyms; falsified by a word the owner types that is not here.
pub static KINDS: &[Kind] = &[
    Kind {
        words: &["miner", "miners", "mining", "miner-bot", "hulk", "hulks"],
        label: "miners",
        stem: "Miners",
        role: "mine",
        skills: "role",
        ship: "Hulk",
    },
    Kind {
        words: &[
            "orca", "orcas", "booster", "boosters", "foreman", "foremen", "porpoise",
        ],
        label: "Orca boosters",
        stem: "Orcas",
        role: "foreman",
        skills: "role",
        ship: "Orca",
    },
    Kind {
        words: &[
            "escort",
            "escorts",
            "guard",
            "guards",
            "protector",
            "protectors",
            "bodyguard",
            "bodyguards",
        ],
        label: "escorts",
        stem: "Escorts",
        role: "escort",
        skills: "role",
        ship: "-",
    },
    Kind {
        words: &[
            "roamer", "roamers", "gang", "gangs", "hunter", "hunters", "raider", "raiders",
            "pirate", "pirates", "roaming",
        ],
        label: "roaming gang",
        stem: "Gang",
        role: "roamgang",
        skills: "role",
        ship: "Rifter",
    },
    Kind {
        words: &["wing", "wings", "wingman", "wingmen", "followers", "squad"],
        label: "wing (follows you)",
        stem: "Wing",
        role: "wing",
        skills: "all5",
        ship: "Stabber",
    },
    // FC-3: the sov wing's other duties. Roles that are not built yet are
    // answered in `bot_control` (`NOT_BUILT`); `sovfleet` spawns the DPS.
    Kind {
        words: &["logi", "logis", "logistics", "repairers", "repairer"],
        label: "logistics wing",
        stem: "WingLogi",
        role: "winglogi",
        skills: "all5",
        ship: "-",
    },
    Kind {
        words: &["linker", "linkers", "linking"],
        label: "Entosis linkers",
        stem: "WingLink",
        role: "winglink",
        skills: "all5",
        ship: "-",
    },
    Kind {
        words: &["hub", "hubhauler", "hubhaulers"],
        label: "hub hauler",
        stem: "WingHaul",
        role: "winghaul",
        skills: "all5",
        ship: "-",
    },
    Kind {
        words: &["sov", "sovfleet"],
        label: "sov fleet",
        stem: "Sov",
        role: "sovfleet",
        skills: "all5",
        ship: "Stabber",
    },
    Kind {
        words: &["camper", "campers", "camp"],
        label: "gate campers",
        stem: "Campers",
        role: "hunter",
        skills: "role",
        ship: "Rifter",
    },
    // BP-CAMP: a dictor gang; hostile, it hunts the caller like any roaming gang
    // (`roamgang` with a quarry) and camps where it lands. Its launcher and
    // probes are granted by the bot in open mode only (`camp::gm_lines`).
    Kind {
        words: &[
            "dictor",
            "dictors",
            "interdictor",
            "interdictors",
            "bubbler",
            "bubblers",
        ],
        label: "dictors",
        stem: "Dictors",
        role: "roamgang",
        skills: "all5",
        ship: "Sabre",
    },
    Kind {
        words: &["hauler", "haulers", "hauling", "freighter", "freighters"],
        label: "haulers",
        stem: "Haulers",
        role: "trader",
        skills: "hauler",
        ship: "-",
    },
    Kind {
        words: &["trader", "traders", "merchant", "merchants"],
        label: "traders",
        stem: "Traders",
        role: "trader",
        skills: "role",
        ship: "-",
    },
    Kind {
        words: &["builder", "builders", "industrialist", "industrialists"],
        label: "builders",
        stem: "Builders",
        role: "builder",
        skills: "role",
        ship: "-",
    },
    Kind {
        words: &["huffer", "huffers", "huff"],
        label: "huffers",
        stem: "Huffers",
        role: "huff",
        skills: "role",
        ship: "-",
    },
    Kind {
        words: &["ratter", "ratters", "ratting", "pve", "sites"],
        label: "site runners",
        stem: "Ratters",
        role: "sites",
        skills: "role",
        ship: "Vexor",
    },
    Kind {
        words: &["missioner", "missioners", "missions", "mission", "runner"],
        label: "mission runners",
        stem: "Missions",
        role: "missions",
        skills: "role",
        ship: "Caracal",
    },
];

/// Words that mean the caller's side.
const ALLIED: &[&str] = &[
    "allied",
    "ally",
    "allies",
    "friendly",
    "friend",
    "friends",
    "friendlies",
];
/// Words that mean the opposing side.
const HOSTILE: &[&str] = &[
    "hostile", "hostiles", "enemy", "enemies", "foe", "foes", "red", "opposing",
];
/// Words that carry no meaning.
const FILLER: &[&str] = &[
    "a", "an", "some", "the", "of", "bot", "bots", "please", "me", "my", "new", "more", "x",
    "fleet",
];
/// Words that open a spawn (`/bots spawn 5 miners`).
const SPAWN_VERBS: &[&str] = &["spawn", "add", "make", "create", "start", "summon"];

/// The kind a word names.
pub fn kind_of(word: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.words.contains(&word))
}

/// `5`, `x5`, `5x`.
fn count_of(word: &str) -> Option<u32> {
    let w = word.trim_start_matches('x').trim_end_matches('x');
    w.parse()
        .ok()
        .filter(|_| w.chars().all(|c| c.is_ascii_digit()))
}

/// Whether `args` are the old explicit spawn: `spawn <group> <side> <role>
/// <count> <skills> <ship> [system]`.
fn is_explicit_spawn(args: &[&str]) -> bool {
    args.first()
        .is_some_and(|w| w.eq_ignore_ascii_case("spawn"))
        && (7..=8).contains(&args.len())
        && args[4].parse::<u32>().is_ok()
}

/// Parse the words after `/bots`.
pub fn parse(args: &[&str]) -> Cmd {
    let Some(first) = args.first() else {
        return Cmd::Help;
    };
    let head = first.to_ascii_lowercase();
    let name_arg = |what: &str| -> Result<String, Cmd> {
        match args {
            [_, name] => Ok((*name).to_owned()),
            [_] => Err(Cmd::Bad(format!("{what}: which group or bot?"))),
            _ => Err(Cmd::Bad(format!("{what} takes one name"))),
        }
    };
    match head.as_str() {
        "help" | "?" | "-h" | "--help" | "usage" => return Cmd::Help,
        "list" | "ls" if args.len() == 1 => return Cmd::Raw,
        "side" if args.len() == 3 => return Cmd::Raw,
        "stop-all" | "stopall" | "killall" | "kill-all" if args.len() == 1 => {
            return Cmd::StopAll;
        }
        "stop" if args.len() == 2 && args[1].eq_ignore_ascii_case("all") => {
            return Cmd::StopAll;
        }
        "despawn" | "remove" | "dismiss" | "disband" => {
            return match name_arg("despawn") {
                Ok(n) => Cmd::Despawn(n),
                Err(c) => c,
            };
        }
        // FC-3: a bare `hold` is the wing's, `hold <group>` pauses.
        "hold" if args.len() == 1 => {
            return Cmd::Order {
                target: None,
                words: vec!["hold".to_owned()],
            };
        }
        "join" if args.len() == 1 => return Cmd::Join,
        // FC-3: `order <words>` to the caller's wing, `order <group> <words>`
        // to one group.
        "order" if args.len() > 1 => {
            let rest = &args[1..];
            if crate::wing_words::parse(rest).is_some() {
                return Cmd::Order {
                    target: None,
                    words: rest.iter().map(|w| w.to_ascii_lowercase()).collect(),
                };
            }
            if rest.len() > 1 && crate::wing_words::parse(&rest[1..]).is_some() {
                return Cmd::Order {
                    target: Some(rest[0].to_owned()),
                    words: rest[1..].iter().map(|w| w.to_ascii_lowercase()).collect(),
                };
            }
            return Cmd::Bad(format!(
                "order: I do not know '{}' as a command for the wing",
                rest.join(" ")
            ));
        }
        "pause" | "park" | "hold" => {
            return match name_arg("pause") {
                Ok(n) => Cmd::Pause(n),
                Err(c) => c,
            };
        }
        "resume" | "unpause" | "continue" => {
            return match name_arg("resume") {
                Ok(n) => Cmd::Resume(n),
                Err(c) => c,
            };
        }
        "inspect" | "look" | "show" | "info" => {
            return match name_arg("inspect") {
                Ok(n) => Cmd::Inspect(n),
                Err(c) => c,
            };
        }
        "bring" | "gather" => {
            return match args {
                [_] => Cmd::Bring("all".to_owned()),
                [_, g] => Cmd::Bring((*g).to_owned()),
                _ => Cmd::Bad("bring takes one group".to_owned()),
            };
        }
        // GANG-SUMMON: `come`, `come to me`, `come here`, `come <group>`,
        // `come <group> to me`: the group (or every group) flies to the
        // caller's character, in space or docked.
        "come" => {
            let rest: Vec<&str> = args[1..]
                .iter()
                .copied()
                .filter(|w| {
                    !matches!(
                        w.to_ascii_lowercase().as_str(),
                        "to" | "me" | "here" | "now" | "please"
                    )
                })
                .collect();
            return match rest.as_slice() {
                [] => Cmd::Come("all".to_owned()),
                [g] => Cmd::Come((*g).to_owned()),
                _ => Cmd::Bad("come takes one group".to_owned()),
            };
        }
        _ => {}
    }
    if is_explicit_spawn(args) {
        return Cmd::Raw;
    }
    // FC-3: the wing's words, said straight (`/bots follow me`).
    if crate::wing_words::parse(args).is_some() {
        return Cmd::Order {
            target: None,
            words: args.iter().map(|w| w.to_ascii_lowercase()).collect(),
        };
    }
    natural(args)
}

fn natural(args: &[&str]) -> Cmd {
    let words: Vec<String> = args
        .iter()
        .map(|w| {
            w.trim_matches(|c: char| matches!(c, ',' | ';' | '!' | '.'))
                .to_ascii_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect();
    let mut count = None;
    let mut kind: Option<&'static Kind> = None;
    let mut hostile = None;
    let mut at = None;
    let mut group = None;
    let mut hunt_me = false;
    let mut with_me = false;
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        let rest = |from: usize| -> (String, usize) {
            // The words up to a `named` clause are a system name.
            let end = words[from..]
                .iter()
                .enumerate()
                .position(|(k, x)| {
                    matches!(x.as_str(), "named" | "called" | "group")
                        // GANG-SUMMON: `... after me`, `... with me`.
                        || (matches!(
                            x.as_str(),
                            "hunt" | "chase" | "after" | "on" | "with" | "beside"
                        ) && words.get(from + k + 1).is_some_and(|n| n == "me"))
                })
                .map_or(words.len(), |p| from + p);
            (words[from..end].join(" "), end)
        };
        if i == 0 && SPAWN_VERBS.contains(&w) {
            // a leading verb adds nothing
        } else if let Some(n) = count_of(w) {
            if count.replace(n).is_some() {
                return Cmd::Bad(format!("two counts: {n} and an earlier one"));
            }
        } else if ALLIED.contains(&w) {
            hostile = Some(false);
        } else if HOSTILE.contains(&w) {
            hostile = Some(true);
        } else if let Some(k) = kind_of(w) {
            // `hostile pirates` reads as a side, `hostile gang` as both;
            // the first kind word wins.
            if kind.is_none() {
                kind = Some(k);
            }
            // The kind word `pirates`/`hunters` also says who it is for
            // only when no side was named: left to the default (ally).
        } else if w == "here" {
            at = Some(At::Here);
        } else if matches!(w, "near" | "nearby" | "close" | "around") {
            at = Some(At::Near);
        } else if matches!(w, "hunt" | "chase" | "after" | "on")
            && words.get(i + 1).is_some_and(|n| n == "me")
        {
            hunt_me = true;
            i += 2;
            continue;
        } else if matches!(w, "with" | "beside") && words.get(i + 1).is_some_and(|n| n == "me") {
            with_me = true;
            i += 2;
            continue;
        } else if matches!(w, "in" | "at" | "into" | "to" | "inside") {
            let (name, end) = rest(i + 1);
            if name.is_empty() {
                return Cmd::Bad(format!("'{w}' needs a system name"));
            }
            at = Some(if name == "here" || name == "me" {
                At::Here
            } else {
                At::System(name)
            });
            i = end;
            continue;
        } else if matches!(w, "named" | "called" | "group" | "as") {
            match args_original_name(args, &words, i) {
                Some(n) => group = Some(n),
                None => return Cmd::Bad(format!("'{w}' needs a group name")),
            }
            i += 2;
            continue;
        } else if FILLER.contains(&w) {
            // nothing
        } else {
            return Cmd::Bad(format!("I do not know the word '{w}'"));
        }
        i += 1;
    }
    let Some(kind) = kind else {
        return Cmd::Bad(format!(
            "what kind of bots? ({})",
            "miners, orcas, escorts, roamers, haulers, ..."
        ));
    };
    let count = count.unwrap_or(DEFAULT_COUNT);
    if count == 0 {
        return Cmd::Bad("count is at least 1".to_owned());
    }
    // A gang that hunts the caller is his enemy: `hunt me` is hostile even
    // without the word (and over an `allied` one), never the caller's side.
    let hostile = hunt_me || hostile.unwrap_or(false);
    let at = at.unwrap_or(At::Here);
    // A hostile gang spawned in or near the caller's system hunts him
    // unless told otherwise; an allied one only escorts when asked.
    let quarry = hunt_me || (hostile && kind.role == "roamgang" && !matches!(at, At::System(_)));
    Cmd::Spawn(SpawnReq {
        count,
        kind,
        hostile,
        at,
        group,
        quarry,
        escort: with_me && !quarry,
    })
}

/// The word after position `i`, in the caller's own case (group names keep
/// theirs). `words` drops empty words, so find it by counting.
fn args_original_name(args: &[&str], words: &[String], i: usize) -> Option<String> {
    let next = words.get(i + 1)?;
    args.iter()
        .find(|a| {
            a.trim_matches(|c: char| matches!(c, ',' | ';' | '!' | '.'))
                .eq_ignore_ascii_case(next)
        })
        .map(|a| {
            a.trim_matches(|c: char| matches!(c, ',' | ';' | '!' | '.'))
                .to_owned()
        })
}

/// The dialog for `help` and for words that mean nothing.
pub fn usage() -> String {
    "/bots <count> [allied|hostile] <kind> [here|near|in <system>]  (count 5, allied, here by default)\n\
Kinds: miners, orcas (boosters), escorts (guards), roamers (gang, hunters), campers, dictors (open mode: a hostile Sabre with a sphere launcher), haulers, traders, builders, ratters, missioners, wing (followers: they join your fleet and follow you).\n\
  /bots 10 allied miners        /bots 5 hostile roamers\n\
  /bots 3 orcas                 /bots 4 allied escorts here\n\
  /bots 6 hostile gang in <system name>      /bots 2 haulers\n\
Gangs: /bots 5 hostile roamers hunt you by default; /bots 4 allied roamers with me; /bots come [group] brings a group to you, docked or in space.\n\
Wing (FC-3): /bots 6 wing | 2 logi | 2 linkers | hub hauler | 10 sovfleet; then /bots join | follow | hold | dock | go home | rearm | weapons free|hold | defend | primary <name> | status (also said in fleet chat); /bots order <group> <words> for one group. linkers and the hub hauler are not built yet.\n\
Manage: /bots list | despawn <group|all> | bring [group] (docked) | come [group] | pause <group|bot|all> | resume <group|bot|all> | inspect <bot> | stop-all\n\
Explicit: /bots spawn <group> <side> <role> <count> <skills> <ship> [system id]\n\
A KILL file in the bots' var dir releases every bot."
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: &str) -> Cmd {
        let words: Vec<&str> = line.split_whitespace().collect();
        parse(&words)
    }

    fn spawn(line: &str) -> SpawnReq {
        match p(line) {
            Cmd::Spawn(s) => s,
            other => panic!("{line}: {other:?}"),
        }
    }

    #[test]
    fn the_owners_examples_parse() {
        let s = spawn("10 allied miners");
        assert_eq!((s.count, s.kind.role, s.hostile), (10, "mine", false));
        assert_eq!(s.at, At::Here);
        let s = spawn("5 hostile roamers");
        assert_eq!((s.count, s.kind.role, s.hostile), (5, "roamgang", true));
        assert_eq!(spawn("3 orcas").kind.role, "foreman");
        let s = spawn("4 allied escorts here");
        assert_eq!((s.count, s.kind.role, s.at), (4, "escort", At::Here));
        let s = spawn("6 hostile gang in Invented Gate");
        assert_eq!(s.at, At::System("invented gate".to_owned()));
        assert_eq!(spawn("2 haulers").kind.skills, "hauler");
    }

    /// BP-CAMP: the preset the pvp spec names for the bubble check.
    #[test]
    fn a_hostile_dictor_is_a_one_ship_gang_that_comes_for_the_caller_in_its_own_hull() {
        let s = spawn("1 hostile dictor here");
        assert_eq!((s.count, s.kind.role, s.hostile), (1, "roamgang", true));
        assert_eq!(s.kind.ship_for(1), "Sabre");
        assert_eq!(
            s.kind.ship_for(9),
            "Sabre",
            "the hull does not grow with the gang"
        );
        assert_eq!(s.at, At::Here);
        assert!(s.quarry, "a hostile roaming gang hunts the caller");
        assert_eq!(spawn("2 hostile bubblers").kind.stem, "Dictors");
    }

    #[test]
    fn a_hostile_gang_hunts_the_caller_by_default() {
        let s = spawn("5 hostile roamers");
        assert!(s.quarry && !s.escort);
        assert!(spawn("5 hostile roamers near").quarry);
        // Named to another system, or not hostile, or not a gang: no quarry.
        assert!(!spawn("5 hostile roamers in Invented Gate").quarry);
        assert!(!spawn("5 allied roamers").quarry);
        assert!(!spawn("5 hostile miners").quarry);
        // Asked for in words.
        assert!(spawn("5 roamers hunt me").quarry);
        // HUNT-SIDE: hunting him means the foe side, never his corp.
        assert!(spawn("5 roamers hunt me").hostile);
        assert!(spawn("20 dictors hunt me").hostile);
        assert!(spawn("5 allied roamers hunt me").hostile);
        assert!(!spawn("4 allied roamers with me").hostile);
        assert!(spawn("5 hostile roamers in Invented Gate after me").quarry);
        let s = spawn("4 allied roamers with me");
        assert!(s.escort && !s.quarry);
        assert_eq!(spawn("3 allied roamers to me").at, At::Here);
    }

    #[test]
    fn defaults_are_five_allied_here() {
        let s = spawn("miner");
        assert_eq!((s.count, s.hostile, s.at.clone()), (5, false, At::Here));
        assert_eq!(s.group, None);
        // A bare side word has no kind: asked for, not guessed.
        assert!(matches!(p("ally"), Cmd::Bad(_)));
    }

    #[test]
    fn case_plurals_synonyms_and_word_order_do_not_matter() {
        for w in ["miner", "Miners", "MINING", "hulks"] {
            assert_eq!(spawn(&format!("2 {w}")).kind.role, "mine", "{w}");
        }
        for w in ["orca", "Orcas", "booster"] {
            assert_eq!(spawn(w).kind.role, "foreman", "{w}");
        }
        for w in ["escort", "guards"] {
            assert_eq!(spawn(w).kind.role, "escort", "{w}");
        }
        for w in ["roamer", "gang", "Hunters"] {
            assert_eq!(spawn(w).kind.role, "roamgang", "{w}");
        }
        let a = spawn("spawn 7 Hostile Miners");
        let b = spawn("hostile miners 7");
        let c = spawn("7x enemy mining");
        assert_eq!((a.count, a.hostile), (7, true));
        assert_eq!(a, b);
        assert_eq!(a.kind, c.kind);
        assert_eq!(c.count, 7);
        assert!(!spawn("friendly miners").hostile);
    }

    #[test]
    fn places_and_names() {
        assert_eq!(spawn("3 miners near").at, At::Near);
        assert_eq!(
            spawn("3 miners in Invented Gate").at,
            At::System("invented gate".to_owned())
        );
        assert_eq!(
            spawn("3 miners at 12345").at,
            At::System("12345".to_owned())
        );
        let s = spawn("3 miners in Invented Gate named Crew");
        assert_eq!(s.at, At::System("invented gate".to_owned()));
        assert_eq!(s.group.as_deref(), Some("Crew"));
        assert_eq!(spawn("3 miners in here").at, At::Here);
    }

    #[test]
    fn a_gangs_hull_grows_with_its_size() {
        let k = spawn("gang").kind;
        assert_eq!(k.ship_for(4), "Rifter");
        assert_eq!(k.ship_for(8), "Stabber");
        assert_eq!(k.ship_for(16), "Hurricane");
        assert_eq!(spawn("miners").kind.ship_for(16), "Hulk");
    }

    /// FC-1: `wing` and its synonyms are the wing role; its hull grows with
    /// its size.
    #[test]
    fn a_wing_is_the_wing_role_with_a_hull_by_size() {
        for w in ["wing", "wings", "wingmen", "followers", "squad"] {
            let s = spawn(&format!("4 {w}"));
            assert_eq!((s.kind.role, s.kind.skills), ("wing", "all5"), "{w}");
            assert!(!s.hostile);
        }
        let k = spawn("wing").kind;
        assert_eq!(k.ship_for(5), "Stabber");
        assert_eq!(k.ship_for(6), "Hurricane");
    }

    #[test]
    fn the_management_verbs() {
        assert_eq!(p("list"), Cmd::Raw);
        assert!(
            matches!(p("status"), Cmd::Order { target: None, .. }),
            "status is a wing order, not a raw command"
        );
        assert_eq!(p("help"), Cmd::Help);
        assert_eq!(p(""), Cmd::Help);
        assert_eq!(p("despawn all"), Cmd::Despawn("all".into()));
        assert_eq!(p("bring Crew"), Cmd::Bring("Crew".into()));
        assert_eq!(p("bring"), Cmd::Bring("all".into()));
        assert_eq!(p("come"), Cmd::Come("all".into()));
        assert_eq!(p("come RedGang2x"), Cmd::Come("RedGang2x".into()));
        assert_eq!(p("come to me"), Cmd::Come("all".into()));
        assert_eq!(p("come Crew here"), Cmd::Come("Crew".into()));
        assert!(matches!(p("come a b"), Cmd::Bad(_)));
        assert_eq!(p("pause Crew"), Cmd::Pause("Crew".into()));
        assert_eq!(p("RESUME crew1"), Cmd::Resume("crew1".into()));
        assert_eq!(p("inspect crew1"), Cmd::Inspect("crew1".into()));
        assert_eq!(p("stop-all"), Cmd::StopAll);
        assert_eq!(p("stop all"), Cmd::StopAll);
        assert!(matches!(p("pause"), Cmd::Bad(_)));
        assert!(matches!(p("pause a b"), Cmd::Bad(_)));
    }

    /// FC-3: the wing's words, said straight, with `order`, or to a group.
    #[test]
    fn the_wings_words_become_orders() {
        let say = |ws: &[&str]| Cmd::Order {
            target: None,
            words: ws.iter().map(|w| (*w).to_owned()).collect(),
        };
        assert_eq!(p("follow me"), say(&["follow", "me"]));
        assert_eq!(p("hold"), say(&["hold"]));
        assert_eq!(p("Weapons Free"), say(&["weapons", "free"]));
        assert_eq!(
            p("primary Invented Rat"),
            say(&["primary", "invented", "rat"])
        );
        assert_eq!(p("order go home"), say(&["go", "home"]));
        assert_eq!(
            p("order Wing1x hold"),
            Cmd::Order {
                target: Some("Wing1x".into()),
                words: vec!["hold".into()]
            }
        );
        assert_eq!(p("x"), say(&["x"]));
        assert!(matches!(p("order dance"), Cmd::Bad(_)));
        assert_eq!(p("join"), Cmd::Join);
        // The old verbs keep their meaning.
        assert_eq!(p("hold Crew"), Cmd::Pause("Crew".into()));
        assert_eq!(p("list"), Cmd::Raw);
        assert_eq!(p("stop all"), Cmd::StopAll);
        assert_eq!(p("come Crew"), Cmd::Come("Crew".into()));
    }

    /// FC-3: the new kinds; the first kind word wins (`hub hauler`).
    #[test]
    fn the_sov_kinds_map_to_their_roles() {
        for (w, role) in [
            ("logi", "winglogi"),
            ("repairers", "winglogi"),
            ("linkers", "winglink"),
            ("hub hauler", "winghaul"),
            ("hubhauler", "winghaul"),
            ("sovfleet", "sovfleet"),
            ("sov fleet", "sovfleet"),
        ] {
            assert_eq!(spawn(&format!("2 {w}")).kind.role, role, "{w}");
        }
        assert_eq!(spawn("hauler hub").kind.role, "trader");
        assert_eq!(spawn("10 sovfleet").count, 10);
        assert_eq!(spawn("sovfleet").kind.ship_for(8), "Hurricane");
    }

    #[test]
    fn the_old_explicit_form_is_left_alone() {
        assert_eq!(p("spawn gang1 RED mine 3 role Hulk 30000042"), Cmd::Raw);
        assert_eq!(p("spawn alpha red mine 3 all5 merlin"), Cmd::Raw);
        assert_eq!(p("side alpha blue"), Cmd::Raw);
    }

    #[test]
    fn nonsense_is_named_and_never_guessed() {
        let Cmd::Bad(why) = p("5 purple widgets") else {
            panic!("a spawn of nothing");
        };
        assert!(why.contains("purple"), "{why}");
        assert!(matches!(p("0 miners"), Cmd::Bad(_)));
        assert!(matches!(p("3 4 miners"), Cmd::Bad(_)));
        assert!(matches!(p("5 miners in"), Cmd::Bad(_)));
        assert!(matches!(p("5"), Cmd::Bad(_)));
        assert!(usage().contains("/bots 10 allied miners"));
    }
}
