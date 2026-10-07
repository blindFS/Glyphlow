use crate::{
    ScrollAction,
    config::{GlyphlowTheme, cgcolor_to_rgba},
    util::{Frame, estimate_frame_for_text, format_fixed_width},
};
use objc2::{
    AnyThread, MainThreadMarker, MainThreadOnly,
    rc::{DefaultRetained, Retained, autoreleasepool},
};
use objc2_app_kit::{
    NSBackgroundColorAttributeName, NSBackingStoreType, NSColor, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSMutableParagraphStyle, NSParagraphStyleAttributeName,
    NSScreen, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CFRetained, CGSize};
use objc2_core_graphics::CGColor;
use objc2_foundation::{NSMutableAttributedString, NSPoint, NSRange, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CATextLayer, CATransaction};
use std::cell::Cell;
use std::ops::Range;

/// Runs `body` with Core Animation actions disabled, so the layer changes it
/// makes take effect on the next frame instead of animating.
///
/// `begin` and `commit` have to be paired, and a stray `begin` leaves the
/// transaction open for every later change on the thread. Keeping the pair
/// inside one construct is the point.
macro_rules! without_animations {
    ($($body:tt)*) => {{
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        $($body)*
        CATransaction::commit();
    }};
}

pub(crate) use without_animations;

/// How a span of text stands out from the rest of the string.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuStyle {
    Key,
    Text,
    Header,
    Error,
    Dim,
}

/// How far [`MenuStyle::Dim`] fades the menu's foreground colour.
const DIM_ALPHA_DIVISOR: u8 = 3;

/// `color` faded to a fraction of its alpha, for text that should recede.
fn dimmed(color: &CFRetained<CGColor>) -> CFRetained<CGColor> {
    let (r, g, b, a) = cgcolor_to_rgba(color).unwrap_or((255, 255, 255, 255));
    let channel = |c: u8| c as f64 / 255.0;
    CGColor::new_generic_rgb(
        channel(r),
        channel(g),
        channel(b),
        channel(a / DIM_ALPHA_DIVISOR),
    )
}

impl MenuStyle {
    /// The colours to paint a span with: a foreground, plus a background when
    /// the style inverts the text.
    ///
    /// `dim` is passed in because it is derived from the theme's foreground
    /// rather than stored in it, so one is built per string, not per span.
    fn colors<'a>(
        self,
        theme: &'a GlyphlowTheme,
        dim: &'a CFRetained<CGColor>,
    ) -> (&'a CFRetained<CGColor>, Option<&'a CFRetained<CGColor>>) {
        match self {
            Self::Key => (&theme.menu_hl_color, None),
            Self::Text => (&theme.menu_text_hl_color, None),
            Self::Header => (&theme.menu_header_hl_color, None),
            Self::Error => (&theme.menu_error_hl_color, None),
            Self::Dim => (dim, None),
        }
    }
}

/// The text of a menu, plus the spans to draw in an accent colour.
///
/// A span is a range in UTF-16 code units, the unit `NSAttributedString` counts
/// in, so a row carrying non-BMP glyphs styles the characters it means.
pub struct MenuString {
    pub text: String,
    spans: Vec<(Range<usize>, MenuStyle)>,
    /// UTF-16 length of `text`, the offset the next span starts at.
    utf16_len: usize,
}

impl MenuString {
    /// Append `text` in the menu's foreground colour.
    pub fn push(&mut self, text: &str) -> &mut Self {
        self.text.push_str(text);
        self.utf16_len += text.encode_utf16().count();
        self
    }

    /// Append `text` in `style`'s colour.
    pub fn push_styled(&mut self, text: &str, style: MenuStyle) -> &mut Self {
        let start = self.utf16_len;
        self.push(text);
        if self.utf16_len > start {
            self.spans.push((start..self.utf16_len, style));
        }
        self
    }

    /// A message that is exactly `text`, in `style`.
    pub fn styled(text: &str, style: MenuStyle) -> Self {
        let mut msg = Self::from("");
        msg.push_styled(text, style);
        msg
    }

