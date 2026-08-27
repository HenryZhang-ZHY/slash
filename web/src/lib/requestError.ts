import { ApiError } from './api'

export function requestErrorKey(error: unknown, fallback: string) {
  if (!(error instanceof ApiError)) return fallback
  if (error.status === 401) return 'common.browserSessionRequired'
  if (error.status === 403) return 'common.permissionRequired'
  if (error.status >= 500) return 'common.serviceUnavailable'
  return fallback
}
