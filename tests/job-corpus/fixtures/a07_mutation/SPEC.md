# `pkg/billing.py` — behaviour specification

This is the contract the module is supposed to honour. It is the only
description of intended behaviour; there are currently no tests.

All amounts are integer **cents**. Floating point must never appear in a
result.

1. **Line subtotal.** A line's subtotal is `unit_price_cents * qty`.
2. **Volume discount.** A per-line discount applies by quantity, in basis
   points (1 bp = 0.01%): `qty >= 100` -> 1500 bp, `qty >= 25` -> 1000 bp,
   `qty >= 10` -> 500 bp, otherwise 0. The tiers are **inclusive at the
   boundary** and the **highest matching tier wins**.
3. **Rounding.** Every division of cents rounds **halves away from zero**
   (half-up for positive amounts). It is not floor, not ceiling, and not
   banker's rounding.
4. **Order promo.** The order-level promo (`promo_bp`) is computed on the
   **post-volume-discount** order net, and is then **capped** at
   `max_promo_cents` when that argument is given.
5. **Tax base.** Tax is computed on the **post-discount** amount, never on the
   pre-discount gross.
6. **Tax exemption.** A `tax_exempt` order pays **zero** tax; all discounts
   still apply to it.
7. **Quantity validation.** A quantity that is not a positive `int` is
   rejected: zero or negative raises `ValueError`, a non-int (including
   `bool`) raises `TypeError`.
8. **Never negative.** If the promo exceeds the net, the discounted amount
   floors at zero (and tax is then computed on zero).
9. **Proration.** `prorate_cents` charges `days_used / days_in_period` of the
   amount, **rounding down** so the customer is never over-charged, and clamps
   `days_used` to `days_in_period`.
