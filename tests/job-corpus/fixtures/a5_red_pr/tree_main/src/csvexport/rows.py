"""The row model the reporting store hands us."""

from dataclasses import dataclass
from datetime import datetime, timezone


@dataclass
class Row:
    """One order line.

    ``created_at`` is always timezone-aware. The reporting store hands most rows
    over in UTC, but rows replayed from the regional queues arrive with their
    own offset.
    """

    id: int
    customer: str
    amount: str
    created_at: datetime


def sample_rows():
    return [
        Row(1, "Acme", "10.00", datetime(2024, 3, 5, 14, 0, tzinfo=timezone.utc)),
        Row(2, "Bees Ltd", "22.50", datetime(2024, 3, 6, 9, 30, tzinfo=timezone.utc)),
    ]
