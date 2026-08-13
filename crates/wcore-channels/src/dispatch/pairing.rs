//! DM pairing — single-use, expiring codes that admit exactly ONE sender.
//!
//! `DmPolicy::Pairing` exists so an operator can admit one person to DMs
//! without opening the channel to the world. Before this module the arm
//! was a stub that denied everything, which left `dm_allowlist = ["*"]`
//! (admit ANYONE who can find the bot) as the only working escape hatch.
//!
//! ## Model
//!
//! - The **operator** mints a code out-of-band ([`PairingBook::mint`] —
//!   reachable from the host, never from an inbound message). Only the
//!   SHA-256 digest of the code is persisted; the plaintext exists once,
//!   in the return value handed to the operator.
//! - The **sender** presents the code as their message body (bare, or
//!   `/pair <code>`). A match pairs their stable `sender_id` and burns
//!   the code. Every later message from that `sender_id` is admitted
//!   without re-pairing.
//! - Anything else denies. There is no other path to a grant: the body
//!   is never interpreted as an instruction, only matched against
//!   [`extract_code`]'s strict shape and then compared, in constant
//!   time, against the stored digests.
//!
//! ## Two processes, one file
//!
//! The operator half and the runtime half are ALWAYS different
//! processes: `wayland-core channel pair …` is one short-lived process
//! per invocation, the gateway's drain loop is another, long-lived one,
//! and they meet nowhere except `<channels_dir>/pairings/<channel>.toml`.
//!
//! So the file — not any in-memory copy of it — is the state. There is
//! deliberately **no cache** in [`PairingBook`] when it is backed by a
//! store: every access decision re-reads, and every mutation is a
//! read-modify-write performed inside an exclusive cross-process lock.
//! A cache here is not a performance detail, it is a correctness bug:
//! it makes a freshly minted code invisible to the running gateway,
//! makes revocation inert while the CLI prints success, and lets each
//! side publish its stale whole-file snapshot over the other's writes.
//!
//! The lock is an advisory `flock` / `LockFileEx` (via `fd-lock`) on a
//! DEDICATED sibling `<channel>.lock` file — the same mechanism
//! `wcore-budget`'s daily spend ledger and `wcore-config`'s credential
//! store already use. Publishing goes through
//! [`wcore_config::atomic_write`] (tempfile + fsync + rename).
//!
//! **Windows.** `fd-lock` maps to `LockFileEx` on a handle we open, and
//! the lock file is never renamed and never removed, so it can never be
//! the participant in an `ERROR_SHARING_VIOLATION` during publish. The
//! state file is opened only INSIDE the lock and closed before the lock
//! is released, so no reader of ours holds a handle while the writer
//! renames over it. A third party that opens the file without
//! `FILE_SHARE_DELETE` (indexer, AV) can still make the rename fail;
//! that surfaces as a refusal to grant, never as a grant. None of this
//! has been exercised on a Windows host — see the residuals.
//!
//! ## Fail-closed points
//!
//! - Unknown channel / missing state file -> empty state -> deny.
//! - Unreadable or malformed state file -> deny, and no write: the
//!   phantom empty state is never published over the operator's file.
//! - Empty `sender_id` (no stable identity to pair) -> deny.
//! - A grant that cannot be PERSISTED is not granted: the redeem and
//!   the durable write happen in ONE locked critical section, so no
//!   process and no restart can disagree about who is paired, and one
//!   code can never be burnt twice.
//! - [`PairingBook::ephemeral`] has no store and no state, so any code
//!   path that forgets to supply the real book denies rather than opens.
//!
//! ## Secrecy
//!
//! The plaintext code is never written to disk, never logged, never
//! echoed to the channel, and never embedded in a deny reason — every
//! pairing denial collapses to the single content-free tag
//! [`PAIRING_DENY_REASON`], so a sender learns nothing from the
//! difference between "wrong code", "expired code" and "not a code".

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::{Choice, ConditionallySelectable, ConstantTimeEq};

use crate::error::ChannelError;
use crate::event::IncomingMessage;

/// Crockford-style base32 alphabet: no `I`, `L`, `O`, `U`, so a code read
/// aloud or retyped does not collide. Exactly 32 symbols, so masking a
/// uniform random byte with `0x1f` is unbiased.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Characters in a pairing code. 26 symbols over a 32-symbol alphabet =
/// 130 bits of entropy — unguessable, and short enough to paste.
pub const CODE_LEN: usize = 26;

/// Default lifetime of a freshly minted code: 15 minutes.
pub const DEFAULT_CODE_TTL_MS: i64 = 15 * 60 * 1000;

/// Upper bound on live (unredeemed, unexpired) codes per channel. Bounds
/// the on-disk state and the redeem loop; minting past the cap evicts the
/// oldest outstanding code.
pub const MAX_PENDING_CODES: usize = 16;

/// Longest inbound body still considered as a possible code. A code
/// message is tiny; anything larger is prose and is rejected without
/// scanning.
const MAX_CODE_INPUT: usize = 256;

/// The ONLY deny reason any pairing denial produces. Deliberately
/// uniform: distinguishing "wrong code" from "no code" would hand a
/// remote sender an oracle, and any richer string risks quoting content.
pub const PAIRING_DENY_REASON: &str = "pairing required";

