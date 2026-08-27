//! Confidential persistence for exact provider requests used by recovery.

use std::sync::{Mutex, mpsc};
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use wcore_config::confidential_blob::{
    ConfidentialBlobAad, ConfidentialBlobKey, ConfidentialKeyStoreError,
    load_confidential_blob_key, load_or_create_confidential_blob_key, open_confidential_blob,
    seal_confidential_blob,
};
use wcore_config::config::Config;

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
    #[error(
        "secure recovery storage is unavailable: no OS keyring was usable and no encrypted \
         credentials vault is unlocked. On a headless host set WAYLAND_VAULT_PASSPHRASE_FD (a \
         passphrase file descriptor — preferred) or WAYLAND_VAULT_PASSPHRASE to unlock the \
         encrypted vault, or turn durable sessions off with [session] enabled = false"
    )]
    NoSecureBackendAvailable,
    #[error(
        "secure recovery storage could not be read: the configured store rejected this profile's \
         recovery key. An encrypted vault opened with the wrong unlock passphrase reads this way \
         — re-check the passphrase for this profile"
    )]
    SecureStoreUnreadable,
    #[error(
        "this profile has no stored recovery key, so a sealed request cannot be opened. The key \
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
    #[error(
        "the configured credential store did not answer within {}s, so this profile's \
         recovery key could not be obtained. Unlock or repair the OS keyring for this \
         profile, or turn durable sessions off with [session] enabled = false",
        waited.as_secs()
    )]
    KeyStoreTimedOut { waited: Duration },
    #[error("secure recovery storage is unavailable")]
    Unavailable,
    #[error("recovery confidential request is invalid")]
    Invalid,
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
    key: Option<ConfidentialBlobKey>,
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
    /// When the load was started, so a later caller's budget can be applied
    /// as a DEADLINE on this load rather than as a fresh spend. Without it a
    /// turn that asks from two call sites pays the budget twice.
    started: std::time::Instant,
    /// Whether the outstanding load was allowed to CREATE the key. A
    /// read-only load's failure cannot answer a caller that may create one.
    create: bool,
    rx: mpsc::Receiver<Result<ConfidentialBlobKey, RecoveryConfidentialError>>,
}

/// Where [`RecoveryRequestProtector`] obtains the key.
enum KeySource {
    ConfiguredStore,
    /// A store that never answers — the exact shape of the wedge
    /// [`KEY_STORE_ACQUIRE_BUDGET`] exists for, and the only way to exercise
    /// that budget on a host whose real store answers (or fails) at once.
    #[cfg(any(test, feature = "test-utils"))]
    WedgedForTest,
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
                key: Some(ConfidentialBlobKey::from_slice(bytes).expect("fixed recovery test key")),
                pending: None,
            }),
            key_source: KeySource::ConfiguredStore,
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

    fn with_key<T>(
        &self,
        config: &Config,
        create: bool,
        budget: Duration,
        operation: impl FnOnce(&ConfidentialBlobKey) -> Result<T, RecoveryConfidentialError>,
    ) -> Result<T, RecoveryConfidentialError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| RecoveryConfidentialError::Unavailable)?;
        if state.key.is_none() {
            // Decide the config-determined cause before touching any store, so
            // a plaintext backend is never reported as an environment problem.
            reject_backend_without_confidential_storage(config)?;
            let key = self.acquire_key(&mut state, config, create, budget)?;
            state.key = Some(key);
        }
        operation(
            state
                .key
                .as_ref()
                .ok_or(RecoveryConfidentialError::Unavailable)?,
        )
    }

    /// Obtain the key from the configured store, or give up inside
    /// [`KEY_STORE_ACQUIRE_BUDGET`] and say which of the two happened.
    ///
    /// The load runs on its own thread because the store call is synchronous
    /// and uncancellable: a deadline can only be imposed on the WAIT, never on
    /// the call. That is why a timeout leaves a thread behind, and why the
    /// receiver is kept in [`ProtectorState::pending`] rather than dropped.
    fn acquire_key(
        &self,
        state: &mut ProtectorState,
        config: &Config,
        create: bool,
        budget: Duration,
    ) -> Result<ConfidentialBlobKey, RecoveryConfidentialError> {
        if let Some(pending) = state.pending.take() {
            match pending.rx.try_recv() {
                // The wedged store finally answered. Adopt it whatever the
                // outstanding load was allowed to do: a key is a key.
                Ok(Ok(key)) => return Ok(key),
                // A failure is authoritative for this caller only if the
                // outstanding load had at least this caller's authority. A
                // read-only load reporting "no key stored" does not answer a
                // caller that is allowed to create one.
                Ok(Err(error)) if pending.create || !create => return Err(error),
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
                Err(mpsc::TryRecvError::Empty) => {
                    let remaining = budget.saturating_sub(pending.started.elapsed());
                    match pending.rx.recv_timeout(remaining) {
                        Ok(Ok(key)) => return Ok(key),
                        Ok(Err(error)) if pending.create || !create => return Err(error),
                        Ok(Err(_)) => {}
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            state.pending = Some(pending);
                            return Err(RecoveryConfidentialError::KeyStoreTimedOut {
                                waited: budget,
                            });
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {}
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }
        let (tx, rx) = mpsc::channel();
        let started = std::time::Instant::now();
        let load = self.key_loader(config, create);
        std::thread::Builder::new()
            .name("wayland-recovery-key".to_owned())
            .spawn(move || {
                // The receiver may be long gone; the send failing is the
                // normal end of a load nobody waited for.
                let _ = tx.send(load());
            })
            .map_err(|_| RecoveryConfidentialError::Unavailable)?;
        match rx.recv_timeout(budget) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                state.pending = Some(PendingKeyLoad {
                    create,
                    rx,
                    started,
                });
                Err(RecoveryConfidentialError::KeyStoreTimedOut { waited: budget })
            }
            // The loader thread died without sending. Nothing is known about
            // the key, but nothing is outstanding either.
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(RecoveryConfidentialError::Unavailable)
            }
        }
    }

    fn key_loader(
        &self,
        config: &Config,
        create: bool,
    ) -> Box<dyn FnOnce() -> Result<ConfidentialBlobKey, RecoveryConfidentialError> + Send> {
        match self.key_source {
            KeySource::ConfiguredStore => {
                // Cloned because the load outlives this call by definition
                // once it times out. It happens at most once per engine.
                let config = config.clone();
                Box::new(move || load_key_from_configured_store(&config, create))
            }
            #[cfg(any(test, feature = "test-utils"))]
            KeySource::WedgedForTest => Box::new(|| {
                loop {
                    std::thread::park();
                }
            }),
        }
    }
}

