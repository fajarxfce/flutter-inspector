//! Bounded, lease-free discovery. Running VMs remain visible before SDK setup.
use crate::vm::VmClient;
use futures_util::{StreamExt, stream};
use inspector_application::{BridgeStatus, DiscoveredApp, DiscoveryReport};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{io::AsyncReadExt, process::Command};

const LIMIT: usize = 32;
const PROCESS_LIMIT: u64 = 1024 * 1024;

#[derive(Clone, Debug)]
struct Candidate {
    uri: String,
    isolate: Option<String>,
    device: String,
    name: String,
}
#[derive(Deserialize)]
struct Advertisement {
    version: u32,
    uri: String,
    isolate: Option<String>,
    #[serde(rename = "updatedAt")]
    updated_at: u64,
    #[serde(default)]
    name: String,
}
#[derive(Debug, PartialEq)]
pub(crate) struct Device {
    pub serial: String,
    pub label: String,
    pub state: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Forward {
    serial: String,
    local: u16,
    remote: u16,
}

pub struct Discovery {
    adb: PathBuf,
    // Only forwards allocated by this instance are eligible for cleanup.
    owned: HashSet<Forward>,
    // Retain validated startup records after logcat rotates.
    android: HashMap<(String, u32), String>,
}
impl Default for Discovery {
    fn default() -> Self {
        Self {
            adb: adb_path(),
            owned: HashSet::new(),
            android: HashMap::new(),
        }
    }
}
impl Discovery {
    pub async fn scan(&mut self) -> DiscoveryReport {
        let mut report = DiscoveryReport {
            apps: local_apps().await,
            ..Default::default()
        };
        // IDE-owned DDS keeps the original VM URI even after Android logs rotate.
        report.apps.extend(tooling_apps().await);
        match self.scan_android(&mut report.notes).await {
            Ok(apps) => report.apps.extend(apps),
            Err(error) => report.notes.push(error),
        }
        let mut seen = HashSet::new();
        report.apps.retain(|app| {
            let key = if app.session_id.is_empty() {
                (app.uri.clone(), app.isolate.clone())
            } else {
                (app.session_id.clone(), app.isolate.clone())
            };
            seen.insert(key)
        });
        report.apps.sort_by(|a, b| {
            (&a.device, &a.name, &a.isolate).cmp(&(&b.device, &b.name, &b.isolate))
        });
        report
            .apps
            .dedup_by(|a, b| a.uri == b.uri && a.isolate == b.isolate);
        report.apps.truncate(LIMIT);
        if report.apps.is_empty() {
            report.notes.push("No running Flutter VM found. Start a debug session; if Android startup logs were cleared, restart the debug app.".into());
        }
        report
    }

    async fn adb(&self, args: &[&str]) -> Result<String, String> {
        run(&self.adb, args).await
    }

