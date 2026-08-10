"""Currency conversion against the pre-2019 rate feed."""

from moneykit import LegacyRateShim, Money

SHIM = LegacyRateShim()


def convert(amount, rate):
    return SHIM.convert(Money(amount), rate).amount
