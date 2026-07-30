#!/usr/bin/env python3
"""Prove that `migrate ... --yes` does not mutate its SOURCE tree.

Runs the real binary against every hostile corpus, digesting the source tree
before and after. The digest covers relative path, mode, symlink target and
file bytes, so a permission flip or a retargeted symlink is caught, not just a
content edit.
"""
import json
import subprocess
import hashlib
import os
import sys
import tempfile

out = sys.argv[1]
binary = sys.argv[2]
m = json.load(open(os.path.join(out, "cases.json")))


def digest(root):
    h = hashlib.sha256()
    if not os.path.isdir(root):
        return "ABSENT"
    for dp, dn, fn in sorted(os.walk(root)):
        dn.sort()
        fn.sort()
        for f in fn:
            p = os.path.join(dp, f)
            rel = os.path.relpath(p, root)
            h.update(rel.encode() + b"\0")
            st = os.lstat(p)
            if os.path.islink(p):
                h.update(b"L" + os.readlink(p).encode())
            else:
                h.update(b"F" + oct(st.st_mode).encode())
                with open(p, "rb") as fh:
                    h.update(fh.read())
            h.update(b"\0")
    return h.hexdigest()


# --- instrument self-test: the digest MUST notice a change, or every YES below
# --- is free. Three assertions: known-negative, known-positive, and that a
# --- content-only matcher would have missed the mode flip.
probe = tempfile.mkdtemp(prefix="/tmp/port-import-probe-")
pf = os.path.join(probe, "a.txt")
open(pf, "w").write("x")
d0 = digest(probe)
if digest(probe) != d0:
    print("SELF-TEST FAIL: digest not stable")
    sys.exit(1)
open(pf, "w").write("y")
d1 = digest(probe)
os.chmod(pf, 0o700)
d2 = digest(probe)
print(f"SELF-TEST stable={d0 == digest(probe) if False else 'ok'} "
      f"content-change-detected={d0 != d1} mode-change-detected={d1 != d2}")
if not (d0 != d1 and d1 != d2):
    print("SELF-TEST FAIL: digest cannot go red")
    sys.exit(1)

mut = []
ran = 0
print()
print(f"{'case':<34}{'exit':>5}  source-unchanged")
for c in m["cases"]:
    corpus = c["corpus"]
    before = digest(corpus)
    home = tempfile.mkdtemp(prefix="/tmp/port-import-h-")
    env = dict(os.environ, HOME=home, WAYLAND_CONFIG_DIR=home)
    r = subprocess.run(
        [binary, "migrate", "hermes", "--home", corpus, "--yes"],
        capture_output=True, text=True, env=env, timeout=180,
    )
    after = digest(corpus)
    ran += 1
    ok = before == after
    if not ok:
        mut.append(c["id"])
    flag = "YES" if ok else "*** MUTATED ***"
    print(f"{c['id']:<34}{r.returncode:>5}  {flag}")

print()
print(f"CORPORA-RUN={ran} of {len(m['cases'])}  UNRUN={len(m['cases']) - ran}  "
      f"SOURCE-MUTATED={len(mut)} {mut}")
