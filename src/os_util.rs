use std::path::PathBuf;

use accessibility::{AXUIElement, AXUIElementAttributes};
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
    "Brave Browser Framework.framework", // Brave Browser
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

const APPLE_ALARM_BUNDLE_IDS: [&str; 3] = [
    "com.apple.coreservices.uiagent",
    "com.apple.accessibility.universalAccessAuthWarn",
    "com.apple.CoreLocationAgent",
];

pub fn get_system_alarm_window() -> Option<AXUIElement> {
    for bundle_id in APPLE_ALARM_BUNDLE_IDS {
        if let Ok(app) = AXUIElement::application_with_bundle(bundle_id)
            && let Ok(window) = app.focused_window()
        {
            return Some(window);
        }
    }
    None
}

pub fn get_focused() -> Option<(i32, AXUIElement, bool)> {
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    log::log!(
        log::Level::Trace,
        "Focused app bundle id: {:?}",
        app.bundleIdentifier()
    );

    let pid = app.processIdentifier();
    Some((
        pid,
        AXUIElement::application(pid),
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
