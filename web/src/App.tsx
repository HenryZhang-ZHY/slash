import { useCallback, useEffect, useState } from 'react'
import { Navigate, Route, BrowserRouter, Routes } from 'react-router-dom'

import { Button } from '@/components/ui/button'
import { api } from '@/lib/api'
import type { MeResponse } from '@/lib/api'
import { LoginPage } from '@/pages/LoginPage'
import { OnboardingPage } from '@/pages/OnboardingPage'

function HomePage() {
  const [me, setMe] = useState<MeResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)

  const load = useCallback(() => {
    setError(null)
    setLoading(true)
    api
      .me()
      .then(setMe)
      .catch((e) => {
        if (e instanceof Error && (e as { status?: number }).status === 401) {
          window.location.href = '/login'
          return
        }
        setError(e instanceof Error ? e.message : '加载失败')
      })
      .finally(() => setLoading(false))
  }, [])

  useEffect(load, [load])

  const logout = async () => {
    await api.logout()
    window.location.href = '/login'
  }

  if (loading) return <div className="p-6 text-muted-foreground">加载中…</div>
  if (error)
    return (
      <div className="p-6">
        读取登录态失败：{error} <Button onClick={load}>重试</Button>
      </div>
    )
  if (!me) return null

  return (
    <div className="mx-auto max-w-2xl p-6">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold">Slash 控制台</h1>
        <div className="flex items-center gap-3">
          <span className="text-sm text-muted-foreground">{me.user.email}</span>
          <Button variant="ghost" onClick={logout}>
            退出
          </Button>
        </div>
      </div>
      <div className="mt-6 grid gap-3">
        <h2 className="text-sm font-medium text-muted-foreground">我的团队</h2>
        {me.teams.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            还没有团队。创建一个团队开始使用 Slash。
          </p>
        ) : (
          me.teams.map((t) => (
            <div key={t.id} className="rounded-lg border px-4 py-3">
              <div className="font-medium">{t.name}</div>
              <div className="text-xs text-muted-foreground">{t.slug}</div>
            </div>
          ))
        )}
      </div>
    </div>
  )
}

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<HomePage />} />
        <Route path="/login" element={<LoginPage />} />
        <Route path="/onboarding" element={<OnboardingPage />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </BrowserRouter>
  )
}
