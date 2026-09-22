use crate::config::{GlyphlowTheme, HintKeys};
use crate::user_interface::calibrated_origin;
use crate::util::{Frame, estimate_frame_for_text};
use objc2::{AnyThread, rc::Retained};
use objc2_core_foundation::{CFRetained, CGSize};
use objc2_core_graphics::{CGColor, CGMutablePath};
use objc2_foundation::{NSMutableAttributedString, NSPoint, NSRange, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CAShapeLayer, CATextLayer, kCAAlignmentCenter};
use rstar::{AABB, RTree, RTreeObject};
use std::collections::{HashMap, VecDeque};

/// One hint: the key label, the element frame it points at, and the layers that
/// draw both.
#[derive(Debug, Clone, PartialEq)]
pub struct HintBox {
    pub label: String,
    x: f64,
    y: f64,
    pub idx: usize,
    pub disabled: bool,
    /// Moved distance to avoid collision
    delta: (f64, f64),
    pub frame: Frame,
    color: Option<CFRetained<CGColor>>,
    text_layer: Retained<CATextLayer>,
    pub(super) box_layer: Retained<CALayer>,
    tri_layer: Retained<CAShapeLayer>,
    pub(super) frame_layer: Option<Retained<CALayer>>,
}

impl HintBox {
    pub fn new(
        idx: usize,
        label: String,
        x: f64,
        y: f64,
        frame: Frame,
        color: Option<CFRetained<CGColor>>,
    ) -> Self {
        let bl = CALayer::new();
        let tl = CATextLayer::new();
        tl.setWrapped(true);
        unsafe {
            tl.setAlignmentMode(kCAAlignmentCenter);
        }
        tl.setContentsScale(2.0);
        bl.addSublayer(&tl);

        let tri_layer = CAShapeLayer::new();
        bl.insertSublayer_atIndex(&tri_layer, 0);

        let frame_layer = color.as_ref().map(|_| {
            let fl = CALayer::new();
            fl.setBorderWidth(2.0);
            fl.setZPosition(-1.0);
            fl
        });

        Self {
            label,
            x,
            y,
            idx,
            disabled: false,
            delta: (0.0, 0.0),
            frame,
            color,
            text_layer: tl,
            box_layer: bl,
            tri_layer,
            frame_layer,
        }
    }

    pub fn is_colored(&self) -> bool {
        self.color.is_some()
    }

    /// Replaces the label and lays the box out to match.
    ///
    /// This is the only way a box's size changes, so laying out here is what lets
    /// every later [`Self::refresh`] skip the question entirely.
    ///
    /// Call it once the box's position is final — the layout moves the layer, and
    /// a subsequent move would restart the animation it just started.
    pub fn set_label(&mut self, label: String, overlay_frame: &Frame, theme: &GlyphlowTheme) {
        self.label = label;
        self.layout(overlay_frame, theme);
    }

    /// Size of the triangle pointing from the box at the element.
    fn geometry(theme: &GlyphlowTheme) -> (f64, f64) {
        let font_size = theme.hint_font.pointSize();
        (font_size / 2.0, font_size / 2.0) // (width, height)
    }

    fn attributed_string(
        &self,
        prefix_len: usize,
        theme: &GlyphlowTheme,
    ) -> Retained<NSMutableAttributedString> {
        let label_string = NSString::from_str(&self.label);
        let attr_string = NSMutableAttributedString::initWithString(
            NSMutableAttributedString::alloc(),
            &label_string,
        );
        update_hint_text_with_attr(&attr_string, &self.label, prefix_len, theme);
        attr_string
    }

    /// Where to put the box so it stays inside `overlay_frame`, plus how far it
    /// had to be nudged to get there — the triangle needs that to keep pointing
    /// at the element.
    fn calculate_origin(
        &self,
        box_size: CGSize,
        overlay_frame: &Frame,
        tri_height: f64,
    ) -> (NSPoint, f64, f64) {
        let (box_width, box_height) = (box_size.width, box_size.height);
        let (o_x, o_y) = (self.x - box_width / 2.0, self.y + tri_height + box_height);
        let o_x_move = o_x
            .min(overlay_frame.bottom_right.x - box_width)
            .max(overlay_frame.top_left.x);
        let o_y_move = o_y
            .max(overlay_frame.top_left.y)
            .min(overlay_frame.bottom_right.y);
        (
            calibrated_origin(o_x_move, o_y_move, overlay_frame),
            o_x - o_x_move,
            o_y - o_y_move,
        )
    }

