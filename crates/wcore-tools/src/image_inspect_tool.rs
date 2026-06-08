//! T8 — read-only image metadata inspection tool.
//!
//! Plan v2 Tier 2B row "T8": "Image inspection (read-only metadata) —
//! Dimensions + EXIF; NO modification".
//!
//! Given an image file on the local filesystem, [`ImageInspectTool`]
//! reports:
//!
//! * pixel **dimensions** (width × height),
//! * the detected **format** (PNG / JPEG / GIF / BMP / WebP / TIFF / …),
//! * the **color type** (e.g. `Rgb8`, `Rgba8`, `L8`),
//! * any **EXIF metadata** carried in the file (camera make/model,
//!   capture timestamp, orientation, exposure, GPS, …).
//!
//! ## Backends — both pure-Rust, no native deps
//!
//! * Dimensions / format / color type come from the
//!   [`image`](https://crates.io/crates/image) crate. The header is read
//!   via [`image::ImageReader::with_guessed_format`] +
//!   [`image::ImageReader::into_decoder`]; **the full pixel buffer is
//!   never decoded** — only the format header is parsed, so even a huge
//!   image is inspected cheaply.
//! * EXIF comes from
//!   [`kamadak-exif`](https://crates.io/crates/kamadak-exif) (the `exif`
//!   crate). It parses the embedded TIFF/EXIF container without touching
//!   pixel data.
//!
//! Both backends are gated behind the **default-on** `image-inspect`
//! cargo feature (mirroring `pdf`). `image` pulls a non-trivial
//! transitive tree, so a downstream that does not need image inspection
//! can build with `--no-default-features` to drop it; `ImageInspectTool`
//! still registers and is schema-visible — it just returns an honest
//! "compiled without image-inspect support" error (NO-STUBS contract).
//!
//! ## Safety posture
//!
//! Strictly **read-only**. The tool never writes, re-encodes, or
//! modifies any image. The LLM-supplied `file_path` is validated via
//! [`crate::path_validation::validate_user_path`] before any filesystem
//! touch (same discipline as `ReadTool` / `PdfTool`): absolute paths
//! only, no traversal, no null bytes, system-secret deny-list.

use std::path::Path;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::path_validation::validate_user_path;

/// Read-only image-metadata inspection tool.
///
/// Stateless — one instance serves every call.
#[derive(Debug, Default, Clone, Copy)]
pub struct ImageInspectTool;

impl ImageInspectTool {
    /// Construct a new `ImageInspectTool`.
    pub fn new() -> Self {
        Self
    }
}

/// Cap on the number of EXIF fields rendered into the report. Real-world
/// images rarely carry more than ~50 tags; the cap bounds output for a
/// pathologically tag-stuffed file.
#[cfg(feature = "image-inspect")]
const MAX_EXIF_FIELDS: usize = 200;

/// Inspect the image at `disk_path` (already path-validated) and render a
/// structured plain-text report. `display_path` is the original
/// user-supplied string, used only in messages.
///
/// Real backend — `image-inspect` feature ON.
#[cfg(feature = "image-inspect")]
fn inspect(disk_path: &Path, display_path: &str) -> ToolResult {
    use image::ImageDecoder;
    use std::fmt::Write as _;

    // --- dimensions / format / color type via `image` (header only) ---
    //
    // `with_guessed_format` sniffs the format from magic bytes (so a
    // mis-extensioned file is still identified correctly); `into_decoder`
    // parses just the format header — it does NOT decode pixels.
    let reader = match image::ImageReader::open(disk_path) {
        Ok(r) => r,
        Err(e) => {
            return ToolResult {
                content: format!("Failed to open image {display_path}: {e}"),
                is_error: true,
            };
        }
    };
    let reader = match reader.with_guessed_format() {
        Ok(r) => r,
        Err(e) => {
            return ToolResult {
                content: format!("Failed to read image header for {display_path}: {e}"),
                is_error: true,
            };
        }
    };

    let format = reader.format();
    let decoder = match reader.into_decoder() {
        Ok(d) => d,
        Err(e) => {
            return ToolResult {
                content: format!(
                    "Not a recognized image, or unsupported format ({display_path}): {e}"
                ),
                is_error: true,
            };
        }
    };

    let (width, height) = decoder.dimensions();
    let color_type = decoder.color_type();

    let mut out = String::new();
    let _ = writeln!(out, "Image: {display_path}");
    let _ = writeln!(out, "Dimensions: {width} x {height} px");
    match format {
        Some(f) => {
            let _ = writeln!(out, "Format: {f:?}");
        }
        None => {
            let _ = writeln!(out, "Format: unknown");
        }
    }
    let _ = writeln!(out, "Color type: {color_type:?}");

    // --- EXIF via `kamadak-exif` ---
    //
    // EXIF is optional. Absence is NOT an error — many valid images
    // (e.g. freshly-encoded PNGs) carry no EXIF at all.
    out.push_str(&render_exif(disk_path));

    ToolResult {
        content: out,
        is_error: false,
    }
}

