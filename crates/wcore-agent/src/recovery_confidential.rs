//! Confidential persistence for exact provider requests used by recovery.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use wcore_config::confidential_blob::{
    ConfidentialBlobAad, ConfidentialBlobKey, ConfidentialKeyStoreError,
    ConfidentialKeyStoreErrorKind, ConfidentialStoreDiagnostic, ConfidentialStoreStage,
    load_confidential_blob_key, load_or_create_confidential_blob_key, open_confidential_blob,
    seal_confidential_blob,
};
use wcore_config::config::Config;
use wcore_config::credentials::{
    ConfidentialCredentialsStore, CredentialsBackend, CredentialsError, CredentialsStorageConfig,
};

/// The single source of this identifier is `wcore_config`, so the profile-delete
/// purge (`purge_profile_confidential_keys`) deletes exactly what this writes.
/// Two independent spellings is how a key ends up with a writer and no deleter.
const KEY_REF: &str = wcore_config::credentials::RECOVERY_PREPARED_REQUEST_KEY_REF;
const PURPOSE: &str = "recovery.prepared-provider-request.v1";
const ENVELOPE_VERSION: u8 = 1;
const ALGORITHM: &str = "xchacha20-poly1305";

/// Versioned encrypted request carried by a recovery checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SealedPreparedRequest {
    pub(crate) envelope_version: u8,
    pub(crate) algorithm: String,
    pub(crate) ciphertext: String,
}

impl SealedPreparedRequest {
    pub(crate) fn validate(&self) -> Result<(), RecoveryConfidentialError> {
        if self.envelope_version != ENVELOPE_VERSION || self.algorithm != ALGORITHM {
            return Err(RecoveryConfidentialError::Invalid);
        }
        let blob = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&self.ciphertext)
            .map_err(|_| RecoveryConfidentialError::Invalid)?;
        if blob.is_empty()
            || base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&blob) != self.ciphertext
        {
            return Err(RecoveryConfidentialError::Invalid);
        }
        Ok(())
    }
}

/// Durable identities authenticated with one exact prepared request.
#[derive(Debug, Clone)]
pub(crate) struct PreparedRequestBinding<'a> {
    pub(crate) session_id: &'a str,
    pub(crate) turn_id: &'a str,
    pub(crate) checkpoint_id: &'a str,
    pub(crate) checkpoint_version: u64,
    pub(crate) dispatch_id: &'a str,
    pub(crate) conversation_id: &'a str,
    pub(crate) conversation_digest: &'a str,
    pub(crate) message_count: u64,
    pub(crate) request_digest: &'a str,
    pub(crate) turn_index: u64,
    pub(crate) stream_attempt: u32,
    pub(crate) overflow_retried: bool,
    pub(crate) length_wedge_retried: bool,
    pub(crate) posture_authority_digest: &'a str,
}

/// Confidential request failures omit key material, payload, ciphertext and
/// associated-data details.
///
/// They do NOT omit which *configured* backend was refused. That value is
/// written in the operator's own cleartext config file, so repeating it
/// discloses nothing — while collapsing it produced the live UAT defect D3:
/// three unrelated causes rendered as one string that told a user to configure
/// a credentials backend they had already configured.
///
/// # Whether these are fatal is a decided question — read the ADR before changing it
///
/// `NoSecureBackendAvailable` in particular has been decided **twice, in opposite
/// directions**: refuse the turn (2026-07-16, `906287e1`, "fail closed instead of
/// replaying ambiguous effects") and then degrade durable sessions off and run
/// (2026-07-30, `c73ac417`, a release blocker on every keyring-less Linux host).
/// The second decision was taken by a cross-audit panel that was never shown the
/// first, and it turned the first one's test red.
///
/// Both decisions, the measured causation between them, the refutation of the
/// second one's reasoning, and what a future revisit must have in front of it are
/// merged into `docs/decisions/0003-durable-sessions-without-a-secure-store.md`.
///
/// If you arrived here from a red assertion in
/// `crates/wcore-cli/tests/f14_sigkill_recovery.rs`, that test is **not stale** —
/// it encodes the 2026-07-16 side of a live disagreement. Read ADR 0003 §7 before
/// re-pointing or deleting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum RecoveryConfidentialError {
    /// C-3: this variant deliberately offers NO vault-passphrase remedy.
    ///
    /// It is decided by [`reject_backend_without_confidential_storage`], a pure
    /// function of `config.storage.credentials.backend` that reads no
    /// environment, and the credentials layer refuses the plaintext backend at
    /// the top of `confidential_backend_plan` — before
    /// `vault_unlock_material_present()` is consulted at all. An unlock
    /// passphrase therefore cannot move this verdict by one bit on either
    /// level, and it used to be the FIRST thing the message told the operator
    /// to try.
    ///
    /// Letting an unlocked vault override an explicit `backend = "plaintext"`
    /// was the other way to make the advice true, and it is NOT available:
    /// ADR 0003 §3 records `backend = "plaintext"` as "refuse (unchanged)"
    /// while its neighbours were relaxed, and
    /// `durable_sessions_must_be_disabled` short-circuits on it for the same
    /// stated reason — "the operator configured a backend that can never hold
    /// confidential material … it must keep failing loudly at session open".
    /// Honouring the passphrase here would reverse that decision silently, so
    /// the dead remedy is dropped instead.
    #[error(
        "storage.credentials.backend is set to \"plaintext\", which cannot hold the confidential \
         key that durable session recovery requires. This is decided by your configuration \
         alone, not by this host, so no vault passphrase can unlock it: set \
         [storage.credentials] backend = \"keyring\", or delete that setting to get the default \
         \"auto\" (OS keyring, then the encrypted vault), or turn durable sessions off with \
         [session] enabled = false"
    )]
    PlaintextBackendRejected,
    /// No confidential backend could be selected or opened.
    ///
    /// Carries what the SELECTION reported, because "unavailable" was the
    /// whole of what this said and a selection can fail for reasons an
    /// operator can act on differently (wayland#1302 c3).
    #[error(
        "secure recovery storage is unavailable: no OS keyring was usable and no encrypted \
         credentials vault is unlocked. On a headless host set WAYLAND_VAULT_PASSPHRASE_FD (a \
         passphrase file descriptor — preferred) or WAYLAND_VAULT_PASSPHRASE to unlock the \
         encrypted vault, or turn durable sessions off with [session] enabled = false. What the \
         store itself reported: {diagnostic}"
    )]
    NoSecureBackendAvailable {
        diagnostic: ConfidentialStoreDiagnostic,
    },
    /// The store was reached and the key could not be obtained from it.
    ///
    /// Carries the step and the class of what the backend said. Before
    /// wayland#1302 c3 this was payload-free, so a locked keyring, a corrupt
    /// vault file and an undecodable stored value were one sentence.
    #[error(
        "secure recovery storage could not be read: the configured store rejected this profile's \
         recovery key. An encrypted vault opened with the wrong unlock passphrase reads this way \
         — re-check the passphrase for this profile. What the store itself reported: {diagnostic}"
    )]
    SecureStoreUnreadable {
        diagnostic: ConfidentialStoreDiagnostic,
    },
    /// The store answered, and holds no key for this profile.
    ///
    /// Deliberately payload-free: it has exactly one producer — a `get` that
    /// returned `Ok(None)` at the read step — so its store report is a
    /// constant, and stating it in the sentence is both true by construction
    /// and the HEALTHY-store half of wayland#1302 c3.
    #[error(
        "this profile has no stored recovery key, so a sealed request cannot be opened. The \
         configured credential store was asked and answered without reporting any error — it \
         holds no key for this profile, so nothing here shows the store to be at fault. The key \
         is created when a new turn starts on a confidential-capable backend"
    )]
    MissingRecoveryKey,
    /// The configured credential store was asked for this profile's sealing
    /// key and did not answer inside [`KEY_STORE_ACQUIRE_BUDGET`].
    ///
    /// It is the only variant here that is not an ANSWER. Every other one
    /// reports something the store or the config told us; this one reports
    /// that nothing was told to us at all, so whether the key exists is
    /// unknown and stays unknown.
    ///
    /// macOS is where this is real. The store call is a synchronous
    /// `Security.framework` entry point, and a keychain item whose ACL does
    /// not trust the calling binary raises an authorization wait — which a
    /// spawned child of a packaged app has no way to satisfy and no way to
    /// dismiss. Before this variant existed there was no deadline on that
    /// wait anywhere on the path, so the turn did not fail: it stopped, with
    /// nothing on the wire and nothing said to the user.
    ///
    /// Carries the budget that was ACTUALLY spent, because it is not one
    /// number: a turn waits [`KEY_STORE_ACQUIRE_BUDGET`] and a resume waits
    /// [`RESUME_KEY_WAIT_BUDGET`]. Rendering a constant here would have told
    /// an operator who waited thirty seconds that we gave up after five.
    #[error("{}", key_store_timeout_message(*waited, *reach, backend))]
    KeyStoreTimedOut {
        waited: Duration,
        /// How far the load got before the wait expired. See [`KeyStoreReach`].
        reach: KeyStoreReach,
        /// Which store this timeout is about, from the operator's own config.
        backend: &'static str,
    },
    /// Everything else. Carries its store report so a refused write — the
    /// shape of a keyring that reads and will not write — is no longer
    /// indistinguishable from a load this process could not even run.
    #[error("secure recovery storage is unavailable. What the store itself reported: {diagnostic}")]
    Unavailable {
        diagnostic: ConfidentialStoreDiagnostic,
    },
    #[error("recovery confidential request is invalid")]
    Invalid,
}

/// How far a key load got before its wait expired.
///
/// The budget is a WALL-CLOCK deadline on a thread that does the work, so
/// expiry has two causes with nothing in common: the store was asked and did
/// not answer, or the load never reached the store at all because this host
/// had no CPU to run it on. Only the first is the operator's to repair, and
/// wayland#1302 measured the second 104 times against a HEALTHY store while
/// the message asserted the first.
///
/// Observed rather than inferred: the loader closure sets its flag at the
/// moment it enters the store call, so a load the host never scheduled — and
/// a load stuck before the call for any other reason — cannot set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyStoreReach {
    /// The wait expired before the store was asked anything. Nothing is known
    /// about the store, and in particular nothing says it is unhealthy.
    NeverAsked,
    /// The store was asked and had not answered when the wait expired. This
    /// one IS the store's condition to answer for.
    Asked,
}

/// The one place a key-store timeout is put into words, so the two causes
/// cannot drift back into one sentence.
fn key_store_timeout_message(waited: Duration, reach: KeyStoreReach, backend: &str) -> String {
    let seconds = waited.as_secs();
    match reach {
        // The store WAS asked and did not answer. It is the operator's to
        // repair, and this arm is why the fix is not just softer wording:
        // deleting the repair remedy outright would leave a genuinely locked
        // or wedged keyring with no remedy at all.
        KeyStoreReach::Asked => format!(
            "the configured credential store ({backend}) was asked for this profile's recovery \
             key and did not answer within {seconds}s. Unlock or repair the OS keyring for this \
             profile, or turn durable sessions off with [session] enabled = false"
        ),
        // Nothing was asked of the store, so nothing about the store is
        // known. Both of the remedies above assume otherwise: one sends the
        // operator to repair something that may be perfectly healthy, and the
        // other trades a feature away permanently to work around a transient
        // scheduling condition that a later turn recovers from by itself.
        KeyStoreReach::NeverAsked => format!(
            "the wait for this profile's recovery key expired after {seconds}s before the \
             configured credential store ({backend}) was ever asked for it: this host did not \
             run the load in time, so the store was never asked, reported nothing, and nothing \
             here shows it to be at fault. Send the same request again — and if this keeps \
             happening, reduce what else is running on this host. A key that loads later is \
             adopted by a later turn without a restart"
        ),
    }
}

/// Which store a timeout is ABOUT, named from config alone.
///
/// `storage.credentials.backend` is written in the operator's own cleartext
/// config, so repeating it discloses nothing — and without it "the credential
/// store" names nothing an operator with more than one profile can act on.
fn configured_backend_label(config: &Config) -> &'static str {
    match config.storage.credentials.backend {
        CredentialsBackend::Auto => "auto: the OS keyring, then the encrypted vault",
        CredentialsBackend::Keyring => "the OS keyring",
        CredentialsBackend::EncryptedFile { .. } => "the encrypted credentials vault",
        // Unreachable from a wait that can expire: `with_key` runs
        // `reject_backend_without_confidential_storage` before any store is
        // opened, so a plaintext backend fails as `PlaintextBackendRejected`
        // and never reaches `acquire_key`.
        CredentialsBackend::Plaintext => "plaintext",
    }
}

/// The statically decidable half of the confidential-storage requirement.
///
/// `credentials.backend = "plaintext"` can never satisfy it — that refusal is
/// deliberate security design and is unchanged here. What changes is *when* the
/// operator hears about it: this is a pure function of config with no side
/// effects, so a persisted session can refuse to open instead of accepting the
/// session and failing every turn afterwards.
pub(crate) fn reject_backend_without_confidential_storage(
    config: &Config,
) -> Result<(), RecoveryConfidentialError> {
    if config
        .storage
        .credentials
        .backend
        .supports_confidential_material()
    {
        Ok(())
    } else {
        Err(RecoveryConfidentialError::PlaintextBackendRejected)
    }
}

