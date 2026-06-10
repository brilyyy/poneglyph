import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { createFileRoute } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'
import {
  Background,
  Controls,
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

import { api, truncate } from '@/lib/api'
import {
  EDGE_TYPES,
  TYPE_COLORS,
  type Edge,
  type EdgeType,
  type Memory,
  type MemoryType,
} from '@/lib/types'
import { ErrorNote, Spinner } from '@/components/ui'

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
    return { id: m.id, x: p?.x, y: p?.y, fx: undefined, fy: undefined }
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
      style: { strokeWidth: Math.max(1, e.weight * 2), stroke: '#cbd5e1' },
    }))

    return { nodes: flowNodes, edges: flowEdges }
  }, [memoryArr, edgeArr, hidden])

  if (initial.isLoading) return <Spinner />
  if (initial.error) return <ErrorNote error={initial.error} />

  return (
    <div className="flex h-[calc(100vh-3rem)] flex-col gap-3">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-bold">Graph explorer</h1>
        <span className="text-sm text-zinc-400">
          {memoryArr.length} nodes · {edgeArr.length} edges · click a node to expand
        </span>
      </div>

      <div className="flex flex-wrap items-center gap-4 text-xs">
        <div className="flex items-center gap-2">
          {(Object.keys(TYPE_COLORS) as MemoryType[]).map((t) => (
            <span key={t} className="flex items-center gap-1 text-zinc-500">
              <span className="h-2.5 w-2.5 rounded-full" style={{ backgroundColor: TYPE_COLORS[t] }} />
              {t}
            </span>
          ))}
        </div>
        <div className="flex items-center gap-2 border-l border-zinc-200 pl-4">
          {EDGE_TYPES.map((t) => (
            <label key={t} className="flex cursor-pointer items-center gap-1 text-zinc-500">
              <input
                type="checkbox"
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

      <div className="relative min-h-0 flex-1 overflow-hidden rounded-xl border border-zinc-200 bg-white">
        <ReactFlow
          nodes={flow.nodes}
          edges={flow.edges}
          fitView
          minZoom={0.05}
          onlyRenderVisibleElements
          nodesConnectable={false}
          onNodeClick={(_, node) => {
            setSelected(memories.get(node.id) ?? null)
            expand(node.id)
          }}
        >
          <Background />
          <Controls />
        </ReactFlow>

        {selected && (
          <div className="absolute right-3 top-3 w-72 rounded-xl border border-zinc-200 bg-white p-4 shadow-lg">
            <div className="mb-2 flex items-center justify-between">
              <span
                className="rounded-full px-2 py-0.5 text-xs font-medium"
                style={{
                  backgroundColor: `${TYPE_COLORS[selected.memory_type]}22`,
                  color: TYPE_COLORS[selected.memory_type],
                }}
              >
                {selected.memory_type}
              </span>
              <button
                className="text-zinc-400 hover:text-zinc-700"
                onClick={() => setSelected(null)}
              >
                ✕
              </button>
            </div>
            <p className="max-h-40 overflow-auto text-sm">{selected.content}</p>
            <a
              href={`/memories/${selected.id}`}
              className="mt-2 inline-block text-xs font-medium text-blue-600 hover:underline"
            >
              open detail →
            </a>
          </div>
        )}
      </div>
    </div>
  )
}
