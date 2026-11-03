//! Tests for the entosis engine: the pure score and window rules at scale 1
//! and at scale N, and the registry paths (activation, warm-up, fuel,
//! capture, block, windows, reinforcement, the guards).
//!
//! **Invented ids and numbers throughout.** The client's effect id, the
//! hub's type id and the group ids appear by name; every other number is the
//! fixture's own, and no `[A]` value is an expected value (they are named
//! constants of the module under test).

use super::*;
use crate::sov_hub::fixture as hubfx;
use crate::sov_hub::fixture::{ALICE, BOB, CAROL, DAVE, SYSTEM};

const SEC: i64 = 10_000_000;

/// The tests' fixed start: 2026-10-27 00:00 UTC as a FILETIME. No test reads
/// the system clock, so whether `t0` falls in a vulnerability window never
/// depends on the hour the suite runs at.
const T0: i64 = 13_437_532_800 * SEC;

/// A moment far after any real run: the GM's immediate edits (`/sov prime`)
/// are stamped with the server's own clock, and are all in force by then.
const FAR_FUTURE: i64 = T0 + 36_500 * 86_400 * SEC;
const HOUR: i64 = crate::seed::HOUR;
const DAY: i64 = crate::seed::DAY;

// ---------------------------------------------------------------------------
// The pure rules
// ---------------------------------------------------------------------------

fn set(v: &[i64]) -> BTreeSet<i64> {
    v.iter().copied().collect()
}

const A: i64 = 99_630_001;
const B: i64 = 99_630_002;
const D: i64 = 99_630_003;

fn scale(n: u32) -> GameClock {
    GameClock::new(n).expect("a valid scale")
}

#[test]
fn attackers_alone_score_up_owner_alone_scores_down_both_block() {
    let hub = Spec::HUB;
    // A held hub of D, bar at one half held by team -D.
    let held = Defender::Owned(D);
    let half = Some(-D);
    let up = decide(hub, held, 0.5, half, &set(&[A]));
    assert_eq!(
        (up.base, up.blocked, up.scoring, up.controlling),
        (1.0, false, Some(-D), Some(-D))
    );
    // Two attacking alliances do not block each other: they are one team.
    let up = decide(hub, held, 0.5, half, &set(&[A, B]));
    assert_eq!((up.base, up.blocked), (1.0, false));
    // The owner alone takes it down.
    let down = decide(hub, held, 0.5, half, &set(&[D]));
    assert_eq!(
        (down.base, down.blocked, down.scoring),
        (-1.0, false, Some(D))
    );
    // Both: a stalemate, the rate zero.
    let both = decide(hub, held, 0.5, half, &set(&[A, D]));
    assert_eq!((both.base, both.blocked), (0.0, true));
    // Nobody: the bar falls at the type's regen rate; at 0 it rests.
    let none = decide(hub, held, 0.5, half, &set(&[]));
    assert_eq!((none.base, none.blocked), (-HUB_REGEN_RATE, false));
    let rest = decide(hub, held, 0.0, None, &set(&[]));
    assert_eq!(
        (rest.base, rest.blocked, rest.controlling),
        (0.0, false, None)
    );
    // The owner has nothing to take down on an empty bar.
    let idle = decide(hub, held, 0.0, None, &set(&[D]));
    assert_eq!((idle.base, idle.blocked, idle.scoring), (0.0, false, None));
}

#[test]
fn a_neutral_hub_is_scored_by_one_alliance_and_blocked_by_two() {
    let hub = Spec::HUB;
    let n = Defender::Neutral;
    let one = decide(hub, n, 0.0, None, &set(&[A]));
    assert_eq!(
        (one.base, one.controlling, one.scoring, one.blocked),
        (1.0, Some(A), Some(A), false)
    );
    let two = decide(hub, n, 0.3, Some(A), &set(&[A, B]));
    assert_eq!(
        (two.base, two.blocked, two.controlling),
        (0.0, true, Some(A))
    );
    // A rival on a bar another alliance holds takes it down first.
    let rival = decide(hub, n, 0.3, Some(A), &set(&[B]));
    assert_eq!(
        (rival.base, rival.controlling, rival.scoring),
        (-1.0, Some(A), Some(B))
    );
    // A bar at 0 belongs to nobody, whoever held it.
    let fresh = decide(hub, n, 0.0, Some(A), &set(&[B]));
    assert_eq!((fresh.base, fresh.controlling), (1.0, Some(B)));
    let regen = decide(hub, n, 0.4, Some(A), &set(&[]));
    assert_eq!(regen.base, -HUB_REGEN_RATE);
}

/// The client's formula, `entosisController._CalculateCurrentScore`.
fn client_score(s0: f64, t0: i64, rate: f64, now: i64, capture: f64) -> f64 {
    (s0 + (now - t0) as f64 / capture / SEC as f64 * rate).clamp(0.0, 1.0)
}

#[test]
fn score_extrapolation_matches_the_client_formula() {
    for n in [1u32, 6, 60] {
        let clock = scale(n);
        let t0 = 134_000_000 * SEC;
        let out = step(None, t0, Spec::HUB, Defender::Neutral, &set(&[A]), clock);
        assert!(out.changed && out.done.is_none());
        let s = out.score;
        // The rate on the wire carries the scale, so the client's own
        // extrapolation counts down the compressed capture.
        assert_eq!(s.rate, f64::from(n));
        for secs in [0i64, 1, 7, 33] {
            let now = t0 + secs * SEC;
            assert!(
                (s.at(now, HUB_CAPTURE_SECONDS) - client_score(0.0, t0, s.rate, now, 595.0)).abs()
                    < 1e-12
            );
        }
        // Complete after 595 / n wall seconds, never sooner.
        let done = s.completes_at(HUB_CAPTURE_SECONDS).expect("rising");
        let wall = (done - t0) as f64 / SEC as f64;
        assert!((wall - 595.0 / f64::from(n)).abs() < 1e-6, "{n}: {wall}");
        let just_before = step(
            Some(s),
            done - SEC / 100,
            Spec::HUB,
            Defender::Neutral,
            &set(&[A]),
            clock,
        );
        assert!(just_before.done.is_none() && !just_before.changed);
        let at = step(
            Some(s),
            done,
            Spec::HUB,
            Defender::Neutral,
            &set(&[A]),
            clock,
        );
        assert_eq!(at.done, Some(Done::Full { team: Some(A) }));
        assert_eq!(at.score, Score::idle(done));
    }
}

#[test]
fn an_unchanged_answer_keeps_the_stored_tuple_and_pushes_nothing() {
    let clock = scale(1);
    let t0 = 134_000_000 * SEC;
    let s = step(None, t0, Spec::HUB, Defender::Neutral, &set(&[A]), clock).score;
    for later in [1, 10, 100] {
        let again = step(
            Some(s),
            t0 + later * SEC,
            Spec::HUB,
            Defender::Neutral,
            &set(&[A]),
            clock,
        );
        assert!(!again.changed);
        assert_eq!(again.score, s, "the base (s0, t0) is not moved");
    }
    // Nothing scoring and nothing stored is not an edge either.
    let nothing = step(None, t0, Spec::HUB, Defender::Neutral, &set(&[]), clock);
    assert!(!nothing.changed && nothing.done.is_none());
}

#[test]
fn a_block_freezes_the_bar_and_the_score_resumes_from_it() {
    let clock = scale(1);
    let t0 = 134_000_000 * SEC;
    let rising = step(None, t0, Spec::HUB, Defender::Neutral, &set(&[A]), clock).score;
    let t1 = t0 + 100 * SEC;
    let blocked = step(
        Some(rising),
        t1,
        Spec::HUB,
        Defender::Neutral,
        &set(&[A, B]),
        clock,
    );
    assert!(blocked.changed);
    let b = blocked.score;
    assert!(b.blocked && b.rate == 0.0);
    let held = 100.0 / HUB_CAPTURE_SECONDS;
    assert!((b.s0 - held).abs() < 1e-9, "{}", b.s0);
    // A block never completes, however long it lasts.
    assert_eq!(b.completes_at(HUB_CAPTURE_SECONDS), None);
    let later = step(
        Some(b),
        t1 + 10_000 * SEC,
        Spec::HUB,
        Defender::Neutral,
        &set(&[A, B]),
        clock,
    );
    assert!(!later.changed && later.done.is_none());
    // B leaves: A resumes from where the bar stood.
    let t2 = t1 + 50 * SEC;
    let resumed = step(Some(b), t2, Spec::HUB, Defender::Neutral, &set(&[A]), clock);
    assert!(resumed.changed && resumed.score.rate == 1.0);
    assert!((resumed.score.s0 - held).abs() < 1e-9);
}

