use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{NSBackingStoreType, NSColor, NSScreen, NSWindow, NSWindowStyleMask};
use objc2_core_foundation::{CFString, CGSize};
use objc2_core_graphics::{CGColor, CGMutablePath};
use objc2_core_text::CTFont;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CAShapeLayer, CATextLayer};

use crate::{HintBox, config::GlyphlowTheme};

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

pub fn draw_hints(window: &NSWindow, hints: &Vec<HintBox>, theme: &GlyphlowTheme, max_width: u32) {
    unsafe {
        let content_view = window.contentView().expect("Content view missing");
        content_view.setWantsLayer(true);
        let root_layer = content_view.layer().expect("Layer missing");

        // Clear existing sublayers
        root_layer.setSublayers(None);

        // Core dimensions
        let box_width = theme.font_size as f64 * max_width as f64 / 1.5 + 6.0;
        let box_height = theme.font_size as f64 + 6.0;
        let tri_height = theme.font_size as f64 / 2.0;
        let tri_width = theme.font_size as f64 / 2.0;
        let font_size = theme.font_size as f64;
        let corner_radius = theme.hint_radius as f64;

        let bg_color = cgcolor_from_hex(&theme.hint_bg_color);

        for hint in hints {
            // 1. Calculate Positions based on your requirement:
            // The input (hint.x, hint.y) is the top point of the triangle.
            // The triangle sits on top of the main box.
            let tri_y_offset = box_height;
            // Center the triangle horizontally on the box
            let tri_x_offset = (box_width - tri_width) / 2.0;

            // 2. Create the main box container
            // Shift the main box so the triangle tip hits the exact (x, y)
            let box_shifted_x = hint.x - (box_width / 2.0);
            let box_shifted_y = hint.y - (box_height + tri_height);

            let box_layer = CALayer::new();
            box_layer.setFrame(NSRect::new(
                NSPoint::new(box_shifted_x, box_shifted_y),
                NSSize::new(box_width, box_height),
            ));
            box_layer.setBackgroundColor(bg_color.as_deref());
            box_layer.setCornerRadius(corner_radius);

            // 3. Create the triangle using CAShapeLayer
            let tri_layer = CAShapeLayer::new();
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
            tri_layer.setFillColor(bg_color.as_deref());
            tri_layer.setFrame(NSRect::new(
                NSPoint::new(tri_x_offset, tri_y_offset),
                NSSize::new(tri_width, tri_height),
            ));
            box_layer.addSublayer(&tri_layer);

            // 4. Create the centered text layer
            let text_layer = CATextLayer::new();
            text_layer.setAlignmentMode(&NSString::from_str("center"));

            // Calculate vertical center offset
            let y_offset = (font_size - box_height) / 2.0 + 1.0;

            text_layer.setFrame(NSRect::new(
                NSPoint::new(0.0, y_offset),
                NSSize::new(box_width, box_height),
            ));

            text_layer.setFontSize(font_size);
            text_layer.setFont(Some(&CTFont::with_name(
                &CFString::from_str(&theme.font),
                font_size,
                std::ptr::null(),
            )));
            text_layer.setString(Some(&NSString::from_str(&hint.label)));
            text_layer.setForegroundColor(cgcolor_from_hex(&theme.hint_fg_color).as_deref());
            text_layer.setContentsScale(2.0); // Retina crispness

            box_layer.addSublayer(&text_layer);
            root_layer.addSublayer(&box_layer);
        }
    }
}

fn hex_to_rgba(hex: &str) -> Option<(f64, f64, f64, f64)> {
    let hex = hex.trim_start_matches('#');
    // TODO: error on invalid format
    let to_float = |i: std::ops::Range<usize>| -> Option<f64> {
        hex.get(i)
            .and_then(|s| u8::from_str_radix(s, 16).ok())
            .map(|iu8| iu8 as f64 / 255.0)
    };
    let r = to_float(0..2)?;
    let g = to_float(2..4)?;
    let b = to_float(4..6)?;
    let a = if hex.len() == 8 { to_float(6..8)? } else { 1.0 };
    Some((r, g, b, a))
}

fn cgcolor_from_hex(hex: &str) -> Option<Retained<CGColor>> {
    let (r, g, b, a) = hex_to_rgba(hex)?;
    Some(NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a).CGColor())
}
