#!/usr/bin/env python3
"""Recover the approved signed candidate without rebuilding or relabeling evidence."""
import argparse
import base64
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import tempfile
import time
import zipfile

REPOSITORY = "FerroxLabs/wayland-core"
PRIOR_RUN = "34300869306"
CONTROL_SHA = "8d311bb8e75a85fda66eb046b0689de65af35322"
SOURCE_SHA = "28634521b854d58672819843865f6ab7dd16d72c"
TAG = "v0.13.13"
CANDIDATE_ID = 10089972966
UPLOAD_JOB = 102350682227
MANIFEST_SHA256 = "bedc5226e7addca5c20ab62acc7f5b9b9afbf0578e48d8b31adfb5af2a956129"
MUTANT_JOB = "Collect original release mutation receipts"
EXCLUDED_DIRS = frozenset(("remove-close-admission", "drop-cancellation", "retain-old-policy",
    "swallow-secure-delete-failure", "remove-refresh-writer-ordering", "bypass-auxiliary-reservation",
    "refund-uncertain-daily-claim", "ignore-host-tool-restrictions"))
TARGETS = ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-apple-darwin",
           "aarch64-apple-darwin", "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc")


def require(ok, why):
    if not ok:
        raise ValueError(why)


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def api(endpoint):
    return json.loads(subprocess.check_output(["gh", "api", endpoint]))


def authenticate(run, upload, jobs, candidate, mutants):
    require(str(run.get("id")) == PRIOR_RUN and run.get("head_sha") == CONTROL_SHA
            and run.get("head_repository", {}).get("full_name") == REPOSITORY
            and run.get("event") == "workflow_dispatch"
            and run.get("path") == ".github/workflows/release.yml", "foreign candidate producer")
    require(upload.get("id") == UPLOAD_JOB and str(upload.get("run_id")) == PRIOR_RUN
            and upload.get("head_sha") == CONTROL_SHA
            and upload.get("name") == "Upload GitHub Release assets", "wrong upload producer")
    steps = {step["name"]: step.get("conclusion") for step in upload.get("steps", [])}
    for name in ("Attest build provenance (keyless, Sigstore-backed)",
                 "Build and sign the release manifest", "Verify the signed manifest against the SHIPPED trust root",
                 "Preserve exact private promotion candidate"):
        require(steps.get(name) == "success", "missing successful producer step: " + name)
    require(any(job.get("name") == MUTANT_JOB and job.get("conclusion") == "success"
                and str(job.get("run_id")) == PRIOR_RUN and job.get("head_sha") == CONTROL_SHA
                for job in jobs), "missing successful original mutation producer")
    for artifact, name in ((candidate, "private-release-candidate"), (mutants, "release-mutants-raw")):
        origin = artifact.get("workflow_run", {})
        require(artifact.get("name") == name and not artifact.get("expired")
                and str(origin.get("id")) == PRIOR_RUN and origin.get("head_sha") == CONTROL_SHA,
                "foreign artifact origin")
        require(re.fullmatch(r"sha256:[0-9a-f]{64}", artifact.get("digest", "")), "missing artifact digest")
    require(candidate.get("id") == CANDIDATE_ID, "unapproved private candidate")


def unpack(archive, output):
    """Validate every ZIP member before writing any; never materialize links."""
    with zipfile.ZipFile(archive) as bundle:
        seen = set()
        for entry in bundle.infolist():
            path = PurePosixPath(entry.filename)
            require(entry.filename and not path.is_absolute() and ".." not in path.parts
                    and "\\" not in entry.filename and ":" not in entry.filename,
                    "unsafe archive path")
            require(entry.filename not in seen, "duplicate archive member")
            seen.add(entry.filename)
            mode = entry.external_attr >> 16
            require(not stat.S_ISLNK(mode) and (not stat.S_IFMT(mode)
                    or stat.S_ISREG(mode) or stat.S_ISDIR(mode)), "nonregular archive member")
        output.mkdir(parents=True, exist_ok=False)
        bundle.extractall(output)


def download(artifact, archive, output):
    with archive.open("wb") as stream:
        subprocess.run(["gh", "api", f"repos/{REPOSITORY}/actions/artifacts/{artifact['id']}/zip"],
                       stdout=stream, check=True)
    require("sha256:" + digest(archive) == artifact["digest"], "downloaded artifact digest mismatch")
    unpack(archive, output)


def verify_signature(manifest, trust, temporary):
    body_digest = hashlib.sha256(json.dumps(manifest["body"], separators=(",", ":"),
                                             ensure_ascii=False).encode()).hexdigest()
    require(body_digest == manifest.get("body_sha256"), "manifest body digest mismatch")
    keys = [key for key in trust["keys"] if key["key_id"] == manifest["authority"]["key_id"]]
    require(len(keys) == 1, "ambiguous manifest authority")
    key = keys[0]
    require(key["role"] == "release_acceptance" and key["valid_from"] <= time.time()
            and (key["retired_at"] is None or time.time() < key["retired_at"]), "inactive manifest authority")
    public = base64.b64decode(key["public_key_base64"], validate=True)
    signature = base64.b64decode(manifest["authority"]["signature_base64"], validate=True)
    require(len(public) == 32 and len(signature) == 64, "invalid signature encoding")
    (temporary / "key.der").write_bytes(bytes.fromhex("302a300506032b6570032100") + public)
    (temporary / "signature").write_bytes(signature)
    (temporary / "message").write_bytes(b"wayland.release.manifest.v1\0" + body_digest.encode())
    subprocess.run(["openssl", "pkeyutl", "-verify", "-pubin", "-inkey", str(temporary / "key.der"),
                    "-keyform", "DER", "-sigfile", str(temporary / "signature"), "-rawin",
                    "-in", str(temporary / "message")], check=True)


