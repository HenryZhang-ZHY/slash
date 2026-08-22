import { describe, expect, it } from 'vitest'

import { normalizeTheme, resolveTheme } from './theme'

describe('theme preference', () => {
  it('normalizes persisted values without trusting unknown input', () => {
    expect(normalizeTheme('light')).toBe('light')
    expect(normalizeTheme('dark')).toBe('dark')
    expect(normalizeTheme('sepia')).toBe('system')
    expect(normalizeTheme(null)).toBe('system')
  })

  it('resolves system preference while preserving explicit choices', () => {
    expect(resolveTheme('system', true)).toBe('dark')
    expect(resolveTheme('system', false)).toBe('light')
    expect(resolveTheme('light', true)).toBe('light')
    expect(resolveTheme('dark', false)).toBe('dark')
  })
})
