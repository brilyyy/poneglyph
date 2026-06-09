//! M2 integration test (PRD §8.6): in-process rmcp client drives the server
//! over a duplex transport; DB side effects are asserted through a second
//! connection to the same SQLite file.
//!
//! Runs without the embedding model (embedder = None ⇒ FTS-only recall), so
//! it is fully offline and CI-safe.

use std::sync::{Arc, Mutex};

use rmcp::{ClientHandler, ServiceExt, model::CallToolRequestParams, model::ClientInfo};
use serde_json::{Value, json};

use poneglyph_core::config::Config;
use poneglyph_core::store::Store;
use poneglyph_mcp::tools::PoneglyphMcp;

#[derive(Debug, Clone, Default)]
struct TestClient;

impl ClientHandler for TestClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}

fn structured(result: &rmcp::model::CallToolResult) -> Value {
    result
        .structured_content
        .clone()
        .expect("tool should return structured content")
}

#[tokio::test]
async fn mcp_round_trip_store_and_recall() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("test.db");

    let store = Arc::new(Mutex::new(Store::open(&db_path)?));
    let server = PoneglyphMcp::new(Arc::clone(&store), None, Arc::new(Config::default()));

    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
    let server_handle = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });
    let client = TestClient.serve(client_transport).await?;

    // --- remember ---
    let result = client
        .call_tool(
            CallToolRequestParams::new("remember").with_arguments(
                json!({
                    "content": "poneglyph uses sqlite-vec for vector search",
                    "memory_type": "fact",
                    "importance": 0.8,
                    "project_path": "/tmp/poneglyph-test-project",
                    "tags": ["architecture", "storage"]
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await?;
    let id = structured(&result)["id"].as_str().unwrap().to_string();
    assert!(!id.is_empty());

    // DB side effects via an independent connection.
    {
        let check = Store::open(&db_path)?;
        let mem = check.get_memory(&id)?.expect("memory persisted");
        assert_eq!(mem.content, "poneglyph uses sqlite-vec for vector search");
        assert_eq!(mem.importance, 0.8);
        assert!(mem.project_id.is_some(), "project should be attached");

        let fts_count: i64 = check.conn.query_row(
            "SELECT COUNT(*) FROM fts_memories WHERE memory_id = ?1",
            [&id],
            |r| r.get(0),
        )?;
        assert_eq!(fts_count, 1, "FTS indexed");

        // M3: remember enqueues edge computation instead of running it inline.
        let job_count: i64 = check.conn.query_row(
            "SELECT COUNT(*) FROM jobs WHERE memory_id = ?1 AND job_type = 'compute_edges'",
            [&id],
            |r| r.get(0),
        )?;
        assert_eq!(job_count, 1, "compute_edges job enqueued");
    }

    // --- recall (sparse path; no embedder in test) ---
    let result = client
        .call_tool(
            CallToolRequestParams::new("recall").with_arguments(
                json!({ "query": "what's used for vector search?" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await?;
    let results = structured(&result)["results"].as_array().unwrap().clone();
    assert!(
        results.iter().any(|r| r["id"] == json!(id)),
        "recall should find the stored memory"
    );

    // --- list_memories ---
    let result = client
        .call_tool(
            CallToolRequestParams::new("list_memories").with_arguments(
                json!({ "project_path": "/tmp/poneglyph-test-project" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await?;
    assert_eq!(structured(&result)["total"], json!(1));

    // --- get_project_context ---
    let result = client
        .call_tool(
            CallToolRequestParams::new("get_project_context").with_arguments(
                json!({ "project_path": "/tmp/poneglyph-test-project" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await?;
    let ctx = structured(&result);
    assert_eq!(ctx["memory_count"], json!(1));
    assert!(ctx["context"].as_str().unwrap().contains("sqlite-vec"));

    // --- update_memory ---
    let result = client
        .call_tool(
            CallToolRequestParams::new("update_memory").with_arguments(
                json!({ "id": id, "new_content": "poneglyph uses sqlite-vec (vec0) for KNN" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await?;
    assert_eq!(structured(&result)["updated"], json!(true));
    {
        let check = Store::open(&db_path)?;
        let mem = check.get_memory(&id)?.unwrap();
        assert!(mem.content.contains("vec0"));
    }

    // --- forget ---
    let result = client
        .call_tool(
            CallToolRequestParams::new("forget")
                .with_arguments(json!({ "id": id }).as_object().unwrap().clone()),
        )
        .await?;
    assert_eq!(structured(&result)["deleted"], json!(true));
    {
        let check = Store::open(&db_path)?;
        assert!(check.get_memory(&id)?.is_none(), "memory deleted");
        let fts_count: i64 = check.conn.query_row(
            "SELECT COUNT(*) FROM fts_memories WHERE memory_id = ?1",
            [&id],
            |r| r.get(0),
        )?;
        assert_eq!(fts_count, 0, "FTS entry cascaded");
    }

    client.cancel().await?;
    server_handle.await??;
    Ok(())
}
