export type ThemePreference = 'light' | 'dark' | 'system'

export const THEME_STORAGE_KEY = 'slash_theme'
let watchingSystemTheme = false

export function normalizeTheme(value: string | null): ThemePreference {
  return value === 'light' || value === 'dark' ? value : 'system'
}

export function resolveTheme(theme: ThemePreference, prefersDark: boolean): 'light' | 'dark' {
  return theme === 'system' ? (prefersDark ? 'dark' : 'light') : theme
}

export function applyTheme(theme: ThemePreference) {
  const dark = resolveTheme(theme, window.matchMedia('(prefers-color-scheme: dark)').matches) === 'dark'
  document.documentElement.classList.toggle('dark', dark)
  document.documentElement.style.colorScheme = dark ? 'dark' : 'light'
}

export function initializeTheme(): ThemePreference {
  const theme = normalizeTheme(localStorage.getItem(THEME_STORAGE_KEY))
  applyTheme(theme)
  if (!watchingSystemTheme) {
    watchingSystemTheme = true
    window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
      const current = normalizeTheme(localStorage.getItem(THEME_STORAGE_KEY))
      if (current === 'system') applyTheme(current)
    })
  }
  return theme
}

export function saveTheme(theme: ThemePreference) {
  localStorage.setItem(THEME_STORAGE_KEY, theme)
  applyTheme(theme)
}
