import { useCallback, useEffect, useState } from 'react'

import { Button } from '@/components/ui/button'
import { testEngineApi, type TestSuiteSummary, type TestSummary } from '@/lib/api'

const STATE_BADGE: Record<TestSummary['state'], string> = {
  enabled: 'bg-emerald-100 text-emerald-700',
  muted: 'bg-amber-100 text-amber-700',
  skipped: 'bg-slate-200 text-slate-600',
}

function SuiteCard({ suite }: { suite: TestSuiteSummary }) {
  const [tests, setTests] = useState<TestSummary[] | null>(null)
  const [open, setOpen] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const loadTests = useCallback(() => {
    setError(null)
    setTests(null)
    testEngineApi
      .listTests(suite.id)
      .then(setTests)
      .catch((e) => setError(e instanceof Error ? e.message : '加载失败'))
  }, [suite.id])

  const toggle = () => {
    const next = !open
    setOpen(next)
    if (next && tests === null) loadTests()
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
          {error ? (
            <div className="flex items-center gap-3 text-sm text-red-600">
              {error} <Button size="sm" variant="outline" onClick={loadTests}>重试</Button>
            </div>
          ) : tests === null ? (
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
                    tests.map((t) => (
                      <tr key={t.id} className="border-t">
                        <td className="px-3 py-2 font-mono text-xs">{t.name}</td>
                        <td className="px-3 py-2">
                          <span className={`rounded px-1.5 py-0.5 text-xs font-medium ${STATE_BADGE[t.state]}`}>
                            {t.state}
                          </span>
                        </td>
                        <td className="px-3 py-2 text-xs text-muted-foreground">
                          {t.last_status ?? '—'}
                          {t.last_captured ? ` · ${new Date(t.last_captured).toLocaleString()}` : ''}
                        </td>
                      </tr>
                    ))
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

  return (
    <div className="mx-auto max-w-3xl p-6">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold">Test Engine 控制台</h1>
        <Button variant="ghost" onClick={() => (window.location.href = '/')}>
          返回
        </Button>
      </div>
      <p className="mt-1 text-sm text-muted-foreground">
        人工测试用：查看已采集的 suite/tests 与 flaky 隔离状态。采集入口：
        <code className="mx-1 rounded bg-muted px-1 py-0.5 text-xs">POST /v1/test-engine/upload</code>
        、<code className="mx-1 rounded bg-muted px-1 py-0.5 text-xs">/cargo</code>、<code className="mx-1 rounded bg-muted px-1 py-0.5 text-xs">/vitest</code>
      </p>

      <div className="mt-6 space-y-3">
        {error ? (
          <div className="flex items-center gap-3 text-sm text-red-600">
            读取失败：{error} <Button size="sm" variant="outline" onClick={load}>重试</Button>
          </div>
        ) : suites === null ? (
          <div className="text-sm text-muted-foreground">加载中…</div>
        ) : suites.length === 0 ? (
          <div className="rounded-lg border px-4 py-6 text-sm text-muted-foreground">
            还没有任何 suite。向 <code className="rounded bg-muted px-1">/v1/test-engine/upload</code> 上传一组测试结果后，这里会列出它们。
          </div>
        ) : (
          suites.map((s) => <SuiteCard key={s.id} suite={s} />)
        )}
      </div>
    </div>
  )
}
