# REMEDY GATE — making the advertised-but-dead class fail CI

**Lane** `lane/remedy-gate` · **base** `plan/f20-unified-audit-repair` @ `2a306ac8` (captured once)
**HEAD** `0ae1b126` · pushed to `gh`. Not merged, no PR.
Every build, test, clippy and live run on `hetzner-dsm` (`/root/wayland-remedy-gate`).
The Mac ran only `cargo fmt --all -- --check`.

**Goal:** a remediation string that names something the product cannot honour should fail CI,
not ship.

**Verdict: achieved for two token categories, with the boundary measured rather than asserted.
Five of seven historical cases go red; the two it misses are proved to be missed.**
It also found one previously-unrecorded instance of the class on its first green run.

---

## 1. The inventory — committed as data before any check was written

`.planning/REMEDY-INVENTORY.tsv` (1,505 rows), produced by
`.planning/scripts/sweep-remedy-strings.py`.

Swept **1,178 production `.rs` files** (`tests/`, `benches/`, `examples/` and every
`#[cfg(test)]` block excluded) containing **39,322 string literals**. Of those, 1,504 rows
are instructive operator-facing strings, broken down by what each advertises:

| advertised token | rows | gate-checked? |
|---|---|---|
| **prose — no extractable token** | **617** | **no — not mechanically checkable** |
| env var | 207 | no |
| config section | 144 | indirectly (via the assignment it heads) |
| config key (bare dotted) | 136 | no — see §5 |
| CLI flag | 127 | partially — only via the dead-end check |
| backticked code token | 118 | no |
| **config assignment (`section.key = value`)** | **107** | **yes — 29 reach the loader check** |
| subcommand | 45 | no |
| doc path | 3 | yes |

**617 of 1,504 rows — 41% — are instruction with no token in them at all.** "Configure an OS
keyring", "run it in a terminal", "re-run in an interactive session". These are not checkable by
any mechanism I can see, and they are the single largest category. Any coverage figure that
does not subtract them is dishonest.

### Two classifier defects found before the data was trusted

Both would have quietly changed the meaning of everything downstream:

1. **The single highest-value string was missing.** Case 1's TOML lives in a `const`, so a
   context whitelist of error/help/stdout dropped it — the sweeper would have been blind to the
   exact defect it was built for.
2. **Section↔key pairing was line-scoped**, which bound `backend` to `[session]` in the live
   headless-keyring string — inventing a key nobody advertised. Now positional, and the masking
   is length-preserving (deleting comment lines shifted offsets and re-pointed five
   `[tools]`/`[session]` keys at `[default]`).

---

## 2. What the gate checks

`crates/wcore-cli/tests/remedy_advertisements.rs` — 7 tests, all green, 60s.

| test | what it asserts |
|---|---|
| `advertised_config_assignments_survive_the_real_loader` | every advertised `key = value` **survives a round trip** through the real `ConfigFile` |
| `checker_reds_on_the_historical_defect_shapes` | the three historical shapes go red, driven through the same code path |
| `extractor_binds_sections_the_way_shipped_strings_phrase_them` | the four phrasings that actually ship, plus one regression negative |
| `no_unconditional_dead_end_is_reachable_from_an_advertised_flag` | no flag in the **real binary's** `--help` reaches an unconditional dead end |
| `advertised_tool_names_resolve_to_a_real_tool` | advertised tool names resolve against every `Tool::name` in the workspace |
| `tool_mention_extraction_reads_the_phrasing_that_shipped` | the tool extractor reads case 7's verbatim string and rejects the five that flooded it |
| `advertised_doc_paths_exist` | cited documents exist |

**The core mechanism is RETENTION, not parseability.** Case 1 *parsed cleanly* — `BrowserConfig`
is `#[serde(default)]` with no `deny_unknown_fields` — and was then dropped on the floor. A
parseability check passes on it, which is precisely why the existing miniature gate in
`recovery_confidential.rs` could not have caught it. Retention subsumes parseability and also
catches the silent drop, which is the nastier half because the operator gets no diagnostic.

Three admission rules decide what is this product's config, in descending confidence:
**A** the root is a real `ConfigFile` field; **B** the path is a real schema path that has **lost
its leading section(s)** (`credentials.backend` vs `storage.credentials.backend`) — the defect
stated as a predicate; **C** the leaf name is known to the schema *and* the carrying string is
about configuration.

---

## 3. Red-before-green — the only acceptance test that counts

`python3 .planning/evidence/remedy-gate/mutate.py` reverts one defect at a time in the working
tree, runs the gate, restores. A mutation whose pattern does not match reports
`SKIPPED-NOT-APPLIED`, never a pass. Baseline is checked green first, or the run aborts.

