//! 27-C4 — drive the REAL audio capture path and say what happened.
//!
//! Criterion 4 asks whether streaming voice supports interruption,
//! cancellation, compatibility, accounting and ordered protocol events. The
//! phase verdict recorded that no audio ever flowed on any machine, and named
//! that an execution shortfall rather than an environmental impossibility.
//!
//! This is a **live probe**, deliberately an example rather than a test:
//!
//! * It needs real capture hardware, so it cannot be a suite member without
//!   either failing spuriously on headless hosts or being `#[ignore]`d — and
//!   an `#[ignore]`d suite is exactly the self-passing gate class this
//!   program keeps finding (a binary here printed `test result: ok` having
//!   run 0 of 12).
//! * As an example it is explicit: you run it, it tells you what the machine
//!   can actually do, and its exit status means something.
//!
//! It needs NO credential. `build_voice_mode_backend` states the split
//! itself: "no STT backend configured — capture works, transcribe will
//! error". Capture is the half that is reachable without a paid key, and it
//! is the half that answers "did audio ever flow".
//!
//! Build and run (the `voice` feature is OFF by default — see the exit codes
//! below, which is itself the point):
//!
//! ```text
//! cargo run -p wcore-agent --features voice --example f27_voice_capture
//! ```
//!
//! Exit codes, distinct so a caller can tell the cases apart rather than
//! reading one bit:
//!
//! | code | meaning |
//! |---|---|
//! | 0 | audio captured; a WAV exists and its byte count is printed |
//! | 2 | the `voice` feature is not compiled in |
//! | 3 | the feature is on but cpal bound no default input device |
//! | 4 | capture ran but produced no samples / no file — a real failure |
//! | 5 | cancellation did not leave the recorder in a clean state |

#[cfg(not(feature = "voice"))]
fn main() {
    // This branch is what a DEFAULT build of this workspace does, and it is a
    // finding in its own right: `wcore-cli`'s default feature set is
    // ["remote-registry", "workflow", "monitor", "review_artifact"], and
    // `.github/workflows/release.yml` builds `cargo build --release -p
    // wcore-cli` with no `--features voice`. Every shipped release artifact
    // therefore contains no voice_mode tool at all.
    eprintln!(
        "F27_VOICE=FEATURE_OFF the `voice` feature is not compiled in. \
         This is the DEFAULT for this workspace and for every shipped release \
         artifact: wcore-cli's default features do not include `voice`, and \
         release.yml builds without --features voice. Rebuild with \
         `--features voice` to reach the capture path."
    );
    std::process::exit(2);
}

#[cfg(feature = "voice")]
fn main() {
    use std::time::Duration;

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let code = rt.block_on(async {
        use wcore_agent::tool_backends::voice_mode::CpalAudioRecorder;
        use wcore_tools::voice_mode::AudioRecorder;

        // `CpalAudioRecorder::try_default()` is the exact resolver
        // `build_voice_mode_backend` calls first; returning None here is
        // precisely the condition under which the whole voice_mode tool
        // hides itself. Driving it directly keeps this probe honest about
        // WHICH step failed, and adds no accessor to production code.
        let Some(recorder) = CpalAudioRecorder::try_default() else {
            eprintln!(
                "F27_VOICE=NO_INPUT_DEVICE cpal bound no default input device on this \
                 host. The voice_mode tool hides itself here (is_available() == false). \
                 This is the honest-degradation path, not a crash."
            );
            return 3;
        };
        println!("F27_VOICE=BACKEND_BOUND cpal bound a default input device");
        let recorder: &dyn AudioRecorder = &recorder;

        // --- capture leg -------------------------------------------------
        if let Err(e) = recorder.start().await {
            eprintln!("F27_VOICE=START_FAILED {e}");
            return 4;
        }
        println!("F27_VOICE=RECORDING started, capturing for 3s");
        tokio::time::sleep(Duration::from_secs(3)).await;
        let rms_during = recorder.current_rms();
        println!("F27_VOICE=RMS_DURING {rms_during}");

        let outcome = match recorder.stop().await {
            Ok(o) => o,
            Err(e) => {
                eprintln!("F27_VOICE=STOP_FAILED {e}");
                return 4;
            }
        };

        match &outcome {
            wcore_tools::voice_mode::RecordingOutcome::Captured { wav_path } => {
                let bytes = std::fs::metadata(wav_path).map(|m| m.len()).unwrap_or(0);
                println!("F27_VOICE=WAV_PATH {}", wav_path.display());
                println!("F27_VOICE=WAV_BYTES {bytes}");
                // A WAV header alone is 44 bytes. Anything at or below that
                // is a container with no audio in it, which must NOT be
                // reported as "audio flowed".
                if bytes <= 44 {
                    eprintln!("F27_VOICE=EMPTY_CAPTURE only {bytes} bytes — header only, no samples");
                    return 4;
                }
            }
            other => {
                eprintln!("F27_VOICE=NO_CAPTURE outcome was {other:?}");
                return 4;
            }
        }

        // --- cancellation leg --------------------------------------------
        // Cancellation is one of Criterion 4's five named clauses and, unlike
        // interruption, it needs no transcription and therefore no credential.
        if let Err(e) = recorder.start().await {
            eprintln!("F27_VOICE=RESTART_FAILED {e}");
            return 5;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        if !recorder.is_recording() {
            eprintln!("F27_VOICE=NOT_RECORDING_BEFORE_CANCEL");
            return 5;
        }
        if let Err(e) = recorder.cancel().await {
            eprintln!("F27_VOICE=CANCEL_FAILED {e}");
            return 5;
        }
        if recorder.is_recording() {
            eprintln!("F27_VOICE=STILL_RECORDING_AFTER_CANCEL");
            return 5;
        }
        println!("F27_VOICE=CANCELLED cleanly, recorder idle");

        // Cancel must be idempotent from any state — call it again on an
        // already-idle recorder and require it not to error.
        if let Err(e) = recorder.cancel().await {
            eprintln!("F27_VOICE=CANCEL_NOT_IDEMPOTENT {e}");
            return 5;
        }
        println!("F27_VOICE=CANCEL_IDEMPOTENT second cancel on an idle recorder is a no-op");

        0
    });

    println!("F27_VOICE_RC={code}");
    std::process::exit(code);
}