/// A sender that completed pairing on a channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedSender {
    /// Stable platform sender id (never a display name).
    pub sender_id: String,
    /// Wall-clock millis at which pairing completed.
    pub paired_at_ms: i64,
}

/// An outstanding, unredeemed code. Only the digest is stored.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingCode {
    /// Lowercase hex SHA-256 of the code. The plaintext is not stored.
    hash: String,
    created_at_ms: i64,
    expires_at_ms: i64,
}

impl std::fmt::Debug for PendingCode {
    /// Redacts the digest so no `{:?}` of pairing state can put
    /// code-derived material into a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingCode")
            .field("hash", &"<redacted>")
            .field("created_at_ms", &self.created_at_ms)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

/// Durable pairing state for ONE channel. Serialized as TOML, mirroring
/// how the crate already persists channel state (see
/// [`crate::config::ChannelConfigLoader`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairingState {
    #[serde(default)]
    paired: Vec<PairedSender>,
    #[serde(default)]
    pending: Vec<PendingCode>,
}

impl PairingState {
    /// Whether `sender_id` has already paired.
    pub fn is_paired(&self, sender_id: &str) -> bool {
        !sender_id.is_empty() && self.paired.iter().any(|p| p.sender_id == sender_id)
    }

    /// Paired senders, oldest first.
    pub fn paired(&self) -> &[PairedSender] {
        &self.paired
    }

    /// Count of codes that are still redeemable at `now_ms`.
    pub fn live_code_count(&self, now_ms: i64) -> usize {
        self.pending
            .iter()
            .filter(|p| p.expires_at_ms > now_ms)
            .count()
    }

    /// Drop expired codes. Expiry is exclusive: an entry whose
    /// `expires_at_ms` EQUALS `now_ms` is dead, matching [`Self::redeem`]
    /// and [`Self::live_code_count`], so a code cannot be redeemed at an
    /// instant a prune would already have removed it.
    ///
    /// Deliberately returns nothing. It used to report whether anything
    /// was removed and no caller ever read that, which made the flag
    /// impossible to test — the entries it keeps are the behaviour.
    fn prune(&mut self, now_ms: i64) {
        self.pending.retain(|p| p.expires_at_ms > now_ms);
    }

    /// Record the digest of a freshly minted code.
    fn record(&mut self, code: &str, now_ms: i64, ttl_ms: i64) {
        self.prune(now_ms);
        while self.pending.len() >= MAX_PENDING_CODES {
            self.pending.remove(0);
        }
        self.pending.push(PendingCode {
            hash: hex32(&digest(code)),
            created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(ttl_ms.max(0)),
        });
    }

    /// Consume `code` if it matches a live pending entry. Single-use: a
    /// match removes the entry, so the same code never redeems twice.
    ///
    /// The comparison is constant time. Two properties matter: each
    /// candidate is compared with [`ConstantTimeEq`] (no byte-prefix
    /// timing leak), and the loop never breaks early, so the work done
    /// does not depend on WHICH entry matched.
    fn redeem(&mut self, code: &str, now_ms: i64) -> bool {
        let presented = digest(code);
        let mut found = 0u8;
        let mut hit = 0u32;
        for (i, p) in self.pending.iter().enumerate() {
            // Expiry is not secret — it depends on the clock, not the code.
            if p.expires_at_ms <= now_ms {
                continue;
            }
            let eq = match unhex32(&p.hash) {
                Some(stored) => presented[..].ct_eq(&stored[..]),
                // Corrupt entry: never matches, and never panics.
                None => Choice::from(0u8),
            };
            let take = eq & Choice::from(1u8 ^ found);
            hit = u32::conditional_select(&hit, &(i as u32), take);
            found |= eq.unwrap_u8();
        }
        if found == 1 {
            self.pending.remove(hit as usize);
            true
        } else {
            false
        }
    }

    /// Mark `sender_id` paired (idempotent).
    fn pair(&mut self, sender_id: &str, now_ms: i64) {
        if self.is_paired(sender_id) {
            return;
        }
        self.paired.push(PairedSender {
            sender_id: sender_id.to_string(),
            paired_at_ms: now_ms,
        });
    }

    /// Remove a paired sender. Returns whether one was removed.
    fn unpair(&mut self, sender_id: &str) -> bool {
        let before = self.paired.len();
        self.paired.retain(|p| p.sender_id != sender_id);
        self.paired.len() != before
    }
}

fn digest(code: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(code.as_bytes());
    h.finalize().into()
}

fn hex32(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap_or('0'));
    }
    s
}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    let bytes = s.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in bytes.chunks_exact(2).enumerate() {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        out[i] = ((hi << 4) | lo) as u8;
    }
    Some(out)
}

