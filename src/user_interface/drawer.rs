use crate::{
    config::GlyphlowTheme,
    util::{Frame, estimate_frame_for_text},
};
use objc2::{
    AnyThread, MainThreadMarker, MainThreadOnly,
    rc::{DefaultRetained, Retained, autoreleasepool},
};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSScreen, NSWindow,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSMutableAttributedString, NSPoint, NSRange, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CATextLayer, CATransaction};

struct Menu {
    container: Retained<CALayer>,
    text_layer: Retained<CATextLayer>,
    menu_string: Retained<NSMutableAttributedString>,
}

const BORDER_WIDTH: f64 = 2.0;
const MIN_FONT_SIZE: f64 = 10.0;

impl Menu {
    fn new(theme: &GlyphlowTheme) -> Self {
        autoreleasepool(|_| {
            let text_layer = CATextLayer::new();
            text_layer.setContentsScale(2.0);
            text_layer.setWrapped(true);

            let container = CALayer::new();
            container.setBorderWidth(BORDER_WIDTH);
            container.addSublayer(&text_layer);
            // Hidden by default
            container.setHidden(true);

            // Init mutable attributed string, need a dummy place holder to keep attributes
            let ns_string = NSString::from_str("n");
            let attr_string = NSMutableAttributedString::initWithString(
                NSMutableAttributedString::alloc(),
                &ns_string,
            );
            let menu = Self {
                container,
                text_layer,
                menu_string: attr_string,
            };

            menu.load_theme(theme);
            menu
        })
    }

    fn free(&self) {
        self.text_layer.removeFromSuperlayer();
        self.container.removeFromSuperlayer();
    }

    fn load_theme(&self, theme: &GlyphlowTheme) {
        self.container.setBorderColor(Some(&theme.menu_fg_color));
        self.container
            .setBackgroundColor(Some(&theme.menu_bg_color));
        self.container
            .setCornerRadius(theme.menu_margin_size as f64);
    }

    fn initialize_string_attributes(&self, theme: &GlyphlowTheme) {
        let full_range = NSRange::new(0, self.menu_string.length());

        unsafe {
            self.menu_string.addAttribute_value_range(
                NSForegroundColorAttributeName,
                theme.menu_fg_color.as_ref(),
                full_range,
            );

            let font = &theme.menu_font;
            self.menu_string
                .addAttribute_value_range(NSFontAttributeName, font, full_range);

            // HACK: For multilingual text, height is underestimated due to fallback fonts.
            // This ensures more vertical spacing.
            let style = NSMutableParagraphStyle::default_retained();
            style.setLineSpacing(1.0);
            // style.setLineHeightMultiple(1.2);
            self.menu_string.addAttribute_value_range(
                NSParagraphStyleAttributeName,
                &style,
                full_range,
            );
        }
    }

    fn estimate_text_size(
        &self,
        screen_frame: &Frame,
        theme: &GlyphlowTheme,
        auto_resize: bool,
    ) -> CGSize {
        let (width, height) = screen_frame.size();
        let (size, visible_len) = estimate_frame_for_text(&self.menu_string, (f64::MAX, f64::MAX));
        let shrinkage = (width / size.width)
            .min(height / size.height)
            .min(visible_len as f64 / self.menu_string.length() as f64);
        let font = &theme.menu_font;

        // NOTE: if estimated size is too large, reduce font size and re-estimate
        if auto_resize && shrinkage < 1.0 {
            // Don't make it too small
            let font_size = (font.pointSize() * shrinkage).max(MIN_FONT_SIZE);
            unsafe {
                self.menu_string.addAttribute_value_range(
                    NSFontAttributeName,
                    &NSFont::fontWithName_size(&font.fontName(), font_size).unwrap(),
                    NSRange::new(0, self.menu_string.length()),
                )
            };
            estimate_frame_for_text(&self.menu_string, (width, height)).0
        } else {
            size
        }
    }

