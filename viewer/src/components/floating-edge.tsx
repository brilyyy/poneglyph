import {
  BaseEdge,
  EdgeLabelRenderer,
  getBezierPath,
  useInternalNode,
  type EdgeProps,
} from '@xyflow/react'

function getIntersection(
  pos: { x: number; y: number },
  w: number,
  h: number,
  otherPos: { x: number; y: number },
) {
  const hw = w / 2
  const hh = h / 2
  const cx = pos.x + hw
  const cy = pos.y + hh
  const dx = otherPos.x - cx
  const dy = otherPos.y - cy
  const absDx = Math.abs(dx)
  const absDy = Math.abs(dy)

  if (absDx === 0 && absDy === 0) return { x: cx, y: cy }

  const t = Math.min(1, hw / (absDx || 1e9), hh / (absDy || 1e9))
  return { x: cx + dx * t, y: cy + dy * t }
}

export default function FloatingEdge({
  id,
  source,
  target,
  label,
  style,
  markerEnd,
}: EdgeProps) {
  const sourceNode = useInternalNode(source)
  const targetNode = useInternalNode(target)

  if (!sourceNode || !targetNode) return null

  const sw = sourceNode.measured?.width ?? 150
  const sh = sourceNode.measured?.height ?? 50
  const tw = targetNode.measured?.width ?? 150
  const th = targetNode.measured?.height ?? 50

  const sPos = sourceNode.internals.positionAbsolute
  const tPos = targetNode.internals.positionAbsolute

  const sourcePoint = getIntersection(sPos, sw, sh, tPos)
  const targetPoint = getIntersection(tPos, tw, th, sPos)

  const [edgePath, labelX, labelY] = getBezierPath({
    sourceX: sourcePoint.x,
    sourceY: sourcePoint.y,
    targetX: targetPoint.x,
    targetY: targetPoint.y,
  })

  return (
    <>
      <BaseEdge id={id} path={edgePath} markerEnd={markerEnd} style={style} />
      {label && (
        <EdgeLabelRenderer>
          <div
            style={{
              position: 'absolute',
              transform: `translate(-50%, -50%) translate(${labelX}px,${labelY}px)`,
              pointerEvents: 'all',
            }}
            className="rounded bg-background/80 px-1 py-0.5 text-[10px] text-muted-foreground backdrop-blur-sm"
          >
            {label}
          </div>
        </EdgeLabelRenderer>
      )}
    </>
  )
}
