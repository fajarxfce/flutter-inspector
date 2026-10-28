use crate::{drift::DriftEditor, providers::ProviderEditor, repeater::Repeater};
use eframe::egui::{self, Color32, RichText};
use inspector_adapters::backend::Services;
use inspector_application::*;
use inspector_domain::*;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Network,
    Repeater,
    Intercept,
    Preferences,
    Drift,
    Settings,
    Projects,
    Gps,
}
#[derive(Clone, Copy)]
pub enum RequestAction {
    Repeater,
    Curl,
    Raw,
    Response,
}
impl RequestAction {
    fn body_id(self, exchange: &Exchange) -> Option<&str> {
        match self {
            Self::Response => exchange.response.as_ref()?.body.as_ref(),
            _ => exchange.request.body.as_ref(),
        }
        .map(|b| b.id.as_str())
    }
}
pub struct InspectorApp {
    logo: egui::TextureHandle,
    pub worker: Worker,
    pub tab: Tab,
    pub logcat: crate::logcat::LogcatUi,
    pub project: crate::project::ProjectUi,
    pub gps: crate::gps::GpsUi,
    pub uri: String,
    pub session: Option<Session>,
    pub isolates: Vec<(String, String)>,
    pub connection: crate::connection::ConnectionUi,
    pub rows: Vec<Exchange>,
    pub total: usize,
    pub offset: usize,
    pub query: String,
    pub selected: Option<Exchange>,
    pub network_state: crate::network::NetworkUi,
    pub bodies: HashMap<String, Arc<BodyContent>>,
    pub requested: HashSet<String>,
    body_order: VecDeque<String>,
    pub repeater: Repeater,
    pub drafts: Vec<Draft>,
    pub pending: Vec<Pending>,
    pub hold_id: String,
    pub hold_url: String,
    pub hold_method: String,
    pub hold_headers: String,
    pub hold_body: String,
    pub hold_status: u16,
    pub hold_replace: bool,
    pub rule: InterceptRule,
    pub preferences: ProviderEditor,
    pub drift: DriftEditor,
    pub settings: Settings,
    pub directory: PathBuf,
    pub notices: VecDeque<(bool, String)>,
    pub import_text: String,
    pub import_open: bool,
    action: VecDeque<(Exchange, RequestAction)>,
    refresh_at: Option<Instant>,
    started: Instant,
    timings: crate::diagnostics::FrameTimings,
}
impl InspectorApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        theme(&ctx, true);
        let directory = std::env::var_os("INSPECTOR_DATA_DIR").map(PathBuf::from);
        let worker = Worker::start(
            move || ctx.request_repaint(),
            move |sink| Services::new(sink, directory),
        );
        let uri = std::env::var("INSPECTOR_VM_URI").unwrap_or_default();
        let mut app = Self {
            logo: crate::branding::texture(&cc.egui_ctx),
            worker,
            tab: Tab::Network,
            logcat: Default::default(),
            project: Default::default(),
            gps: Default::default(),
            uri,
            session: None,
            isolates: vec![],
            connection: crate::connection::ConnectionUi::default(),
            rows: vec![],
            total: 0,
            offset: 0,
            query: String::new(),
            selected: None,
            network_state: Default::default(),
            bodies: HashMap::new(),
            requested: HashSet::new(),
            body_order: VecDeque::new(),
            repeater: Repeater::default(),
            drafts: vec![],
            pending: vec![],
            hold_id: String::new(),
            hold_url: String::new(),
            hold_method: String::new(),
            hold_headers: String::new(),
            hold_body: String::new(),
            hold_status: 200,
            hold_replace: false,
            rule: InterceptRule::default(),
            preferences: ProviderEditor::default(),
            drift: DriftEditor::default(),
            settings: Settings::default(),
            directory: PathBuf::new(),
            notices: VecDeque::new(),
            import_text: String::new(),
            import_open: false,
            action: VecDeque::new(),
            refresh_at: None,
            started: Instant::now(),
            timings: Default::default(),
        };
        app.send(Command::Initialize);
        app.refresh();
        if !app.uri.is_empty() {
            app.connect_to(app.uri.clone(), None);
        }
        app
    }
    pub fn send(&mut self, command: Command) {
        if let Err(e) = self.worker.commands.try_send(command) {
            self.notice(true, format!("Worker busy: {e}. Try again."));
        }
    }
    pub fn notice(&mut self, error: bool, message: String) {
        let message = if message.len() > 8192 {
            message.chars().take(4096).collect::<String>() + "…"
        } else {
            message
        };
        if self.notices.back().is_some_and(|n| n.1 == message) {
            return;
        }
        self.notices.push_back((error, message));
        while self.notices.len() > 20 {
            self.notices.pop_front();
        }
    }
    pub fn refresh(&mut self) {
        if self.network_state.clearing {
            return;
        }
        self.network_state.history_request += 1;
        self.send(Command::History {
            request_id: self.network_state.history_request,
            query: self.query.clone(),
            offset: self.offset,
        });
    }
    pub fn load(&mut self, id: &str) {
        if !self.network_state.clearing
            && !self.bodies.contains_key(id)
            && self.requested.insert(id.into())
        {
            self.send(Command::LoadBody(id.into()));
        }
    }
    pub fn provider(&mut self, tag: &str, method: &str, mut params: serde_json::Value) {
        let id = if tag.starts_with("prefs") {
            &self.preferences.id
        } else {
            &self.drift.id
        };
        params["provider"] = serde_json::json!(id);
        self.send(Command::Provider {
            tag: tag.into(),
            method: method.into(),
            params,
        });
    }
    fn events(&mut self, ctx: &egui::Context) {
        self.repeater_tick(ctx);
        let processing = Instant::now();
        for index in 0..32 {
            if index > 0 && processing.elapsed() > Duration::from_millis(4) {
                ctx.request_repaint();
                break;
            }
            let Ok(event) = self.worker.events.try_recv() else {
                break;
            };
            self.timings.event();
            match event {
                Event::Ready {
                    directory,
                    drafts,
                    settings,
                } => {
                    self.directory = directory;
                    if self.repeater.tabs.is_empty()
                        && let Some(draft) = drafts.first()
                    {
                        self.open_repeater(draft.clone());
                    }
                    self.drafts = drafts;
                    self.settings = settings;
                    theme(ctx, self.settings.dark);
                    self.connection.ready = true;
                    self.refresh_logcat_devices();
                }
                Event::ProjectOpened { request_id, result } => {
                    self.project_opened(request_id, result)
                }
                Event::ProjectDevices { request_id, result } => {
                    self.project_devices(request_id, result)
                }
                Event::Runner(event) => self.runner_event(event),
                Event::Logcat(event) => self.logcat_event(event),
                Event::Gps(event) => self.gps_event(event),
                Event::Connected(session) => {
                    self.logcat_session_reset();
                    self.connection.connected(&session);
                    self.uri = session.uri.clone();
                    self.preferences.id = session
                        .hello
                        .providers
                        .iter()
                        .find(|p| p.kind == "preferences")
                        .map(|p| p.id.clone())
                        .unwrap_or_default();
                    let drift_id = session
                        .hello
                        .providers
                        .iter()
                        .find(|p| p.kind == "drift")
                        .map(|p| p.id.clone())
                        .unwrap_or_default();
                    self.notice(false, format!("Connected to {}", session.hello.name));
                    self.session = Some(session);
                    self.refresh_logcat_devices();
                    if !self.preferences.id.is_empty() {
                        self.provider("prefs.list", "preferences.list", serde_json::json!({}));
                    }
                    self.drift_database(drift_id);
                    self.refresh_drift_databases();
                }
                Event::DriftDatabasesChanged { isolate } => {
                    if self.session.as_ref().is_some_and(|s| s.isolate == isolate) {
                        self.refresh_drift_databases();
                    }
                }
                Event::Disconnected => {
                    self.session = None;
                    self.connection.disconnected();
                    self.drift.invalidate();
                    self.pending.clear();
                    self.rule.enabled = false;
                }
                Event::ConnectionFailed(error) => {
                    self.connection.connecting = false;
                    self.notice(true, error);
                }
                Event::Discovered(report) => {
                    self.connection.scanning = false;
                    self.connection.report = report;
                    self.connection.next_scan = Instant::now() + Duration::from_secs(5);
                }
                Event::Isolates(isolates) => self.isolates = isolates,
                Event::History {
                    request_id,
                    rows,
                    total,
                    query,
                    offset,
                } => {
                    if self.network_state.clearing
                        || request_id != self.network_state.history_request
                        || query != self.query
                        || offset != self.offset
                    {
                        continue;
                    }
                    if let Some(current) = &self.selected
                        && let Some(updated) = rows.iter().find(|r| r.id == current.id)
                    {
                        self.selected = Some(updated.clone());
                    }
                    self.rows = rows;
                    self.total = total;
                    if self.offset > 0 && self.offset >= self.total {
                        self.offset = self.total.saturating_sub(1) / 200 * 200;
                        self.refresh();
                    }
                }
                Event::HistoryCleared { target, result } => {
                    self.network_state.clearing = false;
                    match result {
                        Ok(removed) => {
                            self.network_state.checked.retain(|id| !target.contains(id));
                            if self
                                .selected
                                .as_ref()
                                .is_some_and(|e| target.contains(&e.id))
                            {
                                self.selected = None;
                            }
                            self.action.retain(|(e, _)| !target.contains(&e.id));
                            self.rows.clear();
                            self.bodies.clear();
                            self.body_order.clear();
                            self.requested.clear();
                            self.network_state.reset_detail();
                            if matches!(target, HistoryTarget::All) {
                                self.offset = 0;
                                self.total = 0;
                            }
                            self.notice(false, format!("Cleared {removed} network item(s)"));
                        }
                        Err(error) => self.notice(true, error),
                    }
                    self.refresh();
                }
                Event::Changed => {
                    self.refresh_at
                        .get_or_insert(Instant::now() + Duration::from_millis(100));
                    self.requested.retain(|id| self.bodies.contains_key(id));
                }
                Event::Body { id, content } => {
                    self.requested.remove(&id);
                    let selected_body = self.selected.as_ref().is_some_and(|e| {
                        e.request.body.as_ref().is_some_and(|b| b.id == id)
                            || e.response
                                .as_ref()
                                .and_then(|r| r.body.as_ref())
                                .is_some_and(|b| b.id == id)
                    });
                    let action_body = self
                        .action
                        .iter()
                        .any(|(e, action)| action.body_id(e) == Some(id.as_str()));
                    if (self.network_state.clearing || (!selected_body && !action_body))
                        && self.repeater.body_id() != Some(id.as_str())
                    {
                        continue;
                    }
                    self.body_order.retain(|key| key != &id);
                    self.body_order.push_back(id.clone());
                    self.bodies.insert(id.clone(), content);
                    while self.body_order.len() > 3 {
                        if let Some(old) = self.body_order.pop_front() {
                            self.bodies.remove(&old);
                        }
                    }
                    while let Some(index) = self
                        .action
                        .iter()
                        .position(|(e, action)| action.body_id(e) == Some(id.as_str()))
                    {
                        let (exchange, action) = self.action.remove(index).expect("queued action");
                        self.request_action(exchange, action, ctx);
                    }
                }
                Event::Pending(pending) => self.pending = pending,
                Event::Provider { tag, value } => {
                    if tag.starts_with("drift.") {
                        self.drift_result(&tag, Ok(value));
                    } else if tag == "prefs.list" {
                        self.preferences.result = value;
                    } else {
                        self.provider("prefs.list", "preferences.list", serde_json::json!({}));
                    }
                }
                Event::ProviderFailed { tag, error } => {
                    if tag.starts_with("drift.") {
                        self.drift_result(&tag, Err(error));
                    } else {
                        self.notice(true, error);
                    }
                }
                Event::Drafts(drafts) => self.drafts = drafts,
                event @ (Event::ReplayFinished { .. } | Event::RepeaterLoaded { .. }) => {
                    self.repeater_event(event)
                }
                Event::Notice(text) => self.notice(false, text),
                Event::Error(text) => self.notice(true, text),
            }
        }
        if !self.worker.events.is_empty() {
            ctx.request_repaint();
        }
        if self.refresh_at.is_some_and(|at| at <= Instant::now()) {
            self.refresh_at = None;
            self.refresh();
        }
        if let Some(at) = self.refresh_at {
            ctx.request_repaint_after(at.saturating_duration_since(Instant::now()));
        }
    }
    pub fn request_action(
        &mut self,
        exchange: Exchange,
        action: RequestAction,
        ctx: &egui::Context,
    ) {
        use base64::Engine;
        if self.action.len() >= 32
            && action
                .body_id(&exchange)
                .is_some_and(|id| !self.bodies.contains_key(id))
        {
            self.notice(
                true,
                "Body actions are busy. Try again when loading finishes.".into(),
            );
            return;
        }
        if matches!(action, RequestAction::Response) {
            let Some(response) = &exchange.response else {
                self.notice(true, "No response received yet".into());
                return;
            };
            let text = if let Some(body) = &response.body {
                let Some(content) = self.bodies.get(&body.id) else {
                    self.load(&body.id);
                    self.action.push_back((exchange, action));
                    return;
                };
                content.text.as_str()
            } else {
                ""
            };
            ctx.copy_text(format!(
                "HTTP/1.1 {}\r\n{}\r\n\r\n{}",
                response.status,
                headers_text(&response.headers).replace('\n', "\r\n"),
                text
            ));
            return;
        }
        let mut draft = Draft {
            id: uuid::Uuid::new_v4().to_string(),
            name: format!("{} {}", exchange.request.method, exchange.request.url),
            method: exchange.request.method.clone(),
            url: exchange.request.url.clone(),
            headers: exchange.request.headers.clone(),
            client_id: exchange.client_id.clone(),
            ..Default::default()
        };
        if let Some(body) = &exchange.request.body {
            if !body.complete {
                self.notice(
                    true,
                    "This capture is truncated; an exact replay is unavailable.".into(),
                );
                return;
            }
            if let Some(content) = self.bodies.get(&body.id) {
                match String::from_utf8(content.bytes.clone()) {
                    Ok(text) => draft.body = text,
                    Err(_) => {
                        draft.body =
                            base64::engine::general_purpose::STANDARD.encode(&content.bytes);
                        draft.body_base64 = true;
                    }
                }
            } else {
                self.load(&body.id);
                self.action.push_back((exchange, action));
                return;
            }
        }
        match action {
            RequestAction::Repeater => {
                self.send(Command::SaveDraft(draft.clone()));
                self.open_repeater(draft);
                self.tab = Tab::Repeater;
            }
            RequestAction::Curl => {
                if draft.body_base64 {
                    self.notice(
                        true,
                        "Binary payload: send to Repeater or export HAR to preserve its bytes."
                            .into(),
                    );
                } else {
                    ctx.copy_text(curl(&draft));
                    self.notice(false, "cURL copied".into());
                }
            }
            RequestAction::Raw => {
                if draft.body_base64 {
                    self.notice(
                        true,
                        "Binary payload: use HAR export to preserve its bytes.".into(),
                    );
                } else {
                    ctx.copy_text(format!(
                        "{} {} HTTP/1.1\r\n{}\r\n\r\n{}",
                        draft.method,
                        draft.url,
                        headers_text(&draft.headers).replace('\n', "\r\n"),
                        draft.body
                    ));
                }
            }
            RequestAction::Response => unreachable!("response handled above"),
        }
    }
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.image((self.logo.id(), egui::vec2(32.0, 32.0)));
            ui.vertical(|ui| {
                ui.strong("Flutter");
                ui.weak("Inspector");
            });
        });
        ui.add_space(16.0);
        ui.label(RichText::new("WORKSPACE").size(11.0).weak());
        ui.add_space(10.0);
        for (tab, label) in [
            (Tab::Network, "Network"),
            (Tab::Repeater, "Repeater"),
            (Tab::Intercept, "Intercept"),
            (Tab::Preferences, "Preferences"),
            (Tab::Drift, "Drift"),
            (Tab::Projects, "Projects"),
            (Tab::Settings, "Settings"),
            (Tab::Gps, "GPS"),
        ] {
            let text = if tab == Tab::Intercept && !self.pending.is_empty() {
                format!("{label}   {}", self.pending.len())
            } else {
                label.into()
            };
            ui.add_sized(
                [ui.available_width(), 34.0],
                egui::Button::selectable(self.tab == tab, text),
            )
            .clicked()
            .then(|| self.tab = tab);
        }
        ui.add_space(24.0);
        ui.separator();
        ui.add_space(10.0);
        if let Some(session) = &self.session {
            ui.colored_label(Color32::from_rgb(102, 204, 180), "●  Connected");
            ui.label(&session.hello.name);
            ui.small(format!(
                "{} available providers",
                session.hello.providers.len()
            ));
        } else {
            ui.weak("○  Offline");
            ui.small("History and desktop replay remain available.");
        }
        ui.add_space(20.0);
        ui.small("Original headers and bodies are saved locally.");
    }
    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.add_space(12.0);
        ui.label("Storage");
        ui.monospace(self.directory.display().to_string());
        ui.label("History is saved automatically. Pinned exchanges and saved Repeater drafts survive retention cleanup.");
        ui.horizontal(|ui| {
            ui.label("Retention (days)");
            ui.add(egui::DragValue::new(&mut self.settings.retention_days).range(1..=3650));
        });
        let mut quota = self.settings.quota_bytes as f64 / 1024.0 / 1024.0 / 1024.0;
        ui.horizontal(|ui| {
            ui.label("Storage quota (GiB)");
            ui.add(
                egui::DragValue::new(&mut quota)
                    .range(0.01..=1000.0)
                    .speed(0.1),
            );
        });
        self.settings.quota_bytes = (quota * 1024.0 * 1024.0 * 1024.0) as u64;
        if ui
            .checkbox(&mut self.settings.dark, "Dark appearance")
            .changed()
        {
            theme(ui.ctx(), self.settings.dark);
        }
        if ui.button("Save settings").clicked() {
            self.send(Command::SaveSettings(self.settings.clone()));
        }
        ui.add_space(20.0);
        ui.label(
            "Capture: up to 20 MiB per body. Larger captures carry an explicit truncation marker.",
        );
        ui.label(
            "Credentials, cookies and payloads are stored as their original plaintext values.",
        );
        ui.label("Auto observe discovers local desktop apps and Android apps through ADB. Manual URI remains available for iOS and remote sessions.");
        ui.label("Interception acts inside registered Dio clients; it does not change system proxy or TLS settings.");
        ui.add_space(20.0);
        ui.label("Activity");
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (error, text) in self.notices.iter().rev() {
                ui.colored_label(
                    if *error {
                        Color32::LIGHT_RED
                    } else {
                        ui.visuals().text_color()
                    },
                    text,
                );
            }
        });
    }
    fn import_window(&mut self, ctx: &egui::Context) {
        if !self.import_open {
            return;
        }
        let mut open = true;
        egui::Window::new("Import request")
            .open(&mut open)
            .default_width(650.0)
            .show(ctx, |ui| {
                ui.label("Paste a cURL command or raw HTTP request.");
                ui.add(
                    egui::TextEdit::multiline(&mut self.import_text)
                        .code_editor()
                        .desired_rows(14)
                        .desired_width(f32::INFINITY),
                );
                if ui.button("Open in Repeater").clicked() {
                    let result = if self.import_text.trim_start().starts_with("curl ") {
                        inspector_domain::import::parse_curl(&self.import_text)
                    } else {
                        inspector_domain::import::parse_raw(&self.import_text)
                    };
                    match result {
                        Ok(mut draft) => {
                            draft.id = uuid::Uuid::new_v4().to_string();
                            self.send(Command::SaveDraft(draft.clone()));
                            self.open_repeater(draft);
                            self.tab = Tab::Repeater;
                            self.import_open = false;
                        }
                        Err(e) => self.notice(true, e),
                    }
                }
            });
        self.import_open &= open;
    }
}
impl eframe::App for InspectorApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.gps_tick();
        let started = Instant::now();
        // eframe also runs logic for hidden windows. Draining only in ui() leaves
        // the worker queue full while minimized or on an inactive workspace.
        self.events(ctx);
        self.connection_tick(ctx);
        if let Ok(seconds) = std::env::var("INSPECTOR_SMOKE_SECONDS")
            .unwrap_or_default()
            .parse::<u64>()
        {
            if self.started.elapsed() > Duration::from_secs(seconds) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        self.timings.record_logic(started.elapsed());
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let frame_started = Instant::now();
        let ctx = ui.ctx().clone();
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Q)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        egui::Panel::top("connection").show(ui, |ui| self.connection_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.small(format!("{} exchanges", self.total));
                ui.separator();
                if self.project.active {
                    if ui.small_button("Projects").clicked() {
                        self.tab = Tab::Projects;
                    }
                    for (label, command) in [
                        ("Hot reload", RunnerCommand::Reload),
                        ("Hot restart", RunnerCommand::Restart),
                    ] {
                        if ui
                            .add_enabled(self.project.ready, egui::Button::new(label).small())
                            .clicked()
                        {
                            self.send(Command::Runner(command));
                        }
                    }
                    if ui.small_button("Stop run").clicked() {
                        self.send(Command::Runner(RunnerCommand::Stop));
                    }
                    ui.separator();
                }
                if let Some((error, text)) = self.notices.back() {
                    ui.add(
                        egui::Label::new(RichText::new(text).size(12.0).color(if *error {
                            Color32::LIGHT_RED
                        } else {
                            ui.visuals().weak_text_color()
                        }))
                        .truncate(),
                    );
                }
            });
        });
        egui::Panel::bottom("logcat")
            .default_size(220.0)
            .min_size(115.0)
            .max_size((ui.available_height() - 150.0).max(115.0))
            .resizable(true)
            .show(ui, |ui| self.logcat_ui(ui));
        egui::Panel::left("navigation")
            .default_size(175.0)
            .resizable(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("navigation_scroll")
                    .show(ui, |ui| self.sidebar(ui));
            });
        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Projects => self.project_ui(ui),
            Tab::Network => self.network_ui(ui),
            Tab::Repeater => self.repeater_ui(ui),
            Tab::Intercept => self.intercept_ui(ui),
            Tab::Preferences => self.preferences_ui(ui),
            Tab::Drift => self.drift_ui(ui),
            Tab::Settings => self.settings_ui(ui),
            Tab::Gps => self.gps_ui(ui),
        });
        self.import_window(&ctx);
        self.timings.record(frame_started.elapsed());
    }
}
impl Drop for InspectorApp {
    fn drop(&mut self) {
        self.persist_repeater();
        let _ = self.worker.commands.try_send(Command::Shutdown);
    }
}
fn theme(ctx: &egui::Context, dark: bool) {
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    if dark {
        visuals.override_text_color = Some(Color32::from_rgb(201, 211, 222));
        visuals.panel_fill = Color32::from_rgb(22, 26, 33);
        visuals.window_fill = Color32::from_rgb(28, 33, 41);
        visuals.extreme_bg_color = Color32::from_rgb(16, 20, 26);
        visuals.faint_bg_color = Color32::from_rgb(27, 32, 40);
        visuals.selection.bg_fill = Color32::from_rgb(35, 78, 78);
        visuals.selection.stroke.color = Color32::from_rgb(115, 222, 193);
    }
    ctx.set_visuals(visuals);
    ctx.style_mut_of(
        if dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        },
        |style| {
            style.interaction.selectable_labels = false;
            style.spacing.item_spacing = egui::vec2(8.0, 8.0);
            style.spacing.button_padding = egui::vec2(10.0, 6.0);
        },
    );
}
