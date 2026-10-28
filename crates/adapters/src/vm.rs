use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::{Error as SocketError, Message};

type Reply = oneshot::Sender<Result<Value, String>>;
pub struct VmClient {
    sender: mpsc::Sender<Value>,
    pending: Arc<Mutex<HashMap<u64, Reply>>>,
    sequence: AtomicU64,
    notifications: broadcast::Sender<Value>,
}
pub fn websocket_uri(uri: &str) -> Result<String, String> {
    let mut url = url::Url::parse(uri.trim()).map_err(|e| e.to_string())?;
    let scheme = match url.scheme() {
        "http" | "ws" => "ws",
        "https" | "wss" => "wss",
        _ => return Err("Use a Dart VM Service HTTP(S) or WS(S) URI".into()),
    };
    url.set_scheme(scheme).map_err(|_| "Invalid URI scheme")?;
    if !url.path().ends_with("/ws") {
        let path = format!("{}/ws", url.path().trim_end_matches('/'));
        url.set_path(&path);
    }
    Ok(url.into())
}
impl VmClient {
    pub fn subscribe_notifications(&self) -> broadcast::Receiver<Value> {
        self.notifications.subscribe()
    }
    pub async fn connect(uri: &str) -> Result<Arc<Self>, String> {
        let mut target = websocket_uri(uri)?;
        let (socket, _) = tokio::time::timeout(Duration::from_secs(10), async {
            for _ in 0..5 {
                match tokio_tungstenite::connect_async(&target).await {
                    Ok(connected) => return Ok(connected),
                    Err(SocketError::Http(response)) if response.status().is_redirection() => {
                        let location = response
                            .headers()
                            .get("location")
                            .and_then(|value| value.to_str().ok())
                            .ok_or("VM redirect has no valid Location header")?;
                        let next = url::Url::parse(&target)
                            .map_err(|e| e.to_string())?
                            .join(location)
                            .map_err(|e| e.to_string())?;
                        let next = websocket_uri(next.as_str())?;
                        if target.starts_with("wss:") && next.starts_with("ws:") {
                            return Err("VM redirect cannot downgrade a secure connection".into());
                        }
                        target = next;
                    }
                    Err(error) => return Err(error.to_string()),
                }
            }
            Err("Too many VM Service / DDS redirects".into())
        })
        .await
        .map_err(|_| "VM connection timed out")??;
        let (mut write, mut read) = socket.split();
        let (sender, mut receiver) = mpsc::channel::<Value>(64);
        let pending = Arc::new(Mutex::new(HashMap::<u64, Reply>::new()));
        let waiting = pending.clone();
        let (notifications, _) = broadcast::channel(128);
        let events = notifications.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    value=receiver.recv()=>match value{
                        Some(value)=>if write.send(Message::Text(value.to_string().into())).await.is_err(){break;},
                        None=>break,
                    },
                    message=read.next()=>match message{
                        Some(Ok(Message::Text(text)))=>{
                            if let Ok(value)=serde_json::from_str::<Value>(&text) {
                                if let Some(id)=value.get("id").and_then(Value::as_u64) {
                                    if let Some(reply)=waiting.lock().await.remove(&id){
                                        let result=if value.get("error").is_some(){Err(rpc_error(&value["error"]))}else{Ok(value["result"].clone())};
                                        let _=reply.send(result);
                                    }
                                } else if value["method"] == "streamNotify" {
                                    let _ = events.send(value["params"].clone());
                                }
                            }
                        }
                        Some(Ok(Message::Ping(p)))=>{let _=write.send(Message::Pong(p)).await;}
                        Some(Ok(Message::Close(_)))|Some(Err(_))|None=>break,
                        _=>{}
                    }
                }
            }
            for (_, reply) in waiting.lock().await.drain() {
                let _ = reply.send(Err("VM Service disconnected".into()));
            }
            let _ = write.close().await;
        });
        Ok(Arc::new(Self {
            sender,
            pending,
            sequence: AtomicU64::new(1),
            notifications,
        }))
    }
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.sequence.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            // An enclosing operation may time out before this RPC's own timer.
            pending.retain(|_, reply| !reply.is_closed());
            pending.insert(id, tx);
        }
        if self
            .sender
            .send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await
            .is_err()
        {
            self.pending.lock().await.remove(&id);
            return Err("VM Service disconnected".into());
        }
        match tokio::time::timeout(Duration::from_secs(150), rx).await {
            Ok(Ok(result)) => result,
            _ => {
                self.pending.lock().await.remove(&id);
                Err(format!("{method} timed out or connection closed"))
            }
        }
    }
    /// Heartbeat without requesting every loaded library and extension again.
    pub async fn check_isolate(&self, isolate: &str) -> Result<(), String> {
        let info = self.call("getVM", json!({})).await?;
        if info["isolates"]
            .as_array()
            .is_some_and(|isolates| isolates.iter().any(|entry| entry["id"] == isolate))
        {
            Ok(())
        } else {
            Err("Debug isolate restarted".into())
        }
    }
    pub async fn bridge(
        &self,
        isolate: &str,
        controller: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let value=self.call("ext.flutter_inspector.call",json!({"isolateId":isolate,"payload":json!({"controller":controller,"method":method,"params":params}).to_string()})).await?;
        if let Some(error) = value.get("error") {
            return Err(error.to_string());
        }
        Ok(value["result"].clone())
    }

    /// Read an expression result without assuming VM string previews are complete.
    pub async fn evaluate_string(
        &self,
        isolate: &str,
        library: &str,
        expression: &str,
    ) -> Result<String, String> {
        let value = self.call("evaluate", json!({"isolateId":isolate,"targetId":library,"expression":expression,"disableBreakpoints":true})).await?;
        if value["kind"] != "String" {
            return Err(format!(
                "VM evaluation failed: {}",
                value["message"]
                    .as_str()
                    .unwrap_or("library API unavailable")
            ));
        }
        if value["valueAsStringIsTruncated"] == true {
            let length = value["length"].as_u64().ok_or("Missing VM string length")?;
            if length > 2 * 1024 * 1024 {
                return Err("Inspection result exceeds 2 MiB; narrow the query".into());
            }
            let full = self
                .call(
                    "getObject",
                    json!({"isolateId":isolate,"objectId":value["id"],"offset":0,"count":length}),
                )
                .await?;
            if full["valueAsStringIsTruncated"] == true {
                return Err("VM returned a truncated result".into());
            }
            return full["valueAsString"]
                .as_str()
                .map(str::to_owned)
                .ok_or("VM result expired".into());
        }
        value["valueAsString"]
            .as_str()
            .map(str::to_owned)
            .ok_or("Missing VM result".into())
    }

    /// The library must import dart:developer as developer and dart:convert.
    /// Correlated events allow concurrent requests without consuming DevTools events.
    pub async fn evaluate_async(
        &self,
        isolate: &str,
        library: &str,
        statements: &str,
    ) -> Result<Value, String> {
        let event = format!("flutter_inspector.native.{}", uuid::Uuid::new_v4());
        let mut events = self.notifications.subscribe();
        let expression = format!(
            r#"(() async {{
          try {{
            final value = await (() async {{ {statements} }})();
            final encoded = jsonEncode(value);
            if (encoded.length > 2097152) throw StateError('Inspection result exceeds 2 MiB');
            developer.postEvent('{event}', {{'json': encoded}});
          }} catch (e) {{ developer.postEvent('{event}', {{'error': e.toString()}}); }}
        }})()"#
        );
        // Flutter's expression compiler transport expects one source line.
        // String data is JSON/Dart escaped before reaching this wrapper.
        let expression = expression.replace('\n', " ");
        tokio::time::timeout(Duration::from_secs(15), async {
            let value = self.call("evaluate", json!({"isolateId":isolate,"targetId":library,"expression":expression,"disableBreakpoints":true})).await?;
            if value["type"] == "Error" || value["type"] == "@Error" || value["type"] == "Sentinel" {
                return Err(value["message"].as_str().unwrap_or("VM evaluation unavailable").into());
            }
            loop {
                let message = events.recv().await.map_err(|e| format!("VM event stream: {e}"))?;
                if message["event"]["extensionKind"] != event { continue; }
                let data = &message["event"]["extensionData"];
                if let Some(error) = data["error"].as_str() { return Err(error.to_owned()); }
                return serde_json::from_str(data["json"].as_str().ok_or("Missing evaluation result")?).map_err(|e| e.to_string());
            }
        }).await.map_err(|_| "Inspection timed out; check that the app is not paused at a breakpoint".to_owned())?
    }
}
fn rpc_error(error: &Value) -> String {
    let details = error["data"]["details"].as_str().unwrap_or_default();
    if details.contains("No compilation service available") {
        return "Keep the app running from an IDE or flutter run; a standalone Flutter executable has no expression compiler for built-in inspection".into();
    }
    let message = error["message"].as_str().unwrap_or("VM Service error");
    if details.is_empty() {
        message.into()
    } else {
        format!("{message}: {details}")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn lightweight_heartbeat_detects_hot_restart_on_a_live_vm_socket() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            for isolate in ["isolates/1", "isolates/2"] {
                let Message::Text(request) = socket.next().await.unwrap().unwrap() else {
                    panic!("RPC expected");
                };
                let request: Value = serde_json::from_str(&request).unwrap();
                assert_eq!(
                    request["method"], "getVM",
                    "Heartbeat must not load full isolate details"
                );
                socket
                    .send(Message::Text(
                        json!({
                            "jsonrpc":"2.0", "id":request["id"],
                            "result":{"type":"VM", "isolates":[{"id":isolate}]}
                        })
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
            }
        });
        let vm = VmClient::connect(&format!("http://{address}/token=/"))
            .await
            .unwrap();
        vm.check_isolate("isolates/1").await.unwrap();
        assert_eq!(
            vm.check_isolate("isolates/1").await.unwrap_err(),
            "Debug isolate restarted"
        );
        server.await.unwrap();
    }
    #[test]
    fn uri_keeps_auth_path() {
        assert_eq!(
            websocket_uri("http://127.0.0.1:123/aBc=/").expect("uri"),
            "ws://127.0.0.1:123/aBc=/ws"
        );
        assert!(websocket_uri("file:///etc/passwd").is_err());
    }

    #[tokio::test]
    #[allow(clippy::result_large_err)] // Callback error type is supplied by Tungstenite.
    async fn follows_dds_redirect_with_its_new_token_and_websocket_path() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dds = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dds_port = dds.local_addr().unwrap().port();
        let service = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let service_port = service.local_addr().unwrap().port();
        let redirect = tokio::spawn(async move {
            let (mut socket, _) = service.accept().await.unwrap();
            let mut request = [0; 4096];
            let count = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..count]).starts_with("GET /original=/ws "));
            socket.write_all(format!("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{dds_port}/dds-token=/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        });
        let server = tokio::spawn(async move {
            let (socket, _) = dds.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_hdr_async(socket, |
                request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                response: tokio_tungstenite::tungstenite::handshake::server::Response
            | {
                assert_eq!(request.uri().path(), "/dds-token=/ws");
                Ok(response)
            }).await.unwrap();
            let Message::Text(request) = socket.next().await.unwrap().unwrap() else {
                panic!("RPC expected");
            };
            let request: Value = serde_json::from_str(&request).unwrap();
            socket
                .send(Message::Text(
                    json!({"jsonrpc":"2.0","id":request["id"],"result":{"type":"VM"}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
        });
        let vm = VmClient::connect(&format!("http://127.0.0.1:{service_port}/original=/"))
            .await
            .unwrap();
        assert_eq!(vm.call("getVM", json!({})).await.unwrap()["type"], "VM");
        redirect.await.unwrap();
        server.await.unwrap();
    }
}
