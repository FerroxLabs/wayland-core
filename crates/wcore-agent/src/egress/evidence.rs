//! Fail-closed evaluator capture for the process-wide HTTP egress chokepoint.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use wcore_egress::{EgressEvent, EgressObserver};

const EVIDENCE_ENV: &str = "WCORE_EVAL_EGRESS_EVIDENCE";
const FORMAT_VERSION: u32 = 1;

static EVAL_OBSERVER: OnceLock<Arc<EvalEgressObserver>> = OnceLock::new();

#[derive(Debug)]
struct EvalEgressObserver {
    state: Mutex<ObserverState>,
}

#[derive(Debug)]
struct ObserverState {
    file: File,
    event_count: u64,
    write_error: Option<String>,
    finalized: bool,
}

#[derive(Serialize)]
#[serde(tag = "record", rename_all = "snake_case")]
enum Record<'a> {
    Header { version: u32 },
    Event { event: &'a EgressEvent },
    Footer { complete: bool, event_count: u64 },
}

impl EvalEgressObserver {
    fn create(path: &Path) -> io::Result<Self> {
        validate_path(path)?;
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let observer = Self {
            state: Mutex::new(ObserverState {
                file,
                event_count: 0,
                write_error: None,
                finalized: false,
            }),
        };
        {
            let mut state = observer
                .state
                .lock()
                .map_err(|_| io::Error::other("egress evidence lock poisoned"))?;
            write_record(
                &mut state.file,
                &Record::Header {
                    version: FORMAT_VERSION,
                },
            )?;
        }
        Ok(observer)
    }

    fn finalize(&self) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("egress evidence lock poisoned"))?;
        if state.finalized {
            return state
                .write_error
                .as_ref()
                .map_or(Ok(()), |error| Err(io::Error::other(error.clone())));
        }
        state.finalized = true;
        let complete = state.write_error.is_none();
        let event_count = state.event_count;
        if let Err(error) = write_record(
            &mut state.file,
            &Record::Footer {
                complete,
                event_count,
            },
        ) {
            state.write_error.get_or_insert_with(|| error.to_string());
        }
        if let Err(error) = state.file.sync_all() {
            state.write_error.get_or_insert_with(|| error.to_string());
        }
        state
            .write_error
            .as_ref()
            .map_or(Ok(()), |error| Err(io::Error::other(error.clone())))
    }
}

impl EgressObserver for EvalEgressObserver {
    fn observe(&self, event: EgressEvent) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.finalized || state.write_error.is_some() {
            return;
        }
        if let Err(error) = write_record(&mut state.file, &Record::Event { event: &event }) {
            state.write_error = Some(error.to_string());
            return;
        }
        state.event_count = state.event_count.saturating_add(1);
    }
}

/// Install the evaluator's recorder when its explicit evidence path is present.
pub fn install_eval_egress_observer() -> io::Result<()> {
    let Some(path) = std::env::var_os(EVIDENCE_ENV).filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    let observer = Arc::new(EvalEgressObserver::create(&PathBuf::from(path))?);
    wcore_egress::install_global_observer(observer.clone()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a process-wide egress observer was already installed",
        )
    })?;
    EVAL_OBSERVER.set(observer).map_err(|_| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the evaluator egress observer was already installed",
        )
    })
}

/// Write the completeness footer and durably flush evaluator evidence.
pub fn finalize_eval_egress_observer() -> io::Result<()> {
    EVAL_OBSERVER
        .get()
        .map_or(Ok(()), |observer| observer.finalize())
}

fn validate_path(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "egress evidence path must be absolute",
        ));
    }
    let cwd = std::env::current_dir()?.canonicalize()?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("egress evidence path has no parent"))?
        .canonicalize()?;
    if !parent.starts_with(cwd) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "egress evidence path must stay inside the evaluator workspace",
        ));
    }
    Ok(())
}

fn write_record(file: &mut File, record: &Record<'_>) -> io::Result<()> {
    serde_json::to_writer(&mut *file, record).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.flush()
}
