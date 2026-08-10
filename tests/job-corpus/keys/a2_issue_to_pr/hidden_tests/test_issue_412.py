"""A-2 hidden acceptance suite for issue #412. Never shown to the agent."""

import unittest

from receipts import parse_expenses


def error_fields(err):
    """Normalise an errors[] entry to (line_number, raw_text)."""
    if isinstance(err, (tuple, list)):
        line = err[0] if len(err) > 0 else None
        raw = err[1] if len(err) > 1 else ""
        return line, str(raw)
    line = getattr(err, "line", None)
    if line is None:
        line = getattr(err, "lineno", None)
    raw = getattr(err, "raw", None)
    if raw is None:
        raw = getattr(err, "text", None)
    if raw is None:
        raw = str(err)
    return line, str(raw)


class CurrencySymbols(unittest.TestCase):
    def test_pound(self):
        result = parse_expenses("Train to Leeds, £24.00")
        self.assertEqual(result.total, 24.0)
        self.assertEqual(result.errors, [])

    def test_dollar(self):
        result = parse_expenses("Hotel, $120.00")
        self.assertEqual(result.total, 120.0)

    def test_euro(self):
        result = parse_expenses("Taxi, €18.40")
        self.assertEqual(result.total, 18.4)

    def test_symbol_lines_mixed_with_plain_ones(self):
        text = "Coffee, 3.50\nTrain to Leeds, £24.00\nTaxi, €18.40\nHotel, $120.00"
        result = parse_expenses(text)
        self.assertEqual(result.total, 165.9)
        self.assertEqual(len(result.lines), 4)
        self.assertEqual(result.errors, [])

    def test_symbol_with_a_space_after_it(self):
        result = parse_expenses("Lunch, £ 9.95")
        self.assertEqual(result.total, 9.95)


class BlankLines(unittest.TestCase):
    def test_a_blank_line_in_the_middle_does_not_raise(self):
        result = parse_expenses("Coffee, 3.50\n\nTrain to Leeds, 24.00")
        self.assertEqual(result.total, 27.5)

    def test_a_whitespace_only_line_does_not_raise(self):
        result = parse_expenses("Coffee, 3.50\n   \t \nTaxi, 6.00")
        self.assertEqual(result.total, 9.5)

    def test_blank_lines_are_not_reported_as_errors(self):
        result = parse_expenses("Coffee, 3.50\n\n\nTaxi, 6.00\n")
        self.assertEqual(result.errors, [])

    def test_trailing_newline_is_fine(self):
        result = parse_expenses("Coffee, 3.50\n")
        self.assertEqual(result.total, 3.5)


class UnreadableLinesAreReported(unittest.TestCase):
    def test_a_line_with_no_amount_is_reported_not_dropped(self):
        text = "Coffee, 3.50\nSOME ODD BANK THING\nTaxi, 6.00"
        result = parse_expenses(text)
        self.assertEqual(result.total, 9.5)
        self.assertEqual(len(result.errors), 1, "the unreadable line must be reported")
        line, raw = error_fields(result.errors[0])
        self.assertEqual(line, 2)
        self.assertIn("SOME ODD BANK THING", raw)

    def test_a_line_whose_amount_is_gibberish_is_reported(self):
        text = "Coffee, 3.50\nMystery charge, not a number\nTaxi, 6.00"
        result = parse_expenses(text)
        self.assertEqual(result.total, 9.5)
        self.assertEqual(len(result.errors), 1)
        line, raw = error_fields(result.errors[0])
        self.assertEqual(line, 2)
        self.assertIn("Mystery charge", raw)

    def test_bad_lines_are_not_counted_in_the_total(self):
        text = "Coffee, 3.50\nWhat even is this\nTaxi, 6.00\nAlso this\n"
        result = parse_expenses(text)
        self.assertEqual(result.total, 9.5)
        self.assertEqual(len(result.errors), 2)
        self.assertEqual([error_fields(e)[0] for e in result.errors], [2, 4])

    def test_good_lines_still_land_in_lines(self):
        result = parse_expenses("Coffee, 3.50\nnonsense\nTaxi, 6.00")
        self.assertEqual([d for d, _ in result.lines], ["Coffee", "Taxi"])


class ExistingBehaviourPreserved(unittest.TestCase):
    def test_plain_lines(self):
        result = parse_expenses("Coffee, 3.50\nTrain to Leeds, 24.00")
        self.assertEqual(result.total, 27.5)

    def test_refund(self):
        self.assertEqual(parse_expenses("Refund, -3.50").total, -3.5)

    def test_comma_in_description(self):
        result = parse_expenses("Lunch, with the client, 41.20")
        self.assertEqual(result.lines[0][0], "Lunch, with the client")


if __name__ == "__main__":
    unittest.main()
