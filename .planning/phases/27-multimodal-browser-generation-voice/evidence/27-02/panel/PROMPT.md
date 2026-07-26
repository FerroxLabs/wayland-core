# Phase 27 / plan 27-02 — cross-audit decision bundle

You are one of four independent reviewers. Answer with EXACTLY ONE of the four
option ids below, then your reasoning. Do not invent a fifth option.

## THE QUESTION

How does the engine publish live browser, computer-use and web readiness
without breaking the existing linkage-derived flags that a shipped host already
consumes?

## THE FOUR OPTIONS (verbatim from the plan — do not re-scope)

### `activation-chain-only`
Publish readiness ONLY as typed activation events on the existing ladder, and
leave the linkage booleans exactly as they are with their meaning documented
in-source.

- PROS: Uses a mechanism already designed for precisely this — proved
  activation rather than inferred availability — with an unavailable stage and
  a fixed reason vocabulary already defined; adds no new boolean a host can
  misread; the existing frozen-shape golden tests are untouched because no
  existing field changes; the smallest possible wire delta.
- CONS: A host that reads only the booleans keeps its current misreading until
  it learns the new events, so the false ready persists for unchanged
  consumers; requires the Desktop side to consume an event stream rather than a
  field, which this repository cannot land and cannot verify.

### `chain-plus-derived-flags`
Publish the activation chain AND redefine the existing booleans to mean live
readiness, keeping the old linkage answer available under a separate additive
field.

- PROS: Fixes the misreading for every existing host immediately and without a
  host change, which is the only option that actually removes the false ready
  from consumers that exist today; the linkage answer is preserved rather than
  destroyed, so the release smoke test that asserts linkage survived
  optimization keeps a field to assert on.
- CONS: Changes the meaning of a field already on the wire, which is a
  compatibility event even though the shape is unchanged — it must be called
  that plainly and carried in the contract bump; a host that special-cases the
  old meaning will observe a behavior change it did not ask for.

### `chain-plus-new-flags`
Publish the activation chain and add separate live-readiness booleans alongside
the untouched linkage booleans.

- PROS: Breaks nothing at all — every existing field keeps its exact current
  meaning, and the additive off-default discipline keeps the wire byte-identical
  for hosts that have not learned the new fields.
- CONS: Ships two booleans per capability whose difference is subtle, which is a
  durable misreading hazard of its own; the false ready stays reachable for any
  host that keeps reading the old field, so the requirement is satisfied in
  letter more than in effect.

### `escalate`
Escalate — every option either leaves the false ready reachable or changes the
meaning of a live field, and Sean declines both costs.

- PROS: Leaves the wire and its consumers untouched and keeps the decision
  visible rather than absorbed into an implementation detail.
- CONS: The handshake keeps claiming capabilities a machine cannot deliver,
  which is the exact dishonesty criterion 2 was written to remove.

---

## MEASURED EVIDENCE (taken on real hardware, not read out of source)

Host: `hetzner-dsm`, Ubuntu 24.04, Linux 6.8.0-101-generic.
SHA: `2ecdfdf54ff7fda920eec7d068337006e5da4ee4`.
Binary: `target/release/wayland-core`, built `--release --locked` at that SHA.

Machine facts established first: no `chromium`, `chromium-browser`,
`google-chrome` or `camoufox` anywhere on PATH; `DISPLAY` and `WAYLAND_DISPLAY`
both unset. This box cannot drive a browser and cannot drive a desktop.

### OBS-01 — what the shipped handshake claims on that machine

Driving `wayland-core --json-stream` with a throwaway home and capturing the
`ready` frame:

    browser_suite=True  computer_use=True  plugins=True  mcp=False

### OBS-02..05 — one absence at a time; the claim never moves

Each observation changes exactly ONE variable and re-captures the handshake.

| Observation | Variable changed | browser_suite | computer_use |
|---|---|---|---|
| baseline | nothing | true | true |
| no-browser-backend | PATH emptied so no browser binary resolves | true | true |
| display-advertised | `DISPLAY=:99` set with no X server behind it | true | true |
| cloud-creds-absent | no Browserbase credential | true | true |
| cloud-creds-present | `BROWSERBASE_API_KEY` set to a fixture value | true | true |

The capability claim is INVARIANT under every absence. This is consistent with
the in-repo documentation of the flags: `crates/wcore-cli/tests/
release_binary_smoke.rs` states that `computer_use` is "derived from plugin
presence via `PluginCapabilitySet::from_verified`, NOT from runtime
`HostCuaRegistrar.computer_use_advertised`". The flags report LINKAGE.

### OBS-06 — the activation ladder is wired, but not to these three surfaces

The same captures show the ladder working correctly for other capabilities:

    pricing_refresher       declared -> unavailable (reason: disabled_by_config)
    learned_policy          declared -> unavailable (reason: runtime_path_unwired)
    smart_handoff           declared -> unavailable (reason: disabled_by_config)
    delegate_isolation      declared -> unavailable (reason: isolation_not_enforced)
    mid_flight_monitor      declared -> configured -> constructed -> ready
    cooldown_tracker        declared -> configured -> constructed -> ready

Zero activation events are emitted for browser, computer use or web. The honest
mechanism exists and simply does not cover these three.

### OBS-07 — the operation the true flag promised

With `browser_suite=true` claimed on that same machine, a real navigate was
driven through the product (`Browser` tool, `{"op":{"kind":"navigate",
"url":"https://example.com/"}}`, origin admitted via `[browser.policy]`):

    is_error=True
    session: backend error: Camoufox is unavailable at
    http://localhost:9377/health and Core could not start `camofox-browser`:
    spawn camoufox: No such file or directory (os error 2).

So the handshake said available and the very next operation could not deliver.

### OBS-08 — a policy guarantee that DOES hold

A navigate to `http://127.0.0.1:1/` was refused before the backend was touched:

    policy: policy denied: loopback IP blocked: 127.0.0.1

### OBS-09 — a defect found while establishing the above

With the browser tool disabled, the product prints its own remediation:

    Browser tool is disabled by default. Add allowed domains to your
    config.toml to enable it:

    [browser]
    allowed_origins = ["example.com", "*.mysite.com"]

That instruction does not work. Following it verbatim leaves the tool disabled.
The key the config actually reads is `[browser.policy] allowed_origins`
(`crates/wcore-config/src/browser.rs`). An unavailable whose stated fix is
wrong is not an honest unavailable.

---

## WHAT THE DECISION MUST WEIGH

- The consumer that exists in this repository is
  `crates/wcore-cli/tests/release_binary_smoke.rs`, which asserts
  `browser_suite == true` and `computer_use == true` on a machine that, as
  measured above, has no browser backend and no display. Under
  `chain-plus-derived-flags` that test goes RED on exactly the machine CI runs
  on, and that is a real cost that must be counted, not waved at.
- The protocol already carries an append-only activation ladder with an
  explicit `unavailable` stage, a fixed reason vocabulary, and a
  well-formedness rule that makes a reason-less unavailable unconstructible.
- Whatever is chosen lands with the contract minor bumped, the generator
  version bumped and the byte-exact desktop manifest regenerated. Those files
  are a SERIALIZED SEAM shared with a concurrently executing phase, so the
  change has to be describable as a single precise patch.
- Nothing here is merged to main, opened as a PR, tagged or released by this
  work, so a wrong choice is recoverable by revert.

Answer with exactly one option id, then your reasoning.