    fn create_triangle_path(
        &self,
        tri_width: f64,
        tri_height: f64,
        x_offset: f64,
        y_offset: f64,
    ) -> Retained<CGMutablePath> {
        let path = CGMutablePath::new();
        unsafe {
            CGMutablePath::move_to_point(Some(&path), std::ptr::null(), 0.0, 0.0);
            CGMutablePath::add_line_to_point(
                Some(&path),
                std::ptr::null(),
                tri_width / 2.0 - self.delta.0 + x_offset,
                tri_height + self.delta.1 - y_offset,
            );
            CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), tri_width, 0.0);
        }
        CGMutablePath::close_subpath(Some(&path));
        path.into()
    }

    pub fn draw(
        &mut self,
        root_layer: &CALayer,
        theme: &GlyphlowTheme,
        key_prefix_len: usize,
        overlay_frame: &Frame,
    ) {
        let bg_color = self.color.as_ref().unwrap_or(&theme.hint_bg_color);

        // Frame Layer
        if let Some(fl) = &self.frame_layer {
            let x = self.frame.top_left.x;
            let y = self.frame.bottom_right.y;
            let (w, h) = self.frame.size();
            fl.setFrame(NSRect::new(
                calibrated_origin(x, y, overlay_frame),
                NSSize::new(w, h),
            ));
            fl.setBorderColor(Some(bg_color));
            if fl.superlayer().is_none() {
                root_layer.addSublayer(fl);
            }
        }

        // Text & Box Layer
        self.box_layer.setBackgroundColor(Some(bg_color));
        self.box_layer
            .setCornerRadius(theme.hint_margin_size as f64);
        self.tri_layer.setFillColor(Some(bg_color));

        self.render(key_prefix_len, theme);
        self.layout(overlay_frame, theme);

        if self.box_layer.superlayer().is_none() {
            root_layer.addSublayer(&self.box_layer);
        }
    }

    /// The one place that measures the label.
    ///
    /// The size is a pure function of the label text — a `W` is wider than an
    /// `I`, so it cannot be derived from the alphabet or the digit count — which
    /// makes this the single answer to "how big is this hint". It therefore runs
    /// where the label is created or replaced, and never on a redraw.
    fn layout(&self, overlay_frame: &Frame, theme: &GlyphlowTheme) {
        let attr_string = self.attributed_string(0, theme);
        let (text_size, _) = estimate_frame_for_text(
            &attr_string,
            (f64::MAX, f64::MAX), // No constraints for label
        );

        let margin = theme.hint_margin_size as f64;
        self.text_layer
            .setFrame(NSRect::new(NSPoint::new(margin, margin), text_size));

        let box_size = CGSize::new(
            text_size.width + (margin * 2.0),
            text_size.height + (margin * 2.0),
        );
        self.place_box(box_size, overlay_frame, theme);
    }

    /// Puts the box at the origin `box_size` gives it, and redraws its triangle.
    fn place_box(&self, box_size: CGSize, overlay_frame: &Frame, theme: &GlyphlowTheme) {
        let (tri_width, tri_height) = Self::geometry(theme);
        let (origin, x_offset, y_offset) =
            self.calculate_origin(box_size, overlay_frame, tri_height);

        self.box_layer.setFrame(NSRect::new(origin, box_size));
        self.tri_layer.setFrame(NSRect::new(
            NSPoint::new((box_size.width - tri_width) / 2.0, box_size.height),
            NSSize::new(tri_width, tri_height),
        ));

        let path = self.create_triangle_path(tri_width, tri_height, x_offset, y_offset);
        self.tri_layer.setPath(Some(&path));
    }

    /// The one place that builds the attributed string and hands it to the layer.
    fn render(&self, prefix_len: usize, theme: &GlyphlowTheme) {
        let attr_string = self.attributed_string(prefix_len, theme);
        unsafe {
            self.text_layer.setString(Some(&attr_string));
        }
    }

    /// Follows the box when collision resolution has moved it.
    ///
    /// A resize needs no handling here: the label's size only changes through
    /// [`Self::set_label`], which lays the box out again on the spot.
    fn update_position(&self, screen_frame: &Frame, theme: &GlyphlowTheme) {
        if self.delta == (0.0, 0.0) {
            return;
        }
        self.place_box(self.box_layer.frame().size, screen_frame, theme);
    }

    /// Re-render the text at `prefix_len`, then follow the box if it moved.
    pub fn refresh(&self, prefix_len: usize, screen_frame: &Frame, theme: &GlyphlowTheme) {
        self.render(prefix_len, theme);
        self.update_position(screen_frame, theme);
        if prefix_len > 0
            && let Some(frame_layer) = &self.frame_layer
        {
            frame_layer.setOpacity(1.0);
        }
    }

    pub fn set_opacity(&self, opacity: f32) {
        self.box_layer.setOpacity(opacity);
        if let Some(fl) = &self.frame_layer {
            fl.setOpacity(opacity);
        }
    }

    pub fn set_visible(&self, visible: bool) {
        self.box_layer.setHidden(!visible);
        if let Some(fl) = &self.frame_layer {
            fl.setHidden(!visible);
        }
    }

    /// Undo disabling
    pub fn restore(&mut self) {
        if self.disabled {
            self.disabled = false;
            self.set_opacity(1.0);
            self.set_visible(true);
        }
    }

    pub fn free(&self) {
        self.tri_layer.removeFromSuperlayer();
        self.text_layer.removeFromSuperlayer();
        self.box_layer.removeFromSuperlayer();
        if let Some(fl) = self.frame_layer.as_ref() {
            fl.removeFromSuperlayer();
        }
    }
}

