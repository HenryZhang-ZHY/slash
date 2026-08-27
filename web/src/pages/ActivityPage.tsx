import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ExternalLink, RefreshCw } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useOutletContext, useSearchParams } from 'react-router-dom'

import { StatePanel } from '@/components/StatePanel'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { invocationDurationMs, invocationOutcome } from '@/lib/activity'
import {
  activityApi,
  ApiError,
  type ActivityInstallation,
  type ActivityRepository,
  type CommandInvocation,
  type CursorPage,
  type InvocationStatus,
} from '@/lib/api'
import type { DashboardContext } from '@/components/AppShell'
import { requestErrorKey } from '@/lib/requestError'

const HISTORY_PAGE_SIZE = 50

function GithubIcon({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M12 0c-6.626 0-12 5.373-12 12 0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576 4.765-1.589 8.199-6.086 8.199-11.386 0-6.627-5.373-12-12-12z" />
    </svg>
  )
}

const FILTER_STATUSES: InvocationStatus[] = [
  'claimed',
  'dispatched',
  'correlated',
  'completed',
  'aborted',
  'dispatch_failed',
  'correlation_timeout',
  'superseded',
]

async function loadAll<T>(fetchPage: (cursor: string | null) => Promise<CursorPage<T>>) {
  const items: T[] = []
  const seen = new Set<string>()
  let cursor: string | null = null
  let pageCount = 0
  do {
    if (pageCount++ >= 100) throw new Error('GitHub repository inventory exceeds the supported page limit')
    const page = await fetchPage(cursor)
    items.push(...page.items)
    cursor = page.next_cursor
    if (cursor && seen.has(cursor)) throw new Error('GitHub returned a repeated page cursor')
    if (cursor) seen.add(cursor)
  } while (cursor)
  return items
}

function formatTime(value: string, locale: string) {
  return new Intl.DateTimeFormat(locale, {
    dateStyle: 'medium',
    timeStyle: 'short',
  }).format(new Date(value))
}

function formatDuration(milliseconds: number) {
  if (milliseconds < 1_000) return `${milliseconds} ms`
  if (milliseconds < 60_000) return `${(milliseconds / 1_000).toFixed(1)} s`
  const minutes = Math.floor(milliseconds / 60_000)
  const seconds = Math.floor((milliseconds % 60_000) / 1_000)
  return `${minutes}m ${seconds}s`
}

function outcomeVariant(outcome: string) {
  if (outcome === 'success') return 'secondary' as const
  if (['failure', 'cancelled', 'aborted', 'dispatch_failed', 'correlation_timeout'].includes(outcome)) {
    return 'destructive' as const
  }
  return 'outline' as const
}

