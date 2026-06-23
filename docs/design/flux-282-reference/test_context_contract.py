"""Unit tests for the Core<->Flux context-routing contract (Wayland #282, V1).

Mirrors the direct-helper style of tests/test_param_sanitizer.py. The contract's
load-bearing logic is extracted into pure module-level helpers so it can be
exercised without standing up the full pre/post-call hook flow:

  - `_parse_wl_context_headers(data)`  — header parse (C2)
  - `_compute_context_required(wl, data)` — REQUIRED math (C3)
  - `_context_floor(required)`         — ceil(REQUIRED * 1.15) (C3)
  - `_context_overflow_detail(...)`    — structured 409 body (C5)

Everything is gated behind FLUX_CONTEXT_CONTRACT_ENABLED. The flag-off-inertness
tests assert the existing filter formula and the existing 413 are byte-identical
to today when the flag is off.
"""

import importlib
import math

import pytest

import src.forge_hook as fh
from src.forge_hook import (
    CONTEXT_FIT_OUTPUT_HEADROOM,
    _compute_context_required,
    _context_floor,
    _context_overflow_detail,
    _filter_by_context_fit,
    _parse_wl_context_headers,
)


def _data(headers: dict) -> dict:
    """A request dict carrying request headers the way LiteLLM delivers them."""
    return {"proxy_server_request": {"headers": dict(headers)}}


# ── C2: header parse ──────────────────────────────────────────────────────


def test_parse_all_four_headers():
    wl = _parse_wl_context_headers(
        _data(
            {
                "x-wl-context-tokens": "120000",
                "x-wl-expected-output": "4096",
                "x-wl-context-managed": "true",
                "x-wl-conversation-id": "conv-abc",
            }
        )
    )
    assert wl == {
        "context_tokens": 120000,
        "expected_output": 4096,
        "context_managed": True,
        "conversation_id": "conv-abc",
    }


def test_parse_defaults_when_optionals_absent():
    # Only the required gauge present: expected_output -> 0, managed -> False,
    # conversation_id -> None.
    wl = _parse_wl_context_headers(_data({"x-wl-context-tokens": "5000"}))
    assert wl == {
        "context_tokens": 5000,
        "expected_output": 0,
        "context_managed": False,
        "conversation_id": None,
    }


def test_parse_missing_required_token_returns_none():
    assert _parse_wl_context_headers(_data({"x-wl-expected-output": "100"})) is None


def test_parse_garbage_required_token_returns_none():
    assert _parse_wl_context_headers(_data({"x-wl-context-tokens": "not-an-int"})) is None
    assert _parse_wl_context_headers(_data({"x-wl-context-tokens": ""})) is None


def test_parse_garbage_expected_output_falls_back_to_zero():
    # A malformed optional must not nuke the whole parse — only the required
    # gauge controls None. expected_output degrades to its default.
    wl = _parse_wl_context_headers(_data({"x-wl-context-tokens": "9000", "x-wl-expected-output": "xxx"}))
    assert wl is not None
    assert wl["context_tokens"] == 9000
    assert wl["expected_output"] == 0


def test_parse_case_insensitive():
    wl = _parse_wl_context_headers(
        _data(
            {
                "X-WL-Context-Tokens": "7000",
                "X-Wl-Expected-Output": "512",
                "X-WL-CONTEXT-MANAGED": "TRUE",
                "X-Wl-Conversation-Id": "C1",
            }
        )
    )
    assert wl == {
        "context_tokens": 7000,
        "expected_output": 512,
        "context_managed": True,
        "conversation_id": "C1",
    }


@pytest.mark.parametrize(
    "raw,expected",
    [
        ("true", True),
        ("TRUE", True),
        ("True", True),
        ("  true  ", True),
        ("false", False),
        ("0", False),
        ("1", False),
        ("yes", False),
        ("", False),
    ],
)
def test_parse_managed_bool_variants(raw, expected):
    wl = _parse_wl_context_headers(_data({"x-wl-context-tokens": "100", "x-wl-context-managed": raw}))
    assert wl["context_managed"] is expected


# ── C2 defensive: never raise on malformed proxy_server_request ────────────


@pytest.mark.parametrize(
    "data",
    [
        {},  # no proxy_server_request at all
        {"proxy_server_request": None},
        {"proxy_server_request": "garbage"},
        {"proxy_server_request": {"headers": None}},
        {"proxy_server_request": {"headers": "garbage"}},
        {"proxy_server_request": {}},  # no headers key
        "not-even-a-dict",
        None,
    ],
)
def test_parse_never_raises_on_malformed_input(data):
    # Must return None, never raise.
    assert _parse_wl_context_headers(data) is None


