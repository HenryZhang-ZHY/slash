import { useCallback, useEffect, useState } from 'react'
import {
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  Eye,
  EyeOff,
  KeyRound,
  Plus,
  RefreshCw,
} from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  testEngineApi,
  type TestExecution,
  type TestSuiteSummary,
  type TestSummary,
} from '@/lib/api'

const STATE_BADGE: Record<TestSummary['state'], string> = {
  enabled: 'bg-emerald-100 text-emerald-700',
  muted: 'bg-amber-100 text-amber-700',
  skipped: 'bg-slate-200 text-slate-600',
}

const EXECUTION_BADGE: Record<TestExecution['status'], string> = {
  passed: 'bg-emerald-100 text-emerald-700',
  failed: 'bg-red-100 text-red-700',
  skipped: 'bg-slate-200 text-slate-600',
  errored: 'bg-red-100 text-red-700',
}

function formatDuration(durationMs: number) {
  return durationMs < 1000 ? `${Math.round(durationMs)} ms` : `${(durationMs / 1000).toFixed(2)} s`
}

function TestCaseRow({ test }: { test: TestSummary }) {
  const [open, setOpen] = useState(false)
  const [executions, setExecutions] = useState<TestExecution[] | null>(null)
  const [error, setError] = useState<string | null>(null)

  const toggle = async () => {
    const next = !open
    setOpen(next)
    if (!next || executions !== null) return
    setError(null)
    try {
      setExecutions(await testEngineApi.listExecutions(test.id))
    } catch (e) {
      setError(e instanceof Error ? e.message : '执行历史加载失败')
    }
  }

  return (
    <>
      <tr className="border-t">
        <td className="px-3 py-2 font-mono text-xs">
          <button type="button" onClick={toggle} className="flex items-start gap-1.5 text-left">
            {open ? <ChevronDown className="mt-0.5 size-3.5" /> : <ChevronRight className="mt-0.5 size-3.5" />}
            <span>{test.name}</span>
          </button>
        </td>
        <td className="px-3 py-2">
          <span className={`rounded px-1.5 py-0.5 text-xs font-medium ${STATE_BADGE[test.state]}`}>
            {test.state}
          </span>
        </td>
        <td className="px-3 py-2 text-xs text-muted-foreground">
          {test.last_status ?? '—'}
          {test.last_captured ? ` · ${new Date(test.last_captured).toLocaleString()}` : ''}
        </td>
      </tr>
      {open && (
        <tr className="border-t bg-muted/20">
          <td colSpan={3} className="px-7 py-3">
            {error ? (
              <div className="text-xs text-red-600">{error}</div>
            ) : executions === null ? (
              <div className="text-xs text-muted-foreground">加载执行历史…</div>
            ) : executions.length === 0 ? (
              <div className="text-xs text-muted-foreground">还没有 execution 记录。</div>
            ) : (
              <div className="space-y-1.5">
                {executions.map((execution) => (
                  <div
                    key={execution.id}
                    className="grid grid-cols-[minmax(0,1fr)_auto_auto] items-center gap-3 text-xs"
                  >
                    <div className="min-w-0">
                      <div className="truncate font-mono">{execution.run_ref}</div>
                      <div className="text-muted-foreground">
                        {execution.ci_provider} · {new Date(execution.captured_at).toLocaleString()}
                      </div>
                    </div>
                    <span className={`rounded px-1.5 py-0.5 font-medium ${EXECUTION_BADGE[execution.status]}`}>
                      {execution.status}
                    </span>
                    <span className="w-16 text-right text-muted-foreground">
                      {formatDuration(execution.duration_ms)}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </td>
        </tr>
      )}
    </>
  )
}

function SuiteCard({ suite }: { suite: TestSuiteSummary }) {
  const [tests, setTests] = useState<TestSummary[] | null>(null)
  const [open, setOpen] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [token, setToken] = useState<string | null>(null)
  const [tokenLoaded, setTokenLoaded] = useState(false)
  const [tokenVisible, setTokenVisible] = useState(false)
  const [copied, setCopied] = useState(false)
  const [issuingToken, setIssuingToken] = useState(false)

  const loadTests = useCallback(() => {
    setError(null)
    setTests(null)
    testEngineApi
      .listTests(suite.id)
      .then(setTests)
      .catch((e) => setError(e instanceof Error ? e.message : '加载失败'))
  }, [suite.id])

  const loadToken = useCallback(() => {
    setTokenLoaded(false)
    testEngineApi
      .getToken(suite.id)
      .then((result) => setToken(result.token))
      .catch((e) => setError(e instanceof Error ? e.message : 'Token 加载失败'))
      .finally(() => setTokenLoaded(true))
  }, [suite.id])

  const toggle = () => {
    const next = !open
    setOpen(next)
    if (next && tests === null) loadTests()
    if (next && !tokenLoaded) loadToken()
  }

  const issueToken = async () => {
    setIssuingToken(true)
    setError(null)
    try {
      const result = await testEngineApi.issueToken(suite.id)
      setToken(result.token)
      setTokenLoaded(true)
      setTokenVisible(false)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Token 生成失败')
    } finally {
      setIssuingToken(false)
    }
  }

  const copyToken = async () => {
    if (!token) return
    await navigator.clipboard.writeText(token)
    setCopied(true)
    window.setTimeout(() => setCopied(false), 1500)
  }

  return (
    <div className="rounded-lg border">
      <button
        type="button"
        onClick={toggle}
        className="flex w-full items-center justify-between px-4 py-3 text-left hover:bg-muted/40"
      >
        <div>
          <div className="font-medium">{suite.suite_key}</div>
          <div className="text-xs text-muted-foreground">
            {suite.owner}/{suite.repo}
          </div>
        </div>
        <div className="flex items-center gap-3 text-xs text-muted-foreground">
          <span>{suite.total_tests} tests</span>
          {suite.muted > 0 && <span className="text-amber-600">{suite.muted} muted</span>}
          {suite.skipped > 0 && <span className="text-slate-500">{suite.skipped} skipped</span>}
        </div>
      </button>

      {open && (
        <div className="border-t px-4 py-3">
          <div className="mb-4">
            <Label htmlFor={`suite-token-${suite.id}`}>Collection token</Label>
            <div className="mt-1.5 flex items-center gap-2">
              <Input
                id={`suite-token-${suite.id}`}
                className="font-mono"
                type={tokenVisible ? 'text' : 'password'}
                value={token ?? ''}
                placeholder={tokenLoaded ? '尚无可显示 token' : '加载中…'}
                readOnly
              />
              <Button
                type="button"
                size="icon"
                variant="outline"
                onClick={() => setTokenVisible((visible) => !visible)}
                disabled={!token}
                title={tokenVisible ? '隐藏 token' : '显示 token'}
                aria-label={tokenVisible ? '隐藏 token' : '显示 token'}
              >
                {tokenVisible ? <EyeOff /> : <Eye />}
              </Button>
              <Button
                type="button"
                size="icon"
                variant="outline"
                onClick={copyToken}
                disabled={!token}
                title="复制 token"
                aria-label="复制 token"
              >
                {copied ? <Check /> : <Copy />}
              </Button>
              <Button size="sm" variant="outline" onClick={issueToken} disabled={issuingToken}>
                <KeyRound />
                {issuingToken ? '生成中…' : '生成新 token'}
              </Button>
            </div>
          </div>
          {error ? (
            <div className="mb-3 flex items-center gap-3 text-sm text-red-600">
              {error} <Button size="sm" variant="outline" onClick={loadTests}>重试</Button>
            </div>
          ) : null}
          {tests !== null && (
            <div className="mb-2 flex items-center justify-between">
              <span className="text-xs text-muted-foreground">{tests.length} test cases</span>
              <Button type="button" size="sm" variant="ghost" onClick={loadTests}>
                <RefreshCw />
                刷新 tests
              </Button>
            </div>
          )}
          {tests === null ? (
            <div className="text-sm text-muted-foreground">加载中…</div>
          ) : (
            <div className="overflow-hidden rounded-md border">
              <table className="w-full text-sm">
                <thead className="bg-muted/50 text-left text-xs text-muted-foreground">
                  <tr>
                    <th className="px-3 py-2 font-medium">Test</th>
                    <th className="px-3 py-2 font-medium">State</th>
                    <th className="px-3 py-2 font-medium">Latest run</th>
                  </tr>
                </thead>
                <tbody>
                  {tests.length === 0 ? (
                    <tr>
                      <td colSpan={3} className="px-3 py-3 text-muted-foreground">
                        还没有采集到这个 suite 的测试。
                      </td>
                    </tr>
                  ) : (
                    tests.map((test) => <TestCaseRow key={test.id} test={test} />)
                  )}
                </tbody>
              </table>
            </div>
          )}
        </div>
      )}
    </div>
  )
}

export function TestEnginePage() {
  const [suites, setSuites] = useState<TestSuiteSummary[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [owner, setOwner] = useState('')
  const [repo, setRepo] = useState('')
  const [suiteKey, setSuiteKey] = useState('ci-test')
  const [creating, setCreating] = useState(false)
  const [createError, setCreateError] = useState<string | null>(null)

  const load = useCallback(() => {
    setError(null)
    setSuites(null)
    testEngineApi
      .listSuites()
      .then(setSuites)
      .catch((e) => {
        if (e instanceof Error && (e as { status?: number }).status === 401) {
          window.location.href = '/login'
          return
        }
        setError(e instanceof Error ? e.message : '加载失败')
      })
  }, [])

  useEffect(load, [load])

  const createSuite = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    setCreating(true)
    setCreateError(null)
    try {
      const result = await testEngineApi.createSuite(owner, repo, suiteKey)
      setSuites((current) => [
        result.suite,
        ...(current ?? []).filter((suite) => suite.id !== result.suite.id),
      ])
    } catch (e) {
      setCreateError(e instanceof Error ? e.message : 'Suite 创建失败')
    } finally {
      setCreating(false)
    }
  }

  return (
    <div className="mx-auto max-w-3xl p-6">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold">Test Engine 控制台</h1>
        <Button variant="ghost" onClick={() => (window.location.href = '/')}>
          返回
        </Button>
      </div>
      <p className="mt-1 text-sm text-muted-foreground">
        管理采集 suite、collection token 与 flaky 隔离状态。采集入口：
        <code className="mx-1 rounded bg-muted px-1 py-0.5 text-xs">POST /v1/test-engine/upload</code>
        、<code className="mx-1 rounded bg-muted px-1 py-0.5 text-xs">/cargo</code>、<code className="mx-1 rounded bg-muted px-1 py-0.5 text-xs">/vitest</code>
      </p>

      <form onSubmit={createSuite} className="mt-6 border-y py-4">
        <div className="grid gap-3 sm:grid-cols-[1fr_1fr_1fr_auto] sm:items-end">
          <div className="space-y-1.5">
            <Label htmlFor="suite-owner">GitHub owner</Label>
            <Input
              id="suite-owner"
              value={owner}
              onChange={(event) => setOwner(event.target.value)}
              placeholder="HenryZhang-ZHY"
              required
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="suite-repo">Repository</Label>
            <Input
              id="suite-repo"
              value={repo}
              onChange={(event) => setRepo(event.target.value)}
              placeholder="slash"
              required
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="suite-key">Suite key</Label>
            <Input
              id="suite-key"
              value={suiteKey}
              onChange={(event) => setSuiteKey(event.target.value)}
              required
            />
          </div>
          <Button type="submit" disabled={creating}>
            <Plus />
            {creating ? '创建中…' : '创建 suite'}
          </Button>
        </div>
        {createError && <p className="mt-2 text-sm text-red-600">{createError}</p>}
      </form>

      <div className="mt-6 space-y-3">
        {error ? (
          <div className="flex items-center gap-3 text-sm text-red-600">
            读取失败：{error} <Button size="sm" variant="outline" onClick={load}>重试</Button>
          </div>
        ) : suites === null ? (
          <div className="text-sm text-muted-foreground">加载中…</div>
        ) : suites.length === 0 ? (
          <div className="rounded-lg border px-4 py-6 text-sm text-muted-foreground">
            还没有任何 suite。先在上方创建 suite，再把一次性 token 配置到 CI。
          </div>
        ) : (
          suites.map((suite) => <SuiteCard key={suite.id} suite={suite} />)
        )}
      </div>
    </div>
  )
}
