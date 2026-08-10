"""INV-1 leak detector: encoding-aware scan of captured request bodies.

Pure stdlib, no product imports. Runs identically on Linux, macOS and Windows.

The detector answers exactly one question: does a planted canary token appear,
in any recoverable encoding, inside the bytes the binary under test actually
put on the wire?

Design notes that matter for non-vacuity:

*   The scan is over BODIES captured by a harness-owned server, never over the
    product's own telemetry.  The product's egress observer retains only
    SHA-256 digests by deliberate security design; nothing here changes that.
*   A token can leave the machine wearing a disguise.  We therefore search for
    a family of encodings per canary, not just the literal bytes.
*   Base64 is the awkward one: a token embedded inside a larger base64 blob is
    not byte-aligned with ``b64encode(token)``.  We emit three alignment
    variants (k = 0, 1, 2 padding bytes) and trim the alignment-dependent head
    and tail, which is the standard way to grep base64 for a substring.
*   A truncated leak is still a leak, so each canary also contributes a
    ``suffix`` variant (its 16-hex tail) flagged as PARTIAL.
*   Every variant carries a minimum length so short accidental matches cannot
    manufacture a false positive.
"""

from __future__ import annotations

import base64
import binascii
import gzip
import json
import re
import urllib.parse
import zlib
from dataclasses import dataclass, field
from typing import Iterable, Sequence

# A variant shorter than this is not trusted as evidence of a leak.
MIN_VARIANT_LEN = 12


@dataclass(frozen=True)
class Variant:
    """One searchable byte pattern derived from a canary token."""

    kind: str  # raw | lower | upper | json | percent | hex | b64k0/1/2 | suffix
    needle: bytes
    partial: bool = False  # True when a hit proves only a fragment escaped


@dataclass
class Hit:
    canary_id: str
    surface: str
    variant_kind: str
    partial: bool
    request_index: int
    request_path: str
    byte_offset: int
    context: str

    def to_json(self) -> dict:
        return {
            "canary_id": self.canary_id,
            "surface": self.surface,
            "variant_kind": self.variant_kind,
            "partial": self.partial,
            "request_index": self.request_index,
            "request_path": self.request_path,
            "byte_offset": self.byte_offset,
            "context": self.context,
        }


@dataclass
class ScanReport:
    bodies_scanned: int = 0
    bytes_scanned: int = 0
    decode_failures: list[str] = field(default_factory=list)
    hits: list[Hit] = field(default_factory=list)

    def hits_for(self, canary_id: str) -> list[Hit]:
        return [h for h in self.hits if h.canary_id == canary_id]

    def to_json(self) -> dict:
        return {
            "bodies_scanned": self.bodies_scanned,
            "bytes_scanned": self.bytes_scanned,
            "decode_failures": self.decode_failures,
            "hits": [h.to_json() for h in self.hits],
        }


# --------------------------------------------------------------------------
# Variant generation
# --------------------------------------------------------------------------


