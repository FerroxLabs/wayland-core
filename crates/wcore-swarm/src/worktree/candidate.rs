//! Opaque, live landing-candidate seal for an isolated delegated-mutation
//! checkout.
//!
//! A [`CandidateSeal`] binds — captured from the *live* retained checkout at
//! mint time — the transaction identity, the isolated Git object storage
//! evidence, the pinned commit/tree, and a bounded digest over the working-tree
//! source manifest. The seal is deliberately:
//!
//! * **non-reconstructible** — the only constructor ([`CandidateSeal::mint`])
//!   consumes references to the already-retained authorities; it accepts no
//!   caller-provided hash, path, or serialized field, so it cannot be forged
//!   from metadata;
//! * **live** — it retains the checkout [`DirectoryAuthority`] and an
//!   `Arc<TransactionCleanup>` liveness handle, so it can neither outlive nor
//!   be revalidated against a released transaction;
//! * **fail-closed** — [`CandidateSeal::revalidate`] re-derives every bound
//!   invariant from the current filesystem through the retained authorities
//!   (never by executing `git` and never by shelling out), and rejects any
//!   drift, aliasing, foreign metadata, or configuration poisoning.
//!
//! All Git plumbing is inspected as *files* through the retained
//! [`wcore_sandbox::DirectoryAuthority`], whose relative opens use `O_NOFOLLOW`
//! at every component — so a symlinked `.git`, `.git/objects`, or any linked
//! plumbing path fails closed automatically. Because a delegated worker
//! *controls* the checkout, `revalidate` treats `.git` as attacker-influenced:
//! it rejects a `.git/commondir` redirect, a worktree-scoped config, and any
//! `.git/config` directive outside a deny-by-default benign set (so relocation
//! vectors such as `core.hooksPath` / `core.fsmonitor` cannot smuggle an
//! executable past the hook scan). The source-manifest digest is SHA-256.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use sha2::{Digest, Sha256};
use wcore_sandbox::{DirectoryAuthority as SandboxDirectoryAuthority, SandboxError};

use super::TransactionCleanup;
use super::security::{DirectoryAuthority, DirectoryAuthorityIdentity};
use crate::error::{Result, SwarmError};

/// Maximum bytes read from any single Git plumbing file (`HEAD`, a loose ref,
/// `packed-refs`, `config`, `objects/info/alternates`). Oversized plumbing
/// fails closed.
const MAX_PLUMBING_BYTES: u64 = 1024 * 1024;

/// Total working-tree content byte budget hashed into the source manifest.
/// A checkout whose logical content exceeds this cannot be sealed — the walk
/// fails closed rather than silently hashing a truncated view.
const MANIFEST_BYTE_BUDGET: u64 = 256 * 1024 * 1024;

/// Maximum working-tree entries walked while computing the source manifest.
const MANIFEST_MAX_ENTRIES: u64 = 1_000_000;

/// An opaque, live seal proving that an isolated checkout is a clean landing
/// candidate. Every field is private and captured from the live checkout; the
/// type has no public constructor, is not `Clone`/`Copy`, and is never
/// serialized.
pub struct CandidateSeal {
    /// Transaction owner (worker id) that minted this seal.
    owner: String,
    /// Canonical retained checkout identity, captured at mint.
    checkout_identity: DirectoryAuthorityIdentity,
    /// The live retained checkout authority. Holding it both proves the
    /// checkout object still exists and is the sole channel through which the
    /// seal re-inspects the filesystem — never an ambient pathname.
    checkout: DirectoryAuthority,
    /// Liveness handle for the owning transaction. The seal cannot be
    /// revalidated once the transaction is released, and — because it keeps the
    /// cleanup record alive — it never dangles past the checkout it binds.
    cleanup: Arc<TransactionCleanup>,
    /// Pinned parent commit the isolated checkout was cloned from.
    base_commit: String,
    /// Commit the checkout `HEAD` resolved to at mint.
    head_commit: String,
    /// Tree the pinned commit points at.
    tree: String,
    /// Bounded SHA-256 digest (lowercase hex) over the working-tree source
    /// manifest (entry paths plus file sizes and contents, excluding `.git`).
    manifest_digest: String,
}

