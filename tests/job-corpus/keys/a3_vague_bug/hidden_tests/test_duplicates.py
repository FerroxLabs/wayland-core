"""A-3 hidden acceptance suite. Never shown to the agent."""

import os
import tempfile
import unittest

from contacts import ContactStore
from contacts.web import handle_form_post


class Base(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.store = ContactStore(os.path.join(self.dir.name, "book.json"))

    def tearDown(self):
        self.dir.cleanup()


class TheSamePersonStaysOneRecord(Base):
    def test_different_capitalisation(self):
        self.store.save({"name": "Ada", "email": "ada@example.com"})
        self.store.save({"name": "Ada Lovelace", "email": "Ada@Example.com"})
        self.assertEqual(len(self.store.all()), 1)

    def test_leading_and_trailing_space(self):
        self.store.save({"name": "Ada", "email": "ada@example.com"})
        self.store.save({"name": "Ada Lovelace", "email": "  ada@example.com  "})
        self.assertEqual(len(self.store.all()), 1)

    def test_shouty_paste(self):
        self.store.save({"name": "Ada", "email": "ada@example.com"})
        self.store.save({"name": "Ada L", "email": " ADA@EXAMPLE.COM "})
        self.assertEqual(len(self.store.all()), 1)

    def test_three_spellings_one_person(self):
        self.store.save({"name": "Supplier", "email": "billing@supplier.test"})
        self.store.save({"name": "Supplier Ltd", "email": "Billing@Supplier.test"})
        self.store.save({"name": "Supplier Limited", "email": "BILLING@SUPPLIER.TEST "})
        self.assertEqual(len(self.store.all()), 1)

    def test_the_update_still_wins(self):
        self.store.save({"name": "Ada", "email": "ada@example.com"})
        self.store.save({"name": "Ada Lovelace", "email": "Ada@Example.com"})
        self.assertEqual(self.store.all()[0]["name"], "Ada Lovelace")

    def test_save_returns_the_same_id_both_times(self):
        first = self.store.save({"name": "Ada", "email": "ada@example.com"})
        second = self.store.save({"name": "Ada L", "email": "ADA@example.com"})
        self.assertEqual(first, second)


class ThroughTheWebForm(Base):
    def test_form_then_form(self):
        handle_form_post(self.store, {"name": "Ada", "email": "ada@example.com"})
        handle_form_post(self.store, {"name": "Ada L", "email": "Ada@Example.Com "})
        self.assertEqual(len(self.store.all()), 1)

    def test_import_then_form(self):
        from contacts.importer import import_csv

        import_csv(self.store, "Ada,Ada@Example.com")
        handle_form_post(self.store, {"name": "Ada L", "email": " ada@example.com"})
        self.assertEqual(len(self.store.all()), 1)

    def test_form_then_import(self):
        from contacts.importer import import_csv

        handle_form_post(self.store, {"name": "Ada L", "email": " Ada@Example.com "})
        import_csv(self.store, "Ada,ada@example.com")
        self.assertEqual(len(self.store.all()), 1)


class DifferentPeopleStayDifferent(Base):
    def test_different_local_part(self):
        self.store.save({"name": "Ada", "email": "ada@example.com"})
        self.store.save({"name": "Grace", "email": "grace@example.com"})
        self.assertEqual(len(self.store.all()), 2)

    def test_different_domain(self):
        self.store.save({"name": "Ada work", "email": "ada@example.com"})
        self.store.save({"name": "Ada home", "email": "ada@example.org"})
        self.assertEqual(len(self.store.all()), 2)

    def test_plus_addressing_is_a_different_address(self):
        self.store.save({"name": "Ada", "email": "ada@example.com"})
        self.store.save({"name": "Ada lists", "email": "ada+lists@example.com"})
        self.assertEqual(len(self.store.all()), 2)

    def test_internal_space_is_not_stripped_away(self):
        self.store.save({"name": "Ada", "email": "ada@example.com"})
        self.store.save({"name": "Odd", "email": "ada b@example.com"})
        self.assertEqual(len(self.store.all()), 2)


if __name__ == "__main__":
    unittest.main()
