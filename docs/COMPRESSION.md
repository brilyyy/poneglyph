# Memory Compression

Reduces token cost of context injection (`get_project_context`,
`SessionStart` hook) without touching the searchable record. `content` is
never overwritten — compression writes to a separate cache column that
recall/FTS/vector search never read.

## Modes

```toml
[memory]
compression_enabled = false   # off by default
compression_mode = "caveman"  # "caveman" | "semantic"
```

- **`caveman`** — deterministic grammar substitution (`compress::compress`), synchronous, no LLM, no network.
- **`semantic`** — local-LLM extractive rewrite, then caveman-compresses the result too. Falls back to `caveman` when no usable LLM config is reachable, or when the memory is shorter than the minimum-length threshold (skipped, nothing to compress).

Enabling `compression_enabled` without `semantic` mode applied is a no-op
until the pipeline actually runs the job — see job processing below.

## How it runs

Compression is a background job (`JobType::ExtractCompress`), enqueued
after a memory is stored — it never blocks `remember`. The job calls
`extract_compress()`, which prompts the local LLM:

> "Rewrite the text as densely as possible. Preserve every fact, identifier,
> and detail needed to find it again via search. No commentary, no
> preamble, no summary framing. Output only the rewritten text: the
> shortest version that loses no retrievable information."

The result is caveman-compressed again and stored via
`store.set_compressed_content(id, compressed, "semantic")`.

This is distinct from `summarize` (a separate, lossy, display-only job —
1-2 sentence summary, not meant to be searched against).

## Schema (v4)

```sql
ALTER TABLE memories ADD COLUMN compressed_content TEXT;
ALTER TABLE memories ADD COLUMN compression_mode    TEXT;
```

Additive only — no migration steps beyond the normal migration runner
(`poneglyph init` / first `mcp` after upgrading). See
[MIGRATION.md](MIGRATION.md) for the version history.

## Checking savings

`GET /api/token-savings` (dashboard: `/token-savings`) samples up to 200
memories and reports estimated caveman-compression savings:

```json
{ "sampled_memories": 200, "original_bytes": 48213, "compressed_bytes": 19042, "savings_pct": 60.5, "compression_enabled": false }
```

## See also

- [CODEGRAPH.md](CODEGRAPH.md) — `/token-savings` dashboard route detail
