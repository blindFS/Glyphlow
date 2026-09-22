use crate::{
    action::html_to_attributed_string,
    config::{GlyphlowTheme, HintKeys, cgcolor_to_rgba},
    user_interface::UIDrawer,
    util::{lower_ascii, search_regex},
};
use objc2::rc::{Retained, autoreleasepool};
use objc2_app_kit::NSFontAttributeName;
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

/// A token of the picked text: the original, its hint label, and the folded form
/// searches are matched against.
#[derive(Debug, Clone)]
struct Word {
    text: String,
    label: String,
    ascii: String,
}

/// Splits a text into pickable words and draws them as a hint-labelled block.
pub struct WordPicker {
    raw: String,
    words: Vec<Word>,
    offsets: Vec<usize>,
    screen_ratio: f64,
    pub digits: u32,
    matched: Vec<usize>,
}

impl WordPicker {
    pub fn new(
        text: String,
        screen_ratio: f64,
        theme: &GlyphlowTheme,
        hint_keys: &HintKeys,
        drawer: &UIDrawer,
    ) -> Self {
        let (word_strings, offsets) = multilingual_split(&text);
        let digits = hint_keys.digits_for_len(word_strings.len());
        let mut words = Vec::new();
        for (i, text) in word_strings.into_iter().enumerate() {
            let label = hint_keys.label_for_index(i, Some(digits));
            let ascii = lower_ascii(&text);
            words.push(Word { text, label, ascii });
        }

        let word_picker = Self {
            raw: text,
            words,
            offsets,
            digits,
            screen_ratio,
            matched: Vec::new(),
        };

        autoreleasepool(|_| {
            if let Some((attr_string, _)) = word_picker.get_attributed_string(theme, None, "", "") {
                drawer.draw_attributed_string(theme, attr_string, true);
            }
        });

        word_picker
    }

    pub fn update_text_layer(
        &mut self,
        theme: &GlyphlowTheme,
        drawer: &UIDrawer,
        multi_selection_idx: Option<usize>,
        label_prefix: &str,
        text_prefix: &str,
    ) {
        autoreleasepool(|_| {
            if let Some((attr_string, matched)) =
                self.get_attributed_string(theme, multi_selection_idx, label_prefix, text_prefix)
            {
                self.matched = matched;
                drawer.draw_attributed_string(theme, attr_string, true);
            };
        })
    }

