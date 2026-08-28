//! Windows AppContainer + Job Objects backend.
//!
//! Tier 0 default on Windows per cross-platform strategy. Builds a per-engine
//! AppContainer profile, derives a restricted token from the current process
//! (`DISABLE_MAX_PRIVILEGE` only — **`SidsToDisable` is `0`/null**, see the
//! 2026-07-23 hardware matrix recorded at the `CreateRestrictedToken` call
//! site; containment comes from the AppContainer/lowbox check and Low IL, not
//! from deny-only group SIDs), pins the child's integrity level to Low via an
//! explicit `SetTokenInformation` call, places the child in a Job Object with
//! memory/CPU/active-process/breakaway/priority caps AND a UI restrictions set
//! (no clipboard, no desktop, no inheriting USER handles, no shutdown), and
//! puts it on the engine's OWN window station and desktop rather than the
//! inherited one (see `windows_impl::window_station`). Image load goes through
//! `CreateProcessAsUserW` with `STARTUPINFOEXW` carrying
//! `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` and a
//! `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` scoped to exactly the two stdout/stderr
//! write ends — every other inheritable handle in the parent process is
//! excluded.
//!
//! > **This paragraph used to claim `BUILTIN\Administrators` / `BUILTIN\Users`
//! > / `Authenticated Users` were passed as `SidsToDisable`. That was false and
//! > had been false since 2026-07-23.** PR FerroxLabs/wayland-core#254 was
//! > written against it and proposed re-enabling SIDs that are not disabled, so
//! > its entire diff is a no-op on this tree. Read the call site, not this
//! > header.
//!
//! Pipeline:
//!   1. Recover any durable leases whose recorded owner process is dead, then
//!      create a unique AppContainer profile for this execution. A name
//!      collision is never reused; another unique identity is allocated.
//!   2. OpenProcessToken + CreateRestrictedToken(DISABLE_MAX_PRIVILEGE,
//!      SidsToDisable=null).
//!   3. SetTokenInformation(TokenIntegrityLevel, S-1-16-4096 Low).
//!   4. CreateJobObjectW + SetInformationJobObject:
//!        - Extended limits: KILL_ON_JOB_CLOSE, ACTIVE_PROCESS=512,
//!          DIE_ON_UNHANDLED_EXCEPTION, PRIORITY_CLASS=BELOW_NORMAL,
//!          (BREAKAWAY_OK=0 / SILENT_BREAKAWAY_OK=0 default), plus
//!          PROCESS_MEMORY / PROCESS_TIME from manifest if set.
//!        - Basic UI restrictions: HANDLES, READCLIPBOARD, WRITECLIPBOARD,
//!          SYSTEMPARAMETERS, DISPLAYSETTINGS, GLOBALATOMS, DESKTOP,
//!          EXITWINDOWS.
//!   5. CreatePipe x2 (stdout + stderr) with inheritable write ends.
//!   6. Build STARTUPINFOEXW attribute list with SECURITY_CAPABILITIES and
//!      HANDLE_LIST=[stdout_w, stderr_w].
//!   7. CreateProcessAsUserW with CREATE_SUSPENDED + EXTENDED_STARTUPINFO_PRESENT.
//!   8. AssignProcessToJobObject (BEFORE ResumeThread).
//!   9. ResumeThread.
//!  10. WaitForSingleObject with manifest timeout (defaults to 60s if None).
//!  11. GetExitCodeProcess.
//!  12. ReadFile drain of both pipe read-ends until EOF.
//!  13. CloseHandle on every owned HANDLE; DeleteProcThreadAttributeList; FreeSid.
//!
//! Resource limits ENFORCED by the Windows kernel via Job Objects — backend
//! returns `ResourceLimitEnforcement::Enforced`.
//!
//! Filesystem allowlists (`fs_read_allow`/`fs_write_allow`) ARE wired to
//! AppContainer DACLs (R61). AppContainer SIDs deny access to user-profile
//! paths by default, so before `CreateProcess` the backend adds an
//! ACCESS_ALLOWED ACE for the AppContainer package SID to each allowlisted
//! path's existing DACL (read+execute for `fs_read_allow`, +write for
//! `fs_write_allow`) via `GetNamedSecurityInfoW` → `SetEntriesInAclW` →
//! `SetNamedSecurityInfoW` — merging into, never replacing, the path's DACL.
//! Each execution durably leases its unique profile/SID and canonical local
//! path intents before mutation. Normal cleanup and dead-owner recovery remove
//! and verify only ACEs matching that exact SID before deleting the profile and
//! lease; malformed or unreconciled lease state fails closed. Paths must be
//! absolute and local — UNC/device paths are rejected so a remote share's DACL
//! is never touched. (`NetworkPolicy::AllowHosts` WFP DNS gating remains queued
//! separately.)

