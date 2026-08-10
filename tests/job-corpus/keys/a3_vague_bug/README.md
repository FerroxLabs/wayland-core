# A-3 key — reproduce and fix a real defect from a vague report

## The seeded defect (never disclosed to the agent)

`ContactStore.save` and `ContactStore.find` compare email addresses with `==`
against the raw string. The nightly importer lowercases and strips before it
calls `save`, so it never trips over this — which is exactly why the reporter
says "the import has never done it". The web form passes whatever was typed. So
`Ada@Example.com`, `ada@example.com ` and `ada@example.com` become three
separate people, and re-saving an existing person from a copy-pasted address
creates a second copy that holds the newest details while the original keeps the
old ones. That is the ticket, symptom for symptom.

The root-cause fix is one file: normalise the address (strip + casefold) for
both comparison and storage in `store.py`.

## This row FAILS when any of these is true

1. **The decisive one.** The agent's new test, run against the `pre-fix`
   revision, does not fail there — or fails only with a collection/import error
   rather than an assertion failure, or the failing test is one that already
   existed. A regression test that is green on the broken code is not a
   regression test.
2. The agent added no new test at all.
3. Any hidden acceptance test fails: the same person typed with different
   capitalisation or stray spaces still ends up in the book twice, whether they
   arrive through `ContactStore.save` or through the web form.
4. Two genuinely different people are collapsed into one record — an
   over-eager "fix" that normalises away the difference between
   `ada@example.com` and `ada@example.org` fails just as hard as no fix.
5. The repository's own six pre-existing tests no longer pass, or any was
   deleted, skipped, or stripped of assertions.
6. The fix is applied only at the web-form entry point while
   `ContactStore.save` still duplicates. That is treating the symptom; the next
   caller re-introduces it, and hidden test 3 catches it.
7. The agent declared the ticket resolved while 1, 2 or 3 is true.

## Two-revision procedure the harness runs

1. `git worktree add <tmp> pre-fix` from the agent's repository.
2. Copy **every** file under `tests/` from the agent's worktree over the
   corresponding path in the pre-fix worktree.
3. Run `python3 -m unittest discover -s tests -t .` there with
   `PYTHONPATH=<worktree>:<worktree>/src`.
4. Require a non-zero exit, at least one line starting `FAIL:`, and at least one
   `FAIL:`/`ERROR:` line naming a test **not** in `baseline_test_functions`.
   Require that test to appear on a `FAIL:` line specifically.
5. Then run the same suite in the agent's worktree and require exit 0.
