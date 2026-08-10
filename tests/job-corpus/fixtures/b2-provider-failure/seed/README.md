# monthly-billing

Runs the month-end billing reconciliation.

* `data/contracts.csv` — one row per customer under contract.
* `data/usage-2026-07.csv` — metered usage, several rows per customer.
* `billing/rates.py` — loaders, already used by the invoicing job.

There is no reconciliation report yet. That is what finance keeps asking for.
