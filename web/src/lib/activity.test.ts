import { describe, expect, it } from 'vitest'

import { invocationDurationMs, invocationOutcome } from './activity'

describe('invocation activity formatting', () => {
  it('uses the terminal conclusion as the visible outcome', () => {
    expect(invocationOutcome({ status: 'completed', conclusion: 'success' })).toBe('success')
    expect(invocationOutcome({ status: 'correlated', conclusion: null })).toBe('correlated')
  })

  it('measures terminal and in-flight durations without producing negatives', () => {
    expect(invocationDurationMs('2026-08-27T10:00:00Z', '2026-08-27T10:00:02.250Z')).toBe(2250)
    expect(invocationDurationMs('2026-08-27T10:00:02Z', '2026-08-27T10:00:00Z')).toBe(0)
  })
})
