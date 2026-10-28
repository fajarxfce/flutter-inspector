mod app;
mod branding;
mod connection;
mod diagnostics;
mod drift;
mod gps;
mod http_view;
mod logcat;
mod network;
mod project;
mod providers;
mod repeater;
mod syntax;
mod viewer;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Flutter Inspector")
            .with_app_id("dev.flutterinspector")
            .with_icon(branding::icon().clone())
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1000.0, 650.0])
            .with_resizable(true),
        renderer: eframe::Renderer::Glow,
        glow_options: eframe::egui_glow::GlowConfiguration {
            // Winit already schedules Wayland redraws using compositor callbacks.
            // EGL's second, blocking frame wait can stall the event loop when a
            // surface is hidden, preventing even compositor pings from being handled.
            vsync: !running_on_wayland(),
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "Flutter Inspector",
        options,
        Box::new(|cc| Ok(Box::new(app::InspectorApp::new(cc)))),
    )
}

fn running_on_wayland() -> bool {
    // Match winit's automatic backend selection, including inherited sockets.
    cfg!(target_os = "linux")
        && ["WAYLAND_DISPLAY", "WAYLAND_SOCKET"]
            .iter()
            .any(|key| std::env::var_os(key).is_some_and(|value| !value.is_empty()))
}
