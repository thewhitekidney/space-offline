//! MAIL-1 over real sockets: a player mails another who is pushed it and
//! reads it; a mail to a corp reaches each member; a bot CEO that accepts a
//! player's application mails the player a welcome that the player's
//! `SyncMail` lists and `GetBody` opens, and a mail a player sends to a bot is
//! taken without a murmur.
//!
//! The calls are the client's own (`mailSvc`, `docs/lanes/mail-1.md`):
//! `mailMgr.SyncMail(firstID, lastID)`, `GetBody(id, markAsRead)`,
//! `SendMail(toCharacterIDs, toListID, toCorpOrAllianceID, title, body,
//! isReplyTo, isForwardedFrom)`. Names, ids, careers and texts are invented;
//! no `[A]` value is asserted (limits are the server's named constants). The
//! same limitation as every test here: both ends marshal with `eve-marshal`.

use super::common::{config, dump, gm, login, pilot, serve, until};
use eve_bot::Conn;
use eve_bot::brain::ActKind;
use eve_bot::career::{Band, Career, INCOME_REF, Signals, Support};
use eve_bot::corpmind::Hall;
use eve_bot::driver::{self, BotConfig, Handle, Pace, Society};
use eve_bot::mind::{Mind, TRAIT_COUNT, Trait};
use eve_bot::society;
use eve_marshal::PyValue;
use eve_server::mail::{label, status};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_000_701;
const BOB: i64 = 90_000_702;
const CAROL: i64 = 90_000_703;
const ANN: i64 = 90_000_704;
const MAL: i64 = 90_000_705;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

fn ints(v: &[i64]) -> PyValue {
    PyValue::List(v.iter().map(|n| PyValue::Int(*n)).collect())
}

/// One attribute of a `utillib.KeyVal` (its state dict is value first).
fn attr<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    let PyValue::Instance { state, .. } = v else {
        return None;
    };
    state
        .dict_as_kv()?
        .into_iter()
        .find_map(|(k, val)| matches!(k, PyValue::Bytes(b) if b == name.as_bytes()).then_some(val))
}

fn int(v: Option<&PyValue>) -> i64 {
    match v {
        Some(PyValue::Int(n)) => *n,
        other => panic!("an int, got {other:?}"),
    }
}

fn rows(v: Option<&PyValue>) -> &[PyValue] {
    match v {
        Some(PyValue::List(rows)) => rows,
        other => panic!("a list, got {other:?}"),
    }
}

/// `SyncMail(None, 0)`, an empty cache's call: the headers and the
/// `(messageID, statusMask, labelMask)` of every mail held.
fn sync(c: &mut Conn) -> (Vec<PyValue>, Vec<(i64, i64, i64)>) {
    let answer = c
        .call("mailMgr", "SyncMail", vec![PyValue::None, PyValue::Int(0)])
        .expect("answered")
        .value()
        .expect("a mailbox");
    let new = rows(attr(&answer, "newMail")).to_vec();
    let held = rows(attr(&answer, "mailStatus"))
        .iter()
        .map(|r| {
            (
                int(attr(r, "messageID")),
                int(attr(r, "statusMask")),
                int(attr(r, "labelMask")),
            )
        })
        .collect();
    (new, held)
}

fn send(
    c: &mut Conn,
    to: &[i64],
    corp: Option<i64>,
    title: &str,
    body: &str,
) -> eve_bot::CallOutcome {
    c.call(
        "mailMgr",
        "SendMail",
        vec![
            ints(to),
            PyValue::None,
            corp.map_or(PyValue::None, PyValue::Int),
            uni(title),
            uni(body),
            PyValue::Int(0),
            PyValue::Int(0),
        ],
    )
    .expect("answered")
}

/// The body of a mail: `GetBody` answers a zlib stream, which the client
/// inflates; the server's is stored blocks, so the text is in it verbatim.
fn body_of(c: &mut Conn, id: i64, mark: bool) -> String {
    let z = c
        .call(
            "mailMgr",
            "GetBody",
            vec![PyValue::Int(id), PyValue::Bool(mark)],
        )
        .expect("answered")
        .value();
    let Some(PyValue::Bytes(z)) = z else {
        panic!("a compressed body, got {z:?}")
    };
    assert_eq!(&z[..2], &[0x78, 0x01], "a zlib header");
    String::from_utf8_lossy(&z).into_owned()
}

