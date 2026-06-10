import { Link, Outlet, createRootRoute } from '@tanstack/react-router'

import '../styles.css'

export const Route = createRootRoute({
  component: RootComponent,
})

const NAV = [
  { to: '/', label: 'Dashboard' },
  { to: '/memories', label: 'Memories' },
  { to: '/search', label: 'Search' },
  { to: '/graph', label: 'Graph' },
  { to: '/settings', label: 'Settings' },
] as const

function RootComponent() {
  return (
    <div className="flex min-h-screen bg-zinc-50 text-zinc-900">
      <aside className="flex w-52 shrink-0 flex-col border-r border-zinc-200 bg-white">
        <div className="px-5 py-5">
          <h1 className="text-lg font-bold tracking-tight">poneglyph</h1>
          <p className="text-xs text-zinc-400">local memory engine</p>
        </div>
        <nav className="flex flex-col gap-0.5 px-3">
          {NAV.map((item) => (
            <Link
              key={item.to}
              to={item.to}
              className="rounded-lg px-3 py-2 text-sm font-medium text-zinc-600 hover:bg-zinc-100"
              activeProps={{
                className: 'rounded-lg px-3 py-2 text-sm font-medium bg-zinc-900 text-white',
              }}
              activeOptions={{ exact: item.to === '/' }}
            >
              {item.label}
            </Link>
          ))}
        </nav>
      </aside>
      <main className="min-w-0 flex-1 p-6">
        <Outlet />
      </main>
    </div>
  )
}
