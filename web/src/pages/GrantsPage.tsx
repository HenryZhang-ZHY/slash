import { useCallback, useEffect, useState } from 'react'
import { Plus, Shield, Trash2, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { grantsApi, type CreateGrantInput, type Grant, type OrgMembersResponse } from '@/lib/api'

export function GrantsPage() {
  const { t } = useTranslation()
  const [grants, setGrants] = useState<Grant[]>([])
  const [members, setMembers] = useState<OrgMembersResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)

  const load = useCallback(() => {
    setError(null)
    grantsApi
      .list()
      .then(setGrants)
      .catch((requestError) =>
        setError(requestError instanceof Error ? requestError.message : t('grants.loadFailed')),
      )
    grantsApi
      .orgMembers()
      .then(setMembers)
      .catch(() => {
        /* members only needed when creating; tolerate */
      })
  }, [t])

  useEffect(load, [load])

  const remove = async (grant: Grant) => {
    const confirmed = window.confirm(t('grants.deleteConfirm', { subject: grant.subjectName }))
    if (!confirmed) return
    setError(null)
    try {
      await grantsApi.remove(grant.id)
      setGrants((current) => current.filter((item) => item.id !== grant.id))
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : t('grants.deleteFailed'))
    }
  }

  return (
    <div className="mx-auto w-full max-w-[1680px] px-4 py-6 md:px-8 md:py-8">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold">{t('grants.title')}</h1>
          <p className="mt-1 text-sm text-muted-foreground">{t('grants.subtitle')}</p>
        </div>
        <Button onClick={() => setCreating(true)} disabled={!members}>
          <Plus />
          {t('grants.newGrant')}
        </Button>
      </div>

      {error && <p className="mt-4 text-sm text-red-600">{error}</p>}

      <div className="mt-8 border">
        {grants.length === 0 ? (
          <div className="flex items-center gap-2 px-4 py-10 text-sm text-muted-foreground">
            <Shield className="size-4" />
            {t('grants.empty')}
          </div>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b text-left text-xs text-muted-foreground">
                <th className="px-4 py-2 font-medium">{t('grants.colSubject')}</th>
                <th className="px-4 py-2 font-medium">{t('grants.colScope')}</th>
                <th className="px-4 py-2 font-medium">{t('grants.colRepository')}</th>
                <th className="px-4 py-2 font-medium">{t('grants.colCommand')}</th>
                <th className="px-4 py-2 font-medium">{t('grants.colPermission')}</th>
                <th className="px-4 py-2 font-medium">{t('grants.colEffect')}</th>
                <th className="px-4 py-2 font-medium">{t('grants.colGrantedBy')}</th>
                <th className="px-4 py-2" aria-label={t('grants.colActions')} />
              </tr>
            </thead>
            <tbody>
              {grants.map((grant) => (
                <tr key={grant.id} className="border-b last:border-b-0">
                  <td className="px-4 py-2.5">{grant.subjectName}</td>
                  <td className="px-4 py-2.5 text-muted-foreground">{grant.scope}</td>
                  <td className="px-4 py-2.5 text-muted-foreground">{grant.repository ?? '—'}</td>
                  <td className="px-4 py-2.5 text-muted-foreground">{grant.command ?? '—'}</td>
                  <td className="px-4 py-2.5">{grant.permission}</td>
                  <td className="px-4 py-2.5">
                    <span
                      className={
                        grant.effect === 'deny' ? 'font-semibold text-red-600' : 'text-foreground'
                      }
                    >
                      {grant.effect}
                    </span>
                  </td>
                  <td className="px-4 py-2.5 text-xs text-muted-foreground">
                    {grant.grantedBy ? grant.subjectName : '—'}
                  </td>
                  <td className="px-4 py-2.5 text-right">
                    <Button size="icon" variant="ghost" onClick={() => remove(grant)} aria-label={t('grants.delete')}>
                      <Trash2 className="size-4 text-muted-foreground" />
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {creating && members && (
        <CreateGrantDialog
          members={members}
          onClose={() => setCreating(false)}
          onCreated={(grant) => {
            setGrants((current) => [...current, grant])
            setCreating(false)
          }}
        />
      )}
    </div>
  )
}

function CreateGrantDialog({
  members,
  onClose,
  onCreated,
}: {
  members: OrgMembersResponse
  onClose: () => void
  onCreated: (grant: Grant) => void
}) {
  const { t } = useTranslation()
  const [subjectType, setSubjectType] = useState<'user' | 'team'>('user')
  const [subjectId, setSubjectId] = useState('')
  const [scope, setScope] = useState<'org' | 'repository' | 'command'>('org')
  const [repository, setRepository] = useState('')
  const [command, setCommand] = useState('')
  const [permission, setPermission] = useState<'read' | 'write' | 'admin'>('write')
  const [effect, setEffect] = useState<'allow' | 'deny'>('allow')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const subjects = subjectType === 'user' ? members.users : members.teams

  const submit = async (event: React.FormEvent) => {
    event.preventDefault()
    if (!subjectId) {
      setError(t('grants.subjectRequired'))
      return
    }
    const input: CreateGrantInput = {
      subjectType,
      subjectId,
      scope,
      permission,
      effect,
    }
    if (scope === 'repository' || scope === 'command') {
      if (!repository) {
        setError(t('grants.repositoryRequired'))
        return
      }
      input.repository = repository
    }
    if (scope === 'command') {
      if (!command) {
        setError(t('grants.commandRequired'))
        return
      }
      input.command = command
    }

    setBusy(true)
    setError(null)
    try {
      const created = await grantsApi.create(input)
      onCreated({
        id: created.id,
        subjectType,
        subjectId,
        subjectName:
          subjects.find((subject) => subject.id === subjectId)?.name ?? subjectId,
        scope,
        repository: input.repository ?? null,
        command: input.command ?? null,
        permission,
        effect,
        grantedBy: null,
      })
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : t('grants.createFailed'))
      setBusy(false)
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
      role="dialog"
      aria-modal="true"
      aria-label={t('grants.newGrant')}
    >
      <form onSubmit={submit} className="max-h-[85vh] w-full max-w-lg overflow-y-auto border bg-white p-5">
        <div className="mb-4 flex items-center justify-between">
          <h2 className="text-base font-semibold">{t('grants.newGrant')}</h2>
          <Button type="button" size="icon" variant="ghost" onClick={onClose} aria-label={t('dialog.close')}>
            <X className="size-4" />
          </Button>
        </div>

        <div className="space-y-4">
          <div className="grid grid-cols-2 gap-3">
            <div>
              <Label>{t('grants.subjectType')}</Label>
              <select
                value={subjectType}
                onChange={(event) => {
                  setSubjectType(event.target.value as 'user' | 'team')
                  setSubjectId('')
                }}
                className="mt-1 w-full border bg-white px-3 py-2 text-sm"
              >
                <option value="user">{t('grants.subjectUser')}</option>
                <option value="team">{t('grants.subjectTeam')}</option>
              </select>
            </div>
            <div>
              <Label>{t('grants.subject')}</Label>
              <select
                value={subjectId}
                onChange={(event) => setSubjectId(event.target.value)}
                className="mt-1 w-full border bg-white px-3 py-2 text-sm"
              >
                <option value="">{t('grants.selectSubject')}</option>
                {subjects.map((subject) => (
                  <option key={subject.id} value={subject.id}>
                    {subject.name}
                  </option>
                ))}
              </select>
            </div>
          </div>

          <div className="grid grid-cols-3 gap-3">
            <div>
              <Label>{t('grants.scope')}</Label>
              <select
                value={scope}
                onChange={(event) => setScope(event.target.value as 'org' | 'repository' | 'command')}
                className="mt-1 w-full border bg-white px-3 py-2 text-sm"
              >
                <option value="org">{t('grants.scopeOrg')}</option>
                <option value="repository">{t('grants.scopeRepository')}</option>
                <option value="command">{t('grants.scopeCommand')}</option>
              </select>
            </div>
            <div>
              <Label>{t('grants.permission')}</Label>
              <select
                value={permission}
                onChange={(event) => setPermission(event.target.value as 'read' | 'write' | 'admin')}
                className="mt-1 w-full border bg-white px-3 py-2 text-sm"
              >
                <option value="read">{t('grants.permissionRead')}</option>
                <option value="write">{t('grants.permissionWrite')}</option>
                <option value="admin">{t('grants.permissionAdmin')}</option>
              </select>
            </div>
            <div>
              <Label>{t('grants.effect')}</Label>
              <select
                value={effect}
                onChange={(event) => setEffect(event.target.value as 'allow' | 'deny')}
                className="mt-1 w-full border bg-white px-3 py-2 text-sm"
              >
                <option value="allow">{t('grants.effectAllow')}</option>
                <option value="deny">{t('grants.effectDeny')}</option>
              </select>
            </div>
          </div>

          {(scope === 'repository' || scope === 'command') && (
            <div>
              <Label>{t('grants.repository')}</Label>
              <Input
                value={repository}
                onChange={(event) => setRepository(event.target.value)}
                placeholder="owner/repo"
                className="mt-1"
              />
            </div>
          )}

          {scope === 'command' && (
            <div>
              <Label>{t('grants.command')}</Label>
              <Input
                value={command}
                onChange={(event) => setCommand(event.target.value)}
                placeholder="deploy"
                className="mt-1"
              />
            </div>
          )}

          {error && <p className="text-sm text-red-600">{error}</p>}

          <div className="flex justify-end gap-2 pt-2">
            <Button type="button" variant="outline" onClick={onClose}>
              {t('dialog.cancel')}
            </Button>
            <Button type="submit" disabled={busy}>
              {busy ? t('grants.creating') : t('grants.createGrant')}
            </Button>
          </div>
        </div>
      </form>
    </div>
  )
}
