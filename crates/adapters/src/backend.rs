use crate::{
    har, http, native,
    store::{Store, data_directory},
    vm::VmClient,
};
use async_trait::async_trait;
use base64::Engine;
use inspector_application::*;
use inspector_domain::*;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

struct Connection {
    vm: Arc<VmClient>,
    isolate: String,
    controller: String,
    cursor: AtomicU64,
    alive: Arc<AtomicBool>,
    heartbeat: tokio::task::AbortHandle,
    registry_watcher: tokio::task::AbortHandle,
    registry_changed: Arc<AtomicBool>,
    native: Option<native::NativeSession>,
    native_http: bool,
}
impl Connection {
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        if method == "drift.databases" {
            return tokio::time::timeout(Duration::from_secs(20), async {
                let mut providers = Vec::new();
                if !self.controller.is_empty() {
                    let hello: Hello = serde_json::from_value(
                        self.vm
                            .bridge(&self.isolate, &self.controller, "hello", json!({}))
                            .await?,
                    )
                    .map_err(|e| e.to_string())?;
                    providers.extend(hello.providers.into_iter().filter(|p| p.kind == "drift"));
                }
                if let Some(native) = &self.native {
                    match native.refresh_drift().await {
                        Ok(detected) => providers.extend(detected),
                        Err(error) if providers.is_empty() => return Err(error),
                        Err(_) => {}
                    }
                }
                serde_json::to_value(providers).map_err(|e| e.to_string())
            })
            .await
            .map_err(|_| {
                "Database discovery timed out; check whether the app is paused".to_string()
            })?;
        }
        if let Some(native) = &self.native {
            if self.controller.is_empty()
                || params["provider"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("native."))
                || (self.native_http && matches!(method, "events" | "body.read"))
            {
                return native.call(method, params).await;
            }
            if method == "detach" {
                let _ = native.call("detach", json!({})).await;
            }
        }
        self.vm
            .bridge(&self.isolate, &self.controller, method, params)
            .await
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.heartbeat.abort();
        self.registry_watcher.abort();
    }
}
struct ActiveReplay {
    run_id: String,
    task: tokio::task::AbortHandle,
    remote: Option<(Arc<Connection>, String)>,
}

