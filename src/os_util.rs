use objc2_app_kit::NSWorkspace;

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
