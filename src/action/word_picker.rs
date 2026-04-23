use crate::util::{estimate_frame_for_text, hint_label_from_index};
use objc2::{
    AnyThread,
    rc::{DefaultRetained, Retained},
};
use objc2_app_kit::{
    NSBackgroundColorAttributeName, NSColor, NSFont, NSFontAttributeName, NSMutableParagraphStyle,
    NSParagraphStyleAttributeName,
};
use objc2_core_foundation::{CFRetained, CGSize};
use objc2_core_graphics::CGColor;
use objc2_foundation::{NSMutableAttributedString, NSRange, NSString};
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
struct Word {
    text: String,
    label: String,
}

pub struct WordPicker {
    words: Vec<Word>,
    digits: u32,
}

impl WordPicker {
    pub fn new(text: String) -> Self {
        let words = multilingual_split(&text);
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

    fn to_string(&self, prefix: &str) -> (String, Vec<(usize, usize)>, Vec<String>) {
        let mut buffer = String::new();
        let mut hl_ranges = Vec::new();
        let mut offset = 0;
        let mut matched_words = Vec::new();

        for w in self.words.iter() {
            let word_seg = format!("{}【{}】", w.text, w.label);
            let word_seg_utf16_len = word_seg.encode_utf16().count();
            if !prefix.is_empty() && w.label.starts_with(prefix) {
                hl_ranges.push((offset, word_seg_utf16_len - self.digits as usize - 2));
                matched_words.push(w.text.clone());
            }
            buffer.push_str(&word_seg);
            buffer.push(' ');
            offset += word_seg_utf16_len + 1;
        }
        (buffer, hl_ranges, matched_words)
    }

    pub fn get_attributed_string(
        &self,
        screen_size: CGSize,
        default_font: &Retained<NSFont>,
        hl_color: &CFRetained<CGColor>,
        prefix: &str,
    ) -> (CGSize, Retained<NSMutableAttributedString>, Vec<String>) {
        let (raw_str, highlight_ranges, matched_words) = self.to_string(prefix);
        let ns_string = NSString::from_str(&raw_str);
        let attr_string = NSMutableAttributedString::initWithString(
            NSMutableAttributedString::alloc(),
            &ns_string,
        );
        let str_len = attr_string.length();
        if str_len == 0 {
            return (screen_size, attr_string, matched_words);
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
            .round()
            * 1.5;
        let ideal_width = ideal_width.min(screen_size.width - 20.0);

        unsafe {
            let full_range = NSRange::new(0, str_len);
            attr_string.addAttribute_value_range(NSFontAttributeName, &font, full_range);

            // HACK: For multilingual text, height is underestimated due to fallback fonts.
            // This ensures more vertical spacing.
            let style = NSMutableParagraphStyle::default_retained();
            style.setLineSpacing(1.0);
            // style.setLineHeightMultiple(1.2);
            attr_string.addAttribute_value_range(NSParagraphStyleAttributeName, &style, full_range);

            let ptr = CGColor::components(Some(hl_color));
            let nscolor = if !ptr.is_null() {
                let r = *ptr.offset(0);
                let g = *ptr.offset(1);
                let b = *ptr.offset(2);
                let a = *ptr.offset(3);
                NSColor::colorWithRed_green_blue_alpha(r, g, b, a)
            } else {
                NSColor::blueColor()
            };

            // Background highlighting
            for (start, length) in highlight_ranges {
                // The matching word
                attr_string.addAttribute_value_range(
                    NSBackgroundColorAttributeName,
                    &nscolor,
                    NSRange::new(start, length),
                );

                // The remaining piece of hint label
                attr_string.addAttribute_value_range(
                    NSBackgroundColorAttributeName,
                    &nscolor,
                    NSRange::new(
                        start + length + 1 + prefix.len(),
                        self.digits as usize - prefix.len(),
                    ),
                );
            }
        }

        let (size, _) = estimate_frame_for_text(&attr_string, (ideal_width, screen_size.height));

        (size, attr_string, matched_words)
    }
}

const URL_PATTERN: &str = r"^[a-zA-Z][a-zA-Z0-9+.-]*://\S+$";

// Matches EITHER a sequence of CJK characters OR a sequence of everything else.
// This naturally separates them when they are adjacent.
const SCRIPT_SEGMENT_PATTERN: &str = r"([\u{4E00}-\u{9FFF}\u{3040}-\u{30FF}\u{AC00}-\u{D7AF}]+|[^\u{4E00}-\u{9FFF}\u{3040}-\u{30FF}\u{AC00}-\u{D7AF}]+)";

fn get_url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(URL_PATTERN).unwrap())
}

