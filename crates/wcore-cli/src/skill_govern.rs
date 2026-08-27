//! 23A-C1: the shipped surface for governed skill promotion, revocation and rollback.
//!
//! # Why this is on `wayland-core` and not a side binary
//!
//! `wcore-skills` already carried a working `GovernanceStore` and a
//! `wcore-skill-govern` helper binary. **The release ships exactly one executable** —
//! `release.yml` sets `BINARY_NAME: wayland-core`, every matrix row names it, and the
//! upload glob is `wayland-core-*`. `wcore-skill-govern` appears in no workflow, no
//! packaging script and no manifest, so it is built by developers and delivered to
//! nobody. A capability a customer cannot invoke is not a capability, so the verbs live
//! here, on the binary that is actually installed.
//!
//! The helper binary is retained: it is the harness the revocation tests drive, and
//! deleting a working tool to make a point would cost coverage. It is now a thin peer of
//! this module rather than the only surface.
//!
//! # The four verbs
//!
//! # The evaluation gate (0.12.0 L5, wayland#694)
//!
//! Promotion is scored before it is granted. `wcore_eval::evaluate_skill_dir` reads the
//! artifact's `SKILL.md` and returns a `PromotionEvidence`; `GovernanceStore` refuses
//! unless the score clears the evaluator's threshold. The evidence is a required argument
//! there, so this is not a check this surface can forget to run — it is the only way to
//! call the function at all.
//!
//! **Failing to score is a refusal, not a skip.** A missing or frontmatter-less `SKILL.md`
//! aborts the promotion with the evaluator's error. The alternative — treating "could not
//! evaluate" as "nothing to object to" — is the shape of gate that goes green having
//! examined nothing, and the whole point of L5 is that generated artifacts are data until
//! something looks at them.
//!
//! ```text
//!   --skills-promote  <NAME|UUID>   grant a reviewed skill model-facing status
//!   --skills-revoke   <NAME>        retain the bytes, remove it, suppress re-drafting
//!   --skills-rollback <ID>          restore a revoked skill exactly, clear suppression
//!   --skills-govern                 list what is installed, promoted and revoked
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use wcore_skills::govern::{GovernanceStore, JournalEvent, read_signature};
use wcore_skills::promote::{PromoteError, PromotionEvidence, PromotionState};

/// The authority string recorded on grants issued through this surface.
///
/// Recorded because "on whose authority" is one of the three questions a promotion record
/// has to answer, and the product has no identity system to answer it with a person. What
/// it *can* state truthfully is the surface: an explicit command run by whoever holds the
/// terminal, as opposed to anything automatic. That distinction is the one that matters
/// here, because the defect being closed is a loop that wrote to the user's directory with
/// no user action at all.
const AUTHORITY: &str = "cli:wayland-core --skills-promote";

/// Every directory the loader would treat as a skill, in the user's global skills dirs.
fn skill_dirs() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(d) = wcore_skills::paths::user_skills_dir() {
        roots.push(d);
    }
    roots.extend(wcore_skills::paths::wayland_home_skills_dirs());

    let mut out: Vec<PathBuf> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() && p.join("SKILL.md").is_file() && !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn find_skill(name: &str) -> Option<PathBuf> {
    skill_dirs().into_iter().find(|d| {
        d.file_name()
            .map(|f| f.to_string_lossy() == name)
            .unwrap_or(false)
    })
}

fn store() -> Result<GovernanceStore> {
    GovernanceStore::open_default().context("could not resolve the skill governance root")
}

/// Render one **self-contained** line per skill, every fact on that skill's own line.
///
/// The format is load-bearing, not cosmetic, and the reason is recorded in
/// `wcore-skill-govern`: a rendering that puts skills in one section and their statuses in
/// another cannot be checked except by an unbound pair of substring searches ("mentions
/// the name" AND "mentions revoked"), and that pair is true whenever *any* row is revoked
/// regardless of which. That exact defect was measured in this subsystem — a matcher
/// reported a quarantine tag while holding a model-visible user-authored skill. Binding
/// every fact to one line makes a correct checker writable.
fn print_line(name: &str, status: &str, fields: &[(&str, String)]) {
    let mut line = format!("  {name}  status={status}");
    for (k, v) in fields {
        line.push_str(&format!("  {k}={v}"));
    }
    println!("{line}");
}

