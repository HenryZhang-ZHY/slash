import { useTranslation } from 'react-i18next'
import { Slash } from 'lucide-react'

import { LanguageSwitcher } from '@/components/LanguageSwitcher'

export function AuthShell({
  title,
  description,
  children,
}: {
  title: string
  description: string
  children: React.ReactNode
}) {
  const { t } = useTranslation()

  return (
    <div className="grid min-h-screen bg-white lg:grid-cols-[minmax(320px,0.75fr)_minmax(520px,1.25fr)]">
      <aside className="hidden flex-col justify-between border-r bg-[#f3f3f3] p-10 lg:flex">
        <div className="flex items-center gap-3">
          <div className="flex size-9 items-center justify-center bg-black text-white">
            <Slash className="size-5" strokeWidth={2.4} />
          </div>
          <div>
            <div className="text-sm font-semibold">{t('app.slash')}</div>
            <div className="text-xs text-muted-foreground">{t('auth.controlPlane')}</div>
          </div>
        </div>
        <div className="border-y py-5">
          <div className="grid grid-cols-[100px_1fr] gap-y-3 text-xs">
            <span className="text-muted-foreground">{t('auth.environment')}</span>
            <span>{t('auth.production')}</span>
            <span className="text-muted-foreground">{t('auth.region')}</span>
            <span>Azure · westus3</span>
            <span className="text-muted-foreground">{t('auth.status')}</span>
            <span className="flex items-center gap-2">
              <span className="size-1.5 bg-emerald-500" /> {t('auth.operational')}
            </span>
          </div>
        </div>
        <div className="text-xs text-muted-foreground">{t('auth.secureAccess')}</div>
      </aside>

      <main className="flex min-h-screen items-center justify-center px-5 py-12">
        <div className="w-full max-w-sm">
          <div className="mb-10 flex items-center justify-between lg:flex-col lg:items-start lg:gap-3">
            <div className="flex items-center gap-2">
              <div className="flex size-8 items-center justify-center bg-black text-white lg:hidden">
                <Slash className="size-4" />
              </div>
              <span className="text-sm font-semibold">{t('app.slash')}</span>
            </div>
            <LanguageSwitcher />
          </div>
          <div className="mb-7">
            <h1 className="text-2xl font-semibold">{title}</h1>
            <p className="mt-2 text-sm text-muted-foreground">{description}</p>
          </div>
          {children}
        </div>
      </main>
    </div>
  )
}
