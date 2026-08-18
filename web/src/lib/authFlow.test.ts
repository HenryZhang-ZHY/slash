import { describe, expect, it } from 'vitest'

import { destinationFor, githubErrorKey } from './authFlow'
import type { MeResponse } from './api'

function me(teamCount: number): MeResponse {
  return {
    user: { id: 'user-1', email: 'user@example.com', displayName: 'User' },
    teams: Array.from({ length: teamCount }, (_, index) => ({
      id: `team-${index}`,
      name: `Team ${index}`,
      slug: `team-${index}`,
    })),
    connections: { github: null },
  }
}

describe('authentication product flow', () => {
  it('uses team membership as the single onboarding completion rule', () => {
    expect(destinationFor(me(0))).toBe('/onboarding')
    expect(destinationFor(me(1))).toBe('/')
  })

  it('maps bounded GitHub callback errors without reflecting unknown input', () => {
    expect(githubErrorKey('account_exists')).toBe('auth.githubErrorAccountExists')
    expect(githubErrorKey('identity_in_use')).toBe('auth.githubErrorIdentityInUse')
    expect(githubErrorKey('attacker-controlled-message')).toBe('auth.githubErrorGeneric')
    expect(githubErrorKey(null)).toBe('auth.githubErrorGeneric')
  })
})
