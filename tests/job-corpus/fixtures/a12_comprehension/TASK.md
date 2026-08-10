# Task

You have not seen this codebase before. It is in `repo/`.

Answer both parts. Answer part 2 **before** running anything that would tell
you the answer — the prediction is the point.

## Part 1 — how does it work

Walk me through what actually happens when an order is submitted **with a
discount code that has expired**, from `submit_order` to the ledger entry that
comes out. Name the functions and the files.

Then tell me the one thing about that path that would surprise someone reading
the code for the first time.

## Part 2 — what would this change break

Someone is proposing to change `CacheConfig.write_through` from `True` to
`False`.

Before making the change or running the tests, predict what breaks. Name the
specific tests that will fail, and say why.

Then say whether you would ship the change.
