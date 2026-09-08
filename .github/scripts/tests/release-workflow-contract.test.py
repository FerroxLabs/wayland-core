#!/usr/bin/env python3
"""Keep early admission, evidence capture, and nonduplicated coverage coupled."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/ci.yml"
BUDGET = "fix1_dispatch_budget_aborts_with_partial_result"


def blocks(text, pattern):
    matches = list(re.finditer(pattern, text, re.MULTILINE))
    return {
        match.group(1): text[match.start():matches[i + 1].start() if i + 1 < len(matches) else len(text)]
        for i, match in enumerate(matches)
    }


def audit(text):
    text = re.sub(r"(?m)^\s*#.*\n", "", text)
    jobs = blocks(text, r"^  ([a-z][a-z0-9-]*):\n")
    errors = []
    if "nick-fields/retry" in text:
        errors.append("whole-suite retry action returned")
    for name in ("ci", "ci-windows-hosted", "ci-linux", "build"):
        job = jobs.get(name, "")
        if not re.search(r"(?m)^    needs: (?:admission|\[[^\n]*\badmission\b[^\n]*\])$", job):
            errors.append(f"{name}: missing fast admission dependency")
        if name in ("ci", "ci-windows-hosted"):
            if not re.search(r"(?m)^    needs: admission$", job):
                errors.append(f"{name}: native compilation waits for unrelated build targets")
            if "--producer-job \"Build ($native_target)\"" not in job or "--wait-seconds 4500" not in job:
                errors.append(f"{name}: missing bounded own-target build dependency")
    for name in ("ci", "ci-windows-hosted", "ci-linux"):
        job = jobs.get(name, "")
        steps = blocks(job, r"^      - name: (.+)\n")
        main = steps.get("Run tests (nextest CI profile)", "")
        smoke = steps.get("Release binary smoke (catches plugin dead-code-strip)", "")
        if not main or not smoke or job.index(smoke) >= job.index(main):
            errors.append(f"{name}: smoke must precede workspace tests")
        if "run-tests-with-attempt-evidence.sh" not in main:
            errors.append(f"{name}: workspace evidence is not captured")
        if "not binary_id(=wcore-cli::release_binary_smoke)" not in main:
            errors.append(f"{name}: smoke duplicated in workspace selection")
        if "WCORE_SMOKE_REQUIRE_PREBUILT" not in smoke or "--no-tests=fail" not in smoke:
            errors.append(f"{name}: smoke can silently qualify nothing")
        if "wayland-ci-evidence/" not in job or "Upload complete" not in job:
            errors.append(f"{name}: raw diagnostics are not uploaded")
        if name != "ci-windows-hosted":
            packaged = steps.get("F01 packaged wayland-eval driver gate", "")
            if not packaged or (main and job.index(packaged) >= job.index(main)):
                errors.append(f"{name}: packaged admission is late or absent")
        isolated_name, target = {
            "ci": ("Walk identity controls (isolated macOS execution)", "walk_parallel_identity_test"),
            "ci-linux": ("Recovery fault cuts (isolated Linux execution)", "stabilization_recovery"),
        }.get(name, (None, None))
        if isolated_name:
            isolated = steps.get(isolated_name, "")
            if (not isolated or target not in isolated or target not in main
                    or "--no-tests=fail" not in isolated or "--retries 0" not in isolated
                    or "--test-threads 1" not in isolated
                    or (main and job.index(isolated) >= job.index(main))):
                errors.append(f"{name}: {target} exclusion lacks isolated execution")
        if name != "ci-linux":
            for step_name in ("Reuse source-bound native release binary",
                              "Release binary smoke (catches plugin dead-code-strip)",
                              "Dispatch budget boundary (isolated Windows execution)",
                              "Run tests (nextest CI profile)"):
                if "        shell: bash\n" not in steps.get(step_name, ""):
                    errors.append(f"{name}: Bash script has no explicit Bash shell: {step_name}")
            if name == "ci" and "        shell: bash\n" not in steps.get("F01 packaged wayland-eval driver gate", ""):
                errors.append("ci: packaged Bash script has no explicit Bash shell")
            isolated = steps.get("Dispatch budget boundary (isolated Windows execution)", "")
            for required in (BUDGET, "--release", "--retries 0", "--no-tests=fail", "--test-threads 1", "--profile ci"):
                if required not in isolated:
                    errors.append(f"{name}: isolated budget missing {required}")
            if BUDGET not in main or not isolated or (main and job.index(isolated) >= job.index(main)):
                errors.append(f"{name}: budget exclusion is not paired with early execution")
            durable = steps.get("Durable lifecycle (isolated Windows execution)", "")
            target = "w02_delete_releases_real_durable_writer_and_preserves_history"
            if (target not in durable or target not in main
                    or "--test stabilization_acp_durable" not in durable
                    or "--retries 0" not in durable or "--test-threads 1" not in durable
                    or "--no-tests=fail" not in durable or "shell: bash" not in durable
                    or (main and durable and job.index(durable) >= job.index(main))):
                errors.append(f"{name}: durable exclusion lacks isolated execution")
            reuse = steps.get("Reuse source-bound native release binary", "")
            if "ci-build-artifact.py fetch" not in reuse or (smoke and reuse and job.index(reuse) >= job.index(smoke)):
                errors.append(f"{name}: native reuse missing or too late")
            if "cargo build --release -p wcore-cli" in job:
                errors.append(f"{name}: duplicate native release compilation")
    if "Seal reusable native build identity" not in jobs.get("build", ""):
        errors.append("build producer has no identity seal")
    if "build-identity.json" not in jobs.get("build", ""):
        errors.append("build producer does not upload identity")
    return errors


class Contract(unittest.TestCase):
    def setUp(self):
        self.source = WORKFLOW.read_text()

    def test_current_workflow(self):
        self.assertEqual(audit(self.source), [])

    def test_missing_execution_cannot_hide_behind_exclusion(self):
        changed = self.source.replace("-E 'test(=" + BUDGET + ")'", "-E 'test(no_such_test)'", 1)
        self.assertNotEqual(changed, self.source)
        self.assertTrue(any("isolated budget" in error for error in audit(changed)))

    def test_durable_exclusion_cannot_drop_execution(self):
        changed = self.source.replace("--test stabilization_acp_durable", "--test missing_target", 1)
        self.assertTrue(any("durable exclusion" in error for error in audit(changed)))

    def test_duplicate_smoke_is_detected(self):
        changed = self.source.replace("not binary_id(=wcore-cli::release_binary_smoke)", "all()", 2)
        self.assertTrue(any("smoke duplicated" in error for error in audit(changed)))

    def test_whole_suite_retry_is_detected(self):
        self.assertTrue(audit(self.source + "\n        uses: nick-fields/retry@v3\n"))

    def test_historical_comments_are_not_commands(self):
        self.assertEqual(audit(self.source + "\n# nick-fields/retry@v3\n"), [])

    def test_uncaptured_workspace_is_detected(self):
        changed = self.source.replace("run-tests-with-attempt-evidence.sh", "unrecorded-command.sh")
        self.assertTrue(any("workspace evidence" in error for error in audit(changed)))

    def test_unsealed_producer_is_detected(self):
        changed = self.source.replace("Seal reusable native build identity", "Unsealed build")
        self.assertTrue(any("identity seal" in error for error in audit(changed)))

    def test_windows_default_shell_cannot_parse_bash_implicitly(self):
        changed = self.source.replace("        shell: bash\n", "", 1)
        # Remove the exact native reuse shell even if an earlier unrelated
        # Bash step occupied the first occurrence.
        changed = changed.replace("      - name: Reuse source-bound native release binary\n        shell: bash\n",
                                  "      - name: Reuse source-bound native release binary\n", 1)
        self.assertTrue(any("explicit Bash" in error for error in audit(changed)))

    def test_recovery_exclusion_cannot_drop_execution(self):
        changed = self.source.replace("--test stabilization_recovery", "--test missing_target", 1)
        self.assertTrue(any("stabilization_recovery exclusion" in error for error in audit(changed)))

    def test_primary_windows_is_not_claimed_as_private_fleet(self):
        text = (ROOT / ".github/scripts/annotate-windows-coverage.sh").read_text()
        self.assertIn("(primary Windows route) | $SELF_HOSTED_STATE", text)
        self.assertNotIn("(self-hosted) | $SELF_HOSTED_STATE", text)

    def test_consumer_must_wait_for_its_own_bounded_producer(self):
        changed = self.source.replace("--wait-seconds 4500", "--wait-seconds 1", 1)
        self.assertTrue(any("own-target build dependency" in error for error in audit(changed)))


if __name__ == "__main__":
    unittest.main()
