/*
 * zombie-probe-macos.c — settle, by measurement on real hardware, which macOS
 * API reports a REAL zombie as dead.
 *
 * WHY THIS FILE EXISTS. The Linux arm of `wcore_types::process_liveness` is
 * proven by `crates/wcore-types/tests/real_zombie.rs`, which creates a genuine
 * corpse and asserts both directions. The macOS arm cannot be proven the same
 * way inside this lane: LANE-BRIEF §0 forbids running cargo on the Mac, and
 * neither of the two build hosts available (Linux, Windows) can execute Darwin
 * code. Cross-compiling with `cargo check --target aarch64-apple-darwin`
 * typechecks the arm but proves nothing about kernel behaviour.
 *
 * `cc` is not cargo. So the macOS SEMANTICS are measured here, in C, against a
 * corpse this program creates — and the Rust arm is then written to use
 * whichever API the measurement shows is correct, rather than the one that
 * seemed most likely.
 *
 * Build & run:  cc -o /tmp/zombie-probe-macos zombie-probe-macos.c && /tmp/zombie-probe-macos
 *
 * Exit status: 0 if the measurement completed, 1 if it could not create a
 * corpse (in which case nothing below is a result).
 */

#include <errno.h>
#include <libproc.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/proc_info.h>
#include <sys/sysctl.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

/* <sys/proc.h>: SIDL 1, SRUN 2, SSLEEP 3, SSTOP 4, SZOMB 5 */
#define P_SZOMB 5

/* ---- probe 1: the OLD shape this workspace used everywhere -------------- */
static int old_shape_says_alive(pid_t pid) {
  return kill(pid, 0) == 0;
}

/* ---- probe 2: libproc / proc_pidinfo(PROC_PIDTBSDINFO) ------------------ */
/* Returns pbi_status, or -1 if the call gave nothing back. */
static int libproc_status(pid_t pid) {
  struct proc_bsdinfo info;
  memset(&info, 0, sizeof(info));
  int n = proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, PROC_PIDTBSDINFO_SIZE);
  if (n != (int)PROC_PIDTBSDINFO_SIZE) {
    return -1;
  }
  return (int)info.pbi_status;
}

/* ---- probe 3: sysctl KERN_PROC_PID -> kinfo_proc.kp_proc.p_stat --------- */
/* Returns p_stat, -1 on sysctl error, -2 when it returned zero bytes. */
static int sysctl_p_stat(pid_t pid) {
  int mib[4] = {CTL_KERN, KERN_PROC, KERN_PROC_PID, (int)pid};
  struct kinfo_proc info;
  memset(&info, 0, sizeof(info));
  size_t size = sizeof(info);
  if (sysctl(mib, 4, &info, &size, NULL, 0) != 0) {
    return -1;
  }
  if (size == 0) {
    return -2;
  }
  return (int)info.kp_proc.p_stat;
}

/* ---- probe 4: EXACTLY what the Rust arm does ---------------------------
 * The libc crate has no `kinfo_proc` for Apple targets, so the Rust arm reads
 * the two fields it needs out of a raw byte buffer at fixed offsets, and
 * guards the assumption by reading p_pid back and comparing it to the pid it
 * asked about. This function is that algorithm, in C, so it can be checked
 * against the struct-typed read above on real hardware.
 *
 * out_stat / out_pid are only meaningful when the return value is 1.
 * Return: 1 = read ok, 0 = no such process, -1 = could not tell.
 */
#define P_STAT_OFFSET 36
#define P_PID_OFFSET 40

static int raw_offset_probe(pid_t pid, int *out_stat, int *out_pid) {
  int mib[4] = {CTL_KERN, KERN_PROC, KERN_PROC_PID, (int)pid};
  size_t needed = 0;
  if (sysctl(mib, 4, NULL, &needed, NULL, 0) != 0) {
    return errno == ESRCH ? 0 : -1;
  }
  if (needed == 0) {
    return 0;
  }
  unsigned char *buf = calloc(1, needed);
  if (!buf) {
    return -1;
  }
  size_t size = needed;
  if (sysctl(mib, 4, buf, &size, NULL, 0) != 0) {
    int rc = errno == ESRCH ? 0 : -1;
    free(buf);
    return rc;
  }
  if (size == 0) {
    free(buf);
    return 0;
  }
  if (size < P_PID_OFFSET + 4) {
    free(buf);
    return -1;
  }
  int stat = (int)(signed char)buf[P_STAT_OFFSET];
  int readback;
  memcpy(&readback, buf + P_PID_OFFSET, sizeof(readback));
  free(buf);
  if (readback != (int)pid) {
    /* Layout drifted. Report "cannot tell", never a wrong answer. */
    return -1;
  }
  *out_stat = stat;
  *out_pid = readback;
  return 1;
}

