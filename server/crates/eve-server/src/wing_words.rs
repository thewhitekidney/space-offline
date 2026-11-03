//! **The owner's words to his wing** (plan-fc-follow §2.6, lane FC-3).
//!
//! One pure parser, two inputs: `bot_words` hands it what follows `/bots`
//! (`/bots hold`, `/bots order Wing1x primary Invented Rat`), and the bot
//! driver hands it every line the wing's FC says in fleet chat. A line is a
//! command only when **all** of it is one (after filler words and
//! punctuation are dropped); "hold the door please" is not, because a human
//! FC talks in the same room. The primary is the one command that carries
//! words after its verb.
//!
//! The words are the owner's cheat-sheet (`docs/guides/wing.md`). Every
//! synonym is **[A]**, falsified by a word the owner types that is not here.

/// What the wing is allowed to shoot at; read by the wing's target pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Fight what threatens the FC and his wing (the default).
    Defend,
    /// Do not open fire.
    Hold,
    /// Shoot hostile pilots near the FC without waiting to be shot.
    Free,
}

/// Where an Entosis Link is aimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkAim {
    /// The FC's broadcast, else the hub, else a node.
    Auto,
    Hub,
    Node,
}

/// One command to the wing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WingCmd {
    /// Anchor on the FC; cancels hold, dock, go home and the stance.
    Follow,
    /// Hold where they are.
    Hold,
    /// Dock (in the FC's station when he is docked).
    Dock,
    /// Back to the staging station and stay there.
    GoHome,
    /// Dock, resupply, come back.
    Rearm,
    Stance(Mode),
    /// Shoot the pilot or ship named (lower case).
    Primary(String),
    /// One report line.
    Status,
    /// Sov words: parsed now, acted on by FC-4 (answered "not yet").
    Entosis(LinkAim),
    StopEntosis,
    DropHub,
    FuelHub,
    /// Not a word: `/bots join` tells the wing to look for the FC's fleet
    /// advert now. [`parse`] never returns it.
    Join,
}

impl WingCmd {
    /// What the lead's "copy" names.
    pub fn describe(&self) -> String {
        match self {
            WingCmd::Follow => "following".to_owned(),
            WingCmd::Hold => "holding".to_owned(),
            WingCmd::Dock => "docking".to_owned(),
            WingCmd::GoHome => "going home".to_owned(),
            WingCmd::Rearm => "re-arming".to_owned(),
            WingCmd::Stance(Mode::Defend) => "defending".to_owned(),
            WingCmd::Stance(Mode::Hold) => "weapons hold".to_owned(),
            WingCmd::Stance(Mode::Free) => "weapons free".to_owned(),
            WingCmd::Primary(n) => format!("primary {n}"),
            WingCmd::Status => "status".to_owned(),
            WingCmd::Entosis(_) => "entosis".to_owned(),
            WingCmd::StopEntosis => "stop entosis".to_owned(),
            WingCmd::DropHub => "drop hub".to_owned(),
            WingCmd::FuelHub => "fuel hub".to_owned(),
            WingCmd::Join => "joining".to_owned(),
        }
    }

    /// A sov word: it parses now and is not acted on yet (FC-4).
    pub fn is_sov(&self) -> bool {
        matches!(
            self,
            WingCmd::Entosis(_) | WingCmd::StopEntosis | WingCmd::DropHub | WingCmd::FuelHub
        )
    }
}

/// Words that carry no meaning in a command.
const FILLER: &[&str] = &[
    "please", "pls", "now", "the", "bots", "guys", "all", "everyone", "ok", "okay", "then",
];

/// Whole-line commands, longest phrases first does not matter: the match is
/// exact on the filler-free words.
fn phrase(words: &[&str]) -> Option<WingCmd> {
    use LinkAim::*;
    Some(match words {
        ["follow"]
        | ["follow", "me"]
        | ["on", "me"]
        | ["form", "up"]
        | ["formup"]
        | ["come"]
        | ["come", "to", "me"]
        | ["regroup"]
        | ["anchor"]
        | ["anchor", "up"] => WingCmd::Follow,
        ["hold"] | ["stay"] | ["stop"] | ["hold", "position"] | ["stay", "put"] | ["halt"] => {
            WingCmd::Hold
        }
        ["dock"] | ["dock", "up"] => WingCmd::Dock,
        ["go", "home"] | ["gohome"] | ["rtb"] | ["home"] | ["return", "to", "base"] => {
            WingCmd::GoHome
        }
        ["rearm"] | ["re-arm"] | ["reship"] | ["resupply"] | ["refit"] => WingCmd::Rearm,
        ["weapons", "free"] | ["free"] | ["fire", "at", "will"] => WingCmd::Stance(Mode::Free),
        ["weapons", "hold"] | ["hold", "fire"] | ["cease", "fire"] => WingCmd::Stance(Mode::Hold),
        ["defend"] | ["defensive"] | ["weapons", "tight"] => WingCmd::Stance(Mode::Defend),
        ["entosis"] | ["link"] | ["start", "entosis"] => WingCmd::Entosis(Auto),
        ["entosis", "hub"] | ["link", "hub"] => WingCmd::Entosis(Hub),
        ["entosis", "node"] | ["link", "node"] => WingCmd::Entosis(Node),
        ["stop", "entosis"] | ["unlink"] | ["stop", "linking"] => WingCmd::StopEntosis,
        ["drop", "hub"] | ["launch", "hub"] | ["deploy", "hub"] => WingCmd::DropHub,
        ["fuel", "hub"] | ["fuel"] => WingCmd::FuelHub,
        ["status"] | ["x"] | ["report"] | ["sitrep"] => WingCmd::Status,
        _ => return None,
    })
}

