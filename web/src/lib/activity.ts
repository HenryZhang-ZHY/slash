import type { CommandInvocation } from './api'

export function invocationOutcome(
  invocation: Pick<CommandInvocation, 'status' | 'conclusion'>,
) {
  return invocation.conclusion ?? invocation.status
}

export function invocationDurationMs(startedAt: string, finishedAt: string) {
  return Math.max(0, Date.parse(finishedAt) - Date.parse(startedAt))
}
