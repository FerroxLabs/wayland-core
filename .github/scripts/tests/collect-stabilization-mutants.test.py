#!/usr/bin/env python3
"""Fake commands test bookkeeping ONLY; these are not mutation proof.

NightlyGate additionally EXECUTES the mutants-nightly gate step with a stub
`cargo` on PATH. No real cargo, rustc or build is ever invoked. It exists so
the two polarities core#424 and core#451 c3 depend on -- no data => failure,
a finding => success -- cannot be silently destroyed by a later timeout fix.
"""
import copy
import importlib.util
import json
import os
from pathlib import Path
import re
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


WORKFLOW = ROOT / ".github/workflows/mutants-nightly.yml"

CRON_SUMMARY = "339 mutants tested in 52m: 64 missed, 196 caught, 77 unviable, 2 timeouts"
CRON_BASELINE = "ok       Unmutated baseline in 203s build + 5s test"
CENSORED_BASELINE = "TIMEOUT  Unmutated baseline in 307s build + 180s test"
NO_DATA_TAIL = "ERROR cargo test failed in an unmutated tree, so no mutants were tested"

STUB_CARGO = """#!/bin/sh
# Stub. NOT cargo: no build, no rustc, no network. It reproduces only the two
# cargo-mutants 27.1.0 behaviours this gate branches on -- the missing
# --output parent refusal, and the presence or absence of a summary line.
out=""; prev=""
for a in "$@"; do
  [ "$prev" = "--output" ] && out="$a"
  prev="$a"
done
printf 'STUB_ARGV %s\\n' "$*" >> "$STUB_ARGV_LOG"
parent=$(dirname "$out")
if [ ! -d "$parent" ]; then
  printf 'Error: create output parent directory "%s"\\n' "$out"
  printf 'Caused by:\\n    No such file or directory (os error 2)\\n'
  exit 1
fi
mkdir -p "$out/mutants.out"
cat "$STUB_LOG"
exit "$STUB_EXIT"
"""


def strip_comments(text):
    return re.sub(r"(?m)^\s*#.*\n", "", text)


def gate_script():
    """The `run:` body of the `mutants` step, dedented exactly as Actions runs it."""
    lines = WORKFLOW.read_text().splitlines()
    start = next(i for i, line in enumerate(lines) if line.strip().startswith("- name: Run cargo-mutants"))
    key = next(i for i in range(start, len(lines)) if lines[i].strip() == "run: |")
    indent = len(lines[key]) - len(lines[key].lstrip())
    body = []
    for line in lines[key + 1:]:
        if line.strip() and len(line) - len(line.lstrip()) <= indent:
            break
        body.append(line[indent + 2:] if line.strip() else "")
    return "\n".join(body) + "\n"


