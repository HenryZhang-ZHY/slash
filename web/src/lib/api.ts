// Minimal API client for the slash control-plane Web API. Mirrors the
// backend in crates/slash-server/src/api. Sessions ride on an HttpOnly
// cookie (`slash_session`) so the token never reaches JS; these helpers are
// only about payloads / status handling.

export interface User {
  id: string
  email: string | null
  displayName: string
}

export interface Team {
  id: string
  name: string
  slug: string
}

export interface GithubConnection {
  login: string
}

export interface MeResponse {
  user: User
  teams: Team[]
  connections: {
    github: GithubConnection | null
  }
}

export interface UpdatePasswordRequest {
  email: string | null
  currentPassword: string | null
  newPassword: string
}

export interface MetaResponse {
  version: string
}

interface ApiErrorBody {
  message?: string
  error?: string
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
      else if (body?.error) message = body.error
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
  meta: () => request<MetaResponse>('/api/meta'),
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
  updatePassword: (body: UpdatePasswordRequest) =>
    request<void>('/api/auth/password', {
      method: 'PUT',
      body: JSON.stringify(body),
    }),
  createTeam: (name: string, slug: string) =>
    request<{ team: Team }>('/api/teams', {
      method: 'POST',
      body: JSON.stringify({ name, slug }),
    }),
}

export interface AccessToken {
  id: string
  name: string
  createdAt: string
  lastUsedAt: string | null
  expiresAt: string | null
}

export interface IssuedAccessToken {
  accessToken: AccessToken
  token: string
}

export const accessTokenApi = {
  list: () => request<AccessToken[]>('/api/access-tokens'),
  create: (name: string, expiresInDays?: number) =>
    request<IssuedAccessToken>('/api/access-tokens', {
      method: 'POST',
      body: JSON.stringify({ name, expiresInDays }),
    }),
  revoke: (id: string) =>
    request<void>(`/api/access-tokens/${encodeURIComponent(id)}`, {
      method: 'DELETE',
    }),
}

// --- Repository-scoped slash command activity ---

export interface ActivityInstallation {
  id: string
  account: string
  target_type: string
}

export interface ActivityRepository {
  id: string
  name: string
  full_name: string
  owner: string
  private: boolean
}

export type InvocationStatus =
  | 'claimed'
  | 'dispatched'
  | 'correlated'
  | 'completed'
  | 'aborted'
  | 'dispatch_failed'
  | 'correlation_timeout'
  | 'superseded'

export interface CommandInvocation {
  id: string
  attempt: number
  pr_number: number
  head_sha: string
  actor: string
  command: string
  status: InvocationStatus
  conclusion: string | null
  created_at: string
  dispatched_at: string | null
  correlated_at: string | null
  completed_at: string | null
  pull_url: string
  comment_url: string
  check_url: string | null
  workflow_run_url: string | null
}

export interface CursorPage<T> {
  items: T[]
  next_cursor: string | null
}

export interface InvocationFilters {
  installationId: string
  repositoryId: string
  owner: string
  repo: string
  status?: InvocationStatus
  command?: string
  cursor?: string | null
  limit?: number
}

function pageQuery(cursor?: string | null, limit?: number) {
  const params = new URLSearchParams()
  if (cursor) params.set('cursor', cursor)
  if (limit) params.set('limit', String(limit))
  const query = params.toString()
  return query ? `?${query}` : ''
}

export const activityApi = {
  listInstallations: (cursor?: string | null, limit?: number) =>
    request<CursorPage<ActivityInstallation>>(
      `/api/github/installations${pageQuery(cursor, limit)}`,
    ),
  listRepositories: (installationId: string, cursor?: string | null, limit?: number) =>
    request<CursorPage<ActivityRepository>>(
      `/api/github/installations/${encodeURIComponent(installationId)}/repositories${pageQuery(cursor, limit)}`,
    ),
  listInvocations: (filters: InvocationFilters) => {
    const params = new URLSearchParams({
      installation_id: filters.installationId,
      repository_id: filters.repositoryId,
      owner: filters.owner,
      repo: filters.repo,
    })
    if (filters.status) params.set('status', filters.status)
    if (filters.command) params.set('command', filters.command)
    if (filters.cursor) params.set('cursor', filters.cursor)
    if (filters.limit) params.set('limit', String(filters.limit))
    return request<CursorPage<CommandInvocation>>(`/api/invocations?${params}`)
  },
}

