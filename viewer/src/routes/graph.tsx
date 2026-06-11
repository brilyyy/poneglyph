import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, createFileRoute } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import {
  Background,
  Controls,
  MiniMap,
  ReactFlow,
  useNodesState,
  type Edge as FlowEdge,
  type Node as FlowNode,
  type OnNodesChange,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import {
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  forceX,
  forceY,
  type Simulation,
  type SimulationNodeDatum,
} from "d3-force";

import { api, formatRelative, truncate } from "#/lib/api.ts";
import { getTheme } from "#/lib/theme.ts";
import {
  EDGE_TYPES,
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
import FloatingEdge from "#/components/floating-edge.tsx";

type GraphSearch = { focus?: string };

export const Route = createFileRoute("/graph")({
  validateSearch: (search: Record<string, unknown>): GraphSearch => ({
    focus: typeof search.focus === "string" ? search.focus : undefined,
  }),
  component: GraphPage,
});

interface SimNode extends SimulationNodeDatum {
  id: string;
}

const edgeTypes = { floating: FloatingEdge };
const NODE_W = 150;
const NODE_H = 50;

function GraphPage() {
  const { focus } = Route.useSearch();
  const [memories, setMemories] = useState<Map<string, Memory>>(new Map());
  const [edges, setEdges] = useState<Map<string, Edge>>(new Map());
  const [hidden, setHidden] = useState<Set<EdgeType>>(new Set());
  const [selected, setSelected] = useState<Memory | null>(null);

  // Stable SimNode map (d3 mutates these objects; never feed them to ReactFlow).
  const simNodesRef = useRef<Map<string, SimNode>>(new Map());
  const simRef = useRef<Simulation<SimNode, Edge> | null>(null);
  const rafRef = useRef<number>(0);

  const initial = useQuery({
    queryKey: ["graph", focus ?? "sample"],
    queryFn: () =>
      focus
        ? api.graph({ focus, depth: 1, limit: 500 })
        : api.graph({ limit: 500 }),
  });

  // Initialize or reset on new data.
  useEffect(() => {
    if (!initial.data) return;
    simNodesRef.current = new Map();
    setMemories(new Map(initial.data.nodes.map((n) => [n.id, n])));
    setEdges(new Map(initial.data.edges.map((e) => [e.id, e])));
    setSelected(null);
  }, [initial.data]);

  // Sync simulation whenever memories or edges change.
  useEffect(() => {
    const memArr = [...memories.values()];
    const edgeArr = [...edges.values()];

    // Reuse existing SimNodes (preserve positions across expansion).
    for (const m of memArr) {
      if (!simNodesRef.current.has(m.id)) {
        // Seed new nodes near origin; slightly offset to avoid exact overlap.
        const existing = simNodesRef.current.size;
        simNodesRef.current.set(m.id, {
          id: m.id,
          x: existing > 0 ? (Math.random() - 0.5) * 80 : 0,
          y: existing > 0 ? (Math.random() - 0.5) * 80 : 0,
        });
      }
    }

    const simNodes: SimNode[] = [];
    for (const m of memArr) {
      const sn = simNodesRef.current.get(m.id);
      if (sn) simNodes.push(sn);
    }

    const simLinks = edgeArr.map((e) => ({
      source: e.src_id,
      target: e.dst_id,
    }));

    // Build or reheat simulation.
    if (!simRef.current) {
      simRef.current = forceSimulation<SimNode>(simNodes)
        .force(
          "link",
          forceLink<SimNode, { source: string; target: string }>(simLinks)
            .id((d) => d.id)
            .distance(180)
            .strength(0.3),
        )
        .force(
          "charge",
          forceManyBody<SimNode>().strength(-400).distanceMax(600),
        )
        .force("x", forceX<SimNode>(0).strength(0.05))
        .force("y", forceY<SimNode>(0).strength(0.05))
        .force("collide", forceCollide<SimNode>(85).strength(0.9).iterations(2))
        .alphaDecay(0.03)
        .velocityDecay(0.4);
    } else {
      simRef.current.nodes(simNodes);
      const linkForce = simRef.current.force("link") as ReturnType<
        typeof forceLink
      >;
      if (linkForce) {
        linkForce.links(simLinks);
        linkForce.id((d: any) => d.id);
      }
      simRef.current.alpha(0.4).restart();
    }

    // Tick handler: rAF-throttled position update.
    simRef.current.on("tick", () => {
      if (rafRef.current) return; // already queued
      rafRef.current = requestAnimationFrame(() => {
        rafRef.current = 0;
        setNodes((prev) =>
          prev.map((n) => {
            const sn = simNodesRef.current.get(n.id);
            return sn ? { ...n, position: { x: sn.x ?? 0, y: sn.y ?? 0 } } : n;
          }),
        );
      });
    });

    return () => {
      // Don't stop simulation on effect cleanup; only stop on unmount.
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [memories, edges]);

  // Cleanup simulation on unmount.
  useEffect(() => {
    return () => {
      simRef.current?.stop();
      simRef.current = null;
      if (rafRef.current) cancelAnimationFrame(rafRef.current);
    };
  }, []);

  const merge = useCallback(
    (nodes: Memory[], newEdges: Edge[], around?: string) => {
      setMemories((prev) => {
        const next = new Map(prev);
        const origin = around ? simNodesRef.current.get(around) : undefined;
        for (const n of nodes) {
          if (!next.has(n.id)) {
            next.set(n.id, n);
            if (origin && !simNodesRef.current.has(n.id)) {
              simNodesRef.current.set(n.id, {
                id: n.id,
                x: origin.x + (Math.random() - 0.5) * 80,
                y: origin.y + (Math.random() - 0.5) * 80,
              });
            }
          }
        }
        return next;
      });
      setEdges((prev) => {
        const next = new Map(prev);
        for (const e of newEdges) next.set(e.id, e);
        return next;
      });
    },
    [],
  );

  const expand = useCallback(
    async (id: string) => {
      try {
        const resp = await api.graph({ focus: id, depth: 1, limit: 100 });
        merge(resp.nodes, resp.edges, id);
      } catch {
        // Expansion failure is non-fatal.
      }
    },
    [merge],
  );

  const memoryArr = useMemo(() => [...memories.values()], [memories]);
  const edgeArr = useMemo(() => [...edges.values()], [edges]);

  // Build ReactFlow nodes and edges from current state.
  const flowNodes: FlowNode[] = useMemo(
    () =>
      memoryArr.map((m) => {
        const sn = simNodesRef.current.get(m.id);
        return {
          id: m.id,
          position: { x: sn?.x ?? 0, y: sn?.y ?? 0 },
          data: { label: truncate(m.content, 40) },
          style: {
            backgroundColor: `${TYPE_COLORS[m.memory_type] ?? "#94a3b8"}33`,
            borderColor: TYPE_COLORS[m.memory_type] ?? "#94a3b8",
            borderWidth: 2,
            borderRadius: 10,
            fontSize: 11,
            width: NODE_W,
            padding: 6,
          },
        };
      }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [memoryArr.length],
  );

  // Use useNodesState for controlled node positions during drag.
  const [nodes, setNodes, onNodesChange] = useNodesState(flowNodes);

  // Sync external flowNodes changes (non-drag).
  useEffect(() => {
    setNodes((prev) => {
      const prevIds = new Set(prev.map((n) => n.id));
      const nextIds = new Set(flowNodes.map((n) => n.id));
      // Only update if the node set actually changed.
      if (
        prevIds.size === nextIds.size &&
        [...prevIds].every((id) => nextIds.has(id))
      ) {
        return prev; // positions updated via tick handler
      }
      return flowNodes;
    });
  }, [flowNodes, setNodes]);

  const flowEdges: FlowEdge[] = useMemo(
    () =>
      edgeArr.map((e) => ({
        id: e.id,
        source: e.src_id,
        target: e.dst_id,
        type: "floating" as const,
        hidden: hidden.has(e.edge_type),
        label: e.edge_type === "relation" ? (e.label ?? undefined) : undefined,
        style: { strokeWidth: Math.max(1, e.weight * 2) },
      })),
    [edgeArr, hidden],
  );

  // Drag handlers.
  const onNodeDragStart = useCallback((_: any, node: any) => {
    const sn = simNodesRef.current.get(node.id);
    if (!sn) return;
    sn.fx = sn.x;
    sn.fy = sn.y;
    simRef.current?.alphaTarget(0.3).restart();
  }, []);

  const onNodeDrag = useCallback((_: any, node: any) => {
    const sn = simNodesRef.current.get(node.id);
    if (!sn) return;
    sn.fx = node.position.x;
    sn.fy = node.position.y;
  }, []);

  const onNodeDragStop = useCallback((_: any, node: any) => {
    const sn = simNodesRef.current.get(node.id);
    if (!sn) return;
    sn.fx = null;
    sn.fy = null;
    simRef.current?.alphaTarget(0);
  }, []);

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

  const selectedEdgeCount = selected
    ? edgeArr.filter(
        (e) => e.src_id === selected.id || e.dst_id === selected.id,
      ).length
    : 0;

  return (
    <div className="flex h-[calc(100vh-3rem)] flex-col gap-3">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-bold">Graph explorer</h1>
        <span className="text-sm text-muted-foreground">
          {memoryArr.length} nodes · {edgeArr.length} edges · click a node to
          expand
        </span>
      </div>

      <div className="flex flex-wrap items-center gap-4 text-xs">
        <div className="flex items-center gap-2">
          {(Object.keys(TYPE_COLORS) as MemoryType[]).map((t) => (
            <span
              key={t}
              className="flex items-center gap-1 text-muted-foreground"
            >
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
      </div>

      <div className="relative min-h-0 flex-1 overflow-hidden rounded-xl border border-border bg-card">
        <ReactFlow
          nodes={nodes}
          edges={flowEdges}
          edgeTypes={edgeTypes}
          fitView
          minZoom={0.05}
          onlyRenderVisibleElements
          nodesConnectable={false}
          colorMode={getTheme()}
          onNodeClick={(_, node) => {
            setSelected(memories.get(node.id) ?? null);
            expand(node.id);
          }}
          onPaneClick={() => setSelected(null)}
          onNodeDragStart={onNodeDragStart}
          onNodeDrag={onNodeDrag}
          onNodeDragStop={onNodeDragStop}
          onNodesChange={onNodesChange as OnNodesChange}
        >
          <Background />
          <Controls />
          <MiniMap
            pannable
            zoomable
            nodeColor={(n) =>
              TYPE_COLORS[
                (memories.get(n.id)?.memory_type ??
                  "code_context") as MemoryType
              ]
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
              <p className="max-h-40 overflow-auto text-sm">
                {selected.content}
              </p>
              <p className="text-xs text-muted-foreground">
                {formatRelative(selected.created_at)} · importance{" "}
                {selected.importance.toFixed(2)} · {selectedEdgeCount} edges
              </p>
              <div className="flex gap-2">
                <Button asChild size="sm" variant="outline">
                  <Link to="/memories/$id" params={{ id: selected.id }}>
                    open detail
                  </Link>
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => expand(selected.id)}
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