def select_candidate(candidate, output, trust):
    name = f"wayland-core-{TAG}-release-manifest.json"
    manifest_path = candidate / name
    require(manifest_path.is_file() and not manifest_path.is_symlink(), "missing signed manifest")
    manifest = json.loads(manifest_path.read_text())
    require(manifest.get("schema") == "wayland.release.manifest" and manifest.get("schema_version") == 1,
            "invalid manifest schema")
    require(manifest["body"].get("release_id") == TAG
            and manifest["body"].get("source_commit") == SOURCE_SHA, "stale manifest source")
    with tempfile.TemporaryDirectory() as temp:
        verify_signature(manifest, trust, Path(temp))
    require(digest(manifest_path) == MANIFEST_SHA256, "unapproved signed manifest bytes")
    expected = {f"wayland-core-{TAG}-{target}." + ("zip" if "windows" in target else "tar.gz")
                for target in TARGETS}
    expected |= {f"wayland-core-{TAG}-desktop-contract-v1.tar.gz", f"wayland-core-{TAG}-sbom.json",
                 "wayland-core-checksums.txt", "index.json", "release-metadata-receipt.json",
                 "toolchain.stderr.log", "toolchain.stdout.log"}
    rows = manifest["body"]["artifacts"]
    names = [row["name"] for row in rows]
    require(len(names) == len(set(names)) and set(names) == expected, "unapproved signed asset set")
    require({p.name for p in candidate.iterdir() if p.is_dir()} == EXCLUDED_DIRS,
            "unexpected excluded directories")
    require({p.name for p in candidate.iterdir() if not p.is_dir()} == expected | {name},
            "unexpected candidate files")
    for row in rows:
        filename = row["name"]
        require(Path(filename).name == filename and "/" not in filename and "\\" not in filename,
                "unsafe signed basename")
        path = candidate / filename
        require(path.is_file() and not path.is_symlink(), "missing regular signed asset")
        require(path.stat().st_size == row["byte_length"] and digest(path) == row["sha256"],
                "signed asset digest or length mismatch")
    output.mkdir(parents=True, exist_ok=False)
    for filename in sorted(expected | {name}):
        shutil.copyfile(candidate / filename, output / filename)
    return {p.name: digest(p) for p in output.iterdir()}


def validate_mutants(folder):
    index = json.loads((folder / "index.json").read_text())
    require(index.get("source_sha") == SOURCE_SHA and not index.get("blockers"), "blocked or stale mutants")
    require(index.get("provenance") == {"source_sha": SOURCE_SHA, "repository": REPOSITORY,
            "run_id": PRIOR_RUN, "job": "collect-release-mutants"}, "mutation origin mismatch")
    require(len(index.get("mutants", [])) == 8
            and {row["id"] for row in index["mutants"]} == EXCLUDED_DIRS, "missing original mutants")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prior-run", required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    require(args.prior_run == PRIOR_RUN, "unapproved recovery run")
    source_sha = subprocess.check_output(["git", "-C", str(args.source), "rev-parse", "HEAD"], text=True).strip()
    require(source_sha == SOURCE_SHA, "stale checked-out source")
    base = f"repos/{REPOSITORY}/actions"
    run = api(f"{base}/runs/{PRIOR_RUN}")
    upload = api(f"{base}/jobs/{UPLOAD_JOB}")
    jobs = api(f"{base}/runs/{PRIOR_RUN}/jobs?filter=all&per_page=100")["jobs"]
    artifacts = api(f"{base}/runs/{PRIOR_RUN}/artifacts?per_page=100")["artifacts"]
    candidates = [row for row in artifacts if row["name"] == "private-release-candidate"]
    mutants = [row for row in artifacts if row["name"] == "release-mutants-raw"]
    require(len(candidates) == len(mutants) == 1, "ambiguous prior artifacts")
    authenticate(run, upload, jobs, candidates[0], mutants[0])
    args.output.mkdir(parents=True, exist_ok=False)
    download(candidates[0], args.output / "candidate.zip", args.output / "original")
    download(mutants[0], args.output / "mutants.zip", args.output / "mutants")
    source = (args.source / "crates/wcore-cli/src/update_trust.rs").read_text()
    matches = re.findall(r'pub\s+const\s+RELEASE_TRUST_ROOT_JSON\s*:\s*&str\s*=\s*r#"(.*?)"#;', source, re.S)
    require(len(matches) == 1, "missing bundled trust root")
    assets = select_candidate(args.output / "original", args.output / "candidate", json.loads(matches[0]))
    validate_mutants(args.output / "mutants")
    receipt = {"source_sha": SOURCE_SHA, "tag": TAG, "prior_run": PRIOR_RUN,
               "prior_workflow_sha": CONTROL_SHA, "candidate_artifact": candidates[0],
               "mutant_artifact": mutants[0], "artifacts": assets,
               "excluded_unsigned_directories": sorted(EXCLUDED_DIRS), "status": "passed"}
    (args.output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    main()
