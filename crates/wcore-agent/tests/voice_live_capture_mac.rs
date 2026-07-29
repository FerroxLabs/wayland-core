//! Phase 27 C4 — live microphone capture proof on Darwin.
//!
//! # Why this file exists
//!
//! `voice_mode`'s mic-capture loop is behind the off-by-default `voice`
//! feature, so no CI host has ever executed it. Phase 27 graded C4 NOT MET and
//! a lane deferred it as "hardware-blocked: no permitted host has a capture
//! device". The Mac has one. This file is the proof that closes that gap.
//!
//! # The trap this file is built to avoid
//!
//! A prior lane on this programme reported *"audio flowed from a real
//! microphone"* on the evidence of **RMS 5**. RMS 5 is also what a **muted
//! device with dither** produces, and what a **denied-TCC capture path**
//! produces. The claim had to be withdrawn.
//!
//! **Silence and a dead capture path are indistinguishable by amplitude.** So
//! this file does not measure amplitude. It plays a *known 1 kHz tone* and
//! asks whether the captured audio **contains that specific frequency**, using
//! a Goertzel filter. That is a *qualitative* discriminator: broadband noise
//! at any amplitude — including loud noise — has no 1 kHz peak, so a bigger
//! number cannot fake it.
//!
//! Threshold is pre-registered in [`TONE_PRESENT_RATIO`] / [`TONE_ABSENT_RATIO`]
//! below, with its justification, so it cannot be chosen after seeing the data.
//!
//! # Running
//!
//! ```text
//! cargo test -p wcore-agent --features voice --test voice_live_capture_mac
//! ```
//!
//! The acoustic arms need a real speaker→mic path. When no tone is detected
//! the test reports **INDETERMINATE** and fails loudly — it never silently
//! skips, because a silent skip is the self-passing shape this programme keeps
//! being bitten by.

#![cfg(all(feature = "voice", target_os = "macos"))]

use std::f64::consts::PI;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use wcore_agent::tool_backends::voice_mode::CpalAudioRecorder;
use wcore_tools::voice_mode::{AudioRecorder, RecordingOutcome};

// ---------------------------------------------------------------------------
// Pre-registered thresholds — fixed BEFORE any live arm was run.
// ---------------------------------------------------------------------------

/// Frequency injected into the room and searched for in the capture.
const TONE_HZ: f64 = 1000.0;

/// Off-band probe frequencies used as the local noise floor. Deliberately
/// **non-harmonic** with respect to `TONE_HZ` (1000 Hz), so speaker distortion
/// products at 2000/3000 Hz cannot inflate the floor and mask a real peak.
const OFF_BAND_HZ: [f64; 4] = [617.0, 1481.0, 2273.0, 2887.0];

/// `tone_ratio >= 20` ⇒ the capture CONTAINS the injected tone.
///
/// Justification, fixed in advance: `tone_ratio` is the ratio of Goertzel
/// power at 1 kHz to the mean power at four non-harmonic off-band probes. For
/// any broadband source — room noise, mic self-noise, dither on a muted
/// device, or the all-zero buffers a TCC-denied capture yields — the spectrum
/// is flat-ish at these four probes and the expected ratio is ≈ 1. A ratio of
/// 20 is **13 dB** above that floor: far outside broadband variation, and far
/// *below* what an actually-audible tone produces (typically 30–40 dB). The
/// gap between the two thresholds is deliberately left as an explicit
/// INDETERMINATE band rather than being split, so a marginal result is
/// reported as marginal instead of rounded into a claim.
const TONE_PRESENT_RATIO: f64 = 20.0;

/// `tone_ratio <= 3` ⇒ the capture does NOT contain the tone.
const TONE_ABSENT_RATIO: f64 = 3.0;

/// Capture duration for each acoustic arm.
const ARM_SECONDS: u64 = 3;

// ---------------------------------------------------------------------------
// Goertzel tone detector
// ---------------------------------------------------------------------------

