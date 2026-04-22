use core_foundation::{
    base::{CFRange, CFTypeRef, TCFType},
    string::{CFString, CFStringRef},
};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::NSPasteboard;
use objc2_core_foundation::CGPoint;
use objc2_foundation::{NSArray, NSString};
use regex::Regex;
use std::sync::OnceLock;

unsafe extern "C" {
    /// Show a pop-up dictionary window, same as how Ctrl+Cmd+D works.
    /// Currently not working on macOS 26
    #[allow(dead_code)]
    fn HIDictionaryWindowShow(
        dictionary_ref: CFTypeRef,
        text: CFStringRef,
        range: CFRange,
        font_ref: CFTypeRef,
        origin: CGPoint,
        is_vertical: bool,
        transform: CFTypeRef,
    );

    fn DCSCopyTextDefinition(
        dictionary: CFTypeRef, // Can be NULL
        text_string: CFStringRef,
        range: CFRange,
    ) -> CFStringRef;
}

// FIXME: make it work!
pub fn dictionary_popup(text: &str, point: (f64, f64)) {
    let point = CGPoint::new(point.0, point.1);
    unsafe {
        HIDictionaryWindowShow(
            std::ptr::null(),
            CFString::new(text).as_concrete_TypeRef(),
            CFRange::init(0, text.chars().count() as isize),
            std::ptr::null(),
            point,
            false,
            std::ptr::null(),
        )
    }
}

pub fn dictionary_lookup(text: &str) -> Option<String> {
    let query_str = CFString::new(text);
    println!("Calling DCSCopyTextDefinition with query: {text} ...");

    unsafe {
        let raw_ptr = DCSCopyTextDefinition(
            std::ptr::null(),
            query_str.as_concrete_TypeRef(),
            CFRange::init(0, text.chars().count() as isize),
        );
        if raw_ptr.is_null() {
            return None;
        }

        let raw_string = CFString::wrap_under_create_rule(raw_ptr).to_string();
        let mut formated = String::new();
        let mut char_iter = raw_string.chars().peekable();
        while let Some(char) = char_iter.next() {
            if let Some('.') = char_iter.peek()
                && char.is_ascii_uppercase()
            {
                formated.push('\n');
            } else if char == '▸' {
                formated.push_str("\n    ");
            } else if ('①'..='⑳').contains(&char) {
                formated.push_str("\n  ");
            }
            formated.push(char);
        }

        Some(formated)
    }
}

pub fn text_to_clipboard(text: &str) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let ns_string = NSString::from_str(text);
    let proto_string = ProtocolObject::from_retained(ns_string);
    let objects = NSArray::from_retained_slice(&[proto_string]);
    pb.writeObjects(&objects);
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

pub fn multilingual_split(input: &str) -> Vec<String> {
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
