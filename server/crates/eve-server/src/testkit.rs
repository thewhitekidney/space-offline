//! Shared test fixtures (review packet IP18; V4 §4, S3).
//!
//! Three things every unit test in this crate keeps rebuilding by hand:
//!
//! - [`registry`]: a `Registry` on the default sandbox, the body of the
//!   private `fn registry()` copies in `service/tests.rs`,
//!   `service/fleet.rs` and `service/character.rs`;
//! - [`registry_with`]: the same with the config adjusted first, for a test
//!   that needs its own characters or flags;
//! - [`scratch_dir`]: a fresh directory whose name carries a tag, the pid
//!   and a per-process counter, removed when the guard drops (also on
//!   panic, which the hand-rolled `temp_dir().join(..)` sites never did).
//!
//! The pid separates two `cargo test` runs sharing one target directory
//! (lanes, the gate and `soak.sh` all do); the counter separates two tests
//! in one run that pick the same tag. That pair is the V4 S3 fix.
//!
//! Callers migrate when a lane is in the file anyway, never as a sweep
//! (V4 §4: a sweep touches every hot file).
//!
//! `#[cfg(test)]` only: integration tests under `tests/` cannot see it,
//! because they link the library built without `cfg(test)`.

use crate::model::SandboxConfig;
use crate::service::Registry;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A registry on the default sandbox config.
pub(crate) fn registry() -> Registry {
    Registry::new(SandboxConfig::default())
}

/// A registry built after `adjust` has edited the default sandbox config.
pub(crate) fn registry_with(adjust: impl FnOnce(&mut SandboxConfig)) -> Registry {
    let mut config = SandboxConfig::default();
    adjust(&mut config);
    Registry::new(config)
}

/// A directory owned by one test. Removed, with its contents, on drop.
#[derive(Debug)]
pub(crate) struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl std::ops::Deref for ScratchDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for ScratchDir {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        // Best effort: a failed cleanup must not turn a passing test into a
        // panic-in-drop abort.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A new, empty directory named `eve-<tag>-<pid>-<n>` under the system temp
/// dir. Unique across concurrent processes (pid) and across calls within one
/// process (`n`); a leftover of the same name from a recycled pid is cleared
/// first.
pub(crate) fn scratch_dir(tag: &str) -> ScratchDir {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("eve-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create scratch dir");
    ScratchDir { path }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_dir_is_removed_on_drop() {
        let dir = scratch_dir("testkit-drop");
        let kept = dir.path().to_path_buf();
        std::fs::create_dir_all(dir.join("nested")).expect("nested");
        std::fs::write(dir.join("nested/file"), b"x").expect("write");
        assert!(kept.is_dir());
        drop(dir);
        assert!(!kept.exists(), "{} survived the drop", kept.display());
    }

    #[test]
    fn scratch_dir_is_removed_when_the_test_panics() {
        let kept = std::panic::catch_unwind(|| {
            let dir = scratch_dir("testkit-panic");
            let p = dir.path().to_path_buf();
            std::panic::panic_any(p);
        })
        .expect_err("the closure panics");
        let kept = kept.downcast::<PathBuf>().expect("the path is the payload");
        assert!(!kept.exists(), "{} survived the unwind", kept.display());
    }

    #[test]
    fn two_scratch_dirs_with_one_tag_do_not_collide() {
        let a = scratch_dir("testkit-same");
        let b = scratch_dir("testkit-same");
        assert_ne!(a.path(), b.path());
        assert!(a.is_dir() && b.is_dir());
        let name = a.file_name().and_then(|s| s.to_str()).expect("utf-8 name");
        assert!(
            name.contains(&std::process::id().to_string()),
            "the pid is in {name}"
        );
    }

    #[test]
    fn registry_with_applies_the_adjustment() {
        let reg = registry_with(|c| c.user_name = "testkit-invented".to_owned());
        assert_eq!(reg.config().user_name, "testkit-invented");
        assert_ne!(registry().config().user_name, "testkit-invented");
    }
}
