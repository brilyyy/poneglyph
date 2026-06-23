import { useState } from 'react'
import { createFileRoute } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'

import { api, formatRelative } from '#/lib/api.ts'
import type { AgentsStatus, ProjectContext } from '#/lib/types.ts'
import { Alert, AlertDescription } from '#/components/ui/alert.tsx'
import { Badge } from '#/components/ui/badge.tsx'
import { Button } from '#/components/ui/button.tsx'
import { Card, CardContent, CardHeader, CardTitle } from '#/components/ui/card.tsx'
import { Input } from '#/components/ui/input.tsx'
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
  const services = useQuery({
    queryKey: ['services-status'],
    queryFn: api.servicesStatus,
    refetchInterval: 10_000,
  })

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
          <CardTitle>Services</CardTitle>
        </CardHeader>
        <CardContent>
          {services.isLoading ? (
            <Spinner />
          ) : services.data ? (
            <ul className="divide-y divide-border text-sm">
              <li className="flex items-center justify-between py-2">
                <span>MCP engine (port {services.data.mcp.port})</span>
                <UpBadge up={services.data.mcp.up} />
              </li>
              <li className="flex items-center justify-between py-2">
                <span>
                  LLM
                  {services.data.llm.enabled
                    ? ` (${services.data.llm.provider} · ${services.data.llm.model ?? '—'})`
                    : ' (disabled)'}
                </span>
                <UpBadge up={services.data.llm.up} />
              </li>
              <li className="flex items-center justify-between py-2">
                <span>Viewer (port {services.data.viewer.port})</span>
                <UpBadge up={services.data.viewer.up} />
              </li>
            </ul>
          ) : null}
        </CardContent>
      </Card>

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

      <Card>
        <CardHeader>
          <CardTitle>Consolidation pipeline</CardTitle>
        </CardHeader>
        <CardContent className="space-y-1 text-sm">
          <Row
            label="Scheduled consolidation"
            value={settings.data?.consolidation?.enabled ? 'on' : 'off'}
          />
          <Row
            label="Interval"
            value={
              settings.data?.consolidation?.enabled
                ? `${settings.data.consolidation.interval_hours}h`
                : '—'
            }
          />
          <Row label="Last run" value={formatRelative(stats.data?.last_consolidation_at ?? null)} />
          {Object.keys(stats.data?.by_tier ?? {}).length > 0 && (
            <div className="flex items-center justify-between">
              <span className="text-muted-foreground">By tier</span>
              <div className="flex gap-1">
                {Object.entries(stats.data?.by_tier ?? {}).map(([tier, count]) => (
                  <Badge key={tier} variant="secondary">
                    {tier}: {count}
                  </Badge>
                ))}
              </div>
            </div>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Graph coverage</CardTitle>
        </CardHeader>
        <CardContent className="space-y-1 text-sm text-muted-foreground">
          <p>
            The graph &amp; code graph views sample by default (the totals above are always exact, regardless of the
            render-limit slider on each view).
          </p>
          <p>Visually encoded: node color ← type/kind · node size ← importance (memory) / connection count (code).</p>
          <p>Memory graph also encodes: opacity ← tier (hot/warm/cold) · link width ← edge weight.</p>
          <p>Not yet encoded: access_count, is_decoy, strength.</p>
        </CardContent>
      </Card>

      <ProjectContextPreview />
    </div>
  )
}

function ProjectContextPreview() {
  const [path, setPath] = useState('')
  const [result, setResult] = useState<ProjectContext | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)

  const load = async () => {
    if (!path.trim()) return
    setLoading(true)
    setError(null)
    try {
      setResult(await api.context({ project_path: path.trim() }))
    } catch (e) {
      setError(String(e))
      setResult(null)
    } finally {
      setLoading(false)
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>Project context preview</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        <p className="text-sm text-muted-foreground">
          Preview the ranked context string injected into agent sessions at start (via{' '}
          <code>/api/context</code>) — not surfaced anywhere else in the dashboard.
        </p>
        <div className="flex gap-2">
          <Input
            placeholder="/path/to/project"
            value={path}
            onChange={(e) => setPath(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && void load()}
          />
          <Button onClick={() => void load()} disabled={loading || !path.trim()}>
            {loading ? <Spinner /> : 'Load'}
          </Button>
        </div>
        {error && (
          <Alert variant="destructive">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}
        {result && (
          <div className="space-y-1">
            <p className="text-xs text-muted-foreground">{result.memory_count} memories included</p>
            <pre className="max-h-64 overflow-auto whitespace-pre-wrap rounded-md border border-border bg-muted/30 p-3 text-xs">
              {result.context || '(empty)'}
            </pre>
          </div>
        )}
      </CardContent>
    </Card>
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

function UpBadge({ up }: { up: boolean }) {
  return <Badge variant={up ? 'default' : 'destructive'}>{up ? 'up' : 'down'}</Badge>
}

function Row({ label, value }: { label: string; value: string | number | undefined }) {
  return (
    <div className="flex items-center justify-between">
      <span className="text-muted-foreground">{label}</span>
      <span className="font-medium">{value ?? '—'}</span>
    </div>
  )
}
