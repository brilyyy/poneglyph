mod demo;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing_subscriber::EnvFilter;

use poneglyph_core::config::Config;
use poneglyph_core::embed::Embedder;
use poneglyph_core::model::{MemoryType, Source};
use poneglyph_core::store::Store;

#[derive(Parser)]
#[command(name = "poneglyph", version, about = "Local AI memory engine")]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Path to config file
    #[arg(long, global = true)]
    config: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize database and config
    Init,
    /// Start MCP + HTTP servers
    Serve,
    /// Store a memory
    Remember {
        content: String,
        #[arg(long, default_value = "semantic")]
        r#type: String,
        #[arg(long, default_value = "0.5")]
        importance: f64,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        tag: Vec<String>,
    },
    /// Search memories
    Recall {
        query: String,
        #[arg(long, default_value = "10")]
        limit: usize,
    },
    /// Delete a memory
    Forget {
        id: String,
    },
    /// Export all memories
    Export {
        #[arg(long, default_value = "json")]
        format: String,
    },
    /// Seed sample data into a database (no server; run `poneglyph serve` to view)
    Demo {
        /// Number of memories to seed
        #[arg(long, default_value = "60")]
        count: usize,
        /// Seed into this DB instead of the configured database
        #[arg(long)]
        db: Option<PathBuf>,
        /// Seed even if the target database already has memories
        #[arg(long)]
        force: bool,
    },
    /// Consolidate similar memories into schema decoys
    Consolidate {
        /// Project path to consolidate (all projects if omitted)
        #[arg(long)]
        project: Option<String>,
    },
    /// Run decay: update strengths and archive low-strength memories
    Decay,
    /// Show status
    Status,
}

