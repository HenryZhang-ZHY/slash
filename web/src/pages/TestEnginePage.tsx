import { useDeferredValue, useEffect, useState } from 'react'
import {
  Activity,
  Check,
  ChevronRight,
  CircleAlert,
  Copy,
  Eye,
  EyeOff,
  FileCode2,
  Filter,
  KeyRound,
  ListChecks,
  Plus,
  RefreshCw,
  Search,
  Settings2,
  X,
} from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  testEngineApi,
  type TestExecution,
  type TestExecutionPage,
  type TestSuiteSummary,
  type TestSummary,
} from '@/lib/api'

type CaseFilter = 'all' | 'failed' | 'passing' | 'muted' | 'skipped'
type CaseSort = 'recent' | 'name' | 'slowest' | 'failures'
type ManagementPanel = 'create' | 'settings' | null

const FILTERS: Array<{ value: CaseFilter; label: string }> = [
  { value: 'all', label: 'All' },
  { value: 'failed', label: 'Failing' },
  { value: 'passing', label: 'Passing' },
  { value: 'muted', label: 'Muted' },
  { value: 'skipped', label: 'Skipped' },
]

const STATUS_TONE: Record<string, string> = {
  passed: 'bg-emerald-50 text-emerald-700 ring-emerald-200',
  failed: 'bg-red-50 text-red-700 ring-red-200',
  errored: 'bg-red-50 text-red-700 ring-red-200',
  skipped: 'bg-zinc-100 text-zinc-600 ring-zinc-200',
  enabled: 'bg-emerald-50 text-emerald-700 ring-emerald-200',
  muted: 'bg-amber-50 text-amber-700 ring-amber-200',
}

function formatDuration(durationMs: number | null) {
  if (durationMs === null) return '—'
  if (durationMs < 1) return '<1 ms'
  if (durationMs < 1000) return `${Math.round(durationMs)} ms`
  if (durationMs < 60_000) return `${(durationMs / 1000).toFixed(2)} s`
  return `${(durationMs / 60_000).toFixed(1)} min`
}

function formatDate(value: string | null) {
  if (!value) return 'Never'
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  }).format(new Date(value))
}

function percentage(part: number, total: number) {
  return total === 0 ? 0 : Math.round((part / total) * 1000) / 10
}

function StatusBadge({ value }: { value: string | null }) {
  const status = value ?? 'unknown'
  return (
    <span
      className={`inline-flex items-center rounded-sm px-1.5 py-0.5 text-[11px] font-medium ring-1 ring-inset ${STATUS_TONE[status] ?? 'bg-zinc-100 text-zinc-600 ring-zinc-200'}`}
    >
      {status}
    </span>
  )
}

function Metric({ label, value, detail }: { label: string; value: string; detail?: string }) {
  return (
    <div className="min-w-0 px-4 py-4 first:pl-0 md:px-5 md:first:pl-0">
      <div className="text-[11px] font-medium text-muted-foreground uppercase">{label}</div>
      <div className="mt-1.5 flex items-baseline gap-2">
        <span className="text-xl font-semibold tabular-nums">{value}</span>
        {detail && <span className="truncate text-xs text-muted-foreground">{detail}</span>}
      </div>
    </div>
  )
}

function MetadataRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[104px_minmax(0,1fr)] gap-3 py-2 text-xs">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0 break-words text-foreground">{children}</dd>
    </div>
  )
}

function ExecutionRow({ execution }: { execution: TestExecution }) {
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
        <span>Started {formatDate(execution.started_at)}</span>
        {execution.finished_at && <span>Finished {formatDate(execution.finished_at)}</span>}
        {execution.invocation_id && (
          <span className="font-mono">Invocation {execution.invocation_id.slice(0, 8)}</span>
        )}
      </div>
      {execution.stack && (
        <details className="mt-3 border bg-zinc-950 text-zinc-100">
          <summary className="cursor-pointer px-3 py-2 text-xs text-zinc-300">
            Failure output
          </summary>
          <pre className="max-h-56 overflow-auto border-t border-zinc-800 p-3 text-[11px] leading-relaxed whitespace-pre-wrap">
            {execution.stack}
          </pre>
        </details>
      )}
    </div>
  )
}

