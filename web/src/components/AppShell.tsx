import { useCallback, useEffect, useState } from 'react'
import { FlaskConical, LayoutDashboard, LogOut, Settings, Users } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { NavLink, Outlet, useLocation } from 'react-router-dom'

import { Button } from '@/components/ui/button'
import { LanguageSwitcher } from '@/components/LanguageSwitcher'
import { api, type MeResponse } from '@/lib/api'

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
      <div className="flex min-h-screen items-center justify-center bg-[#f7f7f7] p-6">
        <div className="border bg-white p-5 text-sm">
          <p className="text-red-600">{t('app.sessionError', { error })}</p>
          <Button className="mt-4" variant="outline" onClick={load}>
            {t('app.retry')}
          </Button>
        </div>
      </div>
    )
  }

  if (!me) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-[#f7f7f7] text-sm text-muted-foreground">
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
      : t('app.overview')
  const primaryTeam = me.teams[0]

  return (
    <div className="min-h-screen bg-[#f7f7f7] md:grid md:grid-cols-[232px_minmax(0,1fr)]">
      <aside className="sticky top-0 hidden h-screen flex-col border-r bg-[#f3f3f3] md:flex">
        <div className="flex h-14 items-center gap-2.5 border-b px-4">
          <svg className="size-7" viewBox="0 0 256 256" fill="none" xmlns="http://www.w3.org/2000/svg">
            <polygon fill="white" points="148,20 198,20 108,236 58,236" />
          </svg>
          <span className="text-sm font-semibold">{t('app.slash')}</span>
        </div>

        <div className="border-b px-3 py-3">
          <div className="flex items-center gap-2 px-2 py-1.5">
            <div className="flex size-7 items-center justify-center border bg-white text-xs font-semibold">
              {(primaryTeam?.name ?? me.user.email).slice(0, 1).toUpperCase()}
            </div>
            <div className="min-w-0">
              <div className="truncate text-sm font-medium">{primaryTeam?.name ?? t('app.personalWorkspace')}</div>
              <div className="truncate text-xs text-muted-foreground">{primaryTeam?.slug ?? me.user.email}</div>
            </div>
          </div>
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
                  `flex h-8 items-center gap-2 px-2 text-sm transition-colors ${
                    isActive ? 'bg-white font-medium text-foreground shadow-[0_0_0_1px_rgba(0,0,0,0.08)]' : 'text-muted-foreground hover:bg-white/70 hover:text-foreground'
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
            {t('app.teams')}
          </div>
        </nav>

        <div className="border-t p-3">
          <div className="mb-2 truncate px-2 text-xs text-muted-foreground">{me.user.email}</div>
          <Button className="w-full justify-start" variant="ghost" onClick={logout}>
            <LogOut />
            {t('app.signOut')}
          </Button>
        </div>
      </aside>

      <div className="min-w-0 bg-white">
        <header className="sticky top-0 z-20 flex h-14 items-center justify-between border-b bg-white/95 px-4 backdrop-blur md:px-6">
          <div className="flex items-center gap-2 text-sm">
            <svg className="size-7 md:hidden" viewBox="0 0 256 256" fill="none" xmlns="http://www.w3.org/2000/svg">
              <polygon fill="white" points="148,20 198,20 108,236 58,236" />
            </svg>
            <span className="text-muted-foreground">{t('app.slash')}</span>
            <span className="text-border">/</span>
            <span className="font-medium">{activeLabel}</span>
          </div>
          <div className="flex items-center gap-2">
            <LanguageSwitcher />
            <div className="flex items-center gap-1 md:hidden">
              {navItems.map((item) => {
                const Icon = item.icon
                return (
                  <NavLink
                    key={item.to}
                    to={item.to}
                    end={item.end}
                    title={item.label}
                    aria-label={item.label}
                    className={({ isActive }) =>
                      `flex size-8 items-center justify-center ${isActive ? 'bg-black text-white' : 'text-muted-foreground'}`
                    }
                  >
                    <Icon className="size-4" />
                  </NavLink>
                )
              })}
            </div>
            <div className="hidden text-xs text-muted-foreground md:block">{me.user.email}</div>
          </div>
        </header>
        <main className="min-w-0">
          <Outlet context={{ me, refreshMe: load } satisfies DashboardContext} />
        </main>
      </div>
    </div>
  )
}
