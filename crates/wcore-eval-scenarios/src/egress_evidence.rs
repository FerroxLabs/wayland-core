//! Strict parser for packaged-child managed-HTTP egress evidence.

use std::io;
use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};

const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ManagedHttpEgressEvidence {
    pub attempted: Vec<String>,
    pub allowed: Vec<String>,
    pub denied: Vec<String>,
}

#[derive(Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
enum Record {
    Header { version: u32 },
    Event { event: Event },
    Footer { complete: bool, event_count: u64 },
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

pub(crate) fn read(path: &Path) -> io::Result<ManagedHttpEgressEvidence> {
    let contents = std::fs::read_to_string(path)?;
    let mut lines = contents.lines();
    match parse_record(lines.next())? {
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
    for line in lines {
        match serde_json::from_str::<Record>(line).map_err(io::Error::other)? {
            Record::Event { event } if footer.is_none() => {
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
            } if footer.is_none() => footer = Some((complete, event_count)),
            _ => return Err(io::Error::other("invalid egress evidence record order")),
        }
    }
    let Some((complete, declared_events)) = footer else {
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
    if evidence.attempted.len() != evidence.allowed.len() + evidence.denied.len() {
        return Err(io::Error::other("egress evidence partition is incomplete"));
    }
    evidence.attempted.sort();
    evidence.allowed.sort();
    evidence.denied.sort();
    Ok(evidence)
}

fn parse_record(line: Option<&str>) -> io::Result<Record> {
    serde_json::from_str(line.ok_or_else(|| io::Error::other("egress evidence is empty"))?)
        .map_err(io::Error::other)
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

    #[test]
    fn accepts_complete_partition() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("egress.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"record\":\"header\",\"version\":1}\n",
                "{\"record\":\"event\",\"event\":{\"attempt_id\":1,\"method\":\"POST\",\"destination\":{\"scheme\":\"http\",\"host\":\"127.0.0.1\",\"effective_port\":1234,\"path_query_sha256\":\"abc\"},\"outcome\":{\"kind\":\"http_response\",\"status\":200}}}\n",
                "{\"record\":\"footer\",\"complete\":true,\"event_count\":1}\n"
            ),
        )
        .unwrap();
        let evidence = read(&path).unwrap();
        assert_eq!(evidence.attempted.len(), 1);
        assert_eq!(evidence.allowed, evidence.attempted);
        assert!(evidence.denied.is_empty());
    }

    #[test]
    fn rejects_missing_footer_and_unresolved_attempt() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing.jsonl");
        std::fs::write(&missing, "{\"record\":\"header\",\"version\":1}\n").unwrap();
        assert!(read(&missing).unwrap_err().to_string().contains("footer"));

        let unresolved = root.path().join("unresolved.jsonl");
        std::fs::write(
            &unresolved,
            concat!(
                "{\"record\":\"header\",\"version\":1}\n",
                "{\"record\":\"event\",\"event\":{\"attempt_id\":1,\"method\":\"GET\",\"destination\":{\"scheme\":\"https\",\"host\":\"example.com\",\"effective_port\":443,\"path_query_sha256\":\"abc\"},\"outcome\":{\"kind\":\"abandoned_before_decision\"}}}\n",
                "{\"record\":\"footer\",\"complete\":true,\"event_count\":1}\n"
            ),
        )
        .unwrap();
        assert!(
            read(&unresolved)
                .unwrap_err()
                .to_string()
                .contains("before a policy decision")
        );
    }
}
