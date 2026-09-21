use core_foundation::{attributed_string::CFAttributedStringRef, base::CFRange};
use core_graphics_types::geometry::CGSize;
use core_text::framesetter::CTFramesetter;
use objc2::rc::Retained;
use objc2_core_foundation::{CGPoint, CGRect, CGSize as OCGSize};
use objc2_foundation::{NSMutableAttributedString, NSSize};
use regex::Regex;
use std::{borrow::Cow, cmp::Ordering};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MIN_HEIGHT_THRESHOLD: f64 = 10.0;

pub fn estimate_frame_for_text(
    attr_string: &Retained<NSMutableAttributedString>,
    size: (f64, f64),
) -> (OCGSize, isize) {
    let cf_attr_string = Retained::as_ptr(attr_string) as CFAttributedStringRef;
    let framesetter = CTFramesetter::new_with_attributed_string(cf_attr_string);
    let (CGSize { width, height }, range) = framesetter.suggest_frame_size_with_constraints(
        CFRange::init(0, 0),
        std::ptr::null(),
        CGSize::new(size.0, size.1),
    );
    (OCGSize::new(width, height), range.length)
}

#[derive(Debug, Clone, PartialEq, Copy, Default)]
pub struct Frame {
    pub top_left: CGPoint,
    pub bottom_right: CGPoint,
}

impl Eq for Frame {}

impl PartialOrd for Frame {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

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

    pub fn area(&self) -> f64 {
        let (w, h) = self.size();
        w * h
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
        let mut iter = frames.iter();
        let first = iter.next().cloned().unwrap_or_default();
        iter.fold(first, |acc, f| acc.union(f))
    }

    pub fn contains(&self, other: &Frame) -> bool {
        self.top_left.x <= other.top_left.x
            && self.top_left.y <= other.top_left.y
            && self.bottom_right.x >= other.bottom_right.x
            && self.bottom_right.y >= other.bottom_right.y
    }

    pub fn contains_point(&self, x: f64, y: f64) -> bool {
        self.top_left.x <= x
            && x <= self.bottom_right.x
            && self.top_left.y <= y
            && y <= self.bottom_right.y
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
    choices: &[(&str, Frame, bool)],
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
            let is_on_s_line = (f.bottom_right.y - s_frame.bottom_right.y).abs() < y_thres;
            let is_on_e_line = (f.bottom_right.y - e_frame.bottom_right.y).abs() < y_thres;

            if is_on_s_line && is_on_e_line {
                return f.top_left.x >= s_frame.top_left.x
                    && f.top_left.x <= e_frame.bottom_right.x;
            }
            if is_on_s_line && f.top_left.x >= s_frame.top_left.x {
                return true;
            }
            if is_on_e_line && f.top_left.x <= e_frame.bottom_right.x {
                return true;
            }
            false
        })
        .collect::<Vec<_>>();

    // Sort by Y strictly first (ensures total order)
    within_y_range.sort_by_key(|(_, f, _)| *f);

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

pub fn search_regex(text: &str) -> Option<Regex> {
    (!text.is_empty())
        .then_some({
            let text_pattern = text
                .split('󱁐')
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(".*");
            Regex::new(&format!(".*{text_pattern}.*"))
        })
        .and_then(|r| r.ok())
}

pub fn lower_ascii(text: &str) -> String {
    any_ascii::any_ascii(text).to_ascii_lowercase()
}

