//! Industry: NPC-station manufacturing, and the static layer it runs on.
//!
//! The wire shape is `crate::service::industry`; this module is the model.
//! Everything here is pure: no clock, no store, no `Registry`. A caller hands
//! in `now`, the character's stacks and the facility, and gets back a `Job`
//! plus a `Vec<item::Edit>` that `item::Store::apply` either performs whole or
//! refuses whole.
//!
//! # Rule zero: the client ships CCP's own industry engine
//!
//! `docs/port-tracker.md` §15.1 says to look in `out/pyc/eve/common/` before
//! reading EvEJS at all. There is exactly one industry module there --
//! `eve/common/script/util/industryCommon.pyc`, 616 lines -- and it is the
//! *glue*. The engine itself is a **separate top-level package**,
//! `out/pyc/industry/`, 91 KB across fourteen modules plus an `activities/`
//! subpackage, and it is CCP's own server-side code shipped in the client
//! bundle:
//!
//! ```text
//!   industry/const.pyc       activity ids, job statuses, 40 error codes, every constant below
//!   industry/job.pyc         JobBase / JobData / Job -- the maths and all 35 validations
//!   industry/activity.pyc    Activity: job_time, job_cost, job_max_runs, activity_tax
//!   industry/activities/manufacturing.pyc   ME/TE modifiers, products, the blueprint it returns
//!   industry/activities/{copying,research_time,research_material,invention,reaction}.pyc
//!   industry/blueprint.pyc   Blueprint, and `BlueprintStorage()` -- the reader for blueprints.static
//!   industry/material.pyc    Material, with `missing` and `ratio`
//!   industry/facility.pyc    Facility, `update_activity`, `serviceAccess`
//!   industry/modifiers.pyc   Material/Time/Cost/Probability/MaxRuns/Slot modifiers
//!   industry/storage.pyc     the five `industry_*.static` readers
//! ```
//!
//! The proof that it is shared rather than a client mirror is the same kind
//! the PI lane found: `job.py` imports `eveuniverse.security`,
//! `security.common.const.INDUSTRY_SECURITY_TAX_RATE` and
//! `subscriptionMgr`, and `industryCommon.GetJobModifiers` branches on
//! `if boot.role == 'client'` to pick between `clientDogmaIM` and
//! `GetDogmaIMService()`. Code that needs a `boot.role` test runs under both.
//!
//! **So nothing here is ported from EvEJS.** Their twelve-file,
//! 12,120-line `services/industry/` was read afterwards, as a second
//! opinion, and §"Where EvEJS differs" below records the three places it
//! disagrees with CCP's own code. `docs/lanes/industry.md` has the rest.
//!
//! # The static layer comes from the bundle
//!
//! `res:/staticdata/blueprints.static` is **not** cFSD: it is an `fsdlite`
//! sqlite cache whose `cache` table holds one JSON document per blueprint
//! type id. CP24's `econ` section extracts it to `industry/blueprints.tsv`,
//! and [`Table::from_bundle`] reads every blueprint whose manufacturing
//! activity has one product, priced by `econ/prices.tsv` (CP33b; the rows
//! that were compiled in broke the no-client-data rule,
//! `docs/port-tracker.md` §66). A job whose product or material the item
//! store does not know is refused [`Refusal::UnknownType`] by name, because
//! `item::Edit::Create` refuses `EditError::UnknownType` -- and refusing is
//! correct, see that variant. Without a bundle the table is
//! [`Table::fixture`]'s invented rows.
//!
//! The *other* four industry tables were read too, and two of them produced
//! null results worth more than the data would have been:
//!
//! | resource | rows | what it is |
//! | --- | --- | --- |
//! | `industry_activities.static` | 6 | id, name and description of the six activities. Confirms [`ACTIVITIES`] |
//! | `industry_assembly_lines.static` | 146 | `base_{time,material,cost}_multiplier` per line, plus per-category/group/type-list detail |
//! | `industry_installation_types.static` | 102 | station/structure typeID -> its assembly lines |
//! | `industry_activity_modifier_sources.static` | large | `{typeID: {activity: {time/material/cost: [{dogmaAttributeID, filterID}]}}}` |
//! | `industry_activity_target_filters.static` | - | `filterID -> {categoryIDs, groupIDs, name}` |
//!
//! **`industry_installation_types` contains no NPC station type.** All 102
//! keys are POS assembly arrays, outposts and Upwell structures -- 3962,
//! 13780, 16216..16220, 21642..21646, 24567..24660, 25305, 28076..28185,
//! 28351, 28352, 28606, 29613, 30389, 32245, 33685..33867, 35877..35891,
//! 45537..45550 -- and our station's type **2502 is absent**. So an NPC
//! station has no assembly line, and therefore no `base_time_multiplier`,
//! `base_material_multiplier` or `base_cost_multiplier` of its own. That is
//! why [`Facility::npc_station`] is all `1.0`: it is the table saying so,
//! not a default we picked. **[V]**
//!
//! # Where the facility numbers come from
//!
//! The one number an NPC station *does* carry is its tax, and the client has
//! no table for it either. `Job._get_tax` is
//!
//! ```text
//!     if self.facility and self.facility.tax is not None:  tax = self.facility.tax
//!     elif self.activity and self.facility:                tax = self.activity.activity_tax(self.facility)
//!     return int(round(self.base_cost * tax))
//! ```
//!
//! and `Activity.activity_tax` falls through to
//! `facility.serviceAccess.get(serviceID, 0) or 0` -- the *structure* path,
//! keyed by `structures.GetServiceID`. A station has no `serviceAccess`, so
//! the number has to come from us. [`NPC_STATION_TAX`] is `0.0`, which is
//! **[I]** on our side and **[V2]** via EvEJS's
//! `industryConstants.DEFAULT_TAX_RATE = 0.0`.
//!
//! What the client *does* ship, and what the reprocess lane could not find,
//! is in `bin64/staticdata/mapObjects.db`: the `npcStations` table has
//! `reprocessingEfficiency` and `reprocessingStationsTake` columns, and for
//! our own station 60012526 they read **0.5 and 0.05 exactly**. See
//! `docs/lanes/industry.md` §7 -- it is a reprocessing finding made here and
//! it upgrades two of that lane's constants from [I] to [V].
//!
//! # The maths, from `industry/job.pyc` and `activities/manufacturing.pyc`
//!
//! Four formulas, and the operation order in each is the client's because
//! each one ends in a rounding step that is not associative:
//!
//! ```text
//!   materials   quantity = float(material.base)
//!               for m in input_modifiers if MaterialModifier: quantity *= m.amount
//!               quantity *= activity.job_material_runs(job)          # = job.runs
//!               max(int(ceil(round(quantity, 2))), job.runs)
//!
//!   time        seconds = activity.time * job.runs
//!               for m in input_modifiers if TimeModifier: seconds *= m.amount
//!               timedelta(seconds = long(round(seconds)))
//!
//!   cost        base_cost = int(round(prices[blueprintTypeID] * float(job.runs)))
//!               cost      = int(round(base_cost * prod(cost multipliers) + additive))
//!               tax       = int(round(base_cost * facility tax))
//!               scc       = int(round(base_cost * 0.04 * facility.sccTaxModifier))
//!               sec tax   = int(round(base_cost * 0.05)) iff the system is safe-sec
//!               total     = cost + tax + scc + sec tax
//!
//!   max runs    min(int(ceil(2_592_000 / activity.time / prod(time mults))), MAX_RUNS)
//!               for an original; min(blueprint.runsRemaining, MAX_RUNS) for a copy
//! ```
//!
//! **The ME multiplier applies to the whole batch, not per run.** `base` is
//! multiplied by the modifiers and *then* by the runs, and `round(.., 2)`
//! happens once over the product -- so 100 units at 10% ME is `ceil(900.0)`,
//! not `100 * ceil(9.0)`. The `round(.., 2)` is load-bearing and not
//! defensive tidiness: `1000.0 * 0.9` is `900.0000000000001` in IEEE 754 and
//! a bare `ceil` would bill 901 units of Tritanium forever.
//! [`tests::the_two_dp_round_before_the_ceiling_is_load_bearing`] pins it.
//!
//! The floor at `job.runs` is also the client's, and it is what makes a
//! one-unit material cost at least one unit per run however good the ME.
//!
//! ## `materialEfficiency` is a *percentage off*, and only when non-zero
//!
//! `Manufacturing.job_modifiers` is
//!
//! ```text
//!     if job.materialEfficiency: MaterialModifier(1.0 - job.materialEfficiency / 100.0, ...)
//!     if job.timeEfficiency:     TimeModifier(1.0 - job.timeEfficiency / 100.0, ...)
//! ```
//!
//! -- guarded by truthiness, so ME 0 appends **no modifier at all** rather
//! than a `1.0` one. The distinction is invisible for materials and visible
//! for [`max_runs`], which divides by the product of the time modifiers.
//!
//! ## The character's own multiplier is folded here, like reprocessing's
//!
//! `industryCommon.CHARACTER_MODIFIERS` reads the character's
//! `manufactureTimeMultiplier` (attribute **219**) through
//! `dogmaLocation.GetIndustryCharacterModifiers(characterID)`. We have no
//! dogma engine, so [`time_multiplier`] folds it the way the client's own UI
//! path does. Measured from `typedogma.fsdbinary`, **[V]**:
//!
//! - attribute 219 `manufactureTimeMultiplier` is `1.0` on all **14**
//!   character types (1373..1386) and on nothing else, so the base is 1.0;
//! - attribute **440** `manufacturingTimeBonus` is on exactly **7** types:
//!   skill 3380 Industry at `-4`, and six implants (27167 `-2`, 27170 `-1`,
//!   27171 `-4`, 59797/59799/59801 `-8`);
//! - attribute **1961** `advancedIndustrySkillIndustryJobTimeBonus` is on
//!   exactly **one** type, skill 3388 Advanced Industry, at `-3`;
//! - attribute **1982** `manufactureTimePerLevel` -- the one
//!   `REQUIRED_SKILL_MODIFIERS` reads -- is on **26** types, all of them T2
//!   production skills at `-1` (and 81896 at `-2`). A Tech I blueprint
//!   requires none of them, so the path matters for Tech II blueprints.
//!
//! Each is a "PostPercent" effect, so the terms multiply:
//! `1.0 * (1 - 0.04*L(3380)) * (1 - 0.03*L(3388)) * prod(implants)`.
//!
//! ## Slots, and why a fresh character has exactly one
//!
//! `Job._get_errors` ends with `if self.used_slots >= self.max_slots:
//! SLOTS_FULL`, where `max_slots` sums every `SlotModifier` and the only
//! source of one for manufacturing is attribute **196**
//! `manufactureSlotLimit`. That attribute's `defaultValue` in
//! `dogmaattributes` is **0** -- which would mean nobody can ever install
//! anything -- but the **character type carries it explicitly at 1**
//! (checked on 1373). Attribute **450** `manufacturingSlotBonus` is `+1` on
//! exactly two types, skills 3387 Mass Production and 24625 Advanced Mass
//! Production. So [`max_slots`] is `1 + L(3387) + L(24625)`, capped at 11,
//! and a character with no industry skills has one slot. **[V]**
//!
//! Reading the `defaultValue` and stopping would have produced a server on
//! which every install failed with error 31 and nothing in any log to say
//! why; it is recorded here because that is the shape of a wrong answer that
//! looks like a right one.
//!
//! # Job status, and the gap in the middle of it
//!
//! `docs/port-tracker.md` §14.1 settled this and the spec had it wrong
//! twice. [`Status`] is the closed eight-value set from `industry/const.pyc`,
//! `JOB_STATUS_NAMES` is the eight-entry map over exactly those, and
//! **4..99 are unused**. The client's one semantic test on the field is
//! `status < industry.STATUS_COMPLETED`, so a value we invent below 100 locks
//! a blueprint to a job forever and one at or above 100 frees a blueprint
//! that is still in a job -- silently, in both directions.
//! [`Status::from_code`] therefore refuses the gap rather than rounding into
//! it.
//!
//! **`READY` is derived, not stored**, and that is `industryCommon.JobStatus`:
//!
//! ```text
//!     if data.status == STATUS_INSTALLED and data.endDate < now:      return STATUS_READY
//!     if data.status == STATUS_PAUSED   and data.pauseDate > data.endDate: return STATUS_READY
//!     return data.status
//! ```
//!
//! [`Job::status_at`] is that function. A job we keep at `Installed` is shown
//! as `Ready` by the client the moment its `end_date` passes, with no push
//! needed -- and `industrySvc._PollJobCompletion` re-scatters `OnIndustryJob`
//! client-side once a second for exactly those jobs, so the Jobs tab updates
//! itself. We store `Installed` and let the client derive, because storing
//! `Ready` would mean a timer on our side whose only job is to agree with a
//! comparison the client already makes.
//!
//! # Round 2: science, and where a blueprint's ME, TE and runs live
//!
//! Research (ME and TE), copying and invention are served, each as a job
//! that installs, runs on the clock and delivers. Every formula is CCP's own
//! from `industry/activities/{research_material,research_time,copying,
//! invention}.pyc`, `activity.pyc` and `job.pyc`, read with `tools/dis.py`:
//!
//! ```text
//!   research    level   = ME / 1  (TE / 2)              blueprint_level
//!               levels  = [time * RESEARCH_TIMES[i] for i in range(10)]
//!               time    = sum(levels[min(level + i, 9)] for i in range(runs))
//!               cost    = prices[bp] * 0.02 * time / float(activity.time)
//!               maxRuns = min(Activity.job_max_runs, (10 - ME) / 1)   ((20 - TE) / 2)
//!               output  = ME = min(ME + 1 * runs, 10)   (TE = min(TE + 2 * runs, 20))
//!               RESEARCH_LIMIT (39) when level < 0 or level >= 10
//!   copying     time    = activity.time * licensedRuns * runs
//!               cost    = prices[bp] * 0.02 * licensedRuns * runs
//!               maxRuns = Activity.job_max_runs, MAX_RUNS 500
//!               output  = `runs` copies, runsRemaining = licensedRuns, ME/TE of the original
//!   invention   time    = activity.time * runs
//!               cost    = prices[T2 blueprint] * 0.02 * runs
//!               p       = mean(product.probability or 1.0 for every product)
//!                         * (1 + sum(level(skill) * 1/30 or 1/40))  clamped to 0..1
//!               maxRuns = min(copy.runsRemaining, Activity.job_max_runs), MAX_RUNS 500
//!               output  = successfulRuns copies of the T2 blueprint, ME 2, TE 4,
//!                         runsRemaining = round(product.quantity + 0)
//! ```
//!
//! `prices` is `JobPrices(activityID)`, whose `GetBlueprintPrice` maps every
//! science activity to **manufacturing** -- so every science cost is a
//! fraction of an estimated item value, the same [`estimated_item_value`]
//! manufacturing uses. The character's time multipliers are four more
//! dogma attributes on the character, measured in `typedogma.fsdbinary` and
//! `dogmaeffects` (the `modifierInfo` of each skill's effect, not its name):
//! see [`activity_time_multiplier`]. **[V]** throughout, except where a
//! comment says `[A]`.
//!
//! **ME, TE and runs live in [`Blueprints`], not on the item.** `item::Item`
//! has no columns for them and `item.rs` is not this module's, so the three
//! numbers are a side table keyed by item id, held by [`Jobs`] and persisted
//! as `bpstate` records. An item with no row is what it always was here: an
//! unresearched original, ME 0, TE 0, runs `-1`. A row is written by a
//! research delivery (the original's new ME or TE), by a copy or invention
//! delivery (one row per copy minted, keyed by the id `item::Store::apply`
//! gave it), and by an install that spends a copy's runs. The item id is the
//! key because it is the one thing about a blueprint that survives a move, a
//! trade or a restart.
//!
//! # What this module is not
//!
//! - **Reactions are served (MN-A, R-MOON §4.1)**, at an NPC station, with no
//!   structure: `Reaction` is three lines (`REQUIRES_ORIGINAL`,
//!   `prices[bp] * runs`, `runs * product.quantity`) and every input and
//!   output of the formula tree is an item now. [`Table::parse`] keeps each
//!   formula (a blueprint with activity 9 only) in [`Table::reactions`];
//!   [`Jobs::plan_install`] and [`Jobs::plan_delivery`] run it through the
//!   same plan as manufacturing, with `crate::reactions` holding the
//!   reaction skills' numbers. A manufacturing blueprint asked to react and
//!   a formula asked to manufacture are still
//!   [`Refusal::IncompatibleActivity`].
//! - **Decryptors and relics are `relic.rs`** (lane `t3-a`): the generic
//!   decryptors of group 1304 modify an invention's chance and its copy's
//!   ME / TE / runs from the decryptor type's own attributes, and an ancient
//!   relic stack is an invention-only "blueprint copy". With no decryptor the
//!   copy is ME 2 / TE 4 / `product.quantity` runs exactly, which is the
//!   client's own no-decryptor answer.
//! - **No structures, no rigs, no system cost index.** `industry_assembly_lines`
//!   and `industry_activity_modifier_sources` are the tables for that and
//!   neither is read here; the system cost index is a stub in EvEJS too, by
//!   their own admission (`industrySystemCostIndex.js`, 43 lines).
//! - **No ISK movement here.** [`Cost`] is computed here and charged by
//!   `Registry::install_job` through `Registry::charge`, with the journal
//!   `refType` from [`ACCOUNT_ENTRY_TYPES`]; this module stays pure.
//!
//! # Where EvEJS differs, and which side the client takes
//!
//! 1. **The `locationID = 2003` sentinel.** `industryConstants.js:90` marks
//!    "installed in an industry job" by *moving* the blueprint to location
//!    2003, tested against at nine sites. We do not move the blueprint at
//!    all: the client's own signal is `blueprint.jobID`
//!    (`blueprintSvc._UpdateOnIndustryJobChange` sets it from the push and
//!    `Job._get_errors` raises `BLUEPRINT_INSTALLED` off it), so the id
//!    belongs on the job, not in the address space. 2003 is also below the
//!    90,000,000 `IsSystemOrNPC` threshold documented on
//!    `item::ESCROW_BASE`, which means the client would answer questions
//!    about that location from its own sqlite before ever asking us.
//! 2. **Cancel refunds nothing**, in both, but for different reasons: theirs
//!    is a 1,054-line compensating saga (`industryInstallSettlementState.js`)
//!    that *tries* to refund and gives up; ours is one plan that never
//!    half-happened. [`Jobs::cancel`] returns no edits and says so.
//! 3. **`DEFAULT_TAX_RATE`** is the one number we take from them, as
//!    **[V2]**, because the client genuinely has none.
//!
//! Counted rather than ported, per `docs/port-tracker.md` §10.5: their
//! `industryManagerService.js` exports six handlers and our build's client
//! calls **five** of them plus three they do not implement. The census is in
//! `docs/lanes/industry.md` §4.

use std::collections::BTreeMap;

pub mod relic;
pub use relic::{Decryptor, Relic};

/// Every industry row this module reads, as one value (`docs/content/plan.md`
/// §1.5, ruling 3). The service reads it with
/// `reg.content().get::<industry::Table>()`; a pure function takes `&Table`.
///
/// CP33b: built from the bundle's `industry/blueprints.tsv` and
/// `econ/prices.tsv` ([`Table::from_bundle`]), or from invented rows
/// ([`Table::fixture`]). No client row is compiled in (`docs/port-tracker.md`
/// §66).
///
/// `blueprints` and `science` are **strictly ascending by
/// `blueprint_type_id`**, because the lookups binary-search; [`Table::parse`]
/// builds them that way and
/// `tests::the_parsed_table_is_sorted_so_the_lookups_are_valid` holds it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    /// Every blueprint with a manufacturing activity that makes exactly one
    /// product, Tech I and Tech II alike, by `blueprint_type_id`.
    pub blueprints: Vec<Blueprint>,
    /// The four science activities of each `blueprints` row, same order.
    pub science: Vec<Science>,
    /// Every reaction formula (MN-A): a blueprint whose only activity is 9,
    /// with exactly one product, by `blueprint_type_id`, strictly ascending.
    /// Kept apart from `blueprints` so nothing that walks the manufacturing
    /// list (the bots' builder, the recipe tests) sees a formula; a lookup
    /// that must serve both is [`Table::lookup`]. `materials` and `skills`
    /// are the **activity 9** ones, `time` is the activity's seconds per run
    /// and `product_quantity` the units one run yields.
    pub reactions: Vec<Blueprint>,
    /// `P_ref` by type id: `econ/prices.tsv`'s `ref_price`, the number
    /// `econ_prices` serves as `adjustedPrice` (CP31), so the server's
    /// charge and the client's preview read the same price.
    pub prices: BTreeMap<i64, f64>,
    /// Ancient relics: blueprint types with an invention activity and no
    /// manufacturing one (`relic.rs`), ascending by type id. T3-B.
    pub relics: Vec<Relic>,
}

/// The registration line, in `content_tables::TABLES` since CP33a.
pub const TABLE: crate::content::TableSpec = crate::content::TableSpec {
    name: "industry",
    load: table_load,
    fixture: table_fixture,
};

/// The bundle's rows, or the invented fixture with no bundle (plan §2.2: "with
/// no bundle, both services answer from the fixture tables").
fn table_load(src: &crate::content::Sources) -> Result<crate::content::Boxed, String> {
    Ok(Box::new(match src.bundle {
        Some(b) => Table::from_bundle(b)?,
        None => Table::fixture(),
    }))
}

fn table_fixture() -> crate::content::Boxed {
    Box::new(Table::fixture())
}

/// The bundle file and its header line, copied from `schema.md`.
pub const BLUEPRINTS: (&str, &str) = (
    "industry/blueprints.tsv",
    "#bp_type_id\tactivity_id\tkind\ttype_id\tquantity\tprobability\ttime\tmax_runs",
);

/// One activity of one blueprint, gathered from its `blueprints.tsv` rows.
#[derive(Default)]
struct Gathered {
    seen: bool,
    time: i64,
    max_runs: i64,
    materials: Vec<(i64, i64)>,
    products: Vec<InventionProduct>,
    skills: Vec<(i64, i64)>,
}

impl Gathered {
    /// Only a time: what a research or copying activity must be to be served.
    fn bare_time(&self) -> Option<i64> {
        (self.seen
            && self.materials.is_empty()
            && self.skills.is_empty()
            && self.products.is_empty())
        .then_some(self.time)
    }
}

impl Table {
    /// The bundle's `industry/blueprints.tsv`, priced by its
    /// `econ/prices.tsv` (read by `econ_prices`' own parser).
    pub fn from_bundle(b: &crate::content::Bundle) -> Result<Table, String> {
        let prices = crate::econ_prices::Table::from_bundle(b)?;
        let (rel, header) = BLUEPRINTS;
        Table::parse(&b.data_lines(rel, header)?, &prices).map_err(|e| format!("{rel}: {e}"))
    }

    /// The data lines of `industry/blueprints.tsv`, and the prices.
    ///
    /// A malformed line or an unknown `kind` is an error naming its data line
    /// (counted among the data lines). What is kept:
    ///
    /// - a [`Blueprint`] for every blueprint whose `manufacturing` activity has
    ///   exactly one product (`products[0]` is what the client builds); one
    ///   with none cannot be installed, so it is left out;
    /// - its [`Science`]: each research and copying time, and the invention
    ///   activity when it has a product;
    /// - a [`Blueprint`] in [`Table::reactions`] for every blueprint with **no**
    ///   manufacturing activity whose activity 9 has exactly one product (MN-A:
    ///   a reaction formula; `crate::reactions`). One with none or several
    ///   cannot be installed, so it is left out.
    ///
    /// Materials and skills are sorted by type id. **[A]** that their order
    /// carries no meaning: falsified by a client path that reads
    /// `materials[i]` by position.
    ///
    /// **[A] A research or copying activity with a material, skill or product
    /// row is left out**, so it is refused as `INCOMPATIBLE_ACTIVITY` rather
    /// than served without what it needs: [`Science`] holds a time only.
    /// Falsified by nothing; it is a gap, closed by giving `Science` the lists.
    pub fn parse<S: AsRef<str>>(
        lines: &[S],
        prices: &crate::econ_prices::Table,
    ) -> Result<Table, String> {
        let mut by_bp: BTreeMap<i64, BTreeMap<i64, Gathered>> = BTreeMap::new();
        for (i, line) in lines.iter().enumerate() {
            let line = line.as_ref();
            let bad = || format!("data line {}: {line:?}", i + 1);
            let f: Vec<&str> = line.split('\t').collect();
            let [
                bp,
                activity,
                kind,
                type_id,
                quantity,
                probability,
                time,
                max_runs,
            ] = f[..]
            else {
                return Err(bad());
            };
            let int = |s: &str| s.parse::<i64>().map_err(|_| bad());
            let probability = match probability {
                // `product.probability or 1.0`: an absent chance is 0 here
                // and 1.0 in `invention_probability`.
                "" => 0.0,
                p => p.parse::<f64>().map_err(|_| bad())?,
            };
            let g = by_bp
                .entry(int(bp)?)
                .or_default()
                .entry(int(activity)?)
                .or_default();
            g.seen = true;
            g.time = int(time)?;
            g.max_runs = int(max_runs)?;
            let (type_id, quantity) = (int(type_id)?, int(quantity)?);
            match kind {
                "activity" => {}
                "material" => g.materials.push((type_id, quantity)),
                "skill" => g.skills.push((type_id, quantity)),
                "product" => g.products.push(InventionProduct {
                    type_id,
                    quantity,
                    probability,
                }),
                _ => return Err(bad()),
            }
        }
        let mut table = Table {
            prices: prices
                .prices
                .iter()
                .map(|(&t, p)| (t, p.ref_price))
                .collect(),
            // T3-B: the invention-only rows, before the loop consumes them.
            relics: relic::collect(&by_bp),
            ..Table::default()
        };
        for (blueprint_type_id, mut acts) in by_bp {
            let Some(mfg) = acts.remove(&MANUFACTURING) else {
                // A formula: no manufacturing, one reaction product.
                if let Some(r) = acts.remove(&REACTION)
                    && let [product] = r.products[..]
                {
                    let sorted = |mut v: Vec<(i64, i64)>| {
                        v.sort_unstable();
                        v
                    };
                    table.reactions.push(Blueprint {
                        blueprint_type_id,
                        product_type_id: product.type_id,
                        product_quantity: product.quantity,
                        time: r.time,
                        max_production_limit: r.max_runs,
                        materials: sorted(r.materials),
                        skills: sorted(r.skills),
                    });
                }
                continue;
            };
            let [product] = mfg.products[..] else {
                continue;
            };
            let sorted = |mut v: Vec<(i64, i64)>| {
                v.sort_unstable();
                v
            };
            let get = |a: i64| acts.get(&a);
            let invention = get(INVENTION)
                .filter(|g| !g.products.is_empty())
                .map(|g| Invention {
                    time: g.time,
                    materials: sorted(g.materials.clone()),
                    products: g.products.clone(),
                    skills: sorted(g.skills.clone()),
                });
            table.science.push(Science {
                blueprint_type_id,
                research_material_time: get(RESEARCH_MATERIAL).and_then(Gathered::bare_time),
                research_time_time: get(RESEARCH_TIME).and_then(Gathered::bare_time),
                copying_time: get(COPYING).and_then(Gathered::bare_time),
                invention,
            });
            table.blueprints.push(Blueprint {
                blueprint_type_id,
                product_type_id: product.type_id,
                product_quantity: product.quantity,
                time: mfg.time,
                max_production_limit: mfg.max_runs,
                materials: sorted(mfg.materials),
                skills: sorted(mfg.skills),
            });
        }
        Ok(table)
    }

