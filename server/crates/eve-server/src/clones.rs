//! Clones (P5.9) -- the home (medical clone) station, jump clones, and the
//! revive a podded character needs.
//!
//! Moved verbatim out of `service/character.rs` by IP38-a; the arms that
//! serve it stay there.
//!
//! This module is the clones lane's. The arms that call into it
//! (`home_station`, `jumpCloneSvc`, `charMgr.GetHomeStationRow`) sit in
//! `service/character.rs`'s `dispatch`, because every arm in that file does.
//! `docs/lanes/clones.md` has the research (every call, every push, and the
//! "is the empty answer a lie?" verdicts), the coordinator edits, and exactly
//! how the death wiring (P2.2b) calls [`revive_in_clone`].
//!
//! **The home station is the medical clone.** In this build there is no
//! separate clone-station setting anywhere in the client: the Character
//! Sheet's Home Station tab is the only place one is chosen
//! (`homestation/client/ui/page/select.py`), and its "self-destruct clone"
//! button moves the player *to the home station* (`ui/page/home.py(455)`,
//! validated by `homestation/validation.py(63)`). So "where a podded player
//! wakes up" and "the home station" are one number here. **[I]** that the
//! real server revived a podded capsuleer there too; `docs/re/damage-death.md`
//! §5.2 makes the same call as `[A]`.

use crate::model::Character;
use crate::pyutil::{bstr, keyval};
use crate::service::{GROUP_CAPSULE, Registry, Request, ServiceError, TYPE_CAPSULE, clone_state};
use eve_marshal::{DbRowDescriptor, DbType, DbValue, PyValue};

/// The most jump clones one character may hold. **[A]**: TQ gates this on
/// the character's `maxJumpClones` attribute (Infomorph Psychology and its
/// advanced skill, five each), which the panel reads client-side
/// (`jumpClonesPanel.py(650) clone_limit`) and which we do not compute. Ten is
/// that attribute's ceiling with both skills at V, so nobody is capped below
/// what a trained character could have. The panel's "N out of maximum L"
/// shows the client's own number, not this one.
pub(crate) const MAX_JUMP_CLONES: usize = 10;

/// The first jump clone id handed out. **[A]**: any positive number that
/// fits the `I4` column; nothing looks it up anywhere (`cfg`, `evelocations`)
/// -- the panel only keys on it and seeds `random.Random(id)` with it for the
/// default name (`jumpClonesPanel.py` `JumpClone.default_name`).
const FIRST_JUMP_CLONE_ID: i64 = 1;

/// `UI/Medical/Errors/JumpCloneValidationNotDocked`, a label in this build's
/// table (message 572712) **[V]**.
pub(crate) const CLONE_ERR_NOT_DOCKED: &str = "UI/Medical/Errors/JumpCloneValidationNotDocked";

/// `UI/Medical/Errors/JumpCloneValidationCloneCapacityReached` ("Infomorph
/// capacity reached", message 504893) **[V]** as a label; using it for "one
/// already installed here" as well as for the cap is **[A]**.
pub(crate) const CLONE_ERR_CAPACITY: &str =
    "UI/Medical/Errors/JumpCloneValidationCloneCapacityReached";

/// `ChangeHomeStationValidationError.ALREADY_SET_AS_HOME_STATION`,
/// `homestation/validation.py` **[V]**: the error codes in a candidate's
/// `errors` are these small ints, mapped to labels by
/// `text.describe_change_home_station_validation_error`.
const HOME_ERR_ALREADY_SET: i64 = 2;
/// `INVALID_CANDIDATE` **[V]** (`homestation/validation.py`): a structure
/// without a cloning bay for the pilot.
const HOME_ERR_INVALID_CANDIDATE: i64 = 5;
use crate::structure_profile::{SERVICE_JUMP_CLONE, SERVICE_MEDICAL};

/// A station, as much of it as a character needs to be *put* there.
///
/// The seven `Character` fields that name where a docked character is
/// (`model.rs`, all taken from the client's own `npcStations` and starmap
/// tables), so that moving a character is copying one of these onto it.
/// Kept whole rather than as a bare id because this server has no station
/// table to look the rest up in -- every station a player can stand in is
/// one some character was seeded at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StationRef {
    pub station_id: i64,
    pub owner_id: i64,
    pub type_id: i64,
    pub operation_id: i64,
    pub solar_system_id: i64,
    pub constellation_id: i64,
    pub region_id: i64,
}

impl StationRef {
    /// The station this character is attached to. `None` only for a
    /// character with no station at all, which the seed never makes.
    pub(crate) fn of(c: &Character) -> Option<Self> {
        Some(StationRef {
            station_id: c.station_id?,
            owner_id: c.station_owner_id,
            type_id: c.station_type_id,
            operation_id: c.station_operation_id,
            solar_system_id: c.solar_system_id?,
            constellation_id: c.constellation_id,
            region_id: c.region_id,
        })
    }

    /// Make this the character's station.
    pub(crate) fn seat(&self, c: &mut Character) {
        c.station_id = Some(self.station_id);
        c.station_owner_id = self.owner_id;
        c.station_type_id = self.type_id;
        c.station_operation_id = self.operation_id;
        c.solar_system_id = Some(self.solar_system_id);
        c.constellation_id = self.constellation_id;
        c.region_id = self.region_id;
    }

    /// The seven fields, tab-separated, in declaration order.
    fn render(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.station_id,
            self.owner_id,
            self.type_id,
            self.operation_id,
            self.solar_system_id,
            self.constellation_id,
            self.region_id
        )
    }

    /// The inverse of [`StationRef::render`], from `f[at..at + 7]`.
    fn parse(f: &[&str], at: usize) -> Result<Self, String> {
        let num = |i: usize| -> Result<i64, String> {
            f.get(at + i)
                .and_then(|v| v.parse::<i64>().ok())
                .ok_or_else(|| format!("field {} is not an integer", at + i))
        };
        Ok(StationRef {
            station_id: num(0)?,
            owner_id: num(1)?,
            type_id: num(2)?,
            operation_id: num(3)?,
            solar_system_id: num(4)?,
            constellation_id: num(5)?,
            region_id: num(6)?,
        })
    }
}

/// One installed jump clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JumpClone {
    pub owner_id: i64,
    pub station: StationRef,
    /// The player's name for it; empty means the client's own default
    /// ("Clone XXXX" seeded from the id), `jumpClonesPanel.py`
    /// `JumpClone.name`: `return self._name or self.default_name` **[V]**.
    pub name: String,
}

/// Every character's home station, jump clones, and the capsule this module
/// last put them in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Clones {
    pub(crate) homes: std::collections::BTreeMap<i64, StationRef>,
    jump: std::collections::BTreeMap<i64, JumpClone>,
    /// The implant **types** each jump clone holds, by slot (IMPL). A
    /// clone is not an item here, so its implants are not either: the head's
    /// are items at `flagImplant` and a jump swaps the two sets
    /// ([`crate::implants::swap_head`]). Absent for a clone with none.
    clone_implants: std::collections::BTreeMap<i64, Vec<i64>>,
    /// `character -> (capsule item id, the station it was put in)`.
    ///
    /// **Why this is stored at all.** `Character.ship_id` is not in the world
    /// file; `fitting::restore_active_ships` re-derives it after a load
    /// starting from the *seeded* id, which is the config's capsule. That is
    /// right until a revive destroys that capsule and mints another --
    /// after which a restart would hand the client a `shipid` naming an
    /// item that no longer exists. [`restore_capsules`] puts the minted id
    /// (and its station) back before that derivation runs.
    capsules: std::collections::BTreeMap<i64, (i64, StationRef)>,
}

