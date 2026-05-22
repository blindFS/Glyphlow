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

static BORDER_WIDTH: f64 = 2.0;
static MIN_FONT_SIZE: f64 = 10.0;

impl Menu {
    fn new(theme: &GlyphlowTheme) -> Self {
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

    fn estimate_text_size(&self, screen_size: CGSize, theme: &GlyphlowTheme) -> CGSize {
        let CGSize { width, height } = screen_size;
        let (size, visible_len) = estimate_frame_for_text(&self.menu_string, (width, height));
        let shrinkage = (width / size.width)
            .min(height / size.height)
            .min(visible_len as f64 / self.menu_string.length() as f64);
        let font = &theme.menu_font;

        // NOTE: if estimated size is too large, reduce font size and re-estimate
        if shrinkage < 1.0 {
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

    fn resize_and_show(&self, screen_size: CGSize, theme: &GlyphlowTheme) {
        let size = self.estimate_text_size(screen_size, theme);
        let CGSize { width, height } = size;
        let margin = theme.menu_margin_size as f64;
        let box_width = width + (margin * 2.0);
        let box_height = height + (margin * 2.0);

        let (o_x, o_y) = (
            (screen_size.width - box_width) / 2.0,
            (screen_size.height - box_height) / 2.0,
        );

        let o_x_move = o_x.min(screen_size.width - box_width).max(0.0);
        let o_y_move = o_y.max(0.0).min(screen_size.height - box_height);
        let origin = NSPoint::new(o_x_move, o_y_move);

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

    fn draw(&self, text: &str, screen_size: CGSize, theme: &GlyphlowTheme) {
        autoreleasepool(|_| {
            let ns_string = NSString::from_str(text);
            self.menu_string.mutableString().setString(&ns_string);
            self.initialize_string_attributes(theme);
            self.resize_and_show(screen_size, theme);
        })
    }

    fn draw_attributed_string(
        &self,
        attr_string: Retained<NSMutableAttributedString>,
        screen_size: CGSize,
        theme: &GlyphlowTheme,
    ) {
        autoreleasepool(|_| {
            self.menu_string.setAttributedString(&attr_string);
            self.resize_and_show(screen_size, theme);
        })
    }

    fn hide(&self) {
        self.container.setHidden(true);
    }
}

pub struct UIDrawer {
    theme: GlyphlowTheme,
    pub root: Retained<CALayer>,
    screen_size: CGSize,
    /// Useful for notification clearing
    notification_layers: Vec<Menu>,
    selected_frame: Retained<CALayer>,
    menu: Menu,
}

impl UIDrawer {
    pub fn new(screen_size: CGSize, mtm: MainThreadMarker, theme: &GlyphlowTheme) -> Self {
        let ns_window = create_overlay_window(mtm, screen_size);
        // To work across different macOS native workspaces
        ns_window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        ns_window.makeKeyAndOrderFront(None);
        let root = CALayer::from_window(&ns_window).expect("Failed to get root layer of window.");

        let menu = Menu::new(theme);
        let selected_frame = CALayer::new();
        selected_frame.setBorderWidth(BORDER_WIDTH);
        selected_frame.setBorderColor(Some(&theme.hint_bg_color));
        selected_frame.setZPosition(-1.0);
        // Hide on init
        selected_frame.setHidden(true);

        // Initialized to middle point on screen
        let middle = NSPoint::new(screen_size.width / 2.0, screen_size.height / 2.0);
        let middle_rect = NSRect::new(middle, NSSize::new(0.0, 0.0));
        menu.container.setFrame(middle_rect);
        selected_frame.setFrame(middle_rect);

        root.addSublayer(&selected_frame);
        root.addSublayer(&menu.container);

        Self {
            theme: theme.clone(),
            root,
            screen_size,
            notification_layers: vec![],
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
        self.menu.draw(msg, self.screen_size, &self.theme);
    }

    pub fn draw_attributed_string(&self, attr_string: Retained<NSMutableAttributedString>) {
        self.menu
            .draw_attributed_string(attr_string, self.screen_size, &self.theme);
    }

    pub fn draw_frame(&self, frame: &Frame) {
        let origin = frame.top_left;
        let origin = NSPoint::new(origin.x, origin.y);
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

    pub fn notify(&mut self, msg: &str) {
        let nl = Menu::new(&self.theme);
        self.root.addSublayer(&nl.container);
        nl.draw(msg, self.screen_size, &self.theme);
        self.notification_layers.push(nl);
    }

    // TODO: per notification clearing
    pub fn clear_notifications(&mut self) {
        for nl in self.notification_layers.iter() {
            nl.free();
        }
        self.notification_layers.clear();
    }

    pub fn clear_menus(&mut self) {
        self.menu.hide();
        self.clear_notifications();
        CATransaction::flush();
    }

    pub fn clear(&mut self) {
        self.menu.hide();
        self.selected_frame.setHidden(true);
        self.clear_notifications();
        CATransaction::flush();
    }
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
