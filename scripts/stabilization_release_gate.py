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

Producer entrypoint:
  --action produce --inputs RAW/index.json --artifacts PRIVATE --sha SHA
  --tag TAG --output evidence/receipt.json
RAW/index.json has source_sha, tests/mutants/native arrays and deferred_risks.
Each row has id, proof and junit refs: {path: relative_path, sha256: hex}.
Existing remote-proof rows instead have proof, status and log refs. A mutant
also has a baseline row and patch ref. Native rows may name the standing
Q-368 disposition; their measured command must select their named check.
Capture existing commands before collecting their files into RAW:
  --action capture --junit target/nextest/default/junit.xml --output proof.json
  [--archive exact-windows-candidate.zip] -- cargo nextest run ...
Capture records live source, diff, executor, status, fresh JUnit digest and,
for native commands, the actual archive digest and startup output.
CI ci-linux wraps its existing packaged_driver_gate and uploads
stabilization-raw-SHA. Release produce-release-evidence authenticates that
run, downloads its raw outputs and the immutable private candidate, and
uploads stabilization-evidence-SHA/receipt.json even when admission blocks.
The ordinary CI bundle contains no invented W09/W14 results: a final collector
must add their actual recorded command outputs under the same raw schema.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
import platform
import time
import xml.etree.ElementTree as ET
import zipfile

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
    require(not evidence.get("blockers"), "producer reports blocked prerequisites")
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


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def junit_counts(path):
    """Count actual nextest testcases, never a caller-supplied pass count."""
    cases = list(ET.parse(path).getroot().iter("testcase"))
    skipped = sum(c.find("skipped") is not None for c in cases)
    failed = sum(c.find("failure") is not None or c.find("error") is not None for c in cases)
    retries = sum(len(c.findall(k)) for c in cases for k in
                  ("rerunFailure", "rerunError", "flakyFailure", "flakyError"))
    return dict(selected=len(cases), passed=len(cases) - skipped - failed,
                failed=failed, skipped=skipped, retries=retries)


def read_ref(root, ref):
    path = root / ref["path"]
    require(path.resolve().is_relative_to(root.resolve()) and not path.is_symlink(),
            "evidence path escapes bundle")
    require(path.is_file() and digest(path) == ref["sha256"], "raw evidence digest mismatch")
    return path


def command_result(root, entry, sha):
    """Accept measured capture receipts or the existing remote-proof receipt."""
    proof = json.loads(read_ref(root, entry["proof"]).read_text())
    require(proof["source"] == sha and proof["complete"] is True, "stale/incomplete raw execution")
    argv = proof.get("argv", proof.get("cargo_args", []))
    require("nextest" in argv and "run" in argv and "--no-tests=fail" in argv,
            "raw execution lacks strict test selection")
    require(any(argv[i:i+2] == ["--retries", "0"] for i in range(len(argv))),
            "raw execution must disable retries")
    if proof.get("schema") == "stabilization-command-1":
        junit = read_ref(root, entry["junit"])
        counts = junit_counts(junit)
        require(proof["junit_sha256"] == digest(junit), "JUnit does not belong to execution")
        code = proof["exit_code"]
    else:
        # Existing remote-proof.py .status establishes process completion;
        # JUnit must also carry an explicit digest in the imported raw bundle.
        require(Path(entry["proof"]["path"]).stem == Path(entry["status"]["path"]).stem ==
                Path(entry["log"]["path"]).stem, "remote proof/status/log nonce mismatch")
        status = read_ref(root, entry["status"]).read_text()
        require(hashlib.sha256(status.encode()).hexdigest() == proof["status_sha256"],
                "remote status identity mismatch")
        require(status.endswith("DONE\n") and f"SOURCE={sha}\n" in status,
                "remote execution did not complete")
        code = proof["remote_exit"]
        require(f"EXIT_CODE={code}\n" in status, "remote exit mismatch")
        log = read_ref(root, entry["log"]).read_text()
        summaries = re.findall(r"Summary \[.*?\] (\d+) tests? run: (.*)", log)
        require(len(summaries) == 1, "remote log needs one completed nextest summary")
        selected, summary = summaries[0]
        numbers = {name: int(number) for number, name in re.findall(r"(\d+) (passed|failed|skipped)", summary)}
        require("passed" in numbers and "skipped" in numbers, "incomplete remote summary")
        require(not re.search(r"flaky|retried|timed out|leaked", summary), "remote retry/incomplete test")
        counts = dict(selected=int(selected), passed=numbers["passed"],
                      failed=numbers.get("failed", 0), skipped=numbers["skipped"], retries=0)
        require(counts["passed"] + counts["failed"] == counts["selected"], "remote counts disagree")
    require(type(code) is int, "missing process exit status")
    return proof, code, counts