impl std::fmt::Debug for CandidateSeal {
    // Redacted: naming the bound hashes/paths would help reconstruct the seal.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CandidateSeal")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl CandidateSeal {
    /// Mint a seal from the *live* retained checkout. The only constructor.
    ///
    /// Takes references to the already-retained authorities and the resolved
    /// commit/tree strings that the owning [`super::TransactionWorkspace`]
    /// captured during isolated-checkout construction. No caller-provided
    /// digest or path is accepted, so a seal can only ever be produced against
    /// a live checkout this process currently holds authority over.
    ///
    /// Performs before-and-after validation at the seal boundary: the caller
    /// proves execution authority first, then this mint computes the manifest
    /// and immediately [`Self::revalidate`]s, so a seal is never returned over
    /// state that already drifted while it was being captured.
    pub(super) fn mint(
        owner: &str,
        checkout: &DirectoryAuthority,
        cleanup: &Arc<TransactionCleanup>,
        base_commit: &str,
        head_commit: &str,
        tree: &str,
    ) -> Result<Self> {
        let manifest_digest = manifest_digest(&checkout.to_sandbox())?;
        let seal = Self {
            owner: owner.to_owned(),
            checkout_identity: checkout.identity_token(),
            checkout: checkout.clone(),
            cleanup: Arc::clone(cleanup),
            base_commit: base_commit.to_owned(),
            head_commit: head_commit.to_owned(),
            tree: tree.to_owned(),
            manifest_digest,
        };
        seal.revalidate()?;
        Ok(seal)
    }

    /// Re-prove every bound invariant against the current filesystem through
    /// the retained authorities. Fails closed on release, an outstanding
    /// checkout descriptor loan, identity drift, source drift, a changed
    /// `HEAD`, a `.git/commondir` redirect, a worktree-scoped config, alternate
    /// or replace object aliasing, linked worktree metadata, any `.git/config`
    /// directive outside the deny-by-default benign set (a configured remote,
    /// `extensions.worktreeConfig`, `core.hooksPath` / `core.fsmonitor` /
    /// `core.sshcommand` / any other relocation or command directive, filters,
    /// includes, aliases, credential/url/protocol sections …), a planted hook,
    /// or any inspection failure.
    pub(super) fn revalidate(&self) -> Result<()> {
        // Fail closed if the owning transaction was released: the seal grants
        // no authority once its checkout has been (or is being) torn down.
        if self.cleanup.released.load(Ordering::Acquire) {
            return Err(SwarmError::DispatchAdmission(
                "landing candidate seal refused: owning transaction was released".to_owned(),
            ));
        }

        // Integrity of the bound commit identifiers. Reads base/head/tree so a
        // malformed binding can never masquerade as a clean candidate.
        for (label, value) in [
            ("base commit", &self.base_commit),
            ("head commit", &self.head_commit),
            ("tree", &self.tree),
        ] {
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(SwarmError::DispatchAdmission(format!(
                    "landing candidate seal refused: malformed bound {label}"
                )));
            }
        }

        // A descendant still holding the retained checkout descriptor means the
        // checkout is no longer the seal's exclusive live object.
        if self.checkout.has_outstanding_loans() {
            return Err(SwarmError::DispatchAdmission(
                "landing candidate seal refused: checkout descriptor loan outstanding".to_owned(),
            ));
        }

        let sandbox = self.checkout.to_sandbox();
        let display = sandbox.display_path().to_path_buf();

        // Identity: the retained checkout object still occupies its pathname.
        // A same-path repository substitution fails here with "identity changed".
        self.checkout.validate_path(&display)?;
        if self.checkout.identity_token() != self.checkout_identity {
            return Err(SwarmError::DispatchAdmission(
                "landing candidate seal refused: checkout identity changed".to_owned(),
            ));
        }

        // Source manifest: no working-tree drift since mint.
        let current = manifest_digest(&sandbox)?;
        if current != self.manifest_digest {
            return Err(SwarmError::DispatchAdmission(
                "landing candidate seal refused: source manifest drifted".to_owned(),
            ));
        }

        // Git plumbing (files only). Opening `.git` as a directory rejects a
        // gitlink file or symlinked `.git` — linked-worktree metadata included.
        let git = open_child_dir(&sandbox, ".git")?;

