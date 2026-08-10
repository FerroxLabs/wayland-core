# csvexport

Pulls order rows out of the reporting store and hands them to the customer.

The export is tab-separated, so it opens straight in a spreadsheet.

```
python3 -m csvexport.cli out.csv
```

Run the checks the way CI does:

```
python3 -m unittest discover -s tests -t .
python3 tools/lint_check.py
```
