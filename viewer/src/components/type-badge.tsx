import { Badge } from '#/components/ui/badge.tsx'
import { TYPE_COLORS, type MemoryType } from '#/lib/types.ts'

export function TypeBadge({ type }: { type: MemoryType }) {
  const c = TYPE_COLORS[type] ?? '#94a3b8'
  return (
    <Badge
      variant="outline"
      style={{ borderColor: c, color: c, backgroundColor: `${c}15` }}
    >
      {type}
    </Badge>
  )
}
