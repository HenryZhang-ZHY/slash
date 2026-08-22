import { useCallback, useEffect, useMemo, useState } from 'react'
import { Activity, Boxes, LayoutDashboard, LogOut, Menu, RefreshCw, Search, Webhook } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { NavLink, useLocation } from 'react-router-dom'

import { PageTitle } from '@/components/PageTitle'
import { ProductMark } from '@/components/ProductMark'
import { ProductMenu } from '@/components/ProductMenu'
import { StatePanel } from '@/components/StatePanel'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Separator } from '@/components/ui/separator'
import { Sheet, SheetClose, SheetContent, SheetDescription, SheetHeader, SheetTitle, SheetTrigger } from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ApiError } from '@/lib/api'
import { adminApi, type AdminDelivery, type AdminDeliveryDetail, type AdminInvocation } from '@/lib/adminApi'

type LoadState<T> = { data: T | null; error: string | null; loading: boolean }

function useAdminData<T>(loader: () => Promise<T>) {
  const [state, setState] = useState<LoadState<T>>({ data: null, error: null, loading: true })
  const load = useCallback(() => {
    setState((current) => ({ ...current, error: null, loading: true }))
    loader().then((data) => setState({ data, error: null, loading: false })).catch((error) => setState({ data: null, error: errorMessage(error), loading: false }))
  }, [loader])
  useEffect(load, [load])
  return { ...state, reload: load }
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : 'Request failed'
}

function useFormatters() {
  const { i18n, t } = useTranslation()
  const locale = i18n.language === 'zh-CN' ? 'zh-CN' : 'en'
  return {
    time: (value: string | null) => value ? new Intl.DateTimeFormat(locale, { dateStyle: 'medium', timeStyle: 'short' }).format(new Date(value)) : '—',
    age: (seconds: number | null) => {
      if (seconds === null) return '—'
      if (seconds < 60) return t('admin.ageSeconds', { count: seconds })
      if (seconds < 3600) return t('admin.ageMinutes', { count: Math.floor(seconds / 60) })
      return t('admin.ageHours', { hours: Math.floor(seconds / 3600), minutes: Math.floor((seconds % 3600) / 60) })
    },
  }
}

function StatusBadge({ status }: { status: string }) {
  const { t } = useTranslation()
  const variant = ['failed', 'aborted', 'dispatch_failed', 'correlation_timeout', 'deleted'].includes(status) ? 'destructive' : ['pending', 'claimed', 'dispatched', 'correlated', 'suspended'].includes(status) ? 'secondary' : 'outline'
  return <Badge variant={variant}>{t(`admin.status.${status}`, { defaultValue: status.replaceAll('_', ' ') })}</Badge>
}

function SearchField({ value, onChange, placeholder }: { value: string; onChange: (value: string) => void; placeholder: string }) {
  return <div className="relative w-full sm:max-w-sm"><Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" /><Input className="pl-9" value={value} onChange={(event) => onChange(event.target.value)} placeholder={placeholder} /></div>
}

function AdminLogin({ onAuthenticated }: { onAuthenticated: () => void }) {
  const { t } = useTranslation()
  const [secret, setSecret] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const submit = async (event: React.FormEvent) => {
    event.preventDefault(); setBusy(true); setError(null)
    try { await adminApi.login(secret); setSecret(''); onAuthenticated() } catch (requestError) { setError(errorMessage(requestError)) } finally { setBusy(false) }
  }
  return <div className="flex min-h-screen items-center justify-center bg-muted/20 p-6"><PageTitle title={t('admin.title')} /><div className="absolute right-4 top-4"><ProductMenu /></div><Card className="w-full max-w-sm"><CardHeader><div className="mb-3 flex size-10 items-center justify-center rounded-lg bg-primary text-primary-foreground"><ProductMark className="size-6" /></div><CardTitle>{t('admin.title')}</CardTitle><CardDescription>{t('admin.loginDescription')}</CardDescription></CardHeader><CardContent><form className="space-y-4" onSubmit={submit}><div className="space-y-1.5"><Label htmlFor="admin-secret">{t('admin.secret')}</Label><Input id="admin-secret" type="password" autoComplete="current-password" value={secret} onChange={(event) => setSecret(event.target.value)} required autoFocus /></div>{error ? <p className="text-sm text-destructive">{error}</p> : null}<Button className="w-full" type="submit" disabled={busy}>{busy ? t('admin.signingIn') : t('admin.signIn')}</Button></form></CardContent></Card></div>
}

