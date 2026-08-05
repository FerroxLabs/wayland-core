/* zombie-probe.c — names the C4 descendant-reaping mechanism, or refutes it.
 *
 * WHAT IT MODELS
 * The 13 C4 tests all assert "teardown left no live descendant". Their liveness
 * probe is, verbatim from crates/wcore-eval-scenarios/tests/runner_contracts.rs:125
 *
 *     let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
 *     result == 0 || !matches!(last_os_error().raw_os_error(), Some(libc::ESRCH))
 *
 * and, verbatim from crates/wcore-sandbox/src/backends/process_tree.rs:848
 *
 *     unsafe { libc::kill(pid, 0) == 0 }
 *
 * `probe_alive()` below reproduces the first (the stricter of the two) exactly.
 *
 * TOPOLOGY (mirrors the "owned descendant listener" shape)
 *   PID 1  = this process, standing in for `cargo nextest run`
 *     |__ A  the direct child, reaped explicitly by us -- as the test reaps its own
 *          |__ B  the owned descendant; orphaned by A's exit, then SIGKILLed
 *
 * B is orphaned, so it reparents to the nearest subreaper. The workspace sets
 * PR_SET_CHILD_SUBREAPER nowhere (verified by grep across crates/), so that is
 * PID 1. If PID 1 does not reap, B stays a zombie and `probe_alive(B)` keeps
 * answering "alive" forever, even though B holds nothing and its listener is dead.
 *
 * ARMS
 *   killed  -- SIGKILL B, then probe. The real teardown case.
 *   alive   -- leave B running, then probe. CONTROL: proves the probe still
 *              reports ALIVE for a genuinely live descendant, i.e. that a
 *              reaping PID 1 does not make these tests unfailable.
 *
 * SELF-TEST (3 assertions, run with `selftest`)
 *   A1 known-positive : a genuinely running process     -> probe says ALIVE
 *   A2 known-negative : a reaped pid                    -> probe says GONE
 *   A3 discriminator  : a ZOMBIE (/proc state 'Z')      -> probe says ALIVE
 *   A3 is the one that proves anything: it shows this probe SHAPE -- the shape
 *   the product tests use -- cannot tell a corpse from a live process. A1+A2
 *   alone pass on a probe that is correct and on one that is not.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

/* Verbatim reproduction of runner_contracts.rs::process_exists (unix arm). */
static int probe_alive(pid_t p) {
    errno = 0;
    int r = kill(p, 0);
    if (r == 0) return 1;
    return errno != ESRCH;
}

/* /proc/<pid>/stat field 3 = state char. 'Z' = zombie, 'R'/'S' = live. */
static char pstate(pid_t p) {
    char path[64];
    snprintf(path, sizeof path, "/proc/%d/stat", p);
    FILE *f = fopen(path, "r");
    if (!f) return '-';                 /* no /proc entry at all */
    static char buf[8192];
    size_t n = fread(buf, 1, sizeof buf - 1, f);
    buf[n] = '\0';
    fclose(f);
    char *rp = strrchr(buf, ')');       /* comm may contain spaces/parens */
    if (!rp || !rp[1] || !rp[2]) return '?';
    return rp[2];
}

static pid_t ppid_of(pid_t p) {
    char path[64];
    snprintf(path, sizeof path, "/proc/%d/stat", p);
    FILE *f = fopen(path, "r");
    if (!f) return -1;
    static char buf[8192];
    size_t n = fread(buf, 1, sizeof buf - 1, f);
    buf[n] = '\0';
    fclose(f);
    char *rp = strrchr(buf, ')');
    if (!rp) return -1;
    pid_t ppid = -1;
    /* after "') '" comes: state ppid pgrp ... */
    if (sscanf(rp + 2, "%*c %d", &ppid) != 1) return -1;
    return ppid;
}

static void msleep(int ms) {
    struct timespec ts = {ms / 1000, (long)(ms % 1000) * 1000000L};
    nanosleep(&ts, NULL);
}

