#[derive(Clone, Debug)]
pub struct UpdaterStatus {
    pub available: bool,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct UpdateCheckResult {
    pub message: String,
}

#[cfg(target_os = "windows")]
pub fn prepare_startup() {
    crate::platform_windows::updates::prepare_startup();
}

#[cfg(not(target_os = "windows"))]
pub fn prepare_startup() {}

#[cfg(target_os = "macos")]
pub fn updater_status() -> UpdaterStatus {
    let status = crate::platform_macos::updates::updater_status();
    UpdaterStatus {
        available: status.available,
        detail: status.detail,
    }
}

#[cfg(target_os = "windows")]
pub fn updater_status() -> UpdaterStatus {
    let status = crate::platform_windows::updates::updater_status();
    UpdaterStatus {
        available: status.available,
        detail: status.detail,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn updater_status() -> UpdaterStatus {
    UpdaterStatus {
        available: false,
        detail: "Automatic updates are not available on this platform.".to_string(),
    }
}

#[cfg(target_os = "macos")]
pub fn start_updater() -> Result<(), String> {
    crate::platform_macos::updates::start_updater()
}

#[cfg(target_os = "windows")]
pub fn start_updater() -> Result<(), String> {
    crate::platform_windows::updates::start_updater()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn start_updater() -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn check_for_updates() -> Result<UpdateCheckResult, String> {
    crate::platform_macos::updates::check_for_updates().map(|_| UpdateCheckResult {
        message: "Opened the Remiss update checker.".to_string(),
    })
}

#[cfg(target_os = "windows")]
pub fn check_for_updates() -> Result<UpdateCheckResult, String> {
    crate::platform_windows::updates::check_for_updates()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn check_for_updates() -> Result<UpdateCheckResult, String> {
    Err("Automatic updates are not available on this platform.".to_string())
}
