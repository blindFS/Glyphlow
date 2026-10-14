use std::{
    sync::mpsc::{RecvTimeoutError, channel},
    time::Duration,
};

use block2::RcBlock;
use core_foundation::{
    base::Boolean,
    runloop::{CFRunLoopRunInMode, kCFRunLoopDefaultMode},
};
use objc2::{
    AnyThread,
    rc::{DefaultRetained, Retained, autoreleasepool},
};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardType};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSDictionary, NSError, NSString};
use objc2_screen_capture_kit::SCScreenshotManager;
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedTextObservation, VNRequest,
};

use crate::util::Frame;

/// Screenshot the screen region `rect`.
///
/// ScreenCaptureKit only offers a completion-handler API, so the handler is
/// bridged back through a channel. The wait runs the run loop, because the
/// handler may be delivered on the main one.
fn capture_rect(rect: CGRect) -> Result<Retained<CGImage>, String> {
    let (tx, rx) = channel();

    let inner_result_callback = RcBlock::new(move |output: *mut CGImage, error: *mut NSError| {
        let result = if !error.is_null() || output.is_null() {
            let err = unsafe { Retained::retain(error) };
            let description = err
                .map(|re| re.localizedDescription().to_string())
                .unwrap_or_default();
            Err(format!("Capture failed: {description}"))
        } else {
            match unsafe { Retained::retain(output) } {
                Some(cg_image) => Ok(cg_image),
                None => Err("No image in screenshot output.".to_string()),
            }
        };
        let _ = tx.send(result);
    });

    unsafe {
        SCScreenshotManager::captureImageInRect_completionHandler(
            rect,
            Some(&inner_result_callback),
        )
    };

    loop {
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(result) => return result,
            Err(RecvTimeoutError::Timeout) => unsafe {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, Boolean::from(false));
            },
            Err(RecvTimeoutError::Disconnected) => return Err("Channel closed.".to_string()),
        }
    }
}

/// Capture `frame` and put it on the clipboard as JPEG. `false` on failure.
pub fn screen_shot(frame: &Frame) -> bool {
    let rect = frame.to_cgrect();
    let cg_image = match capture_rect(rect) {
        Ok(img) => img,
        Err(e) => {
            log::error!("{e}");
            return false;
        }
    };

    autoreleasepool(move |_| {
        let bitmap_rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg_image);
        let jpeg_data = unsafe {
            bitmap_rep.representationUsingType_properties(
                NSBitmapImageFileType::JPEG,
                &NSDictionary::new(),
            )
        };

        let pb = NSPasteboard::generalPasteboard();
        // Clear the clipboard before writing
        pb.clearContents();
        pb.setData_forType(
            jpeg_data.as_deref(),
            &NSPasteboardType::from_str("public.jpeg"),
        );

        drop(cg_image);
        true
    })
}

/// Recognised text, with the screen rect it was found in.
pub type OCRResult = Vec<(String, CGRect)>;

/// OCR `frame` and map every observation back to screen coordinates.
///
/// Vision reports normalised bottom-left boxes, so the y axis is flipped here.
pub async fn perform_ocr(
    frame: &Frame,
    languages: &[String],
) -> Result<OCRResult, Box<dyn std::error::Error>> {
    let rect = frame.to_cgrect();
    let (w, h) = frame.size();
    unsafe {
        let cg_image = capture_rect(rect)?;
        autoreleasepool(move |_| {
            let request = VNRecognizeTextRequest::default_retained();
            request.setRecognitionLevel(objc2_vision::VNRequestTextRecognitionLevel::Accurate);

            let langs = languages
                .iter()
                .map(|l| NSString::from_str(l.as_str()))
                .collect::<Vec<_>>();
            let langs_array = NSArray::from_retained_slice(&langs);
            request.setRecognitionLanguages(&langs_array);

            let handler = VNImageRequestHandler::initWithCGImage_options(
                VNImageRequestHandler::alloc(),
                &cg_image,
                &objc2_foundation::NSDictionary::new(),
            );

            let request: Retained<VNRequest> = Retained::cast_unchecked(request);
            let requests = NSArray::from_retained_slice(std::slice::from_ref(&request));
            handler.performRequests_error(&requests)?;

            let mut ocr_results = Vec::new();
            if let Some(results) = request.results() {
                for observation in results {
                    let text_obs: Retained<VNRecognizedTextObservation> =
                        Retained::cast_unchecked(observation);

                    // Restore normalized bounding box to CGRect of pixels
                    let CGRect { origin, size } = text_obs.boundingBox();
                    let x = origin.x * w + rect.origin.x;
                    let width = size.width * w;
                    let y = (1.0 - origin.y - size.height) * h + rect.origin.y;
                    let height = size.height * h;

                    let rect = CGRect::new(CGPoint::new(x, y), CGSize::new(width, height));
                    if let Some(top_candidate) = text_obs.topCandidates(1).iter().next() {
                        ocr_results.push((top_candidate.string().to_string(), rect))
                    }
                }
            }

            drop(handler);
            drop(cg_image);
            Ok(ocr_results)
        })
    }
}
