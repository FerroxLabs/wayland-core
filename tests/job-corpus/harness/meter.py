"""The harness's own metering, and extraction of the product's claims.

Two independent sides that INV-5 reconciles against each other:

  * `Meter` and `HarnessLedger` — what the harness OBSERVED.  The meter is fed
    by the recording proxy that sits between the product and the provider (it
    appends `meter.jsonl`); the ledger is what the harness itself wrote into
    the workspace.  Neither comes from the product.
  * `Claims` — what the product TOLD THE USER: the money figure it printed,
    the files it said the user changed, and its declarations of success.

Nothing here imports a product internal.  Claims are read off the two surfaces
a user actually sees: the terminal text, and the JSON lines the product emits
for a host to render.
"""

from __future__ import annotations

import json
import os
import re
import time
from typing import Any, Dict, Iterable, List, Optional, Tuple


# ---------------------------------------------------------------------------
# Harness-side observation
# ---------------------------------------------------------------------------


class Meter:
    """Provider traffic the harness itself observed, one JSON object per line.

    Record shape (all optional except `kind`):
        {"kind": "provider_request", "ts": 1.0, "host": "api...",
         "model": "...", "input_tokens": 1, "output_tokens": 2,
         "cost_usd": 0.0031, "priced": true}

    `priced` is the honest flag: false means the harness saw real traffic it
    could not put a price on.  A product that turns that into "$0.00" is
    telling the user they spent nothing when nobody knows what they spent.
    """

    def __init__(self, path: Optional[str] = None) -> None:
        self.path = os.path.abspath(path) if path else None
        self.records: List[Dict[str, Any]] = []
        if self.path and os.path.exists(self.path):
            self.load()

    def load(self) -> "Meter":
        self.records = []
        with open(self.path, "r", encoding="utf-8", errors="replace") as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    obj = json.loads(line)
                except ValueError:
                    continue
                if isinstance(obj, dict):
                    self.records.append(obj)
        return self

    def append(self, **record: Any) -> None:
        record.setdefault("ts", time.time())
        record.setdefault("kind", "provider_request")
        self.records.append(record)
        if self.path:
            os.makedirs(os.path.dirname(self.path), exist_ok=True)
            with open(self.path, "a", encoding="utf-8") as fh:
                fh.write(json.dumps(record, sort_keys=True) + "\n")

    # -- aggregates ------------------------------------------------------
    @property
    def available(self) -> bool:
        return bool(self.records)

    @property
    def requests(self) -> List[Dict[str, Any]]:
        return [r for r in self.records if r.get("kind", "provider_request") == "provider_request"]

    @property
    def request_count(self) -> int:
        return len(self.requests)

    @property
    def priced_cost_usd(self) -> float:
        return sum(float(r.get("cost_usd") or 0.0) for r in self.requests if r.get("priced"))

    @property
    def unpriced_request_count(self) -> int:
        return sum(1 for r in self.requests if not r.get("priced"))

    def to_dict(self) -> Dict[str, Any]:
        return {
            "path": self.path,
            "available": self.available,
            "request_count": self.request_count,
            "unpriced_request_count": self.unpriced_request_count,
            "priced_cost_usd": round(self.priced_cost_usd, 6),
        }


class HarnessLedger:
    """Every workspace change the HARNESS made, so the product's account of
    'what the user changed' can be checked against what the user did."""

    def __init__(self) -> None:
        self.edits: List[Dict[str, Any]] = []

    def record_edit(self, relpath: str, reason: str) -> None:
        self.edits.append(
            {"path": relpath.replace(os.sep, "/"), "reason": reason, "ts": time.time()}
        )

    def record_edits(self, relpaths: Iterable[str], reason: str) -> None:
        for p in relpaths:
            self.record_edit(p, reason)

    @property
    def edited_paths(self) -> List[str]:
        seen: List[str] = []
        for e in self.edits:
            if e["path"] not in seen:
                seen.append(e["path"])
        return seen

    @property
    def count(self) -> int:
        return len(self.edited_paths)

    def to_dict(self) -> Dict[str, Any]:
        return {"count": self.count, "paths": self.edited_paths, "edits": self.edits}


# ---------------------------------------------------------------------------
# Product-side claims
# ---------------------------------------------------------------------------

_MONEY = r"\$\s*(\d+(?:\.\d+)?)"

_COST_TEXT_PATTERNS = (
    re.compile(r"(?:total\s+)?(?:cost|spend|spent|charged)[^\n$]{0,40}" + _MONEY, re.I),
    re.compile(_MONEY + r"\s*(?:total|spent|this session)", re.I),
)

_EDITED_COUNT_PATTERNS = (
    re.compile(r"User\s+edited\s+(\d+)\s+files?\b", re.I),
    re.compile(r"(\d+)\s+files?\s+edited\s+by\s+the\s+user", re.I),
)

