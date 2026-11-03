//! Which client builds this server speaks to (R-BUILD §3.3, owner decision
//! O17: stay compatible with the newest client build).
//!
//! **The server speaks first.** H1 carries `(macho, version, build,
//! codename@region)` before the client has said anything, and the client
//! accepts any server build **not higher** than its own. So one H1 serves
//! every build of one [`ReleaseLine`] if it advertises the line's minimum
//! accepted build ([`line_min_build`]); a different line needs a different
//! listener or process. Everything after H2 is per connection: the session
//! carries the client's exact build.
//!
//! [`BUILDS`] is reviewable Rust, not generated (`check_deps.py` forbids a
//! `build.rs`). Its test parses `tools/builds.tsv`, the registry the Python
//! tools read, and fails if the two disagree. A build-specific value becomes
//! code here only on the day it differs between builds: 3628513 needs none.
//! Since 2026-10-31 the reference is 3662168; 3619879 and 3628513 stay
//! registered as `unverified` (reachable with `--client-build`). The
//! 2026-11-09 build will repeat this re-pin.

/// The wire identity of one release line: everything in H1/H2 the client
/// compares for equality. Every build of a line shares it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReleaseLine {
    /// `eveMachoNetVersion.machoVersion`. The client refuses a mismatch.
    pub macho: u32,
    /// `boot.version`; must marshal as a **real** (the check is type-strict).
    pub version: f64,
    /// `boot.codename`.
    pub codename: &'static str,
    /// `boot.region`.
    pub region: &'static str,
    /// `GPSTransport.__HANDSHAKE_SECRET_KEY`.
    pub secret: u32,
    /// `handShakePaddingLength`.
    pub padding: usize,
    /// `codename + "@" + region`, the combined H1/H2 field.
    pub codename_at_region: &'static str,
}

/// How far a registered build has been proved to behave like the pinned one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The build the RE corpus is anchored to. Exactly one.
    Pinned,
    /// `tools/build-delta.py PINNED THIS` said `equivalent`: no protocol,
    /// call-surface, schema or static-data change.
    Equivalent,
    /// Seen, not measured. Refused in strict mode unless named explicitly.
    Unverified,
}

impl Status {
    /// Accepted by default (strict mode, the content bundle).
    pub fn accepted(self) -> bool {
        matches!(self, Status::Pinned | Status::Equivalent)
    }

    /// The word `tools/builds.tsv` uses.
    pub fn name(self) -> &'static str {
        match self {
            Status::Pinned => "pinned",
            Status::Equivalent => "equivalent",
            Status::Unverified => "unverified",
        }
    }
}

/// One registered build.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuildEntry {
    /// `start.ini`'s `build`.
    pub build: u32,
    /// Its release line.
    pub line: &'static ReleaseLine,
    /// What has been proved about it.
    pub status: Status,
    /// A content bundle `data_hash` (`docs/content/schema.md` §1.2) proved
    /// equal for this build, if one is registered: a bundle carrying it is
    /// accepted whatever its `client_build` says.
    pub data_hash: Option<&'static str>,
}

/// V24.01, region `ccp` (`start.ini`, `GPS.pyc`, `eveMachoNetVersion.pyc`;
/// `docs/reference/login-path.md`). Identical in 3619879 and 3628513 **[V]**
/// (`findings/build-delta-3619879-3628513.tsv`), and in 3662168: the delta
/// lists no gps / machoNet / eveMachoNetVersion module as changed and
/// `start.ini` keeps version, codename, region and branch
/// (`findings/build-delta-3619879-3662168.tsv`) **[V]**.
pub const V24_01: ReleaseLine = ReleaseLine {
    macho: 496,
    version: 24.01,
    codename: "V24.01",
    region: "ccp",
    secret: 170_472,
    padding: 64,
    codename_at_region: "V24.01@ccp",
};

/// The release line the server speaks. A new line is a one-constant edit
/// here (plus its `ReleaseLine` and `BuildEntry` rows): every alias in
/// `constants::handshake` and every test reads this.
pub const REFERENCE_LINE: ReleaseLine = V24_01;

