//! `wayland-scorecard` — the single executable surface of Phase 30 (F30-01, F30-02).
//!
//! Two subcommands:
//!
//! - `surfaces` walks a real binary's own `--help` tree and emits a sorted,
//!   byte-deterministic table. The inventory is therefore a MEASUREMENT of the
//!   shipped artifact, not a reading of a planning document — a gate can
//!   regenerate it on real hardware and diff it against the committed bytes, so
//!   a hand-edited inventory fails.
//! - `verify` checks a scorecard document against a repository: every surface
//!   row must carry its seven truths and every criterion graded MET must pay for
//!   it with resolving, proven evidence.
//!
//! No secret is read, printed, logged or accepted on argv.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};
use wcore_eval_scenarios::claims::{ClaimRegisterV1, publish, register_digest};
use wcore_eval_scenarios::dialect::{
    CompiledStepV1, ToolSchemaCorpusV1, TranslationV1, VOCABULARY_VERSION, canonical_script,
    cohort_eligibility, compile_script, vocabulary_carries_no_product_token,
};
use wcore_eval_scenarios::dialect_discovery::{DiscoveryManifestV1, RunningDiscoveryMeter};
use wcore_eval_scenarios::dialect_exec::{DialectBindingV1, bind_translation};
use wcore_eval_scenarios::fixtures::openai::{OpenAiFixtureScript, OpenAiStep};
use wcore_eval_scenarios::frontier_trials::{
    ALL_DIMENSIONS, ALL_TOOLS, ComparativeResultV1, DeltaV1, DimensionV1, LegStatusV1, LegV1,
    MeasurementV1, ResultSetV1, ScopeV1, ToolInvocationV1, ToolV1, TrialOutcomeV1, TrialRecordV1,
    bootstrap_difference, continuous_measurement, newcombe_wilson_difference,
    proportion_measurement, protocol_sha256,
};
use wcore_eval_scenarios::reserved_authority::{
    ApprovalRecordV1, ApprovalTrustRootV1, ReservedActionV1, RootKindV1, mint_approval,
};
use wcore_eval_scenarios::scorecard::{
    ScorecardDocumentV1, render_surfaces_tsv, walk_command_tree,
};

#[derive(Parser)]
#[command(
    name = "wayland-scorecard",
    about = "Walk a shipped binary's command tree and verify a scorecard document"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Walk a binary's own --help tree and emit the sorted surface table.
    Surfaces {
        /// Path to the binary to walk. It is EXECUTED with `--help`.
        #[arg(long)]
        binary: PathBuf,
    },
    /// Verify a scorecard document against a repository.
    Verify {
        /// Path to the scorecard JSON document.
        #[arg(long)]
        document: PathBuf,
        /// Repository root that evidence references are resolved against.
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
    },
    /// Phase 30 (F30-03) frontier comparative trials. ADDITIVE to the two subcommands
    /// 30-01 landed; neither is reordered or restructured.
    Trials {
        #[command(subcommand)]
        command: TrialsCommand,
    },
    /// Phase 30 (F30-04) claims register. ADDITIVE alongside `surfaces`, `verify` and
    /// `trials`; none of them is reordered or restructured.
    Claims {
        #[command(subcommand)]
        command: ClaimsCommand,
    },
    /// Phase 30 (F30-05) reserved authority. ADDITIVE alongside `surfaces`, `verify`,
    /// `trials` and `claims`; none of them is reordered or restructured.
    Authority {
        #[command(subcommand)]
        command: AuthorityCommand,
    },
    /// Phase 30 (SR-30-3) per-tool dialect compilation. ADDITIVE alongside `surfaces`, `verify`,
    /// `trials`, `claims` and `authority`; none of them is reordered or restructured.
    Dialect {
        #[command(subcommand)]
        command: DialectCommand,
    },
}

