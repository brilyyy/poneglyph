// poneglyph passive-capture plugin for opencode (PRD §8.9, SHOULD).
// Drop into your project's (or global) `.opencode/plugins/` directory.
//
// Captures tool executions and user/assistant messages via the local
// poneglyph /ingest endpoint, plus a best-effort project-context fetch on
// load (parity with Claude Code's hooks). Never throws, never blocks —
// failures silently swallowed.

import type { Plugin } from "@opencode-ai/plugin"

const PORT = process.env.PONEGLYPH_PORT ?? "3742"
const TOKEN = process.env.PONEGLYPH_TOKEN
const CONTEXT_TOKENS = process.env.PONEGLYPH_CONTEXT_TOKENS ?? "600"
const BASE_URL = `http://127.0.0.1:${PORT}`

const SKIP_TOOLS = new Set(["read", "glob", "grep", "list", "todoread", "webfetch"])

function headers(): Record<string, string> {
  const h: Record<string, string> = { "Content-Type": "application/json" }
  if (TOKEN) h["Authorization"] = `Bearer ${TOKEN}`
  return h
}

async function ingest(payload: Record<string, unknown>): Promise<void> {
  try {
    await fetch(`${BASE_URL}/ingest`, {
      method: "POST",
      headers: headers(),
      body: JSON.stringify(payload),
      signal: AbortSignal.timeout(2000),
    })
  } catch {
    // Never block the agent on capture failures.
  }
}

// Best-effort session-start context injection. Unlike Claude Code's
// SessionStart hook (whose stdout is injected into the transcript), opencode's
// plugin factory has no return-value injection point at load time — this can
// only surface prior project memory via the console, not place it in the
// model's context. Documented limitation, see README.
async function logProjectContext(directory: string): Promise<void> {
  try {
    const url = new URL(`${BASE_URL}/api/context`)
    url.searchParams.set("project_path", directory)
    url.searchParams.set("max_tokens", CONTEXT_TOKENS)
    const resp = await fetch(url, { headers: headers(), signal: AbortSignal.timeout(2000) })
    if (!resp.ok) return
    const body = (await resp.json()) as { context?: string }
    if (body.context) console.error(`[poneglyph] project memory:\n${body.context}`)
  } catch {
    // Never block plugin load on context-fetch failures.
  }
}

export const PoneglyphCapture: Plugin = async ({ directory }) => {
  void logProjectContext(directory)

  return {
    // Capture tool executions (write tools only).
    "tool.execute.after": async (input, output) => {
      if (SKIP_TOOLS.has(input.tool.toLowerCase())) return

      const content = `${input.tool} ${JSON.stringify(output.args ?? {})}`.slice(0, 4000)
      await ingest({
        event: "tool_use",
        client: "opencode",
        project_path: directory,
        tool: input.tool,
        content,
        metadata: { session_id: input.sessionID ?? null },
      })
    },

    // Capture user and assistant messages as episodic memories (parity with
    // Claude Code's userpromptsubmit.sh + stop.sh).
    "message.updated": async (input) => {
      try {
        const msg = input as any
        const content = typeof msg.content === "string" ? msg.content.slice(0, 4000) : ""
        if (!content) return

        if (msg.role === "user") {
          await ingest({
            event: "user_message",
            client: "opencode",
            project_path: directory,
            content,
            metadata: { session_id: msg.sessionID ?? null },
          })
        } else if (msg.role === "assistant") {
          await ingest({
            event: "assistant_message",
            client: "opencode",
            project_path: directory,
            content,
            metadata: { session_id: msg.sessionID ?? null },
          })
        }
      } catch {
        // Ignore.
      }
    },
  }
}
