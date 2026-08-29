#!/usr/bin/env bash
# RESERVE THE OUTER-RETRY EVIDENCE TREE FOR THE RUNNER USER. wayland#1177 c1.
#
# ── WHY THIS EXISTS ────────────────────────────────────────────────────────
#
# In `ci-linux` every compile, lint and test step runs through `$DOCKER_RUN`,
# which deliberately carries no `-u` (see the DOCKER_RUN note in ci.yml), so
# the container runs as **root** against a workspace the runner owns as uid
# 1001. The first container step that compiles anything therefore creates
# `target/` root-owned.
#
# The outer-retry evidence wrapper (`run-tests-with-attempt-evidence.sh`) runs
# on the HOST, as the runner user. It needs to create `target/nextest/ci/
# outer-attempts`, delete `target/nextest/ci/junit.xml` between attempts, and
# copy a failed attempt's report into that directory. All three need the runner
# to own the containing directories. On run 33227927478 it did not, and the
# wrapper died on BOTH attempts at `mkdir -p "$ATTEMPT_DIR"`:
#
#     mkdir: cannot create directory 'target/nextest': Permission denied
#
# before it ever invoked nextest. No test ran, no junit.xml was written, and the
# required `report` check received zero evidence from the whole Linux leg.
#
# ── WHY A SCRIPT AND NOT A BARE `mkdir -p` STEP ────────────────────────────
#
# A bare `mkdir -p target/nextest/ci/outer-attempts` closes this ONLY while it
# runs before every container step that touches the workspace. That ordering is
# invisible in the file and was wrong on its first attempt: the step landed
# after the Desktop contract corpus pre-flight hint, which is a `docker run ...
# cargo run` — a compile — so on every `pull_request` run `target/` was already
# root-owned and the bare mkdir failed with the identical `Permission denied`,
# now reddening an even earlier step. Reproduced 2026-08-29 outside CI:
# root container creates target/, uid 1001 then gets
# `mkdir: cannot create directory 'target/nextest': Permission denied`.
#
# So the ordering is asserted separately (`contract_gate_topology.rs::
# the_outer_retry_evidence_tree_is_reserved_before_any_container_mounts_the_workspace`)
# AND this script repairs the state if it is ever wrong again. Belt and braces,
# because the failure mode is silent for everyone except the one leg that
# carries the whole workspace suite.
#
# The repair is bounded to the four directories on the evidence path. A
# recursive `chown` of `target/` was rejected: it is the same full-tree walk
# that makes a cold workspace slow, run on every job, to repair a tree that
# need never have been root-owned.
#
# Self-tests: .github/scripts/tests/outer-retry-evidence.test.sh (PART D).
set -uo pipefail

TREE="${ATTEMPT_TREE:-target/nextest/ci/outer-attempts}"

# Nothing to do in the ordinary case, which is also the case CI should be in.
if mkdir -p "$TREE" 2>/dev/null && [ -w "$TREE" ]; then
  probe="$TREE/.reserve-probe.$$"
  if : >"$probe" 2>/dev/null; then
    rm -f "$probe"
    echo "reserved ${TREE} for $(id -un) (uid $(id -u))"
    exit 0
  fi
fi

# Something above us is owned by another user. Walk down from the first
# component, creating and handing over ONLY the directories on this path.
echo "::notice title=Outer-retry evidence::${TREE} was not creatable as $(id -un) (uid $(id -u)); repairing the owners on that path. An earlier root container step created target/ before this ran -- see wayland#1177 c1."

if ! sudo -n true 2>/dev/null; then
  echo "::error title=Outer-retry evidence::cannot reserve '${TREE}': it is not creatable as $(id -un) (uid $(id -u)) and passwordless sudo is unavailable, so the owner cannot be repaired. The outer-retry wrapper will die before nextest runs and this leg will contribute NO evidence to the required 'report' check (wayland#1177 c1)." >&2
  exit 1
fi

case "$TREE" in
  /*) path="/" ;;
  *) path="." ;;
esac
IFS='/' read -r -a parts <<<"$TREE"
for part in "${parts[@]}"; do
  [ -n "$part" ] || continue
  path="${path%/}/$part"
  sudo -n mkdir -p "$path" || {
    echo "::error title=Outer-retry evidence::could not create '${path}' even with sudo." >&2
    exit 1
  }
  sudo -n chown "$(id -u):$(id -g)" "$path" || {
    echo "::error title=Outer-retry evidence::could not hand '${path}' to $(id -un)." >&2
    exit 1
  }
done

# Prove it, rather than assuming the chowns did what they said. A reserve step
# that reports success onto an unwritable tree is the original defect with an
# extra step in front of it.
probe="$TREE/.reserve-probe.$$"
if ! : >"$probe" 2>/dev/null; then
  echo "::error title=Outer-retry evidence::'${TREE}' is STILL not writable by $(id -un) after the repair, so the outer-retry wrapper cannot preserve anything." >&2
  exit 1
fi
rm -f "$probe"
echo "repaired and reserved ${TREE} for $(id -un) (uid $(id -u))"