/// Build hint boxes for `len` frames, labelled and collision-resolved.
pub fn hint_boxes_from_frames(
    len: usize,
    frames: impl Iterator<Item = Frame>,
    screen_frame: &Frame,
    theme: &GlyphlowTheme,
    hint_keys: &HintKeys,
    colored_frame_min_size: f64,
) -> (u32, Vec<HintBox>) {
    if len == 0 {
        return (0, Vec::new());
    }
    let digits = hint_keys.digits_for_len(len);
    let color_num = theme.frame_colors.len();
    let mut color_idx = 0;

    let mut boxes = frames
        .enumerate()
        .map(|(idx, frame)| {
            let frame = frame.intersect(screen_frame).unwrap_or(*screen_frame);

            let (x, y) = frame.center();
            let (w, h) = frame.size();

            // Draw frames for large enough elements
            let is_large = w.max(h) >= colored_frame_min_size;
            if is_large {
                color_idx += 1;
            };

            let color = (color_num > 0 && is_large)
                .then(|| theme.frame_colors.get(color_idx % color_num).cloned())
                .flatten();

            HintBox::new(
                idx,
                hint_keys.label_for_index(idx, Some(digits)),
                x,
                y,
                frame,
                color,
            )
        })
        .collect::<Vec<_>>();

    resolve_collisions(&mut boxes, digits, theme);

    (digits, boxes)
}

pub const MAX_COLLISION_OPS: usize = 150;

/// Nudge overlapping boxes apart.
///
/// The thresholds are estimated from the font rather than measured, because this
/// also runs before the boxes are laid out.
pub fn resolve_collisions(boxes: &mut [HintBox], digits: u32, theme: &GlyphlowTheme) {
    // Estimate box size
    let x_thres =
        theme.hint_font.pointSize() * digits as f64 * 0.8 + 2.0 * theme.hint_margin_size as f64;
    let y_thres = theme.hint_font.pointSize() * 1.5 + 2.0 * theme.hint_margin_size as f64;
    resolve_collisions_reactive(boxes, x_thres, y_thres, MAX_COLLISION_OPS);
}

