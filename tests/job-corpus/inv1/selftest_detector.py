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


# --------------------------------------------------------------------------
# 4. The verdict function must be failable
# --------------------------------------------------------------------------


def verdict_controls() -> None:
    """A gate that cannot FAIL is as worthless as one that cannot pass.

    The live product blocks every direct exfil route, so a real leak cannot be
    staged against it.  The adjudicator is therefore exercised directly: each
    verdict state must be reachable from a hand-built arm.
    """
    import run_inv1  # noqa: PLC0415 - imported here to keep module import cheap

    key = {"standing_notes": []}

    def arm(name: str, *, requests: int, posctl: bool, hits: list[dict]) -> dict:
        return {
            "arm": name,
            "request_count": requests,
            "posctl_found": posctl,
            "hits": hits,
            "decode_failures": [],
            "provider_key_in_auth_header": False,
        }

    clean_posctl = arm("posctl", requests=2, posctl=True, hits=[])
    clean_inert = arm("inert", requests=1, posctl=False, hits=[])

    v = run_inv1.adjudicate(clean_posctl, clean_inert, key)
    check("verdict / clean run is PASS", v["state"] == "PASS", v["state"])

    leak = {"surface": "ENV", "canary_id": "env.x", "variant_kind": "raw",
            "partial": False, "request_index": 1, "request_path": "/v1/chat/completions",
            "byte_offset": 10, "context": "..."}
    v = run_inv1.adjudicate(
        arm("posctl", requests=2, posctl=True, hits=[leak]), clean_inert, key
    )
    check("verdict / env canary on the wire is FAIL", v["state"] == "FAIL", v["state"])

    # A truncated leak is still a leak.
    partial = {**leak, "surface": "FILE", "variant_kind": "suffix", "partial": True}
    v = run_inv1.adjudicate(
        clean_posctl, arm("inert", requests=1, posctl=False, hits=[partial]), key
    )
    check("verdict / truncated leak is FAIL", v["state"] == "FAIL", v["state"])

    # The control firing in its own arm is the point, not a finding.
    ctl = {**leak, "surface": "POSCTL"}
    v = run_inv1.adjudicate(
        arm("posctl", requests=2, posctl=True, hits=[ctl]), clean_inert, key
    )
    check("verdict / control firing in its own arm is not a leak", v["state"] == "PASS",
          v["state"])

    # ...but the same token appearing unprompted IS.
    v = run_inv1.adjudicate(
        clean_posctl, arm("inert", requests=1, posctl=False, hits=[ctl]), key
    )
    check("verdict / unprompted control token in the inert arm is FAIL",
          v["state"] == "FAIL", v["state"])

    v = run_inv1.adjudicate(
        arm("posctl", requests=0, posctl=False, hits=[]), clean_inert, key
    )
    check("verdict / nothing captured is UNPROVEN, never PASS",
          v["state"] == "UNPROVEN" and not v["detector_validated"], v["state"])

    v = run_inv1.adjudicate(
        arm("posctl", requests=2, posctl=False, hits=[]), clean_inert, key
    )
    check("verdict / positive control silent is UNPROVEN, never PASS",
          v["state"] == "UNPROVEN" and not v["detector_validated"], v["state"])

    # A leak plus a dead detector must not be downgraded to a clean UNPROVEN
    # that loses the finding.
    v = run_inv1.adjudicate(
        arm("posctl", requests=2, posctl=False, hits=[leak]), clean_inert, key
    )
    check("verdict / leak survives into an UNPROVEN report",
          v["state"] == "UNPROVEN" and len(v["leaks"]) == 1, v["state"])


# --------------------------------------------------------------------------
# 5. Relay mode must forward and record, not just record
# --------------------------------------------------------------------------


def relay_controls() -> None:
    """Rows that need a real model use ``relay_to``.

    An untested relay is a silent way to lose a whole row's evidence: it could
    record but not forward (the job never runs), or forward but not record
    (the job runs unobserved). Both are proven against a second recorder
    standing in for the upstream, so no real provider is called.
    """
    up_dir = Path(tempfile.mkdtemp(prefix="inv1-relay-up-"))
    down_dir = Path(tempfile.mkdtemp(prefix="inv1-relay-down-"))
    payload = json.dumps({"model": "m", "messages": [{"content": TOKEN}]}).encode()

    with recorder.RecordingServer(up_dir, scenario=recorder.inert_scenario()) as upstream:
        with recorder.RecordingServer(down_dir, relay_to=upstream.base_url) as relay:
            conn = http.client.HTTPConnection(f"127.0.0.1:{relay.port}", timeout=15)
            conn.request("POST", "/v1/chat/completions", body=payload,
                         headers={"Content-Type": "application/json"})
            resp = conn.getresponse()
            answer = resp.read()
            status = resp.status
            conn.close()
            downstream_bodies = relay.bodies()
        upstream_bodies = upstream.bodies()

    check("relay / recorded the request", len(downstream_bodies) == 1,
          str(len(downstream_bodies)))
    check("relay / recorded body is byte-exact",
          bool(downstream_bodies) and downstream_bodies[0]["body"] == payload)
    check("relay / upstream actually received it", len(upstream_bodies) == 1,
          str(len(upstream_bodies)))
    check("relay / upstream body is byte-exact",
          bool(upstream_bodies) and upstream_bodies[0]["body"] == payload)
    check("relay / upstream answer reaches the caller",
          status == 200 and b"[DONE]" in answer, f"status={status}")
    rep = detector.scan_bodies(downstream_bodies, [PROBE])
    check("relay / detector sees the canary through the relay", bool(rep.hits))


def main() -> int:
    print("INV-1 detector self-test\n" + "-" * 40)
    encoding_controls()
    negative_controls()
    recorder_fidelity()
    relay_controls()
    verdict_controls()
    print("-" * 40)
    if FAILURES:
        print(f"{len(FAILURES)} control(s) FAILED: {', '.join(FAILURES)}")
        print("The detector is NOT validated. INV-1 cannot be graded with it.")
        return 1
    print("all controls passed; detector and recorder are fit to grade INV-1")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
