//! Accounts in the world: sign-up, login and grant (ACC2, owner decisions
//! O9 and O10; `docs/lanes/acc2.md`).
//!
//! With `--world-accounts` every account is one `account` delta row of the
//! world file, saved and loaded like every other row (`docs/content/plan.md`
//! §4.2), and every character is owned by one through
//! `Character::owner_user_id`, which the world file already keeps. Nothing
//! else holds an account: [`Book`] is only the hash cost and the in-memory
//! failure backoff.
//!
//! **How a login arrives.** Our launcher starts the client with
//! `/ssoToken=TOKEN` (`tools/run-client.sh`), and on that path the client
//! sends the literal user name `EVE-SSO-CONNECTION` and the password
//! `fakePW` (`docs/reference/login-path.md` §3), so the token is the only per-player
//! input. With world accounts it is `USERNAME:PASSWORD`, split at the first
//! `:`. The client splits its argument on `=`, so a password cannot hold
//! one [V] (`gameui.Startup`).
//!
//! **What a login does** ([`login`]): a known name with the right password
//! is that account, its role capped by the listener's ceiling; a wrong
//! password is refused; an unknown name signs up **only** on a listener
//! with `signup=on`, as a `player` (O10) with one new character of the same
//! name [A: the client here cannot create characters (`roles.rs`,
//! `ccSvc.CreateCharacterWithDoll`), so sign-up makes the one it plays;
//! falsified by a friend who wants another name, which then needs a
//! character-creation packet]. A refusal is sent to the client as a
//! `GPSTransportClosed` in place of H7 ([`refusal`]).
//!
//! The password is hashed with PBKDF2-HMAC-SHA256 under a random salt and
//! compared in constant time. **No log line, error or row ever holds a
//! password**, and the row's salt and hash are never printed.

use crate::accounts::{Refused, Resolved, Role, ct_eq, hex, pbkdf2_hmac_sha256};
use crate::delta::Deltas;
use crate::server::Listener;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use std::collections::HashMap;
use std::io::{self, Read};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

/// The delta kind (`crate::delta_kinds::KINDS`):
/// `user_id -> username role created scheme iterations salt hash [bot]`.
/// The eighth field (BOTMARK) is written only on a bot-marked row, so an
/// unmarked row is byte-for-byte what ACC2 wrote and an ACC2 row loads.
pub const KIND: &str = "account";

/// The eighth field of a bot-marked row ([`Row::bot`]).
pub const BOT_FIELD: &str = "bot";

/// The one hash scheme a row may name. A later scheme is a new name, so an
/// old row still verifies.
pub const SCHEME: &str = "pbkdf2-sha256";

/// PBKDF2 iterations for a new account. [A] slow enough that a leaked world
/// file costs a guesser real time per guess, fast enough that a login stays
/// well under a second in a debug build: measured 2026-10-26 on the build
/// container, one hash took 0.62 s in debug and 0.04 s in release. OWASP's
/// 2023 figure for this PRF is 600,000, which fits once release is the
/// default (O14); each row keeps its own count, so raising this leaves old
/// rows verifiable. Falsified by a login that stalls visibly, or by a
/// leaked file being guessed faster than the owner accepts.
pub const PBKDF2_ITERATIONS: u32 = 100_000;

/// The fewest iterations a human sign-up's hash may have. Two, not one: a
/// single iteration is allowed only for provision-generated random
/// passwords ([`BOT_SPAWN_ITERATIONS`]); a person's password is guessable
/// and needs the stretching, so [`Book`] never goes below this.
pub const MIN_HUMAN_ITERATIONS: u32 = 2;

/// Salt bytes per account, from `/dev/urandom`.
pub const SALT_LEN: usize = 16;

/// A user name's length in bytes. [A] three is the client's own minimum
/// for a character name and 24 keeps the select screen tidy; falsified by
/// a friend whose name does not fit.
pub const NAME_LEN: std::ops::RangeInclusive<usize> = 3..=24;

/// The shortest password a sign-up accepts. [A] long enough that guessing
/// over the tunnel, at the backoff below, is hopeless; falsified by a
/// friend who cannot remember one.
pub const MIN_PASSWORD_LEN: usize = 8;

/// The longest password, so a hostile token cannot make a hash slow.
pub const MAX_PASSWORD_LEN: usize = 256;

/// Failures a user name may have before the backoff starts. [A] covers a
/// few typos; falsified by a friend locked out by typing alone.
pub const FREE_FAILURES_PER_NAME: u32 = 5;

/// Failures a peer address may have before the backoff starts. [A] larger
/// than the per-name allowance for a shared NAT. A **loopback** peer has no
/// peer key at all: every friend arrives through the sshd container, so
/// behind the tunnel they share 127.0.0.1, and one key for all of them let
/// twenty typos lock every friend out (F-HOST H4); the per-name backoff
/// still guards each account. Falsified by a guesser who rotates names
/// from a non-loopback peer past this count unchecked.
pub const FREE_FAILURES_PER_PEER: u32 = 20;

/// The first backoff, doubled on each further failure. [A]
pub const BACKOFF_FIRST: Duration = Duration::from_secs(2);

/// The longest backoff. [A] long enough to make guessing hopeless, short
/// enough that a locked-out friend waits minutes, not a day; falsified by
/// either.
pub const BACKOFF_MAX: Duration = Duration::from_secs(300);

/// The in-memory half of world accounts: the hash cost and the backoff.
/// Shared by every connection through `Accounts`.
#[derive(Debug)]
pub struct Book {
    iterations: u32,
    backoff: Mutex<Backoff>,
}

impl Default for Book {
    fn default() -> Self {
        Book::with_iterations(PBKDF2_ITERATIONS)
    }
}

impl Book {
    /// A book hashing new accounts with `iterations` (tests use a small
    /// count: the count is a row's own field, never asserted).
    pub fn with_iterations(iterations: u32) -> Book {
        Book {
            iterations: iterations.max(MIN_HUMAN_ITERATIONS),
            backoff: Mutex::default(),
        }
    }

