// poneglyph plugin for opencode — MCP + HTTP hybrid capture.
// Drop into your project's (or global) `.opencode/plugins/` directory.
//
// Two capture layers:
//   1. HTTP /ingest  — fire-and-forget event capture (low latency, same as Claude Code hooks)
//   2. MCP tools     — remember/recall/get_project_context (structured queries)
//
// Two injection layers:
//   1. experimental.chat.system.transform — always-fresh project context
//   2. experimental.session.compacting    — context on compaction (fallback)
//
// All failures are swallowed — never blocks the agent.

import type { Plugin } from "@opencode-ai/plugin"

const PORT = parseInt(process.env.PONEGLYPH_PORT || "27271", 10)
const BASE = `http://127.0.0.1:${PORT}`
const TOKEN = process.env.PONEGLYPH_DASHBOARD_TOKEN || ""
const MAX_CONTENT = 4000

// Tools whose output is too noisy / read-only to capture.
const SKIP_TOOLS = new Set(["read", "glob", "grep", "list", "todoread", "webfetch", "skill"])

// File-touching tools that trigger enrichment.
const FILE_TOOLS = new Set(["write", "edit", "multiedit", "notebookedit"])

// Debounce state for consolidate triggers.
const lastConsolidate: Map<string, number> = new Map()
const CONSOLIDATE_DEBOUNCE_MS = 30 * 60 * 1000 // 30 min

// ---------------------------------------------------------------------------
// HTTP capture — fire-and-forget to /ingest
// ---------------------------------------------------------------------------

async function ingest(
  event: string,
  content: string,
  directory: string,
  tool?: string,
  metadata?: Record<string, unknown>,
): Promise<void> {
  try {
    const body: Record<string, unknown> = {
      event,
      client: "opencode",
      content: content.slice(0, MAX_CONTENT),
    }
    if (directory) body.project_path = directory
    if (tool) body.tool = tool
    if (metadata) body.metadata = metadata

    await fetch(`${BASE}/ingest`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...(TOKEN ? { Authorization: `Bearer ${TOKEN}` } : {}),
      },
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(3000),
    })
  } catch {
    // Engine not running — skip silently.
  }
}

// ---------------------------------------------------------------------------
// MCP capture — structured queries via callTool
// ---------------------------------------------------------------------------

async function mcpRemember(
  client: any,
  content: string,
  memoryType: string,
  directory: string,
  tags?: string[],
): Promise<void> {
  try {
    await client.mcp?.callTool?.({
      name: "poneglyph_remember",
      arguments: {
        content: content.slice(0, MAX_CONTENT),
        memory_type: memoryType,
        importance: 0.5,
        project_path: directory,
        ...(tags?.length ? { tags } : {}),
      },
    })
  } catch {
    // MCP server not available.
  }
}

async function mcpRecall(client: any, query: string, directory: string): Promise<string> {
  try {
    const resp = await client.mcp?.callTool?.({
      name: "poneglyph_recall",
      arguments: { query, project_path: directory, limit: 5 },
    })
    if (resp?.content?.[0]?.text) return resp.content[0].text
  } catch {
    // MCP server not available.
  }
  return ""
}

async function mcpContext(client: any, directory: string): Promise<string> {
  try {
    const resp = await client.mcp?.callTool?.({
      name: "poneglyph_get_project_context",
      arguments: { project_path: directory },
    })
    if (resp?.content?.[0]?.text) return resp.content[0].text
  } catch {
    // MCP server not available.
  }
  return ""
}

// ---------------------------------------------------------------------------
// HTTP enrichment — file-level context from /api/enrich
// ---------------------------------------------------------------------------

async function enrichFile(filePath: string, directory: string): Promise<string> {
  try {
    const params = new URLSearchParams({
      file_path: filePath,
      project_path: directory,
      max_tokens: "1000",
    })
    const resp = await fetch(`${BASE}/api/enrich?${params}`, {
      headers: TOKEN ? { Authorization: `Bearer ${TOKEN}` } : {},
      signal: AbortSignal.timeout(3000),
    })
    if (!resp.ok) return ""
    const data = (await resp.json()) as { context?: string }
    return data.context || ""
  } catch {
    return ""
  }
}

// ---------------------------------------------------------------------------
// Consolidate trigger (debounced)
// ---------------------------------------------------------------------------

async function triggerConsolidate(directory: string): Promise<void> {
  const key = directory || "global"
  const now = Date.now()
  const last = lastConsolidate.get(key) || 0
  if (now - last < CONSOLIDATE_DEBOUNCE_MS) return
  lastConsolidate.set(key, now)

  try {
    await fetch(`${BASE}/ingest`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...(TOKEN ? { Authorization: `Bearer ${TOKEN}` } : {}),
      },
      body: JSON.stringify({
        event: "session_end",
        client: "opencode",
        content: "session ended — triggering consolidate",
        project_path: directory,
      }),
      signal: AbortSignal.timeout(3000),
    })
  } catch {
    // Engine not running.
  }
}

// ---------------------------------------------------------------------------
// Plugin entry
// ---------------------------------------------------------------------------

