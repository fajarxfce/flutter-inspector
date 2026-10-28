use crate::{app::InspectorApp, http_view, viewer::Viewer};
use eframe::egui::{self, Color32, RichText};
use inspector_application::{Command, Event};
use inspector_domain::*;
use std::time::{Duration, Instant};

pub struct RepeaterTab {
    pub draft: Draft,
    pub dirty: Option<Instant>,
    pub run_id: Option<String>,
    pub running: bool,
    pub result: Option<Exchange>,
    pub error: Option<String>,
    section: usize,
    response_section: usize,
    params: Vec<Header>,
}
impl RepeaterTab {
    fn new(draft: Draft) -> Self {
        let params = http_view::query(&draft.url);
        Self {
            draft,
            dirty: None,
            run_id: None,
            running: false,
            result: None,
            error: None,
            section: 0,
            response_section: 0,
            params,
        }
    }
}
#[derive(Default)]
pub struct Repeater {
    pub tabs: Vec<RepeaterTab>,
    active: usize,
    viewer: Viewer,
}
impl Repeater {
    pub fn active(&self) -> Option<&RepeaterTab> {
        self.tabs.get(self.active)
    }
    pub fn body_id(&self) -> Option<&str> {
        self.active()?
            .result
            .as_ref()?
            .response
            .as_ref()?
            .body
            .as_ref()
            .map(|b| b.id.as_str())
    }
    pub fn open(&mut self, draft: Draft) -> bool {
        self.viewer = Viewer::default();
        if let Some(index) = self.tabs.iter().position(|tab| tab.draft.id == draft.id) {
            self.active = index;
            false
        } else {
            self.tabs.push(RepeaterTab::new(draft));
            self.active = self.tabs.len() - 1;
            true
        }
    }
    fn select(&mut self, index: usize) {
        self.active = index;
        self.viewer = Viewer::default();
    }
    pub fn result(&mut self, draft_id: &str, run_id: &str, result: Result<Exchange, String>) {
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|t| t.draft.id == draft_id && t.run_id.as_deref() == Some(run_id))
        else {
            return;
        };
        tab.running = false;
        match result {
            Ok(result) => {
                tab.error = result.error.clone();
                tab.result = Some(result);
            }
            Err(error) => tab.error = Some(error),
        }
    }
}

