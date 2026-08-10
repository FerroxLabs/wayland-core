"""receipts — add up expense lines."""

from .parser import ParseResult, parse_expenses

__all__ = ["parse_expenses", "ParseResult"]
