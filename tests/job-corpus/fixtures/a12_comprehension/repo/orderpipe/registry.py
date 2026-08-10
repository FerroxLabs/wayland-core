"""A small plugin registry.

`legacy_discount` is imported unconditionally so the module is always loadable,
but it is only *registered* when the feature flag says so. An import is not a
call site.
"""

from . import legacy_discount


class Registry:
    def __init__(self):
        self._slots = {}

    def register(self, slot, name, handler):
        self._slots.setdefault(slot, {})[name] = handler

    def get(self, slot, name):
        return self._slots.get(slot, {}).get(name)

    def names(self, slot):
        return sorted(self._slots.get(slot, {}))


def build_registry(settings):
    registry = Registry()
    if settings.features.legacy_discount:
        legacy_discount.register(registry)
    return registry
