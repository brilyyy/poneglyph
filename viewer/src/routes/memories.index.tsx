import { Link, createFileRoute } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'

import { api, formatDate, truncate } from '@/lib/api'
import { MEMORY_TYPES } from '@/lib/types'
import { Button, Card, EmptyNote, ErrorNote, Select, Spinner, TypeBadge } from '@/components/ui'

const PAGE_SIZE = 25

type MemoriesSearch = {
  type?: string
  project?: string
  page?: number
}

export const Route = createFileRoute('/memories/')({
  validateSearch: (search: Record<string, unknown>): MemoriesSearch => ({
    type: typeof search.type === 'string' ? search.type : undefined,
    project: typeof search.project === 'string' ? search.project : undefined,
    page: typeof search.page === 'number' ? search.page : undefined,
  }),
  component: MemoriesPage,
})

function MemoriesPage() {
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  const page = search.page ?? 0

  const projects = useQuery({ queryKey: ['projects'], queryFn: api.projects })
  const memories = useQuery({
    queryKey: ['memories', search.type, search.project, page],
    queryFn: () =>
      api.listMemories({
        type: search.type,
        project_path: search.project,
        limit: PAGE_SIZE,
        offset: page * PAGE_SIZE,
      }),
  })

  const total = memories.data?.total ?? 0
  const pages = Math.max(1, Math.ceil(total / PAGE_SIZE))

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-bold">Memories</h1>
        <span className="text-sm text-zinc-400">{total} total</span>
      </div>

      <div className="flex gap-2">
        <Select
          value={search.type ?? ''}
          onChange={(e) =>
            navigate({ search: { ...search, type: e.target.value || undefined, page: 0 } })
          }
        >
          <option value="">all types</option>
          {MEMORY_TYPES.map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </Select>
        <Select
          value={search.project ?? ''}
          onChange={(e) =>
            navigate({ search: { ...search, project: e.target.value || undefined, page: 0 } })
          }
        >
          <option value="">all projects</option>
          {projects.data?.projects.map((p) => (
            <option key={p.id} value={p.path}>
              {p.name}
            </option>
          ))}
        </Select>
      </div>

      <Card>
        {memories.isLoading && <Spinner />}
        {memories.error && (
          <div className="p-4">
            <ErrorNote error={memories.error} />
          </div>
        )}
        {memories.data && memories.data.results.length === 0 && (
          <EmptyNote>No memories match these filters.</EmptyNote>
        )}
        <ul className="divide-y divide-zinc-100">
          {memories.data?.results.map((m) => (
            <li key={m.id}>
              <Link
                to="/memories/$id"
                params={{ id: m.id }}
                className="flex items-center gap-3 px-4 py-3 hover:bg-zinc-50"
              >
                <TypeBadge type={m.memory_type} />
                <span className="min-w-0 flex-1 truncate text-sm">{truncate(m.content, 110)}</span>
                <span className="shrink-0 text-xs text-zinc-400">imp {m.importance.toFixed(2)}</span>
                <span className="shrink-0 text-xs text-zinc-400">{formatDate(m.created_at)}</span>
              </Link>
            </li>
          ))}
        </ul>
      </Card>

      {pages > 1 && (
        <div className="flex items-center gap-3">
          <Button
            variant="ghost"
            disabled={page === 0}
            onClick={() => navigate({ search: { ...search, page: page - 1 } })}
          >
            ← prev
          </Button>
          <span className="text-sm text-zinc-500">
            page {page + 1} / {pages}
          </span>
          <Button
            variant="ghost"
            disabled={page + 1 >= pages}
            onClick={() => navigate({ search: { ...search, page: page + 1 } })}
          >
            next →
          </Button>
        </div>
      )}
    </div>
  )
}
