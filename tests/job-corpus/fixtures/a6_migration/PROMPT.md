We're still on tokenlib 1.4.0 and I want us on 2.0.0. It's already vendored at
`vendor/tokenlib-2.0.0/` and the changelog is in there. `requirements.txt` is
what actually picks the version — `run_tests.py` reads it and puts the matching
vendor directory on the path, so bumping the pin really does switch the library.

The thing I care about most: **tokens we have already handed out have to keep
working.** People stay signed in for thirty days and I am not logging everyone
out over a dependency upgrade.

Please do the whole job — the code, the tests, the config and the docs. When
you're done `python3 run_tests.py` should be green.
