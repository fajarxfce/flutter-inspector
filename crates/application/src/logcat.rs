#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AndroidDevice {
    pub serial: String,
    pub label: String,
    pub state: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Verbose,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
    Fatal,
}
impl LogLevel {
    pub const ALL: [Self; 6] = [
        Self::Verbose,
        Self::Debug,
        Self::Info,
        Self::Warn,
        Self::Error,
        Self::Fatal,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Verbose => "Verbose",
            Self::Debug => "Debug",
            Self::Info => "Info",
            Self::Warn => "Warn",
            Self::Error => "Error",
            Self::Fatal => "Fatal",
        }
    }
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub time: String,
    pub pid: u32,
    pub tid: u32,
    pub level: LogLevel,
    pub tag: String,
    pub message: String,
}
impl LogLine {
    pub fn text(&self) -> String {
        format!(
            "{} {:>5} {:>5} {} {}: {}",
            self.time,
            self.pid,
            self.tid,
            &self.level.label()[..1],
            self.tag,
            self.message
        )
    }
}

#[derive(Clone, Debug)]
pub enum LogcatCommand {
    Start { serial: String, generation: u64 },
    Clear { generation: u64 },
    Stop,
}

#[derive(Clone, Debug)]
pub enum LogcatEvent {
    Devices {
        request_id: u64,
        devices: Vec<AndroidDevice>,
        suggested: Option<String>,
        error: Option<String>,
    },
    Lines {
        generation: u64,
        lines: Vec<LogLine>,
        dropped: u64,
    },
    Status {
        generation: u64,
        running: bool,
        message: String,
    },
}