def produce(index_path, artifacts, sha, tag, output):
    """Normalize actual outputs; always retain a diagnostic blocked receipt."""
    root = Path(index_path).parent
    data = json.loads(Path(index_path).read_text())
    assets = inventory(artifacts)
    result = dict(schema=1, source_sha=sha, tag=tag, artifacts=assets,
                  tests=[], mutants=[], native=[], deferred_risks=data.get("deferred_risks", []),
                  blockers=[])
    blockers = result["blockers"]
    if data.get("source_sha") != sha or data.get("tag", tag) != tag:
        blockers.append("raw bundle source/tag mismatch")
    for kind in ("tests", "mutants", "native"):
        for row in data.get(kind, []):
            try:
                proof, code, counts = command_result(root, row, sha)
                if kind == "tests":
                    require(code == 0, "test command failed")
                    require(not proof.get("diff"), "baseline contains source changes")
                    result[kind].append(dict(id=row["id"], source_sha=sha, **counts))
                elif kind == "mutants":
                    require(row["id"] in MUTANTS, "unexpected mutant")
                    baseline_proof, baseline_code, baseline = command_result(root, row["baseline"], sha)
                    require(not baseline_proof.get("diff"), "mutant baseline contains source changes")
                    require(baseline_code == 0 and baseline["selected"] > 0 and
                            baseline["passed"] == baseline["selected"] and baseline["retries"] == 0,
                            "mutant baseline is not passing")
                    patch = read_ref(root, row["patch"]).read_text()
                    require(patch and proof.get("diff") == patch, "tested mutation patch mismatch")
                    require(counts["selected"] == baseline["selected"] and counts["skipped"] == 0
                            and counts["retries"] == 0, "mutation selection changed")
                    result[kind].append(dict(id=row["id"], source_sha=sha,
                        selected=1 if counts["selected"] > 0 else 0,
                        viable=counts["selected"] > 0, caught=code != 0 and counts["failed"] > 0))
                else:
                    require(row["id"] in NATIVE_CHECKS, "unexpected native check")
                    require(row["id"] in proof.get("argv", []), "native command selected another check")
                    require(counts["skipped"] == 0 and counts["retries"] == 0, "native checks skipped/retried")
                    require(proof.get("platform") == "Windows" and
                            proof.get("machine", "").lower() in ("amd64", "x86_64"),
                            "missing real Windows executor")
                    require(not proof.get("diff"), "native source differs from candidate")
                    name = f"wayland-core-{tag}-x86_64-pc-windows-msvc.zip"
                    require(proof.get("artifact_sha256") == assets.get(name),
                            "native command did not use candidate archive")
                    require(proof.get("startup_exit") == 0, "native startup failed")
                    passed = code == 0 and counts["selected"] > 0 and counts["passed"] == counts["selected"]
                    disposition = row.get("disposition")
                    outcome = "passed" if passed else "failed"
                    if (row["id"] == "live_fs_acl" and code != 0 and counts["failed"] > 0
                            and disposition == "Q-368-disposition; core#368; core#410"):
                        outcome = "quarantined"
                    result[kind].append(dict(id=row["id"], source_sha=sha,
                        target="x86_64-pc-windows-msvc", artifact=name, sha256=assets[name],
                        native=True, selected=counts["selected"], exit_code=code,
                        startup=proof.get("startup"), outcome=outcome, disposition=disposition))
            except (ValueError, KeyError, TypeError, OSError, ET.ParseError) as error:
                blockers.append(f"{kind}/{row.get('id', '?')}: {error}")
    for kind, expected in (("mutants", MUTANTS), ("native", NATIVE_CHECKS)):
        missing = set(expected) - {row["id"] for row in result[kind]}
        if missing:
            blockers.append(f"missing {kind}: {', '.join(sorted(missing))}")
    try:
        validate(result, sha, tag, assets)
    except (ValueError, KeyError, TypeError) as error:
        blockers.append(str(error))
    result["admission"] = "blocked" if blockers else "accepted"
    Path(output).parent.mkdir(parents=True, exist_ok=True)
    Path(output).write_text(json.dumps(result, indent=2) + "\n")
    return result


