# wcore-swarm::dispatch_smoke::dispatches_4_noop_workers_in_parallel + wcore-swarm::worker_runtime_limits::multi_worker_output_exhaustion_fails_without_retaining_buffers (shared error: "worker descendant still holds the retained checkout descriptor")

**Confidence (self-reported):** probable

## Root cause

On Windows that message is not a loan measurement at all — it is the fail-closed arm of a probe that asks a different, *destructive* question. `checkout_loan_outstanding` (crates/wcore-swarm/src/dispatch.rs:552) rebuilds a `RetainedWorkspaceAuthority` and returns `true` whenever `RetainedWorkspaceAuthority::new` returns Err ("a refused new proves nothing"). No Windows code path ever takes a `DirectoryHandleLoan` against the checkout authority — `try_clone_inheritable_handle` is `#[cfg(target_os = "linux")]` (directory_authority.rs:242) and the crate's only other loan sites are the swarm control dir and the transaction ROOT's lease file (worktree.rs:394, :405, :521) — so on Windows the Err arm is the *only* way the predicate can return true. Inside `new`, exactly one step is share-arbitrated: `owner.open_child_directory(&child_name)` (directory_authority.rs:919). That call is `openat(O_RDONLY|O_DIRECTORY|O_NOFOLLOW)` on unix but on Windows requests `FILE_GENERIC_READ|FILE_GENERIC_WRITE|DELETE|SYNCHRONIZE` = 0xC0110000 (directory_authority_windows.rs:261 + the RelativeIntent::Mutate arm at :890-893). Windows refuses a DELETE-bearing open while any existing handle on the object omits FILE_SHARE_DELETE — which is precisely what a process holding that directory as its CURRENT DIRECTORY holds, and the worker's cwd IS the checkout (dispatch.rs:299). I measured this on real Windows (10.0.26200, NTFS): with a live process whose cwd is the directory the read-only open succeeds and the 0xC0110000 open fails with win32=32; once that process is waited for, the same open succeeds within 0-2 ms; and with a surviving GRANDCHILD it stays refused indefinitely. The reason a holder can still be alive at release is the second half: the AppContainer reap calls `TerminateJobObject` (asynchronous for the whole job) and then waits only on the DIRECT child handle (process.rs:1124-1132), so any other job member (grandchild, console host — the code itself notes "a lingering conhost") can still be running when the swarm's release path runs. Net effect: a worker that SUCCEEDED is reported Failed, its terminal status is overwritten, its workspace is retained and its 8 GiB capacity reservation is held. That is a genuine Windows product defect, not merely a racing test; the test-visible race is only what makes it intermittent.

## Evidence

