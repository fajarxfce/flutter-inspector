mod grid;
mod state;
use crate::app::InspectorApp;
use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use inspector_application::Command;
use serde_json::{Value, json};
pub use state::DriftEditor;
use state::{Confirmation, Request, Row};

impl InspectorApp {
    pub fn refresh_drift_databases(&mut self) {
        if self.session.is_some() {
            self.drift_request(Request::Databases, "drift.databases", json!({}));
        }
    }
    fn drift_request(&mut self, kind: Request, method: &str, mut params: Value) {
        let tag = self.drift.begin(kind);
        params["provider"] = self.drift.id.clone().into();
        let command = Command::Provider {
            tag: tag.clone(),
            method: method.into(),
            params,
        };
        if let Err(error) = self.worker.commands.try_send(command) {
            self.drift.finish(&tag);
            self.drift.error = Some(format!("Worker busy: {error}. Try again."));
        }
    }
    pub fn drift_database(&mut self, id: String) {
        self.drift.database(id);
        if !self.drift.id.is_empty() {
            self.drift_request(Request::Tables, "drift.tables", json!({}));
        }
    }
    fn drift_rows(&mut self) {
        if self.drift.table.is_empty() {
            return;
        }
        self.drift_request(
            Request::Rows,
            "drift.rows",
            json!({
                "table": self.drift.table,
                "offset": self.drift.offset,
                "filter": self.drift.filter,
                "order": self.drift.order,
                "descending": self.drift.descending,
            }),
        );
    }
    pub fn drift_result(&mut self, tag: &str, result: Result<Value, String>) {
        let Some(kind) = self.drift.finish(tag) else {
            return;
        };
        // Stale database/table/query response.
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                self.drift.error = Some(error);
                return;
            }
        };
        self.drift.error = None;
        match kind {
            Request::Databases => {
                let providers: Vec<inspector_domain::Provider> = match serde_json::from_value(value)
                {
                    Ok(providers) => providers,
                    Err(error) => {
                        self.drift.error = Some(error.to_string());
                        return;
                    }
                };
                let id = providers
                    .iter()
                    .find(|p| p.id == self.drift.id)
                    .or_else(|| providers.first())
                    .map(|p| p.id.clone())
                    .unwrap_or_default();
                if let Some(session) = &mut self.session {
                    session.hello.providers.retain(|p| p.kind != "drift");
                    session.hello.providers.extend(providers);
                }
                if id != self.drift.id {
                    self.drift_database(id);
                } else if !id.is_empty() && !self.drift.pending() && !self.drift.busy() {
                    self.drift_request(Request::Tables, "drift.tables", json!({}));
                }
            }
            Request::Tables => {
                self.drift.accept_tables(value);
                self.drift_rows();
            }
            Request::Rows => {
                self.drift.accept_rows(value);
                if self.drift.offset > 0 && self.drift.offset as u64 >= self.drift.total {
                    self.drift.offset = self.drift.total.saturating_sub(1) as usize / 200 * 200;
                    self.drift_rows();
                }
            }
            Request::Save | Request::Delete | Request::Empty => {
                self.drift.discard();
                self.drift.selected.clear();
                self.drift.query_result = Value::Null;
                self.notice(
                    false,
                    format!(
                        "{} row(s) {}",
                        value["changed"],
                        if kind == Request::Save {
                            "saved"
                        } else {
                            "deleted"
                        }
                    ),
                );
                self.drift_rows();
            }
            Request::Query => self.drift.query_result = value,
            Request::Execute => {
                self.drift.query_result = value;
                self.drift_request(Request::Tables, "drift.tables", json!({}));
            }
        }
    }
    pub fn drift_ui(&mut self, ui: &mut egui::Ui) {
        let connected = self.session.is_some();
        let locked = self.drift.pending() || self.drift.busy();
        let mut database = None;
        let mut table = None;
        egui::Panel::left("drift_tables")
            .default_size(235.0)
            .size_range(180.0..=420.0)
            .resizable(true)
            .show(ui, |ui| {
                ui.heading("Drift");
                ui.horizontal(|ui| {
                    ui.label("Database");
                    if ui
                        .add_enabled(
                            connected && !self.drift.loading(),
                            egui::Button::new("↻").small(),
                        )
                        .on_hover_text("Refresh databases")
                        .clicked()
                    {
                        self.refresh_drift_databases();
                    }
                });
                let selected = self
                    .session
                    .as_ref()
                    .and_then(|s| s.hello.providers.iter().find(|p| p.id == self.drift.id))
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "No database".into());
                ui.add_enabled_ui(connected && !locked, |ui| {
                    egui::ComboBox::from_id_salt("drift_database")
                        .selected_text(selected)
                        .width(ui.available_width() - 10.0)
                        .show_ui(ui, |ui| {
                            if let Some(session) = &self.session {
                                for provider in
                                    session.hello.providers.iter().filter(|p| p.kind == "drift")
                                {
                                    if ui
                                        .selectable_label(
                                            provider.id == self.drift.id,
                                            &provider.name,
                                        )
                                        .clicked()
                                        && provider.id != self.drift.id
                                    {
                                        database = Some(provider.id.clone());
                                    }
                                }
                            }
                        });
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.strong(format!("Tables · {}", self.drift.tables.len()));
                    if ui
                        .add_enabled(
                            connected && !locked && !self.drift.id.is_empty(),
                            egui::Button::new("Refresh"),
                        )
                        .clicked()
                    {
                        self.drift_request(Request::Tables, "drift.tables", json!({}));
                    }
                });
                ui.add(
                    egui::TextEdit::singleline(&mut self.drift.table_filter)
                        .hint_text("Find table…")
                        .desired_width(f32::INFINITY),
                );
                ui.separator();
                let query = self.drift.table_filter.to_lowercase();
                egui::ScrollArea::vertical()
                    .id_salt("drift_table_list")
                    .show(ui, |ui| {
                        for item in self
                            .drift
                            .tables
                            .iter()
                            .filter(|t| t.name.to_lowercase().contains(&query))
                        {
                            let label = if item.view {
                                format!("{}  · view", item.name)
                            } else {
                                item.name.clone()
                            };
                            if ui
                                .add_enabled(
                                    connected && !locked,
                                    egui::Button::selectable(item.name == self.drift.table, label)
                                        .min_size(egui::vec2(ui.available_width(), 30.0)),
                                )
                                .clicked()
                            {
                                table = Some(item.name.clone());
                            }
                        }
                    });
                if self.drift.loading() {
                    ui.spinner();
                }
                if locked && self.drift.pending() {
                    ui.weak("Save or discard changes before switching tables or databases.");
                }
            });
        // Apply selection before drawing the right pane; old rows never flash in
        // the new database, even if the previous VM reply is still in flight.
        if let Some(id) = database {
            self.drift_database(id);
        }
        if let Some(name) = table {
            self.drift.select_table(name);
            self.drift_rows();
        }
        egui::CentralPanel::default().show(ui, |ui| self.drift_content(ui, connected));
        self.drift_confirmation(ui.ctx(), connected);
    }
    fn drift_content(&mut self, ui: &mut egui::Ui, connected: bool) {
        if self.drift.id.is_empty() {
            ui.heading("Drift");
            if self.drift.loading() {
                ui.spinner();
                ui.label("Looking for Drift databases…");
            } else {
                ui.weak("Open a Drift database in the app. New databases appear automatically.");
            }
            if let Some(error) = &self.drift.error {
                ui.colored_label(Color32::LIGHT_RED, error);
            }
            if ui
                .add_enabled(
                    connected && !self.drift.loading(),
                    egui::Button::new("Refresh databases"),
                )
                .clicked()
            {
                self.refresh_drift_databases();
            }
            return;
        }
        ui.horizontal(|ui| {
            let title = if self.drift.table.is_empty() {
                "Select a table"
            } else {
                &self.drift.table
            };
            ui.heading(title);
            let suffix = if self.drift.filter.is_empty() {
                ""
            } else {
                " matching filter"
            };
            ui.weak(format!("{} rows{suffix}", self.drift.total));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.toggle_value(&mut self.drift.show_sql, "SQL console");
            });
        });
        if let Some(error) = &self.drift.error {
            ui.colored_label(
                Color32::LIGHT_RED,
                error.chars().take(2048).collect::<String>(),
            );
        }
        if !connected {
            ui.colored_label(
                Color32::from_rgb(235, 185, 90),
                "Disconnected. Reconnect to edit this database.",
            );
        }
        if self.drift.show_sql {
            self.drift_sql(ui, connected);
            return;
        }
        let ready = connected && !self.drift.busy() && !self.drift.loading();
        let writable = ready && self.drift.writable();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(writable, egui::Button::new("+ New row"))
                .clicked()
                && let Err(error) = self.drift.new_row()
            {
                self.drift.error = Some(error);
            }
            if ui
                .add_enabled(
                    writable && self.drift.pending(),
                    egui::Button::new("Save changes"),
                )
                .clicked()
            {
                self.drift_save();
            }
            if ui
                .add_enabled(
                    !self.drift.busy() && self.drift.pending(),
                    egui::Button::new("Discard"),
                )
                .clicked()
            {
                self.drift.discard();
                self.drift.error = None;
            }
            ui.separator();
            let clean = !self.drift.pending();
            let delete =
                egui::Button::new(format!("Delete selected ({})", self.drift.selected.len()));
            if ui
                .add_enabled(writable && clean && !self.drift.selected.is_empty(), delete)
                .clicked()
            {
                self.drift.confirmation = Some(Confirmation::Delete);
            }
            if ui
                .add_enabled(writable && clean, egui::Button::new("Empty table"))
                .clicked()
            {
                self.drift.confirmation = Some(Confirmation::Empty);
            }
            if self.drift.busy() {
                ui.spinner();
                ui.label("Saving…");
            }
        });
        let navigate = ready && !self.drift.pending();
        ui.horizontal(|ui| {
            let filter = egui::TextEdit::singleline(&mut self.drift.filter)
                .hint_text("Filter values…")
                .desired_width(200.0);
            let response = ui.add_enabled(navigate, filter);
            if ui
                .add_enabled(navigate, egui::Button::new("Apply filter"))
                .clicked()
                || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            {
                self.drift.offset = 0;
                self.drift_rows();
            }
            if ui
                .add_enabled(navigate, egui::Button::new("Refresh rows"))
                .clicked()
            {
                self.drift_rows();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let has_next = self.drift.total > self.drift.offset as u64 + 200;
                if ui
                    .add_enabled(navigate && has_next, egui::Button::new("›"))
                    .clicked()
                {
                    self.drift.offset += 200;
                    self.drift_rows();
                }
                let start = if self.drift.total == 0 {
                    0
                } else {
                    self.drift.offset + 1
                };
                let end = self.drift.offset + self.drift.rows.len();
                ui.small(format!("{start}–{end} / {}", self.drift.total));
                if ui
                    .add_enabled(navigate && self.drift.offset > 0, egui::Button::new("‹"))
                    .clicked()
                {
                    self.drift.offset = self.drift.offset.saturating_sub(200);
                    self.drift_rows();
                }
            });
        });
        if self.drift.writable() {
            ui.weak("Double-click a cell to edit · Enter to stage · Tab for next cell · right-click for NULL/default · Save changes to commit");
        } else {
            ui.weak("Views are read-only.");
        }
        if self.drift.loading() && self.drift.rows.is_empty() {
            ui.spinner();
            ui.label("Loading table…");
            return;
        }
        if self.drift.columns.is_empty() {
            return;
        }
        if self.drift.rows.is_empty() && self.drift.inserts.is_empty() {
            ui.weak("No rows. Use + New row to insert data.");
        }
        for action in self.drift.grid(ui, ready) {
            self.drift_action(action);
        }
    }
    fn drift_save(&mut self) {
        let result = self.drift.commit_cell().and_then(|()| self.drift.changes());
        match result {
            Ok(changes) if changes.as_array().is_some_and(|v| !v.is_empty()) => {
                self.drift.error = None;
                self.drift_request(
                    Request::Save,
                    "drift.apply",
                    json!({"table": self.drift.table, "changes": changes}),
                );
            }
            Ok(_) => {}
            Err(error) => self.drift.error = Some(error),
        }
    }
    fn drift_action(&mut self, action: grid::Action) {
        use grid::Action;
        let result = match action {
            Action::Edit(row, col) => self.drift.edit(row, col),
            Action::Commit => self.drift.commit_cell(),
            Action::Next => {
                let next = self.drift.cell.as_ref().and_then(|c| {
                    ((c.column + 1)..self.drift.columns.len())
                        .find(|i| !self.drift.columns[*i].generated)
                        .map(|col| (c.row, col))
                });
                self.drift.commit_cell().and_then(|()| {
                    if let Some((row, col)) = next {
                        self.drift.edit(row, col)
                    } else {
                        Ok(())
                    }
                })
            }
            Action::Cancel => {
                self.drift.cell = None;
                Ok(())
            }
            Action::Set(row, col, value) => self.drift.commit_cell().map(|()| {
                let name = self.drift.columns[col].name.clone();
                self.drift.set(row, &name, value);
            }),
            Action::RemoveNew(id) => {
                if self
                    .drift
                    .cell
                    .as_ref()
                    .is_some_and(|c| c.row == Row::New(id))
                {
                    self.drift.cell = None;
                }
                self.drift.inserts.remove(&id);
                Ok(())
            }
            Action::Sort(name) => {
                if self.drift.order == name {
                    self.drift.descending = !self.drift.descending;
                } else {
                    self.drift.order = name;
                    self.drift.descending = false;
                }
                self.drift.offset = 0;
                self.drift_rows();
                Ok(())
            }
        };
        self.drift.error = result.err();
    }
    fn drift_confirmation(&mut self, ctx: &egui::Context, connected: bool) {
        let Some(kind) = self.drift.confirmation else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("drift_delete_confirmation")).show(ctx, |ui| {
            ui.set_width(430.0);
            ui.heading(match kind {
                Confirmation::Delete => "Delete selected rows?",
                Confirmation::Empty => "Empty this table?",
            });
            ui.label(format!("Table: {}", self.drift.table));
            match kind {
                Confirmation::Delete => {
                    ui.label(format!("Delete {} selected row(s). This cannot be undone.", self.drift.selected.len()));
                }
                Confirmation::Empty => {
                    ui.label("Delete ALL rows, including rows hidden by filters or on other pages. The table structure stays intact. This cannot be undone.");
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                cancel = ui.button("Cancel").clicked();
                let label = match kind {
                    Confirmation::Delete => "Delete rows",
                    Confirmation::Empty => "Delete all rows",
                };
                let button = egui::Button::new(RichText::new(label).color(Color32::LIGHT_RED));
                confirm = ui.add_enabled(connected, button).clicked();
            });
        });
        if response.should_close() || cancel {
            self.drift.confirmation = None;
        }
        if confirm {
            self.drift.confirmation = None;
            match kind {
                Confirmation::Empty => self.drift_request(
                    Request::Empty,
                    "drift.empty",
                    json!({"table":self.drift.table,"confirm":self.drift.table}),
                ),
                Confirmation::Delete => match self.drift.deletes() {
                    Ok(changes) => self.drift_request(
                        Request::Delete,
                        "drift.apply",
                        json!({"table":self.drift.table,"changes":changes}),
                    ),
                    Err(error) => self.drift.error = Some(error),
                },
            }
        }
    }
    fn drift_sql(&mut self, ui: &mut egui::Ui, connected: bool) {
        ui.add(
            egui::TextEdit::multiline(&mut self.drift.sql)
                .code_editor()
                .desired_width(f32::INFINITY)
                .desired_rows(5),
        );
        ui.horizontal(|ui| {
            let enabled = connected && !self.drift.busy() && !self.drift.pending();
            if ui
                .add_enabled(enabled, egui::Button::new("Run query"))
                .clicked()
            {
                self.drift_request(
                    Request::Query,
                    "drift.query",
                    json!({"sql": self.drift.sql}),
                );
            }
            if ui
                .add_enabled(enabled, egui::Button::new("Execute statement"))
                .clicked()
            {
                self.drift_request(
                    Request::Execute,
                    "drift.execute",
                    json!({"sql": self.drift.sql}),
                );
            }
            if ui.button("Copy result JSON").clicked() {
                ui.ctx().copy_text(
                    serde_json::to_string_pretty(&self.drift.query_result).unwrap_or_default(),
                );
            }
        });
        ui.weak("Query results are read-only. Switch back to the table to edit rows.");
        let Some(rows) = self.drift.query_result.as_array() else {
            ui.weak(self.drift.query_result.to_string());
            return;
        };
        let columns: Vec<_> = rows
            .first()
            .and_then(Value::as_object)
            .map(|o| o.keys().collect())
            .unwrap_or_default();
        if columns.is_empty() {
            ui.weak("No rows returned.");
            return;
        }
        egui::ScrollArea::horizontal().show(ui, |ui| {
            let mut table = TableBuilder::new(ui)
                .id_salt("drift_sql_results")
                .striped(true)
                .resizable(true);
            for _ in &columns {
                table = table.column(Column::initial(210.0).at_least(80.0));
            }
            table
                .header(30.0, |mut row| {
                    for name in &columns {
                        row.col(|ui| {
                            ui.strong(*name);
                        });
                    }
                })
                .body(|body| {
                    body.rows(30.0, rows.len(), |mut row| {
                        let value = &rows[row.index()];
                        for name in &columns {
                            row.col(|ui| {
                                ui.add(
                                    egui::Label::new(state::preview(value.get(*name))).truncate(),
                                );
                            });
                        }
                    });
                });
        });
    }
}
