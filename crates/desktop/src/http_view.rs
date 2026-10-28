use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use inspector_domain::{Header, headers_text};

pub fn pairs(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, values: &[Header]) {
    TableBuilder::new(ui)
        .id_salt(id)
        .striped(true)
        .resizable(true)
        .column(Column::initial(150.0).at_least(80.0))
        .column(Column::remainder().at_least(100.0))
        .header(24.0, |mut row| {
            row.col(|ui| {
                ui.strong("Name");
            });
            row.col(|ui| {
                ui.strong("Value");
            });
        })
        .body(|body| {
            body.rows(27.0, values.len(), |mut row| {
                let pair = &values[row.index()];
                row.col(|ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&pair.name).monospace().color(
                            if ui.visuals().dark_mode {
                                Color32::from_rgb(113, 190, 235)
                            } else {
                                Color32::from_rgb(0, 85, 145)
                            },
                        ))
                        .truncate()
                        .selectable(true),
                    )
                    .on_hover_text(&pair.name);
                });
                row.col(|ui| {
                    let preview: String = pair.value.chars().take(4096).collect();
                    ui.add(
                        egui::Label::new(RichText::new(preview).monospace())
                            .truncate()
                            .selectable(true),
                    )
                    .on_hover_text(&pair.value)
                    .context_menu(|ui| {
                        if ui.button("Copy value").clicked() {
                            ui.ctx().copy_text(pair.value.clone());
                            ui.close();
                        }
                        if ui.button("Copy name: value").clicked() {
                            ui.ctx().copy_text(format!("{}: {}", pair.name, pair.value));
                            ui.close();
                        }
                    });
                });
            })
        });
}
pub fn headers(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, values: &[Header]) {
    ui.horizontal(|ui| {
        ui.strong(format!("Headers ({})", values.len()));
        if ui.small_button("Copy headers").clicked() {
            ui.ctx().copy_text(headers_text(values));
        }
    });
    if values.is_empty() {
        ui.weak("No headers.");
    } else {
        pairs(ui, id, values);
    }
}
pub fn query(url: &str) -> Vec<Header> {
    url::Url::parse(url)
        .map(|url| {
            url.query_pairs()
                .map(|(name, value)| Header {
                    name: name.into_owned(),
                    value: value.into_owned(),
                })
                .collect()
        })
        .unwrap_or_default()
}
pub fn timestamp(millis: i64, full: bool) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format(if full {
                    "%Y-%m-%d %H:%M:%S%.3f %:z"
                } else {
                    "%H:%M:%S%.3f"
                })
                .to_string()
        })
        .unwrap_or_else(|| "—".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parameters_preserve_duplicates_empty_values_and_decoding() {
        let pairs = query("https://example.test/?q=a+b&q=%2B&empty=");
        assert_eq!(
            pairs.iter().map(|p| p.value.as_str()).collect::<Vec<_>>(),
            vec!["a b", "+", ""]
        );
        assert_eq!(pairs[0].name, pairs[1].name);
        assert_eq!(query("https://example.test/?x=%26%3D")[0].value, "&=");
    }
}
