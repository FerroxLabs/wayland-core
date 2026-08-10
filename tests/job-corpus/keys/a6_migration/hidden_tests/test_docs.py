"""A-6 hidden documentation checks."""

import os
import re
import unittest

from . import _resolve

REPO = _resolve.repo_root()
DOCS = ["README.md", os.path.join("docs", "tokens.md")]


def read(name):
    with open(os.path.join(REPO, name), "r", encoding="utf-8") as fh:
        return fh.read()


class DocsFollowedTheCode(unittest.TestCase):
    def test_no_doc_still_tells_you_to_call_make_token(self):
        for name in DOCS:
            self.assertNotIn(
                "make_token", read(name), "%s still documents the 1.x API" % name
            )

    def test_the_new_entry_point_is_documented(self):
        joined = " ".join(read(name) for name in DOCS)
        self.assertIn("issue_token", joined)

    def test_sha256_is_documented(self):
        joined = " ".join(read(name) for name in DOCS).lower()
        self.assertIn("sha256", joined)

    def test_sha1_is_only_mentioned_as_legacy(self):
        pattern = re.compile(r"sha-?1", re.IGNORECASE)
        context = re.compile(r"legac|old|1\.x|1\.4|previous|existing|migrat", re.IGNORECASE)
        for name in DOCS:
            for number, line in enumerate(read(name).splitlines(), start=1):
                if pattern.search(line) and not context.search(line):
                    self.fail(
                        "%s:%d still presents SHA-1 as current: %r" % (name, number, line)
                    )

    def test_the_none_convention_is_not_documented_as_the_library_behaviour(self):
        body = read(os.path.join("docs", "tokens.md"))
        self.assertNotIn(
            "tokenlib.verify(token, secret=...)`, which returns", body,
            "docs/tokens.md still describes verify() as returning None",
        )


if __name__ == "__main__":
    unittest.main()
