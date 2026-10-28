use crate::store::{Store, now};
use base64::Engine;
use futures_util::StreamExt;
use inspector_domain::*;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct CancellationRecord {
    store: Arc<Mutex<Store>>,
    exchange: Exchange,
    finished: bool,
}
impl Drop for CancellationRecord {
    fn drop(&mut self) {
        if !self.finished {
            self.exchange.state = ExchangeState::Cancelled;
            self.exchange.error = Some("Repeater request cancelled".into());
            if let Ok(store) = self.store.lock() {
                let _ = store.put(&self.exchange);
            }
        }
    }
}
pub async fn replay(draft: Draft, store: Arc<Mutex<Store>>) -> Result<Exchange, String> {
    replay_inner(draft, store, uuid::Uuid::new_v4().to_string(), false).await
}
pub async fn replay_run(
    draft: Draft,
    store: Arc<Mutex<Store>>,
    id: String,
) -> Result<Exchange, String> {
    replay_inner(draft, store, id, true).await
}
async fn replay_inner(
    draft: Draft,
    store: Arc<Mutex<Store>>,
    id: String,
    reserved: bool,
) -> Result<Exchange, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .redirect(if draft.follow_redirects {
            reqwest::redirect::Policy::limited(10)
        } else {
            reqwest::redirect::Policy::none()
        })
        .build()
        .map_err(|e| e.to_string())?;
    let url = url::Url::parse(&draft.url).map_err(|e| e.to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Only HTTP(S) replay is supported".into());
    }
    let bytes = if draft.body_base64 {
        base64::engine::general_purpose::STANDARD
            .decode(&draft.body)
            .map_err(|e| e.to_string())?
    } else {
        draft.body.as_bytes().to_vec()
    };
    if bytes.len() > BODY_LIMIT {
        return Err("Request body exceeds the 20 MiB desktop limit".into());
    }
    let request_body = Body {
        id: format!("{id}-request"),
        size: bytes.len() as u64,
        complete: true,
        content_type: draft
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-type"))
            .map(|h| h.value.clone())
            .unwrap_or_default(),
    };
    let mut exchange = Exchange {
        id: id.clone(),
        repeater_id: Some(draft.id.clone()),
        session_id: "desktop".into(),
        client_id: draft.id.clone(),
        started_at: now(),
        source: "repeater_desktop".into(),
        request: Request {
            method: draft.method.clone(),
            url: draft.url.clone(),
            headers: draft.headers.clone(),
            body: Some(request_body.clone()),
        },
        ..Default::default()
    };
    {
        let store = store.lock().map_err(|e| e.to_string())?;
        if reserved {
            if !store.put(&exchange)? {
                return Err("Repeater send was cancelled or replaced".into());
            }
        } else {
            store.begin_repeater(&draft, &exchange)?;
        }
        store.put_body(&request_body.id, &bytes)?;
    }
    let mut cancellation = CancellationRecord {
        store: store.clone(),
        exchange: exchange.clone(),
        finished: false,
    };
    let mut request = client
        .request(
            reqwest::Method::from_bytes(draft.method.as_bytes()).map_err(|e| e.to_string())?,
            url,
        )
        .body(bytes);
    for h in &draft.headers {
        if !h.name.eq_ignore_ascii_case("content-length") {
            request = request.header(&h.name, &h.value);
        }
    }
    let start = Instant::now();
    match request.send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .map(|(k, v)| Header {
                    name: k.to_string(),
                    value: String::from_utf8_lossy(v.as_bytes()).into_owned(),
                })
                .collect::<Vec<_>>();
            let content_type = headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case("content-type"))
                .map(|h| h.value.clone())
                .unwrap_or_default();
            let mut stream = response.bytes_stream();
            let mut body = vec![];
            let mut complete = true;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(chunk) => {
                        let remaining = BODY_LIMIT - body.len();
                        let take = remaining.min(chunk.len());
                        body.extend_from_slice(&chunk[..take]);
                        if take < chunk.len() {
                            complete = false;
                            break;
                        }
                    }
                    Err(e) => {
                        exchange.error = Some(e.to_string());
                        complete = false;
                        break;
                    }
                }
            }
            let descriptor = Body {
                id: format!("{id}-response"),
                size: body.len() as u64,
                complete,
                content_type,
            };
            exchange.response = Some(Response {
                status,
                headers,
                body: Some(descriptor.clone()),
            });
            exchange.state = if exchange.error.is_some() {
                ExchangeState::Error
            } else {
                ExchangeState::Complete
            };
            exchange.duration_ms = start.elapsed().as_millis() as u64;
            let store = store.lock().map_err(|e| e.to_string())?;
            store.put(&exchange)?;
            store.put_body(&descriptor.id, &body)?;
        }
        Err(e) => {
            exchange.error = Some(e.to_string());
            exchange.state = ExchangeState::Error;
            exchange.duration_ms = start.elapsed().as_millis() as u64;
            store.lock().map_err(|e| e.to_string())?.put(&exchange)?;
        }
    }
    cancellation.finished = true;
    Ok(exchange)
}
