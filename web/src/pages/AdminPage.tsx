import { useCallback, useEffect, useState } from 'react'
import {
  Activity,
  Boxes,
  LayoutDashboard,
  LogOut,
  RefreshCw,
  Shield,
  Webhook,
} from 'lucide-react'
import { NavLink, useLocation } from 'react-router-dom'

import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Separator } from '@/components/ui/separator'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { ApiError } from '@/lib/api'
import {
  adminApi,
  type AdminDelivery,
  type AdminDeliveryDetail,
  type AdminInstallation,
  type AdminInvocation,
} from '@/lib/adminApi'

type LoadState<T> = { data: T | null; error: string | null }

function useAdminData<T>(loader: () => Promise<T>) {
  const [state, setState] = useState<LoadState<T>>({ data: null, error: null })
  const load = useCallback(() => {
    setState((current) => ({ ...current, error: null }))
    loader()
      .then((data) => setState({ data, error: null }))
      .catch((error) => setState({ data: null, error: errorMessage(error) }))
  }, [loader])
  useEffect(load, [load])
  return { ...state, reload: load }
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : 'Request failed'
}

function formatTime(value: string | null) {
  return value ? new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(new Date(value)) : '—'
}

function formatAge(seconds: number | null) {
  if (seconds === null) return '—'
  if (seconds < 60) return `${seconds}s`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`
}

function StatusBadge({ status }: { status: string }) {
  const variant = ['failed', 'aborted', 'dispatch_failed', 'correlation_timeout', 'deleted'].includes(status)
    ? 'destructive'
    : ['pending', 'claimed', 'dispatched', 'correlated', 'suspended'].includes(status)
      ? 'secondary'
      : 'outline'
  return <Badge variant={variant}>{status.replaceAll('_', ' ')}</Badge>
}

function ErrorPanel({ message, retry }: { message: string; retry: () => void }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Unable to load admin data</CardTitle>
        <CardDescription>{message}</CardDescription>
      </CardHeader>
      <CardContent><Button variant="outline" onClick={retry}>Try again</Button></CardContent>
    </Card>
  )
}

function AdminLogin({ onAuthenticated }: { onAuthenticated: () => void }) {
  const [secret, setSecret] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const submit = async (event: React.FormEvent) => {
    event.preventDefault()
    setBusy(true)
    setError(null)
    try {
      await adminApi.login(secret)
      setSecret('')
      onAuthenticated()
    } catch (requestError) {
      setError(errorMessage(requestError))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-muted/30 p-6">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <div className="mb-3 flex size-10 items-center justify-center rounded-lg bg-primary text-primary-foreground"><Shield className="size-5" /></div>
          <CardTitle>Slash Admin</CardTitle>
          <CardDescription>Enter the instance admin secret to continue.</CardDescription>
        </CardHeader>
        <CardContent>
          <form className="space-y-4" onSubmit={submit}>
            <div className="space-y-1.5">
              <Label htmlFor="admin-secret">Admin secret</Label>
              <Input id="admin-secret" type="password" autoComplete="current-password" value={secret} onChange={(event) => setSecret(event.target.value)} required autoFocus />
            </div>
            {error ? <p className="text-sm text-destructive">{error}</p> : null}
            <Button className="w-full" type="submit" disabled={busy}>{busy ? 'Signing in…' : 'Sign in'}</Button>
          </form>
        </CardContent>
      </Card>
    </div>
  )
}

function OverviewPage() {
  const loader = useCallback(() => adminApi.overview(), [])
  const { data, error, reload } = useAdminData(loader)
  if (error) return <ErrorPanel message={error} retry={reload} />
  if (!data) return <AdminSkeleton />
  const cards: Array<[string, number | string, string]> = [
    ['Active installations', data.active_installations, `${data.personal_installations} personal · ${data.organization_installations} organizations`],
    ['Webhooks · 24h', data.deliveries_24h, `${data.failed_deliveries_24h} failed · ${data.pending_deliveries} pending`],
    ['Slash activity · 24h', data.invocations_24h, `${data.failed_invocations_24h} failed · ${data.running_invocations} running`],
    ['Registered users', data.registered_users, `${data.suspended_installations} suspended installations`],
  ]
  return (
    <div className="space-y-6">
      <PageHeading title="Overview" description="Instance health and recent GitHub App activity." />
      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
        {cards.map(([label, value, description]) => (
          <Card key={label} size="sm"><CardHeader><CardDescription>{label}</CardDescription><CardTitle className="text-2xl tabular-nums">{value.toLocaleString()}</CardTitle></CardHeader><CardContent className="text-xs text-muted-foreground">{description}</CardContent></Card>
        ))}
      </div>
      <Card>
        <CardHeader><CardTitle>Diagnostics</CardTitle><CardDescription>Signals that usually need operator attention.</CardDescription></CardHeader>
        <CardContent className="grid gap-4 text-sm sm:grid-cols-2">
          <Diagnostic label="Oldest pending webhook" value={formatAge(data.oldest_pending_seconds)} warning={data.oldest_pending_seconds !== null && data.oldest_pending_seconds > 60} />
          <Diagnostic label="Last installation refresh" value={formatTime(data.last_installation_sync_at)} warning={!data.last_installation_sync_at} />
        </CardContent>
      </Card>
    </div>
  )
}

function InstallationsPage() {
  const loader = useCallback(() => adminApi.installations(), [])
  const { data, error, reload } = useAdminData(loader)
  const [refreshing, setRefreshing] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const refresh = async () => {
    setRefreshing(true)
    setNotice(null)
    try {
      const result = await adminApi.refreshInstallations()
      setNotice(result.refreshed ? `Refreshed ${result.installation_count} installations from GitHub.` : `Already refreshed recently; no GitHub request was made.`)
      reload()
    } catch (requestError) {
      setNotice(errorMessage(requestError))
    } finally {
      setRefreshing(false)
    }
  }
  return (
    <div className="space-y-6">
      <PageHeading title="Installations" description="GitHub accounts where this App is installed.">
        <Button onClick={refresh} disabled={refreshing}><RefreshCw className={refreshing ? 'animate-spin' : ''} />{refreshing ? 'Refreshing…' : 'Refresh from GitHub'}</Button>
      </PageHeading>
      {notice ? <div className="rounded-lg border bg-muted/40 px-3 py-2 text-sm">{notice}</div> : null}
      {error ? <ErrorPanel message={error} retry={reload} /> : data ? <InstallationsTable rows={data} /> : <AdminSkeleton />}
    </div>
  )
}

function InstallationsTable({ rows }: { rows: AdminInstallation[] }) {
  return <Card><Table><TableHeader><TableRow><TableHead>Account</TableHead><TableHead>Type</TableHead><TableHead>Status</TableHead><TableHead>Installation ID</TableHead><TableHead>Installed</TableHead><TableHead>Last observed</TableHead></TableRow></TableHeader><TableBody>
    {rows.map((row) => <TableRow key={row.installation_id}><TableCell className="font-medium">{row.account || 'Unknown account'}</TableCell><TableCell>{row.target_type}</TableCell><TableCell><StatusBadge status={row.state} /></TableCell><TableCell className="font-mono text-xs">{row.installation_id}</TableCell><TableCell>{formatTime(row.installed_at)}</TableCell><TableCell>{formatTime(row.last_synced_at ?? row.updated_at)}</TableCell></TableRow>)}
    {rows.length === 0 ? <TableRow><TableCell colSpan={6} className="h-24 text-center text-muted-foreground">No installations observed. Use Refresh from GitHub to calibrate the count.</TableCell></TableRow> : null}
  </TableBody></Table></Card>
}

function WebhooksPage() {
  const loader = useCallback(() => adminApi.deliveries(), [])
  const { data, error, reload } = useAdminData(loader)
  const [selected, setSelected] = useState<AdminDelivery | null>(null)
  const [detail, setDetail] = useState<AdminDeliveryDetail | null>(null)
  useEffect(() => {
    setDetail(null)
    if (selected) adminApi.delivery(selected.delivery_guid).then(setDetail).catch(() => setDetail(null))
  }, [selected])
  return <div className="space-y-6"><PageHeading title="GitHub webhooks" description="The 200 most recent durable webhook deliveries." />
    {error ? <ErrorPanel message={error} retry={reload} /> : data ? <Card><Table><TableHeader><TableRow><TableHead>Received</TableHead><TableHead>Event</TableHead><TableHead>Repository</TableHead><TableHead>Status</TableHead><TableHead>Attempts</TableHead><TableHead>Delivery</TableHead></TableRow></TableHeader><TableBody>
      {data.map((row) => <TableRow key={row.delivery_guid} className="cursor-pointer" onClick={() => setSelected(row)}><TableCell>{formatTime(row.received_at)}</TableCell><TableCell className="font-medium">{row.event}{row.action ? <span className="ml-1 text-muted-foreground">· {row.action}</span> : null}</TableCell><TableCell>{row.repository ?? '—'}</TableCell><TableCell><StatusBadge status={row.state} /></TableCell><TableCell>{row.attempts}</TableCell><TableCell className="max-w-40 truncate font-mono text-xs">{row.delivery_guid}</TableCell></TableRow>)}
    </TableBody></Table></Card> : <AdminSkeleton />}
    <Sheet open={Boolean(selected)} onOpenChange={(open) => { if (!open) setSelected(null) }}><SheetContent side="right" className="w-full overflow-y-auto sm:max-w-2xl"><SheetHeader><SheetTitle>{selected?.event ?? 'Webhook delivery'}</SheetTitle><SheetDescription className="font-mono">{selected?.delivery_guid}</SheetDescription></SheetHeader><div className="space-y-5 px-4 pb-6">{detail ? <DeliveryDetails detail={detail} /> : <AdminSkeleton />}</div></SheetContent></Sheet>
  </div>
}

function DeliveryDetails({ detail }: { detail: AdminDeliveryDetail }) {
  return <><div className="grid grid-cols-2 gap-3 text-sm"><Diagnostic label="Status" value={detail.delivery.state} warning={detail.delivery.state === 'failed'} /><Diagnostic label="Processed" value={formatTime(detail.delivery.processed_at)} /><Diagnostic label="Repository" value={detail.delivery.repository ?? '—'} /><Diagnostic label="Attempts" value={String(detail.delivery.attempts)} /></div>
    {detail.delivery.last_error ? <div className="rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">{detail.delivery.last_error}</div> : null}
    <Separator /><section className="space-y-2"><h3 className="text-sm font-medium">Slash response</h3>{detail.related_invocations.length ? detail.related_invocations.map((invocation) => <InvocationSummary key={invocation.id} invocation={invocation} />) : <p className="text-sm text-muted-foreground">No slash invocation was associated with this webhook.</p>}</section>
    <Separator /><section className="space-y-2"><h3 className="text-sm font-medium">Payload</h3><pre className="max-h-[32rem] overflow-auto rounded-lg bg-muted p-3 text-xs whitespace-pre-wrap break-all">{JSON.stringify(detail.payload, null, 2)}</pre></section></>
}

function ActivityPage() {
  const loader = useCallback(() => adminApi.invocations(), [])
  const { data, error, reload } = useAdminData(loader)
  return <div className="space-y-6"><PageHeading title="Slash activity" description="The 200 most recent slash command invocations and GitHub responses." />{error ? <ErrorPanel message={error} retry={reload} /> : data ? <Card><Table><TableHeader><TableRow><TableHead>Created</TableHead><TableHead>Command</TableHead><TableHead>Repository / PR</TableHead><TableHead>Actor</TableHead><TableHead>Status</TableHead><TableHead>Conclusion</TableHead><TableHead>GitHub</TableHead></TableRow></TableHeader><TableBody>{data.map((row) => <TableRow key={row.id}><TableCell>{formatTime(row.created_at)}</TableCell><TableCell><code>/{row.command}</code></TableCell><TableCell><a className="hover:underline" href={`https://github.com/${row.owner}/${row.repo}/pull/${row.pr_number}`} target="_blank" rel="noreferrer">{row.owner}/{row.repo} #{row.pr_number}</a></TableCell><TableCell>@{row.actor}</TableCell><TableCell><StatusBadge status={row.status} /></TableCell><TableCell>{row.conclusion ?? '—'}</TableCell><TableCell><div className="flex gap-2">{row.check_run_id ? <a className="text-muted-foreground hover:text-foreground" href={`https://github.com/${row.owner}/${row.repo}/runs/${row.check_run_id}`} target="_blank" rel="noreferrer">check</a> : null}{row.workflow_run_id ? <a className="text-muted-foreground hover:text-foreground" href={`https://github.com/${row.owner}/${row.repo}/actions/runs/${row.workflow_run_id}`} target="_blank" rel="noreferrer">run</a> : null}</div></TableCell></TableRow>)}</TableBody></Table></Card> : <AdminSkeleton />}</div>
}