def capture(junit, output, archive, argv):
    """Wrap an existing nextest command; capture facts, not caller verdicts.

    Invoke on the actual executor. A mutation is applied by the existing W09
    procedure before capture; the exact working diff is saved with its result.
    W14 passes --archive to execute that candidate's --version on Windows.
    Output paths should be outside tracked source. Missing/stale JUnit is blocked.
    """
    require(argv and "nextest" in argv and "run" in argv, "capture requires nextest run")
    sha = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    diff = subprocess.check_output(["git", "diff", "HEAD", "--"], text=True)
    record = dict(schema="stabilization-command-1", source=sha, diff=diff, argv=argv,
                  platform=platform.system(), machine=platform.machine(), complete=False)
    if archive:
        require(platform.system() == "Windows", "native archive capture requires Windows")
        record["artifact_sha256"] = digest(archive)
        with tempfile.TemporaryDirectory() as directory:
            with zipfile.ZipFile(archive) as package:
                members = [n for n in package.namelist() if Path(n).name == "wayland-core.exe"]
                require(len(members) == 1, "archive needs exactly one executable")
                binary = Path(directory) / "wayland-core.exe"
                binary.write_bytes(package.read(members[0]))
            startup = subprocess.run([str(binary), "--version"], capture_output=True, text=True)
            record.update(startup=startup.stdout.strip(), startup_exit=startup.returncode)
    started = time.time_ns()
    completed = subprocess.run(argv)
    record["exit_code"] = completed.returncode
    record["complete"] = (Path(junit).is_file() and Path(junit).stat().st_mtime_ns >= started and
                          subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip() == sha and
                          subprocess.check_output(["git", "diff", "HEAD", "--"], text=True) == diff)
    if record["complete"]:
        record["junit_sha256"] = digest(junit)
    Path(output).parent.mkdir(parents=True, exist_ok=True)
    Path(output).write_text(json.dumps(record, indent=2) + "\n")
    return completed.returncode if record["complete"] else 2


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
        """Create the draft and RETURN it, identified by its own id.

        The previous body created the draft and then went looking for it in a
        LIST, which is a different question with a different answer: a create
        that succeeded followed by a list that had not caught up yet produced
        None, and the caller subscripted it (run 34327731386 attempt 1, whose
        draft 385331683 then existed with zero assets). Returning the created
        object removes the second question entirely -- there is nothing to
        look up, so there is no window in which to fail to find it, and no
        path on which an uncertain result invites a second create.
        """
        # POST /releases CREATES the tag when it is absent, at whatever the
        # default branch happens to be. `gh release create --verify-tag`
        # refused that; the REST call does not, so the check is explicit here
        # or a mistyped tag silently invents one and signs against it.
        self.run("api", f"repos/{self.repo}/git/ref/tags/{self.tag}")
        created = json.loads(self.run(
            "api", "--method", "POST", f"repos/{self.repo}/releases",
            "-f", f"tag_name={self.tag}", "-f", f"name={self.tag}",
            "-F", "draft=true", "-F", "generate_release_notes=true"))
        require(isinstance(created, dict) and created.get("id"),
                "create returned no release id")
        require(created.get("tag_name") == self.tag,
                "created release names a different tag")
        require(created.get("draft") is True, "created release is not a draft")
        created.setdefault("assets", [])
        return created

    def refresh(self, release):
        """Re-read ONE release by id, bounded, absent distinguished from unreachable.

        Used instead of `view()` wherever a release is already known, because
        a list that omits it and a list that could not be fetched are the same
        value -- None -- and only one of them means the release is gone.
        """
        require(release is not None and release.get("id"), "refresh needs a known release")
        last = ""
        for attempt in range(5):
            completed = subprocess.run(
                ["gh", "api", f"repos/{self.repo}/releases/{release['id']}"],
                capture_output=True, text=True)
            if completed.returncode == 0:
                return json.loads(completed.stdout)
            last = completed.stderr.strip()
            require("(HTTP 404)" in last,
                    f"reading release {release['id']} failed and it is NOT a 404, so "
                    f"this is not evidence the release is absent: {last}")
            time.sleep(2 * (attempt + 1))
        raise ValueError(
            f"release {release['id']} was created but is still 404 after five reads: {last}")

    def hashes(self, release):
        # A clear refusal rather than `'NoneType' object is not subscriptable`,
        # which is what this actually failed with in production.
        require(release is not None, "cannot read assets of an absent release")
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
        release = backend.create()
    remote = backend.hashes(release)
    require(all(name in assets and assets[name] == digest for name, digest in remote.items()),
            "remote artifact mismatch; refusing overwrite")
    if action == "stage":
        if not release["draft"]:
            require(remote == assets, "published release differs")
            return
        for name in sorted(assets.keys() - remote.keys()):
            backend.upload(Path(directory) / name)
        require(backend.hashes(backend.refresh(release)) == assets, "incomplete private upload")
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
            creates = 0
            # When set, every LIST lookup after a create answers None, which
            # is the run 34327731386 shape: the create succeeded and the read
            # that followed it did not see the object yet.
            list_goes_blind = False
            def view(self):
                if self.list_goes_blind:
                    return None
                return self.release
            def create(self):
                self.creates += 1
                self.release = dict(id=385331683, draft=True,
                                    body="release notes", assets=[])
                self.remote = {}
                return self.release
            def refresh(self, release):
                require(release is not None, "refresh called with no release")
                return self.release
            def hashes(self, release):
                # The real backend subscripts release["assets"] here. Mirror
                # that so a None reaching this point is a failure in the
                # control rather than something the fake quietly absorbs.
                require(release is not None, "hashes called with an absent release")
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

        # P1: a create that succeeds followed by a list that cannot see it.
        # This raised `'NoneType' object is not subscriptable` and left an
        # empty draft behind; the danger on retry was a SECOND draft. The
        # staging must complete from the created object alone, and create
        # must be called exactly once.
        blind = Fake()
        blind.list_goes_blind = True
        transition(blind, directory, "stage")
        require(blind.creates == 1,
                f"a blind list made the gate create {blind.creates} drafts")
        require(blind.hashes(blind.release) == assets,
                "staging did not complete when the list could not see the draft")
    producer_self_test()
    print("PASS: receipt refusals, private create/resume, promotion retry, withdrawal, mismatch")