- crates/wcore-swarm/src/dispatch.rs:552-564 — `fn checkout_loan_outstanding(workspace, worker_id) -> bool { ... let Ok(retained) = RetainedWorkspaceAuthority::new(root_authority, workspace.checkout_authority(), ...) else { return true; }; retained.checkout_has_outstanding_loans() }` — a refused `new` is reported as an outstanding loan.
- crates/wcore-swarm/src/dispatch.rs:590-601 — `if checkout_loan_outstanding(...) { let diagnostic = "worker descendant still holds the retained checkout descriptor; transaction quarantined and its reservation held for retry"; terminal.status = WorkerStatus::Failed(diagnostic.clone()); ... return ... }` — the verdict also OVERWRITES the real terminal status, which is why the flood test saw this string instead of "output limit exceeded".
- crates/wcore-sandbox/src/directory_authority.rs:919 (`new`) and :959 (`validate`) — `let observed = owner.open_child_directory(&child_name)?;` used only to compare `identity_token()`; the identity proof is the sole consumer.
- crates/wcore-sandbox/src/directory_authority.rs:279 (unix) — `libc::openat(..., O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)` vs crates/wcore-sandbox/src/directory_authority_windows.rs:257-262 + :890-893 — `(RelativeKind::Directory, RelativeIntent::Create | RelativeIntent::Mutate) => FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE | SYNCHRONIZE`. Same function, read-only on POSIX, delete-bearing on Windows.
- crates/wcore-sandbox/src/directory_authority.rs:242 — `#[cfg(target_os = "linux")] pub(crate) fn try_clone_inheritable_handle`; grep over crates/ shows the only other loan producers are worktree.rs:394 (swarm CONTROL dir), :405 and :521 (transaction ROOT lease file). Nothing loans the checkout on Windows, so the counter the message names is always 0 there.
- MEASURED, SeanDesktop, Windows 10.0.26200 NTFS (scratchpad/shareprobe.ps1): baseline read-only OK, MUTATE(0xC0110000) OK; with a live child whose cwd is the dir -> read-only OK, MUTATE FAILED win32=32; with a surviving grandchild -> MUTATE still FAILED win32=32 at +50/100/150/200/250 ms.
- MEASURED (scratchpad/shareprobe2.ps1, 6 trials, `cmd /c rem` = the exact noop worker argv from dispatch_smoke.rs:895): after WaitForExit, MUTATE first succeeded at 0-1 ms. (scratchpad/shareprobe3.ps1, 5 trials, forced kill): while alive win32=32, after wait 0-2 ms. So a refusal at release time means a process is STILL ALIVE.
- crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs:1124-1132 — `TerminateJobObject(job.as_raw(), ...); WaitForSingleObject(process.as_raw(), 2_000);` — terminates the whole job but waits only on the direct child; the comment at :1113-1123 acknowledges conhost/helpers as job members.
- crates/wcore-swarm/src/worktree.rs:225-234 — `TransactionCleanup::release` already applies the correct loan test with no re-open: `self.checkout_authority.get().is_some_and(|checkout| checkout.has_outstanding_loans())`, and on any later failure re-inserts the reservation (worktree.rs:322-336), so the dispatch-side pre-check is not what enforces "reservation held for retry".
- .planning/evidence/win-hidden-fails/LEDGER.txt:88-99 — prior Windows evidence recorded the same string and blamed "the Windows kernel has not finished closing the reaped job's inherited handles, so the loan counter is still non-zero". That explanation is refuted: the loan counter is a process-local AtomicUsize in OUR process that no child can touch, and no Windows site increments it for the checkout.
- .planning/evidence/win-hidden-fails/LEDGER.txt:80-86 — the sibling nondeterministic Windows failure `workspace accounting refused ".git": ... (os error 32)` is the SAME defect one layer over: the accounting walk also opens children with Mutate intent (crates/wcore-swarm/src/worktree.rs:618, :623).

## How to verify

The patch is at /private/tmp/claude-501/-Users-seandonahoe-dev-waylandcore/11929102-d58a-47e9-9644-0e9d530b58c4/scratchpad/cluster1.patch and `git apply --check` passes cleanly against integration head 7accc0c1 (all 5 files). NOTHING WAS COMPILED — I ran no cargo anywhere, per the constraints; the Win32 drain block and the new Windows test are unbuilt.

1) No-regression (Linux, hetzner): `cargo nextest run -p wcore-swarm -p wcore-sandbox`. Only dispatch.rs behaviour changes on unix (the removed re-open was already read-only there), so this must be unchanged. Note `worktree_tests.rs:439` (`release_refuses_while_checkout_loan_outstanding`) still exercises the real loan path via `try_clone_handle` and must stay green.
2) Windows, the actual gate (SeanDesktop, D: drive): `cargo nextest run -p wcore-swarm --test dispatch_smoke --test worker_runtime_limits` and `cargo test -p wcore-sandbox directory_authority::tests::windows_retained_workspace_binds_under_a_share_delete_denying_handle`.
3) The distinguishing observable: the string "worker descendant still holds the retained checkout descriptor" must not appear at all. If a failure remains, it will now read "transaction cleanup: ... (os error 32)" — that is the HONEST diagnosis (a real live holder at deletion time) and it points at the job drain, not at the loan probe. Treat that as a different, still-open finding, not as this fix failing.
4) Kernel-behaviour reproduction (no build needed, ~30 s): `scp scratchpad/shareprobe.ps1 SeanD@seandesktop:D:/tmp/ && ssh SeanD@seandesktop "powershell -NoProfile -ExecutionPolicy Bypass -File D:\\tmp\\shareprobe.ps1"` — prints read-only OK / MUTATE FAILED win32=32 while a holder is live.

