//! Wasmtime engine + epoch ticker for the WASM plugin host.
//!
//! Lifts Ironclaw's three-knob runtime shape:
//! - Component model on, threads off, fuel + epoch interruption on.
//! - A single background thread increments the engine epoch every 500 ms so
//!   Stores configured with `set_epoch_deadline` can be interrupted on a
//!   wall-clock budget.
//!
//! Unlike Ironclaw, our ticker is **drop-safe**: dropping the [`EpochTicker`]
//! handle stops the thread and joins it before returning.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use wasmtime::{Config, Engine};

use crate::error::{Result, WasmPluginError};

/// Interval between epoch increments. Matches Ironclaw (500 ms).
pub const EPOCH_TICK_INTERVAL: Duration = Duration::from_millis(500);

/// Build a wasmtime [`Engine`] preconfigured for the v0.6.5 plugin host.
///
/// Settings:
/// - `wasm_component_model(true)` — required for Tool + Hook worlds.
/// - wasm threads remain disabled (the `threads` feature is intentionally not
///   enabled on this crate's wasmtime dep; plugins are single-threaded).
/// - `consume_fuel(true)` — required by `WasmPluginLimits::fuel`.
/// - `epoch_interruption(true)` — required by the epoch ticker / timeout.
/// - `debug_info(false)` — never ship debug info in plugin sandboxes.
pub fn build_engine() -> Result<Engine> {
    let mut cfg = Config::new();
    cfg.wasm_component_model(true);
    cfg.consume_fuel(true);
    cfg.epoch_interruption(true);
    cfg.debug_info(false);
    // `async: true` bindgen + `instantiate_async` require async support
    // on the engine config (Wave 6B.1).
    cfg.async_support(true);

    Engine::new(&cfg).map_err(WasmPluginError::LoadFailed)
}

/// RAII handle for the background epoch-ticker thread.
///
/// The thread is spawned via [`EpochTicker::start`] and stopped automatically
/// when this handle is dropped — the [`Drop`] impl flips an atomic stop flag
/// and joins the thread, so callers do not need to do anything manual.
pub struct EpochTicker {
    engine: Engine,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    interval: Duration,
}

impl EpochTicker {
    /// Spawn an epoch ticker that increments `engine`'s epoch every
    /// [`EPOCH_TICK_INTERVAL`]. Returns the RAII handle.
    pub fn start(engine: Engine) -> Result<Self> {
        Self::start_with_interval(engine, EPOCH_TICK_INTERVAL)
    }

    /// Variant that lets tests pick a faster tick.
    pub fn start_with_interval(engine: Engine, interval: Duration) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_engine = engine.clone();
        let thread_stop = stop.clone();
        let thread_interval = interval;
        let handle = std::thread::Builder::new()
            .name("wcore-plugin-wasm-epoch-ticker".into())
            .spawn(move || {
                // Sleep in small chunks so shutdown is responsive even when the
                // configured interval is long (e.g. 500 ms in production).
                let chunk = Duration::from_millis(50).min(thread_interval);
                while !thread_stop.load(Ordering::Relaxed) {
                    let mut waited = Duration::ZERO;
                    while waited < thread_interval {
                        if thread_stop.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(chunk);
                        waited += chunk;
                    }
                    thread_engine.increment_epoch();
                }
            })
            .map_err(|e| WasmPluginError::LoadFailed(anyhow::Error::new(e)))?;

        Ok(Self {
            engine,
            stop,
            handle: Some(handle),
            interval,
        })
    }

    /// Access the wasmtime engine the ticker is driving.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Configured tick interval.
    pub fn interval(&self) -> Duration {
        self.interval
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            // Best-effort join — a panicked ticker thread should not propagate
            // through Drop. We log and move on.
            if let Err(e) = handle.join() {
                tracing::warn!(?e, "WASM epoch-ticker thread panicked on shutdown");
            }
        }
    }
}

impl std::fmt::Debug for EpochTicker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EpochTicker")
            .field("interval", &self.interval)
            .field("stopped", &self.stop.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_engine_succeeds() {
        let engine = build_engine().expect("engine must build with default plugin config");
        // Sanity: the engine handle is usable.
        let _ = engine.precompile_compatibility_hash();
    }

    #[test]
    fn epoch_ticker_runs() {
        let engine = build_engine().expect("engine");
        // Use a short interval so the test completes quickly.
        let ticker = EpochTicker::start_with_interval(engine.clone(), Duration::from_millis(20))
            .expect("ticker must start");
        // Wait long enough for at least a few ticks.
        std::thread::sleep(Duration::from_millis(150));
        drop(ticker);
        // Drop must not hang — if we get here, the join completed.
    }

    #[test]
    fn epoch_ticker_drop_joins_thread() {
        let engine = build_engine().expect("engine");
        let ticker = EpochTicker::start_with_interval(engine, Duration::from_millis(20))
            .expect("ticker must start");
        let stop = ticker.stop.clone();
        drop(ticker);
        assert!(
            stop.load(Ordering::Relaxed),
            "stop flag must be set after drop"
        );
    }

    #[test]
    fn epoch_ticker_default_interval_matches_ironclaw() {
        let engine = build_engine().expect("engine");
        let ticker = EpochTicker::start(engine).expect("ticker");
        assert_eq!(ticker.interval(), Duration::from_millis(500));
    }
}
