use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const PROTOCOL_MAJOR: u32 = 1;
pub const BODY_LIMIT: usize = 20 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub kind: String,
    pub name: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub protocol: u32,
    pub session_id: String,
    pub name: String,
    pub providers: Vec<Provider>,
}
impl Hello {
    pub fn validate(&self) -> Result<(), String> {
        if self.protocol != PROTOCOL_MAJOR {
            return Err(format!(
                "Bridge protocol {} is incompatible with desktop protocol {PROTOCOL_MAJOR}",
                self.protocol
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterceptRule {
    pub enabled: bool,
    pub host: String,
    pub path: String,
    pub method: String,
    pub request: bool,
    pub response: bool,
    pub hold_seconds: u32,
}
impl Default for InterceptRule {
    fn default() -> Self {
        Self {
            enabled: false,
            host: String::new(),
            path: String::new(),
            method: String::new(),
            request: true,
            response: false,
            hold_seconds: 120,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub id: String,
    pub direction: String,
    pub expires_at: i64,
    pub exchange: crate::Exchange,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeEvent {
    pub sequence: u64,
    pub kind: String,
    pub data: Value,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EventPage {
    pub cursor: u64,
    pub dropped: u64,
    pub events: Vec<BridgeEvent>,
    #[serde(default)]
    pub pending: Vec<Pending>,
}
