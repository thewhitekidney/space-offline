//! A minimal CPython 2.7 marshalled code object, for the handshake's
//! `signedFunc`.
//!
//! **Why this exists.** Handshake step H8 (`docs/reference/login-path.md` §4) hands the
//! client a `signedFunc`, which it runs through `Crypto.Verify`, then
//! `marshal.loads`, then `eval` inside `GPS.__Execute`. That `marshal` is
//! *CPython's own* code-object marshal, not `blue.marshal` — a different
//! format entirely from the rest of this server, which is why it lives in its
//! own module and shares nothing with `wire`.
//!
//! **Why a deliberately trivial function is enough.** `GPS.__Execute` captures
//! stdout/stderr into a `WriteBuffer`, and if the `eval` raises it swallows the
//! exception, prints the traceback into the buffer and returns
//! `funcResult = {}`. The client never inspects `funcResult`. So the client
//! needs *something* that unmarshals to a code object and is evaluable; it does
//! not need the real expression. That matters, because
//! `carbon/server/script/net/handShake.py` (`GetHandshakeFunc`, `Verify`) is
//! server-only and absent from the client tree, so the real expression's
//! content is simply not recoverable from what we have.
//!
//! ## The format
//!
//! Python 2.7 `marshal` type bytes used here, from CPython 2.7's
//! `Python/marshal.c`:
//!
//! ```text
//! 'N' (0x4E)  None, no payload
//! 'i' (0x69)  int32, little-endian
//! 's' (0x73)  string: int32 length, then that many bytes
//! '(' (0x28)  tuple:  int32 count, then that many objects
//! 'c' (0x63)  code object, with the field order below
//! ```
//!
//! A `'c'` object's fields, in this exact order:
//! `argcount, nlocals, stacksize, flags` (four int32s), then
//! `code, consts, names, varnames, freevars, cellvars` as objects, then
//! `filename, name` as objects, then `firstlineno` (int32), then `lnotab`.
//!
//! Note Python 2.7 marshal has no reference/`FLAG_REF` mechanism — that is a
//! Python 3 addition — so there is nothing to track across the stream. There
//! *is* a string back-reference pair (`'t'` interned-string, which appends to
//! an interning list, and `'R'` stringref, an index into it); CPython's own
//! `.pyc` files are full of both. We never emit either — plain `'s'` decodes
//! identically and costs a few bytes — but `tools/pyc27.py` reads them,
//! because the client's own files do.
//!
//! **This is not the EVE wire marshal.** `blue.marshal` and
//! `crate::wire`/`eve-marshal` share not one type byte with the above: in the
//! wire format `0x14` is a tuple with an escaped count and `0x16` a dict of
//! `(value, key)` pairs, while here `'('` is a tuple with a plain `i32` count
//! and there is no dict shape we use at all. The two formats meet only in that
//! the output of this module travels *inside* a wire-marshal
//! `PyValue::Bytes` — an opaque byte string as
//! far as `blue.marshal` is concerned. Nothing in this file may be reused
//! there, and nothing there may be reused here.
//!
//! ## Status of this module: verified encoding, unverified acceptance
//!
//! **Confirmed:** the bytes this emits parse back cleanly through our own
//! Python 2.7 marshal parser (`tools/pyc27.py`), consuming the stream exactly
//! and yielding `argcount=0, nlocals=0, stacksize=1, flags=0x40,
//! consts=(None,), code=64 00 00 53`. The golden bytes are pinned in the test
//! below.
//!
//! **Not confirmed, and cannot be from here:** that CPython 2.7's real
//! `marshal.loads` accepts it, that `eval` of it succeeds, and that
//! `Crypto.Verify` under Placebo is the identity that `docs/reference/login-path.md`
//! infers it to be. There is no Python 2.7 on this machine and no live client
//! yet. The first client connection settles all three at once; until then,
//! treat this as the most likely handshake step to fail.
//!
//! ## Three emitters, one of which is a template
//!
//! [`handshake_func`] and [`settings_info_func`] are constants, which is the
//! rule `docs/reference/code-push.md` states: *a code push is a constant, never a
//! template.* [`settings_info_scenario_func`] breaks that rule deliberately —
//! it is the client-CI trigger, a `GetSettingsInfo` blob with an `execfile` of
//! a named scenario script in front of the usual `return {}`, so a test can be
//! started inside the real client with nobody typing anything
//! (`docs/archive/plan-client-ci.md` §4.1, `docs/lanes/ci-trigger.md`). The one value
//! it interpolates is validated by [`scenario_path`] and comes from the
//! server's command line, never from the wire.

/// CPython 2.7 `marshal` type bytes. Named rather than inline so a reader can
/// check them against `Python/marshal.c` without decoding ASCII.
mod ty {
    pub const NONE: u8 = b'N';
    pub const INT: u8 = b'i';
    pub const STRING: u8 = b's';
    pub const TUPLE: u8 = b'(';
    pub const CODE: u8 = b'c';
}

/// `CO_OPTIMIZED` — locals live in the frame's fast-locals array rather than
/// in a dict. Every compiled *function* body has it; module and class bodies
/// do not.
const CO_OPTIMIZED: i32 = 0x01;

/// `CO_NEWLOCALS` — the frame gets a fresh `f_locals`. Also universal for
/// function bodies.
const CO_NEWLOCALS: i32 = 0x02;

/// `CO_NOFREE` — set when a code object has neither free nor cell variables.
/// CPython computes this itself when compiling; we must set it by hand.
const CO_NOFREE: i32 = 0x40;

/// `LOAD_CONST 0` then `RETURN_VALUE`: evaluates to `None` and cannot raise.
///
/// Python 2.7 opcodes: `LOAD_CONST` is 100 and takes a 2-byte little-endian
/// argument; `RETURN_VALUE` is 83 and takes none.
const RETURN_NONE: [u8; 4] = [100, 0, 0, 83];

/// `BUILD_MAP 0` then `RETURN_VALUE`: evaluates to `{}` and cannot raise.
///
/// Python 2.7 opcodes: `BUILD_MAP` is 105 and takes a 2-byte little-endian
/// argument, which is only a size *hint* for the new dict (`_PyDict_NewPresized`)
/// and pops nothing; `RETURN_VALUE` is 83 and takes none. Hand-assembled rather
/// than compiled, because there is no Python 2.7 anywhere in this project --
/// see `docs/reference/code-push.md` for how it is checked instead.
const BUILD_EMPTY_MAP_RETURN: [u8; 4] = [105, 0, 0, 83];

fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_string(out: &mut Vec<u8>, s: &[u8]) {
    out.push(ty::STRING);
    put_i32(out, s.len() as i32);
    out.extend_from_slice(s);
}

fn put_empty_tuple(out: &mut Vec<u8>) {
    out.push(ty::TUPLE);
    put_i32(out, 0);
}

