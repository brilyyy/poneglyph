import type {
  AgentsStatus,
  CodegraphResponse,
  CodegraphStats,
  GraphResponse,
  ListResponse,
  MemoryDetail,
  Project,
  SearchHit,
  Stats,
  TimelineResponse,
  TokenSavings,
} from './types.ts'

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(path, init)
  const body = await resp.json().catch(() => null)
  if (!resp.ok) {
    const msg = body && typeof body.error === 'string' ? body.error : `HTTP ${resp.status}`
    throw new Error(msg)
  }
  return body as T
}

function qs(params: Record<string, string | number | undefined>): string {
  const sp = new URLSearchParams()
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== '') sp.set(k, String(v))
  }
  const s = sp.toString()
  return s ? `?${s}` : ''
}

export const api = {
  stats: () => request<Stats>('/api/stats'),

  listMemories: (params: {
    type?: string
    project_path?: string
    limit?: number
    offset?: number
  }) => request<ListResponse>(`/api/memories${qs(params)}`),

  getMemory: (id: string) => request<MemoryDetail>(`/api/memories/${id}`),

  patchMemory: (id: string, new_content: string) =>
    request<{ id: string; updated: boolean }>(`/api/memories/${id}`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ new_content }),
    }),

  deleteMemory: (id: string) =>
    request<{ deleted: boolean }>(`/api/memories/${id}`, { method: 'DELETE' }),

  search: (params: { q: string; limit?: number; type?: string; project_path?: string }) =>
    request<{ results: SearchHit[] }>(`/api/search${qs(params)}`),

  graph: (params: { focus?: string; depth?: number; limit?: number }) =>
    request<GraphResponse>(`/api/graph${qs(params)}`),

  projects: () => request<{ projects: Project[] }>('/api/projects'),

  timeline: (params: {
    project_path?: string
    limit?: number
    offset?: number
    gap_secs?: number
    memory_type?: string
    source?: string
  }) => request<TimelineResponse>(`/api/timeline${qs(params)}`),

  getSettings: () => request<Record<string, any>>('/api/settings'),

  patchSettings: (patch: Record<string, unknown>) =>
    request<{ settings: Record<string, any>; restart_required: boolean }>('/api/settings', {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(patch),
    }),

  codegraph: (params: { focus?: string; depth?: number }) =>
    request<CodegraphResponse>(`/api/codegraph${qs(params)}`),

  codegraphStats: () => request<CodegraphStats>('/api/codegraph/stats'),

  tokenSavings: () => request<TokenSavings>('/api/token-savings'),

  agentsStatus: () => request<AgentsStatus>('/api/agents-status'),
}

export function formatDate(iso: string | null): string {
  if (!iso) return '—'
  const d = new Date(iso)
  return d.toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' })

/** "2 hours ago" style for recent timestamps, absolute date past ~30 days. */
export function formatRelative(iso: string | null): string {
  if (!iso) return '—'
  const then = new Date(iso).getTime()
  const secs = Math.round((then - Date.now()) / 1000)
  const abs = Math.abs(secs)
  if (abs < 60) return rtf.format(secs, 'second')
  if (abs < 3600) return rtf.format(Math.round(secs / 60), 'minute')
  if (abs < 86400) return rtf.format(Math.round(secs / 3600), 'hour')
  if (abs < 30 * 86400) return rtf.format(Math.round(secs / 86400), 'day')
  return formatDate(iso)
}

export function truncate(s: string, max = 120): string {
  return s.length <= max ? s : s.slice(0, max) + '…'
}
