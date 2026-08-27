import { describe, expect, it } from 'vitest'

import { activeSection, consoleSections, testEngineLocation, testEngineSearch } from './navigation'

describe('console navigation', () => {
  it('keeps product areas separate from team and account management', () => {
    expect(consoleSections.map((section) => section.id)).toEqual(['product', 'teams'])
    expect(consoleSections[0].items.map((item) => item.to)).toEqual(['/', '/activity', '/tests'])
    expect(consoleSections[1].items).toEqual([])
  })

  it('uses specific page labels for team and product routes', () => {
    expect(activeSection('/teams/platform')).toBe('team')
    expect(activeSection('/tests')).toBe('tests')
    expect(activeSection('/settings')).toBe('account')
  })

  it('round trips Test Engine selections through URL search parameters', () => {
    const location = testEngineLocation(new URLSearchParams('suite=s1&view=runs&run=r1'))
    expect(location).toEqual({ suiteId: 's1', view: 'runs', testId: null, runId: 'r1' })
    expect(testEngineLocation(new URLSearchParams('view=unknown&test=t1')).view).toBe('cases')
    expect(testEngineSearch({ suiteId: 's1', view: 'cases', testId: 't1', runId: 'ignored' }).toString()).toBe('suite=s1&view=cases&test=t1')
  })
})
