#!/usr/bin/env python3
"""Build the deterministic Phase 27 intake corpus.

Every entry is generated from pinned bytes so the corpus is byte-stable across
platforms and regenerations. Nothing here downloads, nothing here is random.

Run from the repository root:
    python3 .planning/scripts/f27-build-intake-corpus.py
"""

import hashlib
import pathlib
import struct
import sys
import zlib

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / "crates" / "wcore-fixture-harness" / "fixtures" / "f27" / "intake"

# The composer/vision path refuses anything under VISION_MIN_BYTES (16) and
# anything over VISION_MAX_BYTES (20 MiB). Both boundaries are exercised.
VISION_MIN_BYTES = 16
VISION_MAX_BYTES = 20 * 1024 * 1024

SENTINEL = "F27INTAKESENTINEL"


def png(width: int, height: int, payload: bytes = b"") -> bytes:
    """A real, decodable 8-bit RGB PNG plus an optional trailing payload."""

    def chunk(kind: bytes, body: bytes) -> bytes:
        return (
            struct.pack(">I", len(body))
            + kind
            + body
            + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    raw = b"".join(b"\x00" + b"\x7f\x20\x40" * width for _ in range(height))
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
        + payload
    )


def jpeg(payload: bytes = b"") -> bytes:
    """A minimal JFIF-headed JPEG. Enough for magic-byte admission."""
    return (
        b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00"
        + payload
        + b"\xff\xd9"
    )


def pdf(sentinel: str) -> bytes:
    """A hand-built single-page PDF carrying `sentinel` as extractable text."""
    stream = f"BT /F1 24 Tf 72 700 Td ({sentinel}) Tj ET".encode()
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
        b"/Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
        b"<< /Length " + str(len(stream)).encode() + b" >>\nstream\n" + stream + b"\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    out = bytearray(b"%PDF-1.4\n")
    offsets = []
    for i, body in enumerate(objs, start=1):
        offsets.append(len(out))
        out += f"{i} 0 obj\n".encode() + body + b"\nendobj\n"
    xref_at = len(out)
    out += f"xref\n0 {len(objs) + 1}\n".encode()
    out += b"0000000000 65535 f \n"
    for off in offsets:
        out += f"{off:010d} 00000 n \n".encode()
    out += (
        f"trailer\n<< /Size {len(objs) + 1} /Root 1 0 R >>\nstartxref\n{xref_at}\n".encode()
        + b"%%EOF\n"
    )
    return bytes(out)


def ooxml(kind: str) -> bytes:
    """A minimal but structurally real OOXML container for docx/xlsx/pptx."""
    import io
    import zipfile

    part, body = {
        "docx": (
            "word/document.xml",
            '<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/'
            f'wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{SENTINEL}</w:t></w:r>'
            "</w:p></w:body></w:document>",
        ),
        "xlsx": (
            "xl/sharedStrings.xml",
            '<?xml version="1.0"?><sst xmlns="http://schemas.openxmlformats.org/'
            f'spreadsheetml/2006/main" count="1" uniqueCount="1"><si><t>{SENTINEL}</t></si></sst>',
        ),
        "pptx": (
            "ppt/slides/slide1.xml",
            '<?xml version="1.0"?><p:sld xmlns:a="http://schemas.openxmlformats.org/'
            'drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/'
            "presentationml/2006/main\"><p:cSld><p:spTree><a:t>"
            f"{SENTINEL}</a:t></p:spTree></p:cSld></p:sld>",
        ),
    }[kind]

    buf = io.BytesIO()
    # Fixed date_time so the archive bytes are reproducible.
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in (
            (
                "[Content_Types].xml",
                '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/'
                'package/2006/content-types"><Default Extension="xml" '
                'ContentType="application/xml"/></Types>',
            ),
            (part, body),
        ):
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o600 << 16
            z.writestr(info, data)
    return buf.getvalue()


def build() -> dict[str, bytes]:
    valid_png = png(8, 8)
    valid_jpeg = jpeg(b"F27-JPEG-BODY")
    valid_pdf = pdf(SENTINEL)

    entries: dict[str, bytes] = {
        # --- valid, per class -------------------------------------------------
        "valid-image.png": valid_png,
        "valid-image.jpg": valid_jpeg,
        "valid-doc.pdf": valid_pdf,
        "valid-doc.docx": ooxml("docx"),
        "valid-doc.xlsx": ooxml("xlsx"),
        "valid-doc.pptx": ooxml("pptx"),
        # --- extension disagrees with bytes ----------------------------------
        "mismatch-png-body-jpg-ext.jpg": valid_png,
        "mismatch-jpeg-body-png-ext.png": valid_jpeg,
        "mismatch-not-a-pdf.pdf": b"NOT-A-PDF " + b"x" * 64 + SENTINEL.encode(),
        "mismatch-not-a-container.docx": b"NOT-A-ZIP " + b"y" * 64,
        # --- no extension at all ---------------------------------------------
        "noext-png-body": valid_png,
        # --- truncated mid-header --------------------------------------------
        "truncated-header.png": valid_png[:6],
        "truncated-header.pdf": valid_pdf[:4],
        # --- zero byte, one per class ----------------------------------------
        "empty.png": b"",
        "empty.pdf": b"",
        "empty.docx": b"",
        # --- size-cap boundaries for the vision floor ------------------------
        # VISION_MIN_BYTES is 16: one entry lands just under and one just on it.
        "boundary-under-vision-min.png": b"\x89PNG\r\n\x1a\n" + b"\x00" * (VISION_MIN_BYTES - 8 - 1),
        "boundary-at-vision-min.png": b"\x89PNG\r\n\x1a\n" + b"\x00" * (VISION_MIN_BYTES - 8),
    }
    assert len(entries["boundary-under-vision-min.png"]) == VISION_MIN_BYTES - 1
    assert len(entries["boundary-at-vision-min.png"]) == VISION_MIN_BYTES
    return entries


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    entries = build()
    manifest = [
        "# Phase 27 deterministic intake corpus.",
        "# Generated by .planning/scripts/f27-build-intake-corpus.py — do not hand-edit.",
        f"# VISION_MIN_BYTES={VISION_MIN_BYTES} VISION_MAX_BYTES={VISION_MAX_BYTES}",
        f"# SENTINEL={SENTINEL}",
        "name\tbytes\tsha256",
    ]
    for name in sorted(entries):
        data = entries[name]
        (OUT / name).write_bytes(data)
        manifest.append(f"{name}\t{len(data)}\t{hashlib.sha256(data).hexdigest()}")
    (OUT / "MANIFEST.tsv").write_text("\n".join(manifest) + "\n")
    print(f"wrote {len(entries)} corpus entries + MANIFEST.tsv to {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
