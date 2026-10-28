use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Body {
    pub id: String,
    pub size: u64,
    #[serde(default = "yes")]
    pub complete: bool,
    #[serde(default)]
    pub content_type: String,
}
fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<Header>,
    pub body: Option<Body>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub status: u16,
    #[serde(default)]
    pub headers: Vec<Header>,
    pub body: Option<Body>,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeState {
    #[default]
    Pending,
    HeldRequest,
    HeldResponse,
    Complete,
    Error,
    Dropped,
    Cancelled,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Exchange {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeater_id: Option<String>,
    pub session_id: String,
    pub client_id: String,
    pub started_at: i64,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub source: String,
    pub request: Request,
    pub response: Option<Response>,
    #[serde(default)]
    pub state: ExchangeState,
    pub error: Option<String>,
    #[serde(default)]
    pub pinned: bool,
}
impl Exchange {
    pub fn matches(&self, query: &str) -> bool {
        query.split_whitespace().all(|term| {
            let term = term.to_lowercase();
            if let Some(method) = term.strip_prefix("m:") {
                self.request.method.eq_ignore_ascii_case(method)
            } else if let Some(status) = term.strip_prefix("s:") {
                self.response
                    .as_ref()
                    .is_some_and(|r| r.status.to_string().starts_with(status))
            } else {
                self.request.url.to_lowercase().contains(&term)
                    || self.request.method.to_lowercase().contains(&term)
                    || self.client_id.to_lowercase().contains(&term)
            }
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub id: String,
    pub name: String,
    pub method: String,
    pub url: String,
    pub headers: Vec<Header>,
    pub body: String,
    #[serde(default)]
    pub body_base64: bool,
    #[serde(default)]
    pub via_app: bool,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub follow_redirects: bool,
}
impl Default for Draft {
    fn default() -> Self {
        Self {
            id: "scratch".into(),
            name: "Untitled".into(),
            method: "GET".into(),
            url: "https://example.com/".into(),
            headers: vec![],
            body: String::new(),
            body_base64: false,
            via_app: false,
            client_id: String::new(),
            follow_redirects: false,
        }
    }
}
pub fn headers_text(headers: &[Header]) -> String {
    headers
        .iter()
        .map(|h| format!("{}: {}", h.name, h.value))
        .collect::<Vec<_>>()
        .join("\n")
}
pub fn parse_headers(text: &str) -> Result<Vec<Header>, String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (name, value) = line.split_once(':').ok_or("Each header needs a colon")?;
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_\x60|~".contains(&b))
            {
                return Err(format!("Invalid header name: {name}"));
            }
            if value.contains(['\r', '\0']) {
                return Err("Header contains a control character".into());
            }
            Ok(Header {
                name: name.into(),
                value: value.trim().into(),
            })
        })
        .collect()
}
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
pub fn curl(draft: &Draft) -> String {
    let mut parts = vec![
        "curl".into(),
        "-X".into(),
        shell_quote(&draft.method),
        shell_quote(&draft.url),
    ];
    if draft.follow_redirects {
        parts.push("--location".into());
    }
    for h in &draft.headers {
        parts.extend([
            "-H".into(),
            shell_quote(&format!("{}: {}", h.name, h.value)),
        ]);
    }
    if !draft.body.is_empty() && !draft.body_base64 {
        parts.extend(["--data-raw".into(), shell_quote(&draft.body)]);
    }
    parts.join(" ")
}
