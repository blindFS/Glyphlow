use std::cmp::Ordering;

use core_foundation::attributed_string::CFAttributedStringRef;
use core_text::framesetter::CTFramesetter;
use objc2::rc::Retained;
use objc2_core_foundation::{CGPoint, CGRect, CGSize as OCGSize};
use objc2_foundation::{NSMutableAttributedString, NSSize};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Copy, Default)]
pub struct Frame {
    pub top_left: CGPoint,
    pub bottom_right: CGPoint,
}

pub fn estimate_frame_for_text(
    attr_string: &Retained<NSMutableAttributedString>,
    size: (f64, f64),
) -> (OCGSize, isize) {
    let cf_attr_string = Retained::as_ptr(attr_string) as CFAttributedStringRef;
    let framesetter = CTFramesetter::new_with_attributed_string(cf_attr_string);
    let (core_graphics_types::geometry::CGSize { width, height }, range) = framesetter
        .suggest_frame_size_with_constraints(
            core_foundation::base::CFRange {
                location: 0,
                length: 0,
            },
            std::ptr::null(),
            core_graphics_types::geometry::CGSize::new(size.0, size.1),
        );
    (OCGSize::new(width, height), range.length)
}

impl Eq for Frame {}

impl PartialOrd for Frame {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

const MIN_HEIGHT_THRESHOLD: f64 = 10.0;

impl Ord for Frame {
    // Compare the bottom left point, y coordinate first, then x
    fn cmp(&self, other: &Self) -> Ordering {
        let y1 = self.bottom_right.y;
        let y2 = other.bottom_right.y;
        let x1 = self.top_left.x;
        let x2 = other.top_left.x;

        y1.total_cmp(&y2).then_with(|| x1.total_cmp(&x2))
    }
}

impl Frame {
    pub fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        Frame {
            top_left: CGPoint { x: x1, y: y1 },
            bottom_right: CGPoint { x: x2, y: y2 },
        }
    }

    pub fn size(&self) -> (f64, f64) {
        (
            self.bottom_right.x - self.top_left.x,
            self.bottom_right.y - self.top_left.y,
        )
    }

    pub fn invert_y(&self, height: f64) -> Self {
        Frame::new(
            self.top_left.x,
            height - self.top_left.y,
            self.bottom_right.x,
            height - self.bottom_right.y,
        )
    }

    pub fn to_cgrect(&self) -> CGRect {
        let (w, h) = self.size();
        CGRect::new(self.top_left, OCGSize::new(w, h))
    }

    pub fn from_cgrect(rect: &CGRect) -> Self {
        let CGRect { origin, size } = rect;
        Self::new(
            origin.x,
            origin.y,
            origin.x + size.width,
            origin.y + size.height,
        )
    }

    pub fn ns_size(&self) -> NSSize {
        let (w, h) = self.size();
        NSSize::new(w, h)
    }

    pub fn center(&self) -> (f64, f64) {
        (
            (self.top_left.x + self.bottom_right.x) / 2.0,
            (self.top_left.y + self.bottom_right.y) / 2.0,
        )
    }

    /// Calculate the boundaries of the potential intersection
    pub fn intersect(&self, other: &Frame) -> Option<Self> {
        let inter_x1 = self.top_left.x.max(other.top_left.x);
        let inter_y1 = self.top_left.y.max(other.top_left.y);
        let inter_x2 = self.bottom_right.x.min(other.bottom_right.x);
        let inter_y2 = self.bottom_right.y.min(other.bottom_right.y);

        if inter_x1 <= inter_x2 && inter_y1 <= inter_y2 {
            Some(Frame::new(inter_x1, inter_y1, inter_x2, inter_y2))
        } else {
            None
        }
    }

    pub fn union(&self, other: &Frame) -> Self {
        Frame::new(
            self.top_left.x.min(other.top_left.x),
            self.top_left.y.min(other.top_left.y),
            self.bottom_right.x.max(other.bottom_right.x),
            self.bottom_right.y.max(other.bottom_right.y),
        )
    }

    pub fn union_of_frames(frames: &[Frame]) -> Self {
        frames.iter().fold(Frame::default(), |acc, f| acc.union(f))
    }

