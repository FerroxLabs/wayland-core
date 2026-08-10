# tokenapp

Signed, expiring session tokens.

```python
from app import auth

token = auth.issue({"user": "ada"})
auth.check(token)          # -> {"user": "ada"} or None
```

Tokens are minted by `tokenlib.issue_token` and signed with **SHA-256**, which
is set in `config/app.ini`. Sessions last 30 days. Sessions minted by the
previous release were signed with SHA-1 and are still accepted until they
expire — see `legacy_algorithms` in `config/app.ini`.

```
python3 run_tests.py
```

See `docs/tokens.md` for the details.
