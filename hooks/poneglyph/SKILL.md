---
name: poneglyph
description: |
  Use poneglyph's MCP tools for durable cross-session memory (remember/recall/
  get_project_context) and code-impact analysis (codegraph_query/
  codegraph_blast_radius) instead of ad-hoc grepping or re-researching things
  already known. Triggers when: starting work on a project that has poneglyph
  wired up, asking "what calls/imports/breaks if I change X", deciding whether
  a fact/decision/preference is worth keeping for later, or before repeating
  research that may already be stored.
---

# poneglyph

Local-first memory + code-graph MCP server. Two jobs: remember things across
sessions, and answer code-impact questions precisely instead of by grep.

## When to use this

- A fact, decision, or preference worth keeping past this session → `remember`.
- "What calls this?" / "What imports this module?" / "What tests cover this?"
  / "What breaks if I change this file or function?" → `codegraph_query` or
  `codegraph_blast_radius`, not `grep`. Grep finds text matches; the code
  graph finds actual call/import/test edges — no false positives from a
  function name that also appears in a comment or string.
- Before spending time re-deriving something → `recall` first. It may already
  be stored from a prior session.

## Steps

1. **Session start**: call `get_project_context(project_path)` to load prior
   memory for this project before doing anything else.
2. **Before a refactor or "what depends on this" question**: call
   `codegraph_blast_radius(target, depth)` where `target` is a file path
   (relative to the project root) or a symbol name. It returns the root
   symbol(s), transitive dependents (callers/importers, with depth), and
   covering tests. Follow up with `codegraph_query` for a narrower question:
   - `callers_of:<name>` — who calls this
   - `callees_of:<name>` — what this calls
   - `imports_of:<name>` — who imports this
   - `tests_for:<name>` — tests that cover this
   - a bare keyword — substring search over symbol names
3. **Mid-task, when you land on a durable fact/decision**: call
   `remember(content, memory_type, tags)`. Keep `content` short and factual —
   call sites and implementation detail already live in the code graph, don't
   duplicate them into memory.
4. **Before researching something from scratch**: call `recall(query)` — it
   may already be answered from a previous session.
5. **Hygiene**: `list_memories` to see what's stored for a project,
   `update_memory` to correct something instead of creating a duplicate,
   `forget` to remove something that's no longer true.

## Tips

- `codegraph_query`/`codegraph_blast_radius` require `poneglyph graph init`
  (or `update`) to have been run on the project at least once. If both return
  empty results for a target you know exists, that's the likely cause — say
  so rather than concluding the symbol doesn't exist.
- Tag memories on the way in (project, intent, file paths) so a later `recall`
  with a tag filter actually narrows results.
- `remember` is for durable, cross-session facts — not a substitute for
  normal in-conversation reasoning or for content that's already in the code
  graph.
