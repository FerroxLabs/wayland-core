use std::sync::Arc;

use wcore_skills::govern::{GovernanceStore, JournalEvent};
use wcore_skills::refs::{SkillCatalog, SkillRef};
use wcore_skills::types::SkillSource;

use super::{SlashError, SlashHandler, SlashInvocation, SlashOutcome};

/// `/skill` handler. Two variants:
///
/// - [`SkillHandler::Stub`] returns the v0.7.0 placeholder strings.
/// - [`SkillHandler::Runtime`] enumerates the session's resolved
///   [`SkillCatalog`] — the same catalog that backs the model's
///   `SkillTool`. `show` / `run` are intentionally read-only here:
///   "run" defers to the normal `SkillTool` tool-call path (the model
///   does the dispatch via tool call, not via slash-command), so the
///   handler explains the workflow rather than fabricating a fake
///   execution channel.
///
/// # Governance (23A SC-1 clauses b, c, d)
///
/// `govern` / `revoke` / `rollback` put `wcore_skills::govern` on a surface an
/// operator can actually reach. The module has been complete since `460fad3b`,
/// but its only caller was `wcore-skill-govern`, a dev-only auto-discovered bin
/// that appears in no packaging manifest and no release workflow — so a user who
/// installed `wayland-core` could see *that* a generated skill was quarantined and
/// could do nothing about it, and could not see what had already been revoked.
///
/// This lands on `/skill` rather than on new `wayland-core --skills-*` flags
/// deliberately. `wcore-cli/src/main.rs` is the shared-fence file every lane in this
/// programme edits, and an unmerged lane already adds those exact four flags there;
/// a second copy would conflict on the one file that most needs to stay quiet. The
/// slash surface is also where clause (b) is already proven — `/skill list` and
/// `/skill show` are the phase's live-driven observation routes — and it works in
/// the TUI as well as the shipped binary.
///
/// The store is resolved per invocation via
/// [`GovernanceStore::open_default`], which honours `WAYLAND_SKILLS_GOVERNANCE_DIR`
/// and `WAYLAND_HOME`, so governance state stays inside whatever sandbox the rest of
/// the engine is running in. The rendering functions take an explicit store so tests
/// need no process-global environment mutation.
#[derive(Clone, Default)]
pub enum SkillHandler {
    #[default]
    Stub,
    Runtime {
        catalog: Arc<SkillCatalog>,
    },
}

impl std::fmt::Debug for SkillHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stub => f.debug_struct("SkillHandler::Stub").finish(),
            Self::Runtime { catalog } => f
                .debug_struct("SkillHandler::Runtime")
                .field("catalog_len", &catalog.len())
                .finish(),
        }
    }
}

impl SlashHandler for SkillHandler {
    fn name(&self) -> &str {
        "skill"
    }
    fn one_line_help(&self) -> &str {
        "List / show / run a skill; govern, revoke or roll back a generated one."
    }
    fn invoke(&self, invocation: &SlashInvocation) -> Result<SlashOutcome, SlashError> {
        match invocation.args.split_first() {
            None => self.list(),
            Some((first, rest)) => match first.as_str() {
                "list" => self.list(),
                "show" => {
                    let name = rest
                        .first()
                        .ok_or_else(|| SlashError::Bad("/skill show <name>".to_string()))?;
                    self.show(name)
                }
                "run" => {
                    let name = rest.first().ok_or_else(|| {
                        SlashError::Bad("/skill run <name> [...args]".to_string())
                    })?;
                    self.run(name)
                }
                "govern" => self.govern(),
                "revoke" => {
                    let name = rest
                        .first()
                        .ok_or_else(|| SlashError::Bad("/skill revoke <name>".to_string()))?;
                    self.revoke(name)
                }
                "rollback" => {
                    let id = rest.first().ok_or_else(|| {
                        SlashError::Bad(
                            "/skill rollback <revocation-id>  (ids come from `/skill govern`)"
                                .to_string(),
                        )
                    })?;
                    self.rollback(id)
                }
                other => Err(SlashError::Bad(format!(
                    "/skill: unknown sub-action '{other}'. Try: list | show <name> | \
                     run <name> | govern | revoke <name> | rollback <id>"
                ))),
            },
        }
    }
}