        // A fresh isolated main checkout never redirects its common dir. A
        // planted `.git/commondir` makes git resolve objects/refs/packed-refs/
        // config from an attacker directory at land time, which would bypass
        // the pristine-`.git` inspection that follows. Reject its presence.
        if read_child_opt(&git, "commondir")?.is_some() {
            return Err(SwarmError::DispatchAdmission(
                "landing candidate seal refused: checkout carries a .git/commondir redirect"
                    .to_owned(),
            ));
        }

        // Worktree-scoped config is only loaded under extensions.worktreeConfig,
        // but a fresh checkout has none. Reject the file's mere presence here;
        // the enabling extension in `.git/config` is rejected by `scan_config`.
        if read_child_opt(&git, "config.worktree")?.is_some() {
            return Err(SwarmError::DispatchAdmission(
                "landing candidate seal refused: checkout carries a worktree-scoped config"
                    .to_owned(),
            ));
        }

        // HEAD must still resolve to the bound commit.
        let resolved = resolve_head(&git)?;
        if resolved != self.head_commit {
            return Err(SwarmError::DispatchAdmission(
                "landing candidate seal refused: HEAD no longer resolves to the bound commit"
                    .to_owned(),
            ));
        }

        // Object storage: no alternate store, and `objects` opened with
        // O_NOFOLLOW rejects a symlinked object directory.
        let objects = open_child_dir(&git, "objects")?;
        if let Some(info) = open_child_dir_opt(&objects, "info")?
            && read_child_opt(&info, "alternates")?.is_some()
        {
            return Err(SwarmError::DispatchAdmission(
                "landing candidate seal refused: checkout uses an alternate object store"
                    .to_owned(),
            ));
        }

        // Replace refs (loose and packed) rewrite object identity — reject both.
        if let Some(refs) = open_child_dir_opt(&git, "refs")?
            && let Some(replace) = open_child_dir_opt(&refs, "replace")?
            && !child_names(&replace)?.is_empty()
        {
            return Err(replace_refs_error());
        }
        if let Some(packed) = read_child_opt(&git, "packed-refs")? {
            let text = String::from_utf8_lossy(&packed);
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('#') || line.starts_with('^') {
                    continue;
                }
                if let Some((_, name)) = line.split_once(' ')
                    && name.trim().starts_with("refs/replace/")
                {
                    return Err(replace_refs_error());
                }
            }
        }

        // Linked worktree metadata: a populated `.git/worktrees` means shared
        // state with another working tree.
        if let Some(worktrees) = open_child_dir_opt(&git, "worktrees")?
            && !child_names(&worktrees)?.is_empty()
        {
            return Err(SwarmError::DispatchAdmission(
                "landing candidate seal refused: linked worktree metadata present".to_owned(),
            ));
        }

        // Configuration: deny-by-default. Only a benign core/branch/extensions
        // subset that a fresh clone legitimately produces is accepted; every
        // other section or directive — remotes, filters, includes, aliases,
        // relocation/command keys — is rejected.
        if let Some(config) = read_child_opt(&git, "config")? {
            let text = String::from_utf8_lossy(&config);
            scan_config(&text)?;
        }

        // Hooks: any non-sample entry under `.git/hooks` is a planted hook.
        if let Some(hooks) = open_child_dir_opt(&git, "hooks")? {
            for name in child_names(&hooks)? {
                if name.ends_with(".sample") {
                    continue;
                }
                match hooks.open_child_file(&name) {
                    Ok(_) => {
                        return Err(SwarmError::DispatchAdmission(
                            "landing candidate seal refused: planted Git hook present".to_owned(),
                        ));
                    }
                    // Vanished between listing and open — nothing to run.
                    Err(SandboxError::Io(error))
                        if error.kind() == std::io::ErrorKind::NotFound => {}
                    // A linked hook or a non-file entry fails closed.
                    Err(_) => {
                        return Err(SwarmError::DispatchAdmission(
                            "landing candidate seal refused: unreadable Git hook entry".to_owned(),
                        ));
                    }
                }
            }
        }

        // After-check: the checkout object was not swapped mid-revalidation.
        self.checkout.validate_path(&display)?;
        Ok(())
    }
}

