use crate::store::{Store, now};
use base64::Engine;
use inspector_domain::*;
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufWriter, Write},
    path::Path,
};

fn headers(value: &Value) -> Vec<Header> {
    value
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|h| {
                    Some(Header {
                        name: h["name"].as_str()?.into(),
                        value: h["value"].as_str()?.into(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}
pub fn export(store: &Store, path: &Path) -> Result<(), String> {
    let temp = path.with_extension("inspector.tmp");
    let mut writer = BufWriter::new(fs::File::create(&temp).map_err(|e| e.to_string())?);
    writer.write_all(br#"{"log":{"version":"1.2","creator":{"name":"Flutter Inspector","version":"0.1.0"},"entries":["#).map_err(|e|e.to_string())?;
    let mut index = 0;
    store.visit(|e| {
        if index > 0 {
            writer.write_all(b",").map_err(|e| e.to_string())?;
        }
        let body = |b: Option<&Body>| {
            if b.is_none(){return (String::new(),false,true);}
            b.and_then(|b| store.body(&b.id).ok())
                .map(|bytes| match String::from_utf8(bytes) {
                    Ok(text) => (text, false, true),
                    Err(e) => (
                        base64::engine::general_purpose::STANDARD.encode(e.into_bytes()),
                        true,
                        true,
                    ),
                })
                .unwrap_or_default()
        };
        let (request_body, request_encoded, request_available) = body(e.request.body.as_ref());
        let (response_body, response_encoded, response_available) =
            body(e.response.as_ref().and_then(|r| r.body.as_ref()));
        let timestamp = chrono::DateTime::from_timestamp_millis(e.started_at)
            .map(|d| d.to_rfc3339())
            .unwrap_or_default();
        let response = e.response.clone().unwrap_or_default();
        let content_type = response
            .body
            .as_ref()
            .map(|b| b.content_type.as_str())
            .unwrap_or("");
        let value = json!({
            "startedDateTime":timestamp,"time":e.duration_ms,
            "request":{"method":e.request.method,"url":e.request.url,"httpVersion":"HTTP/1.1","headers":e.request.headers,
                "cookies":[],"queryString":[],"headersSize":-1,"bodySize":e.request.body.as_ref().map(|b|b.size).unwrap_or(0),
                "postData":{"mimeType":e.request.body.as_ref().map(|b|b.content_type.as_str()).unwrap_or(""),"text":request_body,"_encoding":if request_encoded{"base64"}else{""}}},
            "response":{"status":response.status,"statusText":"","httpVersion":"HTTP/1.1","headers":response.headers,"cookies":[],
                "content":{"size":response.body.as_ref().map(|b|b.size).unwrap_or(0),"mimeType":content_type,"text":response_body,"encoding":if response_encoded{"base64"}else{""}},
                "redirectURL":"","headersSize":-1,"bodySize":response.body.as_ref().map(|b|b.size).unwrap_or(0)},
            "cache":{},"timings":{"send":0,"wait":e.duration_ms,"receive":0},
            "_inspector":{"state":e.state,"error":e.error,"source":e.source,
                "requestComplete":request_available && e.request.body.as_ref().map(|b|b.complete).unwrap_or(true),
                "responseComplete":response_available && response.body.as_ref().map(|b|b.complete).unwrap_or(true)}
        });
        serde_json::to_writer(&mut writer, &value).map_err(|e| e.to_string())?;
        index+=1;
        Ok(())
    })?;
    writer.write_all(b"]}}").map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    drop(writer);
    fs::rename(temp, path).map_err(|e| e.to_string())
}
pub fn import(store: &Store, path: &Path) -> Result<usize, String> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() > 100 * 1024 * 1024 {
        return Err("HAR import is limited to 100 MiB per file".into());
    }
    let value: Value = serde_json::from_reader(fs::File::open(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let entries = value["log"]["entries"]
        .as_array()
        .ok_or("Expected HAR log.entries")?;
    for entry in entries {
        let id = uuid::Uuid::new_v4().to_string();
        let decode = |text: &Value, encoding: &Value| -> Result<Vec<u8>, String> {
            let text = text.as_str().unwrap_or("");
            if encoding.as_str() == Some("base64") {
                base64::engine::general_purpose::STANDARD
                    .decode(text)
                    .map_err(|e| e.to_string())
            } else {
                Ok(text.as_bytes().to_vec())
            }
        };
        let mut req = decode(
            &entry["request"]["postData"]["text"],
            &entry["request"]["postData"]["_encoding"],
        )?;
        let mut res = decode(
            &entry["response"]["content"]["text"],
            &entry["response"]["content"]["encoding"],
        )?;
        let req_complete = req.len() <= BODY_LIMIT
            && entry["_inspector"]["requestComplete"]
                .as_bool()
                .unwrap_or(true);
        let res_complete = res.len() <= BODY_LIMIT
            && entry["_inspector"]["responseComplete"]
                .as_bool()
                .unwrap_or(true);
        req.truncate(BODY_LIMIT);
        res.truncate(BODY_LIMIT);
        let request = Body {
            id: format!("{id}-request"),
            size: req.len() as u64,
            complete: req_complete,
            content_type: entry["request"]["postData"]["mimeType"]
                .as_str()
                .unwrap_or("")
                .into(),
        };
        let response = Body {
            id: format!("{id}-response"),
            size: res.len() as u64,
            complete: res_complete,
            content_type: entry["response"]["content"]["mimeType"]
                .as_str()
                .unwrap_or("")
                .into(),
        };
        let e = Exchange {
            id,
            session_id: "import".into(),
            client_id: "HAR".into(),
            started_at: entry["startedDateTime"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp_millis())
                .unwrap_or_else(now),
            duration_ms: entry["time"].as_f64().unwrap_or(0.0).max(0.0) as u64,
            source: "har".into(),
            state: serde_json::from_value(entry["_inspector"]["state"].clone())
                .unwrap_or(ExchangeState::Complete),
            error: entry["_inspector"]["error"].as_str().map(str::to_owned),
            request: Request {
                method: entry["request"]["method"]
                    .as_str()
                    .ok_or("Missing request method")?
                    .into(),
                url: entry["request"]["url"]
                    .as_str()
                    .ok_or("Missing request URL")?
                    .into(),
                headers: headers(&entry["request"]["headers"]),
                body: Some(request.clone()),
            },
            response: Some(Response {
                status: entry["response"]["status"].as_u64().unwrap_or(0) as u16,
                headers: headers(&entry["response"]["headers"]),
                body: Some(response.clone()),
            }),
            ..Default::default()
        };
        store.put(&e)?;
        store.put_body(&request.id, &req)?;
        store.put_body(&response.id, &res)?;
    }
    Ok(entries.len())
}
