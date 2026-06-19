// poneglyph plugin for opencode — pure MCP, no HTTP dependency.
// Drop into your project's (or global) `.opencode/plugins/` directory.
//
// Captures tool executions and messages as memories via the poneglyph MCP
// server. Injects project context into session compaction. Never throws,
// never blocks — failures silently swallowed.

import type { Plugin } from "@opencode-ai/plugin"

const SKIP_TOOLS = new Set(["read", "glob", "grep", "list", "todoread", "webfetch", "skill"])

async function remember(client: any, content: string, memoryType: string, directory: string, extra?: Record<string, any>) {
  try {
    await client.mcp?.callTool?.({
      name: "poneglyph_remember",
      arguments: {
        content: content.slice(0, 4000),
        memory_type: memoryType,
        importance: 0.5,
        project_path: directory,
        ...(extra ? { tags: Object.keys(extra) } : {}),
      },
    })
  } catch {
    // poneglyph MCP server not running — skip silently.
  }
}

export const PoneglyphCapture: Plugin = async ({ client, directory }) => {
  await client.app.log({
    body: { service: "poneglyph", level: "info", message: "plugin loaded", extra: { directory } },
  })

  return {
    "experimental.session.compacting": async (_input, output) => {
      try {
        const resp = await client.mcp?.callTool?.({
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
      if (SKIP_TOOLS.has(input.tool.toLowerCase())) return

      const toolName = input.tool
      const args = output?.args ?? {}
      const argsStr = JSON.stringify(args)
      const content = `tool_use(${toolName}) ${argsStr}`.slice(0, 4000)

      await remember(client, content, "code_context", directory, { tool: toolName })
    },

    "message.updated": async (input) => {
      try {
        const msg = input as any
        const content = typeof msg.content === "string" ? msg.content.slice(0, 4000) : ""
        if (!content) return

        if (msg.role === "user") {
          await remember(client, `user_message: ${content}`, "episodic", directory)
        } else if (msg.role === "assistant") {
          await remember(client, `assistant_message: ${content}`, "episodic", directory)
        }
      } catch {
        // Ignore.
      }
    },
  }
}
