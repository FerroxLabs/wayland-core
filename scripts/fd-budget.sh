#!/usr/bin/env bash
#
# fd-budget.sh — guarantee the file-descriptor budget a parallel `cargo nextest`
# run needs, or fail loudly naming the resource. Then exec the given command.
#
#   usage: scripts/fd-budget.sh <command> [args...]
#          scripts/fd-budget.sh --self-test
#
# ── WHY THIS EXISTS ────────────────────────────────────────────────────────────
#
# `cargo nextest` runs ONE PROCESS PER TEST. The runner process holds pipe file
# descriptors for every CONCURRENTLY RUNNING test. Measured on hetzner-dsm
# (96 cores, wcore-agent --lib, 2172 tests) by sampling /proc/<runner>/fd:
#
#     --test-threads=96   peak 299 runner fds     (~2.9 fds per test-thread)
#     --test-threads=192  peak 569 runner fds
#     --test-threads=384  peak >=1024 -> EMFILE
#
# With `test-threads = "num-cpus"` (this repo's setting in .config/nextest.toml)
# the runner's peak demand is therefore ~3 x nproc. When that crosses
# RLIMIT_NOFILE, `fork`/`exec` of the test binary fails with
# `Too many open files (os error 24)` and nextest reports the test as
# **exec failed**.
#
# That failure is dangerous specifically because of how it LOOKS:
#
#   * it is counted and printed alongside real test failures;
#   * WHICH tests are hit is pure scheduling luck, so the failing set differs
#     run to run in both directions -- the classic "flaky suite" signature;
#   * `--test-threads=1` makes it vanish, which reads as "a parallelism bug in
#     our tests" and sends people hunting for shared state;
#   * the test process NEVER RAN, so there is no shared state to find.
#
# This has already been misdiagnosed twice in this repo. It was filed under
# `CLASS-ENV-01` ("process-wide env mutation in parallel tests"), which cannot
# be the mechanism -- nextest is process-per-test, so one test's `set_var`
# is invisible to another, and in an exec-failure the test body never executes
# at all. Measured 2026-07-29 on an unchanged tree: of 96 first-try failures at
# --test-threads=384, **96 were exec failures and 0 were test failures**, every
# one carrying `Too many open files (os error 24)`.
#
# ── WHAT THIS DOES ─────────────────────────────────────────────────────────────
#
# Raises the soft RLIMIT_NOFILE toward the hard limit so supply >= demand, and
# if that is impossible, FAILS with a message naming the resource instead of
# letting the run proceed into a nondeterministic EMFILE regime.
#
# It is deliberately NOT a serialisation: concurrency is untouched.
#
# Non-Unix (no working `ulimit`) is a documented pass-through: Windows has no
# RLIMIT_NOFILE and nextest does not exhibit this failure there.

set -u

# fds per concurrently-running test. Measured 2.9; 4 is deliberate headroom for
# nextest version drift and for tests that leak a descriptor back to the runner.
FD_PER_TEST="${FD_PER_TEST:-4}"
# runner's own fixed overhead: stdio, the cargo metadata handles, the binary
# handles, the JUnit writer. Measured ~21 idle; 192 is deliberate headroom.
FD_OVERHEAD="${FD_OVERHEAD:-192}"

log() { printf '%s\n' "$*" >&2; }

detect_threads() {
    # Honour an explicit override first: nextest reads NEXTEST_TEST_THREADS, and
    # an explicit --test-threads on the command line beats the config file.
    for a in "$@"; do
        case "$a" in
            --test-threads=*) printf '%s' "${a#--test-threads=}"; return ;;
        esac
    done
    if [ -n "${NEXTEST_TEST_THREADS:-}" ]; then
        printf '%s' "$NEXTEST_TEST_THREADS"; return
    fi
    # .config/nextest.toml pins test-threads = "num-cpus" for every profile.
    if command -v nproc >/dev/null 2>&1; then
        nproc
    elif command -v sysctl >/dev/null 2>&1 && sysctl -n hw.ncpu >/dev/null 2>&1; then
        sysctl -n hw.ncpu
    else
        printf '8'
    fi
}

is_windows() {
    # Windows has no RLIMIT_NOFILE governing process spawn, and nextest does not
    # exhibit this failure there. Under git-bash `ulimit -Sn` answers with a
    # Unix-shaped number that cannot actually be raised, so without this
    # pass-through the guard would refuse to run a Windows CI job over a limit
    # that does not constrain anything.
    case "${OS:-}" in Windows_NT) return 0 ;; esac
    case "$(uname -s 2>/dev/null)" in
        MINGW*|MSYS*|CYGWIN*|Windows*) return 0 ;;
    esac
    return 1
}

