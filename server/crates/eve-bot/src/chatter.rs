//! **What a bot says in Local and in fleet chat** (S1, lane chatter): it
//! reads the room, greets a human who arrives, answers a greeting, a "gf"
//! and a "boosts?", makes small talk while it mines, says goodbye before a
//! gate and complains when it is shot -- and keeps quiet the rest of the
//! time. Pure: the brain hands in what it heard ([`Scene`]) and the clock,
//! and gets back at most one line per call.
//!
//! **Who is a human** is the bot-side knowledge the bots already have
//! (`driver::is_human`: not this bot, not the fleet boss, not a pilot the
//! bot marker knows). Nothing the server leaks is read: the server does not
//! say which characters are bots. The driver fills [`Line::human`],
//! [`Scene::arrived`] and [`Scene::local_humans`] with that test.
//!
//! **The governor** keeps a crowd of bots from reading as a script. Each
//! bot has a chattiness `c` in `[0, 1]` (its [`crate::mind::Trait::Sociability`])
//! and may say a line only if
//! - a non-bot is in Local (a fleet room is its own audience);
//! - the gap since its last line is at least [`min_gap`];
//! - fewer than [`cap`] lines went out in the last [`WINDOW`];
//! - the category's own cooldown has passed ([`cooldown`]);
//! - a greeting reaches a given human at most once per [`GREET_AGAIN`] per
//!   system;
//! - a reply waited a human delay ([`reply_delay`]) -- reading and typing.
//!
//! A bot that sees another bot answer first drops its own pending answer
//! (so a crowd of bots greets once, not twenty times); a "boosts?" is also
//! claimed in-process ([`claim_boost`]) so that exactly one Orca, Porpoise
//! or Rorqual answers.
//!
//! **The numbers** follow EvEJS's chat pacing (`brain/src/chat`: gap
//! 20-80 s, 1-6 per 10 min, a 3 s plus up to 30 s reply delay) as a hint,
//! tuned here for a Local of tens of bots, and every one is **[A]**,
//! falsified by the owner reading Local for ten minutes and finding the bots
//! spammy (lower [`cap`], raise [`min_gap`]) or silent (the reverse).
//! The phrases are `content/bots/phrases.tsv`, our own words.

use crate::brain::{unit, xorshift};
use crate::chat::RoomKind;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The governor's window for [`cap`].
pub const WINDOW: Duration = Duration::from_secs(600);
/// A human is greeted again after this long **[A]** (per system).
pub const GREET_AGAIN: Duration = Duration::from_secs(900);
/// A line older than this is not answered: a reply to something said a
/// minute ago reads as a bot **[A]**.
pub const STALE: Duration = Duration::from_secs(45);
/// Small talk waits for this much silence in the room **[A]**.
pub const QUIET_FOR: Duration = Duration::from_secs(90);
/// How long a boost claim for one human holds.
const CLAIM_FOR: Duration = Duration::from_secs(120);

/// What a line is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cat {
    Greet,
    ReplyGreet,
    Gf,
    Boosts,
    Small,
    Farewell,
    UnderAttack,
    /// CHATTER-3: the bot just won a fight against a human.
    GfWon,
    /// ... or lost one.
    GfLost,
    /// A corp mate on the grid is being shot.
    MateAttacked,
    /// A hostile bot's jab at a human in Local; never from any other bot.
    Taunt,
}

impl Cat {
    /// The first column of `phrases.tsv`.
    pub fn key(self) -> &'static str {
        match self {
            Cat::Greet => "greet",
            Cat::ReplyGreet => "reply_greet",
            Cat::Gf => "gf",
            Cat::Boosts => "boosts",
            Cat::Small => "small",
            Cat::Farewell => "farewell",
            Cat::UnderAttack => "under_attack",
            Cat::GfWon => "gf_won",
            Cat::GfLost => "gf_lost",
            Cat::MateAttacked => "mate_attacked",
            Cat::Taunt => "taunt",
        }
    }

    const ALL: [Cat; 11] = [
        Cat::Greet,
        Cat::ReplyGreet,
        Cat::Gf,
        Cat::Boosts,
        Cat::Small,
        Cat::Farewell,
        Cat::UnderAttack,
        Cat::GfWon,
        Cat::GfLost,
        Cat::MateAttacked,
        Cat::Taunt,
    ];

    /// An answer to what someone just said or did (it waits a human delay).
    fn is_reply(self) -> bool {
        matches!(
            self,
            Cat::Greet | Cat::ReplyGreet | Cat::Gf | Cat::Boosts | Cat::GfWon | Cat::GfLost
        )
    }

    /// A line caused by an event of the bot's own, not by something a human
    /// said: other bots speaking first does not cancel it.
    fn is_event(self) -> bool {
        matches!(self, Cat::GfWon | Cat::GfLost)
    }

    /// A call a player would not sit on: it may use the last slot of the
    /// cap and half the gap.
    fn urgent(self) -> bool {
        matches!(self, Cat::Boosts | Cat::UnderAttack | Cat::MateAttacked)
    }
}

// ---- the governor's numbers [A] ------------------------------------------

/// The least gap between two of this bot's lines: 20 s for the chattiest,
/// 80 s for the shyest **[A]**.
pub fn min_gap(c: f64) -> Duration {
    Duration::from_secs_f64(20.0 + (1.0 - c.clamp(0.0, 1.0)) * 60.0)
}

/// Lines per [`WINDOW`]: 1 for the shyest, 6 for the chattiest **[A]**.
pub fn cap(c: f64) -> usize {
    1 + (5.0 * c.clamp(0.0, 1.0)).round() as usize
}

