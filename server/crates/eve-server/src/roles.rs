//! The session role mask.
//!
//! This is the single value that decides whether the client gives you the
//! Singularity sandbox — the dev and GM tooling — or an ordinary player
//! client. It is **not** a client-side flag: `serverInfo.IsLiveServer` is
//! referenced by no module but its own, and nothing in the client references
//! `IsTestServer` or `singularity`. Naming the server "Singularity" does
//! nothing. The tools come from the role mask *we* put in the session.
//!
//! Values are from `carbon/common/script/sys/serviceConst.py` in this build,
//! read out of the bytecode. Full role table, what each one unlocks, and the
//! reasoning behind the recommendation: `docs/reference/session-roles.md`.
//!
//! ## The trap, stated first because two of our own documents fell into it
//!
//! `ROLE_ANY` is **not** "every role". It is defined as all-ones minus
//! `ROLE_IGB` and the `ROLE_EXT_GM*_PLUS` masks — and
//! `ROLE_EXT_GM4_PLUS = ROLE_EXT_GM4 | ROLE_GML | ROLE_GMH`, which propagates
//! into all three of the other `_PLUS` masks. So **`ROLE_ANY` clears both GM
//! bits**:
//!
//! ```text
//! ROLE_ANY = 0xFF9C3FFF7FFFFFFF
//! ROLE_GML = 0x0040000000000000   ->  ROLE_ANY & ROLE_GML == 0
//! ROLE_GMH = 0x0020000000000000   ->  ROLE_ANY & ROLE_GMH == 0
//! ```
//!
//! `docs/archive/session-flow.md` and `docs/reference/login-path.md` §7 both recommended
//! `ROLE_ANY & ~ROLE_SERVICE`, which is therefore the one mask that looks
//! maximally permissive while switching off exactly the feature we want. It
//! loses the 153 `ROLE_GML` test sites, the "GM / WM Extras" submenu on every
//! right-click menu, and six of the Insider window's nine menus. Both
//! documents are corrected; [`ROLE_ANY_EXCLUDES_THE_GM_BITS`] asserts the
//! arithmetic at compile time so it cannot come back.
//!
//! `ROLE_ANY` also sets bit 63, so it cannot be marshalled as an `int64` at
//! all — a second, smaller reason to avoid it.

/// Required by two Carbon guards; a session without it has no player rights.
pub const ROLE_LOGIN: i64 = 1 << 62;
/// The other half of the baseline.
pub const ROLE_PLAYER: i64 = 1 << 61;
/// **Never set this.** See [`FORBIDDEN`].
pub const ROLE_SERVICE: i64 = 1 << 60;
pub const ROLE_NEWBIE: i64 = 1 << 59;
pub const ROLE_ADMIN: i64 = 1 << 56;
pub const ROLE_CONTENT: i64 = 1 << 55;
/// The dominant GM gate: 153 test sites across the client, more than any
/// other role.
pub const ROLE_GML: i64 = 1 << 54;
pub const ROLE_GMH: i64 = 1 << 53;
pub const ROLE_QA: i64 = 1 << 52;
pub const ROLE_PROGRAMMER: i64 = 1 << 51;
pub const ROLE_GMS: i64 = 1 << 38;
pub const ROLE_TOURNAMENT: i64 = 1 << 36;
pub const ROLE_SPAWN: i64 = 1 << 33;
pub const ROLE_HEALOTHERS: i64 = 1 << 23;
pub const ROLE_HEALSELF: i64 = 1 << 22;
pub const ROLE_CHTADMINISTRATOR: i64 = 1 << 21;
/// Makes the client invisible in member lists — strictly worse for us.
pub const ROLE_CHTINVISIBLE: i64 = 1 << 20;
pub const ROLE_LEGIONEER: i64 = 1 << 18;
pub const ROLE_WORLDMOD: i64 = 1 << 12;
/// Classifies the client as an NPC in chat — strictly worse for us.
pub const ROLE_PINKCHAT: i64 = 1 << 6;

/// CCP's own `ROLE_ANY`, recorded **only** so the exclusion can be asserted.
/// Do not use it as a mask: it clears the GM bits and sets bit 63.
pub const ROLE_ANY: u64 = 0xFF9C_3FFF_7FFF_FFFF;

/// CCP's `ROLEMASK_ELEVATEDPLAYER`. The Insider window opens on an any-of
/// test against this, and it inherits `ROLE_ANY`'s exclusions — so it does
/// **not** contain the GM bits either. Insider's menu bar is separately gated
/// on `ROLE_GML`/`ROLE_WORLDMOD`, which is why both have to be set: with only
/// the GM bits Insider never opens, and with only the elevated bits it opens
/// with an empty menu bar.
pub const ROLEMASK_ELEVATEDPLAYER: u64 = 0x959C_3FFF_7EFF_FFDF;

