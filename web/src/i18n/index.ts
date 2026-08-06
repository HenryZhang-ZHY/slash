import i18n from 'i18next'
import { initReactI18next } from 'react-i18next'

import en from './en'
import zhCN from './zh-CN'

export const SUPPORTED_LANGUAGES = [
  { code: 'en', label: 'English' },
  { code: 'zh-CN', label: '简体中文' },
] as const

export type LanguageCode = (typeof SUPPORTED_LANGUAGES)[number]['code']

function detectLanguage(): LanguageCode {
  const saved = localStorage.getItem('slash_lang')
  if (saved === 'en' || saved === 'zh-CN') return saved
  const browser = navigator.language.toLowerCase()
  if (browser.startsWith('zh')) return 'zh-CN'
  return 'en'
}

export const i18nInstance = i18n.use(initReactI18next).init({
  resources: {
    en: { translation: en },
    'zh-CN': { translation: zhCN },
  },
  lng: detectLanguage(),
  fallbackLng: 'en',
  interpolation: { escapeValue: false },
  returnNull: false,
})

export function setLanguage(code: LanguageCode) {
  localStorage.setItem('slash_lang', code)
  void i18n.changeLanguage(code)
}

export function currentLanguage(): LanguageCode {
  return i18n.language === 'zh-CN' ? 'zh-CN' : 'en'
}