/// The delay before a reply: 3 s of reading, then up to 12 s plus 10 s for
/// the shy, of typing **[A]**. EvEJS's tail is 30 s; a reply that late
/// reads as a different conversation.
pub fn reply_delay(c: f64, draw: f64) -> Duration {
    let c = c.clamp(0.0, 1.0);
    Duration::from_secs_f64(3.0 + draw * (12.0 + (1.0 - c) * 10.0))
}

/// The least time between two lines of one category **[A]**.
pub fn cooldown(cat: Cat) -> Duration {
    Duration::from_secs(match cat {
        Cat::Greet => 20,
        Cat::ReplyGreet => 120,
        Cat::Gf => 90,
        Cat::Boosts => 300,
        Cat::Small => 300,
        Cat::Farewell => 180,
        Cat::UnderAttack => 90,
        Cat::GfWon | Cat::GfLost => GF_COOLDOWN_S,
        Cat::MateAttacked => MATE_COOLDOWN_S,
        Cat::Taunt => TAUNT_COOLDOWN_S,
    })
}

/// A "gf" after one's own fight, seconds between two **[A]** (EvEJS: 90).
pub const GF_COOLDOWN_S: u64 = 90;
/// A line for a corp mate being shot, seconds between two **[A]** (EvEJS: 3 min).
pub const MATE_COOLDOWN_S: u64 = 180;
/// A hostile bot's taunt, seconds between two **[A]** (EvEJS: 10 min).
pub const TAUNT_COOLDOWN_S: u64 = 600;
/// A "gf"/"gg" waits this long after the kill or loss is read, `[min, max)`
/// seconds **[A]**: falsified by the line landing before the wreck appears.
pub const RESULT_DELAY_S: (f64, f64) = (2.0, 6.0);
/// A kill or loss read this long ago is not remarked on **[A]**.
pub const RESULT_STALE: Duration = Duration::from_secs(60);
/// A hostile bot's first taunt comes this long after it has an audience,
/// `[min, max)` seconds, then each next one after [`TAUNT_EVERY_S`] **[A]**.
pub const TAUNT_FIRST_S: (f64, f64) = (60.0, 240.0);
/// The gap before the next taunt is drawn, `[min, max)` seconds **[A]**.
pub const TAUNT_EVERY_S: (f64, f64) = (600.0, 1200.0);

// ---- what the brain hands in ---------------------------------------------

/// One line the bot heard, with the driver's verdict on the speaker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub seq: u64,
    pub room: RoomKind,
    pub from: i64,
    pub text: String,
    pub at: Instant,
    /// A non-bot, not the fleet boss (`driver::is_human`).
    pub human: bool,
}

/// A fight the bot finished against a human (a killmail it is on), as the
/// driver reads it: `won` when the bot was an attacker, lost when it was
/// the victim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub seq: u64,
    pub won: bool,
    pub at: Instant,
}

/// The fights a bot's killmail pass found, waiting for the driver to take
/// them: `bot -> (primed, (other pilot, won, read at))`. The first batch a
/// bot reads is its history, not news, and is dropped.
static FIGHTS: Mutex<BTreeMap<i64, Waiting>> = Mutex::new(BTreeMap::new());
/// `(primed, fights)` of one bot: `(other pilot, won, read at)`.
type Waiting = (bool, Vec<(i64, bool, Instant)>);
/// Fights held per bot; the oldest go first.
const FIGHTS_KEPT: usize = 8;

/// `observe::turn` hands in what one killmail pass newly showed (possibly
/// nothing; the first call of a bot only primes).
pub fn note_fights(me: i64, fights: &[(i64, bool)]) {
    let Ok(mut m) = FIGHTS.lock() else {
        return;
    };
    let e = m.entry(me).or_default();
    if !e.0 {
        e.0 = true;
        return;
    }
    let now = Instant::now();
    e.1.extend(fights.iter().map(|&(o, w)| (o, w, now)));
    let over = e.1.len().saturating_sub(FIGHTS_KEPT);
    e.1.drain(..over);
}

/// The driver takes the bot's waiting fights: `(other pilot, won, read at)`.
pub fn take_fights(me: i64) -> Vec<(i64, bool, Instant)> {
    FIGHTS
        .lock()
        .ok()
        .and_then(|mut m| m.get_mut(&me).map(|e| std::mem::take(&mut e.1)))
        .unwrap_or_default()
}

/// A human who came into Local.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arrival {
    pub seq: u64,
    pub char_id: i64,
    pub at: Instant,
}

/// What the room offers this turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub heard: Vec<Line>,
    pub arrived: Vec<Arrival>,
    /// Fights against humans just read from the killmails.
    pub results: Vec<Outcome>,
    /// Non-bot pilots in Local now.
    pub local_humans: usize,
    /// This bot's chattiness, `0 ... 1`.
    pub sociability: f64,
    /// Names for the phrase slots, when known (none are read from the wire
    /// yet: a line with an unknown slot is simply not used).
    pub names: BTreeMap<i64, String>,
    pub system_name: Option<String>,
    pub ore: Option<String>,
}

impl Default for Scene {
    fn default() -> Scene {
        Scene {
            heard: Vec::new(),
            arrived: Vec::new(),
            results: Vec::new(),
            local_humans: 0,
            sociability: 0.5,
            names: BTreeMap::new(),
            system_name: None,
            ore: None,
        }
    }
}

