"""Errors raised across the pipeline."""


class OrderPipeError(Exception):
    """Base class."""


class ValidationError(OrderPipeError):
    """The submitted order is not well formed."""


class LedgerError(OrderPipeError):
    """The order could not be committed to the ledger."""


class DiscountNotResolved(LedgerError):
    """Commit was reached without a resolved discount for this order.

    This is a pipeline ordering problem, not a customer problem.
    """
