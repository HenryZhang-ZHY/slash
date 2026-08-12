// Minimal API client for the slash control-plane Web API. Mirrors the
// backend in crates/slash-server/src/api. Sessions ride on an HttpOnly
// cookie (`slash_session`) so the token never reaches JS; these helpers are
// only about payloads / status handling.

export interface User {
  id: string
  email: string
  displayName: string
}

export interface Team {
  id: string
  name: string
  slug: string
}

export interface MeResponse {
  user: User
  teams: Team[]
}

interface ApiErrorBody {
  message?: string
}

export class ApiError extends Error {
  status: number

  constructor(message: string, status: number) {
    super(message)
    this.status = status
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    credentials: 'same-origin',
    headers: init?.body ? { 'Content-Type': 'application/json' } : undefined,
    ...init,
  })
  if (!res.ok) {
    let message = res.statusText
    try {
      const body = (await res.json()) as ApiErrorBody
      if (body?.message) message = body.message
    } catch {
      /* keep statusText */
    }
    throw new ApiError(message, res.status)
  }
  // 204 / empty responses.
  if (res.status === 204) return undefined as T
  return (await res.json()) as T
}

export const api = {
  register: (email: string, password: string) =>
    request<{ user: User }>('/api/auth/register', {
      method: 'POST',
      body: JSON.stringify({ email, password }),
    }),
  login: (email: string, password: string) =>
    request<{ user: User }>('/api/auth/login', {
      method: 'POST',
      body: JSON.stringify({ email, password }),
    }),
  me: () => request<MeResponse>('/api/auth/me'),
  logout: () => request<void>('/api/auth/logout', { method: 'POST' }),
  createTeam: (name: string, slug: string) =>
    request<{ team: Team }>('/api/teams', {
      method: 'POST',
      body: JSON.stringify({ name, slug }),
    }),
}

// --- Test Engine console API (docs/design/1.0-test-engine.md §6) ---

export interface TestSuiteSummary {
  id: string
  suite_key: string
  owner: string
  repo: string
  total_tests: number
  muted: number
  skipped: number
}

export interface TestSummary {
  id: string
  name: string
  state: 'enabled' | 'muted' | 'skipped'
  last_status: string | null
  last_captured: string | null
}

export interface TestExecution {
  id: string
  status: 'passed' | 'failed' | 'skipped' | 'errored'
  duration_ms: number
  captured_at: string
  run_ref: string
  ci_provider: string
}

export interface SuiteCreated {
  suite: TestSuiteSummary
  token: string
}

export const testEngineApi = {
  listSuites: () => request<TestSuiteSummary[]>('/api/test-engine/suites'),
  createSuite: (owner: string, repo: string, suiteKey: string) =>
    request<SuiteCreated>('/api/test-engine/suites', {
      method: 'POST',
      body: JSON.stringify({ owner, repo, suite_key: suiteKey }),
    }),
  listTests: (suiteId: string) =>
    request<TestSummary[]>(`/api/test-engine/suites/${suiteId}/tests`),
  listExecutions: (testId: string) =>
    request<TestExecution[]>(`/api/test-engine/tests/${testId}/executions`),
  getToken: (suiteId: string) =>
    request<{ token: string | null }>(`/api/test-engine/suites/${suiteId}/tokens`),
  issueToken: (suiteId: string) =>
    request<{ token: string }>(`/api/test-engine/suites/${suiteId}/tokens`, {
      method: 'POST',
    }),
}