fn load_config(cli_config: &Option<PathBuf>) -> Result<Config> {
    match cli_config {
        Some(path) => Config::load_from(path),
        None => Config::load(),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Log to stderr: `poneglyph serve` speaks MCP JSON-RPC on stdout.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env().add_directive("poneglyph=info".parse()?))
        .init();

    let cli = Cli::parse();
    let config = load_config(&cli.config)?;

    match cli.command {
        Command::Init => cmd_init(&config),
        Command::Serve => cmd_serve(&config).await,
        Command::Remember { content, r#type, importance, project, tag } => {
            cmd_remember(&config, &content, &r#type, importance, project.as_deref(), &tag).await
        }
        Command::Recall { query, limit } => cmd_recall(&config, &query, limit).await,
        Command::Forget { id } => cmd_forget(&config, &id),
        Command::Demo { count, db, force } => cmd_demo(&config, count, db, force).await,
        Command::Export { format } => cmd_export(&config, &format),
        Command::Consolidate { project } => cmd_consolidate(&config, project.as_deref()).await,
        Command::Decay => cmd_decay(&config),
        Command::Status => cmd_status(&config),
    }
}

fn cmd_init(config: &Config) -> Result<()> {
    config.ensure_dirs().context("failed to create directories")?;

    // Create default config if it doesn't exist
    let config_path = Config::default_config_path();
    if !config_path.exists() {
        if let Some(dir) = config_path.parent() {
            std::fs::create_dir_all(dir).context("failed to create config directory")?;
        }
        let toml = toml::to_string_pretty(config).unwrap_or_default();
        std::fs::write(&config_path, toml).context("failed to write config")?;
        println!("Config created: {}", config_path.display());
    } else {
        println!("Config already exists: {}", config_path.display());
    }

    // Initialize DB
    Store::open(&config.db_path).context("failed to initialize database")?;
    println!("Database initialized: {}", config.db_path.display());

    Ok(())
}

/// Load the embedding model, degrading to FTS-only operation on failure
/// (e.g. first run while offline).
async fn try_embedder(config: &Config) -> Option<Arc<Embedder>> {
    match Embedder::new(config).await {
        Ok(e) => Some(Arc::new(e)),
        Err(e) => {
            tracing::warn!(error = %e, "embedding model unavailable — running keyword-only");
            None
        }
    }
}

async fn cmd_serve(config: &Config) -> Result<()> {
    config.ensure_dirs()?;
    poneglyph_http::validate_security(config)?;

    let store = Store::open(&config.db_path).context("failed to open database")?;
    let store = Arc::new(Mutex::new(store));
    let embedder = try_embedder(config).await;
    let shared_config = Arc::new(config.clone());

    // Background worker on its own connection (WAL): edges always, LLM
    // enrichment only when enabled in config.
    let (enrich, worker) = poneglyph_core::enrich::spawn_worker(
        config.db_path.clone(),
        poneglyph_core::enrich::WorkerConfig {
            edges: config.memory.edges.clone(),
            llm: config.llm.clone(),
            enrichment: config.enrichment.clone(),
        },
    );

    // Bind HTTP up front so AddrInUse can degrade instead of killing MCP:
    // a second editor spawning `poneglyph serve` shares the other instance's
    // HTTP server (same DB) and runs MCP-only here.
    let listener = match poneglyph_http::bind(config).await {
        Ok(l) => Some(l),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && config.dashboard.mcp => {
            tracing::warn!(
                port = config.dashboard.port,
                "HTTP port busy — another poneglyph instance is serving HTTP; continuing MCP-only"
            );
            None
        }
        Err(e) => return Err(e).context("failed to bind HTTP server"),
    };

    let http_state = poneglyph_http::AppState {
        store: Arc::clone(&store),
        embedder: embedder.clone(),
        config: Arc::clone(&shared_config),
        enrich: Some(enrich.clone()),
    };
    let http = async move {
        match listener {
            Some(l) => poneglyph_http::serve_on(l, http_state).await,
            None => std::future::pending().await,
        }
    };

    // NOTE: stdout belongs to MCP JSON-RPC from here on — no println!.
    let result = if config.dashboard.mcp {
        let mcp = poneglyph_mcp::tools::PoneglyphMcp::new(store, embedder, shared_config)
            .with_enrich(enrich);
        tokio::select! {
            // MCP client disconnect owns the process lifetime.
            r = poneglyph_mcp::server::run_stdio(mcp) => r,
            r = http => r,
        }
    } else {
        // HTTP-only daemon mode (server.mcp = false): run until Ctrl-C.
        tokio::select! {
            r = http => r,
            _ = tokio::signal::ctrl_c() => Ok(()),
        }
    };

    worker.abort(); // clients gone; no more producers
    result
}

async fn cmd_demo(config: &Config, count: usize, db: Option<PathBuf>, force: bool) -> Result<()> {
    let is_default_db = db.is_none();
    let db_path = match db {
        Some(path) => path,
        None => config.db_path.clone(),
    };

    config.ensure_dirs()?;

    // Guard: refuse to seed a non-empty DB unless --force or --db given.
    if db_path.exists() && !force && is_default_db {
        let probe = Store::open(&db_path)?;
        let existing = probe.stats()?.memory_count;
        if existing > 0 {
            anyhow::bail!(
                "Database already has {existing} memories. Pass --force to re-seed or --db <path> to target a different database."
            );
        }
    }

    let store = Store::open(&db_path).context("failed to open database")?;
    let embedder = try_embedder(config).await;

    println!("Seeding {count} demo memories…");
    let outcome = {
        let mut embed_fn;
        let embed: Option<&mut dyn FnMut(&str) -> Result<Vec<f32>>> = match &embedder {
            Some(e) => {
                let e = Arc::clone(e);
                let rt = tokio::runtime::Handle::current();
                embed_fn = move |text: &str| rt.block_on(e.embed_text(text));
                Some(&mut embed_fn)
            }
            None => None,
        };
        tokio::task::block_in_place(|| demo::seed(&store, count, &config.memory.edges, embed))?
    };

    println!(
        "Seeded {} memories, {} edges, {} projects into {}.",
        outcome.memories, outcome.edges, outcome.projects, db_path.display()
    );

    if is_default_db {
        println!("Run `poneglyph serve` to view the data.");
    }

    Ok(())
}

async fn cmd_remember(
    config: &Config,
    content: &str,
    memory_type: &str,
    importance: f64,
    project: Option<&str>,
    tags: &[String],
) -> Result<()> {
    config.ensure_dirs()?;
    let store = Store::open(&config.db_path)?;
    let embedder = try_embedder(config).await;

    let mem_type: MemoryType = memory_type.parse().unwrap_or(MemoryType::Semantic);

    // Resolve project (path → git-remote identity fallback).
    let project_id = match project {
        Some(path) => Some(poneglyph_core::project::detect_project(&store, path)?.id),
        None => None,
    };

    let metadata = if !tags.is_empty() {
        Some(serde_json::json!({ "tags": tags }))
    } else {
        None
    };

    let mem = store.create_memory(
        content,
        mem_type,
        importance,
        Source::Cli,
        project_id.as_deref(),
        metadata.as_ref(),
    )?;

    // Index FTS + vector
    store.index_fts(&mem.id, content)?;
    if let Some(embedder) = &embedder {
        let vec = embedder.embed_text(content).await?;
        store.index_embedding(&mem.id, &vec)?;
    }

    // One-shot process: enqueue the edge job, then drain inline so edges
    // exist without a running server (no-LLM builders are cheap).
    poneglyph_core::enrich::enqueue_compute_edges(&store, &mem.id)?;
    poneglyph_core::enrich::process_pending_jobs(&store, &config.memory.edges)?;

    println!("{}", mem.id);
    Ok(())
}

async fn cmd_recall(config: &Config, query: &str, limit: usize) -> Result<()> {
    config.ensure_dirs()?;
    let store = Store::open(&config.db_path)?;
    let embedder = try_embedder(config).await;

    let query_vec = match &embedder {
        Some(e) => Some(e.embed_text(query).await?),
        None => None,
    };

    let filters = poneglyph_core::retrieve::RecallFilters::default();
    let results = poneglyph_core::retrieve::recall(
        &store.conn,
        query_vec.as_deref(),
        query,
        &filters,
        limit,
    )?;

    if results.is_empty() {
        println!("No memories found.");
        return Ok(());
    }

    for r in &results {
        // Full id: UUIDv7 prefixes are timestamps, so short prefixes collide.
        println!("[{:.4}] {} — {}", r.score, r.memory.id, truncate(&r.memory.content, 80));
    }

    Ok(())
}

fn cmd_forget(config: &Config, id: &str) -> Result<()> {
    config.ensure_dirs()?;
    let store = Store::open(&config.db_path)?;

    let deleted = store.delete_memory(id)?;
    if deleted {
        println!("Deleted: {id}");
    } else {
        println!("Not found: {id}");
    }

    Ok(())
}

fn cmd_export(config: &Config, format: &str) -> Result<()> {
    config.ensure_dirs()?;
    let store = Store::open(&config.db_path)?;

    let (memories, _) = store.list_memories(None, None, 10_000, 0)?;

    match format {
        "json" => {
            let json = serde_json::to_string_pretty(&memories)?;
            println!("{json}");
        }
        "md" => {
            for mem in &memories {
                println!("## {}\n", &mem.id[..8]);
                println!("{}\n", mem.content);
                println!("Type: {} | Importance: {} | Created: {}\n", mem.memory_type, mem.importance, mem.created_at);
                println!("---\n");
            }
        }
        _ => println!("Unknown format: {format} (use json or md)"),
    }

    Ok(())
}

async fn cmd_consolidate(config: &Config, project_path: Option<&str>) -> Result<()> {
    config.ensure_dirs()?;
    let store = Store::open(&config.db_path)?;
    let embedder = try_embedder(config).await;

    // Resolve project
    let project_id = match project_path {
        Some(path) => Some(poneglyph_core::project::detect_project(&store, path)?.id),
        None => None,
    };

    if let Some(pid) = &project_id {
        let results = poneglyph_core::consolidate::consolidate_project(
            &store,
            pid,
            config,
            embedder.as_deref(),
        ).await?;

        if results.is_empty() {
            println!("No clusters found to consolidate.");
        } else {
            println!("Consolidated {} clusters:", results.len());
            for r in &results {
                println!("  decoy {} — {} children: {}", &r.decoy_id[..8], r.child_count, truncate(&r.summary, 60));
            }
        }
    } else {
        // Consolidate all projects
        let projects = store.list_projects()?;
        let mut total_consolidated = 0;

        for project in &projects {
            let results = poneglyph_core::consolidate::consolidate_project(
                &store,
                &project.id,
                config,
                embedder.as_deref(),
            ).await?;

            if !results.is_empty() {
                println!("Project {}:", project.name);
                for r in &results {
                    println!("  decoy {} — {} children", &r.decoy_id[..8], r.child_count);
                }
                total_consolidated += results.len();
            }
        }

        if total_consolidated == 0 {
            println!("No clusters found to consolidate across any project.");
        } else {
            println!("\nTotal: {} clusters consolidated.", total_consolidated);
        }
    }

    Ok(())
}

fn cmd_decay(config: &Config) -> Result<()> {
    config.ensure_dirs()?;
    let store = Store::open(&config.db_path)?;

    let report = poneglyph_core::consolidate::run_decay(&store, config)?;

    println!("Decay report:");
    println!("  Strengths updated: {}", report.strengths_updated);
    println!("  Archived to cold:  {}", report.archived);
    println!("  Pruned (very low): {}", report.pruned);

    Ok(())
}

fn cmd_status(config: &Config) -> Result<()> {
    config.ensure_dirs()?;
    let store = Store::open(&config.db_path)?;
    let stats = store.stats()?;

    println!("Database:    {}", config.db_path.display());
    println!("Model:       {}", config.embedding.model_id);
    println!("Dimensions:  {}", config.embedding.dimensions);
    println!("Memories:    {}", stats.memory_count);
    println!("Edges:       {}", stats.edge_count);
    println!("Projects:    {}", stats.project_count);
    println!("Pending jobs:{}", stats.pending_jobs);
    println!("Enrichment:  {}", if config.llm.enabled { "on" } else { "off" });

    if Config::using_legacy_paths() {
        println!();
        println!("Note: data lives at a legacy (pre-XDG) location.");
        println!("Move it with poneglyph stopped, e.g.:");
        println!("  mv ~/Library/'Application Support'/poneglyph/poneglyph.db ~/.local/share/poneglyph/");
    }

    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
