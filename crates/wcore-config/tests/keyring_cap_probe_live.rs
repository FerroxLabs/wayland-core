//! VERIFY-CRED lane. Measures the host credential store's REAL per-entry
//! ceiling, so the 1000-UTF-16-unit spanning threshold can be graded against
//! each backend instead of against the Windows number alone.
//!
//! `KEYRING_MAX_UTF16_UNITS_PER_ENTRY = 1000` is derived from Windows
//! (`CRED_MAX_CREDENTIAL_BLOB_SIZE` 2560 bytes / 2 = 1280 units, minus
//! headroom). macOS Keychain and Secret Service were shown to be LOSSLESS at
//! that size but were never shown to NEED it. Two things can be wrong:
//!
//! * too LARGE for some backend — a single part would be silently truncated or
//!   refused, which is the failure class this whole rewrite exists to close;
//! * too SMALL for some backend — needless chunking, which multiplies the entry
//!   count, the crash window and the orphan surface on a platform that has no
//!   cap worth working around.
//!
//! `#[ignore]`d: it writes into the host's real credential store.

use wcore_config::credentials::{CredentialsStore, KeyringCredentialsStore};

const SERVICE: &str = "wayland-core-verify-cred-cap-probe";
const THRESHOLD: usize = 1000;

fn utf16_units(value: &str) -> usize {
    value.chars().map(char::len_utf16).sum()
}

/// Can ONE entry hold `units` UTF-16 units, written and read back intact?
///
/// Grades three states, not two: a write that is refused and a write that is
/// ACCEPTED but reads back different are both "no", and they are different
/// kinds of no — the second is silent truncation, which is far worse.
///
/// Written through a BARE `keyring::Entry`. `KeyringCredentialsStore` is the
/// spanning writer, so measuring through it would measure the chunker, not the
/// backend — the exact mistake that makes a cap probe report a cap that is not
/// there.
fn holds(key: &str, units: usize) -> Result<bool, String> {
    let value = "x".repeat(units);
    let entry = keyring::Entry::new(SERVICE, key).map_err(|e| format!("entry: {e}"))?;
    match entry.set_password(&value) {
        Err(_) => Ok(false),
        Ok(()) => match entry.get_password() {
            Ok(read) if read == value => Ok(true),
            Ok(read) => Err(format!(
                "SILENT TRUNCATION: the backend ACCEPTED {units} units and returned {} units",
                utf16_units(&read)
            )),
            Err(keyring::Error::NoEntry) => Err(format!(
                "SILENT LOSS: the backend ACCEPTED {units} units and then returned nothing"
            )),
            Err(e) => Err(format!("read failed at {units} units: {e}")),
        },
    }
}

#[test]
#[ignore = "writes into the host's real credential store; run with -- --ignored"]
fn measure_the_real_per_entry_ceiling_of_this_hosts_credential_store() {
    let store = KeyringCredentialsStore::new(SERVICE);
    let key = "capprobe.single";

    // Instrument self-test. A host that cannot round-trip a tiny value cannot
    // measure anything, and "could not look" must never render as a pass.
    let probe = store.put(key, "probe").and_then(|()| store.get(key));
    assert!(
        matches!(&probe, Ok(Some(v)) if v == "probe"),
        "NOT MEASURED — this host's credential store could not round-trip a 5-character \
         value ({probe:?})"
    );

    // The threshold itself must be holdable, or the product is broken here.
    let at_threshold = holds(key, THRESHOLD).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        at_threshold,
        "DEFECT — this host cannot hold the {THRESHOLD}-unit chunk size the writer uses, so \
         every spanned write on it is refused or torn"
    );

    // Exponential probe upward, then bisect, to find the largest holdable size.
    // Capped at 1 MiB of units: past that the answer is "no meaningful cap".
    const CEILING: usize = 1_048_576;
    let mut lo = THRESHOLD; // known holdable
    let mut hi = None; // smallest known NOT holdable
    let mut size = THRESHOLD * 2;
    while size <= CEILING {
        match holds(key, size).unwrap_or_else(|e| panic!("{e}")) {
            true => {
                lo = size;
                size *= 2;
            }
            false => {
                hi = Some(size);
                break;
            }
        }
    }
    if let Some(mut high) = hi {
        while high - lo > 1 {
            let mid = lo + (high - lo) / 2;
            match holds(key, mid).unwrap_or_else(|e| panic!("{e}")) {
                true => lo = mid,
                false => high = mid,
            }
        }
    }
    let _ = store.delete(key);

    let verdict = match hi {
        Some(_) => format!("{lo} UTF-16 units"),
        None => format!(">= {lo} UTF-16 units (no cap found below {CEILING})"),
    };
    println!(
        "MEASURED os={} real_single_entry_ceiling={verdict} configured_chunk_size={THRESHOLD} \
         ratio={}",
        std::env::consts::OS,
        lo / THRESHOLD
    );
    println!(
        "GRADE too_large={} too_small_by={}x",
        lo < THRESHOLD,
        lo / THRESHOLD
    );
}
