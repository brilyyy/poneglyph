mod demo;
mod detect;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing_subscriber::EnvFilter;

use poneglyph_core::config::Config;
use poneglyph_core::embed::Embedder;
use poneglyph_core::llm::LlmClient;
use poneglyph_core::model::{MemoryType, Source};
use poneglyph_core::store::Store;

/// Stone-tablet emblem, ASCII-rendered from viewer/public/logo.svg.
/// Printed only on `init` — `mcp`'s stdout is reserved for MCP JSON-RPC.
const BANNER: &str = include_str!("banner.txt");

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
    /// Start the MCP server (stdio) — for editor/agent integration
    Mcp,
    /// Start the web dashboard + graph viewer (HTTP) — for browsing in person
    #[cfg(feature = "viewer")]
    Viewer,
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
    /// Get ranked project context (for hooks / session injection)
    Context {
        /// Project path
        #[arg(long)]
        project: String,
        /// Token budget
        #[arg(long, default_value = "600")]
        max_tokens: usize,
    },
    /// Seed sample data into a database (no server; run `poneglyph viewer` to view)
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
    /// Code knowledge graph (Tree-sitter) — distinct from the memory graph
    Graph {
        #[command(subcommand)]
        action: GraphCommand,
    },
    /// Wire up an IDE/agent with poneglyph (MCP server, hooks, plugin, skill)
    Wire {
        /// IDE to wire: claude-code, opencode, cursor, gemini, codex, copilot
        ide: String,
    },
    /// Generate or display session summaries (for hooks)
    SessionSummary {
        /// Project path to scope the summary to
        #[arg(long)]
        project: Option<String>,
        /// Show the most recent session summary instead of generating a new one
        #[arg(long)]
        latest: bool,
    },
}

