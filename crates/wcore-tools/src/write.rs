use std::path::Path;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolEffectContract, ToolEffectKind, ToolResult};

use crate::Tool;
use crate::context::ToolContext;
use crate::effects::{
    FILESYSTEM_EFFECT_RECONCILER, FilesystemWriteAttempt, PreparedToolEffect, ToolEffectExecution,
    classify_filesystem_execution, prepare_filesystem_effect,
};
use crate::file_cache::{FileStateCache, update_cache_after_write};
use crate::path_validation::validate_user_path;
use crate::unsaved_work::{Mode, UnsavedWorkGuard, Verdict};
use crate::vfs::{
    FileContentIdentity, FileMutationOutcome, FileObservation, FilePrecondition,
    IntendedFileMutation,
};

/// #1241 c3 — a verdict taken inside `atomic_write_checked`'s
/// exchange→verdict window on behalf of the tool. See
/// [`WriteTool::publish_window_probe`].
type PublishWindowProbe = Arc<dyn Fn(Option<&[u8]>) -> Result<(), String> + Send + Sync>;

pub struct WriteTool {
    file_cache: Option<Arc<RwLock<FileStateCache>>>,
    /// INV-2: session-scoped record of the user's unsaved work, so a
    /// whole-file overwrite can never silently delete it. Shared with the
    /// Edit tool and with every sub-agent's tools, so one baseline and one
    /// agent-authored set govern both write surfaces.
    unsaved: Arc<UnsavedWorkGuard>,
    /// #1241 c3 — the one seam into `atomic_write_checked`'s
    /// exchange→verdict window, so the `RollbackFailed` arm below can be
    /// graded through `Tool::execute` and not on the classifier alone.
    ///
    /// Reaching that arm for real needs the destination NAME to stop
    /// resolving strictly BETWEEN the publish exchange and the restore
    /// exchange. Nothing on the tool side of that window can make that
    /// happen: the only tool-side code in it is the pure `pre_image_matches`,
    /// and `Swap::Unsupported` cannot differ between two exchanges on one
    /// filesystem. What is left is a racer, which is a flake generator on a
    /// window measured in microseconds. So the unlink is done from inside the
    /// verdict itself, which is the one ordering that is inside the window by
    /// construction.
    ///
    /// `None` in production — only `with_publish_window_probe` sets it, and
    /// that is `#[cfg(test)]`. The closure at the call site is the SAME code
    /// either way: what a probe substitutes is the REASON the publish is
    /// refused, never the observable #1241 c2 grades, which is what
    /// `execute` hands back afterwards.
    publish_window_probe: Option<PublishWindowProbe>,
}

impl WriteTool {
    /// Create a WriteTool with optional file state cache.
    ///
    /// When cache is `Some`, the tool updates the cache after each successful
    /// write so that subsequent Edit/Read calls see the latest content and mtime.
    ///
    /// No "must Read first" guard: Write is intended for creating new files
    /// or complete rewrites.
    ///
    /// P2: a complete rewrite still may not destroy work the user has not
    /// saved. Every overwrite is checked against
    /// [`crate::unsaved_work::UnsavedWorkGuard`] and refused when it would
    /// drop a line that is on disk but in no commit. This guard is always
    /// on — it needs no cache and no wiring.
    ///
    /// Pass `None` to disable cache integration (legacy behavior).
    pub fn new(file_cache: Option<Arc<RwLock<FileStateCache>>>) -> Self {
        Self {
            file_cache,
            unsaved: UnsavedWorkGuard::shared(),
            publish_window_probe: None,
        }
    }

    /// Use `guard` instead of the process-wide one.
    ///
    /// The process-wide guard pins one baseline per repository for the whole
    /// session and is shared by every sub-agent, so a test that needs its own
    /// pins hands in an isolated guard; a host running several independent
    /// sessions in one process wants one for the same reason. There is no
    /// longer any snapshot directory under the profile home — recovery copies
    /// go to the repository's own object store.
    pub fn with_unsaved_guard(mut self, guard: Arc<UnsavedWorkGuard>) -> Self {
        self.unsaved = guard;
        self
    }

    /// #1241 c3 — take `probe`'s answer as the verdict on the displaced
    /// pre-image, so a test can act from inside the exchange→verdict window.
    ///
    /// Test-only. See [`WriteTool::publish_window_probe`] for why the window
    /// has no other entrance.
    #[cfg(test)]
    pub(crate) fn with_publish_window_probe(mut self, probe: PublishWindowProbe) -> Self {
        self.publish_window_probe = Some(probe);
        self
    }