/// The turn's context.
pub struct Ctx<'a> {
    pub now: Instant,
    /// The system the bot is in, for "greeted once per system".
    pub system: Option<i64>,
    pub in_fleet: bool,
    /// An Orca, Porpoise or Rorqual pilot: the one who answers "boosts?".
    pub foreman: bool,
    /// A mining laser is cycling: small talk is about mining.
    pub mining: bool,
    /// A hostile bot (`Policy::hostile`): the only kind that taunts.
    pub hostile: bool,
    pub hull: Option<&'static str>,
    pub scene: &'a Scene,
}

/// A reply being typed.
#[derive(Debug, Clone, PartialEq)]
struct Pending {
    cat: Cat,
    due: Instant,
    /// The line or arrival that caused it.
    trigger: u64,
    target: i64,
    deferred: bool,
}

/// The governor's memory, one per bot.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    rng: u64,
    last_say: Option<Instant>,
    said: Vec<Instant>,
    cat_last: BTreeMap<Cat, Instant>,
    greeted: BTreeMap<(i64, i64), Instant>,
    pending: Option<Pending>,
    seen_line: u64,
    seen_arrival: u64,
    seen_result: u64,
    next_taunt: Option<Instant>,
    last_heard: Option<Instant>,
    next_small: Option<Instant>,
    /// Indices of the last lines used, so a line does not come twice soon.
    recent: Vec<usize>,
}

/// splitmix64's finaliser, so that nearby seeds give unrelated first draws.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// How many recent lines a bot avoids.
const RECENT: usize = 8;

impl State {
    pub fn new(seed: u64) -> State {
        State {
            rng: mix(seed ^ 0xc4a7_7e25_91d3_0b6f) | 1,
            last_say: None,
            said: Vec::new(),
            cat_last: BTreeMap::new(),
            greeted: BTreeMap::new(),
            pending: None,
            seen_line: 0,
            seen_arrival: 0,
            seen_result: 0,
            next_taunt: None,
            last_heard: None,
            next_small: None,
            recent: Vec::new(),
        }
    }

    fn draw(&mut self) -> f64 {
        unit(xorshift(&mut self.rng))
    }

    /// Lines said in the last [`WINDOW`], as of `now`.
    pub fn said_in_window(&self, now: Instant) -> usize {
        self.said
            .iter()
            .filter(|t| now.saturating_duration_since(**t) < WINDOW)
            .count()
    }
}

#[derive(Debug, PartialEq)]
enum Block {
    /// Not before this.
    Gap(Instant),
    Cap,
    Cooldown,
}

fn allow(st: &State, cat: Cat, c: f64, now: Instant) -> Result<(), Block> {
    let gap = if cat.urgent() {
        min_gap(c) / 2
    } else {
        min_gap(c)
    };
    if let Some(t) = st.last_say
        && now.saturating_duration_since(t) < gap
    {
        return Err(Block::Gap(t + gap));
    }
    let limit = cap(c) + usize::from(cat.urgent());
    if st.said_in_window(now) >= limit {
        return Err(Block::Cap);
    }
    if let Some(t) = st.cat_last.get(&cat)
        && now.saturating_duration_since(*t) < cooldown(cat)
    {
        return Err(Block::Cooldown);
    }
    Ok(())
}

// ---- the phrase table -----------------------------------------------------

fn table() -> &'static [(Cat, &'static str)] {
    static T: OnceLock<Vec<(Cat, &'static str)>> = OnceLock::new();
    T.get_or_init(|| {
        include_str!("../../../../content/bots/phrases.tsv")
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .filter_map(|l| {
                let (k, line) = l.split_once('\t')?;
                let cat = Cat::ALL.into_iter().find(|c| c.key() == k.trim())?;
                Some((cat, line.trim()))
            })
            .collect()
    })
}

struct Slots<'a> {
    name: Option<&'a str>,
    system: Option<&'a str>,
    ore: Option<&'a str>,
    hull: Option<&'a str>,
}