#[test]
fn an_unattended_bar_falls_at_the_regen_rate_and_ends_idle() {
    let clock = scale(1);
    let t0 = 134_000_000 * SEC;
    let rising = step(None, t0, Spec::HUB, Defender::Owned(D), &set(&[A]), clock).score;
    let t1 = t0 + 200 * SEC;
    let down = step(
        Some(rising),
        t1,
        Spec::HUB,
        Defender::Owned(D),
        &set(&[]),
        clock,
    );
    assert!(down.changed);
    assert_eq!(down.score.rate, -HUB_REGEN_RATE);
    let reaches = down
        .score
        .completes_at(HUB_CAPTURE_SECONDS)
        .expect("falling");
    // From 200/595 at 0.2 capture-times per capture-time.
    let want = t1 + ((200.0 / 595.0) / HUB_REGEN_RATE * 595.0 * SEC as f64) as i64;
    assert!((reaches - want).abs() < SEC / 1000, "{reaches} {want}");
    // At the end the answer is an idle tuple, pushed so the client's timer
    // stops (the client ignores None).
    let end = step(
        Some(down.score),
        reaches + SEC,
        Spec::HUB,
        Defender::Owned(D),
        &set(&[]),
        clock,
    );
    assert!(end.changed && end.done.is_none());
    assert_eq!(end.score.rate, 0.0);
    assert_eq!(end.score.controlling, None);
    assert_eq!(end.score.s0, 0.0);
}

#[test]
fn a_regress_to_zero_by_the_owner_ends_idle_too() {
    let clock = scale(6);
    let t0 = 134_000_000 * SEC;
    let up = step(None, t0, Spec::HUB, Defender::Owned(D), &set(&[A]), clock).score;
    let t1 = t0 + 20 * SEC;
    let down = step(
        Some(up),
        t1,
        Spec::HUB,
        Defender::Owned(D),
        &set(&[D]),
        clock,
    );
    assert_eq!(down.score.rate, -6.0);
    assert_eq!(down.score.scoring, Some(D));
    let end = down
        .score
        .completes_at(HUB_CAPTURE_SECONDS)
        .expect("falling");
    let idle = step(
        Some(down.score),
        end + SEC,
        Spec::HUB,
        Defender::Owned(D),
        &set(&[D]),
        clock,
    );
    assert_eq!(idle.score.rate, 0.0);
    assert!(idle.changed && idle.done.is_none());
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[test]
fn a_window_is_centred_on_the_prime_hour_and_repeats_every_game_day() {
    let clock = scale(1);
    let day0 = 150_000 * DAY;
    let len = 6 * HOUR;
    // Prime hour 18: centre 18:30, so 15:30 to 21:30.
    let (cur, next) = window_around(18, len, day0 + 16 * HOUR, clock);
    assert_eq!(cur.start, day0 + 15 * HOUR + HOUR / 2);
    assert_eq!(cur.end, day0 + 21 * HOUR + HOUR / 2);
    assert_eq!(next.start, cur.start + DAY);
    // Before it, "current" is the previous day's window, already over.
    let (cur, next) = window_around(18, len, day0 + 10 * HOUR, clock);
    assert_eq!(cur.start, day0 - DAY + 15 * HOUR + HOUR / 2);
    assert!(cur.end < day0 + 10 * HOUR);
    assert_eq!(next.start, day0 + 15 * HOUR + HOUR / 2);
    // A long window around hour 0 wraps back across midnight.
    let (cur, _) = window_around(0, 18 * HOUR, day0 + 2 * HOUR, clock);
    assert_eq!(cur.start, day0 - 8 * HOUR - HOUR / 2);
    assert!(day0 + 2 * HOUR <= cur.end);
}

#[test]
fn a_compressed_game_day_compresses_the_window_and_its_period() {
    for n in [2u32, 6, 24] {
        let clock = scale(n);
        let len = 6 * HOUR;
        let (cur, next) = window_around(18, len, 150_000 * DAY + 12_345 * SEC, clock);
        assert_eq!(next.start - cur.start, clock.day(), "scale {n}");
        assert_eq!(cur.end - cur.start, clock.span(len), "scale {n}");
        // The first start at or after a time is never before it.
        let t = 150_000 * DAY + 777 * SEC;
        let s = window_start_at_or_after(18, len, t, clock);
        assert!(s >= t && s - t <= clock.day(), "scale {n}");
    }
}

#[test]
fn the_window_follows_the_adm_and_never_fills_the_day() {
    let clock = scale(1);
    for adm in [1.0, 2.5, 6.0] {
        let len = crate::sov_index::vulnerability_window(adm);
        let (cur, _) = window_around(3, len, 150_000 * DAY, clock);
        assert_eq!(cur.end - cur.start, len);
        assert!(len < DAY);
    }
    // A higher ADM is a shorter window.
    assert!(
        crate::sov_index::vulnerability_window(6.0) < crate::sov_index::vulnerability_window(1.0)
    );
}

// ---------------------------------------------------------------------------
// The registry paths
// ---------------------------------------------------------------------------

const LINK_TYPE: i64 = 990_630_001;
const STRONTIUM: i64 = 990_630_002;
const LINK_GROUP: i64 = 990_630;
const LINK_ALICE: i64 = 100_630_001;
const LINK_DAVE: i64 = 100_630_002;
const FUEL_ALICE: i64 = 100_630_003;
const FUEL_DAVE: i64 = 100_630_004;
const LINK_BOB: i64 = 100_630_005;
const FUEL_BOB: i64 = 100_630_006;
const LINK_CAROL: i64 = 100_630_007;
const FUEL_CAROL: i64 = 100_630_008;
const LINK_ALICE_2: i64 = 100_630_009;
const EFFECT_NAME: &str = "fixtureEntosis";
/// The invented link: its range (m) and its cycle (ms).
const LINK_RANGE: f64 = 20_000.0;
const LINK_MS: i64 = 4_000;
/// `flagHiSlot0` **[V]**.
const HI_SLOT_0: i64 = 27;
/// Fuel in a hold: plenty.
const FUEL_PLENTY: i64 = 5_000;

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let rows = format!(
        "{}type\t{}\t{}\t{}\t1\t1\t10.0\t0.0\t1000.0\t20000.0\t0\t\t\tInvented Hub Row\n\
         type\t{STRONTIUM}\t{}\t4\t1\t40\t5.0\t0.0\t1.0\t1.0\t0\t\t\tInvented Strontium\n\
         type\t{LINK_TYPE}\t{LINK_GROUP}\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{EFFECT_ENTOSIS_LINK}\t12,16,{EFFECT_ENTOSIS_LINK}\t6=0.0;30=1.0;50=5.0;54={LINK_RANGE};73={LINK_MS}.0;{ATTR_FUEL_TYPE}={STRONTIUM}.0;{ATTR_FUEL_QUANTITY}=1.0\tInvented Link\n\
         effect\t{EFFECT_ENTOSIS_LINK}\t2\t0\t73\t6\t54\t0\t0\t{EFFECT_NAME}\teffects.EntosisBeam\n",
        crate::typedata::FIXTURE,
        sov::TYPE_INFRASTRUCTURE_HUB,
        sov_hub::GROUP_INFRASTRUCTURE_HUB,
        sov_hub::CATEGORY_SOVEREIGNTY_STRUCTURE,
        // Strontium is plain cargo, not a fuel block.
        LINK_GROUP + 1,
    );
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    let sys = |id: i64, sec: &str, name: &str| {
        format!("{id}\t10630001\t20630001\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\t{name}")
    };
    let u = crate::celestial::Universe::parse(
        &[
            sys(SYSTEM, "-0.45", "Invented Null"),
            sys(hubfx::HIGH, "0.9", "Invented High"),
        ],
        &["10630001\t\t\t\t\tInvented Region".to_owned()],
        &["20630001\t10630001\t\t\t\tInvented Constellation".to_owned()],
    )
    .expect("the test universe parses");
    c.insert("universe", u);
    c
}

