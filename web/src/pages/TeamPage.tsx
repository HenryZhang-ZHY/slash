import { useCallback, useEffect, useState } from 'react'
import { Mail, RefreshCw, Trash2, UserPlus, Users } from 'lucide-react'
import { Navigate, useOutletContext, useParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'

import { type DashboardContext } from '@/components/AppShell'
import { StatePanel } from '@/components/StatePanel'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { teamApi, type TeamRole, type TeamRoster } from '@/lib/api'
import { requestErrorKey } from '@/lib/requestError'

export function TeamPage() {
  const { me, refreshMe } = useOutletContext<DashboardContext>()
  const { slug } = useParams()
  const { t } = useTranslation()
  const team = me.teams.find((candidate) => candidate.slug === slug)
  const [roster, setRoster] = useState<TeamRoster | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [email, setEmail] = useState('')
  const [role, setRole] = useState<TeamRole>('member')
  const [busy, setBusy] = useState(false)

  const load = useCallback(() => {
    if (!team) return
    setError(null)
    teamApi.roster(team.id).then(setRoster).catch((requestError) => {
      setError(t(requestErrorKey(requestError, 'team.loadFailed')))
    })
  }, [team, t])
  useEffect(load, [load])
  if (!team) return <Navigate to="/" replace />

  const mutate = async (operation: () => Promise<unknown>) => {
    setError(null)
    setBusy(true)
    try {
      await operation()
      load()
      refreshMe()
    } catch (requestError) {
      setError(t(requestErrorKey(requestError, 'team.updateFailed')))
    } finally {
      setBusy(false)
    }
  }
  const submitInvite = (event: React.FormEvent) => {
    event.preventDefault()
    void mutate(async () => {
      await teamApi.invite(team.id, email, role)
      setEmail('')
    })
  }

  return <div className="mx-auto w-full max-w-5xl px-4 py-6 md:px-8 md:py-8">
    <div className="flex items-end justify-between gap-4"><div><h1 className="text-2xl font-semibold">{team.name}</h1><p className="mt-1 text-sm text-muted-foreground">{t('team.subtitle', { slug: team.slug })}</p></div><Badge variant="outline">{t(`team.role.${team.role}`)}</Badge></div>
    {error ? <div className="mt-5"><StatePanel kind="error" title={t('team.updateFailed')} description={error} retry={load} /></div> : null}
    {roster?.viewerRole === 'maintainer' ? <section className="mt-8 border p-4"><div className="mb-4 flex items-center gap-2"><UserPlus className="size-4" /><h2 className="font-semibold">{t('team.inviteTitle')}</h2></div><form className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_10rem_auto] sm:items-end" onSubmit={submitInvite}><div className="space-y-1.5"><Label htmlFor="invite-email">{t('team.email')}</Label><Input id="invite-email" type="email" value={email} onChange={(event) => setEmail(event.target.value)} placeholder="person@example.com" required /></div><div className="space-y-1.5"><Label htmlFor="invite-role">{t('team.roleLabel')}</Label><select id="invite-role" className="h-9 w-full rounded-md border bg-background px-3 text-sm" value={role} onChange={(event) => setRole(event.target.value as TeamRole)}><option value="member">{t('team.role.member')}</option><option value="maintainer">{t('team.role.maintainer')}</option></select></div><Button type="submit" disabled={busy}><Mail />{busy ? t('team.sending') : t('team.sendInvite')}</Button></form><p className="mt-2 text-xs text-muted-foreground">{t('team.inviteHint')}</p></section> : null}
    <section className="mt-8"><div className="mb-3 flex items-center gap-2"><Users className="size-4" /><h2 className="font-semibold">{t('team.members', { count: roster?.members.length ?? 0 })}</h2></div>{!roster ? <StatePanel title={t('common.loading')} /> : <div className="divide-y border">{roster.members.map((member) => <div key={member.userId} className="flex flex-wrap items-center justify-between gap-3 px-4 py-3"><div className="min-w-0"><div className="truncate text-sm font-medium">{member.displayName || member.email || t('team.unnamedUser')}</div><div className="truncate text-xs text-muted-foreground">{member.email ?? member.userId}</div></div><div className="flex items-center gap-2">{roster.viewerRole === 'maintainer' ? <select aria-label={t('team.roleLabel')} className="h-8 rounded-md border bg-background px-2 text-xs" value={member.role} disabled={busy} onChange={(event) => void mutate(() => teamApi.updateMember(team.id, member.userId, event.target.value as TeamRole))}><option value="member">{t('team.role.member')}</option><option value="maintainer">{t('team.role.maintainer')}</option></select> : <Badge variant="secondary">{t(`team.role.${member.role}`)}</Badge>}{roster.viewerRole === 'maintainer' ? <Button variant="ghost" size="icon-sm" aria-label={t('team.removeMember')} disabled={busy} onClick={() => { if (window.confirm(t('team.removeConfirm'))) void mutate(() => teamApi.removeMember(team.id, member.userId)) }}><Trash2 /></Button> : null}</div></div>)}</div>}</section>
    {roster?.viewerRole === 'maintainer' ? <section className="mt-8"><h2 className="mb-3 font-semibold">{t('team.pendingInvites', { count: roster.invitations.length })}</h2>{roster.invitations.length === 0 ? <StatePanel title={t('team.noPendingInvites')} description={t('team.noPendingInvitesDescription')} /> : <div className="divide-y border">{roster.invitations.map((invitation) => <div key={invitation.id} className="flex flex-wrap items-center justify-between gap-3 px-4 py-3"><div><div className="text-sm font-medium">{invitation.email}</div><div className="text-xs text-muted-foreground">{t(`team.role.${invitation.role}`)} · {t('team.expires', { date: new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' }).format(new Date(invitation.expiresAt)) })}</div></div><div className="flex gap-1"><Button variant="ghost" size="sm" disabled={busy} onClick={() => void mutate(() => teamApi.resend(team.id, invitation.id))}><RefreshCw />{t('team.resend')}</Button><Button variant="ghost" size="icon-sm" aria-label={t('team.revoke')} disabled={busy} onClick={() => void mutate(() => teamApi.revoke(team.id, invitation.id))}><Trash2 /></Button></div></div>)}</div>}</section> : null}
  </div>
}
