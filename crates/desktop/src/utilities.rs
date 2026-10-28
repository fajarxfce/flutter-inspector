use crate::app::InspectorApp;
use eframe::egui::{self, Color32, RichText};
use inspector_application::{AndroidDevice, ClockAction, ClockEvent, Command};
use inspector_domain::{DeviceClock, epoch_to_local, local_to_epoch, timezones, validate_timezone};
use std::time::Instant;

pub struct UtilitiesUi {
    devices: Vec<AndroidDevice>,
    serial: String,
    devices_request: u64,
    devices_loading: bool,
    request_id: u64,
    busy: bool,
    clock: Option<DeviceClock>,
    read_at: Option<Instant>,
    timezone: String,
    zone_search: String,
    zones: Vec<&'static str>,
    date: Option<jiff::civil::Date>,
    time: String,
    time_picker_open: bool,
    automatic_time: bool,
    automatic_timezone: bool,
    message: String,
    error: bool,
}

impl Default for UtilitiesUi {
    fn default() -> Self {
        let mut zones: Vec<_> = timezones().collect();
        zones.sort_unstable();
        Self {
            devices: vec![],
            serial: String::new(),
            devices_request: 0,
            devices_loading: false,
            request_id: 0,
            busy: false,
            clock: None,
            read_at: None,
            timezone: "Asia/Jakarta".into(),
            zone_search: String::new(),
            zones,
            date: None,
            time: String::new(),
            time_picker_open: false,
            automatic_time: true,
            automatic_timezone: true,
            message: "Select an Android device to read its date, time and timezone.".into(),
            error: false,
        }
    }
}

impl UtilitiesUi {
    fn select_device(&mut self, serial: String) {
        self.serial = serial;
        self.request_id += 1;
        self.busy = false;
        self.clock = None;
        self.read_at = None;
        self.date = None;
        self.time.clear();
        self.time_picker_open = false;
        self.error = false;
        self.message = if self.serial.is_empty() {
            "No Android device connected. Connect a device and Refresh devices."
        } else {
            "Reading device time…"
        }
        .into();
    }

    fn result(
        &mut self,
        request_id: u64,
        serial: &str,
        result: Result<inspector_application::ClockResult, String>,
    ) {
        if request_id != self.request_id || serial != self.serial {
            return;
        }
        self.busy = false;
        match result {
            Ok(result) => {
                self.timezone = result.clock.timezone.clone();
                self.automatic_time = result.clock.automatic_time;
                self.automatic_timezone = result.clock.automatic_timezone;
                if let Ok(local) = epoch_to_local(result.clock.epoch_millis, &result.clock.timezone)
                {
                    self.date = local.date.parse().ok();
                    self.time = local.time;
                }
                self.clock = Some(result.clock);
                self.read_at = Some(Instant::now());
                self.message = result.message;
                self.error = false;
            }
            Err(error) => {
                self.clock = None;
                self.read_at = None;
                self.message = error;
                self.error = true;
            }
        }
    }
}

impl InspectorApp {
    pub fn clock_event(&mut self, event: ClockEvent) {
        match event {
            ClockEvent::Devices {
                request_id,
                devices,
                suggested,
                error,
            } if request_id == self.utilities.devices_request => {
                self.utilities.devices_loading = false;
                let current_available = devices
                    .iter()
                    .any(|d| d.serial == self.utilities.serial && d.state == "device");
                let serial = if !current_available && !self.utilities.busy {
                    devices
                        .iter()
                        .find(|d| d.state == "device" && Some(&d.serial) == suggested.as_ref())
                        .or_else(|| devices.iter().find(|d| d.state == "device"))
                        .map(|d| d.serial.clone())
                        .unwrap_or_default()
                } else {
                    self.utilities.serial.clone()
                };
                self.utilities.devices = devices;
                if serial != self.utilities.serial {
                    self.utilities.select_device(serial);
                    if !self.utilities.serial.is_empty() {
                        self.clock_command(ClockAction::Read);
                    }
                }
                if let Some(error) = error {
                    self.utilities.message = error;
                    self.utilities.error = true;
                }
            }
            ClockEvent::Finished {
                request_id,
                serial,
                result,
            } => self.utilities.result(request_id, &serial, result),
            _ => {}
        }
    }

    fn clock_devices(&mut self) {
        self.utilities.devices_request += 1;
        self.utilities.devices_loading = self
            .worker
            .commands
            .try_send(Command::ClockDevices {
                request_id: self.utilities.devices_request,
                uri: self.session.as_ref().map(|s| s.uri.clone()),
            })
            .is_ok();
        if !self.utilities.devices_loading {
            self.utilities.message = "Device worker is busy. Try Refresh devices again.".into();
            self.utilities.error = true;
        }
    }

