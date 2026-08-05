#!/usr/bin/env python3
"""f28-resolve-candidate.py — answer "what exactly are we certifying?" without anyone
typing a feature name.

Phase 28 certifies an artifact that did not exist when its plans were written: phases 24
through 27 were executing in parallel lanes. A matrix written against predicted features
would certify the prediction. So the surface inventory is READ OFF THE SHIPPED BINARY'S
OWN COMMAND TREE. Phase documents are used for exactly two secondary purposes — attributing
a discovered surface to the phase that produced it, and detecting the two mismatch classes:

  * claimed-but-absent  — a phase artifact claims a surface the binary does not expose.
  * present-but-unclaimed — the binary exposes a surface no phase artifact claims.

Both are FINDINGS. Neither is a reason to skip certifying what is actually there.

FAIL-CLOSED, never guess:
  * a phase with no verdict/summary artifact refuses the whole resolution, naming which;
  * a commit not bound to a tree hash refuses, so a moved branch cannot masquerade;
  * a ref instead of a full 40-hex sha refuses rather than picking the newest;
  * every target gets a digest OR an explicit unbindable entry carrying its reason.
    An omission is impossible by schema.

REPRODUCIBLE: the emitted candidate.json contains NO timestamp. It records every input
with its sha256, so `--verify-reproducible` re-resolves from those exact inputs and
compares bytes. A changed input is caught by its digest; a non-deterministic resolver is
caught by the byte comparison.

Exit codes: 0 ok, 1 rejection/fail-closed, 2 usage.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

SCHEMA = "f28-candidate/v1"

# The six per-target release artifacts CI uploads (`.github/workflows/ci.yml`, the
# release-binary matrix; upload added in d9c7683b). Fixed: a target may be UNBINDABLE but
# it may never be absent from the ledger.
TARGETS = (
    ("x86_64-unknown-linux-gnu", "linux"),
    ("aarch64-unknown-linux-gnu", "linux"),
    ("x86_64-apple-darwin", "macos"),
    ("aarch64-apple-darwin", "macos"),
    ("x86_64-pc-windows-msvc", "windows"),
    ("aarch64-pc-windows-msvc", "windows"),
)
ARTIFACT_NAME = "wayland-core-{target}"

CERTIFIED_PHASES = ("24", "25", "26", "27")

SHA40 = re.compile(r"^[0-9a-f]{40}$")

# A claimed CLI surface, extracted ONLY from inside backticks or fenced code blocks. Prose
# mentions are not claims; a phase that claims a verb writes it as code.
#
# TWO forms, because measuring only the first has poor recall and under-detection would
# manufacture false present-but-unclaimed findings:
#   (a) explicit  — `wayland-core gateway status`
#   (b) bare      — `gateway status`, a code span that is ENTIRELY a command. The
#                   whole-span anchor plus the external-tool stoplist is what keeps this
#                   from swallowing `cargo test -p x` or prose.
CLAIM_EXPLICIT = re.compile(r"wayland-core\s+([a-z][a-z0-9-]*)")
CLAIM_BARE = re.compile(r"^([a-z][a-z0-9-]{2,})(?: [a-z][a-z0-9-]*)+$")

# Global flags and non-verbs that follow `wayland-core` in a code span.
NOT_A_VERB = frozenset(
    {"help", "p", "k", "b", "m", "exe", "bin", "target", "release", "debug"}
)

# Commands that are NOT wayland-core verbs. A bare-form span starting with one of these is
# somebody else's tool, not a claimed surface.
EXTERNAL_TOOLS = frozenset(
    """cargo git ssh scp rsync python python3 pip npm npx node just gh sed awk grep egrep
    rg echo printf cat tail head less sort uniq wc cut tr find xargs rm mkdir mv cp ln ls
    chmod chown touch tar zip unzip curl wget jq yq docker podman cross rustup vx cd
    export set unset source sudo su bash sh zsh pwsh powershell cmd schtasks systemctl
    launchctl service sha256sum shasum openssl base64 diff patch make cmake ninja clippy
    rustfmt nextest test true false sleep kill ps top df du env which type readlink
    dirname basename mktemp trap wait exec eval return exit local let const var function
    if then else elif fi for while do done case esac in not the this that with from into
    see run use add get put new old all any one two see note todo fixme
    wayland-core wayland core
    pub fn impl struct enum mod match async await trait where dyn crate ref mut
    async_trait derive cfg allow deny warn expect unwrap""".split()
)

# Words that mark a backtick span as English prose rather than a command line.
ENGLISH_NOISE = frozenset(
    """the a an is are was were be been being of to for and or if when whenever that this
    it its as at by on in with from into than then so but not no yes only always never
    each every some most more less other another same such via per over under after
    before while during until unless because since although though however therefore
    can could may might must should would will shall does did done has have had""".split()
)


class Refusal(Exception):
    """A fail-closed condition. The resolver refuses to emit rather than guess."""


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    return sha256_bytes(path.read_bytes())


# --------------------------------------------------------------------------------------
# Surface capture parsing — the binary's own self-description
# --------------------------------------------------------------------------------------


def parse_surface_capture(text: str) -> dict:
    """Parse the delimited transcript of the binary describing itself.

    Format (emitted by the capture step, retained under evidence/28-01/):
        ### SURFACE-CAPTURE v1
        ### BINARY sha256=<hex> path=<p> host=<h> profile=<prof>
        ### SOURCE commit=<sha> tree=<sha>
        ### COMMAND --help
        <verbatim stdout+stderr>
        ### COMMAND help <verb>
        ...
        ### END-CAPTURE
    """
    if not text.startswith("### SURFACE-CAPTURE v1"):
        raise Refusal("surface capture is not a v1 capture (missing banner)")
    if "### END-CAPTURE" not in text:
        raise Refusal(
            "surface capture has no END-CAPTURE marker — it was truncated, and a "
            "truncated capture would silently under-report the surface inventory"
        )

    binary: dict[str, str] = {}
    source: dict[str, str] = {}
    blocks: dict[str, list[str]] = {}
    current: str | None = None

    for line in text.splitlines():
        if line.startswith("### BINARY "):
            binary = dict(
                kv.split("=", 1) for kv in line[len("### BINARY ") :].split() if "=" in kv
            )
            continue
        if line.startswith("### SOURCE "):
            source = dict(
                kv.split("=", 1) for kv in line[len("### SOURCE ") :].split() if "=" in kv
            )
            continue
        if line.startswith("### COMMAND "):
            current = line[len("### COMMAND ") :].strip()
            blocks[current] = []
            continue
        if line.startswith("### END-CAPTURE"):
            current = None
            continue
        if current is not None:
            blocks[current].append(line)

    for field in ("sha256", "path", "host", "profile"):
        if field not in binary:
            raise Refusal(f"surface capture BINARY line is missing {field}=")
    for field in ("commit", "tree"):
        if field not in source:
            raise Refusal(f"surface capture SOURCE line is missing {field}=")
    if "--help" not in blocks:
        raise Refusal("surface capture has no `--help` block")

    return {"binary": binary, "source": source, "blocks": blocks}


def _commands_in(block: list[str]) -> list[tuple[str, str]]:
    """Extract (verb, description) from a clap `Commands:` section."""
    out: list[tuple[str, str]] = []
    inside = False
    for line in block:
        if line.startswith("Commands:"):
            inside = True
            continue
        if inside:
            if line and not line.startswith(" "):
                break
            m = re.match(r"^  ([a-z][a-z0-9-]*)\s\s+(.*)$", line)
            if m:
                out.append((m.group(1), m.group(2).strip()))
    return out


def resolve_surfaces(capture: dict) -> list[dict]:
    """One record per discovered surface, with a stable id and the binary's own text.

    Deterministic: sorted by id. No feature name is read out of any planning document.
    """
    surfaces: dict[str, dict] = {}
    for verb, desc in _commands_in(capture["blocks"]["--help"]):
        if verb == "help":
            continue
        surfaces[f"cmd:{verb}"] = {
            "id": f"cmd:{verb}",
            "entrypoint": f"wayland-core {verb}",
            "depth": 1,
            "evidence": desc[:240],
        }
        sub_block = capture["blocks"].get(f"help {verb}")
        if sub_block is None:
            continue
        for sub, sub_desc in _commands_in(sub_block):
            if sub == "help":
                continue
            sid = f"cmd:{verb}/{sub}"
            surfaces[sid] = {
                "id": sid,
                "entrypoint": f"wayland-core {verb} {sub}",
                "depth": 2,
                "evidence": sub_desc[:240],
            }
    if not surfaces:
        raise Refusal(
            "the binary's command tree yielded no surfaces; a self-description complete "
            "enough to enumerate user-reachable surfaces is a precondition of this phase"
        )
    return [surfaces[k] for k in sorted(surfaces)]


# --------------------------------------------------------------------------------------
# Phase artifacts — attribution and mismatch only
# --------------------------------------------------------------------------------------


def _code_spans(text: str) -> list[str]:
    """Every backtick span and fenced-code line, individually.

    Prose mentioning a verb is not a claim. A phase artifact that claims a CLI surface
    writes it as code, and restricting extraction to code spans is what keeps the
    finding classes from filling with narrative noise.
    """
    spans: list[str] = []
    for block in re.findall(r"```.*?```", text, re.S):
        spans.extend(line.strip() for line in block.splitlines())
    spans.extend(s.strip("`").strip() for s in re.findall(r"`[^`\n]+`", text))
    return spans


def extract_claims_explicit(text: str) -> set[str]:
    """HIGH-PRECISION claims: `wayland-core <verb>` inside a code span.

    Only this form is allowed to assert claimed-but-absent, because that class accuses a
    phase of claiming a surface that does not exist and a false accusation there costs a
    phase disposition to clear.
    """
    claimed: set[str] = set()
    for span in _code_spans(text):
        for verb in CLAIM_EXPLICIT.findall(span):
            # EXTERNAL_TOOLS is deliberately NOT applied here. The literal `wayland-core `
            # prefix already disambiguates, and applying the stoplist would drop real
            # verbs that collide with an external tool name — `wayland-core node` is the
            # product's verb, not the Node.js runtime.
            if verb not in NOT_A_VERB:
                claimed.add(verb)
    return claimed


def extract_claims_bare(text: str) -> set[str]:
    """HIGH-RECALL claims: a code span that is ENTIRELY `<verb> <sub>...`.

    Used ONLY to attribute a surface already known present. It is deliberately NOT allowed
    to assert claimed-but-absent: this form also matches prose in backticks and Rust
    fragments, and using a noisy instrument to accuse is how a measurement artifact becomes
    a finding. Precision where we accuse, recall where we attribute.
    """
    claimed: set[str] = set()
    for span in _code_spans(text):
        m = CLAIM_BARE.match(span)
        if not m:
            continue
        tokens = span.split()
        if len(tokens) > 4:
            continue
        if any(t in ENGLISH_NOISE for t in tokens):
            continue
        verb = m.group(1)
        if verb not in NOT_A_VERB and verb not in EXTERNAL_TOOLS:
            claimed.add(verb)
    return claimed


def mentions(text: str, verb: str) -> bool:
    """Whether the verb appears anywhere in the artifact as a standalone word.

    Used ONLY to separate "this phase never mentions the surface at all" from "this phase
    discusses the surface but not in a form this extractor recognises as a claim". The
    second is a limitation of the measurement and must not be reported as though it were a
    property of the product.
    """
    return re.search(rf"(?<![a-z0-9-]){re.escape(verb)}(?![a-z0-9-])", text) is not None


def collect_phase_artifacts(repo: Path) -> dict[str, dict]:
    """Locate each certified phase's verdict/summary artifacts and its claimed verbs.

    Refuses when any of 24-27 has none, naming which. Termination state 2 in the plan is
    "record exactly which and what the consequence is" — the caller decides whether to
    proceed with `--allow-missing-phase`; the default is refusal.
    """
    phases_dir = repo / ".planning" / "phases"
    out: dict[str, dict] = {}
    missing: list[str] = []
    for phase in CERTIFIED_PHASES:
        matches = sorted(phases_dir.glob(f"{phase}-*"))
        artifacts: list[Path] = []
        for d in matches:
            artifacts.extend(sorted(d.glob("*SUMMARY.md")))
            artifacts.extend(sorted(d.glob("*VERDICT*.md")))
            artifacts.extend(sorted(d.glob("*PHASE-REPORT*.md")))
        artifacts = sorted(set(artifacts))
        if not artifacts:
            missing.append(phase)
            out[phase] = {"phase": phase, "artifacts": [], "claimed_surfaces": []}
            continue
        explicit: set[str] = set()
        bare: set[str] = set()
        texts: list[str] = []
        for a in artifacts:
            body = a.read_text(encoding="utf-8", errors="replace")
            texts.append(body)
            explicit |= extract_claims_explicit(body)
            bare |= extract_claims_bare(body)
        out[phase] = {
            "phase": phase,
            "artifacts": [str(a.relative_to(repo)) for a in artifacts],
            "claimed_surfaces_explicit": sorted(explicit),
            "claimed_surfaces_bare_only": sorted(bare - explicit),
            "_text": "\n".join(texts),
        }
    out["_missing"] = missing  # type: ignore[assignment]
    return out


# --------------------------------------------------------------------------------------
# Targets
# --------------------------------------------------------------------------------------


def resolve_targets(manifest: dict[str, dict]) -> list[dict]:
    """Every target gets a digest or an explicit unbindable entry. Omission is impossible.

    NOTE, stated because the contrary claim has been used to close a leg on Linux alone:
    the arm64 macOS artifact IS obtainable — CI has uploaded it since d9c7683b. "No macOS
    binary is obtainable" is FALSE and this resolver refuses it as an unbindable reason.
    """
    rows: list[dict] = []
    for target, family in TARGETS:
        entry = manifest.get(target)
        if entry is None:
            raise Refusal(
                f"target {target} has neither a digest nor an unbindable entry; an "
                "omission would let an OS family look certified on evidence that never "
                "existed"
            )
        status = entry.get("status")
        if status == "bound":
            digest = entry.get("digest", "")
            if not re.fullmatch(r"[0-9a-f]{64}", digest):
                raise Refusal(f"target {target} is bound but its digest is not a sha256")
            rows.append(
                {
                    "target": target,
                    "os_family": family,
                    "artifact_name": ARTIFACT_NAME.format(target=target),
                    "status": "bound",
                    "digest": digest,
                    "provenance": entry.get("provenance", ""),
                    "reason": "",
                }
            )
        elif status == "unbindable":
            reason = (entry.get("reason") or "").strip()
            if not reason:
                raise Refusal(f"target {target} is unbindable with no reason recorded")
            if re.search(r"no\s+mac\s*os\s+binary\s+is\s+obtainable", reason, re.I) or (
                "obtainable" in reason.lower() and "darwin" in target and "not" in reason.lower()
            ):
                raise Refusal(
                    f"target {target}: the claim that a macOS binary is unobtainable is "
                    "FALSE — CI has uploaded it since d9c7683b — and may not appear "
                    "anywhere in this phase"
                )
            rows.append(
                {
                    "target": target,
                    "os_family": family,
                    "artifact_name": ARTIFACT_NAME.format(target=target),
                    "status": "unbindable",
                    "digest": None,
                    "provenance": entry.get("provenance", ""),
                    "reason": reason,
                }
            )
        else:
            raise Refusal(f"target {target} has unknown status {status!r}")
    return rows


# --------------------------------------------------------------------------------------
# Resolution
# --------------------------------------------------------------------------------------


def build_candidate(
    *,
    repo: Path,
    commit: str,
    tree: str,
    capture_path: Path,
    capture_text: str,
    target_manifest: dict[str, dict],
    provisional_reason: str,
    kr05_repair_present: str,
    allow_missing_phase: bool,
) -> dict:
    if not SHA40.match(commit):
        raise Refusal(
            f"candidate commit {commit!r} is not a full 40-hex sha; a ref can move and a "
            "moved branch must not be able to masquerade as the same candidate"
        )
    if not SHA40.match(tree):
        raise Refusal(f"candidate tree {tree!r} is not a full 40-hex sha")

    capture = parse_surface_capture(capture_text)
    if capture["source"]["commit"] != commit:
        raise Refusal(
            f"the surface capture was taken at commit {capture['source']['commit']} but "
            f"the candidate is {commit}; the inventory would describe a different tree"
        )
    if capture["source"]["tree"] != tree:
        raise Refusal(
            f"the surface capture was taken at tree {capture['source']['tree']} but the "
            f"candidate tree is {tree}"
        )

    phases = collect_phase_artifacts(repo)
    missing = phases.pop("_missing")  # type: ignore[arg-type]
    if missing and not allow_missing_phase:
        raise Refusal(
            "no verdict or summary artifact on disk for phase(s) "
            + ", ".join(missing)
            + "; refusing to emit a candidate rather than certify against a phase whose "
            "landed set is unknown"
        )

    surfaces = resolve_surfaces(capture)
    present_verbs = {s["id"].split(":", 1)[1].split("/")[0] for s in surfaces}

    # Attribution uses BOTH claim forms (recall). Accusation — claimed-but-absent — uses
    # the explicit form ONLY (precision).
    claimed_by: dict[str, list[str]] = {}
    claimed_explicit_by: dict[str, list[str]] = {}
    for phase, rec in sorted(phases.items()):
        for verb in rec["claimed_surfaces_explicit"]:
            claimed_by.setdefault(verb, []).append(phase)
            claimed_explicit_by.setdefault(verb, []).append(phase)
        for verb in rec["claimed_surfaces_bare_only"]:
            claimed_by.setdefault(verb, []).append(phase)

    for s in surfaces:
        verb = s["id"].split(":", 1)[1].split("/")[0]
        s["attributed_phases"] = sorted(claimed_by.get(verb, []))

    findings: list[dict] = []
    n = 0

    for verb in sorted(set(claimed_explicit_by) - present_verbs):
        n += 1
        findings.append(
            {
                "id": f"F-28-01-R{n:03d}",
                "class": "claimed-but-absent",
                "subject": (
                    f"surface `wayland-core {verb}` is claimed by phase artifact(s) "
                    f"{','.join(claimed_explicit_by[verb])} but is absent from the "
                    "candidate binary's command tree"
                ),
                "inherited_severity": "n/a-raised-by-phase-28",
                "p28_severity": "MEDIUM",
                "contradicted_criterion": "-",
                "available_dispositions": "FIXED,DISPROVED,ACCEPTED,DEFERRED",
                "disposition": "OPEN",
                "rationale": (
                    "Maps to the unresolved-surface skip class for its cells. That class "
                    "requires the phase AND that phase's own recorded requirement "
                    "disposition as evidence; neither is supplied by this resolver and "
                    "plan 28-02 must obtain it before any cell for this surface is skipped."
                ),
                "phases": claimed_by[verb],
            }
        )

    # Present but not matched to a claim. Split three ways rather than two, because
    # "this extractor did not recognise a claim form" is a limitation of the MEASUREMENT
    # and must never be reported as though it were a property of the product.
    for verb in sorted(present_verbs - set(claimed_by)):
        mentioning = sorted(
            p for p, rec in phases.items() if mentions(rec["_text"], verb)
        )
        n += 1
        if mentioning:
            findings.append(
                {
                    "id": f"F-28-01-R{n:03d}",
                    "class": "attribution-weak",
                    "subject": (
                        f"surface `wayland-core {verb}` is exposed by the candidate binary "
                        f"and is discussed by phase artifact(s) {','.join(mentioning)}, but "
                        "not in a form this resolver recognises as a claim"
                    ),
                    "inherited_severity": "n/a-raised-by-phase-28",
                    "p28_severity": "LOW",
                    "contradicted_criterion": "-",
                    "available_dispositions": "FIXED,DISPROVED,ACCEPTED,DEFERRED",
                    "disposition": "OPEN",
                    "rationale": (
                        "This is a RESOLVER RECALL limitation, not an established product "
                        "fact. It is recorded rather than silently upgraded to "
                        "present-but-unclaimed, because reporting a measurement the "
                        "instrument could not take as though it were a finding about the "
                        "product is the error this program has paid for before. The "
                        "surface is certified regardless; only its attribution is unproven."
                    ),
                    "phases": mentioning,
                }
            )
        else:
            findings.append(
                {
                    "id": f"F-28-01-R{n:03d}",
                    "class": "present-but-unclaimed",
                    "subject": (
                        f"surface `wayland-core {verb}` is exposed by the candidate binary "
                        "and appears NOWHERE in any phase 24-27 artifact"
                    ),
                    "inherited_severity": "n/a-raised-by-phase-28",
                    "p28_severity": "MEDIUM",
                    "contradicted_criterion": "-",
                    "available_dispositions": "FIXED,DISPROVED,ACCEPTED,DEFERRED",
                    "disposition": "OPEN",
                    "rationale": (
                        "Certified anyway and flagged. An uncertified surface that ships is "
                        "worse than an unattributed one, so this does NOT reduce coverage; "
                        "it records that attribution is incomplete. The surface may predate "
                        "phases 24-27, which is itself worth knowing at certification time."
                    ),
                    "phases": [],
                }
            )

    inputs = [
        {
            "role": "surface_capture",
            "path": str(capture_path.relative_to(repo)),
            "sha256": sha256_bytes(capture_text.encode("utf-8")),
        }
    ]
    for phase, rec in sorted(phases.items()):
        for rel in rec["artifacts"]:
            inputs.append(
                {
                    "role": f"phase_artifact_{phase}",
                    "path": rel,
                    "sha256": sha256_file(repo / rel),
                }
            )

    return {
        "schema": SCHEMA,
        "candidate": {
            "commit": commit,
            "tree": tree,
            "provisional": bool(provisional_reason),
            "provisional_reason": provisional_reason,
            "kr05_lease_wedge_repair_455dd836": kr05_repair_present,
        },
        "surface_probe_binary": {
            "sha256": capture["binary"]["sha256"],
            "path": capture["binary"]["path"],
            "host": capture["binary"]["host"],
            "profile": capture["binary"]["profile"],
        },
        "attribution_method": {
            "claim_forms": [
                "explicit (HIGH PRECISION, used for BOTH attribution and the "
                "claimed-but-absent accusation): `wayland-core <verb> ...` inside a "
                "backtick span or code fence",
                "bare (HIGH RECALL, used for ATTRIBUTION ONLY): a backtick span that is "
                "ENTIRELY `<verb> <sub>...`, at most 4 tokens, no English prose word, "
                "verb not an external tool or code keyword",
            ],
            "known_limitation": (
                "Recall is not complete. A phase may claim a surface in a form neither "
                "pattern recognises. Such a surface is reported as class "
                "`attribution-weak`, NOT as `present-but-unclaimed`, so a limit of the "
                "instrument is never rendered as a fact about the product. Conversely "
                "the bare form is deliberately BARRED from asserting claimed-but-absent: "
                "accusing a phase of claiming a surface that does not exist costs a "
                "disposition to clear, so only the high-precision form may accuse."
            ),
        },
        "targets": resolve_targets(target_manifest),
        "phases": [
            {k: v for k, v in phases[p].items() if not k.startswith("_")}
            for p in CERTIFIED_PHASES
        ],
        "phases_without_artifact": missing,
        "surfaces": surfaces,
        "surface_count": len(surfaces),
        "findings": findings,
        "inputs": inputs,
    }


def canonical(doc: dict) -> str:
    """One canonical serialization. No timestamp anywhere, so re-resolution is byte-equal."""
    return json.dumps(doc, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


# --------------------------------------------------------------------------------------
# Verification
# --------------------------------------------------------------------------------------


def verify(doc: dict, repo: Path) -> list[str]:
    """Re-validate an emitted candidate against the schema and the fail-closed rules."""
    errs: list[str] = []
    if doc.get("schema") != SCHEMA:
        errs.append(f"schema is {doc.get('schema')!r}, expected {SCHEMA!r}")
        return errs

    cand = doc.get("candidate", {})
    if not SHA40.match(cand.get("commit", "")):
        errs.append("candidate.commit is not a full 40-hex sha")
    if not SHA40.match(cand.get("tree", "")):
        errs.append("candidate.tree is not a full 40-hex sha (commit is not bound to a tree)")

    seen = {t["target"] for t in doc.get("targets", [])}
    for target, _ in TARGETS:
        if target not in seen:
            errs.append(f"target {target} is absent; omission is impossible by schema")
    for t in doc.get("targets", []):
        if t["status"] == "bound" and not re.fullmatch(r"[0-9a-f]{64}", t.get("digest") or ""):
            errs.append(f"target {t['target']} is bound without a sha256 digest")
        if t["status"] == "unbindable" and not (t.get("reason") or "").strip():
            errs.append(f"target {t['target']} is unbindable with no reason")
        if t["status"] not in ("bound", "unbindable"):
            errs.append(f"target {t['target']} has unknown status {t['status']!r}")

    if not doc.get("surfaces"):
        errs.append("surface inventory is empty")
    if doc.get("surface_count") != len(doc.get("surfaces", [])):
        errs.append("surface_count disagrees with the surface list")

    for f in doc.get("findings", []):
        if f.get("class") not in (
            "claimed-but-absent",
            "present-but-unclaimed",
            "attribution-weak",
        ):
            errs.append(f"finding {f.get('id')} has unknown class {f.get('class')!r}")
        if not f.get("p28_severity"):
            errs.append(f"finding {f.get('id')} carries no Phase 28 re-score (amendment A1)")

    for entry in doc.get("inputs", []):
        p = repo / entry["path"]
        if not p.is_file():
            errs.append(f"recorded input {entry['path']} is missing")
        elif sha256_file(p) != entry["sha256"]:
            errs.append(
                f"recorded input {entry['path']} has changed since resolution "
                "(sha256 mismatch) — the candidate ledger no longer describes its inputs"
            )
    return errs


def re_resolve(doc: dict, repo: Path) -> str:
    """Rebuild the candidate from the inputs the document itself records."""
    cap_entry = next(i for i in doc["inputs"] if i["role"] == "surface_capture")
    cap_path = repo / cap_entry["path"]
    manifest = {
        t["target"]: {
            "status": t["status"],
            "digest": t.get("digest") or "",
            "reason": t.get("reason") or "",
            "provenance": t.get("provenance") or "",
        }
        for t in doc["targets"]
    }
    return canonical(
        build_candidate(
            repo=repo,
            commit=doc["candidate"]["commit"],
            tree=doc["candidate"]["tree"],
            capture_path=cap_path,
            capture_text=cap_path.read_text(encoding="utf-8"),
            target_manifest=manifest,
            provisional_reason=doc["candidate"]["provisional_reason"],
            kr05_repair_present=doc["candidate"]["kr05_lease_wedge_repair_455dd836"],
            allow_missing_phase=True,
        )
    )


# --------------------------------------------------------------------------------------
# Human rendering — GENERATED from the JSON, never typed
# --------------------------------------------------------------------------------------


def render_ledger(doc: dict) -> str:
    c = doc["candidate"]
    b = doc["surface_probe_binary"]
    lines: list[str] = []
    w = lines.append
    w("# Phase 28 Candidate Ledger")
    w("")
    w("**GENERATED — do not edit.** Produced from `evidence/28-01/candidate.json` by")
    w("`.planning/scripts/f28-resolve-candidate.py --render`. The JSON is authoritative;")
    w("this file is its human rendering, so the two cannot disagree.")
    w("")
    w("## 1. Candidate identity")
    w("")
    w("| Field | Value |")
    w("|---|---|")
    w(f"| commit | `{c['commit']}` |")
    w(f"| tree | `{c['tree']}` |")
    w(f"| provisional | {'YES' if c['provisional'] else 'no'} |")
    if c["provisional_reason"]:
        w(f"| provisional reason | {c['provisional_reason']} |")
    w(f"| KR-05 wedge repair (`455dd836`) present | {c['kr05_lease_wedge_repair_455dd836']} |")
    w("")
    w("A commit alone is not a candidate. Commit and tree are bound together so a moved")
    w("branch cannot masquerade as the same candidate.")
    w("")
    w("## 2. Surface-probe binary")
    w("")
    w("| Field | Value |")
    w("|---|---|")
    w(f"| sha256 | `{b['sha256']}` |")
    w(f"| path | `{b['path']}` |")
    w(f"| host | `{b['host']}` |")
    w(f"| build profile | `{b['profile']}` |")
    w("")
    w("This is the binary whose own command tree produced section 4. It is recorded")
    w("separately from the per-target release artifacts in section 3 and is NOT a")
    w("substitute for them.")
    w("")
    w("## 3. Per-target binaries")
    w("")
    w("Every target carries a digest or an explicit unbindable entry. An omission is")
    w("impossible by schema, so an OS family cannot appear certified on evidence that")
    w("never existed.")
    w("")
    w("| Target | OS family | Artifact | Status | Digest / reason |")
    w("|---|---|---|---|---|")
    for t in doc["targets"]:
        tail = f"`{t['digest']}`" if t["status"] == "bound" else t["reason"]
        w(
            f"| `{t['target']}` | {t['os_family']} | `{t['artifact_name']}` | "
            f"**{t['status']}** | {tail} |"
        )
    w("")
    w("## 4. Surface inventory — read off the binary")
    w("")
    w(f"**{doc['surface_count']} surfaces** discovered by interrogating the binary's own")
    w("command tree. No feature name was read out of a planning document into this list.")
    w("")
    w("| Surface | Entrypoint | Attributed to |")
    w("|---|---|---|")
    for s in doc["surfaces"]:
        att = ", ".join(s["attributed_phases"]) if s["attributed_phases"] else "—"
        w(f"| `{s['id']}` | `{s['entrypoint']}` | {att} |")
    w("")
    w("## 5. Phase attribution")
    w("")
    w("Explicit claims (`wayland-core <verb>`) can assert claimed-but-absent. Bare-only")
    w("claims (`<verb> <sub>`) attribute but never accuse — the form is noisier, and a")
    w("noisy instrument must not be what accuses a phase.")
    w("")
    w("| Phase | Artifacts | Claimed (explicit) | Claimed (bare only) |")
    w("|---|---|---|---|")
    for p in doc["phases"]:
        w(
            f"| {p['phase']} | {len(p['artifacts'])} | "
            f"{', '.join('`' + v + '`' for v in p['claimed_surfaces_explicit']) or '—'} | "
            f"{', '.join('`' + v + '`' for v in p['claimed_surfaces_bare_only']) or '—'} |"
        )
    if doc["phases_without_artifact"]:
        w("")
        w(
            "**Phases with NO verdict or summary artifact: "
            + ", ".join(doc["phases_without_artifact"])
            + "**"
        )
    w("")
    w("## 6. Findings")
    w("")
    if not doc["findings"]:
        w("None.")
    else:
        w("Class `attribution-weak` records a limit of the resolver's claim extractor, NOT")
        w("a property of the product: the surface IS certified, only its attribution is")
        w("unproven.")
        w("")
        w("| id | class | Phase 28 severity | Subject |")
        w("|---|---|---|---|")
        for f in doc["findings"]:
            w(f"| `{f['id']}` | {f['class']} | {f['p28_severity']} | {f['subject']} |")
    w("")
    w("## 7. Recorded inputs")
    w("")
    w("`--verify-reproducible` re-resolves from exactly these and compares bytes.")
    w("")
    w("| Role | Path | sha256 |")
    w("|---|---|---|")
    for i in doc["inputs"]:
        w(f"| {i['role']} | `{i['path']}` | `{i['sha256'][:16]}…` |")
    w("")
    return "\n".join(lines)


# --------------------------------------------------------------------------------------
# Self-test — the fail-closed conditions demonstrated by fixture, not asserted
# --------------------------------------------------------------------------------------

_GOOD_CAPTURE = """### SURFACE-CAPTURE v1
### BINARY sha256=abc path=/x/wayland-core host=h profile=debug
### SOURCE commit={commit} tree={tree}
### COMMAND --help
Usage: wayland-core [OPTIONS]

