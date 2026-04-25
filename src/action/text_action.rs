use core_foundation::{
    base::{CFRange, CFTypeRef, TCFType},
    string::{CFString, CFStringRef},
};
use objc2::{rc::autoreleasepool, runtime::ProtocolObject};
use objc2_app_kit::NSPasteboard;
use objc2_core_foundation::CGPoint;
use objc2_foundation::{NSArray, NSString};

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

// TODO: Better format
// https://github.com/NSHipster/articles/blob/master/2014-03-10-dictionary-services.md
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

        let cf_string = CFString::wrap_under_create_rule(raw_ptr);
        let raw_string = cf_string.to_string();

        // Format into a paragraph
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
    autoreleasepool(|_| {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let ns_string = NSString::from_str(text);
        let proto_string = ProtocolObject::from_retained(ns_string);
        let objects = NSArray::from_retained_slice(&[proto_string]);
        pb.writeObjects(&objects);
    })
}