impl Clones {
    /// The record tags this type reads and writes in the world file. The
    /// same opaque shape as `Bookmarks` and `Contacts`: `store.rs` only
    /// routes these.
    pub(crate) const RECORDS: &'static [&'static str] =
        &["homestation", "jumpclone", "clonecapsule", "cloneimplant"];

    /// The home station this character set, if they set one.
    pub(crate) fn home(&self, character_id: i64) -> Option<StationRef> {
        self.homes.get(&character_id).copied()
    }

    /// This character's jump clones, in id order.
    pub(crate) fn of(&self, owner_id: i64) -> Vec<(i64, &JumpClone)> {
        self.jump
            .iter()
            .filter(|(_, c)| c.owner_id == owner_id)
            .map(|(id, c)| (*id, c))
            .collect()
    }

    /// The implant types in jump clone `clone_id`, by slot.
    pub(crate) fn implants_in(&self, clone_id: i64) -> &[i64] {
        self.clone_implants
            .get(&clone_id)
            .map_or(&[], Vec::as_slice)
    }

    /// Set what jump clone `clone_id` holds; an empty set holds nothing.
    fn set_implants(&mut self, clone_id: i64, types: Vec<i64>) {
        if types.is_empty() {
            self.clone_implants.remove(&clone_id);
        } else {
            self.clone_implants.insert(clone_id, types);
        }
    }

    /// Forget a jump clone and what it held.
    fn remove_clone(&mut self, clone_id: i64) {
        self.jump.remove(&clone_id);
        self.clone_implants.remove(&clone_id);
    }

    /// One of this owner's jump clones.
    fn owned(&self, owner_id: i64, clone_id: i64) -> Option<&JumpClone> {
        self.jump.get(&clone_id).filter(|c| c.owner_id == owner_id)
    }

    /// Install a jump clone; returns its id. No checks: see
    /// [`install_errors`].
    pub(crate) fn install(&mut self, owner_id: i64, station: StationRef, name: String) -> i64 {
        let id = self
            .jump
            .keys()
            .next_back()
            .map_or(FIRST_JUMP_CLONE_ID, |last| last + 1);
        self.jump.insert(
            id,
            JumpClone {
                owner_id,
                station,
                name,
            },
        );
        id
    }

    /// Append this type's records to a world file being written.
    ///
    /// ```text
    /// homestation  <char> <station x7>
    /// jumpclone    <id> <owner> <station x7> <name>
    /// clonecapsule <char> <capsule> <station x7>
    /// cloneimplant <clone> <type>          one line per implant, in slot order
    /// ```
    ///
    /// The name is last so that it may be empty; tabs and newlines never
    /// reach it ([`clean_clone_name`]). Nothing at all for a world with no
    /// clones, so an older file reads back identical.
    pub(crate) fn render(&self, out: &mut String) {
        use std::fmt::Write as _;
        for (id, s) in &self.homes {
            let _ = writeln!(out, "homestation\t{id}\t{}", s.render());
        }
        for (id, c) in &self.jump {
            let _ = writeln!(
                out,
                "jumpclone\t{id}\t{}\t{}\t{}",
                c.owner_id,
                c.station.render(),
                c.name
            );
        }
        for (id, (pod, s)) in &self.capsules {
            let _ = writeln!(out, "clonecapsule\t{id}\t{pod}\t{}", s.render());
        }
        for (id, types) in &self.clone_implants {
            for t in types {
                let _ = writeln!(out, "cloneimplant\t{id}\t{t}");
            }
        }
    }

    /// Read one record whose tag is in [`Clones::RECORDS`]. Strict, like the
    /// bookmarks and contacts: a line we cannot read is a clone the player
    /// would silently lose, so the world is refused and the line named.
    pub(crate) fn parse_record(&mut self, f: &[&str]) -> Result<(), String> {
        let num = |i: usize| -> Result<i64, String> {
            f.get(i)
                .and_then(|v| v.parse::<i64>().ok())
                .ok_or_else(|| format!("field {i} is not an integer"))
        };
        match f.first().copied() {
            Some("homestation") => {
                self.homes.insert(num(1)?, StationRef::parse(f, 2)?);
            }
            Some("jumpclone") => {
                let clone = JumpClone {
                    owner_id: num(2)?,
                    station: StationRef::parse(f, 3)?,
                    name: f.get(10).copied().unwrap_or("").to_owned(),
                };
                self.jump.insert(num(1)?, clone);
            }
            Some("clonecapsule") => {
                self.capsules
                    .insert(num(1)?, (num(2)?, StationRef::parse(f, 3)?));
            }
            Some("cloneimplant") => {
                self.clone_implants
                    .entry(num(1)?)
                    .or_default()
                    .push(num(2)?);
            }
            other => return Err(format!("{other:?} is not a clones record")),
        }
        Ok(())
    }
}

/// A clone name as the world file can hold it: one line, no tabs, at most
/// the 100 characters the rename popup allows (`jumpClonesPanel.py`
/// `rename`: `NamePopup(..., maxLength=100)` **[V]**).
fn clean_clone_name(name: &str) -> String {
    name.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .take(100)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Put every revived character back in the capsule this module gave them,
/// after a world file loads. **Call before `fitting::restore_active_ships`**,
/// which derives the active ship from `Character.ship_id` -- see
/// [`Clones::capsules`]. A capsule that is gone or not theirs is skipped,
/// and the seed stands.
pub(crate) fn restore_capsules(reg: &mut Registry) {
    let seats: Vec<(i64, i64, StationRef)> = reg
        .clones
        .capsules
        .iter()
        .filter(|(ch, (pod, _))| {
            reg.store()
                .get(*pod)
                .is_some_and(|i| i.owner_id == **ch && i.group_id == GROUP_CAPSULE)
        })
        .map(|(ch, (pod, s))| (*ch, *pod, *s))
        .collect();
    for (character_id, pod, station) in seats {
        if let Some(c) = reg.character_mut(character_id) {
            station.seat(c);
            c.ship_id = pod;
            c.ship_type_id = TYPE_CAPSULE;
        }
    }
}

/// A station that is gone (an Upwell structure leaving space) is nobody's
/// home station any more; the revive falls back to where the pilot is based.
/// The client is told (`OnHomeStationMovedFromStructure`, which clears its
/// cache) and the jump clones installed there are destroyed **[A]**
/// (falsifier: the owner wants them moved, or kept for a re-anchor). The
/// capsule a pilot was last put in there is [`rehome_capsule`]'s.
pub(crate) fn forget_homes_at(reg: &mut Registry, station_id: i64) {
    let lost: Vec<i64> = reg
        .clones
        .homes
        .iter()
        .filter(|(_, s)| s.station_id == station_id)
        .map(|(who, _)| *who)
        .collect();
    reg.clones.homes.retain(|_, s| s.station_id != station_id);
    let clones: Vec<(i64, i64)> = reg
        .clones
        .jump
        .iter()
        .filter(|(_, c)| c.station.station_id == station_id)
        .map(|(id, c)| (*id, c.owner_id))
        .collect();
    for (id, _) in &clones {
        reg.clones.remove_clone(*id);
    }
    for who in lost {
        reg.push(HomeStationMovedFromStructure { character_id: who });
    }
    let mut owners: Vec<i64> = clones.iter().map(|(_, o)| *o).collect();
    owners.sort_unstable();
    owners.dedup();
    for character_id in owners {
        reg.push(JumpCloneCacheInvalidated { character_id });
    }
    reg.mark_dirty();
}

/// The capsule record of `who`, put at `station_id`, moves to `to` with the
/// item (an unanchoring harbours it), so a restart seats the pilot there.
pub(crate) fn rehome_capsule(reg: &mut Registry, who: i64, station_id: i64, to: StationRef) {
    if let Some((_, at)) = reg.clones.capsules.get_mut(&who)
        && at.station_id == station_id
    {
        *at = to;
        reg.mark_dirty();
    }
}

/// The jump clones installed in `station_id`, for the structure a pilot
/// controls or is docked in (`GetNumClonesInPilotsStructure`).
pub(crate) fn clones_at(reg: &Registry, station_id: i64) -> usize {
    reg.clones
        .jump
        .values()
        .filter(|c| c.station.station_id == station_id)
        .count()
}

/// The home station, and whether it is only the fallback.
///
/// The station the character set, else -- `is_fallback` true -- the station
/// they are attached to, which is what both readers answered before clones
/// existed. `None` only for an unknown character.
pub(crate) fn home_of(reg: &Registry, character_id: i64) -> Option<(StationRef, bool)> {
    if let Some(home) = reg.clones.home(character_id) {
        return Some((home, false));
    }
    StationRef::of(reg.character(character_id)?).map(|s| (s, true))
}

/// The station this character is docked in, or `None` in space.
///
/// `Character.station_id` is never cleared on undock, so the roster is the
/// only thing that knows; `space_state::Roster::is_in_space` says why.
fn docked_at(reg: &Registry, character_id: i64) -> Option<StationRef> {
    // `docked_station` rather than the bare roster: a pilot mid-jump is in
    // neither the roster nor a station (`docs/lanes/docked.md`).
    reg.docked_station(character_id)?;
    StationRef::of(reg.character(character_id)?)
}

/// Whether the character's active ship is a capsule.
fn in_capsule(reg: &Registry, character_id: i64) -> bool {
    reg.character(character_id)
        .is_some_and(|c| c.ship_type_id == TYPE_CAPSULE)
}

/// Our words, for a refusal the client has no label for.
fn clones_refusal(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", bstr(text))]),
    ))
}

