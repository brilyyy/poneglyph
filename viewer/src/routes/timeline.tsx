import { useState } from 'react'
import { createFileRoute, Link } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'

import { api, formatRelative, truncate } from '#/lib/api.ts'
import { TypeBadge } from '#/components/type-badge.tsx'
import { Alert, AlertDescription } from '#/components/ui/alert.tsx'
import { Button } from '#/components/ui/button.tsx'
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
  Timeline,
  TimelineContent,
  TimelineDate,
  TimelineHeader,
  TimelineIndicator,
  TimelineItem,
  TimelineSeparator,
  TimelineTitle,
} from '#/components/ui/timeline.tsx'

const PAGE_SIZE = 20
const ALL = 'all'

type TimelineSearch = {
  project?: string
}

export const Route = createFileRoute('/timeline')({
  validateSearch: (search: Record<string, unknown>): TimelineSearch => ({
    project: typeof search.project === 'string' ? search.project : undefined,
  }),
  component: TimelinePage,
})

function TimelinePage() {
  const search = Route.useSearch()
  const navigate = Route.useNavigate()

  const projects = useQuery({ queryKey: ['projects'], queryFn: api.projects })
  const [offset, setOffset] = useState(0)

  const timeline = useQuery({
    queryKey: ['timeline', search.project, offset],
    queryFn: () =>
      api.timeline({
        project_path: search.project,
        limit: PAGE_SIZE,
        offset,
      }),
  })

  const sessions = timeline.data?.sessions ?? []
  const total = timeline.data?.total ?? 0
  const hasMore = offset + sessions.length < total

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-bold">Timeline</h1>
        <span className="text-sm text-muted-foreground">
          {total} session{total !== 1 ? 's' : ''}
        </span>
      </div>

      <div className="flex gap-2">
        <Select
          value={search.project ?? ALL}
          onValueChange={(v) =>
            navigate({ search: { project: v === ALL ? undefined : v } })
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

      {timeline.error && (
        <Alert variant="destructive">
          <AlertDescription>{String(timeline.error)}</AlertDescription>
        </Alert>
      )}

      {timeline.isLoading ? (
        <div className="space-y-3">
          {[...Array(5)].map((_, i) => (
            <Skeleton key={i} className="h-24 w-full" />
          ))}
        </div>
      ) : sessions.length === 0 ? (
        <Empty>
          <EmptyTitle>No sessions yet</EmptyTitle>
          <EmptyDescription>
            Memories are grouped into sessions as they are captured.
          </EmptyDescription>
        </Empty>
      ) : (
        <>
          <Timeline defaultValue={sessions.length}>
            {sessions.map((s, i) => (
              <TimelineItem key={s.session_id ?? `offset-${offset}-${i}`} step={i + 1}>
                <TimelineHeader>
                  <TimelineDate>{formatRelative(s.started_at)}</TimelineDate>
                  <TimelineTitle>
                    {s.session_id ?? formatDateShort(s.started_at)}
                    {s.project_name && (
                      <span className="ml-2 text-xs text-muted-foreground">
                        {s.project_name}
                      </span>
                    )}
                  </TimelineTitle>
                  <TimelineIndicator />
                </TimelineHeader>
                <TimelineContent>
                  <div className="space-y-1">
                    {s.memories.slice(0, 5).map((m) => (
                      <Link
                        key={m.id}
                        to="/memories/$id"
                        params={{ id: m.id }}
                        className="flex items-center gap-2 text-sm hover:underline"
                      >
                        <TypeBadge type={m.memory_type} />
                        <span className="max-w-md truncate text-foreground">
                          {truncate(m.content, 80)}
                        </span>
                        <span className="ml-auto shrink-0 text-xs text-muted-foreground">
                          {formatRelative(m.created_at)}
                        </span>
                      </Link>
                    ))}
                    {s.memories.length > 5 && (
                      <p className="text-xs text-muted-foreground">
                        +{s.memories.length - 5} more
                      </p>
                    )}
                  </div>
                </TimelineContent>
                <TimelineSeparator />
              </TimelineItem>
            ))}
          </Timeline>

          {hasMore && (
            <div className="flex justify-center">
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setOffset((o) => o + PAGE_SIZE)}
              >
                load more
              </Button>
            </div>
          )}
        </>
      )}
    </div>
  )
}

function formatDateShort(iso: string): string {
  const d = new Date(iso)
  return d.toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}
