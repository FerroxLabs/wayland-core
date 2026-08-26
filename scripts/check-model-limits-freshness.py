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

WHAT IT DOES *NOT* CHECK -- STATED, NOT HIDDEN
----------------------------------------------
Only the ordered `CATALOGUE_CEILINGS` table (GLM / Qwen / Kimi / Mistral /
Llama). The older `if`-chain families (Claude, GPT-4.x/5.x, Grok, Gemini,
DeepSeek, MiniMax) carry conditional logic -- nested `!contains` guards, exact
`==` matches -- that a text parser cannot evaluate, and a parser that silently
MIS-evaluates them would be worse than this stated gap. Extending coverage there
means exposing the real Rust lookup to this script, not teaching it to guess.
The gap is printed on every run so it stays visible.

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
DEFAULT_LIMITS = "crates/wcore-config/src/limits.rs"

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


def parse_table(path: str) -> list[tuple[str, int, int]]:
    """Extract the ordered CATALOGUE_CEILINGS entries from limits.rs."""
    src = open(path, encoding="utf-8").read()
    m = re.search(
        r"const CATALOGUE_CEILINGS: &\[\(&str, u32, u32\)\] = &\[(.*?)\n\];",
        src,
        re.S,
    )
    if not m:
        raise SystemExit(
            f"FATAL: could not locate `const CATALOGUE_CEILINGS` in {path}. "
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
        raise SystemExit(f"FATAL: CATALOGUE_CEILINGS in {path} parsed to ZERO entries.")
    return entries


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


def report(findings: list[Finding], entries_count: int) -> int:
    fails = [f for f in findings if f.kind == "FAIL"]
    reports = [f for f in findings if f.kind == "REPORT"]

    print(f"model-limits freshness: checked {entries_count} CATALOGUE_CEILINGS "
          f"arms against {len(FIRST_PARTY)} first-party families.")
    print("NOT CHECKED (stated gap): the older `if`-chain families -- Claude, "
          "GPT-4.x/5.x, Grok, Gemini, DeepSeek, MiniMax. Their conditional arms "
          "cannot be evaluated by a text parser; verify those by hand.")

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
    ap.add_argument("--catalogue", help="read a models.dev snapshot from disk instead of the network")
    ap.add_argument("--timeout", type=int, default=30)
    args = ap.parse_args()

    if args.self_test:
        return self_test()

    entries = parse_table(args.limits)

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

    return report(scan(catalogue, entries), len(entries))


if __name__ == "__main__":
    sys.exit(main())
