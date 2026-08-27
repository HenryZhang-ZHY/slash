import { useDeferredValue, useEffect, useRef, useState } from 'react'
import {
  Activity,
  ArrowLeft,
  ChevronRight,
  CircleAlert,
  FileCode2,
  Filter,
  ListChecks,
  PlayCircle,
  Plus,
  RefreshCw,
  Search,
  Settings2,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useSearchParams } from 'react-router-dom'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { StatusBadge } from '@/components/test-engine/StatusBadge'
import { Metric } from '@/components/test-engine/Metric'
import { MetadataRow } from '@/components/test-engine/MetadataRow'
import { ExecutionRow } from '@/components/test-engine/ExecutionRow'
import { RunExecutionRow } from '@/components/test-engine/RunExecutionRow'
import {
  ManagementDialog,
  type ManagementPanel,
} from '@/components/test-engine/ManagementDialog'
import { useTestEngine } from '@/hooks/useTestEngine'
import { useTestRuns } from '@/hooks/useTestRuns'
import { formatDate, formatDuration, percentage } from '@/lib/test-engine/format'
import { runStatus } from '@/lib/test-engine/runs'
import { testEngineLocation, testEngineSearch } from '@/lib/navigation'

type CaseFilter = 'all' | 'failed' | 'passing' | 'muted' | 'skipped'
type CaseSort = 'recent' | 'name' | 'slowest' | 'failures'

