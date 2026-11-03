//! Player alliances: the state, its rules, and the read-only lookups other
//! modules need (SOC-C1; R-SOCIETY §3.1 and §6 Stage C; lane note
//! `docs/lanes/soc-c1.md`).
//!
//! **The world's deltas are the only copy.** An alliance is four delta kinds
//! (`delta_kinds::KINDS`), and every change is load, mutate, save:
//!
//! ```text
//! alliance      <allianceID>       -> executor creatorCorp creatorChar startDate
//!                                     dictatorial deleted name shortName description url
//! alliancecorp  <corpID>           -> allianceID startDate chosenExecutor
//! allianceapp   <allianceID corpID> -> state date text
//! alliancehist  <corpID seq>       -> allianceID date
//! ```
//!
//! `-` is an absent id. An alliance is never removed, only closed
//! (`deleted`), so ids are never reused and the next id is the largest plus
//! one. `alliancehist`'s `seq` is zero-padded so the rows sort in order.
//!
//! The client side (service arms, pushes, session keys) is
//! `service/alliance.rs`. Everything here is pure and tested with invented
//! ids.

use crate::delta::Deltas;
use crate::service::Registry;
use crate::slim_extra::SlimExtras;
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// The delta kinds this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "alliance";
pub const KIND_MEMBER: &str = "alliancecorp";
pub const KIND_APPLICATION: &str = "allianceapp";
pub const KIND_HISTORY: &str = "alliancehist";
pub const KINDS: [&str; 4] = [KIND, KIND_MEMBER, KIND_APPLICATION, KIND_HISTORY];

/// `inventorycommon/const.py`: `minPlayerAlliance = 99000000`,
/// `maxPlayerAlliance = 99999999` -- the range `idCheckers.IsAlliance`
/// accepts without asking anybody **[V]**.
pub const MIN_PLAYER_ALLIANCE_ID: i64 = 99_000_000;
pub const MAX_PLAYER_ALLIANCE_ID: i64 = 99_999_999;

/// `inventorycommon/const.py` `typeAlliance = 16159` **[V]**: the owner type
/// `cfg.eveowners` must carry for an alliance, which is how the info window
/// and every link decide the id is an alliance.
pub const TYPE_ALLIANCE: i64 = 16_159;

/// `appConst.allianceCreationCost = 1000000000` **[V]** (the constant is in
/// the client; R-SOCIETY marked the fee [U] before it was read). **[A]** that
/// the server charges exactly this. It comes out of the creating corporation's
/// Master Wallet (CORP-W; SOC-C1 charged the CEO's own wallet while corp
/// wallets did not exist). Falsified by a Tranquility capture charging
/// another amount or another division.
pub const ALLIANCE_CREATION_COST: f64 = 1_000_000_000.0;

/// `appConst.refAllianceRegistrationFee = 48` **[V]**: the journal row.
pub const REF_ALLIANCE_REGISTRATION_FEE: i64 = 48;

/// `appConst.allianceNameMaxLenTQ = 50`, the create form's `maxLength` **[V]**.
pub const NAME_MAX_LEN: usize = 50;
/// The shortest name: `AllianceNameInvalidMinLengthBody` reads "Minimum
/// length for a alliance name is 4 characters." **[V]** (`UI/Messages/40`).
pub const NAME_MIN_LEN: usize = 4;
/// The short-name edit's `maxLength=5` in `CreateAllianceWnd` **[V]**.
pub const SHORT_NAME_MAX_LEN: usize = 5;
/// The application editors' `maxLength=1000` and `CheckApplication`'s 1000
/// **[V]**.
pub const APPLICATION_TEXT_MAX: usize = 1000;

/// The alliance description editor's `maxLength=5000`, in both the create
/// and the edit window **[V]** (`all_ui_home`, `descriptionEdit`).
pub const DESCRIPTION_MAX_LEN: usize = 5000;
/// The alliance URL edit's `maxLength=2048` in the same windows **[V]**.
pub const URL_MAX_LEN: usize = 2048;

/// `s` cut to `max` characters.
fn capped(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// A corporation may declare executor support only after it has been a
/// member for a week: `CanNotDeclareExecutorInFirstWeek`, whose body reads
/// "You can only choose an executor after your corporation has been a member
/// for a week." **[V]** (`UI/Messages/259`). 7 days in FILETIME ticks.
pub const EXECUTOR_SUPPORT_MIN_MEMBERSHIP: i64 = 7 * 24 * 3600 * 10_000_000;

/// `appConst.allianceApplication*` **[V]**.
pub const APP_NEW: i64 = 1;
pub const APP_ACCEPTED: i64 = 2;
pub const APP_EFFECTIVE: i64 = 3;
pub const APP_REJECTED: i64 = 4;

/// One alliance.
#[derive(Debug, Clone, PartialEq)]
pub struct Alliance {
    pub alliance_id: i64,
    pub name: String,
    pub short_name: String,
    pub description: String,
    pub url: String,
    /// `None` only for a closed alliance.
    pub executor_corp_id: Option<i64>,
    pub creator_corp_id: i64,
    pub creator_char_id: i64,
    /// Blue FILETIME.
    pub start_date: i64,
    pub dictatorial: bool,
    /// Closed: no member corps left. Kept so the name, the history and the
    /// id stay meaningful.
    pub deleted: bool,
}

/// One corporation's membership of an alliance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Member {
    pub alliance_id: i64,
    pub start_date: i64,
    /// The corp this member supports as executor (`DeclareExecutorSupport`).
    pub chosen_executor: Option<i64>,
}

/// A corporation's application to join an alliance. One per pair: a new
/// application overwrites an older one, as the apply window says
/// (`ThisApplicationOverwritesOlderOnes`) **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct Application {
    pub alliance_id: i64,
    pub corporation_id: i64,
    pub state: i64,
    pub date: i64,
    pub text: String,
}