    /// **Invented rows**: every quantity, time, chance and price is made up.
    ///
    /// The type ids are `item::SPAWNABLE`'s hand rows (`item.rs`, retired by
    /// CP58-CP60), because a job's product and materials must be types the
    /// item store can mint (`EditError::UnknownType`); `9_990_00x` are
    /// invented ids no table knows. With no bundle the server answers from
    /// these, which is a test world, not a playable one (plan §2.3).
    pub fn fixture() -> Table {
        const ROWS: &[&str] = &[
            // A hull with four minerals, three science times and an invention.
            "691\t1\tactivity\t0\t0\t\t600\t20",
            "691\t1\tmaterial\t34\t1000\t\t600\t20",
            "691\t1\tmaterial\t35\t200\t\t600\t20",
            "691\t1\tmaterial\t36\t50\t\t600\t20",
            "691\t1\tmaterial\t37\t10\t\t600\t20",
            "691\t1\tproduct\t587\t1\t\t600\t20",
            "691\t1\tskill\t3380\t1\t\t600\t20",
            "691\t3\tactivity\t0\t0\t\t300\t20",
            "691\t4\tactivity\t0\t0\t\t300\t20",
            "691\t5\tactivity\t0\t0\t\t400\t20",
            "691\t8\tactivity\t0\t0\t\t3000\t20",
            "691\t8\tmaterial\t20172\t2\t\t3000\t20",
            "691\t8\tmaterial\t20424\t2\t\t3000\t20",
            "691\t8\tproduct\t11372\t1\t0.25\t3000\t20",
            "691\t8\tproduct\t11401\t1\t0.25\t3000\t20",
            "691\t8\tskill\t11445\t1\t\t3000\t20",
            "691\t8\tskill\t11452\t1\t\t3000\t20",
            "691\t8\tskill\t21791\t1\t\t3000\t20",
            // A hull whose second material no item table knows.
            "693\t1\tactivity\t0\t0\t\t900\t10",
            "693\t1\tmaterial\t34\t1000\t\t900\t10",
            "693\t1\tmaterial\t9990001\t5\t\t900\t10",
            "693\t1\tproduct\t639\t1\t\t900\t10",
            "693\t1\tskill\t3380\t1\t\t900\t10",
            // A second buildable hull, for the second slot.
            "950\t1\tactivity\t0\t0\t\t450\t20",
            "950\t1\tmaterial\t34\t800\t\t450\t20",
            "950\t1\tmaterial\t35\t100\t\t450\t20",
            "950\t1\tproduct\t603\t1\t\t450\t20",
            "950\t1\tskill\t3380\t1\t\t450\t20",
            // The two invention products: blueprints of invented types.
            "11372\t1\tactivity\t0\t0\t\t1200\t10",
            "11372\t1\tmaterial\t34\t5000\t\t1200\t10",
            "11372\t1\tmaterial\t38\t20\t\t1200\t10",
            "11372\t1\tproduct\t9990002\t1\t\t1200\t10",
            "11372\t1\tskill\t3380\t1\t\t1200\t10",
            "11401\t1\tactivity\t0\t0\t\t1200\t10",
            "11401\t1\tmaterial\t34\t4000\t\t1200\t10",
            "11401\t1\tproduct\t9990003\t1\t\t1200\t10",
            "11401\t1\tskill\t3380\t1\t\t1200\t10",
            // A module from one mineral, with research and copying only.
            "21858\t1\tactivity\t0\t0\t\t120\t10",
            "21858\t1\tmaterial\t34\t40\t\t120\t10",
            "21858\t1\tproduct\t21857\t1\t\t120\t10",
            "21858\t1\tskill\t3380\t1\t\t120\t10",
            "21858\t3\tactivity\t0\t0\t\t60\t10",
            "21858\t4\tactivity\t0\t0\t\t60\t10",
            "21858\t5\tactivity\t0\t0\t\t90\t10",
        ];
        const PRICES: &[&str] = &[
            "34\t4.0\tbase\t1.0\t4\t18\t\t1",
            "35\t10.0\tbase\t1.0\t4\t18\t\t1",
            "36\t40.0\tbase\t1.0\t4\t18\t\t1",
            "37\t100.0\tbase\t1.0\t4\t18\t\t1",
            "38\t500.0\tbase\t1.0\t4\t18\t\t1",
        ];
        // MN-A: three invented reaction formulas and their prices.
        let rows: Vec<String> = ROWS
            .iter()
            .map(|r| (*r).to_owned())
            .chain(crate::reactions::fixture_rows())
            .collect();
        let prices: Vec<String> = PRICES
            .iter()
            .map(|r| (*r).to_owned())
            .chain(crate::reactions::fixture_prices())
            .collect();
        crate::econ_prices::Table::parse(&prices)
            .and_then(|p| Table::parse(&rows, &p))
            .unwrap_or_else(|e| unreachable!("the fixture parses: {e}"))
    }

    /// The blueprint with this **blueprint** type id. (An ancient relic is
    /// not found here: [`Table::relic`].)
    pub fn find(&self, blueprint_type_id: i64) -> Option<&Blueprint> {
        self.blueprints
            .binary_search_by_key(&blueprint_type_id, |b| b.blueprint_type_id)
            .ok()
            .map(|i| &self.blueprints[i])
    }

    /// The reaction formula with this **blueprint** type id (MN-A).
    pub fn find_reaction(&self, blueprint_type_id: i64) -> Option<&Blueprint> {
        self.reactions
            .binary_search_by_key(&blueprint_type_id, |b| b.blueprint_type_id)
            .ok()
            .map(|i| &self.reactions[i])
    }

    /// The row behind a blueprint item of either kind: a manufacturing
    /// blueprint or a reaction formula. What a held item resolves through;
    /// [`Table::find`] stays manufacturing-only.
    pub fn lookup(&self, blueprint_type_id: i64) -> Option<&Blueprint> {
        self.find(blueprint_type_id)
            .or_else(|| self.find_reaction(blueprint_type_id))
    }

    /// Whether this type id is a reaction formula (MN-A).
    pub fn is_formula(&self, type_id: i64) -> bool {
        self.find_reaction(type_id).is_some()
    }

    /// The blueprint that manufactures this product, the lowest blueprint
    /// type id first when two do.
    ///
    /// `cfg.blueprints.index('productTypeID', ...)` on the client side, which
    /// is `industry.INDEXES`' one secondary index. Linear: nothing on a hot
    /// path asks.
    pub fn by_product(&self, product_type_id: i64) -> Option<&Blueprint> {
        self.blueprints
            .iter()
            .find(|b| b.product_type_id == product_type_id)
    }

    /// The science activities of this blueprint type, or `None` for a type
    /// that is not a blueprint here.
    pub fn science(&self, blueprint_type_id: i64) -> Option<&Science> {
        self.science
            .binary_search_by_key(&blueprint_type_id, |s| s.blueprint_type_id)
            .ok()
            .map(|i| &self.science[i])
            .or_else(|| self.relic(blueprint_type_id).map(|r| &r.science))
    }

    /// Whether this type id is one of the blueprints we know, a reaction
    /// formula included.
    pub fn is_blueprint_type(&self, type_id: i64) -> bool {
        self.lookup(type_id).is_some()
    }

    /// `inventorycommon.typeHelpers.GetAdjustedAveragePrice`: `P_ref`, the
    /// `adjustedPrice` `config.GetAverageMarketPrices` serves (CP31).
    ///
    /// **[V]** on its role: the client's own `industryCommon.GetBlueprintPrice`
    /// ends `if adjustedAveragePrice: return adjustedAveragePrice` and logs an
    /// error and returns 0 when there is none, so 0 for an unpriced type is
    /// the client's own behaviour and not a silent default of ours.
    pub fn adjusted_price(&self, type_id: i64) -> f64 {
        self.prices.get(&type_id).copied().unwrap_or(0.0)
    }

    /// Whether `adjusted_price` has a row, as distinct from having a zero.
    pub fn is_priced(&self, type_id: i64) -> bool {
        self.prices.contains_key(&type_id)
    }
}

use crate::item::{self, Edit, Slot};
use crate::skill::Skills;

// ---------------------------------------------------------------------------
// Activities. **[V]** `industry/const.pyc`, read with `tools/pyc27.py`.
// ---------------------------------------------------------------------------

pub const MANUFACTURING: i64 = 1;
pub const RESEARCH_TIME: i64 = 3;
pub const RESEARCH_MATERIAL: i64 = 4;
pub const COPYING: i64 = 5;
pub const INVENTION: i64 = 8;
pub const REACTION: i64 = 9;

/// `industry.ACTIVITIES`, **in the client's own order**.
///
/// The tuple is `(MANUFACTURING, RESEARCH_MATERIAL, RESEARCH_TIME, COPYING,
/// INVENTION, REACTION)` = `(1, 4, 3, 5, 8, 9)`. A previous pass through this
/// lane recorded it as `(1, 3, 4, 5, 8, 9)` with the names in ascending-id
/// order; the set is the same and the order is not, and `industry_activities.static`
/// agrees with the tuple. Nothing we send depends on the order today -- both
/// int-keyed dicts in `service::industry` are sorted by key -- which is
/// precisely why it was worth re-reading rather than inheriting.
pub const ACTIVITIES: [i64; 6] = [
    MANUFACTURING,
    RESEARCH_MATERIAL,
    RESEARCH_TIME,
    COPYING,
    INVENTION,
    REACTION,
];

/// `industry.SCIENCE_ACTIVITIES`: the four that share the laboratory slots
/// and that `GetBlueprintPrice` prices as manufacturing.
pub const SCIENCE_ACTIVITIES: [i64; 4] = [RESEARCH_MATERIAL, RESEARCH_TIME, COPYING, INVENTION];

/// `industry.ACTIVITY_NAMES`, which is also the key `blueprints.static` uses
/// inside each row's `activities` object.
#[cfg(test)]
pub const ACTIVITY_NAMES: [(i64, &str); 6] = [
    (MANUFACTURING, "manufacturing"),
    (RESEARCH_TIME, "research_time"),
    (RESEARCH_MATERIAL, "research_material"),
    (COPYING, "copying"),
    (INVENTION, "invention"),
    (REACTION, "reaction"),
];

/// The activity's name in `blueprints.static`, or `None` for an id that is
/// not an activity.
#[cfg(test)]
pub fn activity_name(activity_id: i64) -> Option<&'static str> {
    ACTIVITY_NAMES
        .iter()
        .find(|(id, _)| *id == activity_id)
        .map(|(_, n)| *n)
}

/// Whether this is one of the four science activities.
pub fn is_science(activity_id: i64) -> bool {
    SCIENCE_ACTIVITIES.contains(&activity_id)
}

// ---------------------------------------------------------------------------
// Job status. **[V]**, and `docs/port-tracker.md` §14.1 for why it matters.
// ---------------------------------------------------------------------------

/// The eight job statuses, and nothing between 3 and 100.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Unsubmitted,
    Installed,
    Paused,
    Ready,
    Completed,
    Delivered,
    Cancelled,
    Reverted,
}

/// `industry.STATUS_COMPLETED`, the threshold the client compares against.
///
/// Not a count and not a status to test equality on:
/// `blueprintSvc._UpdateOnIndustryJobChange` is
/// `blueprint.jobID = jobID if status < STATUS_COMPLETED else None`, and
/// `JobBase.completed` is `bool(self.status >= STATUS_COMPLETED)`.
pub const STATUS_COMPLETED: i64 = 100;

impl Status {
    /// The integer that goes on the wire.
    pub fn code(self) -> i64 {
        match self {
            Status::Unsubmitted => 0,
            Status::Installed => 1,
            Status::Paused => 2,
            Status::Ready => 3,
            Status::Completed => 100,
            Status::Delivered => 101,
            Status::Cancelled => 102,
            Status::Reverted => 103,
        }
    }

    /// The status for a code, or `None`.
    ///
    /// **The 4..99 gap is refused, not rounded.** A value in it would pass
    /// every client guard as "live" while naming no status, which is the
    /// silent failure §14.1 describes.
    pub fn from_code(code: i64) -> Option<Status> {
        Some(match code {
            0 => Status::Unsubmitted,
            1 => Status::Installed,
            2 => Status::Paused,
            3 => Status::Ready,
            100 => Status::Completed,
            101 => Status::Delivered,
            102 => Status::Cancelled,
            103 => Status::Reverted,
            _ => return None,
        })
    }

    /// `JobBase.completed`: `status >= STATUS_COMPLETED`.
    pub fn completed(self) -> bool {
        self.code() >= STATUS_COMPLETED
    }

    /// `industry.JOB_STATUS_NAMES`.
    pub fn name(self) -> &'static str {
        match self {
            Status::Unsubmitted => "unsubmitted",
            Status::Installed => "installed",
            Status::Paused => "paused",
            Status::Ready => "ready",
            Status::Completed => "completed",
            Status::Delivered => "delivered",
            Status::Cancelled => "cancelled",
            Status::Reverted => "reverted",
        }
    }
}

// ---------------------------------------------------------------------------
// Constants. Every one **[V]** from `industry/const.pyc`,
// `inventorycommon/const.pyc`, `dogma/const.pyc` or `eve/common/lib/appConst.pyc`,
// read with `tools/pyc27.py` rather than remembered.
// ---------------------------------------------------------------------------

/// `industry.MAX_RUNS_HARD_CAP`. `Activity.MAX_RUNS` for manufacturing.
pub const MAX_RUNS_HARD_CAP: i64 = 1_000_000;
/// `industry.MAX_RUN_LENGTH`: 30 days in seconds. A job longer than this is
/// `Error::RunLength`, and it is also what bounds [`max_runs`].
pub const MAX_RUN_LENGTH: i64 = 2_592_000;
/// `industry.COST_PERCENTAGE`. **Not used by manufacturing**: the four
/// science activities multiply their cost by it and manufacturing does not
/// (`Manufacturing.job_cost` is `prices[bp] * runs`, bare). Recorded so round
/// 2 does not have to find it.
pub const COST_PERCENTAGE: f64 = 0.02;
/// `industry.SCC_SURCHARGE_RATE`: 4% of the base cost, on every activity.
pub const SCC_SURCHARGE_RATE: f64 = 0.04;
/// `industry.SCC_SURCHARGE_RESEARCH_DISCOUNT_MODIFIER`: research-time and
/// research-material pay half the surcharge. Copying and invention do not.
pub const SCC_SURCHARGE_RESEARCH_DISCOUNT_MODIFIER: f64 = 0.5;
/// `industry.MATERIAL_ROUND_PRECISION`, the `round(quantity, 2)` in
/// [`material_quantity`].
pub const MATERIAL_ROUND_PRECISION: i32 = 2;
/// `industry.MIN_MATERIAL_EFFICIENCY` .. `MAX`, step 1.
#[cfg(test)]
pub const MIN_MATERIAL_EFFICIENCY: i64 = -80;
pub const MAX_MATERIAL_EFFICIENCY: i64 = 10;
pub const STEP_MATERIAL_EFFICIENCY: i64 = 1;
/// `industry.MIN_TIME_EFFICIENCY` .. `MAX`, **step 2** -- which is why TE
/// runs to 20 in ten research steps and ME to 10 in ten.
#[cfg(test)]
pub const MIN_TIME_EFFICIENCY: i64 = -120;
pub const MAX_TIME_EFFICIENCY: i64 = 20;
pub const STEP_TIME_EFFICIENCY: i64 = 2;

/// `industry.SKILL_INDUSTRY`.
pub const SKILL_INDUSTRY: i64 = 3380;
/// `industry.SKILL_SCIENCE`.
pub const SKILL_SCIENCE: i64 = 3402;
/// `industry.SKILL_SUPPLY_CHAIN_MANAGEMENT`, which sets the manufacturing
/// remote-install range through [`DISTANCE_PER_LEVEL`].
pub const SKILL_SUPPLY_CHAIN_MANAGEMENT: i64 = 24268;
/// `industry.SKILL_SCIENTIFIC_NETWORKING` -- the science activities' range.
pub const SKILL_SCIENTIFIC_NETWORKING: i64 = 24270;
/// `industry.SKILL_REMOTE_REACTIONS`.
pub const SKILL_REMOTE_REACTIONS: i64 = 45750;

/// `Advanced Industry`: -3% job time per level, attribute 1961.
/// **Not in `crate::skill::SKILLS`** -- see [`time_multiplier`].
pub const SKILL_ADVANCED_INDUSTRY: i64 = 3388;
/// `Mass Production`: +1 manufacturing slot per level, attribute 450.
pub const SKILL_MASS_PRODUCTION: i64 = 3387;
/// `Advanced Mass Production`: +1 more per level.
pub const SKILL_ADVANCED_MASS_PRODUCTION: i64 = 24625;

/// `industry.DISTANCE_PER_LEVEL`: jumps of remote install reach, by level of
/// the activity's distance skill. Level 0 is **0 jumps**, i.e. the facility
/// must be in the system you are in.
pub const DISTANCE_PER_LEVEL: [i64; 6] = [0, 5, 10, 15, 20, 25];
/// `industry.MAX_INDUSTRY_DISTANCE`.
#[cfg(test)]
pub const MAX_INDUSTRY_DISTANCE: i64 = 25;

/// `industry.DISTANCE_SKILL_FOR_ACTIVITY`.
pub const DISTANCE_SKILL_FOR_ACTIVITY: [(i64, i64); 6] = [
    (MANUFACTURING, SKILL_SUPPLY_CHAIN_MANAGEMENT),
    (REACTION, SKILL_REMOTE_REACTIONS),
    (RESEARCH_TIME, SKILL_SCIENTIFIC_NETWORKING),
    (RESEARCH_MATERIAL, SKILL_SCIENTIFIC_NETWORKING),
    (COPYING, SKILL_SCIENTIFIC_NETWORKING),
    (INVENTION, SKILL_SCIENTIFIC_NETWORKING),
];

/// `security.common.const.INDUSTRY_SECURITY_TAX_RATE`. **[V]**, from
/// `out/pyc/security/common/const.pyc:7`.
///
/// **This was marked [V2] on a false premise**, which is worth keeping rather
/// than quietly correcting: the comment here said "the module itself is
/// server-side and not in the client bundle", so the rate could only be taken
/// on EvEJS's word. **The module is in the bundle.** Its whole body is four
/// constants, and `tools/dis.py` reads them directly:
///
/// ```text
/// line 3   MARKET_SECURITY_TAX_RATE        = 0.05
/// line 5   NPC_BOUNTY_SECURITY_TAX_RATE    = 0.2
/// line 7   INDUSTRY_SECURITY_TAX_RATE      = 0.05
/// line 9   AGENT_MISSION_SECURITY_TAX_RATE = 0.2
/// ```
///
/// EvEJS's value was right; the belief that no artefact existed was not. This
/// is exactly the shape `docs/evejs/evejs-surprises.md` §5.2 names as rule zero --
/// **look in `out/pyc/eve/common/` and its siblings before taking anything on
/// a third party's word** -- and the three neighbouring rates come free for
/// whoever needs them next.
///
/// It applies only in a **safe-sec** system, which `eveuniverse/security.pyc`
/// defines as `securityStatus >= 0.95`. See [`Facility::safe_sec`] for why
/// ours is `false` and why that is a null result rather than a guess.
pub const INDUSTRY_SECURITY_TAX_RATE: f64 = 0.05;

/// What an NPC station takes of a job's base cost. **[I]**, **[V2]**.
///
/// There is no client-side table: `industry_installation_types.static` has no
/// NPC station type at all (see this module's header),
/// `stationoperations.fsdbinary`'s `manufacturingFactor` is the pre-Crius
/// per-station multiplier that nothing in `industry/` reads, and
/// `Activity.activity_tax`'s fallback is the structure `serviceAccess` map,
/// which a station does not have. EvEJS's
/// `industryConstants.DEFAULT_TAX_RATE` is `0.0` and that is what we send.
///
/// A capture of a real `InstallJob` preview at an NPC station would settle
/// it: the client's own `Job._get_tax` would disagree with ours in the
/// tooltip before anything else broke.
pub const NPC_STATION_TAX: f64 = 0.0;

/// `industry.Facility.sccTaxModifier`'s default, and EvEJS's
/// `DEFAULT_SCC_TAX_MODIFIER`. Multiplies [`SCC_SURCHARGE_RATE`], clamped at
/// zero below (`scc_rate = max(0, scc_rate)` in `Job._get_scc_surcharge_ratio`).
pub const DEFAULT_SCC_TAX_MODIFIER: f64 = 1.0;

/// The character type's own `manufactureSlotLimit` (attribute 196).
///
/// **1, read off character type 1373 in `typedogma.fsdbinary`** -- not the
/// attribute's `defaultValue`, which is 0. See this module's header.
pub const CHARACTER_BASE_MANUFACTURE_SLOTS: i64 = 1;

/// The highest [`max_slots`] can reach: base 1 plus five levels each of
/// `Mass Production` and `Advanced Mass Production`.
pub const MAX_MANUFACTURE_SLOTS: i64 = 11;

// ---------------------------------------------------------------------------
// Round 2: science. Every constant **[V]**, from `industry/const.pyc`,
// `industry/activities/*.pyc`, `evetypes/const.pyc`, `typelist.fsdbinary` and
// `typedogma.fsdbinary`, read with `tools/dis.py` and `tools/dogma.py`.
// ---------------------------------------------------------------------------

/// `industry.RESEARCH_TIMES`: each research level's multiplier on the
/// activity's base time, as the client spells them -- ten Python divisions,
/// so the float each one rounds to is the client's too.
///
/// Level 1 is the base time, level 10 is about 1,412x it; ten levels from
/// ME 0 is `sum(RESEARCH_TIMES)`, about 2,438x the base, which is why a fully
/// researched battleship takes weeks.
pub const RESEARCH_TIMES: [f64; 10] = [
    1.0,
    29.0 / 21.0,
    23.0 / 7.0,
    39.0 / 5.0,
    278.0 / 15.0,
    928.0 / 21.0,
    2200.0 / 21.0,
    5251.0 / 21.0,
    4163.0 / 7.0,
    29660.0 / 21.0,
];

/// `industry.INVENTION_MATERIAL_EFFICIENCY` / `INVENTION_TIME_EFFICIENCY`:
/// the ME and TE an invented copy starts with, before a decryptor.
pub const INVENTION_MATERIAL_EFFICIENCY: i64 = 2;
pub const INVENTION_TIME_EFFICIENCY: i64 = 4;

/// `industry.INVENTION_SKILL_PROBABILITY` (`1.0 / 30.0`) and
/// `INVENTION_SKILL_PROBABILITY_LOWER` (`1.0 / 40.0`): what one level of an
/// invention's required skill adds to the `1.0 + ...` multiplier.
pub const INVENTION_SKILL_PROBABILITY: f64 = 1.0 / 30.0;
pub const INVENTION_SKILL_PROBABILITY_LOWER: f64 = 1.0 / 40.0;

/// Type list 799 (`evetypes.TYPE_LIST_LOWER_INVENTION_SKILL_PROBABILITY`,
/// the list `SkillProbabilityHelper` reads to decide which skills get the
/// lower rate): its members, ascending: the four racial **Encryption
/// Methods** skills (21790 Caldari, 21791 Minmatar, 23087 Amarr, 23121
/// Gallente), 3408 Sleeper Encryption Methods, and 52308 / 55025.
///
/// **[V]**, read out of `res:/staticdata/typelist.fsdbinary` by raw cFSD
/// slots -- `tools/fsd.py` has no confirmed column layout for that table, so
/// the record was walked by hand: key 799's seventh slot is a length-prefixed
/// `uint32` array of exactly these seven ids, and it is the only array slot
/// in the record whose length is plausible for a type list. The four
/// encryption skills being there and the datacore skills not is the
/// well-known shape of the formula, which is the cross-check.
pub const LOWER_INVENTION_SKILL_PROBABILITY_TYPES: [i64; 7] =
    [3408, 21790, 21791, 23087, 23121, 52308, 55025];

/// `Copying.MAX_RUNS` and `Invention.MAX_RUNS`: 500 each, where every other
/// activity inherits `Activity.MAX_RUNS = MAX_RUNS_HARD_CAP`.
pub const COPYING_MAX_RUNS: i64 = 500;
pub const INVENTION_MAX_RUNS: i64 = 500;

/// `Research` (3403): -5% research-time job time per level, attribute 453
/// `blueprintmanufactureTimeBonus` onto the character's 385
/// `manufacturingTimeResearchSpeed`.
pub const SKILL_RESEARCH: i64 = 3403;
/// `Metallurgy` (3409): -5% research-material job time per level, attribute
/// 468 onto the character's 398 `mineralNeedResearchSpeed`.
pub const SKILL_METALLURGY: i64 = 3409;
/// `Laboratory Operation` (3406) and `Advanced Laboratory Operation`
/// (24624): +1 science slot per level each, their `laboratorySlotsBonus`
/// onto the character's 467 `maxLaborotorySlots`.
pub const SKILL_LABORATORY_OPERATION: i64 = 3406;
pub const SKILL_ADVANCED_LABORATORY_OPERATION: i64 = 24624;

/// The character type's own `maxLaborotorySlots` (attribute 467): **1** on
/// type 1373, like `manufactureSlotLimit`. So a fresh character has one
/// science slot, shared by all four science activities.
pub const CHARACTER_BASE_LABORATORY_SLOTS: i64 = 1;
/// 1 + five levels each of the two laboratory skills.
pub const MAX_LABORATORY_SLOTS: i64 = 11;

/// `inventorycommon.const.categoryBlueprint`, and `categoryAncientRelic`.
///
/// `industryCommon.IsBlueprintCategory` is `categoryID in (9, 34)`, and
/// `BlueprintInstance` forces `original = False` for category 34 whatever the
/// quantity says -- a relic is never an original.
#[cfg(test)]
pub const CATEGORY_BLUEPRINT: i64 = 9;

/// `inventorycommon.const.flagNone`, `flagHangar`.
#[cfg(test)]
pub const FLAG_NONE: i64 = 0;
pub const FLAG_HANGAR: i64 = 4;

/// `industryCommon.ACCOUNT_ENTRY_TYPES`: the wallet journal `refType` for
/// each activity's installation charge.
///
/// **[V]** from `eve/common/lib/appConst.pyc`. Nothing here spends them --
/// `crate::ledger` is a hot file and `Registry` has no public debit -- and
/// they are recorded because the wiring is one call once it does. See
/// [`Cost`].
pub const ACCOUNT_ENTRY_TYPES: [(i64, i64); 6] = [
    (MANUFACTURING, 56),
    (RESEARCH_TIME, 58),
    (RESEARCH_MATERIAL, 59),
    (COPYING, 60),
    (INVENTION, 57),
    (REACTION, 135),
];

/// One blue FILETIME second: 10,000,000 100-nanosecond ticks.
///
/// Job dates are win64 FILETIMEs, not seconds, and the magnitude is
/// load-bearing exactly as it is for `FILETIME_2100` in `service.rs`:
/// `industryCommon.JobData` puts all three dates through
/// `carbon.common.script.util.format.BlueToDate`, which calls
/// `blue.os.GetTimeParts` and raises on a small integer.
pub const FILETIME_SECOND: i64 = 10_000_000;

// ---------------------------------------------------------------------------
// The static layer.
// ---------------------------------------------------------------------------

/// One blueprint's `manufacturing` activity, as `blueprints.static` has it.
///
/// Field names are the client's. `materials` and `skills` are ascending by
/// type id ([`Table::parse`]); `time` is the activity's base seconds for
/// **one** run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blueprint {
    pub blueprint_type_id: i64,
    /// `activities.manufacturing.products[0].typeID`.
    pub product_type_id: i64,
    /// `activities.manufacturing.products[0].quantity`, the units one run
    /// yields. Not always 1.
    pub product_quantity: i64,
    /// `activities.manufacturing.time`, seconds per run before any modifier.
    pub time: i64,
    /// `maxProductionLimit`, the **copy licence** cap.
    ///
    /// Not a manufacturing cap: `Job._get_licensed_runs` clamps
    /// `licensedRuns` to it and `Copying.job_output_products` writes it into
    /// the copy. Manufacturing from an original is bounded by
    /// [`MAX_RUN_LENGTH`] instead -- see [`max_runs`].
    pub max_production_limit: i64,
    /// `(materialTypeID, quantity)` per run at ME 0.
    pub materials: Vec<(i64, i64)>,
    /// `(skillTypeID, level)` required.
    pub skills: Vec<(i64, i64)>,
}

/// One product of an invention activity: a Tech II blueprint type, the runs
/// each invented copy carries, and the base chance.
///
/// `quantity` is **runs per copy**, not copies: `Invention.job_output_products`
/// writes `runsRemaining = int(round(product.quantity + maxRuns))` and puts
/// `successfulRuns` in the copy's `quantity`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InventionProduct {
    pub type_id: i64,
    pub quantity: i64,
    /// `0.0` when the client has none, which `invention_probability` reads
    /// as `product.probability or 1.0`.
    pub probability: f64,
}

