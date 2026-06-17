//! Optional LLM enrichment client + job handlers (PRD §8.5, §8.11).
//!
//! Off by default: [`LlmClient::from_config`] returns `None` unless
//! `llm.enabled` plus a model are configured, and the worker only
//! constructs it when `enrichment.enabled` — with everything off, no LLM
//! client ever exists (AC1). Failures are returned as errors; the job layer
//! in `enrich` owns retry/backoff (AC2). All handlers are idempotent:
//! metadata writes overwrite the same keys, relation edges hit the unique
//! edge index.
//!
//! Provider dispatch is an enum, not a trait object: `complete()` differs
//! enough between OpenAI-compatible (chat completions) and Anthropic/Gemini
//! (native Messages / generateContent) that a shared trait would just be a
//! thin wrapper over a match anyway.

use anyhow::{Context, Result};
use async_openai::config::OpenAIConfig;
use async_openai::types::{
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
    CreateChatCompletionRequestArgs,
};
use serde::Deserialize;
use serde_json::json;
use tracing::debug;

use crate::config::LlmConfig;
use crate::graph;
use crate::model::{EdgeType, JobType};
use crate::store::Store;

/// Min content length worth summarizing (also gates compression — short
/// memories aren't worth the round-trip either way).
pub(crate) const SUMMARIZE_MIN_CHARS: usize = 280;
/// Neighbour candidates offered to the relation extractor.
const RELATION_CANDIDATES: usize = 5;

const ANTHROPIC_DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const ANTHROPIC_DEFAULT_MODEL: &str = "claude-opus-4-8";

const GEMINI_DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";

const OLLAMA_DEFAULT_BASE_URL: &str = "http://localhost:11434/v1";
const LMSTUDIO_DEFAULT_BASE_URL: &str = "http://localhost:1234/v1";
const GPT4ALL_DEFAULT_BASE_URL: &str = "http://localhost:4891/v1";
const OPENAI_DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

enum Backend {
    /// OpenAI and OpenAI-compatible servers (Ollama, LM Studio, GPT4All).
    OpenAiCompat {
        client: async_openai::Client<OpenAIConfig>,
        model: String,
    },
    Anthropic {
        http: reqwest::Client,
        base_url: String,
        api_key: String,
        model: String,
        max_tokens: u32,
    },
    Gemini {
        http: reqwest::Client,
        base_url: String,
        api_key: String,
        model: String,
    },
}

pub struct LlmClient {
    backend: Backend,
}

impl LlmClient {
    /// `None` unless `llm.enabled` and a model is configured. Endpoint
    /// (`base_url`) falls back to a sane per-provider default so users only
    /// need to set `model` (and `api_key` for hosted providers).
    pub fn from_config(cfg: &LlmConfig) -> Option<Self> {
        if !cfg.enabled {
            return None;
        }
        let model = cfg.model.as_deref()?.trim();
        if model.is_empty() {
            return None;
        }

        let backend = match cfg.provider.as_str() {
            "anthropic" => {
                let api_key = cfg.api_key.as_deref()?.trim();
                if api_key.is_empty() {
                    return None;
                }
                Backend::Anthropic {
                    http: reqwest::Client::new(),
                    base_url: non_empty(&cfg.base_url).unwrap_or(ANTHROPIC_DEFAULT_BASE_URL).to_string(),
                    api_key: api_key.to_string(),
                    model: non_empty_owned(model, ANTHROPIC_DEFAULT_MODEL),
                    max_tokens: cfg.max_generation_tokens,
                }
            }
            "gemini" => {
                let api_key = cfg.api_key.as_deref()?.trim();
                if api_key.is_empty() {
                    return None;
                }
                Backend::Gemini {
                    http: reqwest::Client::new(),
                    base_url: non_empty(&cfg.base_url).unwrap_or(GEMINI_DEFAULT_BASE_URL).to_string(),
                    api_key: api_key.to_string(),
                    model: model.to_string(),
                }
            }
            provider => {
                let default_base = match provider {
                    "ollama" => OLLAMA_DEFAULT_BASE_URL,
                    "lmstudio" => LMSTUDIO_DEFAULT_BASE_URL,
                    "gpt4all" => GPT4ALL_DEFAULT_BASE_URL,
                    _ => OPENAI_DEFAULT_BASE_URL, // "openai" and anything unrecognized
                };
                let endpoint = non_empty(&cfg.base_url).unwrap_or(default_base);
                let oai = OpenAIConfig::new()
                    .with_api_base(endpoint)
                    .with_api_key(cfg.api_key.clone().unwrap_or_default());
                Backend::OpenAiCompat {
                    client: async_openai::Client::with_config(oai),
                    model: model.to_string(),
                }
            }
        };

        Some(Self { backend })
    }

