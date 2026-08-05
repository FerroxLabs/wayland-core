# Staged reply for core#254 — NOT POSTED

**Status: drafted, awaiting Sean.** Posting to an external contributor's PR, and closing it,
are outward-facing actions reserved to Sean. This is staged so the action is one paste, not a
writing session. Nothing here has been sent, and no GitHub action has been taken on #254.

Evidence: `.planning/intel/CORE-254-MAINTAINER-PACKAGE.md`. Fixes re-authored on
`lane/254-take` (`.planning/intel/CORE-254-TAKEN.md`).

**PRECONDITION CLEARED 2026-07-28.** `lane/254-take` landed and merged at `fcb52072`. Both
fixes were taken as described, so the reply below is accurate as written and is ready to post
unchanged.

What actually landed, in case you want to speak to it:

- **`%TEMP%` narrowing** — `scratch_dirs()` now takes a `WorkspaceTrust` and returns a bounded
  dir. Three things beyond a literal re-author: keyed **by trust** (one shared name would have
  created a trust-crossing writable dir *via the narrowing itself*), **fails closed** to an
  empty grant rather than back to `%TEMP%`, and on unix the uid goes in the top component with
  a symlink/ownership check, because `temp_dir()` there is world-writable `/tmp` and
  `create_dir_all` follows symlinks.
- **`\\?\` cwd strip** — `resolve_cwd()` extracted so the real `lpCurrentDirectory` buffer is
  assertable; strips on the wide encoding rather than a `to_str()` round trip, so non-UTF-8
  filenames stay byte-exact. **Live proof:** the un-fixed AppContainer child reported
  `C:\WINDOWS`; the fixed one reported the requested directory.

Both carry a red that was watched fail before the fix went in. `frankforges` is credited in
both commit bodies in prose.

---

## Suggested comment

> Thanks for this — and sorry for the slow reply. I've gone through it properly, including
> building and running it on a Windows box, and I want to be straight with you about where I
> landed: I'm taking part of this and declining part of it.
>
> **Taking, with credit to you:**
>
> - **The `%TEMP%` narrowing.** You're right, and it's still live on `main` — `scratch_dirs()`
>   is a bare `vec![canon(tmp)]` and hands out the whole temp directory.
> - **The `\\?\` cwd strip.** Also still live, and the cleanest change in the PR. We were
>   passing the path through unmodified.
>
> Both are re-authored on our side with regression tests, and the commit messages credit you as
> the finder. Genuinely useful — nobody upstream had caught either.
>
> **Declining, with reasons:**
>
> - **`SidsToDisable`.** We fixed this independently a few days before your PR, and landed
>   somewhere slightly different: we pass `0/null` and disable nothing, backed by a hardware
>   matrix run on 2026-07-23. Yours still disables `Administrators`. Superseded rather than
>   wrong. Worth flagging that your version isn't flag-gated, so it changes `Contained` sessions
>   too.
> - **The `$HOME` change.** We'd already replaced this with a curated capability allowlist
>   (`workspace_policy.rs`), and the Windows-only `Vec::new()` here would regress it.
> - **Relaxed Sandbox Mode.** This is the one I want to explain rather than just reject, because
>   the idea is reasonable and the implementation is where it comes apart.
>
>   The "trusted_local only" restriction exists only in the doc comments — six mentions, no code
>   path. And it isn't implementable as written: `SandboxManifest` carries no trust field and
>   `default_for_platform()` is trust-agnostic, so `Contained` sessions land on the same backend.
>   Two consequences follow. The `fs_read_deny` ACEs stay keyed to a package SID the child no
>   longer has, so secret-denial still runs, still logs "granted", and enforces nothing — a
>   sandbox reporting itself active while inactive is the exact failure mode our certification
>   work treats as critical. And both new config keys use plain `project.or(global)` while
>   `allow_no_sandbox`, `auto_approve` and `allow_list` are clamped a few lines away in the same
>   function — so a cloned repo's `.wayland-core.toml` could turn the Windows sandbox off. We
>   closed the remote-bypass door recently; this would open a config-shaped one beside it.
>
>   If you want to take another run at it, the shape that would work is a real trust field on the
>   manifest, the new keys clamped like their neighbours, and the ACEs re-keyed to whatever SID
>   the child actually carries. I'd review that.
>
> One more thing you might want, unrelated to the decision: your branch surfaced a pre-existing
> hermeticity bug of ours. `website_policy.rs` claims `#[serial_test::serial]` serializes every
> env-mutating test in that binary; it doesn't — the readers are plain `#[test]`. Three
> `wcore-tools` tests go red under a full run and pass in isolation, on `main` as well as on your
> branch. Not caused by you, and it's on our backlog.
>
> Thanks again for finding two real bugs and for proposing the split yourself — that made this
> much easier to act on.

---

## If Sean prefers to close rather than leave open

The two taken fixes land independently, so #254 does not need to merge. Closing with the comment
above is reasonable **once `lane/254-take` has landed** — closing before that would drop the
fixes on the floor. Leaving it open pending a revised Relaxed Mode is equally defensible and
costs nothing.
