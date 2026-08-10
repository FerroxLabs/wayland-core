"""Commit a priced order to the ledger.

The important thing here is the second validation. Pricing already resolved the
discount, but commit does not trust an in-flight value: it reads the resolution
back out of the *durable* layer with `get_committed`, because a ledger entry
outlives the request that produced it and must not be built on something that
was never made durable.

That is also where an expired code finally gets noticed. Pricing downgraded it
silently; commit is the first place that says so out loud, by attaching a note
to the entry.
"""

from .errors import DiscountNotResolved
from .pricing import ResolvedDiscount, _key, discount_cents, subtotal_cents


class LedgerEntry:
    __slots__ = ("order_id", "subtotal_cents", "discount_cents", "tax_cents",
                 "total_cents", "notes")

    def __init__(self, order_id, subtotal, discount, tax, total, notes):
        self.order_id = order_id
        self.subtotal_cents = subtotal
        self.discount_cents = discount
        self.tax_cents = tax
        self.total_cents = total
        self.notes = notes

    def as_dict(self):
        return {name: getattr(self, name) for name in self.__slots__}


class Ledger:
    def __init__(self, settings):
        self.settings = settings
        self.entries = []

    def commit(self, order, cache):
        if not (order.discount_code or "").strip():
            # Nothing was claimed, so there is nothing that needed to be made
            # durable. This is the only path that does not consult the cache.
            resolved = ResolvedDiscount(None, 0, False, "no code supplied")
        else:
            resolved = cache.get_committed(_key(order))
            if resolved is None:
                raise DiscountNotResolved(
                    "no durable discount resolution for order %s" % order.order_id
                )

        notes = []
        if resolved.expired:
            notes.append("discount_rejected:%s" % resolved.code)
        elif resolved.code and resolved.basis_points == 0:
            notes.append("discount_unknown:%s" % resolved.code)

        subtotal = subtotal_cents(order)
        discount = discount_cents(subtotal, resolved)
        taxable = subtotal - discount
        tax = (2 * taxable * self.settings.tax_bp + 10000) // 20000
        entry = LedgerEntry(order.order_id, subtotal, discount, tax, taxable + tax, notes)
        self.entries.append(entry)
        return entry
