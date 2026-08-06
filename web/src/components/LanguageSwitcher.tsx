import { useTranslation } from 'react-i18next'
import { Languages } from 'lucide-react'

import { SUPPORTED_LANGUAGES, currentLanguage, setLanguage } from '@/i18n'

export function LanguageSwitcher() {
  const { t } = useTranslation()
  const active = currentLanguage()

  return (
    <div
      className="flex h-8 items-center gap-1 border bg-white px-1.5"
      role="group"
      aria-label={t('app.language')}
    >
      <Languages className="size-3.5 text-muted-foreground" />
      {SUPPORTED_LANGUAGES.map((lang) => (
        <button
          key={lang.code}
          type="button"
          onClick={() => setLanguage(lang.code)}
          className={`h-6 px-2 text-xs transition-colors ${
            active === lang.code
              ? 'bg-black font-medium text-white'
              : 'text-muted-foreground hover:text-foreground'
          }`}
        >
          {lang.label}
        </button>
      ))}
    </div>
  )
}