Commands:
  gateway  the persistent gateway runtime
  help     Print this message

Arguments:
  [PROMPT]...
### COMMAND help gateway
Usage: wayland-core gateway <COMMAND>

Commands:
  status  print status
  help    Print this message

Options:
  -h
### END-CAPTURE
"""

_C = "a" * 40
_T = "b" * 40


def _manifest(**over) -> dict[str, dict]:
    m = {
        t: {"status": "unbindable", "reason": "no release artifact for this commit"}
        for t, _ in TARGETS
    }
    m.update(over)
    return m


def self_test(repo: Path) -> int:
    failures: list[str] = []

    def refuses(name: str, fn) -> None:
        try:
            fn()
        except Refusal as e:
            if not str(e):
                failures.append(f"{name}: refused with an empty reason")
            return
        failures.append(f"{name}: did NOT refuse — the fail-closed condition is not enforced")

    def accepts(name: str, fn):
        try:
            return fn()
        except Refusal as e:
            failures.append(f"{name}: refused a VALID input ({e}) — the rule over-fires")
            return None

    good = _GOOD_CAPTURE.format(commit=_C, tree=_T)

    def build(**over):
        kw = dict(
            repo=repo,
            commit=_C,
            tree=_T,
            capture_path=repo / ".planning" / "_fixture-capture.txt",
            capture_text=good,
            target_manifest=_manifest(),
            provisional_reason="fixture",
            kr05_repair_present="unknown",
            allow_missing_phase=True,
        )
        kw.update(over)
        return build_candidate(**kw)

    # The happy path must work, or every refusal below is vacuous.
    # (inputs are hashed from disk, so hash only the capture here via a temp file)
    fixture = repo / ".planning" / "_fixture-capture.txt"
    fixture.write_text(good, encoding="utf-8")
    try:
        doc = accepts("valid resolution", build)
        if doc is not None:
            ids = [s["id"] for s in doc["surfaces"]]
            if ids != ["cmd:gateway", "cmd:gateway/status"]:
                failures.append(f"surface resolution wrong: {ids}")
            if len(doc["targets"]) != len(TARGETS):
                failures.append("not every target is present in the ledger")

        # --- fail-closed: a ref instead of a pinned sha -------------------------------
        refuses("commit is a ref, not a sha", lambda: build(commit="main"))
        refuses("commit is abbreviated", lambda: build(commit="32e2f57"))
        # --- fail-closed: commit not bound to a tree ----------------------------------
        refuses("tree is not a sha", lambda: build(tree="HEAD^{tree}"))
        # --- fail-closed: capture describes a different tree --------------------------
        refuses(
            "capture taken at a different commit",
            lambda: build(capture_text=_GOOD_CAPTURE.format(commit="c" * 40, tree=_T)),
        )
        refuses(
            "capture taken at a different tree",
            lambda: build(capture_text=_GOOD_CAPTURE.format(commit=_C, tree="d" * 40)),
        )
        # --- fail-closed: truncated capture -------------------------------------------
        refuses(
            "truncated capture (no END marker)",
            lambda: build(capture_text=good.replace("### END-CAPTURE\n", "")),
        )
        refuses(
            "capture with no banner",
            lambda: build(capture_text=good.replace("### SURFACE-CAPTURE v1\n", "")),
        )
        # --- fail-closed: empty command tree ------------------------------------------
        refuses(
            "binary exposes no surfaces",
            lambda: build(
                capture_text=(
                    "### SURFACE-CAPTURE v1\n"
                    "### BINARY sha256=a path=p host=h profile=debug\n"
                    f"### SOURCE commit={_C} tree={_T}\n"
                    "### COMMAND --help\nUsage: x\n### END-CAPTURE\n"
                )
            ),
        )
        # --- fail-closed: a target omitted entirely ------------------------------------
        short = _manifest()
        short.pop("aarch64-apple-darwin")
        refuses("a target omitted from the manifest", lambda: build(target_manifest=short))
        # --- fail-closed: unbindable with no reason -------------------------------------
        refuses(
            "unbindable with no reason",
            lambda: build(
                target_manifest=_manifest(
                    **{"x86_64-apple-darwin": {"status": "unbindable", "reason": "  "}}
                )
            ),
        )
        # --- fail-closed: the prohibited macOS claim -------------------------------------
        refuses(
            "the false 'no macOS binary is obtainable' claim",
            lambda: build(
                target_manifest=_manifest(
                    **{
                        "aarch64-apple-darwin": {
                            "status": "unbindable",
                            "reason": "no macOS binary is obtainable",
                        }
                    }
                )
            ),
        )
        # --- fail-closed: bound without a real digest -------------------------------------
        refuses(
            "bound target with a non-sha256 digest",
            lambda: build(
                target_manifest=_manifest(
                    **{"x86_64-unknown-linux-gnu": {"status": "bound", "digest": "deadbeef"}}
                )
            ),
        )
        accepts(
            "bound target with a real sha256",
            lambda: build(
                target_manifest=_manifest(
                    **{"x86_64-unknown-linux-gnu": {"status": "bound", "digest": "0" * 64}}
                )
            ),
        )
        # --- fail-closed: a phase with no artifact ------------------------------------------
        refuses(
            "a certified phase has no verdict artifact",
            lambda: build_candidate(
                repo=repo / "does-not-exist",
                commit=_C,
                tree=_T,
                capture_path=fixture,
                capture_text=good,
                target_manifest=_manifest(),
                provisional_reason="fixture",
                kr05_repair_present="unknown",
                allow_missing_phase=False,
            ),
        )
        # --- determinism ---------------------------------------------------------------------
        a, b = build(), build()
        if canonical(a) != canonical(b):
            failures.append("two resolutions of the same input are not byte-identical")

        # --- verify() must reject what build() would never emit --------------------------
        doc2 = build()
        bad = json.loads(canonical(doc2))
        bad["candidate"]["tree"] = "not-a-sha"
        if not verify(bad, repo):
            failures.append("verify() accepted a candidate whose commit is not bound to a tree")
        bad2 = json.loads(canonical(doc2))
        bad2["targets"] = [t for t in bad2["targets"] if t["target"] != "aarch64-apple-darwin"]
        if not verify(bad2, repo):
            failures.append("verify() accepted a candidate with a missing target")
        bad3 = json.loads(canonical(doc2))
        for f in bad3["findings"]:
            f["p28_severity"] = ""
        if bad3["findings"] and not verify(bad3, repo):
            failures.append("verify() accepted a finding with no A1 re-score")
        if verify(json.loads(canonical(doc2)), repo):
            failures.append("verify() rejected a candidate this resolver itself emitted")
    finally:
        fixture.unlink(missing_ok=True)

    print(f"self-test: {len(failures)} failure(s)")
    for f in failures:
        print(f"  FAIL {f}")
    if failures:
        return 1
    print("self-test: every fail-closed condition refused a bad fixture and accepted a")
    print("           good one; resolution is deterministic; verify() rejects and accepts.")
    return 0


# --------------------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------------------


def _repo_root(start: Path) -> Path:
    for c in [start, *start.parents]:
        if (c / ".planning").is_dir():
            return c
    return start


def load_manifest(path: Path) -> dict[str, dict]:
    """target<TAB>status<TAB>digest<TAB>provenance<TAB>reason"""
    m: dict[str, dict] = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        f = (raw.split("\t") + ["", "", "", ""])[:5]
        m[f[0].strip()] = {
            "status": f[1].strip(),
            "digest": f[2].strip(),
            "provenance": f[3].strip(),
            "reason": f[4].strip(),
        }
    return m


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--self-test", action="store_true")
    g.add_argument("--resolve", action="store_true")
    g.add_argument("--verify", metavar="candidate.json")
    g.add_argument("--verify-reproducible", metavar="candidate.json")
    g.add_argument("--render", metavar="candidate.json")
    ap.add_argument("--repo")
    ap.add_argument("--commit")
    ap.add_argument("--tree")
    ap.add_argument("--surface-capture")
    ap.add_argument("--target-manifest")
    ap.add_argument("--provisional-reason", default="")
    ap.add_argument("--kr05-repair", default="unknown")
    ap.add_argument("--out")
    ap.add_argument("--render-out")
    ap.add_argument("--allow-missing-phase", action="store_true")
    args = ap.parse_args(argv)

    here = Path(args.repo).resolve() if args.repo else _repo_root(Path.cwd())

    if args.self_test:
        return self_test(here)

    if args.resolve:
        for req in ("commit", "tree", "surface_capture", "target_manifest", "out"):
            if not getattr(args, req):
                print(f"usage: --resolve requires --{req.replace('_', '-')}", file=sys.stderr)
                return 2
        cap = Path(args.surface_capture).resolve()
        try:
            doc = build_candidate(
                repo=here,
                commit=args.commit,
                tree=args.tree,
                capture_path=cap,
                capture_text=cap.read_text(encoding="utf-8"),
                target_manifest=load_manifest(Path(args.target_manifest)),
                provisional_reason=args.provisional_reason,
                kr05_repair_present=args.kr05_repair,
                allow_missing_phase=args.allow_missing_phase,
            )
        except Refusal as e:
            print(f"REFUSED (fail-closed): {e}", file=sys.stderr)
            return 1
        out = Path(args.out)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(canonical(doc), encoding="utf-8")
        print(
            f"resolved: {doc['surface_count']} surfaces, "
            f"{sum(1 for t in doc['targets'] if t['status'] == 'bound')}/"
            f"{len(doc['targets'])} targets bound, {len(doc['findings'])} finding(s)"
        )
        if args.render_out:
            Path(args.render_out).write_text(render_ledger(doc), encoding="utf-8")
            print(f"rendered: {args.render_out}")
        return 0

    path = Path(args.verify or args.verify_reproducible or args.render)
    if not path.is_file():
        print(f"input not found: {path}", file=sys.stderr)
        return 2
    raw = path.read_text(encoding="utf-8")
    doc = json.loads(raw)

    if args.verify:
        errs = verify(doc, here)
        if errs:
            print(f"--verify: REJECTED ({len(errs)})")
            for e in errs:
                print(f"  {e}")
            return 1
        print(
            f"--verify: OK — commit bound to tree, {len(doc['targets'])} targets each with "
            f"a digest or a reason, {doc['surface_count']} surfaces, "
            f"{len(doc['findings'])} finding(s), {len(doc['inputs'])} input(s) unchanged"
        )
        return 0

    if args.verify_reproducible:
        errs = verify(doc, here)
        if errs:
            print(f"--verify-reproducible: input REJECTED before re-resolution ({len(errs)})")
            for e in errs:
                print(f"  {e}")
            return 1
        try:
            again = re_resolve(doc, here)
        except Refusal as e:
            print(f"--verify-reproducible: re-resolution REFUSED: {e}")
            return 1
        if again != raw:
            print("--verify-reproducible: NOT REPRODUCIBLE — re-resolution differs")
            import difflib

            for line in list(
                difflib.unified_diff(raw.splitlines(), again.splitlines(), "recorded", "re-resolved")
            )[:40]:
                print(f"  {line}")
            return 1
        print(
            "--verify-reproducible: OK — re-resolving from the recorded inputs reproduced "
            f"the file byte for byte ({len(raw)} bytes)"
        )
        return 0

    text = render_ledger(doc)
    if args.render_out:
        Path(args.render_out).write_text(text, encoding="utf-8")
        print(f"rendered: {args.render_out}")
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
