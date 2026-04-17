use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSForegroundColorAttributeName, NSScreen, NSWindow,
    NSWindowStyleMask,
};
use objc2_core_foundation::{CFString, CGPoint, CGSize};
use objc2_core_graphics::{CGColor, CGMutablePath};
use objc2_core_text::CTFont;
use objc2_foundation::{NSMutableAttributedString, NSPoint, NSRange, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CAShapeLayer, CATextLayer, kCAAlignmentCenter};

use crate::{ax_element::HintBox, config::GlyphlowTheme};
use unicode_width::UnicodeWidthStr;

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

        // Front most
        window.setLevel(i32::MAX as isize);

        window
    }
}

pub fn clear_window(window: &NSWindow) -> Option<Retained<CALayer>> {
    let content_view = window.contentView()?;
    content_view.setWantsLayer(true);
    let root_layer = content_view.layer()?;
    // Clear existing sublayers
    unsafe {
        root_layer.setSublayers(None);
    }
    Some(root_layer)
}

pub fn draw_hints(
    window: &NSWindow,
    hints: &Vec<HintBox>,
    theme: &GlyphlowTheme,
    max_width: u32,
    key_prefix_len: usize,
) {
    let root_layer = clear_window(window).expect("Failed to get root layer of the window.");

    // Geometry determined by font size
    let font_size = theme.font_size as f64;
    let box_width = font_size * max_width as f64 / 1.5 + 6.0;
    let box_height = font_size + 6.0;
    let tri_height = font_size / 2.0;
    let tri_width = font_size / 2.0;
    let corner_radius = theme.hint_radius as f64;

    // Colors parsed from hex strings
    let bg_color = cgcolor_from_hex(&theme.hint_bg_color);
    let hl_color =
        cgcolor_from_hex(&theme.hint_hl_color).unwrap_or(NSColor::whiteColor().CGColor());
    let fg_color =
        cgcolor_from_hex(&theme.hint_fg_color).unwrap_or(NSColor::blackColor().CGColor());

    unsafe {
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
            text_layer.setAlignmentMode(kCAAlignmentCenter);

            let y_offset = (font_size * 1.25 - box_height) / 2.0;
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

            // Highlight prefixed keys
            let label_string = NSString::from_str(&hint.label);
            let attr_string = NSMutableAttributedString::initWithString(
                NSMutableAttributedString::alloc(),
                &label_string,
            );
            attr_string.addAttribute_value_range(
                NSForegroundColorAttributeName,
                hl_color.as_ref(),
                NSRange::new(0, key_prefix_len),
            );
            attr_string.addAttribute_value_range(
                NSForegroundColorAttributeName,
                fg_color.as_ref(),
                NSRange::new(key_prefix_len, hint.label.len() - key_prefix_len),
            );
            text_layer.setString(Some(&attr_string));
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

pub fn draw_dictionary_popup(
    window: &NSWindow,
    text: &str,
    center: &(f64, f64),
    screen_size: CGSize,
) {
    let root_layer = clear_window(window).expect("Failed to get root layer of the window.");
    let text_box = draw_text_box(
        text,
        false,
        14.0,
        NSColor::blackColor().CGColor(),
        NSColor::whiteColor().CGColor(),
        10.0,
        CGPoint::new(center.0, screen_size.height - center.1),
        screen_size,
    );
    root_layer.addSublayer(&text_box);
}

#[allow(clippy::too_many_arguments)]
fn draw_text_box(
    text: &str,
    center_text: bool,
    font_size: f64,
    fg_color: Retained<CGColor>,
    bg_color: Retained<CGColor>,
    margin: f64,
    center: CGPoint,
    screen_size: CGSize,
) -> Retained<CALayer> {
    // Estimate Text Area Size
    let lines: Vec<&str> = text.split('\n').collect();
    let line_count = lines.len() as f64;
    let max_chars = lines
        .iter()
        .map(|l| UnicodeWidthStr::width(*l))
        .max()
        .unwrap_or(0) as f64;

    let estimated_text_width = max_chars * (font_size * 0.6);
    let estimated_text_height = line_count * (font_size * 1.4);

    // 3. Calculate Box Size (Text + Margins)
    let box_width = estimated_text_width + (margin * 2.0);
    let box_height = estimated_text_height + (margin * 2.0);

    let origin = NSPoint::new(
        (center.x - (margin / 2.0))
            .min(screen_size.width - box_width)
            .max(0.0),
        (center.y - box_height)
            .max(0.0)
            .min(screen_size.height - box_height),
    );

    let container = CALayer::new();
    container.setFrame(NSRect::new(origin, NSSize::new(box_width, box_height)));
    container.setBackgroundColor(Some(&bg_color));

    let text_layer = CATextLayer::new();
    text_layer.setFrame(NSRect::new(
        NSPoint::new(margin, margin), // Positioned exactly at margin
        NSSize::new(estimated_text_width, estimated_text_height),
    ));

    let ns_string = NSString::from_str(text);
    unsafe {
        text_layer.setString(Some(&ns_string));
        text_layer.setFont(Some(&CTFont::with_name(
            &CFString::from_str("Andale Mono"),
            font_size,
            std::ptr::null(),
        )));
        if center_text {
            text_layer.setAlignmentMode(kCAAlignmentCenter); // Horizontal center
        }
    }
    text_layer.setFontSize(font_size);
    text_layer.setForegroundColor(Some(&fg_color));
    text_layer.setWrapped(false);
    text_layer.setContentsScale(2.0); // Retina crispness

    container.addSublayer(&text_layer);
    container
}
