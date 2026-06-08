# Marketplace allowlist index format (v1.0)

Wayland-Core's `wayland-core plugin install <name>` searches a curated,
signed JSON index for the resolved repo+tag+sha+pubkey of each plugin.
This document specifies that index format.

## Envelope

```json
{
  "signature": "<128-hex-char ed25519 signature over body>",
  "body": {
    "schema_version": "1.0",
    "plugins": [...]
  }
}
```

The signature is the ed25519 signature of `serde_json::to_vec(&body)` —
the canonical, sorted-key JSON serialization. The verifying pubkey is
bundled into the CLI binary at build time (see `INDEX_PUBKEY_HEX` in
`crates/wcore-cli/src/plugin/index.rs`).

## Entry schema

| Field          | Type   | Notes                                                    |
|----------------|--------|----------------------------------------------------------|
| `name`         | string | Canonical plugin name; matches `PluginManifest.name`.    |
| `repo`         | string | Source URL, e.g. `github://owner/repo`.                  |
| `tag`          | string | Pinned tag (`v1.0.0`).                                   |
| `sha256`       | string | Hex SHA-256 of the source archive at `tag`.              |
| `pubkey`       | string | Plugin author's ed25519 pubkey, hex.                     |
| `description`  | string | One-liner shown by `plugin search`.                      |
| `review_date`  | string | ISO-8601 date of the review.                             |
| `review_notes` | string | Free-form notes.                                         |

Unknown fields are rejected (`serde(deny_unknown_fields)`).

## Schema versioning

Today only `schema_version: "1.0"` is accepted. A future bump
(`1.1`, `2.0`) requires expanding the accept-list in
`IndexVerifier::verify`.

## Cache

The verified body is cached to `$HOME/.wayland/index.json`. Default
TTL: 86400s (24h). The CLI re-fetches on TTL expiry and falls back to
the cached body if the network fetch fails.

## Install path (Phase 2 follow-up)

`wayland-core plugin install <name>` will:

1. Load the cached index (or fetch + verify if absent / expired).
2. Resolve `<name>` to an `IndexEntry`.
3. Clone `entry.repo` at `entry.tag`.
4. Verify SHA-256 of the archive matches `entry.sha256`.
5. Verify the plugin's own signature using `entry.pubkey`.
6. Run the existing install pipeline.

This wiring lands as a Phase 2 follow-up — the index types and the
signature-verification primitives ship in 1.E.1 so the install path
has the trust-boundary code ready to consume.
