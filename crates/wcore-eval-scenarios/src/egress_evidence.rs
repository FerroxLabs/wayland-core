//! Strict parser for packaged-child managed-HTTP egress evidence.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, SigningKey, Verifier as _, VerifyingKey};
use rand::rngs::OsRng;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const FORMAT_VERSION: u32 = 2;
const SIGNATURE_DOMAIN: &[u8] = b"wayland-eval-egress-evidence-v2\0";
static CAPTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) struct Capture {
    path: PathBuf,
    key_path: PathBuf,
    verifying_key: VerifyingKey,
}

impl Capture {
    pub(crate) fn create(cwd: &Path) -> io::Result<Self> {
        let directory = cwd.join(".wayland-core");
        std::fs::create_dir_all(&directory)?;
        let sequence = CAPTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let stem = format!("eval-egress-{}-{sequence}", std::process::id());
        let path = directory.join(format!("{stem}.jsonl"));
        let key_path = directory.join(format!("{stem}.key"));
        let signing_key = SigningKey::generate(&mut OsRng);
        if let Err(error) = write_secret(&key_path, &signing_key.to_bytes()) {
            let _ = std::fs::remove_file(&key_path);
            return Err(error);
        }
        Ok(Self {
            path,
            key_path,
            verifying_key: signing_key.verifying_key(),
        })
    }

    pub(crate) fn configure(&self, command: &mut tokio::process::Command) {
        command.env("WCORE_EVAL_EGRESS_EVIDENCE", &self.path);
        command.arg("--eval-egress-key-file").arg(&self.key_path);
    }

