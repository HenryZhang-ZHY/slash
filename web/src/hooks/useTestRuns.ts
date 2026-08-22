import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import {
  testEngineApi,
  type RunExecution,
  type RunExecutionPage,
  type TestRun,
  type TestRunPage,
} from '@/lib/api'

export function useTestRuns(suiteId: string | null, enabled: boolean) {
  const { t } = useTranslation()
  const [runs, setRuns] = useState<TestRunPage | null>(null)
  const [runItems, setRunItems] = useState<TestRun[]>([])
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null)
  const [executions, setExecutions] = useState<RunExecutionPage | null>(null)
  const [executionItems, setExecutionItems] = useState<RunExecution[]>([])
  const [loadingRuns, setLoadingRuns] = useState(false)
  const [loadingExecutions, setLoadingExecutions] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const loadRuns = useCallback(
    async (targetSuiteId: string, offset = 0) => {
      setLoadingRuns(true)
      setError(null)
      try {
        const page = await testEngineApi.listRuns(targetSuiteId, 100, offset)
        setRuns(page)
        setRunItems((current) => (offset === 0 ? page.items : [...current, ...page.items]))
        if (offset === 0) {
          setSelectedRunId((current) =>
            current && page.items.some((run) => run.id === current)
              ? current
              : (page.items[0]?.id ?? null),
          )
        }
      } catch (requestError) {
        setError(requestError instanceof Error ? requestError.message : t('testengine.runsLoadFailed'))
      } finally {
        setLoadingRuns(false)
      }
    },
    [t],
  )

  const loadRunExecutions = useCallback(
    async (runId: string, offset = 0) => {
      setLoadingExecutions(true)
      setError(null)
      try {
        const page = await testEngineApi.listRunExecutions(runId, 100, offset)
        setExecutions(page)
        setExecutionItems((current) => (offset === 0 ? page.items : [...current, ...page.items]))
      } catch (requestError) {
        setError(
          requestError instanceof Error
            ? requestError.message
            : t('testengine.runExecutionsLoadFailed'),
        )
      } finally {
        setLoadingExecutions(false)
      }
    },
    [t],
  )

  useEffect(() => {
    if (!enabled || !suiteId) return
    setRuns(null)
    setRunItems([])
    setSelectedRunId(null)
    void loadRuns(suiteId)
  }, [enabled, suiteId, loadRuns])

  useEffect(() => {
    setExecutions(null)
    setExecutionItems([])
    if (enabled && selectedRunId) void loadRunExecutions(selectedRunId)
  }, [enabled, selectedRunId, loadRunExecutions])

  const selectedRun = runItems.find((run) => run.id === selectedRunId) ?? null

  return {
    runs,
    runItems,
    selectedRunId,
    setSelectedRunId,
    selectedRun,
    executions,
    executionItems,
    loadingRuns,
    loadingExecutions,
    error,
    hasMoreRuns: runs ? runItems.length < runs.total : false,
    hasMoreExecutions: executions ? executionItems.length < executions.total : false,
    loadRuns,
    loadRunExecutions,
  }
}
