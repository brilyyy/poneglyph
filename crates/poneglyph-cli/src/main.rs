mod daemon;
mod demo;
mod detect;
mod eval;
mod graph_registry;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
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

/// Full-color logo for the no-subcommand default view.
const LOGO: &str = include_str!("logo.ans");

#[derive(Parser)]
#[command(name = "poneglyph", version, about = "Local AI memory engine")]
struct Cli {
    /// No subcommand: starts MCP + viewer together (see `cmd_default`).
    #[command(subcommand)]
    command: Option<Command>,
    /// Path to an explicit config file to load instead of the default search
    #[arg(long = "config-file", global = true)]
    config_file: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize the global database and inject usage rules into
    /// CLAUDE.md/AGENTS.md/.cursorrules if present (run `graph init` for
    /// `.poneglyphignore` / code-graph lock)
    Init {
        /// Also (re)write the global config at ~/.config/poneglyph/config.toml
        #[arg(long)]
        config: bool,
        /// Also link global agent rules via ~/.claude/CLAUDE.poneglyph.md /
        /// ~/.config/opencode/AGENTS.poneglyph.md (claude-code, opencode only)
        #[arg(short = 'g', long)]
        global_rules: bool,
        /// Inject rules into this project path instead of the current directory
        #[arg(short = 'p', long, value_name = "PATH")]
        project: Option<PathBuf>,
    },
    /// Manage the `poneglyph mcp` daemon as a login service (launchd/systemd)
    Daemon {
        #[command(subcommand)]
        action: DaemonCommand,
    },
    /// Start the MCP server — Streamable HTTP on `agents.mcp_server_port`
    /// (default 27271) by default, a persistent daemon agents connect to
    /// over the network. Also serves /api, /ingest, /healthz so hooks can
    /// talk to one always-on process. Pass --stdio for the old
    /// editor-spawned-per-session shape.
    Mcp {
        #[arg(long)]
        stdio: bool,
    },
    /// Start the web dashboard + graph viewer (HTTP) — for browsing in person
    #[cfg(feature = "viewer")]
    Viewer,
    /// Store, search, and manage memories
    Memory {
        #[command(subcommand)]
        action: MemoryCommand,
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
    /// Evaluate retrieval against a LongMemEval-style dataset (R@1/R@5/R@10/MRR)
    Eval {
        /// Path to a LongMemEval JSON file (top-level array of instances)
        #[arg(long)]
        dataset: PathBuf,
        /// Only evaluate the first N instances (omit for the full dataset)
        #[arg(long)]
        limit: Option<usize>,
        /// Print the summary as JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Show status
    Status,
    /// Remove dead project registrations (graph_projects.toml entries and
    /// `projects` rows whose directory no longer exists on disk)
    Cleanup,
    /// Code knowledge graph (Tree-sitter) — distinct from the memory graph
    Graph {
        #[command(subcommand)]
        action: GraphCommand,
    },
    /// Wire up an agent with poneglyph: MCP server registration, hook
    /// scripts, and/or the skill file
    Wire {
        target: detect::WireTarget,
        /// Agent to wire: claude-code, opencode, cursor, gemini, codex,
        /// copilot, or '*' for every agent compiled into this binary
        #[arg(long)]
        agent: String,
    },
}

#[derive(Subcommand)]
enum MemoryCommand {
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
    Forget { id: String },
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
    /// Consolidate similar memories into schema decoys
    Consolidate {
        /// Project path to consolidate (all projects if omitted)
        #[arg(long)]
        project: Option<String>,
    },
    /// Run decay: update strengths and archive low-strength memories
    Decay,
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
enum DaemonCommand {
    /// Register `poneglyph mcp` as a login service and start it now
    Enable,
    /// Stop the service and remove it from login startup
    Disable,
    /// Start the already-registered service
    Start,
    /// Stop the running service (stays registered for next login)
    Stop,
    /// Show service + liveness status
    Status,
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
    /// Structured (callers_of:/callees_of:/imports_of:/tests_for:/subtypes_of:/supertypes_of:/path:<a>..<b>) or keyword query
    Query {
        q: String,
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    /// Recursive caller/importer/test trace from a file or symbol
    BlastRadius {
        target: String,
        #[arg(long)]
        depth: Option<usize>,
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    /// Export the graph as json, dot, or graphml
    Export {
        #[arg(long, default_value = "json")]
        format: String,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    /// Everything about a symbol in one call: source snippet, callers/callees,
    /// supertypes/subtypes, covering tests, and bounded blast radius
    Explore {
        target: String,
        #[arg(long)]
        depth: Option<usize>,
        #[arg(long, default_value = ".")]
        path: PathBuf,
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

    let config = match load_config(&cli.config_file) {
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

async fn run(command: Option<Command>, config: &Config) -> Result<()> {
    let Some(command) = command else {
        return cmd_default(config).await;
    };
    match command {
        Command::Init { config: write_global_config, global_rules, project } => {
            cmd_init(&config, write_global_config, global_rules, project.as_deref())
        }
        Command::Daemon { action } => cmd_daemon(&config, action),
        Command::Mcp { stdio } => cmd_mcp(&config, stdio).await,
        #[cfg(feature = "viewer")]
        Command::Viewer => cmd_viewer(&config).await,
        Command::Memory { action } => cmd_memory(&config, action).await,
        Command::Demo { count, db, force } => cmd_demo(&config, count, db, force).await,
        Command::Eval { dataset, limit, json } => cmd_eval(&config, &dataset, limit, json).await,
        Command::Status => cmd_status(&config).await,
        Command::Cleanup => cmd_cleanup(&config),
        Command::Graph { action } => cmd_graph(&config, action),
        Command::Wire { target, agent } => cmd_wire(&config, target, &agent),
    }
}

/// Global DB init + rule injection. `--config` additionally (re)writes the
/// global config; `-g` links global agent rules via a sibling
/// `{CLAUDE/AGENTS}.poneglyph.md` file; `-p` targets another project path
/// instead of the current directory. `.poneglyphignore` / code-graph lock
/// creation lives in `graph init` now, not here.
fn cmd_init(
    config: &Config,
    write_global_config: bool,
    global_rules: bool,
    project: Option<&Path>,
) -> Result<()> {
    println!("\x1b[38;2;153;0;17m{BANNER}\x1b[0m");

    if write_global_config {
        init_global_config()?;
    }

    config
        .ensure_dirs()
        .context("failed to create directories")?;

    // Initialize DB
    Store::open(&config.db_path).context("failed to initialize database")?;
    println!("Database initialized: {}", config.db_path.display());

    // Inject usage rules into whichever of CLAUDE.md/AGENTS.md/.cursorrules
    // already exist in this project. Never creates a file the user doesn't have.
    let target_dir = match project {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().context("failed to read current directory")?,
    };
    match detect::inject_agent_rules(&target_dir) {
        Ok(results) if results.is_empty() => {}
        Ok(results) => {
            for (file, changed) in results {
                println!(
                    "{file}: {}",
                    if changed {
                        "rules injected"
                    } else {
                        "rules already up to date"
                    }
                );
            }
        }
        Err(e) => tracing::warn!(error = %e, "failed to inject project rules"),
    }

    if global_rules {
        if let Some(home) = detect::home_dir() {
            for ide in ["claude-code", "opencode"] {
                match detect::inject_global_rules_import(ide, &home) {
                    Ok(changed) => println!(
                        "{ide}: {}",
                        if changed { "global rules linked" } else { "global rules already linked" }
                    ),
                    Err(e) => tracing::warn!(error = %e, ide, "failed to link global rules"),
                }
            }
        }
    }

    if !write_global_config && !Config::default_config_path().exists() {
        println!("\nNo global config yet — run `poneglyph init --config` once to create one.");
    }
    println!("\nNext: run `poneglyph graph init` to build the code graph, `poneglyph wire all --agent <name>` to set up IDE integration.");
    Ok(())
}

/// Write `~/.config/poneglyph/config.toml`, prompting before overwriting a
/// file from an older template version (renamed to `config.toml.bak` first).
fn init_global_config() -> Result<()> {
    let config_path = Config::default_config_path();
    if config_path.exists() {
        let existing = std::fs::read_to_string(&config_path).unwrap_or_default();
        let existing_version = detect::parse_config_template_version(&existing);
        if existing_version == Some(detect::CURRENT_CONFIG_TEMPLATE_VERSION) {
            println!("Config already up to date: {}", config_path.display());
            return Ok(());
        }
        print!(
            "Existing config at {} predates the bundled template (version {} vs {}). Overwrite? [y/N] ",
            config_path.display(),
            existing_version.map(|v| v.to_string()).unwrap_or_else(|| "unknown".to_string()),
            detect::CURRENT_CONFIG_TEMPLATE_VERSION
        );
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("Left unchanged.");
            return Ok(());
        }
        let backup_path = config_path.with_file_name("config.toml.bak");
        std::fs::rename(&config_path, &backup_path)
            .with_context(|| format!("failed to back up {} to {}", config_path.display(), backup_path.display()))?;
        println!("Backed up old config to {}", backup_path.display());
    } else if let Some(dir) = config_path.parent() {
        std::fs::create_dir_all(dir).context("failed to create config directory")?;
    }
    let detected = detect::detect_local_llm();
    let toml = detect::render_config_template(&detected);
    std::fs::write(&config_path, toml).context("failed to write config")?;
    println!("Config created: {}", config_path.display());
    Ok(())
}

fn cmd_daemon(config: &Config, action: DaemonCommand) -> Result<()> {
    match action {
        DaemonCommand::Enable => daemon::enable(config),
        DaemonCommand::Disable => daemon::disable(),
        DaemonCommand::Start => daemon::start(),
        DaemonCommand::Stop => daemon::stop(),
        DaemonCommand::Status => daemon::status(config),
    }
}

fn cmd_wire(config: &Config, target: detect::WireTarget, agent: &str) -> Result<()> {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "poneglyph".to_string());
    let hooks_dir = Config::config_dir().join("hooks");

    let agents: Vec<&str> = if agent == "*" {
        detect::all_agent_names()
    } else {
        vec![agent]
    };

    for a in agents {
        let outcome = detect::wire_agent_bucket(a, target, &hooks_dir, &exe, config.agents.mcp_server_port)?;
        println!("{:<14} {}", outcome.agent, outcome.status.as_str());
        if a == "claude-code"
            && matches!(outcome.status, detect::SetupStatus::Configured | detect::SetupStatus::AlreadyConfigured)
        {
            println!(
                "               note: `poneglyph mcp` is a persistent daemon — run \
                 `poneglyph daemon enable` to keep it running across logins, or start it \
                 manually (e.g. `poneglyph mcp &`), so Claude Code can reach \
                 http://127.0.0.1:{}/mcp",
                config.agents.mcp_server_port
            );
        }

        // Auto-inject rules into global agent rule file.
        if let Some(home) = detect::home_dir() {
            match detect::inject_global_rules(a, &home) {
                Ok(changed) => {
                    if changed {
                        println!("{a:<14} rules injected");
                    } else {
                        println!("{a:<14} rules already up to date");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, agent = a, "failed to inject global rules");
                }
            }
        }
    }

    Ok(())
}

async fn cmd_memory(config: &Config, action: MemoryCommand) -> Result<()> {
    match action {
        MemoryCommand::Remember {
            content,
            r#type,
            importance,
            project,
            tag,
        } => {
            cmd_remember(
                config,
                &content,
                &r#type,
                importance,
                project.as_deref(),
                &tag,
            )
            .await
        }
        MemoryCommand::Recall { query, limit } => cmd_recall(config, &query, limit).await,
        MemoryCommand::Forget { id } => cmd_forget(config, &id),
        MemoryCommand::Export { format } => cmd_export(config, &format),
        MemoryCommand::Context {
            project,
            max_tokens,
        } => cmd_context(config, &project, max_tokens).await,
        MemoryCommand::Consolidate { project } => cmd_consolidate(config, project.as_deref()).await,
        MemoryCommand::Decay => cmd_decay(config),
        MemoryCommand::SessionSummary { project, latest } => {
            cmd_session_summary(config, project.as_deref(), latest).await
        }
    }
}

/// Load the embedding model, degrading to FTS-only operation on failure
/// (e.g. first run while offline).
pub(crate) async fn try_embedder(config: &Config) -> Option<Arc<Embedder>> {
    match Embedder::new(config).await {
        Ok(e) => Some(Arc::new(e)),
        Err(e) => {
            tracing::warn!(error = %e, "embedding model unavailable — running keyword-only");
            None
        }
    }
}

type ServerBootstrap = (
    Arc<Mutex<Store>>,
    Option<Arc<Embedder>>,
    Arc<Config>,
    poneglyph_core::enrich::EnrichHandle,
    tokio::task::JoinHandle<()>,
);

/// Open the store + embedder and spawn the background enrich worker — the
/// setup shared by `mcp` (MCP) and `viewer` (HTTP), each otherwise
/// running standalone in its own process.
async fn open_store_and_worker(config: &Config) -> Result<ServerBootstrap> {
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
            consolidation: Some(poneglyph_core::enrich::ConsolidationScheduler {
                config: config.clone(),
                embedder: embedder.clone(),
            }),
        },
    );

    Ok((store, embedder, shared_config, enrich, worker))
}

/// No subcommand: print the logo, bring up MCP + viewer together in one
/// process (sharing the one `open_store_and_worker` setup instead of running
/// two separate `poneglyph mcp` / `poneglyph viewer` processes), and print a
/// one-line status once both ports are bound.
#[cfg(feature = "viewer")]
async fn cmd_default(config: &Config) -> Result<()> {
    poneglyph_http::validate_security(config)?;
    println!("{LOGO}");

    let (store, embedder, shared_config, enrich, worker) = open_store_and_worker(config).await?;
    let graph_watch = spawn_graph_watch_supervisor(store.clone(), shared_config.clone());

    let mcp = poneglyph_mcp::tools::PoneglyphMcp::new(store.clone(), embedder.clone(), shared_config.clone())
        .with_enrich(enrich.clone())
        .with_graph_dirty(graph_watch.dirty.clone());
    let mcp_state = poneglyph_http::AppState {
        store: store.clone(),
        embedder: embedder.clone(),
        config: shared_config.clone(),
        enrich: Some(enrich.clone()),
        graph_dirty: Some(graph_watch.dirty.clone()),
    };
    let mcp_app = poneglyph_http::build_router(mcp_state)
        .route("/health", axum::routing::get(|| async { "ok" }))
        .nest_service("/mcp", poneglyph_mcp::server::streamable_http_service(mcp));
    let mcp_addr = format!("127.0.0.1:{}", config.agents.mcp_server_port);
    let mcp_listener = tokio::net::TcpListener::bind(&mcp_addr)
        .await
        .with_context(|| format!("failed to bind MCP engine on {mcp_addr}"))?;

    let viewer_state = poneglyph_http::AppState {
        store,
        embedder,
        config: shared_config,
        enrich: Some(enrich),
        graph_dirty: Some(graph_watch.dirty.clone()),
    };
    let viewer_listener = poneglyph_http::bind(config)
        .await
        .context("failed to bind HTTP server")?;

    println!(
        "mcp active :{}  viewer active :{}",
        config.agents.mcp_server_port, config.dashboard.port
    );
    let result = tokio::select! {
        r = axum::serve(mcp_listener, mcp_app) => r.context("MCP engine server failed"),
        r = poneglyph_http::serve_on(viewer_listener, viewer_state) => r,
        _ = tokio::signal::ctrl_c() => Ok(()),
    };

    worker.abort();
    graph_watch.task.abort();
    result
}

/// No subcommand, `viewer` feature not compiled in: MCP alone.
#[cfg(not(feature = "viewer"))]
async fn cmd_default(config: &Config) -> Result<()> {
    println!("{LOGO}");

    let (store, embedder, shared_config, enrich, worker) = open_store_and_worker(config).await?;
    let graph_watch = spawn_graph_watch_supervisor(store.clone(), shared_config.clone());

    let mcp = poneglyph_mcp::tools::PoneglyphMcp::new(store.clone(), embedder.clone(), shared_config.clone())
        .with_enrich(enrich.clone())
        .with_graph_dirty(graph_watch.dirty.clone());
    let http_state = poneglyph_http::AppState {
        store,
        embedder,
        config: shared_config.clone(),
        enrich: Some(enrich),
        graph_dirty: Some(graph_watch.dirty.clone()),
    };
    let app = poneglyph_http::build_router(http_state)
        .route("/health", axum::routing::get(|| async { "ok" }))
        .nest_service("/mcp", poneglyph_mcp::server::streamable_http_service(mcp));
    let addr = format!("127.0.0.1:{}", config.agents.mcp_server_port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind MCP engine on {addr}"))?;

    println!("mcp active :{}", config.agents.mcp_server_port);
    let result = tokio::select! {
        r = axum::serve(listener, app) => r.context("MCP engine server failed"),
        _ = tokio::signal::ctrl_c() => Ok(()),
    };

    worker.abort();
    graph_watch.task.abort();
    result
}

/// MCP engine. Default: Streamable HTTP on `agents.mcp_server_port` — a
/// persistent daemon serving `/mcp` (agents) alongside the same `/api`,
/// `/ingest`, `/healthz` routes as `poneglyph viewer` (separate port), so
/// hooks have one always-on process to talk to. `--stdio` keeps the old
/// editor-spawned-per-session shape for clients that need it.
async fn cmd_mcp(config: &Config, stdio: bool) -> Result<()> {
    let (store, embedder, shared_config, enrich, worker) = open_store_and_worker(config).await?;
    let graph_watch = spawn_graph_watch_supervisor(store.clone(), shared_config.clone());

    let health = poneglyph_core::llm::health(&config.llm).await;
    tracing::info!(
        llm_enabled = config.llm.enabled,
        reachable = health.reachable,
        status = ?health.status,
        "LLM health check"
    );

    let mcp = poneglyph_mcp::tools::PoneglyphMcp::new(store.clone(), embedder.clone(), shared_config.clone())
        .with_enrich(enrich.clone())
        .with_graph_dirty(graph_watch.dirty.clone());

    if stdio {
        // NOTE: stdout belongs to MCP JSON-RPC from here on — no println!.
        let result = poneglyph_mcp::server::run_stdio(mcp).await;
        worker.abort(); // client gone; no more producers
        graph_watch.task.abort();
        return result;
    }

    let http_state = poneglyph_http::AppState {
        store,
        embedder,
        config: shared_config,
        enrich: Some(enrich),
        graph_dirty: Some(graph_watch.dirty.clone()),
    };
    let app = poneglyph_http::build_router(http_state)
        .route("/health", axum::routing::get(|| async { "ok" }))
        .nest_service("/mcp", poneglyph_mcp::server::streamable_http_service(mcp));

    let addr = format!("127.0.0.1:{}", config.agents.mcp_server_port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind MCP engine on {addr}"))?;
    println!("MCP engine listening on http://{addr} (mcp: /mcp, api: /api, ingest: /ingest)");
    let result = tokio::select! {
        r = axum::serve(listener, app) => r.context("MCP engine server failed"),
        _ = tokio::signal::ctrl_c() => Ok(()),
    };

    worker.abort();
    graph_watch.task.abort();
    result
}

/// Handle for the background graph-watch supervisor: the blocking task
/// itself, plus the set of project ids with changes pending a debounced
/// rebuild — `PoneglyphMcp` reads `dirty` to flag query responses as
/// possibly-stale without blocking the request on a synchronous rebuild.
struct GraphWatchHandle {
    task: tokio::task::JoinHandle<()>,
    dirty: Arc<Mutex<std::collections::HashSet<String>>>,
}

/// Replaces fixed-interval polling with real file-watching: one
/// `notify::RecommendedWatcher` per project tracked in `graph_projects.toml`,
/// debounced per project (`config.graph.watch_delay_ms`), so codegraph
/// results stay fresh without the user running `graph update` by hand.
/// `config.graph.auto_update_minutes` is repurposed from "rebuild interval"
/// to "how often to reconcile the watched-project set against
/// graph_projects.toml" (pick up projects `graph init`'d after the daemon
/// started); 0 still means disabled entirely.
fn spawn_graph_watch_supervisor(store: Arc<Mutex<Store>>, config: Arc<Config>) -> GraphWatchHandle {
    let dirty: Arc<Mutex<std::collections::HashSet<String>>> = Arc::new(Mutex::new(std::collections::HashSet::new()));
    let dirty_for_task = dirty.clone();

    let task = tokio::task::spawn_blocking(move || {
        if config.graph.auto_update_minutes == 0 {
            return; // disabled
        }

        let mut projects = graph_registry::load().map(|p| p.project).unwrap_or_default();
        // Catch up once before watching, so a freshly-started daemon doesn't
        // serve stale results until the first debounce window elapses.
        for p in &projects {
            rebuild_project(&store, &config, p);
        }

        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let mut watchers: std::collections::HashMap<String, notify::RecommendedWatcher> = std::collections::HashMap::new();
        for p in &projects {
            install_watcher(&mut watchers, &tx, &p.dir);
        }

        let mut last_event: std::collections::HashMap<String, std::time::Instant> = std::collections::HashMap::new();
        let debounce = std::time::Duration::from_millis(config.graph.watch_delay_ms);
        let reconcile_every = std::time::Duration::from_secs(config.graph.auto_update_minutes * 60);
        let mut last_reconcile = std::time::Instant::now();

        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(dir) => {
                    last_event.insert(dir.clone(), std::time::Instant::now());
                    if let Some(p) = projects.iter().find(|p| p.dir == dir) {
                        dirty_for_task.lock().unwrap().insert(p.id.clone());
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }

            let ready: Vec<String> =
                last_event.iter().filter(|(_, t)| t.elapsed() >= debounce).map(|(dir, _)| dir.clone()).collect();
            for dir in ready {
                last_event.remove(&dir);
                if let Some(p) = projects.iter().find(|p| p.dir == dir) {
                    rebuild_project(&store, &config, p);
                    dirty_for_task.lock().unwrap().remove(&p.id);
                }
            }

            if last_reconcile.elapsed() >= reconcile_every {
                last_reconcile = std::time::Instant::now();
                if let Ok(fresh) = graph_registry::load() {
                    let fresh = fresh.project;
                    watchers.retain(|dir, _| fresh.iter().any(|p| &p.dir == dir));
                    for p in &fresh {
                        if !watchers.contains_key(&p.dir) {
                            install_watcher(&mut watchers, &tx, &p.dir);
                        }
                    }
                    projects = fresh;
                }
            }
        }
    });

    GraphWatchHandle { task, dirty }
}

fn install_watcher(
    watchers: &mut std::collections::HashMap<String, notify::RecommendedWatcher>,
    tx: &std::sync::mpsc::Sender<String>,
    dir: &str,
) {
    use notify::Watcher;
    if !Path::new(dir).is_dir() {
        return;
    }
    let dir_owned = dir.to_string();
    let tx = tx.clone();
    let Ok(mut watcher) = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.send(dir_owned.clone());
        }
    }) else {
        return;
    };
    if watcher.watch(Path::new(dir), notify::RecursiveMode::Recursive).is_ok() {
        watchers.insert(dir.to_string(), watcher);
    }
}

fn rebuild_project(store: &Arc<Mutex<Store>>, config: &Config, p: &graph_registry::GraphProjectEntry) {
    let path = PathBuf::from(&p.dir);
    if !path.is_dir() {
        return;
    }
    let result = {
        let Ok(guard) = store.lock() else { return };
        poneglyph_core::codegraph::build(&guard, &path, &config.graph, false)
    };
    match result {
        Ok(report) if report.files_parsed > 0 => {
            tracing::info!(dir = %p.dir, nodes = report.nodes, edges = report.edges, "graph rebuilt")
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(dir = %p.dir, error = %e, "graph rebuild failed"),
    }
}

/// HTTP dashboard + graph viewer only — for browsing in a browser.
/// `poneglyph mcp` runs the MCP server as a separate, independent process.
#[cfg(feature = "viewer")]
async fn cmd_viewer(config: &Config) -> Result<()> {
    poneglyph_http::validate_security(config)?;
    let (store, embedder, shared_config, enrich, worker) = open_store_and_worker(config).await?;

    let listener = poneglyph_http::bind(config)
        .await
        .context("failed to bind HTTP server")?;
    let http_state = poneglyph_http::AppState {
        store,
        embedder,
        config: shared_config,
        enrich: Some(enrich),
        // ponytail: no file watcher in this command path, so this view of
        // the graph can never be stale — `poneglyph mcp` is what rebuilds.
        graph_dirty: None,
    };

    println!(
        "Viewer listening on http://{}:{}",
        config.dashboard.host, config.dashboard.port
    );
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
        outcome.memories,
        outcome.edges,
        outcome.projects,
        db_path.display()
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

    let exclude_matcher =
        poneglyph_core::privacy::build_exclude_matcher(&config.privacy.exclude_paths);
    if poneglyph_core::privacy::content_references_excluded_path(content, &exclude_matcher) {
        anyhow::bail!(
            "refusing to store: content references an excluded path (see [privacy].exclude_paths)"
        );
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
        poneglyph_core::enrich::enqueue_compression(
            &store,
            &mem.id,
            config.memory.compression_mode,
        )?;
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
        &config.retrieval,
    )?;

    if results.is_empty() {
        println!("No memories found.");
        return Ok(());
    }

    for r in &results {
        // Full id: UUIDv7 prefixes are timestamps, so short prefixes collide.
        println!(
            "[{:.4}] {} — {}",
            r.score,
            r.memory.id,
            truncate(&r.memory.content, 80)
        );
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
                println!(
                    "Type: {} | Importance: {} | Created: {}\n",
                    mem.memory_type, mem.importance, mem.created_at
                );
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
        eprintln!(
            "poneglyph: code graph is empty — run `poneglyph graph init` to enable codegraph_query/codegraph_blast_radius."
        );
    }

    Ok(())
}

async fn cmd_consolidate(config: &Config, project_path: Option<&str>) -> Result<()> {
    config.ensure_dirs()?;
    let mut store = Store::open(&config.db_path)?;
    let embedder = try_embedder(config).await;
    let llm = LlmClient::from_config(&config.llm);

    // Resolve project
    let project_id = match project_path {
        Some(path) => Some(poneglyph_core::project::detect_project(&store, path)?.id),
        None => None,
    };

    let report = match &project_id {
        Some(pid) => {
            poneglyph_core::pipeline::run_pipeline_for_project(
                &mut store,
                pid,
                config,
                embedder.as_deref(),
                llm.as_ref(),
            )
            .await?
        }
        None => {
            poneglyph_core::pipeline::run_pipeline_for_all_projects(
                &mut store,
                config,
                embedder.as_deref(),
                llm.as_ref(),
            )
            .await?
        }
    };

    println!(
        "Pipeline run complete: {} episodic summaries, {} semantic facts, {} procedures.",
        report.episodic_summaries, report.semantic_facts, report.procedures
    );

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

async fn cmd_eval(config: &Config, dataset: &std::path::Path, limit: Option<usize>, json: bool) -> Result<()> {
    let summary = eval::run(config, dataset, limit).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }

    println!("LongMemEval — {}", dataset.display());
    println!("  instances:        {}", summary.total_instances);
    println!("  evaluated:        {}", summary.evaluated);
    println!("  skipped (no gold): {}", summary.skipped_no_gold);
    println!("  R@1:  {:.3}", summary.recall_at_1);
    println!("  R@5:  {:.3}", summary.recall_at_5);
    println!("  R@10: {:.3}", summary.recall_at_10);
    println!("  MRR:  {:.3}", summary.mrr);

    Ok(())
}

/// Prune dead project registrations: graph_projects.toml entries and
/// memory `projects` rows whose directory no longer exists on disk. Never
/// touches `cg_files`/`cg_nodes` (no project scoping exists there) and
/// never deletes memories (`project_id` FK is `ON DELETE SET NULL`).
fn cmd_cleanup(config: &Config) -> Result<()> {
    config.ensure_dirs()?;
    let mut any = false;

    for p in graph_registry::load()?.project {
        if !Path::new(&p.dir).exists() {
            graph_registry::unregister(&p.dir)?;
            println!("Removed graph registration: {}", p.dir);
            any = true;
        }
    }

    let store = Store::open(&config.db_path)?;
    for p in store.list_projects()? {
        if !Path::new(&p.path).exists() {
            store.delete_project_by_path(&p.path)?;
            println!("Removed project record: {}", p.path);
            any = true;
        }
    }

    if !any {
        println!("Nothing to clean up.");
    }
    Ok(())
}

async fn cmd_status(config: &Config) -> Result<()> {
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
    println!(
        "Enrichment:  {}",
        if config.llm.enabled { "on" } else { "off" }
    );

    let mcp_listening = daemon::port_open(config.agents.mcp_server_port);
    println!(
        "MCP port:    {} ({})",
        config.agents.mcp_server_port,
        if mcp_listening { "listening" } else { "not listening" }
    );
    println!(
        "LLM base URL:{}",
        config.llm.base_url.as_deref().unwrap_or("(provider default)")
    );
    println!("LLM model:   {}", config.llm.model.as_deref().unwrap_or("(unset)"));
    if config.llm.enabled {
        let health = poneglyph_core::llm::health(&config.llm).await;
        println!(
            "LLM health:  {}{}",
            if health.reachable { "reachable" } else { "unreachable" },
            health.status.map(|s| format!(" ({s})")).unwrap_or_default()
        );
    } else {
        println!("LLM health:  (enrichment disabled)");
    }

    if Config::using_legacy_paths() {
        println!();
        println!("Note: data lives at a legacy location.");
        println!("Move it with poneglyph stopped, e.g.:");
        println!("  mv <legacy poneglyph.db> ~/.config/poneglyph/data/");
    }

    Ok(())
}

fn cmd_graph(config: &Config, action: GraphCommand) -> Result<()> {
    use poneglyph_core::codegraph;

    config.ensure_dirs()?;
    let store = Store::open(&config.db_path)?;

    match action {
        GraphCommand::Init { path } => {
            create_graph_project_files(&path)?;
            let report = codegraph::build(&store, &path, &config.graph, true)?;
            print_build_report(&report);
            update_graph_lock(&path);
            register_for_auto_update(&store, &path);
        }
        GraphCommand::Update { path } => {
            let report = codegraph::build(&store, &path, &config.graph, false)?;
            print_build_report(&report);
            update_graph_lock(&path);
            register_for_auto_update(&store, &path);
        }
        GraphCommand::Watch { path } => cmd_graph_watch(&store, &path, config)?,
        GraphCommand::Query { q, path } => {
            let project = poneglyph_core::project::detect_project(&store, &path.canonicalize()?.to_string_lossy())?;
            let query = codegraph::parse_query(&q);
            let results = codegraph::run_query(&store, &project.id, &query)?;
            if results.is_empty() {
                println!("No matches.");
            }
            for n in &results {
                println!("[{}] {} — {}:{}", n.kind, n.name, n.file_path, n.start_line);
            }
        }
        GraphCommand::BlastRadius { target, depth, path } => {
            let project = poneglyph_core::project::detect_project(&store, &path.canonicalize()?.to_string_lossy())?;
            let depth = depth.unwrap_or(config.graph.blast_radius_depth);
            let report = codegraph::blast_radius(&store, &project.id, &target, depth)?;
            if report.root.is_empty() {
                println!("No file or symbol matching '{target}' found in the graph.");
                return Ok(());
            }
            println!("Root ({} symbol(s)):", report.root.len());
            for n in &report.root {
                println!(
                    "  [{}] {} — {}:{}",
                    n.kind, n.name, n.file_path, n.start_line
                );
            }
            println!("\nDependents ({}):", report.dependents.len());
            for d in &report.dependents {
                println!(
                    "  depth {} [{}] {} — {}:{}",
                    d.depth, d.node.kind, d.node.name, d.node.file_path, d.node.start_line
                );
            }
            println!("\nTests ({}):", report.tests.len());
            for t in &report.tests {
                println!("  {} — {}:{}", t.name, t.file_path, t.start_line);
            }
        }
        GraphCommand::Export { format, out, path } => {
            let project = poneglyph_core::project::detect_project(&store, &path.canonicalize()?.to_string_lossy())?;
            let rendered = match format.as_str() {
                "json" => codegraph::export_json(&store, &project.id)?,
                "dot" => codegraph::export_dot(&store, &project.id)?,
                "graphml" => codegraph::export_graphml(&store, &project.id)?,
                other => {
                    anyhow::bail!("unknown export format '{other}' (use json, dot, or graphml)")
                }
            };
            match out {
                Some(out_path) => {
                    std::fs::write(&out_path, rendered)
                        .with_context(|| format!("failed to write {}", out_path.display()))?;
                    println!("Exported: {}", out_path.display());
                }
                None => println!("{rendered}"),
            }
        }
        GraphCommand::Explore { target, depth, path } => {
            let abs_path = path.canonicalize()?;
            let project = poneglyph_core::project::detect_project(&store, &abs_path.to_string_lossy())?;
            let depth = depth.unwrap_or(config.graph.blast_radius_depth);
            let report = codegraph::explore(&store, &project.id, &abs_path, &target, depth)?;
            if report.root.is_empty() {
                println!("No file or symbol matching '{target}' found in the graph.");
                return Ok(());
            }
            println!("Root ({} symbol(s)):", report.root.len());
            for n in &report.root {
                println!("  [{}] {} — {}:{}", n.kind, n.name, n.file_path, n.start_line);
            }
            for s in &report.snippets {
                println!("\n--- {} ({}:{}-{}) ---\n{}", s.node_id, s.file_path, s.start_line, s.end_line, s.source);
            }
            println!("\nCallers ({}):", report.callers.len());
            for n in &report.callers {
                println!("  [{}] {} — {}:{}", n.kind, n.name, n.file_path, n.start_line);
            }
            println!("\nCallees ({}):", report.callees.len());
            for n in &report.callees {
                println!("  [{}] {} — {}:{}", n.kind, n.name, n.file_path, n.start_line);
            }
            println!("\nSupertypes ({}):", report.supertypes.len());
            for n in &report.supertypes {
                println!("  [{}] {} — {}:{}", n.kind, n.name, n.file_path, n.start_line);
            }
            println!("\nSubtypes ({}):", report.subtypes.len());
            for n in &report.subtypes {
                println!("  [{}] {} — {}:{}", n.kind, n.name, n.file_path, n.start_line);
            }
            println!("\nTests ({}):", report.tests.len());
            for t in &report.tests {
                println!("  {} — {}:{}", t.name, t.file_path, t.start_line);
            }
            println!("\nBlast radius dependents ({}):", report.blast_radius.dependents.len());
            for d in &report.blast_radius.dependents {
                println!(
                    "  depth {} [{}] {} — {}:{}",
                    d.depth, d.node.kind, d.node.name, d.node.file_path, d.node.start_line
                );
            }
        }
    }

    Ok(())
}

fn print_build_report(report: &poneglyph_core::codegraph::BuildReport) {
    println!(
        "Parsed {} file(s), {} unchanged, {} removed. {} node(s), {} edge(s).",
        report.files_parsed,
        report.files_unchanged,
        report.files_removed,
        report.nodes,
        report.edges
    );
}

/// Blocks the calling thread, rebuilding incrementally after each debounced
/// burst of filesystem events, until Ctrl-C.
fn cmd_graph_watch(store: &Store, path: &std::path::Path, config: &Config) -> Result<()> {
    use notify::{RecursiveMode, Watcher};

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx).context("failed to start file watcher")?;
    watcher
        .watch(path, RecursiveMode::Recursive)
        .context("failed to watch path")?;

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
                update_graph_lock(path);
            }
            Err(e) => tracing::warn!(error = %e, "graph update failed"),
        }
    }

    Ok(())
}

/// Track this project in `graph_projects.toml` so the daemon's background
/// auto-update task (spawned in `cmd_mcp`) can keep its graph fresh without
/// the user running `graph update` by hand. Best-effort: failures are logged,
/// never fatal to the build that just succeeded.
fn register_for_auto_update(store: &Store, path: &std::path::Path) {
    let Ok(abs) = path.canonicalize() else { return };
    let Ok(project) = poneglyph_core::project::detect_project(store, &abs.to_string_lossy())
    else {
        return;
    };
    if let Err(e) = graph_registry::register(&abs, &project.id) {
        tracing::warn!(error = %e, "failed to register project for auto graph updates");
    }
}

/// Update `last_build` timestamp in `<path>/.poneglyph/code-graph-lock.json`.
fn update_graph_lock(path: &Path) {
    let lock_path = path.join(".poneglyph/code-graph-lock.json");
    if !lock_path.exists() {
        return;
    }
    let Ok(raw) = std::fs::read_to_string(&lock_path) else {
        return;
    };
    let Ok(mut lock) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    lock["last_build"] = serde_json::json!(chrono::Utc::now().to_rfc3339());
    let _ = std::fs::write(
        &lock_path,
        serde_json::to_string_pretty(&lock).unwrap_or_default(),
    );
}

/// Create `.poneglyphignore` and `.poneglyph/code-graph-lock.json` at `path`
/// if missing. Only `graph init` creates these now — project `init` doesn't.
fn create_graph_project_files(path: &Path) -> Result<()> {
    let ignore_path = path.join(".poneglyphignore");
    if !ignore_path.exists() {
        std::fs::write(
            &ignore_path,
            "node_modules/\ntarget/\nvendor/\n.git/\ndist/\nbuild/\nout/\n.DS_Store\n",
        )?;
        println!("Created: {}", ignore_path.display());
    }

    let lock_dir = path.join(".poneglyph");
    let lock_path = lock_dir.join("code-graph-lock.json");
    if !lock_path.exists() {
        std::fs::create_dir_all(&lock_dir).context("failed to create .poneglyph directory")?;
        let lock = serde_json::json!({
            "version": 1,
            "created_at": chrono::Utc::now().to_rfc3339(),
            "last_build": null,
            "languages": ["rust", "typescript", "javascript", "python", "go"]
        });
        std::fs::write(&lock_path, serde_json::to_string_pretty(&lock)?)
            .context("failed to write code-graph-lock.json")?;
        println!("Created: {}", lock_path.display());
    }
    Ok(())
}

async fn cmd_session_summary(
    config: &Config,
    project_path: Option<&str>,
    latest: bool,
) -> Result<()> {
    config.ensure_dirs()?;
    let mut store = Store::open(&config.db_path)?;

    // Resolve project
    let project_id = match project_path {
        Some(path) => Some(poneglyph_core::project::detect_project(&store, path)?.id),
        None => None,
    };

    if latest {
        // Show the most recent session-summary memory
        let (memories, _) = store.list_memories(project_id.as_deref(), Some("episodic"), 50, 0)?;
        let summary = memories.iter().find(|m| {
            m.metadata
                .as_ref()
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

    let llm = LlmClient::from_config(&config.llm);
    let Some(mem) =
        poneglyph_core::pipeline::summarize_session(&mut store, project_id.as_deref(), llm.as_ref()).await?
    else {
        return Ok(());
    };

    // Enqueue LLM enrichment if available (edges are already enqueued by
    // `summarize_session` itself).
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
