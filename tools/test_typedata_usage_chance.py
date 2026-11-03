"""Unit tests for the effect column fittingUsageChanceAttributeID (B0).

Invented rows only. Run: `python3 -I tools/test_typedata_usage_chance.py -q`.
"""

from __future__ import annotations

import os
import sys
import unittest

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import typedata as T  # noqa: E402


class UsageChance(unittest.TestCase):
    def test_the_column_is_written_last_and_defaults_to_zero(self):
        with_it = T.effect_line(990501, {"fittingUsageChanceAttributeID": 990502})
        without = T.effect_line(990503, {})
        self.assertEqual(with_it.split("\t")[12], "990502")
        self.assertEqual(without.split("\t")[12], "0")

    def test_require_demands_one_non_zero_value(self):
        none = T.effect_line(990503, {}) + "\n"
        some = T.effect_line(990501, {"fittingUsageChanceAttributeID": 990502}) + "\n"
        self.assertTrue(T.usage_chance_problems(none, require=True))
        self.assertEqual(T.usage_chance_problems(none), [])
        self.assertEqual(T.usage_chance_problems(some, require=True), [])


if __name__ == "__main__":
    unittest.main()
