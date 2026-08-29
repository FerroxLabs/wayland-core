#!/usr/bin/env python3
"""Release-time staleness gate for `model_output_ceiling`'s catalogue table.

WHY THIS EXISTS
---------------
`crates/wcore-config/src/limits.rs` is a hand-maintained table of per-model
output/context ceilings. It is how #165 happened: a frontier model shipped in
the routing catalog, nobody added its limits here, and the miss produced no
error -- just a silently wrong, too-small window, and a customer run that died
at 178,336 tokens against a fake ~177k ceiling. `every_routed_catalog_model_has
_a_known_window` closed the half of that loop we control (our own catalog). This
closes the other half: the WORLD moving on without us.

A rule in a doc is not a rule. This repo has already watched a warning comment
fail to stop the next editor repeating the exact bug it described, so the
"refresh the table each release" rule is mechanised here instead of written down.

WHERE IT RUNS -- AND WHERE IT DELIBERATELY DOES NOT
---------------------------------------------------
It needs the network, and a third-party catalogue in the main test path buys
flakiness for no benefit. So the LIVE scan runs once per release, in
`release.yml`'s `prepare-release` job (which every publishing job depends on, so
a FAIL genuinely stops the release). `--self-test` needs no network and runs on
every CI run via `just check-all`, so the CHECKER itself cannot rot unnoticed
between releases.

THREE OUTCOMES, AND THEY ARE NOT THE SAME THING
-----------------------------------------------
  FAIL (exit 1)  -- we are wrong about something we CLAIMED. An in-scope
                    first-party model id has no arm, or our figure OVER-claims
                    against the first-party consensus.
  REPORT (exit 0)-- a whole new family appeared at a vendor we track. We cannot
                    know whether it matters and failing a release on someone
                    else's launch is not this script's call, but the release
                    owner must SEE it.
  SKIP (exit 0)  -- models.dev is unreachable. Announced in a banner that cannot
                    be mistaken for a pass. A skip that reads as a pass is the
                    exact defect class this gate exists to catch.

WHAT IT CHECKS, AND THE GAP THAT USED TO BE HERE
------------------------------------------------
Two tables:

  1. The ordered `CATALOGUE_CEILINGS` table (GLM / Qwen / Kimi / Mistral /
     Llama), graded fragment-by-fragment against first-party rows.
  2. `PASSTHROUGH_VENDOR_MODELS` (`crates/wcore-config/src/limits/passthrough.rs`)
     -- the provider-native ids in the older `if`-chain families (Claude,
     GPT-4.x/5.x, Grok, Gemini, DeepSeek, MiniMax) that reach users through
     `--model` passthrough.

(2) is #1176. Those `if` chains carry conditional logic -- nested `!contains`
guards, exact `==` matches -- that a text parser cannot evaluate, and a parser
that silently MIS-evaluated them would be worse than no coverage. So the chain
is not parsed. It is EXPOSED: `PASSTHROUGH_VENDOR_MODELS` records what each
passthrough id must resolve to, the Rust test
`every_passthrough_vendor_model_resolves_its_arm` proves on every PR that the
real `model_output_ceiling` returns exactly those figures, and this script
grades the same rows against the live catalogue. Neither end can rot alone: the
Rust test catches a deleted or wrong arm, this script catches the world moving
on. Together they close the class that cost `claude-opus-5`,
`gpt-4o-2024-05-13` and `gemini-flash-latest` last cycle -- all three found by
hand, by neither guard.

Still NOT checked, stated rather than hidden: ids OUTSIDE the in-scope patterns
(older generations this table deliberately does not arm -- Claude 3.x, GPT-3.5,
DeepSeek V3/R1, the Gemini specialty modalities), and any endpoint that is not
vendor-operated. Both are printed on every run.

Only ids OVER-claimed relative to first-party are failures. Under-claiming is
this table's deliberate policy (see the `CATALOGUE_CEILINGS` doc comment): too
HIGH kills a run mid-flight, too LOW only compacts early.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import tempfile
import urllib.error
import urllib.request

CATALOGUE_URL = "https://models.dev/api.json"
# The table lives in its own module since limits.rs crossed 1000 lines.
# parse_table() fails CLOSED if this path stops holding the const, so a future
# move breaks the release loudly rather than silently scanning nothing.
DEFAULT_LIMITS = "crates/wcore-config/src/limits/catalogue.rs"
# #1176: the passthrough coverage contract for the `if`-chain families. Same
# fail-closed parse as the catalogue table.
DEFAULT_PASSTHROUGH = "crates/wcore-config/src/limits/passthrough.rs"

# Vendor-operated providers, per family. "First-party" means the vendor runs the
# endpoint (or is a first-party tenant of it) -- these are the only rows allowed
# to FAIL the release. Reseller and aggregator rows are noise: the same id is
# served with wildly different windows depending on who is hosting the weights.
FIRST_PARTY = {
    "glm": ["zhipuai", "zai", "zhipuai-coding-plan", "zai-coding-plan"],
    "qwen": ["alibaba", "alibaba-cn", "alibaba-coding-plan", "alibaba-token-plan"],
    "kimi": ["moonshotai", "moonshotai-cn"],
    "mistral": ["mistral"],
    "llama": ["llama"],
}

# Which ids inside each family this table CLAIMS to cover. Everything else in
# the family is a documented, deliberate exclusion (GLM 4.5/4.6, the Kimi K2.0
# line, the host-variable Qwen dense tier, Mistral audio/embedding) and must not
# redden the release -- a gate that fires on our own recorded decisions gets
# switched off within a week, and then it protects nothing.
IN_SCOPE = {
    "glm": re.compile(r"^glm-(4\.7|5(\.\d+)?)(-|$|:|@)"),
    "qwen": re.compile(r"^qwen3\.\d+-(max|plus|flash)(-|$|:)"),
    "kimi": re.compile(r"^kimi-(k3|k2\.[5-9])(-|$|:|@)"),
    "mistral": re.compile(r"^(mistral-(large|medium|small)|magistral|codestral|devstral)"),
    "llama": re.compile(r"^(llama-4-(maverick|scout)|llama-3\.3-)"),
}

# --------------------------------------------------------------------------
# #1176: provider-native passthrough coverage for the `if`-chain families.
# --------------------------------------------------------------------------

# VENDOR-OPERATED endpoints per family, per AGENTS.md. The vendor's own API,
# plus the clouds that serve the vendor's models under a first-party
# arrangement and republish the vendor's own spec (Bedrock/Vertex for Claude,
# Vertex for Gemini, Azure for OpenAI, the Alibaba tenants for DeepSeek).
# Aggregators and resellers are NOT here: they publish `ctx=0`, `out=1010000`
# and dropped digits, and the same id at wildly different limits.
#
# The floor is the MINIMUM across these rows -- AGENTS.md's "when sources
# disagree, take the lower value". Where a cloud republishes a STALE figure and
# the vendor's own endpoint disagrees, that is recorded as a pin below, not
# silently absorbed.
PASSTHROUGH_VENDORS = {
    "claude": ["anthropic", "google-vertex", "amazon-bedrock"],
    "gpt": ["openai", "azure"],
    "grok": ["xai", "amazon-bedrock"],
    "gemini": ["google", "google-vertex"],
    "deepseek": ["deepseek", "alibaba", "alibaba-cn", "alibaba-token-plan"],
    "minimax": ["minimax", "minimax-cn", "minimax-coding-plan"],
}

# Which ids inside each family the `if` chain CLAIMS to size. Everything else
# is a documented, deliberate exclusion (Claude 3.x, GPT-3.5 and the o-series,
# DeepSeek V3/R1, Grok 2) and must not redden the release -- a gate that fires
# on our own recorded decisions gets switched off within a week.
#
# Each pattern is a FLOOR ("this generation and everything above it"), never an
# enumeration of the generations that exist today. An enumeration is blind to a
# NEW generation inside a known family -- `claude-opus-6`, `gpt-6`,
# `deepseek-v5` -- and that blindness IS #165: the REPORT arm below cannot see
# it either, because the family key is already in KNOWN_FAMILIES. The floor is
# set just above the last generation these tables deliberately leave unarmed
# (Claude 3.x, GPT-4/4.5 and the o-series, Grok 2, Gemini 2.0, DeepSeek V3/R1,
# MiniMax M1), so raising the floor is a decision someone has to write down.
PASSTHROUGH_IN_SCOPE = {
    "claude": re.compile(r"^claude-(opus|sonnet|haiku|fable)-(?:[4-9]|\d\d)"),
    "gpt": re.compile(r"^gpt-(?:4o|4\.1|[5-9]|\d\d)"),
    "grok": re.compile(r"^grok-(?:[3-9]|\d\d)"),
    "gemini": re.compile(
        r"^gemini-(?:(?:2\.5|[3-9](?:\.\d+)?|\d\d(?:\.\d+)?)-(?:pro|flash)"
        r"|flash-(?:lite-)?latest)"
    ),
    "deepseek": re.compile(r"^deepseek-v(?:[4-9]|\d\d)"),
    "minimax": re.compile(r"^minimax-m(?:[2-9]|\d\d)"),
}

# AGENTS.md's THIRD preserved rule: "Do not add a static arm for an
# open-weights model served at wildly different limits by different hosts."
#
# Two of the six passthrough families publish their weights, so the same id is
# also served by hosts that are not the vendor and that size it however they
# like. The other four are vendor-only: nobody but Anthropic serves
# `claude-opus-5`, so its arm cannot be wrong for somebody else's endpoint.
#
# This set is what stops the REPORT arm below from automating new violations.
# `PASSTHROUGH_IN_SCOPE` is a set of FLOORS -- "this generation and everything
# above it" -- so without this, the first `minimax-m4` or `deepseek-v5` to
# appear on models.dev reddens the release with the instruction "Add the arm if
# it has none, then add the row", which is precisely the arm the rule forbids.
PASSTHROUGH_OPEN_WEIGHTS = {"deepseek", "minimax"}

# What "wildly different" means, stated as a number so the gate can apply it.
# MEASURED on the 2026-08-28 pull: `minimax-m2.5` runs 65,536 -> 228,700 across
# 46 host rows (3.5x) and `deepseek-v4-pro` runs 128,000 -> 1,050,000 across 74
# rows (8.2x), while a vendor-only id like `claude-opus-5` agrees to the digit
# across every endpoint that serves it. 2.0 sits well clear of the 196,608 vs
# 204,800 kind of noise and well below both real cases.
HOST_SPREAD_RATIO = 2.0

# Specialty modalities the chain deliberately excludes: they are MUCH smaller
# than the text tier and an over-claim would 400 them, so they fail open to the
# unknown path on purpose.
PASSTHROUGH_EXCLUDE = re.compile(r"-(image|tts|native-audio|live)")

# models.dev id dressings that do not change which arm the substring lookup
# hits: Bedrock region prefixes, Bedrock's vendor prefix, Vertex `@` revisions,
# Bedrock `-v1:0` suffixes.
_REGION_PREFIX = re.compile(r"^(us|eu|jp|au|apac|global)\.")
_VENDOR_PREFIX = re.compile(r"^(anthropic|xai|meta|deepseek|mistral|amazon|cohere|ai21|qwen)\.")
_ID_SUFFIX = re.compile(r"(@[^@]*|-v\d+(:\d+)?|:\d+)$")

# Rows where a vendor-operated endpoint disagrees with ANOTHER vendor-operated
# endpoint and we deliberately follow the model's OWN vendor. Keyed by the exact
# four numbers involved, so this is a PIN and not a mute: if our figure changes
# or the observed floor moves in either direction, the key stops matching and
# the release goes red. Printed on every run.
PASSTHROUGH_PINS = {
    ("claude-sonnet-4-6", "output", 128_000, 64_000):
        "amazon-bedrock still republishes Anthropic's OLD 64,000 output figure "
        "for Sonnet 4.6, while `anthropic` -- the vendor that makes the model "
        "-- and google-vertex both report 128,000, matching Anthropic's own "
        "model overview and a Codex/Gemini cross-audit. Lowering the arm would "
        "halve real output on the vendor's own endpoint to match a reseller's "
        "lag.",
}

# Vendors we track closely enough that a brand-new family appearing there is
# worth the release owner's attention.
TRACKED_VENDORS = [
    "anthropic", "openai", "xai", "google", "deepseek", "minimax",
    "zhipuai", "zai", "alibaba", "moonshotai", "mistral", "llama",
]

# Family keys we already know about at those vendors -- either covered by the
# table, covered by the older `if` chain, or a modality we deliberately skip.
KNOWN_FAMILIES = {
    "claude", "gpt", "o1", "o3", "o4", "chatgpt", "codex", "dall", "whisper",
    "tts", "text", "davinci", "babbage", "omni", "computer", "sora", "gemini",
    "gemma", "imagen", "veo", "grok", "deepseek", "minimax", "abab", "speech",
    "music", "video", "glm", "cogview", "cogvideo", "charglm", "emohaa", "codegeex",
    "qwen", "qwq", "qvq", "wan", "tongyi", "farui", "bailian", "kimi", "moonshot",
    "mistral", "magistral", "codestral", "devstral", "ministral", "pixtral",
    "voxtral", "mixtral", "open", "labs", "llama", "muse", "embed", "rerank",
    # Google media / research modalities we deliberately never route.
    "lyria", "deep-research",
}


class Finding:
    def __init__(self, kind: str, text: str) -> None:
        self.kind = kind
        self.text = text


def parse_table(path: str, const: str = "CATALOGUE_CEILINGS") -> list[tuple[str, int, int]]:
    """Extract an ordered `&[(&str, u32, u32)]` table from a Rust source file."""
    src = open(path, encoding="utf-8").read()
    m = re.search(
        rf"const {const}: &\[\(&str, u32, u32\)\] = &\[(.*?)\n\];",
        src,
        re.S,
    )
    if not m:
        raise SystemExit(
            f"FATAL: could not locate `const {const}` in {path}. "
            "The gate cannot verify a table it cannot read -- failing closed."
        )
    body = m.group(1)
    entries = [
        (frag, int(out.replace("_", "")), int(ctx.replace("_", "")))
        for frag, out, ctx in re.findall(
            r'^\s*\("([^"]+)",\s*([0-9_]+),\s*([0-9_]+)\),\s*$', body, re.M
        )
    ]
    if not entries:
        raise SystemExit(f"FATAL: {const} in {path} parsed to ZERO entries.")
    return entries


def canonical(model_id: str) -> str:
    """Strip the models.dev id dressings that do not change the matched arm."""
    b = model_id.split("/")[-1].lower()
    b = _REGION_PREFIX.sub("", b)
    b = _VENDOR_PREFIX.sub("", b)
    prev = None
    while prev != b:
        prev = b
        b = _ID_SUFFIX.sub("", b)
    return b


def host_spread(catalogue: dict, model_id: str):
    """-> (low, high, [provider ids]) for one id across EVERY host, or None.

    Deliberately not restricted to `PASSTHROUGH_VENDORS`. The vendor floor is
    the right input for "is our arm honest about the vendor"; it is exactly the
    WRONG input for "may this id have a single static arm at all", because a
    vendor-only view cannot by construction observe the disagreement that makes
    an open-weights id unarmable. Junk is dropped the same way it is elsewhere:
    a missing or zero context is not a datum, and `output == context` is
    models.dev saying UNKNOWN.
    """
    seen, where = [], []
    for pid, prov in catalogue.items():
        for mid, meta in ((prov or {}).get("models") or {}).items():
            if canonical(mid) != model_id:
                continue
            ctx = (meta.get("limit") or {}).get("context")
            if not ctx:
                continue
            seen.append(ctx)
            where.append(pid)
    if len(seen) < 2:
        return None
    return min(seen), max(seen), sorted(set(where))


def scan_passthrough(catalogue: dict, rows: list[tuple[str, int, int]]) -> list[Finding]:
    """#1176 -- grade PASSTHROUGH_VENDOR_MODELS against vendor-operated rows.

    Exact id match, not containment: a NEW passthrough id must get its own row
    even when it happens to contain an older one, because that is exactly how a
    frontier model silently inherits a stale window (#165).
    """
    table = {mid: (out, ctx) for mid, out, ctx in rows}
    findings: list[Finding] = []

    for family, providers in PASSTHROUGH_VENDORS.items():
        rx = PASSTHROUGH_IN_SCOPE[family]
        observed: dict[str, dict] = {}
        for pid in providers:
            prov = catalogue.get(pid)
            if not prov:
                continue
            for mid, meta in (prov.get("models") or {}).items():
                cid = canonical(mid)
                if not rx.match(cid) or PASSTHROUGH_EXCLUDE.search(cid):
                    continue
                lim = meta.get("limit") or {}
                ctx, out = lim.get("context"), lim.get("output")
                if not ctx:
                    continue  # ctx=0 / missing is catalogue junk, not signal
                rec = observed.setdefault(cid, {"ctx": [], "out": [], "where": []})
                rec["ctx"].append(ctx)
                rec["where"].append(pid)
                # output == context is models.dev saying UNKNOWN, never a
                # ceiling. Dropping it here is what keeps grok-4.5/4.6 ungraded
                # on output while their context stays graded.
                if out and out != ctx:
                    rec["out"].append(out)

        for mid in sorted(observed):
            rec = observed[mid]
            where = sorted(set(rec["where"]))
            floor_ctx = min(rec["ctx"])
            if mid not in table:
                spread = (host_spread(catalogue, mid)
                          if family in PASSTHROUGH_OPEN_WEIGHTS else None)
                if spread and spread[1] >= spread[0] * HOST_SPREAD_RATIO:
                    low, high, hosts = spread
                    findings.append(Finding(
                        "REPORT",
                        f"[{family}] `{mid}` has no row, and it must NOT get "
                        f"one by default: it is an OPEN-WEIGHTS id served at "
                        f"context {low} to {high} ({high / low:.1f}x) across "
                        f"{len(hosts)} hosts {hosts}. AGENTS.md's third rule -- "
                        f"'do not add a static arm for an open-weights model "
                        f"served at wildly different limits by different "
                        f"hosts' -- forbids the arm, because `model_output_"
                        f"ceiling` is keyed on the model id alone and would "
                        f"hand the vendor's figure to every third-party host "
                        f"too. An arm is permitted ONLY if it is below the "
                        f"status-quo fallback in BOTH dimensions, per the rule's "
                        f"own Llama exception; otherwise leave it unarmed and "
                        f"record the decision. NOT a failure."
                    ))
                    continue
                out_note = (
                    f", output={min(rec['out'])}" if rec["out"]
                    else " (output UNKNOWN -- models.dev reports output == context)"
                )
                findings.append(Finding(
                    "FAIL",
                    f"[{family}] `{mid}` is served by vendor-operated {where} and "
                    f"reaches users through provider-native --model passthrough, "
                    f"but has NO row in PASSTHROUGH_VENDOR_MODELS. Nothing then "
                    f"grades whether `model_output_ceiling` sizes it, which is "
                    f"#165's exact shape. Vendor reports context={floor_ctx}"
                    + out_note
                    + ". Add the arm if it has none, then add the row."
                ))
                continue

            our_out, our_ctx = table[mid]
            if our_ctx > floor_ctx:
                findings.append(_pinned_or_fail(
                    mid, "context", our_ctx, floor_ctx,
                    f"[{family}] `{mid}` is recorded at context {our_ctx}, but "
                    f"vendor-operated {where} reports as low as {floor_ctx}. "
                    f"OVER-claiming context is the fatal direction."
                ))
            elif our_ctx < floor_ctx * 0.9:
                findings.append(_pinned_or_fail(
                    mid, "context", our_ctx, floor_ctx,
                    f"[{family}] `{mid}` is recorded at context {our_ctx} while "
                    f"vendor-operated {where} serves {floor_ctx} -- a "
                    f"{floor_ctx / our_ctx:.1f}x under-size (premature "
                    f"compaction, #165)."
                ))
            if rec["out"]:
                floor_out = min(rec["out"])
                if our_out > floor_out:
                    findings.append(_pinned_or_fail(
                        mid, "output", our_out, floor_out,
                        f"[{family}] `{mid}` is recorded at output {our_out}, but "
                        f"vendor-operated {where} reports as low as {floor_out}. "
                        f"An arm REVOKES `should_omit_max_tokens`, so an "
                        f"over-claim is a hard 400 mid-run."
                    ))

    return [f for f in findings if f is not None]


def _pinned_or_fail(mid: str, dim: str, ours: int, floor: int, text: str):
    """A pin turns one exact disagreement into a printed note instead of a FAIL.

    Keyed by all four numbers, so it cannot outlive the situation it describes.
    """
    reason = PASSTHROUGH_PINS.get((mid, dim, ours, floor))
    if reason is None:
        return Finding("FAIL", text)
    return Finding(
        "PINNED",
        f"`{mid}` {dim} {ours} vs vendor floor {floor}: {reason} Pinned to these "
        f"exact numbers -- if either moves, this becomes a FAIL.",
    )


def lookup(entries: list[tuple[str, int, int]], model: str):
    """Mirror the Rust lookup: first fragment contained in the lowercased id."""
    m = model.lower()
    for frag, out, ctx in entries:
        if frag in m:
            return frag, out, ctx
    return None


def bare(model_id: str) -> str:
    return model_id.split("/")[-1].lower()


def fetch(url: str, timeout: int):
    req = urllib.request.Request(url, headers={"User-Agent": "wayland-core-limits-gate"})
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read().decode("utf-8"))


def scan(catalogue: dict, entries: list[tuple[str, int, int]]) -> list[Finding]:
    findings: list[Finding] = []

    # ---- FAIL arm: in-scope first-party ids must have a correct arm ---------
    for family, providers in FIRST_PARTY.items():
        rx = IN_SCOPE[family]
        # id -> lowest first-party (context, output), output ignoring the
        # degenerate output==context encoding models.dev uses for "unknown".
        observed: dict[str, dict] = {}
        for pid in providers:
            prov = catalogue.get(pid)
            if not prov:
                continue
            for mid, meta in (prov.get("models") or {}).items():
                b = bare(mid)
                if not rx.match(b):
                    continue
                lim = meta.get("limit") or {}
                ctx, out = lim.get("context"), lim.get("output")
                if not ctx:
                    continue  # ctx=0 / missing is catalogue junk, not signal
                rec = observed.setdefault(b, {"ctx": [], "out": [], "where": []})
                rec["ctx"].append(ctx)
                rec["where"].append(pid)
                if out and out != ctx:
                    rec["out"].append(out)

        for mid in sorted(observed):
            rec = observed[mid]
            hit = lookup(entries, mid)
            if hit is None:
                findings.append(Finding(
                    "FAIL",
                    f"[{family}] `{mid}` is served first-party by "
                    f"{sorted(set(rec['where']))} and matches NO arm in "
                    f"CATALOGUE_CEILINGS. It falls to the CompactConfig default "
                    f"and is silently mis-sized. First-party reports "
                    f"context={min(rec['ctx'])}"
                    + (f", output={min(rec['out'])}" if rec["out"] else "")
                    + "."
                ))
                continue
            frag, our_out, our_ctx = hit
            floor_ctx = min(rec["ctx"])
            # A NEW model that merely inherits an older ANCESTOR arm ("glm-5.4"
            # falling into the "glm-5" fragment) has no arm of its own, and that
            # is #165 exactly: a frontier model silently handed a stale, far too
            # small window. Deliberate under-claims in this table are all within
            # a few percent (200,000 vs 204,800; 1,000,000 vs 1,048,576), so a
            # >10% shortfall is a fall-through, not policy.
            if our_ctx < floor_ctx * 0.9:
                fix = (
                    f"the arm \"{frag}\" is STALE -- raise it to {floor_ctx}"
                    if frag == mid else
                    f"`{mid}` has NO arm of its own and falls through to "
                    f"\"{frag}\" -- add one ABOVE it"
                )
                findings.append(Finding(
                    "FAIL",
                    f"[{family}] `{mid}` resolves to context {our_ctx} while "
                    f"first-party {sorted(set(rec['where']))} serves "
                    f"{floor_ctx} -- a {floor_ctx / our_ctx:.1f}x under-size "
                    f"(premature compaction, #165). {fix}."
                ))
            if our_ctx > floor_ctx:
                findings.append(Finding(
                    "FAIL",
                    f"[{family}] `{mid}` matches arm \"{frag}\" claiming context "
                    f"{our_ctx}, but first-party {sorted(set(rec['where']))} "
                    f"reports as low as {floor_ctx}. OVER-claiming context is "
                    f"the fatal direction -- lower the arm to {floor_ctx}."
                ))
            if rec["out"]:
                floor_out = min(rec["out"])
                if our_out > floor_out:
                    findings.append(Finding(
                        "FAIL",
                        f"[{family}] `{mid}` matches arm \"{frag}\" claiming "
                        f"output {our_out}, but first-party reports as low as "
                        f"{floor_out}. OVER-claiming output is a hard 400 -- "
                        f"lower the arm to {floor_out}."
                    ))

    # ---- REPORT arm: a whole new family at a vendor we track ---------------
    for pid in TRACKED_VENDORS:
        prov = catalogue.get(pid)
        if not prov:
            continue
        seen: dict[str, list[str]] = {}
        for mid in (prov.get("models") or {}):
            b = bare(mid)
            if any(known in b for known in KNOWN_FAMILIES):
                continue
            key = re.match(r"[a-z]+", b)
            if not key:
                continue
            seen.setdefault(key.group(0), []).append(b)
        for key, ids in sorted(seen.items()):
            findings.append(Finding(
                "REPORT",
                f"new family `{key}*` at vendor `{pid}`: "
                f"{sorted(ids)[:5]}{' ...' if len(ids) > 5 else ''}. Decide "
                f"whether wayland-core should route it; if so, add its verified "
                f"limits to CATALOGUE_CEILINGS."
            ))

    return findings


def report(findings: list[Finding], entries_count: int,
           passthrough_count: int = 0) -> int:
    fails = [f for f in findings if f.kind == "FAIL"]
    reports = [f for f in findings if f.kind == "REPORT"]
    pinned = [f for f in findings if f.kind == "PINNED"]

    print(f"model-limits freshness: checked {entries_count} CATALOGUE_CEILINGS "
          f"arms against {len(FIRST_PARTY)} first-party families.")
    print(f"#1176: checked {passthrough_count} PASSTHROUGH_VENDOR_MODELS rows "
          f"against {len(PASSTHROUGH_VENDORS)} `if`-chain families "
          f"({', '.join(sorted(PASSTHROUGH_VENDORS))}) on vendor-operated "
          f"endpoints only. The chain itself is graded by the Rust test "
          f"`every_passthrough_vendor_model_resolves_its_arm`, which runs on "
          f"every PR -- this half only asks whether the world moved.")
    print("NOT CHECKED (stated gap): ids outside the in-scope patterns -- the "
          "older generations these tables deliberately do not arm (Claude 3.x, "
          "GPT-3.5 / o-series, DeepSeek V3+R1, the Gemini image/tts/live "
          "modalities) -- and any endpoint that is not vendor-operated.")
    print("NOT DEMANDED (AGENTS.md rule 3): a NEW %s id whose hosts disagree by "
          "%.1fx or more is REPORTED, never failed. Those families publish "
          "their weights, `model_output_ceiling` is keyed on the id alone, and "
          "a static arm would hand the vendor's figure to every third-party "
          "host serving the same name."
          % ("/".join(sorted(PASSTHROUGH_OPEN_WEIGHTS)), HOST_SPREAD_RATIO))

    if pinned:
        print("\n" + "=" * 72)
        print("PINNED -- vendor-operated endpoints disagree and we follow the "
              "model's own vendor. NOT a failure, but read them.")
        print("=" * 72)
        for f in pinned:
            print(f"  * {f.text}")

    if reports:
        print("\n" + "=" * 72)
        print("REPORT -- new families seen. NOT a failure; a decision for the "
              "release owner.")
        print("=" * 72)
        for f in reports:
            print(f"  * {f.text}")

    if fails:
        print("\n" + "=" * 72)
        print(f"FAIL -- {len(fails)} model(s) we CLAIM to cover are missing or "
              "over-claimed.")
        print("=" * 72)
        for f in fails:
            print(f"  * {f.text}")
        print("\nRefresh crates/wcore-config/src/limits.rs (and its snapshot "
              "sha256/date comment) before releasing.")
        return 1

    print("\nPASS -- every in-scope first-party model has an arm, and no arm "
          "over-claims.")
    return 0


# --------------------------------------------------------------------------
# Self-test: proves the gate can reach BOTH verdicts, plus the skip banner.
# Modelled on scripts/check-no-vacuous-cargo-test.py --self-test. A gate nobody
# has watched fail has not been tested.
# --------------------------------------------------------------------------

_FIXTURE_TABLE = '''
const CATALOGUE_CEILINGS: &[(&str, u32, u32)] = &[
    ("glm-5.3", 128_000, 1_000_000),
    ("glm-5", 128_000, 200_000),
    ("kimi-k3", 128_000, 1_000_000),
];
'''


# #1176 fixtures. The table records ONE canonical spelling per model; the
# catalogue below deliberately dresses the same models the way models.dev does.
_FIXTURE_PASSTHROUGH = """
pub(crate) const PASSTHROUGH_VENDOR_MODELS: &[(&str, u32, u32)] = &[
    ("claude-opus-5", 128_000, 1_000_000),
    ("claude-sonnet-4-5", 64_000, 200_000),
    ("gpt-4o-2024-05-13", 4_096, 128_000),
    ("grok-4.6", 500_000, 500_000),
];
"""


def _fixture_passthrough_catalogue(**over):
    cat = {
        "anthropic": {"models": {
            "claude-opus-5": {"limit": {"context": 1_000_000, "output": 128_000}},
            "claude-sonnet-4-5": {"limit": {"context": 200_000, "output": 64_000}},
            # Out of scope on purpose: the chain does not arm Claude 3.x.
            "claude-3-opus": {"limit": {"context": 200_000, "output": 4_096}},
        }},
        "amazon-bedrock": {"models": {
            # Every dressing of ONE model: region prefix, vendor prefix,
            # `-v1:0`. All must canonicalize onto the single recorded row.
            "us.anthropic.claude-opus-5": {"limit": {"context": 1_000_000, "output": 128_000}},
            "anthropic.claude-sonnet-4-5-v1:0": {"limit": {"context": 200_000, "output": 64_000}},
        }},
        "google-vertex": {"models": {
            "claude-opus-5@default": {"limit": {"context": 1_000_000, "output": 128_000}},
        }},
        "openai": {"models": {
            "gpt-4o-2024-05-13": {"limit": {"context": 128_000, "output": 4_096}},
        }},
        "xai": {"models": {
            # output == context: models.dev saying UNKNOWN, never a ceiling.
            "grok-4.6": {"limit": {"context": 500_000, "output": 500_000}},
        }},
    }
    for pid, models in over.items():
        cat.setdefault(pid.replace("__", "-"), {"models": {}})["models"].update(models)
    return cat


def _fixture_catalogue(**over):
    cat = {
        "zai": {"models": {
            "glm-5.3": {"limit": {"context": 1_000_000, "output": 131_072}},
            "glm-5": {"limit": {"context": 204_800, "output": 131_072}},
        }},
        "moonshotai": {"models": {
            "kimi-k3": {"limit": {"context": 1_048_576, "output": 131_072}},
        }},
    }
    for pid, models in over.items():
        cat.setdefault(pid.replace("__", "-"), {"models": {}})["models"].update(models)
    return cat


def self_test() -> int:
    tmp = tempfile.mkdtemp(prefix="limits-gate-selftest-")
    table_path = os.path.join(tmp, "limits.rs")
    with open(table_path, "w", encoding="utf-8") as fh:
        fh.write(_FIXTURE_TABLE)
    entries = parse_table(table_path)
    assert len(entries) == 3, entries

    failures = []

    def case(name, catalogue, want_fail, want_report=False):
        found = scan(catalogue, entries)
        got_fail = any(f.kind == "FAIL" for f in found)
        got_report = any(f.kind == "REPORT" for f in found)
        ok = got_fail == want_fail and got_report == want_report
        print(f"  [{'ok' if ok else 'BROKEN'}] {name}: "
              f"FAIL={got_fail} (want {want_fail}), "
              f"REPORT={got_report} (want {want_report})")
        if not ok:
            for f in found:
                print(f"        -> {f.kind}: {f.text}")
            failures.append(name)

    print("self-test: the gate must reach PASS, FAIL and REPORT.")

    # 1. Clean tree, clean catalogue -> PASS. (A gate that cannot PASS gets
    #    disabled, which is worse than one that cannot fail.)
    case("PASS on a table that matches the catalogue", _fixture_catalogue(), False)

    # 2. A newly launched first-party id inside a covered family, with no arm.
    case(
        "FAIL when a covered family gains a first-party id with no arm",
        _fixture_catalogue(zai={"glm-5.4": {"limit": {"context": 2_000_000, "output": 131_072}}}),
        True,
    )

    # 2b. ...and the subtler shape: the new id DOES match an ancestor fragment
    #     ("glm-5.4" contains "glm-5"), so it is not literally armless -- it
    #     silently inherits a stale 200k window. That is #165 and must FAIL.
    case(
        "FAIL when a new id silently inherits an ancestor arm",
        _fixture_catalogue(zai={"glm-5.4": {"limit": {"context": 2_000_000, "output": 131_072}}}),
        True,
    )

    # 3. Our figure OVER-claims context against first-party.
    over = _fixture_catalogue()
    over["zai"]["models"]["glm-5.3"]["limit"]["context"] = 500_000
    case("FAIL when an arm over-claims CONTEXT vs first-party", over, True)

    # 4. Our figure OVER-claims output against first-party.
    over_out = _fixture_catalogue()
    over_out["moonshotai"]["models"]["kimi-k3"]["limit"]["output"] = 64_000
    case("FAIL when an arm over-claims OUTPUT vs first-party", over_out, True)

    # 5. UNDER-claiming is this table's policy, not a defect -> must NOT fail.
    under = _fixture_catalogue()
    under["zai"]["models"]["glm-5.3"]["limit"]["context"] = 1_048_576
    case("PASS when an arm under-claims by a few percent (deliberate policy)",
         under, False)

    # 5b. ...but a GROSS under-claim is a stale figure, not policy, and the
    #     distinction is the whole point: 1,000,000-vs-1,048,576 is the safety
    #     margin, 1,000,000-vs-4,000,000 is us being out of date.
    stale = _fixture_catalogue()
    stale["zai"]["models"]["glm-5.3"]["limit"]["context"] = 4_000_000
    case("FAIL when an arm is grossly stale vs first-party", stale, True)

    # 6. A deliberate exclusion (GLM 4.6 is documented as out of scope) must NOT
    #    redden the release -- this is what stops the gate being switched off.
    excl = _fixture_catalogue()
    excl["zai"]["models"]["glm-4.6"] = {"limit": {"context": 204_800, "output": 131_072}}
    case("PASS on a documented out-of-scope id (glm-4.6)", excl, False)

    # 7. A brand-new family at a tracked vendor: REPORT, never FAIL.
    newfam = _fixture_catalogue()
    newfam["mistral"] = {"models": {"nebulon-1": {"limit": {"context": 128_000, "output": 8_192}}}}
    case("REPORT (not FAIL) on a brand-new family", newfam, False, want_report=True)

    # 8. Catalogue junk (ctx=0) must not be read as a real limit.
    junk = _fixture_catalogue()
    junk["zai"]["models"]["glm-5.3"] = {"limit": {"context": 0, "output": 0}}
    case("PASS when the only row for an id is ctx=0 junk", junk, False)

    # 9. Offline must SKIP loudly, never pass silently.
    rc, banner = _offline_probe()
    # Match the real success LINE ("PASS -- every in-scope..."), not the word
    # "PASS", which the banner itself uses in "THIS IS NOT A PASS."
    ok = (rc == 0 and "SKIPPED" in banner and "NOT A PASS" in banner
          and "PASS --" not in banner)
    print(f"  [{'ok' if ok else 'BROKEN'}] offline announces SKIPPED and never "
          f"prints PASS: rc={rc}")
    if not ok:
        failures.append("offline skip banner")

    # ---- #1176: the passthrough arm ------------------------------------
    pt_path = os.path.join(tmp, "passthrough.rs")
    with open(pt_path, "w", encoding="utf-8") as fh:
        fh.write(_FIXTURE_PASSTHROUGH)
    pt_rows = parse_table(pt_path, "PASSTHROUGH_VENDOR_MODELS")
    assert len(pt_rows) == 4, pt_rows

    def pcase(name, catalogue, want_fail, want_pinned=False):
        found = scan_passthrough(catalogue, pt_rows)
        got_fail = any(f.kind == "FAIL" for f in found)
        got_pin = any(f.kind == "PINNED" for f in found)
        ok = got_fail == want_fail and got_pin == want_pinned
        print(f"  [{'ok' if ok else 'BROKEN'}] {name}: "
              f"FAIL={got_fail} (want {want_fail}), "
              f"PINNED={got_pin} (want {want_pinned})")
        if not ok:
            for f in found:
                print(f"        -> {f.kind}: {f.text}")
            failures.append(name)

    print("\nself-test (#1176 passthrough): the arm must reach PASS and FAIL.")

    # P1. Clean tree -> PASS. Also proves every models.dev dressing of a model
    #     (us./anthropic./@default/-v1:0) folds onto its ONE recorded row --
    #     without that, a clean tree would fail with four phantom "no row"s.
    pcase("PASS when every vendor id has a matching row", _fixture_passthrough_catalogue(), False)

    # P2. THE ACCEPTANCE CASE: a passthrough id served by the vendor with no
    #     row. This is `claude-opus-5` last cycle -- a frontier model shipped,
    #     nobody recorded it, and both old guards passed it over.
    pcase(
        "FAIL when a vendor passthrough id has no row (the #165 shape)",
        _fixture_passthrough_catalogue(
            anthropic={"claude-opus-6": {"limit": {"context": 2_000_000, "output": 256_000}}}),
        True,
    )

    # P3. Over-claiming CONTEXT is the fatal direction.
    over_ctx = _fixture_passthrough_catalogue()
    over_ctx["anthropic"]["models"]["claude-opus-5"]["limit"]["context"] = 400_000
    pcase("FAIL when a row over-claims CONTEXT vs the vendor", over_ctx, True)

    # P4. Over-claiming OUTPUT is a hard 400, because an arm revokes omission.
    over_out = _fixture_passthrough_catalogue()
    over_out["openai"]["models"]["gpt-4o-2024-05-13"]["limit"]["output"] = 2_048
    pcase("FAIL when a row over-claims OUTPUT vs the vendor", over_out, True)

    # P5. A GROSS under-claim is staleness, not the table's few-percent policy.
    stale = _fixture_passthrough_catalogue()
    stale["anthropic"]["models"]["claude-opus-5"]["limit"]["context"] = 4_000_000
    stale["amazon-bedrock"]["models"]["us.anthropic.claude-opus-5"]["limit"]["context"] = 4_000_000
    stale["google-vertex"]["models"]["claude-opus-5@default"]["limit"]["context"] = 4_000_000
    pcase("FAIL when a row is grossly stale vs the vendor", stale, True)

    # P6. NEGATIVE CONTROL: a documented out-of-scope id (Claude 3.x) is in the
    #     fixture catalogue throughout and must never redden anything. P1
    #     passing already proves it; assert the boundary explicitly, in both
    #     directions, so nobody widens the floor without noticing.
    for out_of_scope in ["claude-3-opus", "claude-3-5-sonnet-20241022"]:
        assert not PASSTHROUGH_IN_SCOPE["claude"].match(out_of_scope), out_of_scope
    for in_scope in ["claude-opus-5", "claude-opus-6", "claude-sonnet-12-1"]:
        assert PASSTHROUGH_IN_SCOPE["claude"].match(in_scope), in_scope
    for out_of_scope in ["gpt-4", "gpt-4-turbo", "gpt-4.5-preview", "gpt-3.5-turbo", "o3-pro"]:
        assert not PASSTHROUGH_IN_SCOPE["gpt"].match(out_of_scope), out_of_scope
    for in_scope in ["gpt-4o", "gpt-4.1", "gpt-5.6-sol", "gpt-6", "gpt-10"]:
        assert PASSTHROUGH_IN_SCOPE["gpt"].match(in_scope), in_scope
    assert not PASSTHROUGH_IN_SCOPE["deepseek"].match("deepseek-v3.2")
    assert PASSTHROUGH_IN_SCOPE["deepseek"].match("deepseek-v5")
    assert not PASSTHROUGH_IN_SCOPE["grok"].match("grok-2-vision")
    assert PASSTHROUGH_IN_SCOPE["grok"].match("grok-5")
    assert not PASSTHROUGH_IN_SCOPE["gemini"].match("gemini-2.0-flash")
    assert PASSTHROUGH_IN_SCOPE["gemini"].match("gemini-4-pro")
    assert not PASSTHROUGH_IN_SCOPE["minimax"].match("minimax-m1")
    assert PASSTHROUGH_IN_SCOPE["minimax"].match("minimax-m4")

    # P7. NEGATIVE CONTROL: output == context is UNKNOWN, never a ceiling. The
    #     grok-4.6 row records output 500,000 and the only vendor row is
    #     degenerate; grading it would be inventing a figure.
    degenerate = _fixture_passthrough_catalogue()
    degenerate["xai"]["models"]["grok-4.6"]["limit"] = {"context": 500_000, "output": 500_000}
    pcase("PASS when the vendor's output == context (UNKNOWN, not a ceiling)",
          degenerate, False)

    # P8. NEGATIVE CONTROL: an AGGREGATOR publishing junk for an in-scope id
    #     must not touch the verdict. This is the rule that keeps `ctx=0`,
    #     `out=1010000` and dropped digits out of the floor.
    junk = _fixture_passthrough_catalogue()
    junk["openrouter"] = {"models": {
        "anthropic/claude-opus-5": {"limit": {"context": 8_192, "output": 8_192}},
    }}
    junk["poe"] = {"models": {"claude-opus-5": {"limit": {"context": 0, "output": 0}}}}
    pcase("PASS when only a non-vendor aggregator disagrees", junk, False)

    # P12. #1176 c5 / AGENTS.md rule 3. A NEW open-weights generation appears
    #      inside an in-scope FAMILY FLOOR, and the hosts disagree wildly. The
    #      gate must not demand an arm for it -- REPORT, never FAIL. Before
    #      this arm existed the floor `^minimax-m(?:[2-9]|\d\d)` reddened the
    #      release with "Add the arm if it has none", automating the exact
    #      violation the rule forbids.
    ow = _fixture_passthrough_catalogue(
        minimax={"minimax-m4": {"limit": {"context": 204_800, "output": 128_000}}})
    ow["nebius"] = {"models": {
        "minimax-m4": {"limit": {"context": 65_536, "output": 8_192}}}}
    ow["openrouter"] = {"models": {
        "minimax/minimax-m4": {"limit": {"context": 196_608, "output": 16_000}}}}
    pcase("REPORT, not FAIL, for a new open-weights id the hosts disagree on",
          ow, False)
    found = scan_passthrough(ow, pt_rows)
    ok = any(f.kind == "REPORT" and "minimax-m4" in f.text for f in found)
    print(f"  [{'ok' if ok else 'BROKEN'}] ...and it is REPORTED rather than "
          f"passed over in silence")
    if not ok:
        for f in found:
            print(f"        -> {f.kind}: {f.text}")
        failures.append("open-weights suppression is reported")

    # P13. THE CONTROL FOR P12, and the reason the suppression is a MEASUREMENT
    #      rather than a family-wide exemption. Same family, same floor, but
    #      every host agrees: the id is armable and its absence is the #165
    #      shape again, so it must still FAIL.
    agreed = _fixture_passthrough_catalogue(
        minimax={"minimax-m4": {"limit": {"context": 204_800, "output": 128_000}}})
    agreed["nebius"] = {"models": {
        "minimax-m4": {"limit": {"context": 204_800, "output": 128_000}}}}
    pcase("FAIL for a new open-weights id every host serves identically",
          agreed, True)

    # P14. The suppression must not leak to the vendor-only families. Nobody
    #      but Anthropic serves claude-opus-6; an aggregator publishing 8,192
    #      for it is junk, not a host spread, and the missing arm is still #165.
    leak = _fixture_passthrough_catalogue(
        anthropic={"claude-opus-6": {"limit": {"context": 2_000_000, "output": 256_000}}})
    leak["openrouter"] = {"models": {
        "anthropic/claude-opus-6": {"limit": {"context": 8_192, "output": 8_192}}}}
    pcase("FAIL still, for a vendor-only id an aggregator disagrees about",
          leak, True)

    # P9. A pin suppresses ONE exact disagreement...
    pinned = _fixture_passthrough_catalogue()
    pinned["amazon-bedrock"]["models"]["claude-sonnet-4-6"] = {
        "limit": {"context": 1_000_000, "output": 64_000}}
    pinned["anthropic"]["models"]["claude-sonnet-4-6"] = {
        "limit": {"context": 1_000_000, "output": 128_000}}
    pin_rows = pt_rows + [("claude-sonnet-4-6", 128_000, 1_000_000)]
    found = scan_passthrough(pinned, pin_rows)
    ok = (not any(f.kind == "FAIL" for f in found)
          and any(f.kind == "PINNED" for f in found))
    print(f"  [{'ok' if ok else 'BROKEN'}] a recorded vendor-vs-vendor "
          f"disagreement is PINNED, not FAILED")
    if not ok:
        failures.append("passthrough pin applies")

    # P10. ...and the pin is NARROW: move the observed floor and it stops
    #      applying. A pin that outlives its situation is a mute.
    moved = pinned
    moved["amazon-bedrock"]["models"]["claude-sonnet-4-6"]["limit"]["output"] = 32_000
    found = scan_passthrough(moved, pin_rows)
    ok = any(f.kind == "FAIL" for f in found)
    print(f"  [{'ok' if ok else 'BROKEN'}] the pin stops applying when the "
          f"observed floor moves")
    if not ok:
        for f in found:
            print(f"        -> {f.kind}: {f.text}")
        failures.append("passthrough pin is narrow")

    # P11. The passthrough table must fail CLOSED too.
    try:
        parse_table(pt_path, "NO_SUCH_CONST")
        print("  [BROKEN] a missing passthrough const did NOT fail closed")
        failures.append("passthrough fail-closed")
    except SystemExit:
        print("  [ok] a missing passthrough const fails CLOSED")

    # 10. A table the parser cannot read must fail CLOSED, not scan nothing.
    broken = os.path.join(tmp, "broken.rs")
    open(broken, "w", encoding="utf-8").write("// no table here\n")
    try:
        parse_table(broken)
        print("  [BROKEN] unreadable table did NOT fail closed")
        failures.append("fail-closed on unparseable table")
    except SystemExit:
        print("  [ok] unreadable table fails CLOSED")

    if failures:
        print(f"\nSELF-TEST FAILED: {failures}")
        return 1
    print("\nself-test OK: gate reaches PASS, FAIL, REPORT and SKIP.")
    return 0


def _offline_probe():
    """Run the real offline path and capture what it prints."""
    import io
    import contextlib
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        rc = _skip("simulated: name resolution failed")
    return rc, buf.getvalue()


def _skip(reason: str) -> int:
    print("=" * 72)
    print("SKIPPED -- models.dev was unreachable. THIS IS NOT A PASS.")
    print(f"reason: {reason}")
    print("The model-limits table was NOT verified against the live catalogue.")
    print("Re-run when the network is back, or pass --catalogue <file> with a")
    print("snapshot. Do not read this run as evidence the table is current.")
    print("=" * 72)
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--self-test", action="store_true",
                    help="prove the gate reaches PASS, FAIL, REPORT and SKIP (offline)")
    ap.add_argument("--limits", default=DEFAULT_LIMITS, help="path to limits.rs")
    ap.add_argument("--passthrough", default=DEFAULT_PASSTHROUGH,
                    help="path to limits/passthrough.rs (#1176)")
    ap.add_argument("--catalogue", help="read a models.dev snapshot from disk instead of the network")
    ap.add_argument("--timeout", type=int, default=30)
    args = ap.parse_args()

    if args.self_test:
        return self_test()

    entries = parse_table(args.limits)
    passthrough = parse_table(args.passthrough, "PASSTHROUGH_VENDOR_MODELS")

    if args.catalogue:
        catalogue = json.load(open(args.catalogue, encoding="utf-8"))
    else:
        try:
            catalogue = fetch(CATALOGUE_URL, args.timeout)
        except (urllib.error.URLError, TimeoutError, OSError, json.JSONDecodeError) as exc:
            return _skip(f"{type(exc).__name__}: {exc}")

    if not isinstance(catalogue, dict) or len(catalogue) < 20:
        return _skip(
            f"catalogue looked malformed ({type(catalogue).__name__}, "
            f"{len(catalogue) if hasattr(catalogue, '__len__') else '?'} entries) "
            "-- refusing to grade the table against it"
        )

    findings = scan(catalogue, entries) + scan_passthrough(catalogue, passthrough)
    return report(findings, len(entries), len(passthrough))


if __name__ == "__main__":
    sys.exit(main())
