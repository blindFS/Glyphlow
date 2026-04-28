use crate::{
    action::html_to_attributed_string,
    config::{GlyphlowTheme, cgcolor_to_rgba},
    util::{estimate_frame_for_text, hint_label_from_index},
};
use objc2::rc::Retained;
use objc2_app_kit::{NSFont, NSFontAttributeName};
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSMutableAttributedString, NSRange};
use regex::Regex;
use std::sync::OnceLock;
use unicode_width::UnicodeWidthStr;

const WORD_PICKER_STYLE: &str = r#"
<style>
body {
    font-size: 25px;
    line-height: 1.5;
    color: {fg_color};
}
.line { display: block; }
.h { color: {hl_color} }
.rh { color: {bg_color}; background-color: {hl_color}; }
.d { color: {dim_color} }
</style>"#;

#[derive(Debug, Clone)]
struct Word {
    text: String,
    label: String,
}

pub struct WordPicker {
    words: Vec<Word>,
    pub digits: u32,
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

    /// Returns HTML string
    fn to_string(
        &self,
        prefix: &str,
        width_height_ratio: f64,
        multi_selection_idx: Option<usize>,
    ) -> String {
        let total_unicode_width = self.words.iter().map(|w| w.text.width()).sum::<usize>()
            + self.words.len() * (2 + self.digits as usize);
        // ideal_width / (total_unicode_width / ideal_width) 󰾞 ratio * 3
        let ideal_width = (total_unicode_width as f64 * width_height_ratio * 3.0)
            .sqrt()
            .round() as usize;

        let line_span_head = "<span class=\"line\">";
        let mut buffer = String::new();
        let mut line_width = 0;
        buffer.push_str(line_span_head);

        let helper = |label, class| format!("<span class=\"{class}\">{label}</span>");

        for (idx, w) in self.words.iter().enumerate() {
            let (this_class, label_html) = if multi_selection_idx.is_some_and(|other| other == idx)
            {
                // For already selected start/end, dim the label
                ("rh", helper(&w.label, "d"))
            } else if !prefix.is_empty() && w.label.starts_with(prefix) {
                // For matched, highlight the label suffix
                (
                    "m",
                    format!(
                        "<span class=\"d\">{}</span><span class=\"h\">{}</span>",
                        prefix,
                        w.label.get(prefix.len()..).unwrap_or_default()
                    ),
                )
            } else if prefix.is_empty() {
                // No prefix, highlight the all labels
                ("n", helper(&w.label, "h"))
            } else {
                // Unmatched choices, dim whole span
                ("d", helper(&w.label, "d"))
            };
            let this_span = format!(
                "<span class=\"{}\">{}</span> {} ",
                this_class, w.text, label_html
            );

            let this_width = w.text.width() + self.digits as usize + 2;
            if line_width + this_width <= ideal_width {
                line_width += this_width;
            } else {
                // If the line is empty, don't add an empty span
                if line_width > 0 {
                    buffer.push_str("</span>");
                    buffer.push_str(line_span_head);
                }
                line_width = this_width;
            }
            buffer.push_str(&this_span);
        }
        buffer.push_str("</span>");

        buffer
    }

    pub fn matched_words(&self, prefix: &str) -> Vec<(usize, String)> {
        self.words
            .iter()
            .enumerate()
            .filter(|(_, w)| !prefix.is_empty() && w.label.starts_with(prefix))
            .map(|(idx, w)| (idx, w.text.clone()))
            .collect()
    }

    pub fn select_range(&self, idx1: usize, idx2: usize) -> Option<String> {
        let (start, end) = if idx1 < idx2 {
            (idx1, idx2)
        } else {
            (idx2, idx1)
        };
        let s: Vec<String> = self
            .words
            .get(start..=end)
            .map(|w| w.iter().map(|w| w.text.clone()).collect())?;
        Some(s.join(" "))
    }

    pub fn get_attributed_string(
        &self,
        screen_size: CGSize,
        theme: &GlyphlowTheme,
        prefix: &str,
        multi_selection_idx: Option<usize>,
    ) -> Option<(CGSize, Retained<NSMutableAttributedString>)> {
        let CGSize { width, height } = screen_size;
        let html_str = self.to_string(prefix, width / (height + 0.01), multi_selection_idx);

        // CSS colors
        let attr_string = html_to_attributed_string(
            &html_str,
            Some(&replace_color_in_css(WORD_PICKER_STYLE, theme, 3)),
        )?;

        unsafe {
            attr_string.addAttribute_value_range(
                NSFontAttributeName,
                &theme.menu_font,
                NSRange::new(0, attr_string.length()),
            );
        }

        let (size, _) = estimate_frame_for_text(&attr_string, (width * 3.0, height * 3.0));

        // In case the default font size is too large
        let shrinkage = (width / size.width).min(height / size.height);
        if shrinkage < 1.0 {
            // Don't shrink too much
            let font_size = shrinkage * theme.menu_font.pointSize().max(10.0);
            if let Some(new_font) =
                NSFont::fontWithName_size(&theme.menu_font.fontName(), font_size)
            {
                unsafe {
                    attr_string.addAttribute_value_range(
                        NSFontAttributeName,
                        &new_font,
                        NSRange::new(0, attr_string.length()),
                    );
                }

                let (size, _) = estimate_frame_for_text(&attr_string, (width, height));
                return Some((size, attr_string));
            };
        }

        Some((size, attr_string))
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

fn rgba_to_css_color(rgba: (u8, u8, u8, u8)) -> String {
    let (r, g, b, a) = rgba;
    format!("rgba({}, {}, {}, {:.2})", r, g, b, a as f64 / 255.0)
}

fn replace_color_in_css(css: &str, theme: &GlyphlowTheme, dim_level: u8) -> String {
    let default_rgba = (255, 255, 255, 255);
    let fg_rgba = cgcolor_to_rgba(&theme.menu_fg_color).unwrap_or(default_rgba);
    let bg_rgba = cgcolor_to_rgba(&theme.menu_bg_color).unwrap_or(default_rgba);
    let mut dim_rgba = fg_rgba;
    dim_rgba.3 /= dim_level;
    css.replace("{fg_color}", &rgba_to_css_color(fg_rgba))
        .replace("{bg_color}", &rgba_to_css_color(bg_rgba))
        .replace(
            "{hl_color}",
            &rgba_to_css_color(cgcolor_to_rgba(&theme.menu_hl_color).unwrap_or(default_rgba)),
        )
        .replace("{dim_color}", &rgba_to_css_color(dim_rgba))
}

// TODO: smarter split
fn multilingual_split(input: &str) -> Vec<String> {
    let url_re = get_url_re();
    let segment_re = get_segment_re();
    let mut result = Vec::new();

    // Split into words, CJK words are separated
    for token in input.split_whitespace() {
        if url_re.is_match(token) {
            result.push(token.to_string());
        } else {
            for mat in segment_re.find_iter(token) {
                // hello, -> hello
                let w = mat.as_str().to_string();
                if !w.is_empty() {
                    result.push(w);
                }
            }
        }
    }

    // For a single piece without spaces, split by punctuations
    if result.len() == 1
        && let Some(w) = result.first()
    {
        return w
            .split(|c: char| c.is_ascii_punctuation())
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
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