/// The blocking half, run on its own thread by
/// [`RecoveryRequestProtector::acquire_key`].
fn load_key_from_configured_store(
    config: &Config,
    create: bool,
) -> Result<ConfidentialBlobKey, RecoveryConfidentialError> {
    let store = config
        .open_confidential_credentials_store()
        .map_err(|_| RecoveryConfidentialError::NoSecureBackendAvailable)?;
    let loaded = if create {
        load_or_create_confidential_blob_key(&store, KEY_REF)
    } else {
        load_confidential_blob_key(&store, KEY_REF)
    };
    // The store opened, so the backend exists; a failure past this point is
    // about the key itself, not about availability.
    loaded.map_err(|error| match error {
        ConfidentialKeyStoreError::ReadFailed | ConfidentialKeyStoreError::MalformedStoredKey => {
            RecoveryConfidentialError::SecureStoreUnreadable
        }
        ConfidentialKeyStoreError::MissingStoredKey => {
            RecoveryConfidentialError::MissingRecoveryKey
        }
        _ => RecoveryConfidentialError::Unavailable,
    })
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

        // Spend the turn budget first, so a `pending` load exists. Before
        // this fix, that pending load is precisely what made the resume
        // return instantly with a verdict it never earned.
        assert_eq!(
            protector.preflight(&config),
            Err(RecoveryConfidentialError::KeyStoreTimedOut {
                waited: KEY_STORE_ACQUIRE_BUDGET
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
                waited: KEY_STORE_ACQUIRE_BUDGET
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
                waited: KEY_STORE_ACQUIRE_BUDGET
            }),
            "the second TURN entry point must inherit the first one's verdict"
        );
        assert!(
            second_took < KEY_STORE_ACQUIRE_BUDGET / 2,
            "a second call against the SAME outstanding load must not spend the budget \
             again — the turn would then pay it once per call site; took {second_took:?}"
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
            RecoveryConfidentialError::NoSecureBackendAvailable.to_string(),
            RecoveryConfidentialError::SecureStoreUnreadable.to_string(),
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
        let message = RecoveryConfidentialError::NoSecureBackendAvailable.to_string();

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
        let control =
            env_vars_named_in(&RecoveryConfidentialError::NoSecureBackendAvailable.to_string());
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
        let unavailable = RecoveryConfidentialError::NoSecureBackendAvailable.to_string();
        let unreadable = RecoveryConfidentialError::SecureStoreUnreadable.to_string();

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
            RecoveryConfidentialError::NoSecureBackendAvailable,
            RecoveryConfidentialError::SecureStoreUnreadable,
            RecoveryConfidentialError::MissingRecoveryKey,
            RecoveryConfidentialError::Unavailable,
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