    /// W8b — vfs-aware write body. Routes the write through `ctx.vfs`
    /// (RealFs at top-level, SandboxedFs for sub-agents) so sandbox
    /// enforcement applies. Wave SD adds the same `validate_user_path`
    /// shape check as the legacy entry so a top-level (non-sandboxed)
    /// ctx can't be used as a bypass for the path discipline.
    /// Trades the legacy tmp+rename atomicity for VFS-trait portability;
    /// the `RealFs::write` impl still creates parent dirs.
    async fn write_through_vfs(
        &self,
        input: Value,
        ctx: &ToolContext,
        attempt: &mut FilesystemWriteAttempt,
    ) -> ToolResult {
        let Some(file_path) = input["file_path"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: file_path".to_string(),
                is_error: true,
            };
        };
        let Some(content) = input["content"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: content".to_string(),
                is_error: true,
            };
        };

        let validated = match validate_user_path(Path::new(file_path)) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    content: format!("Refused to write {file_path}: {e}"),
                    is_error: true,
                };
            }
        };
        let path = validated.as_path();
        let existed = ctx.vfs.exists(path).await.unwrap_or(false);

        // INV-2: same unsaved-work assessment as the legacy path, before the
        // write. The pre-image is read back through the SAME vfs this call
        // will write to, so a sandboxed sub-agent is judged against the bytes
        // it can actually see rather than against the real filesystem. A path
        // the sandbox would reject never reaches the guard at all — the
        // sandbox denial, not a message quoting that file's lines, is the
        // right answer there.
        // ADV-8, vfs side: the same fail-open in the same shape. A read that
        // failed, and bytes that are not UTF-8, both became an empty
        // pre-image that the gate below waved through. Only the vfs saying
        // definitely "there is nothing here" may do that.
        let mut attributable = true;
        let mut judged: Option<Vec<u8>> = None;
        let previous = match ctx.vfs.read(path).await {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => {
                    judged = Some(text.as_bytes().to_vec());
                    text
                }
                Err(e) => {
                    let raw = e.into_bytes();
                    judged = Some(raw.clone());
                    if let Verdict::Refuse(refusal) = self.unsaved.assess_opaque(
                        path,
                        file_path,
                        Some(&raw),
                        "the bytes on disk are not valid UTF-8",
                    ) {
                        return ToolResult {
                            content: refusal,
                            is_error: true,
                        };
                    }
                    attributable = false;
                    String::new()
                }
            },
            Err(e) => match ctx.vfs.exists(path).await {
                // Definitely nothing there: an ordinary create.
                Ok(false) => String::new(),
                _ => {
                    if let Verdict::Refuse(refusal) =
                        self.unsaved
                            .assess_opaque(path, file_path, None, &e.to_string())
                    {
                        return ToolResult {
                            content: refusal,
                            is_error: true,
                        };
                    }
                    attributable = false;
                    String::new()
                }
            },
        };
        let mut unsaved_note = String::new();
        if !previous.is_empty() {
            match self
                .unsaved
                .assess(path, file_path, &previous, content, Mode::Rewrite)
            {
                Verdict::Proceed => {}
                Verdict::ProceedWithNote(note) => unsaved_note = note,
                Verdict::Refuse(refusal) => {
                    return ToolResult {
                        content: refusal,
                        is_error: true,
                    };
                }
            }
        }

        // W8b.2.A D.4 — mark this write as engine-originated BEFORE the
        // actual write so an upstream FileWatcher can debounce its own
        // change event and not feed it back as an "external edit".
        // Skipped when no notifier is wired (the test-default case).
        if let Some(n) = ctx.file_write_notifier.as_ref() {
            n.note_self_originated_write(path).await;
        }

        // ADV-7 / #1155, vfs side. The re-check and the write are two
        // operations through the same vfs, which is the window #1155 measured
        // at 140 of 200 on the Edit tool. The pre-image travels WITH the write
        // instead — see `RealFs::compare_exchange_file`.
        let precondition = match judged.as_deref() {
            Some(before) => FilePrecondition::Present(FileContentIdentity::from_bytes(before)),
            None => FilePrecondition::Absent,
        };
        let mutation = IntendedFileMutation::new(precondition, content.as_bytes().to_vec());

        // F13: everything above this line left the target untouched, and
        // everything below it may not have. A `Conflict` is the exception the
        // backend can answer for: nothing was published.
        *attempt = FilesystemWriteAttempt::Attempted;
        match ctx.vfs.compare_exchange_file(path, &mutation).await {
            Ok(FileMutationOutcome::Applied { .. }) => {}
            Ok(FileMutationOutcome::AlreadyApplied { .. }) => {
                // Byte-identical to what is already there. A compare-exchange
                // declines to touch the inode; the shipped write rewrites it, and
                // F889 receipt reconciliation is calibrated against that
                // (f889_write_edit_reconcile_test, the byte-identical-write arm).
                // Changing WHEN a no-op rewrite happens is not this fix's business,
                // and it costs nothing here: `AlreadyApplied` means the destination
                // still holds the pre-image, so no save was displaced and there is
                // no race to lose.
                if let Err(e) = ctx.vfs.write(path, content.as_bytes()).await {
                    return ToolResult {
                        content: format!("Failed to write file: {e}"),
                        is_error: true,
                    };
                }
            }
            Ok(FileMutationOutcome::Conflict { current }) => {
                *attempt = FilesystemWriteAttempt::NotAttempted;
                let why = match (current, judged.is_some()) {
                    (FileObservation::Absent, true) => "it was deleted",
                    (FileObservation::Present(_), false) => "something else created it",
                    _ => "its contents changed on disk",
                };
                return ToolResult {
                    content: crate::unsaved_work::changed_under_write(file_path, why),
                    is_error: true,
                };
            }
            // A backend with no compare-exchange keeps exactly the behaviour
            // it had: re-read, then write. Racy, and unchanged by this fix.
            Err(e) if crate::vfs::is_compare_exchange_unsupported(&e) => {
                let still = match ctx.vfs.read(path).await {
                    Ok(now) => match judged.as_deref() {
                        Some(before) if now == before => Ok(()),
                        Some(_) => Err("its contents changed on disk".to_owned()),
                        None => Err("something else created it".to_owned()),
                    },
                    Err(e) if judged.is_none() => match ctx.vfs.exists(path).await {
                        Ok(false) => Ok(()),
                        _ => Err(format!("it could no longer be read ({e})")),
                    },
                    Err(e) => Err(format!("it could no longer be read ({e})")),
                };
                if let Err(why) = still {
                    *attempt = FilesystemWriteAttempt::NotAttempted;
                    return ToolResult {
                        content: crate::unsaved_work::changed_under_write(file_path, &why),
                        is_error: true,
                    };
                }
                if let Err(e) = ctx.vfs.write(path, content.as_bytes()).await {
                    return ToolResult {
                        content: format!("Failed to write file: {e}"),
                        is_error: true,
                    };
                }
            }
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to write file: {e}"),
                    is_error: true,
                };
            }
        }
        *attempt = FilesystemWriteAttempt::Completed;

        if let Some(cache_arc) = &self.file_cache {
            update_cache_after_write(cache_arc, path, content);
        }
        // Not `previous` when the pre-image was not text: claiming the file
        // was empty would make every line of `content` agent-authored, and so
        // exempt from this guard for the rest of the session.
        let attribution_pre = if attributable {
            previous.as_str()
        } else {
            content
        };
        self.unsaved.note_written(path, attribution_pre, content);

        let line_count = content.lines().count();
        let action = if existed { "Updated" } else { "Created" };
        ToolResult {
            content: format!("{action} {file_path} ({line_count} lines){unsaved_note}"),
            is_error: false,
        }
    }
}

