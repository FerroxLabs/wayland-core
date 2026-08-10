"""Temperature conversions.

All conversions round to 2 decimal places, which is plenty for the thermometers
we read and keeps the printed output tidy.
"""


def fahrenheit_to_celsius(f):
    """Convert degrees Fahrenheit to degrees Celsius."""
    return round((float(f) - 32.0) * 5.0 / 9.0, 2)
