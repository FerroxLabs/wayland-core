import io, sys, re
p = "crates/wcore-agent/src/engine.rs"
s = io.open(p, encoding="utf-8").read()

anchor = """    /// Fire test + the band clamp: an out-of-band trigger (0.95) is clamped to
    /// 0.70; a fraction at/above the clamped trigger fires once and disarms."""
assert s.count(anchor) == 1, s.count(anchor)

new = '''    /// REPRO #1172 c3 — the headline. With a CORROBORATED 4,096-token served
    /// window, core must not size the session against 32,768/128,000.
    #[test]
    fn repro_1172_c3_core_does_not_size_against_8x_the_served_window() {
        let mut engine = make_engine();
        engine.model = "gpt-4o".into();
        engine
            .compact_state
            .served_window
            .observe("openai/gpt-4o", 12_000, 4_096);
        assert_eq!(
            engine.compact_state.served_window.sizing_window(),
            Some(4_096),
            "corroborated, or this arm measures nothing"
        );
        let ctx = engine.resolve_preflight_window(1_000, "gpt-4o");
        assert_eq!(
            ctx.window,
            Some(4_096),
            "core must never size against a window the endpoint has been \\
             observed NOT to serve"
        );
    }

    /// REPRO #1179 / D36 — a CONFIGURED window below the minimum workable one
    /// must not summarize an empty conversation at the top of every turn.
    #[test]
    fn repro_1179_d36_a_configured_unworkable_window_does_not_autocompact_an_empty_turn() {
        let mut engine = make_engine();
        engine.compact_config.context_window = Some(6_000);
        assert!(
            !engine.should_autocompact_now(0),
            "an empty conversation must never trigger the summarizer"
        );
        assert!(
            !engine.should_autocompact_now(
                wcore_config::compact::BASELINE_TURN_TOKENS as u64
            ),
            "core's own baseline turn must never trigger the summarizer"
        );
    }

'''
s = s.replace(anchor, new + anchor, 1)
io.open(p, "w", encoding="utf-8").write(s)
print("ok")
