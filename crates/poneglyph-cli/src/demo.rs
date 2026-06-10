//! Seed data for `poneglyph demo` — realistic sample memories so the viewer
//! has something to show without a real history.

use anyhow::Result;
use poneglyph_core::enrich;
use poneglyph_core::model::{EdgeType, MemoryType, Source};
use poneglyph_core::store::Store;

pub struct SeedProject {
    pub path: &'static str,
    pub name: &'static str,
    pub git_remote: Option<&'static str>,
}

pub const PROJECTS: &[SeedProject] = &[
    SeedProject {
        path: "/tmp/demo/acme-api",
        name: "acme-api",
        git_remote: Some("github.com/acme/api-server"),
    },
    SeedProject { path: "/tmp/demo/dotfiles", name: "dotfiles", git_remote: None },
    SeedProject {
        path: "/tmp/demo/ml-pipeline",
        name: "ml-pipeline",
        git_remote: Some("github.com/acme/ml-pipeline"),
    },
];

pub struct Seed {
    pub content: &'static str,
    pub ty: MemoryType,
    pub importance: f64,
    pub project: usize,
    pub tags: &'static [&'static str],
}

pub const SEEDS: &[Seed] = &[
    Seed { content: "Switched internal service calls from REST to gRPC because p99 latency dropped from 120ms to 18ms in the load test.", ty: MemoryType::Semantic, importance: 0.9, project: 0, tags: &["grpc", "architecture"] },
    Seed { content: "Postgres connection pool is capped at 20 in production; raising it past 25 caused connection storms during deploys.", ty: MemoryType::Fact, importance: 0.8, project: 0, tags: &["postgres", "production"] },
    Seed { content: "Prefer thiserror for library errors and anyhow at the binary edge.", ty: MemoryType::Preference, importance: 0.6, project: 0, tags: &["rust", "errors"] },
    Seed { content: "To rotate the API signing key: 1) generate new key in vault, 2) deploy with both keys accepted, 3) flip signer, 4) remove old key after 24h.", ty: MemoryType::Procedural, importance: 0.85, project: 0, tags: &["security", "runbook"] },
    Seed { content: "Debugged SIGSEGV in libonnxruntime: stale model cache after upgrade; fix was wiping ~/.cache/models and re-downloading.", ty: MemoryType::Episodic, importance: 0.7, project: 2, tags: &["onnx", "debugging"] },
    Seed { content: "Edit src/auth/middleware.rs — fixed token expiry comparison to use <= instead of <.", ty: MemoryType::CodeContext, importance: 0.5, project: 0, tags: &["auth", "bugfix"] },
    Seed { content: "Auth tokens expire after 3600 seconds; the refresh window is the final 300 seconds.", ty: MemoryType::Fact, importance: 0.75, project: 0, tags: &["auth"] },
    Seed { content: "Token expiry check in middleware used < where the spec requires <= — caused intermittent 401s at exactly the expiry second.", ty: MemoryType::Episodic, importance: 0.8, project: 0, tags: &["auth", "bugfix"] },
    Seed { content: "Training jobs must pin numpy<2 until the feature extractor is rebuilt against the new ABI.", ty: MemoryType::Fact, importance: 0.7, project: 2, tags: &["python", "numpy"] },
    Seed { content: "The feature extractor segfaults with numpy 2.x because it was compiled against the 1.x C ABI.", ty: MemoryType::Semantic, importance: 0.75, project: 2, tags: &["python", "numpy"] },
    Seed { content: "Use fish shell abbreviations instead of aliases — they expand inline and stay editable.", ty: MemoryType::Preference, importance: 0.4, project: 1, tags: &["fish", "shell"] },
    Seed { content: "Keyboard remap lives in ~/.config/karabiner/karabiner.json; caps lock is hyper key.", ty: MemoryType::Fact, importance: 0.5, project: 1, tags: &["macos", "keyboard"] },
    Seed { content: "To bootstrap a new machine: run ./install.sh, then `fisher update`, then log out/in for karabiner permissions.", ty: MemoryType::Procedural, importance: 0.6, project: 1, tags: &["setup", "runbook"] },
    Seed { content: "Bash cargo test --workspace — 49 tests passed after the HTTP API landed.", ty: MemoryType::CodeContext, importance: 0.3, project: 0, tags: &["testing"] },
    Seed { content: "Decided to keep the ML feature store in DuckDB rather than Postgres: single-file, columnar scans 40x faster for our batch reads.", ty: MemoryType::Semantic, importance: 0.85, project: 2, tags: &["duckdb", "architecture"] },
    Seed { content: "GPU quota on the shared cluster resets Mondays 00:00 UTC; schedule heavy sweeps for Monday mornings.", ty: MemoryType::Fact, importance: 0.6, project: 2, tags: &["gpu", "cluster"] },
    Seed { content: "Always run database migrations through the CI gate, never by hand against production.", ty: MemoryType::Preference, importance: 0.9, project: 0, tags: &["database", "process"] },
    Seed { content: "Incident 2024-11: deploy during pool exhaustion took the API down for 9 minutes; postmortem action was the pool cap plus deploy-time connection draining.", ty: MemoryType::Episodic, importance: 0.95, project: 0, tags: &["incident", "postgres", "production"] },
    Seed { content: "Edit pipelines/train.py — added retry with exponential backoff around the S3 dataset fetch.", ty: MemoryType::CodeContext, importance: 0.45, project: 2, tags: &["s3", "resilience"] },
    Seed { content: "Terminal: kubectl rollout undo deployment/api-server after the bad 2.31 release.", ty: MemoryType::CodeContext, importance: 0.55, project: 0, tags: &["kubernetes", "rollback"] },
];

