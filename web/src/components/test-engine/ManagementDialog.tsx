import { useEffect, useState } from 'react'
import { Check, Copy, Eye, EyeOff, KeyRound, Plus, Trash2, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  testEngineApi,
  type CollectionTokenSummary,
  type TestSuiteSummary,
} from '@/lib/api'
import { MetadataRow } from '@/components/test-engine/MetadataRow'

export type ManagementPanel = 'create' | 'settings' | null

export function ManagementDialog({
  mode,
  suite,
  onClose,
  onCreated,
}: {
  mode: Exclude<ManagementPanel, null>
  suite: TestSuiteSummary | null
  onClose: () => void
  onCreated: (suite: TestSuiteSummary) => void
}) {
  const [owner, setOwner] = useState(suite?.owner ?? '')
  const [repo, setRepo] = useState(suite?.repo ?? '')
  const [suiteKey, setSuiteKey] = useState('')
  const [createdSuite, setCreatedSuite] = useState<TestSuiteSummary | null>(null)
  const activeSuite = createdSuite ?? suite
  const [token, setToken] = useState<string | null>(null)
  const [tokens, setTokens] = useState<CollectionTokenSummary[]>([])
  const [tokenVisible, setTokenVisible] = useState(false)
  const [copied, setCopied] = useState(false)
  const [revokingTokenId, setRevokingTokenId] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const { t } = useTranslation()

  useEffect(() => {
    if (mode !== 'settings' || !suite) return
    testEngineApi
      .listTokens(suite.id)
      .then(setTokens)
      .catch((requestError) =>
        setError(requestError instanceof Error ? requestError.message : t('dialog.tokenLoadFailed')),
      )
  }, [mode, suite, t])

  const createSuite = async (event: React.FormEvent) => {
    event.preventDefault()
    setBusy(true)
    setError(null)
    try {
      const result = await testEngineApi.createSuite(owner, repo, suiteKey)
      onCreated(result.suite)
      setCreatedSuite(result.suite)
      setToken(result.token)
      setTokenVisible(true)
      setTokens(await testEngineApi.listTokens(result.suite.id))
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : t('dialog.suiteCreateFailed'))
    } finally {
      setBusy(false)
    }
  }

  const issueToken = async () => {
    if (!activeSuite) return
    if (
      tokens.some((item) => item.status === 'active') &&
      !window.confirm(t('dialog.confirmRotateToken'))
    ) {
      return
    }
    setBusy(true)
    setError(null)
    try {
      const result = await testEngineApi.issueToken(activeSuite.id)
      setToken(result.token)
      setTokenVisible(true)
      setTokens(await testEngineApi.listTokens(activeSuite.id))
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : t('dialog.tokenGenerateFailed'))
    } finally {
      setBusy(false)
    }
  }

  const revokeToken = async (tokenId: string) => {
    if (!activeSuite) return
    if (!window.confirm(t('dialog.confirmRevokeToken'))) return
    setRevokingTokenId(tokenId)
    setError(null)
    try {
      await testEngineApi.revokeToken(activeSuite.id, tokenId)
      setTokens(await testEngineApi.listTokens(activeSuite.id))
      setToken(null)
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : t('dialog.tokenRevokeFailed'))
    } finally {
      setRevokingTokenId(null)
    }
  }

  const copyToken = async () => {
    if (!token) return
    await navigator.clipboard.writeText(token)
    setCopied(true)
    window.setTimeout(() => setCopied(false), 1500)
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-end bg-black/20"
      role="presentation"
      onMouseDown={onClose}
    >
      <section
        role="dialog"
        aria-modal="true"
        aria-label={mode === 'create' ? t('dialog.createSuiteAria') : t('dialog.settingsAria')}
        className="h-full w-full max-w-lg overflow-y-auto border-l bg-white shadow-2xl"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="flex h-14 items-center justify-between border-b px-5">
          <div>
            <div className="text-sm font-semibold">
              {mode === 'create' ? t('dialog.createSuite') : t('dialog.suiteSettings')}
            </div>
            {activeSuite && (
              <div className="text-xs text-muted-foreground">
                {activeSuite.owner}/{activeSuite.repo} · {activeSuite.suite_key}
              </div>
            )}
          </div>
          <Button size="icon" variant="ghost" onClick={onClose} aria-label={t('dialog.close')}>
            <X />
          </Button>
        </div>

        {mode === 'create' && !createdSuite ? (
          <form onSubmit={createSuite} className="space-y-5 p-5">
            <div className="space-y-1.5">
              <Label htmlFor="create-owner">{t('dialog.githubOwner')}</Label>
              <Input
                id="create-owner"
                value={owner}
                onChange={(event) => setOwner(event.target.value)}
                placeholder="acme"
                required
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="create-repo">{t('dialog.repository')}</Label>
              <Input
                id="create-repo"
                value={repo}
                onChange={(event) => setRepo(event.target.value)}
                placeholder="widgets"
                required
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="create-key">{t('dialog.suiteKey')}</Label>
              <Input
                id="create-key"
                value={suiteKey}
                onChange={(event) => setSuiteKey(event.target.value)}
                placeholder="ci-test"
                required
              />
            </div>
            {error && <p className="text-sm text-red-600">{error}</p>}
            <div className="flex justify-end gap-2 border-t pt-4">
              <Button type="button" variant="ghost" onClick={onClose}>
                {t('dialog.cancel')}
              </Button>
              <Button type="submit" disabled={busy}>
                <Plus />
                {busy ? t('dialog.creating') : t('dialog.createSuite')}
              </Button>
            </div>
          </form>
        ) : (
          <div className="p-5">
            <div className="border-b pb-6">
              <h3 className="text-sm font-semibold">{t('dialog.collectionToken')}</h3>
              <p className="mt-1 text-xs text-muted-foreground">
                {t('dialog.collectionTokenHint')}
              </p>
              {token ? (
                <div className="mt-4 rounded-md border border-amber-300 bg-amber-50 p-3">
                  <p className="mb-2 text-xs text-amber-900">{t('dialog.tokenShowOnce')}</p>
                  <div className="flex items-center gap-2">
                    <Input
                      className="font-mono"
                      type={tokenVisible ? 'text' : 'password'}
                      value={token}
                      readOnly
                    />
                    <Button
                      size="icon"
                      variant="outline"
                      onClick={() => setTokenVisible((visible) => !visible)}
                      aria-label={tokenVisible ? t('dialog.hideToken') : t('dialog.showToken')}
                    >
                      {tokenVisible ? <EyeOff /> : <Eye />}
                    </Button>
                    <Button
                      size="icon"
                      variant="outline"
                      onClick={copyToken}
                      aria-label={t('dialog.copyToken')}
                    >
                      {copied ? <Check /> : <Copy />}
                    </Button>
                  </div>
                </div>
              ) : (
                <p className="mt-4 rounded-md bg-zinc-50 p-3 text-xs text-muted-foreground">
                  {t('dialog.tokenNotRecoverable')}
                </p>
              )}
              <Button className="mt-3" variant="outline" onClick={issueToken} disabled={busy}>
                <KeyRound />
                {busy
                  ? t('dialog.generating')
                  : tokens.some((item) => item.status === 'active')
                    ? t('dialog.rotateToken')
                    : t('dialog.generateNewToken')}
              </Button>
            </div>
            <div className="border-b py-6">
              <h3 className="text-sm font-semibold">{t('dialog.tokenHistory')}</h3>
              <div className="mt-3 space-y-2">
                {tokens.length ? (
                  tokens.map((item) => (
                    <div key={item.id} className="flex items-start justify-between gap-3 border p-3 text-xs">
                      <div className="min-w-0 space-y-1">
                        <div className="flex items-center gap-2">
                          <code>{item.id.slice(0, 8)}</code>
                          <span>{item.status}</span>
                        </div>
                        <p className="text-muted-foreground">
                          {t('dialog.expires')}: {new Date(item.expires_at).toLocaleString()}
                        </p>
                        <p className="text-muted-foreground">
                          {t('dialog.lastUsed')}:{' '}
                          {item.last_used_at
                            ? new Date(item.last_used_at).toLocaleString()
                            : t('dialog.never')}
                        </p>
                      </div>
                      {item.status === 'active' ? (
                        <Button
                          size="icon"
                          variant="ghost"
                          onClick={() => void revokeToken(item.id)}
                          disabled={revokingTokenId === item.id}
                          aria-label={t('dialog.revokeToken')}
                        >
                          <Trash2 />
                        </Button>
                      ) : null}
                    </div>
                  ))
                ) : (
                  <p className="text-xs text-muted-foreground">{t('dialog.noTokens')}</p>
                )}
              </div>
            </div>
            <div className="pt-6">
              <h3 className="text-sm font-semibold">{t('dialog.collectorEndpoints')}</h3>
              <dl className="mt-3 divide-y border-y">
                <MetadataRow label={t('dialog.generic')}>
                  <code>/v1/test-engine/upload</code>
                </MetadataRow>
                <MetadataRow label={t('dialog.cargo')}>
                  <code>/v1/test-engine/upload/cargo</code>
                </MetadataRow>
                <MetadataRow label={t('dialog.vitest')}>
                  <code>/v1/test-engine/upload/vitest</code>
                </MetadataRow>
              </dl>
            </div>
            {error && <p className="mt-4 text-sm text-red-600">{error}</p>}
          </div>
        )}
      </section>
    </div>
  )
}
