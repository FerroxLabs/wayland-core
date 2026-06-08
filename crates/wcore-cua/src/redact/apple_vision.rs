//! macOS OCR backend — Apple Vision (`VNRecognizeTextRequest`).
//!
//! Ships as the default macOS OCR backend with no feature flag because
//! Vision is part of the OS (10.15+). The implementation uses
//! `objc2-vision` for safe Objective-C bindings and feeds the captured
//! PNG bytes through `VNImageRequestHandler.initWithData:options:` so we
//! don't have to decode → re-encode through CoreGraphics.
//!
//! Coordinate system note: Vision reports bounding boxes in **normalized
//! image space** with the origin at the **bottom-left** (`y` increases
//! upward). The redact pipeline uses **top-left** pixel coordinates, so
//! we multiply by image dims and flip `y` here.

use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::CGRect;
use objc2_foundation::{NSArray, NSData, NSDictionary};
use objc2_vision::{
    VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizeTextRequestRevision3,
    VNRequest, VNRequestTextRecognitionLevel,
};

use super::{BoundingBox, OcrBackend, OcrError, TextRegion};

/// Apple Vision OCR backend. Cheap to construct (just a marker).
pub struct AppleVisionOcr;

impl AppleVisionOcr {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AppleVisionOcr {
    fn default() -> Self {
        Self::new()
    }
}

impl OcrBackend for AppleVisionOcr {
    fn extract_text_regions(&self, png_bytes: &[u8]) -> Result<Vec<TextRegion>, OcrError> {
        // Vision needs image dims in pixels to denormalize bounding
        // boxes back to pixel coords. We re-decode the PNG header
        // through the `image` crate rather than going through CGImage —
        // it's already a workspace dep and gives us width/height
        // without spinning up CoreGraphics.
        let (img_w, img_h) = image::load_from_memory(png_bytes)
            .map(|d| (d.width(), d.height()))
            .map_err(|e| OcrError::new(format!("apple_vision: decode header: {e}")))?;
        if img_w == 0 || img_h == 0 {
            return Ok(Vec::new());
        }

        // Wrap the PNG bytes in NSData (zero-copy thanks to
        // `with_bytes` — Vision retains the data for the lifetime of
        // the request handler).
        let ns_data = NSData::with_bytes(png_bytes);

        // Empty options dict — no orientation hint, no camera intrinsics.
        // `VNImageOption` is a typedef for `NSString` (see
        // objc2-vision::generated::VNRequestHandler), so the dict's
        // key type is fixed by the Vision API.
        let options: Retained<NSDictionary<VNImageOption, AnyObject>> = NSDictionary::new();

        // `initWithData_options` is a safe method per objc2-vision —
        // no unsafe block needed. `ns_data` and `options` live for the
        // duration of the call.
        let handler: Retained<VNImageRequestHandler> = VNImageRequestHandler::initWithData_options(
            VNImageRequestHandler::alloc(),
            &ns_data,
            &options,
        );

        let request: Retained<VNRecognizeTextRequest> = VNRecognizeTextRequest::new();
        // Use revision 3 (the modern, accuracy-tuned variant available
        // since macOS 13). Older OS versions silently clamp to whatever
        // they support — Vision does not error on too-new revisions.
        unsafe {
            request.setRevision(VNRecognizeTextRequestRevision3);
        }
        // Favor accuracy over speed for redaction — false negatives
        // leak secrets.
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(true);

        // Build the requests array (single-request batch is the normal
        // pattern for one-shot OCR). `VNRecognizeTextRequest` is a
        // subclass of `VNRequest`; `NSArray::from_retained_slice` over
        // a slice of the superclass works because objc2 surfaces the
        // upcast as a free coercion at the binding layer.
        let request_as_super: Retained<VNRequest> = {
            // SAFETY: VNRecognizeTextRequest's class hierarchy
            // (declared in objc2-vision via `extern_class!(super = ...
            // VNRequest)`) guarantees the cast is sound. We bump the
            // retain count so the array owns one strong reference and
            // the original `request` binding owns another (for the
            // post-perform `request.results()` call).
            let raw = Retained::as_ptr(&request) as *const VNRequest;
            unsafe { Retained::retain(raw as *mut VNRequest) }
                .expect("VNRecognizeTextRequest -> VNRequest upcast cannot be null")
        };
        let requests_array: Retained<NSArray<VNRequest>> =
            NSArray::from_retained_slice(&[request_as_super]);

        // Run the request synchronously. `performRequests_error` is
        // implemented as `Result<(), Retained<NSError>>` by objc2.
        handler
            .performRequests_error(&requests_array)
            .map_err(|e| OcrError::new(format!("apple_vision: performRequests: {e}")))?;

        let results = request.results();
        let Some(observations) = results else {
            return Ok(Vec::new());
        };

        let mut out = Vec::with_capacity(observations.len());
        for obs in observations.iter() {
            // Top candidate — Vision sorts by confidence descending.
            let candidates = obs.topCandidates(1);
            let Some(cand) = candidates.iter().next() else {
                continue;
            };
            let text = cand.string().to_string();
            let confidence = cand.confidence();
            // boundingBox lives on VNDetectedObjectObservation; the
            // recognized-text observation inherits it. Coords are
            // normalized [0,1] with origin at bottom-left.
            let bbox_norm = unsafe { obs.boundingBox() };
            let (x0, y0, x1, y1) = denormalize_bbox(bbox_norm, img_w, img_h);
            out.push(TextRegion {
                text,
                bbox: BoundingBox { x0, y0, x1, y1 },
                confidence,
            });
        }
        Ok(out)
    }
}

/// Convert a Vision normalized CGRect (origin bottom-left, range
/// `[0,1]`) into pixel-space top-left coords `(x0, y0, x1, y1)` matching
/// the redact pipeline's contract.
fn denormalize_bbox(bbox: CGRect, img_w: u32, img_h: u32) -> (u32, u32, u32, u32) {
    let w_f = img_w as f64;
    let h_f = img_h as f64;
    let x0_f = (bbox.origin.x * w_f).max(0.0).min(w_f - 1.0);
    let x1_f = ((bbox.origin.x + bbox.size.width) * w_f)
        .max(0.0)
        .min(w_f - 1.0);
    // Vision: y=0 is bottom. Convert to top-origin by `1.0 - (y +
    // height)`.
    let y_top_norm = 1.0 - (bbox.origin.y + bbox.size.height);
    let y_bot_norm = 1.0 - bbox.origin.y;
    let y0_f = (y_top_norm * h_f).max(0.0).min(h_f - 1.0);
    let y1_f = (y_bot_norm * h_f).max(0.0).min(h_f - 1.0);
    (
        x0_f.floor() as u32,
        y0_f.floor() as u32,
        x1_f.ceil() as u32,
        y1_f.ceil() as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_constructs() {
        let _b = AppleVisionOcr::new();
    }

    #[test]
    fn denormalize_bbox_round_trips_corners() {
        use objc2_core_foundation::{CGPoint, CGSize};
        // Vision's bottom-left origin maps to top-left output: a box at
        // `(0, 0)`-`(1, 1)` (full image, bottom-left origin) should map
        // to `(0, 0)-(w-1, h-1)` in top-left pixel coords.
        let r = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: 1.0,
                height: 1.0,
            },
        };
        let (x0, y0, x1, y1) = denormalize_bbox(r, 100, 50);
        assert_eq!((x0, y0, x1, y1), (0, 0, 99, 49));
    }