/// Generate a fresh code from the OS CSPRNG. Masking a uniform byte with
/// `0x1f` selects one of the 32 alphabet symbols with no modulo bias.
fn generate_code() -> String {
    let mut raw = [0u8; CODE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    raw.iter()
        .map(|b| ALPHABET[(b & 0x1f) as usize] as char)
        .collect()
}

/// Extract a candidate pairing code from an inbound body, or `None`.
///
/// Accepted shapes, and NOTHING else:
/// - the bare code, e.g. `A1B2...`
/// - `/pair <code>` or `pair <code>` (case-insensitive keyword)
///
/// The candidate is then normalized (ASCII whitespace, `-` and `_`
/// stripped; uppercased) and must be exactly [`CODE_LEN`] symbols drawn
/// from the code alphabet. Prose — including text that *claims* pairing
/// was approved — cannot satisfy that shape, so it yields `None` and the
/// caller denies.
pub fn extract_code(text: &str) -> Option<String> {
    if text.len() > MAX_CODE_INPUT {
        return None;
    }
    let trimmed = text.trim();
    let body = match trimmed.split_once(char::is_whitespace) {
        Some((head, rest))
            if head.eq_ignore_ascii_case("/pair") || head.eq_ignore_ascii_case("pair") =>
        {
            rest
        }
        _ => trimmed,
    };

    let mut norm = String::with_capacity(CODE_LEN);
    for ch in body.chars() {
        if ch.is_ascii_whitespace() || ch == '-' || ch == '_' {
            continue;
        }
        if norm.len() == CODE_LEN {
            return None; // too long
        }
        let up = ch.to_ascii_uppercase();
        if !ALPHABET.contains(&(up as u8)) {
            return None; // not a code symbol
        }
        norm.push(up);
    }
    (norm.len() == CODE_LEN).then_some(norm)
}

/// Validate a channel name before it becomes a filename. Rejects path
/// traversal and separators so a hostile/typo'd channel name cannot
/// escape the pairing directory.
fn validate_channel(channel: &str) -> Result<(), ChannelError> {
    let ok = !channel.is_empty()
        && channel.len() <= 128
        && !channel.starts_with('.')
        && channel
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(ChannelError::Config(format!(
            "invalid channel name for pairing state: {channel:?}"
        )))
    }
}

/// On-disk pairing state, one TOML file per channel under `root`.
///
/// Mirrors [`crate::config::ChannelConfigLoader`]: a root directory of
/// `<name>.toml` files, serde-derived, read and written with `std::fs`.
/// Deliberately a SUBDIRECTORY of the channel config root — a sibling
/// `*.toml` would be picked up by the config loader's `read_dir` scan
/// and fail its name-matches-stem check.
#[derive(Debug, Clone)]
pub struct PairingStore {
    root: PathBuf,
}

impl PairingStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The pairing directory that belongs to a channel config directory:
    /// `<channels_dir>/pairings`.
    ///
    /// There is deliberately no `default_root()` that resolves `$HOME` on
    /// its own. Pairing state MUST live beside the configs the RUNTIME
    /// actually read — the F24-C3-H1 rule — and this crate cannot see the
    /// profile home the host resolved. A store minted into one directory
    /// and read from another is a pairing feature that silently never
    /// works, so every caller names its channels dir.
    pub fn beside_configs(channels_dir: impl AsRef<Path>) -> Self {
        Self::new(channels_dir.as_ref().join("pairings"))
    }

    fn path_for(&self, channel: &str) -> Result<PathBuf, ChannelError> {
        validate_channel(channel)?;
        Ok(self.root.join(format!("{channel}.toml")))
    }

    /// Open (creating if needed) the advisory lock that serializes access
    /// to `channel`'s state across processes.
    ///
    /// A DEDICATED sibling file, never the state file itself: the state
    /// file is republished by `rename`, and on Windows renaming over a
    /// file somebody holds open is `ERROR_SHARING_VIOLATION`. This one is
    /// only ever opened and locked, so it cannot be that somebody.
    ///
    /// The channel name is validated before anything touches the
    /// filesystem, so a hostile name cannot create a lock file outside
    /// the root either.
    fn open_lock(&self, channel: &str) -> Result<fd_lock::RwLock<std::fs::File>, ChannelError> {
        let lock_path = self.path_for(channel)?.with_extension("lock");
        std::fs::create_dir_all(&self.root)
            .map_err(|e| ChannelError::Config(format!("{}: {e}", self.root.display())))?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| ChannelError::Config(format!("{}: {e}", lock_path.display())))?;
        Ok(fd_lock::RwLock::new(file))
    }

    /// Read a channel's state under the SHARED cross-process lock, so a
    /// reader never observes a half-published file and never races the
    /// operator's write.
    ///
    /// A missing file is an EMPTY state (nobody paired, no live codes) —
    /// not an error, and not an open door. A present-but-unreadable or
    /// malformed file IS an error, so the caller denies rather than
    /// silently starting from empty.
    pub fn read(&self, channel: &str) -> Result<PairingState, ChannelError> {
        // A shared lock only needs `&self`; the exclusive one in `update`
        // needs `&mut`.
        let lock = self.open_lock(channel)?;
        let _guard = loop {
            match lock.read() {
                Ok(guard) => break guard,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(ChannelError::Config(format!("pairing lock: {e}"))),
            }
        };
        self.load_locked(channel)
    }

    /// Read-modify-write a channel's state inside the EXCLUSIVE
    /// cross-process lock.
    ///
    /// This is the ONLY mutation path. The state handed to `body` is read
    /// from disk inside the lock, so a decision can never be made against
    /// a snapshot another process has already superseded, and the write
    /// back cannot clobber a concurrent one. `body` returns its outcome
    /// plus whether the state actually changed; an unchanged state is not
    /// republished, so ordinary denied traffic does no I/O beyond the read.
    fn update<T>(
        &self,
        channel: &str,
        body: impl FnOnce(&mut PairingState) -> (T, bool),
    ) -> Result<T, ChannelError> {
        let mut lock = self.open_lock(channel)?;
        // `fd_lock`'s write guard borrows the `RwLock` mutably, so the retry
        // loop cannot return the guard across a function boundary under NLL —
        // the same closure shape `wcore-budget`'s daily ledger uses.
        let _guard = loop {
            match lock.write() {
                Ok(guard) => break guard,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(ChannelError::Config(format!("pairing lock: {e}"))),
            }
        };
        let mut state = self.load_locked(channel)?;
        let (outcome, changed) = body(&mut state);
        if changed {
            self.save_locked(channel, &state)?;
        }
        Ok(outcome)
    }

    /// Load without taking the lock. Callers must already hold it.
    fn load_locked(&self, channel: &str) -> Result<PairingState, ChannelError> {
        let path = self.path_for(channel)?;
        let body = match std::fs::read_to_string(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(PairingState::default());
            }
            Err(e) => {
                return Err(ChannelError::Config(format!("{}: {e}", path.display())));
            }
        };
        toml::from_str(&body).map_err(|e| ChannelError::Config(format!("{}: {e}", path.display())))
    }

    /// Publish without taking the lock. Callers must already hold it
    /// exclusively.
    ///
    /// [`wcore_config::atomic_write`] is the workspace's one durable
    /// publish helper (tempfile + fsync + rename, with the Windows
    /// long-path handling from F26-03-D). Its tempfile name is random, so
    /// two writers cannot collide on it the way a fixed `<name>.toml.tmp`
    /// sibling does.
    fn save_locked(&self, channel: &str, state: &PairingState) -> Result<(), ChannelError> {
        let path = self.path_for(channel)?;
        std::fs::create_dir_all(&self.root)
            .map_err(|e| ChannelError::Config(format!("{}: {e}", self.root.display())))?;
        let body = toml::to_string_pretty(state)
            .map_err(|e| ChannelError::Config(format!("serialize pairing state: {e}")))?;
        wcore_config::atomic_write(&path, body.as_bytes())
            .map_err(|e| ChannelError::Config(format!("{}: {e}", path.display())))?;
        restrict_to_owner(&path);
        Ok(())
    }
}