_EDITED_NAMED_PATTERNS = (
    re.compile(r"User\s+edited\s+([^\s(][^\n]*?)\s+while\s+I\s+was\s+thinking", re.I),
)

_COMPLETION_PATTERNS = (
    re.compile(r"\ball\s+tests\s+(?:are\s+)?(?:now\s+)?pass(?:ing|ed|es)?\b", re.I),
    re.compile(r"\btests?\s+(?:are\s+)?(?:now\s+)?green\b", re.I),
    re.compile(r"\bsuite\s+is\s+green\b", re.I),
    re.compile(r"\bsuccessfully\s+(?:fixed|completed|implemented|migrated|resolved)\b", re.I),
    re.compile(r"\b(?:task|work|job)\s+(?:is\s+)?complete[d]?\b", re.I),
    re.compile(r"\beverything\s+(?:is\s+)?works?\b", re.I),
)


class Claims:
    """What the product told the user, parsed off its own visible output."""

    def __init__(self) -> None:
        self.cost_values: List[float] = []
        self.cost_sources: List[str] = []
        self.session_cost_events: List[Dict[str, Any]] = []
        self.edited_counts: List[int] = []
        self.edited_named: List[str] = []
        self.completion_hits: List[str] = []
        self.json_lines = 0

    # -- parsing ---------------------------------------------------------
    @classmethod
    def parse(cls, text: str) -> "Claims":
        c = cls()
        c._parse_json_lines(text)
        c._parse_text(text)
        return c

    def _parse_json_lines(self, text: str) -> None:
        for line in text.splitlines():
            line = line.strip()
            if not (line.startswith("{") and line.endswith("}")):
                continue
            try:
                obj = json.loads(line)
            except ValueError:
                continue
            if not isinstance(obj, dict):
                continue
            self.json_lines += 1
            self._walk(obj)

    def _walk(self, obj: Any, depth: int = 0) -> None:
        if depth > 8:
            return
        if isinstance(obj, list):
            for item in obj:
                self._walk(item, depth + 1)
            return
        if not isinstance(obj, dict):
            return
        if "total_cost_usd" in obj:
            try:
                total = float(obj["total_cost_usd"])
            except (TypeError, ValueError):
                total = None
            if total is not None:
                self.cost_values.append(total)
                self.cost_sources.append("json:total_cost_usd")
                per_turn = obj.get("per_turn")
                self.session_cost_events.append(
                    {
                        "total_cost_usd": total,
                        "per_turn": per_turn if isinstance(per_turn, list) else [],
                    }
                )
        for value in obj.values():
            self._walk(value, depth + 1)

    def _parse_text(self, text: str) -> None:
        for pat in _COST_TEXT_PATTERNS:
            for m in pat.finditer(text):
                try:
                    self.cost_values.append(float(m.group(1)))
                    self.cost_sources.append("text:" + m.group(0).strip()[:60])
                except ValueError:
                    continue
        for pat in _EDITED_COUNT_PATTERNS:
            for m in pat.finditer(text):
                self.edited_counts.append(int(m.group(1)))
        for pat in _EDITED_NAMED_PATTERNS:
            for m in pat.finditer(text):
                self.edited_named.append(m.group(1).strip())
        for pat in _COMPLETION_PATTERNS:
            for m in pat.finditer(text):
                self.completion_hits.append(m.group(0).strip())

    # -- derived ---------------------------------------------------------
    @property
    def any_cost_claim(self) -> bool:
        return bool(self.cost_values)

    @property
    def max_cost_claim(self) -> Optional[float]:
        return max(self.cost_values) if self.cost_values else None

    @property
    def claims_zero_cost(self) -> bool:
        return bool(self.cost_values) and max(self.cost_values) == 0.0

    def unpriced_turns(self) -> List[Dict[str, Any]]:
        """Turns the product itself flagged as not really priced."""
        out: List[Dict[str, Any]] = []
        for ev in self.session_cost_events:
            for turn in ev.get("per_turn") or []:
                if isinstance(turn, dict) and not turn.get("priced", False):
                    out.append(turn)
        return out

    @property
    def claimed_user_edit_count(self) -> int:
        n = 0
        if self.edited_counts:
            n = max(self.edited_counts)
        return max(n, len(self.edited_named))

    @property
    def claims_success(self) -> bool:
        return bool(self.completion_hits)

    def to_dict(self) -> Dict[str, Any]:
        return {
            "json_lines": self.json_lines,
            "cost_values": self.cost_values,
            "cost_sources": self.cost_sources[:20],
            "max_cost_claim": self.max_cost_claim,
            "unpriced_turns": self.unpriced_turns()[:20],
            "claimed_user_edit_count": self.claimed_user_edit_count,
            "edited_named": self.edited_named[:20],
            "completion_hits": sorted(set(self.completion_hits))[:20],
        }