pub struct Services {
    store: Arc<Mutex<Store>>,
    connection: Mutex<Option<Arc<Connection>>>,
    sink: EventSink,
    downloads: Arc<tokio::sync::Semaphore>,
    loading: Arc<Mutex<HashSet<String>>>,
    replays: Arc<Mutex<HashMap<String, ActiveReplay>>>,
    last_prune: Mutex<Instant>,
    capture_paused: AtomicBool,
    skipped_bodies: AtomicU64,
    discovery: tokio::sync::Mutex<crate::discovery::Discovery>,
    logcat: tokio::sync::Mutex<crate::logcat::Logcat>,
    runner: tokio::sync::Mutex<crate::runner::Runner>,
    gps: tokio::sync::Mutex<crate::gps::Gps>,
    stopping: AtomicBool,
    stop_discovery: tokio::sync::Notify,
}
impl Services {
    pub fn new(sink: EventSink, directory: Option<PathBuf>) -> Result<Self, String> {
        Ok(Self {
            store: Arc::new(Mutex::new(Store::open(
                directory.map(Ok).unwrap_or_else(data_directory)?,
            )?)),
            connection: Mutex::new(None),
            sink,
            downloads: Arc::new(tokio::sync::Semaphore::new(2)),
            loading: Arc::new(Mutex::new(HashSet::new())),
            replays: Arc::new(Mutex::new(HashMap::new())),
            last_prune: Mutex::new(Instant::now()),
            capture_paused: AtomicBool::new(false),
            skipped_bodies: AtomicU64::new(0),
            discovery: tokio::sync::Mutex::new(crate::discovery::Discovery::default()),
            logcat: tokio::sync::Mutex::new(Default::default()),
            runner: tokio::sync::Mutex::new(Default::default()),
            gps: tokio::sync::Mutex::new(Default::default()),
            stopping: AtomicBool::new(false),
            stop_discovery: tokio::sync::Notify::new(),
        })
    }
    fn connection(&self) -> Result<Arc<Connection>, String> {
        self.connection
            .lock()
            .map_err(|e| e.to_string())?
            .clone()
            .ok_or("Connect to a Flutter app first".into())
    }
    async fn disconnect(&self) {
        let old = self.connection.lock().ok().and_then(|mut c| c.take());
        if let Some(connection) = old {
            connection.heartbeat.abort();
            let _ =
                tokio::time::timeout(Duration::from_secs(2), connection.call("detach", json!({})))
                    .await;
        }
        self.sink.send(Event::Disconnected).await;
    }
    async fn disconnect_current(&self, expected: &Arc<Connection>) {
        let old = self.connection.lock().ok().and_then(|mut connection| {
            if connection
                .as_ref()
                .is_some_and(|c| Arc::ptr_eq(c, expected))
            {
                connection.take()
            } else {
                None
            }
        });
        if let Some(old) = old {
            old.heartbeat.abort();
            let _ =
                tokio::time::timeout(Duration::from_secs(2), old.call("detach", json!({}))).await;
            self.sink.send(Event::Disconnected).await;
        }
    }
    async fn connect(&self, uri: String, isolate: Option<String>) -> Result<(), String> {
        self.disconnect().await;
        let vm = VmClient::connect(&uri).await?;
        let info = vm.call("getVM", json!({})).await?;
        let mut candidates = vec![];
        for isolate in info["isolates"]
            .as_array()
            .ok_or("VM returned no isolates")?
        {
            let Some(id) = isolate["id"].as_str() else {
                continue;
            };
            let details = vm.call("getIsolate", json!({"isolateId":id})).await?;
            if native::extension(&details, "ext.flutter_inspector.call")
                || native::supported(&details)
            {
                candidates.push((
                    id.to_owned(),
                    isolate["name"].as_str().unwrap_or("isolate").to_owned(),
                ));
            }
        }
        candidates.sort_by_key(|(_, name)| name != "main");
        self.sink.send(Event::Isolates(candidates.clone())).await;
        let isolate = isolate
            .or_else(|| candidates.first().map(|v| v.0.clone()))
            .ok_or("No supported debug isolate found. Run the Flutter app in debug mode.")?;
        let details = vm.call("getIsolate", json!({"isolateId":isolate})).await?;
        let has_bridge = native::extension(&details, "ext.flutter_inspector.call");
        let controller = if has_bridge {
            uuid::Uuid::new_v4().to_string()
        } else {
            String::new()
        };
        let mut notifications = vm.subscribe_notifications();
        let _ = vm
            .call("streamListen", json!({"streamId":"Extension"}))
            .await;
        let (native, hello, native_http) = if has_bridge {
            let mut hello: Hello =
                serde_json::from_value(vm.bridge(&isolate, &controller, "hello", json!({})).await?)
                    .map_err(|e| e.to_string())?;
            hello.validate()?;
            let native_http = !hello.providers.iter().any(|p| p.kind == "dio");
            let native = if native::supported(&details) {
                match native::NativeSession::open_with_network(
                    vm.clone(),
                    &isolate,
                    &details,
                    native_http,
                )
                .await
                {
                    Ok((native, detected)) => {
                        hello.providers.extend(detected.providers);
                        Some(native)
                    }
                    Err(error) => {
                        self.sink
                            .send(Event::Notice(format!(
                                "Built-in inspectors unavailable: {error}"
                            )))
                            .await;
                        None
                    }
                }
            } else {
                None
            };
            (native, hello, native_http)
        } else {
            let (session, hello) =
                native::NativeSession::open(vm.clone(), &isolate, &details).await?;
            (Some(session), hello, true)
        };
        let is_native = !has_bridge;
        let alive = Arc::new(AtomicBool::new(true));
        let beat_vm = vm.clone();
        let beat_isolate = isolate.clone();
        let beat_controller = controller.clone();
        let beat_alive = alive.clone();
        let heartbeat = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if !matches!(
                    tokio::time::timeout(Duration::from_secs(5), async {
                        if is_native {
                            beat_vm.check_isolate(&beat_isolate).await?;
                            Ok(json!({"alive":true}))
                        } else {
                            beat_vm
                                .bridge(&beat_isolate, &beat_controller, "heartbeat", json!({}))
                                .await
                        }
                    })
                    .await,
                    Ok(Ok(_))
                ) {
                    beat_alive.store(false, Ordering::Relaxed);
                    break;
                }
            }
        })
        .abort_handle();
        let registry_changed = Arc::new(AtomicBool::new(false));
        let changed = registry_changed.clone();
        let watching_isolate = isolate.clone();
        let registry_watcher = tokio::spawn(async move {
            loop {
                match notifications.recv().await {
                    Ok(value)
                        if value["event"]["isolate"]["id"] == watching_isolate
                            && value["event"]["extensionKind"] == "drift:database-list-changed" =>
                    {
                        changed.store(true, Ordering::Relaxed);
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        changed.store(true, Ordering::Relaxed);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        })
        .abort_handle();
        *self.connection.lock().map_err(|e| e.to_string())? = Some(Arc::new(Connection {
            vm,
            isolate: isolate.clone(),
            controller,
            cursor: AtomicU64::new(0),
            alive,
            heartbeat,
            registry_watcher,
            registry_changed,
            native,
            native_http,
        }));
        self.sink
            .send(Event::Connected(Session {
                uri,
                isolate,
                hello,
                vm_pid: info["pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()),
            }))
            .await;
        Ok(())
    }
    fn download(&self, body: Body, connection: Arc<Connection>) {
        if self
            .store
            .lock()
            .map(|s| s.body_exists(&body.id))
            .unwrap_or(false)
        {
            return;
        }
        if !self
            .loading
            .lock()
            .map(|mut l| {
                if l.len() >= 256 {
                    self.skipped_bodies.fetch_add(1, Ordering::Relaxed);
                    false
                } else {
                    l.insert(body.id.clone())
                }
            })
            .unwrap_or(false)
        {
            return;
        }
        let store = self.store.clone();
        let sink = self.sink.clone();
        let limit = self.downloads.clone();
        let loading = self.loading.clone();
        tokio::spawn(async move {
            let result = async {
                let _permit = limit.acquire().await.map_err(|e| e.to_string())?;
                let mut bytes = Vec::new();
                while bytes.len() < body.size as usize && bytes.len() < BODY_LIMIT {
                    if !store
                        .lock()
                        .map_err(|e| e.to_string())?
                        .body_referenced(&body.id)?
                    {
                        return Ok(());
                    }
                    let chunk = connection
                        .call("body.read", json!({"id":body.id,"offset":bytes.len()}))
                        .await?;
                    let data = base64::engine::general_purpose::STANDARD
                        .decode(chunk["data"].as_str().ok_or("Missing body chunk")?)
                        .map_err(|e| e.to_string())?;
                    if data.is_empty() {
                        break;
                    }
                    if bytes.len() + data.len() > BODY_LIMIT {
                        return Err("Bridge body exceeds capture limit".into());
                    }
                    bytes.extend_from_slice(&data);
                }
                if bytes.len() != body.size as usize {
                    return Err("Body snapshot is incomplete".into());
                }
                let id = body.id.clone();
                tokio::task::spawn_blocking(move || {
                    store
                        .lock()
                        .map_err(|e| e.to_string())?
                        .put_body(&id, &bytes)
                })
                .await
                .map_err(|e| e.to_string())??;
                Ok::<_, String>(())
            }
            .await;
            if let Ok(mut l) = loading.lock() {
                l.remove(&body.id);
            }
            if let Err(error) = result {
                sink.send(Event::Notice(format!("Capture body unavailable: {error}")))
                    .await;
            } else {
                sink.send(Event::Changed).await;
            }
        });
    }
}
#[async_trait]
impl Backend for Services {
    async fn handle(&self, command: Command) -> Result<(), String> {
        match command {
            Command::Gps(command) => self.gps.lock().await.handle(command, self.sink.clone())?,
            Command::GpsDevices { request_id } => {
                if let LogcatEvent::Devices { devices, error, .. } =
                    crate::logcat::devices(request_id, None).await
                {
                    self.sink
                        .send(Event::Gps(GpsEvent::Devices {
                            request_id,
                            devices,
                            error,
                        }))
                        .await;
                }
            }
            Command::GpsRoute { request_id, points } => {
                let result = crate::gps::routes::directions(points).await;
                self.sink
                    .send(Event::Gps(GpsEvent::Route { request_id, result }))
                    .await;
            }
            Command::GpsImport { request_id, path } => {
                let result = tokio::task::spawn_blocking(move || crate::gps::routes::import(&path))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r);
                self.sink
                    .send(Event::Gps(GpsEvent::Route { request_id, result }))
                    .await;
            }
            Command::GpsExport { path, points } => {
                let result = tokio::task::spawn_blocking(move || {
                    crate::gps::routes::export(&path, points).map(|_| path)
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
                self.sink.send(Event::Gps(GpsEvent::Saved(result))).await;
            }
            Command::Initialize => {
                let (directory, drafts, settings) = {
                    let s = self.store.lock().map_err(|e| e.to_string())?;
                    (s.root.clone(), s.drafts()?, s.settings()?)
                };
                self.sink
                    .send(Event::Ready {
                        directory,
                        drafts,
                        settings,
                    })
                    .await;
            }
            Command::OpenProject { request_id, path } => {
                let result = tokio::task::spawn_blocking(move || crate::project::open(&path))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|v| v);
                self.sink
                    .send(Event::ProjectOpened { request_id, result })
                    .await;
            }
            Command::ProjectDevices {
                request_id,
                sdk,
                cwd,
            } => {
                let result = crate::project::devices(&sdk, &cwd).await;
                self.sink
                    .send(Event::ProjectDevices { request_id, result })
                    .await;
            }
            Command::Runner(command) => {
                self.runner
                    .lock()
                    .await
                    .handle(command, self.sink.clone())
                    .await?
            }
            Command::LogcatDevices { request_id, uri } => {
                self.sink
                    .send(Event::Logcat(crate::logcat::devices(request_id, uri).await))
                    .await;
            }
            Command::Logcat(command) => {
                self.logcat
                    .lock()
                    .await
                    .handle(command, self.sink.clone())
                    .await;
            }
            Command::Discover => {
                let stopped = self.stop_discovery.notified();
                tokio::pin!(stopped);
                stopped.as_mut().enable();
                if !self.stopping.load(Ordering::SeqCst) {
                    tokio::select! {
                        _ = stopped => {},
                        _ = async {
                            let report = self.discovery.lock().await.scan().await;
                            self.sink.send(Event::Discovered(report)).await;
                        } => {}
                    }
                }
            }
            Command::Connect { uri, isolate } => {
                let result =
                    tokio::time::timeout(Duration::from_secs(10), self.connect(uri, isolate))
                        .await
                        .unwrap_or_else(|_| Err("Flutter connection timed out".into()));
                if let Err(error) = result {
                    self.sink.send(Event::ConnectionFailed(error)).await;
                }
            }
            Command::Disconnect => self.disconnect().await,
            Command::Shutdown => {
                self.stopping.store(true, Ordering::SeqCst);
                self.stop_discovery.notify_waiters();
                let active: Vec<_> = self
                    .replays
                    .lock()
                    .map_err(|e| e.to_string())?
                    .drain()
                    .map(|(_, r)| r)
                    .collect();
                let mut cancellations = Vec::new();
                for replay in active {
                    replay.task.abort();
                    if let Some((connection, provider)) = replay.remote {
                        cancellations.push(async move {
                            let _ = connection
                                .call(
                                    "replay.cancel",
                                    json!({"id":replay.run_id,"provider":provider}),
                                )
                                .await;
                        });
                    }
                }
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    futures_util::future::join_all(cancellations),
                )
                .await;
                self.gps.lock().await.shutdown().await;
                self.logcat.lock().await.stop().await;
                self.runner.lock().await.shutdown().await;
                self.disconnect().await;
                self.discovery.lock().await.shutdown().await;
            }
            Command::History {
                request_id,
                query,
                offset,
            } => {
                let store = self.store.clone();
                let response_query = query.clone();
                let (rows, total) = tokio::task::spawn_blocking(move || {
                    store
                        .lock()
                        .map_err(|e| e.to_string())?
                        .history(&query, offset)
                })
                .await
                .map_err(|e| e.to_string())??;
                self.sink
                    .send(Event::History {
                        request_id,
                        rows,
                        total,
                        query: response_query,
                        offset,
                    })
                    .await;
            }
            Command::ClearHistory { target } => {
                let store = self.store.clone();
                let selection = target.clone();
                let result = tokio::task::spawn_blocking(move || {
                    store
                        .lock()
                        .map_err(|e| e.to_string())?
                        .clear_history(&selection)
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|result| result);
                self.sink
                    .send(Event::HistoryCleared { target, result })
                    .await;
            }
            Command::LoadBody(id) => {
                let store = self.store.clone();
                let key = id.clone();
                let content = tokio::task::spawn_blocking(move || {
                    let bytes = store.lock().map_err(|e| e.to_string())?.body(&key)?;
                    Ok::<_, String>(BodyContent::from_bytes(bytes))
                })
                .await
                .map_err(|e| e.to_string())??;
                self.sink
                    .send(Event::Body {
                        id,
                        content: Arc::new(content),
                    })
                    .await;
            }
            Command::SaveDraft(draft) => {
                let drafts = {
                    let store = self.store.lock().map_err(|e| e.to_string())?;
                    store.save_draft(&draft)?;
                    store.drafts()?
                };
                self.sink.send(Event::Drafts(drafts)).await;
            }
            Command::DeleteDraft(id) => {
                let drafts = {
                    let store = self.store.lock().map_err(|e| e.to_string())?;
                    store.delete_draft(&id)?;
                    store.drafts()?
                };
                self.sink.send(Event::Drafts(drafts)).await;
            }
            Command::LoadRepeater(draft_id) => {
                let result = self
                    .store
                    .lock()
                    .map_err(|e| e.to_string())?
                    .repeater_result(&draft_id);
                self.sink
                    .send(Event::RepeaterLoaded { draft_id, result })
                    .await;
            }
            Command::Replay { draft, run_id } => {
                let id = draft.id.clone();
                if self
                    .replays
                    .lock()
                    .map_err(|e| e.to_string())?
                    .contains_key(&id)
                {
                    self.sink
                        .send(Event::ReplayFinished {
                            draft_id: id,
                            run_id,
                            result: Err("This Repeater tab is already sending".into()),
                        })
                        .await;
                    return Ok(());
                }
                let connection = if draft.via_app {
                    match self.connection() {
                        Ok(c) => Some(c),
                        Err(error) => {
                            self.sink
                                .send(Event::ReplayFinished {
                                    draft_id: id,
                                    run_id,
                                    result: Err(error),
                                })
                                .await;
                            return Ok(());
                        }
                    }
                } else {
                    None
                };
                let pending = Exchange {
                    id: run_id.clone(),
                    repeater_id: Some(id.clone()),
                    client_id: id.clone(),
                    source: if draft.via_app {
                        "repeater_app"
                    } else {
                        "repeater_desktop"
                    }
                    .into(),
                    started_at: crate::store::now(),
                    request: Request {
                        method: draft.method.clone(),
                        url: draft.url.clone(),
                        headers: draft.headers.clone(),
                        body: None,
                    },
                    ..Default::default()
                };
                let reserved = self
                    .store
                    .lock()
                    .map_err(|e| e.to_string())?
                    .begin_repeater(&draft, &pending);
                if let Err(error) = reserved {
                    self.sink
                        .send(Event::ReplayFinished {
                            draft_id: id,
                            run_id,
                            result: Err(error),
                        })
                        .await;
                    return Ok(());
                }
                let remote = connection
                    .as_ref()
                    .map(|c| (c.clone(), draft.client_id.clone()));
                let store = self.store.clone();
                let sink = self.sink.clone();
                let replays = self.replays.clone();
                let task_id = id.clone();
                let task_run = run_id.clone();
                // Hold the map until insertion, so a fast request cannot finish before registration.
                let mut running = self.replays.lock().map_err(|e| e.to_string())?;
                let task = tokio::spawn(async move {
                    let result = if let Some(connection) = connection {
                        async {
                            let mut value =
                                serde_json::to_value(&draft).map_err(|e| e.to_string())?;
                            value["provider"] = json!(draft.client_id);
                            value["draftId"] = json!(draft.id);
                            value["id"] = json!(task_run);
                            let value = connection.call("replay", value).await?;
                            let exchange: Exchange = serde_json::from_value(
                                value["exchange"].clone(),
                            )
                            .map_err(|_| {
                                "Update the optional Dio bridge to return responses to Repeater"
                                    .to_owned()
                            })?;
                            if exchange.id != task_run
                                || exchange.repeater_id.as_deref() != Some(&task_id)
                            {
                                return Err("Mismatched Repeater response".into());
                            }
                            store.lock().map_err(|e| e.to_string())?.put(&exchange)?;
                            // Repeater owns these bytes independently of Network capture/polling.
                            for body in [
                                exchange.request.body.as_ref(),
                                exchange.response.as_ref().and_then(|r| r.body.as_ref()),
                            ]
                            .into_iter()
                            .flatten()
                            {
                                let mut bytes = Vec::new();
                                while bytes.len() < body.size as usize && bytes.len() < BODY_LIMIT {
                                    let chunk = connection
                                        .call(
                                            "body.read",
                                            json!({"id":body.id,"offset":bytes.len()}),
                                        )
                                        .await?;
                                    let data = base64::engine::general_purpose::STANDARD
                                        .decode(
                                            chunk["data"]
                                                .as_str()
                                                .ok_or("Missing response bytes")?,
                                        )
                                        .map_err(|e| e.to_string())?;
                                    if data.is_empty() || bytes.len() + data.len() > BODY_LIMIT {
                                        return Err("Incomplete Repeater body".into());
                                    }
                                    bytes.extend_from_slice(&data);
                                }
                                if bytes.len() != body.size as usize {
                                    return Err("Repeater body exceeds the capture limit".into());
                                }
                                store
                                    .lock()
                                    .map_err(|e| e.to_string())?
                                    .put_body(&body.id, &bytes)?;
                            }
                            Ok(exchange)
                        }
                        .await
                    } else {
                        http::replay_run(draft, store.clone(), task_run.clone()).await
                    };
                    if let Err(error) = &result
                        && let Ok(store) = store.lock()
                        && let Ok(Some(mut exchange)) = store.repeater_result(&task_id)
                        && exchange.id == task_run
                    {
                        exchange.state = ExchangeState::Error;
                        exchange.error = Some(error.clone());
                        let _ = store.put(&exchange);
                    }
                    let current = replays
                        .lock()
                        .map(|mut running| {
                            if running.get(&task_id).is_some_and(|r| r.run_id == task_run) {
                                running.remove(&task_id);
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false);
                    if current {
                        sink.send(Event::ReplayFinished {
                            draft_id: task_id,
                            run_id: task_run,
                            result,
                        })
                        .await;
                    }
                });
                running.insert(
                    id,
                    ActiveReplay {
                        run_id,
                        task: task.abort_handle(),
                        remote,
                    },
                );
            }
            Command::CancelReplay(id) => {
                let active = self.replays.lock().map_err(|e| e.to_string())?.remove(&id);
                if let Some(active) = active {
                    active.task.abort();
                    if let Some((connection, provider)) = active.remote {
                        let run_id = active.run_id.clone();
                        tokio::spawn(async move {
                            let _ = connection
                                .call("replay.cancel", json!({"id":run_id,"provider":provider}))
                                .await;
                        });
                    }
                    let result = {
                        let store = self.store.lock().map_err(|e| e.to_string())?;
                        match store.repeater_result(&id)? {
                            Some(mut exchange) => {
                                exchange.state = ExchangeState::Cancelled;
                                exchange.error = Some("Repeater request cancelled".into());
                                store.put(&exchange)?;
                                Ok(exchange)
                            }
                            None => Err("Repeater request cancelled".into()),
                        }
                    };
                    self.sink
                        .send(Event::ReplayFinished {
                            draft_id: id,
                            run_id: active.run_id,
                            result,
                        })
                        .await;
                }
            }
            Command::Provider {
                tag,
                method,
                params,
            } => {
                let result = match self.connection() {
                    Ok(connection) => connection.call(&method, params).await,
                    Err(error) => Err(error),
                };
                let event = match result {
                    Ok(value) => Event::Provider { tag, value },
                    Err(error) => Event::ProviderFailed { tag, error },
                };
                self.sink.send(event).await;
            }
            Command::SetRule(rule) => {
                self.connection()?
                    .call(
                        "intercept.rules",
                        serde_json::to_value(rule).map_err(|e| e.to_string())?,
                    )
                    .await?;
                self.sink
                    .send(Event::Notice("Intercept rules updated".into()))
                    .await;
            }
            Command::Decide(value) => {
                self.connection()?.call("network.decide", value).await?;
            }
            Command::Pin { id, pinned } => {
                self.store
                    .lock()
                    .map_err(|e| e.to_string())?
                    .pin(&id, pinned)?;
                self.sink.send(Event::Changed).await;
            }
            Command::ExportHar(path) => {
                let store = self.store.clone();
                tokio::task::spawn_blocking(move || {
                    let guard = store.lock().map_err(|e| e.to_string())?;
                    har::export(&guard, &path)
                })
                .await
                .map_err(|e| e.to_string())??;
                self.sink.send(Event::Notice("HAR exported".into())).await;
            }
            Command::ImportHar(path) => {
                let store = self.store.clone();
                let count = tokio::task::spawn_blocking(move || {
                    let guard = store.lock().map_err(|e| e.to_string())?;
                    har::import(&guard, &path)
                })
                .await
                .map_err(|e| e.to_string())??;
                self.sink
                    .send(Event::Notice(format!("Imported {count} exchanges")))
                    .await;
                self.sink.send(Event::Changed).await;
            }
            Command::SaveSettings(settings) => {
                self.store
                    .lock()
                    .map_err(|e| e.to_string())?
                    .save_settings(&settings)?;
                self.sink
                    .send(Event::Notice(
                        "Settings saved. Retention refreshes within 30 seconds.".into(),
                    ))
                    .await;
            }
        }
        Ok(())
    }
    async fn tick(&self) -> Result<(), String> {
        if let Ok(connection) = self.connection() {
            if !connection.alive.load(Ordering::Relaxed) {
                self.disconnect_current(&connection).await;
                return Ok(());
            }
            if connection.registry_changed.swap(false, Ordering::Relaxed) {
                self.sink
                    .send(Event::DriftDatabasesChanged {
                        isolate: connection.isolate.clone(),
                    })
                    .await;
            }
            let page = connection
                .call(
                    "events",
                    json!({"after":connection.cursor.load(Ordering::Relaxed)}),
                )
                .await;
            let page: EventPage =
                match page.and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string())) {
                    Ok(page) => page,
                    Err(e) => {
                        self.disconnect_current(&connection).await;
                        return Err(e);
                    }
                };
            connection.cursor.store(page.cursor, Ordering::Relaxed);
            let changed = !page.events.is_empty();
            for event in page.events {
                if event.kind == "network" && !self.capture_paused.load(Ordering::Relaxed) {
                    let exchange: Exchange =
                        serde_json::from_value(event.data).map_err(|e| e.to_string())?;
                    let retained = self
                        .store
                        .lock()
                        .map_err(|e| e.to_string())?
                        .put(&exchange)?;
                    if !retained {
                        continue;
                    }
                    for body in [
                        exchange.request.body,
                        exchange.response.and_then(|r| r.body),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        self.download(body, connection.clone());
                    }
                }
            }
            self.sink.send(Event::Pending(page.pending)).await;
            if page.dropped > 0 {
                self.sink
                    .send(Event::Notice(format!(
                        "{} capture events expired before retrieval",
                        page.dropped
                    )))
                    .await;
            }
            if changed {
                self.sink.send(Event::Changed).await;
            }
        }
        let prune = {
            let mut last = self.last_prune.lock().map_err(|e| e.to_string())?;
            if last.elapsed() > Duration::from_secs(30) {
                *last = Instant::now();
                true
            } else {
                false
            }
        };
        if prune {
            let store = self.store.clone();
            let capacity = tokio::task::spawn_blocking(move || {
                let store = store.lock().map_err(|e| e.to_string())?;
                store.prune(&store.settings()?)
            })
            .await
            .map_err(|e| e.to_string())??;
            let was_paused = self.capture_paused.swap(!capacity, Ordering::Relaxed);
            if !capacity && !was_paused {
                self.sink.send(Event::Notice("Capture paused: saved requests fill the storage quota. Remove saved data or increase the quota.".into())).await;
            } else if capacity && was_paused {
                self.sink
                    .send(Event::Notice("Storage available; capture resumed.".into()))
                    .await;
            }
        }
        let skipped = self.skipped_bodies.swap(0, Ordering::Relaxed);
        if skipped > 0 {
            self.sink
                .send(Event::Notice(format!(
                    "{skipped} body downloads skipped because the capture queue is full"
                )))
                .await;
        }
        Ok(())
    }
}
