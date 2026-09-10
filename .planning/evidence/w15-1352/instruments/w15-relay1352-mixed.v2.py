#!/usr/bin/env python3
"""w15/relay1352 c2: mixed-reader relay A/B through the real `acp serve`.

Derived from evidence/mixed-soak353/run2/driver.py (sha256 f64b20bd...): the
same fixture provider (held markers pause 30 ms per chunk and hold 10 s), the
same config (encrypted_file vault, [session] require_durability = true), and
the same three reader modes with the same per-row pass rules. What differs:

  * batch COMPOSITION is fixed and explicit, so each arm is guaranteed its fast
    16 MiB rows at concurrency 8 and 32 alongside slow and disconnected readers:
      c8  = 4 fast + 2 slow + 2 disconnected, all 16 MiB
      c32 = 16 fast + 8 slow + 8 disconnected, all 16 MiB
  * a failing row does NOT abort the run; every row is recorded and graded;
  * the error text of every fast row is classified: "protocol relay overloaded"
    is the stage-1 engine relay (this ticket), "live delivery overloaded" is the
    stage-2 HTTP delivery detach;
  * RSS is sampled every 0.25 s for a per-batch peak, and read after each batch
    quiesces for a quiescent value.
It needs no network namespace: provider and core bind 127.0.0.1 only.
"""
import argparse, concurrent.futures, datetime, hashlib, http.client, http.server, json, os
import platform, re, select, signal, socket, statistics, subprocess, threading, time, uuid
from pathlib import Path

MIB = 1024 * 1024
FIXTURE_KEY = "w05-no-real-credentials"
COMPOSITION = {8: ["fast"] * 4 + ["slow"] * 2 + ["disconnected"] * 2,
               32: ["fast"] * 16 + ["slow"] * 8 + ["disconnected"] * 8}