fn item(
    c: &crate::content::Content,
    item_id: i64,
    type_id: i64,
    owner: i64,
    flag_id: i64,
    quantity: i64,
    singleton: bool,
) -> crate::item::Item {
    let t = crate::item::find_in(c, type_id).expect("a test type");
    crate::item::Item {
        item_id,
        type_id,
        owner_id: owner,
        location_id: hubfx::ship(owner),
        flag_id,
        quantity,
        group_id: t.group_id,
        category_id: t.category_id,
        singleton,
    }
}

struct World {
    reg: Registry,
    /// Alice's and Dave's alliances.
    a: i64,
    d: i64,
    /// The hub, neutral.
    hub: i64,
    /// The test's "now".
    t0: i64,
}

/// Four pilots at the hub, each with a link and fuel (Carol's alliance-less),
/// the hub launched by the GM and neutral. Every push drained.
fn world() -> World {
    let pilot = |id: i64, name: &str| crate::model::Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: hubfx::ship(id),
        solar_system_id: Some(SYSTEM),
        ..crate::model::Character::default()
    };
    let content = std::sync::Arc::new(content());
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![
            pilot(ALICE, "Alice"),
            pilot(BOB, "Bob"),
            pilot(CAROL, "Carol"),
            pilot(DAVE, "Dave"),
        ],
        celestials: crate::celestial::Catalogue::parse(&format!(
            "star\t{}\t990620010\t{}\t{}\t{}\t{}\t{SYSTEM}\t0\t0\n",
            hubfx::STAR,
            hubfx::STAR_RADIUS,
            hubfx::STAR_AT[0],
            hubfx::STAR_AT[1],
            hubfx::STAR_AT[2]
        )),
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content.clone());
    let mut items = reg.store().items().to_vec();
    for (link, fuel, who) in [
        (LINK_ALICE, FUEL_ALICE, ALICE),
        (LINK_DAVE, FUEL_DAVE, DAVE),
        (LINK_BOB, FUEL_BOB, BOB),
        (LINK_CAROL, FUEL_CAROL, CAROL),
    ] {
        items.push(item(&content, link, LINK_TYPE, who, HI_SLOT_0, 1, true));
        if who == ALICE {
            // A second link in the next slot, for the one-link rule.
            items.push(item(
                &content,
                LINK_ALICE_2,
                LINK_TYPE,
                who,
                HI_SLOT_0 + 1,
                1,
                true,
            ));
        }
        items.push(item(
            &content,
            fuel,
            STRONTIUM,
            who,
            sov_hub::FLAG_CARGO,
            FUEL_PLENTY,
            false,
        ));
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_631_000,
        ..crate::store::Snapshot::default()
    });
    crate::service::sov_hub::test_corp(&mut reg, hubfx::CORP, ALICE, &[(ALICE, 0), (BOB, 0)]);
    crate::service::sov_hub::test_corp(&mut reg, hubfx::LONE, CAROL, &[(CAROL, 0)]);
    crate::service::sov_hub::test_corp(&mut reg, hubfx::RIVAL, DAVE, &[(DAVE, 0)]);
    let mut st = crate::alliance::Alliances::load(reg.deltas());
    let a = st
        .create(hubfx::CORP, ALICE, "Invented Sov Holders", "ISH", "", "", 1)
        .expect("an alliance");
    let d = st
        .create(hubfx::RIVAL, DAVE, "Invented Rivals", "IRV", "", "", 1)
        .expect("a second alliance");
    st.save(reg.deltas_mut());
    for (who, corp, alliance) in [
        (ALICE, hubfx::CORP, Some(a)),
        (BOB, hubfx::CORP, Some(a)),
        (CAROL, hubfx::LONE, None),
        (DAVE, hubfx::RIVAL, Some(d)),
    ] {
        let c = reg.character_mut(who).expect("a pilot");
        c.corporation_id = corp;
        c.alliance_id = alliance;
    }
    let spot = hubfx::launch_spot(&reg);
    for (n, who) in [ALICE, BOB, CAROL, DAVE].into_iter().enumerate() {
        hubfx::place_at(&mut reg, who, spot);
        let n = n as i64 + 1;
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: n,
            session_id: n,
            user_id: n,
            character_id: Some(who),
        });
    }
    let hub = sov_hub::ensure_hub(&mut reg, SYSTEM).expect("the GM hub");
    // At the hub, in range of a link.
    let at = hubfx::fuel_spot(&reg);
    for who in [ALICE, BOB, CAROL, DAVE] {
        hubfx::place_at(&mut reg, who, at);
    }
    let _ = reg.take_pending();
    let _ = hubfx::due(&mut reg);
    World {
        reg,
        a,
        d,
        hub,
        t0: T0,
    }
}

impl World {
    fn clock(&mut self, n: u32) {
        *self.reg.ext_mut::<GameClock>() = scale(n);
    }

    /// The world clock's stages that matter here, at `t`.
    fn advance(&mut self, t: i64) {
        crate::modules::tick(&mut self.reg, t);
        tick(&mut self.reg, Instant::now(), t);
    }

    /// Lock the hub and start `who`'s link on it at `t`.
    fn link(&mut self, who: i64, t: i64) -> Result<PyValue, ServiceError> {
        let module = match who {
            ALICE => LINK_ALICE,
            DAVE => LINK_DAVE,
            BOB => LINK_BOB,
            _ => LINK_CAROL,
        };
        self.link_with(who, module, t)
    }

    fn link_with(&mut self, who: i64, module: i64, t: i64) -> Result<PyValue, ServiceError> {
        self.reg.locks_mut().add(hubfx::ship(who), self.hub);
        crate::modules::activate(
            &mut self.reg,
            Some(who),
            module,
            EFFECT_NAME,
            Some(self.hub),
            1000,
            t,
        )
    }

    /// The GM capture for Alice's alliance, with the prime hour found that
    /// puts `t0` inside (or outside) the window on the world's clock (at a
    /// compressed scale the window is not aligned with the wall hour).
    fn held_by_a_where(&mut self, inside: bool) {
        let exec = hubfx::owner(&self.reg, self.a);
        sov_hub::capture(&mut self.reg, SYSTEM, exec).expect("captured");
        let clock = crate::game_clock::of(&self.reg);
        for hour in 0..24 {
            sov::set_prime_hour(self.reg.deltas_mut(), self.a, hour, self.t0, 0).expect("hour");
            let c = self.claim();
            let ph = phase(self.reg.deltas(), &c, self.t0, clock);
            if matches!(ph, Phase::Vulnerable(_)) == inside
                && matches!(ph, Phase::Vulnerable(_) | Phase::Invulnerable(_))
            {
                return;
            }
        }
        panic!("no prime hour puts the hub inside = {inside}");
    }

    fn claim(&self) -> Claim {
        sov::claim(self.reg.deltas(), SYSTEM).expect("a hub row")
    }

    fn score(&self) -> Option<Score> {
        score_row(self.reg.deltas(), self.hub).map(|(_, s)| s)
    }

    fn fuel_left(&self, who: i64) -> i64 {
        self.reg
            .store()
            .items()
            .iter()
            .filter(|i| {
                i.type_id == STRONTIUM && i.location_id == hubfx::ship(who) && i.owner_id == who
            })
            .map(|i| i.quantity)
            .sum()
    }

    fn pushes(&mut self) -> Vec<hubfx::Sent> {
        let mut v = hubfx::pending(&mut self.reg);
        v.extend(hubfx::due(&mut self.reg));
        v
    }

    fn period(&self) -> i64 {
        LINK_MS * crate::modules::MSEC
    }
}

/// The hour (UTC) a FILETIME is in.
fn hour_of(t: i64) -> i64 {
    (t / HOUR).rem_euclid(24)
}

fn named<'a>(sent: &'a [hubfx::Sent], name: &str) -> Vec<&'a hubfx::Sent> {
    sent.iter().filter(|(n, _, _)| *n == name).collect()
}

