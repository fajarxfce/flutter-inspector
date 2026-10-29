use crate::app::InspectorApp;
use eframe::egui::{self, Color32, RichText};
use inspector_application::{AndroidAppsEvent, AndroidDevice, AppAction, AppSettingsPage, Command};
use inspector_domain::{AppDetails, AppTarget, InstalledApp};

#[derive(Clone, Copy, Default, PartialEq)]
enum Filter {
    #[default]
    Installed,
    System,
    All,
}
#[derive(Clone, Copy, Default, PartialEq)]
enum Section {
    #[default]
    Permissions,
    Storage,
    Actions,
}
struct Confirmation {
    target: AppTarget,
    action: AppAction,
    title: &'static str,
    message: &'static str,
}

#[derive(Default)]
pub struct AndroidAppsUi {
    devices: Vec<AndroidDevice>,
    serial: String,
    devices_request: u64,
    devices_loading: bool,
    list_request: u64,
    listing: bool,
    apps: Vec<InstalledApp>,
    user: Option<u32>,
    cache_only: bool,
    request_id: u64,
    selected: String,
    details: Option<AppDetails>,
    loading: bool,
    mutating: bool,
    filter: Filter,
    search: String,
    permission_search: String,
    section: Section,
    message: String,
    error: bool,
    confirmation: Option<Confirmation>,
}
impl AndroidAppsUi {
    fn target(&self) -> Option<AppTarget> {
        if self.serial.is_empty() || self.selected.is_empty() {
            return None;
        }
        Some(AppTarget {
            serial: self.serial.clone(),
            user: self.user?,
            package: self.selected.clone(),
        })
    }
    fn clear_selection(&mut self) {
        self.request_id += 1;
        self.selected.clear();
        self.details = None;
        self.loading = false;
        self.confirmation = None;
    }
    fn select_device(&mut self, serial: String) {
        self.clear_selection();
        self.list_request += 1;
        self.serial = serial;
        self.apps.clear();
        self.user = None;
        self.cache_only = false;
        self.listing = false;
        self.message.clear();
        self.error = false;
    }
    fn finished(
        &mut self,
        request_id: u64,
        target: AppTarget,
        result: Result<inspector_application::AppResult, String>,
    ) {
        if request_id != self.request_id || self.target().as_ref() != Some(&target) {
            return;
        }
        self.loading = false;
        self.mutating = false;
        match result {
            Ok(result) => {
                self.details = result.details;
                self.message = result.message;
                self.error = result.warning;
            }
            Err(error) => {
                self.details = None;
                self.message = error;
                self.error = true;
            }
        }
    }
}