    fn resize_and_show(
        &self,
        screen_frame: &Frame,
        overlay_frame: &Frame,
        theme: &GlyphlowTheme,
        auto_resize: bool,
    ) {
        let size = self.estimate_text_size(screen_frame, theme, auto_resize);
        let CGSize { width, height } = size;
        let margin = theme.menu_margin_size as f64;
        let box_width = width + (margin * 2.0);
        let box_height = height + (margin * 2.0);

        let (c_x, c_y) = screen_frame.center();
        let (o_x, o_y) = (c_x - box_width / 2.0, c_y + box_height / 2.0);

        let o_x_move = o_x
            .min(screen_frame.bottom_right.x - box_width)
            .max(screen_frame.top_left.x);
        let o_y_move = o_y.max(screen_frame.top_left.y + box_height);
        let origin = calibrated_origin(o_x_move, o_y_move, overlay_frame);

        self.container
            .setFrame(NSRect::new(origin, NSSize::new(box_width, box_height)));
        self.text_layer.setFrame(NSRect::new(
            NSPoint::new(margin, margin), // Positioned exactly at margin
            size,
        ));

        unsafe {
            self.text_layer.setString(Some(&self.menu_string));
        }
        self.container.setHidden(false);
    }

    fn draw(&self, text: &str, screen_frame: &Frame, overlay_frame: &Frame, theme: &GlyphlowTheme) {
        autoreleasepool(|_| {
            let ns_string = NSString::from_str(text);
            self.menu_string.mutableString().setString(&ns_string);
            self.initialize_string_attributes(theme);
            self.resize_and_show(screen_frame, overlay_frame, theme, true);
        })
    }

    /// Shrink font size on large estimated frame size if `auto_resize` is true
    fn draw_attributed_string(
        &self,
        attr_string: Retained<NSMutableAttributedString>,
        screen_frame: &Frame,
        overlay_frame: &Frame,
        theme: &GlyphlowTheme,
        auto_resize: bool,
    ) {
        autoreleasepool(|_| {
            self.menu_string.setAttributedString(&attr_string);
            self.resize_and_show(screen_frame, overlay_frame, theme, auto_resize);
        })
    }

    fn hide(&self) {
        self.container.setHidden(true);
    }
}

pub struct UIDrawer {
    theme: GlyphlowTheme,
    pub root: Retained<CALayer>,
    pub current_screen_frame: Frame,
    /// Large enough frame to cover all screen frames
    pub(super) overlay_frame: Frame,
    pub(super) screen_frames: Vec<Frame>,
    /// Useful for notification clearing
    notifications: Vec<(usize, Menu)>,
    next_notification_id: usize,
    selected_frame: Retained<CALayer>,
    menu: Menu,
}

impl UIDrawer {
    pub fn new(
        screen_frames: Vec<Frame>,
        overlay_frame: Frame,
        mtm: MainThreadMarker,
        theme: &GlyphlowTheme,
    ) -> Self {
        let ns_window = create_overlay_window(mtm);
        let root = CALayer::from_window(&ns_window).expect("Failed to get root layer of window.");

        let menu = Menu::new(theme);
        let selected_frame = CALayer::new();
        selected_frame.setBorderWidth(BORDER_WIDTH);
        selected_frame.setBorderColor(Some(&theme.hint_bg_color));
        selected_frame.setZPosition(-1.0);
        // Hide on init
        selected_frame.setHidden(true);

        // Initialized to middle point on screen
        let current_screen_frame = screen_frames.first().cloned().unwrap_or_default();
        let (x, y) = current_screen_frame.center();
        let middle = calibrated_origin(x, y, &overlay_frame);
        let middle_rect = NSRect::new(middle, NSSize::new(0.0, 0.0));
        menu.container.setFrame(middle_rect);
        selected_frame.setFrame(middle_rect);

        root.addSublayer(&selected_frame);
        root.addSublayer(&menu.container);

        Self {
            theme: theme.clone(),
            root,
            current_screen_frame,
            overlay_frame,
            screen_frames,
            notifications: vec![],
            next_notification_id: 0,
            selected_frame,
            menu,
        }
    }

    pub fn reload_theme(&mut self, new_theme: &GlyphlowTheme) {
        self.selected_frame
            .setBorderColor(Some(&new_theme.hint_bg_color));
        self.menu.load_theme(new_theme);
        self.theme = new_theme.clone();
    }

    pub fn draw_menu(&self, msg: &str) {
        self.menu.draw(
            msg,
            &self.current_screen_frame,
            &self.overlay_frame,
            &self.theme,
        );
    }