    #[test]
    fn denormalize_bbox_flips_y_axis() {
        use objc2_core_foundation::{CGPoint, CGSize};
        // A box covering the top quarter of the image in **Vision**
        // (origin bottom-left) means `y = 0.75 .. 1.0` in normalized
        // coords. In the redact pipeline that should be `y0=0 ..
        // y1=h/4` (top quarter, top-left origin).
        let r = CGRect {
            origin: CGPoint { x: 0.0, y: 0.75 },
            size: CGSize {
                width: 1.0,
                height: 0.25,
            },
        };
        let (_x0, y0, _x1, y1) = denormalize_bbox(r, 100, 100);
        assert_eq!(y0, 0);
        assert_eq!(y1, 25);
    }

    #[test]
    fn extract_on_blank_image_returns_empty() {
        // Apple Vision on a featureless white PNG should yield zero
        // recognized regions — verifies the backend wires correctly
        // end-to-end without depending on a text fixture (which the
        // build environment may not have).
        use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
        let img = RgbaImage::from_pixel(64, 32, Rgba([255, 255, 255, 255]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        let backend = AppleVisionOcr::new();
        let regions = backend.extract_text_regions(&bytes).expect("Vision call");
        assert!(
            regions.is_empty(),
            "expected no recognized regions on blank image, got {regions:?}"
        );
    }
}
