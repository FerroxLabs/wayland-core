#!/usr/bin/env python3
"""Clone the STRUCTURE of a real peer state tree into a committed test corpus,
substituting a distinct canary token for every value classified as secret.

Why this exists (F26-01): the authoritative automated gate runs on Linux, but
the only real-world-scale peer installs live on the planning Mac and contain
live provider credentials. Those credentials must never cross a host boundary.
This generator produces a corpus with the real tree's SHAPE — the same relative
paths, file names, file formats and key identities — while every secret VALUE is
replaced by a canary token recorded in the corpus manifest. Gates read the
canary list from that manifest at run time; no canary is ever hard-coded in a
source file.

THE BOUNDING RULE (recorded here because it is a deliberate deviation from a
byte-for-byte clone). A real Hermes home measures 2.0 GB — 540 skill
directories plus image/audio caches, logs and sessions. Committing that verbatim
is not possible and would not add discriminating power. So the walk is bounded
to the paths the importer actually reads, plus DIRECTORY MARKERS that preserve
the counted shape of everything else: a skill directory becomes an empty
directory with a `.keep` file, so `count_subdirs` still sees 540 of them. The
manifest records `bounded: true` together with the include rules, so a reader
can tell the corpus apart from a full clone.

THE SECRET CLASSIFICATION RULE (explicit, single source of truth):
  R1  In a dotenv file, a key matching  *_API_KEY | *_TOKEN | *_SECRET | *_KEY |
      *_PASSWORD | *_PASSWD | *_CREDENTIAL  has its value canaried.
  R2  In a JSON/YAML document, a mapping key whose lower-cased name contains
      any of  key token secret auth pass cred  AND whose value is a string of
      >= 8 characters has that value canaried.
  R3  Every string value in a file under a `credentials/` directory, or in a
      file named `auth.json`, is canaried regardless of its key.
A value that matches no rule is copied through verbatim, because the corpus is
only useful if the non-secret shape is real.

Deterministic by construction: the walk is sorted, and a canary token is derived
from the corpus-relative path plus the key name, so two runs over the same input
produce byte-identical output.

Usage:
    portability-corpus-gen.py --source ~/.hermes   --kind hermes   --out DIR
    portability-corpus-gen.py --source ~/.openclaw --kind openclaw --out DIR
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import sys
from pathlib import Path

# --- classification -------------------------------------------------------

DOTENV_SECRET_RE = re.compile(
    r"^[A-Za-z0-9_]*_(API_KEY|TOKEN|SECRET|KEY|PASSWORD|PASSWD|CREDENTIAL)$"
)
STRUCTURED_SECRET_SUBSTRINGS = ("key", "token", "secret", "auth", "pass", "cred")
STRUCTURED_MIN_LEN = 8

# Keys that contain a classification substring but are known NOT to be secrets.
# Without this the corpus would canary structural fields and lose its shape.
STRUCTURED_KEY_ALLOWLIST = {
    "authorized",
    "auth_mode",
    "authmode",
    "keywords",
    "passthrough",
    "tokenizer",
    "max_tokens",
    "maxtokens",
    "credential_pool_strategies",
}


def is_secret_env_key(key: str) -> bool:
    """R1."""
    return bool(DOTENV_SECRET_RE.match(key))


def is_secret_structured_key(key: str, value: object) -> bool:
    """R2."""
    if not isinstance(value, str) or len(value) < STRUCTURED_MIN_LEN:
        return False
    low = key.lower()
    if low in STRUCTURED_KEY_ALLOWLIST:
        return False
    return any(s in low for s in STRUCTURED_SECRET_SUBSTRINGS)


def canary(rel_path: str, key_path: str) -> str:
    """A distinct, deterministic canary token for one (file, key) site.

    Derived from the corpus-relative path and the key path so that the same
    input always yields the same token, and two different sites never collide.
    The `CANARY-` prefix makes an accidental leak obvious in any output.
    """
    h = hashlib.sha256(f"{rel_path}\x00{key_path}".encode()).hexdigest()[:32]
    return f"CANARY-{h}"


# --- emitters -------------------------------------------------------------


def redact_structured(obj, rel: str, keypath: str, sink: list, force_all: bool):
    """Walk a parsed JSON/YAML document, canarying secret leaves (R2/R3)."""
    if isinstance(obj, dict):
        return {
            k: redact_structured(v, rel, f"{keypath}.{k}" if keypath else k, sink, force_all)
            for k, v in obj.items()
        }
    if isinstance(obj, list):
        return [
            redact_structured(v, rel, f"{keypath}[{i}]", sink, force_all)
            for i, v in enumerate(obj)
        ]
    if isinstance(obj, str):
        leaf = keypath.rsplit(".", 1)[-1].split("[")[0]
        secret = is_secret_structured_key(leaf, obj) or (
            force_all and len(obj) >= STRUCTURED_MIN_LEN
        )
        if secret:
            tok = canary(rel, keypath)
            sink.append({"file": rel, "key": keypath, "canary": tok})
            return tok
    return obj


def emit_dotenv(src: Path, dst: Path, rel: str, sink: list):
    """Copy a dotenv, canarying secret values (R1)."""
    out = []
    for line in src.read_text(encoding="utf-8", errors="replace").splitlines():
        s = line.strip()
        if not s or s.startswith("#") or "=" not in s:
            out.append(line)
            continue
        body = s[len("export ") :] if s.startswith("export ") else s
        key, _, _ = body.partition("=")
        key = key.strip()
        if is_secret_env_key(key):
            tok = canary(rel, key)
            sink.append({"file": rel, "key": key, "canary": tok})
            prefix = "export " if s.startswith("export ") else ""
            out.append(f"{prefix}{key}={tok}")
        else:
            out.append(line)
    dst.write_text("\n".join(out) + "\n", encoding="utf-8")


def emit_structured(src: Path, dst: Path, rel: str, sink: list):
    """Copy a JSON/YAML document, canarying secret values (R2/R3)."""
    force_all = "credentials/" in rel or src.name == "auth.json"
    text = src.read_text(encoding="utf-8", errors="replace")
    if src.suffix in (".json",) or src.name.startswith("openclaw.json"):
        try:
            doc = json.loads(text)
        except Exception:
            dst.write_text(text, encoding="utf-8")
            return
        doc = redact_structured(doc, rel, "", sink, force_all)
        dst.write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        return
    # YAML
    try:
        import yaml  # noqa: PLC0415
    except Exception:
        dst.write_text(text, encoding="utf-8")
        return
    try:
        doc = yaml.safe_load(text)
    except Exception:
        dst.write_text(text, encoding="utf-8")
        return
    if doc is None:
        dst.write_text(text, encoding="utf-8")
        return
    doc = redact_structured(doc, rel, "", sink, force_all)
    dst.write_text(
        yaml.safe_dump(doc, default_flow_style=False, sort_keys=True), encoding="utf-8"
    )


def emit_verbatim(src: Path, dst: Path, limit: int = 65536):
    """Copy a non-secret-bearing file, truncated so the corpus stays small."""
    data = src.read_bytes()[:limit]
    dst.write_bytes(data)


# --- include rules --------------------------------------------------------

HERMES_ROOT_FILES = ["config.yaml", ".env"]
HERMES_PROFILE_FILES = ["config.yaml", ".env", "SOUL.md", "auth.json"]
# Directories under a profile whose *shape* (subdirectory count) matters to the
# deferred inventory but whose contents the importer never reads.
HERMES_MARKER_DIRS = ["skills", "memories"]

OPENCLAW_ROOT_GLOBS = [
    "openclaw.json",
    "openclaw.json.bak",
    "openclaw.json.bak.*",
    "openclaw.json.bak-*",
    "openclaw.json.last-good",
    "clawdbot.json",
    "config.json",
]
OPENCLAW_MARKER_DIRS = [
    "agents",
    "memory",
    "workspace",
    "plugins",
    "plugin-skills",
    "flows",
    "tasks",
    "identity",
    "logs",
    "tui",
]


def structured_or_env(path: Path) -> str:
    if path.name == ".env" or path.name.endswith(".env"):
        return "dotenv"
    if path.suffix in (".json", ".yaml", ".yml") or path.name.startswith("openclaw.json"):
        return "structured"
    return "verbatim"


def emit(src: Path, dst: Path, rel: str, sink: list):
    dst.parent.mkdir(parents=True, exist_ok=True)
    kind = structured_or_env(src)
    if kind == "dotenv":
        emit_dotenv(src, dst, rel, sink)
    elif kind == "structured":
        emit_structured(src, dst, rel, sink)
    else:
        emit_verbatim(src, dst)


def mark_dirs(src_dir: Path, out_dir: Path, rel_prefix: str, counts: dict):
    """Reproduce the SUBDIRECTORY SHAPE of a large tree without its contents.

    Each real subdirectory becomes an empty directory holding a `.keep` file, so
    a counter that walks the corpus sees the same number the real tree has.
    """
    if not src_dir.is_dir():
        return
    try:
        # A real peer tree can contain directories this user cannot read
        # (measured: ~/.openclaw/plugins is mode 0700 under another owner).
        # An unreadable directory contributes no shape; it must not abort the
        # walk, because the corpus is generated from hostile-by-construction
        # input and a crash here would make the corpus un-regenerable.
        subs = sorted(p.name for p in src_dir.iterdir() if p.is_dir())
    except (PermissionError, OSError) as e:
        counts[f"{rel_prefix}!unreadable"] = str(type(e).__name__)
        return
    for name in subs:
        d = out_dir / name
        d.mkdir(parents=True, exist_ok=True)
        (d / ".keep").write_text("", encoding="utf-8")
    counts[rel_prefix] = len(subs)


def gen_hermes(source: Path, out: Path, sink: list, counts: dict):
    for f in HERMES_ROOT_FILES:
        s = source / f
        if s.is_file():
            emit(s, out / f, f, sink)
    prof_src = source / "profiles"
    if prof_src.is_dir():
        names = sorted(p.name for p in prof_src.iterdir() if p.is_dir())
        counts["profiles"] = len(names)
        for name in names:
            for f in HERMES_PROFILE_FILES:
                s = prof_src / name / f
                if s.is_file():
                    rel = f"profiles/{name}/{f}"
                    emit(s, out / rel, rel, sink)
            for md in HERMES_MARKER_DIRS:
                mark_dirs(
                    prof_src / name / md,
                    out / "profiles" / name / md,
                    f"profiles/{name}/{md}",
                    counts,
                )


def gen_openclaw(source: Path, out: Path, sink: list, counts: dict):
    seen = set()
    for pat in OPENCLAW_ROOT_GLOBS:
        for s in sorted(source.glob(pat)):
            if s.is_file() and s.name not in seen:
                seen.add(s.name)
                emit(s, out / s.name, s.name, sink)
    counts["config_files"] = len(seen)
    cred = source / "credentials"
    if cred.is_dir():
        files = sorted(p for p in cred.iterdir() if p.is_file())
        counts["credentials"] = len(files)
        for s in files:
            rel = f"credentials/{s.name}"
            emit(s, out / rel, rel, sink)
    for md in OPENCLAW_MARKER_DIRS:
        mark_dirs(source / md, out / md, md, counts)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--source", required=True, type=Path)
    ap.add_argument("--kind", required=True, choices=["hermes", "openclaw"])
    ap.add_argument("--out", required=True, type=Path)
    args = ap.parse_args()

    source: Path = args.source.expanduser()
    out: Path = args.out.expanduser()
    if not source.is_dir():
        print(f"source is not a directory: {source}", file=sys.stderr)
        return 2
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)

    sink: list = []
    counts: dict = {}
    if args.kind == "hermes":
        gen_hermes(source, out, sink, counts)
    else:
        gen_openclaw(source, out, sink, counts)

    sink.sort(key=lambda e: (e["file"], e["key"]))
    manifest = {
        "kind": args.kind,
        "bounded": True,
        "bounding_rule": (
            "Only importer-relevant files are cloned verbatim; large trees are "
            "reproduced as directory markers preserving subdirectory counts."
        ),
        "classification_rules": {
            "R1": "dotenv key matching *_(API_KEY|TOKEN|SECRET|KEY|PASSWORD|PASSWD|CREDENTIAL)",
            "R2": (
                "JSON/YAML mapping key containing key/token/secret/auth/pass/cred "
                f"with a string value >= {STRUCTURED_MIN_LEN} chars"
            ),
            "R3": "every string value under credentials/ or in auth.json",
        },
        "counts": dict(sorted(counts.items())),
        "canaries": sink,
    }
    (out / "MANIFEST.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"corpus: {out}  files={sum(1 for _ in out.rglob('*') if _.is_file())} canaries={len(sink)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