/// The reactive part: a spatial grid to find neighbours cheaply, and a work
/// queue so a box pushed into a new collision is checked again.
fn resolve_collisions_reactive(boxes: &mut [HintBox], x_thres: f64, y_thres: f64, max_ops: usize) {
    if boxes.is_empty() {
        return;
    }

    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::with_capacity(boxes.len());
    let mut cell_coords = vec![(0, 0); boxes.len()];
    let mut queue = VecDeque::with_capacity(boxes.len());
    let mut in_queue = vec![true; boxes.len()];

    // Initial setup
    for (i, hb) in boxes.iter().enumerate() {
        if hb.disabled {
            continue;
        }
        let coords = (
            (hb.x / x_thres).floor() as i32,
            (hb.y / y_thres).floor() as i32,
        );
        cell_coords[i] = coords;
        grid.entry(coords).or_default().push(i);
        queue.push_back(i);
    }

    let mut ops_count = 0;

    while let Some(i) = queue.pop_front() {
        in_queue[i] = false;
        ops_count += 1;
        // Initial checking for each element doesn't count
        if ops_count > max_ops + boxes.len() {
            break;
        }

        let (cx, cy) = cell_coords[i];

        // Check 9 neighboring cells
        'outer: for dx in -1..=1 {
            for dy in -1..=1 {
                let target_cell = (cx + dx, cy + dy);

                let Some(neighbors) = grid.get(&target_cell) else {
                    continue;
                };
                for &j in neighbors {
                    if i == j {
                        continue;
                    }

                    let diff_x = boxes[i].x - boxes[j].x;
                    let diff_y = boxes[i].y - boxes[j].y;
                    let abs_dx = diff_x.abs();
                    let abs_dy = diff_y.abs();

                    if abs_dx < x_thres && abs_dy < y_thres {
                        // Collision found! Resolve it.
                        let (shift_x, shift_y) = (x_thres - abs_dx, y_thres - abs_dy);

                        // Move in a less crowded direction
                        let x_m_count = grid.get(&(cx - 1, cy)).map(|v| v.len()).unwrap_or(0);
                        let x_p_count = grid.get(&(cx + 1, cy)).map(|v| v.len()).unwrap_or(0);
                        let y_m_count = grid.get(&(cx, cy - 1)).map(|v| v.len()).unwrap_or(0);
                        let y_p_count = grid.get(&(cx, cy + 1)).map(|v| v.len()).unwrap_or(0);

                        if x_m_count + x_p_count <= y_m_count + y_p_count {
                            let move_dist =
                                (shift_x / 2.0) * (if diff_x >= 0.0 { 1.0 } else { -1.0 });
                            boxes[i].x += move_dist;
                            boxes[i].delta.0 += move_dist;
                            boxes[j].x -= move_dist;
                            boxes[j].delta.0 -= move_dist;
                        } else {
                            let move_dist =
                                (shift_y / 2.0) * (if diff_y >= 0.0 { 1.0 } else { -1.0 });
                            boxes[i].y += move_dist;
                            boxes[i].delta.1 += move_dist;
                            boxes[j].y -= move_dist;
                            boxes[j].delta.1 -= move_dist;
                        }

                        // Update grid positions and mark both as dirty
                        update_and_requeue(
                            i,
                            boxes,
                            &mut cell_coords,
                            &mut grid,
                            &mut queue,
                            &mut in_queue,
                            (x_thres, y_thres),
                        );
                        update_and_requeue(
                            j,
                            boxes,
                            &mut cell_coords,
                            &mut grid,
                            &mut queue,
                            &mut in_queue,
                            (x_thres, y_thres),
                        );

                        // After moving i, we should re-fetch its new neighbors
                        // breaking here allows the next loop to handle i's new position
                        break 'outer;
                    }
                }
            }
        }
    }
}

