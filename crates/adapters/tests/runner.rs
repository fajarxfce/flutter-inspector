#![cfg(unix)]
use inspector_adapters::backend::Services;
use inspector_application::*;
use std::{collections::BTreeMap, os::unix::fs::PermissionsExt, time::Duration};

async fn event(worker: &mut Worker, predicate: impl Fn(&RunnerEvent) -> bool) -> RunnerEvent {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match worker.events.recv().await.unwrap() {
                Event::Runner(event) if predicate(&event) => return event,
                Event::Error(error) => panic!("{error}"),
                _ => {}
            }
        }
    })
    .await
    .unwrap()
}
fn fixture(root: &std::path::Path) -> LaunchRequest {
    std::fs::write(root.join("pubspec.yaml"), "name: fake").unwrap();
    std::fs::write(root.join("main.dart"), "void main() {}").unwrap();
    let executable = root.join("fake flutter");
    std::fs::write(
        &executable,
        r##"#!/usr/bin/env python3
import json, os, subprocess, sys
from pathlib import Path
Path('argv.json').write_text(json.dumps(sys.argv[1:]))
child = subprocess.Popen(['sleep', '120'])
Path('child.pid').write_text(str(child.pid))
def send(obj): print(json.dumps([obj]), flush=True)
send({'event':'app.start','params':{'appId':'owned-app'}})
send({'event':'app.started','params':{}})
for line in sys.stdin:
    for request in json.loads(line):
        with open('commands.jsonl', 'a') as f: f.write(json.dumps(request) + '\n')
        if request['method'] == 'app.stop': sys.exit(0)
        assert request['params']['appId'] == 'owned-app'
        send({'id':request['id'],'result':{'code':0}})
"##,
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    LaunchRequest {
        sdk: executable,
        device: "linux".into(),
        profile: LaunchProfile {
            name: "Test".into(),
            source: "Test".into(),
            cwd: root.into(),
            program: "main.dart".into(),
            tool_args: vec!["--dart-define=LABEL=a b $(not a shell)".into()],
            args: vec![],
            env: BTreeMap::new(),
            device: "linux".into(),
            issue: None,
        },
    }
}
#[cfg(target_os = "linux")]
fn alive(pid: &str) -> bool {
    // A reaped child is gone; a zombie no longer executes either.
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .is_some_and(|s| !s.split(')').nth(1).unwrap_or_default().starts_with(" Z"))
}
#[tokio::test]
async fn machine_run_reload_restart_stop_preserves_args_and_owns_children() {
    let directory = tempfile::tempdir().unwrap();
    let request = fixture(directory.path());
    let store = directory.path().join("store");
    let mut worker = Worker::start(|| {}, move |sink| Services::new(sink, Some(store)));
    worker
        .commands
        .send(Command::Runner(RunnerCommand::Start {
            generation: 7,
            request: Box::new(request),
        }))
        .await
        .unwrap();
    event(&mut worker, |e| {
        matches!(e, RunnerEvent::State { ready: true, .. })
    })
    .await;
    for command in [RunnerCommand::Reload, RunnerCommand::Restart] {
        worker
            .commands
            .send(Command::Runner(command))
            .await
            .unwrap();
        event(&mut worker, |e| {
            matches!(
                e,
                RunnerEvent::State {
                    ready: false,
                    active: true,
                    ..
                }
            )
        })
        .await;
        event(&mut worker, |e| {
            matches!(e, RunnerEvent::State { ready: true, .. })
        })
        .await;
    }
    let args: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string(directory.path().join("argv.json")).unwrap())
            .unwrap();
    assert!(
        args.iter()
            .any(|s| s == "--dart-define=LABEL=a b $(not a shell)")
    );
    let commands = std::fs::read_to_string(directory.path().join("commands.jsonl")).unwrap();
    assert!(commands.contains("\"fullRestart\": false"));
    assert!(commands.contains("\"fullRestart\": true"));
    #[cfg(target_os = "linux")]
    let pid = std::fs::read_to_string(directory.path().join("child.pid")).unwrap();
    worker
        .commands
        .send(Command::Runner(RunnerCommand::Stop))
        .await
        .unwrap();
    event(&mut worker, |e| {
        matches!(e, RunnerEvent::State { active: false, .. })
    })
    .await;
    #[cfg(target_os = "linux")]
    {
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!alive(&pid), "Owned child survived Stop");
    }
}

#[tokio::test]
async fn closing_worker_stops_a_running_project() {
    let directory = tempfile::tempdir().unwrap();
    let request = fixture(directory.path());
    let store = directory.path().join("store");
    let mut worker = Worker::start(|| {}, move |sink| Services::new(sink, Some(store)));
    worker
        .commands
        .send(Command::Runner(RunnerCommand::Start {
            generation: 1,
            request: Box::new(request),
        }))
        .await
        .unwrap();
    event(&mut worker, |e| {
        matches!(e, RunnerEvent::State { ready: true, .. })
    })
    .await;
    #[cfg(target_os = "linux")]
    let pid = std::fs::read_to_string(directory.path().join("child.pid")).unwrap();
    drop(worker);
    #[cfg(target_os = "linux")]
    assert!(!alive(&pid), "Owned child survived Inspector shutdown");
}
