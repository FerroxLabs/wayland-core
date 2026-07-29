//! JSON-stream composer attachment lowering.
//!
//! Local paths are resolved at the host protocol boundary so providers never
//! receive filesystem paths. The shared `wcore_tools::media_intake` chokepoint
//! owns path validation, the open-once walk, the byte caps and the magic-byte
//! decision — including the extension-versus-bytes cross-check, which this
//! module used to re-implement against its own extension table. This module
//! owns only the composer-specific PNG/JPEG contract, the aggregate byte
//! budget and the provider-neutral base64 projection.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use thiserror::Error;
use wcore_tools::media_intake::IntakeError;
use wcore_tools::vision_tools::{
    admit_local_image, image_intake_policy, render_image_intake_error,
};
use wcore_types::message::ContentBlock;

pub const COMPOSER_MAX_FILES: usize = 8;
pub const COMPOSER_MAX_TOTAL_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AttachmentError {
    #[error("composer message has {count} attachments; at most {limit} are accepted")]
    TooManyFiles { count: usize, limit: usize },
    #[error(
        "composer attachments total more than {limit} bytes; reduce their size or attach fewer files"
    )]
    AggregateTooLarge { limit: usize },
    #[error("attachment {index} ({path}) could not be loaded: {reason}")]
    Load {
        index: usize,
        path: String,
        reason: String,
    },
    #[error(
        "attachment {index} ({path}) has unsupported MIME {mime}; only PNG and JPEG are accepted"
    )]
    UnsupportedMime {
        index: usize,
        path: String,
        mime: String,
    },
    #[error("attachment {index} ({path}) extension declares {declared} but bytes are {detected}")]
    MimeMismatch {
        index: usize,
        path: String,
        declared: &'static str,
        detected: String,
    },
}

/// Load composer paths in their exact wire order and project them to inline
/// provider-neutral image blocks. An empty list is a no-op.
pub fn load_composer_images(files: &[String]) -> Result<Vec<ContentBlock>, AttachmentError> {
    if files.len() > COMPOSER_MAX_FILES {
        return Err(AttachmentError::TooManyFiles {
            count: files.len(),
            limit: COMPOSER_MAX_FILES,
        });
    }

    let policy = image_intake_policy();
    let mut total_bytes = 0usize;
    let mut images = Vec::with_capacity(files.len());
    for (offset, path) in files.iter().enumerate() {
        let index = offset + 1;
        // ONE resolution, through the shared chokepoint. The extension-versus-
        // bytes disagreement now arrives as a typed `FormatMismatch` rather
        // than being recomputed here against a second extension table.
        let admitted = admit_local_image(path, &policy).map_err(|e| match e {
            IntakeError::FormatMismatch { declared, detected } => AttachmentError::MimeMismatch {
                index,
                path: path.clone(),
                declared,
                detected: detected.to_string(),
            },
            other => AttachmentError::Load {
                index,
                path: path.clone(),
                reason: render_image_intake_error(other),
            },
        })?;
        let mime = admitted.kind.as_str();
        let bytes = admitted.bytes;
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .filter(|total| *total <= COMPOSER_MAX_TOTAL_BYTES)
            .ok_or(AttachmentError::AggregateTooLarge {
                limit: COMPOSER_MAX_TOTAL_BYTES,
            })?;
        if !matches!(mime, "image/png" | "image/jpeg") {
            return Err(AttachmentError::UnsupportedMime {
                index,
                path: path.clone(),
                mime: mime.to_string(),
            });
        }
        images.push(ContentBlock::Image {
            mime: mime.to_string(),
            data: STANDARD.encode(bytes),
        });
    }
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn png_bytes(tail: u8) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\ncomposer".to_vec();
        bytes.push(tail);
        bytes
    }

    fn jpeg_bytes(tail: u8) -> Vec<u8> {
        let mut bytes = b"\xff\xd8\xff\xe0composer-jpeg".to_vec();
        bytes.push(tail);
        bytes
    }

    #[test]
    fn preserves_file_order_and_exact_bytes() {
        let dir = tempdir().unwrap();
        let first = dir.path().join("first.png");
        let second = dir.path().join("second.jpg");
        fs::write(&first, png_bytes(1)).unwrap();
        fs::write(&second, jpeg_bytes(2)).unwrap();

        let blocks = load_composer_images(&[
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ])
        .unwrap();

        assert!(matches!(
            &blocks[0],
            ContentBlock::Image { mime, data }
                if mime == "image/png" && STANDARD.decode(data).unwrap() == png_bytes(1)
        ));
        assert!(matches!(
            &blocks[1],
            ContentBlock::Image { mime, data }
                if mime == "image/jpeg" && STANDARD.decode(data).unwrap() == jpeg_bytes(2)
        ));
    }

    #[test]
    fn rejects_extension_mime_mismatch() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("forged.jpg");
        fs::write(&path, png_bytes(3)).unwrap();
        let error = load_composer_images(&[path.to_string_lossy().into_owned()]).unwrap_err();
        assert!(matches!(error, AttachmentError::MimeMismatch { .. }));
    }

    #[test]
    fn rejects_too_many_files_before_opening_any_path() {
        let files = (0..=COMPOSER_MAX_FILES)
            .map(|index| format!("missing-{index}.png"))
            .collect::<Vec<_>>();
        let error = load_composer_images(&files).unwrap_err();
        assert_eq!(
            error,
            AttachmentError::TooManyFiles {
                count: COMPOSER_MAX_FILES + 1,
                limit: COMPOSER_MAX_FILES,
            }
        );
    }

    #[test]
    fn rejects_aggregate_bytes_before_base64_expansion() {
        let dir = tempdir().unwrap();
        let first = dir.path().join("first.png");
        let second = dir.path().join("second.png");
        let mut bytes = vec![0_u8; COMPOSER_MAX_TOTAL_BYTES / 2 + 1];
        bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        fs::write(&first, &bytes).unwrap();
        fs::write(&second, &bytes).unwrap();

        let error = load_composer_images(&[
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ])
        .unwrap_err();
        assert_eq!(
            error,
            AttachmentError::AggregateTooLarge {
                limit: COMPOSER_MAX_TOTAL_BYTES,
            }
        );
    }
}
