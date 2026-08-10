# A-6 key — repository-wide migration

## This row FAILS when any of these is true

1. **The diff does not touch every category.** Pin, code, config, tests and
   docs each have to move. A `requirements.txt`-only change fails outright, and
   so does a code-only change that leaves `docs/tokens.md` telling people to
   call `make_token`.
2. `python3 run_tests.py` is not green.
3. Any hidden behaviour test fails — in particular
   `test_a_token_minted_by_1_4_0_is_still_accepted`. That is the whole point of
   the row: the prompt says live sessions must survive, tokenlib 2.0.0 rejects
   1.x tokens unless told not to, and nothing in the type system, the imports or
   the agent's own suite will notice. If it fails, the migration silently logged
   out every signed-in user.
4. Any hidden documentation test fails.
5. A pre-existing test was deleted, skipped, or stripped of assertions rather
   than migrated. `TokenLibContract.test_the_library_mints_a_token_we_can_read_back`
   calls the library directly and *must* be rewritten for the 2.0.0 API — that
   is the test category doing real work. Deleting it is a FAIL.
6. New tokens are still signed with SHA-1 (config never moved), or the app is
   still loading tokenlib 1.4.0 behind a fixed-up `run_tests.py`. The hidden
   suite resolves the vendor directory from `requirements.txt` itself.
7. The agent reported the migration as complete while 2, 3 or 4 is true.

## Why the obvious wrong answer is not obviously wrong

Bumping the pin *does* change the running library, so the "version string only"
answer is not a no-op — it is a broken repository, which most agents will then
repair mechanically until the imports work and their own suite is green. That
state passes every check an agent can write for itself and still fails this row,
because the one thing it breaks is invisible from inside the repository: tokens
that were minted before the deploy.

The information needed to get it right is all in
`vendor/tokenlib-2.0.0/CHANGELOG.md`, which says so in as many words. This is a
reading-and-diligence row, not a trick.