fn clones_bad_args(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

fn int_arg(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(i) => Some(*i),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Pushes
// ---------------------------------------------------------------------------

/// `OnJumpCloneCacheInvalidated()` -- **no arguments** **[V]**:
/// `clonejumpsvc.py(336)` is `def OnJumpCloneCacheInvalidated(self)`
/// (`co_argcount` 1). It drops `jumpClones`, `jumpCloneImplants` and
/// `timeLastJump` and scatters `OnCloneJumpUpdate`, which the panel
/// controller turns into `on_update` (`jumpClonesPanel.py`).
///
/// **Required after every change to the list**: nothing on the client side
/// drops `jumpClones` after an install, a destroy or a rename -- the cache is
/// filled once (`GetCloneState`, `:90`) and only this or a session reset
/// clears it. Without it the panel shows the old list until relog.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JumpCloneCacheInvalidated {
    pub character_id: i64,
}

impl crate::notify::Push for JumpCloneCacheInvalidated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnJumpCloneCacheInvalidated", vec![])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character_id])
    }
}

/// `OnHomeStationMovedFromStructure()` -- no argument **[V]**
/// (`homestation/client/service.py(153)`): the home structure is gone; the
/// client drops its cached home station and reloads the pages.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HomeStationMovedFromStructure {
    pub character_id: i64,
}

impl crate::notify::Push for HomeStationMovedFromStructure {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnHomeStationMovedFromStructure", vec![])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character_id])
    }
}

/// `OnHomeStationChanged(new_home_station_id)` -- **one argument** **[V]**:
/// `homestation/client/service.py(218)`, `co_varnames` `(self,
/// new_home_station_id)`. It calls `clear_cache()` (home station, remote
/// change time, school HQ, and every candidate list's validation token) and
/// fires `on_home_station_changed`, which reloads both Home Station pages.
///
/// **Required**: `get_home_station` caches on `is UNCACHED` (`:89`) and
/// nothing else clears it, so without this the tab keeps showing the old
/// station until relog.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HomeStationChanged {
    pub character_id: i64,
    pub station_id: i64,
}

impl crate::notify::Push for HomeStationChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnHomeStationChanged", vec![PyValue::Int(self.station_id)])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character_id])
    }
}

// ---------------------------------------------------------------------------
// Moving a character into a fresh capsule
// ---------------------------------------------------------------------------

/// What [`revive_in_clone`] (and a self-destruct, and a remote clone jump)
/// did, and the session change that tells the client.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Revival {
    pub character_id: i64,
    /// Where they are now docked.
    pub station: StationRef,
    /// The station or structure they were docked in, `None` from space.
    pub from_station: Option<i64>,
    /// The `shipid` they had: the dead capsule, for a podding. The session
    /// change's `shipid` *old* value, and what `OnPodKilled` must name.
    pub old_ship_id: i64,
    /// The capsule they are in now, in the station's hangar.
    pub capsule_id: i64,
    /// `{attr: (old, new)}` for `SessionChangeNotification`, in the shape
    /// `Registry::queue_session_change` takes.
    pub change: Vec<(PyValue, PyValue)>,
}

/// Put a character, docked, into a new capsule at `to`.
///
/// One item plan: create a capsule in `to`'s hangar and destroy the
/// character's old capsule if it still exists (`fitting::capsule_of`, which
/// finds it whether it is the active ship or parked in one). A hull is never
/// touched: the death wiring owns hulls, and the docked callers refuse
/// unless the player is already in a capsule.
///
/// The session change, **[V]** in shape from `docs/re/damage-death.md` §5.2
/// and `Registry::dock_change`'s evidence:
///
/// - `shipid: (old, capsule)` always -- `gameui._ParseStateChange:506` labels
///   the change a podding only when `change['shipid'][0]` is the pod it was
///   told died, so the old value must be the id the client has;
/// - from space: `stationid: (None, station)` and `solarsystemid:
///   (system, None)`, exactly the dock's pair -- `solarsystemid` going absent
///   is what tears the ballpark down (`michelle.UpdateBallpark`);
/// - docked elsewhere: `stationid: (old, new)`;
/// - `solarsystemid2`, `constellationid`, `regionid` only when they differ,
///   because a pair whose new value equals the client's is dropped anyway.
///
/// `locationid` is never sent: the client derives it
/// (`RecalculateDependantAttributesWithChanges`).
fn move_into_fresh_capsule(
    reg: &mut Registry,
    character_id: i64,
    to: StationRef,
    lose_implants: bool,
) -> Option<Revival> {
    let before = reg.character(character_id)?.clone();
    let space_system = reg.roster().get(character_id).map(|p| p.solar_system_id);
    let mut plan = vec![crate::item::Edit::Create {
        type_id: TYPE_CAPSULE,
        quantity: 1,
        to: crate::item::Slot::new(character_id, to.station_id, crate::model::FLAG_HANGAR),
    }];
    if let Some(old) = crate::fitting::capsule_of(reg.store(), &before) {
        plan.push(crate::item::Edit::Destroy {
            item_id: old.item_id,
        });
    }
    // A podding takes the head's **destructible** implants with it (the
    // client's own test, `prompt._is_destructible`, **[V]**): they are items
    // at `flagImplant` in the character, destroyed by the same plan so the
    // client sees them go as ordinary item changes. A `nonDestructible` one
    // stays in the head of the clone they wake in.
    let lost = if lose_implants {
        crate::implants::destroyed_in_a_pod(reg, character_id)
    } else {
        Vec::new()
    };
    plan.extend(
        lost.iter()
            .map(|&item_id| crate::item::Edit::Destroy { item_id }),
    );
    // The training rate moves with the implants: credit the head skill at
    // the old rate first, restart it at the new one after.
    let frozen = (!lost.is_empty()).then(|| crate::skill_queue::freeze_training(reg, character_id));
    let applied = reg.apply_items(&plan);
    if let Some(f) = frozen {
        crate::skill_queue::thaw_training(reg, character_id, f);
    }
    let changes = match applied {
        Ok(c) => c,
        Err(e) => {
            crate::outln!("[clones] capsule plan refused for {character_id}: {e}");
            return None;
        }
    };
    if !lost.is_empty() {
        crate::implants::announce(reg, character_id);
    }
    if lose_implants {
        crate::boosters::on_podding(reg, character_id);
    }
    let capsule_id = changes.iter().find_map(|c| match c {
        crate::item::Change::Created { item_id, .. } => Some(*item_id),
        _ => None,
    })?;
    // Defensive: the death wiring takes the character out of space (and
    // releases locks) before it calls us. If it has not, leaving them in the
    // roster would draw a ball for a docked character to every observer.
    if space_system.is_some() {
        reg.roster_mut().leave(character_id);
    }
    if let Some(c) = reg.character_mut(character_id) {
        to.seat(c);
        c.ship_id = capsule_id;
        c.ship_type_id = TYPE_CAPSULE;
    }
    reg.clones.capsules.insert(character_id, (capsule_id, to));
    reg.mark_dirty();

    let pair = |old: PyValue, new: PyValue| PyValue::Tuple(vec![old, new]);
    let int = PyValue::Int;
    let opt = |v: Option<i64>| v.map_or(PyValue::None, PyValue::Int);
    let mut kv = vec![("shipid", pair(int(before.ship_id), int(capsule_id)))];
    let from = StationRef::of(&before);
    // The three session keys that say where a pilot is: in space,
    // `solarsystemid`; in a station, `stationid` (and no `solarsystemid`); in
    // a structure, `structureid` and `solarsystemid` **[V]** (U4,
    // `eveSessions.GetSessionValuesFromItemID`).
    let from_structure = from
        .map(|f| f.station_id)
        .filter(|id| space_system.is_none() && crate::structure_dock::is_structure(reg, *id));
    let to_structure = crate::structure_dock::is_structure(reg, to.station_id);
    let old_station = from
        .map(|f| f.station_id)
        .filter(|_| space_system.is_none() && from_structure.is_none());
    let new_station = (!to_structure).then_some(to.station_id);
    let old_solar = match (space_system, from_structure) {
        (Some(system), _) => Some(system),
        (None, Some(_)) => from.map(|f| f.solar_system_id),
        (None, None) => None,
    };
    let new_solar = to_structure.then_some(to.solar_system_id);
    if old_station != new_station {
        kv.push(("stationid", pair(opt(old_station), opt(new_station))));
    }
    if from_structure != to_structure.then_some(to.station_id) {
        kv.push((
            "structureid",
            pair(
                opt(from_structure),
                opt(to_structure.then_some(to.station_id)),
            ),
        ));
    }
    if old_solar != new_solar {
        kv.push(("solarsystemid", pair(opt(old_solar), opt(new_solar))));
    }
    let old_system2 = space_system.or(from.map(|f| f.solar_system_id));
    if old_system2 != Some(to.solar_system_id) {
        kv.push((
            "solarsystemid2",
            pair(opt(old_system2), int(to.solar_system_id)),
        ));
    }
    if before.constellation_id != to.constellation_id {
        kv.push((
            "constellationid",
            pair(int(before.constellation_id), int(to.constellation_id)),
        ));
    }
    if before.region_id != to.region_id {
        kv.push(("regionid", pair(int(before.region_id), int(to.region_id))));
    }
    Some(Revival {
        character_id,
        station: to,
        from_station: space_system
            .is_none()
            .then(|| from.map(|f| f.station_id))
            .flatten(),
        old_ship_id: before.ship_id,
        capsule_id,
        change: crate::pyutil::pypairs(kv),
    })
}

