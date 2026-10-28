use eframe::egui::{self, Color32, RichText};
use inspector_application::BodyContent;
use std::{collections::HashSet, sync::Arc};

#[derive(Clone, Copy, Default, PartialEq)]
enum Mode {
    #[default]
    Tree,
    Raw,
    Pretty,
    Hex,
    Form,
}

#[derive(Default)]
pub struct Viewer {
    key: String,
    mode: Mode,
    expanded: HashSet<usize>,
    visible: Vec<usize>,
    search: String,
    formatted: String,
    lines: Vec<usize>,
}
impl Viewer {
    pub fn show(&mut self, ui: &mut egui::Ui, id: &str, body: Arc<BodyContent>) {
        self.show_inner(ui, id, body, true, None);
    }
    pub fn show_network(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        body: Arc<BodyContent>,
        mut menu: impl FnMut(&mut egui::Ui),
    ) {
        self.show_inner(ui, id, body, false, Some(&mut menu));
    }
    fn prepare(&mut self, id: &str, body: &BodyContent) {
        if self.key != id {
            self.key = id.into();
            self.expanded = HashSet::from([0]);
            self.search.clear();
            self.mode = if body.json.is_some() {
                Mode::Pretty
            } else {
                Mode::Raw
            };
            self.rebuild(body);
        }
    }
    fn controls(
        &mut self,
        ui: &mut egui::Ui,
        body: &BodyContent,
        copy_button: bool,
        active: bool,
    ) -> bool {
        let mut changed = false;
        for (mode, label, available) in [
            (Mode::Form, "Form fields", body.form.is_some()),
            (Mode::Tree, "JSON tree", body.json.is_some()),
            (Mode::Pretty, "Pretty", body.json.is_some()),
            (Mode::Raw, "Raw", true),
            (Mode::Hex, "Hex", true),
        ] {
            if available
                && ui
                    .selectable_label(active && self.mode == mode, label)
                    .clicked()
            {
                self.mode = mode;
                changed = true;
            }
        }
        if changed {
            self.rebuild(body);
        }
        if copy_button && ui.button("Copy body").clicked() {
            ui.ctx().copy_text(self.copy_text(body));
        }
        changed
    }
    pub fn network_controls(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        body: &BodyContent,
        active: bool,
    ) -> bool {
        self.prepare(id, body);
        self.controls(ui, body, false, active)
    }
    fn show_inner(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        body: Arc<BodyContent>,
        copy_button: bool,
        mut menu: Option<&mut dyn FnMut(&mut egui::Ui)>,
    ) {
        self.prepare(id, &body);
        if menu.is_none() {
            ui.horizontal(|ui| {
                self.controls(ui, &body, copy_button, true);
            });
        }
        let changed = ui
            .add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Find in body…")
                    .desired_width(f32::INFINITY),
            )
            .changed();
        if changed {
            self.rebuild(&body);
        }
        ui.separator();
        if body.json.as_ref().is_some_and(|d| d.limited) {
            ui.weak("Tree preview limited to 100,000 nodes. Raw / Pretty retain the full captured body.");
        }
        if body.bytes.len() > 5 * 1024 * 1024 {
            ui.weak("Large body: Raw / Hex preview. Copy preserves the full captured payload.");
        }
        match self.mode {
            Mode::Form => {
                if let Some(pairs) = &body.form {
                    crate::http_view::pairs(ui, ("body-form", id), pairs);
                }
            }
            Mode::Tree => {
                if let Some(doc) = &body.json {
                    let mut toggle = None;
                    egui::ScrollArea::both().id_salt(("tree", id)).show_rows(
                        ui,
                        21.0,
                        self.visible.len(),
                        |ui, range| {
                            for row in range {
                                let index = self.visible[row];
                                let node = &doc.nodes[index];
                                ui.horizontal(|ui| {
                                    ui.add_space((node.depth.min(24) * 15) as f32);
                                    if node.children.is_empty() {
                                        ui.add_space(20.0);
                                    } else if ui
                                        .small_button(if self.expanded.contains(&index) {
                                            "-"
                                        } else {
                                            "+"
                                        })
                                        .clicked()
                                    {
                                        toggle = Some(index);
                                    }
                                    let response = ui.add(
                                        egui::Label::new(
                                            RichText::new(&node.label)
                                                .monospace()
                                                .color(Color32::from_rgb(113, 190, 235)),
                                        )
                                        .selectable(true),
                                    );
                                    response.context_menu(|ui| {
                                        if ui.button("Copy JSON pointer").clicked() {
                                            ui.ctx().copy_text(node.pointer.clone());
                                            ui.close();
                                        }
                                        if ui.button("Copy value").clicked() {
                                            if let Some(value) = doc.value.pointer(&node.pointer) {
                                                ui.ctx().copy_text(value.to_string());
                                            }
                                            ui.close();
                                        }
                                        if let Some(menu) = menu.as_deref_mut() {
                                            ui.separator();
                                            menu(ui);
                                        }
                                    });
                                    let color = if node.summary.starts_with('"') {
                                        Color32::from_rgb(146, 202, 153)
                                    } else {
                                        Color32::from_rgb(225, 186, 128)
                                    };
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&node.summary).monospace().color(color),
                                        )
                                        .selectable(true),
                                    );
                                });
                            }
                        },
                    );
                    if let Some(index) = toggle {
                        if !self.expanded.remove(&index) {
                            self.expanded.insert(index);
                        }
                        self.rebuild(&body);
                    }
                }
            }
            Mode::Hex => {
                let count = body.bytes.len().div_ceil(16);
                egui::ScrollArea::both().id_salt(("hex", id)).show_rows(
                    ui,
                    20.0,
                    count,
                    |ui, range| {
                        for row in range {
                            let bytes =
                                &body.bytes[row * 16..((row + 1) * 16).min(body.bytes.len())];
                            let hex = bytes
                                .iter()
                                .map(|b| format!("{b:02x}"))
                                .collect::<Vec<_>>()
                                .join(" ");
                            let ascii: String = bytes
                                .iter()
                                .map(|b| {
                                    if b.is_ascii_graphic() {
                                        *b as char
                                    } else {
                                        '.'
                                    }
                                })
                                .collect();
                            let response = ui.add(
                                egui::Label::new(
                                    RichText::new(format!("{:08x}  {hex:47}  {ascii}", row * 16))
                                        .monospace(),
                                )
                                .selectable(true),
                            );
                            if let Some(menu) = menu.as_deref_mut() {
                                response.context_menu(menu);
                            }
                        }
                    },
                );
            }
            _ => {
                let needle = self.search.to_lowercase();
                let text = &self.formatted;
                egui::ScrollArea::both().id_salt(("text", id)).show_rows(
                    ui,
                    20.0,
                    self.lines.len(),
                    |ui, range| {
                        for row in range {
                            let start = self.lines[row];
                            let end = self.lines.get(row + 1).copied().unwrap_or(text.len());
                            let line = text[start..end].trim_end_matches('\n');
                            // Keep a giant minified JSON line from creating millions of glyphs.
                            let preview: String = line.chars().take(4096).collect();
                            let highlighted =
                                !needle.is_empty() && line.to_lowercase().contains(&needle);
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!("{:>5}", row + 1)).monospace().weak(),
                                    )
                                    .selectable(false),
                                );
                                let response = if self.mode == Mode::Pretty {
                                    ui.add(
                                        egui::Label::new(crate::syntax::json_line(
                                            &preview,
                                            ui.visuals().dark_mode,
                                            highlighted,
                                        ))
                                        .selectable(true),
                                    )
                                } else {
                                    let mut text = RichText::new(preview).monospace();
                                    if highlighted {
                                        text = text.background_color(Color32::from_rgb(75, 65, 18));
                                    }
                                    ui.add(egui::Label::new(text).selectable(true))
                                };
                                if let Some(menu) = menu.as_deref_mut() {
                                    response.context_menu(menu);
                                }
                            });
                        }
                    },
                );
            }
        }
    }
    pub fn copy_text(&self, body: &BodyContent) -> String {
        if self.mode == Mode::Pretty {
            body.pretty.clone().unwrap_or_else(|| body.text.clone())
        } else {
            body.text.clone()
        }
    }
    fn rebuild(&mut self, body: &BodyContent) {
        if self.mode == Mode::Tree {
            if let Some(doc) = &body.json {
                self.visible = if self.search.is_empty() {
                    doc.visible(&self.expanded)
                } else {
                    let query = self.search.to_lowercase();
                    doc.nodes
                        .iter()
                        .enumerate()
                        .filter(|(_, n)| {
                            n.label.to_lowercase().contains(&query)
                                || n.summary.to_lowercase().contains(&query)
                        })
                        .map(|(i, _)| i)
                        .collect()
                };
            }
        } else if self.mode != Mode::Hex {
            self.formatted = if self.mode == Mode::Pretty {
                body.pretty.clone().unwrap_or_else(|| body.text.clone())
            } else {
                body.text.clone()
            };
            self.lines = std::iter::once(0)
                .chain(self.formatted.match_indices('\n').map(|(i, _)| i + 1))
                .collect();
        }
    }
}