    /// Append a `(key) display` row on its own line, highlighting the key: padded
    /// to `key_width`, with the `prefix_len` characters already typed dimmed.
    pub fn push_key_map_entry(
        &mut self,
        key: &str,
        display: &str,
        prefix_len: usize,
        key_width: usize,
    ) -> &mut Self {
        let padding = " ".repeat(key_width - key.chars().count());
        let rest = key
            .char_indices()
            .nth(prefix_len)
            .map_or(key.len(), |(byte, _)| byte);
        self.push("\n")
            .push(&padding)
            .push_styled("(", MenuStyle::Dim)
            .push_styled(&"_".repeat(prefix_len), MenuStyle::Dim)
            .push_styled(&key[rest..], MenuStyle::Key)
            .push_styled(")", MenuStyle::Dim)
            .push(" ")
            .push(display)
    }
}

impl From<&str> for MenuString {
    fn from(text: &str) -> Self {
        let mut msg = Self {
            text: String::new(),
            spans: Vec::new(),
            utf16_len: 0,
        };
        msg.push(text);
        msg
    }
}

/// For efficient incremental drawing
struct Drawn {
    /// In UTF-16 units, as `NSAttributedString` counts them.
    spans: Vec<(Range<usize>, MenuStyle)>,
    screen_frame: Frame,
    overlay_frame: Frame,
}

struct Menu {
    container: Retained<CALayer>,
    text_layer: Retained<CATextLayer>,
    menu_string: Retained<NSMutableAttributedString>,
    drawn: Cell<Option<Drawn>>,
}

const BORDER_WIDTH: f64 = 2.0;
const MIN_FONT_SIZE: f64 = 10.0;
const SEARCH_BAR_WIDTH: usize = 10;
/// Kept off the text layer's width so its widest line never sits exactly on the
/// wrap boundary, where CoreText's own rounding can break the line.
const TEXT_WIDTH_SLACK: f64 = 1.0;

impl Menu {
    fn new(theme: &GlyphlowTheme) -> Self {
        autoreleasepool(|_| {
            let text_layer = CATextLayer::new();
            text_layer.setContentsScale(2.0);
            text_layer.setWrapped(true);

            let container = CALayer::new();
            container.setBorderWidth(BORDER_WIDTH);
            container.addSublayer(&text_layer);
            container.setZPosition(1.0);
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
                drawn: Cell::new(None),
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
        // The spans hold the colours they were painted in, so a new theme has to
        // be painted over the whole string again.
        self.drawn.set(None);
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

    /// Shrink the font if the text does not fit `screen_frame`.
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
                    &NSFont::fontWithName_size(&font.fontName(), font_size)
                        .expect("Failed to resize font."),
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
        let text_width = size.width + TEXT_WIDTH_SLACK;
        let margin = theme.menu_margin_size as f64;
        let box_width = text_width + (margin * 2.0);
        let box_height = size.height + (margin * 2.0);

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
            NSSize::new(text_width, size.height),
        ));

        self.refresh_text();
        self.container.setHidden(false);
    }

    fn refresh_text(&self) {
        unsafe {
            self.text_layer.setString(Some(&self.menu_string));
        }
    }

    fn draw(
        &self,
        menu: &MenuString,
        screen_frame: &Frame,
        overlay_frame: &Frame,
        theme: &GlyphlowTheme,
    ) {
        autoreleasepool(|_| {
            let spans = menu.spans.clone();
            // While the layer still holds this text, laid out for these frames,
            // only the colours can have changed: the string, the base attributes
            // and the measured size are all still in place.
            let laid_out = self
                .menu_string
                .string()
                .isEqualToString(&NSString::from_str(&menu.text));
            let previous = self.drawn.take().filter(|drawn| {
                laid_out
                    && drawn.screen_frame == *screen_frame
                    && drawn.overlay_frame == *overlay_frame
            });

            let Some(mut previous) = previous else {
                let ns_string = NSString::from_str(&menu.text);
                self.menu_string.mutableString().setString(&ns_string);
                self.initialize_string_attributes(theme);
                self.restyle(theme, &spans, &[]);
                self.resize_and_show(screen_frame, overlay_frame, theme, true);
                self.drawn.set(Some(Drawn {
                    spans,
                    screen_frame: *screen_frame,
                    overlay_frame: *overlay_frame,
                }));
                return;
            };

            if self.restyle(theme, &spans, &previous.spans) {
                self.refresh_text();
            }
            // The frame is the one already in place, but the menu may have been
            // hidden since it was drawn.
            self.show();
            previous.spans = spans;
            self.drawn.set(Some(previous));
        })
    }

