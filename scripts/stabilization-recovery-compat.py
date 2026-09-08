#!/usr/bin/env python3
"""W04 ordinary Linux binary interoperability, isolated behind loopback only."""
import argparse
import errno
import hashlib
import http.client
import json
import os
from pathlib import Path
import queue
import shutil
import signal
import socket
import subprocess
import threading
import tempfile
import time
import uuid


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


class Core:
    def __init__(self, binary, session, resume, env, workspace, root, name, port):
        self.name = name
        self.frames = []
        self.events = queue.Queue()
        self.log = (root / (name + ".stderr")).open("wb")
        self.output = (root / (name + ".jsonl")).open("w")
        self.process = subprocess.Popen(
            [binary, "--json-stream", "--provider", "openai", "--model", "f24c3-fixture",
             "--base-url", f"http://127.0.0.1:{port}/v1",
             "--resume" if resume else "--session-id", session],
            cwd=workspace, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=self.log, text=True, start_new_session=True,
        )
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        for line in self.process.stdout:
            self.output.write(line)
            self.output.flush()
            try:
                frame = json.loads(line)
            except json.JSONDecodeError:
                continue
            self.frames.append(frame)
            self.events.put(frame)
        self.events.put(None)

    def wait(self, kind, timeout=40):
        return self.wait_one_of({kind}, timeout)

    def wait_one_of(self, kinds, timeout=40):
        deadline = time.monotonic() + timeout
        while True:
            try:
                frame = self.events.get(timeout=max(0.01, deadline - time.monotonic()))
            except queue.Empty as error:
                raise RuntimeError(f"{self.name}: timed out waiting for {sorted(kinds)}") from error
            if frame is None:
                raise RuntimeError(f"{self.name}: Core exited before {sorted(kinds)}")
            if frame.get("type") in kinds:
                return frame
            if time.monotonic() >= deadline:
                raise RuntimeError(f"{self.name}: Core did not emit {sorted(kinds)}")

    def send(self, value):
        self.process.stdin.write(json.dumps(value) + "\n")
        self.process.stdin.flush()

    def stop(self):
        try:
            os.killpg(self.process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        self.process.wait(timeout=10)
        self.reader.join(timeout=5)
        self.log.close()
        self.output.close()


def queued_bytes(port):
    return sum(int(fields[4].split(":")[1], 16)
               for line in Path("/proc/net/tcp").read_text().splitlines()[1:]
               if (fields := line.split()) and int(fields[1].split(":")[1], 16) == port
               and fields[3] == "01")


def storage_checks(args, root, home, sessions, workspace, sid, uncertain, launch, requests, receipt):
    """Operate only on copied fixture state and a task-owned four-MiB tmpfs."""
    checks = {}
    receipt["stage"] = "enospc-mount"
    space = root / "limited-filesystem"
    space.mkdir()
    subprocess.run(["mount", "-t", "tmpfs", "-o", "size=4M", "tmpfs", str(space)],
                   check=True, capture_output=True)
    mounted = True
    config = home / "config.toml"
    original_config = config.read_text()
    try:
        limited = space / "sessions"
        shutil.copytree(sessions, limited)
        config.write_text(original_config.replace("directory = " + json.dumps(str(sessions)),
                                                  "directory = " + json.dumps(str(limited))))
        core = launch(args.candidate, sid, True, "enospc-live-writer")
        fill = space / "owned-filler"
        written = 0
        with fill.open("wb", buffering=0) as output:
            while True:
                try:
                    written += output.write(bytes(4096))
                except OSError as error:
                    assert error.errno == errno.ENOSPC, error
                    break
        assert written > 0 and os.statvfs(space).f_bfree == 0
        before = len(requests())
        core.send({"type": "message", "msg_id": "enospc", "content": "Return only f24c3-enospc",
                   "files": []})
        outcome = core.wait_one_of({"error", "stream_end"})
        assert outcome["type"] == "error" or "error" in json.dumps(outcome).lower(), outcome
        core.stop()
        assert len(requests()) == before, "failed durable write still dispatched a paid call"
        fill.unlink()
        checks["actual_enospc_refuses_dispatch"] = True
        # Restore space, use a fresh writer, and require an actual new turn.
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(home),
               "WAYLAND_HOME": str(home), "WAYLAND_VAULT_PASSPHRASE": "w04-fixture-vault"}
        cancel = subprocess.run([args.candidate, "session", "--dir", str(limited), "cancel", sid],
                                env=env, cwd=workspace, capture_output=True, text=True, timeout=20)
        (root / "enospc-forward-cancel.txt").write_text(cancel.stdout + cancel.stderr)
        assert cancel.returncode == 0, "restored space did not permit safe cancellation"
        core = launch(args.candidate, sid, True, "enospc-forward")
        marker = "f24c3-control-" + uuid.uuid4().hex
        core.send({"type": "message", "msg_id": marker, "content": "Return only " + marker, "files": []})
        assert marker in core.wait("text_delta")["text"]
        core.wait("stream_end"); core.stop()
        checks["enospc_forward_recovery"] = True
        shutil.copytree(limited, root / "enospc-export")
    finally:
        config.write_text(original_config)
        if mounted:
            subprocess.run(["umount", str(space)], check=True, capture_output=True)

    def reconcile(binary, directory, identity, env, preexec_fn=None):
        return subprocess.run([binary, "session", "--dir", str(directory), "reconcile", identity],
                              env=env, cwd=directory, capture_output=True, text=True, timeout=20,
                              preexec_fn=preexec_fn)

    fixture_env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(home),
                   "WAYLAND_HOME": str(home), "WAYLAND_VAULT_PASSPHRASE": "w04-fixture-vault"}
    for shape in ("truncated", "corrupt"):
        receipt["stage"] = shape + "-journal-control"
        directory = root / shape
        shutil.copytree(sessions, directory)
        journal = directory / (uncertain + ".journal")
        control = reconcile(args.candidate, directory, uncertain, fixture_env)
        assert control.returncode == 0, control.stderr
        data = journal.read_bytes()
        assert data, "journal control is empty"
        journal.write_bytes(data + b"\x00\x00" if shape == "truncated" else data[:-1] + bytes([data[-1] ^ 1]))
        mutated = digest(journal)
        result = reconcile(args.candidate, directory, uncertain, fixture_env)
        (root / (shape + ".txt")).write_text(result.stdout + result.stderr)
        if shape == "truncated":
            assert result.returncode == 0, result.stderr
        else:
            assert result.returncode != 0 and digest(journal) == mutated, "corrupt complete frame was accepted or erased"
        checks[shape + "_journal_control"] = True

    # Root cannot prove a chmod refusal. Run the same read command as an
    # unprivileged UID, with a readable positive control before denying access.
    receipt["stage"] = "permission-positive-and-refusal-control"
    # The host's /tmp may be root-private (0700). The uid65534 control
    # must traverse the fixture's ancestors before journal permissions matter.
    with tempfile.TemporaryDirectory(prefix="w04-permission-", dir="/var/tmp") as name:
        directory = Path(name)
        os.chown(directory, 65534, 65534)
        os.chmod(directory, 0o700)
        binary = directory / "wayland-core"
        shutil.copy2(args.candidate, binary); os.chmod(binary, 0o755)
        copied = directory / "sessions"
        shutil.copytree(sessions, copied)
        # Match real snapshot privacy/ownership for the positive control;
        # world-readable snapshots are correctly refused before journal I/O.
        for path in [copied, *copied.rglob("*")]:
            os.chown(path, 65534, 65534)
            os.chmod(path, 0o700 if path.is_dir() else 0o600)
        permission_env = {"PATH": fixture_env["PATH"], "HOME": str(directory), "WAYLAND_HOME": str(directory)}
        def unprivileged():
            os.setgroups([]); os.setgid(65534); os.setuid(65534)
        control = reconcile(str(binary), copied, uncertain, permission_env, unprivileged)
        assert control.returncode == 0, control.stderr
        journal = copied / (uncertain + ".journal")
        before = digest(journal)
        os.chmod(journal, 0)
        result = reconcile(str(binary), copied, uncertain, permission_env, unprivileged)
        assert result.returncode != 0 and "permission denied" in (result.stdout + result.stderr).lower()
        os.chmod(journal, 0o600)
        assert digest(journal) == before
        (root / "permission-refusal.txt").write_text(result.stdout + result.stderr)
        checks["actual_permission_refusal_preserves_state"] = True
    return checks


