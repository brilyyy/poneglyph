import { createFileRoute } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'

import { api } from '#/lib/api.ts'
import { Alert, AlertDescription } from '#/components/ui/alert.tsx'
import { Badge } from '#/components/ui/badge.tsx'
import { Card, CardContent, CardHeader, CardTitle } from '#/components/ui/card.tsx'
import { Spinner } from '#/components/ui/spinner.tsx'

export const Route = createFileRoute('/token-savings')({ component: TokenSavingsPage })

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / (1024 * 1024)).toFixed(2)} MB`
}

function TokenSavingsPage() {
  const savings = useQuery({ queryKey: ['token-savings'], queryFn: api.tokenSavings })

  return (
    <div className="space-y-6">
      <h1 className="text-2xl font-bold">Token savings</h1>

      {savings.error && (
        <Alert variant="destructive">
          <AlertDescription>{String(savings.error)}</AlertDescription>
        </Alert>
      )}

      {savings.isLoading && (
        <div className="flex justify-center p-12">
          <Spinner />
        </div>
      )}

      {savings.data && (
        <>
          <Card>
            <CardHeader className="flex flex-row items-center justify-between">
              <CardTitle>Caveman-grammar compression</CardTitle>
              <Badge variant={savings.data.compression_enabled ? 'default' : 'secondary'}>
                {savings.data.compression_enabled ? 'enabled' : 'disabled'}
              </Badge>
            </CardHeader>
            <CardContent>
              <p className="text-sm text-muted-foreground">
                Estimated from the last {savings.data.sampled_memories} stored memor
                {savings.data.sampled_memories === 1 ? 'y' : 'ies'} by running the real compressor on demand. This is
                a projection, not a measurement of bytes actually saved at rest — compression isn't applied to
                stored content until <code>[memory].compression_enabled</code> is turned on.
              </p>
            </CardContent>
          </Card>

          <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
            <StatCard label="Sampled memories" value={savings.data.sampled_memories.toLocaleString()} />
            <StatCard label="Original size" value={formatBytes(savings.data.original_bytes)} />
            <StatCard label="Compressed size" value={formatBytes(savings.data.compressed_bytes)} />
            <StatCard label="Estimated savings" value={`${savings.data.savings_pct.toFixed(1)}%`} />
          </div>

          <Card>
            <CardContent className="pt-4">
              <div className="h-3 overflow-hidden rounded-full bg-muted">
                <div
                  className="h-full rounded-full bg-primary"
                  style={{ width: `${Math.min(100, Math.max(0, savings.data.savings_pct))}%` }}
                />
              </div>
            </CardContent>
          </Card>
        </>
      )}
    </div>
  )
}

function StatCard({ label, value }: { label: string; value: string }) {
  return (
    <Card className="gap-1 py-4">
      <CardContent className="px-4">
        <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">{label}</p>
        <p className="mt-1 text-2xl font-bold">{value}</p>
      </CardContent>
    </Card>
  )
}
