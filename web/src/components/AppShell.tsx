import { useCallback, useEffect, useState } from 'react'
import { FlaskConical, LayoutDashboard, LogOut, Menu, Settings, Users } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { NavLink, Outlet, useLocation } from 'react-router-dom'

import { Button } from '@/components/ui/button'
import { PageTitle } from '@/components/PageTitle'
import { ProductMark } from '@/components/ProductMark'
import { ProductMenu } from '@/components/ProductMenu'
import { api, type MeResponse } from '@/lib/api'
import { Sheet, SheetClose, SheetContent, SheetHeader, SheetTitle, SheetTrigger } from '@/components/ui/sheet'

export interface DashboardContext {
  me: MeResponse
  refreshMe: () => void
}

export function AppShell() {
  const [me, setMe] = useState<MeResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  const location = useLocation()
  const { t } = useTranslation()

  const load = useCallback(() => {
    setError(null)
    api
      .me()
      .then(setMe)
      .catch((requestError) => {
        if (requestError instanceof Error && (requestError as { status?: number }).status === 401) {
          window.location.href = '/login'
          return
        }
        setError(requestError instanceof Error ? requestError.message : t('app.loadFailed'))
      })
  }, [t])

  useEffect(load, [load])

  const logout = async () => {
    await api.logout()
    window.location.href = '/login'
  }

  if (error) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-background p-6">
        <div className="rounded-xl border bg-card p-5 text-sm text-card-foreground">
          <p className="text-destructive">{t('app.sessionError', { error })}</p>
          <Button className="mt-4" variant="outline" onClick={load}>
            {t('app.retry')}
          </Button>
        </div>
      </div>
    )
  }

  if (!me) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-background text-sm text-muted-foreground">
        {t('app.loadingWorkspace')}
      </div>
    )
  }

  const navItems = [
    { to: '/', label: t('app.overview'), icon: LayoutDashboard, end: true },
    { to: '/tests', label: t('app.testEngineSection'), icon: FlaskConical, end: false },
    { to: '/settings', label: t('app.settings'), icon: Settings, end: false },
  ]

  const activeLabel = location.pathname.startsWith('/tests')
    ? t('app.testEngineSection')
    : location.pathname.startsWith('/settings')
      ? t('app.settings')
      : location.pathname.startsWith('/teams/')
        ? t('app.workspace')
      : t('app.overview')
  const accountIdentity = me.user.email ?? (me.connections.github ? `@${me.connections.github.login}` : me.user.displayName)

  return (
    <div className="min-h-screen bg-muted/20 md:grid md:grid-cols-[248px_minmax(0,1fr)]">
      <PageTitle title={activeLabel} />
      <aside className="sticky top-0 hidden h-screen flex-col border-r bg-sidebar text-sidebar-foreground md:flex">
        <div className="flex h-14 items-center gap-2.5 border-b px-4">
          <ProductMark />
          <span className="text-sm font-semibold">{t('app.slash')}</span>
        </div>

        <div className="border-b px-3 py-3">
          <label className="block px-2 pb-1 text-[11px] font-medium uppercase text-muted-foreground" htmlFor="workspace-switcher">{t('app.workspace')}</label>
          <select
            id="workspace-switcher"
            className="h-9 w-full rounded-md border bg-background px-2 text-sm"
            value={location.pathname.startsWith('/teams/') ? location.pathname.split('/')[2] : ''}
            onChange={(event) => { window.location.href = event.target.value ? `/teams/${event.target.value}` : '/' }}
          >
            <option value="">{t('app.allWorkspaces')}</option>
            {me.teams.map((team) => <option key={team.id} value={team.slug}>{team.name}</option>)}
          </select>
        </div>

        <nav className="flex-1 space-y-1 px-3 py-3">
          <div className="px-2 pb-1.5 text-[11px] font-medium text-muted-foreground uppercase">{t('app.workspace')}</div>
          {navItems.map((item) => {
            const Icon = item.icon
            return (
              <NavLink
                key={item.to}
                to={item.to}
                end={item.end}
                className={({ isActive }) =>
                  `flex h-9 items-center gap-2 rounded-md px-2 text-sm transition-colors ${
                    isActive ? 'bg-sidebar-accent font-medium text-sidebar-accent-foreground' : 'text-muted-foreground hover:bg-sidebar-accent/70 hover:text-foreground'
                  }`
                }
              >
                <Icon className="size-4" />
                {item.label}
              </NavLink>
            )
          })}
          <div className="mt-5 px-2 pb-1.5 text-[11px] font-medium text-muted-foreground uppercase">{t('app.organization')}</div>
          <div className="flex h-8 items-center gap-2 px-2 text-sm text-muted-foreground">
            <Users className="size-4" />
            {t('app.teamCountValue', { count: me.teams.length })}
          </div>
        </nav>

        <div className="border-t p-3">
          <div className="mb-2 truncate px-2 text-xs text-muted-foreground">{accountIdentity}</div>
          <Button className="w-full justify-start" variant="ghost" onClick={logout}>
            <LogOut />
            {t('app.signOut')}
          </Button>
        </div>
      </aside>

      <div className="min-w-0 bg-background">
        <header className="sticky top-0 z-20 flex h-14 items-center justify-between border-b bg-background/95 px-4 backdrop-blur md:px-6">
          <div className="flex items-center gap-2 text-sm">
            <ProductMark className="size-7 md:hidden" />
            <span className="text-muted-foreground">{t('app.slash')}</span>
            <span className="text-border">/</span>
            <span className="font-medium">{activeLabel}</span>
          </div>
          <div className="flex items-center gap-2">
            <ProductMenu />
            <Sheet>
              <SheetTrigger className="md:hidden" render={<Button variant="ghost" size="icon-sm" aria-label={t('app.openNavigation')} />}><Menu /></SheetTrigger>
              <SheetContent side="left" className="w-[min(20rem,88vw)]">
                <SheetHeader><SheetTitle className="flex items-center gap-2"><ProductMark />{t('app.slash')}</SheetTitle></SheetHeader>
                <nav className="space-y-1 px-4">
                  {navItems.map((item) => { const Icon = item.icon; return <SheetClose key={item.to} render={<NavLink to={item.to} end={item.end} className={({ isActive }) => `flex h-11 items-center gap-3 rounded-lg px-3 ${isActive ? 'bg-muted font-medium' : 'text-muted-foreground'}`} />}><Icon className="size-4" />{item.label}</SheetClose> })}
                </nav>
              </SheetContent>
            </Sheet>
            <div className="hidden text-xs text-muted-foreground md:block">{accountIdentity}</div>
          </div>
        </header>
        <main className="min-w-0">
          <Outlet context={{ me, refreshMe: load } satisfies DashboardContext} />
        </main>
      </div>
    </div>
  )
}
