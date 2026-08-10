import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from analyzer.parse import ParseError, parse, parse_line  # noqa: E402
from analyzer import report  # noqa: E402

GOOD = [
    '10.0.0.1 - alice [10/Aug/2026:03:00:01 +0000] "GET /api/orders HTTP/1.1" 200 1024',
    '10.0.0.2 - - [10/Aug/2026:03:00:02 +0000] "POST /api/orders HTTP/1.1" 201 512',
    '10.0.0.3 - bob [10/Aug/2026:03:00:03 +0000] "GET /health HTTP/1.1" 200 -',
    '10.0.0.4 - - [10/Aug/2026:03:00:04 +0000] "GET /api/orders HTTP/1.1" 503 87',
    '10.0.0.5 - - [10/Aug/2026:03:00:05 +0000] "DELETE /api/orders/9 HTTP/1.1" 404 33',
]
BAD = ["not a log line", "", '10.0.0.9 - - [x] "GET" 200 1']


class TestParse(unittest.TestCase):
    def test_fields(self):
        r = parse_line(GOOD[0])
        self.assertEqual(r["ip"], "10.0.0.1")
        self.assertEqual(r["user"], "alice")
        self.assertEqual(r["method"], "GET")
        self.assertEqual(r["path"], "/api/orders")
        self.assertEqual(r["status"], 200)
        self.assertEqual(r["bytes"], 1024)

    def test_dash_bytes_is_zero(self):
        self.assertEqual(parse_line(GOOD[2])["bytes"], 0)

    def test_anonymous_user(self):
        self.assertEqual(parse_line(GOOD[1])["user"], "-")

    def test_rejects_garbage(self):
        for line in ("not a log line", '10.0.0.9 - - [x] "GET" 200 1'):
            with self.assertRaises(ParseError):
                parse_line(line)

    def test_status_is_int(self):
        self.assertIsInstance(parse_line(GOOD[3])["status"], int)

    def test_bulk_counts_bad(self):
        records, bad = parse(GOOD + BAD)
        self.assertEqual(len(records), 5)
        self.assertEqual(bad, 2)

    def test_blank_lines_ignored(self):
        records, bad = parse(["", "   ", GOOD[0]])
        self.assertEqual(len(records), 1)
        self.assertEqual(bad, 0)

    def test_three_digit_status_only(self):
        with self.assertRaises(ParseError):
            parse_line(GOOD[0].replace(" 200 ", " 20 "))


class TestReport(unittest.TestCase):
    def setUp(self):
        self.records, _ = parse(GOOD)

    def test_status_classes(self):
        self.assertEqual(report.status_classes(self.records),
                         {"2xx": 3, "5xx": 1, "4xx": 1})

    def test_top_paths(self):
        self.assertEqual(report.top_paths(self.records, 2),
                         ["/api/orders", "/api/orders/9"])

    def test_top_paths_is_stable(self):
        self.assertEqual(report.top_paths(self.records, 1), ["/api/orders"])

    def test_bytes_by_method(self):
        self.assertEqual(report.bytes_by_method(self.records),
                         {"GET": 1111, "POST": 512, "DELETE": 33})

    def test_error_rate(self):
        self.assertEqual(report.error_rate(self.records), 0.2)

    def test_error_rate_empty(self):
        self.assertEqual(report.error_rate([]), 0.0)

    def test_error_rate_ignores_4xx(self):
        records, _ = parse([GOOD[4]])
        self.assertEqual(report.error_rate(records), 0.0)


if __name__ == "__main__":
    unittest.main()
