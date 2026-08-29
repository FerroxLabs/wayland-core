---
issue: 1202
repo: FerroxLabs/wayland
kind: defect
title: "atomic_write_checked treats a restore that exchanged nothing as a successful rollback, then deletes the user's only copy"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "restore() distinguishes an actual exchange from Swap::Vacant / Swap::Unsupported, and a non-exchange is a restore FAILURE"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D18, found while verifying wayland#1155). Nothing has been done. The measured finding, verbatim: `restore()` treats a non-exchange as a successful rollback, silently converting a refusal into published data loss. In `atomic_write_checked`, when the verdict rejects the pre-image the code calls `restore(&displaced, dest)`; on Linux/macOS that is `publish_displacing(displaced, dest).map(|_| ())`. The `.map(|_| ())` discards the `Swap` discriminant, so `Swap::Vacant` (ENOENT) and `Swap::Unsupported` (EINVAL/ENOSYS/EOPNOTSUPP) both return `Ok(())` having exchanged NOTHING. Control then falls to `discard_displaced(tmp, &displaced)`, which unlinks the pre-image — the only surviving copy of the user's bytes — and returns `Ok(Err(why))`, whose documented contract is 'the destination is exactly as it was'. The caller (edit.rs:536, write.rs:454) renders `changed_under_write(...)` and tells the user the write was refused, while the destination in fact holds the new content and the displaced original has been deleted. That is silent data loss plus a false refusal, inside the guard whose whole purpose is to prevent it — and it is fail-open in exactly the way this module argues against elsewhere (vfs.rs:492: 'Degrading on ANY observation failure would be fail-open on a data-loss guard')."
  - id: c2
    text: "On a restore failure the displaced pre-image is preserved by the existing keep_displaced path, and the caller does NOT report the destination as unchanged"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D18). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "A test drives a refused publish where the destination name disappears between the two exchanges and asserts the original bytes survive; shown RED against today's `.map(|_| ())`"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D18). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c4
    text: "The existing happy-path, successful-rollback, absent-destination and mode/long-path tests stay green"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D18). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

`restore()` treats a non-exchange as a successful rollback, silently converting a refusal into published data loss. In `atomic_write_checked`, when the verdict rejects the pre-image the code calls `restore(&displaced, dest)`; on Linux/macOS that is `publish_displacing(displaced, dest).map(|_| ())`. The `.map(|_| ())` discards the `Swap` discriminant, so `Swap::Vacant` (ENOENT) and `Swap::Unsupported` (EINVAL/ENOSYS/EOPNOTSUPP) both return `Ok(())` having exchanged NOTHING. Control then falls to `discard_displaced(tmp, &displaced)`, which unlinks the pre-image — the only surviving copy of the user's bytes — and returns `Ok(Err(why))`, whose documented contract is 'the destination is exactly as it was'. The caller (edit.rs:536, write.rs:454) renders `changed_under_write(...)` and tells the user the write was refused, while the destination in fact holds the new content and the displaced original has been deleted. That is silent data loss plus a false refusal, inside the guard whose whole purpose is to prevent it — and it is fail-open in exactly the way this module argues against elsewhere (vfs.rs:492: 'Degrading on ANY observation failure would be fail-open on a data-loss guard').

**Where.** crates/wcore-config/src/atomic_io.rs:114 (call) and :188-191 (`#[cfg(any(target_os = 'linux', target_os = 'macos'))] fn restore` = `publish_displacing(displaced, dest).map(|_| ())`)

**Why it matters.** The trigger is narrow — the first exchange already proved the filesystem supports the primitive, so it needs the destination name to disappear between the two calls (an external `rm` + rewrite, a `git checkout`, an editor that unlinks before writing) — but that is precisely the non-cooperating-concurrent-writer scenario #1155 exists to survive, and the failure is the worst possible pairing: the bytes are gone AND the user is told nothing was changed. There is no test for it: the tests module in atomic_io.rs covers the happy path (`the_check_is_handed_the_bytes_the_publish_displaced`), the successful rollback (`a_refused_publish_puts_the_destination_back`, which also asserts inode identity), the absent destination, and the mode/long-path cases — nothing exercises a restore that fails to exchange. Fix is one line: match the `Swap` and treat `Vacant`/`Unsupported` as a restore failure so the existing `keep_displaced` preservation path runs.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
