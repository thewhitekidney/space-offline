//! **Bots that send mail** (MAIL-1, C4's society hooks): a corp's CEO mails
//! the pilot whose application it accepted, and a CEO that sets a corp blue
//! or red mails the other corp's CEO. Each mail goes through the bot's own
//! connection and the client's own call, `mailMgr.SendMail(toCharacterIDs,
//! toListID, toCorpOrAllianceID, title, body, isReplyTo, isForwardedFrom)`
//! **[V]** (`mailSvc.SendMail`), so a refusal is a line, never a crash.
//!
//! **Receiving** needs nothing: a mail a player sends to a bot is filed by the
//! server like any other, and the bot never reads its mailbox or acts on the
//! `OnMailSent` push (unknown pushes are dropped), so a bot cannot error on
//! one. The texts are short and plain on purpose.

use crate::corpmind::Hall;
use crate::{Conn, Result, society};
use eve_marshal::PyValue;

/// The longest text a bot sends **[A]**: well inside the client's 150-char
/// subject and 8000-char body limits, short because a bot's mail is a
/// notice. Falsified by a bot text the server refuses as too long.
pub const MAX_TEXT: usize = 400;

/// The recruitment mail: the application was accepted, the offer waits.
pub fn welcome_text(corp_name: &str) -> (String, String) {
    (
        format!("Welcome to {corp_name}"),
        format!(
            "Your application to {corp_name} was accepted. \
             Open Corporation, Applications, and accept the offer to join us. Fly safe."
        ),
    )
}

/// The diplomacy mail for a contact set toward `their`: blue is an offer of
/// peace, red a warning.
pub fn diplomacy_text(ours: &str, their: &str, blue: bool) -> (String, String) {
    if blue {
        (
            format!("{ours}: blue standing"),
            format!(
                "{ours} has set {their} blue. We would like to fly in peace \
                 beside you. Reply to this mail if you want to talk."
            ),
        )
    } else {
        (
            format!("{ours}: red standing"),
            format!(
                "{ours} has set {their} red. Keep out of our way; \
                 we will shoot what we find."
            ),
        )
    }
}

/// R-REL-3: the mail for a **friendly** (+5) contact: a lighter blue, an
/// offer to fly together (a reply counts like a reply to a blue offer).
pub fn friendly_text(ours: &str, their: &str) -> (String, String) {
    (
        format!("{ours}: friendly standing"),
        format!(
            "{ours} has set {their} friendly. We are glad to fly near you and will \
             not shoot you. Reply to this mail if you want to fly together."
        ),
    )
}

/// R-REL-3: the mail for a **wary** (-5) contact: a warning short of war.
pub fn wary_text(ours: &str, their: &str) -> (String, String) {
    (
        format!("{ours}: wary standing"),
        format!(
            "{ours} has set {their} wary. We do not trust you; stay out of our \
             way and we will leave you alone."
        ),
    )
}

/// R-REL-5: the mail a CEO sends the other side's when it has declared war.
pub fn war_text(ours: &str) -> (String, String) {
    (
        format!("{ours}: war declared"),
        format!(
            "{ours} has declared war on you. After the spool-up our pilots may \
             shoot yours anywhere. Ask for peace by surrender when you have had enough."
        ),
    )
}

/// R-REL-5: the mail for an offer or acceptance of peace (a surrender).
pub fn peace_text(ours: &str) -> (String, String) {
    (
        format!("{ours}: peace"),
        format!(
            "{ours} offers peace. A surrender ends the war and neither side may \
             declare on the other again for two weeks."
        ),
    )
}

/// R-REL-3: **the text for a contact set at `standing`** (the wire value):
/// the blue and red texts at +-10, the friendly and wary ones at +-5.
pub fn standing_text(ours: &str, their: &str, standing: f64) -> (String, String) {
    if standing >= society::BLUE_STANDING {
        diplomacy_text(ours, their, true)
    } else if standing > 0.0 {
        friendly_text(ours, their)
    } else if standing <= society::RED_STANDING {
        diplomacy_text(ours, their, false)
    } else {
        wary_text(ours, their)
    }
}

/// R-REL-3: the invitation mail, after the CEO sent a corp invite.
pub fn invite_text(corp_name: &str) -> (String, String) {
    (
        format!("{corp_name} invites you"),
        format!(
            "We have enjoyed flying with you. {corp_name} has sent you an invitation: \
             open Corporation, Applications, and accept it if you would like to join us."
        ),
    )
}