/// **The death wiring's entry point**: revive a podded character in their
/// medical clone at the home station.
///
/// Does the server-side state change -- a new capsule in the home station's
/// hangar, the dead one destroyed if it is still in the store, the character
/// docked there, the roster left, the capsule remembered for the world file
/// -- and returns the session change **without queuing it**, because the
/// caller runs on the clock and `Registry::queue_session_change` only drains
/// after a payload turn. `docs/lanes/clones.md` §3 has the exact sequence
/// P2.2b must send around it (`OnPodKilled`, `OnPlayerPodDeath`, then this
/// change).
///
/// **Preconditions**: `Character.ship_id` is the pod that died (the id the
/// client holds as `session.shipid`, i.e. after `ShipIdToCapsule`), and the
/// caller has already done the kill's space work (`targeting::left_space`,
/// the `RemoveBall`). `None` only for an unknown character or a store that
/// refuses the plan, which would be our bug.
///
/// `allow(dead_code)` only until P2.2b's `combat::kill` calls it -- remove
/// the attribute in that commit.
pub(crate) fn revive_in_clone(reg: &mut Registry, character_id: i64) -> Option<Revival> {
    let home = revive_place(reg, character_id)?;
    move_into_fresh_capsule(reg, character_id, home, true)
}

/// Where a revive (or a self-destruct) puts `who`: the home station, unless
/// it is a structure that no longer takes them (offline, gone, or the access
/// withdrawn); then the place they are based, then an NPC station of the
/// home's system. **[A]** that a pilot wakes in the next best place and not
/// in an error; falsifier: the owner wants the clone held in the structure's
/// reinforcement. With nowhere better the home is used anyway.
pub(crate) fn revive_place(reg: &Registry, who: i64) -> Option<StationRef> {
    let (home, _) = home_of(reg, who)?;
    if crate::structure_dock::usable_by(reg, who, &home) {
        return Some(home);
    }
    crate::outln!(
        "[clones] {who}: home {} does not take them; waking elsewhere",
        home.station_id
    );
    let based = reg
        .character(who)
        .and_then(StationRef::of)
        .filter(|r| crate::structure_dock::usable_by(reg, who, r));
    based
        .or_else(|| {
            crate::structure_space::station_in(reg, home.solar_system_id)
                .and_then(|id| reg.station_ref(id))
        })
        .or(Some(home))
}

/// The item ids of the implants plugged into `character_id`'s head: the
/// character's items at `flagImplant` (89) **[V]** (`dogma::FLAG_IMPLANT`).
/// Tests only: the game's own readers are [`crate::implants::plugged`] and
/// [`crate::implants::destroyed_in_a_pod`].
#[cfg(test)]
pub(crate) fn implants_of(reg: &Registry, character_id: i64) -> Vec<i64> {
    reg.store()
        .contents_of(character_id)
        .filter(|i| i.flag_id == crate::dogma::FLAG_IMPLANT)
        .map(|i| i.item_id)
        .collect()
}

// ---------------------------------------------------------------------------
// The home station arms
// ---------------------------------------------------------------------------

/// `home_station.get_home_station_candidates()`: a list of objects with
/// **`id`, `type_id`, `solar_system_id`, `is_current_station`,
/// `is_school_hq`, `errors`** -- the six attributes
/// `homestation/client/service.py(110)` copies into each `StationCandidate`
/// (`:126-140`) **[V]**. `errors` is a list of
/// `ChangeHomeStationValidationError` codes; non-empty greys the Set button
/// with the reasons as its tooltip (`select.py` `SelectHomeStationButton`).
///
/// One candidate, the station the character is docked in, and none in space.
/// **[A]** on what is left out: TQ also offers the school HQ and corporation
/// office stations; we have no offices, and the school HQ would rest on
/// `GetPublicInfo`'s `schoolID`, which is a valid key but an `[A]` choice
/// (see that arm). A
/// candidate with `is_current_station` true is not "remote"
/// (`homestation/validation.py` `is_remote`), so choosing it never asks
/// `allow_remote`.
///
/// Empty is **not** a lie in space: the client draws its own "no candidates"
/// hint, and a remote change is not something this server accepts.
pub(crate) fn home_station_candidates(reg: &Registry, actor: Option<i64>) -> PyValue {
    let Some((actor, here)) = actor.and_then(|a| docked_at(reg, a).map(|s| (a, s))) else {
        return PyValue::List(vec![]);
    };
    let errors = if reg.clones.home(actor) == Some(here) {
        vec![PyValue::Int(HOME_ERR_ALREADY_SET)]
    } else if crate::structure_dock::service_refusal(reg, actor, SERVICE_MEDICAL).is_some() {
        // The structure has no cloning bay for them: not a candidate.
        vec![PyValue::Int(HOME_ERR_INVALID_CANDIDATE)]
    } else {
        vec![]
    };
    PyValue::List(vec![keyval(vec![
        ("id", PyValue::Int(here.station_id)),
        ("type_id", PyValue::Int(here.type_id)),
        ("solar_system_id", PyValue::Int(here.solar_system_id)),
        ("is_current_station", PyValue::Bool(true)),
        ("is_school_hq", PyValue::Bool(false)),
        ("errors", PyValue::List(errors)),
    ])])
}

/// `home_station.set_home_station(new_home_station_id, allow_remote)`.
///
/// Sets the docked-in station as home and pushes [`HomeStationChanged`].
/// Anything else is refused in our words: the client's own refusals are a
/// `ChangeHomeStationValidationError` (a `UserError` subclass with an
/// `errors` list, `homestation/validation.py`) and a bare
/// `RemoteChangeNotExpectedError` (`homestation/error.py`), and we have no
/// way to raise either class -- `eve_proto::user_error` builds a plain
/// `UserError`. **[A]** that a `CustomNotify` is an acceptable stand-in; the
/// candidates list never offers anything that reaches it.
pub(crate) fn set_home_station(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(clones_refusal("No character is selected."));
    };
    let Some(station_id) = int_arg(req.args.first()) else {
        return Err(clones_bad_args(req, "(new_home_station_id, allow_remote)"));
    };
    let Some(here) = docked_at(reg, actor) else {
        return Err(clones_refusal(
            "Dock in a station to make it your home station.",
        ));
    };
    if here.station_id != station_id {
        return Err(clones_refusal(
            "This server only sets the station you are docked in as your home station.",
        ));
    }
    if reg.clones.home(actor) == Some(here) {
        return Ok(PyValue::None);
    }
    if let Some(why) = crate::structure_dock::service_refusal(reg, actor, SERVICE_MEDICAL) {
        return Err(clones_refusal(why));
    }
    reg.clones.homes.insert(actor, here);
    reg.mark_dirty();
    reg.push(HomeStationChanged {
        character_id: actor,
        station_id,
    });
    Ok(PyValue::None)
}

