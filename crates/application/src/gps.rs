use crate::AndroidDevice;
use inspector_domain::{GeoPoint, GpsFix};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum GpsCommand {
    Start {
        generation: u64,
        serial: String,
        fix: GpsFix,
    },
    SetFix(GpsFix),
    Drive {
        heading: f64,
        speed: f64,
    },
    Play {
        points: Vec<GeoPoint>,
        speed: f64,
        repeat: bool,
    },
    Pause,
    Resume,
    Stop,
    Restore {
        generation: u64,
        serial: String,
    },
}

#[derive(Clone, Debug)]
pub struct GpsStatus {
    pub generation: u64,
    pub busy: bool,
    pub active: bool,
    pub playing: bool,
    pub paused: bool,
    pub fix: GpsFix,
    pub distance: f64,
    pub total: Option<f64>,
    pub message: String,
    pub error: bool,
}

#[derive(Clone, Debug)]
pub enum GpsEvent {
    Devices {
        request_id: u64,
        devices: Vec<AndroidDevice>,
        error: Option<String>,
    },
    Status(GpsStatus),
    Route {
        request_id: u64,
        result: Result<Vec<GeoPoint>, String>,
    },
    Saved(Result<PathBuf, String>),
}
