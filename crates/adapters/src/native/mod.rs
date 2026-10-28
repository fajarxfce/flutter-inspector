//! Adapters for debug APIs already shipped with Dart, SharedPreferences and Drift.
//! No application registration or Inspector SDK is needed for these capabilities.
mod drift;
mod network;
mod preferences;

use crate::vm::VmClient;
use inspector_domain::{Hello, PROTOCOL_MAJOR, Provider};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};

pub fn library(details: &Value, uri: &str) -> Option<String> {
    details["libraries"]
        .as_array()?
        .iter()
        .find(|lib| lib["uri"] == uri)?["id"]
        .as_str()
        .map(str::to_owned)
}
pub fn extension(details: &Value, name: &str) -> bool {
    details["extensionRPCs"]
        .as_array()
        .is_some_and(|r| r.iter().any(|v| v == name))
}
const PREFS: &str =
    "package:shared_preferences/src/shared_preferences_devtools_extension_data.dart";
const DRIFT: &str = "package:drift/src/runtime/devtools/devtools.dart";

pub fn supported(details: &Value) -> bool {
    extension(details, "ext.dart.io.getHttpProfile")
        || extension(details, "ext.drift.database")
        || library(details, PREFS).is_some()
}

/// JSON strings also need Dart's interpolation character escaped.
fn dart_string(value: &str) -> String {
    serde_json::to_string(value)
        .expect("string serialization")
        .replace('$', r"\$")
}

pub struct NativeSession {
    vm: Arc<VmClient>,
    isolate: String,
    preferences: Option<String>,
    databases: tokio::sync::Mutex<HashMap<String, i64>>,
    network: Option<network::Network>,
}
impl NativeSession {
    pub async fn open(
        vm: Arc<VmClient>,
        isolate: &str,
        details: &Value,
    ) -> Result<(Self, Hello), String> {
        Self::open_with_network(vm, isolate, details, true).await
    }
    pub async fn open_with_network(
        vm: Arc<VmClient>,
        isolate: &str,
        details: &Value,
        capture_http: bool,
    ) -> Result<(Self, Hello), String> {
        let mut providers = Vec::new();
        let preferences = library(details, PREFS);
        if preferences.is_some() {
            for (id, name) in [
                ("native.preferences.legacy", "SharedPreferences"),
                (
                    "native.preferences.async",
                    "SharedPreferencesAsync (default store)",
                ),
            ] {
                providers.push(Provider {
                    id: id.into(),
                    kind: "preferences".into(),
                    name: name.into(),
                });
            }
        }
        // Optional Drift discovery must not prevent HTTP/preferences connecting.
        // The explicit refresh reports discovery errors in the Drift pane.
        let (drift, databases) = detect_drift(&vm, isolate, details)
            .await
            .unwrap_or_default();
        providers.extend(drift);
        // Isolate ids change on hot restart. Reconnecting to the same isolate
        // must not duplicate its existing HTTP history.
        let session_id = format!("native-{}", isolate.replace('/', "-"));
        let network = if capture_http && extension(details, "ext.dart.io.getHttpProfile") {
            let http =
                library(details, "dart:_http").ok_or("Dart HTTP profiler library unavailable")?;
            let network = network::Network::open(vm.clone(), isolate, &http, &session_id).await?;
            providers.push(Provider {
                id: "native.http".into(),
                kind: "http".into(),
                name: "Dart HTTP (including standard Dio)".into(),
            });
            Some(network)
        } else {
            None
        };
        let hello = Hello {
            protocol: PROTOCOL_MAJOR,
            session_id,
            name: details["rootLib"]["uri"]
                .as_str()
                .and_then(|uri| {
                    uri.strip_prefix("package:")
                        .and_then(|p| p.split('/').next())
                })
                .unwrap_or("Flutter app")
                .into(),
            providers,
        };
        Ok((
            Self {
                vm,
                isolate: isolate.into(),
                preferences,
                databases: tokio::sync::Mutex::new(databases),
                network,
            },
            hello,
        ))
    }

    pub async fn refresh_drift(&self) -> Result<Vec<Provider>, String> {
        // Serialize refreshes so an older registry snapshot cannot replace a newer one.
        let mut databases = self.databases.lock().await;
        let details = self
            .vm
            .call("getIsolate", json!({"isolateId":self.isolate}))
            .await?;
        let (providers, detected) = detect_drift(&self.vm, &self.isolate, &details).await?;
        *databases = detected;
        Ok(providers)
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        tokio::time::timeout(Duration::from_secs(20), self.dispatch(method, params))
            .await
            .map_err(|_| format!("{method} timed out; check whether the app is paused"))?
    }
    async fn dispatch(&self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "events" => match &self.network {
                Some(network) => network.events(params["after"].as_u64().unwrap_or(0)).await,
                None => Ok(json!({"cursor":0,"events":[],"pending":[],"dropped":0})),
            },
            "body.read" => self.network.as_ref().ok_or("HTTP profiling unavailable")?.read(&params).await,
            "detach" => {
                if let Some(network) = &self.network { network.close().await?; }
                Ok(json!({}))
            }
            _ if method.starts_with("preferences.") => {
                let library = self.preferences.as_ref().ok_or("This SharedPreferences version has no built-in DevTools helper")?;
                preferences::call(&self.vm, &self.isolate, library, method, &params).await
            }
            _ if method.starts_with("drift.") => {
                let id = self.databases.lock().await.get(params["provider"].as_str().unwrap_or_default()).copied().ok_or("Refresh databases and select an open Drift database")?;
                drift::Database::new(&self.vm, &self.isolate, id).call(method, &params).await
            }
            _ => Err("This optional feature needs flutter_inspector_bridge + flutter_inspector_dio in the debug app".into()),
        }
    }
}

async fn detect_drift(
    vm: &VmClient,
    isolate: &str,
    details: &Value,
) -> Result<(Vec<Provider>, HashMap<String, i64>), String> {
    let mut databases = HashMap::new();
    let mut providers = Vec::new();
    if extension(details, "ext.drift.database") {
        let lib = library(details, DRIFT).ok_or("Drift debug registry is unavailable")?;
        let result = vm.evaluate_string(isolate, &lib, "jsonEncode(TrackedDatabase.all.map((d) => {'id': d.id, 'name': d.database.runtimeType.toString()}).toList())").await?;
        let rows: Vec<Value> = serde_json::from_str(&result).map_err(|e| e.to_string())?;
        for row in rows {
            let id = row["id"].as_i64().ok_or("Invalid Drift database id")?;
            let provider = format!("native.drift.{id}");
            providers.push(Provider {
                id: provider.clone(),
                kind: "drift".into(),
                name: row["name"].as_str().unwrap_or("Drift").into(),
            });
            databases.insert(provider, id);
        }
    }
    Ok((providers, databases))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dart_literals_cannot_interpolate_or_end_the_expression() {
        assert_eq!(
            dart_string("a${secret}\n\"\\b"),
            "\"a\\${secret}\\n\\\"\\\\b\""
        );
    }
}
