mod gps;
mod project;
pub use gps::*;
pub use project::*;
mod history;
pub use history::*;
mod logcat;
use async_trait::async_trait;
use inspector_domain::*;
pub use logcat::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::mpsc;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub retention_days: u32,
    pub quota_bytes: u64,
    pub dark: bool,
    #[serde(default = "default_auto_observe")]
    pub auto_observe: bool,
    #[serde(default)]
    pub recent_projects: Vec<PathBuf>,
}
fn default_auto_observe() -> bool {
    true
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            retention_days: 30,
            quota_bytes: 5 * 1024 * 1024 * 1024,
            dark: true,
            auto_observe: true,
            recent_projects: Vec::new(),
        }
    }
}
#[derive(Clone, Debug)]
pub struct Session {
    pub uri: String,
    pub isolate: String,
    pub vm_pid: Option<u32>,
    pub hello: Hello,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeStatus {
    Ready,
    Native,
    Missing,
    Unavailable(String),
}
impl BridgeStatus {
    pub fn connectable(&self) -> bool {
        matches!(self, Self::Ready | Self::Native)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredApp {
    pub uri: String,
    pub isolate: String,
    pub name: String,
    pub device: String,
    pub session_id: String,
    pub busy: bool,
    pub bridge: BridgeStatus,
}
#[derive(Clone, Debug, Default)]
pub struct DiscoveryReport {
    pub apps: Vec<DiscoveredApp>,
    pub notes: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct BodyContent {
    pub bytes: Vec<u8>,
    pub text: String,
    pub json: Option<inspector_domain::json::JsonDocument>,
    pub pretty: Option<String>,
    pub form: Option<Vec<Header>>,
}
impl BodyContent {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let json = if bytes.len() <= 5 * 1024 * 1024 {
            inspector_domain::json::JsonDocument::parse(&text).ok()
        } else {
            None
        };
        let pretty = json
            .as_ref()
            .and_then(|d| serde_json::to_string_pretty(&d.value).ok());
        let form = (json.is_none()
            && bytes.len() <= 5 * 1024 * 1024
            && text.contains('=')
            && !text.contains(['\n', '\r']))
        .then(|| {
            url::form_urlencoded::parse(text.as_bytes())
                .take(10_000)
                .map(|(name, value)| Header {
                    name: name.into_owned(),
                    value: value.into_owned(),
                })
                .collect()
        });
        Self {
            bytes,
            text,
            json,
            pretty,
            form,
        }
    }
}
#[derive(Clone, Debug)]
pub enum Command {
    Initialize,
    GpsDevices {
        request_id: u64,
    },
    Gps(GpsCommand),
    GpsRoute {
        request_id: u64,
        points: Vec<GeoPoint>,
    },
    GpsImport {
        request_id: u64,
        path: PathBuf,
    },
    GpsExport {
        path: PathBuf,
        points: Vec<GeoPoint>,
    },
    Discover,
    LogcatDevices {
        request_id: u64,
        uri: Option<String>,
    },
    Logcat(LogcatCommand),
    OpenProject {
        request_id: u64,
        path: PathBuf,
    },
    ProjectDevices {
        request_id: u64,
        sdk: PathBuf,
        cwd: PathBuf,
    },
    Runner(RunnerCommand),
    Connect {
        uri: String,
        isolate: Option<String>,
    },
    Disconnect,
    History {
        request_id: u64,
        query: String,
        offset: usize,
    },
    ClearHistory {
        target: HistoryTarget,
    },
    LoadBody(String),
    SaveDraft(Draft),
    DeleteDraft(String),
    Replay {
        draft: Draft,
        run_id: String,
    },
    LoadRepeater(String),
    CancelReplay(String),
    Provider {
        tag: String,
        method: String,
        params: Value,
    },
    SetRule(InterceptRule),
    Decide(Value),
    Pin {
        id: String,
        pinned: bool,
    },
    ExportHar(PathBuf),
    ImportHar(PathBuf),
    SaveSettings(Settings),
    Shutdown,
}
#[derive(Clone, Debug)]
pub enum Event {
    Gps(GpsEvent),
    Ready {
        directory: PathBuf,
        drafts: Vec<Draft>,
        settings: Settings,
    },
    Logcat(LogcatEvent),
    ProjectOpened {
        request_id: u64,
        result: Result<FlutterProject, String>,
    },
    ProjectDevices {
        request_id: u64,
        result: Result<Vec<FlutterDevice>, String>,
    },
    Runner(RunnerEvent),
    Connected(Session),
    DriftDatabasesChanged {
        isolate: String,
    },
    ConnectionFailed(String),
    Discovered(DiscoveryReport),
    Disconnected,
    Isolates(Vec<(String, String)>),
    History {
        request_id: u64,
        rows: Vec<Exchange>,
        total: usize,
        query: String,
        offset: usize,
    },
    HistoryCleared {
        target: HistoryTarget,
        result: Result<usize, String>,
    },
    Changed,
    Body {
        id: String,
        content: Arc<BodyContent>,
    },
    Pending(Vec<Pending>),
    Provider {
        tag: String,
        value: Value,
    },
    ProviderFailed {
        tag: String,
        error: String,
    },
    Drafts(Vec<Draft>),
    RepeaterLoaded {
        draft_id: String,
        result: Result<Option<Exchange>, String>,
    },
    ReplayFinished {
        draft_id: String,
        run_id: String,
        result: Result<Exchange, String>,
    },
    Notice(String),
    Error(String),
}
#[derive(Clone)]
pub struct EventSink {
    sender: mpsc::Sender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
}
impl EventSink {
    /// Keep room for command results when a high-volume log stream fills the queue.
    pub fn try_send(&self, event: Event) -> bool {
        if self.sender.capacity() < 32 {
            return false;
        }
        if self.sender.try_send(event).is_err() {
            return false;
        }
        (self.wake)();
        true
    }
    pub async fn send(&self, event: Event) {
        if self.sender.send(event).await.is_ok() {
            (self.wake)();
        }
    }
}
/// Port implemented by external-service adapters. No UI types cross this boundary.
#[async_trait]
pub trait Backend: Send + Sync + 'static {
    async fn handle(&self, command: Command) -> Result<(), String>;
    async fn tick(&self) -> Result<(), String>;
}
pub struct Worker {
    pub commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Worker {
    pub fn start<B, F>(wake: impl Fn() + Send + Sync + 'static, build: F) -> Self
    where
        B: Backend,
        F: FnOnce(EventSink) -> Result<B, String> + Send + 'static,
    {
        let (commands, mut receiver) = mpsc::channel::<Command>(64);
        let (sender, events) = mpsc::channel(256);
        let sink = EventSink {
            sender,
            wake: Arc::new(wake),
        };
        let thread = std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Cannot start worker: {e}");
                    return;
                }
            };
            runtime.block_on(async move {
                let backend=match build(sink.clone()){Ok(b)=>Arc::new(b),Err(e)=>{sink.send(Event::Error(e)).await;return;}};
                let mut ticker=tokio::time::interval(Duration::from_millis(350));
                let polling=Arc::new(std::sync::atomic::AtomicBool::new(false));
                let limit=Arc::new(tokio::sync::Semaphore::new(8));
                loop {
                    tokio::select! {
                        command=receiver.recv()=>{
                            let Some(command)=command else{break};
                            if matches!(command,Command::Shutdown) {
                                let _=backend.handle(Command::Shutdown).await; break;
                            }
                            if matches!(command,Command::Runner(_)|Command::Logcat(_)|Command::Gps(_)|Command::Replay{..}|Command::CancelReplay(_)|Command::LoadRepeater(_)|Command::SaveDraft(_)|Command::DeleteDraft(_)|Command::SaveSettings(_)|Command::Connect{..}|Command::Disconnect){
                                if let Err(e)=backend.handle(command).await{sink.send(Event::Error(e)).await;}
                                continue;
                            }
                            let Ok(permit)=limit.clone().acquire_owned().await else{break;};
                            let backend=backend.clone();let sink=sink.clone();
                            tokio::spawn(async move {
                                let _permit=permit;
                                if let Err(e)=backend.handle(command).await{sink.send(Event::Error(e)).await;}
                            });
                        }
                        _=ticker.tick()=>{
                            use std::sync::atomic::Ordering;
                            if !polling.swap(true,Ordering::SeqCst){
                                let backend=backend.clone();let sink=sink.clone();let polling=polling.clone();
                                tokio::spawn(async move{
                                    if let Err(e)=backend.tick().await{sink.send(Event::Error(e)).await;}
                                    polling.store(false,Ordering::SeqCst);
                                });
                            }
                        }
                    }
                }
            });
        });
        Self {
            commands,
            events,
            thread: Some(thread),
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.events.close();
        let _ = self.commands.try_send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while !thread.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct RecordingBackend {
        saved: std::sync::Mutex<Vec<String>>,
        closed: std::sync::mpsc::Sender<Vec<String>>,
    }
    #[async_trait]
    impl Backend for RecordingBackend {
        async fn handle(&self, command: Command) -> Result<(), String> {
            match command {
                Command::SaveDraft(draft) => self.saved.lock().unwrap().push(draft.id),
                Command::Shutdown => {
                    self.closed
                        .send(self.saved.lock().unwrap().clone())
                        .unwrap();
                }
                _ => {}
            }
            Ok(())
        }
        async fn tick(&self) -> Result<(), String> {
            Ok(())
        }
    }
    #[test]
    fn graceful_shutdown_flushes_ordered_draft_writes() {
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = Worker::start(
            || {},
            move |_| {
                Ok(RecordingBackend {
                    saved: std::sync::Mutex::new(vec![]),
                    closed: tx,
                })
            },
        );
        for n in 0..32 {
            worker
                .commands
                .blocking_send(Command::SaveDraft(Draft {
                    id: n.to_string(),
                    ..Default::default()
                }))
                .unwrap();
        }
        drop(worker);
        let saved = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(saved, (0..32).map(|n| n.to_string()).collect::<Vec<_>>());
    }
}