impl SkillHandler {
    fn list(&self) -> Result<SlashOutcome, SlashError> {
        match self {
            Self::Stub => Ok(SlashOutcome::Handled {
                output: Some(
                    "/skill list: full skill inventory needs the runtime \
                     SkillRegistry handle; use `wayland-core --skills-audit` \
                     from the CLI in v0.7.0."
                        .to_string(),
                ),
            }),
            Self::Runtime { catalog } => Ok(SlashOutcome::Handled {
                output: Some(runtime_list(catalog)),
            }),
        }
    }

    fn show(&self, name: &str) -> Result<SlashOutcome, SlashError> {
        match self {
            Self::Stub => Ok(SlashOutcome::Handled {
                output: Some(format!(
                    "use `wayland-core --skills-audit` then grep for '{name}' in v0.7.0"
                )),
            }),
            Self::Runtime { catalog } => Ok(SlashOutcome::Handled {
                output: Some(runtime_show(catalog, name)),
            }),
        }
    }

    fn run(&self, name: &str) -> Result<SlashOutcome, SlashError> {
        match self {
            Self::Stub => Ok(SlashOutcome::Handled {
                output: Some(format!(
                    "/skill run '{name}': runtime dispatch wired in 3.C.4 alongside the TUI."
                )),
            }),
            Self::Runtime { catalog } => {
                // The agent dispatches skills via SkillTool tool calls, not via
                // a direct slash-command path: that's the contract SkillTool was
                // wired against (catalog → tool dispatch → execution + procedural
                // telemetry). Calling out from a slash handler would bypass the
                // approval pipeline + the procedural-memory recording.
                // Instead, validate the skill exists and instruct the user.
                match catalog.find(name) {
                    Some(skill) if skill.disable_model_invocation => Ok(SlashOutcome::Handled {
                        output: Some(format!(
                            "/skill run '{name}': this skill is quarantined and cannot be run."
                        )),
                    }),
                    Some(_) => Ok(SlashOutcome::Handled {
                        output: Some(format!(
                            "/skill run '{name}': skill exists in the catalog. \
                             Skill dispatch flows through the agent's SkillTool — \
                             ask the agent to use the skill (e.g. \"use the {name} skill\") \
                             so the request goes through the approval + telemetry pipeline."
                        )),
                    }),
                    None => Ok(SlashOutcome::Handled {
                        output: Some(format!(
                            "/skill run '{name}': no skill named '{name}' in the catalog. \
                             Run `/skill list` to see available skills."
                        )),
                    }),
                }
            }
        }
    }

    /// Clause (b), governance half: what has been revoked, when, and what is retained.
    ///
    /// Works in `Stub` mode too. Governance state is on disk, not in the session's
    /// catalog, so there is no runtime handle to be missing — and an operator whose
    /// session came up without a catalog is exactly the one who may need to see what
    /// the drafter did to their skills directory.
    fn govern(&self) -> Result<SlashOutcome, SlashError> {
        Ok(SlashOutcome::Handled {
            output: Some(match GovernanceStore::open_default() {
                Ok(store) => render_govern(&store),
                Err(e) => format!("/skill govern: {e}"),
            }),
        })
    }

    /// Clause (c): revoke a generated skill through the shipped surface.
    fn revoke(&self, name: &str) -> Result<SlashOutcome, SlashError> {
        let catalog = match self {
            Self::Stub => {
                return Ok(SlashOutcome::Handled {
                    output: Some(format!(
                        "/skill revoke '{name}': needs the session's resolved skill catalog \
                         to locate the skill on disk, and this session has none."
                    )),
                });
            }
            Self::Runtime { catalog } => catalog,
        };
        let store = match GovernanceStore::open_default() {
            Ok(s) => s,
            Err(e) => {
                return Ok(SlashOutcome::Handled {
                    output: Some(format!("/skill revoke '{name}': {e}")),
                });
            }
        };
        Ok(SlashOutcome::Handled {
            output: Some(revoke_named(catalog, &store, name)),
        })
    }

