//! Industry: blueprints, jobs, and the facility list.
//!
//! New in `docs/archive/plan-docked-subsystems.md`, and created by the coordinator
//! rather than by the lane that fills it, because registering a service means
//! editing `DOMAINS` in the parent -- the last shared edit in this tree, spent
//! once, before the round rather than during it.
//!
//! # The four services, and how they were found
//!
//! Enumerated rather than waited for: `grep -ral` over `out/pyc/` for each
//! service name reaches **four** client files and no others
//! (`eve/client/script/industry/{blueprintSvc,facilitySvc,industrySvc}.pyc`
//! plus `eve/common/script/util/industryCommon.pyc`), so the industry remote
//! surface in V24.01 is closed at 21 call sites across four services:
//! `blueprintManager`, `facilityManager`, `industryManager` and
//! `industryMonitor`. The Industry window's own UI
//! (`eve/client/script/ui/shared/industry/`) issues **no** remote call of its
//! own -- it goes through the three client-side services -- which is why
//! fixing those three services' feeds fixes the whole window.
//!
//! **`ramProxy` does not exist in this build.** It was the pre-Crius name and
//! `grep -ral ramProxy out/pyc/` is empty; the brief's "verify rather than
//! assume" earned its keep here.
//!
//! # What the window asks for, in order
//!
//! From `industryWnd.py` and the four browser tabs. **Every line number in
//! this file is decoded from `co_lnotab`** -- via `findings/client-calls.tsv`
//! or `tools/pyc27.py` -- and not read off uncompyle6's output, whose line
//! numbers are its own reconstructed file's and not the client's.
//!
//! ```text
//!   blueprintSvc.py:37           -> blueprintManager.GetLimits
//!   browserFacilities.py:153,161 -> facilityManager.GetFacilities
//!                                   facilityManager.GetMaxActivityModifiers
//!   browserBlueprints.py:578     -> blueprintManager.GetBlueprintDataByOwner
//!   browserJobs.py:292           -> industryManager.GetJobsByOwner
//!   jobsStrip.py:166,245         -> industryManager.GetJobCounts
//! ```
//!
//! Every one of those five was unanswered, and three of them were live locks
//! rather than tracebacks -- see the arms.

use super::*;

/// How many blueprint rows the client may ask for in one query.
///
/// A query cap, not a limit on what a character can own. The real server's
/// value is not observable from the client; what matters is that it is a
/// positive integer, because the client slices with it.
const MAX_BLUEPRINT_RESULTS: i64 = 500;

/// `industry/const.pyc`: `ACTIVITIES`, the six activity ids the client knows.
///
/// ```text
///     MANUFACTURING = 1
///     RESEARCH_TIME = 3
///     RESEARCH_MATERIAL = 4
///     COPYING = 5
///     INVENTION = 8
///     REACTION = 9
/// ```
///
/// Both of this module's int-keyed dicts are keyed by these, and both are
/// deliberately **populated with zeroes rather than left empty** -- the
/// reasons are at the two arms, and they are different reasons.
const ACTIVITIES: [i64; 6] = [1, 3, 4, 5, 8, 9];

/// A dict whose keys are **integers**, which `pyutil::pydict` cannot build.
///
/// `pydict` takes `&str` keys because almost every settings blob the client
/// reads is string-keyed. Industry's two caches are keyed by activity id, and
/// `{'1': 0}` is not `{1: 0}` to `dict.get(activityID, 0)` -- it would miss
/// every key and quietly return the default, which is the one failure mode
/// with no symptom at all.
///
/// The `(value, key)` ordering is the marshal dict's, not a typo: see
/// `PyValue::Dict`.
fn int_keyed_dict(pairs: Vec<(i64, PyValue)>) -> PyValue {
    PyValue::Dict(
        pairs
            .into_iter()
            .map(|(k, v)| (v, PyValue::Int(k)))
            .collect(),
    )
}

/// `eveexceptions.UserError`, with a message id the client itself raises.
///
/// Three industry reads are specified by what they *throw* for an id the
/// server does not know, and the client is written for it: `GetFacility`'s
/// two call sites that matter are both inside `except UserError:` blocks
/// (`facilitySvc.py(57)` and `browserBlueprints.py(341)`), and
/// `blueprintSvc.GetBlueprint` falls back to the blueprint *type* on
/// `UserError`. Answering those with a value is worse than answering with an
/// exception, which is the §4 "fifth kind of empty".
/// One facility as the eleven keys `industryCommon.Facility(data)` reads.
///
/// **Three of the eleven raise on the obvious empty**, and all three raise
/// inside `_PrimeFacility` -- which `_PrimeFacilities` calls in a loop whose
/// `loadedRegion` latch is the statement *after* it. So a raise here is the
/// live lock this module's header describes, not a blank row:
///
/// 1. `tax` must be a **number**. `Job._get_errors` opens
///    `if self.facility.tax is None:` and, for a facility whose category is
///    not `categoryStructure` (65), falls through to `FACILITY_DENIED`. A
///    station is category 3, so `None` disables the submit button for ever.
/// 2. `globalModifiers['system']` must hold a float for **every** activity id
///    listed in `activities`. The client does
///    `if globalModifiers['system'].get(a) not in (0.0, 1.0): ... [a]`, so
///    `{}` makes `.get` return `None`, `None not in (0.0, 1.0)` is true, and
///    the next line is a `KeyError`. `globalModifiers['faction']` must be
///    exactly `1.0` or a faction cost modifier is appended.
/// 3. `rigModifiers` must be **falsy**: `GetFreshFacility` deletes and
///    re-fetches any cached facility that has one, on every job creation.
///
/// `activities`' values are each unpacked as a 6-tuple whose first three
/// elements are sequences of 5-tuples, and the 5-tuple's last element goes
/// through `industry.Reference`, which raises `ValueError` for anything
/// outside its ten codes. Empty sequences exercise none of that. Empty
/// `categories`/`groups`/`invTypes` also leaves `activities[a]['blueprints']`
/// empty, which makes the `FACILITY_TYPE` check falsy -- "accepts any
/// blueprint", which is what an NPC station does.
fn facility_row(f: &crate::industry::Facility) -> PyValue {
    let per_activity = |v: f64| {
        int_keyed_dict(
            crate::industry::ACTIVITIES
                .iter()
                .map(|&a| (a, PyValue::Real(v)))
                .collect(),
        )
    };
    let activities = crate::industry::ACTIVITIES
        .iter()
        .map(|&a| {
            (
                a,
                PyValue::Tuple(vec![
                    PyValue::List(vec![]), // timeModifiers,     5-tuples
                    PyValue::List(vec![]), // materialModifiers, 5-tuples
                    PyValue::List(vec![]), // costModifiers,     5-tuples
                    PyValue::List(vec![]), // categories
                    PyValue::List(vec![]), // groups
                    PyValue::List(vec![]), // invTypes
                ]),
            )
        })
        .collect();
    pydict(vec![
        ("facilityID", PyValue::Int(f.facility_id)),
        ("typeID", PyValue::Int(f.type_id)),
        ("ownerID", PyValue::Int(f.owner_id)),
        // Trap 1.
        ("tax", PyValue::Real(f.tax.unwrap_or(0.0))),
        ("solarSystemID", PyValue::Int(f.solar_system_id)),
        ("online", PyValue::Bool(f.online)),
        ("serviceAccess", PyValue::Dict(vec![])),
        ("sccTaxModifier", PyValue::Real(f.scc_tax_modifier)),
        // Trap 3.
        ("rigModifiers", PyValue::List(vec![])),
        // Trap 2.
        (
            "globalModifiers",
            pydict(vec![
                ("faction", PyValue::Real(1.0)),
                ("system", per_activity(1.0)),
            ]),
        ),
        ("activities", int_keyed_dict(activities)),
    ])
}

/// One blueprint as the fifteen attributes `BlueprintInstance(data)` reads.
///
/// **`quantity` is `-1` and the sign is the message.** The client derives
/// `singleton = data.quantity < 0`, `original = data.quantity != -2` and
/// `quantity = max(data.quantity, 1)` all from this one field, so `-1` means
/// "one, singleton, original", `-2` means "a copy", and a positive number
/// means a stack of non-singletons -- which an original cannot be.
/// `industry::BlueprintItem::wire_quantity` is that rule, with the tests.
///
/// `jobID` is the client's **only** signal that a blueprint is installed:
/// `Job._get_errors` raises `BLUEPRINT_INSTALLED` off it. We do not move the
/// blueprint item itself while a job holds it.
///
/// `isImpounded` is read and never acted on by anything we serve.
///
/// **`locationTypeID` is the station's type when the blueprint sits in the
/// facility, never 0.** The client's `Location.__eq__` compares `itemID`,
/// `typeID`, `ownerID` and `flagID`, and `blueprint_location` is matched
/// against `GetFacilityLocations`' row (`location_row`), so a 0 here made
/// every blueprint read `INVALID_BLUEPRINT_LOCATION`. A blueprint in some
/// other station keeps 0: the station table has no type column to look up
/// (a known gap).
fn blueprint_row(
    reg: &Registry,
    b: &crate::industry::BlueprintItem,
    facility: Option<&crate::industry::Facility>,
) -> PyValue {
    let system = facility.map_or(0, |f| f.solar_system_id);
    let location_type_id = match facility {
        Some(f) if b.location_id == f.facility_id => f.type_id,
        _ => b.location_type_id,
    };
    // T3-B: an ancient relic stack. Its units are taken at install, so it is
    // never "in a job" (`jobID` None), and its quantity is the stack size.
    let relic = reg
        .content()
        .get::<crate::industry::Table>()
        .is_relic(b.type_id);
    keyval(vec![
        ("typeID", PyValue::Int(b.type_id)),
        ("itemID", PyValue::Int(b.item_id)),
        ("timeEfficiency", PyValue::Int(b.time_efficiency)),
        ("materialEfficiency", PyValue::Int(b.material_efficiency)),
        ("runs", PyValue::Int(b.runs_remaining)),
        (
            "quantity",
            PyValue::Int(b.wire_quantity_in(reg.content().get())),
        ),
        ("locationID", PyValue::Int(b.location_id)),
        ("locationTypeID", PyValue::Int(location_type_id)),
        ("locationFlagID", PyValue::Int(b.flag_id)),
        ("flagID", PyValue::Int(b.flag_id)),
        // The facility a blueprint can be used at is the station it is
        // sitting in, so these two are the same id -- true for a station
        // hangar and only for one.
        ("facilityID", PyValue::Int(b.location_id)),
        ("ownerID", PyValue::Int(b.owner_id)),
        (
            "jobID",
            reg.jobs()
                .job_for_blueprint(b.item_id)
                .filter(|_| !relic)
                .map_or(PyValue::None, |j| PyValue::Int(j.job_id)),
        ),
        ("isImpounded", PyValue::Bool(false)),
        ("solarSystemID", PyValue::Int(system)),
    ])
}

/// One job as the 28 attributes `industryCommon.JobData(data, blueprint)`
/// reads, in the order the lane enumerated them from the bytecode.
///
/// Three traps, all of them about *types* rather than values:
///
/// - **`startDate`/`endDate`/`pauseDate` are blue FILETIMEs**, all three put
///   through `BlueToDate` -> `blue.os.GetTimeParts`, which raises on a small
///   integer. `pauseDate` is the only one that may be `None`, because
///   `JobData` guards it with `if data.pauseDate else None`.
/// - **`timeInSeconds` is seconds**, not a FILETIME:
///   `job.time = datetime.timedelta(seconds=data.timeInSeconds)`.
/// - **`status` is what we store, not what the client shows.**
///   `industryCommon.JobStatus` turns Installed into Ready itself once
///   `endDate < now`, and `_PollJobCompletion` re-scatters `OnIndustryJob`
///   locally once a second for exactly those jobs. `Job::status_at` is the
///   same function if an arm ever needs the answer.
fn job_row(job: &crate::industry::Job) -> PyValue {
    keyval(vec![
        ("activityID", PyValue::Int(job.activity_id)),
        ("jobID", PyValue::Int(job.job_id)),
        ("blueprintID", PyValue::Int(job.blueprint_id)),
        ("blueprintTypeID", PyValue::Int(job.blueprint_type_id)),
        (
            "blueprintLocationID",
            PyValue::Int(job.blueprint_location_id),
        ),
        (
            "blueprintLocationFlagID",
            PyValue::Int(job.blueprint_location_flag_id),
        ),
        ("blueprintCopy", PyValue::Bool(job.blueprint_copy)),
        ("facilityID", PyValue::Int(job.facility_id)),
        ("ownerID", PyValue::Int(job.owner_id)),
        ("status", PyValue::Int(job.status.code())),
        ("installerID", PyValue::Int(job.installer_id)),
        (
            "completedCharacterID",
            job.completed_character_id
                .map_or(PyValue::None, PyValue::Int),
        ),
        ("solarSystemID", PyValue::Int(job.solar_system_id)),
        ("stationID", PyValue::Int(job.station_id)),
        ("startDate", PyValue::Int(job.start_date)),
        ("endDate", PyValue::Int(job.end_date)),
        (
            "pauseDate",
            job.pause_date.map_or(PyValue::None, PyValue::Int),
        ),
        ("runs", PyValue::Int(job.runs)),
        ("licensedRuns", PyValue::Int(job.licensed_runs)),
        ("successfulRuns", PyValue::Int(job.successful_runs)),
        ("cost", PyValue::Int(job.cost)),
        ("timeInSeconds", PyValue::Int(job.time_in_seconds)),
        // `JobData` is `data.probability if data.probability is not None
        // else 1`, so a number is always safe and 1.0 is "certain".
        ("probability", PyValue::Real(job.probability)),
        ("productTypeID", PyValue::Int(job.product_type_id)),
        (
            "optionalTypeID",
            job.optional_type_id.map_or(PyValue::None, PyValue::Int),
        ),
        (
            "optionalTypeID2",
            job.optional_type_id2.map_or(PyValue::None, PyValue::Int),
        ),
        ("outputLocationID", PyValue::Int(job.output_location_id)),
        ("outputFlagID", PyValue::Int(job.output_flag_id)),
    ])
}

/// One `industry.job.Location`, and it must be that class rather than a
/// `KeyVal`.
///
/// `industryCommon.MatchLocation` returns `copy.copy(location)` and
/// `Job._get_errors` then asks `if self.inputLocation not in self.locations`.
/// `industry.Location.__eq__` compares `itemID`/`typeID`/`ownerID`/`flagID`,
/// so a copy equals its original. `utillib.KeyVal` is an **old-style** class
/// with no `__eq__`, and Python 2 compares old-style instances by *address*
/// -- so the copy would never equal the original and every install would
/// raise `INVALID_INPUT_LOCATION`. `industry.job.Location` is one of only
/// three `industry.*` names in the client's `globalsWhitelist`, and this is
/// what it is there for.
///
/// **The tag is `0x23`, and that is [V] rather than a guess.** `Location` is
/// new-style with no `__reduce__`, so `object.__reduce_ex__(2)` yields the
/// `copyreg.__newobj__` form -- and `eve_marshal::ObjectForm` records that
/// the client's own `WriteObject` picks between the two tags at
/// `0x1802e02a0` **by testing whether the reduce callable's `__name__` is
/// `"__newobj__"`**. So the writer emits `0x23` for exactly this case. The
/// two readers agree from the other side (`docs/reference/object-wrapper.md` §§2-3):
/// `0x23` reconstructs `header[0][0].__new__(*header[0])` while `0x22` does
/// `header[0](*header[1])`, the generic `(callable, args)` reduce.
///
/// The header is therefore `((Location,), state)`: the class stays element 0
/// of the **inner** tuple, which is the part of this format easiest to get
/// backwards, and the state is header element 1 rather than a field of its
/// own.
///
/// `canView`/`canTake` are both true for your own hangar, and are read by
/// the `BLUEPRINT_ACCESS` and `INPUT_ACCESS` checks.
fn location_row(item_id: i64, type_id: i64, owner_id: i64, flag_id: i64) -> PyValue {
    PyValue::ObjectEx {
        form: eve_marshal::ObjectForm::New,
        header: Box::new(PyValue::Tuple(vec![
            PyValue::Tuple(vec![PyValue::Global(b"industry.job.Location".to_vec())]),
            pydict(vec![
                ("itemID", PyValue::Int(item_id)),
                ("typeID", PyValue::Int(type_id)),
                ("ownerID", PyValue::Int(owner_id)),
                ("flagID", PyValue::Int(flag_id)),
                ("canView", PyValue::Bool(true)),
                ("canTake", PyValue::Bool(true)),
            ]),
        ])),
        list: vec![],
        dict: vec![],
    }
}

fn industry_user_error(msg: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(msg, vec![]))
}

/// One `industry.const.Error` member, as the `0x22` reduce form.
///
/// `industry/enum.pyc` is a backported enum34 whose `__reduce_ex__` is
/// `(self.__class__, self.__getnewargs__())`, and `__getnewargs__` is
/// `(self._value_,)` -- the pickle *callable + args* form, so blue.marshal
/// writes tag `0x22` with header `(Global, (code,))`, no append stream and no
/// dict stream. On load the client calls `industry.const.Error(code)`, which
/// goes through `Enum.__new__` -> `cls._value2member_map_[value]` and returns
/// the canonical member, so `.value`, `.name` and `==` by identity all work
/// on the far side. **[V]**
///
/// **An unknown code raises `ValueError('%s is not a valid Error')` inside
/// the unmarshal**, before any of our code runs on the client -- which is why
/// [`crate::industry::Refusal::code`] only ever emits one of CCP's forty and
/// has a test that says so.
fn industry_error(code: i64) -> PyValue {
    PyValue::ObjectEx {
        form: eve_marshal::ObjectForm::Call,
        header: Box::new(PyValue::Tuple(vec![
            PyValue::Global(b"industry.const.Error".to_vec()),
            PyValue::Tuple(vec![PyValue::Int(code)]),
        ])),
        list: vec![],
        dict: vec![],
    }
}

