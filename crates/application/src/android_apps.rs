use crate::AndroidDevice;
use inspector_domain::{AppDetails, AppTarget, InstalledApp};

#[derive(Clone, Copy, Debug)]
pub enum AppSettingsPage {
    Info,
    Storage,
    Permissions,
    Notifications,
    Overlay,
    AllFiles,
    ExactAlarms,
}

#[derive(Clone, Debug)]
pub enum AppAction {
    Inspect,
    SetPermission { name: String, granted: bool },
    ClearCache,
    ClearStorage,
    ForceStop,
    Launch,
    OpenSettings(AppSettingsPage),
}

#[derive(Clone, Debug)]
pub struct InstalledApps {
    pub user: u32,
    pub apps: Vec<InstalledApp>,
    pub cache_only: bool,
}

#[derive(Clone, Debug)]
pub struct AppResult {
    pub details: Option<AppDetails>,
    pub message: String,
    pub warning: bool,
}

#[derive(Clone, Debug)]
pub enum AndroidAppsEvent {
    Devices {
        request_id: u64,
        devices: Vec<AndroidDevice>,
        suggested: Option<String>,
        error: Option<String>,
    },
    Listed {
        request_id: u64,
        serial: String,
        result: Result<InstalledApps, String>,
    },
    Finished {
        request_id: u64,
        target: AppTarget,
        result: Result<AppResult, String>,
    },
}
