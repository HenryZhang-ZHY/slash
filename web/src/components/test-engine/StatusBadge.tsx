const STATUS_TONE: Record<string, string> = {
  passed: 'bg-emerald-50 text-emerald-700 ring-emerald-200',
  failed: 'bg-red-50 text-red-700 ring-red-200',
  errored: 'bg-red-50 text-red-700 ring-red-200',
  skipped: 'bg-zinc-100 text-zinc-600 ring-zinc-200',
  enabled: 'bg-emerald-50 text-emerald-700 ring-emerald-200',
  muted: 'bg-amber-50 text-amber-700 ring-amber-200',
}

export function StatusBadge({ value }: { value: string | null }) {
  const status = value ?? 'unknown'
  return (
    <span
      className={`inline-flex items-center rounded-sm px-1.5 py-0.5 text-[11px] font-medium ring-1 ring-inset ${STATUS_TONE[status] ?? 'bg-zinc-100 text-zinc-600 ring-zinc-200'}`}
    >
      {status}
    </span>
  )
}