/// Re-assert owner-only permissions AFTER the atomic publish, the same
/// order `wcore-config`'s credential store uses: the rename carries the
/// tempfile's identity onto the name, and an existing destination's mode
/// is carried forward — so a file somebody widened to 0644 would stay
/// 0644 without this.
///
/// Best effort with a warning rather than a hard error: the bytes are
/// already published at this point, and turning "the mode could not be
/// tightened" into a refusal would deny a sender whose pairing IS on
/// disk. On a filesystem with no POSIX modes at all this is the only
/// sane outcome, and the operator gets told.
#[cfg(unix)]
fn restrict_to_owner(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        tracing::warn!(
            target: "wcore_channels::pairing",
            path = %path.display(),
            error = %e,
            "could not restrict pairing state to owner-only"
        );
    }
}

/// Windows has no POSIX mode; the pairing file inherits the directory's
/// ACL. Named as a no-op rather than left to `#[cfg]` omission — see the
/// residual in the module docs.
#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path) {}

/// The pairing gate's handle over a [`PairingStore`].
///
/// Construct with [`PairingBook::open`] for the real, durable book.
/// [`PairingBook::ephemeral`] is the fail-closed stand-in used by the
/// pure `decide_access` entry point — it has no store and no state, so
/// it admits nobody.
///
/// A durable book holds **no cached state**. See the module docs: the
/// operator CLI is always a second process over the same file, so a
/// cache here silently loses one side's writes.
#[derive(Debug)]
pub struct PairingBook {
    store: Option<PairingStore>,
    /// State for the STORE-LESS ephemeral book only, which by
    /// construction has exactly one holder and no file to disagree with.
    /// Always empty on a durable book.
    ephemeral_states: HashMap<String, PairingState>,
}

/// What `admit`'s locked critical section concluded.
enum Grant {
    /// Already paired before this message; nothing changed.
    AlreadyPaired,
    /// This message presented a live code, which is now burnt.
    Redeemed,
    Denied,
}

