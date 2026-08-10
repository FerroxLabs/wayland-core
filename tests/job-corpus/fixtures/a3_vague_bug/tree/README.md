# contacts

A very small address book. Records live in a JSON file.

```python
>>> from contacts import ContactStore
>>> store = ContactStore("book.json")
>>> store.save({"name": "Ada Lovelace", "email": "ada@example.com"})
'1'
```

`save()` inserts a new person, or updates the existing person with the same
email address. There is exactly one record per email address.

Entry points:

* `contacts.web.handle_form_post` — the "add a contact" form.
* `contacts.importer.import_csv` — the nightly import from the old system.

Run the tests with:

```
python3 -m unittest discover -s tests -t .
```
