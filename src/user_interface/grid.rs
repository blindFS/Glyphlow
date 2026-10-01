use super::drawer::without_animations;
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

/// The cells of one recursive-grid run.
///
/// The layers are built once and only ever repositioned, so narrowing or widening
/// the grid allocates nothing. A badge rides inside its cell's border, which is
/// what keeps a cell to one sublayer of the container.
pub struct GridOverlay {
    container: Retained<CALayer>,
    /// One per cell, in reading order.
    cells: Vec<CellLayers>,
    /// The first cell's badge size. The fit test is made once, so this one decides
    /// for every cell.
    badge_size: CGSize,
}

/// The two layers of one cell: its border, and the badge labelling it.
struct CellLayers {
    border: Retained<CALayer>,
    badge: Retained<CALayer>,
}

impl GridOverlay {
    /// Build a cell per label and put them over `region`. A grid too small to
    /// hold a badge keeps the frames and drops the labels.
    pub fn draw(
        region: &Frame,
        labels: &[char],
        grid: &GridConfig,
        root: &CALayer,
        overlay_frame: &Frame,
    ) -> Self {
        let mut cells = Vec::with_capacity(labels.len());
        let mut badge_size = CGSize::new(0.0, 0.0);

        let container = autoreleasepool(|_| {
            let container = CALayer::new();
            for (idx, label) in labels.iter().enumerate() {
                let (cell, size) = CellLayers::new(grid, *label);
                if idx == 0 {
                    badge_size = size;
                }
                container.addSublayer(&cell.border);
                cells.push(cell);
            }
            root.addSublayer(&container);
            container
        });

        let overlay = Self {
            container,
            cells,
            badge_size,
        };
        overlay.place(region, grid, overlay_frame);
        overlay
    }

    /// Move the cells over `region`, hiding the labels when a cell is too small
    /// to hold one.
    pub fn place(&self, region: &Frame, grid: &GridConfig, overlay_frame: &Frame) {
        let (rows, cols) = grid.dims();
        let (cell_w, cell_h) = cell_frame(region, rows, cols, 0).size();
        let labelled = self.badge_size.width <= cell_w && self.badge_size.height <= cell_h;

        without_animations! {
            for (idx, cell) in self.cells.iter().enumerate() {
                cell.place(cell_rect(region, rows, cols, idx, overlay_frame));
                cell.badge.setHidden(!labelled);
            }
        }
    }

    pub fn free(&self) {
        self.container.removeFromSuperlayer();
        CATransaction::flush();
    }
}

impl CellLayers {
    /// The border and badge of one cell, with `label` laid out inside the badge.
    /// The badge size comes back out because it also decides whether any cell can
    /// show a label at all.
    fn new(grid: &GridConfig, label: char) -> (Self, CGSize) {
        let margin = badge_margin(grid);
        let text = attributed_label(label, grid);
        let (text_size, badge_size) = measure_badge(&text, margin);

        let border = CALayer::new();
        border.setBorderWidth(BORDER_WIDTH);
        border.setBorderColor(Some(&grid.bg_color));

        let badge = CALayer::new();
        badge.setBounds(NSRect::new(NSPoint::new(0.0, 0.0), badge_size));
        badge.setBackgroundColor(Some(&grid.bg_color));
        badge.setCornerRadius(margin);

        let text_layer = CATextLayer::new();
        text_layer.setContentsScale(2.0);
        unsafe { text_layer.setAlignmentMode(kCAAlignmentCenter) };
        text_layer.setFrame(NSRect::new(NSPoint::new(margin, margin), text_size));
        unsafe { text_layer.setString(Some(&text)) };
        badge.addSublayer(&text_layer);

        border.addSublayer(&badge);
        (Self { border, badge }, badge_size)
    }

    /// Put the cell over `rect`, in the overlay's layer coordinates. The badge
    /// keeps its size and only follows the cell's centre.
    fn place(&self, rect: NSRect) {
        self.border.setFrame(rect);
        self.badge
            .setPosition(NSPoint::new(rect.size.width / 2.0, rect.size.height / 2.0));
    }
}

/// The `idx`-th cell of `region`, in the overlay's layer coordinates.
fn cell_rect(
    region: &Frame,
    rows: usize,
    cols: usize,
    idx: usize,
    overlay_frame: &Frame,
) -> NSRect {
    let cell = cell_frame(region, rows, cols, idx);
    let (w, h) = cell.size();
    NSRect::new(
        calibrated_origin(cell.top_left.x, cell.bottom_right.y, overlay_frame),
        NSSize::new(w, h),
    )
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

/// The region one level up from `region`: the cell of its parent that holds it.
///
/// A cell is `cols` by `rows` smaller than the region it came from, so `region`'s
/// own size is the cell size at its level. Counting how many such cells `region`'s
/// origin sits from `outer`'s gives an exact integer, so rounding only undoes float
/// drift; its remainder modulo the grid width is `region`'s own column, and the
/// parent's edge is that many cells back.
pub fn parent_frame(region: &Frame, outer: &Frame, rows: usize, cols: usize) -> Frame {
    let (w, h) = region.size();
    let cells_x = ((region.top_left.x - outer.top_left.x) / w).round() as i64;
    let cells_y = ((region.top_left.y - outer.top_left.y) / h).round() as i64;

    let x1 = region.top_left.x - (cells_x.rem_euclid(cols as i64) as f64) * w;
    let y1 = region.top_left.y - (cells_y.rem_euclid(rows as i64) as f64) * h;
    Frame::new(x1, y1, x1 + w * cols as f64, y1 + h * rows as f64)
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

/// The gap between a label and the edge of its badge.
fn badge_margin(grid: &GridConfig) -> f64 {
    grid.font.pointSize() / 3.0
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
