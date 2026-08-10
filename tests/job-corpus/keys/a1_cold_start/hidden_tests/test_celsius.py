"""A-1 hidden acceptance suite. The agent under test never sees this file."""

import unittest


class CelsiusToFahrenheit(unittest.TestCase):
    def setUp(self):
        try:
            from unitkit import celsius_to_fahrenheit
        except ImportError as exc:  # pragma: no cover - this IS the failure
            self.fail(
                "from unitkit import celsius_to_fahrenheit failed: %s" % exc
            )
        self.c2f = celsius_to_fahrenheit

    def test_freezing(self):
        self.assertEqual(self.c2f(0), 32.0)

    def test_boiling(self):
        self.assertEqual(self.c2f(100), 212.0)

    def test_the_crossover(self):
        self.assertEqual(self.c2f(-40), -40.0)

    def test_body_temperature(self):
        self.assertEqual(self.c2f(37), 98.6)

    def test_rounds_to_two_places(self):
        self.assertEqual(self.c2f(36.666), 98.0)

    def test_round_trips_with_the_existing_conversion(self):
        from unitkit import fahrenheit_to_celsius

        self.assertEqual(fahrenheit_to_celsius(self.c2f(21.5)), 21.5)


if __name__ == "__main__":
    unittest.main()