/// How long one turn will wait for the configured credential store to hand
/// over this profile's sealing key before it gives up and says so.
///
/// Five seconds. The reasoning, in the order it constrains the choice:
///
/// * It is the number this product has ALREADY decided means "a user is
///   starting to wonder whether this is dead".
///   `wcore_providers::http_client::STREAM_SILENCE_NOTICE_AFTER` is five
///   seconds and governs the very next step of the same turn. Two different
///   patience budgets on two consecutive steps of one turn would be two
///   answers to one question, and the user experiences the steps as one wait.
/// * A healthy store read is single-digit milliseconds — an OS keyring lookup
///   or a secret-service round trip, not a network call. Five seconds is
///   roughly a thousandfold headroom, so nothing that works is put at risk by
///   it. This is the direction that matters: the budget must not be so tight
///   that a slow-but-working keychain is treated as a wedge.
/// * Being wrong is bounded and self-healing rather than fatal. A store that
///   is merely slow costs crash-replay protection for ONE turn, the user is
///   told exactly that, and because the outstanding load is kept and adopted
///   by a later turn (see [`RecoveryRequestProtector::acquire_key`]) a store
///   that finally answers at t=25s seals normally from the next turn onward.
///
/// Deliberately not configurable. There is no operator whose correct value
/// differs, and the failure it bounds is a wedge, not a slow disk.
pub(crate) const KEY_STORE_ACQUIRE_BUDGET: Duration = Duration::from_secs(5);

/// How long the RESUME-ADMISSION path will wait for the same store.
///
/// Six times [`KEY_STORE_ACQUIRE_BUDGET`], and the asymmetry is deliberate:
/// the two paths are bounding different costs, and are wrong in different
/// directions.
///
/// A turn's budget is bounding DEAD AIR — a user has sent a message and is
/// watching nothing happen — and overrunning it costs only this turn's
/// replay protection, which the user is told about and which the next turn
/// recovers. Five seconds is generous for that.
///
/// Admitting a resume is bounding a ONE-SHOT act the user just asked for,
/// with no stream behind it, and overrunning it costs the whole session:
/// `admit_session_resume` turns any error here into a refusal to open. That
/// makes the errors asymmetric. Too long is a slower `--resume` on a host
/// whose store really is wedged. Too short refuses a session whose sealed
/// request is perfectly readable, on the say-so of a store that was merely
/// slow — and tells the operator to repair a keyring that is not broken.
///
/// This is the direction the turn budget got right and this path got wrong
/// (wayland-core CI, `linux-containerized`, 2026-08-27: three `f14` resume
/// tests refused on a host where the same key loads fine given more time).
pub(crate) const RESUME_KEY_WAIT_BUDGET: Duration = Duration::from_secs(30);

/// ONE further wait, granted only when a budget expired with
/// [`KeyStoreReach::NeverAsked`] — the load never reached the store.
///
/// # Why this exists (wayland#1289)
///
/// [`KEY_STORE_ACQUIRE_BUDGET`] is a wall-clock deadline on a thread that has
/// to be SCHEDULED to do its work. `NeverAsked` is exactly the observation
/// that the deadline expired before the store was asked anything — so what
/// the budget measured was this host's own scheduling, and spending it as if
/// the store had been silent is the defect. Measured: wayland#1302 recorded
/// this reach 104 times against a store that was demonstrably HEALTHY, and
/// wayland#1289 measured every rung of the store itself well inside the
/// budget (keyring 0-4 ms, Argon2id 142 ms to 1244 ms at 192-way) — nothing
/// in the store approaches 5 s, so the expiry was never about the store.
///
/// It is scoped, not a blanket raise, and the scoping is the whole design:
///
/// * `Asked` keeps its 5 s. That IS the store's condition to answer for, and
///   a wedged or ACL-blocked keychain must still be given up on quickly.
/// * The extension is granted ONCE per outstanding load (see
///   [`PendingKeyLoad::extended`]), never once per caller, so a session
///   against a permanently starved host does not extend without bound.
///
/// # What it costs, stated rather than waved at
///
/// This sits on the pre-provider path of every journaled turn, so on a host
/// that never schedules the load the user-visible dead air goes from 5 s to
/// 10 s. That is the trade: 5 s more silence in the case where the current
/// behaviour throws away the turn's replay protection having learned NOTHING
/// about the store. It buys nothing at all in the `Asked` case, which is why
/// it is not granted there.
pub(crate) const KEY_STORE_NEVER_ASKED_EXTENSION: Duration = Duration::from_secs(5);

/// Lazily caches a successfully loaded key for one engine. Backend failures
/// are not cached, so unlocking the configured store can make a later retry
/// succeed without restarting Core.
///
/// # Acquiring the key is BOUNDED, and that bound is the point
///
/// Loading the key is the only blocking call this type makes, and it is a
/// synchronous call into whatever the platform's credential store happens to
/// be. That call can fail to return at all — see
/// [`RecoveryConfidentialError::KeyStoreTimedOut`] — and it sits on the
/// pre-provider path of every journaled turn, ahead of the provider dispatch
/// that produces the first thing a user ever sees.
///
/// All four trait methods funnel through [`Self::with_key`], and
/// [`Self::acquire_key`] is the single place inside it that can block. So the
/// budget is applied there, once, and `preflight`,
/// `sealed_request_key_available`, `seal` and `open` are all bounded by
/// construction — including any caller added later. A bound applied at one
/// caller instead would have left the other three unbounded.
///
/// Sealing is still ATTEMPTED and still PREFERRED. Nothing here weakens what
/// is sealed, what a seal authenticates, or which causes must fail closed;
/// the only thing that changes is that an unbounded wait is now a bounded one
/// with a stated outcome.
pub(crate) struct RecoveryRequestProtector {
    state: Mutex<ProtectorState>,
    key_source: KeySource,
}

#[derive(Default)]
struct ProtectorState {
    /// Shared, not copied, with any other engine that received the same
    /// in-flight load (see [`KeyLoadFlight`]). The key still lives exactly as
    /// long as the engines holding it: nothing outside an engine retains it.
    key: Option<Arc<ConfidentialBlobKey>>,
    /// A load that spent the whole of [`KEY_STORE_ACQUIRE_BUDGET`] without
    /// answering and is still outstanding on its own thread.
    ///
    /// Kept rather than abandoned, because a blocking store call cannot be
    /// cancelled: the thread is stuck either way, and the choice is only
    /// whether its answer is thrown away. Keeping it buys two things. A later
    /// turn adopts the answer if one ever arrives, so a store that unwedges
    /// starts sealing again without a restart; and no second thread is
    /// launched at a store already known not to be answering, so a long
    /// session against a wedged keychain leaks one thread, not one per turn.
    pending: Option<PendingKeyLoad>,
}

struct PendingKeyLoad {
    /// When THIS engine began waiting on the load, so a later caller's budget
    /// can be applied as a DEADLINE rather than as a fresh spend. Without it a
    /// turn that asks from two call sites pays the budget twice. Per engine,
    /// not per load: an engine that joined a load another engine started
    /// still gets its own whole budget.
    started: std::time::Instant,
    /// Whether this engine has already been granted its one
    /// [`KEY_STORE_NEVER_ASKED_EXTENSION`] on this load — a second call site
    /// of the same engine must not buy a second one.
    extended: bool,
    /// The outstanding load, possibly shared with other engines whose store
    /// identity is exactly this one's.
    flight: Arc<KeyLoadFlight>,
}

impl PendingKeyLoad {
    fn reach(&self) -> KeyStoreReach {
        self.flight.reach()
    }
}

/// Where [`RecoveryRequestProtector`] obtains the key.
enum KeySource {
    ConfiguredStore,
    /// A store that never answers — the exact shape of the wedge
    /// [`KEY_STORE_ACQUIRE_BUDGET`] exists for, and the only way to exercise
    /// that budget on a host whose real store answers (or fails) at once.
    #[cfg(any(test, feature = "test-utils"))]
    WedgedForTest,
    /// A load that never reaches the store at all — what a thread this host
    /// never scheduled looks like from the waiting side, and the shape
    /// wayland#1302 measured 104 times against a healthy store.
    #[cfg(any(test, feature = "test-utils"))]
    StarvedForTest,
    /// A load this host was SLOW to schedule, that then reaches the store and
    /// gets an answer — the same starvation as above, except that the thread
    /// eventually runs. It is the only shape that tells granting
    /// [`KEY_STORE_NEVER_ASKED_EXTENSION`] apart from not granting it:
    /// `StarvedForTest` never answers, so it times out either way.
    ///
    /// `cfg(test)` only, not `test-utils`: every other fixture here is
    /// re-exported through an `Engine::use_*` wrapper in `engine.rs`, which is
    /// a `SOURCE_INPUTS` file. This one is needed by a unit test in this
    /// module and nothing else, so it is scoped to where it is used rather
    /// than dragging a corpus regeneration along behind it.
    #[cfg(test)]
    LateButAnsweringForTest,
    /// Backend selection refuses. The one production shape that reaches the
    /// degrade notice carrying a store report (wayland#1302 c3).
    #[cfg(any(test, feature = "test-utils"))]
    SelectionRefusedForTest,
    /// A load that SHARES the way a production load does — keyed by the
    /// production [`KeyLoadIdentity`] — and holds in the store until its gate
    /// opens, then gives its gate's answer. The only double that can put two
    /// engines' loads in flight at once on purpose, which is what grading the
    /// single-flight boundary requires. `cfg(test)` only, for the same reason
    /// as `LateButAnsweringForTest`.
    #[cfg(test)]
    GatedForTest(Arc<TestKeyGate>),
}

/// The store behind [`KeySource::GatedForTest`]: closed until the test opens
/// it, and counting how many loads actually reached it.
#[cfg(test)]
pub(crate) struct TestKeyGate {
    open: Mutex<bool>,
    opened: Condvar,
    loads: std::sync::atomic::AtomicUsize,
    answer: Result<[u8; 32], RecoveryConfidentialError>,
}

#[cfg(test)]
impl TestKeyGate {
    pub(crate) fn new(
        open: bool,
        answer: Result<[u8; 32], RecoveryConfidentialError>,
    ) -> Arc<Self> {
        Arc::new(Self {
            open: Mutex::new(open),
            opened: Condvar::new(),
            loads: std::sync::atomic::AtomicUsize::new(0),
            answer,
        })
    }

    pub(crate) fn release(&self) {
        *self.open.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.opened.notify_all();
    }

