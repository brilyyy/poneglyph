import type { Source, MemoryType } from '#/lib/types.ts'

// ---------------------------------------------------------------------------
// Strength indicator — colored dot showing memory strength (decay state)
// ---------------------------------------------------------------------------

export function StrengthIndicator({ strength }: { strength: number }) {
  const color =
    strength > 0.7
      ? 'bg-green-400'
      : strength > 0.3
        ? 'bg-yellow-400'
        : 'bg-red-400'
  const label =
    strength > 0.7 ? 'strong' : strength > 0.3 ? 'weakening' : 'fading'
  return (
    <span
      className={`inline-block h-2 w-2 rounded-full ${color}`}
      title={`${label} (${(strength * 100).toFixed(0)}%)`}
    />
  )
}

// ---------------------------------------------------------------------------
// Tier badge — hot / warm / cold
// ---------------------------------------------------------------------------

export function TierBadge({ tier }: { tier: string }) {
  const colors: Record<string, string> = {
    hot: 'bg-orange-100 text-orange-800 border-orange-200',
    warm: 'bg-yellow-100 text-yellow-800 border-yellow-200',
    cold: 'bg-blue-100 text-blue-800 border-blue-200',
  }
  return (
    <span
      className={`inline-flex items-center rounded border px-1 py-0.5 text-[10px] font-medium leading-none ${colors[tier] ?? ''}`}
    >
      {tier === 'hot' ? '🔥' : tier === 'cold' ? '❄️' : '♨️'} {tier}
    </span>
  )
}

// ---------------------------------------------------------------------------
// Source badge — captures which client captured this memory
// ---------------------------------------------------------------------------

export function SourceBadge({ source }: { source: Source }) {
  const colors: Record<string, string> = {
    'claude-code': 'bg-orange-100 text-orange-800',
    opencode: 'bg-purple-100 text-purple-800',
    cli: 'bg-gray-100 text-gray-800',
    import: 'bg-cyan-100 text-cyan-800',
    explicit: 'bg-green-100 text-green-800',
    passive: 'bg-slate-100 text-slate-800',
  }
  const labels: Record<string, string> = {
    'claude-code': 'Claude Code',
    opencode: 'OpenCode',
    cli: 'CLI',
    import: 'Import',
    explicit: 'Manual',
    passive: 'Passive',
  }
  return (
    <span
      className={`inline-flex items-center rounded px-1 py-0.5 text-[10px] font-medium leading-none ${colors[source] ?? ''}`}
    >
      {labels[source] ?? source}
    </span>
  )
}

// ---------------------------------------------------------------------------
// Event badge — type of event captured (tool_use, user_message, etc.)
// ---------------------------------------------------------------------------

export function EventBadge({ event }: { event: string }) {
  const colors: Record<string, string> = {
    tool_use: 'bg-indigo-100 text-indigo-800',
    user_message: 'bg-blue-100 text-blue-800',
    assistant_message: 'bg-green-100 text-green-800',
    file_edit: 'bg-amber-100 text-amber-800',
    terminal: 'bg-red-100 text-red-800',
  }
  const icons: Record<string, string> = {
    tool_use: '⚡',
    user_message: '💬',
    assistant_message: '🤖',
    file_edit: '📝',
    terminal: '💻',
  }
  return (
    <span
      className={`inline-flex items-center gap-0.5 rounded px-1 py-0.5 text-[10px] font-medium leading-none ${colors[event] ?? ''}`}
    >
      {icons[event] ?? ''} {event.replace('_', ' ')}
    </span>
  )
}

// ---------------------------------------------------------------------------
// Duration formatter
// ---------------------------------------------------------------------------

export function formatDuration(secs: number): string {
  if (secs < 60) return `${secs}s`
  const mins = Math.floor(secs / 60)
  const remaining = secs % 60
  if (mins < 60) return `${mins}m ${remaining}s`
  const hours = Math.floor(mins / 60)
  const minsRemainder = mins % 60
  return `${hours}h ${minsRemainder}m`
}

// ---------------------------------------------------------------------------
// Type badge — counts per memory type in a session
// ---------------------------------------------------------------------------

export function TypeCounts({
  counts,
}: {
  counts: Partial<Record<MemoryType, number>>
}) {
  const typeColors: Record<string, string> = {
    episodic: 'text-blue-400',
    semantic: 'text-emerald-400',
    procedural: 'text-amber-400',
    fact: 'text-violet-400',
    preference: 'text-pink-400',
    code_context: 'text-slate-400',
  }
  return (
    <span className="inline-flex items-center gap-1 text-[10px] text-muted-foreground">
      {Object.entries(counts).map(([type, count]) => (
        <span key={type} className={typeColors[type] ?? ''}>
          {type}: {count}
        </span>
      ))}
    </span>
  )
}
