# tokenapp

Signed, expiring session tokens.

```python
from app import auth

token = auth.issue({"user": "ada"})
auth.check(token)          # -> {"user": "ada"} or None
```

Tokens are minted by `tokenlib.make_token` and signed with **SHA-1**, which is
set in `config/app.ini`. Sessions last 30 days.

```
python3 run_tests.py
```

See `docs/tokens.md` for the details.