function PageHeading({ title, description, children }: { title: string; description: string; children?: React.ReactNode }) {
  return <div className="flex flex-wrap items-end justify-between gap-4"><div><h1 className="text-2xl font-semibold">{title}</h1><p className="mt-1 text-sm text-muted-foreground">{description}</p></div>{children}</div>
}

function Diagnostic({ label, value, warning = false }: { label: string; value: string; warning?: boolean }) {
  return <div className="rounded-lg border bg-card p-3"><div className="text-xs text-muted-foreground">{label}</div><div className={warning ? 'mt-1 font-medium text-destructive' : 'mt-1 font-medium'}>{value}</div></div>
}

function AdminSkeleton() { return <div className="space-y-3"><Skeleton className="h-24 w-full" /><Skeleton className="h-64 w-full" /></div> }

function RefreshButton({ loading, onClick }: { loading: boolean; onClick: () => void }) {
  const { t } = useTranslation()
  return <Button variant="outline" onClick={onClick} disabled={loading}><RefreshCw className={loading ? 'animate-spin' : ''} />{t('admin.refresh')}</Button>
}

function OverviewPage() {
  const { t } = useTranslation(); const format = useFormatters(); const loader = useCallback(() => adminApi.overview(), []); const { data, error, loading, reload } = useAdminData(loader)
  if (error) return <StatePanel kind="error" title={t('admin.loadFailed')} description={error} retry={reload} />
  if (!data) return <AdminSkeleton />
  const cards: Array<[string, number, string]> = [
    [t('admin.activeInstallations'), data.active_installations, t('admin.installationBreakdown', { personal: data.personal_installations, organizations: data.organization_installations })],
    [t('admin.webhooks24h'), data.deliveries_24h, t('admin.webhookBreakdown', { failed: data.failed_deliveries_24h, pending: data.pending_deliveries })],
    [t('admin.activity24h'), data.invocations_24h, t('admin.activityBreakdown', { failed: data.failed_invocations_24h, running: data.running_invocations })],
    [t('admin.registeredUsers'), data.registered_users, t('admin.suspendedInstallations', { count: data.suspended_installations })],
  ]
  return <div className="space-y-6"><PageHeading title={t('admin.overview')} description={t('admin.overviewDescription')}><RefreshButton loading={loading} onClick={reload} /></PageHeading><div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">{cards.map(([label, value, description]) => <Card key={label} size="sm"><CardHeader><CardDescription>{label}</CardDescription><CardTitle className="text-2xl tabular-nums">{value.toLocaleString()}</CardTitle></CardHeader><CardContent className="text-xs text-muted-foreground">{description}</CardContent></Card>)}</div><Card><CardHeader><CardTitle>{t('admin.diagnostics')}</CardTitle><CardDescription>{t('admin.diagnosticsDescription')}</CardDescription></CardHeader><CardContent className="grid gap-4 text-sm sm:grid-cols-2"><Diagnostic label={t('admin.oldestPending')} value={format.age(data.oldest_pending_seconds)} warning={data.oldest_pending_seconds !== null && data.oldest_pending_seconds > 60} /><Diagnostic label={t('admin.lastInstallationRefresh')} value={format.time(data.last_installation_sync_at)} warning={!data.last_installation_sync_at} /></CardContent></Card></div>
}

