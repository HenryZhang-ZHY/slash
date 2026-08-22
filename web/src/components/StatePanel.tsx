import { AlertCircle, Inbox } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'

export function StatePanel({
  title,
  description,
  retry,
  kind = 'empty',
}: {
  title: string
  description?: string
  retry?: () => void
  kind?: 'empty' | 'error'
}) {
  const { t } = useTranslation()
  const Icon = kind === 'error' ? AlertCircle : Inbox
  return (
    <div className="flex min-h-40 flex-col items-center justify-center rounded-xl border bg-card p-6 text-center text-card-foreground">
      <div className="mb-3 flex size-10 items-center justify-center rounded-full bg-muted">
        <Icon className={kind === 'error' ? 'size-5 text-destructive' : 'size-5 text-muted-foreground'} />
      </div>
      <div className="font-medium">{title}</div>
      {description ? <p className="mt-1 max-w-lg text-sm text-muted-foreground">{description}</p> : null}
      {retry ? <Button className="mt-4" variant="outline" onClick={retry}>{t('common.retry')}</Button> : null}
    </div>
  )
}
