//! Return unused allocator pages after an owning engine pool becomes idle.

/// Bound glibc arena proliferation before application threads are started.
/// A small shared arena pool trades allocator contention for bounded retained
/// free pages on high-core-count hosts; idle trimming returns reusable pages.
pub fn configure_for_launch() -> std::io::Result<()> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: called at process entry before runtime/worker construction.
        // mallopt takes an allocator parameter and does not invalidate memory.
        if unsafe { libc::mallopt(libc::M_ARENA_MAX, 8) } == 0 {
            return Err(std::io::Error::other("glibc arena limit was rejected"));
        }
        // Keep large turn buffers independently releasable. Glibc otherwise
        // raises this threshold as large allocations are freed, moving later
        // buffers into shared arenas whose fragmented pages cannot be trimmed.
        // SAFETY: the same single-threaded initialization boundary as above.
        if unsafe { libc::mallopt(libc::M_MMAP_THRESHOLD, 128 * 1024) } == 0 {
            return Err(std::io::Error::other("glibc mmap threshold was rejected"));
        }
    }
    Ok(())
}

/// Coalesce concurrent idle notifications without delaying session cleanup.
/// Linux glibc retains freed per-thread arena pages after large concurrent
/// turns; trimming releases those pages without changing live allocations.
pub async fn release_idle_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        static LAST_TRIM: std::sync::Mutex<Option<std::time::Instant>> =
            std::sync::Mutex::new(None);
        let result = tokio::task::spawn_blocking(|| {
            let Ok(mut last) = LAST_TRIM.try_lock() else {
                return;
            };
            if last.is_some_and(|time| time.elapsed() < std::time::Duration::from_secs(1)) {
                return;
            }
            // SAFETY: glibc's thread-safe allocator operation accepts a padding
            // size and only returns unused pages; it does not invalidate data.
            unsafe { libc::malloc_trim(0) };
            *last = Some(std::time::Instant::now());
        })
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, "idle allocator page release did not complete");
        }
    }
}
