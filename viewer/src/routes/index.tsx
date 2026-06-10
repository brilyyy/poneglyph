import { Link, createFileRoute } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'

import { api, formatRelative, truncate } from '#/lib/api.ts'
import { MEMORY_TYPES, TYPE_COLORS } from '#/lib/types.ts'
import { TypeBadge } from '#/components/type-badge.tsx'
import { Alert, AlertDescription } from '#/components/ui/alert.tsx'
import {
  Card,
  CardAction,
  CardContent,
  CardHeader,
  CardTitle,
} from '#/components/ui/card.tsx'
import { Empty, EmptyDescription, EmptyTitle } from '#/components/ui/empty.tsx'
import { Skeleton } from '#/components/ui/skeleton.tsx'

export const Route = createFileRoute('/')({ component: Dashboard })

function Dashboard() {
  const stats = useQuery({ queryKey: ['stats'], queryFn: api.stats })
  const recent = useQuery({
    queryKey: ['memories', 'recent'],
    queryFn: () => api.listMemories({ limit: 8 }),
  })

  return (
    <div className="space-y-6">
      <h1 className="text-2xl font-bold">Dashboard</h1>

      {stats.error && (
        <Alert variant="destructive">
          <AlertDescription>{String(stats.error)}</AlertDescription>
        </Alert>
      )}

      <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
        <StatCard label="Memories" value={stats.data?.memory_count} />
        <StatCard label="Edges" value={stats.data?.edge_count} />
        <StatCard label="Projects" value={stats.data?.project_count} />
        <StatCard label="Pending jobs" value={stats.data?.pending_jobs} />
      </div>

      {stats.data && stats.data.memory_count > 0 && (
        <Card>
          <CardHeader>
            <CardTitle>By type</CardTitle>
          </CardHeader>
          <CardContent>
            <TypeBars byType={stats.data.by_type} total={stats.data.memory_count} />
          </CardContent>
        </Card>
      )}

      <Card className="gap-0 pb-0">
        <CardHeader className="pb-4">
          <CardTitle>Recent memories</CardTitle>
          <CardAction>
            <Link
              to="/memories"
              className="text-xs font-medium text-muted-foreground hover:text-foreground"
            >
              view all →
            </Link>
          </CardAction>
        </CardHeader>
        {recent.isLoading && (
          <div className="space-y-2 p-4">
            {[...Array(4)].map((_, i) => (
              <Skeleton key={i} className="h-8 w-full" />
            ))}
          </div>
        )}
        {recent.data && recent.data.results.length === 0 && (
          <Empty>
            <EmptyTitle>No memories yet</EmptyTitle>
            <EmptyDescription>
              Store one with <code>poneglyph remember</code>, the MCP tools, or try{' '}
              <code>poneglyph demo</code>.
            </EmptyDescription>
          </Empty>
        )}
        <ul className="divide-y divide-border">
          {recent.data?.results.map((m) => (
            <li key={m.id}>
              <Link
                to="/memories/$id"
                params={{ id: m.id }}
                className="flex items-center gap-3 px-6 py-3 hover:bg-muted/50"
              >
                <TypeBadge type={m.memory_type} />
                <span className="min-w-0 flex-1 truncate text-sm">{truncate(m.content, 100)}</span>
                <span className="shrink-0 text-xs text-muted-foreground">
                  {formatRelative(m.created_at)}
                </span>
              </Link>
            </li>
          ))}
        </ul>
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

function TypeBars({
  byType,
  total,
}: {
  byType: Partial<Record<string, number>>
  total: number
}) {
  return (
    <div className="space-y-2">
      {MEMORY_TYPES.filter((t) => (byType[t] ?? 0) > 0).map((t) => {
        const n = byType[t] ?? 0
        return (
          <div key={t} className="flex items-center gap-3">
            <span className="w-28 shrink-0 text-xs text-muted-foreground">{t}</span>
            <div className="h-2 flex-1 overflow-hidden rounded-full bg-muted">
              <div
                className="h-full rounded-full"
                style={{ width: `${(n / total) * 100}%`, backgroundColor: TYPE_COLORS[t] }}
              />
            </div>
            <span className="w-10 shrink-0 text-right text-xs tabular-nums text-muted-foreground">
              {n}
            </span>
          </div>
        )
      })}
    </div>
  )
}
