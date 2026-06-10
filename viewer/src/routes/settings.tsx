import { useEffect, useState } from 'react'
import { createFileRoute } from '@tanstack/react-router'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'

import { api } from '#/lib/api.ts'
import { Alert, AlertDescription } from '#/components/ui/alert.tsx'
import { Button } from '#/components/ui/button.tsx'
import { Card, CardContent, CardHeader, CardTitle } from '#/components/ui/card.tsx'
import { Field, FieldDescription, FieldLabel } from '#/components/ui/field.tsx'
import { Input } from '#/components/ui/input.tsx'
import { Label } from '#/components/ui/label.tsx'
import { Spinner } from '#/components/ui/spinner.tsx'
import { Switch } from '#/components/ui/switch.tsx'

export const Route = createFileRoute('/settings')({ component: SettingsPage })

function SettingsPage() {
  const queryClient = useQueryClient()
  const settings = useQuery({ queryKey: ['settings'], queryFn: api.getSettings })
  const [restartRequired, setRestartRequired] = useState(false)

  // Editable fields (server-side whitelist mirrors this).
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
      toast.success('Settings saved to config.toml')
      queryClient.invalidateQueries({ queryKey: ['settings'] })
    },
    onError: (e) => toast.error(`Save failed: ${e.message}`),
  })

  if (settings.isLoading)
    return (
      <div className="flex justify-center p-12">
        <Spinner />
      </div>
    )
  if (settings.error)
    return (
      <Alert variant="destructive">
        <AlertDescription>{String(settings.error)}</AlertDescription>
      </Alert>
    )
  const s = settings.data!

  return (
    <div className="max-w-2xl space-y-4">
      <h1 className="text-2xl font-bold">Settings</h1>

      {restartRequired && (
        <Alert>
          <AlertDescription>
            Settings saved — restart <code>poneglyph serve</code> to apply.
          </AlertDescription>
        </Alert>
      )}

      <Card>
        <CardHeader>
          <CardTitle>Runtime (read-only)</CardTitle>
        </CardHeader>
        <CardContent>
          <dl className="space-y-2 text-sm">
            <Row k="db_path" v={<code className="text-xs">{String(s.db_path)}</code>} />
            <Row k="model" v={<code className="text-xs">{String(s.embedding?.model_id)}</code>} />
            <Row
              k="server"
              v={`${s.server?.bind_addr}:${s.server?.http_port} (token ${s.server?.api_token_set ? 'set' : 'not set'})`}
            />
          </dl>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Graph</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <Field>
            <FieldLabel htmlFor="sim-threshold">Similarity threshold</FieldLabel>
            <Input
              id="sim-threshold"
              value={form.similarity_threshold}
              onChange={(e) => setForm({ ...form, similarity_threshold: e.target.value })}
            />
            <FieldDescription>
              Cosine similarity (0–1) above which a similarity edge is created.
            </FieldDescription>
          </Field>
          <Field>
            <FieldLabel htmlFor="temporal-window">Temporal window (seconds)</FieldLabel>
            <Input
              id="temporal-window"
              value={form.temporal_window_secs}
              onChange={(e) => setForm({ ...form, temporal_window_secs: e.target.value })}
            />
            <FieldDescription>
              Same-project memories created within this window get a temporal edge.
            </FieldDescription>
          </Field>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Context injection</CardTitle>
        </CardHeader>
        <CardContent>
          <Field>
            <FieldLabel htmlFor="max-tokens">Max tokens</FieldLabel>
            <Input
              id="max-tokens"
              value={form.max_tokens}
              onChange={(e) => setForm({ ...form, max_tokens: e.target.value })}
            />
            <FieldDescription>
              Default budget for get_project_context (the SessionStart hook has its own, smaller
              default).
            </FieldDescription>
          </Field>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>LLM enrichment</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex items-center justify-between">
            <Label htmlFor="enrichment-enabled" className="text-sm">
              Enrichment enabled
            </Label>
            <Switch
              id="enrichment-enabled"
              checked={form.enrichment_enabled}
              onCheckedChange={(v) => setForm({ ...form, enrichment_enabled: v })}
            />
          </div>
          <div className="flex items-center justify-between">
            <Label htmlFor="llm-enabled" className="text-sm">
              LLM client enabled
            </Label>
            <Switch
              id="llm-enabled"
              checked={form.llm_enabled}
              onCheckedChange={(v) => setForm({ ...form, llm_enabled: v })}
            />
          </div>
          <Field>
            <FieldLabel htmlFor="llm-endpoint">Endpoint (OpenAI-compatible)</FieldLabel>
            <Input
              id="llm-endpoint"
              placeholder="http://localhost:11434/v1"
              value={form.llm_endpoint}
              onChange={(e) => setForm({ ...form, llm_endpoint: e.target.value })}
            />
          </Field>
          <Field>
            <FieldLabel htmlFor="llm-model">Model</FieldLabel>
            <Input
              id="llm-model"
              placeholder="llama3.2"
              value={form.llm_model}
              onChange={(e) => setForm({ ...form, llm_model: e.target.value })}
            />
          </Field>
          <p className="text-xs text-muted-foreground">
            Off by default — zero LLM calls unless both switches are on. API keys and tokens can
            only be set in config.toml, never over HTTP.
          </p>
        </CardContent>
      </Card>

      <Button onClick={() => save.mutate()} disabled={save.isPending}>
        {save.isPending ? 'Saving…' : 'Save settings'}
      </Button>
    </div>
  )
}

function Row({ k, v }: { k: string; v: React.ReactNode }) {
  return (
    <div className="flex gap-2">
      <dt className="w-24 shrink-0 text-muted-foreground">{k}</dt>
      <dd className="min-w-0">{v}</dd>
    </div>
  )
}