def producer_self_test():
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        artifacts, raw = root / "artifacts", root / "raw"
        artifacts.mkdir()
        raw.mkdir()
        sha, tag = "a" * 40, "v1.2.3"
        for target in TARGETS:
            ext = "zip" if "windows" in target else "tar.gz"
            (artifacts / f"wayland-core-{tag}-{target}.{ext}").write_bytes(target.encode())
        assets = inventory(artifacts)
        def ref(name, text):
            path = raw / name
            path.write_text(text)
            return dict(path=name, sha256=digest(path))
        def execution(name, failed=False, patch="", native=False):
            junit = ref(name + ".xml", '<testsuites><testsuite><testcase name="contract">' +
                        ('<failure message="caught"/>' if failed else '') +
                        '</testcase></testsuite></testsuites>')
            proof = dict(schema="stabilization-command-1", source=sha, complete=True,
                         argv=["cargo", "nextest", "run", "--no-tests=fail", "--retries", "0", name],
                         exit_code=1 if failed else 0, diff=patch, junit_sha256=junit["sha256"])
            if native:
                proof.update(platform="Windows", machine="AMD64", startup="wayland-core 1.2.3",
                             startup_exit=0, artifact_sha256=assets[f"wayland-core-{tag}-x86_64-pc-windows-msvc.zip"])
            return dict(id=name, junit=junit, proof=ref(name + ".json", json.dumps(proof)))
        baseline = execution("baseline")
        data = dict(source_sha=sha, tests=[baseline], mutants=[], native=[], deferred_risks=[
            dict(id=i, severity="high", disposition="standing external quarantine disposition")
            for i in ("core#368", "core#410")])
        index = raw / "index.json"
        index.write_text(json.dumps(data))
        out = root / "receipt.json"
        blocked = produce(index, artifacts, sha, tag, out)
        require(blocked["admission"] == "blocked" and out.is_file(), "missing prerequisites fabricated")
        for mutant in MUTANTS:
            patch = "fixture mutation " + mutant
            row = execution(mutant, failed=True, patch=patch)
            row.update(baseline=baseline, patch=ref(mutant + ".patch", patch))
            data["mutants"].append(row)
        data["native"] = [execution(check, native=True) for check in NATIVE_CHECKS]
        index.write_text(json.dumps(data))
        accepted = produce(index, artifacts, sha, tag, out)
        require(accepted["admission"] == "accepted", str(accepted["blockers"]))
        # Tampering cannot be relabelled as a passing receipt.
        (raw / "baseline.xml").write_text("<testsuites/>")
        blocked = produce(index, artifacts, sha, tag, out)
        require(blocked["admission"] == "blocked", "tampered raw output admitted")



def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--action", choices=("stage", "promote", "withdraw", "produce", "capture"))
    for field in ("artifacts", "evidence", "sha", "tag", "repo"):
        parser.add_argument("--" + field)
    for field in ("inputs", "output", "junit", "archive"):
        parser.add_argument("--" + field)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if args.action == "capture":
        require(args.junit and args.output, "capture requires JUnit and output paths")
        argv = args.command[1:] if args.command[:1] == ["--"] else args.command
        raise SystemExit(capture(args.junit, args.output, args.archive, argv))
    if args.action == "produce":
        require(all((args.inputs, args.artifacts, args.sha, args.tag, args.output)), "missing producer arguments")
        actual = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
        require(actual == args.sha, "producer checkout SHA mismatch")
        result = produce(args.inputs, args.artifacts, args.sha, args.tag, args.output)
        print(json.dumps({"admission": result["admission"], "blockers": result["blockers"]}))
        raise SystemExit(1 if result["blockers"] else 0)
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
