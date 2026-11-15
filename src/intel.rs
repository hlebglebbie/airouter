use std::sync::LazyLock;

use regex::Regex;

use crate::models::ModelInfo;

/// Result of ranking a model by suspected parameter count.
#[derive(Clone, Debug)]
pub struct ModelRank {
    pub score: u64,
    pub reason: String,
}

/// MoE notation like `mixtral-8x7b`.
static RE_MOE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(\d+)\s*x\s*(\d+(?:\.\d+)?)\s*b\b").unwrap());
/// Explicit billions like `llama-3.1-70b`, `qwen2.5-72b`, `235b-a22b`.
static RE_B: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(\d+(?:\.\d+)?)\s*b\b").unwrap());
/// Explicit millions like `700m`.
static RE_M: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(\d+(?:\.\d+)?)\s*m\b").unwrap());

/// Keyword weights used when no explicit parameter count is present in the id.
/// Longest matching keyword wins (more specific); longer entry ties broken by
/// higher weight. Weight is a rough proxy for expected size (bigger = bigger).
static KEYWORDS: &[(&str, u64)] = &[
    ("gpt-5.2", 2000),
    ("gpt-5.1", 2000),
    ("gpt-5", 1500),
    ("gpt-4.1", 950),
    ("gpt-4o-mini", 60),
    ("gpt-4o", 900),
    ("gpt-4-turbo", 800),
    ("gpt-4", 800),
    ("gpt-3.5-turbo", 50),
    ("gpt-3.5", 50),
    ("o4-mini", 60),
    ("o3-mini", 55),
    ("o1-mini", 50),
    ("o1-pro", 2000),
    ("o1", 1500),
    ("o3", 2000),
    ("o4", 2000),
    ("claude-opus", 2000),
    ("opus", 2000),
    ("claude-sonnet", 800),
    ("sonnet-4.5", 900),
    ("sonnet-4", 850),
    ("sonnet", 800),
    ("claude-haiku", 100),
    ("haiku", 100),
    ("deepseek-r1", 671),
    ("deepseek-reasoner", 671),
    ("r1-0528", 671),
    ("r1", 671),
    ("grok-4", 1000),
    ("grok-3", 800),
    ("gemini-3-pro", 1700),
    ("gemini-2.5-pro", 1500),
    ("gemini-2.5-flash", 100),
    ("gemini-2.0-flash", 90),
    ("gemini-2.5", 1000),
    ("gemini-flash", 80),
    ("gemini-pro", 800),
    ("gemini-ultra", 1000),
    ("qwen-max", 800),
    ("qwen-plus", 200),
    ("qwen-turbo", 100),
    ("qwen2.5-72", 500),
    ("llama-4-maverick", 400),
    ("maverick", 400),
    ("llama-4-scout", 100),
    ("scout", 100),
    ("reasoner", 1500),
    ("ultra", 1000),
    ("max", 500),
    ("flash-lite", 200),
    ("plus", 200),
    ("flash", 100),
    ("mini", 50),
    ("turbo", 30),
    ("light", 30),
    ("nano", 10),
    ("lite", 10),
    ("small", 10),
    ("tiny", 5),
    ("smol", 5),
];

/// Substrings that mark a model as non-chat / not benchmarkable.
pub static DENYLIST: &[&str] = &[
    "embed",
    "text-embedding",
    "tts",
    "stt",
    "whisper",
    "audio",
    "speech",
    "transcri",
    "ocr",
    "dall-e",
    "dalle",
    "image",
    "vision",
    "moderation",
    "moder",
    "rerank",
    "guard",
];

