#!/usr/bin/env python3
"""W05 packaged Linux resource control/soak. Run in a fresh loopback-only netns.

No real credentials/providers. Slow-reader legs observe five seconds at 1KiB/s
then disconnect: they do not claim to drain a 16MiB reply at that rate.
The same corpus is used before/after the resource implementation. A control
records unsupported boundaries rather than silently changing the workload.
"""
import argparse
import concurrent.futures
import datetime
import hashlib
import http.client
import http.server
import json
import math
import os
from pathlib import Path
import platform
import re
import signal
import socket
import statistics
import subprocess
import threading
import time
import uuid

MIB = 1024 * 1024
FIXTURE_KEY = "w05-no-real-credentials"


def digest(path):
    with open(path, "rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def p95(samples):
    return sorted(samples)[max(0, math.ceil(len(samples) * .95) - 1)]


def request(port, method, path, body=None):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=60)
    try:
        conn.request(method, path, None if body is None else json.dumps(body),
                     {"Content-Type": "application/json", "X-API-Key": FIXTURE_KEY})
        response = conn.getresponse()
        return response.status, response.read()
    finally:
        conn.close()


class Provider(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def do_POST(self):
        raw = self.rfile.read(int(self.headers["Content-Length"]))
        payload = json.loads(raw)
        markers = re.findall(r"w05:(\w+):(\d+):(\d+)", json.dumps(payload["messages"]))
        if not markers:
            self.send_error(400, "fixture marker missing")
            return
        marker, total, chunk = markers[-1]
        total, chunk = int(total), int(chunk)
        if total > 16 * MIB or not 1 <= chunk <= MIB:
            self.send_error(400, "fixture size outside frozen corpus")
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        sent = 0
        try:
            while sent < total:
                size = min(chunk, total - sent)
                event = {"id": marker, "object": "chat.completion.chunk", "model": "w05-fixture",
                         "choices": [{"index": 0, "delta": {"content": "x" * size}, "finish_reason": None}]}
                self.wfile.write(("data: " + json.dumps(event) + "\n\n").encode())
                self.wfile.flush()
                sent += size
            end = {"id": marker, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                   "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}
            self.wfile.write(("data: " + json.dumps(end) + "\n\ndata: [DONE]\n\n").encode())
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass
        finally:
            with self.server.completed_lock:
                self.server.completed[marker] = {"requested_text_bytes": total, "sent_text_bytes": sent}
            self.close_connection = True


def discover_mcp(binary, workspace, env):
    # Source-confirmed current server methods. notifications/initialized is
    # unsupported by this binary and is not falsely graded as a successful handshake.
    requests = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": {"name": "w05-resource-probe", "version": "1"}}},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
    ]
    start = time.monotonic()
    process = subprocess.Popen([binary, "mcp-serve", "--transport", "stdio"],
        cwd=workspace, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, start_new_session=True)
    try:
        output, error = process.communicate(
            ("\n".join(json.dumps(row) for row in requests) + "\n").encode(), timeout=30)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.communicate(timeout=5)
        raise RuntimeError("MCP discovery failed to exit naturally on EOF")
    elapsed = (time.monotonic() - start) * 1000
    assert process.returncode == 0, ("MCP discovery exit", process.returncode, error.decode(errors="replace"))
    responses = [json.loads(line) for line in output.splitlines() if line.strip()]
    assert len(responses) == 2 and all(not row.get("error") for row in responses), responses
    by_id = {row.get("id"): row for row in responses}
    assert by_id[1]["result"]["protocolVersion"] == "2024-11-05", by_id[1]
    tools = by_id[2]["result"]["tools"]
    names = sorted(tool["name"] for tool in tools)
    assert all(name in names for name in ["Read", "Grep", "Glob"]), names
    assert not alive_group(process.pid), "MCP discovery left owned descendants"
    return {"elapsed_ms": elapsed, "tools": names, "exit_code": process.returncode,
            "response_sha256": hashlib.sha256(output).hexdigest(), "natural_eof_cleanup": True}


def rss(pid):
    fields = dict(line.split(":", 1) for line in Path(f"/proc/{pid}/status").read_text().splitlines() if ":" in line)
    if "VmRSS" not in fields:
        raise ProcessLookupError(f"RSS unavailable for exiting process {pid}")
    return int(fields["VmRSS"].split()[0]) * 1024


def alive_group(group):
    found = []
    for path in Path("/proc").glob("[0-9]*/stat"):
        try:
            # comm can contain spaces/parentheses; fields after final ')' start at state.
            suffix = path.read_text().rsplit(")", 1)[1].split()
            if int(suffix[2]) == group:
                found.append(int(path.parent.name))
        except (FileNotFoundError, ProcessLookupError):
            continue
    return found


def run(args):
    root = Path(args.root).resolve()
    root.mkdir(parents=True, exist_ok=False)
    binary = str(Path(args.binary).resolve())
    receipt = {"source": args.source, "binary_sha256": digest(binary), "driver_sha256": digest(__file__),
               "started": datetime.datetime.now(datetime.timezone.utc).isoformat(),
               "platform": platform.platform(), "cpu_count": os.cpu_count(),
               "meminfo": Path("/proc/meminfo").read_text(), "loadavg_start": os.getloadavg(),
               "cycles_requested": args.cycles, "matrix": [], "cycles": [], "measurements": {},
               "slow_reader_contract": "1024 bytes/sec for 5 seconds, then disconnect; not a full drain",
               "build_profile": args.build_profile}
    (root / "driver.py").write_bytes(Path(__file__).read_bytes())
    core = None
    provider = None
    core_log = None
    stop_sample = threading.Event()
    sampler = None
    samples = []
    try:
        subprocess.run(["ip", "link", "set", "lo", "up"], check=True, capture_output=True)
        routes = json.loads(subprocess.check_output(["ip", "-j", "route"], text=True))
        assert not routes, "requires a fresh network namespace with no external route"
        receipt["network_namespace"] = os.readlink("/proc/self/ns/net")
        receipt["routes"] = routes
        profile, workspace = root / "wayland-core", root / "workspace"
        for directory in [profile, workspace, root / "tmp", root / "plugins", root / "config", root / "cache", root / "data"]:
            directory.mkdir()
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(profile),
               "WAYLAND_HOME": str(profile), "TMPDIR": str(root / "tmp"),
               "XDG_CONFIG_HOME": str(root / "config"), "XDG_CACHE_HOME": str(root / "cache"),
               "XDG_DATA_HOME": str(root / "data"), "WAYLAND_PLUGINS_DIR": str(root / "plugins"),
               "WAYLAND_ACP_SERVER_KEY": FIXTURE_KEY, "WAYLAND_VAULT_PASSPHRASE": FIXTURE_KEY}
        subprocess.run(["git", "init", "--quiet", str(workspace)], env=env, check=True)
        provider = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
        provider.daemon_threads = True
        provider.completed, provider.completed_lock = {}, threading.Lock()
        provider_thread = threading.Thread(target=provider.serve_forever, daemon=True)
        provider_thread.start()
        provider_port = provider.server_address[1]
        with socket.socket() as port_socket:
            port_socket.bind(("127.0.0.1", 0))
            core_port = port_socket.getsockname()[1]
        config = f'''[default]
provider = "openai"
model = "w05-fixture"
[providers.openai]
api_key = "{FIXTURE_KEY}"
base_url = "http://127.0.0.1:{provider_port}/v1"
[providers.openai.compat]
cost_is_known_free = true
[memory]
enabled = false
[inbound_webhook]
enabled = false
[session]
enabled = true
require_durability = true
directory = {json.dumps(str(profile / "sessions"))}
[storage.credentials.backend.encrypted_file]
cipher_path = {json.dumps(str(profile / "credentials.enc"))}
key_params_path = {json.dumps(str(profile / "credentials.params.json"))}
'''
        (profile / "config.toml").write_text(config)
        (profile / "config.toml").chmod(0o600)
        info = subprocess.check_output([binary, "--build-info"], env=env, text=True, timeout=15)
        assert args.source in info, "binary/source mismatch"
        receipt["build_info"] = info
        previous_control = json.loads(Path(args.continue_control).read_text()) if args.continue_control else None
        if previous_control:
            assert previous_control["source"] == args.source and previous_control["binary_sha256"] == receipt["binary_sha256"]
            receipt["matrix"] = previous_control["matrix"]
            receipt["prior_control_failure"] = previous_control["error"]
            receipt["measurements"] = previous_control["measurements"]
            receipt["continued_control"] = args.continue_control
        receipt["mcp_preflight"] = previous_control["mcp_preflight"] if previous_control else discover_mcp(binary, workspace, env)
        receipt["mcp_protocol_limit"] = "Current server does not support notifications/initialized; measured initialize + tools/list + EOF only"
        core_log = (root / "core.log").open("wb")
        def start_core():
            nonlocal core
            core = subprocess.Popen([binary, "acp", "serve", "--bind", f"127.0.0.1:{core_port}",
                                     "--provider", "openai", "--model", "w05-fixture", "--api-key", FIXTURE_KEY,
                                     "--base-url", f"http://127.0.0.1:{provider_port}/v1"],
                                    cwd=workspace, env=env, stdout=core_log, stderr=subprocess.STDOUT, start_new_session=True)
            receipt.setdefault("core_pids", []).append(core.pid)
            deadline = time.monotonic() + 30
            while True:
                try:
                    if request(core_port, "GET", "/v1/health")[0] == 200:
                        break
                except OSError:
                    pass
                assert core.poll() is None and time.monotonic() < deadline, "ACP readiness failed"
                time.sleep(.05)
        def reset_core(reason):
            old_pid = core.pid
            try:
                os.killpg(old_pid, signal.SIGTERM)
                core.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(old_pid, signal.SIGKILL)
                core.wait(timeout=5)
            receipt.setdefault("owned_process_resets", []).append({"pid": old_pid, "reason": reason,
                "remaining_owned_pids": alive_group(old_pid), "quiescent_pass": False})
            assert not alive_group(old_pid), "failed to reap owned ACP process before independent leg"
            start_core()
        start_core()
        def sample():
            while not stop_sample.wait(.05):
                try:
                    samples.append([time.monotonic(), rss(core.pid)])
                except (FileNotFoundError, ProcessLookupError) as error:
                    receipt.setdefault("rss_unavailable", []).append({"time": time.monotonic(), "reason": str(error)})
                    continue
        sampler = threading.Thread(target=sample, daemon=True)
        sampler.start()

        def cycle(total=32, chunk=32, reader="fast"):
            start = time.monotonic()
            status, body = request(core_port, "POST", "/v1/sessions", {})
            assert status == 200, ("create", status, body[:200])
            session = json.loads(body)["session_id"]
            marker = uuid.uuid4().hex
            row = {"session": session, "reader": reader, "text_bytes": total, "provider_chunk_bytes": chunk}
            conn = http.client.HTTPConnection("127.0.0.1", core_port, timeout=60)
            try:
                turn_start = time.monotonic()
                conn.request("POST", f"/v1/sessions/{session}/prompt", json.dumps({"text": f"w05:{marker}:{total}:{chunk}"}),
                             {"Content-Type": "application/json", "X-API-Key": FIXTURE_KEY})
                response = conn.getresponse()
                row["prompt_status"] = response.status
                row["headers_ms"] = (time.monotonic() - turn_start) * 1000
                count, largest, text_bytes, terminal, errors = 0, 0, 0, 0, []
                if reader == "slow":
                    for _ in range(5):
                        response.read(1024)
                        time.sleep(1)
                elif reader == "fast":
                    while True:
                        line = response.readline()
                        if not line:
                            break
                        if line.startswith(b"data: "):
                            frame = json.loads(line[6:])
                            count += 1
                            largest = max(largest, len(line[6:].rstrip(b"\r\n")))
                            if frame.get("kind") == "text_delta":
                                text_bytes += len(frame.get("text", "").encode())
                            if frame.get("kind") in ("done", "error"):
                                terminal += 1
                            if frame.get("kind") == "error":
                                errors.append(frame)
                row.update({"events_read": count, "largest_encoded_event_bytes": largest, "received_text_bytes": text_bytes,
                            "terminal_count": terminal, "errors": errors, "turn_ms": (time.monotonic() - turn_start) * 1000})
            except Exception as error:
                row["read_error"] = repr(error)
            finally:
                conn.close()
                # DELETE measures the acknowledged ownership barrier after all reader modes.
                delete_start = time.monotonic()
                try:
                    delete_status, delete_body = request(core_port, "DELETE", f"/v1/sessions/{session}")
                    row.update({"delete_status": delete_status, "delete_ms": (time.monotonic() - delete_start) * 1000,
                                "delete_error": None if delete_status == 204 else delete_body.decode(errors="replace")})
                except Exception as error:
                    row.update({"delete_status": None, "delete_ms": (time.monotonic() - delete_start) * 1000,
                                "delete_error": repr(error)})
            row["cycle_ms"] = (time.monotonic() - start) * 1000
            row["rss_after_bytes"] = rss(core.pid)
            return row

        if previous_control:
            receipt["acp_preflight"] = previous_control["acp_preflight"]
        else:
            smoke = cycle()
            assert smoke["received_text_bytes"] == 32 and smoke["terminal_count"] == 1 and not smoke["errors"] and smoke["delete_status"] == 204, "ACP positive smoke failed"
            receipt["acp_preflight"] = smoke
        receipt["mode"] = "smoke" if args.smoke_only else "control"
        if not args.smoke_only:
            if args.reuse_startup_receipt:
                previous = json.loads(Path(args.reuse_startup_receipt).read_text())
                assert previous["source"] == args.source and previous["binary_sha256"] == receipt["binary_sha256"], "reused sample identity mismatch"
                for name in ["build-info", "config-path"]:
                    receipt["measurements"][name] = previous["measurements"][name]
                receipt["reused_startup_receipt"] = args.reuse_startup_receipt
            for name, command in [("build-info", ["--build-info"]), ("config-path", ["--config-path"]), ("mcp-discovery", None)]:
                if name in receipt["measurements"]:
                    continue
                durations = []
                for i in range(33):
                    if command is None:
                        elapsed = discover_mcp(binary, workspace, env)["elapsed_ms"]
                    else:
                        start = time.monotonic()
                        subprocess.run([binary, *command], cwd=workspace, env=env, check=True, capture_output=True, timeout=30)
                        elapsed = (time.monotonic() - start) * 1000
                    if i >= 3:
                        durations.append(elapsed)
                receipt["measurements"][name] = {"samples_ms": durations, "p95_ms": p95(durations)}
            # Exactly the frozen combinations. The reader count is actual concurrent sessions.
            # Continue only previously unmeasured concurrency legs. The failed
            # 1-session slow 16MiB leg remains the prior recorded RED, not rerun.
            for concurrency in ([8, 32] if previous_control else [1, 8, 32]):
                for total, chunk in [(32768, 32768), (MIB, MIB), (16 * MIB, 32768)]:
                    for reader in ["fast", "disconnected", "slow"]:
                        before = len(samples)
                        with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as executor:
                            def observe(_):
                                try:
                                    return cycle(total, chunk, reader)
                                except Exception as error:
                                    return {"observation_error": repr(error), "delete_status": None}
                            rows = list(executor.map(observe, range(concurrency)))
                        window = samples[before:]
                        receipt["matrix"].append({"concurrency": concurrency, "text_bytes": total, "chunk_bytes": chunk,
                                                  "reader": reader, "rows": rows, "rss_peak_bytes": max((v for _, v in window), default=rss(core.pid))})
                        (root / "receipt.partial.json").write_text(json.dumps(receipt, indent=2))
                        if any(row.get("delete_status") != 204 for row in rows):
                            reset_core({"concurrency": concurrency, "bytes": total, "reader": reader, "cause": "cleanup incomplete"})
            for _ in range(3):
                cycle()
            for _ in range(args.cycles):
                row = cycle()
                row["control_pass"] = row.get("received_text_bytes") == 32 and row.get("terminal_count") == 1 and not row.get("errors") and row.get("delete_status") == 204
                receipt["cycles"].append(row)
                (root / "receipt.partial.json").write_text(json.dumps(receipt, indent=2))
                if row.get("delete_status") != 204:
                    reset_core({"cycle": len(receipt["cycles"]), "cause": "cleanup incomplete"})
            status, sessions = request(core_port, "GET", "/v1/sessions")
            receipt["sessions_after"] = {"status": status, "body": json.loads(sessions)}
            values = [row["turn_ms"] for row in receipt["cycles"] if "turn_ms" in row]
            receipt["measurements"]["fixture-turn"] = {"samples_ms": values, "p95_ms": p95(values)}
            first = statistics.median(row["rss_after_bytes"] for row in receipt["cycles"][:100])
            last = statistics.median(row["rss_after_bytes"] for row in receipt["cycles"][-100:])
            receipt["rss_windows"] = {"first_100_median_bytes": first, "last_100_median_bytes": last,
                                      "independent_windows": args.cycles >= 200,
                                      "growth_bytes": last - first, "max_growth_bytes": max(first * .1, 32 * MIB)}
            receipt["rss_bound_pass"] = last - first <= max(first * .1, 32 * MIB) if args.cycles >= 200 else None
            if args.control:
                control = json.loads(Path(args.control).read_text())
                comparisons = {}
                for name, measured in receipt["measurements"].items():
                    old = control["measurements"][name]["p95_ms"]
                    limit = max(old * 1.1, old + 20) if name == "fixture-turn" else max(old * 1.2, old + 50)
                    comparisons[name] = {"control_p95_ms": old, "candidate_p95_ms": measured["p95_ms"], "limit_ms": limit,
                                         "pass": measured["p95_ms"] <= limit}
                receipt["comparisons"] = comparisons
        receipt["completed"] = True
        receipt["product_acceptance"] = "not inferred from diagnostic coverage; errors and owned resets remain failures"
    except Exception as error:
        receipt["error"] = repr(error)
        if isinstance(error, subprocess.CalledProcessError):
            receipt["failed_command_output"] = {"stdout": (error.stdout or b"").decode(errors="replace") if isinstance(error.stdout, bytes) else error.stdout,
                                                 "stderr": (error.stderr or b"").decode(errors="replace") if isinstance(error.stderr, bytes) else error.stderr}
        receipt["completed"] = False
    finally:
        stop_sample.set()
        if sampler:
            sampler.join(timeout=2)
        if core:
            try:
                os.killpg(core.pid, signal.SIGTERM)
                core.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(core.pid, signal.SIGKILL)
                core.wait(timeout=5)
            except ProcessLookupError:
                pass
        if provider:
            provider.shutdown()
            provider.server_close()
        if core_log:
            core_log.close()
        receipt["cleanup"] = {"remaining_owned_pids": alive_group(core.pid) if core else [],
                              "sampler_alive": bool(sampler and sampler.is_alive())}
        receipt["rss_samples"] = samples
        receipt["loadavg_end"] = os.getloadavg()
        receipt["finished"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        (root / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(json.dumps({"receipt": str(root / "receipt.json"), "completed": receipt["completed"], "error": receipt.get("error")}))
    return 0 if receipt["completed"] and not receipt["cleanup"]["remaining_owned_pids"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--source", required=True)
    parser.add_argument("--root", required=True)
    parser.add_argument("--cycles", type=int, choices=[100, 1000], default=100)
    parser.add_argument("--build-profile", default="debug debuginfo=0 incremental=0 mold")
    parser.add_argument("--continue-control", help="Finish unmeasured8/32session legs and cycles from the retained partial1session control")
    parser.add_argument("--smoke-only", action="store_true", help="One real MCP discovery and ACP create/turn/delete; no matrix or samples")
    parser.add_argument("--reuse-startup-receipt", help="Preserve already captured same-binary startup/config samples")
    parser.add_argument("--control", help="Prior receipt.json for paired p95 comparisons")
    return run(parser.parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