/// DOC-6: **the doctrine announcement** a CEO mails the corp: the doctrine's
/// key, the hull to fly (a `showinfo` link the client turns into the type's
/// name **[A]**, as [`tier_text`]'s system link), what ship replacement pays
/// and where the corp stages (`staging`, a system link). Plain and short.
pub fn doctrine_text(
    ours: &str,
    key: &str,
    hull: Option<i64>,
    staging: Option<i64>,
) -> (String, String) {
    let mut body = format!("{ours} flies the {key} doctrine.");
    if let Some(h) = hull {
        body += &format!(" Fly <url=showinfo:{h}>this hull</url> and fit what the fit names.");
    }
    body += &format!(
        " Lose one on a corp op and ship replacement pays {:.0}% of the fit.",
        crate::doctrine_stock::SRP_SHARE * 100.0
    );
    body += " Spare hulls, modules and rounds wait in the corporation hangar's doctrine division.";
    if let Some(system) = staging {
        body += &format!(" We stage at <url=showinfo:5//{system}>this system</url>.");
    }
    (format!("{ours}: doctrine"), body)
}

/// DOC-6: **mail the doctrine and the staging to the whole corp** (the
/// CEO's own call: `toCorpOrAllianceID` is the corp **[V]**
/// `mailSvc.SendMail`; the server refuses a corp that is not the sender's):
/// one act line.
pub fn doctrine(
    conn: &mut Conn,
    hall: &Hall,
    corp: i64,
    key: &str,
    fit: Option<&crate::doctrine::RoleFit>,
    staging: Option<i64>,
) -> Result<Option<String>> {
    let ours = name_of(hall, corp).unwrap_or_else(|| "Our corporation".to_owned());
    let (title, body) = doctrine_text(&ours, key, fit.map(|f| f.hull), staging);
    let cut = |s: &str| -> PyValue { PyValue::Unicode(s.chars().take(MAX_TEXT).collect()) };
    let answer = conn.call(
        "mailMgr",
        "SendMail",
        vec![
            PyValue::List(Vec::new()),
            PyValue::None,
            PyValue::Int(corp),
            cut(&title),
            cut(&body),
            PyValue::Int(0),
            PyValue::Int(0),
        ],
    )?;
    Ok(Some(match answer.refusal() {
        None => format!("mail corp {corp}: doctrine {key}: ok"),
        Some(why) => format!("mail corp {corp}: doctrine {key}: refused {why}"),
    }))
}

/// R-REL-3: **the invitation mail** after `corp`'s CEO invited `who`.
pub fn invited(conn: &mut Conn, hall: &Hall, corp: i64, who: i64) -> Result<Option<String>> {
    let name = name_of(hall, corp).unwrap_or_else(|| "our corporation".to_owned());
    let (title, body) = invite_text(&name);
    Ok(Some(match send(conn, &[who], &title, &body)? {
        Ok(_) => format!("mail {who}: invitation to {corp}: ok"),
        Err(key) => format!("mail {who}: invitation to {corp}: refused {key}"),
    }))
}

/// R-REL-4: **the mail a CEO sends a human when their standing crosses a
/// tier**, plain on purpose: what the tier is, what it opens or closes. An
/// Allied pilot also gets the staging system (a `showinfo` link **[A]**: the
/// client turns `showinfo:5//<system id>` into the system's link).
pub fn tier_text(ours: &str, to: society::Stance5, staging: Option<i64>) -> (String, String) {
    use society::Stance5;
    let (title, body) = match to {
        Stance5::Allied => (
            "allied standing",
            "We count you an ally. We answer your calls for backup, offer you \
             contracts at a discount and will take you into our corporation or \
             alliance. Fly with us.",
        ),
        Stance5::Friendly => (
            "friendly standing",
            "We count you a friend. We will not shoot you, we answer a call for \
             backup, and we may invite you to join us. Fly with us and it grows.",
        ),
        Stance5::Neutral => (
            "neutral standing",
            "We are neutral toward you again. We trade as with anyone and do not \
             invite you; fly with us or help us in a fight to be counted a friend.",
        ),
        Stance5::Wary => (
            "wary standing",
            "We do not trust you. We ask more for a trade and will not answer your \
             calls. Help us, trade fairly or fly with us to mend it.",
        ),
        Stance5::Hostile => (
            "hostile standing",
            "We count you an enemy and will shoot you on sight. It fades if you \
             leave us alone; a gift or an honoured contract helps.",
        ),
    };
    let mut body = body.to_owned();
    if let (Stance5::Allied, Some(system)) = (to, staging) {
        body += &format!(" We stage at <url=showinfo:5//{system}>our home system</url>.");
    }
    (format!("{ours}: {title}"), body)
}