## Mutant

Two independent mutants, both Windows-only:
(a) In `RetainedWorkspaceAuthority::new`, change `owner.open_child_directory_observational(&child_name)` back to `owner.open_child_directory(&child_name)`. The new test `windows_retained_workspace_binds_under_a_share_delete_denying_handle` then FAILS: the name lease it holds omits FILE_SHARE_DELETE, so the delete-bearing open is refused by the kernel with os error 32 and `new` returns Err — the exact state the old dispatch probe converted into the quarantine verdict. I measured this refusal directly (shareprobe.ps1), so the gate is not hypothetical.
(b) Delete the "12a. Drain the job" block in process.rs and run the swarm suite on Windows under parallel load; a job member surviving `TerminateJobObject` re-opens the window in which a live cwd holder makes the release path fail.
The gate that would be worthless is a Linux-only one: on POSIX this whole class cannot fail, so the test is deliberately in the `cfg(all(test, windows))` module.

## Unknowns

- I did NOT observe the failing CI path itself. Identifying `open_child_directory` inside `RetainedWorkspaceAuthority::new` as the failing call is (i) elimination from source — no Windows site loans the checkout, so only the Err arm can return true — plus (ii) a measured kernel behaviour. The code swallows the error (`let Ok(...) else`), so no runner log exists to confirm it. The single decisive measurement that would settle it: temporarily log the Err from `new` in that probe and re-run the two tests on a Windows box.
- Whether a job member is actually still alive at release time on hosted windows-latest is UNMEASURED. I proved a live holder produces exactly this refusal and that a waited-for holder releases in 0-2 ms; I could not prove which holder the runner had. The job drain is therefore justified as completing commit 2b662fe8's stated intent, not as an observed fix.
- Nothing was compiled or run in Rust. The Win32 drain (QueryInformationJobObject / JOBOBJECT_BASIC_ACCOUNTING_INFORMATION / JobObjectBasicAccountingInformation, windows-sys 0.59, Win32_System_JobObjects already enabled) and the new Windows test are UNCOMPILED. If the orchestrator wants the lowest-risk subset, the dispatch.rs hunk alone is the misdiagnosis fix and is trivially compilable.
- SECOND DEFECT, deliberately NOT fixed here: `release_terminal` (dispatch.rs:577 and :594) unconditionally OVERWRITES `terminal.status`, so the quarantine (and the malformed-heartbeat) diagnostic destroys the worker's real terminal reason. That is why worker_runtime_limits saw the quarantine string instead of "output limit exceeded". Fixing it changes emitted status strings that other tests may assert exactly, so it needs its own change with a Windows+Linux run.
- THIRD DEFECT, same root cause, out of my two-test scope: the capacity accounting walk `logical_tree_bytes` (worktree.rs:618 `open_child_directory`, :623 `open_child_file`) also takes delete-bearing opens during a pure read, which is the recorded cause of the nondeterministic `workspace accounting refused ".git": ... os error 32` refusal in .planning/evidence/win-hidden-fails/LEDGER.txt:80-86. One-line-each fix using the same observational opens (a `open_child_file_observational` sibling would be needed for the file arm).
- After this patch `RetainedWorkspaceAuthority::checkout_has_outstanding_loans` (directory_authority.rs:949) has no in-crate caller. It is `pub`, so no dead-code warning, but it is now an orphan; I did not delete it (drive-by).
- The dispatch-side pre-check no longer re-proves owner->child identity before release. That proof is not lost in substance: deletion is handle-relative from the retained root (`root_authority.remove_open_dir_all()`), `remove_transaction_root` re-validates the swarm root and the owner-path relationship, and `release()` re-reads the root authority and reservation. But it IS one fewer redundant proof, and a reviewer should agree with that trade explicitly.
- Unverified assumption in the drain: that a console host created for `cmd.exe` under the AppContainer low-IL token is a member of the job. If it is not, TerminateJobObject never kills it and the drain will not see it. Breakaway is denied for normal children (process.rs:638-645), so ordinary descendants are covered.