/// The line with its slots filled, or `None` when one is unknown.
fn fill(line: &str, s: &Slots) -> Option<String> {
    let mut out = String::new();
    let mut rest = line;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let j = rest[i..].find('}')? + i;
        let v = match &rest[i + 1..j] {
            "name" => s.name,
            "system" => s.system,
            "ore" => s.ore,
            "hull" => s.hull,
            _ => None,
        }?;
        out.push_str(v);
        rest = &rest[j + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn pick(st: &mut State, cat: Cat, s: &Slots) -> Option<String> {
    let mut fit: Vec<(usize, String)> = Vec::new();
    for (i, (c, line)) in table().iter().enumerate() {
        if *c == cat
            && let Some(t) = fill(line, s)
        {
            fit.push((i, t));
        }
    }
    let fresh: Vec<&(usize, String)> = fit.iter().filter(|(i, _)| !st.recent.contains(i)).collect();
    let from: Vec<&(usize, String)> = if fresh.is_empty() {
        fit.iter().collect()
    } else {
        fresh
    };
    if from.is_empty() {
        return None;
    }
    let (i, text) = from[(st.draw() * from.len() as f64) as usize % from.len()].clone();
    st.recent.push(i);
    if st.recent.len() > RECENT {
        st.recent.remove(0);
    }
    Some(text)
}

// ---- reading the room -----------------------------------------------------

const GREETINGS: [&str; 14] = [
    "hi", "hello", "hey", "hej", "yo", "sup", "hiya", "heya", "o/", "\\o/", "morning", "evening",
    "gm", "hallo",
];
const THANKS: [&str; 6] = ["gf", "gg", "ggs", "gj", "wp", "o7"];

fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split_whitespace()
        .map(|t| {
            t.trim_matches(|c: char| matches!(c, ',' | '.' | '!' | '?' | ':' | ';'))
                .to_owned()
        })
        .filter(|t| !t.is_empty())
        .collect()
}

/// What a human's line asks for, if anything the bot answers. A "boosts?"
/// is only a request to a foreman.
pub fn classify(text: &str, foreman: bool) -> Option<Cat> {
    let t = tokens(text);
    if t.iter().any(|w| w.starts_with("boost")) {
        return foreman.then_some(Cat::Boosts);
    }
    if t.len() <= 4 && t.iter().any(|w| GREETINGS.contains(&w.as_str())) {
        return Some(Cat::ReplyGreet);
    }
    if t.len() <= 5 && t.iter().any(|w| THANKS.contains(&w.as_str())) {
        return Some(Cat::Gf);
    }
    None
}

/// In-process claim of one "boosts?" asker: the first bot to ask wins.
/// Separate `eve-bots` processes fall back to hearing each other.
pub fn claim_boost(asker: i64, now: Instant) -> bool {
    static CLAIMS: Mutex<Vec<(i64, Instant)>> = Mutex::new(Vec::new());
    let Ok(mut v) = CLAIMS.lock() else {
        return true;
    };
    v.retain(|(_, t)| now.saturating_duration_since(*t) < CLAIM_FOR);
    if v.iter().any(|(a, _)| *a == asker) {
        return false;
    }
    if v.len() >= 64 {
        v.remove(0);
    }
    v.push((asker, now));
    true
}

/// **Say something, or not**: at most one line. Call once per decision.
pub fn speak(st: &mut State, cx: &Ctx) -> Option<String> {
    let (now, sc) = (cx.now, cx.scene);
    let c = sc.sociability.clamp(0.0, 1.0);
    st.said
        .retain(|t| now.saturating_duration_since(*t) < WINDOW);
    st.greeted
        .retain(|_, t| now.saturating_duration_since(*t) < GREET_AGAIN);
    let audience = cx.in_fleet || sc.local_humans > 0;

    // What was said since last time.
    let mut lines: Vec<&Line> = sc.heard.iter().filter(|l| l.seq > st.seen_line).collect();
    lines.sort_by_key(|l| l.seq);
    for l in lines {
        st.seen_line = l.seq;
        let here = matches!(
            (l.room, cx.in_fleet),
            (RoomKind::Local, false) | (RoomKind::Fleet, true)
        );
        if !here {
            continue;
        }
        st.last_heard = Some(l.at);
        if !l.human {
            // Someone else answered first: drop our own answer.
            if let Some(p) = &st.pending
                && !p.cat.is_event()
                && p.trigger < l.seq
            {
                if p.cat == Cat::Greet {
                    st.greeted.insert((p.target, cx.system.unwrap_or(0)), now);
                }
                st.pending = None;
            }
            continue;
        }
        if st.pending.is_some() || now.saturating_duration_since(l.at) > STALE {
            continue;
        }
        let Some(cat) = classify(&l.text, cx.foreman) else {
            continue;
        };
        // Boosts are never a lottery: whoever is a foreman races to claim.
        let p = if cat == Cat::Boosts {
            1.0
        } else {
            0.35 + 0.55 * c
        };
        if cat != Cat::Boosts && st.draw() >= p {
            continue;
        }
        let draw = st.draw();
        let delay = if cat == Cat::Boosts {
            Duration::from_secs_f64(3.0 + draw * 9.0)
        } else {
            reply_delay(c, draw)
        };
        st.pending = Some(Pending {
            cat,
            due: now + delay,
            trigger: l.seq,
            target: l.from,
            deferred: false,
        });
    }

    // Who came in. Not in a fleet: that is the fleet's room, not Local's.
    let seen = st.seen_arrival;
    for a in sc.arrived.iter().filter(|a| a.seq > seen) {
        st.seen_arrival = a.seq;
        if cx.in_fleet || st.pending.is_some() {
            continue;
        }
        if st
            .greeted
            .contains_key(&(a.char_id, cx.system.unwrap_or(0)))
            || now.saturating_duration_since(a.at) > STALE
        {
            continue;
        }
        if st.draw() >= 0.25 + 0.5 * c {
            continue;
        }
        let delay = reply_delay(c, st.draw());
        st.pending = Some(Pending {
            cat: Cat::Greet,
            due: now + delay,
            trigger: a.seq,
            target: a.char_id,
            deferred: false,
        });
    }

    // A fight of the bot's own that just ended: one "gf" or a grumble.
    let seen = st.seen_result;
    for r in sc.results.iter().filter(|r| r.seq > seen) {
        st.seen_result = r.seq;
        if st.pending.is_some() || now.saturating_duration_since(r.at) > RESULT_STALE {
            continue;
        }
        let draw = st.draw();
        let delay = Duration::from_secs_f64(
            RESULT_DELAY_S.0 + draw * (RESULT_DELAY_S.1 - RESULT_DELAY_S.0),
        );
        st.pending = Some(Pending {
            cat: if r.won { Cat::GfWon } else { Cat::GfLost },
            due: now + delay,
            trigger: r.seq,
            target: 0,
            deferred: false,
        });
    }

    // A reply that is due.
    if let Some(p) = st.pending.clone()
        && p.due <= now
    {
        st.pending = None;
        if !audience || now.saturating_duration_since(p.due) > STALE {
            return None;
        }
        if p.cat == Cat::Boosts && !claim_boost(p.target, now) {
            return None;
        }
        match allow(st, p.cat, c, now) {
            Ok(()) => return say(st, cx, p.cat, Some(p.target)),
            Err(Block::Gap(until)) if !p.deferred => {
                st.pending = Some(Pending {
                    due: until,
                    deferred: true,
                    ..p
                });
            }
            Err(_) => {}
        }
        return None;
    }

    // A hostile bot's jab at the humans in Local, now and then. Never in a
    // fleet's room, never from a bot that is not hostile.
    if cx.hostile && st.pending.is_none() && sc.local_humans > 0 && !cx.in_fleet {
        let due = match st.next_taunt {
            Some(t) => t,
            None => {
                let t = now
                    + Duration::from_secs_f64(
                        TAUNT_FIRST_S.0 + st.draw() * (TAUNT_FIRST_S.1 - TAUNT_FIRST_S.0),
                    );
                st.next_taunt = Some(t);
                t
            }
        };
        if now >= due && allow(st, Cat::Taunt, c, now).is_ok() {
            st.next_taunt = Some(
                now + Duration::from_secs_f64(
                    TAUNT_EVERY_S.0 + st.draw() * (TAUNT_EVERY_S.1 - TAUNT_EVERY_S.0),
                ),
            );
            return say(st, cx, Cat::Taunt, None);
        }
    }

    // Small talk, while mining, into a quiet room.
    if st.pending.is_none() && cx.mining && audience {
        let due = match st.next_small {
            Some(t) => t,
            None => {
                let t = now + Duration::from_secs_f64(90.0 + st.draw() * 210.0);
                st.next_small = Some(t);
                t
            }
        };
        let quiet = st
            .last_heard
            .is_none_or(|t| now.saturating_duration_since(t) >= QUIET_FOR);
        if now >= due && quiet && allow(st, Cat::Small, c, now).is_ok() {
            st.next_small =
                Some(now + Duration::from_secs_f64((300.0 + st.draw() * 420.0) * (1.5 - c)));
            return say(st, cx, Cat::Small, None);
        }
    }
    None
}

/// The line for a farewell before a gate jump, or `None` when the governor
/// says no. The travel code calls it just before `CmdStargateJump` and says
/// the line if it gets one **[A: not yet wired to a jump; the trips run
/// outside the brain]**.
pub fn farewell(st: &mut State, cx: &Ctx) -> Option<String> {
    let c = cx.scene.sociability.clamp(0.0, 1.0);
    if !(cx.in_fleet || cx.scene.local_humans > 0) || st.draw() >= 0.2 + 0.4 * c {
        return None;
    }
    allow(st, Cat::Farewell, c, cx.now).ok()?;
    say(st, cx, Cat::Farewell, None)
}

/// The line for being shot, naming the attacker when the scene knows his
/// name. For the threat lane to call when a human's fire lands; urgent, so
/// it answers within half the gap and may use the cap's last slot.
pub fn under_attack(st: &mut State, cx: &Ctx, attacker: Option<i64>) -> Option<String> {
    let c = cx.scene.sociability.clamp(0.0, 1.0);
    if !(cx.in_fleet || cx.scene.local_humans > 0) {
        return None;
    }
    allow(st, Cat::UnderAttack, c, cx.now).ok()?;
    say(st, cx, Cat::UnderAttack, attacker)
}

/// The line for a corp mate being shot on the grid; urgent like
/// [`under_attack`]. For the threat rule to call when a [`crate::threat::Cause::Mate`]
/// alert starts.
pub fn mate_attacked(st: &mut State, cx: &Ctx) -> Option<String> {
    let c = cx.scene.sociability.clamp(0.0, 1.0);
    if !(cx.in_fleet || cx.scene.local_humans > 0) {
        return None;
    }
    allow(st, Cat::MateAttacked, c, cx.now).ok()?;
    say(st, cx, Cat::MateAttacked, None)
}

fn say(st: &mut State, cx: &Ctx, cat: Cat, target: Option<i64>) -> Option<String> {
    let sc = cx.scene;
    let slots = Slots {
        name: target.and_then(|t| sc.names.get(&t)).map(String::as_str),
        system: sc.system_name.as_deref(),
        ore: sc.ore.as_deref(),
        hull: cx.hull,
    };
    let line = pick(st, cat, &slots)?;
    st.last_say = Some(cx.now);
    st.said.push(cx.now);
    st.cat_last.insert(cat, cx.now);
    if cat == Cat::Greet
        && let Some(t) = target
    {
        st.greeted.insert((t, cx.system.unwrap_or(0)), cx.now);
    }
    debug_assert!(cat.is_reply() || target.is_none() || cat == Cat::UnderAttack);
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYS: i64 = 30_000_777;
    const HUMAN: i64 = 970_000_101;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn line(seq: u64, from: i64, text: &str, at: Instant, human: bool) -> Line {
        Line {
            seq,
            room: RoomKind::Local,
            from,
            text: text.to_owned(),
            at,
            human,
        }
    }

    /// One bot's view of a scripted room, stepped in seconds.
    struct Bot {
        st: State,
        c: f64,
        foreman: bool,
        mining: bool,
    }

    impl Bot {
        fn new(seed: u64, c: f64) -> Bot {
            Bot {
                st: State::new(seed),
                c,
                foreman: false,
                mining: false,
            }
        }

        fn step(&mut self, now: Instant, scene: &mut Scene, humans: usize) -> Option<String> {
            scene.sociability = self.c;
            scene.local_humans = humans;
            let cx = Ctx {
                now,
                system: Some(SYS),
                in_fleet: false,
                foreman: self.foreman,
                mining: self.mining,
                hostile: false,
                hull: self.foreman.then_some("Orca"),
                scene,
            };
            speak(&mut self.st, &cx)
        }
    }

    #[test]
    fn every_category_has_at_least_fifteen_lines_and_known_slots() {
        for cat in Cat::ALL {
            let n = table().iter().filter(|(c, _)| *c == cat).count();
            assert!(n >= 15, "{}: {n}", cat.key());
        }
        for (_, l) in table() {
            assert!(!l.is_empty());
            let s = Slots {
                name: Some("x"),
                system: Some("x"),
                ore: Some("x"),
                hull: Some("x"),
            };
            assert!(fill(l, &s).is_some(), "unknown slot in {l:?}");
        }
    }

    #[test]
    fn a_line_with_an_unknown_slot_is_not_used() {
        let none = Slots {
            name: None,
            system: None,
            ore: None,
            hull: None,
        };
        assert_eq!(fill("hi {name}", &none), None);
        assert_eq!(fill("o/", &none).as_deref(), Some("o/"));
        let mut st = State::new(7);
        for _ in 0..200 {
            let l = pick(&mut st, Cat::Greet, &none).unwrap();
            assert!(!l.contains('{'), "{l}");
        }
    }

    #[test]
    fn classify_reads_player_talk() {
        assert_eq!(classify("hello o/", false), Some(Cat::ReplyGreet));
        assert_eq!(classify("Hey!", false), Some(Cat::ReplyGreet));
        assert_eq!(classify("gf", false), Some(Cat::Gf));
        assert_eq!(classify("o7", false), Some(Cat::Gf));
        assert_eq!(classify("boosts?", false), None, "only a foreman answers");
        assert_eq!(classify("any boosts around?", true), Some(Cat::Boosts));
        assert_eq!(
            classify("hey can anyone tell me where the best belt is", false),
            None,
            "a long question is not a greeting"
        );
        assert_eq!(classify("", true), None);
    }

    /// A human who says hello every 4 s for 40 minutes: whatever the
    /// chattiness, no two lines are closer than [`min_gap`] and no 10
    /// minutes hold more than [`cap`] (+1 for an urgent line, which a
    /// greeting is not).
    #[test]
    fn the_gap_and_the_cap_hold_at_every_chattiness() {
        for c in [0.0, 0.25, 0.5, 0.75, 1.0] {
            for seed in 0..12 {
                let t0 = Instant::now();
                let mut bot = Bot::new(seed, c);
                let mut scene = Scene::default();
                let mut says: Vec<Instant> = Vec::new();
                for s in 0..2400u64 {
                    let now = t0 + secs(s);
                    if s % 4 == 0 {
                        scene.heard.push(line(s + 1, HUMAN, "hello", now, true));
                    }
                    if bot.step(now, &mut scene, 1).is_some() {
                        says.push(now);
                    }
                    scene.heard.retain(|l| l.seq > s.saturating_sub(10));
                }
                for w in says.windows(2) {
                    assert!(w[1] - w[0] >= min_gap(c), "c={c} seed={seed}");
                }
                for (i, a) in says.iter().enumerate() {
                    let n = says[i..].iter().filter(|b| **b - *a < WINDOW).count();
                    assert!(n <= cap(c), "c={c} seed={seed}: {n} in a window");
                }
            }
        }
    }

    #[test]
    fn the_reply_waits_a_human_delay() {
        for seed in 0..40 {
            let t0 = Instant::now();
            let mut bot = Bot::new(seed, 1.0);
            let mut scene = Scene::default();
            scene.heard.push(line(1, HUMAN, "hi", t0, true));
            let mut first = None;
            for s in 0..60u64 {
                if bot.step(t0 + secs(s), &mut scene, 1).is_some() {
                    first = Some(s);
                    break;
                }
            }
            if let Some(s) = first {
                assert!(secs(s) >= reply_delay(1.0, 0.0), "seed {seed}: at {s} s");
                assert!(secs(s) <= reply_delay(1.0, 1.0) + secs(1), "seed {seed}");
            }
        }
    }

    #[test]
    fn some_bot_replies_to_a_hello_and_not_all_of_them() {
        let (mut spoke, mut silent) = (0, 0);
        for seed in 0..60 {
            let t0 = Instant::now();
            let mut bot = Bot::new(seed, 0.5);
            let mut scene = Scene::default();
            scene.heard.push(line(1, HUMAN, "hello", t0, true));
            let any = (0..40u64).any(|s| bot.step(t0 + secs(s), &mut scene, 1).is_some());
            if any {
                spoke += 1;
            } else {
                silent += 1;
            }
        }
        assert!(spoke > 5 && silent > 5, "{spoke} spoke, {silent} silent");
    }

    #[test]
    fn silence_with_no_human_in_local() {
        for seed in 0..20 {
            let t0 = Instant::now();
            let mut bot = Bot::new(seed, 1.0);
            bot.mining = true;
            let mut scene = Scene::default();
            for s in 0..3600u64 {
                let now = t0 + secs(s);
                if s % 30 == 0 {
                    // The room is full of talk and arrivals, but local_humans is 0.
                    scene.heard.push(line(s + 1, HUMAN, "hello", now, true));
                    scene.arrived.push(Arrival {
                        seq: s + 1,
                        char_id: HUMAN,
                        at: now,
                    });
                }
                assert_eq!(bot.step(now, &mut scene, 0), None, "seed {seed} at {s}");
            }
        }
    }

    #[test]
    fn a_human_who_arrives_is_greeted_at_most_once_per_fifteen_minutes_per_system() {
        let mut greeted_somewhere = false;
        for seed in 0..40 {
            let t0 = Instant::now();
            let mut bot = Bot::new(seed, 1.0);
            let mut scene = Scene::default();
            let mut says = 0;
            // Arrives, leaves and arrives again every two minutes for 14.
            for s in 0..(14 * 60u64) {
                let now = t0 + secs(s);
                if s % 120 == 0 {
                    scene.arrived.push(Arrival {
                        seq: s / 120 + 1,
                        char_id: HUMAN,
                        at: now,
                    });
                }
                if bot.step(now, &mut scene, 1).is_some() {
                    says += 1;
                }
            }
            assert!(says <= 1, "seed {seed}: {says} greetings");
            greeted_somewhere |= says == 1;
        }
        assert!(greeted_somewhere);
    }

    #[test]
    fn another_bots_greeting_cancels_ours() {
        let mut dropped = 0;
        for seed in 0..60 {
            let t0 = Instant::now();
            let mut bot = Bot::new(seed, 1.0);
            let mut scene = Scene::default();
            scene.arrived.push(Arrival {
                seq: 1,
                char_id: HUMAN,
                at: t0,
            });
            bot.step(t0, &mut scene, 1);
            // A bot's line comes in before ours is due.
            scene
                .heard
                .push(line(2, 970_000_900, "o/", t0 + secs(1), false));
            let said = (1..40u64).any(|s| bot.step(t0 + secs(s), &mut scene, 1).is_some());
            if !said && bot.st.greeted.contains_key(&(HUMAN, SYS)) {
                dropped += 1;
            }
            assert!(!said, "seed {seed} greeted over another bot");
        }
        assert!(dropped > 5, "{dropped}");
    }

    /// Several Orcas and a miner hear "boosts?": exactly one answers, and
    /// the miner never does.
    #[test]
    fn one_boost_answer_per_request() {
        for seed in 0..30u64 {
            let t0 = Instant::now();
            let asker = 970_000_200 + seed as i64;
            let mut bots: Vec<Bot> = (0..4)
                .map(|i| {
                    let mut b = Bot::new(seed * 10 + i, 0.5);
                    b.foreman = i != 3;
                    b
                })
                .collect();
            let mut scenes: Vec<Scene> = bots.iter().map(|_| Scene::default()).collect();
            for sc in &mut scenes {
                sc.heard.push(line(1, asker, "boosts?", t0, true));
            }
            let mut answers = 0;
            let mut next_seq = 2;
            for s in 0..40u64 {
                let now = t0 + secs(s);
                let mut said: Vec<(usize, String)> = Vec::new();
                for (i, b) in bots.iter_mut().enumerate() {
                    if let Some(t) = b.step(now, &mut scenes[i], 1) {
                        said.push((i, t));
                    }
                }
                for (i, _) in &said {
                    assert_ne!(*i, 3, "a miner answered a boost request");
                    answers += 1;
                    // The others hear it a moment later, as a bot's line.
                    for (j, sc) in scenes.iter_mut().enumerate() {
                        if j != *i {
                            sc.heard
                                .push(line(next_seq, 970_000_900 + *i as i64, "x", now, false));
                        }
                    }
                    next_seq += 1;
                }
            }
            assert_eq!(answers, 1, "seed {seed}");
        }
    }

    #[test]
    fn small_talk_only_while_mining_into_a_quiet_room() {
        let t0 = Instant::now();
        let mut bot = Bot::new(3, 1.0);
        bot.mining = true;
        let mut scene = Scene::default();
        let mut says = 0;
        for s in 0..3600u64 {
            if bot.step(t0 + secs(s), &mut scene, 1).is_some() {
                says += 1;
            }
        }
        assert!(
            says >= 1,
            "a miner in a quiet room says something in an hour"
        );
        assert!(says <= cap(1.0) * 6);
        let mut idle = Bot::new(3, 1.0);
        for s in 0..3600u64 {
            assert_eq!(idle.step(t0 + secs(s), &mut scene, 1), None);
        }
    }

    #[test]
    fn farewell_and_under_attack_are_governed() {
        let t0 = Instant::now();
        let scene = Scene {
            sociability: 1.0,
            local_humans: 1,
            ..Scene::default()
        };
        let mut st = State::new(5);
        let cx = |now| Ctx {
            now,
            system: Some(SYS),
            in_fleet: false,
            foreman: false,
            mining: false,
            hostile: false,
            hull: None,
            scene: &scene,
        };
        let first = under_attack(&mut st, &cx(t0), Some(HUMAN));
        assert!(first.is_some());
        assert_eq!(
            under_attack(&mut st, &cx(t0 + secs(1)), Some(HUMAN)),
            None,
            "inside the gap and the category cooldown"
        );
        assert!(under_attack(&mut st, &cx(t0 + secs(200)), None).is_some());
        let mut quiet = scene.clone();
        quiet.local_humans = 0;
        let mut st2 = State::new(5);
        let cq = Ctx {
            now: t0,
            system: Some(SYS),
            in_fleet: false,
            foreman: false,
            mining: false,
            hostile: false,
            hull: None,
            scene: &quiet,
        };
        assert_eq!(under_attack(&mut st2, &cq, None), None);
        assert_eq!(farewell(&mut st2, &cq), None);
        let said = (0..50u64)
            .filter_map(|i| farewell(&mut State::new(i), &cx(t0)))
            .count();
        assert!(said > 0, "a farewell is sometimes said");
    }

    #[test]
    fn lines_do_not_repeat_visibly() {
        let mut st = State::new(11);
        let none = Slots {
            name: None,
            system: None,
            ore: None,
            hull: None,
        };
        let mut last: Vec<String> = Vec::new();
        for _ in 0..100 {
            let l = pick(&mut st, Cat::Small, &none).unwrap();
            assert!(!last.contains(&l), "{l} repeated within {RECENT}");
            last.push(l);
            if last.len() > RECENT {
                last.remove(0);
            }
        }
    }

    /// CHATTER-2: with names, a system and an ore known, the slot lines are
    /// picked, filled, and never leak a brace; without, they stay out.
    #[test]
    fn slot_lines_are_used_once_their_slots_are_known() {
        let t0 = Instant::now();
        let mut scene = Scene {
            sociability: 1.0,
            local_humans: 1,
            ..Scene::default()
        };
        scene.names.insert(HUMAN, "Ada Test".to_owned());
        scene.system_name = Some("Testville".to_owned());
        scene.ore = Some("Oreium".to_owned());
        let cx = Ctx {
            now: t0,
            system: Some(SYS),
            in_fleet: false,
            foreman: false,
            mining: true,
            hostile: false,
            hull: None,
            scene: &scene,
        };
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..400u64 {
            let mut st = State::new(seed);
            for (cat, target) in [
                (Cat::Greet, Some(HUMAN)),
                (Cat::Gf, Some(HUMAN)),
                (Cat::Small, None),
                (Cat::Farewell, None),
            ] {
                let l = say(&mut st, &cx, cat, target).unwrap();
                assert!(!l.contains('{') && !l.contains('}'), "{l}");
                for w in ["Ada Test", "Testville", "Oreium"] {
                    if l.contains(w) {
                        seen.insert(w);
                    }
                }
            }
        }
        assert_eq!(seen.len(), 3, "each slot gets filled sometimes: {seen:?}");
        // Without them the same categories still speak, slot-free.
        let bare = Scene {
            sociability: 1.0,
            local_humans: 1,
            ..Scene::default()
        };
        let cx = Ctx { scene: &bare, ..cx };
        for seed in 0..100u64 {
            let l = say(&mut State::new(seed), &cx, Cat::Farewell, None).unwrap();
            assert!(!l.contains("Testville") && !l.contains('{'));
        }
    }

    fn ctx_of<'a>(now: Instant, scene: &'a Scene, hostile: bool) -> Ctx<'a> {
        Ctx {
            now,
            system: Some(SYS),
            in_fleet: false,
            foreman: false,
            mining: false,
            hostile,
            hull: None,
            scene,
        }
    }

    /// CHATTER-3: a fight against a human is remarked on once after the
    /// delay, as the right category, and a stale result is not.
    #[test]
    fn a_won_or_lost_fight_gets_one_line_after_the_delay() {
        let t0 = Instant::now();
        for won in [true, false] {
            let scene = Scene {
                sociability: 1.0,
                local_humans: 1,
                results: vec![Outcome {
                    seq: 1,
                    won,
                    at: t0,
                }],
                ..Scene::default()
            };
            let mut st = State::new(21);
            assert_eq!(speak(&mut st, &ctx_of(t0, &scene, false)), None);
            let want = if won { Cat::GfWon } else { Cat::GfLost };
            let due = Duration::from_secs_f64(RESULT_DELAY_S.1) + secs(1);
            let line = speak(&mut st, &ctx_of(t0 + due, &scene, false)).expect("a line");
            assert!(table().iter().any(|(c, l)| *c == want && *l == line));
            // The same result is not remarked on twice.
            assert_eq!(
                speak(&mut st, &ctx_of(t0 + due + secs(500), &scene, false)),
                None
            );
        }
        let old = Scene {
            sociability: 1.0,
            local_humans: 1,
            results: vec![Outcome {
                seq: 1,
                won: true,
                at: t0,
            }],
            ..Scene::default()
        };
        let late = t0 + RESULT_STALE + secs(5);
        let mut st = State::new(21);
        assert_eq!(speak(&mut st, &ctx_of(late, &old, false)), None);
        assert_eq!(speak(&mut st, &ctx_of(late + secs(30), &old, false)), None);
    }

    /// The governor holds the new categories to the cap and the cooldowns,
    /// and only a hostile bot ever taunts.
    #[test]
    fn taunts_are_hostile_only_and_the_new_lines_are_governed() {
        let t0 = Instant::now();
        let scene = Scene {
            sociability: 1.0,
            local_humans: 1,
            ..Scene::default()
        };
        for hostile in [false, true] {
            let mut st = State::new(8);
            let mut taunts = 0;
            for s in 0..7200u64 {
                if let Some(l) = speak(&mut st, &ctx_of(t0 + secs(s), &scene, hostile))
                    && table().iter().any(|(c, t)| *c == Cat::Taunt && *t == l)
                {
                    taunts += 1;
                }
                assert!(st.said_in_window(t0 + secs(s)) <= cap(1.0) + 1);
            }
            if hostile {
                assert!(taunts >= 1, "a hostile bot taunts sometimes");
                assert!(taunts as u64 <= 7200 / TAUNT_COOLDOWN_S + 1);
            } else {
                assert_eq!(taunts, 0, "a peaceful bot never taunts");
            }
        }
        let mut st = State::new(2);
        let cx = ctx_of(t0, &scene, false);
        assert!(mate_attacked(&mut st, &cx).is_some());
        let again = ctx_of(t0 + secs(MATE_COOLDOWN_S - 1), &scene, false);
        assert_eq!(mate_attacked(&mut st, &again), None);
        let quiet = Scene::default();
        assert_eq!(
            mate_attacked(&mut State::new(2), &ctx_of(t0, &quiet, false)),
            None
        );
    }
}