/// Wait for `OnMailSent` to reach `c` and return its arguments.
fn pushed(c: &mut Conn) -> Option<Vec<PyValue>> {
    let mut found = None;
    until(Duration::from_secs(20), || {
        for p in c
            .collect_pushes(Duration::from_millis(100))
            .unwrap_or_default()
        {
            if let Some((event, args)) = eve_bot::notification(&p)
                && event == "OnMailSent"
            {
                found = Some(args);
            }
        }
        found.is_some()
    });
    found
}

fn corp_call(c: &mut Conn, corp: i64, method: &str, args: Vec<PyValue>) -> eve_bot::Answer {
    c.moniker_call("corpRegistry", PyValue::Int(corp), method, args, vec![])
        .unwrap_or_else(|e| panic!("corpRegistry.{method}: {e}"))
}

/// A player mails another, who is pushed `OnMailSent`, lists it in
/// `SyncMail`, opens it and marks it read; the sender's own copy is filed
/// Sent.
#[test]
fn a_player_mails_another_player_who_reads_it() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_701),
        pilot(BOB, "Bob", 100_000_702),
    ];
    let addr = serve(config);
    let mut alice = login(addr, "Alice");
    let mut bob = login(addr, "Bob");
    assert!(sync(&mut bob).1.is_empty(), "an empty mailbox to begin");

    let id = match send(&mut alice, &[BOB], None, "Hello Bob", "Fly safe out there.").value() {
        Some(PyValue::Int(id)) => id,
        other => panic!("an id, got {other:?}"),
    };
    let args = pushed(&mut bob).expect("Bob is pushed OnMailSent");
    assert_eq!(args.len(), 8, "{args:?}");
    assert_eq!(args[0], PyValue::Int(id));
    assert_eq!(args[1], PyValue::Int(ALICE));
    assert_eq!(args[6], uni("Hello Bob"));

    let (headers, held) = sync(&mut bob);
    assert_eq!(held, vec![(id, 0, label::INBOX)]);
    assert_eq!(headers.len(), 1);
    assert_eq!(int(attr(&headers[0], "senderID")), ALICE);
    assert_eq!(attr(&headers[0], "title"), Some(&uni("Hello Bob")));
    assert!(body_of(&mut bob, id, true).contains("Fly safe out there."));
    assert_eq!(sync(&mut bob).1, vec![(id, status::READ, label::INBOX)]);
    assert_eq!(
        sync(&mut alice).1,
        vec![(id, status::READ, label::SENT)],
        "Alice's own copy"
    );

    // A refusal is a UserError, not a dropped call.
    let none = send(&mut alice, &[], None, "Nobody", "x");
    assert!(none.refusal().is_some(), "no recipient");
}

/// A mail to a corp reaches each member and not an outsider.
#[test]
fn a_mail_to_a_corp_reaches_each_member() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_701),
        pilot(BOB, "Bob", 100_000_702),
        pilot(CAROL, "Carol", 100_000_703),
    ];
    let addr = serve(config);
    let mut alice = login(addr, "Alice");
    let mut bob = login(addr, "Bob");
    let mut carol = login(addr, "Carol");
    gm(&mut alice, "/giveisk 2000000000");
    let npc_corp = alice.session().get_i64("corpid").expect("a corp");
    let founded = corp_call(
        &mut alice,
        npc_corp,
        "AddCorporation",
        vec![uni("Mailers Ltd"), uni("MAIL"), uni(""), uni("")],
    );
    assert_eq!(founded.refusal(), None, "AddCorporation refused");
    let corp = founded
        .outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("corpid"))
        .expect("the founding moves the session to the new corp");
    // Bob is invited and joins.
    assert_eq!(
        corp_call(
            &mut alice,
            corp,
            "InsertInvitation",
            vec![PyValue::Int(BOB)]
        )
        .refusal(),
        None
    );
    let bobs_corp = bob.session().get_i64("corpid").unwrap_or(0);
    let mine = corp_call(&mut bob, bobs_corp, "GetMyApplications", vec![]);
    let app = {
        let Some(PyValue::Dict(entries)) = mine.value.as_ref() else {
            panic!("applications: {:?}", mine.value)
        };
        let (PyValue::List(apps), _) = &entries[0] else {
            panic!("a list")
        };
        int(attr(&apps[0], "applicationID"))
    };
    let accepted = corp_call(
        &mut bob,
        corp,
        "UpdateApplicationOffer",
        vec![
            PyValue::Int(app),
            PyValue::Int(BOB),
            PyValue::Int(corp),
            uni(""),
            PyValue::Int(2),
        ],
    );
    assert_eq!(accepted.refusal(), None, "accepting refused");

    let id = match send(&mut alice, &[], Some(corp), "Fleet tonight", "Bring ammo.").value() {
        Some(PyValue::Int(id)) => id,
        other => panic!("an id, got {other:?}"),
    };
    assert_eq!(
        pushed(&mut bob).map(|a| a[0].clone()),
        Some(PyValue::Int(id)),
        "Bob is pushed"
    );
    let (headers, held) = sync(&mut bob);
    assert_eq!(held, vec![(id, 0, label::CORPORATION)]);
    assert_eq!(
        attr(&headers[0], "toCorpOrAllianceID"),
        Some(&PyValue::Int(corp))
    );
    assert!(body_of(&mut bob, id, true).contains("Bring ammo."));
    assert!(sync(&mut carol).1.is_empty(), "an outsider hears nothing");
    let outsider = send(&mut carol, &[], Some(corp), "Let me in", "x");
    assert!(outsider.refusal().is_some(), "an outsider cannot mail it");
}

