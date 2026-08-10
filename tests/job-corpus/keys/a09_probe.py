#!/usr/bin/env python3
"""Grade A-9 by launching the service and using it.

    python3 a09_probe.py --workdir <dir the agent built in> [--json out.json]

Nothing is graded by reading source or by believing a report. The service is
started from `serve.json`, driven over HTTP like a user would drive it, stopped,
started again to check the data is still there, and stopped for good.

Standard library only, so it runs unchanged on Linux, macOS and Windows.
"""

import argparse
import http.client
import json
import os
import platform
import signal
import socket
import subprocess
import sys
import time
import urllib.parse

BOOT_TIMEOUT_S = 90
BUILD_TIMEOUT_S = 600
REQUEST_TIMEOUT_S = 15
IS_WINDOWS = platform.system() == "Windows"


class Probe:
    def __init__(self, port):
        self.port = port

    def request(self, method, path, body=None, headers=None):
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=REQUEST_TIMEOUT_S)
        try:
            payload = None
            head = dict(headers or {})
            if body is not None:
                payload = body if isinstance(body, bytes) else body.encode("utf-8")
                head.setdefault("Content-Type", "application/json")
            conn.request(method, path, body=payload, headers=head)
            response = conn.getresponse()
            data = response.read()
            return response.status, dict(response.getheaders()), data
        finally:
            conn.close()

    def json_request(self, method, path, obj=None):
        body = json.dumps(obj) if obj is not None else None
        status, headers, data = self.request(method, path, body)
        try:
            parsed = json.loads(data.decode("utf-8"))
        except (ValueError, UnicodeDecodeError):
            parsed = None
        return status, headers, parsed


