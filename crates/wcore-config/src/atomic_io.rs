//! Atomic file write helpers.
//!
//! `std::fs::write()` lacks two guarantees we need for durable state:
//!
//! 1. **Atomicity** — a crash mid-write leaves a truncated file. The
//!    next read sees garbage, and code that assumed "file exists
//!    therefore content is valid" panics or silently corrupts.
//!
//! 2. **Durability** — written bytes can sit in the OS page cache for
//!    seconds before reaching the disk. A power loss after `write()`
//!    returned `Ok` can still lose the write.
//!
//! [`atomic_write`] gives both: write to a sibling tempfile, fsync the
//! data, then rename into place. The rename is atomic on POSIX and
//! NTFS; the prior fsync ensures the bytes are on platter before the
//! rename commits, so a crash anywhere in the sequence leaves either
//! the old contents or the new contents — never a half-written file.
//!
//! Used for: auth credentials, memory store entries, memory index
//! files — anywhere a partial write would leave the system in a
//! corrupt state.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Write `contents` to `path` atomically and durably.
///
/// 1. Creates a tempfile in the same directory as `path` (so the
///    rename is same-filesystem and therefore atomic).
/// 2. Writes `contents`, then `sync_all()`s the tempfile so its
///    bytes are on platter before the rename.
/// 3. Renames the tempfile over `path`. POSIX guarantees `rename(2)`
///    is atomic; NTFS provides the same guarantee for files on the
///    same volume.
///
/// On any error before the rename, the original `path` is untouched.
///
/// Does NOT fsync the parent directory after the rename. On most
/// modern filesystems (ext4 with `data=ordered`, xfs, apfs, ntfs)
/// the rename is journalled so the new dentry survives a crash; the
/// extra `fsync(parent_dir)` would block the call by ~1ms for a
/// guarantee the journal already provides.
/// # Long destination paths on Windows (F26-03-D)
///
/// The tempfile round trip below reaches Win32 (`MoveFileExW`) without
/// `std`'s long-path handling, so a destination past `MAX_PATH` (260)
/// fails with `ERROR_PATH_NOT_FOUND` even where `std::fs::write` to the
/// very same path succeeds. Measured on Windows 11 26200 at a 320-char
/// plain `C:\...` path: `create_dir_all` OK, `fs::write` OK,
/// `atomic_write` **os error 3**.
///
/// [`long_path_safe_dest`] resolves the parent to its extended-length
/// (`\\?\`) form before the round trip, which lifts the limit at the
/// Win32 layer regardless of the machine's `LongPathsEnabled` setting.
pub fn atomic_write<P: AsRef<Path>>(path: P, contents: &[u8]) -> std::io::Result<()> {
    let path = path.as_ref();
    let dest = long_path_safe_dest(path)?;
    let dest = dest.as_ref();
    let tmp = staged_temp_file(dest, contents)?;
    tmp.persist(dest).map_err(|e| e.error)
}

/// [`atomic_write`], for a caller that may only publish over a destination
/// whose current contents it has already judged.
///
/// `accept` is handed the bytes the destination held **at the instant the new
/// bytes were published** — `None` if it held nothing — and returning
/// `Err(why)` retracts the publish. `Ok(Err(refusal))` therefore means the
/// destination is exactly as it was and [`Refusal::why`] says what was found
/// instead.
///
/// # Why an exchange and not a re-check
///
/// The obvious shape is to re-read the destination and then rename over it,
/// and that is what this did. It cannot work: the re-read and the rename are
/// two operations, and an editor saving between them is overwritten. Narrowing
/// the gap only lowers the rate — measured at ~6.5% with the check moved as
/// late as it can go, immediately before the rename (#1155).
///
/// [`exchange`] closes it instead of narrowing it. `RENAME_EXCHANGE` swaps the
/// two names in one atomic step, so the bytes that arrive in the temp file
/// *are* the bytes the destination held at the moment of publication — there
/// is no second observation to be stale. A save that lost the race is then
/// detected with certainty and put back by a second exchange.
///
/// # What the exchange does NOT close (#1239)
///
/// It closes the check-then-write race, which is the one #1155 measured: the
/// bytes judged ARE the bytes displaced, so no verdict is ever taken against a
/// stale reading. It does not leave the caller with no window at all. Between
/// the exchange and the verdict the destination NAME resolves to the NEW
/// bytes, and two things can happen in it:
///
/// * a **crash** leaves the new contents published where a re-check would have
///   refused. The destination is never torn — it holds either the old bytes or
///   the new ones at every instant — so this is a change of which whole state
///   survives a crash, not of whether a whole state does;
/// * a **save by a non-cooperating editor** lands on the published inode. The
///   refusal path then puts the original back, which DISPLACES that save. It
///   used to be deleted with the rest of the leftovers, and the refusal handed
///   back was byte-identical to one that had cost nobody anything — the user
///   could not tell a lossy refusal from a clean one. The restore is an
///   exchange too, so it hands that save back; it is now preserved on disk and
///   named in [`Refusal::intercepted_save`].
///
/// The window is therefore narrowed — from `read`→`rename` down to
/// `exchange`→`verdict` — and what remains inside it is accounted for rather
/// than silently discarded.
///
/// Where no exchange primitive exists (see [`exchange`]) this falls back to the
/// re-check, which is racy but no worse than what it replaced.
pub fn atomic_write_checked<P: AsRef<Path>>(
    path: P,
    contents: &[u8],
    accept: impl FnOnce(Option<&[u8]>) -> Result<(), String>,
) -> std::io::Result<Result<(), Refusal>> {
    let path = path.as_ref();
    let dest = long_path_safe_dest(path)?;
    let dest = dest.as_ref();
    let tmp = staged_temp_file(dest, contents)?;

    match publish_displacing(&tmp, dest)? {
        Swap::Displaced(displaced) => {
            // `dest` now names `contents`; `displaced` names what `dest` held
            // at the instant of publication.
            let verdict = match std::fs::read(&displaced) {
                Ok(observed) => accept(Some(&observed)),
                Err(e) => Err(format!("it could no longer be read ({e})")),
            };
            if let Err(why) = verdict {
                // Retract, by the inverse of the step that published.
                let put_back = match restore(&displaced, dest) {
                    Ok(put_back) => put_back,
                    Err(e) => {
                        // The publish stands and the displaced file is the only
                        // copy of what it replaced, so it must not be unlinked.
                        let kept = keep_displaced(tmp, &displaced)?;
                        return Err(std::io::Error::other(RollbackFailed {
                            why,
                            cause: e.to_string(),
                            preserved_at: kept,
                        }));
                    }
                };
                match put_back {
                    // #1239 — the restore is an exchange too, so this name now
                    // holds whatever `dest` held at the instant it ran. That is
                    // `contents` unless somebody saved onto the published inode
                    // INSIDE the exchange→verdict window; unlinking their bytes
                    // is the loss this arm exists to refuse.
                    Some(captured) => {
                        if !holds_exactly(&captured, contents) {
                            let kept = keep_displaced(tmp, &captured)?;
                            return Ok(Err(Refusal {
                                why,
                                intercepted_save: Some(kept),
                            }));
                        }
                        discard_displaced(tmp, &captured);
                    }
                    // The restore overwrote rather than exchanged, so nothing
                    // came back to judge and `displaced` was consumed by it.
                    None => discard_displaced(tmp, &displaced),
                }
                return Ok(Err(Refusal {
                    why,
                    intercepted_save: None,
                }));
            }
            discard_displaced(tmp, &displaced);
            Ok(Ok(()))
        }
        // No exchange to make, or none available. Both fall back to reading the
        // destination and then renaming over it, which is racy — see above.
        //
        // The two are NOT equally routine, and this is the single place the
        // difference is observable, so it is recorded here and nowhere else:
        // `Vacant` means the destination did not exist and there was nothing
        // to lose; `Unsupported` means the publish this module promises was
        // refused and the write is proceeding on the design that #370
        // measured losing 7 of 169 interleaved saves on Windows.
        swap @ (Swap::Vacant | Swap::Unsupported(_)) => {
            if let Swap::Unsupported(why) = &swap {
                note_degraded_publish(dest, why);
            }
            let observed = match std::fs::read(dest) {
                Ok(bytes) => Some(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            if let Err(why) = accept(observed.as_deref()) {
                // Nothing was published, so nothing can have been displaced.
                return Ok(Err(Refusal {
                    why,
                    intercepted_save: None,
                }));
            }
            tmp.persist(dest).map(Ok).map_err(|e| e.error)
        }
    }
}

/// A checked publish that was retracted, and what the retraction cost.
///
/// `Ok(Err(Refusal))` still means what it always meant: the destination is
/// exactly as it was. What it never meant, and what the caller could not see,
/// is that the retraction itself is free. The exchange→verdict window is a
/// window in which the destination name resolves to the NEW bytes, and a
/// non-cooperating editor saving in place during it writes into them; putting
/// the original back displaces that save (#1239).
///
/// [`Self::intercepted_save`] is that distinction, given a name rather than
/// left to the caller to sniff out of a message: `None` — the overwhelmingly
/// common case — means the refusal displaced nothing, and `Some(path)` means
/// somebody else's bytes were taken out of the way and are preserved there.
/// A caller that renders "nothing was changed" off a refusal is telling the
/// truth only in the `None` case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    why: String,
    intercepted_save: Option<PathBuf>,
}

impl Refusal {
    /// What the verdict found instead of the bytes it was given to expect.
    pub fn why(&self) -> &str {
        &self.why
    }

    /// Where a save that arrived inside the exchange→verdict window was
    /// preserved, if there was one.
    pub fn intercepted_save(&self) -> Option<&Path> {
        self.intercepted_save.as_deref()
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.intercepted_save {
            Some(at) => write!(
                f,
                "{}; a save made while the check was running was displaced by \
                 putting the original back and is preserved at {}",
                self.why,
                at.display()
            ),
            None => f.write_str(&self.why),
        }
    }
}

/// The guard reached a verdict, the verdict REFUSED, and the publish could not
/// be retracted.
///
/// This is emphatically NOT "the write never happened", which is the other
/// meaning [`atomic_write_checked`] can return `Err` with. The new bytes are
/// published at the destination and the pre-image survives only under
/// [`Self::preserved_at`]. `WriteTool`'s direct path read every `Err` as the
/// first meaning, republished the bytes unchecked and reported success for a
/// write its own guard had refused (#1241), so the two meanings are now told
/// apart by TYPE — recover it with [`rollback_failure`] — rather than by
/// matching on the message text.
#[derive(Debug)]
pub struct RollbackFailed {
    why: String,
    cause: String,
    preserved_at: PathBuf,
}

impl RollbackFailed {
    /// What the verdict found, i.e. why the publish was refused.
    pub fn why(&self) -> &str {
        &self.why
    }

    /// Where the pre-image — the destination's contents before the refused
    /// publish — is preserved. Nothing else holds a copy.
    pub fn preserved_at(&self) -> &Path {
        &self.preserved_at
    }
}

impl std::fmt::Display for RollbackFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Unchanged wording: #1202's test reads the preserved path back out of
        // this text, and so does anything already logging it.
        write!(
            f,
            "{}, and the original could not be put back ({}); it is preserved at {}",
            self.why,
            self.cause,
            self.preserved_at.display()
        )
    }
}

