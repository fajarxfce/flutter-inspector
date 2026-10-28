use crate::app::InspectorApp;
use eframe::egui::{self, Color32, RichText};
use inspector_application::*;
use std::collections::VecDeque;

struct Record {
    line: LogLine,
    text: String,
    search: String,
    bytes: usize,
}
pub struct LogBuffer {
    records: VecDeque<Record>,
    visible: VecDeque<u64>,
    first: u64,
    bytes: usize,
    pub dropped: u64,
    query: String,
    level: LogLevel,
    pid: Option<u32>,
}
impl Default for LogBuffer {
    fn default() -> Self {
        Self {
            records: VecDeque::new(),
            visible: VecDeque::new(),
            first: 0,
            bytes: 0,
            dropped: 0,
            query: String::new(),
            level: LogLevel::Verbose,
            pid: None,
        }
    }
}
impl LogBuffer {
    pub fn clear(&mut self) {
        self.first += self.records.len() as u64;
        self.records.clear();
        self.visible.clear();
        self.bytes = 0;
        self.dropped = 0;
    }
    fn matches(&self, record: &Record) -> bool {
        record.line.level >= self.level
            && self.pid.is_none_or(|pid| pid == record.line.pid)
            && (self.query.is_empty() || record.search.contains(&self.query))
    }
    pub fn append(&mut self, lines: Vec<LogLine>) {
        for line in lines {
            let text = line.text();
            let search = text.to_lowercase();
            let bytes =
                text.len() + search.len() + line.message.len() + line.tag.len() + line.time.len();
            let record = Record {
                line,
                text,
                search,
                bytes,
            };
            if self.matches(&record) {
                self.visible
                    .push_back(self.first + self.records.len() as u64);
            }
            self.bytes += bytes;
            self.records.push_back(record);
            while self.records.len() > 10_000 || self.bytes > 8 * 1024 * 1024 {
                if let Some(old) = self.records.pop_front() {
                    self.bytes -= old.bytes;
                    self.first += 1;
                    self.dropped += 1;
                }
            }
        }
        while self.visible.front().is_some_and(|id| *id < self.first) {
            self.visible.pop_front();
        }
    }
    pub fn filter(&mut self, query: &str, level: LogLevel, pid: Option<u32>) {
        let query = query.to_lowercase();
        if self.query == query && self.level == level && self.pid == pid {
            return;
        }
        self.query = query;
        self.level = level;
        self.pid = pid;
        self.visible = self
            .records
            .iter()
            .enumerate()
            .filter(|(_, r)| self.matches(r))
            .map(|(i, _)| self.first + i as u64)
            .collect();
    }
    pub fn copy_visible(&self) -> String {
        self.visible
            .iter()
            .filter_map(|id| self.records.get((id - self.first) as usize))
            .map(|r| r.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
    pub fn ui(&self, ui: &mut egui::Ui, id: &str, follow: &mut bool) {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.style_mut().interaction.multi_widget_text_select = true;
        let row_height = ui.fonts_mut(|fonts| fonts.row_height(&egui::FontId::monospace(12.0)));
        egui::ScrollArea::both()
            .id_salt(id)
            .auto_shrink([false, false])
            .stick_to_bottom(*follow)
            .show_rows(ui, row_height, self.visible.len(), |ui, range| {
                for index in range {
                    let Some(record) = self
                        .records
                        .get((self.visible[index] - self.first) as usize)
                    else {
                        continue;
                    };
                    // Bound glyph layout for pathological single-line stack dumps/payloads.
                    let preview: std::borrow::Cow<'_, str> = if record.text.len() > 4096 {
                        (record.text.chars().take(4096).collect::<String>()
                            + " … (right-click to copy full line)")
                            .into()
                    } else {
                        record.text.as_str().into()
                    };
                    let response = ui
                        .push_id((id, self.visible[index]), |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(preview)
                                        .monospace()
                                        .size(12.0)
                                        .color(color(record.line.level, ui.visuals().dark_mode)),
                                )
                                .extend()
                                .selectable(true),
                            )
                        })
                        .inner;
                    if response.is_pointer_button_down_on() || response.drag_started() {
                        *follow = false;
                    }
                    response.context_menu(|ui| {
                        if ui.button("Copy log line").clicked() {
                            ui.ctx().copy_text(record.text.clone());
                            ui.close();
                        }
                    });
                }
            });
    }
}
pub fn color(level: LogLevel, dark: bool) -> Color32 {
    match (level, dark) {
        (LogLevel::Verbose, true) => Color32::from_rgb(148, 160, 178),
        (LogLevel::Debug, true) => Color32::from_rgb(109, 185, 255),
        (LogLevel::Info, true) => Color32::from_rgb(120, 217, 160),
        (LogLevel::Warn, true) => Color32::from_rgb(255, 202, 104),
        (LogLevel::Error, true) => Color32::from_rgb(255, 120, 130),
        (LogLevel::Fatal, true) => Color32::from_rgb(234, 144, 255),
        (LogLevel::Verbose, false) => Color32::from_rgb(91, 99, 116),
        (LogLevel::Debug, false) => Color32::from_rgb(0, 91, 176),
        (LogLevel::Info, false) => Color32::from_rgb(0, 112, 59),
        (LogLevel::Warn, false) => Color32::from_rgb(145, 87, 0),
        (LogLevel::Error, false) => Color32::from_rgb(195, 25, 43),
        (LogLevel::Fatal, false) => Color32::from_rgb(147, 33, 168),
    }
}
pub struct LogcatUi {
    pub buffer: LogBuffer,
    devices: Vec<AndroidDevice>,
    serial: String,
    generation: u64,
    request: u64,
    scanning: bool,
    pub running: bool,
    status: String,
    mine: bool,
    pid: String,
    query: String,
    level: LogLevel,
    follow: bool,
}
impl Default for LogcatUi {
    fn default() -> Self {
        Self {
            buffer: Default::default(),
            devices: vec![],
            serial: String::new(),
            generation: 0,
            request: 0,
            scanning: false,
            running: false,
            status: "Looking for Android devices…".into(),
            mine: true,
            pid: String::new(),
            query: String::new(),
            level: LogLevel::Verbose,
            follow: true,
        }
    }
}
impl InspectorApp {
    pub fn logcat_session_reset(&mut self) {
        self.logcat.pid.clear();
    }
    pub fn refresh_logcat_devices(&mut self) {
        let request_id = self.logcat.request + 1;
        if self
            .worker
            .commands
            .try_send(Command::LogcatDevices {
                request_id,
                uri: self.session.as_ref().map(|s| s.uri.clone()),
            })
            .is_ok()
        {
            self.logcat.request = request_id;
            self.logcat.scanning = true;
        }
    }
    fn start_logcat(&mut self) {
        if self.logcat.serial.is_empty() {
            return;
        }
        let generation = self.logcat.generation + 1;
        if self
            .worker
            .commands
            .try_send(Command::Logcat(LogcatCommand::Start {
                serial: self.logcat.serial.clone(),
                generation,
            }))
            .is_ok()
        {
            self.logcat.generation = generation;
            self.logcat.buffer.clear();
            self.logcat.running = true;
            self.logcat.status = "Starting…".into();
        }
    }
    pub fn logcat_event(&mut self, event: LogcatEvent) {
        match event {
            LogcatEvent::Devices {
                request_id,
                devices,
                suggested,
                error,
            } if request_id == self.logcat.request => {
                self.logcat.scanning = false;
                if let Some(error) = &error {
                    self.logcat.status = error.clone();
                }
                let previous = self.logcat.serial.clone();
                let suggested = suggested.or_else(|| {
                    self.project
                        .managed_uri
                        .as_ref()
                        .filter(|uri| self.session.as_ref().is_some_and(|s| &s.uri == *uri))
                        .and_then(|_| {
                            devices.iter().find(|d| {
                                d.serial == self.project.managed_device && d.state == "device"
                            })
                        })
                        .map(|d| d.serial.clone())
                });
                if let Some(serial) = suggested {
                    self.logcat.serial = serial;
                    self.logcat.pid = self
                        .session
                        .as_ref()
                        .and_then(|s| s.vm_pid)
                        .map(|pid| pid.to_string())
                        .unwrap_or_default();
                } else if self.logcat.serial.is_empty()
                    && devices.iter().filter(|d| d.state == "device").count() == 1
                {
                    self.logcat.serial = devices
                        .iter()
                        .find(|d| d.state == "device")
                        .unwrap()
                        .serial
                        .clone();
                }
                self.logcat.devices = devices;
                if self
                    .logcat
                    .devices
                    .iter()
                    .any(|d| d.serial == self.logcat.serial && d.state == "device")
                    && (!self.logcat.running || self.logcat.serial != previous)
                {
                    self.start_logcat();
                } else if self.logcat.devices.is_empty() && !self.logcat.running && error.is_none()
                {
                    self.logcat.status =
                        "Connect an Android device with USB debugging, then Refresh devices."
                            .into();
                }
            }
            LogcatEvent::Lines {
                generation,
                lines,
                dropped,
            } if generation == self.logcat.generation => {
                self.logcat.buffer.dropped += dropped;
                self.logcat.buffer.append(lines);
            }
            LogcatEvent::Status {
                generation,
                running,
                message,
            } if generation == self.logcat.generation => {
                self.logcat.running = running;
                self.logcat.status = message;
            }
            _ => {}
        }
    }
    pub fn logcat_ui(&mut self, ui: &mut egui::Ui) {
        let mut refresh = false;
        let mut start = false;
        let mut clear = false;
        let log = &mut self.logcat;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Logcat");
            let previous = log.serial.clone();
            let label = log
                .devices
                .iter()
                .find(|d| d.serial == log.serial)
                .map(|d| d.label.as_str())
                .unwrap_or("Select Android device");
            egui::ComboBox::from_id_salt("logcat-device")
                .selected_text(label)
                .width(145.0)
                .show_ui(ui, |ui| {
                    for device in &log.devices {
                        ui.add_enabled_ui(device.state == "device", |ui| {
                            ui.selectable_value(
                                &mut log.serial,
                                device.serial.clone(),
                                format!("{} · {} ({})", device.label, device.serial, device.state),
                            );
                        });
                    }
                });
            if previous != log.serial {
                log.pid.clear();
                start = true;
            }
            refresh = ui
                .add_enabled(!log.scanning, egui::Button::new("Refresh devices").small())
                .clicked();
            if !log.running {
                start |= ui
                    .add_enabled(!log.serial.is_empty(), egui::Button::new("Retry").small())
                    .clicked();
            }
            ui.separator();
            ui.selectable_value(&mut log.mine, true, "App kita");
            ui.selectable_value(&mut log.mine, false, "Semua log");
            if log.mine {
                ui.add(
                    egui::TextEdit::singleline(&mut log.pid)
                        .hint_text("App PID")
                        .desired_width(65.0),
                )
                .on_hover_text(
                    "Follows the connected Android app. You can also enter a PID manually.",
                );
            }
            egui::ComboBox::from_id_salt("logcat-level")
                .selected_text(format!("{}+", log.level.label()))
                .width(85.0)
                .show_ui(ui, |ui| {
                    for level in LogLevel::ALL {
                        ui.selectable_value(&mut log.level, level, format!("{}+", level.label()));
                    }
                });
            ui.add(
                egui::TextEdit::singleline(&mut log.query)
                    .hint_text("Filter message, tag, PID…")
                    .desired_width(190.0),
            );
            clear = ui
                .small_button("Clear")
                .on_hover_text("Clear this panel. Device logs are kept.")
                .clicked();
            if ui.small_button("Copy visible").clicked() {
                ui.ctx().copy_text(log.buffer.copy_visible());
            }
            ui.checkbox(&mut log.follow, "Auto-scroll");
        });
        log.buffer.filter(
            &log.query,
            log.level,
            if log.mine {
                Some(
                    log.pid
                        .parse::<u32>()
                        .ok()
                        .filter(|pid| *pid > 0)
                        .unwrap_or(u32::MAX),
                )
            } else {
                None
            },
        );
        ui.horizontal(|ui| {
            ui.small(&log.status);
            ui.weak(format!(
                "{} / {} lines",
                log.buffer.visible.len(),
                log.buffer.records.len()
            ));
            if log.buffer.dropped > 0 {
                ui.weak(format!(
                    "{} older/overflow lines discarded",
                    log.buffer.dropped
                ));
            }
            if log.mine && log.pid.is_empty() {
                ui.weak("Connect your Android app, enter its PID, or choose Semua log.");
            }
        });
        log.buffer.ui(ui, "logcat-lines", &mut log.follow);
        if refresh {
            self.refresh_logcat_devices();
        }
        if start {
            self.start_logcat();
        }
        if clear {
            let generation = self.logcat.generation + 1;
            if self
                .worker
                .commands
                .try_send(Command::Logcat(LogcatCommand::Clear { generation }))
                .is_ok()
            {
                self.logcat.generation = generation;
                self.logcat.buffer.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn line(pid: u32, message: &str) -> LogLine {
        LogLine {
            time: String::new(),
            pid,
            tid: pid,
            level: LogLevel::Info,
            tag: "flutter".into(),
            message: message.into(),
        }
    }
    #[test]
    fn filters_existing_and_new_lines_and_bounds_retention() {
        let mut buffer = LogBuffer::default();
        buffer.append(vec![line(1, "Hello"), line(2, "HELLO"), line(1, "other")]);
        buffer.filter("hello", LogLevel::Info, Some(1));
        assert_eq!(buffer.visible.len(), 1);
        buffer.append(vec![line(1, "HELLO again")]);
        assert_eq!(buffer.visible.len(), 2);
        buffer.append((0..11_000).map(|_| line(2, "new")).collect());
        assert_eq!(buffer.records.len(), 10_000);
        assert!(buffer.visible.is_empty());
        buffer.filter("", LogLevel::Verbose, None);
        assert_eq!(buffer.visible.len(), 10_000);
        buffer.clear();
        assert!(buffer.copy_visible().is_empty());
    }
}
