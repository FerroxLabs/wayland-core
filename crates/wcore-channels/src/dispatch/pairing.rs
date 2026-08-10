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
//! ## Fail-closed points
//!
//! - Unknown channel / missing state file -> empty state -> deny.
//! - Unreadable or malformed state file -> deny (never "assume open").
//! - Empty `sender_id` (no stable identity to pair) -> deny.
//! - A grant that cannot be PERSISTED is not granted: the durability
//!   write happens before the in-memory commit, so a restart and this
//!   process can never disagree about who is paired.
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

    /// Drop expired codes. Returns whether anything was removed.
    fn prune(&mut self, now_ms: i64) -> bool {
        let before = self.pending.len();
        self.pending.retain(|p| p.expires_at_ms > now_ms);
        self.pending.len() != before
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

    /// `<channel config root>/pairings`.
    pub fn default_root() -> PathBuf {
        crate::config::ChannelConfigLoader::default_root().join("pairings")
    }

    fn path_for(&self, channel: &str) -> Result<PathBuf, ChannelError> {
        validate_channel(channel)?;
        Ok(self.root.join(format!("{channel}.toml")))
    }

    /// Read a channel's state. A missing file is an EMPTY state (nobody
    /// paired, no live codes) — not an error, and not an open door. A
    /// present-but-unreadable/malformed file IS an error, so the caller
    /// denies rather than silently starting from empty and overwriting
    /// the operator's real pairings.
    pub fn load(&self, channel: &str) -> Result<PairingState, ChannelError> {
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

    /// Write a channel's state atomically (temp file + rename) with
    /// owner-only permissions on unix.
    pub fn save(&self, channel: &str, state: &PairingState) -> Result<(), ChannelError> {
        let path = self.path_for(channel)?;
        std::fs::create_dir_all(&self.root)
            .map_err(|e| ChannelError::Config(format!("{}: {e}", self.root.display())))?;
        let body = toml::to_string_pretty(state)
            .map_err(|e| ChannelError::Config(format!("serialize pairing state: {e}")))?;
        let tmp = path.with_extension("toml.tmp");
        write_private(&tmp, &body)?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            ChannelError::Config(format!("{}: {e}", path.display()))
        })
    }
}

#[cfg(unix)]
fn write_private(path: &Path, body: &str) -> Result<(), ChannelError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| ChannelError::Config(format!("{}: {e}", path.display())))?;
    f.write_all(body.as_bytes())
        .map_err(|e| ChannelError::Config(format!("{}: {e}", path.display())))?;
    f.sync_all()
        .map_err(|e| ChannelError::Config(format!("{}: {e}", path.display())))
}

#[cfg(not(unix))]
fn write_private(path: &Path, body: &str) -> Result<(), ChannelError> {
    std::fs::write(path, body).map_err(|e| ChannelError::Config(format!("{}: {e}", path.display())))
}

/// The pairing gate's handle: per-channel state, lazily loaded from a
/// [`PairingStore`] and written back on every change.
///
/// Construct with [`PairingBook::open`] for the real, durable book.
/// [`PairingBook::ephemeral`] is the fail-closed stand-in used by the
/// pure `decide_access` entry point — it has no store and no state, so
/// it admits nobody.
#[derive(Debug)]
pub struct PairingBook {
    store: Option<PairingStore>,
    states: HashMap<String, PairingState>,
}

