use super::*;
use crate::viewer::Viewer;

#[derive(Clone, Copy, Default, PartialEq)]
enum Section {
    #[default]
    Body,
    Headers,
    Parameters,
}

#[derive(Default)]
pub(super) struct Detail {
    request: Section,
    response: Section,
    request_viewer: Viewer,
    response_viewer: Viewer,
}

pub(super) enum Action {
    Request(RequestAction),
    Url,
    Pin,
    Clear,
    Body(bool),
}

pub(super) fn menu(
    ui: &mut egui::Ui,
    exchange: &Exchange,
    clearing: bool,
    pane: Option<bool>,
) -> Option<Action> {
    let mut action = None;
    for (label, value) in [
        ("Send to Repeater", RequestAction::Repeater),
        ("Copy cURL", RequestAction::Curl),
        ("Copy request", RequestAction::Raw),
        ("Copy response", RequestAction::Response),
    ] {
        if ui
            .add_enabled(
                !matches!(value, RequestAction::Response) || exchange.response.is_some(),
                egui::Button::new(label),
            )
            .clicked()
        {
            action = Some(Action::Request(value));
        }
    }
    if ui.button("Copy URL").clicked() {
        action = Some(Action::Url);
    }
    if let Some(response) = pane
        && ui.button("Copy body").clicked()
    {
        action = Some(Action::Body(response));
    }
    ui.separator();
    if ui
        .button(if exchange.pinned { "Unpin" } else { "Pin" })
        .clicked()
    {
        action = Some(Action::Pin);
    }
    if ui
        .add_enabled(
            !clearing,
            egui::Button::new(RichText::new("Clear item").color(Color32::LIGHT_RED)),
        )
        .clicked()
    {
        action = Some(Action::Clear);
    }
    if action.is_some() {
        ui.close();
    }
    action
}

impl InspectorApp {
    pub(super) fn network_action(
        &mut self,
        exchange: Exchange,
        action: Action,
        ctx: &egui::Context,
    ) {
        match action {
            Action::Request(action) => self.request_action(exchange, action, ctx),
            Action::Url => ctx.copy_text(exchange.request.url),
            Action::Pin => self.send(Command::Pin {
                id: exchange.id,
                pinned: !exchange.pinned,
            }),
            Action::Clear => self.clear_network(HistoryTarget::single(exchange.id)),
            Action::Body(response) => {
                let body = if response {
                    exchange.response.as_ref().and_then(|r| r.body.as_ref())
                } else {
                    exchange.request.body.as_ref()
                };
                if let Some(body) = body {
                    if let Some(content) = self.bodies.get(&body.id) {
                        let viewer = if response {
                            &self.network_state.detail.response_viewer
                        } else {
                            &self.network_state.detail.request_viewer
                        };
                        ctx.copy_text(viewer.copy_text(content));
                    } else {
                        self.load(&body.id);
                        self.notice(
                            false,
                            "Loading body. Open the menu again when it is ready.".into(),
                        );
                    }
                }
            }
        }
    }

