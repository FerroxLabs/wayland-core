#!/usr/bin/env python3
"""Build the A-10 spreadsheet fixture and print the exact answer.

    python3 gen_a10_spreadsheet.py --out <dir>

Writes `regional-revenue-2026.xlsx` -- a real Office Open XML workbook built
from the standard library, so it regenerates byte-for-byte on any host with no
third-party packages. Also prints the arithmetic, which is transcribed into
`keys/a10_spreadsheet.key.json` by hand so the key and the fixture can be
reviewed against each other.

The data carries four traps on purpose:

  * a region label with a trailing space, and one in lower case
  * deals that are void or pending and must not be counted
  * amounts already denominated in EUR, which must not be converted again
  * an "Annual average" FX row that is not the rate to use

A naive sum over column C gets a different number from every one of them.
"""

import argparse
import os
import zipfile
from xml.sax.saxutils import escape

QUARTERS = {
    "Q1": [
        ("EMEA", "D-1001", 120000, "USD", "closed"),
        ("AMER", "D-1002", 250000, "USD", "closed"),
        ("EMEA", "D-1003", 45000, "EUR", "closed"),
        ("EMEA", "D-1004", 80000, "USD", "pending"),
        ("APAC", "D-1005", 60000, "USD", "closed"),
    ],
    "Q2": [
        ("EMEA", "D-2001", 210000, "USD", "closed"),
        ("EMEA", "D-2002", 30000, "USD", "void"),
        ("AMER", "D-2003", 175000, "USD", "closed"),
        ("EMEA", "D-2004", 64000, "EUR", "closed"),
    ],
    "Q3": [
        ("EMEA ", "D-3001", 98000, "USD", "closed"),
        ("EMEA", "D-3002", 143000, "USD", "closed"),
        ("APAC", "D-3003", 88000, "USD", "closed"),
        ("EMEA", "D-3004", 12000, "EUR", "pending"),
    ],
    "Q4": [
        ("EMEA", "D-4001", 305000, "USD", "closed"),
        ("emea", "D-4002", 27000, "EUR", "closed"),
        ("AMER", "D-4003", 410000, "USD", "closed"),
        ("EMEA", "D-4004", 55000, "USD", "void"),
    ],
}

FX = [
    ("Q1", 1.0850),
    ("Q2", 1.1020),
    ("Q3", 1.0765),
    ("Q4", 1.0940),
    ("Annual average", 1.0894),
]

NOTES = [
    ("Reporting rules",),
    ('Only deals with Status "closed" count as revenue. Void and pending deals are excluded.',),
    ("Amounts already denominated in EUR are reported as-is and must not be converted.",),
    ("Region labels are matched ignoring case and ignoring surrounding spaces.",),
    ("Convert each quarter at that quarter's rate on the FX Rates sheet. The annual average is for commentary only.",),
    ("Round the final figure to 2 decimal places. Do not round intermediate conversions.",),
]

HEADERS = ("Region", "Deal ID", "Amount", "Currency", "Status")
SHEETS = ["Q1", "Q2", "Q3", "Q4", "FX Rates", "Notes"]


def col_name(index):
    name = ""
    while index >= 0:
        name = chr(ord("A") + index % 26) + name
        index = index // 26 - 1
    return name


def cell_xml(ref, value):
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return '<c r="%s"><v>%s</v></c>' % (ref, repr(value) if isinstance(value, float) else value)
    return '<c r="%s" t="inlineStr"><is><t xml:space="preserve">%s</t></is></c>' % (
        ref, escape(str(value))
    )


def sheet_xml(rows):
    out = [
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>',
        '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">',
        "<sheetData>",
    ]
    for r, row in enumerate(rows, start=1):
        cells = "".join(
            cell_xml("%s%d" % (col_name(c), r), value) for c, value in enumerate(row)
        )
        out.append('<row r="%d">%s</row>' % (r, cells))
    out.append("</sheetData></worksheet>")
    return "".join(out)


def build(path):
    sheets = {}
    for quarter, deals in QUARTERS.items():
        sheets[quarter] = [HEADERS] + [list(d) for d in deals]
    sheets["FX Rates"] = [("Quarter", "USD per EUR")] + [list(r) for r in FX]
    sheets["Notes"] = [list(n) for n in NOTES]

    content_types = [
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>',
        '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">',
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>',
        '<Default Extension="xml" ContentType="application/xml"/>',
        '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>',
        '<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>',
    ]
    for i in range(1, len(SHEETS) + 1):
        content_types.append(
            '<Override PartName="/xl/worksheets/sheet%d.xml" '
            'ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>' % i
        )
    content_types.append("</Types>")

    workbook = [
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>',
        '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
        'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>',
    ]
    rels = [
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>',
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">',
    ]
    for i, name in enumerate(SHEETS, start=1):
        workbook.append('<sheet name="%s" sheetId="%d" r:id="rId%d"/>' % (escape(name), i, i))
        rels.append(
            '<Relationship Id="rId%d" Type="http://schemas.openxmlformats.org/officeDocument/'
            '2006/relationships/worksheet" Target="worksheets/sheet%d.xml"/>' % (i, i)
        )
    workbook.append("</sheets></workbook>")
    rels.append(
        '<Relationship Id="rId%d" Type="http://schemas.openxmlformats.org/officeDocument/'
        '2006/relationships/styles" Target="styles.xml"/>' % (len(SHEETS) + 1)
    )
    rels.append("</Relationships>")

    root_rels = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/'
        'relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>'
    )
    styles = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
        '<fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>'
        '<fills count="1"><fill><patternFill patternType="none"/></fill></fills>'
        '<borders count="1"><border/></borders>'
        '<cellStyleXfs count="1"><xf/></cellStyleXfs>'
        '<cellXfs count="1"><xf xfId="0"/></cellXfs></styleSheet>'
    )

    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as zf:
        # Fixed timestamps keep the archive reproducible across hosts.
        def write(name, data):
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 2, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o600 << 16
            zf.writestr(info, data)

        write("[Content_Types].xml", "".join(content_types))
        write("_rels/.rels", root_rels)
        write("xl/workbook.xml", "".join(workbook))
        write("xl/_rels/workbook.xml.rels", "".join(rels))
        write("xl/styles.xml", styles)
        for i, name in enumerate(SHEETS, start=1):
            write("xl/worksheets/sheet%d.xml" % i, sheet_xml(sheets[name]))


def compute():
    """The answer, worked out the way the Notes sheet says to."""
    rates = dict(FX)
    total = 0.0
    trace = []
    for quarter in ("Q1", "Q2", "Q3", "Q4"):
        rate = rates[quarter]
        for region, deal, amount, currency, status in QUARTERS[quarter]:
            if status.strip().lower() != "closed":
                continue
            if region.strip().lower() != "emea":
                continue
            if currency == "EUR":
                eur = float(amount)
                trace.append("%s %s %s EUR already, no conversion -> %.6f" % (quarter, deal, amount, eur))
            else:
                eur = amount / rate
                trace.append("%s %s %s USD / %.4f -> %.6f" % (quarter, deal, amount, rate, eur))
            total += eur
    return total, trace


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    path = os.path.join(args.out, "regional-revenue-2026.xlsx")
    build(path)
    total, trace = compute()
    print("wrote %s (%d bytes)" % (path, os.path.getsize(path)))
    for line in trace:
        print("  " + line)
    print("TOTAL EMEA 2026 in EUR = %.2f" % total)
    print("unrounded = %.10f" % total)


if __name__ == "__main__":
    main()
