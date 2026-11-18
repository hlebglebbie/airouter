use serde::{Deserialize, Serialize};

/// One of the two virtual models exposed on the OpenAI-compatible endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VirtualModel {
    Fast,
    Think,
}

impl VirtualModel {
    /// Maps a client-supplied model marker (fast / think + common aliases).
    pub fn from_marker(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fast" | "gpt-fast" | "fast-model" => Some(VirtualModel::Fast),
            "think" | "gpt-think" | "think-model" => Some(VirtualModel::Think),
            _ => None,
        }
    }

    pub fn marker(&self) -> &'static str {
        match self {
            VirtualModel::Fast => "fast",
            VirtualModel::Think => "think",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Checking,
    Ok,
    Down,
}

#[derive(Debug, Deserialize)]
pub struct AddProviderRequest {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model_override: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProviderRequest {
    /// Empty string means "keep the existing key unchanged".
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    /// `None` or empty clears the override.
    pub model_override: Option<String>,
}

/// Sets manual model selection for the two virtual models. `None` = auto.
#[derive(Debug, Deserialize)]
pub struct RoutingUpdateRequest {
    pub fast: Option<String>,
    pub think: Option<String>,
}

/// Configures the Telegram bot token. Empty string clears the configuration.
#[derive(Debug, Deserialize)]
pub struct TelegramUpdateRequest {
    pub token: String,
}

// ---------- internal state types (also serialized into /api/state) ----------

#[derive(Clone, Debug)]
pub struct ModelInfo {
    pub id: String,
    pub score: u64,
}

#[derive(Clone, Debug)]
pub struct BenchResult {
    pub model: String,
    pub latency_ms: u64,
}

#[derive(Clone, Debug)]
pub struct RouteTarget {
    pub provider_id: String,
    pub provider_name: String,
    pub model: String,
    pub latency_ms: Option<u64>,
    pub score: Option<u64>,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub ts: u64,
    pub kind: String,
    pub message: String,
}

// ---------- /api/state DTOs ----------

#[derive(Debug, Serialize)]
pub struct RouteTargetDto {
    pub provider_id: String,
    pub provider_name: String,
    pub model: String,
    pub latency_ms: Option<u64>,
    pub score: Option<u64>,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct ProviderDto {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key_masked: String,
    pub status: Status,
    pub last_error: Option<String>,
    pub last_latency_ms: Option<u64>,
    pub models: Vec<String>,
    pub model_override: Option<String>,
    pub benchmark_model: Option<String>,
    pub benchmark_latency_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct EventDto {
    pub ts: u64,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct TelegramDto {
    pub configured: bool,
    pub token_masked: String,
}

#[derive(Debug, Serialize)]
pub struct StateDto {
    pub benchmark_running: bool,
    pub routing_fast: Option<RouteTargetDto>,
    pub routing_think: Option<RouteTargetDto>,
    pub fast_override: Option<String>,
    pub think_override: Option<String>,
    pub candidates: Vec<String>,
    pub providers: Vec<ProviderDto>,
    pub events: Vec<EventDto>,
    pub telegram: TelegramDto,
}

/// Masks an API key for display: `sk-…abcd`.
pub fn mask_key(key: &str) -> String {
    if key.is_empty() {
        return String::new();
    }
    let prefix = if key.len() >= 3 { &key[..3] } else { key };
    let tail: String = key.chars().rev().take(4).collect::<String>().chars().rev().collect();
    format!("{prefix}…{tail}")
}

/// Masks a Telegram bot token for display: `123456789:…abcd` (keeps the bot
/// id visible, hides the secret part).
pub fn mask_telegram_token(token: &str) -> String {
    match token.find(':') {
        Some(i) => {
            let (id, secret) = (&token[..i], &token[i + 1..]);
            let tail: String = secret
                .chars()
                .rev()
                .take(4)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            format!("{id}:…{tail}")
        }
        None => mask_key(token),
    }
}