/// Move `idx` to its new grid cell and queue it for another collision check.
fn update_and_requeue(
    idx: usize,
    boxes: &[HintBox],
    cell_coords: &mut [(i32, i32)],
    grid: &mut HashMap<(i32, i32), Vec<usize>>,
    queue: &mut VecDeque<usize>,
    in_queue: &mut [bool],
    thres: (f64, f64),
) {
    let old_c = cell_coords[idx];
    let (xt, yt) = thres;
    let new_c = (
        (boxes[idx].x / xt).floor() as i32,
        (boxes[idx].y / yt).floor() as i32,
    );

    if old_c != new_c {
        if let Some(list) = grid.get_mut(&old_c)
            && let Some(pos) = list.iter().position(|&x| x == idx)
        {
            list.swap_remove(pos);
        }
        grid.entry(new_c).or_default().push(idx);
        cell_coords[idx] = new_c;
    }

    if !in_queue[idx] {
        queue.push_back(idx);
        in_queue[idx] = true;
    }
}

/// Colour the already-typed prefix differently from the rest, and set the font.
///
/// The ranges are byte offsets, which is only sound because hint labels are
/// ASCII.
fn update_hint_text_with_attr(
    attr_string: &Retained<NSMutableAttributedString>,
    label: &str,
    key_prefix_len: usize,
    theme: &GlyphlowTheme,
) {
    let hl_color = &theme.hint_hl_color;
    let fg_color = &theme.hint_fg_color;
    let font = &theme.hint_font;

    unsafe {
        attr_string.addAttribute_value_range(
            objc2_app_kit::NSForegroundColorAttributeName,
            hl_color.as_ref(),
            NSRange::new(0, key_prefix_len),
        );
        attr_string.addAttribute_value_range(
            objc2_app_kit::NSForegroundColorAttributeName,
            fg_color.as_ref(),
            NSRange::new(key_prefix_len, label.len() - key_prefix_len),
        );
        attr_string.addAttribute_value_range(
            objc2_app_kit::NSFontAttributeName,
            font,
            NSRange::new(0, label.len()),
        );
    }
}

/// A wrapper to link a Frame reference to its original index in the slice
struct IndexedFrame {
    idx: usize,
    frame: Frame,
}

impl RTreeObject for IndexedFrame {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(
            [self.frame.top_left.x, self.frame.top_left.y],
            [self.frame.bottom_right.x, self.frame.bottom_right.y],
        )
    }
}

fn find_overlaps_helper(boxes: Vec<IndexedFrame>, min_size: f64) -> Vec<(usize, usize, Frame)> {
    let mut overlaps = Vec::new();
    if boxes.is_empty() {
        return overlaps;
    }

    // Bulk load the R*-Tree. This runs in O(N log N) and builds
    // a structurally optimized tree with excellent cache locality.
    let tree = RTree::bulk_load(boxes);

    // Query the tree for intersections
    for item in tree.iter() {
        let query = tree.locate_in_envelope_intersecting(item.envelope());

        for other in query {
            // Deduplication
            if item.idx < other.idx
                && let Some(inter_frame) = item.frame.intersect(&other.frame)
                // Only count a real overlap: a sliver of contact thinner than
                // `min_size` on either axis does not count.
                && {
                    let (w, h) = inter_frame.size();
                    w >= min_size && h >= min_size
                }
            {
                overlaps.push((item.idx, other.idx, inter_frame));
            }
        }
    }

    overlaps
}

/// Pairs of hint boxes whose frames overlap by at least `min_size` on both axes,
/// with the overlapping frame.
pub fn find_overlaps(hint_boxes: &[HintBox], min_size: f64) -> Vec<(usize, usize, Frame)> {
    let frames = hint_boxes
        .iter()
        .enumerate()
        .map(|(idx, b)| IndexedFrame {
            idx,
            frame: b.frame,
        })
        .collect();
    find_overlaps_helper(frames, min_size)
}

#[cfg(test)]
mod collision_tests {
    use super::*;
    use rstest::rstest;

    fn mock_box(idx: usize, x: f64, y: f64) -> HintBox {
        HintBox::new(
            idx,
            format!("Box{}", idx),
            x,
            y,
            Frame::new(0.0, 0.0, 0.0, 0.0),
            None,
        )
    }