/// Every string in a value, joined: the text a dialog or a push carries
/// (the `Debug` of a byte string is its numbers).
fn flat(v: &PyValue) -> String {
    match v {
        PyValue::Bytes(b) => format!("{} ", String::from_utf8_lossy(b)),
        PyValue::Unicode(s) => format!("{s} "),
        PyValue::Int(n) => format!("{n} "),
        PyValue::Tuple(l) | PyValue::List(l) => l.iter().map(flat).collect(),
        PyValue::Dict(d) => d.iter().map(|(a, b)| flat(a) + &flat(b)).collect(),
        PyValue::Instance { state, .. } => flat(state),
        PyValue::ObjectEx {
            header, list, dict, ..
        } => {
            flat(header)
                + &list.iter().map(flat).collect::<String>()
                + &dict
                    .iter()
                    .map(|(a, b)| flat(a) + &flat(b))
                    .collect::<String>()
        }
        _ => String::new(),
    }
}

fn refusal_text(e: &ServiceError) -> String {
    match e {
        ServiceError::Raise(v) => flat(v),
        other => format!("{other:?}"),
    }
}

fn sent_text(s: &hubfx::Sent) -> String {
    s.1.iter().map(flat).collect()
}

#[test]
fn entosis_needs_an_entosis_target_in_range_and_an_alliance() {
    let mut w = world();
    let t = w.t0;
    // Carol has no alliance.
    let e = w.link(CAROL, t).expect_err("no alliance");
    assert!(refusal_text(&e).contains("alliance"), "{e:?}");
    // Not locked.
    let e = crate::modules::activate(
        &mut w.reg,
        Some(ALICE),
        LINK_ALICE,
        EFFECT_NAME,
        Some(w.hub),
        1000,
        t,
    )
    .expect_err("not locked");
    assert!(
        refusal_text(&e).contains("DeniedActivateTargetNotPresent"),
        "{e:?}"
    );
    // Not a hub: another pilot's ship is no entosis target.
    w.reg.locks_mut().add(hubfx::ship(ALICE), hubfx::ship(DAVE));
    let e = crate::modules::activate(
        &mut w.reg,
        Some(ALICE),
        LINK_ALICE,
        EFFECT_NAME,
        Some(hubfx::ship(DAVE)),
        1000,
        t,
    )
    .expect_err("a ship");
    assert!(
        refusal_text(&e).contains("DeniedActivateTargetModuleDisallowed"),
        "{e:?}"
    );
    // Out of range: 100 km off the hub's surface.
    let far = hubfx::off(hubfx::fuel_spot(&w.reg), [0.0, 0.0, 100_000.0]);
    hubfx::place_at(&mut w.reg, ALICE, far);
    let e = w.link(ALICE, t).expect_err("too far");
    assert!(refusal_text(&e).contains("TargetTooFar"), "{e:?}");
    assert!(w.reg.modules().running(LINK_ALICE).is_none());
    // In range, with an alliance, locked: it starts.
    let near = hubfx::fuel_spot(&w.reg);
    hubfx::place_at(&mut w.reg, ALICE, near);
    w.link(ALICE, t).expect("starts");
    assert!(w.reg.modules().running(LINK_ALICE).is_some());
    assert!(ship_is_linking(&w.reg, hubfx::ship(ALICE)));
}

#[test]
fn first_cycle_is_warmup_and_scoring_starts_at_its_end() {
    let mut w = world();
    w.clock(60);
    w.link(ALICE, w.t0).expect("starts");
    let period = w.period();
    // Inside the warm-up: a link is running, nothing scores.
    w.advance(w.t0 + period - 1);
    let links = links_on(&w.reg, w.hub);
    assert_eq!(links.len(), 1);
    assert!(!links[0].scoring(w.t0 + period - 1));
    assert!(scorers(&links, w.t0 + period - 1).is_empty());
    assert_eq!(w.score(), None, "no score before the warm-up ends");
    // At the end of the first cycle it scores.
    let t = w.t0 + period + SEC / 10;
    w.advance(t);
    let s = w.score().expect("a score row");
    assert_eq!(s.rate, 60.0, "+1 at scale 60");
    assert_eq!(s.controlling, Some(w.a));
    assert_eq!(s.scoring, Some(w.a));
    assert!(!s.blocked);
    assert_eq!(s.t0, t);
    // The slim carries it as the client reads it.
    let PyValue::Tuple(v) = slim_score(w.reg.deltas(), w.hub) else {
        panic!("a tuple")
    };
    assert_eq!(v[0], PyValue::Int(w.a));
    assert_eq!(v[1], PyValue::Int(w.a));
    assert_eq!(v[3], PyValue::Int(NEUTRAL_TEAM));
    let PyValue::Tuple(attrs) = &v[2] else {
        panic!("attributes")
    };
    assert_eq!(
        attrs[..],
        [
            PyValue::Real(0.0),
            PyValue::Int(t),
            PyValue::Real(60.0),
            PyValue::Bool(false)
        ]
    );
    // And a push went out on the edge: the hub's slim changed.
    let sent = w.pushes();
    assert!(
        named(&sent, "DoDestinyUpdate")
            .iter()
            .any(|s| sent_text(s).contains("entosis_score")),
        "{sent:?}"
    );
}

#[test]
fn each_cycle_consumes_one_fuel_unit_and_stops_when_empty() {
    let mut w = world();
    w.clock(1);
    // A hold with three units.
    w.reg
        .apply_items(&[crate::item::Edit::Consume {
            item_id: FUEL_ALICE,
            quantity: FUEL_PLENTY - 3,
        }])
        .expect("trimmed");
    assert_eq!(w.fuel_left(ALICE), 3);
    w.link(ALICE, w.t0).expect("starts with fuel for a cycle");
    let p = w.period();
    // Cycle 1 paid on the first tick, 2 and 3 as they begin.
    w.advance(w.t0 + SEC);
    assert_eq!(w.fuel_left(ALICE), 2);
    w.advance(w.t0 + p + SEC);
    assert_eq!(w.fuel_left(ALICE), 1);
    w.advance(w.t0 + 2 * p + SEC);
    assert_eq!(w.fuel_left(ALICE), 0);
    assert!(w.reg.modules().running(LINK_ALICE).is_some());
    // The fourth cycle has nothing to burn: the module stops.
    w.advance(w.t0 + 3 * p + SEC);
    assert!(w.reg.modules().running(LINK_ALICE).is_none(), "stopped");
    // The stop went to the pilot with its reason.
    let sent = w.pushes();
    assert!(
        named(&sent, "OnGodmaShipEffect")
            .iter()
            .any(|s| sent_text(s).contains("out of fuel")),
        "{sent:?}"
    );
    // And a new activation is refused for want of fuel.
    let e = w.link(ALICE, w.t0 + 4 * p).expect_err("no fuel");
    assert!(refusal_text(&e).contains("fuel"), "{e:?}");
}

#[test]
fn neutral_hub_capture_by_one_alliance_claims_for_its_executor_corp() {
    for n in [1u32, 60] {
        let mut w = world();
        w.clock(n);
        let exec = hubfx::owner(&w.reg, w.a).1;
        w.link(ALICE, w.t0).expect("starts");
        let p = w.period();
        // The capture takes 595 / n seconds after the warm-up.
        let capture = (HUB_CAPTURE_SECONDS / f64::from(n) * SEC as f64) as i64;
        let scoring_from = w.t0 + p;
        let mut t = w.t0;
        let step_by = if n == 1 { 5 * SEC } else { SEC / 2 };
        while t < scoring_from + capture - 2 * step_by {
            t += step_by;
            w.advance(t);
        }
        assert_eq!(
            w.claim().holder(),
            None,
            "not claimed yet at scale {n}, t = {}",
            (t - w.t0) / SEC
        );
        while t < scoring_from + capture + 3 * step_by {
            t += step_by;
            w.advance(t);
        }
        let c = w.claim();
        assert_eq!(c.holder(), Some(w.a), "scale {n}");
        assert_eq!(c.corporation_id, exec, "the executor corp holds it");
        assert_eq!(c.state, HubState::Claimed);
        assert!(c.claim_time.is_some());
        // The bar is idle, not None, and the link has stopped.
        let s = w.score().expect("an idle row");
        assert_eq!((s.rate, s.controlling, s.s0), (0.0, None, 0.0));
        assert!(w.reg.modules().running(LINK_ALICE).is_none());
        // The claim was announced the way D1 does and the holder told.
        let sent = w.pushes();
        assert!(
            !named(&sent, "ProcessSovStatusChanged").is_empty(),
            "{sent:?}"
        );
        assert!(!named(&sent, "OnSolarSystemSovStructuresUpdated").is_empty());
        let got = named(&sent, "OnNotificationReceived");
        assert!(
            got.iter()
                .any(|(_, a, _)| a.get(1) == Some(&PyValue::Int(kind::ALLIANCE_CLAIM_ACQUIRED))),
            "{got:?}"
        );
        // The hub's slim names the new holder and the idle tuple.
        let PyValue::Tuple(v) = slim_score(w.reg.deltas(), w.hub) else {
            panic!()
        };
        assert_eq!(v[3], PyValue::Int(w.a), "the defender is the holder now");
    }
}