/// `home_station.self_destruct_clone()`, run *as* the session-change
/// function (`service.py(160)`), so the session change is the answer.
///
/// The client's own validator (`homestation/validation.py(63)`) only enables
/// the button when docked, in a capsule, and **not** already in the home
/// station; the same three are re-checked here, because a check the client
/// runs is not a check. Then: the same move a podding makes
/// ([`move_into_fresh_capsule`]), queued on the caller.
///
/// In a world where every character is seeded in one station, home is always
/// where they are, so this is unreachable until a second station is
/// dockable; it costs nothing because it is the revive.
pub(crate) fn self_destruct_clone(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(clones_refusal("No character is selected."));
    };
    let Some(here) = docked_at(reg, actor) else {
        return Err(clones_refusal(
            "You must be docked to self-destruct your clone.",
        ));
    };
    if !in_capsule(reg, actor) {
        return Err(clones_refusal(
            "Leave your ship first: only a capsule can self-destruct.",
        ));
    }
    let Some(home) = revive_place(reg, actor) else {
        return Err(clones_refusal("No home station."));
    };
    if home.station_id == here.station_id {
        return Err(clones_refusal("You are already in your home station."));
    }
    let Some(moved) = move_into_fresh_capsule(reg, actor, home, true) else {
        return Err(clones_refusal("The server could not move your clone."));
    };
    reg.queue_session_change(actor, moved.change);
    crate::structure_dock::clone_moved(
        reg,
        actor,
        moved.from_station,
        Some(moved.station.station_id),
    );
    Ok(PyValue::None)
}

// ---------------------------------------------------------------------------
// The jump clone arms
// ---------------------------------------------------------------------------

/// The jump-clone row layout. The same four columns as the parent's
/// `clones_rowset` (which serves the no-character case through
/// `clone_state`), and the columns are the contract there.
fn jump_clone_descriptor() -> DbRowDescriptor {
    DbRowDescriptor::from_pairs(&[
        ("jumpCloneID", DbType::I4),
        ("typeID", DbType::I4),
        ("locationID", DbType::I8),
        ("cloneName", DbType::WStr),
    ])
    .expect("the clone columns are a valid layout")
}

/// `jumpCloneSvc.GetCloneState()`: `KeyVal(clones, implants, timeLastJump)`.
///
/// `clones` is a `CRowset` of this character's jump clones. The readers
/// **[V]**: `jumpClonesPanel.py(593) clones` reads `jumpCloneID`,
/// `locationID` and `cloneName` off each row and nothing else; `clonejumpsvc
/// .py(143)` does `.Index('jumpCloneID')` and reads `.locationID`. So
/// `typeID` is sent NULL: no reader, and no clone grade to name.
///
/// `implants` is one `(jumpCloneID, typeID)` row per implant each clone
/// holds ([`clone_implants_value`]), and **`timeLastJump` stays `None`**: a value there makes
/// `jump_cooldown_duration` (`jumpClonesPanel.py(687)`) read the
/// character's godma `cloneJumpCoolDown`, which we do not serve, and that
/// panel's constructor runs it -- the latch that kills the whole character
/// sheet (see the `jumpCloneSvc` arm). **[A]** so: no jump cooldown.
pub(crate) fn clone_state_value(reg: &Registry, actor: Option<i64>) -> PyValue {
    let Some(actor) = actor else {
        return clone_state();
    };
    let mut rows = eve_rowset::CRowset::new(jump_clone_descriptor());
    for (id, c) in reg.clones.of(actor) {
        let name = if c.name.is_empty() {
            DbValue::Null
        } else {
            DbValue::Object(PyValue::Unicode(c.name.clone()))
        };
        rows.push_values(vec![
            DbValue::Int(id),
            DbValue::Null,
            DbValue::Int(c.station.station_id),
            name,
        ])
        .expect("four values for the four columns declared above");
    }
    keyval(vec![
        ("clones", rows.to_pyvalue()),
        ("implants", clone_implants_value(reg, actor)),
        ("timeLastJump", PyValue::None),
    ])
}

/// The `implants` rowset of `GetCloneState`: one `(jumpCloneID, typeID)` row
/// per implant in each of this character's jump clones **[V]**:
/// `clonejumpsvc.py:261 GetImplantsForClone` does `Filter('jumpCloneID')` on
/// it and the panel reads `.typeID` of each row (`jumpClonesPanel.py:415`).
fn clone_implants_value(reg: &Registry, actor: i64) -> PyValue {
    let mut rows = eve_rowset::CRowset::new(
        DbRowDescriptor::from_pairs(&[("jumpCloneID", DbType::I4), ("typeID", DbType::I4)])
            .expect("the implant columns are a valid layout"),
    );
    for (id, _) in reg.clones.of(actor) {
        for &type_id in reg.clones.implants_in(id) {
            rows.push_values(vec![DbValue::Int(id), DbValue::Int(type_id)])
                .expect("two values for the two columns declared above");
        }
    }
    rows.to_pyvalue()
}

/// Why this character cannot install a jump clone where they are, as the
/// client's own `UI/Medical/Errors/` labels; empty when they can.
///
/// `clonejumpsvc.ValidateInstallJumpClone` renders each with `GetByLabel`
/// and the Install button is disabled when any are present
/// (`InstallCloneBanner.update`) **[V]**. The rules are **[A]**: docked; at
/// most [`MAX_JUMP_CLONES`]; and at most one per station (kept from when
/// two clones in one station could not differ; TQ allows several per
/// station, and nothing here needs them).
pub(crate) fn install_errors(reg: &Registry, character_id: i64) -> Vec<&'static str> {
    let Some(here) = docked_at(reg, character_id) else {
        return vec![CLONE_ERR_NOT_DOCKED];
    };
    if crate::structure_dock::service_refusal(reg, character_id, SERVICE_JUMP_CLONE).is_some() {
        // **[A]** the client has no label for "this structure has no bay";
        // the nearest is the not-docked one. Falsifier: the tooltip reads
        // wrong beside the Install button.
        return vec![CLONE_ERR_NOT_DOCKED];
    }
    let mine = reg.clones.of(character_id);
    if mine.len() >= MAX_JUMP_CLONES
        || mine
            .iter()
            .any(|(_, c)| c.station.station_id == here.station_id)
    {
        return vec![CLONE_ERR_CAPACITY];
    }
    vec![]
}

/// `InstallCloneInStation()`: a jump clone in the docked-in station.
///
/// Free (`GetPriceForClone` answers 0.0, so the client never asks to pay).
/// Re-runs [`install_errors`], because the client only checked on its last
/// panel refresh.
pub(crate) fn install_jump_clone(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(clones_refusal("No character is selected."));
    };
    if !install_errors(reg, actor).is_empty() {
        return Err(clones_refusal(
            "A jump clone cannot be installed here: you must be docked, with no \
             clone in this station and fewer than the maximum installed.",
        ));
    }
    let Some(here) = docked_at(reg, actor) else {
        return Err(clones_refusal("You must be docked."));
    };
    reg.clones.install(actor, here, String::new());
    reg.mark_dirty();
    reg.push(JumpCloneCacheInvalidated {
        character_id: actor,
    });
    Ok(PyValue::None)
}

/// `DestroyInstalledClone(cloneID)`. The client offers it only for clones it
/// lists, so an id that is not this character's is refused rather than
/// ignored.
pub(crate) fn destroy_jump_clone(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(clones_refusal("No character is selected."));
    };
    let Some(id) = int_arg(req.args.first()) else {
        return Err(clones_bad_args(req, "(cloneID,)"));
    };
    if reg.clones.owned(actor, id).is_none() {
        return Err(clones_refusal("That jump clone is not yours."));
    }
    // What the clone held goes with it.
    reg.clones.remove_clone(id);
    reg.mark_dirty();
    reg.push(JumpCloneCacheInvalidated {
        character_id: actor,
    });
    Ok(PyValue::None)
}

/// `SetJumpCloneName(cloneID, newName)`.
pub(crate) fn rename_jump_clone(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(clones_refusal("No character is selected."));
    };
    let (Some(id), Some(name)) = (
        int_arg(req.args.first()),
        req.args.get(1).and_then(|v| match v {
            PyValue::Unicode(s) => Some(s.clone()),
            PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        }),
    ) else {
        return Err(clones_bad_args(req, "(cloneID, newName)"));
    };
    let name = clean_clone_name(&name);
    let Some(clone) = reg.clones.jump.get_mut(&id).filter(|c| c.owner_id == actor) else {
        return Err(clones_refusal("That jump clone is not yours."));
    };
    clone.name = name;
    reg.mark_dirty();
    reg.push(JumpCloneCacheInvalidated {
        character_id: actor,
    });
    Ok(PyValue::None)
}

