import { useState } from 'react'
import { Plus, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  testEngineApi,
  type TestSuiteSummary,
} from '@/lib/api'
import { CollectionTokensSection } from '@/components/test-engine/CollectionTokensSection'
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
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const { t } = useTranslation()

  const createSuite = async (event: React.FormEvent) => {
    event.preventDefault()
    setBusy(true)
    setError(null)
    try {
      const result = await testEngineApi.createSuite(owner, repo, suiteKey)
      onCreated(result.suite)
      setCreatedSuite(result.suite)
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : t('dialog.suiteCreateFailed'))
    } finally {
      setBusy(false)
    }
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
        className="h-full w-full max-w-lg overflow-y-auto border-l bg-background shadow-2xl"
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
            {error && <p className="text-sm text-destructive">{error}</p>}
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
              {activeSuite ? <CollectionTokensSection suite={activeSuite} /> : null}
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
            {error && <p className="mt-4 text-sm text-destructive">{error}</p>}
          </div>
        )}
      </section>
    </div>
  )
}
