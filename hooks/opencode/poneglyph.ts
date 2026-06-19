// poneglyph plugin for opencode — pure MCP, no HTTP dependency.
// Drop into your project's (or global) `.opencode/plugins/` directory.
//
// Uses the poneglyph MCP server tools directly. Captures tool executions
// via structured logging and injects project context into session compaction.
// Never throws, never blocks — failures silently swallowed.

import type { Plugin } from "@opencode-ai/plugin"

export const PoneglyphCapture: Plugin = async ({ client, directory }) => {
  await client.app.log({
    body: { service: "poneglyph", level: "info", message: "plugin loaded", extra: { directory } },
  })

  return {
    "experimental.session.compacting": async (_input, output) => {
      try {
        const resp = await (client as any).mcp?.callTool?.({
          name: "poneglyph_get_project_context",
          arguments: { project_path: directory },
        })
        if (resp?.content?.[0]?.text) {
          output.context.push(`## Project Memory\n${resp.content[0].text}`)
        }
      } catch {
        // poneglyph MCP server not running — skip silently.
      }
    },

    "tool.execute.after": async (input, output) => {
      await client.app.log({
        body: {
          service: "poneglyph",
          level: "debug",
          message: `tool: ${input.tool}`,
          extra: { args: JSON.stringify(output.args ?? {}).slice(0, 2000) },
        },
      })
    },

    "message.updated": async (input) => {
      try {
        const msg = input as any
        if (msg.role === "user" || msg.role === "assistant") {
          await client.app.log({
            body: { service: "poneglyph", level: "debug", message: `${msg.role} message` },
          })
        }
      } catch {
        // Ignore.
      }
    },
  }
}
