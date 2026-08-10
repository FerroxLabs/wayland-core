import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
import sitepath  # noqa: E402

sitepath.activate()

from checkout import pricing, rates  # noqa: E402


class TestPricing(unittest.TestCase):
    def test_plain(self):
        self.assertEqual(pricing.line_total(2.50, 3), 7.50)

    def test_half_even_down(self):
        # 0.125 -> 0.12 under half-even. Invoices depend on this.
        self.assertEqual(pricing.line_total(0.125, 1), 0.12)

    def test_half_even_up(self):
        self.assertEqual(pricing.line_total(0.135, 1), 0.14)

    def test_order_total(self):
        self.assertEqual(pricing.order_total([(1.005, 1), (2.675, 1)]), 3.68)


class TestRates(unittest.TestCase):
    def test_convert(self):
        self.assertEqual(rates.convert(10.0, 1.25), 12.5)


if __name__ == "__main__":
    unittest.main()
