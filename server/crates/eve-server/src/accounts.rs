//! Accounts: who a connection is, and what it may do (packet IP27-a).
//!
//! `docs/lanes/account-spec.md` §3-§5, as amended by IP40, under
//! `docs/review/rulings.md` rulings 1, 2, 8 and 12.
//!
//! **Three modes, never mixed** (§4.1). Without `--accounts` there is one
//! catch-all `gm` account whose id is `--user-id`: behaviour-identical to
//! the server before accounts, and `server::check_listener` keeps that
//! account off every listener a friend could reach. With `--accounts FILE`
//! there is the file and nothing else: an unknown token is refused, and no
//! catch-all exists to fall back on. With `--world-accounts` (ACC2, owner
//! decision O9) the accounts are rows of the world file, made by signing
//! up: `crate::account_book`. The file mode stays as the legacy path.
//!
//! **The file holds hashes, never tokens** (§3.1), one account per line,
//! tab-separated, `#` starting a comment:
//!
//! ```text
//! sha256(token)  user_id  role  name  [start_character]
//! ```
//!
//! `role` is data, not code: who is a GM (owner decision O10) is answered
//! in the file, line by line, and `--new-token` takes it as a required
//! argument, so no default role for an account is written anywhere in this
//! module. The catch-all is `gm` because that *is* today's behaviour.
//!
//! A session's role is `min(account role, listener ceiling)` (§4.1 rule 3):
//! a leaked GM token on a `ceiling=player` listener is a player.
//!
//! ```
//! use eve_server::accounts::{Accounts, Role, hex, sha256};
//! use eve_server::server::{Ceiling, Listener};
//! let line = format!("{}\t1001\tgm\towner\n", hex(&sha256(b"not-a-real-token")));
//! let accounts = Accounts::parse(&line).unwrap();
//! let friends = Listener {
//!     ceiling: Ceiling::Player,
//!     ..Listener::new("127.0.0.1:0".parse().unwrap())
//! };
//! let r = accounts.resolve(&friends, Some(b"not-a-real-token")).unwrap();
//! assert_eq!((r.user_id, r.role), (1001, Role::Player));
//! ```

use crate::roles;
use crate::server::{Ceiling, Listener};
use std::collections::HashMap;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;

/// The `user_id` `--new-token` hands out when no `--accounts FILE` says
/// what is taken. [A] clear of the catch-all's default `--user-id 1`;
/// falsified by nothing a player sees, since a `user_id` is never shown.
pub const FIRST_USER_ID: i64 = 1001;

/// Tokens our own tools and tests send (`account-spec.md` §3.1, [V] grep):
/// `run-client.sh`'s default `offline`, the bots' `dev`, the tests' `token`
/// and `a-token`, the morning probe's `probe-123`, and the empty string.
/// No account may be reachable with one of them.
pub const TOOL_TOKENS: &[&str] = &["", "offline", "dev", "token", "a-token", "probe-123"];

/// What a session may do. `Player < Gm`, so `min` is the cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    Player,
    Gm,
}

impl Role {
    /// The session role mask (`crate::roles`).
    ///
    /// `Gm` is [`roles::SANDBOX_DOCKED`], the mask every session had before
    /// accounts. `Player` is [`roles::PLAYER_ONLY`]: [A] that a client
    /// holding only `ROLE_LOGIN | ROLE_PLAYER` undocks, mines and fights
    /// here; falsified by the first friend session that cannot.
    pub fn mask(self) -> i64 {
        match self {
            Role::Player => roles::PLAYER_ONLY,
            Role::Gm => roles::SANDBOX_DOCKED,
        }
    }

    /// The highest role a listener with this ceiling grants.
    pub fn of_ceiling(c: Ceiling) -> Role {
        match c {
            Ceiling::Player => Role::Player,
            Ceiling::Gm => Role::Gm,
        }
    }

    /// `min(self, ceiling)`.
    pub fn capped(self, c: Ceiling) -> Role {
        self.min(Role::of_ceiling(c))
    }

    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "player" => Some(Role::Player),
            "gm" => Some(Role::Gm),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Role::Player => "player",
            Role::Gm => "gm",
        }
    }
}

/// One line of the accounts file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// `session.userid`. Never a character id, never a connection id.
    pub user_id: i64,
    pub role: Role,
    /// For logs only. Never a key and never a token (ruling 2).
    pub name: String,
    /// The character this account gets (§3.4): handed to the world by
    /// [`Accounts::configure`], created or adopted there once, at startup,
    /// by `Registry::provision_start_characters` (IP27-b).
    pub start_character: Option<String>,
    /// 1-based line in the file, so an error can name the line rather than
    /// the token.
    pub line: usize,
}

/// Who a connection is, after [`Accounts::resolve`]. The role is already
/// capped by the listener's ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub user_id: i64,
    pub role: Role,
}

