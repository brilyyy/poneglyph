import { useState } from 'react'
import { Link, createFileRoute, useNavigate } from '@tanstack/react-router'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'

import { api, formatDate, formatRelative } from '#/lib/api.ts'
import { TypeBadge } from '#/components/type-badge.tsx'
import { StrengthIndicator, TierBadge } from '#/components/timeline-indicators.tsx'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '#/components/ui/alert-dialog.tsx'
import { Alert, AlertDescription } from '#/components/ui/alert.tsx'
import { Badge } from '#/components/ui/badge.tsx'
import { Button } from '#/components/ui/button.tsx'
import {
  Card,
  CardAction,
  CardContent,
  CardHeader,
  CardTitle,
} from '#/components/ui/card.tsx'
import { Spinner } from '#/components/ui/spinner.tsx'
import { Textarea } from '#/components/ui/textarea.tsx'

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
      toast.success('Memory updated')
      queryClient.invalidateQueries({ queryKey: ['memory', id] })
      queryClient.invalidateQueries({ queryKey: ['memories'] })
    },
    onError: (e) => toast.error(`Update failed: ${e.message}`),
  })

  const del = useMutation({
    mutationFn: () => api.deleteMemory(id),
    onSuccess: () => {
      toast.success('Memory deleted')
      queryClient.invalidateQueries({ queryKey: ['memories'] })
      queryClient.invalidateQueries({ queryKey: ['stats'] })
      navigate({ to: '/memories' })
    },
    onError: (e) => toast.error(`Delete failed: ${e.message}`),
  })

  if (detail.isLoading)
    return (
      <div className="flex justify-center p-12">
        <Spinner />
      </div>
    )
  if (detail.error)
    return (
      <Alert variant="destructive">
        <AlertDescription>{String(detail.error)}</AlertDescription>
      </Alert>
    )
  const m = detail.data!

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <h1 className="text-2xl font-bold">Memory</h1>
          <TypeBadge type={m.memory_type} />
          <Badge variant="secondary">{m.source}</Badge>
        </div>
        <div className="flex gap-2">
          {!editing && (
            <Button
              variant="outline"
              onClick={() => {
                setDraft(m.content)
                setEditing(true)
              }}
            >
              Edit
            </Button>
          )}
          <AlertDialog>
            <AlertDialogTrigger asChild>
              <Button variant="destructive">Delete</Button>
            </AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Delete this memory?</AlertDialogTitle>
                <AlertDialogDescription>
                  Permanently removes the memory plus its embeddings, search index, and edges.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Cancel</AlertDialogCancel>
                <AlertDialogAction onClick={() => del.mutate()}>Delete</AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        </div>
      </div>

      <Card>
        <CardContent>
          {editing ? (
            <div className="space-y-3">
              <Textarea value={draft} onChange={(e) => setDraft(e.target.value)} rows={6} />
              <div className="flex gap-2">
                <Button
                  onClick={() => patch.mutate(draft)}
                  disabled={patch.isPending || !draft.trim()}
                >
                  {patch.isPending ? 'Saving…' : 'Save'}
                </Button>
                <Button variant="ghost" onClick={() => setEditing(false)}>
                  Cancel
                </Button>
              </div>
            </div>
          ) : (
            <p className="whitespace-pre-wrap text-sm leading-relaxed">{m.content}</p>
          )}
        </CardContent>
      </Card>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>Details</CardTitle>
          </CardHeader>
          <CardContent>
            <dl className="space-y-2 text-sm">
              <Row k="id" v={<code className="text-xs">{m.id}</code>} />
              <Row k="importance" v={m.importance.toFixed(2)} />
              <Row k="tier" v={<TierBadge tier={m.tier} />} />
              <Row k="strength" v={<StrengthIndicator strength={m.strength} />} />
              {typeof m.metadata?.confidence === 'number' && (
                <Row k="confidence" v={(m.metadata.confidence as number).toFixed(2)} />
              )}
              <Row k="created" v={formatDate(m.created_at)} />
              <Row k="updated" v={formatRelative(m.updated_at)} />
              <Row k="accessed" v={`${formatRelative(m.accessed_at)} (${m.access_count}×)`} />
              <Row
                k="project"
                v={m.project_id ? <code className="text-xs">{m.project_id}</code> : '—'}
              />
            </dl>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Metadata</CardTitle>
          </CardHeader>
          <CardContent>
            <pre className="overflow-auto text-xs text-muted-foreground">
              {m.metadata ? JSON.stringify(m.metadata, null, 2) : '—'}
            </pre>
          </CardContent>
        </Card>
      </div>

      {(m.children.length > 0 || m.parent) && (
        <Card className="gap-0 pb-0">
          <CardHeader className="pb-4">
            <CardTitle>
              {m.is_decoy ? `Consolidated from (${m.children.length})` : 'Consolidated into'}
            </CardTitle>
          </CardHeader>
          <ul className="divide-y divide-border">
            {(m.is_decoy ? m.children : m.parent ? [m.parent] : []).map((other) => (
              <li key={other.id} className="flex items-center gap-3 px-6 py-2 text-sm">
                <TypeBadge type={other.memory_type} />
                <Link
                  to="/memories/$id"
                  params={{ id: other.id }}
                  className="truncate text-xs text-primary hover:underline"
                >
                  {other.content}
                </Link>
              </li>
            ))}
          </ul>
        </Card>
      )}

      <Card className="gap-0 pb-0">
        <CardHeader className="pb-4">
          <CardTitle>Edges ({m.edges.length})</CardTitle>
          <CardAction>
            <Link
              to="/graph"
              search={{ focus: m.id }}
              className="text-xs font-medium text-muted-foreground hover:text-foreground"
            >
              explore in graph →
            </Link>
          </CardAction>
        </CardHeader>
        {m.edges.length === 0 && (
          <p className="px-6 pb-6 text-sm text-muted-foreground">
            No edges yet (computed in the background).
          </p>
        )}
        <ul className="divide-y divide-border">
          {m.edges.map((e) => {
            const otherId = e.src_id === m.id ? e.dst_id : e.src_id
            return (
              <li key={e.id} className="flex items-center gap-3 px-6 py-2 text-sm">
                <Badge variant="secondary">{e.edge_type}</Badge>
                <span className="text-xs text-muted-foreground">w={e.weight.toFixed(2)}</span>
                {e.label && <span className="text-xs italic text-muted-foreground">{e.label}</span>}
                <Link
                  to="/memories/$id"
                  params={{ id: otherId }}
                  className="truncate font-mono text-xs text-primary hover:underline"
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
      <dt className="w-24 shrink-0 text-muted-foreground">{k}</dt>
      <dd className="min-w-0">{v}</dd>
    </div>
  )
}