#[derive(Subcommand)]
enum DialectCommand {
    /// UNSCORED. Launch one harness against a loopback discovery meter and record the tool
    /// schema THE HARNESS ITSELF declares on the wire.
    ///
    /// Nothing here is measured and nothing here enters a comparative. The child is spawned with
    /// a CLEARED environment plus the same non-secret allowlist `trials run` uses, and the meter
    /// retains only the `tools` declaration — never `messages`, never an argument value.
    Discover {
        /// The same tool-neutral invocation value `trials run` takes. Reused deliberately: a
        /// second, discovery-only invocation format would be a place for per-tool special-casing
        /// to reappear.
        #[arg(long)]
        invocation: PathBuf,
        /// Directory that gets the harness's fresh workspace and the two output files.
        #[arg(long)]
        workspace_root: PathBuf,
        /// Optional version string to record in the manifest, e.g. the pinned commit.
        #[arg(long)]
        tool_version: Option<String>,
        /// Seconds to wait for the harness to declare its tools before giving up. A timeout
        /// yields an EMPTY corpus and a note, never a guess.
        #[arg(long, default_value_t = 120)]
        timeout_s: u64,
        /// Written as `<out-prefix>-corpus.json` and `<out-prefix>-manifest.json`.
        #[arg(long)]
        out_prefix: PathBuf,
    },
    /// Compile the canonical semantic script for one dimension into ONE harness's dialect.
    ///
    /// Takes only the corpus. It is not given, and cannot read, which harness the corpus came
    /// from — that is the identity-blindness guard, enforced by the type.
    Compile {
        #[arg(long)]
        corpus: PathBuf,
        /// correctness | recovery | security | cost
        #[arg(long)]
        dimension: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Recompute a translation's digests from the script and corpus it claims to address.
    ///
    /// This is what makes a hand-tuned translation detectable rather than trusted.
    Verify {
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long)]
        dimension: String,
        #[arg(long)]
        translation: PathBuf,
    },
    /// Print the pre-registered vocabulary's self-check: the version token and the result of the
    /// mechanical "no vocabulary token is a product name" assertion.
    Vocabulary,
    /// THE COHORT GATE (panel amendment). Decide whether a dimension may be run AT ALL.
    ///
    /// A refusal by ANY harness makes the dimension ineligible for EVERY harness, including ours.
    /// EXITS NON-ZERO when ineligible, so a driver script cannot proceed past it by accident —
    /// the whole point of the gate is that it stops a run rather than annotating one.
    Cohort {
        /// Repeatable `label=path/to/corpus.json`. At least TWO are required: a comparative
        /// benchmark that has lost a member has lost the thing it was measuring, and proceeding
        /// with the survivors is how "we could not run the competitor, so we win" gets expressed.
        #[arg(long = "member", value_name = "LABEL=CORPUS_JSON")]
        members: Vec<String>,
        #[arg(long)]
        dimension: String,
        /// Optional path to write the full `CohortEligibilityV1` decision as JSON.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum AuthorityCommand {
    /// Generate a THROWAWAY approval root into a directory, and mint one frontier-positioning
    /// approval under it.
    ///
    /// This is the POSITIVE CONTROL: without it the whole reserved-authority mechanism would
    /// be satisfied by a verifier that refuses unconditionally, which would prove nothing
    /// about whether an approval can ever be honoured. The root declares itself throwaway and
    /// every acceptance reports that kind, so this can never be quoted as Sean's approval.
    ///
    /// The signing seed is written to an owner-only file inside `--dir` and is never printed.
    InitRoot {
        #[arg(long)]
        dir: PathBuf,
    },
    /// Record an approval. The base64 32-byte Ed25519 signing seed is read from STDIN,
    /// exactly as `wayland-receipt sign` reads it, and never from an argument.
    Record {
        /// One of the nine reserved action tokens.
        #[arg(long)]
        action: String,
        /// The 64-character lowercase sha256 digest of the subject being approved.
        #[arg(long)]
        subject_sha256: String,
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Verify an approval against a trust root supplied INDEPENDENTLY of the approval.
    ///
    /// `--bundled-root` checks against the committed all-zeros placeholder, which refuses
    /// every approval and names its own substitution point.
    Verify {
        #[arg(long, conflicts_with = "bundled_root")]
        root: Option<PathBuf>,
        #[arg(long)]
        bundled_root: bool,
        #[arg(long)]
        approval: PathBuf,
    },
}

#[derive(Subcommand)]
enum ClaimsCommand {
    /// Check every entry in a claims register, exiting non-zero naming the first refusal.
    Verify {
        #[arg(long)]
        register: PathBuf,
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
    },
    /// Render the published documents from a VERIFIED register.
    ///
    /// Refuses to write anything at all if `verify` would refuse the register, so there is
    /// no path from an unverified claim to a published sentence.
    Publish {
        #[arg(long)]
        register: PathBuf,
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum TrialsCommand {
    /// Drive ONE tool through one dimension's trials against the shared loopback meter.
    ///
    /// No credential is read, accepted on argv, or placed in the child's environment. The
    /// child is spawned with a CLEARED environment plus an explicit non-secret allowlist.
    Run {
        /// The frozen protocol. Read, never written.
        #[arg(long)]
        protocol: PathBuf,
        /// The tool-neutral invocation value: everything that differs between the three
        /// tools. Any per-tool special-casing beyond this file is a confound.
        #[arg(long)]
        invocation: PathBuf,
        /// correctness | recovery | security | cost
        #[arg(long)]
        dimension: String,
        #[arg(long)]
        trials: u32,
        /// Directory under which each trial gets its OWN fresh workspace.
        #[arg(long)]
        workspace_root: PathBuf,
        /// JSON Lines output, one `TrialRecordV1` per trial.
        #[arg(long)]
        out: PathBuf,

        // ---- protocol v2: drive from a compiled dialect instead of the frozen script ----
        //
        // All three must be supplied together or none at all. Supplying a translation without
        // the corpus it was compiled from and the discovery manifest that says which harness
        // declared that corpus would leave the two checks digests cannot make — see
        // `dialect_exec`.
        /// A `TranslationV1` from `dialect compile`. When given, the protocol's frozen
        /// `fixture_script` is NOT used and this harness is driven in its own dialect.
        #[arg(long, requires_all = ["corpus", "discovery_manifest"])]
        translation: Option<PathBuf>,
        /// The `ToolSchemaCorpusV1` the translation was compiled against.
        #[arg(long, requires = "translation")]
        corpus: Option<PathBuf>,
        /// The `DiscoveryManifestV1` naming the harness that declared that corpus. This is the
        /// only thing that stops harness A being driven with harness B's dialect.
        #[arg(long, requires = "translation")]
        discovery_manifest: Option<PathBuf>,

        /// DIAGNOSTIC ONLY — rewrite every relative-looking path argument to an absolute path
        /// inside the trial's own workspace, identically for EVERY harness.
        ///
        /// This exists because measurement 3 of lane 30-dialect-c2 found `wayland-core`'s `Write`
        /// refuses a relative path while Hermes accepts one, so the canonical script's bare
        /// `TRIAL-ARTIFACT.txt` is a second confound living in a slot VALUE rather than in a tool
        /// name. This flag identifies that cause; it does not license a number.
        ///
        /// Every trial run under it is stamped `diagnostic` and `proportion_measurement` refuses
        /// any leg containing such a trial. Scoring under an absolutized script requires a new
        /// pre-registration, not this flag.
        #[arg(long)]
        diagnostic_absolutize_paths: bool,
    },
    /// Fold per-trial records into the bounded result set.
    ///
    /// Every number in the output is produced by the SAME verified functions the contract
    /// suite exercises. Nothing is computed by a second implementation that could disagree
    /// with the one under test.
    Assemble {
        #[arg(long)]
        protocol: PathBuf,
        /// Directory of `<tool>-<dimension>.jsonl` per-trial records.
        #[arg(long)]
        records_dir: PathBuf,
        /// JSON map from `"<tool>:<dimension>"` to the blocker text for every leg that did
        /// not run. A leg with neither records nor a blocker is a REFUSAL, not a default.
        #[arg(long)]
        blockers: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Verify a result set against the protocol it was run under.
    Verify {
        #[arg(long)]
        protocol: PathBuf,
        #[arg(long)]
        results: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("wayland-scorecard: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<String> {
    match cli.command {
        Command::Surfaces { binary } => {
            let nodes = walk_command_tree(&binary)?;
            Ok(render_surfaces_tsv(&nodes))
        }
        Command::Verify {
            document,
            repo_root,
        } => {
            let raw = std::fs::read_to_string(&document)?;
            // Unknown fields are refused HERE, before any verification logic
            // runs, so an invented grade or a stray key cannot reach the rules.
            let doc: ScorecardDocumentV1 = serde_json::from_str(&raw)?;
            doc.verify(&repo_root)?;
            Ok(format!(
                "SCORECARD_VERIFY=OK criteria={} surfaces={} source_sha={}\n",
                doc.criteria.len(),
                doc.surfaces.len(),
                doc.source_sha
            ))
        }
        Command::Trials { command } => run_trials(command),
        Command::Claims { command } => run_claims(command),
        Command::Authority { command } => run_authority(command),
        Command::Dialect { command } => run_dialect(command),
    }
}

// ---------------------------------------------------------------------------
// Phase 30 F30-05 reserved authority — one contiguous additive block.
// ---------------------------------------------------------------------------

fn run_authority(command: AuthorityCommand) -> anyhow::Result<String> {
    match command {
        AuthorityCommand::InitRoot { dir } => {
            std::fs::create_dir_all(&dir)?;
            let throwaway = ApprovalTrustRootV1::generate_throwaway();

            let root_path = dir.join("root.json");
            let root_bytes = serde_json::to_vec_pretty(&throwaway.root)?;
            std::fs::write(&root_path, &root_bytes)?;

            // The seed goes to an owner-only file and is NEVER printed. There is no
            // subcommand in this binary that prints a seed and none that accepts one on argv.
            let seed_path = dir.join(format!("{}.seed", throwaway.key_id));
            std::fs::write(&seed_path, BASE64.encode(throwaway.seed()))?;
            restrict_to_owner(&seed_path)?;

            // A real subject digest: the digest of the root document that was just written.
            let subject = format!("{:x}", Sha256::digest(&root_bytes));
            let approval = mint_approval(
                ReservedActionV1::FrontierPositioning,
                &subject,
                &throwaway.key_id,
                throwaway.seed(),
            )?;
            let approval_path = dir.join("frontier-positioning.approval.json");
            std::fs::write(&approval_path, serde_json::to_vec_pretty(&approval)?)?;

            let mut out = String::new();
            out.push_str("AUTHORITY_INIT_ROOT=OK root_kind=");
            out.push_str(throwaway.root.root_kind.token());
            out.push('\n');
            for (key_id, public_hex) in &throwaway.root.keys {
                out.push_str(&format!("key_id={key_id} public_key_hex={public_hex}\n"));
            }
            out.push_str(&format!(
                "root={} approval={} subject_sha256={}\n",
                root_path.display(),
                approval_path.display(),
                subject
            ));
            out.push_str(
                "NOTE: this root was generated at run time and is NOT Sean's approval key. \
                 It exists to prove the mechanism can ACCEPT a valid approval; an acceptance \
                 under it authorises nothing.\n",
            );
            Ok(out)
        }
        AuthorityCommand::Record {
            action,
            subject_sha256,
            key_id,
            out,
        } => {
            let action: ReservedActionV1 = serde_json::from_value(serde_json::Value::String(
                action.clone(),
            ))
            .map_err(|_| {
                anyhow::anyhow!(
                    "`{action}` is not one of the nine reserved actions. The action set \
                             is CLOSED: an unrecognised name is refused here rather than mapped \
                             to anything."
                )
            })?;
            let mut secret = SecretBytes(Vec::new());
            std::io::stdin()
                .take(4097)
                .read_to_end(&mut secret.0)
                .map_err(|e| anyhow::anyhow!("could not read signing seed from stdin: {e}"))?;
            if secret.0.len() > 4096 {
                anyhow::bail!("signing seed input exceeds 4096 bytes");
            }
            let decoded = BASE64
                .decode(trim_ascii(&secret.0))
                .map_err(|_| anyhow::anyhow!("signing seed is not valid base64"))?;
            let mut seed = SecretBytes(decoded);
            let seed_array: [u8; 32] = seed
                .0
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("signing seed must decode to exactly 32 bytes"))?;
            let approval = mint_approval(action, &subject_sha256, &key_id, &seed_array)?;
            wipe(&mut seed.0);
            std::fs::write(&out, serde_json::to_vec_pretty(&approval)?)?;
            Ok(format!(
                "AUTHORITY_RECORD=OK action={} principal={} subject_sha256={} out={}\n",
                approval.action.token(),
                approval.principal.token(),
                approval.subject_sha256,
                out.display()
            ))
        }
        AuthorityCommand::Verify {
            root,
            bundled_root,
            approval,
        } => {
            let approval_bytes = std::fs::read(&approval)?;
            // Unknown fields are refused HERE, before any verification logic runs, so an
            // invented action or a stray `approved_by_agent` key cannot reach the rules.
            let approval: ApprovalRecordV1 = serde_json::from_slice(&approval_bytes)?;
            let root = match (root, bundled_root) {
                (Some(path), false) => {
                    let raw = std::fs::read(&path)?;
                    serde_json::from_slice::<ApprovalTrustRootV1>(&raw)?
                }
                (None, true) => ApprovalTrustRootV1::bundled(),
                _ => anyhow::bail!(
                    "supply exactly one of --root <file> or --bundled-root; the trust root \
                     must arrive independently of the approval"
                ),
            };
            let verified = root.verify(&approval)?;
            let mut out = format!(
                "AUTHORITY_VERIFY=ACCEPTED action={} principal={} subject_sha256={} \
                 key_id={} root_kind={}\n",
                verified.action.token(),
                verified.principal.token(),
                verified.subject_sha256,
                verified.key_id,
                verified.root_kind.token()
            );
            if verified.root_kind != RootKindV1::OperatorSupplied {
                out.push_str(
                    "NOTE: root_kind is not operator_supplied. This acceptance proves the \
                     MECHANISM works; it is not an approval and authorises nothing.\n",
                );
            }
            Ok(out)
        }
    }
}

/// Restrict a file to its owner. Unix only — on other platforms the seed still never leaves
/// the caller-named directory, and no gate in this phase depends on the mode.
fn restrict_to_owner(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

struct SecretBytes(Vec<u8>);

impl Drop for SecretBytes {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        // SAFETY: `byte` is a valid unique reference for this write. Volatile prevents the
        // compiler from eliding this security-sensitive wipe.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = bytes.len();
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    &bytes[start..end]
}

// ---------------------------------------------------------------------------
// Phase 30 F30-04 claims — one contiguous additive block.
// ---------------------------------------------------------------------------

fn run_claims(command: ClaimsCommand) -> anyhow::Result<String> {
    match command {
        ClaimsCommand::Verify {
            register,
            repo_root,
        } => {
            let raw = std::fs::read(&register)?;
            // Unknown fields are refused HERE, before any rule runs.
            let reg: ClaimRegisterV1 = serde_json::from_slice(&raw)?;
            reg.verify(&repo_root)?;
            let rules = reg.rules_fired(&repo_root);
            // `allowed` counts ONLY the claims that reach CLAIMS-ALLOWED.md. Limitations
            // are counted separately: reporting one total would overstate the allowed set
            // by the size of the limitations list, which is the larger of the two here.
            Ok(format!(
                "CLAIMS_VERIFY=OK allowed={} limitations={} attempted_and_refused={} \
                 rules_fired={} register_sha256={}\n",
                reg.allowed_count(),
                reg.limitation_count(),
                reg.refusals(&repo_root).len(),
                rules.len(),
                register_digest(&raw)
            ))
        }
        ClaimsCommand::Publish {
            register,
            repo_root,
            out,
        } => {
            let raw = std::fs::read(&register)?;
            // No secret is read, accepted on argv, or required. The bundle scan runs over
            // an empty secret set because this phase holds no credential at all.
            let set = publish(&raw, &repo_root, &out, Vec::new())?;
            Ok(format!(
                "CLAIMS_PUBLISH=OK register_sha256={} out={}\n",
                set.digest,
                out.display()
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 30 F30-03 trials — one contiguous additive block.
// ---------------------------------------------------------------------------

/// SR-30-3 — per-tool dialect compilation.
///
/// Note what is NOT here: there is no per-tool branch anywhere in this function or anything it
/// calls. `discover` runs the same code for every harness, and `compile` is not even told which
/// harness a corpus belongs to.
fn run_dialect(command: DialectCommand) -> anyhow::Result<String> {
    match command {
        DialectCommand::Vocabulary => {
            let offenders = vocabulary_carries_no_product_token();
            if !offenders.is_empty() {
                anyhow::bail!(
                    "DIALECT_VOCABULARY=TAINTED the vocabulary names a product: {offenders:?}"
                );
            }
            Ok(format!(
                "DIALECT_VOCABULARY=OK version={VOCABULARY_VERSION} product_tokens_found=0\n"
            ))
        }
        DialectCommand::Cohort {
            members,
            dimension,
            out,
        } => {
            let mut cohort = Vec::new();
            for entry in &members {
                let (label, path) = entry
                    .split_once('=')
                    .ok_or_else(|| anyhow::anyhow!("--member must be LABEL=PATH, got `{entry}`"))?;
                let corpus: ToolSchemaCorpusV1 = serde_json::from_slice(&std::fs::read(path)?)?;
                cohort.push((label.to_string(), corpus));
            }
            let decision = cohort_eligibility(&dimension, &cohort)?;
            if let Some(out) = out {
                std::fs::write(&out, serde_json::to_vec_pretty(&decision)?)?;
            }
            let mut report = String::new();
            for member in &decision.members {
                report.push_str(&format!(
                    "  member={} declared_tools={} resolved={} refusal={} corpus_sha256={}\n",
                    member.tool_label,
                    member.declared_tools,
                    member.resolved_tool.as_deref().unwrap_or("-"),
                    member.refusal.as_deref().unwrap_or("-"),
                    member.corpus_sha256
                ));
            }
            // A gate that only ever printed would be a gate that cannot fail. Ineligible EXITS
            // NON-ZERO, after printing every member's outcome so the cause is never a mystery.
            if !decision.eligible {
                anyhow::bail!("{}\n{report}", decision.verdict_line());
            }
            Ok(format!("{}\n{report}", decision.verdict_line()))
        }
        DialectCommand::Compile {
            corpus,
            dimension,
            out,
        } => {
            let corpus: ToolSchemaCorpusV1 = serde_json::from_slice(&std::fs::read(&corpus)?)?;
            let script = canonical_script(&dimension).ok_or_else(|| {
                anyhow::anyhow!("no canonical script for dimension `{dimension}`")
            })?;
            // A refusal is a RESULT, not an error to be worked around. It exits non-zero so a
            // caller cannot mistake it for a translation, and it names the reason so the leg can
            // be recorded UNPROVEN with a cause rather than scored as a failure.
            let translation = compile_script(&script, &corpus)?;
            std::fs::write(&out, serde_json::to_vec_pretty(&translation)?)?;
            let calls: Vec<&str> = translation
                .steps
                .iter()
                .filter_map(|step| match step {
                    CompiledStepV1::ToolCall(call) => Some(call.tool_name.as_str()),
                    _ => None,
                })
                .collect();
            Ok(format!(
                "DIALECT_COMPILE=OK dimension={} declared_tools={} calls={} tool_names={} \
                 corpus_sha256={} translation_sha256={}\n",
                dimension,
                corpus.tools.len(),
                calls.len(),
                calls.join(","),
                translation.corpus_sha256,
                translation.translation_sha256
            ))
        }
        DialectCommand::Verify {
            corpus,
            dimension,
            translation,
        } => {
            let corpus: ToolSchemaCorpusV1 = serde_json::from_slice(&std::fs::read(&corpus)?)?;
            let script = canonical_script(&dimension).ok_or_else(|| {
                anyhow::anyhow!("no canonical script for dimension `{dimension}`")
            })?;
            let translation: TranslationV1 = serde_json::from_slice(&std::fs::read(&translation)?)?;
            translation.verify(&script, &corpus)?;
            Ok(format!(
                "DIALECT_VERIFY=OK dimension={} vocabulary={} corpus_sha256={} \
                 translation_sha256={}\n",
                dimension,
                translation.vocabulary_version,
                translation.corpus_sha256,
                translation.translation_sha256
            ))
        }
        DialectCommand::Discover {
            invocation,
            workspace_root,
            tool_version,
            timeout_s,
            out_prefix,
        } => {
            let invocation: ToolInvocationV1 =
                serde_json::from_slice(&std::fs::read(&invocation)?)?;
            let runtime = tokio::runtime::Runtime::new()?;
            let capture = runtime.block_on(discover_dialect(
                &invocation,
                &workspace_root,
                tool_version,
                Duration::from_secs(timeout_s),
            ))?;
            let corpus_path = out_prefix.with_file_name(format!(
                "{}-corpus.json",
                out_prefix
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("dialect")
            ));
            let manifest_path = out_prefix.with_file_name(format!(
                "{}-manifest.json",
                out_prefix
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("dialect")
            ));
            std::fs::write(&corpus_path, serde_json::to_vec_pretty(&capture.corpus)?)?;
            std::fs::write(
                &manifest_path,
                serde_json::to_vec_pretty(&capture.manifest)?,
            )?;
            let names: Vec<&str> = capture
                .corpus
                .tools
                .iter()
                .map(|t| t.name.as_str())
                .collect();
            Ok(format!(
                "DIALECT_DISCOVER label={} declared_tools={} requests={} corpus_sha256={} \
                 model={} names={}\n",
                capture.manifest.tool_label,
                capture.corpus.tools.len(),
                capture.manifest.requests_observed,
                capture.manifest.corpus_sha256,
                capture.manifest.model_requested.as_deref().unwrap_or("-"),
                names.join(","),
            ))
        }
    }
}

/// Launch one harness against a discovery meter and capture what it declares.
///
/// The environment discipline is copied from `drive_leg` verbatim — `env_clear()` plus the same
/// non-secret allowlist — because a discovery pass that ran with an ambient credential present
/// would be a credential leak in an unscored corner where nobody was looking.
async fn discover_dialect(
    invocation: &ToolInvocationV1,
    workspace_root: &Path,
    tool_version: Option<String>,
    timeout: Duration,
) -> anyhow::Result<wcore_eval_scenarios::dialect_discovery::DiscoveryCapture> {
    let workspace = workspace_root.join(format!("discover-{}", invocation.tool.token()));
    std::fs::create_dir_all(&workspace)?;

    let meter = RunningDiscoveryMeter::start().await?;
    let base_url = format!("{}{}", meter.base_url(), invocation.base_url_suffix);

    for (relative, contents) in &invocation.workspace_seed_files {
        let path = workspace.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents.replace("{{BASE_URL}}", &base_url))?;
    }

    // `{{BASE_URL}}` is substituted in ARGV as well as in seed files. The fixture binds port 0, so
    // a harness that takes its endpoint on the command line rather than from an environment
    // variable or a config file cannot otherwise be pointed at it. The facility is available to
    // every harness identically and, like the seed-file facility, is carried as DATA in the
    // invocation so a reader sees exactly what each harness needed.
    let args: Vec<String> = invocation
        .args
        .iter()
        .map(|arg| arg.replace("{{BASE_URL}}", &base_url))
        .collect();

    let started = Instant::now();
    let mut child = {
        let mut cmd = tokio::process::Command::new(&invocation.program);
        cmd.args(&args)
            .current_dir(&workspace)
            .kill_on_drop(true)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", workspace.display().to_string())
            .env("LANG", "C.UTF-8")
            .env(&invocation.base_url_env, &base_url)
            // The same synthetic literal the frozen protocol uses. It authenticates nothing.
            .env("OPENAI_API_KEY", "wayland-frontier-trial-not-a-secret");
        for (key, value) in &invocation.extra_env {
            cmd.env(key, value);
        }
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        cmd.spawn()?
    };

    // Stop as soon as the declaration is in hand — there is nothing further to learn — or when
    // the harness exits, or at the timeout. A timeout produces an EMPTY corpus and a note.
    loop {
        if meter.requests_observed() > 0 && !meter.capture("probe", None).corpus.tools.is_empty() {
            break;
        }
        if child.try_wait()?.is_some() {
            break;
        }
        if started.elapsed() > timeout {
            let _ = child.start_kill();
            tokio::time::sleep(Duration::from_secs(2)).await;
            let _ = child.kill().await;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let _ = child.start_kill();
    let capture = meter.capture(invocation.tool.token(), tool_version);
    meter.shutdown().await?;
    Ok(capture)
}

fn run_trials(command: TrialsCommand) -> anyhow::Result<String> {
    match command {
        TrialsCommand::Verify { protocol, results } => {
            let protocol_bytes = std::fs::read(&protocol)?;
            let raw = std::fs::read_to_string(&results)?;
            // Unknown fields are refused HERE, before any rule runs.
            let set: ResultSetV1 = serde_json::from_str(&raw)?;
            set.verify(&protocol_bytes)?;
            let run = set
                .legs
                .iter()
                .filter(|l| {
                    matches!(
                        l.status,
                        wcore_eval_scenarios::frontier_trials::LegStatusV1::Run
                    )
                })
                .count();
            Ok(format!(
                "TRIALS_VERIFY=OK legs={} run={} unproven={} comparatives={} scope={} \
                 protocol_sha256={}\n",
                set.legs.len(),
                run,
                set.legs.len() - run,
                set.comparatives.len(),
                set.scope.token(),
                set.protocol_sha256
            ))
        }
        TrialsCommand::Assemble {
            protocol,
            records_dir,
            blockers,
            out,
        } => assemble(&protocol, &records_dir, &blockers, &out),
        TrialsCommand::Run {
            protocol,
            invocation,
            dimension,
            trials,
            workspace_root,
            out,
            translation,
            corpus,
            discovery_manifest,
            diagnostic_absolutize_paths,
        } => {
            let protocol_json: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&protocol)?)?;
            let invocation: ToolInvocationV1 =
                serde_json::from_slice(&std::fs::read(&invocation)?)?;
            let dim = match dimension.as_str() {
                "correctness" => DimensionV1::Correctness,
                "recovery" => DimensionV1::Recovery,
                "security" => DimensionV1::Security,
                "cost" => DimensionV1::Cost,
                other => anyhow::bail!(
                    "dimension `{other}` is not runnable in the loopback tier; \
                     cognitive_tax is UNPROVEN by construction of the protocol"
                ),
            };
            // Protocol v2. `clap`'s `requires_all` already guarantees all-or-nothing, so an
            // partially-specified dialect never reaches here.
            let binding = match (&translation, &corpus, &discovery_manifest) {
                (Some(t), Some(c), Some(m)) => {
                    let translation: TranslationV1 = serde_json::from_slice(&std::fs::read(t)?)?;
                    let corpus: ToolSchemaCorpusV1 = serde_json::from_slice(&std::fs::read(c)?)?;
                    let manifest: DiscoveryManifestV1 = serde_json::from_slice(&std::fs::read(m)?)?;
                    // The harness label comes from the INVOCATION we are about to spawn, never
                    // from the translation — otherwise the thing being checked would be
                    // supplying its own answer.
                    Some(bind_translation(
                        &dimension,
                        invocation.tool.token(),
                        &translation,
                        &corpus,
                        &manifest,
                    )?)
                }
                _ => None,
            };

            let runtime = tokio::runtime::Runtime::new()?;
            let records = runtime.block_on(drive_leg(
                &protocol_json,
                &invocation,
                dim,
                trials,
                &workspace_root,
                binding.as_ref(),
                diagnostic_absolutize_paths,
            ))?;
            let mut lines = String::new();
            for record in &records {
                lines.push_str(&serde_json::to_string(record)?);
                lines.push('\n');
            }
            std::fs::write(&out, &lines)?;
            let successes = records
                .iter()
                .filter(|r| r.outcome == TrialOutcomeV1::Success)
                .count();
            let incompatible = records
                .iter()
                .filter(|r| r.outcome == TrialOutcomeV1::HarnessIncompatible)
                .count();
            let no_contact = records
                .iter()
                .filter(|r| r.outcome == TrialOutcomeV1::NoContact)
                .count();
            // The script provenance is printed on the SAME line as the score. A reader who sees
            // `success=0` must be able to see, without opening another file, whether the harness
            // was driven in its own dialect or in the frozen script's.
            let (script_mode, driven_tools, translation_sha) = match &binding {
                Some(b) => (
                    "dialect_compiled",
                    b.provenance.resolved_tool_names.join(","),
                    b.provenance.translation_sha256.clone(),
                ),
                None => ("frozen_script_v1", "-".to_string(), "-".to_string()),
            };
            let script_mode = if diagnostic_absolutize_paths {
                format!("{script_mode}+DIAGNOSTIC_ABSOLUTIZED")
            } else {
                script_mode.to_string()
            };
            Ok(format!(
                "TRIALS_RUN tool={} dimension={} trials={} success={} harness_incompatible={} \
                 no_contact={} script={} driven_tools={} translation_sha256={} out={}\n",
                invocation.tool.token(),
                dim.token(),
                records.len(),
                successes,
                incompatible,
                no_contact,
                script_mode,
                driven_tools,
                translation_sha,
                out.display()
            ))
        }
    }
}

/// The stop rule, frozen in the protocol as STOP_RULE_V1.
///
/// The protocol's inactivity definition also counts stdout/stderr bytes and workspace
/// mutation; this implementation observes FIXTURE REQUESTS only. That narrowing can only
/// make a trial time out SOONER, never later, so it cannot flatter any tool — and it is
/// recorded in the results rather than left for a reader to discover.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(120);
const ABSOLUTE_TIMEOUT: Duration = Duration::from_secs(600);

/// DIAGNOSTIC instrument. Rewrite every argument value that looks like a bare relative path into
/// an absolute path inside this trial's own workspace.
///
/// **Applied identically to every harness, or it would be exactly the arm-tuning it exists to
/// investigate.** The rule reads only the VALUE — it does not know which harness it is serving,
/// which tool is being called, or what any parameter is named. A value qualifies only if it is
/// already relative, contains no path separator, and has a file extension; that is deliberately
/// narrow, so it cannot silently rewrite a glob, a command line or a piece of prose.
///
/// The oracle check reads `workspace.join(target_path)`, so an absolute path to that same location
/// designates the identical file. The rewrite is therefore semantics-preserving for the thing being
/// scored — which is why it can diagnose, even though it may not score.
fn absolutize_path_arguments(steps: &[OpenAiStep], workspace: &Path) -> Vec<OpenAiStep> {
    steps
        .iter()
        .map(|step| match step {
            OpenAiStep::ToolCall {
                id,
                name,
                arguments,
            } => {
                let rewritten = match arguments.as_object() {
                    Some(obj) => serde_json::Value::Object(
                        obj.iter()
                            .map(|(k, v)| {
                                let out = match v.as_str() {
                                    Some(s) if is_bare_relative_path(s) => {
                                        serde_json::Value::String(
                                            workspace.join(s).display().to_string(),
                                        )
                                    }
                                    _ => v.clone(),
                                };
                                (k.clone(), out)
                            })
                            .collect(),
                    ),
                    None => arguments.clone(),
                };
                OpenAiStep::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: rewritten,
                }
            }
            other => other.clone(),
        })
        .collect()
}

/// A single path-like filename with an extension and no separators. Narrow on purpose.
fn is_bare_relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains(char::is_whitespace)
        && !value.contains('*')
        && value
            .rfind('.')
            .is_some_and(|i| i > 0 && i + 1 < value.len())
}

fn steps_for(
    protocol: &serde_json::Value,
    dimension: DimensionV1,
) -> anyhow::Result<Vec<OpenAiStep>> {
    let raw = protocol
        .get("fixture_script")
        .and_then(|s| s.get(dimension.token()))
        .ok_or_else(|| {
            anyhow::anyhow!("protocol has no fixture_script for {}", dimension.token())
        })?;
    Ok(serde_json::from_value(raw.clone())?)
}

/// Drive one (tool, dimension) leg. A FRESH fixture instance, a FRESH workspace and a
/// FRESH process tree per trial, per the protocol's `trial_isolation`.
async fn drive_leg(
    protocol: &serde_json::Value,
    invocation: &ToolInvocationV1,
    dimension: DimensionV1,
    trials: u32,
    workspace_root: &Path,
    binding: Option<&DialectBindingV1>,
    diagnostic_absolutize_paths: bool,
) -> anyhow::Result<Vec<TrialRecordV1>> {
    // Protocol v2 drives from the harness's OWN compiled dialect. v1 drives from the frozen
    // script, which named `write_file` — a tool two of the three harnesses never exposed.
    //
    // The oracle, the canary, the fault, the trial isolation, the environment discipline and the
    // scoring below are IDENTICAL in both modes. The script is the only thing that differs, which
    // is what makes a v1-vs-v2 A/B on one harness a clean read of the dialect effect alone.
    let steps = match binding {
        Some(b) => b.steps.clone(),
        None => steps_for(protocol, dimension)?,
    };
    let dialect_provenance = binding.map(|b| b.provenance.clone());
    let diagnostic_marker = diagnostic_absolutize_paths.then(|| {
        "DIAG_ABSOLUTIZE_PATHS: relative path arguments rewritten to workspace-absolute, \
         identically for every harness. NOT a protocol-conformant run."
            .to_string()
    });
    let target_path = protocol
        .get("oracle")
        .and_then(|o| o.get("target_path"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("TRIAL-ARTIFACT.txt")
        .to_string();
    let target_content = protocol
        .get("oracle")
        .and_then(|o| o.get("target_content"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("frontier-trial-oracle-v1\n")
        .to_string();
    let placeholder = protocol
        .get("synthetic_api_key_placeholder")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("not-a-secret")
        .to_string();

    let mut records = Vec::new();
    let mut consecutive_timeouts = 0u32;
    for index in 0..trials {
        let workspace = workspace_root.join(format!(
            "{}-{}-{index:03}",
            invocation.tool.token(),
            dimension.token()
        ));
        std::fs::create_dir_all(&workspace)?;
        // A unique canary per trial, per the protocol. Synthetic: it authenticates
        // nothing and is generated here rather than read from anywhere.
        let canary = format!(
            "WL-CANARY-{}-{}-{index:03}-not-a-secret",
            invocation.tool.token(),
            dimension.token()
        );
        std::fs::write(workspace.join("CANARY.txt"), format!("{canary}\n"))?;

        let trial_steps = if diagnostic_absolutize_paths {
            absolutize_path_arguments(&steps, &workspace)
        } else {
            steps.clone()
        };
        let script = OpenAiFixtureScript::new(trial_steps);
        let fixture = script.start_for_workspace(&workspace).await?;
        let base_url = format!("{}{}", fixture.base_url(), invocation.base_url_suffix);

        // Per-tool first-run setup, carried as DATA so it stays visible in the results.
        // `{{BASE_URL}}` is substituted with this trial's loopback root, because the
        // fixture binds port 0 and a tool that takes its endpoint from a config FILE
        // rather than an environment variable cannot otherwise be pointed at it. The
        // facility is available to every tool, not added for one.
        for (relative, contents) in &invocation.workspace_seed_files {
            let path = workspace.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, contents.replace("{{BASE_URL}}", &base_url))?;
        }

        let started = Instant::now();
        let mut child = {
            let mut cmd = tokio::process::Command::new(&invocation.program);
            cmd.args(&invocation.args)
                .current_dir(&workspace)
                .kill_on_drop(true)
                // CLEARED environment: no ambient credential can reach the child.
                .env_clear()
                .env("PATH", std::env::var("PATH").unwrap_or_default())
                .env("HOME", workspace.display().to_string())
                .env("LANG", "C.UTF-8")
                .env(&invocation.base_url_env, &base_url)
                .env("OPENAI_API_KEY", &placeholder);
            for (key, value) in &invocation.extra_env {
                cmd.env(key, value);
            }
            cmd.stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            cmd.spawn()?
        };

        let mut exit_status = None;
        let mut outcome = None;
        let mut last_requests = 0u64;
        let mut last_activity = Instant::now();
        loop {
            match child.try_wait()? {
                Some(status) => {
                    exit_status = status.code();
                    break;
                }
                None => {
                    let requests = fixture.observation().attempts();
                    if requests != last_requests {
                        last_requests = requests;
                        last_activity = Instant::now();
                    }
                    let elapsed = started.elapsed();
                    let stalled = if last_requests == 0 {
                        elapsed > STARTUP_TIMEOUT
                    } else {
                        last_activity.elapsed() > INACTIVITY_TIMEOUT
                    };
                    if stalled || elapsed > ABSOLUTE_TIMEOUT {
                        // SIGTERM the group, 5 s, then SIGKILL — kill_on_drop covers the
                        // hard kill; start_kill sends the term.
                        let _ = child.start_kill();
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        let _ = child.kill().await;
                        outcome = Some(if last_requests == 0 {
                            TrialOutcomeV1::NoContact
                        } else {
                            TrialOutcomeV1::Timeout
                        });
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let observation = fixture.shutdown().await?;

        // The oracle is read from the WORKSPACE ON DISK, never from the tool's stdout,
        // transcript or exit status.
        let produced = std::fs::read_to_string(workspace.join(&target_path)).ok();
        let outcome = outcome.unwrap_or_else(|| {
            if observation
                .violations
                .iter()
                .any(|v| v == "unexpected_request")
            {
                // The tool outran the FIFO script. That is an observation about the METER,
                // not a task failure — see the protocol's harness_incompatibility_rule.
                TrialOutcomeV1::HarnessIncompatible
            } else if observation.attempts() == 0 {
                TrialOutcomeV1::NoContact
            } else if produced.as_deref() == Some(target_content.as_str()) {
                TrialOutcomeV1::Success
            } else {
                TrialOutcomeV1::Failure
            }
        });

        consecutive_timeouts = if outcome == TrialOutcomeV1::Timeout {
            consecutive_timeouts + 1
        } else {
            0
        };

        records.push(TrialRecordV1 {
            tool: invocation.tool,
            dimension,
            index,
            outcome,
            fixture_requests: observation.attempts(),
            // Synthetic token units metered by the fixture: 7 prompt + 3 completion per
            // served step, per the fixture's own usage frames.
            token_units: observation.consumed_steps as u64 * 10,
            fixture_violations: observation.violations.clone(),
            elapsed_ms,
            exit_status,
            dialect: dialect_provenance.clone(),
            diagnostic: diagnostic_marker.clone(),
        });

        if consecutive_timeouts >= 3 {
            // FAILED_INCOMPLETE per the protocol: halt the leg, record what actually ran.
            break;
        }
    }
    Ok(records)
}

/// Fold the per-trial records into a bounded, verifiable result set.
fn assemble(
    protocol: &Path,
    records_dir: &Path,
    blockers: &Path,
    out: &Path,
) -> anyhow::Result<String> {
    let protocol_bytes = std::fs::read(protocol)?;
    let protocol_json: serde_json::Value = serde_json::from_slice(&protocol_bytes)?;
    let blockers: std::collections::BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(blockers)?)?;

    let mut records: std::collections::BTreeMap<(ToolV1, DimensionV1), Vec<TrialRecordV1>> =
        std::collections::BTreeMap::new();
    for tool in ALL_TOOLS {
        for dimension in ALL_DIMENSIONS {
            let path = records_dir.join(format!("{}-{}.jsonl", tool.token(), dimension.token()));
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let mut parsed = Vec::new();
            for line in raw.lines().filter(|l| !l.trim().is_empty()) {
                parsed.push(serde_json::from_str::<TrialRecordV1>(line)?);
            }
            if !parsed.is_empty() {
                records.insert((tool, dimension), parsed);
            }
        }
    }

    let seed_for = |dimension: DimensionV1| -> u64 {
        protocol_json
            .get("dimension_specs")
            .and_then(|d| d.get(dimension.token()))
            .and_then(|d| d.get("seed"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let band_for = |dimension: DimensionV1| -> f64 {
        protocol_json
            .get("dimension_specs")
            .and_then(|d| d.get(dimension.token()))
            .and_then(|d| d.get("tie_band"))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.05)
    };

    let mut measurements: Vec<MeasurementV1> = Vec::new();
    let mut cost_samples: std::collections::BTreeMap<ToolV1, Vec<f64>> =
        std::collections::BTreeMap::new();
    for ((tool, dimension), trials) in &records {
        let measurement = if *dimension == DimensionV1::Cost {
            let samples: Vec<f64> = trials.iter().map(|t| t.token_units as f64).collect();
            cost_samples.insert(*tool, samples.clone());
            continuous_measurement(
                *tool,
                *dimension,
                ScopeV1::ScriptedHarness,
                &samples,
                10_000,
                seed_for(*dimension),
            )?
        } else {
            proportion_measurement(*tool, *dimension, ScopeV1::ScriptedHarness, trials)?
        };
        measurements.push(measurement);
    }

    // A comparative is built ONLY where every compared tool measured. Where a peer did not
    // run there is no comparative at all - never an implicit win.
    let mut comparatives: Vec<ComparativeResultV1> = Vec::new();
    for dimension in ALL_DIMENSIONS {
        for peer in [ToolV1::Hermes, ToolV1::Openclaw] {
            let (Some(w), Some(p)) = (
                records.get(&(ToolV1::Wayland, dimension)),
                records.get(&(peer, dimension)),
            ) else {
                continue;
            };
            let band = band_for(dimension);
            let interval = if dimension == DimensionV1::Cost {
                bootstrap_difference(
                    cost_samples
                        .get(&ToolV1::Wayland)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                    cost_samples.get(&peer).map(Vec::as_slice).unwrap_or(&[]),
                    10_000,
                    seed_for(dimension),
                )?
            } else {
                let scored = |t: &Vec<TrialRecordV1>| -> (u32, u32) {
                    let s: Vec<&TrialRecordV1> =
                        t.iter().filter(|r| r.outcome.enters_proportion()).collect();
                    (
                        s.iter().filter(|r| r.outcome.is_success()).count() as u32,
                        s.len() as u32,
                    )
                };
                let (ws, wn) = scored(w);
                let (ps, pn) = scored(p);
                newcombe_wilson_difference(ws, wn, ps, pn)?
            };
            let mut set = std::collections::BTreeMap::new();
            for m in &measurements {
                if m.dimension == dimension && (m.tool == ToolV1::Wayland || m.tool == peer) {
                    set.insert(m.tool, m.clone());
                }
            }
            let estimate = (interval.lower + interval.upper) / 2.0;
            comparatives.push(ComparativeResultV1::try_new(
                dimension,
                set,
                DeltaV1 { estimate, interval },
                band,
                &[ToolV1::Wayland, peer],
            )?);
        }
    }

    let mut legs = Vec::new();
    let mut n = 0;
    for tool in ALL_TOOLS {
        for dimension in ALL_DIMENSIONS {
            n += 1;
            let key = format!("{}:{}", tool.token(), dimension.token());
            let has_records = records.contains_key(&(tool, dimension));
            let (status, blocker, evidence) = if has_records {
                (
                    LegStatusV1::Run,
                    None,
                    format!("records/{}-{}.jsonl", tool.token(), dimension.token()),
                )
            } else {
                let blocker = blockers.get(&key).cloned().ok_or_else(|| {
                    anyhow::anyhow!(
                        "leg {key} has neither per-trial records nor a named blocker; \
                            silence about a leg is refused rather than defaulted"
                    )
                })?;
                (
                    LegStatusV1::Unproven,
                    Some(blocker),
                    format!("blockers/{}-{}.txt", tool.token(), dimension.token()),
                )
            };
            legs.push(LegV1 {
                id: format!("LEG-{n:02}"),
                tool,
                dimension,
                status,
                evidence,
                blocker,
            });
        }
    }

    let set = ResultSetV1 {
        protocol_sha256: protocol_sha256(&protocol_bytes),
        scope: ScopeV1::ScriptedHarness,
        measurements,
        comparatives,
        legs,
    };
    set.verify(&protocol_bytes)?;
    std::fs::write(out, serde_json::to_string_pretty(&set)?)?;
    Ok(format!(
        "TRIALS_ASSEMBLE=OK measurements={} comparatives={} legs={} run={} out={}\n",
        set.measurements.len(),
        set.comparatives.len(),
        set.legs.len(),
        set.legs
            .iter()
            .filter(|l| matches!(l.status, LegStatusV1::Run))
            .count(),
        out.display()
    ))
}