function InstallationsPage() {
  const { t } = useTranslation(); const format = useFormatters(); const loader = useCallback(() => adminApi.installations(), []); const { data, error, loading, reload } = useAdminData(loader); const [query, setQuery] = useState(''); const [refreshing, setRefreshing] = useState(false); const [notice, setNotice] = useState<string | null>(null)
  const rows = useMemo(() => (data ?? []).filter((row) => `${row.account} ${row.target_type} ${row.state} ${row.installation_id}`.toLowerCase().includes(query.toLowerCase())), [data, query])
  const refresh = async () => { setRefreshing(true); setNotice(null); try { const result = await adminApi.refreshInstallations(); setNotice(result.refreshed ? t('admin.refreshedInstallations', { count: result.installation_count }) : t('admin.refreshSkipped')); reload() } catch (requestError) { setNotice(errorMessage(requestError)) } finally { setRefreshing(false) } }
  return <div className="space-y-6"><PageHeading title={t('admin.installations')} description={t('admin.installationsDescription')}><div className="flex gap-2"><RefreshButton loading={loading} onClick={reload} /><Button onClick={refresh} disabled={refreshing}><RefreshCw className={refreshing ? 'animate-spin' : ''} />{refreshing ? t('admin.refreshing') : t('admin.refreshFromGitHub')}</Button></div></PageHeading>{notice ? <div className="rounded-lg border bg-muted/40 px-3 py-2 text-sm">{notice}</div> : null}<SearchField value={query} onChange={setQuery} placeholder={t('admin.searchInstallations')} />{error ? <StatePanel kind="error" title={t('admin.loadFailed')} description={error} retry={reload} /> : !data ? <AdminSkeleton /> : rows.length === 0 ? <StatePanel title={query ? t('admin.noSearchResults') : t('admin.noInstallations')} description={query ? undefined : t('admin.noInstallationsDescription')} /> : <><Card className="hidden overflow-hidden md:block"><Table><TableHeader><TableRow><TableHead>{t('admin.account')}</TableHead><TableHead>{t('admin.type')}</TableHead><TableHead>{t('admin.status')}</TableHead><TableHead>{t('admin.installationId')}</TableHead><TableHead>{t('admin.installed')}</TableHead><TableHead>{t('admin.lastObserved')}</TableHead></TableRow></TableHeader><TableBody>{rows.map((row) => <TableRow key={row.installation_id}><TableCell className="font-medium">{row.account || t('admin.unknownAccount')}</TableCell><TableCell>{row.target_type}</TableCell><TableCell><StatusBadge status={row.state} /></TableCell><TableCell className="font-mono text-xs">{row.installation_id}</TableCell><TableCell>{format.time(row.installed_at)}</TableCell><TableCell>{format.time(row.last_synced_at ?? row.updated_at)}</TableCell></TableRow>)}</TableBody></Table></Card><div className="grid gap-3 md:hidden">{rows.map((row) => <Card key={row.installation_id} size="sm"><CardHeader><div className="flex items-center justify-between gap-3"><CardTitle>{row.account || t('admin.unknownAccount')}</CardTitle><StatusBadge status={row.state} /></div><CardDescription>{row.target_type} · {row.installation_id}</CardDescription></CardHeader><CardContent className="text-xs text-muted-foreground">{t('admin.lastObserved')}: {format.time(row.last_synced_at ?? row.updated_at)}</CardContent></Card>)}</div></>}</div>
}

