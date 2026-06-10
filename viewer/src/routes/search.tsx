import { useState } from 'react'
import { Link, createFileRoute } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'

import { api, formatDate, truncate } from '@/lib/api'
import { MEMORY_TYPES } from '@/lib/types'
import { Badge, Card, EmptyNote, ErrorNote, Input, Select, Spinner, TypeBadge } from '@/components/ui'

export const Route = createFileRoute('/search')({ component: SearchPage })

function SearchPage() {
  const [input, setInput] = useState('')
  const [query, setQuery] = useState('')
  const [type, setType] = useState('')

  const results = useQuery({
    queryKey: ['search', query, type],
    queryFn: () => api.search({ q: query, limit: 25, type: type || undefined }),
    enabled: query.trim().length > 0,
  })

  return (
    <div className="space-y-4">
      <h1 className="text-2xl font-bold">Search</h1>

      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault()
          setQuery(input)
        }}
      >
        <Input
          autoFocus
          placeholder="hybrid search (semantic + keyword + graph)…"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          className="flex-1"
        />
        <Select value={type} onChange={(e) => setType(e.target.value)}>
          <option value="">all types</option>
          {MEMORY_TYPES.map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </Select>
      </form>

      {results.isFetching && <Spinner />}
      {results.error && <ErrorNote error={results.error} />}

      {results.data && (
        <Card>
          {results.data.results.length === 0 && <EmptyNote>No results for “{query}”.</EmptyNote>}
          <ul className="divide-y divide-zinc-100">
            {results.data.results.map((hit) => (
              <li key={hit.id}>
                <Link
                  to="/memories/$id"
                  params={{ id: hit.id }}
                  className="flex items-center gap-3 px-4 py-3 hover:bg-zinc-50"
                >
                  <Badge className="bg-amber-50 text-amber-700">{hit.score.toFixed(3)}</Badge>
                  <TypeBadge type={hit.memory_type} />
                  <span className="min-w-0 flex-1 truncate text-sm">{truncate(hit.content, 110)}</span>
                  <span className="shrink-0 text-xs text-zinc-400">{formatDate(hit.created_at)}</span>
                </Link>
              </li>
            ))}
          </ul>
        </Card>
      )}
    </div>
  )
}
