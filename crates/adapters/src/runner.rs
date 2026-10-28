//! One owned Flutter machine session; no shell and no editor process required.
use crate::{process_io::LineReader, project};
use inspector_application::*;
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    process::{ChildStdin, Command},
    sync::mpsc,
};

#[derive(Default)]
pub struct Runner {
    task: Option<tokio::task::JoinHandle<()>>,
    control: Option<mpsc::Sender<RunnerCommand>>,
}
impl Runner {
    pub async fn handle(&mut self, command: RunnerCommand, sink: EventSink) -> Result<(), String> {
        match command {
            RunnerCommand::Start {
                generation,
                request,
            } => {
                if self.task.as_ref().is_some_and(|t| !t.is_finished()) {
                    return Err("Stop the current run before starting another project".into());
                }
                let (sender, receiver) = mpsc::channel(8);
                self.control = Some(sender);
                self.task = Some(tokio::spawn(run(generation, *request, receiver, sink)));
            }
            command => self
                .control
                .as_ref()
                .ok_or("No active Flutter run")?
                .send(command)
                .await
                .map_err(|_| "Flutter run has ended")?,
        }
        Ok(())
    }
    pub async fn shutdown(&mut self) {
        if let Some(control) = self.control.take() {
            let _ = control.send(RunnerCommand::Stop).await;
        }
        if let Some(mut task) = self.task.take()
            && tokio::time::timeout(Duration::from_secs(2), &mut task)
                .await
                .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}
impl Drop for Runner {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

pub fn arguments(request: &LaunchRequest) -> Result<Vec<String>, String> {
    let profile = &request.profile;
    if let Some(issue) = &profile.issue {
        return Err(issue.clone());
    }
    project::validate_tool_args(&profile.tool_args)?;
    if !profile.cwd.join("pubspec.yaml").is_file() {
        return Err("Launch cwd must contain pubspec.yaml".into());
    }
    if !profile.cwd.join(&profile.program).is_file() {
        return Err(format!("Entrypoint does not exist: {}", profile.program));
    }
    if request.device.trim().is_empty() {
        return Err("Select a Flutter device first".into());
    }
    let mut args = vec![
        "run".into(),
        "--machine".into(),
        "--debug".into(),
        "-d".into(),
        request.device.clone(),
        "-t".into(),
        profile.program.clone(),
    ];
    args.extend(profile.tool_args.clone());
    for arg in &profile.args {
        args.push(format!("--dart-entrypoint-args={arg}"));
    }
    Ok(args)
}
async fn rpc(stdin: &mut ChildStdin, id: u64, method: &str, params: Value) -> Result<(), String> {
    let text = serde_json::to_string(&json!([{ "id": id, "method": method, "params": params }]))
        .map_err(|e| e.to_string())?
        + "\n";
    stdin
        .write_all(text.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    stdin.flush().await.map_err(|e| e.to_string())
}
struct Output {
    lines: Vec<LogLine>,
    bytes: usize,
    dropped: u64,
}
impl Output {
    fn push(&mut self, message: String, error: bool) {
        for message in message.lines() {
            if self.lines.len() >= 200 || self.bytes >= 512 * 1024 {
                self.dropped += 1;
                continue;
            }
            self.bytes += message.len();
            self.lines.push(LogLine {
                time: chrono::Local::now().format("%H:%M:%S%.3f").to_string(),
                pid: 0,
                tid: 0,
                level: if error {
                    LogLevel::Error
                } else {
                    LogLevel::Info
                },
                tag: "Flutter".into(),
                message: message.into(),
            });
        }
    }
    fn flush(&mut self, generation: u64, sink: &EventSink) {
        if self.lines.is_empty() && self.dropped == 0 {
            return;
        }
        let count = self.lines.len() as u64;
        if sink.try_send(Event::Runner(RunnerEvent::Output {
            generation,
            lines: std::mem::take(&mut self.lines),
            dropped: self.dropped,
        })) {
            self.dropped = 0;
        } else {
            self.dropped += count;
        }
        self.bytes = 0;
    }
}
async fn state(
    sink: &EventSink,
    generation: u64,
    active: bool,
    ready: bool,
    message: impl Into<String>,
) {
    sink.send(Event::Runner(RunnerEvent::State {
        generation,
        active,
        ready,
        message: message.into(),
    }))
    .await;
}
async fn run(
    generation: u64,
    request: LaunchRequest,
    mut control: mpsc::Receiver<RunnerCommand>,
    sink: EventSink,
) {
    let result = async {
        let args = arguments(&request)?;
        let mut command = Command::new(project::flutter_binary(&request.sdk));
        command.args(args).current_dir(&request.profile.cwd).envs(&request.profile.env)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command.spawn().map_err(|e| format!("Cannot start Flutter: {e}"))?;
        let _tree = crate::process_tree::ProcessTree::new(child.id().ok_or("Missing Flutter process ID")?)?;
        let mut stdin = child.stdin.take().ok_or("Missing Flutter stdin")?;
        let mut stdout = LineReader::new(BufReader::new(child.stdout.take().ok_or("Missing Flutter stdout")?), 256 * 1024);
        let mut stderr = LineReader::new(BufReader::new(child.stderr.take().ok_or("Missing Flutter stderr")?), 64 * 1024);
        let mut stderr_open = true;
        let mut stdout_open = true;
        let mut app_id = String::new();
        let mut uri = String::new();
        let mut ready = false;
        let mut rpc_id = 0;
        let mut operation = None;
        let mut stop_at = None;
        let mut exited_at = None;
        let mut output = Output { lines: vec![], bytes: 0, dropped: 0 };
        let mut timer = tokio::time::interval(Duration::from_millis(100));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        state(&sink, generation, true, false, "Building and launching…").await;
        loop {
            tokio::select! {
                command = control.recv(), if stop_at.is_none() => {
                    let full = matches!(command, Some(RunnerCommand::Restart));
                    match command.unwrap_or(RunnerCommand::Stop) {
                        RunnerCommand::Reload | RunnerCommand::Restart if ready && operation.is_none() => {
                            rpc_id += 1; operation = Some((rpc_id, tokio::time::Instant::now()));
                            rpc(&mut stdin, rpc_id, "app.restart", json!({"appId": app_id, "fullRestart":full, "pause":false})).await?;
                            state(&sink, generation, true, false, if full { "Hot restarting…" } else { "Hot reloading…" }).await;
                        }
                        RunnerCommand::Stop => {
                            stop_at = Some(tokio::time::Instant::now());
                            state(&sink, generation, true, false, "Stopping…").await;
                            if !app_id.is_empty() {
                                rpc_id += 1; let _ = rpc(&mut stdin, rpc_id, "app.stop", json!({"appId":app_id})).await;
                            } else {
                                #[cfg(unix)]
                                if let Some(pid) = child.id() { unsafe { libc::kill(-(pid as i32), libc::SIGTERM); } }
                                #[cfg(windows)]
                                kill_tree(&mut child).await;
                            }
                        }
                        _ => {}
                    }
                }
                line = stdout.next(), if stdout_open => {
                    let Some(line) = line.map_err(|e| e.to_string())? else { stdout_open = false; continue; };
                    if let Ok(Value::Array(messages)) = serde_json::from_str::<Value>(&line) {
                        for message in messages {
                            let params = &message["params"];
                            match message["event"].as_str() {
                                Some("app.start") => app_id = params["appId"].as_str().unwrap_or_default().into(),
                                Some("app.debugPort") => {
                                    uri = params["wsUri"].as_str().unwrap_or_default().into();
                                    if ready && !uri.is_empty() { sink.send(Event::Runner(RunnerEvent::VmService { generation, uri: uri.clone(), device: request.device.clone() })).await; }
                                }
                                Some("app.started") => {
                                    ready = true;
                                    state(&sink, generation, true, true, "Running in debug mode").await;
                                    if !uri.is_empty() { sink.send(Event::Runner(RunnerEvent::VmService { generation, uri: uri.clone(), device: request.device.clone() })).await; }
                                }
                                Some("app.log" | "daemon.logMessage") => output.push(params["log"].as_str().or_else(|| params["message"].as_str()).unwrap_or_default().into(), params["error"].as_bool().unwrap_or(false) || params["level"] == "error"),
                                Some("app.progress") if params["finished"] != true => {
                                    if let Some(text) = params["message"].as_str() { output.push(text.into(), false); }
                                }
                                Some("app.stop") => { ready = false; state(&sink, generation, true, false, "App stopped; closing Flutter…").await; }
                                None if operation.is_some_and(|(id, _)| Some(id) == message["id"].as_u64()) => {
                                        operation = None;
                                        let error = message.get("error").filter(|e| !e.is_null()).map(Value::to_string)
                                            .or_else(|| (message["result"]["code"].as_i64().unwrap_or(0) != 0).then(|| message["result"]["message"].as_str().unwrap_or("Reload failed").to_owned()));
                                        if let Some(error) = error { output.push(error, true); }
                                        state(&sink, generation, true, ready, "Running in debug mode").await;
                                }
                                _ => {}
                            }
                        }
                    } else if !line.trim().is_empty() { output.push(line, false); }
                }
                line = stderr.next(), if stderr_open => {
                    match line { Ok(Some(line)) => output.push(line, true), Ok(None) => stderr_open = false, Err(e) => return Err(e.to_string()) }
                }
                _ = timer.tick() => {
                    output.flush(generation, &sink);
                    if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                        let at = exited_at.get_or_insert_with(tokio::time::Instant::now);
                        // Descendants may still hold the pipe open after the CLI exits.
                        if (!stdout_open && !stderr_open) || at.elapsed() >= Duration::from_millis(200) {
                            if status.success() || stop_at.is_some() { return Ok(()); }
                            return Err(format!("Flutter exited with {status}. See run output."));
                        }
                    }
                    if operation.is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(60)) {
                        operation = None; output.push("Reload timed out; check run output or stop the session.".into(), true);
                        state(&sink, generation, true, ready, "Reload timed out").await;
                    }
                    if stop_at.is_some_and(|at| at.elapsed() > Duration::from_secs(8)) {
                        #[cfg(windows)]
                        kill_tree(&mut child).await;
                        let _ = child.kill().await;
                        return Ok(());
                    }
                }
            }
        }
    }.await;
    state(
        &sink,
        generation,
        false,
        false,
        result.err().unwrap_or_else(|| "Stopped".into()),
    )
    .await;
}
#[cfg(windows)]
async fn kill_tree(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
}