    fn backoff(&self) -> std::sync::MutexGuard<'_, Backoff> {
        self.backoff.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Failures by key (`name:<listener>:<lowercase>` or `peer:<ip>`): how many, the last,
/// and until when a login is refused unread.
#[derive(Debug, Default)]
struct Backoff {
    by_key: HashMap<String, (u32, Instant, Instant)>,
}

impl Backoff {
    fn blocked(&self, key: &str, now: Instant) -> bool {
        self.by_key
            .get(key)
            .is_some_and(|(_, _, until)| now < *until)
    }

    fn fail(&mut self, key: String, free: u32, now: Instant) {
        if self.by_key.len() > 4096 {
            self.by_key
                .retain(|_, (_, last, _)| now.duration_since(*last) < BACKOFF_MAX);
        }
        let e = self.by_key.entry(key).or_insert((0, now, now));
        e.0 += 1;
        e.1 = now;
        if e.0 > free {
            let doublings = (e.0 - free - 1).min(16);
            e.2 = now + (BACKOFF_FIRST * 2u32.pow(doublings)).min(BACKOFF_MAX);
        }
    }

    fn clear(&mut self, key: &str) {
        self.by_key.remove(key);
    }
}

/// One account row.
#[derive(Clone, PartialEq, Eq)]
pub struct Row {
    pub user_id: i64,
    /// As signed up; unique ignoring ASCII case.
    pub username: String,
    pub role: Role,
    /// FILETIME of the sign-up.
    pub created: i64,
    /// **The bot mark** (BOTMARK): every character this account owns is
    /// `Character::is_bot`, so liveness and the economy count it as a bot.
    /// Set only offline ([`mark_bot_offline`], `eve-server --mark-bot`);
    /// no slash command or packet reaches it, and it never changes `role`.
    pub bot: bool,
    iterations: u32,
    salt: Vec<u8>,
    hash: [u8; 32],
}

/// Never the salt or the hash.
impl std::fmt::Debug for Row {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Row({} {:?} {}{})",
            self.user_id,
            self.username,
            self.role.name(),
            if self.bot { " bot" } else { "" }
        )
    }
}

impl Row {
    fn new(user_id: i64, username: &str, password: &[u8], iterations: u32, salt: Vec<u8>) -> Row {
        let mut hash = [0u8; 32];
        pbkdf2_hmac_sha256(password, &salt, iterations, &mut hash);
        Row {
            user_id,
            username: username.to_owned(),
            role: Role::Player,
            created: crate::pyutil::filetime_now(),
            bot: false,
            iterations,
            salt,
            hash,
        }
    }

    /// Whether `password` is this account's, compared in constant time.
    pub fn verify(&self, password: &[u8]) -> bool {
        let mut got = [0u8; 32];
        pbkdf2_hmac_sha256(password, &self.salt, self.iterations, &mut got);
        ct_eq(&got, &self.hash)
    }

    fn key(&self) -> Vec<String> {
        vec![self.user_id.to_string()]
    }

    fn value(&self) -> Vec<String> {
        let mut v = vec![
            self.username.clone(),
            self.role.name().to_owned(),
            self.created.to_string(),
            SCHEME.to_owned(),
            self.iterations.to_string(),
            hex(&self.salt),
            hex(&self.hash),
        ];
        if self.bot {
            v.push(BOT_FIELD.to_owned());
        }
        v
    }

    fn parse(key: &[String], value: &[String]) -> Option<Row> {
        let [user_id] = key else { return None };
        let (bot, value) = match value {
            [rest @ .., last] if rest.len() == 7 && last == BOT_FIELD => (true, rest),
            _ => (false, value),
        };
        let [username, role, created, scheme, iterations, salt, hash] = value else {
            return None;
        };
        if scheme != SCHEME {
            return None;
        }
        Some(Row {
            user_id: user_id.parse().ok()?,
            username: username.clone(),
            role: Role::parse(role)?,
            created: created.parse().ok()?,
            bot,
            iterations: iterations.parse().ok().filter(|n| *n > 0)?,
            salt: unhex(salt)?,
            hash: unhex(hash)?.try_into().ok()?,
        })
    }
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Every well-formed account row. A malformed one is skipped here and kept
/// verbatim in the file.
pub fn rows(d: &Deltas) -> impl Iterator<Item = Row> + '_ {
    d.of(KIND).iter().filter_map(|(k, v)| Row::parse(k, v))
}

/// The account named `username`, ignoring ASCII case.
pub fn find(d: &Deltas, username: &str) -> Option<Row> {
    rows(d).find(|r| r.username.eq_ignore_ascii_case(username))
}

/// Set `username`'s role (O10's grant), answering one line for the person
/// who asked.
pub fn set_role(d: &mut Deltas, username: &str, role: Role) -> Result<String, String> {
    let mut row = find(d, username).ok_or_else(|| format!("no account named {username:?}"))?;
    let was = row.role;
    row.role = role;
    d.put(KIND, row.key(), row.value());
    Ok(format!(
        "account {:?} (user {}): {} -> {}; it takes effect at the next login",
        row.username,
        row.user_id,
        was.name(),
        role.name()
    ))
}

/// Set or clear `username`'s bot mark, answering one line. Never touches
/// the role. Reached only from [`mark_bot_offline`]: there is no in-game
/// path (no slash row, no packet).
fn set_bot(d: &mut Deltas, username: &str, bot: bool) -> Result<String, String> {
    let mut row = find(d, username).ok_or_else(|| format!("no account named {username:?}"))?;
    let was = row.bot;
    row.bot = bot;
    d.put(KIND, row.key(), row.value());
    let word = |b: bool| if b { "bot" } else { "not a bot" };
    Ok(format!(
        "account {:?} (user {}, {}): {} -> {}; its characters follow at the next start",
        row.username,
        row.user_id,
        row.role.name(),
        word(was),
        word(bot)
    ))
}

/// The hash cost of a bot account made at run time (BOT-ACCOUNTS). The
/// password is 128 random bits, so the cost only buys time for the dev
/// server's world lock: 200 spawned bots must not hold it for 8 s. A row
/// keeps its own count, so this never affects another account.
pub const BOT_ITERATIONS: u32 = 1_000;

/// **BOT-ACCOUNTS**: make (or reuse) the bot account `name` with `password`,
/// a `gm` account carrying the bot mark, as `eve-server --mark-bot` would.
/// An existing account must already be a bot's, and gets the new password
/// (the old one is unknown here: only its hash is kept). Dev servers only
/// (the caller checks); the password is never logged. Returns the row.
pub fn ensure_bot_account(d: &mut Deltas, name: &str, password: &str) -> Result<Row, String> {
    let salt = urandom_salt().map_err(|e| format!("/dev/urandom: {e}"))?;
    ensure_bot_account_salted(d, name, password, salt)
}