impl PairingBook {
    /// Durable book rooted at `root` (see [`PairingStore::default_root`]).
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self {
            store: Some(PairingStore::new(root)),
            states: HashMap::new(),
        }
    }

    /// A book with no backing store and no state: every pairing check
    /// denies. Used wherever a caller has no durable store to offer, so
    /// the absence of wiring is a closed door, not an open one.
    pub fn ephemeral() -> Self {
        Self {
            store: None,
            states: HashMap::new(),
        }
    }

    /// Snapshot of one channel's state, loading it on first touch.
    fn state(&mut self, channel: &str) -> Result<&PairingState, ChannelError> {
        if !self.states.contains_key(channel) {
            let loaded = match &self.store {
                Some(s) => s.load(channel)?,
                None => {
                    validate_channel(channel)?;
                    PairingState::default()
                }
            };
            self.states.insert(channel.to_string(), loaded);
        }
        Ok(self
            .states
            .get(channel)
            .expect("state inserted immediately above"))
    }

    /// Persist `next` FIRST, then commit it in memory. Ordering is
    /// load-bearing: a durability failure must leave both the disk and
    /// this process on the previous state, never disagreeing about who
    /// is paired or which codes are still live.
    fn commit(&mut self, channel: &str, next: PairingState) -> Result<(), ChannelError> {
        if let Some(store) = &self.store {
            store.save(channel, &next)?;
        }
        self.states.insert(channel.to_string(), next);
        Ok(())
    }

    /// **Operator-facing.** Mint a fresh single-use code for `channel`,
    /// valid for `ttl_ms` from `now_ms` (wall-clock millis). The
    /// plaintext is returned exactly once — only its digest is stored —
    /// so the operator must deliver it to the person out of band.
    ///
    /// There is no inbound path to this function: nothing a remote
    /// sender can put in a message reaches it.
    pub fn mint(
        &mut self,
        channel: &str,
        now_ms: i64,
        ttl_ms: i64,
    ) -> Result<String, ChannelError> {
        let mut next = self.state(channel)?.clone();
        let code = generate_code();
        next.record(&code, now_ms, ttl_ms);
        self.commit(channel, next)?;
        Ok(code)
    }

    /// Whether `sender_id` is already paired on `channel`.
    pub fn is_paired(&mut self, channel: &str, sender_id: &str) -> Result<bool, ChannelError> {
        Ok(self.state(channel)?.is_paired(sender_id))
    }

    /// Paired senders on `channel`.
    pub fn paired_senders(&mut self, channel: &str) -> Result<Vec<PairedSender>, ChannelError> {
        Ok(self.state(channel)?.paired().to_vec())
    }

    /// Live (unexpired, unredeemed) code count for `channel`.
    pub fn live_code_count(&mut self, channel: &str, now_ms: i64) -> Result<usize, ChannelError> {
        Ok(self.state(channel)?.live_code_count(now_ms))
    }

    /// **Operator-facing.** Revoke a pairing. Returns whether one existed.
    pub fn unpair(&mut self, channel: &str, sender_id: &str) -> Result<bool, ChannelError> {
        let mut next = self.state(channel)?.clone();
        if !next.unpair(sender_id) {
            return Ok(false);
        }
        self.commit(channel, next)?;
        Ok(true)
    }

    /// **Operator-facing.** Invalidate every outstanding code on
    /// `channel` without touching existing pairings.
    pub fn revoke_codes(&mut self, channel: &str) -> Result<usize, ChannelError> {
        let mut next = self.state(channel)?.clone();
        let n = next.pending.len();
        if n == 0 {
            return Ok(0);
        }
        next.pending.clear();
        self.commit(channel, next)?;
        Ok(n)
    }

    /// The gate. `true` iff `msg`'s sender is already paired on
    /// `channel`, or this message presents a valid unexpired code (which
    /// is then burnt and the sender recorded as paired).
    ///
    /// Never returns `true` for any other reason. Every failure mode —
    /// no stable sender id, unreadable state, non-code body, wrong code,
    /// expired code, failed persist — returns `false`.
    pub fn admit(&mut self, channel: &str, msg: &IncomingMessage, now_ms: i64) -> bool {
        let sender = msg.sender_id.clone();
        if sender.is_empty() {
            // No stable identity to pair; a display name is not one.
            return false;
        }

        let current = match self.state(channel) {
            Ok(s) => s.clone(),
            Err(e) => {
                tracing::warn!(
                    target: "wcore_channels::pairing",
                    channel = %channel,
                    error = %e,
                    "pairing state unreadable — denying (fail-closed)"
                );
                return false;
            }
        };

        if current.is_paired(&sender) {
            return true;
        }

        // The ONLY route to a grant: a well-formed code that matches a
        // live digest. The body is never read as an instruction.
        let Some(code) = extract_code(&msg.text) else {
            return false;
        };

        let mut next = current;
        next.prune(now_ms);
        if !next.redeem(&code, now_ms) {
            return false;
        }
        next.pair(&sender, now_ms);

        if let Err(e) = self.commit(channel, next) {
            tracing::warn!(
                target: "wcore_channels::pairing",
                channel = %channel,
                error = %e,
                "could not persist pairing grant — denying (fail-closed)"
            );
            return false;
        }
        tracing::info!(
            target: "wcore_channels::pairing",
            channel = %channel,
            "sender paired via one-time code"
        );
        true
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
        let st = store.load("slack").unwrap();
        assert_eq!(st, PairingState::default());
        assert!(!st.is_paired("u1"));
    }

    #[test]
    fn store_rejects_traversal_channel_names() {
        let tmp = TempDir::new().unwrap();
        let store = PairingStore::new(tmp.path());
        for bad in ["../escape", "a/b", "", ".hidden", "sl ack"] {
            assert!(
                store.load(bad).is_err(),
                "channel name {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn store_round_trips_and_is_owner_only() {
        let tmp = TempDir::new().unwrap();
        let store = PairingStore::new(tmp.path().join("pairings"));
        let mut st = PairingState::default();
        st.record("SOMECODE", 1_000, 60_000);
        st.pair("u1", 1_000);
        store.save("slack", &st).unwrap();
        assert_eq!(store.load("slack").unwrap(), st);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(tmp.path().join("pairings").join("slack.toml")).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }
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
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("slack.toml"), "this is not toml {{{").unwrap();
        let mut book = PairingBook::open(tmp.path());
        assert!(!book.admit("slack", &dm("u1", "ANY"), 1));
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
