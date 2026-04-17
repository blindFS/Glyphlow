use core_foundation::{
    base::{CFRange, CFTypeRef, TCFType},
    string::{CFString, CFStringRef},
};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSWorkspace};
use objc2_core_foundation::CGPoint;
use objc2_foundation::{NSArray, NSString};
use rdev::Key;

pub fn get_focused_pid() -> Option<i32> {
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    Some(app.processIdentifier())
}

// Using a raw system check (AXIsProcessTrusted)
// In a real app, you'd use `AXIsProcessTrustedWithOptions` to show the prompt
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

pub fn check_accessibility_permissions() -> bool {
    unsafe { AXIsProcessTrusted() }
}

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
    unsafe {
        println!("Calling DCSCopyTextDefinition with query: {text} ...");
        let raw_ptr = DCSCopyTextDefinition(
            std::ptr::null(),
            CFString::new(text).as_concrete_TypeRef(),
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
            } else if ('①'..='㊿').contains(&char) {
                formated.push_str("\n  ");
            }
            formated.push(char);
        }

        println!("{formated}");
        Some(formated)
    }
}

pub fn copy_to_clipboard(text: &str) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let ns_string = NSString::from_str(text);
    let proto_string = ProtocolObject::from_retained(ns_string);
    let objects = NSArray::from_retained_slice(&[proto_string]);
    pb.writeObjects(&objects);
}

pub trait AlphabeticKey {
    fn to_char(&self) -> char;
}

impl AlphabeticKey for Key {
    fn to_char(&self) -> char {
        match self {
            Key::KeyA => 'A',
            Key::KeyB => 'B',
            Key::KeyC => 'C',
            Key::KeyD => 'D',
            Key::KeyE => 'E',
            Key::KeyF => 'F',
            Key::KeyG => 'G',
            Key::KeyH => 'H',
            Key::KeyI => 'I',
            Key::KeyJ => 'J',
            Key::KeyK => 'K',
            Key::KeyL => 'L',
            Key::KeyM => 'M',
            Key::KeyN => 'N',
            Key::KeyO => 'O',
            Key::KeyP => 'P',
            Key::KeyQ => 'Q',
            Key::KeyR => 'R',
            Key::KeyT => 'T',
            Key::KeyU => 'U',
            Key::KeyV => 'V',
            Key::KeyW => 'W',
            Key::KeyX => 'X',
            Key::KeyY => 'Y',
            Key::KeyZ => 'Z',
            Key::Backspace | Key::Delete => '-',
            _ => ' ',
        }
    }
}
