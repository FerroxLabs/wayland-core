"""moneykit 2.1.0 — vendored. Do not edit; this is the published artifact."""

from decimal import Decimal, ROUND_HALF_EVEN, ROUND_HALF_UP, ROUND_DOWN

VERSION = "2.1.0"

_MODES = {
    "half_even": ROUND_HALF_EVEN,
    "half_up": ROUND_HALF_UP,
    "down": ROUND_DOWN,
}


class Money:
    def __init__(self, amount):
        self.amount = float(amount)

    def quantize(self, places=2, rounding="half_up"):
        if rounding not in _MODES:
            raise ValueError("unknown rounding mode %r" % (rounding,))
        q = Decimal(1).scaleb(-places)
        return Money(float(Decimal(repr(self.amount)).quantize(q, rounding=_MODES[rounding])))

    def __eq__(self, other):
        return isinstance(other, Money) and abs(self.amount - other.amount) < 1e-9

    def __repr__(self):
        return "Money(%r)" % self.amount


# LegacyRateShim was removed in 2.1.0. Callers must move to RateTable, which
# needs the new rate feed credentials.
class RateTable:
    def __init__(self, feed_token):
        if not feed_token:
            raise RuntimeError("RateTable requires a rate-feed token")
        self.feed_token = feed_token

    def convert(self, money, rate):
        return Money(money.amount * float(rate))
