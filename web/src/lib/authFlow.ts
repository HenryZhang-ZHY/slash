import type { MeResponse } from '@/lib/api'

const githubErrorKeys: Record<string, string> = {
  access_denied: 'auth.githubErrorAccessDenied',
  account_exists: 'auth.githubErrorAccountExists',
  account_unavailable: 'auth.githubErrorAccountUnavailable',
  different_identity_connected: 'auth.githubErrorDifferentIdentity',
  email_unavailable: 'auth.githubErrorEmailUnavailable',
  github_unavailable: 'auth.githubErrorUnavailable',
  identity_in_use: 'auth.githubErrorIdentityInUse',
  invalid_email: 'auth.githubErrorEmailUnavailable',
  invalid_profile: 'auth.githubErrorUnavailable',
  invalid_state: 'auth.githubErrorExpired',
  missing_code: 'auth.githubErrorExpired',
  session_expired: 'auth.githubErrorSessionExpired',
  verified_email_required: 'auth.githubErrorVerifiedEmail',
}

export function destinationFor(me: MeResponse): '/' | '/onboarding' {
  return me.teams.length > 0 ? '/' : '/onboarding'
}

export function githubErrorKey(reason: string | null): string {
  if (!reason) return 'auth.githubErrorGeneric'
  return githubErrorKeys[reason] ?? 'auth.githubErrorGeneric'
}