/// A blueprint's `invention` activity, as `blueprints.static` has it.
#[derive(Debug, Clone, PartialEq)]
pub struct Invention {
    /// Seconds per run before any modifier.
    pub time: i64,
    /// `(typeID, quantity)` per run: datacores, usually.
    pub materials: Vec<(i64, i64)>,
    pub products: Vec<InventionProduct>,
    /// `(skillTypeID, level)`: required **and** the skills the chance reads.
    pub skills: Vec<(i64, i64)>,
}

/// A blueprint's four science activities.
///
/// Research and copying are a time each, or `None` when the blueprint has no
/// such activity -- or has one with a material or skill list, which this
/// model does not serve ([`Table::parse`], [A]).
#[derive(Debug, Clone, PartialEq)]
pub struct Science {
    pub blueprint_type_id: i64,
    pub research_material_time: Option<i64>,
    pub research_time_time: Option<i64>,
    pub copying_time: Option<i64>,
    pub invention: Option<Invention>,
}

/// Whether a job from this blueprint could actually be applied today.
///
/// `false` when the product or any material is a type
/// [`crate::item::find_in`] does not know, because `item::Edit::Create` would
/// refuse the plan with `EditError::UnknownType`.
///
/// **Checked before the plan is built, not after**, so the refusal names the
/// missing type instead of surfacing as an opaque `apply` error two layers
/// up.
#[cfg(test)]
pub fn buildable(content: &crate::content::Content, bp: &Blueprint) -> bool {
    missing_type(content, bp).is_none()
}

/// The first type id a job from this blueprint would need and the item store
/// does not have.
pub fn missing_type(content: &crate::content::Content, bp: &Blueprint) -> Option<i64> {
    if item::find_in(content, bp.product_type_id).is_none() {
        return Some(bp.product_type_id);
    }
    bp.materials
        .iter()
        .map(|&(t, _)| t)
        .find(|&t| item::find_in(content, t).is_none())
}

/// The Estimated Item Value: what one run's materials are worth at ME 0.
///
/// `industryCommon.GetBlueprintPrice(blueprintTypeID, activityID)`:
///
/// ```text
///     blueprint = GetBlueprintType(typeID)          # cfg.blueprints[typeID]
///     if activityID in SCIENCE_ACTIVITIES: activityID = MANUFACTURING
///     materials = blueprint.activities[activityID].materials
///     return sum(GetBlueprintPrice(m.typeID, activityID) * m.quantity for m in materials)
///     # on KeyError / UserError: GetAdjustedAveragePrice(typeID)
/// ```
///
/// **The recursion never recurses in practice, and that is a property of the
/// table rather than of our subset.** `cfg.blueprints` is keyed by
/// *blueprint* type id, and a material is never a blueprint type -- a
/// manufactured component's blueprint is a different type id again -- so
/// every material takes the `KeyError` branch on the first call. Checked
/// against all 4,890 manufacturing rows: no material of any of them is a key
/// of the table. So EIV is one flat sum, the same for us as for the client.
///
/// The price is `P_ref` ([`Table::adjusted_price`]), the `adjustedPrice` the
/// client reads from `config.GetAverageMarketPrices`, so the preview and the
/// charge agree with a bundle loaded.
///
/// Note that the quantities are the blueprint's **base** quantities: EIV does
/// not move with ME, which is why a well-researched job is cheaper in
/// materials and not in ISK.
pub fn estimated_item_value(table: &Table, bp: &Blueprint) -> f64 {
    bp.materials
        .iter()
        .map(|&(t, q)| table.adjusted_price(t) * q as f64)
        .sum()
}

// ---------------------------------------------------------------------------
// The maths.
// ---------------------------------------------------------------------------

/// `round(x, 2)` the way Python 2 does it: half away from zero.
///
/// `f64::round` is also half-away-from-zero, which `f64::round_ties_even` is
/// not -- the distinction is why this is spelled out rather than inlined.
fn round_to(x: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (x * scale).round() / scale
}

/// How many units of one material a whole job needs.
///
/// `Job._get_materials`, exactly:
///
/// ```text
///     quantity = float(material.base)
///     for modifier in input_modifiers if MaterialModifier: quantity *= modifier.amount
///     quantity *= activity.job_material_runs(job)
///     max(int(ceil(round(quantity, 2))), self.runs)
/// ```
///
/// `base` is the blueprint's per-run quantity, `runs` is both a multiplier
/// and the floor, and `facility_multiplier` is the product of every
/// `MaterialModifier` a facility contributes -- `1.0` at an NPC station,
/// which has no assembly line (see the module header).
///
/// ME enters as `1.0 - material_efficiency / 100.0` and **only when
/// `material_efficiency` is non-zero**, which is the client's truthiness
/// guard; at ME 0 the term is absent rather than 1.0.
pub fn material_quantity(
    base: i64,
    runs: i64,
    material_efficiency: i64,
    facility_multiplier: f64,
) -> i64 {
    let mut quantity = base as f64;
    if material_efficiency != 0 {
        quantity *= 1.0 - material_efficiency as f64 / 100.0;
    }
    quantity *= facility_multiplier;
    quantity *= runs as f64;
    let rounded = round_to(quantity, MATERIAL_ROUND_PRECISION).ceil() as i64;
    rounded.max(runs)
}

/// Every material a job needs, in blueprint order.
pub fn job_materials(
    bp: &Blueprint,
    runs: i64,
    material_efficiency: i64,
    facility_multiplier: f64,
) -> Vec<Material> {
    bp.materials
        .iter()
        .map(|&(type_id, base)| Material {
            type_id,
            quantity: material_quantity(base, runs, material_efficiency, facility_multiplier),
        })
        .collect()
}

/// A level lookup over a `Skills`, clamped to the client's 0..=5.
///
/// Same helper as `reprocess::level_of`, and here for the same reason: three
/// of the four skills this module reads are **not in `crate::skill::SKILLS`**,
/// so a test that went through `Skills` could only ever exercise level 0.
fn level_of(skills: Option<&Skills>, type_id: i64) -> i64 {
    skills
        .and_then(|s| s.entries().find(|e| e.type_id == type_id))
        .map_or(0, |e| e.level.clamp(0, 5))
}

/// The character's `manufactureTimeMultiplier`.
///
/// `1.0 * (1 - 0.04*L(3380)) * (1 - 0.03*L(3388))`, times one `(1 + bonus/100)`
/// per implant carrying attribute 440. The base 1.0 is the character type's
/// own value for attribute 219 and the terms are "PostPercent" effects, so
/// they multiply rather than add. Every id and coefficient is measured; see
/// this module's header.
///
/// `level` answers "what level is this skill trained to", a closure rather
/// than a `&Skills`, because `crate::skill::SKILLS` has 3380 and **not** 3388,
/// 3387 or 24625 -- so the maths has to be testable without them. The rows to
/// add are in `docs/lanes/industry.md` §6.
///
/// Lower is faster, and the result is never allowed below zero: five levels
/// of everything plus the best implant is `0.80 * 0.85 * 0.92 = 0.6256`, so
/// the clamp is defensive rather than reachable.
pub fn time_multiplier<F>(level: F, implants: &[i64]) -> f64
where
    F: Fn(i64) -> i64,
{
    let mut m = 1.0;
    m *= 1.0 + -4.0 * level(SKILL_INDUSTRY) as f64 / 100.0;
    m *= 1.0 + -3.0 * level(SKILL_ADVANCED_INDUSTRY) as f64 / 100.0;
    for &implant in implants {
        m *= 1.0 + implant_time_bonus(implant) / 100.0;
    }
    m.max(0.0)
}

/// Attribute 440 on the six implants that carry it. Zero for anything else,
/// which is the `if amount is not None` guard in
/// `industryCommon.GetJobModifiers`.
///
/// **[V]**: 27167 `Zainou 'Beancounter' Industry BX-802` -2, 27170 `BX-801`
/// -1, 27171 `BX-804` -4, and the three Serenity `Beancounter Manufacturing`
/// implants 59797 / 59799 / 59801 at -8. Those six plus skill 3380 are the
/// *only* seven types in the whole table with attribute 440.
pub fn implant_time_bonus(type_id: i64) -> f64 {
    match type_id {
        27170 => -1.0,
        27167 => -2.0,
        27171 => -4.0,
        59797 | 59799 | 59801 => -8.0,
        _ => 0.0,
    }
}

/// The `REQUIRED_SKILL_MODIFIERS` term: attribute 1982 on a blueprint's own
/// required skills.
///
/// `1.0 + amount * level / 100.0` per required skill that has the attribute.
/// A Tech I blueprint requires only skills without 1982; a Tech II one
/// requires a production skill that has it.
pub fn required_skill_time_multiplier<F>(bp: &Blueprint, level: F) -> f64
where
    F: Fn(i64) -> i64,
{
    let mut m = 1.0;
    for &(skill, _) in &bp.skills {
        let per_level = required_skill_time_per_level(skill);
        if per_level != 0.0 {
            m *= 1.0 + per_level * level(skill) as f64 / 100.0;
        }
    }
    m
}

/// Attribute 1982 for one skill type. **[V]** over the whole table: 26 types
/// carry it, 81896 at -2 and the other 25 at -1, and every one of them is a
/// T2 production skill.
pub fn required_skill_time_per_level(type_id: i64) -> f64 {
    match type_id {
        81896 => -2.0,
        3395 | 3396 | 3397 | 3398 | 3400 | 11433 | 11441 | 11442 | 11443 | 11444 | 11445
        | 11446 | 11447 | 11448 | 11449 | 11450 | 11451 | 11452 | 11453 | 11454 | 11455 | 11529
        | 52307 | 77725 | 81050 => -1.0,
        _ => 0.0,
    }
}

/// How long a manufacturing job takes, in whole seconds.
///
/// `Job._get_time`: the activity's base time times the runs, then every
/// `TimeModifier` in turn, then `long(round(seconds))`. The TE term is
/// `1.0 - timeEfficiency / 100.0`, present only when TE is non-zero.
///
/// `character_multiplier` is [`time_multiplier`]'s result and
/// `facility_multiplier` is the facility's own time modifier -- `1.0` at an
/// NPC station.
pub fn job_seconds(
    bp: &Blueprint,
    runs: i64,
    time_efficiency: i64,
    character_multiplier: f64,
    facility_multiplier: f64,
) -> i64 {
    let mut seconds = bp.time as f64 * runs as f64;
    if time_efficiency != 0 {
        seconds *= 1.0 - time_efficiency as f64 / 100.0;
    }
    seconds *= character_multiplier;
    seconds *= facility_multiplier;
    seconds.round() as i64
}

/// The most runs this blueprint can take in one job.
///
/// `Activity.job_max_runs` for an original:
///
/// ```text
///     min(int(ceil(MAX_RUN_LENGTH / float(self.time) / reduce(mul, time_modifiers, 1.0))), MAX_RUNS)
/// ```
///
/// -- and `Manufacturing.job_max_runs` returns `min(runsRemaining, MAX_RUNS)`
/// for a copy instead. `runs_remaining` is `-1` for an original, which is the
/// client's own sentinel (`Blueprint.__new__` sets `runsRemaining = -1`), so
/// that is the branch taken here.
///
/// **`ceil` on a bound that is then used as a maximum is the client's**, and
/// it over-estimates by up to one run: at the boundary the job is a few
/// seconds over 30 days and `Job._get_errors`' own `RUN_LENGTH` check catches
/// it. Reproduced rather than corrected -- the client computes the same
/// number for its own slider, and a server that allowed one run fewer than
/// the slider offers is a client-side error with no message.
pub fn max_runs(
    bp: &Blueprint,
    runs_remaining: i64,
    time_efficiency: i64,
    character_multiplier: f64,
    facility_multiplier: f64,
) -> i64 {
    if runs_remaining >= 0 {
        return runs_remaining.min(MAX_RUNS_HARD_CAP);
    }
    let mut modifiers = character_multiplier * facility_multiplier;
    if time_efficiency != 0 {
        modifiers *= 1.0 - time_efficiency as f64 / 100.0;
    }
    if bp.time <= 0 || modifiers <= 0.0 {
        return MAX_RUNS_HARD_CAP;
    }
    let bound = (MAX_RUN_LENGTH as f64 / bp.time as f64 / modifiers).ceil() as i64;
    bound.clamp(1, MAX_RUNS_HARD_CAP)
}

/// How many manufacturing jobs one character may have running.
///
/// `1 + L(3387) + L(24625)`: the character type's own `manufactureSlotLimit`
/// plus one `SlotModifier` per level of each of the two skills carrying
/// attribute 450. See this module's header for why the base is 1 and not the
/// attribute's `defaultValue` of 0.
pub fn max_slots<F>(level: F) -> i64
where
    F: Fn(i64) -> i64,
{
    (CHARACTER_BASE_MANUFACTURE_SLOTS
        + level(SKILL_MASS_PRODUCTION)
        + level(SKILL_ADVANCED_MASS_PRODUCTION))
    .min(MAX_MANUFACTURE_SLOTS)
}

/// What a job costs to install, in ISK, broken out the way the client's own
/// tooltip is.
///
/// Every field is an integer because every one of the client's is: `base_cost`,
/// `cost`, `tax`, `scc_surcharge` and `industry_security_tax` are all
/// `int(round(...))` in `industry/job.pyc`. Rounding each term separately and
/// then summing is the client's order and differs from rounding the sum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cost {
    /// `int(round(EIV * runs))`. The base every other line is a fraction of.
    pub base: i64,
    /// `Job._get_cost`: the base after cost modifiers. Equal to `base` with
    /// none, which is every NPC-station job.
    pub cost: i64,
    /// `Job._get_tax`: the facility's cut.
    pub tax: i64,
    /// `Job._get_scc_surcharge`: 4% of base, halved for the two research
    /// activities.
    pub scc_surcharge: i64,
    /// `Job._get_industry_security_tax`: 5% of base in a safe-sec system,
    /// zero everywhere else.
    pub security_tax: i64,
}

impl Cost {
    /// `JobBase._get_total_cost`: `cost + tax + scc_surcharge + security_tax`.
    ///
    /// **Note what is not in it: `base`.** `cost` is the base after
    /// modifiers, so adding both would charge the job twice. The client's own
    /// sum has the same four terms.
    pub fn total(&self) -> i64 {
        self.cost + self.tax + self.scc_surcharge + self.security_tax
    }

    /// The wallet journal `refType` for this activity's charge, from
    /// [`ACCOUNT_ENTRY_TYPES`].
    pub fn ref_type(activity_id: i64) -> Option<i64> {
        ACCOUNT_ENTRY_TYPES
            .iter()
            .find(|(a, _)| *a == activity_id)
            .map(|(_, r)| *r)
    }
}

/// Price a job.
///
/// `cost_multiplier` is the product of the facility's and the blueprint's
/// `CostModifier`s -- `1.0` at an NPC station with a blueprint carrying no
/// attribute 1954. **[A]** that no blueprint carries it, since the bundle
/// has no blueprint attributes: falsified by a blueprint type with 1954 in
/// `typedogma`.
#[cfg(test)]
pub fn job_cost(
    table: &Table,
    bp: &Blueprint,
    runs: i64,
    activity_id: i64,
    facility: &Facility,
    cost_multiplier: f64,
) -> Cost {
    cost_lines(
        estimated_item_value(table, bp) * runs as f64,
        activity_id,
        facility,
        cost_multiplier,
    )
}

/// The four cost lines from an activity's raw `job_cost`, before rounding.
///
/// `Job._get_base_cost` is `int(round(activity.job_cost(job)))` and
/// `_get_cost_for_modifiers` multiplies the **unrounded** figure, so the raw
/// float is the input and both roundings happen here. Each activity's
/// `job_cost` is a different formula -- [`job_cost`] is manufacturing's,
/// the science ones are in [`Jobs::plan_install`] -- and this is everything
/// after it, which is the same for all six.
pub fn cost_lines(raw: f64, activity_id: i64, facility: &Facility, cost_multiplier: f64) -> Cost {
    let base = raw.round() as i64;
    let cost = (raw * cost_multiplier).round() as i64;
    let mut scc_rate = (SCC_SURCHARGE_RATE * facility.scc_tax_modifier).max(0.0);
    if activity_id == RESEARCH_TIME || activity_id == RESEARCH_MATERIAL {
        scc_rate *= SCC_SURCHARGE_RESEARCH_DISCOUNT_MODIFIER;
    }
    Cost {
        base,
        cost,
        tax: (base as f64 * facility.tax.unwrap_or(0.0)).round() as i64,
        scc_surcharge: (base as f64 * scc_rate).round() as i64,
        security_tax: if facility.safe_sec {
            (base as f64 * INDUSTRY_SECURITY_TAX_RATE).round() as i64
        } else {
            0
        },
    }
}

// ---------------------------------------------------------------------------
// The maths of round 2: research, copying, invention.
// ---------------------------------------------------------------------------

/// `activity.time`: the activity's base seconds, for one run (one level, for
/// research), or `None` when the blueprint has no such activity -- which is
/// `INCOMPATIBLE_ACTIVITY` (`activityID not in blueprint.activities`).
pub fn activity_time(table: &Table, bp: &Blueprint, activity_id: i64) -> Option<i64> {
    let sci = || table.science(bp.blueprint_type_id);
    let formula = table.is_formula(bp.blueprint_type_id);
    match activity_id {
        // A formula has the one activity, 9: asked to manufacture it is
        // `INCOMPATIBLE_ACTIVITY`, and so is a manufacturing blueprint asked
        // to react.
        MANUFACTURING if !formula => Some(bp.time),
        REACTION if formula => Some(bp.time),
        RESEARCH_MATERIAL => sci().and_then(|s| s.research_material_time),
        RESEARCH_TIME => sci().and_then(|s| s.research_time_time),
        COPYING => sci().and_then(|s| s.copying_time),
        INVENTION => sci().and_then(|s| s.invention.as_ref()).map(|i| i.time),
        _ => None,
    }
}

/// `Activity.is_compatible(blueprint)`: research, copying (and reactions)
/// set `REQUIRES_ORIGINAL`, invention sets `REQUIRES_COPY`, manufacturing
/// neither. `Blueprint.activities` filters on it, so an incompatible pair is
/// `INCOMPATIBLE_ACTIVITY` exactly like a missing activity.
pub fn is_compatible(activity_id: i64, original: bool) -> bool {
    match activity_id {
        RESEARCH_MATERIAL | RESEARCH_TIME | COPYING | REACTION => original,
        INVENTION => !original,
        _ => true,
    }
}

/// `ResearchMaterial.blueprint_level` / `ResearchTime.blueprint_level`: the
/// blueprint's current research level, `ME / STEP_MATERIAL_EFFICIENCY` or
/// `TE / STEP_TIME_EFFICIENCY`, with Python 2's flooring integer division.
pub fn research_level(activity_id: i64, material_efficiency: i64, time_efficiency: i64) -> i64 {
    if activity_id == RESEARCH_TIME {
        time_efficiency.div_euclid(STEP_TIME_EFFICIENCY)
    } else {
        material_efficiency.div_euclid(STEP_MATERIAL_EFFICIENCY)
    }
}

/// `blueprint_levels` and `job_time` of both research activities: the raw
/// seconds of researching `runs` levels from `level`, before any modifier.
///
/// ```text
///     levels = [self.time * RESEARCH_TIMES[i] for i in range(10)]
///     sum(levels[min(current + i, len(levels) - 1)] for i in range(runs))
/// ```
///
/// Summed left to right from zero, as Python's `sum` does, so the float is
/// the client's. A level below zero is refused before this is reached
/// (`RESEARCH_LIMIT`); it is clamped here rather than indexed negatively,
/// where Python would have wrapped to the far end of the list.
pub fn research_job_time(base_time: i64, level: i64, runs: i64) -> f64 {
    let levels = RESEARCH_TIMES.map(|m| base_time as f64 * m);
    (0..runs.max(0))
        .map(|i| levels[(level + i).clamp(0, levels.len() as i64 - 1) as usize])
        .sum()
}

/// The character's time multiplier for one activity.
///
/// `industryCommon.CHARACTER_MODIFIERS` gives each activity its own
/// character attribute, all `1.0` on character type 1373, and each skill
/// reaches its attribute through a `PostPercent` effect. Every pairing below
/// is read from that effect's `modifierInfo` in `dogmaeffects`, not inferred
/// from its name:
///
/// | activity | character attribute | skills (attribute, per level) |
/// | --- | --- | --- |
/// | manufacturing | 219 `manufactureTimeMultiplier` | see [`time_multiplier`] |
/// | research time | 385 `manufacturingTimeResearchSpeed` | 3403 Research (453, -5), 3388 Advanced Industry (1961, -3) |
/// | research material | 398 `mineralNeedResearchSpeed` | 3409 Metallurgy (468, -5), 3388 (1961, -3) |
/// | copying | 387 `copySpeedPercent` | 3402 Science (452, -5), 3388 (1961, -3) |
/// | invention | 1959 `inventionReverseEngineeringResearchSpeed` | 3388 (1961, -3) |
///
/// **No implant is folded for science** `[A]`: the research implants exist
/// (the Zainou `Beancounter` research line) and none is an item here.
pub fn activity_time_multiplier<F>(
    activity_id: i64,
    bp: &Blueprint,
    level: F,
    implants: &[i64],
) -> f64
where
    F: Fn(i64) -> i64,
{
    let per = |skill: i64, percent: f64| 1.0 + percent * level(skill) as f64 / 100.0;
    let advanced = per(SKILL_ADVANCED_INDUSTRY, -3.0);
    match activity_id {
        MANUFACTURING => {
            time_multiplier(&level, implants) * required_skill_time_multiplier(bp, &level)
        }
        RESEARCH_TIME => (per(SKILL_RESEARCH, -5.0) * advanced).max(0.0),
        RESEARCH_MATERIAL => (per(SKILL_METALLURGY, -5.0) * advanced).max(0.0),
        COPYING => (per(SKILL_SCIENCE, -5.0) * advanced).max(0.0),
        INVENTION => advanced.max(0.0),
        // MN-A: `reactionTimeMultiplier`, Reactions' PostPercent only.
        REACTION => crate::reactions::time_multiplier(&level),
        _ => 1.0,
    }
}

/// How many science jobs one character may run: `1 + L(3406) + L(24624)`,
/// the character's own `maxLaborotorySlots` plus one `SlotModifier` per level
/// of each laboratory skill. All four science activities read the same
/// attribute (`CHARACTER_MODIFIERS`' four `SlotModifier` rows).
pub fn laboratory_slots<F>(level: F) -> i64
where
    F: Fn(i64) -> i64,
{
    (CHARACTER_BASE_LABORATORY_SLOTS
        + level(SKILL_LABORATORY_OPERATION)
        + level(SKILL_ADVANCED_LABORATORY_OPERATION))
    .min(MAX_LABORATORY_SLOTS)
}

/// `Activity.job_max_runs`, the generic bound every activity starts from:
///
/// ```text
///     min(int(ceil(MAX_RUN_LENGTH / float(self.time) / reduce(mul, time_modifiers, 1.0))), self.MAX_RUNS)
/// ```
///
/// `self.time` is the activity's **one-run** time even for copying, whose
/// job time is also multiplied by the licensed runs -- the bound is CCP's
/// and is reproduced, not tightened; `RUN_LENGTH` catches the difference.
pub fn activity_max_runs(base_time: i64, time_modifiers: f64, cap: i64) -> i64 {
    if base_time <= 0 || time_modifiers <= 0.0 {
        return cap;
    }
    ((MAX_RUN_LENGTH as f64 / base_time as f64 / time_modifiers).ceil() as i64).min(cap)
}

/// `SkillProbabilityHelper.get_probability_from_type_id`: 1/40 for a skill
/// in type list 799, 1/30 for any other.
pub fn skill_probability(skill_type_id: i64) -> f64 {
    if LOWER_INVENTION_SKILL_PROBABILITY_TYPES.contains(&skill_type_id) {
        INVENTION_SKILL_PROBABILITY_LOWER
    } else {
        INVENTION_SKILL_PROBABILITY
    }
}

/// An invention job's chance per run: `Invention.job_probability` over the
/// two `ProbabilityModifier`s `Invention.job_modifiers` appends.
///
/// ```text
///     base   = industry.mean([p.probability or 1.0 for p in self.products])
///     amount = 1.0 + sum([job.skills.get(s.typeID, 0) * skill_probability(s.typeID) for s in self.skills])
///     max(min(reduce(mul, [base, amount], 1), 1.0), 0.0)
/// ```
///
/// **The mean is over every product, not the one chosen** -- the Rifter's
/// two (Wolf and Jaguar) are both 0.30, so it does not show there, and it
/// would for an activity whose products differed. A decryptor is the third
/// factor ([`invention_probability_with`]).
pub fn invention_probability<F>(inv: &Invention, level: F) -> f64
where
    F: Fn(i64) -> i64,
{
    invention_probability_with(inv, level, 1.0)
}

/// [`invention_probability`] with a decryptor's `ProbabilityModifier`
/// (attribute 1112) as the third factor. `Invention.job_probability` is
/// `max(min(reduce(mul, [every ProbabilityModifier.amount], 1), 1.0), 0.0)`:
/// the clamp is over the whole product, so a multiplier below one can bring
/// a chance the skills pushed past one back under it. `1.0` is no decryptor.
pub fn invention_probability_with<F>(inv: &Invention, level: F, multiplier: f64) -> f64
where
    F: Fn(i64) -> i64,
{
    let chances: Vec<f64> = inv
        .products
        .iter()
        .map(|p| {
            if p.probability != 0.0 {
                p.probability
            } else {
                1.0
            }
        })
        .collect();
    let base = if chances.is_empty() {
        0.0
    } else {
        chances.iter().sum::<f64>() / chances.len() as f64
    };
    let amount = 1.0
        + inv
            .skills
            .iter()
            .map(|&(skill, _)| level(skill) as f64 * skill_probability(skill))
            .sum::<f64>();
    (1.0 * base * amount * multiplier).clamp(0.0, 1.0)
}

/// How many of `runs` invention attempts succeed.
///
/// `JobBase._get_successful_runs` is the **shape** this keeps **[V]**: one
/// seed per job, one draw per run, and a run succeeds when its draw is
/// `<= probability` -- so the answer is fixed the moment the job is created
/// and replays identically from what is stored.
///
/// The **generator** is not CCP's, and is `[A]`: theirs is Python's
/// Mersenne Twister seeded with one `random.random()` the server drew and
/// kept, and neither the seed nor the draw ever reaches the client. Ours is
/// SplitMix64 over the seed [`job_seed`] derives from the job's own id and
/// start date (`server/deps.allow` is empty, so no `rand`). The client never
/// sees anything but the count, after delivery.
pub fn successful_runs(seed: u64, runs: i64, probability: f64) -> i64 {
    let mut state = seed;
    (0..runs.max(0))
        .filter(|_| unit_draw(&mut state) <= probability)
        .count() as i64
}

/// The seed an invention job's draws come from: its id and its start date,
/// both persisted, so a restart replays the same outcome `[A]`.
pub fn job_seed(job: &Job) -> u64 {
    (job.job_id as u64).rotate_left(32) ^ job.start_date as u64
}

/// One SplitMix64 step, as a float in `[0, 1)` with 53 bits of mantissa.
fn unit_draw(state: &mut u64) -> f64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

// ---------------------------------------------------------------------------
// The world: blueprints as items, facilities, jobs.
// ---------------------------------------------------------------------------

/// One material line of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Material {
    pub type_id: i64,
    pub quantity: i64,
}

/// One stack the character has, offered as job input.
///
/// Same shape and the same singleton rule as `reprocess::Input`: a singleton
/// is one object however its row reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stack {
    pub item_id: i64,
    pub type_id: i64,
    pub quantity: i64,
    pub slot: Slot,
}

impl Stack {
    pub fn of(item: &item::Item) -> Self {
        Stack {
            item_id: item.item_id,
            type_id: item.type_id,
            quantity: if item.singleton { 1 } else { item.quantity },
            slot: item.slot(),
        }
    }
}