#[test]
fn gm_capture_and_entosis_completion_run_the_same_function() {
    // Both end in `claim_neutral`; the GM line is its thin wrapper.
    let mut w = world();
    let exec = hubfx::owner(&w.reg, w.d);
    let change = sov_hub::capture(&mut w.reg, SYSTEM, exec).expect("captured");
    assert_eq!(change.new, Some(w.d));
    assert_eq!(w.claim().holder(), Some(w.d));
    // It refuses a held hub, as completion would never ask it to.
    let a = hubfx::owner(&w.reg, w.a);
    let again = claim_neutral(&mut w.reg, SYSTEM, a);
    assert!(again.expect_err("held").contains("held by alliance"));
    // A GM capture stamps the idle score too: the slim is not None.
    assert!(matches!(
        slim_score(w.reg.deltas(), w.hub),
        PyValue::Tuple(_)
    ));
}

#[test]
fn two_alliances_on_a_neutral_hub_block_it() {
    let mut w = world();
    w.clock(60);
    w.link(ALICE, w.t0).expect("Alice starts");
    w.link(DAVE, w.t0).expect("Dave starts");
    let p = w.period();
    w.advance(w.t0 + p + SEC / 10);
    let s = w.score().expect("a row");
    assert!(s.blocked, "{s:?}");
    assert_eq!(s.rate, 0.0);
    // However long it goes on, nobody claims.
    for k in 1..=20 {
        w.advance(w.t0 + p + k * 5 * SEC);
    }
    assert_eq!(w.claim().holder(), None);
    // Dave stops; Alice resumes and completes.
    crate::modules::stop(&mut w.reg, LINK_DAVE, w.t0 + p + 100 * SEC, None);
    let mut t = w.t0 + p + 100 * SEC;
    for _ in 0..40 {
        t += SEC / 2;
        w.advance(t);
    }
    assert_eq!(w.claim().holder(), Some(w.a));
}

#[test]
fn owned_hub_is_refused_outside_the_window() {
    let mut w = world();
    // Alice's alliance holds it; its prime hour is twelve hours away.
    w.held_by_a_where(false);
    let c = w.claim();
    let ph = phase(w.reg.deltas(), &c, w.t0, crate::game_clock::of(&w.reg));
    assert!(matches!(ph, Phase::Invulnerable(_)), "{ph:?}");
    let e = w.link(DAVE, w.t0).expect_err("outside the window");
    let text = refusal_text(&e);
    assert!(text.contains("invulnerable"), "{text}");
    assert!(w.reg.modules().running(LINK_DAVE).is_none());
    // Inside the window it starts.
    sov::set_prime_hour(w.reg.deltas_mut(), w.a, hour_of(w.t0), w.t0, 0).expect("hour");
    let ph = phase(w.reg.deltas(), &c, w.t0, crate::game_clock::of(&w.reg));
    assert!(matches!(ph, Phase::Vulnerable(_)), "{ph:?}");
    w.link(DAVE, w.t0).expect("inside the window");
    // The owner has nothing to defend until somebody attacks.
    let e = w.link(ALICE, w.t0).expect_err("nothing to defend");
    assert!(refusal_text(&e).contains("nobody is attacking"), "{e:?}");
}

#[test]
fn a_link_that_never_scored_stops_when_the_window_closes() {
    let mut w = world();
    w.clock(1);
    w.held_by_a_where(true);
    w.link(DAVE, w.t0).expect("in the window");
    // Move the prime hour away: the window closes under the warming link.
    sov::set_prime_hour(w.reg.deltas_mut(), w.a, (hour_of(w.t0) + 12) % 24, w.t0, 0).expect("hour");
    let t = w.t0 + w.period() + SEC;
    w.advance(t);
    assert!(w.reg.modules().running(LINK_DAVE).is_none(), "stopped");
    // The capture left an idle row; nothing ever moved it.
    let s = w.score().expect("the idle row");
    assert_eq!((s.rate, s.s0, s.controlling), (0.0, 0.0, None));
}

#[test]
fn a_capture_in_progress_goes_on_past_the_window_and_either_side_may_link() {
    let mut w = world();
    w.clock(1);
    w.held_by_a_where(true);
    w.link(DAVE, w.t0).expect("in the window");
    let p = w.period();
    w.advance(w.t0 + p + SEC);
    assert!(w.score().expect("scoring").rate > 0.0);
    // Close the window: the capture is in overtime and continues.
    sov::set_prime_hour(w.reg.deltas_mut(), w.a, (hour_of(w.t0) + 12) % 24, w.t0, 0).expect("hour");
    let t = w.t0 + p + 30 * SEC;
    w.advance(t);
    let c = w.claim();
    let ph = phase(w.reg.deltas(), &c, t, crate::game_clock::of(&w.reg));
    assert!(matches!(ph, Phase::Overtime(_)), "{ph:?}");
    assert!(
        w.reg.modules().running(LINK_DAVE).is_some(),
        "still linking"
    );
    assert!(w.score().expect("scoring").rate > 0.0);
    // Alice may link against it now: that blocks the capture.
    w.link(ALICE, t).expect("the defender may link in overtime");
    w.advance(t + p + SEC);
    assert!(w.score().expect("blocked").blocked);
}

#[test]
fn the_defender_alone_takes_a_bar_down_and_it_ends_idle() {
    let mut w = world();
    w.clock(60);
    w.held_by_a_where(true);
    w.link(DAVE, w.t0)
        .unwrap_or_else(|e| panic!("attacker: {}", refusal_text(&e)));
    let p = w.period();
    let mut t = w.t0 + p;
    for _ in 0..6 {
        t += SEC / 2;
        w.advance(t);
    }
    let up = w.score().expect("scoring");
    assert!(up.rate > 0.0 && up.controlling == Some(-w.a));
    // Dave stops; Alice defends alone, so the bar falls at the full rate.
    crate::modules::stop(&mut w.reg, LINK_DAVE, t, None);
    w.link(ALICE, t).expect("defender with a bar to take down");
    t += p + SEC / 2;
    w.advance(t);
    let down = w.score().expect("a row");
    assert!(down.rate < 0.0, "{down:?}");
    assert_eq!(down.scoring, Some(w.a));
    // Down to 0, then an idle tuple (a row at rate 0).
    for _ in 0..60 {
        t += SEC / 2;
        w.advance(t);
    }
    let end = w.score().expect("a row");
    assert_eq!((end.rate, end.s0, end.controlling), (0.0, 0.0, None));
    assert_eq!(w.claim().state, HubState::Claimed, "not reinforced");
}

