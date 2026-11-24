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
/// word it picks.
const LABEL_WORD_GAP: &str = "  ";

/// One entry's width in monospace cells, matching the spaces the menu emits.
fn entry_width(word: &str, digits: u32) -> usize {
    word.width() + WORD_LABEL_GAP.len() + digits as usize + LABEL_WORD_GAP.len()
}

/// A token of the picked text: where it sits in [`WordPicker::raw`], its label,
/// its folded search form, its menu width, and its frame once placed.
#[derive(Debug, Clone)]
struct Word {
    text: Range<usize>,
    label: String,
    /// Folded search form, or `None` when the raw slice already folds to itself.
    ascii: Option<String>,
    /// [`entry_width`], which the menu wraps its lines by.
    width: usize,
    /// All or nothing: either every word has one, or the picker draws a menu.
    frame: Option<Frame>,
}

/// A text split into pickable words, drawn as hint boxes over the words when the
/// source can place them on screen, and as a hint-labelled menu otherwise.
pub(crate) struct WordPicker {
    raw: String,
    words: Vec<Word>,
    /// How wide the menu should be, in monospace cells. The words never change,
    /// so neither does this.
    ideal_width: usize,
    pub(crate) digits: u32,
    matched: Vec<usize>,
    /// Where `raw` starts in the source element's text, in UTF-16 units.
    context_offset: usize,
    pub in_place: bool,
}

impl WordPicker {
    /// `element` is the text's source, used only to place the words on screen;
    /// `None`, or one that cannot answer `AXBoundsForRange`, keeps the menu.
    pub(crate) fn new(
        text: String,
        screen_ratio: f64,
        screen_frame: &Frame,
        theme: &GlyphlowTheme,
        hint_keys: &HintKeys,
        drawer: &UIDrawer,
        element: Option<&ElementOfInterest>,
    ) -> Self {
        let ranges = multilingual_split(&text);
        let digits = hint_keys.digits_for_len(ranges.len());
        let mut words = Vec::with_capacity(ranges.len());
        let mut total_width = 0;
        for (i, range) in ranges.into_iter().enumerate() {
            let word = &text[range.clone()];
            let width = entry_width(word, digits);
            total_width += width;
            words.push(Word {
                text: range,
                label: hint_keys.label_for_index(i, Some(digits)),
                ascii: folded_form(word),
                width,
                frame: None,
            });
        }

        let context_offset = element.map_or(0, |element| element.context_offset);
        let in_place =
            element.is_some_and(|element| place_words(element, &text, &mut words, screen_frame));

        let word_picker = Self {
            raw: text,
            words,
            ideal_width: ideal_width(total_width, screen_ratio),
            digits,
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
        self.words.get(idx).map(|word| self.folded(word))
    }

    /// A word's search form: its folded text, or the raw slice when folding is a
    /// no-op.
    fn folded<'a>(&'a self, word: &'a Word) -> &'a str {
        word.ascii
            .as_deref()
            .unwrap_or(&self.raw[word.text.clone()])
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
    /// A box is about `digits * size * 0.8 + 2 * margin` wide — the estimate
    /// collision resolution uses — so inverting that keeps short words from
    /// piling their hints up. The result is clamped to the configured font and
    /// to `MIN_HINT_FONT_SIZE`, so a word with no width still gets a legible font.
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

        // Collapsing the floor keeps both bounds in order when the configured
        // font is already smaller than the floor.
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
        let filtering = !label_prefix.is_empty() || !text_prefix.is_empty();
        let typed = label_prefix.len();
        let text_pattern = search_regex(text_prefix);
        let matches = |word: &Word| {
            filtering
                && word.label.starts_with(label_prefix)
                && text_pattern
                    .as_ref()
                    .is_none_or(|pattern| pattern.is_match(self.folded(word)))
        };

        let matched: Vec<usize> = self
            .words
            .iter()
            .enumerate()
            .filter_map(|(idx, word)| matches(word).then_some(idx))
            .collect();

        // The key inside the header stays a key.
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
            let this_width = word.width;
            // A word wider than the line still starts it.
            if line_width + this_width > self.ideal_width && line_width > 0 {
                buffer.push("\n");
                line_width = 0;
            } else if line_width > 0 {
                // Keep the gap off the ends of a line.
                buffer.push(LABEL_WORD_GAP);
            }
            line_width += this_width;

            if multi_selection_idx == Some(idx) {
                // Its key cannot be pressed again.
                buffer
                    .push_styled(text, MenuStyle::Text)
                    .push(WORD_LABEL_GAP)
                    .push_styled(&word.label, MenuStyle::Dim);
            } else if matched.binary_search(&idx).is_ok() {
                // The typed part cannot be pressed again.
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

    /// The matches that still resolve to a word, in match order. A stale index is
    /// skipped rather than panicking, so it cannot take down an input handler.
    fn live_matches(&self) -> impl Iterator<Item = (usize, &str)> + '_ {
        self.matched.iter().filter_map(|idx| {
            self.words
                .get(*idx)
                .map(|w| (*idx, &self.raw[w.text.clone()]))
        })
    }

    /// The match to pick when the matches leave no choice: the only one, or
    /// several that are the same word while `multi_selection` is off. `None` when
    /// nothing matched or the pick is ambiguous.
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

fn get_url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(URL_PATTERN).unwrap())
}

/// The scripts whose characters each split into a word of their own.
fn is_cjk(c: char) -> bool {
    matches!(
        c,
        '\u{4E00}'..='\u{9FFF}' | '\u{3040}'..='\u{30FF}' | '\u{AC00}'..='\u{D7AF}'
    )
}

/// Converts byte ranges to the UTF-16 ranges `AXBoundsForRange` takes.
///
/// Words arrive front to back, so each conversion only encodes the bytes since
/// the last one: placing every word costs a single pass over the text.
struct Utf16Cursor<'a> {
    text: &'a str,
    /// The byte and UTF-16 offsets of the last position reached.
    byte: usize,
    utf16: usize,
}

