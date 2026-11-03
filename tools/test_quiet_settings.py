#!/usr/bin/env python3
"""tools/client-ci/quiet-settings.py: forced for a run, the owner's file back after."""
import importlib.util
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("quiet", os.path.join(HERE, "client-ci", "quiet-settings.py"))
quiet = importlib.util.module_from_spec(spec)
spec.loader.exec_module(quiet)

OWNER = """audio:
  masterVolume: [11, 0.7]
  ownerAudioKey: [11, 1]
device:
  FullScreenSettings:
  - 11
  - {adapter: 0, height: 10}
  shaderQuality: [11, 3]
generic: {}
ui:
  shaderQuality: [11, 9]
"""


class Force(unittest.TestCase):
    def test_keys_change_and_the_rest_stay(self):
        out = quiet.force(OWNER)
        self.assertIn("  audioEnabled: [%d, 0]\n" % quiet.STAMP, out)
        self.assertIn("  masterVolume: [%d, 0.0]\n" % quiet.STAMP, out)
        self.assertIn("  shaderQuality: [%d, 1]\n" % quiet.STAMP, out)
        self.assertNotIn("[11, 0.7]", out)
        self.assertNotIn("[11, 3]", out)
        for kept in ("  ownerAudioKey: [11, 1]\n", "  - {adapter: 0, height: 10}\n", "generic: {}\n"):
            self.assertIn(kept, out)

    def test_another_section_keeps_a_same_named_key(self):
        self.assertIn("ui:\n  shaderQuality: [11, 9]\n", quiet.force(OWNER))

    def test_a_multi_line_value_goes_whole(self):
        text = "device:\n  shaderQuality:\n  - 11\n  - 3\n  ownerKey: [11, 1]\n"
        out = quiet.force(text)
        self.assertNotIn("- 3", out)
        self.assertIn("  ownerKey: [11, 1]\n", out)

    def test_empty_file_gets_both_sections(self):
        out = quiet.force("")
        self.assertIn("audio:\n", out)
        self.assertIn("device:\n", out)

    def test_empty_section_is_opened(self):
        self.assertIn("audio:\n  audioEnabled", quiet.force("audio: {}\n"))


class RoundTrip(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = os.path.join(self.tmp.name, quiet.SETTINGS)

    def read(self):
        with open(self.path, encoding="utf-8") as f:
            return f.read()

    def write(self, text):
        with open(self.path, "w", encoding="utf-8") as f:
            f.write(text)

    def test_restore_gives_back_the_owners_bytes(self):
        self.write(OWNER)
        quiet.apply(self.tmp.name)
        self.assertNotEqual(self.read(), OWNER)
        quiet.restore(self.tmp.name)
        self.assertEqual(self.read(), OWNER)
        self.assertEqual(os.listdir(self.tmp.name), [quiet.SETTINGS])

    def test_a_dead_run_cannot_lose_the_owners_file(self):
        self.write(OWNER)
        quiet.apply(self.tmp.name)
        quiet.apply(self.tmp.name)  # no restore between: the first run died
        quiet.restore(self.tmp.name)
        self.assertEqual(self.read(), OWNER)

    def test_no_file_before_means_no_file_after(self):
        quiet.apply(self.tmp.name)
        self.assertTrue(os.path.exists(self.path))
        quiet.restore(self.tmp.name)
        self.assertEqual(os.listdir(self.tmp.name), [])

    def test_restore_with_nothing_kept_does_nothing(self):
        self.write(OWNER)
        quiet.restore(self.tmp.name)
        self.assertEqual(self.read(), OWNER)


if __name__ == "__main__":
    sys.exit(unittest.main())