/// `--skills-govern`: what is installed, what is promoted, what is revoked.
pub fn run_list() -> Result<()> {
    let store = store()?;
    let dirs = skill_dirs();
    let revoked = store.live_revocations()?;

    println!("governance root: {}", store.root().display());
    println!();
    println!("INSTALLED ({})", dirs.len());
    if dirs.is_empty() {
        println!("  (none)");
    }
    for d in &dirs {
        let name = d.file_name().unwrap_or_default().to_string_lossy();
        let state = store.promotion_state(d)?;
        let status = match &state {
            PromotionState::Promoted(_) => "promoted",
            PromotionState::DigestMismatch { .. } => "quarantined-digest-mismatch",
            PromotionState::NotPromoted => "installed",
        };
        let mut fields = vec![("path", d.display().to_string())];
        match &state {
            PromotionState::Promoted(g) => {
                fields.push(("promotion_id", g.promotion_id.clone()));
                fields.push(("digest", g.content_digest.clone()));
                fields.push(("authority", g.authority.clone()));
                fields.push(("promoted_at", g.promoted_at.clone()));
                if let Some(p) = &g.procedure_id {
                    fields.push(("procedure_id", p.clone()));
                }
            }
            PromotionState::DigestMismatch {
                promotion_id,
                granted,
                found,
            } => {
                fields.push(("promotion_id", promotion_id.clone()));
                fields.push(("granted_digest", granted.clone()));
                fields.push(("found_digest", found.clone()));
            }
            PromotionState::NotPromoted => {}
        }
        if let Some(s) = read_signature(d) {
            fields.push(("signature", s));
        }
        print_line(&name, status, &fields);
    }

    println!();
    println!("REVOKED ({})", revoked.len());
    if revoked.is_empty() {
        println!("  (none)");
    }
    for r in &revoked {
        let mut fields = vec![
            ("id", r.revocation_id.clone()),
            ("revoked_at", r.revoked_at.clone()),
            ("files", r.file_count.to_string()),
            ("bytes", r.byte_count.to_string()),
            ("restores_to", r.source_dir.display().to_string()),
        ];
        if let Some(s) = &r.signature {
            fields.push(("signature", s.clone()));
        }
        print_line(&r.skill_name, "revoked", &fields);
    }

    println!();
    println!("HISTORY");
    let events = store.journal()?;
    if events.is_empty() {
        println!("  (none)");
    }
    for e in events {
        match e {
            JournalEvent::Revoked {
                at,
                skill_name,
                revocation_id,
                ..
            } => println!("  {at}  REVOKED      {skill_name}  id={revocation_id}"),
            JournalEvent::RolledBack {
                at,
                skill_name,
                revocation_id,
                ..
            } => println!("  {at}  ROLLED-BACK  {skill_name}  id={revocation_id}"),
            JournalEvent::DraftSuppressed { at, skill_name, .. } => {
                println!("  {at}  SUPPRESSED   {skill_name}")
            }
            JournalEvent::Promoted {
                at,
                skill_name,
                promotion_id,
                content_digest,
                authority,
                ..
            } => println!(
                "  {at}  PROMOTED     {skill_name}  id={promotion_id}  digest={content_digest}  authority={authority}"
            ),
            JournalEvent::PromotionRefused {
                at,
                skill_name,
                reason,
            } => println!("  {at}  REFUSED      {skill_name}  reason={reason}"),
            JournalEvent::PromotionWithdrawn {
                at,
                skill_name,
                promotion_id,
                reason,
            } => println!("  {at}  WITHDRAWN    {skill_name}  id={promotion_id}  reason={reason}"),
        }
    }
    Ok(())
}

/// `--skills-revoke <NAME>`.
pub fn run_revoke(name: &str) -> Result<()> {
    let store = store()?;
    let dir = find_skill(name)
        .with_context(|| format!("no skill named '{name}' is installed. Run --skills-govern."))?;
    let rec = store.revoke(&dir)?;

    println!("revoked '{}'", rec.skill_name);
    println!("  removed from: {}", rec.source_dir.display());
    println!(
        "  retained:     {} file(s), {} byte(s)",
        rec.file_count, rec.byte_count
    );
    if let Some(sig) = &rec.signature {
        println!("  signature:    {sig}");
    }
    println!("  revocation id: {}", rec.revocation_id);
    println!();
    println!("It will not load, will not execute, and will not be re-drafted.");
    println!(
        "To undo:  wayland-core --skills-rollback {}",
        rec.revocation_id
    );
    Ok(())
}

/// `--skills-rollback <REVOCATION_ID>`.
pub fn run_rollback(id: &str) -> Result<()> {
    let store = store()?;
    let restored = store.rollback(id)?;
    println!("rolled back {id}");
    println!("  restored to: {}", restored.display());
    println!("  the skill loads again and the drafter is no longer suppressed for it");
    println!();
    println!("It is restored **quarantined**, not promoted: rollback returns the exact prior");
    println!("bytes, and the prior state of an auto-drafted skill is not model-facing.");
    Ok(())
}

