import { useEffect, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'

import { AuthShell } from '@/components/AuthShell'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { api } from '@/lib/api'
import { destinationFor, githubErrorKey } from '@/lib/authFlow'

export function LoginPage() {
  const [mode, setMode] = useState<'login' | 'register'>('login')
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const { t } = useTranslation()
  const [githubError] = useState(() => searchParams.get('github_error'))

  useEffect(() => {
    if (searchParams.has('github_error')) {
      setSearchParams({}, { replace: true })
    }
  }, [searchParams, setSearchParams])

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    setError(null)
    setBusy(true)
    try {
      if (mode === 'register') {
        await api.register(email, password)
        navigate('/onboarding')
      } else {
        await api.login(email, password)
        navigate(destinationFor(await api.me()))
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : t('auth.requestFailed'))
    } finally {
      setBusy(false)
    }
  }

  return (
    <AuthShell
      title={mode === 'login' ? t('auth.signInTitle') : t('auth.createTitle')}
      description={mode === 'login' ? t('auth.signInDescription') : t('auth.createDescription')}
    >
      <div className="space-y-4">
        {githubError ? (
          <p className="border-l-2 border-red-500 bg-red-50 px-3 py-2 text-sm text-red-700">
            {t(githubErrorKey(githubError))}
          </p>
        ) : null}

        <a href="/api/auth/github/sign-in" className="block">
          <Button variant="outline" className="w-full" type="button">
            <svg className="mr-2 size-4" viewBox="0 0 24 24" fill="currentColor">
              <path d="M12 0c-6.626 0-12 5.373-12 12 0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576 4.765-1.589 8.199-6.086 8.199-11.386 0-6.627-5.373-12-12-12z"/>
            </svg>
            {t('auth.signInWithGitHub')}
          </Button>
        </a>

        <div className="relative">
          <div className="absolute inset-0 flex items-center">
            <span className="w-full border-t" />
          </div>
          <div className="relative flex justify-center text-xs uppercase">
            <span className="bg-white px-2 text-muted-foreground">{t('auth.or')}</span>
          </div>
        </div>

        <form onSubmit={submit} className="space-y-4">
          <div className="space-y-1.5">
            <Label htmlFor="email">{t('auth.email')}</Label>
            <Input
              id="email"
              type="email"
              value={email}
              onChange={(event) => setEmail(event.target.value)}
              autoComplete="username"
              placeholder="you@example.com"
              required
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="password">{t('auth.password')}</Label>
            <Input
              id="password"
              type="password"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              autoComplete={mode === 'login' ? 'current-password' : 'new-password'}
              required
              minLength={8}
            />
          </div>
          {error ? <p className="border-l-2 border-red-500 bg-red-50 px-3 py-2 text-sm text-red-700">{error}</p> : null}
          <Button className="w-full" type="submit" disabled={busy}>
            {busy ? t('auth.pleaseWait') : mode === 'login' ? t('auth.signIn') : t('auth.createAccount')}
          </Button>
          <div className="border-t pt-3 text-center">
            <Button
              type="button"
              variant="link"
              onClick={() => setMode(mode === 'login' ? 'register' : 'login')}
            >
              {mode === 'login' ? t('auth.createAccountLink') : t('auth.backToSignIn')}
            </Button>
          </div>
        </form>
      </div>
    </AuthShell>
  )
}
