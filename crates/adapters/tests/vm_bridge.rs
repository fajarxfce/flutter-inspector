use base64::Engine;
use inspector_adapters::discovery::{local_apps, tooling_apps};
use inspector_adapters::vm::VmClient;
use inspector_domain::{EventPage, Exchange, Hello};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Fixture(Child);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
#[ignore = "requires Dart SDK and dart pub get; run cargo test -p inspector-adapters --test vm_bridge -- --ignored"]
async fn real_vm_capture_intercept_replay_and_lease() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = tempfile::tempdir().expect("fixture directory");
    let service = dir.path().join("service.json");
    let _fixture = Fixture(
        Command::new("dart")
            .current_dir(&root)
            .arg("--enable-vm-service=0")
            .arg(format!("--write-service-info={}", service.display()))
            .args([
                "run",
                "--no-dds",
                "packages/flutter_inspector_dio/test/fixtures/vm_fixture.dart",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start Dart fixture"),
    );
    let uri = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(bytes) = std::fs::read(&service)
                && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
                && let Some(uri) = value["uri"].as_str()
            {
                break uri.to_owned();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("VM URI");
    // Match IDE launches: DDS redirects the VM websocket to another port/token.
    let dds_status = dir.path().join("dds.json");
    let _dds = Fixture(
        Command::new("dart")
            .arg("development-service")
            .arg(format!("--vm-service-uri={uri}"))
            .args(["--bind-address=127.0.0.1", "--bind-port=0"])
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&dds_status).expect("DDS status file"))
            .spawn()
            .expect("start real DDS"),
    );
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(bytes) = std::fs::read(&dds_status)
                && let Ok(status) = serde_json::from_slice::<Value>(&bytes)
            {
                assert_eq!(status["state"], "started", "DDS failed to start: {status}");
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("DDS started");
    assert!(
        matches!(
            tokio_tungstenite::connect_async(inspector_adapters::vm::websocket_uri(&uri).unwrap()).await,
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status().is_redirection()
        ),
        "Fixture must exercise the real VM-to-DDS redirect"
    );
    let vm = VmClient::connect(&uri)
        .await
        .expect("authenticated websocket");
    let isolate = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let info = vm.call("getVM", json!({})).await.expect("getVM");
            for isolate in info["isolates"].as_array().expect("isolates") {
                let id = isolate["id"].as_str().expect("id");
                let details = vm
                    .call("getIsolate", json!({"isolateId":id}))
                    .await
                    .expect("isolate");
                if details["extensionRPCs"]
                    .as_array()
                    .is_some_and(|r| r.iter().any(|v| v == "ext.flutter_inspector.call"))
                {
                    return id.to_owned();
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("bridge registration");
    let discovered = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(app) = local_apps()
                .await
                .into_iter()
                .find(|app| app.isolate == isolate && app.name == "Protocol fixture")
            {
                break app;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("automatic local discovery without supplying a URI");
    assert_eq!(discovered.name, "Protocol fixture");
    assert!(!discovered.busy);
    assert!(
        tooling_apps()
            .await
            .into_iter()
            .any(|app| app.isolate == isolate && app.name == "Protocol fixture"),
        "The IDE process fallback must work without a local descriptor or Android log"
    );
    let controller = "integration";
    let hello: Hello = serde_json::from_value(
        vm.bridge(&isolate, controller, "hello", json!({}))
            .await
            .expect("hello"),
    )
    .expect("contract");
    hello.validate().expect("version");
    assert!(hello.providers.iter().any(|p| p.kind == "dio"));
    assert!(
        vm.bridge(&isolate, "another", "hello", json!({}))
            .await
            .is_err()
    );
    let result = vm
        .bridge(
            &isolate,
            controller,
            "fixture.request",
            json!({"provider":"fixture"}),
        )
        .await
        .expect("real request");
    assert_eq!(result["data"]["body"], "original");
    let page: EventPage = serde_json::from_value(
        vm.bridge(&isolate, controller, "events", json!({"after":0}))
            .await
            .expect("events"),
    )
    .expect("event contract");
    let exchange: Exchange =
        serde_json::from_value(page.events.last().expect("network event").data.clone())
            .expect("exchange contract");
    let body = exchange.response.expect("response").body.expect("body");
    let chunk = vm
        .bridge(
            &isolate,
            controller,
            "body.read",
            json!({"id":body.id,"offset":0}),
        )
        .await
        .expect("body");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(chunk["data"].as_str().expect("base64"))
        .expect("bytes");
    assert!(
        String::from_utf8(bytes)
            .expect("json")
            .contains("9007199254740993")
    );
    vm.bridge(&isolate,controller,"intercept.rules",json!({"enabled":true,"host":"","path":"","method":"","request":true,"response":false,"holdSeconds":120})).await.expect("rules");
    let sending_vm = vm.clone();
    let sending_isolate = isolate.clone();
    let sending = tokio::spawn(async move {
        sending_vm
            .bridge(
                &sending_isolate,
                controller,
                "fixture.request",
                json!({"provider":"fixture"}),
            )
            .await
    });
    let pending = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let page: EventPage = serde_json::from_value(
                vm.bridge(&isolate, controller, "events", json!({}))
                    .await
                    .expect("events"),
            )
            .expect("contract");
            if let Some(pending) = page.pending.first() {
                break pending.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("held request");
    vm.bridge(
        &isolate,
        controller,
        "network.decide",
        json!({"id":pending.id,"action":"forward","body":"edited via Rust","method":"PUT"}),
    )
    .await
    .expect("forward");
    let result = sending.await.expect("join").expect("response");
    assert_eq!(result["data"]["body"], "edited via Rust");
    assert_eq!(result["data"]["method"], "PUT");
    let info = vm
        .bridge(
            &isolate,
            controller,
            "fixture.info",
            json!({"provider":"fixture"}),
        )
        .await
        .expect("info");
    let replay=vm.bridge(&isolate,controller,"replay",json!({"provider":"api","draftId":"tab-one","id":"send-one","method":"POST","url":format!("{}/echo",info["url"].as_str().expect("url")),"headers":[],"body":"replay"})).await.expect("replay");
    assert_eq!(replay["status"], 200);
    assert_eq!(replay["exchange"]["id"], "send-one");
    assert_eq!(replay["exchange"]["repeaterId"], "tab-one");
    let replay_body = &replay["exchange"]["response"]["body"]["id"];
    let chunk = vm
        .bridge(
            &isolate,
            controller,
            "body.read",
            json!({"id":replay_body,"offset":0}),
        )
        .await
        .unwrap();
    let replay_bytes = base64::engine::general_purpose::STANDARD
        .decode(chunk["data"].as_str().unwrap())
        .unwrap();
    assert!(String::from_utf8(replay_bytes).unwrap().contains("replay"));
    vm.bridge(&isolate, controller, "detach", json!({}))
        .await
        .expect("detach");
    vm.bridge(&isolate, "another", "hello", json!({}))
        .await
        .expect("lease released");
}
