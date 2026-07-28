#!/usr/bin/env python3
"""Mechanically resolve every evidence ID declared in CTRL-01.

Re-runnable. Reads .planning/intel/COMPETITIVE-LEDGER.md, extracts every
evidence-ID index row, and for each one resolves the objects its Artifact cell
names by RUNNING something:

  - a path is resolved with os.stat and the first lines captured
  - a 8-40 hex token is resolved with `git cat-file -t`
  - anything that resolves to nothing is recorded UNRESOLVED with the exact
    command that failed and its exact output

Writes one capture file per ID under ids/ and prints a TSV summary.

Usage: python3 resolve-evidence-ids.py <repo-root> <out-dir>
"""
import os
import re
import subprocess
import sys

REPO = os.path.abspath(sys.argv[1])
OUT = os.path.abspath(sys.argv[2])
IDS = os.path.join(OUT, "ids")
os.makedirs(IDS, exist_ok=True)

# argv[3] overrides the ledger path so the mutation harness can point the SAME
# resolver at a mutated COPY. The real ledger is never written by this script.
LEDGER = (os.path.abspath(sys.argv[3]) if len(sys.argv) > 3
          else os.path.join(REPO, ".planning/intel/COMPETITIVE-LEDGER.md"))

# Path-ish token: something containing a / and ending in a known extension, or a
# bare directory reference ending in /
PATH_RE = re.compile(
    r"`([A-Za-z0-9_\-./{}\\|§ ]*?/[A-Za-z0-9_\-./{}\\|§ ]*?"
    r"(?:\.md|\.log|\.log\.gz|\.txt|\.json|\.py|\.rs|\.ts|\.yml|\.yaml|\.toml|/))`"
)
# Bare hex commit token of 8..40 chars.
#
# NOT every hex-looking token in the ledger is a git object, and treating them
# alike manufactures findings. Three kinds occur and are resolved differently:
#
#   COMMIT   - a git object; resolved with `git cat-file -t`
#   DIGEST   - a truncated sha256 of a BUILD ARTIFACT, written with a trailing
#              ellipsis (`e8431ba2…`). `git cat-file` on one is a category
#              error, so it is resolved by grepping the artifact the ledger
#              names for the token.
#   RUN      - a GitHub workflow run/job id: pure decimal, and `[0-9a-f]{8,40}`
#              matches it. Resolved the same way as DIGEST.
SHA_RE = re.compile(r"\b([0-9a-f]{8,40})\b")
DIGEST_RE = re.compile(r"\b([0-9a-f]{8,40})…")
DECIMAL_RE = re.compile(r"^[0-9]+$")


def run(cmd, cwd=REPO):
    p = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
    return p.returncode, (p.stdout + p.stderr).strip()


def candidate_paths(tok):
    """Ledger paths are written relative to .planning/ or to the repo root."""
    tok = tok.strip()
    yield os.path.join(REPO, tok)
    yield os.path.join(REPO, ".planning", tok)


def resolve_path(tok):
    for cand in candidate_paths(tok):
        if os.path.exists(cand):
            return cand
    return None


def head_of(p, n=6):
    if os.path.isdir(p):
        try:
            entries = sorted(os.listdir(p))[:n]
        except OSError as e:
            return f"<listdir failed: {e}>"
        return "\n".join("  " + e for e in entries)
    try:
        with open(p, "rb") as f:
            raw = f.read(4096)
        text = raw.decode("utf-8", "replace")
        return "\n".join(text.splitlines()[:n])
    except OSError as e:
        return f"<read failed: {e}>"