/// `--skills-promote <NAME|PROCEDURE_ID>`.
///
/// Accepts either form. A UUID is resolved through the P4 procedure table — which is what
/// anyone who scripted the historical flag passed — and anything else is taken as the name
/// of an installed skill, which is what a person reading `--skills-govern` output has in
/// front of them.
pub async fn run_promote(arg: &str) -> Result<()> {
    match uuid::Uuid::parse_str(arg) {
        Ok(uuid) => promote_procedure(uuid).await,
        Err(_) => promote_named(arg),
    }
}

fn promote_named(name: &str) -> Result<()> {
    let store = store()?;
    let dir = find_skill(name)
        .with_context(|| format!("no skill named '{name}' is installed. Run --skills-govern."))?;
    let (evidence, why) = evaluate(&dir);
    let grant = store
        .promote_existing(&dir, None, AUTHORITY, &evidence)
        .map_err(|e| explain(e, why))?;
    report_grant(&grant, &dir);
    Ok(())
}

/// Score the artifact, and report the score whichever way it goes.
///
/// The breakdown is printed on the way past rather than only on refusal. A number that is
/// only ever shown when it blocks something reads as an obstacle; shown on every promotion
/// it is what the grant actually rests on, and the operator can see how much headroom a
/// draft had before the next edit changes its bytes.
///
/// **A failure to score returns failing evidence rather than aborting here**, and the
/// distinction is not cosmetic. Aborting would put "we could not parse it" ahead of
/// `promote_existing`'s own refusals, and the first of those is the revocation fence — a
/// standing user decision, which should not be pre-empted by a parse problem. The artifact
/// is still unpromotable: `unscorable_evidence` scores 0.0 against the real cutoff, so it
/// fails the gate by construction rather than by anyone remembering to check. The original
/// error is carried alongside and surfaced iff the gate is what actually refused.
fn evaluate(dir: &Path) -> (PromotionEvidence, Option<anyhow::Error>) {
    let name = dir.file_name().unwrap_or_default().to_string_lossy();
    match wcore_eval::evaluate_skill_dir(dir) {
        Ok(gate) => {
            let d = gate.outcome.dimensions;
            println!(
                "evaluated '{name}': score {:.3} (threshold {:.3}) -- {}",
                gate.evidence.score, gate.evidence.threshold, gate.evidence.verdict,
            );
            println!(
                "  outcome {:.3} | cost penalty {:.3} | size penalty {:.3}",
                d.outcome, d.cost_penalty, d.size_penalty
            );
            (gate.evidence, None)
        }
        Err(e) => {
            let why = anyhow::Error::from(e).context(format!(
                "cannot evaluate {} for promotion. This is a refusal, not a skip: nothing is \
                 promoted on the strength of an evaluation that did not happen.",
                dir.display()
            ));
            (wcore_eval::unscorable_evidence(), Some(why))
        }
    }
}

/// Promote through a reviewed P4 procedure.
///
/// Two steps that must agree, and the order matters. The filesystem grant is written
/// **first**: it is the thing that changes what the model can reach, so if the run dies
/// between the two the visible outcome is a promoted artifact whose procedure row still
/// reads `Staged` — recoverable by re-running. The reverse order would mark the procedure
/// `Active` with nothing promoted, which reads as done and is not.
async fn promote_procedure(id: uuid::Uuid) -> Result<()> {
    use wcore_memory::v2_types::{AccessToken, ProcedureId, ProcedureStatus, Tier};

    let cwd = std::env::current_dir()?;
    let mem = wcore_memory::Memory::open(&cwd, "cli-skills-cmd")
        .await
        .map_err(|e| anyhow::anyhow!("failed to open project memory: {e}"))?;
    let procs = mem
        .api()
        .list_procedures(Tier::Project, AccessToken::System)
        .await
        .map_err(|e| anyhow::anyhow!("failed to list procedures: {e}"))?;
    let target = procs
        .into_iter()
        .find(|p| p.id == ProcedureId(id))
        .with_context(|| format!("no procedure with id '{id}' found at Tier::Project"))?;

    if target.status == ProcedureStatus::Revoked {
        anyhow::bail!(
            "cannot promote procedure '{}' (id={id}): it is revoked. A revoked \
             procedure is a standing user decision; promotion does not override it.",
            target.name
        );
    }
    if !target.status.can_transition_to(ProcedureStatus::Active) {
        anyhow::bail!(
            "cannot promote procedure '{}' (id={id}): {} → active is not a valid transition",
            target.name,
            target.status.as_str()
        );
    }

    let store = store()?;
    let dir = find_skill(&target.name).with_context(|| {
        format!(
            "procedure '{}' (id={id}) has no installed artifact at that name, so there is \
             nothing to promote. Governed promotion binds one reviewed procedure to one \
             artifact on disk; it does not invent the artifact.",
            target.name
        )
    })?;

    let (evidence, why) = evaluate(&dir);
    let grant = store
        .promote_existing(&dir, Some(&id.to_string()), AUTHORITY, &evidence)
        .map_err(|e| explain(e, why))?;

    let mut updated = target.clone();
    updated.status = ProcedureStatus::Active;
    mem.api()
        .upsert_procedure(updated, AccessToken::System)
        .await
        .map_err(|e| anyhow::anyhow!("failed to upsert procedure: {e}"))?;

    record_promotion_provenance(mem.api(), &target, &grant).await;

    report_grant(&grant, &dir);
    println!(
        "  procedure:    '{}' {} → active",
        target.name,
        target.status.as_str()
    );
    Ok(())
}

