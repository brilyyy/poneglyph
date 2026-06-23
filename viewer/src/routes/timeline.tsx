import { useState } from 'react'
import { createFileRoute } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'

import { api, formatRelative } from '#/lib/api.ts'
import type { Memory, TimelineSession } from '#/lib/types.ts'
import { MEMORY_TYPES } from '#/lib/types.ts'
import { TypeBadge } from '#/components/type-badge.tsx'
import {
  StrengthIndicator,
  TierBadge,
  SourceBadge,
  EventBadge,
  formatDuration,
  TypeCounts,
} from '#/components/timeline-indicators.tsx'
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
  type?: string
  source?: string
}

export const Route = createFileRoute('/timeline')({
  validateSearch: (search: Record<string, unknown>): TimelineSearch => ({
    project: typeof search.project === 'string' ? search.project : undefined,
    type: typeof search.type === 'string' ? search.type : undefined,
    source: typeof search.source === 'string' ? search.source : undefined,
  }),
  component: TimelinePage,
})

function TimelinePage() {
  const search = Route.useSearch()
  const navigate = Route.useNavigate()

  const projects = useQuery({ queryKey: ['projects'], queryFn: api.projects })
  const [offset, setOffset] = useState(0)

  const timeline = useQuery({
    queryKey: ['timeline', search.project, search.type, search.source, offset],
    queryFn: () =>
      api.timeline({
        project_path: search.project,
        memory_type: search.type,
        source: search.source,
        limit: PAGE_SIZE,
        offset,
      }),
  })

  const summary = useQuery({
    queryKey: ['session-summary', search.project],
    queryFn: () => api.sessionSummary({ project_path: search.project }),
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

      {summary.data && (
        <div className="rounded-md border border-border/50 bg-card/50 p-3">
          <p className="mb-1 text-xs font-medium text-muted-foreground">Last session summary</p>
          <p className="text-sm whitespace-pre-wrap">{summary.data.content}</p>
          <p className="mt-1 text-[10px] text-muted-foreground">{formatRelative(summary.data.created_at)}</p>
        </div>
      )}

      <div className="flex gap-2">
        <Select
          value={search.project ?? ALL}
          onValueChange={(v) =>
            navigate({ search: { ...search, project: v === ALL ? undefined : v } })
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

        <Select
          value={search.type ?? ALL}
          onValueChange={(v) =>
            navigate({ search: { ...search, type: v === ALL ? undefined : v } })
          }
        >
          <SelectTrigger className="w-36">
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
          value={search.source ?? ALL}
          onValueChange={(v) =>
            navigate({ search: { ...search, source: v === ALL ? undefined : v } })
          }
        >
          <SelectTrigger className="w-36">
            <SelectValue placeholder="all sources" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL}>all sources</SelectItem>
            {['claude-code', 'opencode', 'cli', 'import'].map((s) => (
              <SelectItem key={s} value={s}>
                {s}
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
                  <SessionCard session={s} />
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

// ---------------------------------------------------------------------------
// Session Card — compact view with stats header + grouped memories
// ---------------------------------------------------------------------------

function SessionCard({ session }: { session: TimelineSession }) {
  const hasSummary = session.memories.some((m) =>
    ((m.metadata as { tags?: string[] } | null)?.tags ?? []).includes('session-summary'),
  )
  return (
    <div className="space-y-2 rounded-md border border-border/50 bg-card/50 p-3">
      {/* Session Stats Header */}
      <div className="flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground">
        <span>{formatDuration(session.duration_secs)}</span>
        <span className="opacity-50">·</span>
        <span>{session.memory_count} memories</span>
        <span className="opacity-50">·</span>
        <span>Avg strength: {(session.avg_strength * 100).toFixed(0)}%</span>
        <span className="opacity-50">·</span>
        <TypeCounts counts={session.type_counts} />
        <span className="opacity-50">·</span>
        <SourceCounts counts={session.source_counts} />
        {hasSummary && (
          <>
            <span className="opacity-50">·</span>
            <span className="text-emerald-500">summarized</span>
          </>
        )}
      </div>

      {/* Grouped Memories */}
      <div className="space-y-1">
        {groupMemories(session.memories).map((item) => {
          if ('type' in item && item.type === 'qa') {
            return <QAPair key={item.input.id} input={item.input} output={item.output} />
          }
          const m = item as Memory
          return <MemoryRow key={m.id} memory={m} />
        })}
        {session.memories.length > 10 && (
          <p className="text-xs text-muted-foreground">
            +{session.memories.length - 10} more memories
          </p>
        )}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Source Counts — compact display of sources
// ---------------------------------------------------------------------------

function SourceCounts({ counts }: { counts: Partial<Record<string, number>> }) {
  return (
    <span className="inline-flex items-center gap-1 text-[10px] text-muted-foreground">
      {Object.entries(counts).map(([src, count]) => (
        <span key={src} className="inline-flex items-center gap-0.5">
          <SourceBadge source={src as any} />
          {count}
        </span>
      ))}
    </span>
  )
}

// ---------------------------------------------------------------------------
// Memory Row — single memory in the timeline
// ---------------------------------------------------------------------------

function MemoryRow({ memory }: { memory: Memory }) {
  const [expanded, setExpanded] = useState(false)
  const event = (memory.metadata as any)?.event as string | undefined
  const tool = (memory.metadata as any)?.tool as string | undefined

  return (
    <div className="flex items-start gap-2 rounded px-2 py-1 hover:bg-accent/30">
      <div className="flex shrink-0 items-center gap-1 pt-0.5">
        <TypeBadge type={memory.memory_type} />
        <StrengthIndicator strength={memory.strength} />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1">
          {event && <EventBadge event={event} />}
          {tool && (
            <span className="text-[10px] text-muted-foreground">({tool})</span>
          )}
          <TierBadge tier={memory.tier} />
        </div>
        <p className={`text-xs leading-relaxed ${expanded ? '' : 'line-clamp-2'}`}>
          {expanded ? memory.content : smartTruncate(memory.content)}
        </p>
        {memory.content.length > 200 && (
          <button
            onClick={() => setExpanded(!expanded)}
            className="text-[10px] text-primary hover:underline"
          >
            {expanded ? 'collapse' : 'expand'}
          </button>
        )}
      </div>
      <div className="flex shrink-0 items-center gap-1 pt-0.5">
        <SourceBadge source={memory.source} />
        <span className="text-[10px] text-muted-foreground">
          {formatRelative(memory.created_at)}
        </span>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// QAPair — collapsible user prompt + assistant response
// ---------------------------------------------------------------------------

function QAPair({ input, output }: { input: Memory; output: Memory }) {
  const [expanded, setExpanded] = useState(false)
  const inputTool = (input.metadata as any)?.tool as string | undefined

  return (
    <div className="rounded border border-border/50 bg-card/80">
      {/* Input (user prompt) — blue left border */}
      <div className="border-l-2 border-blue-400 px-3 py-1.5">
        <div className="flex items-center gap-1">
          <span className="text-[10px] font-semibold text-blue-500">Q</span>
          <SourceBadge source={input.source} />
          <StrengthIndicator strength={input.strength} />
          {inputTool && (
            <span className="text-[10px] text-muted-foreground">({inputTool})</span>
          )}
          <span className="ml-auto text-[10px] text-muted-foreground">
            {formatRelative(input.created_at)}
          </span>
        </div>
        <p className={`text-xs leading-relaxed ${expanded ? '' : 'line-clamp-3'}`}>
          {expanded ? input.content : smartTruncate(input.content)}
        </p>
      </div>

      {/* Output (assistant response) — green left border */}
      <div className="border-l-2 border-green-400 px-3 py-1.5">
        <div className="flex items-center gap-1">
          <span className="text-[10px] font-semibold text-green-500">A</span>
          <SourceBadge source={output.source} />
          <StrengthIndicator strength={output.strength} />
          <TierBadge tier={output.tier} />
          <span className="ml-auto text-[10px] text-muted-foreground">
            {formatRelative(output.created_at)}
          </span>
        </div>
        <p className={`text-xs leading-relaxed ${expanded ? '' : 'line-clamp-3'}`}>
          {expanded ? output.content : smartTruncate(output.content)}
        </p>
      </div>

      {/* Expand/collapse button */}
      {(input.content.length > 300 || output.content.length > 300) && (
        <button
          onClick={() => setExpanded(!expanded)}
          className="w-full border-t border-border/30 px-3 py-1 text-[10px] text-primary hover:bg-accent/30"
        >
          {expanded ? 'collapse all' : 'expand all'}
        </button>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Memory grouping — pair consecutive user_message + assistant_message as Q&A
// ---------------------------------------------------------------------------

type MemoryItem =
  | Memory
  | { type: 'qa'; input: Memory; output: Memory }

function groupMemories(memories: Memory[]): MemoryItem[] {
  const result: MemoryItem[] = []
  let i = 0

  while (i < memories.length) {
    const curr = memories[i]
    const next = memories[i + 1]

    const currEvent = (curr.metadata as any)?.event as string | undefined
    const nextEvent = (next?.metadata as any)?.event as string | undefined

    // Pair consecutive user_message + assistant_message as Q&A
    if (currEvent === 'user_message' && nextEvent === 'assistant_message') {
      result.push({ type: 'qa', input: curr, output: next })
      i += 2
    } else {
      result.push(curr)
      i += 1
    }
  }

  return result
}

// ---------------------------------------------------------------------------
// Smart truncation — first 3 lines + ellipsis
// ---------------------------------------------------------------------------

function smartTruncate(content: string): string {
  const lines = content.split('\n')
  if (lines.length <= 3) {
    return content.length > 200 ? content.slice(0, 200) + '…' : content
  }
  return lines.slice(0, 3).join('\n') + '…'
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function formatDateShort(iso: string): string {
  const d = new Date(iso)
  return d.toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}
