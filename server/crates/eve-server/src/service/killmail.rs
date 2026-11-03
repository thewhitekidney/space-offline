//! **The kill report wire** (SOC-C6): the character sheet's combat log and
//! the kill report window. The reports are `crate::killmail`'s.
//!
//! - `charMgr.GetRecentShipKillsAndLosses(num, startIndex)` [V call:
//!   `infosvc.GetKillsRecentKills` / `GetKillsRecentLosses`, both through
//!   `combatLogPanel.ShowCombatKills` / `ShowCombatLosses` with `num = 25`].
//!   The client filters the answer itself: kills are the rows whose
//!   `finalCharacterID` is the caller, losses those whose
//!   `victimCharacterID` is. So the answer is every report the caller is on,
//!   newest first. **Paging** [I, `combatLogPanel.ShowKillsEx`]: `startIndex`
//!   is `None` for the first page, then `min(killIDs)` for "view more" and
//!   a remembered `max(killIDs) + 1` for "previous", so it reads as "ids
//!   below this one". The arm lives in `service/character.rs` (the
//!   `charMgr` domain) and calls [`recent_ship_kills_and_losses`].
//! - `warStatisticMgr.GetKillMail(killID, hash)` [V call:
//!   `killReportUtil.handle_kill_report_link`, a `killReport:<id>:<hash>`
//!   chat link; the notification centre opens killmails through it too].
//!   The hash is the client's own (`crimewatch.util.GetKillReportHashValue`)
//!   and is checked (SEC-FIX 6, [`crate::notifications::kill_hash`]): a
//!   wrong or missing hash is `None`, as is an id not kept;
//!   `killReportUtil` tests it before opening.
//!
//! **Each report is a `utillib.KeyVal`** [V]: every reader goes by
//! attribute (`FormatKillMail`, `GetCombatEntries`, the `KillMail` list
//! entry) or copies it into a `KeyVal`, and EvEJS serves a `KeyVal` too
//! (fact only). `killTime` is a FILETIME integer: `kill_mail.py` subtracts
//! it from `blue.os.GetWallclockTime()`.

use super::*;
use crate::killmail::{Attacker, Report};

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &["warStatisticMgr"];

/// The most rows one `GetRecentShipKillsAndLosses` answers, whatever `num`
/// asks: the panel asks 25 [V].
pub const MAX_PAGE: usize = 200;

fn id(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

fn real(v: Option<f64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Real)
}

/// One report as the client reads it, with the war the kill was filed
/// under (`warID`, C8-b).
pub fn report_value_in(reg: &Registry, r: &Report) -> PyValue {
    report_value_for(r, crate::war::war_of_kill(reg, r.kill_id))
}

/// **Every attacker of a report** (REL-FIX): a list of `KeyVal`s, the final
/// blow first, one per pilot on the kill. The names are the `final*` fields'
/// with the prefix dropped [V, `finalCharacterID` etc. in
/// `charSheetUtil.GetCombatEntries`; the client never reads this key, it
/// indexes a row by attribute name, so an extra one is harmless]. `finalBlow`
/// is 1 on the first and 0 on the rest.
pub fn attackers_value(r: &Report) -> PyValue {
    let one = |a: &Attacker, final_blow: i64| {
        crate::pyutil::keyval(vec![
            ("characterID", id(a.character)),
            ("corporationID", id(a.corporation)),
            ("allianceID", id(a.alliance)),
            ("factionID", id(a.faction)),
            ("shipTypeID", id(a.ship_type)),
            ("weaponTypeID", id(a.weapon_type)),
            ("securityStatus", real(a.security)),
            ("damageDone", PyValue::Int(a.damage)),
            ("finalBlow", PyValue::Int(final_blow)),
        ])
    };
    PyValue::List(
        std::iter::once(one(&r.final_blow, 1))
            .chain(r.attackers.iter().map(|a| one(a, 0)))
            .collect(),
    )
}