/// One line of a corporation's alliance history: joined `alliance_id` at
/// `date`, or left (`None`) at `date`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stint {
    pub alliance_id: Option<i64>,
    pub date: i64,
}

/// Why something was refused, as the client's own message key (every one is
/// in `findings/user-errors.tsv`).
pub type Refusal = &'static str;

/// What a corp leaving (or being kicked) did to its alliance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Left {
    pub alliance_id: i64,
    pub member: Member,
    /// The executor before and after, when it changed.
    pub executor_change: Option<(Option<i64>, Option<i64>)>,
    /// The alliance has no members left and is now closed.
    pub closed: bool,
}

/// What an executor's answer to an application did.
#[derive(Debug, Clone, PartialEq)]
pub struct Answered {
    pub before: Application,
    pub after: Application,
    /// The corporation is now a member.
    pub joined: bool,
}

/// What a support declaration did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Supported {
    pub before: Option<i64>,
    pub executor_change: Option<(Option<i64>, Option<i64>)>,
}

/// Every alliance, membership, application and history line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Alliances {
    alliances: BTreeMap<i64, Alliance>,
    members: BTreeMap<i64, Member>,
    applications: BTreeMap<(i64, i64), Application>,
    history: BTreeMap<(i64, u32), Stint>,
}

fn id_field(v: Option<i64>) -> String {
    v.map_or_else(|| "-".to_owned(), |v| v.to_string())
}

fn parse_opt(s: &str) -> Result<Option<i64>, String> {
    if s == "-" {
        Ok(None)
    } else {
        s.parse::<i64>()
            .map(Some)
            .map_err(|_| format!("{s:?} is not an id"))
    }
}

fn parse_num(f: &[String], i: usize) -> Result<i64, String> {
    f.get(i)
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("field {i} is not an integer"))
}

fn parse_text(f: &[String], i: usize) -> Result<String, String> {
    f.get(i)
        .cloned()
        .ok_or_else(|| format!("field {i} is missing"))
}

/// Printable short names: letters, digits, `.`, `-`, `_`, as for corp
/// tickers **[A]** (the rule is server-side). Falsified by Tranquility
/// accepting a short name this refuses.
fn short_name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.chars().count() <= SHORT_NAME_MAX_LEN
        && s.chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '.' | '-' | '_'))
}

impl Alliances {
    /// Read the four kinds. A row that does not parse is skipped with a log
    /// line rather than failing the world: it is one alliance fact lost, and
    /// the next save drops it.
    pub fn load(d: &Deltas) -> Alliances {
        let mut out = Alliances::default();
        for (key, value) in d.of(KIND) {
            match Self::parse_alliance(key, value) {
                Ok(a) => {
                    out.alliances.insert(a.alliance_id, a);
                }
                Err(e) => crate::errln!("[alliance] skipping alliance row {key:?}: {e}"),
            }
        }
        for (key, value) in d.of(KIND_MEMBER) {
            let row = (|| -> Result<(i64, Member), String> {
                let corp = parse_num(key, 0)?;
                Ok((
                    corp,
                    Member {
                        alliance_id: parse_num(value, 0)?,
                        start_date: parse_num(value, 1)?,
                        chosen_executor: parse_opt(value.get(2).map_or("-", |s| s.as_str()))?,
                    },
                ))
            })();
            match row {
                Ok((corp, m)) => {
                    out.members.insert(corp, m);
                }
                Err(e) => crate::errln!("[alliance] skipping member row {key:?}: {e}"),
            }
        }
        for (key, value) in d.of(KIND_APPLICATION) {
            let row = (|| -> Result<Application, String> {
                Ok(Application {
                    alliance_id: parse_num(key, 0)?,
                    corporation_id: parse_num(key, 1)?,
                    state: parse_num(value, 0)?,
                    date: parse_num(value, 1)?,
                    text: parse_text(value, 2)?,
                })
            })();
            match row {
                Ok(a) => {
                    out.applications
                        .insert((a.alliance_id, a.corporation_id), a);
                }
                Err(e) => crate::errln!("[alliance] skipping application row {key:?}: {e}"),
            }
        }
        for (key, value) in d.of(KIND_HISTORY) {
            let row = (|| -> Result<((i64, u32), Stint), String> {
                let seq = u32::try_from(parse_num(key, 1)?).map_err(|e| e.to_string())?;
                Ok((
                    (parse_num(key, 0)?, seq),
                    Stint {
                        alliance_id: parse_opt(value.first().map_or("", |s| s.as_str()))?,
                        date: parse_num(value, 1)?,
                    },
                ))
            })();
            match row {
                Ok((k, s)) => {
                    out.history.insert(k, s);
                }
                Err(e) => crate::errln!("[alliance] skipping history row {key:?}: {e}"),
            }
        }
        out
    }

    fn parse_alliance(key: &[String], v: &[String]) -> Result<Alliance, String> {
        let a = Alliance {
            alliance_id: parse_num(key, 0)?,
            executor_corp_id: parse_opt(v.first().map_or("", |s| s.as_str()))?,
            creator_corp_id: parse_num(v, 1)?,
            creator_char_id: parse_num(v, 2)?,
            start_date: parse_num(v, 3)?,
            dictatorial: parse_num(v, 4)? != 0,
            deleted: parse_num(v, 5)? != 0,
            name: parse_text(v, 6)?,
            short_name: parse_text(v, 7)?,
            description: parse_text(v, 8)?,
            url: parse_text(v, 9)?,
        };
        if !(MIN_PLAYER_ALLIANCE_ID..=MAX_PLAYER_ALLIANCE_ID).contains(&a.alliance_id) {
            return Err(format!("{} is not a player alliance id", a.alliance_id));
        }
        Ok(a)
    }

