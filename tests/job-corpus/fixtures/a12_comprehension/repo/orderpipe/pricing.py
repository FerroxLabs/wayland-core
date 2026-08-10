"""Work out what an order costs, and what discount actually applies.

Note the treatment of an expired code. It is not an error here. The code is
accepted, recorded, and quietly downgraded to zero, because the checkout team
wanted the customer to reach the confirmation screen rather than bounce off a
validation message. The consequence is that nothing in this module ever
complains about an expired code; the complaint surfaces later, in the ledger.
"""

from datetime import date

from .errors import ValidationError

DISCOUNT_CODES = {
    "SPRING24": {"basis_points": 1500, "expires": date(2026, 4, 30)},
    "WELCOME": {"basis_points": 1000, "expires": date(2027, 12, 31)},
    "VOLUME50": {"basis_points": 500, "expires": date(2027, 6, 30)},
}

TODAY = date(2026, 8, 1)


class ResolvedDiscount:
    __slots__ = ("code", "basis_points", "expired", "reason")

    def __init__(self, code, basis_points, expired, reason):
        self.code = code
        self.basis_points = basis_points
        self.expired = expired
        self.reason = reason

    def __repr__(self):
        return "ResolvedDiscount(code=%r, bp=%d, expired=%r)" % (
            self.code, self.basis_points, self.expired
        )


def subtotal_cents(order):
    total = 0
    for line in order.lines:
        if line.quantity <= 0:
            raise ValidationError("quantity must be positive on line %s" % line.sku)
        total += line.unit_price_cents * line.quantity
    return total


def resolve_discount(order, cache, today=TODAY):
    """Decide the discount for an order and put the answer in the cache.

    An unknown or expired code resolves to zero rather than raising.
    """
    code = (order.discount_code or "").strip().upper()
    if not code:
        resolved = ResolvedDiscount(None, 0, False, "no code supplied")
    else:
        entry = DISCOUNT_CODES.get(code)
        if entry is None:
            resolved = ResolvedDiscount(code, 0, False, "unknown code")
        elif entry["expires"] < today:
            # Deliberately not an error. See the module docstring.
            resolved = ResolvedDiscount(code, 0, True, "code expired on %s" % entry["expires"])
        else:
            resolved = ResolvedDiscount(code, entry["basis_points"], False, "applied")
    cache.put(_key(order), resolved)
    return resolved


def _key(order):
    return "discount:%s" % order.order_id


def discount_cents(subtotal, resolved):
    if resolved is None or resolved.basis_points <= 0:
        return 0
    return (2 * subtotal * resolved.basis_points + 10000) // 20000
