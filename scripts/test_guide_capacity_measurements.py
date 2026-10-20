"""Regression checks for exact CSV/JSON numeric evidence comparison."""
import csv
import tempfile
import unittest
from pathlib import Path

from scripts.check_guide_capacity_measurements import check_summary_csv


class SummaryCsvPrecisionTests(unittest.TestCase):
    def compare(self, json_value, csv_value):
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            results = folder / 'results.json'
            results.write_text(
                '{"simulation":{"summary":[{"metric":"test",'
                '"target":"Main.test","sample_count":null,'
                '"start_ps":"0","end_ps":"1","value":'
                + json_value + '}]}}'
            )
            summary = folder / 'summary.csv'
            with summary.open('w', newline='') as destination:
                writer = csv.writer(destination)
                writer.writerow(['metric', 'target', 'sample_count', 'start_ps', 'end_ps', 'value'])
                writer.writerow(['test', 'Main.test', '', '0', '1', csv_value])
            check_summary_csv(summary, results)

    def test_adjacent_large_integers_are_different(self):
        for token in ['9007199254740992', '"9007199254740992"']:
            with self.subTest(token=token), self.assertRaises(AssertionError):
                self.compare(token, '9007199254740993')

    def test_decimals_beyond_float_precision_are_different(self):
        with self.assertRaises(AssertionError):
            self.compare('0.123456789012345678901', '0.123456789012345678902')

    def test_exact_matches_preserve_original_decimal_tokens(self):
        for token in ['9007199254740993', '0.123456789012345678901']:
            with self.subTest(token=token):
                self.compare(token, token)

    def test_equivalent_decimal_spellings_match(self):
        for token, saved in [('1.2300', '1.23'), ('"123e-2"', '1.2300'), ('1000', '1e3')]:
            with self.subTest(token=token, saved=saved):
                self.compare(token, saved)

    def test_null_matches_only_empty_csv(self):
        self.compare('null', '')
        with self.assertRaises(AssertionError):
            self.compare('null', '0')
        with self.assertRaises(AssertionError):
            self.compare('0', '')


if __name__ == '__main__':
    unittest.main()