    /// Shrink font size on large estimated frame size if `auto_resize` is true
    pub fn draw_attributed_string(
        &self,
        attr_string: Retained<NSMutableAttributedString>,
        auto_resize: bool,
    ) {
        self.menu.draw_attributed_string(
            attr_string,
            &self.current_screen_frame,
            &self.overlay_frame,
            &self.theme,
            auto_resize,
        );
    }

    pub fn draw_frame(&self, frame: &Frame) {
        let x = frame.top_left.x;
        let y = frame.bottom_right.y;
        let origin = calibrated_origin(x, y, &self.overlay_frame);
        let (w, h) = frame.size();
        let frame = NSRect::new(origin, NSSize::new(w, h));
        self.selected_frame.setFrame(frame);
        self.selected_frame.setHidden(false);
    }

    pub fn draw_frame_instant(&self, frame: &Frame) {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.draw_frame(frame);
        CATransaction::commit();
    }

    pub fn notify(&mut self, msg: &str) -> usize {
        let id = self.next_notification_id;
        self.next_notification_id += 1;
        let nl = Menu::new(&self.theme);
        self.root.addSublayer(&nl.container);
        nl.draw(
            msg,
            &self.current_screen_frame,
            &self.overlay_frame,
            &self.theme,
        );
        self.notifications.push((id, nl));
        id
    }

    pub fn clear_notification(&mut self, id: usize) {
        if let Some(pos) = self.notifications.iter().position(|(nid, _)| *nid == id) {
            let (_, nl) = self.notifications.remove(pos);
            nl.free();
            CATransaction::flush();
        }
    }

    pub fn clear_notifications(&mut self) {
        for (_, nl) in self.notifications.iter() {
            nl.free();
        }
        self.notifications.clear();
    }

    pub fn clear_menus(&mut self) {
        self.menu.hide();
        self.clear_notifications();
        CATransaction::flush();
    }

    pub fn clear_menus_instant(&mut self) {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.menu.hide();
        self.clear_notifications();
        CATransaction::commit();
        CATransaction::flush();
    }

    pub fn clear(&mut self) {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.menu.hide();
        self.selected_frame.setHidden(true);
        self.clear_notifications();
        CATransaction::commit();
        CATransaction::flush();
    }
}

pub fn get_screen_frames(mtm: MainThreadMarker) -> Vec<Frame> {
    let screens = NSScreen::screens(mtm);
    if screens.len() > 1 && NSScreen::screensHaveSeparateSpaces(mtm) {
        log::error!(
            "Multiple screens with separate spaces is not supported.\nYou can turn it off in System Preferences -> Desktop & Dock -> Mission Control -> Displays have separate Spaces."
        );
    }

    // NOTE: This app mainly works with AX coordinate system.
    // NS frames converted.
    let primary_height = screens
        .firstObject()
        .map(|s| s.frame().size.height)
        .unwrap_or(0.0);

    screens
        .iter()
        .map(|s| {
            let f = s.frame();
            Frame::new(
                f.origin.x,
                primary_height - (f.origin.y + f.size.height),
                f.origin.x + f.size.width,
                primary_height - f.origin.y,
            )
        })
        .collect()
}

fn create_overlay_window(mtm: MainThreadMarker) -> Retained<NSWindow> {
    unsafe {
        // A large union frame
        let frame = Frame::union_of_frames(
            &NSScreen::screens(mtm)
                .iter()
                .map(|s| Frame::from_cgrect(&s.frame()))
                .collect::<Vec<_>>(),
        )
        .to_cgrect();

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
        // To work across different macOS native workspaces
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        window.makeKeyAndOrderFront(None);

        window
    }
}

trait GlyphlowDrawingLayer {
    fn from_window(window: &Retained<NSWindow>) -> Option<Retained<CALayer>>;
}

impl GlyphlowDrawingLayer for CALayer {
    fn from_window(window: &Retained<NSWindow>) -> Option<Retained<CALayer>> {
        let content_view = window.contentView()?;
        content_view.setWantsLayer(true);
        let root_layer = content_view.layer()?;
        Some(root_layer)
    }
}

/// Coordinate shift, top left -> bottom left
pub fn calibrated_origin(x: f64, y: f64, overlay_frame: &Frame) -> NSPoint {
    NSPoint::new(
        x - overlay_frame.top_left.x,
        overlay_frame.bottom_right.y - y,
    )
}