/// A blueprint the character actually holds.
///
/// The fifteen fields `industryCommon.BlueprintInstance` copies off our row,
/// minus the four the item store cannot express. ME, TE and runs come from
/// [`Blueprints`] through [`Jobs::blueprint_item`]; see the module header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlueprintItem {
    /// `data.itemID` -> `blueprint.blueprintID`.
    pub item_id: i64,
    /// `data.typeID`, which must be a key of [`Table::blueprints`].
    pub type_id: i64,
    pub owner_id: i64,
    /// `data.locationID` / `locationTypeID` / `locationFlagID`: where the
    /// blueprint *is*. `flagID` is the item's own flag and
    /// `locationFlagID` is the container's; for a station hangar they are the
    /// same, which is why there is one field here and a note rather than two
    /// fields that are always equal.
    pub location_id: i64,
    pub location_type_id: i64,
    pub flag_id: i64,
    /// `data.runs` -> `blueprint.runsRemaining`. `-1` means an original with
    /// no limit, and it is the client's own sentinel.
    pub runs_remaining: i64,
    pub material_efficiency: i64,
    pub time_efficiency: i64,
}

impl BlueprintItem {
    /// An unresearched original, from an item row.
    ///
    /// `None` when the type is not a blueprint we know. ME, TE and runs are
    /// 0 / 0 / -1 because `item::Item` has no columns for them; the recorded
    /// ones are laid over this by [`Jobs::blueprint_item`], which is what an
    /// arm should call.
    pub fn original_of(table: &Table, item: &item::Item) -> Option<Self> {
        if !table.is_blueprint_type(item.type_id) {
            return None;
        }
        Some(BlueprintItem {
            item_id: item.item_id,
            type_id: item.type_id,
            owner_id: item.owner_id,
            location_id: item.location_id,
            location_type_id: 0,
            flag_id: item.flag_id,
            runs_remaining: -1,
            material_efficiency: 0,
            time_efficiency: 0,
        })
    }

    /// `data.quantity`, as the blueprint row must carry it.
    ///
    /// **`-1`, and the sign is load-bearing.**
    /// `industryCommon.BlueprintInstance` derives
    /// `singleton = data.quantity < 0` and
    /// `original = data.quantity != -2 and category != categoryAncientRelic`,
    /// then `quantity = max(data.quantity, 1)`. So `-1` is "one, singleton,
    /// original", `-2` is "a copy", and a positive quantity is a *stack* of
    /// non-singleton blueprints, which an original cannot be.
    pub fn wire_quantity(&self) -> i64 {
        if self.runs_remaining >= 0 { -2 } else { -1 }
    }

    /// `blueprint.original`.
    pub fn original(&self) -> bool {
        self.runs_remaining < 0
    }

    /// An ancient relic stack, as the invention-only copy the client treats
    /// it as (T3-B, `relic.rs`).
    ///
    /// `None` when the type is not a relic we know. `runs_remaining` is the
    /// stack's **quantity** (one unit per run, `[A]`, see `relic.rs`);
    /// ME and TE are 0, there is no recorded state to lay over it.
    pub fn relic_of(table: &Table, item: &item::Item) -> Option<Self> {
        if !table.is_relic(item.type_id) {
            return None;
        }
        Some(BlueprintItem {
            item_id: item.item_id,
            type_id: item.type_id,
            owner_id: item.owner_id,
            location_id: item.location_id,
            location_type_id: 0,
            flag_id: item.flag_id,
            runs_remaining: Stack::of(item).quantity.max(1),
            material_efficiency: 0,
            time_efficiency: 0,
        })
    }

    /// [`BlueprintItem::wire_quantity`], and a relic's own: a **positive**
    /// quantity (the stack size) -- `singleton = quantity < 0` is false, and
    /// `original` is false for category 34 whatever the sign.
    pub fn wire_quantity_in(&self, table: &Table) -> i64 {
        if table.is_relic(self.type_id) {
            self.runs_remaining.max(1)
        } else {
            self.wire_quantity()
        }
    }

    /// The static row behind this item: a blueprint's, or a relic's
    /// synthetic one.
    pub fn blueprint<'t>(&self, table: &'t Table) -> Option<&'t Blueprint> {
        table
            .lookup(self.type_id)
            .or_else(|| table.relic(self.type_id).map(|r| &r.blueprint))
    }

    /// This blueprint with a recorded ME, TE and runs laid over it.
    pub fn with_state(mut self, state: &BlueprintState) -> Self {
        self.runs_remaining = state.runs_remaining;
        self.material_efficiency = state.material_efficiency;
        self.time_efficiency = state.time_efficiency;
        self
    }

    /// The ME, TE and runs this blueprint carries now.
    pub fn state(&self) -> BlueprintState {
        BlueprintState {
            item_id: self.item_id,
            runs_remaining: self.runs_remaining,
            material_efficiency: self.material_efficiency,
            time_efficiency: self.time_efficiency,
        }
    }
}

/// A blueprint's ME, TE and runs, when they are not an unresearched
/// original's.
///
/// `runs_remaining` is `-1` for an original (the client's own sentinel, see
/// [`BlueprintItem::wire_quantity`]) and the runs left on a copy otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlueprintState {
    pub item_id: i64,
    pub runs_remaining: i64,
    pub material_efficiency: i64,
    pub time_efficiency: i64,
}

impl BlueprintState {
    /// What an item with no row is: an unresearched original.
    pub fn original(item_id: i64) -> Self {
        BlueprintState {
            item_id,
            runs_remaining: -1,
            material_efficiency: 0,
            time_efficiency: 0,
        }
    }
}

/// Every blueprint whose ME, TE or runs differ from an unresearched
/// original's, by item id.
///
/// Sorted by `item_id`, like [`Jobs`], and **persisted by rendering and
/// parsing its own records** (`bpstate`), the way `service::MarketBook` and
/// the bookmarks do, so `store.rs` carries it opaquely. See this module's
/// header for why it is a side table and not item columns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Blueprints {
    rows: Vec<BlueprintState>,
}

impl Blueprints {
    /// The record tags [`Blueprints::parse_record`] reads.
    pub const RECORDS: &'static [&'static str] = &["bpstate"];

    pub fn get(&self, item_id: i64) -> Option<&BlueprintState> {
        self.rows
            .binary_search_by_key(&item_id, |r| r.item_id)
            .ok()
            .map(|i| &self.rows[i])
    }

    /// The state of this item: its row, or an unresearched original's.
    pub fn state_of(&self, item_id: i64) -> BlueprintState {
        self.get(item_id)
            .copied()
            .unwrap_or_else(|| BlueprintState::original(item_id))
    }

    /// Record a state, replacing any row for the same item. An unresearched
    /// original's state is the absence of a row, so it removes one.
    pub fn set(&mut self, state: BlueprintState) {
        if state == BlueprintState::original(state.item_id) {
            self.remove(state.item_id);
            return;
        }
        match self
            .rows
            .binary_search_by_key(&state.item_id, |r| r.item_id)
        {
            Ok(i) => self.rows[i] = state,
            Err(i) => self.rows.insert(i, state),
        }
    }

    pub fn remove(&mut self, item_id: i64) {
        if let Ok(i) = self.rows.binary_search_by_key(&item_id, |r| r.item_id) {
            self.rows.remove(i);
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &BlueprintState> {
        self.rows.iter()
    }

    /// One `bpstate\titem\truns\tME\tTE` line per row.
    pub fn render(&self, out: &mut String) {
        use std::fmt::Write as _;
        for r in &self.rows {
            let _ = writeln!(
                out,
                "bpstate\t{}\t{}\t{}\t{}",
                r.item_id, r.runs_remaining, r.material_efficiency, r.time_efficiency
            );
        }
    }

    /// Read one record whose tag is in [`Blueprints::RECORDS`].
    ///
    /// **Strict**, like `MarketBook::parse_record`: a line we cannot read is
    /// a researched blueprint that would silently reload as ME 0, so the
    /// world is refused and the line named instead.
    pub fn parse_record(&mut self, f: &[&str]) -> Result<(), String> {
        let int = |i: usize| -> Result<i64, String> {
            f.get(i)
                .and_then(|v| v.parse::<i64>().ok())
                .ok_or_else(|| format!("field {i} is not an integer"))
        };
        match f.first().copied() {
            Some("bpstate") => {
                self.set(BlueprintState {
                    item_id: int(1)?,
                    runs_remaining: int(2)?,
                    material_efficiency: int(3)?,
                    time_efficiency: int(4)?,
                });
                Ok(())
            }
            other => Err(format!("{other:?} is not a blueprint record")),
        }
    }
}

/// Where a job runs.
///
/// The subset of `industry.Facility` an NPC station needs. The six fields
/// `industryCommon.Facility(data)` reads and this does not -- `rigModifiers`,
/// `globalModifiers`, `activities`, `distance`, and the nested 6-tuple and
/// 5-tuple modifier shapes -- are structure machinery, and the row we send
/// still has to carry them: `docs/lanes/industry.md` §3 has the exact shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Facility {
    pub facility_id: i64,
    pub type_id: i64,
    pub owner_id: i64,
    pub solar_system_id: i64,
    /// `data['tax']`. `None` sends the client down
    /// `Activity.activity_tax`'s `serviceAccess` path, which a station has
    /// no map for, so a station must send a number. See [`NPC_STATION_TAX`].
    pub tax: Option<f64>,
    pub scc_tax_modifier: f64,
    pub online: bool,
    /// The assembly line's `base_time_multiplier` / `base_material_multiplier`
    /// / `base_cost_multiplier`. All `1.0` for an NPC station, because
    /// `industry_installation_types.static` has no NPC station row at all.
    pub time_multiplier: f64,
    pub material_multiplier: f64,
    pub cost_multiplier: f64,
    /// `industry.Facility.distance`: jumps from the character's current
    /// system, which is `clientPathfinderService.GetJumpCountFromCurrent` on
    /// the client side.
    ///
    /// On the `Facility` rather than passed alongside it because that is
    /// where the client keeps it, and because `Job._get_distance` falls back
    /// to `self.facility.distance` when the job has none of its own. `0` for
    /// the station the character is docked in, which is the only facility an
    /// NPC-station server has.
    pub distance: i64,
    /// Whether the system is `securityClassSafeSec` (`securityStatus >= 0.95`),
    /// which is the only thing [`INDUSTRY_SECURITY_TAX_RATE`] keys off.
    ///
    /// **`false`, and it is a null result rather than a choice.** The client
    /// reads it from `cfg.mapSystemCache[id].securityStatus`, which is a
    /// *server*-supplied cache; `bin64/staticdata/mapObjects.db` has only
    /// `celestials` and `npcStations` and no security column, and
    /// `res:/staticdata/starmapcache.pickle`'s `solarSystems[30000001]` is
    /// `{center, sunTypeID, regionID, planetItemIDs, planetCountByType,
    /// neighbours, factionID, constellationID}` -- no security either. So
    /// the client ships no security level for any system and this cannot be
    /// verified from it at all.
    pub safe_sec: bool,
}

impl Facility {
    /// The station a character is docked in.
    ///
    /// Every multiplier is `1.0` and the tax is [`NPC_STATION_TAX`]; see
    /// those two items for the evidence. `online` is `true` because a station
    /// cannot be offline -- `Job._get_errors` raises `FACILITY_OFFLINE`
    /// otherwise and there is no way for a station to get there.
    pub fn npc_station(station_id: i64, owner_id: i64, type_id: i64, solar_system_id: i64) -> Self {
        Facility {
            facility_id: station_id,
            type_id,
            owner_id,
            solar_system_id,
            tax: Some(NPC_STATION_TAX),
            scc_tax_modifier: DEFAULT_SCC_TAX_MODIFIER,
            online: true,
            distance: 0,
            time_multiplier: 1.0,
            material_multiplier: 1.0,
            cost_multiplier: 1.0,
            safe_sec: false,
        }
    }
}

/// One industry job, with every field `industryCommon.JobData` reads.
///
/// Twenty-eight attribute reads, enumerated from the bytecode. They are
/// *attribute* reads, so the row we send has to be a `KeyVal` or a `DbRow`
/// and not a dict; `docs/lanes/industry.md` §3 has that side.
///
/// The three dates are blue FILETIMEs: `JobData` puts all three through
/// `BlueToDate`. See [`FILETIME_SECOND`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Job {
    pub job_id: i64,
    pub activity_id: i64,
    pub blueprint_id: i64,
    pub blueprint_type_id: i64,
    pub blueprint_location_id: i64,
    pub blueprint_location_flag_id: i64,
    /// `data.blueprintCopy`, which `industrySvc._JobInstance` passes straight
    /// to `GetBlueprintType(typeID, isCopy)`. `true` for a job installed from
    /// a copy: manufacturing from one, and every invention.
    pub blueprint_copy: bool,
    pub facility_id: i64,
    pub owner_id: i64,
    /// What we *store*. The client shows [`Job::status_at`] instead.
    pub status: Status,
    pub installer_id: i64,
    pub completed_character_id: Option<i64>,
    pub solar_system_id: i64,
    pub station_id: i64,
    pub start_date: i64,
    pub end_date: i64,
    pub pause_date: Option<i64>,
    pub runs: i64,
    pub licensed_runs: i64,
    pub successful_runs: i64,
    pub cost: i64,
    pub time_in_seconds: i64,
    pub probability: f64,
    pub product_type_id: i64,
    pub optional_type_id: Option<i64>,
    pub optional_type_id2: Option<i64>,
    pub output_location_id: i64,
    pub output_flag_id: i64,
}

impl Job {
    /// `industryCommon.JobStatus(data)`: the status the client will show.
    ///
    /// ```text
    ///     if status == STATUS_INSTALLED and endDate < now:          STATUS_READY
    ///     if status == STATUS_PAUSED and pauseDate > endDate:       STATUS_READY
    ///     otherwise:                                                status
    /// ```
    ///
    /// `now` is a blue FILETIME, the same units as `end_date`.
    pub fn status_at(&self, now: i64) -> Status {
        match self.status {
            Status::Installed if self.end_date < now => Status::Ready,
            Status::Paused if self.pause_date.is_some_and(|p| p > self.end_date) => Status::Ready,
            s => s,
        }
    }

    /// `JobBase.completed`.
    pub fn completed(&self) -> bool {
        self.status.completed()
    }

    /// Whether the job's own clock has run out.
    pub fn ready_at(&self, now: i64) -> bool {
        self.status_at(now) == Status::Ready
    }

    /// What the job will deliver: product type and total units.
    pub fn product(&self, table: &Table) -> Material {
        let per_run = table
            .lookup(self.blueprint_type_id)
            .map_or(1, |b| b.product_quantity.max(1));
        Material {
            type_id: self.product_type_id,
            quantity: per_run * self.successful_runs.max(0),
        }
    }
}

/// What the caller asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallRequest {
    pub activity_id: i64,
    /// `request['blueprintID']` -- the item id of the blueprint.
    pub blueprint_id: i64,
    pub runs: i64,
    /// `request['inputLocation']` / `['outputLocation']`, flattened. The
    /// client sends `industry.Location` instances with six fields each; only
    /// the address survives contact with our store.
    pub input_location_id: i64,
    pub input_flag_id: i64,
    pub output_location_id: i64,
    pub output_flag_id: i64,
    /// `request['licensedRuns']`: the runs each copy of a copying job
    /// carries. `0` when absent, which `Job._get_licensed_runs` reads as its
    /// own `or 1`; copying refuses it ([`Refusal::InvalidLicensedRuns`]).
    pub licensed_runs: i64,
    /// `request['productTypeID']`: which product an invention with more than
    /// one makes (the Rifter's: a Wolf or a Jaguar blueprint).
    /// `Job._get_product` takes the only one when there is one, so `None` is
    /// fine then.
    pub product_type_id: Option<i64>,
    /// The generic decryptor the request's `materials` names (T3-C), by
    /// `relic::decryptor_among`; invention only. One unit is consumed.
    pub decryptor: Option<i64>,
}

/// Who is installing it.
#[derive(Debug, Clone, Copy)]
pub struct Pilot<'a> {
    pub character_id: i64,
    /// `request['corporationID']`. A corporation job needs
    /// `ROLE_FACTORY_MANAGER` and `ROLE_FACTORY_MANUFACTURING`, which
    /// `crate::roles` can answer and nothing here asks: an owner other than
    /// the character is refused outright, see [`Refusal::InvalidOwner`].
    pub corporation_id: Option<i64>,
    pub skills: Option<&'a Skills>,
    pub implants: &'a [i64],
    /// The wallet, for `Error::ACCOUNT_FUNDS`.
    pub balance: f64,
}

/// Why an install was refused.
///
/// Each variant is one of `industry.Error`'s 40 codes and [`Refusal::code`]
/// is that code, so the dispatch arm can put it in a
/// `industry.ValidationError` without a second table. The subset is the 19 of
/// the 40 that an NPC-station job can actually reach -- round 2 added 5
/// `INVALID_LICENSED_RUNS` and 39 `RESEARCH_LIMIT`; the other 21 need
/// corporations, structures or a cost pre-agreement.
/// `docs/lanes/industry.md` §5 lists all 40.
///
/// `Eq` is deliberately **not** derived: [`Refusal::AccountFunds`] carries
/// the wallet balance as the `f64` the ledger keeps it in, and an `Eq` on a
/// float would be a promise this type cannot make.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Refusal {
    /// 1. The owner is not the installing character. No corporation jobs.
    InvalidOwner,
    /// 4. Runs <= 0, or more than a copy has left.
    InvalidRuns(i64),
    /// 6 / 7. The input or output location is not one the job can use.
    MissingInputLocation,
    MissingOutputLocation,
    /// 8. No product -- a blueprint whose manufacturing activity has none.
    InvalidProduct,
    /// 10. Not an activity id at all.
    InvalidActivity(i64),
    /// 11 / 15. No facility, or a facility that is offline.
    MissingFacility,
    FacilityOffline,
    /// 14. The blueprint item is not a blueprint, or not one we know.
    MissingBlueprint,
    /// 16. Not enough of a material: `(typeID, needed, available)`.
    MissingMaterial(i64, i64, i64),
    /// 19. Not enough ISK: `(balance, required)`.
    AccountFunds(f64, i64),
    /// 21. The blueprint has no such activity.
    IncompatibleActivity(i64),
    /// 22. The blueprint is not the character's.
    BlueprintAccess,
    /// 23. The blueprint is already in a job.
    BlueprintInstalled(i64),
    /// 30. A required skill is short: `(typeID, needed, actual)`.
    MissingSkill(i64, i64, i64),
    /// 31. Every manufacturing slot is busy: `(max, used)`.
    SlotsFull(i64, i64),
    /// 32. The facility is further away than the character can reach,
    ///     as `(distance, max)`.
    FacilityDistance(i64, i64),
    /// 38. One run of this job is already longer than 30 days.
    RunLength(i64, i64),
    /// Code 5: a copy's licensed runs are not in `1..=maxProductionLimit`;
    /// the payload is that limit.
    InvalidLicensedRuns(i64),
    /// Code 39: the blueprint is already at the research limit, or below
    /// zero -- `(level, max_level)`, `ResearchMaterial.job_validate`'s args.
    ResearchLimit(i64, i64),
    /// **Not one of CCP's codes.** A product or material type the item store
    /// does not know, which `item::Edit::Create` would refuse anyway. Carried
    /// separately so the message names the type instead of surfacing as an
    /// opaque `EditError` two layers up; [`Refusal::code`] maps it onto 8
    /// `INVALID_PRODUCT`, which is the closest thing the client has a label
    /// for.
    UnknownType(i64),
    /// Code 28 `MISMATCH_MATERIAL`: a decryptor the request names that is not
    /// one the activity offers (not in group 1304, or on a job that takes
    /// none).
    MismatchMaterial(i64),
}

impl Refusal {
    /// The `industry.Error` code, for a `ValidationError` the client can
    /// label through `industryCommon.GetErrorLabel`.
    pub fn code(self) -> i64 {
        match self {
            Refusal::InvalidOwner => 1,
            Refusal::InvalidRuns(_) => 4,
            Refusal::MissingInputLocation => 17,
            Refusal::MissingOutputLocation => 18,
            Refusal::InvalidProduct | Refusal::UnknownType(_) => 8,
            Refusal::InvalidActivity(_) => 10,
            Refusal::MissingFacility => 15,
            Refusal::FacilityOffline => 40,
            Refusal::MissingBlueprint => 14,
            Refusal::MissingMaterial(..) => 16,
            Refusal::AccountFunds(..) => 19,
            Refusal::IncompatibleActivity(_) => 21,
            Refusal::BlueprintAccess => 22,
            Refusal::BlueprintInstalled(_) => 23,
            Refusal::MissingSkill(..) => 30,
            Refusal::SlotsFull(..) => 31,
            Refusal::FacilityDistance(..) => 32,
            Refusal::RunLength(..) => 38,
            Refusal::InvalidLicensedRuns(_) => 5,
            Refusal::ResearchLimit(..) => 39,
            Refusal::MismatchMaterial(_) => 28,
        }
    }
}

/// A job resolved and priced, with the item plan that installs it.
///
/// Nothing has happened yet. The caller applies [`Install::edits`] through
/// `Registry::apply_items` -- which either performs all of them or none --
/// and only then commits [`Install::job`] with [`Jobs::commit`]. Two phases
/// for the same reason `item::Store::apply` has two: a job recorded against
/// materials that were never consumed is worse than no job.
#[derive(Debug, Clone, PartialEq)]
pub struct Install {
    pub job: Job,
    /// One `Edit::Consume` per stack drawn on, in item-id order. **The
    /// blueprint is not in here**: it stays where it is and the client learns
    /// it is busy from `blueprint.jobID`. See the module header's EvEJS note.
    pub edits: Vec<Edit>,
    pub cost: Cost,
    /// What the job will eat, per material type.
    pub materials: Vec<Material>,
    /// The copy's new runs, when the job spends a copy's runs
    /// (manufacturing or invention from a copy). A copy left with **zero**
    /// is consumed by an `Edit::Consume` in [`Install::edits`] and its row is
    /// dropped by [`Jobs::commit_install`] `[A]`: the client's own
    /// `Invention.job_output_extras` hands back a copy with
    /// `max(runsRemaining - runs, 0)` runs, and what the server then does with
    /// an empty one is not in the client.
    pub blueprint_after: Option<BlueprintState>,
}

impl Install {
    /// **The game clock's pace** (lane `clock`): the job ends
    /// `clock.span(timeInSeconds)` after its start instead of
    /// `timeInSeconds`. `timeInSeconds` stays the job's nominal length, the
    /// figure the client's own install window computed **[A]**: the client's
    /// progress bar reads `startDate`/`endDate`, which agree with the
    /// server's delivery. Falsified by a client that derives its countdown
    /// from `timeInSeconds`. Scale 1 returns the install unchanged.
    pub fn paced(mut self, clock: crate::game_clock::GameClock) -> Install {
        if !clock.is_real() {
            let nominal = self.job.time_in_seconds.saturating_mul(FILETIME_SECOND);
            self.job.end_date = self.job.start_date.saturating_add(clock.span(nominal));
        }
        self
    }
}

/// A finished job's output plan.
#[derive(Debug, Clone, PartialEq)]
pub struct Delivery {
    pub job_id: i64,
    /// One `Edit::Create`, or none when the job produced nothing.
    pub edits: Vec<Edit>,
    pub product: Material,
    /// The researched blueprint's new ME or TE, for a research job.
    pub blueprint_after: Option<BlueprintState>,
    /// The ME, TE and runs every blueprint the `Edit::Create` mints starts
    /// with, for a copying or invention job. `item_id` is `0`: the ids are
    /// the store's to choose, and [`Jobs::commit_delivery`] reads them off
    /// the `item::Change::Created`s the plan produced.
    pub creates: Option<BlueprintState>,
}

/// Every job the world holds.
///
/// A `Vec` plus an id allocator, like `item::Store`, and kept sorted by
/// `job_id` so the Jobs tab's order is stable across reads. The `Registry`
/// owns one of these; all the logic is here.
#[derive(Debug, Clone, Default)]
pub struct Jobs {
    jobs: Vec<Job>,
    next_id: i64,
    /// ME, TE and runs, by blueprint item id. Here rather than on the
    /// `Registry` because every plan in this type reads it; see
    /// [`Blueprints`].
    blueprints: Blueprints,
}

/// The first job id. Above `item::MIN_PLAYER_ITEM` for the reason recorded
/// there: `idCheckers.IsSystemOrNPC(id)` is `id < 90000000` and the client
/// answers questions about such ids from its own sqlite before asking us.
pub const MIN_JOB_ID: i64 = 200_000_000;

impl Jobs {
    pub fn new() -> Self {
        Jobs {
            jobs: Vec::new(),
            next_id: MIN_JOB_ID,
            blueprints: Blueprints::default(),
        }
    }

    /// Jobs read back from disk. The blueprint table is restored separately,
    /// by [`Jobs::restore_blueprints`], because it is its own record type.
    pub fn restore(jobs: Vec<Job>, next_id: i64) -> Self {
        let mut j = Jobs {
            jobs,
            next_id: next_id.max(MIN_JOB_ID),
            blueprints: Blueprints::default(),
        };
        j.jobs.sort_by_key(|j| j.job_id);
        j
    }

    /// The blueprint table read back from disk, replacing whatever is here.
    pub fn restore_blueprints(&mut self, blueprints: Blueprints) {
        self.blueprints = blueprints;
    }

    /// Every recorded ME, TE and runs, for the snapshot.
    pub fn blueprints(&self) -> &Blueprints {
        &self.blueprints
    }

    /// Record a blueprint's ME, TE and runs for a copy minted outside a job
    /// (HK3: a hacked can's copies); an original's state removes the row.
    pub fn set_blueprint(&mut self, state: BlueprintState) {
        self.blueprints.set(state);
    }

    /// This item as a blueprint, with its recorded ME, TE and runs, or
    /// `None` when it is not a blueprint type we know.
    ///
    /// **The one way an arm should turn an item into a blueprint**:
    /// [`BlueprintItem::original_of`] alone would show a researched original
    /// as ME 0 and a copy as an original.
    pub fn blueprint_item(&self, table: &Table, item: &item::Item) -> Option<BlueprintItem> {
        BlueprintItem::original_of(table, item)
            .map(|b| b.with_state(&self.blueprints.state_of(item.item_id)))
            // T3-B: an ancient relic has no recorded state; its runs are its stack.
            .or_else(|| BlueprintItem::relic_of(table, item))
    }

    pub fn all(&self) -> &[Job] {
        &self.jobs
    }

