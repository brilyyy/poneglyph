import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Link, createFileRoute } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'
import {
  Background,
  Controls,
  MiniMap,
  ReactFlow,
  type Edge as FlowEdge,
  type Node as FlowNode,
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import {
  forceCenter,
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  type SimulationNodeDatum,
} from 'd3-force'

import { api, formatRelative, truncate } from '#/lib/api.ts'
import { getTheme } from '#/lib/theme.ts'
import {
  EDGE_TYPES,
  TYPE_COLORS,
  type Edge,
  type EdgeType,
  type Memory,
  type MemoryType,
} from '#/lib/types.ts'
import { TypeBadge } from '#/components/type-badge.tsx'
import { Alert, AlertDescription } from '#/components/ui/alert.tsx'
import { Button } from '#/components/ui/button.tsx'
import { Card, CardContent } from '#/components/ui/card.tsx'
import { Spinner } from '#/components/ui/spinner.tsx'

type GraphSearch = { focus?: string }

export const Route = createFileRoute('/graph')({
  validateSearch: (search: Record<string, unknown>): GraphSearch => ({
    focus: typeof search.focus === 'string' ? search.focus : undefined,
  }),
  component: GraphPage,
})

interface SimNode extends SimulationNodeDatum {
  id: string
}

/** Static d3-force layout: run the simulation synchronously, no animation.
 *  Prior positions seed the next run so expansion is incremental. */
function layout(
  memories: Memory[],
  edges: Edge[],
  prior: Map<string, { x: number; y: number }>,
): Map<string, { x: number; y: number }> {
  const simNodes: SimNode[] = memories.map((m) => {
    const p = prior.get(m.id)
    return { id: m.id, x: p?.x, y: p?.y }
  })
  const simLinks = edges.map((e) => ({ source: e.src_id, target: e.dst_id }))

  const sim = forceSimulation(simNodes)
    .force('link', forceLink(simLinks).id((d: any) => d.id).distance(120))
    .force('charge', forceManyBody().strength(-250))
    .force('center', forceCenter(0, 0))
    .force('collide', forceCollide(50))
    .stop()

  for (let i = 0; i < 300; i++) sim.tick()

  const out = new Map<string, { x: number; y: number }>()
  for (const n of simNodes) out.set(n.id, { x: n.x ?? 0, y: n.y ?? 0 })
  return out
}

function GraphPage() {
  const { focus } = Route.useSearch()
  const [memories, setMemories] = useState<Map<string, Memory>>(new Map())
  const [edges, setEdges] = useState<Map<string, Edge>>(new Map())
  const [hidden, setHidden] = useState<Set<EdgeType>>(new Set())
  const [selected, setSelected] = useState<Memory | null>(null)
  const positionsRef = useRef<Map<string, { x: number; y: number }>>(new Map())

  const initial = useQuery({
    queryKey: ['graph', focus ?? 'sample'],
    queryFn: () =>
      focus ? api.graph({ focus, depth: 1, limit: 500 }) : api.graph({ limit: 500 }),
  })

  // Reset and load when the initial query (or focus) changes.
  useEffect(() => {
    if (!initial.data) return
    positionsRef.current = new Map()
    setMemories(new Map(initial.data.nodes.map((n) => [n.id, n])))
    setEdges(new Map(initial.data.edges.map((e) => [e.id, e])))
    setSelected(null)
  }, [initial.data])

  const merge = useCallback((nodes: Memory[], newEdges: Edge[], around?: string) => {
    setMemories((prev) => {
      const next = new Map(prev)
      const origin = around ? positionsRef.current.get(around) : undefined
      for (const n of nodes) {
        if (!next.has(n.id)) {
          next.set(n.id, n)
          // Seed new nodes near the expansion origin so layout stays local.
          if (origin && !positionsRef.current.has(n.id)) {
            positionsRef.current.set(n.id, {
              x: origin.x + (Math.random() - 0.5) * 80,
              y: origin.y + (Math.random() - 0.5) * 80,
            })
          }
        }
      }
      return next
    })
    setEdges((prev) => {
      const next = new Map(prev)
      for (const e of newEdges) next.set(e.id, e)
      return next
    })
  }, [])

  const expand = useCallback(
    async (id: string) => {
      try {
        const resp = await api.graph({ focus: id, depth: 1, limit: 100 })
        merge(resp.nodes, resp.edges, id)
      } catch {
        // Expansion failure is non-fatal; node stays as-is.
      }
    },
    [merge],
  )

  const memoryArr = useMemo(() => [...memories.values()], [memories])
  const edgeArr = useMemo(() => [...edges.values()], [edges])

  const flow = useMemo(() => {
    const pos = layout(memoryArr, edgeArr, positionsRef.current)
    positionsRef.current = pos

    const flowNodes: FlowNode[] = memoryArr.map((m) => ({
      id: m.id,
      position: pos.get(m.id) ?? { x: 0, y: 0 },
      data: { label: truncate(m.content, 40) },
      style: {
        backgroundColor: `${TYPE_COLORS[m.memory_type] ?? '#94a3b8'}33`,
        borderColor: TYPE_COLORS[m.memory_type] ?? '#94a3b8',
        borderWidth: 2,
        borderRadius: 10,
        fontSize: 11,
        width: 150,
        padding: 6,
      },
    }))

    const flowEdges: FlowEdge[] = edgeArr.map((e) => ({
      id: e.id,
      source: e.src_id,
      target: e.dst_id,
      hidden: hidden.has(e.edge_type),
      label: e.edge_type === 'relation' ? (e.label ?? undefined) : undefined,
      style: { strokeWidth: Math.max(1, e.weight * 2) },
    }))

    return { nodes: flowNodes, edges: flowEdges }
  }, [memoryArr, edgeArr, hidden])

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

  const selectedEdgeCount = selected
    ? edgeArr.filter((e) => e.src_id === selected.id || e.dst_id === selected.id).length
    : 0

  return (
    <div className="flex h-[calc(100vh-3rem)] flex-col gap-3">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-bold">Graph explorer</h1>
        <span className="text-sm text-muted-foreground">
          {memoryArr.length} nodes · {edgeArr.length} edges · click a node to expand
        </span>
      </div>

      <div className="flex flex-wrap items-center gap-4 text-xs">
        <div className="flex items-center gap-2">
          {(Object.keys(TYPE_COLORS) as MemoryType[]).map((t) => (
            <span key={t} className="flex items-center gap-1 text-muted-foreground">
              <span
                className="h-2.5 w-2.5 rounded-full"
                style={{ backgroundColor: TYPE_COLORS[t] }}
              />
              {t}
            </span>
          ))}
        </div>
        <div className="flex items-center gap-2 border-l border-border pl-4">
          {EDGE_TYPES.map((t) => (
            <label
              key={t}
              className="flex cursor-pointer items-center gap-1 text-muted-foreground"
            >
              <input
                type="checkbox"
                className="accent-primary"
                checked={!hidden.has(t)}
                onChange={() =>
                  setHidden((prev) => {
                    const next = new Set(prev)
                    if (next.has(t)) next.delete(t)
                    else next.add(t)
                    return next
                  })
                }
              />
              {t}
            </label>
          ))}
        </div>
      </div>

      <div className="relative min-h-0 flex-1 overflow-hidden rounded-xl border border-border bg-card">
        <ReactFlow
          nodes={flow.nodes}
          edges={flow.edges}
          fitView
          minZoom={0.05}
          onlyRenderVisibleElements
          nodesConnectable={false}
          colorMode={getTheme()}
          onNodeClick={(_, node) => {
            setSelected(memories.get(node.id) ?? null)
            expand(node.id)
          }}
          onPaneClick={() => setSelected(null)}
        >
          <Background />
          <Controls />
          <MiniMap
            pannable
            zoomable
            nodeColor={(n) =>
              TYPE_COLORS[(memories.get(n.id)?.memory_type ?? 'code_context') as MemoryType]
            }
          />
        </ReactFlow>

        {selected && (
          <Card className="absolute right-3 top-3 w-72 shadow-lg">
            <CardContent className="space-y-2">
              <div className="flex items-center justify-between">
                <TypeBadge type={selected.memory_type} />
                <button
                  className="text-muted-foreground hover:text-foreground"
                  onClick={() => setSelected(null)}
                >
                  ✕
                </button>
              </div>
              <p className="max-h-40 overflow-auto text-sm">{selected.content}</p>
              <p className="text-xs text-muted-foreground">
                {formatRelative(selected.created_at)} · importance{' '}
                {selected.importance.toFixed(2)} · {selectedEdgeCount} edges
              </p>
              <div className="flex gap-2">
                <Button asChild size="sm" variant="outline">
                  <Link to="/memories/$id" params={{ id: selected.id }}>
                    open detail
                  </Link>
                </Button>
                <Button size="sm" variant="ghost" onClick={() => expand(selected.id)}>
                  expand neighbors
                </Button>
              </div>
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  )
}
