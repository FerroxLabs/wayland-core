#!/usr/bin/env python3
"""Offline negative controls for qualification prerequisite admission."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("prerequisites", ROOT / "scripts/qualification-prerequisites.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


def selected():
    return {"rust-suites": {"fixture": {"testcases": {
        "required_case": {"ignored": False, "filter-match": {"status": "matches"}}
    }}}}


class Prerequisites(unittest.TestCase):
    def test_inventory_positive_and_negative_controls(self):
        self.assertEqual(gate.inventory(selected(), ["required_case"]), 1)
        invalid = [None, {}, {"rust-suites": {}}, {"rust-suites": {"bad": {}}}]
        for status in ["mismatch", "unknown", None]:
            value = selected()
            value["rust-suites"]["fixture"]["testcases"]["required_case"]["filter-match"]["status"] = status
            invalid.append(value)
        for value in invalid:
            with self.subTest(value=value), self.assertRaises(gate.Refused):
                gate.inventory(value)
        with self.assertRaises(gate.Refused):
            gate.inventory(selected(), ["missing_case"])
        value = selected()
        value["rust-suites"]["fixture"]["testcases"]["required_case"]["ignored"] = True
        with self.assertRaises(gate.Refused):
            gate.inventory(value)
        self.assertEqual(gate.inventory(value, include_ignored=True), 1)

    def test_missing_empty_and_malformed_artifacts_fail_the_cli(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "selection.json"
            for content in [None, "", "\n  rustup installed\n{}", "{broken"]:
                if content is not None:
                    path.write_text(content)
                with contextlib.redirect_stderr(io.StringIO()):
                    self.assertEqual(gate.main(["inventory", "--input", str(path)]), 1)
            path.write_text(json.dumps(selected()))
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(gate.main(["inventory", "--input", str(path)]), 0)
            path.write_bytes(b"")
            with self.assertRaises(gate.Refused):
                gate.artifact(path)
            with self.assertRaises(gate.Refused):
                gate.artifact(Path(directory) / "missing")

    def test_collection_keeps_cold_setup_outside_json_capture(self):
        # Fake only the wrapper protocol: warmup emits non-JSON; listing emits
        # an inventory only after warmup, and requires --no-auto-install.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            marker = root / "warm"
            commands = []

            def wrapper(command, **kwargs):
                commands.append(command)
                if command == ["vx", "cargo", "--version"]:
                    self.assertIs(kwargs["stdout"], sys.stderr)
                    marker.write_text("installed")
                else:
                    self.assertTrue(marker.exists())
                    self.assertEqual(command[:5], ["vx", "--no-auto-install", "cargo", "nextest", "list"])
                    self.assertEqual(command[5:9], ["-p", "wcore-cli", "--test", "fixture"])
                    self.assertEqual(command[-2:], ["--message-format", "json"])
                    kwargs["stdout"].write(json.dumps(selected()).encode())
                return subprocess.CompletedProcess(command, 0)

            with patch.object(gate.subprocess, "run", side_effect=wrapper):
                self.assertEqual(gate.collect_inventory(root / "selection.json", ["-p", "wcore-cli", "--test", "fixture"]), 1)
            self.assertEqual(len(commands), 2)
            self.assertEqual(gate.read_json(root / "selection.json"), selected())

    def test_failed_collector_cannot_reuse_a_valid_inventory(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "selection.json"
            path.write_text(json.dumps(selected()))
            results = [subprocess.CompletedProcess([], 0), subprocess.CompletedProcess([], 101)]
            with patch.object(gate.subprocess, "run", side_effect=results), self.assertRaises(gate.Refused):
                gate.collect_inventory(path, ["-p", "wcore-cli", "--lib"])
            self.assertEqual(path.read_bytes(), b"")

    def test_filter_only_or_unknown_build_scope_refuses_before_cargo(self):
        for selectors in [[], ["-E", "test(required_case)"], ["--workspace"],
                          ["-p", "wcore-cli"], ["--test", "fixture"],
                          ["-p", "wcore-cli", "--lib", "--all-targets"],
                          ["-p", "--lib"], ["--", "-p", "wcore-cli", "--lib"]]:
            with self.subTest(selectors=selectors), patch.object(gate.subprocess, "run") as run:
                with self.assertRaises(gate.Refused):
                    gate.collect_inventory("must-not-open.json", selectors)
                run.assert_not_called()

    def test_windows_runner_routes_and_offline_refusal(self):
        labels = [{"name": name} for name in ["self-hosted", "Windows", "X64", "msvc"]]
        snapshot = {"runners": [{"status": "offline", "labels": labels}]}
        event = {"repository": {"full_name": "FerroxLabs/wayland-core"}, "pull_request": {
            "labels": [], "head": {"repo": {"full_name": "FerroxLabs/wayland-core"}}
        }}
        with self.assertRaises(gate.Refused):
            gate.windows_runner(event, snapshot)
        event["pull_request"]["labels"] = [{"name": "windows-hosted"}]
        self.assertIn("windows-latest", gate.windows_runner(event))
        event["pull_request"]["labels"] = []
        event["pull_request"]["head"]["repo"]["full_name"] = "contributor/wayland-core"
        self.assertIn("fork PR", gate.windows_runner(event))
        snapshot["runners"][0]["status"] = "online"
        self.assertIn("online matching", gate.windows_runner({}, snapshot))
        snapshot["runners"][0]["labels"].pop()
        with self.assertRaises(gate.Refused):
            gate.windows_runner({}, snapshot)

    def test_only_compatible_container_engine_is_admitted(self):
        self.assertIn("Linux Docker", gate.linux_container_engine({"OSType": "linux", "ServerVersion": "fixture"}))
        for info in [{"OSType": "windows", "ServerVersion": "fixture"}, {}, {"OSType": "linux"}, None]:
            with self.subTest(info=info), self.assertRaises(gate.Refused):
                gate.linux_container_engine(info)
        with patch.object(gate.subprocess, "run", return_value=subprocess.CompletedProcess([], 1)), self.assertRaises(gate.Refused):
            gate.run_checked(["docker", "info"])

    def test_hosted_default_keeps_forks_hosted_and_checks_explicit_opt_in(self):
        event = {"repository": {"full_name": "FerroxLabs/wayland-core"}, "pull_request": {
            "labels": [], "head": {"repo": {"full_name": "FerroxLabs/wayland-core"}}
        }}
        self.assertIn("hosted default", gate.windows_runner(event, hosted_default=True))
        event["pull_request"]["labels"] = [{"name": "windows-self-hosted"}]
        with self.assertRaises(gate.Refused):
            gate.windows_runner(event, {"runners": []}, hosted_default=True)
        event["pull_request"]["head"]["repo"]["full_name"] = "contributor/wayland-core"
        self.assertIn("fork PR", gate.windows_runner(event, {"runners": []}, hosted_default=True))

    def test_private_vault_preconditions_without_reading_credentials(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            config = {"session": {"enabled": True, "require_durability": True,
                                  "directory": str(root / "sessions")},
                      "storage": {"credentials": {"backend": {"encrypted_file": {
                          "cipher_path": str(root / "credentials.enc"),
                          "key_params_path": str(root / "credentials.params.json")}}}}}
            env = {"WAYLAND_HOME": str(root), "WAYLAND_VAULT_PASSPHRASE": "fake-test-only"}
            self.assertIn("preconditions present", gate.credentials(root, config, env))
            with self.assertRaises(gate.Refused):
                gate.credentials(root, config, {"WAYLAND_HOME": str(root)})
            with self.assertRaises(gate.Refused):
                gate.credentials(root, config, {**env, "WAYLAND_HOME": str(root.parent)})
            with self.assertRaises(gate.Refused):
                gate.credentials(root, config, {**env, "WAYLAND_VAULT_PASSPHRASE_FD": "-1"})
            with tempfile.TemporaryFile() as descriptor:
                # fstat must not read or advance the inherited unlock stream.
                descriptor.write(b"fixture-passphrase")
                descriptor.seek(0)
                gate.credentials(root, config, {"WAYLAND_HOME": str(root),
                                                "WAYLAND_VAULT_PASSPHRASE_FD": str(descriptor.fileno())})
                self.assertEqual(descriptor.tell(), 0)
            config["storage"]["credentials"]["backend"] = "auto"
            with self.assertRaises(gate.Refused):
                gate.credentials(root, config, env)

    def test_scenario_uses_real_dry_admission_protocol_and_refuses_skips(self):
        with tempfile.TemporaryDirectory() as directory:
            task = Path(directory) / "task.json"
            task.write_text("{}")
            for rc, output, accepted in [(0, "PLAN fixture openai\nESTIMATE upper_bound_usd=0.020000 runnable=1 skip=0\n", True),
                                         (0, "ESTIMATE upper_bound_usd=0 runnable=0 skip=1\n", False),
                                         (0, "", False), (2, "", False)]:
                result = subprocess.CompletedProcess([], rc, stdout=output, stderr="")
                with patch.object(gate.subprocess, "run", return_value=result) as run:
                    if accepted:
                        gate.scenario(sys.executable, task)
                    else:
                        with self.assertRaises(gate.Refused):
                            gate.scenario(sys.executable, task)
                    command = run.call_args.args[0]
                    self.assertEqual(command[-2:], ["--dry", "--strict"])
                    self.assertEqual(run.call_args.kwargs["env"]["OPENAI_API_KEY"], "qualification-fixture-not-a-real-key")
            with patch.object(gate.subprocess, "run") as run, self.assertRaises(gate.Refused):
                gate.scenario(Path(directory) / "missing-evaluator", task)
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