export const PoneglyphCapture: Plugin = async ({ client, directory }) => {
  await client.app.log({
    body: { service: "poneglyph", level: "info", message: "plugin loaded", extra: { directory } },
  })

  return {
    // --- System prompt injection (always fresh, not just on compaction) ---
    "experimental.chat.system.transform": async (_input: any, output: any) => {
      try {
        const ctx = await mcpContext(client, directory)
        if (ctx) {
          output.system = output.system || []
          output.system.push(`## Project Memory\n${ctx}`)
        }
      } catch {
        // Skip.
      }
    },

    // --- Compaction injection (fallback) ---
    "experimental.session.compacting": async (_input: any, output: any) => {
      try {
        const ctx = await mcpContext(client, directory)
        if (ctx) {
          output.context.push(`## Project Memory\n${ctx}`)
        }
      } catch {
        // Skip.
      }
    },

    // --- Session lifecycle ---
    "session.created": async (input: any) => {
      await ingest("session_start", `session started: ${input.id || "unknown"}`, directory)
    },

    "session.idle": async () => {
      await triggerConsolidate(directory)
    },

    "session.deleted": async () => {
      await triggerConsolidate(directory)
    },

    "session.error": async (input: any) => {
      const err = input?.error || input?.message || "unknown error"
      await ingest("session_error", `session error: ${String(err)}`, directory)
    },

    // --- Tool execution ---
    "tool.execute.before": async (input: any, output: any) => {
      const toolName = (input?.tool || "").toLowerCase()
      if (!FILE_TOOLS.has(toolName)) return

      // File enrichment: inject relevant context before file-touching tools.
      const filePath = output?.args?.file_path || output?.args?.filePath || ""
      if (!filePath) return

      const context = await enrichFile(filePath, directory)
      if (context) {
        // Stash for post-execution capture; some plugin SDKs don't support
        // output.context on .before — fall back to recall in .after.
        output._poneglyph_enrich = context
      }
    },

    "tool.execute.after": async (input: any, output: any) => {
      const toolName = (input?.tool || "").toLowerCase()
      if (SKIP_TOOLS.has(toolName)) return

      const args = output?.args ?? {}
      const argsStr = JSON.stringify(args)
      const content = `tool_use(${input.tool}) ${argsStr}`.slice(0, MAX_CONTENT)

      // HTTP capture (fast, fire-and-forget).
      await ingest("tool_use", content, directory, input.tool)

      // MCP remember for high-signal tools.
      await mcpRemember(client, content, "code_context", directory, [input.tool])

      // File enrichment: if a file was touched, capture what changed.
      if (FILE_TOOLS.has(toolName)) {
        const filePath = args.file_path || args.filePath || ""
        if (filePath) {
          await ingest(
            "file_edit",
            `edited ${filePath}: ${argsStr}`.slice(0, MAX_CONTENT),
            directory,
            input.tool,
          )
        }
      }
    },

    // --- Messages ---
    "message.updated": async (input: any) => {
      try {
        const msg = input as any
        const content = typeof msg.content === "string" ? msg.content.slice(0, MAX_CONTENT) : ""
        if (!content) return

        if (msg.role === "user") {
          await ingest("user_message", `user_message: ${content}`, directory)
          await mcpRemember(client, `user_message: ${content}`, "episodic", directory)
        } else if (msg.role === "assistant") {
          await ingest("assistant_message", `assistant_message: ${content}`, directory)
          await mcpRemember(client, `assistant_message: ${content}`, "episodic", directory)
        }
      } catch {
        // Ignore.
      }
    },

    "message.removed": async (input: any) => {
      const msg = input as any
      if (msg?.id) {
        await ingest("assistant_message", `message removed: ${msg.id}`, directory)
      }
    },

    // --- Permissions ---
    "permission.asked": async (input: any) => {
      const desc = input?.description || input?.message || "permission requested"
      await ingest("permission", `permission asked: ${String(desc)}`, directory)
    },

    "permission.replied": async (input: any) => {
      const reply = input?.reply || input?.approved || "unknown"
      await ingest("permission", `permission reply: ${JSON.stringify(reply)}`, directory)
    },

    // --- Tasks ---
    "todo.updated": async (input: any) => {
      const todos = input?.todos || input?.items || []
      const summary = Array.isArray(todos)
        ? todos.map((t: any) => `${t.status || "?"}: ${t.content || t.text || "?"}`).join("; ")
        : JSON.stringify(todos)
      await ingest("todo_update", `todos: ${summary}`.slice(0, MAX_CONTENT), directory, undefined, {
        todos: Array.isArray(todos) ? todos : [],
      })
      await mcpRemember(client, `task state: ${summary}`.slice(0, MAX_CONTENT), "procedural", directory)
    },

    // --- Commands ---
    "command.executed": async (input: any) => {
      const cmd = input?.command || input?.cmd || ""
      if (!cmd) return
      await ingest("command_exec", `command: ${cmd}`.slice(0, MAX_CONTENT), directory)
    },

    // --- File watcher ---
    "file.watcher.updated": async (input: any) => {
      const files = input?.files || input?.paths || []
      if (Array.isArray(files) && files.length > 0) {
        await ingest("file_edit", `files changed: ${files.join(", ")}`.slice(0, MAX_CONTENT), directory)
      }
    },
  }
}
