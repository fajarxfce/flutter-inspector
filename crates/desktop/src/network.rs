use crate::app::{InspectorApp, RequestAction};
use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use inspector_application::{Command, HistoryTarget};
use inspector_domain::*;
use serde_json::json;
use std::collections::BTreeSet;
mod detail;

#[derive(Default)]
pub struct NetworkUi {
    pub clearing: bool,
    pub confirm_clear: bool,
    pub history_request: u64,
    pub checked: BTreeSet<String>,
    detail: detail::Detail,
}
impl NetworkUi {
    pub fn reset_detail(&mut self) {
        self.detail = detail::Detail::default();
    }
    fn checked_on_page(&self, rows: &[Exchange]) -> usize {
        rows.iter()
            .filter(|row| self.checked.contains(&row.id))
            .count()
    }
    fn check_page(&mut self, rows: &[Exchange], checked: bool) {
        for row in rows {
            if checked {
                self.checked.insert(row.id.clone());
            } else {
                self.checked.remove(&row.id);
            }
        }
    }
}

impl InspectorApp {
    fn clear_network(&mut self, target: HistoryTarget) {
        if self.network_state.clearing
            || matches!(&target, HistoryTarget::Items(ids) if ids.is_empty())
        {
            return;
        }
        match self
            .worker
            .commands
            .try_send(Command::ClearHistory { target })
        {
            Ok(()) => {
                self.network_state.clearing = true;
                self.network_state.history_request += 1;
            }
            Err(error) => self.notice(true, format!("Worker busy: {error}. Try again.")),
        }
    }
    pub fn network_ui(&mut self, ui: &mut egui::Ui) {
        if self.selected.is_some() {
            egui::Panel::bottom("exchange_detail")
                .default_size(330.0)
                .min_size(160.0)
                .max_size((ui.available_height() - 145.0).max(160.0))
                .resizable(true)
                .show(ui, |ui| self.exchange_ui(ui));
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("Network");
                if ui.small_button("Refresh").clicked() {
                    self.refresh();
                }
                ui.menu_button("Import / export", |ui| {
                    if ui.button("Paste cURL / HTTP").clicked() {
                        self.import_open = true;
                        ui.close();
                    }
                    if ui.button("Import HAR…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("HTTP Archive", &["har", "json"])
                            .pick_file()
                        {
                            self.send(Command::ImportHar(path));
                        }
                        ui.close();
                    }
                    if ui.button("Export history as HAR…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_file_name("capture.har")
                            .save_file()
                        {
                            self.send(Command::ExportHar(path));
                        }
                        ui.close();
                    }
                });
                if ui
                    .add_enabled(!self.network_state.clearing, egui::Button::new("Clear all").small())
                    .clicked()
                {
                    self.network_state.confirm_clear = true;
                }
                let count = self.network_state.checked.len();
                if ui
                    .add_enabled(
                        !self.network_state.clearing && count > 0,
                        egui::Button::new(format!("Clear selected ({count})")).small(),
                    )
                    .on_hover_text("Clear only checked requests, including selections on other pages or hidden by the filter.")
                    .clicked()
                {
                    self.clear_network(HistoryTarget::Items(self.network_state.checked.clone()));
                }
                if self.network_state.clearing {
                    ui.spinner();
                    ui.weak("Clearing…");
                }
            });
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Filter URL, client, m:POST, s:4…")
                        .desired_width(f32::INFINITY),
                )
                .changed()
            {
                self.offset = 0;
                self.refresh();
            }
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(self.offset > 0, egui::Button::new("‹"))
                    .clicked()
                {
                    self.offset = self.offset.saturating_sub(200);
                    self.refresh();
                }
                ui.small(format!(
                    "{}–{} of {}",
                    if self.total == 0 { 0 } else { self.offset + 1 },
                    (self.offset + self.rows.len()).min(self.total),
                    self.total
                ));
                if ui
                    .add_enabled(self.offset + 200 < self.total, egui::Button::new("›"))
                    .clicked()
                {
                    self.offset += 200;
                    self.refresh();
                }
                if !self.network_state.checked.is_empty() {
                    let outside = self.network_state.checked.len()
                        - self.network_state.checked_on_page(&self.rows);
                    ui.weak(format!("{} selected", self.network_state.checked.len()));
                    if outside > 0 {
                        ui.weak(format!("{outside} outside this page"));
                    }
                    if ui
                        .add_enabled(!self.network_state.clearing, egui::Button::new("Deselect all").small())
                        .clicked()
                    {
                        self.network_state.checked.clear();
                    }
                }
            });
            ui.separator();
            if self.rows.is_empty() {
                ui.add_space(50.0);
                ui.heading("Waiting for traffic");
                ui.label("Run your Flutter app in debug mode. New HTTP requests appear after connecting.");
                ui.label("You can also import a HAR. Repeater responses appear in their own tabs.");
                return;
            }
            let mut chosen = None;
            let mut menu_action = None;
            TableBuilder::new(ui)
                .id_salt(("network", self.selected.is_some()))
                .sense(egui::Sense::click())
                .striped(true)
                .resizable(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(28.0))
                .column(Column::initial(58.0))
                .column(Column::initial(65.0))
                .column(Column::remainder().at_least(130.0).resizable(false))
                .column(Column::initial(100.0))
                .column(Column::initial(76.0))
                .column(Column::initial(75.0))
                .header(28.0, |mut header| {
                    header.col(|ui| {
                        let count = self.network_state.checked_on_page(&self.rows);
                        let mut all = count == self.rows.len();
                        let partial = count > 0 && !all;
                        let response = ui
                            .add_enabled(
                                !self.network_state.clearing,
                                egui::Checkbox::without_text(&mut all).indeterminate(partial),
                            )
                            .on_hover_text("Select or deselect requests on this page");
                        if response.changed() {
                            self.network_state.check_page(&self.rows, all);
                        }
                    });
                    for title in ["Method", "Status", "URL", "Time", "Duration", "Source"] {
                        header.col(|ui| {
                            ui.strong(title);
                        });
                    }
                })
                .body(|body| {
                    body.rows(29.0, self.rows.len(), |mut row| {
                        let exchange = &self.rows[row.index()];
                        row.set_selected(
                            self.network_state.checked.contains(&exchange.id)
                                || self.selected.as_ref().is_some_and(|s| s.id == exchange.id),
                        );
                        let mut checkbox_clicked = false;
                        row.col(|ui| {
                            let mut checked = self.network_state.checked.contains(&exchange.id);
                            let response = ui
                                .push_id(&exchange.id, |ui| {
                                    ui.add_enabled(
                                        !self.network_state.clearing,
                                        egui::Checkbox::without_text(&mut checked),
                                    )
                                })
                                .inner
                                .on_hover_text("Select this request for Clear selected");
                            checkbox_clicked = response.clicked();
                            if response.changed() {
                                if checked {
                                    self.network_state.checked.insert(exchange.id.clone());
                                } else {
                                    self.network_state.checked.remove(&exchange.id);
                                }
                            }
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(&exchange.request.method)
                                    .monospace()
                                    .color(Color32::from_rgb(115, 222, 193)),
                            );
                        });
                        row.col(|ui| {
                            let text = exchange
                                .response
                                .as_ref()
                                .map(|r| r.status.to_string())
                                .unwrap_or_else(|| format!("{:?}", exchange.state));
                            let color =
                                if exchange.response.as_ref().is_some_and(|r| r.status >= 400)
                                    || exchange.error.is_some()
                                {
                                    Color32::LIGHT_RED
                                } else {
                                    ui.visuals().text_color()
                                };
                            ui.add(egui::Label::new(RichText::new(text).color(color)).truncate());
                        });
                        row.col(|ui| {
                            ui.add(
                                egui::Label::new(format!(
                                    "{}{}",
                                    if exchange.pinned { "★ " } else { "" },
                                    exchange.request.url
                                ))
                                .truncate(),
                            )
                            .on_hover_text(&exchange.request.url);
                        });
                        row.col(|ui| { ui.label(RichText::new(crate::http_view::timestamp(exchange.started_at,false)).monospace().size(12.0)).on_hover_text(crate::http_view::timestamp(exchange.started_at,true)); });
                        row.col(|ui| {
                            ui.small(format!("{} ms", exchange.duration_ms));
                        });
                        row.col(|ui| {
                            ui.add(egui::Label::new(&exchange.client_id).truncate());
                        });
                        if row.response().clicked() && !checkbox_clicked {
                            chosen = Some(exchange.clone());
                        }
                        row.response().context_menu(|ui| {
                            if let Some(action) = detail::menu(ui, exchange, self.network_state.clearing, None) {
                                menu_action = Some((exchange.clone(), action));
                            }
                        });
                    })
                });
            if let Some(exchange) = chosen {
                self.selected = Some(exchange);
            }
            if let Some((exchange, action)) = menu_action { self.network_action(exchange, action, ui.ctx()); }
        });
        if self.network_state.confirm_clear {
            let mut confirm = false;
            let mut cancel = false;
            let response = egui::Modal::new(egui::Id::new("clear_network_history")).show(ui.ctx(), |ui| {
                ui.set_width(430.0);
                ui.heading("Clear all network history?");
                ui.label("Delete all recorded requests and responses, including pinned items and items hidden by filters. This cannot be undone.");
                ui.label("Saved Repeater requests stay available. New traffic will continue to appear.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    cancel = ui.button("Cancel").clicked();
                    confirm = ui.button(RichText::new("Clear history").color(Color32::LIGHT_RED)).clicked();
                });
            });
            if cancel || response.should_close() {
                self.network_state.confirm_clear = false;
            }
            if confirm {
                self.network_state.confirm_clear = false;
                self.clear_network(HistoryTarget::All);
            }
        }
    }
    pub fn intercept_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Intercept");
        if !self
            .session
            .as_ref()
            .is_some_and(|s| s.hello.providers.iter().any(|p| p.kind == "dio"))
        {
            ui.label(
                "Intercept is optional. Network capture and desktop Repeater work without it.",
            );
            ui.label("To hold or change live traffic, register your Dio client with flutter_inspector_bridge and flutter_inspector_dio in the debug app.");
            return;
        }
        ui.label(
            "Hold matching Dio traffic, edit it, then forward, drop, or return a mock response.",
        );
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.rule.enabled, "Enabled");
            ui.checkbox(&mut self.rule.request, "Requests");
            ui.checkbox(&mut self.rule.response, "Responses");
            ui.label("Timeout");
            ui.add(
                egui::DragValue::new(&mut self.rule.hold_seconds)
                    .range(1..=600)
                    .suffix(" s"),
            );
            if ui
                .add_enabled(self.session.is_some(), egui::Button::new("Apply rules"))
                .clicked()
            {
                self.send(Command::SetRule(self.rule.clone()));
            }
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.rule.host)
                    .hint_text("Host contains")
                    .desired_width(180.0),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.rule.path)
                    .hint_text("Path contains")
                    .desired_width(240.0),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.rule.method)
                    .hint_text("Method (any)")
                    .desired_width(120.0),
            );
        });
        ui.weak("Timeout or controller disconnect forwards the original message. Streaming responses are captured without holding.");
        ui.separator();
        let mut chosen = None;
        for pending in &self.pending {
            let remaining = ((pending.expires_at - inspector_adapters::store::now()) / 1000).max(0);
            if ui
                .selectable_label(
                    self.hold_id == pending.id,
                    format!(
                        "{} · {} {} · {} s remaining",
                        pending.direction,
                        pending.exchange.request.method,
                        pending.exchange.request.url,
                        remaining
                    ),
                )
                .clicked()
            {
                chosen = Some(pending.clone());
            }
        }
        if let Some(pending) = chosen {
            self.hold_id = pending.id;
            self.hold_url = pending.exchange.request.url;
            self.hold_method = pending.exchange.request.method;
            self.hold_headers = if pending.direction == "response" {
                pending
                    .exchange
                    .response
                    .as_ref()
                    .map(|r| headers_text(&r.headers))
                    .unwrap_or_default()
            } else {
                headers_text(&pending.exchange.request.headers)
            };
            self.hold_status = pending.exchange.response.map(|r| r.status).unwrap_or(200);
            self.hold_body.clear();
            self.hold_replace = false;
        }
        if !self.pending.iter().any(|p| p.id == self.hold_id) {
            self.hold_id.clear();
        }
        if self.pending.is_empty() {
            ui.add_space(30.0);
            ui.weak("No messages held.");
        }
        if !self.hold_id.is_empty() {
            ui.separator();
            egui::ScrollArea::vertical().show(ui,|ui|{
                ui.horizontal(|ui|{
                    ui.add(egui::TextEdit::singleline(&mut self.hold_method).desired_width(90.0));
                    ui.add(egui::TextEdit::singleline(&mut self.hold_url).desired_width(f32::INFINITY));
                });
                ui.label("Headers");
                ui.add(egui::TextEdit::multiline(&mut self.hold_headers).code_editor().desired_width(f32::INFINITY).desired_rows(5));
                ui.checkbox(&mut self.hold_replace,"Replace body (otherwise original bytes are preserved)");
                if self.hold_replace{ui.add(egui::TextEdit::multiline(&mut self.hold_body).code_editor().desired_width(f32::INFINITY).desired_rows(8));}
                ui.horizontal(|ui|{ui.label("Response / mock status");ui.add(egui::DragValue::new(&mut self.hold_status).range(100..=599));});
                ui.horizontal(|ui|{
                    for (label,action) in [("Forward original","original"),("Forward edits","forward"),("Drop","drop"),("Mock response","mock")]{
                        if ui.button(label).clicked(){
                            let mut decision=json!({"id":self.hold_id,"action":if action=="original"{"forward"}else{action}});
                            if action=="forward"||action=="mock"{
                                match parse_headers(&self.hold_headers){
                                    Ok(headers)=>decision["headers"]=json!(headers),
                                    Err(e)=>{self.notice(true,e);continue;}
                                }
                                decision["url"]=json!(self.hold_url);decision["method"]=json!(self.hold_method);decision["status"]=json!(self.hold_status);
                                if self.hold_replace||action=="mock"{decision["body"]=json!(self.hold_body);}
                            }
                            self.send(Command::Decide(decision));
                        }
                    }
                });
            });
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
}
