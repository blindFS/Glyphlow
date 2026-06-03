use std::collections::{HashMap, VecDeque};

use objc2::{AnyThread, rc::Retained};
use objc2_core_foundation::{CFRetained, CGSize};
use objc2_core_graphics::{CGColor, CGMutablePath};
use objc2_foundation::{NSMutableAttributedString, NSPoint, NSRange, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CAShapeLayer, CATextLayer, kCAAlignmentCenter};

use crate::config::GlyphlowTheme;
use crate::user_interface::calibrated_origin;
use crate::util::{Frame, digits_by_length, estimate_frame_for_text};

pub fn hint_label_from_index(i: usize, digits: Option<u32>) -> String {
    if i == 0 && digits.is_none() {
        return "A".to_string();
    }

    let mut n = i;
    let mut result = Vec::new();

    while n > 0 {
        let remainder = (n % 26) as u8;
        let char = (b'A' + remainder) as char;
        result.push(char);
        n /= 26;
    }

    let Some(digits) = digits else {
        return result.iter().collect();
    };
    // pad to fixed length
    while result.len() < digits as usize {
        result.push('A');
    }
    result.iter().collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct HintBox {
    pub label: String,
    x: f64,
    y: f64,
    pub idx: usize,
    /// Moved distance to avoid collision
    delta: (f64, f64),
    frame: Option<Frame>,
    color: Option<CFRetained<CGColor>>,
    text_layer: Retained<CATextLayer>,
    box_layer: Retained<CALayer>,
    tri_layer: Retained<CAShapeLayer>,
    frame_layer: Option<Retained<CALayer>>,
}

impl HintBox {
    pub fn new(
        idx: usize,
        label: String,
        x: f64,
        y: f64,
        frame: Option<Frame>,
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

        let frame_layer = frame.map(|_| {
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
            delta: (0.0, 0.0),
            frame,
            color,
            text_layer: tl,
            box_layer: bl,
            tri_layer,
            frame_layer,
        }
    }

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

    fn calculate_origin(
        &self,
        box_size: CGSize,
        screen_frame: &Frame,
        tri_height: f64,
    ) -> (NSPoint, f64, f64) {
        let (box_width, box_height) = (box_size.width, box_size.height);
        let (o_x, o_y) = (self.x - box_width / 2.0, self.y + tri_height + box_height);
        let o_x_move = o_x
            .min(screen_frame.bottom_right.x - box_width)
            .max(screen_frame.top_left.x);
        let o_y_move = o_y
            .max(screen_frame.top_left.y)
            .min(screen_frame.bottom_right.y);
        (
            calibrated_origin(o_x_move, o_y_move, screen_frame),
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
                tri_height + self.delta.1 + y_offset,
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
        screen_frame: &Frame,
    ) {
        let (tri_width, tri_height) = Self::geometry(theme);
        let bg_color = self.color.as_ref().unwrap_or(&theme.hint_bg_color);

        // Frame Layer
        if let Some(fl) = &self.frame_layer
            && let Some(frame) = &self.frame
        {
            let x = frame.top_left.x;
            let y = frame.bottom_right.y;
            let (w, h) = frame.size();
            fl.setFrame(NSRect::new(
                calibrated_origin(x, y, screen_frame),
                NSSize::new(w, h),
            ));
            fl.setBorderColor(Some(bg_color));
            if fl.superlayer().is_none() {
                root_layer.addSublayer(fl);
            }
        }

        // Text & Box Layer
        let attr_string = self.attributed_string(key_prefix_len, theme);
        let (text_size, _) = estimate_frame_for_text(&attr_string, screen_frame.size());
        let margin = theme.hint_margin_size as f64;
        let box_size = CGSize::new(
            text_size.width + (margin * 2.0),
            text_size.height + (margin * 2.0),
        );

        let (origin, x_offset, y_offset) =
            self.calculate_origin(box_size, screen_frame, tri_height);

        self.box_layer.setFrame(NSRect::new(origin, box_size));
        self.box_layer.setBackgroundColor(Some(bg_color));
        self.box_layer.setCornerRadius(margin);

        self.text_layer
            .setFrame(NSRect::new(NSPoint::new(margin, margin), text_size));
        unsafe {
            self.text_layer.setString(Some(&attr_string));
        }

        // Triangle Layer
        let path = self.create_triangle_path(tri_width, tri_height, x_offset, y_offset);
        self.tri_layer.setPath(Some(&path));
        self.tri_layer.setFillColor(Some(bg_color));
        self.tri_layer.setFrame(NSRect::new(
            NSPoint::new((box_size.width - tri_width) / 2.0, box_size.height),
            NSSize::new(tri_width, tri_height),
        ));

        if self.box_layer.superlayer().is_none() {
            root_layer.addSublayer(&self.box_layer);
        }
    }

    /// Updates the text and re-estimates the size, returns true if the size changed
    fn update_text(&self, prefix_len: usize, theme: &GlyphlowTheme) -> bool {
        let attr_string = self.attributed_string(prefix_len, theme);
        unsafe {
            self.text_layer.setString(Some(&attr_string));
        }

        // Re-estimate size if needed
        let (text_size, _) = estimate_frame_for_text(
            &attr_string,
            (f64::MAX, f64::MAX), // No constraints for label
        );

        let current_text_size = self.text_layer.frame().size;
        if text_size == current_text_size {
            return false;
        }

        let margin = theme.hint_margin_size as f64;
        let new_box_size = CGSize::new(
            text_size.width + (margin * 2.0),
            text_size.height + (margin * 2.0),
        );

        let mut box_frame = self.box_layer.frame();
        box_frame.size = new_box_size;
        self.box_layer.setFrame(box_frame);

        let mut text_frame = self.text_layer.frame();
        text_frame.size = text_size;
        self.text_layer.setFrame(text_frame);
        true
    }

    fn update_position(&self, has_resized: bool, screen_frame: &Frame, theme: &GlyphlowTheme) {
        if self.delta == (0.0, 0.0) && !has_resized {
            return;
        }
        let (tri_width, tri_height) = Self::geometry(theme);
        let box_size = self.box_layer.frame().size;
        let (origin, x_offset, y_offset) =
            self.calculate_origin(box_size, screen_frame, tri_height);

        self.box_layer.setFrame(NSRect::new(origin, box_size));

        // Update triangle
        let mut tri_frame = self.tri_layer.frame();
        tri_frame.origin.x = (box_size.width - tri_width) / 2.0;
        tri_frame.origin.y = box_size.height;
        self.tri_layer.setFrame(tri_frame);

        let path = self.create_triangle_path(tri_width, tri_height, x_offset, y_offset);
        self.tri_layer.setPath(Some(&path));
    }

    /// Update text, then refresh
    pub fn refresh(&self, prefix_len: usize, screen_frame: &Frame, theme: &GlyphlowTheme) {
        let has_resized = self.update_text(prefix_len, theme);
        self.update_position(has_resized, screen_frame, theme);
    }

    pub fn set_visible(&self, visible: bool) {
        self.box_layer.setHidden(!visible);
        if let Some(fl) = &self.frame_layer {
            fl.setHidden(!visible);
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

pub fn hint_boxes_from_frames(
    len: usize,
    frames: impl Iterator<Item = Frame>,
    screen_frame: &Frame,
    theme: &GlyphlowTheme,
    colored_frame_min_size: f64,
) -> (u32, Vec<HintBox>) {
    if len == 0 {
        return (0, Vec::new());
    }
    let digits = digits_by_length(len);
    let color_num = theme.frame_colors.len();
    let mut color_idx = 0;

    let mut boxes = frames
        .enumerate()
        .map(|(idx, frame)| {
            let frame = frame.intersect(screen_frame).unwrap_or(*screen_frame);

            let (x, y) = frame.center();
            let (w, h) = frame.size();

            // Draw frames for large enough elements
            let frame = if w.max(h) >= colored_frame_min_size {
                color_idx += 1;
                Some(frame)
            } else {
                None
            };
            let color = (color_num > 0)
                .then(|| {
                    frame
                        .as_ref()
                        .and_then(|_| theme.frame_colors.get(color_idx % color_num).cloned())
                })
                .flatten();

            HintBox::new(
                idx,
                hint_label_from_index(idx, Some(digits)),
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

pub fn resolve_collisions(boxes: &mut [HintBox], digits: u32, theme: &GlyphlowTheme) {
    // Estimate box size
    let x_thres =
        theme.hint_font.pointSize() * digits as f64 * 0.8 + 2.0 * theme.hint_margin_size as f64;
    let y_thres = theme.hint_font.pointSize() * 1.5 + 2.0 * theme.hint_margin_size as f64;
    resolve_collisions_reactive(boxes, x_thres, y_thres, MAX_COLLISION_OPS);
}

fn resolve_collisions_reactive(boxes: &mut [HintBox], x_thres: f64, y_thres: f64, max_ops: usize) {
    if boxes.is_empty() {
        return;
    }

    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::with_capacity(boxes.len());
    let mut cell_coords = vec![(0, 0); boxes.len()];
    let mut queue = VecDeque::with_capacity(boxes.len());
    let mut in_queue = vec![true; boxes.len()];

    // Initial setup
    for i in 0..boxes.len() {
        let coords = (
            (boxes[i].x / x_thres).floor() as i32,
            (boxes[i].y / y_thres).floor() as i32,
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

#[cfg(test)]
mod collision_tests {
    use super::*;

    fn mock_box(idx: usize, x: f64, y: f64) -> HintBox {
        HintBox::new(idx, format!("Box{}", idx), x, y, None, None)
    }

    #[test]
    fn test_simple_collision_resolution() {
        let mut boxes = vec![
            mock_box(0, 100.0, 100.0),
            mock_box(1, 105.0, 100.0), // 5px diff, threshold is 10
        ];

        resolve_collisions_reactive(&mut boxes, 10.0, 10.0, 100);

        let diff_x = (boxes[0].x - boxes[1].x).abs();
        assert!(
            diff_x >= 10.0,
            "Boxes should be at least 10px apart, got {}",
            diff_x
        );
        assert_eq!(
            boxes[0].y, 100.0,
            "Y coordinate shouldn't change if X move was smaller"
        );
    }

    #[test]
    fn test_chain_reaction() {
        // A overlaps B, B overlaps C.
        // Solving A-B should push B into C, which then needs solving.
        let mut boxes = vec![
            mock_box(0, 100.0, 100.0),
            mock_box(1, 108.0, 100.0), // Overlaps 0 by 2px
            mock_box(2, 116.0, 100.0), // Overlaps 1 by 2px
        ];

        resolve_collisions_reactive(&mut boxes, 10.0, 10.0, 500);

        // Verify all pairs
        for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                let dx = (boxes[i].x - boxes[j].x).abs();
                let dy = (boxes[i].y - boxes[j].y).abs();
                assert!(
                    dx >= 10.0 || dy >= 10.0,
                    "Collision found between {} and {}",
                    i,
                    j
                );
            }
        }
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

    #[test]
    fn test_spatial_grid_boundary() {
        // Place boxes on either side of a grid cell boundary
        // If x_thres is 10, cell boundary is at multiples of 10.
        let mut boxes = vec![
            mock_box(0, 9.9, 10.0),  // Cell (0, 1)
            mock_box(1, 10.1, 10.0), // Cell (1, 1)
        ];

        resolve_collisions_reactive(&mut boxes, 10.0, 10.0, 100);

        let diff_x = (boxes[0].x - boxes[1].x).abs();
        let diff_y = (boxes[0].y - boxes[1].y).abs();
        assert!(
            diff_x >= 10.0 || diff_y >= 10.0,
            "Should resolve collisions even across grid boundaries"
        );
    }

    #[test]
    fn test_max_ops_safety() {
        // Create a "Black Hole" of points that cannot be perfectly resolved
        // to ensure we don't loop forever.
        let mut boxes = (0..10)
            .map(|i| mock_box(i, 100.0, 100.0))
            .collect::<Vec<_>>();

        // This should hit the max_ops and exit gracefully
        resolve_collisions_reactive(&mut boxes, 10.0, 10.0, 50);
    }
}