    /// Repaint the spans whose style differs from `previous`, and return whether
    /// anything was repainted.
    ///
    /// Both lists index the same text, so they are compared run by run; a range
    /// that is styled now but was not, or the other way round, goes back to the
    /// foreground colour the whole string was given.
    fn restyle(
        &self,
        theme: &GlyphlowTheme,
        spans: &[(Range<usize>, MenuStyle)],
        previous: &[(Range<usize>, MenuStyle)],
    ) -> bool {
        let plan = restyle_plan(previous, spans);
        if plan.is_empty() {
            return false;
        }

        let dim = dimmed(&theme.menu_fg_color);
        unsafe {
            self.menu_string.beginEditing();
            for (range, style) in plan {
                let (fg, bg) = match style {
                    Some(style) => style.colors(theme, &dim),
                    None => (&theme.menu_fg_color, None),
                };
                let range = NSRange::new(range.start, range.end - range.start);
                self.menu_string.addAttribute_value_range(
                    NSForegroundColorAttributeName,
                    fg.as_ref(),
                    range,
                );
                if let Some(bg) = bg {
                    self.menu_string.addAttribute_value_range(
                        NSBackgroundColorAttributeName,
                        bg.as_ref(),
                        range,
                    );
                }
            }
            self.menu_string.endEditing();
        }
        true
    }

    /// Draw `attr_string`, shrinking the font if `auto_resize` is set.
    fn draw_attributed_string(
        &self,
        attr_string: Retained<NSMutableAttributedString>,
        screen_frame: &Frame,
        overlay_frame: &Frame,
        theme: &GlyphlowTheme,
        auto_resize: bool,
    ) {
        autoreleasepool(|_| {
            // The whole string is replaced, so nothing from the last draw can be
            // reused.
            self.drawn.set(None);
            self.menu_string.setAttributedString(&attr_string);
            self.resize_and_show(screen_frame, overlay_frame, theme, auto_resize);
        })
    }

    fn hide(&self) {
        self.container.setHidden(true);
    }

    fn show(&self) {
        self.container.setHidden(false);
    }
}

pub struct UIDrawer {
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
    search_bar: Menu,
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
        let search_bar = Menu::new(theme);
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

        // Search bar initialized as fixed width
        let dummy_text = format!("/{}", "_".repeat(SEARCH_BAR_WIDTH));
        search_bar.draw(
            &MenuString::from(dummy_text.as_str()),
            &current_screen_frame,
            &overlay_frame,
            theme,
        );
        search_bar.hide();

        root.addSublayer(&selected_frame);
        root.addSublayer(&menu.container);
        root.addSublayer(&search_bar.container);

