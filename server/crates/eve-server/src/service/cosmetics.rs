//! `shipCosmeticsMgr`: the SKIN licences a pilot holds, activating one, and
//! applying one to a ship (L2 SKINS, `crate::skins`).
//!
//! Moved out of `service/inventory.rs`, where the licence calls answered
//! empties so the Personalization window could draw. The comments on the two
//! empties below are the reasons they were never allowed to be `None`; they
//! still hold for the real answers (a list, never `None`).
//!
//! **Authorization.** The acting character is the session's (`req.actor`).
//! A character id in the arguments (`GetLicencedSkinsForShipType(charID,
//! ...)`, `GetFirstPartySkinData(licenseeID, ...)`) is the client's word and
//! is never believed: it must be the actor, or the answer is empty.

use super::*;
use crate::pyutil::keyval;

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &["shipCosmeticsMgr"];

fn int_at(req: &Request<'_>, i: usize) -> Option<i64> {
    match req.args.get(i) {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    }
}

/// The ids a list, tuple or bare id argument names.
fn ids_of(v: Option<&PyValue>) -> Vec<i64> {
    match v {
        Some(PyValue::Int(n)) => vec![*n],
        Some(PyValue::List(l) | PyValue::Tuple(l)) => l
            .iter()
            .filter_map(|x| match x {
                PyValue::Int(n) => Some(*n),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A licence row: `.skinID`, `.expires` (FILETIME or `None` for permanent),
/// `.isSingleUse` **[V]** (`cosmeticsSvc.GetSkins`, `skinsPanel`).
fn row(h: &crate::skins::Held) -> PyValue {
    keyval(vec![
        ("skinID", PyValue::Int(h.skin_id)),
        ("expires", h.expires.map_or(PyValue::None, PyValue::Int)),
        ("isSingleUse", PyValue::Bool(h.single_use)),
    ])
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let actor = req.actor;
    let now = crate::pyutil::filetime_now();
    match req.method {
        // Hull emblems. A dict keyed by a packed slot index.
        //
        // `{backendSlotIndex: cosmeticType}`, where the key packs group
        // and slot as `slot | (group << 6)` and the value is a
        // `CosmeticsType` (`CORPORATION_LOGO 1`, `ALLIANCE_LOGO 2`).
        // `.iteritems()` is called on it, so it must be a real dict.
        //
        // `None` is explicitly tolerated -- the caller guards
        // `if enabledCosmetics is not None` -- so this is not a fix
        // either. `{}` is still better: a non-empty answer is what drives
        // the client into the ship-cosmetics *license* gateway, which is
        // the protobuf transport we do not serve, and an empty dict keeps
        // that whole path dormant.
        //
        // Careful with the name: there are two files called `quasar.py`
        // in `shipcosmetics/client/`. `licensegateway/quasar.py` is
        // protobuf and is where the "Could not fetch owned ship cosmetic
        // licenses" timeout comes from; `fittingsgateway/quasar.py` is
        // this one, and it is plain machoNet.
        "GetEnabledCosmetics" => Ok(PyValue::Dict(vec![])),
        // The character's SKIN licences. **Both failure modes of
        // `None` at once**, which is what made this one worth doing
        // before anyone opened the window again.
        //
        // `cosmetics/client/cosmeticsSvc.py:157`:
        //
        // ```text
        //     def GetLicensedSkins(self):
        //         if self._licensedSkins is None:
        //             shipCosmeticsMgr = sm.RemoteSvc('shipCosmeticsMgr')
        //             self._licensedSkins = shipCosmeticsMgr.GetLicencedSkins()
        //         return self._licensedSkins
        // ```
        //
        // 1. The guard is `is None` and the store-back **succeeds**,
        //    so `None` sets the cache to the one value that fails its
        //    own guard. That is the `contractProxy
        //    .NumOutstandingContracts` mechanism from
        //    `docs/guides/debugging-the-client.md` §4 -- not the LP one,
        //    where the assignment threw and never ran. `ResetCache`
        //    only fires on `OnSessionReset`, so the cache stays
        //    poisoned for the session.
        // 2. **Every** caller iterates the stored value, and none of
        //    them guards it:
        //    `ui/shared/neocom/charsheet/skinsPanel.py:131` and
        //    `ui/cosmetics/ship/pages/collection/collectionUtil.py:16`
        //    both do `{s.skinID: s for s in ...GetLicensedSkins()}`,
        //    and `nodegraph/client/conditions/cosmetic.py:56` does
        //    `for skin in licenced_skins`. So each of the twelve
        //    callers takes a `TypeError`, every time, forever.
        //
        // `[]` closes both: it is iterable, and it is not `None`, so
        // the cache latches on the first call and the client stops
        // asking. The elements would be objects carrying `.skinID`,
        // `.expires` and `.isSingleUse` (read by `GetSkins` and by
        // `skinsPanel._CreateSkinObject`), but with zero rows their
        // type is unobservable, so a plain list is the honest answer
        // rather than a rowset we would be guessing at. A new
        // character owns no SKIN licences, which is why empty is
        // *correct* here and not a stub.
        //
        // **Which half of the Personalization window this fixes, and
        // which it does not.** That window has two unrelated faults
        // and they will be confused for each other. Ours is this one.
        // The other is
        // `shipcosmetics/client/licensegateway/quasar.py` raising
        // `RuntimeError: Could not fetch owned ship cosmetic licenses`
        // -- a **protobuf gateway** call on a transport we
        // deliberately do not serve at all
        // (`docs/guides/debugging-the-client.md` §3c,
        // `docs/archive/plan-docked-subsystems.md` "deliberately not in either
        // round"). No answer of ours can affect it, and it will still
        // stall the window after this arm lands. Do not read that
        // stall as this fix having failed.
        //
        // Careful with the file name: `shipcosmetics/client/` has two
        // `quasar.py`. `fittingsgateway/quasar.py` is plain machoNet
        // and is the `GetEnabledCosmetics` caller above;
        // `licensegateway/quasar.py` is the protobuf one.
        "GetLicencedSkins" => Ok(PyValue::List(match actor {
            Some(a) => crate::skins::held(reg, a, now).iter().map(row).collect(),
            None => vec![],
        })),
        // The per-hull version, `(charID, typeID)`, and it is here
        // because leaving it unanswered is *worse* than a wrong
        // answer: an unanswered call never returns and hangs the
        // tasklet that made it.
        //
        // `cosmeticsSvc.py:167`:
        //
        // ```text
        //     def _GetLicensedPersonalSkinsForType(self, typeID):
        //         if typeID not in self._licensedSkinsByType:
        //             licensedSkins = shipCosmeticsMgr \
        //                 .GetLicencedSkinsForShipType(session.charid, typeID)
        //             self._licensedSkinsByType[typeID] = licensedSkins
        //         return self._licensedSkinsByType[typeID]
        // ```
        //
        // This cache is keyed on `typeID not in`, so a stored `None`
        // latches and does **not** live-lock -- which is why the
        // census marks the consumer `store` and no guard, and why this
        // one is a plain raise rather than the compound failure above.
        // The raise is real, though: `GetSkins` guards with
        // `if licensedSkins:`, but
        // `ui/shared/info/panels/panelShipAvailableSkinLicenses.py:33`
        // does an unguarded
        // `{s.skinID: s.expires for s in cosmeticsSvc
        // .GetLicensedSkinsForType(self.typeID)}` when a ship's Show
        // Info opens its SKIN-licence panel.
        //
        // Note the spelling difference between the two sides: the
        // remote methods are `GetLicencedSkins*` (British) and the
        // client-side wrappers are `GetLicensedSkins*` (American).
        // Grepping for the wrong one finds nothing.
        "GetLicencedSkinsForShipType" => {
            // `(charID, typeID)`: only the actor's own licences.
            let (Some(a), Some(who), Some(hull)) = (actor, int_at(req, 0), int_at(req, 1)) else {
                return Ok(PyValue::List(vec![]));
            };
            let fits = |h: &crate::skins::Held| crate::skins::skin_fits(reg, h.skin_id, hull);
            Ok(PyValue::List(if who == a {
                crate::skins::held(reg, a, now)
                    .iter()
                    .filter(|h| fits(h))
                    .map(row)
                    .collect()
            } else {
                vec![]
            }))
        }
        // `(licenseeID, skinID)`: `None` or `.expires`/`.isSingleUse` **[V]**
        // (`cosmeticsSvc.GetFirstPartySkinObject`). For **any** licensee: the
        // client passes a ship owner's id on purpose (`preview.py`,
        // `controller.py`), and nothing here is secret or writable.
        "GetFirstPartySkinData" => {
            let (Some(_), Some(who), Some(skin)) = (actor, int_at(req, 0), int_at(req, 1)) else {
                return Ok(PyValue::None);
            };
            Ok(crate::skins::held(reg, who, now)
                .iter()
                .filter(|h| h.skin_id == skin)
                .max_by_key(|h| h.expires.unwrap_or(i64::MAX))
                .map_or(PyValue::None, |h| {
                    keyval(vec![
                        ("expires", h.expires.map_or(PyValue::None, PyValue::Int)),
                        ("isSingleUse", PyValue::Bool(h.single_use)),
                    ])
                }))
        }
        // `ActivateSkinLicense(itemIDs)`: consumes licence items, grants the
        // licences; the client resets its cache on `OnSkinLicenseActivated`.
        "ActivateSkinLicense" => {
            let a = actor.ok_or_else(|| crate::skins::SkinError::NoActor.raise())?;
            crate::skins::activate(reg, a, &ids_of(req.args.first()))
                .map(|_| PyValue::None)
                .map_err(crate::skins::SkinError::raise)
        }
        // `ApplySkinToShip(shipID, skinID|None)`.
        "ApplySkinToShip" => {
            let a = actor.ok_or_else(|| crate::skins::SkinError::NoActor.raise())?;
            let (Some(ship), skin) = (int_at(req, 0), int_at(req, 1)) else {
                return Err(crate::skins::SkinError::NoSuchItem.raise());
            };
            crate::skins::apply(reg, a, ship, skin)
                .map(|()| PyValue::None)
                .map_err(crate::skins::SkinError::raise)
        }
        _ => Err(reg.no_method(req)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SandboxConfig;

    /// One call, with a `SandboxConfig::default()` registry behind it.
    fn answer(method: &str, args: &[PyValue]) -> PyValue {
        let mut reg = Registry::new(SandboxConfig::default());
        let req = Request {
            service: "shipCosmeticsMgr",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(1_001),
        };
        dispatch(&mut reg, &req).unwrap_or_else(|e| panic!("{method} answered {e:?}"))
    }

    /// `shipCosmeticsMgr.GetLicencedSkins` must be iterable **and not
    /// `None`** -- the two halves of the same bug.
    ///
    /// `cosmetics/client/cosmeticsSvc.py:157` guards with
    /// `if self._licensedSkins is None:` and stores the answer back onto the
    /// guarded name, so `None` poisons the cache for the session; and all
    /// twelve callers iterate the stored value, so every one of them raised.
    /// A non-`None` iterable closes both at once.
    ///
    /// `GetLicencedSkinsForShipType` is checked with it because an
    /// unanswered call hangs the tasklet that made it, and
    /// `panelShipAvailableSkinLicenses.py:33` iterates its result with no
    /// guard.
    #[test]
    fn licenced_skins_are_a_non_none_iterable() {
        for method in ["GetLicencedSkins", "GetLicencedSkinsForShipType"] {
            let args = [PyValue::Int(1_001), PyValue::Int(670)];
            let got = answer(method, &args);
            assert_eq!(
                got,
                PyValue::List(vec![]),
                "{method} is iterated by every caller, and for GetLicencedSkins the \
                 `is None` guard in cosmeticsSvc.GetLicensedSkins stores the answer \
                 back -- so `None` is both a TypeError and a cache poisoned for the \
                 session. Got {got:?}"
            );
            assert_ne!(got, PyValue::None);
        }
    }

    #[test]
    fn the_per_hull_list_is_the_actors_alone_and_first_party_data_answers_for_any_licensee() {
        let (l, sk) = crate::skins::fixture::lines(5);
        let mut content = crate::content::Content::fixture();
        content.insert("skins", crate::skins::Skins::parse(&l, &sk).unwrap());
        let mut reg =
            Registry::new(SandboxConfig::default()).with_content(std::sync::Arc::new(content));
        let friend = 1_002;
        crate::skins::grant(
            &mut reg,
            friend,
            crate::skins::fixture::LIC_PERM,
            crate::pyutil::filetime_now(),
        )
        .unwrap();
        let ask = |reg: &mut Registry, method: &str, who: i64| {
            let args = [
                PyValue::Int(who),
                PyValue::Int(crate::skins::fixture::SKIN_A),
            ];
            let req = Request {
                service: "shipCosmeticsMgr",
                method,
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(1_001),
            };
            dispatch(reg, &req).unwrap()
        };
        // The client passes a ship owner's id on purpose.
        assert_ne!(
            ask(&mut reg, "GetFirstPartySkinData", friend),
            PyValue::None
        );
        assert_eq!(ask(&mut reg, "GetFirstPartySkinData", 1_001), PyValue::None);
        // The per-hull list is for the session's own pilot only.
        assert_eq!(
            ask(&mut reg, "GetLicencedSkinsForShipType", friend),
            PyValue::List(vec![])
        );
    }
}
