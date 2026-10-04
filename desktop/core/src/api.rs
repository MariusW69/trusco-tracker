use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::Block;
use crate::store::Account;

#[derive(Debug)]
pub enum ApiError {
    /// Device token missing, revoked, or the user left the organization.
    Unauthorized,
    InvalidCode,
    Server(u16, String),
    Network(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Unauthorized => write!(f, "This device is no longer connected to TrusCo"),
            ApiError::InvalidCode => write!(f, "That code is invalid or has expired"),
            ApiError::Server(code, msg) => write!(f, "TrusCo returned {code}: {msg}"),
            ApiError::Network(msg) => write!(f, "Could not reach TrusCo: {msg}"),
        }
    }
}

#[derive(Serialize)]
struct PairRequest<'a> {
    code: &'a str,
    device_name: &'a str,
    platform: &'a str,
    app_version: &'a str,
}

#[derive(Deserialize)]
struct PairResponse {
    device_id: String,
    device_token: String,
    organization_id: String,
    organization_name: Option<String>,
    user_name: Option<String>,
}

#[derive(Deserialize)]
struct MeResponse {
    device_id: String,
    device_name: Option<String>,
    organization_id: String,
    organization_name: Option<String>,
    user_name: Option<String>,
}

#[derive(Serialize)]
pub struct BlockPayload {
    pub external_id: String,
    pub started_at: String,
    pub ended_at: String,
    pub active_seconds: u64,
    pub app_name: String,
    pub app_bundle: Option<String>,
    pub window_title: String,
    pub document_path: Option<String>,
    pub document_pages: Option<u32>,
}

impl From<&Block> for BlockPayload {
    fn from(b: &Block) -> Self {
        Self {
            external_id: b.id.clone(),
            started_at: b.started_at.to_rfc3339(),
            ended_at: b.ended_at.to_rfc3339(),
            active_seconds: b.active_seconds(),
            app_name: b.app_name.clone(),
            app_bundle: b.app_bundle.clone(),
            window_title: b.window_title.clone(),
            document_path: b.document_path.clone(),
            document_pages: b.document_pages,
        }
    }
}

#[derive(Serialize)]
struct SyncRequest<'a> {
    blocks: &'a [BlockPayload],
    app_version: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct SyncResult {
    pub external_id: String,
    pub status: Option<String>,
    pub matter_label: Option<String>,
    pub match_reason: Option<String>,
}

#[derive(Deserialize)]
struct SyncResponse {
    results: Vec<SyncResult>,
}

pub struct Client {
    base: String,
    http: reqwest::blocking::Client,
}

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// reqwest's Display hides the cause ("error sending request"); include the whole chain.
fn network(e: reqwest::Error) -> ApiError {
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    ApiError::Network(msg)
}

pub fn platform_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "other"
    }
}

impl Client {
    pub fn new(server_url: &str) -> Self {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(format!("TrusCoTracker/{APP_VERSION} ({})", platform_name()))
            .build()
            .expect("http client");
        Self { base: server_url.trim_end_matches('/').to_string(), http }
    }

    /// Exchange the 8-character code shown in TrusCo for a long-lived device token.
    pub fn pair(&self, code: &str, device_name: &str) -> Result<(String, Account), ApiError> {
        let res = self
            .http
            .post(format!("{}/api/public/tracker/pair", self.base))
            .json(&PairRequest { code, device_name, platform: platform_name(), app_version: APP_VERSION })
            .send()
            .map_err(network)?;
        match res.status().as_u16() {
            200 => {
                let p: PairResponse = res.json().map_err(|e| ApiError::Server(200, e.to_string()))?;
                let account = Account {
                    device_id: p.device_id,
                    device_name: device_name.to_string(),
                    organization_id: p.organization_id,
                    organization_name: p.organization_name.unwrap_or_default(),
                    user_name: p.user_name.unwrap_or_default(),
                };
                Ok((p.device_token, account))
            }
            400 | 401 => Err(ApiError::InvalidCode),
            s => Err(ApiError::Server(s, res.text().unwrap_or_default())),
        }
    }

    pub fn me(&self, token: &str) -> Result<Account, ApiError> {
        let res = self
            .http
            .get(format!("{}/api/public/tracker/me", self.base))
            .bearer_auth(token)
            .send()
            .map_err(network)?;
        match res.status().as_u16() {
            200 => {
                let m: MeResponse = res.json().map_err(|e| ApiError::Server(200, e.to_string()))?;
                Ok(Account {
                    device_id: m.device_id,
                    device_name: m.device_name.unwrap_or_default(),
                    organization_id: m.organization_id,
                    organization_name: m.organization_name.unwrap_or_default(),
                    user_name: m.user_name.unwrap_or_default(),
                })
            }
            401 => Err(ApiError::Unauthorized),
            s => Err(ApiError::Server(s, res.text().unwrap_or_default())),
        }
    }

    pub fn sync(&self, token: &str, blocks: &[BlockPayload]) -> Result<Vec<SyncResult>, ApiError> {
        let res = self
            .http
            .post(format!("{}/api/public/tracker/sync", self.base))
            .bearer_auth(token)
            .json(&SyncRequest { blocks, app_version: APP_VERSION })
            .send()
            .map_err(network)?;
        match res.status().as_u16() {
            200 => res.json::<SyncResponse>().map(|r| r.results).map_err(|e| ApiError::Server(200, e.to_string())),
            401 => Err(ApiError::Unauthorized),
            s => Err(ApiError::Server(s, res.text().unwrap_or_default())),
        }
    }
}
