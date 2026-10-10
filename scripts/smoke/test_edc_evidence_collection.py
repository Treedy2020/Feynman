"""Offline evidence preparation checks; no DWS, model or container calls."""
from datetime import datetime
import re
import unittest

from collect_edc_evaluation_evidence import CASES, collected_time, ledger


class EdcEvidenceTest(unittest.TestCase):
    def test_pinned_cases_have_full_shas_and_aware_cutoffs(self):
        for case in CASES:
            self.assertIsNotNone(re.fullmatch(r"[0-9a-f]{40}", case["base_sha"]))
            self.assertIsNotNone(datetime.fromisoformat(case["cutoff"]).tzinfo)
            self.assertEqual(len(case["ids"]), len(set(case["ids"])))

    def test_source_count_and_completeness_are_not_inferred(self):
        source = {"messages": [], "count": 0, "complete": True, "hasMore": False, "failures": []}
        self.assertTrue(ledger(source)["complete"])
        for change in ({"complete": False}, {"hasMore": True}, {"failures": ["failed page"]},
                       {"partial": True}, {"truncated": True}, {"count": 1}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                ledger(source | change)

    def test_raw_display_time_uses_explicit_shanghai_zone(self):
        value = collected_time({"createTime": "2026-09-08 17:21:35"})
        self.assertEqual(value.isoformat(), "2026-09-08T17:21:35+08:00")
        self.assertLess(value, datetime.fromisoformat(CASES[0]["cutoff"]))


if __name__ == "__main__":
    unittest.main()
