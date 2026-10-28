use crate::AndroidDevice;
use inspector_domain::DeviceClock;

#[derive(Clone, Debug)]
pub enum ClockAction {
    Read,
    SetTimezone(String),
    SetDateTime {
        date: String,
        time: String,
        timezone: String,
    },
    SyncComputer,
    SetAutomatic {
        time: bool,
        timezone: bool,
    },
    OpenSettings,
}

#[derive(Clone, Debug)]
pub struct ClockResult {
    pub clock: DeviceClock,
    pub message: String,
}

#[derive(Clone, Debug)]
pub enum ClockEvent {
    Devices {
        request_id: u64,
        devices: Vec<AndroidDevice>,
        suggested: Option<String>,
        error: Option<String>,
    },
    Finished {
        request_id: u64,
        serial: String,
        result: Result<ClockResult, String>,
    },
}
