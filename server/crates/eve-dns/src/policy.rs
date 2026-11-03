//! What the stub resolver is willing to say, and what it refuses to say.
//!
//! The jail (`docs/guides/jail.md`) gives the client no route anywhere, so it cannot
//! resolve anything by escaping. It resolves through us or not at all. The
//! client builds its server hostname in `serverInfo._GetServerIP` as
//! `"{}.servers.eveonline.com".format(serverName)` and resolves that, so one
//! suffix rule pointing at our own address is the whole of what it needs.
//!
//! Everything else is a name error. That is not only a default: DNS is also
//! the jail's canary. `tools/jail-verify.sh` asserts that `sentry.io` and
//! `clientresources.eveonline.com` do not resolve, so this policy answering
//! them would quietly destroy the test.

use std::net::Ipv4Addr;

use crate::wire::{CLASS_IN, DnsError, Name, QCLASS_ANY, QTYPE_ANY, Question, TYPE_A};

/// The one suffix the jail actually needs, from `serverInfo._GetServerIP`.
pub const EVE_SERVERS_SUFFIX: &str = "servers.eveonline.com";

/// Default TTL on the records we invent.
///
/// Short on purpose. The answers are fabricated, our address can change
/// between `jail-setup.sh` runs, and the client is the only resolver asking,
/// so caching buys nothing worth a stale answer.
pub const DEFAULT_TTL: u32 = 60;

/// What to do with one question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// A matched name asked for its address: answer with one `A` record.
    Address(Ipv4Addr),
    /// A matched name asked for something we do not have.
    ///
    /// The answer is `NOERROR` with an empty answer section, **not**
    /// `NXDOMAIN`. The distinction matters and is the subtle part of this
    /// crate: `NXDOMAIN` means "this name does not exist, for any type"
    /// (RFC 1035 §4.1.1 Name Error, sharpened by RFC 2308 §2.1), so a
    /// dual-stack resolver that gets `NXDOMAIN` for `AAAA` is entitled to
    /// cache the *name* as nonexistent and never ask for the `A` at all. The
    /// client would then fail to resolve a name we are perfectly willing to
    /// answer. `NOERROR` with no answers is RFC 2308 §2.2 NODATA: the name
    /// exists, this type does not.
    NoData,
    /// An unmatched name: `NXDOMAIN`, whatever the type.
    NameError,
}

/// Which names resolve, to what, and for how long.
#[derive(Debug, Clone)]
pub struct Policy {
    suffixes: Vec<Name>,
    target: Ipv4Addr,
    ttl: u32,
}

impl Policy {
    /// A policy that matches nothing: every query gets `NXDOMAIN`.
    ///
    /// Default-deny, in the same spirit as the jail's `policy drop`. Names are
    /// added deliberately with [`Policy::allow_suffix`].
    pub fn new(target: Ipv4Addr) -> Self {
        Policy {
            suffixes: Vec::new(),
            target,
            ttl: DEFAULT_TTL,
        }
    }

    /// The policy the jail runs: [`EVE_SERVERS_SUFFIX`] resolves to `target`,
    /// everything else is a name error.
    pub fn for_jail(target: Ipv4Addr) -> Self {
        let mut policy = Policy::new(target);
        policy
            .allow_suffix(EVE_SERVERS_SUFFIX)
            .expect("EVE_SERVERS_SUFFIX is a valid name");
        policy
    }

    /// Resolve `suffix` and everything beneath it to the target address.
    pub fn allow_suffix(&mut self, suffix: &str) -> Result<&mut Self, DnsError> {
        let name = Name::from_ascii(suffix)?;
        if name.is_root() {
            // The root as a suffix matches every name in existence, which
            // would turn a default-deny resolver into a wildcard one.
            return Err(DnsError::EmptyLabel);
        }
        if !self.suffixes.iter().any(|s| s.eq_ignore_case(&name)) {
            self.suffixes.push(name);
        }
        Ok(self)
    }

    /// Set the TTL on answered records, clamped by the codec to
    /// [`crate::wire::MAX_TTL`].
    pub fn set_ttl(&mut self, ttl: u32) -> &mut Self {
        self.ttl = ttl;
        self
    }

    /// The address matched names resolve to.
    pub fn target(&self) -> Ipv4Addr {
        self.target
    }

    /// The TTL on answered records.
    pub fn ttl(&self) -> u32 {
        self.ttl
    }

    /// The configured suffixes, in the order they were added.
    pub fn suffixes(&self) -> &[Name] {
        &self.suffixes
    }

    /// True if `name` is at or beneath a configured suffix.
    pub fn matches(&self, name: &Name) -> bool {
        self.suffixes.iter().any(|s| name.is_subdomain_of(s))
    }