// ---- a bot recruits --------------------------------------------------------------------

/// A career played by the miner role, which waits docked without a ship.
static TABLE: &[Career] = &[Career {
    name: "miner",
    role: Some(&eve_bot::roles::MINE),
    support: Support::Served,
    base: 1.0,
    affinity: [0.0; TRAIT_COUNT],
    band: Band::High,
    prior_isk_h: INCOME_REF,
}];

fn founder(
    name: &str,
    hall: &Arc<Hall>,
    addr: SocketAddr,
) -> (Handle, std::thread::JoinHandle<()>) {
    let now = driver::unix_now();
    let mut mind = Mind::seeded(name, 1, TABLE, &Signals::default(), now);
    mind.career = "miner".to_owned();
    mind.since = now;
    for t in &mut mind.traits.0 {
        *t = 0.5;
    }
    mind.traits.0[Trait::Loyalty as usize] = 1.0;
    mind.traits.0[Trait::Sociability as usize] = (society::FOUND_SOCIAL + 1.0) / 2.0;
    mind.traits.0[Trait::Diligence as usize] = 1.0;
    mind.traits.0[Trait::Aggression as usize] = 1.0;
    let mut soc = Society::new(mind, TABLE, Signals::default());
    soc.decide_every = Duration::from_secs(3_600);
    soc.hall = Some(Arc::clone(hall));
    let mut config = BotConfig::new(name, addr, Pace::Fast).with_society(soc);
    config.chat = false;
    config.policy.gm_setup = false;
    driver::spawn(config)
}

fn corp_lines(h: &Handle) -> Vec<String> {
    h.log()
        .into_iter()
        .filter(|e| e.kind == ActKind::Corp)
        .map(|e| e.what)
        .collect()
}

/// A bot founds a corp; a player applies; the bot's CEO accepts and mails the
/// player, whose `SyncMail` lists the welcome and whose `GetBody` opens it.
/// The player also mails the bot, which takes it without error and keeps
/// running (its connection still answers).
#[test]
fn a_bot_ceo_mails_the_pilot_it_accepts() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ANN, "Ann", 100_000_704),
        pilot(MAL, "Mal", 100_000_705),
    ];
    let addr = serve(config);
    {
        let mut c = login(addr, "Ann");
        gm(&mut c, "/giveisk 3000000000");
        gm(&mut c, "/giveskill Corporation Management 1");
    }
    let hall = Arc::new(Hall::new(2).with_every(0));
    let (ann, thread) = founder("Ann", &hall, addr);
    let founded = until(Duration::from_secs(60), || {
        hall.snapshot().bot_corps().len() == 1
    });
    assert!(founded, "Ann founds a corp:\n{}", dump(&ann));
    let corp = hall.snapshot().members[&ANN].corp;

    let mut mal = login(addr, "Mal");
    let npc_corp = mal.session().get_i64("corpid").expect("a corp");
    let applied = corp_call(
        &mut mal,
        corp,
        "InsertApplication",
        vec![PyValue::Int(corp), uni("I would like to fly with you.")],
    );
    let _ = npc_corp;
    assert_eq!(applied.refusal(), None, "InsertApplication refused");

    // Mal writes to the bot too: it must not trip over it.
    let to_bot = send(&mut mal, &[ANN], None, "Hi Ann", "Nice corp.");
    assert_eq!(to_bot.refusal(), None, "a mail to a bot is taken");

    let in_inbox = |c: &mut Conn| sync(c).1.iter().any(|h| h.2 & label::INBOX != 0);
    let mailed = until(Duration::from_secs(90), || in_inbox(&mut mal));
    // The bot records its mail line after the send returns, so the inbox can
    // show the mail a moment before the line exists: wait for it too.
    let mail_line = format!("mail {MAL}");
    let _ = until(Duration::from_secs(10), || {
        corp_lines(&ann).iter().any(|l| l.contains(&mail_line))
    });
    let lines = corp_lines(&ann);
    ann.stop();
    let _ = thread.join();
    assert!(mailed, "Mal is mailed:\n{lines:#?}");
    let (headers, held) = sync(&mut mal);
    let &(id, state, labels) = held
        .iter()
        .find(|h| h.2 & label::INBOX != 0)
        .expect("the welcome");
    assert_eq!((state, labels), (0, label::INBOX), "{held:?}");
    let header = headers
        .iter()
        .find(|h| int(attr(h, "messageID")) == id)
        .expect("its header");
    assert_eq!(int(attr(header, "senderID")), ANN);
    let text = body_of(&mut mal, id, true);
    assert!(text.contains("accepted"), "{text}");
    assert!(
        lines
            .iter()
            .any(|l| l.contains(&format!("mail {MAL}")) && l.ends_with("ok")),
        "{lines:#?}"
    );
    // The bot's own mailbox holds Mal's mail, as any pilot's would.
    let mut ann_again = login(addr, "Ann");
    assert!(
        sync(&mut ann_again)
            .1
            .iter()
            .any(|h| h.2 & label::INBOX != 0),
        "Ann holds Mal's mail"
    );
}

