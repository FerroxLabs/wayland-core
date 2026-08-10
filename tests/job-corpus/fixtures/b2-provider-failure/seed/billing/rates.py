"""Contract loading. Existing code — the monthly run already depends on it."""

from __future__ import annotations

import csv
import os
from typing import Dict


def load_contracts(path: str) -> Dict[str, dict]:
    """customer_id -> {plan, rate_per_unit, minimum_usd}."""
    out: Dict[str, dict] = {}
    with open(path, "r", encoding="utf-8", newline="") as fh:
        for row in csv.DictReader(fh):
            out[row["customer_id"].strip()] = {
                "plan": row["plan"].strip(),
                "rate_per_unit": float(row["rate_per_unit"]),
                "minimum_usd": float(row["minimum_usd"]),
            }
    return out


def load_usage(path: str) -> Dict[str, float]:
    """customer_id -> total quantity for the month."""
    out: Dict[str, float] = {}
    with open(path, "r", encoding="utf-8", newline="") as fh:
        for row in csv.DictReader(fh):
            cid = row["customer_id"].strip()
            out[cid] = out.get(cid, 0.0) + float(row["quantity"])
    return out


def default_data_dir() -> str:
    return os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "data")
