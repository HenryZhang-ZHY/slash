import { useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'

import { AuthShell } from '@/components/AuthShell'
import { Button } from '@/components/ui/button'
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
    <AuthShell
      title={done ? 'Workspace ready' : 'Create your first team'}
      description={
        done
          ? 'Your organization and team are ready.'
          : 'Teams define the access boundary for repositories and automation.'
      }
    >
      {done ? (
        <div className="space-y-5">
          <div className="border-y py-4 text-sm">
            <div className="font-medium">{name}</div>
            <div className="mt-1 text-xs text-muted-foreground">{slugify(slug || name)}</div>
          </div>
          <Button className="w-full" onClick={() => navigate('/')}>
            Enter workspace
          </Button>
        </div>
      ) : (
        <form onSubmit={onSubmit} className="space-y-4">
          <div className="space-y-1.5">
            <Label htmlFor="name">Team name</Label>
            <Input
              id="name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="Acme Engineering"
              required
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="slug">Team slug</Label>
            <Input
              id="slug"
              value={slug}
              onChange={(event) => setSlug(slugify(event.target.value))}
              placeholder="acme-engineering"
              pattern="[a-z0-9-]{1,32}"
              title="Lowercase letters, numbers and hyphens; up to 32 characters"
              required
            />
            <p className="text-xs text-muted-foreground">Lowercase letters, numbers and hyphens.</p>
          </div>
          {error ? <p className="border-l-2 border-red-500 bg-red-50 px-3 py-2 text-sm text-red-700">{error}</p> : null}
          <Button className="w-full" type="submit" disabled={busy}>
            {busy ? 'Creating…' : 'Create team'}
          </Button>
        </form>
      )}
    </AuthShell>
  )
}
