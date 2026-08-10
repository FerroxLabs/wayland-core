"""Hidden acceptance tests for the A-10 screenshot sub-case.

The threshold and the wording checked here exist only in `signup-step-2.png`.
They are in no source file, no comment and no commit message, so these tests
can only pass if someone actually read the picture and then changed the code.

The email cases are the distractor guard: the picture outlines the email field
in red, and an agent that "fixes" email validation on the strength of a red
border breaks them.
"""

import unittest

import validator


class PasswordLength(unittest.TestCase):
    def test_eleven_characters_is_rejected(self):
        problems = validator.validate_password("abcdefgh123")
        self.assertTrue(problems, "an 11 character password should be rejected")

    def test_twelve_characters_is_accepted(self):
        self.assertEqual(validator.validate_password("abcdefgh1234"), [])

    def test_the_message_is_the_one_in_the_design(self):
        problems = validator.validate_password("abcdefgh123")
        self.assertIn("Password must be at least 12 characters", problems)

    def test_eight_characters_is_no_longer_enough(self):
        problems = validator.validate_password("abcdefg1")
        self.assertTrue(problems, "8 characters was the old rule and must now fail")


class RulesThatMustSurvive(unittest.TestCase):
    def test_a_password_with_no_digit_is_still_rejected(self):
        problems = validator.validate_password("abcdefghijklmno")
        self.assertIn("Password must contain at least 1 number", problems)

    def test_an_empty_password_is_still_rejected(self):
        self.assertIn("Password is required", validator.validate_password(""))


class EmailIsTheDistractor(unittest.TestCase):
    def test_a_valid_email_is_still_accepted(self):
        self.assertEqual(validator.validate_email("dana.whitfield@example.com"), [])

    def test_a_truncated_email_is_still_rejected(self):
        self.assertEqual(
            validator.validate_email("dana.whitfield@"),
            ["Enter a valid email address"],
        )

    def test_a_missing_email_is_still_rejected(self):
        self.assertEqual(validator.validate_email(""), ["Email is required"])


class WholeSubmission(unittest.TestCase):
    def test_a_good_submission_has_no_problems(self):
        self.assertEqual(
            validator.validate_signup("dana.whitfield@example.com", "abcdefgh1234"), []
        )

    def test_a_short_password_is_reported_with_the_design_wording(self):
        problems = validator.validate_signup("dana.whitfield@example.com", "abcdefgh123")
        self.assertIn("Password must be at least 12 characters", problems)


if __name__ == "__main__":
    unittest.main()
