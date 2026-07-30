#!/usr/bin/env python3
"""Verify that every advisory suppression's parent trace matches the real graph.

WHY THIS EXISTS (F29-02-H1)
---------------------------
`.cargo/audit.toml` once silenced RUSTSEC-2026-0194/0195 on a stated "sole path"
of `quick-xml <- plist <- syntect <- wcore-cli`. The real lockfile had THREE
direct parent edges, two of them through `wcore-tools`, which parses
user-supplied docx/pptx/xlsx. RUSTSEC-2026-0194 was reachable from an
attacker-supplied spreadsheet: calamine 0.26.1's `next_cell()` calls
`BytesStart::attributes()` with the default duplicate check enabled.

The suppression's reachability argument was not sloppy — it was *correct about
the one path it examined*, and that path was the only safe one of the three.
That is the failure mode this gate exists to make impossible: a justification
that is true of a sample and presented as true of the graph.

It has recurred. Two more traces were found wrong by hand on 2026-07-29:
  * paste (RUSTSEC-2024-0436)      — named `ratatui` as a puller (false: ratatui
                                     pulls no paste) and omitted the
                                     `tokenizers` root.
  * rustls-pemfile (RUSTSEC-2025-0134) — asserted "transitive ONLY via bollard",
                                     naming one of two direct edges.

Three instances, all caught by a human re-deriving the graph. This gate does
that re-derivation mechanically, on every run.

WHAT IT CHECKS
--------------
Every suppression entry must carry a machine-checkable trace tag in its reason:

    [trace crate=<name>@<version> parents=<a,b,...|NONE> expires=<YYYY-MM-DD>]

and the gate then enforces, against `Cargo.lock`:

  C1 RESOLVES   the named crate@version is actually in the lockfile. Catches a
                suppression left behind after the crate left the tree — a mute
                with nothing under it, which hides the next real hit.
  C2 PARENTS    the declared parent set EQUALS the derived direct-parent set.
                Equality, not containment, so it catches BOTH an omitted parent
                (the quick-xml and tokenizers defects) AND a phantom parent that
                is not really there (the ratatui defect).
  C3 EXPIRES    an expiry is declared and has not passed. A permanent mute is a
                decision no one revisits.
  C4 TAGGED     an entry with no trace tag FAILS. Prose is not checkable, and a
                gate that skips untagged entries would be bypassed by writing
                prose — a skip is not a pass.

Derivation is from `Cargo.lock`, i.e. the union of all features and targets.
That is deliberately WIDER than a feature-filtered `cargo tree`: it can require
you to document a parent edge that no default build activates. For a suppression
justification that is the correct bias — you are asserting something about every
way the crate can be reached, so you should have to name every way.

USAGE
    scripts/verify-suppression-traces.py                 # gate
    scripts/verify-suppression-traces.py --self-test     # prove it can fail AND pass
    scripts/verify-suppression-traces.py --now 2027-01-01  # expiry testing
"""

from __future__ import annotations

import argparse
import collections
import datetime as _dt
import re
import sys
from pathlib import Path

TRACE_RE = re.compile(
    r"\[trace\s+crate=(?P<crate>[A-Za-z0-9_.-]+)@(?P<version>[0-9][0-9A-Za-z.+-]*)"
    r"\s+parents=(?P<parents>[A-Za-z0-9_.,+-]+)"
    r"\s+expires=(?P<expires>\d{4}-\d{2}-\d{2})\s*\]"
)


# ---------------------------------------------------------------- lockfile ---
class Lock:
    """Minimal Cargo.lock reader + reverse-dependency index."""

    def __init__(self, text: str):
        self.pkgs = []
        for stanza in text.split("[[package]]")[1:]:
            name = re.search(r'^name = "([^"]+)"', stanza, re.M)
            ver = re.search(r'^version = "([^"]+)"', stanza, re.M)
            if not (name and ver):
                continue
            deps = []
            # Tolerate BOTH the multi-line array cargo actually writes and a
            # single-line array. An earlier version anchored the closing bracket
            # with `^\]`, which silently produced an EMPTY dependency set for
            # inline arrays — every parent set came back empty and the
            # self-test's failure cases passed for the wrong reason.
            m = re.search(r"^dependencies = \[(.*?)\]", stanza, re.M | re.S)
            if m:
                deps = re.findall(r'"([^"]+)"', m.group(1))
            self.pkgs.append(
                {"name": name.group(1), "version": ver.group(1), "deps": deps}
            )
        self.by_name = collections.defaultdict(list)
        for p in self.pkgs:
            self.by_name[p["name"]].append(p)

    def alive(self) -> bool:
        """A parser that returns nothing makes every check vacuously pass."""
        return len(self.pkgs) > 100 and bool(self.by_name.get("serde"))

    def has(self, name: str, version: str) -> bool:
        return any(p["version"] == version for p in self.by_name.get(name, []))

    def direct_parents(self, name: str, version: str) -> set[str]:
        out = set()
        multi = len(self.by_name.get(name, [])) > 1
        for p in self.pkgs:
            for d in p["deps"]:
                parts = d.split()
                if parts[0] != name:
                    continue
                # A dep entry is "name", "name ver", or "name ver (source)".
                # An unversioned entry binds only when one version is resolved.
                if len(parts) == 1 and not multi:
                    out.add(p["name"])
                elif len(parts) > 1 and parts[1] == version:
                    out.add(p["name"])
        return out