function WebhooksPage() {
  const { t } = useTranslation(); const format = useFormatters(); const loader = useCallback(() => adminApi.deliveries(), []); const { data, error, loading, reload } = useAdminData(loader); const [query, setQuery] = useState(''); const [selected, setSelected] = useState<AdminDelivery | null>(null); const [detail, setDetail] = useState<LoadState<AdminDeliveryDetail>>({ data: null, error: null, loading: false })
  const rows = useMemo(() => (data ?? []).filter((row) => `${row.event} ${row.action ?? ''} ${row.repository ?? ''} ${row.state} ${row.delivery_guid}`.toLowerCase().includes(query.toLowerCase())), [data, query])
  const loadDetail = useCallback(() => { if (!selected) return; setDetail({ data: null, error: null, loading: true }); adminApi.delivery(selected.delivery_guid).then((value) => setDetail({ data: value, error: null, loading: false })).catch((requestError) => setDetail({ data: null, error: errorMessage(requestError), loading: false })) }, [selected])
  useEffect(loadDetail, [loadDetail])
  return <div className="space-y-6"><PageHeading title={t('admin.webhooks')} description={t('admin.webhooksDescription')}><RefreshButton loading={loading} onClick={reload} /></PageHeading><SearchField value={query} onChange={setQuery} placeholder={t('admin.searchWebhooks')} />{error ? <StatePanel kind="error" title={t('admin.loadFailed')} description={error} retry={reload} /> : !data ? <AdminSkeleton /> : rows.length === 0 ? <StatePanel title={query ? t('admin.noSearchResults') : t('admin.noWebhooks')} /> : <><Card className="hidden overflow-hidden md:block"><Table><TableHeader><TableRow><TableHead>{t('admin.received')}</TableHead><TableHead>{t('admin.event')}</TableHead><TableHead>{t('admin.repository')}</TableHead><TableHead>{t('admin.status')}</TableHead><TableHead>{t('admin.attempts')}</TableHead><TableHead>{t('admin.delivery')}</TableHead></TableRow></TableHeader><TableBody>{rows.map((row) => <TableRow key={row.delivery_guid} className="cursor-pointer" onClick={() => setSelected(row)}><TableCell>{format.time(row.received_at)}</TableCell><TableCell className="font-medium">{row.event}{row.action ? <span className="ml-1 text-muted-foreground">· {row.action}</span> : null}</TableCell><TableCell>{row.repository ?? '—'}</TableCell><TableCell><StatusBadge status={row.state} /></TableCell><TableCell>{row.attempts}</TableCell><TableCell className="max-w-40 truncate font-mono text-xs">{row.delivery_guid}</TableCell></TableRow>)}</TableBody></Table></Card><div className="grid gap-3 md:hidden">{rows.map((row) => <button key={row.delivery_guid} className="rounded-xl border bg-card p-4 text-left" onClick={() => setSelected(row)}><div className="flex items-start justify-between gap-3"><div><div className="font-medium">{row.event}{row.action ? ` · ${row.action}` : ''}</div><div className="mt-1 text-xs text-muted-foreground">{row.repository ?? row.delivery_guid}</div></div><StatusBadge status={row.state} /></div><div className="mt-3 text-xs text-muted-foreground">{format.time(row.received_at)}</div></button>)}</div></>}<Sheet open={Boolean(selected)} onOpenChange={(open) => { if (!open) setSelected(null) }}><SheetContent side="right" className="w-full overflow-y-auto sm:max-w-2xl"><SheetHeader><SheetTitle>{selected?.event ?? t('admin.webhookDelivery')}</SheetTitle><SheetDescription className="font-mono">{selected?.delivery_guid}</SheetDescription></SheetHeader><div className="space-y-5 px-4 pb-6">{detail.error ? <StatePanel kind="error" title={t('admin.detailLoadFailed')} description={detail.error} retry={loadDetail} /> : detail.data ? <DeliveryDetails detail={detail.data} /> : <AdminSkeleton />}</div></SheetContent></Sheet></div>
}

function DeliveryDetails({ detail }: { detail: AdminDeliveryDetail }) {
  const { t } = useTranslation(); const format = useFormatters()
  return <><div className="grid grid-cols-2 gap-3 text-sm"><Diagnostic label={t('admin.status')} value={t(`admin.status.${detail.delivery.state}`)} warning={detail.delivery.state === 'failed'} /><Diagnostic label={t('admin.processed')} value={format.time(detail.delivery.processed_at)} /><Diagnostic label={t('admin.repository')} value={detail.delivery.repository ?? '—'} /><Diagnostic label={t('admin.attempts')} value={String(detail.delivery.attempts)} /></div>{detail.delivery.last_error ? <div className="rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">{detail.delivery.last_error}</div> : null}<Separator /><section className="space-y-2"><h3 className="text-sm font-medium">{t('admin.slashResponse')}</h3>{detail.related_invocations.length ? detail.related_invocations.map((invocation) => <InvocationSummary key={invocation.id} invocation={invocation} />) : <p className="text-sm text-muted-foreground">{t('admin.noRelatedInvocation')}</p>}</section><Separator /><section className="space-y-2"><h3 className="text-sm font-medium">{t('admin.payload')}</h3><pre className="max-h-[32rem] overflow-auto rounded-lg bg-muted p-3 text-xs whitespace-pre-wrap break-all">{JSON.stringify(detail.payload, null, 2)}</pre></section></>
}