/* Build the A -> B topology. Returns B's pid; A is exited and reaped. */
static pid_t spawn_orphaned_descendant(void) {
    int fds[2];
    if (pipe(fds) != 0) { perror("pipe"); exit(90); }

    pid_t a = fork();
    if (a < 0) { perror("fork A"); exit(90); }
    if (a == 0) {
        close(fds[0]);
        pid_t b = fork();
        if (b < 0) _exit(91);
        if (b == 0) {
            /* B: the owned descendant. Sleeps long enough to be observed. */
            close(fds[1]);
            for (;;) pause();
        }
        /* A: publish B's pid, then exit immediately so B is orphaned. */
        if (write(fds[1], &b, sizeof b) != (ssize_t)sizeof b) _exit(92);
        close(fds[1]);
        _exit(0);
    }
    close(fds[1]);
    pid_t b = -1;
    if (read(fds[0], &b, sizeof b) != (ssize_t)sizeof b) {
        fprintf(stderr, "could not read B pid\n");
        exit(90);
    }
    close(fds[0]);

    /* Reap A specifically -- exactly what the tests do with their direct child.
     * waitpid(<specific pid>) is what Rust's Child::wait() issues; it is NOT
     * wait(-1), so it can never incidentally reap an adopted orphan. */
    int st;
    waitpid(a, &st, 0);
    return b;
}

static int run_arm(const char *arm) {
    pid_t b = spawn_orphaned_descendant();
    msleep(150);                        /* let the reparent settle */

    pid_t reparent = ppid_of(b);
    printf("ARM=%s DESCENDANT=%d REPARENTED_TO=%d SELF=%d\n",
           arm, (int)b, (int)reparent, (int)getpid());

    int killed = (strcmp(arm, "killed") == 0);
    if (killed) kill(b, SIGKILL);

    /* Poll on the tests' own budget shape: up to 3s, 20ms apart. */
    int alive = 1;
    char st = '?';
    for (int i = 0; i < 150; i++) {
        alive = probe_alive(b);
        st = pstate(b);
        if (!alive) break;
        msleep(20);
    }

    printf("ARM=%s PROBE_SAYS=%s PROC_STATE=%c\n",
           arm, alive ? "ALIVE" : "GONE", st);
    printf("ARM=%s VERDICT=%s\n", arm,
           killed ? (alive ? "TEST_WOULD_FAIL" : "TEST_WOULD_PASS")
                  : (alive ? "TEST_WOULD_FAIL" : "TEST_WOULD_PASS"));

    if (!killed) kill(b, SIGKILL);      /* clean up the control arm */
    return 0;
}

static int selftest(void) {
    int pass = 0, fail = 0;
    #define CHECK(name, cond) do { \
        if (cond) { printf("SELFTEST %s: PASS\n", name); pass++; } \
        else      { printf("SELFTEST %s: FAIL\n", name); fail++; } \
    } while (0)

    /* A1 known-positive: a genuinely running child. */
    pid_t live = fork();
    if (live == 0) { for (;;) pause(); }
    msleep(100);
    CHECK("A1_known_positive_live_process_reads_ALIVE", probe_alive(live) == 1);

    /* A3 discriminator: turn that same process into a zombie and re-probe.
     * We kill it and deliberately do NOT wait() -- so it is a corpse we own. */
    kill(live, SIGKILL);
    msleep(200);
    char zst = pstate(live);
    int zalive = probe_alive(live);
    CHECK("A3_zombie_is_indistinguishable_from_live_for_this_probe",
          zst == 'Z' && zalive == 1);
    printf("SELFTEST A3 detail: /proc state=%c probe_alive=%d "
           "(a corpse the probe calls ALIVE)\n", zst, zalive);

    /* A2 known-negative: now reap it; the pid must read GONE. */
    int st;
    waitpid(live, &st, 0);
    msleep(50);
    CHECK("A2_known_negative_reaped_pid_reads_GONE", probe_alive(live) == 0);

    printf("SELFTEST SUMMARY: %d passed, %d failed\n", pass, fail);
    return fail == 0 ? 0 : 1;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    const char *mode = argc > 1 ? argv[1] : "killed";
    printf("PID1_IS=%d COMM_OF_PID1=", (int)getpid());
    {
        FILE *f = fopen("/proc/1/comm", "r");
        char c[128] = "?";
        if (f) { if (!fgets(c, sizeof c, f)) strcpy(c, "?"); fclose(f); }
        c[strcspn(c, "\n")] = '\0';
        printf("%s\n", c);
    }
    if (strcmp(mode, "selftest") == 0) { int rc = selftest(); printf("DONE\n"); return rc; }
    int rc = run_arm(mode);
    printf("DONE\n");
    return rc;
}
