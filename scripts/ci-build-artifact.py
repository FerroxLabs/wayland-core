#!/usr/bin/env python3
"""Produce or consume an exact-contract binary inside one GitHub Actions run.

This is build reuse, not release admission. Signing, packaging, smoke tests and
release evidence retain their existing owners. An incompatible artifact fails;
there is no default-feature/voice or Linux-ABI alias, and this helper never
rebuilds. One refusal is distinguishable: when the sealed binary is exact in
every field except the hosted runner image, fetch exits IMAGE_ONLY_EXIT so the
calling CI step may rebuild natively from the same checkout. A mismatched
binary is never installed.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
import zipfile

SCHEMA = 1
SIDECAR = "build-identity.json"
MAX_BINARY = 512 * 1024 * 1024
IMAGE_ENV = (
    "RUNNER_OS", "RUNNER_ARCH", "ImageOS", "ImageVersion",
    "MACOSX_DEPLOYMENT_TARGET", "SDKROOT", "VCToolsVersion", "WindowsSDKVersion",
)
# argparse uses 2 and every other refusal uses 1; the capture wrapper reports
# signals as 128+n and launch failures as 126/127, so 3 cannot be ambiguous.
IMAGE_ONLY_EXIT = 3


class ImageOnlyMismatch(ValueError):
    """Every sealed field and the binary digest match except runner_image."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def command(args: list[str], timeout: float = 30) -> str:
    result = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
    require(result.returncode == 0, f"command failed: {args[0]} {args[1]} (exit {result.returncode})")
    return result.stdout.strip()


def api(path: str, timeout: float = 30) -> dict:
    return json.loads(command(["gh", "api", path], timeout=timeout))


def pages(path: str, key: str, timeout: float = 30) -> list[dict]:
    values = json.loads(command(["gh", "api", "--paginate", "--slurp", path], timeout=timeout))
    return [item for page in values for item in page[key]]


def sha256(path: Path) -> str:
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def validate_run(run: dict, repository: str, run_id: str, attempt: str) -> None:
    require(run["repository"]["full_name"] == repository, "foreign producer repository")
    require(str(run["id"]) == run_id, "different workflow run")
    require(str(run["run_attempt"]) == attempt, "different workflow attempt")
    require(run["event"] in {"push", "pull_request", "workflow_dispatch"}, "unsupported producer event")
    require(run.get("conclusion") not in {"failure", "cancelled", "timed_out", "action_required", "startup_failure"}, "failed or cancelled producing run")


def context(args: argparse.Namespace) -> tuple[dict, dict]:
    repository = os.environ["GITHUB_REPOSITORY"]
    run_id = os.environ["GITHUB_RUN_ID"]
    attempt = os.environ["GITHUB_RUN_ATTEMPT"]
    require(re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository) is not None, "invalid repository")
    require(run_id.isdigit() and attempt.isdigit(), "invalid run identity")
    run = api(f"repos/{repository}/actions/runs/{run_id}")
    validate_run(run, repository, run_id, attempt)
    source = command(["git", "rev-parse", "HEAD"])
    require(re.fullmatch(r"[0-9a-f]{40}", source) is not None, "invalid checkout source SHA")
    require(not command(["git", "status", "--porcelain", "--untracked-files=no"]), "dirty tracked build source")
    image = {key: os.environ.get(key, "") for key in IMAGE_ENV}
    require(all(image[key] for key in ("RUNNER_OS", "RUNNER_ARCH", "ImageOS", "ImageVersion")), "hosted runner image identity is missing")
    require(args.profile == "release", "initial artifact reuse supports release profile only")
    require(args.target.endswith(("-apple-darwin", "-pc-windows-msvc")), "initial artifact reuse supports native macOS/Windows contracts only")
    require(bool(args.abi_contract.strip()), "ABI contract is required")
    identity = {
        "schema": SCHEMA,
        "repository": repository,
        "run_id": run_id,
        "run_attempt": attempt,
        # A PR's run head is its head commit; git HEAD is the checked-out merge.
        # Both are recorded, and neither is substituted for the other.
        "event": run["event"],
        "event_head_sha": run["head_sha"],
        "source_sha": source,
        "producer_job": args.producer_job,
        "artifact_name": args.artifact,
        "target": args.target,
        "profile": args.profile,
        "features": sorted(set(value.strip() for value in args.features.split(",") if value.strip())),
        "toolchain": command(["vx", "rustc", "-vV"]),
        "rustflags": os.environ.get("RUSTFLAGS", ""),
        "encoded_rustflags": os.environ.get("CARGO_ENCODED_RUSTFLAGS", ""),
        "abi_contract": args.abi_contract,
        "runner_image": image,
    }
    require(identity["features"], "features must explicitly name default or the selected feature set")
    return identity, run


