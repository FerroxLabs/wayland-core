#!/usr/bin/env python3
"""Fake commands test bookkeeping ONLY; these are not mutation proof."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[3]


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


collector = module("collector", ROOT / ".github/scripts/collect-stabilization-mutants.py")
gate = module("gate", ROOT / "scripts/stabilization_release_gate.py")


class Bookkeeping(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.repo = self.root / "source"
        self.repo.mkdir()
        self.git("init", "-q")
        self.git("config", "user.name", "Mutation Bookkeeping Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        self.file = self.repo / "crates/fixture/src/lib.rs"
        self.file.parent.mkdir(parents=True)
        self.file.write_text("original fixture\n")
        (self.repo / ".gitignore").write_text("/target\n")
        (self.repo / ".config").mkdir()
        (self.repo / ".config/nextest.toml").write_text('[profile.default.junit]\npath = "junit.xml"\n')
        self.git("add", ".")
        self.git("commit", "-qm", "offline fixture source")
        self.source = self.git("rev-parse", "HEAD").strip()
        self.mapping = self.root / "mapping"
        self.mapping.mkdir()
        # Use an intentionally synthetic mutation and fake command result.
        # Only real collector executions may supply release mutation proof.
        self.file.write_text("mutated fixture\n")
        data = self.git("diff", "HEAD", "--")
        self.file.write_text("original fixture\n")
        original = json.loads((ROOT / ".github/scripts/stabilization-mutants/mapping.json").read_text())
        self.rows = original["mutants"]
        for row in self.rows:
            path = self.mapping / row["patch"]
            path.write_text(data)
            row["patch_sha256"] = collector.digest(path)
        (self.mapping / "mapping.json").write_text(json.dumps({"schema": 1, "definition_source": "offline-only", "mutants": self.rows}))
        self.output = self.root / "output"
        self.target = self.repo / "target"
        self.calls = []
        self.fault = None
        self.counter = 0

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args], stderr=subprocess.DEVNULL).decode()

    def fake_command(self, argv, cwd, env, stdout, stderr):
        self.calls.append(argv)
        self.assertNotIn("GH_TOKEN", env)
        self.assertNotIn("GITHUB_TOKEN", env)
        self.assertNotIn("OPENAI_API_KEY", env)
        if argv == ["vx", "cargo", "--version"]:
            stdout.write(b"fake Cargo initialization; not a build\n")
            return 0
        proof_path = Path(argv[argv.index("--output") + 1])
        junit = Path(argv[argv.index("--junit") + 1])
        phase, key = proof_path.parent.name, proof_path.parent.parent.name
        row = next(row for row in self.rows if row["id"] == key)
        command = argv[argv.index("--") + 1:]
        self.assertEqual(command, ["vx", "--no-auto-install", *row["argv"]])
        self.assertTrue(Path(env["WAYLAND_HOME"]).is_dir())
        diff = self.git("diff", "HEAD", "--")
        self.assertEqual(bool(diff), phase == "mutant")
        fault = self.fault if phase == "mutant" else None
        code = 100 if phase == "mutant" else 0
        proof = {"schema": "stabilization-command-1", "source": self.source, "complete": True,
                 "diff": diff, "argv": command, "platform": "Linux", "machine": "x86_64", "exit_code": code}
        if fault in ("build-failure", "stale-junit"):
            code = 101
            proof.update(complete=False, exit_code=code)
        else:
            self.counter += 1
            xml = ET.Element("testsuites", uuid=str(self.counter))
            for item in ([] if fault == "empty" else row["expected_cases"]):
                case = ET.SubElement(xml, "testcase", classname=item["classname"], name=item["name"])
                witness = next((w for w in row["expected_failures"] if w["name"] == item["name"]), None)
                if witness and phase == "mutant":
                    failure = ET.SubElement(case, "failure", type=witness["type"])
                    failure.text = "\n".join(witness["contains"])
                    if fault == "unrelated":
                        failure.text = "fixture could not open its credential store"
                    if fault == "timeout":
                        failure.set("type", "test timed out")
                    if fault == "wrong-test":
                        case.set("name", "different_test")
            junit.parent.mkdir(parents=True, exist_ok=True)
            junit.write_bytes(ET.tostring(xml))
            stamp = time.time_ns() + 1_000_000
            os.utime(junit, ns=(stamp, stamp))
            proof["junit_sha256"] = collector.digest(junit)
        proof_path.write_text(json.dumps(proof))
        stdout.write(b"offline fake command output; not mutation evidence\n")
        stderr.write(b"offline fake command diagnostic\n")
        return code

    def collect(self):
        with patch.dict(os.environ, {"GH_TOKEN": "must-not-propagate", "GITHUB_TOKEN": "must-not-propagate",
                                     "OPENAI_API_KEY": "must-not-propagate", "GITHUB_REPOSITORY": "fixture/repo",
                                     "GITHUB_RUN_ID": "offline", "GITHUB_JOB": "collect-release-mutants"}):
            return collector.collect(self.repo, self.output, self.mapping, self.source,
                                     self.target, gate, self.fake_command)

    def test_original_definitions_are_complete_and_patches_are_sealed(self):
        directory = ROOT / ".github/scripts/stabilization-mutants"
        mapping = json.loads((directory / "mapping.json").read_text())
        rows = collector.validate_mapping(mapping, directory, gate)
        self.assertEqual({row["id"] for row in rows}, set(gate.MUTANTS))
        self.assertEqual(mapping["definition_source"], "3530199fb757f3fcefecf9e40ba820775b867d84")
        damaged = copy.deepcopy(mapping)
        damaged["mutants"][0]["patch_sha256"] = "0" * 64
        with self.assertRaises(RuntimeError):
            collector.validate_mapping(damaged, directory, gate)

    def test_fake_full_batch_preserves_refs_commands_source_and_restores_owned_files(self):
        result = self.collect()
        self.assertTrue(result["complete"], result["blockers"])
        self.assertEqual(len(result["mutants"]), 8)
        self.assertEqual(len(self.calls), 17)  # one warmup, sixteen exact scoped captures
        self.assertEqual(result["provenance"], {"repository": "fixture/repo", "run_id": "offline",
                         "job": "collect-release-mutants", "source_sha": self.source})
        self.assertEqual(self.file.read_text(), "original fixture\n")
        self.assertFalse(self.git("status", "--porcelain"))
        self.assertEqual(self.git("rev-parse", "HEAD").strip(), self.source)
        for row in result["mutants"]:
            self.assertTrue(gate.read_ref(self.output, row["patch"]).is_file())
            self.assertEqual(gate.command_result(self.output, row, self.source)[1], 100)
            self.assertEqual(gate.command_result(self.output, row["baseline"], self.source)[1], 0)
        assets = self.root / "assets"
        assets.mkdir()
        (assets / "offline-only").write_bytes(b"not a release artifact")
        normalized = gate.produce(self.output / "index.json", assets, self.source, "v0.0.0", self.root / "normalized.json")
        self.assertEqual(len(normalized["mutants"]), 8)
        self.assertEqual(normalized["admission"], "blocked")  # fake bookkeeping is never full release proof

    def test_build_failure_stale_empty_wrong_test_timeout_and_unrelated_failures_not_caught(self):
        for fault in ["build-failure", "stale-junit", "empty", "wrong-test", "timeout", "unrelated"]:
            with self.subTest(fault=fault):
                self.fault = fault
                self.output = self.root / ("output-" + fault)
                result = self.collect()
                self.assertFalse(result["complete"])
                self.assertEqual(result["mutants"], [])
                self.assertTrue(result["blockers"])
                self.assertEqual(self.file.read_text(), "original fixture\n")
                self.assertTrue((self.output / self.rows[0]["id"] / "mutant/stderr.log").is_file())
                self.assertTrue((self.output / self.rows[0]["id"] / "tested.patch").is_file())

    def test_dirty_unowned_or_wrong_head_refuses_before_commands(self):
        (self.repo / "unowned.txt").write_text("preserve me")
        with self.assertRaises(RuntimeError):
            self.collect()
        self.assertFalse(self.calls)
        self.assertTrue((self.repo / "unowned.txt").exists())
        (self.repo / "unowned.txt").unlink()
        with self.assertRaises(RuntimeError):
            collector.collect(self.repo, self.output, self.mapping, "0" * 40, self.target, gate, self.fake_command)
        self.assertFalse(self.calls)

    def test_existing_output_cannot_overwrite_previous_attempt(self):
        self.output.mkdir()
        (self.output / "first-failure.log").write_text("retained")
        with self.assertRaises(RuntimeError):
            self.collect()
        self.assertEqual((self.output / "first-failure.log").read_text(), "retained")
        self.assertFalse(self.calls)


if __name__ == "__main__":
    unittest.main()
