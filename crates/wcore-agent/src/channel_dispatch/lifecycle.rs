//! Channel session ownership and policy-reload cleanup.

use super::*;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use wcore_channels::{InboundPolicy, config::ChannelConfig};

#[derive(Default)]
pub(super) struct SessionState {
    pub engine: Option<AgentEngine>,
    pub children: Option<crate::spawner::HostChildController>,
    pub attempted: bool,
    pub ready: bool,
}

pub(super) struct ChannelSession {
    pub channel: String,
    pub policy: InboundPolicy,
    pub scope: ChannelToolScope,
    pub cancel: CancellationToken,
    pub cleanup: Arc<crate::bootstrap_cleanup::BootstrapCleanup>,
    pub state: Mutex<SessionState>,
}

impl ChannelSession {
    fn matches(&self, policy: &InboundPolicy, scope: &ChannelToolScope) -> bool {
        self.policy == *policy
            && self.scope.posture == scope.posture
            && self.scope.workspace_root == scope.workspace_root
            && self.scope.ambient_mcp_full_authority_v1 == scope.ambient_mcp_full_authority_v1
    }

    async fn close(&self) -> anyhow::Result<()> {
        self.cancel.cancel();
        // The same mutex owns initialization, execution and retirement.
        // Acquiring it proves the previous caller has dropped its run future.
        let mut state = self.state.lock().await;
        if let Some(engine) = &state.engine
            && engine.session_journal().is_some()
        {
            engine.recovery_plan()?;
            if let Some(children) = &state.children {
                let supervisor = children.supervisor()?;
                for child in supervisor
                    .list()?
                    .iter()
                    .filter(|c| !c.status.is_terminal())
                {
                    supervisor.request_cancel(&child.child_id)?;
                }
                while !supervisor.cleanup_ready()? {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
        if let Some(engine) = state.engine.as_mut() {
            engine.prepare_shutdown();
        }
        state.engine.take();
        state.children.take();
        state.ready = false;
        if state.attempted {
            self.cleanup.close().await?;
        }
        Ok(())
    }
}

impl ChannelTurnDispatcher {
    /// Validate and install policy, then retire affected local sessions before
    /// acknowledging reload. Incomplete entries remain quarantined for retry.
    pub async fn reload_from_configs(&self, configs: Vec<ChannelConfig>) -> anyhow::Result<usize> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        tokio::time::timeout_at(deadline, async {
            let _reload = self.reload.lock().await;
            let (count, affected) = {
                let _admission = self.admission.write().await;
                // Validation fails before the registry changes.
                let count = self.policies.replace_from_configs(configs, PathBuf::from(&self.cwd).as_path())?;
                let snapshot = self.policies.snapshot();
                let pool = self.engines.lock().await;
                let affected: Vec<_> = pool.iter().filter_map(|(id, session)| {
                    let unchanged = snapshot.policies.get(&session.channel)
                        .zip(snapshot.postures.get(&session.channel))
                        .is_some_and(|(policy, scope)| session.matches(policy, scope));
                    if unchanged && !session.cancel.is_cancelled() {
                        None
                    } else {
                        session.cancel.cancel();
                        Some((id.clone(), session.clone()))
                    }
                }).collect();
                (count, affected)
            };
            let results = futures::future::join_all(affected.into_iter().map(|(id, session)| async move {
                session.close().await?;
                let mut pool = self.engines.lock().await;
                if pool.get(&id).is_some_and(|current| Arc::ptr_eq(current, &session)) {
                    pool.remove(&id);
                }
                Ok::<(), anyhow::Error>(())
            })).await;
            let errors: Vec<_> = results.into_iter().filter_map(Result::err).map(|e| e.to_string()).collect();
            anyhow::ensure!(errors.is_empty(), "channel cleanup incomplete; affected sessions quarantined: {}", errors.join("; "));
            Ok(count)
        }).await.map_err(|_| anyhow::anyhow!("channel reload cleanup deadline exceeded; affected sessions quarantined; retry reload"))?
    }

    pub(super) async fn run_admitted(
        &self,
        session_key: &str,
        channel: &str,
        msg: &wcore_channels::IncomingMessage,
        admitted_policy: &InboundPolicy,
    ) -> anyhow::Result<Option<String>> {
        let id = Self::hashed_session_id(session_key);
        let session = {
            let _admission = self.admission.read().await;
            let snapshot = self.policies.snapshot();
            let policy = snapshot
                .policies
                .get(channel)
                .ok_or_else(|| anyhow::anyhow!("channel is no longer configured"))?;
            anyhow::ensure!(
                policy == admitted_policy,
                "channel admission changed while message was queued"
            );
            let scope = snapshot
                .postures
                .get(channel)
                .ok_or_else(|| anyhow::anyhow!("channel tool scope is missing"))?;
            let mut pool = self.engines.lock().await;
            let session = pool
                .entry(id.clone())
                .or_insert_with(|| {
                    Arc::new(ChannelSession {
                        channel: channel.to_owned(),
                        policy: policy.clone(),
                        scope: scope.clone(),
                        cancel: CancellationToken::new(),
                        cleanup: Arc::default(),
                        state: Mutex::default(),
                    })
                })
                .clone();
            anyhow::ensure!(
                session.channel == channel && session.matches(policy, scope),
                "channel session scope changed; reload cleanup required"
            );
            session
        };
        let mut state = session.state.lock().await;
        anyhow::ensure!(
            !session.cancel.is_cancelled(),
            "channel session is closing; retry after reload"
        );
        anyhow::ensure!(
            self.policies.policy_for(channel) == *admitted_policy,
            "channel admission changed while waiting for session"
        );
        if !state.ready
            && let Err(error) = self.initialize(&id, &session, &mut state).await
        {
            session.cancel.cancel();
            return Err(error);
        }
        anyhow::ensure!(
            !session.cancel.is_cancelled(),
            "channel initialization was revoked"
        );
        let prompt = tokio::select! {
            biased;
            () = session.cancel.cancelled() => anyhow::bail!("channel turn was revoked"),
            prompt = self.prompt_for(channel, msg) => prompt,
        };
        let engine = state
            .engine
            .as_mut()
            .expect("ready session owns its engine");
        engine.set_cancel_token(session.cancel.child_token());
        let result = {
            let run = engine.run(&prompt, &msg.id);
            tokio::pin!(run);
            tokio::select! {
                biased;
                () = session.cancel.cancelled() => {
                    // Allow normal durable cancellation before dropping an
                    // uncooperative run. Reload still owns resource cleanup.
                    let _ = tokio::time::timeout(Duration::from_secs(2), &mut run).await;
                    anyhow::bail!("channel turn was revoked");
                }
                result = &mut run => result?,
            }
        };
        if result.text.is_empty() {
            Ok(None)
        } else {
            Ok(Some(result.text))
        }
    }
}
