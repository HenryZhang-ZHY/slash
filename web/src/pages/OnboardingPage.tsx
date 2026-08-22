import { useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'

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
  const { t } = useTranslation()

  // Redirect unauthenticated users to login.
  useEffect(() => {
    api
      .me()
      .then((me) => {
        if (me.teams.length > 0) navigate('/', { replace: true })
      })
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
        setError(err instanceof Error ? err.message : t('onboard.createFailed'))
      } finally {
        setBusy(false)
      }
    },
    [name, slug, t],
  )

  if (checking) return null

  return (
    <AuthShell
      title={done ? t('onboard.readyTitle') : t('onboard.createTitle')}
      description={
        done
          ? t('onboard.readyDescription')
          : t('onboard.createDescription')
      }
    >
      {done ? (
        <div className="space-y-5">
          <div className="border-y py-4 text-sm">
            <div className="font-medium">{name}</div>
            <div className="mt-1 text-xs text-muted-foreground">{slugify(slug || name)}</div>
          </div>
          <Button className="w-full" onClick={() => navigate('/')}>
            {t('onboard.enterWorkspace')}
          </Button>
        </div>
      ) : (
        <form onSubmit={onSubmit} className="space-y-4">
          <div className="space-y-1.5">
            <Label htmlFor="name">{t('onboard.teamName')}</Label>
            <Input
              id="name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="Acme Engineering"
              required
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="slug">{t('onboard.teamSlug')}</Label>
            <Input
              id="slug"
              value={slug}
              onChange={(event) => setSlug(slugify(event.target.value))}
              placeholder="acme-engineering"
              pattern="[a-z0-9-]{1,63}"
              title={t('onboard.slugPattern')}
              required
            />
            <p className="text-xs text-muted-foreground">{t('onboard.slugHint')}</p>
          </div>
          {error ? <p className="border-l-2 border-destructive bg-destructive/10 px-3 py-2 text-sm text-destructive">{error}</p> : null}
          <Button className="w-full" type="submit" disabled={busy}>
            {busy ? t('onboard.creating') : t('onboard.createTeam')}
          </Button>
        </form>
      )}
    </AuthShell>
  )
}
