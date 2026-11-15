use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::intel;
use crate::models::{
    mask_key, Event, EventDto, ModelInfo, ProviderDto, RouteTarget, RouteTargetDto, StateDto,
    Status, VirtualModel,
};

pub const EVENT_CAP: usize = 100;
pub const DATA_FILE_ENV: &str = "AIROUTER_DATA_FILE";
const DEFAULT_DATA_FILE: &str = "providers.json";

/// Minimal persisted representation of a provider (runtime state is re-derived
/// on startup by re-checking each provider).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model_override: Option<String>,
}

/// On-disk shape of `providers.json`. Root is an object; legacy files that
/// were a bare provider array are still accepted on load.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistFile {
    #[serde(default)]
    pub fast_override: Option<String>,
    #[serde(default)]
    pub think_override: Option<String>,
    #[serde(default)]
    pub providers: Vec<PersistedProvider>,
}

#[derive(Clone, Debug)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub base_url: String, // normalized, no trailing slash
    pub api_key: String,
    pub v1_prefix: String,        // "" or "/v1" — which path worked for /models
    pub status: Status,
    pub last_error: Option<String>,
    pub last_latency_ms: Option<u64>,
    pub models: Vec<ModelInfo>,
    pub benchmark: Option<crate::models::BenchResult>,
    pub model_override: Option<String>,
    pub in_flight: bool,
}

impl Provider {
    pub fn new(
        id: String,
        name: String,
        base_url: String,
        api_key: String,
        model_override: Option<String>,
    ) -> Self {
        Self {
            id,
            name,
            base_url,
            api_key,
            v1_prefix: String::new(),
            status: Status::Checking,
            last_error: None,
            last_latency_ms: None,
            models: Vec::new(),
            benchmark: None,
            model_override,
            in_flight: false,
        }
    }
}

/// A routing candidate: a specific model served by a specific healthy provider,
/// already ordered by preference for a given virtual model.
#[derive(Clone, Debug)]
pub struct RouteCandidate {
    pub provider_id: String,
    pub provider_name: String,
    pub base_url: String,
    pub v1_prefix: String,
    pub api_key: String,
    pub model: String,
    pub latency_ms: Option<u64>,
    pub score: Option<u64>,
    pub manual: bool,
}