/// The diplomacy mail (`Plan::Contacts`'s hook): a CEO that sets a contact
/// mails the other corp's CEO, a red on a pilot goes to the pilot, and an
/// alliance or a corp with no known CEO gets nothing. Called the way the
/// plan does, on the CEO's own connection.
#[test]
fn a_contact_set_mails_the_other_side() {
    use eve_bot::corpmind::CorpMind;
    use eve_bot::society::{BLUE_STANDING, Profile, RED_STANDING};
    const OURS: i64 = 98_000_011;
    const THEIRS: i64 = 98_000_012;
    const ORPHAN: i64 = 98_000_013;
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_701),
        pilot(BOB, "Bob", 100_000_702),
        pilot(CAROL, "Carol", 100_000_703),
    ];
    let addr = serve(config);
    let hall = Hall::new(2);
    let mind = |id: i64, ceo: Option<i64>, name: &str| CorpMind {
        id,
        ceo,
        profile: Profile::of("miner", Band::High),
        founded: 0,
        alliance: None,
        name: name.to_owned(),
        applied_alliance: None,
        salt: 0,
    };
    {
        let mut st = hall.lock();
        st.corps
            .insert(OURS, mind(OURS, Some(ALICE), "Invented Ours"));
        st.corps
            .insert(THEIRS, mind(THEIRS, Some(BOB), "Invented Theirs"));
        st.corps
            .insert(ORPHAN, mind(ORPHAN, None, "Invented Orphan"));
    }
    let mut alice = login(addr, "Alice");
    let mut bob = login(addr, "Bob");
    let mut carol = login(addr, "Carol");

    let line = eve_bot::mail::diplomacy(&mut alice, &hall, OURS, THEIRS, BLUE_STANDING)
        .expect("sent")
        .expect("a line");
    assert!(line.ends_with("ok"), "{line}");
    let (headers, held) = sync(&mut bob);
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0].2, label::INBOX);
    assert_eq!(int(attr(&headers[0], "senderID")), ALICE);
    let title = format!("{:?}", attr(&headers[0], "title"));
    assert!(
        title.contains("Invented Ours") && title.contains("blue"),
        "{title}"
    );
    assert!(body_of(&mut bob, held[0].0, false).contains("Invented Theirs"));

    let line = eve_bot::mail::diplomacy(&mut alice, &hall, OURS, CAROL, RED_STANDING)
        .expect("sent")
        .expect("a line");
    assert!(line.ends_with("ok"), "{line}");
    assert_eq!(
        sync(&mut carol).1.len(),
        1,
        "a red on a pilot reaches the pilot"
    );

    for target in [ORPHAN, 99_000_001, ALICE] {
        assert_eq!(
            eve_bot::mail::diplomacy(&mut alice, &hall, OURS, target, BLUE_STANDING)
                .expect("no call"),
            None,
            "{target}"
        );
    }
    assert_eq!(
        eve_bot::mail::diplomacy(&mut alice, &hall, OURS, THEIRS, 0.0).expect("no call"),
        None,
        "neutral says nothing"
    );
    assert_eq!(sync(&mut bob).1.len(), 1, "nothing more reached Bob");
}
