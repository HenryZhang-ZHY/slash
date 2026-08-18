import { useCallback, useEffect, useState, type FormEvent } from 'react'
import { Check, Copy, KeyRound, Plus, Trash2 } from 'lucide-react'
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { accessTokenApi, type AccessToken, type IssuedAccessToken } from '@/lib/api'

function formatDate(value: string | null, language: string, fallback: string) {
  if (!value) return fallback
  return new Intl.DateTimeFormat(language, { dateStyle: 'medium', timeStyle: 'short' }).format(
    new Date(value),
  )
}

export function AccessTokensSection() {
  const { t, i18n } = useTranslation()
  const [tokens, setTokens] = useState<AccessToken[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [createOpen, setCreateOpen] = useState(false)
  const [name, setName] = useState('')
  const [expiry, setExpiry] = useState('90')
  const [creating, setCreating] = useState(false)
  const [issued, setIssued] = useState<IssuedAccessToken | null>(null)
  const [copied, setCopied] = useState(false)

  const load = useCallback(async () => {
    try {
      setError(null)
      setTokens(await accessTokenApi.list())
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : t('tokens.loadFailed'))
    }
  }, [t])

  useEffect(() => {
    void load()
  }, [load])

  const closeCreate = (open: boolean) => {
    setCreateOpen(open)
    if (!open) {
      setName('')
      setExpiry('90')
      setIssued(null)
      setCopied(false)
    }
  }

  const createToken = async (event: FormEvent) => {
    event.preventDefault()
    setCreating(true)
    setError(null)
    try {
      const result = await accessTokenApi.create(
        name,
        expiry === 'none' ? undefined : Number(expiry),
      )
      setIssued(result)
      setTokens((current) => [result.accessToken, ...(current ?? [])])
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : t('tokens.createFailed'))
    } finally {
      setCreating(false)
    }
  }

  const revokeToken = async (id: string) => {
    try {
      setError(null)
      await accessTokenApi.revoke(id)
      setTokens((current) => current?.filter((token) => token.id !== id) ?? [])
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : t('tokens.revokeFailed'))
    }
  }

  const copyToken = async () => {
    if (!issued) return
    await navigator.clipboard.writeText(issued.token)
    setCopied(true)
  }

  return (
    <section>
      <div className="mb-3 flex items-end justify-between gap-4">
        <div>
          <h2 className="text-sm font-semibold">{t('tokens.title')}</h2>
          <p className="mt-1 text-xs text-muted-foreground">{t('tokens.description')}</p>
        </div>
        <Button size="sm" onClick={() => setCreateOpen(true)}>
          <Plus />
          {t('tokens.create')}
        </Button>
      </div>

      {error ? <div className="mb-3 border-l-2 border-red-500 bg-red-50 px-3 py-2 text-sm text-red-700">{error}</div> : null}

      <div className="border">
        {tokens === null ? (
          <div className="px-4 py-8 text-sm text-muted-foreground">{t('tokens.loading')}</div>
        ) : tokens.length === 0 ? (
          <div className="flex flex-col items-center px-4 py-10 text-center">
            <KeyRound className="mb-3 size-5 text-muted-foreground" />
            <div className="text-sm font-medium">{t('tokens.empty')}</div>
            <div className="mt-1 max-w-sm text-xs text-muted-foreground">{t('tokens.emptyHint')}</div>
          </div>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t('tokens.name')}</TableHead>
                <TableHead>{t('tokens.created')}</TableHead>
                <TableHead>{t('tokens.lastUsed')}</TableHead>
                <TableHead>{t('tokens.expires')}</TableHead>
                <TableHead className="w-16"><span className="sr-only">{t('tokens.actions')}</span></TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {tokens.map((token) => {
                const expired = token.expiresAt !== null && new Date(token.expiresAt) <= new Date()
                return (
                  <TableRow key={token.id}>
                    <TableCell className="font-medium">{token.name}</TableCell>
                    <TableCell>{formatDate(token.createdAt, i18n.language, t('tokens.never'))}</TableCell>
                    <TableCell>{formatDate(token.lastUsedAt, i18n.language, t('tokens.neverUsed'))}</TableCell>
                    <TableCell>
                      {expired ? (
                        <Badge variant="destructive">{t('tokens.expired')}</Badge>
                      ) : token.expiresAt ? (
                        formatDate(token.expiresAt, i18n.language, t('tokens.never'))
                      ) : (
                        <Badge variant="outline">{t('tokens.never')}</Badge>
                      )}
                    </TableCell>
                    <TableCell>
                      <AlertDialog>
                        <AlertDialogTrigger render={<Button variant="ghost" size="icon-sm" aria-label={t('tokens.revoke')} />}>
                          <Trash2 />
                        </AlertDialogTrigger>
                        <AlertDialogContent>
                          <AlertDialogHeader>
                            <AlertDialogTitle>{t('tokens.revokeTitle')}</AlertDialogTitle>
                            <AlertDialogDescription>
                              {t('tokens.revokeDescription', { name: token.name })}
                            </AlertDialogDescription>
                          </AlertDialogHeader>
                          <AlertDialogFooter>
                            <AlertDialogCancel>{t('tokens.cancel')}</AlertDialogCancel>
                            <AlertDialogAction variant="destructive" onClick={() => void revokeToken(token.id)}>
                              {t('tokens.revoke')}
                            </AlertDialogAction>
                          </AlertDialogFooter>
                        </AlertDialogContent>
                      </AlertDialog>
                    </TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
        )}
      </div>

      <Dialog open={createOpen} onOpenChange={closeCreate}>
        <DialogContent className="sm:max-w-md">
          {issued ? (
            <>
              <DialogHeader>
                <DialogTitle>{t('tokens.createdTitle')}</DialogTitle>
                <DialogDescription>{t('tokens.createdDescription')}</DialogDescription>
              </DialogHeader>
              <div className="border-l-2 border-amber-500 bg-amber-50 px-3 py-2 text-sm text-amber-900">
                {t('tokens.copyWarning')}
              </div>
              <div className="flex gap-2">
                <Input className="font-mono" value={issued.token} readOnly aria-label={t('tokens.token')} />
                <Button variant="outline" size="icon" onClick={() => void copyToken()} aria-label={t('tokens.copy')}>
                  {copied ? <Check /> : <Copy />}
                </Button>
              </div>
              <DialogFooter>
                <Button onClick={() => closeCreate(false)}>{t('tokens.done')}</Button>
              </DialogFooter>
            </>
          ) : (
            <form onSubmit={createToken}>
              <DialogHeader>
                <DialogTitle>{t('tokens.createTitle')}</DialogTitle>
                <DialogDescription>{t('tokens.createDescription')}</DialogDescription>
              </DialogHeader>
              <div className="my-5 space-y-4">
                <div className="space-y-2">
                  <Label htmlFor="access-token-name">{t('tokens.name')}</Label>
                  <Input
                    id="access-token-name"
                    value={name}
                    maxLength={100}
                    placeholder={t('tokens.namePlaceholder')}
                    onChange={(event) => setName(event.target.value)}
                    required
                    autoFocus
                  />
                </div>
                <div className="space-y-2">
                  <Label>{t('tokens.expiry')}</Label>
                  <Select value={expiry} onValueChange={(value) => setExpiry(value ?? '90')}>
                    <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
                    <SelectContent>
                      <SelectItem value="30">{t('tokens.days', { count: 30 })}</SelectItem>
                      <SelectItem value="90">{t('tokens.days', { count: 90 })}</SelectItem>
                      <SelectItem value="365">{t('tokens.days', { count: 365 })}</SelectItem>
                      <SelectItem value="none">{t('tokens.noExpiry')}</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
              </div>
              <DialogFooter>
                <Button type="button" variant="outline" onClick={() => closeCreate(false)}>{t('tokens.cancel')}</Button>
                <Button type="submit" disabled={creating || name.trim().length === 0}>
                  {creating ? t('tokens.creating') : t('tokens.create')}
                </Button>
              </DialogFooter>
            </form>
          )}
        </DialogContent>
      </Dialog>
    </section>
  )
}