/// R-REL-4: **tell `who` the tier `corp`'s CEO now holds them at**: one act
/// line.
pub fn tier_changed(
    conn: &mut Conn,
    hall: &Hall,
    corp: i64,
    who: i64,
    to: society::Stance5,
    staging: Option<i64>,
) -> Result<Option<String>> {
    let ours = name_of(hall, corp).unwrap_or_else(|| "A corporation".to_owned());
    let (title, body) = tier_text(&ours, to, staging);
    Ok(Some(match send(conn, &[who], &title, &body)? {
        Ok(_) => format!("mail {who}: {} from {corp}: ok", to.name()),
        Err(key) => format!("mail {who}: {} from {corp}: refused {key}", to.name()),
    }))
}

/// Send one mail to `to` on this connection: the new message id, or the
/// `UserError` key the server refused it with.
pub fn send(
    conn: &mut Conn,
    to: &[i64],
    title: &str,
    body: &str,
) -> Result<std::result::Result<i64, String>> {
    let cut = |s: &str| -> PyValue { PyValue::Unicode(s.chars().take(MAX_TEXT).collect()) };
    let answer = conn.call(
        "mailMgr",
        "SendMail",
        vec![
            PyValue::List(to.iter().map(|c| PyValue::Int(*c)).collect()),
            PyValue::None,
            PyValue::None,
            cut(title),
            cut(body),
            PyValue::Int(0),
            PyValue::Int(0),
        ],
    )?;
    Ok(match (answer.refusal(), answer.value()) {
        (Some(key), _) => Err(key),
        (None, Some(PyValue::Int(id))) => Ok(id),
        (None, other) => Err(format!("unexpected answer {other:?}")),
    })
}

/// A corp's name for a text: its own, or a plain word when the hall has not
/// read it yet.
fn name_of(hall: &Hall, corp: i64) -> Option<String> {
    hall.lock()
        .corps
        .get(&corp)
        .map(|c| c.name.clone())
        .filter(|n| !n.is_empty())
}

/// **The recruitment mail**, after `corp`'s CEO accepted `who`'s application
/// (`Plan::Recruit`): one act line, `None` when there is nothing to say.
pub fn recruit(conn: &mut Conn, hall: &Hall, corp: i64, who: i64) -> Result<Option<String>> {
    let name = name_of(hall, corp).unwrap_or_else(|| "our corporation".to_owned());
    let (title, body) = welcome_text(&name);
    Ok(Some(match send(conn, &[who], &title, &body)? {
        Ok(_) => format!("mail {who}: welcome to {corp}: ok"),
        Err(key) => format!("mail {who}: welcome to {corp}: refused {key}"),
    }))
}

/// **The diplomacy mail**, after `owner` (a bot corp, through its CEO) set
/// a contact on `target` with `standing` (`Plan::Contacts`): to the CEO of
/// `target` when it is a corp the hall knows, to `target` itself when it is
/// a character (a red set on a pilot), nothing for an alliance or a corp
/// whose CEO is not known. A neutral standing says nothing.
pub fn diplomacy(
    conn: &mut Conn,
    hall: &Hall,
    owner: i64,
    target: i64,
    standing: f64,
) -> Result<Option<String>> {
    if standing == 0.0 {
        return Ok(None);
    }
    let me = conn.session().char_id();
    let (to, their) = if society::is_player_corp(target) {
        // The lock is not held across `name_of`, which takes it too.
        let ceo = hall.lock().corps.get(&target).and_then(|c| c.ceo);
        match ceo {
            Some(ceo) => (
                ceo,
                name_of(hall, target).unwrap_or_else(|| "your corporation".to_owned()),
            ),
            None => return Ok(None),
        }
    } else if target > 0 && !society::PLAYER_ALLIANCES.contains(&target) {
        (target, "you".to_owned())
    } else {
        return Ok(None);
    };
    if Some(to) == me {
        return Ok(None);
    }
    let ours = name_of(hall, owner).unwrap_or_else(|| "A corporation".to_owned());
    let (title, body) = standing_text(&ours, &their, standing);
    let sent = send(conn, &[to], &title, &body)?;
    if let (Ok(_), Some(me), true) = (&sent, me, standing > 0.0) {
        hall.lock().offers.insert((me, to));
    }
    Ok(Some(match sent {
        Ok(_) => format!(
            "mail {to}: {} to {target}: ok",
            match standing {
                s if s >= society::BLUE_STANDING => "blue",
                s if s > 0.0 => "friendly",
                s if s <= society::RED_STANDING => "red",
                _ => "wary",
            }
        ),
        Err(key) => format!("mail {to}: standing on {target}: refused {key}"),
    }))
}

/// One mail heard in the inbox (R-REL-2): who wrote, and whether it reads
/// as the answer to one of our blue offers. **The text is never read**:
/// only the header's title and sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heard {
    pub id: i64,
    pub sender: i64,
    pub reply_to_offer: bool,
}