def digest(path):
    with open(path, "rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


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
        with self.server.completed_lock:
            self.server.active.add(marker)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        sent = 0
        held = marker.startswith("held")
        fixture_start = time.monotonic()

        def pause(seconds):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if select.select([self.connection], [], [], 0)[0]:
                    if not self.connection.recv(1, socket.MSG_PEEK):
                        raise ConnectionResetError("fixture upstream canceled")
                time.sleep(.02)
        try:
            while sent < total:
                if self.server.mixed_chunks:
                    # Large events first, then many small ones: evicting a big
                    # event and then appending small deltas is the traffic that
                    # exposes retained-history accounting drift.
                    size = min(512 * 1024 if sent < total // 2 else 4096, total - sent)
                else:
                    size = min(chunk, total - sent)
                event = {"id": marker, "object": "chat.completion.chunk", "model": "w05-fixture",
                         "choices": [{"index": 0, "delta": {"content": "x" * size}, "finish_reason": None}]}
                self.wfile.write(("data: " + json.dumps(event) + "\n\n").encode())
                self.wfile.flush()
                sent += size
                if held:
                    pause(.03)
            if held:
                pause(max(0, 10 - (time.monotonic() - fixture_start)))
            end = {"id": marker, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                   "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}
            self.wfile.write(("data: " + json.dumps(end) + "\n\ndata: [DONE]\n\n").encode())
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass
        finally:
            with self.server.completed_lock:
                self.server.active.discard(marker)
                self.server.completed[marker] = {"requested_text_bytes": total, "sent_text_bytes": sent}
            self.close_connection = True


def rss(pid):
    fields = dict(line.split(":", 1) for line in Path(f"/proc/{pid}/status").read_text().splitlines() if ":" in line)
    return int(fields["VmRSS"].split()[0]) * 1024


def classify(errors):
    text = " ".join(json.dumps(e) for e in errors)
    if "protocol relay overloaded" in text:
        return "stage1_relay_overloaded"
    if "live delivery overloaded" in text:
        return "stage2_delivery_overloaded"
    return "other_error" if errors else None


def run(args):
    root = Path(args.root).resolve()
    root.mkdir(parents=True, exist_ok=False)
    binary = str(Path(args.binary).resolve())
    receipt = {"driver": "w15-relay1352-mixed", "source": args.source, "binary_sha256": digest(binary),
               "driver_sha256": digest(__file__), "label": args.label,
               "started": datetime.datetime.now(datetime.timezone.utc).isoformat(),
               "platform": platform.platform(), "cpu_count": os.cpu_count(),
               "loadavg_start": os.getloadavg(), "composition": {str(k): v for k, v in COMPOSITION.items()},
               "schedule": args.schedule, "text_bytes": 16 * MIB,
               "chunk_bytes": "first half 524288, then 4096" if args.mixed_chunks else 32768,
               "mixed_chunks": args.mixed_chunks,
               "batches": [], "rows": []}
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
    provider.completed, provider.completed_lock, provider.active = {}, threading.Lock(), set()
    provider.mixed_chunks = args.mixed_chunks
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    provider_port = provider.server_address[1]
    with socket.socket() as port_socket:
        port_socket.bind(("127.0.0.1", 0))
        core_port = port_socket.getsockname()[1]
    (profile / "config.toml").write_text(f'''[default]
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
''')
    (profile / "config.toml").chmod(0o600)
    info = subprocess.check_output([binary, "--build-info"], env=env, text=True, timeout=30)
    assert args.source in info, ("binary/source mismatch", info)
    receipt["build_info"] = info
    core_log = (root / "core.log").open("wb")
    core = subprocess.Popen([binary, "acp", "serve", "--bind", f"127.0.0.1:{core_port}", "--provider", "openai",
                             "--model", "w05-fixture", "--api-key", FIXTURE_KEY,
                             "--base-url", f"http://127.0.0.1:{provider_port}/v1"],
                            cwd=workspace, env=env, stdout=core_log, stderr=subprocess.STDOUT, start_new_session=True)
    receipt["core_pid"] = core.pid
    stop_sample = threading.Event()
    samples = []

    def sampler():
        while not stop_sample.wait(.25):
            try:
                samples.append((time.monotonic(), rss(core.pid)))
            except (FileNotFoundError, ProcessLookupError, KeyError):
                pass
    try:
        deadline = time.monotonic() + 180
        while True:
            try:
                if request(core_port, "GET", "/v1/health")[0] == 200:
                    break
            except OSError:
                pass
            assert core.poll() is None and time.monotonic() < deadline, "ACP readiness failed"
            time.sleep(.05)
        receipt["rss_at_ready"] = rss(core.pid)
        threading.Thread(target=sampler, daemon=True).start()

        def cycle(reader, concurrency, batch_index):
            total, chunk = 16 * MIB, 32768
            start = time.monotonic()
            status, body = request(core_port, "POST", "/v1/sessions", {})
            assert status == 200, ("create", status, body[:200])
            session = json.loads(body)["session_id"]
            marker = ("held" if reader != "fast" else "fast") + uuid.uuid4().hex
            row = {"batch": batch_index, "concurrency": concurrency, "marker": marker, "session": session,
                   "reader": reader, "text_bytes": total}
            conn = http.client.HTTPConnection("127.0.0.1", core_port, timeout=300)
            try:
                turn_start = time.monotonic()
                conn.request("POST", f"/v1/sessions/{session}/prompt", json.dumps({"text": f"w05:{marker}:{total}:{chunk}"}),
                             {"Content-Type": "application/json", "X-API-Key": FIXTURE_KEY})
                response = conn.getresponse()
                row["prompt_status"] = response.status
                count, largest, text_bytes, terminal, errors = 0, 0, 0, 0, []
                if reader == "disconnected":
                    dispatch_deadline = time.monotonic() + 10
                    while True:
                        with provider.completed_lock:
                            dispatched = marker in provider.active
                        if dispatched or time.monotonic() >= dispatch_deadline:
                            break
                        time.sleep(.01)
                    row["fixture_dispatch_observed"] = dispatched
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
                row.update({"events_read": count, "largest_encoded_event_bytes": largest,
                            "received_text_bytes": text_bytes, "terminal_count": terminal, "errors": errors,
                            "error_class": classify(errors), "turn_ms": (time.monotonic() - turn_start) * 1000})
            except Exception as error:
                row["read_error"] = repr(error)
            finally:
                with provider.completed_lock:
                    row["fixture_active_before_delete"] = marker in provider.active
                conn.close()
                delete_start = time.monotonic()
                try:
                    delete_status, delete_body = request(core_port, "DELETE", f"/v1/sessions/{session}")
                    row.update({"delete_status": delete_status, "delete_ms": (time.monotonic() - delete_start) * 1000})
                except Exception as error:
                    row.update({"delete_status": None, "delete_ms": (time.monotonic() - delete_start) * 1000,
                                "delete_error": repr(error)})
            row["get_after_delete_status"] = request(core_port, "GET", f"/v1/sessions/{session}")[0]
            row["cycle_ms"] = (time.monotonic() - start) * 1000
            return row

        for batch_index, concurrency in enumerate(int(c) for c in args.schedule.split(",")):
            readers = COMPOSITION[concurrency]
            before = len(samples)
            loadavg = os.getloadavg()
            with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as executor:
                rows = list(executor.map(lambda r: cycle(r, concurrency, batch_index), readers))
            quiescence_deadline = time.monotonic() + 10
            while True:
                with provider.completed_lock:
                    active = sorted(provider.active)
                if not active or time.monotonic() >= quiescence_deadline:
                    break
                time.sleep(.05)
            status, sessions = request(core_port, "GET", "/v1/sessions")
            sessions = json.loads(sessions)
            quiescent = rss(core.pid)
            for row in rows:
                row["pass"] = (row.get("prompt_status") == 200 and row.get("delete_status") == 204
                               and row.get("delete_ms", float("inf")) <= 10000
                               and row.get("get_after_delete_status") == 404 and not row.get("read_error"))
                if row["reader"] == "fast":
                    row["pass"] = (row["pass"] and row.get("received_text_bytes") == row["text_bytes"]
                                   and row.get("terminal_count") == 1 and not row.get("errors")
                                   and row.get("largest_encoded_event_bytes", MIB + 1) <= MIB)
                else:
                    row["pass"] = row["pass"] and row["fixture_active_before_delete"]
                with provider.completed_lock:
                    row["fixture_completion"] = provider.completed.pop(row["marker"], None)
            batch = {"index": batch_index, "concurrency": concurrency, "loadavg_before": loadavg,
                     "sessions_after": sessions.get("sessions"), "active_fixture_after": active,
                     "quiescent_rss_bytes": quiescent,
                     "rss_peak_bytes": max((v for _, v in samples[before:]), default=quiescent),
                     "fast_pass": sum(1 for r in rows if r["reader"] == "fast" and r["pass"]),
                     "fast_rows": sum(1 for r in rows if r["reader"] == "fast"),
                     "stage1": sum(1 for r in rows if r.get("error_class") == "stage1_relay_overloaded"),
                     "stage2": sum(1 for r in rows if r.get("error_class") == "stage2_delivery_overloaded"),
                     "rows_pass": sum(1 for r in rows if r["pass"])}
            receipt["batches"].append(batch)
            receipt["rows"].extend(rows)
            print(json.dumps({k: batch[k] for k in ("index", "concurrency", "fast_pass", "fast_rows", "stage1", "stage2",
                                                    "rows_pass", "quiescent_rss_bytes", "rss_peak_bytes")}), flush=True)
            (root / "receipt.partial.json").write_text(json.dumps(receipt, indent=2))
            assert core.poll() is None, "owned ACP exited during run"
            time.sleep(1)
        summary = {}
        for concurrency in sorted({b["concurrency"] for b in receipt["batches"]}):
            fast = [r for r in receipt["rows"] if r["reader"] == "fast" and r["concurrency"] == concurrency]
            other = [r for r in receipt["rows"] if r["reader"] != "fast" and r["concurrency"] == concurrency]
            turns = [r["turn_ms"] for r in fast if "turn_ms" in r]
            summary[str(concurrency)] = {
                "fast_rows": len(fast), "fast_pass": sum(r["pass"] for r in fast),
                "stage1_relay_overloaded": sum(1 for r in fast if r.get("error_class") == "stage1_relay_overloaded"),
                "stage2_delivery_overloaded": sum(1 for r in fast if r.get("error_class") == "stage2_delivery_overloaded"),
                "other_fast_failures": sum(1 for r in fast if not r["pass"] and r.get("error_class") not in
                                           ("stage1_relay_overloaded", "stage2_delivery_overloaded")),
                "fast_turn_ms_median": statistics.median(turns) if turns else None,
                "slow_disconnected_rows": len(other), "slow_disconnected_pass": sum(r["pass"] for r in other)}
        quiescents = [b["quiescent_rss_bytes"] for b in receipt["batches"]]
        receipt["summary"] = {"by_concurrency": summary,
                              "rss_peak_bytes": max(b["rss_peak_bytes"] for b in receipt["batches"]),
                              "quiescent_rss_first": quiescents[0], "quiescent_rss_last": quiescents[-1],
                              "quiescent_rss_max": max(quiescents),
                              "all_sessions_quiesced": all(b["sessions_after"] == [] and not b["active_fixture_after"]
                                                           for b in receipt["batches"])}
        receipt["completed"] = True
    except Exception as error:
        receipt["error"] = repr(error)
        receipt["completed"] = False
    finally:
        stop_sample.set()
        try:
            os.killpg(core.pid, signal.SIGTERM)
            core.wait(timeout=30)
        except Exception:
            try:
                os.killpg(core.pid, signal.SIGKILL)
                core.wait(timeout=10)
            except Exception:
                pass
        provider.shutdown()
        provider.server_close()
        core_log.close()
        receipt["loadavg_end"] = os.getloadavg()
        receipt["finished"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        (root / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(json.dumps({"receipt": str(root / "receipt.json"), "completed": receipt["completed"],
                          "error": receipt.get("error"), "summary": receipt.get("summary")}, indent=2))
    return 0 if receipt["completed"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--source", required=True)
    parser.add_argument("--root", required=True)
    parser.add_argument("--label", default="")
    parser.add_argument("--schedule", default="8,8,32,8,8,32,8,8,8,8",
                        help="comma-separated batch concurrencies (8 or 32)")
    parser.add_argument("--mixed-chunks", action="store_true",
                        help="each turn: first half in 512 KiB chunks, the rest in 4 KiB chunks")
    return run(parser.parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
