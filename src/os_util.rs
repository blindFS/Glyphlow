use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSWorkspace};
use objc2_foundation::{NSArray, NSString};
use rdev::Key;

pub fn get_focused_pid() -> Option<i32> {
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    Some(app.processIdentifier())
}

pub fn check_accessibility_permissions() -> bool {
    // Using a raw system check (AXIsProcessTrusted)
    // In a real app, you'd use `AXIsProcessTrustedWithOptions` to show the prompt
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;

    }

    unsafe { AXIsProcessTrusted() }
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
