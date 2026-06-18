import { createFileRoute } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'

import { api } from '#/lib/api.ts'
import type { AgentsStatus } from '#/lib/types.ts'
import { Alert, AlertDescription } from '#/components/ui/alert.tsx'
import { Badge } from '#/components/ui/badge.tsx'
import { Card, CardContent, CardHeader, CardTitle } from '#/components/ui/card.tsx'
import { Spinner } from '#/components/ui/spinner.tsx'

export const Route = createFileRoute('/status')({ component: StatusPage })

const AGENT_LABELS: Record<keyof AgentsStatus, string> = {
  claude_code: 'Claude Code',
  cursor: 'Cursor',
  gemini_cli: 'Gemini CLI',
  opencode: 'OpenCode',
  codex: 'Codex',
  copilot_cli: 'Copilot CLI',
}

function StatusPage() {
  const stats = useQuery({ queryKey: ['stats'], queryFn: api.stats })
  const codegraphStats = useQuery({ queryKey: ['codegraph-stats'], queryFn: api.codegraphStats })
  const agents = useQuery({ queryKey: ['agents-status'], queryFn: api.agentsStatus })
  const settings = useQuery({ queryKey: ['settings'], queryFn: api.getSettings })

  return (
    <div className="space-y-6">
      <h1 className="text-2xl font-bold">Status</h1>

      {(stats.error || agents.error || settings.error) && (
        <Alert variant="destructive">
          <AlertDescription>{String(stats.error ?? agents.error ?? settings.error)}</AlertDescription>
        </Alert>
      )}

      <Card>
        <CardHeader>
          <CardTitle>Agent wiring</CardTitle>
        </CardHeader>
        <CardContent>
          {agents.isLoading ? (
            <Spinner />
          ) : agents.data ? (
            <ul className="divide-y divide-border">
              {(Object.keys(AGENT_LABELS) as (keyof AgentsStatus)[]).map((key) => {
                const entry = agents.data![key]
                return (
                  <li key={key} className="flex items-center justify-between py-2 text-sm">
                    <span>{AGENT_LABELS[key]}</span>
                    <div className="flex gap-2">
                      <Badge variant={entry.enabled ? 'default' : 'secondary'}>
                        {entry.enabled ? 'enabled' : 'disabled in config'}
                      </Badge>
                      <Badge variant={entry.detected ? 'default' : 'outline'}>
                        {entry.detected ? 'detected' : 'not detected'}
                      </Badge>
                    </div>
                  </li>
                )
              })}
            </ul>
          ) : null}
          <p className="mt-3 text-xs text-muted-foreground">
            Run <code>poneglyph init</code> to wire up detected agents (MCP server + hooks).
          </p>
        </CardContent>
      </Card>

      <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
        <StatCard label="Memories" value={stats.data?.memory_count} />
        <StatCard label="Memory edges" value={stats.data?.edge_count} />
        <StatCard label="Projects" value={stats.data?.project_count} />
        <StatCard label="Pending jobs" value={stats.data?.pending_jobs} />
        <StatCard label="Code graph files" value={codegraphStats.data?.files} />
        <StatCard label="Code graph nodes" value={codegraphStats.data?.nodes} />
        <StatCard label="Code graph edges" value={codegraphStats.data?.edges} />
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Engine</CardTitle>
        </CardHeader>
        <CardContent className="space-y-1 text-sm">
          <Row label="Embedding model" value={settings.data?.embedding?.model_id} />
          <Row label="LLM enrichment" value={settings.data?.llm?.enabled ? 'on' : 'off'} />
          <Row label="LLM provider" value={settings.data?.llm?.enabled ? settings.data?.llm?.provider : '—'} />
          <Row label="Dashboard port" value={settings.data?.dashboard?.port} />
          <Row label="Compression" value={settings.data?.memory?.compression_enabled ? 'on' : 'off'} />
        </CardContent>
      </Card>
    </div>
  )
}

function StatCard({ label, value }: { label: string; value: number | undefined }) {
  return (
    <Card className="gap-1 py-4">
      <CardContent className="px-4">
        <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">{label}</p>
        <p className="mt-1 text-2xl font-bold">{value?.toLocaleString() ?? '—'}</p>
      </CardContent>
    </Card>
  )
}

function Row({ label, value }: { label: string; value: string | number | undefined }) {
  return (
    <div className="flex items-center justify-between">
      <span className="text-muted-foreground">{label}</span>
      <span className="font-medium">{value ?? '—'}</span>
    </div>
  )
}
