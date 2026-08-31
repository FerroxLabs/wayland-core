//! AUDIT-ONLY instrument (second instrument for lane/f13-s2-turn-cost).
//! Times PIIScrubber::scrub over four payload sizes and fits log-log exponent.
use std::time::Instant;
use wcore_safety::PIIScrubber;

fn one(len: usize) -> f64 {
    let text = "x".repeat(len);
    let start = Instant::now();
    let out = PIIScrubber.scrub(&text);
    let e = start.elapsed().as_secs_f64();
    assert!(!out.is_empty());
    e
}

#[test]
#[ignore]
fn zz_audit_scrub_shape() {
    // warm the OnceLock regex sets
    let _ = PIIScrubber.scrub(&"x".repeat(4096));
    let sizes = [60_000usize, 120_000, 240_000, 480_000];
    let mut pts: Vec<(f64, f64)> = Vec::new();
    for &n in &sizes {
        let a = one(n);
        let b = one(n);
        let t = a.min(b);
        println!("SCRUB n={n} secs={t:.4}");
        pts.push(((n as f64).ln(), t.ln()));
    }
    let k = pts.len() as f64;
    let sx: f64 = pts.iter().map(|p| p.0).sum();
    let sy: f64 = pts.iter().map(|p| p.1).sum();
    let sxx: f64 = pts.iter().map(|p| p.0 * p.0).sum();
    let sxy: f64 = pts.iter().map(|p| p.0 * p.1).sum();
    let slope = (k * sxy - sx * sy) / (k * sxx - sx * sx);
    let intercept = (sy - slope * sx) / k;
    let mean_y = sy / k;
    let ss_tot: f64 = pts.iter().map(|p| (p.1 - mean_y).powi(2)).sum();
    let ss_res: f64 = pts
        .iter()
        .map(|p| (p.1 - (intercept + slope * p.0)).powi(2))
        .sum();
    let r2 = 1.0 - ss_res / ss_tot;
    let per_mb = pts[3].1.exp() / 0.48;
    println!("SCRUB_FIT exponent={slope:.4} r2={r2:.4} one_pass_480k={:.4}s per_MB_one_pass={per_mb:.3}s", pts[3].1.exp());
}