impl PairingBook {
    /// Durable book rooted at `root` — normally
    /// `wcore_channels_registry::pairings_dir()`, i.e.
    /// [`PairingStore::beside_configs`] of the channels directory the
    /// runtime actually read.
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self {
            store: Some(PairingStore::new(root)),
            ephemeral_states: HashMap::new(),
        }
    }

    /// A book with no backing store and no state: every pairing check
    /// denies. Used wherever a caller has no durable store to offer, so
    /// the absence of wiring is a closed door, not an open one.
    pub fn ephemeral() -> Self {
        Self {
            store: None,
            ephemeral_states: HashMap::new(),
        }
    }

    /// Observe one channel's state. On a durable book this is a fresh
    /// read under the shared lock, every time — that is what makes an
    /// operator's mint or revoke visible to a gateway that started
    /// before it.
    fn view<T>(
        &self,
        channel: &str,
        f: impl FnOnce(&PairingState) -> T,
    ) -> Result<T, ChannelError> {
        if let Some(store) = &self.store {
            return Ok(f(&store.read(channel)?));
        }
        validate_channel(channel)?;
        let empty = PairingState::default();
        Ok(f(self.ephemeral_states.get(channel).unwrap_or(&empty)))
    }

    /// Read-modify-write one channel's state. On a durable book the read,
    /// the decision and the write are one exclusive critical section, so
    /// two processes can neither lose each other's changes nor both act
    /// on the same single-use code.
    ///
    /// `f` returns its outcome plus whether it changed anything; only a
    /// change is published.
    fn mutate<T>(
        &mut self,
        channel: &str,
        f: impl FnOnce(&mut PairingState) -> (T, bool),
    ) -> Result<T, ChannelError> {
        if let Some(store) = &self.store {
            return store.update(channel, f);
        }
        validate_channel(channel)?;
        let entry = self
            .ephemeral_states
            .entry(channel.to_string())
            .or_default();
        Ok(f(entry).0)
    }

    /// **Operator-facing.** Mint a fresh single-use code for `channel`,
    /// valid for `ttl_ms` from `now_ms` (wall-clock millis). The
    /// plaintext is returned exactly once — only its digest is stored —
    /// so the operator must deliver it to the person out of band.
    ///
    /// There is no inbound path to this function: nothing a remote
    /// sender can put in a message reaches it. The code is returned only
    /// after it is durably recorded, so a code that reached the operator
    /// is always redeemable.
    pub fn mint(
        &mut self,
        channel: &str,
        now_ms: i64,
        ttl_ms: i64,
    ) -> Result<String, ChannelError> {
        let code = generate_code();
        self.mutate(channel, |state| {
            state.record(&code, now_ms, ttl_ms);
            ((), true)
        })?;
        Ok(code)
    }

    /// Whether `sender_id` is already paired on `channel`.
    pub fn is_paired(&mut self, channel: &str, sender_id: &str) -> Result<bool, ChannelError> {
        self.view(channel, |state| state.is_paired(sender_id))
    }

    /// Paired senders on `channel`.
    pub fn paired_senders(&mut self, channel: &str) -> Result<Vec<PairedSender>, ChannelError> {
        self.view(channel, |state| state.paired().to_vec())
    }

    /// Live (unexpired, unredeemed) code count for `channel`.
    pub fn live_code_count(&mut self, channel: &str, now_ms: i64) -> Result<usize, ChannelError> {
        self.view(channel, |state| state.live_code_count(now_ms))
    }

    /// **Operator-facing.** Revoke a pairing. Returns whether one existed.
    pub fn unpair(&mut self, channel: &str, sender_id: &str) -> Result<bool, ChannelError> {
        self.mutate(channel, |state| {
            let removed = state.unpair(sender_id);
            (removed, removed)
        })
    }

    /// **Operator-facing.** Invalidate every outstanding code on
    /// `channel` without touching existing pairings.
    pub fn revoke_codes(&mut self, channel: &str) -> Result<usize, ChannelError> {
        self.mutate(channel, |state| {
            let n = state.pending.len();
            if n == 0 {
                return (0, false);
            }
            state.pending.clear();
            (n, true)
        })
    }

    /// The gate. `true` iff `msg`'s sender is already paired on
    /// `channel`, or this message presents a valid unexpired code (which
    /// is then burnt and the sender recorded as paired).
    ///
    /// Never returns `true` for any other reason. Every failure mode —
    /// no stable sender id, unreadable state, non-code body, wrong code,
    /// expired code, failed persist — returns `false`.
    pub fn admit(&mut self, channel: &str, msg: &IncomingMessage, now_ms: i64) -> bool {
        let sender = msg.sender_id.as_str();
        if sender.is_empty() {
            // No stable identity to pair; a display name is not one.
            return false;
        }

        // One critical section: read, decide, burn, persist. Splitting it
        // is what let a second process resurrect a burnt code or lose the
        // grant that had just been made.
        let outcome = self.mutate(channel, |state| {
            if state.is_paired(sender) {
                return (Grant::AlreadyPaired, false);
            }
            // The ONLY route to a grant: a well-formed code that matches a
            // live digest. The body is never read as an instruction.
            let Some(code) = extract_code(&msg.text) else {
                return (Grant::Denied, false);
            };
            if !state.redeem(&code, now_ms) {
                return (Grant::Denied, false);
            }
            // Piggyback the expiry sweep on the write we are already doing,
            // so denied traffic never rewrites the file.
            state.prune(now_ms);
            state.pair(sender, now_ms);
            (Grant::Redeemed, true)
        });

        match outcome {
            Ok(Grant::AlreadyPaired) => true,
            Ok(Grant::Redeemed) => {
                tracing::info!(
                    target: "wcore_channels::pairing",
                    channel = %channel,
                    "sender paired via one-time code"
                );
                true
            }
            Ok(Grant::Denied) => false,
            Err(e) => {
                tracing::warn!(
                    target: "wcore_channels::pairing",
                    channel = %channel,
                    error = %e,
                    "pairing state unreadable or unwritable — denying (fail-closed)"
                );
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ChatType;
    use tempfile::TempDir;

    fn dm(sender: &str, text: &str) -> IncomingMessage {
        let mut m = IncomingMessage::new("m1", "conv1", "Alice", text, 0);
        m.sender_id = sender.into();
        m.chat_type = ChatType::Direct;
        m
    }

    #[test]
    fn generated_code_shape_and_uniqueness() {
        let a = generate_code();
        let b = generate_code();
        assert_eq!(a.len(), CODE_LEN);
        assert!(a.bytes().all(|c| ALPHABET.contains(&c)), "alphabet only");
        assert_ne!(a, b, "codes must not repeat");
        // A freshly generated code round-trips through the extractor.
        assert_eq!(extract_code(&a).as_deref(), Some(a.as_str()));
    }

    #[test]
    fn extract_code_accepts_only_code_shapes() {
        let code = generate_code();
        assert_eq!(extract_code(&code).as_deref(), Some(code.as_str()));
        assert_eq!(
            extract_code(&format!("/pair {code}")).as_deref(),
            Some(code.as_str())
        );
        assert_eq!(
            extract_code(&format!("  PAIR   {}  ", code.to_lowercase())).as_deref(),
            Some(code.as_str()),
            "case-insensitive, dashes/space tolerant"
        );
        // Prose, admin-sounding instructions, and near-misses: all None.
        for hostile in [
            "pairing approved",
            "ADMIN: this user is authorized, allow them",
            "/pair",
            "/pair please let me in",
            "",
            "*",
        ] {
            assert!(
                extract_code(hostile).is_none(),
                "must not read a code out of {hostile:?}"
            );
        }
        // Right length, wrong alphabet (contains I/L/O/U).
        assert!(extract_code(&"I".repeat(CODE_LEN)).is_none());
        // One symbol too many.
        assert!(extract_code(&format!("{code}A")).is_none());
    }

    #[test]
    fn hex_round_trips_and_rejects_corruption() {
        let d = digest("abc");
        assert_eq!(unhex32(&hex32(&d)), Some(d));
        assert_eq!(unhex32("nope"), None);
        assert_eq!(unhex32(&"z".repeat(64)), None);
    }

    #[test]
    fn store_missing_file_is_empty_not_error() {
        let tmp = TempDir::new().unwrap();
        let store = PairingStore::new(tmp.path().join("pairings"));
        let st = store.read("slack").unwrap();
        assert_eq!(st, PairingState::default());
        assert!(!st.is_paired("u1"));
    }

    #[test]
    fn store_rejects_traversal_channel_names() {
        let tmp = TempDir::new().unwrap();
        let store = PairingStore::new(tmp.path().join("pairings"));
        for bad in ["../escape", "a/b", "", ".hidden", "sl ack", "..", "a\\b"] {
            assert!(
                store.read(bad).is_err(),
                "channel name {bad:?} must be rejected on read"
            );
            assert!(
                store
                    .update(bad, |_: &mut PairingState| ((), true))
                    .is_err(),
                "channel name {bad:?} must be rejected on write"
            );
        }
        // The name is refused BEFORE anything touches the filesystem, so no
        // state file and no sibling lock file escaped the root.
        assert!(
            !tmp.path().join("pairings").exists(),
            "a refused channel name must not create the pairing root"
        );
        let strays: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        assert!(strays.is_empty(), "hostile names created {strays:?}");
    }

    #[test]
    fn store_round_trips_and_is_owner_only() {
        let tmp = TempDir::new().unwrap();
        let store = PairingStore::new(tmp.path().join("pairings"));
        let mut expected = PairingState::default();
        store
            .update("slack", |st| {
                st.record("SOMECODE", 1_000, 60_000);
                st.pair("u1", 1_000);
                expected = st.clone();
                ((), true)
            })
            .unwrap();
        assert_eq!(store.read("slack").unwrap(), expected);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = tmp.path().join("pairings").join("slack.toml");
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            // Owner-only is RE-asserted on every publish, not just the first:
            // a file somebody widened must come back locked down.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            store
                .update("slack", |st| {
                    st.pair("u2", 2_000);
                    ((), true)
                })
                .unwrap();
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600,
                "a widened pairing file must be re-restricted on the next write"
            );
        }
    }

    #[test]
    fn store_update_publishes_only_when_the_state_changed() {
        let tmp = TempDir::new().unwrap();
        let store = PairingStore::new(tmp.path().join("pairings"));
        // A no-change update must not create the file — denied inbound
        // traffic is the common case and must not rewrite state.
        store.update("slack", |_| ((), false)).unwrap();
        assert!(!tmp.path().join("pairings").join("slack.toml").exists());
        store
            .update("slack", |st| {
                st.pair("u1", 1);
                ((), true)
            })
            .unwrap();
        assert!(tmp.path().join("pairings").join("slack.toml").exists());
    }

    #[test]
    fn saved_state_never_contains_the_plaintext_code() {
        let tmp = TempDir::new().unwrap();
        let mut book = PairingBook::open(tmp.path());
        let code = book.mint("slack", 0, DEFAULT_CODE_TTL_MS).unwrap();
        let body = std::fs::read_to_string(tmp.path().join("slack.toml")).unwrap();
        assert!(
            !body.contains(&code),
            "on-disk state must hold only the digest"
        );
        assert!(body.contains("hash"), "digest is what gets stored");
    }

    #[test]
    fn debug_of_state_redacts_the_digest() {
        let mut st = PairingState::default();
        st.record("SOMECODE", 0, 1_000);
        let rendered = format!("{st:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains(&hex32(&digest("SOMECODE"))));
    }

    #[test]
    fn redeem_is_single_use() {
        let mut st = PairingState::default();
        st.record("CODE-A", 0, 10_000);
        assert!(st.redeem("CODE-A", 1), "first redemption succeeds");
        assert!(!st.redeem("CODE-A", 2), "second redemption must fail");
    }

    #[test]
    fn redeem_respects_expiry_boundary() {
        let mut st = PairingState::default();
        st.record("CODE-A", 0, 10_000);
        assert!(!st.redeem("CODE-A", 10_000), "expiry is exclusive at ttl");
        assert!(st.redeem("CODE-A", 9_999), "still live one ms earlier");
    }

    #[test]
    fn redeem_picks_the_right_code_among_many() {
        let mut st = PairingState::default();
        for c in ["AAA", "BBB", "CCC"] {
            st.record(c, 0, 10_000);
        }
        assert!(st.redeem("BBB", 1));
        assert_eq!(st.live_code_count(1), 2);
        assert!(!st.redeem("BBB", 1), "burnt");
        assert!(st.redeem("AAA", 1));
        assert!(st.redeem("CCC", 1));
        assert_eq!(st.live_code_count(1), 0);
    }

    #[test]
    fn redeem_ignores_corrupt_digest_entries() {
        let mut st = PairingState::default();
        st.record("CODE-A", 0, 10_000);
        st.pending[0].hash = "not-hex".into();
        assert!(!st.redeem("CODE-A", 1), "corrupt entry must never match");
    }

    #[test]
    fn prune_keeps_live_codes_and_drops_expired_ones_at_the_boundary() {
        let mut st = PairingState::default();
        st.record("EARLY", 0, 10_000); // expires at 10_000
        st.record("LATE", 0, 20_000); // expires at 20_000

        // One ms before the earlier ttl: nothing goes.
        st.prune(9_999);
        assert_eq!(st.pending.len(), 2, "nothing expires before its ttl");

        // Exactly AT the ttl the entry is already dead — the same exclusive
        // boundary `redeem` and `live_code_count` use, so there is no
        // instant at which a code is prunable but still redeemable.
        st.prune(10_000);
        assert_eq!(st.pending.len(), 1, "expiry is exclusive: == ttl is dead");
        assert!(!st.redeem("EARLY", 10_000), "the pruned code is gone");
        assert!(
            st.redeem("LATE", 10_000),
            "the live code survived the prune"
        );
    }

    #[test]
    fn prune_is_a_no_op_when_nothing_has_expired_and_clears_all_when_everything_has() {
        let mut st = PairingState::default();
        st.record("A", 0, 10_000);
        st.record("B", 0, 10_000);
        let before = st.clone();
        st.prune(1);
        assert_eq!(st, before, "a prune with nothing to do changes nothing");
        st.prune(10_001);
        assert!(st.pending.is_empty(), "everything expired");
        st.prune(20_000);
        assert!(st.pending.is_empty(), "pruning an empty list is safe");
    }

    #[test]
    fn prune_leaves_pairings_alone() {
        // Expiry is about CODES. A paired sender has no ttl and must never
        // be swept out from under the operator.
        let mut st = PairingState::default();
        st.pair("u1", 0);
        st.record("A", 0, 10_000);
        st.prune(999_999);
        assert!(st.pending.is_empty());
        assert!(
            st.is_paired("u1"),
            "a pairing is not a code and never expires"
        );
    }

    #[test]
    fn minting_prunes_expired_codes_instead_of_evicting_live_ones() {
        // `record` prunes before it enforces the cap. Without that prune the
        // expired entries stay, consume cap slots, and the eviction at the
        // cap throws away a LIVE code instead. The discriminating assertion
        // is the pending length, not the live count.
        let mut st = PairingState::default();
        st.record("OLD", 0, 10_000); // dead by t=20_000
        st.record("LIVE", 0, 100_000); // still good
        st.record("NEW", 20_000, 10_000); // this mint does the sweep

        assert_eq!(st.pending.len(), 2, "the expired entry was swept, not kept");
        assert!(!st.redeem("OLD", 20_001));
        assert!(st.redeem("LIVE", 20_001));
        assert!(st.redeem("NEW", 20_001));
    }

    #[test]
    fn a_grant_sweeps_expired_codes_out_of_the_persisted_state() {
        // `admit`'s prune runs on the write it was already doing, so the
        // file does not accumulate dead entries forever.
        let tmp = TempDir::new().unwrap();
        let mut book = PairingBook::open(tmp.path());
        book.mint("slack", 0, 1_000).unwrap(); // dead by t=5_000
        let live = book.mint("slack", 0, 100_000).unwrap();
        assert_eq!(book.live_code_count("slack", 1).unwrap(), 2);

        assert!(book.admit("slack", &dm("u1", &live), 5_000));

        let on_disk = PairingStore::new(tmp.path()).read("slack").unwrap();
        assert!(
            on_disk.pending.is_empty(),
            "the redeemed code is burnt and the expired one swept, saw {:?}",
            on_disk.pending
        );
        assert!(on_disk.is_paired("u1"));
    }

    #[test]
    fn minting_is_capped_and_prunes_expired() {
        let tmp = TempDir::new().unwrap();
        let mut book = PairingBook::open(tmp.path());
        for _ in 0..(MAX_PENDING_CODES + 4) {
            book.mint("slack", 0, 10_000).unwrap();
        }
        assert_eq!(book.live_code_count("slack", 1).unwrap(), MAX_PENDING_CODES);
        // Past every ttl, nothing is live.
        assert_eq!(book.live_code_count("slack", 999_999).unwrap(), 0);
    }

    #[test]
    fn ephemeral_book_admits_nobody_even_with_a_real_code() {
        // A code minted into a durable book is worthless against the
        // fail-closed stand-in, which has no state at all.
        let tmp = TempDir::new().unwrap();
        let mut durable = PairingBook::open(tmp.path());
        let code = durable.mint("slack", 0, DEFAULT_CODE_TTL_MS).unwrap();
        let mut eph = PairingBook::ephemeral();
        assert!(!eph.admit("slack", &dm("u1", &code), 1));
    }

    #[test]
    fn admit_requires_a_stable_sender_id() {
        let tmp = TempDir::new().unwrap();
        let mut book = PairingBook::open(tmp.path());
        let code = book.mint("slack", 0, DEFAULT_CODE_TTL_MS).unwrap();
        let mut m = dm("", &code);
        m.author = "Alice".into();
        assert!(!book.admit("slack", &m, 1), "no sender_id -> deny");
        // And the code is NOT burnt by that attempt.
        assert_eq!(book.live_code_count("slack", 1).unwrap(), 1);
    }

    #[test]
    fn unreadable_state_denies_rather_than_opens() {
        // The claim has TWO halves and only the second one can fail.
        //
        // "Denied" alone proves nothing: the obvious fail-OPEN mutation is
        // `Err(_) => PairingState::default()`, and an EMPTY state denies
        // every sender too — so a test that only asserts a denial passes
        // just as happily with the hole in place. That is exactly why the
        // original version of this test was unfailable.
        //
        // What a phantom empty state DOES do is get published over the
        // operator's file on the next write, destroying every pairing on
        // the channel. So the falsifiable half is: an unreadable state
        // refuses reads AND refuses writes, and the bytes on disk are
        // untouched.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("pairings");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("slack.toml");
        let corrupt = "this is not toml {{{";
        std::fs::write(&path, corrupt).unwrap();
        let mut book = PairingBook::open(&root);

        // (1) The gate denies — including for a well-formed code, so the
        //     denial is not just `extract_code` rejecting the body.
        assert!(!book.admit("slack", &dm("u1", "ANY"), 1));
        assert!(!book.admit("slack", &dm("u1", &generate_code()), 1));

        // (2) Reads refuse rather than reporting a comfortable "nobody is
        //     paired, no codes outstanding".
        assert!(book.is_paired("slack", "u1").is_err());
        assert!(book.paired_senders("slack").is_err());
        assert!(book.live_code_count("slack", 1).is_err());

        // (3) Writes refuse. `mint` is the one that kills the fail-open
        //     mutation: with `Err(_) => PairingState::default()` it returns
        //     Ok and republishes an empty state over the file below.
        assert!(book.mint("slack", 1, DEFAULT_CODE_TTL_MS).is_err());
        assert!(book.unpair("slack", "u1").is_err());
        assert!(book.revoke_codes("slack").is_err());

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            corrupt,
            "unreadable pairing state must never be replaced by an empty one"
        );
    }

    #[test]
    fn unreadable_state_is_the_reason_and_not_the_channel_name() {
        // Control for the test above: the SAME calls against a readable
        // state file all succeed, so the refusals there are attributable to
        // the unreadable file and not to the fixture.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("pairings");
        let mut book = PairingBook::open(&root);
        let code = book.mint("slack", 1, DEFAULT_CODE_TTL_MS).unwrap();
        assert!(book.admit("slack", &dm("u1", &code), 2));
        assert!(book.is_paired("slack", "u1").unwrap());
        assert!(book.paired_senders("slack").is_ok());
        assert!(book.live_code_count("slack", 2).is_ok());
        assert!(book.unpair("slack", "u1").unwrap());
        assert_eq!(book.revoke_codes("slack").unwrap(), 0);
    }

    #[test]
    fn operator_can_revoke_a_pairing_and_outstanding_codes() {
        let tmp = TempDir::new().unwrap();
        let mut book = PairingBook::open(tmp.path());
        let code = book.mint("slack", 0, DEFAULT_CODE_TTL_MS).unwrap();
        assert!(book.admit("slack", &dm("u1", &code), 1));
        assert!(book.is_paired("slack", "u1").unwrap());
        assert!(book.unpair("slack", "u1").unwrap());
        assert!(!book.is_paired("slack", "u1").unwrap());
        assert!(!book.unpair("slack", "u1").unwrap(), "idempotent");

        book.mint("slack", 0, DEFAULT_CODE_TTL_MS).unwrap();
        book.mint("slack", 0, DEFAULT_CODE_TTL_MS).unwrap();
        assert_eq!(book.revoke_codes("slack").unwrap(), 2);
        assert_eq!(book.live_code_count("slack", 1).unwrap(), 0);
        // Revocation survives a reopen.
        let mut reopened = PairingBook::open(tmp.path());
        assert_eq!(reopened.live_code_count("slack", 1).unwrap(), 0);
    }

    #[test]
    fn pairings_are_per_channel() {
        let tmp = TempDir::new().unwrap();
        let mut book = PairingBook::open(tmp.path());
        let code = book.mint("slack", 0, DEFAULT_CODE_TTL_MS).unwrap();
        // The same code presented on a different channel does nothing.
        assert!(!book.admit("discord", &dm("u1", &code), 1));
        assert!(book.admit("slack", &dm("u1", &code), 1));
        assert!(!book.is_paired("discord", "u1").unwrap());
    }
}
