import { useEffect, useState } from 'react'
import { createFileRoute } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'
import { Area, AreaChart, CartesianGrid, XAxis, YAxis } from 'recharts'

import { api } from '#/lib/api.ts'
import type { Activity } from '#/lib/types.ts'
import { pushCapped } from '#/lib/ring.ts'
import { Alert, AlertDescription } from '#/components/ui/alert.tsx'
import { Badge } from '#/components/ui/badge.tsx'
import { Button } from '#/components/ui/button.tsx'
import { Card, CardContent, CardHeader, CardTitle } from '#/components/ui/card.tsx'
import { Spinner } from '#/components/ui/spinner.tsx'
import { Switch } from '#/components/ui/switch.tsx'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '#/components/ui/table.tsx'
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from '#/components/ui/chart.tsx'

export const Route = createFileRoute('/activity')({ component: ActivityPage })

const POLL_MS = 1500
const MAX_SAMPLES = 60

/** Friendly labels for the engine's coarse phase keys (see core/activity.rs). */
const PHASE_LABELS: Record<string, string> = {
  enrich: 'Enriching',
  consolidate: 'Consolidating',
  graph_build: 'Updating code graph',
  retrieve: 'Getting data',
}

type Sample = { t: number; running: number; pending: number }

function sumValues(rec: Record<string, number>): number {
  return Object.values(rec).reduce((a, b) => a + b, 0)
}

const chartConfig = {
  running: { label: 'Running', color: 'var(--chart-1)' },
  pending: { label: 'Pending', color: 'var(--chart-2)' },
} satisfies ChartConfig

function ActivityPage() {
  const [live, setLive] = useState(true)
  const [samples, setSamples] = useState<Sample[]>([])

  const q = useQuery({
    queryKey: ['activity'],
    queryFn: api.activity,
    // Poll only while live-tracking is on, and never behind a hidden tab.
    refetchInterval: live ? POLL_MS : false,
    refetchIntervalInBackground: false,
  })

  // Accumulate each successful poll into a bounded ring for the chart.
  // ponytail: client-side history, resets on reload — backend keeps no series.
  const data: Activity | undefined = q.data
  useEffect(() => {
    if (!data) return
    const running = sumValues(data.jobs.running)
    const pending = sumValues(data.jobs.pending)
    const t = Date.parse(data.generated_at) || Date.now()
    setSamples((s) => pushCapped(s, { t, running, pending }, MAX_SAMPLES))
  }, [data])

  const phases = data?.phases ?? []
  const totalWork = data ? sumValues(data.jobs.running) + sumValues(data.jobs.pending) : 0
  const idle = !!data && phases.length === 0 && totalWork === 0 && data.graph.dirty_projects.length === 0

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-bold">Activity</h1>
        <div className="flex items-center gap-3">
          {live && q.isFetching && <Spinner className="size-4" />}
          <label className="flex cursor-pointer items-center gap-2 text-sm">
            <Switch checked={live} onCheckedChange={setLive} aria-label="Live tracking" />
            Live tracking
          </label>
          {!live && (
            <Button variant="outline" size="sm" onClick={() => void q.refetch()}>
              Refresh
            </Button>
          )}
        </div>
      </div>

      {q.error && (
        <Alert variant="destructive">
          <AlertDescription>{String(q.error)}</AlertDescription>
        </Alert>
      )}

      <Card>
        <CardHeader>
          <CardTitle>Now running</CardTitle>
        </CardHeader>
        <CardContent>
          {phases.length > 0 ? (
            <div className="flex flex-wrap gap-2">
              {phases.map((p) => (
                <Badge key={p} className="gap-1.5 py-1">
                  <Spinner className="size-3" />
                  {PHASE_LABELS[p] ?? p}
                </Badge>
              ))}
            </div>
          ) : (
            <p className="text-sm text-muted-foreground">
              {idle ? 'Idle — nothing in flight.' : live ? 'Waiting…' : 'Live tracking paused.'}
            </p>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>In-flight work over time</CardTitle>
        </CardHeader>
        <CardContent>
          <ChartContainer config={chartConfig} className="h-[220px] w-full">
            <AreaChart data={samples} margin={{ left: 4, right: 8, top: 8 }}>
              <CartesianGrid vertical={false} />
              <XAxis dataKey="t" hide />
              <YAxis allowDecimals={false} width={28} />
              <ChartTooltip
                content={<ChartTooltipContent labelFormatter={(_, p) => new Date(p?.[0]?.payload?.t ?? Date.now()).toLocaleTimeString()} />}
              />
              <Area dataKey="pending" type="step" stackId="1" stroke="var(--color-pending)" fill="var(--color-pending)" fillOpacity={0.3} />
              <Area dataKey="running" type="step" stackId="1" stroke="var(--color-running)" fill="var(--color-running)" fillOpacity={0.55} />
            </AreaChart>
          </ChartContainer>
        </CardContent>
      </Card>

      <div className="grid gap-4 md:grid-cols-2">
        <JobsCard title="Running jobs" jobs={data?.jobs.running} />
        <JobsCard title="Pending jobs" jobs={data?.jobs.pending} />
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Projects awaiting graph rebuild</CardTitle>
        </CardHeader>
        <CardContent>
          {data && data.graph.dirty_projects.length > 0 ? (
            <ul className="space-y-1 font-mono text-sm">
              {data.graph.dirty_projects.map((p) => (
                <li key={p}>{p}</li>
              ))}
            </ul>
          ) : (
            <p className="text-sm text-muted-foreground">None — graph is up to date.</p>
          )}
        </CardContent>
      </Card>
    </div>
  )
}

function JobsCard({ title, jobs }: { title: string; jobs?: Record<string, number> }) {
  const entries = Object.entries(jobs ?? {})
  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
      </CardHeader>
      <CardContent>
        {entries.length === 0 ? (
          <p className="text-sm text-muted-foreground">None.</p>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Type</TableHead>
                <TableHead className="text-right">Count</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {entries.map(([k, v]) => (
                <TableRow key={k}>
                  <TableCell className="font-mono text-xs">{k}</TableCell>
                  <TableCell className="text-right tabular-nums">{v}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </CardContent>
    </Card>
  )
}