static void report(const char *label, pid_t pid) {
  int old = old_shape_says_alive(pid);
  errno = 0;
  int lp = libproc_status(pid);
  int lp_errno = errno;
  int sc = sysctl_p_stat(pid);
  /* p_pid read back through the same struct -- the Rust arm uses this as a
     layout self-check, so confirm it agrees with the pid we asked about. */
  int mib[4] = {CTL_KERN, KERN_PROC, KERN_PROC_PID, (int)pid};
  struct kinfo_proc info;
  memset(&info, 0, sizeof(info));
  size_t size = sizeof(info);
  int readback = -1;
  if (sysctl(mib, 4, &info, &size, NULL, 0) == 0 && size > 0) {
    readback = (int)info.kp_proc.p_pid;
  }
  int raw_stat = -99, raw_pid = -99;
  int raw_rc = raw_offset_probe(pid, &raw_stat, &raw_pid);
  const char *verdict = raw_rc == 1 ? (raw_stat == P_SZOMB ? "DEAD(zombie)" : "LIVE")
                                    : (raw_rc == 0 ? "DEAD(gone)" : "INDETERMINATE");

  printf("%-34s pid=%-7d kill(pid,0)_says_alive=%d  proc_pidinfo.pbi_status=%d (errno=%d %s)  "
         "sysctl.p_stat=%d  sysctl.p_pid_readback=%d  || RUST-ARM-ALGORITHM: rc=%d raw_p_stat=%d "
         "raw_p_pid=%d -> %s\n",
         label, (int)pid, old, lp, lp_errno, lp < 0 ? strerror(lp_errno) : "-", sc, readback,
         raw_rc, raw_stat, raw_pid, verdict);
  fflush(stdout);
}

int main(void) {
  printf("== zombie-probe-macos ==\n");
  printf("SZOMB == %d\n", P_SZOMB);

  /* The Rust arm cannot use libc::kinfo_proc -- the libc crate does not
     define it for Apple targets (measured: E0425 on
     `cargo check --target aarch64-apple-darwin`). So the Rust side declares
     the prefix of the struct itself, and these offsets are the ABI facts it
     is declared against. Printed rather than assumed. */
  printf("ABI sizeof(struct kinfo_proc)        = %zu\n", sizeof(struct kinfo_proc));
  printf("ABI offsetof(kp_proc.p_stat)         = %zu\n",
         (size_t)((char *)&((struct kinfo_proc *)0)->kp_proc.p_stat - (char *)0));
  printf("ABI offsetof(kp_proc.p_pid)          = %zu\n",
         (size_t)((char *)&((struct kinfo_proc *)0)->kp_proc.p_pid - (char *)0));
  printf("ABI sizeof(struct extern_proc)       = %zu\n", sizeof(struct extern_proc));

  /* ---- ARM A: a REAL corpse. Fork, let it exit, never wait(). ---------- */
  pid_t corpse = fork();
  if (corpse < 0) {
    perror("fork");
    return 1;
  }
  if (corpse == 0) {
    _exit(7);
  }
  /* Let the child reach its zombie state. Bounded, and corroborated by ps. */
  for (int i = 0; i < 200; i++) {
    if (sysctl_p_stat(corpse) == P_SZOMB) {
      break;
    }
    usleep(20 * 1000);
  }

  report("ARM A: real unreaped corpse", corpse);

  /* Independent oracle: ps, which is not this program. */
  char cmd[128];
  snprintf(cmd, sizeof(cmd), "ps -o state= -p %d", (int)corpse);
  printf("ARM A independent oracle (ps): ");
  fflush(stdout);
  int ps_rc = system(cmd);
  printf("   (ps exit=%d)\n", WEXITSTATUS(ps_rc));
  fflush(stdout);

  /* ---- ARM B: a genuinely LIVE process. The control. ------------------- */
  /* Without this arm, an API that answered "dead" for everything would look
     correct in ARM A. */
  pid_t live = fork();
  if (live < 0) {
    perror("fork");
    return 1;
  }
  if (live == 0) {
    sleep(30);
    _exit(0);
  }
  usleep(200 * 1000);
  report("ARM B: genuinely live process", live);

  /* ---- ARM D: a LIVE process owned by another user (pid 1 = launchd). ---
     This is the arm that disqualifies "proc_pidinfo failed => dead": if
     proc_pidinfo also fails for a live process we merely lack rights to
     inspect, then treating its failure as death is universal denial. */
  report("ARM D: live, other user (launchd)", 1);

  /* ---- ARM C: fully reaped. --------------------------------------------*/
  pid_t reaped = fork();
  if (reaped == 0) {
    _exit(0);
  }
  int st = 0;
  waitpid(reaped, &st, 0);
  report("ARM C: fully reaped (gone)", reaped);

  /* cleanup */
  kill(live, SIGKILL);
  waitpid(live, &st, 0);
  waitpid(corpse, &st, 0);

  printf("\nREADING: the correct macOS arm is whichever probe reports %d (SZOMB)\n"
         "for ARM A while reporting a NON-%d live state for ARM B.\n",
         P_SZOMB, P_SZOMB);
  return 0;
}
