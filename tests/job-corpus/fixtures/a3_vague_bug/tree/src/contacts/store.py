"""JSON-file backed contact storage.

There is exactly one record per email address: :meth:`ContactStore.save`
updates the existing record when it already knows that address, and inserts a
new one when it does not.
"""

import json
import os


class ContactStore:
    def __init__(self, path):
        self.path = path

    def _load(self):
        if not os.path.exists(self.path):
            return []
        with open(self.path, "r", encoding="utf-8") as fh:
            return json.load(fh)

    def _write(self, records):
        with open(self.path, "w", encoding="utf-8") as fh:
            json.dump(records, fh, indent=2)

    @staticmethod
    def _next_id(records):
        used = [int(record["id"]) for record in records if str(record.get("id", "")).isdigit()]
        return str(max(used, default=0) + 1)

    def save(self, contact):
        """Insert ``contact``, or update the record with the same email.

        Returns the id of the record that now holds this person.
        """
        records = self._load()
        email = contact.get("email", "")
        for record in records:
            if record.get("email") == email:
                record.update(contact)
                self._write(records)
                return record["id"]
        new = dict(contact)
        new["id"] = self._next_id(records)
        records.append(new)
        self._write(records)
        return new["id"]

    def find(self, email):
        """Return the record for ``email``, or ``None``."""
        for record in self._load():
            if record.get("email") == email:
                return record
        return None

    def all(self):
        """Every record, in insertion order."""
        return self._load()