    async fn scan_android(
        &mut self,
        notes: &mut Vec<String>,
    ) -> Result<Vec<DiscoveredApp>, String> {
        let devices = parse_devices(&self.adb(&["devices", "-l"]).await.map_err(|error| {
            format!("Android discovery: {error}. Install Android SDK Platform-Tools or set INSPECTOR_ADB.")
        })?);
        let listing = self.adb(&["forward", "--list"]).await?;
        let mut forwards = parse_forwards(&listing);
        self.owned.retain(|f| forwards.contains(f));
        let mut used = HashSet::new();
        let mut seen_processes = HashSet::new();
        let mut apps = Vec::new();
        for device in devices.into_iter().take(8) {
            if device.state != "device" {
                notes.push(format!(
                    "{}: {}. Unlock the device and accept USB debugging.",
                    device.label, device.state
                ));
                // Preserve owned forwards while this device is temporarily offline.
                used.extend(
                    self.owned
                        .iter()
                        .filter(|f| f.serial == device.serial)
                        .cloned(),
                );
                continue;
            }
            let serial = device.serial.as_str();
            let ps_args = ["-s", serial, "shell", "ps", "-A", "-o", "PID,NAME"];
            let log_args = [
                "-s",
                serial,
                "logcat",
                "-d",
                "-b",
                "main",
                "-v",
                "brief",
                "-e",
                "The Dart VM service is listening on",
                "flutter:I",
                "*:S",
            ];
            let result = tokio::try_join!(self.adb(&ps_args), self.adb(&log_args));
            let (processes, logs) = match result {
                Ok(result) => result,
                Err(error) => {
                    notes.push(format!("{}: {error}", device.label));
                    used.extend(self.owned.iter().filter(|f| f.serial == serial).cloned());
                    // Keep cached endpoints through transient USB failures.
                    seen_processes
                        .extend(self.android.keys().filter(|(s, _)| s == serial).cloned());
                    continue;
                }
            };
            let processes = parse_processes(&processes);
            for pid in processes.keys() {
                seen_processes.insert((serial.to_owned(), *pid));
            }
            for (pid, uri) in parse_vm_logs(&logs, &processes) {
                self.android.insert((serial.to_owned(), pid), uri);
            }
            let endpoints: Vec<_> = self
                .android
                .iter()
                .filter(|((s, pid), _)| s == serial && processes.contains_key(pid))
                .take(LIMIT)
                .map(|((_, pid), uri)| (*pid, uri.clone()))
                .collect();
            for (pid, uri) in endpoints {
                let Some(mut url) = loopback_vm_uri(&uri) else {
                    continue;
                };
                let Some(remote) = url.port() else { continue };
                let forward = if let Some(existing) = forwards
                    .iter()
                    .find(|f| f.serial == serial && f.remote == remote)
                {
                    existing.clone()
                } else {
                    let remote_arg = format!("tcp:{remote}");
                    let port = self
                        .adb(&["-s", serial, "forward", "tcp:0", &remote_arg])
                        .await?;
                    let local = port
                        .trim()
                        .parse::<u16>()
                        .map_err(|_| "ADB returned an invalid forwarded port")?;
                    let forward = Forward {
                        serial: serial.into(),
                        local,
                        remote,
                    };
                    self.owned.insert(forward.clone());
                    forwards.push(forward.clone());
                    forward
                };
                url.set_host(Some("127.0.0.1")).map_err(|e| e.to_string())?;
                url.set_port(Some(forward.local))
                    .map_err(|_| "Invalid forwarded port")?;
                let found = probe(Candidate {
                    uri: url.into(),
                    isolate: None,
                    device: format!("{} · {} · {}", device.label, serial, processes[&pid]),
                    name: processes[&pid].clone(),
                })
                .await;
                if !found.is_empty() {
                    used.insert(forward);
                    apps.extend(found);
                }
            }
        }
        self.android.retain(|key, _| seen_processes.contains(key));
        let obsolete: Vec<_> = self.owned.difference(&used).cloned().collect();
        for forward in obsolete {
            // Re-read before removal: never remove a port that another tool remapped.
            self.remove_forward(&forward).await;
        }
        Ok(apps)
    }

    async fn remove_forward(&mut self, forward: &Forward) {
        if let Ok(list) = self.adb(&["forward", "--list"]).await {
            if !parse_forwards(&list).contains(forward) {
                self.owned.remove(forward);
                return;
            }
            if self
                .adb(&[
                    "-s",
                    &forward.serial,
                    "forward",
                    "--remove",
                    &format!("tcp:{}", forward.local),
                ])
                .await
                .is_ok()
            {
                self.owned.remove(forward);
            }
        }
    }

