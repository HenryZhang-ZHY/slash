import { useEffect, useState } from 'react'

import {
  testEngineApi,
  type TestExecution,
  type TestExecutionPage,
  type TestSuiteSummary,
  type TestSummary,
} from '@/lib/api'

export function useTestEngine() {
  const [suites, setSuites] = useState<TestSuiteSummary[]>([])
  const [selectedSuiteId, setSelectedSuiteId] = useState<string | null>(null)
  const [tests, setTests] = useState<TestSummary[] | null>(null)
  const [selectedTestId, setSelectedTestId] = useState<string | null>(null)
  const [executions, setExecutions] = useState<TestExecutionPage | null>(null)
  const [executionItems, setExecutionItems] = useState<TestExecution[]>([])
  const [loadingSuites, setLoadingSuites] = useState(true)
  const [loadingTests, setLoadingTests] = useState(false)
  const [loadingExecutions, setLoadingExecutions] = useState(false)
  const [updatingState, setUpdatingState] = useState(false)
  const [error, setError] = useState<string | null>(null)

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
      setError(requestError instanceof Error ? requestError.message : 'Execution history 加载失败')
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

  const createSuite = (suite: TestSuiteSummary) => {
    setSuites((current) => [suite, ...current.filter((item) => item.id !== suite.id)])
    setSelectedSuiteId(suite.id)
  }

  const updateTestState = async (state: TestSummary['state']) => {
    const selectedTest = tests?.find((test) => test.id === selectedTestId) ?? null
    if (!selectedTest) return
    setUpdatingState(true)
    setError(null)
    try {
      await testEngineApi.setTestState(selectedTest.id, state)
      setTests(
        (current) =>
          current?.map((test) =>
            test.id === selectedTest.id ? { ...test, state } : test,
          ) ?? null,
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

  const selectedSuite = suites.find((suite) => suite.id === selectedSuiteId) ?? null
  const selectedTest = tests?.find((test) => test.id === selectedTestId) ?? null

  return {
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
    hasMoreExecutions: executions ? executionItems.length < executions.total : false,
    loadSuites,
    loadTests,
    loadExecutions,
    createSuite,
    updateTestState,
  }
}
