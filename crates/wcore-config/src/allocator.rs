//! Return unused allocator pages after an owning engine pool becomes idle.

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