// The probe cache is consumed only by the Windows backend; it is also
// exercised by unit tests on every platform. Gate it to exactly those two so
// it is neither dead code on non-Windows lib builds nor duplicated.
#[cfg(any(windows, test))]
use std::time::{Duration, Instant};

/// How long to wait before a *negative* AppContainer probe verdict may be
/// re-attempted. A positive verdict is sticky for the process lifetime; after a
/// negative one this window throttles re-probing so a transient stall (AV image
/// scan, disk contention, slow profile-service RPC) self-heals without
/// re-running the (wall-clock-guarded) probe on every command
/// (FerroxLabs/wayland-core#125).
///
/// This is a *schedule*, never an answer: the negative verdict itself stays
/// readable until a probe succeeds. See [`ProbeCache::settled`].
///
/// Only the Windows backend consumes this; the cache tests supply their own
/// TTL, so it is `cfg(windows)` (not `test`) to stay dead-code-free elsewhere.
#[cfg(windows)]
const NEGATIVE_PROBE_TTL: Duration = Duration::from_secs(30);

/// Temporal cache verdict for the AppContainer real-spawn probe.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeVerdict {
    /// Never probed. The ONLY state with no answer.
    Unknown,
    /// Probe succeeded — sticky for the process lifetime.
    Available,
    /// Probe failed. The ANSWER is `false` from here until a probe succeeds;
    /// `retry_after` bounds only when the next probe may be attempted.
    Unavailable { retry_after: Instant },
}

/// Availability cache for the AppContainer probe.
///
/// Positive results stick (the sandbox stays available once proven); a negative
/// result throttles re-probing for a short window so a transient failure
/// neither permanently disables isolation (a silent security regression) nor
/// forces a full probe on every command (the ~120s-per-Bash hang of
/// FerroxLabs/wayland-core#125). This temporal logic is platform-independent
/// and unit-tested on all targets; the Windows backend drives it with the real
/// Win32 probe.
///
/// Two questions, deliberately kept apart, because conflating them made the
/// containment predicates oscillate on an unchanged host:
/// - [`Self::settled`] — what do we currently know? Monotone.
/// - [`Self::needs_probe`] — may we spend a probe right now? Temporal.
#[cfg(any(windows, test))]
struct ProbeCache {
    verdict: ProbeVerdict,
}

#[cfg(any(windows, test))]
impl ProbeCache {
    const fn new() -> Self {
        Self {
            verdict: ProbeVerdict::Unknown,
        }
    }

    /// The settled answer, readable WITHOUT probing. `None` only while the
    /// probe has never run.
    ///
    /// **A negative answer never expires.** `NEGATIVE_PROBE_TTL` bounds when
    /// the next probe may be *attempted* ([`Self::needs_probe`]); it must not
    /// also erase the answer, because every containment predicate downstream
    /// reads this. When expiry erased the answer, a permanently-broken host
    /// made `sandbox status`, the `enforces_read_deny` / `owns_descendants_hard`
    /// / `binds_cwd_authority` claims and swarm admission alternate between two
    /// answers on a machine that never changed: unavailable inside the TTL,
    /// "no verdict" (read as available) once it lapsed, unavailable again after
    /// the next probe. A status surface that flip-flops on an unchanged host is
    /// a defect, so the answer here is monotone — it only ever moves from
    /// unknown to a verdict, and from unavailable to available when a probe
    /// actually succeeds.
    fn settled(&self) -> Option<bool> {
        match self.verdict {
            ProbeVerdict::Unknown => None,
            ProbeVerdict::Available => Some(true),
            ProbeVerdict::Unavailable { .. } => Some(false),
        }
    }

    /// Whether a fresh probe is warranted now: never probed, or the negative
    /// verdict's retry window has elapsed. A positive verdict is sticky, so it
    /// never re-probes.
    fn needs_probe(&self, now: Instant) -> bool {
        match self.verdict {
            ProbeVerdict::Unknown => true,
            ProbeVerdict::Available => false,
            ProbeVerdict::Unavailable { retry_after } => now >= retry_after,
        }
    }