/// Why [`Accounts::resolve`] refused a connection. Never carries a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// No token, or an empty one.
    NoToken,
    /// World accounts: the token is not `USERNAME:PASSWORD`.
    NotUserPassword,
    /// World accounts: no account has this name, and this listener does not
    /// sign up (`signup=off`). Shown to the client as a wrong password.
    NoSuchUser,
    /// World accounts: the password does not match. Never carries it.
    WrongPassword { user_id: i64 },
    /// World accounts: a sign-up refused, with a reason the player may read
    /// (the name's or the password's rules). Never carries the password.
    SignupRefused(String),
    /// World accounts: too many failures for this name or this peer.
    Backoff,
    /// `sha256(token)` is in no line. `prefix` is the first 8 hex digits of
    /// the hash, for the log: enough to match a line, useless as a token.
    UnknownToken { prefix: String },
    /// The listener's `account=` names no account. `server::check_listener`
    /// refuses that at startup, so a running server never sees it; it
    /// exists so `resolve` needs no panic.
    NoSuchAccount(i64),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::NoToken => f.write_str("no token"),
            Refused::UnknownToken { prefix } => {
                write!(f, "a token whose sha256 starts {prefix} is in no account")
            }
            Refused::NoSuchAccount(uid) => write!(f, "account={uid} names no account"),
            Refused::NotUserPassword => f.write_str("the token is not USERNAME:PASSWORD"),
            Refused::NoSuchUser => {
                f.write_str("no such account, and this listener does not sign up")
            }
            Refused::WrongPassword { user_id } => write!(f, "wrong password for account {user_id}"),
            Refused::SignupRefused(why) => write!(f, "sign-up refused: {why}"),
            Refused::Backoff => f.write_str("too many failed logins; backing off"),
        }
    }
}

/// The accounts file, the catch-all, or the world's accounts; never two
/// of them (§4.1).
#[derive(Clone)]
pub struct Accounts {
    list: Vec<Account>,
    by_hash: HashMap<[u8; 32], usize>,
    /// `Some(user_id)` without `--accounts`.
    catch_all: Option<i64>,
    /// `Some` with `--world-accounts`: the accounts are world rows, and this
    /// is the in-memory half (the hash cost and the failure backoff).
    book: Option<Arc<crate::account_book::Book>>,
}

/// Hashes are not secrets, but nothing gains from printing them either.
impl std::fmt::Debug for Accounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.catch_all, &self.book) {
            (Some(uid), _) => write!(f, "Accounts(catch-all {uid})"),
            (None, Some(_)) => f.write_str("Accounts(in the world)"),
            (None, None) => write!(f, "Accounts({} accounts)", self.list.len()),
        }
    }
}

impl Accounts {
    /// One `gm` account, `user_id`, for every token: the server without
    /// `--accounts`.
    pub fn catch_all(user_id: i64) -> Accounts {
        Accounts {
            list: Vec::new(),
            by_hash: HashMap::new(),
            catch_all: Some(user_id),
            book: None,
        }
    }

    /// `--world-accounts`: every account is a world row
    /// (`crate::account_book`), made by signing up on a `signup=on`
    /// listener. No file, no catch-all.
    pub fn in_world(book: crate::account_book::Book) -> Accounts {
        Accounts {
            list: Vec::new(),
            by_hash: HashMap::new(),
            catch_all: None,
            book: Some(Arc::new(book)),
        }
    }

    pub fn is_catch_all(&self) -> bool {
        self.catch_all.is_some()
    }

    /// The world accounts' book, `None` in the other two modes.
    pub fn book(&self) -> Option<&crate::account_book::Book> {
        self.book.as_deref()
    }

    /// Tell the world which mode it is in (IP27-b, account-spec §7 step 2):
    /// `catch_all` is `Some(user_id)` for the catch-all and `None` for a
    /// file, and `start_characters` is each account's `start_character`, in
    /// file order. `Server::bind` calls this once, before the world is
    /// built, so the world's mode cannot disagree with the handshake's.
    pub fn configure(&self, sandbox: &mut crate::model::SandboxConfig) {
        sandbox.catch_all = self.catch_all;
        sandbox.start_characters = self
            .list
            .iter()
            .filter_map(|a| Some((a.user_id, a.start_character.clone()?)))
            .collect();
    }

    /// Every account, in file order. Empty for the catch-all.
    pub fn list(&self) -> &[Account] {
        &self.list
    }

    /// The account with this `user_id`. The catch-all has none: it is not
    /// a line, and `account=` naming it is `server`'s business.
    pub fn by_user(&self, user_id: i64) -> Option<&Account> {
        self.list.iter().find(|a| a.user_id == user_id)
    }

