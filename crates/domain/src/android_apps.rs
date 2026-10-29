#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledApp {
    pub package: String,
    pub system: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppTarget {
    pub serial: String,
    pub user: u32,
    pub package: String,
}

#[derive(Clone, Debug)]
pub struct AppPermission {
    pub name: String,
    pub label: String,
    pub group: String,
    pub granted: Option<bool>,
    pub runtime: bool,
    pub flags: Vec<String>,
}
impl AppPermission {
    pub fn editable(&self) -> bool {
        self.runtime
            && self.granted.is_some()
            && !self
                .flags
                .iter()
                .any(|flag| matches!(flag.as_str(), "SYSTEM_FIXED" | "POLICY_FIXED"))
    }
}

#[derive(Clone, Debug)]
pub struct AppDetails {
    pub package: String,
    pub version: String,
    pub target_sdk: String,
    pub system: bool,
    pub debuggable: bool,
    pub stopped: bool,
    pub permissions: Vec<AppPermission>,
    pub notes: Vec<String>,
}

/// Package/permission identifiers passed to Android's shell must be plain names.
pub fn valid_android_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name.as_bytes()[0].is_ascii_alphabetic()
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.')
}
