const STORAGE_KEY = 'slash_pending_team_invitation'

export function invitationTokenFromLocation(): string | null {
  const token = new URLSearchParams(window.location.hash.slice(1)).get('token')
  if (token) sessionStorage.setItem(STORAGE_KEY, token)
  return token ?? sessionStorage.getItem(STORAGE_KEY)
}

export function hasPendingInvitation(): boolean {
  return Boolean(sessionStorage.getItem(STORAGE_KEY))
}

export function clearPendingInvitation(): void {
  sessionStorage.removeItem(STORAGE_KEY)
}
