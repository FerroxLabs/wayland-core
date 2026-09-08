#!/usr/bin/env python3
"""Delegate this publication PR's duplicate CI to exact-head integration push CI.

The push still runs every original build/test job. This PR gate checks the
actual completed producer jobs and artifacts, plus the frozen base/head/merge
content relationship. It never executes tests or creates replacement statuses.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import time

PUBLICATION_HEAD = "stabilize/20260905-core"
REQUIRED_JOBS = {
    "CI (linux-containerized)", "CI (macos-latest)", "CI (Array)",
    "Eval acceptance gate (Linux, containerized)", "report",
    *{f"Build ({target})" for target in (
        "aarch64-apple-darwin", "x86_64-apple-darwin",
        "aarch64-pc-windows-msvc", "x86_64-pc-windows-msvc",
        "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu")},
}


def require(ok, reason):
    if not ok:
        raise ValueError(reason)


def checked(argv):
    result = subprocess.run(argv, capture_output=True, text=True, timeout=60)
    require(result.returncode == 0, f"proof command failed: {argv[0]} (exit {result.returncode})")
    return result.stdout.strip()


def api(path):
    return json.loads(checked(["gh", "api", path]))


def validate_pr(pr, repository, head, base):
    require(pr["head"]["repo"]["full_name"] == repository
            and pr["head"]["ref"] == PUBLICATION_HEAD, "not the controlled publication PR")
    require(pr["state"] == "open" and pr["head"]["sha"] == head
            and pr["base"]["ref"] == "main" and pr["base"]["sha"] == base,
            "publication PR head/base changed; qualify the new candidate")


def validate_run(run, jobs, artifacts, repository, head):
    require(run["repository"]["full_name"] == repository
            and run["head_repository"]["full_name"] == repository, "foreign push producer")
    require(run["event"] == "push" and run["head_sha"] == head
            and run["head_branch"].startswith("integ/release-"), "wrong publication push source")
    require(run["status"] == "completed" and run["conclusion"] == "success", "push qualification did not succeed")
    grouped = {}
    for job in jobs:
        grouped.setdefault(job["name"], []).append(job)
    for name in REQUIRED_JOBS:
        rows = grouped.get(name, [])
        require(len(rows) == 1 and rows[0]["status"] == "completed" and rows[0]["conclusion"] == "success",
                f"required push producer did not succeed: {name}")
    expected = {"nextest-junit-linux-containerized", "nextest-junit-macos-latest", "nextest-junit-Array",
                "stabilization-raw-" + head}
    for name in expected:
        rows = [artifact for artifact in artifacts if artifact["name"] == name]
        require(len(rows) == 1 and not rows[0]["expired"]
                and rows[0]["workflow_run"]["id"] == run["id"]
                and rows[0]["workflow_run"]["head_sha"] == head,
                "missing or foreign qualification artifact: " + name)


def main():
    repository = os.environ["GITHUB_REPOSITORY"]
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    pr = event["pull_request"]
    head, base = pr["head"]["sha"], pr["base"]["sha"]
    require(all(re.fullmatch(r"[0-9a-f]{40}", value) for value in (head, base)), "invalid source identity")
    validate_pr(pr, repository, head, base)
    merge = checked(["git", "rev-parse", "HEAD"])
    require(checked(["git", "rev-parse", "HEAD^{tree}"]) == checked(["git", "rev-parse", head + "^{tree}"]),
            "PR merge content differs from push candidate")
    checked(["git", "merge-base", "--is-ancestor", base, head])
    output = Path(os.environ["RUNNER_TEMP"]) / "publication-push-proof.json"
    # This observes one actual source's run; it never launches or retries CI.
    deadline = time.monotonic() + 270 * 60
    while time.monotonic() < deadline:
        validate_pr(api(f"repos/{repository}/pulls/{pr['number']}"), repository, head, base)
        runs = api(f"repos/{repository}/actions/workflows/ci.yml/runs?event=push&head_sha={head}&per_page=100")["workflow_runs"]
        runs = [run for run in runs if run["head_branch"].startswith("integ/release-") and run["head_sha"] == head]
        if runs:
            run = max(runs, key=lambda row: row["id"])
            if run["status"] == "completed":
                jobs = api(f"repos/{repository}/actions/runs/{run['id']}/attempts/{run['run_attempt']}/jobs?per_page=100")["jobs"]
                artifacts = api(f"repos/{repository}/actions/runs/{run['id']}/artifacts?per_page=100")["artifacts"]
                record = {"repository": repository, "head_sha": head, "base_sha": base,
                          "pr_merge_sha": merge, "push_run": run, "jobs": jobs, "artifacts": artifacts,
                          "accepted": False}
                output.write_text(json.dumps(record, indent=2) + "\n")
                validate_run(run, jobs, artifacts, repository, head)
                # Recheck the base after waiting; a green old integration is not
                # authority for a new main tree.
                validate_pr(api(f"repos/{repository}/pulls/{pr['number']}"), repository, head, base)
                record["accepted"] = True
                output.write_text(json.dumps(record, indent=2) + "\n")
                with open(os.environ["GITHUB_OUTPUT"], "a") as stream:
                    stream.write(f"run_id={run['id']}\n")
                with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as stream:
                    stream.write(f"Full qualification ran once on push {run['id']} at `{head}`. "
                                 f"PR merge `{merge}` has identical content and includes base `{base}`. "
                                 "The PR report consumes the original push JUnit; no tests were relabelled.\n")
                return
        print(f"Waiting for exact-head integration push qualification: {head}", flush=True)
        time.sleep(60)
    raise ValueError("push qualification observation deadline; inspect existing jobs, do not restart them")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        path = Path(os.environ["RUNNER_TEMP"]) / "publication-push-proof.json"
        record = json.loads(path.read_text()) if path.exists() else {}
        record.update(accepted=False, error=str(error))
        path.write_text(json.dumps(record, indent=2) + "\n")
        raise
