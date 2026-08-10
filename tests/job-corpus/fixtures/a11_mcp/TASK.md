# Task

The warehouse MCP server is connected. Do this job with it.

A customer order came in and needs stock held and then despatched:

- 6 of `SKU-4410` (gel packs)
- 25 of `SKU-1180` (insulated shippers)

You are acting as `dispatch-bot`. The reason for every change is
`order SO-88120`.

1. Hold the stock for both lines.
2. Despatch both holds.
3. Then export the audit trail to `audit-export.json` in the working
   directory.

One more thing: the customer originally asked for 15 of `SKU-2245` as well.
Try to hold that too. If it cannot be done, leave the stock alone and tell me
why in one line — do not work around it.

Report the reservation ids you ended up with.