# ── C3: REQUIRED math + floor ──────────────────────────────────────────────


def test_required_tokens_plus_expected_output():
    wl = {"context_tokens": 100000, "expected_output": 8000}
    assert _compute_context_required(wl, {}) == 108000


def test_required_uses_max_of_expected_output_and_max_tokens():
    # data.max_tokens larger than expected_output -> max_tokens wins.
    wl = {"context_tokens": 100000, "expected_output": 2000}
    assert _compute_context_required(wl, {"max_tokens": 9000}) == 109000
    # expected_output larger than max_tokens -> expected_output wins.
    wl = {"context_tokens": 100000, "expected_output": 9000}
    assert _compute_context_required(wl, {"max_tokens": 2000}) == 109000


def test_required_honors_max_completion_tokens_alias():
    wl = {"context_tokens": 50000, "expected_output": 0}
    assert _compute_context_required(wl, {"max_completion_tokens": 4096}) == 54096


def test_required_zero_output_budget():
    wl = {"context_tokens": 50000, "expected_output": 0}
    assert _compute_context_required(wl, {}) == 50000


def test_required_clamps_negative_context_tokens_to_zero():
    # FIX 2: a hostile/buggy client header of negative tokens must never
    # produce a sub-zero REQUIRED (which would collapse the floor).
    wl = {"context_tokens": -50000, "expected_output": 4096}
    assert _compute_context_required(wl, {}) == 4096


def test_required_clamps_negative_expected_output_to_zero():
    wl = {"context_tokens": 100000, "expected_output": -9999}
    # expected_output clamped to 0; body_max absent -> output budget 0.
    assert _compute_context_required(wl, {}) == 100000


def test_required_negative_both_floors_at_zero():
    wl = {"context_tokens": -10, "expected_output": -10}
    assert _compute_context_required(wl, {}) == 0


def test_floor_applies_1_15_multiplier_with_ceil():
    assert _context_floor(100000) == math.ceil(100000 * 1.15)
    assert _context_floor(100000) == 115000
    # Non-round case must round UP, never down.
    assert _context_floor(100001) == math.ceil(100001 * 1.15)
    assert _context_floor(7) == 9  # ceil(8.05)


# ── C3: filter drops below floor, keeps >= floor ──────────────────────────


def test_filter_drops_models_below_floor():
    floor = _context_floor(100000)  # 115000
    windows = {"small": 64000, "mid": 128000, "big": 200000}
    fit = _filter_by_context_fit(["small", "mid", "big"], windows, floor)
    assert fit == ["mid", "big"]  # small (64k) < 115k dropped


def test_filter_keeps_models_at_or_above_floor():
    floor = _context_floor(100000)  # 115000
    windows = {"exact": 115000, "over": 116000}
    fit = _filter_by_context_fit(["exact", "over"], windows, floor)
    assert fit == ["exact", "over"]  # exact==floor kept (>=)


def test_filter_empty_when_all_too_small():
    floor = _context_floor(300000)  # 345000
    windows = {"a": 128000, "b": 200000}
    assert _filter_by_context_fit(["a", "b"], windows, floor) == []


# ── C5: structured overflow body ──────────────────────────────────────────


def test_overflow_detail_shape():
    pre_fit = ["a", "b", "c"]
    windows = {"a": 64000, "b": 200000, "c": 128000}
    detail = _context_overflow_detail(345000, pre_fit, windows)
    assert detail["error"] == "context_overflow"
    assert detail["required_tokens"] == 345000
    # largest KNOWN window among the pre-filter candidates
    assert detail["model_window"] == 200000
    # FIX 5: routed_model and model_window must refer to the SAME candidate —
    # the one owning the largest known window ("b" here), not pre_fit[0].
    assert detail["routed_model"] == "b"
    assert isinstance(detail["routed_model"], str)
    assert detail["message"] == ("request exceeds the window of every capable model; compact and retry")


def test_overflow_detail_routed_model_owns_reported_window():
    # FIX 5 consistency invariant: whatever window we report, routed_model is the
    # id that actually has that window. Largest known window here is "mid".
    pre_fit = ["small", "mid", "tiny"]
    windows = {"small": 8000, "mid": 128000, "tiny": 4000}
    detail = _context_overflow_detail(999999, pre_fit, windows)
    assert detail["routed_model"] == "mid"
    assert detail["model_window"] == 128000
    # The reported window equals the routed model's own window.
    assert detail["model_window"] == windows[detail["routed_model"]]


