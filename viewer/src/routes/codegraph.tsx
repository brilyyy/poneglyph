import { useCallback, useEffect, useMemo, useState } from 'react'
import { createFileRoute } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'

import { api } from '#/lib/api.ts'
import { CG_NODE_COLORS, type CgEdge, type CgEdgeKind, type CgNode, type CgNodeKind } from '#/lib/types.ts'
import { Alert, AlertDescription } from '#/components/ui/alert.tsx'
import { Card, CardContent } from '#/components/ui/card.tsx'
import { Empty, EmptyDescription, EmptyTitle } from '#/components/ui/empty.tsx'
import { Spinner } from '#/components/ui/spinner.tsx'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '#/components/ui/select.tsx'
import { CosmosGraph, type CosmosLink, type CosmosNode } from '#/components/cosmos-graph.tsx'

type CodegraphSearch = { project?: string; focus?: string }

const LIMIT_DEBOUNCE_MS = 300

export const Route = createFileRoute('/codegraph')({
  validateSearch: (search: Record<string, unknown>): CodegraphSearch => ({
    project: typeof search.project === 'string' ? search.project : undefined,
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

/** Plain bullet list of nodes; clicking one re-centers the graph on it. */
function NodeList({ title, nodes, onPick }: { title: string; nodes: CgNode[]; onPick: (n: CgNode) => void }) {
  if (nodes.length === 0) return null
  return (
    <div>
      <p className="text-xs font-medium text-muted-foreground">{title}</p>
      <ul className="mt-1 space-y-0.5">
        {nodes.map((n) => (
          <li key={n.id}>
            <button
              className="text-left text-xs text-foreground underline-offset-2 hover:underline"
              onClick={() => onPick(n)}
            >
              {n.name}
            </button>
          </li>
        ))}
      </ul>
    </div>
  )
}

function CodegraphPage() {
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  const { focus } = search
  const [limitInput, setLimitInput] = useState(500)
  const [limit, setLimit] = useState(500)
  const [nodes, setNodesMap] = useState<Map<string, CgNode>>(new Map())
  const [edges, setEdgesMap] = useState<Map<string, CgEdge>>(new Map())
  const [hidden, setHidden] = useState<Set<CgEdgeKind>>(new Set())
  const [selected, setSelected] = useState<CgNode | null>(null)
  const [queryInput, setQueryInput] = useState('')

  const projects = useQuery({ queryKey: ['projects'], queryFn: api.projects })
  const projectPath = search.project

  const initial = useQuery({
    queryKey: ['codegraph', projectPath, focus ?? 'all', limit],
    queryFn: () =>
      focus
        ? api.codegraph({ project_path: projectPath!, focus, depth: 2 })
        : api.codegraph({ project_path: projectPath!, limit }),
    enabled: !!projectPath,
  })

  useEffect(() => {
    if (!initial.data) return
    setNodesMap(new Map(initial.data.nodes.map((n) => [n.id, n])))
    setEdgesMap(new Map(initial.data.edges.map((e) => [edgeId(e), e])))
    setSelected(null)
  }, [initial.data])

  // Slider tracks the drag instantly; the query-triggering value lags so
  // dragging doesn't fire a refetch + GPU rebuild + camera fit per tick.
  useEffect(() => {
    const t = setTimeout(() => setLimit(limitInput), LIMIT_DEBOUNCE_MS)
    return () => clearTimeout(t)
  }, [limitInput])

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
      if (!projectPath) return
      try {
        const resp = await api.codegraph({ project_path: projectPath, focus: node.name, depth: 1 })
        merge(resp.nodes, resp.edges)
      } catch {
        // expansion failure is non-fatal
      }
    },
    [merge, projectPath],
  )

  const selectNode = useCallback(
    (node: CgNode) => {
      setSelected(node)
      void expand(node)
    },
    [expand],
  )

  const explore = useQuery({
    queryKey: ['codegraph-explore', projectPath, selected?.name],
    queryFn: () => api.codegraphExplore({ project_path: projectPath!, target: selected!.name }),
    enabled: !!selected && !!projectPath,
  })

  const search_ = useQuery({
    queryKey: ['codegraph-query', projectPath, queryInput],
    queryFn: () => api.codegraphQuery({ project_path: projectPath!, q: queryInput }),
    enabled: queryInput.length >= 2 && !!projectPath,
  })

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

  const projectSelect = (
    <Select
      value={search.project ?? ''}
      onValueChange={(v) => navigate({ search: { ...search, project: v || undefined, focus: undefined } })}
    >
      <SelectTrigger className="w-64">
        <SelectValue placeholder="select a project" />
      </SelectTrigger>
      <SelectContent>
        {projects.data?.projects.map((p) => (
          <SelectItem key={p.id} value={p.path}>
            {p.name}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )

  if (!projectPath)
    return (
      <div className="flex h-[calc(100vh-3rem)] flex-col gap-3">
        <div className="flex items-center justify-between">
          <h1 className="text-2xl font-bold">Code graph</h1>
          {projectSelect}
        </div>
        <Empty>
          <EmptyTitle>Select a project</EmptyTitle>
          <EmptyDescription>Pick a project above to browse its code graph.</EmptyDescription>
        </Empty>
      </div>
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
        <div className="flex items-center gap-3">
          {projectSelect}
          <span className="text-sm text-muted-foreground">
            showing {nodeArr.length} of {totalNodes} nodes · {edgeArr.length} of {totalEdges} edges
            {isSampled ? ' (sampled)' : ''} · click a node to expand
          </span>
          {initial.data?.stale && (
            <span className="rounded bg-amber-500/20 px-1.5 py-0.5 text-xs text-amber-600">rebuild pending</span>
          )}
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-4 text-xs">
        <div className="relative">
          <input
            type="text"
            placeholder="find a symbol… (or callers_of:foo, path:a..b)"
            value={queryInput}
            onChange={(e) => setQueryInput(e.target.value)}
            className="w-72 rounded-md border border-border bg-background px-2 py-1 text-xs"
          />
          {queryInput.length >= 2 && search_.data && search_.data.results.length > 0 && (
            <ul className="absolute z-10 mt-1 max-h-64 w-72 overflow-auto rounded-md border border-border bg-popover shadow-lg">
              {search_.data.results.slice(0, 20).map((n) => (
                <li key={n.id}>
                  <button
                    className="block w-full px-2 py-1 text-left hover:bg-muted"
                    onClick={() => {
                      selectNode(n)
                      setQueryInput('')
                    }}
                  >
                    <span className="font-medium">{n.name}</span>{' '}
                    <span className="text-muted-foreground">
                      {KIND_LABEL[n.kind]} · {n.file_path}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>

        <div className="flex items-center gap-2 border-l border-border pl-4">
          {(Object.keys(CG_NODE_COLORS) as CgNodeKind[]).map((k) => (
            <span key={k} className="flex items-center gap-1 text-muted-foreground">
              <span className="h-2.5 w-2.5 rounded-full" style={{ backgroundColor: CG_NODE_COLORS[k] }} />
              {k}
            </span>
          ))}
        </div>
        <div className="flex items-center gap-2 border-l border-border pl-4">
          {(['calls', 'imports', 'tests', 'extends'] as CgEdgeKind[]).map((k) => (
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
          render limit: {limitInput}
          <input
            type="range"
            min={100}
            max={Math.max(2000, totalNodes)}
            step={100}
            value={limitInput}
            onChange={(e) => setLimitInput(Number(e.target.value))}
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
            if (node) selectNode(node)
            else setSelected(null)
          }}
          onBackgroundClick={() => setSelected(null)}
        />

        {selected && (
          <Card className="absolute right-3 top-3 w-80 overflow-auto shadow-lg" style={{ maxHeight: 'calc(100% - 1.5rem)' }}>
            <CardContent className="space-y-3">
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

              {explore.isLoading && <Spinner />}

              {explore.data?.snippets[0] && (
                <pre className="max-h-48 overflow-auto rounded bg-muted p-2 text-[11px] whitespace-pre">
                  {explore.data.snippets[0].source}
                </pre>
              )}

              {explore.data && (
                <>
                  <NodeList title="Callers" nodes={explore.data.callers} onPick={selectNode} />
                  <NodeList title="Callees" nodes={explore.data.callees} onPick={selectNode} />
                  {selected.kind === 'type' && (
                    <>
                      <NodeList title="Extends" nodes={explore.data.supertypes} onPick={selectNode} />
                      <NodeList title="Implemented by" nodes={explore.data.subtypes} onPick={selectNode} />
                    </>
                  )}
                  <NodeList title="Tests" nodes={explore.data.tests} onPick={selectNode} />
                </>
              )}
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  )
}