/// `UserError('IndustryValidationError', {'errors': [(Error, args), ...]})`.
///
/// **Every link in the chain constrains the shape, and all four are [V]:**
///
/// ```text
/// submitButton.SubmitButton.ClickFunc(185)
///     except UserError, e: if getattr(e, 'msg', None) == 'IndustryValidationError':
///         self.tooltipErrors = e.args[1]['errors']
/// industryTooltips.SubmitButtonTooltipPanel.Reconstruct(448)
///     if not errors: return
///     errors = {error[0].value: error for error in errors}.values()
///     for error, errorArgs in errors:          # UNPACK_SEQUENCE 2
///         AddErrorRow(panel, error, errorArgs)
/// industryTooltips.AddErrorRow(164)
///     text = industryCommon.GetErrorLabel(error, *errorArgs)   # CALL_FUNCTION_VAR
///     if not text: text = error.name
/// ```
///
/// So each element is a **2-sequence exactly** -- a bare member raises in the
/// unpack and so does a 3-tuple -- element 0 must carry `.value` (and be
/// hashable, since it keys the dict comprehension) and `.name`, and element 1
/// must be **iterable**, because it is splatted into a call.
///
/// `args` is the empty tuple for every refusal we can emit, and that is a
/// **correction to `docs/lanes/industry.md` §6.6**, which named four branches
/// of `GetErrorLabel` as arg-consuming. In build 3619879 exactly **one**
/// branch reads `args`: `STANDINGS_RESTRICTION`, which takes `args[0]` as a
/// mapping and passes it as `**kwargs`. `MISSING_MATERIAL`, `MISSING_SKILL`,
/// `SLOTS_FULL` and `FACILITY_DISTANCE` are each a bare
/// `GetByLabel(path)` with `CALL_FUNCTION 1`, and `RUN_LENGTH` computes its
/// own `numDays` from `industry.MAX_RUN_LENGTH`. Disassembled from
/// `eve/common/script/util/industryCommon.pyc`; no refusal of ours is
/// `STANDINGS_RESTRICTION`.
///
/// `IndustryValidationError` is dialog **4625**, `notify`/`logline` -- the
/// player gets a log line and the tooltip, not a modal. Checked against
/// `findings/user-errors.tsv`, which is not optional: a key with no row
/// renders as `ErrMessageNotFound`, whose `dialogType` is **fatal**
/// (`docs/re/errors.md` §2.4).
///
/// An empty slice would be a tooltip that silently draws nothing, per the
/// `if not errors: return` above. Every caller passes exactly one refusal,
/// which is all `Jobs::plan_install` can produce; the slice is here because
/// the client's shape is a list and inventing a different one later should
/// not mean rewriting this.
fn validation_error(refusals: &[crate::industry::Refusal]) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "IndustryValidationError",
        crate::pyutil::pypairs(vec![(
            "errors",
            PyValue::List(
                refusals
                    .iter()
                    .map(|r| PyValue::Tuple(vec![industry_error(r.code()), PyValue::Tuple(vec![])]))
                    .collect(),
            ),
        )]),
    ))
}

/// An integer out of a marshal dict, by key.
///
/// **Strict.** A float id is refused rather than rounded: the client sends
/// `Job.dump()`, whose ids are all ints, and the one float in that dict
/// (`time`, which is `total_seconds()`) is a field we recompute rather than
/// read. Rounding a float here would turn a malformed request into a
/// plausible one.
fn dict_int(v: &PyValue, key: &[u8]) -> Option<i64> {
    match crate::pyutil::dict_get(v, key) {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    }
}

/// An install request's `inputLocation` / `outputLocation`, as
/// `(itemID, flagID)`.
///
/// `industry.job.Location` is new-style (`industry.Base(object)`) with no
/// `__reduce__`, so it arrives as an ObjectEx carrying
/// `{itemID, typeID, ownerID, flagID, canView, canTake}` -- but **which tag,
/// and the exact header, is the one thing in this subsystem that only a
/// capture settles** (`docs/lanes/industry.md` §8). The client's `WriteObject`
/// picks between `0x22` and `0x23` by testing whether the reduce callable's
/// `__name__` is `"__newobj__"`, which says `0x23` for a plain new-style
/// class -- but that is our reading of the *writer*, and the thing on the
/// wire here was written by the client, not by us.
///
/// So this reads the state out of whichever form arrives:
///
/// - `0x23` `New`: header `((cls, *args), state)`
/// - `0x22` `Call`: header `(callable, args)` or `(callable, args, state)`
/// - the `dict` stream, where `__setitem__` items land -- and which is
///   **`(key, value)`**, the opposite order to [`PyValue::Dict`]
/// - a plain dict or a `utillib.KeyVal`, which is what a test or a script
///   sends
///
/// The header search takes the **last** dict in the tuple, which is the state
/// in both forms.
///
/// A location we cannot read is **refused, not guessed**. The flag decides
/// which hangar division the output lands in, so a default would quietly put
/// a player's work somewhere they did not ask for.
fn location_of(v: &PyValue) -> Option<(i64, i64)> {
    match v {
        PyValue::ObjectEx { header, dict, .. } => {
            if let (Some(item_id), Some(flag_id)) =
                (objectex_int(dict, b"itemID"), objectex_int(dict, b"flagID"))
            {
                return Some((item_id, flag_id));
            }
            let PyValue::Tuple(parts) = header.as_ref() else {
                return None;
            };
            let state = parts.iter().rev().find(|p| matches!(p, PyValue::Dict(_)))?;
            Some((dict_int(state, b"itemID")?, dict_int(state, b"flagID")?))
        }
        PyValue::Instance { state, .. } => {
            Some((dict_int(state, b"itemID")?, dict_int(state, b"flagID")?))
        }
        PyValue::Dict(_) => Some((dict_int(v, b"itemID")?, dict_int(v, b"flagID")?)),
        _ => None,
    }
}

/// One integer out of an ObjectEx's `__setitem__` stream, which is
/// **`(key, value)`** and not `(value, key)`.
fn objectex_int(dict: &[(PyValue, PyValue)], key: &[u8]) -> Option<i64> {
    dict.iter()
        .find(|(k, _)| crate::pyutil::as_bytes(k) == Some(key))
        .and_then(|(_, v)| match v {
            PyValue::Int(n) => Some(*n),
            _ => None,
        })
}

/// `InstallJob`'s one argument, which is `Job.dump()`.
///
/// Seventeen keys go out; **five** decide what is built, and the rest are the
/// client's own arithmetic, which we recompute rather than trust.
/// `Job._get_errors` on the real server compares `materials`, `cost`, `tax`
/// and `time` against its own figures and raises `MISMATCH_*` (25-28) when
/// they differ -- a consistency check between two implementations of the same
/// formulas. We have one implementation, so there is nothing to compare to;
/// `crate::industry` computes all four and the request's copies are ignored.
/// That is a deliberate difference from CCP and it is the safe direction: a
/// client that lies about the cost is charged ours, not its own.
///
/// `Job.create`'s `required` set is twelve keys and it raises
/// `RuntimeError('Invalid Request: ', ...)` for a missing one; ours refuses
/// with the client's own `IndustryJobInstallError` instead, because a
/// `RuntimeError` across the wire is a traceback dialog.
fn install_request(arg: Option<&PyValue>) -> Result<crate::industry::InstallRequest, ServiceError> {
    let Some(request) = arg else {
        return Err(industry_user_error("IndustryJobInstallError"));
    };
    let (Some(activity_id), Some(blueprint_id), Some(runs)) = (
        dict_int(request, b"activityID"),
        dict_int(request, b"blueprintID"),
        dict_int(request, b"runs"),
    ) else {
        return Err(industry_user_error("IndustryJobInstallError"));
    };
    let input = crate::pyutil::dict_get(request, b"inputLocation")
        .and_then(location_of)
        .ok_or_else(|| validation_error(&[crate::industry::Refusal::MissingInputLocation]))?;
    let output = crate::pyutil::dict_get(request, b"outputLocation")
        .and_then(location_of)
        .ok_or_else(|| validation_error(&[crate::industry::Refusal::MissingOutputLocation]))?;
    Ok(crate::industry::InstallRequest {
        activity_id,
        blueprint_id,
        runs,
        input_location_id: input.0,
        input_flag_id: input.1,
        output_location_id: output.0,
        output_flag_id: output.1,
        // Two of `Job.dump()`'s 17 keys that only science reads: a copy's
        // licence, and which T2 blueprint an invention makes. Both may be
        // `None` on the wire -- `licensedRuns` for every activity but
        // copying, `productTypeID` for a single-product invention -- and the
        // model reads an absent one exactly as `Job` does.
        licensed_runs: dict_int(request, b"licensedRuns").unwrap_or(0),
        product_type_id: dict_int(request, b"productTypeID"),
        decryptor: None,
    })
}

/// The type ids in a request's `materials` dict, `{typeID: quantity}`.
///
/// A marshal dict entry is `(value, key)` (`pyutil::pairs_get`), so the type
/// is the **second** half. An optional material the player left empty has
/// no type (`None`) and is skipped.
fn material_types(request: &PyValue) -> Vec<i64> {
    match crate::pyutil::dict_get(request, b"materials") {
        Some(PyValue::Dict(entries)) => entries
            .iter()
            .filter_map(|(_, k)| match k {
                PyValue::Int(t) => Some(*t),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `OnBlueprintsUpdated(ownerID)` -- **one positional argument**. **[V]**:
/// `blueprintSvc.__notifyevents__` lists it, and the handler drops both
/// memoized blueprint lists and `bpSummaries[ownerID]`, then scatters
/// `OnBlueprintReload` so the open window re-reads.
///
/// **Why it is needed at all**: `OnIndustryJob` only patches `jobID` onto
/// the cached blueprint (`_UpdateOnIndustryJobChange`); it never re-reads
/// one. So without this a researched original would show its old ME until
/// the 15 s memo expired, and a delivered copy would not appear at all.
/// Pushed after a science delivery and after an install that spent a copy's
/// runs -- the two things that change a blueprint row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BlueprintsUpdated {
    pub owner_id: i64,
}

impl crate::notify::Push for BlueprintsUpdated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnBlueprintsUpdated", vec![PyValue::Int(self.owner_id)])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.owner_id])
    }
}

/// `OnIndustryMaterials(jobID, materials)` -- `industrySvc.py(462)`. **[V]**
///
/// `jobID` is the **monitor id** `ConnectJob` handed out (the client compares
/// it with `job.monitorID`), and `materials` is `{typeID: quantity}`, which the
/// handler stores as `job.available`. Marshal dict entries are
/// `(value, key)`, so the quantity comes first.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct IndustryMaterials {
    pub to: i64,
    pub monitor_id: i64,
    pub materials: Vec<(i64, i64)>,
}

