use eframe::egui;
use std::sync::OnceLock;

pub fn icon() -> &'static egui::IconData {
    static ICON: OnceLock<egui::IconData> = OnceLock::new();
    ICON.get_or_init(|| {
        eframe::icon_data::from_png_bytes(include_bytes!("../../../packaging/icon.png"))
            .expect("Bundled application icon must be a valid PNG")
    })
}

pub fn texture(ctx: &egui::Context) -> egui::TextureHandle {
    let icon = icon();
    ctx.load_texture(
        "flutter-inspector-logo",
        egui::ColorImage::from_rgba_unmultiplied(
            [icon.width as usize, icon.height as usize],
            &icon.rgba,
        ),
        egui::TextureOptions::LINEAR,
    )
}