    pub async fn shutdown(&mut self) {
        let forwards: Vec<_> = self.owned.iter().cloned().collect();
        for forward in forwards {
            self.remove_forward(&forward).await;
        }
    }
}

/// Discover bridges in the current user's native temporary directory.
pub async fn local_apps() -> Vec<DiscoveredApp> {
    let candidates = tokio::task::spawn_blocking(|| local_candidates(&std::env::temp_dir()))
        .await
        .unwrap_or_default();
    probe_all(candidates).await
}

/// Find IDE-owned VM sessions, including those whose device logs have rotated.
pub async fn tooling_apps() -> Vec<DiscoveredApp> {
    let candidates = tokio::task::spawn_blocking(tool_candidates)
        .await
        .unwrap_or_default();
    probe_all(candidates).await
}

fn tool_candidates() -> Vec<Candidate> {
    use sysinfo::{ProcessRefreshKind, RefreshKind, System, UpdateKind};
    let system = System::new_with_specifics(
        RefreshKind::nothing().with_processes(
            ProcessRefreshKind::nothing()
                .with_cmd(UpdateKind::OnlyIfNotSet)
                .without_tasks(),
        ),
    );
    system
        .processes()
        .values()
        .filter_map(|process| {
            let args: Vec<_> = process
                .cmd()
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            tooling_candidate(&args)
        })
        .take(LIMIT)
        .collect()
}

fn tooling_candidate(args: &[String]) -> Option<Candidate> {
    let executable = Path::new(args.first()?)
        .file_stem()?
        .to_str()?
        .to_ascii_lowercase();
    if !matches!(executable.as_str(), "dart" | "dartvm" | "dartaotruntime")
        || !args.iter().any(|arg| arg == "development-service")
    {
        return None;
    }
    let option = |key: &str| {
        args.iter().enumerate().find_map(|(index, arg)| {
            arg.strip_prefix(&format!("{key}="))
                .map(str::to_owned)
                .or_else(|| (arg == key).then(|| args.get(index + 1).cloned()).flatten())
        })
    };
    let uri = loopback_vm_uri(&option("--vm-service-uri")?)?;
    let label = option("--app-name").unwrap_or_else(|| "Dart / Flutter app".into());
    let name = label
        .split(" - ")
        .find_map(|part| part.strip_prefix("Package: "))
        .unwrap_or(&label);
    let device = label
        .split(" - ")
        .find_map(|part| part.strip_prefix("Device: "))
        .map(|name| format!("{name} · IDE debug session"))
        .unwrap_or_else(|| "Running debug session".into());
    Some(Candidate {
        uri: uri.into(),
        isolate: None,
        name: name.chars().take(180).collect(),
        device,
    })
}

fn local_candidates(root: &Path) -> Vec<Candidate> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let Ok(entries) = std::fs::read_dir(root) else {
        return vec![];
    };
    entries
        .filter_map(Result::ok)
        .take(10_000)
        .filter_map(|entry| {
            if !entry
                .file_name()
                .to_string_lossy()
                .starts_with("flutter-inspector-")
                || !entry.file_type().ok()?.is_dir()
            {
                return None;
            }
            let path = entry.path().join("discovery.json");
            let meta = std::fs::symlink_metadata(&path).ok()?;
            if !meta.is_file() || meta.len() > 16 * 1024 {
                return None;
            }
            let record: Advertisement = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
            if record.version != 1 || now.abs_diff(record.updated_at) > 15_000 {
                return None;
            }
            let uri = loopback_vm_uri(&record.uri)?;
            Some(Candidate {
                uri: uri.into(),
                isolate: record.isolate,
                device: "This computer".into(),
                name: record.name,
            })
        })
        .take(LIMIT)
        .collect()
}

async fn probe_all(candidates: Vec<Candidate>) -> Vec<DiscoveredApp> {
    stream::iter(candidates)
        .map(probe)
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .flatten()
        .collect()
}

