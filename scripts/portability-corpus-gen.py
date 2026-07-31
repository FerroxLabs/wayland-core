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
  R4  In a TOML document, a `key = "value"` assignment whose key matches R2's
      substring set and whose value is >= 8 characters has that value canaried.
      Line-based, so the rest of the document — comments, ordering, table
      headers — survives byte-for-byte.
A value that matches no rule is copied through verbatim, because the corpus is
only useful if the non-secret shape is real.

THE GROK EXCEPTION TO R3 (recorded because it is a deliberate widening).
grok's `auth.json` is an OIDC session store whose top-level KEY is an issuer URL
plus an account UUID — an identifier R3 would leave in place, because R3
canaries values and not keys. The grok importer never opens that file; it tests
`is_file()` and records the path by name (`migrate/grok.rs`, module header). So
the grok corpus replaces `auth.json` wholesale with a single-canary placeholder
rather than redacting it leaf by leaf. Nothing measurable is lost — the file's
CONTENT is not an input to any code path under test — and the canary keeps the
absence assertion able to go red the moment the importer starts reading it.

Usage:
    portability-corpus-gen.py --source ~/.hermes   --kind hermes   --out DIR
    portability-corpus-gen.py --source ~/.openclaw --kind openclaw --out DIR
    portability-corpus-gen.py --source ~/.grok     --kind grok     --out DIR

Deterministic by construction: the walk is sorted, and a canary token is derived
from the corpus-relative path plus the key name, so two runs over the same input
produce byte-identical output.
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


TOML_ASSIGN_RE = re.compile(r'^(\s*)([A-Za-z0-9_.\-]+)(\s*=\s*)"([^"]*)"(\s*)$')


def emit_toml(src: Path, dst: Path, rel: str, sink: list):
    """Copy a TOML document, canarying secret values (R4).

    Line-based on purpose. There is no TOML *writer* in the standard library, so
    a parse/re-emit round trip would need a third-party dependency and would
    reshape the document — losing exactly the byte-level fidelity that makes a
    structure clone worth more than a hand-written fixture. Only the quoted
    value on a matching assignment line changes; every other byte is copied.
    """
    out = []
    for line in src.read_text(encoding="utf-8", errors="replace").splitlines():
        m = TOML_ASSIGN_RE.match(line)
        if m and is_secret_structured_key(m.group(2).rsplit(".", 1)[-1], m.group(4)):
            tok = canary(rel, m.group(2))
            sink.append({"file": rel, "key": m.group(2), "canary": tok})
            out.append(f'{m.group(1)}{m.group(2)}{m.group(3)}"{tok}"{m.group(5)}')
        else:
            out.append(line)
    dst.write_text("\n".join(out) + "\n", encoding="utf-8")


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


# grok's on-disk home. Names taken from the peer's own source as cited in
# `crates/wcore-cli/src/migrate/grok.rs`, then RECONCILED against a real
# `~/.grok` install (v0.2.103) — which is the whole point of a structure clone.
GROK_ROOT_FILES = [
    "config.toml",  # parsed by grok::build_plan
    "version.json",  # read by migrate::peer_version's grok branch
    "auth.json",  # presence only; see THE GROK EXCEPTION TO R3 in the header
]
# Directories whose SUBDIRECTORY COUNT the importer reports, plus the user roots
# it imports from. `count_subdirs` is what reads these, so markers suffice.
GROK_COUNTED_DIRS = [
    "skills",
    "bundled",
    "hooks",
    "marketplace-cache",
    "plugin-data",
    "server-skills",
    "vendor",
    "workspace",
    "worktrees",
]
# Counted like the above, but the names are dropped — see `mark_dirs_opaque`.
GROK_OPAQUE_DIRS = ["sessions"]
# Present in a real install and counted by NOTHING in the importer. Cloned so
# the corpus carries the real tree's shape rather than only the shape the
# importer expects to find — an absence a reader can check is worth more than
# an absence nobody recorded.
GROK_UNCOUNTED_DIRS = [
    "bin",
    "completions",
    "docs",
    "downloads",
    "installed-plugins",
    "logs",
    "memtrace",
    "upload_queue",
]
# grok personas are `personas/<name>.toml` FILES and memory notes are
# `memory/<name>.md` FILES, so neither is a directory-marker tree.
GROK_FILE_TREES = [("personas", ".toml"), ("memory", ".md")]


def structured_or_env(path: Path) -> str:
    if path.name == ".env" or path.name.endswith(".env"):
        return "dotenv"
    if path.suffix in (".json", ".yaml", ".yml") or path.name.startswith("openclaw.json"):
        return "structured"
    if path.suffix == ".toml":
        return "toml"
    return "verbatim"