/// Render the EXIF section of the report. Returns a section that always
/// starts with a header line, so the caller can append it unconditionally.
#[cfg(feature = "image-inspect")]
fn render_exif(disk_path: &Path) -> String {
    use std::fmt::Write as _;
    use std::fs::File;
    use std::io::BufReader;

    let file = match File::open(disk_path) {
        Ok(f) => f,
        // The `image` read above already proved the file is openable; a
        // failure here is unexpected but non-fatal for the report.
        Err(e) => return format!("EXIF: could not re-open file ({e})\n"),
    };
    let mut bufreader = BufReader::new(&file);

    let exif = match exif::Reader::new().read_from_container(&mut bufreader) {
        Ok(e) => e,
        // `NotFound` is the common, expected case: no EXIF block present.
        Err(exif::Error::NotFound(_)) => return "EXIF: none\n".to_string(),
        Err(e) => return format!("EXIF: could not parse ({e})\n"),
    };

    let fields: Vec<&exif::Field> = exif.fields().collect();
    if fields.is_empty() {
        return "EXIF: none\n".to_string();
    }

    let mut out = String::new();
    let _ = writeln!(out, "EXIF: {} field(s)", fields.len());
    for field in fields.iter().take(MAX_EXIF_FIELDS) {
        let _ = writeln!(
            out,
            "  {} ({}): {}",
            field.tag,
            field.ifd_num,
            field.display_value().with_unit(&exif)
        );
    }
    if fields.len() > MAX_EXIF_FIELDS {
        let _ = writeln!(
            out,
            "  ... [{} more EXIF field(s) omitted]",
            fields.len() - MAX_EXIF_FIELDS
        );
    }
    out
}

/// Degraded backend — `image-inspect` feature OFF.
///
/// The tool still registers and is schema-visible to the LLM, but every
/// call fails loudly with an honest "compiled without image-inspect
/// support" message (NO-STUBS: an honest blocker, not a silent stub).
/// Mirrors how [`crate::pdf_tool`] degrades when its `pdf` feature is off.
#[cfg(not(feature = "image-inspect"))]
fn inspect(_disk_path: &Path, display_path: &str) -> ToolResult {
    ToolResult {
        content: format!(
            "Cannot inspect image {display_path}: this build of wcore-tools \
             was compiled without the `image-inspect` feature. Rebuild with \
             the default features (or `--features image-inspect`) to enable \
             image-metadata + EXIF inspection."
        ),
        is_error: true,
    }
}

#[async_trait]
impl Tool for ImageInspectTool {
    fn name(&self) -> &str {
        "image_inspect"
    }

    fn description(&self) -> &str {
        "Inspects an image file's metadata. Read-only — never modifies the image.\n\n\
         Reports:\n\
         - pixel dimensions (width x height),\n\
         - detected format (PNG, JPEG, GIF, BMP, WebP, TIFF, ...),\n\
         - color type (e.g. Rgb8, Rgba8, L8),\n\
         - EXIF metadata if present (camera make/model, capture time,\n\
           orientation, exposure, GPS, ...).\n\n\
         Usage:\n\
         - file_path must be an absolute path to an image file.\n\
         - Only the format header is read; large images are inspected cheaply.\n\
         - Absence of EXIF is reported as 'EXIF: none', not an error."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the image file to inspect"
                }
            },
            "required": ["file_path"],
            "additionalProperties": false
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Read-only filesystem access — safe alongside other tools.
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(file_path) = input.get("file_path").and_then(|v| v.as_str()) else {
            return ToolResult {
                content: "Missing required parameter: file_path".to_string(),
                is_error: true,
            };
        };

        // Same path discipline as ReadTool / PdfTool: absolute, no
        // traversal, no null bytes, system-secret deny-list.
        let validated = match validate_user_path(Path::new(file_path)) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    content: format!("Refused to inspect {file_path}: {e}"),
                    is_error: true,
                };
            }
        };

        if !validated.is_file() {
            return ToolResult {
                content: format!("Image not found or not a file: {file_path}"),
                is_error: true,
            };
        }

        inspect(&validated, file_path)
    }

    fn category(&self) -> ToolCategory {
        // Read-only file inspection — mirrors ReadTool / PdfTool.
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        format!("Inspect image metadata for {path}")
    }
}

