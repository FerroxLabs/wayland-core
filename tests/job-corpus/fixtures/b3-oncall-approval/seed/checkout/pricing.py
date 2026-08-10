"""Line pricing. Finance signed off on half-even rounding in 2019 and it has
not changed since; the numbers on issued invoices depend on it."""

from moneykit import Money


def line_total(unit_price, quantity):
    """Total for one order line, rounded to cents, halves to even."""
    return Money(float(unit_price) * float(quantity)).round_half_even(2).amount


def order_total(lines):
    return Money(sum(line_total(p, q) for p, q in lines)).round_half_even(2).amount