class NightlyGate(unittest.TestCase):
    """Execute the real gate step against a stub cargo. Both polarities, every arm."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.dir = Path(self.temp.name).resolve()
        self.bin = self.dir / "stub-bin"
        self.bin.mkdir()
        stub = self.bin / "cargo"
        stub.write_text(STUB_CARGO)
        stub.chmod(0o755)
        self.workspace = self.dir / "workspace"
        self.workspace.mkdir()

    def run_gate(self, log_lines, exit_code, script=None, make_target=False):
        """Run the step in a fresh workspace. Returns (exit, outputs, log, argv)."""
        work = self.dir / f"run-{len(list(self.dir.iterdir()))}"
        work.mkdir()
        if make_target:
            (work / "target").mkdir()
        stub_log = work / "stub-output.txt"
        stub_log.write_text("\n".join(log_lines) + "\n")
        argv_log = work / "stub-argv.txt"
        argv_log.write_text("")
        outputs = work / "github-output.txt"
        outputs.write_text("")
        step = work / "step.sh"
        step.write_text(script if script is not None else gate_script())
        env = {
            # PATH cannot reach a real cargo: the stub dir plus the base system
            # only. cargo lives in ~/.cargo/bin, which is deliberately absent.
            "PATH": f"{self.bin}:/usr/bin:/bin",
            "HOME": str(work),
            "CRATE": "wcore-cron",
            "MIN_TEST_TIMEOUT": "90",
            "TIMEOUT_MULTIPLIER": "5",
            "IN_PLACE": "false",
            "SHARD_ARG": "",
            "GITHUB_OUTPUT": str(outputs),
            "GITHUB_RUN_ID": "33844721279",
            "RUNNER_OS": "macOS",
            "STUB_LOG": str(stub_log),
            "STUB_ARGV_LOG": str(argv_log),
            "STUB_EXIT": str(exit_code),
        }
        result = subprocess.run(["bash", str(step)], cwd=work, env=env,
                                capture_output=True, text=True)
        parsed = {}
        for line in outputs.read_text().splitlines():
            if "=" in line and not line.startswith(("missed_mutants<<", "MUTANTS_EOF")):
                key, _, value = line.partition("=")
                parsed[key] = value
        return result, parsed, work, argv_log.read_text()

    # ---------- green polarity: a finding must NOT red the leg ----------

    def test_surviving_mutants_are_a_finding_and_keep_the_leg_green(self):
        # cargo-mutants exits 3 for surviving mutants. core#424 c3 / core#451 c3.
        result, out, work, _ = self.run_gate([CRON_BASELINE, CRON_SUMMARY], 3)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(out["real_data"], "true")
        self.assertEqual(out["missed"], "64")
        self.assertEqual(out["caught"], "196")
        self.assertEqual(out["catch_rate"], "75.4%")
        self.assertEqual(out["exit_code"], "3")
        self.assertEqual(out["summary"], CRON_SUMMARY)
        # The step creates its own --output parent; the stub proves it existed.
        self.assertTrue((work / "target/mutants-wcore-cron/mutants.out").is_dir())

    def test_a_clean_run_with_no_survivors_stays_green_and_files_nothing(self):
        clean = "339 mutants tested in 52m: 0 missed, 260 caught, 77 unviable, 2 timeouts"
        result, out, _, _ = self.run_gate([CRON_BASELINE, clean], 0)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(out["missed"], "0")     # the report step's `if` skips on this
        self.assertEqual(out["catch_rate"], "100.0%")

    # ---------- red polarity: no data must red the leg ----------

    def test_no_summary_line_reds_the_leg_however_cargo_mutants_exited(self):
        for code in (0, 1, 4):
            with self.subTest(exit_code=code):
                result, out, _, _ = self.run_gate([CENSORED_BASELINE, NO_DATA_TAIL], code)
                self.assertNotEqual(result.returncode, 0,
                                    "a data-less leg concluded success: core#424 is reopened")
                self.assertEqual(out["real_data"], "false")
                self.assertEqual(out["catch_rate"], "N/A%")
                self.assertIn("no mutation data was produced", result.stdout)

    def test_the_original_missing_target_parent_defect_now_reds_instead_of_passing(self):
        # The 87 green runs. Remove `mkdir -p target` and the step must go red.
        original = gate_script()
        mutated = original.replace("mkdir -p target\n", "", 1)
        self.assertNotEqual(mutated, original, "mutation did not land: `mkdir -p target` is gone")
        self.assertNotIn("\nmkdir -p target\n", mutated)
        result, out, work, _ = self.run_gate([CRON_BASELINE, CRON_SUMMARY], 3, script=mutated)
        log = (work / ".blackboard/E2E-MUTATION-BASELINE/wcore-cron.log").read_text()
        # Assert the mutation reached the defect, not some unrelated failure.
        self.assertIn("create output parent directory", log)
        self.assertIn("No such file or directory (os error 2)", log)
        self.assertEqual(out["real_data"], "false")
        self.assertNotEqual(result.returncode, 0)
        # ... and the unmutated step survives the same arm, because it mkdirs.
        restored, out2, _, _ = self.run_gate([CRON_BASELINE, CRON_SUMMARY], 3, script=original)
        self.assertEqual(restored.returncode, 0, restored.stderr)
        self.assertEqual(out2["real_data"], "true")

    def test_removing_the_failure_exit_makes_the_gate_unfalsifiable(self):
        # Proves the `exit 1` this file guards is load-bearing, not decoration.
        original = gate_script()
        mutated = original.replace(
            'This is a harness failure, not a coverage result."\n  exit 1\n',
            'This is a harness failure, not a coverage result."\n  exit 0\n', 1)
        self.assertNotEqual(mutated, original, "mutation did not land: no `exit 1` under the error")
        swallowed, _, _, _ = self.run_gate([CENSORED_BASELINE, NO_DATA_TAIL], 4, script=mutated)
        self.assertEqual(swallowed.returncode, 0)   # the 87-run behaviour, reproduced
        red, _, _, _ = self.run_gate([CENSORED_BASELINE, NO_DATA_TAIL], 4, script=original)
        self.assertNotEqual(red.returncode, 0)      # and the shipped step rejects it

    # ---------- core#451: the baseline must be measured, not censored ----------

    def test_the_invocation_cannot_reimpose_a_baseline_censoring_timeout(self):
        argv = self.run_gate([CRON_BASELINE, CRON_SUMMARY], 3)[3]
        self.assertIn("--timeout-multiplier 5", argv)
        self.assertIn("--minimum-test-timeout 90", argv)
        # `-t/--timeout` bounds ALL cargo commands including the unmutated
        # baseline, which is what censored four of five legs (core#451).
        self.assertNotRegex(argv, r"(?<![-\w])(-t|--timeout)(?=[ =\n])")

    def test_the_baseline_measurement_is_recorded_and_censorship_is_not_a_duration(self):
        _, out, work, _ = self.run_gate([CRON_BASELINE, CRON_SUMMARY], 3)
        measured = json.loads((work / ".blackboard/E2E-MUTATION-BASELINE/wcore-cron-baseline.json").read_text())
        self.assertEqual(out["baseline_measured"], "true")
        self.assertEqual((measured["build_seconds"], measured["test_seconds"]), (203, 5))
        self.assertFalse(measured["censored_by_timeout_flag"])
        self.assertEqual(measured["run_id"], "33844721279")

        _, out, work, _ = self.run_gate([CENSORED_BASELINE, NO_DATA_TAIL], 4)
        censored = json.loads((work / ".blackboard/E2E-MUTATION-BASELINE/wcore-cron-baseline.json").read_text())
        self.assertEqual(out["baseline_measured"], "false")
        self.assertFalse(censored["measured"])
        self.assertTrue(censored["censored_by_timeout_flag"])

    # ---------- static wiring the executed arms cannot reach ----------

    def test_issue_filing_is_gated_on_real_data_and_the_matrix_carries_a_floor(self):
        text = strip_comments(WORKFLOW.read_text())
        self.assertIn("if: steps.mutants.outputs.real_data == 'true' "
                      "&& steps.mutants.outputs.missed != '0'", text)
        # The censoring field is gone -- anchored, so `min_test_timeout` does not match it.
        self.assertNotRegex(text, r"(?m)^\s+test_timeout:")
        self.assertEqual(len(re.findall(r"(?m)^\s+min_test_timeout: \d+$", text)), 5)
        self.assertIn(".blackboard/E2E-MUTATION-BASELINE/${{ matrix.crate }}-baseline.json", text)


if __name__ == "__main__":
    unittest.main()