    /// Render the picker as HTML, with the label prefix and the search match
    /// highlighted, plus the indices of the words that matched.
    fn to_string(
        &self,
        width_height_ratio: f64,
        multi_selection_idx: Option<usize>,
        label_prefix: &str,
        text_prefix: &str,
    ) -> (String, Vec<usize>) {
        let text_pattern = search_regex(text_prefix);
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

        let mut matched = Vec::new();
        let helper = |label, class| format!("<span class=\"{class}\">{label}</span>");

        for (idx, w) in self.words.iter().enumerate() {
            let (this_class, label_html) = if multi_selection_idx.is_some_and(|other| other == idx)
            {
                // For already selected start/end, dim the label
                ("rh", helper(&w.label, "d"))
            } else if (!label_prefix.is_empty() || !text_prefix.is_empty())
                && w.label.starts_with(label_prefix)
                && text_pattern
                    .as_ref()
                    .is_none_or(|pattern| pattern.is_match(&w.ascii))
            {
                // For matched, highlight the label suffix
                matched.push(idx);
                (
                    "m",
                    format!(
                        "<span class=\"d\">{}</span><span class=\"h\">{}</span>",
                        label_prefix,
                        w.label.get(label_prefix.len()..).unwrap_or_default()
                    ),
                )
            } else if label_prefix.is_empty() && text_prefix.is_empty() {
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

        let buffer = format!(
            "<span class=\"h\">{}</span>\n{buffer}",
            if !(label_prefix.is_empty() && text_prefix.is_empty()) && matched.is_empty() {
                "Press 󰁮 to return"
            } else {
                "Press / to search"
            }
        );

        (buffer, matched)
    }

    /// The matches that still resolve to a word, in match order.
    ///
    /// Indices come from [`Self::to_string`], so a stale one should not happen;
    /// skipping it rather than panicking keeps a bad index from taking down an
    /// input handler.
    fn live_matches(&self) -> impl Iterator<Item = (usize, &str)> + '_ {
        self.matched
            .iter()
            .filter_map(|idx| self.words.get(*idx).map(|w| (*idx, w.text.as_str())))
    }

    /// How many words match the current prefix and search.
    pub fn matched_count(&self) -> usize {
        self.live_matches().count()
    }

    /// The first matching word, as `(index, text)`.
    pub fn first_matched_word(&self) -> Option<(usize, &str)> {
        self.live_matches().next()
    }

    /// Whether every match is the same word, which keeps the pick unambiguous
    /// even though several copies of it matched.
    ///
    /// False when nothing matched: an empty match set is not a unique pick, and
    /// saying so vacuously is exactly the kind of thing that turns into a bug at
    /// the next call site.
    pub fn all_matches_are_one_word(&self) -> bool {
        let mut live = self.live_matches();
        let Some((_, first)) = live.next() else {
            return false;
        };
        live.all(|(_, word)| word == first)
    }

    pub fn select_range(&self, idx1: usize, idx2: usize) -> Option<String> {
        let (start, end) = if idx1 < idx2 {
            (idx1, idx2)
        } else {
            (idx2, idx1)
        };
        let s_off = self.offsets.get(start)?;
        let e_off = self.offsets.get(end)? + self.words.get(end)?.text.len();

        self.raw.get(*s_off..e_off).map(String::from)
    }

    fn get_attributed_string(
        &self,
        theme: &GlyphlowTheme,
        multi_selection_idx: Option<usize>,
        label_prefix: &str,
        text_prefix: &str,
    ) -> Option<(Retained<NSMutableAttributedString>, Vec<usize>)> {
        let (html_str, matched) = self.to_string(
            self.screen_ratio,
            multi_selection_idx,
            label_prefix,
            text_prefix,
        );

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

        Some((attr_string, matched))
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

/// Split `input` into pickable tokens, with each token's byte offset in `input`.
///
/// Three levels, fallen through only as far as needed: whitespace and script
/// boundaries, then punctuation, then ASCII/non-ASCII — so a URL stays one token
/// while `Hello世界` splits into `Hello`, `世`, `界`.
fn multilingual_split(input: &str) -> (Vec<String>, Vec<usize>) {
    let url_re = get_url_re();
    let segment_re = get_segment_re();
    let mut result = Vec::new();
    let mut offsets = Vec::new();

    // Level 1: Split by whitespace and Regex
    for token in input.split_whitespace() {
        // Calculate the absolute byte offset of the token within 'input'
        let token_start = token.as_ptr() as usize - input.as_ptr() as usize;

        if url_re.is_match(token) {
            result.push(token.to_string());
            offsets.push(token_start);
        } else {
            for mat in segment_re.find_iter(token) {
                result.push(mat.as_str().to_string());
                offsets.push(token_start + mat.start());
            }
        }
    }

    if result.len() > 1 || result.is_empty() {
        return (result, offsets);
    }

    // Level 2: Split by punctuation (only if we have a single block)
    let base_offset = offsets[0];
    let w = &result[0];
    let mut l2_res = Vec::new();
    let mut l2_off = Vec::new();

    for s in w
        .split(|c: char| c.is_ascii_punctuation())
        .filter(|s| !s.is_empty())
    {
        let rel_offset = s.as_ptr() as usize - w.as_ptr() as usize;
        l2_res.push(s.to_string());
        l2_off.push(base_offset + rel_offset);
    }

    if l2_res.len() > 1 || l2_res.is_empty() {
        return (l2_res, l2_off);
    }

    // Level 3: Split ASCII/Non-ASCII (only if Level 2 still returned one block)
    let base_offset = l2_off[0];
    let w = &l2_res[0];
    let mut l3_res = Vec::new();
    let mut l3_off = Vec::new();

    let mut buffer = String::new();
    let mut buffer_start = 0;

    for (i, c) in w.char_indices() {
        if c.is_ascii() {
            if buffer.is_empty() {
                buffer_start = i;
            }
            buffer.push(c);
        } else {
            if !buffer.is_empty() {
                l3_res.push(buffer.clone());
                l3_off.push(base_offset + buffer_start);
                buffer.clear();
            }
            l3_res.push(c.to_string());
            l3_off.push(base_offset + i);
        }
    }
    if !buffer.is_empty() {
        l3_res.push(buffer);
        l3_off.push(base_offset + buffer_start);
    }

    (l3_res, l3_off)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// `multilingual_split` cuts a line into tokens at script boundaries and
    /// reports each token's *byte* offset, so every case pins both halves of
    /// the returned tuple.
    #[rstest]
    #[case::basic_latin("Hello world rust", &["Hello", "world", "rust"], &[0, 6, 12])]
    // "こんにちは" is 15 bytes, the space sits at byte 15, "世界" starts at 16.
    #[case::cjk_only("こんにちは 世界 常用漢字", &["こんにちは", "世界", "常用漢字"], &[0, 16, 23])]
    // Hello (5 bytes) + 世界 (6) + 2024 (4) + 年 (3), with no separators.
    #[case::mixed_adjacency("Hello世界2024年", &["Hello", "世界", "2024", "年"], &[0, 5, 11, 15])]
    #[case::alternating_scripts("Rustはawesomeです", &["Rust", "は", "awesome", "です"], &[0, 4, 7, 14])]
    #[case::punctuation_stays_with_its_word("Wait!世界...", &["Wait!", "世界", "..."], &[0, 5, 11])]
    // A URL stays one token even though it mixes ASCII, CJK and punctuation.
    #[case::url_is_one_token(
        "Check https://example.com/path/世界/page?query=1#hash",
        &["Check", "https://example.com/path/世界/page?query=1#hash"],
        &[0, 6]
    )]
    #[case::empty("", &[], &[])]
    #[case::whitespace_only("   ", &[], &[])]
    // Emoji are 4 bytes each, so the offsets are byte offsets, not char counts.
    #[case::emoji_are_four_bytes_each("English😁👻", &["English", "😁", "👻"], &[0, 7, 11])]
    fn splits_on_script_boundaries(
        #[case] input: &str,
        #[case] words: &[&str],
        #[case] offsets: &[usize],
    ) {
        let expected = (
            words
                .iter()
                .map(|w| (*w).to_owned())
                .collect::<Vec<String>>(),
            offsets.to_vec(),
        );
        assert_eq!(multilingual_split(input), expected);
    }

    /// A picker over `words` whose `matched` list is set verbatim, so the match
    /// queries can be exercised without a live `UIDrawer` (which is what
    /// [`WordPicker::new`] needs and a headless test cannot build).
    fn picker_with(words: &[&str], matched: &[usize]) -> WordPicker {
        WordPicker {
            raw: words.join(" "),
            words: words
                .iter()
                .map(|w| Word {
                    text: (*w).to_owned(),
                    label: String::new(),
                    ascii: lower_ascii(w),
                })
                .collect(),
            offsets: Vec::new(),
            screen_ratio: 1.0,
            digits: 1,
            matched: matched.to_vec(),
        }
    }

    /// These three queries are what decides whether a word-picking keypress is
    /// unambiguous, so pin each of them rather than only the combination the
    /// caller happens to use today.
    #[rstest]
    // Nothing matched is not a unique pick, and must not claim to be one by
    // vacuous truth.
    #[case::nothing_matched(&["alpha"], &[], 0, false, None)]
    // One match is unique, and trivially "all the same word".
    #[case::one_match(&["alpha"], &[0], 1, true, Some((0, "alpha")))]
    // Two copies of one word cannot be told apart, so the pick stays unique.
    #[case::duplicates_of_one_word(&["alpha", "alpha"], &[0, 1], 2, true, Some((0, "alpha")))]
    #[case::two_different_words(&["alpha", "beta"], &[0, 1], 2, false, Some((0, "alpha")))]
    #[case::same_word_thrice(&["go", "go", "go"], &[0, 1, 2], 3, true, Some((0, "go")))]
    // A stale index is skipped rather than panicking, and does not count.
    #[case::stale_index_is_skipped(&["alpha"], &[0, 7], 1, true, Some((0, "alpha")))]
    #[case::every_index_stale(&["alpha"], &[3, 4], 0, false, None)]
    fn match_queries_agree_on_whether_the_pick_is_unambiguous(
        #[case] words: &[&str],
        #[case] matched: &[usize],
        #[case] count: usize,
        #[case] all_one_word: bool,
        #[case] first: Option<(usize, &str)>,
    ) {
        let wp = picker_with(words, matched);
        assert_eq!(wp.matched_count(), count, "matched_count");
        assert_eq!(
            wp.all_matches_are_one_word(),
            all_one_word,
            "all_matches_are_one_word"
        );
        assert_eq!(wp.first_matched_word(), first, "first_matched_word");
    }
}
