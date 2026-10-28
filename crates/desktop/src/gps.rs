use crate::app::InspectorApp;
use eframe::egui::{self, Color32, RichText, Stroke, Vec2};
use inspector_application::{AndroidDevice, Command, GpsCommand, GpsEvent, GpsStatus};
use inspector_domain::{GeoPoint, GpsFix, GpsRoute, MAX_ROUTE_POINTS};
use std::{path::PathBuf, sync::mpsc};
use walkers::{HttpOptions, HttpTiles, Map, MapMemory, Tiles, lat_lon};

const MINT: Color32 = Color32::from_rgb(115, 222, 193);
type FileChoice = (bool, Option<PathBuf>);

pub struct GpsUi {
    devices: Vec<AndroidDevice>,
    serial: String,
    device_request: u64,
    devices_loading: bool,
    generation: u64,
    busy: bool,
    active: bool,
    status: Option<GpsStatus>,
    fix: GpsFix,
    speed_kmh: f64,
    nudge_meters: f64,
    points: Vec<GeoPoint>,
    route_length: f64,
    route_request: u64,
    route_loading: bool,
    repeat: bool,
    draw_route: bool,
    online: bool,
    tiles: Option<HttpTiles>,
    tiles_loading: Option<mpsc::Receiver<HttpTiles>>,
    memory: MapMemory,
    message: String,
    error: bool,
    file_choice: Option<mpsc::Receiver<FileChoice>>,
}
impl Default for GpsUi {
    fn default() -> Self {
        let mut memory = MapMemory::default();
        let _ = memory.set_zoom(15.0);
        Self {
            devices: vec![],
            serial: String::new(),
            device_request: 0,
            devices_loading: false,
            generation: 0,
            busy: false,
            active: false,
            status: None,
            fix: GpsFix::default(),
            speed_kmh: 30.0,
            nudge_meters: 10.0,
            points: vec![],
            route_length: 0.0,
            route_request: 0,
            route_loading: false,
            repeat: false,
            draw_route: false,
            online: true,
            tiles: None,
            tiles_loading: None,
            memory,
            message: "Choose a device, pick a point, then Start GPS.".into(),
            error: false,
            file_choice: None,
        }
    }
}
impl GpsUi {
    fn changed_route(&mut self) {
        self.route_request += 1;
        self.route_loading = false;
        self.route_length = GpsRoute::new(self.points.clone()).map_or(0.0, |r| r.length());
    }
    fn dialog(&mut self, save: bool, ctx: egui::Context) {
        if self.file_choice.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.file_choice = Some(receiver);
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new();
            let path = if save {
                dialog
                    .add_filter("GeoJSON route", &["geojson"])
                    .set_file_name("route.geojson")
                    .save_file()
            } else {
                dialog
                    .add_filter("GPS route", &["gpx", "geojson", "json"])
                    .pick_file()
            };
            let _ = sender.send((save, path));
            ctx.request_repaint();
        });
    }
}

