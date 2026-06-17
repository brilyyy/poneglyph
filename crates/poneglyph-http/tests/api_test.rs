//! HTTP API integration tests (PRD §8.7, §8.8, §12).
//!
//! Drives the router with `tower::ServiceExt::oneshot` — no sockets — and
//! asserts DB side effects through a cloned handle to the same store.
//! Embedder is None throughout (offline / FTS-only), same as the MCP tests.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use poneglyph_core::config::Config;
use poneglyph_core::model::{EdgeType, MemoryType, Source};
use poneglyph_core::store::Store;
use poneglyph_http::{AppState, build_router, validate_security};

fn test_state(config: Config) -> (AppState, Arc<Mutex<Store>>) {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let state = AppState {
        store: Arc::clone(&store),
        embedder: None,
        config: Arc::new(config),
        enrich: None,
    };
    (state, store)
}

fn open_state() -> (AppState, Arc<Mutex<Store>>) {
    test_state(Config::default())
}

async fn send(router: axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

fn json_req(method: &str, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

// ---------------------------------------------------------------------------
// /ingest
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ingest_creates_passive_memory_tagged_tool_and_project() {
    let (state, store) = open_state();
    let router = build_router(state);

    let event = json!({
        "event": "tool_use",
        "client": "claude-code",
        "project_path": "/home/user/myproject",
        "content": "Bash {\"command\": \"cargo test\"}",
        "tool": "Bash",
        "metadata": { "session_id": "abc123" }
    });
    let (status, body) = send(router.clone(), json_req("POST", "/ingest", event)).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = body["id"].as_str().unwrap().to_string();

    // DB side effects (AC §8.7): passive code_context memory, tagged, project attached.
    {
        let store = store.lock().unwrap();
        let mem = store.get_memory(&id).unwrap().unwrap();
        assert_eq!(mem.source, Source::Passive);
        assert_eq!(mem.memory_type, MemoryType::CodeContext);
        assert!(mem.project_id.is_some());

        let meta = mem.metadata.unwrap();
        let tags: Vec<&str> = meta["tags"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
        assert!(tags.contains(&"claude-code"));
        assert!(tags.contains(&"Bash"));
        assert_eq!(meta["tool"], "Bash");
        assert_eq!(meta["extra"]["session_id"], "abc123");

        let project = store.get_project("/home/user/myproject").unwrap().unwrap();
        assert_eq!(Some(project.id), mem.project_id);

        // Edge job enqueued, never computed inline.
        assert_eq!(store.stats().unwrap().pending_jobs, 1);
    }

    // FTS-indexed ⇒ findable via /api/search without an embedder.
    let (status, body) = send(router, get("/api/search?q=cargo")).await;
    assert_eq!(status, StatusCode::OK);
    let results = body["results"].as_array().unwrap();
    assert!(results.iter().any(|r| r["id"] == id.as_str()));
}

#[tokio::test]
async fn ingest_user_message_maps_to_episodic() {
    let (state, store) = open_state();
    let router = build_router(state);

    let event = json!({
        "event": "user_message",
        "client": "claude-code",
        "content": "please fix the auth bug"
    });
    let (status, body) = send(router, json_req("POST", "/ingest", event)).await;
    assert_eq!(status, StatusCode::CREATED);

    let store = store.lock().unwrap();
    let mem = store.get_memory(body["id"].as_str().unwrap()).unwrap().unwrap();
    assert_eq!(mem.memory_type, MemoryType::Episodic);
    assert_eq!(mem.source, Source::Passive);
}

#[tokio::test]
async fn ingest_rejects_bad_events() {
    let (state, _) = open_state();
    let router = build_router(state);

    // Empty content.
    let (status, body) = send(
        router.clone(),
        json_req("POST", "/ingest", json!({"event": "tool_use", "client": "x", "content": "  "})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].is_string());

    // Unknown event kind — serde rejects, still {"error": ...} shaped.
    let (status, body) = send(
        router,
        json_req("POST", "/ingest", json!({"event": "bogus", "client": "x", "content": "hi"})),
    )
    .await;
    assert!(status.is_client_error());
    assert!(body["error"].is_string());
}

// ---------------------------------------------------------------------------
// /api/memories
// ---------------------------------------------------------------------------

#[tokio::test]
async fn memories_list_filter_paginate() {
    let (state, store) = open_state();
    {
        let store = store.lock().unwrap();
        for i in 0..5 {
            store
                .create_memory(&format!("fact {i}"), MemoryType::Fact, 0.5, Source::Cli, None, None)
                .unwrap();
        }
        store
            .create_memory("a preference", MemoryType::Preference, 0.5, Source::Cli, None, None)
            .unwrap();
    }
    let router = build_router(state);

    let (status, body) = send(router.clone(), get("/api/memories?type=fact&limit=2&offset=0")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 5);
    assert_eq!(body["results"].as_array().unwrap().len(), 2);

    let (_, body) = send(router.clone(), get("/api/memories")).await;
    assert_eq!(body["total"], 6);

    // Unknown project ⇒ empty, not error.
    let (status, body) = send(router, get("/api/memories?project_path=/nope")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 0);
}

#[tokio::test]
async fn memory_detail_includes_edges() {
    let (state, store) = open_state();
    let (a, b) = {
        let store = store.lock().unwrap();
        let a = store.create_memory("a", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let b = store.create_memory("b", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        store.create_edge(&a.id, &b.id, EdgeType::Similarity, None, 0.9).unwrap();
        (a, b)
    };
    let router = build_router(state);

    let (status, body) = send(router.clone(), get(&format!("/api/memories/{}", a.id))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], a.id.as_str());
    assert_eq!(body["edges"].as_array().unwrap().len(), 1);
    assert_eq!(body["edges"][0]["dst_id"], b.id.as_str());

    let (status, _) = send(router, get("/api/memories/no-such-id")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn memory_patch_and_delete() {
    let (state, store) = open_state();
    let mem = {
        let store = store.lock().unwrap();
        let m = store.create_memory("original", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        store.index_fts(&m.id, "original").unwrap();
        m
    };
    let router = build_router(state);

    // PATCH updates content + FTS row.
    let (status, body) = send(
        router.clone(),
        json_req("PATCH", &format!("/api/memories/{}", mem.id), json!({"new_content": "rewritten"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["updated"], true);
    {
        let store = store.lock().unwrap();
        assert_eq!(store.get_memory(&mem.id).unwrap().unwrap().content, "rewritten");
        let fts: String = store
            .conn
            .query_row("SELECT content FROM fts_memories WHERE memory_id = ?1", [&mem.id], |r| r.get(0))
            .unwrap();
        assert_eq!(fts, "rewritten");
    }

    let (status, _) = send(
        router.clone(),
        json_req("PATCH", "/api/memories/no-such-id", json!({"new_content": "x"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // DELETE then 404 on repeat.
    let (status, body) = send(
        router.clone(),
        Request::builder().method("DELETE").uri(format!("/api/memories/{}", mem.id)).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["deleted"], true);

    let (status, _) = send(
        router,
        Request::builder().method("DELETE").uri(format!("/api/memories/{}", mem.id)).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// /api/search
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_fts_only_returns_ranked_results() {
    let (state, store) = open_state();
    {
        let store = store.lock().unwrap();
        let m1 = store.create_memory("token expiry uses < not <=", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        store.index_fts(&m1.id, "token expiry uses < not <=").unwrap();
        let m2 = store.create_memory("dinner was pasta", MemoryType::Episodic, 0.5, Source::Cli, None, None).unwrap();
        store.index_fts(&m2.id, "dinner was pasta").unwrap();
    }
    let router = build_router(state);

    let (status, body) = send(router.clone(), get("/api/search?q=token%20expiry")).await;
    assert_eq!(status, StatusCode::OK);
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0]["score"].as_f64().unwrap() > 0.0);
    assert!(results[0]["content"].as_str().unwrap().contains("expiry"));

    // Empty q is a 400.
    let (status, _) = send(router, get("/api/search?q=%20")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// /api/graph
// ---------------------------------------------------------------------------

#[tokio::test]
async fn graph_focus_neighborhood_and_global_sample() {
    let (state, store) = open_state();
    let (a, b, _c, d) = {
        let store = store.lock().unwrap();
        let a = store.create_memory("a", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let b = store.create_memory("b", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let c = store.create_memory("c", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        let d = store.create_memory("d", MemoryType::Fact, 0.5, Source::Cli, None, None).unwrap();
        store.create_edge(&a.id, &b.id, EdgeType::Similarity, None, 0.9).unwrap();
        store.create_edge(&b.id, &c.id, EdgeType::Similarity, None, 0.9).unwrap();
        store.create_edge(&c.id, &d.id, EdgeType::Temporal, None, 1.0).unwrap();
        (a, b, c, d)
    };
    let router = build_router(state);

    // depth=1 around b: a, b, c.
    let (status, body) = send(router.clone(), get(&format!("/api/graph?focus={}&depth=1", b.id))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(body["edges"].as_array().unwrap().len(), 2);

    // No focus ⇒ global sample with cap.
    let (status, body) = send(router.clone(), get("/api/graph?limit=2")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["nodes"].as_array().unwrap().len(), 2);
    // Sample is most-recent first: c, d — only the c—d edge qualifies.
    assert_eq!(body["edges"].as_array().unwrap().len(), 1);
    assert_eq!(body["edges"][0]["dst_id"], d.id.as_str());

    // Unknown focus is a 404.
    let (status, _) = send(router, get("/api/graph?focus=no-such-id")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let _ = a;
}

// ---------------------------------------------------------------------------
// /api/context
// ---------------------------------------------------------------------------

#[tokio::test]
async fn context_returns_ranked_project_memories_under_budget() {
    let (state, store) = open_state();
    {
        let store = store.lock().unwrap();
        let p = store.upsert_project("/home/u/proj", "proj", None).unwrap();
        store
            .create_memory("critical architecture decision", MemoryType::Semantic, 0.9, Source::Cli, Some(&p.id), None)
            .unwrap();
        store
            .create_memory("minor note", MemoryType::Fact, 0.1, Source::Cli, Some(&p.id), None)
            .unwrap();
    }
    let router = build_router(state);

    let (status, body) = send(
        router.clone(),
        get("/api/context?project_path=%2Fhome%2Fu%2Fproj&max_tokens=600"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ctx = body["context"].as_str().unwrap();
    assert!(ctx.contains("critical architecture decision"));
    assert_eq!(body["memory_count"], 2);
    // 600 tokens ≈ 2400 chars budget.
    assert!(ctx.len() <= 600 * 4 + 100);

    // Unknown project ⇒ empty context, not an error.
    let (status, body) = send(router.clone(), get("/api/context?project_path=%2Fnope")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["context"], "");
    assert_eq!(body["memory_count"], 0);

    // Missing project_path ⇒ 4xx.
    let (status, _) = send(router, get("/api/context")).await;
    assert!(status.is_client_error());
}

// ---------------------------------------------------------------------------
// /api/stats, /api/projects, /api/settings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stats_and_projects() {
    let (state, store) = open_state();
    {
        let store = store.lock().unwrap();
        let p = store.upsert_project("/p", "p", None).unwrap();
        store.create_memory("x", MemoryType::Fact, 0.5, Source::Cli, Some(&p.id), None).unwrap();
    }
    let router = build_router(state);

    let (status, body) = send(router.clone(), get("/api/stats")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["memory_count"], 1);
    assert_eq!(body["project_count"], 1);

    let (status, body) = send(router, get("/api/projects")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["projects"].as_array().unwrap().len(), 1);
    assert_eq!(body["projects"][0]["path"], "/p");
}

#[tokio::test]
async fn settings_get_hides_secrets() {
    let mut config = Config::default();
    config.dashboard.token = Some("super-secret".into());
    config.llm.api_key = Some("sk-xyz".into());
    let (state, _) = test_state(config);
    let router = build_router(state);

    let req = Request::builder()
        .uri("/api/settings")
        .header(header::AUTHORIZATION, "Bearer super-secret")
        .body(Body::empty())
        .unwrap();
    let (status, body) = send(router, req).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["dashboard"]["token_set"], true);
    assert!(body["dashboard"].get("token").is_none());
    assert_eq!(body["llm"]["api_key_set"], true);
    assert!(body["llm"].get("api_key").is_none());
    // Body must never contain the secret values anywhere.
    let s = body.to_string();
    assert!(!s.contains("super-secret") && !s.contains("sk-xyz"));
}

#[tokio::test]
async fn settings_patch_rejects_non_whitelisted() {
    let (state, _) = open_state();
    let router = build_router(state);

    let (status, body) = send(
        router.clone(),
        json_req("PATCH", "/api/settings", json!({"dashboard": {"token": "evil"}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("dashboard.token"));

    let (status, _) = send(
        router,
        json_req("PATCH", "/api/settings", json!({"db_path": "/tmp/steal.db"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Auth (PRD §12)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_required_when_token_set() {
    let mut config = Config::default();
    config.dashboard.token = Some("tok".into());
    let (state, _) = test_state(config);
    let router = build_router(state);

    // No token ⇒ 401 on /api and /ingest.
    let (status, body) = send(router.clone(), get("/api/stats")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["error"].is_string());

    let (status, _) = send(
        router.clone(),
        json_req("POST", "/ingest", json!({"event": "terminal", "client": "x", "content": "ls"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Wrong token ⇒ 401.
    let req = Request::builder()
        .uri("/api/stats")
        .header(header::AUTHORIZATION, "Bearer wrong")
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(router.clone(), req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Correct token ⇒ 200.
    let req = Request::builder()
        .uri("/api/stats")
        .header(header::AUTHORIZATION, "Bearer tok")
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(router.clone(), req).await;
    assert_eq!(status, StatusCode::OK);

    // /healthz and / stay open.
    let (status, _) = send(router.clone(), get("/healthz")).await;
    assert_eq!(status, StatusCode::OK);
    let resp = router.oneshot(get("/")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn no_token_configured_means_open() {
    let (state, _) = open_state();
    let router = build_router(state);
    let (status, _) = send(router, get("/api/stats")).await;
    assert_eq!(status, StatusCode::OK);
}

// ---------------------------------------------------------------------------
// validate_security (PRD §12 startup refusal)
// ---------------------------------------------------------------------------

#[test]
fn validate_security_rules() {
    // Loopback without token: fine.
    let config = Config::default();
    assert!(validate_security(&config).is_ok());

    // Non-loopback without token: refused.
    let mut config = Config::default();
    config.dashboard.host = "0.0.0.0".into();
    assert!(validate_security(&config).is_err());

    // Whitespace-only token doesn't count.
    config.dashboard.token = Some("   ".into());
    assert!(validate_security(&config).is_err());

    // Non-loopback with real token: fine.
    config.dashboard.token = Some("tok".into());
    assert!(validate_security(&config).is_ok());

    // Garbage bind addr: refused.
    config.dashboard.host = "not-an-ip".into();
    assert!(validate_security(&config).is_err());
}

// ---------------------------------------------------------------------------
// /api/timeline
// ---------------------------------------------------------------------------

#[tokio::test]
async fn timeline_groups_sessions() {
    let (state, store) = open_state();
    {
        let store = store.lock().unwrap();
        store.create_memory(
            "a", MemoryType::Fact, 0.5, Source::Cli, None,
            Some(&json!({ "session_id": "sess-1" })),
        ).unwrap();
        store.create_memory(
            "b", MemoryType::Fact, 0.5, Source::Cli, None,
            Some(&json!({ "session_id": "sess-1" })),
        ).unwrap();
        store.create_memory(
            "c", MemoryType::Fact, 0.5, Source::Cli, None,
            Some(&json!({ "session_id": "sess-2" })),
        ).unwrap();
    }
    let router = build_router(state);

    let (status, body) = send(router, get("/api/timeline?limit=10")).await;
    assert_eq!(status, StatusCode::OK);
    let sessions = body["sessions"].as_array().unwrap();
    assert_eq!(body["total"], 2);
    assert_eq!(sessions.len(), 2);
    // Each session should have memories.
    for s in sessions {
        assert!(s["memory_count"].as_u64().unwrap() > 0);
        assert!(s["memories"].as_array().unwrap().len() > 0);
    }
}

#[tokio::test]
async fn ingest_hoists_session_id_to_top_level() {
    let (state, store) = open_state();
    let router = build_router(state);

    // Case 1: session_id in ev.metadata → hoisted to top-level metadata.session_id.
    let event = json!({
        "event": "tool_use",
        "client": "claude-code",
        "content": "test hoist",
        "metadata": { "session_id": "hoisted-sid" }
    });
    let (status, body) = send(router.clone(), json_req("POST", "/ingest", event)).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = body["id"].as_str().unwrap();

    {
        let store = store.lock().unwrap();
        let mem = store.get_memory(id).unwrap().unwrap();
        let meta = mem.metadata.unwrap();
        // Top-level session_id should be present (hoisted).
        assert_eq!(meta["session_id"], "hoisted-sid");
        // extra still carries the original metadata.
        assert_eq!(meta["extra"]["session_id"], "hoisted-sid");
    }

    // Case 2: second session — timeline should find both.
    let event2 = json!({
        "event": "tool_use",
        "client": "claude-code",
        "content": "second session",
        "metadata": { "session_id": "other-sid" }
    });
    let (status, _) = send(router.clone(), json_req("POST", "/ingest", event2)).await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, body) = send(router, get("/api/timeline?limit=100")).await;
    assert_eq!(status, StatusCode::OK);
    let sessions = body["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
}

// ---------------------------------------------------------------------------
// /api/codegraph, /api/codegraph/stats
// ---------------------------------------------------------------------------

fn seed_codegraph(store: &Store) {
    use poneglyph_core::model::{CgEdge, CgEdgeKind, CgFile, CgNode, CgNodeKind};
    store.cg_upsert_file(&CgFile { path: "a.rs".into(), language: "rust".into(), content_hash: "h1".into() }).unwrap();
    let caller = CgNode { id: "a.rs#1:caller".into(), file_path: "a.rs".into(), kind: CgNodeKind::Function, name: "caller".into(), start_line: 1, end_line: 1 };
    let callee = CgNode { id: "a.rs#2:callee".into(), file_path: "a.rs".into(), kind: CgNodeKind::Function, name: "callee".into(), start_line: 2, end_line: 2 };
    store.cg_insert_node(&caller).unwrap();
    store.cg_insert_node(&callee).unwrap();
    store.cg_insert_edge(&CgEdge { src_id: caller.id, dst_id: callee.id, kind: CgEdgeKind::Calls }).unwrap();
}

#[tokio::test]
async fn codegraph_endpoint_returns_full_graph_with_no_focus() {
    let (state, store) = open_state();
    seed_codegraph(&store.lock().unwrap());
    let router = build_router(state);

    let (status, body) = send(router, get("/api/codegraph")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(body["edges"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn codegraph_endpoint_focus_returns_blast_radius_subset() {
    let (state, store) = open_state();
    seed_codegraph(&store.lock().unwrap());
    let router = build_router(state);

    let (status, body) = send(router, get("/api/codegraph?focus=callee&depth=2")).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = body["nodes"].as_array().unwrap().iter().map(|n| n["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"callee"));
    assert!(names.contains(&"caller"));
}

#[tokio::test]
async fn codegraph_stats_reports_counts() {
    let (state, store) = open_state();
    seed_codegraph(&store.lock().unwrap());
    let router = build_router(state);

    let (status, body) = send(router, get("/api/codegraph/stats")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["files"], 1);
    assert_eq!(body["nodes"], 2);
    assert_eq!(body["edges"], 1);
}

// ---------------------------------------------------------------------------
// /api/token-savings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn token_savings_estimates_compression_on_stored_content() {
    let (state, store) = open_state();
    {
        let store = store.lock().unwrap();
        store
            .create_memory(
                "The function and the configuration would, however, change because of this.",
                MemoryType::Fact,
                0.5,
                Source::Cli,
                None,
                None,
            )
            .unwrap();
    }
    let router = build_router(state);

    let (status, body) = send(router, get("/api/token-savings")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sampled_memories"], 1);
    assert!(body["original_bytes"].as_u64().unwrap() > body["compressed_bytes"].as_u64().unwrap());
    assert!(body["savings_pct"].as_f64().unwrap() > 0.0);
    assert_eq!(body["compression_enabled"], false);
}

// ---------------------------------------------------------------------------
// /api/agents-status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agents_status_reports_config_flags_for_every_agent() {
    let (state, _store) = open_state();
    let router = build_router(state);

    let (status, body) = send(router, get("/api/agents-status")).await;
    assert_eq!(status, StatusCode::OK);
    for agent in ["claude_code", "cursor", "gemini_cli", "opencode", "codex", "copilot_cli"] {
        assert_eq!(body[agent]["enabled"], true, "{agent} should be enabled by default config");
        assert!(body[agent]["detected"].is_boolean());
    }
}