impl std::error::Error for RollbackFailed {}

/// Recover a [`RollbackFailed`] from the `io::Error` [`atomic_write_checked`]
/// wrapped it in, or `None` if the error is the other kind — a tempfile round
/// trip that never reached a verdict at all.
///
/// The one supported way to tell the two apart. Sniffing the message text is
/// not: it makes the wording load-bearing, and it fails open — an unrecognised
/// string reads as "never reached a verdict", which is the answer that
/// republishes unchecked.
pub fn rollback_failure(error: &std::io::Error) -> Option<&RollbackFailed> {
    error.get_ref()?.downcast_ref::<RollbackFailed>()
}

/// Does the file at `path` hold exactly these bytes?
///
/// Unreadable counts as "no". This decides whether a displaced file is the
/// copy of `contents` this module published a moment ago (discard it) or
/// somebody else's save (keep it), and a file that cannot be read is not
/// provably ours — on a data-loss guard the fail-safe answer is to keep.
fn holds_exactly(path: &Path, contents: &[u8]) -> bool {
    std::fs::read(path).is_ok_and(|bytes| bytes == contents)
}

/// A sibling temp file holding `contents`, fsynced, and already wearing the
/// destination's permission bits so that publishing it never redefines them.
fn staged_temp_file(dest: &Path, contents: &[u8]) -> std::io::Result<tempfile::TempPath> {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(contents)?;
    tmp.as_file().sync_all()?;
    // B6 — carry the destination's own mode onto the temp file BEFORE it is
    // published, so the name is never republished with the tempfile's 0600.
    carry_destination_mode(&tmp, dest);
    Ok(tmp.into_temp_path())
}

/// What an attempt to publish-and-displace did.
///
/// Where there is no primitive that hands the displaced bytes back,
/// [`publish_displacing`] can only ever answer `Unsupported` and the other two
/// are unconstructible. They are still the right shape for the outcome, so the
/// enum is kept whole rather than split into a platform-specific one.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", windows)),
    allow(dead_code)
)]
enum Swap {
    /// The new bytes are published at the destination, and the payload names a
    /// file holding exactly what the destination held at that instant.
    ///
    /// On the exchange platforms that file IS the staged temp file, which now
    /// names the pre-image. On Windows it is a separate sibling backup, which
    /// is why this carries the path instead of the caller assuming it.
    Displaced(PathBuf),
    /// The destination does not exist, so there was nothing to displace.
    Vacant,
    /// This platform, kernel or filesystem has no such primitive, or the
    /// one it has REFUSED this call. The payload names which, verbatim from
    /// the OS where there is an OS answer.
    ///
    /// The reason is carried rather than dropped because the two are not the
    /// same event and only one of them is routine. "Windows has no exchange
    /// primitive" is a property of the platform; `ReplaceFileW` returning
    /// `ERROR_SHARING_VIOLATION` because the user's editor holds the
    /// destination open is a property of THIS write, and it is the case in
    /// which the fallback below can lose the editor's bytes (#370). A caller
    /// that cannot tell them apart cannot report either.
    Unsupported(String),
}

/// How many publishes have degraded from the exchange primitive to the racy
/// check-then-rename fallback since this process started.
///
/// Process-global rather than per-write because the caller of
/// [`atomic_write_checked`] is not the party who needs to know: the operator
/// is. See [`degraded_publish_count`].
static DEGRADED_PUBLISHES: AtomicU64 = AtomicU64::new(0);

