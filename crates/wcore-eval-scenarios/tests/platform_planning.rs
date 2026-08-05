use wcore_eval_scenarios::scenario::{
    Category, Platform, PlatformDisposition, Scenario, UnsupportedPlatform,
};

#[test]
fn scenarios_are_portable_by_default() {
    let scenario = Scenario::new("portable", Category::Coverage);

    for platform in [Platform::Linux, Platform::Macos, Platform::Windows] {
        assert_eq!(
            scenario.resolve_platform(platform, false),
            Ok(PlatformDisposition::Runnable)
        );
    }
}

#[test]
fn unsupported_platform_is_an_explicit_skip_in_lenient_mode() {
    let scenario = Scenario::new("unix_only", Category::Coverage)
        .platforms([Platform::Linux, Platform::Macos]);

    assert_eq!(
        scenario.resolve_platform(Platform::Windows, false),
        Ok(PlatformDisposition::Skipped {
            current: Platform::Windows,
            supported: vec![Platform::Linux, Platform::Macos],
        })
    );
}

#[test]
fn unsupported_platform_fails_preflight_in_strict_mode() {
    let scenario = Scenario::new("unix_only", Category::Coverage)
        .platforms([Platform::Linux, Platform::Macos]);

    assert_eq!(
        scenario.resolve_platform(Platform::Windows, true),
        Err(UnsupportedPlatform {
            scenario: "unix_only",
            current: Platform::Windows,
            supported: vec![Platform::Linux, Platform::Macos],
        })
    );
}