    /// Clause (d): restore a revoked skill, byte for byte, atomically.
    fn rollback(&self, id: &str) -> Result<SlashOutcome, SlashError> {
        Ok(SlashOutcome::Handled {
            output: Some(match GovernanceStore::open_default() {
                Ok(store) => rollback_id(&store, id),
                Err(e) => format!("/skill rollback '{id}': {e}"),
            }),
        })
    }
}

/// Where a skill's revocable directory is, or why it has none.
///
/// Revocation moves bytes in a directory the user owns. Bundled and MCP skills have no
/// such directory — bundled bodies come from `inline_content` and MCP ones from a
/// protocol peer — so revoking them would either fail confusingly or, worse, delete
/// something inside the installed product. Refuse by provenance, and say which.
fn revocable_dir(r: &SkillRef) -> Result<std::path::PathBuf, String> {
    match r.source {
        SkillSource::Bundled | SkillSource::Mcp => {
            return Err(format!(
                "'{}' is a {:?} skill, not a generated one: it has no directory in your \
                 skills tree to revoke. Governance covers skills the product wrote into \
                 a directory you own.",
                r.name, r.source
            ));
        }
        SkillSource::User | SkillSource::Project | SkillSource::Managed | SkillSource::Legacy => {}
    }
    if r.inline_content.is_some() {
        return Err(format!(
            "'{}' has an inline body and no on-disk directory to revoke.",
            r.name
        ));
    }
    let dir = r
        .skill_root
        .clone()
        .or_else(|| r.file_path.parent().map(|p| p.to_path_buf()))
        .ok_or_else(|| format!("'{}' has no resolvable directory on disk.", r.name))?;
    if !dir.is_dir() {
        return Err(format!(
            "'{}' resolves to {}, which is not a directory on disk.",
            r.name,
            dir.display()
        ));
    }
    Ok(dir)
}

fn revoke_named(catalog: &SkillCatalog, store: &GovernanceStore, name: &str) -> String {
    let Some(r) = catalog.find(name) else {
        return format!(
            "/skill revoke '{name}': no skill named '{name}' in the catalog. \
             Run `/skill list` to see available skills."
        );
    };
    let dir = match revocable_dir(&r) {
        Ok(d) => d,
        Err(why) => return format!("/skill revoke '{name}': {why}"),
    };
    match store.revoke(&dir) {
        Ok(rec) => format!(
            "Revoked '{name}'.\n  \
             revocation_id: {id}\n  \
             removed_from:  {from}\n  \
             retained:      {files} file(s), {bytes} byte(s) in {root}\n  \
             suppressed:    the drafter will not recreate this skill\n\n\
             Undo with `/skill rollback {id}`. This session's catalog is now stale for \
             '{name}' — restart to reload.",
            id = rec.revocation_id,
            from = rec.source_dir.display(),
            files = rec.file_count,
            bytes = rec.byte_count,
            root = store.root().display(),
        ),
        Err(e) => format!("/skill revoke '{name}': failed — {e}"),
    }
}

fn rollback_id(store: &GovernanceStore, id: &str) -> String {
    match store.rollback(id) {
        Ok(path) => format!(
            "Rolled back '{id}'.\n  \
             restored_to: {p}\n  \
             suppression: cleared — the drafter may produce this skill again\n\n\
             The restore was staged and published with a single rename, so it was never \
             partially applied. Restart to reload the catalog.",
            p = path.display()
        ),
        Err(e) => format!("/skill rollback '{id}': failed — {e}"),
    }
}

