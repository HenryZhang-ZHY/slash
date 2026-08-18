import { ApiError } from '@/lib/api'

export interface AdminOverview {
  active_installations: number
  personal_installations: number
  organization_installations: number
  suspended_installations: number
  registered_users: number
  deliveries_24h: number
  failed_deliveries_24h: number
  pending_deliveries: number
  oldest_pending_seconds: number | null
  invocations_24h: number
  failed_invocations_24h: number
  running_invocations: number
  last_installation_sync_at: string | null
}

export interface AdminInstallation {
  installation_id: number
  account: string
  target_type: string
  state: 'active' | 'suspended' | 'deleted'
  installed_at: string | null
  updated_at: string
  last_synced_at: string | null
}

export interface AdminDelivery {
  delivery_guid: string
  event: string
  action: string | null
  repository: string | null
  received_at: string
  processed_at: string | null
  state: 'pending' | 'done' | 'failed'
  attempts: number
  last_error: string | null
}

export interface AdminInvocation {
  id: string
  delivery_guid: string | null
  owner: string
  repo: string
  pr_number: number
  actor: string
  command: string
  raw_comment_line: string
  check_run_id: number | null
  workflow_run_id: number | null
  status: string
  conclusion: string | null
  failure_reason: string | null
  created_at: string
  completed_at: string | null
}

export interface AdminDeliveryDetail {
  delivery: AdminDelivery
  payload: unknown
  related_invocations: AdminInvocation[]
}

async function adminRequest<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    credentials: 'same-origin',
    headers: init?.body ? { 'Content-Type': 'application/json' } : undefined,
    ...init,
  })
  if (!response.ok) {
    let message = response.statusText
    try {
      const body = (await response.json()) as { error?: string }
      message = body.error ?? message
    } catch {
      // Keep the HTTP status text for non-JSON failures.
    }
    throw new ApiError(message, response.status)
  }
  if (response.status === 204) return undefined as T
  return (await response.json()) as T
}

export const adminApi = {
  session: () => adminRequest<{ authenticated: true }>('/api/admin/auth/session'),
  login: (secret: string) =>
    adminRequest<{ authenticated: true }>('/api/admin/auth/login', {
      method: 'POST',
      body: JSON.stringify({ secret }),
    }),
  logout: () => adminRequest<void>('/api/admin/auth/logout', { method: 'POST' }),
  overview: () => adminRequest<AdminOverview>('/api/admin/overview'),
  installations: () => adminRequest<AdminInstallation[]>('/api/admin/installations'),
  refreshInstallations: () =>
    adminRequest<{ refreshed: boolean; installation_count: number; last_success_at: string }>(
      '/api/admin/installations/refresh',
      { method: 'POST' },
    ),
  deliveries: () => adminRequest<AdminDelivery[]>('/api/admin/deliveries'),
  delivery: (guid: string) =>
    adminRequest<AdminDeliveryDetail>(`/api/admin/deliveries/${encodeURIComponent(guid)}`),
  invocations: () => adminRequest<AdminInvocation[]>('/api/admin/invocations'),
}
