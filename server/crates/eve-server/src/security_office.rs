//! **The security office** (SEC-1c; `docs/lanes/sec-1.md`): a low-sec CONCORD
//! or DED station turns Clone Soldier tags into security status.
//!
//! The client's window (`securityOfficeWindow`) does the arithmetic on its
//! side with `crimewatch.util.CalculateTagRequirementsForSecIncrease`, shows
//! the cost, and then makes **one call**:
//!
//! ```text
//! crimewatch.ApplyTagsForSecStatus(targetSecurityStatus, currentSecurityStatus)
//! ```
//!
//! on the location-bound crimewatch moniker (`CharGetCrimewatchLocation`);
//! the answer is thrown away and the window refreshes itself from
//! `OnItemChange`, `OnAccountChange` and `OnSecurityStatusUpdate` **[V]**.
//! [`apply`] is the server's half, in the order the spec names:
//!
//! 1. the pilot's status moved since the window opened and the plan now needs
//!    more tags: `MoreTagsNeedDueToLowerSec`;
//! 2. the pilot is docked in a station owned by CONCORD or DED, in low-sec
//!    (`securityOfficeSvc.CanAccessServiceInStation` **[V]**); else a
//!    `CustomNotify`, because the client names no key for it **[A]**;
//! 3. the tags come out of **the station hangar** (flag 4, location = the
//!    station, owner = the pilot), the place `GetTagsInHangar` lists **[V]**;
//! 4. the fee per tag (attribute 1904, journal ref 117, paid to CONCORD)
//!    is charged, the tags are consumed and the status rises.
//!
//! Nothing is taken unless everything can be: the check comes first, then
//! the items, then the ISK.

use crate::sec_status::{self, Tx};
use crate::service::{Registry, Request, ServiceError};
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// One tag type and the half-open status band `[min, max)` it is spent in
/// (`crimewatch.const.securityLevelsPerTagType`) **[V]**.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub type_id: i64,
    pub min: f64,
    pub max: f64,
}

/// The client's four bands, lowest first. The type ids are the client's own
/// constants (`inventorycommon.const.typeSecurityTagCloneSoldier*`,
/// group 1206) **[V]**: Trainer, Recruiter, Transporter, Negotiator.
pub const BANDS: [Band; 4] = [
    Band {
        type_id: 33138,
        min: -10.0,
        max: -8.0,
    },
    Band {
        type_id: 33139,
        min: -8.0,
        max: -5.0,
    },
    Band {
        type_id: 33140,
        min: -5.0,
        max: -2.0,
    },
    Band {
        type_id: 33141,
        min: -2.0,
        max: 0.0,
    },
];

/// `appConst.securityGainPerTag` **[V]**.
pub const GAIN_PER_TAG: f64 = 0.5;

/// `dogmaattributes` 1904, `securityProcessingFee` **[V]**
/// (`appConst.attributeSecurityProcessingFee`): the ISK one tag costs to turn
/// in.
pub const ATTR_PROCESSING_FEE: i64 = 1904;

/// `appConst.refSecurityTagProcessingFee` **[V]**: the journal row.
pub const REF_PROCESSING_FEE: i64 = 117;

/// `appConst.stationOwnersWithSecurityService` **[V]**: CONCORD and DED.
pub const OWNERS_WITH_SERVICE: [i64; 2] = [1_000_125, 1_000_137];

/// Why a plan cannot be made.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlanError {
    /// `requested` is not in `(-10, 10]`.
    OutOfRange,
    /// `requested <= start`: nothing to buy.
    NotHigher,
    /// A status no band covers (at or above 0.0): no tag lifts it further.
    NoTags,
}

/// **`CalculateTagRequirementsForSecIncrease`** ported exactly **[V]**: walk
/// the bands from `start`; in the band holding the running status take
/// `ceil((min(requested, bandMax) - status) / 0.5)` tags of its type and add
/// them x 0.5; repeat until the status reaches `requested`. Answers
/// `(tag type, count)` in band order. Errors where the client raises.
pub fn requirements(
    bands: &[Band],
    start: f64,
    requested: f64,
) -> Result<Vec<(i64, u32)>, PlanError> {
    if !(sec_status::MIN_STATUS < requested && requested <= sec_status::MAX_STATUS) {
        return Err(PlanError::OutOfRange);
    }
    if requested <= start {
        return Err(PlanError::NotHigher);
    }
    let mut out: Vec<(i64, u32)> = Vec::new();
    let mut status = start;
    while status < requested {
        let Some(band) = bands.iter().find(|b| b.min <= status && status < b.max) else {
            return Err(PlanError::NoTags);
        };
        let n = ((requested.min(band.max) - status) / GAIN_PER_TAG).ceil() as u32;
        match out.iter_mut().find(|(t, _)| *t == band.type_id) {
            Some((_, have)) => *have += n,
            None => out.push((band.type_id, n)),
        }
        status += f64::from(n) * GAIN_PER_TAG;
    }
    Ok(out)
}

/// The total number of tags of a plan.
pub fn tag_count(plan: &[(i64, u32)]) -> u32 {
    plan.iter().map(|(_, n)| n).sum()
}

/// `securityOfficeSvc.CanAccessServiceInStation`, as the pure rule: the
/// station's owner is CONCORD or DED, and its system is low-sec
/// (`is_low_sec_solar_system`) **[V]**.
pub fn office_open(owner: i64, class: Option<crate::celestial::SecurityClass>) -> bool {
    OWNERS_WITH_SERVICE.contains(&owner) && class == Some(crate::celestial::SecurityClass::Low)
}