/// The number of times [`atomic_write_checked`] has fallen back to the racy
/// publish in this process.
///
/// **This exists because the degrade used to be invisible.** Every failure of
/// `ReplaceFileW` — including the `ERROR_SHARING_VIOLATION` an open editor
/// produces, which is the reported scenario — answered [`Swap::Unsupported`]
/// and the write silently continued on check-then-rename, the design measured
/// losing 7 of 169 interleaved saves on Windows (`#370`). Nothing counted it,
/// nothing logged it, and no caller could see it, so "the guarantee holds on
/// Windows" was unfalsifiable rather than true.
///
/// A counter and not a `Result`: the fallback still publishes the caller's
/// bytes, so failing the write here would turn a rare loss into a common
/// refusal. What the caller loses is the STRENGTH of the guarantee, and that
/// is an operator-visible fact, not a per-call error.
pub fn degraded_publish_count() -> u64 {
    DEGRADED_PUBLISHES.load(Ordering::Relaxed)
}

/// Record one degrade, both for the operator (log) and for a test (counter).
///
/// `error!` and not `warn!`: with `RUST_LOG` unset only ERROR reaches stderr,
/// so a warning here would satisfy the letter of "logged" while remaining
/// exactly as invisible as the silence it replaces.
fn note_degraded_publish(dest: &Path, why: &str) {
    DEGRADED_PUBLISHES.fetch_add(1, Ordering::Relaxed);
    tracing::error!(
        target: "wcore_config::atomic_io",
        dest = %dest.display(),
        why = %why,
        "the publish-and-displace primitive was refused, so this write fell back \
         to check-then-rename, which can lose a save that arrives inside the \
         check window (FerroxLabs/wayland-core#370)"
    );
}

/// Put `displaced` back at `dest`, undoing a [`Swap::Displaced`] publish, and
/// hand back the name under which whatever `dest` held AT THE INSTANT OF THE
/// RESTORE now lives.
///
/// The inverse of whichever primitive published, so it inherits that
/// primitive's atomicity: a second `RENAME_EXCHANGE` / `RENAME_SWAP` where the
/// publish was one, and `ReplaceFileW` again on Windows where the publish was
/// `ReplaceFileW`.
///
/// Because it is an exchange, it is also an OBSERVATION, and that is what the
/// returned path is for (#1239). It should hold the bytes this module
/// published; anything else is a save that landed inside the exchange→verdict
/// window, and the caller must not unlink it. `None` means the restore
/// overwrote rather than exchanged, so there was nothing to hand back.
///
/// A non-exchange is a FAILURE, not a rollback (#1202). `publish_displacing`
/// answers `Vacant` when the destination name no longer resolves and
/// `Unsupported` when the primitive is unavailable; in both the publish still
/// stands and `displaced` is the only surviving copy of the pre-image.
/// Discarding the [`Swap`] discriminant here reported those as a clean
/// rollback, so the caller unlinked that copy and told the user the
/// destination was untouched — silent data loss behind a false refusal, which
/// is the fail-open this module refuses everywhere else.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn restore(displaced: &Path, dest: &Path) -> std::io::Result<Option<PathBuf>> {
    match publish_displacing(displaced, dest)? {
        Swap::Displaced(exchanged_out) => Ok(Some(exchanged_out)),
        Swap::Vacant => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the destination name no longer exists, so nothing was exchanged back",
        )),
        Swap::Unsupported(why) => Err(std::io::Error::other(format!(
            "the exchange primitive that published is no longer available, \
             so nothing was exchanged back ({why})"
        ))),
    }
}

/// Windows restores with `ReplaceFileW` again rather than with a plain
/// replacing rename, for the same reason the publish uses it: the rename
/// DESTROYS whatever the destination holds, and inside the exchange→verdict
/// window that can be somebody's save (#1239). `ReplaceFileW` moves it to a
/// backup name instead, which is the observation this returns.
///
/// The plain rename is kept as the fallback for the two answers that are not
/// an exchange. `Vacant` — the destination name has gone — is a genuine
/// restore there rather than the failure it is on the exchange platforms: a
/// replacing rename against an absent destination simply puts the pre-image
/// back, which is what this platform has always done and what #1202's test
/// asserts. Nothing was displaced in that case, so there is nothing to return.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn restore(displaced: &Path, dest: &Path) -> std::io::Result<Option<PathBuf>> {
    match publish_displacing(displaced, dest)? {
        Swap::Displaced(exchanged_out) => Ok(Some(exchanged_out)),
        Swap::Vacant | Swap::Unsupported => std::fs::rename(displaced, dest).map(|()| None),
    }
}

/// Drop the leftovers of a completed publish.
///
/// `tmp` guards the staged file. On the exchange platforms `displaced` IS that
/// path (the exchange moved the pre-image into it), so dropping the guard is
/// the whole job. On Windows the staged file was consumed by `ReplaceFileW`
/// and `displaced` is a separate backup this module created, so it has to be
/// unlinked explicitly — the `TempPath` drop then finds nothing and is a no-op.
fn discard_displaced(tmp: tempfile::TempPath, displaced: &Path) {
    if displaced != &*tmp {
        let _ = std::fs::remove_file(displaced);
    }
    drop(tmp);
}

/// Keep a displaced file on disk and name it, for the two paths where it is
/// the only surviving copy of somebody's bytes: a publish that stands because
/// the rollback failed (#1202), and a save that the rollback itself displaced
/// (#1239).
fn keep_displaced(tmp: tempfile::TempPath, displaced: &Path) -> std::io::Result<PathBuf> {
    if displaced == &*tmp {
        return Ok(tmp.keep()?);
    }
    drop(tmp);
    Ok(displaced.to_path_buf())
}