ensure_budget() {
    # Returns 0 if the budget is adequate (raising it if needed), 1 if not.
    local threads required soft hard target
    threads="$1"

    if is_windows; then
        return 0
    fi

    # A non-numeric or non-positive thread count means we cannot reason about
    # demand. Do not guess and do not block the run.
    case "$threads" in
        ''|*[!0-9]*) log "fd-budget: non-numeric test-thread count '$threads'; skipping check"; return 0 ;;
    esac
    [ "$threads" -gt 0 ] 2>/dev/null || return 0

    required=$(( FD_PER_TEST * threads + FD_OVERHEAD ))

    soft=$(ulimit -Sn 2>/dev/null) || { log "fd-budget: no RLIMIT_NOFILE on this platform; pass-through"; return 0; }
    hard=$(ulimit -Hn 2>/dev/null) || hard=unlimited
    [ "$soft" = unlimited ] && return 0

    if [ "$soft" -ge "$required" ] 2>/dev/null; then
        return 0
    fi

    if [ "$hard" = unlimited ]; then
        target="$required"
    elif [ "$hard" -ge "$required" ] 2>/dev/null; then
        target="$hard"
    else
        target="$hard"
    fi

    ulimit -Sn "$target" 2>/dev/null || true
    soft=$(ulimit -Sn 2>/dev/null)
    [ "$soft" = unlimited ] && return 0

    if [ "$soft" -ge "$required" ] 2>/dev/null; then
        log "fd-budget: raised RLIMIT_NOFILE soft limit to $soft (need $required for $threads test-threads)"
        return 0
    fi

    log ""
    log "fd-budget: REFUSING TO RUN — file-descriptor budget too small."
    log ""
    log "  test-threads     : $threads"
    log "  fds required     : $required  (${FD_PER_TEST}/test-thread + ${FD_OVERHEAD} runner overhead)"
    log "  RLIMIT_NOFILE    : soft=$soft hard=$hard"
    log ""
    log "  cargo-nextest runs one process per test and holds pipe fds for every"
    log "  concurrently running test. With this budget it will fail to SPAWN"
    log "  tests part-way through the run. nextest reports those as"
    log "  'exec failed' with 'Too many open files (os error 24)' — they look"
    log "  like flaky test failures, but the tests never ran, and which tests"
    log "  are hit changes every run."
    log ""
    local suggest
    suggest=$(( (soft - FD_OVERHEAD) / FD_PER_TEST ))
    [ "$suggest" -lt 1 ] && suggest=1
    log "  Fix (either):"
    log "    * raise the hard limit:  ulimit -Hn $required   (may need privileges)"
    log "    * lower concurrency:     cargo nextest run --test-threads=$suggest"
    log ""
    return 1
}

# ── self-test ─────────────────────────────────────────────────────────────────
# Three assertions, per the standing rule that a two-assertion self-test passes
# on a broken instrument: known-positive PASSES, known-negative FAILS, and the
# OLD (unguarded) path would have MISSED the known-negative.
self_test() {
    local fails=0 out rc achieved

    # The budget the known-negative assertions rely on. Soft MUST be lowered
    # BEFORE hard: lowering the hard limit below the current soft limit is an
    # error that several shells report only on stderr, leaving the hard limit
    # untouched. That exact mistake made an earlier revision of this self-test
    # report a false FAIL for assertion 2 -- the guard had correctly raised the
    # soft limit back up to an ample hard limit that was never actually lowered.
    # Verify the achieved limits rather than assuming the ulimit calls took.
    achieved=$( ulimit -Sn 64 2>/dev/null; ulimit -Hn 64 2>/dev/null; \
                printf '%s/%s' "$(ulimit -Sn)" "$(ulimit -Hn)" )
    if [ "$achieved" != "64/64" ]; then
        echo "self-test SETUP FAIL: could not establish a 64/64 fd budget (got $achieved);"
        echo "  the known-negative assertions below would be vacuous. Aborting."
        return 1
    fi
    echo "self-test setup: constrained budget established, soft/hard = $achieved"

    # (1) known-positive: an ample budget must pass and must exec the command.
    out=$( ulimit -Sn 4096 2>/dev/null
           NEXTEST_TEST_THREADS=8 "$0" printf KNOWN_POSITIVE_RAN 2>/dev/null )
    rc=$?
    if [ "$rc" -eq 0 ] && [ "$out" = "KNOWN_POSITIVE_RAN" ]; then
        echo "self-test 1 PASS: adequate budget -> guard passes, command runs"
    else
        echo "self-test 1 FAIL: rc=$rc out='$out'"; fails=$((fails + 1))
    fi

    # (2) known-negative: an inadequate budget must FAIL, and must NOT run the
    #     command. 64 fds against 256 test-threads is unambiguously too small.
    out=$( ulimit -Sn 64 2>/dev/null; ulimit -Hn 64 2>/dev/null
           NEXTEST_TEST_THREADS=256 "$0" printf SHOULD_NOT_RUN 2>/dev/null )
    rc=$?
    if [ "$rc" -ne 0 ] && [ "$out" != "SHOULD_NOT_RUN" ]; then
        echo "self-test 2 PASS: inadequate budget -> guard fails loudly, command suppressed"
    else
        echo "self-test 2 FAIL: rc=$rc out='$out' (guard did not block)"; fails=$((fails + 1))
    fi

    # (3) the assertion that proves the guard does anything: under the IDENTICAL
    #     inadequate budget, the OLD unguarded invocation exits 0 and proceeds.
    #     Without this, assertions 1-2 also pass on a guard that is never
    #     consulted, because the command simply runs in both cases.
    out=$( ulimit -Sn 64 2>/dev/null; ulimit -Hn 64 2>/dev/null
           printf SHOULD_NOT_RUN 2>/dev/null )
    rc=$?
    if [ "$rc" -eq 0 ] && [ "$out" = "SHOULD_NOT_RUN" ]; then
        echo "self-test 3 PASS: unguarded path silently proceeds on the same budget the guard rejects"
    else
        echo "self-test 3 FAIL: rc=$rc out='$out' (old path did not proceed; assertion 2 proves nothing)"
        fails=$((fails + 1))
    fi

    if [ "$fails" -eq 0 ]; then
        echo "fd-budget self-test: 3/3 PASS"
        return 0
    fi
    echo "fd-budget self-test: $fails/3 FAILED"
    return 1
}

if [ "${1:-}" = "--self-test" ]; then
    self_test
    exit $?
fi

if [ "$#" -eq 0 ]; then
    log "usage: $0 <command> [args...]   |   $0 --self-test"
    exit 2
fi

ensure_budget "$(detect_threads "$@")" || exit 1
exec "$@"
