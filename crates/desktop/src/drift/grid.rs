use super::state::{DriftEditor, Row, preview};
use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use serde_json::Value;

pub enum Action {
    Edit(Row, usize),
    Set(Row, usize, Option<Value>),
    RemoveNew(u64),
    Commit,
    Next,
    Cancel,
    Sort(String),
}

impl DriftEditor {
    pub fn grid(&mut self, ui: &mut egui::Ui, enabled: bool) -> Vec<Action> {
        let mut actions = Vec::new();
        // Copy only schema metadata. Rows and even large text primary keys stay borrowed.
        let columns = self.columns.clone();
        let keys: Vec<Row> = self
            .inserts
            .keys()
            .copied()
            .map(Row::New)
            .chain((0..self.rows.len()).map(Row::Stored))
            .collect();
        let can_sort = !self.pending() && enabled;
        let writable = self.writable() && enabled;
        ui.set_clip_rect(ui.clip_rect().intersect(ui.available_rect_before_wrap()));

        egui::ScrollArea::horizontal().id_salt("drift_grid_horizontal").show(ui, |ui| {
            let mut table = TableBuilder::new(ui)
                .id_salt((&self.id, &self.table))
                .striped(true).resizable(true).auto_shrink([false, false])
                .column(Column::exact(40.0));
            for column in &columns {
                let width = if column.pk { 100.0 } else { 210.0 };
                table = table.column(Column::initial(width).at_least(75.0));
            }
            table.header(34.0, |mut row| {
                row.col(|ui| {
                    let editable: Vec<_> = (0..self.rows.len())
                        .filter(|i| self.has_identity(*i)).collect();
                    let mut all = !editable.is_empty()
                        && editable.iter().all(|i| self.selected.contains(i));
                    let response = ui.add_enabled(
                        writable && !editable.is_empty(),
                        egui::Checkbox::without_text(&mut all),
                    ).on_hover_text("Select rows on this page");
                    if response.changed() {
                        self.selected = if all { editable.into_iter().collect() } else { Default::default() };
                    }
                });
                for column in &columns {
                    row.col(|ui| {
                        let suffix = if self.order == column.name {
                            if self.descending { " ↓" } else { " ↑" }
                        } else { "" };
                        let pk = if column.pk { " · PK" } else { "" };
                        let name = format!("{}{pk}{suffix}", column.name);
                        let mut tooltip = column.kind.clone();
                        if !column.default.is_empty() {
                            tooltip.push_str(&format!(" · default {}", column.default));
                        }
                        if column.required { tooltip.push_str(" · required"); }
                        if column.generated { tooltip.push_str(" · generated"); }
                        let button = egui::Button::new(RichText::new(name).strong()).frame(false);
                        if ui.add_enabled(can_sort, button).on_hover_text(tooltip).clicked() {
                            actions.push(Action::Sort(column.name.clone()));
                        }
                    });
                }
            }).body(|body| {
                body.rows(32.0, keys.len(), |mut row| {
                    let key = keys[row.index()];
                    let editable = writable && match key {
                        Row::Stored(i) => self.has_identity(i),
                        Row::New(_) => true,
                    };
                    row.col(|ui| match key {
                        Row::New(id) => {
                            if ui.add_enabled(enabled, egui::Button::new("×"))
                                .on_hover_text("Remove this unsaved row").clicked() {
                                actions.push(Action::RemoveNew(id));
                            }
                        }
                        Row::Stored(index) => {
                            let mut selected = self.selected.contains(&index);
                            let response = ui.add_enabled(editable, egui::Checkbox::without_text(&mut selected))
                                .on_disabled_hover_text("A complete primary key is required");
                            if response.changed() {
                                if selected { self.selected.insert(index); }
                                else { self.selected.remove(&index); }
                            }
                        }
                    });
                    for (index, column) in columns.iter().enumerate() {
                        row.col(|ui| {
                            ui.push_id((key_id(key), index), |ui| {
                                if let Some(cell) = self.cell.as_mut().filter(|c| c.row == key && c.column == index) {
                                    let editor = egui::TextEdit::singleline(&mut cell.text)
                                        .desired_width(f32::INFINITY).char_limit(256 * 1024);
                                    let response = ui.add_enabled(enabled, editor);
                                    cell.touched |= response.changed();
                                    if cell.focus {
                                        response.request_focus();
                                        cell.focus = false;
                                    }
                                    if response.has_focus() || response.lost_focus() {
                                        let shortcut = |key| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key));
                                        if shortcut(egui::Key::Escape) { actions.push(Action::Cancel); }
                                        else if shortcut(egui::Key::Enter) { actions.push(Action::Commit); }
                                        else if shortcut(egui::Key::Tab) { actions.push(Action::Next); }
                                    }
                                    return;
                                }
                                let value = self.value(key, &column.name);
                                let changed = match key {
                                    Row::New(_) => true,
                                    Row::Stored(i) => self.edits.get(&i).is_some_and(|e| e.contains_key(&column.name)),
                                };
                                let mut text = RichText::new(preview(value)).monospace();
                                if changed {
                                    text = text.color(Color32::from_rgb(241, 196, 104));
                                } else if value.is_none_or(Value::is_null) || column.generated {
                                    text = text.color(ui.visuals().weak_text_color()).italics();
                                }
                                let label = egui::Label::new(text).truncate().sense(egui::Sense::click());
                                let response = ui.add_sized([ui.available_width(), 25.0], label);
                                let can_edit = editable && !column.generated;
                                if can_edit && response.double_clicked() {
                                    actions.push(Action::Edit(key, index));
                                }
                                let tooltip = if can_edit {
                                    "Double-click to edit. Right-click to copy, set NULL, or reset."
                                } else { "Read-only. Right-click to copy." };
                                response.on_hover_text(tooltip).context_menu(|ui| {
                                    if ui.button("Copy value").clicked() {
                                        ui.ctx().copy_text(value.map(super::state::edit_text).unwrap_or_default());
                                        ui.close();
                                    }
                                    if ui.add_enabled(can_edit, egui::Button::new("Edit cell")).clicked() {
                                        actions.push(Action::Edit(key, index));
                                        ui.close();
                                    }
                                    if ui.add_enabled(can_edit && !column.required, egui::Button::new("Set NULL")).clicked() {
                                        actions.push(Action::Set(key, index, Some(Value::Null)));
                                        ui.close();
                                    }
                                    let reset = if matches!(key, Row::New(_)) { "Use default" } else { "Reset value" };
                                    if ui.add_enabled(can_edit, egui::Button::new(reset)).clicked() {
                                        actions.push(Action::Set(key, index, None));
                                        ui.close();
                                    }
                                });
                            });
                        });
                    }
                });
            });
        });
        actions
    }
}

fn key_id(row: Row) -> (bool, u64) {
    match row {
        Row::Stored(i) => (false, i as u64),
        Row::New(id) => (true, id),
    }
}
