# poneglyph — opencode passive-capture plugin

POSTs opencode tool executions to the local poneglyph `/ingest` endpoint.

## Install

```sh
mkdir -p .opencode/plugin        # or ~/.config/opencode/plugin for global
cp poneglyph.ts .opencode/plugin/
```

Requires `poneglyph serve` running (HTTP on `127.0.0.1:3742` by default).

## Environment

| Variable | Default | Purpose |
|---|---|---|
| `PONEGLYPH_PORT` | `3742` | HTTP port of `poneglyph serve` |
| `PONEGLYPH_TOKEN` | unset | Bearer token, required if `server.api_token` is set |

## Caveat

opencode's plugin API changes between versions — if capture doesn't work,
check the hook name (`tool.execute.after`) against your installed version's
plugin docs (https://opencode.ai/docs/plugins/). The plugin never blocks the
agent: all failures are swallowed with a 2s fetch timeout.
