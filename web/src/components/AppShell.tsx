import { useCallback, useEffect, useState } from 'react'
import { FlaskConical, History, LayoutDashboard, Menu, Users } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { NavLink, Outlet, useLocation } from 'react-router-dom'

import { Button } from '@/components/ui/button'
import { PageTitle } from '@/components/PageTitle'
import { ProductMark } from '@/components/ProductMark'
import { ProductMenu } from '@/components/ProductMenu'
import { api, type MeResponse } from '@/lib/api'
import { activeSection, consoleSections } from '@/lib/navigation'
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

  const icons = { overview: LayoutDashboard, activity: History, tests: FlaskConical }
  const labels = { overview: t('app.overview'), activity: t('app.activity'), tests: t('app.testEngineSection') }
  const navItems = consoleSections[0].items.map((item) => ({ ...item, label: labels[item.id], icon: icons[item.id] }))
  const active = activeSection(location.pathname)
  const activeLabel = active === 'activity' ? t('app.activity') : active === 'tests' ? t('app.testEngineSection') : active === 'account' ? t('app.accountSettings') : active === 'team' ? t('app.teamManagement') : t('app.overview')
  const accountIdentity = me.user.email ?? (me.connections.github ? `@${me.connections.github.login}` : me.user.displayName)

  return (
    <div className="min-h-screen bg-muted/20 md:grid md:grid-cols-[248px_minmax(0,1fr)]">
      <PageTitle title={activeLabel} />
      <aside className="sticky top-0 hidden h-screen flex-col border-r bg-sidebar text-sidebar-foreground md:flex">
        <div className="flex h-14 items-center gap-2.5 border-b px-4">
          <ProductMark />
          <span className="text-sm font-semibold">{t('app.slash')}</span>
        </div>

        <nav className="flex-1 space-y-1 px-3 py-3">
          <div className="px-2 pb-1.5 text-xs font-medium text-muted-foreground uppercase">{t('app.product')}</div>
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
          <div className="mt-5 px-2 pb-1.5 text-xs font-medium text-muted-foreground uppercase">{t('app.teams')}</div>
          {me.teams.map((team) => <NavLink key={team.id} to={`/teams/${team.slug}`} className={({ isActive }) => `flex h-9 items-center gap-2 rounded-md px-2 text-sm transition-colors ${isActive ? 'bg-sidebar-accent font-medium text-sidebar-accent-foreground' : 'text-muted-foreground hover:bg-sidebar-accent/70 hover:text-foreground'}`}><Users className="size-4" /><span className="truncate">{team.name}</span></NavLink>)}
        </nav>
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
            <ProductMenu accountIdentity={accountIdentity} onSignOut={logout} />
            <Sheet>
              <SheetTrigger className="md:hidden" render={<Button variant="ghost" size="icon-sm" aria-label={t('app.openNavigation')} />}><Menu /></SheetTrigger>
              <SheetContent side="left" className="w-[min(20rem,88vw)]">
                <SheetHeader><SheetTitle className="flex items-center gap-2"><ProductMark />{t('app.slash')}</SheetTitle></SheetHeader>
                <nav className="space-y-1 px-4">
                  {navItems.map((item) => { const Icon = item.icon; return <SheetClose key={item.to} render={<NavLink to={item.to} end={item.end} className={({ isActive }) => `flex h-11 items-center gap-3 rounded-lg px-3 ${isActive ? 'bg-muted font-medium' : 'text-muted-foreground'}`} />}><Icon className="size-4" />{item.label}</SheetClose> })}
                  <div className="px-3 pt-5 pb-1 text-xs font-medium uppercase text-muted-foreground">{t('app.teams')}</div>
                  {me.teams.map((team) => <SheetClose key={team.id} render={<NavLink to={`/teams/${team.slug}`} className={({ isActive }) => `flex h-11 items-center gap-3 rounded-lg px-3 ${isActive ? 'bg-muted font-medium' : 'text-muted-foreground'}`} />}><Users className="size-4" />{team.name}</SheetClose>)}
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
