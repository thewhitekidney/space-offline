//! The slash console and the QA tools.
//!
//! Split out of `service.rs`, which had grown to 7374 lines in one file --
//! and that, rather than anything about the work itself, was what capped
//! parallel implementation at one agent. A lane working on this subsystem
//! now opens one file and cannot collide with a lane working on another.
//!
//! The arms are unchanged by the move: `self` became the `reg` argument and
//! nothing else. Helpers deliberately stayed in the parent for now, because
//! several are shared across domains and moving those is where a split of
//! this kind goes wrong.

use super::*;

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &["slash", "publicQaToolsServer"];

/// Whether `caller` may run the slash console.
///
/// A client session needs `ROLE_GML`, the client's own dominant GM gate
/// (`roles.rs`). A server-made call is not gated (RR1 M10): the ticker and
/// the tests call `Registry::slash` and `dispatch` directly, and those are
/// not a player asking. Until accounts land (IP27-a) every session is
/// `SANDBOX_DOCKED`, which has `ROLE_GML`, so nobody is refused yet.
fn may_use_slash(caller: Caller) -> bool {
    match caller {
        Caller::Internal => true,
        Caller::Session { role, .. } => role & crate::roles::ROLE_GML != 0,
    }
}

/// The refusal a player gets.
///
/// A `SlashError` with a `reason`, the same shape the command list uses, so
/// the client's handling is the one it already has. **`"/"` still answers
/// a list, an empty one**: `FetchRemoteCommandList` `eval`s the text after
/// `': '` and runs before every console command, so any other text would be
/// an `UnboundLocalError` in front of the player rather than a refusal
/// (see the arm below). [A] the client shows `reason` for any other
/// command; falsified if a player's console shows a traceback instead.
fn refuse_slash(line: &str) -> ServiceError {
    let reason = if line == "/" {
        "Available commands: []".to_owned()
    } else {
        "GM commands need a GM account on this server.".to_owned()
    };
    ServiceError::Raise(eve_proto::user_error(
        "SlashError",
        vec![(bstr(&reason), bstr("reason"))],
    ))
}

