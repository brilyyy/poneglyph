import { useCallback, useEffect, useMemo, useState } from 'react'
import { createFileRoute } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'

import { api } from '#/lib/api.ts'
import { CG_NODE_COLORS, type CgEdge, type CgEdgeKind, type CgNode, type CgNodeKind } from '#/lib/types.ts'
import { Alert, AlertDescription } from '#/components/ui/alert.tsx'
import { Card, CardContent } from '#/components/ui/card.tsx'
import { Empty, EmptyDescription, EmptyTitle } from '#/components/ui/empty.tsx'
import { Spinner } from '#/components/ui/spinner.tsx'
import { CosmosGraph, type CosmosLink, type CosmosNode } from '#/components/cosmos-graph.tsx'

type CodegraphSearch = { focus?: string }

export const Route = createFileRoute('/codegraph')({
  validateSearch: (search: Record<string, unknown>): CodegraphSearch => ({
    focus: typeof search.focus === 'string' ? search.focus : undefined,
  }),
  component: CodegraphPage,
})

const KIND_LABEL: Record<CgNodeKind, string> = {
  function: 'fn',
  method: 'method',
  type: 'type',
  import: 'import',
  test: 'test',
}

function edgeId(e: CgEdge): string {
  return `${e.src_id}->${e.dst_id}:${e.kind}`
}

