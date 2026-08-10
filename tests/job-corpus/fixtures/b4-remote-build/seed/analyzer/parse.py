"""Parse the access-log dialect this team ships."""

import re

LINE = re.compile(
    r"^(?P<ip>\S+) (?P<ident>\S+) (?P<user>\S+) \[(?P<ts>[^\]]+)\] "
    r'"(?P<method>[A-Z]+) (?P<path>\S+) (?P<proto>[^"]+)" '
    r"(?P<status>\d{3}) (?P<bytes>\d+|-)$"
)


class ParseError(ValueError):
    pass


def parse_line(line):
    m = LINE.match(line.strip())
    if not m:
        raise ParseError("unparseable line: %r" % (line[:80],))
    d = m.groupdict()
    d["status"] = int(d["status"])
    d["bytes"] = 0 if d["bytes"] == "-" else int(d["bytes"])
    return d


def parse(lines):
    out, bad = [], 0
    for line in lines:
        if not line.strip():
            continue
        try:
            out.append(parse_line(line))
        except ParseError:
            bad += 1
    return out, bad