/// Normalised Goertzel power of `samples` at `target_hz`.
///
/// Normalised by `n^2` so the value is comparable across captures of
/// different lengths — the ratio below would survive without it, but an
/// absolute power that silently scales with duration is an easy way to
/// mis-read one arm against another.
fn goertzel_power(samples: &[i16], sample_rate: f64, target_hz: f64) -> f64 {
    let n = samples.len();
    if n == 0 {
        return 0.0;
    }
    let nf = n as f64;
    let k = (0.5 + nf * target_hz / sample_rate).floor();
    let w = 2.0 * PI * k / nf;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &x in samples {
        let s0 = coeff * s1 - s2 + f64::from(x);
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - coeff * s1 * s2) / (nf * nf)
}

/// Ratio of tone-band power to the mean off-band power. Dimensionless, so it
/// is invariant to gain: turning the mic up cannot manufacture a peak.
fn tone_ratio(samples: &[i16], sample_rate: f64) -> f64 {
    let tone = goertzel_power(samples, sample_rate, TONE_HZ);
    let floor: f64 = OFF_BAND_HZ
        .iter()
        .map(|&f| goertzel_power(samples, sample_rate, f))
        .sum::<f64>()
        / OFF_BAND_HZ.len() as f64;
    // INSTRUMENT DEFECT #1, found by this file's own self-test and repaired
    // here rather than written up (LANE-BRIEF §6b-ii).
    //
    // The first version returned 0.0 whenever `floor <= EPSILON`, reasoning
    // that a zero floor means an all-zero buffer. That conflated two opposite
    // states. A *mathematically pure* tone evaluated at exact Goertzel bins
    // leaks EXACTLY zero into the other exact bins, so the purest possible
    // positive signal drove the floor to zero and scored **0.0 — the same
    // value as a dead capture path.** The self-test's known-positive caught
    // it; without that assertion the file would have reported "no tone" for
    // every arm and read as a confident, entirely fabricated negative.
    //
    // The repair discriminates on TOTAL signal power, which is zero only for
    // a genuinely silent buffer, and floors the denominator relative to it.
    let n = samples.len() as f64;
    let total: f64 = samples
        .iter()
        .map(|&s| {
            let v = f64::from(s);
            v * v
        })
        .sum::<f64>()
        / n;
    if total <= f64::EPSILON {
        // Digital silence: no tone, and no floor to divide by. 0.0 is correct
        // here, and is never +inf — an infinity would read as "overwhelming
        // tone present" for the deadest possible capture path.
        return 0.0;
    }
    tone / floor.max(total * 1e-9)
}

/// Root-mean-square amplitude — the measurement the withdrawn claim relied on.
/// Computed here **only** so this file can demonstrate that it does not
/// discriminate.
fn rms(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (sum_sq / samples.len() as f64).sqrt()
}

// ---------------------------------------------------------------------------
// Minimal WAV write/read — deliberately dependency-free so the instrument
// itself has no moving parts that could fail silently.
// ---------------------------------------------------------------------------

fn write_wav_mono16(path: &Path, sample_rate: u32, samples: &[i16]) {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, out).expect("write wav");
}

/// Parse a 16-bit PCM WAV by walking the chunk list. Panics with a specific
/// message on every malformed case rather than returning an empty Vec — an
/// empty Vec would flow into the detector and score a confident zero.
fn read_wav_mono16(path: &Path) -> Vec<i16> {
    let bytes = std::fs::read(path).expect("read wav");
    assert!(bytes.len() > 12, "wav too short to hold a RIFF header");
    assert_eq!(&bytes[0..4], b"RIFF", "not a RIFF file");
    assert_eq!(&bytes[8..12], b"WAVE", "not a WAVE file");
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let sz = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body = pos + 8;
        if id == b"data" {
            let end = (body + sz).min(bytes.len());
            assert!(end > body, "wav 'data' chunk is empty");
            return bytes[body..end]
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect();
        }
        pos = body + sz + (sz & 1);
    }
    panic!("wav has no 'data' chunk");
}

