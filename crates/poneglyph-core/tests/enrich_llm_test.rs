//! LLM enrichment integration tests (PRD §8.5, §8.11, §16).
//!
//! A tiny axum mock stands in for the OpenAI-compatible endpoint, so these
//! exercise the real async-openai wire path with zero network dependency.
//! Failure injection points the client at an unbound port.

use std::sync::Arc;

use axum::routing::post;
use axum::Json;
use serde_json::{json, Value};

use poneglyph_core::config::{EnrichmentConfig, GraphConfig, LlmConfig};
use poneglyph_core::enrich::{self, WorkerConfig};
use poneglyph_core::llm::LlmClient;
use poneglyph_core::model::{EdgeType, JobType, MemoryType, Source};
use poneglyph_core::store::Store;

/// Spawn a mock /chat/completions endpoint that always replies `content`.
async fn mock_llm(content: &str) -> (String, tokio::task::JoinHandle<()>) {
    let reply = Arc::new(content.to_string());
    let app = axum::Router::new().route(
        "/chat/completions",
        post(move |_body: Json<Value>| {
            let reply = Arc::clone(&reply);
            async move {
                Json(json!({
                    "id": "mock",
                    "object": "chat.completion",
                    "created": 0,
                    "model": "mock",
                    "choices": [{
                        "index": 0,
                        "message": { "role": "assistant", "content": *reply },
                        "finish_reason": "stop"
                    }]
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), handle)
}

fn client_for(endpoint: &str) -> LlmClient {
    LlmClient::from_config(&LlmConfig {
        enabled: true,
        endpoint: Some(endpoint.to_string()),
        model: Some("mock-model".into()),
        api_key: None,
    })
    .expect("client should construct")
}

fn worker_cfg() -> WorkerConfig {
    WorkerConfig {
        graph: GraphConfig::default(),
        llm: LlmConfig::default(),
        enrichment: EnrichmentConfig::default(),
    }
}

fn long_content() -> String {
    "We migrated the ingestion pipeline from a cron-driven batch importer to a streaming \
     consumer because the batch window kept overrunning; the cutover required dual-writing \
     for a week and backfilling three months of events from the archive bucket. The backfill \
     itself ran at roughly forty thousand events per minute and surfaced two schema drift \
     bugs that the old importer had been silently papering over."
        .to_string()
}

#[tokio::test]
async fn summarize_writes_metadata_summary() {
    let (endpoint, _srv) = mock_llm("Migrated ingestion from batch to streaming.").await;
    let client = client_for(&endpoint);
    let mut store = Store::open_in_memory().unwrap();

    let m = store
        .create_memory(&long_content(), MemoryType::Semantic, 0.5, Source::Cli, None, None)
        .unwrap();

    poneglyph_core::llm::run_job(&mut store, &client, &JobType::Summarize, &m.id)
        .await
        .unwrap();

    let meta = store.get_memory(&m.id).unwrap().unwrap().metadata.unwrap();
    assert_eq!(meta["summary"], "Migrated ingestion from batch to streaming.");
}

#[tokio::test]
async fn summarize_skips_short_content() {
    let (endpoint, _srv) = mock_llm("should never be called").await;
    let client = client_for(&endpoint);
    let mut store = Store::open_in_memory().unwrap();

    let m = store
        .create_memory("short note", MemoryType::Fact, 0.5, Source::Cli, None, None)
        .unwrap();
    poneglyph_core::llm::run_job(&mut store, &client, &JobType::Summarize, &m.id)
        .await
        .unwrap();
    assert!(store.get_memory(&m.id).unwrap().unwrap().metadata.is_none());
}

#[tokio::test]
async fn entities_merge_into_tags_and_reenqueue_edges() {
    let (endpoint, _srv) = mock_llm(r#"["rust", "sqlite"]"#).await;
    let client = client_for(&endpoint);
    let mut store = Store::open_in_memory().unwrap();

    let meta = json!({ "tags": ["existing"] });
    let m = store
        .create_memory("uses rust and sqlite", MemoryType::Fact, 0.5, Source::Cli, None, Some(&meta))
        .unwrap();

    poneglyph_core::llm::run_job(&mut store, &client, &JobType::ExtractEntities, &m.id)
        .await
        .unwrap();

    let meta = store.get_memory(&m.id).unwrap().unwrap().metadata.unwrap();
    let tags: Vec<&str> = meta["tags"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(tags.contains(&"existing") && tags.contains(&"rust") && tags.contains(&"sqlite"));
    assert_eq!(meta["entities"].as_array().unwrap().len(), 2);

    // tag_overlap recompute enqueued.
    assert_eq!(store.stats().unwrap().pending_jobs, 1);
}

#[tokio::test]
async fn relations_create_labeled_edges() {
    let (endpoint, _srv) = mock_llm(r#"[{"index": 1, "predicate": "duplicates"}]"#).await;
    let client = client_for(&endpoint);
    let mut store = Store::open_in_memory().unwrap();

    // Two embedded memories so nearest_neighbors yields a candidate.
    let mut v1 = vec![0.0f32; 384];
    v1[0] = 1.0;
    let mut v2 = v1.clone();
    v2[1] = 0.05;

    let a = store.create_memory("near duplicate one", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
    let b = store.create_memory("near duplicate two", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
    store.index_embedding(&a.id, &v1).unwrap();
    store.index_embedding(&b.id, &v2).unwrap();

    poneglyph_core::llm::run_job(&mut store, &client, &JobType::ExtractRelations, &a.id)
        .await
        .unwrap();

    let edges = store.get_edges_for_memory(&a.id).unwrap();
    let rel: Vec<_> = edges.iter().filter(|e| e.edge_type == EdgeType::Relation).collect();
    assert_eq!(rel.len(), 1);
    assert_eq!(rel[0].label.as_deref(), Some("duplicates"));
    assert_eq!(rel[0].src_id, a.id);
    assert_eq!(rel[0].dst_id, b.id);
}

#[tokio::test]
async fn score_importance_updates_memory() {
    let (endpoint, _srv) = mock_llm("0.85").await;
    let client = client_for(&endpoint);
    let mut store = Store::open_in_memory().unwrap();

    let m = store
        .create_memory("critical incident postmortem", MemoryType::Episodic, 0.5, Source::Cli, None, None)
        .unwrap();
    poneglyph_core::llm::run_job(&mut store, &client, &JobType::ScoreImportance, &m.id)
        .await
        .unwrap();
    let updated = store.get_memory(&m.id).unwrap().unwrap();
    assert!((updated.importance - 0.85).abs() < 1e-9);
}

#[tokio::test]
async fn garbage_reply_errors_and_retry_path_marks_failed_at_cap() {
    let (endpoint, _srv) = mock_llm("certainly! here is some prose, not JSON").await;
    let client = client_for(&endpoint);
    let mut store = Store::open_in_memory().unwrap();

    let m = store
        .create_memory("x", MemoryType::Fact, 0.5, Source::Cli, None, None)
        .unwrap();
    let job = store.create_job(JobType::ExtractEntities, &m.id).unwrap();

    let mut cfg = worker_cfg();
    cfg.enrichment.enabled = true;
    cfg.llm = LlmConfig {
        enabled: true,
        endpoint: Some(endpoint),
        model: Some("mock-model".into()),
        api_key: None,
    };

    // Drive three passes, rewinding the backoff timestamp between them.
    for _ in 0..3 {
        enrich::process_jobs_async(&mut store, &cfg, Some(&client)).await.unwrap();
        let past = (chrono::Utc::now() - chrono::Duration::seconds(3600)).to_rfc3339();
        store
            .conn
            .execute("UPDATE jobs SET updated_at = ?1 WHERE id = ?2", rusqlite::params![past, job.id])
            .unwrap();
    }

    let (status, attempts): (String, i64) = store
        .conn
        .query_row("SELECT status, attempts FROM jobs WHERE id = ?1", [&job.id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(attempts, 3);
}

#[tokio::test]
async fn unreachable_endpoint_fails_gracefully_system_unaffected() {
    // Port 9 (discard) is essentially never bound; connection refused fast.
    let client = client_for("http://127.0.0.1:9");
    let mut store = Store::open_in_memory().unwrap();

    let m = store
        .create_memory(&long_content(), MemoryType::Fact, 0.5, Source::Cli, None, None)
        .unwrap();
    let err = poneglyph_core::llm::run_job(&mut store, &client, &JobType::Summarize, &m.id).await;
    assert!(err.is_err(), "unreachable endpoint must error, not hang/crash");

    // System unaffected: store keeps working (§8.11 AC2).
    let m2 = store
        .create_memory("still alive", MemoryType::Fact, 0.5, Source::Cli, None, None)
        .unwrap();
    assert!(store.get_memory(&m2.id).unwrap().is_some());
}
