import { useState } from 'react'
import { Link, Outlet, createRootRoute, useRouterState } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'
import { HugeiconsIcon } from '@hugeicons/react'
import {
  ChartAverageIcon,
  CodeSquareIcon,
  DashboardSquare01Icon,
  Database01Icon,
  HistoryIcon,
  Moon01Icon,
  NeuralNetworkIcon,
  PulseIcon,
  Search01Icon,
  Settings01Icon,
  Sun01Icon,
} from '@hugeicons/core-free-icons'

import { api } from '#/lib/api.ts'
import { getTheme, toggleTheme, type Theme } from '#/lib/theme.ts'
import { Toaster } from '#/components/ui/sonner.tsx'
import { Button } from '#/components/ui/button.tsx'
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarInset,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarProvider,
  SidebarTrigger,
} from '#/components/ui/sidebar.tsx'

import logo from '/logo.svg'
import '../styles.css'

export const Route = createRootRoute({
  component: RootComponent,
})

const NAV = [
  { to: '/', label: 'Dashboard', icon: DashboardSquare01Icon, exact: true },
  { to: '/memories', label: 'Memories', icon: Database01Icon, exact: false },
  { to: '/timeline', label: 'Timeline', icon: HistoryIcon, exact: false },
  { to: '/search', label: 'Search', icon: Search01Icon, exact: false },
  { to: '/graph', label: 'Graph', icon: NeuralNetworkIcon, exact: false },
  { to: '/codegraph', label: 'Code graph', icon: CodeSquareIcon, exact: false },
  { to: '/token-savings', label: 'Token savings', icon: ChartAverageIcon, exact: false },
  { to: '/status', label: 'Status', icon: PulseIcon, exact: false },
  { to: '/settings', label: 'Settings', icon: Settings01Icon, exact: false },
] as const

function RootComponent() {
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  const stats = useQuery({ queryKey: ['stats'], queryFn: api.stats })
  const [theme, setThemeState] = useState<Theme>(getTheme)

  return (
    <SidebarProvider>
      <Sidebar collapsible="icon">
        <SidebarHeader>
          <div className="flex items-center gap-2 px-2 py-1.5 group-data-[collapsible=icon]:px-0">
            <img src={logo} alt="poneglyph" className="h-8 w-8" />
            <span className="text-lg font-bold tracking-tight group-data-[collapsible=icon]:hidden">
              poneglyph
            </span>
          </div>
        </SidebarHeader>
        <SidebarContent>
          <SidebarMenu className="px-2">
            {NAV.map((item) => {
              const active = item.exact ? pathname === item.to : pathname.startsWith(item.to)
              return (
                <SidebarMenuItem key={item.to}>
                  <SidebarMenuButton asChild isActive={active} tooltip={item.label}>
                    <Link to={item.to}>
                      <HugeiconsIcon icon={item.icon} strokeWidth={1.8} />
                      <span>{item.label}</span>
                    </Link>
                  </SidebarMenuButton>
                </SidebarMenuItem>
              )
            })}
          </SidebarMenu>
        </SidebarContent>
        <SidebarFooter>
          <div className="flex items-center justify-between gap-2 px-2 pb-1 group-data-[collapsible=icon]:flex-col">
            <span className="truncate text-xs text-muted-foreground group-data-[collapsible=icon]:hidden">
              {stats.data
                ? `${stats.data.memory_count.toLocaleString()} memories · ${stats.data.project_count} projects`
                : '…'}
            </span>
            <Button
              variant="ghost"
              size="icon"
              aria-label="Toggle dark mode"
              onClick={() => setThemeState(toggleTheme())}
            >
              <HugeiconsIcon icon={theme === 'dark' ? Sun01Icon : Moon01Icon} strokeWidth={1.8} />
            </Button>
          </div>
        </SidebarFooter>
      </Sidebar>
      <SidebarInset>
        <main className="min-w-0 flex-1 p-6">
          <SidebarTrigger className="mb-2 md:hidden" />
          <Outlet />
        </main>
      </SidebarInset>
      <Toaster />
    </SidebarProvider>
  )
}