/// Publish `a` over `b` and hand back a name holding what `b` held at that
/// instant. The one place the platform difference lives.
///
/// - **Linux** — `renameat2(RENAME_EXCHANGE)`, since 3.15. Invoked as a raw
///   syscall rather than through the glibc wrapper, which arrived in 2.28 and
///   does not exist on musl at all. The displaced name is `a` itself.
/// - **macOS** — `renamex_np(RENAME_SWAP)`, since 10.12. APFS and HFS+ only.
///   The displaced name is `a` itself.
/// - **Windows** — `ReplaceFileW` with a backup name (#1155's second
///   residual). Win32 has no exchange, and this is not one: `ReplaceFileW`
///   renames the replaced file to `lpBackupFileName` and then renames the
///   replacement into its place, so unlike `RENAME_EXCHANGE` there is an
///   instant at which the destination name does not resolve. What it DOES
///   give is the property the verdict actually needs — the displaced bytes,
///   under a name this module chose — so the check becomes an observation
///   taken AFTER publication rather than a re-read taken before it, and the
///   check-then-write window closes the same way it does elsewhere. The
///   earlier reading, recorded in this file, that `ReplaceFile` "does not
///   publish the displaced file under a name the caller chooses" was simply
///   wrong about `lpBackupFileName`.
///
///   **Ungraded on Windows by the lane that wrote it.** It is reachable only
///   on a platform this workspace cannot execute from Linux, so it ships
///   verified by `cargo check --target x86_64-pc-windows-gnu` and by
///   [`tests::the_check_is_handed_the_bytes_the_publish_displaced`], whose
///   Windows arm now asserts the post-publication reading and will fail the
///   Windows CI job if any of the above is wrong.
///
///   **THE WINDOWS GUARANTEE, DECLARED (#370).** It is weaker than the unix
///   one and this is the sentence that says so, because the alternative —
///   `#342` c3's "the same guarantee holds on Windows, where the product
///   ships" — was measured false. `ReplaceFileW` succeeding gives the full
///   guarantee. `ReplaceFileW` FAILING does not, and it fails for ordinary
///   reasons: an editor holding the destination open without
///   `FILE_SHARE_DELETE` gives `ERROR_SHARING_VIOLATION`, and every failure
///   degrades to check-then-rename, which loses a save that arrives inside
///   the check window. Measured on Windows 11 build 26200 at `retries = 0`:
///   **7 of 169** interleaved saves lost on the Edit path (4.1%), **1 of 144**
///   on the VFS path (0.7%), and in 4 of 24 executions the editor's own
///   `rename` was instead refused outright with `ERROR_ACCESS_DENIED`.
///
///   RE-MEASURED 2026-08-30 on the same host, AFTER `FerroxLabs/wayland`#1202
///   changed `Swap` semantics on this exact path, N = 20 per arm at
///   `retries = 0` with the Windows `ignore` forced: the Edit arm was red in
///   **6 of 20** and lost **3 of 302** interleaved saves (1.0%); the VFS arm
///   was red in **8 of 20** and lost **1 of 219** (0.5%); the other 11 reds
///   were the editor rename refused outright. 14 of 40 executions red. The
///   rates moved; the GUARANTEE did not, and neither did the direction of
///   this declaration.
///
///   So what Windows ships is: *every degrade is COUNTED, and logged at
///   `error!` before the racy publish runs.* That is the property
///   `a_refused_replacefilew_is_counted_and_not_silent` grades, and it is
///   what a caller on Windows may rely on. It is NOT "no save is lost".
///
///   **AND IT IS NOT "the operator is always told".** An earlier draft of this
///   paragraph said the window "is always announced"; that was measured false
///   in two ways at once and is corrected here rather than deleted, because a
///   declaration that overstates is the exact defect `#370` was split out of
///   `#342` to fix, and repeating it one notch smaller would be the same
///   mistake:
///
///   * `degraded_publish_count()` has NO production caller. Nothing in the
///     shipped product reads it; the only references outside these docs are in
///     its own unit test. A counter no code reads is a number, not an
///     observable, and calling it one is how `#342` c3 happened.
///   * the `error!` reaches an operator only where tracing reaches stdio. In
///     the TUI it does not: `wcore-cli/src/main.rs` routes tracing to a
///     non-blocking FILE writer whenever the alt-screen is entered, because
///     the alt-screen owns the terminal and nothing may reach stdio, not even
///     an error. So in the TUI the announcement is in a log file the operator
///     has no reason to open, and under the JSON stream protocol it is on a
///     stderr the host may be discarding.
///
///   `#370`'s own contract offered three options — counted, logged where the
///   operator actually sees it, or surfaced in the TOOL RESULT. Only the first
///   is shipped. The third is the one that survives all three output modes and
///   it is NOT taken here; the residual stays on `#370` rather than being
///   written off by a sentence that implies it was done.
///
/// - **Everything else** — [`Swap::Unsupported`], and the caller falls back to
///   re-check-then-rename.
///
/// Any failure other than a missing destination degrades to
/// [`Swap::Unsupported`] rather than failing the write: the fallback is what
/// every platform did before this existed, so the worst case of a primitive
/// that misbehaves is the previous behaviour, never a lost write.
#[cfg(target_os = "linux")]
fn publish_displacing(a: &Path, b: &Path) -> std::io::Result<Swap> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    fn c(p: &Path) -> std::io::Result<CString> {
        CString::new(p.as_os_str().as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
    }
    let (ca, cb) = (c(a)?, c(b)?);

    // SAFETY: both pointers are NUL-terminated C strings owned by locals that
    // outlive the call, and AT_FDCWD is the documented "resolve relative to the
    // working directory" sentinel for the *at family.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_renameat2 as libc::c_long,
            libc::AT_FDCWD,
            ca.as_ptr(),
            libc::AT_FDCWD,
            cb.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if rc == 0 {
        return Ok(Swap::Displaced(a.to_path_buf()));
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ENOENT) => Ok(Swap::Vacant),
        // ENOSYS: kernel older than 3.15. EINVAL / EOPNOTSUPP: the filesystem
        // does not implement the flag.
        Some(libc::ENOSYS) | Some(libc::EINVAL) | Some(libc::EOPNOTSUPP) => Ok(Swap::Unsupported(
            format!("renameat2(RENAME_EXCHANGE): {err}"),
        )),
        _ => Err(err),
    }
}

#[cfg(target_os = "macos")]
fn publish_displacing(a: &Path, b: &Path) -> std::io::Result<Swap> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    fn c(p: &Path) -> std::io::Result<CString> {
        CString::new(p.as_os_str().as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
    }
    let (ca, cb) = (c(a)?, c(b)?);

    // SAFETY: as above — both pointers are NUL-terminated C strings owned by
    // locals that outlive the call.
    let rc = unsafe { libc::renamex_np(ca.as_ptr(), cb.as_ptr(), libc::RENAME_SWAP) };
    if rc == 0 {
        return Ok(Swap::Displaced(a.to_path_buf()));
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ENOENT) => Ok(Swap::Vacant),
        // A volume without VOL_CAP_INT_RENAME_SWAP: FAT, SMB, and others.
        Some(libc::ENOTSUP) | Some(libc::EOPNOTSUPP) | Some(libc::EINVAL) => {
            Ok(Swap::Unsupported(format!("renamex_np(RENAME_SWAP): {err}")))
        }
        _ => Err(err),
    }
}

/// `ReplaceFileW(dest, replacement, backup)` — see [`publish_displacing`]'s
/// doc for why this is the Windows analogue and what it does not promise.
///
/// The backup name is derived from the staged temp file's own (already random,
/// already unique) name, so it is a sibling on the same volume — `ReplaceFileW`
/// requires that — without creating a second file to reserve a name.
#[cfg(windows)]
fn publish_displacing(a: &Path, b: &Path) -> std::io::Result<Swap> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        REPLACEFILE_IGNORE_ACL_ERRORS, REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW,
    };

    /// `ERROR_FILE_NOT_FOUND`. Spelled out rather than imported so the mapping
    /// to [`Swap::Vacant`] reads at the match arm.
    const ERROR_FILE_NOT_FOUND: i32 = 2;

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    let Some(stem) = a.file_name() else {
        return Ok(Swap::Unsupported(
            "the staged file has no file name, so no sibling backup name can be derived".to_owned(),
        ));
    };
    let mut backup_name = stem.to_os_string();
    backup_name.push(".wl-displaced");
    let backup = a.with_file_name(backup_name);
    // ReplaceFileW moves the replaced file ONTO this name; a stale file there
    // from a killed run must not be mistaken for the pre-image.
    let _ = std::fs::remove_file(&backup);

    // SAFETY: all three pointers are NUL-terminated UTF-16 buffers owned by
    // locals that outlive the call, and both reserved parameters are the
    // documented NULL.
    let ok = unsafe {
        ReplaceFileW(
            wide(b).as_ptr(),
            wide(a).as_ptr(),
            wide(&backup).as_ptr(),
            REPLACEFILE_IGNORE_MERGE_ERRORS | REPLACEFILE_IGNORE_ACL_ERRORS,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok != 0 {
        return Ok(Swap::Displaced(backup));
    }

    let err = std::io::Error::last_os_error();
    // ReplaceFileW documents partial failures in which the replaced file has
    // already been moved aside. Put it back before degrading, so the caller's
    // fallback publishes over the destination rather than over a hole.
    if backup.exists() && !b.exists() {
        let _ = std::fs::rename(&backup, b);
    }
    let _ = std::fs::remove_file(&backup);
    if err.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) {
        return Ok(Swap::Vacant);
    }
    Ok(Swap::Unsupported(format!("ReplaceFileW: {err}")))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn publish_displacing(_a: &Path, _b: &Path) -> std::io::Result<Swap> {
    Ok(Swap::Unsupported(
        "this target has no publish-and-displace primitive".to_owned(),
    ))
}

/// Copy an EXISTING destination's permission bits onto the temp file that is
/// about to replace it (B6).
///
/// `NamedTempFile` creates its file 0600 by design, and the rename carries
/// that mode onto the destination name. Every rewrite of an existing file
/// therefore redefined its permissions: a 0755 script edited by the agent came
/// back 0600 and the agent's own next turn got `Exit code: 126 … Permission
/// denied`; a 0644 file silently lost group/other read.
///
/// Only an existing destination is matched. A file this helper CREATES keeps
/// the private 0600 — credentials, the session mirror and the memory store are
/// all written through here, and widening a new file would be a security
/// change nobody asked for.
///
/// Best-effort: a failure to read or apply the mode must not fail the write,
/// which would turn a cosmetic permission problem into data loss.
#[cfg(unix)]
fn carry_destination_mode(tmp: &tempfile::NamedTempFile, dest: &Path) {
    use std::os::unix::fs::PermissionsExt;

    if let Ok(existing) = std::fs::metadata(dest) {
        // Mask to the permission/setuid/sticky bits; `mode()` also carries the
        // file-type bits, which are not the kernel's to take from a chmod.
        let bits = existing.permissions().mode() & 0o7777;
        let _ = tmp
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(bits));
    }
}