export function TestEnginePage() {
  const [searchParams, setSearchParams] = useSearchParams()
  const initialLocation = useRef(testEngineLocation(searchParams)).current
  const [query, setQuery] = useState('')
  const deferredQuery = useDeferredValue(query)
  const [filter, setFilter] = useState<CaseFilter>('all')
  const [sort, setSort] = useState<CaseSort>('recent')
  const [visibleCaseLimit, setVisibleCaseLimit] = useState(100)
  const [viewMode, setViewMode] = useState<'cases' | 'runs'>(initialLocation.view)
  const [compactView, setCompactView] = useState<'cases' | 'details'>(initialLocation.testId || initialLocation.runId ? 'details' : 'cases')
  const [panel, setPanel] = useState<ManagementPanel>(null)
  const { t } = useTranslation()

  const filterItems: Array<{ value: CaseFilter; label: string }> = [
    { value: 'all', label: t('testengine.filterAll') },
    { value: 'failed', label: t('testengine.filterFailed') },
    { value: 'passing', label: t('testengine.filterPassing') },
    { value: 'muted', label: t('testengine.filterMuted') },
    { value: 'skipped', label: t('testengine.filterSkipped') },
  ]

  const {
    suites,
    selectedSuiteId,
    setSelectedSuiteId,
    selectedSuite,
    tests,
    selectedTestId,
    setSelectedTestId,
    selectedTest,
    executions,
    executionItems,
    loadingSuites,
    loadingTests,
    loadingExecutions,
    updatingState,
    error,
    hasMoreExecutions,
    loadSuites,
    loadTests,
    loadExecutions,
    createSuite,
    updateTestState,
  } = useTestEngine(initialLocation.suiteId, initialLocation.testId)

  const {
    runs,
    runItems,
    selectedRunId,
    setSelectedRunId,
    selectedRun,
    executions: runExecutions,
    executionItems: runExecutionItems,
    loadingRuns,
    loadingExecutions: loadingRunExecutions,
    error: runError,
    hasMoreRuns,
    hasMoreExecutions: hasMoreRunExecutions,
    loadRuns,
    loadRunExecutions,
  } = useTestRuns(selectedSuiteId, viewMode === 'runs', initialLocation.runId)

  useEffect(() => {
    if (!selectedSuiteId) return
    const next = testEngineSearch({ suiteId: selectedSuiteId, view: viewMode, testId: selectedTestId, runId: selectedRunId })
    setSearchParams((current) => current.toString() === next.toString() ? current : next, { replace: true })
  }, [selectedRunId, selectedSuiteId, selectedTestId, setSearchParams, viewMode])

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
  const visibleTests = filteredTests.slice(0, visibleCaseLimit)

  useEffect(() => setVisibleCaseLimit(100), [deferredQuery, filter, selectedSuiteId, sort])

  const suitePassRate = selectedSuite
    ? percentage(selectedSuite.passed_executions, selectedSuite.execution_count)
    : 0
  const casePassRate = selectedTest
    ? percentage(selectedTest.passed_count, selectedTest.execution_count)
    : 0

  return (
    <div className="flex min-h-[calc(100vh-3.5rem)] flex-col bg-background xl:h-[calc(100vh-3.5rem)] xl:min-h-0 xl:overflow-hidden">
      <div className="border-b px-4 py-5 md:px-6 xl:px-8">
        <div className="flex flex-wrap items-end justify-between gap-4">
          <div>
            <div className="text-xs font-medium uppercase tracking-wide text-muted-foreground">{t('testengine.qualityLabel')}</div>
            <h1 className="mt-1 text-2xl font-semibold">{t('testengine.title')}</h1>
            <p className="mt-1 text-sm text-muted-foreground">
              {t('testengine.subtitle')}
            </p>
          </div>
          <div className="flex w-full flex-wrap items-center gap-2 sm:w-auto">
            <Button variant="outline" onClick={() => void loadSuites()} disabled={loadingSuites}>
              <RefreshCw className={loadingSuites ? 'animate-spin' : ''} />
              {t('testengine.refresh')}
            </Button>
            <Button
              variant="outline"
              onClick={() => setPanel('settings')}
              disabled={!selectedSuite}
            >
              <Settings2 />
              {t('testengine.collectionSetup')}
            </Button>
            <Button onClick={() => setPanel('create')}>
              <Plus />
              {t('testengine.newSuite')}
            </Button>
          </div>
        </div>

        {selectedSuite ? <div className="mt-6 flex flex-wrap items-center gap-x-2 gap-y-1 text-sm"><span className="font-medium">{selectedSuite.owner}/{selectedSuite.repo}</span><span className="text-muted-foreground">· {selectedSuite.suite_key}</span><span className="ml-auto text-xs text-muted-foreground">{t('testengine.allTimeMetrics')}</span></div> : null}
        <div className="mt-3 grid grid-cols-2 divide-x border-y md:grid-cols-3 xl:grid-cols-6">
          <Metric label={t('testengine.metricCases')} value={(selectedSuite?.total_tests ?? 0).toLocaleString()} />
          <Metric
            label={t('testengine.metricExecutions')}
            value={(selectedSuite?.execution_count ?? 0).toLocaleString()}
            detail={t('testengine.runs', { count: selectedSuite?.run_count ?? 0 })}
          />
          <Metric label={t('testengine.metricPassRate')} value={`${suitePassRate}%`} />
          <Metric
            label={t('testengine.metricFailures')}
            value={(selectedSuite?.failed_executions ?? 0).toLocaleString()}
          />
          <Metric label={t('testengine.metricMuted')} value={(selectedSuite?.muted ?? 0).toLocaleString()} />
          <Metric
            label={t('testengine.metricAvgDuration')}
            value={formatDuration(selectedSuite?.average_duration_ms ?? null)}
          />
        </div>
        <div className="mt-4 inline-grid grid-cols-2 rounded-lg border bg-muted/40 p-1">
          {(['cases', 'runs'] as const).map((mode) => (
            <button
              key={mode}
              type="button"
              onClick={() => {
                setViewMode(mode)
                setCompactView('cases')
              }}
              className={`h-8 min-w-28 px-3 text-xs transition-colors ${
                viewMode === mode
                  ? 'rounded-md bg-background font-medium shadow-sm'
                  : 'text-muted-foreground hover:text-foreground'
              }`}
            >
              {t(mode === 'cases' ? 'testengine.casesView' : 'testengine.runsView')}
            </button>
          ))}
        </div>
        {compactView === 'details' ? <Button className="mt-4 xl:hidden" variant="ghost" onClick={() => setCompactView('cases')}><ArrowLeft />{t(viewMode === 'runs' ? 'testengine.backToRuns' : 'testengine.backToCases')}</Button> : null}
      </div>

      {(error || (viewMode === 'runs' && runError)) && (
        <div className="flex items-center gap-2 border-b bg-destructive/10 px-4 py-2 text-sm text-destructive md:px-6 xl:px-8">
          <CircleAlert className="size-4" />
          {error || runError}
        </div>
      )}

      <div className="grid flex-1 grid-cols-1 xl:min-h-0 xl:grid-cols-[180px_minmax(430px,1fr)_360px] 2xl:grid-cols-[210px_minmax(520px,1fr)_420px]">
        <aside className="border-b bg-muted/30 xl:min-h-0 xl:border-r xl:border-b-0">
          <div className="flex items-center justify-between px-3 py-3 xl:border-b">
            <span className="text-xs font-medium text-muted-foreground uppercase">{t('testengine.suites')}</span>
            <span className="text-xs text-muted-foreground">{suites.length}</span>
          </div>
          <div className="flex overflow-x-auto p-2 xl:block xl:max-h-[calc(100vh-19rem)] xl:overflow-y-auto">
            {loadingSuites ? (
              <div className="px-2 py-4 text-xs text-muted-foreground">{t('testengine.loadingSuites')}</div>
            ) : suites.length === 0 ? (
              <button
                type="button"
                onClick={() => setPanel('create')}
                className="w-full border border-dashed p-4 text-left text-xs text-muted-foreground"
              >
                {t('testengine.createFirstSuite')}
              </button>
              ) : (
              suites.map((suite) => (
                <button
                  key={suite.id}
                  type="button"
                  onClick={() => { setSelectedSuiteId(suite.id); setCompactView('cases') }}
                  className={`mr-2 min-w-48 rounded-lg border px-3 py-2.5 text-left transition-colors xl:mr-0 xl:mb-1 xl:w-full xl:min-w-0 ${
                    suite.id === selectedSuiteId
                      ? 'border-border bg-background shadow-sm'
                      : 'border-transparent hover:bg-background'
                  }`}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="truncate text-sm font-medium">{suite.suite_key}</span>
                    <ChevronRight className="size-3.5 text-muted-foreground" />
                  </div>
                  <div className="mt-1 truncate text-xs text-muted-foreground">
                    {suite.owner}/{suite.repo}
                  </div>
                  <div className="mt-2 flex gap-3 text-xs text-muted-foreground">
                    <span>{t('testengine.cases', { count: suite.total_tests })}</span>
                    <span>{t('testengine.execs', { count: suite.execution_count })}</span>
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
          <div className={viewMode === 'cases' ? 'contents' : 'hidden'}>
          <div className="border-b px-4 py-3">
            <div className="flex flex-wrap items-center gap-2">
              <div className="relative min-w-56 flex-1">
                <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  className="pl-8"
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder={t('testengine.searchPlaceholder')}
                />
              </div>
              <div className="flex max-w-full items-center overflow-x-auto rounded-md border bg-muted/40 p-1">
                <Filter className="mx-1.5 size-3.5 text-muted-foreground" />
                {filterItems.map((item) => (
                  <button
                    key={item.value}
                    type="button"
                    onClick={() => setFilter(item.value)}
                    className={`h-6 px-2 text-xs ${
                      filter === item.value
                        ? 'rounded bg-background font-medium shadow-sm'
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
                className="h-9 rounded-md border bg-background px-2 text-xs outline-none focus:border-ring"
              >
                <option value="recent">{t('testengine.sortRecent')}</option>
                <option value="name">{t('testengine.sortName')}</option>
                <option value="slowest">{t('testengine.sortSlowest')}</option>
                <option value="failures">{t('testengine.sortFailures')}</option>
              </select>
              {selectedSuite && (
                <Button
                  size="icon"
                  variant="ghost"
                  onClick={() => void loadTests(selectedSuite.id)}
                  title={t('testengine.refreshCases')}
                  aria-label={t('testengine.refreshCases')}
                >
                  <RefreshCw className={loadingTests ? 'animate-spin' : ''} />
                </Button>
              )}
            </div>
            <div className="mt-2 text-xs text-muted-foreground">
              {t('testengine.showingCases', {
                shown: filteredTests.length.toLocaleString(),
                total: (tests?.length ?? 0).toLocaleString(),
              })}
            </div>
          </div>

          <div className="max-h-[70vh] overflow-auto xl:max-h-none xl:min-h-0 xl:flex-1">
            <div className="divide-y lg:hidden">
              {loadingTests || tests === null ? <div className="px-4 py-12 text-center text-sm text-muted-foreground">{t('testengine.loadingTestCases')}</div> : filteredTests.length === 0 ? <div className="px-4 py-12 text-center text-sm text-muted-foreground">{t('testengine.noCasesMatch')}</div> : visibleTests.map((test) => (
                <button key={test.id} type="button" onClick={() => { setSelectedTestId(test.id); setCompactView('details') }} className={`w-full p-4 text-left transition-colors ${test.id === selectedTestId ? 'bg-muted' : 'hover:bg-muted/50'}`}>
                  <div className="flex items-start justify-between gap-3"><div className="min-w-0"><div className="truncate font-mono text-sm font-medium">{test.name}</div><div className="mt-1 truncate text-xs text-muted-foreground">{test.file ?? t('testengine.noSourceLocation')}</div></div><StatusBadge value={test.last_status} /></div>
                  <div className="mt-3 flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground"><span>{t('testengine.execs', { count: test.execution_count })}</span><span>{t('testengine.passRate')}: {percentage(test.passed_count, test.execution_count)}%</span><span>{formatDuration(test.average_duration_ms)}</span>{test.state !== 'enabled' ? <StatusBadge value={test.state} /> : null}</div>
                </button>
              ))}
            </div>
            <table className="hidden w-full min-w-[760px] border-collapse text-left lg:table">
              <thead className="sticky top-0 z-10 bg-muted/60 text-xs text-muted-foreground">
                <tr className="border-b">
                  <th className="px-4 py-2.5 font-medium">{t('testengine.colTestCase')}</th>
                  <th className="w-24 px-3 py-2.5 font-medium">{t('testengine.colLatest')}</th>
                  <th className="w-20 px-3 py-2.5 text-right font-medium">{t('testengine.colExecs')}</th>
                  <th className="w-24 px-3 py-2.5 text-right font-medium">{t('testengine.colPassRate')}</th>
                  <th className="w-24 px-3 py-2.5 text-right font-medium">{t('testengine.colAvgTime')}</th>
                  <th className="w-32 px-4 py-2.5 text-right font-medium">{t('testengine.colLastSeen')}</th>
                </tr>
              </thead>
              <tbody>
                {loadingTests || tests === null ? (
                  <tr>
                    <td colSpan={6} className="px-4 py-12 text-center text-sm text-muted-foreground">
                      {t('testengine.loadingTestCases')}
                    </td>
                  </tr>
                ) : filteredTests.length === 0 ? (
                  <tr>
                    <td colSpan={6} className="px-4 py-12 text-center text-sm text-muted-foreground">
                      {t('testengine.noCasesMatch')}
                    </td>
                  </tr>
                ) : (
                  visibleTests.map((test) => (
                    <tr
                      key={test.id}
                      onClick={() => {
                        setSelectedTestId(test.id)
                        setCompactView('details')
                      }}
                      className={`cursor-pointer border-b transition-colors ${
                        test.id === selectedTestId ? 'bg-muted' : 'hover:bg-muted/50'
                      }`}
                    >
                      <td className="px-4 py-3">
                        <div
                          className="max-w-xl truncate font-mono text-xs font-medium"
                          title={test.name}
                        >
                          {test.name}
                        </div>
                        <div className="mt-1 flex items-center gap-2 text-xs text-muted-foreground">
                          <span className="truncate">{test.file ?? t('testengine.noSourceLocation')}</span>
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
                      <td className="px-4 py-3 text-right text-xs text-muted-foreground">
                        {formatDate(test.last_captured)}
                      </td>
                    </tr>
                  ))
                )}
              </tbody>
            </table>
            {visibleTests.length < filteredTests.length ? <div className="border-t p-4"><Button className="w-full" variant="outline" onClick={() => setVisibleCaseLimit((current) => current + 100)}>{t('testengine.showMoreCases', { shown: visibleTests.length, total: filteredTests.length })}</Button></div> : null}
          </div>
          </div>
          {viewMode === 'runs' && (
            <>
              <div className="flex items-center justify-between gap-3 border-b px-4 py-3">
                <div>
                  <h2 className="text-sm font-semibold">{t('testengine.runHistory')}</h2>
                  <p className="mt-1 text-xs text-muted-foreground">
                    {t('testengine.runHistoryDescription')}
                  </p>
                </div>
                {selectedSuite && (
                  <Button
                    size="icon"
                    variant="ghost"
                    onClick={() => void loadRuns(selectedSuite.id)}
                    title={t('testengine.refreshRuns')}
                    aria-label={t('testengine.refreshRuns')}
                  >
                    <RefreshCw className={loadingRuns ? 'animate-spin' : ''} />
                  </Button>
                )}
              </div>
              <div className="max-h-[70vh] flex-1 overflow-auto xl:max-h-none xl:min-h-0">
                {loadingRuns && runItems.length === 0 ? (
                  <div className="px-4 py-12 text-center text-sm text-muted-foreground">
                    {t('testengine.loadingRuns')}
                  </div>
                ) : runItems.length === 0 ? (
                  <div className="px-4 py-12 text-center text-sm text-muted-foreground">
                    {t('testengine.noRuns')}
                  </div>
                ) : (
                  <div className="divide-y">
                    {runItems.map((run) => (
                      <button
                        key={run.id}
                        type="button"
                        onClick={() => {
                          setSelectedRunId(run.id)
                          setCompactView('details')
                        }}
                        className={`w-full px-4 py-3 text-left transition-colors ${
                          run.id === selectedRunId ? 'bg-muted' : 'hover:bg-muted/50'
                        }`}
                      >
                        <div className="flex items-start justify-between gap-3">
                          <div className="min-w-0">
                            <div className="truncate font-mono text-sm font-medium" title={run.run_ref}>
                              {run.run_ref}
                            </div>
                            <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                              <span>{run.ci_provider}</span>
                              <span>·</span>
                              <span>{formatDate(run.last_captured ?? run.started_at)}</span>
                            </div>
                          </div>
                          <StatusBadge value={runStatus(run)} />
                        </div>
                        <div className="mt-3 grid grid-cols-3 gap-3 text-xs">
                          <div>
                            <div className="text-muted-foreground">{t('testengine.executions')}</div>
                            <div className="mt-0.5 font-medium tabular-nums">{run.execution_count}</div>
                          </div>
                          <div>
                            <div className="text-muted-foreground">{t('testengine.passRate')}</div>
                            <div className="mt-0.5 font-medium tabular-nums">
                              {percentage(run.passed_count, run.execution_count)}%
                            </div>
                          </div>
                          <div>
                            <div className="text-muted-foreground">{t('testengine.totalDuration')}</div>
                            <div className="mt-0.5 font-medium tabular-nums">
                              {formatDuration(run.total_duration_ms)}
                            </div>
                          </div>
                        </div>
                      </button>
                    ))}
                  </div>
                )}
                {hasMoreRuns && selectedSuite && (
                  <div className="p-4">
                    <Button
                      className="w-full"
                      variant="outline"
                      onClick={() => void loadRuns(selectedSuite.id, runItems.length)}
                      disabled={loadingRuns}
                    >
                      {loadingRuns
                        ? t('testengine.loading')
                        : t('testengine.loadMoreRuns', {
                            shown: runItems.length,
                            total: runs?.total,
                          })}
                    </Button>
                  </div>
                )}
              </div>
            </>
          )}
        </section>

        <aside
          className={`min-w-0 bg-background xl:block xl:min-h-0 xl:overflow-y-auto ${
            compactView === 'cases' ? 'hidden' : 'block'
          }`}
        >
          {viewMode === 'cases' ? (!selectedTest ? (
            <div className="flex min-h-64 flex-col items-center justify-center px-8 text-center text-sm text-muted-foreground xl:h-full">
              <ListChecks className="mb-3 size-7" />
              {t('testengine.selectTestCase')}
            </div>
          ) : (
            <div>
              <div className="border-b px-5 py-4">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="text-xs text-muted-foreground uppercase">{t('testengine.colTestCase')}</div>
                    <h2 className="mt-1 break-words font-mono text-sm font-semibold leading-relaxed">
                      {selectedTest.name}
                    </h2>
                  </div>
                  <StatusBadge value={selectedTest.state} />
                </div>
                <div className="mt-4">
                  <div className="mb-1.5 text-xs font-medium text-muted-foreground uppercase">
                    {t('testengine.disposition')}
                  </div>
                  <div className="grid grid-cols-3 rounded-lg border bg-muted/40 p-1">
                    {(['enabled', 'muted', 'skipped'] as const).map((state) => (
                      <button
                        key={state}
                        type="button"
                        onClick={() => void updateTestState(state)}
                        disabled={updatingState}
                        className={`h-7 text-xs capitalize transition-colors ${
                          selectedTest.state === state
                            ? 'rounded-md bg-background font-medium shadow-sm'
                            : 'text-muted-foreground hover:text-foreground'
                        }`}
                      >
                        {t(`testengine.state.${state}`)}
                      </button>
                    ))}
                  </div>
                </div>
                <div className="mt-4 grid grid-cols-3 divide-x border-y">
                  <div className="py-3 pr-3">
                    <div className="text-xs text-muted-foreground uppercase">{t('testengine.passRate')}</div>
                    <div className="mt-1 text-lg font-semibold tabular-nums">{casePassRate}%</div>
                  </div>
                  <div className="px-3 py-3">
                    <div className="text-xs text-muted-foreground uppercase">{t('testengine.executions')}</div>
                    <div className="mt-1 text-lg font-semibold tabular-nums">
                      {selectedTest.execution_count}
                    </div>
                  </div>
                  <div className="py-3 pl-3">
                    <div className="text-xs text-muted-foreground uppercase">{t('testengine.average')}</div>
                    <div className="mt-1 text-lg font-semibold tabular-nums">
                      {formatDuration(selectedTest.average_duration_ms)}
                    </div>
                  </div>
                </div>
                <div className="mt-3 flex h-1.5 overflow-hidden rounded-full bg-muted">
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
                <div className="mt-2 flex flex-wrap gap-4 text-xs text-muted-foreground">
                  <span>{t('testengine.passed', { count: selectedTest.passed_count })}</span>
                  <span>{t('testengine.failed', { count: selectedTest.failed_count + selectedTest.errored_count })}</span>
                  <span>{t('testengine.skipped', { count: selectedTest.skipped_count })}</span>
                </div>
              </div>

              <section className="border-b px-5 py-4">
                <div className="mb-2 flex items-center gap-2">
                  <FileCode2 className="size-4" />
                  <h3 className="text-sm font-semibold">{t('testengine.metadata')}</h3>
                </div>
                <dl className="divide-y">
                  <MetadataRow label={t('testengine.source')}>
                    {selectedTest.file ? (
                      <code>
                        {selectedTest.file}
                        {selectedTest.line_no ? `:${selectedTest.line_no}` : ''}
                      </code>
                    ) : (
                      t('testengine.notReported')
                    )}
                  </MetadataRow>
                  <MetadataRow label={t('testengine.latest')}>
                    <div className="flex items-center gap-2">
                      <StatusBadge value={selectedTest.last_status} />
                      <span>{formatDate(selectedTest.last_captured)}</span>
                    </div>
                  </MetadataRow>
                  <MetadataRow label={t('testengine.provider')}>
                    {selectedTest.last_ci_provider ?? t('testengine.notReported')}
                  </MetadataRow>
                  <MetadataRow label={t('testengine.run')}>
                      <span className="font-mono text-xs">
                      {selectedTest.last_run_ref ?? t('testengine.notReported')}
                    </span>
                  </MetadataRow>
                  <MetadataRow label={t('testengine.labels')}>
                    {selectedTest.labels.length ? (
                      <div className="flex flex-wrap gap-1">
                        {selectedTest.labels.map((label) => (
                          <span key={label} className="rounded bg-muted px-1.5 py-0.5">
                            {label}
                          </span>
                        ))}
                      </div>
                    ) : (
                      t('testengine.none')
                    )}
                  </MetadataRow>
                  <MetadataRow label={t('testengine.ownerTeams')}>
                    {selectedTest.owner_team_ids.length
                      ? selectedTest.owner_team_ids.join(', ')
                      : t('testengine.none')}
                  </MetadataRow>
                  <MetadataRow label={t('testengine.stateDecision')}>
                    <div className="space-y-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-mono text-xs">{selectedTest.state_source}</span>
                        <span>{formatDate(selectedTest.state_changed_at)}</span>
                      </div>
                      {selectedTest.state_reason ? (
                        <p className="text-xs text-muted-foreground">{selectedTest.state_reason}</p>
                      ) : null}
                    </div>
                  </MetadataRow>
                  <MetadataRow label={t('testengine.firstSeen')}>{formatDate(selectedTest.created_at)}</MetadataRow>
                  <MetadataRow label={t('testengine.updated')}>{formatDate(selectedTest.updated_at)}</MetadataRow>
                </dl>
              </section>

              <section className="max-h-[70vh] overflow-y-auto xl:max-h-none xl:overflow-visible">
                <div className="sticky top-0 z-10 flex items-center justify-between border-b bg-background px-5 py-3">
                  <div className="flex items-center gap-2">
                    <Activity className="size-4" />
                    <h3 className="text-sm font-semibold">{t('testengine.executionHistory')}</h3>
                  </div>
                  <span className="text-xs text-muted-foreground">
                    {t('testengine.total', { count: executions?.total ?? selectedTest.execution_count })}
                  </span>
                </div>
                {loadingExecutions && executionItems.length === 0 ? (
                  <div className="px-5 py-8 text-center text-xs text-muted-foreground">
                    {t('testengine.loadingExecutions')}
                  </div>
                ) : executionItems.length === 0 ? (
                  <div className="px-5 py-8 text-center text-xs text-muted-foreground">
                    {t('testengine.noExecutionHistory')}
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
                        ? t('testengine.loading')
                        : t('testengine.loadMore', {
                            shown: executionItems.length,
                            total: executions?.total,
                          })}
                    </Button>
                  </div>
                )}
              </section>
            </div>
          )) : !selectedRun ? (
            <div className="flex min-h-64 flex-col items-center justify-center px-8 text-center text-sm text-muted-foreground xl:h-full">
              <PlayCircle className="mb-3 size-7" />
              {t('testengine.selectRun')}
            </div>
          ) : (
            <div>
              <div className="border-b px-5 py-4">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="text-xs text-muted-foreground uppercase">
                      {t('testengine.run')}
                    </div>
                    <h2 className="mt-1 break-words font-mono text-sm font-semibold leading-relaxed">
                      {selectedRun.run_ref}
                    </h2>
                  </div>
                  <StatusBadge value={runStatus(selectedRun)} />
                </div>
                <div className="mt-4 grid grid-cols-3 divide-x border-y">
                  <div className="py-3 pr-3">
                    <div className="text-xs text-muted-foreground uppercase">
                      {t('testengine.passRate')}
                    </div>
                    <div className="mt-1 text-lg font-semibold tabular-nums">
                      {percentage(selectedRun.passed_count, selectedRun.execution_count)}%
                    </div>
                  </div>
                  <div className="px-3 py-3">
                    <div className="text-xs text-muted-foreground uppercase">
                      {t('testengine.executions')}
                    </div>
                    <div className="mt-1 text-lg font-semibold tabular-nums">
                      {selectedRun.execution_count}
                    </div>
                  </div>
                  <div className="py-3 pl-3">
                    <div className="text-xs text-muted-foreground uppercase">
                      {t('testengine.totalDuration')}
                    </div>
                    <div className="mt-1 text-lg font-semibold tabular-nums">
                      {formatDuration(selectedRun.total_duration_ms)}
                    </div>
                  </div>
                </div>
                <div className="mt-3 flex h-1.5 overflow-hidden rounded-full bg-muted">
                  <div
                    className="bg-emerald-500"
                    style={{
                      width: `${percentage(selectedRun.passed_count, selectedRun.execution_count)}%`,
                    }}
                  />
                  <div
                    className="bg-red-500"
                    style={{
                      width: `${percentage(
                        selectedRun.failed_count + selectedRun.errored_count,
                        selectedRun.execution_count,
                      )}%`,
                    }}
                  />
                  <div
                    className="bg-zinc-400"
                    style={{
                      width: `${percentage(selectedRun.skipped_count, selectedRun.execution_count)}%`,
                    }}
                  />
                </div>
                <div className="mt-2 flex flex-wrap gap-4 text-xs text-muted-foreground">
                  <span>{t('testengine.passed', { count: selectedRun.passed_count })}</span>
                  <span>
                    {t('testengine.failed', {
                      count: selectedRun.failed_count + selectedRun.errored_count,
                    })}
                  </span>
                  <span>{t('testengine.skipped', { count: selectedRun.skipped_count })}</span>
                </div>
              </div>

              <section className="border-b px-5 py-4">
                <div className="mb-2 flex items-center gap-2">
                  <PlayCircle className="size-4" />
                  <h3 className="text-sm font-semibold">{t('testengine.runMetadata')}</h3>
                </div>
                <dl className="divide-y">
                  <MetadataRow label={t('testengine.provider')}>
                    {selectedRun.ci_provider}
                  </MetadataRow>
                  <MetadataRow label={t('testengine.started')}>
                    {formatDate(selectedRun.started_at)}
                  </MetadataRow>
                  <MetadataRow label={t('testengine.lastCaptured')}>
                    {selectedRun.last_captured
                      ? formatDate(selectedRun.last_captured)
                      : t('testengine.notReported')}
                  </MetadataRow>
                  <MetadataRow label={t('testengine.finished')}>
                    {selectedRun.finished_at
                      ? formatDate(selectedRun.finished_at)
                      : t('testengine.notReported')}
                  </MetadataRow>
                  <MetadataRow label={t('testengine.invocation')}>
                    {selectedRun.invocation_id ? (
                      <span className="font-mono text-xs">{selectedRun.invocation_id}</span>
                    ) : (
                      t('testengine.none')
                    )}
                  </MetadataRow>
                </dl>
              </section>

              <section className="max-h-[70vh] overflow-y-auto xl:max-h-none xl:overflow-visible">
                <div className="sticky top-0 z-10 flex items-center justify-between border-b bg-background px-5 py-3">
                  <div className="flex items-center gap-2">
                    <Activity className="size-4" />
                    <h3 className="text-sm font-semibold">{t('testengine.runExecutions')}</h3>
                  </div>
                  <span className="text-xs text-muted-foreground">
                    {t('testengine.total', {
                      count: runExecutions?.total ?? selectedRun.execution_count,
                    })}
                  </span>
                </div>
                {loadingRunExecutions && runExecutionItems.length === 0 ? (
                  <div className="px-5 py-8 text-center text-xs text-muted-foreground">
                    {t('testengine.loadingExecutions')}
                  </div>
                ) : runExecutionItems.length === 0 ? (
                  <div className="px-5 py-8 text-center text-xs text-muted-foreground">
                    {t('testengine.noRunExecutions')}
                  </div>
                ) : (
                  <div>
                    {runExecutionItems.map((execution) => (
                      <RunExecutionRow key={execution.id} execution={execution} />
                    ))}
                  </div>
                )}
                {hasMoreRunExecutions && (
                  <div className="p-4">
                    <Button
                      className="w-full"
                      variant="outline"
                      onClick={() =>
                        selectedRunId &&
                        void loadRunExecutions(selectedRunId, runExecutionItems.length)
                      }
                      disabled={loadingRunExecutions}
                    >
                      {loadingRunExecutions
                        ? t('testengine.loading')
                        : t('testengine.loadMore', {
                            shown: runExecutionItems.length,
                            total: runExecutions?.total,
                          })}
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
