//! Agent auto-detection for `poneglyph init`: probes which coding agents are
//! installed, then idempotently merges MCP server / hook config into each
//! one's existing config files (never clobbering unrelated user settings).

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

use poneglyph_core::config::AgentsConfig;

const POSTTOOLUSE_SH: &str = include_str!("../../../hooks/claude-code/posttooluse.sh");
const USERPROMPTSUBMIT_SH: &str = include_str!("../../../hooks/claude-code/userpromptsubmit.sh");
const STOP_SH: &str = include_str!("../../../hooks/claude-code/stop.sh");
const SESSIONSTART_SH: &str = include_str!("../../../hooks/claude-code/sessionstart.sh");
#[cfg(feature = "opencode")]
const OPENCODE_PLUGIN_TS: &str = include_str!("../../../hooks/opencode/poneglyph.ts");
const SKILL_MD: &str = include_str!("../../../skills/poneglyph/SKILL.md");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStatus {
    Configured,
    AlreadyConfigured,
    NotDetected,
    Disabled,
}

impl SetupStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::AlreadyConfigured => "already configured",
            Self::NotDetected => "not detected",
            Self::Disabled => "disabled in [agents] config",
        }
    }
}

pub struct SetupOutcome {
    pub agent: &'static str,
    pub status: SetupStatus,
}

pub fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

/// Probe every configured agent and wire up the ones found installed.
/// `hooks_dir` is where the bundled Claude Code hook scripts get copied
/// (typically `Config::config_dir().join("hooks")`).
#[allow(dead_code)]
pub fn run_agent_setup(agents: &AgentsConfig, hooks_dir: &Path, exe: &str) -> Result<Vec<SetupOutcome>> {
    let Some(home) = home_dir() else {
        anyhow::bail!("could not resolve home directory");
    };
    #[allow(unused_mut)]
    let mut out = vec![
        setup_claude_code(agents.claude_code, &home, hooks_dir, exe)?,
    ];

    #[cfg(feature = "cursor")]
    out.push(setup_cursor(agents.cursor, &home, exe)?);
    #[cfg(feature = "gemini")]
    out.push(setup_gemini_cli(agents.gemini_cli, &home, exe)?);
    #[cfg(feature = "opencode")]
    out.push(setup_opencode(agents.opencode, &home, exe)?);
    #[cfg(feature = "codex")]
    out.push(setup_codex(agents.codex, &home, exe)?);
    #[cfg(feature = "copilot")]
    out.push(setup_copilot_cli(agents.copilot_cli, &home, exe)?);

    Ok(out)
}

/// Wire up a single agent by name. Always enabled (ignores config flags).
pub fn wire_agent(ide: &str, hooks_dir: &Path, exe: &str) -> Result<SetupOutcome> {
    let Some(home) = home_dir() else {
        anyhow::bail!("could not resolve home directory");
    };
    match ide {
        "claude-code" => setup_claude_code(true, &home, hooks_dir, exe),
        #[cfg(feature = "opencode")]
        "opencode" => setup_opencode(true, &home, exe),
        #[cfg(feature = "cursor")]
        "cursor" => setup_cursor(true, &home, exe),
        #[cfg(feature = "gemini")]
        "gemini" => setup_gemini_cli(true, &home, exe),
        #[cfg(feature = "codex")]
        "codex" => setup_codex(true, &home, exe),
        #[cfg(feature = "copilot")]
        "copilot" => setup_copilot_cli(true, &home, exe),
        _ => {
            #[allow(unused_mut)]
            let mut opts = vec!["claude-code"];
            #[cfg(feature = "opencode")] opts.push("opencode");
            #[cfg(feature = "cursor")] opts.push("cursor");
            #[cfg(feature = "gemini")] opts.push("gemini");
            #[cfg(feature = "codex")] opts.push("codex");
            #[cfg(feature = "copilot")] opts.push("copilot");
            anyhow::bail!("unknown IDE '{ide}'. Options: {}", opts.join(", "))
        }
    }
}

