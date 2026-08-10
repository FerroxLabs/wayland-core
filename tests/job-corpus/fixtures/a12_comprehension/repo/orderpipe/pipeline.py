"""The pipeline itself: validate, price, commit, flush."""

from .cache import ResolutionCache
from .config import Settings
from .errors import ValidationError
from .middleware import AuditTrail, Counter
from .pricing import resolve_discount
from .ledger import Ledger
from .registry import build_registry


class OrderLine:
    __slots__ = ("sku", "quantity", "unit_price_cents")

    def __init__(self, sku, quantity, unit_price_cents):
        self.sku = sku
        self.quantity = quantity
        self.unit_price_cents = unit_price_cents


class Order:
    __slots__ = ("order_id", "customer_id", "lines", "discount_code")

    def __init__(self, order_id, customer_id, lines, discount_code=None):
        self.order_id = order_id
        self.customer_id = customer_id
        self.lines = lines
        self.discount_code = discount_code


class Pipeline:
    def __init__(self, settings=None):
        self.settings = settings or Settings()
        self.cache = ResolutionCache(self.settings.cache)
        self.ledger = Ledger(self.settings)
        self.registry = build_registry(self.settings)
        self.audit = AuditTrail(strict=self.settings.features.strict_audit)
        self.counter = Counter()

    def validate(self, order):
        if not order.order_id:
            raise ValidationError("order_id is required")
        if not order.lines:
            raise ValidationError("an order must have at least one line")
        self.audit.record("validate", order.order_id)

    def submit_order(self, order):
        """Take one order all the way to a ledger entry."""
        self.counter.bump("submitted")
        self.validate(order)

        resolved = resolve_discount(order, self.cache)
        self.audit.record("price", order.order_id, {"discount": resolved.code})

        entry = self.ledger.commit(order, self.cache)
        self.audit.record("commit", order.order_id, {"total": entry.total_cents})

        # Anything still pending becomes durable only now.
        self.cache.flush()
        self.audit.record("flush", order.order_id)
        self.counter.bump("committed")
        return entry