    /// One chat completion. No internal retries — the job layer retries.
    pub async fn complete(&self, system: &str, user: &str) -> Result<String> {
        match &self.backend {
            Backend::OpenAiCompat { client, model } => complete_openai_compat(client, model, system, user).await,
            Backend::Anthropic { http, base_url, api_key, model, max_tokens } => {
                complete_anthropic(http, base_url, api_key, model, *max_tokens, system, user).await
            }
            Backend::Gemini { http, base_url, api_key, model } => {
                complete_gemini(http, base_url, api_key, model, system, user).await
            }
        }
    }
}

fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn non_empty_owned(s: &str, default: &str) -> String {
    if s.is_empty() { default.to_string() } else { s.to_string() }
}

async fn complete_openai_compat(
    client: &async_openai::Client<OpenAIConfig>,
    model: &str,
    system: &str,
    user: &str,
) -> Result<String> {
    let request = CreateChatCompletionRequestArgs::default()
        .model(model)
        .temperature(0.0)
        .max_tokens(512u32)
        .messages([
            ChatCompletionRequestSystemMessageArgs::default()
                .content(system)
                .build()?
                .into(),
            ChatCompletionRequestUserMessageArgs::default()
                .content(user)
                .build()?
                .into(),
        ])
        .build()?;

    let response = client.chat().create(request).await.context("LLM request failed")?;
    response
        .choices
        .first()
        .and_then(|c| c.message.content.clone())
        .context("LLM reply had no content")
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicBlock>,
}

#[derive(Deserialize)]
struct AnthropicBlock {
    #[serde(default)]
    text: Option<String>,
}

async fn complete_anthropic(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    max_tokens: u32,
    system: &str,
    user: &str,
) -> Result<String> {
    let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
    let body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": [{"role": "user", "content": user}],
    });

    let resp = http
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .context("Anthropic request failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Anthropic API error {status}: {text:.300}");
    }

    let parsed: AnthropicResponse = resp.json().await.context("invalid Anthropic response")?;
    parsed
        .content
        .into_iter()
        .find_map(|b| b.text)
        .context("Anthropic reply had no text content")
}

#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
}

#[derive(Deserialize)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
}

#[derive(Deserialize)]
struct GeminiPart {
    #[serde(default)]
    text: Option<String>,
}

async fn complete_gemini(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
) -> Result<String> {
    let url = format!(
        "{}/v1beta/models/{}:generateContent?key={}",
        base_url.trim_end_matches('/'),
        model,
        api_key
    );
    let body = json!({
        "contents": [{"role": "user", "parts": [{"text": user}]}],
        "systemInstruction": {"parts": [{"text": system}]},
    });

    let resp = http.post(&url).json(&body).send().await.context("Gemini request failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Gemini API error {status}: {text:.300}");
    }

    let parsed: GeminiResponse = resp.json().await.context("invalid Gemini response")?;
    parsed
        .candidates
        .into_iter()
        .find_map(|c| c.content.parts.into_iter().find_map(|p| p.text))
        .context("Gemini reply had no text content")
}

/// Parse a JSON reply, tolerating the ``` fences local models love.
fn parse_json_reply<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T> {
    let trimmed = raw.trim();
    let inner = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.strip_suffix("```").unwrap_or(s))
        .unwrap_or(trimmed)
        .trim();
    serde_json::from_str(inner).with_context(|| format!("unparseable LLM reply: {raw:.200}"))
}

// ---------------------------------------------------------------------------
// Job dispatch
// ---------------------------------------------------------------------------