    pub(crate) fn read(&self) -> io::Result<ManagedHttpEgressEvidence> {
        let result = read_authenticated(&self.path, &self.verifying_key);
        let remove_result = std::fs::remove_file(&self.path);
        match (result, remove_result) {
            (Ok(evidence), Ok(())) => Ok(evidence),
            (Ok(_), Err(error)) => Err(io::Error::other(format!(
                "could not remove authenticated egress evidence: {error}"
            ))),
            (Err(error), _) => Err(error),
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.key_path);
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ManagedHttpEgressEvidence {
    pub attempted: Vec<String>,
    pub allowed: Vec<String>,
    pub denied: Vec<String>,
}

#[derive(Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
enum Record {
    Header {
        version: u32,
    },
    Event {
        event: Event,
    },
    Footer {
        complete: bool,
        event_count: u64,
        transcript_sha256: String,
        signature_base64: String,
    },
}

#[derive(Deserialize)]
struct Event {
    method: String,
    destination: Destination,
    outcome: Outcome,
}

#[derive(Deserialize)]
struct Destination {
    scheme: String,
    host: String,
    effective_port: Option<u16>,
    path_query_sha256: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Outcome {
    Denied,
    HttpResponse { status: u16 },
    TransportError { class: serde_json::Value },
    AbandonedBeforeDecision,
    AbandonedAfterAllow,
}

fn read_authenticated(
    path: &Path,
    verifying_key: &VerifyingKey,
) -> io::Result<ManagedHttpEgressEvidence> {
    let contents = std::fs::read(path)?;
    if !contents.ends_with(b"\n") {
        return Err(io::Error::other(
            "egress evidence is not newline terminated",
        ));
    }
    let mut lines = contents.split_inclusive(|byte| *byte == b'\n');
    let header = lines
        .next()
        .ok_or_else(|| io::Error::other("egress evidence is empty"))?;
    match parse_record(strip_newline(header))? {
        Record::Header { version } if version == FORMAT_VERSION => {}
        Record::Header { version } => {
            return Err(io::Error::other(format!(
                "unsupported egress evidence version {version}"
            )));
        }
        _ => return Err(io::Error::other("egress evidence header missing")),
    }

    let mut evidence = ManagedHttpEgressEvidence::default();
    let mut observed_events = 0_u64;
    let mut footer = None;
    let mut transcript = Sha256::new();
    transcript.update(header);
    for line in lines {
        let record =
            serde_json::from_slice::<Record>(strip_newline(line)).map_err(io::Error::other)?;
        match record {
            Record::Event { event } if footer.is_none() => {
                transcript.update(line);
                observed_events = observed_events.saturating_add(1);
                let fingerprint = fingerprint(&event);
                evidence.attempted.push(fingerprint.clone());
                match event.outcome {
                    Outcome::Denied => evidence.denied.push(fingerprint),
                    Outcome::HttpResponse { status } => {
                        let _ = status;
                        evidence.allowed.push(fingerprint);
                    }
                    Outcome::TransportError { class } => {
                        let _ = class;
                        evidence.allowed.push(fingerprint);
                    }
                    Outcome::AbandonedAfterAllow => evidence.allowed.push(fingerprint),
                    Outcome::AbandonedBeforeDecision => {
                        return Err(io::Error::other(
                            "egress attempt was abandoned before a policy decision",
                        ));
                    }
                }
            }
            Record::Footer {
                complete,
                event_count,
                transcript_sha256,
                signature_base64,
            } if footer.is_none() => {
                footer = Some((complete, event_count, transcript_sha256, signature_base64))
            }
            _ => return Err(io::Error::other("invalid egress evidence record order")),
        }
    }
    let Some((complete, declared_events, declared_digest, signature_base64)) = footer else {
        return Err(io::Error::other("egress evidence footer missing"));
    };
    if !complete {
        return Err(io::Error::other(
            "egress evidence writer reported incomplete capture",
        ));
    }
    if declared_events != observed_events {
        return Err(io::Error::other(format!(
            "egress event count mismatch: declared {declared_events}, observed {observed_events}"
        )));
    }
    let observed_digest = format!("{:x}", transcript.finalize());
    if declared_digest != observed_digest {
        return Err(io::Error::other(
            "egress evidence transcript digest mismatch",
        ));
    }
    let signature_bytes = BASE64
        .decode(signature_base64)
        .map_err(|_| io::Error::other("egress evidence signature is not valid base64"))?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| io::Error::other("egress evidence signature has invalid length"))?;
    let signed = signature_message(complete, declared_events, &observed_digest);
    verifying_key
        .verify(&signed, &signature)
        .map_err(|_| io::Error::other("egress evidence signature verification failed"))?;
    if evidence.attempted.len() != evidence.allowed.len() + evidence.denied.len() {
        return Err(io::Error::other("egress evidence partition is incomplete"));
    }
    evidence.attempted.sort();
    evidence.allowed.sort();
    evidence.denied.sort();
    Ok(evidence)
}

fn parse_record(line: &[u8]) -> io::Result<Record> {
    serde_json::from_slice(line).map_err(io::Error::other)
}

fn strip_newline(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\n").unwrap_or(line)
}

fn write_secret(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn signature_message(complete: bool, event_count: u64, transcript_sha256: &str) -> Vec<u8> {
    let mut message = Vec::with_capacity(SIGNATURE_DOMAIN.len() + 1 + 8 + transcript_sha256.len());
    message.extend_from_slice(SIGNATURE_DOMAIN);
    message.push(u8::from(complete));
    message.extend_from_slice(&event_count.to_be_bytes());
    message.extend_from_slice(transcript_sha256.as_bytes());
    message
}

fn fingerprint(event: &Event) -> String {
    let port = if is_loopback(&event.destination.host) {
        0
    } else {
        event.destination.effective_port.unwrap_or_default()
    };
    let port = port.to_string();
    let mut hash = Sha256::new();
    for value in [
        event.method.as_str(),
        event.destination.scheme.as_str(),
        event.destination.host.as_str(),
        port.as_str(),
        event.destination.path_query_sha256.as_str(),
    ] {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value.as_bytes());
    }
    format!("managed_http:v1:{:x}", hash.finalize())
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer as _;

    fn write_authenticated(path: &Path, key: &SigningKey, events: &[&str], complete: bool) {
        let mut transcript = b"{\"record\":\"header\",\"version\":2}\n".to_vec();
        for event in events {
            transcript.extend_from_slice(event.as_bytes());
            transcript.push(b'\n');
        }
        let digest = format!("{:x}", Sha256::digest(&transcript));
        let signed = signature_message(complete, events.len() as u64, &digest);
        let footer = serde_json::json!({
            "record": "footer",
            "complete": complete,
            "event_count": events.len(),
            "transcript_sha256": digest,
            "signature_base64": BASE64.encode(key.sign(&signed).to_bytes()),
        });
        transcript.extend_from_slice(serde_json::to_string(&footer).unwrap().as_bytes());
        transcript.push(b'\n');
        std::fs::write(path, transcript).unwrap();
    }

    #[test]
    fn accepts_complete_partition() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("egress.jsonl");
        let key = SigningKey::from_bytes(&[7; 32]);
        write_authenticated(
            &path,
            &key,
            &[
                "{\"record\":\"event\",\"event\":{\"attempt_id\":1,\"method\":\"POST\",\"destination\":{\"scheme\":\"http\",\"host\":\"127.0.0.1\",\"effective_port\":1234,\"path_query_sha256\":\"abc\"},\"outcome\":{\"kind\":\"http_response\",\"status\":200}}}",
            ],
            true,
        );
        let evidence = read_authenticated(&path, &key.verifying_key()).unwrap();
        assert_eq!(evidence.attempted.len(), 1);
        assert_eq!(evidence.allowed, evidence.attempted);
        assert!(evidence.denied.is_empty());
    }

    #[test]
    fn rejects_missing_footer_and_unresolved_attempt() {
        let root = tempfile::tempdir().unwrap();
        let key = SigningKey::from_bytes(&[8; 32]);
        let missing = root.path().join("missing.jsonl");
        std::fs::write(&missing, "{\"record\":\"header\",\"version\":2}\n").unwrap();
        assert!(
            read_authenticated(&missing, &key.verifying_key())
                .unwrap_err()
                .to_string()
                .contains("footer")
        );

        let unresolved = root.path().join("unresolved.jsonl");
        write_authenticated(
            &unresolved,
            &key,
            &[
                "{\"record\":\"event\",\"event\":{\"attempt_id\":1,\"method\":\"GET\",\"destination\":{\"scheme\":\"https\",\"host\":\"example.com\",\"effective_port\":443,\"path_query_sha256\":\"abc\"},\"outcome\":{\"kind\":\"abandoned_before_decision\"}}}",
            ],
            true,
        );
        assert!(
            read_authenticated(&unresolved, &key.verifying_key())
                .unwrap_err()
                .to_string()
                .contains("before a policy decision")
        );
    }

    #[test]
    fn rejects_candidate_replacement_signed_by_an_untrusted_key() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("egress.jsonl");
        let evaluator_key = SigningKey::from_bytes(&[9; 32]);
        let attacker_key = SigningKey::from_bytes(&[10; 32]);
        write_authenticated(&path, &attacker_key, &[], true);

        let error = read_authenticated(&path, &evaluator_key.verifying_key()).unwrap_err();
        assert!(error.to_string().contains("signature verification failed"));
    }

    #[test]
    fn rejects_footer_completeness_mutation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("egress.jsonl");
        let key = SigningKey::from_bytes(&[11; 32]);
        write_authenticated(&path, &key, &[], false);
        let contents = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            contents.replace("\"complete\":false", "\"complete\":true"),
        )
        .unwrap();

        let error = read_authenticated(&path, &key.verifying_key()).unwrap_err();
        assert!(error.to_string().contains("signature verification failed"));
    }
}
