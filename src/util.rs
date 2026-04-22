use core_foundation::{attributed_string::CFAttributedStringRef, base::CFRange};
use core_graphics_types::geometry::CGSize;
use core_text::framesetter::CTFramesetter;
use objc2::rc::Retained;
use objc2_core_foundation::CGSize as OCGSize;
use objc2_foundation::NSMutableAttributedString;

pub fn hint_label_from_index(i: usize, digits: u32) -> String {
    let mut n = i;
    let mut result = Vec::new();

    while n > 0 {
        let remainder = (n % 26) as u8;
        let char = (b'A' + remainder) as char;
        result.push(char);
        n /= 26;
    }

    // pad to fixed length
    while result.len() < digits as usize {
        result.push('A');
    }
    result.iter().collect()
}

pub fn estimate_frame_for_text(
    attr_string: &Retained<NSMutableAttributedString>,
    size: (f64, f64),
) -> OCGSize {
    let cf_attr_string = Retained::as_ptr(attr_string) as CFAttributedStringRef;
    let framesetter = CTFramesetter::new_with_attributed_string(cf_attr_string);
    let (CGSize { width, height }, _) = framesetter.suggest_frame_size_with_constraints(
        CFRange {
            location: 0,
            length: 0,
        },
        std::ptr::null(),
        CGSize::new(size.0, size.1),
    );
    OCGSize::new(width, height)
}
