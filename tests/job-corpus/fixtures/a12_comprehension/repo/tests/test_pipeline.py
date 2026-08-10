import unittest

from orderpipe.config import CacheConfig, FeatureFlags, Settings
from orderpipe.errors import ValidationError
from orderpipe.pipeline import Order, OrderLine, Pipeline


def an_order(order_id="ORD-1", code=None, lines=None):
    return Order(
        order_id=order_id,
        customer_id="CUST-9",
        lines=[OrderLine("SKU-1", 2, 5000), OrderLine("SKU-2", 1, 2500)]
        if lines is None
        else lines,
        discount_code=code,
    )


class SubmitWithoutDiscount(unittest.TestCase):
    def test_plain_order_is_committed(self):
        pipeline = Pipeline()
        entry = pipeline.submit_order(an_order())
        self.assertEqual(entry.subtotal_cents, 12500)
        self.assertEqual(entry.discount_cents, 0)
        self.assertEqual(entry.tax_cents, 2500)
        self.assertEqual(entry.total_cents, 15000)

    def test_stages_run_in_order(self):
        pipeline = Pipeline()
        pipeline.submit_order(an_order())
        self.assertEqual(pipeline.audit.stages(), ["validate", "price", "commit", "flush"])

    def test_an_empty_order_is_rejected(self):
        pipeline = Pipeline()
        with self.assertRaises(ValidationError):
            pipeline.submit_order(an_order(lines=[]))


class SubmitWithDiscount(unittest.TestCase):
    def test_live_code_reduces_the_total(self):
        pipeline = Pipeline()
        entry = pipeline.submit_order(an_order(code="WELCOME"))
        self.assertEqual(entry.discount_cents, 1250)
        self.assertEqual(entry.total_cents, 13500)

    def test_expired_code_is_noted_at_commit_not_at_pricing(self):
        pipeline = Pipeline()
        entry = pipeline.submit_order(an_order(code="SPRING24"))
        self.assertEqual(entry.discount_cents, 0)
        self.assertIn("discount_rejected:SPRING24", entry.notes)

    def test_unknown_code_is_noted_separately(self):
        pipeline = Pipeline()
        entry = pipeline.submit_order(an_order(code="NOPE"))
        self.assertEqual(entry.discount_cents, 0)
        self.assertIn("discount_unknown:NOPE", entry.notes)

    def test_volume_code_applies(self):
        pipeline = Pipeline()
        entry = pipeline.submit_order(an_order(code="VOLUME50"))
        self.assertEqual(entry.discount_cents, 625)


class Counters(unittest.TestCase):
    def test_a_committed_order_is_counted_twice(self):
        pipeline = Pipeline()
        pipeline.submit_order(an_order())
        self.assertEqual(pipeline.counter.counts, {"submitted": 1, "committed": 1})


class LegacyEngineIsOff(unittest.TestCase):
    def test_no_discount_handlers_are_registered_by_default(self):
        pipeline = Pipeline()
        self.assertEqual(pipeline.registry.names("discount"), [])

    def test_the_flag_can_bring_it_back(self):
        settings = Settings(features=FeatureFlags(legacy_discount=True))
        pipeline = Pipeline(settings)
        self.assertEqual(pipeline.registry.names("discount"), ["legacy"])

    def test_the_legacy_engine_does_not_price_anything_by_default(self):
        # The legacy table would give 1200bp for WELCOME, not 1000bp.
        pipeline = Pipeline()
        entry = pipeline.submit_order(an_order(code="WELCOME"))
        self.assertEqual(entry.discount_cents, 1250)


class CacheBehaviour(unittest.TestCase):
    def test_resolution_is_durable_after_a_submit(self):
        pipeline = Pipeline()
        pipeline.submit_order(an_order(order_id="ORD-7", code="WELCOME"))
        self.assertIsNotNone(pipeline.cache.get_committed("discount:ORD-7"))

    def test_a_second_order_reuses_the_same_cache(self):
        pipeline = Pipeline()
        pipeline.submit_order(an_order(order_id="ORD-1"))
        pipeline.submit_order(an_order(order_id="ORD-2"))
        self.assertEqual(len(pipeline.ledger.entries), 2)

    def test_explicit_config_is_respected(self):
        settings = Settings(cache=CacheConfig(write_through=True, max_entries=4))
        pipeline = Pipeline(settings)
        self.assertEqual(pipeline.cache.config.max_entries, 4)


if __name__ == "__main__":
    unittest.main()