    pub(super) fn exchange_ui(&mut self, ui: &mut egui::Ui) {
        let Some(exchange) = self.selected.clone() else {
            return;
        };
        let mut action = None;
        let header = ui.horizontal(|ui| {
            ui.strong("Exchange");
            ui.label(
                RichText::new(&exchange.request.method)
                    .monospace()
                    .color(Color32::from_rgb(115, 222, 193)),
            );
            ui.allocate_ui_with_layout(
                egui::vec2((ui.available_width() - 40.0).max(80.0), 22.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.add(egui::Label::new(&exchange.request.url).truncate())
                        .on_hover_text(&exchange.request.url);
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("×")
                                .size(22.0)
                                .color(Color32::from_rgb(245, 105, 110)),
                        )
                        .frame(false)
                        .min_size(egui::vec2(28.0, 28.0)),
                    )
                    .on_hover_text("Close detail")
                    .clicked()
                {
                    self.selected = None;
                }
            });
        });
        header
            .response
            .interact(egui::Sense::click())
            .context_menu(|ui| {
                action = menu(ui, &exchange, self.network_state.clearing, None);
            });
        ui.horizontal(|ui| {
            ui.small(crate::http_view::timestamp(exchange.started_at, true));
            ui.separator();
            ui.small(format!("{} ms", exchange.duration_ms));
            ui.separator();
            ui.small(format!("{:?}", exchange.state));
            if let Some(error) = &exchange.error {
                ui.colored_label(Color32::LIGHT_RED, error)
                    .on_hover_text(error);
            }
        });
        ui.separator();
        if self.selected.is_none() {
            return;
        }
        let width = ui.available_width();
        egui::Panel::left("network-request")
            .default_size(width * 0.5)
            .size_range(200.0..=(width - 200.0).max(200.0))
            .resizable(true)
            .show(ui, |ui| self.exchange_pane(ui, &exchange, false));
        egui::CentralPanel::default().show(ui, |ui| self.exchange_pane(ui, &exchange, true));
        if let Some(action) = action {
            self.network_action(exchange, action, ui.ctx());
        }
    }

    fn exchange_pane(&mut self, ui: &mut egui::Ui, exchange: &Exchange, response: bool) {
        ui.push_id(("network-message", response), |ui| {
            let mut action = None;
            let (headers, body) = if response {
                exchange
                    .response
                    .as_ref()
                    .map(|value| (value.headers.as_slice(), value.body.as_ref()))
                    .unwrap_or((&[], None))
            } else {
                (
                    exchange.request.headers.as_slice(),
                    exchange.request.body.as_ref(),
                )
            };
            let content = body.and_then(|body| self.bodies.get(&body.id)).cloned();
            let header = ui.horizontal(|ui| {
                ui.strong(if response { "Response" } else { "Request" });
                if response && let Some(value) = &exchange.response {
                    ui.colored_label(
                        if value.status >= 400 {
                            Color32::LIGHT_RED
                        } else {
                            Color32::from_rgb(115, 222, 193)
                        },
                        value.status.to_string(),
                    );
                }
                if let Some(body) = body {
                    ui.weak(format!("{} bytes", body.size))
                        .on_hover_text(&body.content_type);
                    if !body.complete {
                        ui.colored_label(Color32::YELLOW, "Truncated / interrupted");
                    }
                }
            });
            header
                .response
                .interact(egui::Sense::click())
                .context_menu(|ui| {
                    action = menu(ui, exchange, self.network_state.clearing, Some(response));
                });
            let detail = &mut self.network_state.detail;
            let (section, viewer) = if response {
                (&mut detail.response, &mut detail.response_viewer)
            } else {
                (&mut detail.request, &mut detail.request_viewer)
            };
            ui.horizontal_wrapped(|ui| {
                if let (Some(body), Some(content)) = (body, &content) {
                    if viewer.network_controls(ui, &body.id, content, *section == Section::Body) {
                        *section = Section::Body;
                    }
                } else {
                    ui.selectable_value(section, Section::Body, "Body");
                }
                ui.selectable_value(section, Section::Headers, "Headers");
                if !response {
                    ui.selectable_value(section, Section::Parameters, "Parameters");
                }
            });
            ui.separator();
            let section = *section;
            match section {
                Section::Headers => {
                    ui.weak(format!("{} headers", headers.len()));
                    crate::http_view::pairs(ui, ("network-headers", response), headers);
                }
                Section::Parameters => {
                    let pairs = crate::http_view::query(&exchange.request.url);
                    ui.weak(format!("{} query parameters", pairs.len()));
                    crate::http_view::pairs(ui, "network-parameters", &pairs);
                }
                Section::Body => {
                    if let Some(body) = body {
                        if let Some(content) = content {
                            let viewer = if response {
                                &mut self.network_state.detail.response_viewer
                            } else {
                                &mut self.network_state.detail.request_viewer
                            };
                            let clearing = self.network_state.clearing;
                            viewer.show_network(ui, &body.id, content, |ui| {
                                action = menu(ui, exchange, clearing, Some(response));
                            });
                        } else {
                            self.load(&body.id);
                            ui.weak("Loading captured body…");
                        }
                    } else {
                        ui.weak(if response && exchange.response.is_none() {
                            "Waiting for response…"
                        } else {
                            "No body captured."
                        });
                    }
                }
            }
            if let Some(action) = action {
                self.network_action(exchange.clone(), action, ui.ctx());
            }
        });
    }
}