    pub fn next_id(&self) -> i64 {
        self.next_id
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn get(&self, job_id: i64) -> Option<&Job> {
        self.jobs
            .binary_search_by_key(&job_id, |j| j.job_id)
            .ok()
            .map(|i| &self.jobs[i])
    }

    /// `industryManager.GetJobsByOwner(ownerID, includeCompleted)`.
    ///
    /// The flag is the client's second argument and it means what it says:
    /// `industrySvc.GetOwnerJobs` filters on `job.status < STATUS_COMPLETED`
    /// itself when it is false, so answering with completed jobs anyway would
    /// merely be wasteful -- but it caches on a separate
    /// `_ownerJobsFetchedCompleted` set, so answering *without* them when
    /// asked leaves the Jobs tab's history permanently empty.
    pub fn for_owner(&self, owner_id: i64, include_completed: bool) -> Vec<&Job> {
        self.jobs
            .iter()
            .filter(|j| j.owner_id == owner_id)
            .filter(|j| include_completed || !j.completed())
            .collect()
    }

    /// The job this blueprint item is installed in, if any.
    ///
    /// This is the client's `blueprint.jobID` and the whole of our
    /// "installed" bookkeeping -- there is no sentinel location and the
    /// blueprint does not move.
    pub fn job_for_blueprint(&self, blueprint_item_id: i64) -> Option<&Job> {
        self.jobs
            .iter()
            .find(|j| j.blueprint_id == blueprint_item_id && !j.completed())
    }

    /// `industryManager.GetJobCounts(ownerID)`: live jobs per activity.
    ///
    /// **Int keys matter on the wire** and the dict must not be empty;
    /// `service::industry` has both reasons. Here it is just the counts, with
    /// a zero for every activity so the caller cannot accidentally send a
    /// short dict.
    pub fn counts(&self, owner_id: i64) -> [(i64, i64); 6] {
        let mut out = [(0i64, 0i64); 6];
        for (slot, activity) in out.iter_mut().zip(ACTIVITIES) {
            *slot = (
                activity,
                self.jobs
                    .iter()
                    .filter(|j| j.owner_id == owner_id && !j.completed())
                    .filter(|j| j.activity_id == activity)
                    .count() as i64,
            );
        }
        out
    }

    /// How many of this owner's slots for this activity are in use.
    pub fn used_slots(&self, owner_id: i64, activity_id: i64) -> i64 {
        self.slot_counts(owner_id)
            .iter()
            .find(|(a, _)| *a == activity_id)
            .map_or(0, |(_, n)| *n)
    }

    /// [`Jobs::counts`] as slots: each science activity answers the live jobs
    /// of **all four**, because all four draw on the one
    /// `maxLaborotorySlots` ([`laboratory_slots`]).
    ///
    /// This is what `GetJobCounts` sends: the client's `job.used_slots` is
    /// `self.slots.get(self.activityID)` straight off that call and it
    /// compares it with `max_slots`, so a per-activity count would let a
    /// one-slot character run a copy and a research job at once in the
    /// client's own arithmetic. `[A]` that retail sums them on the wire
    /// rather than the client summing them -- nothing in the client sums.
    pub fn slot_counts(&self, owner_id: i64) -> [(i64, i64); 6] {
        let raw = self.counts(owner_id);
        let science: i64 = raw
            .iter()
            .filter(|(a, _)| is_science(*a))
            .map(|(_, n)| n)
            .sum();
        raw.map(|(a, n)| (a, if is_science(a) { science } else { n }))
    }

    /// Resolve and price an install without changing anything.
    ///
    /// The checks are `Job._get_errors`' in its own order, minus the ones
    /// that need a corporation, a structure or a cost pre-agreement. Order
    /// matters only for which refusal a caller sees first, and the client's
    /// order is the one a player's tooltip is written against.
    ///
    /// `on_hand` is every stack the character could draw on -- the caller
    /// filters by owner and location, exactly as `service::reprocess`'s
    /// `inputs_for` does, so an item id from another station's cache cannot
    /// be consumed remotely.
    ///
    /// The facility's own `distance` is the reach check's input; see
    /// [`Facility::distance`].
    // The table is the eighth argument: the rows are data, not state of
    // `Jobs`, and bundling them into `Pilot` would hide which reads them.
    #[allow(clippy::too_many_arguments)]
    pub fn plan_install(
        &self,
        table: &Table,
        content: &crate::content::Content,
        now: i64,
        pilot: &Pilot<'_>,
        facility: &Facility,
        blueprint: &BlueprintItem,
        request: &InstallRequest,
        on_hand: &[Stack],
    ) -> Result<Install, Refusal> {
        let activity = request.activity_id;
        // `if self.activityID not in industry.ACTIVITY_CLASSES: INVALID_ACTIVITY`
        if !ACTIVITIES.contains(&activity) {
            return Err(Refusal::InvalidActivity(activity));
        }
        // `if self.blueprint.blueprintID is None: MISSING_BLUEPRINT`
        if blueprint.item_id != request.blueprint_id {
            return Err(Refusal::MissingBlueprint);
        }
        let Some(bp) = blueprint.blueprint(table) else {
            return Err(Refusal::MissingBlueprint);
        };
        // `if self.ownerID not in (self.characterID, self.corporationID): INVALID_OWNER`
        // -- `Job._get_ownerID` is `self.blueprint.ownerID`, so the owner of
        // the job *is* the owner of the blueprint. A corporation job needs
        // two roles we do not model, so an owner that is not the character is
        // refused rather than half-allowed.
        if blueprint.owner_id != pilot.character_id {
            return Err(Refusal::InvalidOwner);
        }
        // `if not self.blueprint_location: INVALID_BLUEPRINT_LOCATION` /
        // `elif not canView: BLUEPRINT_ACCESS`. We have no office divisions,
        // so "can view" collapses to "is yours and is here".
        if blueprint.location_id != facility.facility_id {
            return Err(Refusal::BlueprintAccess);
        }
        // `if self.facility is None: MISSING_FACILITY` / `FACILITY_OFFLINE`
        if facility.facility_id == 0 {
            return Err(Refusal::MissingFacility);
        }
        if !facility.online {
            return Err(Refusal::FacilityOffline);
        }
        // `if self.distance is None or self.distance > self.max_distance:
        //      FACILITY_DISTANCE`. Level 0 of the activity's distance skill
        // (Supply Chain Management, or Scientific Networking for science) is
        // `DISTANCE_PER_LEVEL[0] == 0` jumps, so a remote facility is
        // refused for a fresh character -- which is correct and is why the
        // table is indexed rather than assumed to start at 1.
        let level = |t: i64| level_of(pilot.skills, t);
        let reach = max_distance(activity, level);
        if facility.distance < 0 || facility.distance > reach {
            return Err(Refusal::FacilityDistance(facility.distance, reach));
        }
        // `if self.blueprint.jobID is not None: BLUEPRINT_INSTALLED` -- except
        // for an ancient relic stack, whose units a job takes at install
        // (`relic.rs`), so what is left of the stack is free for another job.
        let relic = table.is_relic(blueprint.type_id);
        if !relic && let Some(j) = self.job_for_blueprint(blueprint.item_id) {
            return Err(Refusal::BlueprintInstalled(j.job_id));
        }
        if request.input_location_id == 0 {
            return Err(Refusal::MissingInputLocation);
        }
        if request.output_location_id == 0 {
            return Err(Refusal::MissingOutputLocation);
        }
        // `if self.activityID not in self.blueprint.activities:
        //      INCOMPATIBLE_ACTIVITY` -- and `Blueprint.activities` is the
        // static activities filtered by `Activity.is_compatible`, so an
        // original asked to invent and a copy asked to research land here
        // too. A reaction lands here unless the blueprint is a formula (and
        // a formula unless the activity is a reaction): MN-A.
        let Some(base_time) = activity_time(table, bp, activity) else {
            return Err(Refusal::IncompatibleActivity(activity));
        };
        if !is_compatible(activity, blueprint.original()) {
            return Err(Refusal::IncompatibleActivity(activity));
        }
        let invention = match activity {
            INVENTION => table
                .science(bp.blueprint_type_id)
                .and_then(|s| s.invention.as_ref()),
            _ => None,
        };
        // T3-C: the decryptor `GetDecryptors` offers an invention job, or a
        // refusal for a type it does not offer (`MISMATCH_MATERIAL`).
        let decryptor = match (request.decryptor, activity) {
            (None, _) => None,
            (Some(t), INVENTION) => {
                Some(Decryptor::of(content, t).ok_or(Refusal::MismatchMaterial(t))?)
            }
            (Some(t), _) => return Err(Refusal::MismatchMaterial(t)),
        };

        // `if not self.product: INVALID_PRODUCT`, and our own check that the
        // item store can mint what the job makes and eats -- a type it
        // cannot would fail in `apply`'s phase one with no name on it.
        let product_type_id = match (activity, invention) {
            (MANUFACTURING | REACTION, _) => {
                if bp.materials.is_empty() && bp.product_type_id == 0 {
                    return Err(Refusal::InvalidProduct);
                }
                if let Some(t) = missing_type(content, bp) {
                    return Err(Refusal::UnknownType(t));
                }
                bp.product_type_id
            }
            (INVENTION, Some(inv)) => {
                // `Job._get_product`: the only product when there is one,
                // else the one whose type the request names, else `None`.
                let chosen = match &inv.products[..] {
                    [only] => Some(only.type_id),
                    many => request
                        .product_type_id
                        .filter(|t| many.iter().any(|p| p.type_id == *t)),
                };
                let Some(product) = chosen else {
                    return Err(Refusal::InvalidProduct);
                };
                if table.find(product).is_none() {
                    return Err(Refusal::InvalidProduct);
                }
                let unknown = std::iter::once(product)
                    .chain(inv.materials.iter().map(|&(t, _)| t))
                    .find(|&t| item::find_in(content, t).is_none());
                if let Some(t) = unknown {
                    return Err(Refusal::UnknownType(t));
                }
                product
            }
            // Research and copying make the blueprint's own type: a better
            // original, or copies of it. `[A]` that `productTypeID` on the
            // job row is the blueprint type; the client reads it only to
            // label the job.
            _ => bp.blueprint_type_id,
        };

        let character_time = activity_time_multiplier(activity, bp, level, pilot.implants);
        let time_modifiers = character_time * facility.time_multiplier;

        // `ResearchMaterial.job_validate` / `ResearchTime.job_validate`:
        // `RESEARCH_LIMIT` when the level is below zero or already the last.
        // Checked before the runs, whose ceiling is zero at the limit, so the
        // refusal names the reason rather than the symptom.
        let research = matches!(activity, RESEARCH_MATERIAL | RESEARCH_TIME);
        let current_level = research_level(
            activity,
            blueprint.material_efficiency,
            blueprint.time_efficiency,
        );
        let max_level = RESEARCH_TIMES.len() as i64;
        if research && !(0..max_level).contains(&current_level) {
            return Err(Refusal::ResearchLimit(current_level, max_level));
        }

        // `Job._get_licensed_runs`: `max(min(licensedRuns or 1,
        // maxProductionLimit), 1)`, and copying's own check in
        // `Job._get_errors`: `licensedRuns <= 0 or licensedRuns >
        // blueprint.maxProductionLimit` is `INVALID_LICENSED_RUNS`.
        let licensed_runs = match activity {
            COPYING => {
                if request.licensed_runs <= 0 || request.licensed_runs > bp.max_production_limit {
                    return Err(Refusal::InvalidLicensedRuns(bp.max_production_limit));
                }
                request.licensed_runs
            }
            // Round 1's value, kept: a manufacturing job's licensed runs are
            // read by nothing but the row.
            MANUFACTURING => request.runs.min(bp.max_production_limit.max(1)),
            _ => {
                let asked = if request.licensed_runs != 0 {
                    request.licensed_runs
                } else {
                    1
                };
                asked.min(bp.max_production_limit).max(1)
            }
        };

        // `if runs <= 0 or runs > runsRemaining and runsRemaining != -1:
        //      INVALID_RUNS`, then `Job._get_runs`' own
        // `max(min(runs, maxRuns), 1)` clamp. The ceiling is each activity's
        // `job_max_runs`.
        let ceiling = match activity {
            MANUFACTURING => max_runs(
                bp,
                blueprint.runs_remaining,
                blueprint.time_efficiency,
                character_time,
                facility.time_multiplier,
            ),
            // `Activity.job_max_runs` for an original, and a formula is one.
            REACTION => activity_max_runs(base_time, time_modifiers, MAX_RUNS_HARD_CAP),
            RESEARCH_MATERIAL => activity_max_runs(base_time, time_modifiers, MAX_RUNS_HARD_CAP)
                .min(
                    (MAX_MATERIAL_EFFICIENCY - blueprint.material_efficiency)
                        .div_euclid(STEP_MATERIAL_EFFICIENCY),
                ),
            RESEARCH_TIME => activity_max_runs(base_time, time_modifiers, MAX_RUNS_HARD_CAP).min(
                (MAX_TIME_EFFICIENCY - blueprint.time_efficiency).div_euclid(STEP_TIME_EFFICIENCY),
            ),
            COPYING => activity_max_runs(base_time, time_modifiers, COPYING_MAX_RUNS),
            _ => blueprint.runs_remaining.min(activity_max_runs(
                base_time,
                time_modifiers,
                INVENTION_MAX_RUNS,
            )),
        };
        if request.runs <= 0 || request.runs > ceiling {
            return Err(Refusal::InvalidRuns(ceiling));
        }

        // `for skill in self.required_skills: if actual < skill.level:
        //      MISSING_SKILL` -- `activity.skills`, which only manufacturing
        // and invention have.
        let required: &[(i64, i64)] = match (activity, invention) {
            (MANUFACTURING | REACTION, _) => &bp.skills,
            (_, Some(inv)) => &inv.skills,
            _ => &[],
        };
        for &(skill, want) in required {
            let actual = level(skill);
            if actual < want {
                return Err(Refusal::MissingSkill(skill, want, actual));
            }
        }

        // `if self.used_slots >= self.max_slots: SLOTS_FULL`
        let used = self.used_slots(blueprint.owner_id, activity);
        let slots = if is_science(activity) {
            laboratory_slots(level)
        } else if activity == REACTION {
            crate::reactions::max_slots(level)
        } else {
            max_slots(level)
        };
        if used >= slots {
            return Err(Refusal::SlotsFull(slots, used));
        }

        // `activity.job_time(job)` before any modifier: what the research and
        // copying costs are a fraction of.
        let raw_time = match activity {
            RESEARCH_MATERIAL | RESEARCH_TIME => {
                research_job_time(base_time, current_level, request.runs)
            }
            COPYING => base_time as f64 * licensed_runs as f64 * request.runs as f64,
            _ => base_time as f64 * request.runs as f64,
        };
        let seconds = match activity {
            MANUFACTURING => job_seconds(
                bp,
                request.runs,
                blueprint.time_efficiency,
                character_time,
                facility.time_multiplier,
            ),
            // `Job._get_time`: every input `TimeModifier` for this activity,
            // then `long(round(seconds))`. No activity but manufacturing has
            // a blueprint TE term.
            _ => (raw_time * character_time * facility.time_multiplier).round() as i64,
        };
        // `time_previous = time - time / float(runs); if time_previous >
        //      MAX_RUN_LENGTH: RUN_LENGTH`. The client measures all but the
        // last run, so a single run is never too long however slow it is --
        // which is how a battleship with no skills is installable at all.
        let all_but_last = seconds - seconds / request.runs.max(1);
        if all_but_last > MAX_RUN_LENGTH {
            return Err(Refusal::RunLength(all_but_last, MAX_RUN_LENGTH));
        }

        // `Job._get_materials`: manufacturing's with ME, invention's
        // datacores with none (`Invention.job_modifiers` appends no
        // `MaterialModifier`), and nothing for research or copying, whose
        // activities carry no materials.
        let materials = match (activity, invention) {
            (MANUFACTURING, _) => job_materials(
                bp,
                request.runs,
                blueprint.material_efficiency,
                facility.material_multiplier,
            ),
            // A reaction has no ME ([A 3] in `crate::reactions`): the base
            // quantities times the runs, and the facility's term only.
            (REACTION, _) => job_materials(bp, request.runs, 0, facility.material_multiplier),
            (_, Some(inv)) => inv
                .materials
                .iter()
                .map(|&(type_id, base)| Material {
                    type_id,
                    quantity: material_quantity(
                        base,
                        request.runs,
                        0,
                        facility.material_multiplier,
                    ),
                })
                // The decryptor is one more consumed material, quantity 1.
                .chain(decryptor.map(|d| Material {
                    type_id: d.type_id,
                    quantity: 1,
                }))
                .collect(),
            _ => Vec::new(),
        };
        let mut edits = consume_plan(&materials, on_hand, request)?;

        // A copy spends its runs: manufacturing and invention are the two
        // activities a copy can be installed for, and `INVALID_RUNS` above
        // already held `runs <= runsRemaining`.
        let blueprint_after = if relic {
            // T3-B: a relic spends `runs` units of its stack, and has no
            // ME / TE / runs row to update.
            edits.push(Edit::Consume {
                item_id: blueprint.item_id,
                quantity: request.runs,
            });
            None
        } else if blueprint.original() {
            None
        } else {
            let mut after = blueprint.state();
            after.runs_remaining = (after.runs_remaining - request.runs).max(0);
            if after.runs_remaining == 0 {
                edits.push(Edit::Consume {
                    item_id: blueprint.item_id,
                    quantity: 1,
                });
            }
            Some(after)
        };

        // Each activity's own `job_cost`, unrounded; `cost_lines` does the
        // rest. `prices` maps every science activity to manufacturing, so
        // [`estimated_item_value`] is every one of these prices.
        let eiv = estimated_item_value(table, bp);
        let raw_cost = match activity {
            // `Reaction.job_cost`: `prices[formula] * runs`, and a formula's
            // price is the EIV of its activity-9 materials, which is what
            // `bp.materials` holds for one.
            MANUFACTURING | REACTION => eiv * request.runs as f64,
            RESEARCH_MATERIAL | RESEARCH_TIME => {
                eiv * COST_PERCENTAGE * raw_time / base_time as f64
            }
            COPYING => eiv * COST_PERCENTAGE * licensed_runs as f64 * request.runs as f64,
            _ => {
                table
                    .find(product_type_id)
                    .map_or(0.0, |p| estimated_item_value(table, p))
                    * COST_PERCENTAGE
                    * request.runs as f64
            }
        };
        let cost = cost_lines(raw_cost, activity, facility, facility.cost_multiplier);
        // `elif (self.accounts[self.account] or 0) < self.total_cost:
        //      ACCOUNT_FUNDS`
        if pilot.balance < cost.total() as f64 {
            return Err(Refusal::AccountFunds(pilot.balance, cost.total()));
        }

        let probability = invention.map_or(1.0, |inv| {
            invention_probability_with(
                inv,
                level,
                decryptor.map_or(1.0, |d| d.probability_multiplier),
            )
        });

        let job = Job {
            job_id: self.next_id,
            activity_id: activity,
            blueprint_id: blueprint.item_id,
            blueprint_type_id: blueprint.type_id,
            blueprint_location_id: blueprint.location_id,
            blueprint_location_flag_id: blueprint.flag_id,
            blueprint_copy: !blueprint.original(),
            facility_id: facility.facility_id,
            owner_id: blueprint.owner_id,
            status: Status::Installed,
            installer_id: pilot.character_id,
            completed_character_id: None,
            solar_system_id: facility.solar_system_id,
            station_id: facility.facility_id,
            start_date: now,
            end_date: now + seconds * FILETIME_SECOND,
            pause_date: None,
            runs: request.runs,
            licensed_runs,
            // `JobBase._get_successful_runs` answers `self.runs` until the
            // job is complete, and rolls only then -- so the row says `runs`
            // until delivery, for invention too, and only invention's
            // number ever changes (see [`Jobs::plan_delivery`]).
            successful_runs: request.runs,
            cost: cost.total(),
            time_in_seconds: seconds,
            probability,
            product_type_id,
            optional_type_id: decryptor.map(|d| d.type_id),
            optional_type_id2: None,
            output_location_id: request.output_location_id,
            output_flag_id: request.output_flag_id,
        };

        Ok(Install {
            job,
            edits,
            cost,
            materials,
            blueprint_after,
        })
    }

    /// Record a job whose [`Install::edits`] have been applied.
    ///
    /// Separate from [`Jobs::plan_install`] so that the item plan can fail
    /// without leaving a job behind. Returns the id it was given.
    pub fn commit(&mut self, job: Job) -> i64 {
        let job_id = job.job_id;
        self.next_id = self.next_id.max(job_id + 1);
        match self.jobs.binary_search_by_key(&job_id, |j| j.job_id) {
            Ok(i) => self.jobs[i] = job,
            Err(i) => self.jobs.insert(i, job),
        }
        job_id
    }

    /// Record an installed job and the runs it spent off a copy.
    ///
    /// The caller has applied [`Install::edits`] -- including the
    /// `Edit::Consume` of a copy left with no runs -- so this only writes the
    /// two tables: the job, and the copy's new row, or no row for a copy that
    /// no longer exists.
    pub fn commit_install(&mut self, install: &Install) -> i64 {
        if let Some(after) = install.blueprint_after {
            if after.runs_remaining == 0 {
                self.blueprints.remove(after.item_id);
            } else {
                self.blueprints.set(after);
            }
        }
        self.commit(install.job)
    }

    /// Record a delivered job and what it did to blueprints.
    ///
    /// `changes` is what applying [`Delivery::edits`] returned: every
    /// `Created` in it is a copy this job minted, and gets
    /// [`Delivery::creates`]' ME, TE and runs. Nothing else in the plan
    /// creates an item, so there is no other `Created` to mistake for one.
    pub fn commit_delivery(&mut self, delivery: &Delivery, done: Job, changes: &[item::Change]) {
        if let Some(after) = delivery.blueprint_after {
            self.blueprints.set(after);
        }
        if let Some(template) = delivery.creates {
            for c in changes {
                if let item::Change::Created { item_id, .. } = c {
                    self.blueprints.set(BlueprintState {
                        item_id: *item_id,
                        ..template
                    });
                }
            }
        }
        self.commit(done);
    }

    /// Deliver a finished job: the plan that creates its output.
    ///
    /// `industryManager.CompleteJob(jobID, solarSystemID)`. The client calls
    /// it from the Jobs tab's deliver button and **discards the return
    /// value**, so the only thing that makes it visible is the
    /// `OnIndustryJob` push -- which is why the job's new status is in the
    /// returned `Job` for the caller to push rather than applied here.
    ///
    /// `Status::Delivered` and not `Completed`: `Completed` is what a job
    /// reaches on its own, `Delivered` is what it reaches when somebody
    /// collects it, and both are `>= STATUS_COMPLETED` so the client frees
    /// the blueprint either way.
    pub fn plan_delivery(
        &self,
        table: &Table,
        content: &crate::content::Content,
        job_id: i64,
        now: i64,
    ) -> Result<(Delivery, Job), Refusal> {
        let Some(job) = self.get(job_id) else {
            return Err(Refusal::MissingBlueprint);
        };
        if job.completed() {
            return Err(Refusal::IncompatibleActivity(job.activity_id));
        }
        if !job.ready_at(now) {
            return Err(Refusal::InvalidRuns(job.runs));
        }
        let mut done = *job;
        done.status = Status::Delivered;
        done.completed_character_id = Some(job.installer_id);
        // What the blueprint was when the job was installed. It cannot have
        // changed since: `blueprint.jobID` refuses a second job on it.
        let input = self.blueprints.state_of(job.blueprint_id);
        let (product, blueprint_after, creates) = match job.activity_id {
            // A reaction delivers `runs x product.quantity` of an item and
            // changes no blueprint (`Reaction.job_output_products`).
            MANUFACTURING | REACTION => (job.product(table), None, None),
            // `ResearchMaterial.job_output_products`: `blueprint.copy()` with
            // `materialEfficiency = min(ME + STEP * runs, MAX)`, and the same
            // for time. No item moves; the original is better.
            RESEARCH_MATERIAL | RESEARCH_TIME => {
                let mut after = input;
                if job.activity_id == RESEARCH_MATERIAL {
                    after.material_efficiency = (after.material_efficiency
                        + STEP_MATERIAL_EFFICIENCY * job.runs)
                        .min(MAX_MATERIAL_EFFICIENCY);
                } else {
                    after.time_efficiency = (after.time_efficiency
                        + STEP_TIME_EFFICIENCY * job.runs)
                        .min(MAX_TIME_EFFICIENCY);
                }
                let nothing = Material {
                    type_id: job.blueprint_type_id,
                    quantity: 0,
                };
                (nothing, Some(after), None)
            }
            // `Copying.job_output_products`: `runs` copies, each with
            // `runsRemaining = licensedRuns` and the original's ME and TE.
            COPYING => (
                Material {
                    type_id: job.blueprint_type_id,
                    quantity: job.runs,
                },
                None,
                Some(BlueprintState {
                    item_id: 0,
                    runs_remaining: job.licensed_runs,
                    material_efficiency: input.material_efficiency,
                    time_efficiency: input.time_efficiency,
                }),
            ),
            // `Invention.job_output_products`: `successfulRuns` copies of the
            // chosen blueprint, ME 2, TE 4, and
            // `runsRemaining = int(round(product.quantity + maxRuns))`, each
            // plus the installed decryptor's (`relic::copy_start`). The roll
            // happens here, at completion, exactly where
            // `_get_successful_runs` makes it.
            INVENTION => {
                let successes = successful_runs(job_seed(job), job.runs, job.probability);
                done.successful_runs = successes;
                let per_copy = table
                    .science(job.blueprint_type_id)
                    .and_then(|s| s.invention.as_ref())
                    .and_then(|inv| {
                        inv.products
                            .iter()
                            .find(|p| p.type_id == job.product_type_id)
                    })
                    .map_or(1, |p| p.quantity);
                // T3-C: the decryptor installed with the job, read back from
                // its type (the job persists only the type id).
                let decryptor = job.optional_type_id.and_then(|t| Decryptor::of(content, t));
                let start = relic::copy_start(per_copy, decryptor.as_ref());
                (
                    Material {
                        type_id: job.product_type_id,
                        quantity: successes,
                    },
                    None,
                    Some(BlueprintState {
                        item_id: 0,
                        runs_remaining: start.runs,
                        material_efficiency: start.material_efficiency,
                        time_efficiency: start.time_efficiency,
                    }),
                )
            }
            other => return Err(Refusal::IncompatibleActivity(other)),
        };
        if product.quantity > 0 && item::find_in(content, product.type_id).is_none() {
            return Err(Refusal::UnknownType(product.type_id));
        }
        let edits = if product.quantity > 0 {
            vec![Edit::Create {
                type_id: product.type_id,
                quantity: product.quantity,
                to: Slot::new(job.owner_id, job.output_location_id, job.output_flag_id),
            }]
        } else {
            Vec::new()
        };
        Ok((
            Delivery {
                job_id,
                edits,
                product,
                blueprint_after,
                creates,
            },
            done,
        ))
    }

    /// Cancel a live job.
    ///
    /// **No edits, and no refund.** `industryManager.CancelJob` destroys the
    /// installed materials in retail, and it is the honest behaviour here for
    /// a second reason: the materials were consumed by one atomic plan that
    /// either happened or did not, so there is nothing half-done to
    /// compensate for. EvEJS reaches the same place through a 1,054-line
    /// settlement saga that attempts a refund and gives up
    /// (`industryInstallSettlementState.js`).
    pub fn plan_cancel(&self, job_id: i64, now: i64) -> Result<Job, Refusal> {
        let Some(job) = self.get(job_id) else {
            return Err(Refusal::MissingBlueprint);
        };
        if job.completed() {
            return Err(Refusal::IncompatibleActivity(job.activity_id));
        }
        let mut done = *job;
        done.status = Status::Cancelled;
        done.completed_character_id = Some(job.installer_id);
        done.end_date = now;
        Ok(done)
    }
}

/// The furthest a character can install this activity, in jumps.
///
/// `Job._get_max_distance`:
/// `DISTANCE_PER_LEVEL.get(self.skills.get(DISTANCE_SKILL_FOR_ACTIVITY.get(activityID), 0))`.
pub fn max_distance<F>(activity_id: i64, level: F) -> i64
where
    F: Fn(i64) -> i64,
{
    let skill = DISTANCE_SKILL_FOR_ACTIVITY
        .iter()
        .find(|(a, _)| *a == activity_id)
        .map(|(_, s)| *s);
    let l = skill.map_or(0, level).clamp(0, 5) as usize;
    DISTANCE_PER_LEVEL[l]
}

/// Draw each material out of the stacks on hand.
///
/// One `Edit::Consume` per stack touched, in item-id order so the plan is
/// deterministic and a test can read it. `Consume` and not `Destroy` even
/// when the whole stack goes: `item::Store::apply` turns a full-stack
/// `Consume` into a `Destroy` itself, so saying `Consume` everywhere keeps
/// one rule here and lets the store own the normalisation.
fn consume_plan(
    materials: &[Material],
    on_hand: &[Stack],
    request: &InstallRequest,
) -> Result<Vec<Edit>, Refusal> {
    let mut edits = Vec::new();
    for m in materials {
        let mut here: Vec<&Stack> = on_hand
            .iter()
            .filter(|s| s.type_id == m.type_id)
            .filter(|s| s.slot.location_id == request.input_location_id)
            .filter(|s| s.slot.flag_id == request.input_flag_id)
            .collect();
        here.sort_by_key(|s| s.item_id);
        let available: i64 = here.iter().map(|s| s.quantity.max(0)).sum();
        if available < m.quantity {
            return Err(Refusal::MissingMaterial(m.type_id, m.quantity, available));
        }
        let mut want = m.quantity;
        for s in here {
            if want == 0 {
                break;
            }
            let take = want.min(s.quantity.max(0));
            if take <= 0 {
                continue;
            }
            edits.push(Edit::Consume {
                item_id: s.item_id,
                quantity: take,
            });
            want -= take;
        }
    }
    Ok(edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        /// The rows every test here reads: [`Table::fixture`], the invented
        /// table the service's tests read too. Leaked once per test thread so
        /// a lookup can hand out `&'static` rows.
        static ROWS: &'static Table = Box::leak(Box::new(Table::fixture()));
        /// The invented content tables (`Content::fixture`), leaked the
        /// same way: what the item-type checks read.
        static FX: &'static crate::content::Content =
            Box::leak(Box::new(crate::content::Content::fixture()));
    }

    fn fx() -> &'static crate::content::Content {
        FX.with(|c| *c)
    }

