use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::{
    AnyThread,
    rc::{Retained, autoreleasepool},
    runtime::ProtocolObject,
};
use objc2_app_kit::{NSImage, NSPasteboard};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSError, NSString};
use objc2_screen_capture_kit::SCScreenshotManager;
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedTextObservation, VNRequest,
};
use tokio::sync::oneshot;

use crate::ax_element::Frame;

async fn capture_focused_window(frame_rect: CGRect) -> Result<Retained<CGImage>, String> {
    let (tx, rx) = oneshot::channel();
    let tx_shared = Arc::new(Mutex::new(Some(tx)));

    let inner_result_callback = RcBlock::new(move |output: *mut CGImage, error: *mut NSError| {
        let Some(s) = tx_shared.lock().ok().and_then(|mut x| x.take()) else {
            return;
        };
        if !error.is_null() || output.is_null() {
            let err = unsafe { Retained::retain(error) };
            let description = err
                .map(|re| re.localizedDescription().to_string())
                .unwrap_or_default();
            let _ = s.send(Err(format!("Capture failed: {description}")));
        } else {
            let cg_image = unsafe { Retained::retain(output) };
            if let Some(retained) = cg_image {
                let _ = s.send(Ok(retained));
            } else {
                let _ = s.send(Err("No image in screenshot output.".to_string()));
            }
        }
    });

    unsafe {
        SCScreenshotManager::captureImageInRect_completionHandler(
            frame_rect,
            Some(&inner_result_callback),
        )
    };

    rx.await.map_err(|_| "Channel closed.".to_string())?
}

// TODO: save more memory
pub async fn screen_shot(frame: &Frame) {
    let rect = frame.to_cgrect();
    let ns_size = frame.ns_size();
    let cg_image = match capture_focused_window(rect).await {
        Ok(img) => img,
        Err(e) => {
            println!("{e}");
            return;
        }
    };

    autoreleasepool(|_| {
        let ns_image = NSImage::initWithCGImage_size(NSImage::alloc(), &cg_image, ns_size);
        let pb = NSPasteboard::generalPasteboard();
        // Clear the clipboard before writing
        pb.clearContents();

        let proto_image = ProtocolObject::from_retained(ns_image);
        let objects = NSArray::from_retained_slice(&[proto_image]);
        pb.writeObjects(&objects);
    })
}

pub type OCRResult = Vec<(String, CGRect)>;

pub async fn perform_ocr(
    frame: &Frame,
    languages: &[String],
) -> Result<OCRResult, Box<dyn std::error::Error>> {
    let rect = frame.to_cgrect();
    let (w, h) = frame.size();
    unsafe {
        let cg_image = capture_focused_window(rect).await?;

        autoreleasepool(|_| {
            let request = VNRecognizeTextRequest::init(VNRecognizeTextRequest::alloc());
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
            Ok(ocr_results)
        })
    }
}