## Proposed patch (NOT APPLIED, NOT COMPILED)

```diff
--- a/crates/wcore-swarm/src/dispatch.rs
+++ b/crates/wcore-swarm/src/dispatch.rs
@@ -539,28 +539,46 @@
 }
 
 /// Whether a worker descendant still holds a duplicate of the retained checkout
-/// directory descriptor (inherited across the sandbox spawn boundary). Rebuilds
-/// the owner-bound retained authority to query the shared loan counter.
+/// directory descriptor (inherited across the sandbox spawn boundary).
 ///
-/// The two failure arms are deliberately asymmetric. A root-authority failure is
-/// a PROVEN identity failure, so it returns `false` and the caller falls through
-/// to `release_transaction`, which independently re-validates and fails closed on
-/// the same condition — flipping it would strand every already-drifted
-/// transaction. A refused `RetainedWorkspaceAuthority::new` proves nothing: the
-/// absence of a loan could not be established, so it returns `true` and the
-/// caller quarantines the transaction with its reservation held for retry.
-fn checkout_loan_outstanding(workspace: &TransactionWorkspace, worker_id: &str) -> bool {
-    let Ok(root_authority) = workspace.root_authority() else {
-        return false;
-    };
-    let Ok(retained) = RetainedWorkspaceAuthority::new(
-        root_authority,
-        workspace.checkout_authority(),
-        format!("{worker_id}:{}", workspace.reserved_bytes),
-    ) else {
-        return true;
-    };
-    retained.checkout_has_outstanding_loans()
+/// This reads the retained checkout authority's own shared loan counter and
+/// NOTHING else. That counter is this crate's single definition of an
+/// outstanding descendant loan: every loan site increments the counter of the
+/// authority the loan is taken against, `TransactionCleanup::release` consults
+/// exactly the same counter through `has_outstanding_loans`, and the counter is
+/// shared by every clone of an authority, so reading it through a clone reads
+/// the original.
+///
+/// IT NO LONGER REBUILDS A `RetainedWorkspaceAuthority` TO ASK, and that is the
+/// repair rather than a simplification. The rebuild answered a DIFFERENT
+/// question — "can the owner still OPEN its named child?" — and treated an
+/// unprovable answer as a loan. On Windows that question is DESTRUCTIVE:
+/// `DirectoryAuthority::open_child_directory` requested `FILE_GENERIC_READ |
+/// FILE_GENERIC_WRITE | DELETE | SYNCHRONIZE` where the unix form is
+/// `openat(O_RDONLY | O_DIRECTORY | O_NOFOLLOW)`, and Windows share arbitration
+/// refuses a delete-bearing open while ANY handle already on the object omits
+/// `FILE_SHARE_DELETE` — which is exactly what a process holding that directory
+/// as its CURRENT DIRECTORY holds. MEASURED on Windows 10.0.26200 (NTFS): with
+/// a live process whose cwd is the directory, the read-only open succeeds and
+/// the delete-bearing open fails with ERROR_SHARING_VIOLATION (win32 = 32);
+/// after that process has been waited for, the delete-bearing open succeeds
+/// again within 0-2 ms.
+///
+/// Since NO Windows path ever takes a loan against the checkout authority
+/// (`try_clone_inheritable_handle` is `#[cfg(target_os = "linux")]`, and the
+/// crate's other loan sites are the swarm control directory and the transaction
+/// root's lease file), that refusal was the ONLY way this predicate could
+/// return `true` on Windows: a verdict about share arbitration, reported as a
+/// verdict about loans, over a worker that had already succeeded — and it held
+/// the worker's capacity reservation while doing so.
+///
+/// NOTHING IS LOST BY NOT FAILING CLOSED HERE. `release_transaction` re-proves
+/// every authority itself and, on ANY failure, restores the transaction root
+/// AND re-inserts the capacity reservation before returning the error, so
+/// "reservation held for retry" is enforced by `TransactionCleanup::release` —
+/// which applies this same loan test as its own first guard.
+fn checkout_loan_outstanding(workspace: &TransactionWorkspace) -> bool {
+    workspace.checkout_authority().has_outstanding_handle_loans()
 }
 
 fn release_terminal(
@@ -587,7 +605,7 @@
     // descriptor. Quarantine the transaction (keep it reserved) instead of
     // releasing, so a live child cannot lose its working directory and a
     // same-path replacement cannot be substituted before the loan drops.
-    if checkout_loan_outstanding(&workspace, &terminal.worker_id) {
+    if checkout_loan_outstanding(&workspace) {
         let diagnostic = "worker descendant still holds the retained checkout descriptor; \
              transaction quarantined and its reservation held for retry"
             .to_owned();
--- a/crates/wcore-sandbox/src/directory_authority.rs
+++ b/crates/wcore-sandbox/src/directory_authority.rs
@@ -302,6 +302,41 @@
         ))
     }
 
