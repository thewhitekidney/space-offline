//! **A bot runs a propulsion module or a hardener hot in a fight**
//! (K-OVERHEAT, wave K). Lane note `docs/lanes/k-overheat.md`.
//!
//! One rule, kept simple: when a gun starts firing, every fitted module whose
//! type the **client's own data** gives an overload effect of the kinds below
//! is overloaded, one `dogmaIM.Overload(itemID, effectID)` each; when the fire
//! ceases, they are stopped. A module that cannot (no Thermodynamics, burnt
//! out, offline) is refused by the server and left alone: a refusal is not an
//! error here.
//!
//! - **[V-data] which modules**: the type's category-5 effect is one of
//!   [`PREFERRED`] (`overloadSelfSpeedBonus` 3175 on an afterburner or a
//!   microwarpdrive; `overloadSelf{Em,Thermal,Explosive,Kinetic}HardeningBonus`
//!   3029-3032 and `overloadSelfHardeningInvulnerabilityBonus` 3035 on a
//!   hardener). The bot reads the table the server reads, so "the data says
//!   it can overload" is the same test the server's refusal applies.
//! - **[A] only these kinds**, not the guns: a gun's overload (damage, rate
//!   of fire) heats the rack for a bot that fires for minutes on end, and a
//!   burnt-out gun is a loss the closed economy would have to replace.
//!   Falsified by a bot that should press the damage: add the gun effects to
//!   [`PREFERRED`].
//! - **[A] on at the first shot, off at the cease-fire**: a fight is the span
//!   between them. A bot that never ceases fire keeps its module hot until
//!   the heat burns it out; the driver's cease-fire on a target gone is the
//!   normal end.
//!
//! **Functions touched outside this file**: none. The driver calls
//! [`engage`] and [`disengage`] (coordinator diff), `lib.rs` gets one `pub
//! mod` line.

use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_server::content::Content;

/// `dgmEffOverload` [V].
pub const CATEGORY_OVERLOAD: i64 = 5;

/// The overload effects of a propulsion module and of the hardeners
/// **[V-data]** (`types/typedata.tsv` effect names).
pub const PREFERRED: [i64; 6] = [3175, 3029, 3030, 3031, 3032, 3035];

/// The overload effect (category 5) of a module type, if the content has one.
pub fn overload_effect(content: Option<&Content>, type_id: i64) -> Option<i64> {
    let table = eve_server::typedata::installed_in(content?).table();
    table.get(type_id)?.effects.iter().copied().find(|e| {
        table
            .effect(*e)
            .is_some_and(|r| r.category == CATEGORY_OVERLOAD)
    })
}

/// The fitted `(module, overload effect)` pairs worth running hot: a
/// propulsion module or a hardener, by the data.
pub fn candidates(content: Option<&Content>, fitted: &[(i64, i64)]) -> Vec<(i64, i64)> {
    fitted
        .iter()
        .filter_map(|(module, type_id)| {
            let effect = overload_effect(content, *type_id)?;
            PREFERRED.contains(&effect).then_some((*module, effect))
        })
        .collect()
}

impl Conn {
    /// **Any call on the location's `dogmaIM` moniker**, with the reply's
    /// value: the overload and weapon-bank calls and the repair calls the
    /// ship HUD makes (`godma:2014-2190`, `clientDogmaLocation:728-840`) [V].
    /// A refusal is the answer's outcome ([`CallOutcome::refusal`]), not an
    /// error.
    pub fn dogma_im(&mut self, method: &str, args: Vec<PyValue>) -> Result<crate::Answer> {
        let params = self
            .session()
            .location_moniker()
            .ok_or_else(|| Error::NotSent(format!("{method}: the session has no location")))?;
        self.moniker_call("dogmaIM", params, method, args, vec![])
    }

