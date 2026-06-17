// poneglyph passive-capture plugin for opencode (PRD §8.9, SHOULD).
// Drop into your project's (or global) `.opencode/plugins/` directory.
//
// Captures tool executions and assistant messages via the local poneglyph
// /ingest endpoint. Never throws, never blocks — failures silently swallowed.

import type { Plugin } from "@opencode-ai/plugin"

const PORT = process.env.PONEGLYPH_PORT ?? "3742"
const TOKEN = process.env.PONEGLYPH_TOKEN
const URL = `http://127.0.0.1:${PORT}/ingest`

const SKIP_TOOLS = new Set(["read", "glob", "grep", "list", "todoread", "webfetch"])

function headers(): Record<string, string> {
  const h: Record<string, string> = { "Content-Type": "application/json" }
  if (TOKEN) h["Authorization"] = `Bearer ${TOKEN}`
  return h
}

async function ingest(payload: Record<string, unknown>): Promise<void> {
  try {
    await fetch(URL, {
      method: "POST",
      headers: headers(),
      body: JSON.stringify(payload),
      signal: AbortSignal.timeout(2000),
    })
  } catch {
    // Never block the agent on capture failures.
  }
}

export const PoneglyphCapture: Plugin = async ({ directory }) => {
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

    // Capture assistant messages as episodic memories.
    "message.updated": async (input) => {
      try {
        const msg = input as any
        if (msg.role !== "assistant") return
        const content = typeof msg.content === "string"
          ? msg.content.slice(0, 4000)
          : ""
        if (!content) return

        await ingest({
          event: "assistant_message",
          client: "opencode",
          project_path: directory,
          content,
          metadata: { session_id: msg.sessionID ?? null },
        })
      } catch {
        // Ignore.
      }
    },
  }
}