/// #1241 — which of `atomic_write_checked`'s two `Err` meanings this is, and
/// therefore whether the direct Write path may publish unchecked.
///
/// `Ok(error)` — the tempfile round trip never reached a verdict at all: a
/// cross-device rename, a directory that will not hold a sibling temp file.
/// Nothing was published and no verdict was taken, so the unchecked fallback
/// is still the right answer and still reports success.
///
/// `Err(report)` — the guard RAN, the verdict REFUSED, and the publish could
/// not be retracted. Since #1202 that is a reachable meaning of the same
/// `Err`, and it is the opposite situation: the new bytes are already
/// published and the pre-image survives only under the name the error carries.
/// Falling through would rewrite bytes that are already there, throw the
/// refusal away, and report `Updated <path>` — success, for a write this
/// tool's own guard had just refused, with no mention that a concurrent change
/// was seen or that the user's original is sitting under a `.tmpXXXXXX`
/// sibling that nothing will ever clean up.
///
/// Told apart by TYPE, never by the message text: an unrecognised string would
/// read as "never reached a verdict", which is the answer that republishes.
///
/// Deliberately no cache update and no `note_written` on the report path. The
/// bytes on disk are not this tool's to claim authorship of, and a cache entry
/// left stale fails CLOSED at the next edit.
fn unpublished_or_unrolled(
    file_path: &str,
    error: std::io::Error,
) -> Result<std::io::Error, ToolResult> {
    match wcore_config::rollback_failure(&error) {
        Some(unrolled) => Err(ToolResult {
            content: crate::unsaved_work::refused_but_not_rolled_back(
                file_path,
                unrolled.why(),
                unrolled.preserved_at(),
            ),
            is_error: true,
        }),
        None => Ok(error),
    }
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "Write"
    }

    fn description(&self) -> &str {
        "Writes content to a file, creating parent directories if needed.\n\n\
         Usage:\n\
         - This tool overwrites the existing file completely (not append).\n\
         - If the file already exists, you must use Read first to see its current content.\n\
         - Prefer Edit over Write for modifying existing files — Edit only sends the diff.\n\
         - Use Write only for creating new files or complete rewrites.\n\
         - A rewrite that would delete a line present on disk but absent from the \
         file's last commit is refused: that is unsaved user work. Carry those lines \
         into the content you write — in their changed form if what you are doing \
         changes them.\n\
         - Never route a rewrite around that refusal. Bash refuses a git command \
         that would throw the work tree away, but a write made through Bash (`sed -i`, \
         `>`, `rm`) is not checked by anything and destroys unsaved work irreversibly; \
         using one to apply a change this tool refused is the single worst thing you \
         can do to the user's file."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["file_path", "content"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(file_path) = input["file_path"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: file_path".to_string(),
                is_error: true,
            };
        };
        let Some(content) = input["content"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: content".to_string(),
                is_error: true,
            };
        };

        // Wave SD SECURITY MAJOR #14: validate the LLM-supplied path
        // before touching the disk. Refuses relative paths, traversal,
        // null bytes, and a deny-list of obvious system secrets.
        let validated = match validate_user_path(Path::new(file_path)) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    content: format!("Refused to write {file_path}: {e}"),
                    is_error: true,
                };
            }
        };
        let path = validated.as_path();
        let existed = path.exists();

        // P2: never let a whole-file rewrite delete work the user has not
        // saved. Checked before any disk mutation so a refusal leaves the file
        // exactly as it was. On a create there is nothing to lose, and
        // recording the empty baseline now is what keeps the agent's own later
        // rewrites of its own file free.
        // Gated on the pre-image, not on `existed`: a probe that reports
        // "no such file" on an error it could not classify would otherwise
        // wave the overwrite straight through. If there are bytes there, they
        // are judged, whatever `exists` said.
        //
        // ADV-8: and "there are bytes there" is decided by the read, not by
        // the read succeeding. `unwrap_or_default()` turned every failure —
        // a permission denied, a directory in the way, bytes that are not
        // UTF-8 — into an empty pre-image, which the gate below then waved
        // straight through. Measured as uid 65534 against a root-owned 0600
        // file in a writable directory: no refusal, no note, no copy, and the
        // file went `root:root 0600` -> `nobody:nogroup 0644`. Only a
        // definite "no such file" may produce an empty pre-image.
        let mut attributable = true;
        // The exact bytes the assessment below is about to judge, re-checked
        // immediately before the write lands (ADV-7).
        let mut judged: Option<Vec<u8>> = None;
        let previous = match std::fs::read_to_string(path) {
            Ok(text) => {
                judged = Some(text.as_bytes().to_vec());
                text
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                let raw = std::fs::read(path).ok();
                judged = raw.clone();
                if let Verdict::Refuse(refusal) =
                    self.unsaved
                        .assess_opaque(path, file_path, raw.as_deref(), &e.to_string())
                {
                    return ToolResult {
                        content: refusal,
                        is_error: true,
                    };
                }
                // Proven byte-identical to the pinned commit, so nothing on
                // disk is unsaved. Nothing may be claimed as agent-authored
                // either — there is no line model for a pre-image that is not
                // text — so the attribution record below is told this write
                // introduced nothing.
                attributable = false;
                String::new()
            }
        };
        let mut unsaved_note = String::new();
        if !previous.is_empty() {
            match self
                .unsaved
                .assess(path, file_path, &previous, content, Mode::Rewrite)
            {
                Verdict::Proceed => {}
                Verdict::ProceedWithNote(note) => unsaved_note = note,
                Verdict::Refuse(refusal) => {
                    return ToolResult {
                        content: refusal,
                        is_error: true,
                    };
                }
            }
        }

        // Create parent directories
        if let Some(parent) = path.parent().filter(|p| !p.exists()) {
            match std::fs::create_dir_all(parent) {
                Ok(()) => {}
                Err(e) => {
                    return ToolResult {
                        content: format!("Failed to create directories: {}", e),
                        is_error: true,
                    };
                }
            }
        }

        // ADV-7 / #1155: the assessment took a measured 13.5 ms, and a save
        // that landed inside it was destroyed uncopied while the note claimed
        // the prior contents were preserved. Re-reading the path immediately
        // before the rename only NARROWED that window — measured at 13 losses
        // in 200 with the re-check in place — because the read and the rename
        // are still two operations. `atomic_write_checked` publishes by an
        // atomic exchange, so the pre-image it judges IS the one it displaced.
        let unpublishable =
            match wcore_config::atomic_write_checked(path, content.as_bytes(), |observed| {
                // #1241 c3. `None` in production, so this is the shipped
                // predicate on every real write.
                if let Some(probe) = self.publish_window_probe.as_ref() {
                    probe(observed)?;
                }
                crate::unsaved_work::pre_image_matches(observed, judged.as_deref())
            }) {
                Ok(Ok(())) => None,
                Ok(Err(why)) => {
                    // #1239 — as in edit.rs: say so when the retraction
                    // displaced a save, instead of the wording that promises
                    // nothing was changed.
                    return ToolResult {
                        content: crate::unsaved_work::refusal_message(file_path, &why),
                        is_error: true,
                    };
                }
                Err(e) => match unpublished_or_unrolled(file_path, e) {
                    Ok(never_reached_a_verdict) => Some(never_reached_a_verdict),
                    Err(report) => return report,
                },
            };

        if let Some(e) = unpublishable {
            // Fallback: direct write if the tempfile round trip fails at all
            // (a cross-device rename, or a directory that will not hold a
            // sibling). The guard above did not run, so this is the one path
            // that publishes unchecked. #1241 narrowed WHICH errors get here —
            // a refusal that could not be rolled back is diverted above — so
            // the premise in this comment is true again: reaching this line
            // means no verdict was ever taken.
            if let Err(e) = std::fs::write(path, content) {
                return ToolResult {
                    content: format!("Failed to write file: {}", e),
                    is_error: true,
                };
            }
            if let Some(cache_arc) = &self.file_cache {
                update_cache_after_write(cache_arc, path, content);
            }
            // Not `previous` when the pre-image was not text: claiming the file
            // was empty would make every line of `content` agent-authored, and so
            // exempt from this guard for the rest of the session.
            let attribution_pre = if attributable {
                previous.as_str()
            } else {
                content
            };
            self.unsaved.note_written(path, attribution_pre, content);

            return ToolResult {
                content: format!(
                    "Updated {} (atomic write failed: {}, used direct write){}",
                    file_path, e, unsaved_note
                ),
                is_error: false,
            };
        }

        if let Some(cache_arc) = &self.file_cache {
            update_cache_after_write(cache_arc, path, content);
        }
        // Not `previous` when the pre-image was not text: claiming the file
        // was empty would make every line of `content` agent-authored, and so
        // exempt from this guard for the rest of the session.
        let attribution_pre = if attributable {
            previous.as_str()
        } else {
            content
        };
        self.unsaved.note_written(path, attribution_pre, content);

        let line_count = content.lines().count();
        let action = if existed { "Updated" } else { "Created" };
        ToolResult {
            content: format!("{action} {file_path} ({line_count} lines){unsaved_note}"),
            is_error: false,
        }
    }

    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        self.write_through_vfs(input, ctx, &mut FilesystemWriteAttempt::default())
            .await
    }

    fn max_result_size(&self) -> usize {
        10_000
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Edit
    }

    /// F13: a whole-file rewrite is filesystem-transactional *when the
    /// target can be identified before the write*.
    ///
    /// The kind is declared unconditionally because `effect_contract` sees
    /// only the input; whether THIS call actually gets a receipt is decided
    /// by [`Self::prepare_effect`], and orchestration narrows the recorded
    /// contract back to opaque when no receipt was produced.
    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        ToolEffectContract {
            kind: ToolEffectKind::FilesystemTransactional,
            reconciler: Some(FILESYSTEM_EFFECT_RECONCILER.to_string()),
        }
    }

    /// Bind this call to the exact preimage and postimage identities, so a
    /// crash between here and the terminal journal append can be answered by
    /// one read of the target instead of by a human.
    async fn prepare_effect(
        &self,
        input: &Value,
        ctx: &ToolContext,
    ) -> Result<Option<PreparedToolEffect>, ToolResult> {
        // Malformed input stays the ordinary path's error to report, with the
        // wording it already has; preparation only ever declines.
        let (Some(file_path), Some(content)) =
            (input["file_path"].as_str(), input["content"].as_str())
        else {
            return Ok(None);
        };
        let intended = content.as_bytes().to_vec();
        Ok(prepare_filesystem_effect(
            ctx.vfs.as_ref(),
            Path::new(file_path),
            input,
            move |_preimage| Some(intended),
        )
        .await)
    }

    async fn execute_prepared_effect(
        &self,
        prepared: PreparedToolEffect,
        ctx: &ToolContext,
    ) -> ToolEffectExecution {
        let mut attempt = FilesystemWriteAttempt::default();
        let result = self
            .write_through_vfs(prepared.invocation().clone(), ctx, &mut attempt)
            .await;
        classify_filesystem_execution(&prepared, ctx.vfs.as_ref(), attempt, result).await
    }

    fn describe(&self, input: &Value) -> String {
        let path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        format!("Write to {}", path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    use crate::Tool;
    use crate::file_cache::file_mtime_ms;
    use wcore_config::file_cache::FileCacheConfig;

    /// A Write tool whose recovery snapshots land in a throwaway directory
    /// rather than the real `~/.wayland`.
    fn tool(cache: Option<Arc<RwLock<FileStateCache>>>) -> WriteTool {
        WriteTool::new(cache).with_unsaved_guard(Arc::new(
            crate::unsaved_work::UnsavedWorkGuard::new_isolated(),
        ))
    }

    /// #1241 c2 + c3. A refusal whose rollback exchanged NOTHING must not
    /// reach the unchecked fallback, and what the user is told must carry the
    /// name the pre-image was preserved under.
    ///
    /// The error is not hand-built. It comes out of a real
    /// `atomic_write_checked` run in the state the ticket names — the
    /// destination NAME disappearing between the two exchanges, driven
    /// directly by deleting it from inside the check closure, which is the one
    /// ordering that places the unlink inside the window — and is then handed
    /// to the very function the direct Write path's `Err` arm calls.
    ///
    /// Unix only: `Swap::Displaced` on the exchange platforms is
    /// `RENAME_EXCHANGE` / `RENAME_SWAP`, and Windows restores with
    /// `ReplaceFileW`, which succeeds against an absent destination and so
    /// cannot reach this state.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_refusal_that_could_not_be_rolled_back_is_not_a_fallback() {
        const ORIGINAL: &[u8] = b"the only copy of the user's bytes";

        let dir = tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, ORIGINAL).unwrap();

        let mut handed = None;
        let error = wcore_config::atomic_write_checked(&p, b"ours", |observed| {
            handed = observed.map(<[u8]>::to_vec);
            std::fs::remove_file(&p).unwrap();
            Err("its contents changed on disk".to_owned())
        })
        .expect_err("the rollback should have failed with the destination gone");

        assert_eq!(
            handed.as_deref(),
            Some(ORIGINAL),
            "fixture: the check was not handed the displaced pre-image, so this \
             never reached a rollback at all"
        );

        let report = unpublished_or_unrolled("/w/f.txt", error)
            .expect_err("a refusal that could not be rolled back was sent to the fallback");

        assert!(report.is_error, "reported as a success: {}", report.content);
        assert!(
            !report.content.starts_with("Updated "),
            "reported as an update: {}",
            report.content
        );

        // c2: the text names where the pre-image is, and that name is real.
        let survivors: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|f| std::fs::read(f).is_ok_and(|b| b == ORIGINAL))
            .collect();
        assert_eq!(survivors.len(), 1, "the pre-image did not survive");
        assert!(
            report.content.contains(&survivors[0].display().to_string()),
            "the user is not told where the original was preserved: {}",
            report.content
        );
    }

    /// #1241 c2 + c3, through `WriteTool::execute`.
    ///
    /// The criterion's subject is "the Write tool returns", so the tool is
    /// what is driven and what is graded: the direct, no-`ToolContext`
    /// `execute` runs a REAL `atomic_write_checked` into the state the ticket
    /// names — the guard refused and the publish could not be retracted — and
    /// the assertions are on the `ToolResult` it hands back.
    ///
    /// The destination is unlinked from inside the exchange→verdict window by
    /// the test-only `publish_window_probe`. That is the ONLY seam into the
    /// window, for the reason the field's own doc gives, and it substitutes
    /// exactly one thing: the reason the publish is refused. The exchange, the
    /// failed restore, `keep_displaced`, `RollbackFailed`, the
    /// `unpublished_or_unrolled` classification, the message, and the tool's
    /// return value are all the production code.
    ///
    /// The new content is a SUPERSET of the pre-image on purpose: a rewrite
    /// that drops a line is refused by the unsaved-work guard before any of
    /// this runs, even outside a repository (`unsaved_work_no_git_test`), and
    /// the test would then grade nothing.
    ///
    /// Unix exchange platforms only: `Swap::Displaced` is
    /// `RENAME_EXCHANGE` / `RENAME_SWAP` there, and Windows restores with
    /// `ReplaceFileW`, which succeeds against an absent destination and so
    /// cannot reach this state at all.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn execute_reports_a_refusal_it_could_not_roll_back() {
        const ORIGINAL: &str = "the only copy of the user's bytes\n";
        const REWRITE: &str = "the only copy of the user's bytes\nand a line the agent added\n";

        let dir = tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, ORIGINAL).unwrap();

        // Every entry into the window, in order, so the fixture control below
        // can assert it was entered exactly once and with the pre-image.
        let handed: Arc<std::sync::Mutex<Vec<Option<Vec<u8>>>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = Arc::clone(&handed);
        let doomed = p.clone();
        let tool =
            tool(None).with_publish_window_probe(Arc::new(move |observed: Option<&[u8]>| {
                recorded.lock().unwrap().push(observed.map(<[u8]>::to_vec));
                // Inside the window by construction: the publish exchange has
                // happened (that is what produced `observed`) and the restore
                // exchange has not.
                std::fs::remove_file(&doomed).unwrap();
                Err("its contents changed on disk".to_owned())
            }));

        let result = tool
            .execute(json!({
                "file_path": p.to_str().unwrap(),
                "content": REWRITE,
            }))
            .await;

        // Fixture control. Without it every assertion below would also pass on
        // a refusal taken BEFORE any publish, which is a different arm.
        let handed = handed.lock().unwrap();
        assert_eq!(
            handed.len(),
            1,
            "the exchange→verdict window was entered {} times, not once, so \
             this does not grade the arm it claims to",
            handed.len()
        );
        assert_eq!(
            handed[0].as_deref(),
            Some(ORIGINAL.as_bytes()),
            "the verdict was not handed the displaced pre-image, so no publish \
             was retracted here"
        );

        assert!(result.is_error, "reported as a success: {}", result.content);
        assert!(
            !result.content.starts_with("Updated "),
            "reported as an update: {}",
            result.content
        );
        assert!(
            !result.content.contains("used direct write"),
            "the unchecked fallback republished over a refusal: {}",
            result.content
        );

        // c2: the text names where the pre-image is, and that name is real.
        let survivors: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|f| std::fs::read(f).is_ok_and(|b| b == ORIGINAL.as_bytes()))
            .collect();
        assert_eq!(
            survivors.len(),
            1,
            "the pre-image did not survive: {survivors:?}"
        );
        assert!(
            result.content.contains(&survivors[0].display().to_string()),
            "the user is not told where the original was preserved: {}",
            result.content
        );
    }

    /// #1248 c2 + c3, through the VFS path — the one `write_through_vfs`
    /// takes whenever a `ToolContext` is present, which is every dispatched
    /// tool call.
    ///
    /// The criterion's subject is the SURFACED tool text, so the tool is what
    /// is driven and what is graded: `execute_with_ctx` over a real `RealFs`
    /// runs a real `compare_exchange_file`, which runs a real
    /// `atomic_write_checked`, and the assertions are on the `ToolResult` it
    /// hands back and on the file that is actually on disk when it does.
    ///
    /// The save that the retraction displaces is made from inside the
    /// exchange to verdict window by the test-only `publish_window` probe.
    /// That is the ONLY seam into the window, for the reason that module's own
    /// doc gives, and it substitutes exactly one thing: the reason the publish
    /// is refused. The exchange, the restore, `keep_displaced`, the `Refusal`,
    /// what `compare_exchange_file` carries out of it and what this tool
    /// renders off that are all production code.
    ///
    /// The new content is a SUPERSET of the pre-image on purpose: a rewrite
    /// that drops a line is refused by the unsaved-work guard before any of
    /// this runs, and the test would then grade nothing.
    ///
    /// Unix exchange platforms only: `Swap::Displaced` is `RENAME_EXCHANGE` /
    /// `RENAME_SWAP` there. Windows publishes with `ReplaceFileW` and restores
    /// with a plain replacing rename, which hands nothing back to judge, so no
    /// save can be intercepted there at all.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn the_vfs_path_names_a_save_the_refusal_displaced() {
        const ORIGINAL: &str = "the only copy of the user's bytes\n";
        const REWRITE: &str = "the only copy of the user's bytes\nand a line the agent added\n";
        const THEIR_SAVE: &[u8] = b"what the user saved while the check was running\n";

        let dir = tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, ORIGINAL).unwrap();

        // Every entry into the window, in order, so the fixture control below
        // can assert it was entered exactly once and with the pre-image.
        let handed: Arc<std::sync::Mutex<Vec<Option<Vec<u8>>>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = Arc::clone(&handed);
        let dest = p.clone();
        let _probe = crate::vfs::publish_window::install(
            &p,
            Box::new(move |observed: Option<&[u8]>| {
                recorded.lock().unwrap().push(observed.map(<[u8]>::to_vec));
                // Inside the window by construction: the publish exchange has
                // happened (that is what produced `observed`) and the restore
                // exchange has not, so this name resolves to the NEW inode and
                // the save lands on it exactly as an editor's would.
                std::fs::write(&dest, THEIR_SAVE).unwrap();
                Err("its contents changed on disk".to_owned())
            }),
        );

        let ctx = crate::context::ToolContext::test_default();
        let result = tool(None)
            .execute_with_ctx(
                json!({ "file_path": p.to_str().unwrap(), "content": REWRITE }),
                &ctx,
            )
            .await;

        // Fixture control. Without it every assertion below would also pass on
        // a conflict classified BEFORE any publish, which is a different arm
        // and is what c4 pins.
        let entries = handed.lock().unwrap().len();
        assert_eq!(
            entries, 1,
            "the exchange to verdict window was entered {entries} times, not \
             once, so this does not grade the arm it claims to"
        );
        assert_eq!(
            handed.lock().unwrap()[0].as_deref(),
            Some(ORIGINAL.as_bytes()),
            "the verdict was not handed the displaced pre-image, so no publish \
             was retracted here"
        );

        assert!(result.is_error, "reported as a success: {}", result.content);

        // The destination is back to exactly what it held — which is why the
        // notice cannot be recovered by re-observing it, and has to be carried.
        assert_eq!(std::fs::read(&p).unwrap(), ORIGINAL.as_bytes());

        // The user's save is on disk, under a name only the refusal knows.
        let survivors: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|f| std::fs::read(f).is_ok_and(|b| b == THEIR_SAVE))
            .collect();
        assert_eq!(
            survivors.len(),
            1,
            "the displaced save did not survive: {survivors:?}"
        );
        assert_ne!(survivors[0], p);

        // c2 + c3: the SURFACED text names it, and does not claim the refusal
        // cost nobody anything.
        assert!(
            result
                .content
                .contains(&survivors[0].display().to_string()),
            "the user is not told where their save went: {}",
            result.content
        );
        assert!(
            !result.content.contains("Nothing was changed."),
            "a refusal that displaced a save still says nothing was changed: {}",
            result.content
        );
    }

    /// #1241 c4, the classifier half: a round trip that never reached a
    /// verdict must still be handed back for the unchecked fallback to
    /// publish. Fails if the branch above widens to swallow every `Err`.
    #[test]
    fn a_round_trip_that_never_reached_a_verdict_is_handed_to_the_fallback() {
        let dir = tempdir().unwrap();
        let nowhere = dir.path().join("no-such-dir").join("f.txt");
        let error = wcore_config::atomic_write_checked(&nowhere, b"ours", |_| Ok(()))
            .expect_err("staging a temp file under a missing directory should fail");

        let handed_back = unpublished_or_unrolled("/w/f.txt", error)
            .expect("a genuine round-trip failure was swallowed as a refusal");
        assert_eq!(handed_back.kind(), std::io::ErrorKind::NotFound);
    }

    /// #1241 c4. The other meaning of `Err` — a round trip that never reached
    /// a verdict — must still fall through to the unchecked write, publish,
    /// and report success.
    ///
    /// The trigger is the one the fallback's own comment names: a directory
    /// that will not hold a sibling temp file. It is built by nesting until
    /// the destination path is as long as the platform will accept, so that
    /// the destination itself opens but `.tmpXXXXXX` beside it is one
    /// character too long. That works regardless of who is running the test,
    /// which a permission-based fixture would not — every one of these runs as
    /// root on the build host, where a mode-0555 directory is no obstacle.
    ///
    /// Unix only: the search is over `PATH_MAX`, and Windows' equivalent is
    /// lifted by `long_path_safe_dest` on the very path under test.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_round_trip_that_never_reached_a_verdict_still_publishes_unchecked() {
        /// A directory that will hold `f` but not a `.tmpXXXXXX` sibling.
        /// Found by measurement rather than by arithmetic over a `PATH_MAX`
        /// this crate would have to guess at.
        fn dir_that_refuses_a_sibling_temp_file(base: &Path) -> Option<std::path::PathBuf> {
            let mut dir = base.to_path_buf();
            loop {
                let next = dir.join("d".repeat(100));
                if std::fs::create_dir(&next).is_err() {
                    break;
                }
                dir = next;
            }
            // The last 100-char step failed, so the limit is inside the next
            // hundred characters. Walk down from there and take the first
            // length that shows both halves of the property.
            for len in (1..=99).rev() {
                let candidate = dir.join("d".repeat(len));
                if std::fs::create_dir(&candidate).is_err() {
                    continue;
                }
                let refuses_a_sibling = tempfile::NamedTempFile::new_in(&candidate).is_err();
                let holds_the_destination = std::fs::write(candidate.join("f"), b"probe").is_ok();
                if refuses_a_sibling && holds_the_destination {
                    let _ = std::fs::remove_file(candidate.join("f"));
                    return Some(candidate);
                }
            }
            None
        }

        let base = tempdir().unwrap();
        let Some(dir) = dir_that_refuses_a_sibling_temp_file(base.path()) else {
            panic!(
                "no path length on this filesystem refuses a sibling temp file \
                 while still opening the destination, so this test cannot \
                 reach the fallback it grades"
            );
        };
        let dest = dir.join("f");

        // Fixture control, restated at the moment of use: if the round trip
        // can be staged here then nothing below measures the fallback.
        assert!(
            tempfile::NamedTempFile::new_in(&dir).is_err(),
            "the fixture directory accepts a sibling temp file after all"
        );

        let tool = tool(None);
        let result = tool
            .execute(json!({
                "file_path": dest.to_str().unwrap(),
                "content": "published unchecked",
            }))
            .await;

        assert!(
            !result.is_error,
            "a round trip that never reached a verdict was reported as a \
             failure: {}",
            result.content
        );
        assert!(
            result.content.contains("used direct write"),
            "the fallback did not run: {}",
            result.content
        );
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            "published unchecked",
            "the fallback did not publish"
        );
    }

    fn make_cache() -> Arc<RwLock<FileStateCache>> {
        let config = FileCacheConfig {
            max_entries: 100,
            max_size_bytes: 25 * 1024 * 1024,
            enabled: true,
        };
        Arc::new(RwLock::new(FileStateCache::new(&config)))
    }

    // -- Legacy tests (no cache) --

    #[tokio::test]
    async fn test_write_new_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("hello.txt");

        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "hello world"
        });

        let tool = tool(None);
        let result = tool.execute(input).await;

        assert!(
            !result.is_error,
            "expected success, got: {}",
            result.content
        );
        assert!(file_path.exists(), "file should exist after write");
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "hello world");
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("subdir/nested/file.txt");

        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "nested content"
        });

        let tool = tool(None);
        let result = tool.execute(input).await;

        assert!(
            !result.is_error,
            "expected success, got: {}",
            result.content
        );
        assert!(
            file_path.parent().unwrap().exists(),
            "parent dirs should be created"
        );
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "nested content"
        );
    }

    #[tokio::test]
    async fn test_write_overwrite_existing() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("overwrite.txt");

        let tool = tool(None);

        let input1 = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "original"
        });
        let result1 = tool.execute(input1).await;
        assert!(!result1.is_error);
        assert!(result1.content.contains("Created"));

        let input2 = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "replaced"
        });
        let result2 = tool.execute(input2).await;
        assert!(!result2.is_error);
        assert!(result2.content.contains("Updated"));

        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "replaced");
    }

    #[tokio::test]
    async fn test_write_file_content_matches() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("exact.txt");

        let content = "line 1\nline 2\nline 3\n";
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": content
        });

        let tool = tool(None);
        let result = tool.execute(input).await;

        assert!(
            !result.is_error,
            "expected success, got: {}",
            result.content
        );

        let read_back = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(
            read_back, content,
            "read-back content must exactly match written content"
        );
    }

    // -- Cache integration tests --

    #[tokio::test]
    async fn write_populates_cache() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("cached.txt");

        let cache = make_cache();
        let tool = tool(Some(cache.clone()));

        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "cached content"
        });
        let result = tool.execute(input).await;
        assert!(!result.is_error, "write failed: {}", result.content);

        // Cache should have an entry with correct mtime.
        let disk_mtime = file_mtime_ms(&file_path).unwrap();
        let mut c = cache.write().unwrap();
        let cached = c
            .get(&file_path)
            .expect("file should be in cache after write");
        assert_eq!(cached.mtime_ms, disk_mtime);
        assert!(cached.content.contains("cached content"));
    }

    #[tokio::test]
    async fn write_then_edit_succeeds() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("write_edit.txt");

        let cache = make_cache();
        let write_tool = tool(Some(cache.clone()));
        let edit_tool = crate::edit::EditTool::new(Some(cache)).with_unsaved_guard(Arc::new(
            crate::unsaved_work::UnsavedWorkGuard::new_isolated(),
        ));

        // Write creates the file and populates cache.
        let write_input = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "hello world"
        });
        let wr = write_tool.execute(write_input).await;
        assert!(!wr.is_error, "write failed: {}", wr.content);

        // Edit should succeed without needing a separate Read.
        let edit_input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "hello",
            "new_string": "goodbye"
        });
        let er = edit_tool.execute(edit_input).await;
        assert!(!er.is_error, "edit after write failed: {}", er.content);
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "goodbye world"
        );
    }

    #[tokio::test]
    async fn write_overwrite_updates_cache_mtime() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("overwrite_cache.txt");

        let cache = make_cache();
        let tool = tool(Some(cache.clone()));

        // First write.
        let input1 = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "v1"
        });
        tool.execute(input1).await;

        let mtime1 = {
            let mut c = cache.write().unwrap();
            c.get(&file_path).unwrap().mtime_ms
        };

        // Brief delay to ensure mtime changes.
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Second write.
        let input2 = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "v2"
        });
        tool.execute(input2).await;

        let mtime2 = {
            let mut c = cache.write().unwrap();
            c.get(&file_path).unwrap().mtime_ms
        };

        assert!(
            mtime2 >= mtime1,
            "cache mtime should update after overwrite"
        );
    }
}
