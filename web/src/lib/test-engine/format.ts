export function formatDuration(durationMs: number | null) {
  if (durationMs === null) return '—'
  if (durationMs < 1) return '<1 ms'
  if (durationMs < 1000) return `${Math.round(durationMs)} ms`
  if (durationMs < 60_000) return `${(durationMs / 1000).toFixed(2)} s`
  return `${(durationMs / 60_000).toFixed(1)} min`
}

export function formatDate(value: string | null) {
  if (!value) return 'Never'
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  }).format(new Date(value))
}

export function percentage(part: number, total: number) {
  return total === 0 ? 0 : Math.round((part / total) * 1000) / 10
}