        Self {
            root,
            current_screen_frame,
            overlay_frame,
            screen_frames,
            notifications: vec![],
            next_notification_id: 0,
            selected_frame,
            menu,
            search_bar,
        }
    }

    pub fn select_screen_frame(&mut self, window_frame: &Frame) {
        if let Some(sf) = self
            .screen_frames
            .iter()
            .max_by_key(|f| f.intersect(window_frame).map(|f| f.area().to_bits()))
        {
            self.current_screen_frame = *sf;
        }
    }

    pub fn reload_theme(&mut self, new_theme: &GlyphlowTheme) {
        self.selected_frame
            .setBorderColor(Some(&new_theme.hint_bg_color));
        self.menu.load_theme(new_theme);
        self.search_bar.load_theme(new_theme);
    }

    pub fn draw_menu(&self, msg: &MenuString, theme: &GlyphlowTheme) {
        self.menu
            .draw(msg, &self.current_screen_frame, &self.overlay_frame, theme);
    }

    pub fn hide_search_bar(&self) {
        self.search_bar.hide();
    }

    pub fn draw_search_bar(&self, prefix: &str, init: bool) {
        let msg = format!("/{}", format_fixed_width(prefix, SEARCH_BAR_WIDTH));
        autoreleasepool(|_| {
            if init {
                // Disable movement animations
                without_animations! {
                    self.reposition_search_bar();
                }
                self.search_bar.show();
            }

            // Disable animation to improve responsiveness
            without_animations! {
                let ns_string = NSString::from_str(&msg);
                self.search_bar
                    .menu_string
                    .mutableString()
                    .setString(&ns_string);
                self.search_bar.refresh_text();
            }
        });
    }

    fn reposition_search_bar(&self) {
        if self.menu.container.isHidden() {
            let (x, y) = self.current_screen_frame.center();
            self.search_bar.container.setPosition(NSPoint::new(x, y));
            return;
        }
        let menu_frame = self.menu.container.frame();
        let search_frame = self.search_bar.container.frame();
        let search_height = search_frame.size.height;
        let y = if menu_frame.size.height + search_height > self.current_screen_frame.size().1 {
            menu_frame.origin.y + menu_frame.size.height - search_height
        } else {
            menu_frame.origin.y + menu_frame.size.height
        };
        let origin = NSPoint::new(menu_frame.origin.x, y);
        self.search_bar
            .container
            .setFrame(NSRect::new(origin, search_frame.size));
    }

    /// Draw `attr_string` in the menu, shrinking the font if `auto_resize` is
    /// set.
    pub fn draw_attributed_string(
        &self,
        theme: &GlyphlowTheme,
        attr_string: Retained<NSMutableAttributedString>,
        auto_resize: bool,
    ) {
        self.menu.draw_attributed_string(
            attr_string,
            &self.current_screen_frame,
            &self.overlay_frame,
            theme,
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
        without_animations! {
            self.draw_frame(frame);
        }
    }

    pub fn notify(&mut self, theme: &GlyphlowTheme, msg: &MenuString) -> usize {
        let id = self.next_notification_id;
        self.next_notification_id += 1;
        let nl = Menu::new(theme);
        self.root.addSublayer(&nl.container);
        nl.draw(msg, &self.current_screen_frame, &self.overlay_frame, theme);
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
        self.search_bar.hide();
        self.clear_notifications();
    }

    pub fn clear_menus_instant(&mut self) {
        without_animations! {
            self.menu.hide();
            self.search_bar.hide();
            self.clear_notifications();
        }
        CATransaction::flush();
    }

    pub fn clear(&mut self) {
        without_animations! {
            self.menu.drawn.set(None);
            self.menu.hide();
            self.search_bar.hide();
            self.selected_frame.setHidden(true);
            self.clear_notifications();
        }
        CATransaction::flush();
    }

    pub fn menu_height(&self) -> f64 {
        self.menu.container.frame().size.height
    }

    pub fn scroll_menu(&self, sa: ScrollAction, scroll_distance: f64) {
        let frame = self.menu.container.frame();
        let box_height = frame.size.height;
        let screen_height = self.current_screen_frame.size().1;
        if box_height <= screen_height {
            return;
        }

        // Calculate screen top and bottom in Cocoa y
        let screen_top_cocoa_y =
            self.overlay_frame.bottom_right.y - self.current_screen_frame.top_left.y;
        let screen_bottom_cocoa_y =
            self.overlay_frame.bottom_right.y - self.current_screen_frame.bottom_right.y;

        let initial_y = screen_top_cocoa_y - box_height;
        let bottom_y = screen_bottom_cocoa_y;

        let step = screen_height * scroll_distance;
        let mut y = frame.origin.y;

        match sa {
            ScrollAction::DownRight => {
                y += step;
            }
            ScrollAction::UpLeft => {
                y -= step;
            }
            ScrollAction::Top => {
                y = initial_y;
            }
            ScrollAction::Bottom => {
                y = bottom_y;
            }
            _ => {}
        }

        let new_y = y.clamp(initial_y, bottom_y);
        let mut new_frame = frame;
        new_frame.origin.y = new_y;

        self.menu.container.setFrame(new_frame);
        CATransaction::flush();
    }
}

