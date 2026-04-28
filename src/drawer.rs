use objc2::{
    AnyThread, MainThreadMarker, MainThreadOnly,
    rc::{DefaultRetained, Retained},
};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSScreen, NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::{CFRetained, CGSize};
use objc2_core_graphics::{CGColor, CGMutablePath};
use objc2_foundation::{NSMutableAttributedString, NSPoint, NSRange, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CAShapeLayer, CATextLayer, CATransaction, kCAAlignmentCenter};

use crate::{
    ax_element::{Frame, HintBox},
    config::GlyphlowTheme,
    util::estimate_frame_for_text,
};

enum Center {
    Top(f64, f64),
    Middle(f64, f64),
}

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

pub trait GlyphlowDrawingLayer {
    fn from_window(window: &Retained<NSWindow>) -> Option<Retained<CALayer>>;
    fn clear(&self);
    fn draw_hints<'a>(
        &self,
        hints: impl Iterator<Item = &'a HintBox>,
        theme: &GlyphlowTheme,
        key_prefix_len: usize,
        screen_size: CGSize,
    );
    fn draw_menu(
        &self,
        text: &str,
        screen_size: CGSize,
        theme: &GlyphlowTheme,
    ) -> Retained<CALayer>;
    fn draw_frame_box(&self, frames: &Frame, color: &CFRetained<CGColor>);
    fn draw_attributed_string(
        &self,
        attr_string: Retained<NSMutableAttributedString>,
        screen_size: CGSize,
        text_size: CGSize,
        theme: &GlyphlowTheme,
    );
}

impl GlyphlowDrawingLayer for CALayer {
    fn clear(&self) {
        unsafe {
            CATransaction::begin();
            self.setSublayers(None);
            CATransaction::commit();
            // Force cleared after calling
            CATransaction::flush();
        }
    }

    fn from_window(window: &Retained<NSWindow>) -> Option<Retained<CALayer>> {
        let content_view = window.contentView()?;
        content_view.setWantsLayer(true);
        let root_layer = content_view.layer()?;
        // Clear existing sublayers
        root_layer.clear();
        Some(root_layer)
    }

