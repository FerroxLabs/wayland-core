//! `wcore-skill-govern` — the user-facing surface for 23A-C1 skill revocation and rollback.
//!
//! # Why this is a separate binary
//!
//! The natural home for these verbs is `wayland-core --skills-revoke` / `--skills-rollback`
//! alongside the existing `--skills-audit` / `--skills-archive`. `crates/wcore-cli/src/main.rs`
//! is fenced for concurrent work, so the flags cannot land here; a seam request covers that
//! wiring. Crate-level binaries are an established pattern in this workspace
//! (`wcore-eval`, `wcore-evolve`, `wcore-contract`), so this ships the capability now rather
//! than leaving it library-only and unusable, and the later flag becomes a thin delegation.
//!
//! Deliberately argv-parsed by hand rather than pulling `clap` into `wcore-skills`: the
//! surface is four verbs, and a new dependency on a mid-layer crate is a worse trade.
//!
//! # Verbs
//!
//! ```text
//!   list                        every drafted skill on disk, and whether it is revoked
//!   revoke <name>               retain the bytes, then remove it, and suppress re-drafting
//!   rollback <revocation-id>    restore the exact bytes and clear the suppression
//!   history                     the append-only journal
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use wcore_skills::govern::{GovernanceStore, JournalEvent, read_signature};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verb = args.first().map(String::as_str).unwrap_or("help");

    let store = match GovernanceStore::open_default() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let result = match verb {
        "list" => cmd_list(&store),
        "revoke" => match args.get(1) {
            Some(name) => cmd_revoke(&store, name),
            None => Err("usage: wcore-skill-govern revoke <skill-name>".to_string()),
        },
        "rollback" => match args.get(1) {
            Some(id) => cmd_rollback(&store, id),
            None => Err("usage: wcore-skill-govern rollback <revocation-id>".to_string()),
        },
        "history" => cmd_history(&store),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => Err(format!(
            "unknown command '{other}'. Run `wcore-skill-govern help`."
        )),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!(
        "wcore-skill-govern — revoke and roll back auto-drafted skills\n\
         \n\
         USAGE:\n\
         \x20 wcore-skill-govern list                     show drafted skills and their status\n\
         \x20 wcore-skill-govern revoke <skill-name>      remove a draft and stop it returning\n\
         \x20 wcore-skill-govern rollback <revocation-id> restore a revoked draft exactly\n\
         \x20 wcore-skill-govern history                  show the append-only journal\n\
         \n\
         Revoking retains every byte first, so `rollback` always restores the exact prior\n\
         state. Nothing is ever deleted without a retained copy.\n"
    );
}

/// Every directory the loader would treat as a skill, in the user's global skills dir.
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

