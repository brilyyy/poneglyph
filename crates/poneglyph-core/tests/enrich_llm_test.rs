//! LLM enrichment integration tests (PRD §8.5, §8.11, §16).
//!
//! A tiny axum mock stands in for the OpenAI-compatible endpoint, so these
//! exercise the real async-openai wire path with zero network dependency.
//! Failure injection points the client at an unbound port.

use std::sync::Arc;

use axum::routing::post;
use axum::Json;
use serde_json::{json, Value};

use poneglyph_core::config::{CompressionMode, EnrichmentConfig, LlmConfig, MemoryEdgesConfig};
use poneglyph_core::enrich::{self, WorkerConfig};
use poneglyph_core::llm::LlmClient;
use poneglyph_core::model::{EdgeType, JobType, MemoryType, Source};
use poneglyph_core::retrieve::{recall, RecallFilters};
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
        base_url: Some(endpoint.to_string()),
        model: Some("mock-model".into()),
        ..Default::default()
    })
    .expect("client should construct")
}

fn worker_cfg() -> WorkerConfig {
    WorkerConfig {
        edges: MemoryEdgesConfig::default(),
        llm: LlmConfig::default(),
        enrichment: EnrichmentConfig::default(),
        compression_enabled: false,
        compression_mode: CompressionMode::default(),
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
async fn extract_compress_stores_semantic_rewrite_distinct_from_input() {
    let (endpoint, _srv) = mock_llm("Migrated ingestion pipeline from batch to streaming consumer.").await;
    let client = client_for(&endpoint);
    let mut store = Store::open_in_memory().unwrap();

    let m = store
        .create_memory(&long_content(), MemoryType::Semantic, 0.5, Source::Cli, None, None)
        .unwrap();

    poneglyph_core::llm::run_job(&mut store, &client, &JobType::ExtractCompress, &m.id)
        .await
        .unwrap();

    let (compressed, mode) = store.get_compressed_content(&m.id).unwrap().unwrap();
    assert_eq!(mode, "semantic");
    assert!(!compressed.is_empty());
    assert_ne!(compressed, m.content);

    // Original content is the source of truth, untouched.
    let mem = store.get_memory(&m.id).unwrap().unwrap();
    assert_eq!(mem.content, long_content());
}

#[tokio::test]
async fn extract_compress_skips_short_content() {
    let (endpoint, _srv) = mock_llm("should never be called").await;
    let client = client_for(&endpoint);
    let mut store = Store::open_in_memory().unwrap();

    let m = store
        .create_memory("short note", MemoryType::Fact, 0.5, Source::Cli, None, None)
        .unwrap();
    poneglyph_core::llm::run_job(&mut store, &client, &JobType::ExtractCompress, &m.id)
        .await
        .unwrap();
    assert!(store.get_compressed_content(&m.id).unwrap().is_none());
}

#[tokio::test]
async fn no_llm_configured_extract_compress_job_falls_back_to_caveman_without_failing() {
    let mut store = Store::open_in_memory().unwrap();
    let m = store
        .create_memory(&long_content(), MemoryType::Fact, 0.5, Source::Cli, None, None)
        .unwrap();
    store.create_job(JobType::ExtractCompress, &m.id).unwrap();

    let mut cfg = worker_cfg();
    cfg.compression_enabled = true;
    cfg.compression_mode = CompressionMode::Semantic;
    // cfg.llm stays default/disabled — from_config inside spawn_worker would
    // return None; process_jobs_async is exercised directly here with
    // `llm: None` to simulate exactly that outcome.

    let processed = enrich::process_jobs_async(&mut store, &cfg, None).await.unwrap();
    assert_eq!(processed, 1);

    let status: String = store
        .conn
        .query_row("SELECT status FROM jobs WHERE memory_id = ?1", [&m.id], |r| r.get(0))
        .unwrap();
    assert_eq!(status, "done", "compression must degrade gracefully, not fail the job");

    let (_compressed, mode) = store.get_compressed_content(&m.id).unwrap().unwrap();
    assert_eq!(mode, "caveman");
}

#[tokio::test]
async fn recall_never_reads_compressed_content_even_when_present() {
    let (endpoint, _srv) = mock_llm("Migrated ingestion pipeline from batch to streaming consumer.").await;
    let client = client_for(&endpoint);
    let mut store = Store::open_in_memory().unwrap();

    let m = store
        .create_memory(&long_content(), MemoryType::Semantic, 0.5, Source::Cli, None, None)
        .unwrap();
    store.index_fts(&m.id, &long_content()).unwrap();

    poneglyph_core::llm::run_job(&mut store, &client, &JobType::ExtractCompress, &m.id)
        .await
        .unwrap();
    let (compressed, _mode) = store.get_compressed_content(&m.id).unwrap().unwrap();
    assert!(!compressed.contains("schema drift"), "test setup: keyword must only be in the original");

    // "schema drift" appears only in the original content, never in the
    // compressed rewrite — recall finding it proves recall reads `content`.
    let results = recall(&store.conn, None, "schema drift", &RecallFilters::default(), 10).unwrap();
    assert!(
        results.iter().any(|r| r.memory.id == m.id),
        "recall must search original content, never the compressed cache"
    );
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
        base_url: Some(endpoint),
        model: Some("mock-model".into()),
        ..Default::default()
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

#[tokio::test]
async fn corrupt_job_with_missing_memory_fails_gracefully() {
    let (endpoint, _srv) = mock_llm("should not matter").await;
    let client = client_for(&endpoint);
    let mut store = Store::open_in_memory().unwrap();

    // Insert a job referencing a non-existent memory (FK violation scenario).
    // Foreign keys are ON, so INSERT will fail — but we bypass with raw SQL
    // to simulate a corrupt row.
    store
        .conn
        .execute_batch("PRAGMA foreign_keys = OFF")
        .unwrap();
    store
        .conn
        .execute(
            "INSERT INTO jobs (id, job_type, memory_id, status, attempts, created_at, updated_at)
             VALUES ('bad-job', 'compute_edges', 'no-such-memory', 'pending', 0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    store.conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();

    // Process should not crash — the edge build will fail (memory not found)
    // and the job gets retried/failed gracefully.
    let mut cfg = worker_cfg();
    cfg.enrichment.enabled = true;
    cfg.llm = LlmConfig {
        enabled: true,
        base_url: Some(endpoint),
        model: Some("mock-model".into()),
        ..Default::default()
    };

    let result = enrich::process_jobs_async(&mut store, &cfg, Some(&client)).await;
    assert!(result.is_ok(), "corrupt job must not crash the worker");

    // System still functional.
    let m = store.create_memory("after corrupt", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
    assert!(store.get_memory(&m.id).unwrap().is_some());
}

#[tokio::test]
async fn llm_disabled_job_marked_failed_immediately() {
    let mut store = Store::open_in_memory().unwrap();
    let m = store
        .create_memory("x", MemoryType::Fact, 0.5, Source::Cli, None, None)
        .unwrap();
    store.create_job(JobType::Summarize, &m.id).unwrap();

    let mut cfg = worker_cfg();
    cfg.enrichment.enabled = false; // LLM disabled
    cfg.llm.enabled = false;

    let processed = enrich::process_jobs_async(&mut store, &cfg, None).await.unwrap();
    assert_eq!(processed, 1);

    let status: String = store
        .conn
        .query_row("SELECT status FROM jobs WHERE memory_id = ?1", [&m.id], |r| r.get(0))
        .unwrap();
    assert_eq!(status, "failed");
}

#[tokio::test]
async fn edge_only_jobs_work_with_no_llm() {
    let mut store = Store::open_in_memory().unwrap();
    let a = store.create_memory("a", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
    let b = store.create_memory("b", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
    store.index_embedding(&a.id, &vec![1.0f32; 384]).unwrap();
    store.index_embedding(&b.id, &vec![0.99f32; 384]).unwrap();
    store.create_job(JobType::ComputeEdges, &a.id).unwrap();

    let mut cfg = worker_cfg();
    let processed = enrich::process_jobs_async(&mut store, &cfg, None).await.unwrap();
    assert_eq!(processed, 1);

    let status: String = store
        .conn
        .query_row("SELECT status FROM jobs WHERE memory_id = ?1", [&a.id], |r| r.get(0))
        .unwrap();
    assert_eq!(status, "done");
}
