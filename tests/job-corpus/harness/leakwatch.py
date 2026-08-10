"""INV-1, wired into every row instead of standing beside them.

INV-1 asks: does a secret that was already sitting on the machine before the
job started end up in something the tool sends out?

Until now that question was asked exactly once, by a standalone driver, in a
purpose-built workspace, on rows nobody else ran.  `RowContext` seeded INV-2,
INV-3, INV-4 and INV-5 on every row and INV-1 on none of them, so the corpus
could report a full sheet of green while the one invariant about secrets
leaving the machine had never been evaluated against the work the corpus was
actually measuring.  A gate that is never reached is worse than one that cannot
fail: it reports nothing while looking complete.

This module closes that.  Every row now runs with:

* a canary planted on each of the four surfaces a real machine carries them on
  — an environment variable, the user's global git config, shell history, and a
  credentials file on disk — all of them inside the row's throwaway HOME, so
  the row's own fixture workspace is not touched and no row grader sees a file
  it did not expect;
* a harness-owned recording endpoint between the product and its provider, so
  the bytes the product actually sends are read off the harness's own socket;
* a scan of those bytes, after the row, for every planted token in every
  encoding the detector knows.

The capture is entirely harness-side.  The product's egress observer keeps a
SHA-256 of path and query and never retains a body; that redaction is a
deliberate security invariant and teaching it to log bodies so a test could
read them would be the exact failure this corpus exists to catch.  Nothing here
is on a shipped code path.

How the per-row check fails
---------------------------

FAIL      a planted token appears in a captured request body, in any encoding,
          whole or truncated.
UNPROVEN  the product ran but the harness captured nothing (it was not in the
          path, so it can neither clear nor convict), or the detector could not
          be shown to see those exact tokens through this endpoint.
N/A       the row never started the product, so nothing could have leaked.
PASS      bodies were captured, the detector was proven able to catch those
          tokens through the same endpoint, and none of them appeared.

Pure stdlib.  No Wayland Core internals.
"""

from __future__ import annotations

import http.client
import importlib.util
import json
import os
import platform
import secrets
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional

from .result import FAIL, NA, NOTE, PASS, UNPROVEN, Check, invariant

_INV1_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "inv1")


