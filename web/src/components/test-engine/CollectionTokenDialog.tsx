import { useEffect, useState, type FormEvent } from 'react'
import { Check, Copy } from 'lucide-react'
import { useTranslation } from 'react-i18next'

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
  resolveCollectionTokenExpiry,
  type CollectionTokenExpiry,
} from './collectionTokenExpiry'

export function CollectionTokenDialog({
  open,
  rotating,
  initialName,
  onOpenChange,
  onCreate,
}: {
  open: boolean
  rotating: boolean
  initialName: string
  onOpenChange: (open: boolean) => void
  onCreate: (name: string, expiresAt: string | null) => Promise<string>
}) {
  const { t } = useTranslation()
  const [name, setName] = useState(initialName)
  const [expiry, setExpiry] = useState<CollectionTokenExpiry>('none')
  const [customExpiry, setCustomExpiry] = useState('')
  const [issuedToken, setIssuedToken] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)
  const [copied, setCopied] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!open) return
    setName(initialName)
    setExpiry('none')
    setCustomExpiry('')
    setIssuedToken(null)
    setCopied(false)
    setError(null)
  }, [initialName, open])

  const createToken = async (event: FormEvent) => {
    event.preventDefault()
    setCreating(true)
    setError(null)
    try {
      const expiresAt = resolveCollectionTokenExpiry(expiry, customExpiry)
      setIssuedToken(await onCreate(name, expiresAt))
    } catch (requestError) {
      setError(
        requestError instanceof Error ? requestError.message : t('dialog.tokenGenerateFailed'),
      )
    } finally {
      setCreating(false)
    }
  }

  const copyToken = async () => {
    if (!issuedToken) return
    await navigator.clipboard.writeText(issuedToken)
    setCopied(true)
  }

  const customExpiryValid =
    expiry !== 'custom' ||
    (customExpiry.length > 0 && new Date(customExpiry).getTime() > Date.now())

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        {issuedToken ? (
          <>
            <DialogHeader>
              <DialogTitle>{t('dialog.tokenCreatedTitle')}</DialogTitle>
              <DialogDescription>{t('dialog.tokenCreatedDescription')}</DialogDescription>
            </DialogHeader>
            <div className="border-l-2 border-amber-500 bg-amber-50 px-3 py-2 text-sm text-amber-900">
              {t('dialog.tokenShowOnce')}
            </div>
            <div className="flex gap-2">
              <Input className="font-mono" value={issuedToken} readOnly />
              <Button
                variant="outline"
                size="icon"
                onClick={() => void copyToken()}
                aria-label={t('dialog.copyToken')}
              >
                {copied ? <Check /> : <Copy />}
              </Button>
            </div>
            <DialogFooter>
              <Button onClick={() => onOpenChange(false)}>{t('dialog.done')}</Button>
            </DialogFooter>
          </>
        ) : (
          <form onSubmit={createToken}>
            <DialogHeader>
              <DialogTitle>
                {rotating ? t('dialog.rotateTokenTitle') : t('dialog.createTokenTitle')}
              </DialogTitle>
              <DialogDescription>
                {rotating
                  ? t('dialog.rotateTokenDescription')
                  : t('dialog.createTokenDescription')}
              </DialogDescription>
            </DialogHeader>
            <div className="my-5 space-y-4">
              <div className="space-y-2">
                <Label htmlFor="collection-token-name">{t('dialog.tokenName')}</Label>
                <Input
                  id="collection-token-name"
                  value={name}
                  maxLength={100}
                  placeholder={t('dialog.tokenNamePlaceholder')}
                  onChange={(event) => setName(event.target.value)}
                  required
                  autoFocus
                />
              </div>
              <div className="space-y-2">
                <Label>{t('dialog.tokenExpiry')}</Label>
                <Select
                  value={expiry}
                  onValueChange={(value) =>
                    setExpiry((value ?? 'none') as CollectionTokenExpiry)
                  }
                >
                  <SelectTrigger className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="none">{t('dialog.neverExpires')}</SelectItem>
                    <SelectItem value="30">{t('dialog.days', { count: 30 })}</SelectItem>
                    <SelectItem value="90">{t('dialog.days', { count: 90 })}</SelectItem>
                    <SelectItem value="365">{t('dialog.days', { count: 365 })}</SelectItem>
                    <SelectItem value="custom">{t('dialog.customDate')}</SelectItem>
                  </SelectContent>
                </Select>
              </div>
              {expiry === 'custom' ? (
                <div className="space-y-2">
                  <Label htmlFor="collection-token-custom-expiry">
                    {t('dialog.customExpiry')}
                  </Label>
                  <Input
                    id="collection-token-custom-expiry"
                    type="datetime-local"
                    value={customExpiry}
                    onChange={(event) => setCustomExpiry(event.target.value)}
                    required
                  />
                </div>
              ) : null}
            </div>
            {error ? <p className="mb-4 text-sm text-red-600">{error}</p> : null}
            <DialogFooter>
              <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
                {t('dialog.cancel')}
              </Button>
              <Button
                type="submit"
                disabled={creating || name.trim().length === 0 || !customExpiryValid}
              >
                {creating
                  ? t('dialog.generating')
                  : rotating
                    ? t('dialog.rotateToken')
                    : t('dialog.createToken')}
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  )
}
