//! Bounties and kill rights, and the dynamic bounty multiplier.
//!
//! Split out of `service/space.rs` by W10.5 seam S2 (`docs/content/plan.md`
//! §6.1), so a lane working on bounties opens one file and cannot collide
//! with a lane working on flight. The arms and their tests moved verbatim.
//!
//! **Kill rights** (CW-1) are `crate::crimewatch::killright`'s: these arms
//! parse the client's arguments and hand over.

use super::*;
use crate::bounty_pool as pool;
use crate::crimewatch::killright;

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &["bountyProxy", "dynamicBountyMgr", "killRightMgr"];

/// The `i`-th positional argument, or the keyword `name`.
fn arg<'a>(req: &'a Request<'_>, i: usize, name: &str) -> Option<&'a PyValue> {
    req.args.get(i).or_else(|| {
        req.kwargs
            .iter()
            .find_map(|(v, k)| (crate::pyutil::as_bytes(k) == Some(name.as_bytes())).then_some(v))
    })
}

fn bad(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

fn kill_right_id(req: &Request<'_>) -> Result<i64, ServiceError> {
    crate::pyutil::opt_id(arg(req, 0, "killRightID"))
        .ok_or_else(|| bad(req, "killRightID is not an id"))
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        // Bounties and kill rights. **Answered before anything can ask**,
        // because the shape of the caller makes it a live lock the moment
        // the first ball arrives.
        //
        //     if reg.myKillRights is not None:
        //         return reg.myKillRights
        //     killRights = sm.ProxySvc('bountyProxy').GetMyKillRights()
        //     reg.myKillRights = killRights      # a None is STORED
        //     return killRights
        //
        // That is `contractProxy.NumOutstandingContracts` exactly: the
        // guard is `is not None`, the assignment *completes*, and the
        // cache is poisoned with the one value that fails its own guard.
        // `GetBestKillRight` then does
        // `for killRight in reg.GetMyKillRights():` and raises -- and
        // `activeitem.UpdateAll` re-enters that every **500 ms** while
        // anything is selected.
        //
        // Nothing reaches it yet: `bountySvc._DoBallsAdded` is a
        // `DoBallsAdded` handler, so it first fires the instant we push a
        // state with balls in it. Landing it now means the first push
        // cannot bring a frame-rate bug with it.
        "bountyProxy" => match req.method {
            // Iterated, and rows are read for `.fromID`, `.toID`,
            // `.price` and `.killRightID`: every live right the caller holds
            // or is the target of (CW-1).
            "GetMyKillRights" => Ok(killright::get_my(
                reg,
                req.actor,
                crate::pyutil::filetime_now(),
            )),
            // A 2-tuple, unpacked inside a `uthread.parallel` and fed to
            // `bountyUtil.CacheBounties` and
            // `killRightTracker.CacheKillRights`: the pools on the first
            // argument's ids (as `GetBounties`), and the kill rights for
            // sale on the second argument's characters.
            "GetBountiesAndKillRights" => {
                let ids = killright::ids_in(arg(req, 0, "bountyIDs"));
                let to = killright::ids_in(arg(req, 1, "killRightsToFetch"));
                Ok(PyValue::Tuple(vec![
                    pool::bounties(reg, &ids),
                    killright::on_characters(reg, req.actor, &to, crate::pyutil::filetime_now()),
                ]))
            }
            // `bountySvc.FetchBounties` -> `GetBounties(targetIDs)`: iterated
            // as `(targetID, bounty)` pairs and each `bounty` cached by its
            // `.targetID` and read for `.bounty` (`bountyUtil.CacheBounties`)
            // **[V]**. `None` here threw `TypeError` inside the selected-item
            // window's 500 ms update, at every selection. One row per target,
            // 0.0 where nothing is placed.
            "GetBounties" => {
                let targets = killright::ids_in(arg(req, 0, "targetIDs"));
                Ok(pool::bounties(reg, &targets))
            }
            // The Bounty Office (GAP-BOUNTY, `bounty_pool`): place, list the
            // caller's, the top pools and hunters, search.
            "AddToBounty" => {
                let target = crate::pyutil::opt_id(arg(req, 0, "ownerID"))
                    .ok_or_else(|| bad(req, "ownerID is not an id"))?;
                let amount = killright::number(arg(req, 1, "amount"))
                    .ok_or_else(|| bad(req, "amount is not a number"))?;
                let me = req.actor.ok_or_else(|| bad(req, "no character"))?;
                pool::place(reg, me, target, amount, crate::pyutil::filetime_now())
            }
            "GetMyBounties" => Ok(pool::mine(reg, req.actor)),
            "GetTopPilotBounties" | "GetTopCorpBounties" | "GetTopAllianceBounties" => {
                let page = crate::pyutil::opt_id(arg(req, 0, "page")).unwrap_or(0);
                let kind = match req.method {
                    "GetTopPilotBounties" => pool::Owner::Char,
                    "GetTopCorpBounties" => pool::Owner::Corp,
                    _ => pool::Owner::Alliance,
                };
                Ok(pool::top(reg, kind, page, crate::pyutil::filetime_now()))
            }
            "GetTopPilotBountyHunters"
            | "GetTopCorporationBountyHunters"
            | "GetTopAllianceBountyHunters" => {
                let page = crate::pyutil::opt_id(arg(req, 0, "page")).unwrap_or(0);
                let kind = match req.method {
                    "GetTopPilotBountyHunters" => pool::Owner::Char,
                    "GetTopCorporationBountyHunters" => pool::Owner::Corp,
                    _ => pool::Owner::Alliance,
                };
                Ok(pool::top_hunters(
                    reg,
                    kind,
                    page,
                    crate::pyutil::filetime_now(),
                ))
            }
            "SearchCharBounties" | "SearchCorpBounties" | "SearchAllianceBounties" => {
                let id = crate::pyutil::opt_id(arg(req, 0, "targetID")).unwrap_or(0);
                let kind = match req.method {
                    "SearchCharBounties" => pool::Owner::Char,
                    "SearchCorpBounties" => pool::Owner::Corp,
                    _ => pool::Owner::Alliance,
                };
                Ok(pool::search(reg, kind, id))
            }
            // `FetchKillRightsFromServer(toIDs)`: the same rows alone.
            "GetKillRightsOnCharacters" => {
                let to = killright::ids_in(arg(req, 0, "toIDs"));
                Ok(killright::on_characters(
                    reg,
                    req.actor,
                    &to,
                    crate::pyutil::filetime_now(),
                ))
            }
            // `SellKillRight(killRightID, price, restrictedTo)`.
            "SellKillRight" => {
                let id = kill_right_id(req)?;
                let price = killright::number(arg(req, 1, "price"))
                    .ok_or_else(|| bad(req, "price is not a number"))?;
                let restricted = crate::pyutil::opt_id(arg(req, 2, "restrictedTo"));
                killright::sell(
                    reg,
                    req.actor,
                    id,
                    price,
                    restricted,
                    crate::pyutil::filetime_now(),
                )
            }
            // `CancelSellKillRight(killRightID, toID)`; the reply is
            // discarded.
            "CancelSellKillRight" => {
                let id = kill_right_id(req)?;
                Ok(killright::cancel(
                    reg,
                    req.actor,
                    id,
                    crate::pyutil::filetime_now(),
                ))
            }
            _ => Err(reg.no_method(req)),
        },
        // Kill-right activation and purchase (CW-1). Both replies are
        // discarded; a refusal is a `UserError` the client catches by key
        // (`bountySvc.ActivateKillRight` / `BuyKillRight`) **[V]**.
        "killRightMgr" => match req.method {
            "ActivateKillRight" => {
                let id = kill_right_id(req)?;
                killright::activate(reg, req.actor, id, crate::pyutil::filetime_now())
            }
            "BuyKillRight" => {
                let id = kill_right_id(req)?;
                let price = killright::number(arg(req, 1, "price"))
                    .ok_or_else(|| bad(req, "price is not a number"))?;
                killright::buy(reg, req.actor, id, price, crate::pyutil::filetime_now())
            }
            _ => Err(reg.no_method(req)),
        },
        // The dynamic bounty system's multiplier for this system.
        //
        // Unpacked as `output, dbsIsActive = ...` before any test, so
        // `None` is a `TypeError`. The same function opens with a
        // feature-flag branch that returns CCP's "DBS is not running"
        // value outright --
        //
        //     if GetDynamicBountySystemEmergencyOffline(reg.machoNet):
        //         return (1.0, False)
        //
        // -- which is still the answer with no actor, or an actor with no
        // system. Otherwise CP38 answers `(d, True)` for the actor's
        // system, in space or docked (`econ_dbs::answer`, C5 §5.3).
        "dynamicBountyMgr" => match req.method {
            "GetOutputForClientSolarSystem" => match req.actor.and_then(|a| actor_system(reg, a)) {
                Some(system) => Ok(crate::econ_dbs::answer(
                    reg,
                    system,
                    crate::pyutil::filetime_now(),
                )),
                None => Ok(PyValue::Tuple(vec![
                    PyValue::Real(crate::econ_dbs::DEFAULT),
                    PyValue::Bool(false),
                ])),
            },
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

/// Where the character is (`Registry::session_system`).
fn actor_system(reg: &Registry, actor: i64) -> Option<i64> {
    reg.session_system(actor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(reg: &mut Registry, actor: Option<i64>) -> PyValue {
        dispatch(
            reg,
            &Request {
                service: "dynamicBountyMgr",
                method: "GetOutputForClientSolarSystem",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor,
            },
        )
        .expect("answered")
    }

    /// **The answer is a `(float, bool)` tuple** [V]: the actor's system's
    /// multiplier and active; with no actor, CCP's own offline answer.
    #[test]
    fn get_output_is_the_actors_system_multiplier() {
        let mut reg = crate::testkit::registry();
        let me = reg.config.characters[0].clone();
        let system = me.solar_system_id.expect("a system");
        let now = crate::pyutil::filetime_now();
        let p = crate::econ_dbs::pool(&reg, system);
        crate::econ_dbs::drain(&mut reg, system, p / 4.0, now);
        let PyValue::Tuple(parts) = ask(&mut reg, Some(me.character_id)) else {
            panic!("a tuple")
        };
        let PyValue::Real(d) = parts[0] else {
            panic!("a float")
        };
        assert!(d < crate::econ_dbs::DEFAULT, "the drained value");
        assert_eq!(parts[1], PyValue::Bool(true));
        assert_eq!(
            ask(&mut reg, None),
            PyValue::Tuple(vec![
                PyValue::Real(crate::econ_dbs::DEFAULT),
                PyValue::Bool(false)
            ])
        );
    }

    /// `GetBounties` is iterated as `(targetID, bounty)` pairs; `None` was a
    /// `TypeError` in the selected-item window at every selection.
    #[test]
    fn get_bounties_is_a_list_of_pairs_one_per_target() {
        let mut reg = crate::testkit::registry();
        let got = dispatch(
            &mut reg,
            &Request {
                service: "bountyProxy",
                method: "GetBounties",
                args: &[PyValue::List(vec![
                    PyValue::Int(70_001),
                    PyValue::Int(70_002),
                ])],
                kwargs: &[],
                bind_params: None,
                actor: None,
            },
        )
        .expect("answered");
        let PyValue::List(rows) = got else {
            panic!("a list, got {got:?}")
        };
        assert_eq!(rows.len(), 2);
        let PyValue::Tuple(pair) = &rows[0] else {
            panic!("a pair")
        };
        assert_eq!(pair[0], PyValue::Int(70_001));
    }
}