fn replace_refs_error() -> SwarmError {
    SwarmError::DispatchAdmission(
        "landing candidate seal refused: checkout carries replace refs".to_owned(),
    )
}

fn map_sandbox(error: SandboxError) -> SwarmError {
    SwarmError::DispatchAdmission(error.to_string())
}

/// Open a required direct child directory, mapping the sandbox error into the
/// crate error surface. Fails closed for a missing, linked, or non-directory
/// child.
fn open_child_dir(
    parent: &SandboxDirectoryAuthority,
    name: &str,
) -> Result<SandboxDirectoryAuthority> {
    parent.open_child_directory(name).map_err(map_sandbox)
}

/// Open an optional direct child directory. `None` only for a genuinely absent
/// entry; a linked or non-directory child fails closed.
fn open_child_dir_opt(
    parent: &SandboxDirectoryAuthority,
    name: &str,
) -> Result<Option<SandboxDirectoryAuthority>> {
    match parent.open_child_directory(name) {
        Ok(child) => Ok(Some(child)),
        Err(SandboxError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(map_sandbox(error)),
    }
}

fn read_child_opt(parent: &SandboxDirectoryAuthority, name: &str) -> Result<Option<Vec<u8>>> {
    parent
        .read_child_bounded(name, MAX_PLUMBING_BYTES)
        .map_err(map_sandbox)
}

fn child_names(directory: &SandboxDirectoryAuthority) -> Result<Vec<String>> {
    directory.child_names().map_err(map_sandbox)
}

/// Resolve `HEAD` to a commit id by reading plumbing files only. A symbolic
/// `HEAD` is followed to its loose ref, or `packed-refs`; a detached `HEAD`
/// returns its literal id.
fn resolve_head(git: &SandboxDirectoryAuthority) -> Result<String> {
    let head = read_child_opt(git, "HEAD")?.ok_or_else(|| {
        SwarmError::DispatchAdmission("isolated checkout is missing HEAD".to_owned())
    })?;
    let head = String::from_utf8_lossy(&head).trim().to_owned();
    match head.strip_prefix("ref:") {
        Some(reference) => resolve_ref(git, reference.trim())?.ok_or_else(|| {
            SwarmError::DispatchAdmission(
                "isolated checkout HEAD points at an unresolved ref".to_owned(),
            )
        }),
        None => Ok(head),
    }
}

fn resolve_ref(git: &SandboxDirectoryAuthority, ref_path: &str) -> Result<Option<String>> {
    if let Some(bytes) = read_ref_file(git, ref_path)? {
        return Ok(Some(String::from_utf8_lossy(&bytes).trim().to_owned()));
    }
    if let Some(packed) = read_child_opt(git, "packed-refs")? {
        let text = String::from_utf8_lossy(&packed);
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.starts_with('^') {
                continue;
            }
            if let Some((object, name)) = line.split_once(' ')
                && name.trim() == ref_path
            {
                return Ok(Some(object.trim().to_owned()));
            }
        }
    }
    Ok(None)
}

/// Read a loose ref file by navigating its path components through relative,
/// no-follow directory opens. Returns `None` if any component is absent.
fn read_ref_file(git: &SandboxDirectoryAuthority, ref_path: &str) -> Result<Option<Vec<u8>>> {
    let mut components: Vec<&str> = ref_path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let Some(last) = components.pop() else {
        return Ok(None);
    };
    let mut directory = git.clone();
    for component in components {
        match open_child_dir_opt(&directory, component)? {
            Some(child) => directory = child,
            None => return Ok(None),
        }
    }
    read_child_opt(&directory, last)
}

