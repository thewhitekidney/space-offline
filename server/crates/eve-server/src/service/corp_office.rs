//! Corporation offices over the wire (CORP-O): the station's Offices panel,
//! the Rent and Unrent buttons, the corporation hangars with their seven
//! divisions, the deliveries hangar, and the rent that comes due on the game
//! clock. The state and the rules are `crate::corp_office`; the client
//! reading behind every arm is in `docs/lanes/corp-o.md` §2.
//!
//! **Doors in.** `corp.rs` hands `officeManager` and `itemLocking` to
//! [`office_manager`] and [`item_locking`]; `inventory.rs` offers every
//! `invbroker` call to [`inventory_call`] first, which answers `None` for
//! what is not an office's, a corporation hangar's or the deliveries
//! hangar's, and the ordinary arms run as ever (the way `planet.rs`'s
//! customs offices are served). The rollover runs [`on_rollover`].
//!
//! **Who may what** is `crate::corp_office::Standing`, the client's own bits:
//! rent needs `corpRoleCanRentOffice`, unrent and the impound release need
//! `corpRoleDirector`, a division's listing needs its query bit and a move in
//! or out its take bit. The deliveries hangar is for accountants, junior
//! accountants and traders to see, and accountants and traders to take from.
//!
//! **Money** is the corporation's Master Wallet, through CORP-W's
//! `corp_wallet::charge_fee`: the balance, the journal row (ref 13,
//! `refOfficeRentalFee`) and the push are one step there.

use super::*;
use crate::corp_office::{self as co, Office, Standing};
use crate::corp_wallet as cw;
use crate::item::{Change, Edit, Slot};

/// `appConst.refOfficeRentalFee` **[V]**: the journal reference of every
/// rent and release payment.
pub(crate) const REF_OFFICE_RENTAL_FEE: i64 = 13;

// ---------------------------------------------------------------------------
// Arguments and refusals
// ---------------------------------------------------------------------------

fn refuse(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
}

/// `CrpAccessDenied {reason}`, the client's own refusal for a role it lacks
/// **[V]**.
fn access_denied(reason: &str) -> ServiceError {
    refuse(
        "CrpAccessDenied",
        vec![("reason", PyValue::Unicode(reason.to_owned()))],
    )
}

