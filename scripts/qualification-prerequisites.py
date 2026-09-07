#!/usr/bin/env python3
"""Cheap, composable admission checks. Admission is never qualification proof."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tomllib


class Refused(Exception):
    pass


def require(condition, message):
    if not condition:
        raise Refused(message)


def read_json(path):
    try:
        return json.loads(Path(path).read_text())
    except (OSError, ValueError):
        raise Refused("required JSON artifact is missing or malformed") from None


def artifact(path, executable=False):
    path = Path(path)
    require(path.is_file() and path.stat().st_size > 0, "required artifact is missing or empty")
    require(not executable or os.access(path, os.X_OK), "required artifact is not executable")
    return path.resolve()


def windows_runner(event, runners=None, hosted_default=False):
    require(isinstance(event, dict), "GitHub event must be an object")
    pr = event.get("pull_request")
    if pr is not None:
        require(isinstance(pr, dict), "malformed pull_request event")
        labels = pr.get("labels", [])
        require(isinstance(labels, list), "malformed pull_request labels")
        names = {label.get("name") for label in labels if isinstance(label, dict)}
        head = pr.get("head", {}).get("repo", {}).get("full_name")
        repository = event.get("repository", {}).get("full_name")
        if head and repository and head != repository:
            return "windows-latest (existing fork PR route)"
        if hosted_default and "windows-self-hosted" not in names:
            return "windows-latest (hosted default; no self-hosted opt-in)"
        if not hosted_default and "windows-hosted" in names:
            return "windows-latest (existing windows-hosted PR label)"
    elif hosted_default:
        return "windows-latest (hosted default)"
    require(isinstance(runners, dict) and isinstance(runners.get("runners"), list),
            "self-hosted Windows route needs a fresh repository runner inventory; "
            "for a PR, the existing windows-hosted label selects the hosted route")
    needed = {"self-hosted", "windows", "x64", "msvc"}
    for runner in runners["runners"]:
        require(isinstance(runner, dict), "malformed runner inventory")
        labels = runner.get("labels", [])
        require(isinstance(labels, list), "malformed runner labels")
        names = {label.get("name", "").lower() for label in labels if isinstance(label, dict)}
        if runner.get("status") == "online" and needed <= names:
            return "self-hosted Windows (online matching runner in supplied snapshot; queue not proven)"
    remedy = ("remove the windows-self-hosted PR opt-in" if hosted_default
              else "use the existing windows-hosted PR label")
    raise Refused(f"no online self-hosted Windows/X64/msvc runner; {remedy} "
                  "before starting a new workflow run")


def inventory(data, required=(), include_ignored=False):
    require(isinstance(data, dict) and isinstance(data.get("rust-suites"), dict),
            "nextest inventory lacks rust-suites")
    names = []
    for suite in data["rust-suites"].values():
        require(isinstance(suite, dict) and isinstance(suite.get("testcases"), dict),
                "nextest inventory has malformed testcases")
        for name, case in suite["testcases"].items():
            require(isinstance(case, dict) and isinstance(case.get("filter-match"), dict),
                    "nextest testcase lacks filter-match")
            status = case["filter-match"].get("status")
            require(status in ("matches", "mismatch"), "unknown nextest filter-match status")
            ignored = case.get("ignored")
            require(isinstance(ignored, bool), "nextest testcase lacks boolean ignored state")
            if status == "matches" and (include_ignored or not ignored):
                names.append(name)
    require(names, "nextest selection contains no runnable tests")
    require(set(required) <= set(names), "nextest selection is missing required test names")
    return len(names)


def run_checked(command, **kwargs):
    try:
        result = subprocess.run(command, check=False, **kwargs)
    except (OSError, subprocess.TimeoutExpired):
        raise Refused("prerequisite command is missing or did not complete") from None
    require(result.returncode == 0, f"prerequisite command exited {result.returncode}")
    return result


def collect_inventory(path, selectors, required=(), include_ignored=False):
    # -E filters test execution, not Cargo's build graph. Require the same
    # explicit package and binary selection the affected run will use.
    scope = selectors[:selectors.index("--")] if "--" in selectors else selectors

    def named(option):
        return any((arg == option and index + 1 < len(scope)
                    and scope[index + 1] and not scope[index + 1].startswith("-"))
                   or (arg.startswith(option + "=") and bool(arg[len(option) + 1:]))
                   for index, arg in enumerate(scope))

    broad = {"--workspace", "--all", "--all-targets", "--tests", "--bins", "--examples", "--benches"}
    require(not broad.intersection(scope) and (named("-p") or named("--package"))
            and ("--lib" in scope or named("--test") or named("--bin")),
            "inventory collection needs explicit -p/--package and --lib/--test/--bin; "
            "a nextest -E filter alone does not scope compilation")
    # Cold rustup messages must finish before the JSON file is opened.
    print("ADMISSION_PHASE inventory tool-initialization", file=sys.stderr, flush=True)
    run_checked(["vx", "cargo", "--version"], stdout=sys.stderr)
    print("ADMISSION_PHASE inventory selected-binary-list", file=sys.stderr, flush=True)
    with Path(path).open("wb") as output:
        run_checked(["vx", "--no-auto-install", "cargo", "nextest", "list",
                     *selectors, "--message-format", "json"], stdout=output)
    return inventory(read_json(path), required, include_ignored)


def linux_container_engine(info):
    require(isinstance(info, dict) and info.get("OSType") == "linux",
            "this container workload requires a Linux Docker engine; "
            "a Windows container engine is incompatible")
    require(isinstance(info.get("ServerVersion"), str) and info["ServerVersion"].strip(),
            "Docker daemon did not report a server version")
    return "Linux Docker engine responded (container execution still requires its own test)"


def within(root, value):
    require(isinstance(value, str) and value, "fixture path is missing")
    path = Path(value)
    require(path.is_absolute(), "fixture paths must be explicit absolute paths")
    path = path.resolve()
    require(path != root and root in path.parents, "fixture path escapes its private root")
    return path


def credentials(root, config, env):
    root = Path(root).resolve()
    require(root.is_dir(), "private fixture root does not exist")
    home = env.get("WAYLAND_HOME")
    require(home and Path(home).is_absolute(), "fixture needs explicit child WAYLAND_HOME")
    home = Path(home).resolve()
    require(home == root or root in home.parents, "WAYLAND_HOME escapes private fixture root")
    require(home.is_dir(), "fixture WAYLAND_HOME does not exist")
    require(isinstance(config, dict), "fixture config must be an object")
    session = config.get("session", {})
    require(isinstance(session, dict) and session.get("enabled") is True
            and session.get("require_durability") is True,
            "durable fixture must keep session.enabled and require_durability true")
    within(root, session.get("directory"))
    storage = config.get("storage", {})
    require(isinstance(storage, dict), "fixture storage config is malformed")
    store = storage.get("credentials", {})
    require(isinstance(store, dict), "fixture credentials config is malformed")
    backend = store.get("backend", {})
    require(isinstance(backend, dict) and set(backend) == {"encrypted_file"},
            "durable fixture must select its private encrypted_file backend, not the host keyring")
    vault = backend["encrypted_file"]
    require(isinstance(vault, dict), "encrypted_file config is malformed")
    cipher = within(root, vault.get("cipher_path"))
    params = within(root, vault.get("key_params_path"))
    require(cipher != params, "vault ciphertext and parameters must use different paths")
    descriptor = env.get("WAYLAND_VAULT_PASSPHRASE_FD")
    if descriptor:
        try:
            require(int(descriptor) >= 0, "invalid vault unlock descriptor")
            os.fstat(int(descriptor))  # Do not consume or print unlock material.
        except (ValueError, OSError):
            raise Refused("vault unlock descriptor is invalid or closed") from None
    else:
        require(bool(env.get("WAYLAND_VAULT_PASSPHRASE")), "fixture vault unlock material is absent")
    return "private encrypted-vault preconditions present (key usability not tested)"


def scenario(evaluator, paired_task=None, names=()):
    evaluator = artifact(evaluator, executable=True)
    selectors = []
    if paired_task:
        artifact(paired_task)
        selectors = ["--paired-task", str(paired_task)]
    else:
        require(names, "select at least one scenario")
        for name in names:
            selectors.extend(["--scenario", name])
    # The evaluator's real strict dry-run resolves platform admission before
    # artifact inspection or execution. Never copy its Rust family/platform map.
    env = {**os.environ, "OPENAI_API_KEY": "qualification-fixture-not-a-real-key"}
    result = run_checked([str(evaluator), *selectors, "--provider", "openai",
                          "--base-url", "http://127.0.0.1:9", "--fixture-cost-is-free",
                          "--dry", "--strict"], env=env, capture_output=True, text=True, timeout=30)
    estimate = re.search(r"^ESTIMATE .* runnable=(\d+) skip=(\d+)$", result.stdout, re.MULTILINE)
    require(estimate and int(estimate[1]) > 0 and int(estimate[2]) == 0,
            "strict evaluator dry-run did not admit a nonempty scenario selection without skips")
    return "current-platform scenario admission passed; no provider or key-store request made"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="check", required=True)
    runner = sub.add_parser("windows-runner")
    runner.add_argument("--event", type=Path, required=True)
    runner.add_argument("--runners", type=Path)
    runner.add_argument("--hosted-default", action="store_true")
    selection = sub.add_parser("inventory")
    selection.add_argument("--input", type=Path, required=True)
    selection.add_argument("--collect", action="store_true", help="run vx nextest list into --input")
    selection.add_argument("--require", action="append", default=[])
    selection.add_argument("--include-ignored", action="store_true")
    selection.add_argument("selectors", nargs=argparse.REMAINDER)
    sub.add_parser("linux-container-engine", help="invoke only for a workload that needs containers")
    files = sub.add_parser("artifact")
    location = files.add_mutually_exclusive_group(required=True)
    location.add_argument("--file", type=Path)
    location.add_argument("--on-path")
    files.add_argument("--executable", action="store_true")
    fixture = sub.add_parser("credentials")
    fixture.add_argument("--root", type=Path, required=True)
    fixture.add_argument("--config", type=Path, required=True)
    scenarios = sub.add_parser("scenario")
    scenarios.add_argument("--evaluator", type=Path, required=True)
    selectors = scenarios.add_mutually_exclusive_group(required=True)
    selectors.add_argument("--paired-task", type=Path)
    selectors.add_argument("--scenario", action="append")
    args = parser.parse_args(argv)
    try:
        if args.check == "windows-runner":
            detail = windows_runner(read_json(args.event), read_json(args.runners) if args.runners else None,
                                    args.hosted_default)
        elif args.check == "inventory":
            selectors = args.selectors[1:] if args.selectors[:1] == ["--"] else args.selectors
            require(args.collect or not selectors, "nextest selectors require --collect")
            count = (collect_inventory(args.input, selectors, args.require, args.include_ignored)
                     if args.collect else inventory(read_json(args.input), args.require, args.include_ignored))
            detail = f"{count} runnable selected test cases; test execution is still required"
        elif args.check == "linux-container-engine":
            result = run_checked(["docker", "info", "--format", "{{json .}}"],
                                 capture_output=True, text=True, timeout=30)
            detail = linux_container_engine(json.loads(result.stdout))
        elif args.check == "artifact":
            path = shutil.which(args.on_path) if args.on_path else args.file
            require(path, "required executable is missing from the fixture PATH")
            artifact(path, args.executable or bool(args.on_path))
            detail = "artifact present; source/toolchain identity is checked by the separate build gate"
        elif args.check == "credentials":
            config_path = within(args.root.resolve(), str(args.config.resolve()))
            with config_path.open("rb") as source:
                config = tomllib.load(source)
            detail = credentials(args.root, config, os.environ)
        else:
            detail = scenario(args.evaluator, args.paired_task, args.scenario or ())
        print(json.dumps({"check": args.check, "status": "ADMITTED", "detail": detail}))
        return 0
    except (Refused, OSError, ValueError, TypeError, AttributeError):
        error = sys.exc_info()[1]
        detail = str(error) if isinstance(error, Refused) else "missing or malformed prerequisite artifact"
        print(json.dumps({"check": args.check, "status": "BLOCKED", "detail": detail}), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
