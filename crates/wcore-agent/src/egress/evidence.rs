//! Fail-closed evaluator capture for the process-wide HTTP egress chokepoint.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signer as _, SigningKey};
use serde::Serialize;
use sha2::{Digest, Sha256};
use wcore_egress::{EgressEvent, EgressObserver};

const EVIDENCE_ENV: &str = "WCORE_EVAL_EGRESS_EVIDENCE";
const FORMAT_VERSION: u32 = 2;
const SIGNATURE_DOMAIN: &[u8] = b"wayland-eval-egress-evidence-v2\0";

static EVAL_OBSERVER: OnceLock<Arc<EvalEgressObserver>> = OnceLock::new();

#[derive(Debug)]
struct EvalEgressObserver {
    state: Mutex<ObserverState>,
}

#[derive(Debug)]
struct ObserverState {
    file: File,
    transcript: Sha256,
    signing_key: SigningKey,
    event_count: u64,
    write_error: Option<String>,
    finalized: bool,
}

#[derive(Serialize)]
#[serde(tag = "record", rename_all = "snake_case")]
enum Record<'a> {
    Header {
        version: u32,
    },
    Event {
        event: &'a EgressEvent,
    },
    Footer {
        complete: bool,
        event_count: u64,
        transcript_sha256: String,
        signature_base64: String,
    },
}

impl EvalEgressObserver {
    fn create(path: &Path, signing_key: SigningKey) -> io::Result<Self> {
        validate_path(path)?;
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let observer = Self {
            state: Mutex::new(ObserverState {
                file,
                transcript: Sha256::new(),
                signing_key,
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
            let ObserverState {
                file, transcript, ..
            } = &mut *state;
            write_transcript_record(
                file,
                transcript,
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
        let transcript_sha256 = format!("{:x}", state.transcript.clone().finalize());
        let signed = signature_message(complete, event_count, &transcript_sha256);
        let signature_base64 = BASE64.encode(state.signing_key.sign(&signed).to_bytes());
        if let Err(error) = write_record(
            &mut state.file,
            &Record::Footer {
                complete,
                event_count,
                transcript_sha256,
                signature_base64,
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
        let ObserverState {
            file, transcript, ..
        } = &mut *state;
        if let Err(error) =
            write_transcript_record(file, transcript, &Record::Event { event: &event })
        {
            state.write_error = Some(error.to_string());
            return;
        }
        state.event_count = state.event_count.saturating_add(1);
    }
}

/// Install the evaluator's recorder when its explicit evidence path is present.
///
/// The signing key is delivered through a one-use file consumed by the CLI
/// before any model-controlled tool can start. The evaluator retains only the
/// verifying key, so replacing the workspace pathname cannot forge evidence.
pub fn install_eval_egress_observer(signing_key: Option<SigningKey>) -> io::Result<()> {
    let path = std::env::var_os(EVIDENCE_ENV).filter(|value| !value.is_empty());
    let Some(path) = path else {
        return signing_key.map_or(Ok(()), |_| {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "evaluator egress signing key was supplied without an evidence path",
            ))
        });
    };
    let signing_key = signing_key.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "evaluator egress evidence requires a one-use signing key",
        )
    })?;
    let observer = Arc::new(EvalEgressObserver::create(
        &PathBuf::from(path),
        signing_key,
    )?);
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

fn write_transcript_record(
    file: &mut File,
    transcript: &mut Sha256,
    record: &Record<'_>,
) -> io::Result<()> {
    let mut encoded = serde_json::to_vec(record).map_err(io::Error::other)?;
    encoded.push(b'\n');
    file.write_all(&encoded)?;
    file.flush()?;
    transcript.update(&encoded);
    Ok(())
}

fn signature_message(complete: bool, event_count: u64, transcript_sha256: &str) -> Vec<u8> {
    let mut message = Vec::with_capacity(SIGNATURE_DOMAIN.len() + 1 + 8 + transcript_sha256.len());
    message.extend_from_slice(SIGNATURE_DOMAIN);
    message.push(u8::from(complete));
    message.extend_from_slice(&event_count.to_be_bytes());
    message.extend_from_slice(transcript_sha256.as_bytes());
    message
}