    /// Write the four kinds back whole: every row this state does not hold
    /// is removed, every row it holds is set.
    pub fn save(&self, d: &mut Deltas) {
        for kind in KINDS {
            let keys: Vec<Vec<String>> = d.of(kind).keys().cloned().collect();
            for k in keys {
                d.del(kind, &k);
            }
        }
        for a in self.alliances.values() {
            d.put(
                KIND,
                vec![a.alliance_id.to_string()],
                vec![
                    id_field(a.executor_corp_id),
                    a.creator_corp_id.to_string(),
                    a.creator_char_id.to_string(),
                    a.start_date.to_string(),
                    u8::from(a.dictatorial).to_string(),
                    u8::from(a.deleted).to_string(),
                    a.name.clone(),
                    a.short_name.clone(),
                    a.description.clone(),
                    a.url.clone(),
                ],
            );
        }
        for (corp, m) in &self.members {
            d.put(
                KIND_MEMBER,
                vec![corp.to_string()],
                vec![
                    m.alliance_id.to_string(),
                    m.start_date.to_string(),
                    id_field(m.chosen_executor),
                ],
            );
        }
        for a in self.applications.values() {
            d.put(
                KIND_APPLICATION,
                vec![a.alliance_id.to_string(), a.corporation_id.to_string()],
                vec![a.state.to_string(), a.date.to_string(), a.text.clone()],
            );
        }
        for ((corp, seq), s) in &self.history {
            d.put(
                KIND_HISTORY,
                vec![corp.to_string(), format!("{seq:010}")],
                vec![id_field(s.alliance_id), s.date.to_string()],
            );
        }
    }

    pub fn get(&self, alliance_id: i64) -> Option<&Alliance> {
        self.alliances.get(&alliance_id)
    }

