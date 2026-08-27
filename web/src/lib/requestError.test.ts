import { describe, expect, it } from 'vitest'

import { ApiError } from './api'
import { requestErrorKey } from './requestError'

describe('request error presentation', () => {
  it('does not expose raw authentication transport errors', () => {
    expect(requestErrorKey(new ApiError('not signed in', 401), 'fallback')).toBe('common.browserSessionRequired')
    expect(requestErrorKey(new ApiError('upstream detail', 503), 'fallback')).toBe('common.serviceUnavailable')
    expect(requestErrorKey(new Error('safe detail'), 'fallback')).toBe('fallback')
  })
})