#[derive(Subcommand)]
enum GraphCommand {
    /// Full build: parse every matching file under `path`
    Init {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Incremental build: only reparse files whose content changed
    Update {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Watch `path` and incrementally rebuild on change (debounced)
    Watch {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Structured (callers_of:/callees_of:/imports_of:/tests_for:/path:<a>..<b>) or keyword query
    Query {
        q: String,
    },
    /// Recursive caller/importer/test trace from a file or symbol
    BlastRadius {
        target: String,
        #[arg(long)]
        depth: Option<usize>,
    },
    /// Export the graph as json, dot, or graphml
    Export {
        #[arg(long, default_value = "json")]
        format: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn load_config(cli_config: &Option<PathBuf>) -> Result<Config> {
    match cli_config {
        Some(path) => Config::load_from(path),
        None => Config::load(),
    }
}

#[tokio::main]
async fn main() {
    // Log to stderr: `poneglyph mcp` speaks MCP JSON-RPC on stdout.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("poneglyph=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    let config = match load_config(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            let err: poneglyph_core::error::PoneglyphError = e.into();
            eprintln!("error: {err}");
            std::process::exit(err.exit_code());
        }
    };

    let result = run(cli.command, &config).await;
    if let Err(e) = result {
        let err: poneglyph_core::error::PoneglyphError = e.into();
        match err.kind {
            poneglyph_core::error::ErrorKind::Internal => {
                eprintln!("error: {err}");
                if tracing::enabled!(tracing::Level::DEBUG) {
                    if let Some(source) = &err.source {
                        eprintln!("  caused by: {source:#}");
                    }
                }
            }
            _ => {
                eprintln!("error: {err}");
            }
        }
        std::process::exit(err.exit_code());
    }
}

async fn run(command: Command, config: &Config) -> Result<()> {
    match command {
        Command::Init => cmd_init(&config),
        Command::Mcp => cmd_mcp(&config).await,
        #[cfg(feature = "viewer")]
        Command::Viewer => cmd_viewer(&config).await,
        Command::Remember { content, r#type, importance, project, tag } => {
            cmd_remember(&config, &content, &r#type, importance, project.as_deref(), &tag).await
        }
        Command::Recall { query, limit } => cmd_recall(&config, &query, limit).await,
        Command::Forget { id } => cmd_forget(&config, &id),
        Command::Demo { count, db, force } => cmd_demo(&config, count, db, force).await,
        Command::Export { format } => cmd_export(&config, &format),
        Command::Context { project, max_tokens } => cmd_context(&config, &project, max_tokens).await,
        Command::Consolidate { project } => cmd_consolidate(&config, project.as_deref()).await,
        Command::Decay => cmd_decay(&config),
        Command::Status => cmd_status(&config),
        Command::Graph { action } => cmd_graph(&config, action),
        Command::Wire { ide } => cmd_wire(&config, &ide),
        Command::SessionSummary { project, latest } => cmd_session_summary(&config, project.as_deref(), latest).await,
    }
}

fn cmd_init(config: &Config) -> Result<()> {
    println!("\x1b[38;2;153;0;17m{BANNER}\x1b[0m");

    config.ensure_dirs().context("failed to create directories")?;

    // Create default config if it doesn't exist: every key present but
    // commented, except values resolved by local-provider detection.
    let config_path = Config::default_config_path();
    if !config_path.exists() {
        if let Some(dir) = config_path.parent() {
            std::fs::create_dir_all(dir).context("failed to create config directory")?;
        }
        let detected = detect::detect_local_llm();
        let toml = detect::render_config_template(&detected);
        std::fs::write(&config_path, toml).context("failed to write config")?;
        println!("Config created: {}", config_path.display());
    } else {
        println!("Config already exists: {}", config_path.display());
    }

    // Initialize DB
    Store::open(&config.db_path).context("failed to initialize database")?;
    println!("Database initialized: {}", config.db_path.display());

    // Create project-local .poneglyphignore (skip if exists).
    let ignore_path = std::path::Path::new(".poneglyphignore");
    if !ignore_path.exists() {
        std::fs::write(ignore_path, "node_modules/\ntarget/\nvendor/\n.git/\ndist/\nbuild/\nout/\n.DS_Store\n")?;
        println!("Created: .poneglyphignore");
    }

    // Create .poneglyph/code-graph-lock.json (skip if exists).
    let lock_dir = std::path::Path::new(".poneglyph");
    let lock_path = lock_dir.join("code-graph-lock.json");
    if !lock_path.exists() {
        std::fs::create_dir_all(lock_dir).context("failed to create .poneglyph directory")?;
        let lock = serde_json::json!({
            "version": 1,
            "created_at": chrono::Utc::now().to_rfc3339(),
            "last_build": null,
            "languages": ["rust", "typescript", "javascript", "python", "go"]
        });
        std::fs::write(&lock_path, serde_json::to_string_pretty(&lock)?).context("failed to write code-graph-lock.json")?;
        println!("Created: .poneglyph/code-graph-lock.json");
    }

    println!("\nNext: run `poneglyph wire <ide>` to set up IDE integration.");
    Ok(())
}

fn cmd_wire(_config: &Config, ide: &str) -> Result<()> {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "poneglyph".to_string());
    let hooks_dir = Config::config_dir().join("hooks");

    let outcome = detect::wire_agent(ide, &hooks_dir, &exe)?;
    println!("{:<14} {}", outcome.agent, outcome.status.as_str());

    // Auto-inject rules into global agent rule file.
    if let Some(home) = detect::home_dir() {
        match detect::inject_global_rules(ide, &home) {
            Ok(changed) => {
                if changed {
                    println!("{:<14} rules injected", ide);
                } else {
                    println!("{:<14} rules already up to date", ide);
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to inject global rules");
            }
        }
    }

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

/// Open the store + embedder and spawn the background enrich worker — the
/// setup shared by `mcp` (MCP) and `viewer` (HTTP), each otherwise
/// running standalone in its own process.
async fn open_store_and_worker(
    config: &Config,
) -> Result<(
    Arc<Mutex<Store>>,
    Option<Arc<Embedder>>,
    Arc<Config>,
    poneglyph_core::enrich::EnrichHandle,
    tokio::task::JoinHandle<()>,
)> {
    config.ensure_dirs()?;
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
            compression_enabled: config.memory.compression_enabled,
            compression_mode: config.memory.compression_mode,
        },
    );

    Ok((store, embedder, shared_config, enrich, worker))
}

/// MCP stdio server only — for editor/agent integration. `poneglyph viewer`
/// runs the HTTP dashboard as a separate, independent process.
async fn cmd_mcp(config: &Config) -> Result<()> {
    let (store, embedder, shared_config, enrich, worker) = open_store_and_worker(config).await?;

    // NOTE: stdout belongs to MCP JSON-RPC from here on — no println!.
    let mcp = poneglyph_mcp::tools::PoneglyphMcp::new(store, embedder, shared_config).with_enrich(enrich);
    let result = poneglyph_mcp::server::run_stdio(mcp).await;

    worker.abort(); // client gone; no more producers
    result
}

/// HTTP dashboard + graph viewer only — for browsing in a browser.
/// `poneglyph mcp` runs the MCP server as a separate, independent process.
#[cfg(feature = "viewer")]
async fn cmd_viewer(config: &Config) -> Result<()> {
    poneglyph_http::validate_security(config)?;
    let (store, embedder, shared_config, enrich, worker) = open_store_and_worker(config).await?;

    let listener = poneglyph_http::bind(config).await.context("failed to bind HTTP server")?;
    let http_state = poneglyph_http::AppState {
        store,
        embedder,
        config: shared_config,
        enrich: Some(enrich),
    };

    println!("Viewer listening on http://{}:{}", config.dashboard.host, config.dashboard.port);
    let result = tokio::select! {
        r = poneglyph_http::serve_on(listener, http_state) => r,
        _ = tokio::signal::ctrl_c() => Ok(()),
    };

    worker.abort(); // client gone; no more producers
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
                embed_fn = move |text: &str| rt.block_on(e.embed_passage(text));
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
        println!("Run `poneglyph viewer` to view the data.");
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

    let exclude_matcher = poneglyph_core::privacy::build_exclude_matcher(&config.privacy.exclude_paths);
    if poneglyph_core::privacy::content_references_excluded_path(content, &exclude_matcher) {
        anyhow::bail!("refusing to store: content references an excluded path (see [privacy].exclude_paths)");
    }
    let content = poneglyph_core::privacy::redact_content(content, &config.privacy);

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
        &content,
        mem_type,
        importance,
        Source::Cli,
        project_id.as_deref(),
        metadata.as_ref(),
    )?;

    // Index FTS + vector
    store.index_fts(&mem.id, &content)?;
    if let Some(embedder) = &embedder {
        let vec = embedder.embed_passage(&content).await?;
        store.index_embedding(&mem.id, &vec)?;
    }

    // One-shot process: enqueue the edge job, then drain inline so edges
    // exist without a running server (no-LLM builders are cheap).
    poneglyph_core::enrich::enqueue_compute_edges(&store, &mem.id)?;
    poneglyph_core::enrich::process_pending_jobs(&store, &config.memory.edges)?;

    // Caveman compression runs inline above; semantic compression enqueues a
    // job for whichever resident `serve` worker drains it next.
    if config.memory.compression_enabled {
        poneglyph_core::enrich::enqueue_compression(&store, &mem.id, config.memory.compression_mode)?;
    }

    println!("{}", mem.id);
    Ok(())
}

async fn cmd_recall(config: &Config, query: &str, limit: usize) -> Result<()> {
    config.ensure_dirs()?;
    let store = Store::open(&config.db_path)?;
    let embedder = try_embedder(config).await;

    let query_vec = match &embedder {
        Some(e) => Some(e.embed_query(query).await?),
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

async fn cmd_context(config: &Config, project: &str, max_tokens: usize) -> Result<()> {
    config.ensure_dirs()?;
    let store = Store::open(&config.db_path)?;

    let (context, _memory_count) =
        poneglyph_core::project::get_project_context(&store, project, max_tokens)?;

    if !context.is_empty() {
        print!("{context}");
    }

    // Nudge toward graph init if the code graph is empty.
    let stats = store.stats()?;
    if stats.edge_count == 0 {
        eprintln!("poneglyph: code graph is empty — run `poneglyph graph init` to enable codegraph_query/codegraph_blast_radius.");
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

fn cmd_graph(config: &Config, action: GraphCommand) -> Result<()> {
    use poneglyph_core::codegraph;

    config.ensure_dirs()?;
    let store = Store::open(&config.db_path)?;

    match action {
        GraphCommand::Init { path } => {
            let report = codegraph::build(&store, &path, &config.graph, true)?;
            print_build_report(&report);
            update_graph_lock();
        }
        GraphCommand::Update { path } => {
            let report = codegraph::build(&store, &path, &config.graph, false)?;
            print_build_report(&report);
            update_graph_lock();
        }
        GraphCommand::Watch { path } => cmd_graph_watch(&store, &path, config)?,
        GraphCommand::Query { q } => {
            let query = codegraph::parse_query(&q);
            let results = codegraph::run_query(&store, &query)?;
            if results.is_empty() {
                println!("No matches.");
            }
            for n in &results {
                println!("[{}] {} — {}:{}", n.kind, n.name, n.file_path, n.start_line);
            }
        }
        GraphCommand::BlastRadius { target, depth } => {
            let depth = depth.unwrap_or(config.graph.blast_radius_depth);
            let report = codegraph::blast_radius(&store, &target, depth)?;
            if report.root.is_empty() {
                println!("No file or symbol matching '{target}' found in the graph.");
                return Ok(());
            }
            println!("Root ({} symbol(s)):", report.root.len());
            for n in &report.root {
                println!("  [{}] {} — {}:{}", n.kind, n.name, n.file_path, n.start_line);
            }
            println!("\nDependents ({}):", report.dependents.len());
            for d in &report.dependents {
                println!("  depth {} [{}] {} — {}:{}", d.depth, d.node.kind, d.node.name, d.node.file_path, d.node.start_line);
            }
            println!("\nTests ({}):", report.tests.len());
            for t in &report.tests {
                println!("  {} — {}:{}", t.name, t.file_path, t.start_line);
            }
        }
        GraphCommand::Export { format, out } => {
            let rendered = match format.as_str() {
                "json" => codegraph::export_json(&store)?,
                "dot" => codegraph::export_dot(&store)?,
                "graphml" => codegraph::export_graphml(&store)?,
                other => anyhow::bail!("unknown export format '{other}' (use json, dot, or graphml)"),
            };
            match out {
                Some(path) => {
                    std::fs::write(&path, rendered).with_context(|| format!("failed to write {}", path.display()))?;
                    println!("Exported: {}", path.display());
                }
                None => println!("{rendered}"),
            }
        }
    }

    Ok(())
}

fn print_build_report(report: &poneglyph_core::codegraph::BuildReport) {
    println!(
        "Parsed {} file(s), {} unchanged, {} removed. {} node(s), {} edge(s).",
        report.files_parsed, report.files_unchanged, report.files_removed, report.nodes, report.edges
    );
}

/// Blocks the calling thread, rebuilding incrementally after each debounced
/// burst of filesystem events, until Ctrl-C.
fn cmd_graph_watch(store: &Store, path: &std::path::Path, config: &Config) -> Result<()> {
    use notify::{RecursiveMode, Watcher};

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx).context("failed to start file watcher")?;
    watcher.watch(path, RecursiveMode::Recursive).context("failed to watch path")?;

    println!("Watching {} (Ctrl-C to stop)...", path.display());
    let debounce = std::time::Duration::from_millis(config.graph.watch_delay_ms);
    loop {
        // Block for the first event, then drain whatever else arrives within the debounce window.
        if rx.recv().is_err() {
            break;
        }
        while rx.recv_timeout(debounce).is_ok() {}

        match poneglyph_core::codegraph::build(store, path, &config.graph, false) {
            Ok(report) => {
                print_build_report(&report);
                update_graph_lock();
            }
            Err(e) => tracing::warn!(error = %e, "graph update failed"),
        }
    }

    Ok(())
}

/// Update `last_build` timestamp in `.poneglyph/code-graph-lock.json`.
fn update_graph_lock() {
    let lock_path = std::path::Path::new(".poneglyph/code-graph-lock.json");
    if !lock_path.exists() {
        return;
    }
    let Ok(raw) = std::fs::read_to_string(lock_path) else { return };
    let Ok(mut lock) = serde_json::from_str::<serde_json::Value>(&raw) else { return };
    lock["last_build"] = serde_json::json!(chrono::Utc::now().to_rfc3339());
    let _ = std::fs::write(lock_path, serde_json::to_string_pretty(&lock).unwrap_or_default());
}

fn extractive_summary(top: &[&poneglyph_core::model::Memory]) -> String {
    let text = top.iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n---\n");
    if text.len() > 2000 {
        format!("{}...", &text[..2000])
    } else {
        text
    }
}

async fn cmd_session_summary(config: &Config, project_path: Option<&str>, latest: bool) -> Result<()> {
    config.ensure_dirs()?;
    let store = Store::open(&config.db_path)?;

    // Resolve project
    let project_id = match project_path {
        Some(path) => Some(poneglyph_core::project::detect_project(&store, path)?.id),
        None => None,
    };

    if latest {
        // Show the most recent session-summary memory
        let (memories, _) = store.list_memories(project_id.as_deref(), Some("semantic"), 50, 0)?;
        let summary = memories.iter().find(|m| {
            m.metadata.as_ref()
                .and_then(|meta| meta.get("tags"))
                .and_then(|tags| tags.as_array())
                .map(|arr| arr.iter().any(|t| t.as_str() == Some("session-summary")))
                .unwrap_or(false)
        });

        match summary {
            Some(mem) => print!("{}", mem.content),
            None => { /* no summary yet, silent */ }
        }
        return Ok(());
    }

    // Generate a new extractive summary from recent session memories
    let (memories, _) = store.list_memories(project_id.as_deref(), None, 30, 0)?;

    // Filter to real memories (not decoys, not session-summary tags)
    let real: Vec<_> = memories.iter().filter(|m| {
        !m.is_decoy && !m.metadata.as_ref()
            .and_then(|meta| meta.get("tags"))
            .and_then(|tags| tags.as_array())
            .map(|arr| arr.iter().any(|t| t.as_str() == Some("session-summary")))
            .unwrap_or(false)
    }).collect();

    if real.is_empty() {
        return Ok(());
    }

    // Sort by importance, take top 5
    let mut sorted = real.clone();
    sorted.sort_by(|a, b| b.importance.partial_cmp(&a.importance).unwrap_or(std::cmp::Ordering::Equal));
    let top: Vec<_> = sorted.iter().take(5).copied().collect();

    // Try LLM summarization; fall back to extractive join
    let llm = LlmClient::from_config(&config.llm);
    let summary_text = if let Some(client) = &llm {
        let memories_text = top.iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n");
        match client.complete(
            "You summarize coding sessions for a developer's memory store. \
             Given a few memories from one session, reply with a concise summary \
             of what was worked on. Plain text, no preamble, 2-4 sentences.",
            &memories_text,
        ).await {
            Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => extractive_summary(&top), // LLM failed, fall back
        }
    } else {
        extractive_summary(&top)
    };

    // Store as a tagged memory
    let metadata = serde_json::json!({ "tags": ["session-summary"] });
    let mem = store.create_memory(
        &summary_text,
        MemoryType::Semantic,
        0.5,
        Source::Cli,
        project_id.as_deref(),
        Some(&metadata),
    )?;
    store.index_fts(&mem.id, &summary_text)?;

    // Enqueue edges (non-blocking)
    let _ = poneglyph_core::enrich::enqueue_compute_edges(&store, &mem.id);

    // Enqueue LLM enrichment if available
    if llm.is_some() {
        let _ = poneglyph_core::enrich::enqueue_llm_jobs(&store, &mem.id);
    }

    println!("{}", mem.id);
    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