/// [`ensure_bot_account`] with the salt given.
pub fn ensure_bot_account_salted(
    d: &mut Deltas,
    name: &str,
    password: &str,
    salt: Vec<u8>,
) -> Result<Row, String> {
    let mut made = ensure_bot_accounts(d, &[(name, password, salt)], BOT_ITERATIONS)?;
    made.pop().ok_or_else(|| "no account made".to_owned())
}

/// The hash cost of a `/bots spawn` account: one iteration.
///
/// **Why one is acceptable here and nowhere else.** PBKDF2 stretches a
/// secret an attacker can guess; it adds nothing to a secret they cannot.
/// `bot_control::provision` makes every spawn password from 128 bits of
/// `/dev/urandom`, so a stolen row falls only to a 2^128 search at any
/// iteration count, while [`BOT_ITERATIONS`] cost the world lock ~1.5 ms a
/// bot (300 bots held it ~480 ms, measured 2026-10-29). A password a person
/// chose (sign-up, `--mark-bot`) must NEVER take this count: [`Book`] floors
/// at [`MIN_HUMAN_ITERATIONS`]. [A: `provision` is the only caller that
/// passes this constant, with random passwords; falsified by any other use
/// of `BOT_SPAWN_ITERATIONS` in a grep.] Each row stores its own count, so
/// [`Row::verify`] reads it back.
pub const BOT_SPAWN_ITERATIONS: u32 = 1;

/// **Batch BOT-ACCOUNTS**: [`ensure_bot_account`] for every `(name, password,
/// salt)`, hashing at `iterations`. The rows are read once (the per-bot scan
/// was O(accounts) each) and the next user id is computed once and counted
/// up. Every name is checked before anything is written, so a refused name
/// leaves the book as it was. Returns the rows in order.
pub fn ensure_bot_accounts(
    d: &mut Deltas,
    bots: &[(&str, &str, Vec<u8>)],
    iterations: u32,
) -> Result<Vec<Row>, String> {
    let mut by_name: HashMap<String, Row> = HashMap::new();
    let mut next = crate::accounts::FIRST_USER_ID;
    for r in rows(d) {
        next = next.max(r.user_id.saturating_add(1));
        by_name.insert(r.username.to_ascii_lowercase(), r);
    }
    let mut made: Vec<Row> = Vec::with_capacity(bots.len());
    for (name, password, salt) in bots {
        check_name(name)?;
        let lower = name.to_ascii_lowercase();
        let mut row = Row::new(0, name, password.as_bytes(), iterations, salt.clone());
        match by_name.get(&lower) {
            Some(old) if !old.bot => {
                return Err(format!(
                    "an account named {name:?} exists and is not a bot's; pick another group name"
                ));
            }
            Some(old) => {
                row.user_id = old.user_id;
                row.username = old.username.clone();
                row.created = old.created;
            }
            None => {
                row.user_id = next;
                next = next.saturating_add(1);
            }
        }
        row.role = Role::Gm;
        row.bot = true;
        by_name.insert(lower, row.clone());
        made.push(row);
    }
    for row in &made {
        d.put(KIND, row.key(), row.value());
    }
    Ok(made)
}

/// Whether `user_id` is a bot-marked account.
pub fn is_bot_account(d: &Deltas, user_id: i64) -> bool {
    // A keyed lookup (the row key is the user id), not a parse of every row:
    // `create_character` asks this once per character, and a `/bots spawn` of
    // 300 over ~1000 accounts parsed 300 000 rows for it.
    let key = vec![user_id.to_string()];
    d.of(KIND)
        .get_key_value(&key)
        .and_then(|(k, v)| Row::parse(k, v))
        .is_some_and(|r| r.bot)
}

/// The user ids of every bot-marked account: what
/// `Registry::mark_bot_characters` turns into `Character::is_bot`.
pub fn bot_accounts(d: &Deltas) -> std::collections::BTreeSet<i64> {
    rows(d).filter(|r| r.bot).map(|r| r.user_id).collect()
}

/// A sign-up's name: ASCII letters, digits, space, `-`, `_` and `'`, no
/// leading, trailing or doubled space, [`NAME_LEN`] long
/// (`crate::start::name_shape`).
fn check_name(name: &str) -> Result<(), String> {
    // One rule for every visible name: `crate::start::name_shape` is also
    // what `charUnboundMgr.ValidateNameEx` answers with (P-START-4).
    if crate::start::name_shape(name) != crate::start::NAME_VALID {
        return Err(format!(
            "a user name is {} to {} letters, digits, spaces, - _ or '",
            NAME_LEN.start(),
            NAME_LEN.end()
        ));
    }
    Ok(())
}

/// A sign-up's password: long enough, not the name, not a token our own
/// tools send (ruling 2: nothing guessable from a visible name).
fn check_password(name: &str, password: &str) -> Result<(), String> {
    if password.len() < MIN_PASSWORD_LEN {
        return Err(format!(
            "a password is at least {MIN_PASSWORD_LEN} characters"
        ));
    }
    if password.eq_ignore_ascii_case(name)
        || crate::accounts::TOOL_TOKENS
            .iter()
            .any(|t| password.eq_ignore_ascii_case(t))
    {
        return Err("that password is too easy to guess".to_owned());
    }
    Ok(())
}

