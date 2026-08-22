import { describe, expect, it } from 'vitest'

import type { TestRun } from '@/lib/api'
import { runStatus } from '@/lib/test-engine/runs'

function run(overrides: Partial<TestRun> = {}): TestRun {
  return {
    id: 'run-1',
    run_ref: 'local/1',
    ci_provider: 'local',
    invocation_id: null,
    started_at: '2026-08-22T12:00:00Z',
    finished_at: null,
    last_captured: '2026-08-22T12:01:00Z',
    execution_count: 1,
    passed_count: 1,
    failed_count: 0,
    skipped_count: 0,
    errored_count: 0,
    total_duration_ms: 10,
    ...overrides,
  }
}

describe('runStatus', () => {
  it('prioritizes errored and failed executions over passing results', () => {
    expect(runStatus(run({ errored_count: 1, failed_count: 1 }))).toBe('errored')
    expect(runStatus(run({ failed_count: 1 }))).toBe('failed')
  })

  it('reports skipped only when the run has no passing result', () => {
    expect(runStatus(run({ passed_count: 0, skipped_count: 1 }))).toBe('skipped')
    expect(runStatus(run({ passed_count: 1, skipped_count: 1 }))).toBe('passed')
  })

  it('keeps empty runs distinguishable from successful runs', () => {
    expect(runStatus(run({ execution_count: 0, passed_count: 0 }))).toBe('unknown')
  })
})
