use crate::{
    ax_element::{ElementOfInterest, GetAttribute},
    config::{BACKSPACE_CHAR, GlyphlowTheme, HintKeys},
    user_interface::{MenuString, MenuStyle, UIDrawer},
    util::{Frame, lower_ascii, search_regex, shrink_font_size},
};
use regex::Regex;
use std::{collections::HashMap, ops::Range, sync::OnceLock};
use unicode_width::UnicodeWidthStr;

/// Between a word and its own label.
const WORD_LABEL_GAP: &str = " ";
/// Between a label and the next word, wider so the label reads as part of the
/// word it picks rather than the word after it.
const LABEL_WORD_GAP: &str = "  ";

/// One entry's width in monospace cells, matching the spaces the loop emits.
fn entry_width(word: &str, digits: u32) -> usize {
    word.width() + WORD_LABEL_GAP.len() + digits as usize + LABEL_WORD_GAP.len()
}

/// A token of the picked text: where it sits in [`WordPicker::raw`], its hint
/// label, the folded form searches are matched against, and — once the source
/// element has placed it — its frame on screen.
#[derive(Debug, Clone)]
struct Word {
    text: Range<usize>,
    label: String,
    ascii: String,
    /// All-or-nothing: either every word has one and the picker draws in place,
    /// or none do and it draws a menu.
    frame: Option<Frame>,
}

/// Splits a text into pickable words.
///
/// When the source element can place a ranged piece of its text on screen, each
/// word has a `frame` and the picker is drawn as hint boxes over the words
/// themselves. Otherwise it falls back to a hint-labelled block in a menu.
pub(crate) struct WordPicker {
    raw: String,
    words: Vec<Word>,
    screen_ratio: f64,
    pub(crate) digits: u32,
    matched: Vec<usize>,
    /// Where `raw` starts in the source element's text, in UTF-16 units, so a
    /// picked word or range can report its absolute position.
    context_offset: usize,
    pub in_place: bool,
}

impl WordPicker {
    /// `element` is the source the text was read from. It is only used to place
    /// the words on screen; `None`, or an element that cannot answer
    /// `AXBoundsForRange`, keeps the menu fallback. A narrowed context knows
    /// where it sits in that element, so the picker does too.
    pub(crate) fn new(
        text: String,
        screen_ratio: f64,
        screen_frame: &Frame,
        theme: &GlyphlowTheme,
        hint_keys: &HintKeys,
        drawer: &UIDrawer,
        element: Option<&ElementOfInterest>,
    ) -> Self {
        let (word_strings, offsets) = multilingual_split(&text);
        let digits = hint_keys.digits_for_len(word_strings.len());
        let mut words = Vec::with_capacity(word_strings.len());
        for (i, (word, offset)) in word_strings.into_iter().zip(offsets).enumerate() {
            let label = hint_keys.label_for_index(i, Some(digits));
            words.push(Word {
                text: offset..offset + word.len(),
                label,
                ascii: lower_ascii(&word),
                frame: None,
            });
        }

        let context_offset = element.map_or(0, |element| element.context_offset);
        let mut in_place = false;
        if let Some(element) = element {
            if element.element().is_some() {
                in_place = place_words(element, &text, &mut words, screen_frame)
            } else {
                log::info!(
                    "Word picker: the source is synthetic text with no AX element, so {} word(s) cannot be placed; falling back to the menu.",
                    words.len()
                );
            }
        };

        let word_picker = Self {
            raw: text,
            words,
            digits,
            screen_ratio,
            matched: Vec::new(),
            context_offset,
            in_place,
        };

        if !in_place {
            let (msg, _) = word_picker.menu_string(None, "", "");
            drawer.draw_menu(&msg, theme);
        }

        word_picker
    }