# ------------------------------------------------------------ suppressions ---
Entry = collections.namedtuple("Entry", "source id reason")


def parse_deny(text: str) -> list[Entry]:
    """`[advisories] ignore = [ { id = "...", reason = "..." }, ... ]`"""
    out = []
    m = re.search(r"^ignore = \[(.*?)^\]", text, re.M | re.S)
    if not m:
        return out
    for rec in re.finditer(r"\{(.*?)\}\s*,", m.group(1), re.S):
        body = rec.group(1)
        i = re.search(r'id\s*=\s*"([^"]+)"', body)
        r = re.search(r'reason\s*=\s*"(.*?)"\s*$', body, re.S)
        if i:
            out.append(Entry("deny.toml", i.group(1), r.group(1) if r else ""))
    return out


def parse_osv(text: str) -> list[Entry]:
    """`[[IgnoredVulns]]` blocks with id / reason."""
    out = []
    for block in text.split("[[IgnoredVulns]]")[1:]:
        block = block.split("\n[")[0]
        i = re.search(r'^id\s*=\s*"([^"]+)"', block, re.M)
        r = re.search(r'^reason\s*=\s*"(.*?)"\s*$', block, re.M | re.S)
        if i:
            out.append(
                Entry(".github/osv-scanner.toml", i.group(1), r.group(1) if r else "")
            )
    return out


def parse_audit(text: str) -> list[Entry]:
    """`.cargo/audit.toml` — bare-id list or inline tables."""
    out = []
    m = re.search(r"^ignore = \[(.*?)\]", text, re.M | re.S)
    if not m:
        return out
    body = m.group(1)
    for rec in re.finditer(r"\{(.*?)\}", body, re.S):
        i = re.search(r'id\s*=\s*"([^"]+)"', rec.group(1))
        r = re.search(r'reason\s*=\s*"(.*?)"\s*$', rec.group(1), re.S)
        if i:
            out.append(
                Entry(".cargo/audit.toml", i.group(1), r.group(1) if r else "")
            )
    stripped = re.sub(r"\{.*?\}", "", body, flags=re.S)
    for bare in re.findall(r'"([A-Z]+-\d{4}-\d+)"', stripped):
        out.append(Entry(".cargo/audit.toml", bare, ""))
    return out


# -------------------------------------------------------------------- gate ---
def check(entries: list[Entry], lock: Lock, now: _dt.date):
    """Return (failures, examined). Each failure is (entry, code, detail)."""
    failures = []
    for e in entries:
        m = TRACE_RE.search(e.reason)
        if not m:
            failures.append(
                (e, "C4-UNTAGGED", "no [trace crate=..@.. parents=.. expires=..] tag")
            )
            continue
        crate, version = m.group("crate"), m.group("version")

        if not lock.has(crate, version):
            failures.append(
                (e, "C1-STALE", f"{crate}@{version} is not in Cargo.lock — delete this suppression or correct the version")
            )
            continue

        declared = set()
        if m.group("parents") != "NONE":
            declared = {p for p in m.group("parents").split(",") if p}
        actual = lock.direct_parents(crate, version)
        if declared != actual:
            missing = sorted(actual - declared)
            phantom = sorted(declared - actual)
            bits = []
            if missing:
                bits.append(f"UNDOCUMENTED parents {missing}")
            if phantom:
                bits.append(f"PHANTOM parents {phantom}")
            failures.append(
                (e, "C2-PARENTS",
                 f"{crate}@{version}: declared {sorted(declared)} but graph has "
                 f"{sorted(actual)} — " + "; ".join(bits))
            )
            continue

        exp = _dt.date.fromisoformat(m.group("expires"))
        if exp <= now:
            failures.append(
                (e, "C3-EXPIRED", f"expired {exp} (now {now}) — re-derive the trace and re-accept, or remove")
            )
    return failures, len(entries)