/// Is this directory an auto-drafted skill? Mirrors the loader's manifest check.
fn is_auto_drafted(dir: &Path) -> bool {
    std::fs::read(dir.join("manifest.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("auto_drafted").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

/// Render one **self-contained** line per skill, each carrying its own status.
///
/// The line format is deliberate, not cosmetic. The obvious rendering — skills in one
/// section and revocations in another — cannot be checked without an unbound pair of
/// substring searches ("output mentions the name AND mentions revoked"), and that pair is
/// true whenever *any* row is revoked, regardless of which. A repaired matcher in
/// `wcore-eval-scenarios/tests/f23a_boundary_drive.rs` demonstrated exactly this defect in
/// this subsystem: on unchanged bytes the unbound form reported a quarantine tag while
/// holding a model-visible, user-authored skill.
///
/// So every fact about a skill is emitted on that skill's own line, as `key=value` fields
/// led by the name. A checker binds to one line and reads that line's own `status=`, and
/// cannot borrow a neighbouring row's status. `status_field` in `tests/govern_cli_drive.rs`
/// is the matcher this format exists to make writable, with the three-assertion self-test.
fn print_skill_line(name: &str, status: &str, fields: &[(&str, String)]) {
    let mut line = format!("  {name}  status={status}");
    for (k, v) in fields {
        line.push_str(&format!("  {k}={v}"));
    }
    println!("{line}");
}

fn cmd_list(store: &GovernanceStore) -> Result<(), String> {
    let dirs = skill_dirs();
    let live = store.live_revocations().map_err(|e| e.to_string())?;

    println!("governance root: {}", store.root().display());
    println!();
    println!("ON DISK ({})", dirs.len());
    if dirs.is_empty() {
        println!("  (none)");
    }
    for d in &dirs {
        let name = d.file_name().unwrap_or_default().to_string_lossy();
        let kind = if is_auto_drafted(d) {
            "auto-drafted"
        } else {
            "user-authored"
        };
        let mut fields = vec![
            ("kind", kind.to_string()),
            ("path", d.display().to_string()),
        ];
        if let Some(s) = read_signature(d) {
            fields.push(("signature", s));
        }
        print_skill_line(&name, "present", &fields);
    }

    println!();
    println!("REVOKED ({})", live.len());
    if live.is_empty() {
        println!("  (none)");
    }
    for r in &live {
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
        print_skill_line(&r.skill_name, "revoked", &fields);
    }
    Ok(())
}

fn cmd_revoke(store: &GovernanceStore, name: &str) -> Result<(), String> {
    let dir = skill_dirs()
        .into_iter()
        .find(|d| {
            d.file_name()
                .map(|f| f.to_string_lossy() == name)
                .unwrap_or(false)
        })
        .ok_or_else(|| {
            format!("no skill named '{name}' on disk. Run `wcore-skill-govern list`.")
        })?;

    let rec = store.revoke(&dir).map_err(|e| e.to_string())?;
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
    println!("This skill will NOT be re-drafted. To undo:");
    println!("  wcore-skill-govern rollback {}", rec.revocation_id);
    Ok(())
}

fn cmd_rollback(store: &GovernanceStore, id: &str) -> Result<(), String> {
    let restored = store.rollback(id).map_err(|e| e.to_string())?;
    println!("rolled back {id}");
    println!("  restored to: {}", restored.display());
    println!("  the drafter is no longer suppressed for this skill");
    Ok(())
}

fn cmd_history(store: &GovernanceStore) -> Result<(), String> {
    let events = store.journal().map_err(|e| e.to_string())?;
    if events.is_empty() {
        println!("no governance history at {}", store.root().display());
        return Ok(());
    }
    for e in events {
        match e {
            JournalEvent::Revoked {
                at,
                skill_name,
                revocation_id,
                ..
            } => println!("{at}  REVOKED         {skill_name}  (id {revocation_id})"),
            JournalEvent::RolledBack {
                at,
                skill_name,
                revocation_id,
                ..
            } => println!("{at}  ROLLED-BACK     {skill_name}  (id {revocation_id})"),
            JournalEvent::DraftSuppressed { at, skill_name, .. } => {
                println!("{at}  DRAFT-SUPPRESSED {skill_name}")
            }
            // 23A-C1 promotion events. Rendered here too rather than wildcarded: this
            // binary's `history` is advertised as "the append-only journal", and a
            // renderer that silently dropped three of six event kinds would make the
            // promotion record look empty to anyone auditing through this surface --
            // the same shape of defect as a governance read that fails open.
            JournalEvent::Promoted {
                at,
                skill_name,
                promotion_id,
                content_digest,
                ..
            } => println!(
                "{at}  PROMOTED        {skill_name}  (id {promotion_id}, digest {content_digest})"
            ),
            JournalEvent::PromotionRefused {
                at,
                skill_name,
                reason,
            } => println!("{at}  PROMOTE-REFUSED {skill_name}  ({reason})"),
            JournalEvent::PromotionWithdrawn {
                at,
                skill_name,
                promotion_id,
                reason,
            } => println!(
                "{at}  PROMOTE-WITHDRAWN {skill_name}  (id {promotion_id}, {reason})"
            ),
        }
    }
    Ok(())
}