/// A header (a `SyncMail` `newMail` row) as a [`Heard`], `None` for one with
/// no id or sender, or from `me` (the sent copy). A title that starts with
/// `Re:` and holds the blue offer's words ("blue standing",
/// [`diplomacy_text`]) is a reply to an offer **[A]**: the client prefixes
/// `Re: ` to a reply's subject [V `mailSvc`/`mail.py`]; falsified by a human
/// reply that the client titles otherwise (the accept is then missed, the
/// talk still counts).
pub fn heard(header: &PyValue, me: i64) -> Option<Heard> {
    let id = crate::observe::int(header, "messageID")?;
    let sender = crate::observe::int(header, "senderID").filter(|s| *s > 0 && *s != me)?;
    let title = crate::observe::text(header, "title").unwrap_or_default();
    let lower = title.trim_start().to_lowercase();
    Some(Heard {
        id,
        sender,
        reply_to_offer: lower.starts_with("re:")
            && (lower.contains("blue standing") || lower.contains("friendly standing")),
    })
}

/// **Sync the inbox** (`mailMgr.SyncMail(firstID, lastID)`, the client's own
/// call [V `mailSvc`]): the mails new since id 0, from others. Nothing is
/// marked read, so a human's own client still shows them unread.
pub fn sync_inbox(conn: &mut Conn, me: i64) -> Result<Vec<Heard>> {
    let out = conn.call("mailMgr", "SyncMail", vec![PyValue::None, PyValue::Int(0)])?;
    if out.refusal().is_some() {
        return Ok(Vec::new());
    }
    let Some(v) = out.value() else {
        return Ok(Vec::new());
    };
    Ok(crate::observe::list(&v, "newMail")
        .unwrap_or_default()
        .iter()
        .filter_map(|h| heard(h, me))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_standing_has_its_own_short_text() {
        let word = |s: f64| standing_text("Ours", "Theirs", s).0;
        assert!(word(society::BLUE_STANDING).contains("blue"));
        assert!(word(5.0).contains("friendly"));
        assert!(word(-5.0).contains("wary"));
        assert!(word(society::RED_STANDING).contains("red"));
        for s in [10.0, 5.0, -5.0, -10.0] {
            let (t, b) = standing_text("Ours", "Theirs", s);
            assert!(b.contains("Theirs") && b.contains("Ours"));
            assert!(t.len() < 150 && b.len() < MAX_TEXT);
        }
        let (t, b) = invite_text("Invented Ltd");
        assert!(t.contains("Invented Ltd") && b.len() < MAX_TEXT);
    }

    #[test]
    fn the_doctrine_mail_names_the_hull_srp_and_staging_in_one_short_text() {
        let (t, b) = doctrine_text("Invented Ltd", "armour/short/brawl", Some(12), Some(34));
        assert!(t.contains("Invented Ltd") && b.contains("armour/short/brawl"));
        assert!(b.contains("showinfo:12") && b.contains("showinfo:5//34"));
        assert!(b.contains("ship replacement"));
        assert!(t.len() < 150 && b.len() < MAX_TEXT);
        let (_, bare) = doctrine_text("Invented Ltd", "k", None, None);
        assert!(!bare.contains("showinfo"));
    }

    #[test]
    fn a_reply_to_a_friendly_offer_reads_as_an_accept() {
        let header = |title: &str| {
            PyValue::Dict(vec![
                (PyValue::Int(9), PyValue::Bytes(b"messageID".to_vec())),
                (PyValue::Int(7), PyValue::Bytes(b"senderID".to_vec())),
                (
                    PyValue::Unicode(title.to_owned()),
                    PyValue::Bytes(b"title".to_vec()),
                ),
            ])
        };
        let (t, _) = friendly_text("Ours", "Theirs");
        assert!(
            heard(&header(&format!("Re: {t}")), 1)
                .unwrap()
                .reply_to_offer
        );
        assert!(!heard(&header(&t), 1).unwrap().reply_to_offer);
    }

    #[test]
    fn the_texts_are_short_plain_and_name_both_corps() {
        let (t, b) = welcome_text("Invented Ltd");
        assert!(t.contains("Invented Ltd") && b.contains("Invented Ltd"));
        for (blue, word) in [(true, "blue"), (false, "red")] {
            let (t, b) = diplomacy_text("Ours", "Theirs", blue);
            assert!(t.contains(word) && b.contains("Theirs") && b.contains("Ours"));
            assert!(t.len() < 150 && b.len() < MAX_TEXT);
        }
        assert!(welcome_text("x").1.len() < MAX_TEXT);
    }
}