    fn t() -> &'static Table {
        ROWS.with(|r| *r)
    }

    fn find(blueprint_type_id: i64) -> Option<&'static Blueprint> {
        t().find(blueprint_type_id)
    }

    fn science(blueprint_type_id: i64) -> Option<&'static Science> {
        t().science(blueprint_type_id)
    }

    fn is_blueprint_type(type_id: i64) -> bool {
        t().is_blueprint_type(type_id)
    }

    const STATION: i64 = 60_012_526;
    const STATION_OWNER: i64 = 1_000_126;
    const STATION_TYPE: i64 = 2502;
    const SYSTEM: i64 = 30_000_001;
    const ME: i64 = 90_000_001;

    /// Our own station, which is the only facility this server has.
    fn station() -> Facility {
        Facility::npc_station(STATION, STATION_OWNER, STATION_TYPE, SYSTEM)
    }

    /// The fixture's hull blueprint, which every arithmetic test below is
    /// anchored on. Its invented numbers are small enough to check by hand:
    /// 600 s/run, 1,000 / 200 / 50 / 10 of four minerals priced 4 / 10 / 40 /
    /// 100, so an EIV of 9,000.
    fn rifter() -> &'static Blueprint {
        find(691).expect("691 is in the fixture")
    }

    fn pilot_with<'a>(skills: Option<&'a Skills>, implants: &'a [i64], balance: f64) -> Pilot<'a> {
        Pilot {
            character_id: ME,
            corporation_id: None,
            skills,
            implants,
            balance,
        }
    }

    /// A skill set built by id and level, bypassing `Skills::grant` --
    /// which refuses a type `crate::skill::SKILLS` does not have, and three
    /// of the four skills this module reads are not in it. See
    /// [`time_multiplier`].
    fn levels(pairs: &[(i64, i64)]) -> Skills {
        Skills::restore(
            pairs
                .iter()
                .map(|&(type_id, level)| crate::skill::SkillEntry {
                    type_id,
                    level,
                    points: 0,
                    rank: 1,
                })
                .collect(),
        )
    }

    fn item_at(item_id: i64, type_id: i64, quantity: i64, flag_id: i64) -> item::Item {
        let t = item::find_in(fx(), type_id).expect("the test uses types the item store knows");
        item::Item {
            item_id,
            type_id,
            owner_id: ME,
            location_id: STATION,
            flag_id,
            quantity: if t.singleton { 1 } else { quantity },
            group_id: t.group_id,
            category_id: t.category_id,
            singleton: t.singleton,
        }
    }

    /// A blueprint item in the station hangar, constructed rather than
    /// spawned: the pure tests need no item store.
    fn bpo(item_id: i64, type_id: i64) -> BlueprintItem {
        BlueprintItem {
            item_id,
            type_id,
            owner_id: ME,
            location_id: STATION,
            location_type_id: STATION_TYPE,
            flag_id: FLAG_HANGAR,
            runs_remaining: -1,
            material_efficiency: 0,
            time_efficiency: 0,
        }
    }

    fn request(blueprint_id: i64, runs: i64) -> InstallRequest {
        InstallRequest {
            activity_id: MANUFACTURING,
            blueprint_id,
            runs,
            input_location_id: STATION,
            input_flag_id: FLAG_HANGAR,
            output_location_id: STATION,
            output_flag_id: FLAG_HANGAR,
            licensed_runs: 0,
            product_type_id: None,
            decryptor: None,
        }
    }

    /// Enough of each of a blueprint's materials for `runs` runs, one stack
    /// each, with ascending item ids.
    fn minerals_for(bp: &Blueprint, runs: i64) -> Vec<Stack> {
        bp.materials
            .iter()
            .enumerate()
            .map(|(i, &(type_id, base))| Stack {
                item_id: 100_000 + i as i64,
                type_id,
                quantity: base * runs,
                slot: Slot::new(ME, STATION, FLAG_HANGAR),
            })
            .collect()
    }

    // -----------------------------------------------------------------
    // The static layer.
    // -----------------------------------------------------------------

    /// The fixture's one material no item table knows (invented).
    const UNKNOWN_MATERIAL: i64 = 9_990_001;

    /// Invented `econ/prices.tsv` lines, priced 2, 3 and 5.
    fn invented_prices() -> crate::econ_prices::Table {
        crate::econ_prices::Table::parse(&[
            "990010\t2.0\tbase\t1.0\t4\t99001\t\t1",
            "990011\t3.0\tbase\t1.0\t4\t99001\t\t1",
            "990012\t5.0\trecipe\t1.0\t6\t99002\t1\t1",
        ])
        .expect("invented prices parse")
    }

    /// Every lookup in this module binary-searches, so an unsorted table is a
    /// silently wrong answer rather than a failure: the parse sorts whatever
    /// order the file is in, and sorts each material and skill list too.
    #[test]
    fn the_parsed_table_is_sorted_so_the_lookups_are_valid() {
        let table = Table::parse(
            &[
                "990200\t1\tactivity\t0\t0\t\t60\t5",
                "990200\t1\tmaterial\t990011\t3\t\t60\t5",
                "990200\t1\tmaterial\t990010\t7\t\t60\t5",
                "990200\t1\tproduct\t990012\t1\t\t60\t5",
                "990200\t1\tskill\t990031\t2\t\t60\t5",
                "990200\t1\tskill\t990030\t1\t\t60\t5",
                "990100\t1\tactivity\t0\t0\t\t30\t5",
                "990100\t1\tproduct\t990011\t1\t\t30\t5",
            ],
            &invented_prices(),
        )
        .expect("well-formed rows");
        let ids = |v: Vec<i64>| v.windows(2).all(|w| w[0] < w[1]);
        assert!(ids(table
            .blueprints
            .iter()
            .map(|b| b.blueprint_type_id)
            .collect()));
        assert!(ids(table
            .science
            .iter()
            .map(|s| s.blueprint_type_id)
            .collect()));
        for bp in &table.blueprints {
            assert!(ids(bp.materials.iter().map(|m| m.0).collect()));
            assert!(ids(bp.skills.iter().map(|m| m.0).collect()));
        }
        assert_eq!(table.find(990200).map(|b| b.product_type_id), Some(990012));
        assert_eq!(table.find(990100).map(|b| b.product_type_id), Some(990011));
        assert!(
            t().blueprints
                .windows(2)
                .all(|w| w[0].blueprint_type_id < w[1].blueprint_type_id)
        );
    }

    /// A blueprint is kept when its manufacturing activity has exactly one
    /// product; one without is not installable, and a reaction-only formula
    /// is kept apart, in `reactions`. Each kept blueprint has exactly one science row, and an
    /// activity the file does not list is `None`, which is
    /// `INCOMPATIBLE_ACTIVITY`.
    #[test]
    fn a_blueprint_is_kept_when_manufacturing_makes_one_product() {
        let table = Table::parse(
            &[
                // No product.
                "990300\t1\tactivity\t0\t0\t\t60\t5",
                "990300\t1\tmaterial\t990010\t1\t\t60\t5",
                // A reaction only.
                "990301\t9\tactivity\t0\t0\t\t60\t5",
                "990301\t9\tproduct\t990011\t1\t\t60\t5",
                // Kept, with copying and an invention whose second product
                // has no chance in the file.
                "990302\t1\tactivity\t0\t0\t\t60\t5",
                "990302\t1\tproduct\t990012\t2\t\t60\t5",
                "990302\t5\tactivity\t0\t0\t\t40\t5",
                "990302\t8\tactivity\t0\t0\t\t90\t5",
                "990302\t8\tproduct\t990303\t1\t0.5\t90\t5",
                "990302\t8\tproduct\t990304\t1\t\t90\t5",
            ],
            &invented_prices(),
        )
        .expect("well-formed rows");
        assert_eq!(
            table
                .blueprints
                .iter()
                .map(|b| b.blueprint_type_id)
                .collect::<Vec<_>>(),
            vec![990302]
        );
        assert_eq!(
            table
                .reactions
                .iter()
                .map(|b| b.blueprint_type_id)
                .collect::<Vec<_>>(),
            vec![990301],
            "the reaction-only row is a formula"
        );
        assert!(table.find(990301).is_none());
        assert!(table.find_reaction(990301).is_some());
        let bp = table.find(990302).expect("kept");
        assert_eq!(
            (bp.product_quantity, bp.time, bp.max_production_limit),
            (2, 60, 5)
        );
        let sci = table
            .science(990302)
            .expect("one science row per blueprint");
        assert_eq!(table.science.len(), table.blueprints.len());
        assert_eq!(sci.copying_time, Some(40));
        assert_eq!(sci.research_material_time, None);
        assert_eq!(activity_time(&table, bp, RESEARCH_MATERIAL), None);
        assert_eq!(activity_time(&table, bp, COPYING), Some(40));
        let inv = sci.invention.as_ref().expect("an invention with products");
        // `product.probability or 1.0`: the mean of 0.5 and 1.0.
        assert_eq!(invention_probability(inv, |_| 0), 0.75);
    }

    /// A malformed row or an unknown `kind` refuses the whole table, naming
    /// the data line, rather than loading half of it.
    #[test]
    fn a_bad_row_is_refused_naming_its_line() {
        let prices = invented_prices();
        for (lines, line) in [
            (
                vec![
                    "990400\t1\tactivity\t0\t0\t\t60\t5",
                    "990400\t1\tbyproduct\t990010\t1\t\t60\t5",
                ],
                2,
            ),
            (vec!["990400\t1\tactivity\t0\t0\t\t60"], 1),
            (vec!["990400\t1\tactivity\t0\tx\t\t60\t5"], 1),
            (vec!["990400\t8\tproduct\t990010\t1\tlikely\t60\t5"], 1),
        ] {
            let err = Table::parse(&lines, &prices).expect_err("a bad row");
            assert!(err.starts_with(&format!("data line {line}:")), "{err}");
        }
    }

    /// A job whose material no item table knows cannot be built, and the
    /// refusal names that type rather than failing in `apply`.
    #[test]
    fn a_blueprint_with_an_unknown_material_is_not_buildable() {
        let bp = find(693).expect("693 is in the fixture");
        assert!(!buildable(fx(), bp));
        assert_eq!(missing_type(fx(), bp), Some(UNKNOWN_MATERIAL));
        assert!(buildable(fx(), rifter()));
        assert_eq!(missing_type(fx(), rifter()), None);
    }

    /// **The plan row's test** (CP33b): the EIV of a blueprint is the
    /// client's `industryCommon.GetBlueprintPrice` over `P_ref`, the price
    /// `econ_prices` serves as `adjustedPrice` -- `sum(P_ref(m) * m.quantity)`
    /// over the manufacturing materials, with an unpriced material counting
    /// 0, as the client's own fallback does. **[V]** on the formula.
    #[test]
    fn the_eiv_of_a_blueprint_is_the_clients_formula_over_p_ref() {
        let econ = invented_prices();
        let table = Table::parse(
            &[
                "990500\t1\tactivity\t0\t0\t\t60\t5",
                "990500\t1\tmaterial\t990010\t7\t\t60\t5",
                "990500\t1\tmaterial\t990011\t11\t\t60\t5",
                "990500\t1\tmaterial\t990099\t13\t\t60\t5",
                "990500\t1\tproduct\t990012\t1\t\t60\t5",
            ],
            &econ,
        )
        .expect("well-formed rows");
        let bp = table.find(990500).expect("kept");
        let p_ref = |t: i64| econ.prices.get(&t).map_or(0.0, |p| p.ref_price);
        let client: f64 = bp.materials.iter().map(|&(t, q)| p_ref(t) * q as f64).sum();
        assert_eq!(estimated_item_value(&table, bp), client);
        assert_eq!(client, 7.0 * 2.0 + 11.0 * 3.0);
        assert!(!table.is_priced(990099));
        assert_eq!(table.adjusted_price(990099), 0.0);
        // Every `P_ref` row is carried, products included.
        assert_eq!(table.adjusted_price(990012), 5.0);
    }

    /// Hand-computed from the fixture's invented prices: 1,000 × 4 + 200 × 10
    /// + 50 × 40 + 10 × 100.
    #[test]
    fn the_rifters_estimated_item_value_is_the_sum_of_its_minerals() {
        let want = 1_000.0 * 4.0 + 200.0 * 10.0 + 50.0 * 40.0 + 10.0 * 100.0;
        assert_eq!(want, 9_000.0);
        assert_eq!(estimated_item_value(t(), rifter()), want);
    }

    /// EIV does not move with ME: the client prices the blueprint's *base*
    /// materials, which is why research makes a job cheaper in minerals and
    /// not in ISK.
    #[test]
    fn the_estimated_item_value_ignores_material_efficiency() {
        let bp = rifter();
        let facility = station();
        let a = job_cost(t(), bp, 1, MANUFACTURING, &facility, 1.0);
        // Same call; ME is not even an argument, which is the assertion.
        let b = job_cost(t(), bp, 1, MANUFACTURING, &facility, 1.0);
        assert_eq!(a, b);
        assert_eq!(a.base, 9_000);
    }

    // -----------------------------------------------------------------
    // Status, and the gap in the middle of it.
    // -----------------------------------------------------------------

    /// `docs/port-tracker.md` §14.1: the set is closed and **4..99 are
    /// unused**, and a value in the gap passes every client guard as "live"
    /// while naming no status.
    #[test]
    fn the_status_set_is_closed_and_the_gap_is_refused() {
        let all = [
            Status::Unsubmitted,
            Status::Installed,
            Status::Paused,
            Status::Ready,
            Status::Completed,
            Status::Delivered,
            Status::Cancelled,
            Status::Reverted,
        ];
        assert_eq!(all.len(), 8);
        for s in all {
            assert_eq!(Status::from_code(s.code()), Some(s));
        }
        for code in 4..100 {
            assert_eq!(
                Status::from_code(code),
                None,
                "{code} is in the unused gap and must not resolve"
            );
        }
        for code in [-1, 104, 1000] {
            assert_eq!(Status::from_code(code), None);
        }
    }

    /// `JobBase.completed` is `status >= STATUS_COMPLETED`, a threshold and
    /// not an equality -- which is the whole of §14.1's second correction.
    #[test]
    fn completed_is_a_threshold_at_one_hundred() {
        assert_eq!(STATUS_COMPLETED, 100);
        for s in [
            Status::Unsubmitted,
            Status::Installed,
            Status::Paused,
            Status::Ready,
        ] {
            assert!(!s.completed(), "{} is live", s.name());
            assert!(s.code() < STATUS_COMPLETED);
        }
        for s in [
            Status::Completed,
            Status::Delivered,
            Status::Cancelled,
            Status::Reverted,
        ] {
            assert!(s.completed(), "{} is done", s.name());
            assert!(s.code() >= STATUS_COMPLETED);
        }
    }

    /// `industryCommon.JobStatus`: `Ready` is derived from the clock, in two
    /// different ways, and stored nowhere.
    #[test]
    fn ready_is_derived_from_the_end_date_and_from_the_pause_date() {
        let mut job = installed_job();
        job.end_date = 1_000 * FILETIME_SECOND;
        assert_eq!(job.status_at(999 * FILETIME_SECOND), Status::Installed);
        assert_eq!(job.status_at(1_000 * FILETIME_SECOND), Status::Installed);
        assert_eq!(job.status_at(1_001 * FILETIME_SECOND), Status::Ready);

        // The paused branch does not look at `now` at all.
        job.status = Status::Paused;
        job.pause_date = Some(999 * FILETIME_SECOND);
        assert_eq!(job.status_at(0), Status::Paused);
        job.pause_date = Some(1_001 * FILETIME_SECOND);
        assert_eq!(job.status_at(0), Status::Ready);

        // And a finished job is never re-derived.
        job.status = Status::Delivered;
        assert_eq!(job.status_at(i64::MAX), Status::Delivered);
    }

    fn installed_job() -> Job {
        Job {
            job_id: MIN_JOB_ID,
            activity_id: MANUFACTURING,
            blueprint_id: 500,
            blueprint_type_id: 691,
            blueprint_location_id: STATION,
            blueprint_location_flag_id: FLAG_HANGAR,
            blueprint_copy: false,
            facility_id: STATION,
            owner_id: ME,
            status: Status::Installed,
            installer_id: ME,
            completed_character_id: None,
            solar_system_id: SYSTEM,
            station_id: STATION,
            start_date: 0,
            end_date: 6_000 * FILETIME_SECOND,
            pause_date: None,
            runs: 1,
            licensed_runs: 1,
            successful_runs: 1,
            cost: 0,
            time_in_seconds: 6_000,
            probability: 1.0,
            product_type_id: 587,
            optional_type_id: None,
            optional_type_id2: None,
            output_location_id: STATION,
            output_flag_id: FLAG_HANGAR,
        }
    }

    /// `industry.ACTIVITIES` is `(1, 4, 3, 5, 8, 9)`. A previous pass through
    /// this lane recorded the ids in ascending order; the set is the same and
    /// the tuple is not.
    #[test]
    fn the_activity_tuple_is_in_the_clients_order_not_ascending() {
        assert_eq!(ACTIVITIES, [1, 4, 3, 5, 8, 9]);
        assert_ne!(ACTIVITIES, [1, 3, 4, 5, 8, 9]);
        for a in ACTIVITIES {
            assert!(activity_name(a).is_some(), "{a} must have a name");
        }
        assert_eq!(activity_name(MANUFACTURING), Some("manufacturing"));
        assert_eq!(activity_name(2), None);
        assert!(!is_science(MANUFACTURING));
        assert!(!is_science(REACTION));
        assert_eq!(SCIENCE_ACTIVITIES.len(), 4);
        for a in SCIENCE_ACTIVITIES {
            assert!(is_science(a));
        }
    }

    // -----------------------------------------------------------------
    // The maths.
    // -----------------------------------------------------------------

    /// The whole reason `round(quantity, 2)` sits between the multiply and
    /// the ceiling: 140 units a run, 15 runs at ME 8, is `140 * 0.92 * 15`,
    /// which IEEE 754 computes as `1932.0000000000002`. A bare ceiling bills
    /// **1,933** -- one extra unit on every such job, for ever, with nothing
    /// to notice it.
    #[test]
    fn the_two_dp_round_before_the_ceiling_is_load_bearing() {
        let raw = 140.0f64 * 0.92 * 15.0;
        assert_ne!(raw, 1932.0, "the float really is above the integer");
        assert_eq!(raw.ceil() as i64, 1933, "a bare ceiling over-bills");
        assert_eq!(round_to(raw, MATERIAL_ROUND_PRECISION), 1932.0);
        assert_eq!(material_quantity(140, 15, 8, 1.0), 1932);
    }

    /// `base * modifiers * runs`, rounded **once** over the batch -- not per
    /// run. 100 units at ME 10 over 10 runs is 900, not 10 * 90.
    #[test]
    fn material_efficiency_applies_to_the_batch_and_not_to_each_run() {
        assert_eq!(material_quantity(100, 10, 10, 1.0), 900);
        // A case where per-run rounding would differ: 3 units at ME 10 is
        // 2.7/run. Per run that is ceil(2.7) * 4 = 12; over the batch it is
        // ceil(10.8) = 11.
        assert_eq!(material_quantity(3, 4, 10, 1.0), 11);
        assert_ne!(material_quantity(3, 4, 10, 1.0), 12);
    }

    /// `max(ceil(...), self.runs)`: a material can never fall below one unit
    /// per run however good the research is.
    #[test]
    fn a_material_never_falls_below_one_unit_per_run() {
        // 1 unit at the best possible ME is still 1 per run.
        assert_eq!(material_quantity(1, 10, 10, 1.0), 10);
        // And at the worst ME it is 11 per run, so the floor is not a cap.
        assert_eq!(material_quantity(1, 10, -10, 1.0), 11);
    }

    /// ME 0 appends **no** modifier rather than a 1.0 one, which is the
    /// client's truthiness guard. Invisible for materials and visible for
    /// [`max_runs`], so it is pinned where it can be seen.
    #[test]
    fn zero_efficiency_is_no_modifier_at_all() {
        let bp = rifter();
        assert_eq!(material_quantity(1_000, 1, 0, 1.0), 1_000);
        assert_eq!(job_seconds(bp, 1, 0, 1.0, 1.0), 600);
        // ME runs from -80 to +10 and TE from -120 to +20, in both
        // directions, and nothing in the range panics or goes negative.
        for me in MIN_MATERIAL_EFFICIENCY..=MAX_MATERIAL_EFFICIENCY {
            assert!(material_quantity(1_000, 1, me, 1.0) >= 1);
        }
        let mut te = MIN_TIME_EFFICIENCY;
        while te <= MAX_TIME_EFFICIENCY {
            assert!(job_seconds(bp, 1, te, 1.0, 1.0) > 0);
            te += STEP_TIME_EFFICIENCY;
        }
    }

    /// Every material of a whole job, and the fixture hull's four by hand.
    #[test]
    fn a_whole_jobs_materials_are_the_blueprints_times_the_runs() {
        let bp = rifter();
        let m = job_materials(bp, 3, 0, 1.0);
        assert_eq!(
            m,
            vec![
                Material {
                    type_id: 34,
                    quantity: 3_000
                },
                Material {
                    type_id: 35,
                    quantity: 600
                },
                Material {
                    type_id: 36,
                    quantity: 150
                },
                Material {
                    type_id: 37,
                    quantity: 30
                },
            ]
        );
    }

    /// `1.0 * (1 - 0.04*L(3380)) * (1 - 0.03*L(3388))`, times the implants.
    #[test]
    fn the_time_multiplier_folds_industry_advanced_industry_and_implants() {
        let none = levels(&[]);
        assert_eq!(time_multiplier(|t| level_of(Some(&none), t), &[]), 1.0);

        let industry5 = levels(&[(SKILL_INDUSTRY, 5)]);
        let m = time_multiplier(|t| level_of(Some(&industry5), t), &[]);
        assert!((m - 0.80).abs() < 1e-12, "Industry 5 is -20%, got {m}");

        let both = levels(&[(SKILL_INDUSTRY, 5), (SKILL_ADVANCED_INDUSTRY, 5)]);
        let m = time_multiplier(|t| level_of(Some(&both), t), &[]);
        assert!((m - 0.80 * 0.85).abs() < 1e-12, "and -15% more, got {m}");

        // The best implant is -8%, and the six ids are the only ones that do
        // anything at all.
        let m = time_multiplier(|t| level_of(Some(&both), t), &[59801]);
        assert!((m - 0.80 * 0.85 * 0.92).abs() < 1e-12);
        assert_eq!(implant_time_bonus(27170), -1.0);
        assert_eq!(implant_time_bonus(27167), -2.0);
        assert_eq!(implant_time_bonus(27171), -4.0);
        for t in [59797, 59799, 59801] {
            assert_eq!(implant_time_bonus(t), -8.0);
        }
        // Not an industry implant, and not the reprocessing ones either.
        for t in [3385, 3389, 12196, 587] {
            assert_eq!(implant_time_bonus(t), 0.0);
        }
    }

    /// The `REQUIRED_SKILL_MODIFIERS` path reads attribute 1982 of each
    /// skill the blueprint requires, and nothing else: a blueprint that needs
    /// only Industry gets no bonus from the T2 production skills.
    #[test]
    fn the_required_skill_time_path_reads_only_the_required_skills() {
        let maxed = levels(&[(SKILL_INDUSTRY, 5), (11_448, 5), (11_529, 5), (81_896, 5)]);
        let level = |t: i64| level_of(Some(&maxed), t);
        let bp = rifter();
        assert_eq!(bp.skills, vec![(SKILL_INDUSTRY, 1)]);
        assert_eq!(required_skill_time_multiplier(bp, level), 1.0);
        // 11448 is a T2 production skill at -1%/level.
        assert_eq!(required_skill_time_per_level(11_448), -1.0);
        assert_eq!(required_skill_time_per_level(81_896), -2.0);
        assert_eq!(required_skill_time_per_level(SKILL_INDUSTRY), 0.0);
        let fake = Blueprint {
            skills: vec![(11_448, 1)],
            ..rifter().clone()
        };
        assert!((required_skill_time_multiplier(&fake, level) - 0.95).abs() < 1e-12);
    }

    /// `activity.time * runs`, then the modifiers, then one `round`.
    #[test]
    fn a_jobs_time_is_the_base_times_the_runs_times_the_multipliers() {
        let bp = rifter();
        assert_eq!(job_seconds(bp, 1, 0, 1.0, 1.0), 600);
        assert_eq!(job_seconds(bp, 10, 0, 1.0, 1.0), 6_000);
        // Industry 5.
        assert_eq!(job_seconds(bp, 10, 0, 0.80, 1.0), 4_800);
        // TE 20 on top: 6_000 * 0.8 * 0.8.
        assert_eq!(job_seconds(bp, 10, 20, 0.80, 1.0), 3_840);
        // The rounding is to the nearest second, not a truncation: a
        // multiplier that lands on .5 goes up.
        let odd = Blueprint {
            time: 1,
            ..bp.clone()
        };
        assert_eq!(job_seconds(&odd, 1, 0, 0.5, 1.0), 1);
    }

    /// 30 days divided by the run length, which for the fixture hull's 600 s
    /// is exactly 4,320 -- and 5,400 once Industry 5 shortens each run.
    #[test]
    fn max_runs_is_thirty_days_worth_for_an_original() {
        let bp = rifter();
        assert_eq!(MAX_RUN_LENGTH, 2_592_000);
        assert_eq!(max_runs(bp, -1, 0, 1.0, 1.0), 4_320);
        assert_eq!(max_runs(bp, -1, 0, 0.80, 1.0), 5_400);
        // A copy is capped by what it has left instead, whatever the clock
        // says. `Manufacturing.job_max_runs`.
        assert_eq!(max_runs(bp, 5, 0, 1.0, 1.0), 5);
        assert_eq!(max_runs(bp, 0, 0, 1.0, 1.0), 0);
        // The hard cap bites for something very quick: 30 days of one-second
        // runs is 2,592,000, which `Activity.MAX_RUNS` clamps to a million.
        let quick = Blueprint {
            time: 1,
            ..bp.clone()
        };
        assert_eq!(max_runs(&quick, -1, 0, 1.0, 1.0), MAX_RUNS_HARD_CAP);
        assert_eq!(
            max_runs(
                &Blueprint {
                    time: 0,
                    ..bp.clone()
                },
                -1,
                0,
                1.0,
                1.0
            ),
            MAX_RUNS_HARD_CAP
        );
    }

    /// A fresh character has exactly one manufacturing slot, and that number
    /// comes from the character *type* rather than the attribute's
    /// `defaultValue` of 0 -- which would have made every install fail.
    #[test]
    fn a_character_with_no_skills_has_one_manufacturing_slot() {
        let none = levels(&[]);
        assert_eq!(max_slots(|t| level_of(Some(&none), t)), 1);
        assert_eq!(max_slots(|_| 0), 1);
        let mass = levels(&[(SKILL_MASS_PRODUCTION, 5)]);
        assert_eq!(max_slots(|t| level_of(Some(&mass), t)), 6);
        let both = levels(&[
            (SKILL_MASS_PRODUCTION, 5),
            (SKILL_ADVANCED_MASS_PRODUCTION, 5),
        ]);
        assert_eq!(
            max_slots(|t| level_of(Some(&both), t)),
            MAX_MANUFACTURE_SLOTS
        );
        assert_eq!(max_slots(|_| 99), MAX_MANUFACTURE_SLOTS, "and it is capped");
    }

    /// Level 0 of Supply Chain Management is **0 jumps**, so a fresh
    /// character can only use the facility they are standing in.
    #[test]
    fn remote_install_reach_starts_at_zero_jumps() {
        assert_eq!(max_distance(MANUFACTURING, |_| 0), 0);
        assert_eq!(max_distance(MANUFACTURING, |_| 1), 5);
        assert_eq!(max_distance(MANUFACTURING, |_| 5), MAX_INDUSTRY_DISTANCE);
        // The science activities read a different skill, and reactions a
        // third. Pinned because a wrong mapping is invisible until someone
        // trains one of them.
        let scientific = levels(&[(SKILL_SCIENTIFIC_NETWORKING, 3)]);
        let level = |t: i64| level_of(Some(&scientific), t);
        assert_eq!(max_distance(COPYING, level), 15);
        assert_eq!(max_distance(MANUFACTURING, level), 0);
        assert_eq!(max_distance(REACTION, level), 0);
        // An id that is not an activity reaches nothing.
        assert_eq!(max_distance(2, |_| 5), 0);
    }

    /// Each cost line is rounded on its own and `total` leaves `base` out --
    /// adding it would charge the job twice.
    #[test]
    fn the_cost_lines_are_rounded_separately_and_the_total_excludes_the_base() {
        let bp = rifter();
        let facility = station();
        let c = job_cost(t(), bp, 1, MANUFACTURING, &facility, 1.0);
        assert_eq!(c.base, 9_000);
        assert_eq!(c.cost, 9_000);
        // An NPC station takes nothing, and the only charge is the SCC's 4%.
        assert_eq!(c.tax, 0);
        assert_eq!(c.scc_surcharge, 360);
        assert_eq!(c.security_tax, 0);
        assert_eq!(c.total(), 9_360);
        assert_ne!(c.total(), c.base + c.cost + c.scc_surcharge);

        // A facility that does take a cut.
        let taxed = Facility {
            tax: Some(0.10),
            ..facility
        };
        let c = job_cost(t(), bp, 1, MANUFACTURING, &taxed, 1.0);
        assert_eq!(c.tax, 900);
        assert_eq!(c.total(), 9_000 + 900 + 360);
    }

    /// The two research activities pay half the SCC surcharge; copying and
    /// invention pay all of it. Round 2's formula, pinned now because it is
    /// one line and would otherwise be re-derived.
    #[test]
    fn only_the_two_research_activities_discount_the_scc_surcharge() {
        let bp = rifter();
        let f = station();
        let full = job_cost(t(), bp, 1, MANUFACTURING, &f, 1.0).scc_surcharge;
        assert_eq!(
            job_cost(t(), bp, 1, RESEARCH_TIME, &f, 1.0).scc_surcharge,
            full / 2
        );
        assert_eq!(
            job_cost(t(), bp, 1, RESEARCH_MATERIAL, &f, 1.0).scc_surcharge,
            full / 2
        );
        assert_eq!(job_cost(t(), bp, 1, COPYING, &f, 1.0).scc_surcharge, full);
        assert_eq!(job_cost(t(), bp, 1, INVENTION, &f, 1.0).scc_surcharge, full);
        assert_eq!(job_cost(t(), bp, 1, REACTION, &f, 1.0).scc_surcharge, full);
        // And a negative modifier cannot turn the surcharge into a rebate:
        // `scc_rate = max(0, scc_rate)`.
        let odd = Facility {
            scc_tax_modifier: -5.0,
            ..f
        };
        assert_eq!(
            job_cost(t(), bp, 1, MANUFACTURING, &odd, 1.0).scc_surcharge,
            0
        );
    }

    /// The safe-sec industry tax is 5% of base and applies nowhere we have.
    #[test]
    fn the_security_tax_is_charged_only_in_a_safe_sec_system() {
        let bp = rifter();
        assert!(
            !station().safe_sec,
            "our system is not safe-sec; see Facility::safe_sec"
        );
        assert_eq!(
            job_cost(t(), bp, 1, MANUFACTURING, &station(), 1.0).security_tax,
            0
        );
        let safe = Facility {
            safe_sec: true,
            ..station()
        };
        assert_eq!(
            job_cost(t(), bp, 1, MANUFACTURING, &safe, 1.0).security_tax,
            450
        );
    }

    /// The accounting entry type for each activity, so the ledger wiring is
    /// a lookup and not a decision.
    #[test]
    fn every_activity_has_a_wallet_journal_ref_type() {
        for a in ACTIVITIES {
            assert!(Cost::ref_type(a).is_some(), "{a} has no refType");
        }
        assert_eq!(Cost::ref_type(MANUFACTURING), Some(56));
        assert_eq!(Cost::ref_type(REACTION), Some(135));
        assert_eq!(Cost::ref_type(2), None);
    }

    // -----------------------------------------------------------------
    // Blueprints as items.
    // -----------------------------------------------------------------

    /// `data.quantity`'s **sign** is what tells the client singleton and
    /// original: `-1` is an original, `-2` is a copy, positive is a stack.
    #[test]
    fn the_blueprint_rows_quantity_is_negative_and_the_sign_is_the_message() {
        let original = bpo(500, 691);
        assert!(original.original());
        assert_eq!(original.wire_quantity(), -1);
        let copy = BlueprintItem {
            runs_remaining: 10,
            ..original
        };
        assert!(!copy.original());
        assert_eq!(copy.wire_quantity(), -2);
        // And the client's own derivation over our number.
        for bi in [original, copy] {
            let q = bi.wire_quantity();
            assert!(q < 0, "singleton = data.quantity < 0");
            assert_eq!(q != -2, bi.original(), "original = data.quantity != -2");
            assert_eq!(q.max(1), 1, "quantity = max(data.quantity, 1)");
        }
    }

    /// Only a blueprint type of the [`Table`] becomes a `BlueprintItem`, so a
    /// hull in the hangar is not mistaken for its own blueprint.
    #[test]
    fn only_a_known_blueprint_type_is_a_blueprint_item() {
        let hull = item_at(100_001, 587, 1, FLAG_HANGAR);
        assert!(BlueprintItem::original_of(t(), &hull).is_none());
        assert!(!is_blueprint_type(587));
        assert!(is_blueprint_type(691));
        assert_eq!(
            bpo(500, 691).blueprint(t()).map(|b| b.product_type_id),
            Some(587)
        );

        // The fixture's blueprint ids are `item::SPAWNABLE` rows, so a
        // blueprint type here is an item there, and that item is category 9.
        let row = item::find_in(fx(), 691).expect("a blueprint type must be a spawnable item");
        assert_eq!(row.category_id, 9, "categoryBlueprint");
        let hull_row = item::find_in(fx(), 587).expect("the product is spawnable too");
        assert_ne!(hull_row.category_id, 9, "a hull is not a blueprint");
    }

    // -----------------------------------------------------------------
    // The operation.
    // -----------------------------------------------------------------

    /// Lane clock: at scale N the job ends N times sooner; its nominal
    /// `timeInSeconds` and every other field are the scale-1 install's.
    #[test]
    fn a_paced_job_ends_n_times_sooner() {
        let bp = rifter();
        let jobs = Jobs::new();
        let skills = levels(&[(SKILL_INDUSTRY, 1)]);
        let pilot = pilot_with(Some(&skills), &[], 1_000_000.0);
        let on_hand = minerals_for(bp, 1);
        let start = 7_000 * FILETIME_SECOND;
        let install = || {
            jobs.plan_install(
                t(),
                fx(),
                start,
                &pilot,
                &station(),
                &bpo(500, 691),
                &request(500, 1),
                &on_hand,
            )
            .expect("installs")
        };
        let real = install().paced(crate::game_clock::GameClock::REAL);
        assert_eq!(real.job, install().job, "scale 1 changes nothing");
        let fast = install().paced(crate::game_clock::GameClock::new(4).unwrap());
        let len = |j: &Job| j.end_date - j.start_date;
        assert_eq!(len(&fast.job) * 4, len(&real.job));
        assert_eq!(fast.job.time_in_seconds, real.job.time_in_seconds);
        assert_eq!(fast.job.start_date, start);
    }

    #[test]
    fn a_rifter_job_installs_consumes_its_minerals_and_delivers_a_hull() {
        let bp = rifter();
        let jobs = Jobs::new();
        let skills = levels(&[(SKILL_INDUSTRY, 1)]);
        let pilot = pilot_with(Some(&skills), &[], 1_000_000.0);
        let on_hand = minerals_for(bp, 1);
        let install = jobs
            .plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &bpo(500, 691),
                &request(500, 1),
                &on_hand,
            )
            .expect("a fresh character with the minerals can build a Rifter");

        assert_eq!(install.job.product_type_id, 587);
        assert_eq!(install.job.status, Status::Installed);
        assert_eq!(install.job.runs, 1);
        assert_eq!(install.job.successful_runs, 1);
        // Industry 1 is -4%: 600 * 0.96 = 576.
        assert_eq!(install.job.time_in_seconds, 576);
        assert_eq!(install.job.end_date, 576 * FILETIME_SECOND);
        assert_eq!(install.cost.base, 9_000);
        assert_eq!(install.job.cost, install.cost.total());
        assert_eq!(install.materials.len(), 4);

        // Four edits, one per stack, in item-id order and for the exact
        // quantity the blueprint asks.
        assert_eq!(
            install.edits,
            vec![
                Edit::Consume {
                    item_id: 100_000,
                    quantity: 1_000
                },
                Edit::Consume {
                    item_id: 100_001,
                    quantity: 200
                },
                Edit::Consume {
                    item_id: 100_002,
                    quantity: 50
                },
                Edit::Consume {
                    item_id: 100_003,
                    quantity: 10
                },
            ]
        );

        // And the whole thing runs against a real store.
        let mut store = item::Store::new();
        for (i, s) in on_hand.iter().enumerate() {
            store.insert(item_at(s.item_id, s.type_id, s.quantity, FLAG_HANGAR));
            assert_eq!(store.items()[i].item_id, s.item_id);
        }
        store
            .apply_in(fx(), &install.edits)
            .expect("the plan applies");
        assert!(store.items().is_empty(), "every stack was exactly consumed");

        let mut jobs = jobs;
        let job_id = jobs.commit(install.job);
        assert_eq!(jobs.used_slots(ME, MANUFACTURING), 1);
        assert_eq!(jobs.job_for_blueprint(500).map(|j| j.job_id), Some(job_id));

        // Not ready yet.
        assert!(jobs.plan_delivery(t(), fx(), job_id, 0).is_err());
        let now = install.job.end_date + FILETIME_SECOND;
        let (delivery, done) = jobs
            .plan_delivery(t(), fx(), job_id, now)
            .expect("ready now");
        assert_eq!(
            delivery.product,
            Material {
                type_id: 587,
                quantity: 1
            }
        );
        assert_eq!(
            delivery.edits,
            vec![Edit::Create {
                type_id: 587,
                quantity: 1,
                to: Slot::new(ME, STATION, FLAG_HANGAR),
            }]
        );
        store
            .apply_in(fx(), &delivery.edits)
            .expect("the hull is created");
        assert_eq!(store.items().len(), 1);
        assert_eq!(store.items()[0].type_id, 587);
        assert!(store.items()[0].singleton, "a ship is one object");

        jobs.commit(done);
        assert_eq!(jobs.get(job_id).map(|j| j.status), Some(Status::Delivered));
        assert!(
            jobs.job_for_blueprint(500).is_none(),
            "the blueprint is free again"
        );
        assert_eq!(jobs.used_slots(ME, MANUFACTURING), 0);
    }

    /// A ten-run job costs ten times the materials and ten times the ISK,
    /// and `Edit::Create` of ten singletons becomes ten separate hulls --
    /// which is `item::Store::apply`'s rule and worth pinning from here
    /// because an industry job is the first thing that mints ten at once.
    #[test]
    fn ten_runs_of_a_singleton_product_become_ten_separate_items() {
        let bp = rifter();
        let jobs = Jobs::new();
        let skills = levels(&[(SKILL_INDUSTRY, 1)]);
        let pilot = pilot_with(Some(&skills), &[], 100_000_000.0);
        let on_hand = minerals_for(bp, 10);
        let install = jobs
            .plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &bpo(500, 691),
                &request(500, 10),
                &on_hand,
            )
            .expect("ten runs");
        assert_eq!(install.cost.base, 90_000);
        assert_eq!(install.materials[0].quantity, 10_000);

        let mut jobs = jobs;
        let job_id = jobs.commit(install.job);
        let (delivery, _) = jobs
            .plan_delivery(t(), fx(), job_id, install.job.end_date + 1)
            .expect("ready");
        assert_eq!(delivery.product.quantity, 10);

        let mut store = item::Store::new();
        store.apply_in(fx(), &delivery.edits).expect("ten hulls");
        assert_eq!(store.items().len(), 10);
        assert!(
            store
                .items()
                .iter()
                .all(|i| i.type_id == 587 && i.quantity == 1)
        );
    }

    /// Every refusal, by name, and every one of them before a single edit is
    /// returned. The list is the point: a refusal that is merely "an error"
    /// is a tooltip the player cannot act on.
    #[test]
    fn every_way_an_install_can_be_refused() {
        let bp = rifter();
        let jobs = Jobs::new();
        let skills = levels(&[(SKILL_INDUSTRY, 1)]);
        let rich = pilot_with(Some(&skills), &[], 1_000_000.0);
        let full = minerals_for(bp, 1);
        let ok = request(500, 1);
        let blueprint = bpo(500, 691);

        // Not an activity at all, then two real activities an original
        // cannot take: invention (`REQUIRES_COPY`) and reactions (a
        // manufacturing blueprint has no activity 9).
        let mut bad = ok;
        bad.activity_id = 2;
        assert_eq!(
            jobs.plan_install(t(), fx(), 0, &rich, &station(), &blueprint, &bad, &full),
            Err(Refusal::InvalidActivity(2))
        );
        for a in [INVENTION, REACTION] {
            let mut r = ok;
            r.activity_id = a;
            assert_eq!(
                jobs.plan_install(t(), fx(), 0, &rich, &station(), &blueprint, &r, &full),
                Err(Refusal::IncompatibleActivity(a)),
                "an original cannot take activity {a}"
            );
        }

        // A blueprint id that is not the blueprint handed in.
        assert_eq!(
            jobs.plan_install(
                t(),
                fx(),
                0,
                &rich,
                &station(),
                &blueprint,
                &request(501, 1),
                &full
            ),
            Err(Refusal::MissingBlueprint)
        );
        // A type that is not a blueprint we know.
        let unknown = BlueprintItem {
            type_id: 587,
            ..blueprint
        };
        assert_eq!(
            jobs.plan_install(t(), fx(), 0, &rich, &station(), &unknown, &ok, &full),
            Err(Refusal::MissingBlueprint)
        );
        // Somebody else's blueprint.
        let theirs = BlueprintItem {
            owner_id: ME + 1,
            ..blueprint
        };
        assert_eq!(
            jobs.plan_install(t(), fx(), 0, &rich, &station(), &theirs, &ok, &full),
            Err(Refusal::InvalidOwner)
        );
        // A blueprint in another station.
        let elsewhere = BlueprintItem {
            location_id: STATION + 1,
            ..blueprint
        };
        assert_eq!(
            jobs.plan_install(t(), fx(), 0, &rich, &station(), &elsewhere, &ok, &full),
            Err(Refusal::BlueprintAccess)
        );
        // An offline facility, and no facility at all.
        let offline = Facility {
            online: false,
            ..station()
        };
        assert_eq!(
            jobs.plan_install(t(), fx(), 0, &rich, &offline, &blueprint, &ok, &full),
            Err(Refusal::FacilityOffline)
        );
        let nowhere = Facility {
            facility_id: 0,
            ..station()
        };
        let homeless = BlueprintItem {
            location_id: 0,
            ..blueprint
        };
        assert_eq!(
            jobs.plan_install(t(), fx(), 0, &rich, &nowhere, &homeless, &ok, &full),
            Err(Refusal::MissingFacility)
        );
        // One jump away, with no Supply Chain Management.
        let remote = Facility {
            distance: 1,
            ..station()
        };
        assert_eq!(
            jobs.plan_install(t(), fx(), 0, &rich, &remote, &blueprint, &ok, &full),
            Err(Refusal::FacilityDistance(1, 0))
        );
        // No input or output location.
        let mut r = ok;
        r.input_location_id = 0;
        assert_eq!(
            jobs.plan_install(t(), fx(), 0, &rich, &station(), &blueprint, &r, &full),
            Err(Refusal::MissingInputLocation)
        );
        let mut r = ok;
        r.output_location_id = 0;
        assert_eq!(
            jobs.plan_install(t(), fx(), 0, &rich, &station(), &blueprint, &r, &full),
            Err(Refusal::MissingOutputLocation)
        );
        // Zero runs, and more runs than 30 days allows: Industry 1 makes a
        // run 576 s, and 2,592,000 / 576 is 4,500.
        assert_eq!(
            jobs.plan_install(
                t(),
                fx(),
                0,
                &rich,
                &station(),
                &blueprint,
                &request(500, 0),
                &full
            ),
            Err(Refusal::InvalidRuns(4_500))
        );
        assert_eq!(
            jobs.plan_install(
                t(),
                fx(),
                0,
                &rich,
                &station(),
                &blueprint,
                &request(500, 4_501),
                &full
            ),
            Err(Refusal::InvalidRuns(4_500))
        );
        // No Industry at all.
        let unskilled = levels(&[]);
        let novice = pilot_with(Some(&unskilled), &[], 1_000_000.0);
        assert_eq!(
            jobs.plan_install(t(), fx(), 0, &novice, &station(), &blueprint, &ok, &full),
            Err(Refusal::MissingSkill(SKILL_INDUSTRY, 1, 0))
        );
        // One mineral short.
        let mut short = full.clone();
        short[0].quantity -= 1;
        assert_eq!(
            jobs.plan_install(t(), fx(), 0, &rich, &station(), &blueprint, &ok, &short),
            Err(Refusal::MissingMaterial(34, 1_000, 999))
        );
        // Minerals in the wrong container.
        let mut misfiled = full.clone();
        for s in &mut misfiled {
            s.slot.flag_id = FLAG_NONE;
        }
        assert_eq!(
            jobs.plan_install(t(), fx(), 0, &rich, &station(), &blueprint, &ok, &misfiled),
            Err(Refusal::MissingMaterial(34, 1_000, 0))
        );
        // Broke.
        let poor = pilot_with(Some(&skills), &[], 1.0);
        assert_eq!(
            jobs.plan_install(t(), fx(), 0, &poor, &station(), &blueprint, &ok, &full),
            Err(Refusal::AccountFunds(1.0, 9_360))
        );
        // A material the item store cannot mint.
        let tempest = bpo(501, 693);
        let tempest_bp = find(693).expect("693 is in the fixture");
        assert_eq!(
            jobs.plan_install(
                t(),
                fx(),
                0,
                &pilot_with(Some(&skills), &[], 1e15),
                &station(),
                &tempest,
                &request(501, 1),
                &minerals_for(tempest_bp, 1)
            ),
            Err(Refusal::UnknownType(UNKNOWN_MATERIAL))
        );
    }

    /// One slot, one job. The second install is refused by `SLOTS_FULL` and
    /// a third blueprint does not help.
    #[test]
    fn a_second_job_needs_a_second_slot() {
        let bp = rifter();
        let mut jobs = Jobs::new();
        let skills = levels(&[(SKILL_INDUSTRY, 1)]);
        let pilot = pilot_with(Some(&skills), &[], 100_000_000.0);
        let on_hand = minerals_for(bp, 10);
        let first = jobs
            .plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &bpo(500, 691),
                &request(500, 1),
                &on_hand,
            )
            .expect("the first job");
        jobs.commit(first.job);

        // The same blueprint is now busy.
        assert_eq!(
            jobs.plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &bpo(500, 691),
                &request(500, 1),
                &on_hand
            ),
            Err(Refusal::BlueprintInstalled(first.job.job_id))
        );
        // A different blueprint runs out of slots instead.
        assert_eq!(
            jobs.plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &bpo(501, 950),
                &request(501, 1),
                &on_hand
            ),
            Err(Refusal::SlotsFull(1, 1))
        );
        // With Mass Production 1 it fits.
        let trained = levels(&[(SKILL_INDUSTRY, 1), (SKILL_MASS_PRODUCTION, 1)]);
        let better = pilot_with(Some(&trained), &[], 100_000_000.0);
        let second = jobs
            .plan_install(
                t(),
                fx(),
                0,
                &better,
                &station(),
                &bpo(501, 950),
                &request(501, 1),
                &on_hand,
            )
            .expect("two slots now");
        assert_ne!(second.job.job_id, first.job.job_id);
        jobs.commit(second.job);
        assert_eq!(jobs.used_slots(ME, MANUFACTURING), 2);
    }

    /// Cancelling returns **no edits**: the materials are gone and there is
    /// nothing half-done to compensate for.
    #[test]
    fn cancelling_a_job_refunds_nothing_and_frees_the_blueprint() {
        let bp = rifter();
        let mut jobs = Jobs::new();
        let skills = levels(&[(SKILL_INDUSTRY, 1)]);
        let pilot = pilot_with(Some(&skills), &[], 1_000_000.0);
        let install = jobs
            .plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &bpo(500, 691),
                &request(500, 1),
                &minerals_for(bp, 1),
            )
            .expect("installed");
        let job_id = jobs.commit(install.job);
        let cancelled = jobs.plan_cancel(job_id, 123).expect("cancellable");
        assert_eq!(cancelled.status, Status::Cancelled);
        assert!(
            cancelled.completed(),
            "102 >= 100, so the client frees the blueprint"
        );
        assert_eq!(cancelled.end_date, 123);
        jobs.commit(cancelled);
        assert!(jobs.job_for_blueprint(500).is_none());
        // And a second cancel is refused rather than silently repeated.
        assert!(jobs.plan_cancel(job_id, 124).is_err());
        assert!(jobs.plan_delivery(t(), fx(), job_id, i64::MAX).is_err());
        assert!(jobs.plan_cancel(job_id + 999, 0).is_err());
    }

    /// `GetJobCounts` must be non-empty and int-keyed over all six
    /// activities; `GetJobsByOwner`'s second argument really does mean
    /// something.
    #[test]
    fn the_job_counts_cover_every_activity_and_the_owner_filter_is_real() {
        let bp = rifter();
        let mut jobs = Jobs::new();
        let skills = levels(&[(SKILL_INDUSTRY, 1)]);
        let pilot = pilot_with(Some(&skills), &[], 1_000_000.0);
        assert_eq!(jobs.counts(ME).len(), 6);
        assert!(jobs.counts(ME).iter().all(|&(_, n)| n == 0));
        assert_eq!(
            jobs.counts(ME).iter().map(|&(a, _)| a).collect::<Vec<_>>(),
            ACTIVITIES.to_vec()
        );

        let install = jobs
            .plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &bpo(500, 691),
                &request(500, 1),
                &minerals_for(bp, 1),
            )
            .expect("installed");
        let job_id = jobs.commit(install.job);
        assert_eq!(jobs.used_slots(ME, MANUFACTURING), 1);
        assert_eq!(jobs.used_slots(ME, COPYING), 0);
        assert_eq!(jobs.used_slots(ME + 1, MANUFACTURING), 0);
        assert_eq!(jobs.for_owner(ME, false).len(), 1);
        assert_eq!(jobs.for_owner(ME + 1, false).len(), 0);

        // Delivered jobs leave the live list and stay in the history, which
        // is what `includeCompleted` is for.
        let (_, done) = jobs
            .plan_delivery(t(), fx(), job_id, install.job.end_date + 1)
            .expect("ready");
        jobs.commit(done);
        assert_eq!(jobs.for_owner(ME, false).len(), 0);
        assert_eq!(jobs.for_owner(ME, true).len(), 1);
        assert!(jobs.counts(ME).iter().all(|&(_, n)| n == 0));
    }

    /// Ids are monotonic across a restore, for the same reason item ids are:
    /// the client caches by id.
    #[test]
    fn job_ids_are_monotonic_and_survive_a_restore() {
        let mut jobs = Jobs::new();
        assert_eq!(jobs.next_id(), MIN_JOB_ID);
        assert!(jobs.is_empty());
        let mut a = installed_job();
        a.job_id = MIN_JOB_ID;
        jobs.commit(a);
        assert_eq!(jobs.next_id(), MIN_JOB_ID + 1);

        let restored = Jobs::restore(jobs.all().to_vec(), jobs.next_id());
        assert_eq!(restored.next_id(), MIN_JOB_ID + 1);
        assert_eq!(restored.get(MIN_JOB_ID).map(|j| j.job_id), Some(MIN_JOB_ID));
        // A snapshot whose allocator trails its own jobs does not reissue.
        let trailing = Jobs::restore(jobs.all().to_vec(), 0);
        assert!(trailing.next_id() >= MIN_JOB_ID);
        // And the ids stay above the client's local-sqlite threshold:
        // `idCheckers.IsSystemOrNPC(id)` is `id < 90000000`, and below it the
        // client answers from its own sqlite before asking us.
        const _ABOVE_THE_NPC_THRESHOLD: () = assert!(MIN_JOB_ID > 90_000_000);
    }

    /// The plan is one list and `apply` is all-or-nothing, so a plan whose
    /// materials have since moved leaves the hangar exactly as it was.
    #[test]
    fn a_stale_plan_changes_nothing_at_all() {
        let bp = rifter();
        let jobs = Jobs::new();
        let skills = levels(&[(SKILL_INDUSTRY, 1)]);
        let pilot = pilot_with(Some(&skills), &[], 1_000_000.0);
        let on_hand = minerals_for(bp, 1);
        let install = jobs
            .plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &bpo(500, 691),
                &request(500, 1),
                &on_hand,
            )
            .expect("installed");

        let mut store = item::Store::new();
        // Every stack but the last, so the plan's final edit cannot resolve.
        for s in &on_hand[..3] {
            store.insert(item_at(s.item_id, s.type_id, s.quantity, FLAG_HANGAR));
        }
        let before = store.items().to_vec();
        let err = store
            .apply_in(fx(), &install.edits)
            .expect_err("the plan is stale");
        assert_eq!(err, item::EditError::NoSuchItem(100_003));
        assert_eq!(store.items(), before.as_slice(), "nothing was applied");
    }

    /// Every job this module can build carries a status the client can read
    /// and a product the store can mint -- a sweep over every buildable row
    /// rather than one worked example.
    #[test]
    fn every_buildable_blueprint_installs_and_delivers() {
        let skills = levels(&[(SKILL_INDUSTRY, 5), (SKILL_MASS_PRODUCTION, 5)]);
        let mut built = 0;
        for bp in t().blueprints.iter().filter(|b| buildable(fx(), b)) {
            let jobs = Jobs::new();
            let pilot = pilot_with(Some(&skills), &[], 1e15);
            let install = jobs
                .plan_install(
                    t(),
                    fx(),
                    0,
                    &pilot,
                    &station(),
                    &bpo(500, bp.blueprint_type_id),
                    &request(500, 1),
                    &minerals_for(bp, 1),
                )
                .unwrap_or_else(|e| panic!("{} should install, got {e:?}", bp.blueprint_type_id));
            assert!(install.job.time_in_seconds > 0);
            assert!(install.cost.base > 0, "{} is free", bp.blueprint_type_id);
            assert_eq!(install.edits.len(), bp.materials.len());
            assert_eq!(
                Status::from_code(install.job.status.code()),
                Some(Status::Installed)
            );

            let mut jobs = jobs;
            let job_id = jobs.commit(install.job);
            let (delivery, done) = jobs
                .plan_delivery(t(), fx(), job_id, install.job.end_date + 1)
                .expect("ready");
            assert_eq!(delivery.product.type_id, bp.product_type_id);
            assert_eq!(delivery.product.quantity, bp.product_quantity);
            let mut store = item::Store::new();
            store
                .apply_in(fx(), &delivery.edits)
                .expect("the product is mintable");
            assert!(!store.items().is_empty());
            assert!(done.completed());
            built += 1;
        }
        assert!(built > 1, "the fixture has more than one buildable row");
    }

    /// The refusal codes really are `industry.Error`'s, because the client
    /// labels them by number through `industryCommon.GetErrorLabel` and an
    /// id we invent renders as the enum member's name instead of a sentence.
    #[test]
    fn every_refusal_code_is_one_of_ccps_forty() {
        // `industry.Error`, verbatim from `industry/const.pyc`.
        const CCP: [i64; 40] = [
            1, 2, 3, 4, 5, 29, 8, 9, 10, 11, 45, 46, 12, 13, 14, 15, 16, 17, 18, 30, 19, 20, 44,
            21, 22, 23, 24, 25, 26, 27, 28, 31, 32, 33, 34, 38, 39, 40, 41, 43,
        ];
        let every = [
            Refusal::InvalidOwner,
            Refusal::InvalidRuns(0),
            Refusal::MissingInputLocation,
            Refusal::MissingOutputLocation,
            Refusal::InvalidProduct,
            Refusal::InvalidActivity(0),
            Refusal::MissingFacility,
            Refusal::FacilityOffline,
            Refusal::MissingBlueprint,
            Refusal::MissingMaterial(0, 0, 0),
            Refusal::AccountFunds(0.0, 0),
            Refusal::IncompatibleActivity(0),
            Refusal::BlueprintAccess,
            Refusal::BlueprintInstalled(0),
            Refusal::MissingSkill(0, 0, 0),
            Refusal::SlotsFull(0, 0),
            Refusal::FacilityDistance(0, 0),
            Refusal::RunLength(0, 0),
            Refusal::UnknownType(0),
            Refusal::InvalidLicensedRuns(0),
            Refusal::ResearchLimit(0, 0),
        ];
        for r in every {
            assert!(
                CCP.contains(&r.code()),
                "{r:?} maps to {}, which is not an industry.Error",
                r.code()
            );
            // And never into the 47/48 range CCP added after this subset was
            // enumerated, nor into 0, which is not an error at all.
            assert!(r.code() > 0);
        }
        assert_eq!(
            Refusal::UnknownType(0).code(),
            Refusal::InvalidProduct.code()
        );
    }

    // -----------------------------------------------------------------
    // Round 2: research, copying, invention.
    // -----------------------------------------------------------------

    const WOLF_BLUEPRINT: i64 = 11372;
    const JAGUAR_BLUEPRINT: i64 = 11401;
    const MINMATAR_CORE: i64 = 20172;
    const MECHANICAL_CORE: i64 = 20424;

    /// A blueprint copy in the station hangar.
    fn bpc(item_id: i64, type_id: i64, runs: i64) -> BlueprintItem {
        BlueprintItem {
            runs_remaining: runs,
            ..bpo(item_id, type_id)
        }
    }

    fn science_request(activity_id: i64, blueprint_id: i64, runs: i64) -> InstallRequest {
        InstallRequest {
            activity_id,
            ..request(blueprint_id, runs)
        }
    }

    /// The edits' `Consume`s, summed per item id -- what a plan really takes.
    fn consumed(edits: &[Edit]) -> Vec<(i64, i64)> {
        let mut out: Vec<(i64, i64)> = Vec::new();
        for e in edits {
            if let Edit::Consume { item_id, quantity } = e {
                match out.iter_mut().find(|(i, _)| i == item_id) {
                    Some((_, q)) => *q += quantity,
                    None => out.push((*item_id, *quantity)),
                }
            }
        }
        out
    }

    /// One science row per blueprint, in the same order, and every
    /// invention product of the fixture is a blueprint the table can find --
    /// which `plan_install` requires before it mints one.
    #[test]
    fn the_science_rows_follow_the_blueprints() {
        assert_eq!(
            t().science
                .iter()
                .map(|s| s.blueprint_type_id)
                .collect::<Vec<_>>(),
            t().blueprints
                .iter()
                .map(|b| b.blueprint_type_id)
                .collect::<Vec<_>>(),
            "one science row per blueprint, in the same order"
        );
        for inv in t().science.iter().filter_map(|s| s.invention.as_ref()) {
            assert!(!inv.skills.is_empty(), "every invention needs skills");
            for p in &inv.products {
                assert!(
                    is_blueprint_type(p.type_id),
                    "an invented copy must be a blueprint"
                );
            }
        }
    }

    /// `industry.RESEARCH_TIMES`, the ten Python divisions verbatim.
    #[test]
    fn research_times_are_the_clients_ten_fractions() {
        assert_eq!(RESEARCH_TIMES.len(), 10);
        assert_eq!(RESEARCH_TIMES[0], 1.0);
        assert_eq!(RESEARCH_TIMES[1], 29.0 / 21.0);
        assert_eq!(RESEARCH_TIMES[4], 278.0 / 15.0);
        assert_eq!(RESEARCH_TIMES[9], 29660.0 / 21.0);
        assert!(RESEARCH_TIMES.windows(2).all(|w| w[0] < w[1]));
    }

    /// `job_time` sums the levels **from the current one**, and clamps at
    /// the last.
    #[test]
    fn research_time_sums_the_levels_from_the_current_one() {
        let base = science(691)
            .and_then(|s| s.research_material_time)
            .expect("691 researches");
        let b = base as f64;
        assert_eq!(research_job_time(base, 0, 1), b);
        assert_eq!(research_job_time(base, 0, 2), b + b * (29.0 / 21.0));
        assert_eq!(
            research_job_time(base, 3, 1),
            b * (39.0 / 5.0),
            "level 4 costs RESEARCH_TIMES[3]"
        );
        assert_eq!(
            research_job_time(base, 9, 3),
            3.0 * b * (29660.0 / 21.0),
            "past the last level every step costs the last level"
        );
        // ME's level is ME / 1 and TE's is TE / 2.
        assert_eq!(research_level(RESEARCH_MATERIAL, 7, 14), 7);
        assert_eq!(research_level(RESEARCH_TIME, 7, 14), 7);
    }

    /// `CHARACTER_MODIFIERS` per activity, with each skill's `PostPercent`
    /// read from `dogmaeffects`' `modifierInfo`.
    #[test]
    fn each_science_activity_has_its_own_time_skills() {
        let bp = rifter();
        let none = |_: i64| 0;
        for a in SCIENCE_ACTIVITIES {
            assert_eq!(activity_time_multiplier(a, bp, none, &[]), 1.0);
        }
        let five = |t: i64| {
            if [
                SKILL_RESEARCH,
                SKILL_METALLURGY,
                SKILL_SCIENCE,
                SKILL_ADVANCED_INDUSTRY,
            ]
            .contains(&t)
            {
                5
            } else {
                0
            }
        };
        let advanced = 1.0 + -3.0 * 5.0 / 100.0;
        let skill = 1.0 + -5.0 * 5.0 / 100.0;
        assert_eq!(
            activity_time_multiplier(RESEARCH_TIME, bp, five, &[]),
            skill * advanced
        );
        assert_eq!(
            activity_time_multiplier(RESEARCH_MATERIAL, bp, five, &[]),
            skill * advanced
        );
        assert_eq!(
            activity_time_multiplier(COPYING, bp, five, &[]),
            skill * advanced
        );
        assert_eq!(activity_time_multiplier(INVENTION, bp, five, &[]), advanced);
        // Metallurgy speeds ME research and nothing else.
        let metallurgy = |t: i64| if t == SKILL_METALLURGY { 5 } else { 0 };
        assert_eq!(
            activity_time_multiplier(RESEARCH_TIME, bp, metallurgy, &[]),
            1.0
        );
        assert_eq!(activity_time_multiplier(COPYING, bp, metallurgy, &[]), 1.0);
        assert_eq!(
            activity_time_multiplier(RESEARCH_MATERIAL, bp, metallurgy, &[]),
            skill
        );
    }

    /// `maxLaborotorySlots`: 1 on the character type, +1 per level of each
    /// laboratory skill.
    #[test]
    fn a_character_with_no_skills_has_one_laboratory_slot() {
        assert_eq!(laboratory_slots(|_| 0), 1);
        assert_eq!(
            laboratory_slots(|t| if t == SKILL_LABORATORY_OPERATION {
                3
            } else {
                0
            }),
            4
        );
        assert_eq!(laboratory_slots(|_| 5), MAX_LABORATORY_SLOTS);
    }

    /// `Activity.job_max_runs` and the research and copying overrides.
    #[test]
    fn science_max_runs_are_ccps_ceilings() {
        // Copying: ceil(2,592,000 / 4,800) = 540, capped by MAX_RUNS 500.
        assert_eq!(activity_max_runs(4800, 1.0, COPYING_MAX_RUNS), 500);
        // A slow enough activity is bounded by the thirty days instead.
        assert_eq!(activity_max_runs(128_100, 1.0, INVENTION_MAX_RUNS), 21);
        let jobs = Jobs::new();
        let pilot = pilot_with(None, &[], 1e12);
        // ME 8 has two levels left, so three runs is one too many.
        let at_eight = BlueprintItem {
            material_efficiency: 8,
            ..bpo(500, 691)
        };
        assert_eq!(
            jobs.plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &at_eight,
                &science_request(RESEARCH_MATERIAL, 500, 3),
                &[]
            )
            .map(|i| i.job.runs),
            Err(Refusal::InvalidRuns(2))
        );
    }

    /// The whole ME research path: install, the clock, delivery, and the
    /// blueprint row that comes out the other side.
    #[test]
    fn an_me_research_job_raises_the_blueprints_material_efficiency() {
        let mut jobs = Jobs::new();
        let pilot = pilot_with(None, &[], 1e12);
        let bp = rifter();
        let install = jobs
            .plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &bpo(500, 691),
                &science_request(RESEARCH_MATERIAL, 500, 3),
                &[],
            )
            .expect("research needs no skill and no material");
        assert!(install.edits.is_empty(), "research consumes nothing");
        assert!(install.materials.is_empty());
        let base = science(691)
            .and_then(|s| s.research_material_time)
            .expect("691 researches");
        let raw = research_job_time(base, 0, 3);
        assert_eq!(install.job.time_in_seconds, raw.round() as i64);
        // `prices[bp] * COST_PERCENTAGE * job_time / float(time)`, then the
        // halved surcharge research gets.
        let want = cost_lines(
            estimated_item_value(t(), bp) * COST_PERCENTAGE * raw / base as f64,
            RESEARCH_MATERIAL,
            &station(),
            1.0,
        );
        assert_eq!(install.cost, want);
        assert_eq!(
            want.scc_surcharge,
            (want.base as f64 * SCC_SURCHARGE_RATE * SCC_SURCHARGE_RESEARCH_DISCOUNT_MODIFIER)
                .round() as i64
        );
        assert_eq!(install.job.probability, 1.0);
        let job_id = jobs.commit_install(&install);

        let (delivery, done) = jobs
            .plan_delivery(t(), fx(), job_id, install.job.end_date + 1)
            .expect("ready");
        assert!(delivery.edits.is_empty(), "nothing is minted");
        assert_eq!(
            delivery.blueprint_after.map(|s| s.material_efficiency),
            Some(3)
        );
        jobs.commit_delivery(&delivery, done, &[]);
        let item = item::Item {
            item_id: 500,
            type_id: 691,
            owner_id: ME,
            location_id: STATION,
            flag_id: FLAG_HANGAR,
            quantity: 1,
            group_id: 105,
            category_id: CATEGORY_BLUEPRINT,
            singleton: true,
        };
        let shown = jobs.blueprint_item(t(), &item).expect("a blueprint");
        assert_eq!(
            (
                shown.material_efficiency,
                shown.time_efficiency,
                shown.runs_remaining
            ),
            (3, 0, -1),
            "still an original, three levels better"
        );
    }

    /// TE steps by two, stops at twenty, and a finished blueprint is refused
    /// with `RESEARCH_LIMIT` and the client's two arguments.
    #[test]
    fn te_research_steps_by_two_and_the_limit_is_refused_by_name() {
        let mut jobs = Jobs::new();
        let pilot = pilot_with(None, &[], 1e12);
        let at_sixteen = BlueprintItem {
            time_efficiency: 16,
            ..bpo(500, 691)
        };
        let install = jobs
            .plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &at_sixteen,
                &science_request(RESEARCH_TIME, 500, 2),
                &[],
            )
            .expect("two levels left");
        // Level 8 and level 9 of the blueprint's TE research time.
        let base = science(691)
            .and_then(|s| s.research_time_time)
            .expect("691 researches");
        assert_eq!(
            install.job.time_in_seconds,
            research_job_time(base, 8, 2).round() as i64
        );
        jobs.blueprints.set(at_sixteen.state());
        let job_id = jobs.commit_install(&install);
        let (delivery, _) = jobs
            .plan_delivery(t(), fx(), job_id, install.job.end_date + 1)
            .expect("ready");
        assert_eq!(
            delivery.blueprint_after.map(|s| s.time_efficiency),
            Some(20)
        );

        for (me, te, activity) in [(10, 0, RESEARCH_MATERIAL), (0, 20, RESEARCH_TIME)] {
            let done = BlueprintItem {
                material_efficiency: me,
                time_efficiency: te,
                ..bpo(501, 691)
            };
            assert_eq!(
                Jobs::new().plan_install(
                    t(),
                    fx(),
                    0,
                    &pilot,
                    &station(),
                    &done,
                    &science_request(activity, 501, 1),
                    &[]
                ),
                Err(Refusal::ResearchLimit(10, 10))
            );
        }
    }

    /// **The point of research**: the same job from a researched blueprint
    /// eats fewer minerals, and what it eats is exactly what it quoted.
    #[test]
    fn a_researched_blueprint_manufactures_from_fewer_materials() {
        let bp = rifter();
        let skills = levels(&[(SKILL_INDUSTRY, 1)]);
        let pilot = pilot_with(Some(&skills), &[], 1e12);
        let on_hand = minerals_for(bp, 2);
        let plain = Jobs::new()
            .plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &bpo(500, 691),
                &request(500, 2),
                &on_hand,
            )
            .expect("ME 0");
        let researched = BlueprintItem {
            material_efficiency: 10,
            ..bpo(500, 691)
        };
        let better = Jobs::new()
            .plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &researched,
                &request(500, 2),
                &on_hand,
            )
            .expect("ME 10");
        for (a, b) in plain.materials.iter().zip(&better.materials) {
            assert_eq!(a.type_id, b.type_id);
            assert!(
                b.quantity < a.quantity,
                "{} must cost less at ME 10",
                a.type_id
            );
            assert_eq!(b.quantity, material_quantity(a.quantity / 2, 2, 10, 1.0));
        }
        // Materials consumed = materials quoted, stack by stack.
        let quoted: Vec<(i64, i64)> = better
            .materials
            .iter()
            .map(|m| {
                let stack = on_hand
                    .iter()
                    .find(|s| s.type_id == m.type_id)
                    .expect("stocked");
                (stack.item_id, m.quantity)
            })
            .collect();
        assert_eq!(consumed(&better.edits), quoted);
    }

    /// Copying: `runs` copies of `licensedRuns` each, priced and timed by the
    /// licence, carrying the original's ME and TE; the licence is refused
    /// outside `1..=maxProductionLimit`.
    #[test]
    fn copying_mints_copies_with_the_licence_and_the_originals_efficiency() {
        let mut jobs = Jobs::new();
        let pilot = pilot_with(None, &[], 1e12);
        let bp = rifter();
        let original = BlueprintItem {
            material_efficiency: 5,
            time_efficiency: 10,
            ..bpo(500, 691)
        };
        jobs.blueprints.set(original.state());
        let mut r = science_request(COPYING, 500, 2);
        for bad in [0, bp.max_production_limit + 1] {
            r.licensed_runs = bad;
            assert_eq!(
                jobs.plan_install(t(), fx(), 0, &pilot, &station(), &original, &r, &[]),
                Err(Refusal::InvalidLicensedRuns(bp.max_production_limit))
            );
        }
        r.licensed_runs = 5;
        let install = jobs
            .plan_install(t(), fx(), 0, &pilot, &station(), &original, &r, &[])
            .expect("a copy job");
        // `self.time * licensedRuns * runs`, and the copy's own base time.
        let copying = science(691)
            .and_then(|s| s.copying_time)
            .expect("691 copies");
        assert_eq!(install.job.time_in_seconds, copying * 5 * 2);
        assert_eq!(
            install.cost,
            cost_lines(
                estimated_item_value(t(), bp) * COST_PERCENTAGE * 5.0 * 2.0,
                COPYING,
                &station(),
                1.0
            )
        );
        assert_eq!(install.job.licensed_runs, 5);
        assert!(
            install.blueprint_after.is_none(),
            "an original spends nothing"
        );
        let job_id = jobs.commit_install(&install);

        let (delivery, done) = jobs
            .plan_delivery(t(), fx(), job_id, install.job.end_date + 1)
            .expect("ready");
        assert_eq!(
            delivery.edits,
            vec![Edit::Create {
                type_id: 691,
                quantity: 2,
                to: Slot::new(ME, STATION, FLAG_HANGAR),
            }]
        );
        // Through a real store, so the ids are the ones `apply` chose.
        let mut store = item::Store::new();
        let changes = store
            .apply_in(fx(), &delivery.edits)
            .expect("691 is an item type");
        jobs.commit_delivery(&delivery, done, &changes);
        let copies: Vec<BlueprintItem> = store
            .items()
            .iter()
            .filter(|i| i.type_id == 691)
            .filter_map(|i| jobs.blueprint_item(t(), i))
            .collect();
        assert_eq!(copies.len(), 2, "two singleton copies, not a stack of two");
        for c in copies {
            assert_eq!(
                (c.runs_remaining, c.material_efficiency, c.time_efficiency),
                (5, 5, 10)
            );
            assert_eq!(c.wire_quantity(), -2, "the client's mark of a copy");
        }
    }

    /// `REQUIRES_ORIGINAL` and `REQUIRES_COPY`, through `Blueprint.activities`.
    #[test]
    fn a_copy_cannot_research_or_copy_and_an_original_cannot_invent() {
        let jobs = Jobs::new();
        let pilot = pilot_with(None, &[], 1e12);
        for a in [RESEARCH_MATERIAL, RESEARCH_TIME, COPYING] {
            let mut r = science_request(a, 500, 1);
            r.licensed_runs = 1;
            assert_eq!(
                jobs.plan_install(
                    t(),
                    fx(),
                    0,
                    &pilot,
                    &station(),
                    &bpc(500, 691, 10),
                    &r,
                    &[]
                ),
                Err(Refusal::IncompatibleActivity(a))
            );
        }
        assert_eq!(
            jobs.plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &bpo(500, 691),
                &science_request(INVENTION, 500, 1),
                &[]
            ),
            Err(Refusal::IncompatibleActivity(INVENTION))
        );
        // And a blueprint with no invention activity at all.
        assert_eq!(
            jobs.plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &bpc(500, 21858, 10),
                &science_request(INVENTION, 500, 1),
                &[]
            ),
            Err(Refusal::IncompatibleActivity(INVENTION))
        );
    }

    /// Manufacturing from a copy is bounded by its runs --
    /// `runs > runsRemaining` is `INVALID_RUNS` -- and is a copy job on the
    /// row. *When* the runs are spent, and what becomes of an empty copy, is
    /// `[A]` (see [`Install::blueprint_after`]) and deliberately not asserted.
    #[test]
    fn manufacturing_from_a_copy_is_bounded_by_its_runs() {
        let bp = rifter();
        let skills = levels(&[(SKILL_INDUSTRY, 1)]);
        let pilot = pilot_with(Some(&skills), &[], 1e12);
        let mut jobs = Jobs::new();
        let copy = bpc(500, 691, 3);
        jobs.blueprints.set(copy.state());
        assert_eq!(
            jobs.plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &copy,
                &request(500, 4),
                &minerals_for(bp, 4)
            ),
            Err(Refusal::InvalidRuns(3))
        );
        let all = jobs
            .plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &copy,
                &request(500, 3),
                &minerals_for(bp, 3),
            )
            .expect("all three runs");
        assert!(all.job.blueprint_copy, "`blueprintCopy` on the job row");
        // The minerals are still exactly what was quoted, copy or not.
        let minerals: Vec<(i64, i64)> = consumed(&all.edits)
            .into_iter()
            .filter(|&(i, _)| i != 500)
            .collect();
        let quoted: Vec<(i64, i64)> = minerals_for(bp, 3)
            .iter()
            .zip(&all.materials)
            .map(|(s, m)| (s.item_id, m.quantity))
            .collect();
        assert_eq!(minerals, quoted);
    }

    /// `Invention.job_probability`, by hand from the client's formula: the
    /// fixture's two products are both 0.25, and its three skills are two at
    /// 1/30 a level and one in type list 799 at 1/40.
    #[test]
    fn invention_probability_is_the_clients_formula() {
        let inv = science(691)
            .and_then(|s| s.invention.as_ref())
            .expect("691 invents");
        assert_eq!(
            inv.products
                .iter()
                .map(|p| (p.type_id, p.probability))
                .collect::<Vec<_>>(),
            vec![(WOLF_BLUEPRINT, 0.25), (JAGUAR_BLUEPRINT, 0.25)]
        );
        assert_eq!(invention_probability(inv, |_| 0), 0.25);
        let one = |_: i64| 1;
        let amount = 1.0 + (1.0 / 30.0 + 1.0 / 30.0 + 1.0 / 40.0);
        assert_eq!(invention_probability(inv, one), 1.0 * 0.25 * amount);
        assert_eq!(skill_probability(21791), INVENTION_SKILL_PROBABILITY_LOWER);
        assert_eq!(skill_probability(11452), INVENTION_SKILL_PROBABILITY);
        // Clamped to 1.0, never above.
        let sure = Invention {
            products: vec![InventionProduct {
                type_id: WOLF_BLUEPRINT,
                quantity: 1,
                probability: 0.9,
            }],
            ..inv.clone()
        };
        assert_eq!(invention_probability(&sure, |_| 5), 1.0);
    }

    /// The roll: bounded, certain at 1.0, and the same answer every time it
    /// is asked -- which is what makes a restart safe.
    #[test]
    fn successful_runs_is_bounded_and_replays() {
        for seed in 0..50u64 {
            let n = successful_runs(seed, 20, 0.3);
            assert!((0..=20).contains(&n));
            assert_eq!(n, successful_runs(seed, 20, 0.3), "same seed, same answer");
            assert_eq!(successful_runs(seed, 20, 1.0), 20);
        }
        // Uniform enough that 0.3 means about 30 in 100.
        let hits = successful_runs(7, 10_000, 0.3);
        assert!((2_700..=3_300).contains(&hits), "{hits} of 10,000 at 0.3");
    }

    /// The whole invention path from a copy: datacores consumed as
    /// quoted, the copy's runs spent, the product chosen by the request, and
    /// the delivery minting `successfulRuns` Wolf copies at ME 2 / TE 4 with
    /// one run each.
    #[test]
    fn invention_eats_datacores_and_mints_tech_two_copies() {
        let skills = levels(&[(11445, 1), (11452, 1), (21791, 1)]);
        let pilot = pilot_with(Some(&skills), &[], 1e12);
        let mut jobs = Jobs::new();
        let copy = bpc(500, 691, 10);
        jobs.blueprints.set(copy.state());
        let cores = vec![
            Stack {
                item_id: 100_000,
                type_id: MINMATAR_CORE,
                quantity: 50,
                slot: Slot::new(ME, STATION, FLAG_HANGAR),
            },
            Stack {
                item_id: 100_001,
                type_id: MECHANICAL_CORE,
                quantity: 50,
                slot: Slot::new(ME, STATION, FLAG_HANGAR),
            },
        ];
        // Two products and no choice: `Job._get_product` is `None`.
        assert_eq!(
            jobs.plan_install(
                t(),
                fx(),
                0,
                &pilot,
                &station(),
                &copy,
                &science_request(INVENTION, 500, 4),
                &cores
            ),
            Err(Refusal::InvalidProduct)
        );
        // And the skills are required, not only read.
        let unskilled = pilot_with(None, &[], 1e12);
        let mut r = science_request(INVENTION, 500, 4);
        r.product_type_id = Some(WOLF_BLUEPRINT);
        assert_eq!(
            jobs.plan_install(t(), fx(), 0, &unskilled, &station(), &copy, &r, &cores),
            Err(Refusal::MissingSkill(11445, 1, 0))
        );
        let install = jobs
            .plan_install(t(), fx(), 0, &pilot, &station(), &copy, &r, &cores)
            .expect("an invention");
        assert_eq!(
            install.materials,
            vec![
                Material {
                    type_id: MINMATAR_CORE,
                    quantity: 2 * 4
                },
                Material {
                    type_id: MECHANICAL_CORE,
                    quantity: 2 * 4
                },
            ]
        );
        assert_eq!(consumed(&install.edits), vec![(100_000, 8), (100_001, 8)]);
        let inv = science(691)
            .and_then(|s| s.invention.as_ref())
            .expect("691 invents");
        assert_eq!(install.job.time_in_seconds, inv.time * 4);
        assert_eq!(install.job.product_type_id, WOLF_BLUEPRINT);
        assert_eq!(install.job.probability, invention_probability(inv, |_| 1));
        // `prices[product.typeID] * COST_PERCENTAGE * runs`: the *Wolf*
        // blueprint's estimated item value, not the Rifter's.
        let wolf = find(WOLF_BLUEPRINT).expect("in the fixture");
        assert_eq!(
            install.cost,
            cost_lines(
                estimated_item_value(t(), wolf) * COST_PERCENTAGE * 4.0,
                INVENTION,
                &station(),
                1.0
            )
        );
        assert_eq!(install.job.successful_runs, 4, "runs, until it completes");
        let job_id = jobs.commit_install(&install);

        let (delivery, done) = jobs
            .plan_delivery(t(), fx(), job_id, install.job.end_date + 1)
            .expect("ready");
        let successes = done.successful_runs;
        assert!((0..=4).contains(&successes), "at most one copy per run");
        assert_eq!(delivery.product.quantity, successes);
        assert_eq!(delivery.product.type_id, WOLF_BLUEPRINT);
        assert_eq!(
            delivery.creates,
            Some(BlueprintState {
                item_id: 0,
                runs_remaining: 1,
                material_efficiency: INVENTION_MATERIAL_EFFICIENCY,
                time_efficiency: INVENTION_TIME_EFFICIENCY,
            })
        );
        if successes > 0 {
            let mut store = item::Store::new();
            let changes = store
                .apply_in(fx(), &delivery.edits)
                .expect("Wolf Blueprint is an item type");
            jobs.commit_delivery(&delivery, done, &changes);
            let wolves: Vec<BlueprintItem> = store
                .items()
                .iter()
                .filter(|i| i.type_id == WOLF_BLUEPRINT)
                .filter_map(|i| jobs.blueprint_item(t(), i))
                .collect();
            assert_eq!(wolves.len() as i64, successes);
            assert!(wolves.iter().all(|w| w.state()
                == BlueprintState {
                    item_id: w.item_id,
                    runs_remaining: 1,
                    material_efficiency: 2,
                    time_efficiency: 4,
                }));
        } else {
            assert!(
                delivery.edits.is_empty(),
                "nothing invented, nothing minted"
            );
        }
    }

    /// The `bpstate` records: render, parse every line back, same table; an
    /// unreadable line is refused, and an original's state is no row at all.
    #[test]
    fn the_blueprint_table_survives_the_world_file() {
        let mut table = Blueprints::default();
        table.set(BlueprintState {
            item_id: 7,
            runs_remaining: -1,
            material_efficiency: 10,
            time_efficiency: 20,
        });
        table.set(BlueprintState {
            item_id: 3,
            runs_remaining: 5,
            material_efficiency: 2,
            time_efficiency: 4,
        });
        table.set(BlueprintState::original(9));
        assert_eq!(table.len(), 2, "an unresearched original is not a row");
        let mut text = String::new();
        table.render(&mut text);
        let mut back = Blueprints::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            assert!(Blueprints::RECORDS.contains(&f[0]), "{line}");
            back.parse_record(&f).expect("our own line");
        }
        assert_eq!(back, table);
        assert_eq!(
            back.iter().map(|s| s.item_id).collect::<Vec<_>>(),
            vec![3, 7]
        );
        assert!(back.parse_record(&["bpstate", "1", "x", "0", "0"]).is_err());
        let mut empty = String::new();
        Blueprints::default().render(&mut empty);
        assert!(empty.is_empty());
    }
}