/// Write the promotion into the knowledge graph.
///
/// The grant on disk already records what was promoted and against what score. This is the
/// same fact where the *procedure* lives, so a later reader of the memory store — the
/// curator, a `dream` cycle, anything asking "why is this skill active" — finds the answer
/// beside the row rather than having to know that a governance directory exists.
///
/// Best-effort by design. The grant is the authority and it is already durable; failing to
/// annotate the graph must not turn a completed promotion into an error, because a retry
/// would then hit `AlreadyPromoted` and look like a different failure.
async fn record_promotion_provenance(
    api: &dyn wcore_memory::MemoryApi,
    procedure: &wcore_memory::v2_types::Procedure,
    grant: &wcore_skills::promote::Promotion,
) {
    use wcore_memory::v2_types::{Episode, EpisodeId, EpisodeStatus, Source, Tier};

    let (score, threshold, evaluator) = match &grant.evidence {
        Some(e) => (e.score, e.threshold, e.evaluator.as_str()),
        // Unreachable through this path -- `promote_existing` writes `Some` -- but stating
        // the absence beats fabricating a zero.
        None => (f64::NAN, f64::NAN, "unrecorded"),
    };
    let episode = Episode {
        id: EpisodeId::new(),
        tier: Tier::Project,
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        episode_type: "skill_promoted".to_string(),
        summary: format!(
            "promoted skill '{}' (procedure {}) to active: {evaluator} scored {score:.3} \
             against a {threshold:.3} threshold",
            procedure.name, procedure.id.0
        ),
        atomic_facts: vec![
            format!("promotion_id={}", grant.promotion_id),
            format!("skill_name={}", grant.skill_name),
            format!("procedure_id={}", procedure.id.0),
            format!("content_digest={}", grant.content_digest),
            format!("authority={}", grant.authority),
            format!("evaluator={evaluator}"),
            format!("eval_score={score:.6}"),
            format!("eval_threshold={threshold:.6}"),
        ],
        source: Source::User.as_str(),
        source_product: "wcore-cli".to_string(),
        session_id: None,
        project_root: None,
        decay_score: 1.0,
        status: EpisodeStatus::Active,
    };
    if let Err(e) = api
        .record_episode(episode, wcore_memory::AccessToken::System)
        .await
    {
        tracing::warn!(
            target: "wcore_cli::skill_govern",
            error = %e,
            promotion_id = %grant.promotion_id,
            "promotion succeeded but its provenance episode could not be recorded"
        );
    }
}

fn report_grant(grant: &wcore_skills::promote::Promotion, dir: &Path) {
    println!("promoted '{}'", grant.skill_name);
    println!("  artifact:     {}", dir.display());
    println!("  digest:       {}", grant.content_digest);
    println!("  authority:    {}", grant.authority);
    match &grant.evidence {
        Some(e) => println!(
            "  evaluation:   {} scored {:.3} (threshold {:.3})",
            e.evaluator, e.score, e.threshold
        ),
        None => println!("  evaluation:   none recorded"),
    }
    println!("  promotion id: {}", grant.promotion_id);
    println!(
        "  covers:       {} file(s), {} byte(s)",
        grant.file_count, grant.byte_count
    );
    println!();
    println!("The grant is bound to these exact bytes. Editing the skill returns it to");
    println!("quarantine until it is promoted again — promotion never covers unreviewed edits.");
}

/// Surface a refusal as a plain error. A `Refusal` is a governance decision and its
/// `Display` already says what to do about it, so little is added here beyond the
/// conversion.
///
/// The exception is `EvalBelowThreshold` when the artifact could not be scored at all. The
/// generic wording ("scored it 0.000") would be true and useless; `why` carries the reason
/// the evaluator could not produce a number, which is the only thing the operator can act
/// on. Every other refusal wins over it, which is the point of deferring to here.
fn explain(e: PromoteError, why: Option<anyhow::Error>) -> anyhow::Error {
    let gate_refused = matches!(
        e,
        PromoteError::Refused(wcore_skills::promote::Refusal::EvalBelowThreshold { .. })
    );
    match (gate_refused, why) {
        (true, Some(w)) => w,
        _ => match e {
            PromoteError::Refused(r) => anyhow::anyhow!("{r}"),
            PromoteError::Govern(g) => anyhow::anyhow!("{g}"),
        },
    }
}