function InvocationSummary({ invocation }: { invocation: AdminInvocation }) {
  return <div className="rounded-lg border p-3 text-sm"><div className="flex items-center justify-between gap-3"><code>/{invocation.command}</code><StatusBadge status={invocation.status} /></div><div className="mt-2 text-xs text-muted-foreground">@{invocation.actor} · {invocation.owner}/{invocation.repo} #{invocation.pr_number}</div>{invocation.failure_reason ? <p className="mt-2 text-destructive">{invocation.failure_reason}</p> : null}</div>
}

function PageHeading({ title, description, children }: { title: string; description: string; children?: React.ReactNode }) {
  return <div className="flex flex-wrap items-end justify-between gap-4"><div><h1 className="text-2xl font-semibold">{title}</h1><p className="mt-1 text-sm text-muted-foreground">{description}</p></div>{children}</div>
}

function Diagnostic({ label, value, warning = false }: { label: string; value: string; warning?: boolean }) {
  return <div className="rounded-lg border p-3"><div className="text-xs text-muted-foreground">{label}</div><div className={warning ? 'mt-1 font-medium text-destructive' : 'mt-1 font-medium'}>{value}</div></div>
}

function AdminSkeleton() {
  return <div className="space-y-3"><Skeleton className="h-24 w-full" /><Skeleton className="h-64 w-full" /></div>
}