function CodegraphPage() {
  const { focus } = Route.useSearch()
  const [limit, setLimit] = useState(500)
  const [nodes, setNodesMap] = useState<Map<string, CgNode>>(new Map())
  const [edges, setEdgesMap] = useState<Map<string, CgEdge>>(new Map())
  const [hidden, setHidden] = useState<Set<CgEdgeKind>>(new Set())
  const [selected, setSelected] = useState<CgNode | null>(null)

  const initial = useQuery({
    queryKey: ['codegraph', focus ?? 'all', limit],
    queryFn: () => (focus ? api.codegraph({ focus, depth: 2 }) : api.codegraph({ limit })),
  })

  useEffect(() => {
    if (!initial.data) return
    setNodesMap(new Map(initial.data.nodes.map((n) => [n.id, n])))
    setEdgesMap(new Map(initial.data.edges.map((e) => [edgeId(e), e])))
    setSelected(null)
  }, [initial.data])

  const merge = useCallback((newNodes: CgNode[], newEdges: CgEdge[]) => {
    setNodesMap((prev) => {
      const next = new Map(prev)
      for (const n of newNodes) if (!next.has(n.id)) next.set(n.id, n)
      return next
    })
    setEdgesMap((prev) => {
      const next = new Map(prev)
      for (const e of newEdges) next.set(edgeId(e), e)
      return next
    })
  }, [])

  const expand = useCallback(
    async (node: CgNode) => {
      try {
        const resp = await api.codegraph({ focus: node.name, depth: 1 })
        merge(resp.nodes, resp.edges)
      } catch {
        // expansion failure is non-fatal
      }
    },
    [merge],
  )

  const nodeArr = useMemo(() => [...nodes.values()], [nodes])
  const edgeArr = useMemo(() => [...edges.values()], [edges])

  // Fan-out (call/import/test degree) isn't shown anywhere in the old DOM
  // cards — size by it here so hot-spot symbols stand out at a glance.
  const degree = useMemo(() => {
    const d = new Map<string, number>()
    for (const e of edgeArr) {
      d.set(e.src_id, (d.get(e.src_id) ?? 0) + 1)
      d.set(e.dst_id, (d.get(e.dst_id) ?? 0) + 1)
    }
    return d
  }, [edgeArr])

  const cosmosNodes: CosmosNode[] = useMemo(
    () =>
      nodeArr.map((n) => ({
        id: n.id,
        color: CG_NODE_COLORS[n.kind],
        size: 3 + Math.min(degree.get(n.id) ?? 0, 20) * 0.6,
      })),
    [nodeArr, degree],
  )

  const cosmosLinks: CosmosLink[] = useMemo(
    () => edgeArr.filter((e) => !hidden.has(e.kind)).map((e) => ({ source: e.src_id, target: e.dst_id })),
    [edgeArr, hidden],
  )

  if (initial.isLoading)
    return (
      <div className="flex justify-center p-12">
        <Spinner />
      </div>
    )
  if (initial.error)
    return (
      <Alert variant="destructive">
        <AlertDescription>{String(initial.error)}</AlertDescription>
      </Alert>
    )
  if (nodeArr.length === 0)
    return (
      <Empty>
        <EmptyTitle>No code graph yet</EmptyTitle>
        <EmptyDescription>
          Run <code>poneglyph graph init &lt;path&gt;</code> to parse a project, then refresh.
        </EmptyDescription>
      </Empty>
    )

  const totalNodes = initial.data?.total_nodes ?? nodeArr.length
  const totalEdges = initial.data?.total_edges ?? edgeArr.length
  const isSampled = totalNodes > nodeArr.length

  return (
    <div className="flex h-[calc(100vh-3rem)] flex-col gap-3">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-bold">Code graph</h1>
        <span className="text-sm text-muted-foreground">
          showing {nodeArr.length} of {totalNodes} nodes · {edgeArr.length} of {totalEdges} edges
          {isSampled ? ' (sampled)' : ''} · click a node to expand
        </span>
      </div>

      <div className="flex flex-wrap items-center gap-4 text-xs">
        <div className="flex items-center gap-2">
          {(Object.keys(CG_NODE_COLORS) as CgNodeKind[]).map((k) => (
            <span key={k} className="flex items-center gap-1 text-muted-foreground">
              <span className="h-2.5 w-2.5 rounded-full" style={{ backgroundColor: CG_NODE_COLORS[k] }} />
              {k}
            </span>
          ))}
        </div>
        <div className="flex items-center gap-2 border-l border-border pl-4">
          {(['calls', 'imports', 'tests'] as CgEdgeKind[]).map((k) => (
            <label key={k} className="flex cursor-pointer items-center gap-1 text-muted-foreground">
              <input
                type="checkbox"
                className="accent-primary"
                checked={!hidden.has(k)}
                onChange={() =>
                  setHidden((prev) => {
                    const next = new Set(prev)
                    if (next.has(k)) next.delete(k)
                    else next.add(k)
                    return next
                  })
                }
              />
              {k}
            </label>
          ))}
        </div>
        <label className="flex items-center gap-2 border-l border-border pl-4 text-muted-foreground">
          render limit: {limit}
          <input
            type="range"
            min={100}
            max={Math.max(2000, totalNodes)}
            step={100}
            value={limit}
            onChange={(e) => setLimit(Number(e.target.value))}
            className="w-40 accent-primary"
          />
        </label>
      </div>

      <div className="relative min-h-0 flex-1 overflow-hidden rounded-xl border border-border bg-card">
        <CosmosGraph
          nodes={cosmosNodes}
          links={cosmosLinks}
          onNodeClick={(id) => {
            const node = nodes.get(id) ?? null
            setSelected(node)
            if (node) void expand(node)
          }}
          onBackgroundClick={() => setSelected(null)}
        />

        {selected && (
          <Card className="absolute right-3 top-3 w-80 shadow-lg">
            <CardContent className="space-y-2">
              <div className="flex items-center justify-between">
                <span className="text-sm font-semibold">{selected.name}</span>
                <button className="text-muted-foreground hover:text-foreground" onClick={() => setSelected(null)}>
                  ✕
                </button>
              </div>
              <p className="text-xs text-muted-foreground">
                {KIND_LABEL[selected.kind]} · {selected.file_path}:{selected.start_line}
                {selected.end_line !== selected.start_line ? `-${selected.end_line}` : ''}
                {' · '}
                {degree.get(selected.id) ?? 0} connections
              </p>
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  )
}
