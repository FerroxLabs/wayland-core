"""Test doubles shared by the retry tests."""


class Response:
    def __init__(self, status, headers=None):
        self.status = status
        self.headers = headers or {}


class Transport:
    """Hands back canned responses and records how often it was called."""

    def __init__(self, responses):
        self.responses = list(responses)
        self.sends = 0

    def __call__(self, request):
        self.sends += 1
        if self.responses:
            return self.responses.pop(0)
        return Response(503)


class Clock:
    """Records every sleep instead of actually waiting."""

    def __init__(self):
        self.slept = []

    def __call__(self, seconds):
        self.slept.append(seconds)
