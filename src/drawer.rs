use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{NSBackingStoreType, NSColor, NSScreen, NSWindow, NSWindowStyleMask};
use objc2_core_foundation::CGSize;
use objc2_core_graphics::CGMutablePath;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CAShapeLayer, CATextLayer};

use crate::HintBox;

pub fn get_main_screen_size(mtm: MainThreadMarker) -> CGSize {
    let screens = NSScreen::screens(mtm);
    // The first screen in the array is always the "primary" screen
    // with the menu bar, which defines the coordinate system origin.
    screens.objectAtIndex(0).frame().size
}

pub fn create_overlay_window(mtm: MainThreadMarker, screen_size: CGSize) -> Retained<NSWindow> {
    unsafe {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), screen_size);

        // Use NSBackingStoreType::Buffered (the modern enum path)
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        );

        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setIgnoresMouseEvents(true);
        window.setHasShadow(false);

        // Use a raw integer for the level to avoid the "non-constant" error.
        // 2147483647 is the maximum level (Status/ScreenSaver level)
        window.setLevel(2147483647);

        window
    }
}

pub fn draw_hints(window: &NSWindow, hints: &Vec<HintBox>) {
    unsafe {
        let content_view = window.contentView().expect("Content view missing");
        content_view.setWantsLayer(true);
        let root_layer = content_view.layer().expect("Layer missing");

        // Clear existing sublayers
        root_layer.setSublayers(None);

        // Core dimensions
        let box_width = 34.0;
        let box_height = 24.0;
        let tri_height = 6.0; // Height of the triangle point
        let tri_width = 8.0; // Width of the triangle base
        let font_size = 14.0;
        let corner_radius = 5.0;

        let bg_color = NSColor::yellowColor()
            .colorWithAlphaComponent(0.3)
            .CGColor();

        for hint in hints {
            // 1. Calculate Positions based on your requirement:
            // The input (hint.x, hint.y) is the top point of the triangle.

            // The triangle sits on top of the main box.
            let tri_y_offset = box_height;
            // Center the triangle horizontally on the box
            let tri_x_offset = (box_width - tri_width) / 2.0;

            // Shift the main box so the triangle tip hits the exact (x, y)
            let box_shifted_x = hint.x - (box_width / 2.0);
            let box_shifted_y = hint.y - (box_height + tri_height);

            // 2. Create the main box container
            let box_layer = CALayer::new();
            box_layer.setFrame(NSRect::new(
                NSPoint::new(box_shifted_x, box_shifted_y),
                NSSize::new(box_width, box_height),
            ));
            box_layer.setBackgroundColor(Some(&bg_color));
            box_layer.setCornerRadius(corner_radius);

            // 3. Create the triangle using CAShapeLayer
            let tri_layer = CAShapeLayer::new();

            // Define the triangle path (relative to its frame)
            let path = CGMutablePath::new();
            CGMutablePath::move_to_point(Some(&path), std::ptr::null(), 0.0, 0.0); // A
            CGMutablePath::add_line_to_point(
                Some(&path),
                std::ptr::null(),
                tri_width / 2.0,
                tri_height,
            ); // B
            CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), tri_width, 0.0); // C
            CGMutablePath::close_subpath(Some(&path));

            tri_layer.setPath(Some(&path));
            tri_layer.setFillColor(Some(&bg_color)); // Match box color

            // Position the triangle on top of the main box
            tri_layer.setFrame(NSRect::new(
                NSPoint::new(tri_x_offset, tri_y_offset),
                NSSize::new(tri_width, tri_height),
            ));

            box_layer.addSublayer(&tri_layer);

            // 4. Create the centered text layer
            let text_layer = CATextLayer::new();
            text_layer.setAlignmentMode(&NSString::from_str("center"));

            // Calculate vertical center offset
            let y_offset = (box_height - font_size) / 2.0 - 1.0;

            text_layer.setFrame(NSRect::new(
                NSPoint::new(0.0, y_offset),
                NSSize::new(box_width, font_size + 4.0),
            ));

            text_layer.setString(Some(&NSString::from_str(&hint.label)));
            text_layer.setFontSize(font_size);
            text_layer.setForegroundColor(Some(&NSColor::blackColor().CGColor()));
            text_layer.setContentsScale(2.0); // Retina crispness

            box_layer.addSublayer(&text_layer);
            root_layer.addSublayer(&box_layer);
        }
    }
}