/// The screens in AX coordinates (top-left origin).
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

/// Screen coordinates (top-left origin) into layer coordinates (bottom-left):
/// shift by the overlay origin, then flip the y axis. The overlay is the union of
/// every screen frame, so its origin is not necessarily `(0, 0)`.
pub fn calibrated_origin(x: f64, y: f64, overlay_frame: &Frame) -> NSPoint {
    NSPoint::new(
        x - overlay_frame.top_left.x,
        overlay_frame.bottom_right.y - y,
    )
}

/// The ranges of `spans` to repaint, with the style to paint them in; `None`
/// puts a range back to the foreground colour the whole string was given.
///
/// Both lists are sorted and disjoint, so every one of their boundaries starts a
/// run whose style is fixed on either side, and only a run whose two sides
/// disagree has to be repainted.
fn restyle_plan(
    previous: &[(Range<usize>, MenuStyle)],
    spans: &[(Range<usize>, MenuStyle)],
) -> Vec<(Range<usize>, Option<MenuStyle>)> {
    if previous == spans {
        return Vec::new();
    }

    let mut boundaries: Vec<usize> = previous
        .iter()
        .chain(spans)
        .flat_map(|(span, _)| [span.start, span.end])
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();

    let (mut old, mut new) = (0, 0);
    let mut plan = Vec::new();
    for window in boundaries.windows(2) {
        let (start, end) = (window[0], window[1]);
        let before = style_at(previous, &mut old, start);
        let after = style_at(spans, &mut new, start);
        if before != after {
            plan.push((start..end, after));
        }
    }
    plan
}

/// The style covering `at` in a sorted, disjoint span list, advancing `cursor`
/// past every span that ends before it.
///
/// Only constant amortized: one call walks as many spans as it has to skip, so
/// the caller has to ask for ascending positions — which is what keeps a whole
/// walk linear, since then each span is skipped once and the cursor never
/// rewinds.
fn style_at(
    spans: &[(Range<usize>, MenuStyle)],
    cursor: &mut usize,
    at: usize,
) -> Option<MenuStyle> {
    while spans.get(*cursor).is_some_and(|s| s.0.end <= at) {
        *cursor += 1;
    }
    spans.get(*cursor).filter(|s| s.0.start <= at).map(|s| s.1)
}

#[cfg(test)]
mod calibrated_origin_tests {
    use super::*;
    use rstest::rstest;

    /// Core Animation places layers in a bottom-left origin space while the rest
    /// of the app works in screen (top-left) coordinates, so a vertical flip is
    /// always needed. The overlay is not necessarily anchored at the origin —
    /// with more than one display it is the union of every screen frame — hence
    /// the shift as well.
    #[rstest]
    // A single 1920x1080 display at the origin: the shift is the identity.
    #[case::top_left_corner(0.0, 0.0, (0.0, 0.0, 1920.0, 1080.0), (0.0, 1080.0))]
    #[case::bottom_right_corner(1920.0, 1080.0, (0.0, 0.0, 1920.0, 1080.0), (1920.0, 0.0))]
    // A display left of the main one: the overlay origin is negative, so the
    // horizontal shift is not the identity either.
    #[case::overlay_offset_to_the_left(-1870.0, 200.0, (-1920.0, 0.0, 0.0, 1080.0), (50.0, 880.0))]
    // An overlay taller than a single screen, e.g. stacked displays.
    #[case::overlay_taller_than_one_screen(100.0, 200.0, (0.0, 0.0, 1920.0, 2160.0), (100.0, 1960.0))]
    fn shifts_then_flips_into_layer_coordinates(
        #[case] x: f64,
        #[case] y: f64,
        #[case] overlay: (f64, f64, f64, f64),
        #[case] expected: (f64, f64),
    ) {
        let overlay = Frame::new(overlay.0, overlay.1, overlay.2, overlay.3);
        let origin = calibrated_origin(x, y, &overlay);
        assert_eq!((origin.x, origin.y), expected);
    }
}