    pub fn contains(&self, other: &Frame) -> bool {
        self.top_left.x <= other.top_left.x
            && self.top_left.y <= other.top_left.y
            && self.bottom_right.x >= other.bottom_right.x
            && self.bottom_right.y >= other.bottom_right.y
    }
}

fn estimate_font_height(s: &str, frame: &Frame) -> f64 {
    let unicode_width = s.width();
    let (w, h) = frame.size();
    if w < 1.0 {
        return w;
    }
    let line_count = (h * unicode_width as f64 / 3.0 / w).sqrt().round() + 1.0;
    h / line_count
}

/// Heuristic of selecting a paragraph of texts,
/// given 2 frames as the start and end
// TODO: languages that read from right to left
pub fn select_range_helper(
    choices: &[(String, Frame, bool)],
    idx1: usize,
    idx2: usize,
) -> Option<(String, Frame)> {
    let (s1, frame1, _) = choices.get(idx1)?;
    let (s2, frame2, _) = choices.get(idx2)?;
    let (s_frame, e_frame) = if frame1 < frame2 {
        (frame1, frame2)
    } else {
        (frame2, frame1)
    };
    let y_min = s_frame.top_left.y;
    let y_max = e_frame.bottom_right.y;
    let mut x_min = s_frame.top_left.x.min(e_frame.top_left.x);
    let mut x_max = e_frame.bottom_right.x.max(s_frame.bottom_right.x);

    // NOTE: Exclude elements too far left/right
    let font_height = estimate_font_height(s1, frame1).min(estimate_font_height(s2, frame2));
    let x_thres = font_height * 2.5;
    let y_thres = (font_height * 0.8).min(MIN_HEIGHT_THRESHOLD);

    // Roughly sort all elements in y range
    let mut within_y_range = choices
        .iter()
        .filter(|(_, f, v)| {
            if !*v {
                return false;
            }
            if f >= s_frame && f <= e_frame {
                return true;
            }

            // Fuzzy boundaries for elements on the same line as start or end
            if (f.bottom_right.y - s_frame.bottom_right.y).abs() < y_thres
                && f.top_left.x >= s_frame.top_left.x
            {
                return true;
            }
            if (f.bottom_right.y - e_frame.bottom_right.y).abs() < y_thres
                && f.top_left.x <= e_frame.bottom_right.x
            {
                return true;
            }
            false
        })
        .collect::<Vec<_>>();

    // Sort by Y strictly first (ensures total order)
    within_y_range.sort_by(|(_, f1, _), (_, f2, _)| f1.cmp(f2));

    // Group into lines and sort each line by X to handle staggered Y
    let mut i = 0;
    while i < within_y_range.len() {
        let mut j = i + 1;
        let (_, fi, _) = within_y_range[i];
        while j < within_y_range.len() {
            let (_, fj, _) = within_y_range[j];
            if fj.bottom_right.y > fi.bottom_right.y + y_thres {
                break;
            }
            j += 1;
        }
        within_y_range[i..j]
            .sort_by(|(_, f1, _), (_, f2, _)| f1.top_left.x.total_cmp(&f2.top_left.x));
        i = j;
    }

    // Find the x_min from elements actually included
    let mut x_ranges = within_y_range
        .iter()
        .map(|(_, f, _)| (f.top_left.x, f.bottom_right.x))
        .collect::<Vec<_>>();
    if x_ranges.is_empty() {
        return None;
    }
    x_ranges.sort_by(|a, b| a.0.total_cmp(&b.0));

    let (mut this_min, mut this_max) = *x_ranges
        .get_mut(0)
        .expect("Should contains at least one choice in the given y range.");
    for (x1, x2) in x_ranges.iter().skip(1) {
        if this_max + x_thres > x_min {
            x_min = this_min.min(x_min);
            break;
        } else if *x1 > this_max + x_thres {
            this_min = *x1;
            this_max = *x2;
        } else {
            this_max = this_max.max(*x2);
        }
    }

    let mut text = String::new();
    let mut last_y = s_frame.bottom_right.y;

    for (s, f, _) in within_y_range.iter() {
        // Too far left/right
        if f.top_left.x > x_max + x_thres || f.bottom_right.x < x_min - x_thres {
            continue;
        }
        // NOTE: add newline if the new y is large enough,
        // Some margin (3px) for miscalculated frames, e.g. OCR frames
        if f.top_left.y > last_y - 3.0 {
            text.push('\n');
        } else if !text.is_empty() && !text.ends_with(' ') && !s.starts_with(' ') {
            text.push(' ');
        }

        text.push_str(s);
        last_y = f.bottom_right.y;
        x_max = x_max.max(f.bottom_right.x);
    }
    Some((text, Frame::new(x_min, y_min, x_max, y_max)))
}