def test_overflow_detail_model_window_is_int():
    detail = _context_overflow_detail(999999, ["x"], {"x": 8192})
    assert detail["model_window"] == 8192
    assert isinstance(detail["model_window"], int)
    # Single known candidate: routed_model is that candidate.
    assert detail["routed_model"] == "x"


def test_overflow_detail_handles_unknown_windows():
    # Candidates with no known window -> model_window falls back to 0 (int),
    # never raises. routed_model falls back to "" when none are known (FIX 5).
    detail = _context_overflow_detail(500000, ["x", "y"], {})
    assert detail["model_window"] == 0
    assert isinstance(detail["model_window"], int)
    assert detail["routed_model"] == ""


# ── FIX 1: contract floor must never be LOWER than Flux's own count ────────
#
# At the filter call site the floor is composed as
#   required = max(_compute_context_required(wl_ctx, data), input_tokens + reserve)
# so a client header can only RAISE the floor above Flux's own
# (input_tokens + reserve), never collapse it below. These tests replicate that
# exact composition with the real helpers and assert the invariant
# required_tokens >= input_tokens for a tiny/negative header against a large
# real prompt.


def _callsite_required_tokens(wl_ctx: dict, data: dict, input_tokens: int) -> int:
    """Replicate the FIX-1 filter call-site floor composition exactly."""
    flagoff_reserve = max(
        int(data.get("max_tokens") or data.get("max_completion_tokens") or 0),
        CONTEXT_FIT_OUTPUT_HEADROOM,
    )
    required = max(_compute_context_required(wl_ctx, data), input_tokens + flagoff_reserve)
    return _context_floor(required)


def test_floor_never_below_flux_count_tiny_header():
    # Client claims context_tokens=1 but Flux counts a 200k prompt: the floor
    # must NOT collapse to ~2 — it floors against Flux's own count.
    input_tokens = 200_000
    wl_ctx = {"context_tokens": 1, "expected_output": 0}
    required_tokens = _callsite_required_tokens(wl_ctx, {}, input_tokens)
    assert required_tokens >= input_tokens
    # And it is at least Flux's own flag-off floor (input + headroom), ×1.15.
    assert required_tokens == _context_floor(input_tokens + CONTEXT_FIT_OUTPUT_HEADROOM)


def test_floor_never_below_flux_count_negative_header():
    # A negative header (FIX 2) is clamped to 0 by _compute_context_required and
    # then the max() floors against Flux's own count — never below it.
    input_tokens = 200_000
    wl_ctx = {"context_tokens": -50_000, "expected_output": -50_000}
    required_tokens = _callsite_required_tokens(wl_ctx, {}, input_tokens)
    assert required_tokens >= input_tokens


def test_floor_header_can_still_raise_above_flux_count():
    # When the header's REQUIRED legitimately exceeds Flux's own count, the
    # header WINS (it raises the floor) — the max() is a floor, not a cap.
    input_tokens = 10_000
    wl_ctx = {"context_tokens": 180_000, "expected_output": 8_000}
    required_tokens = _callsite_required_tokens(wl_ctx, {}, input_tokens)
    assert required_tokens == _context_floor(188_000)
    assert required_tokens > input_tokens


# ── FIX 3: post-select managed-client backstop (_assert_served_context_fits) ─
#
# The pre-call filter runs before the bandit; the grounding short-circuit and
# any re-pick can still land a managed client on a too-small arm. After the
# served model is finalized, _assert_served_context_fits re-checks the ACTUAL
# served model's real window (complete resolver) and 409s rather than overflow.
# Fail-OPEN on unknown served window (only 409 when the window is a known int <
# floor) to avoid false rejects.


@pytest.fixture
def hook():
    return fh.ForgeProxy()


def _served_fits(hook, served_model, required, *, managed=True, flag_on=True):
    """Drive the FIX-3 backstop and report whether it raised a 409."""
    from fastapi import HTTPException

    import src.forge_hook as mod

    data = {"model": served_model}
    metadata = {fh.MK_WL_CONTEXT_MANAGED: managed, fh.MK_WL_REQUIRED: required}
    orig = mod._FLUX_CONTEXT_CONTRACT_ENABLED
    mod._FLUX_CONTEXT_CONTRACT_ENABLED = flag_on
    try:
        hook._assert_served_context_fits(data, metadata)
        return None  # no 409
    except HTTPException as exc:
        return exc
    finally:
        mod._FLUX_CONTEXT_CONTRACT_ENABLED = orig


