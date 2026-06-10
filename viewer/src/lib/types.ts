// Mirrors the Rust serde shapes in poneglyph-core/src/model.rs.

export type MemoryType =
  | 'episodic'
  | 'semantic'
  | 'procedural'
  | 'fact'
  | 'preference'
  | 'code_context'

export type Source = 'explicit' | 'passive' | 'cli' | 'import'

export type EdgeType = 'explicit' | 'similarity' | 'temporal' | 'tag_overlap' | 'relation'

export interface Memory {
  id: string
  content: string
  memory_type: MemoryType
  importance: number
  project_id: string | null
  source: Source
  metadata: Record<string, unknown> | null
  created_at: string
  updated_at: string
  accessed_at: string | null
  access_count: number
}

export interface Edge {
  id: string
  src_id: string
  dst_id: string
  edge_type: EdgeType
  label: string | null
  weight: number
  created_at: string
}

export interface Project {
  id: string
  path: string
  git_remote: string | null
  name: string
  created_at: string
  last_seen_at: string
}

export interface Stats {
  memory_count: number
  edge_count: number
  project_count: number
  pending_jobs: number
  by_type: Partial<Record<MemoryType, number>>
}

export interface MemoryDetail extends Memory {
  edges: Edge[]
}

export interface SearchHit extends Memory {
  score: number
}

export interface GraphResponse {
  nodes: Memory[]
  edges: Edge[]
}

export interface ListResponse {
  results: Memory[]
  total: number
}

export const MEMORY_TYPES: MemoryType[] = [
  'episodic',
  'semantic',
  'procedural',
  'fact',
  'preference',
  'code_context',
]

export const EDGE_TYPES: EdgeType[] = [
  'explicit',
  'similarity',
  'temporal',
  'tag_overlap',
  'relation',
]

/** Node/badge color per memory type (graph legend + badges share this). */
export const TYPE_COLORS: Record<MemoryType, string> = {
  episodic: '#60a5fa', // blue-400
  semantic: '#34d399', // emerald-400
  procedural: '#fbbf24', // amber-400
  fact: '#a78bfa', // violet-400
  preference: '#f472b6', // pink-400
  code_context: '#94a3b8', // slate-400
}

export interface TimelineSession {
  session_id: string | null
  project_id: string | null
  project_name: string | null
  started_at: string
  ended_at: string
  memory_count: number
  memories: Memory[]
}

export interface TimelineResponse {
  sessions: TimelineSession[]
  total: number
}