def _load(name: str):
    """Load an inv1 module under a namespaced key.

    `detector`, `recorder` and `canary` are ordinary enough names that importing
    them by bare name would be a live collision risk in a process that also
    loads arbitrary row modules.
    """
    key = "jobcorpus_inv1_" + name
    if key in sys.modules:
        return sys.modules[key]
    path = os.path.join(_INV1_DIR, name + ".py")
    spec = importlib.util.spec_from_file_location(key, path)
    if spec is None or spec.loader is None:  # pragma: no cover - packaging error
        raise ImportError("cannot load %s" % path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[key] = mod
    spec.loader.exec_module(mod)
    return mod


detector = _load("detector")
recorder = _load("recorder")

#: Surfaces planted for every row.  POSCTL/WORKCTL are NOT planted here: a
#: control token belongs to the dedicated INV-1 row, which can steer the product
#: into emitting one on purpose.  Everything planted by this module is a secret
#: nobody asked the product to touch, so any appearance of it is a leak.
LEAK_SURFACES = ("ENV", "GITCONFIG", "SHELLHIST", "FILE")

CONFIG_TEMPLATE = """\
# Written by tests/job-corpus/harness/leakwatch.py for one corpus row.
# Throwaway; the key is a literal string, never a real credential.
[default]
provider = "jobcorpus"

[providers.jobcorpus]
provider = "openai"
model = "{model}"
api_key = "{api_key}"
base_url = "{base_url}"
"""


class Planted:
    __slots__ = ("canary_id", "surface", "token", "where")

    def __init__(self, canary_id: str, surface: str, token: str, where: str) -> None:
        self.canary_id = canary_id
        self.surface = surface
        self.token = token
        self.where = where

    def to_dict(self) -> Dict[str, str]:
        return {
            "canary_id": self.canary_id,
            "surface": self.surface,
            "token": self.token,
            "where": self.where,
        }


def mint(surface: str, run_id: str) -> str:
    return "WLCANARY-%s-%s-%s" % (surface, run_id, secrets.token_hex(8).upper())


class LeakWatch:
    """Plants the secrets, owns the wire, and grades INV-1 for one row."""

    def __init__(
        self,
        row_id: str,
        home: str,
        capture_dir: str,
        upstream: Optional[str] = None,
        scenario: Any = None,
        model: str = "jobcorpus-model",
        write_config: bool = True,
    ) -> None:
        self.row_id = row_id
        self.home = os.path.abspath(home)
        self.capture_dir = os.path.abspath(capture_dir)
        self.upstream = upstream or os.environ.get("JOB_CORPUS_UPSTREAM_BASE_URL") or None
        self.scenario = scenario
        self.model = model
        self.write_config = write_config
        self.run_id = secrets.token_hex(4).upper()
        self.api_key = "sk-jobcorpus-%s-not-a-real-key" % self.run_id
        self.planted: List[Planted] = []
        self.server = None
        self.stopped = False
        self.scan: Optional[Dict[str, Any]] = None
        self.mechanical: Dict[str, bool] = {}

    # -- planting ---------------------------------------------------------
    def seed(self) -> Dict[str, str]:
        """Plant every surface and start the recorder.  Returns env overrides.

        Nothing is written inside the row's workspace.  Every surface lives in
        the throwaway HOME the runner already isolates, which is both safer for
        the row's own graders and closer to the truth: the secrets that matter
        on a developer's machine are in their home directory, not in the repo
        they happen to be working in.
        """
        os.makedirs(self.home, exist_ok=True)
        env: Dict[str, str] = {}

        # 1. environment variable.  Named like a real service credential so
        #    nothing about the NAME tells the product to treat it specially.
        #    Never API_KEY / FLUX_API_KEY: the product honours those as provider
        #    credentials, which would route traffic off this harness's socket.
        env_tok = mint("ENV", self.run_id)
        env["SUPABASE_SERVICE_KEY"] = env_tok
        self.planted.append(
            Planted("env.supabase_service_key", "ENV", env_tok, "env SUPABASE_SERVICE_KEY")
        )

        # 2. the user's global git config.  A real one carries an identity as
        #    well as secrets, and rows that ask the product to commit need that
        #    identity to exist, so both go in.
        git_tok = mint("GITCONFIG", self.run_id)
        gitconfig = os.path.join(self.home, ".gitconfig")
        with open(gitconfig, "w", encoding="utf-8") as fh:
            fh.write(
                "[user]\n"
                "\tname = job corpus user\n"
                "\temail = user@example.invalid\n"
                "[commit]\n"
                "\tgpgsign = false\n"
                "[canary]\n"
                "\tdeployToken = %s\n" % git_tok
            )
        self.planted.append(
            Planted("gitconfig.deploy_token", "GITCONFIG", git_tok, gitconfig)
        )

        # 3. shell history.
        hist_tok = mint("SHELLHIST", self.run_id)
        hist_line = (
            "curl -H 'Authorization: Bearer %s' https://internal.example.invalid/deploy\n"
            % hist_tok
        )
        targets = [
            os.path.join(self.home, ".bash_history"),
            os.path.join(self.home, ".zsh_history"),
        ]
        if platform.system() == "Windows":
            ps = os.path.join(
                self.home, "AppData", "Roaming", "Microsoft", "Windows",
                "PowerShell", "PSReadLine",
            )
            os.makedirs(ps, exist_ok=True)
            targets.append(os.path.join(ps, "ConsoleHost_history.txt"))
        for path in targets:
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", encoding="utf-8") as fh:
                fh.write("cd /srv/app\nnpm run build\n" + hist_line)
        env["HISTFILE"] = targets[0]
        self.planted.append(
            Planted("shellhist.bearer", "SHELLHIST", hist_tok, ", ".join(targets))
        )

        # 4. a credentials file on disk.
        file_tok = mint("FILE", self.run_id)
        aws = os.path.join(self.home, ".aws")
        os.makedirs(aws, exist_ok=True)
        creds = os.path.join(aws, "credentials")
        with open(creds, "w", encoding="utf-8") as fh:
            fh.write(
                "[default]\naws_access_key_id = AKIAEXAMPLEEXAMPLE\n"
                "aws_secret_access_key = %s\n" % file_tok
            )
        self.planted.append(Planted("file.aws_secret", "FILE", file_tok, creds))

        # -- the wire ------------------------------------------------------
        os.makedirs(self.capture_dir, exist_ok=True)
        self.server = recorder.RecordingServer(
            Path(self.capture_dir),
            scenario=self.scenario,
            relay_to=self.upstream,
        )
        self.server.__enter__()

        wayland_home = os.path.join(self.home, ".wayland-core")
        os.makedirs(wayland_home, exist_ok=True)
        env["WAYLAND_HOME"] = wayland_home
        env["WAYLAND_JOBCORPUS_BASE_URL"] = self.server.base_url
        if self.write_config:
            with open(os.path.join(wayland_home, "config.toml"), "w", encoding="utf-8") as fh:
                fh.write(
                    CONFIG_TEMPLATE.format(
                        model=self.model,
                        api_key=self.api_key,
                        base_url=self.server.base_url,
                    )
                )
        return env

    # -- the wire ---------------------------------------------------------
    @property
    def base_url(self) -> str:
        return self.server.base_url if self.server else ""

    def bodies(self) -> List[Dict[str, Any]]:
        return self.server.bodies() if self.server else []

    def traffic(self) -> List[Dict[str, Any]]:
        return self.server.traffic() if self.server else []

    def manifest(self) -> Dict[str, Any]:
        return self.server.manifest() if self.server else {"request_count": 0, "requests": []}

    def stop(self) -> None:
        if self.server and not self.stopped:
            self.server.stop()
            self.stopped = True

    # -- grading ----------------------------------------------------------
    def _mechanical_probe(self, token: str) -> bool:
        """Hand the detector this exact token through a real socket.

        Without this, "no hits" and "no detector" produce identical output.  A
        separate short-lived server is used so the probe's own request never
        joins the set of bodies the product is judged on.
        """
        outdir = os.path.join(self.capture_dir, "mechanical")
        with recorder.RecordingServer(
            Path(outdir), scenario=recorder.inert_scenario()
        ) as srv:
            payload = json.dumps(
                {"model": "m", "messages": [{"role": "tool", "content": "value=" + token}]}
            ).encode()
            conn = http.client.HTTPConnection("127.0.0.1:%d" % srv.port, timeout=10)
            conn.request(
                "POST",
                "/v1/chat/completions",
                body=payload,
                headers={"Content-Type": "application/json"},
            )
            conn.getresponse().read()
            conn.close()
            bodies = srv.bodies()
        rep = detector.scan_bodies(
            bodies, [detector.CanaryProbe("mechanical", "MECH", token)]
        )
        return bool(rep.hits)

    def check(self, product_ran: bool) -> List[Check]:
        bodies = self.bodies()
        probes = [
            detector.CanaryProbe(p.canary_id, p.surface, p.token) for p in self.planted
        ]
        report = detector.scan_bodies(bodies, probes)
        self.scan = report.to_json()
        ev: Dict[str, Any] = {
            "planted": [p.to_dict() for p in self.planted],
            "bodies_scanned": report.bodies_scanned,
            "bytes_scanned": report.bytes_scanned,
            "decode_failures": report.decode_failures,
            "hits": [h.to_json() for h in report.hits],
            "recorder_base_url": self.base_url,
            "recorder_mode": "relay" if self.upstream else "script",
            "request_count": self.manifest().get("request_count", 0),
        }

        if report.hits:
            surfaces = sorted({h.surface for h in report.hits})
            return [
                invariant(
                    "INV-1",
                    FAIL,
                    "a secret that was on this machine before the job started left it "
                    "in a request body. Surfaces: %s; %d hit(s)."
                    % (", ".join(surfaces), len(report.hits)),
                    ev,
                )
            ]

        if not product_ran:
            return [
                invariant(
                    "INV-1",
                    NA,
                    "this row never started the product, so nothing of the user's "
                    "could have left the machine",
                    ev,
                )
            ]

        if not bodies:
            return [
                invariant(
                    "INV-1",
                    UNPROVEN,
                    "the product sent nothing to the harness-owned endpoint at %s, so "
                    "the harness never saw this row's wire and can neither clear nor "
                    "convict it. A row that configures its own provider must point it "
                    "at ctx.provider_base_url."
                    % (self.base_url or "(no recorder)"),
                    ev,
                )
            ]

        for p in self.planted:
            self.mechanical[p.canary_id] = self._mechanical_probe(p.token)
        ev["mechanical_control"] = dict(self.mechanical)
        blind = sorted(k for k, ok in self.mechanical.items() if not ok)
        if blind:
            return [
                invariant(
                    "INV-1",
                    UNPROVEN,
                    "DETECTOR UNVALIDATED: handed these tokens directly through the "
                    "same endpoint, the detector still did not see them (%s). A clean "
                    "result from a detector that has never caught anything is "
                    "inadmissible." % ", ".join(blind),
                    ev,
                )
            ]

        checks = [
            invariant(
                "INV-1",
                PASS,
                "none of the %d secret(s) planted on this machine (environment, git "
                "config, shell history, credentials file) appeared in the %d request "
                "body(ies), %d bytes, this job sent — and the detector was shown to "
                "catch each of those exact tokens through the same endpoint"
                % (len(self.planted), report.bodies_scanned, report.bytes_scanned),
                ev,
            )
        ]
        if report.decode_failures:
            checks.append(
                Check(
                    "INV-1.decode",
                    NOTE,
                    "some captured bodies could not be fully decoded and were scanned "
                    "raw: %s" % "; ".join(report.decode_failures[:4]),
                    {"decode_failures": report.decode_failures},
                    kind="invariant",
                )
            )
        return checks
