#!/usr/bin/env python3
"""Build the A-10 text-PDF fixture: a four-page operations review.

    python3 gen_a10_text_pdf.py --out <dir>          (needs reportlab)

Pages are broken explicitly so the page number of every fact is certain, which
is what makes "cite the page" gradeable.

Three of the five questions cannot be answered from a single page:

  * the SLA question needs the table on page 2 and the restatement on page 4
  * the load-per-engineer question needs two different tables on page 3
  * the deadline question needs the publication date on page 1 and the rule on
    page 4

A reader that stops at the first matching table gets a specific wrong answer,
and the key records which wrong answer means which mistake.
"""

import argparse
import os

from reportlab.lib.pagesizes import A4
from reportlab.lib.units import mm
from reportlab.pdfgen import canvas

WIDTH, HEIGHT = A4

SLA = [
    ("North America", "96.4%"),
    ("Europe", "97.1%"),
    ("Asia Pacific", "94.1%"),
    ("Latin America", "95.2%"),
]

BACKLOG = [
    ("North America", 812, 903, 774),
    ("Europe", 1105, 1198, 1284),
    ("Asia Pacific", 640, 587, 651),
    ("Latin America", 402, 455, 498),
]

HEADCOUNT = [
    ("North America", 9),
    ("Europe", 12),
    ("Asia Pacific", 7),
    ("Latin America", 5),
]


class Page:
    def __init__(self, c):
        self.c = c
        self.y = HEIGHT - 30 * mm

    def heading(self, text, size=16):
        self.c.setFont("Helvetica-Bold", size)
        self.c.drawString(25 * mm, self.y, text)
        self.y -= 10 * mm

    def para(self, text, size=10.5, leading=6 * mm):
        self.c.setFont("Helvetica", size)
        for line in wrap(text, 92):
            self.c.drawString(25 * mm, self.y, line)
            self.y -= leading
        self.y -= 2 * mm

    def row(self, cells, widths, bold=False):
        self.c.setFont("Helvetica-Bold" if bold else "Helvetica", 10.5)
        x = 25 * mm
        for cell, width in zip(cells, widths):
            self.c.drawString(x, self.y, str(cell))
            x += width
        self.y -= 6 * mm

    def rule(self):
        self.c.line(25 * mm, self.y + 2 * mm, WIDTH - 25 * mm, self.y + 2 * mm)
        self.y -= 3 * mm

    def footer(self, number):
        self.c.setFont("Helvetica", 8)
        self.c.drawCentredString(WIDTH / 2, 15 * mm, "Page %d of 4" % number)


def wrap(text, width):
    words = text.split()
    lines, current = [], ""
    for word in words:
        candidate = (current + " " + word).strip()
        if len(candidate) > width:
            lines.append(current)
            current = word
        else:
            current = candidate
    if current:
        lines.append(current)
    return lines


def build(path):
    c = canvas.Canvas(path, pagesize=A4, invariant=1)
    c.setTitle("Northwind Logistics - Q3 2026 Operations Review")
    c.setAuthor("Operations Board")

    # ---- Page 1 ----------------------------------------------------------
    p = Page(c)
    p.heading("Northwind Logistics", 20)
    p.heading("Q3 2026 Operations Review", 14)
    p.para("Published 15 September 2026. Prepared for the Operations Board.")
    p.para(
        "This review covers the third quarter of 2026 across our four operating "
        "regions. It reports on-time delivery against the service level target, "
        "the open support backlog at each month end, and the support staffing "
        "behind that backlog. Definitions and restatements are set out in the "
        "notes at the end of the document, and should be read together with the "
        "tables."
    )
    p.para(
        "Contents: page 2, on-time delivery performance. Page 3, support backlog "
        "and staffing. Page 4, notes, definitions and restatements."
    )
    p.para(
        "Summary. Delivery performance held above target in North America and "
        "Europe. The support backlog continued to grow in Europe for a third "
        "consecutive month. Two regions require remediation plans; see the notes "
        "on page 4 before drawing conclusions from the table on page 2."
    )
    p.footer(1)
    c.showPage()

    # ---- Page 2 ----------------------------------------------------------
    p = Page(c)
    p.heading("1. On-time delivery performance")
    p.para(
        "The service level target for on-time delivery is 95.0% in every region. "
        "Figures below are for the quarter as a whole, as reported at the "
        "cut-off."
    )
    p.row(["Region", "On-time, Q3 2026"], [70 * mm, 50 * mm], bold=True)
    p.rule()
    for region, value in SLA:
        p.row([region, value], [70 * mm, 50 * mm])
    p.rule()
    p.row(["Target", "95.0%"], [70 * mm, 50 * mm], bold=True)
    p.y -= 4 * mm
    p.para(
        "One region is shown below target in this table. See note 3 on page 4: "
        "one further region is restated below target once late-reported data is "
        "included."
    )
    p.footer(2)
    c.showPage()

    # ---- Page 3 ----------------------------------------------------------
    p = Page(c)
    p.heading("2. Support backlog and staffing")
    p.para(
        "Open support tickets at the end of each month. Backlog is measured at "
        "23:59 UTC on the last calendar day of the month; see note 2."
    )
    widths = [55 * mm, 25 * mm, 25 * mm, 25 * mm]
    p.row(["Region", "July", "August", "September"], widths, bold=True)
    p.rule()
    for region, jul, aug, sep in BACKLOG:
        p.row([region, jul, aug, sep], widths)
    p.rule()
    p.y -= 6 * mm
    p.heading("2.1 Support engineers in post", 12)
    p.para("Headcount at the end of September 2026.")
    p.row(["Region", "Support engineers"], [70 * mm, 50 * mm], bold=True)
    p.rule()
    for region, count in HEADCOUNT:
        p.row([region, count], [70 * mm, 50 * mm])
    p.rule()
    p.footer(3)
    c.showPage()

    # ---- Page 4 ----------------------------------------------------------
    p = Page(c)
    p.heading("3. Notes, definitions and restatements")
    p.para(
        "Note 1. On-time means a consignment delivered within the window promised "
        "to the customer at the point of booking. Redeliveries requested by the "
        "customer are excluded."
    )
    p.para(
        "Note 2. Backlog is the count of support tickets in an open state at "
        "23:59 UTC on the last calendar day of the month. Tickets awaiting "
        "customer response are counted as open."
    )
    p.para(
        "Note 3. Restatement. The Latin America on-time figure on page 2 excludes "
        "the Valparaiso depot, whose data arrived after the reporting cut-off. "
        "Including Valparaiso, Latin America on-time performance for Q3 2026 was "
        "93.8%."
    )
    p.para(
        "Note 4. Where a region finishes the quarter below the 95.0% on-time "
        "target, the regional director must deliver a remediation plan to the "
        "Operations Board within 60 days of the publication date shown on page 1."
    )
    p.para(
        "Note 5. Asia Pacific figures include the Singapore hub from July 2026 "
        "onward. Prior quarters are not directly comparable."
    )
    p.footer(4)
    c.showPage()

    c.save()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    path = os.path.join(args.out, "q3-2026-operations-review.pdf")
    build(path)
    print("wrote %s (%d bytes)" % (path, os.path.getsize(path)))

    sep_total = sum(row[3] for row in BACKLOG)
    print("September backlog total = %d" % sep_total)
    print("Europe September backlog per engineer = %.1f" % (1284 / 12.0))
    print("APAC miss = %.1f points; LATAM restated miss = %.1f points"
          % (95.0 - 94.1, 95.0 - 93.8))


if __name__ == "__main__":
    main()