/// Roles that must never be set, with the reason.
///
/// `ROLE_SERVICE` is the dangerous one, and it fails in a way worth
/// describing: `machoNetTransport.LockSession` raises *"Session map failure,
/// attempting to run a session init or change in a service session context"*
/// **before** the packet is applied, and `CoreSession.ApplyRemoteAttributeChanges`
/// raises again. Both test the **pre-change** role, so the notification that
/// introduces it succeeds and then every subsequent session change — including
/// selecting a character — fails. The session is poisoned rather than
/// degraded, which makes it an annoying fault to diagnose from the symptom.
pub const FORBIDDEN: &[(i64, &str)] = &[
    (
        ROLE_SERVICE,
        "poisons the session: every later session change raises",
    ),
    (
        ROLE_NEWBIE,
        "starts a rookie-help chat tasklet against a chat server we do not have",
    ),
    (
        ROLE_CHTINVISIBLE,
        "makes the client invisible in member lists",
    ),
    (ROLE_PINKCHAT, "classifies the client as an NPC in chat"),
];

/// The sandbox mask: every role with a reader in this build that we want, and
/// nothing else.
///
/// `0x61F8005200E41000`. Bit 63 is clear, so this marshals as a plain `int64`
/// (tag `0x03`), bytes `00 10 e4 00 52 00 f8 61`.
///
/// The 25 roles with zero client readers are deliberately omitted — setting a
/// bit nothing tests is noise, and it makes the mask harder to reason about
/// the next time something misbehaves.
pub const SANDBOX: i64 = ROLE_LOGIN
    | ROLE_PLAYER
    | ROLE_ADMIN
    | ROLE_CONTENT
    | ROLE_GML
    | ROLE_GMH
    | ROLE_QA
    | ROLE_PROGRAMMER
    | ROLE_GMS
    | ROLE_TOURNAMENT
    | ROLE_SPAWN
    | ROLE_HEALOTHERS
    | ROLE_HEALSELF
    | ROLE_CHTADMINISTRATOR
    | ROLE_LEGIONEER
    | ROLE_WORLDMOD;

/// [`SANDBOX`] without the two roles that have a measured cost. **This is the
/// default**, and should stay the default until character creation and the
/// localization service are answered.
///
/// - `ROLE_QA` is the one role that makes the client call something we have
///   not implemented: `ccSvc.CreateCharacterWithDoll` → `_QAPickStarterSystem`
///   calls `charUnboundMgr.GetQAStarterSystemIDs()` with **no try/except**, on
///   every character creation. Unimplemented, character creation aborts.
/// - `ROLE_CONTENT` starts the `scenario` service on entering space and can
///   bind `keeper.GetLevelEditor()`; low risk, but the second thing to drop if
///   space misbehaves.
///
/// **Both debts are now paid for `ROLE_QA`**, so it is no longer excluded —
/// see [`SANDBOX_DOCKED`]. This constant is kept because it is the honest
/// "nothing unimplemented can be reached" mask, and it is what to fall back
/// to if the QA bit ever turns out to cost more than the two stubs.
pub const SANDBOX_SAFE: i64 = SANDBOX & !(ROLE_QA | ROLE_CONTENT);

/// **The default.** [`SANDBOX`] without `ROLE_CONTENT`.
///
/// `ROLE_QA` is in, and it is the best value-for-work in the whole role
/// table: it unlocks the Insider **UI** and **QA** menus — the UI debugger,
/// the UI tree, the window manager, the icon browser, the theme, gradient and
/// colour editors, keybindings, sprites, graphs and the performance monitors
/// — and every one of them is **pure client-side `trinity`/`carbonui` code
/// that makes no server call at all.** The entire price is the two stubs
/// below, neither of which carries any data:
///
/// - `charUnboundMgr.GetQAStarterSystemIDs()` → `[]`. Called from
///   `_QAPickStarterSystem` with no handler, but only during *character
///   creation*; `sorted([])` is falsy and it returns immediately.
/// - `localizationServer.UpdateLocalizationQAWrap(bool)` → `None`. Fires once
///   per character login from `localizationClient.OnCharacterSessionChanged`,
///   under `session.role & ROLE_QA == ROLE_QA` — note the **all-of** form —
///   and the return value is discarded.
///
/// `ROLE_CONTENT` stays out: it buys a dungeon editor we cannot serve and a
/// left-drag hijack in space, and neither is worth anything while docked.
///
/// The arithmetic is `0x61780052_00E41000`.
pub const SANDBOX_DOCKED: i64 = SANDBOX & !ROLE_CONTENT;