    /// Record a fresh probe result. A success is sticky; a failure keeps
    /// answering `false` and schedules the next permitted probe `neg_ttl` out.
    ///
    /// A negative NEVER downgrades a sticky `Available`: the probe runs
    /// outside the cache lock, so a concurrent stalled probe can finish
    /// (and time out) after a successful one — the proven-working verdict
    /// must win regardless of record order.
    fn record(&mut self, available: bool, now: Instant, neg_ttl: Duration) {
        if !available && self.verdict == ProbeVerdict::Available {
            return;
        }
        self.verdict = if available {
            ProbeVerdict::Available
        } else {
            ProbeVerdict::Unavailable {
                retry_after: now + neg_ttl,
            }
        };
    }
}

/// Collapse concurrent cold availability probes into a single real probe
/// (FerroxLabs/wayland#754).
///
/// The probe cache alone does not stop a stampede: when the cache is cold, N
/// concurrent callers all miss it *before* any records a verdict, so each runs
/// its OWN probe. On Windows those parallel AppContainer spawns contend on the
/// shared per-PID profile and mostly fail, and every failure is cached as
/// `UnavailableUntil(now + neg_ttl)` — so `default_for_platform()` returns
/// `FailClosedBackend` and refuses EVERY command for the whole TTL window.
///
/// This helper gates the slow (probe) path so only the first cold arrival
/// spawns; the rest block on `gate` and then observe that arrival's verdict via
/// the double-checked cache read. Split out of `is_available` (which is
/// Windows-only) so the concurrency contract is unit-testable on every platform
/// with a counting closure standing in for the real Win32 probe. The gate is
/// held only across a cold probe, so warm calls stay lock-free on the fast path.
#[cfg(any(windows, test))]
fn probe_single_flight(
    cache: &std::sync::Mutex<ProbeCache>,
    gate: &std::sync::Mutex<()>,
    neg_ttl: Duration,
    probe: impl FnOnce() -> bool,
) -> bool {
    // Fast path: a settled verdict that is not due for a re-probe needs
    // neither a probe nor the gate.
    {
        let g = cache.lock().expect("probe cache poisoned");
        if !g.needs_probe(Instant::now())
            && let Some(settled) = g.settled()
        {
            return settled;
        }
    }
    // Slow path: serialize, then re-check the cache under the gate so only the
    // first arrival probes and the rest reuse its verdict.
    // The gate stays un-poisonable in practice: the real Win32 FFI runs on the
    // detached `appcontainer-probe` thread, so a panic there surfaces here as a
    // `RecvTimeoutError::Disconnected` → `false` return, never an unwind through
    // this caller — a caller-thread unwind cannot poison the gate and wedge it.
    let _gate = gate.lock().expect("probe gate poisoned");
    {
        let g = cache.lock().expect("probe cache poisoned");
        if !g.needs_probe(Instant::now())
            && let Some(settled) = g.settled()
        {
            return settled;
        }
    }
    let result = probe();
    let mut g = cache.lock().expect("probe cache poisoned");
    g.record(result, Instant::now(), neg_ttl);
    result
}

#[cfg(test)]
mod probe_cache_tests {
    use super::{Duration, Instant, ProbeCache};

    #[test]
    fn unknown_forces_a_probe() {
        let c = ProbeCache::new();
        assert_eq!(c.settled(), None);
        assert!(c.needs_probe(Instant::now()));
    }

    #[test]
    fn positive_is_sticky() {
        let mut c = ProbeCache::new();
        let t0 = Instant::now();
        c.record(true, t0, Duration::from_secs(30));
        assert_eq!(c.settled(), Some(true));
        // Still available far in the future — never re-probes.
        assert!(!c.needs_probe(t0 + Duration::from_secs(3600)));
        assert_eq!(c.settled(), Some(true));
    }