/// `CloneJump(destLocationID, cloneID, cost, confirmed)`, run as the
/// session-change function (`clonejumpsvc.py(263)`).
///
/// - **A clone in this station**: a swap in place. The body you leave stays
///   here as the jump clone and you wake in the other, which differ only in
///   their implants, so the two **sets are swapped**
///   ([`crate::implants::swap_head`]): the clone keeps the set you wore, the
///   head gets the set it held. The panel re-reads, and the client never
///   checks the cooldown for a same-station jump (`validate_jump_to`:
///   `should_check_timer = not clone.in_current_location`, **[V]**).
/// - **A clone elsewhere**: the character must already be in a capsule (the
///   client unboards first, `UnboardShipAndCloneJump_thread`); they wake at
///   the clone's station in a fresh capsule ([`move_into_fresh_capsule`]),
///   that clone is used up, and **[I]** the body they left becomes a jump
///   clone where it stood. **The clone they wake in brings its own
///   implants and the clone they leave keeps the ones they wore.** The
///   session change is queued on the caller.
pub(crate) fn clone_jump(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(clones_refusal("No character is selected."));
    };
    let (Some(dest), Some(id)) = (int_arg(req.args.first()), int_arg(req.args.get(1))) else {
        return Err(clones_bad_args(
            req,
            "(destLocationID, cloneID, cost, confirmed)",
        ));
    };
    let Some(here) = docked_at(reg, actor) else {
        return Err(clones_refusal("You must be docked to clone jump."));
    };
    let Some(target) = reg
        .clones
        .owned(actor, id)
        .filter(|c| c.station.station_id == dest)
        .cloned()
    else {
        return Err(clones_refusal(
            "That jump clone is not yours, or not there.",
        ));
    };
    // The set the clone holds, and the set the body leaves behind. A type
    // the store no longer knows would make the swap fail after the move, so
    // it is checked first.
    if target.station.station_id != here.station_id
        && !crate::structure_dock::usable_by(reg, actor, &target.station)
    {
        return Err(clones_refusal(
            "That clone is in a structure that no longer lets you dock.",
        ));
    }
    let wake: Vec<i64> = reg.clones.implants_in(id).to_vec();
    if wake
        .iter()
        .any(|t| crate::item::find_in(reg.content(), *t).is_none())
    {
        return Err(clones_refusal(
            "That jump clone holds an implant this server cannot make.",
        ));
    }
    let left_behind = crate::implants::head_types(reg, actor);
    if target.station.station_id == here.station_id {
        // A swap in place: the body you leave stays here as the jump clone
        // and you wake in the other, which differ only in their implants.
        crate::boosters::on_jump(reg, actor);
        if let Err(e) = crate::implants::swap_head(reg, actor, &wake) {
            return Err(clones_refusal(&format!(
                "The implants could not be moved: {e}"
            )));
        }
        reg.clones.set_implants(id, left_behind);
        reg.mark_dirty();
        reg.push(JumpCloneCacheInvalidated {
            character_id: actor,
        });
        return Ok(PyValue::None);
    }
    if !in_capsule(reg, actor) {
        return Err(clones_refusal(
            "Leave your ship first: only a capsule can clone jump.",
        ));
    }
    let Some(moved) = move_into_fresh_capsule(reg, actor, target.station, false) else {
        return Err(clones_refusal("The server could not move your clone."));
    };
    // The clone you wake in brings its own implants; the body you leave
    // becomes a jump clone where it stood, with the implants it wore.
    crate::boosters::on_jump(reg, actor);
    if let Err(e) = crate::implants::swap_head(reg, actor, &wake) {
        crate::outln!("[clones] implant swap refused for {actor} after the move: {e}");
    }
    reg.clones.remove_clone(id);
    let left = reg.clones.install(actor, here, String::new());
    reg.clones.set_implants(left, left_behind);
    reg.mark_dirty();
    reg.queue_session_change(actor, moved.change);
    crate::structure_dock::clone_moved(
        reg,
        actor,
        moved.from_station,
        Some(moved.station.station_id),
    );
    reg.push(JumpCloneCacheInvalidated {
        character_id: actor,
    });
    Ok(PyValue::None)
}

