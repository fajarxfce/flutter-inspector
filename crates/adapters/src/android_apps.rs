use crate::android_shell::AndroidShell;
use inspector_application::{AppAction, AppResult, AppSettingsPage, InstalledApps};
use inspector_domain::{AppDetails, AppPermission, AppTarget, InstalledApp, valid_android_name};
use std::{collections::BTreeMap, path::Path};

#[derive(Default)]
struct PermissionInfo {
    label: String,
    group: String,
    runtime: bool,
}

fn permission_group(name: &str) -> &'static str {
    if name.contains("LOCATION") {
        "Location"
    } else if name.contains("CAMERA") {
        "Camera"
    } else if name.contains("AUDIO") || name.contains("MICROPHONE") {
        "Microphone / audio"
    } else if name.contains("NOTIFICATION") {
        "Notifications"
    } else if name.contains("STORAGE") || name.contains("MEDIA") {
        "Storage / media"
    } else if name.contains("CONTACT") || name.contains("ACCOUNT") {
        "Contacts / accounts"
    } else if name.contains("PHONE") || name.contains("CALL") || name.contains("SMS") {
        "Phone / messages"
    } else if name.contains("CALENDAR") {
        "Calendar"
    } else if name.contains("SENSOR")
        || name.contains("health.")
        || name.contains("ACTIVITY_RECOGNITION")
    {
        "Sensors / health"
    } else {
        "Other"
    }
}

fn permission_catalog(text: &str) -> BTreeMap<String, PermissionInfo> {
    let mut result = BTreeMap::<String, PermissionInfo>::new();
    let mut name = None;
    for line in text.lines().map(str::trim) {
        if let Some(id) = line
            .strip_prefix("+ permission:")
            .or_else(|| line.strip_prefix("permission:"))
        {
            name = valid_android_name(id).then(|| id.to_owned());
            if let Some(id) = &name {
                result.insert(
                    id.clone(),
                    PermissionInfo {
                        group: permission_group(id).into(),
                        ..Default::default()
                    },
                );
            }
        } else if line.starts_with("+ group:") {
            name = None;
        } else if let Some(info) = name.as_ref().and_then(|id| result.get_mut(id)) {
            if let Some(label) = line.strip_prefix("label:").filter(|s| *s != "null") {
                info.label = label.chars().take(200).collect();
            }
            if let Some(level) = line.strip_prefix("protectionLevel:") {
                info.runtime = level.split('|').any(|part| part == "dangerous");
            }
        }
    }
    result
}

pub fn parse_packages(text: &str, system: bool) -> Result<Vec<InstalledApp>, String> {
    let mut apps = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let name = line
            .trim()
            .strip_prefix("package:")
            .ok_or("Unexpected package-list output")?;
        if !valid_android_name(name) {
            return Err("Invalid package in Android's response".into());
        }
        apps.insert(
            name.to_owned(),
            InstalledApp {
                package: name.into(),
                system,
            },
        );
    }
    Ok(apps.into_values().collect())
}

pub async fn list(path: &Path, serial: &str) -> Result<InstalledApps, String> {
    let adb = AndroidShell { path, serial };
    let user: u32 = adb
        .run(&["am", "get-current-user"], 5)
        .await?
        .parse()
        .map_err(|_| "Cannot determine the current Android user")?;
    let user_arg = user.to_string();
    let installed_args = ["pm", "list", "packages", "-3", "--user", &user_arg];
    let system_args = ["pm", "list", "packages", "-s", "--user", &user_arg];
    let (installed, system, help) = tokio::join!(
        adb.run(&installed_args, 8),
        adb.run(&system_args, 8),
        adb.run(&["pm", "help"], 5)
    );
    let mut apps = parse_packages(&installed?, false)?;
    apps.extend(parse_packages(&system?, true)?);
    apps.sort_by(|a, b| a.package.cmp(&b.package));
    apps.dedup_by(|a, b| a.package == b.package);
    Ok(InstalledApps {
        user,
        apps,
        cache_only: help.is_ok_and(|help| help.contains("--cache-only")),
    })
}

