p = "crates/wcore-cli/src/plugin/quarantine.rs"
s = open(p, encoding="utf-8").read()
orig = s
def rep(old, new, count=1):
    global s
    assert s.count(old) == count, (s.count(old), old[:200])
    s = s.replace(old, new, count)

TESTS = r'''
    /// Is `pid` a live (non-reaped) process?
    ///
    /// `kill(pid, 0)` is the only portable oracle here. It also answers "yes"
    /// for a zombie, which is the conservative direction for these tests: a
    /// false "alive" fails them, it never passes them.
    fn is_alive(pid: libc::pid_t) -> bool {
        // SAFETY: signal 0 performs the permission/existence check only.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// Wait up to `budget` for `pid` to disappear.
    fn wait_gone(pid: libc::pid_t, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            if !is_alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        !is_alive(pid)
    }

    /// Prove the liveness oracle can say BOTH things in this process, so a
    /// "the descendant is gone" result below cannot come from an oracle that
    /// only ever says "gone".
    fn assert_oracle_is_bidirectional() {
        let mut probe = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn the oracle control");
        let pid = probe.id() as libc::pid_t;
        assert!(
            is_alive(pid),
            "the liveness oracle failed to see a process it just spawned"
        );
        let _ = probe.kill();
        let _ = probe.wait();
        assert!(
            wait_gone(pid, Duration::from_secs(5)),
            "the liveness oracle failed to see a process it just killed"
        );
    }

    /// A quarantine `git` that times out must take the helpers it spawned with
    /// it — not just its own pid.
    ///
    /// The `setsid` hardening for #338 is what makes this load-bearing: the
    /// descendants are in a session this process does not own, so the previous
    /// `child.kill()` left them running with no owner and nothing else would
    /// ever reap them. A `!`-alias reproduces the exact production shape (a
    /// helper `git` spawns that backgrounds a worker) with no network.
    #[test]
    fn a_timed_out_git_reaps_the_whole_detached_tree() {
        assert_oracle_is_bidirectional();

        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path();
        run_git(&["init", "-q", "."], Some(repo), Duration::from_secs(60)).expect("git init");

        let pidfile = repo.join("worker.pid");
        let alias = format!(
            "alias.wedge=!sh -c 'sleep 300 & echo $! > {} ; sleep 300'",
            pidfile.display()
        );

        let started = Instant::now();
        let err = run_git(
            &["-c", &alias, "wedge"],
            Some(repo),
            Duration::from_millis(1_500),
        )
        .expect_err("the wall-clock guard must fire");
        assert!(
            err.to_string().contains("timed out"),
            "it must be the timeout path that fired: {err}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the guard must fire on its own budget: {:?}",
            started.elapsed()
        );

        // Non-vacuity: the helper really did create a backgrounded descendant.
        let worker: libc::pid_t = std::fs::read_to_string(&pidfile)
            .expect("the helper must have recorded its background worker's pid")
            .trim()
            .parse()
            .expect("worker pid");
        assert!(worker > 0, "worker pid {worker}");

        assert!(
            wait_gone(worker, Duration::from_secs(10)),
            "the background worker {worker} that the timed-out git spawned is STILL ALIVE — \
             killing the direct child does not reach a descendant in the detached session"
        );
    }

    /// The same obligation on the OTHER failure exit: `git` exits promptly but
    /// a helper it spawned holds the inherited pipe, so `join_drain` refuses.
    ///
    /// Enumerated deliberately — `run_git` has two failure shapes that leave a
    /// tree behind, and an entry written from one of them leaves the other to
    /// surface later.
    #[test]
    fn a_pipe_holding_helper_is_reaped_when_the_drain_guard_fires() {
        assert_oracle_is_bidirectional();

        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path();
        run_git(&["init", "-q", "."], Some(repo), Duration::from_secs(60)).expect("git init");

        let pidfile = repo.join("worker.pid");
        let alias = format!(
            "alias.leak=!sh -c 'sleep 300 & echo $! > {} ; exit 0'",
            pidfile.display()
        );

        let err = run_git(&["-c", &alias, "leak"], Some(repo), Duration::from_secs(60))
            .expect_err("the drain guard must fire");
        assert!(
            err.to_string().contains("pipe is still open"),
            "it must be the drain guard that fired, not the wall clock: {err}"
        );

        let worker: libc::pid_t = std::fs::read_to_string(&pidfile)
            .expect("the helper must have recorded its background worker's pid")
            .trim()
            .parse()
            .expect("worker pid");
        assert!(worker > 0, "worker pid {worker}");

        assert!(
            wait_gone(worker, Duration::from_secs(10)),
            "the pipe-holding worker {worker} survived the drain-guard failure — the install \
             reported an error and left an unowned process running"
        );
    }
}
'''

# append inside the existing tests module (replace its final closing brace)
assert s.rstrip().endswith("}")
idx = s.rstrip().rfind("\n}")
s = s[:idx] + "\n" + TESTS.rstrip("\n") + "\n"
open(p, "w", encoding="utf-8").write(s)
print("patch3 applied, delta", len(s) - len(orig))