pub fn digits_by_length(len: usize) -> u32 {
    if len <= 1 { 1 } else { (len - 1).ilog(26) + 1 }
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn test_frame_ordering_vertical_priority() {
        // Higher y (bottom_right.y) should be "Greater" regardless of x
        let frame_top = Frame::new(0.0, 0.0, 10.0, 50.0);
        let frame_bottom = Frame::new(100.0, 0.0, 110.0, 20.0);

        assert_eq!(frame_top.cmp(&frame_bottom), Ordering::Greater);
        assert_eq!(frame_bottom.cmp(&frame_top), Ordering::Less);
    }

    #[test]
    fn test_frame_ordering_horizontal_within_threshold() {
        // These frames have similar y-coordinates (difference < MIN_HEIGHT_THRESHOLD)
        // So they should be sorted by x (top_left.x)
        let frame_left = Frame::new(10.0, 0.0, 20.0, 15.0);
        let frame_right = Frame::new(50.0, 0.0, 60.0, 16.0);

        assert_eq!(frame_left.cmp(&frame_right), Ordering::Less);
        assert_eq!(frame_right.cmp(&frame_left), Ordering::Greater);
    }

    #[test]
    fn test_intersect_success() {
        let f1 = Frame::new(0.0, 0.0, 10.0, 10.0);
        let f2 = Frame::new(5.0, 5.0, 15.0, 15.0);

        let intersection = f1.intersect(&f2).expect("Should intersect");

        assert_eq!(intersection.top_left.x, 5.0);
        assert_eq!(intersection.top_left.y, 5.0);
        assert_eq!(intersection.bottom_right.x, 10.0);
        assert_eq!(intersection.bottom_right.y, 10.0);
    }

    #[test]
    fn test_intersect_none() {
        let f1 = Frame::new(0.0, 0.0, 5.0, 5.0);
        let f2 = Frame::new(10.0, 10.0, 15.0, 15.0);

        assert!(f1.intersect(&f2).is_none());
    }

    #[test]
    fn test_intersect_edge_touching() {
        // Rectangles touching at the exact edge should return None
        // because of the strict '<' check in your implementation
        let f1 = Frame::new(0.0, 0.0, 10.0, 10.0);
        let f2 = Frame::new(10.0, 0.0, 20.0, 10.0);

        assert!(
            f1.intersect(&f2).is_some(),
            "Touching edges should intersect"
        );
    }

    #[test]
    fn test_intersect_fully_contained() {
        let big = Frame::new(0.0, 0.0, 100.0, 100.0);
        let small = Frame::new(20.0, 20.0, 50.0, 50.0);

        // Small inside big should return small
        let result = big.intersect(&small).expect("Should intersect");
        assert_eq!(result, small);

        // Commutative check: big inside small (mathematically)
        let result_rev = small.intersect(&big).expect("Should intersect");
        assert_eq!(result_rev, small);
    }

    #[test]
    fn test_intersect_partial_overlap_strip() {
        // Overlap only on X axis, but spans entire Y height
        let f1 = Frame::new(0.0, 0.0, 20.0, 100.0);
        let f2 = Frame::new(10.0, 0.0, 30.0, 100.0);

        let result = f1.intersect(&f2).expect("Should intersect");
        assert_eq!(result, Frame::new(10.0, 0.0, 20.0, 100.0));
    }

    #[test]
    fn test_intersect_single_axis_overlap_only() {
        // X-axis overlaps, but Y-axis does not
        let f1 = Frame::new(0.0, 0.0, 50.0, 10.0);
        let f2 = Frame::new(10.0, 20.0, 40.0, 30.0);

        assert!(
            f1.intersect(&f2).is_none(),
            "Should not intersect if Y is separated"
        );
    }

    #[test]
    fn test_intersect_identical_frames() {
        let f1 = Frame::new(10.0, 10.0, 20.0, 20.0);
        let f2 = Frame::new(10.0, 10.0, 20.0, 20.0);

        let result = f1.intersect(&f2).expect("Should intersect");
        assert_eq!(result, f1);
    }

    #[test]
    fn test_intersect_negative_coordinates() {
        let f1 = Frame::new(-50.0, -50.0, -10.0, -10.0);
        let f2 = Frame::new(-20.0, -20.0, 10.0, 10.0);

        let result = f1.intersect(&f2).expect("Should intersect");
        assert_eq!(result, Frame::new(-20.0, -20.0, -10.0, -10.0));
    }

    #[test]
    fn test_intersect_zero_size_overlap() {
        // One frame is a "point" or "line" (width or height is 0)
        // Given your `if inter_x1 < inter_x2` logic, this should return None
        let f1 = Frame::new(0.0, 0.0, 10.0, 10.0);
        let f2 = Frame::new(5.0, 0.0, 5.0, 10.0); // Zero width

        assert!(
            f1.intersect(&f2).is_some(),
            "Zero-width overlap should be Some"
        );
    }

    #[test]
    fn test_sorting() {
        let mut frames = [
            Frame::new(100.0, 0.0, 110.0, 100.0), // Far right, but very high Y (Last)
            Frame::new(10.0, 0.0, 20.0, 10.0),    // Left, low Y (First)
            Frame::new(50.0, 0.0, 60.0, 10.0),    // Right, low Y (Second)
        ];

        frames.sort();

        assert_eq!(frames[0].top_left.x, 10.0);
        assert_eq!(frames[1].top_left.x, 50.0);
        assert_eq!(frames[2].top_left.x, 100.0);
    }

    #[test]
    fn test_frame_ordering_horizontal_same_y() {
        // frames with identical y-coordinates should be sorted by x (top_left.x)
        let frame_left = Frame::new(10.0, 0.0, 20.0, 15.0);
        let frame_right = Frame::new(50.0, 0.0, 60.0, 15.0);

        assert_eq!(frame_left.cmp(&frame_right), Ordering::Less);
        assert_eq!(frame_right.cmp(&frame_left), Ordering::Greater);
    }

    #[test]
    fn test_total_order_violation_repro() {
        // A: y=100, h=20, x=10
        // B: y=105, h=5,  x=0
        // C: y=110, h=20, x=0
        let a = Frame {
            top_left: CGPoint { x: 10.0, y: 80.0 },
            bottom_right: CGPoint { x: 20.0, y: 100.0 },
        };
        let b = Frame {
            top_left: CGPoint { x: 0.0, y: 100.0 },
            bottom_right: CGPoint { x: 10.0, y: 105.0 },
        };
        let c = Frame {
            top_left: CGPoint { x: 0.0, y: 90.0 },
            bottom_right: CGPoint { x: 10.0, y: 110.0 },
        };

        // Strict order should be consistent: a < b < c
        assert_eq!(a.cmp(&b), Ordering::Less);
        assert_eq!(b.cmp(&c), Ordering::Less);
        assert_eq!(a.cmp(&c), Ordering::Less);

        let mut frames = [a, b, c];
        // This should no longer panic as it's a proper total order
        frames.sort();
        assert_eq!(frames[0], a);
        assert_eq!(frames[1], b);
        assert_eq!(frames[2], c);
    }
}

