#!/usr/bin/env python3
"""Join authenticated CI F01 and same-release producer artifacts without relabeling."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil

ROOT = Path(os.environ.get("RELEASE_SOURCE_ROOT", Path(__file__).resolve().parents[2]))
spec = importlib.util.spec_from_file_location("release_gate", ROOT / "scripts/stabilization_release_gate.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


def merge(sources, output, sha, repository, run_id, ci_run_id, artifacts, tag, mutants_run_id=None):
    output.mkdir(parents=True, exist_ok=False)
    result = {"schema": 1, "source_sha": sha, "tag": tag, "tests": [], "mutants": [], "native": [],
              "deferred_risks": [], "provenance": []}
    for label, root, kind, job, origin_run in sources:
        index = json.loads((root / "index.json").read_text())
        gate.require(index.get("source_sha") == sha, f"{label}: stale source")
        gate.require(not index.get("blockers"), f"{label}: producer blocked")
        rows = index.get(kind)
        gate.require(isinstance(rows, list) and rows, f"{label}: missing {kind}")
        if label != "ci":
            origin = index.get("provenance", {})
            expected_run = mutants_run_id or run_id if label == "mutants" else run_id
            gate.require(origin.get("source_sha") == sha and origin.get("repository") == repository
                         and str(origin.get("run_id")) == str(expected_run)
                         and str(origin_run) == str(expected_run) and origin.get("job") == job,
                         f"{label}: producer provenance mismatch")
        def rebase(value):
            if isinstance(value, dict):
                if set(value) == {"path", "sha256"}:
                    gate.read_ref(root, value)
                    return {"path": f"{label}/{value['path']}", "sha256": value["sha256"]}
                return {key: rebase(item) for key, item in value.items()}
            return [rebase(item) for item in value] if isinstance(value, list) else value
        result[kind].extend(rebase(rows))
        gate.require(not any(path.is_symlink() for path in root.rglob("*")), "symlink in producer artifact")
        shutil.copytree(root, output / label)
        result["provenance"].append({"artifact": label, "repository": repository,
                                     "run_id": str(origin_run), "job": job, "source_sha": sha})
        if label == "ci":
            result["deferred_risks"] = index.get("deferred_risks", [])
    index_path = output / "index.json"
    index_path.write_text(json.dumps(result, indent=2) + "\n")
    # Reuse the original validator before changing even the stale pending prose.
    # This binds all four native rows to the exact private ZIP and source.
    receipt = gate.produce(index_path, artifacts, sha, tag, output / "collection-admission.json")
    gate.require(receipt["admission"] == "accepted", "; ".join(receipt["blockers"]))
    gate.require(all(row.get("outcome") == "passed" and row.get("exit_code") == 0
                     for row in receipt["native"]), "current release requires passing native checks, not legacy quarantine")
    for risk in result["deferred_risks"]:
        if risk.get("id") == "core#368":
            risk["disposition"] = "ACL repair verified by this candidate's native ACL and lease checks; prior ACL nonpass retained"
    index_path.write_text(json.dumps(result, indent=2) + "\n")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ci", "mutants", "native", "output", "artifacts"):
        parser.add_argument("--" + name, type=Path, required=True)
    for name in ("sha", "tag", "repository", "run-id", "ci-run-id"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--mutants-run-id", help="Authenticated original mutation producer run; preserve provenance")
    args = parser.parse_args()
    merge([("ci", args.ci, "tests", "ci-linux", args.ci_run_id),
           ("mutants", args.mutants, "mutants", "collect-release-mutants", args.mutants_run_id or args.run_id),
           ("native", args.native, "native", "collect-release-native", args.run_id)],
          args.output, args.sha, args.repository, args.run_id, args.ci_run_id, args.artifacts, args.tag, args.mutants_run_id)
