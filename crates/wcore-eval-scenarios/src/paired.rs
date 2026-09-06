//! W16 prepared task files lowered into the existing scenario runner.
//! Peer preparation and verification use the identical inputs and artifact oracle.
use std::{collections::BTreeMap, path::Path, time::Duration};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    fixtures::repository::SeededRepository,
    scenario::{Category, Scenario, Turn},
};

mod effect;
mod oracle;
mod run;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    RepositoryDiagnosis,
    BoundedCodeChange,
    MultiFileChange,
    AnswerWithoutAction,
    ToolSkillDiscovery,
    RelevantMemoryRecall,
    LongSessionConstraintRetention,
    InterruptedExecutionRecovery,
}

impl Family {
    pub fn scenario_id(self) -> &'static str {
        match self {
            Self::RepositoryDiagnosis => "w16_repository_diagnosis",
            Self::BoundedCodeChange => "w16_bounded_code_change",
            Self::MultiFileChange => "w16_multi_file_change",
            Self::AnswerWithoutAction => "w16_answer_without_action",
            Self::ToolSkillDiscovery => "w16_tool_skill_discovery",
            Self::RelevantMemoryRecall => "w16_relevant_memory_recall",
            Self::LongSessionConstraintRetention => "w16_long_session_constraint_retention",
            Self::InterruptedExecutionRecovery => "w16_interrupted_execution_recovery",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PairedTask {
    pub schema: u32,
    pub family: Family,
    pub case_id: String,
    pub seed: String,
    pub sentinel: String,
    pub files: BTreeMap<String, String>,
    pub prompts: Vec<String>,
    /// Whole task, including every cold/resumed session and its auxiliaries.
    pub max_cost_usd: f64,
    pub max_time_secs: u64,
}

impl PairedTask {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let task: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        task.validate()?;
        Ok(task)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.schema == 1, "unsupported paired task schema");
        anyhow::ensure!(
            self.seed.len() == 64 && self.seed.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid fixture seed"
        );
        anyhow::ensure!(
            !self.case_id.is_empty()
                && self
                    .case_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "invalid case id"
        );
        anyhow::ensure!(
            !self.sentinel.is_empty()
                && self.sentinel.len() <= 128
                && self
                    .sentinel
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "invalid sentinel"
        );
        anyhow::ensure!(
            !self.prompts.is_empty() && self.prompts.iter().all(|p| !p.trim().is_empty()),
            "paired task needs substantive prompts"
        );
        anyhow::ensure!(
            self.max_cost_usd.is_finite() && self.max_cost_usd > 0.0 && self.max_cost_usd <= 0.25,
            "paired whole-task cost bound must be positive and at most $0.25"
        );
        anyhow::ensure!(
            self.max_time_secs > 0 && self.max_time_secs <= 7200,
            "invalid paired time bound"
        );
        anyhow::ensure!(
            self.files.keys().all(|p| !p.starts_with(".git/")
                && p != ".git"
                && !p.ends_with("config.toml")
                && !p.ends_with("auth.json")),
            "fixture cannot replace authentication/config authority"
        );
        self.repository()?;
        if self.family == Family::LongSessionConstraintRetention {
            anyhow::ensure!(
                self.prompts.len() >= 8
                    && self.files.values().map(String::len).sum::<usize>() >= 200_000,
                "long-session fixture must retain substantial multi-turn history, not a tiny compaction smoke"
            );
        }
        Ok(())
    }

    fn repository(&self) -> anyhow::Result<SeededRepository> {
        Ok(SeededRepository::new(
            self.files.iter().map(|(p, c)| (p.clone(), c.clone())),
        )?)
    }

    pub fn digest(&self) -> anyhow::Result<String> {
        Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(self)?)))
    }

    /// Materialize inputs for either product; never overwrite an existing trial.
    pub fn prepare_peer(&self, root: &Path) -> anyhow::Result<()> {
        anyhow::ensure!(!root.exists(), "paired preparation root already exists");
        std::fs::create_dir_all(root)?;
        self.repository()?.materialize(&root.join("workspace"))?;
        for (i, prompt) in self.prompts.iter().enumerate() {
            std::fs::write(root.join(format!("prompt-{i}.txt")), prompt)?;
        }
        std::fs::write(root.join("task.json"), serde_json::to_vec_pretty(self)?)?;
        std::fs::write(root.join("fixture.sha256"), self.digest()?)?;
        Ok(())
    }

    pub fn scenario(&self) -> anyhow::Result<Scenario> {
        self.validate()?;
        let repository = self.repository()?;
        let mut scenario = Scenario::new(self.family.scenario_id(), Category::Multiturn)
            .platforms([crate::scenario::Platform::Linux])
            .max_total_cost_usd(self.max_cost_usd)
            .max_total_time(Duration::from_secs(self.max_time_secs))
            .approval(
                if matches!(
                    self.family,
                    Family::RepositoryDiagnosis | Family::AnswerWithoutAction
                ) {
                    crate::scenario::ApprovalPolicy::DenyAll
                } else {
                    crate::scenario::ApprovalPolicy::ApproveAll
                },
            )
            .setup(move |cwd| {
                repository.materialize(cwd)?;
                Ok(())
            });
        for prompt in &self.prompts {
            scenario = scenario.turn(
                Turn::new(prompt)
                    .max_time(Duration::from_secs(self.max_time_secs))
                    .max_steps(24),
            );
        }
        Ok(scenario)
    }

    pub async fn check_artifacts(&self, workspace: &Path, final_text: &str) -> anyhow::Result<()> {
        oracle::check(self, workspace, final_text).await
    }

    /// The peer uses the same MCP service and effect witness as Core. The
    /// supervisor owns this process and sends SIGINT only after reaping the peer.
    pub async fn serve_effects(&self, output: &Path) -> anyhow::Result<()> {
        anyhow::ensure!(
            matches!(
                self.family,
                Family::ToolSkillDiscovery | Family::InterruptedExecutionRecovery
            ),
            "task has no effect service"
        );
        anyhow::ensure!(!output.exists(), "effect output already exists");
        std::fs::create_dir_all(output)?;
        let recovery = self.family == Family::InterruptedExecutionRecovery;
        let prefix = if recovery { "REC" } else { "P" };
        let fixture =
            effect::EffectFixture::start(format!("{prefix}-{}", self.sentinel), recovery).await?;
        let barrier = fixture.barrier();
        std::fs::write(
            output.join("ready.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "url":fixture.url,"barrier":barrier.path,"expected":String::from_utf8(barrier.expected)?,"task_sha256":self.digest()?
            }))?,
        )?;
        tokio::signal::ctrl_c().await?;
        let count = fixture.export(&output.join("effect.jsonl"))?;
        fixture.stop().await?;
        std::fs::write(
            output.join("receipt.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({"effects":count,"task_sha256":self.digest()?,"server_joined":true}),
            )?,
        )?;
        Ok(())
    }
}
