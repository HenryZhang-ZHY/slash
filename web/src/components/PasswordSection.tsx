import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { api } from '@/lib/api'

interface PasswordSectionProps {
  email: string | null
  onUpdated: () => void
}

export function PasswordSection({ email: existingEmail, onUpdated }: PasswordSectionProps) {
  const { t } = useTranslation()
  const hasPassword = existingEmail !== null
  const [email, setEmail] = useState('')
  const [currentPassword, setCurrentPassword] = useState('')
  const [newPassword, setNewPassword] = useState('')
  const [confirmPassword, setConfirmPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState(false)
  const [busy, setBusy] = useState(false)

  const submit = async (event: React.FormEvent) => {
    event.preventDefault()
    setError(null)
    setSuccess(false)

    if (newPassword !== confirmPassword) {
      setError(t('settings.passwordMismatch'))
      return
    }

    setBusy(true)
    try {
      await api.updatePassword({
        email: hasPassword ? null : email,
        currentPassword: hasPassword ? currentPassword : null,
        newPassword,
      })
      setCurrentPassword('')
      setNewPassword('')
      setConfirmPassword('')
      setSuccess(true)
      onUpdated()
    } catch (requestError) {
      setError(
        requestError instanceof Error ? requestError.message : t('settings.passwordUpdateFailed'),
      )
    } finally {
      setBusy(false)
    }
  }

  return (
    <section>
      <h2 className="mb-1 text-sm font-semibold">{t('settings.passwordTitle')}</h2>
      <p className="mb-3 text-xs text-muted-foreground">
        {hasPassword ? t('settings.passwordDescription') : t('settings.passwordlessDescription')}
      </p>
      <form className="space-y-4 border p-4" onSubmit={submit}>
        {!hasPassword ? (
          <div className="space-y-1.5">
            <Label htmlFor="password-email">{t('settings.loginEmail')}</Label>
            <Input
              id="password-email"
              type="email"
              value={email}
              onChange={(event) => setEmail(event.target.value)}
              autoComplete="username"
              placeholder="you@example.com"
              required
            />
          </div>
        ) : (
          <div className="space-y-1.5">
            <Label htmlFor="current-password">{t('settings.currentPassword')}</Label>
            <Input
              id="current-password"
              type="password"
              value={currentPassword}
              onChange={(event) => setCurrentPassword(event.target.value)}
              autoComplete="current-password"
              required
            />
          </div>
        )}

        <div className="space-y-1.5">
          <Label htmlFor="new-password">{t('settings.newPassword')}</Label>
          <Input
            id="new-password"
            type="password"
            value={newPassword}
            onChange={(event) => setNewPassword(event.target.value)}
            autoComplete="new-password"
            minLength={8}
            required
          />
          <p className="text-xs text-muted-foreground">{t('settings.passwordRequirement')}</p>
        </div>

        <div className="space-y-1.5">
          <Label htmlFor="confirm-password">{t('settings.confirmPassword')}</Label>
          <Input
            id="confirm-password"
            type="password"
            value={confirmPassword}
            onChange={(event) => setConfirmPassword(event.target.value)}
            autoComplete="new-password"
            minLength={8}
            required
          />
        </div>

        {error ? (
          <p className="border-l-2 border-destructive bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {error}
          </p>
        ) : null}
        {success ? (
          <p className="border-l-2 border-emerald-500 bg-emerald-50 px-3 py-2 text-sm text-emerald-700 dark:bg-emerald-950/40 dark:text-emerald-300">
            {t('settings.passwordUpdated')}
          </p>
        ) : null}

        <Button type="submit" disabled={busy}>
          {busy
            ? t('settings.passwordSaving')
            : hasPassword
              ? t('settings.changePassword')
              : t('settings.setPassword')}
        </Button>
      </form>
    </section>
  )
}
