import type { TestExecution, TestRun } from '@/lib/api'

export function runStatus(run: TestRun): TestExecution['status'] | 'unknown' {
  if (run.errored_count > 0) return 'errored'
  if (run.failed_count > 0) return 'failed'
  if (run.passed_count > 0) return 'passed'
  if (run.skipped_count > 0) return 'skipped'
  return 'unknown'
}
