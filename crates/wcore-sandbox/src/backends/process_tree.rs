//! Centralized process-tree ownership for direct (Dangerous) execution.

/// Put a direct child in its own process group where the platform supports it.
pub(crate) fn isolate(_command: &mut tokio::process::Command) {
    #[cfg(unix)]
    _command.process_group(0);
}

/// Armed while a direct child future is alive. On Unix, dropping the future
/// kills the whole dedicated process group, including background descendants.
pub(crate) struct ProcessTreeGuard {
    #[cfg(unix)]
    process_group: Option<libc::pid_t>,
}

impl ProcessTreeGuard {
    pub(crate) fn new(_pid: Option<u32>) -> Self {
        Self {
            #[cfg(unix)]
            process_group: _pid.and_then(|pid| libc::pid_t::try_from(pid).ok()),
        }
    }

    pub(crate) fn disarm(&mut self) {
        #[cfg(unix)]
        {
            self.process_group = None;
        }
    }
}

impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(process_group) = self.process_group.take() {
            // SAFETY: `isolate` created a dedicated group whose ID is the child
            // PID. A negative PID targets only that group. SIGKILL is required
            // here because this is the cancellation/expiry containment path.
            unsafe {
                libc::kill(-process_group, libc::SIGKILL);
            }
        }
    }
}
