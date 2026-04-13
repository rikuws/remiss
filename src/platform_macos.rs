#[cfg(target_os = "macos")]
pub fn apply_app_icon() {
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    let app = NSApplication::sharedApplication(mtm);
    let data = NSData::with_bytes(include_bytes!("../assets/brand/app-icon.png"));
    let Some(icon) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };

    unsafe {
        app.setApplicationIconImage(Some(&icon));
    }
}

#[cfg(target_os = "macos")]
pub fn deliver_system_notification(title: &str, body: &str) -> Result<(), String> {
    use std::process::Command;

    let escape = |value: &str| value.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        escape(body),
        escape(title)
    );
    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|error| format!("Failed to launch osascript: {error}"))?;

    if output.status.success() {
        return Ok(());
    }

    Err(format!(
        "osascript notification failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

#[cfg(not(target_os = "macos"))]
pub fn apply_app_icon() {}

#[cfg(not(target_os = "macos"))]
pub fn deliver_system_notification(_title: &str, _body: &str) -> Result<(), String> {
    Ok(())
}
