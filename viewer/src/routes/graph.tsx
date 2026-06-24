import { useCallback, useEffect, useMemo, useState } from "react";
import { Link, createFileRoute } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";

import { api, formatRelative } from "#/lib/api.ts";
import {
  EDGE_TYPES,
  TIER_COLORS,
  TYPE_COLORS,
  type Edge,
  type EdgeType,
  type Memory,
  type MemoryType,
} from "#/lib/types.ts";
import { TypeBadge } from "#/components/type-badge.tsx";
import { Alert, AlertDescription } from "#/components/ui/alert.tsx";
import { Button } from "#/components/ui/button.tsx";
import { Card, CardContent } from "#/components/ui/card.tsx";
import { Spinner } from "#/components/ui/spinner.tsx";
import { CosmosGraph, type CosmosLink, type CosmosNode } from "#/components/cosmos-graph.tsx";

type GraphSearch = { focus?: string };

export const Route = createFileRoute("/graph")({
  validateSearch: (search: Record<string, unknown>): GraphSearch => ({
    focus: typeof search.focus === "string" ? search.focus : undefined,
  }),
  component: GraphPage,
});

// Tier/strength aren't visually encoded by default in the dashboard — fade
// less-relevant memories instead of dropping the data entirely.
const TIER_OPACITY: Record<Memory["tier"], number> = { hot: 1, warm: 0.75, cold: 0.45 };

const LIMIT_DEBOUNCE_MS = 300;