```
=== baseline (no mutation) ===
   test result: ok. 7 passed; 0 failed
case1-27-C2a                 expect=RED          got=RED            OK
case5-headless-keyring       expect=RED          got=RED            OK
case6-init-model             expect=RED          got=RED            OK
case2-23A-C1                 expect=RED          got=RED            OK
case7-piper-tool-name        expect=RED          got=RED            OK
case3-24-C2-NOT-COVERED      expect=STILL-GREEN  got=STILL-GREEN    OK
case4-ollama-NOT-COVERED     expect=STILL-GREEN  got=STILL-GREEN    OK
MUTRC=0
```

Sample failure text, unedited:

```
crates/wcore-browser/src/config_hint.rs:28 advertises
  `browser.allowed_origins = ["example.com", "*.mysite.com"]`
  -- it parses without error and is then SILENTLY DISCARDED

crates/wcore-agent/src/recovery_confidential.rs:76 advertises
  `credentials.backend = "encrypted-file"` -- it parses without error and is
  then SILENTLY DISCARDED

`--skills-promote` is advertised in --help but run_skills_promote() is an
  unconditional dead end
```

**The two STILL-GREEN rows are the point, not filler.** A coverage limit asserted in prose is
worth nothing. These re-introduce cases 3 and 4 and record that the gate does not see them.

---

## 4. The gate found a defect nobody had reported

**`wayland-core init --model X` wrote the model to a section the loader does not read.**

The generated `.wayland/config.toml` carried a **root-level** `model = "X"`. `ConfigFile` has no
root `model` field — it is `default.model` — and `ConfigFile` is `#[serde(default)]` with no
`deny_unknown_fields`, so the key parsed cleanly and was discarded. **The model the operator
explicitly chose never took effect.** Identical shape to `27-C2(a)` and the headless-keyring HIGH.

Live on `hetzner-dsm`, real binary built from this branch:

```
$ wayland-core init --model "claude-sonnet-4-5-20250929"
$ cat .wayland/config.toml
# Wayland-Core project config.
...
model = "claude-sonnet-4-5-20250929"       <-- root level

$ # real engine boot in that project, stderr captured
   5,630 bytes of stderr
   0 occurrences of "ignoring unknown or mis-sectioned config key"
```

The product **has** a mis-sectioned-key warner (`config.rs:3526`) and it did **not** fire. The
drop is fully silent, not merely quiet. Corroboration: every other config written anywhere in
this workspace already spells it `[default]` — this generator was the sole exception.

Fixed at `623ae4e8`, with a round-trip regression test in `init.rs`. The pre-existing test
asserted `body.contains("model =")` and passed against the defect.

**What I did NOT get:** an end-to-end observation of a *different model on the wire* between the
two configs. `--json-stream`'s `ready` event does not carry the model, `get_runtime_diagnostics`
does not either, `mcp-serve` exposes only 3 tools, and `models list` is a static catalog. So the
final link is proven through the real `ConfigFile` type rather than through a provider call. I
am not claiming more than that.

---

## 5. What the gate cannot check — measured, not estimated

| not covered | why | evidence |
|---|---|---|
| **617 prose remedies (41% of the inventory)** | no token to resolve | counted in §1 |
| **case 3 — flag *values* (`--trigger webhook:`)** | a value prefix whose consumer is a dispatcher; no single type to re-parse through | `case3-...` STILL-GREEN, measured |
| **case 4 — ordering (`ollama:`)** | the token is *correct*; the route was unreachable because credential resolution ran first. A control-flow property, not a naming one | `case4-...` STILL-GREEN, measured |
| **tool reachability** | case 7 was dead 4 ways; the check reads only leg 1 (the name does not exist). Legs 2–4 — builder returns `None` unconditionally, `synthesize` is a stub, non-default feature — all pass | stated on the test |
| 207 env vars | not wired: would need "is this read anywhere", plus the inverse (a var that works but is named nowhere — the headless-keyring §3d finding) | — |
| 136 bare dotted config keys | **deliberately dropped**: the category is polluted with SQL aliases (`m.rowid`, `c.guid`), JSON paths and Rust expressions. Admitting it reds the gate on correct text | measured in the inventory |
| 127 flags / 45 subcommands | only reached via the dead-end check, not existence-checked against the clap tree | — |

**Honest coverage figure: the gate checks 29 config assignments and 1 tool mention out of 1,504
inventory rows — about 2% of rows, or ~3.4% of the 887 rows that carry any token at all.**

That number is small and I am not dressing it up. What makes it worth having is *which* 2%: the
config-assignment category contains 4 of the 7 recorded instances of this defect class, and the
tool category contains a 5th. The prose majority has produced none, because prose cannot name a
wrong key.

---