+    /// Identity-witness open of ONE direct child directory: it proves the child
+    /// is the object this retained parent currently names, and asks the kernel
+    /// for nothing else.
+    ///
+    /// On unix this IS [`Self::open_child_directory`] — that open is already
+    /// `O_RDONLY | O_DIRECTORY | O_NOFOLLOW`. On Windows it is strictly weaker:
+    /// `FILE_GENERIC_READ | SYNCHRONIZE` in place of `FILE_GENERIC_READ |
+    /// FILE_GENERIC_WRITE | DELETE | SYNCHRONIZE`.
+    ///
+    /// THE DELETE RIGHT IS WHY THIS EXISTS. Windows share arbitration refuses a
+    /// delete-bearing open while any handle already on the object omits
+    /// `FILE_SHARE_DELETE` — which is exactly what a process holding that
+    /// directory as its CURRENT DIRECTORY holds. Measured on Windows 10.0.26200
+    /// (NTFS): with such a process live, the read-only open succeeds and the
+    /// delete-bearing one fails with ERROR_SHARING_VIOLATION (win32 = 32); once
+    /// that process has been waited for, the delete-bearing open succeeds again
+    /// within 0-2 ms. An IDENTITY PROOF must not fail merely because somebody is
+    /// standing in the directory, so proofs use this and only genuinely
+    /// destructive callers use the mutating form.
+    ///
+    /// The result is an IDENTITY WITNESS ONLY, with the same contract as
+    /// [`Self::open_observational`]: destructive and relative-child operations
+    /// on it are outside its contract and fail closed with an OS access error.
+    pub fn open_child_directory_observational(&self, name: &str) -> Result<Self> {
+        validate_child_name(name)?;
+        #[cfg(windows)]
+        {
+            windows::open_child_directory_observational(self, name)
+        }
+        #[cfg(not(windows))]
+        {
+            self.open_child_directory(name)
+        }
+    }
+
     /// Enumerate direct child names beneath the retained directory. Names are
     /// observations only; callers must open each child through this authority
     /// before trusting its type, metadata, or contents.
@@ -916,7 +951,7 @@
         }
         owner.validate_path(owner.display_path())?;
         workspace.validate_path(display)?;
