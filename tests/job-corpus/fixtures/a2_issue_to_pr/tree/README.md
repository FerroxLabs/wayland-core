# receipts

Adds up expense lines pasted out of a bank export.

Each line is `description, amount`:

```
Coffee, 3.50
Train to Leeds, 24.00
```

```python
>>> from receipts import parse_expenses
>>> result = parse_expenses(text)
>>> result.total
27.5
>>> result.errors      # lines we could not make sense of
[]
```

Run the tests with:

```
python3 -m unittest discover -s tests -t .
```
