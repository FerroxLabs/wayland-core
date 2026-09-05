//! Reserve command identity/capacity before effects, then share their receipt.

use super::{AcpError, AcpServer, CommandReceipt, LedgerOutcome};
use std::future::Future;
use std::sync::Arc;
use tokio::sync::{Mutex, watch};

type Outcome = Option<Result<CommandReceipt, Arc<AcpError>>>;

#[derive(Debug, Default)]
pub(crate) struct CommandOperation {
    attempt: Mutex<Option<watch::Receiver<Outcome>>>,
}

impl CommandOperation {
    async fn start<F>(&self, future: F) -> watch::Receiver<Outcome>
    where
        F: Future<Output = Result<CommandReceipt, AcpError>> + Send + 'static,
    {
        let mut attempt = self.attempt.lock().await;
        if let Some(existing) = attempt.as_ref() {
            let retry = match &*existing.borrow() {
                Some(Err(_)) => true,
                None => existing.has_changed().is_err(),
                Some(Ok(_)) => false,
            };
            if !retry {
                return existing.clone();
            }
        }
        let (tx, rx) = watch::channel(None);
        *attempt = Some(rx.clone());
        tokio::spawn(async move {
            tx.send_replace(Some(future.await.map_err(Arc::new)));
        });
        rx
    }
}

fn replay_error(error: &AcpError) -> AcpError {
    match error {
        AcpError::Cleanup(s) => AcpError::Cleanup(s.clone()),
        AcpError::Transport(s) => AcpError::Transport(s.clone()),
        AcpError::Protocol(s) => AcpError::Protocol(s.clone()),
        AcpError::Auth(s) => AcpError::Auth(s.clone()),
        AcpError::Session(s) => AcpError::Session(s.clone()),
        AcpError::Agent(s) => AcpError::Agent(s.clone()),
        AcpError::Forbidden(s) => AcpError::Forbidden(s.clone()),
        AcpError::Io(e) => AcpError::Io(std::io::Error::new(e.kind(), e.to_string())),
        AcpError::Serde(e) => AcpError::Protocol(e.to_string()),
    }
}

impl AcpServer {
    pub(super) async fn keyed_command<F>(
        &self,
        key: &str,
        fingerprint: &String,
        future: F,
    ) -> Result<CommandReceipt, AcpError>
    where
        F: Future<Output = Result<CommandReceipt, AcpError>> + Send + 'static,
    {
        let operation = {
            let mut ledger = self.commands.write().await;
            match ledger.classify(key, fingerprint) {
                LedgerOutcome::Fresh => {
                    let operation = Arc::new(CommandOperation::default());
                    if !ledger.record(
                        key,
                        fingerprint,
                        &CommandReceipt::Pending(operation.clone()),
                    ) {
                        return Err(AcpError::Protocol(
                            "command identity reservation failed".into(),
                        ));
                    }
                    operation
                }
                LedgerOutcome::Replay(CommandReceipt::Pending(operation)) => operation,
                LedgerOutcome::Replay(receipt) => return Ok(receipt),
                LedgerOutcome::Conflict => {
                    return Err(AcpError::Protocol(
                        "idempotency key is bound to a different command".into(),
                    ));
                }
                LedgerOutcome::InvalidIdentity => {
                    return Err(AcpError::Protocol(
                        "idempotency key is empty or too long".into(),
                    ));
                }
                LedgerOutcome::Full => {
                    return Err(AcpError::Protocol(
                        "idempotency ledger is at capacity".into(),
                    ));
                }
            }
        };
        let mut outcome = operation.start(future).await;
        loop {
            if let Some(result) = outcome.borrow().clone() {
                return result.map_err(|error| replay_error(&error));
            }
            outcome.changed().await.map_err(|_| {
                AcpError::Cleanup("command owner did not publish completion".into())
            })?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::SessionCreateRequest;
    use crate::transport::http::HttpHandler;
    use tokio::sync::{Notify, Semaphore};

    #[tokio::test]
    async fn keyed_retry_joins_the_gap_after_retirement_before_receipt() {
        let server = AcpServer::new();
        let id = server
            .create_session(SessionCreateRequest {
                model: None,
                tools: Vec::new(),
                system_prompt: None,
                agent: None,
                mcp_servers: Vec::new(),
            })
            .await
            .expect("create")
            .session_id;
        let fingerprint = super::super::fingerprint_of("session/delete", &id).expect("fingerprint");
        let retired = Arc::new(Notify::new());
        let release = Arc::new(Semaphore::new(0));
        let owner = server.clone();
        let effect_owner = server.clone();
        let target = id.clone();
        let signal = retired.clone();
        let permit = release.clone();
        let first = tokio::spawn(async move {
            owner
                .keyed_command("gap", &fingerprint, async move {
                    effect_owner.delete_session(target).await?;
                    signal.notify_one();
                    permit.acquire().await.expect("fixture").forget();
                    Ok(CommandReceipt::SessionDeleted)
                })
                .await
        });
        retired.notified().await;
        assert!(
            server.get_session(id.clone()).await.is_err(),
            "real metadata is retired"
        );
        let retry = server.delete_session_idempotent(Some("gap"), id);
        tokio::pin!(retry);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut retry)
                .await
                .is_err(),
            "retry must join pending receipt, not return an early 404"
        );
        release.add_permits(1);
        first.await.expect("first task").expect("receipt");
        retry.await.expect("same-key receipt");
    }
}
