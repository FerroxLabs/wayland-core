//! Several accounts on the SAME provider (issue #14).
//!
//! A company with a dozen OpenRouter accounts needs each account's key to be
//! selectable per session AND storable securely. The *selection* half already
//! worked: one `[providers.<id>]` alias per account, picked with `--provider
//! <id>`. The *storage* half did not — [`credentials_store_key`] is keyed by
//! `ProviderType`, so the ladder held exactly ONE key per provider and every
//! extra account could only be a cleartext `api_key` in `config.toml`, which is
//! the sink `auth add` exists to empty.
//!
//! These drive the real resolution path ([`resolve_council_provider`], which
//! shares `resolve_api_key` with `Config::resolve`) against a real credentials
//! ladder, and assert on the RESOLVED key — not on the slot name, which would
//! pass while resolution read a different slot.
//!
//! HOST INDEPENDENCE: every case sets `WAYLAND_HOME`, which makes `open_store`
//! skip the process-global OS keyring by construction, and supplies a vault
//! passphrase so there is a secure tier to write into. Identical on Linux,
//! macOS and Windows, and it can never touch the developer's real Keychain.
//!
//! NO SECRET IS EVER PRINTED: the fixtures below are literals invented for the
//! test and are not credentials for anything.

use std::collections::HashMap;

use serial_test::serial;
use tempfile::tempdir;
use wcore_config::config::{
    Config, MAX_ACCOUNT_ID_LEN, ProviderConfig, credentials_store_account_key,
    resolve_council_provider, store_provider_account_api_key,
};

const KEY_A: &str = "or-fixture-account-a-11111111";
const KEY_B: &str = "or-fixture-account-b-22222222";
const SHARED_CLEARTEXT: &str = "or-fixture-shared-cleartext-99999999";

/// RAII env guard. Restores prior values on drop so one case cannot leak its
/// fixture into the next in the same process.
struct EnvGuard {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn scoped(home: &std::path::Path) -> Self {
        let pairs: [(&'static str, Option<String>); 5] = [
            ("WAYLAND_HOME", Some(home.display().to_string())),
            (
                "WAYLAND_VAULT_PASSPHRASE",
                Some("multi-account-test-passphrase".to_string()),
            ),
            ("WAYLAND_VAULT_PASSPHRASE_FD", None),
            // An ambient key of either shape would satisfy resolution from the
            // environment and make every assertion below vacuous.
            ("API_KEY", None),
            ("OPENROUTER_API_KEY", None),
        ];
        let saved = pairs
            .iter()
            .map(|(key, value)| {
                let prior = std::env::var_os(key);
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
                (*key, prior)
            })
            .collect();
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, prior) in &self.saved {
            unsafe {
                match prior {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

/// One `[providers.<id>]` alias per account, all on the same built-in provider.
fn account(underlying: &str) -> ProviderConfig {
    ProviderConfig {
        provider: Some(underlying.to_string()),
        ..Default::default()
    }
}

#[test]
#[serial(multi_account_credentials_env)]
fn two_accounts_on_one_provider_each_resolve_their_own_stored_key() {
    let dir = tempdir().unwrap();
    let _env = EnvGuard::scoped(dir.path());

    store_provider_account_api_key("acct-a", KEY_A).expect("store account A");
    store_provider_account_api_key("acct-b", KEY_B).expect("store account B");

    let mut providers = HashMap::new();
    providers.insert("acct-a".to_string(), account("openrouter"));
    providers.insert("acct-b".to_string(), account("openrouter"));

    let base = Config::default();
    let (a, _) = resolve_council_provider(&providers, &base, "acct-a").expect("acct-a resolves");
    let (b, _) = resolve_council_provider(&providers, &base, "acct-b").expect("acct-b resolves");

    assert_eq!(
        a.api_key, KEY_A,
        "account 'acct-a' did not resolve its own stored credential"
    );
    assert_eq!(
        b.api_key, KEY_B,
        "account 'acct-b' did not resolve its own stored credential"
    );
    // The whole point: two accounts, two DIFFERENT keys, one provider.
    assert_ne!(
        a.api_key, b.api_key,
        "both accounts on one provider resolved to the same key"
    );
    assert_eq!(
        a.provider, b.provider,
        "both accounts are the same provider"
    );
    // The label is what metering/billing attributes the spend to, so it must
    // carry the account, not the shared provider slug.
    assert_eq!(a.provider_label, "acct-a");
    assert_eq!(b.provider_label, "acct-b");
}

#[test]
#[serial(multi_account_credentials_env)]
fn an_accounts_stored_key_is_not_shadowed_by_the_shared_provider_key() {
    // The silent-wrong-account case. An alias INHERITS the underlying
    // `[providers.<builtin>].api_key` through `merge_provider_configs`, so
    // without an account rung above the inline value, account B would be
    // charged to the shared cleartext key of a different account — with no
    // error and no visible difference.
    let dir = tempdir().unwrap();
    let _env = EnvGuard::scoped(dir.path());

    store_provider_account_api_key("acct-b", KEY_B).expect("store account B");

    let mut providers = HashMap::new();
    providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            api_key: Some(SHARED_CLEARTEXT.to_string()),
            ..Default::default()
        },
    );
    providers.insert("acct-b".to_string(), account("openrouter"));

    let base = Config::default();
    let (b, _) = resolve_council_provider(&providers, &base, "acct-b").expect("acct-b resolves");

    assert_eq!(
        b.api_key, KEY_B,
        "account 'acct-b' resolved a credential that is not its own"
    );
    assert_ne!(
        b.api_key, SHARED_CLEARTEXT,
        "account 'acct-b' was silently billed to the shared provider key"
    );
}

#[test]
#[serial(multi_account_credentials_env)]
fn selecting_the_builtin_provider_is_unchanged_by_the_account_rung() {
    // Negative control for the rung above: a BUILT-IN selection carries no
    // account id, so it must still resolve exactly as before — inline config
    // value first. Without this, "accounts work" could be true while ordinary
    // single-account resolution silently changed.
    let dir = tempdir().unwrap();
    let _env = EnvGuard::scoped(dir.path());

    store_provider_account_api_key("acct-a", KEY_A).expect("store account A");

    let mut providers = HashMap::new();
    providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            api_key: Some(SHARED_CLEARTEXT.to_string()),
            ..Default::default()
        },
    );
    providers.insert("acct-a".to_string(), account("openrouter"));