def main():
    parser = argparse.ArgumentParser()
    for name in ("candidate", "candidate-sha", "previous", "previous-sha", "fixture", "root"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    root = Path(args.root)
    root.mkdir(parents=True, exist_ok=False)
    receipt = {"candidate_sha": args.candidate_sha, "previous_sha": args.previous_sha,
               "candidate_binary_sha256": digest(args.candidate),
               "previous_binary_sha256": digest(args.previous), "checks": {}}
    cores = []
    provider = None
    receipt["stage"] = "isolated-environment"
    try:
        subprocess.run(["ip", "link", "set", "lo", "up"], check=True, capture_output=True)
        routes = json.loads(subprocess.check_output(["ip", "-j", "route"], text=True))
        assert not routes, "run inside unshare --net; external routes are forbidden"
        receipt["routes"] = routes
        home, workspace = root / "home", root / "workspace"
        home.mkdir(); workspace.mkdir()
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(home),
               "WAYLAND_HOME": str(home), "WAYLAND_VAULT_PASSPHRASE": "w04-fixture-vault",
               "OPENAI_API_KEY": "w04-fixture-provider", "WAYLAND_PLUGINS_DIR": str(root / "plugins"),
               "XDG_CONFIG_HOME": str(root / "xdg-config"), "XDG_DATA_HOME": str(root / "xdg-data"),
               "XDG_CACHE_HOME": str(root / "xdg-cache")}
        subprocess.run(["git", "init", "--quiet", str(workspace)], env=env, check=True)
        for label, binary, sha in (("candidate", args.candidate, args.candidate_sha),
                                   ("previous", args.previous, args.previous_sha)):
            receipt["stage"] = label + "-build-identity"
            info = subprocess.check_output([binary, "--build-info"], env=env, text=True)
            assert sha in info, f"{label} source identity mismatch"
            receipt[label + "_build_info"] = info.strip()
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0)); port = sock.getsockname()[1]
        sessions = home / "sessions"
        (home / "config.toml").write_text(f'''[default]
provider = "openai"
model = "f24c3-fixture"
[providers.openai]
api_key = "w04-fixture-provider"
base_url = "http://127.0.0.1:{port}/v1"
[providers.openai.compat]
cost_is_known_free = true
[memory]
enabled = false
[inbound_webhook]
enabled = false
[session]
enabled = true
require_durability = true
directory = {json.dumps(str(sessions))}
[storage.credentials.backend.encrypted_file]
cipher_path = {json.dumps(str(home / "credentials.enc"))}
key_params_path = {json.dumps(str(home / "credentials.params.json"))}
''')
        journal = root / "provider.jsonl"
        provider_log = (root / "provider.log").open("wb")
        provider = subprocess.Popen(["node", args.fixture, "--port", str(port), "--journal", str(journal),
                                     "--marker", "W04-COMPAT"], env=env, stdout=provider_log,
                                    stderr=subprocess.STDOUT, start_new_session=True)
        receipt["stage"] = "provider-startup"
        deadline = time.monotonic() + 15
        while True:
            try:
                connection = http.client.HTTPConnection("127.0.0.1", port, timeout=1)
                connection.request("GET", "/_llm/health")
                assert connection.getresponse().status == 200
                connection.close(); break
            except (OSError, http.client.HTTPException):
                assert time.monotonic() < deadline, "provider did not start"
                time.sleep(0.05)

        def launch(binary, sid, resume, name):
            receipt["stage"] = name
            core = Core(binary, sid, resume, env, workspace, root, name, port)
            cores.append(core)
            ready = core.wait("ready")
            assert ready["session_id"] == sid and ready["session_persistence"] == "durable", ready
            return core

        def requests():
            return [json.loads(line) for line in journal.read_text().splitlines()
                    if line and json.loads(line).get("kind") == "chat.completions"]

        def previous_or_refusal(sid, name):
            # W08 writes tracker schema 2; the release explicitly accepts only 1.
            # Never count an unrelated startup error or timeout as compatibility.
            receipt["stage"] = name
            before = {str(path.relative_to(sessions)): digest(path)
                      for path in sessions.rglob("*") if path.is_file()
                      and not path.name.endswith(".lock")}
            assert any(path.endswith(".journal") for path in before), "missing private journal"
            before_requests = len(requests())
            core = Core(args.previous, sid, True, env, workspace, root, name, port)
            cores.append(core)
            try:
                ready = core.wait("ready")
            except RuntimeError:
                # Require a natural unsuccessful exit, not our cleanup SIGKILL.
                code = core.process.wait(timeout=10)
                core.reader.join(timeout=5)
                evidence = (root / (name + ".stderr")).read_text() + json.dumps(core.frames)
                expected = "unsupported budget snapshot schema version 2; expected 1"
                assert code > 0 and expected in evidence, f"{name}: not an explicit schema refusal: {evidence}"
                assert not any(frame.get("type") == "ready" for frame in core.frames)
                core.stop()
                after = {str(path.relative_to(sessions)): digest(path)
                         for path in sessions.rglob("*") if path.is_file()
                         and not path.name.endswith(".lock")}
                assert after == before, f"{name}: refusal changed private session/journal bytes"
                assert len(requests()) == before_requests, f"{name}: refusal dispatched provider work"
                receipt.setdefault("downgrade_refusals", []).append(
                    {"stage": name, "reason": expected, "exit_code": code,
                     "preserved_sha256": before, "provider_requests": before_requests})
                return None
            assert ready["session_id"] == sid and ready["session_persistence"] == "durable", ready
            return core

        sid = uuid.uuid4().hex
        markers = []
        for index, binary in enumerate((args.previous, args.candidate, args.previous)):
            marker = "f24c3-control-" + uuid.uuid4().hex
            markers.append(marker)
            core = (previous_or_refusal(sid, f"compat-{index}") if index == 2
                    else launch(binary, sid, index > 0, f"compat-{index}"))
            if core is None:
                core = launch(args.candidate, sid, True, "compat-forward-after-refusal")
                receipt["history_compatibility"] = "safe-refusal-and-forward-recovery"
            elif index == 2:
                receipt["history_compatibility"] = "bidirectional"
            core.send({"type": "message", "msg_id": marker, "content": "Return only " + marker,
                       "files": []})
            delta = core.wait("text_delta")
            assert marker in delta["text"], delta
            core.wait("stream_end")
            core.stop()
        history = "\n".join(path.read_text() for path in sessions.glob("*.json"))
        assert all(marker in history for marker in markers), "roundtrip lost committed history"
        assert len(requests()) == 3, "normal roundtrip made unexpected provider requests"
        receipt["checks"]["release_candidate_release_history"] = True

        writer_binary = (args.candidate if receipt["history_compatibility"] ==
                         "safe-refusal-and-forward-recovery" else args.previous)
        core = launch(writer_binary, sid, True, "writer-control")
        # Idle cancel is read-only; resume must acquire real writer authority.
        receipt["stage"] = "writer-contender"
        assert core.process.poll() is None, "first writer exited before contention"
        before = {str(path.relative_to(sessions)): digest(path)
                  for path in sessions.rglob("*") if path.is_file()
                  and not path.name.endswith(".lock")}
        before_requests = len(requests())
        contender = Core(args.candidate, sid, True, env, workspace, root, "writer-contender", port)
        cores.append(contender)
        code = contender.process.wait(timeout=20)
        contender.reader.join(timeout=5)
        evidence = (root / "writer-contender.stderr").read_text() + json.dumps(contender.frames)
        (root / "writer-contender.txt").write_text(evidence)
        assert code > 0 and "writer lease is already held" in evidence, evidence
        assert not any(frame.get("type") == "ready" for frame in contender.frames)
        assert core.process.poll() is None, "first writer exited during contention"
        contender.stop()
        after = {str(path.relative_to(sessions)): digest(path)
                 for path in sessions.rglob("*") if path.is_file()
                 and not path.name.endswith(".lock")}
        assert after == before, "writer refusal changed private session/journal bytes"
        assert len(requests()) == before_requests, "writer refusal dispatched provider work"
        receipt["writer_refusal"] = {"exit_code": code, "preserved_sha256": before,
                                     "provider_requests": before_requests}
        receipt["checks"]["live_writer_excluded"] = True
        core.stop()
        uncertain = uuid.uuid4().hex
        core = launch(args.previous, uncertain, False, "previous-uncertain")
        os.kill(provider.pid, signal.SIGSTOP)
        core.send({"type": "message", "msg_id": "uncertain", "content": "Return only f24c3-cancel-held",
                   "files": []})
        deadline = time.monotonic() + 20
        while not queued_bytes(port):
            assert time.monotonic() < deadline, "physical request did not reach held provider"
            time.sleep(0.01)
        core.stop()
        # Reopen unknown-outcome state, or require explicit safe downgrade
        # refusal followed by candidate recovery. Never send a new request.
        for index, binary in enumerate((args.candidate, args.previous, args.candidate)):
            resumed = (previous_or_refusal(uncertain, f"uncertain-resume-{index}") if index == 1
                       else launch(binary, uncertain, True, f"uncertain-resume-{index}"))
            if resumed is None:
                receipt["uncertain_compatibility"] = "safe-refusal-and-forward-recovery"
                continue
            if index == 1:
                receipt["uncertain_compatibility"] = "bidirectional"
            resumed.send({"type": "session_resync", "recovery_version": 1,
                          "request_id": f"resync-{index}", "session_id": uncertain})
            snapshot = resumed.wait("session_recovery_snapshot")
            assert snapshot["lifecycle"] == "suspended", snapshot
            assert snapshot["pending_turn"]["reconcile_reason"] == "provider_outcome_unknown", snapshot
            resumed.stop()
        receipt["stage"] = "uncertain-no-redispatch"
        os.kill(provider.pid, signal.SIGCONT)
        deadline = time.monotonic() + 10
        while len(requests()) < 4:
            assert time.monotonic() < deadline
            time.sleep(0.05)
        assert len(requests()) == 4, "reopen replayed uncertain provider work"
        receipt["checks"]["unknown_outcome_survives_roundtrip_without_redispatch"] = True
        receipt["checks"]["forward_recovery_retains_authority"] = True
        receipt["checks"].update(storage_checks(args, root, home, sessions, workspace,
                                                sid, uncertain, launch, requests, receipt))
    except Exception as error:
        receipt["failure"] = f"{receipt['stage']}: {type(error).__name__}: {error}"
    finally:
        for core in cores:
            if core.process.poll() is None:
                core.stop()
        if provider is not None:
            try:
                os.killpg(provider.pid, signal.SIGCONT)
                os.killpg(provider.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                provider.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(provider.pid, signal.SIGKILL); provider.wait(timeout=5)
            provider_log.close()
        processes = [core.process for core in cores] + ([provider] if provider is not None else [])
        remaining = []
        for process in processes:
            try:
                os.killpg(process.pid, 0); remaining.append(process.pid)
            except ProcessLookupError:
                pass
        receipt["remaining_process_groups"] = remaining
        receipt["passed"] = len(receipt["checks"]) == 9 and "failure" not in receipt and not remaining
        (root / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(json.dumps(receipt))
    return 0 if receipt["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
