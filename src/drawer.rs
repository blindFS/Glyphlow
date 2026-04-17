use objc2::{
    AnyThread, MainThreadMarker, MainThreadOnly,
    rc::{DefaultRetained, Retained},
};
use objc2_app_kit::{
    NSAttributedStringNSStringDrawingDeprecated, NSBackingStoreType, NSColor, NSFont,
    NSFontAttributeName, NSForegroundColorAttributeName, NSMutableParagraphStyle,
    NSParagraphStyleAttributeName, NSScreen, NSStringDrawingOptions, NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::CGSize;
use objc2_core_graphics::{CGColor, CGMutablePath};
use objc2_foundation::{NSMutableAttributedString, NSPoint, NSRange, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CAShapeLayer, CATextLayer, kCAAlignmentCenter};

use crate::{ax_element::HintBox, config::GlyphlowTheme};

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
    key_prefix_len: usize,
    screen_size: CGSize,
) {
    let root_layer = clear_window(window).expect("Failed to get root layer of the window.");

    // Geometry determined by font size
    let font_size = theme.font_size as f64;
    let margin_size = theme.margin_size as f64;
    let tri_height = font_size / 2.0;
    let tri_width = font_size / 2.0;
    let corner_radius = theme.hint_radius as f64;

    // Colors parsed from hex strings
    let bg_color = cgcolor_from_hex(&theme.hint_bg_color);
    let hl_color =
        cgcolor_from_hex(&theme.hint_hl_color).unwrap_or(NSColor::whiteColor().CGColor());
    let fg_color =
        cgcolor_from_hex(&theme.hint_fg_color).unwrap_or(NSColor::blackColor().CGColor());
    let font = NSFont::fontWithName_size(&NSString::from_str(&theme.font), font_size);

    unsafe {
        for hint in hints {
            // Create NSMutableAttributedString first to estimate the size
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
            if let Some(font) = &font {
                attr_string.addAttribute_value_range(
                    NSFontAttributeName,
                    font,
                    NSRange::new(0, hint.label.len()),
                );
            }

            // Background Box
            let box_layer = text_box_with_attributed_string(
                attr_string,
                true,
                bg_color.as_deref(),
                margin_size,
                (hint.x, hint.y - tri_height),
                screen_size,
            );
            box_layer.setCornerRadius(corner_radius);

            // Create the triangle pointing to the center
            let box_size = box_layer.bounds().size;
            let tri_y_offset = box_size.height;
            let tri_x_offset = (box_size.width - tri_width) / 2.0;
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
    theme: &GlyphlowTheme,
) {
    let root_layer = clear_window(window).expect("Failed to get root layer of the window.");
    let font = NSFont::fontWithName_size(&NSString::from_str(&theme.font), theme.font_size as f64);
    let text_box = draw_text_box(
        text,
        false,
        font,
        NSColor::blackColor().CGColor(),
        NSColor::whiteColor().CGColor(),
        10.0,
        (center.0, screen_size.height - center.1),
        screen_size,
    );
    root_layer.addSublayer(&text_box);
}

#[allow(clippy::too_many_arguments)]
fn draw_text_box(
    text: &str,
    center_text: bool,
    font: Option<Retained<NSFont>>,
    fg_color: Retained<CGColor>,
    bg_color: Retained<CGColor>,
    margin: f64,
    center: (f64, f64),
    screen_size: CGSize,
) -> Retained<CALayer> {
    unsafe {
        // Estimate text size with attributed string
        let ns_string = NSString::from_str(text);
        let attr_string = NSMutableAttributedString::initWithString(
            NSMutableAttributedString::alloc(),
            &ns_string,
        );
        let full_range = NSRange::new(0, text.chars().count());

        attr_string.addAttribute_value_range(
            NSForegroundColorAttributeName,
            fg_color.as_ref(),
            full_range,
        );
        if let Some(font) = &font {
            attr_string.addAttribute_value_range(NSFontAttributeName, font, full_range);
            // HACK: Somehow it underestimates the height without line spacing, it's still not accurate
            let style = NSMutableParagraphStyle::default_retained();
            style.setLineSpacing(font.pointSize() / 3.0);
            attr_string.addAttribute_value_range(NSParagraphStyleAttributeName, &style, full_range);
        }

        text_box_with_attributed_string(
            attr_string,
            center_text,
            Some(&bg_color),
            margin,
            center,
            screen_size,
        )
    }
}

fn text_box_with_attributed_string(
    attr_string: Retained<NSMutableAttributedString>,
    center_text: bool,
    bg_color: Option<&CGColor>,
    margin: f64,
    center: (f64, f64),
    screen_size: CGSize,
) -> Retained<CALayer> {
    unsafe {
        let max_size = NSSize::new(10000.0, 10000.0);
        let options = NSStringDrawingOptions::UsesLineFragmentOrigin
            | NSStringDrawingOptions::UsesFontLeading;
        let text_bounds = attr_string.boundingRectWithSize_options(max_size, options);

        // Determined the box size and position
        let box_width = text_bounds.size.width + (margin * 2.0);
        let box_height = text_bounds.size.height + (margin * 2.0);
        let origin = NSPoint::new(
            (center.0 - (box_width / 2.0))
                .min(screen_size.width - box_width)
                .max(0.0),
            (center.1 - box_height)
                .max(0.0)
                .min(screen_size.height - box_height),
        );

        let container = CALayer::new();
        container.setFrame(NSRect::new(origin, NSSize::new(box_width, box_height)));
        container.setBackgroundColor(bg_color);

        let text_layer = CATextLayer::new();
        text_layer.setFrame(NSRect::new(
            NSPoint::new(margin, margin), // Positioned exactly at margin
            text_bounds.size,
        ));

        text_layer.setString(Some(&attr_string));
        text_layer.setContentsScale(2.0); // Retina crispness
        if center_text {
            text_layer.setAlignmentMode(kCAAlignmentCenter);
        }
        container.addSublayer(&text_layer);
        container
    }
}