/// Marshal a code object whose body is `return None`.
///
/// `filename` and `name` appear in any traceback the client prints into its
/// capture buffer, so they are worth making recognisable: if this ever shows
/// up in a client log, it should be obvious where it came from.
pub fn return_none(filename: &[u8], name: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(ty::CODE);
    put_i32(&mut out, 0); // argcount
    put_i32(&mut out, 0); // nlocals
    put_i32(&mut out, 1); // stacksize: one value pushed, then returned
    put_i32(&mut out, CO_NOFREE);
    put_string(&mut out, &RETURN_NONE); // co_code
    // co_consts = (None,)
    out.push(ty::TUPLE);
    put_i32(&mut out, 1);
    out.push(ty::NONE);
    put_empty_tuple(&mut out); // co_names
    put_empty_tuple(&mut out); // co_varnames
    put_empty_tuple(&mut out); // co_freevars
    put_empty_tuple(&mut out); // co_cellvars
    put_string(&mut out, filename);
    put_string(&mut out, name);
    put_i32(&mut out, 1); // co_firstlineno
    put_string(&mut out, b""); // co_lnotab: no line-number mapping
    out
}

/// The `signedFunc` we send in H8: `return None`, labelled for a log.
pub fn handshake_func() -> Vec<u8> {
    return_none(b"<handshake>", b"<module>")
}

/// Marshal a code object for a no-argument function whose body is
/// `return {}` — the body we install in the client's `SettingsInfo`.
///
/// **Why the client needs this at all.** `charMgr.GetSettingsInfo` is a
/// server-side code push, and the only one in the call table. From
/// `carbonui/services/settingsSvc.py`, `UpdateSettingsStatistics`
/// (code object `firstlineno` 198, first body line **205**):
///
/// ```text
/// code, verified = Crypto.Verify(sm.RemoteSvc('charMgr').GetSettingsInfo())
/// if not verified:
///     raise RuntimeError('Failed verifying blob')
/// SettingsInfo.__code__ = marshal.loads(code)
/// ret = SettingsInfo()
/// if len(ret) > 0:
///     sm.RemoteSvc('charMgr').LogSettings(ret)
/// ```
///
/// `Crypto.Verify` is `evecrypto/placebo.py(22)` for this build — `cryptoPack`
/// is Placebo — and is the whole of `verify`:
///
/// ```text
/// def verify(signedData):
///     return (signedData[0], signedData[1] == 0)
/// ```
///
/// So the answer must be a 2-sequence `(blob, 0)`: element 1 equal to `0`
/// makes `verified` true, and element 0 is handed straight to
/// `marshal.loads`.
///
/// **What the wrong answer did.** We answered `None` here for several runs,
/// with a source comment predicting it would cost "exactly one isolated log
/// record". It cost a `TypeError: 'NoneType' object is not subscriptable`
/// inside `placebo.verify` — `signedData[0]` on `None` — which we only ever
/// saw because the client **mails us its tracebacks**
/// (`docs/guides/debugging-the-client.md` §3.c2, `evecap --alerts`). The frames were
/// `settingsSvc.py(215) OnCharacterSessionChanged` →
/// `settingsSvc.py(205) UpdateSettingsStatistics` → `placebo.py(22) verify`,
/// and 215 is the sole call site: `OnCharacterSessionChanged` fires it once
/// per session, when `oldCharacterID is None`. One traceback, not a loop —
/// but a traceback on the character-login path all the same.
///
/// **Why `return {}` and not the real statistics function.** The real body is
/// pushed from CCP's cluster at runtime and appears nowhere in the client
/// tree — the client ships the socket, not the payload — so its content is
/// not recoverable from anything we have. What the client does with the
/// result is therefore the whole specification, and that is one line:
/// `len(ret) > 0` gates the `LogSettings` follow-up.
///
/// Any empty container satisfies `len`, so the choice between `{}`, `[]` and
/// `()` is not forced by the call site. A **dict** is right anyway: the value
/// is passed on to `charMgr.LogSettings` as a collection of named settings
/// statistics, and a mapping is the only shape that could ever carry those
/// names. What *is* forced is that it be empty and not `None`: `None` is a
/// `TypeError` at `len` one line later, and a non-empty value would invent
/// telemetry we do not want and send the client back to us with
/// `LogSettings`.
///
/// **Why the fields are what they are.** The code object replaces
/// `SettingsInfo.__code__`, so it has to fit that function. The real
/// `SettingsInfo` (`settingsSvc.py` `firstlineno` 231, body `pass` on line
/// 235), read out of the `.pyc` with `tools/pyc27.py`:
///
/// ```text
/// argcount 0  nlocals 0  stacksize 1  flags 0x43
/// code     'd\x00\x00S'   (LOAD_CONST 0; RETURN_VALUE)
/// consts   (None,)   names ()   varnames ()   freevars ()   cellvars ()
/// ```
///
/// We match `argcount 0` (it is called as `SettingsInfo()`), `nlocals 0` and
/// `flags 0x43`. The one field that is actually *enforced* is the free-variable
/// count: CPython 2.7's `func_set_code` compares `PyCode_GetNumFree(value)`
/// against `len(func.__closure__)` and raises
/// `ValueError: SettingsInfo() requires a code object with 0 free vars, not N`
/// on a mismatch. `SettingsInfo` is a module-level `def` with no closure, so
/// `freevars` must be empty — which it is, and `CO_NOFREE` says so. `argcount`
/// is not checked by the assignment; it is checked by the call one line later.
pub fn return_empty_dict(filename: &[u8], name: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(ty::CODE);
    put_i32(&mut out, 0); // argcount: called as `SettingsInfo()`
    put_i32(&mut out, 0); // nlocals: no arguments and no assignments
    // stacksize: BUILD_MAP pushes one dict, RETURN_VALUE pops it. One slot is
    // the true maximum; CPython would compute the same. Too small is a stack
    // overrun in the interpreter, so this is not a field to round down.
    put_i32(&mut out, 1);
    // flags: byte-for-byte the real `SettingsInfo`'s 0x43. CO_NOFREE is the
    // load-bearing bit (see above); the other two are what any compiled
    // function body carries and cost nothing to be honest about.
    put_i32(&mut out, CO_OPTIMIZED | CO_NEWLOCALS | CO_NOFREE);
    put_string(&mut out, &BUILD_EMPTY_MAP_RETURN); // co_code
    // co_consts, co_names, co_varnames are all empty, and that is the point:
    // `BUILD_MAP 0; RETURN_VALUE` loads nothing, so there is no constant, no
    // global and no local for the client to resolve. A code object that
    // references nothing cannot fail on anything but the opcodes themselves.
    put_empty_tuple(&mut out); // co_consts
    put_empty_tuple(&mut out); // co_names
    put_empty_tuple(&mut out); // co_varnames
    put_empty_tuple(&mut out); // co_freevars  -- must be empty, see above
    put_empty_tuple(&mut out); // co_cellvars
    put_string(&mut out, filename);
    put_string(&mut out, name);
    put_i32(&mut out, 1); // co_firstlineno
    // co_lnotab: empty is valid and means "every offset is co_firstlineno".
    // `PyCode_Addr2Line` walks a table of (addr-delta, line-delta) byte pairs
    // and returns `co_firstlineno` when there are none, so a traceback through
    // this frame prints line 1 of the filename below rather than misreporting
    // a line of the client's own settingsSvc.py.
    put_string(&mut out, b"");
    out
}