impl<'a> Utf16Cursor<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            byte: 0,
            utf16: 0,
        }
    }

    /// The location and length of `range`, both in UTF-16 units. Ranges must
    /// arrive front to back without overlapping, as [`multilingual_split`] yields.
    fn range(&mut self, range: &Range<usize>) -> (usize, usize) {
        self.utf16 += self.text[self.byte..range.start].encode_utf16().count();
        let location = self.utf16;
        let length = self.text[range.clone()].encode_utf16().count();
        self.utf16 += length;
        self.byte = range.end;
        (location, length)
    }
}

/// Place every word on screen, or leave them all unplaced.
///
/// In place needs **all** words to have a real, distinct rectangle: a missing one
/// cannot be picked, and two sharing one would stack their hints.
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

    let Some(ax) = element.element() else {
        log::info!(
            "Word picker: the source is synthetic text with no AX element, so {} word(s) cannot be placed; falling back to the menu.",
            words.len()
        );
        return false;
    };

    let mut frames = Vec::with_capacity(words.len());
    let mut utf16 = Utf16Cursor::new(text);
    for (idx, word) in words.iter().enumerate() {
        let word_text = &text[word.text.clone()];
        let (location, length) = utf16.range(&word.text);
        let Some(frame) = ax.bounds_for_range(element.context_offset + location, length) else {
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

/// The byte ranges of `input` that can be picked, in order.
///
/// Three levels, fallen through only as far as needed: whitespace and script
/// boundaries, then punctuation, then ASCII/non-ASCII — so a URL stays one token
/// while `Hello世界` splits into `Hello`, `世`, `界`.
fn multilingual_split(input: &str) -> Vec<Range<usize>> {
    let mut words = Vec::new();
    for token in token_ranges(input) {
        if get_url_re().is_match(&input[token.clone()]) {
            words.push(token);
        } else {
            push_script_runs(input, token, &mut words);
        }
    }

    // Only a lone block has anything finer to split on.
    if words.len() != 1 {
        return words;
    }
    let words = punctuation_runs(input, words[0].clone());
    if words.len() != 1 {
        return words;
    }
    ascii_runs(input, words[0].clone())
}

/// The whitespace-delimited byte ranges of `input`.
fn token_ranges(input: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    input.split_whitespace().map(|token| {
        // `token` is a slice of `input`, so this difference is its byte offset.
        let start = token.as_ptr() as usize - input.as_ptr() as usize;
        start..start + token.len()
    })
}

/// Push the runs of characters that [`is_cjk`] agrees on in `range`, in order.
fn push_script_runs(input: &str, range: Range<usize>, runs: &mut Vec<Range<usize>>) {
    let mut previous = None;
    for (i, c) in input[range.clone()].char_indices() {
        let start = range.start + i;
        let end = start + c.len_utf8();
        let cjk = is_cjk(c);
        match runs.last_mut() {
            Some(run) if previous == Some(cjk) => run.end = end,
            _ => runs.push(start..end),
        }
        previous = Some(cjk);
    }
}

/// Split `range` at its ASCII punctuation, which is dropped.
fn punctuation_runs(input: &str, range: Range<usize>) -> Vec<Range<usize>> {
    let mut runs = Vec::new();
    let mut piece: Option<Range<usize>> = None;
    for (i, c) in input[range.clone()].char_indices() {
        let start = range.start + i;
        if c.is_ascii_punctuation() {
            runs.extend(piece.take());
        } else {
            match &mut piece {
                Some(piece) => piece.end = start + c.len_utf8(),
                None => piece = Some(start..start + c.len_utf8()),
            }
        }
    }
    runs.extend(piece);
    runs
}

/// Split `range` into ASCII runs, each non-ASCII character a word of its own.
fn ascii_runs(input: &str, range: Range<usize>) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    let mut previous_ascii = false;
    for (i, c) in input[range.clone()].char_indices() {
        let start = range.start + i;
        let end = start + c.len_utf8();
        let ascii = c.is_ascii();
        match runs.last_mut() {
            Some(run) if ascii && previous_ascii => run.end = end,
            _ => runs.push(start..end),
        }
        previous_ascii = ascii;
    }
    runs
}

/// How wide the menu should be, in monospace cells, to look roughly square.
fn ideal_width(total_width: usize, screen_ratio: f64) -> usize {
    (total_width as f64 * screen_ratio * 3.0).sqrt().round() as usize
}

/// The folded form of `word` for searches, or `None` when it is already lowercase
/// ASCII — then callers search the raw slice and save an allocation.
fn folded_form(word: &str) -> Option<String> {
    word.bytes()
        .any(|b| !b.is_ascii() || b.is_ascii_uppercase())
        .then(|| lower_ascii(word))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// Every case pins both ends of every byte range the split reports.
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
    // A lone token reaches the deeper levels the mixed cases never do.
    #[case::single_word_stays_whole("Hello", &["Hello"], &[0])]
    #[case::trailing_punctuation_is_dropped("Wait!", &["Wait"], &[0])]
    #[case::internal_punctuation_splits("hello,world", &["hello", "world"], &[0, 6])]
    #[case::punctuation_only_has_no_words("!!!", &[], &[])]
    #[case::accents_split_off_the_ascii("café", &["caf", "é"], &[0, 3])]
    #[case::accents_survive_on_both_sides("naïve", &["na", "ï", "ve"], &[0, 2, 4])]
    #[case::ascii_between_emoji("a😁b", &["a", "😁", "b"], &[0, 1, 5])]
    // A lone CJK token makes every character a word of its own.
    #[case::lone_cjk_word(
        "こんにちは",
        &["こ", "ん", "に", "ち", "は"],
        &[0, 3, 6, 9, 12]
    )]
    fn splits_on_script_boundaries(
        #[case] input: &str,
        #[case] words: &[&str],
        #[case] offsets: &[usize],
    ) {
        let expected: Vec<Range<usize>> = words
            .iter()
            .zip(offsets)
            .map(|(word, offset)| *offset..*offset + word.len())
            .collect();
        assert_eq!(multilingual_split(input), expected);
    }

    /// Skipping the fold must not change what a word matches.
    #[rstest]
    #[case::already_folded("alpha")]
    #[case::capitalised("Alpha")]
    #[case::accented("café")]
    #[case::emoji("😁")]
    fn folded_form_agrees_with_lower_ascii(#[case] word: &str) {
        let folded = folded_form(word);
        assert_eq!(folded.as_deref().unwrap_or(word), lower_ascii(word));
    }

    /// `AXBoundsForRange` counts UTF-16 units, so this conversion is what keeps
    /// the frames on the right words once anything outside ASCII is involved.
    #[rstest]
    #[case::ascii("hello world", &[(0, 5)], &[(0, 5)])]
    #[case::multibyte_prefix("é alpha", &[(3, 8)], &[(2, 5)])]
    #[case::emoji_counts_as_two_units("😁 hi", &[(5, 7)], &[(3, 2)])]
    // The skipped bytes between words still count, and count once.
    #[case::gaps_count_toward_the_next_word(
        "😁 hi 世",
        &[(0, 4), (5, 7), (8, 11)],
        &[(0, 2), (3, 2), (6, 1)]
    )]
    fn maps_byte_ranges_to_ax_utf16_ranges_in_order(
        #[case] input: &str,
        #[case] ranges: &[(usize, usize)],
        #[case] expected: &[(usize, usize)],
    ) {
        let mut utf16 = Utf16Cursor::new(input);
        let converted: Vec<_> = ranges
            .iter()
            .map(|&(start, end)| utf16.range(&(start..end)))
            .collect();
        assert_eq!(converted, expected);
    }

    /// A range must carry its slice's start along, or the next split would place
    /// its hints at the top of the element.
    #[test]
    fn picked_range_keeps_its_offset_in_the_element() {
        let mut picker = picker_with(&["alpha", "beta", "gamma"], &[]);
        picker.context_offset = 40;

        assert_eq!(picker.word(1).map(|(_, offset)| offset), Some(46));
        assert_eq!(picker.select_range(2, 1), Some(("beta gamma", 46)));
    }

    /// One rectangle for several words means the hints would stack on one spot.
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
    /// queries run without the `UIDrawer` [`WordPicker::new`] would need.
    fn picker_with(words: &[&str], matched: &[usize]) -> WordPicker {
        let raw = words.join(" ");
        let mut start = 0;
        let words: Vec<Word> = words
            .iter()
            .map(|w| {
                let text = start..start + w.len();
                // The single space `join` put after this word.
                start = text.end + 1;
                Word {
                    text,
                    label: String::new(),
                    ascii: folded_form(w),
                    width: entry_width(w, 1),
                    frame: None,
                }
            })
            .collect();
        let total_width = words.iter().map(|word| word.width).sum();
        WordPicker {
            raw,
            words,
            ideal_width: ideal_width(total_width, 1.0),
            digits: 1,
            matched: matched.to_vec(),
            context_offset: 0,
            in_place: false,
        }
    }

    /// This decides whether a keypress picks a word or waits for another key, so
    /// pin every case: none, one, and several, with and without multi-selection.
    #[rstest]
    // Nothing matched is not a pick, even by vacuous truth.
    #[case::nothing_matched(&["alpha"], &[], false, None)]
    // A lone match is a pick even for a range.
    #[case::one_match(&["alpha"], &[0], false, Some((0, "alpha")))]
    #[case::one_match_of_a_range(&["alpha"], &[0], true, Some((0, "alpha")))]
    // Two copies of one word cannot be told apart, but a range still needs two
    // ends that differ.
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
