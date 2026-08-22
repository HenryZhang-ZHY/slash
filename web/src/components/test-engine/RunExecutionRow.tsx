import { useTranslation } from 'react-i18next'

import { StatusBadge } from '@/components/test-engine/StatusBadge'
import type { RunExecution } from '@/lib/api'
import { formatDate, formatDuration } from '@/lib/test-engine/format'

export function RunExecutionRow({ execution }: { execution: RunExecution }) {
  const { t } = useTranslation()
  const source = execution.file
    ? `${execution.file}${execution.line_no ? `:${execution.line_no}` : ''}`
    : t('testengine.noSourceLocation')

  return (
    <div className="border-b px-5 py-3 last:border-b-0">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <StatusBadge value={execution.status} />
            {execution.test_state !== 'enabled' && <StatusBadge value={execution.test_state} />}
            <span className="font-mono text-xs">{formatDuration(execution.duration_ms)}</span>
          </div>
          <div className="mt-2 break-words font-mono text-xs font-medium">
            {execution.test_name}
          </div>
          <div className="mt-1 truncate text-xs text-muted-foreground" title={source}>
            {source}
          </div>
        </div>
        <time className="shrink-0 text-xs text-muted-foreground">
          {formatDate(execution.captured_at)}
        </time>
      </div>
      {execution.stack && (
        <details className="mt-3 border bg-zinc-950 text-zinc-100">
          <summary className="cursor-pointer px-3 py-2 text-xs text-zinc-300">
            {t('execution.failureOutput')}
          </summary>
          <pre className="max-h-56 overflow-auto border-t border-zinc-800 p-3 text-xs leading-relaxed whitespace-pre-wrap">
            {execution.stack}
          </pre>
        </details>
      )}
    </div>
  )
}
