import unittest

from unitkit import fahrenheit_to_celsius


class FahrenheitToCelsius(unittest.TestCase):
    def test_freezing(self):
        self.assertEqual(fahrenheit_to_celsius(32), 0.0)

    def test_boiling(self):
        self.assertEqual(fahrenheit_to_celsius(212), 100.0)

    def test_the_crossover(self):
        self.assertEqual(fahrenheit_to_celsius(-40), -40.0)

    def test_rounds_to_two_places(self):
        self.assertEqual(fahrenheit_to_celsius(98.6), 37.0)


if __name__ == "__main__":
    unittest.main()
