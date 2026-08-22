const STATUS_TONE: Record<string, string> = {
  passed: 'bg-emerald-50 text-emerald-700 ring-emerald-200 dark:bg-emerald-950/50 dark:text-emerald-300 dark:ring-emerald-800',
  failed: 'bg-destructive/10 text-destructive ring-destructive/30',
  errored: 'bg-destructive/10 text-destructive ring-destructive/30',
  skipped: 'bg-muted text-muted-foreground ring-border',
  enabled: 'bg-emerald-50 text-emerald-700 ring-emerald-200 dark:bg-emerald-950/50 dark:text-emerald-300 dark:ring-emerald-800',
  muted: 'bg-amber-50 text-amber-700 ring-amber-200 dark:bg-amber-950/50 dark:text-amber-300 dark:ring-amber-800',
}

export function StatusBadge({ value }: { value: string | null }) {
  const { t } = useTranslation()
  const status = value ?? 'unknown'
  return (
    <span
      className={`inline-flex items-center rounded-sm px-1.5 py-0.5 text-xs font-medium ring-1 ring-inset ${STATUS_TONE[status] ?? 'bg-muted text-muted-foreground ring-border'}`}
    >
      {t(`testengine.status.${status}`, { defaultValue: status })}
    </span>
  )
}
import { useTranslation } from 'react-i18next'