    pub(crate) fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }

    fn hold_until_open(&self) {
        let open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let _open = self
            .opened
            .wait_while(open, |open| !*open)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

impl Default for RecoveryRequestProtector {
    fn default() -> Self {
        Self {
            state: Mutex::new(ProtectorState::default()),
            key_source: KeySource::ConfiguredStore,
        }
    }
}

pub(crate) trait RecoveryRequestProtection: Send + Sync {
    /// Prove that crash-durable request protection is available before a
    /// journaled turn is accepted. This may create the profile's sealing key,
    /// but it never writes request content.
    fn preflight(&self, config: &Config) -> Result<(), RecoveryConfidentialError>;

    /// Can sealed material that ALREADY EXISTS be opened?
    ///
    /// Deliberately not [`Self::preflight`], and the difference is the whole
    /// point. `preflight` asks "may I start sealing?" and CREATES the profile's
    /// key to answer yes. This asks "can I read what is already on disk?", and
    /// it must never create anything: the question is only ever asked about a
    /// journal that already contains ciphertext, and minting a fresh key at
    /// that moment would answer "yes" while guaranteeing every subsequent open
    /// fails — the worst of both honest answers.
    ///
    /// On the trait rather than a free function because the answer belongs to
    /// whichever protection the caller actually holds. A free function would
    /// build a fresh `RecoveryRequestProtector` and consult the real store,
    /// which is wrong for any engine carrying an injected key: it would report
    /// a locked session for material it can open perfectly well.
    fn sealed_request_key_available(
        &self,
        config: &Config,
    ) -> Result<(), RecoveryConfidentialError>;

    /// The same question, asked while ADMITTING A RESUME rather than during a
    /// turn — and it is a separate method because the two differ in the one
    /// thing that matters here: how long the answer is worth waiting for.
    ///
    /// `sealed_request_key_available` is also asked mid-turn
    /// (`engine.rs`, before writing a `ProviderDispatch` checkpoint), where a
    /// user is watching nothing happen and the cost of giving up is one
    /// turn's replay protection. `admit_session_resume` asks it once, with no
    /// stream behind it, and turns ANY error into a refusal to open the
    /// session at all. Same question, two budgets.
    ///
    /// Defaulted so that test doubles — which have no store and no budget —
    /// keep answering exactly as they did.
    fn sealed_request_key_available_for_resume(
        &self,
        config: &Config,
    ) -> Result<(), RecoveryConfidentialError> {
        self.sealed_request_key_available(config)
    }

    fn seal(
        &self,
        config: &Config,
        binding: &PreparedRequestBinding<'_>,
        request: &serde_json::Value,
    ) -> Result<SealedPreparedRequest, RecoveryConfidentialError>;

    fn open(
        &self,
        config: &Config,
        binding: &PreparedRequestBinding<'_>,
        sealed: &SealedPreparedRequest,
    ) -> Result<serde_json::Value, RecoveryConfidentialError>;
}

impl RecoveryRequestProtection for RecoveryRequestProtector {
    fn preflight(&self, config: &Config) -> Result<(), RecoveryConfidentialError> {
        self.with_key(config, true, KEY_STORE_ACQUIRE_BUDGET, |_| Ok(()))
    }

    fn sealed_request_key_available(
        &self,
        config: &Config,
    ) -> Result<(), RecoveryConfidentialError> {
        self.with_key(config, false, KEY_STORE_ACQUIRE_BUDGET, |_| Ok(()))
    }

    fn sealed_request_key_available_for_resume(
        &self,
        config: &Config,
    ) -> Result<(), RecoveryConfidentialError> {
        self.with_key(config, false, RESUME_KEY_WAIT_BUDGET, |_| Ok(()))
    }

    fn seal(
        &self,
        config: &Config,
        binding: &PreparedRequestBinding<'_>,
        request: &serde_json::Value,
    ) -> Result<SealedPreparedRequest, RecoveryConfidentialError> {
        self.with_key(config, true, KEY_STORE_ACQUIRE_BUDGET, |key| {
            seal_with_key(key, binding, request)
        })
    }

    fn open(
        &self,
        config: &Config,
        binding: &PreparedRequestBinding<'_>,
        sealed: &SealedPreparedRequest,
    ) -> Result<serde_json::Value, RecoveryConfidentialError> {
        // RECOVERY-side, so it spends the resume budget. `open` is only ever
        // reached with a sealed request that ALREADY EXISTS on disk --
        // `resume_interrupted_turn` is its single production caller
        // (`engine.rs`), and its failure is mapped straight to a terminal
        // `SessionAuthority` refusal of the resume. That makes it the same
        // question `sealed_request_key_available_for_resume` asks, and it
        // must not be answered by a budget sized for dead air mid-turn.
        //
        // The split here is by OPERATION, not by caller, and it is total:
        // `preflight` and `seal` cannot happen except during a turn, `open`
        // cannot happen except during recovery.
        self.with_key(config, false, RESUME_KEY_WAIT_BUDGET, |key| {
            open_with_key(key, binding, sealed)
        })
    }
}

impl RecoveryRequestProtector {
    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn with_test_key(bytes: &[u8; 32]) -> Self {
        Self {
            state: Mutex::new(ProtectorState {
                key: Some(Arc::new(
                    ConfidentialBlobKey::from_slice(bytes).expect("fixed recovery test key"),
                )),
                pending: None,
            }),
            key_source: KeySource::ConfiguredStore,
        }
    }

    /// A protector whose loads share by the production store identity and
    /// hold until `gate` opens. Grades the single-flight boundary.
    #[cfg(test)]
    pub(crate) fn with_gated_key_store_for_test(gate: Arc<TestKeyGate>) -> Self {
        Self {
            state: Mutex::new(ProtectorState::default()),
            key_source: KeySource::GatedForTest(gate),
        }
    }

    /// A protector whose store never answers, for grading the budget.
    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn with_wedged_key_store_for_test() -> Self {
        Self {
            state: Mutex::new(ProtectorState::default()),
            key_source: KeySource::WedgedForTest,
        }
    }

    /// A protector whose key load reaches the store only AFTER the turn
    /// budget has expired, and then gets an answer. Grades
    /// [`KEY_STORE_NEVER_ASKED_EXTENSION`].
    #[cfg(test)]
    pub(crate) fn with_late_key_store_for_test() -> Self {
        Self {
            state: Mutex::new(ProtectorState::default()),
            key_source: KeySource::LateButAnsweringForTest,
        }
    }

    /// A protector whose key load never reaches the store, for grading what
    /// is said when the wait expires with the store untouched.
    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn with_starved_key_store_for_test() -> Self {
        Self {
            state: Mutex::new(ProtectorState::default()),
            key_source: KeySource::StarvedForTest,
        }
    }

    /// A protector whose backend SELECTION refuses, for grading what the
    /// degrade notice says about a store that answered before any rung was
    /// opened. wayland#1302 c3.
    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn with_unselectable_key_store_for_test() -> Self {
        Self {
            state: Mutex::new(ProtectorState::default()),
            key_source: KeySource::SelectionRefusedForTest,
        }
    }

    fn with_key<T>(
        &self,
        config: &Config,
        create: bool,
        budget: Duration,
        operation: impl FnOnce(&ConfidentialBlobKey) -> Result<T, RecoveryConfidentialError>,
    ) -> Result<T, RecoveryConfidentialError> {
        let mut state = self.state.lock().map_err(|_| key_load_machinery_failed())?;
        if state.key.is_none() {
            // Decide the config-determined cause before touching any store, so
            // a plaintext backend is never reported as an environment problem.
            reject_backend_without_confidential_storage(config)?;
            let backend = configured_backend_label(config);
            let key = self.acquire_key(&mut state, config, create, budget, backend)?;
            state.key = Some(key);
        }
        operation(state.key.as_deref().ok_or_else(key_load_machinery_failed)?)
    }

    /// Obtain the key from the configured store, or give up inside
    /// [`KEY_STORE_ACQUIRE_BUDGET`] and say which of the two happened.
    ///
    /// The load runs on its own thread because the store call is synchronous
    /// and uncancellable: a deadline can only be imposed on the WAIT, never on
    /// the call. That is why a timeout leaves a thread behind, and why the
    /// load is kept in [`ProtectorState::pending`] rather than dropped.
    ///
    /// A NEW load is started through [`start_or_join_key_load`], which hands an
    /// engine a load already in flight for exactly the same store identity
    /// instead of starting a second one. That is the whole of wayland#1349's
    /// repair; every wait, budget, extension and authority rule below is
    /// unchanged and is still applied per engine.
    fn acquire_key(
        &self,
        state: &mut ProtectorState,
        config: &Config,
        create: bool,
        budget: Duration,
        backend: &'static str,
    ) -> Result<Arc<ConfidentialBlobKey>, RecoveryConfidentialError> {
        if let Some(pending) = state.pending.take() {
            match pending.flight.wait(Duration::ZERO) {
                // The wedged store finally answered. Adopt it whatever the
                // outstanding load was allowed to do: a key is a key.
                Ok(Ok(key)) => return Ok(key),
                // A failure is authoritative for this caller only if the
                // outstanding load had at least this caller's authority. A
                // read-only load reporting "no key stored" does not answer a
                // caller that is allowed to create one.
                Ok(Err(error)) if pending.flight.create || !create => return Err(error),
                Ok(Err(_)) => {}
                // Still outstanding. Whether to wait again is the CALLER's
                // budget to spend, not a fixed policy: a turn passes the
                // short budget precisely because another five seconds of
                // dead air buys an answer already known not to be coming,
                // while a resume passes a long one because the alternative
                // is refusing the session outright. Waiting zero is still
                // possible and still means the same thing.
                // Still outstanding. The caller's budget is a DEADLINE on
                // this load, measured from when the load began — never a
                // fresh spend. A turn asking from a second call site after
                // the first already burned the whole budget therefore waits
                // ZERO and inherits the verdict, which is the invariant the
                // budget was introduced with. A resume, whose budget is
                // larger than anything a turn has spent, still has time left
                // on the clock and waits out the remainder.
                Err(FlightWait::Outstanding) => {
                    let remaining = budget.saturating_sub(pending.started.elapsed());
                    let mut pending = pending;
                    match pending.flight.wait(remaining) {
                        Ok(Ok(key)) => return Ok(key),
                        Ok(Err(error)) if pending.flight.create || !create => return Err(error),
                        Ok(Err(_)) => {}
                        Err(FlightWait::Outstanding) => {
                            // wayland#1289. The load still has not reached the
                            // store, so this expiry says nothing about the
                            // store — grant its one extension, if this load
                            // has not already had it.
                            let mut waited = budget;
                            if !pending.extended && pending.reach() == KeyStoreReach::NeverAsked {
                                pending.extended = true;
                                match pending.flight.wait(KEY_STORE_NEVER_ASKED_EXTENSION) {
                                    Ok(Ok(key)) => return Ok(key),
                                    Ok(Err(error)) if pending.flight.create || !create => {
                                        return Err(error);
                                    }
                                    Ok(Err(_)) => {}
                                    Err(FlightWait::Outstanding) => {
                                        waited += KEY_STORE_NEVER_ASKED_EXTENSION;
                                    }
                                    Err(FlightWait::Abandoned) => {
                                        return Err(key_load_machinery_failed());
                                    }
                                }
                            }
                            let reach = pending.reach();
                            state.pending = Some(pending);
                            return Err(RecoveryConfidentialError::KeyStoreTimedOut {
                                waited,
                                reach,
                                backend,
                            });
                        }
                        Err(FlightWait::Abandoned) => {}
                    }
                }
                Err(FlightWait::Abandoned) => {}
            }
        }
        let started = std::time::Instant::now();
        let identity = self.key_load_identity(config, create);
        let flight = start_or_join_key_load(identity, create, |identity| {
            self.key_loader(config, create, identity)
        })?;
        match flight.wait(budget) {
            Ok(result) => result,
            Err(FlightWait::Outstanding) => {
                // wayland#1289. The budget is a wall-clock deadline on a
                // thread that has to be scheduled; if it expired without the
                // store having been asked, it measured this host and not the
                // store. Grant exactly one extension for that, and none at
                // all when the store WAS asked and stayed silent.
                let mut waited = budget;
                let mut extended = false;
                if flight.reach() == KeyStoreReach::NeverAsked {
                    extended = true;
                    match flight.wait(KEY_STORE_NEVER_ASKED_EXTENSION) {
                        Ok(result) => return result,
                        Err(FlightWait::Outstanding) => {
                            waited += KEY_STORE_NEVER_ASKED_EXTENSION;
                        }
                        Err(FlightWait::Abandoned) => {
                            return Err(key_load_machinery_failed());
                        }
                    }
                }
                let pending = PendingKeyLoad {
                    started,
                    extended,
                    flight,
                };
                let reach = pending.reach();
                state.pending = Some(pending);
                Err(RecoveryConfidentialError::KeyStoreTimedOut {
                    waited,
                    reach,
                    backend,
                })
            }
            // The loader thread died without answering. Nothing is known about
            // the key, but nothing is outstanding either.
            Err(FlightWait::Abandoned) => Err(key_load_machinery_failed()),
        }
    }

    /// The identity a load from this protector is shared under, or `None` for
    /// a load that must run on its own.
    ///
    /// Only the production store — and the one test double built to grade it —
    /// ever shares. Every other double models one engine's store on purpose,
    /// and the tests that use them grade exactly that.
    fn key_load_identity(&self, config: &Config, create: bool) -> Option<KeyLoadIdentity> {
        let source = match &self.key_source {
            KeySource::ConfiguredStore => KeyLoadSource::ConfiguredStore,
            #[cfg(test)]
            KeySource::GatedForTest(_) => KeyLoadSource::GatedForTest,
            #[cfg(any(test, feature = "test-utils"))]
            _ => return None,
        };
        KeyLoadIdentity::resolve(source, config, create, std::env::current_dir())
    }

    fn key_loader(
        &self,
        config: &Config,
        create: bool,
        identity: Option<&KeyLoadIdentity>,
    ) -> KeyLoader {
        match &self.key_source {
            KeySource::ConfiguredStore => match identity {
                // A SHARED load reads nothing but its identity. The store
                // config and the credentials path are the identity's own
                // captured values, not re-resolved from ambient state, so
                // what every engine sharing this load compared equal on is
                // exactly what the load opens.
                Some(identity) => {
                    let store_config = identity.store_config();
                    let credentials_path = identity.credentials_path.clone();
                    Box::new(move |asked: &AtomicBool| {
                        // Marked HERE, on the loader thread, immediately
                        // before the blocking call and never on the spawning
                        // side: the whole value of the flag is that a load
                        // this host never ran cannot have set it.
                        asked.store(true, Ordering::Release);
                        load_key_from_opened_store(
                            wcore_config::credentials::open_confidential_store(
                                &store_config,
                                &credentials_path,
                            ),
                            create,
                        )
                    })
                }
                None => {
                    // Cloned because the load outlives this call by definition
                    // once it times out. It happens at most once per engine.
                    let config = config.clone();
                    Box::new(move |asked: &AtomicBool| {
                        asked.store(true, Ordering::Release);
                        load_key_from_configured_store(&config, create)
                    })
                }
            },
            #[cfg(any(test, feature = "test-utils"))]
            KeySource::WedgedForTest => Box::new(move |asked: &AtomicBool| {
                asked.store(true, Ordering::Release);
                loop {
                    std::thread::park();
                }
            }),
            // Deliberately never marks: a load that never reaches the store.
            #[cfg(any(test, feature = "test-utils"))]
            KeySource::StarvedForTest => Box::new(|_: &AtomicBool| {
                loop {
                    std::thread::park();
                }
            }),
            // Marks LATE: past the turn budget, comfortably inside the
            // extension. The answer is a real store answer — "this profile
            // has no key" — so a caller that receives it has demonstrably had
            // the store's word, which a timeout can never be mistaken for.
            #[cfg(test)]
            KeySource::LateButAnsweringForTest => Box::new(move |asked: &AtomicBool| {
                std::thread::sleep(KEY_STORE_ACQUIRE_BUDGET + Duration::from_millis(750));
                asked.store(true, Ordering::Release);
                Err(RecoveryConfidentialError::MissingRecoveryKey)
            }),
            #[cfg(any(test, feature = "test-utils"))]
            KeySource::SelectionRefusedForTest => Box::new(move |asked: &AtomicBool| {
                asked.store(true, Ordering::Release);
                Err(store_selection_failure(
                    &CredentialsError::BackendUnavailable(
                        "no confidential credential backend is available".to_owned(),
                    ),
                ))
            }),
            #[cfg(test)]
            KeySource::GatedForTest(gate) => {
                let gate = Arc::clone(gate);
                Box::new(move |asked: &AtomicBool| {
                    gate.loads.fetch_add(1, Ordering::SeqCst);
                    asked.store(true, Ordering::Release);
                    gate.hold_until_open();
                    gate.answer.map(|bytes| {
                        ConfidentialBlobKey::from_slice(&bytes).expect("fixed gated test key")
                    })
                })
            }
        }
    }
}

/// The blocking half of a load, run on its own thread by
/// [`spawn_key_load`].
type KeyLoader =
    Box<dyn FnOnce(&AtomicBool) -> Result<ConfidentialBlobKey, RecoveryConfidentialError> + Send>;

// ---------------------------------------------------------------------------
// Single-flight for key loads — wayland#1349
// ---------------------------------------------------------------------------
//
// THE DEFECT. Every engine owns a `RecoveryRequestProtector`, so every session
// started its own loader thread, and the file-backed store serialises loads on
// `credentials.confidential-key.lock`. Under concurrency all but one load per
// wave outlived its caller's budget, and each one left behind a live thread and
// a held lock fd. Measured on hetzner-dsm: 462 such threads and 461 such fds
// after ~461 sessions at concurrency 32, every session already deleted — ZERO of
// either at concurrency 1. That is the dominant term of the #1349 RSS growth.
//
// THE REPAIR is to stop starting loads that are already running: N engines that
// ask for the key of exactly the same store at the same time share ONE load.
//
// WHAT IS SHARED, AND FOR HOW LONG. Only a load that is IN FLIGHT. A flight is
// removed from the registry the moment it settles — BEFORE its answer is
// published — so a caller either joins while the load is still running or
// starts a fresh one. No answer, success or failure, is ever served to a caller
// that arrived after it was produced: a locked store stays refused on the very
// next load, and a key is dropped with the last engine holding it exactly as
// before. This is deliberately NOT a memo of the key per process.
//
// THE AUTHORIZATION BOUNDARY. Concurrent engines in one process can have
// different homes, different stores and different vault unlock states, and a
// store's refusal is a security verdict. So engines share only when EVERYTHING
// the load reads compares equal — see `KeyLoadIdentity` — and never on the
// backend label, which is identical for two different vaults. Anything that
// cannot be resolved does not share.

/// Which kind of loader a shared identity belongs to, so a test double can
/// never join a production load or the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyLoadSource {
    ConfiguredStore,
    #[cfg(test)]
    GatedForTest,
}

