use std::path::PathBuf;

use accessibility::{AXUIElement, AXUIElementAttributes};
use accessibility_sys::{
    AXIsProcessTrustedWithOptions, kAXPopoverRole, kAXTrustedCheckOptionPrompt,
};
use core_foundation::{
    base::TCFType, boolean::CFBoolean, dictionary::CFDictionary, string::CFString,
};
use objc2::rc::Retained;
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_core_foundation::CGSize;

use crate::{ax_element::GetAttribute, util::Frame};

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
    Some(
        ELECTRON_FRAMEWORKS
            .iter()
            .any(|framework| framwork_path.join(framework).exists()),
    )
}

const APPLE_ALARM_BUNDLE_IDS: [&str; 3] = [
    "com.apple.coreservices.uiagent",
    "com.apple.accessibility.universalAccessAuthWarn",
    "com.apple.CoreLocationAgent",
];

const APPLE_NOTIFICATION_CENTER_BUNDLE_ID: &str = "com.apple.notificationcenterui";

pub struct AppWindowInfo {
    pub window: AXUIElement,
    pub bundle_id: String,
    pid: i32,
    pub is_electron: bool,
    pub frame: Frame,
}

impl AppWindowInfo {
    pub fn default(screen_size: CGSize) -> Self {
        Self {
            window: AXUIElement::system_wide(),
            bundle_id: String::new(),
            pid: -1,
            is_electron: false,
            frame: Frame::from_origion(screen_size),
        }
    }

    fn new_alarm_window(window: AXUIElement, frame: Frame, bundle_id: String) -> Self {
        Self {
            window,
            frame,
            is_electron: false,
            pid: -1,
            bundle_id,
        }
    }
}

fn get_system_alarm_window(screen_frame: Frame) -> Option<AppWindowInfo> {
    APPLE_ALARM_BUNDLE_IDS
        .iter()
        .find_map(|bundle_id| {
            AXUIElement::application_with_bundle(bundle_id)
                .and_then(|app| {
                    let element = app.focused_window()?;
                    let frame = element.get_frame(screen_frame);
                    Ok(AppWindowInfo::new_alarm_window(
                        element,
                        frame,
                        bundle_id.to_string(),
                    ))
                })
                .ok()
        })
        .or_else(|| {
            let app =
                AXUIElement::application_with_bundle(APPLE_NOTIFICATION_CENTER_BUNDLE_ID).ok()?;
            // Notification banners are not focused, so we need to get the first window
            let windows = app.windows().ok()?;
            let first_win = windows.iter().next().as_deref().cloned()?;
            let frame = first_win.get_frame(screen_frame);
            Some(AppWindowInfo::new_alarm_window(
                first_win,
                frame,
                APPLE_NOTIFICATION_CENTER_BUNDLE_ID.to_string(),
            ))
        })
}

/// Get currently focused window element,
/// along with information about its app
pub fn get_focused_window(
    screen_frame: Frame,
    last_info: &AppWindowInfo,
    electron_init_wait_ms: u64,
) -> Option<AppWindowInfo> {
    // NOTE: prioritize system alarms
    if let Some(res) = get_system_alarm_window(screen_frame) {
        return Some(res);
    }

    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    let bundle_id = app
        .bundleIdentifier()
        .map(|s| s.to_string())
        .unwrap_or_default();
    log::log!(log::Level::Trace, "Focused app bundle id: {:?}", bundle_id);

    let pid = app.processIdentifier();
    let is_electron = check_is_electron_app(&app).unwrap_or_default();
    let app_element = AXUIElement::application(pid);
    let new_pid = pid != last_info.pid;

    // HACK: need this to bootstrap UI tree generation for some electron apps, e.g. Discord
    if is_electron && new_pid {
        let _ = app_element.role();
        std::thread::sleep(std::time::Duration::from_millis(electron_init_wait_ms));
    }

    // NOTE: prioritize popover windows, e.g. Apple Music search
    let window = app_element
        .windows()
        .and_then(|wins| {
            wins.iter()
                .find(|w| w.role().is_ok_and(|r| r == kAXPopoverRole))
                .map(|w_ref| w_ref.clone())
                .ok_or(accessibility::Error::NotFound)
        })
        .or_else(|_| app_element.focused_window());

    let window = window.unwrap_or(app_element);
    let frame = window.get_frame(screen_frame);

    Some(AppWindowInfo {
        window,
        pid,
        is_electron,
        bundle_id,
        frame,
    })
}

pub fn check_accessibility_permissions() -> bool {
    unsafe {
        let key = CFString::wrap_under_create_rule(kAXTrustedCheckOptionPrompt);
        let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
    }
}