impl InspectorApp {
    pub fn android_apps_event(&mut self, event: AndroidAppsEvent) {
        match event {
            AndroidAppsEvent::Devices {
                request_id,
                devices,
                suggested,
                error,
            } if request_id == self.android_apps.devices_request => {
                let apps = &mut self.android_apps;
                apps.devices_loading = false;
                let serial = if devices
                    .iter()
                    .any(|d| d.serial == apps.serial && d.state == "device")
                    || apps.mutating
                {
                    apps.serial.clone()
                } else {
                    devices
                        .iter()
                        .find(|d| d.state == "device" && Some(&d.serial) == suggested.as_ref())
                        .or_else(|| devices.iter().find(|d| d.state == "device"))
                        .map(|d| d.serial.clone())
                        .unwrap_or_default()
                };
                apps.devices = devices;
                if serial != apps.serial {
                    apps.select_device(serial);
                    self.list_android_apps();
                }
                if let Some(error) = error {
                    self.android_apps.message = error;
                    self.android_apps.error = true;
                }
            }
            AndroidAppsEvent::Listed {
                request_id,
                serial,
                result,
            } if request_id == self.android_apps.list_request
                && serial == self.android_apps.serial =>
            {
                let apps = &mut self.android_apps;
                apps.listing = false;
                match result {
                    Ok(result) => {
                        if apps.user != Some(result.user)
                            || !result.apps.iter().any(|a| a.package == apps.selected)
                        {
                            apps.clear_selection();
                        }
                        apps.user = Some(result.user);
                        apps.apps = result.apps;
                        apps.cache_only = result.cache_only;
                        apps.message =
                            format!("{} apps · Android user {}", apps.apps.len(), result.user);
                        apps.error = false;
                    }
                    Err(error) => {
                        apps.apps.clear();
                        apps.clear_selection();
                        apps.message = error;
                        apps.error = true;
                    }
                }
            }
            AndroidAppsEvent::Finished {
                request_id,
                target,
                result,
            } => self.android_apps.finished(request_id, target, result),
            _ => {}
        }
    }
    fn android_app_devices(&mut self) {
        let apps = &mut self.android_apps;
        apps.devices_request += 1;
        apps.devices_loading = self
            .worker
            .commands
            .try_send(Command::AndroidAppDevices {
                request_id: apps.devices_request,
                uri: self.session.as_ref().map(|s| s.uri.clone()),
            })
            .is_ok();
        if !apps.devices_loading {
            apps.message = "Worker busy. Refresh devices to retry.".into();
            apps.error = true;
        }
    }
    fn list_android_apps(&mut self) {
        let apps = &mut self.android_apps;
        if apps.serial.is_empty() || apps.mutating {
            return;
        }
        apps.list_request += 1;
        apps.listing = true;
        if self
            .worker
            .commands
            .try_send(Command::ListAndroidApps {
                request_id: apps.list_request,
                serial: apps.serial.clone(),
            })
            .is_err()
        {
            apps.listing = false;
            apps.message = "Worker busy. Refresh apps to retry.".into();
            apps.error = true;
        }
    }
    fn android_app_action(&mut self, action: AppAction) {
        let apps = &mut self.android_apps;
        if apps.mutating {
            return;
        }
        let Some(target) = apps.target() else {
            return;
        };
        apps.request_id += 1;
        apps.loading = true;
        apps.mutating = !matches!(action, AppAction::Inspect);
        apps.message = if apps.mutating {
            "Applying to selected app…"
        } else {
            "Reading selected app…"
        }
        .into();
        apps.error = false;
        if self
            .worker
            .commands
            .try_send(Command::AndroidApp {
                request_id: apps.request_id,
                target,
                action,
            })
            .is_err()
        {
            apps.loading = false;
            apps.mutating = false;
            apps.message = "Worker busy. Refresh details to retry.".into();
            apps.error = true;
        }
    }
    pub fn android_apps_ui(&mut self, ui: &mut egui::Ui) {
        if self.android_apps.devices_request == 0 {
            self.android_app_devices();
        }
        let mut device = self.android_apps.serial.clone();
        ui.heading("Android apps");
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!self.android_apps.mutating, |ui| {
                egui::ComboBox::from_id_salt("installed-app-device")
                    .width(240.0)
                    .selected_text(
                        self.android_apps
                            .devices
                            .iter()
                            .find(|d| d.serial == device)
                            .map_or("Select Android device", |d| d.label.as_str()),
                    )
                    .show_ui(ui, |ui| {
                        for d in &self.android_apps.devices {
                            ui.add_enabled_ui(d.state == "device", |ui| {
                                ui.selectable_value(
                                    &mut device,
                                    d.serial.clone(),
                                    format!("{} · {}", d.label, d.serial),
                                );
                            });
                        }
                    });
                if ui
                    .add_enabled(
                        !self.android_apps.devices_loading,
                        egui::Button::new("Refresh devices"),
                    )
                    .clicked()
                {
                    self.android_app_devices();
                }
                if ui
                    .add_enabled(
                        !self.android_apps.listing && !device.is_empty(),
                        egui::Button::new("Refresh apps"),
                    )
                    .clicked()
                {
                    self.list_android_apps();
                }
            });
            if self.android_apps.listing || self.android_apps.devices_loading {
                ui.label("Loading app list…");
            } else if let Some(user) = self.android_apps.user {
                ui.weak(format!("Android user {user}"));
            }
        });
        if device != self.android_apps.serial {
            self.android_apps.select_device(device);
            self.list_android_apps();
        }
        ui.separator();
        let mut selected = None;
        let old_filter = self.android_apps.filter;
        egui::Panel::left("installed-app-list")
            .default_size(320.0)
            .size_range(230.0..=480.0)
            .resizable(true)
            .show(ui, |ui| {
                let apps = &mut self.android_apps;
                ui.add_enabled_ui(!apps.mutating, |ui| {
                    ui.horizontal(|ui| {
                        for (filter, label) in [
                            (Filter::Installed, "Installed"),
                            (Filter::System, "System"),
                            (Filter::All, "All"),
                        ] {
                            ui.selectable_value(&mut apps.filter, filter, label);
                        }
                    });
                    ui.add(
                        egui::TextEdit::singleline(&mut apps.search)
                            .hint_text("Search package…")
                            .desired_width(f32::INFINITY),
                    );
                });
                let query = apps.search.to_lowercase();
                let visible: Vec<_> = apps
                    .apps
                    .iter()
                    .filter(|a| {
                        matches!(apps.filter, Filter::All)
                            || a.system == matches!(apps.filter, Filter::System)
                    })
                    .filter(|a| a.package.to_lowercase().contains(&query))
                    .collect();
                ui.weak(format!("{} apps", visible.len()));
                egui::ScrollArea::vertical()
                    .id_salt("installed-app-rows")
                    .auto_shrink([false, false])
                    .show_rows(ui, 36.0, visible.len(), |ui, rows| {
                        for index in rows {
                            let app = visible[index];
                            if ui
                                .add_enabled(
                                    !apps.mutating,
                                    egui::Button::selectable(
                                        app.package == apps.selected,
                                        RichText::new(&app.package).monospace(),
                                    )
                                    .truncate()
                                    .min_size(egui::vec2(ui.available_width(), 36.0)),
                                )
                                .on_hover_text(&app.package)
                                .clicked()
                            {
                                selected = Some(app.package.clone());
                            }
                        }
                    });
            });
        if old_filter != self.android_apps.filter {
            let state = &mut self.android_apps;
            if state
                .apps
                .iter()
                .find(|a| a.package == state.selected)
                .is_some_and(|a| {
                    state.filter != Filter::All && a.system != (state.filter == Filter::System)
                })
            {
                state.clear_selection();
            }
        }
        if let Some(package) = selected {
            self.android_apps.clear_selection();
            self.android_apps.selected = package;
            self.android_apps.permission_search.clear();
            self.android_app_action(AppAction::Inspect);
        }
        let mut action = None;
        egui::CentralPanel::default().show(ui, |ui| {
            let apps = &mut self.android_apps;
            if apps.selected.is_empty() {
                ui.label("Select an app on the left to manage it.");
                ui.weak("Installed shows user-installed apps. System shows preinstalled apps.");
                if apps.serial.is_empty() { ui.label("Connect an Android emulator or USB device, then Refresh devices."); }
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&apps.selected).strong());
                    if ui.small_button("Copy package").clicked() { ui.ctx().copy_text(apps.selected.clone()); }
                    if ui.add_enabled(!apps.loading,egui::Button::new("Refresh details")).clicked() { action = Some(AppAction::Inspect); }
                });
            }
            if !apps.message.is_empty() { ui.colored_label(if apps.error { Color32::LIGHT_RED } else { ui.visuals().weak_text_color() }, &apps.message); }
            if let Some(details) = &apps.details {
                ui.weak(format!("Version {} · Target SDK {} · {}{}", details.version, details.target_sdk, if details.system { "System app" } else { "User app" }, if details.debuggable { " · Debuggable" } else { "" }));
                ui.horizontal(|ui| {
                    for (section,label) in [(Section::Permissions,"Permissions"),(Section::Storage,"Storage"),(Section::Actions,"App actions")] { ui.selectable_value(&mut apps.section,section,label); }
                });
                ui.separator();
                ui.add_enabled_ui(!apps.loading,|ui| {
                    egui::ScrollArea::vertical().id_salt("app-options").show(ui, |ui| match apps.section {
                        Section::Permissions => permission_ui(ui, details, &mut apps.permission_search, &mut action),
                        Section::Storage => storage_ui(ui, apps.cache_only, &mut action),
                        Section::Actions => {
                            ui.label(if details.stopped { "App state: stopped" } else { "App state: not force-stopped" });
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("Open app").clicked() { action = Some(AppAction::Launch); }
                                if ui.button("Force stop").clicked() { action = Some(AppAction::ForceStop); }
                                if ui.button("App info on device").clicked() { action = Some(AppAction::OpenSettings(AppSettingsPage::Info)); }
                            });
                            ui.weak("Force stop ends the app and its background work. Open app starts its launcher activity.");
                        }
                    });
                });
            }
        });
        if let Some(action) = action {
            let confirmation = match action {
                AppAction::ClearStorage => Some((
                    "Clear app storage?",
                    "This deletes this app's local databases, preferences, files and login state for the selected Android user. It cannot be undone.",
                )),
                AppAction::ClearCache => Some((
                    "Clear app cache?",
                    "This removes temporary cache for this app. Saved app data and login state are kept.",
                )),
                AppAction::ForceStop => Some((
                    "Force stop app?",
                    "This ends the app and its background work on the selected device.",
                )),
                _ => None,
            };
            if let Some((title, message)) = confirmation {
                if let Some(target) = self.android_apps.target() {
                    self.android_apps.confirmation = Some(Confirmation {
                        target,
                        action,
                        title,
                        message,
                    });
                }
            } else {
                self.android_app_action(action);
            }
        }
        let mut confirm = false;
        let mut cancel = false;
        if let Some(pending) = &self.android_apps.confirmation {
            let modal = egui::Modal::new(egui::Id::new("app-operation-confirmation")).show(
                ui.ctx(),
                |ui| {
                    ui.set_max_width(460.0);
                    ui.heading(pending.title);
                    ui.label(RichText::new(&pending.target.package).strong());
                    ui.label(format!(
                        "Device {} · Android user {}",
                        pending.target.serial, pending.target.user
                    ));
                    ui.label(pending.message);
                    ui.horizontal(|ui| {
                        cancel = ui.button("Cancel").clicked();
                        confirm = ui
                            .button(RichText::new("Confirm").color(Color32::LIGHT_RED))
                            .clicked();
                    });
                },
            );
            cancel |= modal.should_close();
        }
        if confirm {
            if let Some(pending) = self.android_apps.confirmation.take()
                && self.android_apps.target() == Some(pending.target)
            {
                self.android_app_action(pending.action);
            }
        } else if cancel {
            self.android_apps.confirmation = None;
        }
    }
}