    /// Regression: a negative verdict must NOT oscillate across the TTL.
    ///
    /// `NEGATIVE_PROBE_TTL` schedules the next *probe attempt*. It used to also
    /// erase the *answer* — `cached()` returned `Some(false)` inside the window
    /// and `None` after it — so on a permanently-broken host every containment
    /// predicate that reads this cache alternated between two answers while the
    /// machine never changed. Two answers on an unchanged host is a defect
    /// regardless of which one is "safe": it makes `sandbox status` and swarm
    /// admission unusable as evidence.
    ///
    /// The two questions are now separate and only `needs_probe` is temporal.
    #[test]
    fn a_negative_verdict_never_oscillates_across_the_retry_window() {
        let mut c = ProbeCache::new();
        let t0 = Instant::now();
        let ttl = Duration::from_secs(30);
        c.record(false, t0, ttl);

        // The ANSWER is stable at every point on both sides of the window.
        for at in [
            Duration::ZERO,
            Duration::from_secs(10),
            ttl,
            Duration::from_secs(31),
            Duration::from_secs(3600),
        ] {
            assert!(
                !c.needs_probe(t0 + at) || c.settled() == Some(false),
                "an unavailable verdict must stay unavailable at +{at:?}"
            );
            assert_eq!(
                c.settled(),
                Some(false),
                "the settled answer must not change at +{at:?} on an unchanged host"
            );
        }

        // Only the re-probe schedule is temporal.
        assert!(!c.needs_probe(t0 + Duration::from_secs(10)));
        assert!(c.needs_probe(t0 + ttl));
        assert!(c.needs_probe(t0 + Duration::from_secs(31)));
    }

    /// The same property against the REAL clock, so it cannot be satisfied by
    /// a synthetic-time convention.
    ///
    /// A one-millisecond retry window is guaranteed to have lapsed after the
    /// sleep. Under the old semantics — where the window erased the answer —
    /// the verdict here reads "no verdict", which is what a containment
    /// predicate reads as "still fine". It must read `false`.
    #[test]
    fn a_negative_verdict_survives_a_lapsed_retry_window_on_the_real_clock() {
        let mut c = ProbeCache::new();
        c.record(false, Instant::now(), Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(25));
        assert_eq!(
            c.settled(),
            Some(false),
            "the answer must outlive its retry window — a lapsed window means \
             `probe again`, never `never mind`"
        );
        assert!(
            c.needs_probe(Instant::now()),
            "the lapsed window must still permit a fresh probe"
        );
    }

    #[test]
    fn late_negative_never_downgrades_a_sticky_positive() {
        // Two concurrent probes race the first fill: A succeeds and records
        // Available; B (stalled, timed out) records its failure LAST. The
        // proven-working verdict must survive.
        let mut c = ProbeCache::new();
        let t0 = Instant::now();
        c.record(true, t0, Duration::from_secs(30));
        c.record(false, t0 + Duration::from_secs(1), Duration::from_secs(30));
        assert_eq!(c.settled(), Some(true));
        assert!(!c.needs_probe(t0 + Duration::from_secs(3600)));
    }

    #[test]
    fn negative_then_positive_recovers_and_sticks() {
        let mut c = ProbeCache::new();
        let t0 = Instant::now();
        c.record(false, t0, Duration::from_secs(30));
        // A later successful probe upgrades to sticky-available — recovery is
        // the ONLY thing that may move the answer off `false`.
        c.record(true, t0 + Duration::from_secs(31), Duration::from_secs(30));
        assert_eq!(c.settled(), Some(true));
        assert!(!c.needs_probe(t0 + Duration::from_secs(3600)));
    }

    /// Regression for FerroxLabs/wayland#754: concurrent cold callers must NOT
    /// each run the real probe. Before the single-flight gate, N parallel
    /// `is_available()` callers all missed the cold cache and each launched its
    /// own AppContainer spawn; on Windows those contended and failed, poisoning
    /// the negative cache so every command was refused by `FailClosedBackend`.
    /// With the gate, the probe runs exactly once and every caller observes the
    /// same verdict. Without the gate this asserts >1 and fails.
    #[test]
    fn single_flight_runs_probe_once_under_concurrency() {
        use super::probe_single_flight;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier, Mutex};

        let cache = Arc::new(Mutex::new(ProbeCache::new()));
        let gate = Arc::new(Mutex::new(()));
        let probe_calls = Arc::new(AtomicUsize::new(0));
        let n = 32usize;
        let barrier = Arc::new(Barrier::new(n));