fn render_govern(store: &GovernanceStore) -> String {
    let mut out = format!("Skill governance ({})\n\n", store.root().display());

    match store.live_revocations() {
        Err(e) => out.push_str(&format!("Live revocations: unreadable — {e}\n")),
        Ok(live) if live.is_empty() => {
            out.push_str("Live revocations: none. No skill is currently suppressed.\n");
        }
        Ok(live) => {
            out.push_str(&format!("Live revocations ({}):\n", live.len()));
            for r in &live {
                out.push_str(&format!(
                    "  - {name}\n      id:        {id}\n      revoked:   {at}\n      \
                     was at:    {dir}\n      signature: {sig}\n      retained:  {f} file(s), \
                     {b} byte(s)\n",
                    name = r.skill_name,
                    id = r.revocation_id,
                    at = r.revoked_at,
                    dir = r.source_dir.display(),
                    sig = r.signature.as_deref().unwrap_or("(manifest unreadable)"),
                    f = r.file_count,
                    b = r.byte_count,
                ));
            }
            out.push_str("\nUndo any of these with `/skill rollback <id>`.\n");
        }
    }

    out.push('\n');
    match store.journal() {
        Err(e) => out.push_str(&format!("History: unreadable — {e}\n")),
        Ok(events) if events.is_empty() => {
            out.push_str("History: empty. Governance has taken no action on this machine.\n");
        }
        Ok(events) => {
            // The journal is append-only and never truncated, so it outgrows a terminal.
            // Show the tail and say what was elided, rather than silently showing a
            // window an operator could mistake for the whole record.
            const TAIL: usize = 20;
            let shown = events.len().min(TAIL);
            out.push_str(&format!(
                "History ({} event(s), append-only, showing last {shown}):\n",
                events.len()
            ));
            for e in events.iter().skip(events.len() - shown) {
                out.push_str(&format!("  {}\n", render_event(e)));
            }
            if events.len() > shown {
                out.push_str(&format!(
                    "  ... {} earlier event(s) in {}/journal.jsonl\n",
                    events.len() - shown,
                    store.root().display()
                ));
            }
        }
    }
    out
}

fn render_event(e: &JournalEvent) -> String {
    match e {
        JournalEvent::Revoked {
            at,
            skill_name,
            revocation_id,
            ..
        } => format!("{at}  revoked        {skill_name}  (id {revocation_id})"),
        JournalEvent::RolledBack {
            at,
            skill_name,
            restored_to,
            revocation_id,
        } => format!(
            "{at}  rolled-back    {skill_name}  (id {revocation_id}) -> {}",
            restored_to.display()
        ),
        // The evidence that a revocation is doing continuing work, rather than having
        // deleted something once. Without these lines "the draft did not come back" is
        // indistinguishable from "the drafter never fired".
        JournalEvent::DraftSuppressed {
            at,
            skill_name,
            revocation_id,
            ..
        } => format!("{at}  draft-refused  {skill_name}  (id {revocation_id})"),

        // Promotion side of the journal. Rendered here rather than filtered out: an operator
        // asking "what has governance done to my skills" needs the grants as much as the
        // revocations, and a promotion is the event that makes a generated skill
        // model-visible. The digest is shown because the grant is bound to bytes, not to a
        // name -- without it the record cannot answer "which version was approved".
        JournalEvent::Promoted {
            at,
            skill_name,
            promotion_id,
            content_digest,
            authority,
            ..
        } => format!(
            "{at}  promoted       {skill_name}  (id {promotion_id}) by {authority}  \
             digest {digest}",
            digest = short_digest(content_digest)
        ),
        // A refusal is governance working. An unlogged refusal is indistinguishable from a
        // promotion nobody attempted, so it gets a line of its own.
        JournalEvent::PromotionRefused {
            at,
            skill_name,
            reason,
        } => format!("{at}  promo-refused  {skill_name}  — {reason}"),
        JournalEvent::PromotionWithdrawn {
            at,
            skill_name,
            promotion_id,
            reason,
        } => format!("{at}  promo-withdrawn {skill_name}  (id {promotion_id}) — {reason}"),
    }
}