impl InspectorApp {
    pub fn gps_tick(&mut self) {
        if let Some(tiles) = self
            .gps
            .tiles_loading
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok())
        {
            self.gps.tiles_loading = None;
            self.gps.tiles = Some(tiles);
        }
        let choice = self
            .gps
            .file_choice
            .as_ref()
            .and_then(|r| r.try_recv().ok());
        if let Some((save, path)) = choice {
            self.gps.file_choice = None;
            if let Some(path) = path {
                if save {
                    self.send(Command::GpsExport {
                        path,
                        points: self.gps.points.clone(),
                    });
                } else {
                    self.gps.route_request += 1;
                    let command = Command::GpsImport {
                        request_id: self.gps.route_request,
                        path,
                    };
                    self.gps.route_loading = self.worker.commands.try_send(command).is_ok();
                }
            }
        }
    }
    pub fn gps_event(&mut self, event: GpsEvent) {
        match event {
            GpsEvent::Devices {
                request_id,
                devices,
                error,
            } if request_id == self.gps.device_request => {
                self.gps.devices_loading = false;
                if !self.gps.active
                    && !self.gps.busy
                    && !devices
                        .iter()
                        .any(|d| d.serial == self.gps.serial && d.state == "device")
                {
                    self.gps.serial = devices
                        .iter()
                        .find(|d| d.state == "device")
                        .map(|d| d.serial.clone())
                        .unwrap_or_default();
                }
                self.gps.devices = devices;
                if let Some(error) = error {
                    self.gps.message = error;
                    self.gps.error = true;
                }
            }
            GpsEvent::Status(status) if status.generation == self.gps.generation => {
                self.gps.active = status.active;
                self.gps.busy = status.busy;
                if status.error {
                    self.notice(true, format!("GPS: {}", status.message));
                }
                self.gps.status = Some(status);
            }
            GpsEvent::Route { request_id, result } if request_id == self.gps.route_request => {
                self.gps.route_loading = false;
                match result {
                    Ok(points) => {
                        self.gps.points = points;
                        self.gps.changed_route();
                        if let Some(point) = self.gps.points.first().copied() {
                            self.gps.fix.point = point;
                            self.gps.memory.center_at(position(point));
                        }
                        self.gps.message =
                            "Route ready. Play route starts at its first point.".into();
                        self.gps.error = false;
                    }
                    Err(error) => {
                        self.gps.message = error;
                        self.gps.error = true;
                    }
                }
            }
            GpsEvent::Saved(result) => match result {
                Ok(path) => self.notice(false, format!("GPS route saved to {}", path.display())),
                Err(error) => self.notice(true, error),
            },
            _ => {}
        }
    }
    fn gps_command(&mut self, command: GpsCommand) {
        if self.worker.commands.try_send(Command::Gps(command)).is_ok() {
            self.gps.busy = true;
        } else {
            self.notice(true, "GPS worker busy; try again".into());
        }
    }
    fn gps_devices(&mut self) {
        self.gps.device_request += 1;
        self.gps.devices_loading = self
            .worker
            .commands
            .try_send(Command::GpsDevices {
                request_id: self.gps.device_request,
            })
            .is_ok();
    }
    pub fn gps_ui(&mut self, ui: &mut egui::Ui) {
        if self.gps.device_request == 0 {
            self.gps_devices();
        }
        ui.horizontal(|ui| {
            ui.heading("GPS simulation");
            if self.gps.active {
                ui.colored_label(MINT, "MOCK ACTIVE");
            }
        });
        ui.label("Android emulator & USB device · geolocator · no Flutter bridge required");
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!self.gps.active && !self.gps.busy, |ui| {
                let selected = self
                    .gps
                    .devices
                    .iter()
                    .find(|d| d.serial == self.gps.serial)
                    .map(|d| d.label.as_str())
                    .unwrap_or("Select Android device");
                egui::ComboBox::from_id_salt("gps-device")
                    .selected_text(selected)
                    .width(250.0)
                    .show_ui(ui, |ui| {
                        for device in &self.gps.devices {
                            ui.add_enabled_ui(device.state == "device", |ui| {
                                ui.selectable_value(
                                    &mut self.gps.serial,
                                    device.serial.clone(),
                                    format!("{} · {}", device.label, device.serial),
                                );
                            });
                        }
                    });
            });
            if ui
                .add_enabled(
                    !self.gps.devices_loading,
                    egui::Button::new("Refresh devices"),
                )
                .clicked()
            {
                self.gps_devices();
            }
            if self.gps.busy || self.gps.devices_loading {
                ui.spinner();
            }
            if self.gps.active {
                if ui
                    .add_enabled(!self.gps.busy, egui::Button::new("Stop & restore GPS"))
                    .clicked()
                {
                    self.gps_command(GpsCommand::Stop);
                }
                ui.add_enabled_ui(!self.gps.busy, |ui| {
                    if ui
                        .add_enabled(
                            self.gps.active && !self.gps.busy && self.gps.route_length > 0.0,
                            egui::Button::new("Play route"),
                        )
                        .clicked()
                    {
                        self.gps_command(GpsCommand::Play {
                            points: self.gps.points.clone(),
                            speed: self.gps.speed_kmh / 3.6,
                            repeat: self.gps.repeat,
                        });
                    }
                    let playing = self.gps.status.as_ref().is_some_and(|s| s.playing);
                    let paused = self.gps.status.as_ref().is_some_and(|s| s.paused);
                    if ui
                        .add_enabled(
                            playing,
                            egui::Button::new(if paused { "Resume" } else { "Pause" }),
                        )
                        .clicked()
                    {
                        self.gps_command(if paused {
                            GpsCommand::Resume
                        } else {
                            GpsCommand::Pause
                        });
                    }
                });
            } else {
                let available = !self.gps.busy && !self.gps.serial.is_empty();
                if ui
                    .add_enabled(available, egui::Button::new("Start GPS"))
                    .clicked()
                {
                    self.gps.generation += 1;
                    self.gps_command(GpsCommand::Start {
                        generation: self.gps.generation,
                        serial: self.gps.serial.clone(),
                        fix: self.gps.fix,
                    });
                }
                if ui
                    .add_enabled(available, egui::Button::new("Restore GPS"))
                    .on_hover_text("Recover an interrupted Inspector session on this device")
                    .clicked()
                {
                    self.gps.generation += 1;
                    self.gps_command(GpsCommand::Restore {
                        generation: self.gps.generation,
                        serial: self.gps.serial.clone(),
                    });
                }
            }
        });
        ui.small("Changes device-wide location. Android reports isMocked = true. Stop restores the original providers.");
        if let Some(status) = &self.gps.status {
            ui.colored_label(
                if status.error {
                    Color32::LIGHT_RED
                } else {
                    MINT
                },
                &status.message,
            );
        }
        ui.separator();
        egui::Panel::left("gps-controls")
            .resizable(true)
            .default_size(280.0)
            .size_range(250.0..=380.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("gps-controls-scroll")
                    .show(ui, |ui| self.gps_controls(ui));
            });
        egui::CentralPanel::default().show(ui, |ui| self.gps_map(ui));
    }
    fn gps_controls(&mut self, ui: &mut egui::Ui) {
        ui.strong("Position");
        egui::Grid::new("gps-position")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                for (label, value, range, speed) in [
                    (
                        "Latitude",
                        &mut self.gps.fix.point.latitude,
                        -90.0..=90.0,
                        0.00001,
                    ),
                    (
                        "Longitude",
                        &mut self.gps.fix.point.longitude,
                        -180.0..=180.0,
                        0.00001,
                    ),
                    (
                        "Altitude (m)",
                        &mut self.gps.fix.point.altitude,
                        -12000.0..=100000.0,
                        1.0,
                    ),
                    (
                        "Accuracy (m)",
                        &mut self.gps.fix.accuracy,
                        0.1..=10000.0,
                        0.1,
                    ),
                ] {
                    ui.label(label);
                    ui.add(
                        egui::DragValue::new(value)
                            .range(range)
                            .speed(speed)
                            .max_decimals(6),
                    );
                    ui.end_row();
                }
            });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.gps.active && !self.gps.busy,
                    egui::Button::new("Apply point"),
                )
                .clicked()
            {
                self.gps_command(GpsCommand::SetFix(self.gps.fix));
            }
            if ui.button("Center map").clicked() {
                self.gps.memory.center_at(position(self.gps.fix.point));
            }
            if ui
                .button("Copy")
                .on_hover_text("Copy latitude, longitude")
                .clicked()
            {
                ui.ctx().copy_text(format!(
                    "{:.6}, {:.6}",
                    self.gps.fix.point.latitude, self.gps.fix.point.longitude
                ));
            }
        });
        ui.separator();
        ui.strong("Direction & speed");
        ui.add(egui::Slider::new(&mut self.gps.fix.heading, 0.0..=359.9).text("°"));
        ui.small("0° North · 90° East · 180° South · 270° West");
        ui.horizontal(|ui| {
            ui.label("Speed");
            ui.add(
                egui::DragValue::new(&mut self.gps.speed_kmh)
                    .range(0.1..=1440.0)
                    .speed(1.0)
                    .suffix(" km/h"),
            );
        });
        ui.add_enabled_ui(self.gps.active && !self.gps.busy, |ui| {
            if ui.button("Drive / update direction").clicked() {
                self.gps_command(GpsCommand::Drive {
                    heading: self.gps.fix.heading,
                    speed: self.gps.speed_kmh / 3.6,
                });
            }
            ui.horizontal(|ui| {
                if ui.button("Use live position").clicked()
                    && let Some(s) = &self.gps.status
                {
                    self.gps.fix = s.fix;
                    self.gps.fix.speed = 0.0;
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label("Step");
            ui.add(
                egui::DragValue::new(&mut self.gps.nudge_meters)
                    .range(0.1..=10000.0)
                    .suffix(" m"),
            );
        });
        ui.horizontal(|ui| {
            for (label, heading) in [
                ("West", 270.0),
                ("North", 0.0),
                ("South", 180.0),
                ("East", 90.0),
            ] {
                if ui
                    .add_enabled(!self.gps.busy, egui::Button::new(label))
                    .clicked()
                {
                    let from = if self.gps.active {
                        self.gps
                            .status
                            .as_ref()
                            .map(|s| s.fix.point)
                            .unwrap_or(self.gps.fix.point)
                    } else {
                        self.gps.fix.point
                    };
                    self.gps.fix.point = from.moved(self.gps.nudge_meters, heading);
                    self.gps.fix.heading = heading;
                    if self.gps.active {
                        self.gps_command(GpsCommand::SetFix(self.gps.fix));
                    }
                }
            }
        });
        ui.separator();
        ui.strong("Route");
        ui.checkbox(&mut self.gps.draw_route, "Add waypoints by clicking map");
        ui.label(format!(
            "{} points · {:.2} km",
            self.gps.points.len(),
            self.gps.route_length / 1000.0
        ));
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.gps.points.len() < MAX_ROUTE_POINTS,
                    egui::Button::new("Add point"),
                )
                .clicked()
            {
                self.gps.points.push(self.gps.fix.point);
                self.gps.changed_route();
            }
            if ui
                .add_enabled(!self.gps.points.is_empty(), egui::Button::new("Undo"))
                .clicked()
            {
                self.gps.points.pop();
                self.gps.changed_route();
            }
            if ui
                .add_enabled(
                    !self.gps.points.is_empty(),
                    egui::Button::new("Clear route"),
                )
                .clicked()
            {
                self.gps.points.clear();
                self.gps.changed_route();
            }
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.gps.route_loading && self.gps.file_choice.is_none(),
                    egui::Button::new("Import GPX / JSON"),
                )
                .clicked()
            {
                self.gps.dialog(false, ui.ctx().clone());
            }
            if ui
                .add_enabled(
                    self.gps.route_length > 0.0 && self.gps.file_choice.is_none(),
                    egui::Button::new("Export"),
                )
                .clicked()
            {
                self.gps.dialog(true, ui.ctx().clone());
            }
        });
        if ui
            .add_enabled(
                !self.gps.route_loading && (2..=25).contains(&self.gps.points.len()),
                egui::Button::new("Get road directions"),
            )
            .on_hover_text(
                "Send these waypoints to OSRM and replace the route with driving directions",
            )
            .clicked()
        {
            self.gps.route_request += 1;
            self.gps.route_loading = self
                .worker
                .commands
                .try_send(Command::GpsRoute {
                    request_id: self.gps.route_request,
                    points: self.gps.points.clone(),
                })
                .is_ok();
        }
        ui.small("Road directions use OSRM. Imported/drawn routes work offline.");
        if self.gps.route_loading {
            ui.spinner();
        }
        ui.checkbox(&mut self.gps.repeat, "Loop route (returns to start)");
        ui.small("Playback uses the current route and speed. Replay to apply edits.");
        if let Some(status) = &self.gps.status {
            if let Some(total) = status.total {
                ui.add(
                    egui::ProgressBar::new((status.distance / total.max(1.0)) as f32)
                        .text(format!("{:.0} / {:.0} m", status.distance, total)),
                );
            }
            if status.active {
                ui.label(
                    RichText::new(format!(
                        "Live  {:.6}, {:.6}\n{:.1} km/h · {:.1}°",
                        status.fix.point.latitude,
                        status.fix.point.longitude,
                        status.fix.speed * 3.6,
                        status.fix.heading
                    ))
                    .monospace(),
                );
            }
        }
        ui.colored_label(
            if self.gps.error {
                Color32::LIGHT_RED
            } else {
                Color32::GRAY
            },
            &self.gps.message,
        );
    }
    fn gps_map(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.gps.online, "Map tiles")
                .on_hover_text(
                    "OpenStreetMap tiles need internet; coordinates and simulation work offline",
                );
            if ui.button("+").clicked() {
                let _ = self.gps.memory.zoom_in();
            }
            if ui.button("−").clicked() {
                let _ = self.gps.memory.zoom_out();
            }
            if ui.button("Follow live").clicked() {
                self.gps.memory.follow_my_position();
            }
            ui.hyperlink_to("© OpenStreetMap", "https://www.openstreetmap.org/copyright");
        });
        ui.small(if self.gps.draw_route {
            "Click: add waypoint · drag: pan · scroll: zoom"
        } else {
            "Click: select point · Apply point sends it to device"
        });
        if self.gps.online
            && self.gps.tiles.is_none()
            && self.gps.tiles_loading.is_none()
            && !self.directory.as_os_str().is_empty()
        {
            let (sender, receiver) = mpsc::channel();
            self.gps.tiles_loading = Some(receiver);
            let cache = self.directory.join("map-cache");
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let tiles = HttpTiles::with_options(
                    walkers::sources::OpenStreetMap,
                    HttpOptions {
                        cache: Some(cache),
                        user_agent: Some(walkers::HeaderValue::from_static(
                            "FlutterInspector/0.1 GPS desktop",
                        )),
                        ..Default::default()
                    },
                    ctx.clone(),
                );
                let _ = sender.send(tiles);
                ctx.request_repaint();
            });
        }
        let gps = &mut self.gps;
        let live = gps.status.as_ref().filter(|s| s.active).map(|s| s.fix);
        let anchor = live.map_or(gps.fix.point, |fix| fix.point);
        let tiles = if gps.online {
            gps.tiles.as_mut().map(|t| t as &mut dyn Tiles)
        } else {
            None
        };
        let mut clicked = None;
        Map::new(tiles, &mut gps.memory, position(anchor))
            .double_click_to_zoom(false)
            .show(ui, |ui, response, projector, _| {
                let painter = ui.painter().with_clip_rect(response.rect);
                for pair in gps.points.windows(2) {
                    let a = projector.project(position(pair[0])).to_pos2();
                    let b = projector.project(position(pair[1])).to_pos2();
                    painter.line_segment([a, b], Stroke::new(3.0, Color32::from_rgb(96, 160, 255)));
                }
                for (index, point) in gps.points.iter().enumerate() {
                    if gps.points.len() > 100 && index != 0 && index + 1 != gps.points.len() {
                        continue;
                    }
                    let at = projector.project(position(*point)).to_pos2();
                    painter.circle_filled(at, 4.0, Color32::LIGHT_BLUE);
                    if index == 0 || index + 1 == gps.points.len() {
                        painter.text(
                            at + Vec2::new(8.0, -10.0),
                            egui::Align2::LEFT_CENTER,
                            if index == 0 { "START" } else { "END" },
                            egui::FontId::proportional(11.0),
                            Color32::WHITE,
                        );
                    }
                }
                let selected = projector.project(position(gps.fix.point)).to_pos2();
                painter.circle_stroke(
                    selected,
                    8.0,
                    Stroke::new(2.0, Color32::from_rgb(255, 195, 100)),
                );
                painter.circle_filled(selected, 3.0, Color32::from_rgb(255, 195, 100));
                if let Some(fix) = live {
                    let center = projector.project(position(fix.point)).to_pos2();
                    painter.circle_filled(center, 7.0, MINT);
                    let direction = Vec2::new(
                        fix.heading.to_radians().sin() as f32,
                        -fix.heading.to_radians().cos() as f32,
                    );
                    painter.arrow(center, direction * 30.0, Stroke::new(3.0, MINT));
                }
                if response.clicked()
                    && let Some(at) = response.interact_pointer_pos()
                {
                    let at = projector.unproject(at.to_vec2());
                    clicked = Some(GeoPoint {
                        latitude: at.y(),
                        longitude: (at.x() + 180.0).rem_euclid(360.0) - 180.0,
                        altitude: gps.fix.point.altitude,
                    });
                }
            });
        if let Some(point) = clicked.filter(|p| p.validate().is_ok()) {
            gps.fix.point = point;
            if gps.draw_route && gps.points.len() < MAX_ROUTE_POINTS {
                gps.points.push(point);
                gps.changed_route();
            }
        }
    }
}

fn position(point: GeoPoint) -> walkers::Position {
    // Web Mercator cannot represent the poles; simulation still supports the full latitude range.
    lat_lon(point.latitude.clamp(-85.0, 85.0), point.longitude)
}

impl Drop for GpsUi {
    fn drop(&mut self) {
        // Walkers joins its HTTP runtime on drop. Keep that off the window thread.
        if let Some(tiles) = self.tiles.take() {
            std::thread::spawn(move || drop(tiles));
        }
    }
}