#[test]
fn completed_attack_reinforces_with_campaign_state_and_pushes() {
    for n in [1u32, 60] {
        let mut w = world();
        w.clock(n);
        w.held_by_a_where(true);
        let _ = w.pushes();
        w.link(DAVE, w.t0).expect("attacker");
        let p = w.period();
        let capture = (HUB_CAPTURE_SECONDS / f64::from(n) * SEC as f64) as i64;
        let step_by = if n == 1 { 5 * SEC } else { SEC / 2 };
        let mut t = w.t0;
        while t < w.t0 + p + capture + 3 * step_by {
            t += step_by;
            w.advance(t);
        }
        let c = w.claim();
        assert_eq!(c.state, HubState::Reinforced, "scale {n}");
        assert_eq!(c.holder(), Some(w.a), "still the holder's");
        // The campaign: type, defender, start, the 0.6 / 0.4 bar.
        let k = crate::sov_campaign::campaign(w.reg.deltas(), SYSTEM).expect("a campaign");
        assert_eq!(k.defender, w.a);
        assert_eq!(k.attackers, vec![w.d]);
        assert_eq!(k.event_type, EVENT_TYPE_HUB);
        assert!(!k.started);
        assert_eq!(c.state_until, Some(k.start_time));
        assert_eq!(
            k.scores,
            vec![
                (w.a, crate::sov_campaign::DEFENDER_START_SCORE),
                (-w.a, 1.0 - crate::sov_campaign::DEFENDER_START_SCORE)
            ]
        );
        // At least the delay later, compressed by the clock.
        let delay = scale(n).span(crate::sov_campaign::REINFORCE_DELAY);
        assert!(k.start_time >= k.created + delay, "scale {n}");
        assert!(
            k.start_time <= k.created + delay + scale(n).day(),
            "scale {n}"
        );
        // The structure row says so, the way the client reads it.
        let PyValue::List(rows) = sov::structure_rows_at(w.reg.deltas(), SYSTEM, t, scale(n))
        else {
            panic!()
        };
        let PyValue::Instance { state, .. } = &rows[0] else {
            panic!("a KeyVal: {:?}", rows[0])
        };
        let Some(PyValue::Tuple(cs)) = crate::pyutil::dict_get(state, b"campaignState") else {
            panic!("a campaignState tuple")
        };
        assert_eq!(cs[0], PyValue::Int(EVENT_TYPE_HUB));
        assert_eq!(cs[1], PyValue::Int(w.a));
        assert_eq!(cs[2], PyValue::Int(k.start_time));
        assert!(matches!(&cs[3], PyValue::Dict(d) if d.len() == 2));
        assert_eq!(
            crate::pyutil::dict_get(state, b"vulnerabilityState"),
            Some(&PyValue::None),
            "no window while reinforced"
        );
        // The alliance rows carry the start time and the scores.
        let PyValue::Tuple(parts) = sov::alliance_structures_at(w.reg.deltas(), w.a, t, scale(n))
        else {
            panic!()
        };
        assert!(flat(&parts[1]).contains(&k.start_time.to_string()));
        let PyValue::List(scores) = &parts[2] else {
            panic!()
        };
        assert_eq!(scores.len(), 2);
        // The pushes: the structures with the campaign, the hub's slim, the
        // defender's notice with the time the nodes come.
        let sent = w.pushes();
        let structs = named(&sent, "OnSolarSystemSovStructuresUpdated");
        assert!(
            structs
                .iter()
                .any(|s| sent_text(s).contains(&k.start_time.to_string())),
            "{sent:?}"
        );
        let notes = named(&sent, "OnNotificationReceived");
        let reinforced = notes
            .iter()
            .find(|(_, a, _)| a.get(1) == Some(&PyValue::Int(kind::STRUCTURE_REINFORCED)))
            .unwrap_or_else(|| panic!("a reinforced notice: {notes:?}"));
        assert!(sent_text(reinforced).contains(&k.start_time.to_string()));
        // The link is stopped; a new one is refused until the campaign ends.
        assert!(w.reg.modules().running(LINK_DAVE).is_none());
        let e = w.link(DAVE, t).expect_err("reinforced");
        assert!(refusal_text(&e).contains("reinforced"), "{e:?}");
    }
}

#[test]
fn a_campaign_begins_when_its_time_comes_and_resolves_both_ways() {
    let mut w = world();
    w.clock(60);
    w.held_by_a_where(true);
    let k = reinforce_at(&mut w.reg, SYSTEM, vec![w.d], false, w.t0).expect("reinforced");
    let _ = w.pushes();
    // Before its time: reinforced.
    w.advance(k.start_time - SEC);
    assert_eq!(w.claim().state, HubState::Reinforced);
    // At its time: the node fight, the managers told.
    w.advance(k.start_time + SEC);
    assert_eq!(w.claim().state, HubState::NodeFight);
    assert!(
        crate::sov_campaign::campaign(w.reg.deltas(), SYSTEM)
            .expect("kept")
            .started
    );
    let sent = w.pushes();
    let notes = named(&sent, "OnNotificationReceived");
    assert!(
        notes
            .iter()
            .any(|(_, a, _)| a.get(1) == Some(&PyValue::Int(kind::COMMAND_NODE_EVENT_STARTED))),
        "{notes:?}"
    );
    // The defenders win: back to claimed, the campaign gone.
    crate::sov_campaign::resolve(
        &mut w.reg,
        SYSTEM,
        crate::sov_campaign::Outcome::DefendersWin,
    )
    .expect("resolved");
    assert_eq!(w.claim().state, HubState::Claimed);
    assert!(crate::sov_campaign::campaign(w.reg.deltas(), SYSTEM).is_none());
    // A second campaign, and this time the attackers win: the hub is gone and
    // the claim lapses (D2's lapse), the system open for a new hub.
    reinforce_at(&mut w.reg, SYSTEM, vec![w.d], true, w.t0).expect("again");
    w.advance(w.t0 + SEC);
    assert_eq!(w.claim().state, HubState::NodeFight);
    let _ = w.pushes();
    crate::sov_campaign::resolve(
        &mut w.reg,
        SYSTEM,
        crate::sov_campaign::Outcome::AttackersWin,
    )
    .expect("resolved");
    assert!(sov::claim(w.reg.deltas(), SYSTEM).is_none());
    assert!(sov_hub::hub(w.reg.deltas(), SYSTEM).is_none());
    assert_eq!(
        sov::recent_activity(w.reg.deltas(), T0 + SEC)
            .last()
            .map(|a| a.owner),
        Some(None),
        "a loss row"
    );
    let sent = w.pushes();
    let kinds: Vec<_> = named(&sent, "OnNotificationReceived")
        .iter()
        .filter_map(|(_, a, _)| match a.get(1) {
            Some(PyValue::Int(t)) => Some(*t),
            _ => None,
        })
        .collect();
    assert!(kinds.contains(&kind::STRUCTURE_DESTROYED), "{kinds:?}");
    assert!(kinds.contains(&kind::ALLIANCE_CLAIM_LOST), "{kinds:?}");
}

#[test]
fn a_reinforced_hub_that_runs_dry_keeps_its_campaign_state() {
    // D2's fuel step must not turn a reinforced hub into an offline one.
    let mut w = world();
    w.held_by_a_where(true);
    reinforce_at(&mut w.reg, SYSTEM, vec![w.d], false, w.t0).expect("reinforced");
    sov_hub::burn_now(&mut w.reg, 1);
    assert_eq!(w.claim().state, HubState::Reinforced);
    assert!(crate::sov_campaign::campaign(w.reg.deltas(), SYSTEM).is_some());
}

#[test]
fn an_offline_hub_is_open_the_whole_game_day() {
    let mut w = world();
    w.held_by_a_where(false);
    let c = w.claim();
    let clock = crate::game_clock::of(&w.reg);
    assert!(matches!(
        phase(w.reg.deltas(), &c, w.t0, clock),
        Phase::Invulnerable(_)
    ));
    sov::put(
        w.reg.deltas_mut(),
        &Claim {
            state: HubState::Offline,
            ..c.clone()
        },
    );
    let c = w.claim();
    let Phase::Vulnerable(day) = phase(w.reg.deltas(), &c, w.t0, clock) else {
        panic!("open")
    };
    assert_eq!(day.end - day.start, clock.day());
    w.link(DAVE, w.t0).expect("an offline hub is attackable");
}