export function ActivityPage() {
  const { me } = useOutletContext<DashboardContext>()
  const { t, i18n } = useTranslation()
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const initialSelection = useRef({
    installationId: searchParams.get('installation_id'),
    repositoryId: searchParams.get('repository_id'),
  })
  const currentSelection = useRef(initialSelection.current)
  const historyRequest = useRef(0)
  const [installations, setInstallations] = useState<ActivityInstallation[] | null>(null)
  const [repositories, setRepositories] = useState<ActivityRepository[] | null>(null)
  const [installationId, setInstallationId] = useState(searchParams.get('installation_id') ?? '')
  const [repositoryId, setRepositoryId] = useState(searchParams.get('repository_id') ?? '')
  const [status, setStatus] = useState<InvocationStatus | ''>('')
  const [commandInput, setCommandInput] = useState('')
  const [command, setCommand] = useState('')
  const [invocations, setInvocations] = useState<CommandInvocation[] | null>(null)
  const [nextCursor, setNextCursor] = useState<string | null>(null)
  const [loadingMore, setLoadingMore] = useState(false)
  const [refreshKey, setRefreshKey] = useState(0)
  const [discoveryError, setDiscoveryError] = useState<ApiError | Error | null>(null)
  const [historyError, setHistoryError] = useState<ApiError | Error | null>(null)

  const selectedRepository = useMemo(
    () => repositories?.find((repository) => repository.id === repositoryId) ?? null,
    [repositories, repositoryId],
  )

  const replaceSelection = useCallback((nextInstallationId: string, repository?: ActivityRepository) => {
    const next = new URLSearchParams()
    if (nextInstallationId) next.set('installation_id', nextInstallationId)
    if (repository) {
      next.set('repository_id', repository.id)
      next.set('owner', repository.owner)
      next.set('repo', repository.name)
    }
    setSearchParams((current) => current.toString() === next.toString() ? current : next, { replace: true })
  }, [setSearchParams])

  useEffect(() => {
    if (!me.connections.github) return
    let active = true
    setInstallations(null)
    setRepositories(null)
    setDiscoveryError(null)
    loadAll((cursor) => activityApi.listInstallations(cursor, 100))
      .then((items) => {
        if (!active) return
        setInstallations(items)
        const requested = currentSelection.current.installationId
        const nextId = items.some((item) => item.id === requested) ? requested! : (items[0]?.id ?? '')
        setInstallationId(nextId)
        if (!nextId) replaceSelection('')
      })
      .catch((error: unknown) => {
        if (!active) return
        setInstallations([])
        setDiscoveryError(error instanceof Error ? error : new Error(t('activity.loadFailed')))
      })
    return () => { active = false }
    // refreshKey intentionally makes the complete GitHub inventory reload.
  }, [me.connections.github, refreshKey, replaceSelection, t])

  useEffect(() => {
    if (!installationId) {
      setRepositories([])
      setRepositoryId('')
      return
    }
    let active = true
    setRepositories(null)
    setRepositoryId('')
    setDiscoveryError(null)
    loadAll((cursor) => activityApi.listRepositories(installationId, cursor, 100))
      .then((items) => {
        if (!active) return
        setRepositories(items)
        const requested = currentSelection.current.repositoryId
        const repository = items.find((item) => item.id === requested) ?? items[0]
        setRepositoryId(repository?.id ?? '')
        currentSelection.current = {
          installationId,
          repositoryId: repository?.id ?? null,
        }
        replaceSelection(installationId, repository)
      })
      .catch((error: unknown) => {
        if (!active) return
        setRepositories([])
        setDiscoveryError(error instanceof Error ? error : new Error(t('activity.loadFailed')))
      })
    return () => { active = false }
  }, [installationId, replaceSelection, t])

  const loadHistory = useCallback(async (cursor: string | null, append: boolean) => {
    if (!selectedRepository || !installationId) return
    const requestId = ++historyRequest.current
    if (append) setLoadingMore(true)
    else setInvocations(null)
    setHistoryError(null)
    try {
      const page = await activityApi.listInvocations({
        installationId,
        repositoryId: selectedRepository.id,
        owner: selectedRepository.owner,
        repo: selectedRepository.name,
        status: status || undefined,
        command: command || undefined,
        cursor,
        limit: HISTORY_PAGE_SIZE,
      })
      if (requestId !== historyRequest.current) return
      setInvocations((current) => append ? [...(current ?? []), ...page.items] : page.items)
      setNextCursor(page.next_cursor)
    } catch (error) {
      if (requestId !== historyRequest.current) return
      setHistoryError(error instanceof Error ? error : new Error(t('activity.loadFailed')))
      if (!append) setInvocations([])
    } finally {
      if (requestId === historyRequest.current) setLoadingMore(false)
    }
  }, [command, installationId, selectedRepository, status, t])

  useEffect(() => {
    void loadHistory(null, false)
  }, [loadHistory])

  const chooseInstallation = (nextId: string) => {
    currentSelection.current = { installationId: nextId, repositoryId: null }
    setInstallationId(nextId)
    setRepositoryId('')
    replaceSelection(nextId)
  }

  const chooseRepository = (nextId: string) => {
    const repository = repositories?.find((item) => item.id === nextId)
    setRepositoryId(nextId)
    currentSelection.current = { installationId, repositoryId: nextId }
    if (repository) replaceSelection(installationId, repository)
  }

  if (!me.connections.github) {
    return (
      <div className="mx-auto w-full max-w-6xl px-4 py-8 md:px-8">
        <StatePanel
          title={t('activity.githubRequired')}
          description={t('activity.githubRequiredDescription')}
        />
        <div className="mt-4 flex justify-center">
          <Button onClick={() => navigate('/settings')}><GithubIcon />{t('activity.openSettings')}</Button>
        </div>
      </div>
    )
  }

  const needsAuthorization = discoveryError instanceof ApiError && discoveryError.status === 403

  return (
    <div className="mx-auto w-full max-w-[1680px] px-4 py-6 md:px-8 md:py-8">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold">{t('activity.title')}</h1>
          <p className="mt-1 text-sm text-muted-foreground">{t('activity.subtitle')}</p>
        </div>
        <Button variant="outline" onClick={() => setRefreshKey((key) => key + 1)}>
          <RefreshCw />{t('activity.refresh')}
        </Button>
      </div>

      {needsAuthorization ? (
        <div className="mt-8">
          <StatePanel
            kind="error"
            title={t('activity.authorizationRequired')}
            description={t('activity.authorizationRequiredDescription')}
          />
          <form className="mt-4 flex justify-center" method="POST" action="/api/auth/github/repository-access">
            <Button type="submit"><GithubIcon />{t('activity.authorizeGitHub')}</Button>
          </form>
        </div>
      ) : discoveryError ? (
        <div className="mt-8">
          <StatePanel kind="error" title={t('activity.discoveryUnavailable')} description={t(requestErrorKey(discoveryError, 'activity.loadFailed'))} retry={() => setRefreshKey((key) => key + 1)} />
        </div>
      ) : (
        <>
          <section className="mt-8 grid gap-4 border-y py-5 md:grid-cols-2 xl:grid-cols-[minmax(220px,0.7fr)_minmax(280px,1fr)_180px_minmax(180px,0.6fr)_auto] xl:items-end">
            <label className="grid gap-1.5 text-sm">
              <span className="text-xs font-medium text-muted-foreground">{t('activity.installation')}</span>
              <select className="h-9 rounded-md border bg-background px-3" value={installationId} onChange={(event) => chooseInstallation(event.target.value)} disabled={!installations?.length}>
                {!installations ? <option>{t('common.loading')}</option> : null}
                {installations?.map((installation) => <option key={installation.id} value={installation.id}>{installation.account} · {installation.target_type}</option>)}
              </select>
            </label>
            <label className="grid gap-1.5 text-sm">
              <span className="text-xs font-medium text-muted-foreground">{t('activity.repository')}</span>
              <select className="h-9 rounded-md border bg-background px-3" value={repositoryId} onChange={(event) => chooseRepository(event.target.value)} disabled={!repositories?.length}>
                {!repositories ? <option>{t('common.loading')}</option> : null}
                {repositories?.map((repository) => <option key={repository.id} value={repository.id}>{repository.full_name}{repository.private ? ` · ${t('activity.private')}` : ''}</option>)}
              </select>
            </label>
            <label className="grid gap-1.5 text-sm">
              <span className="text-xs font-medium text-muted-foreground">{t('activity.status')}</span>
              <select className="h-9 rounded-md border bg-background px-3" value={status} onChange={(event) => setStatus(event.target.value as InvocationStatus | '')}>
                <option value="">{t('activity.allStatuses')}</option>
                {FILTER_STATUSES.map((value) => <option key={value} value={value}>{t(`activity.status.${value}`)}</option>)}
              </select>
            </label>
            <label className="grid gap-1.5 text-sm">
              <span className="text-xs font-medium text-muted-foreground">{t('activity.command')}</span>
              <Input value={commandInput} onChange={(event) => setCommandInput(event.target.value)} placeholder={t('activity.commandPlaceholder')} />
            </label>
            <Button onClick={() => setCommand(commandInput.trim())}>{t('activity.apply')}</Button>
          </section>

          {!installations || !repositories ? (
            <div className="py-12 text-center text-sm text-muted-foreground">{t('activity.loadingRepositories')}</div>
          ) : installations.length === 0 ? (
            <div className="mt-8"><StatePanel title={t('activity.noInstallations')} description={t('activity.noInstallationsDescription')} /></div>
          ) : repositories?.length === 0 ? (
            <div className="mt-8"><StatePanel title={t('activity.noRepositories')} description={t('activity.noRepositoriesDescription')} /></div>
          ) : historyError ? (
            <div className="mt-8">
              <StatePanel
                kind="error"
                title={historyError instanceof ApiError && historyError.status === 404 ? t('activity.accessRemoved') : t('activity.historyUnavailable')}
                description={historyError instanceof ApiError && historyError.status === 404 ? t('activity.accessRemovedDescription') : t(requestErrorKey(historyError, 'activity.loadFailed'))}
                retry={() => void loadHistory(null, false)}
              />
            </div>
          ) : invocations === null ? (
            <div className="py-12 text-center text-sm text-muted-foreground">{t('activity.loadingHistory')}</div>
          ) : invocations.length === 0 ? (
            <div className="mt-8"><StatePanel title={t('activity.noActivity')} description={t('activity.noActivityDescription')} /></div>
          ) : (
            <section className="mt-6 border">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>{t('activity.triggered')}</TableHead>
                    <TableHead>{t('activity.command')}</TableHead>
                    <TableHead>{t('activity.pullRequest')}</TableHead>
                    <TableHead>{t('activity.actor')}</TableHead>
                    <TableHead>{t('activity.outcome')}</TableHead>
                    <TableHead>{t('activity.duration')}</TableHead>
                    <TableHead className="text-right">GitHub</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {invocations.map((invocation) => {
                    const outcome = invocationOutcome(invocation)
                    const finishedAt = invocation.completed_at ?? new Date().toISOString()
                    return (
                      <TableRow key={invocation.id}>
                        <TableCell className="text-muted-foreground">{formatTime(invocation.created_at, i18n.language)}</TableCell>
                        <TableCell><code className="rounded bg-muted px-1.5 py-0.5">/{invocation.command}</code>{invocation.attempt > 1 ? <span className="ml-2 text-xs text-muted-foreground">#{invocation.attempt}</span> : null}</TableCell>
                        <TableCell><a className="font-medium hover:underline" href={invocation.pull_url} target="_blank" rel="noreferrer">#{invocation.pr_number}</a></TableCell>
                        <TableCell>@{invocation.actor}</TableCell>
                        <TableCell><Badge variant={outcomeVariant(outcome)}>{t(`activity.status.${outcome}`)}</Badge></TableCell>
                        <TableCell className="tabular-nums text-muted-foreground">{formatDuration(invocationDurationMs(invocation.created_at, finishedAt))}</TableCell>
                        <TableCell className="text-right">
                          <a className="inline-flex items-center gap-1 text-xs text-primary hover:underline" href={invocation.check_url ?? invocation.workflow_run_url ?? invocation.comment_url} target="_blank" rel="noreferrer">
                            {t('activity.open')}<ExternalLink className="size-3" />
                          </a>
                        </TableCell>
                      </TableRow>
                    )
                  })}
                </TableBody>
              </Table>
              {nextCursor ? <div className="flex justify-center border-t p-4"><Button variant="outline" disabled={loadingMore} onClick={() => void loadHistory(nextCursor, true)}>{loadingMore ? t('activity.loadingMore') : t('activity.loadMore')}</Button></div> : null}
            </section>
          )}
        </>
      )}
    </div>
  )
}