impl crate::notify::Push for IndustryMaterials {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnIndustryMaterials",
            vec![
                PyValue::Int(self.monitor_id),
                available_dict(&self.materials),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `{typeID: quantity}` as the wire wants it: entries are `(value, key)`.
fn available_dict(materials: &[(i64, i64)]) -> PyValue {
    PyValue::Dict(
        materials
            .iter()
            .map(|&(type_id, qty)| (PyValue::Int(qty), PyValue::Int(type_id)))
            .collect(),
    )
}

/// One open Industry window's watch on a hangar division.
#[derive(Debug, Clone)]
struct Monitor {
    id: i64,
    actor: i64,
    location: i64,
    flag: i64,
    /// What was last told to the client, sorted by type id, so a change that
    /// leaves the answer the same sends nothing.
    last: Vec<(i64, i64)>,
}

/// The material monitors `industryMonitor.ConnectJob` registers.
///
/// **One per pilot.** The client keeps a single `industrySvc.monitoring`
/// weakref and re-connects whenever the input location changes, so a second
/// `ConnectJob` replaces the first and the table cannot outgrow the number of
/// pilots. Transient: a restart drops them, and the window reconnects.
#[derive(Debug, Default)]
pub(crate) struct Monitors {
    next: i64,
    list: Vec<Monitor>,
}

/// What `actor` holds in one division, `{typeID: quantity}` sorted by type.
///
/// The same rule `consume_plan` draws from -- owner, location and flag, a
/// singleton counting as one -- so the window never promises what the install
/// would then refuse.
fn on_hand(reg: &Registry, actor: i64, location: i64, flag: i64) -> Vec<(i64, i64)> {
    let mut sums: std::collections::BTreeMap<i64, i64> = std::collections::BTreeMap::new();
    for item in reg.store().items() {
        if item.owner_id != actor || item.location_id != location || item.flag_id != flag {
            continue;
        }
        let stack = crate::industry::Stack::of(item);
        *sums.entry(stack.type_id).or_insert(0) += stack.quantity.max(0);
    }
    sums.into_iter().filter(|&(_, q)| q > 0).collect()
}

/// `ConnectJob`: watch the job's input division and answer what is in it.
/// An input location we cannot read is answered as nothing and not watched.
fn connect_monitor(reg: &mut Registry, actor: i64, dump: Option<&PyValue>) -> PyValue {
    reg.industry_monitors.list.retain(|m| m.actor != actor);
    let Some((location, flag)) = dump
        .and_then(|d| crate::pyutil::dict_get(d, b"inputLocation"))
        .and_then(location_of)
    else {
        return PyValue::Tuple(vec![PyValue::None, PyValue::Dict(vec![])]);
    };
    let last = on_hand(reg, actor, location, flag);
    reg.industry_monitors.next += 1;
    let id = reg.industry_monitors.next;
    let answer = available_dict(&last);
    reg.industry_monitors.list.push(Monitor {
        id,
        actor,
        location,
        flag,
        last,
    });
    PyValue::Tuple(vec![PyValue::Int(id), answer])
}

/// After items moved for `owners`: tell each watching window whose division
/// changed. Called from `Registry::apply_items`, the one road item changes
/// take to the client.
pub(super) fn refresh_monitors(reg: &mut Registry, owners: &[i64]) {
    if reg.industry_monitors.list.is_empty() {
        return;
    }
    let mut changed = Vec::new();
    for (i, m) in reg.industry_monitors.list.iter().enumerate() {
        if !owners.contains(&m.actor) {
            continue;
        }
        let now = on_hand(reg, m.actor, m.location, m.flag);
        if now != m.last {
            changed.push((i, now));
        }
    }
    for (i, now) in changed {
        let m = &mut reg.industry_monitors.list[i];
        m.last = now.clone();
        let (to, monitor_id) = (m.actor, m.id);
        reg.push(IndustryMaterials {
            to,
            monitor_id,
            materials: now,
        });
    }
}

/// The job this call names, if the caller owns it.
///
/// `IndustryJobNotFound` (4614, `notify`/`logline`) for an id we do not hold
/// and `IndustryJobAccessDenied` (4613, `info`/`modal` -- the id
/// `industryCommon.AssertJobAccess` raises itself) for one belonging to
/// somebody else. The same pair `GetJob` answers with, so a job cannot be
/// readable by one route and writable by another.
fn owned_job(
    reg: &Registry,
    actor: i64,
    job_id: i64,
) -> Result<crate::industry::Job, ServiceError> {
    let Some(job) = reg.jobs().get(job_id) else {
        return Err(industry_user_error("IndustryJobNotFound"));
    };
    if job.owner_id != actor {
        return Err(industry_user_error("IndustryJobAccessDenied"));
    }
    Ok(*job)
}

/// Deliver one finished job, or say why not.
///
/// The shared body of `CompleteJob` and `CompleteManyJobs`, so that the one
/// that answers a player's button and the one that answers "deliver all"
/// cannot drift apart.
///
/// **The three refusals are checked here rather than decoded out of a
/// `Refusal`.** `Jobs::plan_delivery` signals all three through variants
/// whose names mean something else -- `MissingBlueprint` for a job it cannot
/// find, `IncompatibleActivity` for one already collected, `InvalidRuns` for
/// one still running -- so mapping them back would be inventing a meaning the
/// model does not carry. The model's refusals stay the belt to this brace.
fn deliver_one(reg: &mut Registry, actor: i64, job_id: i64, now: i64) -> Result<(), ServiceError> {
    let job = owned_job(reg, actor, job_id)?;
    if job.completed() {
        return Err(industry_user_error("IndustryJobAlreadyCompleted"));
    }
    if !job.ready_at(now) {
        return Err(industry_user_error("IndustryJobNotReady"));
    }
    let (delivery, done) = reg
        .jobs()
        .plan_delivery(reg.content().get(), reg.content(), job_id, now)
        .map_err(|refusal| validation_error(&[refusal]))?;
    // `IndustryJobNoProduct` (4630) is the client's own id for a delivery
    // that yields nothing, and an `EditError` here means exactly that: the
    // output type is one `item::Edit::Create` refuses.
    reg.deliver_job(delivery, done)
        .map_err(|_| industry_user_error("IndustryJobNoProduct"))?;
    // Every activity but manufacturing changes a blueprint row: research
    // the original's ME or TE, copying and invention by minting copies. A
    // reaction (MN-A) delivers an item stack and leaves its formula alone.
    if !matches!(
        job.activity_id,
        crate::industry::MANUFACTURING | crate::industry::REACTION
    ) {
        reg.push(BlueprintsUpdated {
            owner_id: job.owner_id,
        });
    }
    Ok(())
}

pub(crate) const SERVICES: &[&str] = &[
    "blueprintManager",
    "facilityManager",
    "industryManager",
    "industryMonitor",
];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        "blueprintManager" => match req.method {
            // **The call that killed the industry tab for a whole session,
            // not just for one click.**
            //
            // ```text
            //     def Run(self, *args, **kwargs):
            //         Service.Run(self, *args, **kwargs)
            //         self.blueprintManager = sm.RemoteSvc('blueprintManager')
            //         self.blueprintLimit = self.blueprintManager.GetLimits()['maxBlueprintResults']
            // ```
            //
            // One key, read by name. Our `None` was
            // `TypeError: 'NoneType' object is not subscriptable` -- thrown
            // inside `Run`, under `serviceManager.StartServiceRun`, which
            // stores the exception and re-raises it from **every** later
            // `sm.GetService('blueprintSvc')`. So the service is poisoned for
            // the rest of the session and every blueprint feature stays dead
            // after the first attempt. Same mechanism as the `skillMgr2`
            // note on `Registry::OBJECT_RETURNING`.
            //
            // A dict rather than a `KeyVal`: the client subscripts it with a
            // string, which both satisfy, and a dict is what the real server
            // sends for a plain settings blob.
            //
            // **It must be built with `pydict`.** The first version of this
            // arm wrote the pair the obvious way round --
            // `PyValue::Dict(vec![(bstr("maxBlueprintResults"), Int(500))])`
            // -- and a marshal dict entry is **`(value, key)`**, so that is
            // the dict `{500: 'maxBlueprintResults'}`. The subscript would
            // have raised `KeyError` in the same spot as the `None` did and
            // poisoned the service in exactly the same way, which is to say
            // the fix would have changed the exception's name and nothing
            // else. `pydict` exists so that this inversion is written down
            // once; `the_blueprint_limit_is_a_dict_with_one_named_integer`
            // now looks the key up the way the wire does.
            "GetLimits" => Ok(pydict(vec![(
                "maxBlueprintResults",
                PyValue::Int(MAX_BLUEPRINT_RESULTS),
            )])),
            // The blueprint list, and a **2-sequence whose halves are
            // different container families**. `blueprintSvc.py(183, 201)`:
            //
            // ```text
            //     bps, bpCounts = self.blueprintManager.GetBlueprintDataByOwner(ownerID, facilityID)
            //     bps = self._AttachFacilities(bps)          # for data in blueprints:
            //     ...
            //     self.bpSummaries[ownerID] = bpCounts
            //     return sum(n for n in bpCounts.itervalues())
            // ```
            //
            // So `UNPACK_SEQUENCE 2` first -- arity is not negotiable and our
            // `None` raised there, before either half was ever looked at --
            // then element 0 is **iterated** and element 1 needs
            // **`.itervalues()`**. A list and a dict respectively; swapping
            // them trades a `TypeError` for an `AttributeError` one frame
            // later. `sum()` over an empty dict is `0`, which is the true
            // number of blueprints we have.
            //
            // Both halves may be empty without latching anything: the one
            // `if not bps:` in `GetOwnerBlueprintsAtFacility` re-issues the
            // call *once* inside a `@Memoize(15)`, so the cost of an empty
            // answer is one extra call per 15 s, not a loop.
            //
            // The row shape for when there is something to send is pinned by
            // `industryCommon.BlueprintInstance(data)`, which reads, by
            // **attribute**: `typeID`, `itemID`, `timeEfficiency`,
            // `materialEfficiency`, `runs`, `quantity`, `locationID`,
            // `locationTypeID`, `locationFlagID`, `flagID`, `facilityID`,
            // `ownerID`, `jobID`, `isImpounded`, `solarSystemID`. It copies
            // them onto a `cfg.blueprints` row, so the rows we send are never
            // written back to and a packed `DbRow` is safe here.
            // `GetAllBlueprintsForOwner` does `UNPACK_SEQUENCE 2`, iterates
            // element 0 into `BlueprintInstance` -- **fifteen attribute
            // reads**, so `KeyVal` and never a dict -- and sums element 1
            // with `sum(n for n in bpCounts.itervalues())`.
            "GetBlueprintDataByOwner" => {
                let owner = opt_id(req.args.first()).or(req.actor);
                // `facilityID is None` means "everywhere"; a number filters.
                let facility = opt_id(req.args.get(1));
                let facility_here = reg.industry_facility(req.actor);
                let held: Vec<crate::industry::BlueprintItem> = reg
                    .store()
                    .items()
                    .iter()
                    .filter(|i| owner.is_some_and(|o| i.owner_id == o))
                    .filter_map(|i| reg.jobs().blueprint_item(reg.content().get(), i))
                    .filter(|b| facility.is_none_or(|f| b.location_id == f))
                    .collect();
                let rows: Vec<PyValue> = held
                    .iter()
                    .map(|b| blueprint_row(reg, b, facility_here.as_ref()))
                    .collect();
                // `bpCounts` is `{activityID: n}`. A reaction formula (MN-A)
                // counts under the reaction activity, everything else under
                // manufacturing; the client only sums the values.
                let table: &crate::industry::Table = reg.content().get();
                let formulas = held.iter().filter(|b| table.is_formula(b.type_id)).count() as i64;
                let counts = int_keyed_dict(vec![
                    (
                        crate::industry::MANUFACTURING,
                        PyValue::Int(rows.len() as i64 - formulas),
                    ),
                    (crate::industry::REACTION, PyValue::Int(formulas)),
                ]);
                Ok(PyValue::Tuple(vec![PyValue::List(rows), counts]))
            }
            // One blueprint by item id, and **the honest answer is a
            // `UserError`**. `blueprintSvc.py(131)`:
            //
            // ```text
            //     def GetBlueprintItem(self, blueprintID):
            //         return industryCommon.BlueprintInstance(self.blueprintManager.GetBlueprintData(blueprintID))
            // ```
            //
            // `BlueprintInstance` opens with `cfg.blueprints[data.typeID]`,
            // so `None` is an `AttributeError` that no caller catches. What
            // the callers *do* catch is `UserError`:
            // `GetBlueprint` and `industrySvc.RecreateJob` both fall back to
            // the blueprint **type** on one, which is the designed path for
            // "that blueprint is gone". `IndustryBlueprintNotFound` is the
            // client's own id for it -- `GetBlueprintType` raises it three
            // lines up.
            //
            // Unreachable today (a blueprint id can only come from a list
            // that is empty), but the moment the item store lands this is
            // either right or a traceback, and the raise is the version that
            // degrades gracefully.
            "GetBlueprintData" => {
                let want = opt_id(req.args.first());
                let facility_here = reg.industry_facility(req.actor);
                let found = reg
                    .store()
                    .items()
                    .iter()
                    .filter(|i| req.actor.is_some_and(|o| i.owner_id == o))
                    .filter(|i| want.is_some_and(|w| i.item_id == w))
                    .filter_map(|i| reg.jobs().blueprint_item(reg.content().get(), i))
                    .next();
                match found {
                    Some(b) => Ok(blueprint_row(reg, &b, facility_here.as_ref())),
                    // Still the right answer for anything we do not have, and
                    // for the reason above: the callers catch `UserError` and
                    // fall back to the blueprint *type*.
                    None => Err(industry_user_error("IndustryBlueprintNotFound")),
                }
            }
            _ => Err(reg.no_method(req)),
        },
        "facilityManager" => match req.method {
            // **A live lock, not an empty list.** `facilitySvc.py(167)`:
            //
            // ```text
            //     def _PrimeFacilities(self, force=False):
            //         with self.loading:
            //             if not force and self.loadedRegion == session.regionid:
            //                 return
            //             self.facilities.clear()
            //             for data in self.facilityManager.GetFacilities():   # :167
            //                 self._PrimeFacility(data)
            //             ...
            //             self.loadedRegion = session.regionid               # :175
            // ```
            //
            // The latch is the **last** statement in the function, so `None`
            // raising in the `for` means `loadedRegion` stays `None`, means
            // the guard never matches, means the next `GetFacilities()` or
            // `GetFacility()` or `IsFacility()` primes again -- the
            // `GetAllMyCharacterWalletLPBalances` mechanism exactly, with the
            // store before the throw instead of after it. `[]` ends it: the
            // loop runs zero times, the latch sets, and the Facilities tab is
            // honestly empty.
            //
            // Empty is also *true*: a facility is a station or structure with
            // an industry service, and we have no world yet. Round 2 of the
            // plan is what gives this rows. When it does, the shape is pinned
            // by `industryCommon.Facility(data)`, which **subscripts**
            // (`data['facilityID']`, so dict / `KeyVal` / `DbRow` all work,
            // unlike the blueprint rows above) for: `facilityID`, `typeID`,
            // `ownerID`, `tax`, `solarSystemID`, `online`, `serviceAccess`,
            // `sccTaxModifier`, `rigModifiers`, `globalModifiers` (a dict
            // with a `'faction'` float and a `'system'` dict keyed by
            // activity) and `activities` -- a dict whose every value is
            // unpacked as a **6-tuple**
            // `(timeModifiers, materialModifiers, costModifiers, categories,
            // groups, invTypes)`.
            "GetFacilities" => Ok(PyValue::List(
                reg.industry_facility(req.actor)
                    .map(|f| vec![facility_row(&f)])
                    .unwrap_or_default(),
            )),
            // `facilitySvc.py(72)`, iterated the same way, inside
            // `GetFacilities(facilityIDs)`. Not a latch -- the caller merges
            // whatever comes back into `self.facilities` and filters -- so
            // `[]` just means "none of those ids is a facility I know", and
            // the comprehension after it drops them.
            "GetFacilitiesByID" => {
                // The caller merges whatever comes back and filters, so
                // answering with our one facility regardless of the ids asked
                // for would be wrong in the other direction: it would put a
                // station in a list the client asked about somewhere else.
                // Filter on the ids when they are given.
                let want: Vec<i64> = match req.args.first() {
                    Some(PyValue::List(v)) | Some(PyValue::Tuple(v)) => {
                        v.iter().filter_map(|a| opt_id(Some(a))).collect()
                    }
                    other => opt_id(other).into_iter().collect(),
                };
                let rows = reg
                    .industry_facility(req.actor)
                    .filter(|f| want.is_empty() || want.contains(&f.facility_id))
                    .map(|f| vec![facility_row(&f)])
                    .unwrap_or_default();
                Ok(PyValue::List(rows))
            }
            // `facilitySvc.py(138)`, under `@Memoize(1)`:
            //
            // ```text
            //     locations = self.facilityManager.GetFacilityLocations(facilityID, ownerID)
            //     cfg.evelocations.Prime(set([location.itemID for location in locations if location.typeID != typeOffice]))
            //     return locations
            // ```
            //
            // Iterated for `.itemID` / `.typeID`, then handed on as
            // `job.locations`, which `len()` is taken of and which
            // `industryCommon.MatchLocation` iterates for `.itemID` and
            // `.flagID`. A list, and empty means the job wizard finds no
            // input or output location -- `MatchLocation` returns `None`,
            // `LoadLocations`'s `if len(job.locations):` skips, and the
            // player gets `MISSING_INPUT_LOCATION` on the submit button
            // rather than a traceback. That is the correct complaint for a
            // server with no hangars.
            // **Non-empty, and that is the other half of the unblock.**
            // `industrySvc.LoadLocations` is
            //
            // ```text
            //     job.locations = GetFacilityLocations(job.facilityID, job.ownerID)
            //     if len(job.locations):
            //         self._ApplyJobSettings(job)
            // ```
            //
            // so an empty list means `_ApplyJobSettings` never runs,
            // `inputLocation` and `outputLocation` stay `None`, and
            // `Job._get_errors` raises `MISSING_INPUT_LOCATION` and
            // `MISSING_OUTPUT_LOCATION` -- the submit button disabled with a
            // tooltip the player cannot act on. One row is enough:
            // `MatchLocation` falls back to `locations[0]`.
            //
            // Why these must be `industry.job.Location` and not `KeyVal` is
            // on `location_row`, and it is an `__eq__` that does not exist.
            "GetFacilityLocations" => {
                let Some(f) = reg.industry_facility(req.actor) else {
                    return Ok(PyValue::List(vec![]));
                };
                let Some(owner) = req.actor else {
                    return Ok(PyValue::List(vec![]));
                };
                Ok(PyValue::List(vec![location_row(
                    f.facility_id,
                    f.type_id,
                    owner,
                    crate::industry::FLAG_HANGAR,
                )]))
            }
            // **The one empty here that must not be `{}`.**
            // `facilitySvc.py(100)` is the textbook poisoned cache --
            //
            // ```text
            //     def InitMaxActivityModifiers(self):
            //         if self.maxActivityModifiers is None:
            //             self.maxActivityModifiers = self.facilityManager.GetMaxActivityModifiers()
            // ```
            //
            // -- so `None` stores successfully, fails its own guard forever,
            // and then `GetMaxActivityModifier` raises
            // `AttributeError: 'NoneType' object has no attribute 'get'` on
            // every call. Two failures, one loop, one traceback per repaint.
            //
            // But the reader has **two** branches:
            //
            // ```text
            //     if activityID:
            //         return self.maxActivityModifiers.get(activityID, 1.0)
            //     else:
            //         return max(self.maxActivityModifiers.values()) or 1.0
            // ```
            //
            // and `max({}.values())` is `ValueError: max() arg is an empty
            // sequence`. An empty dict is therefore *almost* right: it
            // survives the only two call sites that exist today
            // (`systemCostIndexGauge.py(38)` and `facilityEntry.py(82)`),
            // both of which pass a real activity id, and raises on the
            // no-argument form the moment anything uses it.
            // So: all six activity ids, explicitly, at the baseline.
            //
            // `1.0` and not `0.0`, and a float and not an int, because the
            // gauge **divides** by it: `Gauge.SetValue(self, value / maxValue)`.
            // `0` there is a `ZeroDivisionError`; an int would make the
            // `or 1.0` branch honest but still invites Python 2 integer
            // division in anything that divides two of these. 1.0 is also the
            // true value -- it is the multiplier for a system with no cost
            // index, which is every system we have.
            "GetMaxActivityModifiers" => Ok(int_keyed_dict(
                ACTIVITIES
                    .iter()
                    .map(|a| (*a, PyValue::Real(1.0)))
                    .collect(),
            )),
            // `facilitySvc.py(103)`, consumed one frame later by
            // `facilityTaxWindow.py(30)`:
            //
            // ```text
            //     self.taxRate = KeyVal(value=self.facilitySvc.GetFacilityTaxes(self.facilityID).taxCorporation)
            // ```
            //
            // An **attribute** read, so a `KeyVal` and not a dict. The value
            // itself may be `None` and the window says so in as many words
            // -- `setvalue='0.0' if taxRateValue is None else ...` -- so
            // `None` inside the bag is the honest "no tax is set", while
            // `None` *as* the bag is `AttributeError`. The other six names
            // are the ones `Submit` sends straight back
            // (`taxAlliance`, `taxStandings{Horrible,Bad,Neutral,Good,High}`),
            // which is the real server's column set for this row.
            //
            // Reachable only from a structure you own, so this is the arm
            // least likely to fire; it is one line and it removes a window
            // that would otherwise look like a different bug.
            "GetFacilityTaxes" => Ok(keyval(vec![
                ("taxCorporation", PyValue::None),
                ("taxAlliance", PyValue::None),
                ("taxStandingsHorrible", PyValue::None),
                ("taxStandingsBad", PyValue::None),
                ("taxStandingsNeutral", PyValue::None),
                ("taxStandingsGood", PyValue::None),
                ("taxStandingsHigh", PyValue::None),
            ])),
            // One facility by id, and the client is **written for the
            // exception**. Two of its three call sites catch it by name:
            //
            // ```text
            //     facilitySvc.py:57   try: self._PrimeFacility(self.facilityManager.GetFacility(facilityID))
            //                         except UserError: pass
            //     browserBlueprints.py:341
            //                         try: facility = facilitySvc.GetFacility(facilityID)
            //                         except UserError: pass
            // ```
            //
            // Returning a value is worse than raising twice over: `None`
            // reaches `industryCommon.Facility(data)` and dies on
            // `data['facilityID']` with a `TypeError` nobody catches, and a
            // *fabricated* row would put a facility in the list that the
            // player cannot install anything at. `IndustryFacilityAccessDenied`
            // is the client's own id, from
            // `industryCommon.AssertFacilityAccess`.
            //
            // Consistent with `GetFacilities` returning nothing: if the list
            // is empty, every id asked about here is one we do not have.
            // The same row for our own station, and the raise kept for
            // anything else -- two of its three call sites are already inside
            // `except UserError:`, so the raise is a supported answer and not
            // a failure.
            "GetFacility" => {
                let want = opt_id(req.args.first());
                match reg
                    .industry_facility(req.actor)
                    .filter(|f| want.is_none_or(|w| w == f.facility_id))
                {
                    Some(f) => Ok(facility_row(&f)),
                    None => Err(industry_user_error("IndustryFacilityAccessDenied")),
                }
            }
            // Deliberately not served -- a mutator whose return value is
            // discarded (`facilitySvc.SetFacilityTaxes` ignores it), so our
            // `None` cannot hang anything and an `UNANSWERED` line is better
            // evidence than a silent success for a tax change that did not
            // happen. Listed here so the next reader does not have to
            // re-derive that. Round 2, with the push path.
            _ => Err(reg.no_method(req)),
        },
        "industryManager" => match req.method {
            // The Jobs tab. `industrySvc.py(112)`, iterated:
            //
            // ```text
            //     for data in sm.RemoteSvc('industryManager').GetJobsByOwner(ownerID, includeCompleted):
            //         job = self._JobInstance(data, fetchBlueprint=False)
            // ```
            //
            // `None` is a `TypeError` in the `for`, after
            // `self._ownerJobs[ownerID].clear()` has already run -- so the
            // client's own cache is wiped and not refilled on every attempt.
            // `[]` is a character with no jobs, which is true, and the loop
            // then runs zero times.
            //
            // Not a latch: the caches here are keyed off the caller's
            // `useCached` flag, not off the answer's truthiness.
            //
            // Row shape, for round 2, from `industryCommon.JobData(data, ...)`
            // -- all **attribute** reads: `activityID`, `jobID`,
            // `blueprintID`, `blueprintTypeID`, `blueprintLocationID`,
            // `blueprintLocationFlagID`, `blueprintCopy`, `facilityID`,
            // `ownerID`, `status`, `installerID`, `completedCharacterID`,
            // `solarSystemID`, `stationID`, `startDate`, `endDate`,
            // `pauseDate`, `runs`, `licensedRuns`, `successfulRuns`, `cost`,
            // `timeInSeconds`, `probability`, `productTypeID`,
            // `optionalTypeID`, `optionalTypeID2`, `outputLocationID`,
            // `outputFlagID`. The three dates are blue times and
            // `BlueToDate` is applied to them, so they are win64 FILETIME
            // longs, not seconds.
            // **The second argument really means something.** `GetOwnerJobs`
            // filters `status < STATUS_COMPLETED` itself, but caches on a
            // separate `_ownerJobsFetchedCompleted` set -- so answering
            // without completed jobs when asked for them leaves the Jobs
            // tab's history permanently empty.
            "GetJobsByOwner" => {
                let owner = opt_id(req.args.first()).or(req.actor);
                let include_completed = matches!(req.args.get(1), Some(PyValue::Bool(true)));
                let Some(owner) = owner else {
                    return Ok(PyValue::List(vec![]));
                };
                Ok(PyValue::List(
                    reg.jobs()
                        .for_owner(owner, include_completed)
                        .into_iter()
                        .map(job_row)
                        .collect(),
                ))
            }
            // **The third live lock in this file, and the quietest.**
            // `industrySvc.py(337)` stores it and
            // `GetJobCountForActivity` (`industrySvc.py(562, 565)`) reads
            // it:
            //
            // ```text
            //     def _UpdateSlots_thread(self):
            //         self.slots = sm.RemoteSvc('industryManager').GetJobCounts(session.charid)
            //     ...
            //     def GetJobCountForActivity(self, activityID):
            //         if self.slots is None:
            //             self._UpdateSlots()
            //             return
            //         else:
            //             return self.slots.get(activityID, 0)
            // ```
            //
            // `None` is stored, fails its own `is None` guard, and the reader
            // *re-arms the fetch and returns `None`* -- which `jobsStrip`
            // renders as `'-'`. So there is no traceback at all: the slot
            // counters read "- / 5" and the call re-issues from every
            // `UpdateState`, throttled to 1-3 s by
            // `call_after_wallclocktime_delay`. That is the
            // `scanMgr.GetSystemScanMgr` cadence -- a handful per minute,
            // forever -- so the "repeat count in the hundreds" tell does not
            // fire on it and the repeat *interval* is the thing to look at.
            //
            // A dict, because the only consumer is `.get(activityID, 0)`,
            // and **int-keyed**, because `activityID` is an int. Populated
            // with explicit zeroes rather than `{}`: `.get`'s default would
            // make `{}` behave identically today, and that is exactly the
            // problem -- it would also make a wrong key type behave
            // identically, with no symptom. Six real keys mean the test can
            // pin the key type, and mean a later non-zero count has somewhere
            // to go.
            // Real counts now, and still **never an empty dict**: the
            // reason is on `GetMaxActivityModifiers` above, and it is that
            // `max({}.values())` is a `ValueError`. `Jobs::counts` returns
            // all six activities in `ACTIVITIES` order for exactly that.
            "GetJobCounts" => {
                let owner = opt_id(req.args.first()).or(req.actor);
                let counts = match owner {
                    // Science shares one pool of slots, so each science key
                    // carries the pool's count: `Jobs::slot_counts`.
                    Some(o) => reg.jobs().slot_counts(o),
                    None => crate::industry::ACTIVITIES.map(|a| (a, 0)),
                };
                Ok(int_keyed_dict(
                    counts.iter().map(|&(a, n)| (a, PyValue::Int(n))).collect(),
                ))
            }
            // One job by id. `industrySvc.py(89)` hands it straight to
            // `_JobInstance`, which reads `data.blueprintID` -- so `None` is
            // an `AttributeError` with no handler. We have no job store, so
            // every id asked about here is one we do not have, and
            // `IndustryJobAccessDenied` (the client's own id, from
            // `industryCommon.AssertJobAccess`) says that out loud.
            //
            // Reachable only from a job list that is empty or an
            // `OnIndustryJob` push we do not send, so this is a guard against
            // a future path rather than a fix for a live one. Raising is the
            // loud-and-visible answer the brief asks for where the shape
            // cannot be invented honestly: a fabricated job row would put a
            // job in the window that no amount of waiting will ever finish.
            "GetJob" => {
                let want = opt_id(req.args.first());
                match want
                    .and_then(|w| reg.jobs().get(w))
                    // `industryCommon.AssertJobAccess`: a job you do not own
                    // is a job you may not read, and the client's own id for
                    // that is the one we were already raising.
                    .filter(|j| req.actor.is_some_and(|a| j.owner_id == a))
                {
                    Some(job) => Ok(job_row(job)),
                    None => Err(industry_user_error("IndustryJobAccessDenied")),
                }
            }
            // **Where a job actually starts.** `industrySvc.InstallJob(job)`
            // is `sm.RemoteSvc('industryManager').InstallJob(job.dump())` --
            // **one argument**, the 17-key dict of `docs/lanes/industry.md`
            // §3.1 -- and the local wrapper *returns* what we answer
            // (`RETURN_VALUE` at offset 30) while `SubmitButton.ClickFunc`
            // then discards it (`POP_TOP`). So the job id we return is
            // correct and unread, and the only thing that puts the job in the
            // window is the `OnIndustryJob` push. **[V]**
            //
            // On a `UserError` the wrapper does
            // `self.facilitySvc.Reload(job.facilityID)` and re-raises, so a
            // refusal costs one `GetFacilities` round trip and leaves the
            // window consistent. That is what makes refusing safe here rather
            // than merely honest.
            "InstallJob" => {
                // **A mutator never answers `None`.** Three of the four
                // discard their return value, so a `None` from a connection
                // that is nobody would report a success that did not happen
                // -- the exact failure the round-1 note refused to ship.
                // `IndustryJobAccessDenied` is a refusal the client has a
                // message for, and no real client can reach it: without a
                // character there is no `industryManager` moniker.
                let Some(actor) = req.actor else {
                    return Err(industry_user_error("IndustryJobAccessDenied"));
                };
                let Some(facility) = reg.industry_facility(Some(actor)) else {
                    // Only a character with no station at all reaches this:
                    // `industry_facility` is "my station", not a docked gate,
                    // and installing from space is retail's remote install
                    // (`docs/lanes/docked.md`).
                    return Err(industry_user_error("IndustryFacilityNotFound"));
                };
                let mut r = install_request(req.args.first())?;
                // T3-C: the generic decryptor among the request's materials.
                r.decryptor = req.args.first().and_then(|a| {
                    crate::industry::relic::decryptor_among(reg.content(), &material_types(a))
                });
                let Some(item) = reg.store().get(r.blueprint_id) else {
                    return Err(industry_user_error("IndustryBlueprintNotFound"));
                };
                if item.owner_id != actor {
                    // `industryCommon.py:259`'s own id, and a modal: being
                    // handed somebody else's blueprint id is not a thing a
                    // player can fix from the tooltip.
                    return Err(industry_user_error("IndustryBlueprintAccessDenied"));
                }
                let Some(blueprint) = reg.jobs().blueprint_item(reg.content().get(), item) else {
                    return Err(industry_user_error("ItemNotBlueprint"));
                };
                // Owner **and** location, exactly as `service::reprocess`'s
                // `inputs_for` does it and as `Jobs::plan_install`'s own doc
                // comment requires. The client holds other stations' hangars
                // in its cache, so without the location filter a job would
                // consume materials the character cannot reach -- the same
                // hole reprocessing had and closed.
                let on_hand: Vec<crate::industry::Stack> = reg
                    .store()
                    .items()
                    .iter()
                    .filter(|i| i.owner_id == actor && i.location_id == r.input_location_id)
                    .map(crate::industry::Stack::of)
                    .collect();
                let pilot = crate::industry::Pilot {
                    character_id: actor,
                    // **Corporation jobs are refused, not half-served.**
                    // `Job._get_errors` wants `ROLE_FACTORY_MANAGER` (1024)
                    // plus `ROLE_FACTORY_MANUFACTURING`, and `crate::roles`
                    // has roles but no office divisions -- so `None` here and
                    // the `Refusal::InvalidOwner` the model derives from it
                    // are the honest answer.
                    corporation_id: None,
                    skills: reg.skills_for(Some(actor)),
                    implants: &[],
                    // `Character::balance`. `ACCOUNT_FUNDS` only compares it;
                    // the debit is `Registry::install_job`'s, and it happens
                    // in this same turn under this same lock.
                    balance: reg.character(actor).map_or(0.0, |c| c.balance),
                };
                let install = reg
                    .jobs()
                    .plan_install(
                        reg.content().get(),
                        reg.content(),
                        crate::pyutil::filetime_now(),
                        &pilot,
                        &facility,
                        &blueprint,
                        &r,
                        &on_hand,
                    )
                    .map_err(|refusal| validation_error(&[refusal]))?
                    // Lane clock: the job's end, at the game clock's pace.
                    .paced(crate::game_clock::of(reg));
                // A copy's runs change on install, and a copy spent to zero is
                // gone: either way the cached blueprint list is stale.
                // A relic stack lost units, which is the same staleness.
                let spent_a_copy = install.blueprint_after.is_some()
                    || reg
                        .content()
                        .get::<crate::industry::Table>()
                        .is_relic(blueprint.type_id);
                let job_id = reg
                    .install_job(install)
                    .map_err(|_| industry_user_error("IndustryJobInstallError"))?;
                if spent_a_copy {
                    reg.push(BlueprintsUpdated { owner_id: actor });
                }
                Ok(PyValue::Int(job_id))
            }
            // "Deliver" on a ready job. `industrySvc.CompleteJob` is a
            // module-level function, not a method, and it calls
            // `industryManager.CompleteJob(int(jobID), solarSystemID)` --
            // **two arguments**, return discarded. **[V]**
            //
            // **The system id is ignored on purpose.** It is the client
            // telling us where it believes the job is; `Job::solar_system_id`
            // is where it really is. Reading the argument would let a caller
            // deliver a job by naming the wrong system, which is a check that
            // can only ever be wrong in the attacker's favour.
            "CompleteJob" => {
                let Some(actor) = req.actor else {
                    return Err(industry_user_error("IndustryJobAccessDenied"));
                };
                let Some(job_id) = opt_id(req.args.first()) else {
                    return Err(industry_user_error("IndustryJobNotFound"));
                };
                deliver_one(reg, actor, job_id, crate::pyutil::filetime_now())?;
                Ok(PyValue::None)
            }
            // The Jobs tab's "deliver all". `industrySvc.CompleteJobs(jobs)`
            // -> `industryManager.CompleteManyJobs([(jobID, solarSystemID),
            // ...])`: **one argument, a list of 2-tuples**, return discarded.
            // **[V]**
            //
            // **Two corrections to this repo's own notes**, both from
            // disassembling `industrySvc.pyc` rather than reading about it.
            // `docs/lanes/industry.md` §6.5 calls the local wrapper
            // `CompleteManyJobs`; it is `CompleteJobs`, at line 217, and it is
            // a module-level function. And §8 of the wave-1 note below says
            // the census "lost" this call to a list comprehension -- it did
            // not: the comprehension is there, but what the census was
            // looking for was the *remote* name, which appears only as a
            // `LOAD_ATTR` on a moniker and never as a call site. The blind
            // spot is real and it is a different blind spot.
            //
            // **Partial success is the answer, not an error.** The client
            // builds the list from jobs it already believes are ready and
            // throws our answer away, so one refusal must not cost the others
            // their delivery; each job that delivers gets its own
            // `OnIndustryJob` push, which is how the window learns. We raise
            // only when **nothing** was delivered, so a player who presses
            // the button and sees no movement still gets a log line.
            "CompleteManyJobs" => {
                let Some(actor) = req.actor else {
                    return Err(industry_user_error("IndustryJobAccessDenied"));
                };
                let (PyValue::List(rows) | PyValue::Tuple(rows)) = req
                    .args
                    .first()
                    .ok_or_else(|| industry_user_error("IndustryJobNotFound"))?
                else {
                    return Err(industry_user_error("IndustryJobNotFound"));
                };
                let wanted: Vec<i64> = rows
                    .iter()
                    .filter_map(|row| match row {
                        PyValue::Tuple(pair) | PyValue::List(pair) => opt_id(pair.first()),
                        // A bare id: no client sends one, a script might.
                        PyValue::Int(n) if *n != 0 => Some(*n),
                        _ => None,
                    })
                    .collect();
                let now = crate::pyutil::filetime_now();
                let mut delivered = 0usize;
                let mut first_refusal = None;
                for job_id in wanted {
                    match deliver_one(reg, actor, job_id, now) {
                        Ok(()) => delivered += 1,
                        Err(e) => first_refusal = first_refusal.or(Some(e)),
                    }
                }
                match (delivered, first_refusal) {
                    (0, Some(refusal)) => Err(refusal),
                    _ => Ok(PyValue::None),
                }
            }
            // Stop a running job. `industrySvc.CancelJob(jobID,
            // solarSystemID)` -> `industryManager.CancelJob(int(jobID),
            // solarSystemID)`, return discarded. **[V]**
            //
            // **No refund, and the materials stay gone**, which is retail's
            // behaviour and is also the only coherent one here: the install
            // consumed them in a single all-or-nothing plan, so there is no
            // half-finished state to compensate for. `Jobs::plan_cancel` says
            // the same thing at the model layer, and notes that EvEJS reaches
            // it through a 1,054-line settlement saga that attempts a refund
            // and gives up.
            "CancelJob" => {
                let Some(actor) = req.actor else {
                    return Err(industry_user_error("IndustryJobAccessDenied"));
                };
                let Some(job_id) = opt_id(req.args.first()) else {
                    return Err(industry_user_error("IndustryJobNotFound"));
                };
                let job = owned_job(reg, actor, job_id)?;
                if job.completed() {
                    return Err(industry_user_error("IndustryJobAlreadyCompleted"));
                }
                let done = reg
                    .jobs()
                    .plan_cancel(job_id, crate::pyutil::filetime_now())
                    .map_err(|refusal| validation_error(&[refusal]))?;
                reg.cancel_job(done);
                Ok(PyValue::None)
            }
            _ => Err(reg.no_method(req)),
        },
        "industryMonitor" => match req.method {
            // The job wizard's material monitor. `industrySvc.py(430)`:
            //
            // ```text
            //     def ConnectJob(self, job):
            //         self.monitoring = weakref.ref(job)
            //         job.monitorID, job.available = sm.RemoteSvc('industryMonitor').ConnectJob(job.dump())
            // ```
            //
            // Called whenever the job's input location changes (`on_input_location`).
            // `available` is `{typeID: quantity}`: every material does
            // `materials.get(self.typeID, 0)` and the submit button reads
            // `MISSING_MATERIAL` from it, so an empty answer disabled the
            // button for a human whatever the hangar held. The answer is the
            // pilot's stacks in the dumped `inputLocation` (owner, location
            // and flag -- what `InstallJob` draws from), and `monitorID` is
            // a real id: `OnIndustryMaterials(monitorID, materials)` follows
            // whenever those stacks change (`refresh_monitors`).
            //
            // The return value is unpacked, so it stays a pair for a
            // connection that is nobody (nothing to watch, nothing held).
            "ConnectJob" => match req.actor {
                Some(actor) => Ok(connect_monitor(reg, actor, req.args.first())),
                None => Ok(PyValue::Tuple(vec![PyValue::None, PyValue::Dict(vec![])])),
            },
            // `DisconnectJob(job.monitorID)` on the window closing; the
            // return is discarded. An id that is not the caller's own, or
            // `None`, releases nothing.
            "DisconnectJob" => {
                if let (Some(actor), Some(id)) = (req.actor, opt_id(req.args.first())) {
                    reg.industry_monitors
                        .list
                        .retain(|m| !(m.id == id && m.actor == actor));
                }
                Ok(PyValue::None)
            }
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

#[cfg(test)]
mod relic_socket;

#[cfg(test)]
mod tests {
    #![allow(unused_imports)]
    use super::*;
    use crate::pyutil::dict_get;

    /// Call one arm of this module with `n` placeholder positional arguments
    /// and **no character selected**.
    ///
    /// This used to say the arguments were never read, "because every arm in
    /// this module answers the same shape regardless of which owner,
    /// facility or job was asked about". That stopped being true when the
    /// arms got a world: they read `req.actor` and the ids they are given.
    /// So this helper now tests one specific case -- the connection that is
    /// nobody -- and [`call_as`] tests the other.
    fn call(service: &str, method: &str, nargs: usize) -> Result<PyValue, ServiceError> {
        let args: Vec<PyValue> = (0..nargs).map(|i| PyValue::Int(i as i64 + 1)).collect();
        call_with(service, method, &args, None)
    }

    /// The first fixture character, which is docked and so has a facility.
    const ACTOR: i64 = 90_000_001;

    fn call_as(service: &str, method: &str, args: &[PyValue]) -> Result<PyValue, ServiceError> {
        call_with(service, method, args, Some(ACTOR))
    }

    fn call_with(
        service: &str,
        method: &str,
        args: &[PyValue],
        actor: Option<i64>,
    ) -> Result<PyValue, ServiceError> {
        let mut reg = Registry::new(SandboxConfig::default());
        dispatch(
            &mut reg,
            &Request {
                service,
                method,
                args,
                kwargs: &[],
                bind_params: None,
                actor,
            },
        )
    }

    /// Every service this module advertises must reach an arm in *this*
    /// module's `dispatch`.
    #[test]
    fn every_service_here_is_routed_to_this_module() {
        for s in SERVICES {
            match call(s, "__no_such_method_exists__", 0) {
                Err(ServiceError::NoSuchMethod { .. }) => {}
                other => panic!(
                    "{s} is advertised by this module but its dispatch answers {other:?} \
                     to a method that cannot exist"
                ),
            }
        }
    }

    /// The one key, by name, because the client reads it by name inside a
    /// service constructor whose failure poisons the service.
    ///
    /// Looked up with `dict_get`, which reads the pair the way the wire does
    /// -- `(value, key)`. The first version of this test matched slot 0
    /// against the key, which agreed with a `PyValue::Dict` built the wrong
    /// way round and so passed on a dict the client would have raised
    /// `KeyError` on.
    #[test]
    fn the_blueprint_limit_is_a_dict_with_one_named_integer() {
        let out = call("blueprintManager", "GetLimits", 0).expect("GetLimits must be answered");
        assert!(
            matches!(out, PyValue::Dict(_)),
            "the client subscripts this with a string: {out:?}"
        );
        let value = dict_get(&out, b"maxBlueprintResults")
            .expect("blueprintSvc.Run reads ['maxBlueprintResults'] and nothing else");
        assert!(
            matches!(value, PyValue::Int(n) if *n > 0),
            "the client slices with this, so it must be a positive integer: {value:?}"
        );
    }

    /// `bps, bpCounts = ...` is an `UNPACK_SEQUENCE 2` that runs before
    /// either half is looked at, and the halves are different families:
    /// element 0 is iterated, element 1 needs `.itervalues()`.
    #[test]
    fn blueprints_by_owner_is_a_list_and_a_dict_in_that_order() {
        let out = call("blueprintManager", "GetBlueprintDataByOwner", 2)
            .expect("GetBlueprintDataByOwner must be answered");
        let PyValue::Tuple(parts) = &out else {
            panic!("unpacked by the client, so a sequence: {out:?}")
        };
        assert_eq!(
            parts.len(),
            2,
            "UNPACK_SEQUENCE 2 raises on any other arity"
        );
        assert!(
            matches!(parts[0], PyValue::List(_)),
            "_AttachFacilities iterates element 0: {:?}",
            parts[0]
        );
        assert!(
            matches!(parts[1], PyValue::Dict(_)),
            "GetTotalNumBlueprintsForOwner calls .itervalues() on element 1: {:?}",
            parts[1]
        );
    }

    /// Four reads the client puts straight into a `for`. A dict would iterate
    /// its keys and a `None` raises, so the family is the whole contract.
    #[test]
    fn everything_the_client_iterates_is_a_list() {
        for (service, method, nargs) in [
            ("facilityManager", "GetFacilities", 0),
            ("facilityManager", "GetFacilitiesByID", 1),
            ("facilityManager", "GetFacilityLocations", 2),
            ("industryManager", "GetJobsByOwner", 2),
        ] {
            let out = call(service, method, nargs)
                .unwrap_or_else(|e| panic!("{service}.{method} must be answered: {e:?}"));
            assert!(
                matches!(out, PyValue::List(_)),
                "{service}.{method} is iterated by the client: {out:?}"
            );
        }
    }

    /// `facilitySvc.GetMaxActivityModifier()` with no argument reaches
    /// `max(self.maxActivityModifiers.values())`, which raises `ValueError`
    /// on an empty dict -- so this is the one "nothing here" in the module
    /// that must be **non-empty** -- and the gauge divides by the value, so
    /// it must be non-zero.
    #[test]
    fn max_activity_modifiers_is_non_empty_and_never_zero() {
        let out = call("facilityManager", "GetMaxActivityModifiers", 0)
            .expect("GetMaxActivityModifiers must be answered");
        let PyValue::Dict(entries) = &out else {
            panic!("the client calls .get() and .values() on it: {out:?}")
        };
        assert!(
            !entries.is_empty(),
            "max() over an empty dict is ValueError: max() arg is an empty sequence"
        );
        for (value, key) in entries {
            // Wire order is (value, key); see PyValue::Dict.
            let PyValue::Int(activity) = key else {
                panic!("activityID is an int, so .get(activityID) needs int keys: {key:?}")
            };
            assert!(
                ACTIVITIES.contains(activity),
                "{activity} is not one of industry.ACTIVITIES"
            );
            assert!(
                matches!(value, PyValue::Real(x) if *x > 0.0),
                "systemCostIndexGauge divides by this: {value:?}"
            );
        }
    }

    /// `self.slots.get(activityID, 0)` is the only consumer, and the guard
    /// above it is `if self.slots is None` -- so the answer must be a dict,
    /// must not be `None`, and must be keyed by **int** activity ids. A
    /// string key would miss every lookup and return the default silently.
    #[test]
    fn job_counts_is_an_int_keyed_dict_of_zeroes() {
        let out =
            call("industryManager", "GetJobCounts", 1).expect("GetJobCounts must be answered");
        let PyValue::Dict(entries) = &out else {
            panic!("stored in self.slots and read with .get(): {out:?}")
        };
        let mut seen: Vec<i64> = Vec::new();
        for (value, key) in entries {
            let PyValue::Int(activity) = key else {
                panic!("keyed by int activityID: {key:?}")
            };
            assert_eq!(
                value,
                &PyValue::Int(0),
                "no jobs exist, and the UI prints this straight into a label"
            );
            seen.push(*activity);
        }
        seen.sort_unstable();
        assert_eq!(
            seen,
            ACTIVITIES.to_vec(),
            "all six industry.ACTIVITIES, so that a missing key can never be \
             mistaken for a zero count"
        );
    }

    /// `GetFacilityTaxes(facilityID).taxCorporation` is an attribute read, so
    /// the answer is an instance and not a dict. The value inside may be
    /// `None`; the bag may not.
    #[test]
    fn facility_taxes_is_a_keyval_with_a_tax_corporation_attribute() {
        let out = call("facilityManager", "GetFacilityTaxes", 2)
            .expect("GetFacilityTaxes must be answered");
        let PyValue::Instance { class, state } = &out else {
            panic!("facilityTaxWindow reads .taxCorporation off it: {out:?}")
        };
        assert_eq!(
            class.as_slice(),
            crate::pyutil::KEYVAL_CLASS.as_bytes(),
            "any other dotted name is refused with HACKER WARNING"
        );
        assert_eq!(
            dict_get(state, b"taxCorporation"),
            Some(&PyValue::None),
            "no corporation tax is set, and the window renders None as '0.0'"
        );
    }

    /// `job.monitorID, job.available = ...ConnectJob(...)` -- arity 2, and
    /// element 1 is `.get(typeID, 0)`-ed by every material in the job. A
    /// connection that is nobody has nothing to watch and nothing held.
    #[test]
    fn connect_job_is_a_pair_whose_second_half_is_a_dict() {
        let out = call("industryMonitor", "ConnectJob", 1).expect("ConnectJob must be answered");
        let PyValue::Tuple(parts) = &out else {
            panic!("UNPACK_SEQUENCE 2 on the client: {out:?}")
        };
        assert_eq!(parts.len(), 2, "two names on the left-hand side, no more");
        assert_eq!(parts[0], PyValue::None, "no actor, no monitor");
        assert_eq!(parts[1], PyValue::Dict(vec![]), "and nothing held");
    }

    /// `{typeID: qty}` out of an answer, whatever order the wire holds it in.
    fn available_of(v: &PyValue) -> std::collections::BTreeMap<i64, i64> {
        let PyValue::Dict(entries) = v else {
            panic!("a dict: {v:?}")
        };
        entries
            .iter()
            .map(|(qty, key)| match (qty, key) {
                (PyValue::Int(q), PyValue::Int(t)) => (*t, *q),
                other => panic!("int pair: {other:?}"),
            })
            .collect()
    }

    fn connect_dump(station: i64, flag: i64) -> PyValue {
        pydict(vec![("inputLocation", location(station, flag))])
    }

    /// The window's submit button reads this: the pilot's own stacks in the
    /// dumped input division, summed by type, and nothing from elsewhere.
    #[test]
    fn connect_job_reports_the_pilots_stacks_in_the_input_division() {
        let (mut reg, station, _bp) = with_a_blueprint(1);
        let held_here = held(&reg, TRITANIUM);
        assert!(held_here > 0, "the fixture stocks the hangar");
        let out = call_in(
            &mut reg,
            "industryMonitor",
            "ConnectJob",
            &[connect_dump(station, crate::industry::FLAG_HANGAR)],
        )
        .expect("answered");
        let PyValue::Tuple(parts) = &out else {
            panic!("a pair: {out:?}")
        };
        assert!(
            matches!(parts[0], PyValue::Int(id) if id > 0),
            "a real, truthy monitor id: {:?}",
            parts[0]
        );
        let avail = available_of(&parts[1]);
        assert_eq!(avail.get(&TRITANIUM), Some(&held_here));
        // Another division of the same station holds none of it.
        let out = call_in(
            &mut reg,
            "industryMonitor",
            "ConnectJob",
            &[connect_dump(station, crate::industry::FLAG_HANGAR + 1)],
        )
        .expect("answered");
        let PyValue::Tuple(parts) = &out else {
            panic!("a pair: {out:?}")
        };
        assert_eq!(available_of(&parts[1]).get(&TRITANIUM), None);
    }

    /// An unreadable input location is nothing to watch.
    #[test]
    fn connect_job_without_a_readable_location_watches_nothing() {
        let (mut reg, _station, _bp) = with_a_blueprint(1);
        let out = call_in(
            &mut reg,
            "industryMonitor",
            "ConnectJob",
            &[pydict(vec![("inputLocation", PyValue::None)])],
        )
        .expect("answered");
        assert_eq!(
            out,
            PyValue::Tuple(vec![PyValue::None, PyValue::Dict(vec![])])
        );
    }

    /// An install spends materials; the open window is told the new counts
    /// under its own monitor id, and a second `ConnectJob` replaces the first.
    #[test]
    fn spending_materials_pushes_the_new_counts_to_the_watching_window() {
        let (mut reg, station, blueprint_id) = with_a_blueprint(3);
        let out = call_in(
            &mut reg,
            "industryMonitor",
            "ConnectJob",
            &[connect_dump(station, crate::industry::FLAG_HANGAR)],
        )
        .expect("answered");
        let PyValue::Tuple(parts) = &out else {
            panic!("a pair: {out:?}")
        };
        let PyValue::Int(monitor) = parts[0] else {
            panic!("a monitor id")
        };
        let _ = reg.take_pending();
        let before = held(&reg, TRITANIUM);
        call_in(
            &mut reg,
            "industryManager",
            "InstallJob",
            &[install_dump(blueprint_id, station, 2)],
        )
        .expect("a funded, skilled, stocked install");
        let pushes: Vec<_> = reg
            .take_pending()
            .iter()
            .map(|(_, n)| n.to_push())
            .filter(|(event, _)| *event == "OnIndustryMaterials")
            .collect();
        assert_eq!(pushes.len(), 1, "one push, not one per stack touched");
        assert_eq!(pushes[0].1[0], PyValue::Int(monitor));
        let avail = available_of(&pushes[0].1[1]);
        assert_eq!(
            avail.get(&TRITANIUM),
            Some(&(before - tritanium_per_run() * 2))
        );
        assert_eq!(avail.get(&TRITANIUM), Some(&held(&reg, TRITANIUM)));
    }

    /// `DisconnectJob` ends the watch; nothing follows a change after it, and
    /// another pilot's id releases nothing.
    #[test]
    fn disconnect_job_stops_the_pushes() {
        let (mut reg, station, blueprint_id) = with_a_blueprint(3);
        let out = call_in(
            &mut reg,
            "industryMonitor",
            "ConnectJob",
            &[connect_dump(station, crate::industry::FLAG_HANGAR)],
        )
        .expect("answered");
        let PyValue::Tuple(parts) = &out else {
            panic!("a pair: {out:?}")
        };
        assert_eq!(
            call_in(
                &mut reg,
                "industryMonitor",
                "DisconnectJob",
                &[PyValue::None]
            )
            .expect("None releases nothing"),
            PyValue::None
        );
        assert_eq!(reg.industry_monitors.list.len(), 1);
        call_in(
            &mut reg,
            "industryMonitor",
            "DisconnectJob",
            &[parts[0].clone()],
        )
        .expect("discarded");
        assert!(reg.industry_monitors.list.is_empty());
        let _ = reg.take_pending();
        call_in(
            &mut reg,
            "industryManager",
            "InstallJob",
            &[install_dump(blueprint_id, station, 1)],
        )
        .expect("install");
        assert!(
            !reg.take_pending()
                .iter()
                .any(|(_, n)| n.to_push().0 == "OnIndustryMaterials"),
            "no monitor, no push"
        );
    }

    /// The three reads that are specified by what they throw. Each one's
    /// caller catches `UserError` by name, so a `CallRsp` -- of any shape --
    /// is the wrong answer.
    #[test]
    fn the_three_by_id_reads_raise_a_user_error() {
        for (service, method, nargs) in [
            ("blueprintManager", "GetBlueprintData", 1),
            ("facilityManager", "GetFacility", 1),
            ("industryManager", "GetJob", 1),
        ] {
            match call(service, method, nargs) {
                Err(ServiceError::Raise(PyValue::ObjectEx { header, .. })) => {
                    let PyValue::Tuple(parts) = *header else {
                        panic!("{service}.{method}: a 0x22 header is a 2-tuple")
                    };
                    assert_eq!(
                        parts[0],
                        PyValue::Global(b"eveexceptions.UserError".to_vec()),
                        "{service}.{method} must raise the class the client catches"
                    );
                }
                other => panic!(
                    "{service}.{method} is consumed inside an `except UserError:` block, \
                     so it must raise, not return: {other:?}"
                ),
            }
        }
    }

    /// The mutator that is still deliberately unanswered.
    ///
    /// This test used to list six, and the four job mutators have since been
    /// served -- which is **the test working as designed**: it was written as
    /// a tripwire so that answering one would be a decision somebody makes on
    /// purpose, and when the decision was made the test is what said so. That
    /// is the difference between pinning a decision and pinning an
    /// implementation; the five cases in `docs/port-tracker.md` §15.5 and
    /// §15.4 were the second kind.
    ///
    /// What is left is the one that still has nothing behind it:
    /// `SetFacilityTaxes` would set a tax on a facility nobody can own.
    /// (`DisconnectJob` is served now that `ConnectJob` creates monitors.)
    #[test]
    fn the_remaining_mutators_are_left_unanswered_on_purpose() {
        assert!(
            matches!(
                call("facilityManager", "SetFacilityTaxes", 1),
                Err(ServiceError::NoSuchMethod { .. })
            ),
            "facilityManager.SetFacilityTaxes discards its return value; answering \
             it would report a success that did not happen."
        );
    }

    /// `ACTOR` must really be the fixture's first character, or every test
    /// below silently exercises the no-actor path instead.
    #[test]
    fn the_fixture_actor_is_a_docked_character() {
        let reg = Registry::new(SandboxConfig::default());
        let f = reg
            .industry_facility(Some(ACTOR))
            .expect("the fixture character must be docked and so have a facility");
        assert!(f.facility_id > 0);
        assert!(
            f.online,
            "Job._get_errors raises FACILITY_OFFLINE otherwise"
        );
    }

    /// The two keys that raise on the obvious empty, both inside
    /// `_PrimeFacility` and so both the live lock rather than a blank row.
    ///
    /// `tax: None` falls through `Job._get_errors` to `FACILITY_DENIED` for a
    /// station (category 3, not 65), and an empty `globalModifiers['system']`
    /// is a `KeyError` one line after the `.get` that returned `None`.
    #[test]
    fn a_facility_row_has_a_numeric_tax_and_a_float_for_every_activity() {
        let out = call_as("facilityManager", "GetFacilities", &[]).expect("answered");
        let PyValue::List(rows) = &out else {
            panic!("iterated by the client: {out:?}")
        };
        assert_eq!(rows.len(), 1, "one docked station, one facility");
        let row = &rows[0];

        let tax = dict_get(row, b"tax").expect("tax is read unconditionally");
        assert!(
            matches!(tax, PyValue::Real(_)),
            "tax must be a number, not None: {tax:?}"
        );

        let global = dict_get(row, b"globalModifiers").expect("globalModifiers is subscripted");
        let faction = dict_get(global, b"faction").expect("['faction']");
        assert_eq!(
            faction,
            &PyValue::Real(1.0),
            "anything but 1.0 appends a faction CostModifier"
        );
        let system = dict_get(global, b"system").expect("['system']");
        let PyValue::Dict(system_entries) = system else {
            panic!("subscripted by activity id: {system:?}")
        };

        // Every activity the row *lists* must have a float here, because that
        // is the set the client iterates.
        let activities = dict_get(row, b"activities").expect("activities");
        let PyValue::Dict(activity_entries) = activities else {
            panic!("int-keyed dict: {activities:?}")
        };
        assert!(!activity_entries.is_empty());
        for (_, key) in activity_entries {
            let found = system_entries.iter().find(|(_, k)| k == key);
            let (value, _) = found.unwrap_or_else(|| {
                panic!("activity {key:?} is listed but has no system modifier -- KeyError")
            });
            assert!(
                matches!(value, PyValue::Real(_)),
                "a float, or `not in (0.0, 1.0)` is true and the next line raises: {value:?}"
            );
        }

        // Falsy, or GetFreshFacility re-fetches the facility on every job.
        let rigs = dict_get(row, b"rigModifiers").expect("rigModifiers");
        assert_eq!(rigs, &PyValue::List(vec![]));
    }

    /// `GetFacilityLocations` must be non-empty, and its rows must be
    /// `industry.job.Location` -- a `KeyVal` would compare by address and
    /// every install would raise `INVALID_INPUT_LOCATION`.
    #[test]
    fn a_facility_location_is_a_whitelisted_object_and_not_a_keyval() {
        let out = call_as(
            "facilityManager",
            "GetFacilityLocations",
            &[PyValue::Int(1), PyValue::Int(ACTOR)],
        )
        .expect("answered");
        let PyValue::List(rows) = &out else {
            panic!("len() is taken of this: {out:?}")
        };
        assert!(
            !rows.is_empty(),
            "an empty list means _ApplyJobSettings never runs and the submit \
             button is disabled for ever"
        );
        let PyValue::ObjectEx { form, header, .. } = &rows[0] else {
            panic!("must be an ObjectEx, not a KeyVal instance: {:?}", rows[0])
        };
        assert_eq!(
            *form,
            eve_marshal::ObjectForm::New,
            "0x23: the client's WriteObject picks it by the reduce callable's \
             __name__ being __newobj__"
        );
        let PyValue::Tuple(parts) = &**header else {
            panic!("header is a tuple: {header:?}")
        };
        assert_eq!(parts.len(), 2, "((cls,), state)");
        let PyValue::Tuple(inner) = &parts[0] else {
            panic!("0x23 keeps the class in the INNER tuple: {:?}", parts[0])
        };
        assert_eq!(
            inner[0],
            PyValue::Global(b"industry.job.Location".to_vec()),
            "and it must be the whitelisted name, or the client refuses the stream"
        );
        // The four fields `Location.__eq__` compares, plus the two access
        // flags the BLUEPRINT_ACCESS and INPUT_ACCESS checks read.
        for key in [b"itemID".as_slice(), b"typeID", b"ownerID", b"flagID"] {
            assert!(
                dict_get(&parts[1], key).is_some(),
                "Location.__eq__ compares {} -- a copy must equal its original",
                String::from_utf8_lossy(key)
            );
        }
        assert_eq!(dict_get(&parts[1], b"canView"), Some(&PyValue::Bool(true)));
        assert_eq!(dict_get(&parts[1], b"canTake"), Some(&PyValue::Bool(true)));
    }

    /// A blueprint in the facility's hangar carries the facility's type in
    /// `locationTypeID`, the same number `GetFacilityLocations` sends as the
    /// `Location` row's `typeID`. The client's `Location.__eq__` compares
    /// both, so a 0 here made every blueprint `INVALID_BLUEPRINT_LOCATION`.
    #[test]
    fn a_hangar_blueprint_names_the_facility_type_the_location_row_names() {
        let (mut reg, _station, blueprint_id) = with_a_blueprint(1);
        // an invented, non-zero station type so the check cannot pass on 0
        reg.character_mut(ACTOR)
            .expect("the fixture character")
            .station_type_id = 4242;
        let rows = blueprint_rows(&mut reg);
        let row = rows
            .iter()
            .find(|r| attr(r, b"itemID") == PyValue::Int(blueprint_id))
            .expect("the hangar blueprint is listed");
        let out = call_in(
            &mut reg,
            "facilityManager",
            "GetFacilityLocations",
            &[PyValue::Int(1), PyValue::Int(ACTOR)],
        )
        .expect("answered");
        let PyValue::List(locs) = &out else {
            panic!("a list: {out:?}")
        };
        let PyValue::ObjectEx { header, .. } = &locs[0] else {
            panic!("a Location: {:?}", locs[0])
        };
        let PyValue::Tuple(parts) = &**header else {
            panic!("header is a tuple")
        };
        assert_eq!(dict_get(&parts[1], b"typeID"), Some(&PyValue::Int(4242)));
        assert_eq!(attr(row, b"locationTypeID"), PyValue::Int(4242));
    }

    /// `max({}.values())` is a `ValueError`, so this dict is never empty --
    /// with an actor or without one.
    #[test]
    fn the_job_counts_are_never_empty() {
        for out in [
            call("industryManager", "GetJobCounts", 1).expect("answered"),
            call_as("industryManager", "GetJobCounts", &[PyValue::Int(ACTOR)]).expect("answered"),
        ] {
            let PyValue::Dict(entries) = &out else {
                panic!("int-keyed dict: {out:?}")
            };
            assert_eq!(
                entries.len(),
                crate::industry::ACTIVITIES.len(),
                "all six activities, or the client's max() over the values raises"
            );
        }
    }

    /// A connection that has selected no character is told about nobody's
    /// blueprints, rather than about the first character's.
    ///
    /// The same property `Request::actor` exists for: `None` is a real
    /// answer. The arm reads `req.actor` as the owner fallback, so an actor
    /// of `None` with no explicit owner must produce no rows at all.
    #[test]
    fn a_session_that_is_nobody_owns_no_blueprints() {
        let out =
            call_with("blueprintManager", "GetBlueprintDataByOwner", &[], None).expect("answered");
        let PyValue::Tuple(parts) = &out else {
            panic!("UNPACK_SEQUENCE 2: {out:?}")
        };
        assert_eq!(parts[0], PyValue::List(vec![]));
    }

    /// A job id nobody owns is refused with the client's own id for it, and
    /// not answered with somebody else's job.
    #[test]
    fn a_job_that_is_not_yours_is_refused() {
        match call_as("industryManager", "GetJob", &[PyValue::Int(999_999)]) {
            Err(ServiceError::Raise(_)) => {}
            other => panic!("AssertJobAccess raises a UserError; got {other:?}"),
        }
    }

    /// The blueprint the install tests use: the `industry::Table::fixture`
    /// row with one material and Industry 1, so a reader can check the
    /// arithmetic by hand. Its quantities and times are invented.
    const AFTERBURNER_BLUEPRINT: i64 = 21858;
    const AFTERBURNER: i64 = 21857;
    const TRITANIUM: i64 = 34;

    /// One run's Tritanium at ME 0, read from the fixture row.
    fn tritanium_per_run() -> i64 {
        let content = crate::content::Content::fixture();
        let table: &crate::industry::Table = content.get();
        let bp = table
            .find(AFTERBURNER_BLUEPRINT)
            .expect("the fixture has the afterburner blueprint");
        assert_eq!(
            bp.materials.len(),
            1,
            "one material, so it is the whole bill"
        );
        bp.materials
            .iter()
            .find(|&&(t, _)| t == TRITANIUM)
            .map(|&(_, q)| q)
            .expect("its material is Tritanium")
    }

    /// A registry where [`ACTOR`] holds a blueprint and the minerals to build
    /// from it, in the station they are docked in, and knows Industry.
    ///
    /// **Industry is not a starter skill**, which is worth saying out loud:
    /// `skill::STARTER` is ten rows and 3380 is not among them, so a fresh
    /// character cannot install a manufacturing job until they buy it. That
    /// is correct -- it is also true in retail -- and
    /// [`every_skill_a_blueprint_requires_can_be_bought`] is what makes it a
    /// closed gap rather than an unreachable subsystem, which is the mistake
    /// reprocessing made and had to fix.
    fn with_a_blueprint(runs_of_tritanium: i64) -> (Registry, i64, i64) {
        let mut reg = Registry::new(SandboxConfig::default());
        let station = reg
            .character(ACTOR)
            .and_then(|c| c.station_id)
            .expect("the fixture character is docked");
        reg.skills
            .get_mut(&ACTOR)
            .expect("the fixture character has a skill sheet")
            .grant_in(&reg.content, crate::industry::SKILL_INDUSTRY, 1)
            .expect("Industry is in the content skill table");
        // **And a wallet, because the fixture character's balance is 0.0.**
        // Found by running these tests: every install refused with
        // `ACCOUNT_FUNDS` (19) before a single material was checked. That is
        // correct behaviour and it is worth knowing out loud -- a fresh
        // character cannot install any job at all until somebody gives them
        // ISK, which today means `/giveisk`.
        if let Some(c) = reg.character_mut(ACTOR) {
            c.balance = 1_000_000_000.0;
        }
        let mut edits = vec![crate::item::Edit::Create {
            type_id: AFTERBURNER_BLUEPRINT,
            quantity: 1,
            to: crate::item::Slot::new(ACTOR, station, crate::industry::FLAG_HANGAR),
        }];
        // `runs_of_tritanium == 0` means "no minerals at all", and that is a
        // missing stack rather than a stack of zero: `Store::apply` refuses
        // `BadQuantity` for a zero-quantity `Create`, correctly.
        if runs_of_tritanium > 0 {
            edits.push(crate::item::Edit::Create {
                type_id: TRITANIUM,
                quantity: tritanium_per_run() * runs_of_tritanium,
                to: crate::item::Slot::new(ACTOR, station, crate::industry::FLAG_HANGAR),
            });
        }
        reg.apply_items(&edits)
            .expect("the fixture's own items must apply");
        let blueprint_id = reg
            .store()
            .items()
            .iter()
            .find(|i| i.type_id == AFTERBURNER_BLUEPRINT && i.owner_id == ACTOR)
            .expect("the blueprint we just created")
            .item_id;
        (reg, station, blueprint_id)
    }

    /// `industry.job.Location`, as the `0x23` form with the state dict the
    /// client's `Location` carries.
    fn location(item_id: i64, flag_id: i64) -> PyValue {
        PyValue::ObjectEx {
            form: eve_marshal::ObjectForm::New,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::Tuple(vec![PyValue::Global(b"industry.job.Location".to_vec())]),
                pydict(vec![
                    ("itemID", PyValue::Int(item_id)),
                    ("flagID", PyValue::Int(flag_id)),
                ]),
            ])),
            list: vec![],
            dict: vec![],
        }
    }