    /// The frames of the words, when [`Self::in_place`] holds.
    pub(crate) fn frames(&self) -> impl ExactSizeIterator<Item = Frame> + '_ {
        self.words
            .iter()
            .map(|word| word.frame.expect("in_place checks every word's frame"))
    }

    /// The folded search form of the word at `idx`.
    pub(crate) fn search_target(&self, idx: usize) -> Option<&str> {
        self.words.get(idx).map(|word| word.ascii.as_str())
    }

    /// The word at `idx`, and where it starts in the source element's text
    /// (UTF-16 units).
    pub(crate) fn word(&self, idx: usize) -> Option<(&str, usize)> {
        let word = self.words.get(idx)?;
        let prefix = self.raw[..word.text.start].encode_utf16().count();
        Some((&self.raw[word.text.clone()], self.context_offset + prefix))
    }

    const MIN_HINT_FONT_SIZE: f64 = 8.0;

    /// The hint font size that keeps a label inside the narrowest word, or
    /// `None` when the configured font already fits.
    ///
    /// A box is about `digits * size * 0.8 + 2 * margin` wide — the same
    /// estimate collision resolution uses. Inverting that for the smallest word
    /// width stops short words from turning the hints into a pile of overlapping
    /// boxes. The result is never larger than the configured font, and never
    /// below a legible floor: a word with no width at all (or one narrower than
    /// the box's margins) gets the floor, not a zero-size font.
    pub(crate) fn hint_font_size(&self, theme: &GlyphlowTheme) -> Option<f64> {
        if !self.in_place {
            return None;
        }
        let configured = theme.hint_font.pointSize();
        let min_width = self
            .words
            .iter()
            .filter_map(|word| word.frame)
            .map(|frame| frame.size().0)
            .fold(f64::INFINITY, f64::min);
        let margins = theme.hint_margin_size as f64 * 2.0;
        let label_width = configured * self.digits.max(1) as f64 * 0.8;

        // Collapsing the floor to the configured size keeps both bounds in order
        // when the configured font is already smaller than the floor.
        let floor = Self::MIN_HINT_FONT_SIZE.min(configured);
        let ratio = (min_width - margins).max(0.0) / label_width;
        shrink_font_size(ratio, configured, floor).filter(|&size| size < configured)
    }

    pub(crate) fn update_text_layer(
        &mut self,
        theme: &GlyphlowTheme,
        drawer: &UIDrawer,
        multi_selection_idx: Option<usize>,
        label_prefix: &str,
        text_prefix: &str,
    ) {
        let (msg, matched) = self.menu_string(multi_selection_idx, label_prefix, text_prefix);
        self.matched = matched;
        drawer.draw_menu(&msg, theme);
    }

    /// The picker's text, with the key of every label and the match state of
    /// every word styled, plus the indices of the words that matched.
    fn menu_string(
        &self,
        multi_selection_idx: Option<usize>,
        label_prefix: &str,
        text_prefix: &str,
    ) -> (MenuString, Vec<usize>) {
        let total_unicode_width = self
            .words
            .iter()
            .map(|w| entry_width(&self.raw[w.text.clone()], self.digits))
            .sum::<usize>();
        // ideal_width / (total_unicode_width / ideal_width) 󰾞 ratio * 3
        let ideal_width = (total_unicode_width as f64 * self.screen_ratio * 3.0)
            .sqrt()
            .round() as usize;

        let filtering = !label_prefix.is_empty() || !text_prefix.is_empty();
        let typed = label_prefix.len();
        let text_pattern = search_regex(text_prefix);
        let matches = |word: &Word| {
            filtering
                && word.label.starts_with(label_prefix)
                && text_pattern
                    .as_ref()
                    .is_none_or(|pattern| pattern.is_match(&word.ascii))
        };

        let matched: Vec<usize> = self
            .words
            .iter()
            .enumerate()
            .filter_map(|(idx, word)| matches(word).then_some(idx))
            .collect();

        // The header names the picker, so every part of it is styled as a header;
        // the key inside it stays a key.
        let (hint_key, hint) = if filtering && matched.is_empty() {
            (BACKSPACE_CHAR, " to return")
        } else {
            ('/', " to search")
        };
        let mut buffer = MenuString::styled("Press ", MenuStyle::Header);
        buffer
            .push_styled(hint_key.encode_utf8(&mut [0; 4]), MenuStyle::Key)
            .push_styled(hint, MenuStyle::Header)
            .push("\n");

        let mut line_width = 0;
        for (idx, word) in self.words.iter().enumerate() {
            let text = &self.raw[word.text.clone()];
            let this_width = entry_width(text, self.digits);
            // Wrap when this word would overflow, unless the line is still empty.
            if line_width + this_width > ideal_width && line_width > 0 {
                buffer.push("\n");
                line_width = 0;
            } else if line_width > 0 {
                // Separate entries on a line, but keep the gap off either end.
                buffer.push(LABEL_WORD_GAP);
            }
            line_width += this_width;

            if multi_selection_idx == Some(idx) {
                // A picked end of the range: its key cannot be pressed again.
                buffer
                    .push_styled(text, MenuStyle::Text)
                    .push(WORD_LABEL_GAP)
                    .push_styled(&word.label, MenuStyle::Dim);
            } else if matched.binary_search(&idx).is_ok() {
                // The typed part of the key cannot be pressed again either.
                buffer
                    .push(text)
                    .push(WORD_LABEL_GAP)
                    .push_styled(&word.label[..typed], MenuStyle::Dim)
                    .push_styled(&word.label[typed..], MenuStyle::Key);
            } else if filtering {
                buffer
                    .push_styled(text, MenuStyle::Dim)
                    .push(WORD_LABEL_GAP)
                    .push_styled(&word.label, MenuStyle::Dim);
            } else {
                buffer
                    .push(text)
                    .push(WORD_LABEL_GAP)
                    .push_styled(&word.label, MenuStyle::Key);
            }
        }
        buffer.push("\n");

        (buffer, matched)
    }

    /// The matches that still resolve to a word, in match order.
    ///
    /// Indices come from [`Self::menu_string`], so a stale one should not happen;
    /// skipping it rather than panicking keeps a bad index from taking down an
    /// input handler.
    fn live_matches(&self) -> impl Iterator<Item = (usize, &str)> + '_ {
        self.matched.iter().filter_map(|idx| {
            self.words
                .get(*idx)
                .map(|w| (*idx, &self.raw[w.text.clone()]))
        })
    }

    /// The match to pick, when the matches leave no choice: the only one, or
    /// several that are all the same word while `multi_selection` is off, since
    /// then there is nothing to tell apart.
    ///
    /// `None` when nothing matched or the pick is ambiguous.
    pub(crate) fn unambiguous_match(&self, multi_selection: bool) -> Option<(usize, &str)> {
        let mut live = self.live_matches();
        let (idx, text) = live.next()?;
        let Some((_, second)) = live.next() else {
            return Some((idx, text));
        };

        (!multi_selection && second == text && live.all(|(_, word)| word == text))
            .then_some((idx, text))
    }

    /// The text of the range between `idx1` and `idx2`, plus where it starts in
    /// the source element's text (UTF-16 units).
    pub(crate) fn select_range(&self, idx1: usize, idx2: usize) -> Option<(&str, usize)> {
        let (start, end) = if idx1 < idx2 {
            (idx1, idx2)
        } else {
            (idx2, idx1)
        };
        let s_off = self.words.get(start)?.text.start;
        let e_off = self.words.get(end)?.text.end;
        let offset = self.context_offset + self.raw[..s_off].encode_utf16().count();

        self.raw.get(s_off..e_off).map(|text| (text, offset))
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

/// The byte range of `text` as an AX text range (UTF-16 units from the start),
/// which is what `AXBoundsForRange` takes.
fn utf16_range(text: &str, range: &Range<usize>) -> (usize, usize) {
    let location = text[..range.start].encode_utf16().count();
    let length = text[range.clone()].encode_utf16().count();
    (location, length)
}

/// Place every word on screen, or leave them all unplaced.
///
/// In place only works when **all** words resolve to a real, distinct rectangle:
/// a missing one would leave a word that cannot be picked, and two words sharing
/// a rectangle would stack their hints on top of each other.
fn place_words(
    element: &ElementOfInterest,
    text: &str,
    words: &mut [Word],
    screen_frame: &Frame,
) -> bool {
    if words.is_empty() {
        log::info!("Word picker: the text has no words to split; falling back to the menu.");
        return false;
    }

    let mut frames = Vec::with_capacity(words.len());
    for (idx, word) in words.iter().enumerate() {
        let word_text = &text[word.text.clone()];
        let (location, length) = utf16_range(text, &word.text);
        let Some(frame) = element
            .element()
            .and_then(|ax| ax.bounds_for_range(element.context_offset + location, length))
        else {
            log::info!(
                "Word picker: AX could not place word {idx} `{word_text}` (range {location}..+{length}); falling back to the menu."
            );
            return false;
        };
        let Some(visible) = frame.intersect(screen_frame) else {
            log::info!(
                "Word picker: word {idx} `{word_text}` at {frame:?} is outside the screen {screen_frame:?}; falling back to the menu."
            );
            return false;
        };
        let (w, h) = visible.size();
        if w <= 0.0 || h <= 0.0 {
            log::info!(
                "Word picker: word {idx} `{word_text}` at {frame:?} has no visible area {visible:?}; falling back to the menu."
            );
            return false;
        }
        frames.push(frame);
    }

    if let Some((first, second, frame)) = duplicate_frame(&frames) {
        log::info!(
            "Word picker: words {first} `{}` and {second} `{}` share the frame {frame:?}; falling back to the menu.",
            &text[words[first].text.clone()],
            &text[words[second].text.clone()]
        );
        return false;
    }

    for (word, frame) in words.iter_mut().zip(frames) {
        word.frame = Some(frame);
    }
    true
}

/// The first pair of frames that coincide, with both indices.
fn duplicate_frame(frames: &[Frame]) -> Option<(usize, usize, Frame)> {
    let mut seen = HashMap::with_capacity(frames.len());
    frames.iter().enumerate().find_map(|(idx, frame)| {
        seen.insert(*frame, idx)
            .map(|earlier| (earlier, idx, *frame))
    })
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
                l3_res.push(std::mem::take(&mut buffer));
                l3_off.push(base_offset + buffer_start);
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

    /// `AXBoundsForRange` addresses text in UTF-16 units, while the split works
    /// in byte offsets, so the conversion is what keeps the frames on the right
    /// words once anything outside ASCII is involved.
    #[rstest]
    #[case::ascii("hello world", 0..5, (0, 5))]
    #[case::multibyte_prefix("é alpha", 3..8, (2, 5))]
    #[case::emoji_counts_as_two_units("😁 hi", 5..7, (3, 2))]
    fn maps_byte_ranges_to_ax_utf16_ranges(
        #[case] input: &str,
        #[case] range: Range<usize>,
        #[case] expected: (usize, usize),
    ) {
        assert_eq!(utf16_range(input, &range), expected);
    }

    /// Picking a range on a split that already owns a slice of the element must
    /// carry that slice's start along, or the next split would place its hints
    /// at the top of the element.
    #[test]
    fn picked_range_keeps_its_offset_in_the_element() {
        let mut picker = picker_with(&["alpha", "beta", "gamma"], &[]);
        picker.context_offset = 40;

        assert_eq!(picker.word(1).map(|(_, offset)| offset), Some(46));
        assert_eq!(picker.select_range(2, 1), Some(("beta gamma", 46)));
    }

    /// AX reporting one rectangle for several words is the signal that the split
    /// cannot be drawn in place: the hints would stack on the same spot.
    #[test]
    fn identical_word_frames_are_not_distinct() {
        let first = Frame::new(0.0, 0.0, 40.0, 20.0);
        let second = Frame::new(40.0, 0.0, 80.0, 20.0);

        assert!(duplicate_frame(&[]).is_none());
        assert!(duplicate_frame(&[first]).is_none());
        assert!(duplicate_frame(&[first, second]).is_none());
        assert_eq!(duplicate_frame(&[first, first]), Some((0, 1, first)));
        assert_eq!(
            duplicate_frame(&[first, second, first]),
            Some((0, 2, first))
        );
    }

    /// The smallest word decides the font, so that is the width varied here.
    /// `None` means the configured font already fits.
    #[rstest]
    #[case::wide_words_keep_the_configured_font(60.0, None)]
    #[case::narrow_words_shrink_to_the_floor(10.0, Some(8.0))]
    #[case::zero_width_still_gets_a_legible_font(0.0, Some(8.0))]
    fn hint_font_size_fits_the_narrowest_word(
        #[case] min_width: f64,
        #[case] expected: Option<f64>,
    ) {
        let mut picker = picker_with(&["a", "b"], &[]);
        picker.in_place = true;
        picker.words[0].frame = Some(Frame::new(0.0, 0.0, min_width, 20.0));
        picker.words[1].frame = Some(Frame::new(0.0, 0.0, 100.0, 20.0));

        assert_eq!(picker.hint_font_size(&GlyphlowTheme::default()), expected);
    }

    /// The floor collapses to the configured size when that is already smaller,
    /// so the clamp never receives inverted bounds — and never enlarges a label.
    #[test]
    fn hint_font_size_never_enlarges_a_small_configured_font() {
        let theme = GlyphlowTheme {
            hint_font: objc2_app_kit::NSFont::fontWithName_size(
                objc2_foundation::ns_string!("Andale Mono"),
                6.0,
            )
            .unwrap(),
            ..Default::default()
        };

        let mut picker = picker_with(&["a"], &[]);
        picker.words[0].frame = Some(Frame::new(0.0, 0.0, 0.0, 20.0));

        assert_eq!(picker.hint_font_size(&theme), None);
    }

    /// A picker over `words` whose `matched` list is set verbatim, so the match
    /// queries can be exercised without a live `UIDrawer` (which is what
    /// [`WordPicker::new`] needs and a headless test cannot build).
    fn picker_with(words: &[&str], matched: &[usize]) -> WordPicker {
        let raw = words.join(" ");
        let mut start = 0;
        let words = words
            .iter()
            .map(|w| {
                let text = start..start + w.len();
                // The single space `join` put after this word.
                start = text.end + 1;
                Word {
                    text,
                    label: String::new(),
                    ascii: lower_ascii(w),
                    frame: None,
                }
            })
            .collect();
        WordPicker {
            raw,
            words,
            screen_ratio: 1.0,
            digits: 1,
            matched: matched.to_vec(),
            context_offset: 0,
            in_place: false,
        }
    }

    /// This is what decides whether a word-picking keypress picks a word or waits
    /// for another key, so pin every case: no match, one, and several that are
    /// the same word or not, with and without multi-selection.
    #[rstest]
    // Nothing matched is not a pick, and must not claim to be one by vacuous
    // truth.
    #[case::nothing_matched(&["alpha"], &[], false, None)]
    // A lone match is a pick even for a range: it is the only word either end
    // could mean.
    #[case::one_match(&["alpha"], &[0], false, Some((0, "alpha")))]
    #[case::one_match_of_a_range(&["alpha"], &[0], true, Some((0, "alpha")))]
    // Two copies of one word cannot be told apart, so the pick stays unique —
    // but not for a range, which needs two ends that differ.
    #[case::duplicates_of_one_word(&["alpha", "alpha"], &[0, 1], false, Some((0, "alpha")))]
    #[case::duplicates_of_one_word_of_a_range(&["alpha", "alpha"], &[0, 1], true, None)]
    #[case::two_different_words(&["alpha", "beta"], &[0, 1], false, None)]
    #[case::two_different_words_of_a_range(&["alpha", "beta"], &[0, 1], true, None)]
    #[case::same_word_thrice(&["go", "go", "go"], &[0, 1, 2], false, Some((0, "go")))]
    #[case::same_word_thrice_of_a_range(&["go", "go", "go"], &[0, 1, 2], true, None)]
    // A stale index is skipped rather than panicking, and does not count.
    #[case::stale_index_is_skipped(&["alpha"], &[0, 7], false, Some((0, "alpha")))]
    #[case::every_index_stale(&["alpha"], &[3, 4], false, None)]
    fn picks_only_when_the_matches_leave_no_choice(
        #[case] words: &[&str],
        #[case] matched: &[usize],
        #[case] multi_selection: bool,
        #[case] expected: Option<(usize, &str)>,
    ) {
        let wp = picker_with(words, matched);
        assert_eq!(wp.unambiguous_match(multi_selection), expected);
    }
}
