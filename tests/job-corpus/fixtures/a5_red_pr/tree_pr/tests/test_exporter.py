import csv
import io
import os
import tempfile
import unittest
from datetime import datetime, timezone

from csvexport.exporter import export_csv
from csvexport.rows import Row


class ExportCsv(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.path = os.path.join(self.dir.name, "out.csv")

    def tearDown(self):
        self.dir.cleanup()

    def export(self, rows):
        export_csv(rows, self.path)
        with open(self.path, "r", encoding="utf-8", newline="") as fh:
            return fh.read()

    def parse(self, text):
        return list(csv.reader(io.StringIO(text)))

    def row(self, customer):
        return Row(1, customer, "10.00", datetime(2024, 3, 5, 14, 0, tzinfo=timezone.utc))

    def test_writes_a_header(self):
        parsed = self.parse(self.export([self.row("Acme")]))
        self.assertEqual(parsed[0], ["id", "customer", "amount", "created_at"])

    def test_writes_one_line_per_row(self):
        parsed = self.parse(self.export([self.row("Acme"), self.row("Bees Ltd")]))
        self.assertEqual(len(parsed), 3)

    def test_a_value_with_a_comma_survives(self):
        parsed = self.parse(self.export([self.row("Acme, Inc")]))
        self.assertEqual(parsed[1][1], "Acme, Inc")

    def test_a_value_with_a_quote_survives(self):
        parsed = self.parse(self.export([self.row('"Bargain" Bins Ltd')]))
        self.assertEqual(parsed[1][1], '"Bargain" Bins Ltd')


if __name__ == "__main__":
    unittest.main()