/// The blob `charMgr.GetSettingsInfo` answers with.
///
/// The filename is the one thing in here a human will ever see: it is what a
/// client-side traceback prints if this frame is ever on the stack, and it is
/// deliberately unmistakable, because a frame from a file that does not exist
/// on disk is otherwise a genuinely baffling thing to be handed. `co_name`
/// stays `SettingsInfo` so the frame still names the function it *is*.
pub fn settings_info_func() -> Vec<u8> {
    return_empty_dict(b"<eve-offline settings stub>", b"SettingsInfo")
}

/// `ty::INT` is used only by this helper, which nothing needs yet; kept so the
/// type byte stays documented alongside the others it belongs with.
#[allow(dead_code)]
fn put_int(out: &mut Vec<u8>, v: i32) {
    out.push(ty::INT);
    put_i32(out, v);
}

// ---------------------------------------------------------------------------
// The CI trigger: the same `GetSettingsInfo` blob, plus an `execfile`.
// ---------------------------------------------------------------------------

/// Python 2.7 opcodes used by the scenario body, from CPython 2.7's
/// `Include/opcode.h`. Named so a reader can check each against the table in
/// `tools/pyc27.py`, which is the same table transcribed independently.
mod op {
    pub const POP_TOP: u8 = 1;
    pub const RETURN_VALUE: u8 = 83;
    pub const POP_BLOCK: u8 = 87;
    pub const END_FINALLY: u8 = 88;
    /// Everything from here up takes a two-byte little-endian argument.
    pub const HAVE_ARGUMENT: u8 = 90;
    pub const LOAD_CONST: u8 = 100;
    pub const BUILD_MAP: u8 = 105;
    pub const JUMP_FORWARD: u8 = 110;
    pub const LOAD_GLOBAL: u8 = 116;
    pub const SETUP_EXCEPT: u8 = 121;
    pub const CALL_FUNCTION: u8 = 131;
}

/// The directory the harness deploys scenario scripts into, as the *client*
/// sees it. A fixed contract shared with the lane that writes the scripts and
/// the lane that runs the client: `tools/run-client.sh` passes
/// `C:\eve-logs` as `/fileLogDirectory` and it is an ordinary host directory
/// inside the Proton prefix (`docs/archive/plan-client-ci.md` §2.2 **[V]**). Do not
/// change it here alone.
const SCENARIO_DIR: &str = r"c:\eve-logs\client-ci";

/// The longest scenario name we will put in a code object. Nothing needs a
/// long one and a bound keeps the emitted constant a predictable size.
const SCENARIO_NAME_MAX: usize = 64;

/// `c:\eve-logs\client-ci\<name>.py`, or an error if `name` is not one we are
/// willing to interpolate into a path.
///
/// **Why this is validated at all.** Everything else in this module is a
/// compile-time constant, and `docs/reference/code-push.md` states the rule that makes
/// that safe: *a code push is a constant, never a template.* This function
/// breaks that rule on purpose — it is the one place where a code object the
/// client **runs** is assembled around a value chosen at runtime — so the
/// value has to be pinned down here instead. The name comes from the server's
/// own command line and never from the wire, but the check is cheap and the
/// failure mode if it were ever reached from elsewhere is arbitrary code
/// execution on the player's machine.
///
/// Accepted: 1..=64 bytes, each of `A-Z a-z 0-9 . _ -`, starting with an
/// ASCII alphanumeric. That first-character rule is what rejects `..`,
/// `.hidden` and `-flag` without a special case for any of them; the
/// character set is what rejects `/`, `\`, `:`, `"` and every other way out of
/// the directory.
pub fn scenario_path(name: &str) -> Result<String, &'static str> {
    if name.is_empty() {
        return Err("scenario name is empty");
    }
    if name.len() > SCENARIO_NAME_MAX {
        return Err("scenario name is longer than 64 bytes");
    }
    if !name.as_bytes()[0].is_ascii_alphanumeric() {
        return Err("scenario name must start with a letter or a digit");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
        return Err("scenario name may only contain A-Z a-z 0-9 . _ -");
    }
    Ok(format!("{SCENARIO_DIR}\\{name}.py"))
}

/// Push an opcode with its two-byte little-endian argument.
fn put_op(out: &mut Vec<u8>, opcode: u8, arg: u16) {
    debug_assert!(opcode >= op::HAVE_ARGUMENT);
    out.push(opcode);
    out.extend_from_slice(&arg.to_le_bytes());
}

