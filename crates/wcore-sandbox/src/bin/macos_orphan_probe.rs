//! macOS orphan probe — the *owner* half of
//! `tests/hard_process_containment_macos.rs`'s SIGKILL acceptance cases.
//!
//! It stands in for the agent process: it spawns one or more contained process
//! trees, takes the `ProcessTreeGuard` that owns each, and then blocks forever.
//! The test SIGKILLs this process, so NO Rust destructor of any kind runs —
//! which is the whole point. Anything that survives is graded from the process
//! table by the test, never from anything this binary reports.
//!
//! argv: `<status-file> <work-dir> <tree-count>`
//!
//! On success it publishes (atomically, via rename) one `TREE=` line per owned
//! tree plus one `STRAY=` line:
//!
//! ```text
//! TREE=<root pid>,<process group>,<backgrounded grandchild pid>
//! STRAY=<pid of the fd-inheritance witness, NOT contained>
//! ```
//!
//! Exit codes: 2 bad argv, 3 setup failure, 4 not macOS. It never exits 0.

#[cfg(target_os = "macos")]
fn main() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use wcore_sandbox::backends::process_tree::{ProcessTreeGuard, isolate_std};

    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: macos_orphan_probe <status-file> <work-dir> <tree-count>");
        std::process::exit(2);
    }
    let status_path = std::path::PathBuf::from(&args[1]);
    let work_dir = std::path::PathBuf::from(&args[2]);
    let Ok(tree_count) = args[3].parse::<usize>() else {
        eprintln!("macos_orphan_probe: tree count must be a positive integer");
        std::process::exit(2);
    };

    // Guards are held for the life of the process; only SIGKILL ends it.
    let mut guards: Vec<ProcessTreeGuard> = Vec::new();
    let mut published = String::new();

    for index in 0..tree_count {
        let descendant_path = work_dir.join(format!("descendant{index}.pid"));
        // Root plus one backgrounded grandchild: a tree, not a single process,
        // so "the direct child was killed" cannot pass for "the tree was
        // reaped".
        //
        // Trees after the first are spawned while an EARLIER guard is already
        // live, which is what makes descriptor inheritance load-bearing: the
        // earlier guard's owner-death channel must not survive in this new
        // workload, and the earlier guard's sentinel must not be pinned open by
        // a later one.
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(format!(
                "/bin/sh -c 'echo $$ > \"{}\"; exec /bin/sleep 300' & exec /bin/sleep 300",
                descendant_path.display()
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        isolate_std(&mut command);
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                eprintln!("macos_orphan_probe: spawn contained tree {index}: {error}");
                std::process::exit(3);
            }
        };
        let root = child.id();
        match ProcessTreeGuard::new(Some(root)) {
            Ok(guard) => guards.push(guard),
            Err(error) => {
                eprintln!("macos_orphan_probe: own contained tree {index}: {error}");
                std::process::exit(3);
            }
        }
        let Some(descendant) = wait_for_recorded_pid(&descendant_path) else {
            eprintln!("macos_orphan_probe: contained tree {index} never recorded its grandchild");
            std::process::exit(3);
        };
        // SAFETY: `getpgid` is read-only and takes the positive PID just spawned.
        let group = unsafe { libc::getpgid(root as libc::pid_t) };
        if group <= 0 {
            eprintln!("macos_orphan_probe: contained tree {index} has no process group");
            std::process::exit(3);
        }
        published.push_str(&format!("TREE={root},{group},{descendant}\n"));
    }

    // One more child, spawned AFTER every guard exists and deliberately NOT
    // contained. In the real agent this is simply the next tool invocation.
    // Here it is the witness for a property the fix depends on: a guard's
    // owner-death channel must be close-on-exec. If it is not, this ordinary
    // unrelated process holds a duplicate of that channel open, the guard's
    // sentinel never sees the owner die, and the contained tree outlives us.
    let stray = match Command::new("/bin/sleep")
        .arg("300")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(stray) => stray,
        Err(error) => {
            eprintln!("macos_orphan_probe: spawn fd-inheritance witness: {error}");
            std::process::exit(3);
        }
    };
    published.push_str(&format!("STRAY={}\n", stray.id()));

    let temporary = status_path.with_extension("partial");
    let write = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(published.as_bytes())?;
        file.sync_all()?;
        drop(file);
        // Renamed rather than written in place so the test can never read a
        // half-written status and mistake a missing tree for a dead one.
        std::fs::rename(&temporary, &status_path)
    })();
    if let Err(error) = write {
        eprintln!("macos_orphan_probe: publish status: {error}");
        std::process::exit(3);
    }

    // Block forever holding every guard. The test kills us with SIGKILL.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

#[cfg(target_os = "macos")]
fn wait_for_recorded_pid(path: &std::path::Path) -> Option<libc::pid_t> {
    for _ in 0..1000 {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(pid) = text.trim().parse::<libc::pid_t>()
            && pid > 0
        {
            return Some(pid);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macos_orphan_probe is macOS-only");
    std::process::exit(4);
}