fn report_value_for(r: &Report, war_id: Option<i64>) -> PyValue {
    let f: &Attacker = &r.final_blow;
    crate::pyutil::keyval(vec![
        ("attackers", attackers_value(r)),
        ("killID", PyValue::Int(r.kill_id)),
        ("killTime", PyValue::Int(r.time)),
        ("solarSystemID", PyValue::Int(r.system)),
        ("moonID", PyValue::None),
        ("victimCharacterID", PyValue::Int(r.victim)),
        ("victimCorporationID", PyValue::Int(r.victim_corporation)),
        ("victimAllianceID", id(r.victim_alliance)),
        ("victimFactionID", PyValue::None),
        ("victimShipTypeID", PyValue::Int(r.ship_type)),
        ("victimDamageTaken", PyValue::Int(r.damage_taken)),
        ("finalCharacterID", id(f.character)),
        ("finalCorporationID", id(f.corporation)),
        ("finalAllianceID", id(f.alliance)),
        ("finalFactionID", id(f.faction)),
        ("finalShipTypeID", id(f.ship_type)),
        ("finalWeaponTypeID", id(f.weapon_type)),
        ("finalSecurityStatus", real(f.security)),
        ("finalDamageDone", PyValue::Int(f.damage)),
        ("warID", id(war_id)),
        ("iskLost", real(r.isk_lost)),
        ("iskDestroyed", real(r.isk_destroyed)),
        ("bountyClaimed", real(r.bounty_claimed)),
        ("loyaltyPoints", PyValue::None),
        ("killRightSupplied", id(r.kill_right_supplied)),
        ("killBlob", PyValue::Bytes(r.blob().into_bytes())),
    ])
}

/// The `(num, startIndex)` both lists take [V]: `num` is the page size
/// (clamped to [`MAX_PAGE`]), `startIndex` `None` or a kill id the page lies
/// below.
fn page_args(req: &Request<'_>) -> (usize, Option<i64>) {
    let num = match req.args.first() {
        Some(PyValue::Int(n)) => usize::try_from(*n).unwrap_or(0).min(MAX_PAGE),
        _ => MAX_PAGE,
    };
    let before = match req.args.get(1) {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    };
    (num, before)
}

/// `charMgr.GetRecentShipKillsAndLosses(num, startIndex)`: a list of the
/// caller's reports, newest first. No character, no reports.
pub(crate) fn recent_ship_kills_and_losses(
    reg: &Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Ok(PyValue::List(Vec::new()));
    };
    let (num, before) = page_args(req);
    Ok(PyValue::List(
        crate::killmail::involving(reg, actor, before, num)
            .iter()
            .map(|r| report_value_in(reg, r))
            .collect(),
    ))
}

