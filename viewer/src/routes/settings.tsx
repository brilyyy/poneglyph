import { useEffect, useState } from 'react'
import { createFileRoute } from '@tanstack/react-router'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { api } from '@/lib/api'
import { Button, Card, CardHeader, ErrorNote, Input, Spinner } from '@/components/ui'

export const Route = createFileRoute('/settings')({ component: SettingsPage })

function SettingsPage() {
  const queryClient = useQueryClient()
  const settings = useQuery({ queryKey: ['settings'], queryFn: api.getSettings })
  const [restartRequired, setRestartRequired] = useState(false)

  // Editable (server-side whitelist mirrors this).
  const [form, setForm] = useState({
    similarity_threshold: '',
    temporal_window_secs: '',
    max_tokens: '',
    enrichment_enabled: false,
    llm_enabled: false,
    llm_endpoint: '',
    llm_model: '',
  })

  useEffect(() => {
    const s = settings.data
    if (!s) return
    setForm({
      similarity_threshold: String(s.graph?.similarity_threshold ?? ''),
      temporal_window_secs: String(s.graph?.temporal_window_secs ?? ''),
      max_tokens: String(s.context?.max_tokens ?? ''),
      enrichment_enabled: Boolean(s.enrichment?.enabled),
      llm_enabled: Boolean(s.llm?.enabled),
      llm_endpoint: s.llm?.endpoint ?? '',
      llm_model: s.llm?.model ?? '',
    })
  }, [settings.data])

  const save = useMutation({
    mutationFn: () =>
      api.patchSettings({
        graph: {
          similarity_threshold: Number(form.similarity_threshold),
          temporal_window_secs: Number(form.temporal_window_secs),
        },
        context: { max_tokens: Number(form.max_tokens) },
        enrichment: { enabled: form.enrichment_enabled },
        llm: {
          enabled: form.llm_enabled,
          endpoint: form.llm_endpoint || null,
          model: form.llm_model || null,
        },
      }),
    onSuccess: (resp) => {
      setRestartRequired(resp.restart_required)
      queryClient.invalidateQueries({ queryKey: ['settings'] })
    },
  })

  if (settings.isLoading) return <Spinner />
  if (settings.error) return <ErrorNote error={settings.error} />
  const s = settings.data!

  return (
    <div className="max-w-2xl space-y-4">
      <h1 className="text-2xl font-bold">Settings</h1>

      {restartRequired && (
        <div className="rounded-lg border border-amber-200 bg-amber-50 p-3 text-sm text-amber-800">
          Settings saved to config.toml — restart <code>poneglyph serve</code> to apply.
        </div>
      )}

      <Card>
        <CardHeader title="Runtime (read-only)" />
        <dl className="space-y-2 p-4 text-sm">
          <Row k="db_path" v={<code className="text-xs">{String(s.db_path)}</code>} />
          <Row k="model" v={<code className="text-xs">{String(s.embedding?.model_id)}</code>} />
          <Row
            k="server"
            v={`${s.server?.bind_addr}:${s.server?.http_port} (token ${s.server?.api_token_set ? 'set' : 'not set'})`}
          />
        </dl>
      </Card>

      <Card>
        <CardHeader title="Graph" />
        <div className="space-y-3 p-4">
          <Field label="similarity threshold (0–1)">
            <Input
              value={form.similarity_threshold}
              onChange={(e) => setForm({ ...form, similarity_threshold: e.target.value })}
            />
          </Field>
          <Field label="temporal window (seconds)">
            <Input
              value={form.temporal_window_secs}
              onChange={(e) => setForm({ ...form, temporal_window_secs: e.target.value })}
            />
          </Field>
        </div>
      </Card>

      <Card>
        <CardHeader title="Context injection" />
        <div className="p-4">
          <Field label="max tokens">
            <Input
              value={form.max_tokens}
              onChange={(e) => setForm({ ...form, max_tokens: e.target.value })}
            />
          </Field>
        </div>
      </Card>

      <Card>
        <CardHeader title="LLM enrichment (optional, off by default)" />
        <div className="space-y-3 p-4">
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={form.enrichment_enabled}
              onChange={(e) => setForm({ ...form, enrichment_enabled: e.target.checked })}
            />
            enrichment enabled
          </label>
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={form.llm_enabled}
              onChange={(e) => setForm({ ...form, llm_enabled: e.target.checked })}
            />
            LLM client enabled
          </label>
          <Field label="endpoint (OpenAI-compatible)">
            <Input
              placeholder="http://localhost:11434/v1"
              value={form.llm_endpoint}
              onChange={(e) => setForm({ ...form, llm_endpoint: e.target.value })}
            />
          </Field>
          <Field label="model">
            <Input
              placeholder="llama3.2"
              value={form.llm_model}
              onChange={(e) => setForm({ ...form, llm_model: e.target.value })}
            />
          </Field>
          <p className="text-xs text-zinc-400">
            API keys and tokens can only be set in config.toml, never over HTTP.
          </p>
        </div>
      </Card>

      {save.error && <ErrorNote error={save.error} />}
      <Button onClick={() => save.mutate()} disabled={save.isPending}>
        {save.isPending ? 'Saving…' : 'Save settings'}
      </Button>
    </div>
  )
}

function Row({ k, v }: { k: string; v: React.ReactNode }) {
  return (
    <div className="flex gap-2">
      <dt className="w-24 shrink-0 text-zinc-400">{k}</dt>
      <dd className="min-w-0">{v}</dd>
    </div>
  )
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block text-sm">
      <span className="mb-1 block text-xs font-medium text-zinc-500">{label}</span>
      {children}
    </label>
  )
}