async fn probe(candidate: Candidate) -> Vec<DiscoveredApp> {
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        let vm = VmClient::connect(&candidate.uri).await?;
        let info = vm.call("getVM", json!({})).await?;
        let isolates = info["isolates"].as_array().ok_or("No isolates")?;
        let mut apps = Vec::new();
        let mut bridge_error = None;
        for isolate in isolates.iter().take(LIMIT) {
            let Some(id) = isolate["id"].as_str() else {
                continue;
            };
            if candidate
                .isolate
                .as_ref()
                .is_some_and(|wanted| wanted != id)
            {
                continue;
            }
            let details = vm.call("getIsolate", json!({"isolateId":id})).await?;
            if !details["extensionRPCs"]
                .as_array()
                .is_some_and(|r| r.iter().any(|v| v == "ext.flutter_inspector.call"))
            {
                if crate::native::supported(&details) {
                    apps.push(DiscoveredApp {
                        uri: candidate.uri.clone(),
                        isolate: id.into(),
                        name: candidate.name.clone(),
                        device: candidate.device.clone(),
                        session_id: format!("native-{}-{}", info["startTime"], id),
                        busy: false,
                        bridge: BridgeStatus::Native,
                    });
                }
                continue;
            }
            // This method must not acquire a lease: discovery and DevTools can coexist.
            let info = match vm.bridge(id, "", "discover", json!({})).await {
                Ok(info) => info,
                Err(error) => {
                    bridge_error = Some(error);
                    continue;
                }
            };
            if info["protocol"] != 1 {
                bridge_error =
                    Some("Update the app's inspector bridge to protocol version 1.".into());
                continue;
            }
            let (Some(name), Some(session_id), Some(busy)) = (
                info["name"].as_str(),
                info["sessionId"].as_str(),
                info["busy"].as_bool(),
            ) else {
                bridge_error = Some("The bridge returned incomplete discovery data.".into());
                continue;
            };
            apps.push(DiscoveredApp {
                uri: candidate.uri.clone(),
                isolate: id.into(),
                name: name.chars().take(128).collect(),
                device: candidate.device.clone(),
                session_id: session_id.into(),
                busy,
                bridge: BridgeStatus::Ready,
            });
        }
        // Background isolates also expose dart:io. Prefer the application isolate
        // for automatic selection; explicit VM connections can select others.
        if apps.iter().any(|a| a.bridge == BridgeStatus::Ready) {
            apps.retain(|a| a.bridge == BridgeStatus::Ready);
        } else if let Some(main) = isolates.iter().find(|i| i["name"] == "main")
            && apps
                .iter()
                .any(|a| a.isolate == main["id"].as_str().unwrap_or_default())
        {
            apps.retain(|a| a.isolate == main["id"].as_str().unwrap_or_default());
        }
        if apps.is_empty() {
            let bridge = bridge_error
                .map(BridgeStatus::Unavailable)
                .unwrap_or(BridgeStatus::Missing);
            apps.push(unconfigured(&candidate, bridge));
        }
        Ok::<_, String>(apps)
    })
    .await;
    match result {
        Ok(Ok(apps)) => apps,
        Ok(Err(error)) => vec![unconfigured(&candidate, BridgeStatus::Unavailable(error))],
        Err(_) => vec![unconfigured(
            &candidate,
            BridgeStatus::Unavailable(
                "VM discovery timed out; check whether the debugger paused startup.".into(),
            ),
        )],
    }
}

fn unconfigured(candidate: &Candidate, bridge: BridgeStatus) -> DiscoveredApp {
    DiscoveredApp {
        uri: candidate.uri.clone(),
        isolate: candidate.isolate.clone().unwrap_or_default(),
        name: if candidate.name.is_empty() {
            "Flutter app".into()
        } else {
            candidate.name.clone()
        },
        device: candidate.device.clone(),
        session_id: String::new(),
        busy: false,
        bridge,
    }
}

fn loopback_vm_uri(text: &str) -> Option<url::Url> {
    let url = url::Url::parse(text).ok()?;
    if !matches!(url.scheme(), "http" | "ws")
        || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"))
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    Some(url)
}