fn keyword_score(lower: &str) -> Option<(u64, &'static str)> {
    let mut best: Option<(usize, u64, &'static str)> = None; // (len, weight, kw)
    for &(kw, w) in KEYWORDS {
        if lower.contains(&kw) {
            let replace = match best {
                None => true,
                Some((len, bw, _)) => kw.len() > len || (kw.len() == len && w > bw),
            };
            if replace {
                best = Some((kw.len(), w, kw));
            }
        }
    }
    best.map(|(_, w, kw)| (w, kw))
}

/// Ranks a model id by suspected parameter count. Score is expressed in
/// millions of parameters (70b -> 70000), plus a human-readable reason.
pub fn rank_model(id: &str) -> ModelRank {
    // 1. MoE notation (8x7b) — total params approximated by product.
    if let Some(c) = RE_MOE.captures(id) {
        let a: u64 = c[1].parse().unwrap_or(1);
        let b: f64 = c[2].parse().unwrap_or(1.0);
        let score = (a as f64 * b * 1000.0) as u64;
        return ModelRank {
            score,
            reason: format!("явно (MoE): {a}x{b}B"),
        };
    }
    // 2. Explicit billions.
    if let Some(c) = RE_B.captures(id) {
        let n: f64 = c[1].parse().unwrap_or(0.0);
        let score = (n * 1000.0) as u64;
        return ModelRank {
            score,
            reason: format!("явно: {}B", c[1].to_string()),
        };
    }
    // 3. Explicit millions.
    if let Some(c) = RE_M.captures(id) {
        let n: u64 = c[1].parse().unwrap_or(0);
        return ModelRank {
            score: n,
            reason: format!("явно: {}M", c[1].to_string()),
        };
    }
    // 4. Keyword heuristics by name.
    if let Some((w, kw)) = keyword_score(&id.to_ascii_lowercase()) {
        return ModelRank {
            score: w,
            reason: format!("по имени: {kw}"),
        };
    }
    ModelRank {
        score: 0,
        reason: "нет данных".to_string(),
    }
}

/// Builds `ModelInfo` list for freshly parsed model ids, keeping stable order.
pub fn score_models(ids: Vec<String>) -> Vec<ModelInfo> {
    ids.into_iter()
        .map(|id| {
            let rank = rank_model(&id);
            ModelInfo {
                id,
                score: rank.score,
            }
        })
        .collect()
}

fn is_chat_model(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    !DENYLIST.iter().any(|d| lower.contains(d))
}

/// Candidate models for the speed benchmark: chat models only, smallest-first,
/// capped. Smallest-first because the fastest model is usually the smallest.
pub fn benchmark_shortlist(models: &[ModelInfo], cap: usize) -> Vec<String> {
    let mut ranked: Vec<&ModelInfo> = models.iter().filter(|m| is_chat_model(&m.id)).collect();
    ranked.sort_by_key(|m| (m.score, m.id.len()));
    ranked.into_iter().take(cap).map(|m| m.id.clone()).collect()
}

/// The smallest (likely fastest) chat model of a provider — used for the quick
/// one-word check without a full benchmark.
pub fn smallest_chat_model(models: &[ModelInfo]) -> Option<String> {
    models
        .iter()
        .filter(|m| is_chat_model(&m.id))
        .min_by_key(|m| (m.score, m.id.len()))
        .map(|m| m.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_sizes() {
        assert_eq!(rank_model("llama-3.1-70b").score, 70000);
        assert_eq!(rank_model("qwen2.5-72b-instruct").score, 72000);
        assert_eq!(rank_model("mixtral-8x7b").score, 56000);
        assert_eq!(rank_model("gryphe/mythomax-l2-13b").score, 13000);
        assert_eq!(rank_model("phi-3-700m").score, 700);
        // 235b-a22b takes the first (total) count.
        assert_eq!(rank_model("deepseek-ai/deepseek-v3-235b-a22b").score, 235000);
    }

    #[test]
    fn keyword_fallback() {
        assert!(rank_model("deepseek-r1").score > rank_model("gpt-4o-mini").score);
        assert!(rank_model("claude-opus-4").score > rank_model("claude-haiku-3").score);
        assert_eq!(rank_model("no-such-model").score, 0);
    }

    #[test]
    fn denylist() {
        let models = score_models(vec![
            "text-embedding-3-small".into(),
            "gpt-4o".into(),
            "dall-e-3".into(),
        ]);
        assert_eq!(benchmark_shortlist(&models, 10), vec!["gpt-4o".to_string()]);
    }

    #[test]
    fn shortlist_capped() {
        let ids: Vec<String> = (1..=10).map(|i| format!("model-{i}b")).collect();
        let models = score_models(ids);
        assert_eq!(benchmark_shortlist(&models, 6).len(), 6);
    }
}