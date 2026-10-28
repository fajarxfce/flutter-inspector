use crate::{app::InspectorApp, logcat::LogBuffer};
use eframe::egui::{self, RichText};
use inspector_application::*;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

pub struct ProjectUi {
    path: String,
    project: Option<FlutterProject>,
    request: u64,
    loading: bool,
    profile: usize,
    sdk: String,
    devices: Vec<FlutterDevice>,
    device: String,
    devices_request: u64,
    scanning: bool,
    tool_args: String,
    pub generation: u64,
    pub active: bool,
    pub ready: bool,
    pub managed_uri: Option<String>,
    pub managed_device: String,
    pub retry_at: Instant,
    status: String,
    output: LogBuffer,
    query: String,
    follow: bool,
    picker: Option<std::sync::mpsc::Receiver<Option<PathBuf>>>,
}
impl Default for ProjectUi {
    fn default() -> Self {
        Self {
            path: String::new(),
            project: None,
            request: 0,
            loading: false,
            profile: 0,
            sdk: String::new(),
            devices: vec![],
            device: String::new(),
            devices_request: 0,
            scanning: false,
            tool_args: String::new(),
            generation: 0,
            active: false,
            ready: false,
            managed_uri: None,
            managed_device: String::new(),
            retry_at: Instant::now(),
            status: "Choose a Flutter project to run it without an editor.".into(),
            output: Default::default(),
            query: String::new(),
            follow: true,
            picker: None,
        }
    }
}
impl InspectorApp {
    fn open_project(&mut self, path: PathBuf) {
        if self.project.active {
            return;
        }
        let request_id = self.project.request + 1;
        if self
            .worker
            .commands
            .try_send(Command::OpenProject { request_id, path })
            .is_ok()
        {
            self.project.request = request_id;
            self.project.loading = true;
            self.project.project = None;
            self.project.devices.clear();
            // Ignore device results for the previous project while opening another.
            self.project.devices_request += 1;
            self.project.scanning = false;
        }
    }
    pub fn project_opened(&mut self, request_id: u64, result: Result<FlutterProject, String>) {
        if request_id != self.project.request {
            return;
        }
        self.project.loading = false;
        match result {
            Ok(project) => {
                self.project.path = project.root.to_string_lossy().into_owned();
                self.project.sdk = project.sdk.to_string_lossy().into_owned();
                self.project.profile = project
                    .profiles
                    .iter()
                    .position(|p| p.issue.is_none())
                    .unwrap_or(0);
                let profile = &project.profiles[self.project.profile];
                self.project.tool_args = profile.tool_args.join("\n");
                self.project.device = profile.device.clone();
                self.project.devices.clear();
                self.project.status =
                    "Project ready. Select configuration and device, then Run debug.".into();
                self.settings.recent_projects.retain(|p| *p != project.root);
                self.settings
                    .recent_projects
                    .insert(0, project.root.clone());
                self.settings.recent_projects.truncate(12);
                self.project.project = Some(project);
                self.send(Command::SaveSettings(self.settings.clone()));
                self.refresh_project_devices();
            }
            Err(error) => self.project.status = error,
        }
    }
    fn refresh_project_devices(&mut self) {
        let Some(project) = &self.project.project else {
            return;
        };
        let request_id = self.project.devices_request + 1;
        if self
            .worker
            .commands
            .try_send(Command::ProjectDevices {
                request_id,
                sdk: self.project.sdk.clone().into(),
                cwd: project.root.clone(),
            })
            .is_ok()
        {
            self.project.devices_request = request_id;
            self.project.scanning = true;
        }
    }
    pub fn project_devices(&mut self, request_id: u64, result: Result<Vec<FlutterDevice>, String>) {
        if request_id != self.project.devices_request {
            return;
        }
        self.project.scanning = false;
        match result {
            Ok(devices) => {
                if self.project.device.is_empty() {
                    self.project.device = devices
                        .iter()
                        .find(|d| d.platform.starts_with("android"))
                        .or_else(|| devices.first())
                        .map(|d| d.id.clone())
                        .unwrap_or_default();
                }
                self.project.devices = devices;
            }
            Err(error) => self.project.status = error,
        }
    }
    pub fn runner_event(&mut self, event: RunnerEvent) {
        match event {
            RunnerEvent::Output {
                generation,
                lines,
                dropped,
            } if generation == self.project.generation => {
                self.project.output.dropped += dropped;
                self.project.output.append(lines);
            }
            RunnerEvent::State {
                generation,
                active,
                ready,
                message,
            } if generation == self.project.generation => {
                self.project.active = active;
                self.project.ready = ready;
                self.project.status = message;
                if !active {
                    self.project.managed_uri = None;
                }
            }
            RunnerEvent::VmService {
                generation,
                uri,
                device,
            } if generation == self.project.generation => {
                self.project.managed_uri = Some(uri.clone());
                self.project.managed_device = device;
                self.project.retry_at = Instant::now() + Duration::from_secs(2);
                self.connect_to(uri, None);
            }
            _ => {}
        }
    }
    fn run_project(&mut self) {
        let Some(project) = &self.project.project else {
            return;
        };
        let mut profile = project.profiles[self.project.profile].clone();
        profile.tool_args = self
            .project
            .tool_args
            .lines()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        let request = LaunchRequest {
            sdk: self.project.sdk.clone().into(),
            profile,
            device: self.project.device.clone(),
        };
        let generation = self.project.generation + 1;
        if self
            .worker
            .commands
            .try_send(Command::Runner(RunnerCommand::Start {
                generation,
                request: Box::new(request),
            }))
            .is_ok()
        {
            self.project.generation = generation;
            self.project.active = true;
            self.project.ready = false;
            self.project.output.clear();
            self.project.managed_uri = None;
            self.project.status = "Starting Flutter…".into();
            if self.session.is_some() {
                self.send(Command::Disconnect);
            }
        }
    }
    pub fn project_ui(&mut self, ui: &mut egui::Ui) {
        let viewport_height = ui.available_height();
        egui::ScrollArea::vertical()
            .id_salt("project-page")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.project_contents(ui, viewport_height);
            });
    }
    fn project_contents(&mut self, ui: &mut egui::Ui, viewport_height: f32) {
        let top = ui.cursor().top();
        if let Some(receiver) = &self.project.picker {
            if let Ok(path) = receiver.try_recv() {
                self.project.picker = None;
                if let Some(path) = path {
                    self.open_project(path);
                }
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
        }
        ui.heading("Projects");
        ui.label("Launch Flutter in debug mode, then inspect it here.");
        let mut open = None;
        ui.add_enabled_ui(!self.project.active && !self.project.loading, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.project.path)
                        .hint_text("Project folder containing pubspec.yaml")
                        .desired_width((ui.available_width() - 285.0).max(180.0)),
                );
                if ui.button("Open project").clicked() {
                    open = Some(PathBuf::from(&self.project.path));
                }
                if ui
                    .add_enabled(self.project.picker.is_none(), egui::Button::new("Browse…"))
                    .clicked()
                {
                    let (sender, receiver) = std::sync::mpsc::channel();
                    let ctx = ui.ctx().clone();
                    std::thread::spawn(move || {
                        let _ = sender.send(
                            rfd::FileDialog::new()
                                .set_title("Choose Flutter project")
                                .pick_folder(),
                        );
                        ctx.request_repaint();
                    });
                    self.project.picker = Some(receiver);
                }
                ui.menu_button("Recent", |ui| {
                    if self.settings.recent_projects.is_empty() {
                        ui.weak("No recent projects yet");
                    }
                    for path in &self.settings.recent_projects {
                        if ui.button(path.display().to_string()).clicked() {
                            open = Some(path.clone());
                            ui.close();
                        }
                    }
                });
            });
        });
        if let Some(path) = open {
            self.open_project(path);
        }
        let mut refresh = false;
        let mut run = false;
        let mut control = None;
        if let Some(project) = &mut self.project.project {
            ui.add_enabled_ui(!self.project.active && !self.project.loading, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Configuration");
                    let previous = self.project.profile;
                    let current = &project.profiles[self.project.profile];
                    egui::ComboBox::from_id_salt("project-profile").selected_text(format!("{} · {}", current.source, current.name)).width(250.0).show_ui(ui, |ui| {
                        for (index, profile) in project.profiles.iter().enumerate() {
                            ui.selectable_value(&mut self.project.profile, index, format!("{} · {}{}", profile.source, profile.name, if profile.issue.is_some() { " (unavailable)" } else { "" }));
                        }
                    });
                    if previous != self.project.profile {
                        let profile = &project.profiles[self.project.profile]; self.project.tool_args = profile.tool_args.join("\n");
                        if !profile.device.is_empty() { self.project.device = profile.device.clone(); }
                    }
                    ui.label("Device");
                    let label = self.project.devices.iter().find(|d| d.id == self.project.device).map(|d| d.name.as_str()).unwrap_or("Select device");
                    egui::ComboBox::from_id_salt("project-device").selected_text(label).width(190.0).show_ui(ui, |ui| {
                        for device in &self.project.devices { ui.selectable_value(&mut self.project.device, device.id.clone(), format!("{} · {} ({})", device.name, device.platform, device.id)); }
                    });
                    refresh = ui.add_enabled(!self.project.scanning, egui::Button::new("Refresh devices")).clicked();
                    if self.project.scanning { ui.weak("Finding devices…"); }
                });
                let profile = &mut project.profiles[self.project.profile];
                if let Some(issue) = &profile.issue { ui.colored_label(egui::Color32::LIGHT_RED, issue); }
                ui.horizontal(|ui| {
                    ui.label("Entrypoint"); ui.add(egui::TextEdit::singleline(&mut profile.program).desired_width(280.0));
                    ui.label("Flutter SDK"); ui.add(egui::TextEdit::singleline(&mut self.project.sdk).desired_width(320.0))
                        .on_hover_text("Flutter SDK folder or flutter executable. Project FVM SDK is detected automatically.");
                });
                egui::CollapsingHeader::new("Launch details / arguments").show(ui, |ui| {
                    ui.label(format!("Working directory: {}", profile.cwd.display()));
                    ui.small("Flutter tool arguments: one argument per line; spaces are preserved. Edits apply to this run only.");
                    ui.add(egui::TextEdit::multiline(&mut self.project.tool_args).desired_rows(3).desired_width(ui.available_width()));
                    ui.small(format!("{} environment override(s), {} app argument(s) loaded from configuration.", profile.env.len(), profile.args.len()));
                    for note in &project.notes { ui.colored_label(egui::Color32::YELLOW, note); }
                });
                run = ui.add_enabled(profile.issue.is_none() && !self.project.device.is_empty(), egui::Button::new("▶ Run debug")).clicked();
            });
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    self.project.active && self.project.ready,
                    egui::Button::new("Hot reload"),
                )
                .clicked()
            {
                control = Some(RunnerCommand::Reload);
            }
            if ui
                .add_enabled(
                    self.project.active && self.project.ready,
                    egui::Button::new("Hot restart"),
                )
                .clicked()
            {
                control = Some(RunnerCommand::Restart);
            }
            if ui
                .add_enabled(self.project.active, egui::Button::new("■ Stop"))
                .clicked()
            {
                control = Some(RunnerCommand::Stop);
            }
            ui.label(RichText::new(&self.project.status).strong());
        });
        if refresh {
            self.refresh_project_devices();
        }
        if run {
            self.run_project();
        }
        if let Some(control) = control
            && self
                .worker
                .commands
                .try_send(Command::Runner(control))
                .is_ok()
        {
            self.project.ready = false;
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong("Run output");
            ui.add(
                egui::TextEdit::singleline(&mut self.project.query)
                    .hint_text("Filter output…")
                    .desired_width(180.0),
            );
            if ui.small_button("Clear output").clicked() {
                self.project.output.clear();
            }
            if ui.small_button("Copy output").clicked() {
                ui.ctx().copy_text(self.project.output.copy_visible());
            }
            ui.checkbox(&mut self.project.follow, "Auto-scroll");
        });
        self.project
            .output
            .filter(&self.project.query, LogLevel::Verbose, None);
        let height = (viewport_height - (ui.cursor().top() - top)).max(140.0);
        ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
            self.project
                .output
                .ui(ui, "run-output", &mut self.project.follow);
        });
    }
}