/// Schema / metadata tests — run regardless of the `image-inspect`
/// feature, since they exercise no image backend.
#[cfg(test)]
mod schema_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn schema_and_metadata_are_well_formed() {
        let tool = ImageInspectTool::new();
        assert_eq!(tool.name(), "image_inspect");
        assert!(tool.is_concurrency_safe(&json!({})));
        let schema = tool.input_schema();
        assert_eq!(schema["required"][0], "file_path");
        assert!(
            tool.describe(&json!({"file_path": "/x.png"}))
                .contains("/x.png")
        );
    }

    #[tokio::test]
    async fn missing_file_path_param_returns_error() {
        let tool = ImageInspectTool::new();
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("file_path"));
    }

    #[tokio::test]
    async fn relative_path_is_refused() {
        let tool = ImageInspectTool::new();
        let result = tool
            .execute(json!({ "file_path": "relative/pic.png" }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("Refused"));
    }

    /// When the crate is built without the `image-inspect` feature, the
    /// tool must still register and fail loudly with an honest message.
    #[cfg(not(feature = "image-inspect"))]
    #[tokio::test]
    async fn degrades_gracefully_without_image_inspect_feature() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("pic.png");
        std::fs::write(&path, b"not really a png").unwrap();

        let tool = ImageInspectTool::new();
        let result = tool
            .execute(json!({ "file_path": path.to_str().unwrap() }))
            .await;

        assert!(result.is_error);
        assert!(
            result
                .content
                .contains("without the `image-inspect` feature")
        );
    }
}

#[cfg(all(test, feature = "image-inspect"))]
mod tests {
    use super::*;
    use image::{ColorType, ImageEncoder, ImageFormat};
    use serde_json::json;
    use std::io::Cursor;
    use tempfile::TempDir;

    /// Encode a tiny solid 2x2 RGB image to in-memory bytes in `format`.
    fn encode_2x2(format: ImageFormat) -> Vec<u8> {
        // 2x2 RGB8 — 12 bytes of pixel data.
        let pixels: [u8; 12] = [
            255, 0, 0, 0, 255, 0, // row 0
            0, 0, 255, 255, 255, 0, // row 1
        ];
        let mut buf = Cursor::new(Vec::new());
        match format {
            ImageFormat::Png => {
                image::codecs::png::PngEncoder::new(&mut buf)
                    .write_image(&pixels, 2, 2, ColorType::Rgb8.into())
                    .expect("PNG encode");
            }
            ImageFormat::Jpeg => {
                image::codecs::jpeg::JpegEncoder::new(&mut buf)
                    .write_image(&pixels, 2, 2, ColorType::Rgb8.into())
                    .expect("JPEG encode");
            }
            other => panic!("unsupported test format: {other:?}"),
        }
        buf.into_inner()
    }

