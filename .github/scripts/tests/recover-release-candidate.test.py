#!/usr/bin/env python3
"""Signed fixtures and provenance negatives for bounded candidate recovery."""
import base64
import copy
import hashlib
import importlib.util
import json
import re
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("recover", ROOT / ".github/scripts/recover-release-candidate.py")
recover = importlib.util.module_from_spec(spec)
spec.loader.exec_module(recover)


class Recovery(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.candidate = self.root / "original"
        self.candidate.mkdir()
        self.output = self.root / "clean"
        subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-out", str(self.root / "private.pem")], check=True)
        public = subprocess.check_output(["openssl", "pkey", "-in", str(self.root / "private.pem"),
                                          "-pubout", "-outform", "DER"])[-32:]
        self.trust = {"keys": [{"key_id": "fixture", "role": "release_acceptance", "valid_from": 0,
                               "retired_at": None, "public_key_base64": base64.b64encode(public).decode()}]}
        names = {f"wayland-core-{recover.TAG}-{target}." + ("zip" if "windows" in target else "tar.gz")
                 for target in recover.TARGETS}
        names |= {f"wayland-core-{recover.TAG}-desktop-contract-v1.tar.gz", f"wayland-core-{recover.TAG}-sbom.json",
                  "wayland-core-checksums.txt", "index.json", "release-metadata-receipt.json",
                  "toolchain.stderr.log", "toolchain.stdout.log"}
        rows = []
        for name in sorted(names):
            path = self.candidate / name
            path.write_text("fixture " + name)
            rows.append({"name": name, "sha256": recover.digest(path), "byte_length": path.stat().st_size})
        for name in recover.EXCLUDED_DIRS:
            (self.candidate / name).mkdir()
            (self.candidate / name / "evidence.log").write_text("preserved evidence")
        self.manifest = {"schema": "wayland.release.manifest", "schema_version": 1,
                         "body": {"source_commit": recover.SOURCE_SHA, "release_id": recover.TAG, "artifacts": rows},
                         "authority": {"key_id": "fixture"}}
        self.manifest_path = self.candidate / f"wayland-core-{recover.TAG}-release-manifest.json"
        self.sign()

    def sign(self):
        body = json.dumps(self.manifest["body"], separators=(",", ":"), ensure_ascii=False).encode()
        self.manifest["body_sha256"] = hashlib.sha256(body).hexdigest()
        message = self.root / "message"
        message.write_bytes(b"wayland.release.manifest.v1\0" + self.manifest["body_sha256"].encode())
        signature = subprocess.check_output(["openssl", "pkeyutl", "-sign", "-inkey", str(self.root / "private.pem"),
                                             "-rawin", "-in", str(message)])
        self.manifest["authority"]["signature_base64"] = base64.b64encode(signature).decode()
        self.manifest_path.write_text(json.dumps(self.manifest))

    def select(self):
        with patch.object(recover, "MANIFEST_SHA256", recover.digest(self.manifest_path)):
            return recover.select_candidate(self.candidate, self.output, self.trust)

    def test_exact_signed_files_preserved_and_original_evidence_untouched(self):
        result = self.select()
        self.assertEqual(len(result), 14)
        for name, digest in result.items():
            self.assertEqual(recover.digest(self.candidate / name), digest)
        self.assertTrue(all((self.candidate / name / "evidence.log").is_file() for name in recover.EXCLUDED_DIRS))
        self.assertTrue(all(path.is_file() for path in self.output.iterdir()))

    def test_invalid_signature_refused(self):
        self.manifest["authority"]["signature_base64"] = base64.b64encode(b"x" * 64).decode()
        self.manifest_path.write_text(json.dumps(self.manifest))
        with self.assertRaises(subprocess.CalledProcessError):
            self.select()
        self.assertFalse(self.output.exists())

    def test_modified_manifest_body_refused(self):
        self.manifest["body"]["artifacts"][0]["sha256"] = "0" * 64
        self.manifest_path.write_text(json.dumps(self.manifest))
        with self.assertRaisesRegex(ValueError, "body digest"):
            self.select()

    def test_missing_and_changed_signed_asset_refused(self):
        path = self.candidate / self.manifest["body"]["artifacts"][0]["name"]
        path.write_text("tampered")
        with self.assertRaisesRegex(ValueError, "digest or length"):
            self.select()
        path.unlink()
        with self.assertRaisesRegex(ValueError, "candidate files"):
            self.select()

    def test_unapproved_or_unsafe_signed_name_refused(self):
        self.manifest["body"]["artifacts"][0]["name"] = "../escape"
        self.sign()
        with self.assertRaisesRegex(ValueError, "asset set"):
            self.select()

    def test_unapproved_regular_file_refused(self):
        (self.candidate / "unexpected").write_text("extra")
        with self.assertRaisesRegex(ValueError, "candidate files"):
            self.select()

    def test_unknown_excluded_directory_refused(self):
        (self.candidate / "unexpected").mkdir()
        with self.assertRaisesRegex(ValueError, "excluded directories"):
            self.select()

    def test_zip_path_escape_and_symlink_refused_before_extraction(self):
        for filename, mode in (("../escape", 0), ("/escape", 0), ("symlink", 0o120777 << 16)):
            with self.subTest(filename=filename):
                archive = self.root / "test.zip"
                with zipfile.ZipFile(archive, "w") as bundle:
                    entry = zipfile.ZipInfo(filename)
                    entry.external_attr = mode
                    bundle.writestr(entry, "escape")
                with self.assertRaises(ValueError):
                    recover.unpack(archive, self.output)
                self.assertFalse(self.output.exists())


class Provenance(unittest.TestCase):
    def setUp(self):
        self.run = {"id": int(recover.PRIOR_RUN), "head_sha": recover.CONTROL_SHA,
                    "head_repository": {"full_name": recover.REPOSITORY},
                    "event": "workflow_dispatch", "path": ".github/workflows/release.yml"}
        self.upload = {"id": recover.UPLOAD_JOB, "run_id": int(recover.PRIOR_RUN), "head_sha": recover.CONTROL_SHA,
                       "name": "Upload GitHub Release assets", "steps": [
                           {"name": name, "conclusion": "success"} for name in (
                               "Attest build provenance (keyless, Sigstore-backed)", "Build and sign the release manifest",
                               "Verify the signed manifest against the SHIPPED trust root", "Preserve exact private promotion candidate")]}
        self.jobs = [{"name": recover.MUTANT_JOB, "conclusion": "success", "run_id": int(recover.PRIOR_RUN),
                      "head_sha": recover.CONTROL_SHA}]
        self.candidate = {"id": recover.CANDIDATE_ID, "name": "private-release-candidate", "expired": False,
                          "digest": "sha256:" + "a" * 64,
                          "workflow_run": {"id": int(recover.PRIOR_RUN), "head_sha": recover.CONTROL_SHA}}
        self.mutants = {**copy.deepcopy(self.candidate), "name": "release-mutants-raw", "id": 10}

    def auth(self):
        recover.authenticate(self.run, self.upload, self.jobs, self.candidate, self.mutants)

    def test_failed_later_stage_does_not_discard_successful_signed_upload(self):
        self.upload["conclusion"] = "failure"
        self.auth()

    def test_foreign_producer_refused(self):
        self.run["head_repository"]["full_name"] = "foreign/repo"
        with self.assertRaisesRegex(ValueError, "foreign candidate"):
            self.auth()

    def test_failed_signing_or_upload_refused(self):
        for step in self.upload["steps"]:
            step["conclusion"] = "failure"
            with self.assertRaisesRegex(ValueError, "producer step"):
                self.auth()
            step["conclusion"] = "success"

    def test_foreign_mutation_artifact_refused(self):
        self.mutants["workflow_run"]["id"] = 99
        with self.assertRaisesRegex(ValueError, "artifact origin"):
            self.auth()

    def test_mutation_failure_refused(self):
        self.jobs[0]["conclusion"] = "failure"
        with self.assertRaisesRegex(ValueError, "mutation producer"):
            self.auth()


class Topology(unittest.TestCase):
    def setUp(self):
        self.text = (ROOT / ".github/workflows/release.yml").read_text()
        matches = list(re.finditer(r"^  ([a-z][a-z0-9-]*):$", self.text, re.M))
        self.jobs = {m.group(1): self.text[m.end():matches[i + 1].start() if i + 1 < len(matches) else len(self.text)]
                     for i, m in enumerate(matches) if m.group(1) not in ("push", "workflow-call", "workflow-dispatch")}

    def simulate(self, prior, fail=None, cancelled=False):
        results = {}
        ancestors = {}
        for name, text in self.jobs.items():
            match = re.search(r"^    needs: (.+)$", text, re.M)
            if match:
                needs = match.group(1).strip("[]").replace(",", " ").split()
            else:
                match = re.search(r"^    needs:\n((?:      - .+\n)+)", text, re.M)
                needs = re.findall(r"      - (.+)", match.group(1)) if match else []
            ancestors[name] = set(needs).union(*(ancestors[need] for need in needs))
            expression = re.search(r"^    if: (.+)(?:\n((?:      .+\n)+))?", text, re.M)
            if expression:
                condition = expression.group(1)
                if condition == ">-":
                    condition = expression.group(2).strip()
                condition = condition.replace("${{", "").replace("}}", "").replace("!cancelled()", repr(not cancelled))
                condition = condition.replace("inputs.prior_candidate_run", repr(prior))
                condition = re.sub(r"needs.([a-z-]+).result", lambda m: repr(results[m.group(1)]), condition)
                condition = " ".join(condition.replace("&&", " and ").replace("||", " or ").split())
                allowed = eval(condition, {"__builtins__": {}}, {})
                if "!cancelled()" not in expression.group(0):
                    allowed = allowed and not cancelled and all(results[need] == "success" for need in ancestors[name])
            else:
                allowed = not cancelled and all(results[need] == "success" for need in ancestors[name])
            results[name] = ("failure" if name == fail else "success") if allowed else "skipped"
        return results

    def test_default_and_recovery_admit_only_their_own_producers(self):
        for prior in ("", recover.PRIOR_RUN):
            result = self.simulate(prior)
            self.assertEqual(result["recover-candidate"], "success" if prior else "skipped")
            for name in ("build", "collect-release-mutants"):
                self.assertEqual(result[name], "skipped" if prior else "success")
            for name in ("github-release", "post-tag-smoke", "collect-release-native",
                         "produce-release-evidence", "promote-release", "publish-npm"):
                self.assertEqual(result[name], "success", (prior, name))

    def test_every_required_failure_blocks_promotion(self):
        for fail in ("prepare-release", "recover-candidate", "github-release", "post-tag-smoke",
                     "collect-release-native", "produce-release-evidence"):
            with self.subTest(fail=fail):
                self.assertNotEqual(self.simulate(recover.PRIOR_RUN, fail)["promote-release"], "success")
        for fail in ("build", "collect-release-mutants"):
            self.assertNotEqual(self.simulate("", fail)["promote-release"], "success")

    def test_transitive_skips_require_explicit_status_on_every_consumer(self):
        for job in ("post-tag-smoke", "collect-release-native", "promote-release", "publish-npm"):
            with self.subTest(job=job):
                original = self.jobs[job]
                self.jobs[job] = re.sub(r"^    if: >-\n(?:      .+\n)+", "", original, flags=re.M)
                self.assertNotEqual(self.jobs[job], original)
                self.assertEqual(self.simulate(recover.PRIOR_RUN)[job], "skipped")
                self.jobs[job] = original

    def test_cancelled_run_cannot_admit_any_job(self):
        self.assertTrue(all(status == "skipped" for status in
                            self.simulate(recover.PRIOR_RUN, cancelled=True).values()))

    def test_every_explicit_consumer_requires_all_direct_parents(self):
        parents = {
            "post-tag-smoke": ("prepare-release", "github-release"),
            "collect-release-native": ("prepare-release", "github-release"),
            "promote-release": ("prepare-release", "github-release", "post-tag-smoke", "produce-release-evidence"),
            "publish-npm": ("prepare-release", "promote-release"),
        }
        for job, needs in parents.items():
            expression = re.search(r"^    if: >-\n((?:      .+\n)+)", self.jobs[job], re.M).group(1)
            self.assertIn("!cancelled()", expression)
            for parent in needs:
                self.assertIn(f"needs.{parent}.result == 'success'", expression)
                for status in ("failure", "skipped", "cancelled"):
                    condition = expression.replace("${{", "").replace("}}", "").replace("!cancelled()", "True")
                    condition = re.sub(r"needs.([a-z-]+).result",
                                       lambda m: repr(status if m.group(1) == parent else "success"), condition)
                    condition = " ".join(condition.replace("&&", " and ").split())
                    self.assertFalse(eval(condition, {"__builtins__": {}}, {}), (job, parent, status))

    def test_ordinary_downloads_only_named_distributed_assets(self):
        job = self.jobs["github-release"]
        downloads = re.findall(r"uses: actions/download-artifact@v8\n        with:\n          name: (.+)", job)
        self.assertEqual(set(downloads), set(recover.TARGETS) | {"desktop-contract-v1"})
        self.assertEqual(len(downloads), 7)
        self.assertNotIn("merge-multiple: true", job)
        self.assertIn("if: steps.resume.outputs.found != 'true'", job)


if __name__ == "__main__":
    unittest.main()