-        let observed = owner.open_child_directory(&child_name)?;
+        let observed = owner.open_child_directory_observational(&child_name)?;
         if observed.identity_token() != workspace.identity_token() {
             return Err(SandboxError::PathDenied(
                 "retained workspace child identity contradicts owner authority".to_owned(),
@@ -956,7 +991,9 @@
         self.owner.validate_path(self.owner.display_path())?;
         self.workspace
             .validate_path(self.workspace.display_path())?;
-        let observed = self.owner.open_child_directory(&self.child_name)?;
+        let observed = self
+            .owner
+            .open_child_directory_observational(&self.child_name)?;
         if observed.identity_token() != self.workspace.identity_token() {
             return Err(SandboxError::PathDenied(
                 "retained workspace identity changed beneath its owner".to_owned(),
--- a/crates/wcore-sandbox/src/directory_authority_windows.rs
+++ b/crates/wcore-sandbox/src/directory_authority_windows.rs
@@ -266,6 +266,31 @@
     Ok(directory_authority(parent, name, handle, identity))
 }
 
+/// Read-only sibling of [`open_child_directory`], for IDENTITY PROOFS.
+///
+/// The ONLY difference is the desired access: `FILE_GENERIC_READ | SYNCHRONIZE`
+/// instead of the mutating mask's added `FILE_GENERIC_WRITE | DELETE`. The
+/// DELETE right is share-arbitrated, so the mutating form is refused with
+/// ERROR_SHARING_VIOLATION while any handle on the child omits
+/// `FILE_SHARE_DELETE` — a live process whose current directory is that child
+/// being the everyday case, not an edge case. Identity is FileId/volume based
+/// and needs only read access, so a proof should pay no share-arbitration cost.
+pub(super) fn open_child_directory_observational(
+    parent: &DirectoryAuthority,
+    name: &str,
+) -> Result<DirectoryAuthority> {
+    let handle = open_relative(
+        parent,
+        name,
+        RelativeKind::Directory,
+        RelativeIntent::ReadOnly,
+    )?;
+    let metadata = handle.metadata()?;
+    validate_real_directory(Path::new("<retained child>"), &metadata)?;
+    let identity = handle_directory_identity(&handle, &metadata)?;
+    Ok(directory_authority(parent, name, handle, identity))
+}
+
 /// Name-only projection of `child_entries`.
 ///
 /// Kept as a projection rather than a second `NtQueryDirectoryFile` loop so the
--- a/crates/wcore-sandbox/src/directory_authority_windows_tests.rs
+++ b/crates/wcore-sandbox/src/directory_authority_windows_tests.rs
@@ -631,4 +631,36 @@
     std::fs::create_dir(&original).unwrap();
     assert!(moved.is_dir());
     assert!(original.is_dir());
+}
+
+/// A retained workspace must still BIND while a share-delete-denying handle is
+/// open on the checkout, because that is exactly what a live worker descendant
+/// holds: Windows opens a process's current directory without
+/// `FILE_SHARE_DELETE`.
+///
+/// HOW THIS FAILS IF THE DEFECT RETURNS. Point either identity proof in
+/// `RetainedWorkspaceAuthority` back at the mutating `open_child_directory` and
+/// the proof's open is refused by the kernel with ERROR_SHARING_VIOLATION (os
+/// error 32) while the lease below is held, so `new` returns Err. That Err is
+/// the state the swarm dispatch path reported as "worker descendant still holds
+/// the retained checkout descriptor", quarantining a successful worker and
+/// holding its capacity reservation.
+#[test]
+fn windows_retained_workspace_binds_under_a_share_delete_denying_handle() {
+    let temp = tempfile::tempdir().unwrap();
+    let owner_path = temp.path().join("transaction");
+    let checkout_path = owner_path.join("checkout");
+    std::fs::create_dir_all(&checkout_path).unwrap();
+
+    let owner = DirectoryAuthority::open(&owner_path).unwrap();
+    // Observational, exactly as the delegated dispatch path retains a checkout:
+    // a delete-bearing authority cannot be name-leased at all.
+    let checkout = DirectoryAuthority::open_observational(&checkout_path).unwrap();
+
+    // `acquire_name_lease` omits FILE_SHARE_DELETE — the same share arbitration
+    // a process standing in the directory imposes.
+    let pin = acquire_name_lease(&checkout).unwrap();
+    RetainedWorkspaceAuthority::new(owner, checkout, "share-arbitration-probe")
+        .expect("identity re-proof must not demand delete access on the checkout");
+    drop(pin);
 }
--- a/crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs
+++ b/crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs
@@ -42,9 +42,10 @@
     JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
     JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
     JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
-    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
-    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, SetInformationJobObject,
-    TerminateJobObject,
+    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_BASIC_UI_RESTRICTIONS,
+    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
+    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, QueryInformationJobObject,
+    SetInformationJobObject, TerminateJobObject,
 };
 use windows_sys::Win32::System::Pipes::CreatePipe;
 use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
@@ -62,6 +63,11 @@
 /// (versions ≤ 0.59); defined locally per the Windows SDK header.
 const SE_GROUP_INTEGRITY: u32 = 0x0000_0020;
 
+/// How long the post-`TerminateJobObject` drain waits for the job to reach zero
+/// active processes. Measured teardown is 0-2 ms; this is a hang guard, not a
+/// budget, and it is bounded so a wedged member cannot stall the caller.
+const JOB_DRAIN_LIMIT_SECS: u64 = 5;
+
 pub struct AppContainerBackend;
 
 impl AppContainerBackend {
@@ -1131,6 +1137,57 @@
             );
             WaitForSingleObject(process.as_raw(), 2_000);
 
+            // ---- 12a. Drain the job to ZERO active processes. ----
+            //
+            // `TerminateJobObject` only REQUESTS termination of every member; it
+            // does not wait, and the wait above covers the DIRECT CHILD ONLY. Any
+            // other member — a grandchild, a console host — can still be alive
+            // when this returns, and on Windows that is not cosmetic: a live
+            // process whose current directory is the worker checkout holds a
+            // handle whose share mode omits `FILE_SHARE_DELETE`, so every
+            // delete-bearing open of that directory is refused by the kernel with
+            // ERROR_SHARING_VIOLATION for as long as it lives. The swarm's
+            // terminal-release path then reports the refusal as "worker
+            // descendant still holds the retained checkout descriptor" and
+            // quarantines a transaction whose worker actually succeeded.
+            //
+            // MEASURED (Windows 10.0.26200, NTFS): with a live process whose cwd
+            // is the directory, the delete-bearing open fails with win32 = 32 for
+            // as long as it lives; once it has been waited for, the same open
+            // succeeds within 0-2 ms. So the only thing needed here is to WAIT.
+            // Breakaway is denied on this job (see the LimitFlags block above),
+            // so job membership is the complete descendant set.
+            //
+            // Bounded, so a member wedged in the kernel cannot hang the caller.
+            {
+                let drain_deadline =
+                    std::time::Instant::now() + Duration::from_secs(JOB_DRAIN_LIMIT_SECS);
+                loop {
+                    let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = mem::zeroed();
+                    let queried = QueryInformationJobObject(
+                        job.as_raw(),
+                        JobObjectBasicAccountingInformation,
+                        ptr::addr_of_mut!(accounting).cast(),
+                        mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
+                        ptr::null_mut(),
+                    );
+                    if queried == 0 || accounting.ActiveProcesses == 0 {
+                        break;
+                    }
+                    if std::time::Instant::now() >= drain_deadline {
+                        tracing::warn!(
+                            target: "wcore_sandbox",
+                            active_processes = accounting.ActiveProcesses,
+                            drain_limit_secs = JOB_DRAIN_LIMIT_SECS,
+                            "terminated job still has active members after its drain bound; \
+                             a descendant may still hold the workspace directory"
+                        );
+                        break;
+                    }
+                    std::thread::sleep(Duration::from_millis(2));
+                }
+            }
+
             // Now that every write-end is closed the reader threads reach EOF;
             // join them to collect the fully-drained output. This MUST run before
             // the deferred error returns so the threads never outlive their
```
