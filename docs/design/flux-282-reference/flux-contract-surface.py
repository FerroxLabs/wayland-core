"""Flux #282 context-routing contract — IMPLEMENTATION REFERENCE (Flux side, V1).

This is the FROZEN handshake surface extracted from the live Flux proxy
(src/forge_hook.py @ master eb6a6b2). It is the exact code that parses Core's
x-wl-* request headers, computes the full-fit floor, raises the structured 409
context_overflow, and emits the x-flux-* signal-back headers. The full proxy
routing/bandit logic around it is intentionally omitted (private + irrelevant to
the handshake). See test_context_contract.py for behavior-by-example. Not runnable
standalone — reference only.
"""

# ============================================================================
# 1) HELPER FUNCTIONS (module-level)
# ============================================================================
# ── Wayland #282 context-routing contract helpers (V1) ─────────────────────────
# Pure functions so the contract's load-bearing math/parse is directly testable
# without standing up the full pre/post-call hook. All gated by the caller behind
# _FLUX_CONTEXT_CONTRACT_ENABLED — these helpers themselves never read the flag.


def _parse_wl_context_headers(data) -> dict | None:
    """Parse Core's x-wl-* context gauge from the request headers (contract §2).

    Reads ``data["proxy_server_request"]["headers"]`` (LiteLLM builds this dict
    with LOWERCASED keys, but we lower-case defensively in case a future caller
    passes mixed-case). Returns a normalized dict, or ``None`` when the required
    ``x-wl-context-tokens`` is absent or unparseable (caller then falls back to
    body token-counting, exactly as today).

    Fully defensive: NEVER raises — any malformed input yields ``None``. A
    malformed OPTIONAL header degrades to its default without nulling the parse;
    only the required gauge controls ``None``.

    Returned shape::

        {"context_tokens": int, "expected_output": int,
         "context_managed": bool, "conversation_id": str | None}
    """
    try:
        psr = data.get("proxy_server_request") if isinstance(data, dict) else None
        headers = psr.get("headers") if isinstance(psr, dict) else None
        if not isinstance(headers, dict):
            return None
        # Case-insensitive lookup: index by lower-cased key.
        lower = {}
        for k, v in headers.items():
            try:
                lower[str(k).lower()] = v
            except Exception:
                continue

        raw_tokens = lower.get("x-wl-context-tokens")
        if raw_tokens is None:
            return None
        try:
            context_tokens = int(str(raw_tokens).strip())
        except (TypeError, ValueError):
            return None

        try:
            expected_output = int(str(lower.get("x-wl-expected-output", "")).strip())
        except (TypeError, ValueError):
            expected_output = 0

        context_managed = str(lower.get("x-wl-context-managed", "")).strip().lower() == "true"

        conv_raw = lower.get("x-wl-conversation-id")
        conversation_id = str(conv_raw) if conv_raw not in (None, "") else None

        return {
            "context_tokens": context_tokens,
            "expected_output": expected_output,
            "context_managed": context_managed,
            "conversation_id": conversation_id,
        }
    except Exception:
        return None


def _compute_context_required(wl: dict, data: dict) -> int:
    """REQUIRED = context_tokens + max(expected_output, request max_tokens) (§3.1).

    The output budget is the larger of Core's declared ``expected_output`` and
    the request body's own ``max_tokens`` / ``max_completion_tokens`` — whichever
    Core under-declared still gets room.
    """
    # FIX 2: clamp negatives — a hostile/buggy client header must never drive
    # REQUIRED below zero (which would collapse the routing floor).
    context_tokens = max(int(wl.get("context_tokens") or 0), 0)
    expected_output = max(int(wl.get("expected_output") or 0), 0)
    body_max = int((data.get("max_tokens") or data.get("max_completion_tokens") or 0) if isinstance(data, dict) else 0)
    return context_tokens + max(expected_output, body_max)


def _context_floor(required: int) -> int:
    """The full-fit floor: ceil(REQUIRED × 1.15) (contract §3.3 headroom)."""
    return math.ceil(required * _CONTEXT_CONTRACT_HEADROOM)