#[test]
fn the_window_edge_is_pushed_once_and_a_restart_pushes_nothing() {
    let mut w = world();
    w.clock(1);
    w.held_by_a_where(true);
    let c = w.claim();
    let clock = crate::game_clock::of(&w.reg);
    let Phase::Vulnerable(win) = phase(w.reg.deltas(), &c, w.t0, clock) else {
        panic!("inside")
    };
    // First sight: remembered, nothing pushed.
    w.advance(w.t0);
    let _ = w.pushes();
    w.advance(w.t0 + 2 * SEC);
    assert!(named(&w.pushes(), "OnSolarSystemSovStructuresUpdated").is_empty());
    // The window closes: one push. TICK-1: this system is cold (nobody
    // flies here), so the edge is found on its system's beat, within
    // `COLD_EVERY` scans (one scan a second); a hot system is shown to find
    // it on the first scan in `a_hot_system_finds_the_window_edge_at_once`.
    let mut seen = 0;
    let mut at = win.end;
    for k in 1..=crate::tick_budget::COLD_EVERY as i64 {
        at = win.end + k * SEC;
        w.advance(at);
        seen += named(&w.pushes(), "OnSolarSystemSovStructuresUpdated").len();
        if seen > 0 {
            break;
        }
    }
    assert_eq!(seen, 1, "one push, on the cold beat");
    for k in 1..=2 * crate::tick_budget::COLD_EVERY as i64 {
        w.advance(at + k * SEC);
        assert!(named(&w.pushes(), "OnSolarSystemSovStructuresUpdated").is_empty());
    }
    // A fresh slot (a restart) pushes nothing for a phase it has not seen.
    *w.reg.ext_mut::<Entosis>() = Entosis::default();
    w.advance(at + 100 * SEC);
    assert!(named(&w.pushes(), "OnSolarSystemSovStructuresUpdated").is_empty());
}

/// **TICK-1**: a system with a pilot in space is looked at on every scan,
/// so its window edge is pushed on the first scan after it, as before.
#[test]
fn a_hot_system_finds_the_window_edge_at_once() {
    let mut w = world();
    w.clock(1);
    w.held_by_a_where(true);
    let c = w.claim();
    let clock = crate::game_clock::of(&w.reg);
    let Phase::Vulnerable(win) = phase(w.reg.deltas(), &c, w.t0, clock) else {
        panic!("inside")
    };
    let hot = |w: &mut World, t: i64| {
        let reasons = [(c.system_id, crate::liveness::Reasons::PILOT)]
            .into_iter()
            .collect();
        w.reg
            .ext_mut::<crate::liveness::Liveness>()
            .apply(&reasons, t);
    };
    let t0 = w.t0;
    hot(&mut w, t0);
    w.advance(t0);
    let _ = w.pushes();
    hot(&mut w, win.end + SEC);
    w.advance(win.end + SEC);
    let sent = w.pushes();
    assert_eq!(
        named(&sent, "OnSolarSystemSovStructuresUpdated").len(),
        1,
        "{sent:?}"
    );
}

#[test]
fn a_drifting_link_stops_out_of_range() {
    let mut w = world();
    w.clock(1);
    w.link(ALICE, w.t0).expect("starts");
    w.advance(w.t0 + SEC);
    assert!(w.reg.modules().running(LINK_ALICE).is_some());
    // Within the slack: it holds.
    let near = hubfx::off(hubfx::fuel_spot(&w.reg), [0.0, 0.0, RANGE_SLACK / 2.0]);
    hubfx::place_at(&mut w.reg, ALICE, near);
    w.advance(w.t0 + 2 * SEC);
    assert!(w.reg.modules().running(LINK_ALICE).is_some());
    let far = hubfx::off(hubfx::fuel_spot(&w.reg), [0.0, 0.0, 2.0 * LINK_RANGE]);
    hubfx::place_at(&mut w.reg, ALICE, far);
    w.advance(w.t0 + 3 * SEC);
    assert!(
        w.reg.modules().running(LINK_ALICE).is_none(),
        "out of range"
    );
    let sent = w.pushes();
    assert!(
        named(&sent, "OnGodmaShipEffect")
            .iter()
            .any(|s| sent_text(s).contains("out of range")),
        "{sent:?}"
    );
}

#[test]
fn linking_ship_cannot_warp_jump_or_dock() {
    let mut w = world();
    assert!(linking(&w.reg, ALICE).is_none());
    assert!(refuse_dock(&w.reg, Some(ALICE)).is_ok());
    w.link(ALICE, w.t0).expect("starts");
    // Through the same tables the warp and the gate check.
    let warp = crate::movement_guard::check(&crate::module::plan().warp_guards, &w.reg, ALICE);
    assert!(warp.is_some(), "warp refused");
    let jump = crate::movement_guard::check(&crate::module::plan().jump_guards, &w.reg, ALICE);
    assert!(jump.is_some(), "jump refused");
    assert!(refuse_dock(&w.reg, Some(ALICE)).is_err(), "dock refused");
    // Bob's ship is not linking: free.
    assert!(linking(&w.reg, BOB).is_none());
    // Once the module stops the ship is free again.
    crate::modules::stop(&mut w.reg, LINK_ALICE, w.t0 + SEC, None);
    assert!(linking(&w.reg, ALICE).is_none());
    assert!(refuse_dock(&w.reg, Some(ALICE)).is_ok());
}

#[test]
fn only_one_link_runs_on_a_ship() {
    let mut w = world();
    w.link(ALICE, w.t0).expect("the first");
    let e = w
        .link_with(ALICE, LINK_ALICE_2, w.t0)
        .expect_err("a second link");
    assert!(refusal_text(&e).contains("Only one"), "{e:?}");
}

#[test]
fn a_hub_can_be_locked_and_swept_like_a_ball_in_the_system() {
    let mut w = world();
    assert!(is_lockable(&w.reg, w.hub, SYSTEM));
    assert!(!is_lockable(&w.reg, w.hub, hubfx::HIGH));
    assert!(!is_lockable(&w.reg, 123, SYSTEM));
    let (system, at) = lock_position(&w.reg, w.hub).expect("a position");
    assert_eq!(system, SYSTEM);
    assert_eq!(at, sov_hub::position_in(&w.reg, SYSTEM));
    crate::targeting::lock(&mut w.reg, ALICE, w.hub).expect("the lock is accepted");
    assert!(
        w.reg
            .locks()
            .targets_of(hubfx::ship(ALICE))
            .contains(&w.hub)
    );
}

#[test]
fn scores_and_campaigns_survive_the_world_file() {
    let mut w = world();
    w.clock(60);
    w.held_by_a_where(true);
    w.link(DAVE, w.t0).expect("attacker");
    w.advance(w.t0 + w.period() + SEC);
    let stored = w.score().expect("scoring");
    reinforce_at(&mut w.reg, SYSTEM, vec![w.d], false, w.t0).expect("reinforced");
    let camp = crate::sov_campaign::campaign(w.reg.deltas(), SYSTEM).expect("a campaign");
    // The rows are plain strings: through put_verbatim, as a save and a load.
    let mut copy = crate::delta::Deltas::default();
    for (kind, rows) in w.reg.deltas().kinds() {
        for (k, v) in rows {
            copy.put_verbatim(kind, k.clone(), v.clone());
        }
    }
    assert_eq!(crate::sov_campaign::campaign(&copy, SYSTEM), Some(camp));
    // The reinforcement reset the score to idle; a scoring tuple round trips
    // the same way.
    put_score(&mut copy, w.hub, SYSTEM, &stored);
    assert_eq!(score_row(&copy, w.hub), Some((SYSTEM, stored)));
}

#[test]
fn a_holder_change_by_the_gm_clears_the_campaign_and_the_score() {
    let mut w = world();
    w.held_by_a_where(true);
    reinforce_at(&mut w.reg, SYSTEM, vec![w.d], false, w.t0).expect("reinforced");
    // `/sov set` to the rival: the campaign belongs to the old holder.
    let exec = hubfx::owner(&w.reg, w.d);
    sov::set_claim(w.reg.deltas_mut(), SYSTEM, Some(exec), w.t0).expect("changed");
    assert_eq!(w.claim().holder(), Some(w.d));
    assert_eq!(w.claim().state, HubState::Claimed);
    assert!(crate::sov_campaign::campaign(w.reg.deltas(), SYSTEM).is_none());
    assert_eq!(slim_score(w.reg.deltas(), w.hub), PyValue::None);
}

