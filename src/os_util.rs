use accessibility_sys::{AXIsProcessTrustedWithOptions, kAXTrustedCheckOptionPrompt};
use core_foundation::{
    base::TCFType, boolean::CFBoolean, dictionary::CFDictionary, string::CFString,
};
use objc2_app_kit::NSWorkspace;

pub fn get_focused_pid() -> Option<i32> {
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    Some(app.processIdentifier())
}

pub fn check_accessibility_permissions() -> bool {
    unsafe {
        let key = CFString::wrap_under_create_rule(kAXTrustedCheckOptionPrompt);
        let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
    }
}