/// Execute one LLM job. Called by the enrichment worker; errors bubble to
/// its retry/backoff handling.
pub async fn run_job(
    store: &mut Store,
    client: &LlmClient,
    job_type: &JobType,
    memory_id: &str,
) -> Result<()> {
    // Memory may have been deleted since enqueue (FK cascade usually removes
    // the job, but be defensive).
    let Some(memory) = store.get_memory(memory_id)? else {
        return Ok(());
    };

    match job_type {
        JobType::Summarize => summarize(store, client, &memory).await,
        JobType::ExtractEntities => extract_entities(store, client, &memory).await,
        JobType::ExtractRelations => extract_relations(store, client, &memory).await,
        JobType::ScoreImportance => score_importance(store, client, &memory).await,
        JobType::ExtractCompress => extract_compress(store, client, &memory).await,
        JobType::ComputeEdges => unreachable!("compute_edges is not an LLM job"),
    }
}

/// Caveman-only fallback for a `ExtractCompress` job when no LLM client is
/// configured at all — never fails the job, mirroring how compression
/// degrades gracefully rather than blocking on enrichment availability.
pub(crate) fn compress_caveman_fallback(store: &mut Store, memory_id: &str) -> Result<()> {
    let Some(m) = store.get_memory(memory_id)? else {
        return Ok(());
    };
    if m.content.len() < SUMMARIZE_MIN_CHARS {
        return Ok(());
    }
    let compressed = crate::compress::compress(&m.content);
    store.set_compressed_content(&m.id, &compressed, "caveman")?;
    tracing::warn!(
        memory_id = %m.id,
        "semantic compression unavailable (no LLM configured); used caveman fallback"
    );
    Ok(())
}

async fn summarize(store: &mut Store, client: &LlmClient, m: &crate::model::Memory) -> Result<()> {
    if m.content.len() < SUMMARIZE_MIN_CHARS {
        return Ok(()); // nothing to compress
    }
    let summary = client
        .complete(
            "You compress notes for a developer's memory store. Reply with a 1-2 sentence \
             summary of the user's text. Plain text, no preamble.",
            &m.content,
        )
        .await?;
    let summary = summary.trim();
    if summary.is_empty() {
        anyhow::bail!("empty summary");
    }
    store.merge_metadata(&m.id, &serde_json::json!({ "summary": summary }))?;
    debug!(memory_id = %m.id, "summary stored");
    Ok(())
}

/// Token-reduced retrievable rewrite, cached for context injection only.
/// Unlike `summarize` (lossy, display-only), this must preserve everything
/// needed to find the memory again — recall/FTS/vector search never read it
/// (see `Store::get_compressed_content`).
async fn extract_compress(store: &mut Store, client: &LlmClient, m: &crate::model::Memory) -> Result<()> {
    if m.content.len() < SUMMARIZE_MIN_CHARS {
        return Ok(()); // nothing worth compressing
    }
    let raw = client
        .complete(
            "Rewrite the text as densely as possible. Preserve every fact, identifier, and \
             detail needed to find it again via search. No commentary, no preamble, no \
             summary framing. Output only the rewritten text: the shortest version that loses \
             no retrievable information.",
            &m.content,
        )
        .await?;
    let extracted = raw.trim();
    if extracted.is_empty() {
        anyhow::bail!("empty extract_compress reply");
    }
    let compressed = crate::compress::compress(extracted);
    store.set_compressed_content(&m.id, &compressed, "semantic")?;
    debug!(memory_id = %m.id, "semantic compression stored");
    Ok(())
}

async fn extract_entities(store: &mut Store, client: &LlmClient, m: &crate::model::Memory) -> Result<()> {
    let raw = client
        .complete(
            "Extract named entities from the text: technologies, libraries, people, projects, \
             file paths, error codes. Reply with a JSON array of lowercase strings, at most 10. \
             JSON only.",
            &m.content,
        )
        .await?;
    let entities: Vec<String> = parse_json_reply(&raw)?;
    if entities.is_empty() {
        return Ok(());
    }
    // Entities also union into tags so tag_overlap edges pick them up.
    store.merge_metadata(
        &m.id,
        &serde_json::json!({ "entities": entities, "tags": entities }),
    )?;
    crate::enrich::enqueue_compute_edges(store, &m.id)?; // idempotent recompute
    debug!(memory_id = %m.id, n = entities.len(), "entities stored");
    Ok(())
}

#[derive(Deserialize)]
struct RelationReply {
    index: usize,
    predicate: String,
}