fn parse_details(
    text: &str,
    target: &AppTarget,
    catalog: &BTreeMap<String, PermissionInfo>,
) -> Result<AppDetails, String> {
    let marker = format!("Package [{}] (", target.package);
    let lines: Vec<_> = text
        .lines()
        .skip_while(|l| !l.trim().starts_with(&marker))
        .collect();
    if lines.is_empty() {
        return Err("App is no longer installed. Refresh the app list.".into());
    }
    let mut details = AppDetails {
        package: target.package.clone(),
        version: String::new(),
        target_sdk: String::new(),
        system: false,
        debuggable: false,
        stopped: false,
        permissions: vec![],
        notes: vec![],
    };
    let mut permissions = BTreeMap::<String, AppPermission>::new();
    let mut section = "";
    let mut user = None;
    let mut installed = false;
    for line in lines.into_iter().skip(1) {
        let trimmed = line.trim();
        if !line.starts_with(' ') && !trimmed.is_empty() || trimmed.starts_with("Package [") {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("versionName=") {
            details.version = rest.chars().take(100).collect();
        }
        if let Some(rest) = trimmed
            .split_whitespace()
            .find_map(|s| s.strip_prefix("targetSdk="))
        {
            details.target_sdk = rest.into();
        }
        if trimmed.starts_with("pkgFlags=[") || trimmed.starts_with("flags=[") {
            details.system |= trimmed
                .split(|c: char| c.is_whitespace() || c == '[' || c == ']')
                .any(|v| v == "SYSTEM");
            details.debuggable |= trimmed.contains("DEBUGGABLE");
        }
        if let Some(rest) = trimmed.strip_prefix("User ") {
            user = rest
                .split_once(':')
                .and_then(|(id, _)| id.parse::<u32>().ok());
            section = "";
            if user == Some(target.user) {
                installed = trimmed.split_whitespace().any(|w| w == "installed=true");
                details.stopped = trimmed.split_whitespace().any(|w| w == "stopped=true");
            }
            continue;
        }
        if matches!(
            trimmed,
            "requested permissions:" | "install permissions:" | "runtime permissions:"
        ) {
            section = trimmed;
            continue;
        }
        if trimmed.ends_with(':') {
            section = "";
            continue;
        }
        if section == "requested permissions:" && valid_android_name(trimmed) {
            let info = catalog.get(trimmed);
            permissions
                .entry(trimmed.into())
                .or_insert_with(|| AppPermission {
                    name: trimmed.into(),
                    label: info.map(|i| i.label.clone()).unwrap_or_default(),
                    group: info
                        .map(|i| i.group.clone())
                        .unwrap_or_else(|| permission_group(trimmed).into()),
                    granted: None,
                    runtime: info.is_some_and(|i| i.runtime),
                    flags: vec![],
                });
        } else if (section == "install permissions:" && user.is_none())
            || (section == "runtime permissions:" && user == Some(target.user))
        {
            let Some((name, state)) = trimmed.split_once(':') else {
                continue;
            };
            if let Some(permission) = permissions.get_mut(name) {
                permission.granted = if state.contains("granted=true") {
                    Some(true)
                } else if state.contains("granted=false") {
                    Some(false)
                } else {
                    None
                };
                permission.runtime |= section == "runtime permissions:";
                permission.flags = state
                    .split_once("flags=[")
                    .and_then(|(_, s)| s.split_once(']'))
                    .map(|(flags, _)| {
                        flags
                            .split('|')
                            .map(str::trim)
                            .filter(|f| !f.is_empty())
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
            }
        }
    }
    if !installed {
        return Err(format!(
            "App is not installed for Android user {}. Refresh the list.",
            target.user
        ));
    }
    details.permissions = permissions.into_values().collect();
    details
        .permissions
        .sort_by(|a, b| (&a.group, &a.name).cmp(&(&b.group, &b.name)));
    Ok(details)
}

async fn inspect(adb: &AndroidShell<'_>, target: &AppTarget) -> Result<AppDetails, String> {
    let dump_args = ["dumpsys", "package", &target.package];
    let (dump, catalog) = tokio::join!(
        adb.run(&dump_args, 8),
        adb.run(&["pm", "list", "permissions", "-f"], 8)
    );
    let mut details = parse_details(
        &dump?,
        target,
        &permission_catalog(catalog.as_deref().unwrap_or_default()),
    )?;
    if catalog.is_err() {
        details.notes.push(
            "Permission descriptions unavailable; installed permission states are still shown."
                .into(),
        );
    }
    Ok(details)
}

pub async fn execute(
    path: &Path,
    target: &AppTarget,
    action: AppAction,
) -> Result<AppResult, String> {
    if !valid_android_name(&target.package) {
        return Err("Invalid Android package name".into());
    }
    if let AppAction::SetPermission { name, .. } = &action
        && !valid_android_name(name)
    {
        return Err("Invalid Android permission name".into());
    }
    let adb = AndroidShell {
        path,
        serial: &target.serial,
    };
    let before = inspect(&adb, target).await?;
    let user = target.user.to_string();
    let package = target.package.as_str();
    let message = match &action {
        AppAction::Inspect => {
            return Ok(AppResult {
                details: Some(before),
                message: "App details loaded".into(),
                warning: false,
            });
        }
        AppAction::SetPermission { name, granted } => {
            let permission = before
                .permissions
                .iter()
                .find(|p| &p.name == name)
                .ok_or("This permission is not declared by the selected app")?;
            if !permission.editable() {
                return Err("Android controls this permission; it is not an editable runtime permission. Open app settings for special access.".into());
            }
            adb.run(
                &[
                    "pm",
                    if *granted { "grant" } else { "revoke" },
                    "--user",
                    &user,
                    package,
                    name,
                ],
                8,
            )
            .await?;
            "Permission updated"
        }
        AppAction::ClearCache | AppAction::ClearStorage => {
            let cache = matches!(action, AppAction::ClearCache);
            if cache && !adb.run(&["pm", "help"], 5).await?.contains("--cache-only") {
                return Err("This Android version does not support clearing only this app's cache through ADB. Use Storage settings.".into());
            }
            let args = if cache {
                vec!["pm", "clear", "--user", &user, "--cache-only", package]
            } else {
                vec!["pm", "clear", "--user", &user, package]
            };
            if adb.run(&args, 20).await? != "Success" {
                return Err("Android did not confirm the clear operation. Refresh or check Storage settings before retrying.".into());
            }
            if cache {
                "Android confirmed app cache cleared"
            } else {
                "Android confirmed app storage cleared"
            }
        }
        AppAction::ForceStop => {
            adb.run(&["am", "force-stop", "--user", &user, package], 8)
                .await?;
            "App force-stopped"
        }
        AppAction::Launch => {
            let resolved = adb
                .run(
                    &[
                        "cmd",
                        "package",
                        "resolve-activity",
                        "--brief",
                        "--user",
                        &user,
                        "-a",
                        "android.intent.action.MAIN",
                        "-c",
                        "android.intent.category.LAUNCHER",
                        package,
                    ],
                    8,
                )
                .await?;
            let component = resolved
                .lines()
                .rev()
                .find(|l| l.split_once('/').is_some_and(|(p, _)| p == package))
                .ok_or("This app has no launcher activity")?;
            if !component
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._/$".contains(&c))
            {
                return Err("Invalid launcher activity returned by Android".into());
            }
            adb.run(&["am", "start", "--user", &user, "-n", component], 8)
                .await?;
            "App opened on the device"
        }
        AppAction::OpenSettings(page) => {
            let action = match page {
                AppSettingsPage::Info => "android.settings.APPLICATION_DETAILS_SETTINGS",
                AppSettingsPage::Storage => "android.settings.APPLICATION_DETAILS_SETTINGS",
                AppSettingsPage::Permissions => "android.intent.action.MANAGE_APP_PERMISSIONS",
                AppSettingsPage::Notifications => "android.settings.APP_NOTIFICATION_SETTINGS",
                AppSettingsPage::Overlay => "android.settings.action.MANAGE_OVERLAY_PERMISSION",
                AppSettingsPage::AllFiles => {
                    "android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION"
                }
                AppSettingsPage::ExactAlarms => "android.settings.REQUEST_SCHEDULE_EXACT_ALARM",
            };
            let uri = format!("package:{package}");
            let mut args = vec!["am", "start", "--user", &user, "-a", action];
            if !matches!(
                page,
                AppSettingsPage::Permissions | AppSettingsPage::Notifications
            ) {
                args.extend(["-d", &uri]);
            }
            args.extend([
                "--es",
                "android.intent.extra.PACKAGE_NAME",
                package,
                "--es",
                "android.provider.extra.APP_PACKAGE",
                package,
            ]);
            let result = adb.run(&args, 8).await;
            if result.is_err() {
                adb.run(
                    &[
                        "am",
                        "start",
                        "--user",
                        &user,
                        "-a",
                        "android.settings.APPLICATION_DETAILS_SETTINGS",
                        "-d",
                        &uri,
                    ],
                    8,
                )
                .await?;
                return Ok(AppResult {
                    details: Some(before),
                    message: "Opened App info; choose the relevant settings on the device.".into(),
                    warning: false,
                });
            }
            return Ok(AppResult {
                details: Some(before),
                message: if matches!(page, AppSettingsPage::Storage) { "Opened App info on the device. Choose Storage to see usage or manage storage there." } else { "Opened settings on the device. Refresh details after making changes there." }.into(),
                warning: false,
            });
        }
    };
    // Do not report success based only on pm's exit code: grants can be ignored.
    match inspect(&adb, target).await {
        Ok(after) => {
            if let AppAction::SetPermission { ref name, granted } = action
                && after
                    .permissions
                    .iter()
                    .find(|p| &p.name == name)
                    .and_then(|p| p.granted)
                    != Some(granted)
            {
                return Err("Android did not apply the requested permission state. App policy, target SDK or another required permission may prevent it. Refresh details or open Permissions on device.".into());
            }
            if matches!(action, AppAction::ForceStop) && !after.stopped {
                return Err("The app is running again or Android did not accept force-stop. Refresh details.".into());
            }
            Ok(AppResult {
                details: Some(after),
                message: format!("{message}. App details refreshed."),
                warning: false,
            })
        }
        Err(error) => Ok(AppResult {
            details: None,
            message: format!("{message}, but reading the new state failed: {error}"),
            warning: true,
        }),
    }
}
