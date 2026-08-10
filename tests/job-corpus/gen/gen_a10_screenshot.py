#!/usr/bin/env python3
"""Build the A-10 screenshot fixture: the sub-case that demands an ACTION.

    python3 gen_a10_screenshot.py --out <dir>        (needs pillow)

This renders the approved design of a sign-up form. The design shows the
password rule the product is supposed to enforce, and the exact wording of the
error the user is supposed to see.

Neither the number nor the wording exists anywhere in the shipped code. They
exist only as pixels. So the job cannot be done by grepping the repository, and
it cannot be done by describing the image either -- the grader runs the
validator afterwards and checks the behaviour changed.

Two distractors are drawn on purpose:
  * the email field is outlined in red but carries no message -- email
    validation must come out unchanged
  * a satisfied checklist item mentions a different number (8), which is the
    number the code currently uses
"""

import argparse
import os

from PIL import Image, ImageDraw, ImageFont

W, H = 1120, 820
BG = (247, 248, 250)
CARD = (255, 255, 255)
INK = (28, 32, 38)
MUTED = (118, 126, 138)
LINE = (218, 223, 230)
DANGER = (198, 40, 40)
OK = (32, 138, 84)
BRAND = (42, 88, 214)

REQUIRED_LENGTH = 12
ERROR_TEXT = "Password must be at least 12 characters"

FONT_CANDIDATES = [
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
     "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"),
    ("/System/Library/Fonts/Supplemental/Arial.ttf",
     "/System/Library/Fonts/Supplemental/Arial Bold.ttf"),
    ("C:\\Windows\\Fonts\\arial.ttf", "C:\\Windows\\Fonts\\arialbd.ttf"),
]


def fonts():
    for regular, bold in FONT_CANDIDATES:
        if os.path.exists(regular) and os.path.exists(bold):
            return regular, bold
    raise SystemExit("no usable TrueType font found; install fonts-dejavu-core")


def build(path):
    regular, bold = fonts()
    f = lambda s: ImageFont.truetype(regular, s)   # noqa: E731
    fb = lambda s: ImageFont.truetype(bold, s)     # noqa: E731

    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)

    # Browser chrome, so it reads as a screenshot rather than a mock.
    d.rectangle([0, 0, W, 56], fill=(236, 239, 243))
    for i, colour in enumerate([(255, 95, 86), (255, 189, 46), (39, 201, 63)]):
        d.ellipse([22 + i * 26, 21, 36 + i * 26, 35], fill=colour)
    d.rounded_rectangle([120, 14, W - 30, 42], 8, fill=(255, 255, 255), outline=LINE)
    d.text((136, 20), "https://app.northwind.example/signup", font=f(15), fill=MUTED)

    # Card
    d.rounded_rectangle([290, 100, 830, 750], 14, fill=CARD, outline=LINE, width=2)
    d.text((330, 140), "Create your account", font=fb(30), fill=INK)
    d.text((330, 186), "Step 2 of 3", font=f(16), fill=MUTED)

    # Email field -- red outline, deliberately no message. A distractor.
    d.text((330, 240), "Work email", font=fb(16), fill=INK)
    d.rounded_rectangle([330, 266, 790, 312], 8, fill=(255, 255, 255), outline=DANGER, width=2)
    d.text((346, 280), "dana.whitfield@", font=f(17), fill=INK)

    # Password field -- the error that matters.
    d.text((330, 352), "Password", font=fb(16), fill=INK)
    d.rounded_rectangle([330, 378, 790, 424], 8, fill=(255, 255, 255), outline=DANGER, width=2)
    d.text((346, 392), "\u2022" * 11, font=f(19), fill=INK)
    d.text((330, 436), "\u26a0", font=f(17), fill=DANGER)
    d.text((356, 436), ERROR_TEXT, font=fb(17), fill=DANGER)

    # Requirement checklist. The satisfied line names 8 -- the number the code
    # currently uses -- so lifting a number out of the picture carelessly is
    # punished.
    d.text((330, 486), "Your password needs to:", font=f(15), fill=MUTED)
    checklist = [
        (False, "be at least %d characters long" % REQUIRED_LENGTH),
        (True, "contain at least 1 number"),
        (True, "contain at least 8 letters"),
    ]
    y = 514
    for satisfied, label in checklist:
        d.text((334, y), "\u2713" if satisfied else "\u2715", font=fb(16),
               fill=OK if satisfied else DANGER)
        d.text((360, y), label, font=f(16), fill=MUTED if satisfied else DANGER)
        y += 28

    d.rounded_rectangle([330, 620, 790, 672], 8, fill=(196, 205, 222))
    d.text((520, 636), "Continue", font=fb(18), fill=(255, 255, 255))
    d.text((330, 694), "Already have an account? Sign in", font=f(15), fill=BRAND)

    d.text((30, H - 34), "Approved design - signup-step-2 - revision 7", font=f(14), fill=MUTED)

    img.save(path, "PNG", optimize=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    path = os.path.join(args.out, "signup-step-2.png")
    build(path)
    print("wrote %s (%d bytes)" % (path, os.path.getsize(path)))
    print("required length in the picture: %d" % REQUIRED_LENGTH)
    print("error wording in the picture:   %s" % ERROR_TEXT)


if __name__ == "__main__":
    main()