    /// Read `--accounts FILE` (§3.1).
    ///
    /// Refused when the file is inside a git work tree (any ancestor of its
    /// canonical path holds `.git`) [A: that catches every copy of a repo;
    /// falsified by a checkout whose `.git` lives elsewhere], when it is
    /// group- or world-writable (whoever can write it can make a GM), and
    /// whenever [`Accounts::parse`] refuses it.
    pub fn load(path: &Path) -> Result<Accounts, String> {
        let shown = path.display();
        let abs = path
            .canonicalize()
            .map_err(|e| format!("--accounts {shown}: {e}"))?;
        if let Some(repo) = inside_a_repo(&abs) {
            return Err(format!(
                "--accounts {shown}: inside the git work tree {}; keep the accounts \
                 file outside every repo (~/.config/eve-offline/accounts.tsv)",
                repo.display()
            ));
        }
        let mut file = std::fs::File::open(&abs).map_err(|e| format!("--accounts {shown}: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = file
                .metadata()
                .map_err(|e| format!("--accounts {shown}: {e}"))?
                .permissions()
                .mode();
            if mode & 0o022 != 0 {
                return Err(format!(
                    "--accounts {shown}: mode {:o} lets others write it, and whoever \
                     writes it can make a GM; chmod go-w it",
                    mode & 0o777
                ));
            }
        }
        let mut text = String::new();
        file.read_to_string(&mut text)
            .map_err(|e| format!("--accounts {shown}: {e}"))?;
        Accounts::parse(&text).map_err(|e| format!("--accounts {shown}: {e}"))
    }

    /// Parse the file's text. Refused: a field count other than four or
    /// five; a hash that is not 64 lowercase hex; a `user_id` that is not
    /// positive; a role other than `player` or `gm`; an empty name; any
    /// repeated hash, `user_id`, name or `start_character` (§4.2); and a
    /// file with no account at all, which would refuse every login.
    pub fn parse(text: &str) -> Result<Accounts, String> {
        let mut list: Vec<Account> = Vec::new();
        let mut by_hash = HashMap::new();
        for (i, raw) in text.lines().enumerate() {
            let n = i + 1;
            let line = raw.trim_end_matches('\r');
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            if !(4..=5).contains(&fields.len()) {
                return Err(format!(
                    "line {n}: {} tab-separated fields, expected \
                     sha256 user_id role name [start_character]",
                    fields.len()
                ));
            }
            let hash = parse_hash(fields[0])
                .ok_or_else(|| format!("line {n}: the hash is not 64 lowercase hex digits"))?;
            let user_id: i64 = fields[1]
                .parse()
                .ok()
                .filter(|u| *u > 0)
                .ok_or_else(|| format!("line {n}: user_id {} is not positive", fields[1]))?;
            let role = Role::parse(fields[2])
                .ok_or_else(|| format!("line {n}: role {} is not player or gm", fields[2]))?;
            let name = fields[3].trim();
            if name.is_empty() {
                return Err(format!("line {n}: the name is empty"));
            }
            let start_character = fields
                .get(4)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            for a in &list {
                let clash = if a.user_id == user_id {
                    Some(format!("user_id {user_id}"))
                } else if a.name == name {
                    Some(format!("name {name}"))
                } else if start_character.is_some() && a.start_character == start_character {
                    Some("start_character".to_owned())
                } else {
                    None
                };
                if let Some(what) = clash {
                    return Err(format!("line {n}: {what} repeats line {}", a.line));
                }
            }
            if let Some(&j) = by_hash.get(&hash) {
                let first: &Account = &list[j];
                return Err(format!(
                    "line {n}: the same token hash as line {}; two accounts cannot share a token",
                    first.line
                ));
            }
            by_hash.insert(hash, list.len());
            list.push(Account {
                user_id,
                role,
                name: name.to_owned(),
                start_character,
                line: n,
            });
        }
        if list.is_empty() {
            return Err("no accounts: every login would be refused".to_owned());
        }
        Ok(Accounts {
            list,
            by_hash,
            catch_all: None,
            book: None,
        })
    }

    /// **No token is a visible name** (ruling 2). Run once the world's
    /// character names are known (`Server::bind`): `sha256` of every name
    /// given, every account name and `start_character`, and every
    /// [`TOOL_TOKENS`] entry must match no line. The error names the
    /// account and its line, never the token (which is the name).
    pub fn check_names<'a>(&self, names: impl Iterator<Item = &'a str>) -> Result<(), String> {
        if self.is_catch_all() {
            return Ok(());
        }
        let hit = |candidate: &str| -> Result<(), String> {
            match self.by_hash.get(&sha256(candidate.as_bytes())) {
                None => Ok(()),
                Some(&i) => Err(format!(
                    "--accounts line {} (account {}): its token is a character name, an \
                     account name or a token our tools send; give it a random one with \
                     eve-server --new-token",
                    self.list[i].line, self.list[i].name
                )),
            }
        };
        names.chain(TOOL_TOKENS.iter().copied()).try_for_each(hit)?;
        for a in &self.list {
            hit(&a.name)?;
            if let Some(s) = &a.start_character {
                hit(s)?;
            }
        }
        Ok(())
    }

    /// Who a connection is (§4.1), in this order:
    ///
    /// 1. the listener's `account=`, whatever the token says (identity by
    ///    listener, the fallback if `/ssoToken` does not survive the
    ///    launcher [P]);
    /// 2. else `sha256(token)`; no token or an unknown one is refused;
    /// 3. the role capped by the listener's ceiling.
    ///
    /// The catch-all answers its own id for everything, capped likewise.
    /// World accounts resolve in `crate::account_book::login`, which needs
    /// the world; here they refuse everything.
    pub fn resolve(&self, listener: &Listener, token: Option<&[u8]>) -> Result<Resolved, Refused> {
        if self.book.is_some() {
            return Err(Refused::NoToken);
        }
        let (user_id, role) = match (self.catch_all, listener.account) {
            (Some(uid), _) => (uid, Role::Gm),
            (None, Some(uid)) => {
                let a = self.by_user(uid).ok_or(Refused::NoSuchAccount(uid))?;
                (a.user_id, a.role)
            }
            (None, None) => {
                let token = token.filter(|t| !t.is_empty()).ok_or(Refused::NoToken)?;
                let hash = sha256(token);
                let i = *self
                    .by_hash
                    .get(&hash)
                    .ok_or_else(|| Refused::UnknownToken {
                        prefix: hex(&hash[..4]),
                    })?;
                (self.list[i].user_id, self.list[i].role)
            }
        };
        Ok(Resolved {
            user_id,
            role: role.capped(listener.ceiling),
        })
    }

