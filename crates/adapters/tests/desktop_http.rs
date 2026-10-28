use inspector_adapters::{http, store::Store};
use inspector_domain::{Draft, ExchangeState, Header};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    time::Duration,
};

#[tokio::test]
async fn desktop_replay_preserves_binary_and_duplicate_response_headers() {
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = server.local_addr().unwrap();
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = server.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = vec![];
        let mut buffer = [0; 1024];
        loop {
            let n = stream.read(&mut buffer).unwrap();
            request.extend_from_slice(&buffer[..n]);
            if request.ends_with(&[0, 255, 1]) {
                break;
            }
            assert!(n > 0);
        }
        assert!(String::from_utf8_lossy(&request).contains("Bearer fixture-secret"));
        stream.write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 4\r\nX-Repeat: one\r\nX-Repeat: two\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n\x00\xff\x01\x80").unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Mutex::new(Store::open(dir.path().into()).unwrap()));
    let draft = Draft {
        url: format!("http://{address}/echo"),
        method: "POST".into(),
        body: "AP8B".into(),
        body_base64: true,
        headers: vec![Header {
            name: "Authorization".into(),
            value: "Bearer fixture-secret".into(),
        }],
        ..Default::default()
    };
    let result = http::replay(draft, store.clone()).await.unwrap();
    assert_eq!(result.state, ExchangeState::Complete);
    let response = result.response.unwrap();
    assert_eq!(response.status, 201);
    assert_eq!(
        response
            .headers
            .iter()
            .filter(|h| h.name == "x-repeat")
            .count(),
        2
    );
    assert_eq!(
        store
            .lock()
            .unwrap()
            .body(&response.body.unwrap().id)
            .unwrap(),
        [0, 255, 1, 128]
    );
    thread.join().unwrap();
}
#[tokio::test]
async fn cancellation_is_persisted_as_terminal_state() {
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = server.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Mutex::new(Store::open(dir.path().into()).unwrap()));
    let task_store = store.clone();
    let task = tokio::spawn(async move {
        http::replay(
            Draft {
                url: format!("http://{address}/slow"),
                ..Default::default()
            },
            task_store,
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    task.abort();
    let _ = task.await;
    let result = store
        .lock()
        .unwrap()
        .repeater_result("scratch")
        .unwrap()
        .unwrap();
    assert_eq!(result.state, ExchangeState::Cancelled);
    assert_eq!(store.lock().unwrap().history("", 0).unwrap().1, 0);
}

#[tokio::test]
async fn deleted_reservation_cannot_restart_a_send_or_recreate_a_draft() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Mutex::new(Store::open(dir.path().into()).unwrap()));
    let draft = Draft {
        url: "http://127.0.0.1:1".into(),
        ..Default::default()
    };
    let result = http::replay_run(draft, store.clone(), "deleted-run".into()).await;
    assert!(result.unwrap_err().contains("cancelled or replaced"));
    let store = store.lock().unwrap();
    assert!(store.drafts().unwrap().is_empty());
    assert!(store.repeater_result("scratch").unwrap().is_none());
}