#[cfg(test)]
mod clones_tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};

    /// The tests were written inside `service/character.rs` and called its
    /// `dispatch`; `Registry::dispatch` routes these services to that one
    /// function (no service is claimed by two modules), so this is the same
    /// call made from outside the private `service::character` module.
    fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
        reg.dispatch(req)
    }

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;
    const POD: i64 = 100_000_001;

    fn two() -> Registry {
        let alice = Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: POD,
            ..Character::default()
        };
        let bob = Character {
            character_id: BOB,
            name: "Bob".to_owned(),
            ship_id: 100_000_002,
            ..Character::default()
        };
        Registry::new(SandboxConfig {
            characters: vec![alice, bob],
            ..SandboxConfig::default()
        })
    }

    fn call(
        reg: &mut Registry,
        actor: i64,
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
                actor: Some(actor),
            },
        )
    }

    fn attr<'a>(v: &'a PyValue, name: &str) -> &'a PyValue {
        let PyValue::Instance { state, .. } = v else {
            panic!("expected a KeyVal, got {v:?}")
        };
        crate::pyutil::dict_get(state, name.as_bytes())
            .unwrap_or_else(|| panic!("no .{name} on {v:?}"))
    }

    /// `(event, args, audience)` of one queued push.
    type Seen = (&'static str, Vec<PyValue>, Option<Vec<i64>>);

    /// Every push queued, oldest first.
    fn pushes(reg: &mut Registry) -> Vec<Seen> {
        reg.take_pending()
            .into_iter()
            .map(|(_, p)| {
                let (name, args) = p.to_push();
                (name, args, p.audience())
            })
            .collect()
    }

    /// The rows of the `clones` rowset, as `(jumpCloneID, locationID)`.
    fn clone_rows(reg: &mut Registry, actor: i64) -> Vec<(i64, i64)> {
        let state = call(reg, actor, "jumpCloneSvc", "GetCloneState", &[]).expect("answered");
        let PyValue::ObjectEx { header, .. } = attr(&state, "clones") else {
            panic!("clones must be a CRowset")
        };
        let PyValue::Tuple(head) = header.as_ref() else {
            panic!("a CRowset header is (class, args)")
        };
        let PyValue::Tuple(args) = &head[1] else {
            panic!("CRowset args")
        };
        let PyValue::List(rows) = &args[1] else {
            panic!("CRowset rows")
        };
        rows.iter()
            .map(|r| {
                let PyValue::DbRow(row) = r else {
                    panic!("a row is a DBRow, got {r:?}")
                };
                let int = |i: usize| match row.values()[i] {
                    DbValue::Int(v) => v,
                    ref other => panic!("column {i} is not an int: {other:?}"),
                };
                (int(0), int(2))
            })
            .collect()
    }

    fn elsewhere() -> StationRef {
        StationRef {
            station_id: 60_000_004,
            owner_id: 1_000_002,
            type_id: 1_529,
            operation_id: 26,
            solar_system_id: 30_002_780,
            constellation_id: 20_000_406,
            region_id: 10_000_033,
        }
    }

    /// `get_home_station` is read for four attributes with no guard
    /// (`service.py(96-101)`), and `is_fallback` is what tells the panel the
    /// home was never chosen.
    #[test]
    fn the_home_station_is_the_fallback_until_one_is_set() {
        let mut reg = two();
        let got = call(&mut reg, ALICE, "home_station", "get_home_station", &[]).unwrap();
        for name in ["id", "type_id", "solar_system_id", "is_fallback"] {
            attr(&got, name);
        }
        assert_eq!(attr(&got, "is_fallback"), &PyValue::Bool(true));
        assert_eq!(attr(&got, "id"), &PyValue::Int(60_012_526));
    }

    /// Setting it: the answer flips `is_fallback`, both home readers agree,
    /// and the one push the client needs carries the station id as its only
    /// argument (`OnHomeStationChanged(self, new_home_station_id)`).
    #[test]
    fn setting_the_home_station_pushes_its_id_and_both_readers_agree() {
        let mut reg = two();
        reg.take_dirty();
        call(
            &mut reg,
            ALICE,
            "home_station",
            "set_home_station",
            &[PyValue::Int(60_012_526), PyValue::Bool(false)],
        )
        .expect("set");
        assert!(reg.take_dirty());
        assert_eq!(
            pushes(&mut reg),
            vec![(
                "OnHomeStationChanged",
                vec![PyValue::Int(60_012_526)],
                Some(vec![ALICE])
            )]
        );
        let got = call(&mut reg, ALICE, "home_station", "get_home_station", &[]).unwrap();
        assert_eq!(attr(&got, "is_fallback"), &PyValue::Bool(false));
        let row = call(&mut reg, ALICE, "charMgr", "GetHomeStationRow", &[]).unwrap();
        assert_eq!(attr(&row, "stationID"), attr(&got, "id"));
        assert_eq!(attr(&row, "solarSystemID"), attr(&got, "solar_system_id"));
        assert_eq!(attr(&row, "stationTypeID"), attr(&got, "type_id"));
    }

    /// **In space there is no "here"**: no home-station candidate, and
    /// setting one is refused and changes nothing. The docked half is
    /// `the_docked_station_is_a_candidate_until_it_is_home`. Reads
    /// `Registry::docked_station`, because `Character::station_id` stays
    /// set in space (`docs/lanes/docked.md`).
    #[test]
    fn in_space_no_station_can_be_made_home() {
        let mut reg = two();
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        assert_eq!(reg.docked_station(ALICE), None);
        let got = call(
            &mut reg,
            ALICE,
            "home_station",
            "get_home_station_candidates",
            &[],
        )
        .unwrap();
        assert_eq!(got, PyValue::List(vec![]));
        let set = call(
            &mut reg,
            ALICE,
            "home_station",
            "set_home_station",
            &[PyValue::Int(60_012_526), PyValue::Bool(false)],
        );
        assert!(matches!(set, Err(ServiceError::Raise(_))), "{set:?}");
        assert_eq!(
            home_of(&reg, ALICE).map(|(_, fallback)| fallback),
            Some(true)
        );
    }

    /// A candidate carries the six attributes `get_home_station_candidates`
    /// copies (`service.py(126-140)`); once the station is home, its
    /// `errors` holds `ALREADY_SET_AS_HOME_STATION` (2), which greys the
    /// button.
    #[test]
    fn the_docked_station_is_a_candidate_until_it_is_home() {
        let mut reg = two();
        let got = call(
            &mut reg,
            ALICE,
            "home_station",
            "get_home_station_candidates",
            &[],
        )
        .unwrap();
        let PyValue::List(list) = &got else {
            panic!("a list, iterated")
        };
        assert_eq!(list.len(), 1);
        for name in [
            "id",
            "type_id",
            "solar_system_id",
            "is_current_station",
            "is_school_hq",
            "errors",
        ] {
            attr(&list[0], name);
        }
        assert_eq!(attr(&list[0], "is_current_station"), &PyValue::Bool(true));
        assert_eq!(attr(&list[0], "errors"), &PyValue::List(vec![]));
        call(
            &mut reg,
            ALICE,
            "home_station",
            "set_home_station",
            &[PyValue::Int(60_012_526), PyValue::Bool(false)],
        )
        .unwrap();
        let got = call(
            &mut reg,
            ALICE,
            "home_station",
            "get_home_station_candidates",
            &[],
        )
        .unwrap();
        let PyValue::List(list) = &got else {
            panic!("a list")
        };
        assert_eq!(
            attr(&list[0], "errors"),
            &PyValue::List(vec![PyValue::Int(HOME_ERR_ALREADY_SET)])
        );
    }

    /// Install, list, rename, destroy -- each change pushes the zero-argument
    /// invalidation (`OnJumpCloneCacheInvalidated(self)`), because nothing
    /// else drops the client's cached list. Installing touches only the
    /// installer's list.
    #[test]
    fn a_jump_clone_is_installed_listed_renamed_and_destroyed() {
        let mut reg = two();
        let ok = call(
            &mut reg,
            ALICE,
            "jumpCloneSvc",
            "ValidateInstallJumpClone",
            &[],
        )
        .unwrap();
        assert_eq!(ok, PyValue::List(vec![]), "nothing installed yet");
        call(
            &mut reg,
            ALICE,
            "jumpCloneSvc",
            "InstallCloneInStation",
            &[],
        )
        .unwrap();
        let invalidate = ("OnJumpCloneCacheInvalidated", vec![], Some(vec![ALICE]));
        assert_eq!(pushes(&mut reg), vec![invalidate.clone()]);
        let rows = clone_rows(&mut reg, ALICE);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, 60_012_526, "locationID is the station");
        assert!(clone_rows(&mut reg, BOB).is_empty());

        let id = rows[0].0;
        call(
            &mut reg,
            ALICE,
            "jumpCloneSvc",
            "SetJumpCloneName",
            &[PyValue::Int(id), PyValue::Unicode("Pvp\tbody".into())],
        )
        .unwrap();
        assert_eq!(pushes(&mut reg), vec![invalidate.clone()]);
        assert_eq!(reg.clones.owned(ALICE, id).unwrap().name, "Pvp body");

        assert!(
            call(
                &mut reg,
                BOB,
                "jumpCloneSvc",
                "DestroyInstalledClone",
                &[PyValue::Int(id)]
            )
            .is_err(),
            "not Bob's to destroy"
        );
        call(
            &mut reg,
            ALICE,
            "jumpCloneSvc",
            "DestroyInstalledClone",
            &[PyValue::Int(id)],
        )
        .unwrap();
        assert_eq!(pushes(&mut reg), vec![invalidate]);
        assert!(clone_rows(&mut reg, ALICE).is_empty());
    }

    /// The revive: a new capsule in the home station's hangar, the dead one
    /// gone, the character in it, and a session change whose `shipid` runs
    /// from the dead pod to the new one -- the one pair
    /// `gameui._ParseStateChange:506` needs to call it a podding.
    #[test]
    fn a_revive_puts_the_character_in_a_fresh_capsule_at_home() {
        let mut reg = two();
        let r = revive_in_clone(&mut reg, ALICE).expect("revived");
        assert_eq!(r.old_ship_id, POD);
        assert_ne!(r.capsule_id, POD);
        assert!(reg.store().get(POD).is_none(), "the dead capsule is gone");
        let pod = reg
            .store()
            .get(r.capsule_id)
            .expect("the new capsule exists");
        assert_eq!(
            (pod.type_id, pod.owner_id, pod.location_id),
            (TYPE_CAPSULE, ALICE, 60_012_526)
        );
        let c = reg.character(ALICE).unwrap();
        assert_eq!((c.ship_id, c.ship_type_id), (r.capsule_id, TYPE_CAPSULE));
        assert_eq!(
            crate::pyutil::pairs_get(&r.change, b"shipid"),
            Some(&PyValue::Tuple(vec![
                PyValue::Int(POD),
                PyValue::Int(r.capsule_id)
            ]))
        );
        assert!(
            reg.take_session_changes().is_empty(),
            "the revive returns its change; the clock caller sends it"
        );
    }

    /// Two types the store knows, standing in for implants at the clone
    /// level (the slot rules are `crate::implants`' tests').
    const IMPLANT_A: i64 = 34;
    const IMPLANT_B: i64 = 185;

    /// Put one `type_id` in `who`'s head.
    fn plug(reg: &mut Registry, who: i64, type_id: i64) -> i64 {
        let ch = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id,
                quantity: 1,
                to: crate::item::Slot::new(who, who, crate::dogma::FLAG_IMPLANT),
            }])
            .expect("created");
        let _ = reg.take_pending();
        ch.iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .expect("an id")
    }

    fn jump(reg: &mut Registry, id: i64, to: StationRef) {
        call(
            reg,
            ALICE,
            "jumpCloneSvc",
            "CloneJump",
            &[
                PyValue::Int(to.station_id),
                PyValue::Int(id),
                PyValue::Real(0.0),
                PyValue::Bool(false),
            ],
        )
        .expect("jumped");
    }

    /// A podding destroys the implants in the head (K-POD).
    #[test]
    fn a_revive_destroys_the_implants() {
        let mut reg = two();
        let imp = plug(&mut reg, ALICE, IMPLANT_A);
        assert_eq!(implants_of(&reg, ALICE), vec![imp]);
        revive_in_clone(&mut reg, ALICE).expect("revived");
        assert!(implants_of(&reg, ALICE).is_empty());
        assert!(reg.store().get(imp).is_none(), "the implant is gone");
    }

    /// **IMPL**: a jump to a clone elsewhere. The clone you leave keeps the
    /// set you wore, as the jump clone where your body stood; the clone you
    /// wake in brings its own.
    #[test]
    fn a_jump_elsewhere_leaves_the_worn_set_behind_and_wakes_with_the_clones() {
        let mut reg = two();
        let worn = plug(&mut reg, ALICE, IMPLANT_A);
        let id = reg.clones.install(ALICE, elsewhere(), String::new());
        reg.clones.set_implants(id, vec![IMPLANT_B]);
        let here = StationRef::of(reg.character(ALICE).unwrap()).expect("seated");
        jump(&mut reg, id, elsewhere());
        // The head now holds the clone's set; the worn item is gone.
        assert!(reg.store().get(worn).is_none());
        assert_eq!(crate::implants::head_types(&reg, ALICE), vec![IMPLANT_B]);
        // The clone that was jumped to is used up; one stands where the body
        // was, and holds what it wore.
        let mine = reg.clones.of(ALICE);
        assert_eq!(mine.len(), 1);
        let (left, c) = mine[0];
        assert_eq!(c.station.station_id, here.station_id);
        assert_eq!(reg.clones.implants_in(left), [IMPLANT_A]);
        assert!(reg.clones.implants_in(id).is_empty() || id == left);
    }

    /// A jump to a clone in the same station swaps the two sets in place.
    #[test]
    fn a_jump_in_place_swaps_the_two_sets() {
        let mut reg = two();
        plug(&mut reg, ALICE, IMPLANT_A);
        let here = StationRef::of(reg.character(ALICE).unwrap()).expect("seated");
        let id = reg.clones.install(ALICE, here, String::new());
        reg.clones.set_implants(id, vec![IMPLANT_B]);
        let _ = pushes(&mut reg);
        jump(&mut reg, id, here);
        assert_eq!(crate::implants::head_types(&reg, ALICE), vec![IMPLANT_B]);
        assert_eq!(reg.clones.implants_in(id), [IMPLANT_A]);
        let seen: Vec<&str> = pushes(&mut reg).iter().map(|p| p.0).collect();
        assert!(seen.contains(&"OnServerImplantsChanged"), "{seen:?}");
        // And back again.
        jump(&mut reg, id, here);
        assert_eq!(crate::implants::head_types(&reg, ALICE), vec![IMPLANT_A]);
        assert_eq!(reg.clones.implants_in(id), [IMPLANT_B]);
    }

    /// The panel reads them: one `(jumpCloneID, typeID)` row per implant in
    /// each clone, and a destroyed clone takes its implants with it.
    #[test]
    fn get_clone_state_lists_each_clones_implants_and_a_destroyed_clone_loses_them() {
        let mut reg = two();
        let id = reg.clones.install(ALICE, elsewhere(), String::new());
        reg.clones.set_implants(id, vec![IMPLANT_A, IMPLANT_B]);
        let rows = |reg: &mut Registry| -> Vec<(i64, i64)> {
            let state = call(reg, ALICE, "jumpCloneSvc", "GetCloneState", &[]).expect("answered");
            let PyValue::ObjectEx { header, .. } = attr(&state, "implants") else {
                panic!("implants must be a CRowset")
            };
            let PyValue::Tuple(head) = header.as_ref() else {
                panic!("a CRowset header is (class, args)")
            };
            let PyValue::Tuple(args) = &head[1] else {
                panic!("CRowset args")
            };
            let PyValue::List(rows) = &args[1] else {
                panic!("CRowset rows")
            };
            rows.iter()
                .map(|r| {
                    let PyValue::DbRow(row) = r else {
                        panic!("a row is a DBRow")
                    };
                    let int = |i: usize| match row.values()[i] {
                        DbValue::Int(v) => v,
                        ref o => panic!("not an int: {o:?}"),
                    };
                    (int(0), int(1))
                })
                .collect()
        };
        assert_eq!(rows(&mut reg), vec![(id, IMPLANT_A), (id, IMPLANT_B)]);
        call(
            &mut reg,
            ALICE,
            "jumpCloneSvc",
            "DestroyInstalledClone",
            &[PyValue::Int(id)],
        )
        .expect("destroyed");
        assert!(rows(&mut reg).is_empty());
        assert!(reg.clones.implants_in(id).is_empty());
    }

    /// From space the change is the dock's pair as well: `stationid` arrives
    /// and `solarsystemid` goes, which is what tears the ballpark down.
    #[test]
    fn a_revive_from_space_carries_the_dock_pair() {
        let mut reg = two();
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                ALICE,
                POD,
                30_000_001,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        let r = revive_in_clone(&mut reg, ALICE).expect("revived");
        assert_eq!(
            crate::pyutil::pairs_get(&r.change, b"stationid"),
            Some(&PyValue::Tuple(vec![
                PyValue::None,
                PyValue::Int(60_012_526)
            ]))
        );
        assert_eq!(
            crate::pyutil::pairs_get(&r.change, b"solarsystemid"),
            Some(&PyValue::Tuple(vec![
                PyValue::Int(30_000_001),
                PyValue::None
            ]))
        );
        assert!(!reg.roster().is_in_space(ALICE));
    }

    /// A jump to a clone in another station moves the character there in a
    /// fresh capsule and queues the station change on the caller.
    #[test]
    fn a_jump_to_another_station_changes_station_and_ship() {
        let mut reg = two();
        let id = reg.clones.install(ALICE, elsewhere(), String::new());
        call(
            &mut reg,
            ALICE,
            "jumpCloneSvc",
            "CloneJump",
            &[
                PyValue::Int(elsewhere().station_id),
                PyValue::Int(id),
                PyValue::Real(0.0),
                PyValue::Bool(false),
            ],
        )
        .expect("jumped");
        let changes = reg.take_session_changes();
        assert_eq!(changes.len(), 1);
        let (who, change) = &changes[0];
        assert_eq!(*who, ALICE);
        assert_eq!(
            crate::pyutil::pairs_get(change, b"stationid"),
            Some(&PyValue::Tuple(vec![
                PyValue::Int(60_012_526),
                PyValue::Int(elsewhere().station_id)
            ]))
        );
        assert!(crate::pyutil::pairs_get(change, b"shipid").is_some());
        assert!(crate::pyutil::pairs_get(change, b"solarsystemid2").is_some());
        let c = reg.character(ALICE).unwrap();
        assert_eq!(c.station_id, Some(elsewhere().station_id));
        assert_eq!(c.solar_system_id, Some(elsewhere().solar_system_id));
    }

    /// Through a real world file: the home, the clones, and -- the reason
    /// `capsules` exists -- the minted capsule is the active ship again after
    /// a restart, rather than the seeded id the revive destroyed.
    #[test]
    fn clones_and_the_revived_capsule_survive_the_world_file() {
        let mut reg = two();
        call(
            &mut reg,
            ALICE,
            "home_station",
            "set_home_station",
            &[PyValue::Int(60_012_526), PyValue::Bool(false)],
        )
        .unwrap();
        call(
            &mut reg,
            ALICE,
            "jumpCloneSvc",
            "InstallCloneInStation",
            &[],
        )
        .unwrap();
        let r = revive_in_clone(&mut reg, ALICE).expect("revived");
        let dir = std::env::temp_dir().join(format!(
            "eve-clones-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("a directory to write in");
        let store = crate::store::FileStore::new(dir.join("world.txt"));
        crate::store::Store::save(&store, &reg.to_snapshot()).expect("write the world");
        let snap = crate::store::Store::load(&store)
            .expect("read the world")
            .expect("a world was written");
        let _ = std::fs::remove_dir_all(&dir);
        let mut fresh = two();
        assert_eq!(
            fresh.character(ALICE).unwrap().ship_id,
            POD,
            "or this proves nothing"
        );
        fresh.apply_snapshot(snap);
        assert_eq!(fresh.clones, reg.clones);
        assert_eq!(fresh.character(ALICE).unwrap().ship_id, r.capsule_id);
    }

    #[test]
    fn clones_round_trip_through_their_records() {
        let mut c = Clones::default();
        c.homes.insert(ALICE, elsewhere());
        c.install(ALICE, elsewhere(), "named one".to_owned());
        let two = c.install(BOB, elsewhere(), String::new());
        c.set_implants(two, vec![34, 185]);
        c.capsules.insert(ALICE, (100_000_123, elsewhere()));
        let mut out = String::new();
        c.render(&mut out);
        let mut back = Clones::default();
        for line in out.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            assert!(Clones::RECORDS.contains(&f[0]));
            back.parse_record(&f).expect(line);
        }
        assert_eq!(back, c);
    }

    #[test]
    fn a_clone_name_is_one_line_and_at_most_a_hundred_characters() {
        assert_eq!(clean_clone_name(" a\tb\nc "), "a b c");
        assert_eq!(clean_clone_name(&"x".repeat(150)).len(), 100);
    }
}