/// An ordinary player, for comparing behaviour against.
pub const PLAYER_ONLY: i64 = ROLE_LOGIN | ROLE_PLAYER;

/// The arithmetic that two of our documents got wrong, asserted at compile
/// time rather than in a test, so that no edit can reintroduce it.
pub const ROLE_ANY_EXCLUDES_THE_GM_BITS: () = {
    assert!(ROLE_ANY & (ROLE_GML as u64) == 0);
    assert!(ROLE_ANY & (ROLE_GMH as u64) == 0);
    assert!(ROLEMASK_ELEVATEDPLAYER & (ROLE_GML as u64) == 0);
    // And our mask does contain them, which is the entire point.
    assert!(SANDBOX & ROLE_GML != 0);
    assert!(SANDBOX & ROLE_GMH != 0);
    assert!(SANDBOX_SAFE & ROLE_GML != 0);
    assert!(SANDBOX_SAFE & ROLE_GMH != 0);
    // Never, under any circumstances.
    assert!(SANDBOX & ROLE_SERVICE == 0);
    assert!(SANDBOX_SAFE & ROLE_SERVICE == 0);
    assert!(SANDBOX_DOCKED & ROLE_SERVICE == 0);
    // The default carries the QA bit and not the content bit.
    assert!(SANDBOX_DOCKED & ROLE_QA != 0);
    assert!(SANDBOX_DOCKED & ROLE_CONTENT == 0);
    assert!(SANDBOX_DOCKED & ROLE_GML != 0);
    assert!(SANDBOX_DOCKED & ROLE_GMH != 0);
    // Bit 63 clear keeps the mask marshallable as an int64.
    assert!(SANDBOX >= 0 && SANDBOX_SAFE >= 0 && SANDBOX_DOCKED >= 0);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_masks_are_the_values_read_from_the_binary() {
        assert_eq!(SANDBOX, 0x61F8_0052_00E4_1000);
        assert_eq!(SANDBOX, 7_059_392_768_105_517_056);
        assert_eq!(SANDBOX_SAFE, 0x6168_0052_00E4_1000);
        assert_eq!(SANDBOX_SAFE, 7_018_860_371_459_182_592);
        assert_eq!(SANDBOX_DOCKED, 0x6178_0052_00E4_1000);
        // Exactly one bit more than the old default, and it is bit 52.
        assert_eq!(SANDBOX_DOCKED & !SANDBOX_SAFE, ROLE_QA);
        assert_eq!(ROLE_QA, 1 << 52);
    }

    #[test]
    fn the_sandbox_mask_marshals_as_an_int64() {
        // Little-endian, as tag 0x03 writes it.
        assert_eq!(
            SANDBOX.to_le_bytes(),
            [0x00, 0x10, 0xe4, 0x00, 0x52, 0x00, 0xf8, 0x61]
        );
    }

    #[test]
    fn the_mask_ccp_calls_any_would_have_disabled_the_gm_tools() {
        // The mistake, written out so the next reader sees it rather than
        // trusting a comment: the "maximally permissive" mask is the one that
        // switches off the feature this project exists for.
        let tempting = ROLE_ANY & !(ROLE_SERVICE as u64);
        assert_eq!(tempting & (ROLE_GML as u64), 0, "ROLE_ANY has no GML");
        assert_eq!(tempting & (ROLE_GMH as u64), 0, "ROLE_ANY has no GMH");
        assert_ne!(tempting & (ROLE_QA as u64), 0, "but it does have QA");
    }

    #[test]
    fn nothing_forbidden_is_in_either_mask() {
        for (role, why) in FORBIDDEN {
            assert_eq!(SANDBOX & role, 0, "SANDBOX contains a role that {why}");
            assert_eq!(
                SANDBOX_SAFE & role,
                0,
                "SANDBOX_SAFE contains a role that {why}"
            );
        }
    }

    #[test]
    fn the_baseline_roles_are_always_present() {
        // Two Carbon guards require both; without them the session has no
        // player rights, and omitting `role` entirely leaves the client at
        // bare ROLE_LOGIN.
        for mask in [SANDBOX, SANDBOX_SAFE, PLAYER_ONLY] {
            assert_ne!(mask & ROLE_LOGIN, 0);
            assert_ne!(mask & ROLE_PLAYER, 0);
        }
    }

    #[test]
    fn the_safe_default_drops_exactly_two_roles() {
        assert_eq!(SANDBOX & !SANDBOX_SAFE, ROLE_QA | ROLE_CONTENT);
    }
}
