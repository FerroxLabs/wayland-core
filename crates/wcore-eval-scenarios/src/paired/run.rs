use super::{Family, PairedTask, effect::EffectFixture};
use crate::{
    providers::ProviderConfig,
    runner::{Failure, ScenarioResult, run_with_binary_in_paths},
    scenario::{SessionIdentity, Turn},
};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

struct Environment {
    _root: tempfile::TempDir,
    home: PathBuf,
    workspace: PathBuf,
}

impl Environment {
    fn build(
        task: &PairedTask,
        provider: &ProviderConfig,
        cap: f64,
        mcp: Option<&str>,
    ) -> anyhow::Result<Self> {
        let seed = crate::tempenv::build_with(
            provider,
            &crate::tempenv::TempEnvOptions {
                budget_max_cost_usd: Some(cap),
                ..Default::default()
            },
        )?;
        let root = tempfile::tempdir()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o711))?;
        }
        let home = root.path().join("wayland-core");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&home)?;
        std::fs::create_dir_all(workspace.join(".wayland-core"))?;
        let mut config: toml::Value =
            toml::from_str(&std::fs::read_to_string(seed.home().join("config.toml"))?)?;
        let session = config
            .get_mut("session")
            .and_then(toml::Value::as_table_mut)
            .ok_or_else(|| anyhow::anyhow!("seed has no session config"))?;
        session.insert(
            "directory".into(),
            home.join("sessions").to_string_lossy().to_string().into(),
        );
        session.insert("enabled".into(), true.into());
        session.insert("require_durability".into(), true.into());
        let table = config
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("invalid seeded config"))?;
        if task.family == Family::RelevantMemoryRecall {
            let mut memory = toml::Table::new();
            memory.insert("enabled".into(), true.into());
            memory.insert("dream_cycle_throttle_secs".into(), 0_i64.into());
            table.insert("memory".into(), toml::Value::Table(memory));
        }
        if let Some(url) = mcp {
            let mut server = toml::Table::new();
            server.insert("transport".into(), "streamable-http".into());
            server.insert("url".into(), url.into());
            server.insert("deferred".into(), false.into());
            let mut servers = toml::Table::new();
            servers.insert("paired_effect".into(), toml::Value::Table(server));
            let mut mcp = toml::Table::new();
            mcp.insert("servers".into(), toml::Value::Table(servers));
            table.insert("mcp".into(), toml::Value::Table(mcp));
        }
        std::fs::write(
            workspace.join(".wayland-core/config.toml"),
            toml::to_string(&config)?,
        )?;
        task.repository()?.materialize(&workspace)?;
        Ok(Self {
            _root: root,
            home,
            workspace,
        })
    }
}

