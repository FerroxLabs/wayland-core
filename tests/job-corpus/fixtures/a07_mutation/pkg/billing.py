"""Order pricing in integer cents.

Every amount in this module is an integer number of cents. Floating point is
never used, because money is not a float.
"""

VOLUME_TIERS = ((100, 1500), (25, 1000), (10, 500))
"""(minimum quantity, discount in basis points), highest tier first."""


def _round_half_up(numerator, denominator):
    """Divide two integers, rounding halves away from zero."""
    if denominator <= 0:
        raise ValueError("denominator must be positive")
    if numerator >= 0:
        return (2 * numerator + denominator) // (2 * denominator)
    return -((-2 * numerator + denominator) // (2 * denominator))


def volume_discount_bp(qty):
    """Return the per-line volume discount for `qty` units, in basis points."""
    # Tier order matters here: the first tier whose minimum is met wins.
    for min_qty, bp in VOLUME_TIERS:
        if qty >= min_qty:
            return bp
    return 0


def _check_qty(qty):
    if isinstance(qty, bool) or not isinstance(qty, int):
        raise TypeError("qty must be an int")
    if qty <= 0:
        raise ValueError("qty must be a positive integer")


def line_total_cents(unit_price_cents, qty):
    """Price one order line after its volume discount."""
    _check_qty(qty)
    if unit_price_cents < 0:
        raise ValueError("unit_price_cents must not be negative")
    subtotal = unit_price_cents * qty
    discount = _round_half_up(subtotal * volume_discount_bp(qty), 10000)
    return subtotal - discount


class Invoice:
    """The priced result of an order."""

    __slots__ = ("gross_cents", "net_cents", "promo_cents", "tax_cents", "total_cents")

    def __init__(self, gross_cents, net_cents, promo_cents, tax_cents, total_cents):
        self.gross_cents = gross_cents
        self.net_cents = net_cents
        self.promo_cents = promo_cents
        self.tax_cents = tax_cents
        self.total_cents = total_cents

    def as_dict(self):
        return {name: getattr(self, name) for name in self.__slots__}

    def __eq__(self, other):
        if not isinstance(other, Invoice):
            return NotImplemented
        return self.as_dict() == other.as_dict()

    def __repr__(self):
        inner = ", ".join("%s=%r" % (k, v) for k, v in self.as_dict().items())
        return "Invoice(%s)" % inner


def price_order(lines, promo_bp=0, max_promo_cents=None, tax_bp=0, tax_exempt=False):
    """Price a whole order.

    `lines` is an iterable of (unit_price_cents, qty) pairs.
    """
    lines = list(lines)
    if not lines:
        raise ValueError("an order must have at least one line")
    gross = 0
    net = 0
    for unit_price_cents, qty in lines:
        _check_qty(qty)
        gross = gross + unit_price_cents * qty
        net = net + line_total_cents(unit_price_cents, qty)
    promo = _round_half_up(net * promo_bp, 10000)
    if max_promo_cents is not None:
        promo = min(promo, max_promo_cents)
    discounted = net - promo
    if discounted < 0:
        discounted = 0
    if tax_exempt:
        tax = 0
    else:
        tax = _round_half_up(discounted * tax_bp, 10000)
    return Invoice(gross, net, promo, tax, discounted + tax)


def prorate_cents(amount_cents, days_used, days_in_period):
    """Charge only for the days used, never rounding up against the customer."""
    if days_in_period <= 0:
        raise ValueError("days_in_period must be positive")
    if days_used < 0:
        raise ValueError("days_used must not be negative")
    days_used = min(days_used, days_in_period)
    return (amount_cents * days_used) // days_in_period
