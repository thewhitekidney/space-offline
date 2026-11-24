//! Live-service content: campaigns, seasons, events, the NPE.
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
pub(crate) const SERVICES: &[&str] = &[
    "survey",
    "loginCampaignManager",
    "seasonalLoginCampaignManager",
    "air_npe",
    "operationsManager",
    "ProjectDiscovery",
    "seasonManager",
    "milestoneMgr",
    "nonDiminishingInjectionMgr",
    "browserLockdownSvc",
    "communityTrackMgr",
];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        "survey" => match req.method {
            // Both are skipped unless the shipped FSD survey data has an
            // active survey, so these should never be reached. Answered
            // quietly rather than refused, because a refusal here would
            // look like a protocol fault in a log.
            "PerformSurveyChecks" => Ok(PyValue::None),
            "GetActiveSurveyID" => Ok(PyValue::None),
            _ => Err(reg.no_method(req)),
        },
        "loginCampaignManager" => match req.method {
            // A falsy return short-circuits every reward check.
            "get_client_campaign_state" => Ok(PyValue::None),
            _ => Err(reg.no_method(req)),
        },
        // The seasonal (daily-login-reward) campaign.
        //
        // **`None` is not the empty answer here**, which is the whole
        // reason this arm is worth a comment. The sibling service above
        // takes a falsy value happily; this one does not. Its only
        // caller, `SeasonalLoginCampaignService.prime_campaign_data`,
        // unpacks the result before it tests anything:
        //
        //     45 CALL_FUNCTION    0
        //     48 UNPACK_SEQUENCE  4          <- unconditional
        //     51..60 STORE_FAST   campaign_id, end_date_ordinal,
        //                         turnover_time_str, num_remaining_rewards
        //     63 LOAD_FAST        campaign_id
        //     66 LOAD_CONST       None
        //     69 COMPARE_OP       9 (is not)
        //     72 POP_JUMP_IF_FALSE 168       -> straight to `return`
        //
        // So a bare `None` raises `TypeError: 'NoneType' object is not
        // iterable` inside the tasklet, while a 4-tuple whose first
        // element is None skips the entire campaign setup and leaves
        // `end_date`, `time_keeper` and `number_of_reward_days_remaining`
        // at None. Every downstream path is guarded by
        // `is_login_campaign_active()`, which is exactly
        // `active_login_reward_campaign_id is not None`.
        //
        // The other three slots are an ordinal for `date.fromordinal`, a
        // `'%H:%M:%S'` string for `strptime`, and a reward count; none is
        // read while the campaign id is None.
        "seasonalLoginCampaignManager" => match req.method {
            "get_active_campaign" => Ok(PyValue::Tuple(vec![
                PyValue::None,
                PyValue::None,
                PyValue::None,
                PyValue::None,
            ])),
            // Not reached while no campaign is active, but it must be a
            // real dict if it ever is: the client calls `.iteritems()` on
            // it unconditionally. Keys are date ordinals, values clone
            // grades. **[I]** -- not seen on the wire yet.
            "get_claim_history" => Ok(PyValue::Dict(vec![])),
            _ => Err(reg.no_method(req)),
        },
        // The new-player experience. `is_air_npe_enabled()` is used as a
        // plain boolean, and the branch it guards chooses between the
        // tutorial and `ActivateView('charsel')`. False leads to character
        // selection, and it is the answer of a server without `--air-npe`.
        // With `--air-npe` and the bundle's node graphs (`crate::air_npe`)
        // it is True, so character creation offers the tutorial; a character
        // with an active tutorial (kept at creation, or a GM's `/airnpe
        // start`) gets state 1 and everyone else the None below.
        "air_npe" => match req.method {
            "is_air_npe_enabled" => Ok(PyValue::Bool(crate::air_npe::enabled(reg, req.actor))),
            // A **scalar state id**, never a container, and cached per
            // character so this is one call per session. The vocabulary
            // is `AirNpeState.ACTIVE = 1 / COMPLETED = 2 / SKIPPED = 3`,
            // and every consumer either compares it for equality or tests
            // `in (..)`. So anything outside {1,2,3} reads as "no NPE
            // state", and None is the honest version of that for a
            // character who has neither completed nor skipped it.
            //
            // `1` makes `is_air_npe_active()` true, suppresses the
            // job-board window and pulls in the `node_graph` service, so
            // it is only for a character whose tutorial runs.
            "get_air_npe_state" => Ok(crate::air_npe::state(reg, req.actor)),
            _ => Err(reg.no_method(req)),
        },
        // The tutorial ("operations"). Used in boolean context only, and
        // `@Memoize`d, so one call per character.
        //
        // False is both honest -- we ship no tutorial content -- and by
        // some distance the cheaper branch. True would construct the
        // operations controller, whose `initialize` fires three more
        // unimplemented calls (`get_character_progress`,
        // `get_active_category_id`, `start_site`) plus an FSD tree and a
        // poller.
        "operationsManager" => match req.method {
            "can_character_play_the_tutorial" => Ok(PyValue::Bool(false)),
            _ => Err(reg.no_method(req)),
        },
        // Stored and consumed as a bool. `None` is falsy and harmless,
        // but it is also the service's "unknown" marker, which costs a
        // second call the first time the window is opened.
        "ProjectDiscovery" => match req.method {
            "initialize_tutorial_status" => Ok(PyValue::Bool(false)),
            // A bool, and a good illustration of why `None` is not free
            // even when nothing throws:
            //
            // `is_enabled` (`[V]`) fetches `remote_service.is_enabled()` while
            // its cache is `None` and returns the cache.
            //
            // `None` is the cache's own "not fetched yet" marker, so
            // answering `None` means the cache never fills and every
            // caller issues a fresh round trip. That is exactly what the
            // server log showed: this one method, twenty times in a
            // single run, while every other gap appeared once.
            //
            // A repeated `UNANSWERED` line is therefore a tell in its own
            // right -- it says the value is cached on `is None`.
            "is_enabled" => Ok(PyValue::Bool(false)),
            _ => Err(reg.no_method(req)),
        },
        // **`None` is exactly right here, and `[]` would be wrong.**
        //
        // `[V]` the client reads the season data for the character, returns
        // when it is `None`, and stores it as the season selection when it is
        // a list.
        //
        // The method is polymorphic by design -- `None`, a list of
        // seasons, or one season -- and `None` is the author's own "no
        // season running" case, tested first. An empty list is falsy but
        // `isinstance(x, list)` is true, which would push the client down
        // the season-selection branch instead.
        "seasonManager" => match req.method {
            "get_season_data_for_character" => Ok(PyValue::None),
            _ => Err(reg.no_method(req)),
        },
        // `carbonui/control/browser/sites.py(375-421)`: the client md5s
        // `str(cachedList)` -- `[]` on a fresh cache -- and downloads the list
        // only when that differs from `GetFlaggedSitesHash()`. Answering `None`
        // to both left `flaggedSites = None`, and `for row in flaggedSites`
        // raised `TypeError: 'NoneType' object is not iterable` out of the
        // first chat line (`chatutil/spam.py` -> `GetBannedInChatList`),
        // killing the chat-window tasklet. md5("[]") with an empty list is the
        // consistent pair: no download, no banned sites.
        "browserLockdownSvc" => match req.method {
            "GetFlaggedSitesHash" => {
                Ok(PyValue::Bytes(b"d751713988987e9331980363e24189ce".to_vec()))
            }
            "GetFlaggedSitesList" => Ok(PyValue::List(vec![])),
            _ => Err(reg.no_method(req)),
        },
        // A side-effect notification whose return value is discarded at
        // the only call site. Registered so it stops reading as a to-do.
        "milestoneMgr" => match req.method {
            "ProcessCharacterLogon" => Ok(PyValue::None),
            _ => Err(reg.no_method(req)),
        },
        // A scalar count. `None` does not crash -- both update handlers
        // guard `is not None` -- but `GetRemaining` caches on
        // `is None`, so `None` makes it re-issue the call on every read.
        "nonDiminishingInjectionMgr" => match req.method {
            "GetAvailableNonDiminishingInjections" => Ok(PyValue::Int(0)),
            _ => Err(reg.no_method(req)),
        },
        // Community tracks (the V24.02 Vanguard/Nemesis progress bar).
        // `[V]` communitytracks/client/repository.py `get_instance_attributes`:
        //
        // it unpacks a (definition id, progress) pair from
        // `communityTrackMgr.get_instance_attributes_public(sequential)`, uses
        // both when neither is `None`, and otherwise logs an error.
        //
        // The unpack is unconditional, so a bare `None` raises `TypeError:
        // 'NoneType' object is not iterable` (seen on 3707965 in
        // fci_tow_conquest_dormant). `(None, None)` is the author's own "no
        // such track instance" case: the repository logs and returns None,
        // `get_track_instance_and_definition` returns `(None, None)` and
        // `update_async` returns early. We run no community track, so that
        // is the honest answer. Not a live track: none is invented here.
        "communityTrackMgr" => match req.method {
            "get_instance_attributes_public" => {
                Ok(PyValue::Tuple(vec![PyValue::None, PyValue::None]))
            }
            // The QA insider menu's toggle (communitytracks/client/qa/menu.py) [V];
            // the client discards the answer.
            "change_qa_mode" => Ok(PyValue::None),
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

    /// The flagged-sites pair is consistent: the hash is md5 of `[]`, the
    /// client's empty cache, so it downloads nothing, and the list is empty,
    /// never `None` (which killed the chat tasklet on the first line).
    #[test]
    fn browser_lockdown_answers_an_empty_consistent_pair() {
        let mut reg = Registry::new(SandboxConfig::default());
        let call = |reg: &mut Registry, method: &'static str| {
            dispatch(
                reg,
                &Request {
                    service: "browserLockdownSvc",
                    method,
                    args: &[],
                    kwargs: &[],
                    bind_params: None,
                    actor: None,
                },
            )
            .unwrap()
        };
        assert_eq!(
            call(&mut reg, "GetFlaggedSitesHash"),
            PyValue::Bytes(b"d751713988987e9331980363e24189ce".to_vec())
        );
        assert_eq!(call(&mut reg, "GetFlaggedSitesList"), PyValue::List(vec![]));
    }

    /// An unknown community-track instance is a 2-tuple of None: the client
    /// unpacks `def_id, progress` unconditionally, so a bare None raised a
    /// TypeError in its tasklet. Invented instance id.
    #[test]
    fn a_community_track_instance_lookup_answers_no_instance_as_a_pair() {
        let mut reg = Registry::new(SandboxConfig::default());
        let args = [PyValue::Int(424242)];
        let got = dispatch(
            &mut reg,
            &Request {
                service: "communityTrackMgr",
                method: "get_instance_attributes_public",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: None,
            },
        )
        .unwrap();
        assert_eq!(got, PyValue::Tuple(vec![PyValue::None, PyValue::None]));
    }
}
