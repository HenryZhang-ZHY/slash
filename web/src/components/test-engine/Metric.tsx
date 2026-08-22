export function Metric({ label, value, detail }: { label: string; value: string; detail?: string }) {
  return (
    <div className="min-w-0 px-4 py-4 first:pl-0 md:px-5 md:first:pl-0">
      <div className="text-xs font-medium text-muted-foreground uppercase">{label}</div>
      <div className="mt-1.5 flex items-baseline gap-2">
        <span className="text-xl font-semibold tabular-nums">{value}</span>
        {detail && <span className="truncate text-xs text-muted-foreground">{detail}</span>}
      </div>
    </div>
  )
}
