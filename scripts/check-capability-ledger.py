#!/usr/bin/env python3
"""Validate W15 evidence bookkeeping, never substitute for execution."""
import argparse
import copy
import json
import re
from pathlib import Path

REQUIRED = {
    "cli-provider-tools", "acp-http-rest", "channel-ingress", "credentials",
    "daily-auxiliary-budget", "mcp-remote-tools", "browser-boundary", "cua-boundary",
    "plugin-wasm-subprocess", "fleet-swarm", "remote-execution", "workflow-runner",
    "manual-skill-governance", "retention-events", "per-turn-strategy-downgrade",
    "network-a2a", "online-system-prompt-evolution", "behavioral-skill-promotion",
}
STATES = {"verified", "explicitly_unsupported", "externally_blocked"}

def validate(data, source_sha=None):
    errors = []
    rows = data.get("rows", [])
    ids = [row.get("id") for row in rows]
    if data.get("schema") != 1 or len(ids) != len(set(ids)) or set(ids) != REQUIRED:
        errors.append("invalid schema, duplicate IDs, or missing/unexpected journey")
    for row in rows:
        label = row.get("id", "?")
        if row.get("state") not in STATES or row.get("risk") not in {"critical", "high", "medium"}:
            errors.append(f"{label}: invalid state/risk")
        for key in ("journey", "owner", "reason", "carrier"):
            if not isinstance(row.get(key), str) or not row[key].strip():
                errors.append(f"{label}: missing {key}")
        if row.get("state") != "verified":
            continue
        receipts = row.get("evidence", [])
        if not receipts:
            errors.append(f"{label}: verified needs behavioral receipt")
        for receipt in receipts:
            required = ("source_sha", "platform", "command", "selected", "result", "negative_control", "receipt")
            if not isinstance(receipt, dict) or any(not receipt.get(k) for k in required):
                errors.append(f"{label}: incomplete behavioral receipt")
                continue
            if not re.fullmatch(r"[0-9a-f]{40}", receipt["source_sha"]):
                errors.append(f"{label}: source must be full SHA")
            if not isinstance(receipt["selected"], list) or not all(isinstance(x, str) and x for x in receipt["selected"]):
                errors.append(f"{label}: selected must list executed IDs")
            if receipt["result"] != "passed" or receipt.get("exit_code") != 0:
                errors.append(f"{label}: nonpassing result")
            if receipt.get("kind") != "executed_behavior":
                errors.append(f"{label}: presence is not behavior")
            if source_sha is None or receipt["source_sha"] != source_sha:
                errors.append(f"{label}: current source binding required")
    return errors

def self_test():
    data = {"schema": 1, "rows": [dict(id=k, risk="high", state="externally_blocked",
        journey=k, owner="W15", reason="execution prerequisite missing", carrier="issue", evidence=[])
        for k in sorted(REQUIRED)]}
    assert not validate(data)
    missing = copy.deepcopy(data); missing["rows"].pop()
    assert validate(missing)
    row = data["rows"][0]; row["state"] = "verified"
    assert validate(data)
    row["evidence"] = [dict(source_sha="a" * 40, platform="linux", command="actual test",
        selected=["sentinel"], result="passed", negative_control="wrong output fails",
        receipt="source-bound execution receipt", exit_code=0, kind="executed_behavior")]
    assert not validate(data, "a" * 40)
    assert validate(data, "b" * 40)
    row["evidence"][0]["kind"] = "presence"
    assert validate(data, "a" * 40)
    print("capability ledger self-test passed; bookkeeping only")

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--source-sha")
    parser.add_argument("--ledger", type=Path, default=Path(__file__).resolve().parents[1] / "docs/stabilization-capability-ledger.json")
    args = parser.parse_args()
    if args.self_test:
        self_test(); return
    errors = validate(json.loads(args.ledger.read_text()), args.source_sha)
    if errors:
        raise SystemExit("\n".join(errors))
    print("capability ledger valid; dispositions recorded, no runtime readiness inferred")

if __name__ == "__main__":
    main()
