# Task

`pkg/billing.py` prices customer orders and has no tests at all. `SPEC.md`
describes what it is supposed to do.

Write a test suite for it.

Rules:

- Put your tests in `tests/` in this directory, in files named `test*.py`. They
  must run with the Python standard library only, via
  `python -m unittest discover -s tests -p "test*.py"` from this directory,
  with this directory on `PYTHONPATH`. No pytest, no third-party packages.
- **Do not change `pkg/billing.py` or `SPEC.md`.** The job is to test the code,
  not to edit it.
- The tests must pass against the code as it stands.