pub(crate) fn parse_devices(text: &str) -> Vec<Device> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let serial = fields.next()?;
            let state = fields.next()?;
            if !["device", "offline", "unauthorized"].contains(&state) {
                return None;
            }
            let label = fields
                .find_map(|f| f.strip_prefix("model:"))
                .unwrap_or(serial)
                .replace('_', " ");
            Some(Device {
                serial: serial.into(),
                label,
                state: state.into(),
            })
        })
        .collect()
}
fn parse_processes(text: &str) -> HashMap<u32, String> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.parse().ok()?, fields.next()?.to_owned()))
        })
        .collect()
}
fn parse_vm_logs(text: &str, processes: &HashMap<u32, String>) -> HashMap<u32, String> {
    let mut found = HashMap::new();
    for line in text.lines() {
        let Some((prefix, rest)) = line.split_once("The Dart VM service is listening on ") else {
            continue;
        };
        let Some(pid) = prefix
            .split_once('(')
            .and_then(|(_, p)| p.split_once(')'))
            .and_then(|(p, _)| p.trim().parse::<u32>().ok())
        else {
            continue;
        };
        if !processes.contains_key(&pid) {
            continue;
        }
        let Some(uri) = rest.split_whitespace().next().and_then(loopback_vm_uri) else {
            continue;
        };
        found.insert(pid, uri.into());
    }
    found
}
fn parse_forwards(text: &str) -> Vec<Forward> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            Some(Forward {
                serial: f.next()?.into(),
                local: f.next()?.strip_prefix("tcp:")?.parse().ok()?,
                remote: f.next()?.strip_prefix("tcp:")?.parse().ok()?,
            })
        })
        .collect()
}

pub(crate) fn adb_path() -> PathBuf {
    if let Some(path) = std::env::var_os("INSPECTOR_ADB") {
        return path.into();
    }
    let binary = if cfg!(windows) { "adb.exe" } else { "adb" };
    let mut roots: Vec<PathBuf> = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect();
    if let Some(base) = directories::BaseDirs::new() {
        roots.extend([
            base.home_dir().join("Android/Sdk"),
            base.home_dir().join("Library/Android/sdk"),
            base.data_local_dir().join("Android/Sdk"),
        ]);
    }
    roots
        .into_iter()
        .map(|root| root.join("platform-tools").join(binary))
        .find(|path| path.is_file())
        .unwrap_or_else(|| binary.into())
}