pub struct AppState {
    pub providers: RwLock<HashMap<String, Provider>>,
    pub events: RwLock<VecDeque<Event>>,
    pub next_id: AtomicU64,
    pub benchmark_running: AtomicBool,
    pub fast_override: RwLock<Option<String>>,
    pub think_override: RwLock<Option<String>>,
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            providers: RwLock::new(HashMap::new()),
            events: RwLock::new(VecDeque::with_capacity(EVENT_CAP)),
            next_id: AtomicU64::new(1),
            benchmark_running: AtomicBool::new(false),
            fast_override: RwLock::new(None),
            think_override: RwLock::new(None),
            http,
        }
    }

    pub fn new_provider_id(&self) -> String {
        let n = self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("p{n}")
    }

    fn data_file() -> String {
        std::env::var(DATA_FILE_ENV).unwrap_or_else(|_| DEFAULT_DATA_FILE.to_string())
    }

    /// Persists providers + routing overrides to disk.
    pub async fn save(&self) {
        let path = Self::data_file();
        self.save_to(&path).await;
    }

    async fn save_to(&self, path: &str) {
        let (fast_override, think_override) = {
            let f = self.fast_override.read().await;
            let t = self.think_override.read().await;
            (f.clone(), t.clone())
        };

        let mut providers: Vec<PersistedProvider> = {
            let providers = self.providers.read().await;
            providers
                .values()
                .map(|p| PersistedProvider {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    base_url: p.base_url.clone(),
                    api_key: p.api_key.clone(),
                    model_override: p.model_override.clone(),
                })
                .collect()
        };
        providers.sort_by_key(|p| id_num(&p.id));

        let file = PersistFile {
            fast_override,
            think_override,
            providers,
        };

        let tmp = format!("{path}.tmp");
        if let Ok(json) = serde_json::to_string_pretty(&file) {
            let _ = std::fs::write(&tmp, json);
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    /// Loads persisted state (empty on missing/corrupt file).
    pub fn load_persisted() -> PersistFile {
        Self::load_from(&Self::data_file())
    }

    fn load_from(path: &str) -> PersistFile {
        match std::fs::read_to_string(path) {
            Ok(s) => match serde_json::from_str::<PersistFile>(&s) {
                Ok(file) => file,
                Err(_) => {
                    // Legacy format: a bare provider array (no overrides field).
                    let providers: Vec<PersistedProvider> =
                        serde_json::from_str(&s).unwrap_or_default();
                    PersistFile {
                        fast_override: None,
                        think_override: None,
                        providers,
                    }
                }
            },
            Err(_) => PersistFile::default(),
        }
    }

    /// Restores the id counter so newly added providers never collide with
    /// persisted ids (`p1`, `p2`, ...).
    pub fn bump_id_counter_after(&self, ids: &[String]) {
        let max = ids.iter().map(|s| id_num(s)).max().unwrap_or(0);
        let next = max.saturating_add(1);
        // never move the counter backwards
        self.next_id
            .fetch_max(next, std::sync::atomic::Ordering::Relaxed);
    }

    pub async fn push_event(&self, kind: &str, message: String) {
        let mut events = self.events.write().await;
        if events.len() == EVENT_CAP {
            events.pop_front();
        }
        events.push_back(Event {
            ts: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            kind: kind.to_string(),
            message,
        });
    }

    /// Ordered fallback candidates for a virtual model, honoring a manual
    /// override when set (and falling back to auto if the chosen model is not
    /// available on any healthy provider).
    pub async fn candidates_for(&self, vm: VirtualModel) -> Vec<RouteCandidate> {
        let manual = match vm {
            VirtualModel::Fast => self.fast_override.read().await.clone(),
            VirtualModel::Think => self.think_override.read().await.clone(),
        };

        let providers = self.providers.read().await;

        if let Some(ov) = manual {
            let mut list: Vec<RouteCandidate> = providers
                .values()
                .filter(|p| p.status == Status::Ok && p.models.iter().any(|m| m.id == ov))
                .map(|p| RouteCandidate {
                    provider_id: p.id.clone(),
                    provider_name: p.name.clone(),
                    base_url: p.base_url.clone(),
                    v1_prefix: p.v1_prefix.clone(),
                    api_key: p.api_key.clone(),
                    model: ov.clone(),
                    latency_ms: p.benchmark.as_ref().map(|b| b.latency_ms),
                    score: p.models.iter().find(|m| m.id == ov).map(|m| m.score),
                    manual: true,
                })
                .collect();

            if !list.is_empty() {
                match vm {
                    VirtualModel::Fast => list.sort_by_key(|c| c.latency_ms.unwrap_or(u64::MAX)),
                    VirtualModel::Think => {
                        list.sort_by_key(|c| std::cmp::Reverse(c.score.unwrap_or(0)))
                    }
                }
                return list;
            }
            // fall through to auto when the manual model is unavailable
        }

        let mut list: Vec<RouteCandidate> = Vec::new();
        for p in providers
            .values()
            .filter(|p| p.status == Status::Ok && !p.models.is_empty())
        {
            let (model, latency_ms, score) = match vm {
                VirtualModel::Fast => {
                    let model = match &p.benchmark {
                        Some(b) => b.model.clone(),
                        None => match intel::smallest_chat_model(&p.models) {
                            Some(m) => m,
                            None => continue,
                        },
                    };
                    (model, p.benchmark.as_ref().map(|b| b.latency_ms), None)
                }
                VirtualModel::Think => {
                    let Some(m) = p.models.iter().max_by_key(|m| m.score) else {
                        continue;
                    };
                    (m.id.clone(), None, Some(m.score))
                }
            };
            list.push(RouteCandidate {
                provider_id: p.id.clone(),
                provider_name: p.name.clone(),
                base_url: p.base_url.clone(),
                v1_prefix: p.v1_prefix.clone(),
                api_key: p.api_key.clone(),
                model,
                latency_ms,
                score,
                manual: false,
            });
        }

        match vm {
            VirtualModel::Fast => list.sort_by_key(|c| c.latency_ms.unwrap_or(u64::MAX)),
            VirtualModel::Think => list.sort_by_key(|c| std::cmp::Reverse(c.score.unwrap_or(0))),
        }
        list
    }

    /// Recomputes routing and returns the dashboard snapshot.
    pub async fn snapshot(&self) -> StateDto {
        let fast = self
            .candidates_for(VirtualModel::Fast)
            .await
            .into_iter()
            .next()
            .map(|c| make_route_target(VirtualModel::Fast, c));
        let think = self
            .candidates_for(VirtualModel::Think)
            .await
            .into_iter()
            .next()
            .map(|c| make_route_target(VirtualModel::Think, c));

        let (fast_override, think_override) = {
            let f = self.fast_override.read().await;
            let t = self.think_override.read().await;
            (f.clone(), t.clone())
        };

        let providers = self.providers.read().await;
        let mut provider_dtos: Vec<ProviderDto> = providers
            .values()
            .map(|p| ProviderDto {
                id: p.id.clone(),
                name: p.name.clone(),
                base_url: p.base_url.clone(),
                api_key_masked: mask_key(&p.api_key),
                status: p.status,
                last_error: p.last_error.clone(),
                last_latency_ms: p.last_latency_ms,
                models: p.models.iter().map(|m| m.id.clone()).collect(),
                model_override: p.model_override.clone(),
                benchmark_model: p.benchmark.as_ref().map(|b| b.model.clone()),
                benchmark_latency_ms: p.benchmark.as_ref().map(|b| b.latency_ms),
            })
            .collect();
        provider_dtos.sort_by(|a, b| a.id.cmp(&b.id));

        let mut candidates: Vec<String> = providers
            .values()
            .flat_map(|p| p.models.iter().map(|m| m.id.clone()))
            .collect();
        candidates.sort();
        candidates.dedup();

        let events = self
            .events
            .read()
            .await
            .iter()
            .rev()
            .map(|e| EventDto {
                ts: e.ts,
                kind: e.kind.clone(),
                message: e.message.clone(),
            })
            .collect();

        StateDto {
            benchmark_running: self.benchmark_running.load(std::sync::atomic::Ordering::Relaxed),
            routing_fast: fast.map(route_target_dto),
            routing_think: think.map(route_target_dto),
            fast_override,
            think_override,
            candidates,
            providers: provider_dtos,
            events,
        }
    }
}

fn route_target_dto(t: RouteTarget) -> RouteTargetDto {
    RouteTargetDto {
        provider_id: t.provider_id,
        provider_name: t.provider_name,
        model: t.model,
        latency_ms: t.latency_ms,
        score: t.score,
        reason: t.reason,
    }
}

fn make_route_target(vm: VirtualModel, c: RouteCandidate) -> RouteTarget {
    let reason = if c.manual {
        "вручную".to_string()
    } else {
        match vm {
            VirtualModel::Fast => format!("быстрейший: {} мс", c.latency_ms.unwrap_or(0)),
            VirtualModel::Think => intel::rank_model(&c.model).reason,
        }
    };
    RouteTarget {
        provider_id: c.provider_id,
        provider_name: c.provider_name,
        model: c.model,
        latency_ms: c.latency_ms,
        score: c.score,
        reason,
    }
}

fn id_num(s: &str) -> u64 {
    s.trim_start_matches('p').parse::<u64>().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn persistence_roundtrip() {
        let dir = std::env::temp_dir().join(format!("airouter-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("providers.json");
        let path = path.to_str().unwrap().to_string();

        let state = AppState::new(reqwest::Client::new());
        *state.fast_override.write().await = Some("gpt-4o".to_string());
        state.providers.write().await.insert(
            "p1".into(),
            Provider::new(
                "p1".into(),
                "TestAPI".into(),
                "https://example.com/v1".into(),
                "sk-secret".into(),
                Some("gpt-4o-mini".into()),
            ),
        );
        state.save_to(&path).await;

        let loaded = AppState::load_from(&path);
        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.providers[0].id, "p1");
        assert_eq!(loaded.providers[0].name, "TestAPI");
        assert_eq!(loaded.providers[0].api_key, "sk-secret");
        assert_eq!(loaded.providers[0].model_override.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(loaded.fast_override.as_deref(), Some("gpt-4o"));
        assert_eq!(loaded.think_override, None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}