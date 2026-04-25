use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::{
    AnyThread, MainThreadMarker,
    rc::{Retained, autoreleasepool},
    runtime::ProtocolObject,
};
use objc2_app_kit::{NSImage, NSPasteboard};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSError, NSSize, NSURL};
use objc2_screen_capture_kit::{
    SCContentFilter, SCScreenshotConfiguration, SCScreenshotManager, SCScreenshotOutput,
    SCShareableContent,
};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedTextObservation, VNRequest,
};
use tokio::sync::oneshot;

use crate::ax_element::Frame;

async fn capture_focused_window_async(
    frame: CGRect,
    pid: i32,
) -> Result<Retained<CGImage>, String> {
    let (tx, rx) = oneshot::channel();
    let tx_shared = Arc::new(Mutex::new(Some(tx)));

    let _mtm = MainThreadMarker::new().ok_or("Must be on main thread")?;

    let tx_outer = Arc::clone(&tx_shared);
    let tx_inner = Arc::clone(&tx_outer);

    let inner_result_callback = RcBlock::new(
        move |output: *mut SCScreenshotOutput, error: *mut NSError| {
            let Some(s) = tx_inner.lock().unwrap().take() else {
                return;
            };
            if !error.is_null() || output.is_null() {
                let description = unsafe { (*error).localizedDescription() };
                let _ = s.send(Err(format!("Capture failed: {description}")));
            } else {
                let cg_image = unsafe { (*output).sdrImage() };
                if let Some(retained) = cg_image {
                    let _ = s.send(Ok(retained));
                } else {
                    let _ = s.send(Err("No image in screenshot output.".to_string()));
                }
            }
        },
    );

    let outer_callback = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            if !error.is_null() {
                if let Some(s) = tx_outer.lock().unwrap().take() {
                    let _ = s.send(Err("Failed to get content".to_string()));
                };
                return;
            }

            unsafe {
                let windows = (*content).windows();
                let found_window = windows.iter().find(|w| {
                    let app = w.owningApplication();
                    let apid = app.map(|a| a.processID()).unwrap_or(0);
                    apid == pid && w.isOnScreen() && w.windowLayer() == 0
                });

                let Some(window) = found_window else {
                    if let Some(s) = tx_outer.lock().unwrap().take() {
                        let _ = s.send(Err("No window found".to_string()));
                    };
                    return;
                };

                let filter = SCContentFilter::initWithDesktopIndependentWindow(
                    SCContentFilter::alloc(),
                    &window,
                );
                let config = SCScreenshotConfiguration::init(SCScreenshotConfiguration::alloc());

                let win_frame = window.frame();
                let mut mframe = frame;
                mframe.origin.x -= win_frame.origin.x;
                mframe.origin.y -= win_frame.origin.y;
                config.setSourceRect(mframe);

                SCScreenshotManager::captureScreenshotWithFilter_configuration_completionHandler(
                    &filter,
                    &config,
                    Some(&inner_result_callback),
                );
            }
        },
    );

    unsafe {
        SCShareableContent::getShareableContentWithCompletionHandler(&outer_callback);
    }

    rx.await.map_err(|_| "Channel closed.".to_string())?
}

pub async fn screen_shot(frame: &Frame, pid: i32) {
    let CGPoint { x, y } = frame.top_left;
    let (w, h) = frame.size();
    let rect = CGRect::new(CGPoint::new(x, y), CGSize::new(w, h));
    let ns_size = NSSize::new(w, h);

    let cg_image = match capture_focused_window_async(rect, pid).await {
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

pub fn perform_ocr(path: &str) -> Result<String, Box<dyn std::error::Error>> {
    unsafe {
        let url = NSURL::fileURLWithPath(&objc2_foundation::NSString::from_str(path));

        let request = VNRecognizeTextRequest::init(VNRecognizeTextRequest::alloc());
        request.setRecognitionLevel(objc2_vision::VNRequestTextRecognitionLevel::Accurate);

        let handler = VNImageRequestHandler::initWithURL_options(
            VNImageRequestHandler::alloc(),
            &url,
            &objc2_foundation::NSDictionary::new(),
        );

        let request: Retained<VNRequest> = Retained::cast_unchecked(request);
        let requests = NSArray::from_retained_slice(std::slice::from_ref(&request));
        handler.performRequests_error(&requests)?;

        let mut full_text = String::new();
        if let Some(results) = request.results() {
            for observation in results {
                let text_obs: Retained<VNRecognizedTextObservation> =
                    Retained::cast_unchecked(observation);

                if let Some(top_candidate) = text_obs.topCandidates(1).iter().next() {
                    full_text.push_str(&top_candidate.string().to_string());
                    full_text.push('\n');
                }
            }
        }

        Ok(full_text)
    }
}