pub(crate) async fn run(program: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("Missing process stdout")?;
    let stderr = child.stderr.take().ok_or("Missing process stderr")?;
    tokio::time::timeout(Duration::from_secs(4), async {
        let read = async |stream: std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>| {
            let mut bytes = Vec::new();
            stream
                .take(PROCESS_LIMIT + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|e| e.to_string())?;
            if bytes.len() as u64 > PROCESS_LIMIT {
                return Err("ADB output exceeds discovery limit".into());
            }
            Ok::<_, String>(bytes)
        };
        let (stdout, stderr) = tokio::try_join!(read(Box::pin(stdout)), read(Box::pin(stderr)))?;
        let status = child.wait().await.map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(String::from_utf8_lossy(&stderr).chars().take(250).collect());
        }
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    })
    .await
    .map_err(|_| "ADB command timed out".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ide_session_uri_is_available_without_startup_logs() {
        let args = [
            "/sdk/bin/dart",
            "development-service",
            "--vm-service-uri=http://127.0.0.1:123/token=/",
            "--app-name",
            "Fixture on Android",
        ]
        .map(str::to_owned);
        let found = tooling_candidate(&args).unwrap();
        assert_eq!(found.name, "Fixture on Android");
        assert_eq!(found.uri, "http://127.0.0.1:123/token=/");
        assert!(tooling_candidate(&["unrelated".into(), "development-service".into()]).is_none());
    }
    #[test]
    fn android_parser_ignores_dead_processes_and_keeps_latest_auth_path() {
        let processes = parse_processes("PID NAME\n42 com.example.app\n43 another");
        let logs = "I/flutter ( 41): The Dart VM service is listening on http://127.0.0.1:100/old=/\n\
I/flutter ( 42): The Dart VM service is listening on http://127.0.0.1:101/first=/\n\
I/flutter (42): The Dart VM service is listening on http://127.0.0.1:102/a-B_c=/\n\
I/flutter (43): The Dart VM service is listening on http://evil.example:103/token=/";
        let parsed = parse_vm_logs(logs, &processes);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[&42], "http://127.0.0.1:102/a-B_c=/");
        assert!(loopback_vm_uri("http://[::1]:123/auth=/").is_some());
    }
    #[test]
    fn device_and_forward_parsers_handle_multiple_devices() {
        let devices = parse_devices(
            "List of devices attached\nemulator-5554 device product:sdk model:Pixel_8\nUSB unauthorized\n",
        );
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].label, "Pixel 8");
        assert_eq!(devices[1].state, "unauthorized");
        assert_eq!(
            parse_forwards("USB tcp:8181 tcp:5555\nOTHER tcp:9000 localabstract:socket\n"),
            vec![Forward {
                serial: "USB".into(),
                local: 8181,
                remote: 5555
            }]
        );
    }
    #[test]
    fn advertisements_expire_and_reject_remote_hosts() {
        let root = tempfile::tempdir().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        for (name, time, uri) in [
            ("live", now, "http://127.0.0.1:123/auth=/"),
            ("stale", now - 16_000, "http://127.0.0.1:123/auth=/"),
            ("remote", now, "http://example.com:123/auth=/"),
        ] {
            let dir = root.path().join(format!("flutter-inspector-{name}"));
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(
                dir.join("discovery.json"),
                json!({"version":1,"updatedAt":time,"uri":uri,"isolate":"isolates/1"}).to_string(),
            )
            .unwrap();
        }
        let candidates = local_candidates(root.path());
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].uri.ends_with("/auth=/"));
    }

    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::result_large_err)] // Tungstenite's handshake callback fixes the error type.
    async fn android_discovery_verifies_bridge_caches_logs_and_preserves_foreign_forwards() {
        use futures_util::SinkExt;
        use serde_json::Value;
        use std::os::unix::fs::PermissionsExt;
        use tokio_tungstenite::tungstenite::Message;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let bridge_enabled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let advertised_bridge = bridge_enabled.clone();
        let server = tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let advertised_bridge = advertised_bridge.clone();
                tokio::spawn(async move {
                    let mut socket = tokio_tungstenite::accept_hdr_async(socket, |
                        request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                        response: tokio_tungstenite::tungstenite::handshake::server::Response
                    | {
                        assert_eq!(request.uri().path(), "/a-B_c=/ws");
                        Ok(response)
                    }).await.unwrap();
                    while let Some(Ok(Message::Text(text))) = socket.next().await {
                        let request: Value = serde_json::from_str(&text).unwrap();
                        let result = match request["method"].as_str().unwrap() {
                            "getVM" => json!({"isolates":[{"id":"isolates/7","name":"main"}]}),
                            "getIsolate" => {
                                if advertised_bridge.load(std::sync::atomic::Ordering::SeqCst) {
                                    json!({"extensionRPCs":["ext.flutter_inspector.call"]})
                                } else {
                                    json!({"extensionRPCs":["ext.dart.io.getHttpProfile"]})
                                }
                            }
                            "ext.flutter_inspector.call" => {
                                let payload: Value = serde_json::from_str(
                                    request["params"]["payload"].as_str().unwrap(),
                                )
                                .unwrap();
                                assert_eq!(payload["method"], "discover");
                                assert_eq!(payload["controller"], "");
                                json!({"result":{"protocol":1,"name":"USB fixture","sessionId":"session-1","busy":false}})
                            }
                            unexpected => panic!("Unexpected RPC: {unexpected}"),
                        };
                        socket
                            .send(Message::Text(
                                json!({"jsonrpc":"2.0","id":request["id"],"result":result})
                                    .to_string()
                                    .into(),
                            ))
                            .await
                            .unwrap();
                    }
                });
            }
        });
        let directory = tempfile::tempdir().unwrap();
        let state = directory.path().join("state.json");
        let original = json!({"logs":true,"running":true,"forward":null,"removed":0});
        std::fs::write(&state, original.to_string()).unwrap();
        let adb = directory.path().join("adb");
        let script = r#"#!/usr/bin/env python3
