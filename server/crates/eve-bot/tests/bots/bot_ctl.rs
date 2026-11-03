//! BOT-CTL over real sockets: a GM's `/bots` reaches the control link's peer
//! and the peer's reply comes back to the console; a non-dev server refuses;
//! and the real `eve_bot::control` peer lists, brings, re-sides and despawns.
//!
//! The same limitation as every test here: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes. Names and ids are
//! invented.

use super::common::{SYSTEM, config, login, pilot, serve, until};
use eve_bot::Conn;
use eve_bot::brain::ActKind;
use eve_bot::control::{Controller, Member, Spawn, run_link};
use eve_bot::driver::{BotConfig, Pace, spawn};
use eve_marshal::PyValue;
use eve_server::bot_control::{ADDR_FILE, TOKEN_FILE};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const GM: i64 = 90_000_951;
const BEA: i64 = 90_000_952;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bot-ctl-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn start(dev: bool, dir: &Path) -> SocketAddr {
    let mut config = config();
    config.dev = dev;
    config.bot_control = Some(dir.to_path_buf());
    config.sandbox.characters = vec![
        pilot(GM, "Gina Gm", 100_000_951),
        pilot(BEA, "Bea Bot", 100_000_952),
    ];
    serve(config)
}

fn link(dir: &Path) -> (SocketAddr, String) {
    let addr = std::fs::read_to_string(dir.join(ADDR_FILE)).expect("the addr file");
    let token = std::fs::read_to_string(dir.join(TOKEN_FILE)).expect("the token file");
    (
        addr.trim().parse().expect("an address"),
        token.trim().to_owned(),
    )
}

/// A fake `eve-bots`: says HELLO, records each command and answers
/// `fake heard: <command>`.
fn fake_peer(dir: &Path) -> Arc<Mutex<Vec<String>>> {
    let (addr, token) = link(dir);
    let mut s = TcpStream::connect(addr).expect("dials the link");
    s.write_all(format!("HELLO {token}\n").as_bytes()).unwrap();
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "OK");
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut line = String::new();
        while r.read_line(&mut line).unwrap_or(0) > 0 {
            let rest = line.trim_end().strip_prefix("CMD ").unwrap().to_owned();
            let (id, cmd) = rest.split_once(' ').unwrap();
            log.lock().unwrap().push(cmd.to_owned());
            let _ = s.write_all(format!("REPLY {id} fake heard: {cmd}\n").as_bytes());
            line.clear();
        }
    });
    seen
}

fn slash(c: &mut eve_bot::Conn, line: &str) -> String {
    let out = c
        .call(
            "slash",
            "SlashCmd",
            vec![PyValue::Bytes(line.as_bytes().to_vec())],
        )
        .expect("the slash is answered");
    if let Some(PyValue::Bytes(b)) = out.value() {
        return String::from_utf8(b).expect("text");
    }
    // SESSION-FIX2: a `/bots` reply is raised as `CustomInfo` so the client
    // shows it in a dialog; its text is the `info` keyword's value.
    assert_eq!(out.refusal().as_deref(), Some("CustomInfo"), "{line}");
    let packet = eve_proto::MachoPacket::from_value(&out.result).expect("a packet");
    packet
        .body
        .iter()
        .find_map(|part| match part {
            PyValue::Tuple(w) => match w.first() {
                Some(PyValue::Bytes(pickle)) => info_text(&eve_server::loads(pickle).ok()?),
                _ => None,
            },
            _ => None,
        })
        .unwrap_or_else(|| panic!("no info text for {line}"))
}

/// The value paired with the key `info`, anywhere in a raised error.
fn info_text(v: &PyValue) -> Option<String> {
    let text = |v: &PyValue| match v {
        PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        PyValue::Unicode(u) => Some(u.clone()),
        _ => None,
    };
    match v {
        PyValue::Tuple(t) | PyValue::List(t) => {
            if let [value, key] = t.as_slice()
                && text(key).as_deref() == Some("info")
            {
                return text(value);
            }
            t.iter().find_map(info_text)
        }
        PyValue::ObjectEx { header, .. } => info_text(header),
        // Dict pairs are `(value, key)` in wire order.
        PyValue::Dict(d) => d.iter().find_map(|(v, k)| {
            if text(k).as_deref() == Some("info") {
                text(v)
            } else {
                info_text(v)
            }
        }),
        _ => None,
    }
}