/// Inject poneglyph usage rules into the global agent rule file.
pub fn inject_global_rules(ide: &str, home: &Path) -> Result<bool> {
    let path = match ide {
        "claude-code" => home.join(".claude").join("CLAUDE.md"),
        #[cfg(feature = "opencode")]
        "opencode" => home.join(".config").join("opencode").join("AGENTS.md"),
        #[cfg(feature = "cursor")]
        "cursor" => home.join(".cursor").join("rules"),
        #[cfg(feature = "gemini")]
        "gemini" => home.join(".gemini").join("rules"),
        #[cfg(feature = "codex")]
        "codex" => home.join(".codex").join("rules"),
        #[cfg(feature = "copilot")]
        "copilot" => home.join(".copilot").join("rules"),
        _ => return Ok(false),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    inject_rules_block(&path)
}

fn setup_claude_code(enabled: bool, home: &Path, hooks_dir: &Path, exe: &str) -> Result<SetupOutcome> {
    if !enabled {
        return Ok(SetupOutcome { agent: "claude-code", status: SetupStatus::Disabled });
    }
    let claude_dir = home.join(".claude");
    if !claude_dir.exists() {
        return Ok(SetupOutcome { agent: "claude-code", status: SetupStatus::NotDetected });
    }

    install_hook_scripts(hooks_dir)?;
    let hooks_changed = merge_claude_code_hooks(&claude_dir.join("settings.json"), hooks_dir)?;
    let mcp_changed = merge_json_mcp_server(&home.join(".claude.json"), "mcpServers", true, exe)?;
    let skill_changed = install_skill_file(&claude_dir.join("skills"))?;

    let status =
        if hooks_changed || mcp_changed || skill_changed { SetupStatus::Configured } else { SetupStatus::AlreadyConfigured };
    Ok(SetupOutcome { agent: "claude-code", status })
}

#[cfg(feature = "cursor")]
fn setup_cursor(enabled: bool, home: &Path, exe: &str) -> Result<SetupOutcome> {
    if !enabled {
        return Ok(SetupOutcome { agent: "cursor", status: SetupStatus::Disabled });
    }
    let cursor_dir = home.join(".cursor");
    if !cursor_dir.exists() {
        return Ok(SetupOutcome { agent: "cursor", status: SetupStatus::NotDetected });
    }
    let changed = merge_json_mcp_server(&cursor_dir.join("mcp.json"), "mcpServers", true, exe)?;
    let status = if changed { SetupStatus::Configured } else { SetupStatus::AlreadyConfigured };
    Ok(SetupOutcome { agent: "cursor", status })
}

#[cfg(feature = "gemini")]
fn setup_gemini_cli(enabled: bool, home: &Path, exe: &str) -> Result<SetupOutcome> {
    if !enabled {
        return Ok(SetupOutcome { agent: "gemini-cli", status: SetupStatus::Disabled });
    }
    let gemini_dir = home.join(".gemini");
    if !gemini_dir.exists() {
        return Ok(SetupOutcome { agent: "gemini-cli", status: SetupStatus::NotDetected });
    }
    let changed = merge_json_mcp_server(&gemini_dir.join("settings.json"), "mcpServers", true, exe)?;
    let status = if changed { SetupStatus::Configured } else { SetupStatus::AlreadyConfigured };
    Ok(SetupOutcome { agent: "gemini-cli", status })
}

#[cfg(feature = "opencode")]
fn setup_opencode(enabled: bool, home: &Path, exe: &str) -> Result<SetupOutcome> {
    if !enabled {
        return Ok(SetupOutcome { agent: "opencode", status: SetupStatus::Disabled });
    }
    let opencode_dir = home.join(".config").join("opencode");
    if !opencode_dir.exists() {
        return Ok(SetupOutcome { agent: "opencode", status: SetupStatus::NotDetected });
    }
    let plugin_changed = install_opencode_plugin(&opencode_dir.join("plugins"))?;
    let mcp_changed = merge_opencode_mcp_server(&opencode_dir.join("opencode.json"), exe)?;
    let skill_changed = install_opencode_skill(&opencode_dir.join("skills"))?;
    let status = if plugin_changed || mcp_changed || skill_changed { SetupStatus::Configured } else { SetupStatus::AlreadyConfigured };
    Ok(SetupOutcome { agent: "opencode", status })
}

#[cfg(feature = "codex")]
fn setup_codex(enabled: bool, home: &Path, exe: &str) -> Result<SetupOutcome> {
    if !enabled {
        return Ok(SetupOutcome { agent: "codex", status: SetupStatus::Disabled });
    }
    let codex_dir = home.join(".codex");
    if !codex_dir.exists() {
        return Ok(SetupOutcome { agent: "codex", status: SetupStatus::NotDetected });
    }
    let changed = merge_codex_mcp_server(&codex_dir.join("config.toml"), exe)?;
    let status = if changed { SetupStatus::Configured } else { SetupStatus::AlreadyConfigured };
    Ok(SetupOutcome { agent: "codex", status })
}

#[cfg(feature = "copilot")]
fn setup_copilot_cli(enabled: bool, home: &Path, exe: &str) -> Result<SetupOutcome> {
    if !enabled {
        return Ok(SetupOutcome { agent: "copilot-cli", status: SetupStatus::Disabled });
    }
    let copilot_home = std::env::var_os("COPILOT_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".copilot"));
    if !copilot_home.exists() {
        return Ok(SetupOutcome { agent: "copilot-cli", status: SetupStatus::NotDetected });
    }
    let changed = merge_json_mcp_server(&copilot_home.join("mcp-config.json"), "mcpServers", true, exe)?;
    let status = if changed { SetupStatus::Configured } else { SetupStatus::AlreadyConfigured };
    Ok(SetupOutcome { agent: "copilot-cli", status })
}

/// Write the bundled Claude Code hook scripts into `hooks_dir`, executable
/// on unix. Always re-written so upgrades pick up script changes.
fn install_hook_scripts(hooks_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(hooks_dir).context("failed to create hooks directory")?;
    for (name, content) in [
        ("posttooluse.sh", POSTTOOLUSE_SH),
        ("userpromptsubmit.sh", USERPROMPTSUBMIT_SH),
        ("stop.sh", STOP_SH),
        ("sessionstart.sh", SESSIONSTART_SH),
    ] {
        let path = hooks_dir.join(name);
        std::fs::write(&path, content).with_context(|| format!("failed to write {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(&path)?.permissions();
            perm.set_mode(0o755);
            std::fs::set_permissions(&path, perm)?;
        }
    }
    Ok(())
}

/// Merge the 4 hook entries into `settings.json`'s `"hooks"` key, skipping
/// any event that already references our script path. Returns whether the
/// file changed.
fn merge_claude_code_hooks(settings_path: &Path, hooks_dir: &Path) -> Result<bool> {
    let mut root = read_json_object(settings_path)?;
    let obj = root.as_object_mut().context("settings.json root must be a JSON object")?;
    let hooks_val = obj.entry("hooks").or_insert_with(|| json!({}));
    let hooks_obj = hooks_val.as_object_mut().context("\"hooks\" key must be a JSON object")?;

    let specs: [(&str, &str, u64, Option<&str>); 4] = [
        ("PostToolUse", "posttooluse.sh", 5, Some("Edit|Write|MultiEdit|Bash|NotebookEdit|Agent|Task")),
        ("UserPromptSubmit", "userpromptsubmit.sh", 5, None),
        ("Stop", "stop.sh", 10, None),
        ("SessionStart", "sessionstart.sh", 5, None),
    ];

    let mut changed = false;
    for (event, script, timeout, matcher) in specs {
        let command = hooks_dir.join(script).display().to_string();
        let arr_val = hooks_obj.entry(event).or_insert_with(|| json!([]));
        let arr = arr_val.as_array_mut().context("hook event entry must be a JSON array")?;
        let already_present = arr.iter().any(|entry| {
            entry
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|hs| hs.iter().any(|h| h.get("command").and_then(Value::as_str) == Some(command.as_str())))
        });
        if already_present {
            continue;
        }
        let mut block = json!({ "hooks": [{ "type": "command", "command": command, "timeout": timeout }] });
        if let Some(m) = matcher {
            block["matcher"] = json!(m);
        }
        arr.push(block);
        changed = true;
    }

    if changed {
        write_json(settings_path, &root)?;
    }
    Ok(changed)
}

/// Idempotently insert a `poneglyph` entry under `top_key` in a JSON config
/// file. Leaves any existing `poneglyph` entry untouched. Returns whether
/// the file changed.
fn merge_json_mcp_server(path: &Path, top_key: &str, include_type_stdio: bool, exe: &str) -> Result<bool> {
    let mut root = read_json_object(path)?;
    let obj = root.as_object_mut().with_context(|| format!("{} root must be a JSON object", path.display()))?;
    let servers_val = obj.entry(top_key).or_insert_with(|| json!({}));
    let servers = servers_val.as_object_mut().with_context(|| format!("\"{top_key}\" key must be a JSON object"))?;
    if servers.contains_key("poneglyph") {
        return Ok(false);
    }
    let mut entry = json!({ "command": exe, "args": ["mcp"] });
    if include_type_stdio {
        entry["type"] = json!("stdio");
    }
    servers.insert("poneglyph".to_string(), entry);
    write_json(path, &root)?;
    Ok(true)
}

/// Idempotently insert a `[mcp_servers.poneglyph]` table into a TOML config
/// file. Returns whether the file changed.
#[cfg(feature = "codex")]
fn merge_codex_mcp_server(path: &Path, exe: &str) -> Result<bool> {
    let mut root: toml::Value = if path.exists() {
        std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?
            .parse()
            .with_context(|| format!("failed to parse {}", path.display()))?
    } else {
        toml::Value::Table(Default::default())
    };
    let table = root.as_table_mut().with_context(|| format!("{} root must be a TOML table", path.display()))?;
    let servers_val = table.entry("mcp_servers").or_insert_with(|| toml::Value::Table(Default::default()));
    let servers = servers_val.as_table_mut().context("\"mcp_servers\" key must be a TOML table")?;
    if servers.contains_key("poneglyph") {
        return Ok(false);
    }
    let mut entry = toml::value::Table::new();
    entry.insert("command".into(), toml::Value::String(exe.into()));
    entry.insert("args".into(), toml::Value::Array(vec![toml::Value::String("mcp".into())]));
    servers.insert("poneglyph".into(), toml::Value::Table(entry));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, toml::to_string_pretty(&root)?).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

/// Copy the OpenCode plugin into `plugin_dir` if missing or stale. Returns
/// whether the file changed.
#[cfg(feature = "opencode")]
fn install_opencode_plugin(plugin_dir: &Path) -> Result<bool> {
    std::fs::create_dir_all(plugin_dir).with_context(|| format!("failed to create {}", plugin_dir.display()))?;
    let path = plugin_dir.join("poneglyph.ts");
    let stale = !path.exists() || std::fs::read_to_string(&path).map(|s| s != OPENCODE_PLUGIN_TS).unwrap_or(true);
    if stale {
        std::fs::write(&path, OPENCODE_PLUGIN_TS).with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(stale)
}

/// Idempotently insert a `poneglyph` MCP entry in opencode's JSON config.
/// OpenCode uses array command format: `{ "command": ["poneglyph", "mcp"] }`.
#[cfg(feature = "opencode")]
fn merge_opencode_mcp_server(path: &Path, exe: &str) -> Result<bool> {
    let mut root = read_json_object(path)?;
    let obj = root.as_object_mut().with_context(|| format!("{} root must be a JSON object", path.display()))?;
    let servers_val = obj.entry("mcp").or_insert_with(|| json!({}));
    let servers = servers_val.as_object_mut().context("\"mcp\" key must be a JSON object")?;
    if servers.contains_key("poneglyph") {
        return Ok(false);
    }
    let entry = json!({ "command": [exe, "mcp"] });
    servers.insert("poneglyph".to_string(), entry);
    write_json(path, &root)?;
    Ok(true)
}

/// Write the bundled poneglyph skill into opencode's skills directory.
#[cfg(feature = "opencode")]
fn install_opencode_skill(skills_dir: &Path) -> Result<bool> {
    let dir = skills_dir.join("poneglyph");
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join("SKILL.md");
    let stale = !path.exists() || std::fs::read_to_string(&path).map(|s| s != SKILL_MD).unwrap_or(true);
    if stale {
        std::fs::write(&path, SKILL_MD).with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(stale)
}

/// Write the bundled poneglyph skill into `skills_dir/poneglyph/SKILL.md` if
/// missing or stale. Claude Code skills are directories, one `SKILL.md` per
/// skill. Returns whether the file changed.
fn install_skill_file(skills_dir: &Path) -> Result<bool> {
    let dir = skills_dir.join("poneglyph");
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join("SKILL.md");
    let stale = !path.exists() || std::fs::read_to_string(&path).map(|s| s != SKILL_MD).unwrap_or(true);
    if stale {
        std::fs::write(&path, SKILL_MD).with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(stale)
}

// ---------------------------------------------------------------------------
// Opt-in agent rule injection (`poneglyph init --inject-rules`): write a
// condensed usage block into project instruction files that already exist.
// Never creates a file the user doesn't have.
// ---------------------------------------------------------------------------

const RULES_START: &str = "<!-- poneglyph:start -->";
const RULES_END: &str = "<!-- poneglyph:end -->";
const RULES_BODY: &str = "## poneglyph: durable memory + code graph\n\nThis project has poneglyph wired up (MCP server `poneglyph mcp`). Prefer\nits tools over re-deriving things or manually scanning directories:\n\n- `remember` / `recall` / `get_project_context` — durable cross-session\n  memory. Call `get_project_context` at session start, `recall` before\n  re-researching something, `remember` for durable facts/decisions/preferences.\n- `codegraph_query` (`callers_of:`/`callees_of:`/`imports_of:`/`tests_for:`/\n  `path:<a>..<b>`, or a bare keyword for a graph-backed name search) and\n  `codegraph_blast_radius` — call/import/test graph. ALWAYS try this FIRST\n  for \"find X\" / \"what calls/imports/breaks if I change X\" questions — it's\n  a targeted index lookup, not a directory walk, so it stays fast as the\n  codebase grows. Fall back to grep/glob only when the graph has nothing.\n  Requires `poneglyph graph init` to have been run once.";
#[allow(dead_code)]
const RULE_FILES: [&str; 3] = ["CLAUDE.md", "AGENTS.md", ".cursorrules"];

/// For each of CLAUDE.md/AGENTS.md/.cursorrules that already exists directly
/// under `project_dir`, idempotently insert/replace a fenced poneglyph usage
/// block. Returns `(filename, changed)` for each file found.
#[allow(dead_code)]
pub fn inject_agent_rules(project_dir: &Path) -> Result<Vec<(String, bool)>> {
    let mut out = Vec::new();
    for name in RULE_FILES {
        let path = project_dir.join(name);
        if !path.exists() {
            continue;
        }
        let changed = inject_rules_block(&path)?;
        out.push((name.to_string(), changed));
    }
    Ok(out)
}

/// Replace the block between `RULES_START`/`RULES_END` markers if present,
/// else append one. Returns whether the file content actually changed.
/// Creates the file if it doesn't exist.
fn inject_rules_block(path: &Path) -> Result<bool> {
    let existing = if path.exists() {
        std::fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?
    } else {
        String::new()
    };
    let block = format!("{RULES_START}\n{RULES_BODY}\n{RULES_END}");

    let new_content = match (existing.find(RULES_START), existing.find(RULES_END)) {
        (Some(start), Some(end)) if end > start => {
            let end = end + RULES_END.len();
            format!("{}{}{}", &existing[..start], block, &existing[end..])
        }
        _ => {
            let sep = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
            format!("{existing}{sep}\n{block}\n")
        }
    };

    if new_content == existing {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, new_content).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

fn read_json_object(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let raw = std::fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value)?).with_context(|| format!("failed to write {}", path.display()))
}

// ---------------------------------------------------------------------------
// Default config.toml generation: every key present but commented out,
// except values resolved by local-provider detection.
// ---------------------------------------------------------------------------

/// Reachable local LLM endpoint, if any.
pub struct Detected {
    pub llm_provider: Option<&'static str>,
    pub llm_base_url: Option<&'static str>,
}

/// Probe well-known local LLM ports with a short timeout. Never blocks long
/// enough to matter for `init`'s UX.
pub fn detect_local_llm() -> Detected {
    let reachable = |port: u16| -> bool {
        format!("127.0.0.1:{port}")
            .parse()
            .ok()
            .is_some_and(|addr| std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok())
    };
    if reachable(11434) {
        Detected { llm_provider: Some("ollama"), llm_base_url: Some("http://localhost:11434/v1") }
    } else if reachable(1234) {
        Detected { llm_provider: Some("lmstudio"), llm_base_url: Some("http://localhost:1234/v1") }
    } else {
        Detected { llm_provider: None, llm_base_url: None }
    }
}

/// Render a full `config.toml`: every key from the schema is present, but
/// commented (so the figment defaults layer applies) unless detection found
/// a concrete value worth uncommenting.
pub fn render_config_template(detected: &Detected) -> String {
    let llm_block = match (detected.llm_provider, detected.llm_base_url) {
        (Some(provider), Some(base_url)) => format!(
            "enabled = true\nprovider = \"{provider}\"  # openai | anthropic | gemini | ollama | lmstudio | gpt4all  (each needs a matching --features llm-* build)\nbase_url = \"{base_url}\"\n# model = \"...\"\n# api_key = \"...\"  # prefer PONEGLYPH_LLM_API_KEY env var\ntimeout_seconds = 60\nmax_generation_tokens = 2048"
        ),
        _ => "# enabled = false\n# provider = \"ollama\"  # openai | anthropic | gemini | ollama | lmstudio | gpt4all  (each needs a matching --features llm-* build)\n# base_url = \"http://localhost:11434/v1\"\n# model = \"...\"\n# api_key = \"...\"  # prefer PONEGLYPH_LLM_API_KEY env var\n# timeout_seconds = 60\n# max_generation_tokens = 2048".to_string(),
    };

    let embedding_block = "# provider = \"local\"        # local | ollama | openai\nmodel_id = \"sentence-transformers/all-MiniLM-L6-v2\"\n# model_path = \"...\"        # relative to data_dir, local provider only\ndimensions = 384\n# device = \"cpu\"            # cpu | cuda\n# batch_size = 32\n# query_prefix = \"\"           # prepended when embedding search queries (e5-family models want \"query: \")\n# passage_prefix = \"\"         # prepended when embedding stored text (e5-family models want \"passage: \")";

    // Build agents section based on enabled features.
    let mut agents_lines = vec!["# claude_code = true"];
    #[cfg(feature = "cursor")]
    agents_lines.push("# cursor = true");
    #[cfg(feature = "gemini")]
    agents_lines.push("# gemini_cli = true");
    #[cfg(feature = "opencode")]
    agents_lines.push("# opencode = true");
    #[cfg(feature = "codex")]
    agents_lines.push("# codex = true");
    #[cfg(feature = "copilot")]
    agents_lines.push("# copilot_cli = true");
    agents_lines.push("# mcp_server_port = 37778");
    let agents_block = agents_lines.join("\n");

    format!(
        r#"# Poneglyph configuration. Commented keys use built-in defaults; uncomment
# to override. See https://github.com/ (poneglyph) for the full reference.

[general]
# data_dir = "..."          # defaults to the XDG data directory
# log_level = "info"        # trace | debug | info | warn | error
# auto_update = true

[embedding]
{embedding_block}

[llm]
{llm_block}

[memory]
# enabled = true
# flush_interval_secs = 5
# max_entry_tokens = 4000
# compression_enabled = false   # caveman-grammar prose compression at rest
# min_relevance_score = 0.6

[memory.layer_retention]
# ephemeral = 0    # 0 = session-only
# short_term = 7
# working = 30
# long_term = 180
# archival = 0     # 0 = permanent

[memory.edges]
# similarity_threshold = 0.82
# temporal_window_secs = 300

[graph]
# enabled = true
# languages = ["rust", "typescript", "javascript", "python", "go"]
# exclude_patterns = ["**/target/**", "**/node_modules/**", "**/.git/**", "**/*.test.ts", "**/*_test.rs"]
# watch_delay_ms = 2000
# blast_radius_depth = 5

[dashboard]
# enabled = true
# port = 3742
# host = "127.0.0.1"
# open_on_start = false
# token = "..."        # prefer PONEGLYPH_DASHBOARD_TOKEN env var
# theme = "system"     # system | dark | light
# items_per_page = 50

[agents]
{agents_block}

[privacy]
# redaction_tags = ["private", "secret", "confidential"]
# exclude_paths = ["**/.env", "**/*.pem", "**/*.key", "**/secrets/**"]
# anonymize_paths = false

[context]
# max_tokens = 2000

[enrichment]
# enabled = false

[decay]
# enabled = true
# min_strength = 0.1
# consolidation_threshold = 0.3
# daily_decay_rate = 0.02

[consolidation]
# enabled = true
# interval_hours = 6
# min_cluster_size = 2
# similarity_threshold = 0.75

[cold_storage]
# enabled = true
# compress_level = 3
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn merge_json_mcp_server_inserts_then_is_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");

        let changed1 = merge_json_mcp_server(&path, "mcpServers", true, "/usr/local/bin/poneglyph").unwrap();
        assert!(changed1);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["poneglyph"]["command"], "/usr/local/bin/poneglyph");
        assert_eq!(v["mcpServers"]["poneglyph"]["type"], "stdio");

        let changed2 = merge_json_mcp_server(&path, "mcpServers", true, "/usr/local/bin/poneglyph").unwrap();
        assert!(!changed2, "second merge must be a no-op");
    }

    #[test]
    fn merge_json_mcp_server_preserves_existing_unrelated_keys() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"mcpServers": {"other-tool": {"command": "other"}}, "unrelated": 1}"#).unwrap();

        merge_json_mcp_server(&path, "mcpServers", true, "poneglyph").unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["other-tool"]["command"], "other");
        assert_eq!(v["unrelated"], 1);
        assert_eq!(v["mcpServers"]["poneglyph"]["command"], "poneglyph");
    }

    #[test]
    fn merge_json_mcp_server_without_type_field() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("opencode.json");
        merge_json_mcp_server(&path, "mcp", false, "poneglyph").unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(v["mcp"]["poneglyph"].get("type").is_none());
        assert_eq!(v["mcp"]["poneglyph"]["args"][0], "mcp");
    }

    #[cfg(feature = "opencode")]
    #[test]
    fn merge_opencode_mcp_server_uses_array_command() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("opencode.json");

        let changed = merge_opencode_mcp_server(&path, "poneglyph").unwrap();
        assert!(changed);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcp"]["poneglyph"]["command"][0], "poneglyph");
        assert_eq!(v["mcp"]["poneglyph"]["command"][1], "mcp");
        assert!(v["mcp"]["poneglyph"].get("args").is_none());

        let changed2 = merge_opencode_mcp_server(&path, "poneglyph").unwrap();
        assert!(!changed2, "second merge must be a no-op");
    }

    #[cfg(feature = "opencode")]
    #[test]
    fn install_opencode_skill_writes_then_is_idempotent() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        assert!(install_opencode_skill(&skills_dir).unwrap());
        assert!(skills_dir.join("poneglyph/SKILL.md").exists());
        assert!(!install_opencode_skill(&skills_dir).unwrap(), "unchanged content must be a no-op");
    }

    #[test]
    fn merge_claude_code_hooks_inserts_all_four_then_idempotent() {
        let dir = tempdir().unwrap();
        let settings_path = dir.path().join("settings.json");
        let hooks_dir = dir.path().join("hooks");

        let changed1 = merge_claude_code_hooks(&settings_path, &hooks_dir).unwrap();
        assert!(changed1);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        for event in ["PostToolUse", "UserPromptSubmit", "Stop", "SessionStart"] {
            assert_eq!(v["hooks"][event].as_array().unwrap().len(), 1, "event {event}");
        }
        assert_eq!(v["hooks"]["PostToolUse"][0]["matcher"], "Edit|Write|MultiEdit|Bash|NotebookEdit|Agent|Task");
        assert!(v["hooks"]["Stop"][0].get("matcher").is_none());

        let changed2 = merge_claude_code_hooks(&settings_path, &hooks_dir).unwrap();
        assert!(!changed2, "second merge must be a no-op");
        let v2: Value = serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(v2["hooks"]["PostToolUse"].as_array().unwrap().len(), 1, "must not duplicate entries");
    }

    #[test]
    fn merge_claude_code_hooks_preserves_unrelated_hook_entries() {
        let dir = tempdir().unwrap();
        let settings_path = dir.path().join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"hooks": {"PostToolUse": [{"matcher": "Other", "hooks": [{"type": "command", "command": "other.sh"}]}]}}"#,
        )
        .unwrap();

        merge_claude_code_hooks(&settings_path, &dir.path().join("hooks")).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(v["hooks"]["PostToolUse"].as_array().unwrap().len(), 2, "must keep the pre-existing entry");
    }

    #[cfg(feature = "codex")]
    #[test]
    fn merge_codex_mcp_server_inserts_then_is_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let changed1 = merge_codex_mcp_server(&path, "poneglyph").unwrap();
        assert!(changed1);
        let v: toml::Value = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(v["mcp_servers"]["poneglyph"]["command"].as_str(), Some("poneglyph"));
        assert_eq!(v["mcp_servers"]["poneglyph"]["args"][0].as_str(), Some("mcp"));

        let changed2 = merge_codex_mcp_server(&path, "poneglyph").unwrap();
        assert!(!changed2, "second merge must be a no-op");
    }

    #[cfg(feature = "codex")]
    #[test]
    fn merge_codex_mcp_server_preserves_existing_table() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[mcp_servers.other]\ncommand = \"other\"\n").unwrap();

        merge_codex_mcp_server(&path, "poneglyph").unwrap();
        let v: toml::Value = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(v["mcp_servers"]["other"]["command"].as_str(), Some("other"));
        assert_eq!(v["mcp_servers"]["poneglyph"]["command"].as_str(), Some("poneglyph"));
    }

    #[test]
    fn install_hook_scripts_writes_all_four_executable() {
        let dir = tempdir().unwrap();
        let hooks_dir = dir.path().join("hooks");
        install_hook_scripts(&hooks_dir).unwrap();
        for name in ["posttooluse.sh", "userpromptsubmit.sh", "stop.sh", "sessionstart.sh"] {
            let path = hooks_dir.join(name);
            assert!(path.exists());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(&path).unwrap().permissions().mode();
                assert_eq!(mode & 0o111, 0o111, "{name} must be executable");
            }
        }
    }

    #[cfg(feature = "opencode")]
    #[test]
    fn install_opencode_plugin_writes_then_is_idempotent() {
        let dir = tempdir().unwrap();
        let plugin_dir = dir.path().join("plugins");
        assert!(install_opencode_plugin(&plugin_dir).unwrap());
        assert!(!install_opencode_plugin(&plugin_dir).unwrap(), "unchanged content must be a no-op");
    }

    #[test]
    fn install_skill_file_writes_then_is_idempotent() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        assert!(install_skill_file(&skills_dir).unwrap());
        assert!(skills_dir.join("poneglyph/SKILL.md").exists());
        assert!(!install_skill_file(&skills_dir).unwrap(), "unchanged content must be a no-op");
    }

    #[test]
    fn setup_claude_code_installs_skill_when_detected() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join(".claude")).unwrap();

        setup_claude_code(true, home, &dir.path().join("hooks"), "poneglyph").unwrap();
        assert!(home.join(".claude/skills/poneglyph/SKILL.md").exists());
    }

    #[test]
    fn render_config_template_uncomments_detected_llm_provider() {
        let detected = Detected { llm_provider: Some("ollama"), llm_base_url: Some("http://localhost:11434/v1") };
        let toml = render_config_template(&detected);
        assert!(toml.contains("provider = \"ollama\""));
        assert!(toml.contains("enabled = true"));
        // Every other section stays commented.
        assert!(toml.contains("# port = 3742"));
        let parsed: toml::Table = toml.parse().expect("template (minus comments) must be valid TOML");
        assert!(parsed.contains_key("llm"));
    }

    #[test]
    fn render_config_template_comments_llm_when_nothing_detected() {
        let detected = Detected { llm_provider: None, llm_base_url: None };
        let toml = render_config_template(&detected);
        assert!(toml.contains("# enabled = false"));
        assert!(!toml.contains("\nenabled = true"));
        let _: toml::Table = toml.parse().expect("fully-commented template must still be valid TOML");
    }

    #[test]
    fn render_config_template_uses_default_model() {
        let detected = Detected { llm_provider: None, llm_base_url: None };
        let toml = render_config_template(&detected);
        assert!(toml.contains("model_id = \"sentence-transformers/all-MiniLM-L6-v2\""));
        assert!(toml.contains("\ndimensions = 384"));
        let parsed: toml::Table = toml.parse().expect("template must be valid TOML");
        assert!(parsed.contains_key("embedding"));
    }

    #[test]
    fn inject_agent_rules_only_touches_existing_files() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "# My project\n\nSome existing notes.\n").unwrap();
        // AGENTS.md and .cursorrules deliberately absent.

        let results = inject_agent_rules(dir.path()).unwrap();
        assert_eq!(results, vec![("CLAUDE.md".to_string(), true)]);
        assert!(!dir.path().join("AGENTS.md").exists(), "must never create files the user doesn't have");
        assert!(!dir.path().join(".cursorrules").exists());

        let content = std::fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
        assert!(content.contains("Some existing notes."), "must preserve existing content");
        assert!(content.contains(RULES_START) && content.contains(RULES_END));
    }

    #[test]
    fn inject_rules_block_is_idempotent_and_replaces_in_place() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        std::fs::write(&path, "# Agents\n\nBefore.\n\nAfter.\n").unwrap();

        assert!(inject_rules_block(&path).unwrap(), "first call inserts the block");
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.contains("Before.") && first.contains("After."));

        assert!(!inject_rules_block(&path).unwrap(), "second call on unchanged content is a no-op");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);

        // Editing the block content (e.g. a future SKILL.md change) replaces
        // in place rather than appending a second block.
        let edited = first.replace(RULES_START, &format!("{RULES_START}\nstale"));
        std::fs::write(&path, &edited).unwrap();
        assert!(inject_rules_block(&path).unwrap());
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after.matches(RULES_START).count(), 1, "must not duplicate the block");
        assert!(!after.contains("stale"));
    }

    #[test]
    fn run_agent_setup_respects_disabled_flags() {
        let agents = AgentsConfig {
            claude_code: false,
            #[cfg(feature = "cursor")]
            cursor: false,
            #[cfg(feature = "gemini")]
            gemini_cli: false,
            #[cfg(feature = "opencode")]
            opencode: false,
            #[cfg(feature = "codex")]
            codex: false,
            #[cfg(feature = "copilot")]
            copilot_cli: false,
            mcp_server_port: 37778,
        };
        let dir = tempdir().unwrap();
        let outcomes = run_agent_setup(&agents, &dir.path().join("hooks"), "poneglyph").unwrap();
        // claude-code always present + any enabled features
        let expected = 1
            + if cfg!(feature = "cursor") { 1 } else { 0 }
            + if cfg!(feature = "gemini") { 1 } else { 0 }
            + if cfg!(feature = "opencode") { 1 } else { 0 }
            + if cfg!(feature = "codex") { 1 } else { 0 }
            + if cfg!(feature = "copilot") { 1 } else { 0 };
        assert_eq!(outcomes.len(), expected);
        assert!(outcomes.iter().all(|o| o.status == SetupStatus::Disabled));
    }
}
