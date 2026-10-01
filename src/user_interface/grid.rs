use crate::{
    config::GridConfig,
    user_interface::calibrated_origin,
    util::{Frame, estimate_frame_for_text},
};
use objc2::{
    AnyThread,
    rc::{Retained, autoreleasepool},
};
use objc2_app_kit::{NSFontAttributeName, NSForegroundColorAttributeName};
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSMutableAttributedString, NSPoint, NSRange, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CATextLayer, CATransaction, kCAAlignmentCenter};

const BORDER_WIDTH: f64 = 2.0;

/// The labelled cells of one recursive-grid step.
pub struct GridOverlay {
    container: Retained<CALayer>,
}

impl GridOverlay {
    /// Draw `labels` over `region`, one label per cell, in reading order. A grid too
    /// small to hold a badge drops the labels and keeps only the frames.
    pub fn draw(
        region: &Frame,
        labels: &[char],
        grid: &GridConfig,
        root: &CALayer,
        overlay_frame: &Frame,
    ) -> Self {
        let (rows, cols) = grid.dims();
        let margin = grid.font.pointSize() / 3.0;

        // Every cell is the same size, so one label decides for all of them: if its
        // badge does not fit, no cell gets one.
        let (cell_w, cell_h) = cell_frame(region, rows, cols, 0).size();
        let labelled = labels.first().is_some_and(|label| {
            let (_, badge_size) = measure_badge(&attributed_label(*label, grid), margin);
            badge_size.width <= cell_w && badge_size.height <= cell_h
        });

        let container = autoreleasepool(|_| {
            let container = CALayer::new();
            for (idx, label) in labels.iter().enumerate() {
                let cell = cell_frame(region, rows, cols, idx);
                let (w, h) = cell.size();
                let origin = calibrated_origin(cell.top_left.x, cell.bottom_right.y, overlay_frame);

                let border = CALayer::new();
                border.setFrame(NSRect::new(origin, NSSize::new(w, h)));
                border.setBorderWidth(BORDER_WIDTH);
                border.setBorderColor(Some(&grid.bg_color));
                container.addSublayer(&border);

                if !labelled {
                    continue;
                }

                let attr_string = attributed_label(*label, grid);
                let (text_size, badge_size) = measure_badge(&attr_string, margin);

                let badge = CALayer::new();
                badge.setFrame(NSRect::new(
                    NSPoint::new(
                        origin.x + (w - badge_size.width) / 2.0,
                        origin.y + (h - badge_size.height) / 2.0,
                    ),
                    badge_size,
                ));
                badge.setBackgroundColor(Some(&grid.bg_color));
                badge.setCornerRadius(margin);

                let text_layer = CATextLayer::new();
                text_layer.setContentsScale(2.0);
                unsafe { text_layer.setAlignmentMode(kCAAlignmentCenter) };
                text_layer.setFrame(NSRect::new(NSPoint::new(margin, margin), text_size));
                unsafe { text_layer.setString(Some(&attr_string)) };
                badge.addSublayer(&text_layer);
                container.addSublayer(&badge);
            }
            root.addSublayer(&container);
            container
        });
        CATransaction::flush();
        Self { container }
    }

    pub fn free(&self) {
        self.container.removeFromSuperlayer();
        CATransaction::flush();
    }
}

/// The `idx`-th cell of `region`, in reading order.
pub fn cell_frame(region: &Frame, rows: usize, cols: usize, idx: usize) -> Frame {
    let (w, h) = region.size();
    let (cell_w, cell_h) = (w / cols as f64, h / rows as f64);
    let (row, col) = (idx / cols, idx % cols);
    let x1 = region.top_left.x + col as f64 * cell_w;
    let y1 = region.top_left.y + row as f64 * cell_h;
    Frame::new(x1, y1, x1 + cell_w, y1 + cell_h)
}

fn attributed_label(label: char, grid: &GridConfig) -> Retained<NSMutableAttributedString> {
    let text = NSString::from_str(&label.to_string());
    let attr_string =
        NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &text);
    let full_range = NSRange::new(0, attr_string.length());
    unsafe {
        attr_string.addAttribute_value_range(NSFontAttributeName, &grid.font, full_range);
        attr_string.addAttribute_value_range(
            NSForegroundColorAttributeName,
            grid.fg_color.as_ref(),
            full_range,
        );
    }
    attr_string
}

/// The text size of `text`, and the badge wrapping it.
fn measure_badge(text: &Retained<NSMutableAttributedString>, margin: f64) -> (CGSize, CGSize) {
    let (text_size, _) = estimate_frame_for_text(text, (f64::MAX, f64::MAX));
    let badge_size = CGSize::new(
        text_size.width + margin * 2.0,
        text_size.height + margin * 2.0,
    );
    (text_size, badge_size)
}