import json, pathlib, sys
path = pathlib.Path(__STATE__)
s = json.loads(path.read_text())
changed = False
a = sys.argv[1:]
if a == ['devices', '-l']:
    print('List of devices attached\nUSB device model:Test_Phone')
elif a == ['forward', '--list']:
    print('OTHER tcp:8765 tcp:9876')
    if s['forward'] is not None:
        print('USB tcp:__PORT__ tcp:' + str(s['forward']))
elif a[2:4] == ['shell', 'ps']:
    print('PID NAME\n42 com.example.fixture' if s['running'] else 'PID NAME')
elif a[2] == 'logcat':
    if s['logs']:
        print('I/flutter ( 42): The Dart VM service is listening on http://127.0.0.1:34567/a-B_c=/')
elif a[2:4] == ['forward', 'tcp:0']:
    s['forward'] = 34567
    changed = True
    print('__PORT__')
elif a[2:4] == ['forward', '--remove']:
    assert s['forward'] == 34567, 'Do not remove a remapped forward'
    s['forward'] = None
    s['removed'] += 1
    changed = True
else:
    raise RuntimeError(a)
if changed:
    path.write_text(json.dumps(s))
"#
        .replace(
            "__STATE__",
            &serde_json::to_string(&state.to_string_lossy()).unwrap(),
        )
        .replace("__PORT__", &port.to_string());
        std::fs::write(&adb, script).unwrap();
        std::fs::set_permissions(&adb, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut discovery = Discovery {
            adb,
            owned: HashSet::new(),
            android: HashMap::new(),
        };
        let mut notes = vec![];
        let apps = discovery.scan_android(&mut notes).await.unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].uri, format!("http://127.0.0.1:{port}/a-B_c=/"));
        assert_eq!(apps[0].name, "USB fixture");
        assert_eq!(discovery.owned.len(), 1);
        let mut value: Value = serde_json::from_slice(&std::fs::read(&state).unwrap()).unwrap();
        value["logs"] = json!(false);
        std::fs::write(&state, value.to_string()).unwrap();
        assert_eq!(discovery.scan_android(&mut notes).await.unwrap().len(), 1);
        bridge_enabled.store(false, std::sync::atomic::Ordering::SeqCst);
        let missing = discovery.scan_android(&mut notes).await.unwrap();
        assert_eq!(
            missing.len(),
            1,
            "A running Flutter app stays visible before bridge setup"
        );
        assert_eq!(missing[0].bridge, BridgeStatus::Native);
        assert_eq!(missing[0].name, "com.example.fixture");
        bridge_enabled.store(true, std::sync::atomic::Ordering::SeqCst);
        discovery.shutdown().await;
        let value: Value = serde_json::from_slice(&std::fs::read(&state).unwrap()).unwrap();
        assert_eq!(value["removed"], 1);
        assert!(discovery.owned.is_empty());

        // A forward supplied by Flutter is borrowed, never removed.
        let mut value = original.clone();
        value["forward"] = json!(34567);
        std::fs::write(&state, value.to_string()).unwrap();
        assert_eq!(discovery.scan_android(&mut notes).await.unwrap().len(), 1);
        assert!(discovery.owned.is_empty());
        discovery.shutdown().await;
        let value: Value = serde_json::from_slice(&std::fs::read(&state).unwrap()).unwrap();
        assert_eq!(value["removed"], 0);

        // Another tool remapping our local port transfers ownership.
        std::fs::write(&state, original.to_string()).unwrap();
        assert_eq!(discovery.scan_android(&mut notes).await.unwrap().len(), 1);
        let mut value: Value = serde_json::from_slice(&std::fs::read(&state).unwrap()).unwrap();
        value["forward"] = json!(45678);
        std::fs::write(&state, value.to_string()).unwrap();
        discovery.shutdown().await;
        let value: Value = serde_json::from_slice(&std::fs::read(&state).unwrap()).unwrap();
        assert_eq!(value["forward"], 45678);
        assert_eq!(value["removed"], 0);
        server.abort();
    }
}