/// **A reply the player can see.** `chat.slash_command` files a command's
/// return value in the logger window (`'slash result: %s'`), which nobody has
/// open, so `/bots`'s usage line and its answers were invisible: the owner
/// typed it and "nothing happened". An exception is the one thing that
/// reaches the screen, so `/bots` answers as a raised `CustomInfo` (a modal;
/// `info` is its text). [A] the client shows `info` verbatim; falsified by an
/// empty or garbled dialog (then try `SlashError`'s `reason`). Every other
/// command still answers with its value, as before.
pub(crate) fn visible_reply(line: &str, reply: PyValue) -> Result<PyValue, ServiceError> {
    let bots = line
        .split_whitespace()
        .next()
        .is_some_and(|w| w.eq_ignore_ascii_case("/bots"));
    let text = match &reply {
        PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        PyValue::Unicode(u) => u.clone(),
        _ => return Ok(reply),
    };
    if !bots {
        return Ok(reply);
    }
    Err(ServiceError::Raise(eve_proto::user_error(
        "CustomInfo",
        vec![(bstr(&text), bstr("info"))],
    )))
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        // The slash console -- the single most-referenced remote service
        // in the client (97 `RemoteSvc('slash')` sites, more than
        // `charMgr` or `map`) and the backbone of the GM sandbox.
        //
        // **The client parses the command and ships it to us whole**, as
        // one string: `"/" + lowercased-command + " " + args`. Note the
        // *unconditional trailing space* even with no arguments, so
        // `"/whoami "` is what arrives -- never compare with `==`.
        //
        // There is no server-side `slash` in the shipped tree; all of it
        // is ours to write. There is also no role check in the client's
        // dispatcher: gating happens at the call sites.
        "slash" => match req.method {
            "SlashCmd" => {
                // The wire form carries an unconditional trailing
                // space, so trim before comparing.
                let line = match req.args.first() {
                    Some(PyValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
                    Some(PyValue::Unicode(u)) => u.clone(),
                    _ => String::new(),
                };
                let line = line.trim().to_owned();
                // **The GM gate, and the only one** (`docs/review/plan.md`
                // IP21; RR1 M10). A session without `ROLE_GML` gets a
                // `SlashError` and nothing runs. Gated here, at the client's
                // way in, and not in `Registry::slash`: every other caller of
                // that is the server's own (`docs/lanes/ip21.md`).
                if !may_use_slash(reg.caller()) {
                    return Err(refuse_slash(&line));
                }
                if line == "/" {
                    // **This one must RAISE, and the "always answer with
                    // a value" default is wrong for it.**
                    //
                    //     def FetchRemoteCommandList(self):
                    //         try:
                    //             sm.RemoteSvc('slash').SlashCmd('/')
                    //         except UserError as e:
                    //             msg = e.args[1]['reason']
                    //             cmds = eval(msg.split(': ')[1])
                    //             cmds.sort()
                    //         ...
                    //         reg.remoteCommandList = cmds
                    //
                    // `cmds` is bound only inside the handler. Verified
                    // from the bytecode rather than the decompiler's
                    // indentation: the success path is
                    // `119 JUMP_FORWARD -> 168 -> 178 JUMP_FORWARD -> 182
                    // LOAD_FAST cmds`, and `STORE_FAST cmds` appears only
                    // inside the two except blocks. So a normal return is
                    // an `UnboundLocalError`, and because `_SlashCmd`
                    // calls this before matching anything, it breaks
                    // **every** slash command -- including `/whoami`,
                    // which never touches the server.
                    //
                    // Two constraints on the text, both from the parse
                    // above: `': '` must appear exactly once before the
                    // payload, and the payload must `eval` to a sortable
                    // Python list literal. The names only drive
                    // autocompletion; they authorise nothing.
                    return Err(ServiceError::Raise(eve_proto::user_error(
                        "SlashError",
                        vec![(
                            bstr(&format!(
                                "Available commands: {:?}",
                                Registry::SLASH_COMMANDS
                            )),
                            bstr("reason"),
                        )],
                    )));
                }
                // Everything else: the console stringifies whatever
                // comes back (`'slash result: %s' % res`) and chat skips
                // the line when it is `None`. So any value works, and a
                // string is the natural one -- which also means the
                // string is our only channel back to the player, and is
                // worth writing for them rather than for us.
                let reply = reg.slash(req.actor, &line);
                // TT-B C1: a `/bots` relay waits on eve-bots over TCP, so
                // `run` handed it out; the dispatcher runs it after the
                // world lock and replaces this placeholder.
                if let Some(job) = crate::bot_control::take_deferred(reg) {
                    crate::bot_control::park(
                        reg,
                        crate::bot_control::PostLock {
                            line: line.clone(),
                            job,
                        },
                    );
                    return Ok(PyValue::None);
                }
                reply.and_then(|reply| visible_reply(&line, reply))
            }
            _ => Err(reg.no_method(req)),
        },
        // **Not gated, and need not be**: every arm below is a no-op or a
        // constant, so a player calling it changes nothing
        // (`public_qa_tools_stays_a_no_op_for_players`). Whoever gives
        // `/giveitem` a body here must gate it as `slash` is gated.
        //
        // The Singularity sandbox proper, and the best-value find in the
        // whole GM recon: **these commands are not role-gated at all**.
        //
        // `chat.handle_slash_command` tests
        // `publicQaToolsClient.CommandAllowed(line)` -- a plain
        // `startswith` over a 14-entry list, with no role check --
        // *before* it reaches the `ROLE_SLASH` branch. That is exactly how
        // CCP let ordinary players use the test server's sandbox.
        //
        // The client-side list, recovered from `co_consts` because
        // uncompyle6 printed it as the indices `[4..17]`:
        // `/moveme /copyskills /copyships /boostsov /booststandings
        //  /giveitem /shipcloak /shipuncloak /mycharid /giveallskills
        //  /givetesteventskills /omega /alpha /completeextraction`.
        //
        // Three methods, and two of the three returns are discarded:
        // `PublicQaToolsClient.SlashCmd` has no `return` at all, and
        // `MoveMeTo` is wrapped in `PerformSessionChange`. Only
        // `CanGiveItemForMultifit` is read, as a bool.
        //
        // **[I]** The argument grammar for `/giveitem` and friends is
        // ours to define -- `CommandAllowed` only matches the prefix, so
        // nothing in the client constrains what follows.
        "publicQaToolsServer" => match req.method {
            // SOC-D2: `/boostsov` moves the caller's own fuel blocks into
            // their alliance's hub (`service/sov_hub.rs`); every other line
            // is still a no-op.
            "SlashCmd" => super::sov_hub::public_qa(reg, req),
            "MoveMeTo" => Ok(PyValue::None),
            "CanGiveItemForMultifit" => Ok(PyValue::Bool(true)),
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    #![allow(unused_imports)]
    use super::*;
    use crate::pyutil::{as_bytes, dict_get};

    /// Tests for this subsystem belong here, not in the parent. That is the
    /// other half of why the split exists: two lanes adding a method each
    /// would otherwise contend for one `mod tests`.
    /// Every service this module advertises must reach an arm in *this*
    /// module's `dispatch`.
    ///
    /// It used to check the name against a hand-maintained list in the
    /// parent, which is now derived from these very lists -- so that check
    /// had become a tautology. This probes the dispatcher instead, with a
    /// method that cannot exist: `NoSuchMethod` means we got to the service,
    /// `NoSuchService` means the name is advertised and falls straight
    /// through, which is a refused call, which is a hung client.
    #[test]
    fn every_service_here_is_routed_to_this_module() {
        let mut reg = Registry::new(SandboxConfig::default());
        for s in SERVICES {
            let req = Request {
                service: s,
                method: "__no_such_method_exists__",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: None,
            };
            match dispatch(&mut reg, &req) {
                Err(ServiceError::NoSuchMethod { .. }) => {}
                other => panic!(
                    "{s} is advertised by this module but its dispatch answers {other:?} \
                     to a method that cannot exist"
                ),
            }
        }
    }

    /// A session as `dispatch.rs` makes one, with the given role.
    fn session(role: i64) -> Caller {
        Caller::Session { user_id: 1, role }
    }

    /// `slash.SlashCmd(line)` on behalf of `caller`, as the wire sends it:
    /// with the trailing space.
    fn slash_cmd(reg: &mut Registry, caller: Caller, line: &str) -> Result<PyValue, ServiceError> {
        let args = [bstr(&format!("{line} "))];
        let actor = Some(reg.config().characters[0].character_id);
        reg.dispatch_as(
            caller,
            &Request {
                service: "slash",
                method: "SlashCmd",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor,
            },
        )
    }

    /// The `reason` of a raised `SlashError`, or a panic naming what came
    /// back instead.
    fn slash_error_reason(out: Result<PyValue, ServiceError>) -> String {
        let Err(ServiceError::Raise(PyValue::ObjectEx { header, .. })) = &out else {
            panic!("expected a raised SlashError, got {out:?}")
        };
        let PyValue::Tuple(h) = header.as_ref() else {
            panic!("UserError header is a tuple: {header:?}")
        };
        let PyValue::Tuple(args) = &h[1] else {
            panic!("UserError args are a tuple: {h:?}")
        };
        assert_eq!(args[0], bstr("SlashError"));
        match dict_get(&args[1], b"reason").and_then(as_bytes) {
            Some(r) => String::from_utf8_lossy(r).into_owned(),
            None => panic!("a SlashError carries a reason: {args:?}"),
        }
    }

    /// The text of a raised `CustomInfo` (what `/bots` answers with), or a
    /// panic naming what came back instead.
    fn info_text(out: Result<PyValue, ServiceError>) -> String {
        let Err(ServiceError::Raise(PyValue::ObjectEx { header, .. })) = &out else {
            panic!("expected a raised CustomInfo, got {out:?}")
        };
        let PyValue::Tuple(h) = header.as_ref() else {
            panic!("UserError header is a tuple: {header:?}")
        };
        let PyValue::Tuple(args) = &h[1] else {
            panic!("UserError args are a tuple: {h:?}")
        };
        assert_eq!(args[0], bstr("CustomInfo"));
        let text = dict_get(&args[1], b"info").and_then(as_bytes);
        String::from_utf8_lossy(text.expect("an info text")).into_owned()
    }

    #[test]
    fn a_bots_command_is_answered_where_the_player_can_see_it() {
        let gm = session(crate::session::Session::default().role);
        let mut reg = Registry::new(SandboxConfig::default());
        // Not on a dev server: still an answer, and a visible one.
        let text = info_text(slash_cmd(&mut reg, gm, "/bots"));
        assert!(text.contains("/bots"), "{text}");
        let text = info_text(slash_cmd(&mut reg, gm, "/bots miner"));
        assert!(text.contains("/bots"), "{text}");
        // Other commands still answer with their value.
        let out = slash_cmd(&mut reg, gm, "/whoami").expect("answers");
        assert!(matches!(out, PyValue::Bytes(_)), "{out:?}");
    }

    #[test]
    fn a_player_session_cannot_run_createitem() {
        let mut reg = Registry::new(SandboxConfig::default());
        let before = reg.store.items().len();
        let reason = slash_error_reason(slash_cmd(
            &mut reg,
            session(crate::roles::PLAYER_ONLY),
            "/createitem Rifter 1",
        ));
        assert!(!reason.is_empty());
        assert_eq!(reg.store.items().len(), before, "nothing was created");
        assert_eq!(reg.caller(), Caller::Internal, "the caller is reset");

        // The command list still parses for the client, and lists nothing.
        let reason =
            slash_error_reason(slash_cmd(&mut reg, session(crate::roles::PLAYER_ONLY), "/"));
        assert_eq!(reason.split(": ").nth(1), Some("[]"), "{reason}");
    }

    #[test]
    fn a_gm_session_can_run_createitem() {
        // Today's role for every session (`Session::default`), so this is
        // also the proof that nobody's behaviour changes before IP27-a.
        let gm = session(crate::session::Session::default().role);
        let mut reg = Registry::new(SandboxConfig::default());
        let before = reg.store.items().len();
        let out = slash_cmd(&mut reg, gm, "/createitem Rifter 1").expect("answers");
        assert!(matches!(out, PyValue::Bytes(_)), "{out:?}");
        assert!(reg.store.items().len() > before, "the item exists");

        // And the full command list, as before the gate.
        let reason = slash_error_reason(slash_cmd(&mut reg, gm, "/"));
        assert_ne!(reason.split(": ").nth(1), Some("[]"), "{reason}");
    }

    #[test]
    fn public_qa_tools_stays_a_no_op_for_players() {
        let mut reg = Registry::new(SandboxConfig::default());
        let before = reg.store.items().len();
        let player = session(crate::roles::PLAYER_ONLY);
        let line = [bstr("/giveitem Rifter 1")];
        let actor = Some(reg.config().characters[0].character_id);
        for (method, args) in [
            ("SlashCmd", &line[..]),
            ("MoveMeTo", &[][..]),
            ("CanGiveItemForMultifit", &[][..]),
        ] {
            let out = reg.dispatch_as(
                player,
                &Request {
                    service: "publicQaToolsServer",
                    method,
                    args,
                    kwargs: &[],
                    bind_params: None,
                    actor,
                },
            );
            assert!(out.is_ok(), "{method} answers a player: {out:?}");
        }
        assert_eq!(reg.store.items().len(), before, "and changes nothing");
    }
}
