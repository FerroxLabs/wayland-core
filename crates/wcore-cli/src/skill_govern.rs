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
//! ```text
//!   --skills-promote  <NAME|UUID>   grant a reviewed skill model-facing status
//!   --skills-revoke   <NAME>        retain the bytes, remove it, suppress re-drafting
//!   --skills-rollback <ID>          restore a revoked skill exactly, clear suppression
//!   --skills-govern                 list what is installed, promoted and revoked
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use wcore_skills::govern::{GovernanceStore, JournalEvent, read_signature};
use wcore_skills::promote::{PromoteError, PromotionState};

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
    let grant = store
        .promote_existing(&dir, None, AUTHORITY)
        .map_err(explain)?;
    report_grant(&grant, &dir);
    Ok(())
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

    let grant = store
        .promote_existing(&dir, Some(&id.to_string()), AUTHORITY)
        .map_err(explain)?;

    let mut updated = target.clone();
    updated.status = ProcedureStatus::Active;
    mem.api()
        .upsert_procedure(updated, AccessToken::System)
        .await
        .map_err(|e| anyhow::anyhow!("failed to upsert procedure: {e}"))?;

    report_grant(&grant, &dir);
    println!(
        "  procedure:    '{}' {} → active",
        target.name,
        target.status.as_str()
    );
    Ok(())
}

fn report_grant(grant: &wcore_skills::promote::Promotion, dir: &Path) {
    println!("promoted '{}'", grant.skill_name);
    println!("  artifact:     {}", dir.display());
    println!("  digest:       {}", grant.content_digest);
    println!("  authority:    {}", grant.authority);
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
/// `Display` already says what to do about it, so nothing is added here beyond the
/// conversion.
fn explain(e: PromoteError) -> anyhow::Error {
    match e {
        PromoteError::Refused(r) => anyhow::anyhow!("{r}"),
        PromoteError::Govern(g) => anyhow::anyhow!("{g}"),
    }
}
