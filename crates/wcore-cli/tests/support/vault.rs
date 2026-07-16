//! Confidential-recovery storage for packaged CLI test children.

#[cfg(unix)]
use std::os::unix::io::RawFd;

const TEST_VAULT_PASSPHRASE: &str = "wcore-cli-hermetic-test-vault-passphrase";

/// Keeps the parent side of the inherited passphrase descriptor alive until
/// the child has spawned, then closes it.
pub struct VaultGuard {
    #[cfg(unix)]
    fd: Option<RawFd>,
}

#[cfg(unix)]
impl Drop for VaultGuard {
    fn drop(&mut self) {
        if let Some(fd) = self.fd {
            // SAFETY: this guard uniquely owns the parent descriptor.
            let _ = unsafe { libc::close(fd) };
        }
    }
}

/// Configure a standard-library child with an ephemeral encrypted vault.
#[allow(dead_code)] // Integration targets compile this shared support module independently.
pub fn configure_process(command: &mut std::process::Command) -> VaultGuard {
    command
        .env_remove("WAYLAND_VAULT_PASSPHRASE")
        .env_remove("WAYLAND_VAULT_PASSPHRASE_FD");
    #[cfg(unix)]
    {
        let fd = inheritable_pipe();
        command.env("WAYLAND_VAULT_PASSPHRASE_FD", fd.to_string());
        VaultGuard { fd: Some(fd) }
    }
    #[cfg(not(unix))]
    {
        // Windows has no Unix-style inherited file descriptor. This is a
        // test-only child environment; production still warns on this legacy
        // compatibility path.
        command.env("WAYLAND_VAULT_PASSPHRASE", TEST_VAULT_PASSPHRASE);
        VaultGuard {}
    }
}

/// Configure a PTY child with an ephemeral encrypted vault.
///
/// `portable-pty` closes arbitrary inherited descriptors while preparing the
/// child, so the preferred FD transport cannot reach this test-only process.
/// Use the supported legacy environment transport here; production launches
/// and standard-process tests continue to use the FD transport above.
#[allow(dead_code)] // Integration targets compile this shared support module independently.
pub fn configure_pty(command: &mut portable_pty::CommandBuilder) -> VaultGuard {
    command.env_remove("WAYLAND_VAULT_PASSPHRASE");
    command.env_remove("WAYLAND_VAULT_PASSPHRASE_FD");
    command.env("WAYLAND_VAULT_PASSPHRASE", TEST_VAULT_PASSPHRASE);
    VaultGuard {
        #[cfg(unix)]
        fd: None,
    }
}

#[cfg(unix)]
#[allow(dead_code)] // Used only by integration targets that spawn std::process children.
fn inheritable_pipe() -> RawFd {
    let mut pipe = [0; 2];
    // SAFETY: `pipe` points to two valid integers. Plain `pipe(2)` is
    // intentional: the read end must survive exec into packaged Core.
    assert_eq!(
        unsafe { libc::pipe(pipe.as_mut_ptr()) },
        0,
        "create vault pipe"
    );
    let mut written = 0;
    while written < TEST_VAULT_PASSPHRASE.len() {
        // SAFETY: the write descriptor belongs to this process and the source
        // slice remains valid for the duration of the call.
        let count = unsafe {
            libc::write(
                pipe[1],
                TEST_VAULT_PASSPHRASE.as_bytes()[written..].as_ptr().cast(),
                TEST_VAULT_PASSPHRASE.len() - written,
            )
        };
        assert!(count > 0, "write vault passphrase pipe");
        written += count as usize;
    }
    // SAFETY: the complete secret is buffered and the writer is no longer
    // needed; closing it gives Core an unambiguous EOF.
    assert_eq!(
        unsafe { libc::close(pipe[1]) },
        0,
        "close vault pipe writer"
    );
    pipe[0]
}
