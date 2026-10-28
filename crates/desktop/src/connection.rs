use crate::app::InspectorApp;
use eframe::egui::{self, Color32, RichText};
use inspector_application::{BridgeStatus, Command, DiscoveredApp, DiscoveryReport, Session};
use std::time::{Duration, Instant};

pub struct ConnectionUi {
    pub report: DiscoveryReport,
    pub scanning: bool,
    pub connecting: bool,
    pub ready: bool,
    pub next_scan: Instant,
    retry_after: Instant,
    preferred: Option<(String, String)>,
    manual: bool,
    attention_uri: Option<String>,
    setup_open: bool,
}
impl Default for ConnectionUi {
    fn default() -> Self {
        Self {
            report: DiscoveryReport::default(),
            scanning: false,
            connecting: false,
            ready: false,
            next_scan: Instant::now(),
            retry_after: Instant::now(),
            preferred: None,
            manual: false,
            attention_uri: None,
            setup_open: false,
        }
    }
}
impl ConnectionUi {
    fn should_scan(&self, auto_observe: bool, connected: bool, now: Instant) -> bool {
        auto_observe
            && self.ready
            && !connected
            && !self.connecting
            && !self.scanning
            && now >= self.next_scan
    }
    pub fn disconnected(&mut self) {
        // Connect intentionally emits Disconnected before replacing the session.
        // Preserve the discovered target during that transition.
        if !self.connecting {
            self.report.apps.clear();
            self.next_scan = Instant::now();
            self.retry_after = Instant::now();
        }
    }
    pub fn connected(&mut self, session: &Session) {
        self.connecting = false;
        if let Some(app) = self
            .report
            .apps
            .iter()
            .find(|app| app.uri == session.uri && app.isolate == session.isolate)
        {
            self.preferred = Some((app.name.clone(), app.device.clone()));
        } else {
            // Keep a manual connection from auto-switching to an unrelated app.
            self.preferred = Some((
                session.hello.name.clone(),
                format!("manual:{}", session.uri),
            ));
        }
    }
    fn candidate(&self) -> Option<DiscoveredApp> {
        if self.scanning {
            return None;
        }
        // Multiple matching instances require selection, even when one is busy.
        let mut matching = self.report.apps.iter().filter(|app| {
            app.bridge.connectable()
                && self
                    .preferred
                    .as_ref()
                    .is_none_or(|(name, device)| name == &app.name && device == &app.device)
        });
        let app = matching.next()?;
        if matching.next().is_some() || app.busy {
            return None;
        }
        Some(app.clone())
    }
}
impl InspectorApp {
    pub fn connect_to(&mut self, uri: String, isolate: Option<String>) {
        if self.connection.connecting {
            return;
        }
        match self.worker.commands.try_send(Command::Connect {
            uri: uri.clone(),
            isolate,
        }) {
            Ok(()) => {
                self.uri = uri;
                self.connection.connecting = true;
                self.connection.retry_after = Instant::now() + Duration::from_secs(5);
            }
            Err(error) => self.notice(true, format!("Worker busy: {error}. Try again.")),
        }
    }
    fn discover(&mut self) {
        if self.connection.scanning {
            return;
        }
        if self.worker.commands.try_send(Command::Discover).is_ok() {
            self.connection.scanning = true;
        }
    }
    pub fn connection_tick(&mut self, ctx: &egui::Context) {
        if self.project.active
            && self.session.is_none()
            && !self.connection.connecting
            && Instant::now() >= self.project.retry_at
            && let Some(uri) = self.project.managed_uri.clone()
        {
            self.project.retry_at = Instant::now() + Duration::from_secs(3);
            self.connect_to(uri, None);
        }
        if self.connection.ready && self.settings.auto_observe && !self.project.active {
            if self.connection.should_scan(
                self.settings.auto_observe,
                self.session.is_some(),
                Instant::now(),
            ) {
                self.discover();
            }
            if self.session.is_none()
                && !self.connection.connecting
                && Instant::now() >= self.connection.retry_after
                && let Some(app) = self.connection.candidate()
            {
                self.connection.preferred = Some((app.name, app.device));
                self.connect_to(app.uri, Some(app.isolate));
            }
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
    pub fn connection_bar(&mut self, ui: &mut egui::Ui) {
        let mut selected = None;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("FLUTTER INSPECTOR")
                    .strong()
                    .color(Color32::from_rgb(102, 204, 180)),
            );
            ui.separator();
            let title = if self.connection.connecting {
                "Connecting…".to_owned()
            } else if let Some(session) = &self.session {
                session.hello.name.clone()
            } else if self.connection.report.apps.is_empty() {
                "Waiting for a debug app".into()
            } else if self.connection.report.apps.len() == 1 {
                self.connection.report.apps[0].name.clone()
            } else {
                format!("Select app ({})", self.connection.report.apps.len())
            };
            egui::ComboBox::from_id_salt("discovered_apps")
                .selected_text(title)
                .width(240.0)
                .show_ui(ui, |ui| {
                    for app in &self.connection.report.apps {
                        let current = self
                            .session
                            .as_ref()
                            .is_some_and(|s| s.uri == app.uri && s.isolate == app.isolate);
                        let label = format!(
                            "{} — {}{}",
                            app.name,
                            app.device,
                            match &app.bridge {
                                BridgeStatus::Missing => " (debug APIs unavailable)",
                                BridgeStatus::Unavailable(_) => " (connection issue)",
                                BridgeStatus::Ready if app.busy && !current => " (in use)",
                                BridgeStatus::Ready | BridgeStatus::Native => "",
                            },
                        );
                        if ui
                            .add_enabled(
                                !self.connection.connecting && (!app.busy || current),
                                egui::Button::selectable(current, label),
                            )
                            .clicked()
                            && !current
                        {
                            selected = Some(app.clone());
                        }
                    }
                    if self.connection.report.apps.is_empty() {
                        ui.label("Run a debug app with InspectorBridge initialized.");
                    }
                    for note in &self.connection.report.notes {
                        ui.weak(note);
                    }
                });
            if ui
                .checkbox(&mut self.settings.auto_observe, "Auto observe")
                .on_hover_text("Find and reconnect debug apps while disconnected. Use Refresh to find other apps while connected.")
                .changed()
            {
                if self.session.is_none() {
                    self.connection.preferred = None;
                }
                self.connection.next_scan = Instant::now();
                self.send(Command::SaveSettings(self.settings.clone()));
            }
            if self.connection.scanning {
                ui.spinner();
            } else if ui.button("Refresh").clicked() {
                self.discover();
            }
            if self.session.is_some()
                && !self.connection.connecting
                && ui.button("Disconnect").clicked()
            {
                // An explicit disconnect must stay disconnected.
                self.project.managed_uri = None;
                self.settings.auto_observe = false;
                self.send(Command::SaveSettings(self.settings.clone()));
                self.send(Command::Disconnect);
            }
            ui.toggle_value(&mut self.connection.manual, "Manual URI");
        });
        if let Some(app) = selected {
            if app.bridge.connectable() {
                self.connection.attention_uri = None;
                self.connection.preferred = Some((app.name, app.device));
                self.connect_to(app.uri, Some(app.isolate));
            } else {
                self.connection.attention_uri = Some(app.uri);
            }
        }
        let attention = self
            .connection
            .report
            .apps
            .iter()
            .find(|app| {
                !app.bridge.connectable()
                    && (self.connection.attention_uri.as_ref() == Some(&app.uri)
                        || (self.session.is_none() && self.connection.report.apps.len() == 1))
            })
            .cloned();
        if let Some(app) = attention {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Color32::from_rgb(235, 185, 90), format!("Detected: {}", app.name));
                match app.bridge {
                    BridgeStatus::Missing => {
                        ui.label("Debug APIs are not ready. Resume the app and refresh after initialization.");
                        if ui.button("Setup").clicked() { self.connection.setup_open = true; }
                    }
                    BridgeStatus::Unavailable(error) => {
                        ui.label(format!("Could not inspect VM: {error}"));
                    }
                    BridgeStatus::Ready | BridgeStatus::Native => {}
                }
            });
        }
        let setup = "Network, SharedPreferences and Drift use the app's built-in debug APIs. No Inspector SDK is needed.\n\nOptional: add flutter_inspector_bridge and flutter_inspector_dio to enable intercept and replay through an existing Dio client. See README.md for setup.";
        egui::Window::new("Inspection setup")
            .open(&mut self.connection.setup_open)
            .resizable(true)
            .default_width(600.0)
            .show(ui.ctx(), |ui| {
                ui.label(setup);
                if ui.button("Copy setup instructions").clicked() {
                    ui.ctx().copy_text(setup.into());
                }
            });
        if self.connection.manual {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.uri)
                        .hint_text("http://127.0.0.1:PORT/TOKEN=/")
                        .desired_width((ui.available_width() - 270.0).max(150.0)),
                );
                if ui
                    .add_enabled(!self.connection.connecting, egui::Button::new("Connect"))
                    .clicked()
                {
                    self.connection.preferred = None;
                    self.connect_to(self.uri.trim().into(), None);
                }
                if let Some(session) = &self.session {
                    let mut chosen = session.isolate.clone();
                    egui::ComboBox::from_id_salt("isolate")
                        .selected_text(&session.isolate)
                        .show_ui(ui, |ui| {
                            for (id, name) in &self.isolates {
                                ui.selectable_value(&mut chosen, id.clone(), name);
                            }
                        });
                    if chosen != session.isolate {
                        self.connect_to(self.uri.clone(), Some(chosen));
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app(name: &str, busy: bool) -> DiscoveredApp {
        DiscoveredApp {
            uri: "http://127.0.0.1:123/token=/".into(),
            isolate: "isolates/1".into(),
            name: name.into(),
            device: "USB".into(),
            session_id: "one".into(),
            busy,
            bridge: BridgeStatus::Ready,
        }
    }
    #[test]
    fn automatic_scans_stop_while_connected_and_resume_after_disconnect() {
        let now = Instant::now();
        let mut state = ConnectionUi {
            ready: true,
            next_scan: now,
            ..Default::default()
        };
        assert!(state.should_scan(true, false, now));
        assert!(!state.should_scan(false, false, now));
        assert!(!state.should_scan(true, true, now + Duration::from_secs(60)));
        state.connecting = true;
        assert!(!state.should_scan(true, false, now));
        state.connecting = false;
        state.scanning = true;
        state.report.apps = vec![app("A", false)];
        assert!(!state.should_scan(true, false, now));
        assert!(state.candidate().is_none());
        state.scanning = false;
        state.preferred = Some(("A".into(), "USB".into()));
        state.next_scan = now + Duration::from_secs(60);
        state.disconnected();
        assert!(
            state.report.apps.is_empty(),
            "Do not retry a stale endpoint"
        );
        assert!(state.should_scan(true, false, Instant::now()));
        assert_eq!(state.preferred, Some(("A".into(), "USB".into())));
    }
    #[test]
    fn connecting_preserves_discovery_metadata_until_session_is_established() {
        let mut state = ConnectionUi {
            connecting: true,
            ..Default::default()
        };
        state.report.apps = vec![app("A", false)];
        state.disconnected();
        assert_eq!(state.report.apps.len(), 1);
    }
    #[test]
    fn auto_observe_waits_on_ambiguity_busy_apps_and_missing_selected_app() {
        let mut state = ConnectionUi::default();
        state.report.apps = vec![app("A", false)];
        assert!(state.candidate().is_some());
        state.report.apps.push(app("B", true));
        assert!(state.candidate().is_none());
        state.preferred = Some(("A".into(), "USB".into()));
        assert_eq!(state.candidate().unwrap().name, "A");
        state.report.apps.remove(0);
        assert!(state.candidate().is_none());
        state.preferred = Some(("B".into(), "USB".into()));
        assert!(state.candidate().is_none());
        state.preferred = Some(("A".into(), "USB".into()));
        let mut restarted = app("A", false);
        restarted.uri = "http://127.0.0.1:456/new=/".into();
        restarted.isolate = "isolates/99".into();
        restarted.session_id = "new-session".into();
        state.report.apps.push(restarted);
        assert_eq!(state.candidate().unwrap().isolate, "isolates/99");
        state.report.apps.last_mut().unwrap().bridge = BridgeStatus::Missing;
        assert!(
            state.candidate().is_none(),
            "An unconfigured app must not auto-connect"
        );
        state.report.apps.last_mut().unwrap().bridge = BridgeStatus::Native;
        assert!(
            state.candidate().is_some(),
            "Built-in inspectors do not need SDK registration"
        );
    }
}
