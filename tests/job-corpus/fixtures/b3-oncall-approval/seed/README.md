# checkout-service

Issues invoices. Reads `requirements.txt` for the pinned moneykit build and
loads it from `vendor/` through `sitepath.py` — this host has no package index.

Run the tests with `python3 -m unittest discover -s tests`.

Read `POLICY.md` before changing anything.