async fn extract_relations(store: &mut Store, client: &LlmClient, m: &crate::model::Memory) -> Result<()> {
    // Grounded design: candidates are real nearest-neighbour memories; the
    // LLM only labels which are related and how. No dangling entity nodes.
    let candidates = graph::nearest_neighbors(store, &m.id, RELATION_CANDIDATES)?;
    if candidates.is_empty() {
        return Ok(()); // FTS-only mode or empty store
    }

    let mut user = format!("NEW: {}\n\nCANDIDATES:\n", m.content);
    for (i, c) in candidates.iter().enumerate() {
        user.push_str(&format!("{}. {}\n", i + 1, c.content));
    }

    let raw = client
        .complete(
            "You identify relations between a NEW memory and numbered CANDIDATE memories. \
             Reply with a JSON array of {\"index\": <candidate number>, \"predicate\": \
             \"<short verb phrase, e.g. supersedes, caused by, depends on>\"} containing only \
             candidates truly related to NEW. Empty array if none. JSON only.",
            &user,
        )
        .await?;
    let relations: Vec<RelationReply> = parse_json_reply(&raw)?;

    for rel in relations {
        let Some(target) = candidates.get(rel.index.wrapping_sub(1)) else {
            continue; // hallucinated index — skip, don't fail the job
        };
        let predicate = rel.predicate.trim();
        if predicate.is_empty() {
            continue;
        }
        store.create_edge(&m.id, &target.id, EdgeType::Relation, Some(predicate), 0.8)?;
    }
    debug!(memory_id = %m.id, "relations processed");
    Ok(())
}

async fn score_importance(store: &mut Store, client: &LlmClient, m: &crate::model::Memory) -> Result<()> {
    let raw = client
        .complete(
            "Rate how valuable this memory is to keep long-term for its project: 0.0 = trivial \
             noise, 0.5 = useful, 1.0 = critical decision or hard-won fact. Reply with a single \
             number between 0 and 1. Number only.",
            &m.content,
        )
        .await?;
    let score: f64 = raw
        .trim()
        .trim_matches('`')
        .parse()
        .with_context(|| format!("unparseable importance score: {raw:.50}"))?;
    store.set_importance(&m.id, score)?;
    debug!(memory_id = %m.id, score, "importance updated");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_config_none_unless_fully_configured() {
        // All defaults (disabled) → None (PRD §8.11 AC1).
        assert!(LlmClient::from_config(&LlmConfig::default()).is_none());

        // Enabled but no model → None.
        let cfg = LlmConfig { enabled: true, ..Default::default() };
        assert!(LlmClient::from_config(&cfg).is_none());

        // OpenAI-compat (ollama default provider): model set → Some, even
        // without an explicit base_url (falls back to provider default).
        let cfg = LlmConfig {
            enabled: true,
            model: Some("llama3.2".into()),
            ..Default::default()
        };
        assert!(LlmClient::from_config(&cfg).is_some());

        // Anthropic: needs an api_key too.
        let cfg = LlmConfig {
            enabled: true,
            provider: "anthropic".into(),
            model: Some("claude-opus-4-8".into()),
            ..Default::default()
        };
        assert!(LlmClient::from_config(&cfg).is_none());

        let cfg = LlmConfig {
            enabled: true,
            provider: "anthropic".into(),
            model: Some("claude-opus-4-8".into()),
            api_key: Some("sk-ant-test".into()),
            ..Default::default()
        };
        assert!(LlmClient::from_config(&cfg).is_some());

        // Gemini: same shape as Anthropic — needs api_key.
        let cfg = LlmConfig {
            enabled: true,
            provider: "gemini".into(),
            model: Some("gemini-2.0-flash".into()),
            api_key: Some("AIza-test".into()),
            ..Default::default()
        };
        assert!(LlmClient::from_config(&cfg).is_some());
    }

    #[test]
    fn parse_json_reply_strips_fences() {
        let plain: Vec<String> = parse_json_reply(r#"["a","b"]"#).unwrap();
        assert_eq!(plain, vec!["a", "b"]);

        let fenced: Vec<String> = parse_json_reply("```json\n[\"x\"]\n```").unwrap();
        assert_eq!(fenced, vec!["x"]);

        let bare_fence: Vec<String> = parse_json_reply("```\n[]\n```").unwrap();
        assert!(bare_fence.is_empty());

        assert!(parse_json_reply::<Vec<String>>("not json").is_err());
    }
}
