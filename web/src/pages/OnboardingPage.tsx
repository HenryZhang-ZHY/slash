import { useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'

import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { api } from '@/lib/api'

export function OnboardingPage() {
  const [name, setName] = useState('')
  const [slug, setSlug] = useState('')
  const [checking, setChecking] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [done, setDone] = useState(false)
  const navigate = useNavigate()

  // Redirect unauthenticated users to login.
  useEffect(() => {
    api
      .me()
      .catch(() => navigate('/login'))
      .finally(() => setChecking(false))
  }, [navigate])

  const slugify = (v: string) =>
    v.toLowerCase().trim().replace(/[^a-z0-9-]/g, '-').replace(/-+/g, '-').replace(/^-|-$/g, '')

  const onSubmit = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault()
      setError(null)
      setBusy(true)
      try {
        const { team } = await api.createTeam(name, slugify(slug || name))
        if (team) setDone(true)
      } catch (err) {
        setError(err instanceof Error ? err.message : '创建失败')
      } finally {
        setBusy(false)
      }
    },
    [name, slug],
  )

  if (checking) return null

  return (
    <div className="flex min-h-screen items-center justify-center bg-background p-4">
      <Card className="w-full max-w-md">
        <CardHeader>
          <CardTitle>{done ? '搞定 🎉' : '创建你的团队'}</CardTitle>
          <CardDescription>
            {done
              ? '你的团队已创建。后续可以在这里添加成员、配置仓库权限。'
              : '团队是访问控制的中心。先给你的团队起个名字。'}
          </CardDescription>
        </CardHeader>
        <CardContent>
          {done ? (
            <Button onClick={() => navigate('/')}>进入控制台</Button>
          ) : (
            <form onSubmit={onSubmit} className="grid gap-3">
              <div className="grid gap-1.5">
                <Label htmlFor="name">团队名称</Label>
                <Input
                  id="name"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="例如 Acme"
                  required
                />
              </div>
              <div className="grid gap-1.5">
                <Label htmlFor="slug">团队标识（slug）</Label>
                <Input
                  id="slug"
                  value={slug}
                  onChange={(e) => setSlug(slugify(e.target.value))}
                  placeholder="例如 acme"
                  pattern="[a-z0-9-]{1,32}"
                  title="小写字母、数字、连字符，最多 32 位"
                  required
                />
                <p className="text-xs text-muted-foreground">
                  只含小写字母、数字和连字符。不填则从团队名生成。
                </p>
              </div>
              {error ? <p className="text-sm text-destructive">{error}</p> : null}
              <Button type="submit" disabled={busy}>
                {busy ? '请稍候…' : '创建团队'}
              </Button>
            </form>
          )}
        </CardContent>
      </Card>
    </div>
  )
}
