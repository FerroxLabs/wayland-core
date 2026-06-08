//! Resource limits for WASM plugins.
//!
//! Lifts the three-knob pattern from Ironclaw (`ironclaw_wasm::limiter`):
//! cumulative memory cap across multi-memory components, small instance/table/
//! memory counts, and a fixed table-growth ceiling.

use std::time::Duration;

use wasmtime::ResourceLimiter;

/// Default cap on cumulative linear memory growth across all memories in a
/// component instance. 10 MiB matches Ironclaw's default.
pub const DEFAULT_MEMORY_BYTES: u64 = 10 * 1024 * 1024;

/// Default wasmtime fuel budget per execution. 500M matches Ironclaw.
pub const DEFAULT_FUEL: u64 = 500_000_000;

/// Default wall-clock deadline per execution (60s, matches Ironclaw).
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Per-plugin resource limits — the canonical three knobs.
#[derive(Debug, Clone, Copy)]
pub struct WasmPluginLimits {
    /// Cumulative memory bytes across all memories in the instance.
    pub memory_bytes: u64,
    /// Wasmtime fuel budget for one execution.
    pub fuel: u64,
    /// Wall-clock deadline (epoch-ticker driven) for one execution.
    pub timeout_secs: u64,
}

impl Default for WasmPluginLimits {
    fn default() -> Self {
        Self {
            memory_bytes: DEFAULT_MEMORY_BYTES,
            fuel: DEFAULT_FUEL,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }
}

impl WasmPluginLimits {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }

    pub fn with_memory_bytes(mut self, memory_bytes: u64) -> Self {
        self.memory_bytes = memory_bytes;
        self
    }

    pub fn with_fuel(mut self, fuel: u64) -> Self {
        self.fuel = fuel;
        self
    }

    pub fn with_timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }
}

/// `wasmtime::ResourceLimiter` impl that enforces [`WasmPluginLimits`].
///
/// One instance is attached to each `Store` via `Store::limiter`.
#[derive(Debug)]
pub struct WasmResourceLimiter {
    memory_limit: u64,
    memory_used: u64,
    pending_memory_growth: u64,
    max_tables: u32,
    max_instances: u32,
    max_memories: u32,
}

impl WasmResourceLimiter {
    /// New limiter with `memory_limit` bytes of cumulative memory budget.
    /// Instance/table/memory counts match Ironclaw defaults (10/10/10).
    pub fn new(memory_limit: u64) -> Self {
        Self {
            memory_limit,
            memory_used: 0,
            pending_memory_growth: 0,
            max_tables: 10,
            max_instances: 10,
            max_memories: 10,
        }
    }

    /// Convenience: build a limiter from a [`WasmPluginLimits`].
    pub fn from_limits(limits: &WasmPluginLimits) -> Self {
        Self::new(limits.memory_bytes)
    }

    /// Memory bytes currently accounted as in-use (test/observability hook).
    pub fn memory_used(&self) -> u64 {
        self.memory_used
    }
}

impl ResourceLimiter for WasmResourceLimiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, wasmtime::Error> {
        self.pending_memory_growth = 0;

        let current = current as u64;
        let desired = desired as u64;
        let growth = desired.saturating_sub(current);
        let total_memory = self.memory_used.saturating_add(growth);
        if total_memory > self.memory_limit {
            tracing::warn!(
                current,
                desired,
                growth,
                used = self.memory_used,
                total = total_memory,
                limit = self.memory_limit,
                "WASM memory growth denied"
            );
            return Ok(false);
        }

        self.memory_used = total_memory;
        self.pending_memory_growth = growth;
        Ok(true)
    }

    fn memory_grow_failed(&mut self, error: wasmtime::Error) -> Result<(), wasmtime::Error> {
        self.memory_used = self.memory_used.saturating_sub(self.pending_memory_growth);
        self.pending_memory_growth = 0;
        tracing::debug!(error = ?error, "WASM memory growth failed after approval");
        Ok(())
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool, wasmtime::Error> {
        if desired > 10_000 {
            tracing::warn!(current, desired, "WASM table growth denied");
            return Ok(false);
        }
        Ok(true)
    }

    fn instances(&self) -> usize {
        self.max_instances as usize
    }

    fn tables(&self) -> usize {
        self.max_tables as usize
    }

    fn memories(&self) -> usize {
        self.max_memories as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limits_match_ironclaw_constants() {
        let limits = WasmPluginLimits::default();
        assert_eq!(limits.memory_bytes, 10 * 1024 * 1024, "10 MiB default");
        assert_eq!(limits.fuel, 500_000_000, "500M fuel default");
        assert_eq!(limits.timeout_secs, 60, "60s timeout default");
        assert_eq!(limits.timeout(), Duration::from_secs(60));
    }

    #[test]
    fn limiter_caps_memory() {
        // 11 MiB worth of cumulative growth must be rejected when the cap is 10 MiB.
        let mut limiter = WasmResourceLimiter::new(10 * 1024 * 1024);
        // First 8 MiB: ok.
        assert!(limiter.memory_growing(0, 8 * 1024 * 1024, None).unwrap());
        // Another 2 MiB: still ok (10 MiB total, exactly at cap).
        assert!(limiter.memory_growing(0, 2 * 1024 * 1024, None).unwrap());
        // One more byte over the cap: rejected.
        assert!(
            !limiter.memory_growing(0, 1024 * 1024, None).unwrap(),
            "growth past 10 MiB must be denied"
        );
    }

    #[test]
    fn limiter_caps_instance_count() {
        let limiter = WasmResourceLimiter::new(1024);
        assert_eq!(limiter.instances(), 10, "instance cap matches Ironclaw");
        assert_eq!(limiter.tables(), 10, "table cap matches Ironclaw");
        assert_eq!(limiter.memories(), 10, "memory cap matches Ironclaw");
    }

    #[test]
    fn limiter_caps_table_growth() {
        let mut limiter = WasmResourceLimiter::new(1024);
        assert!(limiter.table_growing(0, 10_000, None).unwrap());
        assert!(!limiter.table_growing(0, 10_001, None).unwrap());
    }

    #[test]
    fn from_limits_uses_memory_bytes() {
        let limits = WasmPluginLimits::default().with_memory_bytes(4096);
        let limiter = WasmResourceLimiter::from_limits(&limits);
        assert_eq!(limiter.memory_used(), 0);
        // Indirectly confirm via cap behaviour.
        let mut limiter = limiter;
        assert!(!limiter.memory_growing(0, 8192, None).unwrap());
    }
}