def _context_overflow_detail(required: int, pre_fit: list[str], context_windows: dict) -> dict:
    """Structured 409 ``context_overflow`` body for a managed client (§2, C5).

    ``model_window`` is the largest KNOWN window among the pre-filter candidates
    (0 when none are known — never raises). ``routed_model`` is the id of THAT
    same candidate (FIX 5: window + id must be consistent), "" when none known.
    """
    known = [(m, context_windows.get(m)) for m in pre_fit if isinstance(context_windows.get(m), int)]
    if known:
        routed_model, model_window = max(known, key=lambda pair: pair[1])
    else:
        routed_model, model_window = "", 0
    return {
        "error": "context_overflow",
        "required_tokens": int(required),
        "model_window": int(model_window),
        "routed_model": str(routed_model),
        "message": "request exceeds the window of every capable model; compact and retry",
    }



# ============================================================================
# 2) PRE-CALL FILTER CALL SITE — REQUIRED, x1.15 floor, managed-overflow 409
#    (inside async pre-call hook, after the eligible-model pool is built;
#     wl_ctx is None => flag off or no headers => today's additive path)
# ============================================================================
                        wl_ctx = None
                        if _FLUX_CONTEXT_CONTRACT_ENABLED and is_tier_alias(model):
                            wl_ctx = _parse_wl_context_headers(data)

                        # Token counting is CPU-bound — keep it off the event
                        # loop (mirrors the select() to_thread call below).
                        input_tokens = await asyncio.to_thread(_estimate_token_count, messages, "")

                        if wl_ctx is not None:
                            # FIX 1: the header can only RAISE the floor above
                            # Flux's own authoritative count, never lower it. A
                            # client claiming context_tokens=1 on a real 200k
                            # prompt must not collapse the floor and route the
                            # huge prompt onto the smallest arm (strictly less
                            # safe than flag-off). Floor the header-derived
                            # REQUIRED against (input_tokens + flag-off reserve).
                            _flagoff_reserve = max(
                                int(data.get("max_tokens") or data.get("max_completion_tokens") or 0),
                                CONTEXT_FIT_OUTPUT_HEADROOM,
                            )
                            required = max(
                                _compute_context_required(wl_ctx, data),
                                input_tokens + _flagoff_reserve,
                            )
                            required_tokens = _context_floor(required)
                            # Stash for the signal-back headers (C6): REQUIRED and
                            # Flux's own authoritative input count.
                            metadata[MK_WL_CONTEXT_TOKENS] = wl_ctx["context_tokens"]
                            metadata[MK_WL_EXPECTED_OUTPUT] = wl_ctx["expected_output"]
                            metadata[MK_WL_CONTEXT_MANAGED] = wl_ctx["context_managed"]
                            metadata[MK_WL_CONVERSATION_ID] = wl_ctx["conversation_id"]
                            metadata[MK_WL_REQUIRED] = required
                            metadata[MK_WL_INPUT_TOKENS_COUNTED] = input_tokens
                        else:
                            # Reserve output room within the model's window: the
                            # request's own max_tokens when set, else a sane floor.
                            reserve = max(
                                int(data.get("max_tokens") or data.get("max_completion_tokens") or 0),
                                CONTEXT_FIT_OUTPUT_HEADROOM,
                            )
                            required_tokens = input_tokens + reserve

                        pre_fit = list(eligible_models)
                        fit = _filter_by_context_fit(pre_fit, ctx_windows, required_tokens)
                        if fit and len(fit) < len(pre_fit):
                            dropped = len(pre_fit) - len(fit)
                            logger.info(
                                "context_fit filtered %d/%d eligible models (need~%d tokens)",
                                dropped,
                                len(pre_fit),
                                required_tokens,
                            )
                            from src.flux_metrics import flux_context_fit_filter_total  # noqa: PLC0415

                            flux_context_fit_filter_total.labels(result="dropped").inc(dropped)
                            eligible_models = fit
                        elif not fit:
                            # Every eligible model has a known too-small window.
                            # _filter_by_context_fit fails open on unknown
                            # windows, so this is reachable only when every
                            # candidate window is known AND too small — a real
                            # context-limit error, not a coverage gap.
                            from src.flux_metrics import flux_context_fit_filter_total  # noqa: PLC0415

                            flux_context_fit_filter_total.labels(result="all_too_small").inc()
                            logger.warning(
                                "context_fit: all %d eligible models too small for ~%d tokens — rejecting",
                                len(pre_fit),
                                required_tokens,
                            )
                            if wl_ctx is not None and wl_ctx["context_managed"]:
                                # Managed client: never silently truncate. Return
                                # the structured 409 context_overflow (contract §2)
                                # so Core compacts-then-retries this turn.
                                raise HTTPException(
                                    status_code=409,
                                    detail=_context_overflow_detail(
                                        metadata.get(MK_WL_REQUIRED, required_tokens),
                                        pre_fit,
                                        ctx_windows,
                                    ),
                                )
                            raise HTTPException(
                                status_code=413,
                                detail={
                                    "error": "context_window_exceeded",
                                    "reason": (
                                        f"Request needs ~{required_tokens} tokens (input + "
                                        f"output headroom) but exceeds the context window of "
                                        f"every model eligible for this tier."
                                    ),
                                    "required_tokens": required_tokens,
                                },
                            )
                except HTTPException:
                    raise
                except Exception:
                    logger.debug("context_fit filter failed", exc_info=True)