def main():
    with open(LEDGER, encoding="utf-8") as f:
        lines = f.read().splitlines()

    rows = []
    for ln in lines:
        m = re.match(r"^\| `([^`]+)` \| (.*?) \|\s*$", ln)
        if m:
            rows.append((m.group(1), m.group(2)))

    if not rows:
        sys.exit("FATAL: no evidence-ID rows parsed out of the ledger")

    tsv = []
    seen = set()
    for idx, (eid, artifact) in enumerate(rows, start=1):
        if eid in seen:
            continue
        seen.add(eid)
        safe = re.sub(r"[^A-Za-z0-9_.-]", "_", eid)
        capture = os.path.join(IDS, f"{safe}.txt")
        log = []
        log.append(f"EVIDENCE ID : {eid}")
        log.append(f"LEDGER ROW  : {idx}")
        log.append(f"ARTIFACT    : {artifact}")
        log.append("")

        path_toks = [t for t in PATH_RE.findall(artifact)]
        # The ledger's shorthand for a second artifact in the same directory is
        # a BARE filename ("<dir>/A.md + B.md"). Treating B.md as unresolvable
        # would report the ledger defective for the reader's own convention, so
        # bare filenames are resolved against the directory of a full path on
        # the same row. Anything that still does not resolve is a real finding.
        bare_toks = [
            t for t in re.findall(r"`([A-Za-z0-9_.-]+\.(?:md|json|log|txt))`",
                                  artifact)
            if "/" not in t
        ]
        sha_toks = [t for t in SHA_RE.findall(artifact)]
        # A path token whose {n} placeholder makes it non-resolvable is kept and
        # reported as such rather than silently dropped.
        resolved_any = False
        unresolved = []

        log.append("== PATH RESOLUTION ==")
        if not path_toks:
            log.append("(no path token in the artifact cell)")
        for t in path_toks:
            cand = resolve_path(t)
            if cand:
                resolved_any = True
                st = os.stat(cand)
                log.append(f"[OK]   stat {t}")
                log.append(f"       -> {os.path.relpath(cand, REPO)}  size={st.st_size}")
                log.append("       head:")
                log.append(head_of(cand))
            else:
                tried = " ; ".join(
                    os.path.relpath(c, REPO) for c in candidate_paths(t)
                )
                log.append(f"[FAIL] stat {t}")
                log.append(f"       tried: {tried}")
                log.append("       -> No such file or directory")
                unresolved.append(f"path:{t}")
        log.append("")

        digest_toks = set(DIGEST_RE.findall(artifact))
        resolved_files = [
            resolve_path(t) for t in path_toks if resolve_path(t)
        ]
        # Sibling resolution for the bare-filename shorthand.
        if bare_toks:
            log.append("== SIBLING (bare-filename) RESOLUTION ==")
            dirs = {os.path.dirname(r) for r in resolved_files}
            for b in bare_toks:
                found = None
                for d in dirs:
                    cand = os.path.join(d, b)
                    if os.path.exists(cand):
                        found = cand
                        break
                if found:
                    resolved_files.append(found)
                    log.append(
                        f"[OK]   sibling {b} -> {os.path.relpath(found, REPO)}"
                    )
                else:
                    log.append(
                        f"[FAIL] sibling {b} not found beside "
                        + (", ".join(sorted(os.path.relpath(d, REPO)
                                            for d in dirs)) or "(nothing)")
                    )
                    unresolved.append(f"sibling:{b}")
            log.append("")

        log.append("== GIT OBJECT RESOLUTION ==")
        if not sha_toks:
            log.append("(no hex object token in the artifact cell)")
        for s in sha_toks:
            if DECIMAL_RE.match(s):
                kind = "RUN"
            elif s in digest_toks:
                kind = "DIGEST"
            else:
                kind = "COMMIT"

            if kind == "COMMIT":
                rc, out = run(["/usr/bin/git", "cat-file", "-t", s])
                if rc == 0 and out.strip() in ("commit", "tree", "blob", "tag"):
                    resolved_any = True
                    rc2, out2 = run(
                        ["/usr/bin/git", "log", "-1", "--format=%H %ad %s",
                         "--date=short", s]
                    ) if out.strip() == "commit" else (0, "")
                    log.append(f"[OK]   COMMIT git cat-file -t {s} -> {out.strip()}")
                    if out2:
                        log.append(f"       git log -1 {s} -> {out2}")
                else:
                    log.append(f"[FAIL] COMMIT git cat-file -t {s} (rc={rc})")
                    log.append(f"       -> {out}")
                    unresolved.append(f"commit:{s}")
                continue

            # DIGEST / RUN: not a git object. Resolve by locating the token
            # inside the artifact the ledger names for this ID.
            hit = None
            for rf in resolved_files:
                if os.path.isdir(rf):
                    continue
                rc, out = run(["/usr/bin/grep", "-c", "-F", s, rf], cwd="/")
                if rc == 0:
                    hit = (rf, out.strip())
                    break
            if hit:
                resolved_any = True
                log.append(
                    f"[OK]   {kind} {s} found {hit[1]}x in "
                    f"{os.path.relpath(hit[0], REPO)}"
                )
                log.append(
                    f"       cmd: /usr/bin/grep -c -F {s} "
                    f"{os.path.relpath(hit[0], REPO)}"
                )
            else:
                where = ", ".join(
                    os.path.relpath(r, REPO) for r in resolved_files
                ) or "(no resolvable artifact on this row)"
                log.append(f"[FAIL] {kind} {s} not present in: {where}")
                unresolved.append(f"{kind.lower()}:{s}")
        log.append("")

        if unresolved and not resolved_any:
            status = "UNRESOLVED"
        elif unresolved:
            status = "PARTIAL"
        elif resolved_any:
            status = "CONFIRMED"
        else:
            status = "UNRESOLVED"

        log.append(f"== VERDICT == {status}")
        if unresolved:
            log.append("unresolved members: " + ", ".join(unresolved))

        with open(capture, "w", encoding="utf-8") as f:
            f.write("\n".join(log) + "\n")

        tsv.append((eid, status, os.path.relpath(capture, OUT),
                    ";".join(unresolved)))

    for eid, status, rel, unres in tsv:
        print(f"{eid}\t{status}\t{rel}\t{unres}")


if __name__ == "__main__":
    main()