#[test]
fn a_gms_spawn_reaches_the_peer_and_its_reply_comes_back() {
    let d = dir("fake");
    let addr = start(true, &d);
    let seen = fake_peer(&d);
    let mut gm = login(addr, "Gina Gm");
    let reply = slash(&mut gm, "/bots spawn alpha red mine 3 all5 merlin 30000001");
    // BOT-ACCOUNTS: the server adds the three bots' logins to a spawn.
    assert!(
        reply.starts_with("fake heard: spawn alpha red mine 3 all5 merlin 30000001 tokens=alpha1:"),
        "{reply}"
    );
    // The server resolves the caller for `bring`: her system, station, id.
    let reply = slash(&mut gm, "/bots bring alpha");
    assert!(
        reply.starts_with(&format!("fake heard: bring alpha {SYSTEM} "))
            && reply.ends_with(&GM.to_string()),
        "{reply}"
    );
    // A malformed count never leaves the server.
    let reply = slash(&mut gm, "/bots spawn alpha red mine 0 all5 merlin");
    assert!(reply.contains("count is 1..="), "{reply}");
    assert_eq!(seen.lock().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&d);
}

/// BOT-ADMIN: the owner's words, typed in the client, reach the peer as the
/// explicit line (the caller's own system, a fresh group name), unknown words
/// and `help` answer with a usage dialog, and the old form still works.
#[test]
fn natural_words_reach_the_peer_as_an_explicit_spawn() {
    let d = dir("words");
    let addr = start(true, &d);
    let seen = fake_peer(&d);
    let mut gm = login(addr, "Gina Gm");
    let reply = slash(&mut gm, "/bots 4 Allied MINERS");
    assert!(
        reply.starts_with("4 allied miners (side ")
            && reply.contains(", group Miners1x:\nfake heard: spawn Miners1x ")
            && reply.contains(&format!(" mine 4 role Hulk {SYSTEM} tokens=Miners1x1:")),
        "{reply}"
    );
    // The bare kind word the owner typed first.
    let reply = slash(&mut gm, "/bots escorts");
    assert!(
        reply.contains(&format!(" escort 5 role - {SYSTEM} tokens=Escorts1x1:")),
        "{reply}"
    );
    let reply = slash(&mut gm, "/bots 6 hostile gang here");
    assert!(
        reply.contains(&format!(
            "spawn RedGang1x FOES roamgang 6 role Stabber {SYSTEM} quarry={GM}:{SYSTEM} tokens="
        )),
        "{reply}"
    );
    // GANG-SUMMON: `come` names the caller's character and system.
    let reply = slash(&mut gm, "/bots come RedGang1x");
    assert_eq!(reply, format!("fake heard: summon RedGang1x {SYSTEM} {GM}"));
    // A system the server does not know by name says so (no bundle here).
    let reply = slash(&mut gm, "/bots 2 haulers in Nowhere Special");
    assert!(
        reply.contains("no system names") || reply.contains("no system named"),
        "{reply}"
    );
    // Help and nonsense are a dialog with examples; nothing is relayed.
    let reply = slash(&mut gm, "/bots help");
    assert!(
        reply.contains("/bots 10 allied miners") && reply.contains("inspect"),
        "{reply}"
    );
    let reply = slash(&mut gm, "/bots frobnicate");
    assert!(
        reply.contains("frobnicate") && reply.contains("Kinds:"),
        "{reply}"
    );
    // The old explicit form, as the owner used to have to type it.
    let reply = slash(&mut gm, "/bots spawn gang1 RED mine 3 role Hulk 30000042");
    assert!(
        reply.starts_with("fake heard: spawn gang1 RED mine 3 role Hulk 30000042 tokens=gang11:"),
        "{reply}"
    );
    for (line, heard) in [
        ("/bots pause Miners1x", "fake heard: pause Miners1x"),
        ("/bots resume Miners1x1", "fake heard: resume Miners1x1"),
        ("/bots inspect Miners1x1", "fake heard: inspect Miners1x1"),
        ("/bots stop-all", "fake heard: stop-all"),
        ("/bots despawn all", "fake heard: despawn all"),
    ] {
        assert_eq!(slash(&mut gm, line), heard);
    }
    assert_eq!(seen.lock().unwrap().len(), 4 + 5 + 1);
    let _ = std::fs::remove_dir_all(&d);
}