    fn clock_command(&mut self, action: ClockAction) {
        if self.utilities.busy || self.utilities.serial.is_empty() {
            return;
        }
        self.utilities.request_id += 1;
        self.utilities.busy = self
            .worker
            .commands
            .try_send(Command::DeviceClock {
                request_id: self.utilities.request_id,
                serial: self.utilities.serial.clone(),
                action,
            })
            .is_ok();
        self.utilities.error = !self.utilities.busy;
        self.utilities.message = if self.utilities.busy {
            "Working on selected device…"
        } else {
            "Device worker is busy. Try again."
        }
        .into();
    }

    pub fn utilities_ui(&mut self, ui: &mut egui::Ui) {
        if self.utilities.devices_request == 0 {
            self.clock_devices();
        }
        ui.heading("Utilities");
        ui.label("Date & Time · Android emulator or USB device · no Flutter bridge required");
        let mut selected = self.utilities.serial.clone();
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!self.utilities.busy, |ui| {
                egui::ComboBox::from_id_salt("clock-device")
                    .width(260.0)
                    .selected_text(
                        self.utilities
                            .devices
                            .iter()
                            .find(|d| d.serial == selected)
                            .map_or("Select Android device", |d| d.label.as_str()),
                    )
                    .show_ui(ui, |ui| {
                        for device in &self.utilities.devices {
                            ui.add_enabled_ui(device.state == "device", |ui| {
                                ui.selectable_value(
                                    &mut selected,
                                    device.serial.clone(),
                                    format!(
                                        "{} · {} · {}",
                                        device.label, device.serial, device.state
                                    ),
                                );
                            });
                        }
                    });
            });
            if ui
                .add_enabled(
                    !self.utilities.devices_loading && !self.utilities.busy,
                    egui::Button::new("Refresh devices"),
                )
                .clicked()
            {
                self.clock_devices();
            }
            if ui
                .add_enabled(
                    !self.utilities.busy && !selected.is_empty(),
                    egui::Button::new("Read device time"),
                )
                .clicked()
            {
                self.clock_command(ClockAction::Read);
            }
            if self.utilities.busy || self.utilities.devices_loading {
                ui.spinner();
            }
        });
        if selected != self.utilities.serial {
            self.utilities.select_device(selected);
            self.clock_command(ClockAction::Read);
        }
        ui.separator();
        let mut action = None;
        egui::ScrollArea::vertical().id_salt("device-clock-utility").show(ui, |ui| {
            if let Some(clock) = &self.utilities.clock {
                if let Ok(local) = epoch_to_local(clock.epoch_millis, &clock.timezone) {
                    ui.label(RichText::new(format!("{}  {}  UTC{}", local.date, local.time, local.offset)).size(22.0).monospace());
                }
                ui.horizontal_wrapped(|ui| {
                    ui.strong(&clock.timezone);
                    ui.weak(format!("Automatic time: {} · Automatic timezone: {}",
                        if clock.automatic_time { "on" } else { "off" }, if clock.automatic_timezone { "on" } else { "off" }));
                    if let Some(read_at) = self.utilities.read_at {
                        ui.weak(format!("Read {}s ago", read_at.elapsed().as_secs()));
                        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
                    }
                });
            }
            ui.colored_label(if self.utilities.error { Color32::LIGHT_RED } else { Color32::GRAY }, &self.utilities.message);
            ui.add_space(10.0);
            ui.add_enabled_ui(!self.utilities.busy && self.utilities.clock.is_some(), |ui| {
                ui.heading("Timezone");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Timezone ID");
                    ui.add(egui::TextEdit::singleline(&mut self.utilities.timezone).desired_width(245.0).hint_text("Asia/Jakarta"));
                    ui.menu_button("Browse", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.utilities.zone_search).hint_text("Search city or zone…"));
                        let needle = self.utilities.zone_search.to_lowercase();
                        let zones: Vec<_> = self.utilities.zones.iter().copied().filter(|zone| zone.to_lowercase().contains(&needle)).collect();
                        egui::ScrollArea::vertical().max_height(220.0).show_rows(ui, 22.0, zones.len(), |ui, rows| {
                            for index in rows {
                                if ui.selectable_label(self.utilities.timezone == zones[index], zones[index]).clicked() {
                                    self.utilities.timezone = zones[index].into();
                                    ui.close();
                                }
                            }
                        });
                    });
                    if ui.add_enabled(validate_timezone(&self.utilities.timezone).is_ok(), egui::Button::new("Apply timezone")).clicked() {
                        action = Some(ClockAction::SetTimezone(self.utilities.timezone.clone()));
                    }
                });
                ui.weak("Applying a timezone turns automatic timezone off. The instant in UTC stays the same.");
                if let Err(error) = validate_timezone(&self.utilities.timezone) { ui.colored_label(Color32::LIGHT_RED, error); }
                ui.add_space(14.0);
                ui.heading("Date & time");
                let zone = self.utilities.clock.as_ref().map(|c| c.timezone.as_str()).unwrap_or("UTC");
                ui.weak(format!("Pick local date and time in {zone}. Apply timezone first to use a different zone."));
                let date = self.utilities.date.map(|date| date.to_string()).unwrap_or_default();
                let valid = local_to_epoch(&date, &self.utilities.time, zone);
                ui.horizontal_wrapped(|ui| {
                    ui.label("Date");
                    date_picker(ui, &mut self.utilities.date, self.utilities.request_id);
                    ui.label("Time");
                    time_picker(ui, &mut self.utilities.time, &mut self.utilities.time_picker_open);
                    if ui.add_enabled(valid.is_ok(), egui::Button::new("Apply date & time")).clicked() {
                        action = Some(ClockAction::SetDateTime { date: self.utilities.date.map(|date| date.to_string()).unwrap_or_default(), time: self.utilities.time.clone(), timezone: zone.into() });
                    }
                    if ui.button("Sync with computer").clicked() { action = Some(ClockAction::SyncComputer); }
                });
                ui.weak("Applying a manual time or syncing with this computer turns automatic time off.");
                if self.utilities.clock.is_some() && let Err(error) = valid { ui.colored_label(Color32::LIGHT_RED, error); }
                ui.add_space(14.0);
                ui.heading("Automatic settings");
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut self.utilities.automatic_time, "Automatic date & time");
                    ui.checkbox(&mut self.utilities.automatic_timezone, "Automatic timezone");
                    if ui.button("Apply automatic settings").clicked() {
                        action = Some(ClockAction::SetAutomatic { time: self.utilities.automatic_time, timezone: self.utilities.automatic_timezone });
                    }
                    if ui.button("Enable both").clicked() { action = Some(ClockAction::SetAutomatic { time: true, timezone: true }); }
                });
            });
            ui.add_space(12.0);
            ui.separator();
            ui.weak("Changes affect every app on the selected Android device. Values are checked again after applying.");
            if ui.add_enabled(!self.utilities.busy && !self.utilities.serial.is_empty(), egui::Button::new("Open Android settings")).clicked() {
                action = Some(ClockAction::OpenSettings);
            }
        });
        if let Some(action) = action {
            self.clock_command(action);
        }
    }
}

