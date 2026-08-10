"""Reference suite: a competent answer to A-7.

Not given to the agent. Its only job is to prove the gate is winnable -- that
every seeded defect is catchable by tests written from SPEC.md alone, and that
none of the behaviour-preserving rewrites break a reasonable suite.
"""

import unittest

from pkg import billing


class VolumeTiers(unittest.TestCase):
    def test_boundaries_are_inclusive(self):
        self.assertEqual(billing.volume_discount_bp(9), 0)
        self.assertEqual(billing.volume_discount_bp(10), 500)
        self.assertEqual(billing.volume_discount_bp(24), 500)
        self.assertEqual(billing.volume_discount_bp(25), 1000)
        self.assertEqual(billing.volume_discount_bp(99), 1000)
        self.assertEqual(billing.volume_discount_bp(100), 1500)

    def test_highest_matching_tier_wins(self):
        self.assertEqual(billing.volume_discount_bp(150), 1500)
        self.assertEqual(billing.volume_discount_bp(1000), 1500)


class Rounding(unittest.TestCase):
    def test_exact_half_rounds_away_from_zero(self):
        # 10 units at 1 cent: 5% of 10 cents is exactly 0.5 -> 1.
        self.assertEqual(billing.line_total_cents(1, 10), 9)

    def test_below_half_rounds_down(self):
        # 12 units at 1 cent: 5% of 12 is 0.6 -> 1. Use a sub-half case too.
        self.assertEqual(billing._round_half_up(4, 10), 0)
        self.assertEqual(billing._round_half_up(5, 10), 1)
        self.assertEqual(billing._round_half_up(15, 10), 2)

    def test_negative_halves_round_away_from_zero(self):
        self.assertEqual(billing._round_half_up(-5, 10), -1)


class Quantities(unittest.TestCase):
    def test_zero_is_rejected(self):
        with self.assertRaises(ValueError):
            billing.line_total_cents(100, 0)

    def test_negative_is_rejected(self):
        with self.assertRaises(ValueError):
            billing.line_total_cents(100, -3)

    def test_non_int_is_rejected(self):
        with self.assertRaises(TypeError):
            billing.line_total_cents(100, 2.0)
        with self.assertRaises(TypeError):
            billing.line_total_cents(100, True)


class OrderPricing(unittest.TestCase):
    def test_promo_is_capped(self):
        # net 90000; a 2000bp promo would be 18000, capped at 5000.
        invoice = billing.price_order([(1000, 100)], promo_bp=2000, max_promo_cents=5000)
        self.assertEqual(invoice.net_cents, 85000)
        self.assertEqual(invoice.promo_cents, 5000)

    def test_uncapped_promo_is_the_full_amount(self):
        invoice = billing.price_order([(1000, 100)], promo_bp=1000)
        self.assertEqual(invoice.promo_cents, 8500)

    def test_tax_is_charged_on_the_discounted_amount(self):
        invoice = billing.price_order([(1000, 100)], promo_bp=1000, tax_bp=2000)
        # net 85000, promo 8500, discounted 76500, tax 20% = 15300.
        self.assertEqual(invoice.tax_cents, 15300)
        self.assertEqual(invoice.total_cents, 91800)

    def test_exempt_customer_pays_no_tax_but_keeps_discounts(self):
        invoice = billing.price_order([(1000, 100)], promo_bp=1000, tax_bp=2000, tax_exempt=True)
        self.assertEqual(invoice.tax_cents, 0)
        self.assertEqual(invoice.promo_cents, 8500)
        self.assertEqual(invoice.total_cents, 76500)

    def test_total_never_goes_negative(self):
        invoice = billing.price_order([(100, 1)], promo_bp=20000, tax_bp=2000)
        self.assertEqual(invoice.total_cents, 0)
        self.assertEqual(invoice.tax_cents, 0)

    def test_gross_is_before_volume_discount(self):
        invoice = billing.price_order([(1000, 100)])
        self.assertEqual(invoice.gross_cents, 100000)
        self.assertEqual(invoice.net_cents, 85000)

    def test_empty_order_is_rejected(self):
        with self.assertRaises(ValueError):
            billing.price_order([])


class Proration(unittest.TestCase):
    def test_partial_period_rounds_down(self):
        # 10000 cents, 10 of 30 days -> 3333.33 -> 3333.
        self.assertEqual(billing.prorate_cents(10000, 10, 30), 3333)

    def test_full_period_is_the_whole_amount(self):
        self.assertEqual(billing.prorate_cents(10000, 30, 30), 10000)

    def test_days_used_is_clamped(self):
        self.assertEqual(billing.prorate_cents(10000, 45, 30), 10000)

    def test_invalid_period_is_rejected(self):
        with self.assertRaises(ValueError):
            billing.prorate_cents(10000, 1, 0)
        with self.assertRaises(ValueError):
            billing.prorate_cents(10000, -1, 30)


if __name__ == "__main__":
    unittest.main()