/// Everything a configured-store key load reads, resolved at the moment the
/// load is asked for. Two engines share a load only if these are EQUAL.
///
/// Field by field, from what `load_key_from_opened_store` and the credentials
/// layer beneath it consume:
///
/// * `backend` + `service_name` — the whole of `[storage.credentials]`,
///   including the vault file paths inside `EncryptedFile`. Built back into a
///   `CredentialsStorageConfig` by struct literal in [`Self::store_config`], so
///   a field added to that struct fails to compile here until it is considered.
/// * `credentials_path` — the profile home the store and its backend pin
///   resolve against (`WAYLAND_HOME` / `XDG_DATA_HOME` / the platform dir).
/// * `working_directory` — relative paths above resolve against it.
/// * `environment` — a digest of the COMPLETE process environment, in its own
///   order. Deliberately not a list of the variables the load is known to read
///   (`WAYLAND_HOME`, `WAYLAND_VAULT_PASSPHRASE[_FD]`, the keyring's own): a
///   list would silently miss the next variable someone teaches the store to
///   read, and fail OPEN. Hashing all of it fails closed — any difference, even
///   an irrelevant one, only costs a share.
/// * `create` — a read-only load and a load that may create the key are
///   different questions with different authority, so they never share.
///
/// Not `Debug`, on purpose: nothing about a store's identity needs printing,
/// and the environment digest has no business in a log line.
#[derive(PartialEq, Eq)]
struct KeyLoadIdentity {
    source: KeyLoadSource,
    create: bool,
    backend: CredentialsBackend,
    service_name: Option<String>,
    credentials_path: PathBuf,
    working_directory: PathBuf,
    environment: [u8; 32],
}

impl KeyLoadIdentity {
    /// `None` when any part cannot be resolved; the caller then loads alone.
    fn resolve(
        source: KeyLoadSource,
        config: &Config,
        create: bool,
        working_directory: std::io::Result<PathBuf>,
    ) -> Option<Self> {
        let working_directory = working_directory.ok()?;
        Some(Self {
            source,
            create,
            backend: config.storage.credentials.backend.clone(),
            service_name: config.storage.credentials.service_name.clone(),
            credentials_path: wcore_config::config::credentials_storage_path(),
            working_directory,
            environment: environment_digest(),
        })
    }

    fn store_config(&self) -> CredentialsStorageConfig {
        CredentialsStorageConfig {
            backend: self.backend.clone(),
            service_name: self.service_name.clone(),
        }
    }
}

/// SHA-256 over the whole environment, each name and value length-prefixed.
///
/// The environment carries secrets (a vault passphrase, provider keys), so the
/// identity keeps only this digest, and the transient copies `vars_os` hands
/// out are wiped before they are freed.
fn environment_digest() -> [u8; 32] {
    let mut hasher = Sha256::new();
    for (name, value) in std::env::vars_os() {
        for part in [name, value] {
            let bytes = zeroize::Zeroizing::new(part.into_encoded_bytes());
            hasher.update((bytes.len() as u64).to_be_bytes());
            hasher.update(bytes.as_slice());
        }
    }
    let mut digest = [0_u8; 32];
    digest.copy_from_slice(&hasher.finalize());
    digest
}

/// One key load, and everything waiting on it.
struct KeyLoadFlight {
    /// `None` for a load that was never shareable; such a flight is never
    /// registered and never joined.
    identity: Option<KeyLoadIdentity>,
    /// Whether the load may CREATE the key. A read-only load's failure cannot
    /// answer a caller that may create one.
    create: bool,
    /// Set by the loader thread as it enters the store call, and read by
    /// whichever caller's wait expires.
    asked: AtomicBool,
    outcome: Mutex<FlightOutcome>,
    settled: Condvar,
    /// How many callers joined this load rather than starting their own.
    #[cfg(test)]
    joined: std::sync::atomic::AtomicUsize,
}

enum FlightOutcome {
    Outstanding,
    Answered(Result<Arc<ConfidentialBlobKey>, RecoveryConfidentialError>),
    /// The loader thread ended without answering.
    Abandoned,
}

/// Why a wait on a flight returned without an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlightWait {
    Outstanding,
    Abandoned,
}

impl KeyLoadFlight {
    fn new(identity: Option<KeyLoadIdentity>, create: bool) -> Self {
        Self {
            identity,
            create,
            asked: AtomicBool::new(false),
            outcome: Mutex::new(FlightOutcome::Outstanding),
            settled: Condvar::new(),
            #[cfg(test)]
            joined: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn reach(&self) -> KeyStoreReach {
        if self.asked.load(Ordering::Acquire) {
            KeyStoreReach::Asked
        } else {
            KeyStoreReach::NeverAsked
        }
    }

    /// Wait up to `timeout` for the answer. Every waiter receives the same
    /// answer, as many times as it asks.
    fn wait(
        &self,
        timeout: Duration,
    ) -> Result<Result<Arc<ConfidentialBlobKey>, RecoveryConfidentialError>, FlightWait> {
        let outcome = self.outcome.lock().unwrap_or_else(PoisonError::into_inner);
        let (outcome, _) = self
            .settled
            .wait_timeout_while(outcome, timeout, |outcome| {
                matches!(outcome, FlightOutcome::Outstanding)
            })
            .unwrap_or_else(PoisonError::into_inner);
        match &*outcome {
            FlightOutcome::Outstanding => Err(FlightWait::Outstanding),
            FlightOutcome::Abandoned => Err(FlightWait::Abandoned),
            FlightOutcome::Answered(result) => Ok(result.clone()),
        }
    }

    /// Record the answer. Leaves the registry FIRST, so no caller can join a
    /// load whose answer already exists — see the section comment above.
    /// Idempotent: only the first settlement is kept.
    fn settle(&self, answer: FlightOutcome) {
        if self.identity.is_some()
            && let Ok(mut in_flight) = KEY_LOADS_IN_FLIGHT.lock()
        {
            in_flight.retain(|flight| !std::ptr::eq(Arc::as_ptr(flight), self));
        }
        {
            let mut outcome = self.outcome.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(*outcome, FlightOutcome::Outstanding) {
                *outcome = answer;
            }
        }
        self.settled.notify_all();
    }
}

/// Every shareable load currently running in this process.
static KEY_LOADS_IN_FLIGHT: Mutex<Vec<Arc<KeyLoadFlight>>> = Mutex::new(Vec::new());

/// Join the in-flight load for exactly `identity`, or start one.
///
/// Fails CLOSED at every step it cannot vouch for: no identity, or a registry
/// whose lock is poisoned, both mean an independent load that nobody can join.
fn start_or_join_key_load(
    identity: Option<KeyLoadIdentity>,
    create: bool,
    make_loader: impl FnOnce(Option<&KeyLoadIdentity>) -> KeyLoader,
) -> Result<Arc<KeyLoadFlight>, RecoveryConfidentialError> {
    let Some(identity) = identity else {
        return spawn_key_load(KeyLoadFlight::new(None, create), make_loader(None));
    };
    let Ok(mut in_flight) = KEY_LOADS_IN_FLIGHT.lock() else {
        return spawn_key_load(KeyLoadFlight::new(None, create), make_loader(None));
    };
    if let Some(flight) = in_flight
        .iter()
        .find(|flight| flight.identity.as_ref() == Some(&identity))
    {
        #[cfg(test)]
        flight.joined.fetch_add(1, Ordering::SeqCst);
        return Ok(Arc::clone(flight));
    }
    let load = make_loader(Some(&identity));
    // Spawned while the registry is held, and registered before it is
    // released: a load that finishes at once blocks in `settle` until it is
    // registered, so it can never leave a stale entry behind.
    let flight = spawn_key_load(KeyLoadFlight::new(Some(identity), create), load)?;
    in_flight.push(Arc::clone(&flight));
    Ok(flight)
}

/// Start `load` on its own thread. The thread settles the flight however it
/// ends, including by panic.
fn spawn_key_load(
    flight: KeyLoadFlight,
    load: KeyLoader,
) -> Result<Arc<KeyLoadFlight>, RecoveryConfidentialError> {
    struct SettleOnExit(Arc<KeyLoadFlight>);
    impl Drop for SettleOnExit {
        fn drop(&mut self) {
            self.0.settle(FlightOutcome::Abandoned);
        }
    }

    let flight = Arc::new(flight);
    let for_loader = Arc::clone(&flight);
    std::thread::Builder::new()
        .name("wayland-recovery-key".to_owned())
        .spawn(move || {
            // Built on the loader thread, so a thread that never started
            // settles nothing and touches no registry.
            let settling = SettleOnExit(for_loader);
            let answer = load(&settling.0.asked);
            settling
                .0
                .settle(FlightOutcome::Answered(answer.map(Arc::new)));
        })
        .map_err(|_| key_load_machinery_failed())?;
    Ok(flight)
}

/// The blocking half, run on its own thread by
/// [`RecoveryRequestProtector::acquire_key`].
fn load_key_from_configured_store(
    config: &Config,
    create: bool,
) -> Result<ConfidentialBlobKey, RecoveryConfidentialError> {
    load_key_from_opened_store(config.open_confidential_credentials_store(), create)
}

/// [`load_key_from_configured_store`] once the store has been opened — the one
/// body both a private and a shared load run.
fn load_key_from_opened_store(
    opened: Result<ConfidentialCredentialsStore, CredentialsError>,
    create: bool,
) -> Result<ConfidentialBlobKey, RecoveryConfidentialError> {
    let store = opened.map_err(|error| store_selection_failure(&error))?;
    let loaded = if create {
        load_or_create_confidential_blob_key(&store, KEY_REF)
    } else {
        load_confidential_blob_key(&store, KEY_REF)
    };
    // The store opened, so the backend exists; a failure past this point is
    // about the key itself, not about availability.
    loaded.map_err(key_load_failure)
}

/// A confidential backend could not be selected or opened.
///
/// Pure, and separated from its one caller so the classification can be graded
/// without a real store: the caller needs a `Config` whose keyring exists.
/// wayland#1302 c3 — the backend error was discarded here entirely, so every
/// selection failure reached the operator as one sentence.
fn store_selection_failure(error: &CredentialsError) -> RecoveryConfidentialError {
    RecoveryConfidentialError::NoSecureBackendAvailable {
        diagnostic: ConfidentialStoreDiagnostic::from_backend_error(
            ConfidentialStoreStage::Select,
            error,
        ),
    }
}

/// The store opened and the key did not come back.
///
/// Carries the store's own report through unchanged. The KIND still decides
/// which sentence and which remedy the operator gets — that mapping is
/// unaltered — and the diagnostic decides what the sentence can say about the
/// store, which before wayland#1302 c3 was nothing.
fn key_load_failure(error: ConfidentialKeyStoreError) -> RecoveryConfidentialError {
    let diagnostic = error.diagnostic();
    match error.kind() {
        ConfidentialKeyStoreErrorKind::ReadFailed
        | ConfidentialKeyStoreErrorKind::MalformedStoredKey => {
            RecoveryConfidentialError::SecureStoreUnreadable { diagnostic }
        }
        ConfidentialKeyStoreErrorKind::MissingStoredKey => {
            RecoveryConfidentialError::MissingRecoveryKey
        }
        ConfidentialKeyStoreErrorKind::InvalidReference
        | ConfidentialKeyStoreErrorKind::WriteFailed
        | ConfidentialKeyStoreErrorKind::LockFailed => {
            RecoveryConfidentialError::Unavailable { diagnostic }
        }
    }
}

/// The load itself could not be run or completed — a poisoned lock, a thread
/// that could not be spawned, a loader that died without answering.
///
/// No backend was reached, so the report says exactly that rather than
/// implying one answered. This is the same discipline
/// [`KeyStoreReach::NeverAsked`] applies to the timeout.
fn key_load_machinery_failed() -> RecoveryConfidentialError {
    RecoveryConfidentialError::Unavailable {
        diagnostic: ConfidentialStoreDiagnostic::local(ConfidentialStoreStage::Load),
    }
}

fn seal_with_key(
    key: &ConfidentialBlobKey,
    binding: &PreparedRequestBinding<'_>,
    request: &serde_json::Value,
) -> Result<SealedPreparedRequest, RecoveryConfidentialError> {
    let plaintext = serde_json::to_vec(request).map_err(|_| RecoveryConfidentialError::Invalid)?;
    let aad = request_aad(binding)?;
    let blob = seal_confidential_blob(key, &aad, &plaintext)
        .map_err(|_| RecoveryConfidentialError::Invalid)?;
    Ok(SealedPreparedRequest {
        envelope_version: ENVELOPE_VERSION,
        algorithm: ALGORITHM.to_owned(),
        ciphertext: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(blob),
    })
}

fn open_with_key(
    key: &ConfidentialBlobKey,
    binding: &PreparedRequestBinding<'_>,
    sealed: &SealedPreparedRequest,
) -> Result<serde_json::Value, RecoveryConfidentialError> {
    sealed.validate()?;
    let blob = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&sealed.ciphertext)
        .map_err(|_| RecoveryConfidentialError::Invalid)?;
    if base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&blob) != sealed.ciphertext {
        return Err(RecoveryConfidentialError::Invalid);
    }
    let aad = request_aad(binding)?;
    let plaintext =
        open_confidential_blob(key, &aad, &blob).map_err(|_| RecoveryConfidentialError::Invalid)?;
    serde_json::from_slice(&plaintext).map_err(|_| RecoveryConfidentialError::Invalid)
}

