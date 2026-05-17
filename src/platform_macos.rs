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
pub fn prepare_system_notifications() -> Result<(), String> {
    system_notifications::prepare()
}

#[cfg(not(target_os = "macos"))]
pub fn prepare_system_notifications() -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn deliver_system_notification(title: &str, body: &str) -> Result<(), String> {
    system_notifications::deliver(title, body)
}

#[cfg(not(target_os = "macos"))]
pub fn deliver_system_notification(_title: &str, _body: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn install_deep_link_url_event_handler(
    dispatcher: crate::deep_link::DeepLinkDispatcher,
) -> Result<(), String> {
    deep_link_url_events::install(dispatcher)
}

#[cfg(not(target_os = "macos"))]
pub fn install_deep_link_url_event_handler(
    _dispatcher: crate::deep_link::DeepLinkDispatcher,
) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
mod deep_link_url_events {
    use std::{cell::RefCell, ffi::CString};

    use objc2::{
        define_class, msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject},
        sel, ClassType,
    };
    use objc2_foundation::{NSObject, NSObjectProtocol, NSString};

    use crate::deep_link::DeepLinkDispatcher;

    const KEY_DIRECT_OBJECT: u32 = four_char_code(*b"----");
    const K_AE_GET_URL: u32 = four_char_code(*b"GURL");
    const K_INTERNET_EVENT_CLASS: u32 = four_char_code(*b"GURL");

    thread_local! {
        static DISPATCHER: RefCell<Option<DeepLinkDispatcher>> = const { RefCell::new(None) };
        static HANDLER: RefCell<Option<Retained<RemissUrlEventHandler>>> = const { RefCell::new(None) };
    }

    define_class!(
        // SAFETY:
        // - The superclass NSObject has no subclassing requirements.
        // - The handler stores no Rust ivars and is retained for the full
        //   process lifetime after registration.
        #[unsafe(super(NSObject))]
        struct RemissUrlEventHandler;

        // SAFETY: NSObjectProtocol has no additional safety requirements.
        unsafe impl NSObjectProtocol for RemissUrlEventHandler {}

        impl RemissUrlEventHandler {
            #[unsafe(method(handleGetURLEvent:withReplyEvent:))]
            fn handle_get_url_event(
                &self,
                event: &AnyObject,
                _reply_event: &AnyObject,
            ) {
                let direct_object: *mut AnyObject =
                    unsafe { msg_send![event, paramDescriptorForKeyword: KEY_DIRECT_OBJECT] };
                if direct_object.is_null() {
                    eprintln!("Ignoring Remiss URL event without a direct URL object.");
                    return;
                }
                let url: *mut NSString = unsafe { msg_send![direct_object, stringValue] };
                if url.is_null() {
                    eprintln!("Ignoring Remiss URL event with a non-string direct object.");
                    return;
                }
                let url = unsafe { &*url }.to_string();

                DISPATCHER.with(|dispatcher| {
                    if let Some(dispatcher) = dispatcher.borrow().as_ref() {
                        dispatcher.receive_urls(vec![url]);
                    } else {
                        eprintln!("Ignoring Remiss URL event before dispatcher installation.");
                    }
                });
            }
        }
    );

    impl RemissUrlEventHandler {
        fn new() -> Retained<Self> {
            unsafe { msg_send![Self::class(), new] }
        }
    }

    pub fn install(dispatcher: DeepLinkDispatcher) -> Result<(), String> {
        DISPATCHER.with(|slot| {
            *slot.borrow_mut() = Some(dispatcher);
        });

        HANDLER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = Some(RemissUrlEventHandler::new());
            }

            let handler = slot
                .as_ref()
                .expect("Remiss URL event handler should be initialized");
            let class_name = CString::new("NSAppleEventManager").unwrap();
            let Some(manager_class) = AnyClass::get(&class_name) else {
                return Err("NSAppleEventManager is unavailable on this process.".to_string());
            };
            let manager: *mut AnyObject =
                unsafe { msg_send![manager_class, sharedAppleEventManager] };
            if manager.is_null() {
                return Err(
                    "NSAppleEventManager.sharedAppleEventManager returned null.".to_string()
                );
            }

            unsafe {
                let _: () = msg_send![
                    manager,
                    setEventHandler: handler.as_super().as_super(),
                    andSelector: sel!(handleGetURLEvent:withReplyEvent:),
                    forEventClass: K_INTERNET_EVENT_CLASS,
                    andEventID: K_AE_GET_URL
                ];
            }
            Ok(())
        })
    }

    const fn four_char_code(bytes: [u8; 4]) -> u32 {
        ((bytes[0] as u32) << 24)
            | ((bytes[1] as u32) << 16)
            | ((bytes[2] as u32) << 8)
            | bytes[3] as u32
    }
}

#[cfg(target_os = "macos")]
mod system_notifications {
    use std::{
        ffi::{c_void, CString},
        path::PathBuf,
        ptr,
        sync::atomic::{AtomicBool, AtomicU64, Ordering},
    };