def _b64_alignment_variant(token: str, k: int) -> bytes | None:
    """Base64 pattern that survives arbitrary alignment inside a larger blob.

    Prefixing ``k`` filler bytes shifts the token into one of the three
    possible byte phases.  The first output quad of ``b64encode(filler+token)``
    depends on the filler, so it is dropped when ``k > 0``; the final quad may
    encode a partial group, so it is dropped unconditionally.
    """
    encoded = base64.b64encode(("A" * k + token).encode()).decode()
    head = 4 if k else 0
    core = encoded[head:]
    # Drop the trailing (possibly partial / padded) quad.
    core = core[: max(0, (len(core) // 4) * 4 - 4)]
    if len(core) < MIN_VARIANT_LEN:
        return None
    return core.encode()


def variants_for(token: str) -> list[Variant]:
    """Every encoding of ``token`` the detector will hunt for."""
    out: list[Variant] = []

    def add(kind: str, s: str | bytes, partial: bool = False) -> None:
        b = s.encode() if isinstance(s, str) else s
        if len(b) < MIN_VARIANT_LEN:
            return
        if any(v.needle == b for v in out):
            return
        out.append(Variant(kind, b, partial))

    add("raw", token)
    add("lower", token.lower())
    add("upper", token.upper())
    # json.dumps escaping is a no-op for this charset, but a token could be
    # re-serialised by an intermediary that escapes more aggressively.
    add("json", json.dumps(token)[1:-1])
    add("percent", urllib.parse.quote(token, safe=""))
    add("hex_lower", token.encode().hex())
    add("hex_upper", token.encode().hex().upper())
    for k in (0, 1, 2):
        v = _b64_alignment_variant(token, k)
        if v:
            add(f"b64k{k}", v)
    # Truncated / partial escape: the distinctive tail on its own.
    tail = token.rsplit("-", 1)[-1]
    if len(tail) >= MIN_VARIANT_LEN:
        add("suffix", tail, partial=True)

    return out


# --------------------------------------------------------------------------
# Body decoding
# --------------------------------------------------------------------------


def decode_body(raw: bytes, content_encoding: str | None) -> tuple[bytes, str | None]:
    """Return (decoded_bytes, failure_note).

    Unknown or broken encodings are NOT silently swallowed: the raw bytes are
    still scanned and the failure is recorded so a clean result can never rest
    on an undecoded body.
    """
    enc = (content_encoding or "").strip().lower()
    if not enc or enc == "identity":
        return raw, None
    try:
        if enc == "gzip":
            return gzip.decompress(raw), None
        if enc in ("deflate", "zlib"):
            try:
                return zlib.decompress(raw), None
            except zlib.error:
                return zlib.decompress(raw, -zlib.MAX_WBITS), None
        if enc == "br":
            try:
                import brotli  # type: ignore
            except ImportError:
                return raw, "brotli body not decoded (brotli module unavailable)"
            return brotli.decompress(raw), None
    except Exception as exc:  # noqa: BLE001 - any decode failure must surface
        return raw, f"{enc} decode failed: {exc!r}"
    return raw, f"unknown content-encoding {enc!r}; scanned raw"


_WS = re.compile(rb"\s+")


def _views(body: bytes) -> list[tuple[str, bytes]]:
    """Byte views of one body.  Each is scanned independently."""
    views = [("body", body)]
    stripped = _WS.sub(b"", body)
    if stripped != body:
        views.append(("body_nows", stripped))
    # A JSON body may carry a base64 blob (an attachment, an image part).  Pull
    # out long base64-looking runs and scan their decoded form too.
    for m in re.finditer(rb"[A-Za-z0-9+/]{40,}={0,2}", body):
        chunk = m.group(0)
        try:
            decoded = base64.b64decode(chunk + b"=" * (-len(chunk) % 4), validate=False)
        except (binascii.Error, ValueError):
            continue
        if decoded:
            views.append((f"b64blob@{m.start()}", decoded))
    return views


def _context(buf: bytes, at: int, needle_len: int, window: int = 48) -> str:
    lo = max(0, at - window)
    hi = min(len(buf), at + needle_len + window)
    return buf[lo:hi].decode("utf-8", errors="replace")


# --------------------------------------------------------------------------
# Scanning
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class CanaryProbe:
    """What the scanner needs to know about one planted canary."""

    canary_id: str
    surface: str
    token: str


def scan_bodies(
    bodies: Sequence[dict],
    probes: Iterable[CanaryProbe],
) -> ScanReport:
    """Scan captured request bodies for every canary.

    ``bodies`` is a sequence of capture records as written by ``recorder.py``:
    ``{"index": int, "path": str, "body": bytes, "content_encoding": str|None}``.
    """
    probes = list(probes)
    compiled = [(p, variants_for(p.token)) for p in probes]
    report = ScanReport()

    for rec in bodies:
        raw = rec["body"]
        decoded, note = decode_body(raw, rec.get("content_encoding"))
        if note:
            report.decode_failures.append(f"request {rec.get('index')}: {note}")
        report.bodies_scanned += 1
        report.bytes_scanned += len(decoded)

        for view_name, buf in _views(decoded):
            for probe, vs in compiled:
                for v in vs:
                    start = 0
                    while True:
                        at = buf.find(v.needle, start)
                        if at < 0:
                            break
                        report.hits.append(
                            Hit(
                                canary_id=probe.canary_id,
                                surface=probe.surface,
                                variant_kind=(
                                    v.kind if view_name == "body" else f"{v.kind}/{view_name}"
                                ),
                                partial=v.partial,
                                request_index=int(rec.get("index", -1)),
                                request_path=str(rec.get("path", "")),
                                byte_offset=at,
                                context=_context(buf, at, len(v.needle)),
                            )
                        )
                        start = at + 1
    return report