impl InspectorApp {
    pub fn persist_repeater(&mut self) {
        let ready: Vec<_> = self
            .repeater
            .tabs
            .iter_mut()
            .filter_map(|tab| tab.dirty.take().map(|_| tab.draft.clone()))
            .collect();
        for draft in ready {
            self.send(Command::SaveDraft(draft));
        }
    }
    pub fn repeater_tick(&mut self, ctx: &egui::Context) {
        let mut ready = Vec::new();
        for tab in &mut self.repeater.tabs {
            if let Some(at) = tab.dirty {
                if at.elapsed() >= Duration::from_millis(600) {
                    tab.dirty = None;
                    ready.push(tab.draft.clone());
                } else {
                    ctx.request_repaint_after(
                        Duration::from_millis(600).saturating_sub(at.elapsed()),
                    );
                }
            }
        }
        for draft in ready {
            self.send(Command::SaveDraft(draft));
        }
    }
    pub fn open_repeater(&mut self, draft: Draft) {
        let id = draft.id.clone();
        if self.repeater.open(draft) {
            self.send(Command::LoadRepeater(id));
        }
    }
    pub fn repeater_event(&mut self, event: Event) {
        match event {
            Event::ReplayFinished {
                draft_id,
                run_id,
                result,
            } => self.repeater.result(&draft_id, &run_id, result),
            Event::RepeaterLoaded { draft_id, result } => {
                if let Some(tab) = self
                    .repeater
                    .tabs
                    .iter_mut()
                    .find(|t| t.draft.id == draft_id && t.run_id.is_none())
                {
                    match result {
                        Ok(result) => {
                            tab.result = result;
                            if let Some(e) = &mut tab.result {
                                if e.state == ExchangeState::Pending {
                                    e.state = ExchangeState::Cancelled;
                                    e.error = Some(
                                        "Previous send was interrupted. Send again to retry."
                                            .into(),
                                    );
                                }
                                tab.error = e.error.clone();
                            }
                        }
                        Err(error) => tab.error = Some(error),
                    }
                }
            }
            _ => {}
        }
    }
    fn new_repeater(&mut self) {
        let draft = Draft {
            id: uuid::Uuid::new_v4().to_string(),
            name: format!("Request {}", self.drafts.len() + 1),
            ..Default::default()
        };
        self.send(Command::SaveDraft(draft.clone()));
        self.open_repeater(draft);
    }
    fn send_repeater(&mut self) {
        let Some(tab) = self.repeater.tabs.get_mut(self.repeater.active) else {
            return;
        };
        if tab.running {
            return;
        }
        if let Err(error) = parse_headers(&headers_text(&tab.draft.headers)) {
            tab.error = Some(error);
            return;
        }
        let run_id = uuid::Uuid::new_v4().to_string();
        let command = Command::Replay {
            draft: tab.draft.clone(),
            run_id: run_id.clone(),
        };
        match self.worker.commands.try_send(command) {
            Ok(()) => {
                tab.run_id = Some(run_id);
                tab.running = true;
                tab.result = None;
                tab.error = None;
            }
            Err(_) => tab.error = Some("Worker busy; try sending again".into()),
        }
    }
    pub fn repeater_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.heading("Repeater");
            if ui.button("+ New tab").clicked() {
                self.new_repeater();
            }
            if ui.button("Paste cURL / HTTP").clicked() {
                self.import_open = true;
            }
            let mut open = None;
            ui.menu_button("Saved requests", |ui| {
                if self.drafts.is_empty() {
                    ui.weak("No saved requests yet.");
                }
                for draft in &self.drafts {
                    if ui.button(&draft.name).clicked() {
                        open = Some(draft.clone());
                        ui.close();
                    }
                }
            });
            if let Some(draft) = open {
                self.open_repeater(draft);
            }
        });
        ui.small("Each tab has its own request and response. Edits and sends leave the original Network capture intact.");
        let mut selected = None;
        let mut close = None;
        let mut delete = None;
        egui::ScrollArea::horizontal()
            .id_salt("repeater-tabs")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (index, tab) in self.repeater.tabs.iter().enumerate() {
                        ui.push_id(&tab.draft.id, |ui| {
                            let name: String = tab.draft.name.chars().take(28).collect();
                            let label = if tab.running {
                                format!("{name} · sending")
                            } else {
                                name
                            };
                            let response = ui
                                .selectable_label(index == self.repeater.active, label)
                                .on_hover_text(&tab.draft.url);
                            if response.clicked() {
                                selected = Some(index);
                            }
                            response.context_menu(|ui| {
                                if ui.button("Delete saved request and response").clicked() {
                                    delete = Some(index);
                                    ui.close();
                                }
                            });
                            if ui
                                .small_button("×")
                                .on_hover_text("Close tab; saved request stays available")
                                .clicked()
                            {
                                close = Some(index);
                            }
                        });
                    }
                });
            });
        if let Some(index) = selected {
            self.repeater.select(index);
        }
        if let Some(index) = delete.or(close) {
            self.persist_repeater();
            let id = self.repeater.tabs[index].draft.id.clone();
            if self.repeater.tabs[index].running {
                self.send(Command::CancelReplay(id.clone()));
            }
            if delete.is_some() {
                self.send(Command::DeleteDraft(id));
            }
            self.repeater.tabs.remove(index);
            self.repeater.active = if index < self.repeater.active {
                self.repeater.active - 1
            } else {
                self.repeater
                    .active
                    .min(self.repeater.tabs.len().saturating_sub(1))
            };
            self.repeater.viewer = Viewer::default();
        }
        if self.repeater.tabs.is_empty() {
            ui.add_space(30.0);
            ui.heading("Start a Repeater request");
            ui.label("Right-click a Network row → Send to Repeater, open a saved request, or create a new tab.");
            return;
        }
        ui.separator();
        let index = self.repeater.active;
        let tab = &mut self.repeater.tabs[index];
        let mut dirty = false;
        let mut send = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            dirty |= ui
                .add(egui::TextEdit::singleline(&mut tab.draft.method).desired_width(64.0))
                .changed();
            let url = ui.add(
                egui::TextEdit::singleline(&mut tab.draft.url)
                    .hint_text("https://…")
                    .desired_width((ui.available_width() - 160.0).max(100.0)),
            );
            if url.changed() {
                tab.params = http_view::query(&tab.draft.url);
                dirty = true;
            }
            send = ui
                .add_enabled(!tab.running, egui::Button::new("Send"))
                .clicked();
            cancel = ui
                .add_enabled(tab.running, egui::Button::new("Cancel"))
                .clicked();
            if tab.running {
                ui.spinner();
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Name");
            dirty |= ui
                .add(egui::TextEdit::singleline(&mut tab.draft.name).desired_width(190.0))
                .changed();
            dirty |= ui
                .checkbox(&mut tab.draft.follow_redirects, "Follow redirects")
                .changed();
            let has_dio = self
                .session
                .as_ref()
                .is_some_and(|s| s.hello.providers.iter().any(|p| p.kind == "dio"));
            let toggle = ui
                .add_enabled(
                    has_dio || tab.draft.via_app,
                    egui::Checkbox::new(&mut tab.draft.via_app, "Via app Dio"),
                )
                .on_disabled_hover_text(
                    "Optional: requires the Dio bridge. Default sends directly from this desktop.",
                )
                .changed();
            dirty |= toggle;
            if toggle
                && tab.draft.via_app
                && let Some(session) = &self.session
                && !session
                    .hello
                    .providers
                    .iter()
                    .any(|p| p.kind == "dio" && p.id == tab.draft.client_id)
                && let Some(provider) = session.hello.providers.iter().find(|p| p.kind == "dio")
            {
                tab.draft.client_id = provider.id.clone();
            }
            if tab.draft.via_app {
                egui::ComboBox::from_id_salt(("replay-client", &tab.draft.id))
                    .selected_text(&tab.draft.client_id)
                    .show_ui(ui, |ui| {
                        if let Some(session) = &self.session {
                            for provider in
                                session.hello.providers.iter().filter(|p| p.kind == "dio")
                            {
                                dirty |= ui
                                    .selectable_value(
                                        &mut tab.draft.client_id,
                                        provider.id.clone(),
                                        &provider.name,
                                    )
                                    .changed();
                            }
                        }
                    });
            }
            if ui.small_button("Copy cURL").clicked() {
                if tab.draft.body_base64 {
                    tab.error = Some("Binary body: keep it in Repeater to preserve bytes.".into());
                } else {
                    ui.ctx().copy_text(curl(&tab.draft));
                }
            }
        });
        if tab.draft.via_app {
            ui.small("App interceptors run again and can change authentication or request fields.");
        }
        if let Some(error) = &tab.error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        if dirty {
            tab.dirty = Some(Instant::now());
        }
        let id = tab.draft.id.clone();
        if send {
            self.send_repeater();
        }
        if cancel {
            self.send(Command::CancelReplay(id));
        }
        let width = (ui.available_width() * 0.48).max(230.0);
        egui::Panel::left("repeater-request")
            .default_size(width)
            .resizable(true)
            .min_size(230.0)
            .show(ui, |ui| self.repeater_request(ui));
        egui::CentralPanel::default().show(ui, |ui| self.repeater_response(ui));
    }
    fn repeater_request(&mut self, ui: &mut egui::Ui) {
        let tab = &mut self.repeater.tabs[self.repeater.active];
        ui.strong("Request");
        let mut dirty = false;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut tab.section, 0, "Body");
            ui.selectable_value(
                &mut tab.section,
                1,
                format!("Headers ({})", tab.draft.headers.len()),
            );
            ui.selectable_value(
                &mut tab.section,
                2,
                format!("Parameters ({})", tab.params.len()),
            );
        });
        ui.separator();
        if tab.section == 1 {
            dirty |= edit_pairs(ui, &mut tab.draft.headers, "header");
        } else if tab.section == 2 {
            if edit_pairs(ui, &mut tab.params, "parameter") {
                match url::Url::parse(&tab.draft.url) {
                    Ok(mut url) => {
                        if tab.params.is_empty() {
                            url.set_query(None);
                        } else {
                            url.query_pairs_mut()
                                .clear()
                                .extend_pairs(tab.params.iter().map(|p| (&p.name, &p.value)));
                        }
                        tab.draft.url = url.into();
                        dirty = true;
                    }
                    Err(error) => tab.error = Some(error.to_string()),
                }
            }
        } else {
            dirty |= ui
                .checkbox(&mut tab.draft.body_base64, "Body is base64 (binary)")
                .changed();
            egui::ScrollArea::both()
                .id_salt(("repeater-body", &tab.draft.id))
                .show(ui, |ui| {
                    if tab.draft.body.len() > 512 * 1024 {
                        ui.weak("Large payload: preview only. Send preserves all bytes.");
                        ui.add(
                            egui::Label::new(
                                RichText::new(
                                    tab.draft.body.chars().take(4096).collect::<String>(),
                                )
                                .monospace(),
                            )
                            .selectable(true),
                        );
                    } else {
                        dirty |= crate::syntax::editor(ui, &mut tab.draft.body, 18).changed();
                    }
                });
        }
        if dirty {
            tab.dirty = Some(Instant::now());
        }
    }
    fn repeater_response(&mut self, ui: &mut egui::Ui) {
        let index = self.repeater.active;
        ui.strong("Response");
        let tab = &mut self.repeater.tabs[index];
        let Some(exchange) = tab.result.clone() else {
            ui.add_space(20.0);
            ui.label(if tab.running {
                "Sending this tab's request…"
            } else {
                "Send this request to see its response here."
            });
            return;
        };
        ui.horizontal_wrapped(|ui| {
            if let Some(response) = &exchange.response {
                ui.colored_label(
                    if response.status >= 400 {
                        Color32::LIGHT_RED
                    } else {
                        Color32::from_rgb(115, 222, 193)
                    },
                    format!("Status {}", response.status),
                );
            }
            ui.label(format!("{} ms", exchange.duration_ms));
            ui.small(http_view::timestamp(exchange.started_at, false));
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut tab.response_section, 0, "Body");
            ui.selectable_value(&mut tab.response_section, 1, "Headers");
        });
        ui.separator();
        let Some(response) = &exchange.response else {
            ui.weak("No response received.");
            return;
        };
        if tab.response_section == 1 {
            http_view::headers(
                ui,
                ("repeater-response-headers", &tab.draft.id),
                &response.headers,
            );
            return;
        }
        if let Some(body) = &response.body {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{} bytes · {}", body.size, body.content_type));
                if ui
                    .add_enabled(
                        self.bodies.contains_key(&body.id),
                        egui::Button::new("Copy response").small(),
                    )
                    .clicked()
                    && let Some(content) = self.bodies.get(&body.id)
                {
                    ui.ctx().copy_text(format!(
                        "HTTP/1.1 {}\r\n{}\r\n\r\n{}",
                        response.status,
                        headers_text(&response.headers).replace('\n', "\r\n"),
                        content.text
                    ));
                }
            });
            if !body.complete {
                ui.colored_label(Color32::YELLOW, "Truncated / interrupted");
            }
            if let Some(content) = self.bodies.get(&body.id).cloned() {
                self.repeater.viewer.show(ui, &body.id, content);
            } else {
                self.load(&body.id);
                ui.weak("Loading response body…");
                if ui.button("Retry body").clicked() {
                    self.requested.remove(&body.id);
                    self.load(&body.id);
                }
            }
        } else {
            ui.weak("Empty response body.");
        }
    }
}

