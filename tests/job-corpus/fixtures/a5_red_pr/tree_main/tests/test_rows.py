import unittest
from datetime import timezone

from csvexport.rows import sample_rows


class Rows(unittest.TestCase):
    def test_sample_rows_are_timezone_aware(self):
        for row in sample_rows():
            self.assertIsNotNone(row.created_at.tzinfo)

    def test_sample_rows_are_utc(self):
        for row in sample_rows():
            self.assertEqual(row.created_at.utcoffset(), timezone.utc.utcoffset(None))


if __name__ == "__main__":
    unittest.main()
