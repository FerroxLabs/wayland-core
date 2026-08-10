import unittest

from receipts import parse_expenses


class ParseExpenses(unittest.TestCase):
    def test_adds_up_plain_lines(self):
        result = parse_expenses("Coffee, 3.50\nTrain to Leeds, 24.00")
        self.assertEqual(result.total, 27.5)
        self.assertEqual(len(result.lines), 2)

    def test_keeps_the_description(self):
        result = parse_expenses("Train to Leeds, 24.00")
        self.assertEqual(result.lines[0][0], "Train to Leeds")

    def test_handles_a_refund(self):
        result = parse_expenses("Coffee, 3.50\nRefund, -3.50")
        self.assertEqual(result.total, 0.0)

    def test_description_may_contain_a_comma(self):
        result = parse_expenses("Lunch, with the client, 41.20")
        self.assertEqual(result.lines[0][0], "Lunch, with the client")
        self.assertEqual(result.total, 41.2)


if __name__ == "__main__":
    unittest.main()
