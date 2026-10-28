//! Small JSON lexer for visible preview lines; no parsing or full-body work per frame.
use eframe::egui::{self, Color32, FontId, TextFormat, text::LayoutJob};

pub fn json_line(text: &str, dark: bool, highlight: bool) -> LayoutJob {
    let palette = if dark {
        [
            Color32::from_rgb(113, 190, 235),
            Color32::from_rgb(151, 210, 160),
            Color32::from_rgb(239, 181, 112),
            Color32::from_rgb(199, 151, 238),
            Color32::from_gray(215),
        ]
    } else {
        [
            Color32::from_rgb(0, 85, 145),
            Color32::from_rgb(26, 109, 54),
            Color32::from_rgb(158, 76, 5),
            Color32::from_rgb(126, 45, 155),
            Color32::from_gray(45),
        ]
    };
    let bytes = text.as_bytes();
    let mut job = LayoutJob::default();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let color = match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                    } else if bytes[i] == b'"' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
                let key = text[i..].trim_start().starts_with(':');
                palette[usize::from(!key)]
            }
            b'-' | b'0'..=b'9' => {
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_digit() || b".eE+-".contains(&bytes[i]))
                {
                    i += 1;
                }
                palette[2]
            }
            b't' | b'f' | b'n' => {
                while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                    i += 1;
                }
                palette[3]
            }
            _ => {
                i += text[i..].chars().next().map_or(1, char::len_utf8);
                palette[4]
            }
        };
        job.append(
            &text[start..i],
            0.0,
            TextFormat {
                font_id: FontId::monospace(13.0),
                color,
                background: if highlight {
                    if dark {
                        Color32::from_rgb(75, 65, 18)
                    } else {
                        Color32::from_rgb(255, 238, 145)
                    }
                } else {
                    Color32::TRANSPARENT
                },
                ..Default::default()
            },
        );
    }
    job
}

pub fn editor(ui: &mut egui::Ui, text: &mut String, rows: usize) -> egui::Response {
    let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, width: f32| {
        let text = buffer.as_str();
        let mut job = if text.len() <= 64 * 1024 {
            json_line(text, ui.visuals().dark_mode, false)
        } else {
            LayoutJob::simple(
                text.to_owned(),
                FontId::monospace(13.0),
                ui.visuals().text_color(),
                width,
            )
        };
        job.wrap.max_width = width;
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };
    ui.add(
        egui::TextEdit::multiline(text)
            .code_editor()
            .desired_rows(rows)
            .desired_width(f32::INFINITY)
            .layouter(&mut layouter),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn colors_keys_values_and_escaped_utf8_without_changing_bytes() {
        let text = r#"{"nama":"é\\\"hi", "count":9007199254740993, "ok":true, "empty":null}"#;
        let job = json_line(text, true, false);
        assert_eq!(job.text, text);
        let colors: std::collections::HashSet<_> =
            job.sections.iter().map(|s| s.format.color).collect();
        assert_eq!(colors.len(), 5);
    }
}