pub fn format_fixed_width(input: &str, fixed_length: usize) -> Cow<'_, str> {
    if input.width() <= fixed_length {
        return Cow::Borrowed(input);
    }

    if fixed_length == 0 {
        return "".into();
    }

    let mut current_width = 0;
    let mut trunc_byte_idx = input.len();

    for ch in input.chars().rev() {
        let ch_width = ch.width().unwrap_or_default();
        current_width += ch_width;
        if current_width > fixed_length - 1 {
            break;
        }
        trunc_byte_idx -= ch.len_utf8();
    }

    let mut s = ".".to_string();
    s.push_str(&input[trunc_byte_idx..]);
    Cow::Owned(s)
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    use rstest::rstest;
    use std::cmp::Ordering;

    /// `Ord` compares `bottom_right.y` first and only falls back to
    /// `top_left.x`, so vertical position always wins. `MIN_HEIGHT_THRESHOLD`
    /// plays no part in ordering — it is only used when guessing font height.
    #[rstest]
    #[case::vertical_priority(
        Frame::new(0.0, 0.0, 10.0, 50.0),
        Frame::new(100.0, 0.0, 110.0, 20.0),
        Ordering::Greater
    )]
    #[case::same_y_falls_back_to_x(
        Frame::new(10.0, 0.0, 20.0, 15.0),
        Frame::new(50.0, 0.0, 60.0, 15.0),
        Ordering::Less
    )]
    #[case::differing_y_ignores_x(
        Frame::new(10.0, 0.0, 20.0, 15.0),
        Frame::new(50.0, 0.0, 60.0, 16.0),
        Ordering::Less
    )]
    fn ordering_is_y_then_x(#[case] a: Frame, #[case] b: Frame, #[case] expected: Ordering) {
        assert_eq!(a.cmp(&b), expected);
        assert_eq!(
            b.cmp(&a),
            expected.reverse(),
            "ordering must be antisymmetric"
        );
    }

    /// The bounds check is inclusive (`<=`), so frames that merely touch —
    /// sharing an edge, or even a single corner — still count as intersecting.
    /// The result is a possibly zero-width or zero-height frame.
    #[rstest]
    #[case::partial_overlap(
        Frame::new(0.0, 0.0, 10.0, 10.0),
        Frame::new(5.0, 5.0, 15.0, 15.0),
        Some(Frame::new(5.0, 5.0, 10.0, 10.0))
    )]
    #[case::disjoint_on_both_axes(
        Frame::new(0.0, 0.0, 5.0, 5.0),
        Frame::new(10.0, 10.0, 15.0, 15.0),
        None
    )]
    #[case::x_overlaps_but_y_is_separated(
        Frame::new(0.0, 0.0, 50.0, 10.0),
        Frame::new(10.0, 20.0, 40.0, 30.0),
        None
    )]
    #[case::sharing_a_vertical_edge(
        Frame::new(0.0, 0.0, 10.0, 10.0),
        Frame::new(10.0, 0.0, 20.0, 10.0),
        Some(Frame::new(10.0, 0.0, 10.0, 10.0))
    )]
    #[case::zero_width_frame_inside(
        Frame::new(0.0, 0.0, 10.0, 10.0),
        Frame::new(5.0, 0.0, 5.0, 10.0),
        Some(Frame::new(5.0, 0.0, 5.0, 10.0))
    )]
    #[case::fully_contained(
        Frame::new(0.0, 0.0, 100.0, 100.0),
        Frame::new(20.0, 20.0, 50.0, 50.0),
        Some(Frame::new(20.0, 20.0, 50.0, 50.0))
    )]
    #[case::identical(
        Frame::new(10.0, 10.0, 20.0, 20.0),
        Frame::new(10.0, 10.0, 20.0, 20.0),
        Some(Frame::new(10.0, 10.0, 20.0, 20.0))
    )]
    #[case::overlapping_strip(
        Frame::new(0.0, 0.0, 20.0, 100.0),
        Frame::new(10.0, 0.0, 30.0, 100.0),
        Some(Frame::new(10.0, 0.0, 20.0, 100.0))
    )]
    #[case::negative_coordinates(
        Frame::new(-50.0, -50.0, -10.0, -10.0),
        Frame::new(-20.0, -20.0, 10.0, 10.0),
        Some(Frame::new(-20.0, -20.0, -10.0, -10.0))
    )]
    fn intersect_is_inclusive_and_commutative(
        #[case] a: Frame,
        #[case] b: Frame,
        #[case] expected: Option<Frame>,
    ) {
        assert_eq!(a.intersect(&b), expected);
        assert_eq!(
            b.intersect(&a),
            expected,
            "intersection must be commutative"
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

    /// Regression test: an earlier comparison was not a total order, so
    /// `sort` could panic with "user-provided comparison function does not
    /// correctly implement a total order". These three frames are the
    /// counter-example that used to trigger it.
    #[test]
    fn test_total_order_is_consistent_for_sorting() {
        let a = Frame::new(10.0, 80.0, 20.0, 100.0);
        let b = Frame::new(0.0, 100.0, 10.0, 105.0);
        let c = Frame::new(0.0, 90.0, 10.0, 110.0);

        // Transitivity: a < b and b < c must imply a < c.
        assert_eq!(a.cmp(&b), Ordering::Less);
        assert_eq!(b.cmp(&c), Ordering::Less);
        assert_eq!(a.cmp(&c), Ordering::Less);

        let mut frames = [a, b, c];
        frames.sort();
        assert_eq!(frames, [a, b, c]);
    }

    /// `contains` is inclusive on all four edges, matching `intersect`. The
    /// element explorer's early-stop relies on this: a parent whose frame only
    /// *touches* the child's still counts as containing it.
    #[rstest]
    #[case::strictly_inside(true, Frame::new(10.0, 10.0, 20.0, 20.0))]
    #[case::identical(true, Frame::new(0.0, 0.0, 100.0, 100.0))]
    #[case::touching_the_right_edge(true, Frame::new(50.0, 0.0, 100.0, 100.0))]
    #[case::touching_the_bottom_edge(true, Frame::new(0.0, 50.0, 100.0, 100.0))]
    #[case::one_pixel_past_the_right_edge(false, Frame::new(50.0, 0.0, 100.1, 100.0))]
    #[case::one_pixel_past_the_left_edge(false, Frame::new(-0.1, 0.0, 50.0, 100.0))]
    fn containment_is_inclusive_on_every_edge(#[case] expected: bool, #[case] inner: Frame) {
        let outer = Frame::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(outer.contains(&inner), expected);
    }

    /// The overlay frame is the union of every screen, and it is the coordinate
    /// space every hint box, ripple and cursor trail is drawn in — so a wrong
    /// union misplaces all of them.
    #[rstest]
    #[case::single_screen(
        vec![Frame::new(0.0, 0.0, 1920.0, 1080.0)],
        Frame::new(0.0, 0.0, 1920.0, 1080.0)
    )]
    #[case::screens_side_by_side(
        vec![Frame::new(0.0, 0.0, 1920.0, 1080.0), Frame::new(1920.0, 0.0, 3840.0, 1080.0)],
        Frame::new(0.0, 0.0, 3840.0, 1080.0)
    )]
    #[case::screen_to_the_left(
        vec![Frame::new(-1920.0, 0.0, 0.0, 1080.0), Frame::new(0.0, 0.0, 1920.0, 1080.0)],
        Frame::new(-1920.0, 0.0, 1920.0, 1080.0)
    )]
    fn unions_every_screen_into_one_overlay(#[case] screens: Vec<Frame>, #[case] expected: Frame) {
        assert_eq!(Frame::union_of_frames(&screens), expected);
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
    ) -> (&str, Frame, bool) {
        (text, Frame::new(x, y, x + w, y + h), visible)
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
    fn test_select_same_line_exclude_before_after() {
        let choices = vec![
            make_choice("Before ", 0.0, 0.0, 40.0, 10.0, true),
            make_choice("Start ", 45.0, 0.0, 40.0, 10.0, true),
            make_choice("Middle ", 90.0, 0.0, 40.0, 10.0, true),
            make_choice("End ", 135.0, 0.0, 40.0, 10.0, true),
            make_choice("After", 180.0, 0.0, 40.0, 10.0, true),
        ];

        // Select from "Start " (idx 1) to "End " (idx 3)
        let (text, frame) = select_range_helper(&choices, 1, 3).unwrap();

        // It should ONLY include "Start ", "Middle ", and "End "
        assert_eq!(text, "Start Middle End ");

        // Bounding box should encompass only the selected range
        assert_eq!(frame.top_left.x, 45.0);
        assert_eq!(frame.top_left.y, 0.0);
        assert_eq!(frame.bottom_right.x, 175.0);
        assert_eq!(frame.bottom_right.y, 10.0);
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

#[cfg(test)]
mod format_str_tests {
    use super::format_fixed_width;
    use rstest::rstest;

    /// `format_fixed_width` truncates from the *left*, replacing the dropped
    /// prefix with a single `.`, and it budgets in display columns rather than
    /// bytes — which is what the CJK and emoji cases pin down.
    #[rstest]
    #[case::empty_input("", 5, "")]
    #[case::empty_input_zero_width("", 0, "")]
    // A zero budget drops everything, even for non-empty input.
    #[case::zero_width_drops_everything("hello", 0, "")]
    #[case::exact_fit("hello", 5, "hello")]
    #[case::shorter_than_budget("abc", 5, "abc")]
    // Budget 3 leaves 2 columns for the suffix.
    #[case::ascii_truncated("hello", 3, ".lo")]
    // Budget 1 leaves no room for the suffix, so only the dot survives.
    #[case::budget_of_one("hello", 1, ".")]
    // "こんにちは" is 10 columns wide but 15 bytes.
    #[case::cjk_exact_fit("こんにちは", 10, "こんにちは")]
    #[case::cjk_truncated("こんにちは", 5, ".ちは")]
    // One column of slack, but the next character is 2 wide and cannot fit.
    #[case::cjk_slack_is_not_filled("こんにちは", 6, ".ちは")]
    // "🦀" is 2 columns wide, so three of them fill a budget of 6.
    #[case::emoji_exact_fit("🦀🦀🦀", 6, "🦀🦀🦀")]
    #[case::emoji_truncated("🦀🦀🦀", 5, ".🦀🦀")]
    #[case::mixed_width_keeps_only_the_emoji("Rust🦀", 3, ".🦀")]
    #[case::mixed_width_keeps_ascii_and_emoji("Rust🦀", 5, ".st🦀")]
    // A combining accent is zero-width, so it rides along with the dot.
    #[case::combining_diacritic_stays_attached("xyz\u{301}", 1, ".\u{301}")]
    fn truncates_to_a_fixed_display_width(
        #[case] input: &str,
        #[case] width: usize,
        #[case] expected: &str,
    ) {
        assert_eq!(format_fixed_width(input, width), expected);
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use rstest::rstest;

    /// The query is a "contains" pattern, but `󱁐` — the in-band stand-in the key
    /// listener substitutes for a real space — means "anything in between".
    /// Without it a two-word query would be impossible to express, because the
    /// space key is never delivered as part of a query.
    #[rstest]
    #[case::plain_substring("alpha", "alpha beta", true)]
    #[case::substring_anywhere("pha", "alpha beta", true)]
    #[case::absent("omega", "alpha beta", false)]
    #[case::words_in_order("alpha󱁐beta", "alpha beta", true)]
    #[case::words_with_a_gap("alpha󱁐beta", "alpha and beta", true)]
    #[case::words_out_of_order("beta󱁐alpha", "alpha beta", false)]
    fn matches_a_substring_with_in_band_word_separators(
        #[case] query: &str,
        #[case] haystack: &str,
        #[case] expected: bool,
    ) {
        let re = search_regex(query).expect("a non-empty query always compiles");
        assert_eq!(re.is_match(haystack), expected, "query {query:?}");
    }

    /// An empty query means "no filter", which callers detect as `None` — not as
    /// a pattern that matches nothing.
    #[test]
    fn an_empty_query_is_no_filter() {
        assert!(search_regex("").is_none());
    }

    /// Stray separators collapse, so a query cannot be broken by an accidental
    /// space at either end or by two in a row.
    #[rstest]
    #[case::leading("󱁐alpha", "alpha")]
    #[case::trailing("alpha󱁐", "alpha")]
    #[case::repeated("alpha󱁐󱁐󱁐beta", "alpha󱁐beta")]
    fn stray_separators_collapse(#[case] query: &str, #[case] equivalent: &str) {
        assert_eq!(
            search_regex(query).unwrap().as_str(),
            search_regex(equivalent).unwrap().as_str(),
            "{query:?} must behave exactly like {equivalent:?}"
        );
    }

    /// A query made of separators alone collapses to the empty pattern, which
    /// matches everything. That is the same *effect* as an empty query, but
    /// reached by a different route: `None` versus a pattern that matches
    /// anything. Both are "no filter" to the callers, which test with
    /// `is_none_or`.
    #[test]
    fn a_query_of_separators_alone_matches_everything() {
        let re = search_regex("󱁐").expect("a non-empty query still yields a pattern");

        assert!(re.is_match(""));
        assert!(re.is_match("anything at all"));
    }

    /// The user types into a live search box, so an uncompilable pattern is one
    /// keystroke away. It must degrade to "no filter" — filtering everything out
    /// would look like the app had lost its content.
    #[rstest]
    #[case::unclosed_group("(")]
    #[case::unclosed_class("[")]
    fn a_malformed_query_degrades_to_no_filter(#[case] query: &str) {
        assert!(
            search_regex(query).is_none(),
            "{query:?} cannot be compiled, so it must not filter anything out"
        );
    }

    /// Search targets and word-picker words are compared case- and
    /// accent-insensitively, so this folding happens once, up front.
    #[rstest]
    #[case::uppercase("ALPHA", "alpha")]
    #[case::accented("CAFÉ", "cafe")]
    #[case::sharp_s("Straße", "strasse")]
    fn lower_ascii_folds_case_and_accents(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(lower_ascii(input), expected);
    }
}