fn permission_ui(
    ui: &mut egui::Ui,
    details: &AppDetails,
    search: &mut String,
    action: &mut Option<AppAction>,
) {
    ui.label("Runtime permissions");
    ui.weak("Toggles apply immediately. Android can stop the app when a permission is revoked.");
    if ui.button("Permissions on device").clicked() {
        *action = Some(AppAction::OpenSettings(AppSettingsPage::Permissions));
    }
    ui.add(
        egui::TextEdit::singleline(search)
            .hint_text("Filter permissions…")
            .desired_width(f32::INFINITY),
    );
    for note in &details.notes {
        ui.weak(note);
    }
    let query = search.to_lowercase();
    let mut group = "";
    for permission in details.permissions.iter().filter(|p| {
        p.name.to_lowercase().contains(&query)
            || p.label.to_lowercase().contains(&query)
            || p.group.to_lowercase().contains(&query)
    }) {
        if group != permission.group {
            group = &permission.group;
            ui.add_space(8.0);
            ui.strong(group);
        }
        ui.push_id(&permission.name, |ui| {
            ui.horizontal_wrapped(|ui| {
                if permission.editable() {
                    let mut granted = permission.granted == Some(true);
                    let label = if granted { "Granted" } else { "Denied" };
                    if ui.checkbox(&mut granted, label).changed() {
                        *action = Some(AppAction::SetPermission {
                            name: permission.name.clone(),
                            granted,
                        });
                    }
                } else {
                    ui.weak(match permission.granted {
                        Some(true) => "Granted · locked",
                        Some(false) => "Denied · locked",
                        None => "OS-managed",
                    });
                }
                ui.label(
                    RichText::new(
                        permission
                            .name
                            .strip_prefix("android.permission.")
                            .unwrap_or(&permission.name),
                    )
                    .monospace(),
                )
                .on_hover_text(format!(
                    "{}\n{}",
                    permission.name,
                    permission.flags.join(" | ")
                ));
            });
            if !permission.label.is_empty() {
                ui.weak(&permission.label);
            }
        });
    }
    ui.add_space(12.0);
    ui.separator();
    ui.strong("Special access on device");
    ui.weak("These settings are managed by Android separately from runtime permissions.");
    ui.horizontal_wrapped(|ui| {
        for (label, page) in [
            ("Notifications", AppSettingsPage::Notifications),
            ("Display over apps", AppSettingsPage::Overlay),
            ("All files access", AppSettingsPage::AllFiles),
            ("Exact alarms", AppSettingsPage::ExactAlarms),
        ] {
            if ui.button(label).clicked() {
                *action = Some(AppAction::OpenSettings(page));
            }
        }
    });
}

