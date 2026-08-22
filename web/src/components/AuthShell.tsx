import { useTranslation } from 'react-i18next'

import { PageTitle } from '@/components/PageTitle'
import { ProductMark } from '@/components/ProductMark'
import { ProductMenu } from '@/components/ProductMenu'

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
    <div className="grid min-h-screen bg-background lg:grid-cols-[minmax(320px,0.75fr)_minmax(520px,1.25fr)]">
      <PageTitle title={title} />
      <aside className="hidden flex-col justify-between border-r bg-muted/40 p-10 lg:flex">
        <div className="flex items-center gap-3">
          <ProductMark className="size-9" />
          <div>
            <div className="text-sm font-semibold">{t('app.slash')}</div>
            <div className="text-xs text-muted-foreground">{t('auth.controlPlane')}</div>
          </div>
        </div>
        <div className="max-w-sm">
          <div className="text-3xl font-semibold tracking-tight">{t('auth.shellHeadline')}</div>
          <p className="mt-3 text-sm leading-6 text-muted-foreground">{t('auth.shellDescription')}</p>
        </div>
        <div className="text-xs text-muted-foreground">{t('auth.secureAccess')}</div>
      </aside>

      <main className="flex min-h-screen items-center justify-center px-5 py-12">
        <div className="w-full max-w-sm">
          <div className="mb-10 flex items-center justify-between lg:flex-col lg:items-start lg:gap-3">
            <div className="flex items-center gap-2">
              <ProductMark className="size-8 lg:hidden" />
              <span className="text-sm font-semibold">{t('app.slash')}</span>
            </div>
            <ProductMenu />
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
