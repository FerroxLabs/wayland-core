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


#: Where the operator pins per-model prices.  Deliberately a FILE, not a table
#: baked into this module: a price nobody can point at the source of is a made-up
#: number, and a made-up number in the one place the corpus checks money would be
#: worse than no check at all.  A model absent from the file is UNPRICED, which is
#: a state the harness can act on — it is exactly the condition under which a
#: product showing "$0.00" is making a claim nobody can support.
DEFAULT_PRICE_FILE = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "keys", "model_prices.json"
)


class PriceBook:
    """Per-model prices, in USD per million tokens, loaded from disk."""

    def __init__(self, path: Optional[str] = None) -> None:
        self.path = os.path.abspath(path or DEFAULT_PRICE_FILE)
        self.models: Dict[str, Dict[str, Any]] = {}
        self.loaded = False
        self.error: Optional[str] = None
        self.load()

    def load(self) -> "PriceBook":
        if not os.path.isfile(self.path):
            self.error = "no price file at %s" % self.path
            return self
        try:
            with open(self.path, "r", encoding="utf-8") as fh:
                data = json.load(fh)
        except (OSError, ValueError) as exc:
            self.error = "price file unreadable: %r" % (exc,)
            return self
        models = data.get("models") if isinstance(data, dict) else None
        if isinstance(models, dict):
            self.models = {str(k): v for k, v in models.items() if isinstance(v, dict)}
            self.loaded = True
        else:
            self.error = "price file has no 'models' object"
        return self

    def price(self, model: Optional[str], input_tokens, output_tokens):
        """(cost_usd, priced, why).

        `priced` is False whenever ANY input to the arithmetic is missing: an
        unknown model, or a request whose token counts the provider never
        reported.  Half a price is not a price.
        """
        if not model:
            return 0.0, False, "the request named no model on the wire"
        entry = self.models.get(model)
        if entry is None:
            return 0.0, False, "no pinned price for model %r in %s" % (model, self.path)
        if input_tokens is None and output_tokens is None:
            return 0.0, False, "the provider reported no token usage for this request"
        try:
            per_in = float(entry.get("input_usd_per_mtok", 0.0))
            per_out = float(entry.get("output_usd_per_mtok", 0.0))
        except (TypeError, ValueError):
            return 0.0, False, "the pinned price for %r is not a number" % model
        cost = (int(input_tokens or 0) * per_in + int(output_tokens or 0) * per_out) / 1_000_000.0
        return cost, True, "priced from %s" % self.path


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

    @property
    def models(self) -> List[str]:
        seen: List[str] = []
        for r in self.requests:
            m = r.get("model")
            if isinstance(m, str) and m and m not in seen:
                seen.append(m)
        return seen

    @property
    def input_tokens(self) -> int:
        return sum(int(r.get("input_tokens") or 0) for r in self.requests)

    @property
    def output_tokens(self) -> int:
        return sum(int(r.get("output_tokens") or 0) for r in self.requests)

    def record_traffic(self, traffic: Iterable[Dict[str, Any]], prices: "PriceBook") -> int:
        """Append what a recording proxy saw, priced by the pinned price book.

        This is the meter's only real input.  Before it existed the meter was
        fed by the selftest and by nothing else, so INV-5.cost could only fail
        when the product volunteered `priced: false` about itself — a check
        that can only fire on self-incrimination is not a check.
        """
        n = 0
        for rec in traffic:
            if rec.get("kind") != "provider_request":
                self.append(**{k: v for k, v in rec.items()})
                continue
            cost, priced, why = prices.price(
                rec.get("model"), rec.get("input_tokens"), rec.get("output_tokens")
            )
            self.append(cost_usd=round(cost, 8), priced=priced, pricing_note=why, **rec)
            n += 1
        return n

    def to_dict(self) -> Dict[str, Any]:
        return {
            "path": self.path,
            "available": self.available,
            "request_count": self.request_count,
            "unpriced_request_count": self.unpriced_request_count,
            "priced_cost_usd": round(self.priced_cost_usd, 6),
            "models": self.models,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
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
    # The way people actually sign off.  "Done — the parser now handles
    # currency symbols" matched none of the six above, so a product could
    # declare victory and INV-5.completion would read nothing at all.
    re.compile(r"^\s*(?:done|fixed|all\s+set|that'?s\s+it)\b\s*[—:\-.!]", re.I | re.M),
    re.compile(
        r"\bnow\s+(?:correctly\s+|properly\s+)?"
        r"(?:handles|works|passes|supports|parses|returns|accepts)\b",
        re.I,
    ),
    re.compile(r"\bthe\s+(?:bug|issue|failure|regression)\s+is\s+(?:now\s+)?fixed\b", re.I),
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
        #: Model identities the product named to the user, and how many turns
        #: it accounted for.  Reconciled against the wire by INV-5.traffic.
        self.claimed_models: List[str] = []
        self.claimed_turns: Optional[int] = None

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
                per_turn = per_turn if isinstance(per_turn, list) else []
                self.session_cost_events.append(
                    {"total_cost_usd": total, "per_turn": per_turn}
                )
                if per_turn:
                    self.claimed_turns = max(self.claimed_turns or 0, len(per_turn))
                for turn in per_turn:
                    if isinstance(turn, dict):
                        self._note_model(turn.get("model"))
        self._note_model(obj.get("model"))
        for value in obj.values():
            self._walk(value, depth + 1)

    def _note_model(self, value: Any) -> None:
        if isinstance(value, str) and value and value not in self.claimed_models:
            self.claimed_models.append(value)

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
            "claimed_models": self.claimed_models[:20],
            "claimed_turns": self.claimed_turns,
        }
