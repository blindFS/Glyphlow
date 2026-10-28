use std::{
    sync::mpsc::{Receiver, RecvTimeoutError, channel},
    thread,
    time::{Duration, Instant},
};

use block2::RcBlock;
use core_foundation::runloop::{CFRunLoopRunInMode, kCFRunLoopDefaultMode};
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

/// The cap on waiting for ScreenCaptureKit's completion handler, which is
/// documented to fire exactly once but is not enforced.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long OCR may run before it is abandoned.
const OCR_TIMEOUT: Duration = Duration::from_millis(1500);

/// Poll interval for a wait that pumps the run loop.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Wait for `rx`, pumping the run loop so main-thread work still runs.
///
/// `Timeout` means `timeout` elapsed, `Disconnected` that the sender went away
/// without sending.
fn recv_pumping<T>(rx: &Receiver<T>, timeout: Duration) -> Result<T, RecvTimeoutError> {
    let deadline = Instant::now() + timeout;
    loop {
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(value) => return Ok(value),
            Err(RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    return Err(RecvTimeoutError::Timeout);
                }
                unsafe {
                    CFRunLoopRunInMode(
                        kCFRunLoopDefaultMode,
                        POLL_INTERVAL.as_secs_f64(),
                        false.into(),
                    );
                }
            }
            Err(RecvTimeoutError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
        }
    }
}

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

    match recv_pumping(&rx, CAPTURE_TIMEOUT) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err("Timed out waiting for the capture.".to_string()),
        Err(RecvTimeoutError::Disconnected) => Err("Channel closed.".to_string()),
    }
}

/// Capture `frame` and put it on the clipboard as JPEG. `false` on failure.
pub(crate) fn screen_shot(frame: &Frame) -> bool {
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
pub(crate) type OCRResult = Vec<(String, CGRect)>;

/// OCR `frame`, giving up after [`OCR_TIMEOUT`] with `Ok(None)`.
///
/// Recognition runs on its own thread: on a cold cache Vision compiles the
/// models first, which takes far longer than the caller may stay blocked.
pub(crate) fn perform_ocr(
    frame: &Frame,
    languages: Vec<String>,
) -> Result<Option<OCRResult>, Box<dyn std::error::Error>> {
    let cg_image = capture_rect(frame.to_cgrect())?;
    let frame = *frame;

    let (tx, rx) = channel();
    thread::spawn(move || {
        let _ = tx.send(recognize(cg_image, frame, &languages));
    });

    match recv_pumping(&rx, OCR_TIMEOUT) {
        Ok(result) => result.map(Some).map_err(Into::into),
        Err(RecvTimeoutError::Timeout) => Ok(None),
        Err(RecvTimeoutError::Disconnected) => Err("OCR thread died.".into()),
    }
}

/// Recognise the text in `cg_image` and map it back to screen coordinates.
///
/// Vision reports normalised bottom-left boxes, so the y axis is flipped here.
fn recognize(
    cg_image: Retained<CGImage>,
    frame: Frame,
    languages: &[String],
) -> Result<OCRResult, String> {
    let rect = frame.to_cgrect();
    let (w, h) = frame.size();
    unsafe {
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
            handler
                .performRequests_error(&requests)
                .map_err(|e| e.localizedDescription().to_string())?;

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