function ManagementDialog({
  mode,
  suite,
  onClose,
  onCreated,
}: {
  mode: Exclude<ManagementPanel, null>
  suite: TestSuiteSummary | null
  onClose: () => void
  onCreated: (suite: TestSuiteSummary) => void
}) {
  const [owner, setOwner] = useState(suite?.owner ?? '')
  const [repo, setRepo] = useState(suite?.repo ?? '')
  const [suiteKey, setSuiteKey] = useState('')
  const [token, setToken] = useState<string | null>(null)
  const [tokenVisible, setTokenVisible] = useState(false)
  const [copied, setCopied] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (mode !== 'settings' || !suite) return
    testEngineApi
      .getToken(suite.id)
      .then((response) => setToken(response.token))
      .catch((requestError) =>
        setError(requestError instanceof Error ? requestError.message : 'Token 加载失败'),
      )
  }, [mode, suite])

  const createSuite = async (event: React.FormEvent) => {
    event.preventDefault()
    setBusy(true)
    setError(null)
    try {
      const result = await testEngineApi.createSuite(owner, repo, suiteKey)
      onCreated(result.suite)
      onClose()
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : 'Suite 创建失败')
    } finally {
      setBusy(false)
    }
  }

  const issueToken = async () => {
    if (!suite) return
    setBusy(true)
    setError(null)
    try {
      const result = await testEngineApi.issueToken(suite.id)
      setToken(result.token)
      setTokenVisible(false)
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : 'Token 生成失败')
    } finally {
      setBusy(false)
    }
  }

  const copyToken = async () => {
    if (!token) return
    await navigator.clipboard.writeText(token)
    setCopied(true)
    window.setTimeout(() => setCopied(false), 1500)
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-end bg-black/20"
      role="presentation"
      onMouseDown={onClose}
    >
      <section
        role="dialog"
        aria-modal="true"
        aria-label={mode === 'create' ? 'Create test suite' : 'Suite settings'}
        className="h-full w-full max-w-lg overflow-y-auto border-l bg-white shadow-2xl"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="flex h-14 items-center justify-between border-b px-5">
          <div>
            <div className="text-sm font-semibold">
              {mode === 'create' ? 'Create suite' : 'Suite settings'}
            </div>
            {suite && (
              <div className="text-xs text-muted-foreground">
                {suite.owner}/{suite.repo} · {suite.suite_key}
              </div>
            )}
          </div>
          <Button size="icon" variant="ghost" onClick={onClose} aria-label="Close">
            <X />
          </Button>
        </div>

        {mode === 'create' ? (
          <form onSubmit={createSuite} className="space-y-5 p-5">
            <div className="space-y-1.5">
              <Label htmlFor="create-owner">GitHub owner</Label>
              <Input
                id="create-owner"
                value={owner}
                onChange={(event) => setOwner(event.target.value)}
                placeholder="acme"
                required
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="create-repo">Repository</Label>
              <Input
                id="create-repo"
                value={repo}
                onChange={(event) => setRepo(event.target.value)}
                placeholder="widgets"
                required
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="create-key">Suite key</Label>
              <Input
                id="create-key"
                value={suiteKey}
                onChange={(event) => setSuiteKey(event.target.value)}
                placeholder="ci-test"
                required
              />
            </div>
            {error && <p className="text-sm text-red-600">{error}</p>}
            <div className="flex justify-end gap-2 border-t pt-4">
              <Button type="button" variant="ghost" onClick={onClose}>
                Cancel
              </Button>
              <Button type="submit" disabled={busy}>
                <Plus />
                {busy ? 'Creating…' : 'Create suite'}
              </Button>
            </div>
          </form>
        ) : (
          <div className="p-5">
            <div className="border-b pb-6">
              <h3 className="text-sm font-semibold">Collection token</h3>
              <p className="mt-1 text-xs text-muted-foreground">
                Used by CI collectors to authenticate uploads to this suite.
              </p>
              <div className="mt-4 flex items-center gap-2">
                <Input
                  className="font-mono"
                  type={tokenVisible ? 'text' : 'password'}
                  value={token ?? ''}
                  placeholder="No recoverable token"
                  readOnly
                />
                <Button
                  size="icon"
                  variant="outline"
                  onClick={() => setTokenVisible((visible) => !visible)}
                  disabled={!token}
                  aria-label={tokenVisible ? 'Hide token' : 'Show token'}
                >
                  {tokenVisible ? <EyeOff /> : <Eye />}
                </Button>
                <Button
                  size="icon"
                  variant="outline"
                  onClick={copyToken}
                  disabled={!token}
                  aria-label="Copy token"
                >
                  {copied ? <Check /> : <Copy />}
                </Button>
              </div>
              <Button className="mt-3" variant="outline" onClick={issueToken} disabled={busy}>
                <KeyRound />
                {busy ? 'Generating…' : 'Generate new token'}
              </Button>
            </div>
            <div className="pt-6">
              <h3 className="text-sm font-semibold">Collector endpoints</h3>
              <dl className="mt-3 divide-y border-y">
                <MetadataRow label="Generic">
                  <code>/v1/test-engine/upload</code>
                </MetadataRow>
                <MetadataRow label="Cargo">
                  <code>/v1/test-engine/upload/cargo</code>
                </MetadataRow>
                <MetadataRow label="Vitest">
                  <code>/v1/test-engine/upload/vitest</code>
                </MetadataRow>
              </dl>
            </div>
            {error && <p className="mt-4 text-sm text-red-600">{error}</p>}
          </div>
        )}
      </section>
    </div>
  )
}

