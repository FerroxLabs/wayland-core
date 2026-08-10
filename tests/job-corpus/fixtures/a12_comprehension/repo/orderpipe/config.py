"""Runtime configuration."""


class CacheConfig:
    """How the resolution cache behaves.

    `write_through` decides *when* a resolved value becomes durable. With it on,
    a put lands in the durable layer immediately. With it off, puts are held in
    a pending buffer until `flush()` is called at the end of the pipeline.
    """

    def __init__(self, write_through=True, max_entries=512):
        self.write_through = write_through
        self.max_entries = max_entries


class FeatureFlags:
    """Feature switches. Everything here is off unless someone turns it on."""

    def __init__(self, legacy_discount=False, strict_audit=False):
        self.legacy_discount = legacy_discount
        self.strict_audit = strict_audit


class Settings:
    def __init__(self, cache=None, features=None, tax_bp=2000):
        self.cache = cache or CacheConfig()
        self.features = features or FeatureFlags()
        self.tax_bp = tax_bp