/// BOT-ADMIN: a pause over the control link parks a docked bot, `inspect`
/// shows its state and rows, a resume lets it go, and `stop-all` ends it.
#[test]
fn pause_parks_a_docked_bot_and_resume_and_stop_all_release_it() {
    let d = dir("pause");
    let addr = start(true, &d);
    let hook: eve_bot::control::SpawnHook = Box::new(move |_spec: &Spawn| {
        let mut cfg =
            BotConfig::new("Bea Bot", addr, Pace::Fast).with_role(eve_bot::roles::default());
        cfg.policy.gm_setup = true;
        let (handle, thread) = spawn(cfg);
        Ok(vec![Member {
            name: "Crew1".to_owned(),
            handle,
            thread,
        }])
    });
    let controller = Arc::new(Mutex::new(Controller::new(hook)));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = run_link(d.clone(), Arc::clone(&controller), Arc::clone(&stop));
    let mut gm = login(addr, "Gina Gm");
    let mut reply = String::new();
    assert!(until(Duration::from_secs(20), || {
        reply = slash(&mut gm, "/bots spawn Crew red mine 1 all5 merlin");
        !reply.contains("no eve-bots")
    }));
    assert!(
        reply.starts_with("spawned 1 x mine in group Crew"),
        "{reply}"
    );
    let bot = {
        let c = controller.lock().unwrap();
        c.handle_of("crew1").expect("the bot's handle")
    };
    let reply = slash(&mut gm, "/bots pause crew");
    assert!(
        reply.contains("will park at their next docked turn"),
        "{reply}"
    );
    assert!(
        until(Duration::from_secs(40), || bot.is_parked()),
        "the docked bot never parked"
    );
    let reply = slash(&mut gm, "/bots inspect Crew1");
    assert!(
        reply.starts_with("Crew1: paused (parked); docked at")
            && reply.contains("act-log rows:")
            && reply.contains("parked by a GM"),
        "{reply}"
    );
    assert!(slash(&mut gm, "/bots list").contains("[paused]"));
    // While parked the bot adds no acts of its own beyond the park line.
    let rows = bot.log().len();
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(bot.log().len(), rows, "a parked bot acted");
    let reply = slash(&mut gm, "/bots resume Crew");
    assert!(reply.contains("running again"), "{reply}");
    assert!(
        until(Duration::from_secs(20), || !bot.is_parked()),
        "the bot never resumed"
    );
    assert!(bot.log().iter().any(|e| e.what.contains("resumed by a GM")));
    assert!(slash(&mut gm, "/bots stop-all").starts_with("stopped 1 group(s), 1 bot(s)"));
    assert_eq!(slash(&mut gm, "/bots list"), "no bot groups");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = thread.join();
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_server_that_is_not_dev_refuses_and_relays_nothing() {
    let d = dir("nodev");
    let addr = start(false, &d);
    let seen = fake_peer(&d);
    let mut gm = login(addr, "Gina Gm");
    let reply = slash(&mut gm, "/bots list");
    assert!(reply.contains("dev server"), "{reply}");
    std::thread::sleep(Duration::from_millis(200));
    assert!(seen.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn no_peer_connected_is_said() {
    let d = dir("nopeer");
    let addr = start(true, &d);
    let mut gm = login(addr, "Gina Gm");
    let reply = slash(&mut gm, "/bots list");
    assert!(reply.contains("no eve-bots is connected"), "{reply}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_real_peer_spawns_lists_brings_resides_and_despawns() {
    let d = dir("real");
    let addr = start(true, &d);
    let handles: Arc<Mutex<Vec<eve_bot::driver::Handle>>> = Arc::default();
    let kept = Arc::clone(&handles);
    let hook: eve_bot::control::SpawnHook = Box::new(move |spec: &Spawn| {
        // The one invented character stands for the whole group.
        let mut cfg =
            BotConfig::new("Bea Bot", addr, Pace::Fast).with_role(eve_bot::roles::default());
        cfg.policy.gm_setup = true;
        let (handle, thread) = spawn(cfg);
        kept.lock().unwrap().push(handle.clone());
        let _ = spec;
        Ok(vec![Member {
            name: "alpha-1".to_owned(),
            handle,
            thread,
        }])
    });
    let controller = Arc::new(Mutex::new(Controller::new(hook)));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = run_link(d.clone(), Arc::clone(&controller), Arc::clone(&stop));
    let mut gm = login(addr, "Gina Gm");
    // The peer dials in its own time: ask until it has.
    let mut reply = String::new();
    assert!(until(Duration::from_secs(20), || {
        reply = slash(&mut gm, "/bots spawn alpha red mine 1 all5 merlin");
        !reply.contains("no eve-bots")
    }));
    assert!(
        reply.starts_with("spawned 1 x mine in group alpha (red): alpha-1"),
        "{reply}"
    );
    let list = slash(&mut gm, "/bots list");
    assert!(
        list.starts_with("alpha side=red role=mine count=1 alive=1\n  alpha-1: "),
        "{list}"
    );
    assert!(slash(&mut gm, "/bots spawn alpha red mine 1 all5 merlin").contains("already exists"));
    assert!(slash(&mut gm, "/bots side alpha blue").contains("red -> blue"));
    assert_eq!(controller.lock().unwrap().side_of("alpha"), Some("blue"));
    // Bring: the bot takes the GM's station as its home on its next turn.
    let reply = slash(&mut gm, "/bots bring alpha");
    assert!(reply.contains("heading for station"), "{reply}");
    let bot = handles.lock().unwrap()[0].clone();
    assert!(
        until(Duration::from_secs(30), || bot.log().iter().any(|e| e.kind
            == ActKind::Travel
            && e.what.contains("brought to station"))),
        "the bot never took the GM's word"
    );
    assert_eq!(
        slash(&mut gm, "/bots despawn alpha"),
        "despawned 1 group(s), 1 bot(s)"
    );
    assert_eq!(slash(&mut gm, "/bots list"), "no bot groups");
    assert!(slash(&mut gm, "/bots despawn alpha").contains("no group"));
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = thread.join();
    let _ = std::fs::remove_dir_all(&d);
}

/// BOT-ACCOUNTS: on a dev server with world accounts, a `/bots spawn` of
/// names the world has never seen makes their accounts and characters, hands
/// the logins to the peer, and the bots log in, dock in the target system and
/// are logged off by a despawn.
#[test]
fn a_spawn_of_new_names_makes_their_accounts_and_the_bots_log_in() {
    use eve_server::account_book::{Book, ensure_bot_account, find};
    use eve_server::accounts::Accounts;
    use eve_server::server::Server;
    use std::sync::atomic::{AtomicBool, Ordering};

    const GM_PASSWORD: &str = "invented-gm-pass-1";
    let d = dir("accts");
    let mut cfg = config();
    cfg.dev = true;
    cfg.bot_control = Some(d.clone());
    cfg.handshake.accounts = Some(Arc::new(Accounts::in_world(Book::with_iterations(2))));
    let server = Server::bind(cfg).expect("bind");
    let addr = server.local_addr().expect("addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    // The GM's own account (a bot-marked one is fine for this).
    {
        let mut w = world.write().expect("world");
        let row = ensure_bot_account(w.deltas_mut(), "Gina Gm", GM_PASSWORD).expect("account");
        w.create_owned_character(row.user_id, "Gina Gm".to_owned());
    }
    let gone: Arc<Mutex<Vec<Arc<AtomicBool>>>> = Arc::default();
    let kept = Arc::clone(&gone);
    let handles: Arc<Mutex<Vec<eve_bot::driver::Handle>>> = Arc::default();
    let seen_handles = Arc::clone(&handles);
    let hook: eve_bot::control::SpawnHook = Box::new(move |spec: &Spawn| {
        let mut members = Vec::new();
        for i in 1..=spec.count {
            let name = format!("{}{i}", spec.group);
            let token = spec
                .tokens
                .get(&name)
                .ok_or_else(|| format!("no token for {name}"))?;
            let mut cfg = BotConfig::new(&name, addr, Pace::Fast)
                .with_role(eve_bot::roles::default())
                .with_token(token);
            cfg.policy.gm_setup = false;
            let (handle, thread) = spawn(cfg);
            seen_handles.lock().unwrap().push(handle.clone());
            let flag = Arc::new(AtomicBool::new(false));
            kept.lock().unwrap().push(Arc::clone(&flag));
            let watcher = std::thread::spawn(move || {
                let _ = thread.join();
                flag.store(true, Ordering::Relaxed);
            });
            members.push(Member {
                name,
                handle,
                thread: watcher,
            });
        }
        Ok(members)
    });
    let controller = Arc::new(Mutex::new(Controller::new(hook)));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = run_link(d.clone(), Arc::clone(&controller), Arc::clone(&stop));
    let mut gm = Conn::connect(addr, Duration::from_secs(10)).expect("connects");
    gm.handshake_as(&format!("Gina Gm:{GM_PASSWORD}"))
        .expect("the GM's login");
    gm.select_character_named("Gina Gm").expect("selects");
    let line = format!("/bots spawn alpha red mine 3 all5 merlin {SYSTEM}");
    let mut reply = String::new();
    assert!(until(Duration::from_secs(20), || {
        reply = slash(&mut gm, &line);
        !reply.contains("no eve-bots")
    }));
    assert!(
        reply.starts_with("spawned 3 x mine in group alpha (red): alpha1 alpha2 alpha3"),
        "{reply}"
    );
    // Accounts and characters exist, marked bot, in the side's corporation.
    // NAMES: the character has a pool name, so find it by its account.
    {
        let w = world.read().expect("world");
        let mut first = None;
        for n in ["alpha1", "alpha2", "alpha3"] {
            let row = find(w.deltas(), n).expect("an account");
            assert!(row.bot, "{n} is marked bot");
            let c = w
                .config()
                .characters
                .iter()
                .find(|c| c.owner_user_id == Some(row.user_id))
                .expect("a character");
            first.get_or_insert(c.character_id);
            assert!(c.is_bot && c.owner_user_id == Some(row.user_id));
            assert_eq!(c.solar_system_id, Some(SYSTEM));
        }
        let corp = first
            .and_then(|id| w.character(id))
            .map(|c| c.corporation_id);
        assert!(corp.is_some_and(|c| c != 0), "in a dev corporation");
    }
    // All three log in and are seen docked in the target system.
    let mut docked = [false; 3];
    assert!(
        until(Duration::from_secs(40), || {
            for (i, h) in handles.lock().unwrap().iter().enumerate() {
                if matches!(
                    h.observation().map(|o| o.place),
                    Some(eve_bot::brain::Place::Docked { .. })
                ) {
                    docked[i] = true;
                }
            }
            docked.iter().all(|d| *d)
        }),
        "docked: {docked:?}"
    );
    // The same names again reuse the accounts: a despawn, then a respawn.
    assert_eq!(
        slash(&mut gm, "/bots despawn alpha"),
        "despawned 1 group(s), 3 bot(s)"
    );
    assert!(
        until(Duration::from_secs(40), || gone
            .lock()
            .unwrap()
            .iter()
            .all(|f| f.load(Ordering::Relaxed))),
        "the bots logged off"
    );
    let first = find(world.read().expect("world").deltas(), "alpha1").map(|r| r.user_id);
    let again = slash(&mut gm, &line);
    assert!(again.starts_with("spawned 3 x mine"), "{again}");
    assert_eq!(
        find(world.read().expect("world").deltas(), "alpha1").map(|r| r.user_id),
        first,
        "the account is reused"
    );
    slash(&mut gm, "/bots despawn all");
    stop.store(true, Ordering::Relaxed);
    let _ = thread.join();
    let _ = std::fs::remove_dir_all(&d);
}

/// **The owner's natural-grammar spawn undocks** (roamer-fit): `/bots 2
/// allied escorts here` -- not the explicit `spawn` form -- reaches the peer
/// as a roster row with `skills role`, the hook builds the bots the way
/// `eve-bots` does (the role by name), and both come out of the station.
/// The 300 dev roamers sat docked for want of ammo; this holds the chain
/// from the words to the undock, with the GM setup standing in for the
/// dev market (the sandbox sells nothing).
#[test]
fn a_natural_grammar_spawn_undocks_within_a_bound() {
    use eve_server::account_book::{Book, ensure_bot_account};
    use eve_server::accounts::Accounts;
    use eve_server::server::Server;
    use std::sync::atomic::Ordering;

    const GM_PASSWORD: &str = "invented-gm-pass-2";
    let d = dir("words-seat");
    let mut cfg = config();
    cfg.dev = true;
    cfg.bot_control = Some(d.clone());
    cfg.handshake.accounts = Some(Arc::new(Accounts::in_world(Book::with_iterations(2))));
    let server = Server::bind(cfg).expect("bind");
    let addr = server.local_addr().expect("addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    {
        let mut w = world.write().expect("world");
        let row = ensure_bot_account(w.deltas_mut(), "Gina Gm", GM_PASSWORD).expect("account");
        w.create_owned_character(row.user_id, "Gina Gm".to_owned());
    }
    let handles: Arc<Mutex<Vec<eve_bot::driver::Handle>>> = Arc::default();
    let seen = Arc::clone(&handles);
    let rows: Arc<Mutex<Vec<Spawn>>> = Arc::default();
    let heard = Arc::clone(&rows);
    let hook: eve_bot::control::SpawnHook = Box::new(move |spec: &Spawn| {
        heard.lock().unwrap().push(spec.clone());
        let role = eve_bot::roles::parse(&spec.role).ok_or("a role")?;
        let mut members = Vec::new();
        for i in 1..=spec.count {
            let name = format!("{}{i}", spec.group);
            let token = spec.tokens.get(&name).ok_or("a token")?;
            let cfg = BotConfig::new(&name, addr, Pace::Fast)
                .with_role(role)
                .with_token(token);
            let (handle, thread) = spawn(cfg);
            seen.lock().unwrap().push(handle.clone());
            members.push(Member {
                name,
                handle,
                thread,
            });
        }
        Ok(members)
    });
    let controller = Arc::new(Mutex::new(Controller::new(hook)));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = run_link(d.clone(), Arc::clone(&controller), Arc::clone(&stop));
    let mut gm = Conn::connect(addr, Duration::from_secs(10)).expect("connects");
    gm.handshake_as(&format!("Gina Gm:{GM_PASSWORD}"))
        .expect("the GM's login");
    gm.select_character_named("Gina Gm").expect("selects");
    let mut reply = String::new();
    assert!(until(Duration::from_secs(20), || {
        reply = slash(&mut gm, "/bots 2 allied escorts here");
        !reply.contains("no eve-bots")
    }));
    assert!(reply.contains("spawned 2 x escort"), "{reply}");
    let row = rows.lock().unwrap()[0].clone();
    assert_eq!((row.role.as_str(), row.skills.as_str()), ("escort", "role"));
    let out = |handles: &[eve_bot::driver::Handle]| {
        handles.len() == 2
            && handles.iter().all(|h| {
                h.observation()
                    .is_some_and(|o| matches!(o.place, eve_bot::brain::Place::InSpace { .. }))
            })
    };
    let undocked = until(Duration::from_secs(90), || {
        out(&handles.lock().unwrap().clone())
    });
    let log: Vec<String> = handles
        .lock()
        .unwrap()
        .iter()
        .flat_map(|h| h.log())
        .map(|e| format!("{:?} {}", e.kind, e.what))
        .collect();
    slash(&mut gm, "/bots despawn all");
    stop.store(true, Ordering::Relaxed);
    let _ = thread.join();
    let _ = std::fs::remove_dir_all(&d);
    assert!(undocked, "both undocked; their acts:\n{}", log.join("\n"));
}

/// GANG-SUMMON: `/bots come alpha` over the real link calls a group to the
/// GM, who is in space far from the station: the (non-gang) bot is told
/// `summon <group> <system> <character>`, undocks if it has to and warps to
/// the GM's character.
#[test]
fn come_calls_a_group_to_a_gm_in_space() {
    const BELT: i64 = 40_977_301;
    let d = dir("come");
    let mut cfg = config();
    cfg.dev = true;
    cfg.bot_control = Some(d.clone());
    let home = eve_server::model::Character::default();
    cfg.sandbox.characters = vec![
        pilot(GM, "Gina Gm", 100_000_951),
        pilot(BEA, "Bea Bot", 100_000_952),
    ];
    cfg.sandbox.celestials = eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{SYSTEM}\t{}\t{}\n\
         station\t{}\t{}\t5000.0\t0.0\t0.0\t0.0\t{SYSTEM}\t0\t0\n\
         belt\t{BELT}\t15\t0.0\t40000000.0\t0.0\t0.0\t{SYSTEM}\t0\t0\n",
        home.constellation_id,
        home.region_id,
        home.station_id.expect("docked"),
        home.station_type_id,
    ));
    let addr = serve(cfg);
    let handles: Arc<Mutex<Vec<eve_bot::driver::Handle>>> = Arc::default();
    let kept = Arc::clone(&handles);
    let hook: eve_bot::control::SpawnHook = Box::new(move |_: &Spawn| {
        let mut cfg =
            BotConfig::new("Bea Bot", addr, Pace::Fast).with_role(eve_bot::roles::default());
        cfg.policy.gm_setup = true;
        let (handle, thread) = spawn(cfg);
        kept.lock().unwrap().push(handle.clone());
        Ok(vec![Member {
            name: "alpha-1".to_owned(),
            handle,
            thread,
        }])
    });
    let controller = Arc::new(Mutex::new(Controller::new(hook)));
    let stop = Arc::new(AtomicBool::new(false));
    let thread = run_link(d.clone(), Arc::clone(&controller), Arc::clone(&stop));
    let mut gm = login(addr, "Gina Gm");
    // The GM undocks and sits at a belt, a long warp from the station.
    gm.undock().expect("undocks");
    gm.enter_space(SYSTEM).expect("the state");
    gm.warp_to_item(BELT, 0.0).expect("warps to the belt");
    let mut reply = String::new();
    assert!(until(Duration::from_secs(20), || {
        reply = slash(&mut gm, "/bots spawn alpha ally mine 1 all5 merlin");
        !reply.contains("no eve-bots")
    }));
    assert!(reply.starts_with("spawned 1"), "{reply}");
    let reply = slash(&mut gm, "/bots come alpha");
    assert_eq!(
        reply,
        format!("1 bot(s) of alpha are coming to {GM} in system {SYSTEM}")
    );
    let bot = handles.lock().unwrap()[0].clone();
    let warped = until(Duration::from_secs(90), || {
        let _ = gm.collect_pushes(Duration::from_millis(25));
        bot.log()
            .iter()
            .any(|e| e.kind == ActKind::WarpToMember && e.ids == vec![GM])
    });
    let log: Vec<String> = bot
        .log()
        .iter()
        .map(|e| format!("{:?} {} {:?}", e.kind, e.what, e.verdict))
        .collect();
    let called = bot
        .log()
        .iter()
        .any(|e| e.kind == ActKind::Travel && e.what.contains("called to"));
    slash(&mut gm, "/bots despawn alpha");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = thread.join();
    let _ = std::fs::remove_dir_all(&d);
    assert!(called, "the bot never took the call:\n{}", log.join("\n"));
    assert!(warped, "it never warped to the GM:\n{}", log.join("\n"));
}
