use std::path::PathBuf;

use accessibility_sys::{AXIsProcessTrustedWithOptions, kAXTrustedCheckOptionPrompt};
use core_foundation::{
    base::TCFType, boolean::CFBoolean, dictionary::CFDictionary, string::CFString,
};
use objc2::rc::Retained;
use objc2_app_kit::{NSRunningApplication, NSWorkspace};

const ELECTRON_FRAMEWORKS: [&str; 6] = [
    "Electron Framework.framework", // Standard Electron (VS Code, Slack, Discord)
    "Google Chrome Framework.framework", // Google Chrome
    "Chromium Framework.framework", // Unbranded Chromium
    "Microsoft Edge Framework.framework", // Microsoft Edge
    "Brave Framework.framework",    // Brave Browser
    "libAvaloniaNative.dylib",      // HACK: Avalonia
];

fn check_is_electron_app(app: &Retained<NSRunningApplication>) -> Option<bool> {
    let boundle_path = PathBuf::from(app.bundleURL()?.path()?.to_string());
    let framwork_path = boundle_path.join("Contents").join("Frameworks");
    for framework in ELECTRON_FRAMEWORKS {
        if framwork_path.join(framework).exists() {
            return Some(true);
        }
    }
    Some(false)
}

pub fn get_focused_pid() -> Option<(i32, bool)> {
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;

    Some((
        app.processIdentifier(),
        check_is_electron_app(&app).unwrap_or_default(),
    ))
}

pub fn check_accessibility_permissions() -> bool {
    unsafe {
        let key = CFString::wrap_under_create_rule(kAXTrustedCheckOptionPrompt);
        let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
    }
}