function GraphPage() {
  const { focus } = Route.useSearch();
  const [limitInput, setLimitInput] = useState(500);
  const [limit, setLimit] = useState(500);
  const [memories, setMemories] = useState<Map<string, Memory>>(new Map());
  const [edges, setEdges] = useState<Map<string, Edge>>(new Map());
  const [hidden, setHidden] = useState<Set<EdgeType>>(new Set());
  const [selected, setSelected] = useState<Memory | null>(null);
  const [colorBy, setColorBy] = useState<"type" | "tier">("type");

  const initial = useQuery({
    queryKey: ["graph", focus ?? "sample", limit],
    queryFn: () =>
      focus
        ? api.graph({ focus, depth: 1, limit })
        : api.graph({ limit }),
  });

  useEffect(() => {
    if (!initial.data) return;
    setMemories(new Map(initial.data.nodes.map((n) => [n.id, n])));
    setEdges(new Map(initial.data.edges.map((e) => [e.id, e])));
    setSelected(null);
  }, [initial.data]);

  // Slider tracks the drag instantly; the query-triggering value lags so
  // dragging doesn't fire a refetch + GPU rebuild + camera fit per tick.
  useEffect(() => {
    const t = setTimeout(() => setLimit(limitInput), LIMIT_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [limitInput]);

  const merge = useCallback((nodes: Memory[], newEdges: Edge[]) => {
    setMemories((prev) => {
      const next = new Map(prev);
      for (const n of nodes) next.set(n.id, n);
      return next;
    });
    setEdges((prev) => {
      const next = new Map(prev);
      for (const e of newEdges) next.set(e.id, e);
      return next;
    });
  }, []);

  const expand = useCallback(
    async (id: string) => {
      try {
        const resp = await api.graph({ focus: id, depth: 1, limit: 100 });
        merge(resp.nodes, resp.edges);
      } catch {
        // Expansion failure is non-fatal.
      }
    },
    [merge],
  );

  const memoryArr = useMemo(() => [...memories.values()], [memories]);
  const edgeArr = useMemo(() => [...edges.values()], [edges]);

  const cosmosNodes: CosmosNode[] = useMemo(
    () =>
      memoryArr.map((m) => ({
        id: m.id,
        color:
          colorBy === "tier"
            ? TIER_COLORS[m.tier]
            : TYPE_COLORS[m.memory_type] ?? "#94a3b8",
        // Decoy (consolidated) nodes render larger, so clusters stand out.
        size: 3 + m.importance * 9 + (m.is_decoy ? 4 : 0),
        opacity: TIER_OPACITY[m.tier],
      })),
    [memoryArr, colorBy],
  );

  const cosmosLinks: CosmosLink[] = useMemo(
    () =>
      edgeArr
        .filter((e) => !hidden.has(e.edge_type))
        .map((e) => ({ source: e.src_id, target: e.dst_id, width: Math.max(1, e.weight * 3) })),
    [edgeArr, hidden],
  );

  if (initial.isLoading)
    return (
      <div className="flex justify-center p-12">
        <Spinner />
      </div>
    );
  if (initial.error)
    return (
      <Alert variant="destructive">
        <AlertDescription>{String(initial.error)}</AlertDescription>
      </Alert>
    );

  const totalNodes = initial.data?.total_nodes ?? memoryArr.length;
  const totalEdges = initial.data?.total_edges ?? edgeArr.length;
  const isSampled = totalNodes > memoryArr.length;

  const selectedEdges = selected
    ? edgeArr.filter((e) => e.src_id === selected.id || e.dst_id === selected.id)
    : [];
  const selectedRelations = selectedEdges.filter((e) => e.edge_type === "relation" && e.label);

  return (
    <div className="flex h-[calc(100vh-3rem)] flex-col gap-3">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-bold">Graph explorer</h1>
        <span className="text-sm text-muted-foreground">
          showing {memoryArr.length} of {totalNodes} nodes · {edgeArr.length} of {totalEdges} edges
          {isSampled ? " (sampled)" : ""} · click a node to expand
        </span>
      </div>

      <div className="flex flex-wrap items-center gap-4 text-xs">
        <div className="flex items-center gap-2 rounded-md border border-border p-0.5">
          <button
            className={`rounded-sm px-2 py-1 ${colorBy === "type" ? "bg-accent" : "text-muted-foreground"}`}
            onClick={() => setColorBy("type")}
          >
            color: type
          </button>
          <button
            className={`rounded-sm px-2 py-1 ${colorBy === "tier" ? "bg-accent" : "text-muted-foreground"}`}
            onClick={() => setColorBy("tier")}
          >
            color: tier
          </button>
        </div>
        <div className="flex items-center gap-2 border-l border-border pl-4">
          {colorBy === "tier"
            ? (Object.keys(TIER_COLORS) as Memory["tier"][]).map((t) => (
                <span key={t} className="flex items-center gap-1 text-muted-foreground">
                  <span
                    className="h-2.5 w-2.5 rounded-full"
                    style={{ backgroundColor: TIER_COLORS[t] }}
                  />
                  {t}
                </span>
              ))
            : (Object.keys(TYPE_COLORS) as MemoryType[]).map((t) => (
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
                    const next = new Set(prev);
                    if (next.has(t)) next.delete(t);
                    else next.add(t);
                    return next;
                  })
                }
              />
              {t}
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
            const m = memories.get(id) ?? null;
            setSelected(m);
            if (m) void expand(id);
          }}
          onBackgroundClick={() => setSelected(null)}
        />

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
              <p className="max-h-40 overflow-auto text-sm">
                {selected.content}
              </p>
              <p className="text-xs text-muted-foreground">
                {formatRelative(selected.created_at)} · importance{" "}
                {selected.importance.toFixed(2)} · {selected.tier} tier ·{" "}
                {selectedEdges.length} edges{selected.is_decoy ? " · decoy" : ""}
              </p>
              {selectedRelations.length > 0 && (
                <ul className="space-y-0.5 text-xs text-muted-foreground">
                  {selectedRelations.map((e) => (
                    <li key={e.id} className="truncate">
                      → {e.label}
                    </li>
                  ))}
                </ul>
              )}
              <div className="flex gap-2">
                <Button asChild size="sm" variant="outline">
                  <Link to="/memories/$id" params={{ id: selected.id }}>
                    open detail
                  </Link>
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => void expand(selected.id)}
                >
                  expand neighbors
                </Button>
              </div>
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  );
}