/// Shorten a `sha256:<64 hex>` digest for a terminal line, keeping the algorithm prefix so a
/// reader can tell a truncated digest from a short one. Anything unrecognised is passed
/// through untouched rather than blindly sliced, which would panic on a multi-byte boundary.
fn short_digest(d: &str) -> String {
    match d.split_once(':') {
        Some((algo, hex)) if hex.len() > 12 && hex.is_char_boundary(12) => {
            format!("{algo}:{}…", &hex[..12])
        }
        _ => d.to_string(),
    }
}

fn runtime_list(catalog: &SkillCatalog) -> String {
    if catalog.is_empty() {
        return "/skill list: no skills loaded in this session.".to_string();
    }
    let mut out = format!("Skills in catalog ({}):\n", catalog.len());
    let mut visible = 0usize;
    let mut hidden = 0usize;
    for r in catalog.refs() {
        let tag = if r.disable_model_invocation {
            hidden += 1;
            "(hidden)"
        } else {
            visible += 1;
            ""
        };
        let src = format!("{:?}", r.source).to_lowercase();
        out.push_str(&format!(
            "  - {name}{tag} [src={src}]\n",
            name = r.name,
            tag = if tag.is_empty() {
                String::new()
            } else {
                format!(" {tag}")
            },
        ));
    }
    out.push_str(&format!(
        "\nSummary: {visible} visible to the model, {hidden} hidden.\n"
    ));
    out
}

