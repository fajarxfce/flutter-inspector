//! Run under xvfb-run. flutter run supplies the expression compiler used by
//! DevTools; launching a prebuilt Flutter executable alone does not.
use base64::{Engine, engine::general_purpose::STANDARD};
use inspector_adapters::{
    native::{NativeSession, library},
    vm::VmClient,
};
use inspector_domain::{EventPage, Exchange};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = Command::new("kill")
            .args(["-TERM", "--", &format!("-{}", self.0.id())])
            .status();
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
async fn wait_uri(path: &std::path::Path) -> String {
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            if let Some(word) = text
                .split_whitespace()
                .find(|s| s.starts_with("http://127.0.0.1:"))
            {
                break word.to_owned();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("Fixture VM started")
}
async fn body(session: &NativeSession, body: &inspector_domain::Body) -> Vec<u8> {
    let mut bytes = Vec::new();
    while bytes.len() < body.size as usize {
        let value = session
            .call("body.read", json!({"id":body.id,"offset":bytes.len()}))
            .await
            .unwrap();
        let chunk = STANDARD.decode(value["data"].as_str().unwrap()).unwrap();
        assert!(!chunk.is_empty());
        bytes.extend(chunk);
    }
    bytes
}
#[tokio::test]
#[ignore = "requires Linux Flutter fixture built without bridge and a display"]
async fn built_in_network_preferences_and_drift_without_registration() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join("fixture.log");
    let mut command = Command::new("flutter");
    command
        .current_dir(root.join("examples/flutter_fixture"))
        .args([
            "run",
            "--no-pub",
            "-d",
            "linux",
            "--debug",
            "--dart-define=INSPECTOR_BRIDGE=false",
        ]);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let _fixture = Process(
        command
            .env_remove("WAYLAND_DISPLAY")
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .stdout(std::fs::File::create(&log).unwrap())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let uri = wait_uri(&log).await;
    let vm = VmClient::connect(&uri).await.unwrap();
    let info = vm.call("getVM", json!({})).await.unwrap();
    let isolate = info["isolates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "main")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let details = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let details = vm
                .call("getIsolate", json!({"isolateId":isolate}))
                .await
                .unwrap();
            if inspector_adapters::native::extension(&details, "ext.drift.database") {
                break details;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        !inspector_adapters::native::extension(&details, "ext.flutter_inspector.call"),
        "Rebuild fixture with INSPECTOR_BRIDGE=false"
    );
    vm.call("streamListen", json!({"streamId":"Extension"}))
        .await
        .unwrap();
    let (session, hello) = NativeSession::open(vm.clone(), isolate, &details)
        .await
        .unwrap();
    assert!(hello.providers.iter().any(|p| p.kind == "http"));
    assert!(!hello.providers.iter().any(|p| p.kind == "dio"));
    let discovered = inspector_adapters::discovery::tooling_apps().await;
    assert!(
        discovered.iter().any(|app| app.isolate == isolate
            && app.bridge == inspector_application::BridgeStatus::Native),
        "IDE/DDS discovery must find the app without SDK advertisements"
    );
    let drift = hello
        .providers
        .iter()
        .find(|p| p.kind == "drift")
        .expect("automatic Drift discovery")
        .id
        .clone();
    let root_library = details["rootLib"]["id"].as_str().unwrap();

    // Arbitrary preference text is data, never interpolated Dart code.
    for provider in ["native.preferences.legacy", "native.preferences.async"] {
        let text = "quotes ' \" \\ \n ${throw StateError('injected')}";
        session
            .call(
                "preferences.set",
                json!({"provider":provider,"key":"inspector.test","type":"String","value":text}),
            )
            .await
            .unwrap();
        let rows = session
            .call("preferences.list", json!({"provider":provider}))
            .await
            .unwrap();
        assert_eq!(
            rows.as_array()
                .unwrap()
                .iter()
                .find(|r| r["key"] == "inspector.test")
                .unwrap()["value"],
            text
        );
        session
            .call(
                "preferences.remove",
                json!({"provider":provider,"key":"inspector.test"}),
            )
            .await
            .unwrap();
        let rows = session
            .call("preferences.list", json!({"provider":provider}))
            .await
            .unwrap();
        assert!(
            !rows
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["key"] == "inspector.test")
        );
    }
    let prefs_lib = library(
        &details,
        "package:shared_preferences/src/shared_preferences_devtools_extension_data.dart",
    )
    .unwrap();
    assert!(
        vm.evaluate_async(isolate, &prefs_lib, "throw StateError('async error test');")
            .await
            .unwrap_err()
            .contains("async error test")
    );
    let (left, right) = tokio::join!(
        session.call(
            "preferences.list",
            json!({"provider":"native.preferences.legacy"})
        ),
        session.call(
            "preferences.list",
            json!({"provider":"native.preferences.async"})
        )
    );
    left.unwrap();
    right.unwrap();

    let tables = session
        .call("drift.tables", json!({"provider":drift}))
        .await
        .unwrap();
    assert!(
        tables
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "notes")
    );
    session.call("drift.insert",json!({"provider":drift,"table":"notes","values":{"id":99,"title":"new ' ${row}","completed":0,"payload":{"base64":"AP8B"}}})).await.unwrap();
    session.call("drift.update",json!({"provider":drift,"table":"notes","identity":{"id":99},"values":{"title":"updated"}})).await.unwrap();
    let rows = session
        .call(
            "drift.rows",
            json!({"provider":drift,"table":"notes","filter":"updated","order":"id"}),
        )
        .await
        .unwrap();
    assert_eq!(rows["total"], 1);
    assert_eq!(rows["rows"][0]["payload"], json!({"base64":"AP8B"}));
    assert!(
        session
            .call(
                "drift.delete",
                json!({"provider":drift,"table":"notes","identity":{}})
            )
            .await
            .is_err()
    );
    session
        .call(
            "drift.delete",
            json!({"provider":drift,"table":"notes","identity":{"id":99}}),
        )
        .await
        .unwrap();
    let rows = session
        .call(
            "drift.query",
            json!({"provider":drift,"sql":"SELECT count(*) AS n FROM notes;"}),
        )
        .await
        .unwrap();
    assert_eq!(rows[0]["n"], 2);

    // Grid mutation batches run inside Drift's transaction context.
    session.call("drift.execute",json!({"provider":drift,"sql":"CREATE TABLE grid_test(id INTEGER PRIMARY KEY, title TEXT NOT NULL DEFAULT 'new', payload BLOB)"})).await.unwrap();
    session.call("drift.apply",json!({"provider":drift,"table":"grid_test","changes":[{"kind":"insert","values":{}},{"kind":"insert","values":{"title":"second"}}]})).await.unwrap();
    let failed=session.call("drift.apply",json!({"provider":drift,"table":"grid_test","changes":[{"kind":"update","identity":{"id":1},"values":{"title":"roll back"}},{"kind":"insert","values":{"id":2}}]})).await;
    assert!(failed.is_err());
    let rows = session
        .call(
            "drift.rows",
            json!({"provider":drift,"table":"grid_test","order":"id"}),
        )
        .await
        .unwrap();
    assert_eq!(rows["rows"][0]["title"], "new");
    assert!(session.call("drift.apply",json!({"provider":drift,"table":"grid_test","changes":[{"kind":"update","identity":{"id":1},"original":{"title":"stale"},"values":{"title":"overwrite"}}]})).await.is_err());
    let changed=session.call("drift.apply",json!({"provider":drift,"table":"grid_test","changes":[{"kind":"update","identity":{"id":1},"original":{"title":"new"},"values":{"title":"saved","payload":{"base64":"AP8="}}},{"kind":"delete","identity":{"id":2}}]})).await.unwrap();
    assert_eq!(changed["changed"], 2);
    let rows = session
        .call("drift.rows", json!({"provider":drift,"table":"grid_test"}))
        .await
        .unwrap();
    assert_eq!(rows["rows"][0]["payload"], json!({"base64":"AP8="}));
    assert!(
        session
            .call(
                "drift.empty",
                json!({"provider":drift,"table":"grid_test","confirm":"wrong"})
            )
            .await
            .is_err()
    );
    let empty = session
        .call(
            "drift.empty",
            json!({"provider":drift,"table":"grid_test","confirm":"grid_test"}),
        )
        .await
        .unwrap();
    assert_eq!(empty["changed"], 1);
    let rows = session
        .call("drift.rows", json!({"provider":drift,"table":"grid_test"}))
        .await
        .unwrap();
    assert_eq!(rows["total"], 0);
    assert_eq!(rows["columns"].as_array().unwrap().len(), 3);
    let audit = hello
        .providers
        .iter()
        .find(|p| p.name == "AuditDatabase")
        .unwrap();
    let tables = session
        .call("drift.tables", json!({"provider":audit.id}))
        .await
        .unwrap();
    assert_eq!(tables.as_array().unwrap().len(), 1);
    assert_eq!(tables[0]["name"], "audit_log");

    // Dio is unmodified. Both binary bytes and JSON are captured through dart:io.
    vm.evaluate_async(isolate,root_library,"await fixture.dio.post('/json', data: {'hello': 'native'}); await fixture.dio.get('/binary'); await fixture.dio.get('/large'); return true;").await.unwrap();
    let page: EventPage =
        serde_json::from_value(session.call("events", json!({"after":0})).await.unwrap()).unwrap();
    let rows: Vec<Exchange> = page
        .events
        .into_iter()
        .map(|e| serde_json::from_value(e.data).unwrap())
        .collect();
    let post = rows.iter().find(|e| e.request.method == "POST").unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body(&session, post.request.body.as_ref().unwrap()).await)
            .unwrap(),
        json!({"hello":"native"})
    );
    let binary = rows
        .iter()
        .find(|e| e.request.url.ends_with("/binary"))
        .unwrap();
    assert_eq!(
        body(
            &session,
            binary.response.as_ref().unwrap().body.as_ref().unwrap()
        )
        .await,
        (0u8..=255).collect::<Vec<_>>()
    );
    let large = rows
        .iter()
        .find(|e| e.request.url.ends_with("/large"))
        .unwrap()
        .response
        .as_ref()
        .unwrap()
        .body
        .as_ref()
        .unwrap();
    assert!(large.size > 1024 * 1024);
    assert!(
        serde_json::from_slice::<Value>(&body(&session, large).await).is_ok(),
        "Large JSON survives VM string preview truncation and chunking"
    );
    assert!(
        session
            .call("intercept.rules", json!({}))
            .await
            .unwrap_err()
            .contains("optional")
    );
    session.call("detach", json!({})).await.unwrap();
    assert_eq!(
        vm.call(
            "ext.dart.io.httpEnableTimelineLogging",
            json!({"isolateId":isolate})
        )
        .await
        .unwrap()["enabled"],
        false
    );

    // Installing only the two interception packages must retain built-in prefs
    // and Drift, and must route those providers through the actual backend.
    vm.evaluate_async(isolate,root_library,
        "final bridge = await InspectorBridge.start(name: 'Optional Dio fixture'); DioInspector(id: 'api', dio: fixture.dio, bridge: bridge); return true;"
    ).await.unwrap();
    use inspector_application::{Command as Work, Event, Worker};
    let data = temp.path().join("inspector");
    let mut worker = Worker::start(
        || {},
        move |sink| inspector_adapters::backend::Services::new(sink, Some(data)),
    );
    worker
        .commands
        .send(Work::Connect {
            uri,
            isolate: Some(isolate.into()),
        })
        .await
        .unwrap();
    let connected = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match worker.events.recv().await.unwrap() {
                Event::Connected(session) => break session,
                Event::Error(error) | Event::ConnectionFailed(error) => panic!("{error}"),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert!(connected.hello.providers.iter().any(|p| p.kind == "dio"));
    assert!(
        connected
            .hello
            .providers
            .iter()
            .any(|p| p.id == "native.preferences.legacy")
    );
    assert!(connected.hello.providers.iter().any(|p| p.id == drift));
    worker
        .commands
        .send(Work::Provider {
            tag: "prefs.list".into(),
            method: "preferences.list".into(),
            params: json!({"provider":"native.preferences.legacy"}),
        })
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match worker.events.recv().await.unwrap() {
                Event::Provider { value, .. } => break value,
                Event::Error(error) => panic!("{error}"),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert!(
        result
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["key"] == "auth.token")
    );
    // Database lifecycle notifications must refresh a session that is already
    // connected, including with the optional Dio bridge installed.
    assert!(
        !connected
            .hello
            .providers
            .iter()
            .any(|p| p.name == "LateDatabase")
    );
    vm.evaluate_async(
        isolate,
        root_library,
        "await fixture.openLateDatabase(); return true;",
    )
    .await
    .unwrap();
    async fn wait_database_change(worker: &mut Worker, isolate: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Event::DriftDatabasesChanged { isolate: changed } =
                    worker.events.recv().await.unwrap()
                {
                    assert_eq!(changed, isolate);
                    break;
                }
            }
        })
        .await
        .expect("database lifecycle notification");
    }
    async fn refresh(worker: &mut Worker) -> Value {
        worker
            .commands
            .send(Work::Provider {
                tag: "drift.databases.test".into(),
                method: "drift.databases".into(),
                params: json!({}),
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match worker.events.recv().await.unwrap() {
                    Event::Provider { tag, value } if tag == "drift.databases.test" => break value,
                    Event::ProviderFailed { error, .. } => panic!("{error}"),
                    _ => {}
                }
            }
        })
        .await
        .unwrap()
    }
    wait_database_change(&mut worker, isolate).await;
    let providers = refresh(&mut worker).await;
    let late = providers
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "LateDatabase")
        .expect("late database detected");
    let direct = session.refresh_drift().await.unwrap();
    assert!(direct.iter().any(|p| p.id == late["id"].as_str().unwrap()));
    assert_eq!(
        session
            .call("drift.tables", json!({"provider":late["id"]}))
            .await
            .unwrap()[0]["name"],
        "late_notes"
    );
    vm.evaluate_async(
        isolate,
        root_library,
        "await fixture.closeLateDatabase(); return true;",
    )
    .await
    .unwrap();
    wait_database_change(&mut worker, isolate).await;
    let providers = refresh(&mut worker).await;
    assert!(
        !providers
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "LateDatabase")
    );
    session.refresh_drift().await.unwrap();
    assert!(
        session
            .call("drift.tables", json!({"provider":late["id"]}))
            .await
            .is_err()
    );
    drop(worker);
}