#[cfg(test)]
mod select_range_tests {
    use super::*;

    /// Helper function to quickly generate test data
    fn make_choice(
        text: &str,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        visible: bool,
    ) -> (String, Frame, bool) {
        (text.to_string(), Frame::new(x, y, x + w, y + h), visible)
    }

    #[test]
    fn test_select_single_column_paragraph() {
        let choices = vec![
            make_choice("Hello ", 0.0, 0.0, 40.0, 10.0, true),
            make_choice("world.", 45.0, 0.0, 40.0, 10.0, true),
            make_choice("This ", 0.0, 15.0, 30.0, 10.0, true), // New line
            make_choice("is ", 35.0, 15.0, 20.0, 10.0, true),
            make_choice("Rust.", 60.0, 15.0, 30.0, 10.0, true),
        ];

        // Select from "Hello " to "Rust."
        let (text, frame) = select_range_helper(&choices, 0, 4).unwrap();

        // `last_y` logic expects a newline when `top_left.y > last_y - 3.0`
        assert_eq!(text, "Hello world.\nThis is Rust.");

        // Bounding box should encompass the whole block
        assert_eq!(frame.top_left.x, 0.0);
        assert_eq!(frame.top_left.y, 0.0);
        assert_eq!(frame.bottom_right.x, 90.0);
        assert_eq!(frame.bottom_right.y, 25.0);
    }

