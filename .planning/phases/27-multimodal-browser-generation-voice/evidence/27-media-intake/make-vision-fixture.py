#!/usr/bin/env python3
"""Deterministic vision fixture for the 27-media-intake live leg.

Ground truth is an UNGUESSABLE token. A model that cannot actually see the
image cannot emit "VORTHAK" or "7492" — so recovering either from the response
proves the bytes reached a vision model, rather than proving the model is
good at guessing what a test image probably contains.

Regenerates byte-identically on any platform (fixed size, fixed colours, fixed
default bitmap font, no timestamp, PNG written with pinned parameters).

Usage:  python3 make-vision-fixture.py <outdir>
"""

import hashlib
import sys
from pathlib import Path

from PIL import Image, ImageDraw

# The ground truth. Deliberately not a word and not a plausible default.
TOKEN = "VORTHAK"
NUMBER = "7492"

W, H = 480, 240


def build() -> Image.Image:
    img = Image.new("RGB", (W, H), (255, 255, 255))
    d = ImageDraw.Draw(img)
    # Large text via the default bitmap font, scaled by drawing into a small
    # image and resizing with NEAREST — keeps output deterministic across
    # platforms (no system font dependency, no antialiasing variance).
    small = Image.new("RGB", (W // 4, H // 4), (255, 255, 255))
    ds = ImageDraw.Draw(small)
    ds.text((6, 14), TOKEN, fill=(0, 0, 0))
    ds.text((6, 34), NUMBER, fill=(0, 0, 0))
    big = small.resize((W, H), Image.NEAREST)
    img.paste(big, (0, 0))
    # A red triangle in the lower right — a second, shape-based channel of
    # evidence that does not depend on OCR.
    d = ImageDraw.Draw(img)
    d.polygon([(380, 200), (450, 200), (415, 140)], fill=(220, 0, 0))
    return img


def main() -> int:
    outdir = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
    outdir.mkdir(parents=True, exist_ok=True)
    out = outdir / "vision-fixture.png"
    build().save(out, format="PNG", optimize=False, compress_level=6)
    data = out.read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    print(f"path={out}")
    print(f"bytes={len(data)}")
    print(f"sha256={digest}")
    print(f"ground_truth_token={TOKEN}")
    print(f"ground_truth_number={NUMBER}")
    print("ground_truth_shape=red triangle")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