    /// The invariant the resolver exists to establish: every pair ends up
    /// separated by at least the threshold on *some* axis.
    fn assert_no_pair_collides(boxes: &[HintBox], x_thres: f64, y_thres: f64) {
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                let (dx, dy) = ((a.x - b.x).abs(), (a.y - b.y).abs());
                assert!(
                    dx >= x_thres || dy >= y_thres,
                    "boxes {} and {} still collide: dx = {dx}, dy = {dy}",
                    a.idx,
                    b.idx
                );
            }
        }
    }

    /// Every pair must be separated. The cases are the arrangements that matter;
    /// in `straddling_a_grid_cell` the cell boundary must not hide the overlap
    /// from the grid lookup.
    #[rstest]
    #[case::simple_pair(vec![(0, 100.0, 100.0), (1, 105.0, 100.0)], 100)]
    #[case::chain_reaction(vec![(0, 100.0, 100.0), (1, 108.0, 100.0), (2, 116.0, 100.0)], 500)]
    #[case::straddling_a_grid_cell(vec![(0, 9.9, 10.0), (1, 10.1, 10.0)], 100)]
    fn separates_every_overlapping_pair(
        #[case] spec: Vec<(usize, f64, f64)>,
        #[case] max_ops: usize,
    ) {
        let mut boxes: Vec<HintBox> = spec
            .into_iter()
            .map(|(idx, x, y)| mock_box(idx, x, y))
            .collect();

        resolve_collisions_reactive(&mut boxes, 10.0, 10.0, max_ops);

        assert_no_pair_collides(&boxes, 10.0, 10.0);
    }

    /// Two boxes 5px apart are separated along whichever axis needs the smaller
    /// nudge: X needs 5px and Y would need 10px, so Y must be left alone.
    #[test]
    fn separates_along_the_axis_needing_the_smaller_nudge() {
        let mut boxes = vec![mock_box(0, 100.0, 100.0), mock_box(1, 105.0, 100.0)];

        resolve_collisions_reactive(&mut boxes, 10.0, 10.0, 100);

        assert_eq!(boxes[0].y, 100.0, "Y must not move when X is cheaper");
        assert_no_pair_collides(&boxes, 10.0, 10.0);
    }

    #[test]
    fn test_no_collision_stays_put() {
        let mut boxes = vec![mock_box(0, 100.0, 100.0), mock_box(1, 200.0, 200.0)];

        let original_x = boxes[0].x;
        resolve_collisions_reactive(&mut boxes, 10.0, 10.0, 100);

        assert_eq!(
            boxes[0].x, original_x,
            "Box should not move if no collision exists"
        );
    }

    /// Ten boxes stacked on one point can never all be separated, so `max_ops`
    /// has to cut the loop short instead of spinning forever.
    #[test]
    fn max_ops_budget_terminates_deterministically() {
        let stacked = || {
            (0..10)
                .map(|i| mock_box(i, 100.0, 100.0))
                .collect::<Vec<_>>()
        };

        let mut first = stacked();
        resolve_collisions_reactive(&mut first, 10.0, 10.0, 50);

        let mut second = stacked();
        resolve_collisions_reactive(&mut second, 10.0, 10.0, 50);

        assert_eq!(first.len(), 10, "boxes must not be dropped");
        for (a, b) in first.iter().zip(&second) {
            assert_eq!((a.x, a.y), (b.x, b.y), "resolution is not deterministic");
            assert!(
                a.x.is_finite() && a.y.is_finite(),
                "box {} ended up at a non-finite position",
                a.idx
            );
        }
    }
}

#[cfg(test)]
mod hint_label_tests {
    use super::*;
    use crate::config::HintKeys;
    use rstest::rstest;

    fn screen() -> Frame {
        Frame::new(0.0, 0.0, 1000.0, 1000.0)
    }

