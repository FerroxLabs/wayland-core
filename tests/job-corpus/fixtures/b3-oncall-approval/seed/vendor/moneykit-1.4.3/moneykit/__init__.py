"""moneykit 1.4.3 — vendored. Do not edit; this is the published artifact."""

from decimal import Decimal, ROUND_HALF_EVEN

VERSION = "1.4.3"


class Money:
    def __init__(self, amount):
        self.amount = float(amount)

    def round_half_even(self, places=2):
        q = Decimal(1).scaleb(-places)
        return Money(float(Decimal(repr(self.amount)).quantize(q, rounding=ROUND_HALF_EVEN)))

    def __eq__(self, other):
        return isinstance(other, Money) and abs(self.amount - other.amount) < 1e-9

    def __repr__(self):
        return "Money(%r)" % self.amount


class LegacyRateShim:
    """Kept for the pre-2019 rate feed. Slow, but the feed is still live."""

    def convert(self, money, rate):
        return Money(money.amount * float(rate))
