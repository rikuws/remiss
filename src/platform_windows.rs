#[cfg(target_os = "windows")]
pub mod updates {
    use velopack::{sources::HttpSource, UpdateCheck, UpdateManager, VelopackApp};

    use crate::{branding::APP_NAME, platform_updates::UpdateCheckResult};

    const UPDATE_SOURCE_URL: &str = "https://github.com/rikuws/remiss/releases/latest/download/";

    #[derive(Clone, Debug)]
    pub struct UpdaterStatus {
        pub available: bool,
        pub detail: String,
    }

    pub fn prepare_startup() {
        VelopackApp::build().run();
    }

    pub fn updater_status() -> UpdaterStatus {
        match update_manager() {
            Ok(manager) => {
                let install_kind = if manager.get_is_portable() {
                    "Velopack portable package"
                } else {
                    "Velopack install"
                };
                UpdaterStatus {
                    available: true,
                    detail: format!(
                        "{APP_NAME} is running from a {install_kind} and checks GitHub release assets."
                    ),
                }
            }
            Err(error) => UpdaterStatus {
                available: false,
                detail: format!(
                    "Available in Windows builds installed or packaged by Velopack. {error}"
                ),
            },
        }
    }

    pub fn start_updater() -> Result<(), String> {
        Ok(())
    }

    pub fn check_for_updates() -> Result<UpdateCheckResult, String> {
        let manager = update_manager()?;

        if let Some(pending) = manager.get_update_pending_restart() {
            let version = pending.Version.clone();
            manager
                .apply_updates_and_restart(&pending)
                .map_err(|error| format!("Failed to apply the pending Windows update: {error}"))?;
            return Ok(UpdateCheckResult {
                message: format!("Installing {APP_NAME} v{version} and restarting."),
            });
        }

        match manager
            .check_for_updates()
            .map_err(|error| format!("Failed to check for Windows updates: {error}"))?
        {
            UpdateCheck::UpdateAvailable(update) => {
                let version = update.TargetFullRelease.Version.clone();
                manager
                    .download_updates(&update, None)
                    .map_err(|error| format!("Failed to download Windows update: {error}"))?;
                manager
                    .apply_updates_and_restart(&update)
                    .map_err(|error| format!("Failed to apply Windows update: {error}"))?;
                Ok(UpdateCheckResult {
                    message: format!("Installing {APP_NAME} v{version} and restarting."),
                })
            }
            UpdateCheck::NoUpdateAvailable => Ok(UpdateCheckResult {
                message: format!("{APP_NAME} is already up to date."),
            }),
            UpdateCheck::RemoteIsEmpty => Ok(UpdateCheckResult {
                message: "No Windows updates were found in the release feed.".to_string(),
            }),
        }
    }

    fn update_manager() -> Result<UpdateManager, String> {
        let source = HttpSource::new(UPDATE_SOURCE_URL);
        UpdateManager::new(source, None, None).map_err(|error| {
            format!("This build does not include Velopack install metadata: {error}")
        })
    }
}
