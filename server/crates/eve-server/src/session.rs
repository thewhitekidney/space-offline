//! What the server tells the client about itself once it is logged in.
//!
//! The handshake authenticates; the **session** is what gives the client an
//! identity and, crucially, a **role mask**. `docs/reference/session-roles.md` has the
//! role table; `crate::roles` has the constants and the one arithmetic trap.
//!
//! The session arrives as a `SessionInitialStateNotification` (command 18),
//! whose `params` are `('sid', 'sessionType', 'initialstate')` — confirmed
//! from `machoNetPacket.py`'s `__machodesc__`. `initialstate` is a **flat**
//! `{attr: value}` map and is applied verbatim:
//! `CoreSession.ApplyRemoteAttributeChanges` does no filtering and no type
//! coercion, unlike `SetAttributes`. Whatever we put here is what the session
//! has.
//!
//! **Omitting `role` is not neutral.** `GetDefaultValueOfAttribute('role')`
//! returns bare `ROLE_LOGIN`, so a session without an explicit role has no
//! player rights at all.

use crate::pyutil::pydict;
use crate::roles;
use eve_marshal::{PyValue, handshake as hs};

/// The session the server establishes for a logged-in client.
#[derive(Debug, Clone)]
pub struct Session {
    /// `sid`. **Per connection.** Cosmetic client-side -- no client path
    /// assigns `session.sid` from a packet, the client keeps the one it
    /// generated at boot (`docs/re/session-and-binding.md` §1) -- but it is
    /// the correlation key in a two-client capture, so it must differ per
    /// connection. `server::serve_one` mints it; the default below is only
    /// the default.
    pub session_id: i64,
    /// The transport address and the `*clientID` narrowcast id. **Per
    /// connection, never per account.** Two connections of one account must
    /// not share one, or whatever is kept by client id keeps only the later
    /// of the two. Minted by `server::client_id` from CCP's own formula; see
    /// that function.
    pub client_id: i64,
    /// The **account** id -- `session.userid`. Never a character id, never a
    /// connection id.
    ///
    /// `basesession.py:1600` raises `RuntimeError("A session's userID may not
    /// change")`, so this is set once, at connect, and must never appear in a
    /// session change.
    pub user_id: i64,
    /// The character this session selected, written by
    /// `dispatch::GpsSession::handle_select_character` and nowhere else.
    ///
    /// **`None` is a real answer, not a cue to pick one.** It is `None` from
    /// connect until `charUnboundMgr.SelectCharacterID`, and a service arm
    /// with no actor must answer its no-character branch rather than guess.
    /// It never enters [`Session::initial_state`] -- `charid` absent is what
    /// routes the client to character select.
    pub character_id: Option<i64>,
    /// See [`crate::roles`]. Defaults to [`roles::SANDBOX_DOCKED`], the
    /// catch-all account's mask; with accounts it is the resolved role's
    /// ([`Session::adopt`]).
    pub role: i64,
    pub language_id: String,
    /// `sessionType`. 5 for a normal client session.
    pub session_type: i32,
    /// The client's exact build, from H2 (`handshake::ClientIdentity::
    /// boot_build`), set when the handshake completes (BA-4). **Per
    /// connection**: one server answers every build of its accepted set,
    /// and a service that must one day behave per build branches on this.
    /// Nothing does yet. Never sent to the client.
    pub client_build: Option<i64>,
}

impl Default for Session {
    fn default() -> Self {
        Session {
            session_id: 1,
            client_id: 1,
            user_id: 1,
            character_id: None,
            role: roles::SANDBOX_DOCKED,
            language_id: "EN".to_owned(),
            session_type: hs::SESSION_TYPE,
            client_build: None,
        }
    }
}

impl Session {
    /// Take the account resolved in H6 (`docs/lanes/account-spec.md` §5):
    /// `userid`, and the role mask of `min(account role, listener
    /// ceiling)`. Called once, when the handshake completes and **before**
    /// the session is sent, because `basesession.py:1600` forbids a later
    /// `userid` change. The catch-all resolves to `Role::Gm`, whose mask is
    /// the default above, so a server without accounts sees no change.
    pub fn adopt(&mut self, r: crate::accounts::Resolved) {
        self.user_id = r.user_id;
        self.role = r.role.mask();
    }