        let mut handles = Vec::with_capacity(n);
        for _ in 0..n {
            let cache = Arc::clone(&cache);
            let gate = Arc::clone(&gate);
            let probe_calls = Arc::clone(&probe_calls);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                // Release all threads at once to maximize the cold-cache race.
                barrier.wait();
                probe_single_flight(&cache, &gate, Duration::from_secs(30), || {
                    probe_calls.fetch_add(1, Ordering::SeqCst);
                    // Widen the window a real Win32 probe would occupy.
                    std::thread::sleep(Duration::from_millis(5));
                    true
                })
            }));
        }
        let all_available = handles.into_iter().all(|h| h.join().unwrap());
        assert!(
            all_available,
            "every caller must observe the Available verdict"
        );
        assert_eq!(
            probe_calls.load(Ordering::SeqCst),
            1,
            "the real probe must run exactly once despite {n} concurrent cold callers (#754)"
        );
    }

    /// Fail-closed counterpart to the test above (FerroxLabs/wayland#754): the
    /// single-flight fix must NOT weaken fail-closed under concurrency. When the
    /// (one) probe reports the sandbox UNAVAILABLE, N concurrent cold callers
    /// must still (a) run the probe exactly once — no stampede, (b) EVERY caller
    /// observe `false` (→ `FailClosedBackend`, never a silent unsandboxed
    /// fallback), and (c) leave the cache holding the negative `UnavailableUntil`
    /// verdict so later calls within the TTL reuse it instead of re-probing.
    #[test]
    fn single_flight_fails_closed_once_under_concurrency() {
        use super::probe_single_flight;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier, Mutex};

        let cache = Arc::new(Mutex::new(ProbeCache::new()));
        let gate = Arc::new(Mutex::new(()));
        let probe_calls = Arc::new(AtomicUsize::new(0));
        let n = 32usize;
        let ttl = Duration::from_secs(30);
        let barrier = Arc::new(Barrier::new(n));

        let mut handles = Vec::with_capacity(n);
        for _ in 0..n {
            let cache = Arc::clone(&cache);
            let gate = Arc::clone(&gate);
            let probe_calls = Arc::clone(&probe_calls);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                probe_single_flight(&cache, &gate, ttl, || {
                    probe_calls.fetch_add(1, Ordering::SeqCst);
                    // Widen the window a real Win32 probe would occupy.
                    std::thread::sleep(Duration::from_millis(5));
                    false // sandbox UNAVAILABLE — must fail closed
                })
            }));
        }
        // (b) every caller observes the fail-closed verdict (none saw Available).
        let any_available = handles.into_iter().any(|h| h.join().unwrap());
        assert!(
            !any_available,
            "every caller must observe the UNAVAILABLE verdict (fail closed)"
        );
        // (a) no stampede: the failing probe ran exactly once.
        assert_eq!(
            probe_calls.load(Ordering::SeqCst),
            1,
            "the failing probe must run exactly once despite {n} concurrent cold callers (#754)"
        );
        // (c) the cache holds the negative verdict, reused within the retry
        // window — an Unknown/uncached verdict would surface as `None`.
        assert_eq!(
            cache.lock().unwrap().settled(),
            Some(false),
            "a concurrent fail-closed probe must settle Unavailable, not leave the cache cold"
        );
        // …and a follow-up caller reuses that verdict WITHOUT re-probing: this
        // closure would flip the result to Available if it ran, so it must not.
        let reused = probe_single_flight(&cache, &gate, ttl, || {
            probe_calls.fetch_add(1, Ordering::SeqCst);
            true
        });
        assert!(
            !reused,
            "cached fail-closed verdict must be reused, not re-probed"
        );
        assert_eq!(
            probe_calls.load(Ordering::SeqCst),
            1,
            "cached UnavailableUntil must be reused within the TTL — no re-probe"
        );
    }
}

/// The AppContainer + Job Object hard-containment identity.
///
/// Centralized here so the mechanism wiring lives in-scope. The real Windows
/// backend (`windows_impl::process`) overrides
/// [`super::SandboxBackend::hard_containment_identity`] /
/// [`super::SandboxBackend::probe_hard_containment`] by delegating to this
/// helper and live-probing a benign AppContainer + Job Object spawn; that
/// live-spawn override is exercised by Windows CI. The non-Windows `stub_impl`
/// below deliberately does NOT override those methods, so on every non-Windows
/// target the stub keeps the trait default and is structurally non-qualifying.
///
/// This preserves the accepted 20-02 per-execution AppContainer profile / ACL /
/// recovery authority: it adds a mechanism identity only and touches none of
/// that lifecycle.
#[cfg(any(windows, test))]
// Consumed by the real Windows backend's (out-of-scope `windows_impl::process`)
// trait override and by the crate-private test below. `allow(dead_code)` guards
// the Windows lib build until that override lands (CI-verified on Windows).
#[allow(dead_code)]
pub(crate) fn windows_appcontainer_hard_containment_identity() -> super::HardContainmentIdentity {
    super::HardContainmentIdentity {
        mechanism: super::HardContainmentMechanism::WindowsAppContainerJobObject,
        executable_identity: "windows-appcontainer".to_owned(),
        runtime_identity: "windows-appcontainer+job-object".to_owned(),
        process_tree_mechanism: super::process_tree::ProcessTreeMechanism::WindowsJobObject,
    }
}

