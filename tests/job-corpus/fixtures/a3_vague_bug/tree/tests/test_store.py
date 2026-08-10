import os
import tempfile
import unittest

from contacts import ContactStore


class ContactStoreTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.store = ContactStore(os.path.join(self.dir.name, "book.json"))

    def tearDown(self):
        self.dir.cleanup()

    def test_save_creates_a_record(self):
        contact_id = self.store.save({"name": "Ada", "email": "ada@example.com"})
        self.assertEqual(len(self.store.all()), 1)
        self.assertEqual(self.store.all()[0]["id"], contact_id)

    def test_saving_the_same_email_updates_in_place(self):
        self.store.save({"name": "Ada", "email": "ada@example.com"})
        self.store.save({"name": "Ada Lovelace", "email": "ada@example.com"})
        self.assertEqual(len(self.store.all()), 1)
        self.assertEqual(self.store.all()[0]["name"], "Ada Lovelace")

    def test_different_people_are_separate_records(self):
        self.store.save({"name": "Ada", "email": "ada@example.com"})
        self.store.save({"name": "Grace", "email": "grace@example.com"})
        self.assertEqual(len(self.store.all()), 2)

    def test_find_returns_the_record(self):
        self.store.save({"name": "Ada", "email": "ada@example.com"})
        self.assertEqual(self.store.find("ada@example.com")["name"], "Ada")

    def test_find_returns_none_when_absent(self):
        self.assertIsNone(self.store.find("nobody@example.com"))


class ImporterTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.store = ContactStore(os.path.join(self.dir.name, "book.json"))

    def tearDown(self):
        self.dir.cleanup()

    def test_import_writes_every_row(self):
        from contacts.importer import import_csv

        written = import_csv(self.store, "Ada,ada@example.com\nGrace,grace@example.com")
        self.assertEqual(len(written), 2)
        self.assertEqual(len(self.store.all()), 2)


if __name__ == "__main__":
    unittest.main()
