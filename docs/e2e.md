# E2E Infrastructure

End-to-end test runner for Wayland-Core, hosted on DigitalOcean ephemeral droplets.

The local scripts in `scripts/wayland-e2e-*.sh` spin a fresh Ubuntu droplet, install the Rust toolchain, clone the repo via a read-only deploy key, run a workload (smoke / nextest / cargo-mutants), capture results, then destroy the droplet. A nightly GitHub Actions workflow does the same on a schedule.

## Quick reference

| Command | Purpose | Runtime | Cost |
|---|---|---|---|
| `scripts/wayland-e2e-smoke.sh` | Validate DO + SSH + cloud-init + deploy-key path | ~90 sec | ~$0.0001 |
| `scripts/wayland-e2e-real-workload.sh` | Build wcore-cli + run nextest + cargo-mutants list on a fresh droplet | ~25-40 min | ~$0.05-0.10 |

Both scripts trap `EXIT` and always destroy the droplet, even on Ctrl+C or partial failure.

## One-time setup (the 6 steps)

You'll need: a DigitalOcean account, `ssh-keygen`, `curl`, `python3`. About 15 minutes total.

### 1. Generate a dedicated DO API token

DO Dashboard → **API** → **Generate New Token**.

- Name: `wayland-e2e-ephemeral`
- Scopes: `droplet` (read+write), `ssh_keys` (read), `regions` (read), `images` (read+write), `snapshots` (read)
- **Do NOT** use a full-account token — the e2e flow only needs the scopes above.

Copy the token (starts with `dop_v1_`). You'll paste it into the env file in step 6.

### 2. Generate a dedicated SSH keypair for DO droplets

```bash
ssh-keygen -t ed25519 -f ~/.ssh/do_wayland_e2e -C "wayland-e2e-ephemeral" -N ""
```

`-N ""` means no passphrase — required because the scripts run unattended.

### 3. Upload the SSH public key to DO

DO Dashboard → **Settings** → **Security** → **Add SSH Key** → paste `~/.ssh/do_wayland_e2e.pub`.

After upload, copy the **MD5 fingerprint** (looks like `aa:bb:cc:...`) from the DO key list. You'll need both the numeric `id` and the fingerprint.

To get them programmatically:

```bash
curl -H "Authorization: Bearer $DIGITALOCEAN_API_TOKEN" \
  https://api.digitalocean.com/v2/account/keys | jq '.ssh_keys[]'
```

### 4. Generate + register a GitHub deploy key for the repo

```bash
ssh-keygen -t ed25519 -f ~/.ssh/deploy_wayland_core -C "wayland-e2e-clone" -N ""
cat ~/.ssh/deploy_wayland_core.pub
```

Then in your browser: **https://github.com/FerroxLabs/wayland-core/settings/keys** → **Add deploy key**.

- Title: `wayland-e2e-clone`
- Key: paste the `.pub` content
- **Allow write access**: **leave UNCHECKED** (read-only)

### 5. Set a DO billing alert

DO Dashboard → **Settings** → **Billing** → **Billing Alerts** → **Add Alert**. Recommended $25–$50/mo cap. The scripts cannot set this for you (DO API doesn't expose it); it's a one-time manual step.

### 6. Create the env file

```bash
mkdir -p ~/.config/wayland-e2e
chmod 700 ~/.config/wayland-e2e
cp scripts/do.env.example ~/.config/wayland-e2e/do.env
chmod 600 ~/.config/wayland-e2e/do.env
$EDITOR ~/.config/wayland-e2e/do.env
```

Fill in the values from steps 1–4. The example file documents each variable inline.

## Run the smoke test

```bash
scripts/wayland-e2e-smoke.sh
```

Expected output ends with `SMOKE TEST: PASS`. Total time ~90 seconds. If anything fails, the trap destroys the droplet — check your DO dashboard if you see "destroy returned HTTP <non-204>".

## Run the real-workload test

```bash
scripts/wayland-e2e-real-workload.sh
```

This actually exercises the Rust toolchain end-to-end: builds `wcore-cli` in release mode, runs `cargo nextest` on 5 representative crates, and enumerates `cargo mutants`. Outputs land in `/tmp/wayland-e2e-real-<timestamp>/`. Expected runtime 25–40 min on a `s-4vcpu-8gb` droplet ($0.0714/hr).

Override the droplet size:

```bash
DROPLET_SIZE=s-8vcpu-16gb scripts/wayland-e2e-real-workload.sh
```

## Troubleshooting

**`Size is not available in this region`** — DigitalOcean doesn't offer the requested droplet size in your configured region. List what IS available:

```bash
curl -s -H "Authorization: Bearer $DIGITALOCEAN_API_TOKEN" \
  "https://api.digitalocean.com/v2/sizes?per_page=200" | \
  jq '.sizes[] | select(.regions[] | contains("nyc3")) | {slug, vcpus, memory, price_hourly}'
```

**`cannot find 'ld'` / `-fuse-ld=mold`** — the project's `.cargo/config.toml` specifies `mold` as the linker. Make sure `mold` is in the cloud-init `packages:` list in the script.

**SSH retries fail** — the droplet booted but `sshd` isn't responding. Wait longer (the script retries 6× 8s) or check the DO console for boot errors via the dashboard's recovery console.

**Orphan droplet detected on DO dashboard** — a previous script failed in a way the trap couldn't recover (e.g., your laptop crashed). Destroy manually:

```bash
DROPLET_ID=<id from dashboard>
curl -X DELETE \
  -H "Authorization: Bearer $DIGITALOCEAN_API_TOKEN" \
  "https://api.digitalocean.com/v2/droplets/$DROPLET_ID"
```

Or filter by tag (all e2e droplets are tagged `wayland-e2e`):

```bash
curl -s -H "Authorization: Bearer $DIGITALOCEAN_API_TOKEN" \
  "https://api.digitalocean.com/v2/droplets?tag_name=wayland-e2e" | jq '.droplets[] | {id, name, created_at}'
```

## Security model

- **DO API token** lives only in `~/.config/wayland-e2e/do.env` (chmod 600) and is sourced into the script's env on each run. Never committed (the path is outside the repo + `do.env` is in `.gitignore` as belt-and-suspenders).
- **DO SSH private key** stays on your machine. The pubkey on DO authorizes droplet access for any droplet attached to that key — keep the scope of "what droplets get this key" tight via the per-create API call.
- **GitHub deploy key** is read-only on `FerroxLabs/wayland-core`. It cannot push code. Its private half lives at `$GITHUB_DEPLOY_KEY_PATH` locally; the scripts inject it into the droplet via cloud-init's `write_files`.
- **Cloud-init payload** is plaintext on DO's metadata service for the droplet's lifetime. The deploy key is recoverable from `http://169.254.169.254/metadata/v1/user-data` by anything inside the droplet — acceptable for short-lived ephemeral runs, but if you build a long-lived snapshot, rotate the deploy key out of cloud-init and inject it via SSH instead.

## Files

| Path | Purpose |
|---|---|
| `scripts/wayland-e2e-smoke.sh` | Fast control-plane smoke (~90 sec) |
| `scripts/wayland-e2e-real-workload.sh` | Full workload validation (~25-40 min) |
| `scripts/do.env.example` | Env file template |
| `~/.config/wayland-e2e/do.env` | Your secrets (gitignored) |
| `.blackboard/E2E-DO-ONDEMAND-2026-05-24.md` | Architecture spec |
| `.blackboard/E2E-TESTING-STRATEGY-2026-05-24.md` | Broader testing strategy |
