import { useState } from 'react'
import { Link, createFileRoute, useNavigate } from '@tanstack/react-router'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { api, formatDate } from '@/lib/api'
import { Badge, Button, Card, CardHeader, ErrorNote, Spinner, TypeBadge } from '@/components/ui'

export const Route = createFileRoute('/memories/$id')({ component: MemoryDetailPage })

function MemoryDetailPage() {
  const { id } = Route.useParams()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState('')

  const detail = useQuery({ queryKey: ['memory', id], queryFn: () => api.getMemory(id) })

  const patch = useMutation({
    mutationFn: (content: string) => api.patchMemory(id, content),
    onSuccess: () => {
      setEditing(false)
      queryClient.invalidateQueries({ queryKey: ['memory', id] })
      queryClient.invalidateQueries({ queryKey: ['memories'] })
    },
  })

  const del = useMutation({
    mutationFn: () => api.deleteMemory(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['memories'] })
      navigate({ to: '/memories' })
    },
  })

  if (detail.isLoading) return <Spinner />
  if (detail.error) return <ErrorNote error={detail.error} />
  const m = detail.data!

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <h1 className="text-2xl font-bold">Memory</h1>
          <TypeBadge type={m.memory_type} />
          <Badge className="bg-zinc-100 text-zinc-500">{m.source}</Badge>
        </div>
        <div className="flex gap-2">
          {!editing && (
            <Button
              variant="ghost"
              onClick={() => {
                setDraft(m.content)
                setEditing(true)
              }}
            >
              Edit
            </Button>
          )}
          <Button
            variant="danger"
            onClick={() => {
              if (confirm('Delete this memory permanently?')) del.mutate()
            }}
          >
            Delete
          </Button>
        </div>
      </div>

      <Card className="p-4">
        {editing ? (
          <div className="space-y-3">
            <textarea
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              rows={6}
              className="w-full rounded-lg border border-zinc-300 p-3 text-sm focus:border-zinc-500 focus:outline-none"
            />
            <div className="flex gap-2">
              <Button onClick={() => patch.mutate(draft)} disabled={patch.isPending || !draft.trim()}>
                {patch.isPending ? 'Saving…' : 'Save'}
              </Button>
              <Button variant="ghost" onClick={() => setEditing(false)}>
                Cancel
              </Button>
            </div>
            {patch.error && <ErrorNote error={patch.error} />}
          </div>
        ) : (
          <p className="whitespace-pre-wrap text-sm leading-relaxed">{m.content}</p>
        )}
      </Card>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader title="Details" />
          <dl className="space-y-2 p-4 text-sm">
            <Row k="id" v={<code className="text-xs">{m.id}</code>} />
            <Row k="importance" v={m.importance.toFixed(2)} />
            <Row k="created" v={formatDate(m.created_at)} />
            <Row k="updated" v={formatDate(m.updated_at)} />
            <Row k="accessed" v={`${formatDate(m.accessed_at)} (${m.access_count}×)`} />
            <Row k="project" v={m.project_id ? <code className="text-xs">{m.project_id}</code> : '—'} />
          </dl>
        </Card>

        <Card>
          <CardHeader title="Metadata" />
          <pre className="overflow-auto p-4 text-xs text-zinc-600">
            {m.metadata ? JSON.stringify(m.metadata, null, 2) : '—'}
          </pre>
        </Card>
      </div>

      <Card>
        <CardHeader
          title={`Edges (${m.edges.length})`}
          action={
            <Link
              to="/graph"
              search={{ focus: m.id }}
              className="text-xs font-medium text-zinc-500 hover:text-zinc-900"
            >
              explore in graph →
            </Link>
          }
        />
        {m.edges.length === 0 && (
          <p className="p-4 text-sm text-zinc-400">No edges yet (computed in the background).</p>
        )}
        <ul className="divide-y divide-zinc-100">
          {m.edges.map((e) => {
            const otherId = e.src_id === m.id ? e.dst_id : e.src_id
            return (
              <li key={e.id} className="flex items-center gap-3 px-4 py-2 text-sm">
                <Badge className="bg-zinc-100 text-zinc-600">{e.edge_type}</Badge>
                <span className="text-xs text-zinc-400">w={e.weight.toFixed(2)}</span>
                {e.label && <span className="text-xs italic text-zinc-500">{e.label}</span>}
                <Link
                  to="/memories/$id"
                  params={{ id: otherId }}
                  className="truncate font-mono text-xs text-blue-600 hover:underline"
                >
                  {otherId}
                </Link>
              </li>
            )
          })}
        </ul>
      </Card>
    </div>
  )
}

function Row({ k, v }: { k: string; v: React.ReactNode }) {
  return (
    <div className="flex gap-2">
      <dt className="w-24 shrink-0 text-zinc-400">{k}</dt>
      <dd className="min-w-0">{v}</dd>
    </div>
  )
}
