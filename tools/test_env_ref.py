"""Tests for tools/env-ref.py, on an invented tree (stdlib only)."""

import importlib.util
import os
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("env_ref", os.path.join(HERE, "env-ref.py"))
env_ref = importlib.util.module_from_spec(spec)
spec.loader.exec_module(env_ref)

CLI = 'const USAGE: &str = "\\\n  --alpha N   a flag\n  --beta      another\n";\n#[cfg(test)]\nmod t { const X: &str = "--gamma"; }\n'
MOD = 'pub const FLAGS: &[Flag] = &[Flag { name: "--world-zeta", takes_value: true }];\n'
BOTS = 'match a { "--connect" => 1, "--quiet" => 2 }\n'
SH = '#!/bin/sh\nX=${EVE_FOO:-1}\n# EVE_BAR is read below\n'
DOC = """# ref
| `--alpha N` | x |
| `--beta` | x |
| `--world-zeta` | x |
| `--connect IP:PORT` | x |
| `--quiet` | x |
| `EVE_FOO` | x |
| `EVE_BAR` | x |
"""


class EnvRefTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        r = self.root = self.tmp.name
        self.put("server/crates/eve-server/src/cli.rs", CLI)
        self.put("server/crates/eve-mod-world/src/settings.rs", MOD)
        self.put("server/crates/eve-bot/src/bin/eve-bots.rs", BOTS)
        self.put("tools/x.sh", SH)
        self.put("tools/notes.md", "EVE_IN_MARKDOWN is not code\n")
        self.put("server/target/debug/junk.rs", "EVE_IN_TARGET\n")
        self.put("docs/reference/launch-and-env.md", DOC)
        self.assertTrue(os.path.isdir(r))

    def tearDown(self):
        self.tmp.cleanup()

    def put(self, rel, text):
        p = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(text)

    def test_scan_env_skips_markdown_and_target(self):
        names = env_ref.scan_env(self.root)
        self.assertEqual(sorted(names), ["EVE_BAR", "EVE_FOO"])

    def test_scan_flags_ignores_test_module(self):
        flags = env_ref.scan_flags(self.root)
        self.assertEqual(flags["eve-server"], {"--alpha", "--beta", "--world-zeta"})
        self.assertEqual(flags["eve-bots"], {"--connect", "--quiet"})

    def test_check_ok(self):
        self.assertEqual(env_ref.check(self.root), [])

    def test_check_undocumented(self):
        self.put("tools/y.py", 'import os\nos.environ.get("EVE_NEW")\n')
        problems = env_ref.check(self.root)
        self.assertEqual(len(problems), 1)
        self.assertIn("undocumented: EVE_NEW", problems[0])

    def test_check_undocumented_flag(self):
        self.put("server/crates/eve-bot/src/bin/eve-bots.rs", BOTS + '"--new-one"\n')
        self.assertIn("undocumented: eve-bots --new-one", env_ref.check(self.root))

    def test_check_stale(self):
        self.put("docs/reference/launch-and-env.md", DOC + "| `EVE_GONE` | x |\n| `--gone` | x |\n")
        problems = env_ref.check(self.root)
        self.assertIn("stale: EVE_GONE is in the doc but not in the code", problems)
        self.assertIn("stale: --gone is in the doc but not in the code", problems)

    def test_prose_mentions_are_not_rows(self):
        self.put("docs/reference/launch-and-env.md", DOC + "Prose names `EVE_PROSE` freely.\n")
        self.assertEqual(env_ref.check(self.root), [])

    def test_private_paths_are_optional(self):
        self.put("private/export-private-paths.txt", "# private\ndeploy/\ntools/secret-host.sh\n")
        self.put("deploy/entrypoint.sh", "echo ${EVE_DEPLOY_ONLY:-x}\n")
        self.put("tools/secret-host.sh", "echo $EVE_HOST_ONLY\n")
        self.assertNotIn("EVE_HOST_ONLY", env_ref.scan_env(self.root))
        self.assertEqual(env_ref.check(self.root), [])  # not required
        self.put("docs/reference/launch-and-env.md", DOC + "| `EVE_DEPLOY_ONLY` | x |\n")
        self.assertEqual(env_ref.check(self.root), [])  # and not stale

    def test_missing_doc(self):
        os.remove(os.path.join(self.root, "docs/reference/launch-and-env.md"))
        self.assertEqual(len(env_ref.check(self.root)), 1)

    def test_main_check_exit_codes(self):
        self.assertEqual(env_ref.main(["--check", "--root", self.root]), 0)
        self.put("tools/z.sh", "echo $EVE_UNLISTED\n")
        self.assertEqual(env_ref.main(["--check", "--root", self.root]), 1)


if __name__ == "__main__":
    unittest.main()
