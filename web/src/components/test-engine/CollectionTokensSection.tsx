import { useCallback, useEffect, useState } from 'react'
import { KeyRound, Plus, RotateCw, Trash2 } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { testEngineApi, type CollectionTokenSummary, type TestSuiteSummary } from '@/lib/api'
import { CollectionTokenDialog } from './CollectionTokenDialog'

function formatDate(value: string | null, language: string, fallback: string) {
  if (!value) return fallback
  return new Intl.DateTimeFormat(language, { dateStyle: 'medium', timeStyle: 'short' }).format(
    new Date(value),
  )
}

function TokenStatus({ status }: { status: CollectionTokenSummary['status'] }) {
  const { t } = useTranslation()
  if (status === 'active') return <Badge>{t('dialog.tokenActive')}</Badge>
  if (status === 'expired') {
    return <Badge variant="destructive">{t('dialog.tokenExpired')}</Badge>
  }
  return <Badge variant="secondary">{t('dialog.tokenRevoked')}</Badge>
}

export function CollectionTokensSection({ suite }: { suite: TestSuiteSummary }) {
  const { t, i18n } = useTranslation()
  const [tokens, setTokens] = useState<CollectionTokenSummary[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [rotating, setRotating] = useState(false)
  const [initialName, setInitialName] = useState('')
  const [revokingTokenId, setRevokingTokenId] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      setError(null)
      setTokens(await testEngineApi.listTokens(suite.id))
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : t('dialog.tokenLoadFailed'))
    }
  }, [suite.id, t])

  useEffect(() => {
    void load()
  }, [load])

  const openCreate = () => {
    setRotating(false)
    setInitialName('')
    setDialogOpen(true)
  }

  const openRotate = (token: CollectionTokenSummary) => {
    setRotating(true)
    setInitialName(token.name)
    setDialogOpen(true)
  }

  const createToken = async (name: string, expiresAt: string | null) => {
    setError(null)
    const result = await testEngineApi.issueToken(suite.id, name, expiresAt)
    await load()
    return result.token
  }

  const revokeToken = async (tokenId: string) => {
    setRevokingTokenId(tokenId)
    setError(null)
    try {
      await testEngineApi.revokeToken(suite.id, tokenId)
      await load()
    } catch (requestError) {
      setError(
        requestError instanceof Error ? requestError.message : t('dialog.tokenRevokeFailed'),
      )
    } finally {
      setRevokingTokenId(null)
    }
  }

  return (
    <section>
      <div className="flex items-end justify-between gap-4">
        <div>
          <h3 className="text-sm font-semibold">{t('dialog.collectionTokens')}</h3>
          <p className="mt-1 text-xs text-muted-foreground">
            {t('dialog.collectionTokenHint')}
          </p>
        </div>
        <Button size="sm" onClick={openCreate}>
          <Plus />
          {t('dialog.createToken')}
        </Button>
      </div>

      {error ? <p className="mt-4 text-sm text-destructive">{error}</p> : null}

      <div className="mt-4 space-y-3">
        {tokens === null ? (
          <p className="border p-4 text-xs text-muted-foreground">{t('dialog.loadingTokens')}</p>
        ) : tokens.length === 0 ? (
          <div className="flex flex-col items-center border px-4 py-8 text-center">
            <KeyRound className="mb-3 size-5 text-muted-foreground" />
            <div className="text-sm font-medium">{t('dialog.noTokens')}</div>
            <p className="mt-1 max-w-sm text-xs text-muted-foreground">
              {t('dialog.noTokensHint')}
            </p>
          </div>
        ) : (
          tokens.map((token) => (
            <article key={token.id} className="border p-3">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="truncate text-sm font-medium">{token.name}</span>
                    <TokenStatus status={token.status} />
                  </div>
                  <code className="mt-1 block text-xs text-muted-foreground">
                    {token.id.slice(0, 8)}
                  </code>
                </div>
                <div className="flex shrink-0 gap-1">
                  {token.status === 'active' ? (
                    <>
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => openRotate(token)}
                        aria-label={t('dialog.rotateNamedToken', { name: token.name })}
                      >
                        <RotateCw />
                        {t('dialog.rotateToken')}
                      </Button>
                      <AlertDialog>
                        <AlertDialogTrigger
                          render={
                            <Button
                              size="icon-sm"
                              variant="ghost"
                              aria-label={t('dialog.revokeNamedToken', { name: token.name })}
                            />
                          }
                        >
                          <Trash2 />
                        </AlertDialogTrigger>
                        <AlertDialogContent>
                          <AlertDialogHeader>
                            <AlertDialogTitle>{t('dialog.revokeTokenTitle')}</AlertDialogTitle>
                            <AlertDialogDescription>
                              {t('dialog.revokeTokenDescription', { name: token.name })}
                            </AlertDialogDescription>
                          </AlertDialogHeader>
                          <AlertDialogFooter>
                            <AlertDialogCancel>{t('dialog.cancel')}</AlertDialogCancel>
                            <AlertDialogAction
                              variant="destructive"
                              disabled={revokingTokenId === token.id}
                              onClick={() => void revokeToken(token.id)}
                            >
                              {t('dialog.revokeToken')}
                            </AlertDialogAction>
                          </AlertDialogFooter>
                        </AlertDialogContent>
                      </AlertDialog>
                    </>
                  ) : null}
                </div>
              </div>
              <dl className="mt-3 grid grid-cols-2 gap-3 text-xs">
                <div>
                  <dt className="text-muted-foreground">{t('dialog.lastUsed')}</dt>
                  <dd className="mt-1">
                    {formatDate(token.last_used_at, i18n.language, t('dialog.neverUsed'))}
                  </dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">{t('dialog.expires')}</dt>
                  <dd className="mt-1">
                    {formatDate(token.expires_at, i18n.language, t('dialog.neverExpires'))}
                  </dd>
                </div>
              </dl>
            </article>
          ))
        )}
      </div>

      <CollectionTokenDialog
        open={dialogOpen}
        rotating={rotating}
        initialName={initialName}
        onOpenChange={setDialogOpen}
        onCreate={createToken}
      />
    </section>
  )
}