    use block2::RcBlock;
    use objc2::{
        define_class, msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject, Bool, ProtocolObject},
        ClassType,
    };
    use objc2_foundation::{NSError, NSObject, NSObjectProtocol, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
        UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
        UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };

    static DELEGATE_INSTALLED: AtomicBool = AtomicBool::new(false);
    static AUTHORIZATION_REQUESTED: AtomicBool = AtomicBool::new(false);
    static NOTIFICATION_ID: AtomicU64 = AtomicU64::new(1);

    define_class!(
        // SAFETY:
        // - The superclass NSObject has no subclassing requirements.
        // - The delegate stores no Rust ivars and is intentionally retained
        //   for the full process lifetime after registration.
        #[unsafe(super(NSObject))]
        struct RemissNotificationDelegate;

        // SAFETY: NSObjectProtocol has no additional safety requirements.
        unsafe impl NSObjectProtocol for RemissNotificationDelegate {}

        // SAFETY: UNUserNotificationCenterDelegate has no additional safety
        // requirements. Both methods use the exact Objective-C signatures.
        unsafe impl UNUserNotificationCenterDelegate for RemissNotificationDelegate {
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present_notification(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                completion_handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                completion_handler.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List,));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive_notification_response(
                &self,
                _center: &UNUserNotificationCenter,
                _response: &UNNotificationResponse,
                completion_handler: &block2::DynBlock<dyn Fn()>,
            ) {
                activate_remiss_application();
                completion_handler.call(());
            }
        }
    );

    impl RemissNotificationDelegate {
        fn new() -> Retained<Self> {
            unsafe { msg_send![Self::class(), new] }
        }
    }

    pub fn prepare() -> Result<(), String> {
        if app_contents_dir().is_none() {
            return Ok(());
        }

        ensure_setup();
        Ok(())
    }

    pub fn deliver(title: &str, body: &str) -> Result<(), String> {
        if app_contents_dir().is_none() {
            return Err("Native Remiss notifications require running from Remiss.app.".to_string());
        }

        let center = ensure_setup();
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));

        let notification_id = NOTIFICATION_ID.fetch_add(1, Ordering::Relaxed);
        let identifier = NSString::from_str(&format!("dev.rikuwikman.remiss.{notification_id}"));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &identifier,
            content.as_super(),
            None,
        );
        let title_for_log = title.to_string();
        let completion = RcBlock::new(move |error: *mut NSError| {
            if let Some(message) = ns_error_message(error) {
                eprintln!("Failed to deliver Remiss notification '{title_for_log}': {message}");
            }
        });
        center.addNotificationRequest_withCompletionHandler(&request, Some(&completion));

        Ok(())
    }

    fn ensure_setup() -> Retained<UNUserNotificationCenter> {
        let center = UNUserNotificationCenter::currentNotificationCenter();
        install_delegate(&center);
        request_authorization(&center);
        center
    }

    fn install_delegate(center: &UNUserNotificationCenter) {
        if DELEGATE_INSTALLED.swap(true, Ordering::SeqCst) {
            return;
        }

        let delegate = RemissNotificationDelegate::new();
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        let _ = Retained::into_raw(delegate);
    }

    fn request_authorization(center: &UNUserNotificationCenter) {
        if AUTHORIZATION_REQUESTED.swap(true, Ordering::SeqCst) {
            return;
        }

        let completion = RcBlock::new(|granted: Bool, error: *mut NSError| {
            if let Some(message) = ns_error_message(error) {
                eprintln!("Remiss notification authorization failed: {message}");
            }

            if !granted.as_bool() {
                eprintln!("Remiss notifications are disabled in System Settings.");
            }
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert,
            &completion,
        );
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

    fn ns_error_message(error: *mut NSError) -> Option<String> {
        if error.is_null() {
            return None;
        }

        let description: *mut NSString = unsafe { msg_send![error, localizedDescription] };
        if description.is_null() {
            None
        } else {
            Some(unsafe { &*description }.to_string())
        }
    }

    #[repr(C)]
    struct DispatchQueue {
        _private: [u8; 0],
    }

    extern "C" {
        static _dispatch_main_q: DispatchQueue;
        fn dispatch_async_f(
            queue: *const DispatchQueue,
            context: *mut c_void,
            work: extern "C" fn(*mut c_void),
        );
    }

    fn activate_remiss_application() {
        unsafe {
            dispatch_async_f(
                ptr::addr_of!(_dispatch_main_q),
                ptr::null_mut(),
                activate_remiss_application_now,
            );
        }
    }

    extern "C" fn activate_remiss_application_now(_context: *mut c_void) {
        let class_name = CString::new("NSApplication").unwrap();
        let Some(app_class) = AnyClass::get(&class_name) else {
            return;
        };
        let app: *mut AnyObject = unsafe { msg_send![app_class, sharedApplication] };
        if app.is_null() {
            return;
        }

        unsafe {
            let _: () = msg_send![app, unhide: Option::<&AnyObject>::None];
            let _: () = msg_send![app, activateIgnoringOtherApps: Bool::YES];
        }
    }
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
