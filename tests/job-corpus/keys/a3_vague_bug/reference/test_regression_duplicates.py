"""Reference regression test for A-3.

Used by the harness self-test as the positive control for the two-revision
check: this file MUST fail by assertion against the ``pre-fix`` revision and
pass after the reference fix. If it ever stops failing at pre-fix, the fixture
has rotted and the row can no longer discriminate.
"""

import os
import tempfile
import unittest

from contacts import ContactStore


class DuplicateOnResave(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.store = ContactStore(os.path.join(self.dir.name, "book.json"))

    def tearDown(self):
        self.dir.cleanup()

    def test_resaving_with_a_differently_typed_address_does_not_duplicate(self):
        self.store.save({"name": "Ada", "email": "ada@example.com"})
        self.store.save({"name": "Ada Lovelace", "email": " Ada@Example.COM "})
        self.assertEqual(len(self.store.all()), 1)


if __name__ == "__main__":
    unittest.main()