/// **The words as a command**, or `None`: not a command (never a guess).
pub fn parse(words: &[&str]) -> Option<WingCmd> {
    let clean: Vec<String> = words
        .iter()
        .flat_map(|w| w.split_whitespace())
        .map(|w| {
            w.trim_matches(|c: char| matches!(c, ',' | ';' | '!' | '.' | '?' | ':'))
                .to_ascii_lowercase()
        })
        .filter(|w| !w.is_empty() && !FILLER.contains(&w.as_str()))
        .collect();
    let refs: Vec<&str> = clean.iter().map(String::as_str).collect();
    if let [verb, rest @ ..] = refs.as_slice()
        && matches!(*verb, "primary" | "focus")
        && !rest.is_empty()
    {
        return Some(WingCmd::Primary(rest.join(" ")));
    }
    phrase(&refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: &str) -> Option<WingCmd> {
        let words: Vec<&str> = line.split_whitespace().collect();
        parse(&words)
    }

    /// Every verb and synonym of the cheat-sheet.
    #[test]
    fn the_table_of_words() {
        for w in ["follow", "Follow me", "on me", "form up", "come", "regroup"] {
            assert_eq!(p(w), Some(WingCmd::Follow), "{w}");
        }
        for w in ["hold", "stay", "stop", "hold position", "STOP!"] {
            assert_eq!(p(w), Some(WingCmd::Hold), "{w}");
        }
        for w in ["dock", "dock up"] {
            assert_eq!(p(w), Some(WingCmd::Dock), "{w}");
        }
        for w in ["go home", "rtb", "RTB."] {
            assert_eq!(p(w), Some(WingCmd::GoHome), "{w}");
        }
        for w in ["rearm", "reship"] {
            assert_eq!(p(w), Some(WingCmd::Rearm), "{w}");
        }
        assert_eq!(p("weapons free"), Some(WingCmd::Stance(Mode::Free)));
        assert_eq!(p("weapons hold"), Some(WingCmd::Stance(Mode::Hold)));
        assert_eq!(p("defend"), Some(WingCmd::Stance(Mode::Defend)));
        assert_eq!(p("entosis"), Some(WingCmd::Entosis(LinkAim::Auto)));
        assert_eq!(p("entosis hub"), Some(WingCmd::Entosis(LinkAim::Hub)));
        assert_eq!(p("entosis node"), Some(WingCmd::Entosis(LinkAim::Node)));
        for w in ["stop entosis", "unlink"] {
            assert_eq!(p(w), Some(WingCmd::StopEntosis), "{w}");
        }
        for w in ["drop hub", "launch hub"] {
            assert_eq!(p(w), Some(WingCmd::DropHub), "{w}");
        }
        for w in ["fuel hub", "fuel"] {
            assert_eq!(p(w), Some(WingCmd::FuelHub), "{w}");
        }
        for w in ["status", "x"] {
            assert_eq!(p(w), Some(WingCmd::Status), "{w}");
        }
    }

    #[test]
    fn filler_case_and_punctuation_do_not_matter() {
        assert_eq!(p("Hold, please!"), Some(WingCmd::Hold));
        assert_eq!(p("ok everyone form up now"), Some(WingCmd::Follow));
        assert_eq!(p("  WEAPONS   FREE "), Some(WingCmd::Stance(Mode::Free)));
    }

    #[test]
    fn the_primary_carries_a_name() {
        assert_eq!(
            p("primary Invented Rat"),
            Some(WingCmd::Primary("invented rat".to_owned()))
        );
        assert_eq!(p("focus Stabber"), Some(WingCmd::Primary("stabber".into())));
        // A primary of nothing is nothing.
        assert_eq!(p("primary"), None);
    }

    #[test]
    fn talk_that_is_not_a_command_is_ignored() {
        for w in [
            "",
            "hold the door open for me",
            "i will dock later",
            "follow the white rabbit",
            "gg",
            "what is the status of the gate",
            "stopwatch",
        ] {
            assert_eq!(p(w), None, "{w:?}");
        }
        // `Join` is the control's, never a word.
        assert_eq!(p("join"), None);
    }

    #[test]
    fn only_the_sov_words_are_sov() {
        assert!(p("drop hub").unwrap().is_sov());
        assert!(p("unlink").unwrap().is_sov());
        assert!(!p("hold").unwrap().is_sov());
        assert!(!WingCmd::Primary("x".into()).is_sov());
    }
}
