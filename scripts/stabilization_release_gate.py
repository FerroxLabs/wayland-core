#!/usr/bin/env python3
"""W12 private staging and behavioral admission (stdlib only).

Evidence schema 1 is a trusted CI artifact, NOT a ledger or user assertion.
The caller must authenticate its producing run and pin that run to source_sha.
Required fields: source_sha, tag, artifacts {basename: sha256}, tests [{id,
source_sha, selected, passed, failed, skipped, retries}], mutants [{id,
source_sha, viable, selected, caught}], native [{id, target, source_sha, artifact,
sha256, startup, exit_code, native, outcome, disposition}], deferred_risks [{id, severity, disposition}].
Required W14 Windows checks execute natively. Header checks do not count.
Standing ACL nonpass remains quarantined, never represented as passing.
Stage is private and may run without receipts. Promote authenticates receipts
and compares EVERY remote asset before changing visibility. Never clobbers.
Withdrawal makes the same release private again; it cannot revoke downloaded
copies, npm packages or notifications already delivered.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile

TARGETS = (
    "aarch64-apple-darwin", "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu", "x86_64-unknown-linux-gnu",
    "aarch64-pc-windows-msvc", "x86_64-pc-windows-msvc",
)
NATIVE_CHECKS = ("quarantine_console_authority_windows",
                 "quarantine_terminal_authority_windows", "live_fs_acl",
                 "hard_process_containment_windows")
MUTANTS = (
    "remove-close-admission", "drop-cancellation", "retain-old-policy",
    "swallow-secure-delete-failure", "remove-refresh-writer-ordering",
    "bypass-auxiliary-reservation", "refund-uncertain-daily-claim",
    "ignore-host-tool-restrictions",
)


def require(ok, reason):
    if not ok:
        raise ValueError(reason)


def inventory(directory):
    files = sorted(Path(directory).iterdir())
    require(files, "empty artifact set")
    require(all(p.is_file() and not p.is_symlink() for p in files),
            "artifacts must be regular files")
    return {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in files}


def indexed(rows, field):
    require(isinstance(rows, list) and rows, "missing " + field + " receipts")
    result = {row[field]: row for row in rows}
    require(len(result) == len(rows), "duplicate " + field)
    return result


def validate(evidence, sha, tag, assets):
    require(re.fullmatch(r"[0-9a-f]{40}", sha), "expected full source SHA")
    require(evidence.get("schema") == 1, "unsupported receipt schema")
    require(evidence.get("source_sha") == sha, "stale source SHA")
    require(evidence.get("tag") == tag, "wrong release tag")
    require(evidence.get("artifacts") == assets, "accepted artifact identity mismatch")
    require(assets and all(re.fullmatch(r"[0-9a-f]{64}", h) for h in assets.values()),
            "invalid artifact digest")
    tests = indexed(evidence.get("tests"), "id")
    for row in tests.values():
        require(row.get("source_sha") == sha, "stale test receipt")
        require(type(row.get("selected")) is int and row["selected"] > 0,
                "zero selected tests")
        require(row.get("passed") == row["selected"] and
                all(row.get(k) == 0 for k in ("failed", "skipped", "retries")),
                "tests incomplete, failed or retried")
    mutants = indexed(evidence.get("mutants"), "id")
    require(set(mutants) == set(MUTANTS), "required mutant set mismatch")
    for row in mutants.values():
        require(row.get("source_sha") == sha, "stale mutant receipt")
        require(row.get("viable") is True and type(row.get("selected")) is int and row["selected"] == 1
                and row.get("caught") is True, "required mutant not viable/run/caught")
    for target in TARGETS:
        ext = "zip" if "windows" in target else "tar.gz"
        require(f"wayland-core-{tag}-{target}.{ext}" in assets,
                "missing distributed artifact")
    native = indexed(evidence.get("native"), "id")
    require(set(native) == set(NATIVE_CHECKS), "missing required native receipt")
    for check, row in native.items():
        require(row.get("target") == "x86_64-pc-windows-msvc", "wrong native executor")
        name = f"wayland-core-{tag}-x86_64-pc-windows-msvc.zip"
        require(row.get("source_sha") == sha, "stale native receipt")
        require(row.get("artifact") == name and row.get("sha256") == assets[name],
                "native artifact identity mismatch")
        require(row.get("native") is True and
                row.get("startup") == "wayland-core " + tag.removeprefix("v"),
                "wrong startup or non-native execution")
        require(type(row.get("selected")) is int and row["selected"] > 0,
                "zero selected native tests")
        passing = row.get("outcome") == "passed" and row.get("exit_code") == 0
        quarantine = (check == "live_fs_acl" and row.get("outcome") == "quarantined"
                      and type(row.get("exit_code")) is int and row["exit_code"] != 0
                      and row.get("disposition") == "Q-368-disposition; core#368; core#410")
        require(passing or quarantine, "native check failed without standing disposition")
    risks = indexed(evidence.get("deferred_risks"), "id")
    require({"core#368", "core#410"} <= set(risks), "standing Windows risks absent")
    for row in risks.values():
        require(row.get("severity") in ("critical", "high", "medium", "low") and
                isinstance(row.get("disposition"), str) and row["disposition"].strip(),
                "deferred risk lacks severity/disposition")
    return "\n".join(f"- {r['id']} ({r['severity']}): {r['disposition']}"
                     for r in risks.values())


class Github:
    def __init__(self, repo, tag):
        self.repo, self.tag = repo, tag

    def run(self, *args):
        return subprocess.check_output(["gh", *args], text=True)

    def view(self):
        # List successfully first: an auth/network failure is never 'absent'.
        rows = json.loads(self.run("api", "--paginate", "--slurp",
                                  f"repos/{self.repo}/releases?per_page=100"))
        found = [r for page in rows for r in page if r["tag_name"] == self.tag]
        require(len(found) <= 1, "duplicate release")
        return found[0] if found else None

    def create(self):
        self.run("release", "create", self.tag, "--repo", self.repo,
                 "--draft", "--verify-tag", "--title", self.tag, "--generate-notes")

    def hashes(self, release):
        if not release["assets"]:
            return {}
        with tempfile.TemporaryDirectory() as directory:
            self.run("release", "download", self.tag, "--repo", self.repo,
                     "--dir", directory)
            result = inventory(directory)
        require(set(result) == {a["name"] for a in release["assets"]},
                "remote asset set changed during download")
        return result

    def upload(self, path):
        self.run("release", "upload", self.tag, str(path), "--repo", self.repo)

    def visibility(self, draft, notes=None):
        args = ["release", "edit", self.tag, "--repo", self.repo,
                "--draft=" + str(draft).lower()]
        if not draft:
            args += ["--prerelease=" + str("-" in self.tag).lower()]
        if notes is not None:
            args += ["--notes", notes]
        self.run(*args)


def transition(backend, directory, action, risk_notes=""):
    assets = inventory(directory)
    release = backend.view()
    if action == "withdraw":
        require(release is not None, "cannot withdraw absent release")
        require(backend.hashes(release) == assets, "withdrawal identity mismatch")
        backend.visibility(True)
        return
    if release is None:
        require(action == "stage", "promotion requires private staging")
        backend.create()
        release = backend.view()
    remote = backend.hashes(release)
    require(all(name in assets and assets[name] == digest for name, digest in remote.items()),
            "remote artifact mismatch; refusing overwrite")
    if action == "stage":
        if not release["draft"]:
            require(remote == assets, "published release differs")
            return
        for name in sorted(assets.keys() - remote.keys()):
            backend.upload(Path(directory) / name)
        require(backend.hashes(backend.view()) == assets, "incomplete private upload")
    else:
        require(remote == assets, "promotion requires exact complete asset set")
        marker = "\n\n### Deferred release risks\n"
        notes = (release.get("body") or "").split(marker)[0] + marker + risk_notes
        backend.visibility(False, notes)


def self_test():
    sha, tag = "a" * 40, "v1.2.3"
    with tempfile.TemporaryDirectory() as directory:
        for target in TARGETS:
            ext = "zip" if "windows" in target else "tar.gz"
            (Path(directory) / f"wayland-core-{tag}-{target}.{ext}").write_bytes(target.encode())
        assets = inventory(directory)
        evidence = dict(schema=1, source_sha=sha, tag=tag, artifacts=assets,
            tests=[dict(id="contract", source_sha=sha, selected=1, passed=1, failed=0, skipped=0, retries=0)],
            mutants=[dict(id=m, source_sha=sha, viable=True, selected=1, caught=True) for m in MUTANTS],
            native=[dict(id=c, target="x86_64-pc-windows-msvc", source_sha=sha,
                         artifact=f"wayland-core-{tag}-x86_64-pc-windows-msvc.zip",
                         sha256=assets[f"wayland-core-{tag}-x86_64-pc-windows-msvc.zip"],
                         native=True, exit_code=0, startup="wayland-core 1.2.3",
                         outcome="passed", selected=1) for c in NATIVE_CHECKS],
            deferred_risks=[dict(id=i, severity="high", disposition="quarantine; external decision")
                            for i in ("core#368", "core#410")])
        notes = validate(evidence, sha, tag, assets)
        quarantined = copy.deepcopy(evidence)
        acl = next(r for r in quarantined["native"] if r["id"] == "live_fs_acl")
        acl.update(outcome="quarantined", exit_code=1,
                   disposition="Q-368-disposition; core#368; core#410")
        validate(quarantined, sha, tag, assets)
        cases = [lambda e: e.update(source_sha="b" * 40),
                 lambda e: e["native"][0].update(startup="wayland-core 0.0.0"),
                 lambda e: e["tests"][0].update(selected=0),
                 lambda e: e["mutants"][0].update(viable=False),
                 lambda e: e["mutants"][0].update(caught=False),
                 lambda e: e["native"].pop(),
                 lambda e: e["native"][0].update(native=False),
                 lambda e: e["native"][0].update(selected=0),
                 lambda e: e["native"][0].update(exit_code=1),
                 lambda e: e["tests"][0].update(source_sha="b" * 40),
                 lambda e: e["tests"][0].update(retries=1),
                 lambda e: e["mutants"][0].update(source_sha="b" * 40),
                 lambda e: e["artifacts"].update(extra="0" * 64),
                 lambda e: e["deferred_risks"].clear()]
        for mutate in cases:
            bad = copy.deepcopy(evidence)
            mutate(bad)
            try:
                validate(bad, sha, tag, assets)
            except ValueError:
                continue
            raise AssertionError("negative receipt accepted")

        class Fake:
            release = None
            remote = {}
            interrupt = False
            def view(self):
                return self.release
            def create(self):
                self.release = dict(draft=True, body="release notes", assets=[])
                self.remote = {}
            def hashes(self, release):
                return dict(self.remote)
            def upload(self, path):
                self.remote[path.name] = hashlib.sha256(path.read_bytes()).hexdigest()
                if self.interrupt:
                    self.interrupt = False
                    raise InterruptedError("upload completed; response lost")
            def visibility(self, draft, notes=None):
                self.release["draft"] = draft
                if notes is not None:
                    self.release["body"] = notes
        fake = Fake()
        fake.interrupt = True
        try:
            transition(fake, directory, "stage")
        except InterruptedError:
            pass
        require(fake.release["draft"], "interruption exposed release")
        transition(fake, directory, "stage")
        transition(fake, directory, "promote", notes)
        first = copy.deepcopy(fake.release)
        transition(fake, directory, "promote", notes)
        require(fake.release == first and "core#410" in first["body"], "retry/risk loss")
        transition(fake, directory, "withdraw")
        require(fake.release["draft"], "withdrawal failed")
        fake.remote[next(iter(assets))] = "0" * 64
        for action in ("stage", "promote", "withdraw"):
            try:
                transition(fake, directory, action, notes)
            except ValueError:
                continue
            raise AssertionError("mismatch accepted")
    print("PASS: receipt refusals, private create/resume, promotion retry, withdrawal, mismatch")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--action", choices=("stage", "promote", "withdraw"))
    for field in ("artifacts", "evidence", "sha", "tag", "repo"):
        parser.add_argument("--" + field)
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    require(all((args.action, args.artifacts, args.sha, args.tag, args.repo)), "missing arguments")
    actual = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    require(actual == args.sha, "checkout SHA mismatch")
    tag_sha = subprocess.check_output(["git", "rev-parse", args.tag + "^{commit}"], text=True).strip()
    require(tag_sha == actual, "tag SHA mismatch")
    notes = ""
    if args.action == "promote":
        require(args.evidence, "missing behavioral receipts (W09/native prerequisites)")
        evidence = json.loads(Path(args.evidence).read_text())
        notes = validate(evidence, args.sha, args.tag, inventory(args.artifacts))
        print(notes)
    backend = Github(args.repo, args.tag)
    remote_sha = backend.run("api", f"repos/{args.repo}/commits/{args.tag}",
                             "--jq", ".sha").strip()
    require(remote_sha == args.sha, "remote tag SHA mismatch")
    transition(backend, args.artifacts, args.action, notes)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, TypeError, OSError, subprocess.CalledProcessError) as error:
        raise SystemExit("BLOCKED: " + str(error))
