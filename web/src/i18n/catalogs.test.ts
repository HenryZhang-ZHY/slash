import { describe, expect, it } from 'vitest'

import en from './en'
import zhCN from './zh-CN'

describe('translation catalogs', () => {
  it('keeps English and Simplified Chinese keys synchronized', () => {
    expect(Object.keys(zhCN).sort()).toEqual(Object.keys(en).sort())
  })
})