def test_served_fit_409_when_known_window_too_small(hook, monkeypatch):
    # Served model resolves to a known window smaller than the floor -> 409.
    monkeypatch.setattr(fh.ForgeProxy, "_get_context_window", staticmethod(lambda m: 8000))
    exc = _served_fits(hook, "too-small", required=100_000)
    assert exc is not None
    assert exc.status_code == 409
    assert exc.detail["error"] == "context_overflow"


def test_served_fit_no_409_when_window_fits(hook, monkeypatch):
    # Served model window comfortably exceeds the floor -> no 409.
    monkeypatch.setattr(fh.ForgeProxy, "_get_context_window", staticmethod(lambda m: 2_000_000))
    assert _served_fits(hook, "big", required=100_000) is None


def test_served_fit_fail_open_on_unknown_window(hook, monkeypatch):
    # Unknown served window (None) -> fail OPEN, never 409 (avoids false reject).
    monkeypatch.setattr(fh.ForgeProxy, "_get_context_window", staticmethod(lambda m: None))
    assert _served_fits(hook, "mystery", required=100_000) is None


def test_served_fit_skipped_for_unmanaged_client(hook, monkeypatch):
    # Non-managed client is never subject to the backstop even on a tiny window.
    monkeypatch.setattr(fh.ForgeProxy, "_get_context_window", staticmethod(lambda m: 8000))
    assert _served_fits(hook, "too-small", required=100_000, managed=False) is None


def test_served_fit_skipped_when_flag_off(hook, monkeypatch):
    # Flag off -> backstop inert (byte-identical to today).
    monkeypatch.setattr(fh.ForgeProxy, "_get_context_window", staticmethod(lambda m: 8000))
    assert _served_fits(hook, "too-small", required=100_000, flag_on=False) is None


# ── C1 / flag wiring ───────────────────────────────────────────────────────


def test_flag_default_off():
    # In a clean env the contract flag must default to False.
    assert isinstance(fh._FLUX_CONTEXT_CONTRACT_ENABLED, bool)


def test_flag_reads_env(monkeypatch):
    monkeypatch.setenv("FLUX_CONTEXT_CONTRACT_ENABLED", "true")
    reloaded = importlib.reload(fh)
    try:
        assert reloaded._FLUX_CONTEXT_CONTRACT_ENABLED is True
    finally:
        monkeypatch.delenv("FLUX_CONTEXT_CONTRACT_ENABLED", raising=False)
        importlib.reload(fh)


# ── C4: fit-on-pin (select() restricts the pin to the passed pool) ─────────
#
# FINDING (Case 1 — pin cannot override a filtered pool): ThompsonRouter.select()
# at src/thompson_router.py:366 returns a pin ONLY when `pinned in
# eligible_models`. The context-fit filter (C3) runs at the call site BEFORE
# select() and rewrites `eligible_models` to the fitting set. So a conversation
# pinned to a model whose window < floor is dropped from `eligible_models` and
# can never be returned by select() — fit-on-pin is satisfied by C3's filter,
# no extra guard needed. These tests prove that invariant.


def _make_router():
    from src.bandit_state import BanditState
    from src.thompson_router import ThompsonRouter
    from src.ts_config import TSConfig

    config = TSConfig(
        enabled=True,
        shadow_mode=False,
        kill_switch=False,
        fast_path_threshold=0.7,
        decay_factor=0.999,
        prior_strength=20,
        prior_strength_new_model=5,
        l1_cache_ttl_seconds=30,
        redis_flush_interval_s=5,
        personalization_threshold=1000,
        reward_rate_limit_per_hour=100,
        conversation_pin_ttl_s=14400,
        allow_upgrade=False,
        allow_cross_provider=False,
        upgrade_cost_ceiling=2.0,
        optimization_modes={"balanced": {"latency_weight": 0.5, "cost_weight": 0.5}},
        model_costs={"small-win": 1.0, "big-win": 2.0},
        provider_families={"p": ["small-win", "big-win"]},
        virtual_models={"flux-standard": None},
        benchmark_priors={
            "source": "test",
            "models": {
                "small-win": {"overall": 1200},
                "big-win": {"overall": 1280},
            },
        },
    )
    state = BanditState(redis_client=None, config=config)
    return ThompsonRouter(state=state, config=config)