/// Windows has no POSIX mode. The nearest analogue, the read-only ATTRIBUTE,
/// must NOT be copied onto the temp file: `MoveFileExW(REPLACE_EXISTING)`
/// refuses a read-only participant, so carrying it would turn today's silent
/// permission change into a failed write. Left as a no-op deliberately rather
/// than by omission.
#[cfg(not(unix))]
fn carry_destination_mode(_tmp: &tempfile::NamedTempFile, _dest: &Path) {}

/// Length past which a destination is rewritten to extended-length form on
/// Windows. Well below `MAX_PATH` (260) so there is headroom for the sibling
/// temp file's own name, which is what the round trip actually creates.
#[cfg(windows)]
const WINDOWS_LONG_PATH_THRESHOLD: usize = 200;

/// The single place platform long-path handling lives, per the
/// centralize-platform-differences rule. On unix this is the identity.
///
/// Only paths at or past the threshold are rewritten, so the ordinary short
/// path keeps exactly its present behavior and the change is bounded to the
/// case that is broken today.
#[cfg(windows)]
fn long_path_safe_dest(path: &Path) -> std::io::Result<std::borrow::Cow<'_, Path>> {
    use std::borrow::Cow;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let too_long = path.as_os_str().len() >= WINDOWS_LONG_PATH_THRESHOLD
        || parent.as_os_str().len() >= WINDOWS_LONG_PATH_THRESHOLD;
    if !too_long || path.to_string_lossy().starts_with(r"\\?\") {
        return Ok(Cow::Borrowed(path));
    }
    let Some(file_name) = path.file_name() else {
        return Ok(Cow::Borrowed(path));
    };
    // `canonicalize` is how std spells "extended-length form" on Windows. The
    // parent must already exist for the tempfile round trip to work at all, so
    // resolving it here costs one metadata call and no new failure mode: if it
    // cannot be resolved the original path is used and the caller sees the same
    // error it would have seen anyway.
    match std::fs::canonicalize(parent) {
        Ok(canon) => Ok(Cow::Owned(canon.join(file_name))),
        Err(_) => Ok(Cow::Borrowed(path)),
    }
}

#[cfg(not(windows))]
#[inline]
fn long_path_safe_dest(path: &Path) -> std::io::Result<std::borrow::Cow<'_, Path>> {
    // unix has no MAX_PATH equivalent at this scale (PATH_MAX is 4096) and no
    // extended-length form, so there is nothing to rewrite.
    Ok(std::borrow::Cow::Borrowed(path))
}