#[test]
fn the_gm_slash_words_report_and_reinforce_and_resolve() {
    let mut w = world();
    w.held_by_a_where(true);
    let s = |w: &mut World, line: &str| {
        let words: Vec<&str> = line.split_whitespace().collect();
        slash(&mut w.reg, Some(ALICE), &words)
    };
    let out = s(&mut w, "entosis here");
    assert!(
        out.contains("vulnerable") && out.contains("score 0.000"),
        "{out}"
    );
    let out = s(&mut w, "reinforce here now");
    assert!(out.starts_with("reinforced"), "{out}");
    assert_eq!(w.claim().state, HubState::Reinforced);
    let out = s(&mut w, "entosis");
    assert!(out.contains("reinforced"), "{out}");
    // A campaign that starts now begins on the next tick.
    let start = crate::sov_campaign::campaign(w.reg.deltas(), SYSTEM)
        .expect("a campaign")
        .start_time;
    w.advance(start + SEC);
    assert_eq!(w.claim().state, HubState::NodeFight);
    let out = s(&mut w, "resolve here defenders");
    assert!(out.contains("resolved"), "{out}");
    assert_eq!(w.claim().state, HubState::Claimed);
    assert!(s(&mut w, "resolve here nobody").starts_with("usage"));
    assert!(s(&mut w, "resolve here defenders").contains("no campaign"));
}

#[test]
fn a_lapsed_hub_stops_its_links_and_forgets_its_score() {
    let mut w = world();
    w.clock(1);
    w.link(ALICE, w.t0).expect("starts");
    w.advance(w.t0 + w.period() + SEC);
    assert!(w.score().is_some());
    let h = sov_hub::hub(w.reg.deltas(), SYSTEM).expect("a hub");
    sov_hub::lapse(&mut w.reg, &h, w.t0 + 10 * SEC);
    assert!(w.reg.modules().running(LINK_ALICE).is_none());
    assert!(score_row(w.reg.deltas(), w.hub).is_none());
    assert!(w.reg.deltas().of(KIND).is_empty());
}

#[test]
fn the_slot_and_the_stage_are_registered() {
    // `Registry::new` builds every slot: a missing registration panics here.
    let mut reg = Registry::new(crate::model::SandboxConfig::default());
    assert!(reg.ext::<Entosis>().paid.is_empty());
    assert!(
        crate::module::plan()
            .stages
            .iter()
            .any(|s| s.name == "entosis")
    );
    assert!(crate::delta_kinds::is_registered(KIND));
    assert!(crate::delta_kinds::is_registered(crate::sov_campaign::KIND));
    // A world with no hubs: the stage does nothing and costs nothing.
    tick(&mut reg, Instant::now(), T0);
    assert!(reg.take_pending().is_empty());
}

#[test]
fn two_links_of_one_alliance_are_one_scorer_not_a_block() {
    let mut w = world();
    w.clock(60);
    w.link(ALICE, w.t0).expect("Alice");
    w.link(BOB, w.t0).expect("Bob");
    w.advance(w.t0 + w.period() + SEC / 10);
    let s = w.score().expect("a row");
    assert!(!s.blocked && s.rate > 0.0, "{s:?}");
}

#[test]
fn the_gm_prime_line_sets_the_hour_at_once_by_id_or_ticker() {
    let mut w = world();
    let words = |line: &str| {
        line.split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let run = |w: &mut World, line: &str| {
        let owned = words(line);
        let args: Vec<&str> = owned.iter().map(String::as_str).collect();
        slash(&mut w.reg, Some(ALICE), &args)
    };
    let line = format!("prime {} 7", w.a);
    let out = run(&mut w, &line);
    assert!(out.contains("prime hour 7"), "{out}");
    assert_eq!(
        sov::prime_hour(w.reg.deltas(), w.a, FAR_FUTURE).current,
        Some(7)
    );
    let out = run(&mut w, "prime irv 9");
    assert!(out.contains("prime hour 9"), "{out}");
    assert_eq!(
        sov::prime_hour(w.reg.deltas(), w.d, FAR_FUTURE).current,
        Some(9)
    );
    let line = format!("prime {} 24", w.a);
    assert!(run(&mut w, &line).contains("0 to 23"));
    assert!(run(&mut w, "prime nobody 3").contains("no alliance"));
}

/// **TICK-1**: the stage settles exactly the hubs `settle_hub` could do
/// something for -- a running link, or a stored score -- and the same ones,
/// in the same order, as walking every hub and asking. Sixty idle hubs are
/// never read; a score row for something that is not a hub is ignored.
#[test]
fn only_a_hub_with_a_link_or_a_score_is_settled() {
    const EXTRA_SYSTEM: i64 = 31_620_000;
    const EXTRA_HUB: i64 = 700_620_000;
    let mut w = world();
    w.clock(1);
    for i in 0..60 {
        sov_hub::put(
            w.reg.deltas_mut(),
            &sov_hub::Hub {
                system_id: EXTRA_SYSTEM + i,
                hub_id: EXTRA_HUB + i,
                position: [0.0; 3],
                launcher_id: None,
                launch_time: T0,
                offline_days: 0,
            },
        );
    }
    let legacy = |w: &World| -> Vec<sov_hub::Hub> {
        sov_hub::all(w.reg.deltas())
            .into_iter()
            .filter(|h| {
                !links_on(&w.reg, h.hub_id).is_empty()
                    || score_row(w.reg.deltas(), h.hub_id).is_some()
            })
            .collect()
    };
    assert!(
        hubs_with_work(&w.reg).is_empty(),
        "nothing running, no score"
    );
    assert_eq!(hubs_with_work(&w.reg), legacy(&w));
    // A score on three extra hubs (two of them out of id order against the
    // system text) and on an id that is no hub.
    let score = Score {
        controlling: Some(A),
        scoring: Some(A),
        s0: 0.2,
        t0: T0,
        rate: 1.0,
        blocked: false,
    };
    for i in [7, 12, 3] {
        put_score(w.reg.deltas_mut(), EXTRA_HUB + i, EXTRA_SYSTEM + i, &score);
    }
    put_score(w.reg.deltas_mut(), 123, EXTRA_SYSTEM + 9, &score);
    // A link on the fixture's own hub.
    w.link(ALICE, w.t0).expect("a link starts");
    let work = hubs_with_work(&w.reg);
    assert_eq!(work, legacy(&w));
    assert_eq!(work.len(), 4);
    assert!(work.iter().any(|h| h.hub_id == w.hub));
    // The order is the stored order: system id as text.
    let keys: Vec<String> = work.iter().map(|h| h.system_id.to_string()).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    // The tick settles them and leaves an idle hub's absence of a row alone.
    w.advance(w.t0 + SEC);
    assert!(score_row(w.reg.deltas(), EXTRA_HUB + 20).is_none());
}

/// **TICK-1, catch-up**: a score is a tuple (`s0`, `t0`, `rate`) read at
/// `now`, so a hub looked at once after a long gap finds the bar where
/// looking every second would have left it -- here a bar falling at the
/// regen rate reaches idle in either case.
#[test]
fn a_hub_looked_at_late_finds_the_score_where_frequent_looks_leave_it() {
    let falling = |w: &mut World| {
        let s = Score {
            controlling: Some(-w.d),
            scoring: None,
            s0: 0.5,
            t0: w.t0,
            rate: -HUB_REGEN_RATE,
            blocked: false,
        };
        put_score(w.reg.deltas_mut(), w.hub, SYSTEM, &s);
    };
    let gap = 3_000;
    let mut often = world();
    often.clock(1);
    falling(&mut often);
    for k in 1..=gap {
        often.advance(often.t0 + k * SEC);
    }
    let mut once = world();
    once.clock(1);
    falling(&mut once);
    once.advance(once.t0 + gap * SEC);
    let at = often.t0 + gap * SEC;
    let read = |w: &World| {
        score_row(w.reg.deltas(), w.hub).map_or(0.0, |(_, s)| s.at(at, Spec::HUB.capture_seconds))
    };
    assert_eq!(read(&often), read(&once));
    assert!(read(&once) < 1e-9, "the bar ended idle");
}
