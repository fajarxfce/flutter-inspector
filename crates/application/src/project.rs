use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LaunchProfile {
    pub name: String,
    pub source: String,
    pub cwd: PathBuf,
    pub program: String,
    pub tool_args: Vec<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub device: String,
    pub issue: Option<String>,
}
#[derive(Clone, Debug)]
pub struct FlutterProject {
    pub root: PathBuf,
    pub sdk: PathBuf,
    pub profiles: Vec<LaunchProfile>,
    pub notes: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct FlutterDevice {
    pub id: String,
    pub name: String,
    pub platform: String,
}
#[derive(Clone, Debug)]
pub struct LaunchRequest {
    pub sdk: PathBuf,
    pub profile: LaunchProfile,
    pub device: String,
}
#[derive(Clone, Debug)]
pub enum RunnerCommand {
    Start {
        generation: u64,
        request: Box<LaunchRequest>,
    },
    Reload,
    Restart,
    Stop,
}
#[derive(Clone, Debug)]
pub enum RunnerEvent {
    Output {
        generation: u64,
        lines: Vec<super::LogLine>,
        dropped: u64,
    },
    State {
        generation: u64,
        active: bool,
        ready: bool,
        message: String,
    },
    VmService {
        generation: u64,
        uri: String,
        device: String,
    },
}