/// Bounded, deterministic SHA-256 digest (lowercase hex) over the working-tree
/// source manifest. Walks the checkout through the retained authority (never an
/// ambient path), excludes the top-level `.git`, and feeds a domain-separated,
/// length-framed byte stream — `0x00` + prefix + framed child names for each
/// directory; `0x01` + framed path + exec bit + length + framed content for
/// each file — so a type swap, rename, empty-subdirectory add/remove, mode
/// change (`chmod +x`), or content change all perturb the digest. This binds
/// the full git-tree identity (name + mode + content). An oversized tree or too
/// many entries fails closed.
///
/// Tracked working-tree **symlinks** are not yet bindable: there is no
/// no-follow readlink primitive on the retained authority, so a symlinked entry
/// (rejected by `O_NOFOLLOW` with `ELOOP`) fails closed with a *specific* error
/// rather than a generic sandbox I/O error. Binding symlink targets into the
/// manifest is deferred to the consuming landing plan.
pub(super) fn manifest_digest(checkout: &SandboxDirectoryAuthority) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut budget = MANIFEST_BYTE_BUDGET;
    let mut entries: u64 = 0;
    // (directory authority, path prefix, is checkout root)
    let mut pending = vec![(checkout.clone(), String::new(), true)];

    while let Some((directory, prefix, is_root)) = pending.pop() {
        let mut names = child_names(&directory)?;
        // Exclude the top-level `.git` from the manifest entirely — both its
        // shape contribution and its contents — so planting or perturbing Git
        // metadata never changes the source digest.
        if is_root {
            names.retain(|name| name.as_str() != ".git");
        }
        // Bind the directory's own shape into the digest so an added or removed
        // empty entry still perturbs the manifest.
        hasher.update([0_u8]);
        update_framed(&mut hasher, prefix.as_bytes());
        hasher.update((names.len() as u64).to_le_bytes());
        for name in &names {
            update_framed(&mut hasher, name.as_bytes());
        }

        for name in names {
            entries = entries.checked_add(1).ok_or_else(|| {
                SwarmError::DispatchAdmission("source manifest entry count overflowed".to_owned())
            })?;
            if entries > MANIFEST_MAX_ENTRIES {
                return Err(SwarmError::DispatchAdmission(
                    "source manifest exceeded the entry budget".to_owned(),
                ));
            }
            let child_path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            match directory.open_child_directory(&name) {
                Ok(child) => pending.push((child, child_path, false)),
                Err(SandboxError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotADirectory =>
                {
                    // A regular (non-directory) file. `open_child_file` uses
                    // `O_NOFOLLOW`; if the entry was swapped for a symlink
                    // between the two opens it fails with `ELOOP`, which
                    // `classify_walk_error` reports specifically.
                    let file = directory
                        .open_child_file(&name)
                        .map_err(|error| classify_walk_error(&child_path, error))?;
                    let length = file.len().map_err(map_sandbox)?;
                    let executable = file.is_executable().map_err(map_sandbox)?;
                    let content = file.read_bounded(budget).map_err(map_sandbox)?;
                    budget = budget.checked_sub(content.len() as u64).ok_or_else(|| {
                        SwarmError::DispatchAdmission(
                            "source manifest exceeded byte budget".to_owned(),
                        )
                    })?;
                    // Bind the full git-tree identity: name + mode + content. The
                    // exec bit distinguishes `100644` from `100755`, so a bare
                    // `chmod +x` after mint is drift, not a silent no-op.
                    hasher.update([1_u8]);
                    update_framed(&mut hasher, child_path.as_bytes());
                    hasher.update([executable as u8]);
                    hasher.update(length.to_le_bytes());
                    update_framed(&mut hasher, &content);
                }
                // A symlink (its `O_NOFOLLOW` open yields `ELOOP`), or any other
                // refusal, is not a clean regular file: fail closed with a
                // specific diagnostic.
                Err(error) => return Err(classify_walk_error(&child_path, error)),
            }
        }
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest.iter() {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    Ok(hex)
}

/// Length-frame a byte slice into the SHA-256 stream so concatenation of
/// variable-length fields stays injective (no ambiguous boundaries).
fn update_framed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Turn a manifest-walk refusal into a clear error. A symlinked entry surfaces
/// as `ELOOP` (`O_NOFOLLOW`); report it specifically so callers can distinguish
/// "tracked symlink, not yet supported" from a genuine inspection failure.
fn classify_walk_error(path: &str, error: SandboxError) -> SwarmError {
    // `ErrorKind::FilesystemLoop` is still unstable (`io_error_more`), so match on
    // the raw OS error instead: an `O_NOFOLLOW` open of a symlinked final
    // component fails with `ELOOP`.
    if is_symlink_loop(&error) {
        return SwarmError::DispatchAdmission(format!(
            "candidate seal does not support tracked symlinks in the working tree: {path}"
        ));
    }
    map_sandbox(error)
}

#[cfg(unix)]
fn is_symlink_loop(error: &SandboxError) -> bool {
    let SandboxError::Io(io) = error else {
        return false;
    };
    // Prefer the exact errno; fall back to the message in case a wrapper carried
    // `ELOOP` only in its text (`ErrorKind::FilesystemLoop` is still unstable).
    io.raw_os_error() == Some(libc::ELOOP)
        || io.to_string().contains("Too many levels of symbolic links")
}

#[cfg(not(unix))]
fn is_symlink_loop(_error: &SandboxError) -> bool {
    false
}

/// Deny-by-default scan of a Git `config`. Only the benign `core`, `branch`,
/// and `extensions` keys a fresh `git clone --no-local --no-tags
/// --single-branch --no-checkout` + `remote remove origin` + `checkout -b`
/// legitimately produces are accepted; every other section, and every
/// relocation/command directive within the accepted sections, is rejected.
///
/// A legitimate delegated worker never edits `.git/config`, so a strict scan
/// closes the whole class of config-driven command-execution and object/ref
/// redirect vectors (`core.hooksPath`, `core.fsmonitor`, `core.sshcommand`,
/// `filter.*`, `include.*`/`includeIf.*`, `remote.*`, `url.*`, `credential.*`,
/// `extensions.worktreeConfig`, …) rather than enumerating them individually.
pub(super) fn scan_config(config: &str) -> Result<()> {
    // Lowercased current section name (`[section "subsection"]` → `section`).
    let mut section = String::new();
    for raw in config.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') {
            let inner = line.trim_start_matches('[').trim_end_matches(']').trim();
            section = inner
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            if section == "remote" {
                return Err(config_remote_error());
            }
            if !matches!(section.as_str(), "core" | "branch" | "extensions") {
                return Err(config_poison_error(&format!(
                    "unexpected [{section}] section"
                )));
            }
            continue;
        }
        // A key line, possibly value-less (git reads a bare key as `true`).
        let (key, value) = match line.split_once('=') {
            Some((key, value)) => (
                key.trim().to_ascii_lowercase(),
                value.trim().to_ascii_lowercase(),
            ),
            None => (line.to_ascii_lowercase(), String::new()),
        };
        match section.as_str() {
            "core" => {
                // Deny-by-default allowlist: accept ONLY the value-only,
                // non-executable core keys a fresh clone/checkout legitimately
                // writes. Any unknown key — `gitproxy`, `hookspath`,
                // `fsmonitor`, or a future exec/redirect directive — is
                // rejected outright.
                const BENIGN_CORE_KEYS: &[&str] = &[
                    "repositoryformatversion",
                    "filemode",
                    "bare",
                    "logallrefupdates",
                    "symlinks",
                    "ignorecase",
                    "precomposeunicode",
                    "protecthfs",
                    "protectntfs",
                    "quotepath",
                    "autocrlf",
                    "eol",
                    "hidedotfiles",
                ];
                if !BENIGN_CORE_KEYS.contains(&key.as_str()) {
                    return Err(config_poison_error(&format!("core.{key}")));
                }
            }
            "branch" => {
                if key.ends_with("command") || key.ends_with("helper") {
                    return Err(config_poison_error(&format!("branch key {key}")));
                }
            }
            "extensions" => {
                // Worktree-scoped config would activate `config.worktree`.
                // Reject unless explicitly disabled.
                if key == "worktreeconfig"
                    && !matches!(value.as_str(), "false" | "no" | "off" | "0")
                {
                    return Err(config_poison_error("extensions.worktreeConfig"));
                }
            }
            _ => return Err(config_poison_error("unexpected configuration key")),
        }
    }
    Ok(())
}

fn config_remote_error() -> SwarmError {
    SwarmError::DispatchAdmission(
        "landing candidate seal refused: checkout retains a configured remote".to_owned(),
    )
}

fn config_poison_error(detail: &str) -> SwarmError {
    SwarmError::DispatchAdmission(format!(
        "landing candidate seal refused: disallowed Git configuration present ({detail})"
    ))
}