    /// Build a minimal valid JPEG that carries an APP1/EXIF segment with a
    /// single ASCII `Make` tag ("Wayland"). Hand-assembled because the
    /// `image` crate does not write EXIF.
    ///
    /// Layout: SOI, APP1(EXIF), then a real (EXIF-free) JPEG body minus
    /// its own SOI — kamadak-exif only needs to find the APP1 segment.
    fn jpeg_with_exif() -> Vec<u8> {
        // --- TIFF/EXIF payload: header + one IFD with one ASCII field ---
        // Big-endian TIFF.
        let mut tiff: Vec<u8> = Vec::new();
        tiff.extend_from_slice(b"MM"); // big-endian
        tiff.extend_from_slice(&[0x00, 0x2A]); // TIFF magic 42
        tiff.extend_from_slice(&[0x00, 0x00, 0x00, 0x08]); // IFD0 at offset 8

        // IFD0: 1 entry.
        tiff.extend_from_slice(&[0x00, 0x01]); // entry count = 1
        // Entry: Make (0x010F), type ASCII (2), count 8 ("Wayland\0").
        tiff.extend_from_slice(&[0x01, 0x0F]); // tag = Make
        tiff.extend_from_slice(&[0x00, 0x02]); // type = ASCII
        tiff.extend_from_slice(&[0x00, 0x00, 0x00, 0x08]); // count = 8
        // Value is 8 bytes (> 4), so the next 4 bytes are an offset.
        // IFD0 starts at 8; entry-count(2) + entry(12) + next-IFD(4) = 18,
        // so the value lives at offset 8 + 2 + 12 + 4 = 26.
        tiff.extend_from_slice(&[0x00, 0x00, 0x00, 0x1A]); // value offset = 26
        tiff.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // next IFD = 0 (none)
        tiff.extend_from_slice(b"Wayland\0"); // the ASCII value, 8 bytes

        // --- APP1 segment wrapping the EXIF identifier + TIFF payload ---
        let mut app1_body: Vec<u8> = Vec::new();
        app1_body.extend_from_slice(b"Exif\0\0");
        app1_body.extend_from_slice(&tiff);
        // APP1 length field counts the length bytes themselves (+2).
        let app1_len = (app1_body.len() + 2) as u16;

        // --- assemble the JPEG: SOI + APP1 + (body of a real JPEG) ---
        let mut jpeg: Vec<u8> = Vec::new();
        jpeg.extend_from_slice(&[0xFF, 0xD8]); // SOI
        jpeg.extend_from_slice(&[0xFF, 0xE1]); // APP1 marker
        jpeg.extend_from_slice(&app1_len.to_be_bytes());
        jpeg.extend_from_slice(&app1_body);

        // Append a genuine JPEG's content (drop its leading SOI so we
        // don't have two). This keeps the file decodable by `image`.
        let real = encode_2x2(ImageFormat::Jpeg);
        jpeg.extend_from_slice(&real[2..]);
        jpeg
    }

    #[tokio::test]
    async fn reports_png_dimensions_and_format() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pic.png");
        std::fs::write(&path, encode_2x2(ImageFormat::Png)).unwrap();

        let tool = ImageInspectTool::new();
        let result = tool
            .execute(json!({ "file_path": path.to_str().unwrap() }))
            .await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(
            result.content.contains("Dimensions: 2 x 2 px"),
            "missing dimensions in: {}",
            result.content
        );
        assert!(
            result.content.contains("Format: Png"),
            "missing format in: {}",
            result.content
        );
        assert!(
            result.content.contains("Color type:"),
            "missing color type in: {}",
            result.content
        );
        // A freshly-encoded PNG has no EXIF.
        assert!(
            result.content.contains("EXIF: none"),
            "expected no EXIF for plain PNG: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn reports_jpeg_dimensions_and_format() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pic.jpg");
        std::fs::write(&path, encode_2x2(ImageFormat::Jpeg)).unwrap();

        let tool = ImageInspectTool::new();
        let result = tool
            .execute(json!({ "file_path": path.to_str().unwrap() }))
            .await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("Dimensions: 2 x 2 px"));
        assert!(
            result.content.contains("Format: Jpeg"),
            "missing JPEG format in: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn reports_exif_when_present() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("exif.jpg");
        std::fs::write(&path, jpeg_with_exif()).unwrap();

        let tool = ImageInspectTool::new();
        let result = tool
            .execute(json!({ "file_path": path.to_str().unwrap() }))
            .await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(
            result.content.contains("EXIF:") && !result.content.contains("EXIF: none"),
            "expected EXIF fields in: {}",
            result.content
        );
        assert!(
            result.content.contains("Make") && result.content.contains("Wayland"),
            "expected the embedded Make=Wayland tag in: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn reports_no_exif_for_plain_image() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("plain.png");
        std::fs::write(&path, encode_2x2(ImageFormat::Png)).unwrap();

        let tool = ImageInspectTool::new();
        let result = tool
            .execute(json!({ "file_path": path.to_str().unwrap() }))
            .await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(
            result.content.contains("EXIF: none"),
            "expected 'EXIF: none' for a plain PNG: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn non_image_file_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("not_an_image.txt");
        std::fs::write(&path, b"this is plain text, definitely not an image").unwrap();

        let tool = ImageInspectTool::new();
        let result = tool
            .execute(json!({ "file_path": path.to_str().unwrap() }))
            .await;

        assert!(
            result.is_error,
            "a non-image file should produce an error: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn missing_file_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("does_not_exist.png");

        let tool = ImageInspectTool::new();
        let result = tool
            .execute(json!({ "file_path": path.to_str().unwrap() }))
            .await;

        assert!(result.is_error);
        assert!(
            result.content.contains("not found"),
            "expected 'not found' in: {}",
            result.content
        );
    }
}
