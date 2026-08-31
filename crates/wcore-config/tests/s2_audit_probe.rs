//! S2 independent instrument probe for wayland#1252 c5 (site C).
//! Compares the NEW `scrub_detail` against a faithful transcription of the
//! hand cut it replaced, over realistic `details` shapes.
use wcore_config::portability::scrub_detail;

/// Faithful transcription of the PRE-FIX `strip_url_userinfo` (base ca15a48bf).
fn old_strip_url_userinfo(v: &str) -> String {
    let Some(scheme_end) = v.find("://") else {
        return v.to_string();
    };
    let rest_start = scheme_end + 3;
    let rest = &v[rest_start..];
    let authority_end = rest.find(0x2fu8 as char).unwrap_or(rest.len());
    let Some(at) = rest[..authority_end].find(0x40u8 as char) else {
        return v.to_string();
    };
    format!("{}<redacted>@{}", &v[..rest_start], &rest[at + 1..])
}

#[test]
fn s2_probe_credential_shapes_still_redacted() {
    let cases = [
        "https://u:pw@h.example/x",
        "srv --url https://u:pw@h.example/x",
        "srv --url=https://u:pw@h.example/x",
        "--endpoint=https://u:pw@h.example/x",
        "url=https://u:pw@h.example/x",
        "\"https://u:pw@h.example/x\"",
        "(https://u:pw@h.example/x)",
        "<https://u:pw@h.example/x>",
        "postgres://u:pw@db.example:5432/app",
        "DATABASE_URL=postgres://u:pw@db.example:5432/app",
        "{\"url\":\"https://u:pw@h.example/x\"}",
        "connect https://u:pw@h.example/x; retry",
    ];
    let mut leaks = Vec::new();
    for c in cases {
        let new = scrub_detail(c);
        let old = old_strip_url_userinfo(c);
        let new_leaks = new.contains("pw@") || new.contains("u:pw");
        let old_leaks = old.contains("pw@") || old.contains("u:pw");
        println!("INPUT : {c}");
        println!("  OLD : {old}   leak={old_leaks}");
        println!("  NEW : {new}   leak={new_leaks}");
        if new_leaks && !old_leaks {
            leaks.push(c);
        }
    }
    assert!(
        leaks.is_empty(),
        "NEW scrub_detail leaks credential material the OLD cut redacted: {leaks:#?}"
    );
}
