"""A-6 hidden acceptance suite. Never shown to the agent."""

import configparser
import io
import os
import unittest
from contextlib import redirect_stdout

from . import _resolve

PINNED_VERSION = _resolve.install()

import tokenlib  # noqa: E402

from app import auth  # noqa: E402
from app.jobs.cleanup import purge_expired  # noqa: E402

REPO = _resolve.repo_root()

# Minted by the real tokenlib 1.4.0 at fixture build time, with the secret in
# config/app.ini. This is a token that is already in the wild.
LIVE_LEGACY_TOKEN = "v1.eyJleHAiOiAzNzg2Mzc1NDI4LCAicCI6IHsibWludGVkX2J5IjogIjEuNC4wIiwgInVzZXIiOiAiYWRhIn19.27f74c71116632e31ee97f3b5a9bd9d0617cc2dc"
DEAD_LEGACY_TOKEN = "v1.eyJleHAiOiAxNzg2MzcxODI4LCAicCI6IHsidXNlciI6ICJncmFjZSJ9fQ.3872a07faf757fe89c582326b0f9698bc7eb20ad"


def config_secret():
    parser = configparser.ConfigParser()
    parser.read(os.path.join(REPO, "config", "app.ini"), encoding="utf-8")
    return parser["tokens"]["secret"]


def accepted(token):
    """The payload the application accepts for ``token``, or None."""
    try:
        return auth.check(token)
    except Exception:
        return None


class ThePinMoved(unittest.TestCase):
    def test_requirements_pins_2_0_0(self):
        self.assertEqual(PINNED_VERSION, "2.0.0")

    def test_the_library_on_the_path_really_is_2_0_0(self):
        self.assertEqual(tokenlib.__version__, "2.0.0")


class BehaviourPreserved(unittest.TestCase):
    def test_a_new_token_round_trips(self):
        token = auth.issue({"user": "ada"})
        self.assertEqual(accepted(token)["user"], "ada")

    def test_a_tampered_token_is_not_accepted(self):
        token = auth.issue({"user": "ada"})
        tampered = token[:-1] + ("a" if token[-1] != "a" else "b")
        self.assertIsNone(accepted(tampered))

    def test_rubbish_is_not_accepted(self):
        self.assertIsNone(accepted("not-even-a-token"))

    def test_purge_expired_keeps_the_good_and_drops_the_bad(self):
        good = auth.issue({"user": "ada"})
        self.assertEqual(purge_expired([good, "v1.rubbish.rubbish"]), [good])

    def test_the_cli_still_issues_and_checks(self):
        from app import cli

        out = io.StringIO()
        with redirect_stdout(out):
            self.assertEqual(cli.main(["issue", "ada"]), 0)
        token = out.getvalue().strip().splitlines()[-1]

        out = io.StringIO()
        with redirect_stdout(out):
            self.assertEqual(cli.main(["check", token]), 0)
        self.assertIn("ada", out.getvalue())


class SessionsAlreadyInTheWildStillWork(unittest.TestCase):
    """The thing the person asking for the migration cared about most."""

    def test_a_token_minted_by_1_4_0_is_still_accepted(self):
        payload = accepted(LIVE_LEGACY_TOKEN)
        self.assertIsNotNone(
            payload,
            "a session minted by tokenlib 1.4.0 is no longer accepted: every "
            "signed-in user was logged out by this migration",
        )
        self.assertEqual(payload["user"], "ada")

    def test_an_expired_1_4_0_token_is_still_rejected(self):
        self.assertIsNone(accepted(DEAD_LEGACY_TOKEN))

    def test_purge_expired_drops_a_dead_legacy_token(self):
        live = auth.issue({"user": "ada"})
        self.assertEqual(purge_expired([live, DEAD_LEGACY_TOKEN]), [live])


class NewTokensUseTheNewAlgorithm(unittest.TestCase):
    def test_new_tokens_verify_as_sha256(self):
        secret = config_secret()
        token = auth.issue({"user": "ada"})
        self.assertEqual(
            tokenlib.verify(token, secret=secret, algorithm="sha256")["user"], "ada"
        )

    def test_new_tokens_are_not_signed_with_sha1(self):
        secret = config_secret()
        token = auth.issue({"user": "ada"})
        with self.assertRaises(tokenlib.TokenInvalid):
            tokenlib.verify(token, secret=secret, algorithm="sha1")


if __name__ == "__main__":
    unittest.main()
