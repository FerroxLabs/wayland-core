"""The discount engine we used before pricing.py.

Still imported by the registry so that the flag can turn it back on in an
emergency, but `FeatureFlags.legacy_discount` is off by default and nothing in
the shipped configuration turns it on. No request has gone through this module
since the 3.0 release.

It is left here on purpose. Deleting it was rejected in review because the
rollback plan for the 3.0 pricing change depends on it existing.
"""

from datetime import date

LEGACY_TABLE = {
    "SPRING24": 2000,
    "WELCOME": 1200,
    "VOLUME50": 800,
    "STAFF": 5000,
}


def legacy_resolve(order, today=None):
    """The old resolution. Percentages were higher and expiry was ignored."""
    today = today or date.today()
    code = (order.discount_code or "").strip().upper()
    return {"code": code or None, "basis_points": LEGACY_TABLE.get(code, 0)}


def register(registry):
    registry.register("discount", "legacy", legacy_resolve)