/// Who a world-accounts connection is (the H6 decision), **taking the world
/// lock only briefly**: the hash runs with no lock held, so a login never
/// stalls the other players. Called by `Handshake::pre_login` before the
/// dispatcher takes the lock for the turn.
pub fn login(
    book: &Book,
    listener: &Listener,
    peer: Option<IpAddr>,
    token: Option<&[u8]>,
    world: &RwLock<Registry>,
) -> Result<Resolved, Refused> {
    let token = token.filter(|t| !t.is_empty()).ok_or(Refused::NoToken)?;
    let token = std::str::from_utf8(token).map_err(|_| Refused::NotUserPassword)?;
    let (name, password) = token.split_once(':').ok_or(Refused::NotUserPassword)?;
    if name.is_empty() || password.is_empty() || password.len() > MAX_PASSWORD_LEN {
        return Err(Refused::NotUserPassword);
    }
    // Per listener: behind the tunnel a friend's bad logins on 26001 must not
    // lock the owner's own name out of 26002.
    let name_key = format!("name:{}:{}", listener.addr, name.to_ascii_lowercase());
    // No peer key for loopback: behind the tunnel it is every friend at once.
    let peer_key = peer
        .filter(|p| !p.is_loopback())
        .map(|p| format!("peer:{p}"));
    let now = Instant::now();
    {
        let b = book.backoff();
        if b.blocked(&name_key, now) || peer_key.as_ref().is_some_and(|k| b.blocked(k, now)) {
            return Err(Refused::Backoff);
        }
    }
    let fail = |why: Refused| {
        let mut b = book.backoff();
        b.fail(name_key.clone(), FREE_FAILURES_PER_NAME, now);
        if let Some(k) = &peer_key {
            b.fail(k.clone(), FREE_FAILURES_PER_PEER, now);
        }
        Err(why)
    };
    let found = find(
        crate::service::world_read_timed(world, "login").deltas(),
        name,
    );
    let row = match found {
        Some(row) => {
            if !row.verify(password.as_bytes()) {
                return fail(Refused::WrongPassword {
                    user_id: row.user_id,
                });
            }
            row
        }
        None if !listener.signup => return fail(Refused::NoSuchUser),
        None => match sign_up(book, name, password, world, listener.bot_names) {
            Ok(row) => row,
            Err(why) => return fail(why),
        },
    };
    let mut b = book.backoff();
    b.clear(&name_key);
    if let Some(k) = &peer_key {
        b.clear(k);
    }
    Ok(Resolved {
        user_id: row.user_id,
        role: row.role.capped(listener.ceiling),
    })
}

/// Make a new `player` account and its character. The hash runs before the
/// lock; under it the name is checked again, so two sign-ups racing for one
/// name make one account.
fn sign_up(
    book: &Book,
    name: &str,
    password: &str,
    world: &RwLock<Registry>,
    bot_names: bool,
) -> Result<Row, Refused> {
    check_name(name).map_err(Refused::SignupRefused)?;
    check_password(name, password).map_err(Refused::SignupRefused)?;
    let salt = urandom_salt().map_err(|e| {
        crate::errln!("[accounts] /dev/urandom: {e}");
        Refused::SignupRefused("the server could not make a salt".to_owned())
    })?;
    let mut row = Row::new(0, name, password.as_bytes(), book.iterations, salt);
    let mut w = crate::service::world_write_timed(world, "sign_up");
    if find(w.deltas(), name).is_some() {
        return Err(Refused::SignupRefused(
            "that name was taken a moment ago".to_owned(),
        ));
    }
    if w.config()
        .characters
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case(name))
    {
        return Err(Refused::SignupRefused(
            "a character already has that name".to_owned(),
        ));
    }
    row.user_id = rows(w.deltas())
        .map(|r| r.user_id.saturating_add(1))
        .max()
        .unwrap_or(crate::accounts::FIRST_USER_ID)
        .max(crate::accounts::FIRST_USER_ID);
    w.deltas_mut().put(KIND, row.key(), row.value());
    // `botnames=on` (NAMES): a bot signs up as `hulka3` and its character is
    // a pool name; the account name stays the key every tool uses.
    let character_name = if bot_names {
        taken_free_pick(&w, name)
    } else {
        name.to_owned()
    };
    let character_id = w.create_owned_character(row.user_id, character_name.clone());
    crate::outln!(
        "[accounts] signed up {name:?}: user {} (player), character {character_id} {character_name:?}",
        row.user_id
    );
    Ok(row)
}

/// The pool name for bot key `key` that no character, account or tool token
/// has (`crate::bot_names`); the key itself when the pool is empty or used up.
pub fn taken_free_pick(w: &Registry, key: &str) -> String {
    TakenNames::new(w).pick(key)
}

/// The names a pool pick must avoid, read once: every character name, every
/// account name and every tool token (lower case). A `/bots spawn` of 300
/// picks 300 names, and the per-pick scans of the characters and a parse of
/// every account row per candidate cost ~100 ms under the world lock.
pub struct TakenNames(std::collections::HashSet<String>);

impl TakenNames {
    pub fn new(w: &Registry) -> TakenNames {
        let mut set = std::collections::HashSet::new();
        set.extend(
            w.config()
                .characters
                .iter()
                .map(|c| c.name.to_ascii_lowercase()),
        );
        set.extend(rows(w.deltas()).map(|r| r.username.to_ascii_lowercase()));
        set.extend(
            crate::accounts::TOOL_TOKENS
                .iter()
                .map(|t| t.to_ascii_lowercase()),
        );
        TakenNames(set)
    }

    /// [`taken_free_pick`]'s pick, and the name is taken from here on (the
    /// caller makes the character).
    pub fn pick(&mut self, key: &str) -> String {
        let name =
            crate::bot_names::pick_for_key(key, |n| self.0.contains(&n.to_ascii_lowercase()))
                .unwrap_or_else(|| key.to_owned());
        self.0.insert(name.to_ascii_lowercase());
        name
    }
}

#[cfg(test)]
fn read(world: &RwLock<Registry>) -> std::sync::RwLockReadGuard<'_, Registry> {
    world.read().unwrap_or_else(|e| e.into_inner())
}

fn urandom_salt() -> io::Result<Vec<u8>> {
    let mut salt = vec![0u8; SALT_LEN];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut salt)?;
    Ok(salt)
}