/// Labeled relation edges between seed indices (src, dst, predicate).
pub const RELATIONS: &[(usize, usize, &str)] = &[
    (7, 5, "fixed by"),
    (17, 1, "led to"),
    (9, 8, "explains"),
    (6, 7, "context for"),
];

/// Explicit edges between seed indices.
pub const EXPLICIT: &[(usize, usize)] = &[(0, 17), (14, 9), (3, 16)];

pub struct SeedOutcome {
    pub memories: usize,
    pub edges: usize,
    pub projects: usize,
}

/// Seed `count` memories (cycling the templates), backdate them over ~30
/// days, drain edge jobs inline, then add hand-picked explicit/relation
/// edges. `embed` is an optional per-content embedding callback so the CLI
/// can pass the real embedder while tests stay offline.
pub fn seed(
    store: &Store,
    count: usize,
    graph_cfg: &poneglyph_core::config::GraphConfig,
    mut embed: Option<&mut dyn FnMut(&str) -> Result<Vec<f32>>>,
) -> Result<SeedOutcome> {
    let project_ids: Vec<String> = PROJECTS
        .iter()
        .map(|p| store.upsert_project(p.path, p.name, p.git_remote).map(|p| p.id))
        .collect::<Result<_>>()?;

    let now = chrono::Utc::now();
    let mut memory_ids = Vec::with_capacity(count);

    for i in 0..count {
        let seed = &SEEDS[i % SEEDS.len()];
        let content = if i < SEEDS.len() {
            seed.content.to_string()
        } else {
            format!("{} (variant {})", seed.content, i / SEEDS.len())
        };
        let session = format!("demo-session-{}", i / 3 + 1);
        let metadata = serde_json::json!({ "tags": seed.tags, "session_id": session });

        let mem = store.create_memory(
            &content,
            seed.ty.clone(),
            seed.importance,
            Source::Import,
            Some(&project_ids[seed.project]),
            Some(&metadata),
        )?;
        store.index_fts(&mem.id, &content)?;
        if let Some(embed) = embed.as_deref_mut() {
            let vec = embed(&content)?;
            store.index_embedding(&mem.id, &vec)?;
        }

        // Backdate over ~30 days in small clusters so temporal edges form
        // within groups instead of one giant clique. Demo-only raw SQL.
        let days_ago = (i / 3) as i64 % 30;
        let cluster_offset_secs = (i % 3) as i64 * 60; // within the 5-min window
        let ts = (now - chrono::Duration::days(days_ago)
            + chrono::Duration::seconds(cluster_offset_secs))
        .to_rfc3339();
        store.conn.execute(
            "UPDATE memories SET created_at = ?1, updated_at = ?1 WHERE id = ?2",
            rusqlite::params![ts, mem.id],
        )?;

        enrich::enqueue_compute_edges(store, &mem.id)?;
        memory_ids.push(mem.id);
    }

    // Drain all edge jobs inline (no resident worker in demo).
    while enrich::process_pending_jobs(store, graph_cfg)? > 0 {}

    // Hand-picked edges so every edge type shows in the explorer.
    for &(a, b) in EXPLICIT {
        if a < memory_ids.len() && b < memory_ids.len() {
            store.create_edge(&memory_ids[a], &memory_ids[b], EdgeType::Explicit, None, 1.0)?;
        }
    }
    for &(a, b, label) in RELATIONS {
        if a < memory_ids.len() && b < memory_ids.len() {
            store.create_edge(&memory_ids[a], &memory_ids[b], EdgeType::Relation, Some(label), 0.9)?;
        }
    }

    let stats = store.stats()?;
    Ok(SeedOutcome {
        memories: stats.memory_count as usize,
        edges: stats.edge_count as usize,
        projects: stats.project_count as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use poneglyph_core::config::GraphConfig;

    #[test]
    fn seed_populates_store_offline() {
        let store = Store::open_in_memory().unwrap();
        let out = seed(&store, 20, &GraphConfig::default(), None).unwrap();
        assert_eq!(out.memories, 20);
        assert_eq!(out.projects, 3);
        assert!(out.edges > 0, "temporal/tag edges should form without embeddings");
        assert_eq!(store.stats().unwrap().pending_jobs, 0, "queue drained");
    }

    #[test]
    fn seed_cycles_past_template_count() {
        let store = Store::open_in_memory().unwrap();
        let out = seed(&store, SEEDS.len() + 5, &GraphConfig::default(), None).unwrap();
        assert_eq!(out.memories, SEEDS.len() + 5);
    }

    #[test]
    fn seed_rows_carry_session_id() {
        let store = Store::open_in_memory().unwrap();
        seed(&store, 6, &GraphConfig::default(), None).unwrap();
        let (mems, _) = store.list_memories(None, None, 10, 0).unwrap();
        for mem in &mems {
            let meta = mem.metadata.as_ref().unwrap();
            let sid = meta.get("session_id").and_then(|v| v.as_str());
            assert!(sid.is_some(), "every seeded memory should have a session_id");
            assert!(sid.unwrap().starts_with("demo-session-"));
        }
    }
}