// --- Test Engine console API (docs/test-engine.md) ---

export interface TestSuiteSummary {
  id: string
  suite_key: string
  owner: string
  repo: string
  total_tests: number
  muted: number
  skipped: number
  run_count: number
  execution_count: number
  passed_executions: number
  failed_executions: number
  skipped_executions: number
  errored_executions: number
  average_duration_ms: number | null
  last_captured: string | null
}

export interface TestSummary {
  id: string
  name: string
  state: 'enabled' | 'muted' | 'skipped'
  state_source: 'default' | 'manual' | 'monitor'
  state_reason: string | null
  state_changed_by_user_id: string | null
  state_changed_at: string
  file: string | null
  line_no: number | null
  labels: string[]
  owner_team_ids: string[]
  created_at: string
  updated_at: string
  last_status: string | null
  last_captured: string | null
  last_run_ref: string | null
  last_ci_provider: string | null
  execution_count: number
  passed_count: number
  failed_count: number
  skipped_count: number
  errored_count: number
  average_duration_ms: number | null
}

export interface TestExecution {
  id: string
  status: 'passed' | 'failed' | 'skipped' | 'errored'
  duration_ms: number
  stack: string | null
  captured_at: string
  run_id: string
  run_ref: string
  ci_provider: string
  started_at: string
  finished_at: string | null
  invocation_id: string | null
}

export interface TestExecutionPage {
  total: number
  limit: number
  offset: number
  items: TestExecution[]
}

export interface TestRun {
  id: string
  run_ref: string
  ci_provider: string
  invocation_id: string | null
  started_at: string
  finished_at: string | null
  last_captured: string | null
  execution_count: number
  passed_count: number
  failed_count: number
  skipped_count: number
  errored_count: number
  total_duration_ms: number
}

export interface TestRunPage {
  total: number
  limit: number
  offset: number
  items: TestRun[]
}

export interface RunExecution {
  id: string
  test_id: string
  test_name: string
  test_state: TestSummary['state']
  file: string | null
  line_no: number | null
  status: TestExecution['status']
  duration_ms: number
  stack: string | null
  captured_at: string
}

export interface RunExecutionPage {
  total: number
  limit: number
  offset: number
  items: RunExecution[]
}

export interface SuiteCreated {
  suite: TestSuiteSummary
}

export interface CollectionTokenSummary {
  id: string
  name: string
  status: 'active' | 'expired' | 'revoked'
  created_at: string
  expires_at: string | null
  last_used_at: string | null
  revoked_at: string | null
}

export interface IssuedCollectionToken {
  id: string
  name: string
  token: string
  expires_at: string | null
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
  listRuns: (suiteId: string, limit = 100, offset = 0) =>
    request<TestRunPage>(
      `/api/test-engine/suites/${suiteId}/runs?limit=${limit}&offset=${offset}`,
    ),
  listRunExecutions: (runId: string, limit = 100, offset = 0) =>
    request<RunExecutionPage>(
      `/api/test-engine/runs/${runId}/executions?limit=${limit}&offset=${offset}`,
    ),
  listExecutions: (testId: string, limit = 100, offset = 0) =>
    request<TestExecutionPage>(
      `/api/test-engine/tests/${testId}/executions?limit=${limit}&offset=${offset}`,
    ),
  setTestState: (testId: string, state: TestSummary['state']) =>
    request<{ state: TestSummary['state'] }>(`/api/test-engine/tests/${testId}/state`, {
      method: 'PATCH',
      body: JSON.stringify({ state }),
    }),
  listTokens: (suiteId: string) =>
    request<CollectionTokenSummary[]>(`/api/test-engine/suites/${suiteId}/tokens`),
  issueToken: (suiteId: string, name: string, expiresAt: string | null) =>
    request<IssuedCollectionToken>(`/api/test-engine/suites/${suiteId}/tokens`, {
      method: 'POST',
      body: JSON.stringify({ name, expires_at: expiresAt }),
    }),
  revokeToken: (suiteId: string, tokenId: string) =>
    request<void>(`/api/test-engine/suites/${suiteId}/tokens/${tokenId}`, {
      method: 'DELETE',
    }),
}
