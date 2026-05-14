#[cfg(target_os = "macos")]
pub fn show_about_panel() -> Result<(), String> {
    use std::ffi::CString;

    use objc2::{
        msg_send,
        runtime::{AnyClass, AnyObject},
    };

    let class_name = CString::new("NSApplication").unwrap();
    let app_class = AnyClass::get(&class_name)
        .ok_or_else(|| "NSApplication is unavailable on this process.".to_string())?;
    let app: *mut AnyObject = unsafe { msg_send![app_class, sharedApplication] };
    if app.is_null() {
        return Err("NSApplication.sharedApplication returned null.".to_string());
    }

    unsafe {
        let _: () = msg_send![app, orderFrontStandardAboutPanel: Option::<&AnyObject>::None];
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn show_about_panel() -> Result<(), String> {
    Err("The native About panel is only available on macOS.".to_string())
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
pub fn deliver_system_notification(_title: &str, _body: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
pub mod updates {
    use std::{
        cell::RefCell,
        ffi::{CStr, CString},
        fs,
        os::unix::ffi::OsStrExt,
        path::{Path, PathBuf},
    };

    use objc2::{
        msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject, Bool},
    };

    #[derive(Clone, Debug)]
    pub struct UpdaterStatus {
        pub available: bool,
        pub detail: String,
    }

    thread_local! {
        static UPDATER_CONTROLLER: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    }

    pub fn updater_status() -> UpdaterStatus {
        let Some(contents_dir) = app_contents_dir() else {
            return UpdaterStatus {
                available: false,
                detail: "Available in packaged app builds.".to_string(),
            };
        };

        if !sparkle_framework_binary_path(&contents_dir).is_file() {
            UpdaterStatus {
                available: false,
                detail: "Sparkle.framework is not bundled with this app build.".to_string(),
            }
        } else if !bundle_has_sparkle_public_key(&contents_dir) {
            UpdaterStatus {
                available: false,
                detail: "Sparkle.framework is bundled, but this build has no public update key."
                    .to_string(),
            }
        } else {
            UpdaterStatus {
                available: true,
                detail: "Sparkle is bundled and configured to check GitHub releases.".to_string(),
            }
        }
    }

    pub fn start_updater() -> Result<(), String> {
        if app_contents_dir().is_none() {
            return Ok(());
        }

        ensure_update_controller().map(|_| ())
    }

    pub fn check_for_updates() -> Result<(), String> {
        let controller = ensure_update_controller()?;
        unsafe {
            let _: () = msg_send![&*controller, checkForUpdates: Option::<&AnyObject>::None];
        }
        Ok(())
    }

    fn ensure_update_controller() -> Result<Retained<AnyObject>, String> {
        UPDATER_CONTROLLER.with(|controller| {
            if let Some(existing) = controller.borrow().as_ref() {
                return Ok(existing.clone());
            }

            let created = create_update_controller()?;
            *controller.borrow_mut() = Some(created.clone());
            Ok(created)
        })
    }

    fn create_update_controller() -> Result<Retained<AnyObject>, String> {
        load_sparkle_framework()?;

        let class_name = CString::new("SPUStandardUpdaterController").unwrap();
        let controller_class = AnyClass::get(&class_name).ok_or_else(|| {
            "Sparkle loaded, but SPUStandardUpdaterController is unavailable.".to_string()
        })?;

        unsafe {
            let allocated: *mut AnyObject = msg_send![controller_class, alloc];
            let controller: *mut AnyObject = msg_send![
                allocated,
                initWithStartingUpdater: Bool::YES,
                updaterDelegate: Option::<&AnyObject>::None,
                userDriverDelegate: Option::<&AnyObject>::None
            ];
            Retained::from_raw(controller)
                .ok_or_else(|| "Sparkle returned a null updater controller.".to_string())
        }
    }

    fn load_sparkle_framework() -> Result<(), String> {
        let contents_dir = app_contents_dir().ok_or_else(|| {
            "Sparkle updates are only available when Remiss is running from Remiss.app.".to_string()
        })?;
        let framework = sparkle_framework_binary_path(&contents_dir);
        if !framework.is_file() {
            return Err(format!(
                "Sparkle.framework was not found at '{}'.",
                framework.display()
            ));
        }
        if !bundle_has_sparkle_public_key(&contents_dir) {
            return Err(
                "This Remiss build does not include a Sparkle public update key.".to_string(),
            );
        }

        let path = CString::new(framework.as_os_str().as_bytes()).map_err(|_| {
            format!(
                "Sparkle framework path contains a null byte: '{}'.",
                framework.display()
            )
        })?;
        let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
        if handle.is_null() {
            return Err(format!(
                "Failed to load Sparkle.framework: {}",
                dlerror_string()
            ));
        }

        Ok(())
    }

    fn app_contents_dir() -> Option<PathBuf> {
        let executable = std::env::current_exe().ok()?;
        let macos_dir = executable.parent()?;
        if macos_dir.file_name().and_then(|name| name.to_str()) != Some("MacOS") {
            return None;
        }

        let contents_dir = macos_dir.parent()?;
        if contents_dir.file_name().and_then(|name| name.to_str()) != Some("Contents") {
            return None;
        }

        let app_dir = contents_dir.parent()?;
        if app_dir.extension().and_then(|extension| extension.to_str()) != Some("app") {
            return None;
        }

        Some(contents_dir.to_path_buf())
    }

    fn sparkle_framework_binary_path(contents_dir: &Path) -> PathBuf {
        contents_dir
            .join("Frameworks")
            .join("Sparkle.framework")
            .join("Sparkle")
    }

    fn bundle_has_sparkle_public_key(contents_dir: &Path) -> bool {
        fs::read_to_string(contents_dir.join("Info.plist"))
            .map(|plist| plist.contains("<key>SUPublicEDKey</key>"))
            .unwrap_or(false)
    }

    fn dlerror_string() -> String {
        let error = unsafe { libc::dlerror() };
        if error.is_null() {
            return "unknown dynamic loader error".to_string();
        }

        unsafe { CStr::from_ptr(error).to_string_lossy().into_owned() }
    }
}

#[cfg(not(target_os = "macos"))]
pub mod updates {
    #[derive(Clone, Debug)]
    pub struct UpdaterStatus {
        pub available: bool,
        pub detail: String,
    }

    pub fn updater_status() -> UpdaterStatus {
        UpdaterStatus {
            available: false,
            detail: "Automatic updates are only available on macOS.".to_string(),
        }
    }

    pub fn start_updater() -> Result<(), String> {
        Ok(())
    }

    pub fn check_for_updates() -> Result<(), String> {
        Err("Automatic updates are only available on macOS.".to_string())
    }
}
