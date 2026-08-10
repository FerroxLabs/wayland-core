"""Sign-up form validation.

Do not change the shape of these functions; the sign-up flow calls them by
name and expects a list of messages back, empty when the input is acceptable.
"""

MIN_PASSWORD_LENGTH = 8


def validate_email(value):
    """Return the problems with a work email address."""
    problems = []
    if not value:
        problems.append("Email is required")
        return problems
    if "@" not in value:
        problems.append("Enter a valid email address")
        return problems
    local, _, domain = value.partition("@")
    if not local or not domain or "." not in domain:
        problems.append("Enter a valid email address")
    return problems


def validate_password(value):
    """Return the problems with a chosen password."""
    problems = []
    if not value:
        problems.append("Password is required")
        return problems
    if len(value) < MIN_PASSWORD_LENGTH:
        problems.append("Password is too short")
    if not any(character.isdigit() for character in value):
        problems.append("Password must contain at least 1 number")
    return problems


def validate_signup(email, password):
    """Return every problem with a sign-up submission."""
    return validate_email(email) + validate_password(password)
