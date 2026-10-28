use crate::app::InspectorApp;
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use serde_json::{Value, json};

pub struct ProviderEditor {
    pub id: String,
    pub result: Value,
    key: String,
    kind: String,
    value: String,
    filter: String,
}
impl Default for ProviderEditor {
    fn default() -> Self {
        Self {
            id: String::new(),
            result: Value::Null,
            key: String::new(),
            kind: "String".into(),
            value: "\"\"".into(),
            filter: String::new(),
        }
    }
}
impl InspectorApp {
    fn choose_provider(&mut self, ui: &mut egui::Ui, kind: &str) -> bool {
        let editor = &mut self.preferences;
        let selected_name = self
            .session
            .as_ref()
            .and_then(|s| s.hello.providers.iter().find(|p| p.id == editor.id))
            .map(|p| p.name.as_str())
            .unwrap_or("Select provider");
        let mut changed = false;
        egui::ComboBox::from_id_salt(("provider", kind))
            .selected_text(selected_name)
            .show_ui(ui, |ui| {
                if let Some(session) = &self.session {
                    for provider in session.hello.providers.iter().filter(|p| p.kind == kind) {
                        changed |= ui
                            .selectable_value(&mut editor.id, provider.id.clone(), &provider.name)
                            .changed();
                    }
                }
            });
        changed
    }
    pub fn preferences_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("SharedPreferences");
        ui.horizontal(|ui| {
            if self.choose_provider(ui, "preferences") || ui.button("Refresh").clicked() {
                self.provider("prefs.list", "preferences.list", json!({}));
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.preferences.filter).hint_text("Filter keys…"),
            );
        });
        if self.preferences.id.is_empty() {
            ui.weak("No compatible SharedPreferences store detected. Initialize preferences in the app, then reconnect.");
            return;
        }
        egui::Panel::bottom("preference_editor").default_size(235.0).resizable(true).show(ui,|ui|{
            ui.heading("Edit value");
            ui.horizontal(|ui|{
                ui.add(egui::TextEdit::singleline(&mut self.preferences.key).hint_text("Key").desired_width(250.0));
                egui::ComboBox::from_id_salt("pref_type").selected_text(&self.preferences.kind).show_ui(ui,|ui|{
                    for kind in ["String","int","double","bool","List<String>"]{ui.selectable_value(&mut self.preferences.kind,kind.into(),kind);}
                });
                if ui.button("Set value").clicked(){
                    match serde_json::from_str::<Value>(&self.preferences.value){
                        Ok(value)=>self.provider("prefs.set","preferences.set",json!({"key":self.preferences.key,"type":self.preferences.kind,"value":value})),
                        Err(e)=>self.notice(true,format!("Enter a JSON value: {e}")),
                    }
                }
                if ui.button("Remove key").clicked(){self.provider("prefs.remove","preferences.remove",json!({"key":self.preferences.key}));}
            });
            ui.weak("Enter JSON: strings in quotes, true/false, numbers, or an array of strings.");
            ui.add(egui::TextEdit::multiline(&mut self.preferences.value).code_editor().desired_width(f32::INFINITY).desired_rows(5));
        });
        let query = self.preferences.filter.to_lowercase();
        let rows: Vec<_> = self
            .preferences
            .result
            .as_array()
            .into_iter()
            .flatten()
            .filter(|v| {
                v["key"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&query)
            })
            .collect();
        ui.set_clip_rect(ui.clip_rect().intersect(ui.available_rect_before_wrap()));
        TableBuilder::new(ui)
            .sense(egui::Sense::click())
            .striped(true)
            .resizable(true)
            .column(Column::initial(280.0))
            .column(Column::initial(110.0))
            .column(Column::remainder())
            .header(28.0, |mut row| {
                for title in ["Key", "Type", "Value"] {
                    row.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(29.0, rows.len(), |mut row| {
                    let value = &rows[row.index()];
                    row.col(|ui| {
                        ui.label(value["key"].as_str().unwrap_or(""));
                    });
                    row.col(|ui| {
                        ui.label(value["type"].as_str().unwrap_or(""));
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(value["value"].to_string()).truncate());
                    });
                    if row.response().clicked() {
                        self.preferences.key = value["key"].as_str().unwrap_or("").into();
                        self.preferences.kind = value["type"].as_str().unwrap_or("String").into();
                        self.preferences.value = value["value"].to_string();
                    }
                })
            });
    }
}
