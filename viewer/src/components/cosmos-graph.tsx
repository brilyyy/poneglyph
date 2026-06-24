import { useEffect, useRef } from 'react'
import { Graph, type GraphConfig } from '@cosmos.gl/graph'

import { getTheme } from '#/lib/theme.ts'

export interface CosmosNode {
  id: string
  color: string // hex, e.g. '#34d399'
  size: number // point radius before pointSizeScale
  opacity?: number // 0..1, default 1 — used to encode tier/strength
}

export interface CosmosLink {
  source: string
  target: string
  width?: number
}

interface CosmosGraphProps {
  nodes: CosmosNode[]
  links: CosmosLink[]
  onNodeClick?: (id: string) => void
  onBackgroundClick?: () => void
  className?: string
}

function hexToRgba(hex: string, alpha: number): [number, number, number, number] {
  const h = hex.replace('#', '')
  const r = parseInt(h.slice(0, 2), 16) / 255
  const g = parseInt(h.slice(2, 4), 16) / 255
  const b = parseInt(h.slice(4, 6), 16) / 255
  return [r, g, b, alpha]
}

/**
 * GPU-rendered (WebGL via cosmos.gl) graph view shared by the memory graph
 * and code graph pages — scales to far more points than a DOM/SVG renderer
 * (React Flow) can. Node positions persist in `positionsRef` across data
 * updates so expanding a neighborhood doesn't reshuffle existing points.
 */
export function CosmosGraph({ nodes, links, onNodeClick, onBackgroundClick, className }: CosmosGraphProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const graphRef = useRef<Graph | null>(null)
  const nodeIdsRef = useRef<string[]>([])
  const positionsRef = useRef<Map<string, [number, number]>>(new Map())
  const callbacksRef = useRef({ onNodeClick, onBackgroundClick })
  callbacksRef.current = { onNodeClick, onBackgroundClick }

  useEffect(() => {
    if (!containerRef.current) return
    const config: GraphConfig = {
      backgroundColor: getTheme() === 'dark' ? '#0a0a0a' : '#ffffff',
      // ponytail: cap DPR at 2 — 3x+ panels otherwise render ~9x the
      // fragment-shader work of a 1x canvas for no visible gain.
      pixelRatio: Math.min(window.devicePixelRatio || 1, 2),
      spaceSize: 8192,
      simulationGravity: 0.25,
      simulationCenter: 0.5,
      simulationRepulsion: 1,
      simulationLinkSpring: 1,
      simulationFriction: 0.85,
      // fitViewOnInit fires immediately against zero points (data arrives
      // async) and its GPU position readback can throw in that state —
      // rescalePositions handles framing once real data lands instead.
      fitViewOnInit: false,
      rescalePositions: true,
      enableDrag: true,
      scalePointsOnZoom: true,
      renderHoveredPointRing: true,
      onPointClick: (index) => {
        const id = nodeIdsRef.current[index]
        if (id) callbacksRef.current.onNodeClick?.(id)
      },
      onBackgroundClick: () => callbacksRef.current.onBackgroundClick?.(),
      // The post-render fitView below is a same-frame snapshot taken before
      // the force simulation has spread points out from their initial
      // scatter — correct it once the layout actually settles. `false`:
      // a sim that just cooled down shouldn't be kicked back into motion.
      onSimulationEnd: () => graphRef.current?.fitView(250, 0.1, false),
    }
    const graph = new Graph(containerRef.current, config)
    graphRef.current = graph
    return () => {
      graph.destroy()
      graphRef.current = null
    }
  }, [])

  useEffect(() => {
    const graph = graphRef.current
    if (!graph) return

    const idToIndex = new Map(nodes.map((n, i) => [n.id, i]))
    nodeIdsRef.current = nodes.map((n) => n.id)

    const positions = new Float32Array(nodes.length * 2)
    const colors = new Float32Array(nodes.length * 4)
    const sizes = new Float32Array(nodes.length)
    nodes.forEach((n, i) => {
      let pos = positionsRef.current.get(n.id)
      if (!pos) {
        // Small initial scatter near the origin/default viewport — the
        // simulation's own repulsion spreads points out from there. A wide
        // scatter risks landing entirely outside the (non-refit) viewport
        // for small node counts.
        pos = [(Math.random() - 0.5) * 60, (Math.random() - 0.5) * 60]
        positionsRef.current.set(n.id, pos)
      }
      positions[i * 2] = pos[0]
      positions[i * 2 + 1] = pos[1]
      const [r, g, b, a] = hexToRgba(n.color, n.opacity ?? 1)
      colors[i * 4] = r
      colors[i * 4 + 1] = g
      colors[i * 4 + 2] = b
      colors[i * 4 + 3] = a
      sizes[i] = n.size
    })

    // ponytail: fixed node-count threshold, not a measured frame budget —
    // bump it (or make it configurable) if it's wrong for real hardware.
    // Large graphs skip the force simulation entirely (native cosmos.gl
    // config flag) and render statically at their stored/scattered
    // positions — keeps WebGL load down instead of animating forever.
    const enableSimulation = nodes.length <= 2000
    graph.setConfigPartial({ enableSimulation })

    const validLinks = links.filter((l) => idToIndex.has(l.source) && idToIndex.has(l.target))
    const linkArr = new Float32Array(validLinks.length * 2)
    const linkWidths = new Float32Array(validLinks.length)
    validLinks.forEach((l, i) => {
      linkArr[i * 2] = idToIndex.get(l.source)!
      linkArr[i * 2 + 1] = idToIndex.get(l.target)!
      linkWidths[i] = l.width ?? 1
    })

    graph.setPointPositions(positions)
    graph.setPointColors(colors)
    graph.setPointSizes(sizes)
    graph.setLinks(linkArr)
    graph.setLinkWidths(linkWidths)
    graph.render()
    // ponytail: refits on every nodes/links change (load, expand, filter
    // toggle, limit change) — resets any manual pan/zoom each time. That's
    // the requested behavior; add a "lock view" toggle if it gets annoying.
    // This call is an instant rough preview; for simulated graphs the real
    // fit happens via onSimulationEnd above once positions settle — keep
    // both, this one is the only fit that ever runs for static (>2000
    // node, simulation-disabled) graphs.
    if (nodes.length > 0) graph.fitView(250, 0.1, enableSimulation)
  }, [nodes, links])

  return <div ref={containerRef} className={className} style={{ width: '100%', height: '100%' }} />
}
