use super::dart_string;
use crate::vm::VmClient;
use inspector_domain::{BODY_LIMIT, Body, Exchange, ExchangeState, Header, Request, Response};
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) struct Network {
    vm: Arc<VmClient>,
    isolate: String,
    library: String,
    session: String,
    was_enabled: bool,
}
impl Network {
    pub async fn open(
        vm: Arc<VmClient>,
        isolate: &str,
        library: &str,
        session: &str,
    ) -> Result<Self, String> {
        let state = vm
            .call(
                "ext.dart.io.httpEnableTimelineLogging",
                json!({"isolateId":isolate}),
            )
            .await?;
        let was_enabled = state["enabled"] == true;
        if !was_enabled {
            vm.call(
                "ext.dart.io.httpEnableTimelineLogging",
                json!({"isolateId":isolate,"enabled":"true"}),
            )
            .await?;
        }
        Ok(Self {
            vm,
            isolate: isolate.into(),
            library: library.into(),
            session: session.into(),
            was_enabled,
        })
    }
    pub async fn close(&self) -> Result<(), String> {
        if !self.was_enabled {
            self.vm
                .call(
                    "ext.dart.io.httpEnableTimelineLogging",
                    json!({"isolateId":self.isolate,"enabled":"false"}),
                )
                .await?;
        }
        Ok(())
    }
    pub async fn events(&self, after: u64) -> Result<Value, String> {
        let mut params = json!({"isolateId":self.isolate});
        if after > 0 {
            params["updatedSince"] = after.to_string().into();
        }
        let profile = self.vm.call("ext.dart.io.getHttpProfile", params).await?;
        let requests = profile["requests"]
            .as_array()
            .ok_or("Invalid Dart HTTP profile")?;
        // One bounded size query per poll, without fetching full bodies as JSON byte arrays.
        let terminal: Vec<&str> = requests
            .iter()
            .take(1000)
            .filter(|r| finished(r))
            .filter_map(|r| r["id"].as_str())
            .collect();
        let sizes: Value = if terminal.is_empty() {
            json!({})
        } else {
            let ids = dart_string(&serde_json::to_string(&terminal).map_err(|e| e.to_string())?);
            let expression = format!(
                "jsonEncode({{for (final id in jsonDecode({ids})) id: (() {{ final p = HttpProfiler.getHttpProfileRequest(id); return p == null ? null : [p.requestBody.length, p.responseBody.length]; }})()}})"
            );
            let value = self
                .vm
                .evaluate_string(&self.isolate, &self.library, &expression)
                .await?;
            serde_json::from_str(&value).map_err(|e| e.to_string())?
        };
        let mut events = Vec::new();
        for request in requests.iter().take(1000) {
            let id = request["id"].as_str().ok_or("HTTP request has no id")?;
            let exchange = exchange(&self.session, request, &sizes[id])?;
            events.push(json!({"sequence":profile["timestamp"],"kind":"network","data":exchange}));
        }
        Ok(
            json!({"cursor":profile["timestamp"],"events":events,"pending":[],"dropped":requests.len().saturating_sub(1000)}),
        )
    }
    pub async fn read(&self, params: &Value) -> Result<Value, String> {
        let id = params["id"].as_str().ok_or("Missing body id")?;
        let parts: Vec<_> = id.split('|').collect();
        if parts.len() != 3 || parts[0] != self.session {
            return Err("Body belongs to another debug session".into());
        }
        let field = match parts[2] {
            "request" => "requestBody",
            "response" => "responseBody",
            _ => return Err("Invalid body direction".into()),
        };
        let offset = params["offset"]
            .as_u64()
            .unwrap_or(0)
            .min(BODY_LIMIT as u64);
        let end = (offset + 65536).min(BODY_LIMIT as u64);
        let request = dart_string(parts[1]);
        let expression = format!(
            "(() {{ final p = HttpProfiler.getHttpProfileRequest({request}); if (p == null) throw StateError('HTTP profile was cleared'); final body = p.{field}; return base64Encode(body.sublist({offset}, body.length < {end} ? body.length : {end})); }})()"
        );
        let bytes = self
            .vm
            .evaluate_string(&self.isolate, &self.library, &expression)
            .await?;
        Ok(json!({"data":bytes}))
    }
}
fn finished(value: &Value) -> bool {
    value["response"]["endTime"].is_number()
        || value["request"]["error"].is_string()
        || value["response"]["error"].is_string()
}
fn headers(value: &Value) -> Vec<Header> {
    let mut headers = Vec::new();
    if let Some(map) = value.as_object() {
        for (name, values) in map {
            if let Some(values) = values.as_array() {
                for value in values {
                    if let Some(value) = value.as_str() {
                        headers.push(Header {
                            name: name.clone(),
                            value: value.into(),
                        });
                    }
                }
            }
        }
    }
    headers
}
fn body(
    session: &str,
    id: &str,
    side: &str,
    size: &Value,
    headers: &[Header],
    complete: bool,
) -> Option<Body> {
    let size = size.as_u64()?;
    Some(Body {
        id: format!("{session}|{id}|{side}"),
        size: size.min(BODY_LIMIT as u64),
        complete: complete && size <= BODY_LIMIT as u64,
        content_type: headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-type"))
            .map(|h| h.value.clone())
            .unwrap_or_default(),
    })
}
fn exchange(session: &str, value: &Value, sizes: &Value) -> Result<Exchange, String> {
    let id = value["id"].as_str().ok_or("HTTP id missing")?;
    let start = value["startTime"].as_i64().unwrap_or(0);
    let end = value["response"]["endTime"]
        .as_i64()
        .or(value["endTime"].as_i64())
        .unwrap_or(start);
    let request_headers = headers(&value["request"]["headers"]);
    let response_headers = headers(&value["response"]["headers"]);
    let error = value["request"]["error"]
        .as_str()
        .or(value["response"]["error"].as_str())
        .unwrap_or_default()
        .to_owned();
    let complete = finished(value) && error.is_empty();
    Ok(Exchange {
        id: format!("{session}|{id}"),
        repeater_id: None,
        session_id: session.into(),
        client_id: "native.http".into(),
        started_at: start / 1000,
        duration_ms: end.saturating_sub(start).max(0) as u64 / 1000,
        source: "dart:io".into(),
        pinned: false,
        state: if !error.is_empty() {
            ExchangeState::Error
        } else if complete {
            ExchangeState::Complete
        } else {
            ExchangeState::Pending
        },
        error: if error.is_empty() { None } else { Some(error) },
        request: Request {
            method: value["method"].as_str().unwrap_or("GET").into(),
            url: value["uri"].as_str().unwrap_or_default().into(),
            body: body(
                session,
                id,
                "request",
                &sizes[0],
                &request_headers,
                value["endTime"].is_number() && !value["request"]["error"].is_string(),
            ),
            headers: request_headers,
        },
        response: value["response"]["statusCode"]
            .as_u64()
            .map(|status| Response {
                status: status as u16,
                body: body(
                    session,
                    id,
                    "response",
                    &sizes[1],
                    &response_headers,
                    complete,
                ),
                headers: response_headers,
            }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sending_finished_does_not_mean_response_finished() {
        let mut value = json!({"id":"1","method":"POST","uri":"https://example.test","startTime":1000,"endTime":2000,"response":{"statusCode":200}});
        assert_eq!(
            exchange("s", &value, &Value::Null).unwrap().state,
            ExchangeState::Pending
        );
        value["response"]["endTime"] = json!(6000);
        let result = exchange("s", &value, &json!([3, BODY_LIMIT + 1])).unwrap();
        assert_eq!(result.duration_ms, 5);
        assert_eq!(result.state, ExchangeState::Complete);
        assert!(!result.response.unwrap().body.unwrap().complete);
    }
    #[test]
    fn failed_upload_is_not_marked_safe_to_duplicate() {
        let value = json!({"id":"1","startTime":1000,"endTime":2000,"request":{"error":"connection closed"}});
        let result = exchange("s", &value, &json!([12, 0])).unwrap();
        assert_eq!(result.state, ExchangeState::Error);
        assert!(!result.request.body.unwrap().complete);
    }
}
