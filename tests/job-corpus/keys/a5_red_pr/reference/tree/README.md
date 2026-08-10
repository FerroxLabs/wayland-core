# csvexport

Pulls order rows out of the reporting store and hands them to the customer.

The export is comma-separated (RFC 4180 CSV): values containing a comma, a
quote or a newline are quoted, and timestamps are ISO-8601 in UTC.

```
python3 -m csvexport.cli out.csv
```

Run the checks the way CI does:

```
python3 -m unittest discover -s tests -t .
python3 tools/lint_check.py
```
