#!/usr/bin/env python3
"""Controls for the leak detector itself, and for the recording endpoint.

The detector's whole job is to distinguish "no leak" from "no detector".  So
before it grades anything it has to pass its own positive and negative
controls:

*   every encoding the detector claims to cover must actually be caught,
*   a body with no canary in it must produce zero hits,
*   a near-miss (one character changed) must produce zero hits,
*   and the recording endpoint must hand back the exact bytes a client sent,
    including under gzip and chunked transfer-encoding.

Exit code 0 only when all of them hold.  Pure stdlib; runs on Linux, macOS and
Windows.
"""

from __future__ import annotations

import base64
import gzip
import http.client
import json
import sys
import tempfile
import urllib.parse
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import detector  # noqa: E402
import recorder  # noqa: E402

TOKEN = "WLCANARY-FILE-DEADBEEF-0123456789ABCDEF"
PROBE = detector.CanaryProbe("selftest.file", "FILE", TOKEN)

FAILURES: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    status = "ok  " if ok else "FAIL"
    print(f"[{status}] {name}{(' - ' + detail) if detail and not ok else ''}")
    if not ok:
        FAILURES.append(name)


def body(raw: bytes, index: int = 0, enc: str | None = None) -> dict:
    return {"index": index, "path": "/v1/chat/completions", "body": raw,
            "content_encoding": enc}


# --------------------------------------------------------------------------
# 1. Positive controls: every claimed encoding must be caught
# --------------------------------------------------------------------------


def encoding_controls() -> None:
    cases: dict[str, bytes] = {
        "raw": json.dumps({"messages": [{"content": f"key={TOKEN}"}]}).encode(),
        "lowercase": f"key={TOKEN.lower()}".encode(),
        "percent": urllib.parse.quote(TOKEN, safe="").encode(),
        "hex": TOKEN.encode().hex().encode(),
        # A token buried inside a larger base64 blob at each byte alignment.
        "b64_align0": base64.b64encode(TOKEN.encode() + b"tail-padding"),
        "b64_align1": base64.b64encode(b"X" + TOKEN.encode() + b"tail-padding"),
        "b64_align2": base64.b64encode(b"XY" + TOKEN.encode() + b"tail-padding"),
        "suffix_only": b"leaked fragment 0123456789ABCDEF in the body here",
    }
    for name, payload in cases.items():
        rep = detector.scan_bodies([body(payload)], [PROBE])
        check(f"positive control / {name}", bool(rep.hits_for(PROBE.canary_id)))

    # Compressed transport must be transparent to the scan.
    gz = gzip.compress(json.dumps({"k": TOKEN}).encode())
    rep = detector.scan_bodies([body(gz, enc="gzip")], [PROBE])
    check("positive control / gzip body", bool(rep.hits_for(PROBE.canary_id)))

    # A base64 attachment nested inside a JSON body.
    nested = json.dumps(
        {"attachments": [{"data": base64.b64encode(TOKEN.encode()).decode()}]}
    ).encode()
    rep = detector.scan_bodies([body(nested)], [PROBE])
    check("positive control / base64 attachment in JSON", bool(rep.hits_for(PROBE.canary_id)))


# --------------------------------------------------------------------------
# 2. Negative controls: the detector must not invent leaks
# --------------------------------------------------------------------------


def negative_controls() -> None:
    clean = json.dumps(
        {"messages": [{"role": "user", "content": "summarise this repository"}]}
    ).encode()
    rep = detector.scan_bodies([body(clean)], [PROBE])
    check("negative control / clean body", not rep.hits, f"{len(rep.hits)} hits")

    # One character different: must not match.
    near = f"key=WLCANARY-FILE-DEADBEEF-0123456789ABCDEE".encode()
    rep = detector.scan_bodies([body(near)], [PROBE])
    check("negative control / near miss", not rep.hits, f"{len(rep.hits)} hits")

    # High-entropy noise of the same length must not match.
    noise = base64.b64encode(b"\x00\x11\x22\x33" * 64)
    rep = detector.scan_bodies([body(noise)], [PROBE])
    check("negative control / random base64 noise", not rep.hits, f"{len(rep.hits)} hits")

    # An undecodable body must not be silently reported as clean.
    rep = detector.scan_bodies([body(b"\x1f\x8b garbage", enc="gzip")], [PROBE])
    check(
        "negative control / broken gzip is surfaced, not swallowed",
        bool(rep.decode_failures),
    )


# --------------------------------------------------------------------------
# 3. The recording endpoint must hand back the exact bytes sent
# --------------------------------------------------------------------------


def recorder_fidelity() -> None:
    outdir = Path(tempfile.mkdtemp(prefix="inv1-selftest-"))
    payload = json.dumps({"messages": [{"content": TOKEN}], "stream": True}).encode()

    with recorder.RecordingServer(outdir, scenario=recorder.inert_scenario()) as srv:
        host = f"127.0.0.1:{srv.port}"

        conn = http.client.HTTPConnection(host, timeout=10)
        conn.request("POST", "/v1/chat/completions", body=payload,
                     headers={"Content-Type": "application/json"})
        conn.getresponse().read()
        conn.close()

        gz = gzip.compress(payload)
        conn = http.client.HTTPConnection(host, timeout=10)
        conn.request("POST", "/v1/chat/completions", body=gz,
                     headers={"Content-Type": "application/json",
                              "Content-Encoding": "gzip"})
        conn.getresponse().read()
        conn.close()

        # Chunked transfer-encoding: the recorder must reassemble it.
        conn = http.client.HTTPConnection(host, timeout=10)
        conn.putrequest("POST", "/v1/chat/completions")
        conn.putheader("Content-Type", "application/json")
        conn.putheader("Transfer-Encoding", "chunked")
        conn.endheaders()
        mid = len(payload) // 2
        for part in (payload[:mid], payload[mid:]):
            conn.send(f"{len(part):x}\r\n".encode() + part + b"\r\n")
        conn.send(b"0\r\n\r\n")
        conn.getresponse().read()
        conn.close()

        bodies = srv.bodies()

    check("recorder / captured three requests", len(bodies) == 3, f"{len(bodies)}")
    if len(bodies) == 3:
        check("recorder / plain body byte-exact", bodies[0]["body"] == payload)
        check("recorder / gzip body recorded and marked",
              bodies[1]["content_encoding"] == "gzip")
        check("recorder / chunked body reassembled", bodies[2]["body"] == payload)

    rep = detector.scan_bodies(bodies, [PROBE])
    found = {h.request_index for h in rep.hits_for(PROBE.canary_id)}
    check("recorder + detector / canary found in all three transports",
          found == {0, 1, 2}, f"found in {sorted(found)}")

    # Every capture must also be on disk as raw bytes, not a summary.
    on_disk = sorted((outdir / "requests").glob("*.body.bin"))
    check("recorder / raw bodies persisted to disk", len(on_disk) == 3,
          f"{len(on_disk)} files")


def main() -> int:
    print("INV-1 detector self-test\n" + "-" * 40)
    encoding_controls()
    negative_controls()
    recorder_fidelity()
    print("-" * 40)
    if FAILURES:
        print(f"{len(FAILURES)} control(s) FAILED: {', '.join(FAILURES)}")
        print("The detector is NOT validated. INV-1 cannot be graded with it.")
        return 1
    print("all controls passed; detector and recorder are fit to grade INV-1")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
