#!/usr/bin/env python3
"""Build the A-10 video fixture: a screen recording with one real incident.

    python3 gen_a10_video.py --out <dir>        (needs pillow + ffmpeg)

Twenty seconds of a service dashboard. Two things pop up:

  * at 5.0s an amber warning, W-1180, which clears at 8.0s -- the distractor
  * at 12.4s a red error, ERR-5521, which stays until the end -- the answer

Both look like incidents. Only one is the error. Answering with the warning,
or with 5.0 seconds, means the recording was skimmed rather than watched, and
the key records that as a specific failure rather than a near miss.

Frame times are exact because every frame is drawn deliberately at a fixed
frame rate, so the true onset is arithmetic, not an estimate.
"""

import argparse
import math
import os
import shutil
import subprocess
import tempfile

from PIL import Image, ImageDraw, ImageFont

W, H = 960, 540
FPS = 12
DURATION_S = 20.0

WARNING_START, WARNING_END = 5.0, 8.0
ERROR_START = 12.4

WARNING_CODE = "W-1180"
WARNING_TEXT = "cache warm-up slower than usual"
ERROR_CODE = "ERR-5521"
ERROR_TEXT = "payment gateway timeout"

BG = (18, 20, 26)
PANEL = (28, 31, 40)
GRID = (44, 48, 60)
INK = (226, 231, 240)
MUTED = (132, 140, 156)
GREEN = (46, 188, 116)
AMBER = (226, 164, 38)
RED = (226, 68, 68)

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


def latency(t):
    """A calm line that climbs once the gateway starts timing out."""
    base = 120 + 18 * math.sin(t * 0.9) + 6 * math.sin(t * 3.1)
    if t >= ERROR_START:
        base += min(430.0, (t - ERROR_START) * 150.0)
    elif WARNING_START <= t <= WARNING_END:
        base += 45
    return base


def draw_frame(index, regular, bold):
    t = index / float(FPS)
    f = lambda s: ImageFont.truetype(regular, s)   # noqa: E731
    fb = lambda s: ImageFont.truetype(bold, s)     # noqa: E731

    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)

    d.text((28, 22), "Northwind — service health", font=fb(22), fill=INK)
    d.text((28, 54), "checkout-api · production · eu-west-1", font=f(14), fill=MUTED)
    d.text((W - 150, 26), "%05.1fs" % t, font=f(16), fill=MUTED)

    # Latency chart
    d.rounded_rectangle([28, 92, W - 28, 360], 10, fill=PANEL)
    d.text((44, 104), "p95 request latency (ms)", font=f(14), fill=MUTED)
    for i in range(1, 5):
        y = 140 + i * 42
        d.line([44, y, W - 44, y], fill=GRID)

    points = []
    window = 20.0
    for px in range(0, W - 88):
        sample_t = max(0.0, t - window * (1 - px / float(W - 88)))
        value = latency(sample_t)
        y = 320 - min(220.0, value * 0.36)
        points.append((44 + px, y))
    colour = RED if t >= ERROR_START else (AMBER if WARNING_START <= t <= WARNING_END else GREEN)
    d.line(points, fill=colour, width=2)
    d.text((W - 190, 104), "now %d ms" % int(latency(t)), font=fb(15), fill=colour)

    # Status strip
    d.rounded_rectangle([28, 376, W - 28, 436], 10, fill=PANEL)
    if t >= ERROR_START:
        status, tint = "DEGRADED", RED
    elif WARNING_START <= t <= WARNING_END:
        status, tint = "WATCH", AMBER
    else:
        status, tint = "HEALTHY", GREEN
    d.ellipse([46, 398, 62, 414], fill=tint)
    d.text((76, 396), status, font=fb(20), fill=tint)
    d.text((76, 418), "checkout-api", font=f(13), fill=MUTED)

    # Toasts
    if WARNING_START <= t <= WARNING_END:
        d.rounded_rectangle([W - 430, 452, W - 28, 512], 10, fill=(58, 46, 16), outline=AMBER, width=2)
        d.text((W - 412, 462), WARNING_CODE, font=fb(16), fill=AMBER)
        d.text((W - 412, 486), WARNING_TEXT, font=f(14), fill=INK)
    if t >= ERROR_START:
        d.rounded_rectangle([28, 452, 430, 512], 10, fill=(62, 20, 20), outline=RED, width=2)
        d.text((46, 462), ERROR_CODE, font=fb(16), fill=RED)
        d.text((46, 486), ERROR_TEXT, font=f(14), fill=INK)

    return img


def build(path):
    if shutil.which("ffmpeg") is None:
        raise SystemExit("ffmpeg is required to build the video fixture")
    regular, bold = fonts()
    frames = int(DURATION_S * FPS)
    tmp = tempfile.mkdtemp(prefix="a10-video-")
    try:
        for i in range(frames):
            draw_frame(i, regular, bold).save(os.path.join(tmp, "f%05d.png" % i))
        subprocess.run(
            ["ffmpeg", "-y", "-loglevel", "error", "-framerate", str(FPS),
             "-i", os.path.join(tmp, "f%05d.png"),
             "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "26",
             "-movflags", "+faststart", path],
            check=True,
        )
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    path = os.path.join(args.out, "checkout-api-incident.mp4")
    build(path)
    print("wrote %s (%d bytes)" % (path, os.path.getsize(path)))
    print("distractor warning %s at %.1fs, clears %.1fs" % (WARNING_CODE, WARNING_START, WARNING_END))
    print("error %s first visible at %.4fs" % (ERROR_CODE, math.ceil(ERROR_START * FPS) / FPS))


if __name__ == "__main__":
    main()
