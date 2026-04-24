use core_foundation::base::{CFRelease, CFTypeRef};
use objc2::{AnyThread, runtime::ProtocolObject};
use objc2_app_kit::{NSImage, NSPasteboard};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSSize};

use crate::ax_element::Frame;

// WARN: this is deprecated
// TODO: move to screen capture kit
unsafe extern "C" {
    fn CGWindowListCreateImage(
        screen_bounds: CGRect,
        list_option: u32,
        window_id: u32,
        image_option: u32,
    ) -> *const CGImage;
}

pub fn screen_shot(frame: &Frame) {
    let CGPoint { x, y } = frame.top_left;
    let (w, h) = frame.size();
    let rect = CGRect::new(CGPoint::new(x, y), CGSize::new(w, h));

    let ns_size = NSSize::new(w, h);
    let ns_image = unsafe {
        // Constants: kCGWindowListOptionAll = 0, kCGNullWindowID = 0, kCGWindowImageDefault = 0
        let cg_image = CGWindowListCreateImage(rect, 0, 0, 0);
        if cg_image.is_null() {
            return;
        } else {
            let ns_image = NSImage::initWithCGImage_size(NSImage::alloc(), &*cg_image, ns_size);
            // NOTE: make sure memory is freed
            CFRelease(cg_image as CFTypeRef);
            ns_image
        }
    };

    let pb = NSPasteboard::generalPasteboard();
    // Clear the clipboard before writing
    pb.clearContents();

    let proto_image = ProtocolObject::from_retained(ns_image);
    let objects = NSArray::from_retained_slice(&[proto_image]);
    pb.writeObjects(&objects);
}