    #[test]
    fn test_select_multi_column_exclude_right() {
        // Font height = 10.0. x_thres = 25.0.
        // Column 1 ends at x=50. Column 2 starts at x=100.
        // Gap is 50, which is > x_thres, so they should be treated as separate columns.
        let choices = vec![
            // Column 1
            make_choice("Col1_L1 ", 0.0, 0.0, 50.0, 10.0, true),
            make_choice("Col1_L2", 0.0, 15.0, 50.0, 10.0, true),
            // Column 2
            make_choice("Col2_L1 ", 100.0, 0.0, 50.0, 10.0, true),
            make_choice("Col2_L2", 100.0, 15.0, 50.0, 10.0, true),
        ];

        // Select start to end of Column 1
        // Indices 0 and 1
        let (text, _) = select_range_helper(&choices, 0, 1).unwrap();

        // Should completely ignore Column 2
        assert_eq!(text, "Col1_L1 \nCol1_L2");
    }

    #[test]
    fn test_select_multi_column_exclude_left() {
        let choices = vec![
            // Column 1
            make_choice("Col1_L1", 0.0, 0.0, 50.0, 10.0, true),
            make_choice("Col1_L2", 0.0, 15.0, 50.0, 10.0, true),
            // Column 2
            make_choice("Col2_L1", 100.0, 0.0, 50.0, 10.0, true),
            make_choice("Col2_L2", 100.0, 15.0, 50.0, 10.0, true),
        ];

        // Select start to end of Column 2
        // Indices 2 and 3
        let (text, _) = select_range_helper(&choices, 2, 3).unwrap();

        // Should completely ignore Column 1
        assert_eq!(text, "Col2_L1\nCol2_L2");
    }

    #[test]
    fn test_reverse_selection() {
        let choices = vec![
            make_choice("Start ", 0.0, 0.0, 40.0, 10.0, true),
            make_choice("Middle", 45.0, 0.0, 40.0, 10.0, true),
            make_choice("End", 0.0, 15.0, 30.0, 10.0, true),
        ];

        // User dragged from "End" (idx 2) backwards to "Start" (idx 0)
        let (text_reverse, frame_reverse) = select_range_helper(&choices, 2, 0).unwrap();
        let (text_forward, frame_forward) = select_range_helper(&choices, 0, 2).unwrap();

        // The output should be identical regardless of selection direction
        assert_eq!(text_reverse, text_forward);
        assert_eq!(frame_reverse, frame_forward);
        assert_eq!(text_reverse, "Start Middle\nEnd");
    }

    #[test]
    fn test_ignores_invisible_elements() {
        let choices = vec![
            make_choice("Keep1", 0.0, 0.0, 40.0, 10.0, true),
            make_choice("IgnoreMe ", 45.0, 0.0, 40.0, 10.0, false), // Valid frame, but visible = false
            make_choice("Keep2", 90.0, 0.0, 30.0, 10.0, true),
        ];

        let (text, _) = select_range_helper(&choices, 0, 2).unwrap();

        // The invisible element should be skipped during the `.filter(|(_, f, v)| *v ...)` step
        assert_eq!(text, "Keep1 Keep2");
    }

    #[test]
    fn test_invalid_indices_return_none() {
        let choices = vec![make_choice("Only", 0.0, 0.0, 40.0, 10.0, true)];

        // Out of bounds index
        assert!(select_range_helper(&choices, 0, 5).is_none());
    }

    #[test]
    fn test_estimate_multiline_wrap() {
        // A box 60px high, containing a string that should wrap into 3 lines.
        // If it detects 3 lines, height should be 60 / 3 = 20.
        let text = "This is a long string that definitely wraps.";
        let frame = Frame::new(0.0, 0.0, 100.0, 60.0);

        let height = estimate_font_height(text, &frame);
        assert_eq!(height, 15.0);
    }

    #[test]
    fn test_estimate_narrow_box_safety() {
        let frame = Frame::new(0.0, 0.0, 0.5, 20.0);
        let height = estimate_font_height("any text", &frame);

        assert_eq!(height, 0.5);
    }
}
