import { describe, expect, it } from 'vitest'

import { accountMenuAccessibleName } from '@/lib/productMenu'

describe('accountMenuAccessibleName', () => {
  it('identifies both the menu action and the signed-in account', () => {
    expect(accountMenuAccessibleName('Open account menu', 'reader@example.com')).toBe(
      'Open account menu: reader@example.com',
    )
  })
})