    /// The flat `initialstate` map.
    ///
    /// **`charid` is deliberately absent.** `gameui`'s session handler routes
    /// on `session.charid is None`, and that is what sends the client to the
    /// character-select screen rather than into the game. Adding it here would
    /// skip the screen we are trying to reach.
    pub fn initial_state(&self) -> Vec<(PyValue, PyValue)> {
        let PyValue::Dict(entries) = pydict(vec![
            ("userid", PyValue::Int(self.user_id)),
            ("role", PyValue::Int(self.role)),
            (
                "languageID",
                PyValue::Bytes(self.language_id.as_bytes().to_vec()),
            ),
        ]) else {
            unreachable!("pydict returns a dict")
        };
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pyutil::dict_get;

    fn state(s: &Session) -> PyValue {
        PyValue::Dict(s.initial_state())
    }

    #[test]
    fn the_default_session_is_a_sandbox_with_the_gm_bits() {
        let s = Session::default();
        assert_ne!(s.role & roles::ROLE_GML, 0, "no GM tools without ROLE_GML");
        assert_ne!(s.role & roles::ROLE_GMH, 0);
        assert_ne!(s.role & roles::ROLE_LOGIN, 0);
        assert_ne!(s.role & roles::ROLE_PLAYER, 0);
        assert_eq!(s.role & roles::ROLE_SERVICE, 0, "ROLE_SERVICE poisons it");
    }

    #[test]
    fn charid_is_absent_so_the_client_goes_to_character_select() {
        // gameui routes on `session.charid is None`. Present, even as None,
        // is a different thing from absent in a flat attribute map that is
        // applied verbatim - so assert absence.
        assert!(dict_get(&state(&Session::default()), b"charid").is_none());
    }

    #[test]
    fn the_role_is_an_integer_the_client_can_mask_against() {
        let s = Session::default();
        let got = dict_get(&state(&s), b"role").cloned();
        assert_eq!(got, Some(PyValue::Int(roles::SANDBOX_DOCKED)));
    }

    /// `ROLE_QA` is what opens the Insider UI and QA menus, and the whole
    /// price of it is two stub methods carrying no data. It was excluded from
    /// the default for as long as those were unimplemented; this is the test
    /// that notices if someone takes it back out without also noticing.
    #[test]
    fn the_default_role_carries_the_qa_bit() {
        let s = Session::default();
        assert_ne!(
            s.role & roles::ROLE_QA,
            0,
            "no Insider UI menu without ROLE_QA"
        );
        // And not the one bit we deliberately left out: ROLE_CONTENT buys a
        // dungeon editor we cannot serve and a left-drag hijack in space.
        assert_eq!(s.role & roles::ROLE_CONTENT, 0);
    }

    #[test]
    fn the_session_carries_the_three_attributes_the_client_needs() {
        let st = state(&Session::default());
        for key in [b"userid".as_slice(), b"role", b"languageID"] {
            assert!(
                dict_get(&st, key).is_some(),
                "missing {}",
                String::from_utf8_lossy(key)
            );
        }
    }

    /// The resolved account, and nothing else, sets `userid` and `role`.
    #[test]
    fn a_resolved_player_gets_the_player_mask_and_its_own_userid() {
        use crate::accounts::{Resolved, Role};
        let mut s = Session::default();
        s.adopt(Resolved {
            user_id: 4321,
            role: Role::Player,
        });
        let st = state(&s);
        assert_eq!(dict_get(&st, b"userid"), Some(&PyValue::Int(4321)));
        assert_eq!(
            dict_get(&st, b"role"),
            Some(&PyValue::Int(Role::Player.mask()))
        );
        assert_eq!(s.role & roles::ROLE_GML, 0, "a player with GM tools");
        // A gm resolves to the default mask: no change without accounts.
        s.adopt(Resolved {
            user_id: 1,
            role: Role::Gm,
        });
        assert_eq!(s.role, Session::default().role);
    }

    #[test]
    fn session_type_is_five() {
        assert_eq!(Session::default().session_type, 5);
    }

    /// The character layer is server-side only.
    ///
    /// `character_id` was added to this struct so that service arms stop
    /// reading "the first character". It must not leak into `initialstate`:
    /// `charid` present is what skips character select, and `initial_state`
    /// is applied verbatim by `ApplyRemoteAttributeChanges`.
    #[test]
    fn the_selected_character_never_enters_the_initial_state() {
        let s = Session {
            character_id: Some(140_000_001),
            ..Session::default()
        };
        assert!(dict_get(&state(&s), b"charid").is_none());
        assert!(dict_get(&state(&s), b"characterID").is_none());
        // And the three keys we do send are still exactly those three.
        let PyValue::Dict(entries) = state(&s) else {
            unreachable!("state is a dict")
        };
        assert_eq!(entries.len(), 3);
    }

    /// Neither does the connection address.
    ///
    /// `client_id` is a transport address. The session attribute the client
    /// keeps under that name is installed by `_AssociateWithSession`, not by
    /// us (`docs/reference/login-path.md:423` sets `clientID = 0` with no packet), so
    /// sending it in `initialstate` would be us inventing an attribute.
    #[test]
    fn the_client_id_never_enters_the_initial_state() {
        let s = Session {
            client_id: 10_000_000_001,
            ..Session::default()
        };
        assert!(dict_get(&state(&s), b"clientID").is_none());
        // `userid` is still the account id, unchanged by any of this.
        assert_eq!(dict_get(&state(&s), b"userid"), Some(&PyValue::Int(1)));
    }
}