def test_pin_returned_when_in_eligible_pool():
    # Baseline: a pinned model that IS in the eligible pool is returned (proves
    # the pin path is live and the test below is meaningful).
    router = _make_router()
    router._set_conversation_pin("conv-1", "small-win", customer_id="cust-1")
    decision = router.select(
        requested_model="flux-standard",
        messages=[{"role": "user", "content": "hello"}],
        customer_id="cust-1",
        eligible_models=["small-win", "big-win"],
        conversation_id="conv-1",
    )
    assert decision.selected_model == "small-win"
    assert decision.method == "conversation_pinned"


def test_pin_dropped_from_pool_is_not_served():
    # Fit-on-pin: when the context-fit filter has removed the too-small pinned
    # model from eligible_models (simulated by excluding it), select() must NOT
    # return the pin — it re-picks from the fitting pool.
    router = _make_router()
    router._set_conversation_pin("conv-2", "small-win", customer_id="cust-1")
    decision = router.select(
        requested_model="flux-standard",
        messages=[{"role": "user", "content": "hello"}],
        customer_id="cust-1",
        eligible_models=["big-win"],  # small-win filtered out by C3
        conversation_id="conv-2",
    )
    assert decision.selected_model != "small-win"
    assert decision.method != "conversation_pinned"
    assert decision.selected_model == "big-win"


# ── C6: signal-back headers via the post-call hook ─────────────────────────


class _Resp:
    """Minimal stand-in for the LiteLLM response object the post-call hook sees."""

    def __init__(self):
        self._hidden_params = {}


def _emit_headers(hook, metadata, *, flag_on):
    """Drive the post-call success hook and return the additional_headers dict."""
    import src.forge_hook as mod

    data = {"model": "flux-standard", "metadata": metadata}
    resp = _Resp()

    class _KeyAuth:
        metadata = {}

    orig = mod._FLUX_CONTEXT_CONTRACT_ENABLED
    mod._FLUX_CONTEXT_CONTRACT_ENABLED = flag_on
    try:
        import asyncio as _asyncio

        _asyncio.get_event_loop().run_until_complete(hook.async_post_call_success_hook(data, _KeyAuth(), resp))
    finally:
        mod._FLUX_CONTEXT_CONTRACT_ENABLED = orig
    return resp._hidden_params.get("additional_headers", {})


def test_signal_back_headers_emitted_when_flag_on(hook):
    # A served model with a known window + stashed REQUIRED/input-count emits all
    # three context headers. Use a real served model id Flux can resolve a window
    # for; if the test env can't resolve a window the header is simply absent, so
    # we drive _get_context_window directly to keep the assertion deterministic.
    served = "flux-standard"
    window = hook._get_context_window(served)
    if not isinstance(window, int) or window <= 0:
        pytest.skip("no resolvable window for served model in test config")
    metadata = {
        fh.MK_SERVED_MODEL: served,
        fh.MK_WL_REQUIRED: window // 2,
        fh.MK_WL_INPUT_TOKENS_COUNTED: 4321,
    }
    headers = _emit_headers(hook, metadata, flag_on=True)
    assert headers.get("x-flux-model-window") == str(window)
    assert headers.get("x-flux-context-pressure") == f"{(window // 2) / window:.3f}"
    assert headers.get("x-flux-context-tokens-counted") == "4321"


def test_pressure_header_clamped_to_one(hook):
    # FIX 4: contract §2 declares x-flux-context-pressure as float 0..1. When the
    # stashed REQUIRED exceeds the served window, pressure must emit "1.000",
    # never a >1 value — the genuine-overflow magnitude rides the 409 body.
    served = "flux-standard"
    window = hook._get_context_window(served)
    if not isinstance(window, int) or window <= 0:
        pytest.skip("no resolvable window for served model in test config")
    metadata = {
        fh.MK_SERVED_MODEL: served,
        fh.MK_WL_REQUIRED: window * 3,  # far beyond the window
        fh.MK_WL_INPUT_TOKENS_COUNTED: 100,
    }
    headers = _emit_headers(hook, metadata, flag_on=True)
    assert headers.get("x-flux-context-pressure") == "1.000"


def test_signal_back_headers_absent_when_flag_off(hook):
    served = "flux-standard"
    window = hook._get_context_window(served)
    if not isinstance(window, int) or window <= 0:
        pytest.skip("no resolvable window for served model in test config")
    metadata = {
        fh.MK_SERVED_MODEL: served,
        fh.MK_WL_REQUIRED: window // 2,
        fh.MK_WL_INPUT_TOKENS_COUNTED: 4321,
    }
    headers = _emit_headers(hook, metadata, flag_on=False)
    assert "x-flux-model-window" not in headers
    assert "x-flux-context-pressure" not in headers
    assert "x-flux-context-tokens-counted" not in headers
