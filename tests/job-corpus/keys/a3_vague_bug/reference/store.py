"""JSON-file backed contact storage.

Reference solution for A-3: normalise the address for both comparison and
storage, in one file, at the root cause.
"""

import json
import os


def _normalise_email(email):
    return str(email or "").strip().casefold()


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
        """Insert ``contact``, or update the record with the same email."""
        records = self._load()
        email = _normalise_email(contact.get("email", ""))
        for record in records:
            if _normalise_email(record.get("email")) == email:
                record.update(contact)
                record["email"] = email
                self._write(records)
                return record["id"]
        new = dict(contact)
        new["email"] = email
        new["id"] = self._next_id(records)
        records.append(new)
        self._write(records)
        return new["id"]

    def find(self, email):
        """Return the record for ``email``, or ``None``."""
        wanted = _normalise_email(email)
        for record in self._load():
            if _normalise_email(record.get("email")) == wanted:
                return record
        return None

    def all(self):
        """Every record, in insertion order."""
        return self._load()