function AdminShell() {
  const location = useLocation()
  const items = [
    { to: '/admin', label: 'Overview', icon: LayoutDashboard, exact: true },
    { to: '/admin/installations', label: 'Installations', icon: Boxes },
    { to: '/admin/webhooks', label: 'Webhooks', icon: Webhook },
    { to: '/admin/activity', label: 'Slash activity', icon: Activity },
  ]
  const content = location.pathname.startsWith('/admin/installations') ? <InstallationsPage /> : location.pathname.startsWith('/admin/webhooks') ? <WebhooksPage /> : location.pathname.startsWith('/admin/activity') ? <ActivityPage /> : <OverviewPage />
  return <div className="min-h-screen bg-muted/20 md:grid md:grid-cols-[232px_minmax(0,1fr)]"><aside className="hidden border-r bg-background md:flex md:flex-col"><div className="flex h-14 items-center gap-2 border-b px-4"><Shield className="size-5" /><span className="font-semibold">Slash Admin</span></div><nav className="flex-1 space-y-1 p-3">{items.map(({ to, label, icon: Icon, exact }) => <NavLink key={to} to={to} end={exact} className={({ isActive }) => `flex h-9 items-center gap-2 rounded-lg px-3 text-sm ${isActive ? 'bg-muted font-medium' : 'text-muted-foreground hover:bg-muted/60 hover:text-foreground'}`}><Icon className="size-4" />{label}</NavLink>)}</nav><div className="border-t p-3"><Button variant="ghost" className="w-full justify-start" onClick={async () => { await adminApi.logout(); window.location.reload() }}><LogOut />Sign out</Button></div></aside><div className="min-w-0"><header className="sticky top-0 z-20 flex h-14 items-center justify-between border-b bg-background/95 px-4 backdrop-blur md:px-6"><div className="flex items-center gap-2 text-sm"><Shield className="size-4" /><span>Instance administration</span></div><nav className="flex md:hidden">{items.map(({ to, label, icon: Icon, exact }) => <NavLink key={to} to={to} end={exact} aria-label={label} className={({ isActive }) => `flex size-8 items-center justify-center rounded-lg ${isActive ? 'bg-primary text-primary-foreground' : 'text-muted-foreground'}`}><Icon className="size-4" /></NavLink>)}</nav></header><main className="mx-auto w-full max-w-[1680px] p-4 md:p-8">{content}</main></div></div>
}

export function AdminPage() {
  const [auth, setAuth] = useState<'checking' | 'authenticated' | 'anonymous'>('checking')
  useEffect(() => {
    adminApi.session().then(() => setAuth('authenticated')).catch((error) => {
      if (error instanceof ApiError && error.status === 404) window.location.href = '/'
      else setAuth('anonymous')
    })
  }, [])
  if (auth === 'checking') return <div className="flex min-h-screen items-center justify-center text-sm text-muted-foreground">Checking admin session…</div>
  if (auth === 'anonymous') return <AdminLogin onAuthenticated={() => setAuth('authenticated')} />
  return <AdminShell />
}