    /// One `Job.dump()`, with the five keys that decide what gets built.
    fn install_dump(blueprint_id: i64, station: i64, runs: i64) -> PyValue {
        pydict(vec![
            ("activityID", PyValue::Int(crate::industry::MANUFACTURING)),
            ("blueprintID", PyValue::Int(blueprint_id)),
            ("runs", PyValue::Int(runs)),
            (
                "inputLocation",
                location(station, crate::industry::FLAG_HANGAR),
            ),
            (
                "outputLocation",
                location(station, crate::industry::FLAG_HANGAR),
            ),
        ])
    }

    fn call_in(
        reg: &mut Registry,
        service: &str,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        dispatch(
            reg,
            &Request {
                service,
                method,
                args,
                kwargs: &[],
                bind_params: None,
                actor: Some(ACTOR),
            },
        )
    }

    /// How much of a type [`ACTOR`] holds.
    fn held(reg: &Registry, type_id: i64) -> i64 {
        reg.store()
            .items()
            .iter()
            .filter(|i| i.owner_id == ACTOR && i.type_id == type_id)
            .map(|i| i.quantity)
            .sum()
    }

    /// The `industry.Error` codes inside a raised `IndustryValidationError`.
    ///
    /// Reads the payload the way the client does -- `args[1]['errors']`, then
    /// each element's `[0]` -- so a shape the tooltip could not unpack fails
    /// here rather than in a player's client.
    fn validation_codes(e: &ServiceError) -> Vec<i64> {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            panic!("not a raised exception: {e:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("a 0x22 header is a tuple")
        };
        assert_eq!(
            parts[0],
            PyValue::Global(b"eveexceptions.UserError".to_vec()),
            "the client catches UserError and nothing else here"
        );
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("UserError's header args are a tuple")
        };
        assert_eq!(
            args[0],
            PyValue::Bytes(b"IndustryValidationError".to_vec()),
            "`getattr(exception, 'msg', None) == 'IndustryValidationError'` \
             is what routes this to the tooltip"
        );
        let errors =
            crate::pyutil::dict_get(&args[1], b"errors").expect("`exception.args[1]['errors']`");
        let PyValue::List(rows) = errors else {
            panic!("the client iterates `errors`: {errors:?}")
        };
        rows.iter()
            .map(|row| {
                // `for error, errorArgs in errors:` -- UNPACK_SEQUENCE 2.
                let PyValue::Tuple(pair) = row else {
                    panic!("each error is a 2-sequence: {row:?}")
                };
                assert_eq!(pair.len(), 2, "UNPACK_SEQUENCE 2 in Reconstruct");
                assert!(
                    matches!(&pair[1], PyValue::Tuple(_) | PyValue::List(_)),
                    "errorArgs is splatted with CALL_FUNCTION_VAR, so it must iterate"
                );
                let PyValue::ObjectEx { header, .. } = &pair[0] else {
                    panic!("element 0 is the enum member: {:?}", pair[0])
                };
                let PyValue::Tuple(h) = header.as_ref() else {
                    panic!("a 0x22 header is a tuple")
                };
                assert_eq!(
                    h[0],
                    PyValue::Global(b"industry.const.Error".to_vec()),
                    "`error[0].value` needs a real enum member, not an int"
                );
                let PyValue::Tuple(ctor) = &h[1] else {
                    panic!("`Error(code)` takes one positional argument")
                };
                let PyValue::Int(code) = ctor[0] else {
                    panic!("the code is an int")
                };
                code
            })
            .collect()
    }

    /// The `msg` of a raised `UserError`.
    fn user_error_msg(e: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            panic!("not a raised exception: {e:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("a 0x22 header is a tuple")
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("UserError's header args are a tuple")
        };
        match &args[0] {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            other => panic!("the message id is a str: {other:?}"),
        }
    }

    /// Finish a job's clock without waiting for it.
    ///
    /// Rewrites `end_date` into the past and commits the job back, which is
    /// what `Jobs::restore` will do when persistence lands. A job's clock is
    /// not a wait a test gets to take.
    fn finish_the_clock(reg: &mut Registry, job_id: i64) {
        let mut job = *reg.jobs().get(job_id).expect("the job we just installed");
        job.end_date = 1;
        reg.jobs_mut().commit(job);
    }

    /// The whole install, as the window drives it: materials leave, the job
    /// exists, and the push that is the only thing the client reads goes out.
    #[test]
    fn installing_a_job_consumes_the_materials_and_announces_the_job() {
        let (mut reg, station, blueprint_id) = with_a_blueprint(3);
        let before = held(&reg, TRITANIUM);
        let answer = call_in(
            &mut reg,
            "industryManager",
            "InstallJob",
            &[install_dump(blueprint_id, station, 2)],
        )
        .expect("a funded, skilled, stocked install");
        let PyValue::Int(job_id) = answer else {
            panic!("InstallJob answers the job id: {answer:?}")
        };
        assert_eq!(
            held(&reg, TRITANIUM),
            before - tritanium_per_run() * 2,
            "two runs at ME 0 cost two runs' worth of Tritanium"
        );
        let job = reg.jobs().get(job_id).expect("the job is in the world");
        assert_eq!(job.runs, 2);
        assert_eq!(job.product_type_id, AFTERBURNER);
        assert_eq!(
            job.blueprint_id, blueprint_id,
            "the blueprint stays where it is and the client learns it is busy \
             from `blueprint.jobID`"
        );
        // On the wire, not in the queue: `jobID` is the **first** argument
        // of `OnIndustryJob`, and a test that matched the queued type would
        // pass with the six arguments in any order.
        assert!(
            reg.take_pending().iter().any(|(_, n)| {
                let (event, args) = n.to_push();
                event == "OnIndustryJob" && args[0] == PyValue::Int(job_id)
            }),
            "the return value is discarded by ClickFunc, so the OnIndustryJob \
             push is the only thing that puts the job in the window"
        );
    }

    /// The fee leaves the wallet and lands in the journal.
    ///
    /// `docs/port-tracker.md` §15.6 carried this as open, because charging
    /// needed a public debit on `Registry` and there was none.
    #[test]
    fn the_install_fee_is_charged_and_journalled() {
        let (mut reg, station, blueprint_id) = with_a_blueprint(1);
        let before = reg.character(ACTOR).map(|c| c.balance).expect("a wallet");
        let entries_before = reg.journal().entries().len();
        let answer = call_in(
            &mut reg,
            "industryManager",
            "InstallJob",
            &[install_dump(blueprint_id, station, 1)],
        )
        .expect("the install");
        let PyValue::Int(job_id) = answer else {
            panic!("the job id")
        };
        let cost = reg.jobs().get(job_id).expect("the job").cost;
        assert!(cost > 0, "an EIV-priced job is not free");
        assert_eq!(
            reg.character(ACTOR).map(|c| c.balance),
            Some(before - cost as f64),
            "the balance is the one thing a player checks"
        );
        let new: Vec<_> = reg.journal().entries()[entries_before..].to_vec();
        assert_eq!(new.len(), 1, "one debit, one row");
        assert_eq!(
            new[0].entry_type_id, 56,
            "56 is `refIndustryJobInstallation`, from appConst"
        );
        assert_eq!(new[0].amount, -(cost as f64), "a debit is negative");
        assert_eq!(new[0].reference_id, job_id, "the row names the job");
    }

    /// A job nobody can afford the materials for is refused in the shape the
    /// tooltip reads, and **nothing moves**.
    #[test]
    fn an_install_without_the_materials_refuses_and_changes_nothing() {
        let (mut reg, station, blueprint_id) = with_a_blueprint(1);
        let before = held(&reg, TRITANIUM);
        let balance = reg.character(ACTOR).map(|c| c.balance);
        let e = call_in(
            &mut reg,
            "industryManager",
            "InstallJob",
            &[install_dump(blueprint_id, station, 5)],
        )
        .expect_err("five runs' worth of Tritanium is not there");
        assert_eq!(
            validation_codes(&e),
            vec![16],
            "16 is MISSING_MATERIAL, which is the row the tooltip draws"
        );
        assert_eq!(
            held(&reg, TRITANIUM),
            before,
            "a refused plan consumes nothing"
        );
        assert_eq!(reg.character(ACTOR).map(|c| c.balance), balance);
        assert!(reg.jobs().is_empty(), "and leaves no job behind");
    }

    /// Materials in another station's hangar cannot be reached.
    ///
    /// The hole reprocessing had and closed: the client caches other
    /// stations' inventories, so an `itemID` from one of them is a real id
    /// that a naive owner-only filter would happily consume.
    #[test]
    fn materials_in_another_station_are_not_on_hand() {
        let (mut reg, station, blueprint_id) = with_a_blueprint(0);
        let elsewhere = station + 1;
        reg.apply_items(&[crate::item::Edit::Create {
            type_id: TRITANIUM,
            quantity: tritanium_per_run() * 10,
            to: crate::item::Slot::new(ACTOR, elsewhere, crate::industry::FLAG_HANGAR),
        }])
        .expect("stock in the other station");
        let e = call_in(
            &mut reg,
            "industryManager",
            "InstallJob",
            &[install_dump(blueprint_id, station, 1)],
        )
        .expect_err("the minerals are in the wrong station");
        assert_eq!(validation_codes(&e), vec![16]);
        assert_eq!(
            held(&reg, TRITANIUM),
            tritanium_per_run() * 10,
            "and the remote stack is untouched"
        );
    }

    /// Somebody else's blueprint is not a blueprint you may install.
    #[test]
    fn a_blueprint_that_is_not_yours_is_refused() {
        let (mut reg, station, _) = with_a_blueprint(1);
        let other = 90_000_002;
        reg.apply_items(&[crate::item::Edit::Create {
            type_id: AFTERBURNER_BLUEPRINT,
            quantity: 1,
            to: crate::item::Slot::new(other, station, crate::industry::FLAG_HANGAR),
        }])
        .expect("the other character's blueprint");
        let theirs = reg
            .store()
            .items()
            .iter()
            .find(|i| i.owner_id == other && i.type_id == AFTERBURNER_BLUEPRINT)
            .expect("just created")
            .item_id;
        let e = call_in(
            &mut reg,
            "industryManager",
            "InstallJob",
            &[install_dump(theirs, station, 1)],
        )
        .expect_err("that blueprint belongs to somebody else");
        assert_eq!(user_error_msg(&e), "IndustryBlueprintAccessDenied");
        assert!(reg.jobs().is_empty());
    }

    /// A job cannot be delivered until its own clock has run out, and then it
    /// can, and the product appears where it was asked for.
    #[test]
    fn a_job_delivers_only_after_its_clock_runs_out() {
        let (mut reg, station, blueprint_id) = with_a_blueprint(1);
        let PyValue::Int(job_id) = call_in(
            &mut reg,
            "industryManager",
            "InstallJob",
            &[install_dump(blueprint_id, station, 1)],
        )
        .expect("the install") else {
            panic!("the job id")
        };
        let early = call_in(
            &mut reg,
            "industryManager",
            "CompleteJob",
            &[PyValue::Int(job_id), PyValue::Int(30_000_001)],
        )
        .expect_err("the job's time has not passed");
        assert_eq!(user_error_msg(&early), "IndustryJobNotReady");
        assert_eq!(held(&reg, AFTERBURNER), 0, "and nothing was produced");

        finish_the_clock(&mut reg, job_id);
        let _ = reg.take_pending();
        call_in(
            &mut reg,
            "industryManager",
            "CompleteJob",
            &[PyValue::Int(job_id), PyValue::Int(30_000_001)],
        )
        .expect("a ready job delivers");
        assert_eq!(held(&reg, AFTERBURNER), 1, "one run, one afterburner");
        assert_eq!(
            reg.jobs().get(job_id).map(|j| j.status),
            Some(crate::industry::Status::Delivered),
            "Delivered, not Completed: somebody collected it"
        );
        // `successfulRuns` is the **sixth** argument, and `None` on every
        // push the client generates for itself -- so "not None" is the
        // property, in the slot the handler reads it from.
        assert!(
            reg.take_pending().iter().any(|(_, n)| {
                let (event, args) = n.to_push();
                event == "OnIndustryJob" && args[5] != PyValue::None
            }),
            "delivery is the one push that carries a real successfulRuns"
        );
        let again = call_in(
            &mut reg,
            "industryManager",
            "CompleteJob",
            &[PyValue::Int(job_id), PyValue::Int(30_000_001)],
        )
        .expect_err("a delivered job cannot be delivered twice");
        assert_eq!(user_error_msg(&again), "IndustryJobAlreadyCompleted");
        assert_eq!(held(&reg, AFTERBURNER), 1, "and no second afterburner");
    }

    /// "Deliver all" delivers the ones that are ready and does **not** fail
    /// the batch for the ones that are not.
    #[test]
    fn deliver_all_delivers_what_it_can_and_still_answers() {
        let (mut reg, station, blueprint_id) = with_a_blueprint(1);
        let PyValue::Int(ready) = call_in(
            &mut reg,
            "industryManager",
            "InstallJob",
            &[install_dump(blueprint_id, station, 1)],
        )
        .expect("the install") else {
            panic!("the job id")
        };
        finish_the_clock(&mut reg, ready);
        let not_a_job = ready + 999;
        let answer = call_in(
            &mut reg,
            "industryManager",
            "CompleteManyJobs",
            &[PyValue::List(vec![
                PyValue::Tuple(vec![PyValue::Int(ready), PyValue::Int(30_000_001)]),
                PyValue::Tuple(vec![PyValue::Int(not_a_job), PyValue::Int(30_000_001)]),
            ])],
        )
        .expect("one bad id must not cost the good one its delivery");
        assert_eq!(answer, PyValue::None);
        assert_eq!(held(&reg, AFTERBURNER), 1);
    }

    /// When nothing at all could be delivered, the player gets a log line
    /// rather than silence.
    #[test]
    fn deliver_all_with_nothing_deliverable_raises() {
        let (mut reg, _, _) = with_a_blueprint(0);
        let e = call_in(
            &mut reg,
            "industryManager",
            "CompleteManyJobs",
            &[PyValue::List(vec![PyValue::Tuple(vec![
                PyValue::Int(1),
                PyValue::Int(30_000_001),
            ])])],
        )
        .expect_err("no job of ours is in that list");
        assert_eq!(user_error_msg(&e), "IndustryJobNotFound");
    }

    /// Cancelling does not refund the materials (our rule; whether retail does is open).
    #[test]
    fn cancelling_a_job_does_not_give_the_materials_back() {
        let (mut reg, station, blueprint_id) = with_a_blueprint(1);
        let PyValue::Int(job_id) = call_in(
            &mut reg,
            "industryManager",
            "InstallJob",
            &[install_dump(blueprint_id, station, 1)],
        )
        .expect("the install") else {
            panic!("the job id")
        };
        assert_eq!(held(&reg, TRITANIUM), 0, "the install consumed the stack");
        call_in(
            &mut reg,
            "industryManager",
            "CancelJob",
            &[PyValue::Int(job_id), PyValue::Int(30_000_001)],
        )
        .expect("a live job can be stopped");
        assert_eq!(
            reg.jobs().get(job_id).map(|j| j.status),
            Some(crate::industry::Status::Cancelled)
        );
        assert_eq!(
            held(&reg, TRITANIUM),
            0,
            "our rule: the materials stay spent (`Jobs::plan_cancel` edits nothing)"
        );
        assert_eq!(held(&reg, AFTERBURNER), 0, "and nothing was produced");
    }

    /// Another character's job is neither readable nor writable.
    ///
    /// The same message id `GetJob` answers with, so there is no route by
    /// which a job is private to read and public to deliver.
    #[test]
    fn another_characters_job_cannot_be_delivered_or_cancelled() {
        let (mut reg, station, blueprint_id) = with_a_blueprint(1);
        let PyValue::Int(job_id) = call_in(
            &mut reg,
            "industryManager",
            "InstallJob",
            &[install_dump(blueprint_id, station, 1)],
        )
        .expect("the install") else {
            panic!("the job id")
        };
        finish_the_clock(&mut reg, job_id);
        for method in ["CompleteJob", "CancelJob", "GetJob"] {
            let e = dispatch(
                &mut reg,
                &Request {
                    service: "industryManager",
                    method,
                    args: &[PyValue::Int(job_id), PyValue::Int(30_000_001)],
                    kwargs: &[],
                    bind_params: None,
                    actor: Some(90_000_002),
                },
            )
            .expect_err("that is not their job");
            assert_eq!(
                user_error_msg(&e),
                "IndustryJobAccessDenied",
                "{method} must refuse a job the caller does not own"
            );
        }
        assert_eq!(held(&reg, AFTERBURNER), 0, "and nothing was delivered");
    }

    /// A mutator called by a connection that is nobody refuses rather than
    /// answering `None`.
    ///
    /// This is what is left of `the_job_mutators_are_left_unanswered_on_purpose`,
    /// and it is the half of that test that was a property rather than a
    /// decision: three of the four discard their return value, so answering
    /// `None` reports a success that did not happen.
    #[test]
    fn a_mutator_never_reports_success_to_a_session_that_is_nobody() {
        for method in ["InstallJob", "CompleteJob", "CompleteManyJobs", "CancelJob"] {
            let e = call("industryManager", method, 1)
                .expect_err("a session with no character has no jobs");
            assert_eq!(
                user_error_msg(&e),
                "IndustryJobAccessDenied",
                "{method} answered a session that is nobody"
            );
        }
    }

    /// Every skill a blueprint requires is one a character can obtain.
    ///
    /// **Reprocessing shipped unreachable for exactly this reason** -- the
    /// yield depended on three skills that were not in `skill::SKILLS`, so
    /// nobody could train them and every refine was the bare station rate.
    /// It runs over the fixture rows (plan §1.5), so it guards the fixture;
    /// a bundle blueprint whose skill nobody can train is refused
    /// `MissingSkill` at install, by name. Both tables come from the same
    /// `Content` (CP59b), as they do in a server.
    #[test]
    fn every_skill_a_blueprint_requires_can_be_bought() {
        let content = crate::content::Content::fixture();
        let table: &crate::industry::Table = content.get();
        for bp in table.blueprints.iter() {
            for &(skill, level) in &bp.skills {
                assert!(
                    crate::skill::find_in(&content, skill).is_some(),
                    "blueprint {} needs skill {skill} at level {level}, which is not in \
                     the skill table -- so the job can never be installed by anybody",
                    bp.blueprint_type_id
                );
            }
        }
    }

    /// An `industry.job.Location` is read out of either ObjectEx form, out of
    /// the `__setitem__` stream, and out of a plain dict.
    ///
    /// **Which tag the client uses is the one thing in this subsystem that
    /// only a capture settles** (`docs/lanes/industry.md` §8), so the parser
    /// accepts all of them and this pins that it really does. The property is
    /// "the address survives every form the client could have written",
    /// not "the parser has four branches".
    #[test]
    fn a_location_is_read_from_either_objectex_form() {
        let state = pydict(vec![
            ("itemID", PyValue::Int(60_012_526)),
            ("typeID", PyValue::Int(2502)),
            ("flagID", PyValue::Int(4)),
        ]);
        let new_form = PyValue::ObjectEx {
            form: eve_marshal::ObjectForm::New,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::Tuple(vec![PyValue::Global(b"industry.job.Location".to_vec())]),
                state.clone(),
            ])),
            list: vec![],
            dict: vec![],
        };
        let call_form = PyValue::ObjectEx {
            form: eve_marshal::ObjectForm::Call,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::Global(b"industry.job.Location".to_vec()),
                PyValue::Tuple(vec![]),
                state.clone(),
            ])),
            list: vec![],
            dict: vec![],
        };
        // The `__setitem__` stream is **(key, value)**, the opposite order to
        // a marshal dict -- which is why it has its own reader.
        let setitem_form = PyValue::ObjectEx {
            form: eve_marshal::ObjectForm::New,
            header: Box::new(PyValue::Tuple(vec![PyValue::Tuple(vec![PyValue::Global(
                b"industry.job.Location".to_vec(),
            )])])),
            list: vec![],
            dict: vec![
                (bstr("itemID"), PyValue::Int(60_012_526)),
                (bstr("flagID"), PyValue::Int(4)),
            ],
        };
        for (what, v) in [
            ("0x23 new", new_form),
            ("0x22 call", call_form),
            ("__setitem__", setitem_form),
            ("a plain dict", state),
            (
                "a KeyVal",
                keyval(vec![
                    ("itemID", PyValue::Int(60_012_526)),
                    ("flagID", PyValue::Int(4)),
                ]),
            ),
        ] {
            assert_eq!(
                location_of(&v),
                Some((60_012_526, 4)),
                "{what} must yield the same address"
            );
        }
        assert_eq!(
            location_of(&PyValue::Int(60_012_526)),
            None,
            "a bare id carries no flag, and guessing one would put a player's \
             work in a division they did not choose"
        );
    }

    /// A request we cannot parse is refused, and refused as the right kind of
    /// error: the two location keys get a tooltip row, everything else gets
    /// the install error the client has a modal for.
    #[test]
    fn an_unreadable_install_request_is_refused_by_kind() {
        assert_eq!(
            user_error_msg(&install_request(None).expect_err("no argument at all")),
            "IndustryJobInstallError"
        );
        let no_runs = pydict(vec![
            ("activityID", PyValue::Int(1)),
            ("blueprintID", PyValue::Int(100_000_001)),
        ]);
        assert_eq!(
            user_error_msg(&install_request(Some(&no_runs)).expect_err("`runs` is required")),
            "IndustryJobInstallError"
        );
        let no_output = pydict(vec![
            ("activityID", PyValue::Int(1)),
            ("blueprintID", PyValue::Int(100_000_001)),
            ("runs", PyValue::Int(1)),
            ("inputLocation", location(60_012_526, 4)),
        ]);
        assert_eq!(
            validation_codes(
                &install_request(Some(&no_output)).expect_err("`outputLocation` is required")
            ),
            vec![18],
            "18 is MISSING_OUTPUT_LOCATION, and the tooltip can say so"
        );
    }

    // -----------------------------------------------------------------
    // Round 2: research and copying, through the wire.
    // -----------------------------------------------------------------

    /// One `Job.dump()` for a science activity. `licensedRuns` is sent the
    /// way the client sends it -- present for every activity, read only by
    /// copying.
    fn science_dump(
        blueprint_id: i64,
        station: i64,
        activity: i64,
        runs: i64,
        licensed: i64,
    ) -> PyValue {
        pydict(vec![
            ("activityID", PyValue::Int(activity)),
            ("blueprintID", PyValue::Int(blueprint_id)),
            ("runs", PyValue::Int(runs)),
            ("licensedRuns", PyValue::Int(licensed)),
            ("productTypeID", PyValue::None),
            (
                "inputLocation",
                location(station, crate::industry::FLAG_HANGAR),
            ),
            (
                "outputLocation",
                location(station, crate::industry::FLAG_HANGAR),
            ),
        ])
    }

    /// An attribute of one `BlueprintInstance` row, which is a `KeyVal`.
    fn attr(row: &PyValue, key: &[u8]) -> PyValue {
        let PyValue::Instance { state, .. } = row else {
            panic!("a blueprint row is a KeyVal: {row:?}")
        };
        dict_get(state, key)
            .cloned()
            .unwrap_or_else(|| panic!("no {:?} on {row:?}", String::from_utf8_lossy(key)))
    }

    /// `GetBlueprintDataByOwner`'s rows for [`ACTOR`].
    fn blueprint_rows(reg: &mut Registry) -> Vec<PyValue> {
        let out = call_in(
            reg,
            "blueprintManager",
            "GetBlueprintDataByOwner",
            &[PyValue::Int(ACTOR), PyValue::None],
        )
        .expect("the blueprint list");
        let PyValue::Tuple(parts) = out else {
            panic!("a 2-tuple")
        };
        let PyValue::List(rows) = &parts[0] else {
            panic!("element 0 is a list")
        };
        rows.clone()
    }

    fn installed(answer: Result<PyValue, ServiceError>) -> i64 {
        match answer {
            Ok(PyValue::Int(job_id)) => job_id,
            other => panic!("InstallJob answers the job id: {other:?}"),
        }
    }

    /// The research path end to end: install from the window's dump, deliver,
    /// see the new ME on the blueprint row the window reads, see the push
    /// that makes the window re-read, and keep it across a restart.
    #[test]
    fn a_researched_blueprint_shows_its_me_and_survives_a_restart() {
        let (mut reg, station, blueprint_id) = with_a_blueprint(0);
        let job_id = installed(call_in(
            &mut reg,
            "industryManager",
            "InstallJob",
            &[science_dump(
                blueprint_id,
                station,
                crate::industry::RESEARCH_MATERIAL,
                2,
                0,
            )],
        ));
        assert_eq!(
            reg.jobs().get(job_id).map(|j| j.activity_id),
            Some(crate::industry::RESEARCH_MATERIAL)
        );
        finish_the_clock(&mut reg, job_id);
        let _ = reg.take_pending();
        call_in(
            &mut reg,
            "industryManager",
            "CompleteJob",
            &[PyValue::Int(job_id), PyValue::Int(30_000_001)],
        )
        .expect("a ready research job delivers");
        assert!(
            reg.take_pending().iter().any(|(_, n)| {
                let (event, args) = n.to_push();
                event == "OnBlueprintsUpdated" && args == vec![PyValue::Int(ACTOR)]
            }),
            "OnIndustryJob never re-reads a blueprint, so this push is the \
             only reason the window shows the new ME"
        );
        let me_of = |reg: &mut Registry| {
            blueprint_rows(reg)
                .iter()
                .find(|r| attr(r, b"itemID") == PyValue::Int(blueprint_id))
                .map(|r| (attr(r, b"materialEfficiency"), attr(r, b"quantity")))
        };
        assert_eq!(
            me_of(&mut reg),
            Some((PyValue::Int(2), PyValue::Int(-1))),
            "ME 2 and still an original"
        );

        let snap = reg.to_snapshot();
        let mut back = Registry::new(SandboxConfig::default());
        back.apply_snapshot(snap);
        assert_eq!(
            me_of(&mut back),
            Some((PyValue::Int(2), PyValue::Int(-1))),
            "research is a promise kept: it survives the world file"
        );
    }

    /// A copy job mints copies the window lists **as copies**: `quantity -2`
    /// and the licence in `runs`, one row per copy.
    #[test]
    fn a_copy_job_mints_copies_the_window_lists_as_copies() {
        let (mut reg, station, blueprint_id) = with_a_blueprint(0);
        let job_id = installed(call_in(
            &mut reg,
            "industryManager",
            "InstallJob",
            &[science_dump(
                blueprint_id,
                station,
                crate::industry::COPYING,
                2,
                3,
            )],
        ));
        finish_the_clock(&mut reg, job_id);
        call_in(
            &mut reg,
            "industryManager",
            "CompleteJob",
            &[PyValue::Int(job_id), PyValue::Int(30_000_001)],
        )
        .expect("a ready copy job delivers");
        let rows = blueprint_rows(&mut reg);
        let copies: Vec<&PyValue> = rows
            .iter()
            .filter(|r| attr(r, b"itemID") != PyValue::Int(blueprint_id))
            .collect();
        assert_eq!(copies.len(), 2, "two copies, plus the original");
        for c in copies {
            assert_eq!(attr(c, b"typeID"), PyValue::Int(AFTERBURNER_BLUEPRINT));
            assert_eq!(attr(c, b"quantity"), PyValue::Int(-2), "the mark of a copy");
            assert_eq!(attr(c, b"runs"), PyValue::Int(3), "the licence");
        }
    }

    // -----------------------------------------------------------------
    // MN-A: reactions at the station, over the dispatcher.
    // -----------------------------------------------------------------

    use crate::reactions::{
        FIXTURE_ALLOY, FIXTURE_FORMULA, FIXTURE_FORMULA_TWO, FIXTURE_FUEL, FIXTURE_GOO,
        FIXTURE_MINERAL_A, FIXTURE_MINERAL_B, SKILL_MASS_REACTIONS, SKILL_REACTIONS,
    };

    /// A registry whose content knows the invented formulas, where [`ACTOR`]
    /// holds [`FIXTURE_FORMULA`] and enough inputs for `runs` runs at the
    /// station they are docked in, with Reactions trained to `level`.
    fn with_a_formula(runs: i64, level: i64) -> (Registry, i64, i64) {
        let mut reg = Registry::new(SandboxConfig::default())
            .with_content(std::sync::Arc::new(crate::reactions::test_content()));
        let station = reg
            .character(ACTOR)
            .and_then(|c| c.station_id)
            .expect("the fixture character is docked");
        let sheet = vec![(SKILL_REACTIONS, level), (SKILL_MASS_REACTIONS, 0)];
        *reg.skills
            .get_mut(&ACTOR)
            .expect("the fixture character has a skill sheet") = crate::skill::Skills::restore(
            sheet
                .into_iter()
                .map(|(type_id, level)| crate::skill::SkillEntry {
                    type_id,
                    level,
                    points: 0,
                    rank: 1,
                })
                .collect(),
        );
        if let Some(c) = reg.character_mut(ACTOR) {
            c.balance = 1_000_000_000.0;
        }
        let at = crate::item::Slot::new(ACTOR, station, crate::industry::FLAG_HANGAR);
        let mut edits = vec![crate::item::Edit::Create {
            type_id: FIXTURE_FORMULA,
            quantity: 1,
            to: at,
        }];
        let table: &crate::industry::Table = reg.content().get();
        let formula = table.find_reaction(FIXTURE_FORMULA).expect("the formula");
        if runs > 0 {
            for &(type_id, per_run) in &formula.materials {
                edits.push(crate::item::Edit::Create {
                    type_id,
                    quantity: per_run * runs,
                    to: at,
                });
            }
        }
        reg.apply_items(&edits).expect("the fixture's items apply");
        let formula_id = reg
            .store()
            .items()
            .iter()
            .find(|i| i.type_id == FIXTURE_FORMULA && i.owner_id == ACTOR)
            .expect("the formula we just created")
            .item_id;
        (reg, station, formula_id)
    }

    fn react(
        reg: &mut Registry,
        station: i64,
        formula_id: i64,
        runs: i64,
    ) -> Result<PyValue, ServiceError> {
        call_in(
            reg,
            "industryManager",
            "InstallJob",
            &[science_dump(
                formula_id,
                station,
                crate::industry::REACTION,
                runs,
                0,
            )],
        )
    }

    /// **The packet's socket-shaped test**: a pilot installs a reaction at a
    /// station and collects the output. Inputs leave, the fee is journalled
    /// as a reaction fee, the formula stays, the product lands, and the
    /// window's lists agree at each step.
    #[test]
    fn a_pilot_installs_a_reaction_at_a_station_and_collects_the_output() {
        let runs = 3;
        let (mut reg, station, formula_id) = with_a_formula(runs, 3);
        let fuel = held(&reg, FIXTURE_FUEL);
        assert_eq!(fuel, 5 * runs);

        // The formula is a blueprint row: an unresearched original, free.
        let rows = blueprint_rows(&mut reg);
        let row = rows
            .iter()
            .find(|r| attr(r, b"itemID") == PyValue::Int(formula_id))
            .expect("the formula is listed");
        assert_eq!(attr(row, b"typeID"), PyValue::Int(FIXTURE_FORMULA));
        assert_eq!(attr(row, b"quantity"), PyValue::Int(-1), "an original");
        assert_eq!(attr(row, b"runs"), PyValue::Int(-1));
        assert_eq!(attr(row, b"materialEfficiency"), PyValue::Int(0));
        assert_eq!(attr(row, b"jobID"), PyValue::None);
        // And the count dict has a reaction entry the client can sum.
        let counts = call_in(
            &mut reg,
            "blueprintManager",
            "GetBlueprintDataByOwner",
            &[PyValue::Int(ACTOR), PyValue::None],
        )
        .expect("the list");
        let PyValue::Tuple(parts) = counts else {
            panic!("2-tuple")
        };
        let PyValue::Dict(pairs) = &parts[1] else {
            panic!("a dict")
        };
        let total: i64 = pairs
            .iter()
            .map(|(v, _)| match v {
                PyValue::Int(n) => *n,
                other => panic!("an int count: {other:?}"),
            })
            .sum();
        assert_eq!(total, 1, "one blueprint in all");
        assert!(
            pairs.contains(&(PyValue::Int(1), PyValue::Int(crate::industry::REACTION))),
            "the formula counts under the reaction activity: {pairs:?}"
        );

        // Install.
        let balance = reg.character(ACTOR).map(|c| c.balance).expect("wallet");
        let entries_before = reg.journal().entries().len();
        let _ = reg.take_pending();
        let job_id = installed(react(&mut reg, station, formula_id, runs));
        let job = *reg.jobs().get(job_id).expect("the job");
        assert_eq!(job.activity_id, crate::industry::REACTION);
        assert_eq!(job.product_type_id, FIXTURE_GOO);
        assert_eq!(job.runs, runs);
        assert_eq!(job.blueprint_id, formula_id);
        assert!(!job.blueprint_copy);
        assert_eq!(held(&reg, FIXTURE_FUEL), 0, "the fuel blocks are burned");
        assert_eq!(held(&reg, FIXTURE_MINERAL_A), 0);
        assert_eq!(held(&reg, FIXTURE_MINERAL_B), 0);
        assert_eq!(held(&reg, FIXTURE_FORMULA), 1, "the formula is kept");
        assert!(job.cost > 0, "an EIV-priced job is not free");
        assert_eq!(
            reg.character(ACTOR).map(|c| c.balance),
            Some(balance - job.cost as f64)
        );
        let new: Vec<_> = reg.journal().entries()[entries_before..].to_vec();
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].entry_type_id, 135, "refReaction");
        assert_eq!(new[0].amount, -(job.cost as f64));
        assert_eq!(new[0].reference_id, job_id);
        assert_eq!(
            crate::econ_ledger::classify(&new[0]),
            crate::econ_ledger::Class::Sink,
            "the fee is a sink, and the classification stays total"
        );
        assert!(
            reg.take_pending().iter().any(|(_, n)| {
                let (event, args) = n.to_push();
                event == "OnIndustryJob" && args[0] == PyValue::Int(job_id)
            }),
            "the window learns of the job by its push"
        );
        // The formula now says it is busy.
        let busy = blueprint_rows(&mut reg)
            .into_iter()
            .find(|r| attr(r, b"itemID") == PyValue::Int(formula_id))
            .expect("listed");
        assert_eq!(attr(&busy, b"jobID"), PyValue::Int(job_id));
        // The jobs list reads it back as a reaction.
        let listed = call_in(
            &mut reg,
            "industryManager",
            "GetJobsByOwner",
            &[PyValue::Int(ACTOR), PyValue::Bool(false)],
        )
        .expect("the job list");
        let PyValue::List(jobs) = listed else {
            panic!("a list")
        };
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            attr(&jobs[0], b"activityID"),
            PyValue::Int(crate::industry::REACTION)
        );

        // Early is refused, then delivered.
        let early = call_in(
            &mut reg,
            "industryManager",
            "CompleteJob",
            &[PyValue::Int(job_id), PyValue::Int(30_000_001)],
        )
        .expect_err("not yet");
        assert_eq!(user_error_msg(&early), "IndustryJobNotReady");
        assert_eq!(held(&reg, FIXTURE_GOO), 0);
        finish_the_clock(&mut reg, job_id);
        let _ = reg.take_pending();
        call_in(
            &mut reg,
            "industryManager",
            "CompleteJob",
            &[PyValue::Int(job_id), PyValue::Int(30_000_001)],
        )
        .expect("a ready reaction delivers");
        assert_eq!(held(&reg, FIXTURE_GOO), 20 * runs, "runs x quantity");
        assert_eq!(
            reg.jobs().get(job_id).map(|j| j.status),
            Some(crate::industry::Status::Delivered)
        );
        let pushes = reg.take_pending();
        assert!(
            pushes.iter().any(|(_, n)| n.to_push().0 == "OnIndustryJob"),
            "delivery pushes the finished job"
        );
        assert!(
            !pushes
                .iter()
                .any(|(_, n)| n.to_push().0 == "OnBlueprintsUpdated"),
            "a reaction changes no blueprint row"
        );
        // The formula is free again and can be run a second time.
        let after = blueprint_rows(&mut reg)
            .into_iter()
            .find(|r| attr(r, b"itemID") == PyValue::Int(formula_id))
            .expect("still listed");
        assert_eq!(attr(&after, b"jobID"), PyValue::None);
        let again = call_in(
            &mut reg,
            "industryManager",
            "CompleteJob",
            &[PyValue::Int(job_id), PyValue::Int(30_000_001)],
        )
        .expect_err("not twice");
        assert_eq!(user_error_msg(&again), "IndustryJobAlreadyCompleted");
        assert_eq!(held(&reg, FIXTURE_GOO), 20 * runs);
    }

    /// The chain a T3 build walks: the first stage's product is the second
    /// stage's input, through two installs and two deliveries.
    #[test]
    fn a_reaction_chain_runs_through_two_formulas() {
        let runs = 2;
        let (mut reg, station, first) = with_a_formula(runs, 3);
        let at = crate::item::Slot::new(ACTOR, station, crate::industry::FLAG_HANGAR);
        reg.apply_items(&[
            crate::item::Edit::Create {
                type_id: FIXTURE_FORMULA_TWO,
                quantity: 1,
                to: at,
            },
            // The second stage's own fuel (the first stage burned its own).
            crate::item::Edit::Create {
                type_id: FIXTURE_FUEL,
                quantity: 5 * 4,
                to: at,
            },
        ])
        .expect("the second formula");
        let second = reg
            .store()
            .items()
            .iter()
            .find(|i| i.type_id == FIXTURE_FORMULA_TWO && i.owner_id == ACTOR)
            .expect("the second formula")
            .item_id;
        let one = installed(react(&mut reg, station, first, runs));
        finish_the_clock(&mut reg, one);
        call_in(
            &mut reg,
            "industryManager",
            "CompleteJob",
            &[PyValue::Int(one), PyValue::Int(30_000_001)],
        )
        .expect("stage one delivers");
        assert_eq!(held(&reg, FIXTURE_GOO), 40);
        // 40 goo at 10 a run is four runs; the fuel was provisioned for four.
        let two = installed(react(&mut reg, station, second, 4));
        assert_eq!(held(&reg, FIXTURE_GOO), 0, "all the goo is spent");
        finish_the_clock(&mut reg, two);
        call_in(
            &mut reg,
            "industryManager",
            "CompleteJob",
            &[PyValue::Int(two), PyValue::Int(30_000_001)],
        )
        .expect("stage two delivers");
        assert_eq!(held(&reg, FIXTURE_ALLOY), 20, "four runs of five");
    }

    /// Durations are the game clock's: at scale N the job ends N times
    /// sooner and its nominal length is unchanged.
    #[test]
    fn a_reaction_job_runs_on_the_game_clock() {
        let n: u32 = 5;
        let end_of = |scale: u32| {
            let (mut reg, station, formula_id) = with_a_formula(2, 3);
            crate::game_clock::set(
                &mut reg,
                crate::game_clock::GameClock::new(scale).expect("a scale"),
            );
            let job_id = installed(react(&mut reg, station, formula_id, 2));
            let job = *reg.jobs().get(job_id).expect("the job");
            (job.end_date - job.start_date, job.time_in_seconds)
        };
        let (real, nominal) = end_of(1);
        let (fast, nominal_fast) = end_of(n);
        assert_eq!(nominal, nominal_fast, "timeInSeconds is the nominal length");
        assert_eq!(real, nominal * crate::industry::FILETIME_SECOND);
        assert_eq!(fast * i64::from(n), real);
    }

    /// The refusals a reaction can meet arrive in the shape the tooltip reads.
    #[test]
    fn a_reaction_is_refused_in_the_tooltips_codes() {
        // No skill: Reactions 2 is short of the formula's level.
        let (mut reg, station, formula_id) = with_a_formula(1, 2);
        let e = react(&mut reg, station, formula_id, 1).expect_err("short of Reactions");
        assert_eq!(validation_codes(&e), vec![30]);
        assert!(reg.jobs().is_empty());
        assert_eq!(held(&reg, FIXTURE_FUEL), 5, "and nothing moved");

        // No inputs: 16, and nothing moves.
        let (mut reg, station, formula_id) = with_a_formula(0, 3);
        let e = react(&mut reg, station, formula_id, 1).expect_err("no inputs");
        assert_eq!(validation_codes(&e), vec![16]);

        // One slot: a second formula waits for Mass Reactions.
        let (mut reg, station, first) = with_a_formula(2, 3);
        let at = crate::item::Slot::new(ACTOR, station, crate::industry::FLAG_HANGAR);
        reg.apply_items(&[crate::item::Edit::Create {
            type_id: FIXTURE_FORMULA,
            quantity: 1,
            to: at,
        }])
        .expect("a second copy of the formula");
        let other = reg
            .store()
            .items()
            .iter()
            .find(|i| i.type_id == FIXTURE_FORMULA && i.item_id != first)
            .expect("the second formula item")
            .item_id;
        installed(react(&mut reg, station, first, 1));
        let e = react(&mut reg, station, other, 1).expect_err("one slot");
        assert_eq!(validation_codes(&e), vec![31]);

        // A manufacturing request on a formula is an incompatible activity.
        let (mut reg, station, formula_id) = with_a_formula(1, 3);
        let e = call_in(
            &mut reg,
            "industryManager",
            "InstallJob",
            &[install_dump(formula_id, station, 1)],
        )
        .expect_err("a formula does not manufacture");
        assert_eq!(validation_codes(&e), vec![21]);
    }

    /// A fresh character can buy every skill a formula needs, so the
    /// reactions are not an unreachable subsystem. Fixture-driven like the
    /// manufacturing one; a bundle formula whose skill nobody can train is
    /// refused `MissingSkill` at install, by name.
    #[test]
    fn every_fixture_formula_names_the_reactions_skill() {
        let content = crate::reactions::test_content();
        let table: &crate::industry::Table = content.get();
        assert!(!table.reactions.is_empty());
        for f in &table.reactions {
            assert!(
                f.skills.iter().any(|&(s, _)| s == SKILL_REACTIONS),
                "formula {} needs Reactions",
                f.blueprint_type_id
            );
        }
    }

    /// A reaction in flight is a promise kept: the job survives the world
    /// file, is delivered after the restart, and the product is the same.
    #[test]
    fn a_running_reaction_survives_a_restart_and_delivers() {
        let (mut reg, station, formula_id) = with_a_formula(2, 3);
        let job_id = installed(react(&mut reg, station, formula_id, 2));
        let before = *reg.jobs().get(job_id).expect("the job");
        let snap = reg.to_snapshot();
        let mut back = Registry::new(SandboxConfig::default())
            .with_content(std::sync::Arc::new(crate::reactions::test_content()));
        back.apply_snapshot(snap);
        assert_eq!(back.jobs().get(job_id).copied(), Some(before));
        assert_eq!(held(&back, FIXTURE_FUEL), 0, "the inputs stay spent");
        finish_the_clock(&mut back, job_id);
        call_in(
            &mut back,
            "industryManager",
            "CompleteJob",
            &[PyValue::Int(job_id), PyValue::Int(30_000_001)],
        )
        .expect("delivers after the restart");
        assert_eq!(held(&back, FIXTURE_GOO), 40);
    }
}