    /// Every alliance, closed ones included, in id order.
    pub fn all(&self) -> impl Iterator<Item = &Alliance> + '_ {
        self.alliances.values()
    }

    /// The alliance this corporation is in.
    pub fn member(&self, corporation_id: i64) -> Option<Member> {
        self.members.get(&corporation_id).copied()
    }

    /// Every member corporation of one alliance, in corp id order.
    pub fn members_of(&self, alliance_id: i64) -> Vec<(i64, Member)> {
        self.members
            .iter()
            .filter(|(_, m)| m.alliance_id == alliance_id)
            .map(|(c, m)| (*c, *m))
            .collect()
    }

    pub fn application(&self, alliance_id: i64, corporation_id: i64) -> Option<&Application> {
        self.applications.get(&(alliance_id, corporation_id))
    }

    /// Every application to one alliance, in corp id order.
    pub fn applications_to(&self, alliance_id: i64) -> Vec<&Application> {
        self.applications
            .values()
            .filter(|a| a.alliance_id == alliance_id)
            .collect()
    }

    /// Every application one corporation made, in alliance id order.
    pub fn applications_from(&self, corporation_id: i64) -> Vec<&Application> {
        self.applications
            .values()
            .filter(|a| a.corporation_id == corporation_id)
            .collect()
    }

    /// One corporation's history, **newest first**, as
    /// `allianceRegistry.GetEmploymentRecord` lists it.
    pub fn history_of(&self, corporation_id: i64) -> Vec<Stint> {
        let mut v: Vec<Stint> = self
            .history
            .range((corporation_id, 0)..=(corporation_id, u32::MAX))
            .map(|(_, s)| *s)
            .collect();
        v.reverse();
        v
    }

    fn record(&mut self, corporation_id: i64, alliance_id: Option<i64>, date: i64) {
        let seq = self
            .history
            .range((corporation_id, 0)..=(corporation_id, u32::MAX))
            .next_back()
            .map_or(0, |((_, s), _)| s.saturating_add(1));
        self.history
            .insert((corporation_id, seq), Stint { alliance_id, date });
    }

    fn next_id(&self) -> i64 {
        self.alliances
            .keys()
            .next_back()
            .copied()
            .unwrap_or(MIN_PLAYER_ALLIANCE_ID)
            .max(MIN_PLAYER_ALLIANCE_ID)
            + 1
    }

    /// Check a name and short name for a new alliance. Trimmed by the caller.
    pub fn check_names(&self, name: &str, short_name: &str) -> Result<(), Refusal> {
        let n = name.chars().count();
        if n < NAME_MIN_LEN {
            return Err("AllianceNameInvalidMinLength");
        }
        if n > NAME_MAX_LEN {
            return Err("AllianceNameInvalidMaxLength");
        }
        // SEC-FIX 7: no control characters or markup in a name, by the
        // wallet divisions' rule (`corp_wallet::bad_name_char`).
        if name.chars().any(crate::corp_wallet::bad_name_char) {
            return Err("AllianceNameInvalidSomeChar");
        }
        if !short_name_ok(short_name) {
            return Err("AllianceShortNameInvalid");
        }
        // Closed alliances keep their names [A]: a name in somebody's
        // history should not come back as a different alliance. Falsified
        // by Tranquility releasing the name of a closed alliance.
        if self
            .alliances
            .values()
            .any(|a| a.name.eq_ignore_ascii_case(name))
        {
            return Err("AllianceNameInvalidTaken");
        }
        if self
            .alliances
            .values()
            .any(|a| a.short_name.eq_ignore_ascii_case(short_name))
        {
            return Err("AllianceShortNameInvalidTaken");
        }
        Ok(())
    }

    /// Found an alliance with `corporation_id` as its executor and first
    /// member. The caller has checked that `creator_char_id` is that corp's
    /// CEO and can pay.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &mut self,
        corporation_id: i64,
        creator_char_id: i64,
        name: &str,
        short_name: &str,
        description: &str,
        url: &str,
        now: i64,
    ) -> Result<i64, Refusal> {
        if self.members.contains_key(&corporation_id) {
            return Err("AllianceCreateFailCorpInAlliance");
        }
        self.check_names(name, short_name)?;
        let alliance_id = self.next_id();
        if alliance_id > MAX_PLAYER_ALLIANCE_ID {
            return Err("AllianceNameInvalidTaken");
        }
        self.alliances.insert(
            alliance_id,
            Alliance {
                alliance_id,
                name: name.to_owned(),
                short_name: short_name.to_owned(),
                description: capped(description, DESCRIPTION_MAX_LEN),
                url: capped(url, URL_MAX_LEN),
                executor_corp_id: Some(corporation_id),
                creator_corp_id: corporation_id,
                creator_char_id,
                start_date: now,
                dictatorial: false,
                deleted: false,
            },
        );
        self.join(corporation_id, alliance_id, now);
        Ok(alliance_id)
    }

    fn join(&mut self, corporation_id: i64, alliance_id: i64, now: i64) {
        self.members.insert(
            corporation_id,
            Member {
                alliance_id,
                start_date: now,
                chosen_executor: None,
            },
        );
        // A corp's open applications elsewhere lapse once it is in [A]:
        // they could only be accepted into a second alliance. Falsified by
        // Tranquility keeping them.
        self.applications
            .retain(|(a, c), _| *c != corporation_id || *a == alliance_id);
        self.record(corporation_id, Some(alliance_id), now);
    }

    /// `corpRegistry.ApplyToJoinAlliance`: a new application, replacing any
    /// older one to the same alliance. Returns the one it replaced.
    pub fn apply(
        &mut self,
        corporation_id: i64,
        alliance_id: i64,
        text: &str,
        now: i64,
    ) -> Result<Option<Application>, Refusal> {
        if self.members.contains_key(&corporation_id) {
            return Err("AllianceJoinFailCorpInAlliance");
        }
        let Some(a) = self.alliances.get(&alliance_id) else {
            return Err("AllianceNotFound");
        };
        if a.deleted {
            return Err("CantApplyToDisbandedAlliance");
        }
        let text: String = text.chars().take(APPLICATION_TEXT_MAX).collect();
        Ok(self.applications.insert(
            (alliance_id, corporation_id),
            Application {
                alliance_id,
                corporation_id,
                state: APP_NEW,
                date: now,
                text,
            },
        ))
    }

    /// `corpRegistry.DeleteAllianceApplication`: the applicant takes back a
    /// new or rejected application (the only two the client offers Delete
    /// on, `GetApplicationMenu`) **[V]**.
    pub fn withdraw(
        &mut self,
        corporation_id: i64,
        alliance_id: i64,
    ) -> Result<Application, Refusal> {
        match self.applications.get(&(alliance_id, corporation_id)) {
            Some(a) if matches!(a.state, APP_NEW | APP_REJECTED) => Ok(self
                .applications
                .remove(&(alliance_id, corporation_id))
                .expect("present: matched above")),
            _ => Err("AllianceApplicationNotFound"),
        }
    }

    /// `allianceRegistry.UpdateApplication(corpID, text, state)`: the
    /// alliance accepts or rejects a new application.
    ///
    /// **[A]: an accepted application takes effect at once**, so the state
    /// goes straight to `APP_EFFECTIVE` and the corp joins. Tranquility
    /// waits 24 hours (the client's own
    /// `AcceptedApplicationsTake24HoursToBecomeEffective`). Falsified by the
    /// owner wanting the wait back (it would be a timer through the
    /// rollover).
    pub fn answer(
        &mut self,
        alliance_id: i64,
        corporation_id: i64,
        state: i64,
        text: Option<&str>,
        now: i64,
    ) -> Result<Answered, Refusal> {
        let before = match self.applications.get(&(alliance_id, corporation_id)) {
            Some(a) if a.state == APP_NEW => a.clone(),
            _ => return Err("AllianceApplicationNotFound"),
        };
        if !matches!(state, APP_ACCEPTED | APP_REJECTED) {
            return Err("AllianceApplicationNotFound");
        }
        if self.alliances.get(&alliance_id).is_none_or(|a| a.deleted) {
            return Err("AllianceNotFound");
        }
        let joined = state == APP_ACCEPTED;
        if joined && self.members.contains_key(&corporation_id) {
            return Err("AllianceJoinFailCorpInAlliance");
        }
        let mut after = before.clone();
        after.state = if joined { APP_EFFECTIVE } else { APP_REJECTED };
        if let Some(t) = text {
            after.text = t.chars().take(APPLICATION_TEXT_MAX).collect();
        }
        self.applications
            .insert((alliance_id, corporation_id), after.clone());
        if joined {
            self.join(corporation_id, alliance_id, now);
        }
        Ok(Answered {
            before,
            after,
            joined,
        })
    }

    /// The corp with the most support, if it beats `current`'s. Votes count
    /// only from members, for members. Ties keep the current executor, then
    /// prefer the longest-standing member, then the lower id.
    ///
    /// **SEC-FIX 2, [A]: a sitting executor is replaced only by a strict
    /// majority of the member corps.** The client has no such number (its
    /// only executor rule is the first-week one, [`EXECUTOR_SUPPORT_MIN_MEMBERSHIP`]);
    /// the server's is not in the client. A corp that has declared nothing
    /// backs the sitting executor, so one new member's single vote cannot
    /// take the seat from a founder who nobody opposes. With no sitting
    /// executor (succession) the most supported takes it, as before.
    /// Falsified by a capture of Tranquility letting a plurality change it.
    fn most_supported(&self, alliance_id: i64, current: Option<i64>) -> Option<i64> {
        let members = self.members_of(alliance_id);
        let votes = |corp: i64| {
            members
                .iter()
                .filter(|(_, m)| m.chosen_executor == Some(corp))
                .count()
        };
        let sitting = current.filter(|c| members.iter().any(|(m, _)| m == c));
        let mut best = sitting;
        for (corp, m) in &members {
            best = match best {
                None => Some(*corp),
                Some(b) => {
                    let (vb, vc) = (votes(b), votes(*corp));
                    let older = self
                        .members
                        .get(&b)
                        .is_some_and(|bm| m.start_date < bm.start_date);
                    if vc > vb || (vc == vb && Some(b) != current && older) {
                        Some(*corp)
                    } else {
                        Some(b)
                    }
                }
            };
        }
        match (sitting, best) {
            (Some(s), Some(b)) if s != b && votes(b) * 2 <= members.len() => Some(s),
            _ => best,
        }
    }

    /// A member corp leaves, or is kicked: the same for the alliance.
    ///
    /// **[A] on succession**: an executor that leaves hands over at once to
    /// the member with the most support, then the longest-standing one.
    /// Tranquility recounts support at downtime. The last member leaving
    /// closes the alliance. Falsified by the owner wanting the executor
    /// unable to leave while others remain.
    pub fn leave(&mut self, corporation_id: i64, now: i64) -> Result<Left, Refusal> {
        let Some(member) = self.members.remove(&corporation_id) else {
            return Err("CantConfigureNotInAlliance");
        };
        let alliance_id = member.alliance_id;
        self.record(corporation_id, None, now);
        // Support for the leaver lapses.
        for m in self.members.values_mut() {
            if m.alliance_id == alliance_id && m.chosen_executor == Some(corporation_id) {
                m.chosen_executor = None;
            }
        }
        let remaining = self.members_of(alliance_id).len();
        let current = self
            .alliances
            .get(&alliance_id)
            .and_then(|a| a.executor_corp_id);
        let next = if remaining == 0 {
            None
        } else if current == Some(corporation_id) {
            self.most_supported(alliance_id, None)
        } else {
            current
        };
        let closed = remaining == 0;
        if let Some(a) = self.alliances.get_mut(&alliance_id) {
            a.executor_corp_id = next;
            if closed {
                a.deleted = true;
            }
        }
        if closed {
            self.applications.retain(|(a, _), _| *a != alliance_id);
        }
        Ok(Left {
            alliance_id,
            member,
            executor_change: (current != next).then_some((current, next)),
            closed,
        })
    }

    /// `allianceRegistry.DeclareExecutorSupport(corpID)`: `corporation_id`
    /// supports `candidate` as executor.
    ///
    /// **[V]** the declaring corp must have been a member for a week
    /// (`CanNotDeclareExecutorInFirstWeek`, [`EXECUTOR_SUPPORT_MIN_MEMBERSHIP`]).
    /// **[A]: support is counted at once**, and a candidate with a strict
    /// majority of the members (see `most_supported`) takes over. Tranquility
    /// counts at downtime. Falsified by the owner wanting the delay.
    pub fn declare_support(
        &mut self,
        corporation_id: i64,
        candidate: i64,
        now: i64,
    ) -> Result<Supported, Refusal> {
        let Some(me) = self.members.get(&corporation_id).copied() else {
            return Err("CantConfigureNotInAlliance");
        };
        if now
            < me.start_date
                .saturating_add(EXECUTOR_SUPPORT_MIN_MEMBERSHIP)
        {
            return Err("CanNotDeclareExecutorInFirstWeek");
        }
        if self.members.get(&candidate).map(|m| m.alliance_id) != Some(me.alliance_id) {
            return Err("CantConfigureNotInAlliance");
        }
        if self
            .alliances
            .get(&me.alliance_id)
            .is_some_and(|a| a.dictatorial)
        {
            return Err("CanNotChooseExecutorAsDictatorial");
        }
        if let Some(m) = self.members.get_mut(&corporation_id) {
            m.chosen_executor = Some(candidate);
        }
        let current = self
            .alliances
            .get(&me.alliance_id)
            .and_then(|a| a.executor_corp_id);
        let next = self.most_supported(me.alliance_id, current);
        if let Some(a) = self.alliances.get_mut(&me.alliance_id) {
            a.executor_corp_id = next;
        }
        Ok(Supported {
            before: me.chosen_executor,
            executor_change: (current != next).then_some((current, next)),
        })
    }

    /// `allianceRegistry.UpdateAlliance(description, url)`. Returns the old
    /// pair.
    pub fn update(
        &mut self,
        alliance_id: i64,
        description: &str,
        url: &str,
    ) -> Result<(String, String), Refusal> {
        let Some(a) = self.alliances.get_mut(&alliance_id).filter(|a| !a.deleted) else {
            return Err("AllianceNotFound");
        };
        let old = (
            std::mem::replace(&mut a.description, capped(description, DESCRIPTION_MAX_LEN)),
            std::mem::replace(&mut a.url, capped(url, URL_MAX_LEN)),
        );
        Ok(old)
    }

    /// `corpRegistry.GetSuggestedAllianceShortNames(name)`: initials, then
    /// the first letters, then the first letters with a digit -- upper case,
    /// at most five characters, none taken. **[A]** on the recipe; the
    /// client only lists what it is sent.
    pub fn suggest_short_names(&self, name: &str) -> Vec<String> {
        let words: Vec<String> = name
            .split_whitespace()
            .map(|w| {
                w.chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect::<String>()
                    .to_uppercase()
            })
            .filter(|w| !w.is_empty())
            .collect();
        let joined: String = words.concat();
        let mut candidates: Vec<String> = Vec::new();
        let initials: String = words.iter().filter_map(|w| w.chars().next()).collect();
        candidates.push(initials.chars().take(SHORT_NAME_MAX_LEN).collect());
        for n in (2..=SHORT_NAME_MAX_LEN).rev() {
            candidates.push(joined.chars().take(n).collect());
        }
        let stem: String = joined.chars().take(SHORT_NAME_MAX_LEN - 1).collect();
        for d in 1..=9 {
            candidates.push(format!("{stem}{d}"));
        }
        let mut out: Vec<String> = Vec::new();
        for c in candidates {
            if short_name_ok(&c)
                && !out.contains(&c)
                && !self
                    .alliances
                    .values()
                    .any(|a| a.short_name.eq_ignore_ascii_case(&c))
            {
                out.push(c);
            }
            if out.len() == 5 {
                break;
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Lookups for other modules. Each reads one delta row; none loads the whole
// state.
// ---------------------------------------------------------------------------

/// The alliance a corporation is in. **For SOC-C2** (group contacts) and
/// anything else that needs "which alliance": one delta row, no load.
pub fn of_corp(d: &Deltas, corporation_id: i64) -> Option<i64> {
    d.of(KIND_MEMBER)
        .get(&vec![corporation_id.to_string()])
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<i64>().ok())
}

/// The alliance a character is in, through their corporation. **For SOC-C2.**
pub fn of_character(reg: &Registry, character_id: i64) -> Option<i64> {
    reg.character(character_id).and_then(|c| c.alliance_id)
}

fn alliance_field(d: &Deltas, alliance_id: i64, i: usize) -> Option<String> {
    d.of(KIND)
        .get(&vec![alliance_id.to_string()])
        .and_then(|v| v.get(i))
        .cloned()
}

/// An alliance's name, for `config.GetMultiOwnersEx`.
pub fn name_of(d: &Deltas, alliance_id: i64) -> Option<String> {
    alliance_field(d, alliance_id, 6)
}

/// An alliance's short name, for `config.GetMultiAllianceShortNamesEx`.
pub fn short_name_of(d: &Deltas, alliance_id: i64) -> Option<String> {
    alliance_field(d, alliance_id, 7)
}

/// After a world load: every character's `alliance_id` from their corp's
/// membership. Called from `corp::sync_characters`, after the corps are
/// reconciled.
pub fn sync_characters(reg: &mut Registry) {
    let pairs: Vec<(i64, Option<i64>)> = reg
        .config()
        .characters
        .iter()
        .map(|c| (c.character_id, of_corp(reg.deltas(), c.corporation_id)))
        .collect();
    for (id, alliance) in pairs {
        if let Some(c) = reg.character_mut(id) {
            c.alliance_id = alliance;
        }
    }
}

/// `config.GetMultiAllianceShortNamesEx(ids)`: the `(header, data)` form,
/// `allianceID` and `shortName` -- the two columns
/// `eveCfg.allianceshortnameRowHeader` declares **[V]**. Only ids that name
/// an alliance come back; the client remembers the rest as unknown.
pub fn multi_short_names(reg: &Registry, args: &[PyValue]) -> PyValue {
    let ids: Vec<i64> = match args.first() {
        Some(PyValue::List(v) | PyValue::Tuple(v)) => v
            .iter()
            .filter_map(|x| match x {
                PyValue::Int(i) => Some(*i),
                _ => None,
            })
            .collect(),
        Some(PyValue::Int(i)) => vec![*i],
        _ => Vec::new(),
    };
    let mut rs = eve_rowset::Rowset::new(&["allianceID", "shortName"]).expect("a fixed header");
    for id in ids {
        if let Some(short) = short_name_of(reg.deltas(), id) {
            rs.push(vec![PyValue::Int(id), PyValue::Unicode(short)])
                .expect("two cells for two columns");
        }
    }
    rs.to_pyvalue()
}

/// A slim-extra source (`crate::slim_extra::SOURCES`): a pilot's ship slim
/// carries `allianceID` when the pilot is in an alliance.
/// `tactical.CheckSameAlliance` is `session.allianceid and
/// slimItem.allianceID == session.allianceid` **[V]** (`state.py`), so
/// without it an alliance mate never shows as one on the overview.
pub fn slim_extras(reg: &Registry, ids: &[i64]) -> SlimExtras {
    ids.iter()
        .filter_map(|id| {
            let p = reg.roster().by_ship(*id)?;
            let alliance = reg.character(p.character_id)?.alliance_id?;
            Some((*id, vec![("allianceID", PyValue::Int(alliance))]))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORP_A: i64 = 98_000_101;
    const CORP_B: i64 = 98_000_102;
    const CORP_C: i64 = 98_000_103;
    const CEO_A: i64 = 90_000_101;
    const T0: i64 = 134_000_000_000_000_000;
    /// Past every member's first week.
    const LATER: i64 = T0 + 2 * EXECUTOR_SUPPORT_MIN_MEMBERSHIP;

    fn founded() -> (Alliances, i64) {
        let mut s = Alliances::default();
        let id = s
            .create(CORP_A, CEO_A, "Invented Pact", "INVP", "d", "u", T0)
            .expect("founds");
        (s, id)
    }

    #[test]
    fn a_new_alliance_gets_an_id_the_client_calls_an_alliance_with_its_founder_as_executor() {
        let (s, id) = founded();
        assert!((MIN_PLAYER_ALLIANCE_ID..=MAX_PLAYER_ALLIANCE_ID).contains(&id));
        let a = s.get(id).expect("exists");
        assert_eq!(a.executor_corp_id, Some(CORP_A));
        assert_eq!((a.creator_corp_id, a.creator_char_id), (CORP_A, CEO_A));
        assert_eq!(s.member(CORP_A).map(|m| m.alliance_id), Some(id));
        assert_eq!(
            s.history_of(CORP_A),
            vec![Stint {
                alliance_id: Some(id),
                date: T0
            }]
        );
    }

    #[test]
    fn names_are_checked_and_unique() {
        let (mut s, _) = founded();
        let long = "x".repeat(NAME_MAX_LEN + 1);
        for (name, short, key) in [
            ("abc", "AB", "AllianceNameInvalidMinLength"),
            ("Bad<b>Name", "AB", "AllianceNameInvalidSomeChar"),
            ("Bad\u{7}Name", "AB", "AllianceNameInvalidSomeChar"),
            ("Bad&Name", "AB", "AllianceNameInvalidSomeChar"),
            (long.as_str(), "AB", "AllianceNameInvalidMaxLength"),
            ("Other Name", "", "AllianceShortNameInvalid"),
            ("Other Name", "TOOLONG", "AllianceShortNameInvalid"),
            ("Other Name", "A B", "AllianceShortNameInvalid"),
            ("invented pact", "NEW", "AllianceNameInvalidTaken"),
            ("Other Name", "invp", "AllianceShortNameInvalidTaken"),
        ] {
            assert_eq!(
                s.create(CORP_B, 1, name, short, "", "", T0),
                Err(key),
                "{name:?}/{short:?}"
            );
        }
        assert_eq!(
            s.create(CORP_A, CEO_A, "Second", "SEC", "", "", T0),
            Err("AllianceCreateFailCorpInAlliance")
        );
        assert!(s.member(CORP_B).is_none());
    }

    #[test]
    fn an_application_is_accepted_and_the_corp_joins() {
        let (mut s, id) = founded();
        assert_eq!(s.apply(CORP_B, id, "first", T0 + 1), Ok(None));
        // A second application overwrites the first.
        let old = s.apply(CORP_B, id, "please", T0 + 2).expect("applies");
        assert_eq!(old.map(|a| a.text), Some("first".to_owned()));
        assert_eq!(s.applications_to(id).len(), 1);
        assert_eq!(s.applications_from(CORP_B)[0].state, APP_NEW);

        let done = s
            .answer(id, CORP_B, APP_ACCEPTED, Some("welcome"), T0 + 3)
            .expect("accepts");
        assert!(done.joined);
        assert_eq!(done.after.state, APP_EFFECTIVE);
        assert_eq!(done.after.text, "welcome");
        assert_eq!(s.member(CORP_B).map(|m| m.alliance_id), Some(id));
        assert_eq!(s.members_of(id).len(), 2);
        // Answered once only.
        assert_eq!(
            s.answer(id, CORP_B, APP_REJECTED, None, T0 + 4),
            Err("AllianceApplicationNotFound")
        );
        assert_eq!(
            s.apply(CORP_B, id, "again", T0 + 5),
            Err("AllianceJoinFailCorpInAlliance")
        );
    }

    #[test]
    fn a_rejected_application_can_be_withdrawn_and_nothing_else_moves() {
        let (mut s, id) = founded();
        s.apply(CORP_B, id, "", T0).expect("applies");
        let done = s
            .answer(id, CORP_B, APP_REJECTED, None, T0)
            .expect("rejects");
        assert!(!done.joined);
        assert_eq!(done.after.state, APP_REJECTED);
        assert!(s.member(CORP_B).is_none());
        assert_eq!(s.withdraw(CORP_B, id).map(|a| a.state), Ok(APP_REJECTED));
        assert_eq!(s.withdraw(CORP_B, id), Err("AllianceApplicationNotFound"));
        assert_eq!(s.apply(CORP_B, id + 1, "", T0), Err("AllianceNotFound"));
    }

    #[test]
    fn the_executor_leaving_hands_over_and_the_last_corp_closes_the_alliance() {
        let (mut s, id) = founded();
        for (corp, t) in [(CORP_B, 1), (CORP_C, 2)] {
            s.apply(corp, id, "", T0 + t).expect("applies");
            s.answer(id, corp, APP_ACCEPTED, None, T0 + t)
                .expect("accepts");
        }
        // One vote of three is not a majority; two are.
        let sup = s.declare_support(CORP_B, CORP_C, LATER).expect("supports");
        assert_eq!(sup.executor_change, None);
        let sup = s.declare_support(CORP_C, CORP_C, LATER).expect("supports");
        assert_eq!(sup.executor_change, Some((Some(CORP_A), Some(CORP_C))));
        let left = s.leave(CORP_C, T0 + 10).expect("leaves");
        assert_eq!(left.executor_change, Some((Some(CORP_C), Some(CORP_A))));
        assert!(!left.closed);
        // B's support for C lapsed with C.
        assert_eq!(s.member(CORP_B).and_then(|m| m.chosen_executor), None);
        let left = s.leave(CORP_A, T0 + 11).expect("leaves");
        assert_eq!(left.executor_change, Some((Some(CORP_A), Some(CORP_B))));
        let left = s.leave(CORP_B, T0 + 12).expect("leaves");
        assert!(left.closed);
        assert!(s.get(id).is_some_and(|a| a.deleted));
        assert_eq!(
            s.apply(CORP_C, id, "", T0),
            Err("CantApplyToDisbandedAlliance")
        );
        assert_eq!(s.leave(CORP_B, T0), Err("CantConfigureNotInAlliance"));
        // History, newest first: left, joined.
        let h = s.history_of(CORP_C);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].alliance_id, None);
        assert_eq!(h[1].alliance_id, Some(id));
    }

    #[test]
    fn support_needs_two_members_of_one_alliance() {
        let (mut s, id) = founded();
        assert_eq!(
            s.declare_support(CORP_B, CORP_A, LATER),
            Err("CantConfigureNotInAlliance")
        );
        assert_eq!(
            s.declare_support(CORP_A, CORP_B, LATER),
            Err("CantConfigureNotInAlliance")
        );
        let own = s
            .declare_support(CORP_A, CORP_A, LATER)
            .expect("supports itself");
        assert_eq!(own.executor_change, None);
        assert_eq!(s.get(id).and_then(|a| a.executor_corp_id), Some(CORP_A));
    }

    #[test]
    fn joining_drops_the_corps_other_applications() {
        let (mut s, first) = founded();
        let second = s
            .create(CORP_C, 2, "Second Pact", "SEC", "", "", T0)
            .expect("founds");
        s.apply(CORP_B, first, "", T0).expect("applies");
        s.apply(CORP_B, second, "", T0).expect("applies");
        s.answer(second, CORP_B, APP_ACCEPTED, None, T0)
            .expect("accepts");
        assert!(s.application(first, CORP_B).is_none());
        assert_eq!(s.applications_from(CORP_B).len(), 1);
    }

    #[test]
    fn the_state_survives_the_deltas() {
        let (mut s, id) = founded();
        s.apply(CORP_B, id, "tab\there", T0).expect("applies");
        s.apply(CORP_C, id, "", T0).expect("applies");
        s.answer(id, CORP_C, APP_ACCEPTED, None, T0 + 1)
            .expect("accepts");
        s.declare_support(CORP_C, CORP_C, LATER).expect("supports");
        s.update(id, "new description", "").expect("updates");
        let mut d = Deltas::default();
        s.save(&mut d);
        assert_eq!(Alliances::load(&d), s);
        assert_eq!(of_corp(&d, CORP_C), Some(id));
        assert_eq!(of_corp(&d, CORP_B), None);
        assert_eq!(name_of(&d, id).as_deref(), Some("Invented Pact"));
        assert_eq!(short_name_of(&d, id).as_deref(), Some("INVP"));
        // A save of less removes what is gone.
        s.leave(CORP_C, T0 + 2).expect("leaves");
        s.save(&mut d);
        assert_eq!(of_corp(&d, CORP_C), None);
        assert_eq!(Alliances::load(&d), s);
    }

    #[test]
    fn many_history_lines_keep_their_order() {
        let (mut s, id) = founded();
        for t in 1..=12 {
            s.leave(CORP_A, T0 + 2 * t).ok();
            s.join(CORP_A, id, T0 + 2 * t + 1);
        }
        let mut d = Deltas::default();
        s.save(&mut d);
        let back = Alliances::load(&d);
        let dates: Vec<i64> = back.history_of(CORP_A).iter().map(|h| h.date).collect();
        let mut sorted = dates.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(dates, sorted);
        assert_eq!(dates.len(), 25);
    }

    #[test]
    fn suggested_short_names_are_valid_and_free() {
        let (s, _) = founded();
        let got = s.suggest_short_names("Invented Pact Two");
        assert!(!got.is_empty());
        assert!(got.iter().all(|n| short_name_ok(n)));
        assert!(!got.iter().any(|n| n.eq_ignore_ascii_case("INVP")));
        assert_eq!(got[0], "IPT");
    }

    /// SEC-FIX 2: one new member's own vote used to beat the founder's zero
    /// (one vote beats none, and the join took the seat at once).
    #[test]
    fn a_joining_corp_cannot_take_the_executor_seat_on_one_vote() {
        let (mut s, id) = founded();
        s.apply(CORP_B, id, "", T0).expect("applies");
        s.answer(id, CORP_B, APP_ACCEPTED, None, T0)
            .expect("accepts");
        let sup = s.declare_support(CORP_B, CORP_B, LATER).expect("supports");
        assert_eq!(sup.executor_change, None);
        assert_eq!(s.get(id).and_then(|a| a.executor_corp_id), Some(CORP_A));
        // The founder's own corp counts for itself by default, so a second
        // vote for B is the only way: B's support is one of two, no majority.
        assert_eq!(
            s.member(CORP_B).and_then(|m| m.chosen_executor),
            Some(CORP_B)
        );
        // A corp that declares for B too makes two of two.
        let sup = s.declare_support(CORP_A, CORP_B, LATER).expect("supports");
        assert_eq!(sup.executor_change, Some((Some(CORP_A), Some(CORP_B))));
    }

    /// SEC-FIX 2: the client's first-week rule.
    #[test]
    fn support_is_refused_in_the_corps_first_week() {
        let (mut s, id) = founded();
        s.apply(CORP_B, id, "", T0).expect("applies");
        s.answer(id, CORP_B, APP_ACCEPTED, None, T0)
            .expect("accepts");
        assert_eq!(
            s.declare_support(CORP_B, CORP_B, T0 + EXECUTOR_SUPPORT_MIN_MEMBERSHIP - 1),
            Err("CanNotDeclareExecutorInFirstWeek")
        );
        assert!(
            s.declare_support(CORP_B, CORP_B, T0 + EXECUTOR_SUPPORT_MIN_MEMBERSHIP)
                .is_ok()
        );
    }

    /// SEC-FIX 4: the client's `maxLength`s hold on create and update.
    #[test]
    fn description_and_url_are_cut_to_the_clients_limits() {
        let long = |n: usize| "d".repeat(n);
        let mut s = Alliances::default();
        let id = s
            .create(
                CORP_A,
                CEO_A,
                "Invented Pact",
                "INVP",
                &long(1_000_000),
                &long(1_000_000),
                T0,
            )
            .expect("founds");
        let a = s.get(id).expect("exists");
        assert_eq!(a.description.chars().count(), DESCRIPTION_MAX_LEN);
        assert_eq!(a.url.chars().count(), URL_MAX_LEN);
        s.update(id, &long(DESCRIPTION_MAX_LEN + 1), &long(URL_MAX_LEN + 1))
            .expect("updates");
        let a = s.get(id).expect("exists");
        assert_eq!(a.description.chars().count(), DESCRIPTION_MAX_LEN);
        assert_eq!(a.url.chars().count(), URL_MAX_LEN);
    }
}
