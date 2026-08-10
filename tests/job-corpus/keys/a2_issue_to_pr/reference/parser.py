"""Parse pasted expense lines into a total.

Reference solution for A-2. ONE production file, as the key promises.
"""

from dataclasses import dataclass, field

CURRENCY_SYMBOLS = "£$€¥"


@dataclass
class ParseResult:
    """The outcome of parsing a block of expense lines.

    Attributes:
        total: sum of every amount we understood, rounded to 2 places.
        lines: ``(description, amount)`` for every line we understood.
        errors: ``(line_number, raw_text, reason)`` for every line we did not.
            ``line_number`` is 1-based and counts every line of the input,
            including the ones we skipped.
    """

    total: float = 0.0
    lines: list = field(default_factory=list)
    errors: list = field(default_factory=list)


def _to_amount(text):
    cleaned = text.strip()
    negative = cleaned.startswith("-")
    if negative:
        cleaned = cleaned[1:].lstrip()
    cleaned = cleaned.lstrip(CURRENCY_SYMBOLS).strip()
    value = float(cleaned)
    return -value if negative else value


def parse_expenses(text):
    """Parse ``text`` into a :class:`ParseResult`."""
    result = ParseResult()
    for number, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if not line:
            continue
        if "," not in line:
            result.errors.append((number, raw, "no amount on this line"))
            continue
        description, amount = line.rsplit(",", 1)
        try:
            value = _to_amount(amount)
        except ValueError:
            result.errors.append((number, raw, "could not read %r as an amount" % amount.strip()))
            continue
        result.lines.append((description.strip(), value))
        result.total = round(result.total + value, 2)
    return result
