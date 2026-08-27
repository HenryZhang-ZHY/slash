export const consoleSections = [
  {
    id: 'product',
    items: [
      { id: 'overview', to: '/', end: true },
      { id: 'activity', to: '/activity', end: false },
      { id: 'tests', to: '/tests', end: false },
    ],
  },
  { id: 'teams', items: [] },
] as const

export function activeSection(pathname: string) {
  if (pathname.startsWith('/activity')) return 'activity'
  if (pathname.startsWith('/tests')) return 'tests'
  if (pathname.startsWith('/settings')) return 'account'
  if (pathname.startsWith('/teams/')) return 'team'
  return 'overview'
}

export type TestEngineView = 'cases' | 'runs'

export function testEngineLocation(search: URLSearchParams) {
  const requestedView = search.get('view')
  return {
    suiteId: search.get('suite'),
    view: requestedView === 'runs' ? 'runs' as const : 'cases' as const,
    testId: search.get('test'),
    runId: search.get('run'),
  }
}
