import { createFileRoute, useNavigate } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'

import { api, formatRelative, truncate } from '#/lib/api.ts'
import { MEMORY_TYPES } from '#/lib/types.ts'
import { TypeBadge } from '#/components/type-badge.tsx'
import { Alert, AlertDescription } from '#/components/ui/alert.tsx'
import { Button } from '#/components/ui/button.tsx'
import { Card } from '#/components/ui/card.tsx'
import { Empty, EmptyDescription, EmptyTitle } from '#/components/ui/empty.tsx'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '#/components/ui/select.tsx'
import { Skeleton } from '#/components/ui/skeleton.tsx'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '#/components/ui/table.tsx'

const PAGE_SIZE = 25
/** Radix Select forbids empty-string item values. */
const ALL = 'all'

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
  const rowNavigate = useNavigate()
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
        <span className="text-sm text-muted-foreground">{total.toLocaleString()} total</span>
      </div>

      <div className="flex gap-2">
        <Select
          value={search.type ?? ALL}
          onValueChange={(v) =>
            navigate({ search: { ...search, type: v === ALL ? undefined : v, page: 0 } })
          }
        >
          <SelectTrigger className="w-40">
            <SelectValue placeholder="all types" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL}>all types</SelectItem>
            {MEMORY_TYPES.map((t) => (
              <SelectItem key={t} value={t}>
                {t}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>

        <Select
          value={search.project ?? ALL}
          onValueChange={(v) =>
            navigate({ search: { ...search, project: v === ALL ? undefined : v, page: 0 } })
          }
        >
          <SelectTrigger className="w-52">
            <SelectValue placeholder="all projects" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL}>all projects</SelectItem>
            {projects.data?.projects.map((p) => (
              <SelectItem key={p.id} value={p.path}>
                {p.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      {memories.error && (
        <Alert variant="destructive">
          <AlertDescription>{String(memories.error)}</AlertDescription>
        </Alert>
      )}

      <Card className="overflow-hidden py-0">
        {memories.isLoading ? (
          <div className="space-y-2 p-4">
            {[...Array(8)].map((_, i) => (
              <Skeleton key={i} className="h-9 w-full" />
            ))}
          </div>
        ) : memories.data && memories.data.results.length === 0 ? (
          <Empty>
            <EmptyTitle>No memories match</EmptyTitle>
            <EmptyDescription>Try clearing the filters.</EmptyDescription>
          </Empty>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="w-32">Type</TableHead>
                <TableHead>Content</TableHead>
                <TableHead className="w-24 text-right">Importance</TableHead>
                <TableHead className="w-32 text-right">Created</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {memories.data?.results.map((m) => (
                <TableRow
                  key={m.id}
                  className="cursor-pointer"
                  onClick={() => rowNavigate({ to: '/memories/$id', params: { id: m.id } })}
                >
                  <TableCell>
                    <TypeBadge type={m.memory_type} />
                  </TableCell>
                  <TableCell className="max-w-0 truncate">{truncate(m.content, 110)}</TableCell>
                  <TableCell className="text-right tabular-nums text-muted-foreground">
                    {m.importance.toFixed(2)}
                  </TableCell>
                  <TableCell className="text-right text-xs text-muted-foreground">
                    {formatRelative(m.created_at)}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </Card>

      {pages > 1 && (
        <div className="flex items-center gap-3">
          <Button
            variant="ghost"
            size="sm"
            disabled={page === 0}
            onClick={() => navigate({ search: { ...search, page: page - 1 } })}
          >
            ← prev
          </Button>
          <span className="text-sm text-muted-foreground">
            page {page + 1} / {pages}
          </span>
          <Button
            variant="ghost"
            size="sm"
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
