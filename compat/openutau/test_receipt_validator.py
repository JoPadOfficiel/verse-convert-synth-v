#!/usr/bin/env python3
"""Check the gate's actual validator against external, qualified full TRX files."""
import argparse
import copy
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

NS = {"t": "http://microsoft.com/schemas/VisualStudio/TeamTest/2010"}
PREFIX = "OpenUtau.App.NativePronunciationTest."
SCRIPT = Path(__file__).resolve().parents[2] / "scripts/test-openutau-clean-lyrics.sh"
VALIDATOR = SCRIPT.read_text().split("<<'PY_RECEIPT'\n", 1)[1].split("\nPY_RECEIPT", 1)[0]


class ReceiptValidatorTest(unittest.TestCase):
    receipts = []
    checks = 0

    def validate(self, root, revision, mode, expected):
        with tempfile.NamedTemporaryFile(suffix=".trx") as file:
            ET.ElementTree(root).write(file.name, encoding="utf-8", xml_declaration=True)
            # The production invocation must keep assertions active despite this.
            process = subprocess.run(
                [sys.executable, "-I", "-", file.name, revision, mode],
                input=VALIDATOR, text=True, capture_output=True,
                env=dict(os.environ, PYTHONOPTIMIZE="1"),
            )
        type(self).checks += 1
        self.assertEqual(process.returncode == 0, expected, process.stdout + process.stderr)

    def test_valid_full_and_fixtures_only_receipts(self):
        for revision, total, root in self.receipts:
            rows = root.findall(".//t:UnitTestResult", NS)
            self.assertEqual(len(rows), total)
            self.assertEqual(sum(row.get("testName", "").startswith(PREFIX) for row in rows), 46)
            for mode in ("full", "fixtures-only"):
                with self.subTest(revision=revision, mode=mode):
                    self.validate(root, revision, mode, True)

    def test_every_single_pronunciation_case_is_required(self):
        for revision, _, original in self.receipts:
            names = [r.get("testName") for r in original.findall(".//t:UnitTestResult", NS)
                     if r.get("testName", "").startswith(PREFIX)]
            for name in names:
                for mode in ("full", "fixtures-only"):
                    with self.subTest(revision=revision, mode=mode, missing=name):
                        root = copy.deepcopy(original)
                        results = root.find("t:Results", NS)
                        results.remove(next(r for r in results if r.get("testName") == name))
                        self.validate(root, revision, mode, False)

    def test_twenty_six_deleted_results_are_refused(self):
        for revision, total, original in self.receipts:
            root = copy.deepcopy(original)
            results = root.find("t:Results", NS)
            kept_methods = set()
            for row in list(results):
                name = row.get("testName", "")
                if not name.startswith(PREFIX):
                    continue
                method = name.split("(", 1)[0].removeprefix(PREFIX)
                keep = method == "ExactMfaHintReachesNativeProcess" or (
                    method in (
                        "VerseExportResolvesEveryNativeWordOverride",
                        "MissingDurationPhoneRaisesNativeErrorWithoutApproximateFallback",
                        "MissingAcousticPhoneFailsAfterSuccessfulNativePhonemization",
                        "FrenchWholeWordHintSurvivesNativeLifecycle",
                    ) and method not in kept_methods
                )
                if keep:
                    kept_methods.add(method)
                else:
                    results.remove(row)
            # Retain all 16 hint cases and every formerly checked >=1 method.
            self.assertEqual(len(results), total - 26)
            self.assertEqual(sum(r.get("testName", "").startswith(PREFIX) for r in results), 20)
            for mode in ("full", "fixtures-only"):
                with self.subTest(revision=revision, mode=mode):
                    self.validate(root, revision, mode, False)

    def test_same_count_duplicate_substitution_and_skipped_case_are_refused(self):
        for revision, _, original in self.receipts:
            for fault in ("duplicate", "different_argument", "unknown_method", "skipped"):
                root = copy.deepcopy(original)
                rows = [r for r in root.findall(".//t:UnitTestResult", NS)
                        if r.get("testName", "").startswith(PREFIX)]
                if fault == "duplicate":
                    rows[0].set("testName", rows[1].get("testName"))
                elif fault == "different_argument":
                    row = next(r for r in rows if "word:" in r.get("testName", ""))
                    row.set("testName", row.get("testName").replace("word:", "unexpected_argument:", 1))
                elif fault == "unknown_method":
                    rows[0].set("testName", PREFIX + "UnexpectedRegression")
                else:
                    rows[0].set("outcome", "NotExecuted")
                for mode in ("full", "fixtures-only"):
                    with self.subTest(revision=revision, mode=mode, fault=fault):
                        self.validate(root, revision, mode, False)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-trx", type=Path, required=True)
    parser.add_argument("--beta-trx", type=Path, required=True)
    arguments = parser.parse_args()
    ReceiptValidatorTest.receipts = [
        ("3f213e8993ca792c3e6f8958c92ab27eae78eac5", 68, ET.parse(arguments.baseline_trx).getroot()),
        ("ec7ba520583173c67aabfc5feab33390b4f720a4", 70, ET.parse(arguments.beta_trx).getroot()),
    ]
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(ReceiptValidatorTest)
    outcome = unittest.TextTestRunner(verbosity=2).run(suite)
    print(f"Validated {ReceiptValidatorTest.checks} receipt cases; no native build or fixture regeneration.")
    sys.exit(0 if outcome.wasSuccessful() else 1)
