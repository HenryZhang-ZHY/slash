import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useOutletContext, useSearchParams } from 'react-router-dom'
import { CheckCircle2 } from 'lucide-react'

import { Button } from '@/components/ui/button'
import type { DashboardContext } from '@/components/AppShell'
import { githubErrorKey } from '@/lib/authFlow'

function GithubIcon({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="currentColor">
      <path d="M12 0c-6.626 0-12 5.373-12 12 0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576 4.765-1.589 8.199-6.086 8.199-11.386 0-6.627-5.373-12-12-12z"/>
    </svg>
  )
}

export function SettingsPage() {
  const { me } = useOutletContext<DashboardContext>()
  const { t } = useTranslation()
  const [searchParams, setSearchParams] = useSearchParams()
  const [githubResult] = useState(() => searchParams.get('github'))
  const [githubError] = useState(() => searchParams.get('reason'))
  const github = me.connections.github

  useEffect(() => {
    if (searchParams.has('github') || searchParams.has('reason')) {
      setSearchParams({}, { replace: true })
    }
  }, [searchParams, setSearchParams])

  return (
    <div className="mx-auto w-full max-w-[1680px] px-4 py-6 md:px-8 md:py-8">
      <div className="mb-6">
        <h1 className="text-2xl font-semibold">{t('settings.title')}</h1>
        <p className="mt-1 text-sm text-muted-foreground">{t('settings.subtitle')}</p>
      </div>

      <div className="max-w-2xl space-y-8">
        {/* Profile section */}
        <section>
          <h2 className="mb-3 text-sm font-semibold">{t('settings.profile')}</h2>
          <div className="border">
            {me.user.email ? (
              <div className="flex items-center justify-between px-4 py-3">
                <div>
                  <div className="text-xs text-muted-foreground">{t('auth.email')}</div>
                  <div className="mt-0.5 text-sm">{me.user.email}</div>
                </div>
              </div>
            ) : null}
            {me.user.displayName ? (
              <div className={`flex items-center justify-between px-4 py-3 ${me.user.email ? 'border-t' : ''}`}>
                <div>
                  <div className="text-xs text-muted-foreground">{t('settings.displayName')}</div>
                  <div className="mt-0.5 text-sm">{me.user.displayName}</div>
                </div>
              </div>
            ) : null}
          </div>
        </section>

        {/* Connected accounts section */}
        <section>
          <h2 className="mb-3 text-sm font-semibold">{t('settings.connectedAccounts')}</h2>
          <div className="border">
            <div className="flex items-center justify-between px-4 py-3">
              <div className="flex items-center gap-3">
                <GithubIcon className="size-5 text-muted-foreground" />
                <div>
                  <div className="text-sm font-medium">GitHub</div>
                  <div className="text-xs text-muted-foreground">
                    {github
                      ? t('settings.githubConnectedAs', { login: github.login })
                      : t('settings.githubDescription')}
                  </div>
                </div>
              </div>
              {github ? (
                <span className="flex items-center gap-1.5 text-xs font-medium text-emerald-700">
                  <CheckCircle2 className="size-4" />
                  {t('settings.connected')}
                </span>
              ) : (
                <form method="POST" action="/api/auth/github/connect">
                  <Button type="submit" variant="outline" size="sm">
                    {t('settings.connectGitHub')}
                  </Button>
                </form>
              )}
            </div>
          </div>

          {githubResult === 'connected' ? (
            <div className="mt-3 flex items-center gap-2 border-l-2 border-emerald-500 bg-emerald-50 px-3 py-2 text-sm text-emerald-700">
              <CheckCircle2 className="size-4" />
              {t('settings.githubLinkedSuccess')}
            </div>
          ) : null}
          {githubResult === 'error' ? (
            <div className="mt-3 border-l-2 border-red-500 bg-red-50 px-3 py-2 text-sm text-red-700">
              {t(githubErrorKey(githubError))}
            </div>
          ) : null}
        </section>
      </div>
    </div>
  )
}