fn storage_ui(ui: &mut egui::Ui, cache_only: bool, action: &mut Option<AppAction>) {
    ui.heading("Storage & cache");
    ui.label("Cached files");
    ui.weak("Remove temporary cached files while keeping saved app data.");
    if ui
        .add_enabled(cache_only, egui::Button::new("Clear cache"))
        .clicked()
    {
        *action = Some(AppAction::ClearCache);
    }
    if !cache_only {
        ui.weak("Cache-only clearing is unavailable through ADB on this device. Open App info, then Storage.");
    }
    ui.add_space(16.0);
    ui.label("App data");
    ui.weak("Reset the app's local data, including databases, preferences, files and login state.");
    if ui
        .button(RichText::new("Clear storage").color(Color32::LIGHT_RED))
        .clicked()
    {
        *action = Some(AppAction::ClearStorage);
    }
    ui.add_space(16.0);
    if ui.button("App info / storage on device").clicked() {
        *action = Some(AppAction::OpenSettings(AppSettingsPage::Storage));
    }
    ui.weak("Storage usage is shown by Android in App info → Storage.");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_results_cannot_change_another_app_or_device() {
        let mut state = AndroidAppsUi::default();
        state.select_device("first".into());
        state.user = Some(0);
        state.selected = "dev.example.one".into();
        let target = state.target().unwrap();
        let request = state.request_id;
        state.clear_selection();
        state.selected = "dev.example.two".into();
        state.loading = true;
        state.finished(request, target, Err("old app".into()));
        assert!(state.loading);
        assert!(!state.error);
        let target = state.target().unwrap();
        let request = state.request_id;
        state.select_device("second".into());
        state.user = Some(0);
        state.selected = target.package.clone();
        state.loading = true;
        state.finished(request, target, Err("old device".into()));
        assert!(state.loading);
        assert!(!state.error);
    }
}