/// What the client is sent in place of H7 when a world-accounts login is
/// refused: a marshalled `GPSTransportClosed`, the one exception
/// `machobase.Loads` raises on receipt [V] (`machobase.Loads`,
/// `GPS.Authenticate`). `machoNet.ConnectToServer` turns a `reasonCode`
/// that is a dialog into `UserError(reasonCode)`, and any other into
/// `OnConnectionFailed {'what': reason}` [V]. A wrong name or password gets
/// `LoginAuthFailed`, the login form's own wrong-password dialog [V]
/// (`loginII._Connect`, `findings/user-errors.tsv`) [A: that it is in
/// `cfg.messages`, so the client shows it rather than the reason text;
/// falsified by an "OnConnectionFailed" dialog on a wrong password]. The
/// other refusals carry their reason as text.
pub fn refusal(why: &Refused) -> PyValue {
    let (code, text) = match why {
        Refused::SignupRefused(reason) => (None, format!("Sign-up refused: {reason}.")),
        Refused::NotUserPassword => (
            None,
            "This server wants /ssoToken=USERNAME:PASSWORD.".to_owned(),
        ),
        Refused::Backoff => (
            None,
            "Too many failed logins. Wait a few minutes and try again.".to_owned(),
        ),
        _ => (
            Some("LoginAuthFailed"),
            "Wrong user name or password.".to_owned(),
        ),
    };
    transport_closed(&text, code)
}

/// A marshalled `GPSTransportClosed` with `text` and `reasonCode`. It is the
/// refusal's shape, and NET-TAKEOVER's close for the connection a newer login
/// replaces (`reasonCode` `CLIENTDISCONNECTED`, `machoNet.py:1292,2486`; mid-session
/// `macho.Loads` raises it and the transport closes, `machoNetTransport.py:413-415`).
pub fn transport_closed(text: &str, code: Option<&str>) -> PyValue {
    use crate::pyutil::{bstr, pydict};
    PyValue::ObjectEx {
        form: eve_marshal::ObjectForm::Call,
        header: Box::new(PyValue::Tuple(vec![
            PyValue::Global(b"carbon.common.script.net.GPSExceptions.GPSTransportClosed".to_vec()),
            PyValue::Tuple(vec![bstr(text)]),
            pydict(vec![
                ("reasonCode", code.map_or(PyValue::None, bstr)),
                ("reasonArgs", PyValue::Dict(Vec::new())),
            ]),
        ])),
        list: Vec::new(),
        dict: Vec::new(),
    }
}

/// `/grant <username> <player|gm>`: a GM sets an account's role (O10). The
/// slash service already refuses a session without `ROLE_GML`, so only a
/// GM on a `ceiling=gm` listener reaches this.
pub(crate) fn grant(
    reg: &mut Registry,
    _actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let said = |s: String| Ok(crate::pyutil::bstr(&s));
    let Some((role, name)) = args.split_last().filter(|(_, n)| !n.is_empty()) else {
        return said("usage: /grant <username> <player|gm>".to_owned());
    };
    let Some(role) = Role::parse(role) else {
        return said(format!("role {role}: expected player or gm"));
    };
    let name = name.join(" ");
    match set_role(reg.deltas_mut(), &name, role) {
        Ok(line) => {
            crate::outln!("[accounts] /grant: {line}");
            said(line)
        }
        Err(e) => said(e),
    }
}

/// The lock file beside a world file, held for the server's life so that
/// `--grant` cannot write the file under a running server.
pub fn lock_path(world_file: &Path) -> PathBuf {
    let mut name = world_file.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    world_file.with_file_name(name)
}

/// Take the world file's lock, or say who holds it. The lock is the
/// kernel's (`flock`), so a killed server leaves none behind.
pub fn lock_world(world_file: &Path) -> io::Result<std::fs::File> {
    let path = lock_path(world_file);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    file.try_lock().map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::WouldBlock,
            format!(
                "{} is locked: an eve-server is running with this world; stop it first",
                path.display()
            ),
        ),
        std::fs::TryLockError::Error(e) => e,
    })?;
    Ok(file)
}

/// `eve-server --world FILE --grant USERNAME ROLE`: set a role in the world
/// file, offline. Refused while a server holds the world's lock.
pub fn grant_offline(world_file: &Path, username: &str, role: &str) -> Result<String, String> {
    let role = Role::parse(role).ok_or_else(|| format!("role {role}: expected player or gm"))?;
    edit_offline(world_file, |d| set_role(d, username, role))
}

/// `eve-server --world FILE --mark-bot USERNAME [off]`: set (or clear) an
/// account's bot mark in the world file, offline, under the world's lock
/// like [`grant_offline`]. The only way to set it.
pub fn mark_bot_offline(world_file: &Path, username: &str, bot: bool) -> Result<String, String> {
    edit_offline(world_file, |d| set_bot(d, username, bot))
}

