import { Link, createFileRoute } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'

import { api, formatDate, truncate } from '@/lib/api'
import { Card, CardHeader, EmptyNote, ErrorNote, Spinner, TypeBadge } from '@/components/ui'

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

      {stats.error && <ErrorNote error={stats.error} />}
      <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
        <StatCard label="Memories" value={stats.data?.memory_count} />
        <StatCard label="Edges" value={stats.data?.edge_count} />
        <StatCard label="Projects" value={stats.data?.project_count} />
        <StatCard label="Pending jobs" value={stats.data?.pending_jobs} />
      </div>

      <Card>
        <CardHeader
          title="Recent memories"
          action={
            <Link to="/memories" className="text-xs font-medium text-zinc-500 hover:text-zinc-900">
              view all →
            </Link>
          }
        />
        {recent.isLoading && <Spinner />}
        {recent.error && (
          <div className="p-4">
            <ErrorNote error={recent.error} />
          </div>
        )}
        {recent.data && recent.data.results.length === 0 && (
          <EmptyNote>No memories yet. Store one with `poneglyph remember` or the MCP tools.</EmptyNote>
        )}
        <ul className="divide-y divide-zinc-100">
          {recent.data?.results.map((m) => (
            <li key={m.id}>
              <Link
                to="/memories/$id"
                params={{ id: m.id }}
                className="flex items-center gap-3 px-4 py-3 hover:bg-zinc-50"
              >
                <TypeBadge type={m.memory_type} />
                <span className="min-w-0 flex-1 truncate text-sm">{truncate(m.content, 100)}</span>
                <span className="shrink-0 text-xs text-zinc-400">{formatDate(m.created_at)}</span>
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
    <Card className="p-4">
      <p className="text-xs font-medium uppercase tracking-wide text-zinc-400">{label}</p>
      <p className="mt-1 text-2xl font-bold">{value ?? '—'}</p>
    </Card>
  )
}