/// The tags `character` has in `station`'s hangar, by type (the
/// `GetTagsInHangar` of the server's side).
pub fn tags_in_hangar(
    w: &Registry,
    character: i64,
    station: i64,
    bands: &[Band],
) -> BTreeMap<i64, i64> {
    let mut out = BTreeMap::new();
    let slot = crate::item::Slot::new(character, station, crate::model::FLAG_HANGAR);
    for item in w.store().at(slot) {
        if bands.iter().any(|b| b.type_id == item.type_id) {
            *out.entry(item.type_id).or_insert(0) += item.quantity;
        }
    }
    out
}

fn notify(text: &str) -> ServiceError {
    crate::modules::user_error("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

fn number(v: Option<&PyValue>) -> Option<f64> {
    match v {
        Some(PyValue::Real(r)) if r.is_finite() => Some(*r),
        Some(PyValue::Int(i)) => Some(*i as f64),
        _ => None,
    }
}

/// The fee of one tag of `type_id`, from the type table; `0` where the table
/// has no row (a server with no `--typedata`). **[A]** Falsified by a tag
/// that is free in a world with a bundle.
fn fee_of(w: &Registry, type_id: i64) -> f64 {
    crate::typedata::attribute_in(w.content(), type_id, ATTR_PROCESSING_FEE).unwrap_or(0.0)
}

/// **`ApplyTagsForSecStatus(target, current)`** for the acting pilot.
pub fn apply(w: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    apply_with(w, req, &BANDS)
}

/// [`apply`] over a band table: the tests hand their own.
pub(crate) fn apply_with(
    w: &mut Registry,
    req: &Request<'_>,
    bands: &[Band],
) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Ok(PyValue::None);
    };
    let (Some(target), Some(shown)) = (number(req.args.first()), number(req.args.get(1))) else {
        return Err(notify(
            "The security office needs a target and a current status.",
        ));
    };
    let Some(station) = w.docked_station(actor) else {
        return Err(notify(
            "The security office is a station service: dock first.",
        ));
    };
    let (owner, system) = w
        .character(actor)
        .map(|c| (c.station_owner_id, c.solar_system_id))
        .unwrap_or_default();
    let class = system.and_then(|s| {
        w.content()
            .get::<crate::celestial::Universe>()
            .security_class(s)
    });
    if !office_open(owner, class) {
        return Err(notify(
            "Only low-sec stations of CONCORD and the DED have a security office.",
        ));
    }
    let status = crate::npc::bounty::security_status(w, actor);
    if target <= status {
        return Ok(PyValue::None);
    }
    let plan = match requirements(bands, status, target) {
        Ok(p) => p,
        Err(_) => return Err(notify("No tags can raise your security status that far.")),
    };
    // The window planned from `shown`. If the status fell since, the plan it
    // paid for is short.
    if shown != status {
        let paid_for = requirements(bands, shown, target).map_or(0, |p| tag_count(&p));
        if tag_count(&plan) > paid_for {
            return Err(crate::modules::user_error(
                "MoreTagsNeedDueToLowerSec",
                vec![],
            ));
        }
    }
    let have = tags_in_hangar(w, actor, station, bands);
    if plan
        .iter()
        .any(|(t, n)| have.get(t).copied().unwrap_or(0) < i64::from(*n))
    {
        return Err(notify("You do not have the tags in this station's hangar."));
    }
    let fee: f64 = plan
        .iter()
        .map(|(t, n)| fee_of(w, *t) * f64::from(*n))
        .sum();
    let balance = w.character(actor).map_or(0.0, |c| c.balance);
    if balance < fee {
        return Err(crate::modules::user_error(
            "NotEnoughMoney",
            vec![
                ("amount", PyValue::Real(fee)),
                ("balance", PyValue::Real(balance)),
            ],
        ));
    }
    let mut edits = Vec::new();
    for (type_id, count) in &plan {
        let mut left = i64::from(*count);
        let slot = crate::item::Slot::new(actor, station, crate::model::FLAG_HANGAR);
        let mut stacks: Vec<(i64, i64)> = w
            .store()
            .at(slot)
            .filter(|i| i.type_id == *type_id)
            .map(|i| (i.item_id, i.quantity))
            .collect();
        stacks.sort_unstable();
        for (item_id, quantity) in stacks {
            if left == 0 {
                break;
            }
            let take = quantity.min(left);
            edits.push(crate::item::Edit::Consume {
                item_id,
                quantity: take,
            });
            left -= take;
        }
    }
    if w.apply_items(&edits).is_err() {
        return Err(notify("The tags could not be taken from the hangar."));
    }
    if fee > 0.0 {
        w.charge(actor, fee, REF_PROCESSING_FEE, sec_status::OWNER_CONCORD, 0);
    }
    let gained = f64::from(tag_count(&plan)) * GAIN_PER_TAG;
    let after = target.min(status + gained);
    crate::npc::bounty::set_security_status(w, actor, after);
    sec_status::record(
        w,
        actor,
        Tx {
            event: sec_status::EVENT_HAND_IN_TAGS,
            date: crate::pyutil::filetime_now(),
            modification: sec_status::modification(status, after),
            new_value: after,
            other_owner: sec_status::OWNER_CONCORD,
            other_type: 0,
            location: station,
            start: None,
        },
    );
    Ok(PyValue::None)
}

#[cfg(test)]
mod tests;
