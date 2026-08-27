import { useEffect, useState } from 'react'
import { Check, ChevronDown, Info, Languages, LogOut, Monitor, Moon, Settings, SlidersHorizontal, Sun, UserRound } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'

import { ProductMark } from '@/components/ProductMark'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle, SheetTrigger } from '@/components/ui/sheet'
import { SUPPORTED_LANGUAGES, currentLanguage, setLanguage } from '@/i18n'
import { api } from '@/lib/api'
import { accountMenuAccessibleName } from '@/lib/productMenu'
import { initializeTheme, saveTheme, type ThemePreference } from '@/lib/theme'

export function ProductMenu({ accountIdentity, onSignOut }: { accountIdentity?: string; onSignOut?: () => void }) {
  const { t } = useTranslation()
  const [theme, setTheme] = useState<ThemePreference>(() => initializeTheme())
  const [aboutOpen, setAboutOpen] = useState(false)
  const [version, setVersion] = useState<string | null>(null)
  const activeLanguage = currentLanguage()

  useEffect(() => {
    if (!aboutOpen || version) return
    api.meta().then((meta) => setVersion(meta.version)).catch(() => setVersion(t('common.unavailable')))
  }, [aboutOpen, t, version])

  const changeTheme = (next: ThemePreference) => {
    setTheme(next)
    saveTheme(next)
  }

  const themes: Array<{ value: ThemePreference; label: string; icon: typeof Sun }> = [
    { value: 'light', label: t('theme.light'), icon: Sun },
    { value: 'dark', label: t('theme.dark'), icon: Moon },
    { value: 'system', label: t('theme.system'), icon: Monitor },
  ]

  return (
    <>
      <Sheet>
        <SheetTrigger
          render={
            <Button
              variant={accountIdentity ? 'outline' : 'ghost'}
              size={accountIdentity ? 'default' : 'icon-sm'}
              className={accountIdentity ? 'max-w-[min(22rem,60vw)]' : undefined}
              aria-label={accountIdentity ? accountMenuAccessibleName(t('common.accountMenu'), accountIdentity) : t('common.productMenu')}
            />
          }
        >
          {accountIdentity ? (
            <>
              <UserRound />
              <span className="hidden sm:inline">{t('common.account')}</span>
              <span className="hidden text-muted-foreground md:inline" aria-hidden="true">·</span>
              <span className="hidden max-w-40 truncate font-normal text-muted-foreground md:inline">{accountIdentity}</span>
              <ChevronDown className="size-3.5 text-muted-foreground transition-transform group-aria-expanded/button:rotate-180" />
            </>
          ) : <SlidersHorizontal />}
        </SheetTrigger>
        <SheetContent side="right" className="w-full sm:max-w-sm">
          <SheetHeader>
            <SheetTitle>{accountIdentity ? t('common.account') : t('common.preferences')}</SheetTitle>
            <SheetDescription>{accountIdentity ?? t('common.preferencesDescription')}</SheetDescription>
          </SheetHeader>
          <div className="space-y-6 px-4 pb-6">
            {accountIdentity ? <section className="space-y-2"><Button className="w-full justify-start" variant="outline" render={<Link to="/settings" />}><Settings />{t('app.accountSettings')}</Button></section> : null}
            <section>
              <div className="mb-2 flex items-center gap-2 text-xs font-medium uppercase tracking-wide text-muted-foreground"><Languages className="size-4" />{t('app.language')}</div>
              <div className="grid grid-cols-2 gap-2">
                {SUPPORTED_LANGUAGES.map((language) => (
                  <Button key={language.code} variant={activeLanguage === language.code ? 'secondary' : 'outline'} onClick={() => setLanguage(language.code)}>
                    {language.label}{activeLanguage === language.code ? <Check /> : null}
                  </Button>
                ))}
              </div>
            </section>
            <section>
              <div className="mb-2 text-xs font-medium uppercase tracking-wide text-muted-foreground">{t('theme.title')}</div>
              <div className="grid grid-cols-3 gap-2">
                {themes.map(({ value, label, icon: Icon }) => (
                  <Button key={value} className="h-auto flex-col gap-1 py-3" variant={theme === value ? 'secondary' : 'outline'} onClick={() => changeTheme(value)}>
                    <Icon />{label}
                  </Button>
                ))}
              </div>
            </section>
            <Button className="w-full justify-start" variant="outline" onClick={() => setAboutOpen(true)}><Info />{t('about.title')}</Button>
            {onSignOut ? <Button className="w-full justify-start" variant="ghost" onClick={onSignOut}><LogOut />{t('app.signOut')}</Button> : null}
          </div>
        </SheetContent>
      </Sheet>

      <Dialog open={aboutOpen} onOpenChange={setAboutOpen}>
        <DialogContent>
          <DialogHeader>
            <div className="mb-2 flex size-10 items-center justify-center rounded-lg bg-primary text-primary-foreground"><ProductMark className="size-6" /></div>
            <DialogTitle>{t('about.title')}</DialogTitle>
            <DialogDescription>{t('about.description')}</DialogDescription>
          </DialogHeader>
          <div className="rounded-lg border bg-muted/40 p-3">
            <div className="text-xs text-muted-foreground">{t('about.release')}</div>
            <div className="mt-1 font-mono text-sm">Slash {version ?? t('common.loading')}</div>
          </div>
        </DialogContent>
      </Dialog>
    </>
  )
}
