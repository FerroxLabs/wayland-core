# 23A-C1 — live drive of the governance surface on the shipped binary

Not a test run. The real `wayland-core` binary, built from
`9ee433de` on `hetzner-dsm` (`/root/wayland-23a-c1-atomic/target/debug/wayland-core`),
driven from a shell against a sandboxed profile.

```
WAYLAND_HOME=/tmp/f23a-live
WAYLAND_SKILLS_GOVERNANCE_DIR=/tmp/f23a-live/gov
```

The subject is an auto-drafted skill written into `$WAYLAND_HOME/skills/auto/auto-livedemo/`
exactly as `SkillDrafter::draft` writes one: `SKILL.md`, `manifest.json` with
`auto_drafted: true` and `signature: live-sig-1`, plus a nested `refs/notes.md` so the restore
has to reproduce a subdirectory and not just a flat file list.

**Credential note (LANE-BRIEF §0 / §3b-ii).** No real credential was used. The engine refuses
to boot without an API key, so the drive passes the literal string
`placeholder-no-network-call-is-made`. That is not a secret and is printed here deliberately.
Slash commands short-circuit in `handle_slash_or_run` before `engine.run()`, so no provider is
ever contacted — visible in the transcript as `vision: no API key found … tool will be hidden`
and `transcription: no API key found`, i.e. the process genuinely had no usable provider and
the governance verbs worked anyway. `WAYLAND_HOME` also isolates the profile from
`/root/.wayland/.env`, so the `ANTHROPIC_API_KEY` that host injects was not in play.

## What the operator can now do

### 1. Observe — the draft is present and quarantined (clause a + b, pre-existing)

```
$ wayland-core "/skill list"
  - auto:auto-livedemo (hidden) [src=user]
Summary: 0 visible to the model, 1 hidden.
```

### 2. KNOWN-NEGATIVE, run first — governance names nothing before anything happens

```
$ wayland-core "/skill govern"
Skill governance (/tmp/f23a-live/gov)
Live revocations: none. No skill is currently suppressed.
History: empty. Governance has taken no action on this machine.
```

This is the control for step 4. Without it, "governance shows the revocation" would pass on a
renderer that prints unconditionally.

### 3. Revoke — clause (c), on the binary a user installs

```
$ wayland-core "/skill revoke auto:auto-livedemo"
Revoked 'auto:auto-livedemo'.
  revocation_id: 09109bd1-227c-4d64-a6ea-5a07bdd08a2e
  removed_from:  /tmp/f23a-live/skills/auto/auto-livedemo
  retained:      3 file(s), 309 byte(s) in /tmp/f23a-live/gov
  suppressed:    the drafter will not recreate this skill
Undo with `/skill rollback 09109bd1-227c-4d64-a6ea-5a07bdd08a2e`.

$ ls -A /tmp/f23a-live/skills/auto/
    [end of listing]            <- the drafter-written directory is gone

$ wayland-core "/skill list"
/skill list: no skills loaded in this session.
```

### 4. Observe the governance record — clause (b), the half that was PARTIAL

```
$ wayland-core "/skill govern"
Skill governance (/tmp/f23a-live/gov)
Live revocations (1):
  - auto-livedemo
      id:        09109bd1-227c-4d64-a6ea-5a07bdd08a2e
      revoked:   2026-07-29T13:14:12.771648374+00:00
      was at:    /tmp/f23a-live/skills/auto/auto-livedemo
      signature: live-sig-1
      retained:  3 file(s), 309 byte(s)
Undo any of these with `/skill rollback <id>`.
History (1 event(s), append-only, showing last 1):
  2026-07-29T13:14:12.771648374+00:00  revoked  auto-livedemo  (id 09109bd1-…)
```

*what* was revoked, *when*, *from where*, and *what is retained* — the four things the verdict
recorded as unreachable.

### 5. Roll back — clause (d)

```
$ wayland-core "/skill rollback 09109bd1-227c-4d64-a6ea-5a07bdd08a2e"
Rolled back '09109bd1-227c-4d64-a6ea-5a07bdd08a2e'.
  restored_to: /tmp/f23a-live/skills/auto/auto-livedemo
  suppression: cleared — the drafter may produce this skill again

$ find /tmp/f23a-live/skills/auto -type f -printf '%p  %s bytes\n' | sort
    …/auto-livedemo/manifest.json  145 bytes
    …/auto-livedemo/refs/notes.md   19 bytes
    …/auto-livedemo/SKILL.md       145 bytes
$ cat …/auto-livedemo/refs/notes.md
reference material
$ ls -Ad /tmp/f23a-live/skills/auto/.wl-rollback-*
    (none)                       <- no staging directory left behind
```

145 + 145 + 19 = **309 bytes**, matching the `retained:` figure the revoke printed. The nested
`refs/` subdirectory came back with it.

### 6. The journal is append-only, and the skill is quarantined again

```
$ wayland-core "/skill govern"
Live revocations: none. No skill is currently suppressed.
History (2 event(s), append-only, showing last 2):
  2026-07-29T13:14:12.771648374+00:00  revoked      auto-livedemo  (id 09109bd1-…)
  2026-07-29T13:14:38.555349937+00:00  rolled-back  auto-livedemo  (id 09109bd1-…) -> /tmp/…/auto-livedemo

$ wayland-core "/skill list"
  - auto:auto-livedemo (hidden) [src=user]
Summary: 0 visible to the model, 1 hidden.
```

The rollback did **not** erase the revocation from history, and the restored draft is still
quarantined — the criterion's clause (a) is not weakened by having a rollback verb.

## What the live drive caught that the tests did not

**Catalog names are namespaced.** `/skill revoke auto-livedemo` — the name on disk, and the
name every unit test uses — returned:

```
/skill revoke 'auto-livedemo': no skill named 'auto-livedemo' in the catalog.
Run `/skill list` to see available skills.
```

The loader registers the skill as **`auto:auto-livedemo`** because it lives under
`skills/auto/`. The unit tests construct `SkillRef`s directly and so never see the namespace
prefix; only a real run through the real loader does. This is not a product defect — `/skill
list` prints the namespaced name, so an operator following the tool's own output types the
right thing, and the not-found message points them at `/skill list`. But it is exactly the
class of gap LANE-BRIEF §3.1 exists for: **the suite was green while the obvious command a
human would type did not work**, and nothing but launching the binary would have shown it.

## Reproduce

```bash
ssh hetzner-dsm
export PATH=/root/.cargo/bin:$PATH
cd /root/wayland-23a-c1-atomic && git checkout 9ee433de
cargo build -p wcore-cli
WH=/tmp/f23a-live; mkdir -p $WH/skills/auto/auto-livedemo/refs
# ... write SKILL.md + manifest.json + refs/notes.md as above ...
export WAYLAND_HOME=$WH WAYLAND_SKILLS_GOVERNANCE_DIR=$WH/gov
target/debug/wayland-core --api-key placeholder-no-network-call-is-made "/skill govern"
```

Two invocation traps, both hit during this drive and both recorded so the next reader does not:

- **`-p` is `--provider`, not "prompt".** The prompt is positional. `wayland-core -p "/skill
  list"` fails with `Unknown provider: '/skill list'`.
- **Slash output goes to stderr** (`OutputSink::emit_info`). `2>/dev/null` silently discards
  the entire result and leaves an empty transcript that looks like a failure to produce output.
