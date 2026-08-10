#!/usr/bin/env python3
"""Build the A-10 scanned-PDF fixture: an invoice that went through a bad scanner.

    python3 gen_a10_scanned_pdf.py --out <dir>       (needs pillow + reportlab)

The page is rendered as text, then rotated off-square, speckled, blurred and
saved as a middling-quality JPEG before being wrapped in a PDF. There is no
text layer at all: the only way to answer is to read the pixels.

One line is deliberately printed larger than the rest -- the OCR control line.
It carries a nonsense phrase that exists nowhere else. If a reader cannot
recover the control line, the scan is harsher than any real scan and the
sub-case is UNPROVEN rather than failed. That keeps a fixture-quality problem
from being charged to the product.
"""

import argparse
import os
import random

from PIL import Image, ImageDraw, ImageFilter, ImageFont
from reportlab.lib.pagesizes import A4
from reportlab.lib.utils import ImageReader
from reportlab.pdfgen import canvas

WIDTH, HEIGHT = 1700, 2340  # roughly 200 dpi A4
ROTATION_DEGREES = -4.6
SEED = 20260312

FONT_CANDIDATES = [
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "C:\\Windows\\Fonts\\arial.ttf",
]

# Letters only on purpose. At this blur the digit 1 reliably degrades to a
# bracket, and the control line must not fail for a reason the graded fields
# would not fail for.
CONTROL_PHRASE = "OCR CONTROL LINE ZEBRA QUARTZ HALYARD"

LINES = [
    (90, 150, 64, "MERIDIAN FREIGHT SERVICES LTD"),
    (90, 230, 34, "Unit 14, Ashgrove Industrial Estate, Bristol BS11 9QT"),
    (90, 280, 34, "VAT Registration No: GB 384 2915 77"),
    (90, 400, 52, "INVOICE"),
    (90, 480, 38, "Invoice number: INV-2026-04817"),
    (90, 535, 38, "Invoice date: 12 March 2026"),
    (90, 590, 38, "Payment due: 11 April 2026"),
    (90, 645, 38, "Purchase order: PO-77341"),
    (90, 760, 38, "Bill to: Halverson Retail Group, 22 Cheapside, London EC2V 6DN"),
    (90, 880, 40, "Description                          Qty        Unit        Amount"),
    (90, 935, 38, "Pallet handling                      240        24.50       5,880.00"),
    (90, 990, 38, "Cross-dock transfer                   96        31.25       3,000.00"),
    (90, 1045, 38, "Temperature controlled storage        14       118.00       1,652.00"),
    (90, 1100, 38, "Fuel surcharge                         1     1,464.50       1,464.50"),
    (90, 1210, 38, "Subtotal                                                  11,996.50"),
    (90, 1265, 38, "VAT at 20%                                                 2,399.30"),
    (90, 1320, 44, "Total due                                                 14,395.80"),
    (90, 1440, 34, "Remit to: Meridian Freight Services Ltd, sort code 20-45-11"),
    (90, 1490, 34, "Account 60418822. Please quote the invoice number."),
    (90, 1620, 56, CONTROL_PHRASE),
    (90, 1740, 32, "Registered in England and Wales, company number 04418271."),
]


def load_font(size):
    for path in FONT_CANDIDATES:
        if os.path.exists(path):
            return ImageFont.truetype(path, size)
    raise SystemExit("no usable TrueType font found; install fonts-dejavu-core")


def render_page():
    image = Image.new("L", (WIDTH, HEIGHT), 255)
    draw = ImageDraw.Draw(image)
    for x, y, size, text in LINES:
        draw.text((x, y), text, font=load_font(size), fill=30)
    return image


def degrade(image):
    rng = random.Random(SEED)

    # An off-square feed, on a slightly grey scanner bed.
    image = image.rotate(ROTATION_DEGREES, resample=Image.BICUBIC, expand=False, fillcolor=246)

    # Paper texture and sensor speckle.
    pixels = image.load()
    for _ in range(int(WIDTH * HEIGHT * 0.012)):
        x = rng.randrange(WIDTH)
        y = rng.randrange(HEIGHT)
        pixels[x, y] = max(0, min(255, pixels[x, y] + rng.randint(-90, 60)))

    # A shadow down the gutter, the way a book scan falls off.
    for x in range(0, 120):
        shade = int(40 * (1 - x / 120.0))
        for y in range(0, HEIGHT, 2):
            pixels[x, y] = max(0, pixels[x, y] - shade)

    image = image.filter(ImageFilter.GaussianBlur(radius=0.7))
    return image


def build(path):
    image = degrade(render_page())
    jpeg_path = path.replace(".pdf", ".page1.jpg")
    image.convert("L").save(jpeg_path, "JPEG", quality=58, optimize=True)

    c = canvas.Canvas(path, pagesize=A4)
    c.setTitle("scan_20260312_0001")
    page_w, page_h = A4
    c.drawImage(ImageReader(jpeg_path), 0, 0, width=page_w, height=page_h)
    c.showPage()
    c.save()
    os.remove(jpeg_path)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    path = os.path.join(args.out, "scan_20260312_0001.pdf")
    build(path)
    print("wrote %s (%d bytes)" % (path, os.path.getsize(path)))
    print("rotation %.1f degrees, JPEG quality 58, no text layer" % ROTATION_DEGREES)
    print("control phrase: %s" % CONTROL_PHRASE)


if __name__ == "__main__":
    main()
