import { describe, expect, it } from 'vitest'

import { resolveCollectionTokenExpiry } from './collectionTokenExpiry'

const NOW = new Date('2026-08-22T10:00:00.000Z')

describe('resolveCollectionTokenExpiry', () => {
  it('uses no expiry by default', () => {
    expect(resolveCollectionTokenExpiry('none', '', NOW)).toBeNull()
  })

  it('turns a day preset into an absolute timestamp', () => {
    expect(resolveCollectionTokenExpiry('90', '', NOW)).toBe('2026-11-20T10:00:00.000Z')
  })

  it('preserves a custom local date as an absolute timestamp', () => {
    const custom = '2026-12-01T12:30'

    expect(resolveCollectionTokenExpiry('custom', custom, NOW)).toBe(
      new Date(custom).toISOString(),
    )
  })
})
