use std::fmt::Display;

use objc2::{
    AnyThread,
    rc::{DefaultRetained, Retained},
};
use objc2_app_kit::{
    NSFont, NSFontAttributeName, NSMutableParagraphStyle, NSParagraphStyleAttributeName,
};
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSMutableAttributedString, NSRange, NSString};

use crate::util::{estimate_frame_for_text, hint_label_from_index};

#[derive(Debug, Clone)]
pub struct Word {
    pub text: String,
    pub label: String,
}

pub struct WordPicker {
    pub words: Vec<Word>,
    pub digits: u32,
}

impl WordPicker {
    pub fn new(words: Vec<String>) -> Self {
        let digits = words.len().ilog(26) + 1;
        let mut result = Vec::new();
        for (i, text) in words.into_iter().enumerate() {
            let label = hint_label_from_index(i, digits);
            result.push(Word { text, label });
        }
        Self {
            words: result,
            digits,
        }
    }

    // TODO: highlighting
    pub fn get_attributed_string(
        &self,
        screen_size: CGSize,
        default_font: &Retained<NSFont>,
    ) -> (CGSize, Retained<NSMutableAttributedString>) {
        let ns_string = NSString::from_str(&self.to_string());
        let attr_string = NSMutableAttributedString::initWithString(
            NSMutableAttributedString::alloc(),
            &ns_string,
        );
        let str_len = attr_string.length();
        if str_len == 0 {
            return (screen_size, attr_string);
        }
        // Approximate font area as 1.5 * 0.7 font_size ^ 2
        let CGSize {
            width: w,
            height: h,
        } = screen_size;
        let max_font_size = (w * h / str_len as f64 / 1.5 / 0.7).sqrt().round();
        let font_size = max_font_size.min(default_font.pointSize()).max(1.0);
        let font = NSFont::fontWithName_size(&default_font.fontName(), font_size)
            .unwrap_or_else(|| default_font.clone());

        // Estimate ideal frame width to keep close width-height ratio as screen_size
        let ideal_width = (str_len as f64 * font_size * font_size * 1.3 * 0.6 * w / (h + 0.1))
            .sqrt()
            .round();
        let full_range = NSRange::new(0, str_len);

        unsafe {
            attr_string.addAttribute_value_range(NSFontAttributeName, &font, full_range);

            // HACK: For multilingual text, height is underestimated due to fallback fonts.
            // This ensures more vertical spacing.
            let style = NSMutableParagraphStyle::default_retained();
            style.setLineSpacing(1.0);
            // style.setLineHeightMultiple(1.2);
            attr_string.addAttribute_value_range(NSParagraphStyleAttributeName, &style, full_range);
        }

        let size = estimate_frame_for_text(&attr_string, (ideal_width, screen_size.height));

        (size, attr_string)
    }
}

impl Display for WordPicker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = self
            .words
            .iter()
            .map(|w| format!("{}【{}】", w.text, w.label))
            .collect::<Vec<_>>()
            .join(" ");
        write!(f, "{str}")
    }
}
