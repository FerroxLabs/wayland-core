#!/usr/bin/env python3
"""Plan conservative affected checks and report source-bound qualification state.

Consumes trusted, explicitly scoped JSON; does not execute checks or publish.
Existing release/security gates remain authoritative required checks.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys


class Refused(Exception):
    pass


def require(condition, reason):
    if not condition:
        raise Refused(reason)


def git(repo, *args):
    result = subprocess.run(["git", "-C", str(repo), *args], capture_output=True)
    require(result.returncode == 0, "required Git source or input is unavailable")
    return result.stdout


def sha(value):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{40}", value), "full source SHA required")
    return value


def relative(value):
    require(isinstance(value, str) and value and "\\" not in value, "repository-relative input required")
    path = PurePosixPath(value)
    require(not path.is_absolute() and ".." not in path.parts and str(path) == value,
            "input must be an explicit normalized repository-relative path")
    return value


def manifest_checks(manifest):
    require(manifest.get("schema") == 1, "unsupported qualification manifest schema")
    sha(manifest.get("candidate_sha"))
    if manifest.get("pr_head_sha") is not None:
        sha(manifest["pr_head_sha"])
    checks = manifest.get("checks")
    require(isinstance(checks, list) and checks, "required check manifest is empty")
    result = {}
    for check in checks:
        require(isinstance(check, dict), "malformed required check")
        key = check.get("id")
        require(isinstance(key, str) and key and key not in result, "missing or duplicate required check ID")
        require(check.get("stage") in ("fast", "build", "affected", "release"), "required check stage is missing")
        require(isinstance(check.get("owner"), str) and check["owner"], "required check owner is missing")
        require(check.get("kind") in ("test", "build", "gate"), "required check kind is missing")
        require(isinstance(check.get("command"), list) and check["command"]
                and all(isinstance(arg, str) and arg for arg in check["command"]), "explicit command argv required")
        require(isinstance(check.get("toolchain"), dict) and check["toolchain"], "explicit toolchain identity required")
        require(isinstance(check.get("features"), list), "explicit features list required")
        require(isinstance(check.get("target"), str) and check["target"], "explicit target required")
        require(isinstance(check.get("runtime_contract"), dict) and check["runtime_contract"], "explicit runtime contract required")
        require(isinstance(check.get("inputs_complete"), bool), "declare whether the input closure is complete")
        inputs = check.get("inputs")
        require(isinstance(inputs, list) and len(set(inputs)) == len(inputs), "explicit unique input list required")
        for item in inputs:
            relative(item)
        require(not check["inputs_complete"] or inputs, "a complete input closure cannot be empty")
        result[key] = check
    return result


def cargo_targets(metadata, candidate):
    require(metadata.get("source_sha") == candidate, "Cargo metadata is not bound to the candidate")
    cargo = metadata.get("cargo", {})
    root = cargo.get("workspace_root", "").replace("\\", "/").rstrip("/")
    require(root and isinstance(cargo.get("workspace_members"), list), "workspace metadata is incomplete")
    targets = {}
    for package in cargo.get("packages", []):
        if package.get("id") not in cargo["workspace_members"]:
            continue
        for target in package.get("targets", []):
            if target.get("kind") != ["test"]:
                continue
            source = target.get("src_path", "").replace("\\", "/")
            require(source.startswith(root + "/"), "Cargo test target is outside the workspace")
            source = relative(source[len(root) + 1:])
            key = f"{package['name']}::test::{target['name']}"
            require(key not in targets, "Cargo target identity is ambiguous")
            targets[key] = {"package": package["name"], "kind": "test", "name": target["name"],
                            "source": source, "cargo_selectors": ["-p", package["name"], "--test", target["name"]]}
    return targets


def plan(repo, base, manifest, metadata):
    checks = manifest_checks(manifest)
    candidate = manifest["candidate_sha"]
    base = sha(base)
    git(repo, "cat-file", "-e", candidate + "^{commit}")
    targets = cargo_targets(metadata, candidate)
    changed = [path.decode() for path in git(repo, "diff", "--no-renames", "--name-only", "-z", base, candidate).split(b"\0") if path]
    owners = manifest.get("fixture_owners", {})
    require(isinstance(owners, dict), "fixture ownership must be an explicit map")
    affected = set()
    reasons = []
    for path in changed:
        parts = PurePosixPath(path).parts
        if "tests" not in parts or any(part in ("support", "common", "helpers", "shared") for part in parts):
            reasons.append({"path": path, "reason": "production, shared helper, configuration, or unknown input"})
            continue
        direct = [key for key, target in targets.items() if target["source"] == path]
        declared = owners.get(path, [])
        require(isinstance(declared, list), "fixture owner entries must be target-ID lists")
        matches = set(direct) | set(declared)
        if len(matches) != 1 or not matches <= targets.keys():
            reasons.append({"path": path, "reason": "fixture does not map to exactly one Cargo integration-test target"})
            continue
        target = next(iter(matches))
        if not any(target in check.get("cargo_targets", []) for check in checks.values()):
            reasons.append({"path": path, "reason": "no required affected check names this Cargo target"})
            continue
        affected.add(target)
    mode = "full" if reasons else "affected" if changed else "unchanged"
    next_checks = list(checks) if reasons else [key for key, check in checks.items()
                                                if affected.intersection(check.get("cargo_targets", []))]
    return {"schema": 1, "base_sha": base, "candidate_sha": candidate, "mode": mode,
            "disposition": "full" if reasons else "affected", "requires_verified_baseline": True,
            "changed_paths": changed, "affected_targets": {key: targets[key] for key in sorted(affected)},
            "next_checks": next_checks, "required_checks": list(checks), "reasons": reasons,
            "commands": [{"id": key, "argv": checks[key]["command"]} for key in next_checks],
            "limitation": "affected mode never removes required release checks; reuse needs a separate receipt decision"}


def input_digests(repo, source, paths):
    digests = {}
    for path in paths:
        relative(path)
        listing = git(repo, "ls-tree", source, "--", path).decode()
        require(listing.startswith(("100644 blob ", "100755 blob ")), "declared input is absent or not a regular tracked file")
        digests[path] = hashlib.sha256(git(repo, "show", f"{source}:{path}")).hexdigest()
    return digests


def assess_receipt(repo, candidate, check, receipt):
    require(receipt.get("complete") is True and receipt.get("status") == "passed", "receipt is incomplete or nonpass")
    require(type(receipt.get("exit_code")) is int and receipt["exit_code"] == 0, "receipt exit status is not a measured zero")
    require(receipt.get("flaky") is False and type(receipt.get("attempts")) is int
            and receipt["attempts"] == 1, "failed/retried/flaky evidence cannot pass")
    source = sha(receipt.get("source_sha"))
    git(repo, "cat-file", "-e", source + "^{commit}")
    for field in ("command", "toolchain", "features", "target", "runtime_contract"):
        require(receipt.get(field) == check[field], f"receipt {field} identity mismatch")
    if check["kind"] == "test":
        require(type(receipt.get("selected")) is int and receipt["selected"] > 0,
                "test receipt selected no cases")
        require(type(receipt.get("passed")) is int and receipt["passed"] == receipt["selected"],
                "not all selected cases passed")
        require(all(type(receipt.get(key)) is int and receipt[key] == 0
                    for key in ("failed", "skipped", "retries")), "selected tests failed, skipped, or retried")
    if source != candidate:
        require(check["inputs_complete"] and check["inputs"], "reuse has no declared complete input closure")
        expected = input_digests(repo, candidate, check["inputs"])
        require(receipt.get("input_sha256") == expected, "candidate input digests differ or closure is missing")
        require(input_digests(repo, source, check["inputs"]) == expected,
                "receipt input digests do not describe its original source")
    elif check["inputs_complete"]:
        require(receipt.get("input_sha256") == input_digests(repo, candidate, check["inputs"]),
                "current-source input digests are missing or mismatched")
    return {"status": "passed", "mode": "current" if source == candidate else "reused",
            "receipt_source_sha": source, "candidate_sha": candidate}


def readiness(repo, manifest, state, receipts):
    checks = manifest_checks(manifest)
    candidate = manifest["candidate_sha"]
    git(repo, "cat-file", "-e", candidate + "^{commit}")
    require(isinstance(receipts, list), "receipts must be an explicit list")
    grouped = {}
    for receipt in receipts:
        require(isinstance(receipt, dict) and isinstance(receipt.get("id"), str), "receipt ID missing")
        grouped.setdefault(receipt["id"], []).append(receipt)
    rows = []
    for key, check in checks.items():
        row = {"id": key, "stage": check["stage"], "owner": check["owner"]}
        found = grouped.get(key, [])
        if not found:
            row.update(status="pending", reason="required receipt missing")
        elif len(found) != 1:
            row.update(status="blocked", reason="multiple receipts require an explicit evidence selection")
        else:
            try:
                row.update(assess_receipt(repo, candidate, check, found[0]))
            except Refused as error:
                row.update(status="blocked", reason=str(error), receipt_source_sha=found[0].get("source_sha"))
        rows.append(row)
    checkout = state.get("checkout_sha")
    head = state.get("pr_head_sha")
    expected_head = manifest.get("pr_head_sha")
    identity_ok = checkout == candidate and state.get("worktree_clean") is True and head == expected_head
    identity_reason = ("checkout and PR head match their separately declared identities" if identity_ok
                       else "checkout/clean-state/latest PR head does not match the declared candidate")
    builds = [row for row in rows if row["stage"] == "build"]
    built = identity_ok and bool(builds) and all(row["status"] == "passed" for row in builds)
    verified = (identity_ok and built and any(row["stage"] in ("affected", "release") for row in rows)
                and all(row["status"] == "passed" for row in rows))
    merge = state.get("merge") or {}
    merged = bool(merge.get("merged") is True and merge.get("merged_at")
                  and merge.get("head_sha") == (expected_head or candidate)
                  and re.fullmatch(r"[0-9a-f]{40}", str(merge.get("merge_commit_sha", ""))))
    release = state.get("release") or {}
    published = bool(type(release.get("id")) is int and release["id"] > 0
                     and release.get("draft") is False and release.get("published_at")
                     and release.get("html_url") and manifest.get("release_tag")
                     and release.get("tag_name") == manifest["release_tag"]
                     and release.get("resolved_source_sha") == candidate)
    phases = {
        "committed": {"passed": identity_ok, "detail": identity_reason},
        "built": {"passed": built, "detail": "all declared build checks passed" if built else "matching required build evidence is pending or blocked"},
        "verified": {"passed": verified, "detail": "all required checks passed for this candidate" if verified else "required qualification or current identity is pending or blocked"},
        "merged": {"passed": merged, "detail": "provided PR merge record matches the declared head" if merged else "matching actual PR merge record missing"},
        "published": {"passed": published, "detail": "actual published release resolves to candidate source" if published else "matching published release/source evidence missing; tag existence is insufficient"},
    }
    return {"schema": 1, "candidate_sha": candidate, "checkout_sha": checkout,
            "expected_pr_head_sha": expected_head, "observed_pr_head_sha": head,
            "phases": phases, "checks": rows,
            "ready": verified, "complete": all(phase["passed"] for phase in phases.values()),
            "evidence_policy": "provided receipts and merge/release records must be authenticated by their collector; this helper never publishes"}


def markdown(report):
    def cell(value):
        return str(value).replace("|", "\\|").replace("\n", " ")

    lines = [f"Candidate: `{report['candidate_sha']}`",
             f"Actual checkout: `{report.get('checkout_sha')}`; observed PR head: `{report.get('observed_pr_head_sha')}`.",
             "", "| Work item | Status | Progress / pending work |", "|---|---|---|"]
    for name, phase in report["phases"].items():
        status = "✅ Done" if phase["passed"] else "⬜ Pending"
        lines.append(f"| {name} | {status} | {cell(phase['detail'])} |")
    for row in report["checks"]:
        status = "✅ Done" if row["status"] == "passed" else "⬜ Pending"
        detail = row.get("reason") or f"{row['mode']} PASS; original receipt source {row['receipt_source_sha']}"
        lines.append(f"| {cell(row['id'])} | {status} | {cell(detail)} |")
    status = "✅ Done" if report["complete"] else "⬜ Pending"
    lines.append(f"| Overall execution goal | {status} | "
                 + ("All declared phases evidenced." if report["complete"] else "Outstanding phases/checks remain; no publication action taken.") + " |")
    return "\n".join(lines) + "\n"


def load(path):
    return json.loads(Path(path).read_text())


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    for name in ("plan", "status"):
        command = sub.add_parser(name)
        command.add_argument("--repo", type=Path, required=True)
        command.add_argument("--manifest", type=Path, required=True)
        command.add_argument("--output", type=Path, required=True)
        if name == "plan":
            command.add_argument("--base", required=True)
            command.add_argument("--cargo-metadata", type=Path, required=True)
        else:
            command.add_argument("--state", type=Path, required=True)
            command.add_argument("--receipts", type=Path, required=True)
            command.add_argument("--markdown", type=Path)
            command.add_argument("--require-phase", choices=("committed", "built", "verified", "merged", "published"), default="verified")
    args = parser.parse_args(argv)
    try:
        manifest = load(args.manifest)
        if args.action == "plan":
            report = plan(args.repo, args.base, manifest, load(args.cargo_metadata))
            code = 0
        else:
            report = readiness(args.repo, manifest, load(args.state), load(args.receipts))
            if args.markdown:
                args.markdown.write_text(markdown(report))
            passed = report["phases"][args.require_phase]["passed"]
            if args.require_phase in ("verified", "merged", "published"):
                passed = passed and report["ready"]
            code = int(not passed)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({"action": args.action, "output": str(args.output),
                          "mode": report.get("mode"), "ready": report.get("ready")}))
        return code
    except (Refused, OSError, ValueError, TypeError, KeyError, AttributeError) as error:
        reason = str(error) if isinstance(error, Refused) else "malformed or missing input"
        # Never leave a previous green output behind after malformed inputs.
        blocked = {"schema": 1, "ready": False, "complete": False,
                   "disposition": "full", "refused": True, "reason": reason}
        try:
            args.output.write_text(json.dumps(blocked, indent=2) + "\n")
            if getattr(args, "markdown", None):
                args.markdown.write_text("| Work item | Status | Progress / pending work |\n"
                                         "|---|---|---|\n"
                                         "| Overall execution goal | ⬜ Pending | Qualification inputs refused; see JSON reason. |\n")
        except OSError:
            print("qualification refused: unable to replace readiness output", file=sys.stderr)
        print(f"qualification refused: {reason}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