## 6. Every gate here can fail, and each was seen failing

Not asserted — observed, in order:

- **Run 1: 2 passed, 3 FAILED.** `toml` 1.x's `FromStr` rejected all 36 candidates, so the main
  check counted **zero** checked and 36 "illustrative". Without the `checked >= 15` floor this
  ships as a green measuring nothing. The floor is why it was visible.
- **Run 2: 4 passed, 1 FAILED with 9 reports** — 6 were instrument defects (array-of-tables
  flattened; sibling required fields missing from one-key-at-a-time checking; a key literally
  named `set`, harvested from the English "backend **is set to** \"plaintext\"").
- **Mutation run 1: case 5 came back STILL-GREEN.** The most useful result of the lane. Case 5
  had only ever been caught *by accident*: its path was mis-bound to `session.credentials.backend`,
  and `session` is a real root, so admission fired for a reason unrelated to the defect. Its real
  message contains "confidential" but never "config", so the wording rule missed it too. That is
  what forced admission rule B.
- **Tool check: the corpus floor of 2 tripped** — because this lane's own fix removed two of the
  three mentions it was calibrated against. The weight was **moved onto a pinned control**
  (`tool_mention_extraction_reads_the_phrasing_that_shipped`), not lowered away.

---

## 7. Gate results — isolated per-crate runs

| suite | result |
|---|---|
| `wcore-cli --test remedy_advertisements` | **7 passed, 0 failed** (29 config assignments, 1 tool name, both resolved) |
| `mutate.py` (7 mutations) | **7/7 expectations met, `MUTRC=0`** |
| `wcore-cli --lib` | **1831 passed, 0 failed** (`LIBRC=0`; the one `FAILED` line is the nested `always_fails` fixture the suite shells out to — per LANE-BRIEF, read the lines, don't count them) |
| `wcore-browser` (whole crate) | 27 + 4 + 1 passed, 0 failed |
| `cargo clippy -p wcore-cli -p wcore-agent --all-targets` | **0 errors, 0 warnings in my files** |
| `cargo fmt --all -- --check` (Mac) | clean, 0 bytes |

### One pre-existing red, measured not assumed

`cargo test -p wcore-agent --lib` → **2115 passed, 16 failed.** All 16 are journal / durable-session
/ recovery-checkpoint tests; none touch `tts` or `piper`.

**A/B at the identical commit**, reverting only my two `wcore-agent` files to their base content:
**2112 passed, 19 failed.** More failures *without* my change than with it. The cluster is
pre-existing and flaky (16 vs 19 across two runs), not a regression from this lane. My
`wcore-agent` diff is two `tracing::` string literals.

---

## 8. Fixes landed (both are de-advertisements or re-sections, not features)

- **`623ae4e8`** — `init` writes `model` under `[default]`. Found by this gate.
- **`3e6ee6dd`** — the TTS keyless warning stops advertising `piper_download`. This is the
  `23A-C1` repair (de-advertise), **not** the `27-C2(a)` one (correct the name): the route cannot
  be corrected because it does not exist. **Piper is not implemented by this change and legs 2–4
  are untouched** — only the promise is withdrawn. Restore the mention in the same commit that
  makes a real local voice work, and not before.

**Worth escalating beyond this lane:** the Piper string misled *us*, not just users. Two planning
documents recommended Piper as "the only route that does not go through Sean" — a conclusion drawn
from reading the warning rather than the implementation. Those documents are other lanes'
artifacts and I have not edited them; they need correcting.

---

## 9. Fence and boundary compliance

- `git diff 2a306ac8 -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs` → **empty.**
  The shared fence was not touched at all. Diffed against the captured SHA, never the branch name.
- Full change set: **8 files, +3514 −3.** Four are new `.planning/` artifacts.
- Did NOT run `wcore-contract generate`. Did NOT merge into `plan/f20-unified-audit-repair`. Did
  NOT open a PR, tag, release, or close an issue. No `git add -A`, no `checkout`/`reset`/`stash`/
  `rebase` on shared refs. No `Co-Authored-By`.
- No test was weakened, ignored, re-gated or deleted. The one floor that was lowered (tool corpus
  2 → 1) had its weight moved onto a new pinned control in the same commit, and the reason is
  recorded at the assert.

---

## 10. Reproduce

```bash
cargo test -p wcore-cli --test remedy_advertisements -- --nocapture
export PATH=/root/.cargo/bin:$PATH
python3 .planning/evidence/remedy-gate/mutate.py    # 7/7, exits 0
python3 .planning/scripts/sweep-remedy-strings.py > /tmp/inv.tsv   # regenerate the inventory
```

`mutate.py` checks its own baseline is green before mutating anything. If the baseline is red,
every row below it would be meaningless and it stops.