def load(root: Path) -> list[Entry]:
    entries = []
    for rel, fn in (
        ("deny.toml", parse_deny),
        (".github/osv-scanner.toml", parse_osv),
        (".cargo/audit.toml", parse_audit),
    ):
        p = root / rel
        if p.exists():
            entries += fn(p.read_text(encoding="utf-8"))
    return entries


def run(root: Path, now: _dt.date) -> int:
    lock = Lock((root / "Cargo.lock").read_text(encoding="utf-8"))
    if not lock.alive():
        print("FATAL: Cargo.lock parsed to nothing — instrument dead, refusing to pass")
        return 2
    print(f"lockfile packages parsed: {len(lock.pkgs)}  (parser self-test: OK)")

    entries = load(root)
    failures, examined = check(entries, lock, now)

    by_src = collections.Counter(e.source for e in entries)
    for src, n in sorted(by_src.items()):
        print(f"  {src}: {n} suppression(s)")
    print(f"SUPPRESSIONS_EXAMINED={examined}")
    print(f"SUPPRESSIONS_FAILED={len(failures)}")

    for e, code, detail in failures:
        print(f"\nFAIL [{code}] {e.source}  {e.id}\n      {detail}")

    if failures:
        print(f"\nverify-suppression-traces: FAILED ({len(failures)} of {examined})")
        return 1
    print(f"\nverify-suppression-traces: OK ({examined} suppression(s) verified "
          f"against the real graph)")
    return 0


# --------------------------------------------------------------- self-test ---
SELF_TEST_LOCK = """
[[package]]
name = "serde"
version = "1.0.0"

[[package]]
name = "quick-xml"
version = "0.39.4"

[[package]]
name = "quick-xml"
version = "0.31.0"

[[package]]
name = "calamine"
version = "0.26.1"
dependencies = [
 "quick-xml 0.31.0",
]

[[package]]
name = "plist"
version = "1.9.0"
dependencies = [
 "quick-xml 0.39.4",
]

[[package]]
name = "wcore-tools"
version = "0.12.25"
dependencies = [
 "quick-xml 0.39.4",
 "calamine 0.26.1",
]

[[package]]
name = "paste"
version = "1.0.15"

[[package]]
name = "tokenizers"
version = "0.21.4"
dependencies = [
 "paste 1.0.15",
]

[[package]]
name = "gemm"
version = "0.19.0"
dependencies = [
 "paste 1.0.15",
]

[[package]]
name = "rustls-pemfile"
version = "2.2.0"

[[package]]
name = "bollard"
version = "0.17.1"
dependencies = [
 "rustls-pemfile 2.2.0",
]

[[package]]
name = "rustls-native-certs"
version = "0.7.3"
dependencies = [
 "rustls-pemfile 2.2.0",
]
""" + "\n".join(
    f'[[package]]\nname = "filler{i}"\nversion = "0.1.0"\n' for i in range(120)
)

T = "[trace crate={c}@{v} parents={p} expires={e}]"