fn tone_samples(sample_rate: u32, seconds: f64, hz: f64, amplitude: f64) -> Vec<i16> {
    let n = (sample_rate as f64 * seconds) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(sample_rate);
            (amplitude * (2.0 * PI * hz * t).sin()) as i16
        })
        .collect()
}

/// Deterministic low-amplitude pseudo-noise, shaped to land at a target RMS.
/// This is the "muted device with dither" signal — the thing the withdrawn
/// claim could not tell apart from real speech.
fn dither_samples(n: usize, target_rms: f64) -> Vec<i16> {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    (0..n)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let unit = ((state >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0;
            (unit * target_rms * 1.732) as i16 // uniform → rms = a/sqrt(3)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 1. Instrument self-test — three assertions, per LANE-BRIEF §6b-ii.
// ---------------------------------------------------------------------------

#[test]
fn goertzel_instrument_self_test() {
    let sr = 16_000.0;

    // (1) KNOWN-POSITIVE: a synthetic 1 kHz tone must be detected.
    let pos = tone_samples(16_000, 1.0, TONE_HZ, 8000.0);
    let r_pos = tone_ratio(&pos, sr);
    assert!(
        r_pos >= TONE_PRESENT_RATIO,
        "known-positive failed: synthetic 1kHz tone scored {r_pos:.1}, \
         expected >= {TONE_PRESENT_RATIO}. Instrument is dead — every \
         subsequent 'no tone' result would be a free negative."
    );

    // (2) KNOWN-NEGATIVE: dither at RMS 5 must NOT be detected, and must
    //     score below the absent threshold — not merely lower than (1).
    let neg = dither_samples(16_000, 5.0);
    let r_neg = tone_ratio(&neg, sr);
    assert!(
        r_neg <= TONE_ABSENT_RATIO,
        "known-negative failed: RMS-5 dither scored {r_neg:.1}, expected \
         <= {TONE_ABSENT_RATIO}. The detector is not discriminating."
    );

    // (3) THE OLD MATCHER WOULD HAVE MISSED IT. This is the assertion that
    //     proves the repair does something. The withdrawn claim used RMS.
    //     Build a dither signal whose RMS EXCEEDS the real tone's RMS, so an
    //     RMS-threshold instrument ranks the DEAD signal above the LIVE one,
    //     while the Goertzel instrument separates them cleanly.
    let quiet_tone = tone_samples(16_000, 1.0, TONE_HZ, 60.0); // rms ~42
    let loud_dither = dither_samples(16_000, 400.0); // rms ~400
    let (rms_tone, rms_dither) = (rms(&quiet_tone), rms(&loud_dither));
    let (g_tone, g_dither) = (tone_ratio(&quiet_tone, sr), tone_ratio(&loud_dither, sr));

    eprintln!(
        "SELF-TEST(3): quiet-tone rms={rms_tone:.0} ratio={g_tone:.1} | \
         loud-dither rms={rms_dither:.0} ratio={g_dither:.1}"
    );
    assert!(
        rms_dither > rms_tone,
        "setup invalid: dither must be LOUDER than the tone for this assertion \
         to mean anything (dither {rms_dither:.0} vs tone {rms_tone:.0})"
    );
    assert!(
        g_tone >= TONE_PRESENT_RATIO && g_dither <= TONE_ABSENT_RATIO,
        "the repair is a no-op: Goertzel must rank the quiet REAL tone \
         ({g_tone:.1}) above the loud DEAD dither ({g_dither:.1}), which an \
         RMS threshold provably cannot — RMS ranks them {rms_dither:.0} > {rms_tone:.0}."
    );

    // (4) REGRESSION GUARD for INSTRUMENT DEFECT #1 (see `tone_ratio`).
    //     The original zero-floor guard scored a PURE TONE and DIGITAL
    //     SILENCE identically at 0.0. These two must never be equal again.
    let silence = vec![0i16; 16_000];
    let r_silence = tone_ratio(&silence, sr);
    let r_pure = tone_ratio(&pos, sr);
    assert_eq!(
        r_silence, 0.0,
        "digital silence must score exactly 0.0, got {r_silence}"
    );
    assert!(
        r_pure > TONE_PRESENT_RATIO,
        "pure tone must score above {TONE_PRESENT_RATIO}, got {r_pure}"
    );
    assert!(
        (r_pure - r_silence).abs() > TONE_PRESENT_RATIO,
        "DEFECT #1 HAS REGRESSED: a pure tone ({r_pure}) and digital silence \
         ({r_silence}) are scoring the same. The old guard did exactly this, \
         which would turn every arm of this file into a free negative."
    );
    eprintln!("SELF-TEST(4): pure-tone ratio={r_pure:.3e}  digital-silence ratio={r_silence}");
}

// ---------------------------------------------------------------------------
// 2. Live capture arms.
// ---------------------------------------------------------------------------

struct Arm {
    samples: Vec<i16>,
    ratio: f64,
    rms: f64,
}

/// Capture from the real default input device for `ARM_SECONDS`, optionally
/// playing `tone_wav` into the room concurrently.
async fn capture_arm(label: &str, tone_wav: Option<&Path>) -> Arm {
    let rec = CpalAudioRecorder::try_default()
        .expect("no default input device — this test requires a real capture device");

    rec.start().await.expect("recorder start failed");
    assert!(rec.is_recording(), "recorder did not enter recording state");

    let mut child = tone_wav.map(|w| {
        Command::new("afplay")
            .arg(w)
            .spawn()
            .expect("afplay spawn failed")
    });

    tokio::time::sleep(Duration::from_secs(ARM_SECONDS)).await;

    // Prove the stream was FLOWING before we stop it. "It stopped" is free on
    // a stream that never started.
    let mid_rms = rec.current_rms();

    let outcome = rec.stop().await.expect("recorder stop failed");
    if let Some(c) = child.as_mut() {
        let _ = c.wait();
    }

    let wav = match outcome {
        RecordingOutcome::Captured { wav_path } => wav_path,
        RecordingOutcome::Empty => panic!(
            "arm '{label}': recorder returned Empty — no audio captured at all. \
             Note the product collapses 'too short / silent / cancelled' into \
             this single outcome, so it cannot itself tell you which occurred."
        ),
    };

    let samples = read_wav_mono16(&wav);
    let expected = 16_000 * ARM_SECONDS as usize;
    assert!(
        samples.len() > expected / 2,
        "arm '{label}': captured {} samples, expected ~{expected} \
         (>50% required). The stream was not flowing.",
        samples.len()
    );

    let (ratio, r) = (tone_ratio(&samples, 16_000.0), rms(&samples));
    eprintln!(
        "ARM {label}: samples={} mid_rms={mid_rms} rms={r:.1} tone_ratio={ratio:.2}",
        samples.len()
    );
    let _ = std::fs::remove_file(&wav);
    Arm {
        samples,
        ratio,
        rms: r,
    }
}

/// **The capture-liveness control.** Two arms on the same device, same
/// duration, differing only in whether a known tone was injected.
#[tokio::test(flavor = "multi_thread")]
async fn live_capture_contains_known_tone_and_control_arm_does_not() {
    let dir = std::env::temp_dir();
    let tone_path = dir.join("wl-c4-probe-1khz.wav");
    // 44.1 kHz for the playback file — the output device is whatever the host
    // has selected, and 44.1 is the safest rate for an arbitrary speaker.
    write_wav_mono16(
        &tone_path,
        44_100,
        &tone_samples(44_100, ARM_SECONDS as f64 + 0.5, TONE_HZ, 12000.0),
    );

    // Arm B (control) FIRST, so the room is characterised before anything is
    // played into it. Running it second would let speaker ring-out leak in.
    let silent = capture_arm("B-control-no-tone", None).await;
    let tone = capture_arm("A-live-tone-playing", Some(&tone_path)).await;
    let _ = std::fs::remove_file(&tone_path);

    // Both arms must have delivered real, varying audio. An all-zero buffer
    // (TCC denial) would score rms 0 and ratio 0 in BOTH arms.
    assert!(
        silent.rms > 0.0 && tone.rms > 0.0,
        "one or both arms captured digital silence (control rms={:.1}, \
         tone rms={:.1}) — the capture path is dead, not quiet.",
        silent.rms,
        tone.rms
    );

    eprintln!(
        "DISCRIMINATION: control ratio={:.2} (threshold <= {TONE_ABSENT_RATIO}) | \
         tone ratio={:.2} (threshold >= {TONE_PRESENT_RATIO}) | separation={:.1}x",
        silent.ratio,
        tone.ratio,
        tone.ratio / silent.ratio.max(f64::EPSILON)
    );

    assert!(
        silent.ratio <= TONE_ABSENT_RATIO,
        "CONTROL ARM CONTAMINATED: the no-tone arm scored {:.2} at 1 kHz, \
         above the pre-registered absent threshold {TONE_ABSENT_RATIO}. \
         Something in the room is emitting 1 kHz; the positive arm proves \
         nothing until this is resolved.",
        silent.ratio
    );

    assert!(
        tone.ratio >= TONE_PRESENT_RATIO,
        "INDETERMINATE — NOT a proof of a dead mic. The tone arm scored \
         {:.2}, below the pre-registered {TONE_PRESENT_RATIO}. The capture \
         path delivered {} real samples at rms {:.1}, so the DEVICE is live; \
         what is unproven is the acoustic speaker->mic path (output volume, \
         speaker powered, physical proximity). Report as INDETERMINATE; do \
         not report it as either a live or a dead capture device.",
        tone.ratio,
        tone.samples.len(),
        tone.rms
    );
}

// ---------------------------------------------------------------------------
// 3. Ring-buffer overflow: does capture SURVIVE past the 60 s cap?
// ---------------------------------------------------------------------------

/// `RingBuffer::push` calls `Vec::remove(0)` once the ring is full — an O(n)
/// memmove over 960_000 `i16` (1.92 MB) **per sample**, executed inside the
/// cpal input callback while holding the shared state lock. At 16 kHz that is
/// ~30 GB/s of memmove. Arithmetic alone cannot say whether a given machine
/// survives it, so this measures rather than asserts.
///
/// The discriminating trick: the tone is played **only during the final
/// seconds**, i.e. deep into the overflow regime. If the callback has stalled
/// or is dropping buffers, the retained tail will not contain the tone.
#[tokio::test(flavor = "multi_thread")]
async fn capture_survives_ring_buffer_overflow_past_60s() {
    const TOTAL_SECS: u64 = 70;
    const TONE_TAIL_SECS: f64 = 4.0;

    let dir = std::env::temp_dir();
    let tone_path = dir.join("wl-c4-overflow-1khz.wav");
    write_wav_mono16(
        &tone_path,
        44_100,
        &tone_samples(44_100, TONE_TAIL_SECS, TONE_HZ, 12000.0),
    );

    // INSTRUMENT DEFECT #2, found by running this gate twice and repaired here
    // rather than written up (LANE-BRIEF §6b-ii). The first run FAILED with
    // tail ratio 0.41; an immediate re-run PASSED with 11227.13. A gate that
    // reports a product defect on one run and a clean pass on the next is not
    // measuring the product. The most likely confound is the acoustic path,
    // not the ring buffer: the host's default output is a Bluetooth speaker,
    // which sleeps and misses the first seconds of playback.
    //
    // Two repairs:
    //  (a) a wake-up pre-roll BEFORE capture starts, so the speaker is awake
    //      by the measurement window. It lands at t≈0 of a 70 s capture and is
    //      therefore evicted by the 60 s ring cap, so it cannot contaminate;
    //  (b) the tone is now scored over the WHOLE retained buffer as well as
    //      the tail, which separates the two failure causes that the original
    //      single assertion conflated:
    //        whole LOW  -> the tone never reached the mic: acoustic path
    //                      failed, INDETERMINATE, says nothing about the ring;
    //        whole HIGH but tail LOW -> the tone did reach the mic but is
    //                      absent from the retained tail: a REAL degradation.
    let _ = Command::new("afplay")
        .arg(&tone_path)
        .status()
        .map_err(|e| eprintln!("wake pre-roll failed to spawn: {e}"));
    tokio::time::sleep(Duration::from_millis(500)).await;

    // INSTRUMENT DEFECT #2b. A one-shot pre-roll was not enough: the Bluetooth
    // output sleeps again during the 65 s quiet window and misses the
    // measurement tone, so the gate reported INDETERMINATE in 2 of 3 runs. It
    // was declining to claim a product defect, which is correct behaviour — but
    // a gate that cannot deliver its own precondition is not a gate.
    //
    // Fix: hold the speaker awake for the whole quiet window with a continuous
    // 220 Hz keep-alive. 220 Hz is deliberately OFF-BAND — neither it nor its
    // harmonics (440/660/880/1100/1320…) land within thousands of Goertzel bins
    // of the 1 kHz target or of any of the four off-band probes at a 0.25 Hz
    // bin width — so it keeps the output alive without touching the measurement.
    let keepalive_path = dir.join("wl-c4-keepalive-220hz.wav");
    write_wav_mono16(
        &keepalive_path,
        44_100,
        &tone_samples(44_100, (TOTAL_SECS - 3) as f64, 220.0, 6000.0),
    );

    let rec = CpalAudioRecorder::try_default().expect("no default input device");
    rec.start().await.expect("start failed");
    let mut keepalive = Command::new("afplay")
        .arg(&keepalive_path)
        .spawn()
        .expect("keep-alive spawn failed");

    // Run well past the 60 s cap with NO tone, so the ring saturates and every
    // subsequent sample pays the O(n) cost.
    tokio::time::sleep(Duration::from_secs(TOTAL_SECS - 5)).await;
    let rms_in_overflow = rec.current_rms();
    assert!(
        rec.is_recording(),
        "recorder fell out of recording state during overflow"
    );

    // Now inject the tone, deep inside the overflow regime.
    let mut child = Command::new("afplay")
        .arg(&tone_path)
        .spawn()
        .expect("afplay spawn failed");
    tokio::time::sleep(Duration::from_secs(5)).await;
    let _ = child.wait();
    let _ = keepalive.kill();
    let _ = keepalive.wait();
    let _ = std::fs::remove_file(&keepalive_path);

    let t_stop = std::time::Instant::now();
    let outcome = rec.stop().await.expect("stop failed");
    let stop_elapsed = t_stop.elapsed();
    let _ = std::fs::remove_file(&tone_path);

    let wav = match outcome {
        RecordingOutcome::Captured { wav_path } => wav_path,
        RecordingOutcome::Empty => panic!("70 s capture returned Empty"),
    };
    let samples = read_wav_mono16(&wav);
    let _ = std::fs::remove_file(&wav);

    // The ring is capped at 60 s @ 16 kHz.
    const CAP: usize = 16_000 * 60;
    eprintln!(
        "OVERFLOW: total={TOTAL_SECS}s retained={} samples ({:.1}s) cap={CAP} \
         rms_in_overflow={rms_in_overflow} stop_took={:?}",
        samples.len(),
        samples.len() as f64 / 16_000.0,
        stop_elapsed
    );

    assert!(
        samples.len() <= CAP,
        "ring exceeded its documented 60 s cap: {} > {CAP}",
        samples.len()
    );

    // THE REAL QUESTION: is the retained tail still real, live audio?
    let tail_len = (16_000.0 * TONE_TAIL_SECS) as usize;
    let tail = &samples[samples.len().saturating_sub(tail_len)..];
    let tail_ratio = tone_ratio(tail, 16_000.0);
    let whole_ratio = tone_ratio(&samples, 16_000.0);
    eprintln!(
        "OVERFLOW TAIL: {} samples, rms={:.1}, tail_ratio={tail_ratio:.2}, \
         whole_buffer_ratio={whole_ratio:.2}",
        tail.len(),
        rms(tail)
    );

    // Cause separation — see INSTRUMENT DEFECT #2 above.
    //
    // INSTRUMENT DEFECT #3, found by running the repaired gate and repaired
    // here. The first version of this separation checked `whole_ratio` FIRST
    // and unconditionally, which is wrong: the tone deliberately occupies only
    // TONE_TAIL_SECS of a 60 s retained window, so coherent-gain dilution
    // drives the whole-buffer ratio down by roughly (4/60)^2. Measured
    // whole=1.67 while tail=137.03 — i.e. the gate shouted "the tone never
    // reached the microphone" in the very same run in which the tail proved,
    // at a 6.8x margin over threshold, that it plainly had.
    //
    // `whole_ratio` is only meaningful as a TIE-BREAKER, and only when the
    // tail has already come back low. Correct order:
    //   tail HIGH                -> capture survived overflow. PASS.
    //   tail LOW  + whole LOW    -> tone never arrived: acoustic path failed,
    //                               INDETERMINATE, says nothing about the ring.
    //   tail LOW  + whole HIGH   -> tone arrived but is missing from the tail:
    //                               a REAL degradation.
    if tail_ratio < TONE_PRESENT_RATIO {
        assert!(
            whole_ratio >= TONE_PRESENT_RATIO,
            "INDETERMINATE, NOT a product defect. The injected tone is absent \
             from the retained tail (tail={tail_ratio:.2}) AND from the whole \
             retained buffer (whole={whole_ratio:.2}), so it never reached the \
             microphone — the speaker->mic acoustic path failed (sleeping \
             Bluetooth output, volume, proximity). This run says NOTHING about \
             the ring buffer. Re-run; do not report a capture defect."
        );
        panic!(
            "REAL DEGRADATION PAST THE 60s CAP: the tone DID reach the mic \
             (whole={whole_ratio:.2}) but is absent from the retained tail \
             (tail={tail_ratio:.2} < {TONE_PRESENT_RATIO}). The acoustic path \
             is proven good this run, so the O(n) `Vec::remove(0)` in \
             RingBuffer::push, running inside the cpal callback, is implicated."
        );
    }
}

// ---------------------------------------------------------------------------
// 4. Cancellation on a PROVEN-FLOWING stream.
// ---------------------------------------------------------------------------

/// C4 `cancellation`. Proves the stream was flowing *before* the cancel, so
/// "it stopped" is not free.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_discards_a_stream_proven_to_be_flowing() {
    let rec = CpalAudioRecorder::try_default().expect("no default input device");

    rec.start().await.expect("start failed");
    tokio::time::sleep(Duration::from_millis(1200)).await;

    // PROOF OF FLOW: the ring buffer must hold real audio before we cancel.
    assert!(
        rec.is_recording(),
        "recorder reports not-recording 1.2s after start"
    );
    let flowing_rms = rec.current_rms();
    assert!(
        flowing_rms > 0,
        "PRE-CANCEL FLOW UNPROVEN: current_rms is 0 after 1.2s of capture. \
         Cancelling now would prove nothing — a stream that never started \
         always 'stops'."
    );
    eprintln!("PRE-CANCEL: is_recording=true current_rms={flowing_rms}");

    rec.cancel().await.expect("cancel failed");

    assert!(!rec.is_recording(), "still recording after cancel");
    assert_eq!(
        rec.current_rms(),
        0,
        "cancel did not discard the buffered audio"
    );

    // And the discarded audio must not be recoverable as a WAV.
    let after = rec.stop().await.expect("stop after cancel failed");
    assert_eq!(
        after,
        RecordingOutcome::Empty,
        "stop after cancel produced a recording — audio survived cancellation"
    );
    eprintln!("POST-CANCEL: is_recording=false rms=0 stop=Empty");
}
