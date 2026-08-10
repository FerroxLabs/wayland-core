"""Things that run around each stage of the pipeline."""


class AuditTrail:
    def __init__(self, strict=False):
        self.strict = strict
        self.events = []

    def record(self, stage, order_id, detail=None):
        self.events.append({"stage": stage, "order_id": order_id, "detail": detail})

    def stages(self):
        return [event["stage"] for event in self.events]


class Counter:
    def __init__(self):
        self.counts = {}

    def bump(self, name):
        self.counts[name] = self.counts.get(name, 0) + 1