/// The pinned build: what the RE corpus and `tools/client-pin.sha256` refer
/// to. Re-pinned from 3619879 to 3662168 on 2026-10-31; older `[V] build
/// 3619879` comments in the tree are history and still true of that build.
pub const REFERENCE: u32 = 3_662_168;

/// Every registered build. Mirrors `tools/builds.tsv` (checked by test).
pub static BUILDS: &[BuildEntry] = &[
    BuildEntry {
        build: 3_619_879,
        line: &V24_01,
        status: Status::Unverified,
        data_hash: None,
    },
    BuildEntry {
        build: 3_628_513,
        line: &V24_01,
        status: Status::Unverified,
        data_hash: None,
    },
    BuildEntry {
        build: REFERENCE,
        line: &V24_01,
        status: Status::Pinned,
        data_hash: None,
    },
];

/// The registry entry for `build`.
pub fn entry(build: u32) -> Option<&'static BuildEntry> {
    BUILDS.iter().find(|e| e.build == build)
}

/// The pinned build's entry.
pub fn reference() -> &'static BuildEntry {
    entry(REFERENCE).expect("REFERENCE is registered")
}

/// Every build accepted by default ([`Status::accepted`]), ascending.
pub fn accepted() -> Vec<u32> {
    let mut v: Vec<u32> = BUILDS
        .iter()
        .filter(|e| e.status.accepted())
        .map(|e| e.build)
        .collect();
    v.sort_unstable();
    v
}

/// What H1 advertises for `line`: the lowest accepted build on it, so every
/// accepted client (whose check is `client >= server`) passes.
pub fn line_min_build(line: &ReleaseLine) -> u32 {
    BUILDS
        .iter()
        .filter(|e| e.status.accepted() && e.line == line)
        .map(|e| e.build)
        .min()
        .expect("every registered line has an accepted build")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tools/builds.tsv`, the registry the Python tools read.
    fn registry_tsv() -> Vec<(u32, String, String, String)> {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tools/builds.tsv");
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        text.lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                assert_eq!(f.len(), 5, "builds.tsv row {l:?}");
                (
                    f[0].parse().expect("build number"),
                    f[1].to_owned(),
                    f[2].to_owned(),
                    f[3].to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn the_table_matches_tools_builds_tsv() {
        let mut want = registry_tsv();
        let mut got: Vec<(u32, String, String, String)> = BUILDS
            .iter()
            .map(|e| {
                (
                    e.build,
                    e.line.codename.to_owned(),
                    e.status.name().to_owned(),
                    e.data_hash.unwrap_or("-").to_owned(),
                )
            })
            .collect();
        want.sort();
        got.sort();
        assert_eq!(
            got, want,
            "eve-marshal::build::BUILDS and tools/builds.tsv disagree"
        );
    }

    #[test]
    fn exactly_one_build_is_pinned_and_it_is_the_reference() {
        let pinned: Vec<u32> = BUILDS
            .iter()
            .filter(|e| e.status == Status::Pinned)
            .map(|e| e.build)
            .collect();
        assert_eq!(pinned, vec![REFERENCE]);
        assert_eq!(reference().build, REFERENCE);
    }

    #[test]
    fn every_line_advertises_its_lowest_accepted_build() {
        for e in BUILDS.iter().filter(|e| e.status.accepted()) {
            let min = line_min_build(e.line);
            assert!(min <= e.build, "{} below its line's minimum {min}", e.build);
            assert!(accepted().contains(&min));
        }
    }

    #[test]
    fn builds_are_unique_and_lines_are_self_consistent() {
        for (i, a) in BUILDS.iter().enumerate() {
            assert!(BUILDS[i + 1..].iter().all(|b| b.build != a.build));
            let l = a.line;
            assert_eq!(l.codename_at_region, format!("{}@{}", l.codename, l.region));
            // The type is what matters: a whole-number version still
            // marshals as a Real because the field is an f64.
            assert!(l.version.is_finite(), "boot.version must be a real");
        }
        assert_eq!(entry(0), None);
    }
}