    /// The `user_id` `--new-token` hands out: one past the largest here,
    /// and never below [`FIRST_USER_ID`].
    pub fn next_user_id(&self) -> i64 {
        self.list
            .iter()
            .map(|a| a.user_id.saturating_add(1))
            .max()
            .unwrap_or(FIRST_USER_ID)
            .max(FIRST_USER_ID)
    }
}

/// What `eve-server --new-token` prints (§3.1): three lines and nothing
/// else. It never writes a file, so no tool of ours chooses where a secret
/// lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewToken {
    /// The `accounts.tsv` line, hash and all.
    pub accounts_line: String,
    /// The token, 64 hex digits, for the player's `/ssoToken=`.
    pub token: String,
    /// `NAME<TAB>TOKEN`, for a bots' `--tokens FILE` (IP41).
    pub bots_line: String,
}

/// Build a new account from 32 random bytes. `existing` is `--accounts`
/// when also given: it supplies the next `user_id` and refuses a name or a
/// `start_character` already used.
pub fn new_token(
    random: &[u8; 32],
    name: &str,
    role: &str,
    start_character: Option<&str>,
    existing: Option<&Accounts>,
) -> Result<NewToken, String> {
    let role = Role::parse(role).ok_or_else(|| format!("role {role}: expected player or gm"))?;
    let bad = |s: &str| s.trim().is_empty() || s.contains(['\t', '\n', '\r']);
    if bad(name) {
        return Err("NAME must be non-empty, without tabs or newlines".to_owned());
    }
    if start_character.is_some_and(bad) {
        return Err("START_CHARACTER must be non-empty, without tabs or newlines".to_owned());
    }
    if let Some(a) = existing {
        if a.list.iter().any(|a| a.name == name) {
            return Err(format!("an account named {name} exists"));
        }
        if start_character.is_some()
            && a.list
                .iter()
                .any(|a| a.start_character.as_deref() == start_character)
        {
            return Err("that start_character belongs to another account".to_owned());
        }
    }
    let user_id = existing.map_or(FIRST_USER_ID, Accounts::next_user_id);
    let token = hex(random);
    let mut accounts_line = format!(
        "{}\t{user_id}\t{}\t{name}",
        hex(&sha256(token.as_bytes())),
        role.name()
    );
    if let Some(s) = start_character {
        accounts_line.push('\t');
        accounts_line.push_str(s);
    }
    Ok(NewToken {
        accounts_line,
        bots_line: format!("{name}\t{token}"),
        token,
    })
}

/// 32 bytes from `/dev/urandom`, for [`new_token`].
pub fn urandom32() -> io::Result<[u8; 32]> {
    let mut out = [0u8; 32];
    urandom_fill(&mut out)?;
    Ok(out)
}

/// Fill `buf` from `/dev/urandom`: one open, one read.
pub fn urandom_fill(buf: &mut [u8]) -> io::Result<()> {
    std::fs::File::open("/dev/urandom")?.read_exact(buf)
}

/// The nearest ancestor of `path` that holds `.git`, if any.
fn inside_a_repo(path: &Path) -> Option<&Path> {
    path.ancestors().find(|a| a.join(".git").exists())
}