fn edit_pairs(ui: &mut egui::Ui, values: &mut Vec<Header>, kind: &str) -> bool {
    let mut changed = false;
    if ui.button(format!("+ Add {kind}")).clicked() {
        values.push(Header::default());
        changed = true;
    }
    ui.small("Duplicate names are preserved. Remove a row with ×.");
    let mut remove = None;
    egui::ScrollArea::vertical()
        .id_salt(("edit-pairs", kind))
        .show_rows(ui, 30.0, values.len(), |ui, range| {
            for index in range {
                ui.push_id((kind, index), |ui| {
                    ui.horizontal(|ui| {
                        let width = ui.available_width();
                        changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut values[index].name)
                                    .hint_text("Name")
                                    .desired_width((width * 0.35).max(65.0)),
                            )
                            .changed();
                        changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut values[index].value)
                                    .hint_text("Value")
                                    .desired_width((width * 0.65 - 40.0).max(75.0)),
                            )
                            .changed();
                        if ui.small_button("×").clicked() {
                            remove = Some(index);
                        }
                    });
                });
            }
        });
    if let Some(index) = remove {
        values.remove(index);
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn responses_cannot_cross_tabs_or_replace_a_newer_send() {
        let mut state = Repeater::default();
        for id in ["one", "two"] {
            state.open(Draft {
                id: id.into(),
                ..Default::default()
            });
        }
        state.tabs[0].run_id = Some("new".into());
        state.tabs[0].running = true;
        state.tabs[1].run_id = Some("two-send".into());
        state.tabs[1].running = true;
        state.result("one", "old", Ok(Exchange::default()));
        assert!(state.tabs[0].running);
        assert!(state.tabs[0].result.is_none());
        state.result(
            "one",
            "new",
            Ok(Exchange {
                id: "response-one".into(),
                ..Default::default()
            }),
        );
        assert_eq!(state.tabs[0].result.as_ref().unwrap().id, "response-one");
        assert!(state.tabs[1].running);
        assert!(state.tabs[1].result.is_none());
        state.result("two", "two-send", Err("timeout".into()));
        assert_eq!(state.tabs[1].error.as_deref(), Some("timeout"));
        assert_eq!(state.active, 1);
    }
}