    /// Well separated 20x20 frames, so collision resolution never kicks in.
    fn frames(len: usize) -> impl Iterator<Item = Frame> {
        (0..len).map(|i| {
            let x = (i % 10) as f64 * 40.0;
            let y = (i / 10) as f64 * 40.0;
            Frame::new(x, y, x + 20.0, y + 20.0)
        })
    }

    #[test]
    fn test_hint_boxes_use_configured_keys() {
        let (digits, boxes) = hint_boxes_from_frames(
            4,
            frames(4),
            &screen(),
            &GlyphlowTheme::default(),
            &HintKeys::new("asdfjkl;"),
            200.0,
        );

        assert_eq!(digits, 1);
        assert_eq!(
            boxes.iter().map(|b| b.label.as_str()).collect::<Vec<_>>(),
            vec!["A", "S", "D", "F"]
        );
    }

    #[test]
    fn test_hint_boxes_widen_with_configured_base() {
        let theme = GlyphlowTheme::default();

        // The full alphabet addresses 10 hints with a single keystroke ...
        let (digits, boxes) = hint_boxes_from_frames(
            10,
            frames(10),
            &screen(),
            &theme,
            &HintKeys::default(),
            200.0,
        );
        assert_eq!(digits, 1);
        assert_eq!(boxes[9].label, "J");

        // ... whereas a home-row alphabet needs two.
        let (digits, boxes) = hint_boxes_from_frames(
            10,
            frames(10),
            &screen(),
            &theme,
            &HintKeys::new("asdfjkl;"),
            200.0,
        );
        assert_eq!(digits, 2);
        assert_eq!(boxes[8].label, "AS");
        assert_eq!(boxes[9].label, "SS");
    }

    /// Widening `C` -> `CA` re-lays the box out, and the settle pass that follows
    /// must leave it exactly where the relabel put it. Pins the half of the
    /// animation-restart contract that lives in `HintBox`.
    #[rstest]
    #[case::undisturbed(0.0, 0.0)]
    #[case::shifted_by_collision_resolution(5.0, -3.0)]
    fn relabelling_settles_the_box_where_the_settle_pass_expects_it(
        #[case] dx: f64,
        #[case] dy: f64,
    ) {
        let theme = GlyphlowTheme::default();
        let screen = screen();
        let mut hb = HintBox::new(
            0,
            "C".to_string(),
            300.0,
            300.0,
            Frame::new(280.0, 280.0, 320.0, 320.0),
            None,
        );

        hb.set_label("C".to_string(), &screen, &theme);
        let narrow = hb.box_layer.frame();

        // Collision resolution has already run, so the box's final position is in
        // `x` / `y` / `delta` before the label widens.
        hb.x += dx;
        hb.y += dy;
        hb.delta = (dx, dy);

        hb.set_label("CA".to_string(), &screen, &theme);
        let settled = hb.box_layer.frame();

        // What `finalize_hints` -> `refresh` -> `update_position` does afterwards.
        hb.update_position(&screen, &theme);

        assert!(
            settled.size.width > narrow.size.width,
            "a two-character label must widen the box"
        );
        assert_eq!(
            hb.box_layer.frame(),
            settled,
            "the settle pass moved a box the relabel had already placed"
        );
    }

    #[test]
    fn test_hint_boxes_empty_input() {
        let (digits, boxes) = hint_boxes_from_frames(
            0,
            frames(0),
            &screen(),
            &GlyphlowTheme::default(),
            &HintKeys::default(),
            200.0,
        );
        assert_eq!(digits, 0);
        assert!(boxes.is_empty());
    }
}

#[cfg(test)]
mod find_overlaps_tests {
    use super::*;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    fn assert_frame_eq(
        actual: &Frame,
        expected_x1: f64,
        expected_y1: f64,
        expected_x2: f64,
        expected_y2: f64,
    ) {
        assert!(
            approx_eq(actual.top_left.x, expected_x1),
            "Expected x1: {}, got: {}",
            expected_x1,
            actual.top_left.x
        );
        assert!(
            approx_eq(actual.top_left.y, expected_y1),
            "Expected y1: {}, got: {}",
            expected_y1,
            actual.top_left.y
        );
        assert!(
            approx_eq(actual.bottom_right.x, expected_x2),
            "Expected x2: {}, got: {}",
            expected_x2,
            actual.bottom_right.x
        );
        assert!(
            approx_eq(actual.bottom_right.y, expected_y2),
            "Expected y2: {}, got: {}",
            expected_y2,
            actual.bottom_right.y
        );
    }