/// The `charMgr.GetSettingsInfo` blob, with a scenario script wired into it:
/// the client-CI trigger.
///
/// The Python this hand-assembles, and the only Python it hand-assembles:
///
/// ```text
/// def SettingsInfo():
///     try:
///         execfile(r'c:\eve-logs\client-ci\<name>.py', globals())
///     except:
///         pass
///     return {}
/// ```
///
/// ## Why this call site, and what it must still do
///
/// `docs/archive/plan-client-ci.md` §4.1 settles the choice of hook: the startup
/// `/tools=` argument is dead in a packaged client (`autoexec.pyc:202-209`
/// **[V]**), and of the two code pushes we already serve, this one runs
/// **after character selection**, with `sm`, `session` and the service layer
/// up. H8's `signedFunc` runs before services exist.
///
/// The blob's own contract is unchanged and must stay so: the body still ends
/// `BUILD_MAP 0 ; RETURN_VALUE`, so `settingsSvc.py(209)`'s `ret =
/// SettingsInfo()` gets `{}`, `len(ret) > 0` on 210 is false and
/// `LogSettings` on 211 is not reached — exactly what
/// `settings_info_func()` does today. A scenario that breaks settings loading
/// has broken login, so the scenario is bolted *in front of* that body and
/// changes nothing after it.
///
/// ## Why the `try`/`except`, given that the call site already catches
///
/// The call site does swallow, but two frames up and at a cost. Read bottom
/// up (all **[V]**, from `tools/dis.py` on this build's own `.pyc` files):
///
/// ```text
/// serviceManager.py(881)  uthread.worker(prefix + srvGuid,
///                             self.MollycoddledUthread, srvGuid, eventid,
///                             getattr(srv, eventid), args, keywords, ...)
/// serviceManager.py(1013) MollycoddledUthread:
///                             try: func(*args, **keywords)
///                             except: self.LogError(...); log.LogException()
///                                     sys.exc_clear()
/// sessions.py(659)        sm.ScatterEvent('OnCharacterSessionChanged', ...)
/// settingsSvc.py(215)     self.UpdateSettingsStatistics()
/// settingsSvc.py(205-211) the code push, with **no** try/except of its own
/// ```
///
/// So `UpdateSettingsStatistics` itself catches nothing, and a throw from
/// inside our code object unwinds into `MollycoddledUthread`'s bare `except`.
/// The client survives — but line 209 never returns, so the settings path is
/// abandoned half-done and the client mails us an error naming
/// `OnCharacterSessionChanged`, which is a confusing place to be told that a
/// test script has a typo.
///
/// Catching inside the code object turns that into: the scenario's exception
/// is dropped, `{}` is still returned, the settings path completes normally.
/// The traceback is not lost — it is the *scenario's* job to report its own
/// failure (`result.json`, `docs/archive/plan-client-ci.md` §3), and a scenario that
/// dies silently shows up as a missing result file, which is the harness's
/// timeout rather than the client's problem. A bare `except:` also catches
/// `SystemExit`, which matters: a scenario that ends by exiting should not be
/// able to take the settings frame with it.
///
/// ## Inline, not `uthread.new` — and why the evidence says so
///
/// §4.1 left this open. The evidence closes it in favour of inline:
///
/// 1. **Nothing waits.** `ScatterEvent` hands each handler to
///    `uthread.worker` and discards the returned tasklet (`POP_TOP` at
///    `serviceManager.py(881)`, offset 367) **[V]**. `uthread.worker` is
///    `PoolWorker` (`uthread/__init__.py:1021`), which is `Pool`, which
///    builds a **fresh** `bluepy.TaskletExt` per call and schedules it
///    (`uthread/__init__.py:713-717`) **[V]** — it is not a bounded pool, so
///    there is no worker slot to starve. Our frame is therefore already on a
///    detached, un-joined, per-event tasklet: blocking it for the whole
///    scenario blocks nothing the client is waiting on.
/// 2. **No block trap.** `NotifySessionChangeWithoutTheStars` sets
///    `block_trap` only for `eventid == 'DoSessionChanging'`
///    (`serviceManager.py(932-934)`) **[V]**; the `ScatterEvent` path sets
///    none, and a fresh tasklet starts untrapped. So a scenario may sleep and
///    wait, which is the whole point of one.
/// 3. **A tasklet would not buy protection.** Stackless is cooperative, so a
///    scenario that never yields freezes the client whether it was spawned or
///    not; `uthread.new` moves the freeze, it does not prevent it.
/// 4. **It would cost a name we cannot resolve.** `uthread` is *not* in
///    `settingsSvc`'s module globals (its `co_names` are `Bunch, CHAR_GROUPS,
///    Crypto, …, marshal, os, prefs` — no `uthread`) **[V]**, and
///    `builtinmangler.py` injects exceptions, `const` and `strx` into
///    `__builtin__` but no `uthread` **[V]**. Our code object executes with
///    `SettingsInfo.__globals__`, i.e. that module dict, so
///    `LOAD_GLOBAL uthread` would have to be an `IMPORT_NAME` and a local —
///    more hand-assembled opcodes in the one thing on this machine that
///    cannot be executed to check.
///
/// A scenario that *wants* its own tasklet can say `uthread.new(...)` on its
/// first line, in ordinary Python that a real compiler produced. That is the
/// right place for the decision, not here.
///
/// ## Where the opcodes come from
///
/// Not from reasoning — from the client's own compiled code. `svc_console.py`
/// ships `execfile(filename, globals())`, the very expression the human types
/// today, and its bytes at `cmd_execfile` offsets 55..71 are **[V]**:
///
/// ```text
/// 74 06 00   LOAD_GLOBAL  6 (execfile)
/// 7c 02 00   LOAD_FAST    2 (filename)
/// 74 07 00   LOAD_GLOBAL  7 (globals)
/// 83 00 00   CALL_FUNCTION 0
/// 83 02 00   CALL_FUNCTION 2
/// 01         POP_TOP
/// ```
///
/// Ours is that sequence with `LOAD_FAST filename` replaced by `LOAD_CONST`
/// of a baked-in path, which is the only difference a const-instead-of-a-local
/// can make. `cmd_execfile`'s own `co_stacksize` is **3**, the same depth this
/// expression reaches here, and it resolves `execfile` and `globals` by
/// `LOAD_GLOBAL` from a module that imports neither — so both really are
/// reachable as builtins in this build.
///
/// The `except:` epilogue is likewise copied from compiler output rather than
/// invented: `POP_BLOCK ; JUMP_FORWARD 7 ; POP_TOP ×3 ; JUMP_FORWARD 1 ;
/// END_FINALLY` is byte-identical to `linecache.updatecache`,
/// `pstats.load_stats`, `profile.trace_dispatch_c_call` and
/// `copy_reg._slotnames` in this client's own `lib/` **[V]**, and
/// `profile.trace_dispatch_c_call` even has the same `SETUP_EXCEPT 20`
/// argument, because its `try` body happens to be the same 16 bytes long.
/// `END_FINALLY` is unreachable for a bare `except` and the compiler emits it
/// anyway; we keep it for the same reason, so the stream is one a real
/// compiler could have produced.
///
/// ## Errors
///
/// `Err` if `name` is not an acceptable scenario name — see
/// [`scenario_path`]. The caller should treat that as a startup error and
/// refuse to run, not fall back to the plain blob, because a mistyped
/// `--scenario` that silently starts a normal session is a test that passes
/// by doing nothing.
pub fn settings_info_scenario_func(name: &str) -> Result<Vec<u8>, &'static str> {
    let path = scenario_path(name)?;

    // --- co_code --------------------------------------------------------
    //
    // Offsets are written out because every jump argument below is derived
    // from them, and a hand-assembled jump that lands mid-instruction is the
    // single most likely way for this to go wrong.
    //
    //   0  SETUP_EXCEPT 20      handler at 3 + 20 = 23
    //   3  LOAD_GLOBAL  0       execfile
    //   6  LOAD_CONST   1       the scenario path
    //   9  LOAD_GLOBAL  1       globals
    //  12  CALL_FUNCTION 0      globals()
    //  15  CALL_FUNCTION 2      execfile(path, globals())
    //  18  POP_TOP              discard its None
    //  19  POP_BLOCK            leave the try block
    //  20  JUMP_FORWARD 7       to 30, skipping the handler
    //  23  POP_TOP              handler: discard exc type,
    //  24  POP_TOP                               value,
    //  25  POP_TOP                               traceback
    //  26  JUMP_FORWARD 1       to 30
    //  29  END_FINALLY          unreachable; the compiler emits it, so do we
    //  30  BUILD_MAP 0          the {} settingsSvc.py(210) measures
    //  33  RETURN_VALUE
    let mut code = Vec::new();
    put_op(&mut code, op::SETUP_EXCEPT, 20);
    put_op(&mut code, op::LOAD_GLOBAL, 0); // execfile
    put_op(&mut code, op::LOAD_CONST, 1); // consts[1] = the path
    put_op(&mut code, op::LOAD_GLOBAL, 1); // globals
    put_op(&mut code, op::CALL_FUNCTION, 0);
    put_op(&mut code, op::CALL_FUNCTION, 2);
    code.push(op::POP_TOP);
    code.push(op::POP_BLOCK);
    put_op(&mut code, op::JUMP_FORWARD, 7);
    code.push(op::POP_TOP);
    code.push(op::POP_TOP);
    code.push(op::POP_TOP);
    put_op(&mut code, op::JUMP_FORWARD, 1);
    code.push(op::END_FINALLY);
    put_op(&mut code, op::BUILD_MAP, 0);
    code.push(op::RETURN_VALUE);

    // --- the code object ------------------------------------------------
    let mut out = Vec::new();
    out.push(ty::CODE);
    put_i32(&mut out, 0); // argcount: still called as `SettingsInfo()`
    put_i32(&mut out, 0); // nlocals: no arguments, no assignments
    // stacksize 3, the deepest the value stack gets:
    //   [execfile], [execfile, path], [execfile, path, globals]  -> 3
    // then CALL_FUNCTION 0 leaves 3, CALL_FUNCTION 2 leaves 1, POP_TOP 0.
    // The handler is entered with the three exception values, which is also
    // 3 -- CPython 2.7's `stackdepth_walk` scores a SETUP_EXCEPT target at
    // `depth + 3`, and depth is 0 there. `cmd_execfile` carries 3 for the
    // same expression. A test below recomputes this from the emitted bytes.
    put_i32(&mut out, 3);
    // flags: byte-for-byte the real `SettingsInfo`'s 0x43. CO_NOFREE is the
    // bit `func_set_code` actually enforces (`docs/reference/code-push.md`).
    put_i32(&mut out, CO_OPTIMIZED | CO_NEWLOCALS | CO_NOFREE);
    put_string(&mut out, &code);
    // co_consts = (None, path). Index 0 is the docstring slot, which CPython
    // fills with None for a function without one -- the shipped
    // `SettingsInfo` has exactly `(None,)` **[V]** -- so the path lands at 1
    // and the LOAD_CONST above says 1.
    out.push(ty::TUPLE);
    put_i32(&mut out, 2);
    out.push(ty::NONE);
    put_string(&mut out, path.as_bytes());
    // co_names = ('execfile', 'globals'), in the order the LOAD_GLOBALs
    // index them. Both are builtins; `svc_console.cmd_execfile` resolves the
    // same two the same way **[V]**.
    out.push(ty::TUPLE);
    put_i32(&mut out, 2);
    put_string(&mut out, b"execfile");
    put_string(&mut out, b"globals");
    put_empty_tuple(&mut out); // co_varnames
    put_empty_tuple(&mut out); // co_freevars -- must be empty, see docs/reference/code-push.md
    put_empty_tuple(&mut out); // co_cellvars
    put_string(&mut out, b"<eve-offline ci trigger>");
    put_string(&mut out, b"SettingsInfo");
    put_i32(&mut out, 1); // co_firstlineno
    // co_lnotab empty: every offset reports line 1 of the filename above,
    // rather than misreporting a line of the client's own settingsSvc.py.
    put_string(&mut out, b"");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden bytes, produced by this function and verified by parsing them
    /// with `tools/pyc27.py` in the analysis container. If this test fails,
    /// the encoder changed and the parse must be redone — do not just update
    /// the constant.
    const GOLDEN: &str = "630000000000000000010000004000000073040000006400005328010000004e2800\
000000280000000028000000002800000000730b0000003c68616e647368616b653e73080000003c6d6f64756c65\
3e010000007300000000";

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn matches_the_verified_golden_bytes() {
        assert_eq!(hex(&handshake_func()), GOLDEN.replace('\n', ""));
    }

    #[test]
    fn length_is_ninety_bytes() {
        assert_eq!(handshake_func().len(), 90);
    }

    #[test]
    fn the_names_land_in_the_output() {
        let co = return_none(b"<somewhere>", b"<thing>");
        let window = co.windows(11);
        assert!(window.clone().any(|w| w == b"<somewhere>"));
        assert!(co.windows(7).any(|w| w == b"<thing>"));
        // Changing the names must change the length, i.e. they are not padded
        // into a fixed field.
        assert_ne!(co.len(), handshake_func().len());
    }

    /// Golden bytes for the `charMgr.GetSettingsInfo` blob, 109 bytes.
    ///
    /// **Verified by parsing them with `tools/pyc27.py`**, our read-only
    /// Python 2.7 `.pyc` reader -- the same parser that reads the client's own
    /// compiled modules, so parsing our blob with it exercises the real
    /// format rather than a mirror of our own assumptions. It consumed exactly
    /// 109 of 109 bytes and yielded
    ///
    /// ```text
    /// argcount 0  nlocals 0  stacksize 1  flags 0x43
    /// code     'i\x00\x00S'  ->  BUILD_MAP 0 ; RETURN_VALUE
    /// consts () names () varnames () freevars () cellvars ()
    /// filename '<eve-offline settings stub>'  name 'SettingsInfo'
    /// firstlineno 1  lnotab ''
    /// ```
    ///
    /// If this test fails, the encoder changed. Re-run the parse before
    /// touching the constant:
    ///
    /// ```text
    /// tools/analysis.sh python3 -I -c "
    /// import sys; sys.path.insert(0,'/work/tools')
    /// from pyc27 import Unmarshal, Code, dis
    /// b = bytes.fromhex('<the new hex>')
    /// u = Unmarshal(b, 0); c = u.load()
    /// assert u.p == len(b), (u.p, len(b))
    /// print([(f, getattr(c, f)) for f in Code.__slots__]); print(list(dis(c)))
    /// "
    /// ```
    const SETTINGS_GOLDEN: &str = "6300000000000000000100000043000000730400000069000053280000\
            0000280000000028000000002800000000280000000073\
            1b0000003c6576652d6f66666c696e6520736574746\
            96e677320737475623e730c0000005365747469\
            6e6773496e666f010000007300000000";

    #[test]
    fn settings_blob_matches_the_verified_golden_bytes() {
        assert_eq!(
            hex(&settings_info_func()),
            SETTINGS_GOLDEN.replace(['\n', ' '], "")
        );
        assert_eq!(settings_info_func().len(), 109);
    }

    /// Read the emitted stream back field by field, so the assertion is about
    /// *structure* and not only about a hex string that two people could agree
    /// to change together. Deliberately a separate, dumb reader rather than a
    /// shared one: if the encoder and the checker share code, a sign error
    /// cancels out and the test proves nothing.
    #[test]
    fn settings_blob_round_trips_through_a_reader() {
        let b = settings_info_func();
        let mut p = 0usize;
        let byte = |p: &mut usize| {
            let v = b[*p];
            *p += 1;
            v
        };
        let i32_at = |p: &mut usize| {
            let v = i32::from_le_bytes(b[*p..*p + 4].try_into().unwrap());
            *p += 4;
            v
        };

        assert_eq!(byte(&mut p), b'c', "type byte: TYPE_CODE");
        assert_eq!(i32_at(&mut p), 0, "co_argcount: SettingsInfo() takes none");
        assert_eq!(i32_at(&mut p), 0, "co_nlocals");
        assert_eq!(i32_at(&mut p), 1, "co_stacksize: BUILD_MAP pushes one");
        assert_eq!(
            i32_at(&mut p),
            0x43,
            "co_flags: the real SettingsInfo's exact flags, CO_NOFREE included"
        );

        // co_code
        assert_eq!(byte(&mut p), b's', "co_code is TYPE_STRING");
        assert_eq!(i32_at(&mut p), 4, "co_code length");
        assert_eq!(
            &b[p..p + 4],
            &[105u8, 0, 0, 83],
            "BUILD_MAP 0 (105, two-byte arg) ; RETURN_VALUE (83)"
        );
        p += 4;

        // co_consts, co_names, co_varnames, co_freevars, co_cellvars -- five
        // empty tuples. co_freevars being empty is the one CPython enforces
        // when `SettingsInfo.__code__` is assigned.
        for which in [
            "co_consts",
            "co_names",
            "co_varnames",
            "co_freevars",
            "co_cellvars",
        ] {
            assert_eq!(byte(&mut p), b'(', "{which} is TYPE_TUPLE");
            assert_eq!(i32_at(&mut p), 0, "{which} is empty");
        }

        for (which, want) in [
            ("co_filename", &b"<eve-offline settings stub>"[..]),
            ("co_name", &b"SettingsInfo"[..]),
        ] {
            assert_eq!(byte(&mut p), b's', "{which} is TYPE_STRING");
            let n = i32_at(&mut p) as usize;
            assert_eq!(n, want.len(), "{which} length");
            assert_eq!(&b[p..p + n], want, "{which}");
            p += n;
        }

        assert_eq!(i32_at(&mut p), 1, "co_firstlineno");
        assert_eq!(byte(&mut p), b's', "co_lnotab is TYPE_STRING");
        assert_eq!(
            i32_at(&mut p),
            0,
            "co_lnotab is empty: all lines are line 1"
        );

        // The stream must end exactly here. A marshal reader stops when the
        // object is complete and never notices trailing junk, so a trailing
        // byte would be invisible to the client and to the parse above.
        assert_eq!(p, b.len(), "the code object consumes the whole stream");
    }

    /// The two blobs this module emits must not drift into each other. They
    /// differ in three fields that matter -- opcode, flags and consts -- and
    /// one of them is `eval`ed while the other is installed as a function
    /// body, so a copy-paste between them is a real hazard.
    #[test]
    fn the_two_blobs_are_not_the_same_thing() {
        let h = handshake_func();
        let s = settings_info_func();
        assert_ne!(h, s);
        // LOAD_CONST (100) vs BUILD_MAP (105), at the same offset: byte 22
        // is the first byte of co_code, after the 'c' tag, four i32 fields,
        // the 's' tag and the i32 length.
        assert_eq!(h[22], 100);
        assert_eq!(s[22], 105);
        // flags 0x40 (module-ish) vs 0x43 (a function body).
        assert_eq!(&h[13..17], &0x40i32.to_le_bytes());
        assert_eq!(&s[13..17], &0x43i32.to_le_bytes());
    }

    #[test]
    fn starts_with_the_code_type_byte_and_a_zero_argcount() {
        let co = handshake_func();
        assert_eq!(co[0], b'c');
        assert_eq!(&co[1..5], &0i32.to_le_bytes());
    }

    // -----------------------------------------------------------------------
    // The CI trigger blob.
    // -----------------------------------------------------------------------

    /// A deliberately dumb reader for the fields of an emitted code object,
    /// written for the tests and sharing no code with the encoder. Returns
    /// `(co_code, co_consts_raw, co_names, co_stacksize, trailing_bytes)`
    /// where the const/name entries are the raw marshal payloads.
    struct Fields {
        stacksize: i32,
        flags: i32,
        code: Vec<u8>,
        consts: Vec<Vec<u8>>,
        names: Vec<Vec<u8>>,
        filename: Vec<u8>,
        name: Vec<u8>,
        consumed: usize,
    }

    /// Parse one marshalled code object. Only the shapes this module emits
    /// are understood -- `N`, `s` and `(` -- which is the point: anything
    /// else in the stream is a bug, not a format to support.
    fn parse(b: &[u8]) -> Fields {
        let mut p = 0usize;
        let byte = |p: &mut usize| {
            let v = b[*p];
            *p += 1;
            v
        };
        let i32_at = |p: &mut usize| {
            let v = i32::from_le_bytes(b[*p..*p + 4].try_into().unwrap());
            *p += 4;
            v
        };
        fn string(b: &[u8], p: &mut usize) -> Vec<u8> {
            assert_eq!(b[*p], b's', "expected TYPE_STRING at {p}");
            *p += 1;
            let n = i32::from_le_bytes(b[*p..*p + 4].try_into().unwrap()) as usize;
            *p += 4;
            let v = b[*p..*p + n].to_vec();
            *p += n;
            v
        }
        /// A tuple of `N` or `s` entries; `None` becomes an empty vec, which
        /// is unambiguous here because every string we emit is non-empty.
        fn tuple(b: &[u8], p: &mut usize) -> Vec<Vec<u8>> {
            assert_eq!(b[*p], b'(', "expected TYPE_TUPLE at {p}");
            *p += 1;
            let n = i32::from_le_bytes(b[*p..*p + 4].try_into().unwrap()) as usize;
            *p += 4;
            (0..n)
                .map(|_| {
                    if b[*p] == b'N' {
                        *p += 1;
                        Vec::new()
                    } else {
                        string(b, p)
                    }
                })
                .collect()
        }

        assert_eq!(byte(&mut p), b'c', "TYPE_CODE");
        assert_eq!(i32_at(&mut p), 0, "co_argcount");
        assert_eq!(i32_at(&mut p), 0, "co_nlocals");
        let stacksize = i32_at(&mut p);
        let flags = i32_at(&mut p);
        let code = string(b, &mut p);
        let consts = tuple(b, &mut p);
        let names = tuple(b, &mut p);
        assert!(tuple(b, &mut p).is_empty(), "co_varnames must be empty");
        assert!(tuple(b, &mut p).is_empty(), "co_freevars must be empty");
        assert!(tuple(b, &mut p).is_empty(), "co_cellvars must be empty");
        let filename = string(b, &mut p);
        let name = string(b, &mut p);
        assert_eq!(i32_at(&mut p), 1, "co_firstlineno");
        assert!(string(b, &mut p).is_empty(), "co_lnotab must be empty");
        Fields {
            stacksize,
            flags,
            code,
            consts,
            names,
            filename,
            name,
            consumed: p,
        }
    }

    const SCENARIO: &str = "undock_smoke";

    fn scenario_blob() -> Vec<u8> {
        settings_info_scenario_func(SCENARIO).expect("a plain name must be accepted")
    }

    #[test]
    fn scenario_path_is_the_agreed_contract() {
        assert_eq!(
            scenario_path("undock_smoke").unwrap(),
            r"c:\eve-logs\client-ci\undock_smoke.py"
        );
        // Two other lanes depend on this exact spelling: the one that writes
        // the scripts and the one that launches the client.
        assert_eq!(scenario_path("a").unwrap(), r"c:\eve-logs\client-ci\a.py");
    }

    #[test]
    fn a_name_that_could_escape_the_directory_is_refused() {
        for bad in [
            "",            // nothing to run
            "..",          // the parent directory
            "../../evil",  // classic traversal
            r"..\..\evil", // the Windows spelling, which is what matters
            ".hidden",     // leading dot, i.e. `..` minus one character
            "-flag",       // could be read as an argument elsewhere
            "a b",         // a space would need quoting somewhere
            "a/b",         // a subdirectory
            r"a\b",        // a subdirectory, Windows
            "c:evil",      // a drive-relative path
            "a'b",         // quote characters have no business here
            "a\"b",
            "a.py\u{0}x", // an embedded NUL truncates in C, not in Rust
        ] {
            assert!(
                scenario_path(bad).is_err(),
                "scenario_path({bad:?}) must be refused"
            );
            assert!(
                settings_info_scenario_func(bad).is_err(),
                "settings_info_scenario_func({bad:?}) must be refused"
            );
        }
        // 64 is the limit, 65 is not.
        assert!(scenario_path(&"a".repeat(64)).is_ok());
        assert!(scenario_path(&"a".repeat(65)).is_err());
    }

    /// The fields the client's own code constrains: `co_argcount` 0 (it is
    /// called as `SettingsInfo()`), `co_freevars` empty (`func_set_code`
    /// enforces it), `co_flags` the real function's 0x43, and a body that
    /// still ends by returning an empty dict so `settingsSvc.py(210)`'s
    /// `len(ret) > 0` is false.
    #[test]
    fn the_scenario_blob_keeps_the_settings_contract() {
        let b = scenario_blob();
        let f = parse(&b);
        assert_eq!(f.consumed, b.len(), "the object consumes the whole stream");
        assert_eq!(f.flags, 0x43, "the real SettingsInfo's flags");
        assert_eq!(
            f.name, b"SettingsInfo",
            "the frame still names the function"
        );
        assert_eq!(f.filename, b"<eve-offline ci trigger>");
        assert_eq!(f.stacksize, 3);
        // The last two instructions are the whole of today's blob.
        assert_eq!(
            &f.code[f.code.len() - 4..],
            &BUILD_EMPTY_MAP_RETURN,
            "the body must still end BUILD_MAP 0 ; RETURN_VALUE"
        );
        // consts = (None, path); names = ('execfile', 'globals').
        assert_eq!(f.consts.len(), 2);
        assert!(
            f.consts[0].is_empty(),
            "consts[0] is the None docstring slot"
        );
        assert_eq!(f.consts[1], scenario_path(SCENARIO).unwrap().as_bytes());
        assert_eq!(f.names, vec![b"execfile".to_vec(), b"globals".to_vec()]);
    }

    /// The `execfile(path, globals())` call, byte for byte against the
    /// client's own compiled copy of the same expression.
    ///
    /// `eve/devtools/script/svc_console.py`, `cmd_execfile` (`firstlineno`
    /// 130, source line 137), offsets 55..71 of its `co_code` **[V]**, read
    /// with `tools/pyc27.py`:
    ///
    /// ```text
    /// 74 06 00  LOAD_GLOBAL 6 (execfile)
    /// 7c 02 00  LOAD_FAST   2 (filename)
    /// 74 07 00  LOAD_GLOBAL 7 (globals)
    /// 83 00 00  CALL_FUNCTION 0
    /// 83 02 00  CALL_FUNCTION 2
    /// 01        POP_TOP
    /// ```
    ///
    /// This is the one assertion in this module that is not self-referential:
    /// it says our opcodes are the opcodes a real Python 2.7 compiler emitted
    /// for the expression we are claiming to emit. The only differences are
    /// the operand indices (ours are 0 and 1 because our `co_names` has two
    /// entries, not eight) and `LOAD_FAST` -> `LOAD_CONST`, because our path
    /// is baked in where the console's comes from a parsed argument.
    #[test]
    fn the_execfile_call_matches_the_clients_own_compiled_bytes() {
        const CLIENT: [u8; 16] = [
            0x74, 0x06, 0x00, // LOAD_GLOBAL  execfile
            0x7c, 0x02, 0x00, // LOAD_FAST    filename
            0x74, 0x07, 0x00, // LOAD_GLOBAL  globals
            0x83, 0x00, 0x00, // CALL_FUNCTION 0
            0x83, 0x02, 0x00, // CALL_FUNCTION 2
            0x01, // POP_TOP
        ];
        let ours = {
            let b = scenario_blob();
            // The call starts right after SETUP_EXCEPT, which is 3 bytes.
            parse(&b).code[3..19].to_vec()
        };

        // Same opcodes, in the same order, with the same shape.
        let opcodes = |s: &[u8]| -> Vec<u8> {
            let mut v = Vec::new();
            let mut i = 0;
            while i < s.len() {
                v.push(s[i]);
                i += if s[i] >= 90 { 3 } else { 1 };
            }
            v
        };
        assert_eq!(
            opcodes(&ours),
            opcodes(&CLIENT)
                .iter()
                .map(|&o| if o == 0x7c { 0x64 } else { o })
                .collect::<Vec<_>>(),
            "ours is the client's sequence with LOAD_FAST replaced by LOAD_CONST"
        );
        // The two CALL_FUNCTIONs must carry the client's own argument counts:
        // 0 for `globals()` and 2 for `execfile(path, globals())`.
        assert_eq!(&ours[9..12], &CLIENT[9..12], "CALL_FUNCTION 0");
        assert_eq!(&ours[12..15], &CLIENT[12..15], "CALL_FUNCTION 2");
    }

    /// The bare `except:` epilogue, byte for byte against four independent
    /// copies the client ships, all read with `tools/pyc27.py` **[V]**:
    /// `linecache.updatecache` (`firstlineno` 68), `pstats.load_stats` (118),
    /// `profile.trace_dispatch_c_call` (312) and `copy_reg._slotnames` (102).
    /// Every one of them has this exact eleven-byte sequence, and
    /// `profile.trace_dispatch_c_call` opens its `try` with `SETUP_EXCEPT 20`
    /// as well, because its body happens to be the same sixteen bytes long as
    /// ours.
    #[test]
    fn the_bare_except_matches_the_clients_own_compiled_bytes() {
        const CLIENT_EPILOGUE: [u8; 11] = [
            0x57, // POP_BLOCK
            0x6e, 0x07, 0x00, // JUMP_FORWARD 7  -> past the handler
            0x01, 0x01, 0x01, // POP_TOP x3      -> exc type, value, traceback
            0x6e, 0x01, 0x00, // JUMP_FORWARD 1  -> past END_FINALLY
            0x58, // END_FINALLY, unreachable for a bare except
        ];
        let b = scenario_blob();
        let code = parse(&b).code;
        assert_eq!(&code[0..3], &[0x79, 0x14, 0x00], "SETUP_EXCEPT 20");
        assert_eq!(&code[19..30], &CLIENT_EPILOGUE);
        assert_eq!(code.len(), 34);
    }

    /// Walk the emitted bytecode as an interpreter would: decode every
    /// instruction, follow both arms of the `try`, and recompute the value
    /// stack depth. This is what makes the hex golden below mean something --
    /// it checks the bytes *do* what the comment says rather than merely
    /// being the bytes we wrote down.
    ///
    /// The stack effects are CPython 2.7's own (`Python/compile.c`,
    /// `opcode_stack_effect`), and the `depth + 3` for a `SETUP_EXCEPT`
    /// target is `stackdepth_walk`'s rule for the three exception values the
    /// handler is entered with.
    #[test]
    fn the_scenario_bytecode_is_a_well_formed_instruction_stream() {
        let b = scenario_blob();
        let f = parse(&b);
        let code = &f.code;

        // Instruction boundaries, so a jump can be checked against them.
        let mut boundaries = std::collections::BTreeSet::new();
        let mut i = 0usize;
        while i < code.len() {
            boundaries.insert(i);
            i += if code[i] >= 90 { 3 } else { 1 };
        }
        assert_eq!(i, code.len(), "the last instruction must not overrun");

        let arg = |at: usize| u16::from_le_bytes([code[at + 1], code[at + 2]]) as usize;

        // (offset, depth) worklist. Every entry point is pushed explicitly,
        // so an unreachable instruction stays unvisited and is caught below.
        let mut work = vec![(0usize, 0i32)];
        let mut seen: std::collections::BTreeMap<usize, i32> = std::collections::BTreeMap::new();
        let mut max_depth = 0i32;
        let mut returned = false;

        while let Some((mut at, mut depth)) = work.pop() {
            loop {
                assert!(
                    boundaries.contains(&at),
                    "jump into the middle of an instruction at {at}"
                );
                if let Some(&had) = seen.get(&at) {
                    assert_eq!(had, depth, "two paths reach {at} with different depths");
                    break;
                }
                seen.insert(at, depth);
                let o = code[at];
                let size = if o >= 90 { 3 } else { 1 };
                match o {
                    121 => {
                        // SETUP_EXCEPT: pushes a block, not a value. Its
                        // handler is entered with three values on the stack.
                        let target = at + 3 + arg(at);
                        work.push((target, depth + 3));
                        max_depth = max_depth.max(depth + 3);
                    }
                    116 | 100 | 105 => depth += 1, // LOAD_GLOBAL/LOAD_CONST/BUILD_MAP
                    131 => {
                        // CALL_FUNCTION: pops na positional + 2*nk keyword
                        // values and the callable, pushes one result.
                        let a = arg(at);
                        let (na, nk) = (a & 0xff, a >> 8);
                        depth -= (na + 2 * nk) as i32;
                    }
                    1 => depth -= 1, // POP_TOP
                    87 => {}         // POP_BLOCK
                    110 => {
                        // JUMP_FORWARD: no fall-through.
                        let target = at + 3 + arg(at);
                        at = target;
                        continue;
                    }
                    83 => {
                        // RETURN_VALUE
                        assert_eq!(depth, 1, "RETURN_VALUE must find exactly one value");
                        returned = true;
                        break;
                    }
                    other => panic!("unexpected opcode {other} at {at}"),
                }
                assert!(depth >= 0, "stack underflow at {at}");
                max_depth = max_depth.max(depth);
                at += size;
            }
        }

        assert!(returned, "every path must reach RETURN_VALUE");
        assert_eq!(
            max_depth, f.stacksize,
            "co_stacksize must be the depth the stream actually reaches"
        );
        // END_FINALLY is the one instruction a bare `except` cannot reach;
        // CPython emits it anyway and so do we, so it must be the *only*
        // unvisited one.
        let unvisited: Vec<usize> = boundaries
            .iter()
            .copied()
            .filter(|o| !seen.contains_key(o))
            .collect();
        assert_eq!(unvisited, vec![29], "only END_FINALLY may be unreachable");
        assert_eq!(code[29], 88, "and offset 29 really is END_FINALLY");
    }

    /// Golden bytes for the CI-trigger blob with scenario `undock_smoke`,
    /// 204 bytes.
    ///
    /// **Verified by parsing them with `tools/pyc27.py`** in the analysis
    /// container -- the same read-only Python 2.7 marshal reader that reads
    /// the client's own `.pyc` files. It consumed exactly 204 of 204 bytes
    /// and disassembled to
    ///
    /// ```text
    /// argcount 0  nlocals 0  stacksize 3  flags 0x43
    ///   0  SETUP_EXCEPT   20
    ///   3  LOAD_GLOBAL    execfile
    ///   6  LOAD_CONST     'c:\\eve-logs\\client-ci\\undock_smoke.py'
    ///   9  LOAD_GLOBAL    globals
    ///  12  CALL_FUNCTION  0
    ///  15  CALL_FUNCTION  2
    ///  18  POP_TOP
    ///  19  POP_BLOCK
    ///  20  JUMP_FORWARD   7
    ///  23  POP_TOP
    ///  24  POP_TOP
    ///  25  POP_TOP
    ///  26  JUMP_FORWARD   1
    ///  29  END_FINALLY
    ///  30  BUILD_MAP      0
    ///  33  RETURN_VALUE
    /// consts (None, 'c:\\eve-logs\\client-ci\\undock_smoke.py')
    /// names  ('execfile', 'globals')
    /// filename '<eve-offline ci trigger>'  name 'SettingsInfo'
    /// firstlineno 1  lnotab ''
    /// ```
    ///
    /// If this fails, the encoder changed: re-run the parse (the recipe is in
    /// `docs/reference/code-push.md`, and `docs/lanes/ci-trigger.md` repeats it with
    /// this blob's hex) before touching the constant.
    const SCENARIO_GOLDEN: &str = "630000000000000000030000004300000073220000007914007400006401007401008300008302000157\
        6e07000101016e0100586900005328020000004e7325000000633a5c6576652d6c6f67735c636c69656e\
        742d63695c756e646f636b5f736d6f6b652e7079280200000073080000006578656366696c6573070000\
        00676c6f62616c7328000000002800000000280000000073180000003c6576652d6f66666c696e652063\
        6920747269676765723e730c00000053657474696e6773496e666f010000007300000000";

    #[test]
    fn scenario_blob_matches_the_verified_golden_bytes() {
        assert_eq!(
            hex(&scenario_blob()),
            SCENARIO_GOLDEN.replace(['\n', ' '], "")
        );
    }

    /// The three blobs must stay distinct, and in particular the scenario one
    /// must never be what a server without `--scenario` sends.
    #[test]
    fn the_scenario_blob_is_not_the_plain_one() {
        assert_ne!(scenario_blob(), settings_info_func());
        assert_ne!(scenario_blob(), handshake_func());
        // ... but it ends the same way, which is the point.
        let plain = settings_info_func();
        assert_eq!(&parse(&plain).code[..], &BUILD_EMPTY_MAP_RETURN);
        let s = scenario_blob();
        let c = parse(&s).code;
        assert_eq!(&c[c.len() - 4..], &BUILD_EMPTY_MAP_RETURN);
    }
}
