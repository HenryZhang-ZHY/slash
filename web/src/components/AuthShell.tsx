import { Slash } from 'lucide-react'

export function AuthShell({
  title,
  description,
  children,
}: {
  title: string
  description: string
  children: React.ReactNode
}) {
  return (
    <div className="grid min-h-screen bg-white lg:grid-cols-[minmax(320px,0.75fr)_minmax(520px,1.25fr)]">
      <aside className="hidden flex-col justify-between border-r bg-[#f3f3f3] p-10 lg:flex">
        <div className="flex items-center gap-3">
          <div className="flex size-9 items-center justify-center bg-black text-white">
            <Slash className="size-5" strokeWidth={2.4} />
          </div>
          <div>
            <div className="text-sm font-semibold">Slash</div>
            <div className="text-xs text-muted-foreground">Engineering control plane</div>
          </div>
        </div>
        <div className="border-y py-5">
          <div className="grid grid-cols-[100px_1fr] gap-y-3 text-xs">
            <span className="text-muted-foreground">Environment</span>
            <span>Production</span>
            <span className="text-muted-foreground">Region</span>
            <span>Azure · westus3</span>
            <span className="text-muted-foreground">Status</span>
            <span className="flex items-center gap-2">
              <span className="size-1.5 bg-emerald-500" /> Operational
            </span>
          </div>
        </div>
        <div className="text-xs text-muted-foreground">Secure workspace access</div>
      </aside>

      <main className="flex min-h-screen items-center justify-center px-5 py-12">
        <div className="w-full max-w-sm">
          <div className="mb-10 flex items-center gap-2 lg:hidden">
            <div className="flex size-8 items-center justify-center bg-black text-white">
              <Slash className="size-4" />
            </div>
            <span className="text-sm font-semibold">Slash</span>
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