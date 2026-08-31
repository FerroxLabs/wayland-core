//! INDEPENDENT VERIFIER INSTRUMENT (not the lane's fixture).
//!
//! Differential A/B of the PUBLIC api the production caller uses
//! (`wcore_agent::output_redaction::redact_tool_output` -> `PIIScrubber::scrub`).
//! Run the SAME file in the base tree (integ/f13) and the lane tree; the two
//! stdouts must be byte-identical or detection/redaction changed.
use wcore_safety::PIIScrubber;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % (n as u64)) as usize
    }
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

const SECRETS: &[&str] = &[
    "AKIAIOSFODNN7EXAMPLE",
    "sk-abcdefghijklmnopqrstuvwxyz012345",
    "sk-ant-api03-AAAA_BBBB-CCCC",
    "ghp_abcdefghijklmnopqrstuvwxyz0123",
    concat!("xo", "xb-1234567890-abcdefghijklmno"),
    "AIzaSyA1234567890abcdefghijklmnopqrstuv",
    "hf_abcdefghijklmnopqrstuvwxyz01",
    "Bearer aaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "API_KEY=supersecretvalue123456",
    "postgres://u:pw@host/db",
    "eyJhbGciOi.eyJzdWIiOi.SflKxwRJSM",
    "+441234567890",
];

/// Alphabet pools chosen so the four base64 alphabets DISAGREE on some cases:
/// `+/` is STANDARD-only, `-_` is URL_SAFE-only, `=` is padding.
const POOLS: &[&str] = &[
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+/=",
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_=",
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+/-_=",
    "abcdefghijklmnopqrstuvwxyz0123456789 \r\n\t",
    "abcdef .,;:!?\n",
];

fn case(rng: &mut Rng, i: usize) -> String {
    use base64::Engine as _;
    let mut s = String::new();
    let chunks = 1 + rng.below(5);
    for _ in 0..chunks {
        match rng.below(6) {
            // raw run from one of the pools
            0 | 1 | 2 => {
                let pool: Vec<char> = POOLS[rng.below(POOLS.len())].chars().collect();
                let n = rng.below(90);
                for _ in 0..n {
                    s.push(pool[rng.below(pool.len())]);
                }
            }
            // a base64-encoded secret, in one of the four alphabets
            3 => {
                let sec = SECRETS[rng.below(SECRETS.len())];
                let pad = "?".repeat(rng.below(40));
                let plain = format!("{sec}{pad}");
                let enc = match rng.below(4) {
                    0 => base64::engine::general_purpose::STANDARD.encode(&plain),
                    1 => base64::engine::general_purpose::STANDARD_NO_PAD.encode(&plain),
                    2 => base64::engine::general_purpose::URL_SAFE.encode(&plain),
                    _ => base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&plain),
                };
                s.push_str(&enc);
            }
            // a plaintext secret
            4 => {
                s.push_str(SECRETS[rng.below(SECRETS.len())]);
            }
            // a SECRET_ASSIGNMENT-shaped line
            _ => {
                s.push('\n');
                s.push_str("export SERVICE_TOKEN = ");
                s.push_str(SECRETS[rng.below(SECRETS.len())]);
                s.push('\n');
            }
        }
        if rng.below(3) == 0 {
            s.push(' ');
        }
    }
    if i % 7 == 0 {
        s.push_str(&"x".repeat(200));
    }
    s
}

#[test]
fn differential_corpus() {
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let mut lines = Vec::new();
    for i in 0..4000usize {
        let input = case(&mut rng, i);
        let out = PIIScrubber.scrub(&input);
        lines.push(format!(
            "{i} in_len={} in_fnv={:016x} out_len={} out_fnv={:016x} enc={} tot={}",
            input.len(),
            fnv(&input),
            out.len(),
            fnv(&out),
            out.matches("[REDACTED:ENCODED_SECRET]").count(),
            out.matches("[REDACTED:").count()
        ));
    }
    let all = lines.join("\n");
    std::fs::write(
        std::env::var("DIFF_OUT").unwrap_or_else(|_| "/tmp/diffscrub.txt".into()),
        &all,
    )
    .unwrap();
    eprintln!("DIGEST {:016x} cases={}", fnv(&all), lines.len());
}