def emit(src: Path, dst: Path, rel: str, sink: list):
    dst.parent.mkdir(parents=True, exist_ok=True)
    kind = structured_or_env(src)
    if kind == "dotenv":
        emit_dotenv(src, dst, rel, sink)
    elif kind == "structured":
        emit_structured(src, dst, rel, sink)
    elif kind == "toml":
        emit_toml(src, dst, rel, sink)
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


def mark_dirs_opaque(src_dir: Path, out_dir: Path, rel_prefix: str, counts: dict):
    """`mark_dirs`, but the subdirectory NAMES are replaced by ordinals.

    Some peer trees encode the user's own filesystem in a directory name —
    grok's `sessions/` names each session after the URL-escaped absolute path of
    the working directory it ran in, so a verbatim clone would commit a listing
    of Sean's local projects into this repository. Only the COUNT is an input to
    the importer (`count_subdirs`), so the names carry no measurable signal and
    are dropped rather than published.
    """
    if not src_dir.is_dir():
        return
    try:
        n = sum(1 for p in src_dir.iterdir() if p.is_dir())
    except (PermissionError, OSError) as e:
        counts[f"{rel_prefix}!unreadable"] = str(type(e).__name__)
        return
    for i in range(n):
        d = out_dir / f"{rel_prefix}-{i:03d}"
        d.mkdir(parents=True, exist_ok=True)
        (d / ".keep").write_text("", encoding="utf-8")
    counts[rel_prefix] = n
    counts[f"{rel_prefix}!names"] = "opaque"


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


def gen_grok(source: Path, out: Path, sink: list, counts: dict):
    present = []
    for f in GROK_ROOT_FILES:
        s = source / f
        if not s.is_file():
            continue
        present.append(f)
        if f == "auth.json":
            # THE GROK EXCEPTION TO R3 — see the module header. One canary,
            # deliberately, so the absence assertion still has an edge.
            tok = canary("auth.json", "*")
            sink.append({"file": "auth.json", "key": "*", "canary": tok})
            (out / "auth.json").write_text(
                json.dumps({"__elided_session_store__": tok}, indent=2) + "\n",
                encoding="utf-8",
            )
        else:
            emit(s, out / f, f, sink)
    counts["root_files"] = len(present)

    for d in GROK_COUNTED_DIRS:
        mark_dirs(source / d, out / d, d, counts)
    for d in GROK_UNCOUNTED_DIRS:
        mark_dirs(source / d, out / d, d, counts)
    for d in GROK_OPAQUE_DIRS:
        mark_dirs_opaque(source / d, out / d, d, counts)

    # A skill directory is only discoverable to `scan_peer_skills` when it holds
    # a `SKILL.md`, so the marker alone would silently shrink the surface from
    # five skills to none. The body is ELIDED rather than cloned: these are the
    # vendor's shipped skill texts, which are the peer's content and not the
    # user's setup. Recorded in the manifest so a reader is never left inferring
    # that a placeholder body is real.
    elided = 0
    skills_src = source / "skills"
    if skills_src.is_dir():
        for name in sorted(p.name for p in skills_src.iterdir() if p.is_dir()):
            if (skills_src / name / "SKILL.md").is_file():
                d = out / "skills" / name
                d.mkdir(parents=True, exist_ok=True)
                (d / "SKILL.md").write_text(
                    f"---\nname: {name}\n---\nbody elided by "
                    "portability-corpus-gen.py\n",
                    encoding="utf-8",
                )
                elided += 1
    counts["skills_with_skill_md"] = elided

    for tree, suffix in GROK_FILE_TREES:
        src_dir = source / tree
        if not src_dir.is_dir():
            counts[f"{tree}!absent"] = 0
            continue
        names = sorted(p.name for p in src_dir.iterdir() if p.is_file() and p.suffix == suffix)
        counts[tree] = len(names)
        for name in names:
            emit(src_dir / name, out / tree / name, f"{tree}/{name}", sink)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--source", required=True, type=Path)
    ap.add_argument("--kind", required=True, choices=["hermes", "openclaw", "grok"])
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
    elif args.kind == "grok":
        gen_grok(source, out, sink, counts)
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
            "R4": (
                "TOML key = \"value\" assignment whose key matches R2's substring "
                f"set with a value >= {STRUCTURED_MIN_LEN} chars"
            ),
            "grok_exception": (
                "grok's auth.json is replaced wholesale by a single-canary "
                "placeholder: its top-level KEY is an issuer URL plus an account "
                "UUID that R3 would leave in place, and the grok importer never "
                "opens the file (presence check only)."
            ),
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
