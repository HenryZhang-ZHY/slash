import { useEffect, useState } from 'react'
import { MailCheck } from 'lucide-react'
import { useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'

import { AuthShell } from '@/components/AuthShell'
import { Button } from '@/components/ui/button'
import { api, teamApi, type InvitationPreview } from '@/lib/api'
import { clearPendingInvitation, invitationTokenFromLocation } from '@/lib/pendingInvitation'

export function InvitationPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [token] = useState(invitationTokenFromLocation)
  const [preview, setPreview] = useState<InvitationPreview | null>(null)
  const [signedIn, setSignedIn] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (!token) {
      setError(t('invite.missing'))
      return
    }
    teamApi.previewInvitation(token).then(setPreview).catch((requestError) => {
      setError(requestError instanceof Error ? requestError.message : t('invite.invalid'))
    })
    api.me().then(() => setSignedIn(true)).catch(() => setSignedIn(false))
  }, [t, token])

  const accept = async () => {
    if (!token) return
    setBusy(true)
    setError(null)
    try {
      const { teamSlug } = await teamApi.acceptInvitation(token)
      clearPendingInvitation()
      navigate(`/teams/${teamSlug}`)
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : t('invite.invalid'))
    } finally {
      setBusy(false)
    }
  }

  return (
    <AuthShell title={t('invite.title')} description={preview ? t('invite.description', { team: preview.teamName }) : t('invite.loading')}>
      <div className="space-y-5">
        {preview ? <div className="border-y py-4"><div className="flex items-center gap-3"><div className="flex size-10 items-center justify-center rounded-full bg-muted"><MailCheck className="size-5" /></div><div><div className="font-medium">{preview.teamName}</div><div className="text-xs text-muted-foreground">{preview.email} · {t(`team.role.${preview.role}`)}</div></div></div></div> : null}
        {error ? <p className="border-l-2 border-destructive bg-destructive/10 px-3 py-2 text-sm text-destructive">{error}</p> : null}
        {preview && signedIn ? <Button className="w-full" disabled={busy} onClick={() => void accept()}>{busy ? t('invite.accepting') : t('invite.accept')}</Button> : null}
        {preview && !signedIn ? <div className="space-y-2"><Button className="w-full" onClick={() => navigate('/login')}>{t('invite.signIn')}</Button><a href="/api/auth/github/sign-in" className="block"><Button className="w-full" variant="outline" type="button">{t('auth.signInWithGitHub')}</Button></a><p className="text-center text-xs text-muted-foreground">{t('invite.accountHint')}</p></div> : null}
      </div>
    </AuthShell>
  )
}