impl PairedTask {
    /// Remaining paid admission is deliberately not implied by a working bridge.
    /// Loopback controls can execute now; a metered paid boundary must be supplied
    /// before the comparative spending ceiling can be enforced for both products.
    pub async fn run_core(
        &self,
        provider: &ProviderConfig,
        binary: &Path,
        output: &Path,
    ) -> anyhow::Result<ScenarioResult> {
        anyhow::ensure!(
            provider.cost_is_known_free,
            "paired paid calls are blocked until shared all-call spend admission is verified"
        );
        // Validates the known-free designation is actually loopback.
        let _validated = crate::tempenv::build(provider)?;
        anyhow::ensure!(!output.exists(), "paired output already exists");
        std::fs::create_dir_all(output)?;
        let effect = if matches!(
            self.family,
            Family::ToolSkillDiscovery | Family::InterruptedExecutionRecovery
        ) {
            let prefix = if self.family == Family::ToolSkillDiscovery {
                "P"
            } else {
                "REC"
            };
            Some(
                EffectFixture::start(
                    format!("{prefix}-{}", self.sentinel),
                    self.family == Family::InterruptedExecutionRecovery,
                )
                .await?,
            )
        } else {
            None
        };
        let sessions: u32 = match self.family {
            Family::RelevantMemoryRecall => 3,
            Family::InterruptedExecutionRecovery => 2,
            _ => 1,
        };
        let cap = self.max_cost_usd / f64::from(sessions);
        let env = Environment::build(self, provider, cap, effect.as_ref().map(|e| e.url.as_str()))?;
        let mut scenario = self.scenario()?;
        scenario.setup = None; // Inputs are materialized once, not reset on resume.
        scenario.max_total_cost_usd = cap;
        scenario.max_total_time =
            Duration::from_secs((self.max_time_secs / u64::from(sessions)).max(1));
        let mut results = Vec::new();
        if self.family == Family::RelevantMemoryRecall {
            anyhow::ensure!(
                self.prompts.len() == 2,
                "memory task requires store and recall prompts"
            );
            let control = Environment::build(self, provider, cap, None)?;
            scenario.turns =
                vec![Turn::new(&self.prompts[1]).max_time(Duration::from_secs(self.max_time_secs))];
            let result = run_with_binary_in_paths(
                &scenario,
                provider,
                binary,
                &control.workspace,
                &control.home,
            )
            .await?;
            record_session(output, results.len(), &result)?;
            anyhow::ensure!(
                result.passed
                    && !result
                        .final_text
                        .contains(&format!("MEM-{}", self.sentinel)),
                "fresh-home memory control failed or leaked fact"
            );
            results.push(result);
            scenario.turns = vec![Turn::new(&self.prompts[0])];
            let store =
                run_with_binary_in_paths(&scenario, provider, binary, &env.workspace, &env.home)
                    .await?;
            let stored = store.passed;
            record_session(output, results.len(), &store)?;
            results.push(store);
            anyhow::ensure!(
                stored,
                "memory storage session failed; refusing another call"
            );
            scenario.turns = vec![Turn::new(&self.prompts[1])];
        } else if self.family == Family::InterruptedExecutionRecovery {
            anyhow::ensure!(
                self.prompts.len() == 2,
                "recovery task requires initial and resume prompts"
            );
            let identity = self.seed[..32].to_owned();
            scenario.session = SessionIdentity::Create(identity.clone());
            scenario.turns = vec![Turn::new(&self.prompts[0])];
            scenario.cut_after_effect = effect.as_ref().map(EffectFixture::barrier);
            let interrupted =
                run_with_binary_in_paths(&scenario, provider, binary, &env.workspace, &env.home)
                    .await?;
            let real_cut = interrupted.execution.cleanup_verified && interrupted.failures.iter().any(|f| matches!(f, Failure::RunnerError(message) if message == "fixture after-effect process cut"));
            std::fs::write(
                output.join("interrupted.json"),
                serde_json::to_vec_pretty(&interrupted)?,
            )?;
            let effects_at_cut = effect
                .as_ref()
                .expect("recovery effect fixture")
                .export(&output.join("after-cut-effect.jsonl"))?;
            anyhow::ensure!(
                real_cut && effects_at_cut == 1,
                "expected after-effect cut did not occur with verified cleanup"
            );
            results.push(interrupted);
            effect.as_ref().expect("recovery effect fixture").release();
            scenario.session = SessionIdentity::Resume(identity);
            scenario.cut_after_effect = None;
            scenario.turns = vec![Turn::new(&self.prompts[1])];
        }
        let final_result =
            run_with_binary_in_paths(&scenario, provider, binary, &env.workspace, &env.home)
                .await?;
        record_session(output, results.len(), &final_result)?;
        results.push(final_result);
        let mut result = results.last().expect("at least one run").clone();
        result.cost_usd = results.iter().map(|r| r.cost_usd).sum();
        result.wall_time = results.iter().map(|r| r.wall_time).sum();
        result.execution.provider_attempts = results
            .iter()
            .map(|r| r.execution.provider_attempts)
            .collect::<Option<Vec<_>>>()
            .map(|counts| counts.into_iter().sum());
        result.execution.provider_retries = results
            .iter()
            .map(|r| r.execution.provider_retries)
            .collect::<Option<Vec<_>>>()
            .map(|counts| counts.into_iter().sum());
        if sessions > 1 {
            // Per-session usage remains in sessions.json. Do not label only
            // the final session's usage as the complete multi-session total.
            result.execution.provider_usage = None;
        }
        result.execution.cleanup_verified = results.iter().all(|r| r.execution.cleanup_verified);
        if self.family == Family::RelevantMemoryRecall {
            let identities: std::collections::BTreeSet<_> = results
                .iter()
                .flat_map(|r| &r.info_events)
                .filter_map(|event| event.strip_prefix("paired_session_identity:"))
                .collect();
            if identities.len() != 3 {
                result.failures.push(Failure::RunnerError(
                    "memory control/store/recall did not use three native sessions".into(),
                ));
            }
        }
        for earlier in &results[..results.len() - 1] {
            result.failures.extend(earlier.failures.iter().filter(|f| !matches!(f, Failure::RunnerError(m) if m == "fixture after-effect process cut")).cloned());
        }
        if let Err(error) = self
            .check_artifacts(&env.workspace, &result.final_text)
            .await
        {
            result.failures.push(Failure::RunnerError(format!(
                "paired artifact oracle: {error}"
            )));
        }
        if self.family == Family::AnswerWithoutAction && !result.trace.entries.is_empty() {
            result.failures.push(Failure::RunnerError(
                "answer-without-action invoked tools".into(),
            ));
        }
        if self.family == Family::LongSessionConstraintRetention
            && !result
                .info_events
                .iter()
                .any(|e| e.starts_with("paired_compaction_observed:"))
        {
            result.failures.push(Failure::RunnerError(
                "long-session task never observed real compaction".into(),
            ));
        }
        if self.family == Family::ToolSkillDiscovery
            && !result
                .trace
                .entries
                .iter()
                .any(|entry| entry.tool_name == "Skill" && entry.input.contains("parcel-label"))
        {
            result.failures.push(Failure::RunnerError(
                "label task did not invoke its installed skill".into(),
            ));
        }
        if let Some(effect) = effect {
            let count = effect.export(&output.join("effect.jsonl"))?;
            if count != 1 {
                result.failures.push(Failure::RunnerError(format!(
                    "expected exactly one actual effect, observed {count}"
                )));
            }
            effect.stop().await?;
        }
        if result.cost_usd > self.max_cost_usd {
            result.failures.push(Failure::RunnerError(
                "paired whole-task cost bound exceeded".into(),
            ));
        }
        if result.wall_time > Duration::from_secs(self.max_time_secs) {
            result.failures.push(Failure::RunnerError(
                "paired whole-task time bound exceeded".into(),
            ));
        }
        // Keep the ordinary runner's diagnostic outcome semantics. A real
        // cleanup failure is already a typed Failure; unavailable authoritative
        // proof stays in execution evidence and blocks receipt certification.
        // Converting that absence into an unexplained failed outcome produces
        // an invalid receipt (passed=false with no stable failure reason).
        result.passed = result.passed && result.failures.is_empty();
        std::fs::write(
            output.join("sessions.json"),
            serde_json::to_vec_pretty(&results)?,
        )?;
        std::fs::write(output.join("task.json"), serde_json::to_vec_pretty(self)?)?;
        export_workspace(&env.workspace, &output.join("workspace"))?;
        Ok(result)
    }
}

fn export_workspace(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_name() == ".wayland-core" || entry.file_name() == ".git" {
            continue;
        }
        let kind = entry.file_type()?;
        anyhow::ensure!(!kind.is_symlink(), "refusing symlink in paired artifacts");
        if kind.is_dir() {
            export_workspace(&entry.path(), &to.join(entry.file_name()))?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn record_session(output: &Path, index: usize, result: &ScenarioResult) -> anyhow::Result<()> {
    std::fs::write(
        output.join(format!("session-{index}.json")),
        serde_json::to_vec_pretty(result)?,
    )?;
    Ok(())
}