fn get_segment_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(SCRIPT_SEGMENT_PATTERN).unwrap())
}

// TODO: smarter split
fn multilingual_split(input: &str) -> Vec<String> {
    let url_re = get_url_re();
    let segment_re = get_segment_re();
    let mut result = Vec::new();

    // For a single piece without spaces, split by punctuations
    if !input.contains(' ') {
        return input
            .split(|c: char| c.is_ascii_punctuation())
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
    }

    // Split into words, CJK words are separated
    for token in input.split_whitespace() {
        if url_re.is_match(token) {
            result.push(token.to_string());
        } else {
            for mat in segment_re.find_iter(token) {
                result.push(mat.as_str().to_string());
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_latin_splitting() {
        let input = "Hello world rust";
        let expected = vec!["Hello", "world", "rust"];
        assert_eq!(multilingual_split(input), expected);
    }

    #[test]
    fn test_cjk_only_splitting() {
        // CJK groups should stay together if not separated by spaces
        let input = "こんにちは 世界 常用漢字";
        let expected = vec!["こんにちは", "世界", "常用漢字"];
        assert_eq!(multilingual_split(input), expected);
    }

    #[test]
    fn test_mixed_adjacency_splitting() {
        // This is the core requirement: split when script types change
        let input = "Hello世界2024年";
        let expected = vec!["Hello", "世界", "2024", "年"];
        assert_eq!(multilingual_split(input), expected);
    }

    #[test]
    fn test_url_protection() {
        // URLs containing CJK or special chars should remain intact
        let input =
            "Check https://example.com/path/世界/page?query=1#hash and ftp://files.org/data";
        let result = multilingual_split(input);

        assert!(result.contains(&"https://example.com/path/世界/page?query=1#hash".to_string()));
        assert!(result.contains(&"ftp://files.org/data".to_string()));
        assert_eq!(result[0], "Check");
    }

    #[test]
    fn test_multiple_script_boundaries() {
        // Testing complex mixed strings
        let input = "Rustはawesomeです";
        let expected = vec!["Rust", "は", "awesome", "です"];
        assert_eq!(multilingual_split(input), expected);
    }

    #[test]
    fn test_punctuation_behavior() {
        // Standard punctuation usually falls into the "Non-CJK" category
        // but since they are non-CJK, they cluster with ASCII words.
        let input = "Wait!世界...";
        let expected = vec!["Wait!", "世界", "..."];
        assert_eq!(multilingual_split(input), expected);
    }

    #[test]
    fn test_edge_case_empty_and_whitespace() {
        assert_eq!(multilingual_split(""), Vec::<String>::new());
        assert_eq!(multilingual_split("   "), Vec::<String>::new());
        assert_eq!(multilingual_split("\n\t "), Vec::<String>::new());
    }

    #[test]
    fn test_non_standard_protocols() {
        let input = "magnet:?xt=urn:btih:123 custom+proto://data";
        let expected = vec!["magnet:?xt=urn:btih:123", "custom+proto://data"];
        assert_eq!(multilingual_split(input), expected);
    }

    #[test]
    fn test_cjk_extensions_and_mixed_korean() {
        // Testing Hangul (Korean) adjacency
        let input = "Rust랑한국어";
        let expected = vec!["Rust", "랑한국어"];
        assert_eq!(multilingual_split(input), expected);
    }
}