# ============================================================================
# 3) POST-SELECT BACKSTOP — managed client never served onto a too-small arm
# ============================================================================
    def _assert_served_context_fits(self, data: dict, metadata: dict) -> None:
        """Wayland #282 (FIX 3): managed-client post-select fit backstop.

        The pre-call context-fit filter runs BEFORE the bandit; the grounding
        short-circuit and any post-filter re-pick can still land a managed client
        on a too-small arm, silently overflowing the contract's 409. After the
        served model is finalized, re-check the ACTUAL served model's real window
        (the COMPLETE resolver) and refuse with the structured 409 rather than
        dispatch an overflow.

        FAIL-OPEN on an unknown served window (only 409 when the window is a
        known int < floor) to avoid false rejects. Gated by the contract flag
        and the managed marker — inert otherwise (byte-identical to today).
        """
        if not (_FLUX_CONTEXT_CONTRACT_ENABLED and metadata.get(MK_WL_CONTEXT_MANAGED)):
            return
        from fastapi import HTTPException  # noqa: PLC0415

        _served = data.get("model")
        _win = self._get_context_window(_served) if _served else None
        _req = metadata.get(MK_WL_REQUIRED)
        if isinstance(_win, int) and _win > 0 and isinstance(_req, int) and _win < _context_floor(_req):
            raise HTTPException(
                status_code=409,
                detail=_context_overflow_detail(_context_floor(_req), [_served], {_served: _win}),
            )

    @staticmethod

# ============================================================================
# 4) SIGNAL-BACK HEADERS (post-call success hook): window / pressure / counted
# ============================================================================
            # ── C6: Wayland #282 context-routing signal-back (gated) ──────
            # When the contract is on, surface the served model's real window,
            # the context pressure (REQUIRED / window), and Flux's authoritative
            # input token count so Core reconciles its #280 gauge against the
            # model Flux ACTUALLY served. Best-effort: a missing window or an
            # absent stash simply omits that header — never breaks the response.
            # When the flag is off none of these emit (byte-identical to today).
            if _FLUX_CONTEXT_CONTRACT_ENABLED:
                served_model = metadata.get(MK_SERVED_MODEL) or model
                served_window = self._get_context_window(served_model) if served_model else None
                if isinstance(served_window, int) and served_window > 0:
                    headers["x-flux-model-window"] = str(served_window)
                    required = metadata.get(MK_WL_REQUIRED)
                    if isinstance(required, int):
                        # FIX 4: contract §2 declares pressure as float 0..1;
                        # genuine-overflow magnitude rides the 409 body, not here.
                        headers["x-flux-context-pressure"] = f"{min(required / served_window, 1.0):.3f}"
                counted = metadata.get(MK_WL_INPUT_TOKENS_COUNTED)
                if isinstance(counted, int):
                    headers["x-flux-context-tokens-counted"] = str(counted)
