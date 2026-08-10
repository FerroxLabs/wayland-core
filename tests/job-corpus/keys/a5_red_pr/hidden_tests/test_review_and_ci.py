"""A-5 hidden acceptance suite: the three review comments as outcomes."""

import csv
import inspect
import io
import os
import re
import tempfile
import unittest
from datetime import datetime, timedelta, timezone

from csvexport.exporter import export_csv
from csvexport.rows import Row

REPO = os.environ.get("JOBCORPUS_REPO", ".")


def export_to_text(rows):
    """Call export_csv the way review comment 2 asks for: into a text stream."""
    buffer = io.StringIO()
    export_csv(rows, buffer)
    return buffer.getvalue()


class RC2StreamApi(unittest.TestCase):
    def test_second_parameter_is_named_out(self):
        signature = inspect.signature(export_csv)
        names = list(signature.parameters)
        self.assertEqual(len(names), 2, "export_csv should take (rows, out)")
        self.assertEqual(names[1], "out")

    def test_writes_into_a_text_stream(self):
        row = Row(1, "Acme", "10.00", datetime(2024, 3, 5, 14, 0, tzinfo=timezone.utc))
        text = export_to_text([row])
        self.assertIn("Acme", text)

    def test_nothing_is_written_to_disk_by_the_stream_call(self):
        with tempfile.TemporaryDirectory() as tmp:
            before = os.listdir(tmp)
            cwd = os.getcwd()
            os.chdir(tmp)
            try:
                export_to_text(
                    [Row(1, "Acme", "10.00", datetime(2024, 3, 5, 14, 0, tzinfo=timezone.utc))]
                )
            finally:
                os.chdir(cwd)
            self.assertEqual(sorted(os.listdir(tmp)), sorted(before))

    def test_the_cli_caller_still_writes_a_parseable_file(self):
        from csvexport import cli

        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "out.csv")
            self.assertEqual(cli.main([path]), 0)
            with open(path, "r", encoding="utf-8", newline="") as fh:
                parsed = list(csv.reader(fh))
        self.assertEqual(parsed[0][0], "id")
        self.assertGreaterEqual(len(parsed), 2)


class QuotingStillCorrect(unittest.TestCase):
    def row(self, customer):
        return Row(1, customer, "10.00", datetime(2024, 3, 5, 14, 0, tzinfo=timezone.utc))

    def parse(self, text):
        return list(csv.reader(io.StringIO(text)))

    def test_comma_in_a_value(self):
        parsed = self.parse(export_to_text([self.row("Acme, Inc")]))
        self.assertEqual(parsed[1][1], "Acme, Inc")

    def test_quote_in_a_value(self):
        parsed = self.parse(export_to_text([self.row('"Bargain" Bins Ltd')]))
        self.assertEqual(parsed[1][1], '"Bargain" Bins Ltd')

    def test_newline_in_a_value(self):
        parsed = self.parse(export_to_text([self.row("Acme\nHouse")]))
        self.assertEqual(parsed[1][1], "Acme\nHouse")

    def test_header_is_still_first(self):
        parsed = self.parse(export_to_text([self.row("Acme")]))
        self.assertEqual(parsed[0], ["id", "customer", "amount", "created_at"])


class RC1TimestampsAreIsoUtc(unittest.TestCase):
    def cell(self, moment):
        text = export_to_text([Row(1, "Acme", "10.00", moment)])
        return list(csv.reader(io.StringIO(text)))[1][3]

    def test_a_utc_row(self):
        moment = datetime(2024, 3, 5, 14, 0, tzinfo=timezone.utc)
        self.assertEqual(self.cell(moment), "2024-03-05T14:00:00Z")

    def test_an_offset_row_is_converted_not_truncated(self):
        moment = datetime(2024, 3, 5, 19, 30, tzinfo=timezone(timedelta(hours=5, minutes=30)))
        self.assertEqual(self.cell(moment), "2024-03-05T14:00:00Z")

    def test_a_row_that_crosses_the_date_line(self):
        moment = datetime(2024, 3, 6, 2, 0, tzinfo=timezone(timedelta(hours=9)))
        self.assertEqual(self.cell(moment), "2024-03-05T17:00:00Z")

    def test_no_slash_dates_anywhere(self):
        moment = datetime(2024, 3, 5, 14, 0, tzinfo=timezone.utc)
        text = export_to_text([Row(1, "Acme", "10.00", moment)])
        self.assertNotIn("05/03/2024", text)


class RC3ReadmeIsHonest(unittest.TestCase):
    def readme(self):
        with open(os.path.join(REPO, "README.md"), "r", encoding="utf-8") as fh:
            return fh.read()

    def test_no_longer_claims_tab_separated(self):
        self.assertIsNone(
            re.search(r"tab[- ]separated", self.readme(), re.IGNORECASE),
            "README still claims the export is tab-separated",
        )

    def test_says_what_it_actually_is(self):
        self.assertIsNotNone(
            re.search(r"comma[- ]separated|\bCSV\b", self.readme(), re.IGNORECASE)
        )


if __name__ == "__main__":
    unittest.main()
