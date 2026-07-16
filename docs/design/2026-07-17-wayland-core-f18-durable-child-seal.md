# F18 durable child seal

Date: 2026-07-17

Integrated commit: `ada492d`

F18 defines one canonical, crash-durable child resource. The model lives in
`wcore-types`, `DurableChildStore` remains the single public declaration,
transition, inspect, and list boundary, and `wcore-protocol` re-exports that
model without introducing a parallel lifecycle. Commands and supervision
events remain F22 work.

## Acceptance evidence

- Durable record identity, lineage, policy snapshot, provider/model,
  workspace, lifecycle, desired state, recovery, result, delivery, retry, and
  idempotency evidence are persisted and restart-stable.
- Invalid identifiers, stale/conflicting transitions, post-terminal mutation,
  parent expiry, recovery intent, delivery failure, and unknown outcomes fail
  closed in the durable-store corpus.
- Desktop contract v1.8, generator
  `wcore-desktop-contract-gen/11`, publishes canonical record, ordered-list,
  transition, malformed-ID, and unknown-field fixtures.
- Fixture digest:
  `sha256:dfc21b05e37a1d7659cf9da880f1b2b86d8c0d1900b48ddc6273728c4eb00f7e`.
- Source-input digest:
  `sha256:317698bb078a9766c2c6b795036b9ae4a0f3479099d8e561e513710c35b07277`.

## Exact integrated-HEAD proof

Executed through the Hetzner remote Cargo harness against `ada492d`:

- `cargo test -p wcore-agent --test durable_child_store_test`: 12 passed.
- `cargo test -p wcore-protocol --test desktop_contract_corpus`: 15 passed.
- `cargo clippy -p wcore-protocol -p wcore-agent --all-targets -- -D warnings`:
  passed.

F18 is sealed. F19 owns routing every spawner through this lifecycle. F20-F23
remain outside this seal.
