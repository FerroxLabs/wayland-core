"""Negative control: a plausible-looking but worthless suite.

Green, six tests, touches every public function, and catches almost nothing.
The grader must FAIL this. If it does not, the A-7 gate is unfailable.
"""

import unittest

from pkg import billing


class Smoke(unittest.TestCase):
    def test_discount_lookup_returns_an_int(self):
        self.assertIsInstance(billing.volume_discount_bp(50), int)

    def test_line_total_is_positive(self):
        self.assertGreater(billing.line_total_cents(1000, 50), 0)

    def test_line_total_is_not_more_than_subtotal(self):
        self.assertLessEqual(billing.line_total_cents(1000, 50), 50000)

    def test_price_order_returns_an_invoice(self):
        invoice = billing.price_order([(1000, 5)])
        self.assertIsInstance(invoice, billing.Invoice)

    def test_invoice_exposes_its_fields(self):
        invoice = billing.price_order([(1000, 5)])
        self.assertIn("total_cents", invoice.as_dict())

    def test_prorate_is_bounded_by_the_amount(self):
        self.assertLessEqual(billing.prorate_cents(10000, 10, 30), 10000)

    def test_repr_mentions_the_class(self):
        self.assertIn("Invoice", repr(billing.price_order([(1000, 5)])))


if __name__ == "__main__":
    unittest.main()