    /// Decide one question.
    ///
    /// `A` in class `IN` (or `ANY` of either) on a matched name gets the
    /// address. Any other type or class on a matched name gets
    /// [`Decision::NoData`]. An unmatched name gets [`Decision::NameError`].
    pub fn decide(&self, question: &Question) -> Decision {
        if !self.matches(&question.name) {
            return Decision::NameError;
        }
        let class_ok = question.qclass == CLASS_IN || question.qclass == QCLASS_ANY;
        let type_ok = question.qtype == TYPE_A || question.qtype == QTYPE_ANY;
        if class_ok && type_ok {
            Decision::Address(self.target)
        } else {
            Decision::NoData
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{TYPE_AAAA, TYPE_SOA};

    const TARGET: Ipv4Addr = Ipv4Addr::new(10, 137, 0, 1);

    fn question(name: &str, qtype: u16) -> Question {
        Question {
            name: Name::from_ascii(name).unwrap(),
            qtype,
            qclass: CLASS_IN,
        }
    }

    #[test]
    fn the_jail_policy_answers_eve_and_nothing_else() {
        let policy = Policy::for_jail(TARGET);
        assert_eq!(
            policy.decide(&question("tranquility.servers.eveonline.com", TYPE_A)),
            Decision::Address(TARGET)
        );
        // These two are exactly what tools/jail-verify.sh asserts must not
        // resolve. If this test ever passes with an address, the jail's own
        // leak check has been disarmed.
        for denied in [
            "sentry.io",
            "clientresources.eveonline.com",
            "eveonline.com",
            "servers.eveonline.com.attacker.test",
            "google.com",
        ] {
            assert_eq!(
                policy.decide(&question(denied, TYPE_A)),
                Decision::NameError,
                "{denied} must not resolve"
            );
        }
    }

    #[test]
    fn an_inverted_policy_would_fail_here() {
        // The pair that pins the direction: a matched name must never be a
        // name error, and an unmatched name must never get an address. A
        // policy with its match test negated fails both halves.
        let policy = Policy::for_jail(TARGET);
        let matched = question("tranquility.servers.eveonline.com", TYPE_A);
        let unmatched = question("sentry.io", TYPE_A);
        assert_ne!(policy.decide(&matched), Decision::NameError);
        assert_ne!(policy.decide(&unmatched), Decision::Address(TARGET));
        assert!(policy.matches(&matched.name));
        assert!(!policy.matches(&unmatched.name));
    }

    #[test]
    fn aaaa_on_a_matched_name_is_nodata_never_nxdomain() {
        // The subtle one. NXDOMAIN here lets a dual-stack resolver cache the
        // whole name as nonexistent and skip the A query that would have
        // worked.
        let policy = Policy::for_jail(TARGET);
        assert_eq!(
            policy.decide(&question("tranquility.servers.eveonline.com", TYPE_AAAA)),
            Decision::NoData
        );
        assert_ne!(
            policy.decide(&question("tranquility.servers.eveonline.com", TYPE_AAAA)),
            Decision::NameError,
            "NXDOMAIN on AAAA poisons the name for A as well"
        );
        // Same reasoning for every other type on a name we do serve.
        for qtype in [
            TYPE_SOA, 2,  /* NS */
            15, /* MX */
            16, /* TXT */
            65, /* HTTPS */
        ] {
            assert_eq!(
                policy.decide(&question("tranquility.servers.eveonline.com", qtype)),
                Decision::NoData,
                "qtype {qtype} on a matched name"
            );
        }
        // But an unmatched name is a name error for AAAA too -- there the name
        // really does not exist.
        assert_eq!(
            policy.decide(&question("sentry.io", TYPE_AAAA)),
            Decision::NameError
        );
    }

    #[test]
    fn qtype_any_and_qclass_any_get_the_address() {
        let policy = Policy::for_jail(TARGET);
        let mut q = question("tranquility.servers.eveonline.com", QTYPE_ANY);
        assert_eq!(policy.decide(&q), Decision::Address(TARGET));
        q.qtype = TYPE_A;
        q.qclass = QCLASS_ANY;
        assert_eq!(policy.decide(&q), Decision::Address(TARGET));
    }

    #[test]
    fn a_class_we_do_not_serve_is_nodata_on_a_matched_name() {
        let policy = Policy::for_jail(TARGET);
        let mut q = question("tranquility.servers.eveonline.com", TYPE_A);
        q.qclass = 3; // CH, RFC 1035 3.2.4
        assert_eq!(policy.decide(&q), Decision::NoData);
    }

    #[test]
    fn an_empty_policy_denies_everything() {
        let policy = Policy::new(TARGET);
        assert!(policy.suffixes().is_empty());
        for name in ["servers.eveonline.com", "localhost", "a.b.c.d.e"] {
            assert_eq!(
                policy.decide(&question(name, TYPE_A)),
                Decision::NameError,
                "{name} with no suffixes configured"
            );
        }
    }

    #[test]
    fn extra_suffixes_can_be_added_and_the_apex_matches() {
        let mut policy = Policy::new(TARGET);
        policy
            .allow_suffix("servers.eveonline.com")
            .unwrap()
            .allow_suffix("Login.EveOnline.com")
            .unwrap();
        assert_eq!(policy.suffixes().len(), 2);
        for name in [
            "servers.eveonline.com",
            "SERVERS.EVEONLINE.COM",
            "login.eveonline.com",
            "sso.login.eveonline.com",
        ] {
            assert_eq!(
                policy.decide(&question(name, TYPE_A)),
                Decision::Address(TARGET),
                "{name}"
            );
        }
        // Adding the same suffix twice does not duplicate it.
        policy.allow_suffix("servers.eveonline.COM").unwrap();
        assert_eq!(policy.suffixes().len(), 2);
    }

    #[test]
    fn the_root_is_refused_as_a_suffix() {
        let mut policy = Policy::new(TARGET);
        assert!(policy.allow_suffix(".").is_err());
        assert!(policy.allow_suffix("").is_err());
        assert!(policy.allow_suffix("a..b").is_err());
        assert!(policy.allow_suffix(&"a".repeat(64)).is_err());
        assert!(policy.suffixes().is_empty());
    }

    #[test]
    fn the_ttl_is_configurable() {
        let mut policy = Policy::for_jail(TARGET);
        assert_eq!(policy.ttl(), DEFAULT_TTL);
        policy.set_ttl(7);
        assert_eq!(policy.ttl(), 7);
        assert_eq!(policy.target(), TARGET);
    }
}
