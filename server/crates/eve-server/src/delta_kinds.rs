//! Every delta kind, and the module that owns it (review packet IP2;
//! `docs/review/V1-architecture.md` F3 and P6, `docs/content/plan.md` §4.2).
//!
//! A kind is the first field of a `delta` row in the world file, so two
//! modules that picked the same string would share rows with no error. This
//! list is where a kind is claimed. **Coordinator-only**: a packet that adds a
//! kind asks for its row here, and its module exports `pub const KIND: &str`
//! and writes only through that (content plan CA16).
//!
//! [`crate::delta::Deltas::put`] debug-asserts that its kind is listed. A
//! load keeps an unlisted kind verbatim all the same (CP5's rule), through
//! [`crate::delta::Deltas::put_verbatim`], so a file from a newer build loses
//! nothing.
//!
//! The list is also part of the world-format version (IP11 hashes it), so
//! adding, renaming or removing a row is a format change.

/// `(kind, owning module)`, in plan §4.2's order. The owner is the module
/// that will write the kind; most of them do not exist yet, and their packet
/// is in the comment above each row.
pub const KINDS: &[(&str, &str)] = &[
    // DG-2 (#202): `char day slot -> progress state earned_isk` (state o/d/r),
    // and `char day slot key -> content_key`, a GM swap override.
    ("dgoal", "daily_goals"),
    // #205: `char -> charInfo portraitInfo`, the creator's doll as two hex
    // marshal streams (`paperdoll`), kept only once its shape is checked.
    ("pdoll", "paperdoll"),
    // HN-1 (#203): `next`; `r id -> running owner location system item type count price
    // restriction created expires status winner end_date is_copy`; `t id number -> owner running`.
    ("raffle", "hypernet"),
    // CAP-F: `char -> activation_until fatigue_until last_updated`, wall
    // FILETIMEs of a pilot's jump timers, only while one is running.
    ("jumpfat", "jump_fatigue"),
    // CP14: `belt_item_id n -> type_id x y z remaining`.
    ("rock", "asteroid"),
    // MUTA-1: item_id -> source_type mutator_type creator created_at rolls.
    ("dynitem", "dynamic_item"),
    // MN-C: `moon window n -> type_id remaining`, a moon belt rock that changed.
    ("moonrock", "moon_belt"),
    // CP21: `system_id archetype key k -> generation state at dungeon_id`.
    ("site", "population"),
    // CP30: `belt_item_id -> respawn_at`.
    ("beltwait", "belts"),
    // CP26a: `char agent -> mission_id state offer_seq accepted_at expires_at history`.
    ("mission", "mission"),
    // CP35: `station_id type_id side -> remaining touched_at`.
    ("npcorder", "econ_exchange"),
    // CP35: `station_id type_id side -> m`.
    ("npcmult", "econ_exchange"),
    // CP31: `type_id -> avg day`.
    ("mktidx", "econ_prices"),
    // CP38: `system_id -> d touched_at`.
    ("dbs", "econ_dbs"),
    // CP43: `day ref -> credit debit rows`, `day total -> faucet sink M_h
    // M_bots npc_fills trades dbs_min unclassified`, `cursor -> last_tx`.
    ("econ", "econ_ledger"),
    // CP37b: `char corp -> balance`.
    ("lp", "loyalty"),
    // CP41: `char arc_id -> state completed_at`.
    ("arc", "epic_arc"),
    // CP53b: `system_id -> score occupier taken_at`.
    ("fwvp", "facwar"),
    // #8 (FW-7): `system_id site_id -> score sides_occupier won_faction won_at`,
    // a battlefield scoreboard.
    ("fwsb", "fw_scoreboard"),
    // CP53a: `char -> faction since cooldown_until`, a direct enlistment.
    ("fwenlist", "facwar_mgr"),
    // CP53a: `char faction -> rank`, the last militia rank seen.
    ("fwrank", "facwar_mgr"),
    // CP53a: `owner -> faction status since`, a corp's or alliance's militia.
    ("fwcorp", "facwar_mgr"),
    // CP53a: `corp -> faction,...`, the militias a director allows.
    ("fwallow", "facwar_mgr"),
    // CP53a: `system_id -> points`, the iHub LP pool.
    ("fwlp", "facwar_mgr"),
    // FW-5: `group owner day -> kills vp`, a militia pilot's, corp's, alliance's
    // or militia's FW kills and victory points (day 0 is the lifetime row).
    ("fwstat", "fw_stats"),
    // FW-1: `char start_filetime -> corp`, a pilot's corporation history for
    // `corporationSvc.GetEmploymentRecord`.
    ("corphist", "corp"),
    // CP54a/b: `system_id key k -> dest_system expires_at mass_left
    // generation` (CP54a appends `generation`, the slot's draw index).
    ("wh", "wormhole"),
    // Y-ESCAL: `site_id -> system dungeon from_dungeon journal created
    // expires step chars`, a live escalation; `next -> n`, the counter.
    ("escal", "escalation"),
    // CP44: `ship_id -> owner type_id fraction start end`.
    ("insurance", "insurance"),
    // GAP-REPAIR: `ship_or_module_id -> armor_damage structure_damage`, the
    // damage a docked item carries until the repair shop mends it.
    ("hulldmg", "repair"),
    // CP48: `char faction level -> count`, the storyline trigger's counter.
    ("story", "storyline"),
    // CP48: `char mission_id -> completed_at`, a fixed-list mission done.
    ("listdone", "storyline"),
    // CP50: `char -> status`, a security status other than the clean 0.0.
    ("secstatus", "npc::bounty"),
    // SEC-1: `char seq -> eventTypeID eventDate modification newValue
    // otherOwnerID otherTypeID locationID start`, a security status history
    // row (the capped `GetSecurityStatusTransactions` list).
    ("sectx", "sec_status"),
    // E6: `char system -> amount due_at kills`, a bounty payout not yet paid.
    ("bountypay", "npc::bounty"),
    // ACC2: `user_id -> username role created scheme iterations salt hash`,
    // one account of `--world-accounts` (owner decision O9).
    ("account", "account_book"),
    // SOC-C6: `kill_id -> time system victim corp alliance hull damage
    // isk_lost isk_destroyed final attackers items`, and `next -> n`.
    ("killmail", "killmail"),
    // SEC-FIX 10: `character kill_id -> -`, the combat log's per-character
    // index, and a `ready` row once it covers every kept report.
    ("killindex", "killmail"),
    // SOC-C2: `owner contact -> relationship labelMask`, a corporation's or
    // an alliance's contact book.
    ("orgcontact", "group_contacts"),
    // SOC-C1: `allianceID -> executor creatorCorp creatorChar startDate
    // dictatorial deleted name shortName description url`.
    ("alliance", "alliance"),
    // SOC-C1: `corpID -> allianceID startDate chosenExecutor`.
    ("alliancecorp", "alliance"),
    // SOC-C1: `allianceID corpID -> state date text`.
    ("allianceapp", "alliance"),
    // SOC-C1: `corpID seq -> allianceID date`, a corp's alliance history.
    ("alliancehist", "alliance"),
    // CORP-W: `corp key -> cents`, a corporation wallet division's balance.
    ("corpwal", "corp_wallet"),
    // CORP-W: `corp seq -> key date ref o1 o2 reference cents balance d:description`.
    ("corpwjrn", "corp_wallet"),
    // CORP-W: `corp -> 14 x (n:name | -)`, hangar then wallet division names.
    ("corpwdiv", "corp_wallet"),
    // CORP-W: `character -> key`, the active wallet division.
    ("corpwkey", "corp_wallet"),
    // CORP-O: `office item id -> corp station solarsystem paid_until
    // impounded_at rented_at`, one rented office (the item is in the store).
    ("corpoff", "corp_office"),
    // SOC-D1: `systemID -> allianceID corpID hubID state claimTime
    // stateUntil [later packets' fields]`, one system's hub and claim.
    ("sov", "sov"),
    // SOC-D1: `allianceID -> currentHour newHour validAfter`, the prime hour.
    ("sovprime", "sov"),
    // SOC-D1: `seq -> systemID ownerID oldOwnerID changeTime`, recent claim
    // changes for the map.
    ("sovact", "sov"),
    // NOTIF-1: `receiver id -> type sender created processed data`, one
    // notification, and `next -> n`.
    ("notification", "notifications"),
    // MAIL-1: `id -> sender sent to_chars to_list to_corp title body` and
    // `next -> n`, one mail.
    ("mail", "mail"),
    // MAIL-1: `character id -> status labels`, one character's hold on a mail.
    ("mailbox", "mail"),
    // MAIL-1: `character label -> name color`, a custom mail label.
    ("maillabel", "mail"),
    // MAIL-1: `id -> key name owner access member_access cost welcome_title
    // welcome_body has_welcome` and `next -> n`, a mailing list.
    ("mlist", "mail"),
    // MAIL-1: `list character -> role`, a mailing list member.
    ("mlmember", "mail"),
    // MAIL-1: `list entity -> access`, a mailing list's access override.
    ("mlaccess", "mail"),
    // SOC-D4: `systemID -> military industrial militaryToday industrialToday
    // militaryUp industrialUp`, a claimed system's development indices.
    ("sovidx", "sov_index"),
    // SOC-D4: `allianceID -> currentCapital newCapital validAfter`, the
    // capital last announced.
    ("sovcap", "sov_index"),
    // SOC-D2: one row per launched hub: id, position, launcher, offline days.
    ("sovhub", "sov_hub"),
    // SOV-FUEL (#218): `hubID reagentTypeID -> amount last_updated per_hour carry`, a hub's reagent bay.
    ("sovrgt", "sov_hub_fuel"),
    // SKY-1: `skyhookID -> planetID systemID corpID active activatedAt builtUntil
    // secured unsecured lastCycle vulnStart vulnEnd state`, a skyhook.
    ("skyhook", "skyhook"),
    // SKY-2 (#82): `skyhookID -> start end linkState linkedShip linkStarted linkCompleteAt
    // thief n silo secure noticed`, a skyhook's theft window and its silo link.
    ("skyraid", "skyhook_theft"),
    // SOV-UPG-1 (#79): `hubID -> type:online:state,...`, a hub's installed upgrades in configured order.
    ("sovupg", "sov_upgrade"),
    // MERC-1: `denID -> owner skyhookID planetID systemID typeID enabled paused
    // devBase devAt anarchyBase anarchyAt infomorphs lastGenTick seen`, a den.
    ("mercden", "merc_den"),
    // MERC-2: `uuid hex -> denID templateNo started expiry siteID`, a den
    // activity (MTO); and the keys `gen <den>` and `site` for its clocks.
    ("mercact", "merc_den"),
    // SOV-WF (#219): `hubID -> inactive | import src... | export amount dest|- | transit`, a hub's workforce transfer.
    ("sovwf", "sov_workforce"),
    // SOC-D3a: `targetID -> systemID controlling scoring s0 t0 rate blocked`,
    // a hub's entosis score as last decided (the client extrapolates it).
    ("entosis", "entosis"),
    // SOC-D3a: `systemID -> hubID eventType defender startTime started
    // created attackers scores [D3b's fields]`, a reinforced hub's campaign.
    ("sovcamp", "sov_campaign"),
    // CW-1: `char -> level`, a ship safety setting other than green.
    ("cwsafety", "crimewatch"),
    // CW-1: `killRightID -> from to expiry price restrictedTo`, and
    // `next -> n`.
    ("killright", "crimewatch::killright"),
    // C8-a: `warID -> declaredBy against timeDeclared timeStarted timeFinished
    // retracted retractedBy billID mutual createdFrom openForAllies
    // reasonEnded warHQ reasonStarted notified`.
    ("war", "war"),
    // C8-a: `warID allyID -> timeStarted timeFinished told`.
    ("warally", "war"),
    // C8-a: `owner -> itemID stationID systemID`, a war HQ stand-in.
    ("warhq", "war"),
    // C8-a: `corp -> 0|1`, a GM override of the war permit.
    ("warpermit", "war"),
    // C8-b: `warID killID -> `, a kill filed under a war.
    ("warkill", "war"),
    // C8-a: `name -> n`, the war, negotiation, bill and HQ id counters.
    ("warnext", "war"),
    // C8-c: `negID -> warID type owner1 owner2 declaredBy against isk
    // description state created accepted declined retracted`.
    ("warneg", "war_deal"),
    // C8-c: `from to -> sent` a mutual-war invite; `owner -> 1` blocks them.
    ("warinv", "war_deal"),
    // C8-c: `owner other -> expiry reason warID`, a peace treaty.
    ("peace", "war_deal"),
    // C8-d: `billID -> debtor creditor amount due paid paidAt warID against`.
    ("warbill", "war_deal"),
    // C8-d: `owner -> 0|1`, whether war bills are paid when due.
    ("warauto", "war_deal"),
    // GAP-KILLMARKS: `ship_id -> n`, the kills a ship has made.
    ("killmark", "killmarks"),
    // GAP-CALENDAR: `ev id -> owner creator scope date duration title desc
    // importance auto deleted modified invitees`, `resp id char -> status`,
    // `next -> n`.
    ("calendar", "calendar"),
    // MAP-PERSONAL: `char system -> visits last_filetime`, the systems a human pilot entered.
    ("visit", "visits"),
    // GAP-REMAP: `char -> per cha int mem wil lastRemapFILETIME bonusRemaps`.
    ("remap", "skill_queue"),
    // BOOST: `char slot -> type expiry_filetime duration_ms side,side,...`, a
    // booster taken and not yet run out (`-` for no side effect).
    ("booster", "boosters"),
    // WS-1: `char -> points`, the unallocated skill points a pilot may spend.
    ("freesp", "free_sp"),
    // L4: `ledger key -> minted moved`, skill points that entered (an injector
    // drop) or only moved (extraction, injection).
    ("sptrade", "skill_trading"),
    // C8-c: `character -> corp until`, a pilot who left a corp at war.
    ("warleft", "war_deal"),
    // E1: `character -> career station parked_at settled_to day used_h
    // runs`, a logged-off bot's parked career (lazy settlement).
    ("agent", "econ_agents"),
    // SOC-D3b: `nodeID -> systemID hubSystemID hubID x y z spawnedAt` for a
    // command node of a running fight, and `seq -> next id offset`.
    ("cnode", "command_nodes"),
    // CAP-H: `ship_id -> fleetHangarFleet fleetHangarCorp smbFleet smbCorp`
    // (`1`/`0`), only for a ship whose switches differ from the default.
    ("shipcfg", "ship_config"),
    // GAPS-2: `item_id -> name`, the name a pilot gave a ship or a container.
    ("itemname", "item_names"),
    // GAPS-2: `char "i" owner_id -> text` (the note on a character or
    // corporation, `charMgr.SetNote`) and `char "n" note_id -> label text`
    // (the notepad's notes, `AddOwnerNote`).
    ("note", "notes"),
    // GAPS-2: `char label_bit -> name color`, a contact label of the
    // character's own address book.
    ("clabel", "contact_labels"),
    // GAP-ACCESS: `"g" group -> creator name desc`, `"m" group member -> type`,
    // `"next" -> id`: the access groups of `ownerGroupManager`.
    ("ogroup", "owner_groups"),
    // GAPS-2: `receiving_corp issuer_corp -> balance`, LP a corporation
    // holds (`LPSvc.TransferLP*`).
    ("corplp", "lp_transfer"),
    // GAPS-2: `clone_id -> owner ship_id installed_at`, a jump clone kept in
    // a ship (`jumpCloneSvc.AcceptShipCloneInstallation`).
    ("shipclone", "ship_clone"),
    // FITTING-MARKET: `owner fittingID -> shipTypeID savedDate name
    // description fitData`, a saved fitting of a character or corporation.
    ("fit", "fitting_store"),
    // BANK-PERSIST: `ship_id -> master:slave,slave ...`, a ship's weapon banks.
    ("wbank", "weapon_bank"),
    // MODULE-ONLINE: `module_id -> 1`, a fitted module taken offline.
    ("modoff", "modules"),
    // LOGIN-SPACE: `char -> ship system x y z`, where a ship left space when its
    // log-off hold ended, so the next login returns there.
    ("logoffspot", "logoff_spot"),
    // DEV-MODE: `char -> 1`, a character a `--dev` world has funded to 1 B.
    ("devtopup", "start"),
    // CHAR-BORN: `char -> created_at`, the FILETIME a character was made, so a
    // saved character keeps its age (age-limited boosters, the sheet, remaps).
    ("charborn", "start"),
    // SOC-CHARDEL: `char -> due done`, a character put up for
    // deletion (`done` 0, `due` the FILETIME it may be deleted from) or deleted
    // (`done` 1). A tombstone, not an erasure: mails, killmails and corp
    // history still resolve the name.
    ("chardel", "start"),
    // GATEWAY-2: `char "p" plan -> name description requirements`,
    // `char "a" -> plan` (the tracked plan), `char "m" milestone -> plan
    // train_type skill_type skill_level description`: a pilot's personal
    // skill plans, served by the public gateway.
    ("skillplan", "gateway_plans"),
    // GAP-BOUNTY: `target -> bounty updated_at`, the ISK placed on a pilot, corp or alliance.
    ("bountypool", "bounty_pool"),
    // GAP-BOUNTY: `contribution_id -> placer target amount at`; `next -> n`.
    ("bountygive", "bounty_pool"),
    // GAP-BOUNTY: `owner -> claimed kills`, the Top Hunters tab.
    ("bountyhunt", "bounty_pool"),
    // MINE-UI: `char day system type -> quantity wasted corp`, the Mining Ledger.
    ("mledger", "mining_ledger"),
    // SMALL-SOCIAL: `char key -> value`, a character setting
    // (`charMgr.SaveCharacterSetting`).
    ("csetting", "char_settings"),
    // S1-SHARE-LINKS: `hash sq -> type owner yaml`, a stored overview profile or
    // shared setting a chat link points at.
    ("sharelink", "share_links"),
    // SMALL-SOCIAL: `id -> owner create_char create_dt edit_char edit_dt
    // sort_order title body` and `next -> n`, a corp or alliance bulletin.
    ("bulletin", "bulletins"),
    // GAP-TRADE: `item_id -> source_location source_flag owner container`, an
    // item a direct trade holds; a load returns it to its source.
    ("tradestage", "trade"),
    // L1 NES-BACKEND: the PLEX vault. `bal user -> cents`, `log user seq ->
    // ts delta resulting key` (newest 200), `logn user -> n`, `led key -> in
    // out` (the PLEX ledger by source), `ref -> n` (invbroker references).
    ("nes", "nes_vault"),
    // L1 NES-BACKEND: the redeem queue. `tok user id -> char type qty offer
    // name message created from`, `next -> n`, `once user offer -> 1`.
    ("nestok", "nes"),
    // L2 SKINS: licences by licence type, never by SKIN id (ids swap between
    // builds). `lic char lic_type -> expires single_use`, `app ship -> lic_type
    // kept`.
    ("skin", "skins"),
    // EM-3 (#207): `own char ship_type cosmetic_type -> 1` (an emblem licence a
    // pilot owns), `slot ship_item backend_slot -> cosmetic_type writer` (an
    // enabled logo slot, lit only for its writer; dropped with the ship item).
    ("emblem", "emblem"),
    // U1-UPWELL: `structure_id -> type owner_corp system x y z yaw name bio
    // profile state upkeep shield armor hull timer_start timer_end timer_paused
    // unanchor_at reinforce_weekday reinforce_hour deed_present [next_weekday
    // next_hour apply_at vulnerable repairing first_aggressed last_aggressed
    // notified abandon_at abandon_warned deploy_start deploy_end]`, one Upwell
    // structure (the item is in the store).
    ("structure", "structure"),
    // ANSI-1 (#85): `structure_id -> destination_system`, where an Ansiblex
    // is aimed; a pair of gates aimed at each other is a link.
    ("jbridge", "jump_bridge"),
    // ANSI-2a (#86): `structure_id -> charge_gj settled_at counter`, an
    // Ansiblex's capacitor as last settled; the counter is the slim's etag.
    ("jbcap", "jump_bridge"),
    // U2-UPWELL: `p corp id -> name desc default`, `next -> n`, `rf corp ->
    // weekday hour`: a corporation's structure profiles and its default
    // reinforcement time.
    ("strprofile", "structure_profile"),
    // U2-UPWELL: `profile setting group -> value`, one setting row of a profile.
    ("strsetting", "structure_profile"),
    // UPWELL-FIT (#41): `structure module_id -> 1`, a structure service module
    // held online. A row whose module is no longer fitted there is ignored and
    // purged on the next write.
    ("strsvc", "structure_services"),
    // STR-FUEL (#42 part b): `structure_id -> last_burn_hour low_fuel_warned`,
    // the UTC hour (FILETIME) a structure's modules last paid their fuel for,
    // and whether the low-fuel mail went out for this crossing.
    ("strfuel", "structure_fuel"),
    // #53 MOON-SCHEDULE: `structure_id -> id moon system owner_corp started_by
    // started start_move chunk_available natural_decay duration_s yield_mult
    // calendar_event phase chunk_pos field_seq`, one running moon extraction
    // per drilling structure (`moon_extract::row`); `next -> n` is the next id.
    ("moonextract", "moon_extract"),
    // ASSET-SAFETY-1 (#49): `wrap_item_id -> owner system origin created
    // name`, an asset safety wrap of a structure that left space.
    ("safetywrap", "asset_safety"),
    // #57 CONTAINERS: `container which -> password`, a secure container's
    // general (1) or configuration (2) password, clear text.
    ("cpass", "containers"),
    // #82 MTU-DEPOT: `unit -> system x y z active_at decay_at`, a container
    // launched whole, a Mobile Depot or an MTU parked in space, so a restart
    // brings it back where it was.
    ("spunit", "mtu"),
    // #172 NW-2: `unit -> state until hurt`, an anchorable deployable's
    // anchoring state (`free`, `anchoring`, `anchored`, `unanchoring`), the
    // blue time a change ends, and the hull points it has lost. The unit's
    // place and clock are its `spunit` row.
    ("anchor", "anchor"),
    // #77 BE-2: `ship_id -> system x y z abandoned_at corp alliance`, a ship
    // left in space with no pilot (owner decision 2026-10-31: it stays until
    // it is destroyed or boarded and stolen, so it survives a restart). The
    // hull is in the store at (system, 0).
    ("pilotless", "pilotless"),
    // #89 ESS-1: `system -> value last_autopay next_autopay`, the ISK in a
    // system's ESS main bank and when it last and next pays out.
    ("essbank", "ess"),
    // #89 ESS-1: `system character -> amount`, what a pilot has put into
    // that bank since its last autopayment.
    ("esscon", "ess"),
    // #90 ESS-2: `system thief -> amount`, the theft ledger of a main bank.
    ("essthf", "ess"),
    // #162 SOC-CHAN1: `room -> name owner password motd`, a player-made chat
    // channel (`player_<n>`), and `#next -> n`, the counter its name comes from.
    // #173 SOC-CHAN2 appends `access acl limits` to the row (`allowed`,
    // `moderated` or `blocked`; `id:aff,..`; `kind:id:until,..`); an older row
    // has none of them and parses as an open channel. A kind added to a row,
    // not a meaning changed: no `STORE_FORMAT` bump.
    ("chanroom", "chat"),
    // #166 SOC-MEDALS: `m id -> corp creator date title description
    // (part graphic color)...` and `next -> n`, a corporation medal.
    ("corpmedal", "corp_medals"),
    // #166 SOC-MEDALS: `medal char n -> issuer date reason status`, one award.
    ("medalgot", "corp_medals"),
    // #102 CORP-ADS: `ad id -> corp created expires type_mask lang_mask
    // other_mask hour_mask min_sp title description recruiters` and
    // `next -> n`, a corporation recruitment advert.
    ("corpad", "corp_rec"),
    // #178 S7: `corp holder -> n`, the shares a pilot or a corp keeps of a
    // corp, and `corp # -> n`, its issued total (the founded marker).
    ("corpshare", "corp_shares"),
    // #103 CORP-VOTES: `next -> n`, `case id -> corp type creator start end
    // title description`, `opt id n -> text parameter parameter1 parameter2`,
    // `cast id voter -> option weight`, a corporation vote.
    ("corpvote", "corp_votes"),
    // #66 PG-3: Proving Grounds results. `s match_type schedule char -> wins
    // losses draws reached_at awarded`, `m seq -> match_type schedule at
    // char:result,...` (the match history), `next -> n`, `e match_type
    // schedule -> awarded_at` (an event's award marker).
    ("pgstat", "proving::results"),
];

/// Whether `kind` has a row in [`KINDS`] or in a module's kinds
/// ([`crate::module::plan`]).
pub fn is_registered(kind: &str) -> bool {
    crate::module::plan().kinds.iter().any(|(k, _)| *k == kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No two rows claim one kind, and every kind is a short lowercase ASCII
    /// word: the world file splits on tabs and escapes the rest, but a kind
    /// is also a name people grep for, so it stays plain.
    #[test]
    fn kinds_are_unique_and_lowercase() {
        let mut seen = std::collections::BTreeSet::new();
        for (kind, owner) in KINDS {
            assert!(seen.insert(*kind), "{kind:?} is claimed twice");
            assert!(!kind.is_empty(), "an empty kind");
            assert!(
                kind.bytes().all(|b| b.is_ascii_lowercase()),
                "{kind:?} is not lowercase ASCII letters"
            );
            assert!(!owner.is_empty(), "{kind:?} has no owner");
            assert!(is_registered(kind));
        }
        assert!(!is_registered("not-a-kind-yet"));
    }
}