    /// `dogmaIM.Overload(itemID, effectID)` / `StopOverload` on the location's
    /// `dogmaIM` moniker (`godma:2014-2060`) [V]. A refusal is
    /// [`Error::Refused`] with the server's key.
    pub fn overload_module(&mut self, module_id: i64, effect_id: i64, on: bool) -> Result<()> {
        let method = if on { "Overload" } else { "StopOverload" };
        let answer = self.dogma_im(
            method,
            vec![PyValue::Int(module_id), PyValue::Int(effect_id)],
        )?;
        match answer.outcome.refusal() {
            Some(key) => Err(Error::Refused(format!("{method}({module_id}): {key}"))),
            None => Ok(()),
        }
    }
}

/// The ship's fitted modules as `(item, type)`.
fn fitted(conn: &mut Conn) -> Result<Vec<(i64, i64)>> {
    let ship = conn
        .session()
        .ship_id()
        .ok_or_else(|| Error::NotSent("overload: the session names no ship".to_owned()))?;
    Ok(conn
        .ship_items(ship, None)?
        .into_iter()
        .filter(|r| r.is_fitted())
        .map(|r| (r.item_id, r.type_id))
        .collect())
}

/// Overload every candidate that is not overloaded yet. The modules the
/// server accepted, in fitting order.
pub fn engage(conn: &mut Conn, content: Option<&Content>) -> Result<Vec<i64>> {
    let mut on = Vec::new();
    for (module, effect) in candidates(content, &fitted(conn)?) {
        if conn.active_effects(module)?.contains(&effect) {
            continue;
        }
        match conn.overload_module(module, effect, true) {
            Ok(()) => on.push(module),
            Err(Error::Refused(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(on)
}

/// Stop every candidate that is overloaded. The modules stopped.
pub fn disengage(conn: &mut Conn, content: Option<&Content>) -> Result<Vec<i64>> {
    let mut off = Vec::new();
    for (module, effect) in candidates(content, &fitted(conn)?) {
        if !conn.active_effects(module)?.contains(&effect) {
            continue;
        }
        match conn.overload_module(module, effect, false) {
            Ok(()) => off.push(module),
            Err(Error::Refused(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(off)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HULL: i64 = 990_701_101;
    const AB: i64 = 990_701_102;
    const HARDENER: i64 = 990_701_103;
    const GUN: i64 = 990_701_104;
    const PLAIN: i64 = 990_701_105;

    fn content() -> Content {
        let rows = format!(
            "{}\
             type\t{HULL}\t25\t6\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tFixture Hull\n\
             type\t{AB}\t46\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t3175\t\tFixture AB\n\
             type\t{HARDENER}\t77\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t3029\t\tFixture Hardener\n\
             type\t{GUN}\t53\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t3025\t\tFixture Gun\n\
             type\t{PLAIN}\t53\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t\tFixture Plain\n\
             effect\t3175\t5\t0\t0\t0\t0\t0\t0\toverloadSelfSpeedBonus\t\n\
             effect\t3029\t5\t0\t0\t0\t0\t0\t0\toverloadSelfEmHardeningBonus\t\n\
             effect\t3025\t5\t0\t0\t0\t0\t0\t0\toverloadSelfDamageBonus\t\n",
            eve_server::typedata::FIXTURE,
        );
        let mut c = Content::fixture();
        let table = eve_server::typedata::Table::parse(&rows).expect("rows parse");
        c.insert("typedata", eve_server::typedata::Installed::build(table));
        c
    }

    #[test]
    fn the_overload_effect_is_the_types_category_five_effect() {
        let c = content();
        assert_eq!(overload_effect(Some(&c), AB), Some(3175));
        assert_eq!(overload_effect(Some(&c), GUN), Some(3025));
        assert_eq!(overload_effect(Some(&c), PLAIN), None);
        assert_eq!(overload_effect(Some(&c), 1), None, "not in the table");
        assert_eq!(overload_effect(None, AB), None, "no content, no data");
    }

    #[test]
    fn only_a_propulsion_module_or_a_hardener_is_run_hot() {
        let c = content();
        let fitted = [(1, AB), (2, GUN), (3, HARDENER), (4, PLAIN)];
        assert_eq!(
            candidates(Some(&c), &fitted),
            [(1, 3175), (3, 3029)],
            "the gun and the plain module are left alone"
        );
        assert!(candidates(None, &fitted).is_empty());
        assert!(candidates(Some(&c), &[]).is_empty());
    }
}