fn parse_hash(s: &str) -> Option<[u8; 32]> {
    let b = s.as_bytes();
    if b.len() != 64 || !b.iter().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let nib = |c: u8| {
        if c.is_ascii_digit() {
            c - b'0'
        } else {
            c - b'a' + 10
        }
    };
    let mut out = [0u8; 32];
    for (o, pair) in out.iter_mut().zip(b.chunks_exact(2)) {
        *o = (nib(pair[0]) << 4) | nib(pair[1]);
    }
    Some(out)
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// SHA-256's round constants (FIPS 180-4 §4.2.2).
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256's initial hash value (FIPS 180-4 §5.3.3).
const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// One block of the compression function (FIPS 180-4 §6.2.2).
fn compress(h: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 64];
    for (t, word) in block.chunks_exact(4).enumerate() {
        w[t] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
    }
    for t in 16..64 {
        let s0 = w[t - 15].rotate_right(7) ^ w[t - 15].rotate_right(18) ^ (w[t - 15] >> 3);
        let s1 = w[t - 2].rotate_right(17) ^ w[t - 2].rotate_right(19) ^ (w[t - 2] >> 10);
        w[t] = w[t - 16]
            .wrapping_add(s0)
            .wrapping_add(w[t - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for (k, wt) in K.iter().zip(w) {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(*k)
            .wrapping_add(wt);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
        *x = x.wrapping_add(y);
    }
}

/// Finish a hash whose state `h` has already absorbed `done` bytes, all in
/// whole blocks, with `data`: the padding of §5.1.1 is 0x80, zeros to 56
/// mod 64, then the total bit length big-endian.
fn finish(mut h: [u32; 8], done: u64, data: &[u8]) -> [u8; 32] {
    let mut blocks = data.chunks_exact(64);
    for block in &mut blocks {
        compress(&mut h, block);
    }
    let rest = blocks.remainder();
    let mut tail = [0u8; 128];
    tail[..rest.len()].copy_from_slice(rest);
    tail[rest.len()] = 0x80;
    let n = if rest.len() < 56 { 64 } else { 128 };
    let bits = done.wrapping_add(data.len() as u64).wrapping_mul(8);
    tail[n - 8..n].copy_from_slice(&bits.to_be_bytes());
    for block in tail[..n].chunks_exact(64) {
        compress(&mut h, block);
    }
    let mut out = [0u8; 32];
    for (chunk, word) in out.chunks_exact_mut(4).zip(h) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// SHA-256 (FIPS 180-4 §6.2), std-only: the workspace takes no dependency
/// for ~60 lines (`server/deps.allow` stays empty).
pub fn sha256(data: &[u8]) -> [u8; 32] {
    finish(H0, 0, data)
}

/// HMAC-SHA256 (RFC 2104) with its two pad blocks absorbed once, so each
/// MAC of a short message is two compressions: what makes PBKDF2's
/// iterations cheap enough to run std-only (ACC2).
#[derive(Clone)]
pub struct HmacSha256 {
    inner: [u32; 8],
    outer: [u32; 8],
}

impl HmacSha256 {
    pub fn new(key: &[u8]) -> HmacSha256 {
        let mut k = [0u8; 64];
        if key.len() > 64 {
            k[..32].copy_from_slice(&sha256(key));
        } else {
            k[..key.len()].copy_from_slice(key);
        }
        let pad = |byte: u8| {
            let mut h = H0;
            let block: Vec<u8> = k.iter().map(|b| b ^ byte).collect();
            compress(&mut h, &block);
            h
        };
        HmacSha256 {
            inner: pad(0x36),
            outer: pad(0x5c),
        }
    }

    pub fn mac(&self, msg: &[u8]) -> [u8; 32] {
        finish(self.outer, 64, &finish(self.inner, 64, msg))
    }
}

/// PBKDF2 (RFC 8018 §5.2) with HMAC-SHA256 as the PRF, filling `out`.
pub fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {
    let prf = HmacSha256::new(password);
    for (i, chunk) in out.chunks_mut(32).enumerate() {
        let mut first = salt.to_vec();
        first.extend_from_slice(&(i as u32 + 1).to_be_bytes());
        let mut u = prf.mac(&first);
        let mut t = u;
        for _ in 1..iterations {
            u = prf.mac(&u);
            for (x, y) in t.iter_mut().zip(u) {
                *x ^= y;
            }
        }
        chunk.copy_from_slice(&t[..chunk.len()]);
    }
}

#[cfg(test)]
thread_local! {
    /// How many times [`ct_eq`] ran on this thread: how a test sees that a
    /// password check went through it.
    pub(crate) static CT_EQ_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Equal, in time that depends on the lengths only: every byte is read
/// whatever the first difference, so a password check leaks nothing about
/// how much of a guess was right.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    #[cfg(test)]
    CT_EQ_CALLS.with(|n| n.set(n.get() + 1));
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y));
    std::hint::black_box(diff) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An invented token's line. Never a real token.
    fn line(token: &str, uid: i64, role: &str, name: &str) -> String {
        format!(
            "{}\t{uid}\t{role}\t{name}\n",
            hex(&sha256(token.as_bytes()))
        )
    }

    fn listener() -> Listener {
        Listener::new("127.0.0.1:0".parse().unwrap())
    }

    // --- IP27-b: character ownership ------------------------------------

    use crate::model::{Character, SandboxConfig};
    use crate::service::{Caller, Registry, Request};
    use eve_marshal::PyValue;

    /// Two invented tokens, each an account with an invented
    /// `start_character`.
    const ANN: (&str, i64, &str) = ("ip27b-invented-token-ann", 3001, "Invented Ann");
    const BEN: (&str, i64, &str) = ("ip27b-invented-token-ben", 3002, "Invented Ben");

    fn two_accounts() -> Accounts {
        let with_start = |(token, uid, start): (&str, i64, &str)| {
            format!(
                "{}\t{uid}\tplayer\tacct{uid}\t{start}\n",
                hex(&sha256(token.as_bytes()))
            )
        };
        Accounts::parse(&(with_start(ANN) + &with_start(BEN))).unwrap()
    }

    /// A world in `accounts`' mode, seeded with `characters`, provisioned.
    fn world(accounts: &Accounts, characters: Vec<Character>) -> Registry {
        let mut config = SandboxConfig {
            characters,
            ..SandboxConfig::default()
        };
        accounts.configure(&mut config);
        let mut reg = Registry::new(config);
        reg.provision_start_characters().unwrap();
        reg
    }

    fn caller(accounts: &Accounts, token: &str) -> Caller {
        let r = accounts
            .resolve(&listener(), Some(token.as_bytes()))
            .unwrap();
        Caller::Session {
            user_id: r.user_id,
            role: r.role.mask(),
        }
    }

    /// The names on the select screen `caller` is shown.
    fn roster(reg: &mut Registry, caller: Caller) -> Vec<String> {
        let req = Request {
            service: "charUnboundMgr",
            method: "GetCharacterSelectionData",
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: None,
        };
        let PyValue::Tuple(four) = reg.dispatch_as(caller, &req).unwrap() else {
            panic!("selection data")
        };
        let PyValue::List(rows) = &four[2] else {
            panic!("characterDetails")
        };
        rows.iter()
            .map(|row| {
                let PyValue::Instance { state, .. } = row else {
                    panic!("row")
                };
                let id = match crate::pyutil::dict_get(state, b"characterID") {
                    Some(PyValue::Int(id)) => *id,
                    other => panic!("characterID {other:?}"),
                };
                reg.character(id)
                    .expect("a row is a character")
                    .name
                    .clone()
            })
            .collect()
    }

    fn named<'r>(reg: &'r Registry, name: &str) -> &'r Character {
        reg.config()
            .characters
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("no character {name}"))
    }

    #[test]
    fn two_tokens_get_two_rosters() {
        let accounts = two_accounts();
        let mut reg = world(&accounts, Vec::new());
        let ann = caller(&accounts, ANN.0);
        let ben = caller(&accounts, BEN.0);
        assert_eq!(roster(&mut reg, ann), vec![ANN.2.to_owned()]);
        assert_eq!(roster(&mut reg, ben), vec![BEN.2.to_owned()]);
        assert_eq!(
            roster(&mut reg, Caller::Internal).len(),
            2,
            "a server-made call sees everyone"
        );

        // Without --accounts, nothing is filtered: today's server.
        let mut open = world(&Accounts::catch_all(1), vec![Character::default()]);
        let everyone = open.config().characters.len();
        let who = Caller::Session {
            user_id: 1,
            role: Role::Gm.mask(),
        };
        assert_eq!(roster(&mut open, who).len(), everyone);
        assert!(everyone > 0);
    }

    #[test]
    fn a_start_character_is_created_once() {
        let accounts = two_accounts();
        let mut reg = world(&accounts, Vec::new());
        let before: Vec<(i64, i64)> = reg
            .config()
            .characters
            .iter()
            .map(|c| (c.character_id, c.ship_id))
            .collect();
        assert_eq!(before.len(), 2);
        let ann = named(&reg, ANN.2);
        assert_eq!(ann.owner_user_id, Some(ANN.1));
        assert!(
            reg.store().get(ann.ship_id).is_some(),
            "the capsule is an item"
        );
        assert_ne!(before[0].0, before[1].0);
        assert_ne!(before[0].1, before[1].1);

        // Provisioning again finds both by name and makes nothing.
        reg.provision_start_characters().unwrap();
        let after: Vec<(i64, i64)> = reg
            .config()
            .characters
            .iter()
            .map(|c| (c.character_id, c.ship_id))
            .collect();
        assert_eq!(after, before);

        // An unowned character of that name is adopted, not duplicated.
        let unowned = Character {
            name: ANN.2.to_owned(),
            ..Character::default()
        };
        let adopted = world(&accounts, vec![unowned]);
        assert_eq!(named(&adopted, ANN.2).owner_user_id, Some(ANN.1));
        assert_eq!(adopted.config().characters.len(), 2);

        // One owned by another account refuses startup.
        let mut config = SandboxConfig {
            characters: vec![Character {
                name: ANN.2.to_owned(),
                owner_user_id: Some(BEN.1),
                ..Character::default()
            }],
            ..SandboxConfig::default()
        };
        accounts.configure(&mut config);
        let err = Registry::new(config)
            .provision_start_characters()
            .unwrap_err();
        assert!(
            err.contains(&ANN.1.to_string()) && err.contains(&BEN.1.to_string()),
            "{err}"
        );
    }

    #[test]
    fn a_restart_keeps_each_character_on_its_account() {
        use crate::store::Store;
        let accounts = two_accounts();
        let reg = world(&accounts, Vec::new());
        let dir = std::env::temp_dir().join(format!("eve-ip27b-restart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("world.dat");
        let file = crate::store::FileStore::new(path.clone());
        file.save(&reg.to_snapshot()).unwrap();

        // The next start: no --character, so the roster is the world's.
        let mut config = SandboxConfig {
            characters: Vec::new(),
            ..SandboxConfig::default()
        };
        accounts.configure(&mut config);
        let mut again = Registry::new(config);
        again.apply_snapshot(crate::store::FileStore::new(path).load().unwrap().unwrap());
        again.provision_start_characters().unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        for (token, uid, name) in [ANN, BEN] {
            let was = named(&reg, name);
            let now = named(&again, name);
            assert_eq!(
                (now.character_id, now.owner_user_id),
                (was.character_id, Some(uid))
            );
            assert_eq!(
                roster(&mut again, caller(&accounts, token)),
                vec![name.to_owned()]
            );
        }
        assert_eq!(
            again.config().characters.len(),
            2,
            "created once, not twice"
        );
    }

    /// `SelectCharacterID` answers only for a character the caller may
    /// play (§4.6); `dispatch.rs` answers nothing on `None`.
    #[test]
    fn another_accounts_character_cannot_be_selected() {
        let accounts = two_accounts();
        let reg = world(&accounts, Vec::new());
        let ann = caller(&accounts, ANN.0);
        let ben = caller(&accounts, BEN.0);
        let anns = named(&reg, ANN.2).character_id;
        assert!(reg.character_change_as(ann, anns).is_some());
        assert!(reg.character_change_as(ben, anns).is_none());

        // Without --accounts, anyone may select anything, as before.
        let open = world(&Accounts::catch_all(1), vec![Character::default()]);
        let id = open.config().characters[0].character_id;
        let other = Caller::Session {
            user_id: 99,
            role: Role::Player.mask(),
        };
        assert!(open.character_change_as(other, id).is_some());
    }

    /// FIPS 180-4's published examples (NIST CSRC example values for
    /// SHA-256, and FIPS 180-2 Appendix B.3's one million `a`). The 56-byte
    /// input pads into a second block; the million crosses many.
    #[test]
    fn sha256_matches_the_published_vectors() {
        let a_million = vec![b'a'; 1_000_000];
        let cases: [(&[u8], &str); 5] = [
            (
                b"",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                b"abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (
                b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
                "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
            ),
            (
                &a_million,
                "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(hex(&sha256(input)), want, "input of {} bytes", input.len());
        }
    }

    /// RFC 4231 test cases 2 (a short key) and 6 (a key longer than a
    /// block, hashed first): HMAC-SHA256's published vectors.
    #[test]
    fn hmac_sha256_matches_rfc_4231() {
        assert_eq!(
            hex(&HmacSha256::new(b"Jefe").mac(b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex(&HmacSha256::new(&[0xaa; 131])
                .mac(b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    /// PBKDF2-HMAC-SHA256's published vectors: the RFC 6070 inputs as
    /// widely republished for SHA-256 (c = 1, 2, 4096, and the 40-byte
    /// two-block case), and RFC 7914 §11's 64-byte one.
    #[test]
    fn pbkdf2_hmac_sha256_matches_the_published_vectors() {
        let cases: [(&[u8], &[u8], u32, &str); 5] = [
            (
                b"password",
                b"salt",
                1,
                "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b",
            ),
            (
                b"password",
                b"salt",
                2,
                "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43",
            ),
            (
                b"password",
                b"salt",
                4096,
                "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a",
            ),
            (
                b"passwordPASSWORDpassword",
                b"saltSALTsaltSALTsaltSALTsaltSALTsalt",
                4096,
                "348c89dbcbd32b2f32d814b8116e84cf2b17347ebc1800181c4e2a1fb8dd53e1c635518c7dac47e9",
            ),
            (
                b"passwd",
                b"salt",
                1,
                "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc\
                 49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783",
            ),
        ];
        for (password, salt, c, want) in cases {
            let mut out = vec![0u8; want.len() / 2];
            pbkdf2_hmac_sha256(password, salt, c, &mut out);
            assert_eq!(hex(&out), want, "c = {c}");
        }
    }

    #[test]
    fn ct_eq_compares_whole_slices() {
        assert!(ct_eq(b"same", b"same"));
        assert!(!ct_eq(b"same", b"samf"));
        assert!(!ct_eq(b"xame", b"same"));
        assert!(!ct_eq(b"same", b"same!"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn a_token_equal_to_a_character_name_is_rejected() {
        let a = Accounts::parse(&line("Invented Pilot", 7, "player", "ola")).unwrap();
        let err = a
            .check_names(["Someone Else", "Invented Pilot"].into_iter())
            .unwrap_err();
        assert!(err.contains("line 1") && err.contains("ola"), "{err}");
        assert!(
            !err.contains("Invented Pilot"),
            "the error names the token: {err}"
        );
        // A random token passes the same check.
        let ok = Accounts::parse(&line(&hex(&[7u8; 32]), 7, "player", "ola")).unwrap();
        assert!(ok.check_names(["Invented Pilot"].into_iter()).is_ok());
        // Account names and the tools' fixed tokens are visible names too.
        for token in ["ola", "dev", ""] {
            let a = Accounts::parse(&line(token, 7, "gm", "ola")).unwrap();
            assert!(a.check_names(std::iter::empty()).is_err(), "{token:?}");
        }
        // The catch-all has no token to check.
        assert!(
            Accounts::catch_all(1)
                .check_names(["x"].into_iter())
                .is_ok()
        );
    }

    #[test]
    fn a_listener_account_overrides_the_token() {
        let text = line("tok-a", 11, "gm", "a") + &line("tok-b", 12, "player", "b");
        let a = Accounts::parse(&text).unwrap();
        let mut l = listener();
        assert_eq!(a.resolve(&l, Some(b"tok-a")).unwrap().user_id, 11);
        l.account = Some(12);
        for token in [Some(b"tok-a".as_slice()), Some(b"unknown"), None] {
            let r = a.resolve(&l, token).unwrap();
            assert_eq!((r.user_id, r.role), (12, Role::Player), "{token:?}");
        }
        l.account = Some(99);
        assert_eq!(a.resolve(&l, None), Err(Refused::NoSuchAccount(99)));
    }

    #[test]
    fn unknown_and_missing_tokens_are_refused_and_the_ceiling_caps() {
        let a = Accounts::parse(&line("tok-a", 11, "gm", "a")).unwrap();
        let mut l = listener();
        assert_eq!(a.resolve(&l, None), Err(Refused::NoToken));
        assert_eq!(a.resolve(&l, Some(b"")), Err(Refused::NoToken));
        let Err(Refused::UnknownToken { prefix }) = a.resolve(&l, Some(b"tok-b")) else {
            panic!("an unknown token resolved")
        };
        assert_eq!(prefix, hex(&sha256(b"tok-b")[..4]));
        assert_eq!(a.resolve(&l, Some(b"tok-a")).unwrap().role, Role::Gm);
        l.ceiling = Ceiling::Player;
        assert_eq!(a.resolve(&l, Some(b"tok-a")).unwrap().role, Role::Player);
        // The catch-all is a gm, capped the same way, whatever the token.
        let c = Accounts::catch_all(5);
        assert_eq!(
            c.resolve(&l, None),
            Ok(Resolved {
                user_id: 5,
                role: Role::Player
            })
        );
        assert_eq!(c.resolve(&listener(), Some(b"x")).unwrap().role, Role::Gm);
    }

    #[test]
    fn the_loader_refuses_every_malformed_or_repeated_line() {
        let h = |t: &str| hex(&sha256(t.as_bytes()));
        let good = line("t1", 1, "gm", "a");
        for bad in [
            String::new(),
            "# only a comment\n".to_owned(),
            format!("{}\t1\tgm\n", h("t1")),
            format!("{}\t1\tgm\ta\tb\tc\n", h("t1")),
            format!("{}\t1\tgm\ta\n", h("t1").to_uppercase()),
            format!("{}\t1\tgm\ta\n", &h("t1")[1..]),
            format!("{}\t0\tgm\ta\n", h("t1")),
            format!("{}\tx\tgm\ta\n", h("t1")),
            format!("{}\t1\troot\ta\n", h("t1")),
            format!("{}\t1\tgm\t \n", h("t1")),
            good.clone() + &line("t1", 2, "gm", "b"),
            good.clone() + &line("t2", 1, "gm", "b"),
            good.clone() + &line("t2", 2, "gm", "a"),
            format!(
                "{}\t1\tgm\ta\tPilot\n{}\t2\tgm\tb\tPilot\n",
                h("t1"),
                h("t2")
            ),
        ] {
            assert!(Accounts::parse(&bad).is_err(), "accepted {bad:?}");
        }
        let a = Accounts::parse(&format!("# header\n\n{good}")).unwrap();
        assert_eq!(a.list()[0].line, 3);
        assert!(!a.is_catch_all());
    }

    #[test]
    fn the_loader_refuses_a_file_inside_a_repo_or_writable_by_others() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ip27a-{}", std::process::id()));
        let repo = dir.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let inside = repo.join("accounts.tsv");
        let outside = dir.join("accounts.tsv");
        for p in [&inside, &outside] {
            std::fs::write(p, line("t1", 1, "gm", "a")).unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let err = Accounts::load(&inside).unwrap_err();
        assert!(err.contains("git"), "{err}");
        assert!(Accounts::load(&outside).is_ok());
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o620)).unwrap();
        assert!(Accounts::load(&outside).is_err(), "group-writable accepted");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_new_token_is_random_hex_and_its_line_loads() {
        let random = [0x5au8; 32];
        let t = new_token(&random, "ola", "player", Some("Invented Pilot"), None).unwrap();
        assert_eq!(t.token, hex(&random));
        assert_eq!(t.bots_line, format!("ola\t{}", t.token));
        let a = Accounts::parse(&t.accounts_line).unwrap();
        assert_eq!(a.list()[0].user_id, FIRST_USER_ID);
        let r = a.resolve(&listener(), Some(t.token.as_bytes())).unwrap();
        assert_eq!(r.role, Role::Player);
        // With the file given: the next id, and no repeated name.
        let t2 = new_token(&[1u8; 32], "kari", "gm", None, Some(&a)).unwrap();
        assert!(
            t2.accounts_line
                .contains(&format!("\t{}\t", FIRST_USER_ID + 1))
        );
        assert!(new_token(&[1u8; 32], "ola", "gm", None, Some(&a)).is_err());
        assert!(new_token(&[1u8; 32], "x", "gm", Some("Invented Pilot"), Some(&a)).is_err());
        assert!(new_token(&[1u8; 32], "x", "admin", None, None).is_err());
        assert!(new_token(&[1u8; 32], "a\tb", "gm", None, None).is_err());
        assert_ne!(urandom32().unwrap(), urandom32().unwrap());
    }

    #[test]
    fn the_role_masks_are_the_sandbox_and_the_bare_player() {
        assert_eq!(Role::Gm.mask(), crate::session::Session::default().role);
        assert_eq!(Role::Player.mask() & roles::ROLE_GML, 0);
        assert!(Role::Player < Role::Gm);
        assert_eq!(Role::Gm.capped(Ceiling::Player), Role::Player);
        assert_eq!(Role::Player.capped(Ceiling::Gm), Role::Player);
    }
}
