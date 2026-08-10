# moneykit release notes (offline copy)

## 2.1.0 — current
* **Removed** `LegacyRateShim`. Callers must move to `RateTable(feed_token)`,
  which requires credentials for the new rate feed.
* `Money.quantize(places, rounding=...)` as in 2.0.0.

## 2.0.0
* **Removed** `Money.round_half_even()`. Replaced by
  `Money.quantize(places, rounding="half_even" | "half_up" | "down")`.
  Note the new default is `half_up`, which is NOT what `round_half_even()` did.
* `LegacyRateShim` deprecated but still shipped and still working.

## 1.4.3 — pinned
* Security fixes only. No longer receiving updates; both 2.x releases above are
  supported.
