//! Pipeline LLM-path integration tests — a mock OpenAI-compatible endpoint
//! stands in for the local model, so these confirm each stage actually takes
//! its LLM branch (and produces the expected shape) instead of only ever
//! exercising the deterministic fallback covered by `pipeline.rs`'s own
//! unit tests.
//!
//! Requires `--features llm-openai` (off by default).
#![cfg(feature = "llm-openai")]

use std::sync::Arc;

use axum::routing::post;
use axum::Json;
use serde_json::{json, Value};

use poneglyph_core::config::LlmConfig;
use poneglyph_core::llm::LlmClient;
use poneglyph_core::model::{MemoryType, Source};
use poneglyph_core::pipeline;
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

#[tokio::test]
async fn summarize_session_uses_llm_reply_when_available() {
    let (endpoint, _srv) = mock_llm("Refactored the retrieval fusion logic.").await;
    let client = client_for(&endpoint);
    let mut store = Store::open_in_memory().unwrap();
    let project = store.upsert_project("/p", "p", None).unwrap();

    store
        .create_memory("touched retrieve.rs", MemoryType::Fact, 0.5, Source::Cli, Some(&project.id), None)
        .unwrap();

    let mem = pipeline::summarize_session(&mut store, Some(&project.id), Some(&client))
        .await
        .unwrap()
        .expect("a session summary should be produced");

    assert_eq!(mem.content, "Refactored the retrieval fusion logic.");
    assert_eq!(mem.memory_type, MemoryType::Episodic);
}

#[tokio::test]
async fn distill_semantic_facts_uses_llm_reply_when_available() {
    let reply = json!([
        { "fact": "the project uses sqlite for storage", "confidence": 0.9, "sources": [1, 2] }
    ])
    .to_string();
    let (endpoint, _srv) = mock_llm(&reply).await;
    let client = client_for(&endpoint);
    let mut store = Store::open_in_memory().unwrap();
    let project = store.upsert_project("/p", "p", None).unwrap();
    let config = poneglyph_core::config::Config::default();

    let e1 = store
        .create_memory("set up sqlite schema", MemoryType::Episodic, 0.5, Source::Cli, Some(&project.id), None)
        .unwrap();
    let e2 = store
        .create_memory("added a sqlite migration", MemoryType::Episodic, 0.5, Source::Cli, Some(&project.id), None)
        .unwrap();

    let results = pipeline::distill_semantic_facts(&mut store, &project.id, &config, None, Some(&client))
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].summary, "the project uses sqlite for storage");
    assert!((results[0].confidence - 0.9).abs() < 1e-9);

    let decoy = store.get_memory(&results[0].decoy_id).unwrap().unwrap();
    assert_eq!(decoy.metadata.unwrap()["consolidation"], "llm_distillation");

    let children = store.get_decoy_children(&results[0].decoy_id).unwrap();
    let child_ids: Vec<&str> = children.iter().map(|m| m.id.as_str()).collect();
    assert!(child_ids.contains(&e1.id.as_str()));
    assert!(child_ids.contains(&e2.id.as_str()));
}

#[tokio::test]
async fn synthesize_procedures_uses_llm_reply_when_available() {
    let reply = json!({
        "name": "test-before-commit",
        "trigger": "before committing",
        "steps": ["run tests", "fix failures", "commit"],
        "outcome": "a green commit"
    })
    .to_string();
    let (endpoint, _srv) = mock_llm(&reply).await;
    let client = client_for(&endpoint);
    let mut store = Store::open_in_memory().unwrap();
    let project = store.upsert_project("/p", "p", None).unwrap();

    for tool in ["edit", "test", "commit", "edit"] {
        let meta = json!({ "tool": tool });
        store
            .create_memory(&format!("ran {tool}"), MemoryType::CodeContext, 0.4, Source::Passive, Some(&project.id), Some(&meta))
            .unwrap();
    }

    let procedures = pipeline::synthesize_procedures(&mut store, &project.id, Some(&client)).await.unwrap();

    assert_eq!(procedures.len(), 1);
    let proc = &procedures[0];
    assert_eq!(proc.memory_type, MemoryType::Procedural);
    assert_eq!(proc.metadata.as_ref().unwrap()["consolidation"], "llm_synthesis");
    assert!(proc.content.contains("test-before-commit"));
}