export function TestEnginePage() {
  const [suites, setSuites] = useState<TestSuiteSummary[]>([])
  const [selectedSuiteId, setSelectedSuiteId] = useState<string | null>(null)
  const [tests, setTests] = useState<TestSummary[] | null>(null)
  const [selectedTestId, setSelectedTestId] = useState<string | null>(null)
  const [executions, setExecutions] = useState<TestExecutionPage | null>(null)
  const [executionItems, setExecutionItems] = useState<TestExecution[]>([])
  const [query, setQuery] = useState('')
  const deferredQuery = useDeferredValue(query)
  const [filter, setFilter] = useState<CaseFilter>('all')
  const [sort, setSort] = useState<CaseSort>('recent')
  const [compactView, setCompactView] = useState<'cases' | 'details'>('cases')
  const [loadingSuites, setLoadingSuites] = useState(true)
  const [loadingTests, setLoadingTests] = useState(false)
  const [loadingExecutions, setLoadingExecutions] = useState(false)
  const [updatingState, setUpdatingState] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [panel, setPanel] = useState<ManagementPanel>(null)

  const selectedSuite = suites.find((suite) => suite.id === selectedSuiteId) ?? null
  const selectedTest = tests?.find((test) => test.id === selectedTestId) ?? null

  const loadSuites = async () => {
    setLoadingSuites(true)
    setError(null)
    try {
      const result = await testEngineApi.listSuites()
      setSuites(result)
      setSelectedSuiteId((current) =>
        current && result.some((suite) => suite.id === current)
          ? current
          : (result[0]?.id ?? null),
      )
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : 'Suite 加载失败')
    } finally {
      setLoadingSuites(false)
    }
  }

  const loadTests = async (suiteId: string) => {
    setLoadingTests(true)
    setError(null)
    try {
      const result = await testEngineApi.listTests(suiteId)
      setTests(result)
      setSelectedTestId((current) =>
        current && result.some((test) => test.id === current)
          ? current
          : (result.find(
              (test) => test.last_status === 'failed' || test.last_status === 'errored',
            )?.id ??
            result[0]?.id ??
            null),
      )
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : 'Test cases 加载失败')
    } finally {
      setLoadingTests(false)
    }
  }

  const loadExecutions = async (testId: string, offset = 0) => {
    setLoadingExecutions(true)
    try {
      const page = await testEngineApi.listExecutions(testId, 100, offset)
      setExecutions(page)
      setExecutionItems((current) => (offset === 0 ? page.items : [...current, ...page.items]))
    } catch (requestError) {
      setError(
        requestError instanceof Error ? requestError.message : 'Execution history 加载失败',
      )
    } finally {
      setLoadingExecutions(false)
    }
  }

  useEffect(() => {
    void loadSuites()
  }, [])

  useEffect(() => {
    if (!selectedSuiteId) {
      setTests([])
      return
    }
    setTests(null)
    setSelectedTestId(null)
    void loadTests(selectedSuiteId)
  }, [selectedSuiteId])

  useEffect(() => {
    setExecutions(null)
    setExecutionItems([])
    if (selectedTestId) void loadExecutions(selectedTestId)
  }, [selectedTestId])

  const normalizedQuery = deferredQuery.trim().toLowerCase()
  const filteredTests = (tests ?? [])
    .filter((test) => {
      if (
        normalizedQuery &&
        !`${test.name} ${test.file ?? ''} ${test.labels.join(' ')}`
          .toLowerCase()
          .includes(normalizedQuery)
      ) {
        return false
      }
      if (filter === 'failed') return test.last_status === 'failed' || test.last_status === 'errored'
      if (filter === 'passing') return test.last_status === 'passed'
      if (filter === 'muted') return test.state === 'muted'
      if (filter === 'skipped') {
        return test.state === 'skipped' || test.last_status === 'skipped'
      }
      return true
    })
    .sort((left, right) => {
      if (sort === 'name') return left.name.localeCompare(right.name)
      if (sort === 'slowest') {
        return (right.average_duration_ms ?? 0) - (left.average_duration_ms ?? 0)
      }
      if (sort === 'failures') return right.failed_count - left.failed_count
      return new Date(right.last_captured ?? 0).getTime() - new Date(left.last_captured ?? 0).getTime()
    })

  const createSuite = (suite: TestSuiteSummary) => {
    setSuites((current) => [suite, ...current.filter((item) => item.id !== suite.id)])
    setSelectedSuiteId(suite.id)
  }

  const updateTestState = async (state: TestSummary['state']) => {
    if (!selectedTest) return
    setUpdatingState(true)
    setError(null)
    try {
      await testEngineApi.setTestState(selectedTest.id, state)
      setTests((current) =>
        current?.map((test) => (test.id === selectedTest.id ? { ...test, state } : test)) ?? null,
      )
      if (selectedSuiteId) {
        setSuites((current) =>
          current.map((suite) => {
            if (suite.id !== selectedSuiteId) return suite
            const previous = selectedTest.state
            return {
              ...suite,
              muted: suite.muted + (state === 'muted' ? 1 : 0) - (previous === 'muted' ? 1 : 0),
              skipped:
                suite.skipped + (state === 'skipped' ? 1 : 0) - (previous === 'skipped' ? 1 : 0),
            }
          }),
        )
      }
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : 'Disposition 更新失败')
    } finally {
      setUpdatingState(false)
    }
  }

  const suitePassRate = selectedSuite
    ? percentage(selectedSuite.passed_executions, selectedSuite.execution_count)
    : 0
  const casePassRate = selectedTest
    ? percentage(selectedTest.passed_count, selectedTest.execution_count)
    : 0
  const hasMoreExecutions = executions ? executionItems.length < executions.total : false

  return (
    <div className="flex min-h-[calc(100vh-3.5rem)] flex-col bg-white xl:h-[calc(100vh-3.5rem)] xl:min-h-0 xl:overflow-hidden">
      <div className="border-b px-4 py-5 md:px-6 xl:px-8">
        <div className="flex flex-wrap items-end justify-between gap-4">
          <div>
            <div className="text-xs text-muted-foreground">Quality / Test Engine</div>
            <h1 className="mt-1 text-2xl font-semibold">Test intelligence</h1>
            <p className="mt-1 text-sm text-muted-foreground">
              Cases, flaky disposition, metadata and execution history across CI providers.
            </p>
          </div>
          <div className="flex items-center gap-2">
            <Button variant="outline" onClick={() => void loadSuites()} disabled={loadingSuites}>
              <RefreshCw className={loadingSuites ? 'animate-spin' : ''} />
              Refresh
            </Button>
            <Button
              variant="outline"
              onClick={() => setPanel('settings')}
              disabled={!selectedSuite}
            >
              <Settings2 />
              Settings
            </Button>
            <Button onClick={() => setPanel('create')}>
              <Plus />
              New suite
            </Button>
          </div>
        </div>

        <div className="mt-6 grid grid-cols-2 divide-x border-y md:grid-cols-3 xl:grid-cols-6">
          <Metric label="Test cases" value={(selectedSuite?.total_tests ?? 0).toLocaleString()} />
          <Metric
            label="Executions"
            value={(selectedSuite?.execution_count ?? 0).toLocaleString()}
            detail={`${selectedSuite?.run_count ?? 0} runs`}
          />
          <Metric label="Pass rate" value={`${suitePassRate}%`} />
          <Metric
            label="Failures"
            value={(selectedSuite?.failed_executions ?? 0).toLocaleString()}
          />
          <Metric label="Muted" value={(selectedSuite?.muted ?? 0).toLocaleString()} />
          <Metric
            label="Avg duration"
            value={formatDuration(selectedSuite?.average_duration_ms ?? null)}
          />
        </div>
        <div className="mt-4 grid grid-cols-2 border bg-[#fafafa] p-0.5 xl:hidden">
          {(['cases', 'details'] as const).map((view) => (
            <button
              key={view}
              type="button"
              onClick={() => setCompactView(view)}
              className={`h-8 text-xs capitalize ${
                compactView === view
                  ? 'bg-white font-medium shadow-sm'
                  : 'text-muted-foreground'
              }`}
            >
              {view}
            </button>
          ))}
        </div>
      </div>

      {error && (
        <div className="flex items-center gap-2 border-b bg-red-50 px-4 py-2 text-xs text-red-700 md:px-6 xl:px-8">
          <CircleAlert className="size-4" />
          {error}
        </div>
      )}

      <div className="grid flex-1 grid-cols-1 xl:min-h-0 xl:grid-cols-[180px_minmax(430px,1fr)_360px] 2xl:grid-cols-[210px_minmax(520px,1fr)_420px]">
        <aside className="border-b bg-[#fafafa] xl:min-h-0 xl:border-r xl:border-b-0">
          <div className="flex items-center justify-between px-3 py-3 xl:border-b">
            <span className="text-[11px] font-medium text-muted-foreground uppercase">Suites</span>
            <span className="text-[11px] text-muted-foreground">{suites.length}</span>
          </div>
          <div className="flex overflow-x-auto p-2 xl:block xl:max-h-[calc(100vh-19rem)] xl:overflow-y-auto">
            {loadingSuites ? (
              <div className="px-2 py-4 text-xs text-muted-foreground">Loading suites…</div>
            ) : suites.length === 0 ? (
              <button
                type="button"
                onClick={() => setPanel('create')}
                className="w-full border border-dashed p-4 text-left text-xs text-muted-foreground"
              >
                Create your first suite
              </button>
            ) : (
              suites.map((suite) => (
                <button
                  key={suite.id}
                  type="button"
                  onClick={() => setSelectedSuiteId(suite.id)}
                  className={`mr-2 min-w-48 border px-3 py-2.5 text-left transition-colors xl:mr-0 xl:mb-1 xl:w-full xl:min-w-0 ${
                    suite.id === selectedSuiteId
                      ? 'border-zinc-300 bg-white shadow-sm'
                      : 'border-transparent hover:bg-white'
                  }`}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="truncate text-sm font-medium">{suite.suite_key}</span>
                    <ChevronRight className="size-3.5 text-muted-foreground" />
                  </div>
                  <div className="mt-1 truncate text-[11px] text-muted-foreground">
                    {suite.owner}/{suite.repo}
                  </div>
                  <div className="mt-2 flex gap-3 text-[11px] text-muted-foreground">
                    <span>{suite.total_tests} cases</span>
                    <span>{suite.execution_count} execs</span>
                  </div>
                </button>
              ))
            )}
          </div>
        </aside>

        <section
          className={`min-w-0 border-b xl:flex xl:min-h-0 xl:flex-col xl:border-r xl:border-b-0 ${
            compactView === 'details' ? 'hidden xl:flex' : 'block'
          }`}
        >
          <div className="border-b px-4 py-3">
            <div className="flex flex-wrap items-center gap-2">
              <div className="relative min-w-56 flex-1">
                <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  className="pl-8"
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="Search test name, file or label…"
                />
              </div>
              <div className="flex h-8 items-center border bg-[#fafafa] p-0.5">
                <Filter className="mx-1.5 size-3.5 text-muted-foreground" />
                {FILTERS.map((item) => (
                  <button
                    key={item.value}
                    type="button"
                    onClick={() => setFilter(item.value)}
                    className={`h-6 px-2 text-xs ${
                      filter === item.value
                        ? 'bg-white font-medium shadow-sm'
                        : 'text-muted-foreground hover:text-foreground'
                    }`}
                  >
                    {item.label}
                  </button>
                ))}
              </div>
              <select
                value={sort}
                onChange={(event) => setSort(event.target.value as CaseSort)}
                className="h-8 border bg-white px-2 text-xs outline-none focus:border-ring"
              >
                <option value="recent">Recently seen</option>
                <option value="name">Name</option>
                <option value="slowest">Slowest</option>
                <option value="failures">Most failures</option>
              </select>
              {selectedSuite && (
                <Button
                  size="icon"
                  variant="ghost"
                  onClick={() => void loadTests(selectedSuite.id)}
                  title="Refresh cases"
                  aria-label="Refresh cases"
                >
                  <RefreshCw className={loadingTests ? 'animate-spin' : ''} />
                </Button>
              )}
            </div>
            <div className="mt-2 text-[11px] text-muted-foreground">
              Showing {filteredTests.length.toLocaleString()} of{' '}
              {(tests?.length ?? 0).toLocaleString()} cases
            </div>
          </div>

          <div className="max-h-[70vh] overflow-auto xl:max-h-none xl:min-h-0 xl:flex-1">
            <table className="w-full min-w-[820px] border-collapse text-left">
              <thead className="sticky top-0 z-10 bg-[#fafafa] text-[11px] text-muted-foreground">
                <tr className="border-b">
                  <th className="px-4 py-2.5 font-medium">Test case</th>
                  <th className="w-24 px-3 py-2.5 font-medium">Latest</th>
                  <th className="w-20 px-3 py-2.5 text-right font-medium">Execs</th>
                  <th className="w-24 px-3 py-2.5 text-right font-medium">Pass rate</th>
                  <th className="w-24 px-3 py-2.5 text-right font-medium">Avg time</th>
                  <th className="w-32 px-4 py-2.5 text-right font-medium">Last seen</th>
                </tr>
              </thead>
              <tbody>
                {loadingTests || tests === null ? (
                  <tr>
                    <td colSpan={6} className="px-4 py-12 text-center text-sm text-muted-foreground">
                      Loading test cases…
                    </td>
                  </tr>
                ) : filteredTests.length === 0 ? (
                  <tr>
                    <td colSpan={6} className="px-4 py-12 text-center text-sm text-muted-foreground">
                      No test cases match this view.
                    </td>
                  </tr>
                ) : (
                  filteredTests.map((test) => (
                    <tr
                      key={test.id}
                      onClick={() => {
                        setSelectedTestId(test.id)
                        setCompactView('details')
                      }}
                      className={`cursor-pointer border-b transition-colors ${
                        test.id === selectedTestId ? 'bg-zinc-100' : 'hover:bg-zinc-50'
                      }`}
                    >
                      <td className="px-4 py-3">
                        <div
                          className="max-w-xl truncate font-mono text-xs font-medium"
                          title={test.name}
                        >
                          {test.name}
                        </div>
                        <div className="mt-1 flex items-center gap-2 text-[11px] text-muted-foreground">
                          <span className="truncate">{test.file ?? 'No source location'}</span>
                          {test.state !== 'enabled' && <StatusBadge value={test.state} />}
                        </div>
                      </td>
                      <td className="px-3 py-3">
                        <StatusBadge value={test.last_status} />
                      </td>
                      <td className="px-3 py-3 text-right text-xs tabular-nums">
                        {test.execution_count}
                      </td>
                      <td className="px-3 py-3 text-right text-xs tabular-nums">
                        {percentage(test.passed_count, test.execution_count)}%
                      </td>
                      <td className="px-3 py-3 text-right text-xs tabular-nums">
                        {formatDuration(test.average_duration_ms)}
                      </td>
                      <td className="px-4 py-3 text-right text-[11px] text-muted-foreground">
                        {formatDate(test.last_captured)}
                      </td>
                    </tr>
                  ))
                )}
              </tbody>
            </table>
          </div>
        </section>

        <aside
          className={`min-w-0 bg-white xl:block xl:min-h-0 xl:overflow-y-auto ${
            compactView === 'cases' ? 'hidden' : 'block'
          }`}
        >
          {!selectedTest ? (
            <div className="flex min-h-64 flex-col items-center justify-center px-8 text-center text-sm text-muted-foreground xl:h-full">
              <ListChecks className="mb-3 size-7" />
              Select a test case to inspect metadata and execution history.
            </div>
          ) : (
            <div>
              <div className="border-b px-5 py-4">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="text-[11px] text-muted-foreground uppercase">Test case</div>
                    <h2 className="mt-1 break-words font-mono text-sm font-semibold leading-relaxed">
                      {selectedTest.name}
                    </h2>
                  </div>
                  <StatusBadge value={selectedTest.state} />
                </div>
                <div className="mt-4">
                  <div className="mb-1.5 text-[10px] font-medium text-muted-foreground uppercase">
                    Disposition
                  </div>
                  <div className="grid grid-cols-3 border bg-[#fafafa] p-0.5">
                    {(['enabled', 'muted', 'skipped'] as const).map((state) => (
                      <button
                        key={state}
                        type="button"
                        onClick={() => void updateTestState(state)}
                        disabled={updatingState}
                        className={`h-7 text-xs capitalize transition-colors ${
                          selectedTest.state === state
                            ? 'bg-white font-medium shadow-sm'
                            : 'text-muted-foreground hover:text-foreground'
                        }`}
                      >
                        {state}
                      </button>
                    ))}
                  </div>
                </div>
                <div className="mt-4 grid grid-cols-3 divide-x border-y">
                  <div className="py-3 pr-3">
                    <div className="text-[10px] text-muted-foreground uppercase">Pass rate</div>
                    <div className="mt-1 text-lg font-semibold tabular-nums">{casePassRate}%</div>
                  </div>
                  <div className="px-3 py-3">
                    <div className="text-[10px] text-muted-foreground uppercase">Executions</div>
                    <div className="mt-1 text-lg font-semibold tabular-nums">
                      {selectedTest.execution_count}
                    </div>
                  </div>
                  <div className="py-3 pl-3">
                    <div className="text-[10px] text-muted-foreground uppercase">Average</div>
                    <div className="mt-1 text-lg font-semibold tabular-nums">
                      {formatDuration(selectedTest.average_duration_ms)}
                    </div>
                  </div>
                </div>
                <div className="mt-3 flex h-1.5 overflow-hidden bg-zinc-100">
                  <div
                    className="bg-emerald-500"
                    style={{
                      width: `${percentage(selectedTest.passed_count, selectedTest.execution_count)}%`,
                    }}
                  />
                  <div
                    className="bg-red-500"
                    style={{
                      width: `${percentage(
                        selectedTest.failed_count + selectedTest.errored_count,
                        selectedTest.execution_count,
                      )}%`,
                    }}
                  />
                  <div
                    className="bg-zinc-400"
                    style={{
                      width: `${percentage(selectedTest.skipped_count, selectedTest.execution_count)}%`,
                    }}
                  />
                </div>
                <div className="mt-2 flex gap-4 text-[10px] text-muted-foreground">
                  <span>{selectedTest.passed_count} passed</span>
                  <span>{selectedTest.failed_count + selectedTest.errored_count} failed</span>
                  <span>{selectedTest.skipped_count} skipped</span>
                </div>
              </div>

              <section className="border-b px-5 py-4">
                <div className="mb-2 flex items-center gap-2">
                  <FileCode2 className="size-4" />
                  <h3 className="text-sm font-semibold">Metadata</h3>
                </div>
                <dl className="divide-y">
                  <MetadataRow label="Source">
                    {selectedTest.file ? (
                      <code>
                        {selectedTest.file}
                        {selectedTest.line_no ? `:${selectedTest.line_no}` : ''}
                      </code>
                    ) : (
                      'Not reported'
                    )}
                  </MetadataRow>
                  <MetadataRow label="Latest">
                    <div className="flex items-center gap-2">
                      <StatusBadge value={selectedTest.last_status} />
                      <span>{formatDate(selectedTest.last_captured)}</span>
                    </div>
                  </MetadataRow>
                  <MetadataRow label="Provider">
                    {selectedTest.last_ci_provider ?? 'Not reported'}
                  </MetadataRow>
                  <MetadataRow label="Run">
                    <span className="font-mono text-[11px]">
                      {selectedTest.last_run_ref ?? 'Not reported'}
                    </span>
                  </MetadataRow>
                  <MetadataRow label="Labels">
                    {selectedTest.labels.length ? (
                      <div className="flex flex-wrap gap-1">
                        {selectedTest.labels.map((label) => (
                          <span key={label} className="bg-zinc-100 px-1.5 py-0.5">
                            {label}
                          </span>
                        ))}
                      </div>
                    ) : (
                      'None'
                    )}
                  </MetadataRow>
                  <MetadataRow label="Owner teams">
                    {selectedTest.owner_team_ids.length
                      ? selectedTest.owner_team_ids.join(', ')
                      : 'None'}
                  </MetadataRow>
                  <MetadataRow label="First seen">
                    {formatDate(selectedTest.created_at)}
                  </MetadataRow>
                  <MetadataRow label="Updated">
                    {formatDate(selectedTest.updated_at)}
                  </MetadataRow>
                </dl>
              </section>

              <section className="max-h-[70vh] overflow-y-auto xl:max-h-none xl:overflow-visible">
                <div className="sticky top-0 z-10 flex items-center justify-between border-b bg-white px-5 py-3">
                  <div className="flex items-center gap-2">
                    <Activity className="size-4" />
                    <h3 className="text-sm font-semibold">Execution history</h3>
                  </div>
                  <span className="text-[11px] text-muted-foreground">
                    {executions?.total ?? selectedTest.execution_count} total
                  </span>
                </div>
                {loadingExecutions && executionItems.length === 0 ? (
                  <div className="px-5 py-8 text-center text-xs text-muted-foreground">
                    Loading executions…
                  </div>
                ) : executionItems.length === 0 ? (
                  <div className="px-5 py-8 text-center text-xs text-muted-foreground">
                    No execution history.
                  </div>
                ) : (
                  <div>
                    {executionItems.map((execution) => (
                      <ExecutionRow key={execution.id} execution={execution} />
                    ))}
                  </div>
                )}
                {hasMoreExecutions && (
                  <div className="p-4">
                    <Button
                      className="w-full"
                      variant="outline"
                      onClick={() =>
                        selectedTestId && void loadExecutions(selectedTestId, executionItems.length)
                      }
                      disabled={loadingExecutions}
                    >
                      {loadingExecutions
                        ? 'Loading…'
                        : `Load more (${executionItems.length} of ${executions?.total})`}
                    </Button>
                  </div>
                )}
              </section>
            </div>
          )}
        </aside>
      </div>

      {panel && (
        <ManagementDialog
          mode={panel}
          suite={selectedSuite}
          onClose={() => setPanel(null)}
          onCreated={createSuite}
        />
      )}
    </div>
  )
}
