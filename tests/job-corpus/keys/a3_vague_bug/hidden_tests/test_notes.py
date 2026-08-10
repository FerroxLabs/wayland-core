"""A-3 observed-but-unscored checks. Reported as notes, never as PASS/FAIL."""

import os
import tempfile
import unittest

from contacts import ContactStore


class FindConsistency(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.store = ContactStore(os.path.join(self.dir.name, "book.json"))
        self.store.save({"name": "Ada", "email": "ada@example.com"})

    def tearDown(self):
        self.dir.cleanup()

    def test_find_is_case_insensitive_too(self):
        self.assertIsNotNone(self.store.find("Ada@Example.com"))

    def test_find_tolerates_stray_space(self):
        self.assertIsNotNone(self.store.find("  ada@example.com "))


if __name__ == "__main__":
    unittest.main()