def validate_identity(record: dict, expected: dict, binary: Path) -> None:
    require(set(record) == set(expected) | {"binary_sha256", "binary_size"}, "unknown or missing build identity fields")
    mismatched = [key for key, value in expected.items() if record.get(key) != value]
    hard = [key for key in mismatched if key != "runner_image"]
    require(not hard, f"build identity mismatch: {', '.join(hard)}")
    require(record["binary_size"] == binary.stat().st_size, "binary size mismatch")
    require(record["binary_sha256"] == sha256(binary), "binary SHA-256 mismatch")
    if mismatched:
        image = lambda value: json.dumps(value, sort_keys=True, separators=(",", ":"))
        raise ImageOnlyMismatch(
            "build identity mismatch: runner_image only "
            f"(producer {image(record['runner_image'])}, consumer {image(expected['runner_image'])})"
        )


def producer_status(jobs: list[dict], name: str) -> bool:
    matching = [job for job in jobs if job["name"] == name]
    require(len(matching) <= 1, "ambiguous named producer")
    if not matching or matching[0]["status"] != "completed":
        return False
    require(matching[0]["conclusion"] == "success", "named build producer did not succeed")
    return True


def select_artifact(artifacts: list[dict], expected: dict) -> dict | None:
    matching = [artifact for artifact in artifacts if artifact["name"] == expected["artifact_name"]]
    require(len(matching) <= 1, "ambiguous artifact name")
    if not matching:
        return None
    artifact = matching[0]
    require(not artifact["expired"], "build artifact expired")
    require(str(artifact["workflow_run"]["id"]) == expected["run_id"], "artifact belongs to another run")
    require(artifact["workflow_run"]["head_sha"] == expected["event_head_sha"], "artifact event head differs from producing run")
    return artifact


def unpack(archive: Path, directory: Path, expected: dict) -> Path:
    name = "wayland-core.exe" if expected["target"].endswith("-windows-msvc") else "wayland-core"
    with zipfile.ZipFile(archive) as files:
        entries = [info for info in files.infolist() if not info.is_dir()]
        require(sorted(info.filename for info in entries) == sorted([name, SIDECAR]), "artifact must contain exactly one binary and its identity sidecar")
        require(files.getinfo(SIDECAR).file_size <= 64 * 1024, "oversized identity sidecar")
        require(0 < files.getinfo(name).file_size <= MAX_BINARY, "invalid binary size")
        record = json.loads(files.read(SIDECAR))
        binary = directory / name
        with files.open(name) as source, binary.open("wb") as destination:
            import shutil
            shutil.copyfileobj(source, destination)
    validate_identity(record, expected, binary)
    binary.chmod(0o755)
    return binary


def validate_build_info(output: str, expected_sha: str) -> None:
    match = re.fullmatch(r"wayland-core ([^\s]+) \(source ([0-9a-f]{40})\)", output.strip())
    require(match is not None and match[2] == expected_sha, "embedded --build-info source differs from checkout")