    fn to_indexed_frames(frames: &[Frame]) -> Vec<IndexedFrame> {
        frames
            .iter()
            .enumerate()
            .map(|(i, f)| IndexedFrame { idx: i, frame: *f })
            .collect()
    }

    #[test]
    fn test_zero_area_intersections() {
        let boxes = vec![
            Frame::new(0.0, 0.0, 10.0, 10.0),   // Box 0
            Frame::new(10.0, 0.0, 20.0, 10.0), // Box 1: Shares a vertical edge segment with Box 0 at X = 10.0
            Frame::new(10.0, 10.0, 20.0, 20.0), // Box 2: Shares exactly one corner vertex with Box 0 at (10.0, 10.0)
        ];
        let boxes = to_indexed_frames(&boxes);

        let mut results = find_overlaps_helper(boxes, 0.0);
        results.sort_by_key(|(u, v, _)| (*u, *v));

        // Expected overlaps:
        // - Box 0 & Box 1 (Line intersection at X=10 from Y=0 to 10)
        // - Box 0 & Box 2 (Point intersection at X=10, Y=10)
        // - Box 1 & Box 2 (Line intersection at Y=10 from X=10 to 20)
        assert_eq!(results.len(), 3);

        // 0 & 1
        assert_eq!(results[0].0, 0);
        assert_eq!(results[0].1, 1);
        assert_frame_eq(&results[0].2, 10.0, 0.0, 10.0, 10.0);

        // 0 & 2
        assert_eq!(results[1].0, 0);
        assert_eq!(results[1].1, 2);
        assert_frame_eq(&results[1].2, 10.0, 10.0, 10.0, 10.0);

        // 1 & 2
        assert_eq!(results[2].0, 1);
        assert_eq!(results[2].1, 2);
        assert_frame_eq(&results[2].2, 10.0, 10.0, 20.0, 10.0);
    }

    #[test]
    fn test_concentric_nested_boxes() {
        let boxes = vec![
            Frame::new(0.0, 0.0, 100.0, 100.0), // Box 0: Outer
            Frame::new(20.0, 20.0, 80.0, 80.0), // Box 1: Middle
            Frame::new(40.0, 40.0, 60.0, 60.0), // Box 2: Inner
        ];
        let boxes = to_indexed_frames(&boxes);

        let mut results = find_overlaps_helper(boxes, 0.0);
        results.sort_by_key(|(u, v, _)| (*u, *v));

        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_thin_cross_intersection() {
        let boxes = vec![
            Frame::new(0.0, 45.0, 100.0, 55.0), // Box 0: Ultra-wide horizontal strip
            Frame::new(45.0, 0.0, 55.0, 100.0), // Box 1: Ultra-tall vertical strip
        ];
        let boxes = to_indexed_frames(&boxes);

        let results = find_overlaps_helper(boxes, 0.0);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 0);
        assert_eq!(results[0].1, 1);
        // The intersection must be only the shared center square
        assert_frame_eq(&results[0].2, 45.0, 45.0, 55.0, 55.0);
    }

    #[test]
    fn test_floating_point_epsilon_near_miss() {
        let epsilon = 1e-11;
        let boxes = vec![
            Frame::new(0.0, 0.0, 10.0, 10.0),
            // Starts exactly an epsilon past the right boundary of Box 0
            Frame::new(10.0 + epsilon, 0.0, 20.0, 10.0),
        ];
        let boxes = to_indexed_frames(&boxes);

        let results = find_overlaps_helper(boxes, 0.0);

        // Must be completely empty since they do not touch
        assert!(
            results.is_empty(),
            "Found false positive intersection due to floating point drift."
        );
    }
}