    let base = Config::default();
    let (shared, _) =
        resolve_council_provider(&providers, &base, "openrouter").expect("builtin resolves");

    assert_eq!(shared.api_key, SHARED_CLEARTEXT);
    assert_eq!(shared.provider_label, "openrouter");
}

#[test]
#[serial(multi_account_credentials_env)]
fn an_account_with_no_stored_key_still_falls_through_to_the_provider() {
    // The documented alias-overlay semantic, asserted so a later change cannot
    // break it silently: an alias with NO credential of its own inherits the
    // underlying provider's, exactly as it inherits `model` and `base_url`.
    let dir = tempdir().unwrap();
    let _env = EnvGuard::scoped(dir.path());

    let mut providers = HashMap::new();
    providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            api_key: Some(SHARED_CLEARTEXT.to_string()),
            ..Default::default()
        },
    );
    providers.insert("acct-c".to_string(), account("openrouter"));

    let base = Config::default();
    let (c, _) = resolve_council_provider(&providers, &base, "acct-c").expect("acct-c resolves");
    assert_eq!(c.api_key, SHARED_CLEARTEXT);
}

#[test]
fn account_slot_names_are_narrow_and_never_collide_with_a_builtin() {
    // A slot name is a keyring entry, a TOML key in the plaintext backend, and
    // a prefix the chunked-write path appends to. Anything that could forge or
    // collide with a neighbouring slot gets NO slot at all.
    assert_eq!(
        credentials_store_account_key("acct-a").as_deref(),
        Some("providers.acct-a.api_key")
    );
    assert_eq!(
        credentials_store_account_key("Acct_B2").as_deref(),
        Some("providers.Acct_B2.api_key")
    );

    for hostile in [
        "",
        "a.b",  // would read as a nested TOML path
        "a b",  // whitespace
        "a\"b", // quote — forges a key boundary
        "a/b",
        "a\nb",
        "acct-\u{00e9}", // non-ASCII
    ] {
        assert_eq!(
            credentials_store_account_key(hostile),
            None,
            "hostile account id {hostile:?} was granted a store slot"
        );
    }
    let too_long = "a".repeat(MAX_ACCOUNT_ID_LEN + 1);
    assert_eq!(credentials_store_account_key(&too_long), None);
    assert!(credentials_store_account_key(&"a".repeat(MAX_ACCOUNT_ID_LEN)).is_some());

    // A built-in slug delegates, so account and provider writers/readers share
    // one slot — and an out-of-band provider still has none.
    assert_eq!(
        credentials_store_account_key("openrouter").as_deref(),
        Some("providers.openrouter.api_key")
    );
    assert_eq!(credentials_store_account_key("bedrock"), None);
    assert_eq!(credentials_store_account_key("vertex"), None);
}
