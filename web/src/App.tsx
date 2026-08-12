import { useEffect, useState } from 'react'
import { ArrowRight, FlaskConical, Users } from 'lucide-react'
import {
  BrowserRouter,
  Navigate,
  Route,
  Routes,
  useNavigate,
  useOutletContext,
} from 'react-router-dom'

import { Button } from '@/components/ui/button'
import { AppShell, type DashboardContext } from '@/components/AppShell'
import { testEngineApi, type TestSuiteSummary } from '@/lib/api'
import { LoginPage } from '@/pages/LoginPage'
import { OnboardingPage } from '@/pages/OnboardingPage'
import { TestEnginePage } from '@/pages/TestEnginePage'

function HomePage() {
  const navigate = useNavigate()
  const { me } = useOutletContext<DashboardContext>()
  const [suites, setSuites] = useState<TestSuiteSummary[]>([])

  useEffect(() => {
    testEngineApi.listSuites().then(setSuites).catch(() => setSuites([]))
  }, [])

  const testCount = suites.reduce((sum, suite) => sum + suite.total_tests, 0)
  const executionCount = suites.reduce((sum, suite) => sum + suite.execution_count, 0)

  return (
    <div className="mx-auto w-full max-w-[1680px] px-4 py-6 md:px-8 md:py-8">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold">Overview</h1>
          <p className="mt-1 text-sm text-muted-foreground">团队、仓库自动化与测试健康概览。</p>
        </div>
        <Button onClick={() => navigate('/tests')}>
          <FlaskConical />
          打开 Test Engine
        </Button>
      </div>

      <div className="mt-8 grid border-y sm:grid-cols-2 xl:grid-cols-4">
        {[
          ['Teams', me.teams.length.toLocaleString()],
          ['Test suites', suites.length.toLocaleString()],
          ['Test cases', testCount.toLocaleString()],
          ['Executions', executionCount.toLocaleString()],
        ].map(([label, value], index) => (
          <div key={label} className={`px-4 py-5 ${index > 0 ? 'border-t sm:border-t-0 sm:border-l' : ''}`}>
            <div className="text-xs text-muted-foreground">{label}</div>
            <div className="mt-2 text-2xl font-semibold tabular-nums">{value}</div>
          </div>
        ))}
      </div>

      <div className="mt-8 grid gap-8 xl:grid-cols-[minmax(0,1fr)_minmax(360px,0.55fr)]">
        <section>
          <div className="mb-3 flex items-center gap-2">
            <Users className="size-4" />
            <h2 className="text-sm font-semibold">Teams</h2>
          </div>
          <div className="border">
            {me.teams.length === 0 ? (
              <div className="px-4 py-8 text-sm text-muted-foreground">还没有团队。</div>
            ) : (
              me.teams.map((team, index) => (
                <div key={team.id} className={`flex items-center justify-between px-4 py-3 ${index > 0 ? 'border-t' : ''}`}>
                  <div>
                    <div className="text-sm font-medium">{team.name}</div>
                    <div className="text-xs text-muted-foreground">{team.slug}</div>
                  </div>
                  <span className="text-xs text-muted-foreground">Active</span>
                </div>
              ))
            )}
          </div>
        </section>

        <section>
          <div className="mb-3 flex items-center gap-2">
            <FlaskConical className="size-4" />
            <h2 className="text-sm font-semibold">Test Engine</h2>
          </div>
          <button
            type="button"
            onClick={() => navigate('/tests')}
            className="flex w-full items-center justify-between border px-4 py-4 text-left transition-colors hover:bg-muted/40"
          >
            <div>
              <div className="text-sm font-medium">查看测试健康与执行历史</div>
              <div className="mt-1 text-xs text-muted-foreground">
                {suites.length} suites · {testCount.toLocaleString()} cases · {executionCount.toLocaleString()} executions
              </div>
            </div>
            <ArrowRight className="size-4 text-muted-foreground" />
          </button>
        </section>
      </div>
    </div>
  )
}

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route path="/onboarding" element={<OnboardingPage />} />
        <Route element={<AppShell />}>
          <Route path="/" element={<HomePage />} />
          <Route path="/tests" element={<TestEnginePage />} />
        </Route>
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </BrowserRouter>
  )
}
