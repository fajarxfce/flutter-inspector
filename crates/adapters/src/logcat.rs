use crate::{discovery, process_io::LineReader};
use inspector_application::*;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::BufReader, process::Command, sync::mpsc};

pub struct Logcat {
    adb: PathBuf,
    task: Option<tokio::task::JoinHandle<()>>,
    control: Option<mpsc::Sender<u64>>,
}
impl Default for Logcat {
    fn default() -> Self {
        Self {
            adb: discovery::adb_path(),
            task: None,
            control: None,
        }
    }
}
impl Logcat {
    pub async fn stop(&mut self) {
        self.control = None;
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
    pub async fn handle(&mut self, command: LogcatCommand, sink: EventSink) {
        match command {
            LogcatCommand::Stop => self.stop().await,
            LogcatCommand::Clear { generation } => {
                if let Some(control) = &self.control {
                    let _ = control.send(generation).await;
                }
            }
            LogcatCommand::Start { serial, generation } => {
                self.stop().await;
                let (sender, receiver) = mpsc::channel(8);
                self.control = Some(sender);
                self.task = Some(tokio::spawn(stream(
                    self.adb.clone(),
                    serial,
                    generation,
                    receiver,
                    sink,
                )));
            }
        }
    }
}
impl Drop for Logcat {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

pub async fn devices(request_id: u64, uri: Option<String>) -> LogcatEvent {
    let adb = discovery::adb_path();
    match discovery::run(&adb, &["devices", "-l"]).await {
        Ok(text) => {
            let devices = discovery::parse_devices(&text)
                .into_iter()
                .map(|d| AndroidDevice {
                    serial: d.serial,
                    label: d.label,
                    state: d.state,
                })
                .collect();
            let suggested = if let Some(uri) = uri {
                let forwards = discovery::run(&adb, &["forward", "--list"])
                    .await
                    .unwrap_or_default();
                device_for_uri(&uri, &forwards)
            } else {
                None
            };
            LogcatEvent::Devices {
                request_id,
                devices,
                suggested,
                error: None,
            }
        }
        Err(error) => LogcatEvent::Devices {
            request_id,
            devices: vec![],
            suggested: None,
            error: Some(format!("ADB unavailable: {error}")),
        },
    }
}
fn device_for_uri(uri: &str, forwards: &str) -> Option<String> {
    let uri = url::Url::parse(uri).ok()?;
    if !matches!(uri.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) {
        return None;
    }
    let local = format!("tcp:{}", uri.port()?);
    forwards.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let serial = fields.next()?;
        (fields.next()? == local).then(|| serial.to_owned())
    })
}

pub fn parse_line(text: &str) -> Option<LogLine> {
    let mut rest = text.trim_start();
    fn field<'a>(rest: &mut &'a str) -> Option<&'a str> {
        let end = rest.find(char::is_whitespace)?;
        let value = &rest[..end];
        *rest = rest[end..].trim_start();
        Some(value)
    }
    let date = field(&mut rest)?;
    if date.len() != 5 || date.as_bytes()[2] != b'-' {
        return None;
    }
    let time = field(&mut rest)?;
    let pid = field(&mut rest)?.parse().ok()?;
    let tid = field(&mut rest)?.parse().ok()?;
    let level = match field(&mut rest)? {
        "V" => LogLevel::Verbose,
        "D" => LogLevel::Debug,
        "I" => LogLevel::Info,
        "W" => LogLevel::Warn,
        "E" => LogLevel::Error,
        "F" | "A" => LogLevel::Fatal,
        _ => return None,
    };
    let (tag, message) = rest.split_once(':')?;
    Some(LogLine {
        time: format!("{date} {time}"),
        pid,
        tid,
        level,
        tag: tag.trim().to_owned(),
        message: message.strip_prefix(' ').unwrap_or(message).to_owned(),
    })
}

