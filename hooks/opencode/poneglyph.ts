// poneglyph passive-capture plugin for opencode (PRD §8.9, SHOULD).
// Drop into your project's (or global) `.opencode/plugin/` directory.
//
// Thin shim: POSTs tool executions to the local poneglyph /ingest endpoint.
// Never throws, never blocks — capture failures are silently swallowed.
//
// NOTE: opencode's plugin API moves quickly; verify the hook name against
// your installed version (https://opencode.ai/docs/plugins/).

import type { Plugin } from "@opencode-ai/plugin"

const PORT = process.env.PONEGLYPH_PORT ?? "3742"
const TOKEN = process.env.PONEGLYPH_TOKEN

const SKIP_TOOLS = new Set(["read", "glob", "grep", "list", "todowrite", "todoread", "webfetch"])

export const PoneglyphCapture: Plugin = async ({ directory }) => {
  return {
    "tool.execute.after": async (input, output) => {
      try {
        if (SKIP_TOOLS.has(input.tool.toLowerCase())) return

        const content = `${input.tool} ${JSON.stringify(output.args ?? {})}`.slice(0, 4000)
        const headers: Record<string, string> = { "Content-Type": "application/json" }
        if (TOKEN) headers["Authorization"] = `Bearer ${TOKEN}`

        await fetch(`http://127.0.0.1:${PORT}/ingest`, {
          method: "POST",
          headers,
          body: JSON.stringify({
            event: "tool_use",
            client: "opencode",
            project_path: directory,
            tool: input.tool,
            content,
            metadata: { session_id: input.sessionID ?? null },
          }),
          signal: AbortSignal.timeout(2000),
        })
      } catch {
        // Never block the agent on capture failures.
      }
    },
  }
}
