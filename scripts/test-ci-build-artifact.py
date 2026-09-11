#!/usr/bin/env python3
"""Offline negative controls for same-run build reuse; no credentials or builds."""
import argparse
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

SPEC = importlib.util.spec_from_file_location("ci_build_artifact", Path(__file__).with_name("ci-build-artifact.py"))
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class BuildArtifactTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.binary = self.root / "wayland-core.exe"
        self.binary.write_bytes(b"fixture binary bytes")
        self.run = {
            "repository": {"full_name": "owner/repo"}, "id": 12,
            "run_attempt": 2, "event": "pull_request", "conclusion": None,
            "head_sha": "b" * 40,
        }
        self.expected = {
            "schema": 1, "repository": "owner/repo", "run_id": "12",
            "run_attempt": "2", "event": "pull_request", "event_head_sha": "b" * 40,
            "source_sha": "a" * 40, "producer_job": "Build (x86_64-pc-windows-msvc)",
            "artifact_name": "wayland-core-x86_64-pc-windows-msvc",
            "target": "x86_64-pc-windows-msvc", "profile": "release",
            "features": ["default"], "toolchain": "rustc 1.95\nhost: x86_64-pc-windows-msvc",
            "rustflags": "", "encoded_rustflags": "", "abi_contract": "native-Windows-v1",
            "runner_image": {"ImageOS": "win25", "ImageVersion": "20260901.1"},
        }
        self.record = dict(self.expected, binary_size=self.binary.stat().st_size, binary_sha256=MODULE.sha256(self.binary))

    def test_exact_binary_and_identity_are_accepted(self):
        MODULE.validate_identity(self.record, self.expected, self.binary)
        MODULE.validate_build_info("wayland-core 0.13.13 (source " + "a" * 40 + ")", "a" * 40)

    def test_each_contract_mismatch_is_refused(self):
        for key in self.expected:
            with self.subTest(key=key):
                changed = copy.deepcopy(self.record)
                changed[key] = "different"
                with self.assertRaises(ValueError) as caught:
                    MODULE.validate_identity(changed, self.expected, self.binary)
                self.assertEqual(isinstance(caught.exception, MODULE.ImageOnlyMismatch), key == "runner_image")
        for key in self.record:
            with self.subTest(missing=key):
                changed = dict(self.record)
                del changed[key]
                with self.assertRaises(ValueError):
                    MODULE.validate_identity(changed, self.expected, self.binary)

    def test_voice_is_not_default_and_abi_changes_do_not_reuse(self):
        for change in ({"features": ["default", "voice"]}, {"abi_contract": "different-sysroot"}, {"profile": "debug"}):
            with self.assertRaises(ValueError):
                MODULE.validate_identity(dict(self.record, **change), self.expected, self.binary)

    def test_changed_bytes_and_embedded_source_are_refused(self):
        self.binary.write_bytes(b"different binary byt")
        with self.assertRaises(ValueError):
            MODULE.validate_identity(self.record, self.expected, self.binary)
        for output in ["wayland-core 0.13.13 (source " + "b" * 40 + ")", "unknown", "wayland-core 0.13.13 (source " + "a" * 40 + ")\nextra"]:
            with self.assertRaises(ValueError):
                MODULE.validate_build_info(output, "a" * 40)

    def test_foreign_run_attempt_event_and_failed_run_are_refused(self):
        MODULE.validate_run(self.run, "owner/repo", "12", "2")
        for change in ({"repository": {"full_name": "foreign/repo"}}, {"id": 13}, {"run_attempt": 1}, {"event": "workflow_run"}, {"conclusion": "failure"}):
            with self.assertRaises(ValueError):
                MODULE.validate_run(dict(self.run, **change), "owner/repo", "12", "2")

    def test_only_the_named_successful_producer_is_ready(self):
        name = self.expected["producer_job"]
        self.assertFalse(MODULE.producer_status([], name))
        self.assertFalse(MODULE.producer_status([dict(name=name, status="in_progress")], name))
        self.assertTrue(MODULE.producer_status([dict(name=name, status="completed", conclusion="success")], name))
        for conclusion in ["failure", "skipped", "cancelled", "timed_out"]:
            with self.assertRaises(ValueError):
                MODULE.producer_status([dict(name=name, status="completed", conclusion=conclusion)], name)

    def test_artifact_cannot_be_selected_from_another_run_or_head(self):
        artifact = {"name": self.expected["artifact_name"], "expired": False, "workflow_run": {"id": 12, "head_sha": "b" * 40}}
        self.assertEqual(MODULE.select_artifact([artifact], self.expected), artifact)
        for change in [{"expired": True}, {"workflow_run": {"id": 13, "head_sha": "b" * 40}}, {"workflow_run": {"id": 12, "head_sha": "a" * 40}}]:
            with self.assertRaises(ValueError):
                MODULE.select_artifact([dict(artifact, **change)], self.expected)

    def test_archive_is_sealed_and_rejects_missing_metadata_or_extra_paths(self):
        for bad in [None, "missing", "../escape", "duplicate"]:
            archive = self.root / "artifact.zip"
            with zipfile.ZipFile(archive, "w") as output:
                output.writestr("wayland-core.exe", self.binary.read_bytes())
                if bad != "missing":
                    output.writestr(MODULE.SIDECAR, json.dumps(self.record))
                if bad in ("../escape", "duplicate"):
                    output.writestr(bad, b"unexpected")
            target = self.root / (bad or "valid").replace("/", "_")
            target.mkdir()
            if bad is None:
                self.assertEqual(MODULE.unpack(archive, target, self.expected).read_bytes(), self.binary.read_bytes())
            else:
                with self.assertRaises(ValueError):
                    MODULE.unpack(archive, target, self.expected)

    def test_pr_merge_source_is_distinct_from_api_head_and_vx_is_used(self):
        args = argparse.Namespace(target="x86_64-pc-windows-msvc", profile="release", features="default", abi_contract="native-Windows-v1", producer_job=self.expected["producer_job"], artifact=self.expected["artifact_name"])
        environment = {"GITHUB_REPOSITORY": "owner/repo", "GITHUB_RUN_ID": "12", "GITHUB_RUN_ATTEMPT": "2", "RUNNER_OS": "Windows", "RUNNER_ARCH": "X64", "ImageOS": "win25", "ImageVersion": "20260901.1"}
        commands = []
        def command(argv, **kwargs):
            commands.append(argv)
            if argv[:3] == ["git", "rev-parse", "HEAD"]:
                return "a" * 40
            if argv[:2] == ["git", "status"]:
                return ""
            if argv == ["vx", "rustc", "-vV"]:
                return self.expected["toolchain"]
            raise AssertionError(argv)
        with patch.dict(os.environ, environment, clear=True), patch.object(MODULE, "api", return_value=self.run), patch.object(MODULE, "command", command):
            identity, _ = MODULE.context(args)
        self.assertEqual(identity["source_sha"], "a" * 40)
        self.assertEqual(identity["event_head_sha"], "b" * 40)
        self.assertIn(["vx", "rustc", "-vV"], commands)

    def test_fetch_installs_only_after_digest_and_embedded_source_validation(self):
        archive = self.root / "download.zip"
        with zipfile.ZipFile(archive, "w") as output:
            output.writestr("wayland-core.exe", self.binary.read_bytes())
            output.writestr(MODULE.SIDECAR, json.dumps(self.record))
        original_bytes = self.binary.read_bytes()
        artifact = {"id": 99, "name": self.expected["artifact_name"], "expired": False, "workflow_run": {"id": 12, "head_sha": "b" * 40}}
        job = {"name": self.expected["producer_job"], "status": "completed", "conclusion": "success"}
        args = argparse.Namespace(wait_seconds=30, target=self.expected["target"], binary=str(self.binary))
        def download(command, **kwargs):
            self.assertEqual(command, ["gh", "api", "repos/owner/repo/actions/artifacts/99/zip"])
            kwargs["stdout"].write(archive.read_bytes())
            import subprocess
            return subprocess.CompletedProcess(command, 0)
        for embedded_sha in ["b" * 40, "a" * 40]:
            self.binary.write_bytes(b"preexisting binary")
            with patch.object(MODULE, "context", return_value=(self.expected, self.run)), patch.object(MODULE, "api", return_value=self.run), patch.object(MODULE, "pages", side_effect=[[job], [artifact]]), patch.object(MODULE.subprocess, "run", download), patch.object(MODULE, "command", return_value=f"wayland-core 0.13.13 (source {embedded_sha})"):
                if embedded_sha != self.expected["source_sha"]:
                    with self.assertRaisesRegex(ValueError, "embedded"):
                        MODULE.fetch(args)
                    self.assertEqual(self.binary.read_bytes(), b"preexisting binary")
                else:
                    MODULE.fetch(args)
                    self.assertEqual(self.binary.read_bytes(), original_bytes)

    def test_wait_bound_refuses_without_downloading_or_rebuilding(self):
        args = argparse.Namespace(wait_seconds=1, target=self.expected["target"], binary=str(self.binary))
        with patch.object(MODULE, "context", return_value=(self.expected, self.run)), patch.object(MODULE.time, "monotonic", side_effect=[0, 2]), patch.object(MODULE, "api") as api:
            with self.assertRaisesRegex(ValueError, "wait bound"):
                MODULE.fetch(args)
            api.assert_not_called()

    NEW_IMAGE = {"ImageOS": "win25", "ImageVersion": "20260908.2"}

    def test_runner_image_only_mismatch_is_distinct_and_names_both_images(self):
        with self.assertRaises(MODULE.ImageOnlyMismatch) as caught:
            MODULE.validate_identity(dict(self.record, runner_image=self.NEW_IMAGE), self.expected, self.binary)
        self.assertIn("20260908.2", str(caught.exception))
        self.assertIn("20260901.1", str(caught.exception))

    def test_runner_image_plus_any_other_mismatch_is_a_hard_refusal(self):
        changes = [{key: "different"} for key in self.expected if key != "runner_image"]
        changes += [{"binary_size": 1}, {"binary_sha256": "0" * 64}]
        for change in changes:
            with self.subTest(change=change):
                with self.assertRaises(ValueError) as caught:
                    MODULE.validate_identity(dict(self.record, runner_image=self.NEW_IMAGE, **change), self.expected, self.binary)
                self.assertNotIsInstance(caught.exception, MODULE.ImageOnlyMismatch)

    def fetch_exit(self, record, payload=None):
        """Run the real CLI fetch path against a mocked run; return exit code, installed bytes, stderr."""
        archive = self.root / "exit.zip"
        with zipfile.ZipFile(archive, "w") as output:
            output.writestr("wayland-core.exe", self.binary.read_bytes() if payload is None else payload)
            output.writestr(MODULE.SIDECAR, json.dumps(record))
        installed = self.root / "installed" / "wayland-core.exe"
        installed.parent.mkdir(exist_ok=True)
        installed.write_bytes(b"preexisting binary")
        artifact = {"id": 99, "name": self.expected["artifact_name"], "expired": False, "workflow_run": {"id": 12, "head_sha": "b" * 40}}
        job = {"name": self.expected["producer_job"], "status": "completed", "conclusion": "success"}
        def download(command, **kwargs):
            kwargs["stdout"].write(archive.read_bytes())
            return subprocess.CompletedProcess(command, 0)
        argv = ["ci-build-artifact.py", "fetch", "--binary", str(installed), "--target", self.expected["target"],
                "--profile", "release", "--features", "default", "--abi-contract", "native-Windows-v1",
                "--producer-job", self.expected["producer_job"], "--artifact", self.expected["artifact_name"],
                "--wait-seconds", "30"]
        stderr = io.StringIO()
        with patch.object(sys, "argv", argv), patch.object(sys, "stderr", stderr), patch.object(MODULE, "context", return_value=(self.expected, self.run)), patch.object(MODULE, "api", return_value=self.run), patch.object(MODULE, "pages", side_effect=[[job], [artifact]]), patch.object(MODULE.subprocess, "run", download), patch.object(MODULE, "command", return_value="wayland-core 0.13.14 (source " + "a" * 40 + ")"):
            try:
                MODULE.main()
                code = 0
            except SystemExit as exit:
                code = exit.code
        return code, installed.read_bytes(), stderr.getvalue()

    def test_cli_exit_code_separates_image_only_from_hard_refusals(self):
        # ci.yml branches on the literal 3; changing the constant must fail here.
        self.assertEqual(MODULE.IMAGE_ONLY_EXIT, 3)
        self.assertEqual(self.fetch_exit(self.record)[:2], (0, self.binary.read_bytes()))
        code, installed, stderr = self.fetch_exit(dict(self.record, runner_image=self.NEW_IMAGE))
        self.assertEqual((code, installed), (3, b"preexisting binary"))
        self.assertIn("20260908.2", stderr)
        self.assertIn("20260901.1", stderr)
        for label, record, payload in [
            ("features", dict(self.record, runner_image=self.NEW_IMAGE, features=["default", "voice"]), None),
            ("source", dict(self.record, runner_image=self.NEW_IMAGE, source_sha="c" * 40), None),
            ("toolchain", dict(self.record, runner_image=self.NEW_IMAGE, toolchain="rustc 1.96\nhost: x86_64-pc-windows-msvc"), None),
            ("sha", dict(self.record, runner_image=self.NEW_IMAGE), b"different binary byt"),
        ]:
            with self.subTest(label):
                self.assertEqual(self.fetch_exit(record, payload)[:2], (1, b"preexisting binary"))

    def test_produce_can_bind_a_local_rebuild_to_the_checkout(self):
        output = self.root / "local-identity.json"
        args = argparse.Namespace(binary=str(self.binary), output=str(output), verify_build_info=True)
        for embedded in ("b" * 40, "a" * 40):
            with patch.object(MODULE, "context", return_value=(self.expected, self.run)), patch.object(MODULE, "command", return_value=f"wayland-core 0.13.14 (source {embedded})"):
                if embedded != self.expected["source_sha"]:
                    with self.assertRaisesRegex(ValueError, "embedded"):
                        MODULE.produce(args)
                    self.assertFalse(output.exists())
                else:
                    MODULE.produce(args)
                    MODULE.validate_identity(json.loads(output.read_text()), self.expected, self.binary)


if __name__ == "__main__":
    unittest.main()