fn edit_offline(
    world_file: &Path,
    edit: impl FnOnce(&mut Deltas) -> Result<String, String>,
) -> Result<String, String> {
    use crate::store::Store;
    let _lock = lock_world(world_file).map_err(|e| e.to_string())?;
    let store = crate::store::FileStore::new(world_file.to_path_buf());
    let mut snapshot = store
        .load()
        .map_err(|e| format!("{}: {e}", world_file.display()))?
        .ok_or_else(|| format!("{}: no world file yet", world_file.display()))?;
    let line = edit(&mut snapshot.deltas)?;
    store
        .save(&snapshot)
        .map_err(|e| format!("{}: {e}", world_file.display()))?;
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::Accounts;
    use crate::model::SandboxConfig;
    use crate::server::Ceiling;
    use crate::service::Caller;

    /// Invented names and passwords; a two-iteration hash keeps tests fast
    /// (the count is never asserted).
    const ANN: &str = "Invented Ann";
    const ANN_PW: &str = "acc2-invented-pass-1";

    fn book() -> Book {
        Book::with_iterations(2)
    }

    fn world() -> RwLock<Registry> {
        let mut config = SandboxConfig {
            characters: Vec::new(),
            ..SandboxConfig::default()
        };
        Accounts::in_world(book()).configure(&mut config);
        RwLock::new(Registry::new(config))
    }

    fn listener(signup: bool, ceiling: Ceiling) -> Listener {
        Listener {
            signup,
            ceiling,
            ..Listener::new("127.0.0.1:0".parse().unwrap())
        }
    }

    fn token(name: &str, pw: &str) -> Vec<u8> {
        format!("{name}:{pw}").into_bytes()
    }

    fn try_login(
        b: &Book,
        l: &Listener,
        w: &RwLock<Registry>,
        name: &str,
        pw: &str,
    ) -> Result<Resolved, Refused> {
        login(b, l, None, Some(&token(name, pw)), w)
    }

    /// The names on the select screen `user_id` is shown.
    fn roster(w: &RwLock<Registry>, user_id: i64) -> Vec<String> {
        let reg = read(w);
        let who = Caller::Session {
            user_id,
            role: Role::Player.mask(),
        };
        reg.config()
            .characters
            .iter()
            .filter(|c| reg.may_play(who, c))
            .map(|c| c.name.clone())
            .collect()
    }

    #[test]
    fn a_botnames_signup_keeps_the_account_key_and_names_the_character_from_the_pool() {
        let (b, w) = (book(), world());
        let l = Listener {
            bot_names: true,
            ..listener(true, Ceiling::Player)
        };
        let one = try_login(&b, &l, &w, "invbota1", "pw-invented-1x").unwrap();
        let two = try_login(&b, &l, &w, "invbota2", "pw-invented-2x").unwrap();
        let (n1, n2) = (roster(&w, one.user_id), roster(&w, two.user_id));
        assert_eq!((n1.len(), n2.len()), (1, 1));
        assert_ne!(n1, n2);
        assert!(crate::bot_names::pool().contains(&n1[0]));
        assert!(find(read(&w).deltas(), "invbota1").is_some());
        // Without the flag the character is the account's name.
        let plain = try_login(
            &b,
            &listener(true, Ceiling::Player),
            &w,
            "invbotb1",
            "pw-invented-3x",
        )
        .unwrap();
        assert_eq!(roster(&w, plain.user_id), vec!["invbotb1".to_owned()]);
    }

    #[test]
    fn signup_creates_a_player_with_one_character() {
        let (b, w) = (book(), world());
        let r = try_login(&b, &listener(true, Ceiling::Gm), &w, ANN, ANN_PW).unwrap();
        assert_eq!(r.role, Role::Player, "every new account is a player (O10)");
        let row = find(read(&w).deltas(), "invented ANN").expect("found ignoring case");
        assert_eq!((row.user_id, row.role), (r.user_id, Role::Player));
        assert_eq!(roster(&w, r.user_id), vec![ANN.to_owned()]);
        // The same name again, any case, is that account, not a second one.
        let again =
            try_login(&b, &listener(true, Ceiling::Gm), &w, "INVENTED ANN", ANN_PW).unwrap();
        assert_eq!(again.user_id, r.user_id);
        assert_eq!(rows(read(&w).deltas()).count(), 1);
        // A second account gets its own id and sees only its own character.
        let ben = try_login(
            &b,
            &listener(true, Ceiling::Gm),
            &w,
            "Invented Ben",
            "acc2-invented-pass-2",
        )
        .unwrap();
        assert_ne!(ben.user_id, r.user_id);
        assert_eq!(roster(&w, ben.user_id), vec!["Invented Ben".to_owned()]);
    }

    #[test]
    fn a_wrong_password_is_refused_through_the_constant_time_compare() {
        let (b, w) = (book(), world());
        let l = listener(true, Ceiling::Gm);
        try_login(&b, &l, &w, ANN, ANN_PW).unwrap();
        let before = crate::accounts::CT_EQ_CALLS.with(|n| n.get());
        let err = try_login(&b, &l, &w, ANN, "acc2-not-the-password").unwrap_err();
        assert!(matches!(err, Refused::WrongPassword { .. }), "{err:?}");
        assert!(!err.to_string().contains("not-the-password"), "{err}");
        assert!(crate::accounts::CT_EQ_CALLS.with(|n| n.get()) > before);
        let PyValue::ObjectEx { header, .. } = refusal(&err) else {
            panic!("an exception object")
        };
        let PyValue::Tuple(h) = *header else { panic!() };
        assert_eq!(
            crate::pyutil::dict_get(&h[2], b"reasonCode"),
            Some(&crate::pyutil::bstr("LoginAuthFailed"))
        );
    }

    #[test]
    fn with_signup_off_an_unknown_user_is_refused() {
        let (b, w) = (book(), world());
        let err = try_login(&b, &listener(false, Ceiling::Player), &w, ANN, ANN_PW).unwrap_err();
        assert_eq!(err, Refused::NoSuchUser);
        assert_eq!(rows(read(&w).deltas()).count(), 0);
        assert!(read(&w).config().characters.is_empty());
        // Not a USERNAME:PASSWORD token at all.
        assert_eq!(
            login(&b, &listener(true, Ceiling::Gm), None, Some(b"offline"), &w),
            Err(Refused::NotUserPassword)
        );
        // A sign-up that breaks the rules makes nothing.
        for (name, pw) in [
            ("ab", ANN_PW),
            (ANN, "short"),
            (ANN, ANN),
            ("bad\tname", ANN_PW),
        ] {
            let err = try_login(&b, &listener(true, Ceiling::Gm), &w, name, pw).unwrap_err();
            assert!(
                matches!(err, Refused::SignupRefused(_)),
                "{name:?}: {err:?}"
            );
        }
        assert_eq!(rows(read(&w).deltas()).count(), 0);
    }

    #[test]
    fn grant_changes_the_role_and_the_ceiling_still_caps_it() {
        let (b, w) = (book(), world());
        let gm_listener = listener(true, Ceiling::Gm);
        try_login(&b, &gm_listener, &w, ANN, ANN_PW).unwrap();
        let said = grant(
            &mut crate::service::world_write(&w),
            None,
            &["invented", "ann", "gm"],
        )
        .unwrap();
        assert!(
            matches!(said, PyValue::Bytes(ref s) if s.starts_with(b"account")),
            "{said:?}"
        );
        assert_eq!(
            try_login(&b, &gm_listener, &w, ANN, ANN_PW).unwrap().role,
            Role::Gm
        );
        let friends = listener(false, Ceiling::Player);
        assert_eq!(
            try_login(&b, &friends, &w, ANN, ANN_PW).unwrap().role,
            Role::Player
        );
        // Back down, and a name that is no account.
        set_role(read_mut(&w).deltas_mut(), ANN, Role::Player).unwrap();
        assert_eq!(
            try_login(&b, &gm_listener, &w, ANN, ANN_PW).unwrap().role,
            Role::Player
        );
        assert!(set_role(read_mut(&w).deltas_mut(), "nobody here", Role::Gm).is_err());
    }

    fn read_mut(w: &RwLock<Registry>) -> std::sync::RwLockWriteGuard<'_, Registry> {
        crate::service::world_write(w)
    }

    #[test]
    fn failures_back_off_per_name() {
        let (b, w) = (book(), world());
        let l = listener(true, Ceiling::Gm);
        try_login(&b, &l, &w, ANN, ANN_PW).unwrap();
        let mut last = Ok(());
        for _ in 0..=FREE_FAILURES_PER_NAME {
            last = try_login(&b, &l, &w, ANN, "acc2-not-the-password").map(|_| ());
        }
        assert!(last.is_err());
        // Locked now: even the right password is refused unread.
        assert_eq!(try_login(&b, &l, &w, ANN, ANN_PW), Err(Refused::Backoff));
    }

    /// A name's backoff is per listener: failures on the friends' listener do
    /// not lock the same name out of the owner's.
    #[test]
    fn a_names_backoff_is_per_listener() {
        let (b, w) = (book(), world());
        let owner = Listener {
            addr: "127.0.0.1:1".parse().unwrap(),
            ..listener(true, Ceiling::Gm)
        };
        let friends = Listener {
            addr: "127.0.0.1:2".parse().unwrap(),
            ..listener(true, Ceiling::Player)
        };
        try_login(&b, &owner, &w, ANN, ANN_PW).unwrap();
        for _ in 0..=FREE_FAILURES_PER_NAME {
            let _ = try_login(&b, &friends, &w, ANN, "acc2-not-the-password");
        }
        assert_eq!(
            try_login(&b, &friends, &w, ANN, ANN_PW),
            Err(Refused::Backoff)
        );
        assert!(try_login(&b, &owner, &w, ANN, ANN_PW).is_ok());
    }

    /// F-HOST H4: behind the tunnel every friend is 127.0.0.1, so a loopback
    /// peer must not share one backoff; a real address still does.
    #[test]
    fn a_loopback_peer_has_no_shared_backoff_but_another_peer_has() {
        let (b, w) = (book(), world());
        let l = listener(false, Ceiling::Player); // unknown names fail
        let attempt = |peer: IpAddr, name: &str| {
            login(
                &b,
                &l,
                Some(peer),
                Some(&token(name, "pw-not-the-one-x")),
                &w,
            )
            .map(|_| ())
        };
        let lo: IpAddr = "127.0.0.1".parse().unwrap();
        for i in 0..=FREE_FAILURES_PER_PEER {
            let _ = attempt(lo, &format!("invfriend{i}"));
        }
        // A different name from the same loopback peer is still read.
        assert_ne!(
            login(
                &b,
                &l,
                Some(lo),
                Some(&token("invfresh", "pw-invented-3x")),
                &w
            )
            .map(|_| ()),
            Err(Refused::Backoff)
        );
        // A non-loopback peer (a documentation address) is still limited.
        let far: IpAddr = "192.0.2.7".parse().unwrap();
        for i in 0..=FREE_FAILURES_PER_PEER {
            let _ = attempt(far, &format!("invother{i}"));
        }
        assert_eq!(
            login(
                &b,
                &l,
                Some(far),
                Some(&token("invlate", "pw-invented-4x")),
                &w
            )
            .map(|_| ()),
            Err(Refused::Backoff)
        );
    }

    #[test]
    fn accounts_and_owners_survive_a_save_and_reload() {
        use crate::store::Store;
        let (b, w) = (book(), world());
        let ann = try_login(&b, &listener(true, Ceiling::Gm), &w, ANN, ANN_PW).unwrap();
        set_role(read_mut(&w).deltas_mut(), ANN, Role::Gm).unwrap();
        let dir = std::env::temp_dir().join(format!("eve-acc2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("world.txt");
        let file = crate::store::FileStore::new(path.clone());
        std::fs::create_dir_all(&dir).unwrap();
        file.save(&read(&w).to_snapshot()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains(ANN_PW),
            "the password reached the world file"
        );

        // The offline grant refuses while the lock is held, then works.
        let held = lock_world(&path).unwrap();
        assert!(
            grant_offline(&path, ANN, "player")
                .unwrap_err()
                .contains("locked")
        );
        drop(held);
        grant_offline(&path, ANN, "player").unwrap();

        let again = world();
        crate::service::world_write(&again)
            .apply_snapshot(crate::store::FileStore::new(path).load().unwrap().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
        let r = try_login(&b, &listener(false, Ceiling::Gm), &again, ANN, ANN_PW).unwrap();
        assert_eq!((r.user_id, r.role), (ann.user_id, Role::Player));
        assert_eq!(roster(&again, ann.user_id), vec![ANN.to_owned()]);
        assert!(roster(&again, ann.user_id + 1).is_empty());
        assert!(
            try_login(
                &b,
                &listener(false, Ceiling::Gm),
                &again,
                ANN,
                "acc2-not-the-password"
            )
            .is_err()
        );
    }

    /// BOTMARK: the mark is set offline only, survives a save and reload
    /// (and an in-game /grant), never raises the role, and makes every
    /// character the account owns a bot in `Character::is_bot`.
    #[test]
    fn the_bot_mark_is_offline_only_survives_reload_and_marks_the_characters() {
        use crate::store::Store;
        const BEN: &str = "Invented Ben";
        let (b, w) = (book(), world());
        let l = listener(true, Ceiling::Gm);
        let ann = try_login(&b, &l, &w, ANN, ANN_PW).unwrap();
        let ben = try_login(&b, &l, &w, BEN, "botmark-invented-pass-2").unwrap();
        let dir = std::env::temp_dir().join(format!("eve-botmark-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("world.txt");
        let file = crate::store::FileStore::new(path.clone());
        file.save(&read(&w).to_snapshot()).unwrap();

        // Refused under a running server's lock, then set offline.
        let held = lock_world(&path).unwrap();
        assert!(
            mark_bot_offline(&path, ANN, true)
                .unwrap_err()
                .contains("locked")
        );
        drop(held);
        mark_bot_offline(&path, "invented ann", true).unwrap();
        assert!(mark_bot_offline(&path, "nobody here", true).is_err());

        let again = world();
        crate::service::world_write(&again).apply_snapshot(file.load().unwrap().unwrap());
        let bots = |w: &RwLock<Registry>| -> Vec<(String, bool)> {
            read(w)
                .config()
                .characters
                .iter()
                .map(|c| (c.name.clone(), c.is_bot))
                .collect()
        };
        assert_eq!(
            bots(&again),
            vec![(ANN.to_owned(), true), (BEN.to_owned(), false)]
        );
        let row = find(read(&again).deltas(), ANN).unwrap();
        assert!(row.bot);
        // The mark never raises a role: still the player it signed up as.
        assert_eq!(row.role, Role::Player);
        assert_eq!(
            try_login(&b, &l, &again, ANN, ANN_PW).unwrap().role,
            Role::Player
        );
        assert!(!find(read(&again).deltas(), BEN).unwrap().bot);
        assert!(is_bot_account(read(&again).deltas(), ann.user_id));
        assert!(!is_bot_account(read(&again).deltas(), ben.user_id));

        // A character created later for the marked account is a bot too.
        let id = read_mut(&again).create_owned_character(ann.user_id, "Invented Annex".to_owned());
        assert!(read(&again).character(id).unwrap().is_bot);

        // No in-game path: no slash command names a bot (BOT-CTL's dev-only
        // `/bots` drives the bot process and never sets the mark), `/grant`
        // takes no `bot` role, and a /grant keeps the mark as it was.
        assert!(
            crate::slash::COMMANDS
                .iter()
                .all(|c| c.name == "bots" || !c.name.contains("bot"))
        );
        let mut reg = read_mut(&again);
        grant(&mut reg, None, &["Invented", "Ben", "bot"]).unwrap();
        assert!(!find(reg.deltas(), BEN).unwrap().bot);
        grant(&mut reg, None, &["Invented", "Ann", "gm"]).unwrap();
        let row = find(reg.deltas(), ANN).unwrap();
        assert_eq!((row.role, row.bot), (Role::Gm, true));
        drop(reg);

        // Cleared offline: the row is ACC2's seven fields again.
        file.save(&read(&again).to_snapshot()).unwrap();
        mark_bot_offline(&path, ANN, false).unwrap();
        let snap = file.load().unwrap().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(snap.deltas.of(KIND).iter().all(|(_, v)| v.len() == 7));
        assert!(rows(&snap.deltas).all(|r| !r.bot));
    }

    /// An ACC2 row (seven fields, no mark) still loads, unmarked, and an
    /// eighth field that is not the mark is not a row.
    #[test]
    fn an_old_account_row_loads_unmarked() {
        let key = vec!["1001".to_owned()];
        let old: Vec<String> = [
            "Invented Cat",
            "gm",
            "126966528000000000",
            SCHEME,
            "2",
            "00112233",
            &"ab".repeat(32),
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let row = Row::parse(&key, &old).expect("an ACC2 row");
        assert_eq!((row.role, row.bot), (Role::Gm, false));
        assert_eq!(
            row.value(),
            old,
            "an unmarked row is written as ACC2 wrote it"
        );
        let mut marked = old.clone();
        marked.push(BOT_FIELD.to_owned());
        let row = Row::parse(&key, &marked).expect("a marked row");
        assert_eq!((row.role, row.bot), (Role::Gm, true));
        assert_eq!(row.value(), marked);
        let mut odd = old;
        odd.push("something-else".to_owned());
        assert!(Row::parse(&key, &odd).is_none());
    }

    /// A human sign-up is never hashed at the bot-spawn count: the book floors
    /// at `MIN_HUMAN_ITERATIONS` even when asked for one, and the row a
    /// sign-up writes keeps that count.
    #[test]
    fn a_human_account_row_is_never_made_with_one_iteration() {
        const { assert!(MIN_HUMAN_ITERATIONS > BOT_SPAWN_ITERATIONS) };
        assert!(Book::with_iterations(1).iterations > BOT_SPAWN_ITERATIONS);
        assert!(Book::default().iterations > BOT_SPAWN_ITERATIONS);
        let w = world();
        let b = Book::with_iterations(1);
        let row = sign_up(&b, "Invented Hal", "acc2-invented-pass-9", &w, false).expect("sign-up");
        assert!(row.iterations > BOT_SPAWN_ITERATIONS);
        assert!(row.verify(b"acc2-invented-pass-9"));
    }

    /// A batch gives consecutive user ids, each password verifies at one
    /// iteration (stored per row), a re-run keeps ids, and a human's name is
    /// still refused with nothing written.
    #[test]
    fn a_bot_batch_numbers_consecutively_and_verifies_at_one_iteration() {
        let mut d = Deltas::default();
        let bots: Vec<(String, String, Vec<u8>)> = (1u8..=3)
            .map(|i| {
                (
                    format!("Invbot{i}"),
                    format!("pw-{i:02}-invented"),
                    vec![i; 16],
                )
            })
            .collect();
        let args: Vec<(&str, &str, Vec<u8>)> = bots
            .iter()
            .map(|(n, p, s)| (n.as_str(), p.as_str(), s.clone()))
            .collect();
        let made = ensure_bot_accounts(&mut d, &args, BOT_SPAWN_ITERATIONS).expect("batch");
        let ids: Vec<i64> = made.iter().map(|r| r.user_id).collect();
        assert_eq!(ids, vec![ids[0], ids[0] + 1, ids[0] + 2]);
        assert!(ids[0] >= crate::accounts::FIRST_USER_ID);
        for (r, (_, pw, _)) in made.iter().zip(&bots) {
            assert_eq!(r.iterations, BOT_SPAWN_ITERATIONS);
            assert!(r.bot && r.role == Role::Gm);
            let stored = find(&d, &r.username).expect("stored");
            assert!(stored.verify(pw.as_bytes()));
            assert!(!stored.verify(b"wrong-invented"));
        }
        let again = ensure_bot_accounts(&mut d, &args, BOT_SPAWN_ITERATIONS).expect("again");
        assert_eq!(
            again.iter().map(|r| r.user_id).collect::<Vec<_>>(),
            ids,
            "a re-run keeps each bot's id"
        );
        // A human's name in the batch refuses it whole.
        let mut d2 = Deltas::default();
        let human = Row::new(5_000, "Invbot2", b"invented-human-pw", 2, vec![9; 16]);
        d2.put(KIND, human.key(), human.value());
        let before = rows(&d2).count();
        assert!(ensure_bot_accounts(&mut d2, &args, BOT_SPAWN_ITERATIONS).is_err());
        assert_eq!(rows(&d2).count(), before, "nothing written");
    }
}