async fn stream(
    adb: PathBuf,
    serial: String,
    mut generation: u64,
    mut control: mpsc::Receiver<u64>,
    sink: EventSink,
) {
    let mut command = Command::new(adb);
    command
        .args([
            "-s",
            &serial,
            "logcat",
            "-b",
            "all",
            "-v",
            "threadtime",
            "-T",
            "200",
            "*:V",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let result = async {
        let mut child = command.spawn().map_err(|e| format!("Cannot start ADB: {e}"))?;
        let mut stdout = LineReader::new(BufReader::new(child.stdout.take().ok_or("Missing ADB stdout")?), 64 * 1024);
        let mut stderr = LineReader::new(BufReader::new(child.stderr.take().ok_or("Missing ADB stderr")?), 4096);
        let mut stderr_open = true;
        let mut last_error = String::new();
        let mut batch = Vec::new();
        let mut bytes = 0;
        let mut dropped = 0;
        let mut timer = tokio::time::interval(Duration::from_millis(100));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        sink.send(Event::Logcat(LogcatEvent::Status { generation, running: true, message: "Streaming".into() })).await;
        loop {
            tokio::select! {
                next = control.recv() => {
                    let Some(next) = next else { return Ok(()); };
                    generation = next; batch.clear(); bytes = 0; dropped = 0;
                }
                line = stdout.next() => {
                    let Some(line) = line.map_err(|e| e.to_string())? else { break; };
                    if batch.len() >= 200 || bytes >= 512 * 1024 { dropped += 1; continue; }
                    if let Some(line) = parse_line(&line) { bytes += line.message.len(); batch.push(line); }
                }
                line = stderr.next(), if stderr_open => {
                    match line { Ok(Some(line)) => last_error = line, _ => stderr_open = false }
                }
                _ = timer.tick() => {
                    flush(&sink, generation, &mut batch, &mut dropped); bytes = 0;
                }
            }
        }
        flush(&sink, generation, &mut batch, &mut dropped);
        let status = child.wait().await.map_err(|e| e.to_string())?;
        if status.success() { Ok(()) } else { Err(format!("ADB stopped ({status}): {last_error}")) }
    }.await;
    sink.send(Event::Logcat(LogcatEvent::Status {
        generation,
        running: false,
        message: result
            .err()
            .unwrap_or_else(|| "Device stream ended. Use Retry to reconnect.".into()),
    }))
    .await;
}
fn flush(sink: &EventSink, generation: u64, batch: &mut Vec<LogLine>, dropped: &mut u64) {
    if batch.is_empty() && *dropped == 0 {
        return;
    }
    let count = batch.len() as u64;
    let lines = std::mem::take(batch);
    if sink.try_send(Event::Logcat(LogcatEvent::Lines {
        generation,
        lines,
        dropped: *dropped,
    })) {
        *dropped = 0;
    } else {
        *dropped += count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_threadtime_and_rejects_headers() {
        let line = parse_line("10-02 12:05:01.002  127  130 E flutter : error: line two").unwrap();
        assert_eq!(
            (line.pid, line.tid, line.level),
            (127, 130, LogLevel::Error)
        );
        assert_eq!(line.tag, "flutter");
        assert_eq!(line.message, "error: line two");
        assert!(parse_line("--------- beginning of main").is_none());
        assert!(parse_line("x").is_none());
    }
    #[test]
    fn matches_only_the_forwarded_local_vm_port() {
        let forwards = "phone1 tcp:123 tcp:222\nphone2 tcp:456 tcp:333\n";
        assert_eq!(
            device_for_uri("ws://127.0.0.1:456/auth=/ws", forwards).as_deref(),
            Some("phone2")
        );
        assert!(device_for_uri("http://remote:456/auth=/", forwards).is_none());
        assert!(device_for_uri("http://localhost:999/", forwards).is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn streaming_clear_keeps_one_process_and_shutdown_terminates_it() {
        use std::os::unix::fs::PermissionsExt;
        struct Backend {
            logcat: tokio::sync::Mutex<Logcat>,
            sink: EventSink,
        }
        #[async_trait::async_trait]
        impl inspector_application::Backend for Backend {
            async fn handle(&self, command: inspector_application::Command) -> Result<(), String> {
                match command {
                    inspector_application::Command::Logcat(command) => {
                        self.logcat
                            .lock()
                            .await
                            .handle(command, self.sink.clone())
                            .await
                    }
                    inspector_application::Command::Shutdown => {
                        self.logcat.lock().await.stop().await
                    }
                    _ => {}
                }
                Ok(())
            }
            async fn tick(&self) -> Result<(), String> {
                Ok(())
            }
        }
        async fn lines(worker: &mut Worker, generation: u64) -> Vec<LogLine> {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(Event::Logcat(LogcatEvent::Lines {
                        generation: id,
                        lines,
                        ..
                    })) = worker.events.recv().await
                        && id == generation
                        && !lines.is_empty()
                    {
                        return lines;
                    }
                }
            })
            .await
            .unwrap()
        }
        let dir = tempfile::tempdir().unwrap();
        let adb = dir.path().join("adb");
        std::fs::write(
            &adb,
            r#"#!/usr/bin/env python3
import os, time
from pathlib import Path
root = Path(__file__).parent
(root / 'pid').write_text(str(os.getpid()))
while True:
    text = (root / 'phase').read_text()
    if text: print('10-02 01:02:03.000 42 42 I test: ' + text, flush=True)
    time.sleep(0.03)
"#,
        )
        .unwrap();
        std::fs::set_permissions(&adb, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.path().join("phase"), "before").unwrap();
        let mut worker = Worker::start(
            || {},
            move |sink| {
                Ok(Backend {
                    logcat: tokio::sync::Mutex::new(Logcat {
                        adb,
                        task: None,
                        control: None,
                    }),
                    sink,
                })
            },
        );
        worker
            .commands
            .send(inspector_application::Command::Logcat(
                LogcatCommand::Start {
                    serial: "fake".into(),
                    generation: 1,
                },
            ))
            .await
            .unwrap();
        assert!(
            lines(&mut worker, 1)
                .await
                .iter()
                .all(|l| l.message == "before")
        );
        let pid = std::fs::read_to_string(dir.path().join("pid")).unwrap();
        std::fs::write(dir.path().join("phase"), "").unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        worker
            .commands
            .send(inspector_application::Command::Logcat(
                LogcatCommand::Clear { generation: 2 },
            ))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        std::fs::write(dir.path().join("phase"), "after").unwrap();
        assert!(
            lines(&mut worker, 2)
                .await
                .iter()
                .all(|l| l.message == "after")
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("pid")).unwrap(),
            pid,
            "Clear restarted ADB"
        );
        drop(worker);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let pid: i32 = pid.parse().unwrap();
        assert_ne!(
            unsafe { libc::kill(pid, 0) },
            0,
            "ADB stream survived shutdown"
        );
    }
}