def self_test() -> int:
    lock = Lock(SELF_TEST_LOCK)
    now = _dt.date(2026, 7, 30)
    assert lock.alive(), "self-test lockfile must be alive"

    # FIXTURE GUARD. The first version of this self-test wrote its dependency
    # arrays inline while the parser required a line-leading `]`, so EVERY
    # parent set was empty. The failure cases still "passed" — because declared
    # != empty — i.e. the self-test was green on a fixture with no edges at all.
    # Assert the fixture's edges exist before trusting any assertion built on it.
    assert lock.direct_parents("quick-xml", "0.39.4") == {"plist", "wcore-tools"}, \
        "fixture has no edges — self-test would pass for the wrong reason"
    assert lock.direct_parents("paste", "1.0.15") == {"gemm", "tokenizers"}
    assert lock.direct_parents("rustls-pemfile", "2.2.0") == {
        "bollard", "rustls-native-certs"}
    assert lock.direct_parents("serde", "1.0.0") == set()

    results = []

    def case(label, entries, want_fail, want_code=None):
        fails, examined = check(entries, lock, now)
        got_fail = bool(fails)
        codes = [c for _, c, _ in fails]
        ok = got_fail == want_fail and (want_code is None or want_code in codes)
        results.append((ok, label, f"examined={examined} codes={codes}"))
        return ok

    E = lambda r: [Entry("self-test", "RUSTSEC-TEST", r)]  # noqa: E731

    # ---- direction 1: CAN IT PASS? (a gate with no reachable pass state is
    # as worthless as one that cannot fail — LANE-BRIEF 3b-iii)
    case("PASS: complete, accurate, unexpired trace",
         E("quick-xml everywhere " + T.format(
             c="quick-xml", v="0.39.4", p="plist,wcore-tools", e="2099-01-01")),
         want_fail=False)
    case("PASS: crate with no parents declares NONE",
         E(T.format(c="serde", v="1.0.0", p="NONE", e="2099-01-01")),
         want_fail=False)
    case("PASS: the five real deny.toml shapes (1, 2 and many parents)",
         [Entry("self-test", "A", T.format(c="rustls-pemfile", v="2.2.0",
                p="bollard,rustls-native-certs", e="2099-01-01")),
          Entry("self-test", "B", T.format(c="paste", v="1.0.15",
                p="gemm,tokenizers", e="2099-01-01"))],
         want_fail=False)

    # ---- direction 2: CAN IT FAIL? one case per historical defect
    case("FAIL: the ACTUAL F29-02-H1 defect — 'sole path' naming 1 of 3",
         E("sole path: quick-xml <- plist <- syntect <- wcore-cli " + T.format(
             c="quick-xml", v="0.39.4", p="plist", e="2099-01-01")),
         want_fail=True, want_code="C2-PARENTS")
    case("FAIL: the ACTUAL paste defect — phantom ratatui + omitted tokenizers",
         E(T.format(c="paste", v="1.0.15", p="gemm,ratatui", e="2099-01-01")),
         want_fail=True, want_code="C2-PARENTS")
    case("FAIL: the ACTUAL rustls-pemfile defect — 'only via bollard', 1 of 2",
         E(T.format(c="rustls-pemfile", v="2.2.0", p="bollard", e="2099-01-01")),
         want_fail=True, want_code="C2-PARENTS")
    case("FAIL: stale mute — crate no longer in the lockfile",
         E(T.format(c="quick-xml", v="0.41.0", p="plist", e="2099-01-01")),
         want_fail=True, want_code="C1-STALE")
    case("FAIL: expired suppression",
         E(T.format(c="serde", v="1.0.0", p="NONE", e="2026-01-01")),
         want_fail=True, want_code="C3-EXPIRED")
    case("FAIL: prose-only reason cannot bypass the gate",
         E("Definitely fine, transitive only via plist, trust me."),
         want_fail=True, want_code="C4-UNTAGGED")

    # ---- direction 3: would the OLD instrument have missed it? (LANE-BRIEF
    # 6b-ii demands this third assertion; without it a self-test passes on a
    # broken gate too.) The predecessor "instrument" was prose containment:
    # does the reason MENTION the parent? On the real F29-02-H1 text it does
    # mention plist, so a containment matcher goes green on the defect.
    defect = "sole path: quick-xml <- plist <- syntect <- wcore-cli"
    old_matcher_green = "plist" in defect
    new_catches = bool(check(E(defect + " " + T.format(
        c="quick-xml", v="0.39.4", p="plist", e="2099-01-01")), lock, now)[0])
    results.append((old_matcher_green and new_catches,
                    "CONTROL: prose-containment matcher passes the defect, this gate fails it",
                    f"old_green={old_matcher_green} new_fails={new_catches}"))

    # ---- direction 4: a dead parser must not pass anything
    results.append((not Lock("").alive(),
                    "CONTROL: empty lockfile is detected as a dead instrument",
                    "Lock('').alive() is False"))

    width = max(len(l) for _, l, _ in results)
    for ok, label, detail in results:
        print(f"[{'ok  ' if ok else 'FAIL'}] {label.ljust(width)}  {detail}")
    passed = sum(1 for ok, _, _ in results if ok)
    print(f"\nself-test: {passed}/{len(results)} assertions passed")
    return 0 if passed == len(results) else 1


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default=".", type=Path)
    ap.add_argument("--now", default=None, help="override today's date (YYYY-MM-DD)")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    now = _dt.date.fromisoformat(a.now) if a.now else _dt.date.today()
    return run(a.root, now)


if __name__ == "__main__":
    sys.exit(main())