def produce(args: argparse.Namespace) -> None:
    expected, _ = context(args)
    binary = Path(args.binary)
    require(binary.is_file() and 0 < binary.stat().st_size <= MAX_BINARY, "build binary is missing or invalid")
    if args.verify_build_info:
        validate_build_info(command([str(binary.resolve()), "--build-info"], timeout=90), expected["source_sha"])
    record = dict(expected, binary_sha256=sha256(binary), binary_size=binary.stat().st_size)
    Path(args.output).write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    print(f"Produced build identity: source={expected['source_sha']} target={expected['target']} sha256={record['binary_sha256']}", flush=True)


def fetch(args: argparse.Namespace) -> None:
    expected, _ = context(args)
    require(args.wait_seconds > 0, "wait bound must be positive")
    deadline = time.monotonic() + args.wait_seconds
    base = f"repos/{expected['repository']}/actions/runs/{expected['run_id']}"
    require(f"host: {args.target}" in expected["toolchain"].splitlines(), "consumer target is not native to its Rust toolchain")
    def remaining() -> float:
        seconds = deadline - time.monotonic()
        require(seconds > 0, "matching build producer/artifact did not complete within the wait bound")
        return min(seconds, 30)
    print(f"Waiting for same-run producer {expected['producer_job']} (bound {args.wait_seconds}s)", flush=True)
    while True:
        validate_run(api(base, timeout=remaining()), expected["repository"], expected["run_id"], expected["run_attempt"])
        jobs = pages(f"{base}/attempts/{expected['run_attempt']}/jobs?per_page=100", "jobs", timeout=remaining())
        if producer_status(jobs, expected["producer_job"]):
            artifact = select_artifact(pages(f"{base}/artifacts?per_page=100", "artifacts", timeout=remaining()), expected)
            if artifact is not None:
                break
        require(time.monotonic() < deadline, "matching build producer/artifact did not complete within the wait bound")
        time.sleep(min(5, max(0, deadline - time.monotonic())))
    destination = Path(args.binary)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="ci-build-artifact-", dir=destination.parent) as temporary:
        directory = Path(temporary)
        archive = directory / "artifact.zip"
        with archive.open("wb") as output:
            result = subprocess.run(
                ["gh", "api", f"repos/{expected['repository']}/actions/artifacts/{artifact['id']}/zip"],
                stdout=output, stderr=subprocess.PIPE, timeout=90,
            )
        require(result.returncode == 0, "artifact download failed")
        binary = unpack(archive, directory, expected)
        # Same bounded source probe as the existing eval artifact contract (90s
        # permits first-exec macOS signature validation); never use a debug binary.
        validate_build_info(command([str(binary.resolve()), "--build-info"], timeout=90), expected["source_sha"])
        os.replace(binary, destination)
    print(f"Reused exact build: source={expected['source_sha']} target={expected['target']} sha256={sha256(destination)}", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["produce", "fetch"])
    parser.add_argument("--binary", required=True)
    parser.add_argument("--output")
    parser.add_argument("--target", required=True)
    parser.add_argument("--profile", required=True)
    parser.add_argument("--features", required=True)
    parser.add_argument("--abi-contract", required=True)
    parser.add_argument("--producer-job", required=True)
    parser.add_argument("--artifact", required=True)
    parser.add_argument("--wait-seconds", type=int, default=1800)
    parser.add_argument("--verify-build-info", action="store_true",
                        help="produce: require the binary's embedded source to equal the checkout")
    args = parser.parse_args()
    require(args.action != "produce" or args.output, "produce requires --output")
    try:
        (produce if args.action == "produce" else fetch)(args)
    except ImageOnlyMismatch as error:
        parser.exit(IMAGE_ONLY_EXIT, f"build artifact refused: {error}\n")
    except (ValueError, KeyError, subprocess.SubprocessError, OSError, json.JSONDecodeError, zipfile.BadZipFile) as error:
        parser.exit(1, f"build artifact refused: {error}\n")


if __name__ == "__main__":
    main()