fn request_aad(
    binding: &PreparedRequestBinding<'_>,
) -> Result<ConfidentialBlobAad, RecoveryConfidentialError> {
    if binding.session_id.is_empty()
        || binding.turn_id.is_empty()
        || binding.checkpoint_id.is_empty()
        || binding.dispatch_id.is_empty()
        || binding.conversation_id.is_empty()
        || binding.conversation_digest.is_empty()
        || binding.request_digest.is_empty()
        || binding.posture_authority_digest.is_empty()
    {
        return Err(RecoveryConfidentialError::Invalid);
    }
    let mut canonical = Vec::new();
    for field in [
        binding.session_id,
        binding.turn_id,
        binding.checkpoint_id,
        binding.dispatch_id,
        binding.conversation_id,
        binding.conversation_digest,
        binding.request_digest,
        binding.posture_authority_digest,
    ] {
        let length = u32::try_from(field.len()).map_err(|_| RecoveryConfidentialError::Invalid)?;
        canonical.extend_from_slice(&length.to_be_bytes());
        canonical.extend_from_slice(field.as_bytes());
    }
    canonical.extend_from_slice(&binding.checkpoint_version.to_be_bytes());
    canonical.extend_from_slice(&binding.message_count.to_be_bytes());
    canonical.extend_from_slice(&binding.turn_index.to_be_bytes());
    canonical.extend_from_slice(&binding.stream_attempt.to_be_bytes());
    canonical.push(u8::from(binding.overflow_retried));
    canonical.push(u8::from(binding.length_wedge_retried));
    Ok(ConfidentialBlobAad::new(PURPOSE, canonical))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use wcore_config::credentials::{CredentialsBackend, CredentialsStorageConfig};

    fn binding<'a>() -> PreparedRequestBinding<'a> {
        PreparedRequestBinding {
            session_id: "session-a",
            turn_id: "turn-a",
            checkpoint_id: "checkpoint-a",
            checkpoint_version: 3,
            dispatch_id: "dispatch-a",
            conversation_id: "conversation-a",
            conversation_digest: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            message_count: 2,
            request_digest: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            turn_index: 1,
            stream_attempt: 0,
            overflow_retried: false,
            length_wedge_retried: false,
            posture_authority_digest: "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        }
    }

    // -----------------------------------------------------------------------
    // wayland#1349 — single-flight key loads, and the boundary they must hold
    // -----------------------------------------------------------------------

    /// The key the UNLOCKED store's load hands out in every test below.
    const UNLOCKED_KEY: [u8; 32] = [0xA1; 32];

    fn eventually(within: Duration, condition: impl Fn() -> bool) -> bool {
        let deadline = std::time::Instant::now() + within;
        loop {
            if condition() {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Callers joined to in-flight GATED loads. Production loads and every
    /// other double are excluded, so no concurrently running test can move it.
    fn gated_joiners() -> usize {
        KEY_LOADS_IN_FLIGHT
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|flight| {
                flight
                    .identity
                    .as_ref()
                    .is_some_and(|identity| identity.source == KeyLoadSource::GatedForTest)
            })
            .map(|flight| flight.joined.load(Ordering::SeqCst))
            .sum()
    }

    fn vault_config(root: &std::path::Path) -> Config {
        config_with_backend(CredentialsBackend::EncryptedFile {
            cipher_path: root.join("credentials.enc"),
            key_params_path: root.join("credentials.kdf.json"),
        })
    }

    fn gated(gate: &Arc<TestKeyGate>) -> RecoveryRequestProtector {
        RecoveryRequestProtector::with_gated_key_store_for_test(Arc::clone(gate))
    }

    fn preflight_in_background(
        protector: RecoveryRequestProtector,
        config: Config,
    ) -> mpsc::Receiver<Result<(), RecoveryConfidentialError>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(protector.preflight(&config));
        });
        rx
    }

    /// Put the UNLOCKED store's load in flight and hold it there; then ask a
    /// second, LOCKED store — built by `locked_session` — while it is.
    /// Returns `(unlocked verdict, locked verdict, whether the locked verdict
    /// arrived while the unlocked load was still in flight)`.
    fn race_locked_against_in_flight_unlocked(
        unlocked_session: impl FnOnce(
            &Arc<TestKeyGate>,
        ) -> mpsc::Receiver<Result<(), RecoveryConfidentialError>>,
        locked_session: impl FnOnce(
            &Arc<TestKeyGate>,
        ) -> mpsc::Receiver<Result<(), RecoveryConfidentialError>>,
    ) -> (
        Result<(), RecoveryConfidentialError>,
        Result<(), RecoveryConfidentialError>,
        bool,
        usize,
    ) {
        let unlocked_gate = TestKeyGate::new(false, Ok(UNLOCKED_KEY));
        let unlocked_verdict = unlocked_session(&unlocked_gate);
        assert!(
            eventually(Duration::from_secs(10), || unlocked_gate.loads() == 1),
            "the unlocked store's load never reached its store, so nothing was in flight and \
             this test grades nothing"
        );

        let locked_gate = TestKeyGate::new(true, Err(read_failure()));
        let locked_verdict = locked_session(&locked_gate);
        let early = locked_verdict.recv_timeout(Duration::from_secs(3));
        let answered_while_in_flight = early.is_ok();

        unlocked_gate.release();
        let unlocked = unlocked_verdict
            .recv_timeout(Duration::from_secs(30))
            .expect("the unlocked session must answer once its store is released");
        let locked = match early {
            Ok(verdict) => verdict,
            Err(_) => locked_verdict
                .recv_timeout(Duration::from_secs(30))
                .expect("the locked session must answer"),
        };
        (
            unlocked,
            locked,
            answered_while_in_flight,
            locked_gate.loads(),
        )
    }

    fn assert_the_locked_session_was_refused_by_its_own_store(
        unlocked: Result<(), RecoveryConfidentialError>,
        locked: Result<(), RecoveryConfidentialError>,
        answered_while_in_flight: bool,
        locked_loads: usize,
    ) {
        // CONTROL: the unlocked load really did produce a key, so a sharing
        // defect had something to hand over. Without this the refusal below
        // could pass because nothing was ever available to leak.
        assert_eq!(
            unlocked,
            Ok(()),
            "control: the unlocked store's in-flight load must produce a key"
        );
        assert_eq!(
            locked,
            Err(read_failure()),
            "AUTHORIZATION BOUNDARY CROSSED: a session whose store refuses was answered with \
             {locked:?}. It joined another store's in-flight key load instead of being refused \
             by its own store"
        );
        assert!(
            answered_while_in_flight,
            "the locked session waited on the unlocked session's in-flight load instead of \
             asking its own store"
        );
        assert_eq!(
            locked_loads, 1,
            "the locked session's own store must have been asked exactly once"
        );
    }

    /// wayland#1349 — THE AUTHORIZATION BOUNDARY, across two vaults.
    ///
    /// Two sessions in one process, two different encrypted vaults with the
    /// SAME backend label. One vault is unlocked and its key load is in
    /// flight; the other vault refuses (the wrong-passphrase read failure). The
    /// refusing session must be refused by its own store — it must NOT be
    /// handed the key the unlocked session is loading.
    ///
    /// Red arm: key the dedupe on the backend label. Both vaults label as "the
    /// encrypted credentials vault", the locked session joins the unlocked
    /// session's load, and receives its key.
    #[test]
    #[serial_test::serial]
    fn a_refusing_vault_is_refused_while_another_vaults_key_load_is_in_flight() {
        let dir = tempfile::tempdir().expect("temp dir");
        let unlocked = vault_config(&dir.path().join("unlocked"));
        let locked = vault_config(&dir.path().join("locked"));
        assert_eq!(
            configured_backend_label(&unlocked),
            configured_backend_label(&locked),
            "non-vacuity: the two stores must share a backend LABEL, or a label-keyed dedupe \
             would keep them apart by accident and this test could not catch it"
        );

        let (unlocked, locked, answered_while_in_flight, locked_loads) =
            race_locked_against_in_flight_unlocked(
                |gate| preflight_in_background(gated(gate), unlocked),
                |gate| preflight_in_background(gated(gate), locked),
            );
        assert_the_locked_session_was_refused_by_its_own_store(
            unlocked,
            locked,
            answered_while_in_flight,
            locked_loads,
        );
    }

    /// wayland#1349 — THE AUTHORIZATION BOUNDARY, across two profile homes.
    ///
    /// The isolated-profile shape: the SAME config in both sessions, so even
    /// the store paths in `[storage.credentials]` are identical, and only the
    /// profile home the store resolves against differs. A key load in flight
    /// for one home must never answer a session in the other.
    ///
    /// Red arm: key the dedupe on the backend label (both are `auto`).
    #[test]
    #[serial_test::serial]
    fn a_session_in_another_profile_home_never_receives_this_homes_in_flight_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let unlocked_home = dir.path().join("unlocked-home");
        let locked_home = dir.path().join("locked-home");
        let config = Config::default();
        let home_probe = std::cell::RefCell::new(None);

        let (unlocked, locked, answered_while_in_flight, locked_loads) =
            race_locked_against_in_flight_unlocked(
                |gate| {
                    *home_probe.borrow_mut() = Some(EnvVarProbe::set(
                        "WAYLAND_HOME",
                        unlocked_home.to_str().expect("utf-8 temp path"),
                    ));
                    preflight_in_background(gated(gate), config.clone())
                },
                |gate| {
                    // The unlocked session's identity is already captured —
                    // its load reached the store — so moving the home now is
                    // exactly a second profile arriving while it is in flight.
                    drop(home_probe.borrow_mut().take());
                    *home_probe.borrow_mut() = Some(EnvVarProbe::set(
                        "WAYLAND_HOME",
                        locked_home.to_str().expect("utf-8 temp path"),
                    ));
                    preflight_in_background(gated(gate), config.clone())
                },
            );
        drop(home_probe.borrow_mut().take());
        assert_the_locked_session_was_refused_by_its_own_store(
            unlocked,
            locked,
            answered_while_in_flight,
            locked_loads,
        );
    }

    /// wayland#1349 — the leak's shape, collapsed. N sessions asking for the
    /// key of ONE store at once start ONE load: one loader thread and one
    /// held key-creation lock, where each used to start its own and strand it.
    ///
    /// Also the POSITIVE CONTROL for the two boundary tests above: sharing must
    /// actually happen, or those tests would pass against an implementation
    /// that never shares anything. Every session proves it holds the one load's
    /// key by sealing with it.
    ///
    /// Red arm: never share (no identity). 32 loads.
    #[test]
    #[serial_test::serial]
    fn concurrent_sessions_on_one_store_start_one_key_load() {
        const SESSIONS: usize = 32;
        let dir = tempfile::tempdir().expect("temp dir");
        let config = vault_config(dir.path());
        let gate = TestKeyGate::new(false, Ok(UNLOCKED_KEY));
        let request = serde_json::json!({"m": 1349});

        let (tx, rx) = mpsc::channel();
        let start_session = || {
            let protector = gated(&gate);
            let config = config.clone();
            let request = request.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(protector.seal(&config, &binding(), &request));
            });
        };
        start_session();
        assert!(
            eventually(Duration::from_secs(10), || gate.loads() == 1),
            "the first session's load never reached its store"
        );
        for _ in 1..SESSIONS {
            start_session();
        }
        let all_joined = eventually(Duration::from_secs(10), || gated_joiners() == SESSIONS - 1);
        gate.release();
        let sealed: Vec<_> = (0..SESSIONS)
            .map(|_| {
                rx.recv_timeout(Duration::from_secs(30))
                    .expect("every session must answer")
            })
            .collect();

        assert_eq!(
            gate.loads(),
            1,
            "{SESSIONS} concurrent sessions on one store started {} key loads — one loader \
             thread and one held lock fd each, which is the wayland#1349 leak",
            gate.loads()
        );
        assert!(
            all_joined,
            "the other sessions never joined the in-flight load"
        );
        let key = ConfidentialBlobKey::from_slice(&UNLOCKED_KEY).expect("fixed test key");
        for sealed in sealed {
            let sealed = sealed.expect("every session must receive the one load's key");
            assert_eq!(
                open_with_key(&key, &binding(), &sealed),
                Ok(request.clone()),
                "a session sealed with a key that was not the one load's key"
            );
        }
    }

    /// wayland#1349 — only a load IN FLIGHT is shared. Once a load has settled,
    /// neither its key nor its refusal answers the next session: a store that is
    /// refusing now is asked and refuses, and a store that recovered is asked
    /// and answers. This is what separates single-flight from a per-process
    /// memo, which would defeat drop-at-session-end and pin a stale verdict.
    ///
    /// Red arm: keep the settled flight in the registry.
    #[test]
    #[serial_test::serial]
    fn a_settled_key_load_answers_no_later_session() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = vault_config(dir.path());

        let unlocked = TestKeyGate::new(true, Ok(UNLOCKED_KEY));
        assert_eq!(gated(&unlocked).preflight(&config), Ok(()));

        let now_refusing = TestKeyGate::new(true, Err(read_failure()));
        assert_eq!(
            gated(&now_refusing).preflight(&config),
            Err(read_failure()),
            "a store that refuses NOW must be asked, not answered with a key an earlier \
             session already loaded"
        );
        assert_eq!(now_refusing.loads(), 1);

        let recovered = TestKeyGate::new(true, Ok(UNLOCKED_KEY));
        assert_eq!(
            gated(&recovered).preflight(&config),
            Ok(()),
            "a store that answers NOW must be asked, not refused on an earlier verdict"
        );
        assert_eq!(recovered.loads(), 1);
    }

    /// wayland#1349 — FAIL CLOSED. What cannot be resolved does not share, and
    /// a load that may create the key never shares with one that may not.
    #[test]
    fn an_unresolvable_or_different_authority_load_is_never_shared() {
        let config = Config::default();
        assert!(
            KeyLoadIdentity::resolve(
                KeyLoadSource::ConfiguredStore,
                &config,
                true,
                Err(std::io::Error::other("working directory removed")),
            )
            .is_none(),
            "an identity whose working directory cannot be resolved must not be shareable"
        );

        let may_create = KeyLoadIdentity::resolve(
            KeyLoadSource::ConfiguredStore,
            &config,
            true,
            Ok(PathBuf::from("/w")),
        )
        .expect("resolvable");
        let read_only = KeyLoadIdentity::resolve(
            KeyLoadSource::ConfiguredStore,
            &config,
            false,
            Ok(PathBuf::from("/w")),
        )
        .expect("resolvable");
        assert!(
            may_create != read_only,
            "a load that may create the key and one that may not must never share"
        );

        let private = start_or_join_key_load(None, true, |_| {
            Box::new(|_: &AtomicBool| Err(RecoveryConfidentialError::MissingRecoveryKey))
        })
        .expect("a private load starts");
        assert!(private.identity.is_none());
        assert!(matches!(
            private.wait(Duration::from_secs(30)),
            Ok(Err(RecoveryConfidentialError::MissingRecoveryKey))
        ));
        assert!(
            !KEY_LOADS_IN_FLIGHT
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .any(|flight| Arc::ptr_eq(flight, &private)),
            "a load with no identity must never be registered for joining"
        );
    }

    /// The bound on the only blocking call this type makes.
    ///
    /// GRADES THE WIRING, not a helper: it goes in through the production
    /// trait method `preflight`, through the production `with_key`, to the
    /// production `acquire_key`. Only the store behind it is swapped, for one
    /// that behaves the way a macOS keychain item with an untrusting ACL
    /// behaves — it never answers. Deleting the `recv_timeout` bound in
    /// `acquire_key` (replacing it with a plain `recv`) is exactly the
    /// pre-fix code and turns this red.
    ///
    /// Driven from a worker thread with its own much longer deadline, so that
    /// mutation FAILS this test instead of hanging it. A test that hangs
    /// under mutation proves nothing anyone can read in a log.
    ///
    /// The second assertion pair is the one that matters for a real turn: the
    /// pre-provider path asks this type the same question from more than one
    /// call site, and a per-call budget would have multiplied the dead air by
    /// the number of them. Both entry points here are TURN entry points, and
    /// both therefore spend [`KEY_STORE_ACQUIRE_BUDGET`]. The resume entry
    /// point is deliberately not one of them — it spends a different budget
    /// on purpose, and is graded by
    /// `a_resume_does_not_inherit_a_turns_shorter_verdict`.
    /// The resume-admission path must NOT be answered by a turn's shorter
    /// give-up.
    ///
    /// `admit_session_resume` turns any error from
    /// `sealed_request_key_available` into a refusal to open the session at
    /// all, so inheriting a five-second verdict there costs the whole
    /// session — and costs it on the word of a store that was merely slow.
    /// wayland-core CI reproduced exactly that on `linux-containerized`:
    /// three `f14` resume tests refused on a host where the same key loads
    /// fine given more time.
    ///
    /// Graded WITHOUT waiting out `RESUME_KEY_WAIT_BUDGET`. The property is
    /// "it is still waiting", not "it eventually gave up", so the test
    /// observes the call from outside and asserts that it has NOT returned
    /// once a turn's whole budget has elapsed twice over. The wedged store
    /// never answers, so the worker parks for the life of this test process —
    /// which nextest gives each test anyway.
    #[test]
    fn a_resume_does_not_inherit_a_turns_shorter_verdict() {
        assert!(
            RESUME_KEY_WAIT_BUDGET > KEY_STORE_ACQUIRE_BUDGET,
            "the two budgets differing is the whole point of this path"
        );

        let protector = RecoveryRequestProtector::with_wedged_key_store_for_test();
        let config = Config::default();
        let backend = configured_backend_label(&config);

        // Spend the turn budget first, so a `pending` load exists. Before
        // this fix, that pending load is precisely what made the resume
        // return instantly with a verdict it never earned.
        assert_eq!(
            protector.preflight(&config),
            Err(RecoveryConfidentialError::KeyStoreTimedOut {
                waited: KEY_STORE_ACQUIRE_BUDGET,
                reach: KeyStoreReach::Asked,
                backend,
            }),
            "the turn entry point must still give up at its own budget"
        );

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(protector.sealed_request_key_available_for_resume(&config));
        });

        match rx.recv_timeout(KEY_STORE_ACQUIRE_BUDGET * 2) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("the resume worker died instead of waiting")
            }
            Ok(verdict) => panic!(
                "the resume path returned {verdict:?} after less than {:?} — it inherited \
                 the turn's give-up instead of spending {RESUME_KEY_WAIT_BUDGET:?} of its \
                 own, and a session that could have been resumed is refused",
                KEY_STORE_ACQUIRE_BUDGET * 2
            ),
        }
    }

    #[test]
    fn a_wedged_key_store_gives_up_inside_its_budget_at_every_entry_point() {
        let protector = RecoveryRequestProtector::with_wedged_key_store_for_test();
        let config = Config::default();
        let backend = configured_backend_label(&config);
        assert_eq!(
            reject_backend_without_confidential_storage(&config),
            Ok(()),
            "the default profile must reach the store at all, or this test grades nothing"
        );

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let first_at = std::time::Instant::now();
            let first = protector.preflight(&config);
            let first_took = first_at.elapsed();
            let second_at = std::time::Instant::now();
            let second = protector
                .seal(&config, &binding(), &serde_json::json!({"m": 1}))
                .map(|_| ());
            let _ = tx.send((first, first_took, second, second_at.elapsed()));
        });
        let (first, first_took, second, second_took) = rx
            .recv_timeout(KEY_STORE_ACQUIRE_BUDGET * 12)
            .expect("a credential store that never answers must not hold a turn open forever");

        assert_eq!(
            first,
            Err(RecoveryConfidentialError::KeyStoreTimedOut {
                waited: KEY_STORE_ACQUIRE_BUDGET,
                reach: KeyStoreReach::Asked,
                backend,
            }),
            "a store that never answers must be reported as not having answered, and must \
             report the budget it actually spent"
        );
        assert!(
            first_took >= KEY_STORE_ACQUIRE_BUDGET,
            "the budget must actually be spent before giving up, took {first_took:?}"
        );
        assert!(
            first_took < KEY_STORE_ACQUIRE_BUDGET * 3,
            "giving up must happen at the budget, not somewhere past it: took {first_took:?}"
        );

        assert_eq!(
            second,
            Err(RecoveryConfidentialError::KeyStoreTimedOut {
                waited: KEY_STORE_ACQUIRE_BUDGET,
                reach: KeyStoreReach::Asked,
                backend,
            }),
            "the second TURN entry point must inherit the first one's verdict"
        );
        assert!(
            second_took < KEY_STORE_ACQUIRE_BUDGET / 2,
            "a second call against the SAME outstanding load must not spend the budget \
             again — the turn would then pay it once per call site; took {second_took:?}"
        );
    }

    /// wayland#1302 — the two ways the wait can expire must not be told to
    /// the operator as one thing, and the one that never touched the store
    /// must not send them to repair it.
    ///
    /// This is the whole defect: the starved arm below reaches the same
    /// message against a store that was never asked anything, so the store it
    /// names as broken is, by construction, healthy. 104 measured
    /// reproductions on a real host (wayland#1289) have exactly this shape.
    ///
    /// Both arms run concurrently, on their own threads, because each spends
    /// a whole [`KEY_STORE_ACQUIRE_BUDGET`] inside a `recv_timeout` that
    /// cannot be hurried, and because either double parks its loader for the
    /// life of the test process.
    #[test]
    fn both_key_store_timeout_causes_are_told_apart() {
        let asked_arm = std::thread::spawn(|| {
            RecoveryRequestProtector::with_wedged_key_store_for_test().preflight(&Config::default())
        });
        let starved_arm = std::thread::spawn(|| {
            RecoveryRequestProtector::with_starved_key_store_for_test()
                .preflight(&Config::default())
        });
        let asked = asked_arm.join().expect("the wedged arm must not panic");
        let starved = starved_arm.join().expect("the starved arm must not panic");

        let config = Config::default();
        let backend = configured_backend_label(&config);
        assert_eq!(
            asked,
            Err(RecoveryConfidentialError::KeyStoreTimedOut {
                waited: KEY_STORE_ACQUIRE_BUDGET,
                reach: KeyStoreReach::Asked,
                backend,
            }),
            "a store that was asked and never answered must be recorded as asked"
        );
        assert_eq!(
            starved,
            Err(RecoveryConfidentialError::KeyStoreTimedOut {
                // wayland#1289: a load that never reached the store is granted
                // one KEY_STORE_NEVER_ASKED_EXTENSION before it is given up
                // on, and the message reports what was ACTUALLY spent. The
                // `asked` arm above still reports the bare budget — that
                // contrast is the non-vacuity guard on the extension, and if
                // this fix ever leaked into the Asked path that assertion,
                // not this one, is what fails.
                waited: KEY_STORE_ACQUIRE_BUDGET + KEY_STORE_NEVER_ASKED_EXTENSION,
                reach: KeyStoreReach::NeverAsked,
                backend,
            }),
            "a load that never reached the store must be recorded as never asked"
        );

        let asked = asked.unwrap_err().to_string();
        let starved = starved.unwrap_err().to_string();
        assert_ne!(
            asked, starved,
            "a store that refused to answer and a load that never asked it anything are \
             different failures and cannot share one sentence: {asked:?}"
        );

        // The CONTROL. A store that really was asked and really did not
        // answer is the operator's to repair, and must still say so.
        assert!(
            asked.contains("Unlock or repair"),
            "a genuinely unresponsive store must still get the repair remedy, got {asked:?}"
        );

        // The DEFECT. Nothing was asked of the store, so nothing about it is
        // known — and neither of the remedies that assume otherwise may be
        // offered.
        let lowered = starved.to_lowercase();
        assert!(
            !lowered.contains("repair"),
            "a wait that never reached the store must not send the operator to repair it, \
             got {starved:?}"
        );
        assert!(
            !lowered.contains("unlock"),
            "a wait that never reached the store must not send the operator to unlock it, \
             got {starved:?}"
        );
        assert!(
            !starved.contains("enabled = false"),
            "giving up durable sessions is not the remedy for a transient scheduling \
             condition, got {starved:?}"
        );

        // What IS known reaches the text: which store the wait was about, and
        // that this one never got as far as asking it.
        for message in [&asked, &starved] {
            assert!(
                message.contains(backend),
                "the message must name the configured backend, got {message:?}"
            );
        }
    }

    /// wayland#1289 — a load the host was slow to SCHEDULE gets one bounded
    /// extension, and the store's answer reaches the caller instead of a
    /// timeout that was never about the store.
    ///
    /// The fixture reaches the store 5.75s in, i.e. past
    /// [`KEY_STORE_ACQUIRE_BUDGET`] and inside
    /// [`KEY_STORE_NEVER_ASKED_EXTENSION`], and then answers
    /// `MissingRecoveryKey` — a real store ANSWER, which a timeout can never
    /// be mistaken for. Before the extension this call returned
    /// `KeyStoreTimedOut { reach: NeverAsked }` at 5s: it gave up on a load
    /// that was about to succeed, having learned nothing about the store, and
    /// spent the turn's replay protection to do it.
    ///
    /// The paired NON-VACUITY assertion lives in
    /// `both_key_store_timeout_causes_are_told_apart`: its wedged arm sets
    /// `asked` and never answers, and still reports `waited:
    /// KEY_STORE_ACQUIRE_BUDGET`. If the extension ever leaked into the
    /// `Asked` path — which would double the give-up time on a genuinely
    /// wedged keychain — that test fails and this one would not notice.
    #[test]
    fn a_load_the_host_was_slow_to_schedule_is_given_one_bounded_extension() {
        let started = std::time::Instant::now();
        let late =
            RecoveryRequestProtector::with_late_key_store_for_test().preflight(&Config::default());
        let took = started.elapsed();

        assert_eq!(
            late,
            Err(RecoveryConfidentialError::MissingRecoveryKey),
            "the extension must deliver the store's own answer, not a timeout; got {late:?} \
             after {took:?}"
        );
        assert!(
            took >= KEY_STORE_ACQUIRE_BUDGET,
            "the answer arrived before the budget even expired, so this run did not \
             exercise the extension at all and its pass is vacuous: took {took:?}"
        );
        assert!(
            took < KEY_STORE_ACQUIRE_BUDGET + KEY_STORE_NEVER_ASKED_EXTENSION,
            "the extension is ONE bounded wait, not an unbounded one: took {took:?}"
        );
    }

    #[test]
    fn sealed_request_roundtrips_without_plaintext() {
        let key = ConfidentialBlobKey::generate();
        let request = serde_json::json!({"secret": "F14-UNIQUE-PLAINTEXT-SENTINEL"});

        let sealed = seal_with_key(&key, &binding(), &request).unwrap();

        assert!(!sealed.ciphertext.contains("F14-UNIQUE-PLAINTEXT-SENTINEL"));
        assert_eq!(open_with_key(&key, &binding(), &sealed).unwrap(), request);
    }

    #[test]
    fn wrong_binding_key_or_ciphertext_fails_closed() {
        let key = ConfidentialBlobKey::generate();
        let request = serde_json::json!({"request": "exact"});
        let sealed = seal_with_key(&key, &binding(), &request).unwrap();

        let mut wrong_binding = binding();
        wrong_binding.dispatch_id = "dispatch-b";
        assert_eq!(
            open_with_key(&key, &wrong_binding, &sealed),
            Err(RecoveryConfidentialError::Invalid)
        );
        assert_eq!(
            open_with_key(&ConfidentialBlobKey::generate(), &binding(), &sealed),
            Err(RecoveryConfidentialError::Invalid)
        );

        let mut tampered = sealed;
        let last = tampered.ciphertext.pop().unwrap();
        tampered
            .ciphertext
            .push(if last == 'A' { 'B' } else { 'A' });
        assert_eq!(
            open_with_key(&key, &binding(), &tampered),
            Err(RecoveryConfidentialError::Invalid)
        );
    }

    #[test]
    fn every_durable_binding_field_is_authenticated() {
        let key = ConfidentialBlobKey::generate();
        let request = serde_json::json!({"request": "exact"});
        let original = binding();
        let sealed = seal_with_key(&key, &original, &request).unwrap();
        let mut changed = Vec::new();

        macro_rules! changed_binding {
            ($field:ident, $value:expr) => {{
                let mut binding = original.clone();
                binding.$field = $value;
                changed.push(binding);
            }};
        }
        changed_binding!(session_id, "session-b");
        changed_binding!(turn_id, "turn-b");
        changed_binding!(checkpoint_id, "checkpoint-b");
        changed_binding!(checkpoint_version, 4);
        changed_binding!(dispatch_id, "dispatch-b");
        changed_binding!(conversation_id, "conversation-b");
        changed_binding!(
            conversation_digest,
            "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
        );
        changed_binding!(message_count, 3);
        changed_binding!(
            request_digest,
            "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
        );
        changed_binding!(turn_index, 2);
        changed_binding!(stream_attempt, 1);
        changed_binding!(overflow_retried, true);
        changed_binding!(length_wedge_retried, true);
        changed_binding!(
            posture_authority_digest,
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        );

        for changed_binding in changed {
            assert_eq!(
                open_with_key(&key, &changed_binding, &sealed),
                Err(RecoveryConfidentialError::Invalid)
            );
        }
    }

    #[test]
    fn noncanonical_or_unknown_envelope_fails_closed() {
        let key = ConfidentialBlobKey::generate();
        let request = serde_json::json!({"request": "exact"});
        let mut sealed = seal_with_key(&key, &binding(), &request).unwrap();

        sealed.ciphertext.push('=');
        assert_eq!(
            open_with_key(&key, &binding(), &sealed),
            Err(RecoveryConfidentialError::Invalid)
        );

        let mut sealed = seal_with_key(&key, &binding(), &request).unwrap();
        sealed.algorithm = "unknown".to_owned();
        assert_eq!(
            open_with_key(&key, &binding(), &sealed),
            Err(RecoveryConfidentialError::Invalid)
        );
    }

    #[test]
    fn errors_never_render_request_or_binding_material() {
        let key = ConfidentialBlobKey::generate();
        let request_secret = "F14-REQUEST-SECRET";
        let binding_secret = "F14-BINDING-SECRET";
        let request = serde_json::json!({"secret": request_secret});
        let mut bound = binding();
        bound.turn_id = binding_secret;
        let mut sealed = seal_with_key(&key, &bound, &request).unwrap();
        sealed.ciphertext.push('=');

        let rendered = open_with_key(&key, &bound, &sealed)
            .unwrap_err()
            .to_string();
        assert!(!rendered.contains(request_secret));
        assert!(!rendered.contains(binding_secret));
    }

    /// Stands in for whatever a real backend puts in its error text — a
    /// passphrase, a token, a decoded secret. Nothing on this path may carry
    /// it outward.
    const SENSITIVE_SENTINEL: &str = "SENTINEL-must-not-leak-9f3a2b";

    /// A backend error carrying the sentinel, in the variant only the keyring
    /// backend constructs.
    fn keyring_error() -> CredentialsError {
        CredentialsError::Keyring(SENSITIVE_SENTINEL.to_owned())
    }

    /// Backend SELECTION refused, as `select_confidential_backend` refuses.
    fn selection_failure() -> RecoveryConfidentialError {
        store_selection_failure(&CredentialsError::BackendUnavailable(
            SENSITIVE_SENTINEL.to_owned(),
        ))
    }

    /// The store was reached and the read failed — a locked keyring.
    fn read_failure() -> RecoveryConfidentialError {
        key_load_failure(ConfidentialKeyStoreError::new(
            ConfidentialKeyStoreErrorKind::ReadFailed,
            ConfidentialStoreDiagnostic::from_backend_error(
                ConfidentialStoreStage::Read,
                &keyring_error(),
            ),
        ))
    }

    /// The store answered and holds nothing — a HEALTHY store.
    fn missing_key() -> RecoveryConfidentialError {
        key_load_failure(ConfidentialKeyStoreError::new(
            ConfidentialKeyStoreErrorKind::MissingStoredKey,
            ConfidentialStoreDiagnostic::local(ConfidentialStoreStage::Read),
        ))
    }

    /// wayland#1302 c3, in the words of the criterion: a healthy store and a
    /// locked one must not be indistinguishable in the output.
    ///
    /// Every one of these rendered from a payload-free variant before this
    /// change, so the two read-side arms were byte-identical and the
    /// selection arm said only "unavailable".
    #[test]
    fn what_the_store_reported_reaches_the_message() {
        let selection = selection_failure().to_string();
        let locked = read_failure().to_string();
        let healthy = missing_key().to_string();

        assert!(
            locked.contains("read/keyring-error") && locked.contains("the OS keyring"),
            "a locked store must carry the step, the class and the rung that answered: {locked}"
        );
        assert!(
            healthy.contains("was asked and answered without reporting any error"),
            "a healthy store must be reported as having answered: {healthy}"
        );
        assert!(
            selection.contains("select/backend-unavailable"),
            "a selection refusal must carry the step and class it failed at: {selection}"
        );
        assert_ne!(locked, healthy);
        assert_ne!(locked, selection);
        assert_ne!(healthy, selection);
    }

    /// THE SECURITY CONTROL on c3.
    ///
    /// The suppression this path shipped with exists to keep backend text out
    /// of error chains, and c3 relaxes what is CARRIED, not what is disclosed.
    /// A sentinel embedded in the backend's own error must not reach any
    /// rendering, through `Display` or through `Debug`, at either layer.
    ///
    /// Both halves are asserted together deliberately: an implementation that
    /// leaks nothing because it carries nothing passes the first half and
    /// fails the second, and that is exactly the pre-fix state.
    #[test]
    fn a_sensitive_sentinel_in_a_backend_error_never_reaches_the_operator() {
        for error in [
            selection_failure(),
            read_failure(),
            missing_key(),
            key_load_failure(ConfidentialKeyStoreError::new(
                ConfidentialKeyStoreErrorKind::WriteFailed,
                ConfidentialStoreDiagnostic::from_backend_error(
                    ConfidentialStoreStage::Create,
                    &keyring_error(),
                ),
            )),
            key_load_machinery_failed(),
        ] {
            let rendered = error.to_string();
            assert!(
                !rendered.contains(SENSITIVE_SENTINEL),
                "backend error text reached Display: {rendered}"
            );
            assert!(
                !format!("{error:?}").contains(SENSITIVE_SENTINEL),
                "backend error text reached Debug: {error:?}"
            );
            // The same rendering an operator gets from a refused resume.
            let refusal = crate::recovery::locked_session_refusal("session-sentinel", &error);
            assert!(
                !refusal.contains(SENSITIVE_SENTINEL),
                "backend error text reached the resume refusal: {refusal}"
            );
        }

        // Instrument control: the scan must be able to FIND the sentinel, or
        // every assertion above passes vacuously.
        assert!(
            format!("{}", keyring_error()).contains(SENSITIVE_SENTINEL),
            "the sentinel is not in the backend error under test; the leak scan would pass \
             vacuously"
        );
    }

    /// The store report must reach the surface an operator actually reads, not
    /// only `Display`. `locked_session_refusal` interpolates the cause, so a
    /// report that stops at the enum would still be invisible on a refused
    /// resume.
    #[test]
    fn the_store_report_reaches_the_resume_refusal() {
        let locked = crate::recovery::locked_session_refusal("session-a", &read_failure());
        let healthy = crate::recovery::locked_session_refusal("session-a", &missing_key());

        assert!(
            locked.contains("read/keyring-error"),
            "the safe error code must reach the refusal: {locked}"
        );
        assert!(
            locked.contains("the OS keyring"),
            "the rung that answered must reach the refusal: {locked}"
        );
        assert_ne!(
            locked, healthy,
            "a locked store and a healthy one must not produce one refusal string"
        );
    }

    /// A rung is named only where the error identifies one.
    ///
    /// `CredentialsError::Keyring` is constructed by the keyring backend and
    /// nothing else. `BackendUnavailable` is produced by selection and by more
    /// than one rung, so attributing it to a rung would be the same class of
    /// fabrication wayland#1302 is about.
    #[test]
    fn an_ambiguous_backend_error_is_not_attributed_to_a_rung() {
        let ambiguous = key_load_failure(ConfidentialKeyStoreError::new(
            ConfidentialKeyStoreErrorKind::ReadFailed,
            ConfidentialStoreDiagnostic::from_backend_error(
                ConfidentialStoreStage::Read,
                &CredentialsError::BackendUnavailable(SENSITIVE_SENTINEL.to_owned()),
            ),
        ))
        .to_string();

        assert!(
            !ambiguous.contains("the OS keyring"),
            "an error several rungs produce must not be attributed to one: {ambiguous}"
        );
        assert!(
            ambiguous.contains("read/backend-unavailable"),
            "the class must still reach the message: {ambiguous}"
        );
    }

    /// c1 is not disturbed: a timeout still reports REACH, and neither arm
    /// invents a store error it never received.
    #[test]
    fn a_timeout_still_reports_reach_and_invents_no_store_error() {
        for (reach, expected) in [
            (KeyStoreReach::Asked, "did not answer"),
            (KeyStoreReach::NeverAsked, "reported nothing"),
        ] {
            let rendered = RecoveryConfidentialError::KeyStoreTimedOut {
                waited: KEY_STORE_ACQUIRE_BUDGET,
                reach,
                backend: "the OS keyring",
            }
            .to_string();
            assert!(rendered.contains(expected), "{rendered}");
            assert!(
                !rendered.contains("store-report"),
                "a wait that expired received no store report and must not carry one: {rendered}"
            );
        }
    }

    fn config_with_backend(backend: CredentialsBackend) -> Config {
        let mut config = Config::default();
        config.storage.credentials = CredentialsStorageConfig {
            backend,
            service_name: None,
        };
        config
    }

    /// D3: the plaintext backend used to be reported as "secure recovery
    /// storage is unavailable; configure an OS keyring or encrypted credentials
    /// vault" — guidance for a user who has configured nothing, given to a user
    /// who has configured exactly the one value that is fatal. The failure must
    /// name itself and name the setting to change.
    #[test]
    fn preflight_fails_with_actionable_guidance_before_request_persistence() {
        let error = RecoveryRequestProtector::default()
            .preflight(&config_with_backend(CredentialsBackend::Plaintext))
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("plaintext"),
            "the cause must be named: {error}"
        );
        assert!(
            error.contains("credentials.backend"),
            "the setting to change must be named: {error}"
        );
        assert!(
            error.contains("session"),
            "the user must be told which capability requires it: {error}"
        );
    }

    /// Every `backend = "<value>"` a remediation string tells an operator to
    /// write must actually be accepted by the config parser it will be written
    /// into.
    ///
    /// This exists because it was NOT true. Both messages advertised
    /// `credentials.backend = "encrypted-file"`, and all three of its parts were
    /// wrong at once, measured live on a keyring-less host:
    ///   * `[credentials]` is not a section — the loader logs "ignoring unknown
    ///     or mis-sectioned config key `credentials` … it has no effect" and
    ///     then re-emits this identical error. A closed loop.
    ///   * at the real section `[storage.credentials]`, `"encrypted-file"` is
    ///     rejected: `unknown variant, expected one of auto, plaintext,
    ///     keyring, encrypted_file` — the config no longer loads AT ALL, so
    ///     following the advice is strictly worse than ignoring it.
    ///   * even `"encrypted_file"` fails, because the variant is
    ///     `EncryptedFile { cipher_path, key_params_path }` — a struct variant
    ///     that can never be a bare string in any spelling.
    ///
    /// The gate can fail: it re-parses whatever the messages say through the
    /// real `CredentialsStorageConfig`, so re-introducing any unrepresentable
    /// value reds it. Verified red against the pre-fix strings.
    #[test]
    fn every_backend_value_the_messages_advertise_actually_parses() {
        let messages = [
            RecoveryConfidentialError::PlaintextBackendRejected.to_string(),
            selection_failure().to_string(),
            read_failure().to_string(),
            RecoveryConfidentialError::MissingRecoveryKey.to_string(),
        ];

        let mut checked = 0usize;
        for message in &messages {
            // Pull every `backend = "value"` / `backend to "value"` the text
            // offers, however it is phrased around the quotes.
            for (index, _) in message.match_indices("backend") {
                let tail = &message[index..];
                let Some(open) = tail.find('"') else { continue };
                let Some(len) = tail[open + 1..].find('"') else {
                    continue;
                };
                let value = &tail[open + 1..open + 1 + len];
                // Only a bare word can be a backend value; skip prose quotes.
                if value.is_empty() || value.contains(' ') {
                    continue;
                }
                let toml = format!("backend = \"{value}\"");
                assert!(
                    toml::from_str::<CredentialsStorageConfig>(&toml).is_ok(),
                    "message advertises backend = \"{value}\", which \
                     [storage.credentials] rejects. An operator who follows this \
                     text literally ends up with a config that will not load.\n\
                     message: {message}"
                );
                checked += 1;
            }
        }
        assert!(
            checked > 0,
            "no backend value was extracted from any message — the gate would \
             pass vacuously; fix the extraction, do not delete the assert"
        );
    }

    /// The messages must point at a remedy that a headless operator can reach
    /// with product-supplied information alone.
    ///
    /// Measured: `WAYLAND_VAULT_PASSPHRASE` appears in ZERO files under `docs/`,
    /// in ZERO bytes of `--help`, and (before this fix) in no error message —
    /// yet setting it, with no config change whatsoever, is the ONE thing that
    /// makes a default install complete a turn on a host with no OS keyring.
    #[test]
    fn the_unavailable_message_names_a_remedy_an_operator_can_actually_perform() {
        let message = selection_failure().to_string();

        assert!(
            message.contains("WAYLAND_VAULT_PASSPHRASE"),
            "the vault unlock transport is the only remedy that works without a \
             config change, and it is documented nowhere else: {message}"
        );

        // The other advertised escape must be spelled exactly as the config
        // schema accepts it, not described in prose.
        assert!(
            message.contains("[session] enabled = false"),
            "the persistence-off escape must be given as a writable config key: {message}"
        );
        assert!(
            toml::from_str::<wcore_config::config::SessionConfig>("enabled = false").is_ok(),
            "the key this message advertises must exist in SessionConfig"
        );
    }

    /// Uppercase `SNAKE_CASE` tokens are how every message in this enum spells
    /// an environment variable, and the surrounding prose is lowercase, so this
    /// cannot pick one up by accident. Deliberately not a `WAYLAND_` prefix
    /// match: a future message that advertises some other process variable is
    /// exactly as dead, and must be caught the same way.
    fn env_vars_named_in(message: &str) -> Vec<String> {
        message
            .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .filter(|token| {
                token.len() >= 4
                    && token.contains('_')
                    && token
                        .chars()
                        .any(|character| character.is_ascii_uppercase())
                    && !token
                        .chars()
                        .any(|character| character.is_ascii_lowercase())
            })
            .map(str::to_owned)
            .collect()
    }

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Sets one environment variable for the length of a probe and puts the
    /// prior value back, including on unwind.
    struct EnvVarProbe {
        name: String,
        prior: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvVarProbe {
        fn set(name: &str, value: &str) -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
            let prior = std::env::var_os(name);
            // SAFETY: env mutation is serialized by `ENV_LOCK` and by
            // `#[serial_test::serial]` on the only test that constructs this.
            unsafe { std::env::set_var(name, value) };
            Self {
                name: name.to_owned(),
                prior,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvVarProbe {
        fn drop(&mut self) {
            // SAFETY: as above.
            match &self.prior {
                Some(value) => unsafe { std::env::set_var(&self.name, value) },
                None => unsafe { std::env::remove_var(&self.name) },
            }
        }
    }

    /// C-3: a remedy a message names must be able to change the verdict of the
    /// code that emits it.
    ///
    /// `PlaintextBackendRejected` has exactly one producer,
    /// [`reject_backend_without_confidential_storage`], whose only input is
    /// `config.storage.credentials.backend`. The shipped message opened with
    /// "Unlock an encrypted vault by setting WAYLAND_VAULT_PASSPHRASE_FD … or
    /// WAYLAND_VAULT_PASSPHRASE" — the FIRST thing it told the operator to try,
    /// and an operator who did it got the byte-identical refusal back. Three
    /// remedies worked; the one printed first could not.
    ///
    /// Deliberately a property, not a string comparison: the variable names are
    /// EXTRACTED from the message, actually set, and the verdict re-measured. A
    /// reword that keeps dead env advice still reds, and if this function ever
    /// does start honouring an unlock variable the gate goes green on its own
    /// rather than having to be edited.
    #[test]
    #[serial_test::serial]
    fn the_plaintext_refusal_names_no_remedy_its_own_verdict_cannot_honour() {
        // Instrument control. An empty extraction below has to mean "no dead
        // advice", never "the scanner stopped working", so prove the scanner
        // finds the variables that ARE named elsewhere in this same enum.
        let control = env_vars_named_in(&selection_failure().to_string());
        for expected in ["WAYLAND_VAULT_PASSPHRASE_FD", "WAYLAND_VAULT_PASSPHRASE"] {
            assert!(
                control.iter().any(|found| found == expected),
                "the env-var scanner is dead: it did not find {expected} in the unavailable \
                 message, so this gate would pass vacuously. Fix the extraction, do not \
                 delete the assert. Found: {control:?}"
            );
        }

        let config = config_with_backend(CredentialsBackend::Plaintext);
        let refused = reject_backend_without_confidential_storage(&config);
        assert_eq!(
            refused,
            Err(RecoveryConfidentialError::PlaintextBackendRejected),
            "positive control: this config must produce the message under test"
        );

        let message = RecoveryConfidentialError::PlaintextBackendRejected.to_string();
        for name in env_vars_named_in(&message) {
            let _probe = EnvVarProbe::set(&name, "c3-remedy-probe");
            assert_ne!(
                reject_backend_without_confidential_storage(&config),
                refused,
                "the message tells an operator to set {name}, but setting it leaves the \
                 verdict of reject_backend_without_confidential_storage unchanged. That \
                 function reads no environment, so this remedy can never resolve the error \
                 it is attached to.\nmessage: {message}"
            );
        }
    }

    /// D3/D8: the plaintext backend and an unavailable secure backend are
    /// different problems with different fixes, so they must not render as one
    /// indistinguishable string.
    #[test]
    fn distinct_confidential_failures_do_not_share_one_message() {
        let plaintext = RecoveryConfidentialError::PlaintextBackendRejected.to_string();
        let unavailable = selection_failure().to_string();
        let unreadable = read_failure().to_string();

        assert_ne!(plaintext, unavailable);
        assert_ne!(plaintext, unreadable);
        assert_ne!(unavailable, unreadable);
    }

    /// The static rule the session-open check uses. Refusing plaintext for
    /// confidential material is the security property being preserved, not
    /// relaxed.
    #[test]
    fn only_plaintext_is_statically_rejected() {
        assert_eq!(
            reject_backend_without_confidential_storage(&config_with_backend(
                CredentialsBackend::Plaintext
            )),
            Err(RecoveryConfidentialError::PlaintextBackendRejected)
        );
        assert_eq!(
            reject_backend_without_confidential_storage(&config_with_backend(
                CredentialsBackend::Auto
            )),
            Ok(())
        );
        assert_eq!(
            reject_backend_without_confidential_storage(&config_with_backend(
                CredentialsBackend::Keyring
            )),
            Ok(())
        );
    }

    /// Naming the configured backend is not a disclosure — the value is written
    /// in the user's own cleartext config. Key material, ciphertext and AAD
    /// still must never appear.
    #[test]
    fn cause_specific_messages_still_render_no_secret_material() {
        for error in [
            RecoveryConfidentialError::PlaintextBackendRejected,
            selection_failure(),
            read_failure(),
            RecoveryConfidentialError::MissingRecoveryKey,
            key_load_machinery_failed(),
            RecoveryConfidentialError::Invalid,
        ] {
            let rendered = error.to_string();
            assert!(!rendered.contains(KEY_REF), "key ref leaked: {rendered}");
            assert!(
                !rendered.contains(PURPOSE),
                "AAD purpose leaked: {rendered}"
            );
            assert!(
                !rendered.contains(ALGORITHM),
                "cipher detail leaked: {rendered}"
            );
        }
    }
}