/// Build a path under `base` whose total length exceeds Windows' `MAX_PATH`
/// (260), using nesting rather than one long component so that every individual
/// component stays inside the 255-byte per-component limit every filesystem
/// enforces.
///
/// Shared by the tests below and by `wcore-cli`'s backup long-path proof, so
/// both measure the same shape rather than two hand-rolled approximations that
/// could drift apart and disagree about what "deep" means.
///
/// `sep` selects the separator used to build the RELATIVE part. This is the
/// variable under test, not a detail: a backup manifest records payload paths
/// `/`-separated (`path-norm=slash-relative`), so the restore reconstructs a
/// target path by joining ONE forward-slash string onto the target root. A
/// fixture built with `\` is a different shape and, measured, does not reach
/// the defect.
#[doc(hidden)]
pub fn deep_path_over_max_path(base: &Path, leaf: &str, sep: char) -> std::path::PathBuf {
    const WINDOWS_MAX_PATH: usize = 260;
    let mut rel = String::new();
    // Components of 40 chars each, each well inside the 255-byte per-component
    // limit every filesystem enforces, nested until the total is past 260.
    while base.as_os_str().len() + rel.len() + leaf.len() + 2 <= WINDOWS_MAX_PATH + 40 {
        rel.push_str("d234567890123456789012345678901234567890");
        rel.push(sep);
    }
    rel.push_str(leaf);
    // A single join of the whole relative string, exactly as the restore write
    // loop does it (`target.join(&entry.path)`).
    base.join(rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F26-03-D. The Windows restore failed with `os error 3` past `MAX_PATH`
    /// while `create` accepted the same tree.
    ///
    /// The separator is the variable, and it is the whole point. A first
    /// version of this test built the deep path with `\` and PASSED on Windows
    /// at 306 characters — every leg green, defect untouched. The restore does
    /// not build paths that way: a manifest records payload paths
    /// `/`-separated, so the write loop joins one forward-slash string onto the
    /// target root. The `\` leg is kept as a CONTROL: it proves the harness,
    /// the box and the length regime are all fine, so a failure on the `/` leg
    /// isolates the separator rather than the depth.
    ///
    /// Each of the three calls the restore path makes is measured separately —
    /// `create_dir_all`, plain `fs::write`, `atomic_write` — because they fail
    /// for different reasons and a combined check would hide which.
    ///
    /// On unix this is already green against the untouched tree: `PATH_MAX` is
    /// 4096 and `/` is the native separator, so neither variable exists there.
    /// The unix run proves the fixture is well formed; only the Windows run is
    /// a gate.
    #[test]
    fn atomic_write_survives_a_path_past_windows_max_path() {
        // A PLAIN absolute base, deliberately NOT `canonicalize()`d. On Windows
        // `canonicalize()` returns an extended-length `\\?\` path, which is
        // already in the form that lifts the 260 limit — a base built that way
        // measures the working mode and can never reach the defect. A restore
        // target arrives from the command line as a plain `C:\...` path, so
        // that is what the fixture must use.
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_path_buf();
        assert!(
            !base.to_string_lossy().starts_with(r"\\?\"),
            "fixture base is already verbatim, so it cannot reach the defect: {}",
            base.display()
        );

        // The control leg runs FIRST. If it fails, the box or the harness is
        // the problem and the `/` result would be uninterpretable.
        //
        // The control uses the platform's NATIVE separator, not a literal `\`.
        // On unix `\` is an ordinary filename character, so a `\`-built fixture
        // is one 300-byte component rather than a nested path, and it fails
        // with ENAMETOOLONG against `NAME_MAX` — a fixture defect that says
        // nothing about `atomic_write`. Measured: it did exactly that on Linux.
        // On unix the two legs are therefore the same shape, and the test
        // honestly degenerates to a single measurement there; the separator
        // distinction only exists on Windows.
        let native = std::path::MAIN_SEPARATOR;
        for (label, sep) in [
            ("control-native-sep", native),
            ("subject-forwardslash", '/'),
        ] {
            let root = base.join(label);
            std::fs::create_dir_all(&root).unwrap();
            let deep = deep_path_over_max_path(&root, "payload.txt", sep);
            let len = deep.as_os_str().len();
            assert!(
                len > 260,
                "[{label}] fixture too shallow to reach the defect: {len} chars"
            );

            let parent = deep.parent().unwrap();
            let mkdir = std::fs::create_dir_all(parent);
            println!("LONGPATH[{label}]-LEN: {len}");
            println!(
                "LONGPATH[{label}]-CREATE-DIR-ALL: {:?}",
                mkdir.as_ref().err()
            );
            mkdir.unwrap_or_else(|e| panic!("[{label}] create_dir_all failed past MAX_PATH: {e}"));

            // Plain `fs::write` goes through std's own long-path handling;
            // `atomic_write` additionally goes through the tempfile round trip.
            // Measuring both tells us which layer is responsible.
            let plain = parent.join("plain.txt");
            let plain_res = std::fs::write(&plain, b"plain");
            println!(
                "LONGPATH[{label}]-STD-WRITE: {:?}",
                plain_res.as_ref().err()
            );

            let atomic_res = atomic_write(&deep, b"atomic");
            println!(
                "LONGPATH[{label}]-ATOMIC-WRITE: {:?}",
                atomic_res.as_ref().err()
            );

            plain_res.unwrap_or_else(|e| panic!("[{label}] fs::write failed past MAX_PATH: {e}"));
            atomic_res
                .unwrap_or_else(|e| panic!("[{label}] atomic_write failed past MAX_PATH: {e}"));
            assert_eq!(std::fs::read(&deep).unwrap(), b"atomic");
            // It must REPLACE as well as create — persisting over an existing
            // long path is a different Win32 call shape from creating one.
            atomic_write(&deep, b"atomic-2").unwrap();
            assert_eq!(std::fs::read(&deep).unwrap(), b"atomic-2");
        }
    }

    /// #1155. The pre-image handed to the check is the one the publish
    /// DISPLACED, not one read back from the path afterwards, and the
    /// difference is observable: by the time the check runs, the destination
    /// already holds the new bytes.
    ///
    /// This is the whole mechanism, so it is asserted directly rather than
    /// only through the concurrent arm in `wcore-tools`, which can only ever
    /// sample the race.
    #[test]
    fn the_check_is_handed_the_bytes_the_publish_displaced() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, b"old").unwrap();

        let mut seen: Option<Vec<u8>> = None;
        let mut on_disk_during = Vec::new();
        let r = atomic_write_checked(&p, b"new", |observed| {
            seen = observed.map(<[u8]>::to_vec);
            on_disk_during = std::fs::read(&p).unwrap();
            Ok(())
        })
        .unwrap();

        assert!(r.is_ok());
        assert_eq!(seen.as_deref(), Some(&b"old"[..]), "displaced bytes");
        // The publish has ALREADY happened when the check runs, so the
        // destination reads as the new bytes and a refusal is a rollback.
        //
        // This assertion used to be split: Windows had no primitive that gave
        // the displaced bytes back, degraded to re-check-then-rename, and this
        // arm asserted `b"old"` -- i.e. it asserted that the #1155 race stayed
        // open there. `publish_displacing` now uses `ReplaceFileW` with a
        // backup name, so the property is the same on every platform and this
        // is one assertion again. It is ALSO the grading instrument for that
        // Windows path, which no Linux host can execute: if `ReplaceFileW`
        // does not behave as `publish_displacing`'s doc claims, this fails in
        // the Windows CI job.
        assert_eq!(
            on_disk_during, b"new",
            "the publish precedes the check, so the check is handed what it displaced"
        );
        assert_eq!(std::fs::read(&p).unwrap(), b"new");
    }

    /// #370 NEGATIVE CONTROL — the silent degrade is OBSERVABLE when it fires.
    ///
    /// The ticket's contract is that `Swap::Unsupported` on Windows must stop
    /// being silent. A test that only asserted the counter's existence would
    /// pass with the counter never reached, so this REPRODUCES the reported
    /// scenario rather than modelling it: an editor's handle on the
    /// destination, shared for read and write but NOT for delete.
    /// `ReplaceFileW` has to rename that destination aside, which needs
    /// DELETE access, so the kernel refuses it with `ERROR_SHARING_VIOLATION`
    /// and the publish degrades — the exact path #370 measured losing bytes.
    ///
    /// `>` and not `+ 1` deliberately: the counter is process-global and the
    /// unit tests in this binary run in parallel, so a sibling degrading
    /// concurrently must not redden this. Monotonic and never reset, so `>`
    /// still fails if `note_degraded_publish` stops counting — which is the
    /// mutation this arm exists to catch.
    #[cfg(windows)]
    #[test]
    fn a_refused_replacefilew_is_counted_and_not_silent() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_SHARE_READ,
            FILE_SHARE_WRITE, OPEN_EXISTING,
        };

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("held.txt");
        std::fs::write(&p, b"theirs").unwrap();

        let wide: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 buffer owned by a local
        // that outlives the call; both pointer parameters are the documented
        // NULL for "no security attributes" and "no template file".
        let held = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        assert!(
            held != INVALID_HANDLE_VALUE,
            "fixture: the destination could not be held open ({}), so nothing \
             below reproduces the sharing violation",
            std::io::Error::last_os_error()
        );

        let before = degraded_publish_count();
        // The outcome is deliberately not asserted. With the destination held
        // without FILE_SHARE_DELETE the fallback's own `persist` is refused
        // too, which is #370's SECOND Windows failure (the editor's save gets
        // `ERROR_ACCESS_DENIED` rather than losing bytes). What is asserted is
        // the property the ticket asks for: the degrade was not silent.
        let _outcome = atomic_write_checked(&p, b"ours", |_| Ok(()));
        let after = degraded_publish_count();
        // SAFETY: `held` is a live handle this test opened and has not closed.
        unsafe {
            CloseHandle(held);
        }

        assert!(
            after > before,
            "a refused ReplaceFileW fell back to the racy publish without \
             counting it: degraded_publish_count stayed at {before}"
        );
    }

    /// A refused publish leaves the destination byte-identical. The publish has
    /// already happened when the check runs, so this exercises the rollback,
    /// not a skipped write.
    #[test]
    fn a_refused_publish_puts_the_destination_back() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, b"theirs").unwrap();
        let before = std::fs::metadata(&p).unwrap();

        let r = atomic_write_checked(&p, b"ours", |_| Err("changed".to_owned())).unwrap();

        let refusal = r.expect_err("the publish was not refused");
        assert_eq!(refusal.why(), "changed");
        // The control for #1239: nobody else wrote, so the retraction cost
        // nothing and the refusal says so.
        assert_eq!(refusal.intercepted_save(), None);
        assert_eq!(std::fs::read(&p).unwrap(), b"theirs");
        // The rolled-back destination is the ORIGINAL file, not a copy of it:
        // an editor holding it open must not find itself writing to an inode
        // nothing points at any more.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().ino(),
                before.ino(),
                "the rollback published a different inode at the destination"
            );
        }
        let _ = before;
        // And no temp file is left behind.
        let strays: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n != "f.txt")
            .collect();
        assert!(strays.is_empty(), "left behind {strays:?}");
    }

    /// #1239. A save that lands inside the exchange→verdict window is
    /// displaced by the rollback, and used to be deleted with the leftovers —
    /// leaving a refusal byte-identical to one that had cost nobody anything.
    ///
    /// The window is driven directly rather than sampled: the check closure IS
    /// the window. It runs after the exchange has published and before the
    /// verdict is acted on, so a `std::fs::write` made from inside it lands
    /// exactly where a non-cooperating editor's in-place save would — on the
    /// inode the destination NAME currently resolves to, which is the one this
    /// module is about to exchange back out and drop.
    ///
    /// Three arms, as the report measured them:
    ///
    /// * `ARM_A` — the save happens, the verdict refuses. The save must not
    ///   vanish, and the refusal must not read like `ARM_B`'s.
    /// * `ARM_B` — CONTROL. Nobody else writes; the same verdict refuses. The
    ///   refusal here is the honest "this cost nothing" one, and the two being
    ///   indistinguishable was half the defect.
    /// * `ARM_C` — SENSITIVITY CONTROL. The identical save, with a verdict
    ///   that ACCEPTS. It survives and is published, which proves the probe
    ///   can observe survival at all and that the loss in `ARM_A` is
    ///   attributable to the rollback rather than to the exchange or the
    ///   fixture.
    #[test]
    fn a_save_made_inside_the_verdict_window_survives_the_rollback() {
        const ORIGINAL: &[u8] = b"original";
        const OURS: &[u8] = b"ours";
        const THEIRS: &[u8] = b"THEIRSAVE";

        /// One arm. `save` is the concurrent in-place save, made from inside
        /// the window; `refuse` is the verdict.
        fn arm(
            save: bool,
            refuse: bool,
        ) -> (tempfile::TempDir, std::io::Result<Result<(), Refusal>>) {
            let dir = tempfile::tempdir().unwrap();
            let p = dir.path().join("f.txt");
            std::fs::write(&p, ORIGINAL).unwrap();
            let outcome = atomic_write_checked(&p, OURS, |observed| {
                assert_eq!(
                    observed,
                    Some(ORIGINAL),
                    "fixture: the check was not handed the displaced pre-image"
                );
                if save {
                    // In place: truncate and rewrite the SAME inode, which is
                    // what an editor that does not unlink does, and which is
                    // the inode the destination name points at right now.
                    std::fs::write(&p, THEIRS).unwrap();
                }
                if refuse {
                    Err("changed under write".to_owned())
                } else {
                    Ok(())
                }
            });
            (dir, outcome)
        }

        fn surviving(dir: &Path, bytes: &[u8]) -> Vec<PathBuf> {
            std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|f| std::fs::read(f).is_ok_and(|b| b == bytes))
                .collect()
        }

        // ARM_C first: it is the control that proves the probe can see a
        // concurrent save survive. If it fails, nothing below means anything.
        let (dir_c, out_c) = arm(true, false);
        assert_eq!(
            out_c.expect("ARM_C: the round trip failed").as_ref(),
            Ok(&()),
            "ARM_C: the accepting verdict did not publish"
        );
        assert_eq!(
            std::fs::read(dir_c.path().join("f.txt")).unwrap(),
            THEIRS,
            "ARM_C: the probe cannot observe a concurrent save surviving, so \
             it cannot testify to one being destroyed either"
        );

        // ARM_B: nobody else wrote. The refusal is honest and costs nothing.
        let (dir_b, out_b) = arm(false, true);
        let ref_b = out_b
            .expect("ARM_B: the round trip failed")
            .expect_err("ARM_B: the publish was not refused");
        assert_eq!(ref_b.why(), "changed under write");
        assert_eq!(
            ref_b.intercepted_save(),
            None,
            "ARM_B: a refusal that displaced nothing must not claim it did"
        );
        assert_eq!(std::fs::read(dir_b.path().join("f.txt")).unwrap(), ORIGINAL);
        let strays_b: Vec<_> = std::fs::read_dir(dir_b.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n != "f.txt")
            .collect();
        assert!(strays_b.is_empty(), "ARM_B left behind {strays_b:?}");

        // ARM_A: the save landed in the window and the verdict refused.
        let (dir_a, out_a) = arm(true, true);
        let ref_a = out_a
            .expect("ARM_A: the round trip failed")
            .expect_err("ARM_A: the publish was not refused");

        // The destination is still exactly as it was — the contract that
        // `Ok(Err(_))` has always carried, and which is not what broke.
        assert_eq!(std::fs::read(dir_a.path().join("f.txt")).unwrap(), ORIGINAL);

        // c1: their bytes are on disk somewhere. Found by scanning, because
        // the displaced save lives under whatever name `keep_displaced`
        // settled on.
        let survivors = surviving(dir_a.path(), THEIRS);
        assert_eq!(
            survivors.len(),
            1,
            "ARM_A: the save made inside the verdict window was destroyed; the \
             directory holds {:?}",
            std::fs::read_dir(dir_a.path())
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect::<Vec<_>>()
        );

        // c1: and the caller is told where.
        assert_eq!(
            ref_a.intercepted_save(),
            Some(survivors[0].as_path()),
            "ARM_A: the refusal does not name where the displaced save was kept"
        );

        // c2: THE property. ARM_A and ARM_B were byte-identical — same return
        // value, same destination bytes, same directory listing — so nothing
        // the caller could read told a lossy refusal from a harmless one.
        assert_ne!(
            ref_a, ref_b,
            "a refusal that destroyed someone's save is indistinguishable from \
             one that destroyed nothing"
        );
        assert_ne!(ref_a.to_string(), ref_b.to_string());
        assert!(
            ref_a
                .to_string()
                .contains(&survivors[0].display().to_string()),
            "the rendered refusal does not carry the preserved path: {ref_a}"
        );
    }

    /// #1241 c4. The OTHER meaning of `Err` out of [`atomic_write_checked`] —
    /// a tempfile round trip that never reached a verdict — must not be
    /// mistaken for a refusal that could not be rolled back.
    ///
    /// The classifier is what decides whether `WriteTool`'s direct path
    /// publishes unchecked, and it fails in the dangerous direction if it
    /// answers `Some` for everything: a genuine round-trip failure would then
    /// be reported as a refusal and the write would never land. The error here
    /// is produced by the real code — a destination whose parent does not
    /// exist, so the sibling temp file cannot be staged.
    #[test]
    fn an_error_that_never_reached_a_verdict_is_not_a_rollback_failure() {
        let dir = tempfile::tempdir().unwrap();
        let nowhere = dir.path().join("no-such-dir").join("f.txt");

        let e = atomic_write_checked(&nowhere, b"ours", |_| Ok(()))
            .expect_err("staging a temp file in a directory that does not exist should fail");

        assert!(
            rollback_failure(&e).is_none(),
            "a round trip that never reached a verdict was classified as a \
             refusal whose rollback failed: {e}"
        );
    }

    /// #1202. A rollback that exchanged NOTHING is a restore FAILURE, not a
    /// clean refusal.
    ///
    /// [`restore`] is the inverse exchange, and it can come back having
    /// swapped nothing: `Swap::Vacant` when the destination NAME has
    /// disappeared since the publish (an external `rm`, a `git checkout`, an
    /// editor that unlinks before writing — precisely the non-cooperating
    /// writer #1155 exists to survive), `Swap::Unsupported` when the primitive
    /// is no longer available. `restore` used to discard that discriminant
    /// with `.map(|_| ())`, so both answered `Ok(())`: the publish still
    /// stood, [`discard_displaced`] then unlinked the pre-image — the only
    /// copy of the user's bytes — and the caller was handed `Ok(Err(why))`,
    /// whose documented contract is "the destination is exactly as it was".
    /// edit.rs renders `changed_under_write` off that, so the user was told
    /// the write was refused while the new bytes sat published and the
    /// original was gone.
    ///
    /// The window is driven directly rather than sampled: the check closure
    /// deletes the destination and THEN refuses, which is the one ordering
    /// that places the unlink between the two exchanges.
    ///
    /// Only `Vacant` is reachable from a live test — a filesystem that served
    /// `RENAME_EXCHANGE` one syscall ago still serves it — but `Vacant` and
    /// `Unsupported` leave `restore` through the same match, so the arm this
    /// reddens is the arm both take.
    #[test]
    fn a_restore_that_exchanged_nothing_is_a_failure_not_a_rollback() {
        const ORIGINAL: &[u8] = b"the only copy of the user's bytes";

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, ORIGINAL).unwrap();

        let mut handed: Option<Vec<u8>> = None;
        let outcome = atomic_write_checked(&p, b"ours", |observed| {
            handed = observed.map(<[u8]>::to_vec);
            // Between the publish and the rollback the destination NAME goes
            // away, so the second exchange has nothing to swap with.
            std::fs::remove_file(&p).unwrap();
            Err("changed under the write".to_owned())
        });

        // Fixture control. If the check was not handed the displaced
        // pre-image then the publish never happened and nothing below is
        // measuring a rollback at all.
        assert_eq!(
            handed.as_deref(),
            Some(ORIGINAL),
            "fixture: the check was not handed the displaced pre-image, so this \
             never reached the rollback"
        );

        // THE property: the user's bytes are still on disk. Found by scanning
        // rather than by name, because after a failed restore they live under
        // whatever name `keep_displaced` settled on.
        let survivors: Vec<PathBuf> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|f| std::fs::read(f).is_ok_and(|b| b == ORIGINAL))
            .collect();
        assert_eq!(
            survivors.len(),
            1,
            "the original bytes did not survive a rollback that exchanged \
             nothing; the directory holds {:?}",
            std::fs::read_dir(dir.path())
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect::<Vec<_>>()
        );

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            // `Ok(Err(why))` promises the destination is exactly as it was,
            // and it is not — the publish stands and the original is under
            // another name. The caller must be told this FAILED.
            let Err(err) = outcome else {
                panic!(
                    "a rollback that exchanged nothing was reported as a clean \
                     refusal ({outcome:?}); edit.rs renders `changed_under_write` \
                     off that and tells the user nothing was changed"
                );
            };
            let msg = err.to_string();
            let Some((_, named)) = msg.split_once("preserved at ") else {
                panic!("the failure does not name where the original was kept: {msg}");
            };
            assert_eq!(
                Path::new(named),
                survivors[0].as_path(),
                "the failure names {named}, but the surviving copy is {}",
                survivors[0].display()
            );
            assert_eq!(std::fs::read(named).unwrap(), ORIGINAL);
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            // Windows publishes with `ReplaceFileW` and restores with a plain
            // replacing rename, which succeeds against an absent destination:
            // the pre-image simply comes back. The refusal is honest there, so
            // the outcome is the ordinary `Ok(Err(why))` and the survivor is
            // the destination itself.
            let refusal = outcome.unwrap().expect_err("the publish was not refused");
            assert_eq!(refusal.why(), "changed under the write");
            assert_eq!(refusal.intercepted_save(), None);
            assert_eq!(survivors[0], p);
        }
    }

    /// An absent destination has nothing to exchange with, so the check is told
    /// `None` rather than being skipped.
    #[test]
    fn an_absent_destination_is_reported_as_no_pre_image() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("new.txt");

        let mut seen = Some(vec![0u8]);
        let r = atomic_write_checked(&p, b"body", |observed| {
            seen = observed.map(<[u8]>::to_vec);
            Ok(())
        })
        .unwrap();

        assert!(r.is_ok());
        assert_eq!(seen, None);
        assert_eq!(std::fs::read(&p).unwrap(), b"body");
    }

    #[test]
    fn atomic_write_replaces_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("target.txt");
        std::fs::write(&path, b"old contents").unwrap();

        atomic_write(&path, b"new contents").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new contents");
    }

    #[test]
    fn atomic_write_creates_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");
        atomic_write(&path, b"hello").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    }

    /// B6 — the rename-over-tempfile publish must not redefine the
    /// destination's permissions.
    ///
    /// `NamedTempFile` creates its file 0600 by design, and `persist()`
    /// carries that mode onto the destination name, so every rewrite of an
    /// existing file silently replaced its mode with 0600. Measured through
    /// the product: a 0755 script edited by the agent came back 0600 and the
    /// agent's own next turn got `Exit code: 126 … Permission denied`.
    ///
    /// Two files are seeded identically and only one is written. The
    /// untouched sibling is the CONTROL: if its mode moves too, the
    /// environment (umask, filesystem, test runner) is responsible and the
    /// subject measurement says nothing.
    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_the_destination_files_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let subject = dir.path().join("edited.sh");
        let control = dir.path().join("untouched.sh");
        for p in [&subject, &control] {
            std::fs::write(p, b"#!/bin/sh\necho HELLO\n").unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mode =
            |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;
        // The fixture itself must be real before anything is measured off it.
        assert_eq!(mode(&subject), 0o755, "fixture did not take 0755");

        atomic_write(&subject, b"#!/bin/sh\necho GOODBYE\n").unwrap();

        assert_eq!(
            mode(&control),
            0o755,
            "CONTROL: the untouched sibling lost its mode, so the environment \
             is stripping modes and the subject leg is uninterpretable"
        );
        assert_eq!(
            mode(&subject),
            0o755,
            "atomic_write redefined the destination's mode: 0o755 -> 0o{:o} \
             (an edited shell script stops being executable)",
            mode(&subject)
        );
        assert_eq!(
            std::fs::read(&subject).unwrap(),
            b"#!/bin/sh\necho GOODBYE\n"
        );
    }

    /// The other half of B6: a file the caller creates through this helper
    /// keeps the private-by-default 0600 it has always had. Credentials,
    /// session mirrors and the memory store are all written this way and
    /// several of them rely on it, so preservation must apply to an EXISTING
    /// destination only — never widen a new file.
    #[cfg(unix)]
    #[test]
    fn atomic_write_still_creates_new_files_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh.json");
        atomic_write(&path, b"{}").unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            0o600,
            "a newly created file must stay owner-only"
        );
    }

    #[test]
    fn atomic_write_failure_leaves_original_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("readonly_dir").join("file.txt");
        // Parent doesn't exist — write must fail without affecting any
        // other file. We can't easily simulate a mid-write crash in a
        // unit test, but a missing parent directory exercises the
        // pre-rename error path.
        let result = atomic_write(&path, b"contents");
        assert!(result.is_err());
    }
}
