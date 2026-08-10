"""tokenapp command line."""

import sys

from . import auth


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if len(argv) < 2:
        print("usage: cli issue <user> | check <token>")
        return 2
    command, argument = argv[0], argv[1]
    if command == "issue":
        print(auth.issue({"user": argument}))
        return 0
    if command == "check":
        payload = auth.check(argument)
        if payload is None:
            print("invalid")
            return 1
        print(payload["user"])
        return 0
    print("unknown command %s" % command)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