function ActivityPage() {
  const { t } = useTranslation(); const format = useFormatters(); const loader = useCallback(() => adminApi.invocations(), []); const { data, error, loading, reload } = useAdminData(loader); const [query, setQuery] = useState('')
  const rows = useMemo(() => (data ?? []).filter((row) => `${row.command} ${row.owner}/${row.repo} ${row.actor} ${row.status} ${row.conclusion ?? ''}`.toLowerCase().includes(query.toLowerCase())), [data, query])
  return <div className="space-y-6"><PageHeading title={t('admin.activity')} description={t('admin.activityDescription')}><RefreshButton loading={loading} onClick={reload} /></PageHeading><SearchField value={query} onChange={setQuery} placeholder={t('admin.searchActivity')} />{error ? <StatePanel kind="error" title={t('admin.loadFailed')} description={error} retry={reload} /> : !data ? <AdminSkeleton /> : rows.length === 0 ? <StatePanel title={query ? t('admin.noSearchResults') : t('admin.noActivity')} /> : <><Card className="hidden overflow-hidden lg:block"><Table><TableHeader><TableRow><TableHead>{t('admin.created')}</TableHead><TableHead>{t('admin.command')}</TableHead><TableHead>{t('admin.repositoryPr')}</TableHead><TableHead>{t('admin.actor')}</TableHead><TableHead>{t('admin.status')}</TableHead><TableHead>{t('admin.conclusion')}</TableHead><TableHead>{t('admin.github')}</TableHead></TableRow></TableHeader><TableBody>{rows.map((row) => <TableRow key={row.id}><TableCell>{format.time(row.created_at)}</TableCell><TableCell><code>/{row.command}</code></TableCell><TableCell><a className="hover:underline" href={`https://github.com/${row.owner}/${row.repo}/pull/${row.pr_number}`} target="_blank" rel="noreferrer">{row.owner}/{row.repo} #{row.pr_number}</a></TableCell><TableCell>@{row.actor}</TableCell><TableCell><StatusBadge status={row.status} /></TableCell><TableCell>{row.conclusion ? t(`admin.status.${row.conclusion}`, { defaultValue: row.conclusion }) : '—'}</TableCell><TableCell><div className="flex gap-2">{row.check_run_id ? <a className="text-muted-foreground hover:text-foreground" href={`https://github.com/${row.owner}/${row.repo}/runs/${row.check_run_id}`} target="_blank" rel="noreferrer">{t('admin.check')}</a> : null}{row.workflow_run_id ? <a className="text-muted-foreground hover:text-foreground" href={`https://github.com/${row.owner}/${row.repo}/actions/runs/${row.workflow_run_id}`} target="_blank" rel="noreferrer">{t('admin.run')}</a> : null}</div></TableCell></TableRow>)}</TableBody></Table></Card><div className="grid gap-3 lg:hidden">{rows.map((row) => <Card key={row.id} size="sm"><CardHeader><div className="flex items-center justify-between gap-3"><CardTitle><code>/{row.command}</code></CardTitle><StatusBadge status={row.status} /></div><CardDescription>{row.owner}/{row.repo} #{row.pr_number} · @{row.actor}</CardDescription></CardHeader><CardContent className="text-xs text-muted-foreground">{format.time(row.created_at)}</CardContent></Card>)}</div></>}</div>
}

function InvocationSummary({ invocation }: { invocation: AdminInvocation }) { return <div className="rounded-lg border p-3 text-sm"><div className="flex items-center justify-between gap-3"><code>/{invocation.command}</code><StatusBadge status={invocation.status} /></div><div className="mt-2 text-xs text-muted-foreground">@{invocation.actor} · {invocation.owner}/{invocation.repo} #{invocation.pr_number}</div>{invocation.failure_reason ? <p className="mt-2 text-destructive">{invocation.failure_reason}</p> : null}</div> }

function AdminShell() {
  const { t } = useTranslation(); const location = useLocation()
  const items = [{ to: '/admin', label: t('admin.overview'), icon: LayoutDashboard, exact: true }, { to: '/admin/installations', label: t('admin.installations'), icon: Boxes }, { to: '/admin/webhooks', label: t('admin.webhooks'), icon: Webhook }, { to: '/admin/activity', label: t('admin.activity'), icon: Activity }]
  const active = items.find((item) => item.exact ? location.pathname === item.to : location.pathname.startsWith(item.to)) ?? items[0]
  const content = location.pathname.startsWith('/admin/installations') ? <InstallationsPage /> : location.pathname.startsWith('/admin/webhooks') ? <WebhooksPage /> : location.pathname.startsWith('/admin/activity') ? <ActivityPage /> : <OverviewPage />
  const signOut = async () => { await adminApi.logout(); window.location.reload() }
  return <div className="min-h-screen bg-muted/20 md:grid md:grid-cols-[248px_minmax(0,1fr)]"><PageTitle title={`${active.label} · ${t('admin.title')}`} /><aside className="hidden border-r bg-sidebar text-sidebar-foreground md:flex md:flex-col"><div className="flex h-14 items-center gap-2 border-b px-4"><ProductMark className="size-6" /><span className="font-semibold">{t('admin.title')}</span></div><nav className="flex-1 space-y-1 p-3">{items.map(({ to, label, icon: Icon, exact }) => <NavLink key={to} to={to} end={exact} className={({ isActive }) => `flex h-9 items-center gap-2 rounded-lg px-3 text-sm ${isActive ? 'bg-sidebar-accent font-medium text-sidebar-accent-foreground' : 'text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground'}`}><Icon className="size-4" />{label}</NavLink>)}</nav><div className="border-t p-3"><Button variant="ghost" className="w-full justify-start" onClick={signOut}><LogOut />{t('app.signOut')}</Button></div></aside><div className="min-w-0"><header className="sticky top-0 z-20 flex h-14 items-center justify-between border-b bg-background/95 px-4 backdrop-blur md:px-6"><div className="flex items-center gap-2 text-sm"><ProductMark className="size-5" /><span className="text-muted-foreground">{t('admin.instanceAdministration')}</span><span className="text-border">/</span><span className="font-medium">{active.label}</span></div><div className="flex items-center gap-1"><ProductMenu /><Sheet><SheetTrigger className="md:hidden" render={<Button variant="ghost" size="icon-sm" aria-label={t('app.openNavigation')} />}><Menu /></SheetTrigger><SheetContent side="left" className="w-[min(20rem,88vw)]"><SheetHeader><SheetTitle className="flex items-center gap-2"><ProductMark />{t('admin.title')}</SheetTitle></SheetHeader><nav className="space-y-1 px-4">{items.map(({ to, label, icon: Icon, exact }) => <SheetClose key={to} render={<NavLink to={to} end={exact} className={({ isActive }) => `flex h-11 items-center gap-3 rounded-lg px-3 ${isActive ? 'bg-muted font-medium' : 'text-muted-foreground'}`} />}><Icon className="size-4" />{label}</SheetClose>)}</nav><div className="mt-auto p-4"><Button variant="outline" className="w-full justify-start" onClick={signOut}><LogOut />{t('app.signOut')}</Button></div></SheetContent></Sheet></div></header><main className="mx-auto w-full max-w-[1680px] p-4 md:p-8">{content}</main></div></div>
}

export function AdminPage() {
  const { t } = useTranslation(); const [auth, setAuth] = useState<'checking' | 'authenticated' | 'anonymous'>('checking')
  useEffect(() => { adminApi.session().then(() => setAuth('authenticated')).catch((error) => { if (error instanceof ApiError && error.status === 404) window.location.href = '/'; else setAuth('anonymous') }) }, [])
  if (auth === 'checking') return <div className="flex min-h-screen items-center justify-center bg-background text-sm text-muted-foreground">{t('admin.checkingSession')}</div>
  if (auth === 'anonymous') return <AdminLogin onAuthenticated={() => setAuth('authenticated')} />
  return <AdminShell />
}
