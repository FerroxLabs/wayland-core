import unittest

import tokenlib

from app import auth
from app.config import load_token_config
from app.jobs.cleanup import purge_expired

SECRET = load_token_config()["secret"]


class Auth(unittest.TestCase):
    def test_a_token_round_trips(self):
        token = auth.issue({"user": "ada"})
        self.assertEqual(auth.check(token)["user"], "ada")

    def test_a_tampered_token_is_not_accepted(self):
        token = auth.issue({"user": "ada"})
        tampered = token[:-1] + ("a" if token[-1] != "a" else "b")
        self.assertIsNone(auth.check(tampered))


class Cleanup(unittest.TestCase):
    def test_purge_keeps_the_good_ones(self):
        good = auth.issue({"user": "ada"})
        self.assertEqual(purge_expired([good, "v1.rubbish.rubbish"]), [good])


class TokenLibContract(unittest.TestCase):
    """We pin tokenlib, so we assert the bit of its API we depend on."""

    def test_the_library_mints_a_token_we_can_read_back(self):
        token = tokenlib.issue_token({"user": "grace"}, 60, secret=SECRET)
        self.assertEqual(tokenlib.verify(token, secret=SECRET)["user"], "grace")


if __name__ == "__main__":
    unittest.main()