fn runtime_show(catalog: &SkillCatalog, name: &str) -> String {
    match catalog.find(name) {
        None => format!(
            "/skill show '{name}': not found in catalog. Run `/skill list` to see available skills."
        ),
        Some(r) => {
            let mut out = format!("Skill: {}\n", r.name);
            if let Some(d) = &r.display_name {
                out.push_str(&format!("  display_name: {d}\n"));
            }
            out.push_str(&format!("  description: {}\n", r.description));
            if let Some(w) = &r.when_to_use {
                out.push_str(&format!("  when_to_use: {w}\n"));
            }
            if !r.paths.is_empty() {
                out.push_str(&format!("  paths: {:?}\n", r.paths));
            }
            out.push_str(&format!("  source: {:?}\n", r.source));
            out.push_str(&format!("  loaded_from: {:?}\n", r.loaded_from));
            out.push_str(&format!("  file_path: {}\n", r.file_path.display()));
            out.push_str(&format!(
                "  visibility: {}\n",
                if r.disable_model_invocation {
                    "hidden from model"
                } else {
                    "visible to model"
                }
            ));
            out.push_str(&format!("  user_invocable: {}\n", r.user_invocable));
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slash::parse;

    // ------------------------------------------------------------------
    // Back-compat tests — Stub variant preserves the v0.7.0 behaviour
    // ------------------------------------------------------------------

    #[test]
    fn stub_list_handled() {
        let inv = parse("/skill list").unwrap();
        let out = SkillHandler::Stub.invoke(&inv).unwrap();
        assert!(matches!(out, SlashOutcome::Handled { output: Some(_) }));
    }

    #[test]
    fn stub_show_requires_name() {
        let inv = parse("/skill show").unwrap();
        assert!(matches!(
            SkillHandler::Stub.invoke(&inv),
            Err(SlashError::Bad(_))
        ));
    }

    #[test]
    fn stub_run_requires_name() {
        let inv = parse("/skill run").unwrap();
        assert!(matches!(
            SkillHandler::Stub.invoke(&inv),
            Err(SlashError::Bad(_))
        ));
    }

    #[test]
    fn default_constructs_stub() {
        let h = SkillHandler::default();
        assert!(matches!(h, SkillHandler::Stub));
    }

    // ------------------------------------------------------------------
    // Runtime variant — exercises the real catalog
    // ------------------------------------------------------------------

    fn empty_catalog() -> Arc<SkillCatalog> {
        Arc::new(SkillCatalog::from_refs(Vec::new()))
    }

    #[test]
    fn runtime_list_empty_catalog() {
        let handler = SkillHandler::Runtime {
            catalog: empty_catalog(),
        };
        let inv = parse("/skill list").unwrap();
        let out = handler.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!();
        };
        // Must NOT contain the stub-mode placeholder.
        assert!(
            !s.contains("--skills-audit"),
            "runtime list leaked stub string: {s}"
        );
        assert!(s.contains("no skills"), "got: {s}");
    }

    #[test]
    fn runtime_show_missing_skill() {
        let handler = SkillHandler::Runtime {
            catalog: empty_catalog(),
        };
        let inv = parse("/skill show nonexistent").unwrap();
        let out = handler.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!();
        };
        assert!(s.contains("not found"), "got: {s}");
        assert!(s.contains("nonexistent"), "got: {s}");
    }

    #[test]
    fn runtime_run_missing_skill_says_not_found() {
        let handler = SkillHandler::Runtime {
            catalog: empty_catalog(),
        };
        let inv = parse("/skill run nope").unwrap();
        let out = handler.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!();
        };
        // Must NOT contain the stub-mode placeholder.
        assert!(!s.contains("3.C.4"), "runtime run leaked stub string: {s}");
        assert!(s.contains("no skill named"), "got: {s}");
    }

    // ------------------------------------------------------------------
    // Governance — clauses (b), (c), (d) on the shipped slash surface
    // ------------------------------------------------------------------

    fn skill_ref(name: &str, source: SkillSource, root: Option<std::path::PathBuf>) -> SkillRef {
        SkillRef {
            name: name.to_string(),
            display_name: None,
            description: "generated".to_string(),
            when_to_use: None,
            paths: Vec::new(),
            source,
            loaded_from: wcore_skills::types::LoadedFrom::Skills,
            file_path: root
                .as_ref()
                .map(|r| r.join("SKILL.md"))
                .unwrap_or_else(|| std::path::PathBuf::from("unused")),
            skill_root: root,
            content_length_hint: 0,
            user_invocable: true,
            disable_model_invocation: true,
            has_artifacts: false,
            inline_content: None,
        }
    }

    /// A draft on disk plus a governance store rooted in the same tempdir. Nothing here
    /// resolves a process-global path, so these tests neither race nor touch a real profile.
    fn govern_fixture(name: &str) -> (tempfile::TempDir, std::path::PathBuf, GovernanceStore) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "body\n").unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            br#"{"auto_drafted":true,"signature":"sig-1"}"#,
        )
        .unwrap();
        let store = GovernanceStore::new(tmp.path().join("skills-governance"));
        (tmp, dir, store)
    }

    #[test]
    fn unknown_sub_action_advertises_the_governance_verbs() {
        let inv = parse("/skill wat").unwrap();
        let Err(SlashError::Bad(msg)) = SkillHandler::Stub.invoke(&inv) else {
            panic!("expected a usage error");
        };
        for verb in ["govern", "revoke", "rollback"] {
            assert!(
                msg.contains(verb),
                "usage line hides '{verb}', so an operator cannot find it: {msg}"
            );
        }
    }

    #[test]
    fn govern_on_a_clean_store_reports_empty_rather_than_erroring() {
        let tmp = tempfile::tempdir().unwrap();
        let store = GovernanceStore::new(tmp.path().join("g"));
        let out = render_govern(&store);
        assert!(out.contains("Live revocations: none"), "got: {out}");
        assert!(out.contains("History: empty"), "got: {out}");
    }

    #[test]
    fn revoke_then_govern_shows_the_revocation_and_its_history() {
        let (_tmp, dir, store) = govern_fixture("auto-sl");
        let catalog = Arc::new(SkillCatalog::from_refs(vec![skill_ref(
            "auto-sl",
            SkillSource::User,
            Some(dir.clone()),
        )]));

        // Known-negative first, with the store proven readable in the same test: before the
        // revoke, `govern` must NOT name this skill. Otherwise the assertion after the
        // revoke would pass on a renderer that prints the name unconditionally.
        assert!(
            !render_govern(&store).contains("auto-sl"),
            "govern named a skill that had not been revoked yet"
        );

        let msg = revoke_named(&catalog, &store, "auto-sl");
        assert!(msg.starts_with("Revoked 'auto-sl'."), "got: {msg}");
        assert!(!dir.exists(), "revoke left the skill directory in place");

        let out = render_govern(&store);
        assert!(
            out.contains("auto-sl"),
            "govern hides the revocation: {out}"
        );
        assert!(out.contains("sig-1"), "govern hides the signature: {out}");
        assert!(out.contains("revoked "), "govern hides the history: {out}");
        assert!(
            out.contains("Live revocations (1)"),
            "govern miscounts: {out}"
        );

        // And it round-trips through the same surface.
        let id = store.live_revocations().unwrap()[0].revocation_id.clone();
        let back = rollback_id(&store, &id);
        assert!(
            back.starts_with(&format!("Rolled back '{id}'.")),
            "got: {back}"
        );
        assert!(dir.join("SKILL.md").is_file(), "rollback restored nothing");

        // The journal is append-only: the revocation event survives the rollback.
        let after = render_govern(&store);
        assert!(after.contains("Live revocations: none"), "got: {after}");
        assert!(
            after.contains("revoked "),
            "rollback erased history: {after}"
        );
        assert!(after.contains("rolled-back"), "got: {after}");
    }

    #[test]
    fn revoke_refuses_a_bundled_skill_rather_than_touching_the_install() {
        let r = skill_ref("bundled-thing", SkillSource::Bundled, None);
        let why = revocable_dir(&r).expect_err("bundled skills must not be revocable");
        assert!(why.contains("Bundled"), "got: {why}");

        // Control: the same helper accepts a real on-disk user skill, so the refusal above
        // is provenance-specific and not a helper that rejects everything.
        let (_tmp, dir, _store) = govern_fixture("auto-ok");
        let ok = skill_ref("auto-ok", SkillSource::User, Some(dir.clone()));
        assert_eq!(revocable_dir(&ok).unwrap(), dir);
    }

    #[test]
    fn revoke_of_an_unknown_skill_says_so_without_touching_the_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = GovernanceStore::new(tmp.path().join("g"));
        let msg = revoke_named(&SkillCatalog::from_refs(Vec::new()), &store, "nope");
        assert!(msg.contains("no skill named"), "got: {msg}");
        assert!(store.live_revocations().unwrap().is_empty());
    }

    #[test]
    fn rollback_of_an_unknown_id_reports_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let store = GovernanceStore::new(tmp.path().join("g"));
        let msg = rollback_id(&store, "no-such-id");
        assert!(msg.contains("failed"), "got: {msg}");
        assert!(msg.contains("no-such-id"), "got: {msg}");
    }

    #[test]
    fn runtime_run_rejects_model_hidden_skill() {
        let hidden = wcore_skills::refs::SkillRef {
            name: "auto-hidden".to_string(),
            display_name: None,
            description: "generated".to_string(),
            when_to_use: None,
            paths: Vec::new(),
            source: wcore_skills::types::SkillSource::Project,
            loaded_from: wcore_skills::types::LoadedFrom::Skills,
            file_path: std::path::PathBuf::from("unused"),
            skill_root: None,
            content_length_hint: 0,
            user_invocable: true,
            disable_model_invocation: true,
            has_artifacts: false,
            inline_content: None,
        };
        let handler = SkillHandler::Runtime {
            catalog: Arc::new(SkillCatalog::from_refs(vec![hidden])),
        };
        let inv = parse("/skill run auto-hidden").unwrap();
        let out = handler.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!();
        };
        assert!(s.contains("quarantined") && s.contains("cannot be run"));
        assert!(!s.contains("ask the agent"));
    }
}
