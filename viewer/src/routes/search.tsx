import { useEffect, useState } from 'react'
import { Link, createFileRoute } from '@tanstack/react-router'
import { keepPreviousData, useQuery } from '@tanstack/react-query'

import { api, formatRelative, truncate } from '#/lib/api.ts'
import { MEMORY_TYPES } from '#/lib/types.ts'
import { TypeBadge } from '#/components/type-badge.tsx'
import { Alert, AlertDescription } from '#/components/ui/alert.tsx'
import { Badge } from '#/components/ui/badge.tsx'
import { Card } from '#/components/ui/card.tsx'
import { Empty, EmptyDescription, EmptyTitle } from '#/components/ui/empty.tsx'
import { Input } from '#/components/ui/input.tsx'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '#/components/ui/select.tsx'
import { Spinner } from '#/components/ui/spinner.tsx'

const ALL = 'all'
const DEBOUNCE_MS = 300

export const Route = createFileRoute('/search')({ component: SearchPage })

function SearchPage() {
  const [input, setInput] = useState('')
  const [query, setQuery] = useState('')
  const [type, setType] = useState<string>(ALL)

  // Search-as-you-type, debounced.
  useEffect(() => {
    const t = setTimeout(() => setQuery(input.trim()), DEBOUNCE_MS)
    return () => clearTimeout(t)
  }, [input])

  const results = useQuery({
    queryKey: ['search', query, type],
    queryFn: () =>
      api.search({ q: query, limit: 25, type: type === ALL ? undefined : type }),
    enabled: query.length > 0,
    placeholderData: keepPreviousData,
  })

  return (
    <div className="space-y-4">
      <h1 className="text-2xl font-bold">Search</h1>

      <div className="flex gap-2">
        <Input
          autoFocus
          placeholder="hybrid search (semantic + keyword + graph)…"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          className="flex-1"
        />
        <Select value={type} onValueChange={setType}>
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
      </div>

      {results.error && (
        <Alert variant="destructive">
          <AlertDescription>{String(results.error)}</AlertDescription>
        </Alert>
      )}

      {results.isLoading && query.length > 0 && (
        <div className="flex justify-center p-8">
          <Spinner />
        </div>
      )}

      {results.data && (
        <Card
          className="gap-0 py-0 transition-opacity"
          style={{ opacity: results.isFetching ? 0.6 : 1 }}
        >
          {results.data.results.length === 0 ? (
            <Empty>
              <EmptyTitle>No results</EmptyTitle>
              <EmptyDescription>Nothing matched “{query}”.</EmptyDescription>
            </Empty>
          ) : (
            <ul className="divide-y divide-border">
              {results.data.results.map((hit) => (
                <li key={hit.id}>
                  <Link
                    to="/memories/$id"
                    params={{ id: hit.id }}
                    className="flex items-center gap-3 px-4 py-3 hover:bg-muted/50"
                  >
                    <Badge variant="outline" className="tabular-nums">
                      {hit.score.toFixed(3)}
                    </Badge>
                    <TypeBadge type={hit.memory_type} />
                    <span className="min-w-0 flex-1 truncate text-sm">
                      {truncate(hit.content, 110)}
                    </span>
                    <span className="shrink-0 text-xs text-muted-foreground">
                      {formatRelative(hit.created_at)}
                    </span>
                  </Link>
                </li>
              ))}
            </ul>
          )}
        </Card>
      )}
    </div>
  )
}