fn custom_notify(text: &str) -> ServiceError {
    refuse("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

fn bad_args(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

/// An ISK argument, in cents so two floats compare by what they pay.
fn cents_arg(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Real(r) => Some(cw::to_cents(*r)),
        PyValue::Int(i) => Some(i.saturating_mul(100)),
        _ => None,
    }
}

/// The player corporation `character` is in and what they hold there:
/// `rolesAtAll | rolesAtOther` **[V]**. `None` in an NPC corp or a closed
/// one.
fn standing(reg: &Registry, character: i64) -> Option<(i64, Standing)> {
    let m = reg.corporations.membership(character)?;
    let corp = reg
        .corporations
        .get(m.corporation_id)
        .filter(|c| !c.deleted)?;
    Some((
        corp.corporation_id,
        Standing {
            roles: m.roles | m.roles_at_other,
        },
    ))
}

fn my_standing(reg: &Registry, req: &Request<'_>) -> Result<(i64, i64, Standing), ServiceError> {
    let actor = req
        .actor
        .ok_or_else(|| access_denied("There is no character to act for."))?;
    let (corp, s) = standing(reg, actor)
        .ok_or_else(|| access_denied("Only a member of a player corporation can do that."))?;
    Ok((actor, corp, s))
}

/// The station a bound `officeManager` call is about: the bare station id
/// **[V]** (`Moniker('officeManager', stationID)`).
fn station_of(req: &Request<'_>) -> Option<i64> {
    match req.bind_params? {
        PyValue::Int(id) => Some(*id),
        PyValue::Tuple(p) | PyValue::List(p) => match p.first()? {
            PyValue::Int(id) => Some(*id),
            _ => None,
        },
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Pushes
// ---------------------------------------------------------------------------

/// `OnOfficeRentalChange(corporationID, officeID)`: `officeManager` clears
/// its station and corporation office caches, then tells the lobby and the
/// inventory windows (`officeManager.OnOfficeRentalChange`) **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OfficeRentalChange {
    pub to: Vec<i64>,
    pub corp: i64,
    pub office: i64,
}

impl crate::notify::Push for OfficeRentalChange {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnOfficeRentalChange",
            vec![PyValue::Int(self.corp), PyValue::Int(self.office)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// How a push leaves: with the answer of the call that made it, or on the
/// clock (the rollover).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    Call,
    Clock,
}

fn tell(reg: &mut Registry, via: Via, n: impl crate::notify::Push + 'static) {
    match via {
        Via::Call => reg.push(n),
        Via::Clock => reg.schedule(Duration::ZERO, n),
    }
}

/// Tell the corporation's members, and whoever is docked at the station
/// (whose Offices panel lists the corporation), that an office changed.
fn announce(reg: &mut Registry, via: Via, o: &Office) {
    let mut to = reg.corporations.members_of(o.corp);
    to.extend(
        reg.characters_at_station(o.station)
            .iter()
            .map(|c| c.character_id),
    );
    to.sort_unstable();
    to.dedup();
    tell(
        reg,
        via,
        OfficeRentalChange {
            to,
            corp: o.corp,
            office: o.office_id,
        },
    );
}

/// Tell the members who may look into `flag` (or every member, for `None`)
/// that `locations` changed, so an open division window asks again.
fn invalidate_readers(
    reg: &mut Registry,
    corp: i64,
    flag: Option<i64>,
    skip: i64,
    locations: Vec<i64>,
) {
    let to: Vec<i64> = reg
        .corporations
        .members_of(corp)
        .into_iter()
        .filter(|id| *id != skip)
        .filter(|id| {
            standing(reg, *id).is_some_and(|(_, s)| match flag {
                Some(f) => s.can_query(f),
                None => true,
            })
        })
        .collect();
    if to.is_empty() {
        return;
    }
    reg.push(crate::notify::Addressed {
        to: Some(to),
        inner: Box::new(crate::notify::InvalidateLocations {
            location_ids: locations,
        }),
    });
}

// ---------------------------------------------------------------------------
// The rental
// ---------------------------------------------------------------------------

fn station_system(reg: &Registry, station: i64, fallback: Option<i64>) -> (i64, i64) {
    match reg.station_ref(station) {
        Some(s) => (s.solar_system_id, s.type_id),
        None => (fallback.unwrap_or(0), 0),
    }
}

/// The checks a quote and a rental share, in the client's own words.
fn may_rent(reg: &Registry, req: &Request<'_>, station: i64) -> Result<(i64, i64), ServiceError> {
    let (actor, corp, s) = my_standing(reg, req)?;
    if !s.can_rent() {
        return Err(refuse(
            "RentingOfficeQuotesOnlyGivenToActiveCEOsOrEquivale",
            vec![],
        ));
    }
    if reg.docked_station(actor) != Some(station) {
        return Err(custom_notify(
            "You have to be docked at the station to rent an office there.",
        ));
    }
    if co::live_at(reg.deltas(), corp, station).is_some() {
        return Err(refuse("RentingYouHaveAnOfficeHere", vec![]));
    }
    if co::free_at(reg.deltas(), station) == 0 {
        return Err(refuse("NoOfficesAreAvailableForRenting", vec![]));
    }
    Ok((actor, corp))
}

/// `officeManager.GetPriceQuote(corpID)`: the rent for one period.
fn price_quote(reg: &Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let station = station_of(req).ok_or_else(|| bad_args(req, "bound on a station"))?;
    let (_, corp) = may_rent(reg, req, station)?;
    if let Some(PyValue::Int(asked)) = req.args.first()
        && *asked != corp
    {
        return Err(access_denied("A quote is for your own corporation."));
    }
    Ok(PyValue::Real(co::OFFICE_RENT_ISK))
}

/// The one period of rent, in wall ticks, on the game clock.
fn period(reg: &Registry) -> i64 {
    crate::game_clock::span(reg, co::RENT_PERIOD_DAYS * co::DAY_TICKS)
}

/// `officeManager.RentOffice(cost)`: pay the Master Wallet's rent and mint
/// the office. A `cost` that is not today's quote is refused with the real
/// one (`RentingAnOfficeCostsMore {amount}`) **[V]**.
fn rent(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let station = station_of(req).ok_or_else(|| bad_args(req, "bound on a station"))?;
    let (actor, corp) = may_rent(reg, req, station)?;
    let quote = cw::to_cents(co::OFFICE_RENT_ISK);
    match cents_arg(req.args.first()) {
        Some(c) if c == quote => {}
        Some(_) => {
            return Err(refuse(
                "RentingAnOfficeCostsMore",
                vec![("amount", PyValue::Real(co::OFFICE_RENT_ISK))],
            ));
        }
        None => return Err(bad_args(req, "(cost)")),
    }
    super::corp_wallet::check_fee(reg, corp, cw::MASTER_KEY, co::OFFICE_RENT_ISK)?;
    let (system, _) = station_system(
        reg,
        station,
        reg.character(actor).and_then(|c| c.solar_system_id),
    );
    let change = reg
        .apply_items_quiet(&[Edit::Create {
            type_id: co::TYPE_OFFICE,
            quantity: 1,
            to: Slot::new(corp, station, co::FLAG_OFFICE),
        }])
        .map_err(|e| {
            crate::outln!("[office] could not mint the office item: {e}");
            custom_notify("The server could not open that office.")
        })?;
    let Some(office_id) = change.iter().find_map(|c| match c {
        Change::Created { item_id, .. } => Some(*item_id),
        _ => None,
    }) else {
        return Err(custom_notify("The server could not open that office."));
    };
    let ceo = reg.corporations.get(corp).map_or(actor, |c| c.ceo_id);
    // The balance was checked above, so this cannot come back short.
    if super::corp_wallet::charge_fee(
        reg,
        corp,
        cw::MASTER_KEY,
        co::OFFICE_RENT_ISK,
        REF_OFFICE_RENTAL_FEE,
        ceo,
        station,
    )
    .is_none()
    {
        // Put the item back: no rent was taken, so no office exists.
        let _ = reg.apply_items_quiet(&[Edit::Destroy { item_id: office_id }]);
        return Err(custom_notify("The rent could not be paid."));
    }
    let now = crate::pyutil::filetime_now();
    let o = Office {
        office_id,
        corp,
        station,
        system,
        paid_until: now.saturating_add(period(reg)),
        impounded_at: 0,
        rented_at: now,
    };
    co::put(reg.deltas_mut(), &o);
    reg.dirty = true;
    announce(reg, Via::Call, &o);
    Ok(PyValue::None)
}

/// Every item at or under `root`, children first, so a destroy plan never
/// leaves a child pointing at a parent that is gone.
fn descendants(reg: &Registry, root: i64) -> Vec<i64> {
    let mut out = Vec::new();
    let mut frontier = vec![root];
    while let Some(id) = frontier.pop() {
        for i in reg.store().contents_of(id) {
            out.push(i.item_id);
            frontier.push(i.item_id);
        }
    }
    out.reverse();
    out
}

fn has_contents(reg: &Registry, office_id: i64) -> bool {
    reg.store().contents_of(office_id).next().is_some()
}

/// The rental is over. Empty: the office item and its row go. With items in
/// it: the item's flag becomes `flagImpounded` and the row records when, so
/// `HasCorpImpoundedItems` is true and the corporation can pay to have the
/// items back **[V]** (`OfficesPanel._load_buttons`, the Unrent
/// confirmation `crpUnrentOfficeWithContent`).
fn end_rental(reg: &mut Registry, via: Via, mut o: Office, now: i64) {
    if has_contents(reg, o.office_id) {
        let moved = reg.apply_items_quiet(&[Edit::Move {
            item_id: o.office_id,
            to: Slot::new(o.corp, o.station, co::FLAG_IMPOUNDED),
        }]);
        if moved.is_ok() {
            o.impounded_at = now.max(1);
            co::put(reg.deltas_mut(), &o);
        }
    } else {
        let _ = reg.apply_items_quiet(&[Edit::Destroy {
            item_id: o.office_id,
        }]);
        co::remove(reg.deltas_mut(), o.office_id);
    }
    reg.dirty = true;
    announce(reg, via, &o);
}

fn my_live_office_here(reg: &Registry, corp: i64, station: i64) -> Result<Office, ServiceError> {
    co::live_at(reg.deltas(), corp, station).ok_or_else(|| refuse("NoOfficeAtStation", vec![]))
}

/// `officeManager.UnrentOffice()`, bound on the station (also from afar:
/// `UnrentRemoteOffice`) **[V]**. A director's.
fn unrent(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let station = station_of(req).ok_or_else(|| bad_args(req, "bound on a station"))?;
    let (_, corp, s) = my_standing(reg, req)?;
    if !s.can_unrent() {
        return Err(access_denied("Only a director can give up an office."));
    }
    let o = my_live_office_here(reg, corp, station)?;
    end_rental(reg, Via::Call, o, crate::pyutil::filetime_now());
    Ok(PyValue::None)
}

fn impounded_here(reg: &Registry, corp: i64, station: i64) -> Option<Office> {
    co::impounded_at(reg.deltas(), corp, station)
}

/// `officeManager.GetItemsFromImpound(quotedPrice)`: pay the release price
/// from the Master Wallet and have the impounded items in the caller's
/// hangar here **[A]** (the client does not say where they land). A director's.
fn release(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let station = station_of(req).ok_or_else(|| bad_args(req, "bound on a station"))?;
    let (actor, corp, s) = my_standing(reg, req)?;
    if !s.can_release() {
        return Err(access_denied(
            "Only a director can release impounded items.",
        ));
    }
    if reg.docked_station(actor) != Some(station) {
        return Err(refuse("NoOfficeAtStation", vec![]));
    }
    let o =
        impounded_here(reg, corp, station).ok_or_else(|| refuse("NoOfficeAtStation", vec![]))?;
    let price = co::release_price();
    match cents_arg(req.args.first()) {
        Some(c) if c == cw::to_cents(price) => {}
        Some(_) => {
            return Err(refuse(
                "RentingAnOfficeCostsMore",
                vec![("amount", PyValue::Real(price))],
            ));
        }
        None => return Err(bad_args(req, "(quotedPrice)")),
    }
    super::corp_wallet::check_fee(reg, corp, cw::MASTER_KEY, price)?;
    // The top level of what the office held; a container's contents go
    // with it.
    let mut plan: Vec<Edit> = reg
        .store()
        .contents_of(o.office_id)
        .map(|i| Edit::Move {
            item_id: i.item_id,
            to: Slot::new(actor, station, crate::model::FLAG_HANGAR),
        })
        .collect();
    plan.push(Edit::Destroy {
        item_id: o.office_id,
    });
    reg.apply_items(&plan).map_err(|e| {
        crate::outln!("[office] impound release refused: {e}");
        custom_notify("The server could not release those items.")
    })?;
    // The balance was checked above, so this cannot come back short.
    let _ = super::corp_wallet::charge_fee(
        reg,
        corp,
        cw::MASTER_KEY,
        price,
        REF_OFFICE_RENTAL_FEE,
        actor,
        station,
    );
    co::remove(reg.deltas_mut(), o.office_id);
    reg.dirty = true;
    announce(reg, Via::Call, &o);
    Ok(PyValue::None)
}

/// `officeManager.TrashImpoundedOffice()`: destroy what was impounded. A
/// director's.
fn trash(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let station = station_of(req).ok_or_else(|| bad_args(req, "bound on a station"))?;
    let (_, corp, s) = my_standing(reg, req)?;
    if !s.can_release() {
        return Err(access_denied("Only a director can trash impounded items."));
    }
    let o =
        impounded_here(reg, corp, station).ok_or_else(|| refuse("NoOfficeAtStation", vec![]))?;
    let mut plan: Vec<Edit> = descendants(reg, o.office_id)
        .into_iter()
        .map(|item_id| Edit::Destroy { item_id })
        .collect();
    plan.push(Edit::Destroy {
        item_id: o.office_id,
    });
    reg.apply_items_quiet(&plan).map_err(|e| {
        crate::outln!("[office] trashing the impound refused: {e}");
        custom_notify("The server could not trash those items.")
    })?;
    co::remove(reg.deltas_mut(), o.office_id);
    reg.dirty = true;
    announce(reg, Via::Call, &o);
    Ok(PyValue::None)
}

/// `GetMyCorporationsOffices` rows: `officeID`, `stationID`, `solarsystemID`
/// (lowercase `s`) and `stationTypeID`, the attributes the client reads off
/// an office (`officeManager`, `corp_ui_home`) **[V]**.
fn my_offices(reg: &Registry, actor: Option<i64>) -> PyValue {
    let Some((corp, _)) = actor.and_then(|a| standing(reg, a)) else {
        return PyValue::List(vec![]);
    };
    let header = ["officeID", "stationID", "solarsystemID", "stationTypeID"];
    PyValue::List(
        co::live_of(reg.deltas(), corp)
            .into_iter()
            .filter_map(|o| {
                let (_, station_type) = station_system(reg, o.station, None);
                eve_rowset::Row::new(
                    &header,
                    vec![
                        PyValue::Int(o.office_id),
                        PyValue::Int(o.station),
                        PyValue::Int(o.system),
                        PyValue::Int(station_type),
                    ],
                )
                .ok()
                .map(|r| r.to_pyvalue())
            })
            .collect(),
    )
}

/// `officeManager`, bound on a station (the Offices panel, the rent and
/// unrent buttons) and not (`GetMyCorporationsOffices`).
pub(crate) fn office_manager(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    match req.method {
        // A list of corporation ids, never `None`: `officeManager.offices`
        // stores the answer and *then* primes `cfg.eveowners` with it, so a
        // `None` is cached and raises inside the Offices panel's own
        // swallowed `except`, and the panel spins for ever **[V]**.
        "GetCorporationsWithOffices" => {
            let station = station_of(req).ok_or_else(|| bad_args(req, "bound on a station"))?;
            Ok(PyValue::List(
                co::corps_at(reg.deltas(), station)
                    .into_iter()
                    .map(PyValue::Int)
                    .collect(),
            ))
        }
        "GetMyCorporationsOffices" => Ok(my_offices(reg, req.actor)),
        "GetEmptyOfficeCount" => {
            let station = station_of(req).ok_or_else(|| bad_args(req, "bound on a station"))?;
            Ok(PyValue::Int(co::free_at(reg.deltas(), station) as i64))
        }
        "GetPriceQuote" => price_quote(reg, req),
        "RentOffice" => rent(reg, req),
        "UnrentOffice" => unrent(reg, req),
        // `GetCorpOfficeAtLocation` calls it once to warm the office item;
        // the result is discarded.
        "PrimeOfficeItem" => Ok(PyValue::None),
        "HasCorpImpoundedItems" => {
            let station = station_of(req).ok_or_else(|| bad_args(req, "bound on a station"))?;
            Ok(PyValue::Bool(
                req.actor
                    .and_then(|a| standing(reg, a))
                    .is_some_and(|(corp, _)| impounded_here(reg, corp, station).is_some()),
            ))
        }
        "GetImpoundReleasePrice" => Ok(PyValue::Real(co::release_price())),
        "GetItemsFromImpound" => release(reg, req),
        "TrashImpoundedOffice" => trash(reg, req),
        _ => Err(reg.no_method(req)),
    }
}

fn which_arg(v: Option<&PyValue>) -> String {
    match v {
        Some(PyValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
        Some(PyValue::Unicode(u)) => u.clone(),
        _ => String::new(),
    }
}

/// `corpmgr.GetAssetInventory(corpID, which)`: the Corporation > Assets
/// tab's list of places, one row per location **[V]**
/// (`corp_ui_accounts.ShowAssets`). The client reads `locationID` and
/// `solarsystemID` off every row (`GetRegions`, `GetLocationData`),
/// `itemCount` when the row has the attribute (the deliveries tabs show and
/// sort by it) and `typeID` only through `getattr` (a structure's type).
/// An answer of `None` raised in `GetRegions` and froze the window.
///
/// `offices` are the live rentals (an empty one is shown as an unused
/// office), `impounded` the lapsed ones, `deliveries` the stations holding
/// the corporation's deliveries hangar. `property` (in space),
/// `capsuleerdeliveries` and `assetwraps` have no source here: this server
/// puts no corporation item in space, has no capsuleer deliveries and no
/// asset safety, so their genuinely empty rowset is the true answer.
/// Only a member of the corporation asked about sees anything.
pub(crate) fn asset_inventory(reg: &Registry, req: &Request<'_>) -> eve_rowset::CRowset {
    let which = which_arg(req.args.get(1));
    let mine = req.actor.and_then(|a| standing(reg, a)).map(|(c, _)| c);
    let asked = opt_id(req.args.first());
    let counted = which == "deliveries";
    let mut cols = vec![("locationID", DbType::I8), ("solarsystemID", DbType::I4)];
    if counted {
        cols.push(("itemCount", DbType::I4));
    }
    let descriptor = DbRowDescriptor::new(
        cols.into_iter()
            .map(|(n, t)| Column {
                name: n.as_bytes().to_vec(),
                db_type: t,
            })
            .collect(),
    )
    .expect("the asset location columns are a valid layout");
    let mut rowset = eve_rowset::CRowset::new(descriptor);
    let mut put = |station: i64, system: i64, count: i64| {
        let mut v = vec![DbValue::Int(station), DbValue::Int(system)];
        if counted {
            v.push(DbValue::Int(count));
        }
        rowset
            .push_values(v)
            .expect("the asset row matches its descriptor");
    };
    if let Some(corp) = mine.filter(|c| Some(*c) == asked) {
        match which.as_str() {
            "offices" => {
                for o in co::live_of(reg.deltas(), corp) {
                    put(o.station, o.system, 0);
                }
            }
            "impounded" => {
                for o in co::all(reg.deltas())
                    .into_iter()
                    .filter(|o| o.corp == corp && !o.live())
                {
                    put(o.station, o.system, 0);
                }
            }
            "deliveries" => {
                let mut by_station: std::collections::BTreeMap<i64, i64> = Default::default();
                for i in reg
                    .store()
                    .owned_by(corp)
                    .filter(|i| i.flag_id == co::FLAG_CORP_DELIVERIES)
                {
                    *by_station.entry(i.location_id).or_default() += 1;
                }
                for (station, n) in by_station {
                    let (system, _) = station_system(reg, station, None);
                    put(station, system, n);
                }
            }
            _ => {}
        }
    }
    rowset
}

/// `corpmgr.GetAssetInventoryForLocation(corpID, locationID, which)`: what
/// a location group of the Assets tab holds when it is opened, a `CRowset`
/// of item rows (the client appends virtual columns to `items.header`)
/// **[V]**. `offices` is the divisions' items (the client groups them by
/// `flagID` and asks again per division), `impounded` the lapsed office's
/// contents, `deliveries` the deliveries hangar. Division items need the
/// asker's query bit, as the office's own inventory does.
pub(crate) fn asset_inventory_for_location(
    reg: &Registry,
    req: &Request<'_>,
) -> eve_rowset::CRowset {
    let which = which_arg(req.args.get(2));
    let mut rowset = eve_rowset::CRowset::new(crate::service::item_row_descriptor());
    let seen = req.actor.and_then(|a| standing(reg, a));
    let (Some((corp, s)), Some(place)) = (seen, opt_id(req.args.get(1))) else {
        return rowset;
    };
    if opt_id(req.args.first()) != Some(corp) {
        return rowset;
    }
    let items: Vec<&crate::item::Item> = match which.as_str() {
        "offices" | "impounded" => {
            let office = if which == "offices" {
                co::live_at(reg.deltas(), corp, place)
            } else {
                co::impounded_at(reg.deltas(), corp, place)
            };
            office
                .into_iter()
                .flat_map(|o| reg.store().contents_of(o.office_id))
                .filter(|i| i.owner_id == corp && s.can_query(i.flag_id))
                .collect()
        }
        "deliveries" if s.sees_deliveries() => reg
            .store()
            .owned_by(corp)
            .filter(|i| i.location_id == place && i.flag_id == co::FLAG_CORP_DELIVERIES)
            .collect(),
        _ => Vec::new(),
    };
    for i in items {
        rowset
            .push_row(reg.item_row(i))
            .expect("an item row has the item descriptor");
    }
    rowset
}

/// `itemLocking`: no item is locked. `IsItemLocked` and `IsItemIDLocked` ask
/// `GetItemsByLocation(locationID)` for every item in a corporation office
/// and for every drag (`invCache.Add`), then test `itemID in` the answer; a
/// service this server did not answer was a failed call on each of them
/// **[V]** (`itemLocking.py`). Nothing here locks an item, so the answer is
/// an empty container.
pub(crate) fn item_locking(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "GetItemsByLocation" | "GetLockedItemLocations" => Ok(PyValue::List(vec![])),
        _ => Err(reg.no_method(req)),
    }
}

// ---------------------------------------------------------------------------
// The rent comes due
// ---------------------------------------------------------------------------

/// The rollover step: bill every office whose rent is due out of its
/// corporation's Master Wallet, and let the rental lapse where the wallet
/// cannot pay (empty offices vanish; the others are impounded). Answers how
/// many offices were billed or lapsed.
///
/// **Day granularity [A]**: the rollover runs once per game-day, so a rental
/// ends within a game-day of its date. A lapse is not graced: the bill is
/// paid in full on the day, or the office lapses on it. Falsified by a corp
/// that expects a second chance.
pub(crate) fn on_rollover(reg: &mut Registry, now: i64) -> usize {
    let due: Vec<Office> = co::all(reg.deltas())
        .into_iter()
        .filter(|o| o.due(now))
        .collect();
    let span = period(reg);
    let mut touched = 0;
    for o in due {
        let alive = reg.corporations.get(o.corp).is_some_and(|c| !c.deleted);
        let ceo = reg.corporations.get(o.corp).map_or(0, |c| c.ceo_id);
        let paid = alive
            && super::corp_wallet::charge_fee(
                reg,
                o.corp,
                cw::MASTER_KEY,
                co::OFFICE_RENT_ISK,
                REF_OFFICE_RENTAL_FEE,
                ceo,
                o.station,
            )
            .is_some();
        if paid {
            let renewed = Office {
                paid_until: co::next_paid_until(o.paid_until, now, span),
                ..o
            };
            co::put(reg.deltas_mut(), &renewed);
            reg.dirty = true;
            announce(reg, Via::Clock, &renewed);
        } else {
            end_rental(reg, Via::Clock, o, now);
        }
        touched += 1;
    }
    touched
}

// ---------------------------------------------------------------------------
// The hangars
// ---------------------------------------------------------------------------

/// Where a corporation's item is kept, for the rules about moving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kept {
    /// In division `flag` of the live office `office`.
    Division { office: Office, flag: i64 },
    /// In the corporation's deliveries hangar at `station`.
    Deliveries { corp: i64, station: i64 },
}

/// Where `item` is kept by a corporation, if it is in an office's division or
/// in a deliveries hangar. An impounded office's contents are not reachable.
fn kept(reg: &Registry, item_id: i64) -> Option<Kept> {
    let i = reg.item(item_id)?;
    if i.flag_id == co::FLAG_CORP_DELIVERIES {
        return Some(Kept::Deliveries {
            corp: i.owner_id,
            station: i.location_id,
        });
    }
    let office = co::get(reg.deltas(), i.location_id)?;
    (office.live() && co::division_of_flag(i.flag_id).is_some()).then_some(Kept::Division {
        office,
        flag: i.flag_id,
    })
}

/// May `s` (in `corp`) move an item out of where it is kept?
fn may_take_from(corp: i64, s: Standing, k: Kept) -> Result<(), ServiceError> {
    match k {
        Kept::Division { office, flag } => {
            if office.corp != corp || !s.can_take(flag) {
                return Err(access_denied(
                    "You do not have the role to take from this division.",
                ));
            }
        }
        Kept::Deliveries { corp: owner, .. } => {
            if owner != corp || !s.can_take_deliveries() {
                return Err(access_denied(
                    "You do not have the role to take from the deliveries hangar.",
                ));
            }
        }
    }
    Ok(())
}

/// A stack of `qty` (or the whole item) moved to `to`.
fn move_edit(
    reg: &Registry,
    item_id: i64,
    qty: Option<i64>,
    to: Slot,
) -> Result<Edit, ServiceError> {
    let i = reg
        .item(item_id)
        .ok_or_else(|| custom_notify("That item is no longer there."))?;
    Ok(match qty {
        Some(q) if q <= 0 => return Err(custom_notify("There is nothing to move.")),
        Some(q) if q < i.quantity && !i.singleton => Edit::Split {
            item_id,
            quantity: q,
            to,
        },
        _ => Edit::Move { item_id, to },
    })
}

fn plan_failed(e: crate::item::EditError) -> ServiceError {
    crate::outln!("[office] item plan refused: {e}");
    custom_notify("The server could not move that item.")
}

/// Put an item into division `flag` of `office`: the drag onto a division
/// (`Add(itemID, sourceID, qty=, flag=)` on the office's inventory). The take
/// bit of the division is the gate, as the client's window is view-only
/// without it (`SetAccess`) **[V]**.
fn deposit(
    reg: &mut Registry,
    actor: i64,
    office: &Office,
    item_id: i64,
    qty: Option<i64>,
    flag: Option<i64>,
) -> Result<(), ServiceError> {
    let Some((corp, s)) = standing(reg, actor) else {
        return Err(access_denied(
            "Only a member of a player corporation can do that.",
        ));
    };
    if corp != office.corp {
        return Err(access_denied("That office belongs to another corporation."));
    }
    let flag = flag
        .filter(|f| co::division_of_flag(*f).is_some())
        .ok_or_else(|| custom_notify("Put it in one of the corporation's hangar divisions."))?;
    if !s.can_take(flag) {
        return Err(access_denied("You do not have the role for this division."));
    }
    if reg.docked_station(actor) != Some(office.station) {
        return Err(refuse("NoOfficeAtStation", vec![]));
    }
    // The active ship is never moved out from under its pilot
    // (`invCache.Add` raises the same key before the call).
    if reg.character(actor).is_some_and(|c| c.ship_id == item_id) {
        return Err(refuse("CanNotPlaceActiveShipInAnotherHangar", vec![]));
    }
    let Some(item) = reg.item(item_id).cloned() else {
        return Err(custom_notify("That item is no longer there."));
    };
    let from_hangar = item.owner_id == actor
        && item.location_id == office.station
        && item.flag_id == crate::model::FLAG_HANGAR;
    if !from_hangar {
        // A corporation's own item moving between its divisions, or out of
        // the deliveries hangar, under the source's role.
        match kept(reg, item_id) {
            Some(k) if item.owner_id == corp => may_take_from(corp, s, k)?,
            _ => {
                return Err(custom_notify(
                    "That item has to be in your hangar here to go into the corporation hangar.",
                ));
            }
        }
    }
    let to = Slot::new(corp, office.office_id, flag);
    let edit = move_edit(reg, item_id, qty, to)?;
    reg.apply_items(&[edit]).map_err(plan_failed)?;
    invalidate_readers(reg, corp, Some(flag), actor, vec![office.office_id]);
    Ok(())
}

/// Take an item out of an office division or the deliveries hangar into the
/// caller's own hangar, under the source's role.
fn take(
    reg: &mut Registry,
    actor: i64,
    item_id: i64,
    qty: Option<i64>,
) -> Result<(), ServiceError> {
    let Some(k) = kept(reg, item_id) else {
        return Err(custom_notify("That item is not in a corporation hangar."));
    };
    let Some((corp, s)) = standing(reg, actor) else {
        return Err(access_denied(
            "Only a member of a player corporation can do that.",
        ));
    };
    may_take_from(corp, s, k)?;
    let (station, flag, loc) = match k {
        Kept::Division { office, flag } => (office.station, Some(flag), office.office_id),
        Kept::Deliveries { station, .. } => (station, None, station),
    };
    if reg.docked_station(actor) != Some(station) {
        return Err(refuse("NoOfficeAtStation", vec![]));
    }
    let to = Slot::new(actor, station, crate::model::FLAG_HANGAR);
    let edit = move_edit(reg, item_id, qty, to)?;
    reg.apply_items(&[edit]).map_err(plan_failed)?;
    invalidate_readers(reg, corp, flag, actor, vec![loc]);
    Ok(())
}

/// A corporation's item of `type_id` arrives in its deliveries hangar at
/// `station`: what a corporation's market buy, contract or courier hands
/// over. Answers the item id. The caller has already taken the payment.
#[allow(dead_code)]
pub(crate) fn deliver_to_corp(
    reg: &mut Registry,
    corp: i64,
    station: i64,
    type_id: i64,
    quantity: i64,
) -> Result<i64, String> {
    let changes = reg
        .apply_items(&[Edit::Create {
            type_id,
            quantity,
            to: Slot::new(corp, station, co::FLAG_CORP_DELIVERIES),
        }])
        .map_err(|e| e.to_string())?;
    let id = changes
        .iter()
        .find_map(|c| match c {
            Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .ok_or_else(|| "nothing was created".to_owned())?;
    tell_deliveries(reg, corp, station);
    Ok(id)
}

/// Tell the members who may open a corporation's deliveries hangar that
/// something arrived at `station` (CORP-BUY: the market's corp fills use it
/// too).
pub(crate) fn tell_deliveries(reg: &mut Registry, corp: i64, station: i64) {
    let to: Vec<i64> = reg
        .corporations
        .members_of(corp)
        .into_iter()
        .filter(|m| standing(reg, *m).is_some_and(|(_, s)| s.sees_deliveries()))
        .collect();
    if !to.is_empty() {
        reg.push(crate::notify::Addressed {
            to: Some(to),
            inner: Box::new(crate::notify::InvalidateLocations {
                location_ids: vec![station],
            }),
        });
    }
}

fn flag_filter(req: &Request<'_>) -> Option<i64> {
    match crate::pyutil::pairs_get(req.kwargs, b"flag") {
        Some(PyValue::Int(f)) => Some(*f),
        _ => match req.args.first() {
            Some(PyValue::Int(f)) => Some(*f),
            _ => None,
        },
    }
}

fn flags_filter(req: &Request<'_>) -> Vec<i64> {
    let list = crate::pyutil::pairs_get(req.kwargs, b"flags").or(req.args.first());
    match list {
        Some(PyValue::List(v) | PyValue::Tuple(v)) => v
            .iter()
            .filter_map(|f| match f {
                PyValue::Int(n) => Some(*n),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// An item's own `{capacity, used}` for the office: the client builds the
/// same row locally for an office (`invCache.GetCapacity`: capacity
/// 9,000,000,000,000,000) **[V]**.
fn office_capacity() -> PyValue {
    crate::pyutil::keyval(vec![
        ("capacity", PyValue::Real(9_000_000_000_000_000.0)),
        ("used", PyValue::Real(0.0)),
    ])
}

/// What the actor may see in an office: the items in the divisions whose
/// query bit they hold, passing `keep`.
fn office_rows(
    reg: &Registry,
    actor: Option<i64>,
    office: &Office,
    keep: impl Fn(i64) -> bool,
) -> Vec<PyValue> {
    let Some((corp, s)) = actor.and_then(|a| standing(reg, a)) else {
        return Vec::new();
    };
    if corp != office.corp || !office.live() {
        return Vec::new();
    }
    reg.store()
        .contents_of(office.office_id)
        .filter(|i| i.owner_id == corp && s.can_query(i.flag_id) && keep(i.flag_id))
        .map(|i| reg.item_row_value(i))
        .collect()
}

/// The deliveries hangar's items at the actor's station, for those who see
/// it.
fn deliveries_rows(reg: &Registry, actor: Option<i64>) -> Vec<PyValue> {
    let Some(actor) = actor else {
        return Vec::new();
    };
    let Some((corp, s)) = standing(reg, actor) else {
        return Vec::new();
    };
    let Some(station) = reg.docked_station(actor) else {
        return Vec::new();
    };
    if !s.sees_deliveries() {
        return Vec::new();
    }
    reg.store()
        .items()
        .iter()
        .filter(|i| {
            i.owner_id == corp && i.location_id == station && i.flag_id == co::FLAG_CORP_DELIVERIES
        })
        .map(|i| reg.item_row_value(i))
        .collect()
}

fn item_ids(v: Option<&PyValue>) -> Vec<i64> {
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

fn int_kwarg(req: &Request<'_>, name: &str) -> Option<i64> {
    match crate::pyutil::pairs_get(req.kwargs, name.as_bytes()) {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    }
}

/// Run `one` for each id; the first refusal is raised once all have been
/// tried (the reply is discarded by the client, so the raise is the only
/// news a refusal can be), unless nothing at all went through.
fn each(
    ids: Vec<i64>,
    mut one: impl FnMut(i64) -> Result<(), ServiceError>,
) -> Result<PyValue, ServiceError> {
    let mut first = None;
    for id in ids {
        if let Err(e) = one(id) {
            first.get_or_insert(e);
        }
    }
    match first {
        Some(e) => Err(e),
        None => Ok(PyValue::None),
    }
}

/// `invbroker.DeliverToCorpHangar(fromID, stationID, itemIDs, qty, ownerID,
/// flag)`: the item menu's "Deliver corp stuff to > Corp hangar > division",
/// from another division or from the deliveries hangar **[V]**
/// (`invItemFunctions.DeliverToCorpHangarFolder`, `GetInventoryMgr()`). Needs
/// the source's role and the target division's.
fn deliver_to_corp_hangar(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let actor = req
        .actor
        .ok_or_else(|| access_denied("There is no character to act for."))?;
    let (Some(PyValue::Int(_from)), Some(PyValue::Int(station))) =
        (req.args.first(), req.args.get(1))
    else {
        return Err(bad_args(
            req,
            "(fromID, stationID, itemIDs, qty, ownerID, flag)",
        ));
    };
    let ids = item_ids(req.args.get(2));
    let qty = match req.args.get(3) {
        Some(PyValue::Int(q)) if ids.len() == 1 => Some(*q),
        _ => None,
    };
    let flag = match req.args.get(5) {
        Some(PyValue::Int(f)) => Some(*f),
        _ => None,
    };
    let Some((corp, s)) = standing(reg, actor) else {
        return Err(access_denied(
            "Only a member of a player corporation can do that.",
        ));
    };
    let flag = flag
        .filter(|f| co::division_of_flag(*f).is_some())
        .ok_or_else(|| custom_notify("Deliver it to one of the corporation's hangar divisions."))?;
    if !s.can_take(flag) {
        return Err(access_denied("You do not have the role for this division."));
    }
    let office = co::live_at(reg.deltas(), corp, *station)
        .ok_or_else(|| refuse("NoOfficeAtStation", vec![]))?;
    each(ids, |id| {
        let k = kept(reg, id)
            .ok_or_else(|| custom_notify("That item is not in a corporation hangar."))?;
        may_take_from(corp, s, k)?;
        let to = Slot::new(corp, office.office_id, flag);
        let edit = move_edit(reg, id, qty, to)?;
        reg.apply_items(&[edit]).map_err(plan_failed)?;
        invalidate_readers(reg, corp, Some(flag), actor, vec![office.office_id]);
        Ok(())
    })
}

/// The `invbroker` calls that are a corporation's: offered first by
/// `inventory::dispatch`. `None` means "not mine, carry on".
pub(crate) fn inventory_call(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    if req.service == "invbroker" && req.method == "DeliverToCorpHangar" {
        return Some(deliver_to_corp_hangar(reg, req));
    }
    if req.service != "invbroker::inventory" {
        return None;
    }
    let id = container_of(req.bind_params)?;
    if id == co::CONTAINER_CORP_MARKET {
        return match req.method {
            "List" | "ListByFlags" | "ListIncludingContainers" => {
                Some(Ok(PyValue::List(deliveries_rows(reg, req.actor))))
            }
            "GetCapacity" => Some(Ok(office_capacity())),
            _ => None,
        };
    }
    if let Some(office) = co::get(reg.deltas(), id) {
        return office_call(reg, req, &office);
    }
    // A drag *out* of a corporation hangar onto any other inventory.
    if matches!(req.method, "Add" | "MultiAdd") {
        let ids = item_ids(req.args.first());
        let first = ids.first().copied()?;
        kept(reg, first)?;
        let actor = req.actor?;
        if id != CONTAINER_HANGAR {
            return Some(Err(custom_notify(
                "Move it to your hangar first; this server takes items out of a corporation hangar into your hangar only.",
            )));
        }
        let qty = int_kwarg(req, "qty");
        if req.method == "Add" {
            return Some(take(reg, actor, first, qty).map(|()| PyValue::None));
        }
        return Some(each(ids, |i| take(reg, actor, i, None)));
    }
    None
}

/// The calls on an office's own inventory (`GetInventoryFromId(officeID)`).
fn office_call(
    reg: &mut Registry,
    req: &Request<'_>,
    office: &Office,
) -> Option<Result<PyValue, ServiceError>> {
    match req.method {
        "List" => {
            let want = flag_filter(req);
            Some(Ok(PyValue::List(office_rows(
                reg,
                req.actor,
                office,
                |f| want.is_none_or(|w| w == f),
            ))))
        }
        "ListByFlags" => {
            let flags = flags_filter(req);
            Some(Ok(PyValue::List(office_rows(
                reg,
                req.actor,
                office,
                |f| flags.contains(&f),
            ))))
        }
        "ListIncludingContainers" => Some(Ok(PyValue::List(office_rows(
            reg,
            req.actor,
            office,
            |_| true,
        )))),
        "GetCapacity" => Some(Ok(office_capacity())),
        // The container asks what it is (`invCacheContainer._SetItemData`);
        // an office is the office item, for whoever may ask.
        "GetSelfInvItem" => {
            let item = reg.item(office.office_id)?;
            Some(Ok(reg.item_row_value(item)))
        }
        "Add" => {
            let actor = req.actor?;
            let Some(PyValue::Int(item_id)) = req.args.first() else {
                return Some(Err(bad_args(req, "(itemID, sourceID)")));
            };
            Some(
                deposit(
                    reg,
                    actor,
                    office,
                    *item_id,
                    int_kwarg(req, "qty"),
                    int_kwarg(req, "flag"),
                )
                .map(|()| PyValue::None),
            )
        }
        "MultiAdd" => {
            let actor = req.actor?;
            let ids = item_ids(req.args.first());
            let flag = int_kwarg(req, "flag");
            Some(each(ids, |i| deposit(reg, actor, office, i, None, flag)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
