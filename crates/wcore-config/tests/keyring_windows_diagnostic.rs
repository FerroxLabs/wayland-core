#![cfg(windows)]

use std::sync::{Arc, Barrier};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use wcore_config::credentials::{CredentialsStore, KeyringCredentialsStore};

// Diagnostic only: distinct owned entries, no ambient credential enumeration.
// Immediate failures remain failures even if the later diagnostic read succeeds.
fn pair(service: &str, index: usize) -> usize {
    let mut failures = 0;
    let key = format!("raw-{index}");
    let entry = keyring::Entry::new(service, &key).expect("raw entry");
    let put = entry.set_password("test-secret");
    let immediate = entry.get_password();
    let ok = put.is_ok() && matches!(immediate.as_deref(), Ok("test-secret"));
    eprintln!(
        "raw index={index} thread={:?} put_ok={} read_ok={} read_missing={}",
        std::thread::current().id(),
        put.is_ok(),
        ok,
        matches!(immediate, Err(keyring::Error::NoEntry))
    );
    if !ok {
        failures += 1;
        std::thread::sleep(Duration::from_millis(20));
        eprintln!(
            "raw delayed_read_ok={}",
            matches!(entry.get_password().as_deref(), Ok("test-secret"))
        );
    }
    let deleted = entry.delete_credential();
    let absent = matches!(entry.get_password(), Err(keyring::Error::NoEntry));
    if deleted.is_err() || !absent {
        failures += 1;
    }

    let key = format!("store-{index}");
    let store = KeyringCredentialsStore::new(service);
    let put = store.put(&key, "test-secret");
    let immediate = store.get(&key);
    let ok = put.is_ok() && matches!(&immediate, Ok(Some(v)) if v == "test-secret");
    eprintln!(
        "store index={index} thread={:?} put_ok={} read_ok={} read_missing={}",
        std::thread::current().id(),
        put.is_ok(),
        ok,
        matches!(immediate, Ok(None))
    );
    if !ok {
        failures += 1;
        std::thread::sleep(Duration::from_millis(20));
        eprintln!(
            "store delayed_read_ok={}",
            matches!(store.get(&key), Ok(Some(v)) if v == "test-secret")
        );
    }
    let deleted = store.delete(&key);
    let absent = matches!(store.get(&key), Ok(None));
    if deleted.is_err() || !absent {
        failures += 1;
    }
    failures
}

#[test]
fn native_raw_and_public_keyring_immediate_reads() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let base = format!("wayland-diagnostic-keyring-{}-{nonce}", std::process::id());
    let mut failures = 0;
    eprintln!("phase=isolated pairs=16");
    for i in 0..16 {
        failures += pair(&base, i);
    }
    eprintln!("phase=independent_key_pressure threads=4 pairs_per_thread=16");
    let barrier = Arc::new(Barrier::new(4));
    let workers: Vec<_> = (0..4)
        .map(|worker| {
            let service = format!("{base}-{worker}");
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                (0..16).map(|i| pair(&service, i)).sum::<usize>()
            })
        })
        .collect();
    for worker in workers {
        failures += worker.join().expect("diagnostic worker");
    }
    assert_eq!(
        failures, 0,
        "immediate keyring or cleanup failures; later reads never erase failures"
    );
}
