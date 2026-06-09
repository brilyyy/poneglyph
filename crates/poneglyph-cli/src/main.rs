use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

use poneglyph_core::config::Config;
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
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("poneglyph=info".parse()?))
        .init();

    let cli = Cli::parse();
    let config = load_config(&cli.config)?;

    match cli.command {
        Command::Init => cmd_init(&config),
        Command::Serve => cmd_serve(&config).await,
        Command::Remember { content, r#type, importance, project, tag } => {
            cmd_remember(&config, &content, &r#type, importance, project.as_deref(), &tag)
        }
        Command::Recall { query, limit } => cmd_recall(&config, &query, limit),
        Command::Forget { id } => cmd_forget(&config, &id),
        Command::Export { format } => cmd_export(&config, &format),
        Command::Status => cmd_status(&config),
    }
}

fn cmd_init(config: &Config) -> Result<()> {
    config.ensure_dirs().context("failed to create directories")?;

    // Create default config if it doesn't exist
    let config_path = Config::default_config_path();
    if !config_path.exists() {
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

async fn cmd_serve(config: &Config) -> Result<()> {
    config.ensure_dirs()?;
    let store = Store::open(&config.db_path).context("failed to open database")?;
    let _ = store; // Placeholder — MCP + HTTP servers will be wired in later phases
    println!("Starting poneglyph server...");
    println!("MCP stdio server: active");
    println!("HTTP server: {}:{}", config.server.bind_addr, config.server.http_port);
    // TODO: MCP + HTTP server implementation (Phase M2-M4)
    tokio::signal::ctrl_c().await?;
    Ok(())
}

fn cmd_remember(
    config: &Config,
    content: &str,
    memory_type: &str,
    importance: f64,
    project: Option<&str>,
    tags: &[String],
) -> Result<()> {
    config.ensure_dirs()?;
    let store = Store::open(&config.db_path)?;

    let mem_type: MemoryType = memory_type.parse().unwrap_or(MemoryType::Semantic);

    // Resolve project
    let project_id = if let Some(path) = project {
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());
        let p = store.upsert_project(path, &name, None)?;
        Some(p.id)
    } else {
        None
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

    // Index FTS
    store.index_fts(&mem.id, content)?;

    println!("{}", mem.id);
    Ok(())
}

fn cmd_recall(config: &Config, query: &str, limit: usize) -> Result<()> {
    config.ensure_dirs()?;
    let store = Store::open(&config.db_path)?;

    let filters = poneglyph_core::retrieve::RecallFilters::default();
    let results = poneglyph_core::retrieve::recall(
        &store.conn,
        &vec![0.0; config.embedding.dimensions], // Placeholder — real embedding needs model
        query,
        &filters,
        limit,
    )?;

    if results.is_empty() {
        println!("No memories found.");
        return Ok(());
    }

    for r in &results {
        println!("[{:.4}] {} — {}", r.score, &r.memory.id[..8], truncate(&r.memory.content, 80));
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

    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
