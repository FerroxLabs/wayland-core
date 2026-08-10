#!/usr/bin/env python3
"""Build the A-10 TUI attachment fixtures -- the leg nobody has ever tested.

    python3 gen_a10_tui_attach.py --out <dir>     (PNG needs pillow; PDF does not)

This is deliberately NOT about understanding a document. It is about whether a
file a person drops into the running TUI arrives at all. A backend unit test
that parses a PDF proves nothing about that, and on Windows in particular the
path never survives the trip intact.

Every artifact carries a canary: a short token that appears inside the file and
**nowhere in its name or its path**. So the reply can only contain the canary
if the bytes were actually read. Echoing the path back, or describing what the
file is probably about, cannot fake it.

The four locations exist because each one has broken a real product:

  plain/                     the easy case, and the control
  with space/                a directory with a space -- drag-drop quoting
  it's a "quoted" folder/    an apostrophe and double quotes in a directory name
  ünïcødé-文書/               non-ASCII path components

If `plain/` fails, the feature is broken outright. If only the others fail, the
feature works and the quoting does not, which is a different defect with a
different owner.
"""

import argparse
import os

CANARIES = {
    "pdf": "CANARY-PDF-TROMBONE-4417",
    "png": "CANARY-PNG-SEAGRASS-8830",
    "txt": "CANARY-TXT-MARIGOLD-2093",
}

DIRECTORIES = [
    ("plain", "plain"),
    ("space", "with space"),
    ("quotes", "it's a \"quoted\" folder"),
    ("unicode", "\u00fcn\u00efc\u00f8d\u00e9-\u6587\u66f8"),
]

# Names chosen so the canary is nowhere in the path, and so the file name
# itself also exercises quoting.
FILENAMES = {
    "pdf": "Q3 report (final)'s copy.pdf",
    "png": "screen shot 2026-08-10 at 14.02.png",
    "txt": "notes - draft #2.txt",
}


def make_pdf(path, canary):
    """A minimal, valid, single-page PDF with the canary as visible text."""
    content = ("BT /F1 24 Tf 60 700 Td (%s) Tj T* ET" % canary).encode("ascii")
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] "
        b"/Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
        b"<< /Length %d >>\nstream\n%s\nendstream" % (len(content), content),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    out = bytearray(b"%PDF-1.4\n")
    offsets = []
    for number, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += ("%d 0 obj\n" % number).encode("ascii") + body + b"\nendobj\n"
    xref_at = len(out)
    out += ("xref\n0 %d\n" % (len(objects) + 1)).encode("ascii")
    out += b"0000000000 65535 f \n"
    for offset in offsets:
        out += ("%010d 00000 n \n" % offset).encode("ascii")
    out += ("trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n"
            % (len(objects) + 1, xref_at)).encode("ascii")
    with open(path, "wb") as fh:
        fh.write(bytes(out))


def make_png(path, canary):
    from PIL import Image, ImageDraw, ImageFont

    candidates = [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        "C:\\Windows\\Fonts\\arialbd.ttf",
    ]
    font = None
    for candidate in candidates:
        if os.path.exists(candidate):
            font = ImageFont.truetype(candidate, 44)
            break
    if font is None:
        raise SystemExit("no usable TrueType font found")
    img = Image.new("RGB", (900, 260), (255, 255, 255))
    d = ImageDraw.Draw(img)
    d.text((40, 60), canary, font=font, fill=(15, 15, 15))
    d.text((40, 140), "attachment probe", font=font, fill=(120, 120, 120))
    img.save(path, "PNG", optimize=True)


def make_txt(path, canary):
    with open(path, "w", encoding="utf-8") as fh:
        fh.write("Attachment probe.\n\n%s\n\nNothing else here.\n" % canary)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--skip-png", action="store_true")
    args = ap.parse_args()

    made = []
    for label, directory in DIRECTORIES:
        target = os.path.join(args.out, directory)
        os.makedirs(target, exist_ok=True)
        for kind, filename in FILENAMES.items():
            if kind == "png" and args.skip_png:
                continue
            path = os.path.join(target, filename)
            canary = CANARIES[kind]
            if kind == "pdf":
                make_pdf(path, canary)
            elif kind == "png":
                make_png(path, canary)
            else:
                make_txt(path, canary)
            assert canary not in path, "the canary must not appear in the path"
            made.append((label, kind, path, canary))

    for label, kind, path, canary in made:
        print("%-8s %-4s %-9s %s" % (label, kind, canary.split("-")[2], path))
    print("\n%d artifacts. No canary appears in any file name or path." % len(made))


if __name__ == "__main__":
    main()