#[cfg(test)]
mod hard_containment_identity_tests {
    #[test]
    fn appcontainer_identity_names_job_object_mechanism() {
        let identity = super::windows_appcontainer_hard_containment_identity();
        assert_eq!(
            identity.mechanism,
            super::super::HardContainmentMechanism::WindowsAppContainerJobObject
        );
        assert_eq!(
            identity.process_tree_mechanism,
            super::super::process_tree::ProcessTreeMechanism::WindowsJobObject
        );
    }
}

// Compiled on every platform for the same reason `windows_cmdline` is: the
// ACL mutation lock's timeout, retry and contention message are pure logic, and
// they are the difference between a mystery hang and a failure an operator can
// act on. Only the `cfg(windows)` `acl_lease` module below calls it.
#[path = "appcontainer/acl_lock_policy.rs"]
pub(crate) mod acl_lock_policy;

#[cfg(windows)]
#[path = "appcontainer/acl_lease.rs"]
mod appcontainer_acl_lease;

#[cfg(windows)]
mod windows_impl {
    //! Windows AppContainer + Job Objects backend, split across `command`,
    //! `handles`, `process`, `window_station` and `tests` so every source file
    //! stays under the 1000-line limit (F20-03 Task 1A). Cross-module items
    //! are `pub(super)`; the public surface (`AppContainerBackend`) is
    //! unchanged.
    mod command;
    mod handles;
    mod process;
    mod shared_verdict;
    #[cfg(test)]
    mod tests;
    mod window_station;

    pub use process::AppContainerBackend;
}

#[cfg(not(windows))]
mod stub_impl {
    //! Non-Windows compile-stub. NOT a deferral — the real backend lives in
    //! the `#[cfg(windows)]` module above. This stub exists so the crate
    //! compiles + unit-tests on macOS/Linux dev machines, mirroring the
    //! pattern bwrap/sandbox-exec use for their own foreign platforms.

    use super::super::SandboxBackend;
    use crate::error::{Result, SandboxError};
    use crate::manifest::SandboxManifest;
    use crate::{SandboxCommand, SandboxOutput};
    use async_trait::async_trait;

    pub struct AppContainerBackend;

    impl AppContainerBackend {
        pub fn new() -> Self {
            Self
        }
    }

    impl Default for AppContainerBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    // NOTE: this stub deliberately does NOT override `hard_containment_identity`
    // or `probe_hard_containment`, so it keeps the trait defaults (`None` /
    // `PolicyNotSupported`) and is structurally incapable of minting hard
    // containment on any non-Windows target.
    #[async_trait]
    impl SandboxBackend for AppContainerBackend {
        fn name(&self) -> &'static str {
            "appcontainer_stub"
        }
        fn is_available(&self) -> bool {
            false
        }
        async fn execute(
            &self,
            _manifest: &SandboxManifest,
            _cmd: SandboxCommand,
        ) -> Result<SandboxOutput> {
            Err(SandboxError::ExecFailed(
                "AppContainer backend is Windows-only; this host runs the cfg(not(windows)) stub"
                    .into(),
            ))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn stub_is_unavailable() {
            let b = AppContainerBackend::new();
            assert!(!b.is_available());
            let res = b
                .execute(
                    &SandboxManifest::default(),
                    SandboxCommand {
                        argv: vec!["foo".into()],
                        cwd: None,
                    },
                )
                .await;
            assert!(res.is_err());
        }

        #[tokio::test]
        async fn stub_name() {
            let b = AppContainerBackend::new();
            assert_eq!(b.name(), "appcontainer_stub");
        }
    }
}

#[cfg(not(windows))]
pub use stub_impl::AppContainerBackend;
#[cfg(windows)]
pub use windows_impl::AppContainerBackend;