    fn draw_hints<'a>(
        &self,
        hints: impl Iterator<Item = &'a HintBox>,
        theme: &GlyphlowTheme,
        key_prefix_len: usize,
        screen_size: CGSize,
    ) {
        // Geometry determined by font size
        let font_size = &theme.hint_font.pointSize();
        let tri_height = font_size / 2.0;
        let tri_width = font_size / 2.0;

        // Colors parsed from hex strings
        let bg_color = &theme.hint_bg_color;
        let hl_color = &theme.hint_hl_color;
        let fg_color = &theme.hint_fg_color;
        let font = &theme.hint_font;

        for hint in hints {
            let bg_color = hint.color.as_ref().unwrap_or(bg_color);

            if let Some(frame) = &hint.frame {
                self.draw_frame_box(frame, bg_color);
            }
            // Create NSMutableAttributedString first to estimate the size
            // Highlight prefixed keys
            let label_string = NSString::from_str(&hint.label);
            let attr_string = NSMutableAttributedString::initWithString(
                NSMutableAttributedString::alloc(),
                &label_string,
            );
            unsafe {
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
                attr_string.addAttribute_value_range(
                    NSFontAttributeName,
                    font,
                    NSRange::new(0, hint.label.len()),
                );
            }

            // Background Box
            let (size, _) =
                estimate_frame_for_text(&attr_string, (screen_size.width, screen_size.height));
            let box_layer = text_box_with_attributed_string(
                attr_string,
                true,
                bg_color,
                theme.hint_margin_size as f64,
                Center::Top(hint.x, hint.y - tri_height),
                screen_size,
                size,
            );

            // Create the triangle pointing to the center
            let box_size = box_layer.bounds().size;
            let tri_y_offset = box_size.height;
            let tri_x_offset = (box_size.width - tri_width) / 2.0;
            let tri_layer = CAShapeLayer::new();
            let path = CGMutablePath::new();

            unsafe {
                CGMutablePath::move_to_point(Some(&path), std::ptr::null(), 0.0, 0.0); // A
                CGMutablePath::add_line_to_point(
                    Some(&path),
                    std::ptr::null(),
                    tri_width / 2.0,
                    tri_height,
                ); // B
                CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), tri_width, 0.0); // C
            }
            CGMutablePath::close_subpath(Some(&path));

            tri_layer.setPath(Some(&path));
            tri_layer.setFillColor(Some(bg_color));
            tri_layer.setFrame(NSRect::new(
                NSPoint::new(tri_x_offset, tri_y_offset),
                NSSize::new(tri_width, tri_height),
            ));

            box_layer.addSublayer(&tri_layer);
            self.addSublayer(&box_layer);
        }
    }

    fn draw_attributed_string(
        &self,
        attr_string: Retained<NSMutableAttributedString>,
        screen_size: CGSize,
        text_size: CGSize,
        theme: &GlyphlowTheme,
    ) {
        let text_box = text_box_with_attributed_string(
            attr_string,
            false,
            &theme.menu_bg_color,
            theme.menu_margin_size as f64,
            Center::Middle(screen_size.width / 2.0, screen_size.height / 2.0),
            screen_size,
            text_size,
        );
        text_box.setBorderWidth(2.0);
        text_box.setBorderColor(Some(&theme.menu_fg_color));
        self.addSublayer(&text_box);
    }

    fn draw_menu(
        &self,
        text: &str,
        screen_size: CGSize,
        theme: &GlyphlowTheme,
    ) -> Retained<CALayer> {
        let text_box = draw_text_box(
            text,
            false,
            &theme.menu_font,
            &theme.menu_fg_color,
            &theme.menu_bg_color,
            theme.menu_margin_size as f64,
            Center::Middle(screen_size.width / 2.0, screen_size.height / 2.0),
            screen_size,
        );
        text_box.setBorderWidth(2.0);
        text_box.setBorderColor(Some(&theme.menu_fg_color));
        self.addSublayer(&text_box);
        text_box
    }

    fn draw_frame_box(&self, frame: &Frame, color: &CFRetained<CGColor>) {
        let container = CALayer::new();
        let origin = frame.top_left;
        let origin = NSPoint::new(origin.x, origin.y);
        let (w, h) = frame.size();
        container.setFrame(NSRect::new(origin, NSSize::new(w, h)));
        container.setBorderWidth(2.0);
        container.setBorderColor(Some(color));
        container.setZPosition(-1.0);

        self.addSublayer(&container);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_text_box(
    text: &str,
    center_text: bool,
    font: &Retained<NSFont>,
    fg_color: &CFRetained<CGColor>,
    bg_color: &CFRetained<CGColor>,
    margin: f64,
    center: Center,
    screen_size: CGSize,
) -> Retained<CALayer> {
    unsafe {
        // Estimate text size with attributed string
        let ns_string = NSString::from_str(text);
        let attr_string = NSMutableAttributedString::initWithString(
            NSMutableAttributedString::alloc(),
            &ns_string,
        );
        let full_range = NSRange::new(0, attr_string.length());

        attr_string.addAttribute_value_range(
            NSForegroundColorAttributeName,
            fg_color.as_ref(),
            full_range,
        );

        attr_string.addAttribute_value_range(NSFontAttributeName, font, full_range);

        // HACK: For multilingual text, height is underestimated due to fallback fonts.
        // This ensures more vertical spacing.
        let style = NSMutableParagraphStyle::default_retained();
        style.setLineSpacing(1.0);
        // style.setLineHeightMultiple(1.2);
        attr_string.addAttribute_value_range(NSParagraphStyleAttributeName, &style, full_range);

        let (mut size, visible_len) =
            estimate_frame_for_text(&attr_string, (screen_size.width, screen_size.height * 2.0));

        // NOTE: if estimated size is too large, reduce font size and retry
        if size.height + 2.0 * margin > screen_size.height {
            let font_size = font.pointSize() * visible_len as f64 / ns_string.len() as f64;
            // Don't make it too small
            let font_size = font_size.max(10.0);
            attr_string.addAttribute_value_range(
                NSFontAttributeName,
                &NSFont::fontWithName_size(&font.fontName(), font_size).unwrap(),
                full_range,
            );
            size = estimate_frame_for_text(&attr_string, (screen_size.width, screen_size.height)).0;
        }

        text_box_with_attributed_string(
            attr_string,
            center_text,
            bg_color,
            margin,
            center,
            screen_size,
            size,
        )
    }
}

fn text_box_with_attributed_string(
    attr_string: Retained<NSMutableAttributedString>,
    center_text: bool,
    bg_color: &CFRetained<CGColor>,
    margin: f64,
    center: Center,
    screen_size: CGSize,
    frame_size: CGSize,
) -> Retained<CALayer> {
    let CGSize { width, height } = frame_size;

    let box_width = width + (margin * 2.0);
    let box_height = height + (margin * 2.0);

    let (o_x, o_y) = match center {
        Center::Top(x, y) => (x - box_width / 2.0, y - box_height),
        Center::Middle(x, y) => (x - box_width / 2.0, y - box_height / 2.0),
    };
    let origin = NSPoint::new(
        o_x.min(screen_size.width - box_width).max(0.0),
        o_y.max(0.0).min(screen_size.height - box_height),
    );

    let container = CALayer::new();
    container.setFrame(NSRect::new(origin, NSSize::new(box_width, box_height)));
    container.setBackgroundColor(Some(bg_color));

    let text_layer = CATextLayer::new();
    text_layer.setFrame(NSRect::new(
        NSPoint::new(margin, margin), // Positioned exactly at margin
        frame_size,
    ));
    text_layer.setWrapped(true);

    unsafe {
        text_layer.setString(Some(&attr_string));
        if center_text {
            text_layer.setAlignmentMode(kCAAlignmentCenter);
        }
    }

    text_layer.setContentsScale(2.0); // Retina crispness
    container.addSublayer(&text_layer);
    container.setCornerRadius(margin);
    container
}