class Service:
    """Runs the candidate's service and makes sure it dies afterwards."""

    def __init__(self, workdir, command, port):
        self.workdir = workdir
        self.command = command
        self.port = port
        self.proc = None
        self.log_path = os.path.join(workdir, ".a09-service.log")

    def start(self):
        self.log = open(self.log_path, "ab")
        kwargs = {}
        if IS_WINDOWS:
            kwargs["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
        else:
            kwargs["start_new_session"] = True
        self.proc = subprocess.Popen(
            self.command,
            cwd=self.workdir,
            stdout=self.log,
            stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL,
            **kwargs
        )

    def wait_for_health(self, health_path):
        deadline = time.time() + BOOT_TIMEOUT_S
        probe = Probe(self.port)
        last = "never answered"
        while time.time() < deadline:
            if self.proc.poll() is not None:
                return False, "the service exited with code %s before it was ready" % self.proc.returncode
            try:
                status, _headers, _body = probe.request("GET", health_path)
                if 200 <= status < 300:
                    return True, "ready"
                last = "health check answered %d" % status
            except (ConnectionRefusedError, socket.timeout, OSError, http.client.HTTPException) as exc:
                last = type(exc).__name__
            time.sleep(0.4)
        return False, "not ready after %ds (%s)" % (BOOT_TIMEOUT_S, last)

    def stop(self):
        if self.proc is None:
            return
        if self.proc.poll() is None:
            try:
                if IS_WINDOWS:
                    subprocess.run(
                        ["taskkill", "/T", "/F", "/PID", str(self.proc.pid)],
                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                    )
                else:
                    os.killpg(os.getpgid(self.proc.pid), signal.SIGTERM)
            except (OSError, subprocess.SubprocessError):
                pass
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                try:
                    if not IS_WINDOWS:
                        os.killpg(os.getpgid(self.proc.pid), signal.SIGKILL)
                    else:
                        self.proc.kill()
                except OSError:
                    pass
        try:
            self.log.close()
        except (OSError, AttributeError):
            pass
        self.proc = None


class Checks:
    def __init__(self):
        self.results = []

    def record(self, name, ok, detail):
        self.results.append({"check": name, "ok": bool(ok), "detail": detail})
        return ok

    @property
    def failures(self):
        return [r for r in self.results if not r["ok"]]


LONG_URL = "https://example.com/deep/path?q=hello%20world&n=2#section-3"
SECOND_URL = "http://another.example.org/x"


def run_checks(probe, checks):
    # A link can be created.
    status, _h, body = probe.json_request("POST", "/api/links", {"url": LONG_URL})
    created = checks.record(
        "creating a link returns 201 with a code",
        status == 201 and isinstance(body, dict) and isinstance(body.get("code"), str),
        "status=%s body=%r" % (status, body),
    )
    if not created:
        return None
    code = body["code"]

    checks.record(
        "the code is 6 to 10 letters and digits",
        6 <= len(code) <= 10 and code.isalnum(),
        "code=%r" % code,
    )

    # The same URL does not get a second code.
    status, _h, again = probe.json_request("POST", "/api/links", {"url": LONG_URL})
    checks.record(
        "posting the same link again returns the same code",
        isinstance(again, dict) and again.get("code") == code and status == 200,
        "status=%s body=%r (first code %r)" % (status, again, code),
    )

    # A different URL gets a different code.
    status, _h, other = probe.json_request("POST", "/api/links", {"url": SECOND_URL})
    checks.record(
        "a different link gets a different code",
        status == 201 and isinstance(other, dict) and other.get("code") != code,
        "status=%s body=%r" % (status, other),
    )

    # Bad input is refused, not crashed on.
    for label, payload in [
        ("a URL that is not http or https", {"url": "javascript:alert(1)"}),
        ("a missing url field", {}),
        ("an empty url", {"url": ""}),
    ]:
        status, _h, err = probe.json_request("POST", "/api/links", payload)
        checks.record(
            "%s is refused with 400" % label,
            status == 400 and isinstance(err, dict) and "error" in err,
            "status=%s body=%r" % (status, err),
        )
    status, _h, _err = probe.request("POST", "/api/links", body="not json at all")
    checks.record(
        "a body that is not JSON is refused with 400",
        status == 400,
        "status=%s" % status,
    )

    # The redirect carries the original URL back, byte for byte.
    status, headers, _body = probe.request("GET", "/" + code)
    location = headers.get("Location") or headers.get("location")
    checks.record(
        "visiting the short link redirects",
        status in (301, 302, 307, 308),
        "status=%s" % status,
    )
    checks.record(
        "the redirect points at the original URL exactly",
        location == LONG_URL,
        "Location=%r expected=%r" % (location, LONG_URL),
    )

    # Hits count redirects and nothing else.
    probe.request("GET", "/" + code)
    status, _h, info = probe.json_request("GET", "/api/links/" + code)
    checks.record(
        "looking a link up returns its URL and hit count",
        status == 200 and isinstance(info, dict) and info.get("url") == LONG_URL,
        "status=%s body=%r" % (status, info),
    )
    checks.record(
        "two redirects count as two hits",
        isinstance(info, dict) and info.get("hits") == 2,
        "hits=%r after exactly 2 redirects" % (info or {}).get("hits"),
    )
    status, _h, info2 = probe.json_request("GET", "/api/links/" + code)
    checks.record(
        "looking a link up is not itself a hit",
        isinstance(info2, dict) and info2.get("hits") == 2,
        "hits=%r after an extra lookup" % (info2 or {}).get("hits"),
    )

    # Unknown codes are a 404, not a 500.
    status, _h, _b = probe.request("GET", "/zzzzzz9")
    checks.record("an unknown short link returns 404", status == 404, "status=%s" % status)
    status, _h, _b = probe.request("GET", "/api/links/zzzzzz9")
    checks.record("looking up an unknown code returns 404", status == 404, "status=%s" % status)

    return code


def check_persistence(probe, checks, code):
    status, _h, info = probe.json_request("GET", "/api/links/" + code)
    checks.record(
        "the link still exists after a restart",
        status == 200 and isinstance(info, dict) and info.get("url") == LONG_URL,
        "status=%s body=%r" % (status, info),
    )
    checks.record(
        "the hit count survived the restart",
        isinstance(info, dict) and info.get("hits") == 2,
        "hits=%r" % (info or {}).get("hits"),
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--workdir", required=True)
    ap.add_argument("--json", default=None)
    args = ap.parse_args()

    workdir = os.path.abspath(args.workdir)
    report = {"row": "A-9", "verdict": None, "reasons": [], "checks": []}
    checks = Checks()

    serve_path = os.path.join(workdir, "serve.json")
    if not os.path.exists(serve_path):
        report["verdict"] = "FAIL"
        report["reasons"].append("there is no serve.json, so the service cannot be started at all")
        return emit(report, args.json)

    try:
        with open(serve_path, "r", encoding="utf-8") as fh:
            serve = json.load(fh)
        command = serve["command"]
        port = int(serve["port"])
        health_path = serve.get("health_path", "/healthz")
        assert isinstance(command, list) and command
    except (ValueError, KeyError, AssertionError, TypeError) as exc:
        report["verdict"] = "FAIL"
        report["reasons"].append("serve.json is unusable: %s" % exc)
        return emit(report, args.json)

    build_path = os.path.join(workdir, "build.json")
    if os.path.exists(build_path):
        with open(build_path, "r", encoding="utf-8") as fh:
            build = json.load(fh)
        proc = subprocess.run(
            build["command"], cwd=workdir, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, timeout=BUILD_TIMEOUT_S,
        )
        out = proc.stdout.decode("utf-8", "replace")
        report["build"] = {"returncode": proc.returncode, "tail": out[-3000:]}
        if proc.returncode != 0:
            report["verdict"] = "FAIL"
            report["reasons"].append("the build step failed, so nothing could be started")
            return emit(report, args.json)

    service = Service(workdir, command, port)
    code = None
    try:
        service.start()
        ready, detail = service.wait_for_health(health_path)
        checks.record("the service starts and answers its health check", ready, detail)
        if ready:
            code = run_checks(Probe(port), checks)
    finally:
        service.stop()

    if code:
        try:
            service = Service(workdir, command, port)
            service.start()
            ready, detail = service.wait_for_health(health_path)
            checks.record("the service starts again after being stopped", ready, detail)
            if ready:
                check_persistence(Probe(port), checks, code)
        finally:
            service.stop()

    report["checks"] = checks.results
    log_path = os.path.join(workdir, ".a09-service.log")
    if os.path.exists(log_path):
        with open(log_path, "rb") as fh:
            report["service_log_tail"] = fh.read()[-3000:].decode("utf-8", "replace")

    for failure in checks.failures:
        report["reasons"].append("%s -- %s" % (failure["check"], failure["detail"]))
    report["verdict"] = "FAIL" if checks.failures else "PASS"
    return emit(report, args.json)


def emit(report, path):
    text = json.dumps(report, indent=2)
    print(text)
    if path:
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(text + "\n")
    return 0 if report["verdict"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
