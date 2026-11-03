"""Tests for `tools/check_deps.py` (review V6 F11; packet IP20).

Each test builds a synthetic server tree in a temp dir: a clean tree that
passes, plus one defect. Invented crate names only; the real tree is never
read, so this file says nothing about what the server depends on.

Run::

    python3 -I tools/test_check_deps.py
"""

from __future__ import annotations

import os
import pathlib
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.append(HERE)  # last, so nothing in tools/ shadows the stdlib

import check_deps  # noqa: E402

CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"

ROOT = """\
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.dependencies]
"""

CRATE = """\
[package]
name = "{name}"
version = "0.0.1"

[dependencies]
"""

CONFIG = """\
[source.crates-io]
replace-with = "distro"

[source.distro]
directory = "/usr/share/cargo/registry"

[net]
offline = true
"""

LOCK = """\
version = 4

[[package]]
name = "alpha"
version = "0.0.1"
"""


class CheckDeps(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.server = pathlib.Path(self._tmp.name) / "server"
        self.write("Cargo.toml", ROOT)
        self.write("deps.allow", "# none\n")
        self.write("Cargo.lock", LOCK)
        self.write(".cargo/config.toml", CONFIG)
        self.write("crates/alpha/Cargo.toml", CRATE.format(name="alpha"))
        self.write("crates/alpha/src/lib.rs", "")

    def tearDown(self):
        self._tmp.cleanup()

    def write(self, rel, text):
        p = self.server / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)

    def append(self, rel, text):
        p = self.server / rel
        p.write_text(p.read_text() + text)

    def errors(self):
        return check_deps.check(self.server)[0]

    def assertFails(self, *needles):
        errs = self.errors()
        self.assertTrue(errs, "expected a failure, got none")
        joined = "\n".join(errs)
        for n in needles:
            self.assertIn(n, joined)

    # --- the clean tree --------------------------------------------------

    def test_a_clean_std_only_tree_passes(self):
        self.assertEqual(self.errors(), [])

    def test_main_exit_status_follows_the_errors(self):
        orig = check_deps.SERVER
        try:
            check_deps.SERVER = self.server
            with open(os.devnull, "w") as null:
                out, err = sys.stdout, sys.stderr
                sys.stdout = sys.stderr = null
                try:
                    clean = check_deps.main()
                    self.write(".cargo/config.toml", CONFIG.replace("offline = true", ""))
                    dirty = check_deps.main()
                finally:
                    sys.stdout, sys.stderr = out, err
        finally:
            check_deps.SERVER = orig
        self.assertEqual((clean, dirty), (0, 1))

    # --- Cargo.lock sources ----------------------------------------------

    def test_a_lock_package_with_any_source_fails_while_the_allowlist_is_empty(self):
        self.append("Cargo.lock", f'\n[[package]]\nname = "zeta"\nversion = "1.0.0"\nsource = "{CRATES_IO}"\n')
        self.assertFails("Cargo.lock", "'zeta'", "not in deps.allow")

    def test_a_git_lock_source_fails_even_when_the_name_is_allowlisted(self):
        self.write("deps.allow", "zeta\n")
        self.write("Cargo.toml", ROOT + 'zeta = "1"\n')
        self.append("Cargo.lock", '\n[[package]]\nname = "zeta"\nversion = "1.0.0"\n'
                    'source = "git+https://example.invalid/zeta#0123"\n')
        self.assertFails("Cargo.lock", "'zeta'", "not the distro registry")

    def test_an_allowlisted_crates_io_lock_package_passes(self):
        self.write("deps.allow", "zeta\n")
        self.write("Cargo.toml", ROOT + 'zeta = "1"\n')
        self.append("Cargo.lock", f'\n[[package]]\nname = "zeta"\nversion = "1.0.0"\nsource = "{CRATES_IO}"\n')
        self.assertEqual(self.errors(), [])

    # --- [patch] and [replace] -------------------------------------------

    def test_patch_in_the_workspace_manifest_fails(self):
        self.append("Cargo.toml", '\n[patch.crates-io]\nzeta = { path = "../zeta" }\n')
        self.assertFails("server/Cargo.toml", "[patch]")

    def test_replace_in_the_workspace_manifest_fails(self):
        self.append("Cargo.toml", '\n[replace]\n"zeta:1.0.0" = { path = "../zeta" }\n')
        self.assertFails("server/Cargo.toml", "[replace]")

    def test_patch_in_a_crate_manifest_fails(self):
        self.append("crates/alpha/Cargo.toml", '\n[patch.crates-io]\nzeta = { path = "../zeta" }\n')
        self.assertFails("crates/alpha/Cargo.toml", "[patch]")

    def test_patch_in_the_cargo_config_fails(self):
        self.append(".cargo/config.toml", '\n[patch.crates-io]\nzeta = { path = "../zeta" }\n')
        self.assertFails(".cargo/config.toml", "[patch]")

    # --- .cargo/config.toml ----------------------------------------------

    def test_a_missing_offline_line_fails(self):
        self.write(".cargo/config.toml", CONFIG.replace("offline = true", ""))
        self.assertFails("offline = true")

    def test_offline_false_fails(self):
        self.write(".cargo/config.toml", CONFIG.replace("offline = true", "offline = false"))
        self.assertFails("offline = true")

    def test_a_missing_replace_with_fails(self):
        self.write(".cargo/config.toml", CONFIG.replace('replace-with = "distro"\n', ""))
        self.assertFails('replace-with = "distro"')

    def test_replace_with_another_source_fails(self):
        self.write(".cargo/config.toml", CONFIG.replace('"distro"\n', '"mirror"\n', 1))
        self.assertFails('replace-with = "distro"')

    def test_a_distro_source_that_is_not_a_directory_fails(self):
        self.write(".cargo/config.toml", CONFIG.replace(
            'directory = "/usr/share/cargo/registry"', 'registry = "https://example.invalid/index"'))
        self.assertFails("[source.distro]")

    def test_a_missing_cargo_config_fails(self):
        (self.server / ".cargo" / "config.toml").unlink()
        self.assertFails(".cargo/config.toml", "missing")

    # --- build scripts ---------------------------------------------------

    def test_an_unlisted_build_rs_fails(self):
        self.write("crates/alpha/build.rs", "fn main() {}\n")
        self.assertFails("crates/alpha/build.rs", "BUILD_RS_ALLOW")

    def test_a_build_key_naming_another_script_fails(self):
        self.write("crates/alpha/Cargo.toml",
                   CRATE.format(name="alpha").replace('version = "0.0.1"\n', 'version = "0.0.1"\nbuild = "gen.rs"\n'))
        self.assertFails("crates/alpha/Cargo.toml", "build = 'gen.rs'")

    def test_an_allowlisted_build_rs_passes(self):
        self.write("crates/alpha/build.rs", "fn main() {}\n")
        orig = check_deps.BUILD_RS_ALLOW
        try:
            check_deps.BUILD_RS_ALLOW = frozenset({"alpha"})
            self.assertEqual(self.errors(), [])
        finally:
            check_deps.BUILD_RS_ALLOW = orig

    # --- the manifest rules that predate IP20 ----------------------------

    def test_a_registry_dependency_in_a_crate_fails(self):
        self.append("crates/alpha/Cargo.toml", 'zeta = "1"\n')
        self.assertFails("'zeta'", "path or `workspace = true`")

    def test_a_git_workspace_dependency_fails(self):
        self.write("deps.allow", "zeta\n")
        self.write("Cargo.toml", ROOT + 'zeta = { git = "https://example.invalid/zeta" }\n')
        self.assertFails("'zeta'", "distro registry")

    def test_an_unused_allowlist_entry_fails(self):
        self.write("deps.allow", "zeta\n")
        self.assertFails("deps.allow", "unused")


if __name__ == "__main__":
    unittest.main()
