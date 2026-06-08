//! S4 verification — build a seccomp filter from `SyscallPolicy::Strict`,
//! export the BPF to a tempfile, and assert the bytes form a syntactically
//! valid BPF program (header + at least one instruction).
//!
//! Linux + `seccomp` feature only. macOS hosts skip this file entirely.

#![cfg(all(target_os = "linux", feature = "seccomp"))]

use std::io::{Read, Seek, SeekFrom};
use wcore_sandbox::backends::bwrap_seccomp;
use wcore_sandbox::manifest::SyscallPolicy;

#[test]
fn inherit_policy_produces_no_filter() {
    let out = bwrap_seccomp::export_filter_to_tempfile(SyscallPolicy::Inherit).expect("inherit ok");
    assert!(out.is_none(), "Inherit policy must not emit a BPF filter");
}

#[test]
fn strict_policy_round_trip() {
    let mut file = match bwrap_seccomp::export_filter_to_tempfile(SyscallPolicy::Strict) {
        Ok(Some(f)) => f,
        Ok(None) => panic!("Strict policy must emit a BPF filter"),
        Err(e) => {
            // libseccomp's C library may be absent in some Linux CI envs.
            // The unit test in src/backends/bwrap_seccomp.rs treats this
            // as a skip; mirror that here.
            eprintln!("skip: libseccomp unavailable: {e}");
            return;
        }
    };
    file.seek(SeekFrom::Start(0)).expect("rewind");
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).expect("read BPF bytes");
    // A BPF program is an array of `struct sock_filter` (8 bytes each).
    // A non-empty allowlist must produce many instructions; require at
    // least 16 bytes (= 2 instructions, the absolute minimum for any
    // useful filter). The exact count varies with libseccomp version, so
    // we test a coarse lower bound only.
    assert!(
        buf.len() >= 16 && buf.len() % 8 == 0,
        "BPF program shape invalid: {} bytes",
        buf.len()
    );
    // Allowlist size sanity check.
    assert!(
        !bwrap_seccomp::DEFAULT_SYSCALL_ALLOWLIST.is_empty(),
        "DEFAULT_SYSCALL_ALLOWLIST must be non-empty"
    );
}
