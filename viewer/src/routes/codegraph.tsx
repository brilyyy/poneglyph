import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { createFileRoute } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'
import {
  applyNodeChanges,
  Background,
  Controls,
  MiniMap,
  ReactFlow,
  useNodesState,
  type Edge as FlowEdge,
  type Node as FlowNode,
  type OnNodesChange,
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import {
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  forceX,
  forceY,
  type Simulation,
  type SimulationNodeDatum,
} from 'd3-force'

import { api } from '#/lib/api.ts'
import { getTheme } from '#/lib/theme.ts'
import { CG_NODE_COLORS, type CgEdge, type CgEdgeKind, type CgNode, type CgNodeKind } from '#/lib/types.ts'
import { Alert, AlertDescription } from '#/components/ui/alert.tsx'
import { Card, CardContent } from '#/components/ui/card.tsx'
import { Empty, EmptyDescription, EmptyTitle } from '#/components/ui/empty.tsx'
import { Spinner } from '#/components/ui/spinner.tsx'
import FloatingEdge from '#/components/floating-edge.tsx'

type CodegraphSearch = { focus?: string }

export const Route = createFileRoute('/codegraph')({
  validateSearch: (search: Record<string, unknown>): CodegraphSearch => ({
    focus: typeof search.focus === 'string' ? search.focus : undefined,
  }),
  component: CodegraphPage,
})

interface SimNode extends SimulationNodeDatum {
  id: string
}

const edgeTypes = { floating: FloatingEdge }
const NODE_W = 170
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
  const [nodes, setNodesMap] = useState<Map<string, CgNode>>(new Map())
  const [edges, setEdgesMap] = useState<Map<string, CgEdge>>(new Map())
  const [hidden, setHidden] = useState<Set<CgEdgeKind>>(new Set())
  const [selected, setSelected] = useState<CgNode | null>(null)

  const simNodesRef = useRef<Map<string, SimNode>>(new Map())
  const simRef = useRef<Simulation<SimNode, CgEdge> | null>(null)
  const rafRef = useRef<number>(0)

  const initial = useQuery({
    queryKey: ['codegraph', focus ?? 'all'],
    queryFn: () => (focus ? api.codegraph({ focus, depth: 2 }) : api.codegraph({})),
  })

  useEffect(() => {
    if (!initial.data) return
    simNodesRef.current = new Map()
    setNodesMap(new Map(initial.data.nodes.map((n) => [n.id, n])))
    setEdgesMap(new Map(initial.data.edges.map((e) => [edgeId(e), e])))
    setSelected(null)
  }, [initial.data])

  useEffect(() => {
    const nodeArr = [...nodes.values()]
    const edgeArr = [...edges.values()]

    for (const n of nodeArr) {
      if (!simNodesRef.current.has(n.id)) {
        const existing = simNodesRef.current.size
        simNodesRef.current.set(n.id, {
          id: n.id,
          x: existing > 0 ? (Math.random() - 0.5) * 80 : 0,
          y: existing > 0 ? (Math.random() - 0.5) * 80 : 0,
        })
      }
    }

    const simNodes: SimNode[] = nodeArr.map((n) => simNodesRef.current.get(n.id)!).filter(Boolean)
    const simLinks = edgeArr.map((e) => ({ source: e.src_id, target: e.dst_id }))

    if (!simRef.current) {
      simRef.current = forceSimulation<SimNode>(simNodes)
        .force(
          'link',
          forceLink<SimNode, { source: string; target: string }>(simLinks).id((d) => d.id).distance(160).strength(0.3),
        )
        .force('charge', forceManyBody<SimNode>().strength(-350).distanceMax(600))
        .force('x', forceX<SimNode>(0).strength(0.05))
        .force('y', forceY<SimNode>(0).strength(0.05))
        .force('collide', forceCollide<SimNode>(90).strength(0.9).iterations(2))
        .alphaDecay(0.03)
        .velocityDecay(0.4)
    } else {
      simRef.current.nodes(simNodes)
      const linkForce = simRef.current.force('link') as ReturnType<typeof forceLink>
      if (linkForce) {
        linkForce.links(simLinks)
        linkForce.id((d: any) => d.id)
      }
      simRef.current.alpha(0.4).restart()
    }

    simRef.current.on('tick', () => {
      if (rafRef.current) return
      rafRef.current = requestAnimationFrame(() => {
        rafRef.current = 0
        setFlowNodes((prev) =>
          prev.map((n) => {
            const sn = simNodesRef.current.get(n.id)
            return sn ? { ...n, position: { x: sn.x ?? 0, y: sn.y ?? 0 } } : n
          }),
        )
      })
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [nodes, edges])

  useEffect(() => {
    return () => {
      simRef.current?.stop()
      simRef.current = null
      if (rafRef.current) cancelAnimationFrame(rafRef.current)
    }
  }, [])

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

  const baseFlowNodes: FlowNode[] = useMemo(
    () =>
      nodeArr.map((n) => {
        const sn = simNodesRef.current.get(n.id)
        return {
          id: n.id,
          position: { x: sn?.x ?? 0, y: sn?.y ?? 0 },
          data: { label: `${KIND_LABEL[n.kind]} ${n.name}` },
          style: {
            backgroundColor: `${CG_NODE_COLORS[n.kind]}33`,
            borderColor: CG_NODE_COLORS[n.kind],
            borderWidth: 2,
            borderRadius: 10,
            fontSize: 11,
            width: NODE_W,
            padding: 6,
          },
        }
      }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [nodeArr.length],
  )

  const [flowNodes, setFlowNodes] = useNodesState(baseFlowNodes)

  useEffect(() => {
    setFlowNodes((prev) => {
      const prevIds = new Set(prev.map((n) => n.id))
      const nextIds = new Set(baseFlowNodes.map((n) => n.id))
      if (prevIds.size === nextIds.size && [...prevIds].every((id) => nextIds.has(id))) return prev
      return baseFlowNodes
    })
  }, [baseFlowNodes, setFlowNodes])

  const flowEdges: FlowEdge[] = useMemo(
    () =>
      edgeArr.map((e) => ({
        id: edgeId(e),
        source: e.src_id,
        target: e.dst_id,
        type: 'floating' as const,
        hidden: hidden.has(e.kind),
        label: e.kind,
      })),
    [edgeArr, hidden],
  )

  const onNodesChange: OnNodesChange = useCallback(
    (changes) => setFlowNodes((nds) => applyNodeChanges(changes, nds)),
    [setFlowNodes],
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

  return (
    <div className="flex h-[calc(100vh-3rem)] flex-col gap-3">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-bold">Code graph</h1>
        <span className="text-sm text-muted-foreground">
          {nodeArr.length} nodes · {edgeArr.length} edges · click a node to expand
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
      </div>

      <div className="relative min-h-0 flex-1 overflow-hidden rounded-xl border border-border bg-card">
        <ReactFlow
          nodes={flowNodes}
          edges={flowEdges}
          edgeTypes={edgeTypes}
          fitView
          minZoom={0.05}
          onlyRenderVisibleElements
          nodesConnectable={false}
          colorMode={getTheme()}
          onNodeClick={(_, n) => {
            const node = nodes.get(n.id) ?? null
            setSelected(node)
            if (node) expand(node)
          }}
          onPaneClick={() => setSelected(null)}
          onNodesChange={onNodesChange}
        >
          <Background />
          <Controls />
          <MiniMap pannable zoomable nodeColor={(n) => CG_NODE_COLORS[nodes.get(n.id)?.kind ?? 'function']} />
        </ReactFlow>

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
              </p>
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  )
}
