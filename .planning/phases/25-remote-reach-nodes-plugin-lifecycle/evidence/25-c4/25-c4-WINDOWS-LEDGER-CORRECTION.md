# Correction to `25-04-fail-closed-windows-ledger.txt` (G5)

Lane `25-c4-egress`, 2026-07-29. The original ledger file is **left byte-for-byte intact**;
this file is the correction record. Silently rewriting another lane's evidence would destroy
the audit trail that makes the defect legible.

## The false line

```
25-04-fail-closed-windows-ledger.txt:4
F25-SC4-CASE-DENIED-EGRESS: REFUSED host=seandesktop exit=1 \
  verdict=egress-surface-refused-without-credential \
  evidence=25-04-win-case-denied-egress.txt
```

The capture it cites records the opposite:

```
25-04-win-case-denied-egress.txt
COMMAND: wayland-core.exe backend probe cloud
…
EXIT: 0
```

**Reproduced today on the same host with the same binary**
(`C:\ferrox-25h\wayland-core.exe`, `wayland-core 0.12.25`), exit codes read from a status
file because a non-zero exit collapses to `1` over ssh+PowerShell (LANE-BRIEF §3.2):

```
BINARY=C:\ferrox-25h\wayland-core.exe
WLRC_PROBE=0        ← `backend probe cloud`
WLRC_RUN=1          ← `backend run --backend cloud --receipt-out D:\lane-25c4\r.json`
WLDONE
receipt written: False
```

Both markers present ⇒ a true reading, not an UNREADABLE one. Full capture:
`25-c4-windows-rerun.txt`.

## Corrected lines

```
F25-SC4-CASE-DENIED-EGRESS: SUPERSEDED — the cited capture is `backend probe cloud`,
  which exited 0, NOT 1. `probe` is a read-only availability check; it is not an egress
  denial and never was. Corrected by lane 25-c4-egress on 2026-07-29.

F25-SC4-CASE-DENIED-SECRET-RUN-WIN: REFUSED host=seandesktop exit=1 (read from status file)
  verdict=credential-absent-no-fallback  receipt-written=NO
  evidence=25-c4-windows-rerun.txt
  NOTE: this is the `backend run --backend cloud` leg the ledger's egress line implied but
  which had never actually been run on Windows. It DOES fail closed at exit 1 and writes no
  receipt — but the mechanism is credential absence, which is the SAME mechanism as
  CASE-DENIED-SECRET. It is therefore NOT an egress-policy denial, and Gap 4a stands
  unclosed on Windows.
```

## Second stale line found while correcting the first

```
25-04-fail-closed-windows-ledger.txt:5
F25-SC4-CASE-ROTATED-KEY: REFUSED … verdict=rotated-key-signature-refused
```

The Linux leg corrected this label to
`rotated-out-signature-refused-via-body-digest-cli-verify-is-integrity-only`, because
`backend receipt verify` establishes integrity and explicitly **not** identity. The Windows
line still carries the uncorrected wording, which overstates what was proven — the same
"corrected in one place, left wrong in the other" pattern as the egress line, and the second
instance of LANE-BRIEF §6b-ii in this one file.

```
F25-SC4-CASE-ROTATED-KEY: RELABEL → rotated-out-signature-refused-via-body-digest-
  cli-verify-is-integrity-only (matches the corrected Linux label).
  Identity is now separately provable via `backend receipt verify --against-backend`
  — see 25-c4-identity-proof.txt. That proof was run on Linux only.
```

## What is still NOT closed on Windows

An **egress-policy** denial on Windows needs two things this lane could not obtain cheaply:

1. a Windows build of `e34a323c` (the commit that arms the policy on the `backend` path) —
   every `wayland-core.exe` on `seandesktop` predates the fix, so the Windows binary still
   runs the cloud backend under allow-all;
2. a cloud credential on `seandesktop` — the F25 Fly credential exists only on
   `hetzner-dsm`, and the two hosts cannot reach each other.

Neither is a Sean-reserved item; both are build/provisioning work. Estimated 1 lane-session.
**Stated as open rather than papered over.**
