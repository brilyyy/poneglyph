//! Optional LLM enrichment client + job handlers (PRD §8.5, §8.11).
//!
//! Off by default: [`LlmClient::from_config`] returns `None` unless
//! `llm.enabled` plus an endpoint and model are configured, and the worker
//! only constructs it when `enrichment.enabled` — with everything off, no
//! LLM client ever exists (AC1). Failures are returned as errors; the job
//! layer in `enrich` owns retry/backoff (AC2). All handlers are idempotent:
//! metadata writes overwrite the same keys, relation edges hit the unique
//! edge index.

use anyhow::{Context, Result};
use async_openai::config::OpenAIConfig;
use async_openai::types::{
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
    CreateChatCompletionRequestArgs,
};
use serde::Deserialize;
use tracing::debug;

use crate::config::LlmConfig;
use crate::graph;
use crate::model::{EdgeType, JobType};
use crate::store::Store;

/// Min content length worth summarizing.
const SUMMARIZE_MIN_CHARS: usize = 280;
/// Neighbour candidates offered to the relation extractor.
const RELATION_CANDIDATES: usize = 5;

pub struct LlmClient {
    client: async_openai::Client<OpenAIConfig>,
    model: String,
}

impl LlmClient {
    /// `None` unless llm.enabled and both endpoint and model are set.
    pub fn from_config(cfg: &LlmConfig) -> Option<Self> {
        if !cfg.enabled {
            return None;
        }
        let endpoint = cfg.endpoint.as_deref()?.trim();
        let model = cfg.model.as_deref()?.trim();
        if endpoint.is_empty() || model.is_empty() {
            return None;
        }
        let oai = OpenAIConfig::new()
            .with_api_base(endpoint)
            .with_api_key(cfg.api_key.clone().unwrap_or_default());
        Some(Self {
            client: async_openai::Client::with_config(oai),
            model: model.to_string(),
        })
    }

    /// One chat completion. No internal retries — the job layer retries.
    pub async fn complete(&self, system: &str, user: &str) -> Result<String> {
        let request = CreateChatCompletionRequestArgs::default()
            .model(&self.model)
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

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .context("LLM request failed")?;
        let content = response
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .context("LLM reply had no content")?;
        Ok(content)
    }
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
        JobType::ComputeEdges => unreachable!("compute_edges is not an LLM job"),
    }
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

        // Enabled but no endpoint/model → None.
        let cfg = LlmConfig { enabled: true, ..Default::default() };
        assert!(LlmClient::from_config(&cfg).is_none());

        let cfg = LlmConfig {
            enabled: true,
            endpoint: Some("http://localhost:11434/v1".into()),
            model: Some("llama3.2".into()),
            api_key: None,
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