/// **`corpRegistry.GetRecentKills(num, offset)` and `GetRecentLosses`** (C6B)
/// [V: `base_corporation.GetRecentKills` / `GetRecentLosses`, called by
/// `corp_ui_killreports.CorpKillReports` with `num = 25` and the same
/// paging as the character sheet's log]. Unlike the character panel the
/// corporation panel does **not** filter the answer: it draws every row it
/// is given, so the server picks them. Kills are the reports in which one of
/// the caller's corporation's pilots fired (the final blow or an attacker)
/// [A]; losses are those whose victim was in it. Open to every member (the
/// tab has no role gate [V]). `None` for any other method, so the corp
/// chain goes on.
pub(crate) fn corp_registry(
    reg: &Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    let losses = match req.method {
        "GetRecentKills" => false,
        "GetRecentLosses" => true,
        _ => return None,
    };
    let (num, before) = page_args(req);
    let corporation = req
        .actor
        .and_then(|a| reg.character(a))
        .map(|c| c.corporation_id)
        // REVIEW-FIX: an NPC corporation is a shared group of strangers; its
        // "report" would hand every starter pilot the others' attackers.
        .filter(|c| !crate::war::is_npc_corporation(*c));
    Some(Ok(PyValue::List(
        corporation
            .map(|c| crate::killmail::corp_involving(reg, c, losses, before, num))
            .unwrap_or_default()
            .iter()
            .map(|r| report_value_in(reg, r))
            .collect(),
    )))
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        "warStatisticMgr" => match req.method {
            // The war report's calls, bound on a warID (C8-b).
            "GetBaseInfo" | "GetKills" | "GetKillsByGroup" => {
                super::war::war_statistic(reg, req).unwrap_or_else(|| Err(reg.no_method(req)))
            }
            // SEC-FIX 6: the hash is checked. Every caller in the client
            // computes it from the kill it already holds
            // (`GetKillReportHashValue`) or reads it from a notification or
            // a chat link, so an id alone, guessed, opens nothing.
            "GetKillMail" => Ok(match (req.args.first(), req.args.get(1)) {
                (Some(PyValue::Int(kill_id)), Some(hash)) => crate::killmail::get(reg, *kill_id)
                    .filter(|r| {
                        crate::pyutil::as_bytes(hash).is_some_and(|h| {
                            h.eq_ignore_ascii_case(crate::notifications::kill_hash(r).as_bytes())
                        })
                    })
                    .as_ref()
                    .map_or(PyValue::None, |r| report_value_in(reg, r)),
                _ => PyValue::None,
            }),
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` gates the call. An error, not a panic.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::{Damage, Shot};

    const ALICE: i64 = 90_000_101;
    const BOB: i64 = 90_000_102;
    const ALICE_SHIP: i64 = 100_000_101;
    const BOB_SHIP: i64 = 100_000_102;
    /// An invented hull and an invented gun, and an invented module and
    /// cargo stack in Bob's hold.
    const HULL: i64 = 990_610_001;
    const GUN: i64 = 990_610_002;
    const MODULE: i64 = 990_610_003;
    const CARGO: i64 = 990_610_004;

    /// Alice and Bob undocked into one system, each in a ship that is an
    /// item, Bob's holding a fitted module and a cargo stack.
    fn world() -> Registry {
        let pilot = |id: i64, name: &str, ship: i64| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ship_type_id: HULL,
            ..crate::model::Character::default()
        };
        let mut reg = crate::testkit::registry_with(|c| {
            c.characters = vec![
                pilot(ALICE, "Alice", ALICE_SHIP),
                pilot(BOB, "Bob", BOB_SHIP),
            ];
        });
        let mut items = reg.store().items().to_vec();
        for i in items
            .iter_mut()
            .filter(|i| i.item_id == ALICE_SHIP || i.item_id == BOB_SHIP)
        {
            i.type_id = HULL;
        }
        let inside = |item_id: i64, type_id: i64, flag_id: i64, quantity: i64| crate::item::Item {
            item_id,
            type_id,
            owner_id: BOB,
            location_id: BOB_SHIP,
            flag_id,
            quantity,
            group_id: 1,
            category_id: 7,
            singleton: quantity == 1,
        };
        items.push(inside(100_000_201, MODULE, 11, 1));
        items.push(inside(100_000_202, CARGO, 5, 30));
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_000_203,
            ..crate::store::Snapshot::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        let _ = reg.take_pending();
        reg
    }

    /// One volley of `em` from `shooter` at `victim`.
    fn volley(reg: &mut Registry, shooter: i64, victim: i64, em: f64) {
        let t0 = crate::pyutil::filetime_now();
        let v = reg.roster().get(victim).copied().expect("in space");
        let from = reg.roster().get(shooter).map_or(0, |p| p.ship_id);
        let shot = Shot {
            at: t0,
            attacker_ship_id: from,
            weapon_item_id: 1,
            weapon_type_id: GUN,
            target_ship_id: v.ship_id,
            damage: Damage {
                em,
                explosive: 0.0,
                kinetic: 0.0,
                thermal: 0.0,
            },
            hit_quality: crate::combat::HIT_QUALITY,
        };
        crate::combat::fire(reg, shot, shooter, v, t0);
    }

    fn call(
        reg: &mut Registry,
        actor: i64,
        service: &str,
        method: &str,
        args: &[PyValue],
    ) -> PyValue {
        reg.dispatch(&Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(actor),
        })
        .expect("answered")
    }

    /// One attribute of a `KeyVal` (its state dict is stored value first).
    fn attr<'a>(v: &'a PyValue, name: &str) -> &'a PyValue {
        let PyValue::Instance { state, .. } = v else {
            panic!("not a KeyVal: {v:?}")
        };
        state
            .dict_as_kv()
            .and_then(|kv| {
                kv.into_iter().find_map(|(k, val)| {
                    (crate::pyutil::as_bytes(k) == Some(name.as_bytes())).then_some(val)
                })
            })
            .unwrap_or_else(|| panic!("no {name} on {v:?}"))
    }

    /// **A kill through the real path**: two volleys from Alice kill Bob's
    /// ship; the report is kept, both pilots' combat logs list it, the
    /// window's fields are filled, the blob carries Bob's items, the ledger
    /// row exists, and the tally is empty again.
    #[test]
    fn a_kill_is_reported_to_both_pilots() {
        let mut reg = world();
        volley(&mut reg, ALICE, BOB, 10.0);
        volley(&mut reg, ALICE, BOB, 1.0e9);
        assert_ne!(
            reg.roster().get(BOB).map(|p| p.ship_id),
            Some(BOB_SHIP),
            "setup: Bob's ship died"
        );
        assert!(
            crate::kill_hooks::PLAYER
                .iter()
                .any(|r| r.name == "killmail_on_kill"),
            "the report is a kill_hooks::PLAYER row"
        );
        assert_eq!(reg.ext::<crate::killmail::Tally>().ships(), 0);

        for who in [ALICE, BOB] {
            let PyValue::List(rows) = call(
                &mut reg,
                who,
                "charMgr",
                "GetRecentShipKillsAndLosses",
                &[PyValue::Int(25), PyValue::None],
            ) else {
                panic!("a list")
            };
            assert_eq!(rows.len(), 1, "{who} sees the kill");
            let r = &rows[0];
            assert_eq!(attr(r, "victimCharacterID"), &PyValue::Int(BOB));
            assert_eq!(attr(r, "finalCharacterID"), &PyValue::Int(ALICE));
            assert_eq!(attr(r, "victimShipTypeID"), &PyValue::Int(HULL));
            assert_eq!(attr(r, "finalShipTypeID"), &PyValue::Int(HULL));
            assert_eq!(attr(r, "finalWeaponTypeID"), &PyValue::Int(GUN));
            let PyValue::Int(dmg) = attr(r, "victimDamageTaken") else {
                panic!("damage")
            };
            assert_eq!(attr(r, "finalDamageDone"), &PyValue::Int(*dmg));
            assert!(*dmg > 0);
            let blob = crate::pyutil::as_bytes(attr(r, "killBlob")).expect("bytes");
            let blob = String::from_utf8_lossy(blob);
            assert!(blob.contains(&format!("t=\"{MODULE}\"")), "{blob}");
            assert!(blob.contains(&format!("t=\"{CARGO}\"")), "{blob}");
            assert!(blob.starts_with("<attackers></attackers>"), "{blob}");
        }

        // The window's link path opens the same report, with its hash.
        let kill = crate::killmail::involving(&reg, BOB, None, 1)[0].clone();
        let kill_id = kill.kill_id;
        // SEC-FIX 6: a guessed id, with no hash or a wrong one, opens nothing.
        for args in [
            vec![PyValue::Int(kill_id)],
            vec![PyValue::Int(kill_id), PyValue::Bytes(b"x".to_vec())],
            vec![PyValue::Int(kill_id), PyValue::None],
        ] {
            assert_eq!(
                call(&mut reg, ALICE, "warStatisticMgr", "GetKillMail", &args),
                PyValue::None,
                "{args:?}"
            );
        }
        let one = call(
            &mut reg,
            ALICE,
            "warStatisticMgr",
            "GetKillMail",
            &[
                PyValue::Int(kill_id),
                PyValue::Bytes(crate::notifications::kill_hash(&kill).into_bytes()),
            ],
        );
        assert_eq!(attr(&one, "killID"), &PyValue::Int(kill_id));
        assert_eq!(
            call(
                &mut reg,
                ALICE,
                "warStatisticMgr",
                "GetKillMail",
                &[PyValue::Int(kill_id + 99), PyValue::Bytes(b"x".to_vec())]
            ),
            PyValue::None
        );
    }

    /// **REL-FIX**: the row carries every attacker, the final blow first.
    #[test]
    fn a_report_row_lists_every_attacker() {
        let hit = |who: i64, damage: f64| crate::killmail::Hit {
            who,
            character: Some(who),
            corporation: Some(who + 1),
            alliance: None,
            ship_type: Some(HULL),
            weapon_type: Some(GUN),
            damage,
            last: 1,
        };
        let r = crate::killmail::build(
            1,
            1,
            1,
            crate::killmail::Victim {
                character: BOB,
                corporation: 1,
                alliance: None,
                ship_type: HULL,
            },
            &[hit(ALICE, 30.0), hit(77, 20.0)],
            Some(hit(ALICE, 30.0)),
            Vec::new(),
            |_| None,
            |_| None,
        );
        let row = report_value_for(&r, None);
        let PyValue::List(list) = attr(&row, "attackers") else {
            panic!("a list")
        };
        let who: Vec<_> = list
            .iter()
            .map(|a| attr(a, "characterID").clone())
            .collect();
        assert_eq!(who, vec![PyValue::Int(ALICE), PyValue::Int(77)]);
        assert_eq!(attr(&list[0], "finalBlow"), &PyValue::Int(1));
        assert_eq!(attr(&list[1], "finalBlow"), &PyValue::Int(0));
        assert_eq!(attr(&row, "finalCharacterID"), &PyValue::Int(ALICE));
    }

    /// Paging: ids below `startIndex`, newest first, at most `num`; a
    /// stranger sees nothing.
    #[test]
    fn the_combat_log_pages_by_kill_id() {
        let mut reg = world();
        for id in 1..=3 {
            let r = crate::killmail::build(
                id,
                id,
                1,
                crate::killmail::Victim {
                    character: BOB,
                    corporation: 1,
                    alliance: None,
                    ship_type: HULL,
                },
                &[],
                None,
                Vec::new(),
                |_| None,
                |_| None,
            );
            crate::killmail::put(&mut reg, &r);
        }
        let ids = |v: PyValue| -> Vec<PyValue> {
            let PyValue::List(rows) = v else {
                panic!("a list")
            };
            rows.iter().map(|r| attr(r, "killID").clone()).collect()
        };
        let page = call(
            &mut reg,
            BOB,
            "charMgr",
            "GetRecentShipKillsAndLosses",
            &[PyValue::Int(2), PyValue::None],
        );
        assert_eq!(ids(page), vec![PyValue::Int(3), PyValue::Int(2)]);
        let next = call(
            &mut reg,
            BOB,
            "charMgr",
            "GetRecentShipKillsAndLosses",
            &[PyValue::Int(2), PyValue::Int(2)],
        );
        assert_eq!(ids(next), vec![PyValue::Int(1)]);
        let other = call(
            &mut reg,
            ALICE,
            "charMgr",
            "GetRecentShipKillsAndLosses",
            &[PyValue::Int(25), PyValue::None],
        );
        assert!(ids(other).is_empty());
    }

    /// REVIEW-FIX: a pilot of an NPC corporation gets no corporation list.
    #[test]
    fn an_npc_corporation_has_no_kill_list() {
        const NPC_CORP: i64 = 1_000_009;
        let mut reg = world();
        reg.character_mut(ALICE).expect("Alice").corporation_id = NPC_CORP;
        let shooter = crate::killmail::Attacker {
            character: Some(BOB),
            corporation: Some(NPC_CORP),
            alliance: None,
            faction: None,
            ship_type: Some(HULL),
            weapon_type: Some(GUN),
            damage: 5,
            security: None,
        };
        let mut r = crate::killmail::build(
            1,
            1,
            1,
            crate::killmail::Victim {
                character: BOB,
                corporation: NPC_CORP,
                alliance: None,
                ship_type: HULL,
            },
            &[],
            None,
            Vec::new(),
            |_| None,
            |_| None,
        );
        r.final_blow = shooter;
        crate::killmail::put(&mut reg, &r);
        for method in ["GetRecentKills", "GetRecentLosses"] {
            let v = call(
                &mut reg,
                ALICE,
                "corpRegistry",
                method,
                &[PyValue::Int(25), PyValue::None],
            );
            assert!(
                matches!(v, PyValue::List(ref rows) if rows.is_empty()),
                "{method}"
            );
        }
    }

    /// C6B: `corpRegistry.GetRecentKills` and `GetRecentLosses` answer the
    /// caller's own corporation's reports, filtered by the server (the
    /// corporation panel draws every row it is given), paged by id; a
    /// stranger's corporation sees none of them, and another method is not
    /// this arm's.
    #[test]
    fn the_corp_kill_list_is_the_callers_corporations() {
        const FOREIGN: i64 = 98_000_777;
        let mut reg = world();
        // REVIEW-FIX: the list is a player corporation's; the default is an NPC one.
        for who in [ALICE, BOB] {
            reg.character_mut(who).expect("pilot").corporation_id = 98_000_440;
        }
        let mine = reg.character(ALICE).expect("Alice").corporation_id;
        let mut put = |id: i64, shooter_corp: i64, victim_corp: i64| {
            let shooter = crate::killmail::Attacker {
                character: Some(BOB),
                corporation: Some(shooter_corp),
                alliance: None,
                faction: None,
                ship_type: Some(HULL),
                weapon_type: Some(GUN),
                damage: 5,
                security: None,
            };
            let mut r = crate::killmail::build(
                id,
                id,
                1,
                crate::killmail::Victim {
                    character: ALICE,
                    corporation: victim_corp,
                    alliance: None,
                    ship_type: HULL,
                },
                &[],
                None,
                Vec::new(),
                |_| None,
                |_| None,
            );
            r.final_blow = shooter;
            crate::killmail::put(&mut reg, &r);
        };
        put(1, mine, FOREIGN);
        put(2, FOREIGN, mine);
        put(3, mine, FOREIGN);
        put(4, FOREIGN, FOREIGN);
        let ids = |v: PyValue| -> Vec<PyValue> {
            let PyValue::List(rows) = v else {
                panic!("a list")
            };
            rows.iter().map(|r| attr(r, "killID").clone()).collect()
        };
        let ask = |reg: &mut Registry, who: i64, method: &str, args: &[PyValue]| {
            ids(call(reg, who, "corpRegistry", method, args))
        };
        let page = [PyValue::Int(25), PyValue::None];
        assert_eq!(
            ask(&mut reg, ALICE, "GetRecentKills", &page),
            vec![PyValue::Int(3), PyValue::Int(1)]
        );
        assert_eq!(
            ask(&mut reg, BOB, "GetRecentLosses", &page),
            vec![PyValue::Int(2)],
            "any member of the corporation sees its list"
        );
        assert_eq!(
            ask(
                &mut reg,
                ALICE,
                "GetRecentKills",
                &[PyValue::Int(1), PyValue::Int(3)]
            ),
            vec![PyValue::Int(1)],
            "ids below the offset"
        );
        assert!(
            reg.dispatch(&Request {
                service: "corpRegistry",
                method: "GetRecentKills",
                args: &page,
                kwargs: &[],
                bind_params: None,
                actor: None,
            })
            .map(|v| ids(v).is_empty())
            .unwrap_or(true)
        );
    }

    /// C6B: a kill of the target's ship under an activated kill right is
    /// "on behalf of" the right's holder, and the window's two fields read
    /// it (`killRightSupplied`, and the bounty none).
    #[test]
    fn a_kill_under_a_kill_right_names_its_holder() {
        let reg = world();
        // Kill rights cannot be activated in null security: give the pilots'
        // system an invented high-security row.
        let sys = reg
            .character(ALICE)
            .and_then(|c| c.solar_system_id)
            .expect("Alice is in space");
        let mut content = crate::content::Content::fixture();
        content.insert(
            "universe",
            crate::celestial::Universe::parse(
                &[format!(
                    "{sys}\t19000001\t29000001\t\t\t0.75\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tx"
                )],
                &[],
                &[],
            )
            .expect("the invented row parses"),
        );
        let mut reg = reg.with_content(std::sync::Arc::new(content));
        let now = crate::pyutil::filetime_now();
        let kr = crate::crimewatch::killright::grant(&mut reg, BOB, ALICE, now).expect("granted");
        crate::crimewatch::killright::activate(&mut reg, Some(BOB), kr.id, now)
            .expect("Bob activates his right");
        volley(&mut reg, BOB, ALICE, 1.0e9);
        let rows = call(
            &mut reg,
            BOB,
            "charMgr",
            "GetRecentShipKillsAndLosses",
            &[PyValue::Int(25), PyValue::None],
        );
        let PyValue::List(rows) = rows else {
            panic!("a list")
        };
        assert_eq!(rows.len(), 1, "the kill is reported");
        assert_eq!(attr(&rows[0], "killRightSupplied"), &PyValue::Int(BOB));
        assert_eq!(attr(&rows[0], "bountyClaimed"), &PyValue::None);
        assert_eq!(attr(&rows[0], "loyaltyPoints"), &PyValue::None);
    }
}
