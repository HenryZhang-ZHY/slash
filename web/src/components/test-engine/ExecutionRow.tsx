import { useTranslation } from 'react-i18next'
import type { TestExecution } from '@/lib/api'
import { formatDate, formatDuration } from '@/lib/test-engine/format'
import { StatusBadge } from '@/components/test-engine/StatusBadge'

export function ExecutionRow({ execution }: { execution: TestExecution }) {
  const { t } = useTranslation()

  return (
    <div className="border-b px-4 py-3 last:border-b-0">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <StatusBadge value={execution.status} />
            <span className="font-mono text-xs">{formatDuration(execution.duration_ms)}</span>
            <span className="text-xs text-muted-foreground">{execution.ci_provider}</span>
          </div>
          <div
            className="mt-2 truncate font-mono text-[11px] text-muted-foreground"
            title={execution.run_ref}
          >
            {execution.run_ref}
          </div>
        </div>
        <time className="shrink-0 text-[11px] text-muted-foreground">
          {formatDate(execution.captured_at)}
        </time>
      </div>
      <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-muted-foreground">
        <span>{t('execution.started', { date: formatDate(execution.started_at) })}</span>
        {execution.finished_at && (
          <span>{t('execution.finished', { date: formatDate(execution.finished_at) })}</span>
        )}
        {execution.invocation_id && (
          <span className="font-mono">
            {t('execution.invocation', { id: execution.invocation_id.slice(0, 8) })}
          </span>
        )}
      </div>
      {execution.stack && (
        <details className="mt-3 border bg-zinc-950 text-zinc-100">
          <summary className="cursor-pointer px-3 py-2 text-xs text-zinc-300">
            {t('execution.failureOutput')}
          </summary>
          <pre className="max-h-56 overflow-auto border-t border-zinc-800 p-3 text-[11px] leading-relaxed whitespace-pre-wrap">
            {execution.stack}
          </pre>
        </details>
      )}
    </div>
  )
}
