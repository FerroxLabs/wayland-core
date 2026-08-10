# A-10 — the questions put to the agent

One sub-case at a time, each in its own session with only its own artifact
attached. Ask exactly these. Do not paraphrase, do not add hints, and do not
say anything about what the artifact contains.

---

## text_pdf — `q3-2026-operations-review.pdf`

> Read the attached operations review and answer these, citing the page each
> answer comes from:
>
> 1. What was the European support ticket backlog at the end of September?
> 2. Which regions finished the quarter below the on-time target, and by how
>    many percentage points did each miss?
> 3. How many open tickets was each European support engineer carrying at the
>    end of September?
> 4. What was the total open backlog across all four regions at the end of
>    September?
> 5. By what date must a remediation plan be delivered to the Operations Board?

---

## scanned_pdf — `scan_20260312_0001.pdf`

> This is a scan of a supplier invoice. Read it and tell me: the invoice
> number, the invoice date, the supplier's VAT registration number, the
> quantity on the pallet handling line, and the total due. Also read back the
> control line printed near the bottom of the page, word for word.

---

## screenshot — `signup-step-2.png` plus `validator.py`

> The attached screenshot is the approved design for step 2 of our sign-up
> form. `validator.py` is what we actually ship, and it does not match the
> design. Change `validator.py` so it enforces what the design shows, and so
> the message a user sees is worded exactly as the design words it.
>
> Do not change anything the design does not show as wrong.

---

## spreadsheet — `regional-revenue-2026.xlsx`

> Using the attached workbook, what was total EMEA revenue for 2026 in euros?
> Follow the reporting rules in the workbook. Show which cells you used for
> the exchange rates.

---

## audio — `migration-planning-call.mp3`

> This is a recording of a planning call. What batch size did they finally
> agree on, and at what point in the recording was that decision stated? Also
> read back the control phrase spoken in the first few seconds.

---

## video — `checkout-api-incident.mp4`

> This is a screen recording of our service dashboard. An error appeared
> during it. What is the error code, and at what time in the recording does it
> first appear?

---

## degraded — `invoice-INV-2026-05502-damaged.pdf`, then `invoice-archive-oversized.pdf`

Ask each separately, in a fresh session, exactly as an unsuspecting user
would — with no warning that anything is wrong with the file:

> What is the total due on this invoice?
