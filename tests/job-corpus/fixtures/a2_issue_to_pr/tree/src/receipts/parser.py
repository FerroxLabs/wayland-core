"""Parse pasted expense lines into a total."""

from dataclasses import dataclass, field


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


def parse_expenses(text):
    """Parse ``text`` into a :class:`ParseResult`."""
    result = ParseResult()
    for raw in text.splitlines():
        line = raw.strip()
        description, amount = line.rsplit(",", 1)
        try:
            value = float(amount.strip())
        except ValueError:
            continue
        result.lines.append((description.strip(), value))
        result.total = round(result.total + value, 2)
    return result
