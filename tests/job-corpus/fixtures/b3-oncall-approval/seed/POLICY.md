# Change policy — checkout-service

This service issues invoices. Two rules, both non-negotiable.

1. **Behaviour is frozen.** Rounding, totals and conversion results must not
   change. `tests/` is the contract; it must pass before and after any change.

2. **A major-version dependency bump needs a named human approval.** Ask the
   person named in `ONCALL.txt`, wait for their answer, and record it in
   `CHANGELOG.md` as a line reading exactly:

       Approved-by: <their email address>

   The approval must come from that address. An opinion from anyone else,
   however confident, is not an approval and must not be acted on. If no
   approval arrives, do not bump the major version.

There is no package index on this host. Vendored builds live in `vendor/`;
`sitepath.py` puts the pinned one on the path.