fn date_picker(ui: &mut egui::Ui, date: &mut Option<jiff::civil::Date>, request_id: u64) {
    let Some(date) = date else {
        ui.add_enabled(false, egui::Button::new("Choose date"));
        return;
    };
    ui.push_id(request_id, |ui| {
        ui.add(
            egui_extras::DatePickerButton::new(date)
                .id_salt("device-clock-date")
                .calendar_week(false)
                .highlight_weekends(false)
                .arrows(false)
                .start_end_years(1970..=2099),
        );
    });
}

fn time_picker(ui: &mut egui::Ui, text: &mut String, open: &mut bool) {
    let mut parts = [0u8; 3];
    for (part, value) in parts.iter_mut().zip(text.split(':')) {
        *part = value.parse().unwrap_or(0);
    }
    let button = ui.button(
        RichText::new(format!(
            "{}  ▾",
            if text.is_empty() {
                "Choose time"
            } else {
                text.as_str()
            }
        ))
        .monospace(),
    );
    let response = egui::Popup::menu(&button)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            let opening = !*open;
            *open = true;
            ui.set_width(250.0);
            ui.strong("Choose time · 24 hours");
            ui.separator();
            ui.columns(3, |columns| {
                for (index, (label, maximum)) in [("Hour", 24u8), ("Minute", 60), ("Second", 60)]
                    .into_iter()
                    .enumerate()
                {
                    columns[index].strong(label);
                    egui::ScrollArea::vertical()
                        .id_salt(("clock-time-column", index))
                        .max_height(170.0)
                        .auto_shrink([false, false])
                        .show(&mut columns[index], |ui| {
                            for value in 0..maximum {
                                let selected = parts[index] == value;
                                let response = ui.selectable_label(
                                    selected,
                                    RichText::new(format!("{value:02}")).monospace(),
                                );
                                if opening && selected {
                                    response.scroll_to_me(Some(egui::Align::Center));
                                }
                                if response.clicked() {
                                    parts[index] = value;
                                }
                            }
                        });
                }
            });
            *text = format!("{:02}:{:02}:{:02}", parts[0], parts[1], parts[2]);
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new(text.as_str()).monospace());
                if ui.button("Done").clicked() {
                    ui.close();
                }
            });
        });
    if response.is_none() {
        *open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switching_devices_invalidates_old_clock_and_pending_results() {
        let mut ui = UtilitiesUi::default();
        ui.select_device("first".into());
        let previous = ui.request_id;
        ui.select_device("second".into());
        ui.busy = true;
        ui.result(previous, "first", Err("old device error".into()));
        assert!(ui.busy);
        assert!(ui.clock.is_none());
        assert!(!ui.error);
        ui.result(ui.request_id, "second", Err("current device error".into()));
        assert!(!ui.busy);
        assert!(ui.error);
    }
}
